use std::{
    collections::HashMap,
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot, watch},
};

use crate::{
    ClientHandler, Error, Event, Generation, Negotiated, RpcError, SessionState, compat,
    transport::{self, Runtime, lock},
};

#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub event_capacity: usize,
    pub max_line_bytes: usize,
    pub max_concurrent_handlers: usize,
    pub load_timeout: Duration,
    pub replay_idle_gap: Duration,
    pub cancel_timeout: Duration,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            event_capacity: 4096,
            max_line_bytes: 16 * 1024 * 1024,
            max_concurrent_handlers: 64,
            load_timeout: Duration::from_secs(90),
            replay_idle_gap: Duration::from_secs(2),
            cancel_timeout: Duration::from_secs(15),
        }
    }
}

#[derive(Clone)]
pub struct Client {
    runtime: Arc<Runtime>,
    options: ClientOptions,
    negotiated: Arc<Mutex<Option<Negotiated>>>,
    active: Arc<Mutex<HashMap<String, Vec<watch::Receiver<bool>>>>>,
}

pub struct Events {
    receiver: mpsc::Receiver<transport::Delivery>,
    acknowledged: Option<oneshot::Sender<()>>,
    shared: Arc<transport::Shared>,
    terminal: watch::Receiver<Option<Error>>,
    ended: bool,
}

impl Events {
    /// Confirms the last event has been handled, allowing later wire requests and replies.
    pub fn acknowledge(&mut self) {
        if let Some(tx) = self.acknowledged.take() {
            let _ = tx.send(());
        }
    }

    /// Acknowledges the previous event, then drains admitted events before the terminal error.
    pub async fn next(&mut self) -> Option<Result<Event, Error>> {
        self.acknowledge();
        if self.ended {
            return None;
        }
        loop {
            if let Ok(event) = self.receiver.try_recv() {
                self.acknowledged = event.acknowledged;
                return Some(Ok(event.event));
            }
            if let Some(error) = self.terminal.borrow().clone() {
                self.ended = true;
                return Some(Err(error));
            }
            tokio::select! {
                biased;
                event = self.receiver.recv() => if let Some(event) = event { self.acknowledged = event.acknowledged;
                return Some(Ok(event.event)); },
                _ = self.terminal.changed() => {},
            }
        }
    }
}

impl Drop for Events {
    fn drop(&mut self) {
        self.shared.terminate(Error::Closed);
    }
}

