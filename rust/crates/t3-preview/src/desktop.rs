use crate::{Error, Result, driver::TabKey, lock, manager::Events};
use async_trait::async_trait;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};
use t3_contracts::{desktop_browser::*, preview_automation::PreviewAutomationLoadError};
use tokio::sync::mpsc;

pub const PREVIEW_STOP_LOADING_CHANNEL: &str = "desktop:preview-stop-loading";

/// The native shell owns bootstrap descriptors and the authenticated one-client CDP relay.
#[async_trait]
pub trait DesktopIpc: Send + Sync {
    /// Invoke the shell bridge on the guest webContents; preserve the embedder and relay.
    async fn stop_loading(&self, _tab_id: &str) -> Result<()> {
        Err(Error::operation(
            "PreviewDesktopBridgeUnavailableError",
            "The desktop shell has not implemented stopLoading.",
        ))
    }
    async fn send(&self, command: DesktopBrowserCommand) -> Result<()>;
    async fn relay(
        &self,
        key: TabKey,
        incoming: mpsc::UnboundedReceiver<String>,
    ) -> Result<DesktopEndpoint>;
}
pub struct DesktopEndpoint {
    pub url: String,
    pub lease: Box<dyn Send + Sync>,
}

#[derive(Clone, Debug)]
pub struct DesktopChange {
    pub key: TabKey,
    pub attached: bool,
}
#[derive(Default)]
struct State {
    attached: BTreeSet<TabKey>,
    shown: BTreeMap<TabKey, bool>,
    failures: BTreeMap<TabKey, PreviewAutomationLoadError>,
    inbound: BTreeMap<TabKey, (String, mpsc::UnboundedSender<String>)>,
}
#[derive(Clone, Default)]
pub struct DesktopBrowserChannel {
    ipc: Option<Arc<dyn DesktopIpc>>,
    state: Arc<Mutex<State>>,
    changes: Events<DesktopChange>,
}

