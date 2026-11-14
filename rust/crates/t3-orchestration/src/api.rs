use crate::{
    effects::*,
    error::Result,
    provider::{ProviderAdapter, ProviderEvent, SessionRuntime},
    stores::receipts::CommandReceipt,
};
use async_trait::async_trait;
use std::{path::PathBuf, sync::Arc};
pub use t3_contracts::{
    application_event::{ApplicationProjectEvent, ApplicationStoredEvent},
    base_schemas::*,
    orchestration_v2::*,
};
use t3_contracts::{
    provider_instance::ProviderInstanceId, thread_pull_request::ThreadPullRequestLink,
};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub trait Clock: Send + Sync {
    fn now(&self) -> IsoDateTime;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdKind {
    Event,
    Command,
    Run,
    RunAttempt,
    Node,
    Message,
    ProviderSession,
    ProviderThread,
    ProviderTurn,
    TurnItem,
    Checkpoint,
    CheckpointScope,
    ContextTransfer,
    ContextHandoff,
    RuntimeRequest,
    Thread,
}
pub trait IdGenerator: Send + Sync {
    fn next(&self, kind: IdKind) -> String;
}
#[derive(Debug, Clone)]
pub struct DecisionContext {
    pub projection: Option<OrchestrationV2ThreadProjection>,
    pub now: IsoDateTime,
    pub ids: Vec<(IdKind, String)>,
    pub capabilities: Option<OrchestrationV2ProviderCapabilities>,
}
#[derive(Debug, Clone, Default)]
pub struct Decision {
    pub events: Vec<OrchestrationV2DomainEvent>,
    pub effects: Vec<PendingEffect>,
    pub cancellations: Vec<CancelEffects>,
}
#[derive(Debug, Clone)]
pub struct CancelEffects {
    pub thread_id: ThreadId,
    pub effect_types: Vec<String>,
    pub reason: String,
}
pub trait DecisionEngine: Send + Sync {
    fn decide(
        &self,
        context: &DecisionContext,
        command: &OrchestrationV2ServerCommand,
    ) -> Result<Decision>;

    fn decide_prepared(
        &self,
        _context: &PreparedDecisionContext,
        _command: &OrchestrationV2ServerCommand,
    ) -> Result<Decision> {
        Err(crate::error::Error::Unsupported(
            "DecisionEngine.decide_prepared",
        ))
    }
}

#[derive(Debug, Clone, Default)]
pub struct DecisionInputs {
    pub runtime_policy: Option<crate::provider::ConfiguredRuntimePolicy>,
    pub selection_transition: Option<crate::provider::SelectionTransitionPlan>,
    pub background_work: Vec<(ProviderThreadId, bool)>,
    pub related_projections: Vec<OrchestrationV2ThreadProjection>,
    pub projects: Vec<crate::stores::projects::ProjectRow>,
    pub next_item_ordinals: Vec<(ThreadId, Option<RunId>, u64)>,
    pub shell_completions: Vec<(ThreadId, Option<IsoDateTime>)>,
    /// The selected adapters' registry metadata (identity, capabilities, enabled state).
    pub provider_metadata: Vec<(ProviderInstanceId, AdapterMetadata)>,
    pub provider_session_ids: Vec<(ProviderInstanceId, ProviderSessionId)>,
    /// Observed presence of a live native session. An unobserved session is absent from the
    /// list, which differs from an observed missing session (`false`).
    pub live_sessions: Vec<(ProviderSessionId, bool)>,
    /// Live sessions whose runtime applies a model selection mid-turn (TS
    /// `session.applyModelSelection !== undefined`).
    pub selection_sessions: Vec<ProviderSessionId>,
    /// Project-scoped enablement (`provider_instance_enabled`) for the instances involved.
    pub provider_enabled: Vec<(ProviderInstanceId, bool)>,
    pub supported_runtime_modes: Vec<(
        ProviderInstanceId,
        Vec<t3_contracts::provider_policy::RuntimeMode>,
    )>,
    /// `CheckpointRestoreSafety`'s filesystem and project checks, run outside the decision.
    pub restore_isolated: Option<bool>,
    /// Exact per-thread attachment inventories, side chats included.
    pub attachment_ids: Vec<(ThreadId, Vec<String>)>,
    /// Composer message context the host's `MessageContextPreparer` resolved.
    pub message_context: Option<crate::decision::engine::PreparedMessageContext>,
    /// Runs with a pending restart continuation, per thread.
    pub restart_continuations: crate::ports::decisions::orchestrator::RestartContinuations,
}

#[derive(Debug, Clone)]
pub struct PreparedDecisionContext {
    pub context: DecisionContext,
    pub inputs: DecisionInputs,
}

impl DecisionContext {
    pub fn with_inputs(self, inputs: DecisionInputs) -> PreparedDecisionContext {
        PreparedDecisionContext {
            context: self,
            inputs,
        }
    }
}
#[derive(Debug, Clone)]
pub enum CommitGuard {
    None,
    RunCurrent {
        thread_id: ThreadId,
        run_id: RunId,
        attempt_id: RunAttemptId,
        status: OrchestrationV2RunStatus,
    },
    ProviderThreadOwner {
        provider_thread_id: ProviderThreadId,
        run_id: RunId,
        attempt_id: RunAttemptId,
        last_run_ordinal: u64,
    },
    ProviderThreadUnchanged(Box<OrchestrationV2ProviderThread>),
    TurnItemUnchanged(Box<OrchestrationV2TurnItem>),
}
#[derive(Debug, Clone)]
pub struct CommitRequest {
    pub receipt: Option<CommandReceipt>,
    pub decision: Decision,
    pub guard: CommitGuard,
}
#[derive(Debug, Clone)]
pub struct CommitResult {
    pub committed: bool,
    pub receipt: Option<CommandReceipt>,
    pub events: Vec<OrchestrationV2StoredEvent>,
    pub cancelled_effect_ids: Vec<String>,
}

pub use crate::runtime::commit::WriteOptions as CommitOptions;

/// Runs after receipt deduplication and guards, before normalization, on the writer transaction.
/// No external I/O or publication; all callback writes roll back if any commit stage fails.
pub type CommitPreparation = Box<
    dyn FnOnce(&t3_persistence::rusqlite::Transaction<'_>, CommitRequest) -> Result<CommitRequest>
        + Send
        + 'static,
>;
pub trait ProjectionWriter: Send + Sync {
    fn apply(
        &self,
        tx: &t3_persistence::rusqlite::Transaction<'_>,
        event: &OrchestrationV2StoredEvent,
    ) -> Result<()>;
}
#[derive(Debug, Clone)]
pub struct TimelineQuery {
    pub thread_id: ThreadId,
    pub before_position: Option<u64>,
    pub after_position: Option<u64>,
    pub limit: u64,
    pub item_id: Option<TurnItemId>,
    pub attachment_id: Option<String>,
    pub view: Option<TimelineView>,
}
#[derive(Debug, Clone)]
pub struct TimelinePage {
    pub items: Vec<OrchestrationV2ProjectedTurnItem>,
    pub total_items: u64,
    pub has_more: bool,
    pub has_earlier: bool,
}
#[derive(Debug, Clone)]
pub struct ControlContext {
    pub thread: OrchestrationV2AppThread,
    pub run: Option<OrchestrationV2Run>,
    pub attempt: Option<OrchestrationV2RunAttempt>,
    pub node: Option<OrchestrationV2ExecutionNode>,
    pub provider_session: Option<OrchestrationV2ProviderSession>,
    pub provider_thread: Option<OrchestrationV2ProviderThread>,
    pub provider_turn: Option<OrchestrationV2ProviderTurn>,
    pub request: Option<OrchestrationV2RuntimeRequest>,
    pub message: Option<OrchestrationV2ConversationMessage>,
    pub item: Option<OrchestrationV2TurnItem>,
}
#[derive(Debug, Clone)]
pub enum ControlQuery {
    TurnStart { thread_id: ThreadId, run_id: RunId },
    RunningTurn(ThreadId),
    RuntimeResponse(RuntimeRequestId),
    ProviderThread(ProviderThreadId),
    Checkpoint(ThreadId),
    Settlement(ThreadId),
    Recovery(ThreadId),
}
#[derive(Debug, Clone, Default)]
pub struct SnapshotWindow {
    pub row_limit: u64,
    pub user_turn_limit: Option<u64>,
    pub defer_work: bool,
    pub anchor_item_id: Option<TurnItemId>,
    pub anchor_thread_id: Option<ThreadId>,
    pub required_run_id: Option<RunId>,
}
#[derive(Debug, Clone)]
pub struct ProjectionSnapshot {
    pub schema_version: u64,
    pub snapshot_sequence: u64,
    pub projection: OrchestrationV2ThreadProjection,
}
#[derive(Debug, Clone, Default)]
pub struct ShellQuery {
    pub archived: bool,
    pub project_id: Option<ProjectId>,
    pub unsettled_only: bool,
    pub family_of: Option<ThreadId>,
    pub worktree_path: Option<String>,
    pub thread_ids: Option<Vec<ThreadId>>,
}
#[derive(Debug, Clone)]
pub struct HistoryQuery {
    pub thread_id: ThreadId,
    pub cursor: String,
    pub through_entry_id: Option<String>,
    pub conversation_only: bool,
    pub work_mode: Option<HistoryWorkMode>,
}
#[derive(Debug, Clone, Copy)]
pub enum HistoryWorkMode {
    Deferred,
    Work,
}
#[derive(Debug, Clone)]
pub struct CheckpointCaptureContext {
    pub run: Option<OrchestrationV2Run>,
    pub root_node: Option<OrchestrationV2ExecutionNode>,
    pub scope: Option<OrchestrationV2CheckpointScope>,
    pub provider_thread: Option<OrchestrationV2ProviderThread>,
    pub preserved_ordinals: Vec<u64>,
    pub side_chat: bool,
}
#[derive(Debug, Clone)]
pub struct CheckpointRollbackContext {
    pub thread: OrchestrationV2AppThread,
    pub provider_thread: Option<OrchestrationV2ProviderThread>,
    pub session: Option<OrchestrationV2ProviderSession>,
    pub checkpoint: Option<OrchestrationV2Checkpoint>,
    pub scope: Option<OrchestrationV2CheckpointScope>,
    pub target_attempt: Option<OrchestrationV2RunAttempt>,
    pub provider_turns: Vec<OrchestrationV2ProviderTurn>,
    pub rolled_back_attempt_ids: Vec<RunAttemptId>,
    pub runs_to_rollback: Vec<OrchestrationV2Run>,
    pub root_nodes: Vec<OrchestrationV2ExecutionNode>,
    pub stale_checkpoints: Vec<OrchestrationV2Checkpoint>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    Runs,
    Attempts,
    Nodes,
    Subagents,
    ProviderSessions,
    ProviderThreads,
    ProviderTurns,
    RuntimeRequests,
    Messages,
    Plans,
    TurnItems,
    CheckpointScopes,
    Checkpoints,
    ContextHandoffs,
    ContextTransfers,
}
#[derive(Debug, Clone)]
pub enum ProjectionRecordSet {
    Runs(Vec<OrchestrationV2Run>),
    Attempts(Vec<OrchestrationV2RunAttempt>),
    Nodes(Vec<OrchestrationV2ExecutionNode>),
    Subagents(Vec<OrchestrationV2Subagent>),
    ProviderSessions(Vec<OrchestrationV2ProviderSession>),
    ProviderThreads(Vec<OrchestrationV2ProviderThread>),
    ProviderTurns(Vec<OrchestrationV2ProviderTurn>),
    RuntimeRequests(Vec<OrchestrationV2RuntimeRequest>),
    Messages(Vec<OrchestrationV2ConversationMessage>),
    Plans(Vec<OrchestrationV2PlanArtifact>),
    TurnItems(Vec<OrchestrationV2TurnItem>),
    CheckpointScopes(Vec<OrchestrationV2CheckpointScope>),
    Checkpoints(Vec<OrchestrationV2Checkpoint>),
    ContextHandoffs(Vec<OrchestrationV2ContextHandoff>),
    ContextTransfers(Vec<OrchestrationV2ContextTransfer>),
}
#[derive(Debug, Clone)]
pub struct ProjectionRecords {
    pub thread: OrchestrationV2AppThread,
    pub records: Vec<ProjectionRecordSet>,
}
#[derive(Debug, Clone, Default)]
pub struct RecordFilter {
    pub run_ids: Option<Vec<RunId>>,
    pub attempt_ids: Option<Vec<RunAttemptId>>,
    pub provider_thread_ids: Option<Vec<ProviderThreadId>>,
    pub provider_turn_thread_ids: Option<Vec<ProviderThreadId>>,
    pub latest_provider_turn_usage_only: bool,
    pub runtime_request_statuses: Option<Vec<RuntimeRequestStatus>>,
    pub context_transfer_statuses: Option<Vec<ContextTransferStatus>>,
    pub subagent_ids: Option<Vec<NodeId>>,
    pub subagent_run_ids: Option<Vec<RunId>>,
    pub node_ids: Option<Vec<NodeId>>,
    pub turn_item_request_ids: Option<Vec<RuntimeRequestId>>,
    pub turn_item_types: Option<Vec<String>>,
    pub turn_item_statuses: Option<Vec<OrchestrationV2TurnItemStatus>>,
    pub omit_settled_leaf_nodes: bool,
    pub omit_settled_subagents: bool,
    pub except_subagent_ids: Vec<NodeId>,
    pub omit_settled_subagent_records: bool,
    pub since_last_compaction: Option<(Vec<RunId>, MessageId)>,
    /// Turn items of one run (`Some`) or runless (`None`); TS's single-run form.
    pub turn_item_run_id: Option<Option<RunId>>,
    /// A `None` entry matches runless items; an empty list matches nothing.
    pub turn_item_run_ids: Option<Vec<Option<RunId>>>,
    pub turn_item_node_ids: Option<Vec<NodeId>>,
    /// Provider turns by attempt, independent of `attempt_ids` (attempt records).
    pub provider_turn_attempt_ids: Option<Vec<RunAttemptId>>,
}

#[derive(Debug, Clone, Default)]
pub struct MessageFilter {
    pub ids: Option<Vec<MessageId>>,
    pub roles: Option<Vec<ConversationMessageRole>>,
    pub run_ids: Option<Vec<RunId>>,
    pub monitor_notifications_only: bool,
}

#[derive(Debug, Clone)]
pub struct ProjectionRecordQuery {
    pub thread_id: ThreadId,
    pub fields: Vec<RecordKind>,
    pub filter: RecordFilter,
    pub messages: MessageFilter,
    pub run_statuses: Option<Vec<OrchestrationV2RunStatus>>,
    pub include_latest_run: bool,
}

#[derive(Debug, Clone)]
pub struct ProviderTargetQuery {
    pub thread_id: ThreadId,
    pub provider_thread_id: ProviderThreadId,
    pub provider_turn_id: ProviderTurnId,
    pub attempt_id: Option<RunAttemptId>,
    pub message_id: Option<MessageId>,
}

pub use crate::projections::control::{CheckpointContext, TurnStartContext};

/// One row of the store's active shell index (TS `ActiveShellIndexEntry`).
#[derive(Debug, Clone)]
pub struct ActiveShellIndexEntry {
    pub id: ThreadId,
    pub project_id: ProjectId,
    pub lineage: OrchestrationV2AppThreadLineage,
    pub settled_override: Option<AppThreadSettledOverride>,
    pub settled_at: Option<IsoDateTime>,
    pub pinned_at: Option<Option<IsoDateTime>>,
    pub snoozed_until: Option<Option<IsoDateTime>>,
    /// False only when no run, request, work item or watch can make the shell live.
    pub may_be_live: bool,
    /// Whether it delegated a task to a linked environment.
    pub has_remote_tasks: bool,
}

/// Chooses which shells to read from the index. Runs inside the read transaction: no I/O.
pub type ShellIndexSelection =
    Box<dyn FnOnce(&[ActiveShellIndexEntry]) -> Vec<ThreadId> + Send + 'static>;

pub struct SubscriberShellRead {
    /// Untrimmed active snapshot (as `shell_snapshot(None, None)`) of the selected threads.
    pub snapshot: OrchestrationV2ShellSnapshot,
    /// The index the selection saw; `None` when no selection was given.
    pub index: Option<Vec<ActiveShellIndexEntry>>,
}

#[async_trait]
pub trait ProjectionReader: Send + Sync {
    async fn thread_titles(
        &self,
        _thread_ids: Vec<ThreadId>,
    ) -> Result<OrchestrationV2ThreadTitles> {
        Err(crate::error::Error::Unsupported(
            "ProjectionReader.thread_titles",
        ))
    }
    async fn thread_link_metadata(
        &self,
        _thread_ids: Vec<ThreadId>,
    ) -> Result<std::collections::BTreeMap<ThreadId, OrchestrationV2ThreadLinkMetadata>> {
        Err(crate::error::Error::Unsupported(
            "ProjectionReader.thread_link_metadata",
        ))
    }
    async fn older_settled_threads(
        &self,
        _input: OrchestrationV2GetOlderSettledThreadsInput,
    ) -> Result<OrchestrationV2OlderSettledThreadsPage> {
        Err(crate::error::Error::Unsupported(
            "ProjectionReader.older_settled_threads",
        ))
    }
    /// Reads the requested thread first; a request owned by another thread yields a context
    /// without request, node, item or session, and that thread is never decoded.
    async fn runtime_response_context(
        &self,
        _thread_id: ThreadId,
        _request_id: RuntimeRequestId,
    ) -> Result<Option<ControlContext>> {
        Err(crate::error::Error::Unsupported(
            "ProjectionReader.runtime_response_context",
        ))
    }
    /// Run, scope and checkpoint metadata for diffs, without transcript or full bodies.
    async fn checkpoint_context(&self, _thread_id: ThreadId) -> Result<CheckpointContext> {
        Err(crate::error::Error::Unsupported(
            "ProjectionReader.checkpoint_context",
        ))
    }
    async fn turn_start_context(
        &self,
        _thread_id: ThreadId,
        _run_id: RunId,
    ) -> Result<TurnStartContext> {
        Err(crate::error::Error::Unsupported(
            "ProjectionReader.turn_start_context",
        ))
    }
    /// Index, selected shells, projects and latest sequence come from one read transaction.
    async fn subscriber_shell_snapshot(
        &self,
        _select: Option<ShellIndexSelection>,
    ) -> Result<SubscriberShellRead> {
        Err(crate::error::Error::Unsupported(
            "ProjectionReader.subscriber_shell_snapshot",
        ))
    }
    async fn query_records(&self, _query: ProjectionRecordQuery) -> Result<ProjectionRecords> {
        Err(crate::error::Error::Unsupported(
            "ProjectionReader.query_records",
        ))
    }
    async fn message_count(&self, _thread_id: ThreadId, _filter: MessageFilter) -> Result<u64> {
        Err(crate::error::Error::Unsupported(
            "ProjectionReader.message_count",
        ))
    }
    async fn runtime_recovery(
        &self,
        _thread_id: ThreadId,
    ) -> Result<OrchestrationV2ThreadProjection> {
        Err(crate::error::Error::Unsupported(
            "ProjectionReader.runtime_recovery",
        ))
    }
    async fn provider_target(&self, _query: ProviderTargetQuery) -> Result<Option<ControlContext>> {
        Err(crate::error::Error::Unsupported(
            "ProjectionReader.provider_target",
        ))
    }
    async fn records(
        &self,
        thread_id: ThreadId,
        fields: Vec<RecordKind>,
        filter: RecordFilter,
    ) -> Result<ProjectionRecords>;
    async fn snapshot(
        &self,
        thread_id: ThreadId,
        window: Option<SnapshotWindow>,
    ) -> Result<ProjectionSnapshot>;
    async fn history(&self, query: HistoryQuery) -> Result<OrchestrationV2ThreadHistoryPage>;
    async fn search(
        &self,
        query: OrchestrationV2SearchThreadInput,
    ) -> Result<OrchestrationV2SearchThreadResult>;
    async fn thread_shells(&self, query: ShellQuery) -> Result<OrchestrationV2ThreadShellSnapshot>;
    async fn checkpoint_capture(
        &self,
        thread_id: ThreadId,
        run_id: RunId,
        scope_id: CheckpointScopeId,
    ) -> Result<CheckpointCaptureContext>;
    async fn checkpoint_rollback(
        &self,
        thread_id: ThreadId,
        provider_thread_id: ProviderThreadId,
        checkpoint_id: CheckpointId,
        scope_id: CheckpointScopeId,
    ) -> Result<CheckpointRollbackContext>;
    async fn runtime_request(
        &self,
        thread_id: ThreadId,
        request_id: RuntimeRequestId,
    ) -> Result<Option<OrchestrationV2RuntimeRequest>>;
    async fn turn_item(
        &self,
        thread_id: ThreadId,
        item_id: TurnItemId,
    ) -> Result<Option<OrchestrationV2TurnItem>>;
    async fn plan(
        &self,
        thread_id: ThreadId,
        plan_id: PlanId,
    ) -> Result<Option<OrchestrationV2PlanArtifact>>;
    async fn held_queued_runs(&self, thread_id: ThreadId) -> Result<Vec<OrchestrationV2Run>>;
    async fn app_owned_child_threads(&self, thread_id: ThreadId) -> Result<Vec<ThreadId>>;
    async fn pending_delegated_completion(&self, child_thread_id: ThreadId) -> Result<bool>;
    async fn turn_start_history(
        &self,
        thread_id: ThreadId,
        run_ids: Option<Vec<RunId>>,
    ) -> Result<Vec<OrchestrationV2TurnItem>>;
    async fn unpaired_interrupt(
        &self,
        thread_id: ThreadId,
        request_id: TurnItemId,
        result_id: TurnItemId,
    ) -> Result<bool>;
    async fn attachment_ids(&self, thread_id: ThreadId) -> Result<Vec<String>>;
    async fn transcript_byte_floor(&self, thread_id: ThreadId, limit: u64) -> Result<u64>;

    async fn thread(&self, thread_id: ThreadId) -> Result<Option<OrchestrationV2AppThread>>;
    async fn projection(
        &self,
        thread_id: ThreadId,
    ) -> Result<Option<OrchestrationV2ThreadProjection>>;
    async fn shell(&self, thread_id: ThreadId) -> Result<Option<OrchestrationV2ThreadShell>>;
    async fn shell_snapshot(
        &self,
        project_ids: Option<Vec<ProjectId>>,
        root_thread_id: Option<ThreadId>,
    ) -> Result<OrchestrationV2ShellSnapshot>;
    async fn control(&self, query: ControlQuery) -> Result<Option<ControlContext>>;
    async fn timeline(&self, query: TimelineQuery) -> Result<TimelinePage>;
    async fn run(&self, thread_id: ThreadId, run_id: RunId) -> Result<Option<OrchestrationV2Run>>;
    async fn run_message(
        &self,
        thread_id: ThreadId,
        run_id: RunId,
    ) -> Result<Option<OrchestrationV2ConversationMessage>>;
    async fn can_start_queued_run(&self, thread_id: ThreadId, run_id: RunId) -> Result<bool>;
    async fn recovery_thread_ids(&self) -> Result<Vec<ThreadId>>;
    async fn unreadable_thread_ids(&self) -> Result<Vec<ThreadId>>;
    async fn next_turn_item_ordinal(
        &self,
        thread_id: ThreadId,
        run_id: Option<RunId>,
    ) -> Result<u64>;
}
#[async_trait]
pub trait EventCommitter: Send + Sync {
    async fn commit(&self, input: CommitRequest) -> Result<CommitResult>;
    async fn commit_with_options(
        &self,
        input: CommitRequest,
        options: CommitOptions,
    ) -> Result<CommitResult> {
        if options.command_id.is_some()
            || options.unless_native_thread_owned.is_some()
            || options.guard_pending_user_input_cancellations
            || options.update_delegated_completion_only_run_id.is_some()
            || options.update_all_delegated_completions_only
            || options.unless_interrupt_requested.is_some()
        {
            return Err(crate::error::Error::Unsupported(
                "EventCommitter.commit_with_options",
            ));
        }
        self.commit(input).await
    }
    async fn commit_prepared(
        &self,
        _input: CommitRequest,
        _options: CommitOptions,
        _prepare: CommitPreparation,
    ) -> Result<CommitResult> {
        Err(crate::error::Error::Unsupported(
            "EventCommitter.commit_prepared",
        ))
    }
    async fn commit_project(
        &self,
        event: ApplicationProjectEvent,
        receipt: CommandReceipt,
    ) -> Result<CommandReceipt>;
    async fn events_by_command(
        &self,
        command_id: CommandId,
    ) -> Result<Vec<OrchestrationV2StoredEvent>>;
    fn subscribe(&self) -> broadcast::Receiver<ApplicationStoredEvent>;
    /// Same publication as `subscribe`, shared rather than cloned per receiver.
    fn subscribe_shared(&self) -> Result<broadcast::Receiver<Arc<ApplicationStoredEvent>>> {
        Err(crate::error::Error::Unsupported(
            "EventCommitter.subscribe_shared",
        ))
    }
    async fn latest_sequence(&self, thread_id: Option<ThreadId>) -> Result<u64>;
}
#[async_trait]
pub trait CommandDispatcher: Send + Sync {
    fn command_lanes(
        &self,
    ) -> Result<Arc<crate::ports::decisions::thread_command_executor::KeyedLanes>> {
        Err(crate::error::Error::Unsupported(
            "CommandDispatcher.command_lanes",
        ))
    }
    async fn dispatch(&self, command: OrchestrationV2ServerCommand) -> Result<CommandReceipt>;
}
#[async_trait]
pub trait EffectExecutor: Send + Sync {
    async fn execute(&self, effect: EffectRow, cancellation: CancellationToken) -> Result<()>;
    async fn execute_with_options(
        &self,
        effect: EffectRow,
        cancellation: CancellationToken,
        _options: EffectExecutionOptions,
    ) -> Result<()> {
        self.execute(effect, cancellation).await
    }
    async fn failed(
        &self,
        effect: EffectRow,
        error: &crate::error::Error,
        will_retry: bool,
    ) -> Result<()>;
}
#[async_trait]
pub trait EffectWorker: Send + Sync {
    async fn run_once(&self, options: ClaimOptions) -> Result<bool>;
    async fn run_recovery_once(&self) -> Result<bool>;
    async fn drain(&self, max_effects: usize) -> Result<usize>;
    async fn await_work(&self) -> Result<()>;
    async fn await_settled(&self, effect_id: String) -> Result<()>;
    async fn shutdown(&self) -> Result<()>;
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoverySummary {
    pub terminalized_runs: u64,
    pub stopped_sessions: u64,
    pub closed_requests: u64,
    pub retired_effects: u64,
    pub requeued_effects: u64,
}
#[async_trait]
pub trait RecoveryService: Send + Sync {
    async fn recover(&self) -> Result<RecoverySummary>;
    async fn reconcile(&self, thread_id: Option<ThreadId>) -> Result<RecoverySummary>;
    async fn prepare_for_shutdown(&self) -> Result<()>;
}
#[derive(Debug, Clone)]
pub struct CaptureCheckpoint {
    pub scope: OrchestrationV2CheckpointScope,
    pub ordinal: u64,
    pub message: String,
}
#[derive(Debug, Clone)]
pub struct CheckpointDiff {
    pub files: Vec<OrchestrationV2CheckpointFileSummary>,
    pub patch: String,
}
#[async_trait]
pub trait CheckpointBackend: Send + Sync {
    async fn capture(
        &self,
        input: CaptureCheckpoint,
        cancellation: CancellationToken,
    ) -> Result<CheckpointRef>;
    async fn lookup(
        &self,
        scope: OrchestrationV2CheckpointScope,
        ordinal: u64,
    ) -> Result<Option<CheckpointRef>>;
    async fn diff(
        &self,
        cwd: PathBuf,
        from: CheckpointRef,
        to: CheckpointRef,
    ) -> Result<CheckpointDiff>;
    async fn restore(
        &self,
        scope: OrchestrationV2CheckpointScope,
        ordinal: u64,
        cancellation: CancellationToken,
    ) -> Result<()>;
    async fn prune(&self, scope: OrchestrationV2CheckpointScope, preserve: Vec<u64>) -> Result<()>;
    fn checkpoint_ref(
        &self,
        _scope_id: &CheckpointScopeId,
        _ordinal: u64,
    ) -> Result<CheckpointRef> {
        Err(crate::error::Error::Unsupported(
            "CheckpointBackend.checkpoint_ref",
        ))
    }
    async fn is_git_repository(&self, _cwd: PathBuf) -> Result<bool> {
        Err(crate::error::Error::Unsupported(
            "CheckpointBackend.is_git_repository",
        ))
    }
    /// Rejects, without deleting anything, any checkpoint whose ref this scope does not own.
    async fn delete_checkpoint_refs(
        &self,
        _scope: OrchestrationV2CheckpointScope,
        _checkpoints: Vec<OrchestrationV2Checkpoint>,
    ) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "CheckpointBackend.delete_checkpoint_refs",
        ))
    }
}

