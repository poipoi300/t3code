use crate::{
    Error, Result,
    io::{Command, Commands},
    service::ToolInstaller,
};
use async_trait::async_trait;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_contracts::device::{DeviceToolVersion, DeviceToolVersions, DeviceUpdateTool};
use tokio::sync::Mutex;

#[async_trait]
pub trait ProcessInventory: Send + Sync {
    async fn command_lines(&self) -> Option<String>;
    async fn alive(&self, pid: u32) -> bool;
}
pub struct NativeProcessInventory {
    pub commands: Arc<dyn Commands>,
}
#[async_trait]
impl ProcessInventory for NativeProcessInventory {
    async fn command_lines(&self) -> Option<String> {
        let mut command = if cfg!(windows) {
            Command::new(
                "powershell.exe",
                [
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "Get-CimInstance Win32_Process | Select-Object -ExpandProperty CommandLine",
                ],
            )
        } else {
            Command::new("ps", ["-ax", "-o", "command="])
        };
        command.timeout = Duration::from_secs(10);
        self.commands
            .run(command)
            .await
            .ok()
            .filter(|r| r.code == 0 && !r.stdout.is_empty())
            .map(|r| r.stdout)
    }
    async fn alive(&self, pid: u32) -> bool {
        let mut command = if cfg!(windows) {
            Command::new(
                "tasklist",
                [
                    "/FI".into(),
                    format!("PID eq {pid}"),
                    "/FO".into(),
                    "CSV".into(),
                    "/NH".into(),
                ],
            )
        } else {
            Command::new("kill", ["-0".into(), pid.to_string()])
        };
        command.timeout = Duration::from_secs(10);
        match self.commands.run(command).await {
            Ok(result) => {
                if cfg!(windows) {
                    result.code != 0
                        || result
                            .stdout
                            .split(',')
                            .nth(1)
                            .is_some_and(|p| p.trim_matches('"') == pid.to_string())
                } else {
                    result.code == 0
                }
            }
            Err(_) => true,
        }
    }
}

struct MaintenanceLock {
    path: PathBuf,
    owner: String,
}
impl MaintenanceLock {
    async fn release(mut self) -> Result<()> {
        match tokio::fs::remove_file(self.path.join(&self.owner)).await {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(crate::error::invalid("maintenance unlock")),
        }
        match tokio::fs::remove_dir(&self.path).await {
            Ok(()) => (),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound
                        | std::io::ErrorKind::DirectoryNotEmpty
                        | std::io::ErrorKind::AlreadyExists
                        | std::io::ErrorKind::PermissionDenied
                ) => {}
            Err(_) => return Err(crate::error::invalid("maintenance unlock")),
        }
        self.owner.clear();
        Ok(())
    }
}
impl Drop for MaintenanceLock {
    fn drop(&mut self) {
        if self.owner.is_empty() {
            return;
        }
        let path = self.path.clone();
        let owner = self.owner.clone();
        let cleanup = move || {
            let _ = std::fs::remove_file(path.join(owner));
            let _ = std::fs::remove_dir(path);
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            drop(runtime.spawn_blocking(cleanup));
        } else {
            let _ = std::thread::Builder::new()
                .name("device-maintenance-cleanup".into())
                .spawn(cleanup);
        }
    }
}
async fn maintenance_lock(
    root: &Path,
    inventory: &dyn ProcessInventory,
) -> Result<MaintenanceLock> {
    tokio::fs::create_dir_all(root)
        .await
        .map_err(|_| crate::error::invalid("maintenance directory"))?;
    let path = root.join(".maintenance-lock");
    let nonce = uuid::Uuid::new_v4();
    let owner = format!("{}.{nonce}.json", std::process::id());
    let candidate = tempfile::Builder::new()
        .prefix(".maintenance-lock.")
        .tempdir_in(root)
        .map_err(|_| crate::error::invalid("maintenance candidate"))?;
    tokio::fs::write(
        candidate.path().join(&owner),
        serde_json::json!({"pid":std::process::id()}).to_string(),
    )
    .await
    .map_err(|_| crate::error::invalid("maintenance owner"))?;
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if tokio::fs::rename(candidate.path(), &path).await.is_ok() {
                return Ok(MaintenanceLock { path, owner });
            }
            if let Ok(mut entries) = tokio::fs::read_dir(&path).await {
                let first = entries
                    .next_entry()
                    .await
                    .map_err(|_| crate::error::invalid("maintenance lock"))?;
                if entries
                    .next_entry()
                    .await
                    .map_err(|_| crate::error::invalid("maintenance lock"))?
                    .is_none()
                    && let Some(entry) = first
                    && let Ok(bytes) = tokio::fs::read(entry.path()).await
                    && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
                    && let Some(pid) = value["pid"]
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .filter(|p| *p > 0)
                    && !inventory.alive(pid).await
                {
                    let _ = tokio::fs::remove_file(entry.path()).await;
                }
            }
            let _ = tokio::fs::remove_dir(&path).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| {
        Error::Unavailable(
            "Device tool maintenance is locked. Retry when the other operation finishes.".into(),
        )
    })?
}

