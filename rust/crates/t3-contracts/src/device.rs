use crate::base_schemas::ThreadId;
use crate::base_schemas::TrimmedNonEmptyString;
use crate::settings_secret_value::SettingsSecretValue;
use crate::speech::SpeechNumber;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DevicePlatform {
    #[serde(rename = "ios")]
    Ios,
    #[serde(rename = "android")]
    Android,
}
pub type DeviceHostId = TrimmedNonEmptyString;
pub type DeviceId = TrimmedNonEmptyString;
pub const LOCAL_DEVICE_HOST_ID: &str = "local";
pub const LOCAL_DEVICE_HOST_LABEL: &str = "This machine";
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum SshDeviceHostKind {
    #[default]
    #[serde(rename = "ssh")]
    Ssh,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshDeviceHostConfig {
    #[serde(default)]
    pub kind: SshDeviceHostKind,
    pub id: DeviceHostId,
    pub label: TrimmedNonEmptyString,
    pub target: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub identity_file: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub port: Option<Option<u16>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EasDeviceHostKind {
    #[serde(rename = "eas")]
    Eas,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EasDeviceHostConfig {
    pub kind: EasDeviceHostKind,
    pub id: DeviceHostId,
    pub label: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub app_id: Option<Option<TrimmedNonEmptyString>>,
    #[serde(default = "empty_secret")]
    pub access_token: SettingsSecretValue,
    #[serde(default = "default_session_minutes")]
    pub max_session_minutes: u64,
    #[serde(default = "default_idle_minutes")]
    pub idle_minutes: u64,
    #[serde(default = "default_concurrent_sessions")]
    pub max_concurrent_sessions: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SimlockDeviceHostKind {
    #[serde(rename = "simlock")]
    Simlock,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimlockDeviceHostConfig {
    pub kind: SimlockDeviceHostKind,
    pub id: DeviceHostId,
    pub label: TrimmedNonEmptyString,
    pub gateway_url: TrimmedNonEmptyString,
    #[serde(default = "empty_secret")]
    pub agent_token: SettingsSecretValue,
    #[serde(default = "default_concurrent_sessions")]
    pub max_leases: u64,
    #[serde(default = "default_lease_minutes")]
    pub max_lease_minutes: u64,
}
fn empty_secret() -> SettingsSecretValue {
    SettingsSecretValue::Literal(String::new())
}
fn default_session_minutes() -> u64 {
    30
}
fn default_idle_minutes() -> u64 {
    10
}
fn default_concurrent_sessions() -> u64 {
    2
}
fn default_lease_minutes() -> u64 {
    60
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DeviceHostConfig {
    SshDeviceHostConfig(SshDeviceHostConfig),
    EasDeviceHostConfig(EasDeviceHostConfig),
    SimlockDeviceHostConfig(SimlockDeviceHostConfig),
}
pub type SshDeviceHostConfigs = Vec<SshDeviceHostConfig>;
pub type EasDeviceHostConfigs = Vec<EasDeviceHostConfig>;
pub type SimlockDeviceHostConfigs = Vec<SimlockDeviceHostConfig>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEasAccountInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub access_token: Option<Option<SettingsSecretValue>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceEasCredentialSource {
    #[serde(rename = "settings")]
    Settings,
    #[serde(rename = "env")]
    Env,
    #[serde(rename = "eas-login")]
    EasLogin,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEasApp {
    pub id: String,
    pub full_name: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEasAccount {
    pub signed_in_as: String,
    pub credential_source: DeviceEasCredentialSource,
    pub apps: Vec<DeviceEasApp>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSimlockIdentityInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub gateway_url: Option<Option<crate::base_schemas::TrimmedString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub agent_token: Option<Option<SettingsSecretValue>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceSimlockTokenSource {
    #[serde(rename = "entered")]
    Entered,
    #[serde(rename = "settings")]
    Settings,
    #[serde(rename = "env")]
    Env,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSimlockIdentity {
    pub gateway_url: String,
    pub signed_in_as: String,
    pub token_source: DeviceSimlockTokenSource,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceSimlockIdentityErrorTag {
    #[serde(rename = "DeviceSimlockIdentityError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSimlockIdentityError {
    #[serde(rename = "_tag")]
    pub _tag: DeviceSimlockIdentityErrorTag,
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceEasAccountErrorTag {
    #[serde(rename = "DeviceEasAccountError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEasAccountError {
    #[serde(rename = "_tag")]
    pub _tag: DeviceEasAccountErrorTag,
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSummary {
    pub host_id: DeviceHostId,
    pub id: DeviceId,
    pub platform: DevicePlatform,
    pub name: TrimmedNonEmptyString,
    pub version: String,
    pub booted: bool,
    pub physical: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicePlatformAvailability {
    pub platform: DevicePlatform,
    pub available: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reason: Option<Option<String>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceToolVersion {
    pub required_version: String,
    pub installed_versions: Vec<String>,
    #[serde(with = "crate::wire::nullable")]
    pub running_version: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceToolVersions {
    pub hub: DeviceToolVersion,
    pub agent: DeviceToolVersion,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceHostKind {
    #[serde(rename = "local")]
    Local,
    #[serde(rename = "ssh")]
    Ssh,
    #[serde(rename = "eas")]
    Eas,
    #[serde(rename = "simlock")]
    Simlock,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceHostSummary {
    pub id: DeviceHostId,
    pub label: TrimmedNonEmptyString,
    pub platforms: Vec<DevicePlatformAvailability>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tools: Option<Option<DeviceToolVersions>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_inspection_error: Option<Option<String>>,
    pub hub_installed: bool,
    pub agent_device_installed: bool,
    pub kind: DeviceHostKind,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceHostStatus {
    #[serde(rename = "disabled")]
    Disabled,
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "installing")]
    Installing,
    #[serde(rename = "starting")]
    Starting,
    #[serde(rename = "ready")]
    Ready,
    #[serde(rename = "failed")]
    Failed,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSession {
    pub thread_id: ThreadId,
    pub host_id: DeviceHostId,
    pub device_id: DeviceId,
    pub platform: DevicePlatform,
    pub opened_at: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceHostStatusDetail {
    pub status: DeviceHostStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub detail: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub android_discovery_incomplete: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub incomplete_platforms: Option<Option<Vec<DevicePlatform>>>,
}
pub type DeviceHostStatuses = BTreeMap<DeviceHostId, DeviceHostStatusDetail>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootingDevice {
    pub host_id: DeviceHostId,
    pub id: DeviceId,
    pub platform: DevicePlatform,
    pub name: TrimmedNonEmptyString,
    pub version: String,
    pub booted: bool,
    pub physical: bool,
    pub thread_id: ThreadId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceServiceState {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub supports_host_retry: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub supports_tool_update: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub supports_tool_inspection: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub supports_eas_hosts: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub supports_simlock_hosts: Option<Option<bool>>,
    pub host_status: DeviceHostStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_status_detail: Option<Option<String>>,
    pub host_statuses: DeviceHostStatuses,
    pub devices: Vec<DeviceSummary>,
    pub sessions: Vec<DeviceSession>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub booting_devices: Option<Option<Vec<BootingDevice>>>,
    pub onboarding_completed: bool,
    pub agent_access_enabled: bool,
    pub hub_base_path: String,
    pub revision: i64,
    pub hosts: Vec<DeviceHostSummary>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub eas_hosts: Option<Option<Vec<DeviceHostSummary>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub simlock_hosts: Option<Option<Vec<DeviceHostSummary>>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceUpdateTool {
    #[serde(rename = "hub")]
    Hub,
    #[serde(rename = "agent")]
    Agent,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceListInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub update_tool: Option<Option<DeviceUpdateTool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub inspect_only: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub retry_host_id: Option<Option<DeviceHostId>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceConfigureInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub enabled: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub agent_access_enabled: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub onboarding_completed: Option<Option<bool>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceOpenInput {
    pub thread_id: ThreadId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    pub device_id: DeviceId,
    pub platform: DevicePlatform,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub boot: Option<Option<bool>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCloseInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    pub thread_id: ThreadId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub device_id: Option<Option<DeviceId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub shutdown: Option<Option<bool>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceShutdownInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    pub device_id: DeviceId,
    pub platform: DevicePlatform,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceAppearance {
    #[serde(rename = "light")]
    Light,
    #[serde(rename = "dark")]
    Dark,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceTextSize {
    #[serde(rename = "small")]
    Small,
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "large")]
    Large,
    #[serde(rename = "extra-large")]
    ExtraLarge,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceColorFilter {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "grayscale")]
    Grayscale,
    #[serde(rename = "red-green")]
    RedGreen,
    #[serde(rename = "green-red")]
    GreenRed,
    #[serde(rename = "blue-yellow")]
    BlueYellow,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceOrientation {
    #[serde(rename = "portrait")]
    Portrait,
    #[serde(rename = "landscape_left")]
    LandscapeLeft,
    #[serde(rename = "portrait_upside_down")]
    PortraitUpsideDown,
    #[serde(rename = "landscape_right")]
    LandscapeRight,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceLiquidGlass {
    #[serde(rename = "clear")]
    Clear,
    #[serde(rename = "tinted")]
    Tinted,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLocation {
    pub latitude: SpeechNumber,
    pub longitude: SpeechNumber,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSettings {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub appearance: Option<Option<DeviceAppearance>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub text_size: Option<Option<DeviceTextSize>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reduce_motion: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub increase_contrast: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reduce_transparency: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub show_borders: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub voice_over: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub liquid_glass: Option<Option<DeviceLiquidGlass>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub color_filter: Option<Option<DeviceColorFilter>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub network_enabled: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub location: Option<Option<DeviceLocation>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceForegroundApp {
    pub id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub name: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub version: Option<Option<String>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceDetail {
    pub host_id: DeviceHostId,
    pub device_id: DeviceId,
    pub settings: DeviceSettings,
    #[serde(with = "crate::wire::nullable")]
    pub foreground_app: Option<DeviceForegroundApp>,
    pub read_at: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DevicePermission {
    #[serde(rename = "camera")]
    Camera,
    #[serde(rename = "microphone")]
    Microphone,
    #[serde(rename = "photos")]
    Photos,
    #[serde(rename = "contacts")]
    Contacts,
    #[serde(rename = "calendar")]
    Calendar,
    #[serde(rename = "reminders")]
    Reminders,
    #[serde(rename = "location")]
    Location,
    #[serde(rename = "notifications")]
    Notifications,
    #[serde(rename = "motion")]
    Motion,
    #[serde(rename = "media-library")]
    MediaLibrary,
    #[serde(rename = "faceid")]
    Faceid,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceToggleSetting {
    #[serde(rename = "reduceMotion")]
    ReduceMotion,
    #[serde(rename = "increaseContrast")]
    IncreaseContrast,
    #[serde(rename = "reduceTransparency")]
    ReduceTransparency,
    #[serde(rename = "showBorders")]
    ShowBorders,
    #[serde(rename = "voiceOver")]
    VoiceOver,
    #[serde(rename = "networkEnabled")]
    NetworkEnabled,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DevicePermissionDecision {
    #[serde(rename = "grant")]
    Grant,
    #[serde(rename = "revoke")]
    Revoke,
    #[serde(rename = "reset")]
    Reset,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DevicePushPayload {
    String(String),
    DevicePushObject(DevicePushObject),
}
pub type DevicePushObject = BTreeMap<String, Value>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceSetAppearanceActionType {
    #[serde(rename = "setAppearance")]
    SetAppearance,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSetAppearanceAction {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    pub device_id: DeviceId,
    pub r#type: DeviceSetAppearanceActionType,
    pub value: DeviceAppearance,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceSetTextSizeActionType {
    #[serde(rename = "setTextSize")]
    SetTextSize,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSetTextSizeAction {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    pub device_id: DeviceId,
    pub r#type: DeviceSetTextSizeActionType,
    pub value: DeviceTextSize,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceSetToggleActionType {
    #[serde(rename = "setToggle")]
    SetToggle,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSetToggleAction {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    pub device_id: DeviceId,
    pub r#type: DeviceSetToggleActionType,
    pub setting: DeviceToggleSetting,
    pub value: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceSetLiquidGlassActionType {
    #[serde(rename = "setLiquidGlass")]
    SetLiquidGlass,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSetLiquidGlassAction {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    pub device_id: DeviceId,
    pub r#type: DeviceSetLiquidGlassActionType,
    pub value: DeviceLiquidGlass,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceSetColorFilterActionType {
    #[serde(rename = "setColorFilter")]
    SetColorFilter,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSetColorFilterAction {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    pub device_id: DeviceId,
    pub r#type: DeviceSetColorFilterActionType,
    pub value: DeviceColorFilter,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceSetOrientationActionType {
    #[serde(rename = "setOrientation")]
    SetOrientation,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSetOrientationAction {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    pub device_id: DeviceId,
    pub r#type: DeviceSetOrientationActionType,
    pub value: DeviceOrientation,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceSetLocationActionType {
    #[serde(rename = "setLocation")]
    SetLocation,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSetLocationAction {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    pub device_id: DeviceId,
    pub r#type: DeviceSetLocationActionType,
    pub latitude: SpeechNumber,
    pub longitude: SpeechNumber,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceClearLocationActionType {
    #[serde(rename = "clearLocation")]
    ClearLocation,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceClearLocationAction {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    pub device_id: DeviceId,
    pub r#type: DeviceClearLocationActionType,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceSetPermissionActionType {
    #[serde(rename = "setPermission")]
    SetPermission,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSetPermissionAction {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    pub device_id: DeviceId,
    pub r#type: DeviceSetPermissionActionType,
    pub app_id: TrimmedNonEmptyString,
    pub permission: DevicePermission,
    pub decision: DevicePermissionDecision,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceOpenUrlActionType {
    #[serde(rename = "openUrl")]
    OpenUrl,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceOpenUrlAction {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    pub device_id: DeviceId,
    pub r#type: DeviceOpenUrlActionType,
    pub url: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceLaunchAppActionType {
    #[serde(rename = "launchApp")]
    LaunchApp,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLaunchAppAction {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    pub device_id: DeviceId,
    pub r#type: DeviceLaunchAppActionType,
    pub app_id: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceTerminateAppActionType {
    #[serde(rename = "terminateApp")]
    TerminateApp,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceTerminateAppAction {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    pub device_id: DeviceId,
    pub r#type: DeviceTerminateAppActionType,
    pub app_id: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceShakeActionType {
    #[serde(rename = "shake")]
    Shake,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceShakeAction {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    pub device_id: DeviceId,
    pub r#type: DeviceShakeActionType,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceSendPushActionType {
    #[serde(rename = "sendPush")]
    SendPush,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSendPushAction {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    pub device_id: DeviceId,
    pub r#type: DeviceSendPushActionType,
    pub app_id: TrimmedNonEmptyString,
    pub payload: DevicePushPayload,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DeviceActionInput {
    DeviceSetAppearanceAction(DeviceSetAppearanceAction),
    DeviceSetTextSizeAction(DeviceSetTextSizeAction),
    DeviceSetToggleAction(DeviceSetToggleAction),
    DeviceSetLiquidGlassAction(DeviceSetLiquidGlassAction),
    DeviceSetColorFilterAction(DeviceSetColorFilterAction),
    DeviceSetOrientationAction(DeviceSetOrientationAction),
    DeviceSetLocationAction(DeviceSetLocationAction),
    DeviceClearLocationAction(DeviceClearLocationAction),
    DeviceSetPermissionAction(DeviceSetPermissionAction),
    DeviceOpenUrlAction(DeviceOpenUrlAction),
    DeviceLaunchAppAction(DeviceLaunchAppAction),
    DeviceTerminateAppAction(DeviceTerminateAppAction),
    DeviceShakeAction(DeviceShakeAction),
    DeviceSendPushAction(DeviceSendPushAction),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceDetailInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    pub device_id: DeviceId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceHostUnavailableErrorTag {
    #[serde(rename = "DeviceHostUnavailableError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceHostUnavailableError {
    #[serde(rename = "_tag")]
    pub _tag: DeviceHostUnavailableErrorTag,
    pub host_id: DeviceHostId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_label: Option<Option<String>>,
    pub reason: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<Value>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DevicePlatformUnavailableErrorTag {
    #[serde(rename = "DevicePlatformUnavailableError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicePlatformUnavailableError {
    #[serde(rename = "_tag")]
    pub _tag: DevicePlatformUnavailableErrorTag,
    pub host_id: DeviceHostId,
    pub platform: DevicePlatform,
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceNotFoundErrorTag {
    #[serde(rename = "DeviceNotFoundError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceNotFoundError {
    #[serde(rename = "_tag")]
    pub _tag: DeviceNotFoundErrorTag,
    pub host_id: DeviceHostId,
    pub device_id: DeviceId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceBootErrorReason {
    #[serde(rename = "disk_space")]
    DiskSpace,
    #[serde(rename = "timeout")]
    Timeout,
    #[serde(rename = "launch_failed")]
    LaunchFailed,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceBootErrorTag {
    #[serde(rename = "DeviceBootError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceBootError {
    #[serde(rename = "_tag")]
    pub _tag: DeviceBootErrorTag,
    pub host_id: DeviceHostId,
    pub device_id: DeviceId,
    pub reason: DeviceBootErrorReason,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub diagnostics: Option<Option<String>>,
    pub cause: Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceOperationErrorReason {
    #[serde(rename = "command_failed")]
    CommandFailed,
    #[serde(rename = "request_failed")]
    RequestFailed,
    #[serde(rename = "invalid_payload")]
    InvalidPayload,
    #[serde(rename = "settings_failed")]
    SettingsFailed,
    #[serde(rename = "hub_rejected")]
    HubRejected,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceOperationErrorTag {
    #[serde(rename = "DeviceOperationError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceOperationError {
    #[serde(rename = "_tag")]
    pub _tag: DeviceOperationErrorTag,
    pub operation: String,
    pub reason: DeviceOperationErrorReason,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub exit_code: Option<Option<SpeechNumber>>,
    pub cause: Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceActionUnavailableErrorReason {
    #[serde(rename = "unsupported")]
    Unsupported,
    #[serde(rename = "helper_missing")]
    HelperMissing,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceActionUnavailableErrorTag {
    #[serde(rename = "DeviceActionUnavailableError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceActionUnavailableError {
    #[serde(rename = "_tag")]
    pub _tag: DeviceActionUnavailableErrorTag,
    pub operation: String,
    pub platform: DevicePlatform,
    pub reason: DeviceActionUnavailableErrorReason,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DeviceError {
    DeviceHostUnavailableError(DeviceHostUnavailableError),
    DevicePlatformUnavailableError(DevicePlatformUnavailableError),
    DeviceNotFoundError(DeviceNotFoundError),
    DeviceBootError(DeviceBootError),
    DeviceOperationError(DeviceOperationError),
    DeviceActionUnavailableError(DeviceActionUnavailableError),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceToolOpenSession {
    pub host_id: DeviceHostId,
    pub device_id: DeviceId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceToolListResult {
    pub host_statuses: DeviceHostStatuses,
    pub hosts: Vec<DeviceHostSummary>,
    pub devices: Vec<DeviceSummary>,
    pub open: Vec<DeviceToolOpenSession>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceToolOpenInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub device_id: Option<Option<DeviceId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub platform: Option<Option<DevicePlatform>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAgentCommand {
    pub command: String,
    pub target_args: Vec<String>,
    pub session: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceToolOpenResult {
    pub device: DeviceSummary,
    pub agent_device: DeviceAgentCommand,
    pub quick_start: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceToolTargetInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub device_id: Option<Option<DeviceId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceScreenshotMimeType {
    #[serde(rename = "image/png")]
    Png,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceScreenshot {
    pub mime_type: DeviceScreenshotMimeType,
    pub data: String,
    pub width: i64,
    pub height: i64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceToolScreenshotResult {
    pub device: DeviceSummary,
    pub screenshot: DeviceScreenshot,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceToolCloseInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub device_id: Option<Option<DeviceId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host_id: Option<Option<DeviceHostId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub shutdown: Option<Option<bool>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceToolUnavailableErrorTag {
    #[serde(rename = "DeviceToolUnavailableError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceToolUnavailableError {
    #[serde(rename = "_tag")]
    pub _tag: DeviceToolUnavailableErrorTag,
    pub reason: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<Value>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DeviceToolError {
    DeviceToolUnavailableError(DeviceToolUnavailableError),
    DeviceHostUnavailableError(DeviceHostUnavailableError),
    DevicePlatformUnavailableError(DevicePlatformUnavailableError),
    DeviceNotFoundError(DeviceNotFoundError),
    DeviceBootError(DeviceBootError),
    DeviceOperationError(DeviceOperationError),
    DeviceActionUnavailableError(DeviceActionUnavailableError),
}
