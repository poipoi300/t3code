use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ThreadProjection {
    #[serde(rename = "thread")]
    pub thread: OrchestrationV2AppThread,
    #[serde(rename = "runs")]
    pub runs: Vec<OrchestrationV2Run>,
    #[serde(rename = "attempts")]
    pub attempts: Vec<OrchestrationV2RunAttempt>,
    #[serde(rename = "nodes")]
    pub nodes: Vec<OrchestrationV2ExecutionNode>,
    #[serde(rename = "subagents")]
    pub subagents: Vec<OrchestrationV2Subagent>,
    #[serde(rename = "providerSessions")]
    pub provider_sessions: Vec<OrchestrationV2ProviderSession>,
    #[serde(rename = "providerThreads")]
    pub provider_threads: Vec<OrchestrationV2ProviderThread>,
    #[serde(rename = "providerTurns")]
    pub provider_turns: Vec<OrchestrationV2ProviderTurn>,
    #[serde(rename = "runtimeRequests")]
    pub runtime_requests: Vec<OrchestrationV2RuntimeRequest>,
    #[serde(rename = "messages")]
    pub messages: Vec<OrchestrationV2ConversationMessage>,
    #[serde(rename = "plans")]
    pub plans: Vec<OrchestrationV2PlanArtifact>,
    #[serde(
        rename = "turnItems",
        deserialize_with = "crate::wire::deserialize_union_array"
    )]
    pub turn_items: Vec<OrchestrationV2TurnItem>,
    #[serde(rename = "checkpointScopes")]
    pub checkpoint_scopes: Vec<OrchestrationV2CheckpointScope>,
    #[serde(rename = "checkpoints")]
    pub checkpoints: Vec<OrchestrationV2Checkpoint>,
    #[serde(rename = "contextHandoffs")]
    pub context_handoffs: Vec<OrchestrationV2ContextHandoff>,
    #[serde(rename = "contextTransfers")]
    pub context_transfers: Vec<OrchestrationV2ContextTransfer>,
    #[serde(
        rename = "visibleTurnItems",
        deserialize_with = "deserialize_projected_items"
    )]
    pub visible_turn_items: Vec<OrchestrationV2ProjectedTurnItem>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ThreadTranscript {
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "title")]
    pub title: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "items")]
    pub items: Vec<OrchestrationV2ProjectedTurnItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationV2ShellThreadStatus {
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "preparing")]
    Preparing,
    #[serde(rename = "queued")]
    Queued,
    #[serde(rename = "starting")]
    Starting,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "interrupted")]
    Interrupted,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "rolled_back")]
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ThreadShellActivityRunStatus {
    #[serde(rename = "preparing")]
    Preparing,
    #[serde(rename = "starting")]
    Starting,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "waiting")]
    Waiting,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ThreadShell {
    #[serde(rename = "createdBy")]
    pub created_by: OrchestrationV2Actor,
    #[serde(rename = "creationSource")]
    pub creation_source: OrchestrationV2CreationSource,
    #[serde(rename = "id")]
    pub id: crate::base_schemas::ThreadId,
    #[serde(rename = "projectId")]
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(rename = "title")]
    pub title: String,
    #[serde(rename = "providerInstanceId")]
    pub provider_instance_id: crate::provider_instance::ProviderInstanceId,
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
        rename = "linkedPullRequest",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub linked_pull_request: Option<Option<crate::thread_pull_request::ThreadLinkedPullRequest>>,
    #[serde(
        rename = "pullRequests",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub pull_requests: Option<Option<Vec<crate::thread_pull_request::ThreadPullRequestLink>>>,
    #[serde(
        rename = "branchPullRequest",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub branch_pull_request: Option<Option<crate::thread_pull_request::ThreadLinkedPullRequest>>,
    #[serde(rename = "lineage")]
    pub lineage: OrchestrationV2AppThreadLineage,
    #[serde(rename = "forkedFrom", with = "crate::wire::nullable")]
    pub forked_from: Option<Option<AppThreadForkedFrom>>,
    #[serde(rename = "activeProviderThreadId", with = "crate::wire::nullable")]
    pub active_provider_thread_id: Option<crate::base_schemas::ProviderThreadId>,
    #[serde(
        rename = "historyOrigin",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub history_origin: Option<Option<OrchestrationV2ThreadHistoryOrigin>>,
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
    #[serde(rename = "latestRunId", with = "crate::wire::nullable")]
    pub latest_run_id: Option<crate::base_schemas::RunId>,
    #[serde(
        rename = "latestRunRequestedAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub latest_run_requested_at: Option<Option<crate::wire::WireDateTime>>,
    #[serde(
        rename = "latestRunStartedAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub latest_run_started_at: Option<Option<crate::wire::WireDateTime>>,
    #[serde(
        rename = "latestRunCompletedAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub latest_run_completed_at: Option<Option<crate::wire::WireDateTime>>,
    #[serde(
        rename = "latestRunStopRequested",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub latest_run_stop_requested: Option<Option<bool>>,
    #[serde(
        rename = "latestRunBackgroundWake",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub latest_run_background_wake: Option<Option<bool>>,
    #[serde(rename = "activeRunId", with = "crate::wire::nullable")]
    pub active_run_id: Option<crate::base_schemas::RunId>,
    #[serde(
        rename = "activityRunStartedAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub activity_run_started_at: Option<Option<crate::wire::WireDateTime>>,
    #[serde(
        rename = "activityRunStatus",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub activity_run_status: Option<Option<ThreadShellActivityRunStatus>>,
    #[serde(
        rename = "blockedOnThreadWait",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub blocked_on_thread_wait: Option<Option<bool>>,
    #[serde(rename = "status")]
    pub status: OrchestrationV2ShellThreadStatus,
    #[serde(
        rename = "lastError",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub last_error: Option<Option<String>>,
    #[serde(
        rename = "lastErrorClass",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub last_error_class: Option<Option<OrchestrationV2ProviderFailureClass>>,
    #[serde(
        rename = "lastErrorCode",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub last_error_code: Option<Option<String>>,
    #[serde(
        rename = "usageLimitResetAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub usage_limit_reset_at: Option<Option<crate::base_schemas::IsoDateTime>>,
    #[serde(rename = "pendingRuntimeRequest", with = "crate::wire::nullable")]
    pub pending_runtime_request: Option<OrchestrationV2PendingRuntimeRequestSummary>,
    #[serde(
        rename = "pendingGuestMessageCount",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub pending_guest_message_count: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(rename = "latestVisibleMessage", with = "crate::wire::nullable")]
    pub latest_visible_message: Option<OrchestrationV2LatestVisibleMessageSummary>,
    #[serde(rename = "latestUserMessageAt", with = "crate::wire::nullable")]
    pub latest_user_message_at: Option<crate::wire::WireDateTime>,
    #[serde(
        rename = "latestUserAuthoredMessageAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub latest_user_authored_message_at: Option<Option<crate::wire::WireDateTime>>,
    #[serde(rename = "hasActionableProposedPlan")]
    pub has_actionable_proposed_plan: bool,
    #[serde(
        rename = "pendingBackgroundTasks",
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option",
        default
    )]
    pub pending_background_tasks: Option<Option<Vec<OrchestrationV2PendingBackgroundTask>>>,
    #[serde(
        rename = "providerInstanceHistory",
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option",
        default
    )]
    pub provider_instance_history:
        Option<Option<Vec<crate::provider_instance::ProviderInstanceId>>>,
    #[serde(
        rename = "goal",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub goal: Option<Option<OrchestrationV2ProviderGoal>>,
    #[serde(rename = "itemCount")]
    pub item_count: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "visibleItemCount")]
    pub visible_item_count: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "createdAt")]
    pub created_at: crate::wire::WireDateTime,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "archivedAt", with = "crate::wire::nullable")]
    pub archived_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "settledOverride", with = "crate::wire::nullable")]
    pub settled_override: Option<AppThreadSettledOverride>,
    #[serde(rename = "settledAt", with = "crate::wire::nullable")]
    pub settled_at: Option<crate::wire::WireDateTime>,
    #[serde(
        rename = "unsettledAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub unsettled_at: Option<Option<crate::wire::WireDateTime>>,
    #[serde(
        rename = "snoozedUntil",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub snoozed_until: Option<Option<crate::wire::WireDateTime>>,
    #[serde(
        rename = "snoozedAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub snoozed_at: Option<Option<crate::wire::WireDateTime>>,
    #[serde(
        rename = "limitRecovery",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub limit_recovery: Option<Option<OrchestrationV2LimitRecovery>>,
    #[serde(
        rename = "pinnedAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub pinned_at: Option<Option<crate::wire::WireDateTime>>,
    #[serde(
        rename = "autoSettleDisabledAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub auto_settle_disabled_at: Option<Option<crate::wire::WireDateTime>>,
    #[serde(
        rename = "pinOrderKey",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub pin_order_key: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "activeOrderKey",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub active_order_key: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "tags",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tags: Option<Option<crate::thread_tags::StoredThreadTags>>,
    #[serde(
        rename = "lastVisitedAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub last_visited_at: Option<Option<crate::wire::WireDateTime>>,
    #[serde(
        rename = "opens",
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_lenient_opens"
    )]
    pub opens: Option<Option<OrchestrationV2ThreadOpens>>,
    #[serde(
        rename = "titleRegeneration",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub title_regeneration: Option<Option<AppThreadTitleRegeneration>>,
    #[serde(
        rename = "lastTitleRegeneration",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub last_title_regeneration: Option<Option<OrchestrationV2LastTitleRegeneration>>,
    #[serde(rename = "deletedAt", with = "crate::wire::nullable")]
    pub deleted_at: Option<crate::wire::WireDateTime>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2OlderSettledThreads {
    #[serde(rename = "before")]
    pub before: crate::base_schemas::IsoDateTime,
    #[serde(rename = "asOfSequence")]
    pub as_of_sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "counts")]
    pub counts: Vec<OrchestrationV2OlderSettledCount>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ThreadShellSnapshot {
    #[serde(rename = "schemaVersion")]
    pub schema_version: crate::base_schemas::PositiveInt,
    #[serde(rename = "snapshotSequence")]
    pub snapshot_sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "threads")]
    pub threads: Vec<OrchestrationV2ThreadShell>,
    #[serde(rename = "archivedThreads")]
    pub archived_threads: Vec<OrchestrationV2ThreadShell>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ShellSnapshot {
    #[serde(rename = "schemaVersion")]
    pub schema_version: crate::base_schemas::PositiveInt,
    #[serde(rename = "snapshotSequence")]
    pub snapshot_sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "threads")]
    pub threads: Vec<OrchestrationV2ThreadShell>,
    #[serde(rename = "archivedThreads")]
    pub archived_threads: Vec<OrchestrationV2ThreadShell>,
    #[serde(rename = "projects")]
    pub projects: Vec<crate::orchestration_project::OrchestrationProjectShell>,
    #[serde(
        rename = "olderSettled",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub older_settled: Option<OrchestrationV2OlderSettledThreads>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ShellHttpSnapshot {
    #[serde(rename = "schemaVersion")]
    pub schema_version: crate::base_schemas::PositiveInt,
    #[serde(rename = "snapshotSequence")]
    pub snapshot_sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "threads")]
    pub threads: Vec<OrchestrationV2ThreadShell>,
    #[serde(rename = "archivedThreads")]
    pub archived_threads: Vec<OrchestrationV2ThreadShell>,
    #[serde(rename = "projects")]
    pub projects: Vec<crate::orchestration_project::OrchestrationProjectShell>,
    #[serde(
        rename = "olderSettled",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub older_settled: Option<OrchestrationV2OlderSettledThreads>,
    #[serde(
        rename = "projectIdentities",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub project_identities: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ShellResumable {
    #[serde(rename = "resumable")]
    pub resumable: LiteralTrue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShellStreamItemSnapshot {
    #[serde(rename = "snapshot")]
    pub snapshot: OrchestrationV2ShellSnapshot,
    #[serde(
        rename = "resolvedRepositoryIdentityRoots",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub resolved_repository_identity_roots: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShellStreamItemProjectUpdated {
    #[serde(rename = "sequence")]
    pub sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "project")]
    pub project: crate::orchestration_project::OrchestrationProjectShell,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShellStreamItemProjectRemoved {
    #[serde(rename = "sequence")]
    pub sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "projectId")]
    pub project_id: crate::base_schemas::ProjectId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ShellStreamItemThreadUpdatedLocation {
    #[serde(rename = "active")]
    Active,
    #[serde(rename = "archive")]
    Archive,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShellStreamItemThreadUpdated {
    #[serde(rename = "sequence")]
    pub sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "location")]
    pub location: ShellStreamItemThreadUpdatedLocation,
    #[serde(rename = "thread")]
    pub thread: OrchestrationV2ThreadShell,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShellStreamItemThreadRemoved {
    #[serde(rename = "sequence")]
    pub sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "location")]
    pub location: ShellStreamItemThreadUpdatedLocation,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum OrchestrationV2ShellStreamItem {
    #[serde(rename = "synchronized")]
    Synchronized(Box<ThreadForkSourcePointLatestStable>),
    #[serde(rename = "snapshot-required")]
    SnapshotRequired(Box<ThreadForkSourcePointLatestStable>),
    #[serde(rename = "snapshot")]
    Snapshot(Box<ShellStreamItemSnapshot>),
    #[serde(rename = "project.updated")]
    ProjectUpdated(Box<ShellStreamItemProjectUpdated>),
    #[serde(rename = "project.removed")]
    ProjectRemoved(Box<ShellStreamItemProjectRemoved>),
    #[serde(rename = "thread.updated")]
    ThreadUpdated(Box<ShellStreamItemThreadUpdated>),
    #[serde(rename = "thread.removed")]
    ThreadRemoved(Box<ShellStreamItemThreadRemoved>),
}
impl crate::wire::TaggedUnion for OrchestrationV2ShellStreamItem {
    const TAG: &'static str = "kind";
    const KNOWN_TAGS: &'static [&'static str] = &[
        "synchronized",
        "snapshot-required",
        "snapshot",
        "project.updated",
        "project.removed",
        "thread.updated",
        "thread.removed",
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ThreadProjectionJson {
    #[serde(rename = "thread")]
    pub thread: OrchestrationV2AppThreadJson,
    #[serde(rename = "runs")]
    pub runs: Vec<OrchestrationV2RunJson>,
    #[serde(rename = "attempts")]
    pub attempts: Vec<OrchestrationV2RunAttemptJson>,
    #[serde(rename = "nodes")]
    pub nodes: Vec<OrchestrationV2ExecutionNodeJson>,
    #[serde(rename = "subagents")]
    pub subagents: Vec<OrchestrationV2SubagentJson>,
    #[serde(rename = "providerSessions")]
    pub provider_sessions: Vec<OrchestrationV2ProviderSessionJson>,
    #[serde(rename = "providerThreads")]
    pub provider_threads: Vec<OrchestrationV2ProviderThreadJson>,
    #[serde(rename = "providerTurns")]
    pub provider_turns: Vec<OrchestrationV2ProviderTurnJson>,
    #[serde(rename = "runtimeRequests")]
    pub runtime_requests: Vec<OrchestrationV2RuntimeRequestJson>,
    #[serde(rename = "messages")]
    pub messages: Vec<OrchestrationV2ConversationMessageJson>,
    #[serde(rename = "plans")]
    pub plans: Vec<OrchestrationV2PlanArtifact>,
    #[serde(
        rename = "turnItems",
        deserialize_with = "crate::wire::deserialize_union_array"
    )]
    pub turn_items: Vec<OrchestrationV2TurnItemJson>,
    #[serde(rename = "checkpointScopes")]
    pub checkpoint_scopes: Vec<OrchestrationV2CheckpointScopeJson>,
    #[serde(rename = "checkpoints")]
    pub checkpoints: Vec<OrchestrationV2CheckpointJson>,
    #[serde(rename = "contextHandoffs")]
    pub context_handoffs: Vec<OrchestrationV2ContextHandoffJson>,
    #[serde(rename = "contextTransfers")]
    pub context_transfers: Vec<OrchestrationV2ContextTransferJson>,
    #[serde(
        rename = "visibleTurnItems",
        deserialize_with = "deserialize_projected_items"
    )]
    pub visible_turn_items: Vec<OrchestrationV2ProjectedTurnItemJson>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
}