impl Client {
    pub fn connect<R, W>(
        reader: R,
        writer: W,
        handler: Arc<dyn ClientHandler>,
        options: ClientOptions,
    ) -> (Self, Events)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (runtime, receiver) = transport::connect(reader, writer, handler, &options);
        let terminal = runtime.shared.terminal.subscribe();
        (
            Self {
                runtime: runtime.clone(),
                options,
                negotiated: Arc::new(Mutex::new(None)),
                active: Arc::new(Mutex::new(HashMap::new())),
            },
            Events {
                receiver,
                acknowledged: None,
                shared: runtime.shared.clone(),
                terminal,
                ended: false,
            },
        )
    }

    pub fn negotiated(&self) -> Option<Negotiated> {
        lock(&self.negotiated).clone()
    }

    pub fn generation(&self) -> Generation {
        self.runtime.shared.generation()
    }

    pub fn session_state(&self, session_id: &str) -> SessionState {
        lock(&self.runtime.shared.sessions)
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn raw_notifications(&self) -> crate::RawNotifications {
        crate::RawNotifications {
            queue: self.runtime.shared.raw.clone(),
            terminal: self.runtime.shared.terminal.subscribe(),
        }
    }

    pub async fn shutdown(&self) {
        self.runtime.shutdown().await;
    }

    /// Raw JSON-RPC escape hatch, including underscore-prefixed vendor methods.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, Error> {
        self.runtime.shared.request(method, params).await
    }

    pub async fn request_as<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<R, Error> {
        let params = serde_json::to_value(params).map_err(|_| Error::Schema(method.into()))?;
        serde_json::from_value(self.request(method, params).await?)
            .map_err(|_| Error::Schema(method.into()))
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<(), Error> {
        self.runtime.shared.notify(method, params).await
    }

    pub async fn initialize(&self, params: Value) -> Result<Negotiated, Error> {
        if !params.is_object() {
            return Err(RpcError::invalid_params().into());
        }
        let raw = self
            .request("initialize", compat::initialize_request(params))
            .await?;
        if !raw["protocolVersion"].is_number() {
            return Err(Error::Schema("initialize".into()));
        }
        let negotiated = compat::negotiate(raw);
        self.runtime.shared.generation.store(
            if negotiated.generation == Generation::V2 {
                2
            } else {
                1
            },
            Ordering::Release,
        );
        *lock(&self.negotiated) = Some(negotiated.clone());
        Ok(negotiated)
    }

    pub async fn authenticate(&self, params: Value) -> Result<Value, Error> {
        self.request(
            if self.generation() == Generation::V1 {
                "authenticate"
            } else {
                "auth/login"
            },
            params,
        )
        .await
    }

    pub async fn logout(&self, params: Value) -> Result<Value, Error> {
        self.request(
            if self.generation() == Generation::V1 {
                "logout"
            } else {
                "auth/logout"
            },
            params,
        )
        .await
    }

    async fn setup(&self, method: &str, params: Value) -> Result<Value, Error> {
        if !params.is_object() {
            return Err(RpcError::invalid_params().into());
        }
        let session_id = params["sessionId"].as_str().map(str::to_owned);
        if let Some(id) = &session_id {
            lock(&self.runtime.shared.sessions)
                .entry(id.clone())
                .or_default();
        }
        let before = session_id
            .as_ref()
            .map(|id| self.session_state(id).revision)
            .unwrap_or(0);
        let v2 = self.generation() == Generation::V2;
        let mut response = self
            .request(
                method,
                if v2 {
                    compat::setup_request(params)
                } else {
                    params
                },
            )
            .await?;
        if v2 {
            compat::config_options(&mut response);
        }
        if let Some(id) = response["sessionId"]
            .as_str()
            .map(str::to_owned)
            .or(session_id)
        {
            let mut sessions = lock(&self.runtime.shared.sessions);
            let state = sessions.entry(id).or_default();
            if state.revision == before {
                state.config_options = response
                    .get("configOptions")
                    .filter(|v| !v.is_null())
                    .cloned();
            }
            state.modes = response.get("modes").filter(|v| v.is_object()).cloned();
            state.models = response.get("models").filter(|v| v.is_object()).cloned();
        }
        Ok(response)
    }

    pub async fn new_session(&self, params: Value) -> Result<Value, Error> {
        self.setup("session/new", params).await
    }

    pub async fn load_session(&self, mut params: Value) -> Result<Value, Error> {
        if !params.is_object() {
            return Err(RpcError::invalid_params().into());
        }
        let session_id = params["sessionId"]
            .as_str()
            .ok_or_else(RpcError::invalid_params)?
            .to_owned();
        let (tx, mut activity) = watch::channel(None);
        {
            let mut loads = lock(&self.runtime.shared.loads);
            if loads.contains_key(&session_id) {
                return Err(Error::Schema("session already loading".into()));
            }
            loads.insert(session_id.clone(), tx);
        }
        let _guard = LoadGuard {
            client: self,
            session_id: session_id.clone(),
        };
        let method = if self.generation() == Generation::V2 {
            // Draft-v2 agents replaced load with resume plus explicit history replay.
            params["replayFrom"] = json!({"type":"start"});
            "session/resume"
        } else {
            "session/load"
        };
        let idle = async {
            loop {
                let last = *activity.borrow_and_update();
                if let Some(last) = last {
                    tokio::select! {
                        biased;
                        _ = activity.changed() => {},
                        _ = tokio::time::sleep_until(last + self.options.replay_idle_gap) => break,
                    }
                } else {
                    let _ = activity.changed().await;
                }
            }
            // Agents can finish replay without replying to load; the idle gap establishes readiness.
            let mut response = json!({"_meta":{"t3SessionLoadReady":"replay_idle"}});
            if let Some(options) = self.session_state(&session_id).config_options {
                response["configOptions"] = options;
            }
            if let Some(mode) = self
                .negotiated()
                .and_then(|n| n.response.pointer("/_meta/modeState").cloned())
            {
                response["modes"] = mode;
            }
            Ok(response)
        };
        tokio::time::timeout(self.options.load_timeout, async {
            tokio::select! { biased; result = self.setup(method, params) => result, result = idle => result }
        }).await.map_err(|_| Error::Timeout("session load"))?
    }

    pub async fn resume_session(&self, params: Value) -> Result<Value, Error> {
        self.setup("session/resume", params).await
    }
    pub async fn fork_session(&self, params: Value) -> Result<Value, Error> {
        self.setup("session/fork", params).await
    }
    pub async fn list_sessions(&self, params: Value) -> Result<Value, Error> {
        self.request("session/list", params).await
    }
    pub async fn close_session(&self, params: Value) -> Result<Value, Error> {
        let id = params["sessionId"].as_str().map(str::to_owned);
        let result = self.request("session/close", params).await?;
        if let Some(id) = id {
            lock(&self.runtime.shared.sessions).remove(&id);
        }
        Ok(result)
    }

    pub async fn delete_session(&self, params: Value) -> Result<Value, Error> {
        if !self.negotiated().is_some_and(|n| {
            n.response
                .pointer("/agentCapabilities/sessionCapabilities/delete")
                .is_some_and(|v| !v.is_null())
        }) {
            return Err(RpcError::method_not_found("session/delete").into());
        }
        let id = params["sessionId"].as_str().map(str::to_owned);
        let result = self.request("session/delete", params).await?;
        if let Some(id) = id {
            lock(&self.runtime.shared.sessions).remove(&id);
        }
        Ok(result)
    }

    pub async fn list_providers(&self, params: Value) -> Result<Value, Error> {
        self.v2_request("providers/list", params).await
    }
    pub async fn set_provider(&self, params: Value) -> Result<Value, Error> {
        self.v2_request("providers/set", params).await
    }
    pub async fn disable_provider(&self, params: Value) -> Result<Value, Error> {
        self.v2_request("providers/disable", params).await
    }

    async fn v2_request(&self, method: &str, params: Value) -> Result<Value, Error> {
        if self.generation() != Generation::V2 {
            return Err(RpcError::method_not_found(method).into());
        }
        self.request(method, params).await
    }

    pub async fn set_mode(&self, params: Value) -> Result<Value, Error> {
        self.v1_request("session/set_mode", params).await
    }
    pub async fn set_model(&self, params: Value) -> Result<Value, Error> {
        self.v1_request("session/set_model", params).await
    }

    async fn v1_request(&self, method: &str, params: Value) -> Result<Value, Error> {
        if self.generation() != Generation::V1 {
            return Err(RpcError::method_not_found(method).into());
        }
        self.request(method, params).await
    }

    pub async fn set_config_option(&self, mut params: Value) -> Result<Value, Error> {
        if !params.is_object() {
            return Err(RpcError::invalid_params().into());
        }
        let session_id = params["sessionId"]
            .as_str()
            .ok_or_else(RpcError::invalid_params)?
            .to_owned();
        lock(&self.runtime.shared.sessions)
            .entry(session_id.clone())
            .or_default();
        let before = self.session_state(&session_id).revision;
        let v2 = self.generation() == Generation::V2;
        if v2 {
            params["type"] = json!(if params["value"].is_boolean() {
                "boolean"
            } else {
                "id"
            });
        }
        let mut result = self.request("session/set_config_option", params).await?;
        if v2 {
            compat::config_options(&mut result);
            if result.get("configOptions").is_none() {
                result["configOptions"] = json!([]);
            }
        }
        let mut sessions = lock(&self.runtime.shared.sessions);
        let state = sessions.entry(session_id).or_default();
        // Agents can push newer options before their stale set_config_option response is resumed.
        if state.revision == before {
            state.config_options = result.get("configOptions").cloned();
        }
        Ok(result)
    }

    pub async fn prompt(&self, params: Value) -> Result<Value, Error> {
        let session_id = params["sessionId"]
            .as_str()
            .ok_or_else(RpcError::invalid_params)?
            .to_owned();
        let (done, receiver) = watch::channel(false);
        {
            let mut active = lock(&self.active);
            if self.generation() != Generation::V1 && active.contains_key(&session_id) {
                return Err(Error::PromptActive(session_id));
            }
            active.entry(session_id.clone()).or_default().push(receiver);
        }
        let _guard = PromptGuard {
            client: self,
            session_id: session_id.clone(),
            done,
        };
        if self.generation() == Generation::V1 {
            // fx returns invalid empty usage on cancellation; the stop reason remains authoritative.
            let result = self.request("session/prompt", params).await?;
            if !result["stopReason"].is_string() {
                return Err(Error::Schema("session/prompt".into()));
            }
            return Ok(result);
        }
        let (tx, rx) = oneshot::channel();
        lock(&self.runtime.shared.prompts).insert(session_id, tx);
        self.runtime.shared.check()?;
        // Draft-v2 agents acknowledge prompts early; completion arrives on the idle update.
        self.request("session/prompt", params).await?;
        rx.await.map_err(|_| Error::Closed)?
    }

    /// Returns only after cancel has been flushed; it does not interrupt the prompt RPC.
    pub async fn cancel(&self, params: Value) -> Result<(), Error> {
        self.notify("session/cancel", params).await
    }

    pub async fn cancel_and_wait(&self, params: Value) -> Result<(), Error> {
        let session_id = params["sessionId"]
            .as_str()
            .ok_or_else(RpcError::invalid_params)?;
        let active = lock(&self.active).get(session_id).cloned();
        self.cancel(params.clone()).await?;
        if let Some(active) = active {
            tokio::time::timeout(self.options.cancel_timeout, async move {
                for mut prompt in active {
                    prompt.wait_for(|done| *done).await?;
                }
                Ok::<_, watch::error::RecvError>(())
            })
            .await
            .map_err(|_| Error::Timeout("cancellation"))?
            .map_err(|_| Error::Closed)?;
        }
        Ok(())
    }
}

struct LoadGuard<'a> {
    client: &'a Client,
    session_id: String,
}
impl Drop for LoadGuard<'_> {
    fn drop(&mut self) {
        lock(&self.client.runtime.shared.loads).remove(&self.session_id);
    }
}

struct PromptGuard<'a> {
    client: &'a Client,
    session_id: String,
    done: watch::Sender<bool>,
}
impl Drop for PromptGuard<'_> {
    fn drop(&mut self) {
        lock(&self.client.runtime.shared.prompts).remove(&self.session_id);
        let mut active = lock(&self.client.active);
        if let Some(prompts) = active.get_mut(&self.session_id) {
            prompts.retain(|prompt| !prompt.same_channel(&self.done.subscribe()));
            if prompts.is_empty() {
                active.remove(&self.session_id);
            }
        }
        self.done.send_replace(true);
    }
}
