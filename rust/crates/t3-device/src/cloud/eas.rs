use super::expo::{Create, ExpoApi, ExpoProvider, ExpoResult, Session};
use crate::{
    Error, Result, agent,
    host::{
        self, AgentEndpoint, DeviceHost, DeviceOperations, Listing, OpenContext, Phase, Ready,
        Routing, SessionRelease, Upstream,
    },
    io::{
        Command, CommandOutput, Commands, Http, HttpRequest, Secret, json_request, request_bytes,
    },
};
use async_trait::async_trait;
use http::Method;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use t3_contracts::device::*;
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEntry {
    pub name: String,
    pub app_id: String,
    pub recorded_at: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub refused: bool,
}
#[derive(Clone)]
struct Running {
    device: DeviceSummary,
    name: String,
    remote_id: String,
    origin: String,
    token: Secret,
    agent: AgentEndpoint,
    stopping: bool,
    checked: u64,
}
#[derive(Clone)]
struct Start {
    cancel: CancellationToken,
    done: watch::Receiver<bool>,
}
struct State {
    sessions: BTreeMap<String, Running>,
    starting: BTreeMap<String, Start>,
    unresolved: BTreeMap<String, String>,
    uncertain: BTreeMap<String, (LedgerEntry, u64)>,
    last_api: Option<Arc<dyn ExpoApi>>,
    ready: bool,
    epoch: u64,
    closed: bool,
    reaped: u64,
}
struct Inner {
    config: EasDeviceHostConfig,
    ledger: PathBuf,
    provider: Arc<dyn ExpoProvider>,
    http: Arc<dyn Http>,
    state: Mutex<State>,
    ledger_lock: Mutex<()>,
    reap_lock: Mutex<()>,
    started: tokio::time::Instant,
    wall: u64,
}
#[derive(Clone)]
pub struct EasHost {
    inner: Arc<Inner>,
}
impl EasHost {
    pub fn new(
        config: EasDeviceHostConfig,
        state: PathBuf,
        provider: Arc<dyn ExpoProvider>,
        http: Arc<dyn Http>,
    ) -> Result<Self> {
        super::validate_config(&DeviceHostConfig::EasDeviceHostConfig(config.clone()))?;
        Ok(Self {
            inner: Arc::new(Inner {
                ledger: state
                    .join("device/eas")
                    .join(format!("{}.json", config.id.as_ref())),
                config,
                provider,
                http,
                state: Mutex::new(State {
                    sessions: BTreeMap::new(),
                    starting: BTreeMap::new(),
                    unresolved: BTreeMap::new(),
                    uncertain: BTreeMap::new(),
                    last_api: None,
                    ready: false,
                    epoch: 0,
                    closed: false,
                    reaped: 0,
                }),
                ledger_lock: Mutex::new(()),
                reap_lock: Mutex::new(()),
                started: tokio::time::Instant::now(),
                wall: chrono::Utc::now().timestamp_millis().max(0) as u64,
            }),
        })
    }
    fn now(&self) -> u64 {
        self.inner.wall
            + self
                .inner
                .started
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64
    }
    fn unavailable(&self, reason: impl Into<String>) -> Error {
        Error::host(self.id(), Some(self.label()), reason)
    }
    async fn api(&self, stopping: bool) -> Result<Arc<dyn ExpoApi>> {
        match self.inner.provider.api().await {
            Ok(api) => {
                self.inner.state.lock().await.last_api = Some(api.clone());
                Ok(api)
            }
            Err(e) => {
                if stopping {
                    self.inner.state.lock().await.last_api.clone().ok_or(e)
                } else {
                    Err(e)
                }
            }
        }
    }
    async fn read_ledger(&self) -> Result<Vec<LedgerEntry>> {
        match tokio::fs::read(&self.inner.ledger).await {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| {
                self.unavailable(
                    "The record of running EAS sessions could not be read. Fix or remove it.",
                )
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
            Err(_) => Err(self.unavailable(
                "The record of running EAS sessions could not be read. Fix or remove it.",
            )),
        }
    }
    async fn ledger(&self, update: impl FnOnce(&mut Vec<LedgerEntry>)) -> Result<()> {
        let _guard = self.inner.ledger_lock.lock().await;
        if self.inner.state.lock().await.closed {
            return Err(self.unavailable("This EAS host was replaced or removed."));
        }
        let mut entries = self.read_ledger().await?;
        update(&mut entries);
        agent::atomic_write(
            &self.inner.ledger,
            &serde_json::to_vec(&entries).map_err(|_| crate::error::invalid("EAS ledger"))?,
        )
        .await
    }
    async fn forget_entry(&self, name: &str) {
        let _ = self
            .ledger(|entries| entries.retain(|e| e.name != name))
            .await;
    }
    async fn stop_session(&self, api: &dyn ExpoApi, id: &str, name: &str) -> Result<()> {
        bounded(api.stop(id)).await?;
        self.forget_entry(name).await;
        Ok(())
    }
    async fn reap(&self, api: &dyn ExpoApi) -> Result<()> {
        let _guard = self.inner.reap_lock.lock().await;
        if self.inner.state.lock().await.closed {
            return Ok(());
        }
        let now = self.now();
        self.inner.state.lock().await.reaped = now;
        let entries = self.read_ledger().await?;
        let owned = {
            let s = self.inner.state.lock().await;
            s.sessions
                .values()
                .map(|s| s.name.clone())
                .chain(s.starting.keys().cloned())
                .chain(s.unresolved.keys().cloned())
                .collect::<BTreeSet<_>>()
        };
        let leftovers = entries
            .into_iter()
            .filter(|e| !owned.contains(&e.name))
            .collect::<Vec<_>>();
        for app in leftovers
            .iter()
            .map(|e| e.app_id.clone())
            .collect::<BTreeSet<_>>()
        {
            let active = bounded(api.active(&app)).await;
            for entry in leftovers.iter().filter(|e| e.app_id == app) {
                let mut found = false;
                if let Ok(active) = &active {
                    for session in active.iter().filter(|s| s.name.as_deref() == Some(entry.name.as_str())) {
                        found = true;
                        if self
                            .stop_session(api, &session.id, &entry.name)
                            .await
                            .is_err()
                        {
                            self.inner
                                .state
                                .lock()
                                .await
                                .unresolved
                                .insert(entry.name.clone(), session.id.clone());
                        }
                    }
                }
                let mut state = self.inner.state.lock().await;
                if found
                    || active.is_ok()
                        && state
                            .uncertain
                            .get(&entry.name)
                            .is_some_and(|(_, until)| now >= *until)
                    || now.saturating_sub(entry.recorded_at) > 180 * 60_000
                {
                    state.uncertain.remove(&entry.name);
                }
                drop(state);
                if !found
                    && (now.saturating_sub(entry.recorded_at) > 24 * 60 * 60_000
                        || active.is_ok() && now.saturating_sub(entry.recorded_at) > 180 * 60_000)
                {
                    self.forget_entry(&entry.name).await;
                }
            }
        }
        Ok(())
    }
    async fn drop_ended(&self) {
        let now = self.now();
        let (sessions, unresolved, reap) = {
            let s = self.inner.state.lock().await;
            (
                s.sessions
                    .values()
                    .filter(|r| r.stopping || now.saturating_sub(r.checked) > 15_000)
                    .cloned()
                    .collect::<Vec<_>>(),
                s.unresolved.clone(),
                now.saturating_sub(s.reaped) > 60_000 || !s.uncertain.is_empty(),
            )
        };
        if sessions.is_empty() && unresolved.is_empty() && !reap {
            return;
        }
        let api = match self.api(true).await {
            Ok(api) => api,
            Err(_) => {
                self.inner
                    .state
                    .lock()
                    .await
                    .uncertain
                    .retain(|_, (_, until)| now < *until);
                return;
            }
        };
        for (name, id) in unresolved {
            if self.stop_session(api.as_ref(), &id, &name).await.is_ok()
                || bounded(api.session(&id)).await.is_ok_and(|s| s.ended())
            {
                self.forget_entry(&name).await;
                self.inner.state.lock().await.unresolved.remove(&name);
            }
        }
        for running in sessions {
            let ended = if running.stopping
                && self
                    .stop_session(api.as_ref(), running.device.id.as_ref(), &running.name)
                    .await
                    .is_ok()
            {
                true
            } else {
                bounded(api.session(running.device.id.as_ref()))
                    .await
                    .is_ok_and(|s| s.ended())
            };
            if ended {
                self.inner
                    .state
                    .lock()
                    .await
                    .sessions
                    .remove(running.device.id.as_ref());
                self.forget_entry(&running.name).await;
            } else if let Some(r) = self
                .inner
                .state
                .lock()
                .await
                .sessions
                .get_mut(running.device.id.as_ref())
            {
                r.checked = now;
            }
        }
        if reap {
            let _ = self.reap(api.as_ref()).await;
        }
    }
    async fn start(
        &self,
        device: DeviceSummary,
        context: OpenContext,
        name: String,
        epoch: u64,
        cancel: CancellationToken,
    ) -> Result<String> {
        let api = self.api(false).await?;
        let app = match self.inner.config.app_id.as_ref().and_then(|v| v.as_ref()) {
            Some(id) => id.as_ref().to_owned(),
            None => match context.workspace_root {
                Some(root) => crate::project::find_eas_project_id(&root)
                    .await
                    .unwrap_or_default(),
                None => String::new(),
            },
        };
        if app.is_empty() {
            return Err(self.unavailable("No EAS project to run the session under. Choose one for this host in Settings, or open the device from a thread whose project names extra.eas.projectId in app.json or its app config."));
        }
        let (account, available) = bounded(api.availability(&app)).await?;
        if !available {
            return Err(self.unavailable(format!(
                "EAS Simulator is not enabled for the {account} Expo account."
            )));
        }
        let entry = LedgerEntry {
            name: name.clone(),
            app_id: app.clone(),
            recorded_at: self.now(),
            refused: false,
        };
        self.ledger(|entries| entries.push(entry.clone())).await?;
        if cancel.is_cancelled() || self.inner.state.lock().await.epoch != epoch {
            self.forget_entry(&name).await;
            return Err(self.unavailable("Device host was stopped while the session started."));
        }
        let id = match bounded(api.create(Create {
            app: app.clone(),
            platform: device.platform.clone(),
            name: name.clone(),
            max_minutes: self.inner.config.max_session_minutes,
            idle_minutes: self.inner.config.idle_minutes,
        }))
        .await
        {
            Ok(id) => id,
            Err(error) => {
                let mut found = false;
                if let Ok(active) = bounded(api.active(&app)).await {
                    for session in active.iter().filter(|s| s.name.as_deref() == Some(name.as_str())) {
                        found = true;
                        if self
                            .stop_session(api.as_ref(), &session.id, &name)
                            .await
                            .is_err()
                        {
                            self.inner
                                .state
                                .lock()
                                .await
                                .unresolved
                                .insert(name.clone(), session.id.clone());
                        }
                    }
                }
                if !found {
                    if error.rejected {
                        let _ = self
                            .ledger(|entries| {
                                if let Some(e) = entries.iter_mut().find(|e| e.name == name) {
                                    e.refused = true;
                                }
                            })
                            .await;
                    } else {
                        self.inner
                            .state
                            .lock()
                            .await
                            .uncertain
                            .insert(name.clone(), (entry, self.now() + 5 * 60_000));
                    }
                }
                return Err(error.into());
            }
        };
        let result = tokio::select! {biased;_=cancel.cancelled()=>Err(self.unavailable("Device host was stopped while the session started.")),result=self.track(api.as_ref(),&id,&name,device.platform,epoch)=>result};
        if result.is_err() && self.stop_session(api.as_ref(), &id, &name).await.is_err() {
            self.inner
                .state
                .lock()
                .await
                .unresolved
                .insert(name, id.clone());
        }
        result
    }
    async fn track(
        &self,
        api: &dyn ExpoApi,
        id: &str,
        name: &str,
        platform: DevicePlatform,
        epoch: u64,
    ) -> Result<String> {
        let polling = async {
            loop {
                let session = retry(|| api.session(id)).await?;
                if session.ended() {
                    return Err(Error::boot(
                        self.id(),
                        id,
                        DeviceBootErrorReason::LaunchFailed,
                        Some(format!(
                            "EAS Simulator session ended ({}) before it was ready",
                            session.status
                        )),
                    ));
                }
                if session.status == "IN_PROGRESS" && session.agent.is_some() {
                    return Ok(session);
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        };
        let session: Session = tokio::time::timeout(Duration::from_secs(900), polling)
            .await
            .map_err(|_| Error::boot(self.id(), id, DeviceBootErrorReason::Timeout, None))??;
        let (url, token) = session.preview.ok_or_else(|| {
            Error::boot(
                self.id(),
                id,
                DeviceBootErrorReason::LaunchFailed,
                Some("EAS Simulator returned no preview endpoint".into()),
            )
        })?;
        let origin = crate::io::validated_url(&url)?
            .origin()
            .ascii_serialization();
        let prefix = if platform == DevicePlatform::Ios {
            ""
        } else {
            "/vendor/serve-emu"
        };
        let mut response = None;
        for attempt in 0..4 {
            let request = HttpRequest::new(
                Method::GET,
                &format!("{origin}{prefix}/api"),
                Duration::from_secs(20),
            )?
            .bearer(&token);
            match json_request(self.inner.http.as_ref(), request, "EAS preview").await {
                Ok(value) => {
                    response = Some(value);
                    break;
                }
                Err(e) => {
                    if attempt == 3 {
                        return Err(e);
                    } else {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                }
            }
        }
        let response = response.ok_or_else(|| crate::error::invalid("EAS preview"))?;
        let remote_id = response[if platform == DevicePlatform::Ios {
            "device"
        } else {
            "serial"
        }]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| crate::error::invalid("EAS device id"))?
        .to_owned();
        crate::error::validate_selector(&remote_id,"EAS device id")?;
        let mut agent = session
            .agent
            .ok_or_else(|| crate::error::invalid("EAS agent"))?;
        agent.agent_device_id = Some(remote_id.clone());
        let mut state = self.inner.state.lock().await;
        if state.epoch != epoch {
            return Err(self.unavailable("Device host was stopped while the session started."));
        }
        state.sessions.insert(
            id.into(),
            Running {
                device: DeviceSummary {
                    host_id: self.id().into(),
                    id: id.into(),
                    name: if platform == DevicePlatform::Ios {
                        "iPhone (EAS Simulator)"
                    } else {
                        "Android (EAS Simulator)"
                    }
                    .into(),
                    platform,
                    version: "Cloud".into(),
                    booted: true,
                    physical: false,
                },
                name: name.into(),
                remote_id,
                origin,
                token,
                agent,
                stopping: false,
                checked: 0,
            },
        );
        Ok(id.into())
    }
    fn ready(&self) -> Ready {
        Ready {
            origin: "https://api.expo.dev".into(),
            node_path: "node".into(),
            devices: Arc::new(self.clone()),
            commands: Arc::new(NoCommands("EAS Simulator")),
            ax_helper: None,
            sim_cli: None,
            agent: None,
            routing: Some(Arc::new(self.clone())),
            release: Some(Arc::new(self.clone())),
        }
    }
    pub async fn close_host(&self) -> Result<()> {
        self.stop().await?;
        let _guard = self.inner.ledger_lock.lock().await;
        self.inner.state.lock().await.closed = true;
        Ok(())
    }
}
#[async_trait]
impl DeviceHost for EasHost {
    fn id(&self) -> &str {
        self.inner.config.id.as_ref()
    }
    fn label(&self) -> &str {
        self.inner.config.label.as_ref()
    }
    async fn summary(&self) -> DeviceHostSummary {
        let mut summary = host::summary(
            self.id(),
            self.label(),
            DeviceHostKind::Eas,
            vec![
                self.availability(DevicePlatform::Ios).await,
                self.availability(DevicePlatform::Android).await,
            ],
        );
        summary.hub_installed = true;
        summary.agent_device_installed = true;
        summary
    }
    async fn availability(&self, platform: DevicePlatform) -> DevicePlatformAvailability {
        host::available(
            platform,
            self.inner.provider.api().await.err().map(|e| e.to_string()),
        )
    }
    async fn ensure_ready(&self, _: Phase) -> Result<Ready> {
        let first = {
            let mut s = self.inner.state.lock().await;
            let first = !s.ready;
            s.ready = true;
            first
        };
        if first {
            if let Ok(entries) = self.read_ledger().await {
                let now = self.now();
                let mut s = self.inner.state.lock().await;
                for entry in entries {
                    if !entry.refused {
                        s.uncertain.insert(
                            entry.name.clone(),
                            (entry.clone(), (entry.recorded_at + 5 * 60_000).max(now)),
                        );
                    }
                }
            }
            if let Ok(api) = self.api(false).await {
                let _ = self.reap(api.as_ref()).await;
            }
        }
        Ok(self.ready())
    }
    async fn ensure_agent_ready(&self, phase: Phase) -> Result<Ready> {
        self.ensure_ready(phase).await
    }
    async fn current(&self) -> Option<Ready> {
        self.inner.state.lock().await.ready.then(|| self.ready())
    }
    async fn stop_agent(&self) -> Result<()> {
        Ok(())
    }
    async fn retire(&self) -> Result<()> {
        self.close_host().await
    }
    async fn stop(&self) -> Result<()> {
        let starts = {
            let mut state = self.inner.state.lock().await;
            state.epoch += 1;
            state.starting.values().cloned().collect::<Vec<_>>()
        };
        for start in &starts {
            start.cancel.cancel();
        }
        for mut start in starts {
            while !*start.done.borrow() {
                if start.done.changed().await.is_err() {
                    break;
                }
            }
        }
        let api = match self.api(true).await {
            Ok(api) => api,
            Err(_) => return Ok(()),
        };
        let sessions = self
            .inner
            .state
            .lock()
            .await
            .sessions
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for running in sessions {
            let _ = self
                .shutdown(running.device.id.as_ref(), running.device.platform)
                .await;
        }
        self.drop_ended().await;
        let _ = self.reap(api.as_ref()).await;
        Ok(())
    }
}
#[async_trait]
impl DeviceOperations for EasHost {
    async fn list(&self) -> Result<Listing> {
        self.drop_ended().await;
        let mut devices = catalog(self.id());
        devices.extend(
            self.inner
                .state
                .lock()
                .await
                .sessions
                .values()
                .map(|s| s.device.clone()),
        );
        Ok(Listing {
            devices,
            ..Default::default()
        })
    }
    async fn boot(&self, device: &DeviceSummary, context: &OpenContext) -> Result<String> {
        if self
            .inner
            .state
            .lock()
            .await
            .sessions
            .contains_key(device.id.as_ref())
        {
            return Ok(device.id.as_ref().into());
        }
        let _ = self.api(false).await?;
        let name = format!(
            "T3 Code – {} – {}",
            self.label(),
            &uuid::Uuid::new_v4().simple().to_string()[..6]
        );
        let (cancel, done, epoch) = {
            let mut state = self.inner.state.lock().await;
            let count = state.sessions.len()
                + state.starting.len()
                + state.unresolved.len()
                + state.uncertain.len();
            if count >= self.inner.config.max_concurrent_sessions as usize {
                return Err(self.unavailable(format!(
                    "{} is limited to {} running {}. Close one in the Device panel first.",
                    self.label(),
                    self.inner.config.max_concurrent_sessions,
                    if self.inner.config.max_concurrent_sessions == 1 {
                        "session"
                    } else {
                        "sessions"
                    }
                )));
            }
            let cancel = CancellationToken::new();
            let (tx, rx) = watch::channel(false);
            let epoch = state.epoch;
            state.starting.insert(
                name.clone(),
                Start {
                    cancel: cancel.clone(),
                    done: rx,
                },
            );
            (cancel, tx, epoch)
        };
        let this = self.clone();
        let device = device.clone();
        let context = context.clone();
        tokio::spawn(async move {
            let result = this
                .start(device, context, name.clone(), epoch, cancel)
                .await;
            this.inner.state.lock().await.starting.remove(&name);
            let _ = done.send(true);
            result
        })
        .await
        .map_err(|_| Error::Cancelled("EAS start"))?
    }
    async fn attach(&self, _: &DeviceSummary) -> Result<()> {
        Ok(())
    }
    async fn shutdown(&self, id: &str, _: DevicePlatform) -> Result<()> {
        let running = {
            let mut state = self.inner.state.lock().await;
            state.sessions.get_mut(id).map(|r| {
                r.stopping = true;
                r.clone()
            })
        };
        if let Some(running) = running {
            let api = self.api(true).await?;
            self.stop_session(api.as_ref(), id, &running.name).await?;
            self.inner.state.lock().await.sessions.remove(id);
        }
        Ok(())
    }
    async fn screenshot(&self, device: &DeviceSummary) -> Result<Vec<u8>> {
        let running = self
            .inner
            .state
            .lock()
            .await
            .sessions
            .get(device.id.as_ref())
            .cloned()
            .ok_or_else(|| crate::error::invalid("EAS screenshot"))?;
        let prefix = if running.device.platform == DevicePlatform::Ios {
            ""
        } else {
            "/vendor/serve-emu"
        };
        let mut request = HttpRequest::new(
            Method::POST,
            &format!("{}{prefix}/api/screenshot", running.origin),
            Duration::from_secs(20),
        )?
        .bearer(&running.token);
        request
            .url
            .query_pairs_mut()
            .append_pair("device", &running.remote_id);
        let (status, bytes) = request_bytes(self.inner.http.as_ref(), request).await?;
        if !(200..300).contains(&status) {
            return Err(crate::error::invalid("EAS screenshot"));
        }
        Ok(bytes)
    }
    async fn agent_endpoint(&self, id: &str) -> Result<Option<AgentEndpoint>> {
        self.inner
            .state
            .lock()
            .await
            .sessions
            .get(id)
            .map(|s| Some(s.agent.clone()))
            .ok_or_else(|| self.unavailable("This EAS Simulator device is no longer running."))
    }
    fn ends_devices(&self) -> bool {
        true
    }
}
#[async_trait]
impl Routing for EasHost {
    async fn upstream(&self, path: &str, query: &[(String, String)]) -> Result<Option<Upstream>> {
        let platform = if path.starts_with("/vendor/serve-sim/") {
            DevicePlatform::Ios
        } else if path.starts_with("/vendor/serve-emu/") {
            DevicePlatform::Android
        } else {
            return Ok(None);
        };
        let helper = path
            .strip_prefix("/vendor/serve-sim/helper/")
            .and_then(|s| s.split_once('/'));
        let id = if let Some((id, _)) = helper {
            match decode_segment(id) {
                Some(id) => Some(id),
                None => return Ok(None),
            }
        } else {
            query
                .iter()
                .find(|(k, _)| k == "device")
                .map(|(_, v)| v.clone())
        };
        let state = self.inner.state.lock().await;
        let running = if let Some(id) = id {
            state.sessions.get(&id)
        } else if state.sessions.len() == 1 {
            state.sessions.values().next()
        } else {
            None
        };
        let Some(running) = running.filter(|r| r.device.platform == platform) else {
            return Ok(None);
        };
        let mut url = crate::io::validated_url(&running.origin)?;
        let prefix = if platform == DevicePlatform::Ios {
            "/vendor/serve-sim"
        } else {
            "/vendor/serve-emu"
        };
        let path = if let Some((_, tail)) = helper {
            format!("/helper/{}/{tail}", encode_segment(&running.remote_id))
        } else if platform == DevicePlatform::Ios {
            path.strip_prefix(prefix).unwrap_or(path).into()
        } else {
            path.into()
        };
        url.set_path(&path);
        if !query.is_empty() {
            let mut params = url.query_pairs_mut();
            for (k, v) in query {
                params.append_pair(k, if k == "device" { &running.remote_id } else { v });
            }
        }
        let protocol = if platform == DevicePlatform::Ios {
            "serve-sim"
        } else {
            "serve-emu"
        };
        Ok(Some(Upstream {
            url: url.into(),
            headers: BTreeMap::from([(
                "authorization".into(),
                Secret::new(format!("Bearer {}", running.token.expose())),
            )]),
            protocols: vec![Secret::new(format!(
                "{protocol}.token.{}",
                running.token.expose()
            ))],
        }))
    }
}
#[async_trait]
impl SessionRelease for EasHost {
    async fn release(&self, session: &str, device: &str) -> Result<()> {
        let endpoint = self
            .inner
            .state
            .lock()
            .await
            .sessions
            .get(device)
            .map(|s| s.agent.clone());
        if let Some(endpoint) = endpoint {
            agent::release_session(self.inner.http.as_ref(), &endpoint, session).await?;
        }
        Ok(())
    }
}
pub struct NoCommands(pub &'static str);
#[async_trait]
impl Commands for NoCommands {
    async fn run(&self, _: Command) -> Result<CommandOutput> {
        Ok(CommandOutput {
            code: 127,
            stdout: String::new(),
            stderr: format!("{} devices cannot run host commands.", self.0),
        })
    }
}
pub fn encode_segment(value: &str) -> String {
    const COMPONENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'!')
        .remove(b'~')
        .remove(b'*')
        .remove(b'\'')
        .remove(b'(')
        .remove(b')');
    percent_encoding::utf8_percent_encode(value, COMPONENT).to_string()
}
pub fn decode_segment(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'%'
            && (i + 2 >= bytes.len()
                || !bytes[i + 1].is_ascii_hexdigit()
                || !bytes[i + 2].is_ascii_hexdigit())
        {
            return None;
        }
    }
    percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .ok()
        .map(|s| s.into_owned())
}
fn catalog(host: &str) -> Vec<DeviceSummary> {
    [
        ("eas-ios", "iPhone (EAS Simulator)", DevicePlatform::Ios),
        (
            "eas-android",
            "Android (EAS Simulator)",
            DevicePlatform::Android,
        ),
    ]
    .into_iter()
    .map(|(id, name, platform)| DeviceSummary {
        host_id: host.into(),
        id: id.into(),
        name: name.into(),
        platform,
        version: "Cloud".into(),
        booted: false,
        physical: false,
    })
    .collect()
}
async fn bounded<T>(future: impl Future<Output = ExpoResult<T>>) -> ExpoResult<T> {
    tokio::time::timeout(Duration::from_secs(30), future)
        .await
        .map_err(|_| super::expo::ExpoError {
            operation: "complete request".into(),
            detail: "Expo's API did not answer in time".into(),
            rejected: false,
            status: None,
        })?
}
async fn retry<T, F, Fut>(mut run: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ExpoResult<T>>,
{
    for attempt in 0..4 {
        match bounded(run()).await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if attempt == 3 {
                    return Err(e.into());
                } else {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    }
    Err(crate::error::invalid("Expo retry"))
}