pub type OrchestrationV2ThreadShellJson = OrchestrationV2ThreadShell;

pub type OrchestrationV2ShellSnapshotJson = OrchestrationV2ShellSnapshot;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ArchivedShellSnapshot {
    #[serde(rename = "schemaVersion")]
    pub schema_version: crate::base_schemas::PositiveInt,
    #[serde(rename = "snapshotSequence")]
    pub snapshot_sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "projects")]
    pub projects: Vec<crate::orchestration_project::OrchestrationProjectShell>,
    #[serde(rename = "threads")]
    pub threads: Vec<OrchestrationV2ThreadShell>,
}

pub type OrchestrationV2GetThreadProjectionInput = OrchestrationV2SubscribePromptSuggestionInput;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2SubscribeShellInput {
    #[serde(
        rename = "afterSequence",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub after_sequence: Option<crate::base_schemas::NonNegativeInt>,
    #[serde(
        rename = "httpSnapshotFallback",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub http_snapshot_fallback: Option<LiteralTrue>,
    #[serde(
        rename = "knownProjectIdentities",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub known_project_identities: Option<String>,
    #[serde(
        rename = "requestCompletionMarker",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub request_completion_marker: Option<bool>,
    #[serde(
        rename = "omitIdleSubagents",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub omit_idle_subagents: Option<bool>,
    #[serde(
        rename = "omitOlderSettled",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub omit_older_settled: Option<bool>,
}

