use crate::{
    Result,
    io::{Command, CommandOutput, Secret},
};
use async_trait::async_trait;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use t3_contracts::device::*;

#[derive(Clone, Default)]
pub struct OpenContext {
    pub workspace_root: Option<PathBuf>,
}

#[derive(Clone, Default)]
pub struct Listing {
    pub devices: Vec<DeviceSummary>,
    pub detail: Option<String>,
    pub android_discovery_incomplete: bool,
    pub incomplete_platforms: Vec<DevicePlatform>,
}

#[derive(Clone)]
pub struct AgentEndpoint {
    pub base_url: String,
    pub token: Secret,
    pub entry_path: PathBuf,
    pub agent_device_id: Option<String>,
    pub session: Option<String>,
    pub state_dir: Option<PathBuf>,
    pub install_directory: Option<PathBuf>,
}
#[derive(Clone)]
pub struct AgentCli {
    pub node: String,
    pub entry: PathBuf,
}
#[async_trait]
pub trait AgentCliProvider: Send + Sync {
    async fn ensure(&self) -> Result<AgentCli>;
}
impl std::fmt::Debug for AgentEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AgentEndpoint([REDACTED])")
    }
}

#[derive(Clone)]
pub struct Upstream {
    pub url: String,
    pub headers: BTreeMap<String, Secret>,
    pub protocols: Vec<Secret>,
}

#[async_trait]
pub trait DeviceOperations: Send + Sync {
    async fn list(&self) -> Result<Listing>;
    async fn boot(&self, device: &DeviceSummary, context: &OpenContext) -> Result<String>;
    async fn attach(&self, device: &DeviceSummary) -> Result<()>;
    async fn shutdown(&self, id: &str, platform: DevicePlatform) -> Result<()>;
    async fn screenshot(&self, device: &DeviceSummary) -> Result<Vec<u8>>;
    async fn agent_endpoint(&self, _id: &str) -> Result<Option<AgentEndpoint>> {
        Ok(None)
    }
    fn ends_devices(&self) -> bool {
        false
    }
}

#[derive(Clone)]
pub struct Ready {
    pub origin: String,
    pub node_path: String,
    pub devices: Arc<dyn DeviceOperations>,
    pub commands: Arc<dyn crate::io::Commands>,
    pub ax_helper: Option<String>,
    pub sim_cli: Option<String>,
    pub agent: Option<AgentEndpoint>,
    pub routing: Option<Arc<dyn Routing>>,
    pub release: Option<Arc<dyn SessionRelease>>,
}

#[async_trait]
pub trait Routing: Send + Sync {
    async fn upstream(&self, path: &str, query: &[(String, String)]) -> Result<Option<Upstream>>;
}
#[async_trait]
pub trait SessionRelease: Send + Sync {
    async fn release(&self, session: &str, device: &str) -> Result<()>;
}

pub type Phase = Arc<dyn Fn(DeviceHostStatus, Option<String>) + Send + Sync>;

#[async_trait]
pub trait DeviceHost: Send + Sync {
    fn id(&self) -> &str;
    fn label(&self) -> &str;
    async fn summary(&self) -> DeviceHostSummary;
    async fn inspect(&self) -> Result<DeviceHostSummary> {
        Ok(self.summary().await)
    }
    async fn availability(&self, platform: DevicePlatform) -> DevicePlatformAvailability;
    async fn ensure_ready(&self, phase: Phase) -> Result<Ready>;
    async fn ensure_agent_ready(&self, phase: Phase) -> Result<Ready>;
    async fn current(&self) -> Option<Ready>;
    async fn stop_agent(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    async fn retire(&self) -> Result<()> {
        self.stop().await
    }
    async fn run(&self, command: Command) -> Result<CommandOutput> {
        match self.current().await {
            Some(ready) => ready.commands.run(command).await,
            None => Err(crate::Error::host(
                self.id(),
                Some(self.label()),
                "Device host is not running.",
            )),
        }
    }
}

#[async_trait]
pub trait HostFactory: Send + Sync {
    async fn create(&self, config: &DeviceHostConfig) -> Result<Arc<dyn DeviceHost>>;
    async fn test(&self, config: &SshDeviceHostConfig) -> Result<DeviceHostSummary>;
    async fn is_local(&self, _config: &SshDeviceHostConfig) -> bool {
        false
    }
}

pub fn summary(
    id: &str,
    label: &str,
    kind: DeviceHostKind,
    platforms: Vec<DevicePlatformAvailability>,
) -> DeviceHostSummary {
    DeviceHostSummary {
        id: id.into(),
        label: label.into(),
        kind,
        platforms,
        tools: None,
        tool_inspection_error: None,
        hub_installed: false,
        agent_device_installed: false,
    }
}

pub fn available(platform: DevicePlatform, reason: Option<String>) -> DevicePlatformAvailability {
    DevicePlatformAvailability {
        platform,
        available: reason.is_none(),
        reason: reason.map(Some),
    }
}
