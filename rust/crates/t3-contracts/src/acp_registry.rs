use crate::base_schemas::TrimmedNonEmptyString;
use crate::base_schemas::{ProjectId, ThreadId};
use crate::model::ProviderOptionDescriptor;
use crate::provider_instance::ProviderInstanceId;
use crate::speech::True;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
pub const ACP_REGISTRY_MAX_AGENTS: usize = 512;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AcpRegistryDistribution {
    #[serde(rename = "binary")]
    Binary,
    #[serde(rename = "npx")]
    Npx,
    #[serde(rename = "uvx")]
    Uvx,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AcpRegistryIntegrity {
    #[serde(rename = "sha256")]
    Sha256,
    #[serde(rename = "registry")]
    Registry,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistrySearchInput {
    pub query: crate::base_schemas::TrimmedString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub limit: Option<Option<u64>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistrySearchAgent {
    pub id: TrimmedNonEmptyString,
    pub name: TrimmedNonEmptyString,
    pub version: TrimmedNonEmptyString,
    pub description: String,
    pub authors: Vec<String>,
    #[serde(with = "crate::wire::nullable")]
    pub license: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub website: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub repository: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub icon: Option<String>,
    pub distribution: AcpRegistryDistribution,
    pub integrity: AcpRegistryIntegrity,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistrySearchResult {
    pub agents: Vec<AcpRegistrySearchAgent>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryPrepareInput {
    pub agent_id: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryPrepareResult {
    pub agent_id: TrimmedNonEmptyString,
    pub version: TrimmedNonEmptyString,
    pub distribution: AcpRegistryDistribution,
    pub prepared: True,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryManagedBinaryUninstallInput {
    pub agent_id: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryManagedBinaryUninstallResult {
    pub agent_id: TrimmedNonEmptyString,
    pub removed: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AcpRegistryProbeAuthMethodType {
    #[serde(rename = "agent")]
    Agent,
    #[serde(rename = "env_var")]
    EnvVar,
    #[serde(rename = "terminal")]
    Terminal,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryProbeAuthMethod {
    pub id: TrimmedNonEmptyString,
    pub name: TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub description: Option<String>,
    pub r#type: AcpRegistryProbeAuthMethodType,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub command: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub env_var_names: Option<Vec<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub link: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryUrlAuthAction {
    pub elicitation_id: TrimmedNonEmptyString,
    pub url: String,
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub created_at: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub expires_at: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryAcceptUrlAuthInput {
    pub instance_id: ProviderInstanceId,
    pub elicitation_id: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryAcceptUrlAuthResult {
    pub accepted: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistrySession {
    pub session_id: TrimmedNonEmptyString,
    pub cwd: TrimmedNonEmptyString,
    pub additional_directories: Vec<TrimmedNonEmptyString>,
    #[serde(with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub updated_at: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub imported_thread_id: Option<ThreadId>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub native_history_pending: Option<bool>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryListSessionsInput {
    pub instance_id: ProviderInstanceId,
    pub project_id: ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub cursor: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryListSessionsResult {
    pub sessions: Vec<AcpRegistrySession>,
    #[serde(with = "crate::wire::nullable")]
    pub next_cursor: Option<String>,
    pub can_load: bool,
    pub can_resume: bool,
    pub can_delete: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryImportSessionInput {
    pub instance_id: ProviderInstanceId,
    pub project_id: ProjectId,
    pub session_id: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub title: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub updated_at: Option<Option<TrimmedNonEmptyString>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryImportSessionResult {
    pub thread_id: ThreadId,
    pub imported: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryDeleteSessionInput {
    pub instance_id: ProviderInstanceId,
    pub project_id: ProjectId,
    pub session_id: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryDeleteSessionResult {
    pub deleted: True,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryProviderConnection {
    pub api_type: TrimmedNonEmptyString,
    pub base_url: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryConfigurableProvider {
    pub provider_id: TrimmedNonEmptyString,
    pub supported: Vec<TrimmedNonEmptyString>,
    pub required: bool,
    #[serde(with = "crate::wire::nullable")]
    pub current: Option<AcpRegistryProviderConnection>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryListProvidersInput {
    pub instance_id: ProviderInstanceId,
    pub project_id: ProjectId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryListProvidersResult {
    pub providers: Vec<AcpRegistryConfigurableProvider>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistrySetProviderInput {
    pub instance_id: ProviderInstanceId,
    pub project_id: ProjectId,
    pub provider_id: TrimmedNonEmptyString,
    pub api_type: TrimmedNonEmptyString,
    pub base_url: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub headers: Option<BTreeMap<TrimmedNonEmptyString, String>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistrySetProviderResult {
    pub configured: True,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryDisableProviderInput {
    pub instance_id: ProviderInstanceId,
    pub project_id: ProjectId,
    pub provider_id: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryDisableProviderResult {
    pub disabled: True,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryLogoutInput {
    pub instance_id: ProviderInstanceId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryLogoutResult {
    pub logged_out: True,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryProbeModel {
    pub id: TrimmedNonEmptyString,
    pub name: TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub description: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistrySessionManagement {
    pub can_list: bool,
    pub can_load: bool,
    pub can_resume: bool,
    pub can_logout: bool,
    pub can_delete: bool,
    pub can_configure_providers: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryProbeResult {
    pub instance_id: ProviderInstanceId,
    pub ready: True,
    #[serde(with = "crate::wire::nullable")]
    pub icon: Option<String>,
    pub auth_methods: Vec<AcpRegistryProbeAuthMethod>,
    pub models: Vec<AcpRegistryProbeModel>,
    #[serde(with = "crate::wire::nullable")]
    pub current_model_id: Option<TrimmedNonEmptyString>,
    pub config_options: Vec<ProviderOptionDescriptor>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub model_option_ids: Option<Vec<TrimmedNonEmptyString>>,
    #[serde(default)]
    pub session_management: AcpRegistrySessionManagement,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AcpRegistryOperationErrorReason {
    #[serde(rename = "agent_not_configured")]
    AgentNotConfigured,
    #[serde(rename = "agent_not_found")]
    AgentNotFound,
    #[serde(rename = "archive_invalid")]
    ArchiveInvalid,
    #[serde(rename = "authentication_failed")]
    AuthenticationFailed,
    #[serde(rename = "checksum_mismatch")]
    ChecksumMismatch,
    #[serde(rename = "download_failed")]
    DownloadFailed,
    #[serde(rename = "install_failed")]
    InstallFailed,
    #[serde(rename = "instance_not_found")]
    InstanceNotFound,
    #[serde(rename = "logout_unsupported")]
    LogoutUnsupported,
    #[serde(rename = "logout_failed")]
    LogoutFailed,
    #[serde(rename = "probe_failed")]
    ProbeFailed,
    #[serde(rename = "project_not_found")]
    ProjectNotFound,
    #[serde(rename = "registry_unavailable")]
    RegistryUnavailable,
    #[serde(rename = "runner_unavailable")]
    RunnerUnavailable,
    #[serde(rename = "session_import_failed")]
    SessionImportFailed,
    #[serde(rename = "session_delete_unsupported")]
    SessionDeleteUnsupported,
    #[serde(rename = "session_delete_failed")]
    SessionDeleteFailed,
    #[serde(rename = "session_list_unsupported")]
    SessionListUnsupported,
    #[serde(rename = "session_resume_unsupported")]
    SessionResumeUnsupported,
    #[serde(rename = "providers_unsupported")]
    ProvidersUnsupported,
    #[serde(rename = "providers_list_failed")]
    ProvidersListFailed,
    #[serde(rename = "provider_configuration_failed")]
    ProviderConfigurationFailed,
    #[serde(rename = "unsupported_distribution")]
    UnsupportedDistribution,
    #[serde(rename = "unsupported_platform")]
    UnsupportedPlatform,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AcpRegistryOperationErrorTag {
    #[serde(rename = "AcpRegistryOperationError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRegistryOperationError {
    #[serde(rename = "_tag")]
    pub _tag: AcpRegistryOperationErrorTag,
    pub reason: AcpRegistryOperationErrorReason,
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub auth_methods: Option<Vec<AcpRegistryProbeAuthMethod>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub auth_action: Option<AcpRegistryUrlAuthAction>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub cause: Option<Value>,
}