/// TS `RuntimePolicyV2`. A policy above the managed ceiling fails with
/// `Error::Service { operation: "runtime-policy.above-ceiling", detail: <client message> }`.
#[async_trait]
pub trait RuntimePolicyResolver: Send + Sync {
    async fn resolve(
        &self,
        thread: OrchestrationV2AppThread,
        model_selection: t3_contracts::model_selection::ModelSelection,
    ) -> Result<crate::provider::ConfiguredRuntimePolicy>;
}
#[async_trait]
pub trait TitleGenerator: Send + Sync {
    async fn generate_with_input(
        &self,
        _input: TitleGenerationInput,
        _cancellation: CancellationToken,
    ) -> Result<String> {
        Err(crate::error::Error::Unsupported(
            "TitleGenerator.generate_with_input",
        ))
    }
    async fn generate(
        &self,
        thread_id: ThreadId,
        messages: Vec<OrchestrationV2HistoricalMessage>,
        cancellation: CancellationToken,
    ) -> Result<String>;
}

#[derive(Debug, Clone)]
pub struct ForkTitleContext {
    pub title: String,
    pub history: String,
}

#[derive(Debug, Clone)]
pub struct TitleGenerationInput {
    pub thread_id: ThreadId,
    pub project_id: ProjectId,
    pub cwd: PathBuf,
    pub project_title: String,
    pub message: String,
    pub attachments: Vec<t3_contracts::chat_attachment::ChatAttachment>,
    pub previous_title: Option<String>,
    pub fork_source: Option<ForkTitleContext>,
    pub instructions: String,
    pub model_selection: t3_contracts::model_selection::ModelSelection,
}

