use crate::{
    Error, Result,
    agent::{self, AgentTarget, Launchers},
    host::{DeviceHost, HostFactory, OpenContext, Ready},
    io::Command,
};
use async_trait::async_trait;
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use t3_contracts::{base_schemas::ThreadId, device::*};
use tokio::sync::{Mutex, mpsc};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct DeviceSettings {
    pub enabled: bool,
    pub agent_access_enabled: bool,
    pub onboarding_completed: bool,
    pub hosts: Vec<DeviceHostConfig>,
}
impl std::fmt::Debug for DeviceSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceSettings")
            .field("enabled", &self.enabled)
            .field("agent_access_enabled", &self.agent_access_enabled)
            .field("onboarding_completed", &self.onboarding_completed)
            .field("hosts", &self.hosts.len())
            .finish()
    }
}
#[async_trait]
pub trait Settings: Send + Sync {
    async fn read(&self) -> Result<DeviceSettings>;
    async fn update(&self, input: &DeviceConfigureInput) -> Result<DeviceSettings>;
}
pub struct MemorySettings(pub Mutex<DeviceSettings>);
#[async_trait]
impl Settings for MemorySettings {
    async fn read(&self) -> Result<DeviceSettings> {
        Ok(self.0.lock().await.clone())
    }
    async fn update(&self, input: &DeviceConfigureInput) -> Result<DeviceSettings> {
        let mut s = self.0.lock().await;
        if let Some(Some(v)) = input.enabled {
            s.enabled = v;
        }
        if let Some(Some(v)) = input.agent_access_enabled {
            s.agent_access_enabled = v;
        }
        if let Some(Some(v)) = input.onboarding_completed {
            s.onboarding_completed = v;
        }
        Ok(s.clone())
    }
}

#[async_trait]
pub trait Accounts: Send + Sync {
    async fn eas_account(&self, input: DeviceEasAccountInput) -> Result<DeviceEasAccount>;
    async fn simlock_identity(
        &self,
        input: DeviceSimlockIdentityInput,
    ) -> Result<DeviceSimlockIdentity>;
}
#[async_trait]
pub trait ToolInstaller: Send + Sync {
    async fn install(&self, tool: DeviceUpdateTool) -> Result<()>;
}

