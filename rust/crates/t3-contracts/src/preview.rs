use crate::base_schemas::{ThreadId, TrimmedNonEmptyString};
use crate::browser_profile::{BrowserProfile, BrowserProfileId};
use crate::wire::JsNumber;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PREVIEW_URL_MAX_LENGTH: usize = 2048;
pub const CONFIGURED_LOCAL_SERVER_URLS_MAX_ITEMS: usize = 32;
pub const PREVIEW_VIEWPORT_MIN_DIMENSION: u64 = 240;
pub const PREVIEW_VIEWPORT_MAX_DIMENSION: u64 = 3840;
pub const PREVIEW_VIEWPORT_MAX_AREA: u64 = 3840 * 2160;
pub const PREVIEW_STREAM_HOST_SETUP_CLOSE_CODE: u16 = 4503;
pub type PreviewTabId = TrimmedNonEmptyString;
pub type ConfiguredLocalServerUrls = Vec<TrimmedNonEmptyString>;
pub const PREVIEW_ZOOM_LEVELS: &[f64] = &[
    0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0,
];

pub type PreviewZoomFactor = JsNumber;
pub type BrowserRecordingFrameRate = u64;
pub const DEFAULT_BROWSER_RECORDING_FRAME_RATE: BrowserRecordingFrameRate = 30;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PreviewNumber {
    Finite(JsNumber),
    NonFinite(PreviewNonFiniteNumber),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewNonFiniteNumber {
    #[serde(rename = "Infinity")]
    Infinity,
    #[serde(rename = "-Infinity")]
    NegativeInfinity,
    #[serde(rename = "NaN")]
    NaN,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "bool", into = "bool")]
pub struct PreviewConfiguredUrlProbing;

impl TryFrom<bool> for PreviewConfiguredUrlProbing {
    type Error = &'static str;
    fn try_from(value: bool) -> Result<Self, Self::Error> {
        if value {
            Ok(Self)
        } else {
            Err("expected true")
        }
    }
}