pub use crate::ports::sessions_recovery::provider_adapter_registry::AdapterMetadata;

#[async_trait]
pub trait ProviderRegistry: Send + Sync {
    async fn adapter(&self, instance_id: ProviderInstanceId) -> Result<Arc<dyn ProviderAdapter>>;
    async fn metadata(&self, _instance_id: ProviderInstanceId) -> Result<AdapterMetadata> {
        Err(crate::error::Error::Unsupported(
            "ProviderRegistry.metadata",
        ))
    }
    async fn list(&self) -> Result<Vec<ProviderInstanceId>> {
        Err(crate::error::Error::Unsupported("ProviderRegistry.list"))
    }
}

#[async_trait]
pub trait SessionRetentionCheck: Send + Sync {
    async fn keep(&self) -> Result<bool>;
}

pub struct ReleaseSession {
    pub provider_session_id: ProviderSessionId,
    pub own_provider_thread_ids: Vec<ProviderThreadId>,
    pub through_run_ordinal: u64,
    pub reason: String,
    pub detail: Option<String>,
    pub keep_if: Option<Arc<dyn SessionRetentionCheck>>,
}

#[derive(Debug, Clone)]
pub struct DetachSession {
    pub provider_session_id: ProviderSessionId,
    pub thread_id: ThreadId,
    pub detail: Option<String>,
    pub revoke_credential: bool,
}