#[async_trait]
pub trait Admission: Send + Sync {
    async fn check(&self) -> Result<()>;
}
struct Unrestricted;
#[async_trait]
impl Admission for Unrestricted {
    async fn check(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
struct HostEntry {
    host: Arc<dyn DeviceHost>,
    generation: u64,
    config: Option<DeviceHostConfig>,
}
struct State {
    wire: DeviceServiceState,
    hosts: Vec<HostEntry>,
    generation: u64,
    subscribers: Vec<mpsc::UnboundedSender<DeviceServiceState>>,
    launchers: BTreeMap<String, (String, bool)>,
}
#[derive(Clone)]
pub struct DeviceService {
    inner: Arc<Inner>,
}
struct Inner {
    state: Mutex<State>,
    lifecycle: Mutex<()>,
    launcher_lock: Mutex<()>,
    settings: Arc<dyn Settings>,
    factory: Option<Arc<dyn HostFactory>>,
    launchers: Option<Arc<dyn Launchers>>,
    installer: Option<Arc<dyn ToolInstaller>>,
    accounts: Option<Arc<dyn Accounts>>,
}

impl DeviceService {
    pub async fn new(
        hosts: Vec<Arc<dyn DeviceHost>>,
        settings: Arc<dyn Settings>,
        factory: Option<Arc<dyn HostFactory>>,
        launchers: Option<Arc<dyn Launchers>>,
        installer: Option<Arc<dyn ToolInstaller>>,
        accounts: Option<Arc<dyn Accounts>>,
    ) -> Result<Self> {
        let config = settings.read().await?;
        let mut summaries = Vec::new();
        let mut entries = Vec::new();
        let mut ids = BTreeSet::new();
        for host in hosts {
            if !ids.insert(host.id().to_owned()) {
                return Err(crate::error::invalid("duplicate host id"));
            }
            summaries.push(host.summary().await);
            entries.push(HostEntry {
                host,
                generation: 0,
                config: None,
            });
        }
        let wire = DeviceServiceState {
            supports_host_retry: Some(Some(true)),
            supports_tool_update: Some(Some(installer.is_some())),
            supports_tool_inspection: Some(Some(true)),
            supports_eas_hosts: Some(Some(factory.is_some())),
            supports_simlock_hosts: Some(Some(factory.is_some())),
            hosts: summaries,
            host_status: if config.enabled {
                DeviceHostStatus::Idle
            } else {
                DeviceHostStatus::Disabled
            },
            host_status_detail: None,
            host_statuses: BTreeMap::new(),
            devices: vec![],
            sessions: vec![],
            booting_devices: None,
            onboarding_completed: config.onboarding_completed,
            agent_access_enabled: config.agent_access_enabled,
            hub_base_path: crate::DEVICE_HUB_ROUTE_PREFIX.into(),
            revision: 0,
            eas_hosts: None,
            simlock_hosts: None,
        };
        if let Some(launchers) = &launchers {
            launchers.prune(&BTreeSet::new()).await?;
        }
        let service = Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    wire,
                    hosts: entries,
                    generation: 0,
                    subscribers: vec![],
                    launchers: BTreeMap::new(),
                }),
                lifecycle: Mutex::new(()),
                launcher_lock: Mutex::new(()),
                settings,
                factory,
                launchers,
                installer,
                accounts,
            }),
        };
        service.reconcile(config.hosts).await?;
        Ok(service)
    }
    pub async fn state(&self) -> DeviceServiceState {
        self.inner.state.lock().await.wire.clone()
    }
    pub async fn state_stream(&self) -> mpsc::UnboundedReceiver<DeviceServiceState> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut state = self.inner.state.lock().await;
        let _ = tx.send(state.wire.clone());
        state.subscribers.push(tx);
        rx
    }
    async fn publish(&self, update: impl FnOnce(&mut DeviceServiceState)) -> DeviceServiceState {
        let mut state = self.inner.state.lock().await;
        update(&mut state.wire);
        state.wire.revision += 1;
        let next = state.wire.clone();
        state.subscribers.retain(|s| s.send(next.clone()).is_ok());
        next
    }
    async fn host(&self, id: Option<&str>) -> Result<HostEntry> {
        let id = id.unwrap_or(LOCAL_DEVICE_HOST_ID);
        self.inner
            .state
            .lock()
            .await
            .hosts
            .iter()
            .find(|e| e.host.id() == id)
            .cloned()
            .ok_or_else(|| Error::host(id, None, "Unknown host."))
    }
    async fn unchanged(&self, entry: &HostEntry) -> bool {
        self.inner.state.lock().await.hosts.iter().any(|e| {
            e.host.id() == entry.host.id()
                && e.generation == entry.generation
                && Arc::ptr_eq(&e.host, &entry.host)
        })
    }
    async fn status(&self, entry: &HostEntry, status: DeviceHostStatus, detail: Option<String>) {
        let mut state = self.inner.state.lock().await;
        if state.wire.host_status == DeviceHostStatus::Disabled
            && status != DeviceHostStatus::Disabled
        {
            return;
        }
        if !state
            .hosts
            .iter()
            .any(|e| e.host.id() == entry.host.id() && e.generation == entry.generation)
        {
            return;
        }
        let previous = state
            .wire
            .host_statuses
            .get(&DeviceHostId::from(entry.host.id()));
        let detail = detail.or_else(|| {
            previous
                .filter(|p| {
                    status == DeviceHostStatus::Ready
                        && (p.incomplete_platforms.is_some()
                            || p.android_discovery_incomplete.is_some())
                })
                .and_then(|p| p.detail.clone().flatten())
        });
        let value = DeviceHostStatusDetail {
            status: status.clone(),
            detail: detail.clone().map(Some),
            android_discovery_incomplete: previous.and_then(|p| p.android_discovery_incomplete),
            incomplete_platforms: previous.and_then(|p| p.incomplete_platforms.clone()),
        };
        state
            .wire
            .host_statuses
            .insert(entry.host.id().into(), value);
        if entry.host.id() == LOCAL_DEVICE_HOST_ID {
            state.wire.host_status = status;
            state.wire.host_status_detail = detail.map(Some);
        }
        state.wire.revision += 1;
        let next = state.wire.clone();
        state.subscribers.retain(|s| s.send(next.clone()).is_ok());
    }
    async fn ready_locked(&self, entry: &HostEntry, agent: bool) -> Result<Ready> {
        let settings = self.inner.settings.read().await?;
        if !settings.enabled || agent && !settings.agent_access_enabled {
            return Err(Error::host(
                entry.host.id(),
                Some(entry.host.label()),
                "Device support is off. Enable it in the Device panel before installing or starting device tools.",
            ));
        }
        let (phase_tx, mut phase_rx) = mpsc::unbounded_channel();
        let callback = Arc::new(move |phase, detail| {
            let _ = phase_tx.send((phase, detail));
        });
        let future = async {
            if agent {
                entry.host.ensure_agent_ready(callback).await
            } else {
                entry.host.ensure_ready(callback).await
            }
        };
        tokio::pin!(future);
        let result = loop {
            tokio::select! {result=&mut future=>break result,Some((phase,detail))=phase_rx.recv()=>self.status(entry,phase,detail).await}
        };
        while let Ok((phase, detail)) = phase_rx.try_recv() {
            self.status(entry, phase, detail).await;
        }
        let weak = Arc::downgrade(&self.inner);
        let id = entry.host.id().to_owned();
        let generation = entry.generation;
        tokio::spawn(async move {
            while let Some((phase, detail)) = phase_rx.recv().await {
                let Some(inner) = weak.upgrade() else { break };
                let service = Self { inner };
                let entry = service
                    .inner
                    .state
                    .lock()
                    .await
                    .hosts
                    .iter()
                    .find(|e| e.host.id() == id && e.generation == generation)
                    .cloned();
                if let Some(entry) = entry {
                    service.status(&entry, phase, detail).await;
                } else {
                    break;
                }
            }
        });
        match result {
            Ok(ready) => {
                if !self.unchanged(entry).await {
                    return Err(Error::host(
                        entry.host.id(),
                        Some(entry.host.label()),
                        "Host configuration changed. Retry the operation.",
                    ));
                }
                self.status(entry, DeviceHostStatus::Ready, None).await;
                Ok(ready)
            }
            Err(error) => {
                self.status(entry, DeviceHostStatus::Failed, Some(error.to_string()))
                    .await;
                Err(error)
            }
        }
    }
    pub async fn readiness(&self, id: Option<&str>) -> Result<Ready> {
        let _guard = self.inner.lifecycle.lock().await;
        let entry = self.host(id).await?;
        self.ready_locked(&entry, false).await
    }
    pub async fn readiness_if_supported(&self, id: Option<&str>) -> Result<Option<Ready>> {
        self.supported(id, false).await
    }
    pub async fn agent_readiness_if_supported(&self, id: Option<&str>) -> Result<Option<Ready>> {
        self.supported(id, true).await
    }
    async fn supported(&self, id: Option<&str>, agent: bool) -> Result<Option<Ready>> {
        let _guard = self.inner.lifecycle.lock().await;
        let settings = self.inner.settings.read().await?;
        if !settings.enabled || agent && !settings.agent_access_enabled {
            return Ok(None);
        }
        let entry = self.host(id).await?;
        let summary = entry.host.summary().await;
        if summary.kind == DeviceHostKind::Local && !summary.platforms.iter().any(|p| p.available) {
            return Ok(None);
        }
        self.ready_locked(&entry, agent).await.map(Some)
    }
    pub async fn current_readiness(&self, id: Option<&str>) -> Option<Ready> {
        match self.host(id).await {
            Ok(e) => e.host.current().await,
            Err(_) => None,
        }
    }
    async fn refresh(&self, entry: &HostEntry, ready: &Ready) -> Result<DeviceServiceState> {
        let before = self.state().await.sessions;
        let listing = ready.devices.list().await?;
        let _guard = self.inner.lifecycle.lock().await;
        if !self.inner.settings.read().await?.enabled || !self.unchanged(entry).await {
            return Ok(self.state().await);
        }
        let summary = entry.host.summary().await;
        let id = entry.host.id();
        let next = self
            .publish(|state| {
                if let Some(previous) = state.hosts.iter_mut().find(|h| h.id.as_ref() == id) {
                    *previous = summary;
                }
                state.devices.retain(|d| {
                    d.host_id.as_ref() != id || listing.incomplete_platforms.contains(&d.platform)
                });
                state.devices.extend(
                    listing
                        .devices
                        .iter()
                        .filter(|d| !listing.incomplete_platforms.contains(&d.platform))
                        .cloned(),
                );
                if ready.devices.ends_devices() {
                    state.sessions.retain(|s| {
                        s.host_id.as_ref() != id
                            || !before.contains(s)
                            || listing.devices.iter().any(|d| d.id == s.device_id)
                    });
                }
                if id == LOCAL_DEVICE_HOST_ID {
                    state.host_status_detail = listing.detail.clone().map(Some);
                }
                state.host_statuses.insert(
                    id.into(),
                    DeviceHostStatusDetail {
                        status: DeviceHostStatus::Ready,
                        detail: listing.detail.map(Some),
                        android_discovery_incomplete: listing
                            .android_discovery_incomplete
                            .then_some(Some(true)),
                        incomplete_platforms: (!listing.incomplete_platforms.is_empty())
                            .then_some(Some(listing.incomplete_platforms)),
                    },
                );
            })
            .await;
        self.prune_launchers().await?;
        Ok(next)
    }
    pub async fn list(&self) -> Result<DeviceServiceState> {
        if !self.inner.settings.read().await?.enabled {
            return Ok(self.state().await);
        }
        let entries = self.inner.state.lock().await.hosts.clone();
        stream::iter(entries)
            .for_each_concurrent(4, |entry| async move {
                let result = async {
                    if let Some(ready) = self.readiness_if_supported(Some(entry.host.id())).await? {
                        self.refresh(&entry, &ready).await?;
                    }
                    Ok::<_, Error>(())
                }
                .await;
                if let Err(error) = result {
                    self.status(&entry, DeviceHostStatus::Failed, Some(error.to_string()))
                        .await;
                }
            })
            .await;
        Ok(self.state().await)
    }
    pub async fn list_request(&self, input: DeviceListInput) -> Result<DeviceServiceState> {
        if input.inspect_only.flatten() == Some(true)
            && input
                .update_tool
                .as_ref()
                .and_then(|v| v.as_ref())
                .is_none()
        {
            return self.inspect().await;
        }
        if let Some(tool) = input.update_tool.flatten() {
            return self.update_tool(tool).await;
        }
        if let Some(id) = input.retry_host_id.flatten() {
            return self.retry_host(id.as_ref()).await;
        }
        self.list().await
    }
    pub async fn inspect(&self) -> Result<DeviceServiceState> {
        let entries = self.inner.state.lock().await.hosts.clone();
        for entry in entries {
            let summary = entry.host.inspect().await;
            if !self.unchanged(&entry).await {
                continue;
            }
            self.publish(|state|{if let Some(host)=state.hosts.iter_mut().find(|h|h.id.as_ref()==entry.host.id()){match summary{Ok(next)=>*host=next,Err(_)=>host.tool_inspection_error=Some(Some("Cannot check versions. Reconnect the host and check again. Installed tools have not been changed.".into()))}}}).await;
        }
        Ok(self.state().await)
    }
    pub async fn retry_host(&self, id: &str) -> Result<DeviceServiceState> {
        let entry = self.host(Some(id)).await?;
        if !self.inner.settings.read().await?.enabled {
            return Ok(self.state().await);
        }
        let result = async {
            let ready = match self.agent_readiness_if_supported(Some(id)).await? {
                Some(r) => Some(r),
                None => self.readiness_if_supported(Some(id)).await?,
            };
            if let Some(ready) = ready {
                self.refresh(&entry, &ready).await?;
            }
            Ok::<_, Error>(())
        }
        .await;
        if let Err(e) = result {
            self.status(&entry, DeviceHostStatus::Failed, Some(e.to_string()))
                .await;
        }
        Ok(self.state().await)
    }
    pub async fn update_tool(&self, tool: DeviceUpdateTool) -> Result<DeviceServiceState> {
        let _guard = self.inner.lifecycle.lock().await;
        self.inner
            .installer
            .as_ref()
            .ok_or_else(|| {
                Error::Unavailable(
                    "Tool installation is unavailable in this device service.".into(),
                )
            })?
            .install(tool)
            .await?;
        self.inspect().await
    }
    pub async fn configure(&self, input: DeviceConfigureInput) -> Result<DeviceServiceState> {
        let service = self.clone();
        tokio::spawn(async move { service.configure_inner(input).await })
            .await
            .map_err(|_| Error::Cancelled("configure"))?
    }
    async fn configure_inner(&self, input: DeviceConfigureInput) -> Result<DeviceServiceState> {
        {
            let _guard = self.inner.lifecycle.lock().await;
            let settings = self.inner.settings.update(&input).await?;
            let entries = self.inner.state.lock().await.hosts.clone();
            for entry in entries {
                if !settings.enabled {
                    entry.host.stop().await?;
                } else if input.agent_access_enabled.flatten() == Some(false) {
                    entry.host.stop_agent().await?;
                }
            }
            self.publish(|state| {
                state.host_status = if settings.enabled {
                    DeviceHostStatus::Idle
                } else {
                    DeviceHostStatus::Disabled
                };
                state.host_status_detail = None;
                state.host_statuses.clear();
                state.agent_access_enabled = settings.agent_access_enabled;
                state.onboarding_completed = settings.onboarding_completed;
                if !settings.enabled {
                    state.devices.clear();
                    state.sessions.clear();
                    state.booting_devices = Some(Some(vec![]));
                }
            })
            .await;
            self.prune_launchers().await?;
        }
        if input.agent_access_enabled.flatten() == Some(true) {
            let _ = self.agent_readiness_if_supported(None).await?;
        }
        self.list().await
    }
    pub async fn open(
        &self,
        input: DeviceOpenInput,
        context: OpenContext,
    ) -> Result<DeviceSession> {
        self.open_authorized(input, context, Arc::new(Unrestricted))
            .await
    }
    pub async fn open_authorized(
        &self,
        input: DeviceOpenInput,
        context: OpenContext,
        admission: Arc<dyn Admission>,
    ) -> Result<DeviceSession> {
        let service = self.clone();
        tokio::spawn(async move { service.open_inner(input, context, admission).await })
            .await
            .map_err(|_| Error::Cancelled("open"))?
    }
    async fn open_inner(
        &self,
        input: DeviceOpenInput,
        context: OpenContext,
        admission: Arc<dyn Admission>,
    ) -> Result<DeviceSession> {
        {
            let _guard = self.inner.lifecycle.lock().await;
            admission.check().await?;
        }
        let entry = self
            .host(
                input
                    .host_id
                    .as_ref()
                    .and_then(|v| v.as_ref())
                    .map(AsRef::as_ref),
            )
            .await?;
        let id = entry.host.id();
        let availability = entry.host.availability(input.platform.clone()).await;
        if !availability.available {
            return Err(Error::platform(
                id,
                input.platform,
                availability
                    .reason
                    .flatten()
                    .unwrap_or_else(|| "Platform toolchain missing.".into()),
            ));
        }
        let ready = self.readiness(Some(id)).await?;
        let mut state = self.refresh(&entry, &ready).await?;
        let mut device = find_device(&state, id, input.device_id.as_ref())
            .ok_or_else(|| Error::not_found(id, input.device_id.as_ref()))?;
        if device.platform == DevicePlatform::Android && !device.physical && ready.routing.is_none()
        {
            let mut cmd = Command::new(
                &ready.node_path,
                [
                    "-e",
                    "const {spawnSync}=require('node:child_process');const r=spawnSync(process.env.SERVE_EMU_FFMPEG?.trim()||'ffmpeg',['-hide_banner','-encoders'],{encoding:'utf8',timeout:10000,killSignal:'SIGKILL'});process.stdout.write(r.stdout||'');process.stderr.write(r.stderr||'');process.exitCode=r.error?1:(r.status??1);",
                ],
            );
            cmd.timeout = std::time::Duration::from_secs(20);
            let encoders = ready.commands.run(cmd).await?;
            let re = regex::Regex::new(r"(?m)^\s*V\S*[ \t]+libx264(?:[ \t]|$)")
                .map_err(|_| crate::error::invalid("encoders"))?;
            if encoders.code != 0
                || !re.is_match(&format!("{}\n{}", encoders.stdout, encoders.stderr))
            {
                return Err(Error::platform(
                    id,
                    DevicePlatform::Android,
                    "Android emulator video needs working ffmpeg with the libx264 encoder. Install it on the device host's PATH or set SERVE_EMU_FFMPEG there, then retry opening the emulator.",
                ));
            }
        }
        if !device.booted && input.boot.flatten() != Some(false) {
            let booting: BootingDevice = serde_json::from_value({
                let mut v =
                    serde_json::to_value(&device).map_err(|_| crate::error::invalid("boot"))?;
                v["threadId"] = serde_json::json!(input.thread_id);
                v
            })
            .map_err(|_| crate::error::invalid("boot"))?;
            self.publish(|s| {
                let entries = s
                    .booting_devices
                    .get_or_insert_with(|| Some(vec![]))
                    .get_or_insert_with(Vec::new);
                entries.retain(|e| e.host_id != device.host_id || e.id != device.id);
                entries.push(booting);
            })
            .await;
            let result = ready.devices.boot(&device, &context).await;
            self.publish(|s| {
                if let Some(Some(entries)) = &mut s.booting_devices {
                    entries.retain(|e| e.host_id != device.host_id || e.id != device.id);
                }
            })
            .await;
            let booted_id = result?;
            state = self.refresh(&entry, &ready).await?;
            if state
                .host_statuses
                .get(&DeviceHostId::from(id))
                .and_then(|h| h.incomplete_platforms.as_ref())
                .and_then(|v| v.as_ref())
                .is_some_and(|p| p.contains(&device.platform))
            {
                return Err(Error::not_found(id, &booted_id));
            }
            device = find_device(&state, id, &booted_id)
                .or_else(|| find_device(&state, id, device.id.as_ref()))
                .ok_or_else(|| Error::not_found(id, &booted_id))?;
        } else if device.booted {
            ready.devices.attach(&device).await?;
        }
        let _guard = self.inner.lifecycle.lock().await;
        admission.check().await?;
        if !self.unchanged(&entry).await {
            return Err(Error::host(
                id,
                Some(entry.host.label()),
                "Host configuration changed. Retry the operation.",
            ));
        }
        if !self.inner.settings.read().await?.enabled {
            return Err(Error::host(
                id,
                Some(entry.host.label()),
                "Device support was turned off while the device was opening.",
            ));
        }
        let session = DeviceSession {
            thread_id: input.thread_id,
            host_id: device.host_id,
            device_id: device.id,
            platform: device.platform,
            opened_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        };
        self.publish(|state| {
            state.sessions.retain(|s| {
                !(s.thread_id == session.thread_id
                    && s.host_id == session.host_id
                    && s.device_id == session.device_id)
            });
            state.sessions.push(session.clone());
        })
        .await;
        Ok(session)
    }
    pub async fn close(&self, input: DeviceCloseInput) -> Result<()> {
        self.close_authorized(input, Arc::new(Unrestricted)).await
    }
    pub async fn close_authorized(
        &self,
        input: DeviceCloseInput,
        admission: Arc<dyn Admission>,
    ) -> Result<()> {
        let service = self.clone();
        tokio::spawn(async move { service.close_inner(input, admission).await })
            .await
            .map_err(|_| Error::Cancelled("close"))?
    }
    async fn close_inner(
        &self,
        input: DeviceCloseInput,
        admission: Arc<dyn Admission>,
    ) -> Result<()> {
        let shutdown = input.shutdown.flatten() == Some(true);
        let closing;
        {
            let _guard = self.inner.lifecycle.lock().await;
            let state = self.state().await;
            admission.check().await?;
            closing = state
                .sessions
                .iter()
                .filter(|s| {
                    s.thread_id == input.thread_id
                        && input
                            .host_id
                            .as_ref()
                            .and_then(|v| v.as_ref())
                            .is_none_or(|id| *id == s.host_id)
                        && input
                            .device_id
                            .as_ref()
                            .and_then(|v| v.as_ref())
                            .is_none_or(|id| *id == s.device_id)
                })
                .cloned()
                .collect::<Vec<_>>();
            let ending = if shutdown {
                state
                    .sessions
                    .iter()
                    .filter(|s| {
                        closing
                            .iter()
                            .any(|c| s.host_id == c.host_id && s.device_id == c.device_id)
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                closing.clone()
            };
            for session in &ending {
                let entry = self.host(Some(session.host_id.as_ref())).await?;
                let ready = entry.host.current().await;
                if ready.is_none() && entry.host.summary().await.kind == DeviceHostKind::Ssh {
                    return Err(Error::host(
                        entry.host.id(),
                        Some(entry.host.label()),
                        "Device host is reconnecting. Retry close after it reconnects.",
                    ));
                }
                if let Some(release) = ready.as_ref().and_then(|r| r.release.as_ref()) {
                    release
                        .release(
                            &agent::session(
                                session.thread_id.as_ref(),
                                session.host_id.as_ref(),
                                session.device_id.as_ref(),
                            ),
                            session.device_id.as_ref(),
                        )
                        .await?;
                }
            }
            self.release_launchers(&ending).await?;
            if shutdown {
                for session in &closing {
                    let ready = self
                        .current_readiness(Some(session.host_id.as_ref()))
                        .await
                        .ok_or_else(|| {
                            Error::host(
                                session.host_id.as_ref(),
                                None,
                                "Device host is not running. Reconnect before retrying close.",
                            )
                        })?;
                    self.power_off(
                        &ready,
                        session.host_id.as_ref(),
                        session.device_id.as_ref(),
                        session.platform.clone(),
                    )
                    .await?;
                }
            }
            self.publish(|state| state.sessions.retain(|s| !ending.contains(s)))
                .await;
            self.prune_launchers().await?;
        }
        if shutdown {
            for session in closing {
                let _ = self.refresh_host(session.host_id.as_ref()).await;
            }
        }
        Ok(())
    }
    async fn power_off(
        &self,
        ready: &Ready,
        host: &str,
        id: &str,
        platform: DevicePlatform,
    ) -> Result<()> {
        ready.devices.shutdown(id, platform).await?;
        self.publish(|s| {
            for device in &mut s.devices {
                if device.host_id.as_ref() == host && device.id.as_ref() == id {
                    device.booted = false;
                }
            }
        })
        .await;
        Ok(())
    }
    pub async fn shutdown(&self, input: DeviceShutdownInput) -> Result<()> {
        let service = self.clone();
        tokio::spawn(async move { service.shutdown_inner(input).await })
            .await
            .map_err(|_| Error::Cancelled("shutdown"))?
    }
    async fn shutdown_inner(&self, input: DeviceShutdownInput) -> Result<()> {
        let entry = self
            .host(
                input
                    .host_id
                    .as_ref()
                    .and_then(|v| v.as_ref())
                    .map(AsRef::as_ref),
            )
            .await?;
        let id = entry.host.id();
        {
            let _guard = self.inner.lifecycle.lock().await;
            self.publish(|s| {
                s.sessions
                    .retain(|s| !(s.host_id.as_ref() == id && s.device_id == input.device_id))
            })
            .await;
            self.prune_launchers().await?;
            let ready = self.ready_locked(&entry, false).await?;
            self.power_off(&ready, id, input.device_id.as_ref(), input.platform)
                .await?;
        }
        let _ = self.refresh_host(id).await;
        Ok(())
    }
    async fn refresh_host(&self, id: &str) -> Result<DeviceServiceState> {
        let entry = self.host(Some(id)).await?;
        if let Some(ready) = entry.host.current().await {
            self.refresh(&entry, &ready).await
        } else {
            Ok(self.state().await)
        }
    }
    async fn resolve_device(&self, host: Option<&str>, id: &str) -> Result<(Ready, DeviceSummary)> {
        let entry = self.host(host).await?;
        let ready = self.readiness(Some(entry.host.id())).await?;
        if let Some(device) = find_device(&self.state().await, entry.host.id(), id) {
            return Ok((ready, device));
        }
        let state = self.refresh(&entry, &ready).await?;
        let device = find_device(&state, entry.host.id(), id)
            .ok_or_else(|| Error::not_found(entry.host.id(), id))?;
        Ok((ready, device))
    }
    pub async fn screenshot(
        &self,
        host: Option<&str>,
        id: &str,
    ) -> Result<(DeviceSummary, Vec<u8>)> {
        let (ready, device) = self.resolve_device(host, id).await?;
        let png = ready.devices.screenshot(&device).await?;
        Ok((device, png))
    }
    pub async fn detail(&self, input: DeviceDetailInput) -> Result<DeviceDetail> {
        let (ready, device) = self
            .resolve_device(
                input
                    .host_id
                    .as_ref()
                    .and_then(|v| v.as_ref())
                    .map(AsRef::as_ref),
                input.device_id.as_ref(),
            )
            .await?;
        crate::actions::read_detail(&ready, &device).await
    }
    pub async fn action(&self, input: DeviceActionInput) -> Result<DeviceDetail> {
        let value = serde_json::to_value(&input).map_err(|_| crate::error::invalid("action"))?;
        let host = value["hostId"].as_str();
        let id = value["deviceId"]
            .as_str()
            .ok_or_else(|| crate::error::invalid("action"))?;
        let (ready, device) = self.resolve_device(host, id).await?;
        crate::actions::run_action(&ready, device.platform.clone(), &input).await?;
        crate::actions::read_detail(&ready, &device).await
    }
    pub async fn sessions_for_thread(&self, thread: &ThreadId) -> Vec<DeviceSession> {
        self.state()
            .await
            .sessions
            .into_iter()
            .filter(|s| s.thread_id == *thread)
            .collect()
    }
    pub async fn locate_device(
        &self,
        host: Option<&str>,
        id: &str,
    ) -> Result<Option<DeviceSummary>> {
        let state = self.state().await;
        locate(
            &state,
            host,
            id,
            state
                .devices
                .iter()
                .filter(|d| d.id.as_ref() == id)
                .map(|d| (d.host_id.as_ref(), d.clone()))
                .collect(),
        )
    }
    pub async fn locate_session(
        &self,
        thread: &ThreadId,
        host: Option<&str>,
        id: &str,
    ) -> Result<Option<DeviceSession>> {
        let state = self.state().await;
        locate(
            &state,
            host,
            id,
            state
                .sessions
                .iter()
                .filter(|s| s.thread_id == *thread && s.device_id.as_ref() == id)
                .map(|s| (s.host_id.as_ref(), s.clone()))
                .collect(),
        )
    }
    pub async fn agent_target(
        &self,
        thread: &ThreadId,
        host: &str,
        id: &str,
    ) -> Result<AgentTarget> {
        let ready=self.agent_readiness_if_supported(Some(host)).await?.ok_or_else(||Error::host(host,None,"Agent device access requires enabled device support, agent access, and an available simulator platform on this host."))?;
        let _guard = self.inner.lifecycle.lock().await;
        let (endpoint, per_device) = match ready.devices.agent_endpoint(id).await? {
            Some(e) => (e, true),
            None => (
                ready.agent.ok_or_else(|| {
                    Error::host(host, None, "This host has no agent-device daemon.")
                })?,
                false,
            ),
        };
        let path = self
            .inner
            .launchers
            .as_ref()
            .ok_or_else(|| {
                Error::host(
                    host,
                    None,
                    "Agent configuration is unavailable in this device service.",
                )
            })?
            .configure(host, per_device.then_some(id), &endpoint)
            .await?;
        let own = agent::session(thread.as_ref(), host, id);
        let session = endpoint.session.clone().unwrap_or(own);
        Ok(AgentTarget {
            agent_device_id: if per_device {
                endpoint.agent_device_id
            } else {
                Some(id.into())
            },
            session: session.clone(),
            args: vec!["--config".into(), path, "--session".into(), session],
            install_directory: endpoint.install_directory,
            host_session: endpoint.session.is_some(),
        })
    }
    pub async fn agent_launcher(
        &self,
        thread: &ThreadId,
        host: &str,
        id: &str,
        target: &AgentTarget,
        args: &[String],
    ) -> Result<Option<String>> {
        let _guard = self.inner.launcher_lock.lock().await;
        let key = agent::session(thread.as_ref(), host, id);
        if !self.state().await.sessions.iter().any(|s| {
            s.thread_id == *thread && s.host_id.as_ref() == host && s.device_id.as_ref() == id
        }) {
            return Ok(None);
        }
        let launcher = self
            .inner
            .launchers
            .as_ref()
            .ok_or_else(|| {
                Error::host(
                    host,
                    None,
                    "Agent launchers are unavailable in this device service.",
                )
            })?
            .bind(&key, args)
            .await?;
        self.inner
            .state
            .lock()
            .await
            .launchers
            .insert(key, (launcher.clone(), target.host_session));
        Ok(Some(launcher))
    }
    async fn release_launchers(&self, sessions: &[DeviceSession]) -> Result<()> {
        let _guard = self.inner.launcher_lock.lock().await;
        if let Some(launchers) = &self.inner.launchers {
            for session in sessions {
                let key = agent::session(
                    session.thread_id.as_ref(),
                    session.host_id.as_ref(),
                    session.device_id.as_ref(),
                );
                let bound = self.inner.state.lock().await.launchers.get(&key).cloned();
                if let Some((launcher, false)) = bound {
                    launchers.release(&launcher).await?;
                    self.inner.state.lock().await.launchers.remove(&key);
                }
            }
        }
        Ok(())
    }
    async fn prune_launchers(&self) -> Result<()> {
        let _guard = self.inner.launcher_lock.lock().await;
        let live: BTreeSet<_> = self
            .state()
            .await
            .sessions
            .iter()
            .map(|s| {
                agent::session(
                    s.thread_id.as_ref(),
                    s.host_id.as_ref(),
                    s.device_id.as_ref(),
                )
            })
            .collect();
        let retired = {
            let mut state = self.inner.state.lock().await;
            let retired = state
                .launchers
                .iter()
                .filter(|(k, _)| !live.contains(*k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>();
            for (k, _) in &retired {
                state.launchers.remove(k);
            }
            retired
        };
        if let Some(launchers) = &self.inner.launchers {
            for (_, (launcher, shared)) in retired {
                if !shared {
                    launchers.release(&launcher).await?;
                }
            }
            launchers.prune(&live).await?;
        }
        Ok(())
    }
    pub async fn test_host(&self, input: SshDeviceHostConfig) -> Result<DeviceHostSummary> {
        let factory = self.inner.factory.as_ref().ok_or_else(|| {
            Error::host(
                input.id.as_ref(),
                Some(input.label.as_ref()),
                "SSH probing is unavailable in this device service.",
            )
        })?;
        if factory.is_local(&input).await {
            return Ok(self.host(None).await?.host.summary().await);
        }
        factory.test(&input).await
    }
    pub async fn eas_account(&self, input: DeviceEasAccountInput) -> Result<DeviceEasAccount> {
        let result = match &self.inner.accounts {
            Some(accounts) => accounts.eas_account(input).await,
            None => Err(Error::Unavailable(
                "EAS accounts are unavailable in this device service.".into(),
            )),
        };
        result.map_err(|error| {
            Error::EasAccount(DeviceEasAccountError {
                _tag: DeviceEasAccountErrorTag::Error,
                reason: error.to_string(),
            })
        })
    }
    pub async fn simlock_identity(
        &self,
        input: DeviceSimlockIdentityInput,
    ) -> Result<DeviceSimlockIdentity> {
        let result = match &self.inner.accounts {
            Some(accounts) => accounts.simlock_identity(input).await,
            None => Err(Error::Unavailable(
                "Simlock hosts are unavailable in this device service.".into(),
            )),
        };
        result.map_err(|error| {
            Error::SimlockIdentity(DeviceSimlockIdentityError {
                _tag: DeviceSimlockIdentityErrorTag::Error,
                reason: error.to_string(),
            })
        })
    }
    pub async fn reconcile(&self, configs: Vec<DeviceHostConfig>) -> Result<()> {
        let Some(factory) = &self.inner.factory else {
            return if configs.is_empty() {
                Ok(())
            } else {
                Err(Error::Unavailable(
                    "Device host factory is unavailable.".into(),
                ))
            };
        };
        let mut next = Vec::new();
        let mut ids = BTreeSet::from([LOCAL_DEVICE_HOST_ID.to_owned()]);
        for config in configs {
            crate::cloud::validate_config(&config)?;
            let id = match &config {
                DeviceHostConfig::SshDeviceHostConfig(c) => c.id.as_ref(),
                DeviceHostConfig::EasDeviceHostConfig(c) => c.id.as_ref(),
                DeviceHostConfig::SimlockDeviceHostConfig(c) => c.id.as_ref(),
            };
            if !ids.insert(id.to_owned()) {
                continue;
            }
            if let DeviceHostConfig::SshDeviceHostConfig(c) = &config
                && factory.is_local(c).await
            {
                continue;
            }
            next.push(config);
        }
        let _guard = self.inner.lifecycle.lock().await;
        let previous = self.inner.state.lock().await.hosts.clone();
        let mut entries = previous
            .iter()
            .filter(|e| e.config.is_none())
            .cloned()
            .collect::<Vec<_>>();
        for config in next {
            if let Some(entry) = previous.iter().find(|e| e.config.as_ref() == Some(&config)) {
                entries.push(entry.clone());
            } else {
                let host = factory.create(&config).await?;
                let generation = {
                    let mut state = self.inner.state.lock().await;
                    state.generation += 1;
                    state.generation
                };
                entries.push(HostEntry {
                    host,
                    generation,
                    config: Some(config),
                });
            }
        }
        let changed = entries.len() != previous.len()
            || entries
                .iter()
                .zip(&previous)
                .any(|(a, b)| !Arc::ptr_eq(&a.host, &b.host));
        if !changed {
            return Ok(());
        }
        let retained: BTreeSet<_> = entries
            .iter()
            .filter(|e| previous.iter().any(|p| Arc::ptr_eq(&p.host, &e.host)))
            .map(|e| e.host.id().to_owned())
            .collect();
        let mut summaries = Vec::new();
        for entry in &entries {
            summaries.push(entry.host.summary().await);
        }
        self.inner.state.lock().await.hosts = entries;
        self.publish(|s| {
            s.hosts = summaries;
            s.devices.retain(|d| retained.contains(d.host_id.as_ref()));
            s.sessions.retain(|d| retained.contains(d.host_id.as_ref()));
            s.host_statuses
                .retain(|id, _| retained.contains(id.as_ref()));
        })
        .await;
        self.prune_launchers().await?;
        for entry in previous {
            if !retained.contains(entry.host.id()) {
                entry.host.retire().await?;
            }
        }
        Ok(())
    }
    pub async fn stop(&self) -> Result<()> {
        self.configure(DeviceConfigureInput {
            enabled: Some(Some(false)),
            agent_access_enabled: None,
            onboarding_completed: None,
        })
        .await
        .map(|_| ())
    }
}

/// Applies the TypeScript contract's cloud-host encoding transform before Serde.
pub fn wire_state(mut state: DeviceServiceState) -> DeviceServiceState {
    let mut eas = Vec::new();
    let mut simlock = Vec::new();
    state.hosts.retain(|host| match host.kind {
        DeviceHostKind::Eas => {
            eas.push(host.clone());
            false
        }
        DeviceHostKind::Simlock => {
            simlock.push(host.clone());
            false
        }
        _ => true,
    });
    state.eas_hosts = Some(Some(eas));
    state.simlock_hosts = Some(Some(simlock));
    state
}

fn find_device(state: &DeviceServiceState, host: &str, id: &str) -> Option<DeviceSummary> {
    state
        .devices
        .iter()
        .find(|d| d.host_id.as_ref() == host && d.id.as_ref() == id)
        .cloned()
}
fn locate<T: Clone>(
    state: &DeviceServiceState,
    host: Option<&str>,
    id: &str,
    matches: Vec<(&str, T)>,
) -> Result<Option<T>> {
    let on_host = matches
        .iter()
        .filter(|(h, _)| host.is_none_or(|id| id == *h))
        .collect::<Vec<_>>();
    let ids: BTreeSet<_> = on_host.iter().map(|(h, _)| *h).collect();
    let name = |id: &str| {
        state
            .hosts
            .iter()
            .find(|h| h.id.as_ref() == id)
            .map(|h| format!("'{}' (hostId {id})", h.label.as_ref()))
            .unwrap_or_else(|| format!("hostId {id}"))
    };
    if ids.len() > 1 {
        return Err(Error::Ambiguous {
            device_id: id.into(),
            hosts: ids.into_iter().map(name).collect(),
        });
    }
    if let Some((_, value)) = on_host.last() {
        return Ok(Some(value.clone()));
    }
    if let Some(host) = host
        && !matches.is_empty()
    {
        return Err(Error::OtherHost {
            device_id: id.into(),
            host: name(host),
            hosts: matches
                .iter()
                .map(|(h, _)| *h)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(name)
                .collect(),
        });
    }
    Ok(None)
}
