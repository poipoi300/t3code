use async_trait::async_trait;
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};
use t3_contracts::{
    base_schemas::*,
    chat_attachment::ChatAttachment,
    model_selection::ModelSelection,
    orchestration_v2::*,
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
    provider_policy::{
        ProviderApprovalDecision, ProviderInteractionMode, ProviderUserInputAnswers, RuntimeMode,
    },
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub type ProviderResult<T> = std::result::Result<T, ProviderError>;
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider setup {operation}: {detail}", operation = .0.operation, detail = .0.detail)]
    Setup(Box<t3_contracts::provider_setup::ProviderSetupError>),
    #[error("unsupported provider operation: {0}")]
    Unsupported(&'static str),
    #[error("provider session stopped")]
    SessionStopped,
    #[error("native thread already has a writer: {0:?}")]
    ThreadInUse(ProviderThreadId),
    #[error("steer declined: {0:?}")]
    SteerDeclined(ProviderTurnId),
    #[error("interrupt timed out after {timeout_ms} ms for {turn_id:?}")]
    InterruptTimeout {
        turn_id: ProviderTurnId,
        timeout_ms: u64,
    },
    #[error("sign in required")]
    RequiresSignIn,
    #[error("protocol failure: {reason:?}")]
    Protocol {
        reason: Option<ProtocolReason>,
        detail: String,
        payload: Option<Value>,
    },
    #[error("provider {operation:?}: {detail}")]
    Operation {
        operation: ProviderOperation,
        detail: String,
    },
    #[error("provider subscription closed")]
    SubscriptionClosed,
    /// The session's event stream ended in failure (TS `ProviderAdapterEventStreamError`).
    /// `detached_thread_id` is set when T3 closed the session because that thread detached
    /// from it, not because the provider failed.
    #[error("provider event stream for {provider_session_id:?} failed: {detail}")]
    EventStream {
        provider_session_id: ProviderSessionId,
        detached_thread_id: Option<ThreadId>,
        detail: String,
    },
    #[error("provider operation cancelled")]
    Cancelled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolReason {
    PreviousTurnActive,
    AttachmentUnreadable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderOperation {
    Capabilities,
    Open,
    Close,
    Resume,
    Ensure,
    Snapshot,
    Rollback,
    Fork,
    Start,
    Steer,
    Interrupt,
    Respond,
    Stream,
    Compact,
}

#[derive(Debug, Clone)]
pub struct RuntimePolicy {
    pub runtime_mode: RuntimeMode,
    pub interaction_mode: ProviderInteractionMode,
    pub cwd: Option<PathBuf>,
    pub approval_policy: Option<Value>,
    pub sandbox_policy: Option<Value>,
    pub max_runtime_mode: Option<RuntimeMode>,
}

#[derive(Debug, Clone, Default)]
pub struct RuntimePolicyOptions {
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConfiguredRuntimePolicy {
    pub policy: RuntimePolicy,
    pub options: RuntimePolicyOptions,
}

impl RuntimePolicy {
    pub fn with_reasoning_effort(
        self,
        reasoning_effort: Option<String>,
    ) -> ConfiguredRuntimePolicy {
        ConfiguredRuntimePolicy {
            policy: self,
            options: RuntimePolicyOptions { reasoning_effort },
        }
    }
}
#[derive(Clone)]
pub struct OpenSession {
    pub thread_id: ThreadId,
    pub provider_session_id: ProviderSessionId,
    pub model_selection: ModelSelection,
    pub runtime_policy: RuntimePolicy,
    pub prompt_suggestions: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    pub resume_from_session: Option<OrchestrationV2ProviderSession>,
    pub initial_native_thread_id: Option<String>,
    pub initial_provider_item_identity_version: Option<u8>,
}
#[derive(Debug, Clone)]
pub struct EnsureThread {
    pub thread_id: ThreadId,
    pub title: Option<String>,
    pub model_selection: ModelSelection,
    pub runtime_policy: RuntimePolicy,
    pub provider_session_id: Option<ProviderSessionId>,
    pub existing_provider_thread: Option<OrchestrationV2ProviderThread>,
    pub force_new_native_thread: bool,
}
#[derive(Debug, Clone)]
pub struct ResumeThread {
    pub provider_thread: OrchestrationV2ProviderThread,
    pub thread_id: Option<ThreadId>,
    pub model_selection: Option<ModelSelection>,
    pub runtime_policy: Option<RuntimePolicy>,
}
#[derive(Debug, Clone)]
pub struct TurnMessage {
    pub message_id: MessageId,
    pub text: String,
    pub attachments: Vec<ChatAttachment>,
    pub created_by: OrchestrationV2Actor,
    pub creation_source: Option<OrchestrationV2CreationSource>,
    pub scheduled_task_id: Option<ScheduledTaskId>,
    pub sender_thread_id: Option<ThreadId>,
}
#[derive(Debug, Clone)]
pub struct TurnInput {
    pub app_thread: OrchestrationV2AppThread,
    pub thread_id: ThreadId,
    pub run_id: RunId,
    pub run_ordinal: u64,
    pub native_thread_has_turns: Option<bool>,
    pub provider_turn_ordinal: u64,
    pub restart_continuation_of_run_id: Option<RunId>,
    pub attempt_id: RunAttemptId,
    pub root_node_id: NodeId,
    pub provider_thread: OrchestrationV2ProviderThread,
    pub message: TurnMessage,
    pub model_selection: ModelSelection,
    pub runtime_policy: RuntimePolicy,
    pub app_context: Vec<(String, String)>,
    /// Leading characters of `message.text` that are inline handoff context, not the
    /// user's words (TS `inlineContextLength`).
    pub inline_context_length: Option<usize>,
}
#[derive(Debug, Clone)]
pub struct SteerInput {
    pub thread_id: ThreadId,
    pub run_id: RunId,
    pub provider_thread: OrchestrationV2ProviderThread,
    pub provider_turn_id: ProviderTurnId,
    pub message: TurnMessage,
    pub model_selection: Option<ModelSelection>,
}
#[derive(Debug, Clone)]
pub struct InterruptInput {
    pub provider_thread: OrchestrationV2ProviderThread,
    pub provider_turn_id: ProviderTurnId,
    pub request_runtime_restart: bool,
}
#[derive(Debug, Clone)]
pub struct RuntimeResponse {
    pub request_id: RuntimeRequestId,
    pub decision: Option<ProviderApprovalDecision>,
    pub feedback: Option<String>,
    pub answers: Option<ProviderUserInputAnswers>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadSnapshot {
    pub provider_thread: OrchestrationV2ProviderThread,
    pub provider_turns: Vec<OrchestrationV2ProviderTurn>,
    pub messages: Vec<OrchestrationV2ConversationMessage>,
    pub reasoning_message_ids: Option<Vec<MessageId>>,
    pub runtime_requests: Vec<OrchestrationV2RuntimeRequest>,
    pub provider_payload: Option<Value>,
}
#[derive(Debug, Clone)]
pub enum RollbackTarget {
    ThreadStart {
        checkpoint_id: CheckpointId,
    },
    ProviderTurn {
        checkpoint_id: CheckpointId,
        app_run_ordinal: u64,
        provider_turn: Box<OrchestrationV2ProviderTurn>,
        message: Option<TurnMessage>,
    },
}
#[derive(Debug, Clone)]
pub struct RollbackInput {
    pub provider_thread: OrchestrationV2ProviderThread,
    pub target: RollbackTarget,
    pub provider_thread_turns: Vec<OrchestrationV2ProviderTurn>,
}
#[derive(Debug, Clone)]
pub struct ForkInput {
    pub source_provider_thread: OrchestrationV2ProviderThread,
    pub source_provider_turns: Option<Vec<OrchestrationV2ProviderTurn>>,
    pub provider_turn_id: Option<ProviderTurnId>,
    pub target_thread_id: ThreadId,
    pub owner_node_id: Option<NodeId>,
    pub model_selection: Option<ModelSelection>,
    pub runtime_policy: Option<RuntimePolicy>,
}
#[derive(Debug, Clone)]
pub struct SelectionTransitionInput {
    pub current: ModelSelection,
    pub target: ModelSelection,
    pub session_capabilities: OrchestrationV2ProviderCapabilities,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionTransitionPlan {
    ApplyOnNextTurn,
    RestartSession,
    CreateWithHandoff,
    Reject { reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderEvent {
    pub driver: ProviderDriverKind,
    pub kind: ProviderEventKind,
}
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderEventKind {
    AppThread {
        app_thread: Box<OrchestrationV2AppThread>,
        create_only: Option<bool>,
    },
    Session(Box<OrchestrationV2ProviderSession>),
    Thread {
        provider_thread: Box<OrchestrationV2ProviderThread>,
        pending_background_work: Option<bool>,
        session_scoped: Option<bool>,
        retained_native_turn_ids: Option<Vec<String>>,
    },
    Turn {
        provider_turn: Box<OrchestrationV2ProviderTurn>,
        thread_id: Option<ThreadId>,
    },
    Node(Box<OrchestrationV2ExecutionNode>),
    Subagent(Box<OrchestrationV2Subagent>),
    Message(Box<OrchestrationV2ConversationMessage>),
    TurnItem {
        item: Box<OrchestrationV2TurnItem>,
        session_scoped: Option<bool>,
    },
    Steer {
        thread_id: ThreadId,
        run_id: RunId,
        message_id: MessageId,
        status: SteerStatus,
    },
    BackgroundWork {
        provider_turn_id: ProviderTurnId,
        pending: bool,
    },
    CommandOutput {
        thread_id: ThreadId,
        item_id: TurnItemId,
        chunk: String,
    },
    PromptSuggestion {
        thread_id: ThreadId,
        run_id: RunId,
        text: String,
    },
    RuntimeRequest {
        request: Box<OrchestrationV2RuntimeRequest>,
        thread_id: Option<ThreadId>,
    },
    Plan(Box<OrchestrationV2PlanArtifact>),
    Terminal {
        provider_thread_id: ProviderThreadId,
        provider_turn_id: ProviderTurnId,
        run_ordinal: u64,
        disposition: ThreadDisposition,
        outcome: TerminalOutcome,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteerStatus {
    Pending,
    Completed,
    Cancelled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadDisposition {
    Reusable,
    Broken,
}
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalOutcome {
    Completed {
        reason: Option<String>,
    },
    Interrupted {
        reason: Option<String>,
    },
    Cancelled {
        reason: Option<String>,
    },
    Failed {
        failure: Box<OrchestrationV2ProviderFailure>,
        failure_item_ordinal: u64,
        retry: Option<OrchestrationV2ProviderRetry>,
        retry_started_at: Option<IsoDateTime>,
        child_turns_continue: bool,
    },
}

pub struct EventSubscription {
    pub events: mpsc::Receiver<ProviderResult<ProviderEvent>>,
    pub cancellation: CancellationToken,
}
impl EventSubscription {
    pub fn close(&mut self) {
        self.cancellation.cancel();
        self.events.close();
    }
}
impl Drop for EventSubscription {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    fn instance_id(&self) -> &ProviderInstanceId;
    fn driver(&self) -> ProviderDriverKind;
    async fn capabilities(&self) -> ProviderResult<OrchestrationV2ProviderCapabilities>;
    async fn plan_selection_transition(
        &self,
        input: SelectionTransitionInput,
    ) -> ProviderResult<SelectionTransitionPlan>;
    async fn open_session(
        &self,
        input: OpenSession,
        cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>>;

    async fn open_session_with_policy(
        &self,
        input: OpenSession,
        options: RuntimePolicyOptions,
        cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        if options.reasoning_effort.is_some() {
            return Err(ProviderError::Unsupported("open_session_with_policy"));
        }
        self.open_session(input, cancellation).await
    }
}
#[async_trait]
pub trait SessionRuntime: Send + Sync {
    async fn ensure_thread_with_policy(
        &self,
        input: EnsureThread,
        options: RuntimePolicyOptions,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        if options.reasoning_effort.is_some() {
            return Err(ProviderError::Unsupported("ensure_thread_with_policy"));
        }
        self.ensure_thread(input).await
    }
    async fn resume_thread_with_policy(
        &self,
        input: ResumeThread,
        options: RuntimePolicyOptions,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        if options.reasoning_effort.is_some() {
            return Err(ProviderError::Unsupported("resume_thread_with_policy"));
        }
        self.resume_thread(input).await
    }
    async fn start_turn_with_policy(
        &self,
        input: TurnInput,
        options: RuntimePolicyOptions,
    ) -> ProviderResult<()> {
        if options.reasoning_effort.is_some() {
            return Err(ProviderError::Unsupported("start_turn_with_policy"));
        }
        self.start_turn(input).await
    }
    async fn fork_with_policy(
        &self,
        input: ForkInput,
        options: RuntimePolicyOptions,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        if options.reasoning_effort.is_some() {
            return Err(ProviderError::Unsupported("fork_with_policy"));
        }
        self.fork(input).await
    }
    fn session(&self) -> &OrchestrationV2ProviderSession;
    fn can_resume_after_restart(&self) -> bool;
    fn background_tasks_end_with_process(&self) -> bool;
    async fn subscribe(&self) -> ProviderResult<EventSubscription>;
    async fn close(&self) -> ProviderResult<()>;
    async fn ensure_thread(
        &self,
        input: EnsureThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread>;
    async fn resume_thread(
        &self,
        input: ResumeThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread>;
    async fn start_turn(&self, input: TurnInput) -> ProviderResult<()>;
    async fn steer_turn(&self, input: SteerInput) -> ProviderResult<()>;
    async fn interrupt_turn(&self, input: InterruptInput) -> ProviderResult<()>;
    async fn respond(&self, input: RuntimeResponse) -> ProviderResult<()>;
    async fn snapshot(
        &self,
        thread: OrchestrationV2ProviderThread,
    ) -> ProviderResult<ThreadSnapshot>;
    async fn rollback(&self, input: RollbackInput) -> ProviderResult<ThreadSnapshot>;
    async fn fork(&self, input: ForkInput) -> ProviderResult<OrchestrationV2ProviderThread>;
    fn extensions(&self) -> Option<&dyn SessionExtensions> {
        None
    }
}
#[derive(Debug, Clone, Default)]
pub struct ExtensionCapabilities {
    pub inject_history: bool,
    pub compact: bool,
    pub apply_selection: bool,
    pub stop_task: bool,
    pub await_tasks: bool,
    pub stop_subagent: bool,
    pub unload: bool,
    pub feedback: bool,
    pub mcp_apps: bool,
}
#[derive(Debug, Clone)]
pub struct SubagentOutcome {
    pub status: TerminalTaskStatus,
    pub completed_at: IsoDateTime,
    pub result: Option<String>,
}
#[derive(Debug, Clone, Copy)]
pub enum TerminalTaskStatus {
    Completed,
    Failed,
    Cancelled,
}
#[derive(Debug, Clone)]
pub struct McpTool {
    pub name: String,
    pub input_schema: Value,
    pub meta: Option<Value>,
    pub annotations: Option<Value>,
}
#[derive(Debug, Clone)]
pub struct McpResult {
    pub content: Vec<Value>,
    pub structured_content: Option<Value>,
    pub is_error: Option<bool>,
    pub meta: Option<Value>,
}
#[async_trait]
pub trait SessionExtensions: Send + Sync {
    fn capabilities(&self) -> ExtensionCapabilities;
    async fn pending_background_work(
        &self,
        thread: Option<OrchestrationV2ProviderThread>,
        include_subagents: bool,
    ) -> ProviderResult<bool>;
    async fn model_context_window(
        &self,
        selection: ModelSelection,
        cwd: Option<PathBuf>,
    ) -> ProviderResult<Option<u64>>;
    fn can_reuse_context_usage(&self, previous: &ModelSelection, next: &ModelSelection) -> bool;
    fn normalize_thread_load_selection(&self, selection: ModelSelection) -> ModelSelection;
    async fn inject_history(
        &self,
        thread: OrchestrationV2ProviderThread,
        messages: Vec<OrchestrationV2HistoricalMessage>,
        context: String,
    ) -> ProviderResult<bool>;
    async fn compact(&self, input: TurnInput) -> ProviderResult<()>;
    async fn apply_selection(
        &self,
        thread: OrchestrationV2ProviderThread,
        selection: ModelSelection,
    ) -> ProviderResult<()>;
    async fn stop_task(
        &self,
        thread: OrchestrationV2ProviderThread,
        task_id: String,
    ) -> ProviderResult<()>;
    async fn await_tasks(
        &self,
        thread: OrchestrationV2ProviderThread,
        task_ids: Vec<String>,
        awaited: bool,
    ) -> ProviderResult<Vec<String>>;
    async fn stop_subagent(
        &self,
        thread: OrchestrationV2ProviderThread,
        native_task_id: String,
        requested_at: Option<IsoDateTime>,
    ) -> ProviderResult<Option<SubagentOutcome>>;
    async fn unload(
        &self,
        thread: OrchestrationV2ProviderThread,
        archive: bool,
    ) -> ProviderResult<()>;
    async fn upload_feedback(
        &self,
        thread: OrchestrationV2ProviderThread,
        reason: Option<String>,
    ) -> ProviderResult<String>;
    async fn list_tools(
        &self,
        thread: OrchestrationV2ProviderThread,
        server: String,
    ) -> ProviderResult<Vec<McpTool>>;
    async fn call_tool(
        &self,
        thread: OrchestrationV2ProviderThread,
        server: String,
        tool: String,
        arguments: Value,
    ) -> ProviderResult<McpResult>;
    async fn read_resource(
        &self,
        thread: OrchestrationV2ProviderThread,
        server: String,
        uri: String,
    ) -> ProviderResult<Vec<Value>>;
}
