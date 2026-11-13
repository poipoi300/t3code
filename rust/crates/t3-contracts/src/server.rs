//! Server configuration, lifecycle, provider catalogs, and diagnostics wire contracts.

use crate::{
    acp_registry::AcpRegistryUrlAuthAction,
    auth::{AuthClientPresentationMetadata, EncodedNumber, ServerAuthDescriptor},
    base_schemas::{
        IsoDateTime, NonNegativeInt, PositiveInt, ProjectId, ThreadId, TrimmedNonEmptyString,
    },
    editor::{EditorId, FileManagerRevealKind, RemoteOpenTarget},
    environment::{ExecutionEnvironmentDescriptor, ServerSelfUpdateMethod},
    ipc::DesktopUpdateChannelSchema,
    keybindings::{KeybindingCommand, KeybindingValue, KeybindingWhen, ResolvedKeybindingsConfig},
    managed_policy::ManagedPolicySummary,
    model::{ModelCapabilities, ProviderOptionDescriptor},
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
    provider_policy::RuntimeMode,
    provider_usage_limits::{ServerProviderUsageLimits, UsageLimitSourceSnapshots},
    resource_telemetry::{WireLiteral, WireOption},
    settings::ServerSettings,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ServerCompatibleArray<T>(pub Vec<T>);
impl<'de, T: serde::de::DeserializeOwned> Deserialize<'de> for ServerCompatibleArray<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        crate::wire::deserialize_lenient_array(deserializer).map(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeybindingsMalformedConfigIssueKind {
    #[serde(rename = "keybindings.malformed-config")]
    KeybindingsMalformedConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeybindingsInvalidEntryIssueKind {
    #[serde(rename = "keybindings.invalid-entry")]
    KeybindingsInvalidEntry,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeybindingsMalformedConfigIssue {
    pub kind: KeybindingsMalformedConfigIssueKind,
    pub message: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeybindingsInvalidEntryIssue {
    pub kind: KeybindingsInvalidEntryIssueKind,
    pub message: TrimmedNonEmptyString,
    pub index: EncodedNumber,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerConfigIssue {
    MalformedConfig(KeybindingsMalformedConfigIssue),
    InvalidEntry(KeybindingsInvalidEntryIssue),
}

pub type ServerConfigIssues = ServerCompatibleArray<ServerConfigIssue>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerProviderState {
    #[serde(rename = "ready")]
    Ready,
    #[serde(rename = "warning")]
    Warning,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "disabled")]
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerProviderAuthStatus {
    #[serde(rename = "authenticated")]
    Authenticated,
    #[serde(rename = "unauthenticated")]
    Unauthenticated,
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderAuth {
    pub status: ServerProviderAuthStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub r#type: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub label: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub email: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub action: Option<Option<AcpRegistryUrlAuthAction>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub can_logout: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub subscription_sharing: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub profile_id: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub workspace_id: Option<Option<TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerProviderNewBadge {
    #[serde(rename = "new")]
    New,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderModel {
    pub slug: TrimmedNonEmptyString,
    pub name: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub short_name: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub sub_provider: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub aliases: Option<Option<Vec<TrimmedNonEmptyString>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub badge: Option<Option<ServerProviderNewBadge>>,
    pub is_custom: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub is_default: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub is_legacy: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub supports_text_generation: Option<Option<bool>>,
    #[serde(with = "crate::wire::nullable")]
    pub capabilities: Option<ModelCapabilities>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderUpdateRequiredModel {
    pub slug: TrimmedNonEmptyString,
    pub name: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub badge: Option<Option<ServerProviderNewBadge>>,
    pub min_version: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderSlashCommandInput {
    pub hint: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderSlashCommand {
    pub name: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub description: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub input: Option<Option<ServerProviderSlashCommandInput>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderSkill {
    pub name: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub description: Option<Option<TrimmedNonEmptyString>>,
    pub path: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub scope: Option<Option<TrimmedNonEmptyString>>,
    pub enabled: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub display_name: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub short_description: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub user_invocation_only: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub user_invocable: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderWorkspaceSnapshot {
    pub cwd: TrimmedNonEmptyString,
    pub checked_at: IsoDateTime,
    pub slash_commands: Vec<ServerProviderSlashCommand>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub slash_commands_pending: Option<Option<bool>>,
    pub skills: Vec<ServerProviderSkill>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub option_descriptors: Option<Option<Vec<ProviderOptionDescriptor>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub models: Option<Option<Vec<ServerProviderModel>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub models_pending: Option<Option<bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerProviderAvailability {
    #[serde(rename = "available")]
    Available,
    #[serde(rename = "unavailable")]
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderContinuation {
    pub group_key: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerProviderCompatibilityStatus {
    #[serde(rename = "unknown")]
    Unknown,
    #[serde(rename = "supported")]
    Supported,
    #[serde(rename = "graceful")]
    Graceful,
    #[serde(rename = "unsupported")]
    Unsupported,
    #[serde(rename = "broken")]
    Broken,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderCompatibilityAdvisory {
    pub status: ServerProviderCompatibilityStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub latest_version_status: Option<ServerProviderCompatibilityStatus>,
    #[serde(with = "crate::wire::nullable")]
    pub message: Option<TrimmedNonEmptyString>,
    #[serde(with = "crate::wire::nullable")]
    pub recommended_version: Option<TrimmedNonEmptyString>,
    #[serde(with = "crate::wire::nullable")]
    pub recommended_range: Option<TrimmedNonEmptyString>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerProviderVersionAdvisoryStatus {
    #[serde(rename = "unknown")]
    Unknown,
    #[serde(rename = "current")]
    Current,
    #[serde(rename = "behind_latest")]
    BehindLatest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderVersionAdvisory {
    pub status: ServerProviderVersionAdvisoryStatus,
    #[serde(with = "crate::wire::nullable")]
    pub current_version: Option<TrimmedNonEmptyString>,
    #[serde(with = "crate::wire::nullable")]
    pub latest_version: Option<TrimmedNonEmptyString>,
    #[serde(with = "crate::wire::nullable")]
    pub update_command: Option<TrimmedNonEmptyString>,
    #[serde(default)]
    pub can_update: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub can_install_version: Option<bool>,
    #[serde(with = "crate::wire::nullable")]
    pub checked_at: Option<IsoDateTime>,
    #[serde(with = "crate::wire::nullable")]
    pub message: Option<TrimmedNonEmptyString>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerProviderUpdateStatus {
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "queued")]
    Queued,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "succeeded")]
    Succeeded,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "unchanged")]
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderUpdateState {
    pub status: ServerProviderUpdateStatus,
    #[serde(with = "crate::wire::nullable")]
    pub started_at: Option<IsoDateTime>,
    #[serde(with = "crate::wire::nullable")]
    pub finished_at: Option<IsoDateTime>,
    #[serde(with = "crate::wire::nullable")]
    pub message: Option<TrimmedNonEmptyString>,
    #[serde(with = "crate::wire::nullable")]
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderSetup {
    pub can_authenticate: bool,
    pub can_install: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub documentation_url: Option<TrimmedNonEmptyString>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderNativeSessions {
    pub can_list: bool,
    pub can_load: bool,
    pub can_resume: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub can_delete: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderRuntimePaths {
    pub home_path: TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub shadow_home_path: Option<TrimmedNonEmptyString>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProvider {
    pub instance_id: ProviderInstanceId,
    pub driver: ProviderDriverKind,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub display_name: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub accent_color: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub icon_url: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub badge_label: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub continuation: Option<Option<ServerProviderContinuation>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub show_interaction_mode_toggle: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reports_context_window: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub supported_runtime_modes: Option<Option<ServerCompatibleArray<RuntimeMode>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub supports_conversation_rollback: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub supports_text_generation: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub setup: Option<Option<ServerProviderSetup>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub native_sessions: Option<Option<ServerProviderNativeSessions>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub configurable_providers: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub runtime_paths: Option<ServerProviderRuntimePaths>,
    pub enabled: bool,
    pub installed: bool,
    #[serde(with = "crate::wire::nullable")]
    pub version: Option<TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub installed_version: Option<Option<TrimmedNonEmptyString>>,
    pub status: ServerProviderState,
    pub auth: ServerProviderAuth,
    pub checked_at: IsoDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub message: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub availability: Option<Option<ServerProviderAvailability>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub unavailable_reason: Option<Option<TrimmedNonEmptyString>>,
    pub models: Vec<ServerProviderModel>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub update_required_models: Option<Vec<ServerProviderUpdateRequiredModel>>,
    #[serde(default)]
    pub slash_commands: Vec<ServerProviderSlashCommand>,
    #[serde(default)]
    pub skills: Vec<ServerProviderSkill>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub workspace_snapshots: Option<Vec<ServerProviderWorkspaceSnapshot>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub usage_limits: Option<Option<ServerProviderUsageLimits>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub version_advisory: Option<ServerProviderVersionAdvisory>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub compatibility_advisory: Option<ServerProviderCompatibilityAdvisory>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub update_state: Option<ServerProviderUpdateState>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub catalog_digest: Option<TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub loaded_catalog_digest: Option<TrimmedNonEmptyString>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderDelta {
    pub instance_id: ProviderInstanceId,
    pub driver: ProviderDriverKind,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub display_name: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub accent_color: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub icon_url: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub badge_label: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub continuation: Option<Option<ServerProviderContinuation>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub show_interaction_mode_toggle: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reports_context_window: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub supported_runtime_modes: Option<Option<ServerCompatibleArray<RuntimeMode>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub supports_conversation_rollback: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub supports_text_generation: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub setup: Option<Option<ServerProviderSetup>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub native_sessions: Option<Option<ServerProviderNativeSessions>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub configurable_providers: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub runtime_paths: Option<ServerProviderRuntimePaths>,
    pub enabled: bool,
    pub installed: bool,
    #[serde(with = "crate::wire::nullable")]
    pub version: Option<TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub installed_version: Option<Option<TrimmedNonEmptyString>>,
    pub status: ServerProviderState,
    pub auth: ServerProviderAuth,
    pub checked_at: IsoDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub message: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub availability: Option<Option<ServerProviderAvailability>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub unavailable_reason: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub models: Option<Vec<ServerProviderModel>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub update_required_models: Option<Vec<ServerProviderUpdateRequiredModel>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub slash_commands: Option<Vec<ServerProviderSlashCommand>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub skills: Option<Vec<ServerProviderSkill>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub workspace_snapshots: Option<Vec<ServerProviderWorkspaceSnapshot>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub usage_limits: Option<Option<ServerProviderUsageLimits>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub version_advisory: Option<ServerProviderVersionAdvisory>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub compatibility_advisory: Option<ServerProviderCompatibilityAdvisory>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub update_state: Option<ServerProviderUpdateState>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub catalog_digest: Option<TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub loaded_catalog_digest: Option<TrimmedNonEmptyString>,
}

pub type ServerProviders = ServerCompatibleArray<ServerProvider>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OtlpEndpointSource {
    #[serde(rename = "environment")]
    Environment,
    #[serde(rename = "bootstrap")]
    Bootstrap,
    #[serde(rename = "settings")]
    Settings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerObservability {
    pub logs_directory_path: TrimmedNonEmptyString,
    pub local_tracing_enabled: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub otlp_traces_url: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub otlp_traces_source: Option<Option<OtlpEndpointSource>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub otlp_traces_variable: Option<Option<TrimmedNonEmptyString>>,
    pub otlp_traces_enabled: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub otlp_metrics_url: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub otlp_metrics_source: Option<Option<OtlpEndpointSource>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub otlp_metrics_variable: Option<Option<TrimmedNonEmptyString>>,
    pub otlp_metrics_enabled: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub otlp_logs_url: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub otlp_logs_source: Option<Option<OtlpEndpointSource>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub otlp_logs_variable: Option<Option<TrimmedNonEmptyString>>,
    #[serde(default)]
    pub otlp_logs_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OtlpSignal {
    #[serde(rename = "traces")]
    Traces,
    #[serde(rename = "metrics")]
    Metrics,
    #[serde(rename = "logs")]
    Logs,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtlpEndpointCheckInput {
    pub signal: OtlpSignal,
    pub url: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OtlpEndpointAcceptedTag {
    #[serde(rename = "Accepted")]
    Accepted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtlpEndpointAccepted {
    #[serde(rename = "_tag")]
    pub tag: OtlpEndpointAcceptedTag,
    pub latency_ms: EncodedNumber,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OtlpEndpointRejectedTag {
    #[serde(rename = "Rejected")]
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtlpEndpointRejected {
    #[serde(rename = "_tag")]
    pub tag: OtlpEndpointRejectedTag,
    pub status: EncodedNumber,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OtlpEndpointUnreachableTag {
    #[serde(rename = "Unreachable")]
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtlpEndpointUnreachable {
    #[serde(rename = "_tag")]
    pub tag: OtlpEndpointUnreachableTag,
    pub timed_out: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OtlpEndpointCheckResult {
    Accepted(OtlpEndpointAccepted),
    Rejected(OtlpEndpointRejected),
    Unreachable(OtlpEndpointUnreachable),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerPrepareTriageInput {
    pub client: AuthClientPresentationMetadata,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerPrepareTriageResult {
    pub prompt: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerPrepareTriageErrorTag {
    #[serde(rename = "ServerPrepareTriageError")]
    ServerPrepareTriageError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerPrepareTriageError {
    #[serde(rename = "_tag")]
    pub tag: ServerPrepareTriageErrorTag,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerTraceDiagnosticsErrorKind {
    #[serde(rename = "trace-file-not-found")]
    TraceFileNotFound,
    #[serde(rename = "trace-file-read-failed")]
    TraceFileReadFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerTraceDiagnosticsSpanSummary {
    pub name: TrimmedNonEmptyString,
    pub count: NonNegativeInt,
    pub failure_count: NonNegativeInt,
    pub total_duration_ms: EncodedNumber,
    pub average_duration_ms: EncodedNumber,
    pub max_duration_ms: EncodedNumber,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerTraceDiagnosticsFailureSummary {
    pub name: TrimmedNonEmptyString,
    pub cause: TrimmedNonEmptyString,
    pub count: NonNegativeInt,
    pub last_seen_at: IsoDateTime,
    pub trace_id: TrimmedNonEmptyString,
    pub span_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerTraceDiagnosticsRecentFailure {
    pub name: TrimmedNonEmptyString,
    pub cause: TrimmedNonEmptyString,
    pub duration_ms: EncodedNumber,
    pub ended_at: IsoDateTime,
    pub trace_id: TrimmedNonEmptyString,
    pub span_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerTraceDiagnosticsSpanOccurrence {
    pub name: TrimmedNonEmptyString,
    pub duration_ms: EncodedNumber,
    pub ended_at: IsoDateTime,
    pub trace_id: TrimmedNonEmptyString,
    pub span_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerTraceDiagnosticsLogEvent {
    pub span_name: TrimmedNonEmptyString,
    pub level: TrimmedNonEmptyString,
    pub message: TrimmedNonEmptyString,
    pub seen_at: IsoDateTime,
    pub trace_id: TrimmedNonEmptyString,
    pub span_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerTraceDiagnosticsFailure {
    pub kind: ServerTraceDiagnosticsErrorKind,
    pub message: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerTraceDiagnosticsResult {
    pub trace_file_path: TrimmedNonEmptyString,
    pub scanned_file_paths: Vec<TrimmedNonEmptyString>,
    pub read_at: IsoDateTime,
    pub record_count: NonNegativeInt,
    pub parse_error_count: NonNegativeInt,
    pub first_span_at: WireOption<IsoDateTime>,
    pub last_span_at: WireOption<IsoDateTime>,
    pub failure_count: NonNegativeInt,
    pub interruption_count: NonNegativeInt,
    pub slow_span_threshold_ms: NonNegativeInt,
    pub slow_span_count: NonNegativeInt,
    pub log_level_counts: std::collections::BTreeMap<TrimmedNonEmptyString, NonNegativeInt>,
    pub top_spans_by_count: Vec<ServerTraceDiagnosticsSpanSummary>,
    pub slowest_spans: Vec<ServerTraceDiagnosticsSpanOccurrence>,
    pub common_failures: Vec<ServerTraceDiagnosticsFailureSummary>,
    pub latest_failures: Vec<ServerTraceDiagnosticsRecentFailure>,
    pub latest_warning_and_error_logs: Vec<ServerTraceDiagnosticsLogEvent>,
    pub partial_failure: WireOption<bool>,
    pub error: WireOption<ServerTraceDiagnosticsFailure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerProcessSignal {
    #[serde(rename = "SIGINT")]
    SIGINT,
    #[serde(rename = "SIGKILL")]
    SIGKILL,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProcessDiagnosticsEntry {
    pub pid: PositiveInt,
    pub start_time_ms: NonNegativeInt,
    pub ppid: NonNegativeInt,
    pub pgid: WireOption<i64>,
    pub status: TrimmedNonEmptyString,
    pub cpu_percent: EncodedNumber,
    pub rss_bytes: NonNegativeInt,
    pub elapsed: TrimmedNonEmptyString,
    pub command: TrimmedNonEmptyString,
    pub depth: NonNegativeInt,
    pub child_pids: Vec<PositiveInt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProcessDiagnosticsFailure {
    pub message: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProcessDiagnosticsResult {
    pub server_pid: PositiveInt,
    pub read_at: IsoDateTime,
    pub process_count: NonNegativeInt,
    pub total_rss_bytes: NonNegativeInt,
    pub total_cpu_percent: EncodedNumber,
    pub processes: Vec<ServerProcessDiagnosticsEntry>,
    pub error: WireOption<ServerProcessDiagnosticsFailure>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProcessResourceHistoryInput {
    pub window_ms: NonNegativeInt,
    pub bucket_ms: NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProcessResourceHistoryBucket {
    pub started_at: IsoDateTime,
    pub ended_at: IsoDateTime,
    pub avg_cpu_percent: EncodedNumber,
    pub max_cpu_percent: EncodedNumber,
    pub max_rss_bytes: NonNegativeInt,
    pub max_process_count: NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProcessResourceHistorySummary {
    pub process_key: TrimmedNonEmptyString,
    pub pid: PositiveInt,
    pub ppid: NonNegativeInt,
    pub command: TrimmedNonEmptyString,
    pub depth: NonNegativeInt,
    pub is_server_root: bool,
    pub first_seen_at: IsoDateTime,
    pub last_seen_at: IsoDateTime,
    pub current_cpu_percent: EncodedNumber,
    pub avg_cpu_percent: EncodedNumber,
    pub max_cpu_percent: EncodedNumber,
    pub cpu_seconds_approx: EncodedNumber,
    pub current_rss_bytes: NonNegativeInt,
    pub max_rss_bytes: NonNegativeInt,
    pub sample_count: NonNegativeInt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerProcessResourceHistoryFailureTag {
    #[serde(rename = "ProcessDiagnosticsQueryTimeoutError")]
    ProcessDiagnosticsQueryTimeoutError,
    #[serde(rename = "ProcessDiagnosticsQueryFailedError")]
    ProcessDiagnosticsQueryFailedError,
    #[serde(rename = "ProcessDiagnosticsServerProcessSignalError")]
    ProcessDiagnosticsServerProcessSignalError,
    #[serde(rename = "ProcessDiagnosticsNotDescendantError")]
    ProcessDiagnosticsNotDescendantError,
    #[serde(rename = "ProcessDiagnosticsSignalFailedError")]
    ProcessDiagnosticsSignalFailedError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProcessResourceHistoryFailure {
    pub failure_tag: ServerProcessResourceHistoryFailureTag,
    pub message: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProcessResourceHistoryResult {
    pub read_at: IsoDateTime,
    pub window_ms: NonNegativeInt,
    pub bucket_ms: NonNegativeInt,
    pub sample_interval_ms: NonNegativeInt,
    pub retained_sample_count: NonNegativeInt,
    pub total_cpu_seconds_approx: EncodedNumber,
    pub buckets: Vec<ServerProcessResourceHistoryBucket>,
    pub top_processes: Vec<ServerProcessResourceHistorySummary>,
    pub error: WireOption<ServerProcessResourceHistoryFailure>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSignalProcessInput {
    pub pid: PositiveInt,
    pub start_time_ms: NonNegativeInt,
    pub signal: ServerProcessSignal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSignalProcessResult {
    pub pid: PositiveInt,
    pub signal: ServerProcessSignal,
    pub signaled: bool,
    pub message: WireOption<TrimmedNonEmptyString>,
}

pub type EnvironmentThemeColor = String;

pub type EnvironmentThemeId = String;

pub type EnvironmentThemeColors = std::collections::BTreeMap<String, TrimmedNonEmptyString>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentThemeAppearance {
    #[serde(rename = "light")]
    Light,
    #[serde(rename = "dark")]
    Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentThemeVariants {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub light: Option<Option<EnvironmentThemeColors>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub dark: Option<Option<EnvironmentThemeColors>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentThemeFile {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub version: Option<Option<WireLiteral<1>>>,
    pub name: TrimmedNonEmptyString,
    pub appearance: EnvironmentThemeAppearance,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub canvas: Option<Option<EnvironmentThemeColor>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub accent: Option<Option<EnvironmentThemeColor>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub colors: Option<Option<EnvironmentThemeColors>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub variants: Option<Option<EnvironmentThemeVariants>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentTheme {
    pub id: EnvironmentThemeId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub version: Option<Option<WireLiteral<1>>>,
    pub name: TrimmedNonEmptyString,
    pub appearance: EnvironmentThemeAppearance,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub canvas: Option<Option<EnvironmentThemeColor>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub accent: Option<Option<EnvironmentThemeColor>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub colors: Option<Option<EnvironmentThemeColors>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub variants: Option<Option<EnvironmentThemeVariants>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerDirectEndpointKind {
    #[serde(rename = "lan")]
    Lan,
    #[serde(rename = "tailnet")]
    Tailnet,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerDirectEndpoint {
    pub kind: ServerDirectEndpointKind,
    pub http_base_url: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerEventLoopStall {
    pub detected_at: IsoDateTime,
    pub duration_ms: NonNegativeInt,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub load_average: Option<EncodedNumber>,
    pub cpu_count: PositiveInt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub environment: ExecutionEnvironmentDescriptor,
    pub auth: ServerAuthDescriptor,
    pub cwd: TrimmedNonEmptyString,
    pub keybindings_config_path: TrimmedNonEmptyString,
    pub keybindings: ResolvedKeybindingsConfig,
    pub issues: ServerConfigIssues,
    pub providers: ServerProviders,
    pub available_editors: ServerCompatibleArray<EditorId>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub remote_open_targets: Option<ServerCompatibleArray<RemoteOpenTarget>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub direct_endpoints: Option<ServerCompatibleArray<ServerDirectEndpoint>>,
    pub observability: ServerObservability,
    pub settings: ServerSettings,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub managed_policy: Option<ManagedPolicySummary>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub shell_resume_completion_marker: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub shell_thread_families: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub thread_link_metadata: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub shell_older_settled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub shell_reveal_in_file_manager: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub shell_reveal_in_file_manager_kind: Option<FileManagerRevealKind>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub thread_resume_completion_marker: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub thread_snapshot_pagination: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub thread_find: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub thread_find_progressive: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub secret_provider_settings: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub secret_references: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub secret_request_references: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub scratch_workspace_root: Option<TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub new_projects_root: Option<TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub last_event_loop_stall: Option<ServerEventLoopStall>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub environment_themes: Option<Option<Vec<EnvironmentTheme>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub usage_limit_sources: Option<Option<UsageLimitSourceSnapshots>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerUpsertKeybindingReplaceTarget {
    pub key: KeybindingValue,
    pub command: KeybindingCommand,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub when: Option<Option<KeybindingWhen>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerUpsertKeybindingInput {
    pub key: KeybindingValue,
    pub command: KeybindingCommand,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub when: Option<Option<KeybindingWhen>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub replace: Option<Option<ServerUpsertKeybindingReplaceTarget>>,
}

pub type ServerRemoveKeybindingInput = ServerUpsertKeybindingReplaceTarget;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerUpsertKeybindingResult {
    pub keybindings: ResolvedKeybindingsConfig,
    pub issues: ServerConfigIssues,
}

pub type ServerRemoveKeybindingResult = ServerUpsertKeybindingResult;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfigUpdatedPayload {
    pub issues: ServerConfigIssues,
    pub providers: ServerProviders,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub settings: Option<Option<ServerSettings>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfigKeybindingsUpdatedPayload {
    pub keybindings: ResolvedKeybindingsConfig,
    pub issues: ServerConfigIssues,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfigProviderStatusesPayload {
    pub providers: ServerProviders,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfigProviderStatusDeltasPayload {
    pub providers: ServerCompatibleArray<ServerProviderDelta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfigSettingsUpdatedPayload {
    pub settings: ServerSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfigEnvironmentThemesUpdatedPayload {
    pub themes: Vec<EnvironmentTheme>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfigUsageLimitSourcesUpdatedPayload {
    pub sources: UsageLimitSourceSnapshots,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerConfigStreamSnapshotType {
    #[serde(rename = "snapshot")]
    Snapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfigStreamSnapshotEvent {
    pub version: WireLiteral<1>,
    pub r#type: ServerConfigStreamSnapshotType,
    pub config: ServerConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerConfigStreamKeybindingsUpdatedType {
    #[serde(rename = "keybindingsUpdated")]
    KeybindingsUpdated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfigStreamKeybindingsUpdatedEvent {
    pub version: WireLiteral<1>,
    pub r#type: ServerConfigStreamKeybindingsUpdatedType,
    pub payload: ServerConfigKeybindingsUpdatedPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerConfigStreamProviderStatusesType {
    #[serde(rename = "providerStatuses")]
    ProviderStatuses,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfigStreamProviderStatusesEvent {
    pub version: WireLiteral<1>,
    pub r#type: ServerConfigStreamProviderStatusesType,
    pub payload: ServerConfigProviderStatusesPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerConfigStreamProviderStatusDeltasType {
    #[serde(rename = "providerStatusDeltas")]
    ProviderStatusDeltas,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfigStreamProviderStatusDeltasEvent {
    pub version: WireLiteral<1>,
    pub r#type: ServerConfigStreamProviderStatusDeltasType,
    pub payload: ServerConfigProviderStatusDeltasPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerConfigStreamSettingsUpdatedType {
    #[serde(rename = "settingsUpdated")]
    SettingsUpdated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfigStreamSettingsUpdatedEvent {
    pub version: WireLiteral<1>,
    pub r#type: ServerConfigStreamSettingsUpdatedType,
    pub payload: ServerConfigSettingsUpdatedPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerConfigStreamEnvironmentThemesUpdatedType {
    #[serde(rename = "environmentThemesUpdated")]
    EnvironmentThemesUpdated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfigStreamEnvironmentThemesUpdatedEvent {
    pub version: WireLiteral<1>,
    pub r#type: ServerConfigStreamEnvironmentThemesUpdatedType,
    pub payload: ServerConfigEnvironmentThemesUpdatedPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerConfigStreamUsageLimitSourcesUpdatedType {
    #[serde(rename = "usageLimitSourcesUpdated")]
    UsageLimitSourcesUpdated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfigStreamUsageLimitSourcesUpdatedEvent {
    pub version: WireLiteral<1>,
    pub r#type: ServerConfigStreamUsageLimitSourcesUpdatedType,
    pub payload: ServerConfigUsageLimitSourcesUpdatedPayload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerConfigStreamEvent {
    Snapshot(Box<ServerConfigStreamSnapshotEvent>),
    KeybindingsUpdated(ServerConfigStreamKeybindingsUpdatedEvent),
    ProviderStatuses(ServerConfigStreamProviderStatusesEvent),
    ProviderStatusDeltas(ServerConfigStreamProviderStatusDeltasEvent),
    SettingsUpdated(Box<ServerConfigStreamSettingsUpdatedEvent>),
    EnvironmentThemesUpdated(ServerConfigStreamEnvironmentThemesUpdatedEvent),
    UsageLimitSourcesUpdated(ServerConfigStreamUsageLimitSourcesUpdatedEvent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerSelfUpdateOutcomeStatus {
    #[serde(rename = "committed")]
    Committed,
    #[serde(rename = "rolled-back")]
    RolledBack,
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSelfUpdateOutcome {
    pub id: TrimmedNonEmptyString,
    pub from_version: TrimmedNonEmptyString,
    pub target_version: TrimmedNonEmptyString,
    pub status: ServerSelfUpdateOutcomeStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub reason: Option<TrimmedNonEmptyString>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerLifecycleReadyPayload {
    pub at: IsoDateTime,
    pub environment: ExecutionEnvironmentDescriptor,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub update_outcome: Option<ServerSelfUpdateOutcome>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerBootstrapStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "complete")]
    Complete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerLifecycleWelcomePayload {
    pub environment: ExecutionEnvironmentDescriptor,
    pub cwd: TrimmedNonEmptyString,
    pub project_name: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub bootstrap_status: Option<Option<ServerBootstrapStatus>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub bootstrap_project_id: Option<Option<ProjectId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub bootstrap_thread_id: Option<Option<ThreadId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub bootstrap_project_created: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub bootstrap_thread_created: Option<Option<bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerLegacyThreadMigrationStatus {
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "complete")]
    Complete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerLifecycleLegacyThreadMigrationPayload {
    pub status: ServerLegacyThreadMigrationStatus,
    pub total_thread_count: NonNegativeInt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerLifecycleStreamWelcomeType {
    #[serde(rename = "welcome")]
    Welcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerLifecycleStreamWelcomeEvent {
    pub version: WireLiteral<1>,
    pub sequence: NonNegativeInt,
    pub r#type: ServerLifecycleStreamWelcomeType,
    pub payload: ServerLifecycleWelcomePayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerLifecycleStreamReadyType {
    #[serde(rename = "ready")]
    Ready,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerLifecycleStreamReadyEvent {
    pub version: WireLiteral<1>,
    pub sequence: NonNegativeInt,
    pub r#type: ServerLifecycleStreamReadyType,
    pub payload: ServerLifecycleReadyPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerLifecycleStreamLegacyThreadMigrationType {
    #[serde(rename = "legacyThreadMigration")]
    LegacyThreadMigration,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerLifecycleStreamLegacyThreadMigrationEvent {
    pub version: WireLiteral<1>,
    pub sequence: NonNegativeInt,
    pub r#type: ServerLifecycleStreamLegacyThreadMigrationType,
    pub payload: ServerLifecycleLegacyThreadMigrationPayload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerLifecycleStreamEvent {
    Welcome(ServerLifecycleStreamWelcomeEvent),
    Ready(ServerLifecycleStreamReadyEvent),
    LegacyThreadMigration(ServerLifecycleStreamLegacyThreadMigrationEvent),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderUpdatedPayload {
    pub providers: ServerProviders,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderUpdateInput {
    pub provider: ProviderDriverKind,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub target_version: Option<TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub instance_id: Option<ProviderInstanceId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerProviderUpdateErrorTag {
    #[serde(rename = "ServerProviderUpdateError")]
    ServerProviderUpdateError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderUpdateError {
    #[serde(rename = "_tag")]
    pub tag: ServerProviderUpdateErrorTag,
    pub provider: ProviderDriverKind,
    pub reason: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerNpmReleaseInput {
    pub spec: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerNpmRelease {
    pub version: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub database_migration: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerNpmReleaseErrorTag {
    #[serde(rename = "ServerNpmReleaseError")]
    ServerNpmReleaseError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerNpmReleaseError {
    #[serde(rename = "_tag")]
    pub tag: ServerNpmReleaseErrorTag,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSelfUpdateInput {
    pub target_version: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub channel: Option<DesktopUpdateChannelSchema>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub target_database_migration: Option<i64>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSelfUpdateResult {
    pub target_version: TrimmedNonEmptyString,
    pub method: ServerSelfUpdateMethod,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub update_id: Option<TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub desktop_update_token: Option<TrimmedNonEmptyString>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopUpdateCommitInput {
    pub request_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerSelfUpdateProgressStage {
    #[serde(rename = "downloading")]
    Downloading,
    #[serde(rename = "installing")]
    Installing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerSelfUpdateProgressType {
    #[serde(rename = "progress")]
    Progress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerSelfUpdateCompleteType {
    #[serde(rename = "complete")]
    Complete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSelfUpdateProgress {
    pub r#type: ServerSelfUpdateProgressType,
    pub stage: ServerSelfUpdateProgressStage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSelfUpdateComplete {
    pub r#type: ServerSelfUpdateCompleteType,
    pub result: ServerSelfUpdateResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerSelfUpdateProgressEvent {
    Progress(ServerSelfUpdateProgress),
    Complete(ServerSelfUpdateComplete),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerSelfUpdateErrorTag {
    #[serde(rename = "ServerSelfUpdateError")]
    ServerSelfUpdateError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSelfUpdateError {
    #[serde(rename = "_tag")]
    pub tag: ServerSelfUpdateErrorTag,
    pub reason: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

crate::redacted_debug!(ServerSelfUpdateResult { target_version, method, update_id } redact { desktop_update_token });