#[async_trait]
pub trait SessionDirectory: Send + Sync {
    /// `attach` carrying configured runtime-policy options (reasoning effort) to the adapter.
    async fn attach_with_policy(
        &self,
        instance_id: ProviderInstanceId,
        input: crate::provider::OpenSession,
        options: crate::provider::RuntimePolicyOptions,
    ) -> Result<Arc<dyn SessionRuntime>> {
        if options.reasoning_effort.is_some() {
            return Err(crate::error::Error::Unsupported(
                "SessionDirectory.attach_with_policy",
            ));
        }
        self.attach(instance_id, input).await
    }
    async fn detach_thread(&self, _input: DetachSession) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "SessionDirectory.detach_thread",
        ))
    }
    /// Implementations inspect busy turns and remove under the same gate used by turn starts.
    async fn release_unless_busy(&self, _input: ReleaseSession) -> Result<bool> {
        Err(crate::error::Error::Unsupported(
            "SessionDirectory.release_unless_busy",
        ))
    }
    async fn other_working_threads(
        &self,
        _session_id: ProviderSessionId,
        _thread_id: ThreadId,
    ) -> Result<Vec<ThreadId>> {
        Err(crate::error::Error::Unsupported(
            "SessionDirectory.other_working_threads",
        ))
    }
    async fn get(&self, session_id: ProviderSessionId) -> Result<Option<Arc<dyn SessionRuntime>>>;
    async fn attach(
        &self,
        instance_id: ProviderInstanceId,
        input: crate::provider::OpenSession,
    ) -> Result<Arc<dyn SessionRuntime>>;
    async fn detach(&self, session_id: ProviderSessionId, revoke_credential: bool) -> Result<()>;
    async fn shutdown(&self) -> Result<()>;
}
/// Live command output (TS `CommandOutputHub.append`/`finish`). Chunks bypass persistence and
/// run subscribers so a flood cannot delay domain events.
#[async_trait]
pub trait CommandOutputSink: Send + Sync {
    async fn append(&self, thread_id: ThreadId, item_id: TurnItemId, chunk: String) -> Result<()>;
    /// The command settled; its tail stays readable for late subscribers.
    async fn finish(&self, thread_id: ThreadId, item_id: TurnItemId) -> Result<()>;
}

