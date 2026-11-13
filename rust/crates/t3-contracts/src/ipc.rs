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
#[serde(rename_all = "camelCase")]
pub struct ContextMenuItemSchema {
    #[serde()]
    pub id: String,
    #[serde()]
    pub label: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub detail: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub destructive: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub disabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub header: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub icon: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub separator_before: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub checked: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub children: Option<Vec<ContextMenuItemSchema>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopAppStageLabelSchema {
    #[serde(rename = "Alpha")]
    Alpha,
    #[serde(rename = "Beta")]
    Beta,
    #[serde(rename = "Dev")]
    Dev,
    #[serde(rename = "Nightly")]
    Nightly,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAppBrandingSchema {
    #[serde()]
    pub base_name: String,
    #[serde()]
    pub stage_label: DesktopAppStageLabelSchema,
    #[serde()]
    pub display_name: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopCaptureConfigApplied {
    #[serde(with = "crate::wire::nullable")]
    pub backup_path: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub warning: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopCaptureConfigPreviewOperation {
    #[serde(rename = "install")]
    Install,
    #[serde(rename = "remove")]
    Remove,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopCaptureConfigPreview {
    #[serde()]
    pub id: String,
    #[serde()]
    pub path: String,
    #[serde()]
    pub resolved_path: String,
    #[serde()]
    pub before: String,
    #[serde()]
    pub after: String,
    #[serde()]
    pub shortcut: String,
    #[serde()]
    pub operation: DesktopCaptureConfigPreviewOperation,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopCaptureConfigRequestOperation {
    #[serde(rename = "install")]
    Install,
    #[serde(rename = "remove")]
    Remove,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopCaptureConfigRequest {
    #[serde()]
    pub operation: DesktopCaptureConfigRequestOperation,
    #[serde()]
    pub choose_file: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub shortcut: Option<Option<String>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopCaptureExtensionStateStatus {
    #[serde(rename = "not-installed")]
    NotInstalled,
    #[serde(rename = "disabled")]
    Disabled,
    #[serde(rename = "enabled")]
    Enabled,
    #[serde(rename = "restart-required")]
    RestartRequired,
    #[serde(rename = "update-required")]
    UpdateRequired,
    #[serde(rename = "extensions-disabled")]
    ExtensionsDisabled,
    #[serde(rename = "unsupported")]
    Unsupported,
    #[serde(rename = "error")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopCaptureExtensionState {
    #[serde()]
    pub status: DesktopCaptureExtensionStateStatus,
    #[serde()]
    pub message: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopCaptureHelperStateStatus {
    #[serde(rename = "not-installed")]
    NotInstalled,
    #[serde(rename = "update-required")]
    UpdateRequired,
    #[serde(rename = "ready")]
    Ready,
    #[serde(rename = "error")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopCaptureHelperState {
    #[serde()]
    pub status: DesktopCaptureHelperStateStatus,
    #[serde()]
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub feedback_available: Option<Option<bool>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopCliCommandStateSchemaPathSetupValue {
    #[serde()]
    pub file: String,
    #[serde()]
    pub added: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopCliCommandStateSchema {
    #[serde()]
    pub supported: bool,
    #[serde(with = "crate::wire::nullable")]
    pub installed_path: Option<String>,
    #[serde()]
    pub on_path: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub shadowed_by: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub path_setup: Option<DesktopCliCommandStateSchemaPathSetupValue>,
    #[serde(with = "crate::wire::nullable")]
    pub launcher_path: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopSshHostSourceSchema {
    #[serde(rename = "ssh-config")]
    SshConfig,
    #[serde(rename = "known-hosts")]
    KnownHosts,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopDiscoveredSshHostSchema {
    #[serde()]
    pub alias: String,
    #[serde()]
    pub hostname: String,
    #[serde(with = "crate::wire::nullable")]
    pub username: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub port: Option<crate::settings::WireNumber>,
    #[serde()]
    pub source: DesktopSshHostSourceSchema,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopEnvironmentBootstrapSchema {
    #[serde()]
    pub id: String,
    #[serde()]
    pub label: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub running_distro: Option<Option<String>>,
    #[serde(with = "crate::wire::nullable")]
    pub http_base_url: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub ws_base_url: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub bootstrap_token: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopMediaReadSchema {
    #[serde()]
    pub status: crate::settings::WireNumber,
    #[serde()]
    pub mime_type: String,
    #[serde()]
    pub data: String,
}
pub type DesktopSnapShotId = crate::base_schemas::TrimmedNonEmptyString;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopPendingSnapShotMimeType {
    #[serde(rename = "image/png")]
    ImagePng,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPendingSnapShot {
    #[serde()]
    pub id: DesktopSnapShotId,
    #[serde()]
    pub name: String,
    #[serde()]
    pub mime_type: DesktopPendingSnapShotMimeType,
    #[serde()]
    pub size_bytes: i64,
    #[serde()]
    pub source: crate::chat_attachment::SnapShotSource,
}
pub type DesktopPreviewTabIdSchema = String;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewAnnotationSendEnabledInputSchema {
    #[serde()]
    pub tab_id: DesktopPreviewTabIdSchema,
    #[serde()]
    pub enabled: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopPreviewAnnotationThemeSchemaColorScheme {
    #[serde(rename = "light")]
    Light,
    #[serde(rename = "dark")]
    Dark,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewAnnotationThemeSchema {
    #[serde()]
    pub color_scheme: DesktopPreviewAnnotationThemeSchemaColorScheme,
    #[serde()]
    pub radius: String,
    #[serde()]
    pub background: String,
    #[serde()]
    pub foreground: String,
    #[serde()]
    pub popover: String,
    #[serde()]
    pub popover_foreground: String,
    #[serde()]
    pub primary: String,
    #[serde()]
    pub primary_foreground: String,
    #[serde()]
    pub muted: String,
    #[serde()]
    pub muted_foreground: String,
    #[serde()]
    pub accent: String,
    #[serde()]
    pub accent_foreground: String,
    #[serde()]
    pub border: String,
    #[serde()]
    pub input: String,
    #[serde()]
    pub ring: String,
    #[serde()]
    pub font_sans: String,
    #[serde()]
    pub font_mono: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewAnnotationThemeInputSchema {
    #[serde()]
    pub theme: DesktopPreviewAnnotationThemeSchema,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewAnnotationVoiceConfigSchema {
    #[serde()]
    pub session_id: String,
    #[serde()]
    pub shortcuts: Vec<crate::keybindings::KeybindingShortcut>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopPreviewAnnotationVoiceEventSchemaAction {
    #[serde(rename = "sync")]
    Sync,
    #[serde(rename = "start")]
    Start,
    #[serde(rename = "stop")]
    Stop,
    #[serde(rename = "cancel")]
    Cancel,
    #[serde(rename = "skip")]
    Skip,
    #[serde(rename = "settings")]
    Settings,
    #[serde(rename = "key")]
    Key,
    #[serde(rename = "focus")]
    Focus,
    #[serde(rename = "blur")]
    Blur,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopPreviewAnnotationVoiceEventSchemaKeyboardValueType {
    #[serde(rename = "keydown")]
    Keydown,
    #[serde(rename = "keyup")]
    Keyup,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewAnnotationVoiceEventSchemaKeyboardValue {
    #[serde()]
    pub r#type: DesktopPreviewAnnotationVoiceEventSchemaKeyboardValueType,
    #[serde()]
    pub key: String,
    #[serde()]
    pub code: String,
    #[serde()]
    pub meta_key: bool,
    #[serde()]
    pub ctrl_key: bool,
    #[serde()]
    pub shift_key: bool,
    #[serde()]
    pub alt_key: bool,
    #[serde()]
    pub repeat: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewAnnotationVoiceEventSchema {
    #[serde()]
    pub session_id: String,
    #[serde()]
    pub text: String,
    #[serde()]
    pub cursor: i64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub selection_end: Option<Option<i64>>,
    #[serde()]
    pub action: DesktopPreviewAnnotationVoiceEventSchemaAction,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub keyboard: Option<Option<DesktopPreviewAnnotationVoiceEventSchemaKeyboardValue>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopPreviewAnnotationVoiceStateSchemaPhase {
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "preparing")]
    Preparing,
    #[serde(rename = "recording")]
    Recording,
    #[serde(rename = "transcribing")]
    Transcribing,
    #[serde(rename = "post-processing")]
    PostProcessing,
    #[serde(rename = "error")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopPreviewAnnotationVoiceStateSchemaErrorActionValue {
    #[serde(rename = "retry")]
    Retry,
    #[serde(rename = "settings")]
    Settings,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewAnnotationVoiceStateSchemaDraftValue {
    #[serde()]
    pub text: String,
    #[serde()]
    pub cursor: i64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewAnnotationVoiceStateSchema {
    #[serde()]
    pub tab_id: DesktopPreviewTabIdSchema,
    #[serde()]
    pub session_id: String,
    #[serde()]
    pub available: bool,
    #[serde()]
    pub phase: DesktopPreviewAnnotationVoiceStateSchemaPhase,
    #[serde(with = "crate::wire::nullable")]
    pub status: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub error_action: Option<DesktopPreviewAnnotationVoiceStateSchemaErrorActionValue>,
    #[serde(with = "crate::wire::nullable")]
    pub preview: Option<String>,
    #[serde()]
    pub level: crate::settings::WireNumber,
    #[serde(with = "crate::wire::nullable")]
    pub shortcut_label: Option<String>,
    #[serde()]
    pub freezes_editor: bool,
    #[serde()]
    pub blocks_submission: bool,
    #[serde(with = "crate::wire::nullable")]
    pub draft: Option<DesktopPreviewAnnotationVoiceStateSchemaDraftValue>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewArtifactInputSchema {
    #[serde()]
    pub path: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewClearDataInputSchema {
    #[serde()]
    pub environment_id: crate::base_schemas::EnvironmentId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub profile_id: Option<Option<crate::browser_profile::BrowserProfileId>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopPreviewColorSchemeSchema {
    #[serde(rename = "system")]
    System,
    #[serde(rename = "light")]
    Light,
    #[serde(rename = "dark")]
    Dark,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewConfigInputSchema {
    #[serde()]
    pub environment_id: crate::base_schemas::EnvironmentId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub profile_id: Option<Option<crate::browser_profile::BrowserProfileId>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewCreateTabInputSchemaServerTabValue {
    #[serde()]
    pub thread_id: crate::base_schemas::TrimmedNonEmptyString,
    #[serde()]
    pub tab_id: crate::base_schemas::TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewCreateTabInputSchema {
    #[serde()]
    pub tab_id: DesktopPreviewTabIdSchema,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub zoom_factor: Option<Option<crate::settings::WireNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub color_scheme: Option<Option<DesktopPreviewColorSchemeSchema>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub server_tab: Option<Option<DesktopPreviewCreateTabInputSchemaServerTabValue>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewFindInPageInputSchema {
    #[serde()]
    pub tab_id: DesktopPreviewTabIdSchema,
    #[serde()]
    pub text: String,
    #[serde()]
    pub forward: bool,
    #[serde()]
    pub new_session: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewNavigateInputSchema {
    #[serde()]
    pub tab_id: DesktopPreviewTabIdSchema,
    #[serde()]
    pub url: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewPickElementInputSchema {
    #[serde()]
    pub tab_id: DesktopPreviewTabIdSchema,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub voice: Option<Option<DesktopPreviewAnnotationVoiceConfigSchema>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewRecordingArtifactSchema {
    #[serde()]
    pub id: String,
    #[serde()]
    pub tab_id: DesktopPreviewTabIdSchema,
    #[serde()]
    pub path: String,
    #[serde()]
    pub mime_type: String,
    #[serde()]
    pub size_bytes: i64,
    #[serde()]
    pub created_at: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopPreviewRecordingInputSchemaMember1Type {
    #[serde(rename = "pointer")]
    Pointer,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopPreviewRecordingInputSchemaMember1Phase {
    #[serde(rename = "move")]
    Move,
    #[serde(rename = "down")]
    Down,
    #[serde(rename = "up")]
    Up,
    #[serde(rename = "click")]
    Click,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewRecordingInputSchemaMember1 {
    #[serde()]
    pub r#type: DesktopPreviewRecordingInputSchemaMember1Type,
    #[serde()]
    pub phase: DesktopPreviewRecordingInputSchemaMember1Phase,
    #[serde()]
    pub x: crate::wire::JsNumber,
    #[serde()]
    pub y: crate::wire::JsNumber,
    #[serde()]
    pub width: crate::wire::JsNumber,
    #[serde()]
    pub height: crate::wire::JsNumber,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopPreviewRecordingInputSchemaMember2Type {
    #[serde(rename = "key")]
    Key,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewRecordingInputSchemaMember2 {
    #[serde()]
    pub r#type: DesktopPreviewRecordingInputSchemaMember2Type,
    #[serde(with = "crate::wire::nullable")]
    pub label: Option<String>,
    #[serde()]
    pub held: bool,
    #[serde()]
    pub width: crate::wire::JsNumber,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopPreviewRecordingInputSchemaMember3Type {
    #[serde(rename = "clear")]
    Clear,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewRecordingInputSchemaMember3 {
    #[serde()]
    pub r#type: DesktopPreviewRecordingInputSchemaMember3Type,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DesktopPreviewRecordingInputSchema {
    Pointer1(Box<DesktopPreviewRecordingInputSchemaMember1>),
    Key2(Box<DesktopPreviewRecordingInputSchemaMember2>),
    Clear3(Box<DesktopPreviewRecordingInputSchemaMember3>),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewRecordingSaveInputSchema {
    #[serde()]
    pub tab_id: DesktopPreviewTabIdSchema,
    #[serde()]
    pub mime_type: String,
    #[serde()]
    pub data: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewRegisterWebviewInputSchema {
    #[serde()]
    pub tab_id: DesktopPreviewTabIdSchema,
    #[serde()]
    pub web_contents_id: i64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopPreviewScreenshotArtifactSchemaMimeType {
    #[serde(rename = "image/png")]
    ImagePng,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewScreenshotArtifactSchema {
    #[serde()]
    pub id: String,
    #[serde()]
    pub tab_id: DesktopPreviewTabIdSchema,
    #[serde()]
    pub path: String,
    #[serde()]
    pub mime_type: DesktopPreviewScreenshotArtifactSchemaMimeType,
    #[serde()]
    pub size_bytes: i64,
    #[serde()]
    pub created_at: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewSetAudioMutedInputSchema {
    #[serde()]
    pub tab_id: DesktopPreviewTabIdSchema,
    #[serde()]
    pub audio_muted: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewSetColorSchemeInputSchema {
    #[serde()]
    pub tab_id: DesktopPreviewTabIdSchema,
    #[serde()]
    pub color_scheme: DesktopPreviewColorSchemeSchema,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewSetShownInputSchema {
    #[serde()]
    pub tab_id: DesktopPreviewTabIdSchema,
    #[serde()]
    pub shown: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewSetZoomFactorInputSchema {
    #[serde()]
    pub tab_id: DesktopPreviewTabIdSchema,
    #[serde()]
    pub zoom_factor: crate::settings::WireNumber,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewTabInputSchema {
    #[serde()]
    pub tab_id: DesktopPreviewTabIdSchema,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewWebviewConfigSchema {
    #[serde()]
    pub partition: String,
    #[serde()]
    pub web_preferences: String,
    #[serde(with = "crate::wire::nullable")]
    pub preload_url: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopRuntimeArchSchema {
    #[serde(rename = "arm64")]
    Arm64,
    #[serde(rename = "x64")]
    X64,
    #[serde(rename = "other")]
    Other,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopServerExposureModeSchema {
    #[serde(rename = "local-only")]
    LocalOnly,
    #[serde(rename = "network-accessible")]
    NetworkAccessible,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopServerExposureStateSchema {
    #[serde()]
    pub mode: DesktopServerExposureModeSchema,
    #[serde(with = "crate::wire::nullable")]
    pub endpoint_url: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub advertised_host: Option<String>,
    #[serde()]
    pub tailscale_serve_enabled: bool,
    #[serde()]
    pub tailscale_serve_port: crate::settings::WireNumber,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopSnapShotMimeType {
    #[serde(rename = "image/png")]
    ImagePng,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSnapShot {
    #[serde()]
    pub id: DesktopSnapShotId,
    #[serde()]
    pub name: String,
    #[serde()]
    pub mime_type: DesktopSnapShotMimeType,
    #[serde()]
    pub size_bytes: i64,
    #[serde()]
    pub source: crate::chat_attachment::SnapShotSource,
    #[serde()]
    pub data_url: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSnapShotAnimationDestinationViewportFrame {
    #[serde()]
    pub x: crate::settings::WireNumber,
    #[serde()]
    pub y: crate::settings::WireNumber,
    #[serde()]
    pub width: crate::settings::WireNumber,
    #[serde()]
    pub height: crate::settings::WireNumber,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSnapShotAnimationDestinationDetailsValue {
    #[serde()]
    pub app_name: crate::base_schemas::TrimmedNonEmptyString,
    #[serde()]
    pub window_title: crate::base_schemas::TrimmedString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub app_icon_data_url: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSnapShotAnimationDestination {
    #[serde()]
    pub id: DesktopSnapShotId,
    #[serde()]
    pub viewport_frame: DesktopSnapShotAnimationDestinationViewportFrame,
    #[serde()]
    pub background_color: String,
    #[serde()]
    pub border_color: String,
    #[serde()]
    pub border_width: crate::settings::WireNumber,
    #[serde()]
    pub corner_radius: crate::settings::WireNumber,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub details: Option<Option<DesktopSnapShotAnimationDestinationDetailsValue>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopSnapShotEventMember1Type {
    #[serde(rename = "requested")]
    Requested,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSnapShotEventMember1 {
    #[serde()]
    pub r#type: DesktopSnapShotEventMember1Type,
    #[serde()]
    pub id: DesktopSnapShotId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopSnapShotEventMember2Type {
    #[serde(rename = "started")]
    Started,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSnapShotEventMember2 {
    #[serde()]
    pub r#type: DesktopSnapShotEventMember2Type,
    #[serde()]
    pub id: DesktopSnapShotId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopSnapShotEventMember3Type {
    #[serde(rename = "ready")]
    Ready,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSnapShotEventMember3 {
    #[serde()]
    pub r#type: DesktopSnapShotEventMember3Type,
    #[serde()]
    pub id: DesktopSnapShotId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopSnapShotEventMember4Type {
    #[serde(rename = "failed")]
    Failed,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSnapShotEventMember4 {
    #[serde()]
    pub r#type: DesktopSnapShotEventMember4Type,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub id: Option<Option<DesktopSnapShotId>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopSnapShotEventMember5Type {
    #[serde(rename = "shortcut-changed")]
    ShortcutChanged,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSnapShotEventMember5 {
    #[serde()]
    pub r#type: DesktopSnapShotEventMember5Type,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DesktopSnapShotEvent {
    Requested1(Box<DesktopSnapShotEventMember1>),
    Started2(Box<DesktopSnapShotEventMember2>),
    Ready3(Box<DesktopSnapShotEventMember3>),
    Failed4(Box<DesktopSnapShotEventMember4>),
    ShortcutChanged5(Box<DesktopSnapShotEventMember5>),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopSnapShotMode {
    #[serde(rename = "direct")]
    Direct,
    #[serde(rename = "portal")]
    Portal,
    #[serde(rename = "unavailable")]
    Unavailable,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopSnapShotSetupAction {
    #[serde(rename = "install-extension")]
    InstallExtension,
    #[serde(rename = "enable-extension")]
    EnableExtension,
    #[serde(rename = "disable-extension")]
    DisableExtension,
    #[serde(rename = "install-kde-helper")]
    InstallKdeHelper,
    #[serde(rename = "remove-kde-helper")]
    RemoveKdeHelper,
    #[serde(rename = "install-hyprland-helper")]
    InstallHyprlandHelper,
    #[serde(rename = "remove-hyprland-helper")]
    RemoveHyprlandHelper,
    #[serde(rename = "test-mac-capture")]
    TestMacCapture,
    #[serde(rename = "allow-screen-recording")]
    AllowScreenRecording,
    #[serde(rename = "allow-accessibility")]
    AllowAccessibility,
    #[serde(rename = "retry-shortcut")]
    RetryShortcut,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSnapShotShortcutAvailability {
    #[serde()]
    pub available: bool,
    #[serde(with = "crate::wire::nullable")]
    pub message: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopSnapShotStateLinuxDesktopValue {
    #[serde(rename = "gnome")]
    Gnome,
    #[serde(rename = "kde")]
    Kde,
    #[serde(rename = "niri")]
    Niri,
    #[serde(rename = "hyprland")]
    Hyprland,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopSnapShotStateLinuxBackendValue {
    #[serde(rename = "screenshot-portal")]
    ScreenshotPortal,
    #[serde(rename = "gnome-extension")]
    GnomeExtension,
    #[serde(rename = "niri")]
    Niri,
    #[serde(rename = "kde")]
    Kde,
    #[serde(rename = "hyprland")]
    Hyprland,
    #[serde(rename = "picker")]
    Picker,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSnapShotStateMacPermissionsValue {
    #[serde()]
    pub screen_recording: bool,
    #[serde()]
    pub accessibility: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSnapShotState {
    #[serde()]
    pub mode: DesktopSnapShotMode,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub windows: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub linux_desktop: Option<Option<DesktopSnapShotStateLinuxDesktopValue>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub linux_backend: Option<Option<DesktopSnapShotStateLinuxBackendValue>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub linux_feedback_available: Option<Option<bool>>,
    #[serde()]
    pub shortcut: crate::settings::SnapShotShortcut,
    #[serde()]
    pub shortcut_registered: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub shortcut_pending: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub shortcut_can_retry: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub shortcut_label: Option<Option<String>>,
    #[serde(with = "crate::wire::nullable")]
    pub shortcut_message: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub shortcut_binding: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub shortcut_config_path: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub shortcut_action_registered: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub gnome_extension: Option<Option<DesktopCaptureExtensionState>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub kde_helper: Option<Option<DesktopCaptureHelperState>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub hyprland_helper: Option<Option<DesktopCaptureHelperState>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub mac_permissions: Option<Option<DesktopSnapShotStateMacPermissionsValue>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub shortcut_verified: Option<Option<bool>>,
    #[serde(with = "crate::wire::nullable")]
    pub message: Option<String>,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSshBearerBootstrapInputSchema {
    #[serde()]
    pub http_base_url: String,
    #[serde()]
    pub credential: String,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSshBearerRequestInputSchema {
    #[serde()]
    pub http_base_url: String,
    #[serde()]
    pub bearer_token: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSshEnvironmentTargetSchema {
    #[serde()]
    pub alias: String,
    #[serde()]
    pub hostname: String,
    #[serde(with = "crate::wire::nullable")]
    pub username: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub port: Option<crate::settings::WireNumber>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopSshEnvironmentBootstrapSchemaRemoteServerKindValue {
    #[serde(rename = "external")]
    External,
    #[serde(rename = "managed")]
    Managed,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSshEnvironmentBootstrapSchema {
    #[serde()]
    pub target: DesktopSshEnvironmentTargetSchema,
    #[serde()]
    pub http_base_url: String,
    #[serde()]
    pub ws_base_url: String,
    #[serde(with = "crate::wire::nullable")]
    pub pairing_token: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub remote_port: Option<crate::settings::WireNumber>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub remote_server_kind: Option<DesktopSshEnvironmentBootstrapSchemaRemoteServerKindValue>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSshEnvironmentEnsureOptionsSchema {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub issue_pairing_token: Option<bool>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSshEnvironmentEnsureInputSchema {
    #[serde()]
    pub target: DesktopSshEnvironmentTargetSchema,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub options: Option<DesktopSshEnvironmentEnsureOptionsSchema>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopSshPasswordPromptCancelledResultSchemaType {
    #[serde(rename = "ssh-password-prompt-cancelled")]
    SshPasswordPromptCancelled,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSshPasswordPromptCancelledResultSchema {
    #[serde()]
    pub r#type: DesktopSshPasswordPromptCancelledResultSchemaType,
    #[serde()]
    pub message: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DesktopSshEnvironmentEnsureResultSchema {
    DesktopSshEnvironmentBootstrap(Box<DesktopSshEnvironmentBootstrapSchema>),
    DesktopSshPasswordPromptCancelledResult(Box<DesktopSshPasswordPromptCancelledResultSchema>),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSshHttpBaseUrlInputSchema {
    #[serde()]
    pub http_base_url: String,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSshPasswordPromptResolutionInputSchema {
    #[serde()]
    pub request_id: String,
    #[serde(with = "crate::wire::nullable")]
    pub password: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopThemeSchema {
    #[serde(rename = "light")]
    Light,
    #[serde(rename = "dark")]
    Dark,
    #[serde(rename = "system")]
    System,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopUpdateStatusSchema {
    #[serde(rename = "disabled")]
    Disabled,
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "checking")]
    Checking,
    #[serde(rename = "up-to-date")]
    UpToDate,
    #[serde(rename = "available")]
    Available,
    #[serde(rename = "downloading")]
    Downloading,
    #[serde(rename = "downloaded")]
    Downloaded,
    #[serde(rename = "error")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopUpdateChannelSchema {
    #[serde(rename = "latest")]
    Latest,
    #[serde(rename = "nightly")]
    Nightly,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopUpdateReleaseNoteSchema {
    #[serde()]
    pub version: String,
    #[serde()]
    pub items: Vec<String>,
    #[serde()]
    pub total_items: crate::settings::WireNumber,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub released_at: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub intro: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub stats: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub footer: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DesktopUpdateStateSchemaErrorContextValue {
    #[serde(rename = "check")]
    Check,
    #[serde(rename = "download")]
    Download,
    #[serde(rename = "install")]
    Install,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopUpdateStateSchema {
    #[serde()]
    pub enabled: bool,
    #[serde()]
    pub status: DesktopUpdateStatusSchema,
    #[serde()]
    pub channel: DesktopUpdateChannelSchema,
    #[serde()]
    pub current_version: String,
    #[serde()]
    pub host_arch: DesktopRuntimeArchSchema,
    #[serde()]
    pub app_arch: DesktopRuntimeArchSchema,
    #[serde()]
    pub running_under_arm64_translation: bool,
    #[serde(with = "crate::wire::nullable")]
    pub available_version: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub downloaded_version: Option<String>,
    #[serde()]
    pub release_notes: Vec<DesktopUpdateReleaseNoteSchema>,
    #[serde()]
    pub omitted_release_count: crate::settings::WireNumber,
    #[serde(with = "crate::wire::nullable")]
    pub download_percent: Option<crate::settings::WireNumber>,
    #[serde(with = "crate::wire::nullable")]
    pub checked_at: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub message: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub error_context: Option<DesktopUpdateStateSchemaErrorContextValue>,
    #[serde()]
    pub can_retry: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub not_applied_version: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopUpdateActionResultSchema {
    #[serde()]
    pub accepted: bool,
    #[serde()]
    pub completed: bool,
    #[serde()]
    pub state: DesktopUpdateStateSchema,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopUpdateCheckResultSchema {
    #[serde()]
    pub checked: bool,
    #[serde()]
    pub state: DesktopUpdateStateSchema,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DesktopWslDistroSchemaVersion {
    V1(WireLiteral<1>),
    V2(WireLiteral<2>),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopWslDistroSchema {
    #[serde()]
    pub name: String,
    #[serde()]
    pub is_default: bool,
    #[serde()]
    pub version: DesktopWslDistroSchemaVersion,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopWslStateSchema {
    #[serde()]
    pub enabled: bool,
    #[serde(with = "crate::wire::nullable")]
    pub distro: Option<String>,
    #[serde()]
    pub available: bool,
    #[serde()]
    pub wsl_only: bool,
    #[serde()]
    pub distros: Vec<DesktopWslDistroSchema>,
    #[serde(with = "crate::wire::nullable")]
    pub preflight_error: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedSavedEnvironmentRecordSchemaRelayManagedValue {
    #[serde()]
    pub relay_url: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedSavedEnvironmentRecordSchema {
    #[serde()]
    pub environment_id: crate::base_schemas::EnvironmentId,
    #[serde()]
    pub label: String,
    #[serde()]
    pub ws_base_url: String,
    #[serde()]
    pub http_base_url: String,
    #[serde()]
    pub created_at: String,
    #[serde(with = "crate::wire::nullable")]
    pub last_connected_at: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub desktop_ssh: Option<DesktopSshEnvironmentTargetSchema>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub relay_managed: Option<PersistedSavedEnvironmentRecordSchemaRelayManagedValue>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PickFolderOptionsSchema {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub initial_path: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub target_environment_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickedElementStackFrameSchema {
    #[serde(with = "crate::wire::nullable")]
    pub function_name: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub file_name: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub line_number: Option<crate::settings::WireNumber>,
    #[serde(with = "crate::wire::nullable")]
    pub column_number: Option<crate::settings::WireNumber>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickedElementPayloadSchema {
    #[serde()]
    pub page_url: String,
    #[serde(with = "crate::wire::nullable")]
    pub page_title: Option<String>,
    #[serde()]
    pub tag_name: String,
    #[serde(with = "crate::wire::nullable")]
    pub selector: Option<String>,
    #[serde()]
    pub html_preview: String,
    #[serde(with = "crate::wire::nullable")]
    pub component_name: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub source: Option<PickedElementStackFrameSchema>,
    #[serde()]
    pub stack: Vec<PickedElementStackFrameSchema>,
    #[serde()]
    pub styles: String,
    #[serde()]
    pub picked_at: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickedThemeFileSchema {
    #[serde()]
    pub name: String,
    #[serde()]
    pub size: crate::settings::WireNumber,
    #[serde()]
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAnnotationRectSchema {
    #[serde()]
    pub x: crate::settings::WireNumber,
    #[serde()]
    pub y: crate::settings::WireNumber,
    #[serde()]
    pub width: crate::settings::WireNumber,
    #[serde()]
    pub height: crate::settings::WireNumber,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAnnotationElementTargetSchema {
    #[serde()]
    pub id: String,
    #[serde()]
    pub element: PickedElementPayloadSchema,
    #[serde()]
    pub rect: PreviewAnnotationRectSchema,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAnnotationRegionTargetSchema {
    #[serde()]
    pub id: String,
    #[serde()]
    pub rect: PreviewAnnotationRectSchema,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAnnotationPointSchema {
    #[serde()]
    pub x: crate::settings::WireNumber,
    #[serde()]
    pub y: crate::settings::WireNumber,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAnnotationStrokeTargetSchema {
    #[serde()]
    pub id: String,
    #[serde()]
    pub color: String,
    #[serde()]
    pub width: crate::settings::WireNumber,
    #[serde()]
    pub points: Vec<PreviewAnnotationPointSchema>,
    #[serde()]
    pub bounds: PreviewAnnotationRectSchema,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAnnotationStyleChangeSchema {
    #[serde()]
    pub target_id: String,
    #[serde(with = "crate::wire::nullable")]
    pub selector: Option<String>,
    #[serde()]
    pub property: String,
    #[serde()]
    pub previous_value: String,
    #[serde()]
    pub value: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAnnotationScreenshotSchema {
    #[serde()]
    pub data_url: String,
    #[serde()]
    pub width: crate::settings::WireNumber,
    #[serde()]
    pub height: crate::settings::WireNumber,
    #[serde()]
    pub crop_rect: PreviewAnnotationRectSchema,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAnnotationPayloadSchema {
    #[serde()]
    pub id: String,
    #[serde()]
    pub page_url: String,
    #[serde(with = "crate::wire::nullable")]
    pub page_title: Option<String>,
    #[serde()]
    pub comment: String,
    #[serde()]
    pub elements: Vec<PreviewAnnotationElementTargetSchema>,
    #[serde()]
    pub regions: Vec<PreviewAnnotationRegionTargetSchema>,
    #[serde()]
    pub strokes: Vec<PreviewAnnotationStrokeTargetSchema>,
    #[serde()]
    pub style_changes: Vec<PreviewAnnotationStyleChangeSchema>,
    #[serde(with = "crate::wire::nullable")]
    pub screenshot: Option<PreviewAnnotationScreenshotSchema>,
    #[serde()]
    pub created_at: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PreviewAnnotationSubmissionSchema {
    #[serde(rename = "attach")]
    Attach,
    #[serde(rename = "send")]
    Send,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAnnotationSubmissionResultSchema {
    #[serde()]
    pub annotation: PreviewAnnotationPayloadSchema,
    #[serde()]
    pub submission: PreviewAnnotationSubmissionSchema,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub screenshot_failed: Option<bool>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SystemSettingsPaneSchema {
    #[serde(rename = "full-disk-access")]
    FullDiskAccess,
}

crate::redacted_debug!(DesktopEnvironmentBootstrapSchema { id, label, running_distro, http_base_url, ws_base_url } redact { bootstrap_token });
crate::redacted_debug!(DesktopSshBearerBootstrapInputSchema { http_base_url } redact { credential });
crate::redacted_debug!(DesktopSshBearerRequestInputSchema { http_base_url } redact { bearer_token });
crate::redacted_debug!(DesktopSshEnvironmentBootstrapSchema { target, http_base_url, ws_base_url, remote_port, remote_server_kind } redact { pairing_token });
crate::redacted_debug!(DesktopSshPasswordPromptResolutionInputSchema { request_id } redact { password });