pub const HUB_VERSION: &str = "0.22.1";
pub const AGENT_VERSION: &str = "0.21.36";
#[derive(Clone, Copy)]
pub enum Tool {
    Hub,
    Agent,
}
impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Self::Hub => "expo-device-hub",
            Self::Agent => "agent-device",
        }
    }
    pub fn version(self) -> &'static str {
        match self {
            Self::Hub => HUB_VERSION,
            Self::Agent => AGENT_VERSION,
        }
    }
    pub fn entry(self) -> &'static str {
        match self {
            Self::Hub => "dist/server/cli.mjs",
            Self::Agent => "bin/agent-device.mjs",
        }
    }
    pub fn paths(self, base: &Path, version: &str) -> ToolPaths {
        let install = base.join("tools").join(self.name()).join(version);
        ToolPaths {
            entry: install
                .join("node_modules")
                .join(self.name())
                .join(self.entry()),
            sentinel: install.join(".install-complete"),
            install,
        }
    }
}
#[derive(Clone)]
pub struct ToolPaths {
    pub install: PathBuf,
    pub entry: PathBuf,
    pub sentinel: PathBuf,
}
pub struct Toolchain {
    pub base: PathBuf,
    pub commands: Arc<dyn Commands>,
    lock: Mutex<()>,
}
pub struct ToolchainAgentCli {
    pub node: crate::io::NodeRuntime,
    pub tools: Arc<Toolchain>,
}
#[async_trait]
impl crate::host::AgentCliProvider for ToolchainAgentCli {
    async fn ensure(&self) -> Result<crate::host::AgentCli> {
        let Some(node) = self.node.resolve() else {
            return Err(Error::needs_node(
                t3_contracts::device::LOCAL_DEVICE_HOST_ID,
                t3_contracts::device::LOCAL_DEVICE_HOST_LABEL,
                "Device automation",
                "Device automation",
            ));
        };
        crate::error::validate_arg(&node, "Node executable")?;
        let paths = self.tools.ensure(Tool::Agent).await?;
        Ok(crate::host::AgentCli {
            node,
            entry: paths.entry,
        })
    }
}
impl Toolchain {
    pub fn new(base: PathBuf, commands: Arc<dyn Commands>) -> Self {
        Self {
            base,
            commands,
            lock: Mutex::new(()),
        }
    }
    pub async fn installed(&self, tool: Tool) -> bool {
        installed(&tool.paths(&self.base, tool.version()), tool.version()).await
    }
    pub async fn prune(&self, tool: Tool, inventory: &dyn ProcessInventory) -> Result<()> {
        let root = self.base.join("tools");
        let guard = maintenance_lock(&root, inventory).await?;
        let result = self.prune_locked(tool, inventory).await;
        let cleanup = guard.release().await;
        result.and(cleanup)
    }
    async fn prune_locked(&self, tool: Tool, inventory: &dyn ProcessInventory) -> Result<()> {
        let root = self.base.join("tools");
        let Some(commands) = inventory.command_lines().await else {
            return Ok(());
        };
        let commands = if cfg!(windows) {
            commands.to_lowercase().replace('\\', "/")
        } else {
            commands
        };
        let parent = root.join(tool.name());
        let mut entries = match tokio::fs::read_dir(&parent).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(crate::error::invalid("tool directory")),
        };
        let mut completed = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|_| crate::error::invalid("tool directory"))?
        {
            if !entry
                .file_type()
                .await
                .map_err(|_| crate::error::invalid("tool directory"))?
                .is_dir()
            {
                continue;
            }
            let version = entry.file_name().to_string_lossy().into_owned();
            if !version_valid(&version) {
                continue;
            }
            let sentinel = entry.path().join(".install-complete");
            if tokio::fs::read_to_string(&sentinel)
                .await
                .is_ok_and(|s| s.trim() == version)
                && let Ok(modified) = tokio::fs::metadata(&sentinel)
                    .await
                    .and_then(|m| m.modified())
            {
                completed.push((version, entry.path(), modified));
            }
        }
        if !completed.iter().any(|(v, _, _)| v == tool.version()) {
            return Ok(());
        }
        let previous = completed
            .iter()
            .filter(|(v, _, _)| v != tool.version())
            .max_by(|(av, _, at), (bv, _, bt)| at.cmp(bt).then_with(|| natural_version_cmp(av, bv)))
            .map(|(v, _, _)| v.clone());
        for (version, path, _) in completed {
            let needle = format!("{}{}", path.display(), std::path::MAIN_SEPARATOR);
            let needle = if cfg!(windows) {
                needle.to_lowercase().replace('\\', "/")
            } else {
                needle
            };
            if version == tool.version()
                || previous.as_ref() == Some(&version)
                || commands.contains(&needle)
            {
                continue;
            }
            tokio::fs::remove_dir_all(path)
                .await
                .map_err(|_| crate::error::invalid("pruning device tool"))?;
        }
        Ok(())
    }
    pub async fn ensure(&self, tool: Tool) -> Result<ToolPaths> {
        let _guard = self.lock.lock().await;
        let paths = tool.paths(&self.base, tool.version());
        if installed(&paths, tool.version()).await {
            return Ok(paths);
        }
        let parent = paths
            .install
            .parent()
            .ok_or_else(|| crate::error::invalid("tool path"))?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|_| install_error(tool, "preparing the install directory", None))?;
        match tokio::fs::remove_dir_all(&paths.install).await {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(install_error(tool, "removing an incomplete install", None)),
        }
        let staging = tempfile::Builder::new()
            .prefix(".staging-")
            .tempdir_in(parent)
            .map_err(|_| install_error(tool, "preparing the install directory", None))?;
        let args = vec![
            "install".into(),
            "--prefix".into(),
            staging.path().to_string_lossy().into_owned(),
            "--no-fund".into(),
            "--no-audit".into(),
            format!("{}@{}", tool.name(), tool.version()),
        ];
        let mut command = Command::new("npm", &args);
        command.timeout = Duration::from_secs(600);
        let mut result = self
            .commands
            .run(command)
            .await
            .map_err(|_| install_error(tool, "running npm install", None))?;
        if result.code == 127 {
            let mut fallback = Command::new(
                "pnpm",
                [
                    vec!["--package=npm@11".into(), "dlx".into(), "npm".into()],
                    args,
                ]
                .concat(),
            );
            fallback.timeout = Duration::from_secs(600);
            result = self
                .commands
                .run(fallback)
                .await
                .map_err(|_| install_error(tool, "running npm install", None))?;
        }
        if result.code == 127 {
            return Err(Error::Unavailable("Device support needs npm, which is not on the T3 server's PATH. Install npm (it comes with Node.js from nodejs.org), restart T3 Code, then retry.".into()));
        }
        if result.code != 0 {
            return Err(install_error(
                tool,
                "running npm install",
                Some(result.code),
            ));
        }
        if !tokio::fs::try_exists(
            staging
                .path()
                .join("node_modules")
                .join(tool.name())
                .join(tool.entry()),
        )
        .await
        .unwrap_or(false)
        {
            return Err(install_error(
                tool,
                "verifying the installed entry point",
                None,
            ));
        }
        tokio::fs::write(
            staging.path().join(".install-complete"),
            format!("{}\n", tool.version()),
        )
        .await
        .map_err(|_| install_error(tool, "recording the completed install", None))?;
        if tokio::fs::rename(staging.path(), &paths.install)
            .await
            .is_err()
            && !installed(&paths, tool.version()).await
        {
            return Err(install_error(tool, "publishing the install", None));
        }
        Ok(paths)
    }
    pub async fn inventory(
        &self,
        hub: Option<String>,
        agent: Option<String>,
    ) -> Option<DeviceToolVersions> {
        Some(DeviceToolVersions {
            hub: self.inspect(Tool::Hub, hub).await.ok()?,
            agent: self.inspect(Tool::Agent, agent).await.ok()?,
        })
    }
    async fn inspect(
        &self,
        tool: Tool,
        running_version: Option<String>,
    ) -> Result<DeviceToolVersion> {
        let mut entries = match tokio::fs::read_dir(self.base.join("tools").join(tool.name())).await
        {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(DeviceToolVersion {
                    required_version: tool.version().into(),
                    installed_versions: vec![],
                    running_version,
                });
            }
            Err(_) => return Err(crate::error::invalid("inventory")),
        };
        let mut versions = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|_| crate::error::invalid("inventory"))?
        {
            let version = entry.file_name().to_string_lossy().into_owned();
            if !version_valid(&version) {
                continue;
            }
            let paths = tool.paths(&self.base, &version);
            let sentinel = match tokio::fs::read_to_string(&paths.sentinel).await {
                Ok(s) => s,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return Err(crate::error::invalid("inventory")),
            };
            if sentinel.trim() == version
                && tokio::fs::try_exists(&paths.entry)
                    .await
                    .map_err(|_| crate::error::invalid("inventory"))?
            {
                versions.push(version);
            }
        }
        versions.sort();
        Ok(DeviceToolVersion {
            required_version: tool.version().into(),
            installed_versions: versions,
            running_version,
        })
    }
}
fn natural_version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |s: &str| {
        s.split(['.', '-'])
            .map(|p| {
                p.parse::<u64>()
                    .map_or_else(|_| (0, p.to_owned()), |n| (n, String::new()))
            })
            .collect::<Vec<_>>()
    };
    parts(a).cmp(&parts(b))
}
#[async_trait]
impl ToolInstaller for Toolchain {
    async fn install(&self, tool: DeviceUpdateTool) -> Result<()> {
        self.ensure(match tool {
            DeviceUpdateTool::Hub => Tool::Hub,
            DeviceUpdateTool::Agent => Tool::Agent,
        })
        .await
        .map(|_| ())
    }
}
async fn installed(paths: &ToolPaths, version: &str) -> bool {
    tokio::fs::try_exists(&paths.entry).await.unwrap_or(false)
        && tokio::fs::read_to_string(&paths.sentinel)
            .await
            .is_ok_and(|s| s.trim() == version)
}
pub fn version_valid(version: &str) -> bool {
    regex::Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[a-zA-Z0-9.-]+)?$")
        .is_ok_and(|r| r.is_match(version))
}
fn install_error(tool: Tool, step: &str, code: Option<i32>) -> Error {
    Error::Unavailable(format!(
        "Installing {} failed while {step}{}.",
        tool.name(),
        code.map(|c| format!(" (exit code {c})"))
            .unwrap_or_default()
    ))
}