#[async_trait]
pub trait EventIngestor: Send + Sync {
    async fn ingest_with_context(
        &self,
        session_id: ProviderSessionId,
        instance_id: ProviderInstanceId,
        thread_id: ThreadId,
        event: ProviderEvent,
        context: IngestionContext,
    ) -> Result<()> {
        if context.command_id.is_some() || context.analytics.is_some() {
            return Err(crate::error::Error::Unsupported(
                "EventIngestor.ingest_with_context",
            ));
        }
        self.ingest(session_id, instance_id, thread_id, event).await
    }
    async fn ingest(
        &self,
        session_id: ProviderSessionId,
        instance_id: ProviderInstanceId,
        thread_id: ThreadId,
        event: ProviderEvent,
    ) -> Result<()>;
}
#[async_trait]
pub trait ThreadOperations: Send + Sync {
    async fn continue_run_with_options(
        &self,
        thread_id: ThreadId,
        source_run_id: RunId,
        _options: EffectExecutionOptions,
    ) -> Result<()> {
        self.continue_run(thread_id, source_run_id).await
    }
    async fn start_with_options(
        &self,
        thread_id: ThreadId,
        run_id: RunId,
        _options: EffectExecutionOptions,
    ) -> Result<()> {
        self.start(thread_id, run_id).await
    }
    async fn start(&self, thread_id: ThreadId, run_id: RunId) -> Result<()>;
    async fn finalize(
        &self,
        thread_id: ThreadId,
        run_id: RunId,
        scope_id: CheckpointScopeId,
    ) -> Result<()>;
    async fn continue_run(&self, thread_id: ThreadId, source_run_id: RunId) -> Result<()>;
    async fn stop_delegated(&self, thread_id: ThreadId, reason: Option<String>) -> Result<()>;
    async fn generate_title(&self, thread_id: ThreadId, kind: TitleKind) -> Result<()>;
    /// Effect-driven forms: `command_id` is the effect's command, which TS uses as the
    /// receipt identity of the commands these operations dispatch.
    async fn stop_delegated_for_effect(
        &self,
        thread_id: ThreadId,
        _command_id: CommandId,
        reason: Option<String>,
    ) -> Result<()> {
        self.stop_delegated(thread_id, reason).await
    }
    async fn generate_title_for_effect(
        &self,
        thread_id: ThreadId,
        _command_id: CommandId,
        kind: TitleKind,
    ) -> Result<()> {
        self.generate_title(thread_id, kind).await
    }
    /// TS `Orchestrator.recoverDelegatedTask`: after a final failed restart continuation
    /// (`source_run_id`) and after successful checkpoint finalization (`completed_run_id`).
    async fn recover_delegated_task(
        &self,
        _thread_id: ThreadId,
        _source_run_id: Option<RunId>,
        _completed_run_id: Option<RunId>,
    ) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "ThreadOperations.recover_delegated_task",
        ))
    }
    /// Post-commit and receipt-replay continuation of `queue.resume` / `queued-run.start`.
    async fn resume_queue(
        &self,
        _thread_id: ThreadId,
        _selected_run_id: Option<RunId>,
    ) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "ThreadOperations.resume_queue",
        ))
    }
}
#[derive(Debug, Clone)]
pub enum ResourceKind {
    ScheduledTasks,
    Terminals,
    Devices,
    Previews,
    Attachments(Vec<String>),
}
#[async_trait]
pub trait ResourceCleanup: Send + Sync {
    async fn cleanup(&self, thread_id: ThreadId, kind: ResourceKind) -> Result<()>;
    /// Closes the thread's settle-time resources; not `cleanup(Terminals)`, which deletes all.
    async fn close_on_settle(&self, _thread_id: ThreadId) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "ResourceCleanup.close_on_settle",
        ))
    }
}
#[async_trait]
pub trait PullRequestBackend: Send + Sync {
    async fn detail(
        &self,
        _input: t3_contracts::pull_request::PullRequestRef,
    ) -> Result<t3_contracts::pull_request::PullRequestDetail> {
        Err(crate::error::Error::Unsupported(
            "PullRequestBackend.detail",
        ))
    }
    async fn activity(
        &self,
        _input: t3_contracts::pull_request::PullRequestRef,
    ) -> Result<t3_contracts::pull_request::PullRequestActivity> {
        Err(crate::error::Error::Unsupported(
            "PullRequestBackend.activity",
        ))
    }
    async fn thread_comments(
        &self,
        _input: t3_contracts::pull_request::PullRequestThreadCommentsInput,
    ) -> Result<t3_contracts::pull_request::PullRequestThreadCommentsResult> {
        Err(crate::error::Error::Unsupported(
            "PullRequestBackend.thread_comments",
        ))
    }
    async fn watch_fingerprint(
        &self,
        _input: t3_contracts::pull_request::PullRequestRef,
    ) -> Result<Option<PullRequestWatchFingerprint>> {
        Err(crate::error::Error::Unsupported(
            "PullRequestBackend.watch_fingerprint",
        ))
    }
    /// Without TS `recoverTransientFailure`: a transient failure is returned, not retried.
    async fn summary(
        &self,
        _input: t3_contracts::pull_request::PullRequestRef,
    ) -> Result<t3_contracts::pull_request::PullRequestSummary> {
        Err(crate::error::Error::Unsupported(
            "PullRequestBackend.summary",
        ))
    }
    async fn stack(
        &self,
        _input: t3_contracts::pull_request::PullRequestRef,
        _include_details: bool,
    ) -> Result<Option<t3_contracts::thread_pull_request::ThreadPullRequestStack>> {
        Err(crate::error::Error::Unsupported("PullRequestBackend.stack"))
    }
    /// Whether reads for this host go through its batched read pool.
    async fn batches_reads(
        &self,
        _input: t3_contracts::pull_request::PullRequestRef,
    ) -> Result<bool> {
        Err(crate::error::Error::Unsupported(
            "PullRequestBackend.batches_reads",
        ))
    }
    async fn invalidate(
        &self,
        _input: t3_contracts::pull_request::PullRequestInvalidateInput,
        _notify_readers: bool,
    ) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "PullRequestBackend.invalidate",
        ))
    }
    /// A counter that advances whenever the host learns this pull request changed.
    async fn changes(
        &self,
        _input: t3_contracts::pull_request::PullRequestRef,
    ) -> Result<tokio::sync::watch::Receiver<u64>> {
        Err(crate::error::Error::Unsupported(
            "PullRequestBackend.changes",
        ))
    }
    /// Pull requests the host observed merging.
    async fn subscribe_merges(
        &self,
    ) -> Result<broadcast::Receiver<t3_contracts::pull_request::PullRequestRef>> {
        Err(crate::error::Error::Unsupported(
            "PullRequestBackend.subscribe_merges",
        ))
    }
    async fn notify_linked_threads_changed(&self) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "PullRequestBackend.notify_linked_threads_changed",
        ))
    }
    async fn refresh(
        &self,
        cwd: PathBuf,
        links: Vec<ThreadPullRequestLink>,
    ) -> Result<Vec<ThreadPullRequestLink>>;
    async fn discover(&self, cwd: PathBuf, branch: String) -> Result<Vec<ThreadPullRequestLink>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestWatchFingerprint {
    pub status: String,
    pub remarks: String,
}
#[derive(Debug, Clone)]
pub struct Workspace {
    pub cwd: PathBuf,
    pub branch: Option<String>,
    pub worktree_path: Option<PathBuf>,
}
#[derive(Debug, Clone)]
pub struct BranchPullRequestQuery {
    pub cwd: PathBuf,
    pub branch: String,
    pub refresh: bool,
    /// Re-read a terminal (merged or closed) pull request instead of trusting the cache.
    pub confirm_terminal: bool,
    /// Read branch and repository state from Git now, not from the status cache.
    pub fresh_git_state: bool,
}

