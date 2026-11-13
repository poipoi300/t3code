use crate::{
    base_schemas::{AuthSessionId, EnvironmentId, RpcClientId, ThreadId},
    provider_instance::ProviderInstanceId,
    speech::SpeechNumber,
    wire::WireDateTime,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackgroundBooleanState {
    #[serde(rename = "true")]
    True,
    #[serde(rename = "false")]
    False,
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostPowerThermalState {
    #[serde(rename = "unknown")]
    Unknown,
    #[serde(rename = "nominal")]
    Nominal,
    #[serde(rename = "fair")]
    Fair,
    #[serde(rename = "serious")]
    Serious,
    #[serde(rename = "critical")]
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostPowerSource {
    #[serde(rename = "unknown")]
    Unknown,
    #[serde(rename = "node-macos-shell")]
    NodeMacosShell,
    #[serde(rename = "node-macos-native")]
    NodeMacosNative,
    #[serde(rename = "node-linux")]
    NodeLinux,
    #[serde(rename = "node-windows")]
    NodeWindows,
    #[serde(rename = "electron-main")]
    ElectronMain,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPowerSnapshot {
    pub source: HostPowerSource,
    pub idle: BackgroundBooleanState,
    #[serde(with = "crate::wire::nullable")]
    pub idle_seconds: Option<SpeechNumber>,
    pub locked: BackgroundBooleanState,
    pub suspended: bool,
    pub on_battery: BackgroundBooleanState,
    pub low_power_mode: BackgroundBooleanState,
    pub thermal_state: HostPowerThermalState,
    pub stale: bool,
    pub updated_at: WireDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BackgroundScope {
    #[serde(rename = "server-config")]
    ServerConfig,
    #[serde(rename = "provider-status")]
    ProviderStatus {
        #[serde(
            rename = "instanceId",
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::optional"
        )]
        instance_id: Option<ProviderInstanceId>,
    },
    #[serde(rename = "vcs-status")]
    VcsStatus { cwd: String },
    #[serde(rename = "git-refs")]
    GitRefs { cwd: String },
    #[serde(rename = "diagnostics")]
    Diagnostics,
    #[serde(rename = "thread")]
    Thread {
        #[serde(rename = "threadId")]
        thread_id: ThreadId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientKind {
    #[serde(rename = "web")]
    Web,
    #[serde(rename = "desktop-renderer")]
    DesktopRenderer,
    #[serde(rename = "mobile")]
    Mobile,
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientAppState {
    #[serde(rename = "active")]
    Active,
    #[serde(rename = "inactive")]
    Inactive,
    #[serde(rename = "background")]
    Background,
    #[serde(rename = "unknown")]
    Unknown,
}

pub type ClientActivityClientId = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientActivityReportInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub environment_id: Option<EnvironmentId>,
    pub client_id: ClientActivityClientId,
    pub client_kind: ClientKind,
    pub visible: bool,
    pub focused: bool,
    pub recently_interacted: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub app_state: Option<ClientAppState>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub low_power_mode: Option<BackgroundBooleanState>,
    pub scopes: Vec<BackgroundScope>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub ttl_ms: Option<SpeechNumber>,
    pub observed_at: WireDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientActivityLease {
    pub session_id: AuthSessionId,
    pub rpc_client_id: RpcClientId,
    pub client_id: ClientActivityClientId,
    pub client_kind: ClientKind,
    pub visible: bool,
    pub focused: bool,
    pub recently_interacted: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub app_state: Option<ClientAppState>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub low_power_mode: Option<BackgroundBooleanState>,
    pub scopes: Vec<BackgroundScope>,
    pub updated_at: WireDateTime,
    pub expires_at: WireDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundPolicySnapshot {
    pub host_power: HostPowerSnapshot,
    pub leases: Vec<ClientActivityLease>,
    pub active_foreground_lease_count: SpeechNumber,
    pub active_scope_keys: Vec<String>,
    pub should_run_opportunistic_work: bool,
    pub updated_at: WireDateTime,
}
