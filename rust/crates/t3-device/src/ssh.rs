use crate::{
    Error, Result,
    agent::{self, DaemonRelease},
    host::{self, AgentEndpoint, DeviceHost, Phase, Ready},
    hub::HubOperations,
    io::{Command, CommandOutput, Commands, Http, Secret, Spawner},
    local::{Ports, wait_ready},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{collections::BTreeSet, net::IpAddr, path::PathBuf, sync::Arc, time::Duration};
use t3_contracts::device::*;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
pub fn bootstrap_script(owner: &str, mode: &str) -> Result<String> {
    if owner.len() != 24
        || !owner.bytes().all(|b| b.is_ascii_hexdigit())
        || !["probe", "start", "agent-start", "stop-agent", "stop"].contains(&mode)
    {
        return Err(crate::error::invalid("SSH bootstrap"));
    }
    Ok(format!(
        "const owner={};const mode={};const hubVersion={};const agentVersion={};\n{}",
        json!(owner),
        json!(mode),
        json!(crate::tools::HUB_VERSION),
        json!(crate::tools::AGENT_VERSION),
        include_str!("remote_bootstrap.cjs")
    ))
}
pub fn base_args(config: &SshDeviceHostConfig) -> Result<Vec<String>> {
    crate::error::validate_selector(config.target.as_ref(), "SSH target")?;
    let mut args = vec![
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ConnectTimeout=10".into(),
    ];
    if let Some(Some(port)) = config.port {
        if port == 0 {
            return Err(crate::error::invalid("SSH port"));
        }
        args.extend(["-p".into(), port.to_string()]);
    }
    if let Some(Some(path)) = &config.identity_file {
        crate::error::validate_arg(path.as_ref(), "SSH identity file")?;
        args.extend(["-i".into(), path.as_ref().into()]);
    }
    Ok(args)
}

#[async_trait]
pub trait Dns: Send + Sync {
    async fn lookup(&self, hostname: &str) -> Result<Vec<IpAddr>>;
}
pub struct NativeDns;
#[async_trait]
impl Dns for NativeDns {
    async fn lookup(&self, hostname: &str) -> Result<Vec<IpAddr>> {
        tokio::time::timeout(
            Duration::from_secs(2),
            tokio::net::lookup_host((hostname, 22)),
        )
        .await
        .map_err(|_| crate::error::invalid("DNS lookup"))?
        .map(|v| v.map(|a| a.ip()).collect())
        .map_err(|_| crate::error::invalid("DNS lookup"))
    }
}
pub async fn is_local(
    config: &SshDeviceHostConfig,
    commands: &dyn Commands,
    dns: &dyn Dns,
    addresses: &BTreeSet<IpAddr>,
) -> bool {
    let Ok(mut args) = base_args(config) else {
        return false;
    };
    args.extend(["-G".into(), config.target.as_ref().into()]);
    let mut command = Command::new(if cfg!(windows) { "ssh.exe" } else { "ssh" }, args);
    command.timeout = Duration::from_secs(5);
    let Ok(output) = commands.run(command).await else {
        return false;
    };
    if output.code != 0 {
        return false;
    }
    let values: std::collections::BTreeMap<_, _> = output
        .stdout
        .lines()
        .filter_map(|l| l.split_once(' '))
        .map(|(k, v)| (k, v.trim()))
        .collect();
    if values.get("port") != Some(&"22")
        || ["proxycommand", "proxyjump"]
            .iter()
            .any(|k| values.get(k).is_some_and(|v| *v != "none"))
    {
        return false;
    }
    let Some(host) = values.get("hostname") else {
        return false;
    };
    let host = host.trim_matches(['[', ']']);
    let ips = if let Ok(ip) = host.parse() {
        vec![ip]
    } else {
        dns.lookup(host).await.unwrap_or_default()
    };
    !ips.is_empty()
        && ips.iter().all(|ip| {
            let ip = match ip {
                IpAddr::V6(v) => v.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(*ip),
                ip => *ip,
            };
            ip.is_loopback() || addresses.contains(&ip)
        })
}

#[derive(Clone)]
pub struct SshOptions {
    pub config: SshDeviceHostConfig,
    pub owner: String,
    pub commands: Arc<dyn Commands>,
    pub http: Arc<dyn Http>,
    pub spawner: Arc<dyn Spawner>,
    pub ports: Arc<dyn Ports>,
}
impl SshOptions {
    async fn bootstrap(&self, mode: &str) -> Result<Value> {
        let mut args = base_args(&self.config)?;
        let shell = format!(
            "{}command -v node >/dev/null 2>&1 || {{ echo \"Node is missing from the non-interactive SSH PATH\" >&2; exit 1; }}; exec node",
            include_str!("remote_environment.sh")
        );
        args.extend([
            self.config.target.as_ref().into(),
            "sh".into(),
            "-c".into(),
            quote(&shell),
        ]);
        let mut command = Command::new(if cfg!(windows) { "ssh.exe" } else { "ssh" }, args);
        command.stdin = Some(bootstrap_script(&self.owner, mode)?.into_bytes());
        command.timeout = Duration::from_secs(if ["start", "agent-start"].contains(&mode) {
            1300
        } else {
            45
        });
        let result = self.commands.run(command).await?;
        if result.code != 0 {
            let reason = if result.code == 1
                && result
                    .stderr
                    .lines()
                    .any(|l| l.trim() == "Node is missing from the non-interactive SSH PATH")
            {
                "Connected over SSH, but Node is not on this host's PATH for non-interactive SSH commands.".to_owned()
            } else if (1..=254).contains(&result.code) || result.code == 9009 {
                let mut args = base_args(&self.config)?;
                args.extend([
                    self.config.target.as_ref().into(),
                    "echo %OS% $env:OS".into(),
                ]);
                let mut cmd = Command::new("ssh", args);
                cmd.timeout = Duration::from_secs(15);
                let mut windows = self.commands.run(cmd).await.is_ok_and(|r| {
                    r.code == 0 && r.stdout.split_whitespace().any(|s| s == "Windows_NT")
                });
                if !windows {
                    let mut args = base_args(&self.config)?;
                    args.extend([
                        self.config.target.as_ref().into(),
                        "node".into(),
                        "-p".into(),
                        "process.platform".into(),
                    ]);
                    let mut cmd = Command::new("ssh", args);
                    cmd.timeout = Duration::from_secs(15);
                    windows = self
                        .commands
                        .run(cmd)
                        .await
                        .is_ok_and(|r| r.code == 0 && r.stdout.trim() == "win32");
                }
                if windows {
                    "Connected over SSH, but this host runs Windows, and SSH device hosts currently need macOS or Linux. To use this machine's devices, run T3 Code on it and connect to that environment.".into()
                } else {
                    format!(
                        "Connected over SSH, but the device host check failed (exit code {}).",
                        result.code
                    )
                }
            } else {
                "Could not connect to this host over SSH.".into()
            };
            return Err(Error::host(
                self.config.id.as_ref(),
                Some(self.config.label.as_ref()),
                reason,
            ));
        }
        if ["stop", "stop-agent"].contains(&mode) {
            return Ok(Value::Null);
        }
        serde_json::from_str(result.stdout.trim())
            .map_err(|_| crate::error::invalid("SSH host response"))
    }
    pub async fn probe(&self) -> Result<DeviceHostSummary> {
        let value = self.bootstrap("probe").await?;
        let platforms = serde_json::from_value(value["platforms"].clone())
            .map_err(|_| crate::error::invalid("SSH platforms"))?;
        let mut summary = host::summary(
            self.config.id.as_ref(),
            self.config.label.as_ref(),
            DeviceHostKind::Ssh,
            platforms,
        );
        if let Some(tools) = value.get("tools") {
            let tools: DeviceToolVersions = serde_json::from_value(tools.clone())
                .map_err(|_| crate::error::invalid("SSH tools"))?;
            summary.hub_installed = tools
                .hub
                .installed_versions
                .contains(&tools.hub.required_version);
            summary.agent_device_installed = tools
                .agent
                .installed_versions
                .contains(&tools.agent.required_version);
            summary.tools = Some(Some(tools));
        }
        Ok(summary)
    }
}
struct Connection {
    ready: Ready,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
    alive: Arc<std::sync::atomic::AtomicBool>,
}
pub struct SshDeviceHost {
    options: Arc<SshOptions>,
    connection: Arc<Mutex<Option<Connection>>>,
    summary: Arc<Mutex<DeviceHostSummary>>,
    activated: Arc<Mutex<bool>>,
    stopped: Arc<std::sync::atomic::AtomicBool>,
}
impl SshDeviceHost {
    pub fn new(options: SshOptions) -> Result<Self> {
        base_args(&options.config)?;
        let summary = host::summary(
            options.config.id.as_ref(),
            options.config.label.as_ref(),
            DeviceHostKind::Ssh,
            vec![],
        );
        Ok(Self {
            options: Arc::new(options),
            connection: Arc::new(Mutex::new(None)),
            summary: Arc::new(Mutex::new(summary)),
            activated: Arc::new(Mutex::new(false)),
            stopped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }
    fn connect(
        &self,
        phase: Phase,
        agent: bool,
    ) -> futures_util::future::BoxFuture<'_, Result<Ready>> {
        Box::pin(async move {
            let mut connection = self.connection.lock().await;
            if self.stopped.load(std::sync::atomic::Ordering::Acquire) {
                return Err(Error::host(self.id(), None, "Device host was stopped."));
            }
            if let Some(current) = connection.as_ref()
                && current.alive.load(std::sync::atomic::Ordering::Acquire)
                && (!agent || current.ready.agent.is_some())
            {
                return Ok(current.ready.clone());
            }
            if let Some(old) = connection.take() {
                old.cancel.cancel();
                let _ = old.task.await;
            }
            let summary = self.options.probe().await?;
            phase(
                if summary.hub_installed {
                    DeviceHostStatus::Starting
                } else {
                    DeviceHostStatus::Installing
                },
                None,
            );
            *self.summary.lock().await = summary;
            *self.activated.lock().await = true;
            let remote = self
                .options
                .bootstrap(if agent { "agent-start" } else { "start" })
                .await?;
            let mut last_error = None;
            for _ in 0..3 {
                let result = self.forward(&remote, phase.clone()).await;
                match result {
                    Ok(next) => {
                        let ready = next.ready.clone();
                        *connection = Some(next);
                        return Ok(ready);
                    }
                    Err(e) => last_error = Some(e),
                }
            }
            if agent {
                let _ = self.options.bootstrap("stop-agent").await;
            }
            Err(last_error.unwrap_or_else(|| crate::error::invalid("SSH forwarding")))
        })
    }
    async fn forward(&self, remote: &Value, phase: Phase) -> Result<Connection> {
        let port = |key: &str| {
            remote[key]
                .as_u64()
                .and_then(|n| u16::try_from(n).ok())
                .filter(|p| *p > 0)
                .ok_or_else(|| crate::error::invalid("SSH port"))
        };
        let hub_remote = port("hubPort")?;
        let hub_port = self.options.ports.reserve().await?;
        let daemon_remote = remote
            .get("daemonPort")
            .map(|_| port("daemonPort"))
            .transpose()?;
        let daemon_port = if daemon_remote.is_some() {
            Some(self.options.ports.reserve().await?)
        } else {
            None
        };
        let mut args = base_args(&self.options.config)?;
        for option in [
            "ExitOnForwardFailure=yes",
            "ControlMaster=no",
            "ControlPath=none",
            "ControlPersist=no",
            "ServerAliveInterval=10",
            "ServerAliveCountMax=3",
        ] {
            args.extend(["-o".into(), option.into()]);
        }
        args.extend([
            "-T".into(),
            "-L".into(),
            format!("127.0.0.1:{hub_port}:127.0.0.1:{hub_remote}"),
        ]);
        if let (Some(local), Some(remote)) = (daemon_port, daemon_remote) {
            args.extend(["-L".into(), format!("127.0.0.1:{local}:127.0.0.1:{remote}")]);
        }
        args.extend([
            self.options.config.target.as_ref().into(),
            "sh".into(),
            "-c".into(),
            quote("exec cat >/dev/null"),
        ]);
        let mut child = self
            .options
            .spawner
            .spawn(Command::new(
                if cfg!(windows) { "ssh.exe" } else { "ssh" },
                args,
            ))
            .await?;
        let origin = format!("http://127.0.0.1:{hub_port}");
        if let Err(e) = wait_ready(
            self.options.http.as_ref(),
            &origin,
            "/readyz",
            Duration::from_secs(15),
        )
        .await
        {
            let _ = child.stop().await;
            return Err(e);
        }
        let agent = if let Some(port) = daemon_port {
            let endpoint = AgentEndpoint {
                base_url: format!("http://127.0.0.1:{port}"),
                token: Secret::new(
                    remote["token"]
                        .as_str()
                        .ok_or_else(|| crate::error::invalid("SSH token"))?,
                ),
                entry_path: PathBuf::from(
                    remote["entryPath"]
                        .as_str()
                        .ok_or_else(|| crate::error::invalid("SSH entry"))?,
                ),
                agent_device_id: None,
                session: None,
                state_dir: None,
                install_directory: None,
            };
            if let Err(e) = wait_ready(
                self.options.http.as_ref(),
                &endpoint.base_url,
                "/health",
                Duration::from_secs(15),
            )
            .await
            {
                let _ = child.stop().await;
                return Err(e);
            }
            Some(endpoint)
        } else {
            None
        };
        let platforms: Vec<DevicePlatformAvailability> =
            serde_json::from_value(remote["platforms"].clone())
                .map_err(|_| crate::error::invalid("SSH platforms"))?;
        let commands: Arc<dyn Commands> = Arc::new(SshCommands {
            options: self.options.clone(),
        });
        let ready = Ready {
            origin: origin.clone(),
            node_path: remote["nodePath"]
                .as_str()
                .ok_or_else(|| crate::error::invalid("SSH Node path"))?
                .into(),
            devices: Arc::new(HubOperations {
                host_id: self.id().into(),
                origin: origin.clone(),
                http: self.options.http.clone(),
                commands: commands.clone(),
                platforms: platforms
                    .iter()
                    .filter(|p| p.available)
                    .map(|p| p.platform.clone())
                    .collect(),
            }),
            commands,
            ax_helper: remote
                .pointer("/helpers/serveSimAxSettings")
                .and_then(Value::as_str)
                .map(Into::into),
            sim_cli: remote
                .pointer("/helpers/serveSimCli")
                .and_then(Value::as_str)
                .map(Into::into),
            release: agent.as_ref().map(|e| {
                Arc::new(DaemonRelease {
                    http: self.options.http.clone(),
                    endpoint: e.clone(),
                }) as Arc<dyn crate::host::SessionRelease>
            }),
            agent,
            routing: None,
        };
        let mut summary = self.summary.lock().await;
        summary.hub_installed = true;
        summary.agent_device_installed |= ready.agent.is_some();
        summary.platforms = platforms;
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let flag = alive.clone();
        let http = self.options.http.clone();
        let health_origin = origin;
        let daemon_health = ready.agent.as_ref().map(|a| a.base_url.clone());
        let weak_connection = Arc::downgrade(&self.connection);
        let weak_summary = Arc::downgrade(&self.summary);
        let weak_activated = Arc::downgrade(&self.activated);
        let stopped = self.stopped.clone();
        let options = self.options.clone();
        let wants_agent = ready.agent.is_some();
        let task = tokio::spawn(async move {
            let monitor = async {
                let mut misses = 0;
                loop {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    let mut healthy = crate::local::probe_ready(
                        http.as_ref(),
                        &health_origin,
                        "/readyz",
                        Duration::from_secs(5),
                    )
                    .await;
                    if let Some(origin) = &daemon_health {
                        healthy &= crate::local::probe_ready(
                            http.as_ref(),
                            origin,
                            "/health",
                            Duration::from_secs(5),
                        )
                        .await;
                    }
                    misses = if healthy { 0 } else { misses + 1 };
                    if misses >= 3 {
                        break;
                    }
                }
            };
            tokio::select! {biased;_=stop.cancelled()=>(),_=child.exited()=>(),_=monitor=>()};
            let _ = child.stop().await;
            flag.store(false, std::sync::atomic::Ordering::Release);
            if stop.is_cancelled() {
                return;
            }
            phase(
                DeviceHostStatus::Starting,
                Some("Reconnecting to device host…".into()),
            );
            let mut delay = 1000;
            loop {
                tokio::select! {biased;_=stop.cancelled()=>return,_=tokio::time::sleep(Duration::from_millis(delay))=>()}
                let (Some(connection), Some(summary), Some(activated)) = (
                    weak_connection.upgrade(),
                    weak_summary.upgrade(),
                    weak_activated.upgrade(),
                ) else {
                    return;
                };
                if stopped.load(std::sync::atomic::Ordering::Acquire) {
                    return;
                }
                {
                    let mut current = connection.lock().await;
                    if current
                        .as_ref()
                        .is_some_and(|c| !Arc::ptr_eq(&c.alive, &flag))
                    {
                        return;
                    }
                    current.take();
                }
                let host = SshDeviceHost {
                    options: options.clone(),
                    connection,
                    summary,
                    activated,
                    stopped: stopped.clone(),
                };
                match host.connect(phase.clone(), wants_agent).await {
                    Ok(_) => return,
                    Err(error) => phase(DeviceHostStatus::Failed, Some(error.to_string())),
                }
                delay = (delay * 2).min(30000);
            }
        });
        Ok(Connection {
            ready,
            cancel,
            task,
            alive,
        })
    }
}
#[async_trait]
impl DeviceHost for SshDeviceHost {
    fn id(&self) -> &str {
        self.options.config.id.as_ref()
    }
    fn label(&self) -> &str {
        self.options.config.label.as_ref()
    }
    async fn summary(&self) -> DeviceHostSummary {
        self.summary.lock().await.clone()
    }
    async fn inspect(&self) -> Result<DeviceHostSummary> {
        let summary = self.options.probe().await?;
        *self.summary.lock().await = summary.clone();
        Ok(summary)
    }
    async fn availability(&self, platform: DevicePlatform) -> DevicePlatformAvailability {
        match self.options.probe().await {
            Ok(summary) => {
                let found = summary
                    .platforms
                    .iter()
                    .find(|p| p.platform == platform)
                    .cloned();
                self.summary.lock().await.platforms = summary.platforms;
                found.unwrap_or_else(|| {
                    host::available(platform, Some("Platform toolchain missing.".into()))
                })
            }
            Err(_) => host::available(
                platform,
                Some("Cannot reach device host. Test its SSH connection in Settings.".into()),
            ),
        }
    }
    async fn ensure_ready(&self, phase: Phase) -> Result<Ready> {
        self.stopped
            .store(false, std::sync::atomic::Ordering::Release);
        self.connect(phase, false).await
    }
    async fn ensure_agent_ready(&self, phase: Phase) -> Result<Ready> {
        self.stopped
            .store(false, std::sync::atomic::Ordering::Release);
        self.connect(phase, true).await
    }
    async fn current(&self) -> Option<Ready> {
        self.connection
            .lock()
            .await
            .as_ref()
            .filter(|c| c.alive.load(std::sync::atomic::Ordering::Acquire))
            .map(|c| c.ready.clone())
    }
    async fn stop_agent(&self) -> Result<()> {
        let previous = self.connection.lock().await.take();
        if let Some(previous) = previous {
            previous.cancel.cancel();
            let _ = previous.task.await;
            self.options.bootstrap("stop-agent").await?;
            let _ = self.connect(Arc::new(|_, _| {}), false).await?;
        }
        Ok(())
    }
    async fn stop(&self) -> Result<()> {
        self.stopped
            .store(true, std::sync::atomic::Ordering::Release);
        let previous = self.connection.lock().await.take();
        if let Some(previous) = previous {
            previous.cancel.cancel();
            let _ = previous.task.await;
        }
        if *self.activated.lock().await {
            self.options.bootstrap("stop").await?;
            *self.activated.lock().await = false;
        }
        Ok(())
    }
}
impl Drop for SshDeviceHost {
    fn drop(&mut self) {
        if Arc::strong_count(&self.connection) == 1
            && let Ok(c) = self.connection.try_lock()
            && let Some(c) = c.as_ref()
        {
            c.cancel.cancel();
        }
    }
}
struct SshCommands {
    options: Arc<SshOptions>,
}
#[async_trait]
impl Commands for SshCommands {
    async fn run(&self, command: Command) -> Result<CommandOutput> {
        let mut args = base_args(&self.options.config)?;
        let script = format!(
            "{}cd \"$HOME/.t3/device/hosts/{}\" && exec {}",
            include_str!("remote_environment.sh"),
            self.options.owner,
            std::iter::once(&command.program)
                .chain(command.args.iter())
                .map(|v| quote(v))
                .collect::<Vec<_>>()
                .join(" ")
        );
        args.extend([
            self.options.config.target.as_ref().into(),
            "sh".into(),
            "-c".into(),
            quote(&script),
        ]);
        let mut remote = Command::new(if cfg!(windows) { "ssh.exe" } else { "ssh" }, args);
        remote.stdin = command.stdin;
        remote.timeout = command.timeout;
        remote.max_bytes = command.max_bytes;
        let mut result = self.options.commands.run(remote).await?;
        if result.code != 0 {
            result.stdout = crate::error::bounded(&result.stdout, 4000);
            result.stderr = "Remote device command failed.".into();
        }
        Ok(result)
    }
}

pub fn owner(environment: &str, state: &str, host: &str) -> String {
    agent::hash(&format!("{environment}\0{state}\0{host}"))
}