/// TS `GitBranchPullRequest`: the status pull request plus identity and terminal times.
#[derive(Debug, Clone)]
pub struct GitBranchPullRequest {
    pub pull_request: t3_contracts::git::VcsStatusChangeRequest,
    pub repository_key: Option<String>,
    pub updated_at: Option<IsoDateTime>,
    pub closed_at: Option<IsoDateTime>,
    pub merged_at: Option<IsoDateTime>,
    pub head_sha: Option<String>,
}

#[async_trait]
pub trait WorkspaceBackend: Send + Sync {
    async fn repository_identity(
        &self,
        _cwd: PathBuf,
    ) -> Result<Option<t3_contracts::environment::RepositoryIdentity>> {
        Err(crate::error::Error::Unsupported(
            "WorkspaceBackend.repository_identity",
        ))
    }
    /// `refresh` bypasses the host's cached identity; without it this is `repository_identity`.
    async fn repository_identity_with_refresh(
        &self,
        cwd: PathBuf,
        refresh: bool,
    ) -> Result<Option<t3_contracts::environment::RepositoryIdentity>> {
        if refresh {
            return Err(crate::error::Error::Unsupported(
                "WorkspaceBackend.repository_identity_with_refresh",
            ));
        }
        self.repository_identity(cwd).await
    }
    async fn branch_pull_request(
        &self,
        _query: BranchPullRequestQuery,
    ) -> Result<Option<GitBranchPullRequest>> {
        Err(crate::error::Error::Unsupported(
            "WorkspaceBackend.branch_pull_request",
        ))
    }
    async fn invalidate_status(&self, _cwd: PathBuf) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "WorkspaceBackend.invalidate_status",
        ))
    }
    async fn default_branch(&self, _cwd: PathBuf) -> Result<Option<String>> {
        Err(crate::error::Error::Unsupported(
            "WorkspaceBackend.default_branch",
        ))
    }
    async fn checked_out_pull_request(
        &self,
        _cwd: PathBuf,
        _branch: String,
        _url: String,
    ) -> Result<bool> {
        Err(crate::error::Error::Unsupported(
            "WorkspaceBackend.checked_out_pull_request",
        ))
    }
    async fn prepare(
        &self,
        project_id: ProjectId,
        thread_id: ThreadId,
        strategy: OrchestrationV2ThreadLaunchWorkspaceStrategy,
        cancellation: CancellationToken,
    ) -> Result<Workspace>;
    async fn remove(&self, workspace: Workspace, cancellation: CancellationToken) -> Result<()>;
    async fn current_branch(&self, cwd: PathBuf) -> Result<Option<String>>;
}
#[async_trait]
pub trait AttachmentBackend: Send + Sync {
    /// On failure, the backend removes partial copies; success transfers their exact paths to the caller.
    async fn claim_pending(
        &self,
        _thread_id: ThreadId,
        _attachments: Vec<t3_contracts::chat_attachment::ChatAttachment>,
    ) -> Result<ClaimedAttachments> {
        Err(crate::error::Error::Unsupported(
            "AttachmentBackend.claim_pending",
        ))
    }
    async fn release_claimed_paths(&self, _paths: Vec<PathBuf>) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "AttachmentBackend.release_claimed_paths",
        ))
    }
    async fn resolve(
        &self,
        thread_id: ThreadId,
        attachments: Vec<t3_contracts::chat_attachment::ChatAttachment>,
    ) -> Result<Vec<t3_contracts::chat_attachment::ChatAttachment>>;
    async fn claim(&self, thread_id: ThreadId, attachment_ids: Vec<String>) -> Result<()>;
    async fn release(&self, thread_id: ThreadId, attachment_ids: Vec<String>) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct ClaimedAttachments {
    pub attachments: Vec<t3_contracts::chat_attachment::ChatAttachment>,
    pub claimed_paths: Vec<PathBuf>,
}