pub type OrchestrationV2GetThreadFamilyInput = OrchestrationV2SubscribePromptSuggestionInput;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ThreadFamily {
    #[serde(rename = "snapshotSequence")]
    pub snapshot_sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "threads")]
    pub threads: Vec<OrchestrationV2ThreadShell>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2GetThreadTitlesInput {
    #[serde(rename = "threadIds")]
    pub thread_ids: Vec<crate::base_schemas::ThreadId>,
}

pub type OrchestrationV2ThreadTitles =
    std::collections::BTreeMap<crate::base_schemas::ThreadId, String>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2GetOlderSettledThreadsInput {
    #[serde(rename = "before")]
    pub before: OrchestrationV2OlderSettledCursor,
    #[serde(
        rename = "projectIds",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub project_ids: Option<Vec<crate::base_schemas::ProjectId>>,
    #[serde(rename = "limit")]
    pub limit: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2OlderSettledThreadsPage {
    #[serde(rename = "snapshotSequence")]
    pub snapshot_sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "threads")]
    pub threads: Vec<OrchestrationV2ThreadShell>,
    #[serde(rename = "counts")]
    pub counts: Vec<OrchestrationV2OlderSettledCount>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ThreadDetailSnapshot {
    #[serde(rename = "snapshotSequence")]
    pub snapshot_sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "projection")]
    pub projection: OrchestrationV2ThreadProjection,
    #[serde(
        rename = "historyCursor",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub history_cursor: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "hasMoreHistory",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub has_more_history: Option<bool>,
    #[serde(
        rename = "latestLocalTurnOrdinal",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub latest_local_turn_ordinal: Option<Option<crate::base_schemas::NonNegativeInt>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrchestrationV2ThreadBoundedSnapshot {
    pub snapshot_sequence: crate::base_schemas::NonNegativeInt,
    pub projection: OrchestrationV2ThreadProjection,
    pub history_cursor: Option<crate::base_schemas::TrimmedNonEmptyString>,
    pub has_more_history: bool,
    pub latest_local_turn_ordinal: Option<crate::base_schemas::NonNegativeInt>,
    pub turn_items_omit_local_visible: Option<LiteralTrue>,
    pub id_table: Option<LiteralTrue>,
    pub ids: Option<Vec<CompactId>>,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "OrchestrationV2ThreadBoundedSnapshot")]