impl From<PreviewConfiguredUrlProbing> for bool {
    fn from(_: PreviewConfiguredUrlProbing) -> Self {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewViewportSize {
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRenderedViewportSize {
    pub width: u64,
    pub height: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewViewportPresetId {
    #[serde(rename = "iphone-se")]
    IphoneSe,
    #[serde(rename = "iphone-xr")]
    IphoneXr,
    #[serde(rename = "iphone-12-pro")]
    Iphone12Pro,
    #[serde(rename = "iphone-14-pro-max")]
    Iphone14ProMax,
    #[serde(rename = "pixel-7")]
    Pixel7,
    #[serde(rename = "samsung-galaxy-s8-plus")]
    SamsungGalaxyS8Plus,
    #[serde(rename = "samsung-galaxy-s20-ultra")]
    SamsungGalaxyS20Ultra,
    #[serde(rename = "ipad-mini")]
    IpadMini,
    #[serde(rename = "ipad-air")]
    IpadAir,
    #[serde(rename = "ipad-pro")]
    IpadPro,
    #[serde(rename = "surface-pro-7")]
    SurfacePro7,
    #[serde(rename = "surface-duo")]
    SurfaceDuo,
    #[serde(rename = "galaxy-z-fold-5")]
    GalaxyZFold5,
    #[serde(rename = "asus-zenbook-fold")]
    AsusZenbookFold,
    #[serde(rename = "samsung-galaxy-a51-71")]
    SamsungGalaxyA5171,
    #[serde(rename = "nest-hub")]
    NestHub,
    #[serde(rename = "nest-hub-max")]
    NestHubMax,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoredPreviewViewportPresetId {
    #[serde(rename = "iphone-se")]
    IphoneSe,
    #[serde(rename = "iphone-xr")]
    IphoneXr,
    #[serde(rename = "iphone-12-pro")]
    Iphone12Pro,
    #[serde(rename = "iphone-14-pro-max")]
    Iphone14ProMax,
    #[serde(rename = "pixel-7")]
    Pixel7,
    #[serde(rename = "samsung-galaxy-s8-plus")]
    SamsungGalaxyS8Plus,
    #[serde(rename = "samsung-galaxy-s20-ultra")]
    SamsungGalaxyS20Ultra,
    #[serde(rename = "ipad-mini")]
    IpadMini,
    #[serde(rename = "ipad-air")]
    IpadAir,
    #[serde(rename = "ipad-pro")]
    IpadPro,
    #[serde(rename = "surface-pro-7")]
    SurfacePro7,
    #[serde(rename = "surface-duo")]
    SurfaceDuo,
    #[serde(rename = "galaxy-z-fold-5")]
    GalaxyZFold5,
    #[serde(rename = "asus-zenbook-fold")]
    AsusZenbookFold,
    #[serde(rename = "samsung-galaxy-a51-71")]
    SamsungGalaxyA5171,
    #[serde(rename = "nest-hub")]
    NestHub,
    #[serde(rename = "nest-hub-max")]
    NestHubMax,
    #[serde(rename = "desktop-1920x1080")]
    Desktop1920x1080,
    #[serde(rename = "desktop-1440x900")]
    Desktop1440x900,
    #[serde(rename = "laptop-1366x768")]
    Laptop1366x768,
    #[serde(rename = "laptop-1280x800")]
    Laptop1280x800,
    #[serde(rename = "ipad-pro-11")]
    IpadPro11,
    #[serde(rename = "iphone-15-pro")]
    Iphone15Pro,
    #[serde(rename = "pixel-8")]
    Pixel8,
    #[serde(rename = "galaxy-s24")]
    GalaxyS24,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewViewportSetting {
    #[serde(rename = "fill")]
    Fill {},
    #[serde(rename = "freeform")]
    Freeform { width: i64, height: i64 },
    #[serde(rename = "preset")]
    Preset {
        width: i64,
        height: i64,
        preset_id: StoredPreviewViewportPresetId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewAppearancePreference {
    #[serde(rename = "system")]
    System,
    #[serde(rename = "light")]
    Light,
    #[serde(rename = "dark")]
    Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewFileDownload {
    pub id: String,
    pub file_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewNavStatus {
    #[serde(rename = "Idle")]
    Idle {},
    #[serde(rename = "Loading")]
    Loading {
        url: TrimmedNonEmptyString,
        title: String,
    },
    #[serde(rename = "Success")]
    Success {
        url: TrimmedNonEmptyString,
        title: String,
    },
    #[serde(rename = "LoadFailed")]
    LoadFailed {
        url: TrimmedNonEmptyString,
        title: String,
        code: i64,
        description: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        download: Option<Option<PreviewFileDownload>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewRuntime {
    #[serde(rename = "desktop")]
    Desktop,
    #[serde(rename = "server")]
    Server,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewHostSetupNeed {
    #[serde(rename = "sandbox")]
    Sandbox,
    #[serde(rename = "libraries")]
    Libraries,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewStreamHostSetup {
    pub need: PreviewHostSetupNeed,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRevealRequest {
    pub id: String,
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewSessionSnapshot {
    pub thread_id: TrimmedNonEmptyString,
    pub tab_id: PreviewTabId,
    pub nav_status: PreviewNavStatus,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub viewport: Option<Option<PreviewViewportSetting>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub color_scheme: Option<Option<PreviewAppearancePreference>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub zoom_factor: Option<Option<PreviewZoomFactor>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub profile_id: Option<Option<BrowserProfileId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub runtime: Option<Option<PreviewRuntime>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub headless: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub automation_owner: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reveal: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reveal_request: Option<Option<PreviewRevealRequest>>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewOpenInput {
    pub thread_id: ThreadId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub url: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub viewport: Option<Option<PreviewViewportSetting>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub profile_id: Option<Option<BrowserProfileId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub runtime: Option<Option<PreviewRuntime>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reveal: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewReportStatusInput {
    pub thread_id: ThreadId,
    pub tab_id: PreviewTabId,
    pub nav_status: PreviewNavStatus,
    pub can_go_back: bool,
    pub can_go_forward: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewResizeInput {
    pub thread_id: ThreadId,
    pub tab_id: PreviewTabId,
    pub viewport: PreviewViewportSetting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewClearTarget {
    #[serde(rename = "cookies")]
    Cookies,
    #[serde(rename = "cache")]
    Cache,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAdjustInput {
    pub thread_id: ThreadId,
    pub tab_id: PreviewTabId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub color_scheme: Option<Option<PreviewAppearancePreference>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub zoom_factor: Option<Option<PreviewZoomFactor>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub hard_reload: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub clear: Option<Option<PreviewClearTarget>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewCloseInput {
    pub thread_id: ThreadId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tab_id: Option<Option<PreviewTabId>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewReportProfilesInput {
    pub profiles: Vec<BrowserProfile>,
    pub default_profile_id: BrowserProfileId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub recording_frame_rate: Option<Option<BrowserRecordingFrameRate>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewClearProfileInput {
    pub profile_id: BrowserProfileId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewListInput {
    pub thread_id: ThreadId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewListResult {
    pub sessions: Vec<PreviewSessionSnapshot>,
    pub server_epoch: TrimmedNonEmptyString,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewReloadRequest {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub hard_reload: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub clear: Option<Option<PreviewClearTarget>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum PreviewEvent {
    #[serde(rename = "opened")]
    Opened {
        thread_id: TrimmedNonEmptyString,
        tab_id: PreviewTabId,
        created_at: String,
        server_epoch: TrimmedNonEmptyString,
        revision: u64,
        snapshot: PreviewSessionSnapshot,
    },
    #[serde(rename = "navigated")]
    Navigated {
        thread_id: TrimmedNonEmptyString,
        tab_id: PreviewTabId,
        created_at: String,
        server_epoch: TrimmedNonEmptyString,
        revision: u64,
        snapshot: PreviewSessionSnapshot,
    },
    #[serde(rename = "resized")]
    Resized {
        thread_id: TrimmedNonEmptyString,
        tab_id: PreviewTabId,
        created_at: String,
        server_epoch: TrimmedNonEmptyString,
        revision: u64,
        snapshot: PreviewSessionSnapshot,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        request: Option<Option<PreviewReloadRequest>>,
    },
    #[serde(rename = "failed")]
    Failed {
        thread_id: TrimmedNonEmptyString,
        tab_id: PreviewTabId,
        created_at: String,
        server_epoch: TrimmedNonEmptyString,
        revision: u64,
        url: TrimmedNonEmptyString,
        title: String,
        code: i64,
        description: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        download: Option<Option<PreviewFileDownload>>,
    },
    #[serde(rename = "closed")]
    Closed {
        thread_id: TrimmedNonEmptyString,
        tab_id: PreviewTabId,
        created_at: String,
        server_epoch: TrimmedNonEmptyString,
        revision: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredLocalServerTerminal {
    pub thread_id: ThreadId,
    pub terminal_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredLocalServerAgent {
    pub thread_id: ThreadId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredLocalServer {
    pub host: TrimmedNonEmptyString,
    pub port: u16,
    pub url: TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub process_name: Option<TrimmedNonEmptyString>,
    #[serde(with = "crate::wire::nullable")]
    pub pid: Option<u64>,
    #[serde(with = "crate::wire::nullable")]
    pub terminal: Option<DiscoveredLocalServerTerminal>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub agent: Option<Option<DiscoveredLocalServerAgent>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewStopLocalServerInput {
    pub pid: u64,
    pub port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewStopLocalServerReason {
    #[serde(rename = "not-found")]
    NotFound,
    #[serde(rename = "shared-group")]
    SharedGroup,
    #[serde(rename = "unsupported")]
    Unsupported,
    #[serde(rename = "signal-failed")]
    SignalFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewStopLocalServerError {
    #[serde(rename = "PreviewStopLocalServerError")]
    PreviewStopLocalServerError {
        reason: PreviewStopLocalServerReason,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        cause: Option<Option<Value>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredLocalServerList {
    pub servers: Vec<DiscoveredLocalServer>,
    pub scanned_at: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub configured_url_probing: Option<Option<PreviewConfiguredUrlProbing>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewSessionLookupError {
    #[serde(rename = "PreviewSessionLookupError")]
    PreviewSessionLookupError { thread_id: String, tab_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewInvalidUrlReason {
    #[serde(rename = "empty")]
    Empty,
    #[serde(rename = "parse")]
    Parse,
    #[serde(rename = "unsupported-protocol")]
    UnsupportedProtocol,
    #[serde(rename = "unexpected")]
    Unexpected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewInvalidUrlError {
    #[serde(rename = "PreviewInvalidUrlError")]
    PreviewInvalidUrlError {
        input_length: PreviewNumber,
        reason: PreviewInvalidUrlReason,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        protocol: Option<Option<String>>,
        cause: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewControlRequiredError {
    #[serde(rename = "PreviewControlRequiredError")]
    PreviewControlRequiredError { tab_id: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum PreviewClearProfileError {
    #[serde(rename = "PreviewClearProfileError")]
    PreviewClearProfileError { profile_id: String, cause: Value },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PreviewError {
    SessionLookup(PreviewSessionLookupError),
    InvalidUrl(PreviewInvalidUrlError),
    ControlRequired(PreviewControlRequiredError),
}
