use serde::{Deserialize, Serialize};

/// Effect encodes non-finite numbers as string literals in JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WireNumber {
    Finite(crate::wire::JsNumber),
    NonFinite(WireNonFiniteNumber),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WireNonFiniteNumber {
    #[serde(rename = "Infinity")]
    Infinity,
    #[serde(rename = "-Infinity")]
    NegativeInfinity,
    #[serde(rename = "NaN")]
    NaN,
}

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

fn deserialize_quit_confirmation<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<QuitConfirmationMode, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Source {
        Mode(QuitConfirmationMode),
        Legacy(bool),
    }
    Ok(match Source::deserialize(deserializer)? {
        Source::Mode(mode) => mode,
        Source::Legacy(true) => QuitConfirmationMode::Hold,
        Source::Legacy(false) => QuitConfirmationMode::Direct,
    })
}

fn deserialize_writing_style<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<SourceControlWritingStyleSettings, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Source {
        Settings(SourceControlWritingStyleSettings),
        Legacy(String),
    }
    Ok(match Source::deserialize(deserializer)? {
        Source::Settings(settings) => settings,
        Source::Legacy(value) => {
            let value = crate::wire::trim_js(&value);
            SourceControlWritingStyleSettings {
                mode: if value.is_empty() {
                    SourceControlWritingStyleMode::RepoConventions
                } else {
                    SourceControlWritingStyleMode::Custom
                },
                custom_instructions: crate::base_schemas::TrimmedString(value.to_owned()),
                follow_change_request_templates: true,
            }
        }
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AntigravityAuthMethod {
    #[serde(rename = "oauth-personal")]
    OauthPersonal,
    #[serde(rename = "oauth-business")]
    OauthBusiness,
    #[serde(rename = "gemini-api-key")]
    GeminiApiKey,
    #[serde(rename = "agent-platform")]
    AgentPlatform,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AntigravitySettings {
    #[serde(default = "antigravity_settings_enabled_default")]
    pub enabled: bool,
    #[serde(default = "antigravity_settings_auth_method_default")]
    pub auth_method: AntigravityAuthMethod,
    #[serde(default = "antigravity_settings_api_key_default")]
    pub api_key: crate::settings_secret_value::SettingsSecretValue,
    #[serde(default = "antigravity_settings_gcp_project_default")]
    pub gcp_project: crate::base_schemas::TrimmedString,
    #[serde(default = "antigravity_settings_gcp_location_default")]
    pub gcp_location: crate::base_schemas::TrimmedString,
    #[serde(default = "antigravity_settings_binary_path_default")]
    pub binary_path: crate::base_schemas::TrimmedString,
    #[serde(default = "antigravity_settings_custom_models_default")]
    pub custom_models: Vec<crate::model::CustomModelSetting>,
}
impl Default for AntigravitySettings {
    fn default() -> Self {
        Self {
            enabled: antigravity_settings_enabled_default(),
            auth_method: antigravity_settings_auth_method_default(),
            api_key: antigravity_settings_api_key_default(),
            gcp_project: antigravity_settings_gcp_project_default(),
            gcp_location: antigravity_settings_gcp_location_default(),
            binary_path: antigravity_settings_binary_path_default(),
            custom_models: antigravity_settings_custom_models_default(),
        }
    }
}
pub type AppearanceContrast = i64;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundActivityOverrides {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub automatic_git_fetch_interval: Option<WireNumber>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub provider_health_refresh_interval: Option<WireNumber>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub host_power_monitor_active_interval: Option<WireNumber>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub host_power_monitor_idle_interval: Option<WireNumber>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub pause_when_host_locked: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub pause_when_host_low_power: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub pause_when_client_low_power: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub pause_when_on_battery: Option<bool>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BackgroundActivityProfile {
    #[serde(rename = "balanced")]
    Balanced,
    #[serde(rename = "performance")]
    Performance,
    #[serde(rename = "battery-saver")]
    BatterySaver,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BackgroundActivityProfileSelection {
    #[serde(rename = "balanced")]
    Balanced,
    #[serde(rename = "performance")]
    Performance,
    #[serde(rename = "battery-saver")]
    BatterySaver,
    #[serde(rename = "custom")]
    Custom,
}
pub type BackgroundActivitySettingsSchemaVersion = WireLiteral<1>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundActivitySettings {
    #[serde(default = "background_activity_settings_schema_version_default")]
    pub schema_version: BackgroundActivitySettingsSchemaVersion,
    #[serde(default = "background_activity_settings_profile_default")]
    pub profile: BackgroundActivityProfileSelection,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub base_profile: Option<BackgroundActivityProfile>,
    #[serde(default = "background_activity_settings_overrides_default")]
    pub overrides: BackgroundActivityOverrides,
}
impl Default for BackgroundActivitySettings {
    fn default() -> Self {
        Self {
            schema_version: background_activity_settings_schema_version_default(),
            profile: background_activity_settings_profile_default(),
            base_profile: None,
            overrides: background_activity_settings_overrides_default(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BranchNamingMode {
    #[serde(rename = "static")]
    Static,
    #[serde(rename = "semantic")]
    Semantic,
    #[serde(rename = "custom")]
    Custom,
}
pub type BrowserCustomSearchUrl = String;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BrowserLinkTarget {
    #[serde(rename = "system")]
    System,
    #[serde(rename = "app")]
    App,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BrowserSearchEngine {
    #[serde(rename = "duckduckgo")]
    Duckduckgo,
    #[serde(rename = "google")]
    Google,
    #[serde(rename = "bing")]
    Bing,
    #[serde(rename = "brave")]
    Brave,
    #[serde(rename = "kagi")]
    Kagi,
    #[serde(rename = "ecosia")]
    Ecosia,
    #[serde(rename = "custom")]
    Custom,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ChatWidth {
    #[serde(rename = "comfortable")]
    Comfortable,
    #[serde(rename = "wide")]
    Wide,
    #[serde(rename = "full")]
    Full,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InstanceIdleCompaction {
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "onReturn")]
    OnReturn,
    #[serde(rename = "beforeCacheExpires")]
    BeforeCacheExpires,
    #[serde(rename = "off")]
    Off,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSettings {
    #[serde(default = "claude_settings_enabled_default")]
    pub enabled: bool,
    #[serde(default = "claude_settings_binary_path_default")]
    pub binary_path: String,
    #[serde(default = "claude_settings_home_path_default")]
    pub home_path: crate::base_schemas::TrimmedString,
    #[serde(default = "claude_settings_custom_models_default")]
    pub custom_models: Vec<crate::model::CustomModelSetting>,
    #[serde(default = "claude_settings_launch_args_default")]
    pub launch_args: String,
    #[serde(default = "claude_settings_auto_compact_window_default")]
    pub auto_compact_window: crate::base_schemas::TrimmedString,
    #[serde(default = "claude_settings_idle_compaction_default")]
    pub idle_compaction: InstanceIdleCompaction,
}
impl Default for ClaudeSettings {
    fn default() -> Self {
        Self {
            enabled: claude_settings_enabled_default(),
            binary_path: claude_settings_binary_path_default(),
            home_path: claude_settings_home_path_default(),
            custom_models: claude_settings_custom_models_default(),
            launch_args: claude_settings_launch_args_default(),
            auto_compact_window: claude_settings_auto_compact_window_default(),
            idle_compaction: claude_settings_idle_compaction_default(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NotificationMode {
    #[serde(rename = "off")]
    Off,
    #[serde(rename = "notifications")]
    Notifications,
    #[serde(rename = "sound")]
    Sound,
    #[serde(rename = "notifications-and-sound")]
    NotificationsAndSound,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DiffColorScheme {
    #[serde(rename = "red-green")]
    RedGreen,
    #[serde(rename = "blue-orange")]
    BlueOrange,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EffortDisplay {
    #[serde(rename = "label")]
    Label,
    #[serde(rename = "bars")]
    Bars,
}
pub type LoadBalancingWeights =
    std::collections::BTreeMap<crate::base_schemas::TrimmedNonEmptyString, i64>;
pub type PanelAnimationDurationMs = i64;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum QuitConfirmationMode {
    #[serde(rename = "direct")]
    Direct,
    #[serde(rename = "hold")]
    Hold,
    #[serde(rename = "double-click")]
    DoubleClick,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DiffLayout {
    #[serde(rename = "stacked")]
    Stacked,
    #[serde(rename = "split")]
    Split,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EnvironmentIdentificationMode {
    #[serde(rename = "artwork")]
    Artwork,
    #[serde(rename = "nightly-artwork")]
    NightlyArtwork,
    #[serde(rename = "dev-artwork")]
    DevArtwork,
    #[serde(rename = "pill")]
    Pill,
    #[serde(rename = "none")]
    None,
}
pub type GlassOpacity = i64;
pub type InterfaceFontSize = i64;
pub type PromptFontSize = i64;
pub type CodeFontSize = i64;
pub type TerminalFontSize = i64;
pub type FontFamilyPreference = String;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelFavorite {
    #[serde()]
    pub provider: crate::provider_instance::ProviderInstanceId,
    #[serde()]
    pub model: crate::base_schemas::TrimmedNonEmptyString,
}
pub type ModelFavorites = Vec<ModelFavorite>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelPreferencesValueValue {
    #[serde(default = "provider_model_preferences_value_value_hidden_models_default")]
    pub hidden_models: Vec<String>,
    #[serde(default = "provider_model_preferences_value_value_model_order_default")]
    pub model_order: Vec<String>,
}
impl Default for ProviderModelPreferencesValueValue {
    fn default() -> Self {
        Self {
            hidden_models: provider_model_preferences_value_value_hidden_models_default(),
            model_order: provider_model_preferences_value_value_model_order_default(),
        }
    }
}
pub type ProviderModelPreferences = std::collections::BTreeMap<
    crate::provider_instance::ProviderInstanceId,
    ProviderModelPreferencesValueValue,
>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientSettingsPatchSendShortcutValue {
    #[serde(rename = "enter")]
    Enter,
    #[serde(rename = "mod-enter-multiline")]
    ModEnterMultiline,
    #[serde(rename = "mod-enter")]
    ModEnter,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientSettingsPatchFollowUpBehaviorValue {
    #[serde(rename = "queue")]
    Queue,
    #[serde(rename = "steer")]
    Steer,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToolCallVisibility {
    #[serde(rename = "showAll")]
    ShowAll,
    #[serde(rename = "hideSuccessful")]
    HideSuccessful,
    #[serde(rename = "hideAll")]
    HideAll,
    #[serde(rename = "zen")]
    Zen,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScreenReaderAnnouncements {
    #[serde(rename = "status")]
    Status,
    #[serde(rename = "messages")]
    Messages,
    #[serde(rename = "everything")]
    Everything,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SidebarProjectGroupingMode {
    #[serde(rename = "repository")]
    Repository,
    #[serde(rename = "repository_path")]
    RepositoryPath,
    #[serde(rename = "separate")]
    Separate,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SidebarProjectSortOrder {
    #[serde(rename = "updated_at")]
    UpdatedAt,
    #[serde(rename = "created_at")]
    CreatedAt,
    #[serde(rename = "manual")]
    Manual,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SidebarThreadSortOrder {
    #[serde(rename = "updated_at")]
    UpdatedAt,
    #[serde(rename = "created_at")]
    CreatedAt,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TimestampFormat {
    #[serde(rename = "locale")]
    Locale,
    #[serde(rename = "12-hour")]
    V12Hour,
    #[serde(rename = "24-hour")]
    V24Hour,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VoiceMicrophoneEntryMember1Kind {
    #[serde(rename = "default")]
    Default,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceMicrophoneEntryMember1 {
    #[serde()]
    pub kind: VoiceMicrophoneEntryMember1Kind,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VoiceMicrophoneEntryMember2Kind {
    #[serde(rename = "device")]
    Device,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceMicrophoneEntryMember2 {
    #[serde()]
    pub kind: VoiceMicrophoneEntryMember2Kind,
    #[serde()]
    pub device_id: String,
    #[serde()]
    pub label: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VoiceMicrophoneEntry {
    Default1(Box<VoiceMicrophoneEntryMember1>),
    Device2(Box<VoiceMicrophoneEntryMember2>),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VoiceShortcutMode {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "hold")]
    Hold,
    #[serde(rename = "toggle")]
    Toggle,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SnapShotShortcutMember1Kind {
    #[serde(rename = "both-shift-keys")]
    BothShiftKeys,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapShotShortcutMember1 {
    #[serde()]
    pub kind: SnapShotShortcutMember1Kind,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SnapShotShortcutMember2Kind {
    #[serde(rename = "modifier-pair")]
    ModifierPair,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SnapShotModifier {
    #[serde(rename = "shift")]
    Shift,
    #[serde(rename = "meta")]
    Meta,
    #[serde(rename = "control")]
    Control,
    #[serde(rename = "alt")]
    Alt,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapShotShortcutMember2 {
    #[serde()]
    pub kind: SnapShotShortcutMember2Kind,
    #[serde()]
    pub modifier: SnapShotModifier,
}
pub type SnapShotKeyChord = crate::keybindings::KeybindingShortcut;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SnapShotShortcut {
    BothShiftKeys1(Box<SnapShotShortcutMember1>),
    ModifierPair2(Box<SnapShotShortcutMember2>),
    SnapShotKeyChord(Box<SnapShotKeyChord>),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SnapShotSound {
    #[serde(rename = "soft-pop")]
    SoftPop,
    #[serde(rename = "camera-shutter")]
    CameraShutter,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ClientSettingsPatch {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub notification_mode: Option<NotificationMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub in_app_notifications_enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub diff_color_scheme: Option<DiffColorScheme>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub chat_width: Option<ChatWidth>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub effort_display: Option<EffortDisplay>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub load_balancing_enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub load_balancing_weights: Option<LoadBalancingWeights>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub appearance_contrast: Option<AppearanceContrast>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub panel_animation_duration_ms: Option<PanelAnimationDurationMs>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub browser_default_viewport: Option<crate::preview::PreviewViewportSetting>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub browser_default_zoom_factor: Option<crate::preview::PreviewZoomFactor>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub browser_default_appearance: Option<crate::preview::PreviewAppearancePreference>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub browser_recording_frame_rate: Option<crate::preview::BrowserRecordingFrameRate>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub browser_recording_show_key_presses: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub browser_recording_show_mouse_presses: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub browser_link_target: Option<BrowserLinkTarget>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub browser_search_engine: Option<BrowserSearchEngine>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub browser_custom_search_url: Option<BrowserCustomSearchUrl>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub browser_auto_show_floating_preview: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub browser_profiles: Option<Vec<crate::browser_profile::BrowserProfile>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub browser_default_profile_id: Option<crate::browser_profile::BrowserProfileId>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub confirm_quit: Option<QuitConfirmationMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub confirm_thread_archive: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub confirm_thread_delete: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub confirm_thread_unpin: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub diff_files_collapsed: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub diff_ignore_whitespace: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub diff_layout: Option<DiffLayout>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub environment_identification_mode: Option<EnvironmentIdentificationMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub glass_opacity: Option<GlassOpacity>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub onboarding_completed_at: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub font_size_interface: Option<InterfaceFontSize>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub font_size_prompt: Option<PromptFontSize>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub font_size_code: Option<CodeFontSize>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub font_size_terminal: Option<TerminalFontSize>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub font_family_code: Option<FontFamilyPreference>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub font_family_composer: Option<FontFamilyPreference>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub font_family_sans: Option<FontFamilyPreference>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub font_family_terminal: Option<FontFamilyPreference>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub font_smoothing: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub persist_composer_context_strip: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub favorites: Option<ModelFavorites>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub provider_model_preferences: Option<ProviderModelPreferences>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub pull_request_merge_method_overrides: Option<
        std::collections::BTreeMap<
            crate::base_schemas::TrimmedNonEmptyString,
            crate::pull_request::PullRequestMergeMethod,
        >,
    >,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub pull_request_links_open_in_app: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub composer_rich_text_enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub send_shortcut: Option<ClientSettingsPatchSendShortcutValue>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub follow_up_behavior: Option<ClientSettingsPatchFollowUpBehaviorValue>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub proactive_panels_enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub show_skills_in_slash_menu: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub boring_sidebar_enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub boring_sidebar_multiline_rows: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub tool_call_visibility: Option<ToolCallVisibility>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub screen_reader_announcements: Option<ScreenReaderAnnouncements>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub screen_reader_window_title: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub thread_details_usage_enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub thread_details_items: Option<std::collections::BTreeMap<String, bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub sidebar_working_shelf_enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub sidebar_project_grouping_mode: Option<SidebarProjectGroupingMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub sidebar_project_grouping_overrides: Option<
        std::collections::BTreeMap<
            crate::base_schemas::TrimmedNonEmptyString,
            SidebarProjectGroupingMode,
        >,
    >,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub sidebar_project_sort_order: Option<SidebarProjectSortOrder>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub sidebar_thread_sort_order: Option<SidebarThreadSortOrder>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub timestamp_format: Option<TimestampFormat>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub voice_microphone: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub voice_microphones: Option<Vec<VoiceMicrophoneEntry>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub voice_shortcut_mode: Option<VoiceShortcutMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub speech_language: Option<crate::speech::SpeechLanguage>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub speech_translate_to_english: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub speech_custom_words: Option<crate::speech::SpeechCustomWords>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub speech_silence_filtering: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub speech_remove_filler_words: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub speech_post_processing_prompt: Option<crate::speech::SpeechPostProcessingPromptSettings>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub speech_correction_word: Option<crate::speech::SpeechCorrectionCue>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub language: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub snap_shot_enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub snap_shot_include_accessibility: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub snap_shot_shortcut: Option<SnapShotShortcut>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub snap_shot_play_sound: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub snap_shot_sound: Option<SnapShotSound>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub snap_shot_flash: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub snap_shot_animations: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub word_wrap: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub math_rendering_enabled: Option<bool>,
}
pub type QuitConfirmationModeSetting = QuitConfirmationMode;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientSettingsSchemaSendShortcut {
    #[serde(rename = "enter")]
    Enter,
    #[serde(rename = "mod-enter-multiline")]
    ModEnterMultiline,
    #[serde(rename = "mod-enter")]
    ModEnter,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientSettingsSchemaFollowUpBehavior {
    #[serde(rename = "queue")]
    Queue,
    #[serde(rename = "steer")]
    Steer,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientSettingsSchema {
    #[serde(default = "client_settings_schema_notification_mode_default")]
    pub notification_mode: NotificationMode,
    #[serde(default = "client_settings_schema_in_app_notifications_enabled_default")]
    pub in_app_notifications_enabled: bool,
    #[serde(default = "client_settings_schema_diff_color_scheme_default")]
    pub diff_color_scheme: DiffColorScheme,
    #[serde(default = "client_settings_schema_chat_width_default")]
    pub chat_width: ChatWidth,
    #[serde(default = "client_settings_schema_effort_display_default")]
    pub effort_display: EffortDisplay,
    #[serde(default = "client_settings_schema_load_balancing_enabled_default")]
    pub load_balancing_enabled: bool,
    #[serde(default = "client_settings_schema_load_balancing_weights_default")]
    pub load_balancing_weights: LoadBalancingWeights,
    #[serde(default = "client_settings_schema_appearance_contrast_default")]
    pub appearance_contrast: AppearanceContrast,
    #[serde(default = "client_settings_schema_panel_animation_duration_ms_default")]
    pub panel_animation_duration_ms: PanelAnimationDurationMs,
    #[serde(default = "client_settings_schema_browser_default_viewport_default")]
    pub browser_default_viewport: crate::preview::PreviewViewportSetting,
    #[serde(default = "client_settings_schema_browser_default_zoom_factor_default")]
    pub browser_default_zoom_factor: crate::preview::PreviewZoomFactor,
    #[serde(default = "client_settings_schema_browser_default_appearance_default")]
    pub browser_default_appearance: crate::preview::PreviewAppearancePreference,
    #[serde(default = "client_settings_schema_browser_recording_frame_rate_default")]
    pub browser_recording_frame_rate: crate::preview::BrowserRecordingFrameRate,
    #[serde(default = "client_settings_schema_browser_recording_show_key_presses_default")]
    pub browser_recording_show_key_presses: bool,
    #[serde(default = "client_settings_schema_browser_recording_show_mouse_presses_default")]
    pub browser_recording_show_mouse_presses: bool,
    #[serde(default = "client_settings_schema_browser_link_target_default")]
    pub browser_link_target: BrowserLinkTarget,
    #[serde(default = "client_settings_schema_browser_search_engine_default")]
    pub browser_search_engine: BrowserSearchEngine,
    #[serde(default = "client_settings_schema_browser_custom_search_url_default")]
    pub browser_custom_search_url: BrowserCustomSearchUrl,
    #[serde(default = "client_settings_schema_browser_auto_show_floating_preview_default")]
    pub browser_auto_show_floating_preview: bool,
    #[serde(default = "client_settings_schema_browser_profiles_default")]
    pub browser_profiles: Vec<crate::browser_profile::BrowserProfile>,
    #[serde(default = "client_settings_schema_browser_default_profile_id_default")]
    pub browser_default_profile_id: crate::browser_profile::BrowserProfileId,
    #[serde(
        default = "client_settings_schema_confirm_quit_default",
        deserialize_with = "deserialize_quit_confirmation"
    )]
    pub confirm_quit: QuitConfirmationModeSetting,
    #[serde(default = "client_settings_schema_confirm_thread_archive_default")]
    pub confirm_thread_archive: bool,
    #[serde(default = "client_settings_schema_confirm_thread_delete_default")]
    pub confirm_thread_delete: bool,
    #[serde(default = "client_settings_schema_confirm_thread_unpin_default")]
    pub confirm_thread_unpin: bool,
    #[serde(default = "client_settings_schema_diff_files_collapsed_default")]
    pub diff_files_collapsed: bool,
    #[serde(default = "client_settings_schema_diff_ignore_whitespace_default")]
    pub diff_ignore_whitespace: bool,
    #[serde(default = "client_settings_schema_diff_layout_default")]
    pub diff_layout: DiffLayout,
    #[serde(default = "client_settings_schema_environment_identification_mode_default")]
    pub environment_identification_mode: EnvironmentIdentificationMode,
    #[serde(default = "client_settings_schema_glass_opacity_default")]
    pub glass_opacity: GlassOpacity,
    #[serde(default = "client_settings_schema_font_size_interface_default")]
    pub font_size_interface: InterfaceFontSize,
    #[serde(default = "client_settings_schema_font_size_prompt_default")]
    pub font_size_prompt: PromptFontSize,
    #[serde(default = "client_settings_schema_font_size_code_default")]
    pub font_size_code: CodeFontSize,
    #[serde(default = "client_settings_schema_font_size_terminal_default")]
    pub font_size_terminal: TerminalFontSize,
    #[serde(default = "client_settings_schema_font_family_code_default")]
    pub font_family_code: FontFamilyPreference,
    #[serde(default = "client_settings_schema_font_family_composer_default")]
    pub font_family_composer: FontFamilyPreference,
    #[serde(default = "client_settings_schema_font_family_sans_default")]
    pub font_family_sans: FontFamilyPreference,
    #[serde(default = "client_settings_schema_font_family_terminal_default")]
    pub font_family_terminal: FontFamilyPreference,
    #[serde(default = "client_settings_schema_font_smoothing_default")]
    pub font_smoothing: bool,
    #[serde(
        default = "client_settings_schema_onboarding_completed_at_default",
        with = "crate::wire::nullable"
    )]
    pub onboarding_completed_at: Option<String>,
    #[serde(default = "client_settings_schema_persist_composer_context_strip_default")]
    pub persist_composer_context_strip: bool,
    #[serde(default = "client_settings_schema_favorites_default")]
    pub favorites: ModelFavorites,
    #[serde(default = "client_settings_schema_provider_model_preferences_default")]
    pub provider_model_preferences: ProviderModelPreferences,
    #[serde(default = "client_settings_schema_pull_request_merge_method_overrides_default")]
    pub pull_request_merge_method_overrides: std::collections::BTreeMap<
        crate::base_schemas::TrimmedNonEmptyString,
        crate::pull_request::PullRequestMergeMethod,
    >,
    #[serde(default = "client_settings_schema_pull_request_links_open_in_app_default")]
    pub pull_request_links_open_in_app: bool,
    #[serde(default = "client_settings_schema_composer_rich_text_enabled_default")]
    pub composer_rich_text_enabled: bool,
    #[serde(default = "client_settings_schema_send_shortcut_default")]
    pub send_shortcut: ClientSettingsSchemaSendShortcut,
    #[serde(default = "client_settings_schema_follow_up_behavior_default")]
    pub follow_up_behavior: ClientSettingsSchemaFollowUpBehavior,
    #[serde(default = "client_settings_schema_proactive_panels_enabled_default")]
    pub proactive_panels_enabled: bool,
    #[serde(default = "client_settings_schema_show_skills_in_slash_menu_default")]
    pub show_skills_in_slash_menu: bool,
    #[serde(default = "client_settings_schema_boring_sidebar_enabled_default")]
    pub boring_sidebar_enabled: bool,
    #[serde(default = "client_settings_schema_boring_sidebar_multiline_rows_default")]
    pub boring_sidebar_multiline_rows: bool,
    #[serde(default = "client_settings_schema_tool_call_visibility_default")]
    pub tool_call_visibility: ToolCallVisibility,
    #[serde(default = "client_settings_schema_screen_reader_announcements_default")]
    pub screen_reader_announcements: ScreenReaderAnnouncements,
    #[serde(default = "client_settings_schema_screen_reader_window_title_default")]
    pub screen_reader_window_title: bool,
    #[serde(default = "client_settings_schema_thread_details_usage_enabled_default")]
    pub thread_details_usage_enabled: bool,
    #[serde(default = "client_settings_schema_thread_details_items_default")]
    pub thread_details_items: std::collections::BTreeMap<String, bool>,
    #[serde(default = "client_settings_schema_sidebar_working_shelf_enabled_default")]
    pub sidebar_working_shelf_enabled: bool,
    #[serde(default = "client_settings_schema_sidebar_project_grouping_mode_default")]
    pub sidebar_project_grouping_mode: SidebarProjectGroupingMode,
    #[serde(default = "client_settings_schema_sidebar_project_grouping_overrides_default")]
    pub sidebar_project_grouping_overrides: std::collections::BTreeMap<
        crate::base_schemas::TrimmedNonEmptyString,
        SidebarProjectGroupingMode,
    >,
    #[serde(default = "client_settings_schema_sidebar_project_sort_order_default")]
    pub sidebar_project_sort_order: SidebarProjectSortOrder,
    #[serde(default = "client_settings_schema_sidebar_thread_sort_order_default")]
    pub sidebar_thread_sort_order: SidebarThreadSortOrder,
    #[serde(default = "client_settings_schema_timestamp_format_default")]
    pub timestamp_format: TimestampFormat,
    #[serde(default = "client_settings_schema_voice_microphone_default")]
    pub voice_microphone: String,
    #[serde(default = "client_settings_schema_voice_microphones_default")]
    pub voice_microphones: Vec<VoiceMicrophoneEntry>,
    #[serde(default = "client_settings_schema_voice_shortcut_mode_default")]
    pub voice_shortcut_mode: VoiceShortcutMode,
    #[serde(default = "client_settings_schema_speech_language_default")]
    pub speech_language: crate::speech::SpeechLanguage,
    #[serde(default = "client_settings_schema_speech_translate_to_english_default")]
    pub speech_translate_to_english: bool,
    #[serde(default = "client_settings_schema_speech_custom_words_default")]
    pub speech_custom_words: crate::speech::SpeechCustomWords,
    #[serde(default = "client_settings_schema_speech_silence_filtering_default")]
    pub speech_silence_filtering: bool,
    #[serde(default = "client_settings_schema_speech_remove_filler_words_default")]
    pub speech_remove_filler_words: bool,
    #[serde(default = "client_settings_schema_speech_post_processing_prompt_default")]
    pub speech_post_processing_prompt: crate::speech::SpeechPostProcessingPromptSettings,
    #[serde(default = "client_settings_schema_speech_correction_word_default")]
    pub speech_correction_word: crate::speech::SpeechCorrectionCue,
    #[serde(default = "client_settings_schema_language_default")]
    pub language: String,
    #[serde(default = "client_settings_schema_snap_shot_enabled_default")]
    pub snap_shot_enabled: bool,
    #[serde(default = "client_settings_schema_snap_shot_include_accessibility_default")]
    pub snap_shot_include_accessibility: bool,
    #[serde(default = "client_settings_schema_snap_shot_shortcut_default")]
    pub snap_shot_shortcut: SnapShotShortcut,
    #[serde(default = "client_settings_schema_snap_shot_play_sound_default")]
    pub snap_shot_play_sound: bool,
    #[serde(default = "client_settings_schema_snap_shot_sound_default")]
    pub snap_shot_sound: SnapShotSound,
    #[serde(default = "client_settings_schema_snap_shot_flash_default")]
    pub snap_shot_flash: bool,
    #[serde(default = "client_settings_schema_snap_shot_animations_default")]
    pub snap_shot_animations: bool,
    #[serde(default = "client_settings_schema_word_wrap_default")]
    pub word_wrap: bool,
    #[serde(default = "client_settings_schema_math_rendering_enabled_default")]
    pub math_rendering_enabled: bool,
}
impl Default for ClientSettingsSchema {
    fn default() -> Self {
        Self {
            notification_mode: client_settings_schema_notification_mode_default(),
            in_app_notifications_enabled:
                client_settings_schema_in_app_notifications_enabled_default(),
            diff_color_scheme: client_settings_schema_diff_color_scheme_default(),
            chat_width: client_settings_schema_chat_width_default(),
            effort_display: client_settings_schema_effort_display_default(),
            load_balancing_enabled: client_settings_schema_load_balancing_enabled_default(),
            load_balancing_weights: client_settings_schema_load_balancing_weights_default(),
            appearance_contrast: client_settings_schema_appearance_contrast_default(),
            panel_animation_duration_ms: client_settings_schema_panel_animation_duration_ms_default(
            ),
            browser_default_viewport: client_settings_schema_browser_default_viewport_default(),
            browser_default_zoom_factor: client_settings_schema_browser_default_zoom_factor_default(
            ),
            browser_default_appearance: client_settings_schema_browser_default_appearance_default(),
            browser_recording_frame_rate:
                client_settings_schema_browser_recording_frame_rate_default(),
            browser_recording_show_key_presses:
                client_settings_schema_browser_recording_show_key_presses_default(),
            browser_recording_show_mouse_presses:
                client_settings_schema_browser_recording_show_mouse_presses_default(),
            browser_link_target: client_settings_schema_browser_link_target_default(),
            browser_search_engine: client_settings_schema_browser_search_engine_default(),
            browser_custom_search_url: client_settings_schema_browser_custom_search_url_default(),
            browser_auto_show_floating_preview:
                client_settings_schema_browser_auto_show_floating_preview_default(),
            browser_profiles: client_settings_schema_browser_profiles_default(),
            browser_default_profile_id: client_settings_schema_browser_default_profile_id_default(),
            confirm_quit: client_settings_schema_confirm_quit_default(),
            confirm_thread_archive: client_settings_schema_confirm_thread_archive_default(),
            confirm_thread_delete: client_settings_schema_confirm_thread_delete_default(),
            confirm_thread_unpin: client_settings_schema_confirm_thread_unpin_default(),
            diff_files_collapsed: client_settings_schema_diff_files_collapsed_default(),
            diff_ignore_whitespace: client_settings_schema_diff_ignore_whitespace_default(),
            diff_layout: client_settings_schema_diff_layout_default(),
            environment_identification_mode:
                client_settings_schema_environment_identification_mode_default(),
            glass_opacity: client_settings_schema_glass_opacity_default(),
            font_size_interface: client_settings_schema_font_size_interface_default(),
            font_size_prompt: client_settings_schema_font_size_prompt_default(),
            font_size_code: client_settings_schema_font_size_code_default(),
            font_size_terminal: client_settings_schema_font_size_terminal_default(),
            font_family_code: client_settings_schema_font_family_code_default(),
            font_family_composer: client_settings_schema_font_family_composer_default(),
            font_family_sans: client_settings_schema_font_family_sans_default(),
            font_family_terminal: client_settings_schema_font_family_terminal_default(),
            font_smoothing: client_settings_schema_font_smoothing_default(),
            onboarding_completed_at: client_settings_schema_onboarding_completed_at_default(),
            persist_composer_context_strip:
                client_settings_schema_persist_composer_context_strip_default(),
            favorites: client_settings_schema_favorites_default(),
            provider_model_preferences: client_settings_schema_provider_model_preferences_default(),
            pull_request_merge_method_overrides:
                client_settings_schema_pull_request_merge_method_overrides_default(),
            pull_request_links_open_in_app:
                client_settings_schema_pull_request_links_open_in_app_default(),
            composer_rich_text_enabled: client_settings_schema_composer_rich_text_enabled_default(),
            send_shortcut: client_settings_schema_send_shortcut_default(),
            follow_up_behavior: client_settings_schema_follow_up_behavior_default(),
            proactive_panels_enabled: client_settings_schema_proactive_panels_enabled_default(),
            show_skills_in_slash_menu: client_settings_schema_show_skills_in_slash_menu_default(),
            boring_sidebar_enabled: client_settings_schema_boring_sidebar_enabled_default(),
            boring_sidebar_multiline_rows:
                client_settings_schema_boring_sidebar_multiline_rows_default(),
            tool_call_visibility: client_settings_schema_tool_call_visibility_default(),
            screen_reader_announcements: client_settings_schema_screen_reader_announcements_default(
            ),
            screen_reader_window_title: client_settings_schema_screen_reader_window_title_default(),
            thread_details_usage_enabled:
                client_settings_schema_thread_details_usage_enabled_default(),
            thread_details_items: client_settings_schema_thread_details_items_default(),
            sidebar_working_shelf_enabled:
                client_settings_schema_sidebar_working_shelf_enabled_default(),
            sidebar_project_grouping_mode:
                client_settings_schema_sidebar_project_grouping_mode_default(),
            sidebar_project_grouping_overrides:
                client_settings_schema_sidebar_project_grouping_overrides_default(),
            sidebar_project_sort_order: client_settings_schema_sidebar_project_sort_order_default(),
            sidebar_thread_sort_order: client_settings_schema_sidebar_thread_sort_order_default(),
            timestamp_format: client_settings_schema_timestamp_format_default(),
            voice_microphone: client_settings_schema_voice_microphone_default(),
            voice_microphones: client_settings_schema_voice_microphones_default(),
            voice_shortcut_mode: client_settings_schema_voice_shortcut_mode_default(),
            speech_language: client_settings_schema_speech_language_default(),
            speech_translate_to_english: client_settings_schema_speech_translate_to_english_default(
            ),
            speech_custom_words: client_settings_schema_speech_custom_words_default(),
            speech_silence_filtering: client_settings_schema_speech_silence_filtering_default(),
            speech_remove_filler_words: client_settings_schema_speech_remove_filler_words_default(),
            speech_post_processing_prompt:
                client_settings_schema_speech_post_processing_prompt_default(),
            speech_correction_word: client_settings_schema_speech_correction_word_default(),
            language: client_settings_schema_language_default(),
            snap_shot_enabled: client_settings_schema_snap_shot_enabled_default(),
            snap_shot_include_accessibility:
                client_settings_schema_snap_shot_include_accessibility_default(),
            snap_shot_shortcut: client_settings_schema_snap_shot_shortcut_default(),
            snap_shot_play_sound: client_settings_schema_snap_shot_play_sound_default(),
            snap_shot_sound: client_settings_schema_snap_shot_sound_default(),
            snap_shot_flash: client_settings_schema_snap_shot_flash_default(),
            snap_shot_animations: client_settings_schema_snap_shot_animations_default(),
            word_wrap: client_settings_schema_word_wrap_default(),
            math_rendering_enabled: client_settings_schema_math_rendering_enabled_default(),
        }
    }
}
pub type CloudSpeechCredential = crate::settings_secret_value::SettingsSecretValue;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSpeechCredentials {
    #[serde(default = "cloud_speech_credentials_openai_default")]
    pub openai: CloudSpeechCredential,
    #[serde(default = "cloud_speech_credentials_xai_default")]
    pub xai: CloudSpeechCredential,
    #[serde(default = "cloud_speech_credentials_openrouter_default")]
    pub openrouter: CloudSpeechCredential,
}
impl Default for CloudSpeechCredentials {
    fn default() -> Self {
        Self {
            openai: cloud_speech_credentials_openai_default(),
            xai: cloud_speech_credentials_xai_default(),
            openrouter: cloud_speech_credentials_openrouter_default(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CodexSettingsSetupModeValue {
    #[serde(rename = "managed")]
    Managed,
    #[serde(rename = "existing")]
    Existing,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSettings {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub setup_mode: Option<CodexSettingsSetupModeValue>,
    #[serde(default = "codex_settings_enabled_default")]
    pub enabled: bool,
    #[serde(default = "codex_settings_binary_path_default")]
    pub binary_path: String,
    #[serde(default = "codex_settings_home_path_default")]
    pub home_path: crate::base_schemas::TrimmedString,
    #[serde(default = "codex_settings_shadow_home_path_default")]
    pub shadow_home_path: crate::base_schemas::TrimmedString,
    #[serde(default = "codex_settings_launch_args_default")]
    pub launch_args: crate::base_schemas::TrimmedString,
    #[serde(default = "codex_settings_custom_models_default")]
    pub custom_models: Vec<crate::model::CustomModelSetting>,
}
impl Default for CodexSettings {
    fn default() -> Self {
        Self {
            setup_mode: None,
            enabled: codex_settings_enabled_default(),
            binary_path: codex_settings_binary_path_default(),
            home_path: codex_settings_home_path_default(),
            shadow_home_path: codex_settings_shadow_home_path_default(),
            launch_args: codex_settings_launch_args_default(),
            custom_models: codex_settings_custom_models_default(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DatabaseReclaimErrorTag {
    #[serde(rename = "DatabaseReclaimError")]
    DatabaseReclaimError,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseReclaimError {
    #[serde(rename = "_tag")]
    pub _tag: DatabaseReclaimErrorTag,
    #[serde()]
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseStorageStatus {
    #[serde()]
    pub database_bytes: WireNumber,
    #[serde()]
    pub reclaimable_bytes: WireNumber,
    #[serde()]
    pub incremental_vacuum_enabled: bool,
    #[serde()]
    pub compact_command: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub can_reclaim: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub reclaim_failure: Option<String>,
}
pub type DefaultThemePreference = String;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum IdleCompactionMode {
    #[serde(rename = "onReturn")]
    OnReturn,
    #[serde(rename = "beforeCacheExpires")]
    BeforeCacheExpires,
    #[serde(rename = "off")]
    Off,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TextGeneratorKind {
    #[serde(rename = "openaiCompatible")]
    OpenaiCompatible,
    #[serde(rename = "anthropic")]
    Anthropic,
}
pub type TextGeneratorBaseUrl = crate::base_schemas::TrimmedNonEmptyString;
/// TypeScript's `TextGeneratorBaseUrl`: at most 2,048 units, and an http(s) URL without user
/// info, query or fragment as WHATWG `new URL` reads it (an empty `?` or `#` is allowed).
pub fn text_generator_base_url_valid(value: &str) -> bool {
    let acceptable = |url: url::Url| {
        matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none_or(str::is_empty)
            && url.query().is_none_or(str::is_empty)
            && url.fragment().is_none_or(str::is_empty)
    };
    value.encode_utf16().count() <= 2048
        && match url::Url::parse(value) {
            Ok(url) => acceptable(url),
            // Node's URL and this crate's UTS 46 disagree on some labels (Node accepts
            // `xn--a`), so a host refused by IDNA alone is judged with IDNA taken out:
            // non-ASCII becomes `a` and `xn--` becomes `xa--`; every delimiter stays.
            Err(url::ParseError::IdnaError) => {
                let mut neutral: String = value
                    .chars()
                    .map(|ch| if ch.is_ascii() { ch } else { 'a' })
                    .collect();
                let lower = neutral.to_ascii_lowercase();
                for (at, _) in lower.match_indices("xn--") {
                    neutral.replace_range(at..at + 4, "xa--");
                }
                url::Url::parse(&neutral).is_ok_and(acceptable)
            }
            Err(_) => false,
        }
}
fn deserialize_text_generator_base_url<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<TextGeneratorBaseUrl, D::Error> {
    let value = TextGeneratorBaseUrl::deserialize(deserializer)?;
    if text_generator_base_url_valid(&value.0) {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "expected an http(s) URL without user info, query or fragment",
        ))
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextGeneratorConfig {
    #[serde()]
    pub kind: TextGeneratorKind,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub label: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(deserialize_with = "deserialize_text_generator_base_url")]
    pub base_url: TextGeneratorBaseUrl,
    #[serde(default = "text_generator_config_api_key_default")]
    pub api_key: crate::settings_secret_value::SettingsSecretValue,
    #[serde(default = "text_generator_config_enabled_default")]
    pub enabled: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListTextGeneratorModelsInput {
    #[serde()]
    pub id: crate::provider_instance::ProviderInstanceId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub config: Option<Option<TextGeneratorConfig>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub refresh: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub test: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub model: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextGeneratorModel {
    #[serde()]
    pub id: crate::base_schemas::TrimmedNonEmptyString,
    #[serde()]
    pub name: crate::base_schemas::TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListTextGeneratorModelsResult {
    #[serde()]
    pub models: Vec<TextGeneratorModel>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tested_model: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilitySettings {
    #[serde(default = "observability_settings_otlp_traces_url_default")]
    pub otlp_traces_url: crate::base_schemas::TrimmedString,
    #[serde(default = "observability_settings_otlp_metrics_url_default")]
    pub otlp_metrics_url: crate::base_schemas::TrimmedString,
    #[serde(default = "observability_settings_otlp_logs_url_default")]
    pub otlp_logs_url: crate::base_schemas::TrimmedString,
}
impl Default for ObservabilitySettings {
    fn default() -> Self {
        Self {
            otlp_traces_url: observability_settings_otlp_traces_url_default(),
            otlp_metrics_url: observability_settings_otlp_metrics_url_default(),
            otlp_logs_url: observability_settings_otlp_logs_url_default(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectProviderInstanceOverride {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub enabled: Option<bool>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WorktreeCleanupValueValueMember1Mode {
    #[serde(rename = "off")]
    Off,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeCleanupValueValueMember1 {
    #[serde()]
    pub mode: WorktreeCleanupValueValueMember1Mode,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WorktreeCleanupValueValueMember2Mode {
    #[serde(rename = "custom")]
    Custom,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WorktreeKeepWhen {
    #[serde(rename = "any-local-files")]
    AnyLocalFiles,
    #[serde(rename = "uncommitted-changes")]
    UncommittedChanges,
    #[serde(rename = "tracked-changes")]
    TrackedChanges,
}
pub type StorageRetentionDays = Option<i64>;
pub type SettledWorktreeRetentionDays = Option<i64>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeCleanupRules {
    #[serde(default = "worktree_cleanup_rules_worktree_keep_when_default")]
    pub worktree_keep_when: WorktreeKeepWhen,
    #[serde(with = "crate::wire::nullable")]
    pub worktree_after_days: StorageRetentionDays,
    #[serde(
        default = "worktree_cleanup_rules_worktree_settled_after_days_default",
        with = "crate::wire::nullable"
    )]
    pub worktree_settled_after_days: SettledWorktreeRetentionDays,
    #[serde()]
    pub worktree_on_merge: bool,
    #[serde()]
    pub worktree_on_delete: bool,
    #[serde()]
    pub worktree_unchanged: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeCleanupValueValueMember2 {
    #[serde()]
    pub mode: WorktreeCleanupValueValueMember2Mode,
    #[serde()]
    pub rules: WorktreeCleanupRules,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorktreeCleanupValueValue {
    Off1(Box<WorktreeCleanupValueValueMember1>),
    Custom2(Box<WorktreeCleanupValueValueMember2>),
}
pub type WorktreeCleanup = Option<WorktreeCleanupValueValue>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SourceControlWritingStyleMode {
    #[serde(rename = "repo_conventions")]
    RepoConventions,
    #[serde(rename = "conventional_commits")]
    ConventionalCommits,
    #[serde(rename = "custom")]
    Custom,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlWritingStyleSettings {
    #[serde(default = "source_control_writing_style_settings_mode_default")]
    pub mode: SourceControlWritingStyleMode,
    #[serde(default = "source_control_writing_style_settings_custom_instructions_default")]
    pub custom_instructions: crate::base_schemas::TrimmedString,
    #[serde(
        default = "source_control_writing_style_settings_follow_change_request_templates_default"
    )]
    pub follow_change_request_templates: bool,
}
impl Default for SourceControlWritingStyleSettings {
    fn default() -> Self {
        Self {
            mode: source_control_writing_style_settings_mode_default(),
            custom_instructions: source_control_writing_style_settings_custom_instructions_default(
            ),
            follow_change_request_templates:
                source_control_writing_style_settings_follow_change_request_templates_default(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(transparent)]
pub struct SourceControlWritingStyleSetting(pub SourceControlWritingStyleSettings);
impl<'de> Deserialize<'de> for SourceControlWritingStyleSetting {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_writing_style(deserializer).map(Self)
    }
}
pub type SidebarAutoSettleAfterDays = crate::wire::JsNumber;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResponseStreamingMode {
    #[serde(rename = "turn")]
    Turn,
    #[serde(rename = "paragraph")]
    Paragraph,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSettingsOverrides {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub worktree_cleanup: Option<WorktreeCleanup>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub default_model_selection: Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub default_runtime_mode: Option<crate::provider_policy::RuntimeMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub default_thread_env_mode: Option<crate::environment::ThreadEnvMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub new_worktrees_start_from_origin: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::lenient"
    )]
    pub worktree_submodules: Option<crate::environment::WorktreeSubmodules>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub default_auto_pull: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub default_project_scripts: Option<Vec<crate::project::ProjectScript>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub enable_agent_browser_access: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub enable_agent_device_access: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub text_generation_model_selection: Option<crate::model_selection::ModelSelection>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub thread_title_instructions: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub source_control_writer_model_selection:
        Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub review_guide_model_selection: Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub review_guide_instructions: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub source_control_writing_style: Option<SourceControlWritingStyleSetting>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub remove_agent_credits_on_merge: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub branch_naming_mode: Option<BranchNamingMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub branch_name_prefix: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub branch_name_instructions: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub pull_request_merge_method: Option<Option<crate::pull_request::PullRequestMergeMethod>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub sidebar_auto_settle_on_merge: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub sidebar_auto_settle_after_days: Option<Option<SidebarAutoSettleAfterDays>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub sidebar_auto_settle_keep_pull_requests: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub continue_threads_after_server_update: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub response_streaming_mode: Option<ResponseStreamingMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub speech_project_custom_words: Option<crate::speech::SpeechCustomWords>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub provider_instances: Option<
        std::collections::BTreeMap<
            crate::provider_instance::ProviderInstanceId,
            ProjectProviderInstanceOverride,
        >,
    >,
}
pub type SecretProviderKind = crate::secret_reference::SecretProviderSlug;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretProviderConfig {
    #[serde()]
    pub kind: SecretProviderKind,
    #[serde()]
    pub label: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub config: Option<serde_json::Value>,
}
pub type SecretProviderLabel = crate::base_schemas::TrimmedNonEmptyString;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCleanupSettings {
    #[serde(default = "storage_cleanup_settings_worktree_keep_when_default")]
    pub worktree_keep_when: WorktreeKeepWhen,
    #[serde(
        default = "storage_cleanup_settings_worktree_after_days_default",
        with = "crate::wire::nullable"
    )]
    pub worktree_after_days: StorageRetentionDays,
    #[serde(
        default = "storage_cleanup_settings_worktree_settled_after_days_default",
        with = "crate::wire::nullable"
    )]
    pub worktree_settled_after_days: SettledWorktreeRetentionDays,
    #[serde(default = "storage_cleanup_settings_worktree_on_merge_default")]
    pub worktree_on_merge: bool,
    #[serde(default = "storage_cleanup_settings_worktree_on_delete_default")]
    pub worktree_on_delete: bool,
    #[serde(default = "storage_cleanup_settings_worktree_unchanged_default")]
    pub worktree_unchanged: bool,
    #[serde(
        default = "storage_cleanup_settings_browser_artifacts_after_days_default",
        with = "crate::wire::nullable"
    )]
    pub browser_artifacts_after_days: StorageRetentionDays,
    #[serde(
        default = "storage_cleanup_settings_logs_after_days_default",
        with = "crate::wire::nullable"
    )]
    pub logs_after_days: StorageRetentionDays,
}
impl Default for StorageCleanupSettings {
    fn default() -> Self {
        Self {
            worktree_keep_when: storage_cleanup_settings_worktree_keep_when_default(),
            worktree_after_days: storage_cleanup_settings_worktree_after_days_default(),
            worktree_settled_after_days:
                storage_cleanup_settings_worktree_settled_after_days_default(),
            worktree_on_merge: storage_cleanup_settings_worktree_on_merge_default(),
            worktree_on_delete: storage_cleanup_settings_worktree_on_delete_default(),
            worktree_unchanged: storage_cleanup_settings_worktree_unchanged_default(),
            browser_artifacts_after_days:
                storage_cleanup_settings_browser_artifacts_after_days_default(),
            logs_after_days: storage_cleanup_settings_logs_after_days_default(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettingsProjectSettingsOverridesValue {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub worktree_cleanup: Option<WorktreeCleanup>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub default_model_selection: Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub default_runtime_mode: Option<crate::provider_policy::RuntimeMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub default_thread_env_mode: Option<crate::environment::ThreadEnvMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub new_worktrees_start_from_origin: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::lenient"
    )]
    pub worktree_submodules: Option<crate::environment::WorktreeSubmodules>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub default_auto_pull: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub default_project_scripts: Option<Vec<crate::project::ProjectScript>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub enable_agent_browser_access: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub enable_agent_device_access: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub text_generation_model_selection: Option<crate::model_selection::ModelSelection>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub thread_title_instructions: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub source_control_writer_model_selection:
        Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub review_guide_model_selection: Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub review_guide_instructions: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub source_control_writing_style: Option<SourceControlWritingStyleSetting>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub remove_agent_credits_on_merge: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub branch_naming_mode: Option<BranchNamingMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub branch_name_prefix: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub branch_name_instructions: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub pull_request_merge_method: Option<Option<crate::pull_request::PullRequestMergeMethod>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub sidebar_auto_settle_on_merge: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub sidebar_auto_settle_after_days: Option<Option<SidebarAutoSettleAfterDays>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub sidebar_auto_settle_keep_pull_requests: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub continue_threads_after_server_update: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub response_streaming_mode: Option<ResponseStreamingMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub speech_project_custom_words: Option<crate::speech::SpeechCustomWords>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub provider_instances: Option<
        std::collections::BTreeMap<
            crate::provider_instance::ProviderInstanceId,
            ProjectProviderInstanceOverride,
        >,
    >,
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LegacyCompactIdleThreads {
    Member1(Box<bool>),
    IdleCompactionMode(Box<IdleCompactionMode>),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SnoozePresetSettingMember1Kind {
    #[serde(rename = "after")]
    After,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnoozePresetSettingMember1 {
    #[serde()]
    pub kind: SnoozePresetSettingMember1Kind,
    #[serde()]
    pub minutes: i64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SnoozePresetSettingMember2Kind {
    #[serde(rename = "at")]
    At,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SnoozePresetDay {
    #[serde(rename = "today")]
    Today,
    #[serde(rename = "tomorrow")]
    Tomorrow,
    #[serde(rename = "monday")]
    Monday,
    #[serde(rename = "tuesday")]
    Tuesday,
    #[serde(rename = "wednesday")]
    Wednesday,
    #[serde(rename = "thursday")]
    Thursday,
    #[serde(rename = "friday")]
    Friday,
    #[serde(rename = "saturday")]
    Saturday,
    #[serde(rename = "sunday")]
    Sunday,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnoozePresetSettingMember2 {
    #[serde()]
    pub kind: SnoozePresetSettingMember2Kind,
    #[serde()]
    pub day: SnoozePresetDay,
    #[serde()]
    pub time: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SnoozePresetSetting {
    After1(Box<SnoozePresetSettingMember1>),
    At2(Box<SnoozePresetSettingMember2>),
}
pub type SnoozePresetSettings = Vec<SnoozePresetSetting>;
pub type SourceControlHostSettings = std::collections::BTreeMap<String, serde_json::Value>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UsageLimitSourceConfigKind {
    #[serde(rename = "cliproxy")]
    Cliproxy,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimitSourceConfig {
    #[serde()]
    pub kind: UsageLimitSourceConfigKind,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub label: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde()]
    pub url: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(default = "usage_limit_source_config_management_key_default")]
    pub management_key: crate::settings_secret_value::SettingsSecretValue,
    #[serde(default = "usage_limit_source_config_enabled_default")]
    pub enabled: bool,
}
pub type UsageModelTokenPrice = crate::wire::JsNumber;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageModelPriceOverride {
    #[serde()]
    pub input_cost_per_million_tokens: UsageModelTokenPrice,
    #[serde()]
    pub output_cost_per_million_tokens: UsageModelTokenPrice,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub cache_read_cost_per_million_tokens: Option<UsageModelTokenPrice>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub cache_write_cost_per_million_tokens: Option<UsageModelTokenPrice>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettings {
    #[serde(
        default = "server_settings_favorites_default",
        with = "crate::wire::nullable"
    )]
    pub favorites: Option<ModelFavorites>,
    #[serde(
        default = "server_settings_provider_model_preferences_default",
        with = "crate::wire::nullable"
    )]
    pub provider_model_preferences: Option<ProviderModelPreferences>,
    #[serde(
        default = "server_settings_worktree_cleanup_default",
        with = "crate::wire::nullable"
    )]
    pub worktree_cleanup: WorktreeCleanup,
    #[serde(default = "server_settings_storage_cleanup_default")]
    pub storage_cleanup: StorageCleanupSettings,
    #[serde(default = "server_settings_worktrees_directory_default")]
    pub worktrees_directory: crate::base_schemas::TrimmedString,
    #[serde(default = "server_settings_fast_worktree_checkout_default")]
    pub fast_worktree_checkout: bool,
    #[serde(default = "server_settings_previous_worktrees_directories_default")]
    pub previous_worktrees_directories: Vec<crate::base_schemas::TrimmedString>,
    #[serde(default = "server_settings_response_streaming_mode_default")]
    pub response_streaming_mode: ResponseStreamingMode,
    #[serde(default = "server_settings_enable_provider_update_checks_default")]
    pub enable_provider_update_checks: bool,
    #[serde(default = "server_settings_continue_threads_after_server_update_default")]
    pub continue_threads_after_server_update: bool,
    #[serde(default = "server_settings_keep_awake_while_agents_work_default")]
    pub keep_awake_while_agents_work: bool,
    #[serde(default = "server_settings_enable_agent_browser_access_default")]
    pub enable_agent_browser_access: bool,
    #[serde(default = "server_settings_cloud_speech_credentials_default")]
    pub cloud_speech_credentials: CloudSpeechCredentials,
    #[serde(default = "server_settings_speech_model_id_default")]
    pub speech_model_id: String,
    #[serde(default = "server_settings_speech_acceleration_default")]
    pub speech_acceleration: crate::speech::SpeechAcceleration,
    #[serde(default = "server_settings_speech_model_unload_timeout_default")]
    pub speech_model_unload_timeout: crate::speech::SpeechModelUnloadTimeout,
    #[serde(default = "server_settings_speech_project_custom_words_default")]
    pub speech_project_custom_words: crate::speech::SpeechCustomWords,
    #[serde(
        default = "server_settings_speech_post_processing_model_selection_default",
        with = "crate::wire::nullable"
    )]
    pub speech_post_processing_model_selection: Option<crate::model_selection::ModelSelection>,
    #[serde(default = "server_settings_project_agent_browser_access_overrides_default")]
    pub project_agent_browser_access_overrides:
        std::collections::BTreeMap<crate::base_schemas::ProjectId, bool>,
    #[serde(default = "server_settings_default_auto_pull_default")]
    pub default_auto_pull: bool,
    #[serde(default = "server_settings_default_project_scripts_default")]
    pub default_project_scripts: Vec<crate::project::ProjectScript>,
    #[serde(default = "server_settings_project_script_overrides_default")]
    pub project_script_overrides: std::collections::BTreeMap<
        crate::base_schemas::ProjectId,
        Option<Vec<crate::project::ProjectScript>>,
    >,
    #[serde(default = "server_settings_project_auto_pull_overrides_default")]
    pub project_auto_pull_overrides:
        std::collections::BTreeMap<crate::base_schemas::ProjectId, bool>,
    #[serde(
        default = "server_settings_default_model_selection_default",
        with = "crate::wire::nullable"
    )]
    pub default_model_selection: Option<crate::model_selection::ModelSelection>,
    #[serde(
        default = "server_settings_fallback_model_selection_default",
        with = "crate::wire::nullable"
    )]
    pub fallback_model_selection: Option<crate::model_selection::ModelSelection>,
    #[serde(default = "server_settings_default_runtime_mode_default")]
    pub default_runtime_mode: crate::provider_policy::RuntimeMode,
    #[serde(default = "server_settings_project_settings_overrides_default")]
    pub project_settings_overrides: std::collections::BTreeMap<
        crate::base_schemas::ProjectId,
        ServerSettingsProjectSettingsOverridesValue,
    >,
    #[serde(default = "server_settings_project_settings_folded_default")]
    pub project_settings_folded: bool,
    #[serde(default = "server_settings_enable_agent_device_access_default")]
    pub enable_agent_device_access: bool,
    #[serde(default = "server_settings_enable_device_support_default")]
    pub enable_device_support: bool,
    #[serde(default = "server_settings_device_onboarding_completed_default")]
    pub device_onboarding_completed: bool,
    #[serde(default = "server_settings_device_hosts_default")]
    pub device_hosts: crate::device::SshDeviceHostConfigs,
    #[serde(default = "server_settings_eas_device_hosts_default")]
    pub eas_device_hosts: crate::device::EasDeviceHostConfigs,
    #[serde(default = "server_settings_simlock_device_hosts_default")]
    pub simlock_device_hosts: crate::device::SimlockDeviceHostConfigs,
    #[serde(default = "server_settings_simlock_device_hosts_enabled_default")]
    pub simlock_device_hosts_enabled: bool,
    #[serde(
        default = "server_settings_sidebar_auto_settle_after_days_default",
        with = "crate::wire::nullable"
    )]
    pub sidebar_auto_settle_after_days: Option<SidebarAutoSettleAfterDays>,
    #[serde(default = "server_settings_compact_idle_threads_default")]
    pub compact_idle_threads: LegacyCompactIdleThreads,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub idle_compaction: Option<IdleCompactionMode>,
    #[serde(default = "server_settings_snooze_limited_threads_default")]
    pub snooze_limited_threads: bool,
    #[serde(default = "server_settings_auto_resume_limited_threads_default")]
    pub auto_resume_limited_threads: bool,
    #[serde(default = "server_settings_snooze_presets_default")]
    pub snooze_presets: SnoozePresetSettings,
    #[serde(
        default = "server_settings_hidden_thread_tags_default",
        deserialize_with = "crate::wire::deserialize_lenient_array"
    )]
    pub hidden_thread_tags: Vec<String>,
    #[serde(default = "server_settings_hide_tagged_threads_default")]
    pub hide_tagged_threads: bool,
    #[serde(default = "server_settings_guest_sharing_enabled_default")]
    pub guest_sharing_enabled: bool,
    #[serde(default = "server_settings_linked_environments_enabled_default")]
    pub linked_environments_enabled: bool,
    #[serde(default = "server_settings_prompt_suggestions_enabled_default")]
    pub prompt_suggestions_enabled: bool,
    #[serde(default = "server_settings_sidebar_auto_settle_keep_pull_requests_default")]
    pub sidebar_auto_settle_keep_pull_requests: bool,
    #[serde(default = "server_settings_sidebar_auto_settle_on_merge_default")]
    pub sidebar_auto_settle_on_merge: bool,
    #[serde(default = "server_settings_background_activity_default")]
    pub background_activity: BackgroundActivitySettings,
    #[serde(default = "server_settings_automatic_git_fetch_interval_default")]
    pub automatic_git_fetch_interval: WireNumber,
    #[serde(default = "server_settings_provider_health_refresh_interval_default")]
    pub provider_health_refresh_interval: WireNumber,
    #[serde(default = "server_settings_background_activity_profile_default")]
    pub background_activity_profile: BackgroundActivityProfile,
    #[serde(default = "server_settings_default_theme_default")]
    pub default_theme: DefaultThemePreference,
    #[serde(default = "server_settings_default_theme_set_at_default")]
    pub default_theme_set_at: String,
    #[serde(
        default = "server_settings_environment_icon_default",
        with = "crate::wire::lenient"
    )]
    pub environment_icon: Option<crate::environment::EnvironmentMachineKind>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::lenient"
    )]
    pub default_thread_env_mode: Option<crate::environment::ThreadEnvMode>,
    #[serde(default = "server_settings_new_worktrees_start_from_origin_default")]
    pub new_worktrees_start_from_origin: bool,
    #[serde(
        default = "server_settings_worktree_submodules_default",
        with = "crate::wire::lenient"
    )]
    pub worktree_submodules: Option<crate::environment::WorktreeSubmodules>,
    #[serde(default = "server_settings_add_project_base_directory_default")]
    pub add_project_base_directory: crate::base_schemas::TrimmedString,
    #[serde(default = "server_settings_ghq_clone_layout_default")]
    pub ghq_clone_layout: bool,
    #[serde(default = "server_settings_text_generation_model_selection_default")]
    pub text_generation_model_selection: crate::model_selection::ModelSelection,
    #[serde(default = "server_settings_thread_title_instructions_default")]
    pub thread_title_instructions: crate::base_schemas::TrimmedString,
    #[serde(default = "server_settings_branch_naming_mode_default")]
    pub branch_naming_mode: BranchNamingMode,
    #[serde(default = "server_settings_branch_name_prefix_default")]
    pub branch_name_prefix: crate::base_schemas::TrimmedString,
    #[serde(default = "server_settings_branch_name_instructions_default")]
    pub branch_name_instructions: crate::base_schemas::TrimmedString,
    #[serde(default = "server_settings_remove_agent_credits_on_merge_default")]
    pub remove_agent_credits_on_merge: bool,
    #[serde(default = "server_settings_source_control_writing_style_default")]
    pub source_control_writing_style: SourceControlWritingStyleSetting,
    #[serde(
        default = "server_settings_source_control_writer_model_selection_default",
        with = "crate::wire::nullable"
    )]
    pub source_control_writer_model_selection: Option<crate::model_selection::ModelSelection>,
    #[serde(default = "server_settings_review_guides_enabled_default")]
    pub review_guides_enabled: bool,
    #[serde(
        default = "server_settings_review_guide_model_selection_default",
        with = "crate::wire::nullable"
    )]
    pub review_guide_model_selection: Option<crate::model_selection::ModelSelection>,
    #[serde(default = "server_settings_review_guide_instructions_default")]
    pub review_guide_instructions: crate::base_schemas::TrimmedString,
    #[serde(
        default = "server_settings_pull_request_merge_method_default",
        with = "crate::wire::nullable"
    )]
    pub pull_request_merge_method: Option<crate::pull_request::PullRequestMergeMethod>,
    #[serde(default = "server_settings_provider_instances_default")]
    pub provider_instances: std::collections::BTreeMap<
        crate::provider_instance::ProviderInstanceId,
        crate::provider_instance::ProviderInstanceConfig,
    >,
    #[serde(default = "server_settings_observability_default")]
    pub observability: ObservabilitySettings,
    #[serde(default = "server_settings_source_control_hosts_default")]
    pub source_control_hosts: std::collections::BTreeMap<String, SourceControlHostSettings>,
    #[serde(default = "server_settings_offered_source_control_providers_default")]
    pub offered_source_control_providers: std::collections::BTreeMap<String, bool>,
    #[serde(default = "server_settings_usage_limit_sources_default")]
    pub usage_limit_sources: std::collections::BTreeMap<
        crate::usage_limit_source_id::UsageLimitSourceId,
        UsageLimitSourceConfig,
    >,
    #[serde(default = "server_settings_text_generators_default")]
    pub text_generators: std::collections::BTreeMap<
        crate::provider_instance::ProviderInstanceId,
        TextGeneratorConfig,
    >,
    #[serde(default = "server_settings_secret_providers_default")]
    pub secret_providers:
        std::collections::BTreeMap<crate::secret_reference::SecretProviderId, SecretProviderConfig>,
    #[serde(default = "server_settings_usage_price_overrides_default")]
    pub usage_price_overrides: std::collections::BTreeMap<
        crate::base_schemas::TrimmedNonEmptyString,
        UsageModelPriceOverride,
    >,
    #[serde(default = "server_settings_usage_model_aliases_default")]
    pub usage_model_aliases: std::collections::BTreeMap<
        crate::base_schemas::TrimmedNonEmptyString,
        crate::base_schemas::TrimmedNonEmptyString,
    >,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::lenient"
    )]
    pub usage_model_filter: Option<crate::usage::UsageModelFilter>,
    #[serde(default, deserialize_with = "deserialize_default_false")]
    pub require_connect_device_approval: bool,
    #[serde(default, deserialize_with = "deserialize_default_appearance")]
    pub default_appearance: DefaultAppearancePreference,
    #[serde(default, deserialize_with = "deserialize_default_appearance_set_at")]
    pub default_appearance_set_at: String,
}
impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            favorites: server_settings_favorites_default(),
            provider_model_preferences: server_settings_provider_model_preferences_default(),
            worktree_cleanup: server_settings_worktree_cleanup_default(),
            storage_cleanup: server_settings_storage_cleanup_default(),
            worktrees_directory: server_settings_worktrees_directory_default(),
            fast_worktree_checkout: server_settings_fast_worktree_checkout_default(),
            previous_worktrees_directories: server_settings_previous_worktrees_directories_default(
            ),
            response_streaming_mode: server_settings_response_streaming_mode_default(),
            enable_provider_update_checks: server_settings_enable_provider_update_checks_default(),
            continue_threads_after_server_update:
                server_settings_continue_threads_after_server_update_default(),
            keep_awake_while_agents_work: server_settings_keep_awake_while_agents_work_default(),
            require_connect_device_approval: false,
            default_appearance: DefaultAppearancePreference::Unset,
            default_appearance_set_at: String::new(),
            enable_agent_browser_access: server_settings_enable_agent_browser_access_default(),
            cloud_speech_credentials: server_settings_cloud_speech_credentials_default(),
            speech_model_id: server_settings_speech_model_id_default(),
            speech_acceleration: server_settings_speech_acceleration_default(),
            speech_model_unload_timeout: server_settings_speech_model_unload_timeout_default(),
            speech_project_custom_words: server_settings_speech_project_custom_words_default(),
            speech_post_processing_model_selection:
                server_settings_speech_post_processing_model_selection_default(),
            project_agent_browser_access_overrides:
                server_settings_project_agent_browser_access_overrides_default(),
            default_auto_pull: server_settings_default_auto_pull_default(),
            default_project_scripts: server_settings_default_project_scripts_default(),
            project_script_overrides: server_settings_project_script_overrides_default(),
            project_auto_pull_overrides: server_settings_project_auto_pull_overrides_default(),
            default_model_selection: server_settings_default_model_selection_default(),
            fallback_model_selection: server_settings_fallback_model_selection_default(),
            default_runtime_mode: server_settings_default_runtime_mode_default(),
            project_settings_overrides: server_settings_project_settings_overrides_default(),
            project_settings_folded: server_settings_project_settings_folded_default(),
            enable_agent_device_access: server_settings_enable_agent_device_access_default(),
            enable_device_support: server_settings_enable_device_support_default(),
            device_onboarding_completed: server_settings_device_onboarding_completed_default(),
            device_hosts: server_settings_device_hosts_default(),
            eas_device_hosts: server_settings_eas_device_hosts_default(),
            simlock_device_hosts: server_settings_simlock_device_hosts_default(),
            simlock_device_hosts_enabled: server_settings_simlock_device_hosts_enabled_default(),
            sidebar_auto_settle_after_days: server_settings_sidebar_auto_settle_after_days_default(
            ),
            compact_idle_threads: server_settings_compact_idle_threads_default(),
            idle_compaction: None,
            snooze_limited_threads: server_settings_snooze_limited_threads_default(),
            auto_resume_limited_threads: server_settings_auto_resume_limited_threads_default(),
            snooze_presets: server_settings_snooze_presets_default(),
            hidden_thread_tags: server_settings_hidden_thread_tags_default(),
            hide_tagged_threads: server_settings_hide_tagged_threads_default(),
            guest_sharing_enabled: server_settings_guest_sharing_enabled_default(),
            linked_environments_enabled: server_settings_linked_environments_enabled_default(),
            prompt_suggestions_enabled: server_settings_prompt_suggestions_enabled_default(),
            sidebar_auto_settle_keep_pull_requests:
                server_settings_sidebar_auto_settle_keep_pull_requests_default(),
            sidebar_auto_settle_on_merge: server_settings_sidebar_auto_settle_on_merge_default(),
            background_activity: server_settings_background_activity_default(),
            automatic_git_fetch_interval: server_settings_automatic_git_fetch_interval_default(),
            provider_health_refresh_interval:
                server_settings_provider_health_refresh_interval_default(),
            background_activity_profile: server_settings_background_activity_profile_default(),
            default_theme: server_settings_default_theme_default(),
            default_theme_set_at: server_settings_default_theme_set_at_default(),
            environment_icon: server_settings_environment_icon_default(),
            default_thread_env_mode: None,
            new_worktrees_start_from_origin:
                server_settings_new_worktrees_start_from_origin_default(),
            worktree_submodules: server_settings_worktree_submodules_default(),
            add_project_base_directory: server_settings_add_project_base_directory_default(),
            ghq_clone_layout: server_settings_ghq_clone_layout_default(),
            text_generation_model_selection:
                server_settings_text_generation_model_selection_default(),
            thread_title_instructions: server_settings_thread_title_instructions_default(),
            branch_naming_mode: server_settings_branch_naming_mode_default(),
            branch_name_prefix: server_settings_branch_name_prefix_default(),
            branch_name_instructions: server_settings_branch_name_instructions_default(),
            remove_agent_credits_on_merge: server_settings_remove_agent_credits_on_merge_default(),
            source_control_writing_style: server_settings_source_control_writing_style_default(),
            source_control_writer_model_selection:
                server_settings_source_control_writer_model_selection_default(),
            review_guides_enabled: server_settings_review_guides_enabled_default(),
            review_guide_model_selection: server_settings_review_guide_model_selection_default(),
            review_guide_instructions: server_settings_review_guide_instructions_default(),
            pull_request_merge_method: server_settings_pull_request_merge_method_default(),
            provider_instances: server_settings_provider_instances_default(),
            observability: server_settings_observability_default(),
            source_control_hosts: server_settings_source_control_hosts_default(),
            offered_source_control_providers:
                server_settings_offered_source_control_providers_default(),
            usage_limit_sources: server_settings_usage_limit_sources_default(),
            text_generators: server_settings_text_generators_default(),
            secret_providers: server_settings_secret_providers_default(),
            usage_price_overrides: server_settings_usage_price_overrides_default(),
            usage_model_aliases: server_settings_usage_model_aliases_default(),
            usage_model_filter: None,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerSettingsErrorTag {
    #[serde(rename = "ServerSettingsError")]
    ServerSettingsError,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerSettingsOperation {
    #[serde(rename = "normalize")]
    Normalize,
    #[serde(rename = "check-exists")]
    CheckExists,
    #[serde(rename = "create-provider-instance")]
    CreateProviderInstance,
    #[serde(rename = "read-file")]
    ReadFile,
    #[serde(rename = "read-provider-history")]
    ReadProviderHistory,
    #[serde(rename = "read-project-settings")]
    ReadProjectSettings,
    #[serde(rename = "read-secret")]
    ReadSecret,
    #[serde(rename = "remove-secret")]
    RemoveSecret,
    #[serde(rename = "remove-stale-secret")]
    RemoveStaleSecret,
    #[serde(rename = "write-secret")]
    WriteSecret,
    #[serde(rename = "write-secret-provider")]
    WriteSecretProvider,
    #[serde(rename = "write-file")]
    WriteFile,
    #[serde(rename = "prepare-directory")]
    PrepareDirectory,
    #[serde(rename = "write-managed-setting")]
    WriteManagedSetting,
    #[serde(rename = "use-disallowed-provider")]
    UseDisallowedProvider,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettingsError {
    #[serde(rename = "_tag")]
    pub _tag: ServerSettingsErrorTag,
    #[serde()]
    pub settings_path: String,
    #[serde()]
    pub operation: ServerSettingsOperation,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub environment_variable: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub managed_setting: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub managed_by: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_label: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettingsPatchSetModelFavoritesValueValue {
    #[serde()]
    pub provider: crate::provider_instance::ProviderInstanceId,
    #[serde()]
    pub model: crate::base_schemas::TrimmedNonEmptyString,
    #[serde()]
    pub favorite: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettingsPatchSetModelsHiddenValueValue {
    #[serde()]
    pub provider: crate::provider_instance::ProviderInstanceId,
    #[serde()]
    pub model: crate::base_schemas::TrimmedNonEmptyString,
    #[serde()]
    pub hidden: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettingsPatchSetProviderModelOrderValue {
    #[serde()]
    pub provider: crate::provider_instance::ProviderInstanceId,
    #[serde()]
    pub model_order: Vec<crate::base_schemas::TrimmedString>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettingsPatchMigrateModelPreferencesValue {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub favorites: Option<ModelFavorites>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub provider_model_preferences: Option<ProviderModelPreferences>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerSettingsPatchWorktreeCleanupValueValueMember1Mode {
    #[serde(rename = "off")]
    Off,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettingsPatchWorktreeCleanupValueValueMember1 {
    #[serde()]
    pub mode: ServerSettingsPatchWorktreeCleanupValueValueMember1Mode,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerSettingsPatchWorktreeCleanupValueValueMember2Mode {
    #[serde(rename = "custom")]
    Custom,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettingsPatchWorktreeCleanupValueValueMember2Rules {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub worktree_keep_when: Option<WorktreeKeepWhen>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub worktree_after_days: Option<StorageRetentionDays>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub worktree_settled_after_days: Option<SettledWorktreeRetentionDays>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub worktree_on_merge: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub worktree_on_delete: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub worktree_unchanged: Option<bool>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettingsPatchWorktreeCleanupValueValueMember2 {
    #[serde()]
    pub mode: ServerSettingsPatchWorktreeCleanupValueValueMember2Mode,
    #[serde()]
    pub rules: ServerSettingsPatchWorktreeCleanupValueValueMember2Rules,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerSettingsPatchWorktreeCleanupValueValue {
    Off1(Box<ServerSettingsPatchWorktreeCleanupValueValueMember1>),
    Custom2(Box<ServerSettingsPatchWorktreeCleanupValueValueMember2>),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettingsPatchStorageCleanupValue {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub worktree_keep_when: Option<WorktreeKeepWhen>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub worktree_after_days: Option<StorageRetentionDays>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub worktree_settled_after_days: Option<SettledWorktreeRetentionDays>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub worktree_on_merge: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub worktree_on_delete: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub worktree_unchanged: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub browser_artifacts_after_days: Option<StorageRetentionDays>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub logs_after_days: Option<StorageRetentionDays>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettingsPatchCloudSpeechCredentialsValue {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub openai: Option<crate::settings_secret_value::SettingsSecretValue>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub xai: Option<crate::settings_secret_value::SettingsSecretValue>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub openrouter: Option<crate::settings_secret_value::SettingsSecretValue>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettingsPatchProjectSettingsOverridesValueValueValue {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub worktree_cleanup: Option<WorktreeCleanup>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub default_model_selection: Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub default_runtime_mode: Option<crate::provider_policy::RuntimeMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub default_thread_env_mode: Option<crate::environment::ThreadEnvMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub new_worktrees_start_from_origin: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::lenient"
    )]
    pub worktree_submodules: Option<crate::environment::WorktreeSubmodules>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub default_auto_pull: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub default_project_scripts: Option<Vec<crate::project::ProjectScript>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub enable_agent_browser_access: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub enable_agent_device_access: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub text_generation_model_selection: Option<crate::model_selection::ModelSelection>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub thread_title_instructions: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub source_control_writer_model_selection:
        Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub review_guide_model_selection: Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub review_guide_instructions: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub source_control_writing_style: Option<SourceControlWritingStyleSettings>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub remove_agent_credits_on_merge: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub branch_naming_mode: Option<BranchNamingMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub branch_name_prefix: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub branch_name_instructions: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub pull_request_merge_method: Option<Option<crate::pull_request::PullRequestMergeMethod>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub sidebar_auto_settle_on_merge: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub sidebar_auto_settle_after_days: Option<Option<SidebarAutoSettleAfterDays>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub sidebar_auto_settle_keep_pull_requests: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub continue_threads_after_server_update: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub response_streaming_mode: Option<ResponseStreamingMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub speech_project_custom_words: Option<crate::speech::SpeechCustomWords>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub provider_instances: Option<
        std::collections::BTreeMap<
            crate::provider_instance::ProviderInstanceId,
            ProjectProviderInstanceOverride,
        >,
    >,
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

pub type ServerSettingsPatchBackgroundActivityValueSchemaVersionValue = WireLiteral<1>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettingsPatchBackgroundActivityValue {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub schema_version: Option<ServerSettingsPatchBackgroundActivityValueSchemaVersionValue>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub profile: Option<BackgroundActivityProfileSelection>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub base_profile: Option<BackgroundActivityProfile>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub overrides: Option<BackgroundActivityOverrides>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelectionPatch {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub instance_id: Option<crate::provider_instance::ProviderInstanceId>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub model: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub options: Option<crate::model::ProviderOptionSelections>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettingsPatchSourceControlWritingStyleValue {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub mode: Option<SourceControlWritingStyleMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub custom_instructions: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub follow_change_request_templates: Option<bool>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettingsPatchObservabilityValue {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub otlp_traces_url: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub otlp_metrics_url: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub otlp_logs_url: Option<crate::base_schemas::TrimmedString>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretProviderConfigWrite {
    #[serde()]
    pub kind: SecretProviderKind,
    #[serde()]
    pub label: SecretProviderLabel,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub config: Option<serde_json::Value>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettingsPatch {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub set_model_favorites: Option<Vec<ServerSettingsPatchSetModelFavoritesValueValue>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub set_models_hidden: Option<Vec<ServerSettingsPatchSetModelsHiddenValueValue>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub set_provider_model_order: Option<ServerSettingsPatchSetProviderModelOrderValue>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub favorites: Option<ModelFavorites>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub provider_model_preferences: Option<ProviderModelPreferences>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub migrate_model_preferences: Option<ServerSettingsPatchMigrateModelPreferencesValue>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub worktree_cleanup: Option<Option<ServerSettingsPatchWorktreeCleanupValueValue>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub storage_cleanup: Option<ServerSettingsPatchStorageCleanupValue>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub worktrees_directory: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub fast_worktree_checkout: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub response_streaming_mode: Option<ResponseStreamingMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub enable_provider_update_checks: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub continue_threads_after_server_update: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub keep_awake_while_agents_work: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub enable_agent_browser_access: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub cloud_speech_credentials: Option<ServerSettingsPatchCloudSpeechCredentialsValue>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub speech_model_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "patch_speech_acceleration"
    )]
    pub speech_acceleration: Option<crate::speech::SpeechAcceleration>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub speech_model_unload_timeout: Option<crate::speech::SpeechModelUnloadTimeout>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub speech_project_custom_words: Option<crate::speech::SpeechCustomWords>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub speech_post_processing_model_selection:
        Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub project_agent_browser_access_overrides:
        Option<std::collections::BTreeMap<crate::base_schemas::ProjectId, Option<bool>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub default_auto_pull: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub default_project_scripts: Option<Vec<crate::project::ProjectScript>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub project_script_overrides: Option<
        std::collections::BTreeMap<
            crate::base_schemas::ProjectId,
            Option<Vec<crate::project::ProjectScript>>,
        >,
    >,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub project_auto_pull_overrides:
        Option<std::collections::BTreeMap<crate::base_schemas::ProjectId, Option<bool>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub default_model_selection: Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub default_runtime_mode: Option<crate::provider_policy::RuntimeMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub fallback_model_selection: Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub project_settings_overrides: Option<
        std::collections::BTreeMap<
            crate::base_schemas::ProjectId,
            Option<ServerSettingsPatchProjectSettingsOverridesValueValueValue>,
        >,
    >,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub enable_agent_device_access: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub enable_device_support: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub device_onboarding_completed: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub device_hosts: Option<crate::device::SshDeviceHostConfigs>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub eas_device_hosts: Option<crate::device::EasDeviceHostConfigs>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub simlock_device_hosts: Option<crate::device::SimlockDeviceHostConfigs>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub simlock_device_hosts_enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub sidebar_auto_settle_after_days: Option<Option<SidebarAutoSettleAfterDays>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub compact_idle_threads: Option<LegacyCompactIdleThreads>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub idle_compaction: Option<IdleCompactionMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub sidebar_auto_settle_keep_pull_requests: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub sidebar_auto_settle_on_merge: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub auto_resume_limited_threads: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub snooze_limited_threads: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub snooze_presets: Option<SnoozePresetSettings>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub hidden_thread_tags: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub hide_tagged_threads: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub guest_sharing_enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub linked_environments_enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub prompt_suggestions_enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub background_activity: Option<ServerSettingsPatchBackgroundActivityValue>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub automatic_git_fetch_interval: Option<WireNumber>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub provider_health_refresh_interval: Option<WireNumber>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub background_activity_profile: Option<BackgroundActivityProfile>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub environment_icon: Option<Option<crate::environment::EnvironmentMachineKind>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub default_thread_env_mode: Option<Option<crate::environment::ThreadEnvMode>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub new_worktrees_start_from_origin: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub worktree_submodules: Option<Option<crate::environment::WorktreeSubmodules>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub add_project_base_directory: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub ghq_clone_layout: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub text_generation_model_selection: Option<ModelSelectionPatch>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub thread_title_instructions: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub branch_naming_mode: Option<BranchNamingMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub branch_name_prefix: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub branch_name_instructions: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub remove_agent_credits_on_merge: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub source_control_writing_style: Option<ServerSettingsPatchSourceControlWritingStyleValue>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub source_control_writer_model_selection:
        Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub review_guides_enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub review_guide_model_selection: Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub review_guide_instructions: Option<crate::base_schemas::TrimmedString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub pull_request_merge_method: Option<Option<crate::pull_request::PullRequestMergeMethod>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub observability: Option<ServerSettingsPatchObservabilityValue>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub source_control_hosts: Option<std::collections::BTreeMap<String, SourceControlHostSettings>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub offered_source_control_providers: Option<std::collections::BTreeMap<String, bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub provider_instances: Option<
        std::collections::BTreeMap<
            crate::provider_instance::ProviderInstanceId,
            crate::provider_instance::ProviderInstanceConfig,
        >,
    >,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub usage_limit_sources: Option<
        std::collections::BTreeMap<
            crate::usage_limit_source_id::UsageLimitSourceId,
            Option<UsageLimitSourceConfig>,
        >,
    >,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub text_generators: Option<
        std::collections::BTreeMap<
            crate::provider_instance::ProviderInstanceId,
            Option<TextGeneratorConfig>,
        >,
    >,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub secret_providers: Option<
        std::collections::BTreeMap<
            crate::secret_reference::SecretProviderId,
            Option<SecretProviderConfigWrite>,
        >,
    >,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub usage_price_overrides: Option<
        std::collections::BTreeMap<
            crate::base_schemas::TrimmedNonEmptyString,
            Option<UsageModelPriceOverride>,
        >,
    >,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub usage_model_aliases: Option<
        std::collections::BTreeMap<
            crate::base_schemas::TrimmedNonEmptyString,
            Option<crate::base_schemas::TrimmedNonEmptyString>,
        >,
    >,
    #[serde(
        rename = "requireConnectDeviceApproval",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub require_connect_device_approval: Option<bool>,
    #[serde(
        rename = "defaultTheme",
        default,
        skip_serializing_if = "Option::is_none",
        with = "patch_theme_string"
    )]
    pub default_theme: Option<DefaultThemePreference>,
    #[serde(
        rename = "defaultThemeSetAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "patch_theme_string"
    )]
    pub default_theme_set_at: Option<String>,
    #[serde(
        rename = "defaultAppearance",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub default_appearance: Option<DefaultAppearancePreference>,
    #[serde(
        rename = "defaultAppearanceSetAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "patch_theme_string"
    )]
    pub default_appearance_set_at: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StorageCleanupReportTrigger {
    #[serde(rename = "automatic")]
    Automatic,
    #[serde(rename = "manual")]
    Manual,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StorageCleanupReportEntryKind {
    #[serde(rename = "worktree")]
    Worktree,
    #[serde(rename = "browser-artifacts")]
    BrowserArtifacts,
    #[serde(rename = "logs")]
    Logs,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StorageCleanupReportEntryOutcome {
    #[serde(rename = "removed")]
    Removed,
    #[serde(rename = "kept")]
    Kept,
    #[serde(rename = "failed")]
    Failed,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCleanupReportEntry {
    #[serde()]
    pub kind: StorageCleanupReportEntryKind,
    #[serde()]
    pub outcome: StorageCleanupReportEntryOutcome,
    #[serde()]
    pub reason: String,
    #[serde(with = "crate::wire::nullable")]
    pub path: Option<String>,
    #[serde(
        default = "storage_cleanup_report_entry_project_path_default",
        with = "crate::wire::nullable"
    )]
    pub project_path: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub thread_id: Option<crate::base_schemas::ThreadId>,
    #[serde(with = "crate::wire::nullable")]
    pub thread_title: Option<String>,
    #[serde(
        default = "storage_cleanup_report_entry_bytes_default",
        with = "crate::wire::nullable"
    )]
    pub bytes: Option<WireNumber>,
    #[serde(
        default = "storage_cleanup_report_entry_files_default",
        with = "crate::wire::nullable"
    )]
    pub files: Option<WireNumber>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCleanupReportCounts {
    #[serde()]
    pub removed: WireNumber,
    #[serde()]
    pub kept: WireNumber,
    #[serde()]
    pub failed: WireNumber,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCleanupReport {
    #[serde()]
    pub trigger: StorageCleanupReportTrigger,
    #[serde()]
    pub started_at: String,
    #[serde()]
    pub finished_at: String,
    #[serde()]
    pub entries: Vec<StorageCleanupReportEntry>,
    #[serde()]
    pub counts: StorageCleanupReportCounts,
    #[serde()]
    pub omitted_count: WireNumber,
    #[serde(default = "storage_cleanup_report_bytes_freed_default")]
    pub bytes_freed: WireNumber,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TextGeneratorErrorTag {
    #[serde(rename = "TextGeneratorError")]
    TextGeneratorError,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextGeneratorError {
    #[serde(rename = "_tag")]
    pub _tag: TextGeneratorErrorTag,
    #[serde()]
    pub detail: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VoiceTranscriptionDestinationMember1Kind {
    #[serde(rename = "environment")]
    Environment,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTranscriptionDestinationMember1 {
    #[serde()]
    pub kind: VoiceTranscriptionDestinationMember1Kind,
    #[serde()]
    pub environment_id: crate::base_schemas::EnvironmentId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VoiceTranscriptionDestinationMember2Kind {
    #[serde(rename = "cloud")]
    Cloud,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VoiceTranscriptionDestinationMember2Provider {
    #[serde(rename = "openai")]
    Openai,
    #[serde(rename = "xai")]
    Xai,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTranscriptionDestinationMember2 {
    #[serde()]
    pub kind: VoiceTranscriptionDestinationMember2Kind,
    #[serde()]
    pub environment_id: crate::base_schemas::EnvironmentId,
    #[serde()]
    pub provider: VoiceTranscriptionDestinationMember2Provider,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VoiceTranscriptionDestinationMember3Kind {
    #[serde(rename = "cloud")]
    Cloud,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VoiceTranscriptionDestinationMember3Provider {
    #[serde(rename = "openrouter")]
    Openrouter,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTranscriptionDestinationMember3 {
    #[serde()]
    pub kind: VoiceTranscriptionDestinationMember3Kind,
    #[serde()]
    pub environment_id: crate::base_schemas::EnvironmentId,
    #[serde()]
    pub provider: VoiceTranscriptionDestinationMember3Provider,
    #[serde()]
    pub model: crate::speech::CloudSpeechModelId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VoiceTranscriptionDestination {
    Environment1(Box<VoiceTranscriptionDestinationMember1>),
    Cloud2(Box<VoiceTranscriptionDestinationMember2>),
    Cloud3(Box<VoiceTranscriptionDestinationMember3>),
}

fn antigravity_settings_enabled_default() -> bool {
    false
}
fn antigravity_settings_auth_method_default() -> AntigravityAuthMethod {
    AntigravityAuthMethod::OauthPersonal
}
fn antigravity_settings_api_key_default() -> crate::settings_secret_value::SettingsSecretValue {
    crate::settings_secret_value::SettingsSecretValue::Literal("".to_owned())
}
fn antigravity_settings_gcp_project_default() -> crate::base_schemas::TrimmedString {
    crate::base_schemas::TrimmedString("".to_owned())
}
fn antigravity_settings_gcp_location_default() -> crate::base_schemas::TrimmedString {
    crate::base_schemas::TrimmedString("".to_owned())
}
fn antigravity_settings_binary_path_default() -> crate::base_schemas::TrimmedString {
    crate::base_schemas::TrimmedString("".to_owned())
}
fn antigravity_settings_custom_models_default() -> Vec<crate::model::CustomModelSetting> {
    Vec::new()
}
fn background_activity_settings_schema_version_default() -> BackgroundActivitySettingsSchemaVersion
{
    WireLiteral
}
fn background_activity_settings_profile_default() -> BackgroundActivityProfileSelection {
    BackgroundActivityProfileSelection::Balanced
}
fn background_activity_settings_overrides_default() -> BackgroundActivityOverrides {
    BackgroundActivityOverrides {
        automatic_git_fetch_interval: None,
        provider_health_refresh_interval: None,
        host_power_monitor_active_interval: None,
        host_power_monitor_idle_interval: None,
        pause_when_host_locked: None,
        pause_when_host_low_power: None,
        pause_when_client_low_power: None,
        pause_when_on_battery: None,
    }
}
fn claude_settings_enabled_default() -> bool {
    true
}
fn claude_settings_binary_path_default() -> String {
    "claude".to_owned()
}
fn claude_settings_home_path_default() -> crate::base_schemas::TrimmedString {
    crate::base_schemas::TrimmedString("".to_owned())
}
fn claude_settings_custom_models_default() -> Vec<crate::model::CustomModelSetting> {
    Vec::new()
}
fn claude_settings_launch_args_default() -> String {
    "".to_owned()
}
fn claude_settings_auto_compact_window_default() -> crate::base_schemas::TrimmedString {
    crate::base_schemas::TrimmedString("".to_owned())
}
fn claude_settings_idle_compaction_default() -> InstanceIdleCompaction {
    InstanceIdleCompaction::Default
}
fn provider_model_preferences_value_value_hidden_models_default() -> Vec<String> {
    Vec::new()
}
fn provider_model_preferences_value_value_model_order_default() -> Vec<String> {
    Vec::new()
}
fn client_settings_schema_notification_mode_default() -> NotificationMode {
    NotificationMode::Off
}
fn client_settings_schema_in_app_notifications_enabled_default() -> bool {
    false
}
fn client_settings_schema_diff_color_scheme_default() -> DiffColorScheme {
    DiffColorScheme::RedGreen
}
fn client_settings_schema_chat_width_default() -> ChatWidth {
    ChatWidth::Comfortable
}
fn client_settings_schema_effort_display_default() -> EffortDisplay {
    EffortDisplay::Label
}
fn client_settings_schema_load_balancing_enabled_default() -> bool {
    false
}
fn client_settings_schema_load_balancing_weights_default() -> LoadBalancingWeights {
    std::collections::BTreeMap::new()
}
fn client_settings_schema_appearance_contrast_default() -> AppearanceContrast {
    100
}
fn client_settings_schema_panel_animation_duration_ms_default() -> PanelAnimationDurationMs {
    0
}
fn client_settings_schema_browser_default_viewport_default()
-> crate::preview::PreviewViewportSetting {
    crate::preview::PreviewViewportSetting::Fill {}
}
fn client_settings_schema_browser_default_zoom_factor_default() -> crate::preview::PreviewZoomFactor
{
    1.into()
}
fn client_settings_schema_browser_default_appearance_default()
-> crate::preview::PreviewAppearancePreference {
    crate::preview::PreviewAppearancePreference::System
}
fn client_settings_schema_browser_recording_frame_rate_default()
-> crate::preview::BrowserRecordingFrameRate {
    30
}
fn client_settings_schema_browser_recording_show_key_presses_default() -> bool {
    false
}
fn client_settings_schema_browser_recording_show_mouse_presses_default() -> bool {
    false
}
fn client_settings_schema_browser_link_target_default() -> BrowserLinkTarget {
    BrowserLinkTarget::System
}
fn client_settings_schema_browser_search_engine_default() -> BrowserSearchEngine {
    BrowserSearchEngine::Duckduckgo
}
fn client_settings_schema_browser_custom_search_url_default() -> BrowserCustomSearchUrl {
    "".to_owned()
}
fn client_settings_schema_browser_auto_show_floating_preview_default() -> bool {
    true
}
fn client_settings_schema_browser_profiles_default() -> Vec<crate::browser_profile::BrowserProfile>
{
    Vec::new()
}
fn client_settings_schema_browser_default_profile_id_default()
-> crate::browser_profile::BrowserProfileId {
    "default".to_owned()
}
fn client_settings_schema_confirm_quit_default() -> QuitConfirmationModeSetting {
    QuitConfirmationModeSetting::Hold
}
fn client_settings_schema_confirm_thread_archive_default() -> bool {
    false
}
fn client_settings_schema_confirm_thread_delete_default() -> bool {
    true
}
fn client_settings_schema_confirm_thread_unpin_default() -> bool {
    false
}
fn client_settings_schema_diff_files_collapsed_default() -> bool {
    true
}
fn client_settings_schema_diff_ignore_whitespace_default() -> bool {
    true
}
fn client_settings_schema_diff_layout_default() -> DiffLayout {
    DiffLayout::Stacked
}
fn client_settings_schema_environment_identification_mode_default() -> EnvironmentIdentificationMode
{
    EnvironmentIdentificationMode::Artwork
}
fn client_settings_schema_glass_opacity_default() -> GlassOpacity {
    80
}
fn client_settings_schema_font_size_interface_default() -> InterfaceFontSize {
    16
}
fn client_settings_schema_font_size_prompt_default() -> PromptFontSize {
    14
}
fn client_settings_schema_font_size_code_default() -> CodeFontSize {
    13
}
fn client_settings_schema_font_size_terminal_default() -> TerminalFontSize {
    12
}
fn client_settings_schema_font_family_code_default() -> FontFamilyPreference {
    "".to_owned()
}
fn client_settings_schema_font_family_composer_default() -> FontFamilyPreference {
    "".to_owned()
}
fn client_settings_schema_font_family_sans_default() -> FontFamilyPreference {
    "".to_owned()
}
fn client_settings_schema_font_family_terminal_default() -> FontFamilyPreference {
    "".to_owned()
}
fn client_settings_schema_font_smoothing_default() -> bool {
    true
}
fn client_settings_schema_onboarding_completed_at_default() -> Option<String> {
    None
}
fn client_settings_schema_persist_composer_context_strip_default() -> bool {
    false
}
fn client_settings_schema_favorites_default() -> ModelFavorites {
    Vec::new()
}
fn client_settings_schema_provider_model_preferences_default() -> ProviderModelPreferences {
    std::collections::BTreeMap::new()
}
fn client_settings_schema_pull_request_merge_method_overrides_default()
-> std::collections::BTreeMap<
    crate::base_schemas::TrimmedNonEmptyString,
    crate::pull_request::PullRequestMergeMethod,
> {
    std::collections::BTreeMap::new()
}
fn client_settings_schema_pull_request_links_open_in_app_default() -> bool {
    true
}
fn client_settings_schema_composer_rich_text_enabled_default() -> bool {
    true
}
fn client_settings_schema_send_shortcut_default() -> ClientSettingsSchemaSendShortcut {
    ClientSettingsSchemaSendShortcut::Enter
}
fn client_settings_schema_follow_up_behavior_default() -> ClientSettingsSchemaFollowUpBehavior {
    ClientSettingsSchemaFollowUpBehavior::Steer
}
fn client_settings_schema_proactive_panels_enabled_default() -> bool {
    false
}
fn client_settings_schema_show_skills_in_slash_menu_default() -> bool {
    true
}
fn client_settings_schema_boring_sidebar_enabled_default() -> bool {
    false
}
fn client_settings_schema_boring_sidebar_multiline_rows_default() -> bool {
    false
}
fn client_settings_schema_tool_call_visibility_default() -> ToolCallVisibility {
    ToolCallVisibility::ShowAll
}
fn client_settings_schema_screen_reader_announcements_default() -> ScreenReaderAnnouncements {
    ScreenReaderAnnouncements::Status
}
fn client_settings_schema_screen_reader_window_title_default() -> bool {
    false
}
fn client_settings_schema_thread_details_usage_enabled_default() -> bool {
    false
}
fn client_settings_schema_thread_details_items_default() -> std::collections::BTreeMap<String, bool>
{
    std::collections::BTreeMap::new()
}
fn client_settings_schema_sidebar_working_shelf_enabled_default() -> bool {
    true
}
fn client_settings_schema_sidebar_project_grouping_mode_default() -> SidebarProjectGroupingMode {
    SidebarProjectGroupingMode::Repository
}
fn client_settings_schema_sidebar_project_grouping_overrides_default() -> std::collections::BTreeMap<
    crate::base_schemas::TrimmedNonEmptyString,
    SidebarProjectGroupingMode,
> {
    std::collections::BTreeMap::new()
}
fn client_settings_schema_sidebar_project_sort_order_default() -> SidebarProjectSortOrder {
    SidebarProjectSortOrder::UpdatedAt
}
fn client_settings_schema_sidebar_thread_sort_order_default() -> SidebarThreadSortOrder {
    SidebarThreadSortOrder::UpdatedAt
}
fn client_settings_schema_timestamp_format_default() -> TimestampFormat {
    TimestampFormat::Locale
}
fn client_settings_schema_voice_microphone_default() -> String {
    "".to_owned()
}
fn client_settings_schema_voice_microphones_default() -> Vec<VoiceMicrophoneEntry> {
    Vec::new()
}
fn client_settings_schema_voice_shortcut_mode_default() -> VoiceShortcutMode {
    VoiceShortcutMode::Auto
}
fn client_settings_schema_speech_language_default() -> crate::speech::SpeechLanguage {
    "auto".to_owned()
}
fn client_settings_schema_speech_translate_to_english_default() -> bool {
    false
}
fn client_settings_schema_speech_custom_words_default() -> crate::speech::SpeechCustomWords {
    Vec::new()
}
fn client_settings_schema_speech_silence_filtering_default() -> bool {
    true
}
fn client_settings_schema_speech_remove_filler_words_default() -> bool {
    true
}
fn client_settings_schema_speech_post_processing_prompt_default()
-> crate::speech::SpeechPostProcessingPromptSettings {
    crate::speech::SpeechPostProcessingPromptSettings {
        mode: crate::speech::SpeechPostProcessingPromptMode::Default,
        custom_instructions: String::new(),
    }
}
fn client_settings_schema_speech_correction_word_default() -> crate::speech::SpeechCorrectionCue {
    "".to_owned()
}
fn client_settings_schema_language_default() -> String {
    "en".to_owned()
}
fn client_settings_schema_snap_shot_enabled_default() -> bool {
    false
}
fn client_settings_schema_snap_shot_include_accessibility_default() -> bool {
    true
}
fn client_settings_schema_snap_shot_shortcut_default() -> SnapShotShortcut {
    SnapShotShortcut::BothShiftKeys1(Box::new(SnapShotShortcutMember1 {
        kind: SnapShotShortcutMember1Kind::BothShiftKeys,
    }))
}
fn client_settings_schema_snap_shot_play_sound_default() -> bool {
    true
}
fn client_settings_schema_snap_shot_sound_default() -> SnapShotSound {
    SnapShotSound::SoftPop
}
fn client_settings_schema_snap_shot_flash_default() -> bool {
    true
}
fn client_settings_schema_snap_shot_animations_default() -> bool {
    true
}
fn client_settings_schema_word_wrap_default() -> bool {
    true
}
fn client_settings_schema_math_rendering_enabled_default() -> bool {
    false
}
fn cloud_speech_credentials_openai_default() -> CloudSpeechCredential {
    crate::settings_secret_value::SettingsSecretValue::Literal("".to_owned())
}
fn cloud_speech_credentials_xai_default() -> CloudSpeechCredential {
    crate::settings_secret_value::SettingsSecretValue::Literal("".to_owned())
}
fn cloud_speech_credentials_openrouter_default() -> CloudSpeechCredential {
    crate::settings_secret_value::SettingsSecretValue::Literal("".to_owned())
}
fn codex_settings_enabled_default() -> bool {
    true
}
fn codex_settings_binary_path_default() -> String {
    "codex".to_owned()
}
fn codex_settings_home_path_default() -> crate::base_schemas::TrimmedString {
    crate::base_schemas::TrimmedString("".to_owned())
}
fn codex_settings_shadow_home_path_default() -> crate::base_schemas::TrimmedString {
    crate::base_schemas::TrimmedString("".to_owned())
}
fn codex_settings_launch_args_default() -> crate::base_schemas::TrimmedString {
    crate::base_schemas::TrimmedString("".to_owned())
}
fn codex_settings_custom_models_default() -> Vec<crate::model::CustomModelSetting> {
    Vec::new()
}
fn text_generator_config_api_key_default() -> crate::settings_secret_value::SettingsSecretValue {
    crate::settings_secret_value::SettingsSecretValue::Literal("".to_owned())
}
fn text_generator_config_enabled_default() -> bool {
    true
}
fn observability_settings_otlp_traces_url_default() -> crate::base_schemas::TrimmedString {
    crate::base_schemas::TrimmedString("".to_owned())
}
fn observability_settings_otlp_metrics_url_default() -> crate::base_schemas::TrimmedString {
    crate::base_schemas::TrimmedString("".to_owned())
}
fn observability_settings_otlp_logs_url_default() -> crate::base_schemas::TrimmedString {
    crate::base_schemas::TrimmedString("".to_owned())
}
fn worktree_cleanup_rules_worktree_keep_when_default() -> WorktreeKeepWhen {
    WorktreeKeepWhen::UncommittedChanges
}
fn worktree_cleanup_rules_worktree_settled_after_days_default() -> SettledWorktreeRetentionDays {
    None
}
fn source_control_writing_style_settings_mode_default() -> SourceControlWritingStyleMode {
    SourceControlWritingStyleMode::RepoConventions
}
fn source_control_writing_style_settings_custom_instructions_default()
-> crate::base_schemas::TrimmedString {
    crate::base_schemas::TrimmedString("".to_owned())
}
fn source_control_writing_style_settings_follow_change_request_templates_default() -> bool {
    true
}
fn server_settings_favorites_default() -> Option<ModelFavorites> {
    None
}
fn server_settings_provider_model_preferences_default() -> Option<ProviderModelPreferences> {
    None
}
fn server_settings_worktree_cleanup_default() -> WorktreeCleanup {
    None
}
fn storage_cleanup_settings_worktree_keep_when_default() -> WorktreeKeepWhen {
    WorktreeKeepWhen::UncommittedChanges
}
fn storage_cleanup_settings_worktree_after_days_default() -> StorageRetentionDays {
    None
}
fn storage_cleanup_settings_worktree_settled_after_days_default() -> SettledWorktreeRetentionDays {
    None
}
fn storage_cleanup_settings_worktree_on_merge_default() -> bool {
    false
}
fn storage_cleanup_settings_worktree_on_delete_default() -> bool {
    false
}
fn storage_cleanup_settings_worktree_unchanged_default() -> bool {
    false
}
fn storage_cleanup_settings_browser_artifacts_after_days_default() -> StorageRetentionDays {
    None
}
fn storage_cleanup_settings_logs_after_days_default() -> StorageRetentionDays {
    None
}
fn server_settings_storage_cleanup_default() -> StorageCleanupSettings {
    StorageCleanupSettings {
        worktree_keep_when: WorktreeKeepWhen::UncommittedChanges,
        worktree_after_days: None,
        worktree_settled_after_days: None,
        worktree_on_merge: false,
        worktree_on_delete: false,
        worktree_unchanged: false,
        browser_artifacts_after_days: None,
        logs_after_days: None,
    }
}
fn server_settings_worktrees_directory_default() -> crate::base_schemas::TrimmedString {
    crate::base_schemas::TrimmedString("".to_owned())
}
fn server_settings_fast_worktree_checkout_default() -> bool {
    false
}
fn server_settings_previous_worktrees_directories_default()
-> Vec<crate::base_schemas::TrimmedString> {
    Vec::new()
}
fn server_settings_response_streaming_mode_default() -> ResponseStreamingMode {
    ResponseStreamingMode::Paragraph
}
fn server_settings_enable_provider_update_checks_default() -> bool {
    true
}
fn server_settings_continue_threads_after_server_update_default() -> bool {
    true
}
fn server_settings_keep_awake_while_agents_work_default() -> bool {
    true
}
fn server_settings_enable_agent_browser_access_default() -> bool {
    true
}
fn server_settings_cloud_speech_credentials_default() -> CloudSpeechCredentials {
    CloudSpeechCredentials {
        openai: crate::settings_secret_value::SettingsSecretValue::Literal("".to_owned()),
        xai: crate::settings_secret_value::SettingsSecretValue::Literal("".to_owned()),
        openrouter: crate::settings_secret_value::SettingsSecretValue::Literal("".to_owned()),
    }
}
fn server_settings_speech_model_id_default() -> String {
    "handy-computer/parakeet-unified-en-0.6b-gguf".to_owned()
}
fn server_settings_speech_acceleration_default() -> crate::speech::SpeechAcceleration {
    "auto".to_owned()
}
fn server_settings_speech_model_unload_timeout_default() -> crate::speech::SpeechModelUnloadTimeout
{
    crate::speech::SpeechModelUnloadTimeout::Min15
}
fn server_settings_speech_project_custom_words_default() -> crate::speech::SpeechCustomWords {
    Vec::new()
}
fn server_settings_speech_post_processing_model_selection_default()
-> Option<crate::model_selection::ModelSelection> {
    None
}
fn server_settings_project_agent_browser_access_overrides_default()
-> std::collections::BTreeMap<crate::base_schemas::ProjectId, bool> {
    std::collections::BTreeMap::new()
}
fn server_settings_default_auto_pull_default() -> bool {
    false
}
fn server_settings_default_project_scripts_default() -> Vec<crate::project::ProjectScript> {
    Vec::new()
}
fn server_settings_project_script_overrides_default() -> std::collections::BTreeMap<
    crate::base_schemas::ProjectId,
    Option<Vec<crate::project::ProjectScript>>,
> {
    std::collections::BTreeMap::new()
}
fn server_settings_project_auto_pull_overrides_default()
-> std::collections::BTreeMap<crate::base_schemas::ProjectId, bool> {
    std::collections::BTreeMap::new()
}
fn server_settings_default_model_selection_default()
-> Option<crate::model_selection::ModelSelection> {
    None
}
fn server_settings_fallback_model_selection_default()
-> Option<crate::model_selection::ModelSelection> {
    None
}
fn server_settings_default_runtime_mode_default() -> crate::provider_policy::RuntimeMode {
    crate::provider_policy::RuntimeMode::FullAccess
}
fn server_settings_project_settings_overrides_default() -> std::collections::BTreeMap<
    crate::base_schemas::ProjectId,
    ServerSettingsProjectSettingsOverridesValue,
> {
    std::collections::BTreeMap::new()
}
fn server_settings_project_settings_folded_default() -> bool {
    false
}
fn server_settings_enable_agent_device_access_default() -> bool {
    false
}
fn server_settings_enable_device_support_default() -> bool {
    false
}
fn server_settings_device_onboarding_completed_default() -> bool {
    false
}
fn server_settings_device_hosts_default() -> crate::device::SshDeviceHostConfigs {
    Vec::new()
}
fn server_settings_eas_device_hosts_default() -> crate::device::EasDeviceHostConfigs {
    Vec::new()
}
fn server_settings_simlock_device_hosts_default() -> crate::device::SimlockDeviceHostConfigs {
    Vec::new()
}
fn server_settings_simlock_device_hosts_enabled_default() -> bool {
    false
}
fn server_settings_sidebar_auto_settle_after_days_default() -> Option<SidebarAutoSettleAfterDays> {
    Some(7.into())
}
fn server_settings_compact_idle_threads_default() -> LegacyCompactIdleThreads {
    LegacyCompactIdleThreads::Member1(Box::new(false))
}
fn server_settings_snooze_limited_threads_default() -> bool {
    false
}
fn server_settings_auto_resume_limited_threads_default() -> bool {
    false
}
fn server_settings_snooze_presets_default() -> SnoozePresetSettings {
    vec![
        SnoozePresetSetting::After1(Box::new(SnoozePresetSettingMember1 {
            kind: SnoozePresetSettingMember1Kind::After,
            minutes: 60,
        })),
        SnoozePresetSetting::After1(Box::new(SnoozePresetSettingMember1 {
            kind: SnoozePresetSettingMember1Kind::After,
            minutes: 180,
        })),
        SnoozePresetSetting::At2(Box::new(SnoozePresetSettingMember2 {
            kind: SnoozePresetSettingMember2Kind::At,
            day: SnoozePresetDay::Today,
            time: "18:00".to_owned(),
        })),
        SnoozePresetSetting::At2(Box::new(SnoozePresetSettingMember2 {
            kind: SnoozePresetSettingMember2Kind::At,
            day: SnoozePresetDay::Tomorrow,
            time: "09:00".to_owned(),
        })),
        SnoozePresetSetting::At2(Box::new(SnoozePresetSettingMember2 {
            kind: SnoozePresetSettingMember2Kind::At,
            day: SnoozePresetDay::Monday,
            time: "09:00".to_owned(),
        })),
    ]
}
fn server_settings_hidden_thread_tags_default() -> Vec<String> {
    Vec::new()
}
fn server_settings_hide_tagged_threads_default() -> bool {
    false
}
fn server_settings_guest_sharing_enabled_default() -> bool {
    false
}
fn server_settings_linked_environments_enabled_default() -> bool {
    false
}
fn server_settings_prompt_suggestions_enabled_default() -> bool {
    false
}
fn server_settings_sidebar_auto_settle_keep_pull_requests_default() -> bool {
    false
}
fn server_settings_sidebar_auto_settle_on_merge_default() -> bool {
    true
}
fn server_settings_background_activity_default() -> BackgroundActivitySettings {
    BackgroundActivitySettings {
        schema_version: WireLiteral,
        profile: BackgroundActivityProfileSelection::Balanced,
        base_profile: None,
        overrides: BackgroundActivityOverrides {
            automatic_git_fetch_interval: None,
            provider_health_refresh_interval: None,
            host_power_monitor_active_interval: None,
            host_power_monitor_idle_interval: None,
            pause_when_host_locked: None,
            pause_when_host_low_power: None,
            pause_when_client_low_power: None,
            pause_when_on_battery: None,
        },
    }
}
fn server_settings_automatic_git_fetch_interval_default() -> WireNumber {
    WireNumber::Finite(30000.into())
}
fn server_settings_provider_health_refresh_interval_default() -> WireNumber {
    WireNumber::Finite(300000.into())
}
fn server_settings_background_activity_profile_default() -> BackgroundActivityProfile {
    BackgroundActivityProfile::Balanced
}
fn server_settings_default_theme_default() -> DefaultThemePreference {
    "".to_owned()
}
fn server_settings_default_theme_set_at_default() -> String {
    "".to_owned()
}
fn server_settings_environment_icon_default() -> Option<crate::environment::EnvironmentMachineKind>
{
    None
}
fn server_settings_new_worktrees_start_from_origin_default() -> bool {
    true
}
fn server_settings_worktree_submodules_default() -> Option<crate::environment::WorktreeSubmodules> {
    None
}
fn server_settings_add_project_base_directory_default() -> crate::base_schemas::TrimmedString {
    crate::base_schemas::TrimmedString("".to_owned())
}
fn server_settings_ghq_clone_layout_default() -> bool {
    false
}
fn server_settings_text_generation_model_selection_default()
-> crate::model_selection::ModelSelection {
    crate::model_selection::ModelSelection {
        instance_id: crate::provider_instance::ProviderInstanceId("codex".to_owned()),
        model: "gpt-6-luna".to_owned(),
        options: Some(crate::model::ProviderOptionSelections(vec![
            crate::model::ProviderOptionSelection {
                id: "reasoningEffort".to_owned(),
                value: crate::model::ProviderOptionSelectionValue::String("low".to_owned()),
            },
        ])),
    }
}
fn server_settings_thread_title_instructions_default() -> crate::base_schemas::TrimmedString {
    crate::base_schemas::TrimmedString("".to_owned())
}
fn server_settings_branch_naming_mode_default() -> BranchNamingMode {
    BranchNamingMode::Static
}
fn server_settings_branch_name_prefix_default() -> crate::base_schemas::TrimmedString {
    crate::base_schemas::TrimmedString("t3".to_owned())
}
fn server_settings_branch_name_instructions_default() -> crate::base_schemas::TrimmedString {
    crate::base_schemas::TrimmedString("".to_owned())
}
fn server_settings_remove_agent_credits_on_merge_default() -> bool {
    false
}
fn server_settings_source_control_writing_style_default() -> SourceControlWritingStyleSetting {
    SourceControlWritingStyleSetting(SourceControlWritingStyleSettings::default())
}
fn server_settings_source_control_writer_model_selection_default()
-> Option<crate::model_selection::ModelSelection> {
    None
}
fn server_settings_review_guides_enabled_default() -> bool {
    false
}
fn server_settings_review_guide_model_selection_default()
-> Option<crate::model_selection::ModelSelection> {
    None
}
fn server_settings_review_guide_instructions_default() -> crate::base_schemas::TrimmedString {
    crate::base_schemas::TrimmedString("".to_owned())
}
fn server_settings_pull_request_merge_method_default()
-> Option<crate::pull_request::PullRequestMergeMethod> {
    None
}
fn server_settings_provider_instances_default() -> std::collections::BTreeMap<
    crate::provider_instance::ProviderInstanceId,
    crate::provider_instance::ProviderInstanceConfig,
> {
    std::collections::BTreeMap::new()
}
fn server_settings_observability_default() -> ObservabilitySettings {
    ObservabilitySettings {
        otlp_traces_url: crate::base_schemas::TrimmedString("".to_owned()),
        otlp_metrics_url: crate::base_schemas::TrimmedString("".to_owned()),
        otlp_logs_url: crate::base_schemas::TrimmedString("".to_owned()),
    }
}
fn server_settings_source_control_hosts_default()
-> std::collections::BTreeMap<String, SourceControlHostSettings> {
    std::collections::BTreeMap::new()
}
fn server_settings_offered_source_control_providers_default()
-> std::collections::BTreeMap<String, bool> {
    std::collections::BTreeMap::new()
}
fn usage_limit_source_config_management_key_default()
-> crate::settings_secret_value::SettingsSecretValue {
    crate::settings_secret_value::SettingsSecretValue::Literal("".to_owned())
}
fn usage_limit_source_config_enabled_default() -> bool {
    true
}
fn server_settings_usage_limit_sources_default() -> std::collections::BTreeMap<
    crate::usage_limit_source_id::UsageLimitSourceId,
    UsageLimitSourceConfig,
> {
    std::collections::BTreeMap::new()
}
fn server_settings_text_generators_default()
-> std::collections::BTreeMap<crate::provider_instance::ProviderInstanceId, TextGeneratorConfig> {
    std::collections::BTreeMap::new()
}
fn server_settings_secret_providers_default()
-> std::collections::BTreeMap<crate::secret_reference::SecretProviderId, SecretProviderConfig> {
    std::collections::BTreeMap::new()
}
fn server_settings_usage_price_overrides_default()
-> std::collections::BTreeMap<crate::base_schemas::TrimmedNonEmptyString, UsageModelPriceOverride> {
    std::collections::BTreeMap::new()
}
fn server_settings_usage_model_aliases_default() -> std::collections::BTreeMap<
    crate::base_schemas::TrimmedNonEmptyString,
    crate::base_schemas::TrimmedNonEmptyString,
> {
    std::collections::BTreeMap::new()
}
fn storage_cleanup_report_entry_project_path_default() -> Option<String> {
    None
}
fn storage_cleanup_report_entry_bytes_default() -> Option<WireNumber> {
    None
}
fn storage_cleanup_report_entry_files_default() -> Option<WireNumber> {
    None
}
fn storage_cleanup_report_bytes_freed_default() -> WireNumber {
    WireNumber::Finite(0.into())
}

pub type ClientSettings = ClientSettingsSchema;

pub type WireDurationMillis = WireNumber;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefaultAppearancePreference {
    #[default]
    #[serde(rename = "")]
    Unset,
    #[serde(rename = "light")]
    Light,
    #[serde(rename = "dark")]
    Dark,
    #[serde(rename = "system")]
    System,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClientServerSettingsPatch(pub ServerSettingsPatch);
impl From<ServerSettingsPatch> for ClientServerSettingsPatch {
    fn from(mut patch: ServerSettingsPatch) -> Self {
        patch.default_theme = None;
        patch.default_theme_set_at = None;
        patch.default_appearance = None;
        patch.default_appearance_set_at = None;
        Self(patch)
    }
}
impl From<ClientServerSettingsPatch> for ServerSettingsPatch {
    fn from(patch: ClientServerSettingsPatch) -> Self {
        ClientServerSettingsPatch::from(patch.0).0
    }
}

impl Serialize for ClientServerSettingsPatch {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        ServerSettingsPatch::from(self.clone()).serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for ClientServerSettingsPatch {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = serde_json::Value::deserialize(deserializer)?;
        if let Some(fields) = value.as_object_mut() {
            for key in [
                "defaultTheme",
                "defaultThemeSetAt",
                "defaultAppearance",
                "defaultAppearanceSetAt",
            ] {
                fields.remove(key);
            }
        }
        serde_json::from_value(value)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

fn deserialize_default_false<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Option::<bool>::deserialize(d).map(Option::unwrap_or_default)
}
fn deserialize_default_appearance<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<DefaultAppearancePreference, D::Error> {
    Option::<DefaultAppearancePreference>::deserialize(d).map(Option::unwrap_or_default)
}
fn deserialize_default_appearance_set_at<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<String, D::Error> {
    let value = Option::<String>::deserialize(d)?.unwrap_or_default();
    if value.encode_utf16().count() > 64 {
        return Err(serde::de::Error::custom(
            "appearance timestamp exceeds 64 UTF-16 units",
        ));
    }
    Ok(value)
}

mod patch_speech_acceleration {
    use serde::Deserialize;
    #[derive(Deserialize)]
    #[serde(try_from = "String")]
    struct Acceleration(String);
    impl TryFrom<String> for Acceleration {
        type Error = &'static str;
        fn try_from(value: String) -> Result<Self, Self::Error> {
            if value == "auto"
                || value == "cpu"
                || (value.strip_prefix("gpu:").is_some_and(|v| {
                    !v.is_empty() && !v.starts_with(['\n', '\r', '\u{2028}', '\u{2029}'])
                }) && value.encode_utf16().count() <= 500)
            {
                Ok(Self(value))
            } else {
                Err("invalid speech acceleration")
            }
        }
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
        crate::wire::optional::deserialize::<D, Acceleration>(d).map(|v| v.map(|v| v.0))
    }
    pub use crate::wire::optional::serialize;
}

mod patch_theme_string {
    use serde::Deserialize;
    #[derive(Deserialize)]
    #[serde(try_from = "String")]
    struct Preference(String);
    impl TryFrom<String> for Preference {
        type Error = &'static str;
        fn try_from(value: String) -> Result<Self, Self::Error> {
            if value.encode_utf16().count() <= 64 {
                Ok(Self(value))
            } else {
                Err("theme preference exceeds 64 UTF-16 units")
            }
        }
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
        crate::wire::optional::deserialize::<D, Preference>(d).map(|v| v.map(|v| v.0))
    }
    pub use crate::wire::optional::serialize;
}