struct BoundedSnapshotFields {
    #[serde(rename = "snapshotSequence")]
    pub snapshot_sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "projection")]
    pub projection: OrchestrationV2ThreadProjection,
    #[serde(rename = "historyCursor", with = "crate::wire::nullable")]
    pub history_cursor: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(rename = "hasMoreHistory")]
    pub has_more_history: bool,
    #[serde(rename = "latestLocalTurnOrdinal", with = "crate::wire::nullable")]
    pub latest_local_turn_ordinal: Option<crate::base_schemas::NonNegativeInt>,
    #[serde(
        rename = "turnItemsOmitLocalVisible",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub turn_items_omit_local_visible: Option<LiteralTrue>,
    #[serde(
        rename = "idTable",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub id_table: Option<LiteralTrue>,
    #[serde(
        rename = "ids",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub ids: Option<Vec<CompactId>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrchestrationV2ThreadHistoryPage {
    pub snapshot_sequence: crate::base_schemas::NonNegativeInt,
    pub items: Vec<OrchestrationV2ProjectedTurnItem>,
    pub next_cursor: Option<crate::base_schemas::TrimmedNonEmptyString>,
    pub has_more_history: bool,
    pub id_table: Option<LiteralTrue>,
    pub ids: Option<Vec<CompactId>>,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "OrchestrationV2ThreadHistoryPage")]
