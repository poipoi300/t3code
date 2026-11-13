use crate::base_schemas::{EnvironmentId, ThreadId, TrimmedNonEmptyString};
use crate::browser_profile::BrowserProfileId;
use crate::preview::{
    PreviewNumber, PreviewRenderedViewportSize, PreviewTabId, PreviewViewportPresetId,
    PreviewViewportSetting,
};
use crate::provider_instance::ProviderInstanceId;
use crate::wire::JsNumber;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type PreviewAutomationClientId = TrimmedNonEmptyString;
pub type PreviewAutomationConnectionId = TrimmedNonEmptyString;
pub const PREVIEW_RECORDING_STOP_TIMEOUT_MS: u64 = 120_000;
pub const SERVER_BROWSER_AUTOMATION_CLIENT_ID: &str = "server-browser";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewAutomationOperation {
    #[serde(rename = "status")]
    Status,
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "navigate")]
    Navigate,
    #[serde(rename = "snapshot")]
    Snapshot,
    #[serde(rename = "click")]
    Click,
    #[serde(rename = "type")]
    Type,
    #[serde(rename = "press")]
    Press,
    #[serde(rename = "scroll")]
    Scroll,
    #[serde(rename = "evaluate")]
    Evaluate,
    #[serde(rename = "waitFor")]
    WaitFor,
    #[serde(rename = "recordingStart")]
    RecordingStart,
    #[serde(rename = "recordingStop")]
    RecordingStop,
    #[serde(rename = "resize")]
    Resize,
    #[serde(rename = "setColorScheme")]
    SetColorScheme,
    #[serde(rename = "dialog")]
    Dialog,
    #[serde(rename = "close")]
    Close,
    #[serde(rename = "upload")]
    Upload,
    #[serde(rename = "hover")]
    Hover,
    #[serde(rename = "select")]
    Select,
    #[serde(rename = "drag")]
    Drag,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationTabTargetInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id: Option<Option<PreviewTabId>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewAutomationTabOwner {
    #[serde(rename = "agent")]
    Agent,
    #[serde(rename = "human")]
    Human,
    #[serde(rename = "unclaimed")]
    Unclaimed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationProfile {
    pub id: BrowserProfileId,
    pub name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub incognito: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationLoadError {
    pub url: String,
    pub code: i64,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationControl {
    pub owner: PreviewAutomationTabOwner,
    pub owned_by_caller: bool,
    pub generation: PreviewNumber,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationDialog {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
    pub default_value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationFileChooser {
    pub multiple: bool,
    pub accept: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationDownload {
    pub file_name: String,
    pub path: String,
    pub size_bytes: PreviewNumber,
    pub url: String,
    pub completed_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationStatusTab {
    pub tab_id: PreviewTabId,
    #[serde(with = "crate::wire::nullable")]
    pub url: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub opener_tab_id: Option<Option<PreviewTabId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub owner: Option<Option<PreviewAutomationTabOwner>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub owned_by_caller: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub visible: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub profile_id: Option<Option<BrowserProfileId>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationStatus {
    pub available: bool,
    pub visible: bool,
    #[serde(with = "crate::wire::nullable")]
    pub tab_id: Option<PreviewTabId>,
    #[serde(with = "crate::wire::nullable")]
    pub url: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub title: Option<String>,
    pub loading: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub load_error: Option<Option<PreviewAutomationLoadError>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub control: Option<Option<PreviewAutomationControl>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub dialog: Option<Option<PreviewAutomationDialog>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub viewport_setting: Option<Option<PreviewViewportSetting>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub viewport: Option<Option<PreviewRenderedViewportSize>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub file_chooser: Option<Option<PreviewAutomationFileChooser>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub downloads: Option<Option<Vec<PreviewAutomationDownload>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tabs: Option<Option<Vec<PreviewAutomationStatusTab>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub profiles: Option<Option<Vec<PreviewAutomationProfile>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub default_profile_id: Option<Option<BrowserProfileId>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationDialogInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id: Option<Option<PreviewTabId>>,
    pub accept: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub prompt_text: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationOpenInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id: Option<Option<PreviewTabId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub url: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub open: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub show: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reuse_existing_tab: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub profile_id: Option<Option<TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrowserNavigationProtocol {
    #[serde(rename = "http")]
    Http,
    #[serde(rename = "https")]
    Https,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
pub enum BrowserNavigationTarget {
    #[serde(rename = "url")]
    Url { url: String },
    #[serde(rename = "environment-port")]
    EnvironmentPort {
        port: u16,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        protocol: Option<Option<BrowserNavigationProtocol>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        path: Option<Option<String>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewAutomationReadiness {
    #[serde(rename = "load")]
    Load,
    #[serde(rename = "domContentLoaded")]
    DomContentLoaded,
    #[serde(rename = "none")]
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationNavigateInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id: Option<Option<PreviewTabId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub url: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub target: Option<Option<BrowserNavigationTarget>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub readiness: Option<Option<PreviewAutomationReadiness>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub timeout_ms: Option<Option<i64>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewAutomationResizeMode {
    #[serde(rename = "fill")]
    Fill,
    #[serde(rename = "freeform")]
    Freeform,
    #[serde(rename = "preset")]
    Preset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewAutomationOrientation {
    #[serde(rename = "portrait")]
    Portrait,
    #[serde(rename = "landscape")]
    Landscape,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationResizeInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id: Option<Option<PreviewTabId>>,
    pub mode: PreviewAutomationResizeMode,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub preset: Option<Option<PreviewViewportPresetId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub width: Option<Option<i64>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub height: Option<Option<i64>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub orientation: Option<Option<PreviewAutomationOrientation>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub timeout_ms: Option<Option<i64>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationResizeResult {
    pub tab_id: PreviewTabId,
    pub setting: PreviewViewportSetting,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub viewport: Option<Option<PreviewRenderedViewportSize>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewAutomationColorScheme {
    #[serde(rename = "system")]
    System,
    #[serde(rename = "light")]
    Light,
    #[serde(rename = "dark")]
    Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationSetColorSchemeInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id: Option<Option<PreviewTabId>>,
    pub color_scheme: PreviewAutomationColorScheme,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationSetColorSchemeResult {
    pub tab_id: PreviewTabId,
    pub color_scheme: PreviewAutomationColorScheme,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewAutomationPointerButton {
    #[serde(rename = "left")]
    Left,
    #[serde(rename = "right")]
    Right,
    #[serde(rename = "middle")]
    Middle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationClickInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id: Option<Option<PreviewTabId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub selector: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub locator: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub x: Option<Option<JsNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub y: Option<Option<JsNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub timeout_ms: Option<Option<i64>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub button: Option<Option<PreviewAutomationPointerButton>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub click_count: Option<Option<i64>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationHoverInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id: Option<Option<PreviewTabId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub selector: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub locator: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub x: Option<Option<JsNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub y: Option<Option<JsNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub timeout_ms: Option<Option<i64>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationSelectInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id: Option<Option<PreviewTabId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub selector: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub locator: Option<Option<TrimmedNonEmptyString>>,
    pub values: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub timeout_ms: Option<Option<i64>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationSelectResult {
    pub selected: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationDragInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id: Option<Option<PreviewTabId>>,
    pub source: TrimmedNonEmptyString,
    pub target: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub timeout_ms: Option<Option<i64>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationUploadInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id: Option<Option<PreviewTabId>>,
    pub paths: Vec<TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub selector: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub locator: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub timeout_ms: Option<Option<i64>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationTypeInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id: Option<Option<PreviewTabId>>,
    pub text: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub selector: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub locator: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub clear: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub timeout_ms: Option<Option<i64>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewAutomationModifier {
    #[serde(rename = "Alt")]
    Alt,
    #[serde(rename = "Control")]
    Control,
    #[serde(rename = "Meta")]
    Meta,
    #[serde(rename = "Shift")]
    Shift,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationPressInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id: Option<Option<PreviewTabId>>,
    pub key: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub modifiers: Option<Option<Vec<PreviewAutomationModifier>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationScrollInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id: Option<Option<PreviewTabId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub delta_x: Option<Option<JsNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub delta_y: Option<Option<JsNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub selector: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub locator: Option<Option<TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewAutomationAction {
    #[serde(rename = "click")]
    Click,
    #[serde(rename = "type")]
    Type,
    #[serde(rename = "hover")]
    Hover,
    #[serde(rename = "select")]
    Select,
    #[serde(rename = "drag")]
    Drag,
    #[serde(rename = "upload")]
    Upload,
    #[serde(rename = "press")]
    Press,
    #[serde(rename = "scroll")]
    Scroll,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationActInput {
    pub action: PreviewAutomationAction,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id: Option<Option<PreviewTabId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub locator: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub selector: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub x: Option<Option<JsNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub y: Option<Option<JsNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub button: Option<Option<PreviewAutomationPointerButton>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub click_count: Option<Option<i64>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub text: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub clear: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub values: Option<Option<Vec<String>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub source: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub target: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub paths: Option<Option<Vec<String>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub key: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub modifiers: Option<Option<Vec<PreviewAutomationModifier>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub delta_x: Option<Option<JsNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub delta_y: Option<Option<JsNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub timeout_ms: Option<Option<i64>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationEvaluateInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id: Option<Option<PreviewTabId>>,
    pub expression: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub await_promise: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationWaitForInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id: Option<Option<PreviewTabId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub selector: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub locator: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub text: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub url_includes: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub timeout_ms: Option<Option<i64>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationElement {
    pub tag: String,
    #[serde(with = "crate::wire::nullable")]
    pub role: Option<String>,
    pub name: String,
    pub selector: String,
    pub x: PreviewNumber,
    pub y: PreviewNumber,
    pub width: PreviewNumber,
    pub height: PreviewNumber,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationConsoleEntry {
    pub level: String,
    pub text: String,
    pub timestamp: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub source: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationNetworkEntry {
    pub url: String,
    pub method: String,
    #[serde(with = "crate::wire::nullable")]
    pub status: Option<PreviewNumber>,
    pub failed: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub error_text: Option<Option<String>>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewAutomationActionStatus {
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "succeeded")]
    Succeeded,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "interrupted")]
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationActionEvent {
    pub id: String,
    pub action: String,
    pub status: PreviewAutomationActionStatus,
    pub started_at: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub completed_at: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub error: Option<Option<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewAutomationScreenshotMimeType {
    #[serde(rename = "image/png")]
    ImagePng,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationScreenshot {
    pub mime_type: PreviewAutomationScreenshotMimeType,
    pub data: String,
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationFullScreenshot {
    pub data: String,
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationSnapshot {
    pub url: String,
    pub title: String,
    pub loading: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub load_error: Option<Option<PreviewAutomationLoadError>>,
    pub visible_text: String,
    pub interactive_elements: Vec<PreviewAutomationElement>,
    pub accessibility_tree: Value,
    pub console_entries: Vec<PreviewAutomationConsoleEntry>,
    pub network_entries: Vec<PreviewAutomationNetworkEntry>,
    pub action_timeline: Vec<PreviewAutomationActionEvent>,
    pub screenshot: PreviewAutomationScreenshot,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub full_screenshot: Option<Option<PreviewAutomationFullScreenshot>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationSnapshotInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub full_screenshot: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationRecordingStatus {
    pub tab_id: PreviewTabId,
    pub recording: bool,
    #[serde(with = "crate::wire::nullable")]
    pub started_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationRecordingArtifact {
    pub id: String,
    pub tab_id: PreviewTabId,
    pub path: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationHostIdentity {
    pub client_id: PreviewAutomationClientId,
    pub environment_id: EnvironmentId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationHost {
    pub client_id: PreviewAutomationClientId,
    pub environment_id: EnvironmentId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub supported_operations: Option<Option<Vec<PreviewAutomationOperation>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationLiveTab {
    pub thread_id: ThreadId,
    pub tab_id: PreviewTabId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub visible: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationHostFocus {
    pub client_id: PreviewAutomationClientId,
    pub environment_id: EnvironmentId,
    pub connection_id: PreviewAutomationConnectionId,
    pub focused: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub live_tabs: Option<Option<Vec<PreviewAutomationLiveTab>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationRequest {
    pub request_id: TrimmedNonEmptyString,
    pub thread_id: ThreadId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id: Option<Option<PreviewTabId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id_explicit: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub agent_session_id: Option<Option<String>>,
    pub operation: PreviewAutomationOperation,
    pub input: Value,
    pub timeout_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum PreviewAutomationStreamEvent {
    #[serde(rename = "connected")]
    Connected {
        connection_id: PreviewAutomationConnectionId,
    },
    #[serde(rename = "request")]
    Request {
        connection_id: PreviewAutomationConnectionId,
        request: Box<PreviewAutomationRequest>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationResponseError {
    #[serde(rename = "_tag")]
    pub tag: TrimmedNonEmptyString,
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub detail: Option<Option<Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAutomationResponse {
    pub client_id: PreviewAutomationClientId,
    pub connection_id: PreviewAutomationConnectionId,
    pub request_id: TrimmedNonEmptyString,
    pub ok: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub result: Option<Option<Value>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub error: Option<Option<PreviewAutomationResponseError>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewAutomationCapability {
    #[serde(rename = "preview")]
    Preview,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewAutomationUnavailableError {
    #[serde(rename = "PreviewAutomationUnavailableError")]
    PreviewAutomationUnavailableError {
        capability: PreviewAutomationCapability,
        environment_id: EnvironmentId,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        thread_id: Option<Option<ThreadId>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        provider_session_id: Option<Option<TrimmedNonEmptyString>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        provider_instance_id: Option<Option<ProviderInstanceId>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum McpCapabilityUnavailableError {
    #[serde(rename = "McpCapabilityUnavailableError")]
    McpCapabilityUnavailableError {
        capability: TrimmedNonEmptyString,
        environment_id: EnvironmentId,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        thread_id: Option<Option<ThreadId>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        provider_session_id: Option<Option<TrimmedNonEmptyString>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        provider_instance_id: Option<Option<ProviderInstanceId>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewAutomationRemoteDetailKind {
    #[serde(rename = "null")]
    Null,
    #[serde(rename = "array")]
    Array,
    #[serde(rename = "object")]
    Object,
    #[serde(rename = "string")]
    String,
    #[serde(rename = "number")]
    Number,
    #[serde(rename = "boolean")]
    Boolean,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewAutomationNoAvailableHostError {
    #[serde(rename = "PreviewAutomationNoAvailableHostError")]
    PreviewAutomationNoAvailableHostError {
        operation: PreviewAutomationOperation,
        environment_id: EnvironmentId,
        thread_id: ThreadId,
        provider_session_id: TrimmedNonEmptyString,
        provider_instance_id: ProviderInstanceId,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        client_id: Option<Option<TrimmedNonEmptyString>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        connection_id: Option<Option<PreviewAutomationConnectionId>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        request_id: Option<Option<TrimmedNonEmptyString>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        tab_id: Option<Option<PreviewTabId>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        timeout_ms: Option<Option<i64>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        remote_tag: Option<Option<TrimmedNonEmptyString>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        remote_message_length: Option<Option<u64>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        remote_detail_kind: Option<Option<PreviewAutomationRemoteDetailKind>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        cause: Option<Option<Value>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewAutomationUnsupportedClientError {
    #[serde(rename = "PreviewAutomationUnsupportedClientError")]
    PreviewAutomationUnsupportedClientError {
        operation: PreviewAutomationOperation,
        environment_id: EnvironmentId,
        thread_id: ThreadId,
        provider_session_id: TrimmedNonEmptyString,
        provider_instance_id: ProviderInstanceId,
        client_id: TrimmedNonEmptyString,
        connection_id: PreviewAutomationConnectionId,
        request_id: TrimmedNonEmptyString,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        tab_id: Option<Option<PreviewTabId>>,
        timeout_ms: i64,
        remote_tag: TrimmedNonEmptyString,
        remote_message_length: u64,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        remote_detail_kind: Option<Option<PreviewAutomationRemoteDetailKind>>,
        cause: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewAutomationTabNotFoundError {
    #[serde(rename = "PreviewAutomationTabNotFoundError")]
    PreviewAutomationTabNotFoundError {
        operation: PreviewAutomationOperation,
        environment_id: EnvironmentId,
        thread_id: ThreadId,
        provider_session_id: TrimmedNonEmptyString,
        provider_instance_id: ProviderInstanceId,
        client_id: TrimmedNonEmptyString,
        connection_id: PreviewAutomationConnectionId,
        request_id: TrimmedNonEmptyString,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        tab_id: Option<Option<PreviewTabId>>,
        timeout_ms: i64,
        remote_tag: TrimmedNonEmptyString,
        remote_message_length: u64,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        remote_detail_kind: Option<Option<PreviewAutomationRemoteDetailKind>>,
        cause: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewAutomationRemoteUnavailableError {
    #[serde(rename = "PreviewAutomationRemoteUnavailableError")]
    PreviewAutomationRemoteUnavailableError {
        operation: PreviewAutomationOperation,
        environment_id: EnvironmentId,
        thread_id: ThreadId,
        provider_session_id: TrimmedNonEmptyString,
        provider_instance_id: ProviderInstanceId,
        client_id: TrimmedNonEmptyString,
        connection_id: PreviewAutomationConnectionId,
        request_id: TrimmedNonEmptyString,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        tab_id: Option<Option<PreviewTabId>>,
        timeout_ms: i64,
        remote_tag: TrimmedNonEmptyString,
        remote_message_length: u64,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        remote_detail_kind: Option<Option<PreviewAutomationRemoteDetailKind>>,
        cause: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewAutomationTimeoutError {
    #[serde(rename = "PreviewAutomationTimeoutError")]
    PreviewAutomationTimeoutError {
        operation: PreviewAutomationOperation,
        environment_id: EnvironmentId,
        thread_id: ThreadId,
        provider_session_id: TrimmedNonEmptyString,
        provider_instance_id: ProviderInstanceId,
        client_id: TrimmedNonEmptyString,
        connection_id: PreviewAutomationConnectionId,
        request_id: TrimmedNonEmptyString,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        tab_id: Option<Option<PreviewTabId>>,
        timeout_ms: i64,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        remote_tag: Option<Option<TrimmedNonEmptyString>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        remote_message_length: Option<Option<u64>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        remote_detail_kind: Option<Option<PreviewAutomationRemoteDetailKind>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        cause: Option<Option<Value>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        reason: Option<Option<String>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewAutomationControlReason {
    #[serde(rename = "agentMismatch")]
    AgentMismatch,
    #[serde(rename = "unclaimedTab")]
    UnclaimedTab,
    #[serde(rename = "humanControl")]
    HumanControl,
    #[serde(rename = "tabRequired")]
    TabRequired,
    #[serde(rename = "closed")]
    Closed,
    #[serde(rename = "interrupted")]
    Interrupted,
    #[serde(rename = "dialogPending")]
    DialogPending,
    #[serde(rename = "tabLimit")]
    TabLimit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewAutomationControlInterruptedError {
    #[serde(rename = "PreviewAutomationControlInterruptedError")]
    PreviewAutomationControlInterruptedError {
        operation: PreviewAutomationOperation,
        environment_id: EnvironmentId,
        thread_id: ThreadId,
        provider_session_id: TrimmedNonEmptyString,
        provider_instance_id: ProviderInstanceId,
        client_id: TrimmedNonEmptyString,
        connection_id: PreviewAutomationConnectionId,
        request_id: TrimmedNonEmptyString,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        tab_id: Option<Option<PreviewTabId>>,
        timeout_ms: i64,
        remote_tag: TrimmedNonEmptyString,
        remote_message_length: u64,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        remote_detail_kind: Option<Option<PreviewAutomationRemoteDetailKind>>,
        cause: Value,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        reason: Option<Option<PreviewAutomationControlReason>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewAutomationExecutionError {
    #[serde(rename = "PreviewAutomationExecutionError")]
    PreviewAutomationExecutionError {
        operation: PreviewAutomationOperation,
        environment_id: EnvironmentId,
        thread_id: ThreadId,
        provider_session_id: TrimmedNonEmptyString,
        provider_instance_id: ProviderInstanceId,
        client_id: TrimmedNonEmptyString,
        connection_id: PreviewAutomationConnectionId,
        request_id: TrimmedNonEmptyString,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        tab_id: Option<Option<PreviewTabId>>,
        timeout_ms: i64,
        remote_tag: TrimmedNonEmptyString,
        remote_message_length: u64,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        remote_detail_kind: Option<Option<PreviewAutomationRemoteDetailKind>>,
        cause: Value,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        reason: Option<Option<String>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewAutomationSelectorKind {
    #[serde(rename = "locator")]
    Locator,
    #[serde(rename = "selector")]
    Selector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewAutomationEditableSelectorKind {
    #[serde(rename = "focused-element")]
    FocusedElement,
    #[serde(rename = "locator")]
    Locator,
    #[serde(rename = "selector")]
    Selector,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewAutomationInvalidSelectorError {
    #[serde(rename = "PreviewAutomationInvalidSelectorError")]
    PreviewAutomationInvalidSelectorError {
        operation: PreviewAutomationOperation,
        environment_id: EnvironmentId,
        thread_id: ThreadId,
        provider_session_id: TrimmedNonEmptyString,
        provider_instance_id: ProviderInstanceId,
        client_id: TrimmedNonEmptyString,
        connection_id: PreviewAutomationConnectionId,
        request_id: TrimmedNonEmptyString,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        tab_id: Option<Option<PreviewTabId>>,
        timeout_ms: i64,
        remote_tag: TrimmedNonEmptyString,
        remote_message_length: u64,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        remote_detail_kind: Option<Option<PreviewAutomationRemoteDetailKind>>,
        cause: Value,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        selector_kind: Option<Option<PreviewAutomationSelectorKind>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        selector_length: Option<Option<u64>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        stale_ref: Option<Option<bool>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewAutomationTargetNotEditableError {
    #[serde(rename = "PreviewAutomationTargetNotEditableError")]
    PreviewAutomationTargetNotEditableError {
        operation: PreviewAutomationOperation,
        environment_id: EnvironmentId,
        thread_id: ThreadId,
        provider_session_id: TrimmedNonEmptyString,
        provider_instance_id: ProviderInstanceId,
        client_id: TrimmedNonEmptyString,
        connection_id: PreviewAutomationConnectionId,
        request_id: TrimmedNonEmptyString,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        tab_id: Option<Option<PreviewTabId>>,
        timeout_ms: i64,
        remote_tag: TrimmedNonEmptyString,
        remote_message_length: u64,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        remote_detail_kind: Option<Option<PreviewAutomationRemoteDetailKind>>,
        cause: Value,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        selector_kind: Option<Option<PreviewAutomationEditableSelectorKind>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        selector_length: Option<Option<u64>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewAutomationResultTooLargeError {
    #[serde(rename = "PreviewAutomationResultTooLargeError")]
    PreviewAutomationResultTooLargeError {
        operation: PreviewAutomationOperation,
        environment_id: EnvironmentId,
        thread_id: ThreadId,
        provider_session_id: TrimmedNonEmptyString,
        provider_instance_id: ProviderInstanceId,
        client_id: TrimmedNonEmptyString,
        connection_id: PreviewAutomationConnectionId,
        request_id: TrimmedNonEmptyString,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        tab_id: Option<Option<PreviewTabId>>,
        timeout_ms: i64,
        remote_tag: TrimmedNonEmptyString,
        remote_message_length: u64,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        remote_detail_kind: Option<Option<PreviewAutomationRemoteDetailKind>>,
        cause: Value,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        maximum_bytes: Option<Option<u64>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewAutomationClientDisconnectedError {
    #[serde(rename = "PreviewAutomationClientDisconnectedError")]
    PreviewAutomationClientDisconnectedError {
        operation: PreviewAutomationOperation,
        environment_id: EnvironmentId,
        thread_id: ThreadId,
        provider_session_id: TrimmedNonEmptyString,
        provider_instance_id: ProviderInstanceId,
        client_id: TrimmedNonEmptyString,
        connection_id: PreviewAutomationConnectionId,
        request_id: TrimmedNonEmptyString,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        tab_id: Option<Option<PreviewTabId>>,
        timeout_ms: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewAutomationRequestQueueClosedError {
    #[serde(rename = "PreviewAutomationRequestQueueClosedError")]
    PreviewAutomationRequestQueueClosedError {
        operation: PreviewAutomationOperation,
        environment_id: EnvironmentId,
        thread_id: ThreadId,
        provider_session_id: TrimmedNonEmptyString,
        provider_instance_id: ProviderInstanceId,
        client_id: TrimmedNonEmptyString,
        connection_id: PreviewAutomationConnectionId,
        request_id: TrimmedNonEmptyString,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        tab_id: Option<Option<PreviewTabId>>,
        timeout_ms: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewAutomationMalformedResponseError {
    #[serde(rename = "PreviewAutomationMalformedResponseError")]
    PreviewAutomationMalformedResponseError {
        operation: PreviewAutomationOperation,
        environment_id: EnvironmentId,
        thread_id: ThreadId,
        provider_session_id: TrimmedNonEmptyString,
        provider_instance_id: ProviderInstanceId,
        client_id: TrimmedNonEmptyString,
        connection_id: PreviewAutomationConnectionId,
        request_id: TrimmedNonEmptyString,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        tab_id: Option<Option<PreviewTabId>>,
        timeout_ms: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewAutomationRecordingTransferError {
    #[serde(rename = "PreviewAutomationRecordingTransferError")]
    PreviewAutomationRecordingTransferError {
        thread_id: ThreadId,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        cause: Option<Option<Value>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewAutomationRecordingDesktopUpdateRequiredError {
    #[serde(rename = "PreviewAutomationRecordingDesktopUpdateRequiredError")]
    PreviewAutomationRecordingDesktopUpdateRequiredError {
        thread_id: ThreadId,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        cause: Option<Option<Value>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewAutomationRecordingTooLargeError {
    #[serde(rename = "PreviewAutomationRecordingTooLargeError")]
    PreviewAutomationRecordingTooLargeError {
        thread_id: ThreadId,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        cause: Option<Option<Value>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewAutomationRecordingDeadlineExpiredError {
    #[serde(rename = "PreviewAutomationRecordingDeadlineExpiredError")]
    PreviewAutomationRecordingDeadlineExpiredError {
        thread_id: ThreadId,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        cause: Option<Option<Value>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PreviewAutomationError {
    RecordingTransferError(Box<PreviewAutomationRecordingTransferError>),
    RecordingDesktopUpdateRequiredError(Box<PreviewAutomationRecordingDesktopUpdateRequiredError>),
    RecordingTooLargeError(Box<PreviewAutomationRecordingTooLargeError>),
    RecordingDeadlineExpiredError(Box<PreviewAutomationRecordingDeadlineExpiredError>),
    UnavailableError(Box<PreviewAutomationUnavailableError>),
    NoAvailableHostError(Box<PreviewAutomationNoAvailableHostError>),
    UnsupportedClientError(Box<PreviewAutomationUnsupportedClientError>),
    TabNotFoundError(Box<PreviewAutomationTabNotFoundError>),
    TimeoutError(Box<PreviewAutomationTimeoutError>),
    ControlInterruptedError(Box<PreviewAutomationControlInterruptedError>),
    ExecutionError(Box<PreviewAutomationExecutionError>),
    InvalidSelectorError(Box<PreviewAutomationInvalidSelectorError>),
    TargetNotEditableError(Box<PreviewAutomationTargetNotEditableError>),
    ResultTooLargeError(Box<PreviewAutomationResultTooLargeError>),
    ClientDisconnectedError(Box<PreviewAutomationClientDisconnectedError>),
    RequestQueueClosedError(Box<PreviewAutomationRequestQueueClosedError>),
    RemoteUnavailableError(Box<PreviewAutomationRemoteUnavailableError>),
    MalformedResponseError(Box<PreviewAutomationMalformedResponseError>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewUrlResolutionKind {
    #[serde(rename = "direct")]
    Direct,
    #[serde(rename = "direct-private-network")]
    DirectPrivateNetwork,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewUrlResolution {
    pub requested_url: String,
    pub resolved_url: String,
    pub resolution_kind: PreviewUrlResolutionKind,
    pub environment_id: EnvironmentId,
}