#[async_trait]
pub trait SettingsService: Send + Sync {
    async fn get(&self) -> Result<t3_contracts::settings::ServerSettings> {
        Err(crate::error::Error::Unsupported("SettingsService.get"))
    }
    fn subscribe(
        &self,
    ) -> Result<tokio::sync::watch::Receiver<Arc<t3_contracts::settings::ServerSettings>>> {
        Err(crate::error::Error::Unsupported(
            "SettingsService.subscribe",
        ))
    }
}

#[derive(Debug, Clone)]
pub struct SettleScriptInput {
    pub thread_id: ThreadId,
    pub project_id: ProjectId,
    pub worktree_path: PathBuf,
}

pub enum SettleScriptRun {
    Skipped,
    Started {
        completion: tokio::sync::oneshot::Receiver<Result<()>>,
    },
}

#[async_trait]
pub trait SettleScriptRunner: Send + Sync {
    async fn run(
        &self,
        _input: SettleScriptInput,
        _cancellation: CancellationToken,
    ) -> Result<SettleScriptRun> {
        Err(crate::error::Error::Unsupported("SettleScriptRunner.run"))
    }
}

/// TS `ThreadWorktreeRestore`: recreates a thread's missing worktree under its workspace lease
/// before a send starts a turn.
#[async_trait]
pub trait ThreadWorktreeRestore: Send + Sync {
    async fn restore(&self, thread: OrchestrationV2AppThread) -> Result<()>;
}