struct HistoryPageFields {
    #[serde(rename = "snapshotSequence")]
    pub snapshot_sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "items", deserialize_with = "deserialize_projected_items")]
    pub items: Vec<OrchestrationV2ProjectedTurnItem>,
    #[serde(rename = "nextCursor", with = "crate::wire::nullable")]
    pub next_cursor: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(rename = "hasMoreHistory")]
    pub has_more_history: bool,
    #[serde(
        rename = "idTable",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub id_table: Option<LiteralTrue>,
    #[serde(
        rename = "ids",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub ids: Option<Vec<CompactId>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ThreadStreamItemMember1Kind {
    #[serde(rename = "synchronized")]
    Synchronized,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadStreamItemMember1 {
    #[serde(rename = "kind")]
    pub kind: ThreadStreamItemMember1Kind,
    #[serde(
        rename = "ids",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub ids: Option<Vec<CompactId>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ThreadStreamItemMember2Kind {
    #[serde(rename = "snapshot")]
    Snapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadStreamItemMember2 {
    #[serde(rename = "kind")]
    pub kind: ThreadStreamItemMember2Kind,
    #[serde(rename = "snapshotSequence")]
    pub snapshot_sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "projection")]
    pub projection: OrchestrationV2ThreadProjection,
    #[serde(
        rename = "historyCursor",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub history_cursor: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "hasMoreHistory",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub has_more_history: Option<bool>,
    #[serde(
        rename = "latestLocalTurnOrdinal",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub latest_local_turn_ordinal: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "turnItemsOmitLocalVisible",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub turn_items_omit_local_visible: Option<LiteralTrue>,
    #[serde(
        rename = "idTable",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub id_table: Option<LiteralTrue>,
    #[serde(
        rename = "ids",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub ids: Option<Vec<CompactId>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ThreadStreamItemMember3Kind {
    #[serde(rename = "event")]
    Event,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadStreamItemMember3 {
    #[serde(rename = "kind")]
    pub kind: ThreadStreamItemMember3Kind,
    #[serde(rename = "sequence")]
    pub sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "event")]
    pub event: OrchestrationV2DomainEvent,
    #[serde(
        rename = "ids",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub ids: Option<Vec<CompactId>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnknownThreadStreamEventEvent {
    #[serde(rename = "type")]
    pub r#type: String,
    #[serde(
        rename = "payload",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub payload: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2UnknownThreadStreamEvent {
    #[serde(rename = "kind")]
    pub kind: ThreadStreamItemMember3Kind,
    #[serde(rename = "sequence")]
    pub sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "event")]
    pub event: UnknownThreadStreamEventEvent,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OrchestrationV2ThreadStreamItem {
    Synchronized(Box<ThreadStreamItemMember1>),
    Snapshot(Box<ThreadStreamItemMember2>),
    Event(Box<ThreadStreamItemMember3>),
    UnknownEvent { sequence: u64, event_type: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GetThreadProjectionErrorTag {
    #[serde(rename = "OrchestrationV2GetThreadProjectionError")]
    OrchestrationV2GetThreadProjectionError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2GetThreadProjectionError {
    #[serde(rename = "_tag")]
    pub _tag: GetThreadProjectionErrorTag,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "message")]
    pub message: String,
    #[serde(
        rename = "cause",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GetShellSnapshotErrorTag {
    #[serde(rename = "OrchestrationV2GetShellSnapshotError")]
    OrchestrationV2GetShellSnapshotError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2GetShellSnapshotError {
    #[serde(rename = "_tag")]
    pub _tag: GetShellSnapshotErrorTag,
    #[serde(rename = "message")]
    pub message: String,
    #[serde(
        rename = "cause",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderReplayTranscript {
    #[serde(rename = "provider")]
    pub provider: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "protocol")]
    pub protocol: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "version")]
    pub version: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "scenario")]
    pub scenario: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "metadata",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub metadata: Option<Option<std::collections::BTreeMap<String, serde_json::Value>>>,
    #[serde(rename = "entries")]
    pub entries: Vec<ProviderReplayEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProviderReplayTranscriptHeaderType {
    #[serde(rename = "transcript_start")]
    TranscriptStart,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderReplayTranscriptHeader {
    #[serde(rename = "type")]
    pub r#type: ProviderReplayTranscriptHeaderType,
    #[serde(rename = "provider")]
    pub provider: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "protocol")]
    pub protocol: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "version")]
    pub version: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "scenario")]
    pub scenario: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "metadata",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub metadata: Option<Option<std::collections::BTreeMap<String, serde_json::Value>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderReplayNdjsonRecordTranscriptStart {
    #[serde(rename = "provider")]
    pub provider: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "protocol")]
    pub protocol: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "version")]
    pub version: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "scenario")]
    pub scenario: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "metadata",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub metadata: Option<Option<std::collections::BTreeMap<String, serde_json::Value>>>,
}

