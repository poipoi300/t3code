use super::deserialize_null;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsServerUpsertKeybindingError {
    KeybindingsConfigError(Box<crate::keybindings::KeybindingsConfigError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsServerGetConfigError {
    KeybindingsConfigError(Box<crate::keybindings::KeybindingsConfigError>),
    ServerSettingsError(Box<crate::settings::ServerSettingsError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WsServerRefreshProvidersPayload {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cwd: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub fresh: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub refresh_models: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub model_catalog_only: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reload_secrets: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub workspace_snapshots_on_demand: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub usage_limits_command: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_catalogs_on_demand: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub omit_providers: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub machine_catalog: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsServerRefreshProvidersError {
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
    ProviderSetupError(Box<crate::provider_setup::ProviderSetupError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsServerUpdateProviderError {
    ServerProviderUpdateError(Box<crate::server::ServerProviderUpdateError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsProviderConsumeResetCreditError {
    ProviderSetupError(Box<crate::provider_setup::ProviderSetupError>),
    UsageLimitSourceError(Box<crate::provider_usage_limits::UsageLimitSourceError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ProviderSetupRpcError {
    ProviderSetupError(Box<crate::provider_setup::ProviderSetupError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsServerGetNpmReleaseError {
    ServerNpmReleaseError(Box<crate::server::ServerNpmReleaseError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsServerUpdateServerError {
    ServerSelfUpdateError(Box<crate::server::ServerSelfUpdateError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsServerRunStorageCleanupError {
    ServerSettingsError(Box<crate::settings::ServerSettingsError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsServerReclaimDatabaseSpaceError {
    DatabaseReclaimError(Box<crate::settings::DatabaseReclaimError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WsPeerLinksListSuccess {
    pub links: Vec<crate::peer_link::PeerLinkSummary>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsPeerLinksListError {
    PeerLinkError(Box<crate::peer_link::PeerLinkError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsPeerLinksAnswerRequestError {
    PeerLinkRequestError(Box<crate::peer_link::PeerLinkRequestError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WsServerUpdateSettingsPayload {
    pub patch: crate::settings::ServerSettingsPatch,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub provider_instance_mutation: Option<crate::provider_instance::ProviderInstanceMutation>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WsServerCheckSecretSourcePayload {
    pub source: crate::secret_reference::SecretReference,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsServerSearchAcpRegistryError {
    AcpRegistryOperationError(Box<crate::acp_registry::AcpRegistryOperationError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsServerListTextGeneratorModelsError {
    TextGeneratorError(Box<crate::settings::TextGeneratorError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsServerPrepareTriageError {
    ServerPrepareTriageError(Box<crate::server::ServerPrepareTriageError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsServerGetUsageSummaryError {
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
    UsageReadError(Box<crate::usage::UsageReadError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsCloudInstallRelayClientError {
    RelayClientInstallFailedError(Box<crate::relay_client::RelayClientInstallFailedError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum PullRequestRpcError {
    PullRequestUnavailableError(Box<crate::pull_request::PullRequestUnavailableError>),
    PullRequestOperationError(Box<crate::pull_request::PullRequestOperationError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsSourceControlLookupRepositoryError {
    SourceControlRepositoryError(Box<crate::source_control::SourceControlRepositoryError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsSourceControlSignInStartError {
    SourceControlSignInError(Box<crate::source_control::SourceControlSignInError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsProjectCloneStartError {
    SourceControlRepositoryError(Box<crate::source_control::SourceControlRepositoryError>),
    OrchestrationDispatchCommandError(
        Box<crate::orchestration_dispatch::OrchestrationDispatchCommandError>,
    ),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsProjectsSearchEntriesError {
    ProjectSearchEntriesError(Box<crate::project::ProjectSearchEntriesError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsProjectsSearchContentsError {
    ProjectSearchContentsError(Box<crate::project::ProjectSearchContentsError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsProjectsListEntriesError {
    ProjectListEntriesError(Box<crate::project::ProjectListEntriesError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsProjectsReadFileError {
    ProjectReadFileError(Box<crate::project::ProjectReadFileError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsProjectsWriteFileError {
    ProjectWriteFileError(Box<crate::project::ProjectWriteFileError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsProjectsMutateError {
    ProjectMutationError(Box<crate::project::ProjectMutationError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsProjectsEnsureScratchError {
    OrchestrationDispatchCommandError(
        Box<crate::orchestration_dispatch::OrchestrationDispatchCommandError>,
    ),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsShellOpenInEditorError {
    ExternalLauncherError(Box<crate::editor::ExternalLauncherError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsFilesystemBrowseError {
    FilesystemBrowseError(Box<crate::filesystem::FilesystemBrowseError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsAgentSessionsScanError {
    AgentSessionScanError(Box<crate::agent_sessions::AgentSessionScanError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsAgentSessionsImportError {
    AgentSessionImportProjectChangedError(
        Box<crate::agent_sessions::AgentSessionImportProjectChangedError>,
    ),
    AgentSessionImportProjectNotFoundError(
        Box<crate::agent_sessions::AgentSessionImportProjectNotFoundError>,
    ),
    AgentSessionScanError(Box<crate::agent_sessions::AgentSessionScanError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsAssetsCreateUrlError {
    AssetAccessError(Box<crate::assets::AssetAccessError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsAssetsPersistChatAttachmentsError {
    PersistChatAttachmentsError(Box<crate::chat_attachment::PersistChatAttachmentsError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsAttachmentsCreateUploadUrlError {
    AttachmentUploadSigningKeyError(Box<crate::assets::AttachmentUploadSigningKeyError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsMcpAppsCallToolError {
    McpAppRequestError(Box<crate::mcp_apps::McpAppRequestError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsProviderUploadFeedbackError {
    ProviderUploadFeedbackError(Box<crate::provider::ProviderUploadFeedbackError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsSubscribeVcsBranchError {
    GitManagerServiceError(Box<crate::git::GitManagerServiceError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsVcsPullError {
    GitCommandError(Box<crate::git::GitCommandError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WsVcsRefreshStatusPayload {
    pub cwd: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub local_only: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WsVcsCreateWorktreePayload {
    pub cwd: crate::base_schemas::TrimmedNonEmptyString,
    pub ref_name: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub new_ref_name: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub base_ref_name: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(deserialize_with = "deserialize_null")]
    pub path: (),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsVcsInitError {
    VcsError(Box<crate::vcs::VcsError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsReviewGetBranchCommitsError {
    ReviewDiffPreviewError(Box<crate::review::ReviewDiffPreviewError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsTerminalOpenError {
    TerminalError(Box<crate::terminal::TerminalError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsPreviewOpenError {
    PreviewError(Box<crate::preview::PreviewError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsPreviewClearProfileError {
    PreviewClearProfileError(Box<crate::preview::PreviewClearProfileError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsPreviewStopLocalServerError {
    PreviewStopLocalServerError(Box<crate::preview::PreviewStopLocalServerError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WsSubscribeDiscoveredLocalServersPayload {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub configured_urls: Option<Option<crate::preview::ConfiguredLocalServerUrls>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsDeviceTestHostError {
    DeviceError(Box<crate::device::DeviceError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsDeviceEasAccountError {
    DeviceEasAccountError(Box<crate::device::DeviceEasAccountError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsDeviceSimlockIdentityError {
    DeviceSimlockIdentityError(Box<crate::device::DeviceSimlockIdentityError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsOrchestrationV2DispatchCommandError {
    OrchestrationV2DispatchCommandError(
        Box<crate::orchestration_v2::OrchestrationV2DispatchCommandError>,
    ),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsOrchestrationV2GetTurnDiffError {
    OrchestrationGetTurnDiffError(Box<crate::checkpoint_diff::OrchestrationGetTurnDiffError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsOrchestrationV2GetFullThreadDiffError {
    OrchestrationGetFullThreadDiffError(
        Box<crate::checkpoint_diff::OrchestrationGetFullThreadDiffError>,
    ),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsOrchestrationV2SearchThreadError {
    OrchestrationV2SearchThreadError(
        Box<crate::orchestration_v2::OrchestrationV2SearchThreadError>,
    ),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsOrchestrationV2SearchThreadsError {
    OrchestrationSearchThreadsError(Box<crate::thread_search::OrchestrationSearchThreadsError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsOrchestrationV2GetArchivedShellSnapshotError {
    OrchestrationV2GetShellSnapshotError(
        Box<crate::orchestration_v2::OrchestrationV2GetShellSnapshotError>,
    ),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsOrchestrationV2GetThreadProjectionError {
    OrchestrationV2GetThreadProjectionError(
        Box<crate::orchestration_v2::OrchestrationV2GetThreadProjectionError>,
    ),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsOrchestrationV2StopWorkflowError {
    OrchestrationV2StopWorkflowError(
        Box<crate::orchestration_v2::OrchestrationV2StopWorkflowError>,
    ),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsOrchestrationV2ForceStopThreadError {
    OrchestrationV2ForceStopThreadError(
        Box<crate::orchestration_v2::OrchestrationV2ForceStopThreadError>,
    ),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsOrchestrationV2GetWorkflowScriptError {
    OrchestrationGetWorkflowScriptError(
        Box<crate::orchestration_v2::OrchestrationGetWorkflowScriptError>,
    ),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsOrchestrationV2GetNativeChildWorkError {
    OrchestrationV2GetNativeChildWorkError(
        Box<crate::orchestration_v2::OrchestrationV2GetNativeChildWorkError>,
    ),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsOrchestrationV2LaunchThreadError {
    OrchestrationV2ThreadLaunchError(
        Box<crate::orchestration_v2::OrchestrationV2ThreadLaunchError>,
    ),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsOrchestrationV2SubscribeCommandOutputError {
    OrchestrationV2CommandOutputError(
        Box<crate::orchestration_v2::OrchestrationV2CommandOutputError>,
    ),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WsSubscribeServerConfigPayload {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub environment_themes: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub usage_limit_sources: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub usage_limits_command: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub workspace_snapshots_on_demand: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_catalogs_on_demand: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_catalog_digests: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_status_deltas: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsScheduledTasksSubscribeError {
    ScheduledTaskError(Box<crate::scheduled_task::ScheduledTaskError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsGuestShareCreateError {
    GuestShareCreateError(Box<crate::guest_share::GuestShareCreateError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsSecretsAnswerRequestError {
    SecretRequestError(Box<crate::secret_request::SecretRequestError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WsSubscribeAuthAccessError {
    AuthAccessStreamError(Box<crate::auth::AuthAccessStreamError>),
    EnvironmentAuthorizationError(Box<crate::auth::EnvironmentAuthorizationError>),
}
