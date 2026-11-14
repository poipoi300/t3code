use crate::{
    Error, Result,
    agent::DaemonRelease,
    host::{self, AgentEndpoint, DeviceHost, Phase, Ready},
    hub::HubOperations,
    io::{Command, Commands, Http, HttpRequest, Secret, Spawner, request_bytes},
    tools::{Tool, Toolchain},
};
use async_trait::async_trait;
use http::Method;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use t3_contracts::device::*;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[async_trait]
pub trait Ports: Send + Sync {
    async fn reserve(&self) -> Result<u16>;
}
pub struct NativePorts;
#[async_trait]
impl Ports for NativePorts {
    async fn reserve(&self) -> Result<u16> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| Error::Unavailable("Could not reserve a device helper port.".into()))?;
        listener
            .local_addr()
            .map(|a| a.port())
            .map_err(|_| crate::error::invalid("port"))
    }
}

#[async_trait]
pub trait Machine: Send + Sync {
    fn platform(&self) -> &str;
    fn environment(&self) -> BTreeMap<String, String>;
    async fn exists(&self, path: &Path) -> bool;
    async fn real_path(&self, path: &Path) -> Option<PathBuf>;
    fn user_id(&self) -> Option<u32> {
        None
    }
    async fn owned_directory(&self, _path: &Path, _uid: u32) -> bool {
        false
    }
}
pub struct NativeMachine;
#[async_trait]
impl Machine for NativeMachine {
    fn user_id(&self) -> Option<u32> {
        #[cfg(unix)]
        {
            Some(rustix::process::getuid().as_raw())
        }
        #[cfg(not(unix))]
        {
            None
        }
    }
    async fn owned_directory(&self, path: &Path, uid: u32) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            tokio::fs::metadata(path)
                .await
                .is_ok_and(|m| m.is_dir() && m.uid() == uid)
        }
        #[cfg(not(unix))]
        {
            let _ = (path, uid);
            false
        }
    }
    fn platform(&self) -> &str {
        if cfg!(windows) {
            "win32"
        } else if cfg!(target_os = "macos") {
            "darwin"
        } else {
            "linux"
        }
    }
    fn environment(&self) -> BTreeMap<String, String> {
        std::env::vars().collect()
    }
    async fn exists(&self, path: &Path) -> bool {
        tokio::fs::try_exists(path).await.unwrap_or(false)
    }
    async fn real_path(&self, path: &Path) -> Option<PathBuf> {
        tokio::fs::canonicalize(path).await.ok()
    }
}