impl<'de> Deserialize<'de> for OrchestrationV2ThreadBoundedSnapshot {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use crate::orchestration_v2_id_table::{RecordContainer, decode_record_id_table};
        let raw = serde_json::Value::deserialize(deserializer)?;
        let mut expanded = raw.clone();
        decode_record_id_table(&mut expanded, RecordContainer::Key("projection"))
            .map_err(serde::de::Error::custom)?;
        BoundedSnapshotFields::deserialize(raw).map_err(serde::de::Error::custom)
    }
}
impl Serialize for OrchestrationV2ThreadBoundedSnapshot {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use crate::orchestration_v2_id_table::{RecordContainer, encode_record_id_table};
        #[derive(Serialize)]
        struct Encoded<'a>(
            #[serde(with = "BoundedSnapshotFields")] &'a OrchestrationV2ThreadBoundedSnapshot,
        );
        if self.id_table.is_some() {
            encode_record_id_table(&Encoded(self), RecordContainer::Key("projection"))
                .map_err(serde::ser::Error::custom)?
                .serialize(serializer)
        } else {
            BoundedSnapshotFields::serialize(self, serializer)
        }
    }
}

impl<'de> Deserialize<'de> for OrchestrationV2ThreadHistoryPage {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use crate::orchestration_v2_id_table::{RecordContainer, decode_record_id_table};
        let raw = serde_json::Value::deserialize(deserializer)?;
        let mut expanded = raw.clone();
        decode_record_id_table(&mut expanded, RecordContainer::TopLevel)
            .map_err(serde::de::Error::custom)?;
        HistoryPageFields::deserialize(raw).map_err(serde::de::Error::custom)
    }
}
impl Serialize for OrchestrationV2ThreadHistoryPage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use crate::orchestration_v2_id_table::{RecordContainer, encode_record_id_table};
        #[derive(Serialize)]
        struct Encoded<'a>(
            #[serde(with = "HistoryPageFields")] &'a OrchestrationV2ThreadHistoryPage,
        );
        if self.id_table.is_some() {
            encode_record_id_table(&Encoded(self), RecordContainer::TopLevel)
                .map_err(serde::ser::Error::custom)?
                .serialize(serializer)
        } else {
            HistoryPageFields::serialize(self, serializer)
        }
    }
}