/// Server-owned storage for the Git repository each checkpoint scope was bound to (t3-git H-1),
/// keyed by the scope's folder. Lives outside the checkout and outside the database schema.
#[async_trait]
pub trait RepositoryBindingStore: Send + Sync {
    async fn get(&self, scope: &std::path::Path) -> Result<Option<serde_json::Value>>;
    async fn put(&self, scope: &std::path::Path, binding: serde_json::Value) -> Result<()>;
}

/// Processes the host started for a thread that can write to its workspace: terminals, dev
/// servers, preview and setup scripts. A file restore holds them for its whole run, because Git
/// checks paths before it writes and a live writer can change them in between (t3-git H-2).
#[async_trait]
pub trait ThreadWriters: Send + Sync {
    /// Stops new writers for `thread_id` from starting until the returned hold drops, and names
    /// those still running.
    async fn hold(&self, thread_id: ThreadId, cwd: PathBuf) -> Result<WriterHold>;
}

/// The writers a host still runs for a thread, and the hold that keeps new ones from starting.
pub struct WriterHold {
    /// As a user would name each, such as "terminal `npm run dev`".
    pub running: Vec<String>,
    /// Released when dropped.
    pub guard: Box<dyn std::any::Any + Send + Sync>,
}

#[async_trait]
pub trait AgentScope: Send + Sync {
    async fn kill(&self, _thread_id: ThreadId) -> Result<bool> {
        Err(crate::error::Error::Unsupported("AgentScope.kill"))
    }
    async fn oom_killed(&self, _thread_id: ThreadId) -> Result<bool> {
        Err(crate::error::Error::Unsupported("AgentScope.oom_killed"))
    }
}

#[derive(Debug, Clone)]
pub struct ProviderTurnAnalyticsContext {
    pub run_id: Option<RunId>,
    pub model_selection: t3_contracts::model_selection::ModelSelection,
    pub runtime_mode: Option<t3_contracts::provider_policy::RuntimeMode>,
    pub interaction_mode: Option<t3_contracts::provider_policy::ProviderInteractionMode>,
    pub origin: Option<ProviderTurnOrigin>,
    pub segment: Option<ProviderTurnSegment>,
    pub restart_continuation: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct ProviderTurnOrigin {
    pub created_by: OrchestrationV2Actor,
    pub creation_source: OrchestrationV2CreationSource,
    pub scheduled: bool,
    pub subagent: bool,
}

#[derive(Debug, Clone)]
pub enum ProviderTurnSegment {
    Attempt(RunAttemptReason),
    Continuation,
    NativeChild,
}

#[derive(Debug, Clone)]
pub struct CompletedProviderTurn {
    pub driver: t3_contracts::provider_instance::ProviderDriverKind,
    pub provider_turn: OrchestrationV2ProviderTurn,
    pub context: Option<ProviderTurnAnalyticsContext>,
}

#[derive(Debug, Clone, Default)]
pub struct IngestionContext {
    pub command_id: Option<CommandId>,
    pub analytics: Option<ProviderTurnAnalyticsContext>,
}

#[async_trait]
pub trait ProviderTelemetry: Send + Sync {
    async fn refresh_usage(
        &self,
        _instance_id: ProviderInstanceId,
        _cancellation: CancellationToken,
    ) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "ProviderTelemetry.refresh_usage",
        ))
    }
    async fn turn_completed(
        &self,
        _input: CompletedProviderTurn,
        _cancellation: CancellationToken,
    ) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "ProviderTelemetry.turn_completed",
        ))
    }
}

#[derive(Debug, Clone)]
pub struct ProviderControlTarget {
    pub thread_id: ThreadId,
    pub provider_session_id: ProviderSessionId,
    pub provider_thread_id: ProviderThreadId,
    pub provider_turn_id: ProviderTurnId,
}

#[derive(Debug, Clone)]
pub struct InterruptTurn {
    pub target: ProviderControlTarget,
    pub subagent: Option<InterruptSubagent>,
    pub requested_at: Option<IsoDateTime>,
}

#[derive(Debug, Clone)]
pub struct RestartTurn {
    pub target: ProviderControlTarget,
    pub replacement_provider_session_id: Option<ProviderSessionId>,
    pub interrupted_attempt_id: RunAttemptId,
}

#[derive(Debug, Clone)]
pub struct SteerTurn {
    pub target: ProviderControlTarget,
    pub message_id: MessageId,
    pub model_selection: Option<t3_contracts::model_selection::ModelSelection>,
}

#[derive(Debug, Clone)]
pub struct ApplyModelSelection {
    pub thread_id: ThreadId,
    pub provider_session_id: ProviderSessionId,
    pub provider_thread_id: ProviderThreadId,
    pub model_selection: t3_contracts::model_selection::ModelSelection,
}

#[async_trait]
pub trait ProviderTurnControl: Send + Sync {
    async fn interrupt(
        &self,
        _input: InterruptTurn,
        _cancellation: CancellationToken,
    ) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "ProviderTurnControl.interrupt",
        ))
    }
    /// Like `interrupt`, returning the subagent outcome the provider confirmed, if any.
    async fn interrupt_with_outcome(
        &self,
        input: InterruptTurn,
        cancellation: CancellationToken,
    ) -> Result<Option<crate::provider::SubagentOutcome>> {
        self.interrupt(input, cancellation).await?;
        Ok(None)
    }
    async fn interrupt_and_await_terminal(
        &self,
        _input: RestartTurn,
        _cancellation: CancellationToken,
    ) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "ProviderTurnControl.interrupt_and_await_terminal",
        ))
    }
    async fn steer(&self, _input: SteerTurn, _cancellation: CancellationToken) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "ProviderTurnControl.steer",
        ))
    }
    async fn apply_selection(
        &self,
        _input: ApplyModelSelection,
        _cancellation: CancellationToken,
    ) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "ProviderTurnControl.apply_selection",
        ))
    }
}

pub use crate::ports::checkpoints_forks::checkpoint_rollback_service::RollbackRequest;

#[async_trait]
pub trait CheckpointRollback: Send + Sync {
    async fn rollback(
        &self,
        _input: RollbackRequest,
        _cancellation: CancellationToken,
    ) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "CheckpointRollback.rollback",
        ))
    }
}

#[async_trait]
pub trait RuntimeRequestResponder: Send + Sync {
    async fn respond(
        &self,
        _thread_id: ThreadId,
        _session_id: ProviderSessionId,
        _response: crate::provider::RuntimeResponse,
        _cancellation: CancellationToken,
    ) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "RuntimeRequestResponder.respond",
        ))
    }
}

#[derive(Debug, Clone, Copy)]
pub enum TimelineView {
    Messages,
    Activity,
}