impl DesktopBrowserChannel {
    pub fn new(ipc: Arc<dyn DesktopIpc>) -> Self {
        Self {
            ipc: Some(ipc),
            ..Self::default()
        }
    }
    pub fn available(&self) -> bool {
        self.ipc.is_some()
    }
    pub fn is_attached(&self, key: &TabKey) -> bool {
        lock(&self.state).attached.contains(key)
    }
    pub fn is_shown(&self, key: &TabKey) -> bool {
        self.available() && lock(&self.state).shown.get(key).copied().unwrap_or(true)
    }
    pub fn load_failure(&self, key: &TabKey) -> Option<PreviewAutomationLoadError> {
        lock(&self.state).failures.get(key).cloned()
    }
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<DesktopChange> {
        self.changes.subscribe()
    }
    pub fn receive(&self, event: DesktopBrowserEvent) {
        let mut state = lock(&self.state);
        match event {
            DesktopBrowserEvent::Attached { thread_id, tab_id } => {
                let key = TabKey::new(thread_id.0, tab_id.0);
                state.attached.insert(key.clone());
                self.changes.publish(DesktopChange {
                    key,
                    attached: true,
                });
            }
            DesktopBrowserEvent::Detached { thread_id, tab_id } => {
                let key = TabKey::new(thread_id.0, tab_id.0);
                state.attached.remove(&key);
                state.shown.remove(&key);
                state.failures.remove(&key);
                state.inbound.remove(&key);
                self.changes.publish(DesktopChange {
                    key,
                    attached: false,
                });
            }
            DesktopBrowserEvent::Shown {
                thread_id,
                tab_id,
                shown,
            } => {
                state
                    .shown
                    .insert(TabKey::new(thread_id.0, tab_id.0), shown);
            }
            DesktopBrowserEvent::LoadFailed {
                thread_id,
                tab_id,
                url,
                code,
                description,
            } => {
                state.failures.insert(
                    TabKey::new(thread_id.0, tab_id.0),
                    PreviewAutomationLoadError {
                        url: url.chars().take(2048).collect(),
                        code,
                        description,
                    },
                );
            }
            DesktopBrowserEvent::Cdp {
                thread_id,
                tab_id,
                message,
            } => {
                if let Some((_, tx)) = state.inbound.get(&TabKey::new(thread_id.0, tab_id.0)) {
                    let _ = tx.send(message);
                }
            }
        }
    }
    pub fn disconnect(&self) {
        let mut state = lock(&self.state);
        let keys = std::mem::take(&mut state.attached);
        state.shown.clear();
        state.failures.clear();
        state.inbound.clear();
        for key in keys {
            self.changes.publish(DesktopChange {
                key,
                attached: false,
            });
        }
    }
    pub async fn await_attached(&self, key: &TabKey, timeout: Duration) -> bool {
        if !self.available() {
            return false;
        }
        let mut changes = self.subscribe();
        if self.is_attached(key) {
            return true;
        }
        tokio::time::timeout(timeout, async {
            while let Some(change) = changes.recv().await {
                if change.attached && &change.key == key {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false)
    }
    pub async fn endpoint(&self, key: &TabKey) -> Result<DesktopConnection> {
        let ipc = self.ipc.as_ref().ok_or(Error::Disconnected)?;
        let (tx, rx) = mpsc::unbounded_channel();
        let token = uuid::Uuid::new_v4().to_string();
        {
            let mut state = lock(&self.state);
            if !state.attached.contains(key) || state.inbound.contains_key(key) {
                return Err(Error::Disconnected);
            }
            state.inbound.insert(key.clone(), (token.clone(), tx));
        }
        let registration = InboundLease {
            channel: self.clone(),
            key: key.clone(),
            token,
        };
        let endpoint = tokio::time::timeout(Duration::from_secs(5), ipc.relay(key.clone(), rx))
            .await
            .map_err(|_| Error::Timeout("desktop CDP relay".into()))??;
        if !self.is_attached(key) {
            return Err(Error::Disconnected);
        }
        Ok(DesktopConnection {
            endpoint,
            _registration: registration,
        })
    }
    pub async fn stop_loading(&self, key: &TabKey) -> Result<()> {
        if !self.is_attached(key) {
            return Err(Error::TabNotFound);
        }
        let ipc = self.ipc.as_ref().ok_or(Error::Disconnected)?;
        tokio::time::timeout(Duration::from_secs(5), ipc.stop_loading(&key.tab_id))
            .await
            .map_err(|_| Error::Timeout("desktop stopLoading".into()))?
    }
    pub async fn command(&self, command: DesktopBrowserCommand) {
        if let Some(ipc) = &self.ipc
            && let Err(error) = tokio::time::timeout(Duration::from_secs(5), ipc.send(command))
                .await
                .map_err(|_| Error::Timeout("desktop IPC".into()))
                .and_then(|result| result)
        {
            tracing::warn!(%error,"desktop browser command failed");
        }
    }
    pub async fn turn(&self, thread: &str, active: bool) {
        self.command(DesktopBrowserCommand::Turn {
            thread_id: thread.into(),
            active,
        })
        .await;
    }
}

pub struct DesktopConnection {
    pub endpoint: DesktopEndpoint,
    _registration: InboundLease,
}
struct InboundLease {
    channel: DesktopBrowserChannel,
    key: TabKey,
    token: String,
}
impl Drop for InboundLease {
    fn drop(&mut self) {
        let mut state = lock(&self.channel.state);
        if !state
            .inbound
            .get(&self.key)
            .is_some_and(|(token, _)| token == &self.token)
        {
            return;
        }
        state.inbound.remove(&self.key);
        drop(state);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let channel = self.channel.clone();
            let key = self.key.clone();
            runtime.spawn(async move {
                channel
                    .command(DesktopBrowserCommand::Release {
                        thread_id: key.thread_id.into(),
                        tab_id: key.tab_id.into(),
                    })
                    .await;
            });
        }
    }
}