impl<'de> Deserialize<'de> for OrchestrationV2ThreadStreamItem {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use crate::orchestration_v2_id_table::{RecordContainer, decode_record_id_table};
        use crate::wire::TaggedUnion;
        let raw = serde_json::Value::deserialize(deserializer)?;
        let kind = raw
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| serde::de::Error::custom("expected a stream kind"))?;
        let mut expanded = raw.clone();
        let container = if kind == "snapshot" {
            RecordContainer::Key("projection")
        } else {
            RecordContainer::NoRecords
        };
        decode_record_id_table(&mut expanded, container).map_err(serde::de::Error::custom)?;
        let decode = |error| serde::de::Error::custom(error);
        match kind {
            "synchronized" => serde_json::from_value(raw)
                .map(|item| Self::Synchronized(Box::new(item)))
                .map_err(decode),
            "snapshot" => serde_json::from_value(raw)
                .map(|item| Self::Snapshot(Box::new(item)))
                .map_err(decode),
            "event" => {
                let event = raw
                    .get("event")
                    .ok_or_else(|| serde::de::Error::custom("missing stream event"))?;
                let event_type = event
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| serde::de::Error::custom("missing event type"))?;
                let unknown_item = event_type == "turn-item.updated"
                    && event
                        .get("payload")
                        .and_then(|item| item.get("type"))
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|tag| !OrchestrationV2TurnItem::KNOWN_TAGS.contains(&tag));
                if !OrchestrationV2DomainEvent::KNOWN_TAGS.contains(&event_type) || unknown_item {
                    let sequence = serde_json::from_value(
                        raw.get("sequence")
                            .cloned()
                            .ok_or_else(|| serde::de::Error::custom("missing sequence"))?,
                    )
                    .map_err(decode)?;
                    Ok(Self::UnknownEvent {
                        sequence,
                        event_type: event_type.to_owned(),
                    })
                } else {
                    serde_json::from_value(raw)
                        .map(|item| Self::Event(Box::new(item)))
                        .map_err(decode)
                }
            }
            _ => Err(serde::de::Error::custom("unknown stream kind")),
        }
    }
}
impl Serialize for OrchestrationV2ThreadStreamItem {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use crate::orchestration_v2_id_table::{RecordContainer, encode_record_id_table};
        match self {
            Self::Synchronized(item) => item.serialize(serializer),
            Self::Snapshot(item) if item.id_table.is_some() => {
                encode_record_id_table(item, RecordContainer::Key("projection"))
                    .map_err(serde::ser::Error::custom)?
                    .serialize(serializer)
            }
            Self::Snapshot(item) => item.serialize(serializer),
            Self::Event(item) => item.serialize(serializer),
            Self::UnknownEvent { .. } => Err(serde::ser::Error::custom(
                "Servers never send unknown thread events.",
            )),
        }
    }
}
