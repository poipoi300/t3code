use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingBackgroundTaskCommand {
    #[serde(
        rename = "command",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub command: Option<Option<String>>,
    #[serde(
        rename = "itemId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub item_id: Option<Option<crate::base_schemas::TurnItemId>>,
    #[serde(
        rename = "sourceThreadId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub source_thread_id: Option<Option<crate::base_schemas::ThreadId>>,
    #[serde(rename = "taskId")]
    pub task_id: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "description",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub description: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "parentTaskId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub parent_task_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "nativeItemId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub native_item_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "awaited",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub awaited: Option<Option<LiteralTrue>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CommandNotificationSourceKind {
    #[serde(rename = "command")]
    Command,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandNotificationSource {
    #[serde(rename = "kind")]
    pub kind: CommandNotificationSourceKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TurnItemCommandExecutionCompletionWake {
    #[serde(rename = "always")]
    Always,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemCommandExecution {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "input")]
    pub input: String,
    #[serde(
        rename = "output",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub output: Option<Option<String>>,
    #[serde(
        rename = "outputOmitted",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub output_omitted: Option<Option<bool>>,
    #[serde(
        rename = "outputIndicatesFailure",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub output_indicates_failure: Option<Option<bool>>,
    #[serde(
        rename = "exitCode",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub exit_code: Option<Option<i64>>,
    #[serde(
        rename = "monitor",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub monitor: Option<Option<bool>>,
    #[serde(
        rename = "completionWake",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub completion_wake: Option<Option<TurnItemCommandExecutionCompletionWake>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadStop {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "reason",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reason: Option<Option<String>>,
    #[serde(
        rename = "stopDelegatedTasks",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub stop_delegated_tasks: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CommandThreadCreateImportedNativeThreadRefStrength {
    #[serde(rename = "strong")]
    Strong,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadCreateImportedNativeThreadRef {
    #[serde(rename = "driver")]
    pub driver: crate::provider_instance::ProviderDriverKind,
    #[serde(rename = "nativeId")]
    pub native_id: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "strength")]
    pub strength: CommandThreadCreateImportedNativeThreadRefStrength,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadCreateImportedNativeThread {
    #[serde(rename = "ref")]
    pub r#ref: CommandThreadCreateImportedNativeThreadRef,
    #[serde(
        rename = "metadata",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub metadata: Option<Option<OrchestrationV2ProviderThreadNativeMetadata>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadCreate {
    #[serde(rename = "createdBy")]
    pub created_by: OrchestrationV2Actor,
    #[serde(rename = "creationSource")]
    pub creation_source: OrchestrationV2CreationSource,
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "projectId")]
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(rename = "title")]
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "modelSelection")]
    pub model_selection: crate::model_selection::ModelSelection,
    #[serde(rename = "runtimeMode")]
    pub runtime_mode: crate::provider_policy::RuntimeMode,
    #[serde(rename = "interactionMode")]
    pub interaction_mode: crate::provider_policy::ProviderInteractionMode,
    #[serde(rename = "branch", with = "crate::wire::nullable")]
    pub branch: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(rename = "worktreePath", with = "crate::wire::nullable")]
    pub worktree_path: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(
        rename = "linkOrigin",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub link_origin: Option<Option<OrchestrationV2LinkOrigin>>,
    #[serde(
        rename = "delegatedFrom",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub delegated_from: Option<Option<OrchestrationV2DelegatedFrom>>,
    #[serde(
        rename = "importedNativeThread",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub imported_native_thread: Option<Option<CommandThreadCreateImportedNativeThread>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadArchive {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadGuestMessageDismiss {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "guestMessageId")]
    pub guest_message_id: crate::base_schemas::GuestMessageId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadDelete {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "onlyIfUnused",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub only_if_unused: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadSettle {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "expectedLatestRunId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_latest_run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "settledAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub settled_at: Option<Option<crate::wire::WireDateTime>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadAutoSettle {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "snapshotAt")]
    pub snapshot_at: crate::wire::WireDateTime,
    #[serde(
        rename = "settledAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub settled_at: Option<Option<crate::wire::WireDateTime>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CommandThreadUnsettleReason {
    #[serde(rename = "user")]
    User,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadUnsettle {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "reason")]
    pub reason: CommandThreadUnsettleReason,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadSnooze {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "snoozedUntil")]
    pub snoozed_until: crate::base_schemas::IsoDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadAutoSettleSet {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadPin {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "orderKey",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub order_key: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadTagSet {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "tag")]
    pub tag: crate::thread_tags::ThreadTag,
    #[serde(rename = "tagged")]
    pub tagged: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadPinReorder {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "orderKey")]
    pub order_key: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadVisit {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "visitedAt")]
    pub visited_at: crate::base_schemas::IsoDateTime,
    #[serde(
        rename = "opened",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub opened: Option<Option<bool>>,
    #[serde(
        rename = "searchQuery",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub search_query: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadMetadataUpdate {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "title",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "regenerateTitle",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub regenerate_title: Option<Option<bool>>,
    #[serde(
        rename = "branch",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub branch: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "worktreePath",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub worktree_path: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "expectedWorktreePath",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub expected_worktree_path: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "expectedBranch",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub expected_branch: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "expectedEmpty",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_empty: Option<Option<bool>>,
    #[serde(
        rename = "limitRecovery",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub limit_recovery: Option<Option<OrchestrationV2LimitRecoveryUpdate>>,
    #[serde(
        rename = "linkedPullRequest",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub linked_pull_request: Option<Option<crate::thread_pull_request::ThreadLinkedPullRequest>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadPullRequestLink {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "host")]
    pub host: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "repository")]
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "number")]
    pub number: crate::base_schemas::PositiveInt,
    #[serde(rename = "url")]
    pub url: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "source")]
    pub source: crate::thread_pull_request::ThreadPullRequestLinkSource,
    #[serde(
        rename = "repositoryWebUrl",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub repository_web_url: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadPullRequestUnlink {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "host")]
    pub host: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "repository")]
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "number")]
    pub number: crate::base_schemas::PositiveInt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadPullRequestLinkSync {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "host")]
    pub host: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "repository")]
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "number")]
    pub number: crate::base_schemas::PositiveInt,
    #[serde(rename = "snapshot")]
    pub snapshot: crate::thread_pull_request::ThreadPullRequestSnapshot,
    #[serde(rename = "stack", with = "crate::wire::nullable")]
    pub stack: Option<crate::thread_pull_request::ThreadPullRequestStack>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadPullRequestWatchLink {
    #[serde(rename = "url")]
    pub url: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "source")]
    pub source: crate::thread_pull_request::ThreadPullRequestLinkSource,
    #[serde(
        rename = "repositoryWebUrl",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub repository_web_url: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadPullRequestWatch {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "host")]
    pub host: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "repository")]
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "number")]
    pub number: crate::base_schemas::PositiveInt,
    #[serde(rename = "watching")]
    pub watching: bool,
    #[serde(
        rename = "link",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub link: Option<Option<CommandThreadPullRequestWatchLink>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadPullRequestSyncExpected {
    #[serde(rename = "workspaceRoot")]
    pub workspace_root: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "branch", with = "crate::wire::nullable")]
    pub branch: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(rename = "worktreePath", with = "crate::wire::nullable")]
    pub worktree_path: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(rename = "linkedPullRequest", with = "crate::wire::nullable")]
    pub linked_pull_request: Option<crate::thread_pull_request::ThreadLinkedPullRequest>,
    #[serde(rename = "branchPullRequest", with = "crate::wire::nullable")]
    pub branch_pull_request: Option<crate::thread_pull_request::ThreadLinkedPullRequest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadPullRequestSync {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "projectId")]
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(rename = "snapshotSequence")]
    pub snapshot_sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "expected")]
    pub expected: CommandThreadPullRequestSyncExpected,
    #[serde(rename = "branchPullRequest", with = "crate::wire::nullable")]
    pub branch_pull_request: Option<crate::thread_pull_request::ThreadLinkedPullRequest>,
    #[serde(
        rename = "linkedPullRequest",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub linked_pull_request: Option<Option<crate::thread_pull_request::ThreadLinkedPullRequest>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadTitleRegenerationComplete {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "requestId")]
    pub request_id: crate::base_schemas::CommandId,
    #[serde(
        rename = "title",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "outcome",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub outcome: Option<Option<crate::thread_title::ThreadTitleRegenerationOutcome>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadRuntimeModeSet {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "runtimeMode")]
    pub runtime_mode: crate::provider_policy::RuntimeMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadInteractionModeSet {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "interactionMode")]
    pub interaction_mode: crate::provider_policy::ProviderInteractionMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadModelSelectionSet {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "modelSelection")]
    pub model_selection: crate::model_selection::ModelSelection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandProviderSessionDetach {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "providerSessionId")]
    pub provider_session_id: crate::base_schemas::ProviderSessionId,
    #[serde(
        rename = "reason",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reason: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CommandMessageDispatchDeliveryIntent {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "steer")]
    Steer,
    #[serde(rename = "restart")]
    Restart,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandMessageDispatchDispatchModeDeferStart {
    #[serde(
        rename = "workspaceStrategy",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub workspace_strategy: Option<Option<OrchestrationV2ThreadLaunchWorkspaceStrategy>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandMessageDispatchDispatchModeSteerActive {
    #[serde(rename = "targetRunId")]
    pub target_run_id: crate::base_schemas::RunId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CommandMessageDispatchDispatchMode {
    #[serde(rename = "defer_start")]
    DeferStart(Box<CommandMessageDispatchDispatchModeDeferStart>),
    #[serde(rename = "steer_active")]
    SteerActive(Box<CommandMessageDispatchDispatchModeSteerActive>),
    #[serde(rename = "restart_active")]
    RestartActive(Box<CommandMessageDispatchDispatchModeSteerActive>),
    #[serde(rename = "queue_after_active")]
    QueueAfterActive(Box<ThreadForkSourcePointLatestStable>),
    #[serde(rename = "start_immediately")]
    StartImmediately(Box<ThreadForkSourcePointLatestStable>),
}
impl crate::wire::TaggedUnion for CommandMessageDispatchDispatchMode {
    const TAG: &'static str = "type";
    const KNOWN_TAGS: &'static [&'static str] = &[
        "defer_start",
        "steer_active",
        "restart_active",
        "queue_after_active",
        "start_immediately",
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandMessageDispatch {
    #[serde(
        rename = "notification",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub notification: Option<Option<OrchestrationV2Notification>>,
    #[serde(
        rename = "guestMessageId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub guest_message_id: Option<Option<crate::base_schemas::GuestMessageId>>,
    #[serde(rename = "createdBy")]
    pub created_by: OrchestrationV2Actor,
    #[serde(rename = "creationSource")]
    pub creation_source: OrchestrationV2CreationSource,
    #[serde(
        rename = "scheduledTaskId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub scheduled_task_id: Option<Option<crate::base_schemas::ScheduledTaskId>>,
    #[serde(
        rename = "senderThreadId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub sender_thread_id: Option<Option<crate::base_schemas::ThreadId>>,
    #[serde(
        rename = "linkedSender",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub linked_sender: Option<Option<OrchestrationV2LinkedSender>>,
    #[serde(
        rename = "idleCompaction",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub idle_compaction: Option<Option<OrchestrationV2IdleCompaction>>,
    #[serde(
        rename = "freshContext",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub fresh_context: Option<Option<bool>>,
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "messageId")]
    pub message_id: crate::base_schemas::MessageId,
    #[serde(rename = "text")]
    pub text: String,
    #[serde(
        rename = "context",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub context: Option<Option<crate::composer_context::OrchestrationMessageContext>>,
    #[serde(rename = "attachments")]
    pub attachments: Vec<crate::chat_attachment::ChatAttachment>,
    #[serde(
        rename = "titleSeed",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title_seed: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "modelSelection",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub model_selection: Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        rename = "sourcePlanRef",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub source_plan_ref: Option<Option<RunSourcePlanRef>>,
    #[serde(
        rename = "restartContinuationOfRunId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub restart_continuation_of_run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "usageLimitContinuationOfRunId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub usage_limit_continuation_of_run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "manualContinuationOfRunId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub manual_continuation_of_run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "steerFollowUpOfRunId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub steer_follow_up_of_run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "steerFollowUpAfterRunOrdinal",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub steer_follow_up_after_run_ordinal: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "usageLimitRecoveryRequestId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub usage_limit_recovery_request_id: Option<Option<crate::base_schemas::CommandId>>,
    #[serde(
        rename = "deliveryIntent",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub delivery_intent: Option<Option<CommandMessageDispatchDeliveryIntent>>,
    #[serde(
        rename = "delegatedCompletion",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub delegated_completion: Option<Option<ConversationMessageDelegatedCompletion>>,
    #[serde(rename = "dispatchMode")]
    pub dispatch_mode: CommandMessageDispatchDispatchMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandPreparedRunRelease {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "runId")]
    pub run_id: crate::base_schemas::RunId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandNotificationDeliveryAccept {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "messageId")]
    pub message_id: crate::base_schemas::MessageId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CommandPreparedRunProgressPhase {
    #[serde(rename = "worktree")]
    Worktree,
    #[serde(rename = "setup")]
    Setup,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandPreparedRunProgress {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "runId")]
    pub run_id: crate::base_schemas::RunId,
    #[serde(rename = "phase")]
    pub phase: CommandPreparedRunProgressPhase,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandPreparedRunFail {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "runId")]
    pub run_id: crate::base_schemas::RunId,
    #[serde(rename = "failure")]
    pub failure: OrchestrationV2ProviderFailure,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandSubagentStop {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "runId")]
    pub run_id: crate::base_schemas::RunId,
    #[serde(rename = "subagentId")]
    pub subagent_id: crate::base_schemas::NodeId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandRunInterrupt {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "runId")]
    pub run_id: crate::base_schemas::RunId,
    #[serde(
        rename = "reason",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reason: Option<Option<String>>,
    #[serde(
        rename = "holdQueue",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub hold_queue: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandQueuedMessagePromoteToSteer {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "queuedRunId")]
    pub queued_run_id: crate::base_schemas::RunId,
    #[serde(rename = "targetRunId")]
    pub target_run_id: crate::base_schemas::RunId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandQueuedRunReorder {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "runId")]
    pub run_id: crate::base_schemas::RunId,
    #[serde(rename = "beforeRunId", with = "crate::wire::nullable")]
    pub before_run_id: Option<crate::base_schemas::RunId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandQueuedRunEdit {
    #[serde(
        rename = "context",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub context: Option<Option<crate::composer_context::OrchestrationMessageContext>>,
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "runId")]
    pub run_id: crate::base_schemas::RunId,
    #[serde(rename = "text")]
    pub text: String,
    #[serde(
        rename = "attachments",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub attachments: Option<Option<Vec<crate::chat_attachment::ChatAttachment>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandRuntimeRequestRespond {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "requestId")]
    pub request_id: crate::base_schemas::RuntimeRequestId,
    #[serde(
        rename = "decision",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub decision: Option<Option<crate::provider_policy::ProviderApprovalDecision>>,
    #[serde(
        rename = "feedback",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub feedback: Option<Option<String>>,
    #[serde(
        rename = "answers",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub answers: Option<Option<crate::provider_policy::ProviderUserInputAnswers>>,
    #[serde(
        rename = "attachmentsByQuestionId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub attachments_by_question_id: Option<Option<crate::provider_policy::UserInputAttachments>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadUserInputDismiss {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "requestId")]
    pub request_id: crate::base_schemas::RuntimeRequestId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandCheckpointRollback {
    #[serde(
        rename = "restoreFiles",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub restore_files: Option<Option<bool>>,
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "scopeId")]
    pub scope_id: crate::base_schemas::CheckpointScopeId,
    #[serde(rename = "checkpointId")]
    pub checkpoint_id: crate::base_schemas::CheckpointId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadFork {
    #[serde(rename = "createdBy")]
    pub created_by: OrchestrationV2Actor,
    #[serde(rename = "creationSource")]
    pub creation_source: OrchestrationV2CreationSource,
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "sourceThreadId")]
    pub source_thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "targetThreadId")]
    pub target_thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "sourcePoint")]
    pub source_point: OrchestrationV2ThreadForkSourcePoint,
    #[serde(
        rename = "title",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "sideChat",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub side_chat: Option<Option<bool>>,
    #[serde(
        rename = "createdAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub created_at: Option<Option<crate::wire::WireDateTime>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadMergeBack {
    #[serde(rename = "createdBy")]
    pub created_by: OrchestrationV2Actor,
    #[serde(rename = "creationSource")]
    pub creation_source: OrchestrationV2CreationSource,
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "sourceThreadId")]
    pub source_thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "targetThreadId")]
    pub target_thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "sourcePoint")]
    pub source_point: OrchestrationV2ThreadForkSourcePoint,
    #[serde(
        rename = "createdAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub created_at: Option<Option<crate::wire::WireDateTime>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandDelegatedTaskRequest {
    #[serde(rename = "createdBy")]
    pub created_by: OrchestrationV2Actor,
    #[serde(rename = "creationSource")]
    pub creation_source: OrchestrationV2CreationSource,
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "parentThreadId")]
    pub parent_thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "parentRunId")]
    pub parent_run_id: crate::base_schemas::RunId,
    #[serde(rename = "parentNodeId")]
    pub parent_node_id: crate::base_schemas::NodeId,
    #[serde(rename = "task")]
    pub task: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "title",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(rename = "modelSelection")]
    pub model_selection: crate::model_selection::ModelSelection,
    #[serde(rename = "runtimeMode")]
    pub runtime_mode: crate::provider_policy::RuntimeMode,
    #[serde(rename = "interactionMode")]
    pub interaction_mode: crate::provider_policy::ProviderInteractionMode,
    #[serde(
        rename = "workspaceStrategy",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub workspace_strategy: Option<Option<OrchestrationV2ThreadLaunchWorkspaceStrategy>>,
    #[serde(
        rename = "completionWake",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub completion_wake: Option<Option<SubagentCompletionWake>>,
    #[serde(
        rename = "createdAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub created_at: Option<Option<crate::wire::WireDateTime>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandDelegatedTaskWakePolicy {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "parentThreadId")]
    pub parent_thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "taskId")]
    pub task_id: crate::base_schemas::NodeId,
    #[serde(rename = "completionWake")]
    pub completion_wake: SubagentCompletionWake,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandDelegatedTaskCompletionDeliveryAcknowledge {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "parentThreadId")]
    pub parent_thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "taskId")]
    pub task_id: crate::base_schemas::NodeId,
    #[serde(rename = "observedByRunId", with = "crate::wire::nullable")]
    pub observed_by_run_id: Option<crate::base_schemas::RunId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandDelegatedTaskCompletionDeliveryDispose {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "parentThreadId")]
    pub parent_thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "taskId")]
    pub task_id: crate::base_schemas::NodeId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandThreadCreatedRecord {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "parentThreadId")]
    pub parent_thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "parentRunId")]
    pub parent_run_id: crate::base_schemas::RunId,
    #[serde(rename = "parentNodeId")]
    pub parent_node_id: crate::base_schemas::NodeId,
    #[serde(rename = "targetThreadId")]
    pub target_thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "targetRunId", with = "crate::wire::nullable")]
    pub target_run_id: Option<crate::base_schemas::RunId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OrchestrationV2Command {
    #[serde(rename = "thread.stop")]
    ThreadStop(Box<CommandThreadStop>),
    #[serde(rename = "thread.create")]
    ThreadCreate(Box<CommandThreadCreate>),
    #[serde(rename = "thread.archive")]
    ThreadArchive(Box<CommandThreadArchive>),
    #[serde(rename = "thread.guest-share.close")]
    ThreadGuestShareClose(Box<CommandThreadArchive>),
    #[serde(rename = "thread.guest-message.dismiss")]
    ThreadGuestMessageDismiss(Box<CommandThreadGuestMessageDismiss>),
    #[serde(rename = "thread.unarchive")]
    ThreadUnarchive(Box<CommandThreadArchive>),
    #[serde(rename = "thread.delete")]
    ThreadDelete(Box<CommandThreadDelete>),
    #[serde(rename = "thread.settle")]
    ThreadSettle(Box<CommandThreadSettle>),
    #[serde(rename = "thread.auto-settle")]
    ThreadAutoSettle(Box<CommandThreadAutoSettle>),
    #[serde(rename = "thread.unsettle")]
    ThreadUnsettle(Box<CommandThreadUnsettle>),
    #[serde(rename = "thread.snooze")]
    ThreadSnooze(Box<CommandThreadSnooze>),
    #[serde(rename = "thread.unsnooze")]
    ThreadUnsnooze(Box<CommandThreadUnsettle>),
    #[serde(rename = "thread.auto-settle.set")]
    ThreadAutoSettleSet(Box<CommandThreadAutoSettleSet>),
    #[serde(rename = "thread.pin")]
    ThreadPin(Box<CommandThreadPin>),
    #[serde(rename = "thread.unpin")]
    ThreadUnpin(Box<CommandThreadArchive>),
    #[serde(rename = "thread.tag.set")]
    ThreadTagSet(Box<CommandThreadTagSet>),
    #[serde(rename = "thread.pin.reorder")]
    ThreadPinReorder(Box<CommandThreadPinReorder>),
    #[serde(rename = "thread.active.reorder")]
    ThreadActiveReorder(Box<CommandThreadPinReorder>),
    #[serde(rename = "thread.visit")]
    ThreadVisit(Box<CommandThreadVisit>),
    #[serde(rename = "thread.mark-unread")]
    ThreadMarkUnread(Box<CommandThreadArchive>),
    #[serde(rename = "thread.metadata.update")]
    ThreadMetadataUpdate(Box<CommandThreadMetadataUpdate>),
    #[serde(rename = "thread.pull-request.link")]
    ThreadPullRequestLink(Box<CommandThreadPullRequestLink>),
    #[serde(rename = "thread.pull-request.unlink")]
    ThreadPullRequestUnlink(Box<CommandThreadPullRequestUnlink>),
    #[serde(rename = "thread.pull-request-link.sync")]
    ThreadPullRequestLinkSync(Box<CommandThreadPullRequestLinkSync>),
    #[serde(rename = "thread.pull-request.watch")]
    ThreadPullRequestWatch(Box<CommandThreadPullRequestWatch>),
    #[serde(rename = "thread.pull-request.sync")]
    ThreadPullRequestSync(Box<CommandThreadPullRequestSync>),
    #[serde(rename = "thread.title.regeneration.complete")]
    ThreadTitleRegenerationComplete(Box<CommandThreadTitleRegenerationComplete>),
    #[serde(rename = "thread.runtime-mode.set")]
    ThreadRuntimeModeSet(Box<CommandThreadRuntimeModeSet>),
    #[serde(rename = "thread.interaction-mode.set")]
    ThreadInteractionModeSet(Box<CommandThreadInteractionModeSet>),
    #[serde(rename = "thread.model-selection.set")]
    ThreadModelSelectionSet(Box<CommandThreadModelSelectionSet>),
    #[serde(rename = "provider-session.detach")]
    ProviderSessionDetach(Box<CommandProviderSessionDetach>),
    #[serde(rename = "message.dispatch")]
    MessageDispatch(Box<CommandMessageDispatch>),
    #[serde(rename = "prepared-run.release")]
    PreparedRunRelease(Box<CommandPreparedRunRelease>),
    #[serde(rename = "notification.delivery.accept")]
    NotificationDeliveryAccept(Box<CommandNotificationDeliveryAccept>),
    #[serde(rename = "prepared-run.progress")]
    PreparedRunProgress(Box<CommandPreparedRunProgress>),
    #[serde(rename = "prepared-run.fail")]
    PreparedRunFail(Box<CommandPreparedRunFail>),
    #[serde(rename = "prepared-run.retry")]
    PreparedRunRetry(Box<CommandPreparedRunRelease>),
    #[serde(rename = "subagent.stop")]
    SubagentStop(Box<CommandSubagentStop>),
    #[serde(rename = "run.interrupt")]
    RunInterrupt(Box<CommandRunInterrupt>),
    #[serde(rename = "queued-message.promote-to-steer")]
    QueuedMessagePromoteToSteer(Box<CommandQueuedMessagePromoteToSteer>),
    #[serde(rename = "queue.resume")]
    QueueResume(Box<CommandThreadArchive>),
    #[serde(rename = "queued-run.start")]
    QueuedRunStart(Box<CommandPreparedRunRelease>),
    #[serde(rename = "queued-run.reorder")]
    QueuedRunReorder(Box<CommandQueuedRunReorder>),
    #[serde(rename = "queued-run.cancel")]
    QueuedRunCancel(Box<CommandPreparedRunRelease>),
    #[serde(rename = "queued-run.edit")]
    QueuedRunEdit(Box<CommandQueuedRunEdit>),
    #[serde(rename = "runtime-request.respond")]
    RuntimeRequestRespond(Box<CommandRuntimeRequestRespond>),
    #[serde(rename = "thread.user-input.dismiss")]
    ThreadUserInputDismiss(Box<CommandThreadUserInputDismiss>),
    #[serde(rename = "checkpoint.rollback")]
    CheckpointRollback(Box<CommandCheckpointRollback>),
    #[serde(rename = "thread.fork")]
    ThreadFork(Box<CommandThreadFork>),
    #[serde(rename = "thread.merge_back")]
    ThreadMergeBack(Box<CommandThreadMergeBack>),
    #[serde(rename = "delegated_task.request")]
    DelegatedTaskRequest(Box<CommandDelegatedTaskRequest>),
    #[serde(rename = "delegated_task.wake-policy")]
    DelegatedTaskWakePolicy(Box<CommandDelegatedTaskWakePolicy>),
    #[serde(rename = "delegated_task.completion-delivery.acknowledge")]
    DelegatedTaskCompletionDeliveryAcknowledge(
        Box<CommandDelegatedTaskCompletionDeliveryAcknowledge>,
    ),
    #[serde(rename = "delegated_task.completion-delivery.dispose")]
    DelegatedTaskCompletionDeliveryDispose(Box<CommandDelegatedTaskCompletionDeliveryDispose>),
    #[serde(rename = "thread.created.record")]
    ThreadCreatedRecord(Box<CommandThreadCreatedRecord>),
    #[serde(rename = "provider.switch")]
    ProviderSwitch(Box<CommandThreadModelSelectionSet>),
}
impl crate::wire::TaggedUnion for OrchestrationV2Command {
    const TAG: &'static str = "type";
    const KNOWN_TAGS: &'static [&'static str] = &[
        "thread.stop",
        "thread.create",
        "thread.archive",
        "thread.guest-share.close",
        "thread.guest-message.dismiss",
        "thread.unarchive",
        "thread.delete",
        "thread.settle",
        "thread.auto-settle",
        "thread.unsettle",
        "thread.snooze",
        "thread.unsnooze",
        "thread.auto-settle.set",
        "thread.pin",
        "thread.unpin",
        "thread.tag.set",
        "thread.pin.reorder",
        "thread.active.reorder",
        "thread.visit",
        "thread.mark-unread",
        "thread.metadata.update",
        "thread.pull-request.link",
        "thread.pull-request.unlink",
        "thread.pull-request-link.sync",
        "thread.pull-request.watch",
        "thread.pull-request.sync",
        "thread.title.regeneration.complete",
        "thread.runtime-mode.set",
        "thread.interaction-mode.set",
        "thread.model-selection.set",
        "provider-session.detach",
        "message.dispatch",
        "prepared-run.release",
        "notification.delivery.accept",
        "prepared-run.progress",
        "prepared-run.fail",
        "prepared-run.retry",
        "subagent.stop",
        "run.interrupt",
        "queued-message.promote-to-steer",
        "queue.resume",
        "queued-run.start",
        "queued-run.reorder",
        "queued-run.cancel",
        "queued-run.edit",
        "runtime-request.respond",
        "thread.user-input.dismiss",
        "checkpoint.rollback",
        "thread.fork",
        "thread.merge_back",
        "delegated_task.request",
        "delegated_task.wake-policy",
        "delegated_task.completion-delivery.acknowledge",
        "delegated_task.completion-delivery.dispose",
        "thread.created.record",
        "provider.switch",
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalCommandThreadGuestShareOpen {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "shareId")]
    pub share_id: crate::base_schemas::GuestShareId,
    #[serde(rename = "host")]
    pub host: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "expiresAt")]
    pub expires_at: crate::base_schemas::IsoDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalCommandThreadGuestShareRedeem {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "shareId")]
    pub share_id: crate::base_schemas::GuestShareId,
    #[serde(rename = "guestName")]
    pub guest_name: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalCommandThreadGuestMessageSubmit {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "shareId")]
    pub share_id: crate::base_schemas::GuestShareId,
    #[serde(rename = "guestMessageId")]
    pub guest_message_id: crate::base_schemas::GuestMessageId,
    #[serde(rename = "text")]
    pub text: String,
    #[serde(
        rename = "attachments",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub attachments: Option<Option<Vec<crate::chat_attachment::ChatAttachment>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalCommandThreadProjectAdoptFolder {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "projectId")]
    pub project_id: crate::base_schemas::ProjectId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalCommandThreadPullRequestWatchSyncOthersItem {
    #[serde(rename = "host")]
    pub host: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "repository")]
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "number")]
    pub number: crate::base_schemas::PositiveInt,
    #[serde(rename = "startedAt")]
    pub started_at: crate::base_schemas::IsoDateTime,
    #[serde(rename = "watch", with = "crate::wire::nullable")]
    pub watch: Option<crate::thread_pull_request::ThreadPullRequestWatch>,
    #[serde(
        rename = "checksPendingBefore",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub checks_pending_before: Option<Option<bool>>,
    #[serde(
        rename = "endedWatch",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub ended_watch: Option<Option<crate::thread_pull_request::ThreadPullRequestWatch>>,
    #[serde(
        rename = "includesRemarks",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub includes_remarks: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalCommandThreadPullRequestWatchSyncWake {
    #[serde(rename = "messageId")]
    pub message_id: crate::base_schemas::MessageId,
    #[serde(rename = "text")]
    pub text: String,
    #[serde(rename = "notification")]
    pub notification: OrchestrationV2Notification,
    #[serde(
        rename = "includesRemarks",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub includes_remarks: Option<Option<bool>>,
    #[serde(
        rename = "sections",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub sections: Option<Option<Vec<PullRequestWakeSection>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalCommandThreadPullRequestWatchSync {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "host")]
    pub host: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "repository")]
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "number")]
    pub number: crate::base_schemas::PositiveInt,
    #[serde(rename = "startedAt")]
    pub started_at: crate::base_schemas::IsoDateTime,
    #[serde(rename = "watch", with = "crate::wire::nullable")]
    pub watch: Option<crate::thread_pull_request::ThreadPullRequestWatch>,
    #[serde(
        rename = "checksPendingBefore",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub checks_pending_before: Option<Option<bool>>,
    #[serde(
        rename = "endedWatch",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub ended_watch: Option<Option<crate::thread_pull_request::ThreadPullRequestWatch>>,
    #[serde(
        rename = "others",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub others: Option<Option<Vec<InternalCommandThreadPullRequestWatchSyncOthersItem>>>,
    #[serde(
        rename = "wake",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub wake: Option<Option<InternalCommandThreadPullRequestWatchSyncWake>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalCommandDelegatedTaskRemoteRequest {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "parentThreadId")]
    pub parent_thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "parentRunId")]
    pub parent_run_id: crate::base_schemas::RunId,
    #[serde(rename = "parentNodeId")]
    pub parent_node_id: crate::base_schemas::NodeId,
    #[serde(rename = "task")]
    pub task: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "title",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(rename = "driver")]
    pub driver: crate::provider_instance::ProviderDriverKind,
    #[serde(rename = "modelSelection")]
    pub model_selection: crate::model_selection::ModelSelection,
    #[serde(rename = "remoteChild")]
    pub remote_child: OrchestrationV2RemoteTaskChild,
    #[serde(
        rename = "completionWake",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub completion_wake: Option<Option<SubagentCompletionWake>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InternalCommandDelegatedTaskRemoteCompleteStatus {
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "interrupted")]
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InternalCommandDelegatedTaskRemoteCompleteStoppedThere {
    #[serde(rename = "ran-out")]
    RanOut,
    #[serde(rename = "stopped")]
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalCommandDelegatedTaskRemoteComplete {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "parentThreadId")]
    pub parent_thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "taskId")]
    pub task_id: crate::base_schemas::NodeId,
    #[serde(rename = "status")]
    pub status: InternalCommandDelegatedTaskRemoteCompleteStatus,
    #[serde(rename = "result")]
    pub result: String,
    #[serde(
        rename = "stoppedThere",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub stopped_there: Option<Option<InternalCommandDelegatedTaskRemoteCompleteStoppedThere>>,
    #[serde(
        rename = "stopCount",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub stop_count: Option<Option<crate::base_schemas::NonNegativeInt>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalCommandCheckpointRollbackFail {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "requestId")]
    pub request_id: crate::base_schemas::CommandId,
    #[serde(rename = "message")]
    pub message: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalCommandThreadInterruptUnresponsive {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "providerThreadId")]
    pub provider_thread_id: crate::base_schemas::ProviderThreadId,
    #[serde(rename = "providerTurnId")]
    pub provider_turn_id: crate::base_schemas::ProviderTurnId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InternalCommandThreadBackgroundWorkSettleSubagentOutcomeStatus {
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalCommandThreadBackgroundWorkSettleSubagentOutcome {
    #[serde(rename = "status")]
    pub status: InternalCommandThreadBackgroundWorkSettleSubagentOutcomeStatus,
    #[serde(rename = "completedAt")]
    pub completed_at: crate::wire::WireDateTime,
    #[serde(rename = "result", with = "crate::wire::nullable")]
    pub result: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalCommandThreadBackgroundWorkSettleSubagent {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::NodeId,
    #[serde(rename = "stoppedAt")]
    pub stopped_at: crate::wire::WireDateTime,
    #[serde(rename = "outcome")]
    pub outcome: InternalCommandThreadBackgroundWorkSettleSubagentOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalCommandThreadBackgroundWorkSettle {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "providerThreadId")]
    pub provider_thread_id: crate::base_schemas::ProviderThreadId,
    #[serde(rename = "providerTurnId")]
    pub provider_turn_id: crate::base_schemas::ProviderTurnId,
    #[serde(
        rename = "reason",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reason: Option<Option<String>>,
    #[serde(
        rename = "subagent",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub subagent: Option<Option<InternalCommandThreadBackgroundWorkSettleSubagent>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalCommandSecretRequestRecord {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "runId")]
    pub run_id: crate::base_schemas::RunId,
    #[serde(rename = "nodeId")]
    pub node_id: crate::base_schemas::NodeId,
    #[serde(rename = "turnItemId")]
    pub turn_item_id: crate::base_schemas::TurnItemId,
    #[serde(rename = "label")]
    pub label: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "reason")]
    pub reason: String,
    #[serde(
        rename = "url",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub url: Option<Option<String>>,
    #[serde(
        rename = "placeholder",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub placeholder: Option<Option<String>>,
    #[serde(
        rename = "delivery",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub delivery: Option<Option<OrchestrationV2SecretRequestDelivery>>,
    #[serde(rename = "secretStatus")]
    pub secret_status: OrchestrationV2SecretRequestStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalCommandLinkRequestRecordWake {
    #[serde(rename = "messageId")]
    pub message_id: crate::base_schemas::MessageId,
    #[serde(rename = "text")]
    pub text: String,
    #[serde(rename = "notification")]
    pub notification: OrchestrationV2Notification,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalCommandLinkRequestRecord {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "runId")]
    pub run_id: crate::base_schemas::RunId,
    #[serde(rename = "nodeId")]
    pub node_id: crate::base_schemas::NodeId,
    #[serde(rename = "turnItemId")]
    pub turn_item_id: crate::base_schemas::TurnItemId,
    #[serde(
        rename = "url",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub url: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(rename = "reason")]
    pub reason: String,
    #[serde(
        rename = "environmentId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub environment_id: Option<Option<crate::base_schemas::EnvironmentId>>,
    #[serde(
        rename = "label",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub label: Option<Option<String>>,
    #[serde(
        rename = "hint",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub hint: Option<Option<String>>,
    #[serde(
        rename = "requestedAccess",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub requested_access: Option<Option<crate::auth::AuthMcpClientAccess>>,
    #[serde(rename = "linkStatus")]
    pub link_status: OrchestrationV2LinkRequestStatus,
    #[serde(
        rename = "linkedEnvironmentId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub linked_environment_id: Option<Option<crate::base_schemas::EnvironmentId>>,
    #[serde(
        rename = "linkedLabel",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub linked_label: Option<Option<String>>,
    #[serde(
        rename = "linkedAccess",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub linked_access: Option<Option<crate::auth::AuthMcpClientAccess>>,
    #[serde(
        rename = "failure",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub failure: Option<Option<String>>,
    #[serde(
        rename = "wake",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub wake: Option<Option<InternalCommandLinkRequestRecordWake>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OrchestrationV2InternalCommand {
    #[serde(rename = "thread.guest-share.open")]
    ThreadGuestShareOpen(Box<InternalCommandThreadGuestShareOpen>),
    #[serde(rename = "thread.guest-share.redeem")]
    ThreadGuestShareRedeem(Box<InternalCommandThreadGuestShareRedeem>),
    #[serde(rename = "thread.guest-message.submit")]
    ThreadGuestMessageSubmit(Box<InternalCommandThreadGuestMessageSubmit>),
    #[serde(rename = "thread.project.adopt-folder")]
    ThreadProjectAdoptFolder(Box<InternalCommandThreadProjectAdoptFolder>),
    #[serde(rename = "thread.pull-request-watch.checks-reopen")]
    ThreadPullRequestWatchChecksReopen(Box<CommandThreadArchive>),
    #[serde(rename = "thread.pull-request-watch.sync")]
    ThreadPullRequestWatchSync(Box<InternalCommandThreadPullRequestWatchSync>),
    #[serde(rename = "delegated_task.remote.request")]
    DelegatedTaskRemoteRequest(Box<InternalCommandDelegatedTaskRemoteRequest>),
    #[serde(rename = "delegated_task.remote.complete")]
    DelegatedTaskRemoteComplete(Box<InternalCommandDelegatedTaskRemoteComplete>),
    #[serde(rename = "delegated_task.remote.stop")]
    DelegatedTaskRemoteStop(Box<CommandDelegatedTaskCompletionDeliveryDispose>),
    #[serde(rename = "checkpoint.rollback.fail")]
    CheckpointRollbackFail(Box<InternalCommandCheckpointRollbackFail>),
    #[serde(rename = "runtime-request.delivery.fail")]
    RuntimeRequestDeliveryFail(Box<CommandThreadUserInputDismiss>),
    #[serde(rename = "thread.interrupt.unresponsive")]
    ThreadInterruptUnresponsive(Box<InternalCommandThreadInterruptUnresponsive>),
    #[serde(rename = "thread.background-work.settle")]
    ThreadBackgroundWorkSettle(Box<InternalCommandThreadBackgroundWorkSettle>),
    #[serde(rename = "secret_request.record")]
    SecretRequestRecord(Box<InternalCommandSecretRequestRecord>),
    #[serde(rename = "link_request.record")]
    LinkRequestRecord(Box<InternalCommandLinkRequestRecord>),
}
impl crate::wire::TaggedUnion for OrchestrationV2InternalCommand {
    const TAG: &'static str = "type";
    const KNOWN_TAGS: &'static [&'static str] = &[
        "thread.guest-share.open",
        "thread.guest-share.redeem",
        "thread.guest-message.submit",
        "thread.project.adopt-folder",
        "thread.pull-request-watch.checks-reopen",
        "thread.pull-request-watch.sync",
        "delegated_task.remote.request",
        "delegated_task.remote.complete",
        "delegated_task.remote.stop",
        "checkpoint.rollback.fail",
        "runtime-request.delivery.fail",
        "thread.interrupt.unresponsive",
        "thread.background-work.settle",
        "secret_request.record",
        "link_request.record",
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2DispatchCommandResult {
    #[serde(rename = "sequence")]
    pub sequence: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DispatchCommandErrorTag {
    #[serde(rename = "OrchestrationV2DispatchCommandError")]
    OrchestrationV2DispatchCommandError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2DispatchCommandError {
    #[serde(rename = "_tag")]
    pub _tag: DispatchCommandErrorTag,
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "commandType")]
    pub command_type: String,
    #[serde(rename = "message")]
    pub message: String,
    #[serde(
        rename = "detail",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub detail: Option<Option<String>>,
    #[serde(
        rename = "cause",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}
