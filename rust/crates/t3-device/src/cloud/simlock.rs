use super::{
    Secrets,
    eas::{NoCommands, decode_segment, encode_segment},
};
use crate::{
    Error, Result, agent,
    host::{
        self, AgentEndpoint, DeviceHost, DeviceOperations, Listing, OpenContext, Phase, Ready,
        Routing, Upstream,
    },
    io::{Command, Commands, Http, HttpRequest, Secret, request_bytes},
};
use async_trait::async_trait;
use http::Method;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use t3_contracts::device::*;
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;

pub fn gateway(raw: &str) -> Result<String> {
    let url = crate::io::validated_url(raw)?;
    if url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || raw.chars().any(char::is_whitespace)
    {
        return Err(crate::error::invalid("Simlock gateway URL"));
    }
    Ok(url.origin().ascii_serialization())
}
pub fn machine_token(environment: &BTreeMap<String, String>) -> Option<(String, Secret)> {
    let base = environment
        .get("AGENT_DEVICE_DAEMON_BASE_URL")?
        .trim()
        .trim_end_matches('/');
    let token = environment.get("AGENT_DEVICE_DAEMON_AUTH_TOKEN")?.trim();
    if token.is_empty() {
        return None;
    }
    let base = base.strip_suffix("/v1/agent-device")?;
    Some((gateway(base).ok()?, Secret::new(token)))
}
pub async fn resolve_token(
    secrets: &dyn Secrets,
    gateway_url: &str,
    saved: Option<&SimlockDeviceHostConfig>,
    environment: &BTreeMap<String, String>,
) -> Result<Option<(Secret, DeviceSimlockTokenSource)>> {
    let target = gateway(gateway_url)?;
    if let Some(saved) = saved
        && gateway(saved.gateway_url.as_ref()).ok().as_deref() == Some(&target)
    {
        let token = secrets.resolve(&saved.agent_token).await?;
        if !token.expose().is_empty() {
            return Ok(Some((token, DeviceSimlockTokenSource::Settings)));
        }
    }
    Ok(machine_token(environment)
        .filter(|(url, _)| *url == target)
        .map(|(_, token)| (token, DeviceSimlockTokenSource::Env)))
}
pub async fn gateway_json(
    http: &dyn Http,
    base: &str,
    token: &Secret,
    method: Method,
    path: &str,
    body: Option<&Value>,
) -> Result<Value> {
    let mut request =
        HttpRequest::new(method, &format!("{base}{path}"), Duration::from_secs(15))?.bearer(token);
    if let Some(body) = body {
        request = request.json(body)?;
    }
    let (status, bytes) = request_bytes(http, request).await.map_err(|_| {
        Error::Unavailable(format!("Could not reach the Simlock gateway at {base}."))
    })?;
    if status == 401 || status == 403 {
        return Err(Error::GatewayStatus {
            status,
            reason: "The Simlock gateway refused the agent token for this environment.".into(),
        });
    }
    if !(200..300).contains(&status) {
        return Err(Error::GatewayStatus {
            status,
            reason: format!("The Simlock gateway at {base} answered HTTP {status}."),
        });
    }
    serde_json::from_slice(&bytes).map_err(|_| {
        Error::Unavailable(format!(
            "The Simlock gateway at {base} sent an answer T3 could not read."
        ))
    })
}
pub struct SimlockOptions {
    pub config: SimlockDeviceHostConfig,
    pub state: PathBuf,
    pub node: crate::io::NodeRuntime,
    pub agent_entry: PathBuf,
    pub cli_provider: Option<Arc<dyn crate::host::AgentCliProvider>>,
    pub commands: Arc<dyn Commands>,
    pub http: Arc<dyn Http>,
    pub secrets: Arc<dyn Secrets>,
    pub environment: BTreeMap<String, String>,
}
#[derive(Clone)]
struct Lease {
    id: String,
    device: DeviceSummary,
    session: String,
    dir: PathBuf,
    config: PathBuf,
    install: PathBuf,
    token: Secret,
    cancel: CancellationToken,
}
struct State {
    leases: BTreeMap<String, Lease>,
    starting: BTreeMap<String, (CancellationToken, watch::Receiver<bool>)>,
    epoch: u64,
    ready: bool,
    cli: Option<crate::host::AgentCli>,
    catalog: Option<(tokio::time::Instant, Vec<DeviceSummary>)>,
    platforms: Vec<DevicePlatformAvailability>,
}
struct Inner {
    options: SimlockOptions,
    gateway: String,
    state: Mutex<State>,
    catalog_lock: Mutex<()>,
    start_lock: Mutex<()>,
}
#[derive(Clone)]
pub struct SimlockHost {
    inner: Arc<Inner>,
}
impl SimlockHost {
    pub fn new(options: SimlockOptions) -> Result<Self> {
        super::validate_config(&DeviceHostConfig::SimlockDeviceHostConfig(
            options.config.clone(),
        ))?;
        let gateway = gateway(options.config.gateway_url.as_ref())?;
        Ok(Self {
            inner: Arc::new(Inner {
                options,
                gateway,
                state: Mutex::new(State {
                    leases: BTreeMap::new(),
                    starting: BTreeMap::new(),
                    epoch: 0,
                    ready: false,
                    cli: None,
                    catalog: None,
                    platforms: vec![],
                }),
                catalog_lock: Mutex::new(()),
                start_lock: Mutex::new(()),
            }),
        })
    }
    fn unavailable(&self, reason: impl Into<String>) -> Error {
        Error::host(self.id(), Some(self.label()), reason)
    }
    async fn token(&self) -> Result<Secret> {
        resolve_token(
            self.inner.options.secrets.as_ref(),
            &self.inner.gateway,
            Some(&self.inner.options.config),
            &self.inner.options.environment,
        )
        .await?
        .map(|(t, _)| t)
        .ok_or_else(|| {
            self.unavailable(format!(
                "Add an agent token for {} in Settings.",
                self.label()
            ))
        })
    }
    async fn catalog(&self) -> Result<Vec<DeviceSummary>> {
        let _guard = self.inner.catalog_lock.lock().await;
        if let Some((at, devices)) = &self.inner.state.lock().await.catalog
            && at.elapsed() < Duration::from_secs(60)
        {
            return Ok(devices.clone());
        }
        let result: Result<Vec<DeviceSummary>> = async {
            let token = self.token().await?;
            let value = gateway_json(
                self.inner.options.http.as_ref(),
                &self.inner.gateway,
                &token,
                Method::GET,
                "/v1/catalog",
                None,
            )
            .await?;
            let mut devices = Vec::new();
            let mut all_nonempty = true;
            for platform in value["platforms"]
                .as_array()
                .ok_or_else(|| crate::error::invalid("Simlock catalog"))?
            {
                let kind = match platform["platform"].as_str() {
                    Some("ios") => DevicePlatform::Ios,
                    Some("android") => DevicePlatform::Android,
                    _ => continue,
                };
                let models = platform["modelRuntimes"]
                    .as_object()
                    .ok_or_else(|| crate::error::invalid("Simlock catalog"))?;
                let mut added = 0;
                for (model, runtimes) in models {
                    let runtimes = runtimes
                        .as_array()
                        .ok_or_else(|| crate::error::invalid("Simlock runtimes"))?;
                    let class = platform["modelClasses"].get(model).and_then(Value::as_str);
                    let workers = platform["modelWorkers"]
                        .get(model)
                        .and_then(Value::as_array);
                    if runtimes.is_empty()
                        || workers.is_some_and(|w| w.is_empty())
                        || class.is_some_and(|c| !["phone", "tablet"].contains(&c))
                    {
                        continue;
                    }
                    crate::error::validate_selector(model, "Simlock model")?;
                    let platform_name = if kind == DevicePlatform::Ios {
                        "ios"
                    } else {
                        "android"
                    };
                    let runtime = runtimes.last().and_then(Value::as_str).unwrap_or("");
                    devices.push(DeviceSummary {
                        host_id: self.id().into(),
                        id: format!("simlock:{platform_name}:{model}").into(),
                        name: model.as_str().into(),
                        platform: kind.clone(),
                        version: format!(
                            "{}{}{}",
                            if kind == DevicePlatform::Ios {
                                "iOS"
                            } else {
                                "Android"
                            },
                            if runtime.is_empty() { "" } else { " " },
                            runtime
                        ),
                        booted: false,
                        physical: false,
                    });
                    added += 1;
                }
                if added == 0 {
                    all_nonempty = false;
                }
            }
            let mut state = self.inner.state.lock().await;
            state.catalog = (!devices.is_empty() && all_nonempty)
                .then(|| (tokio::time::Instant::now(), devices.clone()));
            state.platforms = [DevicePlatform::Ios, DevicePlatform::Android]
                .into_iter()
                .map(|p| {
                    host::available(
                        p.clone(),
                        (!devices.iter().any(|d| d.platform == p)).then(|| {
                            format!(
                                "The pool has no {} devices.",
                                if p == DevicePlatform::Ios {
                                    "iOS"
                                } else {
                                    "Android"
                                }
                            )
                        }),
                    )
                })
                .collect();
            Ok(devices)
        }
        .await;
        if let Err(error) = &result {
            self.inner.state.lock().await.platforms =
                [DevicePlatform::Ios, DevicePlatform::Android]
                    .into_iter()
                    .map(|p| host::available(p, Some(error.to_string())))
                    .collect();
        }
        result
    }
    async fn cli(
        &self,
        lease: &Lease,
        args: Vec<String>,
        timeout: Duration,
        cwd: Option<PathBuf>,
    ) -> Result<String> {
        let cli = self
            .inner
            .state
            .lock()
            .await
            .cli
            .clone()
            .ok_or_else(|| self.unavailable("Agent tools are not ready."))?;
        let mut args = [
            vec![cli.entry.to_string_lossy().into_owned()],
            args,
            vec![
                "--config".into(),
                lease.config.to_string_lossy().into_owned(),
                "--session".into(),
                lease.session.clone(),
                "--json".into(),
            ],
        ]
        .concat();
        let mut command = Command::new(&cli.node, std::mem::take(&mut args));
        command.timeout = timeout;
        command.cwd = cwd;
        for key in [
            "AGENT_DEVICE_DAEMON_BASE_URL",
            "AGENT_DEVICE_DAEMON_AUTH_TOKEN",
            "AGENT_DEVICE_CONFIG",
            "AGENT_DEVICE_STATE_DIR",
        ] {
            command.env.insert(key.into(), None);
        }
        for (key, value) in [
            ("ELECTRON_RUN_AS_NODE", "1"),
            ("AGENT_DEVICE_NO_UPDATE_NOTIFIER", "1"),
            ("FORCE_COLOR", "0"),
            ("NO_COLOR", "1"),
        ] {
            command.env.insert(key.into(), Some(Secret::new(value)));
        }
        let result = self.inner.options.commands.run(command).await?;
        if result.code != 0 {
            let value = serde_json::from_str::<Value>(&result.stdout).ok();
            let mut detail = value
                .as_ref()
                .and_then(|v| v.pointer("/error/message"))
                .and_then(Value::as_str)
                .unwrap_or("agent-device command failed.")
                .to_owned();
            if let Some(value) = value {
                if let Some(hint) = value.pointer("/error/hint").and_then(Value::as_str) {
                    detail.push(' ');
                    detail.push_str(hint);
                }
                if let Some(line) = value.pointer("/error/details/line").and_then(Value::as_str) {
                    detail.push_str(" The daemon answered: ");
                    detail.push_str(&crate::error::bounded(line, 300));
                }
            }
            return Err(self.unavailable(crate::error::bounded(
                &detail.replace(lease.token.expose(), "[redacted]"),
                1997,
            )));
        }
        Ok(result.stdout)
    }
    async fn release(&self, lease: Lease) {
        lease.cancel.cancel();
        if self
            .cli(&lease, vec!["close".into()], Duration::from_secs(120), None)
            .await
            .is_err()
        {
            let _ = gateway_json(
                self.inner.options.http.as_ref(),
                &self.inner.gateway,
                &lease.token,
                Method::DELETE,
                &format!("/v1/leases/{}", encode_segment(&lease.id)),
                None,
            )
            .await;
        }
        self.inner.state.lock().await.leases.remove(&lease.id);
        let _ = tokio::fs::remove_dir_all(lease.dir).await;
    }
    async fn open_lease(
        &self,
        device: DeviceSummary,
        context: OpenContext,
        name: String,
        epoch: u64,
        cancel: CancellationToken,
    ) -> Result<String> {
        let target = device
            .id
            .as_ref()
            .strip_prefix("simlock:")
            .and_then(|s| s.split_once(':'))
            .filter(|(p, _)| ["ios", "android"].contains(p))
            .ok_or_else(|| {
                self.unavailable(
                    "This Simlock device is no longer leased. Open it again from the list.",
                )
            })?;
        crate::error::validate_selector(target.1, "Simlock model")?;
        let token = self.token().await?;
        let dir = self
            .inner
            .options
            .state
            .join("device/simlock")
            .join(self.id())
            .join(&name);
        let config = dir.join("agent-device.json");
        let install = match context.workspace_root {
            Some(path) => tokio::fs::canonicalize(path)
                .await
                .unwrap_or_else(|_| dir.clone()),
            None => dir.clone(),
        };
        let endpoint = AgentEndpoint {
            base_url: format!("{}/v1/agent-device", self.inner.gateway),
            token: token.clone(),
            entry_path: self.inner.options.agent_entry.clone(),
            agent_device_id: None,
            session: Some(format!("t3-{name}")),
            state_dir: Some(dir.clone()),
            install_directory: Some(install.clone()),
        };
        agent::write_config(&config, &endpoint).await?;
        let mut lease = Lease {
            id: String::new(),
            device: device.clone(),
            session: format!("t3-{name}"),
            dir,
            config,
            install,
            token,
            cancel: CancellationToken::new(),
        };
        let start = async {
            self.cli(
                &lease,
                vec![
                    "connect".into(),
                    "proxy".into(),
                    "--daemon-base-url".into(),
                    endpoint.base_url,
                ],
                Duration::from_secs(120),
                None,
            )
            .await?;
            self.cli(
                &lease,
                vec![
                    "open".into(),
                    "--platform".into(),
                    target.0.into(),
                    "--device".into(),
                    target.1.into(),
                ],
                Duration::from_secs(420),
                Some(lease.install.clone()),
            )
            .await?;
            let value: Value = serde_json::from_slice(
                &tokio::fs::read(
                    lease
                        .dir
                        .join("remote-connections")
                        .join(format!("{}.json", lease.session)),
                )
                .await
                .map_err(|_| self.unavailable("agent-device recorded no Simlock lease."))?,
            )
            .map_err(|_| crate::error::invalid("Simlock lease record"))?;
            let id = value["leaseId"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| crate::error::invalid("Simlock lease id"))?
                .to_owned();
            crate::error::validate_selector(&id, "Simlock lease id")?;
            if matches!(id.as_str(), "." | "..") || id.contains(['/', '\\']) {
                return Err(crate::error::invalid("Simlock lease id"));
            }
            if self.inner.state.lock().await.epoch != epoch {
                return Err(
                    self.unavailable("Device support stopped while the device was opening.")
                );
            }
            Ok(id)
        };
        let result = tokio::select! {biased;_=cancel.cancelled()=>Err(self.unavailable("Device support stopped while the device was opening.")),r=start=>r};
        match result {
            Ok(id) => {
                lease.id = id.clone();
                lease.device.id = id.as_str().into();
                lease.device.booted = true;
                self.inner
                    .state
                    .lock()
                    .await
                    .leases
                    .insert(id.clone(), lease.clone());
                let weak = Arc::downgrade(&self.inner);
                let stop = lease.cancel.clone();
                let deadline = tokio::time::Instant::now()
                    + Duration::from_secs(self.inner.options.config.max_lease_minutes * 60);
                let renewal = lease.clone();
                let mut next = tokio::time::Instant::now() + Duration::from_secs(120);
                tokio::spawn(async move {
                    loop {
                        tokio::select! {biased;_=stop.cancelled()=>return,_=tokio::time::sleep_until(next.min(deadline))=>()}
                        let Some(inner) = weak.upgrade() else { return };
                        let host = SimlockHost { inner };
                        if tokio::time::Instant::now() >= deadline {
                            host.release(renewal).await;
                            return;
                        }
                        let result = gateway_json(
                            host.inner.options.http.as_ref(),
                            &host.inner.gateway,
                            &renewal.token,
                            Method::POST,
                            &format!("/v1/leases/{}/renew", encode_segment(&renewal.id)),
                            Some(&json!({})),
                        )
                        .await;
                        if matches!(result, Err(Error::GatewayStatus { status: 404, .. })) {
                            host.inner.state.lock().await.leases.remove(&renewal.id);
                            let _ = tokio::fs::remove_dir_all(&renewal.dir).await;
                            return;
                        }
                        next = tokio::time::Instant::now() + Duration::from_secs(120);
                    }
                });
                Ok(id)
            }
            Err(error) => {
                let _ = self
                    .cli(&lease, vec!["close".into()], Duration::from_secs(120), None)
                    .await;
                let _ = tokio::fs::remove_dir_all(&lease.dir).await;
                Err(Error::boot(
                    self.id(),
                    device.id.as_ref(),
                    DeviceBootErrorReason::LaunchFailed,
                    Some(error.to_string()),
                ))
            }
        }
    }
    async fn ready(&self) -> Ready {
        let node = self
            .inner
            .state
            .lock()
            .await
            .cli
            .as_ref()
            .map(|cli| cli.node.clone())
            .unwrap_or_default();
        Ready {
            origin: self.inner.gateway.clone(),
            node_path: node,
            devices: Arc::new(self.clone()),
            commands: Arc::new(NoCommands("Simlock")),
            ax_helper: None,
            sim_cli: None,
            agent: None,
            routing: Some(Arc::new(self.clone())),
            release: None,
        }
    }
}
#[async_trait]
impl DeviceHost for SimlockHost {
    fn id(&self) -> &str {
        self.inner.options.config.id.as_ref()
    }
    fn label(&self) -> &str {
        self.inner.options.config.label.as_ref()
    }
    async fn summary(&self) -> DeviceHostSummary {
        let mut summary = host::summary(
            self.id(),
            self.label(),
            DeviceHostKind::Simlock,
            self.inner.state.lock().await.platforms.clone(),
        );
        summary.hub_installed = true;
        summary.agent_device_installed = true;
        summary
    }
    async fn availability(&self, platform: DevicePlatform) -> DevicePlatformAvailability {
        let _ = self.catalog().await;
        self.inner
            .state
            .lock()
            .await
            .platforms
            .iter()
            .find(|p| p.platform == platform)
            .cloned()
            .unwrap_or_else(|| host::available(platform, Some("Pool platform unavailable.".into())))
    }
    async fn ensure_ready(&self, phase: Phase) -> Result<Ready> {
        let _guard = self.inner.start_lock.lock().await;
        if self.inner.state.lock().await.ready {
            return Ok(self.ready().await);
        }
        let Some(node) = self.inner.options.node.resolve() else {
            return Err(Error::needs_node(
                self.id(),
                self.label(),
                "Local device support",
                "Device automation",
            ));
        };
        phase(DeviceHostStatus::Installing, None);
        let cli = if let Some(provider) = &self.inner.options.cli_provider {
            provider.ensure().await?
        } else {
            crate::host::AgentCli {
                node,
                entry: self.inner.options.agent_entry.clone(),
            }
        };
        let mut state = self.inner.state.lock().await;
        state.ready = true;
        state.cli = Some(cli);
        drop(state);
        Ok(self.ready().await)
    }
    async fn ensure_agent_ready(&self, phase: Phase) -> Result<Ready> {
        self.ensure_ready(phase).await
    }
    async fn current(&self) -> Option<Ready> {
        let ready = self.inner.state.lock().await.ready;
        if ready {
            Some(self.ready().await)
        } else {
            None
        }
    }
    async fn stop_agent(&self) -> Result<()> {
        Ok(())
    }
    async fn stop(&self) -> Result<()> {
        let starts = {
            let mut state = self.inner.state.lock().await;
            state.epoch += 1;
            state.starting.values().cloned().collect::<Vec<_>>()
        };
        for (cancel, _) in &starts {
            cancel.cancel();
        }
        for (_, mut done) in starts {
            while !*done.borrow() {
                if done.changed().await.is_err() {
                    break;
                }
            }
        }
        let leases = self
            .inner
            .state
            .lock()
            .await
            .leases
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for lease in leases {
            self.release(lease).await;
        }
        Ok(())
    }
}
#[async_trait]
impl DeviceOperations for SimlockHost {
    async fn list(&self) -> Result<Listing> {
        let mut devices = self.catalog().await?;
        devices.extend(
            self.inner
                .state
                .lock()
                .await
                .leases
                .values()
                .map(|l| l.device.clone()),
        );
        Ok(Listing {
            devices,
            ..Default::default()
        })
    }
    async fn boot(&self, device: &DeviceSummary, context: &OpenContext) -> Result<String> {
        let name = uuid::Uuid::new_v4().simple().to_string()[..6].to_owned();
        let (cancel, done, epoch) = {
            let mut state = self.inner.state.lock().await;
            if state.leases.contains_key(device.id.as_ref()) {
                return Ok(device.id.as_ref().into());
            }
            if state.leases.len() + state.starting.len()
                >= self.inner.options.config.max_leases as usize
            {
                return Err(self.unavailable(format!(
                    "{} is limited to {} {} at once. Close one in the Device panel first.",
                    self.label(),
                    self.inner.options.config.max_leases,
                    if self.inner.options.config.max_leases == 1 {
                        "device"
                    } else {
                        "devices"
                    }
                )));
            }
            let cancel = CancellationToken::new();
            let (tx, rx) = watch::channel(false);
            let epoch = state.epoch;
            state.starting.insert(name.clone(), (cancel.clone(), rx));
            (cancel, tx, epoch)
        };
        let host = self.clone();
        let device = device.clone();
        let context = context.clone();
        tokio::spawn(async move {
            let result = host
                .open_lease(device, context, name.clone(), epoch, cancel)
                .await;
            host.inner.state.lock().await.starting.remove(&name);
            let _ = done.send(true);
            result
        })
        .await
        .map_err(|_| Error::Cancelled("Simlock open"))?
    }
    async fn attach(&self, _: &DeviceSummary) -> Result<()> {
        Ok(())
    }
    async fn shutdown(&self, id: &str, _: DevicePlatform) -> Result<()> {
        let lease = self.inner.state.lock().await.leases.get(id).cloned();
        if let Some(lease) = lease {
            self.release(lease).await;
        }
        Ok(())
    }
    async fn screenshot(&self, device: &DeviceSummary) -> Result<Vec<u8>> {
        let lease = self
            .inner
            .state
            .lock()
            .await
            .leases
            .get(device.id.as_ref())
            .cloned()
            .ok_or_else(|| crate::error::invalid("Simlock screenshot"))?;
        let file = lease
            .dir
            .join(format!("screenshot-{}.png", uuid::Uuid::new_v4().simple()));
        let result = async {
            self.cli(
                &lease,
                vec!["screenshot".into(), file.to_string_lossy().into_owned()],
                Duration::from_secs(120),
                None,
            )
            .await?;
            tokio::fs::read(&file)
                .await
                .map_err(|_| crate::error::invalid("Simlock screenshot"))
        }
        .await;
        let _ = tokio::fs::remove_file(file).await;
        result
    }
    async fn agent_endpoint(&self, id: &str) -> Result<Option<AgentEndpoint>> {
        let state = self.inner.state.lock().await;
        let lease = state
            .leases
            .get(id)
            .ok_or_else(|| self.unavailable("This Simlock lease has ended."))?;
        Ok(Some(AgentEndpoint {
            base_url: format!("{}/v1/agent-device", self.inner.gateway),
            token: lease.token.clone(),
            entry_path: state
                .cli
                .as_ref()
                .map(|cli| cli.entry.clone())
                .unwrap_or_else(|| self.inner.options.agent_entry.clone()),
            agent_device_id: None,
            session: Some(lease.session.clone()),
            state_dir: Some(lease.dir.clone()),
            install_directory: Some(lease.install.clone()),
        }))
    }
    fn ends_devices(&self) -> bool {
        true
    }
}
#[async_trait]
impl Routing for SimlockHost {
    async fn upstream(&self, path: &str, query: &[(String, String)]) -> Result<Option<Upstream>> {
        let stream = path
            .strip_prefix("/vendor/serve-sim/helper/")
            .and_then(|s| s.strip_suffix("/stream.avcc"))
            .filter(|s| !s.contains('/'));
        let id = if let Some(segment) = stream {
            match decode_segment(segment) {
                Some(id) => id,
                None => return Ok(None),
            }
        } else if path == "/vendor/serve-sim/helper/ws" {
            match query.iter().find(|(k, _)| k == "device") {
                Some((_, id)) => id.clone(),
                None => return Ok(None),
            }
        } else {
            return Ok(None);
        };
        let state = self.inner.state.lock().await;
        let Some(lease) = state.leases.get(&id) else {
            return Ok(None);
        };
        Ok(Some(Upstream {
            url: format!(
                "{}/v1/leases/{}/{}",
                self.inner.gateway,
                encode_segment(&id),
                if stream.is_some() { "stream" } else { "input" }
            ),
            headers: BTreeMap::from([(
                "authorization".into(),
                Secret::new(format!("Bearer {}", lease.token.expose())),
            )]),
            protocols: vec![],
        }))
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        if let Ok(state) = self.state.try_lock() {
            for lease in state.leases.values() {
                lease.cancel.cancel();
            }
            for (cancel, _) in state.starting.values() {
                cancel.cancel();
            }
        }
    }
}

pub fn catalog_models(value: &Value) -> BTreeSet<String> {
    value["platforms"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|p| {
            p["modelRuntimes"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(name, _)| name.clone())
        })
        .collect()
}
