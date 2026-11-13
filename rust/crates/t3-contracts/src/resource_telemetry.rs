use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireLiteral<const VALUE: u64>;
impl<const VALUE: u64> Serialize for WireLiteral<VALUE> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(VALUE)
    }
}
impl<'de, const VALUE: u64> Deserialize<'de> for WireLiteral<VALUE> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let actual = u64::deserialize(deserializer)?;
        if actual == VALUE {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!("expected {VALUE}")))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", content = "value")]
pub enum WireOption<T> {
    None,
    Some(T),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopElectronProcessType {
    #[serde(rename = "Browser")]
    Browser,
    #[serde(rename = "Tab")]
    Tab,
    #[serde(rename = "Utility")]
    Utility,
    #[serde(rename = "Zygote")]
    Zygote,
    #[serde(rename = "Sandbox helper")]
    SandboxHelper,
    #[serde(rename = "GPU")]
    GPU,
    #[serde(rename = "Pepper Plugin")]
    PepperPlugin,
    #[serde(rename = "Pepper Plugin Broker")]
    PepperPluginBroker,
    #[serde(rename = "Unknown")]
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopElectronProcessMetric {
    #[serde()]
    pub pid: u64,
    #[serde()]
    pub creation_time_ms: u64,
    #[serde()]
    pub r#type: DesktopElectronProcessType,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub service_name: Option<String>,
    #[serde()]
    pub cpu_percent: crate::settings::WireNumber,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub cumulative_cpu_seconds: Option<crate::settings::WireNumber>,
    #[serde()]
    pub idle_wakeups_per_second: crate::settings::WireNumber,
    #[serde()]
    pub working_set_bytes: u64,
    #[serde()]
    pub peak_working_set_bytes: u64,
}
pub type DesktopHostTelemetryHelloVersion = WireLiteral<1>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopHostTelemetryHelloType {
    #[serde(rename = "desktopTelemetryHello")]
    DesktopTelemetryHello,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopHostTelemetryHello {
    #[serde()]
    pub version: DesktopHostTelemetryHelloVersion,
    #[serde()]
    pub r#type: DesktopHostTelemetryHelloType,
    #[serde()]
    pub electron_pid: u64,
}
pub type DesktopShellEnvironmentSnapshotVersion = WireLiteral<1>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopShellEnvironmentSnapshotType {
    #[serde(rename = "shellEnvironment")]
    ShellEnvironment,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ShellEnvironmentPatch {
    #[serde(
        rename = "PATH",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub path: Option<Option<String>>,
    #[serde(
        rename = "DBUS_SESSION_BUS_ADDRESS",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub dbus_session_bus_address: Option<Option<String>>,
    #[serde(
        rename = "DISPLAY",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub display: Option<Option<String>>,
    #[serde(
        rename = "LANG",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub lang: Option<Option<String>>,
    #[serde(
        rename = "LC_ALL",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub lc_all: Option<Option<String>>,
    #[serde(
        rename = "LC_CTYPE",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub lc_ctype: Option<Option<String>>,
    #[serde(
        rename = "SSH_AUTH_SOCK",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub ssh_auth_sock: Option<Option<String>>,
    #[serde(
        rename = "HOMEBREW_PREFIX",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub homebrew_prefix: Option<Option<String>>,
    #[serde(
        rename = "HOMEBREW_CELLAR",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub homebrew_cellar: Option<Option<String>>,
    #[serde(
        rename = "HOMEBREW_REPOSITORY",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub homebrew_repository: Option<Option<String>>,
    #[serde(
        rename = "XDG_CONFIG_HOME",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub xdg_config_home: Option<Option<String>>,
    #[serde(
        rename = "XDG_CURRENT_DESKTOP",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub xdg_current_desktop: Option<Option<String>>,
    #[serde(
        rename = "XDG_DATA_HOME",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub xdg_data_home: Option<Option<String>>,
    #[serde(
        rename = "XDG_RUNTIME_DIR",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub xdg_runtime_dir: Option<Option<String>>,
    #[serde(
        rename = "XDG_SESSION_DESKTOP",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub xdg_session_desktop: Option<Option<String>>,
    #[serde(
        rename = "XDG_SESSION_TYPE",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub xdg_session_type: Option<Option<String>>,
    #[serde(
        rename = "WAYLAND_DISPLAY",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub wayland_display: Option<Option<String>>,
    #[serde(
        rename = "SSL_CERT_FILE",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub ssl_cert_file: Option<Option<String>>,
    #[serde(
        rename = "SSL_CERT_DIR",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub ssl_cert_dir: Option<Option<String>>,
    #[serde(
        rename = "NODE_EXTRA_CA_CERTS",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub node_extra_ca_certs: Option<Option<String>>,
    #[serde(
        rename = "T3CODE_TELEMETRY_ENABLED",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub t3_code_telemetry_enabled: Option<Option<String>>,
    #[serde(
        rename = "T3CODE_GITHUB_APP_CLIENT_ID",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub t3_code_github_app_client_id: Option<Option<String>>,
    #[serde(
        rename = "__MISE_DIFF",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub __mise_diff: Option<Option<String>>,
    #[serde(
        rename = "JAVA_HOME",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub java_home: Option<Option<String>>,
    #[serde(
        rename = "FNM_DIR",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub fnm_dir: Option<Option<String>>,
    #[serde(
        rename = "FNM_MULTISHELL_PATH",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub fnm_multishell_path: Option<Option<String>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopShellEnvironmentSnapshot {
    #[serde()]
    pub version: DesktopShellEnvironmentSnapshotVersion,
    #[serde()]
    pub r#type: DesktopShellEnvironmentSnapshotType,
    #[serde()]
    pub discovered: bool,
    #[serde()]
    pub platform: String,
    #[serde()]
    pub environment: ShellEnvironmentPatch,
}
pub type DesktopHostTelemetrySnapshotVersion = WireLiteral<1>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopHostTelemetrySnapshotType {
    #[serde(rename = "desktopTelemetry")]
    DesktopTelemetry,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopHostPowerSnapshot {
    #[serde()]
    pub source: crate::background::HostPowerSource,
    #[serde()]
    pub idle: crate::background::BackgroundBooleanState,
    #[serde(with = "crate::wire::nullable")]
    pub idle_seconds: Option<crate::settings::WireNumber>,
    #[serde()]
    pub locked: crate::background::BackgroundBooleanState,
    #[serde()]
    pub suspended: bool,
    #[serde()]
    pub on_battery: crate::background::BackgroundBooleanState,
    #[serde()]
    pub low_power_mode: crate::background::BackgroundBooleanState,
    #[serde()]
    pub thermal_state: crate::background::HostPowerThermalState,
    #[serde()]
    pub stale: bool,
    #[serde()]
    pub updated_at: crate::wire::WireDateTime,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopHostTelemetrySnapshot {
    #[serde()]
    pub version: DesktopHostTelemetrySnapshotVersion,
    #[serde()]
    pub r#type: DesktopHostTelemetrySnapshotType,
    #[serde()]
    pub sequence: u64,
    #[serde()]
    pub sampled_at_unix_ms: u64,
    #[serde()]
    pub electron_pid: u64,
    #[serde()]
    pub power: DesktopHostPowerSnapshot,
    #[serde(with = "crate::wire::nullable")]
    pub speed_limit_percent: Option<crate::settings::WireNumber>,
    #[serde()]
    pub electron_processes: Vec<DesktopElectronProcessMetric>,
}
pub type DesktopUpdateStatusReportVersion = WireLiteral<1>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopUpdateStatusReportType {
    #[serde(rename = "desktopUpdateStatus")]
    DesktopUpdateStatus,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopUpdateRemoteOutcome {
    #[serde(rename = "ready-to-install")]
    ReadyToInstall,
    #[serde(rename = "up-to-date")]
    UpToDate,
    #[serde(rename = "failed")]
    Failed,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopUpdateStatusReport {
    #[serde()]
    pub version: DesktopUpdateStatusReportVersion,
    #[serde()]
    pub r#type: DesktopUpdateStatusReportType,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub request_id: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub outcome: Option<DesktopUpdateRemoteOutcome>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub reason: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde()]
    pub state: crate::ipc::DesktopUpdateStateSchema,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DesktopHostTelemetryMessage {
    DesktopHostTelemetryHello(Box<DesktopHostTelemetryHello>),
    DesktopShellEnvironmentSnapshot(Box<DesktopShellEnvironmentSnapshot>),
    DesktopHostTelemetrySnapshot(Box<DesktopHostTelemetrySnapshot>),
    DesktopUpdateStatusReport(Box<DesktopUpdateStatusReport>),
}
pub type DesktopTelemetryCancelDesktopUpdateVersion = WireLiteral<1>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopTelemetryCancelDesktopUpdateType {
    #[serde(rename = "cancelDesktopUpdate")]
    CancelDesktopUpdate,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopTelemetryCancelDesktopUpdate {
    #[serde()]
    pub version: DesktopTelemetryCancelDesktopUpdateVersion,
    #[serde()]
    pub r#type: DesktopTelemetryCancelDesktopUpdateType,
    #[serde()]
    pub request_id: crate::base_schemas::TrimmedNonEmptyString,
}
pub type DesktopTelemetryCommitDesktopUpdateVersion = WireLiteral<1>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopTelemetryCommitDesktopUpdateType {
    #[serde(rename = "commitDesktopUpdate")]
    CommitDesktopUpdate,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopTelemetryCommitDesktopUpdate {
    #[serde()]
    pub version: DesktopTelemetryCommitDesktopUpdateVersion,
    #[serde()]
    pub r#type: DesktopTelemetryCommitDesktopUpdateType,
    #[serde()]
    pub request_id: crate::base_schemas::TrimmedNonEmptyString,
}
pub type DesktopTelemetrySetDiagnosticsDemandVersion = WireLiteral<1>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopTelemetrySetDiagnosticsDemandType {
    #[serde(rename = "setDiagnosticsDemand")]
    SetDiagnosticsDemand,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopTelemetrySetDiagnosticsDemand {
    #[serde()]
    pub version: DesktopTelemetrySetDiagnosticsDemandVersion,
    #[serde()]
    pub r#type: DesktopTelemetrySetDiagnosticsDemandType,
    #[serde()]
    pub enabled: bool,
}
pub type DesktopTelemetrySetHostPowerIntervalsVersion = WireLiteral<1>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopTelemetrySetHostPowerIntervalsType {
    #[serde(rename = "setHostPowerIntervals")]
    SetHostPowerIntervals,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopTelemetrySetHostPowerIntervals {
    #[serde()]
    pub version: DesktopTelemetrySetHostPowerIntervalsVersion,
    #[serde()]
    pub r#type: DesktopTelemetrySetHostPowerIntervalsType,
    #[serde()]
    pub active_interval_ms: u64,
    #[serde()]
    pub idle_interval_ms: u64,
}
pub type DesktopTelemetryRequestDesktopUpdateVersion = WireLiteral<1>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopTelemetryRequestDesktopUpdateType {
    #[serde(rename = "requestDesktopUpdate")]
    RequestDesktopUpdate,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopTelemetryRequestDesktopUpdate {
    #[serde()]
    pub version: DesktopTelemetryRequestDesktopUpdateVersion,
    #[serde()]
    pub r#type: DesktopTelemetryRequestDesktopUpdateType,
    #[serde()]
    pub request_id: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub channel: Option<crate::ipc::DesktopUpdateChannelSchema>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub target_version: Option<crate::base_schemas::TrimmedNonEmptyString>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DesktopTelemetryControlMessage {
    DesktopTelemetrySetDiagnosticsDemand(Box<DesktopTelemetrySetDiagnosticsDemand>),
    DesktopTelemetrySetHostPowerIntervals(Box<DesktopTelemetrySetHostPowerIntervals>),
    DesktopTelemetryRequestDesktopUpdate(Box<DesktopTelemetryRequestDesktopUpdate>),
    DesktopTelemetryCommitDesktopUpdate(Box<DesktopTelemetryCommitDesktopUpdate>),
    DesktopTelemetryCancelDesktopUpdate(Box<DesktopTelemetryCancelDesktopUpdate>),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostResourcesSnapshot {
    #[serde()]
    pub sampled_at: u64,
    #[serde(with = "crate::wire::nullable")]
    pub cpu_utilization: Option<crate::wire::JsNumber>,
    #[serde()]
    pub cpu_count: u64,
    #[serde()]
    pub available_memory_bytes: u64,
    #[serde()]
    pub total_memory_bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceAttributionEntry {
    #[serde()]
    pub component: crate::base_schemas::TrimmedNonEmptyString,
    #[serde()]
    pub operation: crate::base_schemas::TrimmedNonEmptyString,
    #[serde()]
    pub logical_read_bytes: u64,
    #[serde()]
    pub logical_write_bytes: u64,
    #[serde()]
    pub count: u64,
    #[serde()]
    pub duration_ms: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceAttributionSnapshot {
    #[serde()]
    pub read_at: crate::wire::WireDateTime,
    #[serde()]
    pub entries: Vec<ResourceAttributionEntry>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMonitorCapabilities {
    #[serde()]
    pub cumulative_cpu_time: bool,
    #[serde()]
    pub current_cpu_percent: bool,
    #[serde()]
    pub resident_memory: bool,
    #[serde()]
    pub virtual_memory: bool,
    #[serde()]
    pub io_bytes: bool,
    #[serde()]
    pub process_start_time: bool,
    #[serde()]
    pub process_tree: bool,
}
pub type ResourceMonitorConfigureCommandVersion = WireLiteral<3>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceMonitorConfigureCommandType {
    #[serde(rename = "configure")]
    Configure,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMonitorExternalProcess {
    #[serde()]
    pub pid: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub start_time_ms: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMonitorConfigureCommand {
    #[serde()]
    pub version: ResourceMonitorConfigureCommandVersion,
    #[serde()]
    pub r#type: ResourceMonitorConfigureCommandType,
    #[serde()]
    pub root_pid: u64,
    #[serde()]
    pub sample_interval_ms: u64,
    #[serde()]
    pub external_processes: Vec<ResourceMonitorExternalProcess>,
}
pub type ResourceMonitorSetExternalProcessesCommandVersion = WireLiteral<3>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceMonitorSetExternalProcessesCommandType {
    #[serde(rename = "setExternalProcesses")]
    SetExternalProcesses,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMonitorSetExternalProcessesCommand {
    #[serde()]
    pub version: ResourceMonitorSetExternalProcessesCommandVersion,
    #[serde()]
    pub r#type: ResourceMonitorSetExternalProcessesCommandType,
    #[serde()]
    pub processes: Vec<ResourceMonitorExternalProcess>,
}
pub type ResourceMonitorSetSampleIntervalCommandVersion = WireLiteral<3>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceMonitorSetSampleIntervalCommandType {
    #[serde(rename = "setSampleInterval")]
    SetSampleInterval,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMonitorSetSampleIntervalCommand {
    #[serde()]
    pub version: ResourceMonitorSetSampleIntervalCommandVersion,
    #[serde()]
    pub r#type: ResourceMonitorSetSampleIntervalCommandType,
    #[serde()]
    pub sample_interval_ms: u64,
}
pub type ResourceMonitorSetStreamingCommandVersion = WireLiteral<3>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceMonitorSetStreamingCommandType {
    #[serde(rename = "setStreaming")]
    SetStreaming,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMonitorSetStreamingCommand {
    #[serde()]
    pub version: ResourceMonitorSetStreamingCommandVersion,
    #[serde()]
    pub r#type: ResourceMonitorSetStreamingCommandType,
    #[serde()]
    pub enabled: bool,
}
pub type ResourceMonitorSampleNowCommandVersion = WireLiteral<3>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceMonitorSampleNowCommandType {
    #[serde(rename = "sampleNow")]
    SampleNow,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMonitorSampleNowCommand {
    #[serde()]
    pub version: ResourceMonitorSampleNowCommandVersion,
    #[serde()]
    pub r#type: ResourceMonitorSampleNowCommandType,
    #[serde()]
    pub request_id: crate::base_schemas::TrimmedNonEmptyString,
}
pub type ResourceMonitorProcessTableCommandVersion = WireLiteral<3>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceMonitorProcessTableCommandType {
    #[serde(rename = "processTable")]
    ProcessTable,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMonitorProcessTableCommand {
    #[serde()]
    pub version: ResourceMonitorProcessTableCommandVersion,
    #[serde()]
    pub r#type: ResourceMonitorProcessTableCommandType,
    #[serde()]
    pub request_id: crate::base_schemas::TrimmedNonEmptyString,
}
pub type ResourceMonitorReadHistoryCommandVersion = WireLiteral<3>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceMonitorReadHistoryCommandType {
    #[serde(rename = "readHistory")]
    ReadHistory,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMonitorReadHistoryCommand {
    #[serde()]
    pub version: ResourceMonitorReadHistoryCommandVersion,
    #[serde()]
    pub r#type: ResourceMonitorReadHistoryCommandType,
    #[serde()]
    pub request_id: crate::base_schemas::TrimmedNonEmptyString,
    #[serde()]
    pub window_ms: u64,
}
pub type ResourceMonitorShutdownCommandVersion = WireLiteral<3>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceMonitorShutdownCommandType {
    #[serde(rename = "shutdown")]
    Shutdown,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMonitorShutdownCommand {
    #[serde()]
    pub version: ResourceMonitorShutdownCommandVersion,
    #[serde()]
    pub r#type: ResourceMonitorShutdownCommandType,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResourceMonitorCommand {
    ResourceMonitorConfigureCommand(Box<ResourceMonitorConfigureCommand>),
    ResourceMonitorSetExternalProcessesCommand(Box<ResourceMonitorSetExternalProcessesCommand>),
    ResourceMonitorSetSampleIntervalCommand(Box<ResourceMonitorSetSampleIntervalCommand>),
    ResourceMonitorSetStreamingCommand(Box<ResourceMonitorSetStreamingCommand>),
    ResourceMonitorSampleNowCommand(Box<ResourceMonitorSampleNowCommand>),
    ResourceMonitorProcessTableCommand(Box<ResourceMonitorProcessTableCommand>),
    ResourceMonitorReadHistoryCommand(Box<ResourceMonitorReadHistoryCommand>),
    ResourceMonitorShutdownCommand(Box<ResourceMonitorShutdownCommand>),
}
pub type ResourceMonitorErrorEventVersion = WireLiteral<3>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceMonitorErrorEventType {
    #[serde(rename = "error")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMonitorErrorEvent {
    #[serde()]
    pub version: ResourceMonitorErrorEventVersion,
    #[serde()]
    pub r#type: ResourceMonitorErrorEventType,
    #[serde()]
    pub code: crate::base_schemas::TrimmedNonEmptyString,
    #[serde()]
    pub message: crate::base_schemas::TrimmedNonEmptyString,
    #[serde()]
    pub recoverable: bool,
}
pub type ResourceMonitorHelloEventVersion = WireLiteral<3>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceMonitorHelloEventType {
    #[serde(rename = "hello")]
    Hello,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMonitorHelloEvent {
    #[serde()]
    pub version: ResourceMonitorHelloEventVersion,
    #[serde()]
    pub r#type: ResourceMonitorHelloEventType,
    #[serde()]
    pub sidecar_version: crate::base_schemas::TrimmedNonEmptyString,
    #[serde()]
    pub sidecar_pid: u64,
    #[serde()]
    pub platform: crate::base_schemas::TrimmedNonEmptyString,
    #[serde()]
    pub arch: crate::base_schemas::TrimmedNonEmptyString,
    #[serde()]
    pub capabilities: ResourceMonitorCapabilities,
}
pub type ResourceMonitorSnapshotEventVersion = WireLiteral<3>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceMonitorSnapshotEventType {
    #[serde(rename = "snapshot")]
    Snapshot,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceMonitorProcessSampleIoSemantics {
    #[serde(rename = "storage")]
    Storage,
    #[serde(rename = "all-io")]
    AllIo,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMonitorProcessSample {
    #[serde()]
    pub pid: u64,
    #[serde()]
    pub ppid: u64,
    #[serde()]
    pub start_time_ms: u64,
    #[serde()]
    pub run_time_ms: u64,
    #[serde()]
    pub name: String,
    #[serde()]
    pub command: String,
    #[serde()]
    pub status: String,
    #[serde()]
    pub cpu_percent: crate::settings::WireNumber,
    #[serde()]
    pub cpu_time_ms: u64,
    #[serde()]
    pub resident_bytes: u64,
    #[serde()]
    pub virtual_bytes: u64,
    #[serde()]
    pub io_read_bytes: u64,
    #[serde()]
    pub io_write_bytes: u64,
    #[serde()]
    pub io_semantics: ResourceMonitorProcessSampleIoSemantics,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMonitorSnapshotEvent {
    #[serde()]
    pub version: ResourceMonitorSnapshotEventVersion,
    #[serde()]
    pub r#type: ResourceMonitorSnapshotEventType,
    #[serde()]
    pub sequence: u64,
    #[serde()]
    pub sampled_at_unix_ms: u64,
    #[serde()]
    pub collection_duration_micros: u64,
    #[serde()]
    pub scanned_process_count: u64,
    #[serde()]
    pub retained_process_count: u64,
    #[serde()]
    pub inaccessible_process_count: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub request_id: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub external_processes: Option<Vec<ResourceMonitorExternalProcess>>,
    #[serde()]
    pub processes: Vec<ResourceMonitorProcessSample>,
}
pub type ResourceMonitorProcessTableEventVersion = WireLiteral<3>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceMonitorProcessTableEventType {
    #[serde(rename = "processTable")]
    ProcessTable,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMonitorProcessTableEntry {
    #[serde()]
    pub pid: u64,
    #[serde()]
    pub ppid: u64,
    #[serde()]
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMonitorProcessTableEvent {
    #[serde()]
    pub version: ResourceMonitorProcessTableEventVersion,
    #[serde()]
    pub r#type: ResourceMonitorProcessTableEventType,
    #[serde()]
    pub request_id: crate::base_schemas::TrimmedNonEmptyString,
    #[serde()]
    pub processes: Vec<ResourceMonitorProcessTableEntry>,
}
pub type ResourceMonitorHistoryChunkEventVersion = WireLiteral<3>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceMonitorHistoryChunkEventType {
    #[serde(rename = "historyChunk")]
    HistoryChunk,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMonitorHistoryChunkEvent {
    #[serde()]
    pub version: ResourceMonitorHistoryChunkEventVersion,
    #[serde()]
    pub r#type: ResourceMonitorHistoryChunkEventType,
    #[serde()]
    pub request_id: crate::base_schemas::TrimmedNonEmptyString,
    #[serde()]
    pub done: bool,
    #[serde()]
    pub snapshots: Vec<ResourceMonitorSnapshotEvent>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResourceMonitorEvent {
    ResourceMonitorHelloEvent(Box<ResourceMonitorHelloEvent>),
    ResourceMonitorSnapshotEvent(Box<ResourceMonitorSnapshotEvent>),
    ResourceMonitorProcessTableEvent(Box<ResourceMonitorProcessTableEvent>),
    ResourceMonitorHistoryChunkEvent(Box<ResourceMonitorHistoryChunkEvent>),
    ResourceMonitorErrorEvent(Box<ResourceMonitorErrorEvent>),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTelemetryAggregate {
    #[serde()]
    pub process_count: u64,
    #[serde()]
    pub current_cpu_percent: crate::settings::WireNumber,
    #[serde()]
    pub cpu_time_ms: u64,
    #[serde()]
    pub current_rss_bytes: u64,
    #[serde()]
    pub peak_rss_bytes: u64,
    #[serde()]
    pub io_read_bytes: u64,
    #[serde()]
    pub io_write_bytes: u64,
    #[serde()]
    pub io_read_bytes_per_second: crate::settings::WireNumber,
    #[serde()]
    pub io_write_bytes_per_second: crate::settings::WireNumber,
    #[serde()]
    pub process_starts: u64,
    #[serde()]
    pub process_exits: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTelemetryGroups {
    #[serde()]
    pub backend: ResourceTelemetryAggregate,
    #[serde()]
    pub electron: ResourceTelemetryAggregate,
    #[serde()]
    pub monitor: ResourceTelemetryAggregate,
    #[serde()]
    pub all_t3: ResourceTelemetryAggregate,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceTelemetrySourceStatus {
    #[serde(rename = "starting")]
    Starting,
    #[serde(rename = "healthy")]
    Healthy,
    #[serde(rename = "degraded")]
    Degraded,
    #[serde(rename = "unavailable")]
    Unavailable,
    #[serde(rename = "stopped")]
    Stopped,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTelemetrySourceHealth {
    #[serde()]
    pub status: ResourceTelemetrySourceStatus,
    #[serde()]
    pub last_sample_at: WireOption<crate::wire::WireDateTime>,
    #[serde()]
    pub last_error: WireOption<crate::base_schemas::TrimmedNonEmptyString>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTelemetryHealth {
    #[serde()]
    pub native: ResourceTelemetrySourceHealth,
    #[serde()]
    pub desktop: ResourceTelemetrySourceHealth,
    #[serde()]
    pub sidecar_version: WireOption<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde()]
    pub sidecar_pid: WireOption<u64>,
    #[serde()]
    pub restart_count: u64,
    #[serde()]
    pub collection_duration_micros: u64,
    #[serde()]
    pub scanned_process_count: u64,
    #[serde()]
    pub retained_process_count: u64,
    #[serde()]
    pub inaccessible_process_count: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTelemetryHistoryBucket {
    #[serde()]
    pub started_at: crate::wire::WireDateTime,
    #[serde()]
    pub ended_at: crate::wire::WireDateTime,
    #[serde()]
    pub avg_cpu_percent: crate::settings::WireNumber,
    #[serde()]
    pub max_cpu_percent: crate::settings::WireNumber,
    #[serde()]
    pub max_rss_bytes: u64,
    #[serde()]
    pub io_read_bytes: u64,
    #[serde()]
    pub io_write_bytes: u64,
    #[serde()]
    pub max_process_count: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTelemetryProcessIdentity {
    #[serde()]
    pub pid: u64,
    #[serde()]
    pub start_time_ms: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceTelemetryProcessCategory {
    #[serde(rename = "server")]
    Server,
    #[serde(rename = "server-child")]
    ServerChild,
    #[serde(rename = "provider-root")]
    ProviderRoot,
    #[serde(rename = "terminal-root")]
    TerminalRoot,
    #[serde(rename = "electron-main")]
    ElectronMain,
    #[serde(rename = "electron-renderer")]
    ElectronRenderer,
    #[serde(rename = "electron-gpu")]
    ElectronGpu,
    #[serde(rename = "electron-utility")]
    ElectronUtility,
    #[serde(rename = "resource-monitor")]
    ResourceMonitor,
    #[serde(rename = "unknown-t3")]
    UnknownT3,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceTelemetryIoSemantics {
    #[serde(rename = "storage")]
    Storage,
    #[serde(rename = "logical")]
    Logical,
    #[serde(rename = "all-io")]
    AllIo,
    #[serde(rename = "unavailable")]
    Unavailable,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTelemetryProcessSummary {
    #[serde()]
    pub identity: ResourceTelemetryProcessIdentity,
    #[serde()]
    pub ppid: u64,
    #[serde()]
    pub depth: u64,
    #[serde()]
    pub name: String,
    #[serde()]
    pub command: String,
    #[serde()]
    pub category: ResourceTelemetryProcessCategory,
    #[serde()]
    pub first_seen_at: crate::wire::WireDateTime,
    #[serde()]
    pub last_seen_at: crate::wire::WireDateTime,
    #[serde()]
    pub current_cpu_percent: crate::settings::WireNumber,
    #[serde()]
    pub avg_cpu_percent: crate::settings::WireNumber,
    #[serde()]
    pub max_cpu_percent: crate::settings::WireNumber,
    #[serde()]
    pub cpu_time_ms: u64,
    #[serde()]
    pub current_rss_bytes: u64,
    #[serde()]
    pub peak_rss_bytes: u64,
    #[serde()]
    pub io_read_bytes: u64,
    #[serde()]
    pub io_write_bytes: u64,
    #[serde()]
    pub io_semantics: ResourceTelemetryIoSemantics,
    #[serde()]
    pub sample_count: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTelemetryHistory {
    #[serde()]
    pub read_at: crate::wire::WireDateTime,
    #[serde()]
    pub window_ms: u64,
    #[serde()]
    pub bucket_ms: u64,
    #[serde()]
    pub sample_interval_ms: u64,
    #[serde()]
    pub retained_sample_count: u64,
    #[serde()]
    pub buckets: Vec<ResourceTelemetryHistoryBucket>,
    #[serde()]
    pub top_processes: Vec<ResourceTelemetryProcessSummary>,
    #[serde()]
    pub health: ResourceTelemetryHealth,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTelemetryHistoryInput {
    #[serde()]
    pub window_ms: u64,
    #[serde()]
    pub bucket_ms: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTelemetryProcess {
    #[serde()]
    pub identity: ResourceTelemetryProcessIdentity,
    #[serde()]
    pub ppid: u64,
    #[serde()]
    pub child_pids: Vec<u64>,
    #[serde()]
    pub depth: u64,
    #[serde()]
    pub name: String,
    #[serde()]
    pub command: String,
    #[serde()]
    pub status: String,
    #[serde()]
    pub category: ResourceTelemetryProcessCategory,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub electron_type: Option<DesktopElectronProcessType>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub electron_service_name: Option<String>,
    #[serde()]
    pub cpu_percent: crate::settings::WireNumber,
    #[serde()]
    pub cpu_time_ms: u64,
    #[serde()]
    pub resident_bytes: u64,
    #[serde()]
    pub peak_resident_bytes: u64,
    #[serde()]
    pub virtual_bytes: u64,
    #[serde()]
    pub io_read_bytes: u64,
    #[serde()]
    pub io_write_bytes: u64,
    #[serde()]
    pub io_read_bytes_per_second: crate::settings::WireNumber,
    #[serde()]
    pub io_write_bytes_per_second: crate::settings::WireNumber,
    #[serde()]
    pub io_semantics: ResourceTelemetryIoSemantics,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub idle_wakeups_per_second: Option<crate::settings::WireNumber>,
    #[serde()]
    pub run_time_ms: u64,
    #[serde()]
    pub first_seen_at: crate::wire::WireDateTime,
    #[serde()]
    pub last_seen_at: crate::wire::WireDateTime,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTelemetrySnapshot {
    #[serde()]
    pub read_at: crate::wire::WireDateTime,
    #[serde()]
    pub sample_interval_ms: u64,
    #[serde()]
    pub processes: Vec<ResourceTelemetryProcess>,
    #[serde()]
    pub groups: ResourceTelemetryGroups,
    #[serde()]
    pub power: crate::background::HostPowerSnapshot,
    #[serde()]
    pub speed_limit_percent: WireOption<crate::settings::WireNumber>,
    #[serde()]
    pub attribution: ResourceAttributionSnapshot,
    #[serde()]
    pub health: ResourceTelemetryHealth,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTelemetryRetryResult {
    #[serde()]
    pub accepted: bool,
    #[serde()]
    pub snapshot: ResourceTelemetrySnapshot,
}