#[derive(Clone, Debug, Default)]
pub struct AndroidSdk {
    pub root: Option<PathBuf>,
    pub adb: bool,
    pub emulator: bool,
    pub avdmanager: bool,
    pub legacy_avdmanager: bool,
}
pub async fn android_sdk(machine: &dyn Machine) -> AndroidSdk {
    let env = machine.environment();
    let windows = machine.platform() == "win32";
    let home = env
        .get("HOME")
        .or(env.get("USERPROFILE"))
        .map(String::as_str)
        .unwrap_or("");
    let explicit = env
        .get("ANDROID_HOME")
        .or(env.get("ANDROID_SDK_ROOT"))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let inspect = |root: PathBuf| async move {
        let adb = machine
            .exists(&root.join(if windows {
                "platform-tools/adb.exe"
            } else {
                "platform-tools/adb"
            }))
            .await;
        let emulator = machine
            .exists(&root.join(if windows {
                "emulator/emulator.exe"
            } else {
                "emulator/emulator"
            }))
            .await;
        let avdmanager = machine
            .exists(&root.join(if windows {
                "cmdline-tools/latest/bin/avdmanager.bat"
            } else {
                "cmdline-tools/latest/bin/avdmanager"
            }))
            .await;
        let legacy_avdmanager = !avdmanager
            && machine
                .exists(&root.join(if windows {
                    "tools/bin/avdmanager.bat"
                } else {
                    "tools/bin/avdmanager"
                }))
                .await;
        AndroidSdk {
            root: Some(root),
            adb,
            emulator,
            avdmanager,
            legacy_avdmanager,
        }
    };
    if let Some(root) = explicit {
        return inspect(root.into()).await;
    }
    let candidates = [
        Path::new(home).join("Library/Android/sdk"),
        Path::new(home).join("Android/Sdk"),
        env.get("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(home).join("AppData/Local"))
            .join("Android/Sdk"),
    ];
    for candidate in candidates {
        let sdk = inspect(candidate).await;
        if sdk.adb || sdk.emulator {
            return sdk;
        }
    }
    let path = env
        .get("PATH")
        .or(env.get("Path"))
        .map(String::as_str)
        .unwrap_or("");
    for directory in path.split(if windows { ';' } else { ':' }) {
        if directory.is_empty() {
            continue;
        }
        let adb = Path::new(directory).join(if windows { "adb.exe" } else { "adb" });
        if !machine.exists(&adb).await {
            continue;
        }
        let resolved = machine.real_path(&adb).await.unwrap_or(adb);
        if let Some(root) = resolved.parent().and_then(Path::parent) {
            let sdk = inspect(root.into()).await;
            if sdk.adb || sdk.emulator {
                return sdk;
            }
        }
    }
    AndroidSdk::default()
}
pub async fn platform_reason(
    machine: &dyn Machine,
    commands: &dyn Commands,
    platform: DevicePlatform,
) -> Option<String> {
    if platform == DevicePlatform::Ios {
        if machine.platform() != "darwin" {
            return Some("iOS Simulators need macOS with Xcode.".into());
        }
        return match commands
            .run(Command::new("xcrun", ["--find", "simctl"]))
            .await
        {
            Ok(out) if out.code == 0 => None,
            _ => Some("Xcode command line tools were not found.".into()),
        };
    }
    let sdk = android_sdk(machine).await;
    let Some(root) = sdk.root else {
        return Some("Android SDK was not found. Install it with Android Studio or set ANDROID_HOME to your SDK directory.".into());
    };
    let root = root.display();
    if !sdk.adb {
        return Some(format!(
            "Android SDK Platform-Tools are missing from {root}. Install them in Android Studio's SDK Manager."
        ));
    }
    if !sdk.emulator {
        return Some(format!(
            "Android Emulator is missing from {root}. Install it in Android Studio's SDK Manager."
        ));
    }
    if !sdk.avdmanager {
        return Some(if sdk.legacy_avdmanager {
            format!(
                "The Android SDK command-line tools in {root} appear to be an older, unsupported version. Install Android SDK Command-line Tools (latest) in Android Studio's SDK Manager under SDK Tools."
            )
        } else {
            format!(
                "Android SDK Command-line Tools (latest) are missing from {root}. Install them in Android Studio's SDK Manager."
            )
        });
    }
    None
}

pub struct LocalOptions {
    pub state: PathBuf,
    pub cwd: PathBuf,
    pub node: String,
    pub tools: Arc<Toolchain>,
    pub http: Arc<dyn Http>,
    pub commands: Arc<dyn Commands>,
    pub spawner: Arc<dyn Spawner>,
    pub ports: Arc<dyn Ports>,
    pub machine: Arc<dyn Machine>,
}
struct Running {
    ready: Ready,
    alive: Arc<AtomicBool>,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}
pub struct LocalDeviceHost {
    options: Arc<LocalOptions>,
    running: Mutex<Option<Running>>,
}
impl LocalDeviceHost {
    pub fn new(options: LocalOptions) -> Self {
        Self {
            options: Arc::new(options),
            running: Mutex::new(None),
        }
    }
    async fn ensure(&self, phase: Phase, agent: bool) -> Result<Ready> {
        let mut running = self.running.lock().await;
        let mut ready = if let Some(active) =
            running.as_ref().filter(|r| r.alive.load(Ordering::Acquire))
        {
            active.ready.clone()
        } else {
            if let Some(old) = running.as_ref() {
                old.cancel.cancel();
            }
            if self.options.node.is_empty() {
                return Err(Error::host(
                    LOCAL_DEVICE_HOST_ID,
                    Some(LOCAL_DEVICE_HOST_LABEL),
                    "Local device support requires Node.js. Install Node.js and restart T3 Code.",
                ));
            }
            if !self.options.tools.installed(Tool::Hub).await {
                phase(DeviceHostStatus::Installing, None);
            }
            let tool = self.options.tools.ensure(Tool::Hub).await?;
            phase(DeviceHostStatus::Starting, None);
            let port = self.options.ports.reserve().await?;
            let origin = format!("http://127.0.0.1:{port}");
            let mut args = vec![
                tool.entry.to_string_lossy().into_owned(),
                "--port".into(),
                port.to_string(),
                "--host".into(),
                "127.0.0.1".into(),
                "--hide-sidebar".into(),
                "--hide-boot-device".into(),
            ];
            if self.options.machine.platform() != "darwin" {
                args.extend(["--platform".into(), "android".into()]);
            }
            let mut command = Command::new(&self.options.node, args);
            command.cwd = Some(self.options.cwd.clone());
            let sdk = android_sdk(self.options.machine.as_ref()).await;
            let environment = device_environment(
                self.options.machine.as_ref(),
                &sdk,
                self.options.commands.as_ref(),
            )
            .await;
            command.env = environment
                .iter()
                .map(|(k, v)| (k.clone(), Some(Secret::new(v))))
                .collect();
            let mut child = self.options.spawner.spawn(command.clone()).await?;
            if let Err(error) = wait_ready(
                self.options.http.as_ref(),
                &origin,
                "/readyz",
                Duration::from_secs(30),
            )
            .await
            {
                let _ = child.stop().await;
                return Err(error);
            }
            let platforms = self
                .platforms()
                .await
                .into_iter()
                .filter(|p| p.available)
                .map(|p| p.platform)
                .collect();
            let commands: Arc<dyn Commands> = Arc::new(LocalCommands {
                inner: self.options.commands.clone(),
                environment,
                sdk: sdk.clone(),
                cwd: self.options.cwd.clone(),
                windows: self.options.machine.platform() == "win32",
            });
            let vendor = tool
                .install
                .join("node_modules/expo-device-hub/vendor/serve-sim/dist");
            let ax = vendor.join("simax/serve-sim-ax-settings");
            let cli = vendor.join("serve-sim.js");
            let ready = Ready {
                origin: origin.clone(),
                node_path: self.options.node.clone(),
                devices: Arc::new(HubOperations {
                    host_id: LOCAL_DEVICE_HOST_ID.into(),
                    origin: origin.clone(),
                    http: self.options.http.clone(),
                    commands: commands.clone(),
                    platforms,
                }),
                commands,
                ax_helper: self
                    .options
                    .machine
                    .exists(&ax)
                    .await
                    .then(|| ax.to_string_lossy().into_owned()),
                sim_cli: self
                    .options
                    .machine
                    .exists(&cli)
                    .await
                    .then(|| cli.to_string_lossy().into_owned()),
                agent: running.as_ref().and_then(|r| r.ready.agent.clone()),
                routing: None,
                release: running.as_ref().and_then(|r| r.ready.release.clone()),
            };
            let cancel = CancellationToken::new();
            let stop = cancel.clone();
            let alive = Arc::new(AtomicBool::new(true));
            let flag = alive.clone();
            let options = self.options.clone();
            let task = tokio::spawn(async move {
                let mut delay = 0;
                let mut started = tokio::time::Instant::now();
                loop {
                    tokio::select! {biased;_=stop.cancelled()=>{let _=child.stop().await;break;},_=child.exited()=>{}}
                    flag.store(false, Ordering::Release);
                    if started.elapsed() >= Duration::from_secs(60) {
                        delay = 0;
                    }
                    tokio::select! {biased;_=stop.cancelled()=>break, _=tokio::time::sleep(Duration::from_millis(delay))=>{}}
                    delay = if delay == 0 {
                        1000
                    } else {
                        (delay * 2).min(30000)
                    };
                    match options.spawner.spawn(command.clone()).await {
                        Ok(mut replacement) => {
                            let result = tokio::select! {biased;_=stop.cancelled()=>{let _=replacement.stop().await;break;},r=wait_ready(options.http.as_ref(),&origin,"/readyz",Duration::from_secs(30))=>r};
                            if result.is_err() {
                                let _ = replacement.stop().await;
                                break;
                            }
                            child = replacement;
                            started = tokio::time::Instant::now();
                            flag.store(true, Ordering::Release);
                        }
                        Err(_) => break,
                    }
                }
                flag.store(false, Ordering::Release);
            });
            *running = Some(Running {
                ready: ready.clone(),
                alive,
                cancel,
                task,
            });
            let _ = self
                .options
                .tools
                .prune(
                    Tool::Hub,
                    &crate::tools::NativeProcessInventory {
                        commands: self.options.commands.clone(),
                    },
                )
                .await;
            ready
        };
        if agent {
            if !self.options.tools.installed(Tool::Agent).await {
                phase(DeviceHostStatus::Installing, None);
            }
            let tool = self.options.tools.ensure(Tool::Agent).await?;
            phase(DeviceHostStatus::Starting, None);
            let endpoint = self.daemon(&tool.entry).await?;
            let _ = self
                .options
                .tools
                .prune(
                    Tool::Agent,
                    &crate::tools::NativeProcessInventory {
                        commands: self.options.commands.clone(),
                    },
                )
                .await;
            ready.release = Some(Arc::new(DaemonRelease {
                http: self.options.http.clone(),
                endpoint: endpoint.clone(),
            }));
            ready.agent = Some(endpoint);
            if let Some(active) = running.as_mut() {
                active.ready = ready.clone();
            }
        }
        Ok(ready)
    }
    async fn daemon(&self, entry: &Path) -> Result<AgentEndpoint> {
        let state = self.options.state.join("device/agent-device");
        let file = state.join("daemon.json");
        if let Some(endpoint) = read_daemon(&file, entry).await
            && probe_ready(
                self.options.http.as_ref(),
                &endpoint.base_url,
                "/health",
                Duration::from_secs(2),
            )
            .await
        {
            return Ok(endpoint);
        }
        let _ = tokio::fs::remove_file(&file).await;
        tokio::fs::create_dir_all(&state)
            .await
            .map_err(|_| crate::error::invalid("daemon directory"))?;
        let mut command = Command::new(
            &self.options.node,
            [
                entry.to_string_lossy().into_owned(),
                "devices".into(),
                "--json".into(),
            ],
        );
        command.timeout = Duration::from_secs(30);
        command.env = agent_environment(&state);
        let result = self.options.commands.run(command).await;
        if let Some(endpoint) = read_daemon(&file, entry).await {
            return Ok(endpoint);
        }
        let result = result?;
        if result.code != 0 {
            let reason = serde_json::from_str::<Value>(&result.stdout).ok();
            let detail = match reason
                .as_ref()
                .and_then(|v| v.pointer("/error/details/reason"))
                .and_then(Value::as_str)
            {
                Some("daemon_registration_unproven") => {
                    if reason
                        .as_ref()
                        .and_then(|v| v.pointer("/error/details/inspection/reason"))
                        .and_then(Value::as_str)
                        == Some("non-directory")
                    {
                        "Daemon registration ownership could not be verified: daemon.lock is not a directory."
                    } else {
                        "Daemon registration ownership could not be verified."
                    }
                }
                Some("daemon_retirement_unconfirmed") => {
                    "Daemon replacement could not be confirmed."
                }
                _ => "The AgentDevice startup command failed.",
            };
            return Err(Error::host(
                LOCAL_DEVICE_HOST_ID,
                None,
                format!(
                    "Agent tools failed to start (exit code {}). {detail} Inspect daemon.lock and daemon diagnostics in {}. Stop older clients and daemons with their original CLI before manually repairing retained state.",
                    result.code,
                    state.display()
                ),
            ));
        }
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Some(endpoint) = read_daemon(&file, entry).await {
                    return endpoint;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| {
            Error::host(
                LOCAL_DEVICE_HOST_ID,
                None,
                "Agent tools did not start within 30000 ms.",
            )
        })
    }
    async fn platforms(&self) -> Vec<DevicePlatformAvailability> {
        let (ios, android) = tokio::join!(
            self.availability(DevicePlatform::Ios),
            self.availability(DevicePlatform::Android)
        );
        vec![ios, android]
    }
    async fn stop_daemon(&self) -> Result<()> {
        let state = self.options.state.join("device/agent-device");
        let file = state.join("daemon.json");
        let Some(record) = tokio::fs::read(&file)
            .await
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        else {
            return Ok(());
        };
        if let Some(pid) = record["pid"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|p| *p > 0)
        {
            use crate::tools::ProcessInventory;
            if !(crate::tools::NativeProcessInventory {
                commands: self.options.commands.clone(),
            })
            .alive(pid)
            .await
            {
                tokio::fs::remove_file(&file)
                    .await
                    .map_err(|_| crate::error::invalid("daemon record"))?;
                return Ok(());
            }
        }
        let version = record["version"]
            .as_str()
            .filter(|v| crate::tools::version_valid(v))
            .unwrap_or(crate::tools::AGENT_VERSION);
        let mut entry = Tool::Agent.paths(&self.options.tools.base, version).entry;
        if !self.options.machine.exists(&entry).await {
            entry = Tool::Agent
                .paths(&self.options.tools.base, crate::tools::AGENT_VERSION)
                .entry;
        }
        if !self.options.machine.exists(&entry).await {
            return Ok(());
        }
        let mut command = Command::new(
            &self.options.node,
            [
                entry.to_string_lossy().into_owned(),
                "daemon".into(),
                "stop".into(),
                "--state-dir".into(),
                state.to_string_lossy().into_owned(),
            ],
        );
        command.timeout = Duration::from_secs(10);
        command.env = agent_environment(&state);
        let result = self.options.commands.run(command).await?;
        if result.code != 0 {
            return Err(Error::command("stop daemon", result.code));
        }
        Ok(())
    }
}
#[async_trait]
impl DeviceHost for LocalDeviceHost {
    fn id(&self) -> &str {
        LOCAL_DEVICE_HOST_ID
    }
    fn label(&self) -> &str {
        LOCAL_DEVICE_HOST_LABEL
    }
    async fn summary(&self) -> DeviceHostSummary {
        let mut summary = host::summary(
            self.id(),
            self.label(),
            DeviceHostKind::Local,
            self.platforms().await,
        );
        summary.hub_installed = self.options.tools.installed(Tool::Hub).await;
        summary.agent_device_installed = self.options.tools.installed(Tool::Agent).await;
        let running = self.running.lock().await;
        let hub = running
            .as_ref()
            .filter(|r| r.alive.load(Ordering::Acquire))
            .map(|_| crate::tools::HUB_VERSION.into());
        let agent = running
            .as_ref()
            .and_then(|r| r.ready.agent.as_ref())
            .map(|_| crate::tools::AGENT_VERSION.into());
        summary.tools = self.options.tools.inventory(hub, agent).await.map(Some);
        summary
    }
    async fn availability(&self, platform: DevicePlatform) -> DevicePlatformAvailability {
        host::available(
            platform.clone(),
            platform_reason(
                self.options.machine.as_ref(),
                self.options.commands.as_ref(),
                platform,
            )
            .await,
        )
    }
    async fn ensure_ready(&self, phase: Phase) -> Result<Ready> {
        self.ensure(phase, false).await
    }
    async fn ensure_agent_ready(&self, phase: Phase) -> Result<Ready> {
        self.ensure(phase, true).await
    }
    async fn current(&self) -> Option<Ready> {
        self.running.lock().await.as_ref().map(|r| r.ready.clone())
    }
    async fn stop_agent(&self) -> Result<()> {
        self.stop_daemon().await?;
        if let Some(running) = self.running.lock().await.as_mut() {
            running.ready.agent = None;
            running.ready.release = None;
        }
        Ok(())
    }
    async fn stop(&self) -> Result<()> {
        let running = self.running.lock().await.take();
        if let Some(running) = running {
            running.cancel.cancel();
            let _ = running.task.await;
        }
        self.stop_daemon().await
    }
}
impl Drop for LocalDeviceHost {
    fn drop(&mut self) {
        if let Ok(running) = self.running.try_lock()
            && let Some(running) = running.as_ref()
        {
            running.cancel.cancel();
        }
    }
}

async fn read_daemon(path: &Path, entry: &Path) -> Option<AgentEndpoint> {
    let value: Value = serde_json::from_slice(&tokio::fs::read(path).await.ok()?).ok()?;
    let port = u16::try_from(value["httpPort"].as_u64()?)
        .ok()
        .filter(|v| *v > 0)?;
    let token = value["token"].as_str()?.to_owned();
    Some(AgentEndpoint {
        base_url: format!("http://127.0.0.1:{port}"),
        token: Secret::new(token),
        entry_path: entry.into(),
        agent_device_id: None,
        session: None,
        state_dir: None,
        install_directory: None,
    })
}
pub async fn wait_ready(
    http: &dyn Http,
    origin: &str,
    path: &str,
    timeout: Duration,
) -> Result<()> {
    tokio::time::timeout(timeout, async {
        loop {
            if let Ok(request) = HttpRequest::new(
                Method::GET,
                &format!("{origin}{path}"),
                Duration::from_secs(2),
            ) && request_bytes(http, request)
                .await
                .is_ok_and(|(status, _)| status == 200)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| Error::Unavailable("Device helper did not become ready.".into()))
}
fn agent_environment(state: &Path) -> BTreeMap<String, Option<Secret>> {
    let mut env = BTreeMap::new();
    for (key, value) in [
        ("AGENT_DEVICE_STATE_DIR", state.to_string_lossy().as_ref()),
        ("AGENT_DEVICE_DAEMON_SERVER_MODE", "http"),
        ("AGENT_DEVICE_DAEMON_IDLE_TIMEOUT_MS", "0"),
        ("AGENT_DEVICE_NO_UPDATE_NOTIFIER", "1"),
        ("ELECTRON_RUN_AS_NODE", "1"),
        ("FORCE_COLOR", "0"),
        ("NO_COLOR", "1"),
    ] {
        env.insert(key.into(), Some(Secret::new(value)));
    }
    for key in [
        "AGENT_DEVICE_DAEMON_BASE_URL",
        "AGENT_DEVICE_DAEMON_AUTH_TOKEN",
        "AGENT_DEVICE_CONFIG",
    ] {
        env.insert(key.into(), None);
    }
    env
}
async fn device_environment(
    machine: &dyn Machine,
    sdk: &AndroidSdk,
    commands: &dyn Commands,
) -> BTreeMap<String, String> {
    let mut env = machine.environment();
    env.insert("FORCE_COLOR".into(), "0".into());
    env.insert("NO_COLOR".into(), "1".into());
    if machine.platform() == "linux"
        && !env.contains_key("XDG_RUNTIME_DIR")
        && let Some(uid) = machine.user_id()
    {
        let dir = PathBuf::from(format!("/run/user/{uid}"));
        if machine.owned_directory(&dir, uid).await {
            env.insert("XDG_RUNTIME_DIR".into(), dir.to_string_lossy().into_owned());
        }
    }
    if let Some(root) = &sdk.root {
        env.insert("ANDROID_HOME".into(), root.to_string_lossy().into_owned());
        let path_key = if machine.platform() == "win32" {
            env.keys()
                .find(|key| key.eq_ignore_ascii_case("path"))
                .cloned()
                .unwrap_or_else(|| "PATH".into())
        } else {
            "PATH".into()
        };
        let path = env.get(&path_key).cloned().unwrap_or_default();
        if machine.platform() == "win32" {
            env.retain(|key, _| !key.eq_ignore_ascii_case("path") || *key == path_key);
        }
        env.insert(
            path_key,
            format!(
                "{}{}{}{}{}",
                root.join("platform-tools").display(),
                if machine.platform() == "win32" {
                    ";"
                } else {
                    ":"
                },
                root.join("emulator").display(),
                if machine.platform() == "win32" {
                    ";"
                } else {
                    ":"
                },
                path
            ),
        );
        let mut overrides = vec![];
        for facing in ["front", "back"] {
            let mut cmd = Command::new(
                root.join(if machine.platform() == "win32" {
                    "emulator/emulator.exe"
                } else {
                    "emulator/emulator"
                })
                .to_string_lossy(),
                [format!("-help-camera-{facing}")],
            );
            cmd.timeout = Duration::from_secs(5);
            if let Ok(out) = commands.run(cmd).await {
                let text = format!("{}\n{}", out.stdout, out.stderr);
                if out.code == 0
                    && text
                        .split(|c: char| !c.is_alphanumeric())
                        .any(|s| s == "emulated")
                    && !text
                        .split(|c: char| !c.is_alphanumeric())
                        .any(|s| s == "imagefile")
                {
                    overrides.push(format!("-camera-{facing} emulated"));
                }
            }
        }
        if !overrides.is_empty() {
            if let Some(existing) = env.get("EXPO_DEVICE_HUB_EMULATOR_EXTRA_ARGS") {
                overrides.push(existing.clone());
            }
            env.insert(
                "EXPO_DEVICE_HUB_EMULATOR_EXTRA_ARGS".into(),
                overrides.join(" "),
            );
        }
    }
    env
}
struct LocalCommands {
    inner: Arc<dyn Commands>,
    environment: BTreeMap<String, String>,
    sdk: AndroidSdk,
    cwd: PathBuf,
    windows: bool,
}
#[async_trait]
impl Commands for LocalCommands {
    async fn run(&self, mut command: Command) -> Result<crate::io::CommandOutput> {
        if command.program == "emulator"
            && let Some(root) = &self.sdk.root
        {
            command.program = root
                .join(if self.windows {
                    "emulator/emulator.exe"
                } else {
                    "emulator/emulator"
                })
                .to_string_lossy()
                .into_owned();
        }
        command.cwd.get_or_insert_with(|| self.cwd.clone());
        for (k, v) in &self.environment {
            command
                .env
                .entry(k.clone())
                .or_insert_with(|| Some(Secret::new(v)));
        }
        self.inner.run(command).await
    }
}

pub async fn probe_ready(http: &dyn Http, origin: &str, path: &str, timeout: Duration) -> bool {
    let Ok(request) = HttpRequest::new(Method::GET, &format!("{origin}{path}"), timeout) else {
        return false;
    };
    request_bytes(http, request)
        .await
        .is_ok_and(|(status, _)| status == 200)
}
