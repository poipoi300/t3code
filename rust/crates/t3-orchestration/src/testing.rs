pub mod replay;

use crate::{api::*, effects::*, error::Result, provider::*, stores::StoreDatabase};
use async_trait::async_trait;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use t3_contracts::{
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
    thread_pull_request::ThreadPullRequestLink,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub struct TestDatabase {
    pub stores: StoreDatabase,
    pub directory: Option<tempfile::TempDir>,
}
impl TestDatabase {
    pub fn memory() -> Result<Self> {
        Ok(Self {
            stores: StoreDatabase(t3_persistence::Database::open(":memory:")?),
            directory: None,
        })
    }
    pub fn temporary() -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let database = t3_persistence::Database::open(directory.path().join("statev2.sqlite"))?;
        Ok(Self {
            stores: StoreDatabase(database),
            directory: Some(directory),
        })
    }
}
pub struct ManualClock(Mutex<IsoDateTime>);
impl ManualClock {
    pub fn new(now: &str) -> Self {
        Self(Mutex::new(t3_contracts::wire::WireDateTime(now.to_owned())))
    }
    pub fn set(&self, now: &str) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) =
            t3_contracts::wire::WireDateTime(now.to_owned());
    }
}
impl Clock for ManualClock {
    fn now(&self) -> IsoDateTime {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}
pub struct SequenceIds {
    prefix: String,
    counter: AtomicU64,
}
impl SequenceIds {
    pub fn new(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_owned(),
            counter: AtomicU64::new(0),
        }
    }
}
impl IdGenerator for SequenceIds {
    fn next(&self, kind: IdKind) -> String {
        format!(
            "{}:{kind:?}:{}",
            self.prefix,
            self.counter.fetch_add(1, Ordering::Relaxed)
        )
    }
}

#[derive(Debug, Clone)]
pub enum FakeCall {
    Open(ProviderSessionId),
    Close,
    Subscribe,
    Ensure(Box<EnsureThread>),
    Resume(Box<ResumeThread>),
    Start(Box<TurnInput>),
    Steer(Box<SteerInput>),
    Interrupt(Box<InterruptInput>),
    Respond(RuntimeResponse),
    Snapshot(ProviderThreadId),
    Rollback(Box<RollbackInput>),
    Fork(Box<ForkInput>),
}

pub struct FakeProviderAdapter {
    instance_id: ProviderInstanceId,
    driver: ProviderDriverKind,
    capabilities: OrchestrationV2ProviderCapabilities,
    pub runtime: Arc<FakeSessionRuntime>,
    pub open_session_with_policy:
        Script<(OpenSession, RuntimePolicyOptions, CancellationToken), Arc<dyn SessionRuntime>>,
}
pub struct FakeSessionRuntime {
    pub ensure_thread_with_policy:
        Script<(EnsureThread, RuntimePolicyOptions), OrchestrationV2ProviderThread>,
    pub resume_thread_with_policy:
        Script<(ResumeThread, RuntimePolicyOptions), OrchestrationV2ProviderThread>,
    pub start_turn_with_policy: Script<(TurnInput, RuntimePolicyOptions), ()>,
    pub fork_with_policy: Script<(ForkInput, RuntimePolicyOptions), OrchestrationV2ProviderThread>,
    session: OrchestrationV2ProviderSession,
    snapshot: Mutex<ThreadSnapshot>,
    subscribers: Mutex<
        Vec<(
            mpsc::Sender<ProviderResult<ProviderEvent>>,
            CancellationToken,
        )>,
    >,
    calls: mpsc::UnboundedSender<FakeCall>,
    cancellation: CancellationToken,
    /// `Some` gives the session extensions whose `pending_background_work` answers with it.
    pub background_work: Mutex<Option<std::result::Result<bool, String>>>,
    /// What `leaves_no_background_work` declares (true: the fake starts nothing of its own).
    pub leaves_no_background_work: std::sync::atomic::AtomicBool,
    /// When set, `rollback` signals the sender on entry and returns once the receiver resolves.
    #[allow(clippy::type_complexity)]
    pub rollback_hold: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
}
impl FakeProviderAdapter {
    pub fn new(
        instance_id: ProviderInstanceId,
        driver: ProviderDriverKind,
        capabilities: OrchestrationV2ProviderCapabilities,
        session: OrchestrationV2ProviderSession,
        snapshot: ThreadSnapshot,
    ) -> (Self, mpsc::UnboundedReceiver<FakeCall>) {
        let (calls, receiver) = mpsc::unbounded_channel();
        (
            Self {
                instance_id,
                driver,
                capabilities,
                open_session_with_policy: Script::default(),
                runtime: Arc::new(FakeSessionRuntime {
                    ensure_thread_with_policy: Script::default(),
                    resume_thread_with_policy: Script::default(),
                    start_turn_with_policy: Script::default(),
                    fork_with_policy: Script::default(),
                    session,
                    snapshot: Mutex::new(snapshot),
                    subscribers: Mutex::new(Vec::new()),
                    calls,
                    cancellation: CancellationToken::new(),
                    background_work: Mutex::new(None),
                    leaves_no_background_work: std::sync::atomic::AtomicBool::new(true),
                    rollback_hold: Mutex::new(None),
                }),
            },
            receiver,
        )
    }
}
impl FakeSessionRuntime {
    fn record(&self, call: FakeCall) -> ProviderResult<()> {
        self.calls
            .send(call)
            .map_err(|_| ProviderError::SubscriptionClosed)
    }
    pub fn set_snapshot(&self, snapshot: ThreadSnapshot) {
        *self.snapshot.lock().unwrap_or_else(|e| e.into_inner()) = snapshot;
    }
    fn read_snapshot(&self) -> ThreadSnapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub async fn emit(&self, event: ProviderEvent) -> ProviderResult<()> {
        let subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for (sender, cancellation) in subscribers {
            tokio::select! {
                _ = self.cancellation.cancelled() => return Err(ProviderError::SessionStopped),
                _ = cancellation.cancelled() => {},
                result = sender.send(Ok(event.clone())) => { if result.is_err() { continue; } }
            }
        }
        Ok(())
    }
}
#[async_trait]
impl ProviderAdapter for FakeProviderAdapter {
    fn supported_runtime_modes(&self) -> Option<Vec<t3_contracts::provider_policy::RuntimeMode>> {
        None
    }
    async fn open_session_with_policy(
        &self,
        input: OpenSession,
        options: RuntimePolicyOptions,
        cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        self.open_session_with_policy
            .call((input, options, cancellation))
            .map_err(provider_script_error)
    }
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.instance_id
    }
    fn driver(&self) -> ProviderDriverKind {
        self.driver.clone()
    }
    async fn capabilities(&self) -> ProviderResult<OrchestrationV2ProviderCapabilities> {
        Ok(self.capabilities.clone())
    }
    async fn plan_selection_transition(
        &self,
        _input: SelectionTransitionInput,
    ) -> ProviderResult<SelectionTransitionPlan> {
        Ok(SelectionTransitionPlan::ApplyOnNextTurn)
    }
    async fn open_session(
        &self,
        input: OpenSession,
        cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        if cancellation.is_cancelled() || self.runtime.cancellation.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        if t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(input.provider_session_id.clone())
            != self.runtime.session.id
        {
            return Err(ProviderError::Operation {
                operation: ProviderOperation::Open,
                detail: "fake has a different session id".into(),
            });
        }
        self.runtime
            .record(FakeCall::Open(input.provider_session_id))?;
        Ok(self.runtime.clone())
    }
}
#[async_trait]
impl SessionRuntime for FakeSessionRuntime {
    async fn ensure_thread_with_policy(
        &self,
        input: EnsureThread,
        options: RuntimePolicyOptions,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.ensure_thread_with_policy
            .call((input, options))
            .map_err(provider_script_error)
    }
    async fn resume_thread_with_policy(
        &self,
        input: ResumeThread,
        options: RuntimePolicyOptions,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.resume_thread_with_policy
            .call((input, options))
            .map_err(provider_script_error)
    }
    async fn start_turn_with_policy(
        &self,
        input: TurnInput,
        options: RuntimePolicyOptions,
    ) -> ProviderResult<()> {
        self.start_turn_with_policy
            .call((input, options))
            .map_err(provider_script_error)
    }
    async fn fork_with_policy(
        &self,
        input: ForkInput,
        options: RuntimePolicyOptions,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.fork_with_policy
            .call((input, options))
            .map_err(provider_script_error)
    }
    fn session(&self) -> &OrchestrationV2ProviderSession {
        &self.session
    }
    fn can_resume_after_restart(&self) -> bool {
        true
    }
    fn background_tasks_end_with_process(&self) -> bool {
        true
    }
    fn leaves_no_background_work(&self) -> bool {
        self.leaves_no_background_work.load(Ordering::SeqCst)
    }
    fn extensions(&self) -> Option<&dyn SessionExtensions> {
        self.background_work
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
            .then_some(self as &dyn SessionExtensions)
    }
    async fn subscribe(&self) -> ProviderResult<EventSubscription> {
        self.record(FakeCall::Subscribe)?;
        if self.cancellation.is_cancelled() {
            return Err(ProviderError::SessionStopped);
        }
        let (sender, events) = mpsc::channel(64);
        let cancellation = self.cancellation.child_token();
        self.subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((sender, cancellation.clone()));
        Ok(EventSubscription {
            events,
            cancellation,
        })
    }
    async fn close(&self) -> ProviderResult<()> {
        self.cancellation.cancel();
        self.subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.record(FakeCall::Close)
    }
    async fn ensure_thread(
        &self,
        input: EnsureThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.record(FakeCall::Ensure(Box::new(input)))?;
        Ok(self.read_snapshot().provider_thread)
    }
    async fn resume_thread(
        &self,
        input: ResumeThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.record(FakeCall::Resume(Box::new(input)))?;
        Ok(self.read_snapshot().provider_thread)
    }
    async fn start_turn(&self, input: TurnInput) -> ProviderResult<()> {
        self.record(FakeCall::Start(Box::new(input)))
    }
    async fn steer_turn(&self, input: SteerInput) -> ProviderResult<()> {
        self.record(FakeCall::Steer(Box::new(input)))
    }
    async fn interrupt_turn(&self, input: InterruptInput) -> ProviderResult<()> {
        self.record(FakeCall::Interrupt(Box::new(input)))
    }
    async fn respond(&self, input: RuntimeResponse) -> ProviderResult<()> {
        self.record(FakeCall::Respond(input))
    }
    async fn snapshot(
        &self,
        thread: OrchestrationV2ProviderThread,
    ) -> ProviderResult<ThreadSnapshot> {
        self.record(FakeCall::Snapshot(match thread.id {
            t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(id) => id,
            _ => {
                return Err(ProviderError::Operation {
                    operation: ProviderOperation::Snapshot,
                    detail: "fake requires expanded IDs".into(),
                });
            }
        }))?;
        Ok(self.read_snapshot())
    }
    async fn rollback(&self, input: RollbackInput) -> ProviderResult<ThreadSnapshot> {
        self.record(FakeCall::Rollback(Box::new(input)))?;
        let hold = self
            .rollback_hold
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some((entered, release)) = hold {
            let _ = entered.send(());
            let _ = release.await;
        }
        Ok(self.read_snapshot())
    }
    async fn fork(&self, input: ForkInput) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.record(FakeCall::Fork(Box::new(input)))?;
        Ok(self.read_snapshot().provider_thread)
    }
}

fn unused_extension<T>() -> ProviderResult<T> {
    Err(ProviderError::Unsupported("fake session extension"))
}

/// Only `pending_background_work` is scripted (by `background_work`); the rest is unused.
#[async_trait]
impl SessionExtensions for FakeSessionRuntime {
    fn capabilities(&self) -> ExtensionCapabilities {
        ExtensionCapabilities::default()
    }
    async fn pending_background_work(
        &self,
        _thread: Option<OrchestrationV2ProviderThread>,
        _include_subagents: bool,
    ) -> ProviderResult<bool> {
        match self
            .background_work
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            Some(Ok(pending)) => Ok(pending),
            Some(Err(detail)) => Err(ProviderError::Operation {
                operation: ProviderOperation::Snapshot,
                detail,
            }),
            None => unused_extension(),
        }
    }
    async fn model_context_window(
        &self,
        _selection: t3_contracts::model_selection::ModelSelection,
        _cwd: Option<PathBuf>,
    ) -> ProviderResult<Option<u64>> {
        unused_extension()
    }
    fn can_reuse_context_usage(
        &self,
        _previous: &t3_contracts::model_selection::ModelSelection,
        _next: &t3_contracts::model_selection::ModelSelection,
    ) -> bool {
        false
    }
    fn normalize_thread_load_selection(
        &self,
        selection: t3_contracts::model_selection::ModelSelection,
    ) -> t3_contracts::model_selection::ModelSelection {
        selection
    }
    async fn inject_history(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _messages: Vec<OrchestrationV2HistoricalMessage>,
        _context: String,
    ) -> ProviderResult<bool> {
        unused_extension()
    }
    async fn compact(&self, _input: TurnInput) -> ProviderResult<()> {
        unused_extension()
    }
    async fn apply_selection(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _selection: t3_contracts::model_selection::ModelSelection,
    ) -> ProviderResult<()> {
        unused_extension()
    }
    async fn stop_task(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _task_id: String,
    ) -> ProviderResult<()> {
        unused_extension()
    }
    async fn await_tasks(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _task_ids: Vec<String>,
        _awaited: bool,
    ) -> ProviderResult<Vec<String>> {
        unused_extension()
    }
    async fn stop_subagent(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _native_task_id: String,
        _requested_at: Option<IsoDateTime>,
    ) -> ProviderResult<Option<SubagentOutcome>> {
        unused_extension()
    }
    async fn unload(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _archive: bool,
    ) -> ProviderResult<()> {
        unused_extension()
    }
    async fn upload_feedback(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _reason: Option<String>,
    ) -> ProviderResult<String> {
        unused_extension()
    }
    async fn list_tools(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _server: String,
    ) -> ProviderResult<Vec<McpTool>> {
        unused_extension()
    }
    async fn call_tool(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _server: String,
        _tool: String,
        _arguments: serde_json::Value,
    ) -> ProviderResult<McpResult> {
        unused_extension()
    }
    async fn read_resource(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _server: String,
        _uri: String,
    ) -> ProviderResult<Vec<serde_json::Value>> {
        unused_extension()
    }
}

pub const TEST_TIME: &str = "2026-09-03T00:00:00.000Z";

fn provider_script_error(error: crate::error::Error) -> ProviderError {
    match error {
        crate::error::Error::Provider(error) => error,
        crate::error::Error::Unsupported(operation) => ProviderError::Unsupported(operation),
        error => ProviderError::Operation {
            operation: ProviderOperation::Open,
            detail: error.to_string(),
        },
    }
}
pub fn detached_event(id: &str, thread_id: &str) -> Result<OrchestrationV2DomainEvent> {
    Ok(serde_json::from_value(
        serde_json::json!({"id":id,"threadId":thread_id,"type":"provider-session.detached","occurredAt":TEST_TIME,"payload":{"providerSessionId":"session:test","detachedAt":TEST_TIME}}),
    )?)
}
pub fn project_event(
    id: &str,
    project_id: &str,
) -> Result<t3_contracts::application_event::ApplicationProjectEvent> {
    Ok(serde_json::from_value(
        serde_json::json!({"sequence":0,"eventId":id,"aggregateKind":"project","aggregateId":project_id,"type":"project.created","occurredAt":TEST_TIME,"commandId":null,"causationEventId":null,"correlationId":null,"metadata":{},"payload":{"projectId":project_id,"title":"Project","workspaceRoot":"/test/project","defaultModelSelection":null,"scripts":[],"createdAt":TEST_TIME,"updatedAt":TEST_TIME}}),
    )?)
}

pub struct Script<I, O>(Arc<ScriptState<I, O>>);
struct ScriptState<I, O> {
    responses: Mutex<std::collections::VecDeque<Result<O>>>,
    calls: mpsc::UnboundedSender<I>,
    receiver: tokio::sync::Mutex<mpsc::UnboundedReceiver<I>>,
}
impl<I, O> Clone for Script<I, O> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<I, O> Default for Script<I, O> {
    fn default() -> Self {
        let (calls, receiver) = mpsc::unbounded_channel();
        Self(Arc::new(ScriptState {
            responses: Mutex::new(std::collections::VecDeque::new()),
            calls,
            receiver: tokio::sync::Mutex::new(receiver),
        }))
    }
}
impl<I, O> Script<I, O> {
    pub fn push(&self, result: Result<O>) {
        self.0
            .responses
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_back(result);
    }
    pub fn call(&self, input: I) -> Result<O> {
        let mut responses = self
            .0
            .responses
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.0
            .calls
            .send(input)
            .map_err(|_| crate::error::Error::Cancelled)?;
        responses
            .pop_front()
            .unwrap_or(Err(crate::error::Error::Unsupported("unscripted seam")))
    }
    pub async fn next_call(&self) -> Option<I> {
        self.0.receiver.lock().await.recv().await
    }
}

macro_rules! scripted_service {
    ($fake:ident : $service:ident { $( $method:ident ( $( $arg:ident : $input:ty ),* ) -> $output:ty; )* }) => {
        #[derive(Default)]
        pub struct $fake { $(pub $method: Script<($($input,)*), $output>,)* }
        #[async_trait]
        impl $service for $fake {
            $(async fn $method(&self, $($arg: $input),*) -> Result<$output> {
                self.$method.call(($($arg,)*))
            })*
        }
    }
}

scripted_service! { FakeProjectionReader: ProjectionReader {
    turn_start_context(thread_id: ThreadId, run_id: RunId) -> TurnStartContext;
    thread_titles(thread_ids: Vec<ThreadId>) -> OrchestrationV2ThreadTitles;
    thread_link_metadata(thread_ids: Vec<ThreadId>) -> std::collections::BTreeMap<ThreadId, OrchestrationV2ThreadLinkMetadata>;
    older_settled_threads(input: OrchestrationV2GetOlderSettledThreadsInput) -> OrchestrationV2OlderSettledThreadsPage;
    runtime_response_context(thread_id: ThreadId, request_id: RuntimeRequestId) -> Option<ControlContext>;
    checkpoint_context(thread_id: ThreadId) -> CheckpointContext;
    subscriber_shell_snapshot(select: Option<ShellIndexSelection>) -> SubscriberShellRead;
    query_records(query: ProjectionRecordQuery) -> ProjectionRecords;
    message_count(thread_id: ThreadId, filter: MessageFilter) -> u64;
    runtime_recovery(thread_id: ThreadId) -> OrchestrationV2ThreadProjection;
    provider_target(query: ProviderTargetQuery) -> Option<ControlContext>;
    records(thread_id: ThreadId, fields: Vec<RecordKind>, filter: RecordFilter) -> ProjectionRecords;
    snapshot(thread_id: ThreadId, window: Option<SnapshotWindow>) -> ProjectionSnapshot;
    history(query: HistoryQuery) -> OrchestrationV2ThreadHistoryPage;
    search(query: OrchestrationV2SearchThreadInput) -> OrchestrationV2SearchThreadResult;
    thread_shells(query: ShellQuery) -> OrchestrationV2ThreadShellSnapshot;
    checkpoint_capture(thread_id: ThreadId, run_id: RunId, scope_id: CheckpointScopeId) -> CheckpointCaptureContext;
    checkpoint_rollback(thread_id: ThreadId, provider_thread_id: ProviderThreadId, checkpoint_id: CheckpointId, scope_id: CheckpointScopeId) -> CheckpointRollbackContext;
    runtime_request(thread_id: ThreadId, request_id: RuntimeRequestId) -> Option<OrchestrationV2RuntimeRequest>;
    turn_item(thread_id: ThreadId, item_id: TurnItemId) -> Option<OrchestrationV2TurnItem>;
    plan(thread_id: ThreadId, plan_id: PlanId) -> Option<OrchestrationV2PlanArtifact>;
    held_queued_runs(thread_id: ThreadId) -> Vec<OrchestrationV2Run>;
    app_owned_child_threads(thread_id: ThreadId) -> Vec<ThreadId>;
    pending_delegated_completion(child_thread_id: ThreadId) -> bool;
    turn_start_history(thread_id: ThreadId, run_ids: Option<Vec<RunId>>) -> Vec<OrchestrationV2TurnItem>;
    unpaired_interrupt(thread_id: ThreadId, request_id: TurnItemId, result_id: TurnItemId) -> bool;
    attachment_ids(thread_id: ThreadId) -> Vec<String>;
    transcript_byte_floor(thread_id: ThreadId, limit: u64) -> u64;
    thread(thread_id: ThreadId) -> Option<OrchestrationV2AppThread>;
    projection(thread_id: ThreadId) -> Option<OrchestrationV2ThreadProjection>;
    shell(thread_id: ThreadId) -> Option<OrchestrationV2ThreadShell>;
    shell_snapshot(project_ids: Option<Vec<ProjectId>>, root_thread_id: Option<ThreadId>) -> OrchestrationV2ShellSnapshot;
    control(query: ControlQuery) -> Option<ControlContext>;
    timeline(query: TimelineQuery) -> TimelinePage;
    run(thread_id: ThreadId, run_id: RunId) -> Option<OrchestrationV2Run>;
    run_message(thread_id: ThreadId, run_id: RunId) -> Option<OrchestrationV2ConversationMessage>;
    can_start_queued_run(thread_id: ThreadId, run_id: RunId) -> bool;
    recovery_thread_ids() -> Vec<ThreadId>;
    unreadable_thread_ids() -> Vec<ThreadId>;
    next_turn_item_ordinal(thread_id: ThreadId, run_id: Option<RunId>) -> u64;
} }

scripted_service! { FakeTitleGenerator: TitleGenerator {
    generate_with_input(input: TitleGenerationInput, cancellation: CancellationToken) -> String;
    generate(thread_id: ThreadId, messages: Vec<OrchestrationV2HistoricalMessage>, cancellation: CancellationToken) -> String;
} }

scripted_service! { FakeProviderRegistry: ProviderRegistry {
    adapter(instance_id: ProviderInstanceId) -> Arc<dyn ProviderAdapter>;
    metadata(instance_id: ProviderInstanceId) -> AdapterMetadata;
    list() -> Vec<ProviderInstanceId>;
} }

scripted_service! { FakeSessionDirectory: SessionDirectory {
    attach_with_policy(instance_id: ProviderInstanceId, input: crate::provider::OpenSession, options: RuntimePolicyOptions) -> Arc<dyn SessionRuntime>;
    detach_thread(input: DetachSession) -> ();
    release_unless_busy(input: ReleaseSession) -> bool;
    other_working_threads(session_id: ProviderSessionId, thread_id: ThreadId) -> Vec<ThreadId>;
    get(session_id: ProviderSessionId) -> Option<Arc<dyn SessionRuntime>>;
    attach(instance_id: ProviderInstanceId, input: crate::provider::OpenSession) -> Arc<dyn SessionRuntime>;
    detach(session_id: ProviderSessionId, revoke_credential: bool) -> ();
    shutdown() -> ();
} }

scripted_service! { FakeThreadOperations: ThreadOperations {
    continue_run_with_options(thread_id: ThreadId, source_run_id: RunId, options: EffectExecutionOptions) -> ();
    start_with_options(thread_id: ThreadId, run_id: RunId, options: EffectExecutionOptions) -> ();
    start(thread_id: ThreadId, run_id: RunId) -> ();
    finalize(thread_id: ThreadId, run_id: RunId, scope_id: CheckpointScopeId) -> ();
    continue_run(thread_id: ThreadId, source_run_id: RunId) -> ();
    stop_delegated(thread_id: ThreadId, reason: Option<String>) -> ();
    generate_title(thread_id: ThreadId, kind: TitleKind) -> ();
    stop_delegated_for_effect(thread_id: ThreadId, command_id: CommandId, reason: Option<String>) -> ();
    generate_title_for_effect(thread_id: ThreadId, command_id: CommandId, kind: TitleKind) -> ();
    recover_delegated_task(thread_id: ThreadId, source_run_id: Option<RunId>, completed_run_id: Option<RunId>) -> ();
    resume_queue(thread_id: ThreadId, selected_run_id: Option<RunId>) -> ();
} }

scripted_service! { FakePullRequestBackend: PullRequestBackend {
    detail(input: t3_contracts::pull_request::PullRequestRef) -> t3_contracts::pull_request::PullRequestDetail;
    activity(input: t3_contracts::pull_request::PullRequestRef) -> t3_contracts::pull_request::PullRequestActivity;
    thread_comments(input: t3_contracts::pull_request::PullRequestThreadCommentsInput) -> t3_contracts::pull_request::PullRequestThreadCommentsResult;
    watch_fingerprint(input: t3_contracts::pull_request::PullRequestRef) -> Option<PullRequestWatchFingerprint>;
    summary(input: t3_contracts::pull_request::PullRequestRef) -> t3_contracts::pull_request::PullRequestSummary;
    stack(input: t3_contracts::pull_request::PullRequestRef, include_details: bool) -> Option<t3_contracts::thread_pull_request::ThreadPullRequestStack>;
    batches_reads(input: t3_contracts::pull_request::PullRequestRef) -> bool;
    invalidate(input: t3_contracts::pull_request::PullRequestInvalidateInput, notify_readers: bool) -> ();
    changes(input: t3_contracts::pull_request::PullRequestRef) -> tokio::sync::watch::Receiver<u64>;
    subscribe_merges() -> tokio::sync::broadcast::Receiver<t3_contracts::pull_request::PullRequestRef>;
    notify_linked_threads_changed() -> ();
    refresh(cwd: PathBuf, links: Vec<ThreadPullRequestLink>) -> Vec<ThreadPullRequestLink>;
    discover(cwd: PathBuf, branch: String) -> Vec<ThreadPullRequestLink>;
} }

scripted_service! { FakeWorkspaceBackend: WorkspaceBackend {
    repository_identity(cwd: PathBuf) -> Option<t3_contracts::environment::RepositoryIdentity>;
    repository_identity_with_refresh(cwd: PathBuf, refresh: bool) -> Option<t3_contracts::environment::RepositoryIdentity>;
    branch_pull_request(query: BranchPullRequestQuery) -> Option<GitBranchPullRequest>;
    invalidate_status(cwd: PathBuf) -> ();
    default_branch(cwd: PathBuf) -> Option<String>;
    checked_out_pull_request(cwd: PathBuf, branch: String, url: String) -> bool;
    prepare(project_id: ProjectId, thread_id: ThreadId, strategy: OrchestrationV2ThreadLaunchWorkspaceStrategy, cancellation: CancellationToken) -> Workspace;
    remove(workspace: Workspace, cancellation: CancellationToken) -> ();
    current_branch(cwd: PathBuf) -> Option<String>;
} }

scripted_service! { FakeAttachmentBackend: AttachmentBackend {
    claim_pending(thread_id: ThreadId, attachments: Vec<t3_contracts::chat_attachment::ChatAttachment>) -> ClaimedAttachments;
    release_claimed_paths(paths: Vec<PathBuf>) -> ();
    resolve(thread_id: ThreadId, attachments: Vec<t3_contracts::chat_attachment::ChatAttachment>) -> Vec<t3_contracts::chat_attachment::ChatAttachment>;
    claim(thread_id: ThreadId, attachment_ids: Vec<String>) -> ();
    release(thread_id: ThreadId, attachment_ids: Vec<String>) -> ();
} }

scripted_service! { FakeAgentScope: AgentScope {
    kill(thread_id: ThreadId) -> bool;
    oom_killed(thread_id: ThreadId) -> bool;
} }

scripted_service! { FakeProviderTelemetry: ProviderTelemetry {
    refresh_usage(instance_id: ProviderInstanceId, cancellation: CancellationToken) -> ();
    turn_completed(input: CompletedProviderTurn, cancellation: CancellationToken) -> ();
} }

scripted_service! { FakeProviderTurnControl: ProviderTurnControl {
    interrupt(input: InterruptTurn, cancellation: CancellationToken) -> ();
    interrupt_with_outcome(input: InterruptTurn, cancellation: CancellationToken) -> Option<crate::provider::SubagentOutcome>;
    interrupt_and_await_terminal(input: RestartTurn, cancellation: CancellationToken) -> ();
    steer(input: SteerTurn, cancellation: CancellationToken) -> ();
    apply_selection(input: ApplyModelSelection, cancellation: CancellationToken) -> ();
} }

scripted_service! { FakeCheckpointRollback: CheckpointRollback {
    rollback(input: RollbackRequest, cancellation: CancellationToken) -> ();
} }

scripted_service! { FakeRuntimeRequestResponder: RuntimeRequestResponder {
    respond(thread_id: ThreadId, session_id: ProviderSessionId, response: crate::provider::RuntimeResponse, cancellation: CancellationToken) -> ();
} }

scripted_service! { FakeSettleScriptRunner: SettleScriptRunner {
    run(input: SettleScriptInput, cancellation: CancellationToken) -> SettleScriptRun;
} }

scripted_service! { FakeEventIngestor: EventIngestor {
    ingest_with_context(session_id: ProviderSessionId, instance_id: ProviderInstanceId, thread_id: ThreadId, event: ProviderEvent, context: IngestionContext) -> ();
    ingest(session_id: ProviderSessionId, instance_id: ProviderInstanceId, thread_id: ThreadId, event: ProviderEvent) -> ();
} }

scripted_service! { FakeThreadWorktreeRestore: ThreadWorktreeRestore {
    restore(thread: OrchestrationV2AppThread) -> bool;
} }

scripted_service! { FakeSessionRetentionCheck: SessionRetentionCheck {
    keep() -> bool;
} }

scripted_service! { FakeRuntimePolicyResolver: RuntimePolicyResolver {
    resolve(thread: OrchestrationV2AppThread, model_selection: t3_contracts::model_selection::ModelSelection) -> ConfiguredRuntimePolicy;
} }

#[derive(Default)]
pub struct FakeCheckpointBackend {
    pub capture: Script<(CaptureCheckpoint, CancellationToken), CheckpointRef>,
    pub lookup: Script<(OrchestrationV2CheckpointScope, u64), Option<CheckpointRef>>,
    pub diff: Script<(PathBuf, CheckpointRef, CheckpointRef), CheckpointDiff>,
    pub restore: Script<(OrchestrationV2CheckpointScope, u64, CancellationToken), ()>,
    pub prune: Script<(OrchestrationV2CheckpointScope, Vec<u64>), ()>,
    pub checkpoint_ref: Script<(CheckpointScopeId, u64), CheckpointRef>,
    pub is_git_repository: Script<PathBuf, bool>,
    pub delete_checkpoint_refs: Script<
        (
            OrchestrationV2CheckpointScope,
            Vec<OrchestrationV2Checkpoint>,
        ),
        (),
    >,
}
#[async_trait]
impl CheckpointBackend for FakeCheckpointBackend {
    async fn capture(
        &self,
        input: CaptureCheckpoint,
        cancellation: CancellationToken,
    ) -> Result<CheckpointRef> {
        self.capture.call((input, cancellation))
    }
    async fn lookup(
        &self,
        scope: OrchestrationV2CheckpointScope,
        ordinal: u64,
    ) -> Result<Option<CheckpointRef>> {
        self.lookup.call((scope, ordinal))
    }
    async fn diff(
        &self,
        cwd: PathBuf,
        from: CheckpointRef,
        to: CheckpointRef,
    ) -> Result<CheckpointDiff> {
        self.diff.call((cwd, from, to))
    }
    async fn restore(
        &self,
        scope: OrchestrationV2CheckpointScope,
        ordinal: u64,
        cancellation: CancellationToken,
    ) -> Result<()> {
        self.restore.call((scope, ordinal, cancellation))
    }
    async fn prune(&self, scope: OrchestrationV2CheckpointScope, preserve: Vec<u64>) -> Result<()> {
        self.prune.call((scope, preserve))
    }
    fn checkpoint_ref(&self, scope_id: &CheckpointScopeId, ordinal: u64) -> Result<CheckpointRef> {
        self.checkpoint_ref.call((scope_id.clone(), ordinal))
    }
    async fn is_git_repository(&self, cwd: PathBuf) -> Result<bool> {
        self.is_git_repository.call(cwd)
    }
    async fn delete_checkpoint_refs(
        &self,
        scope: OrchestrationV2CheckpointScope,
        checkpoints: Vec<OrchestrationV2Checkpoint>,
    ) -> Result<()> {
        self.delete_checkpoint_refs.call((scope, checkpoints))
    }
}

#[derive(Default)]
pub struct FakeCommandDispatcher {
    pub dispatch: Script<OrchestrationV2ServerCommand, crate::stores::receipts::CommandReceipt>,
    pub command_lanes:
        Script<(), Arc<crate::ports::decisions::thread_command_executor::KeyedLanes>>,
}
#[async_trait]
impl CommandDispatcher for FakeCommandDispatcher {
    async fn dispatch(
        &self,
        command: OrchestrationV2ServerCommand,
    ) -> Result<crate::stores::receipts::CommandReceipt> {
        self.dispatch.call(command)
    }
    fn command_lanes(
        &self,
    ) -> Result<Arc<crate::ports::decisions::thread_command_executor::KeyedLanes>> {
        self.command_lanes.call(())
    }
}

#[derive(Default)]
pub struct FakeDecisionEngine {
    pub decide: Script<(DecisionContext, OrchestrationV2ServerCommand), Decision>,
    pub decide_prepared: Script<(PreparedDecisionContext, OrchestrationV2ServerCommand), Decision>,
}
impl DecisionEngine for FakeDecisionEngine {
    fn decide(
        &self,
        context: &DecisionContext,
        command: &OrchestrationV2ServerCommand,
    ) -> Result<Decision> {
        self.decide.call((context.clone(), command.clone()))
    }
    fn decide_prepared(
        &self,
        context: &PreparedDecisionContext,
        command: &OrchestrationV2ServerCommand,
    ) -> Result<Decision> {
        self.decide_prepared
            .call((context.clone(), command.clone()))
    }
}

#[derive(Default)]
pub struct FakeSettingsService {
    pub get: Script<(), t3_contracts::settings::ServerSettings>,
    pub subscribe:
        Script<(), tokio::sync::watch::Receiver<Arc<t3_contracts::settings::ServerSettings>>>,
}
#[async_trait]
impl SettingsService for FakeSettingsService {
    async fn get(&self) -> Result<t3_contracts::settings::ServerSettings> {
        self.get.call(())
    }
    fn subscribe(
        &self,
    ) -> Result<tokio::sync::watch::Receiver<Arc<t3_contracts::settings::ServerSettings>>> {
        self.subscribe.call(())
    }
}

#[derive(Default)]
pub struct FakeEffectExecutor {
    pub execute: Script<(EffectRow, CancellationToken), ()>,
    pub execute_with_options: Script<(EffectRow, CancellationToken, EffectExecutionOptions), ()>,
    pub failed: Script<(EffectRow, String, bool), ()>,
}
#[async_trait]
impl EffectExecutor for FakeEffectExecutor {
    async fn execute(&self, effect: EffectRow, cancellation: CancellationToken) -> Result<()> {
        self.execute.call((effect, cancellation))
    }
    async fn execute_with_options(
        &self,
        effect: EffectRow,
        cancellation: CancellationToken,
        options: EffectExecutionOptions,
    ) -> Result<()> {
        self.execute_with_options
            .call((effect, cancellation, options))
    }
    async fn failed(
        &self,
        effect: EffectRow,
        error: &crate::error::Error,
        will_retry: bool,
    ) -> Result<()> {
        self.failed.call((effect, error.to_string(), will_retry))
    }
}

pub struct FakeEventCommitter {
    pub database: StoreDatabase,
    pub commit: Script<(CommitRequest, CommitOptions), CommitResult>,
    pub commit_project: Script<
        (
            ApplicationProjectEvent,
            crate::stores::receipts::CommandReceipt,
        ),
        crate::stores::receipts::CommandReceipt,
    >,
    pub events_by_command: Script<CommandId, Vec<OrchestrationV2StoredEvent>>,
    pub latest_sequence: Script<Option<ThreadId>, u64>,
    pub publication: tokio::sync::broadcast::Sender<ApplicationStoredEvent>,
    pub shared_publication: tokio::sync::broadcast::Sender<Arc<ApplicationStoredEvent>>,
}
impl FakeEventCommitter {
    pub fn new(database: StoreDatabase) -> Self {
        Self {
            database,
            commit: Script::default(),
            commit_project: Script::default(),
            events_by_command: Script::default(),
            latest_sequence: Script::default(),
            publication: tokio::sync::broadcast::channel(16).0,
            shared_publication: tokio::sync::broadcast::channel(16).0,
        }
    }
    /// Delivers to both `subscribe` and `subscribe_shared` receivers, as a sink publishes.
    pub fn publish(&self, event: ApplicationStoredEvent) {
        let _ = self.shared_publication.send(Arc::new(event.clone()));
        let _ = self.publication.send(event);
    }
}
#[async_trait]
impl EventCommitter for FakeEventCommitter {
    async fn commit(&self, input: CommitRequest) -> Result<CommitResult> {
        self.commit.call((input, CommitOptions::default()))
    }
    async fn commit_with_options(
        &self,
        input: CommitRequest,
        options: CommitOptions,
    ) -> Result<CommitResult> {
        self.commit.call((input, options))
    }
    async fn commit_prepared(
        &self,
        input: CommitRequest,
        options: CommitOptions,
        prepare: CommitPreparation,
    ) -> Result<CommitResult> {
        let script = self.commit.clone();
        self.database
            .transaction(move |tx| script.call((prepare(tx, input)?, options)))
            .await
    }
    async fn commit_project(
        &self,
        event: ApplicationProjectEvent,
        receipt: crate::stores::receipts::CommandReceipt,
    ) -> Result<crate::stores::receipts::CommandReceipt> {
        self.commit_project.call((event, receipt))
    }
    async fn events_by_command(
        &self,
        command_id: CommandId,
    ) -> Result<Vec<OrchestrationV2StoredEvent>> {
        self.events_by_command.call(command_id)
    }
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ApplicationStoredEvent> {
        self.publication.subscribe()
    }
    fn subscribe_shared(
        &self,
    ) -> Result<tokio::sync::broadcast::Receiver<Arc<ApplicationStoredEvent>>> {
        Ok(self.shared_publication.subscribe())
    }
    async fn latest_sequence(&self, thread_id: Option<ThreadId>) -> Result<u64> {
        self.latest_sequence.call(thread_id)
    }
}

/// A resolver that always returns one configured policy: turn-start tests that do not exercise
/// policy resolution call `start_resolving` with it.
pub struct FixedRuntimePolicy(pub ConfiguredRuntimePolicy);
impl FixedRuntimePolicy {
    pub fn new(policy: RuntimePolicy) -> Self {
        Self(ConfiguredRuntimePolicy {
            policy,
            options: RuntimePolicyOptions::default(),
        })
    }
}
#[async_trait]
impl crate::api::RuntimePolicyResolver for FixedRuntimePolicy {
    async fn resolve(
        &self,
        _thread: OrchestrationV2AppThread,
        _selection: t3_contracts::model_selection::ModelSelection,
    ) -> Result<ConfiguredRuntimePolicy> {
        Ok(self.0.clone())
    }
}

#[cfg(test)]
mod seam_tests {
    use super::*;
    use crate::error::Error;

    fn request() -> CommitRequest {
        CommitRequest {
            receipt: None,
            decision: Decision::default(),
            guard: CommitGuard::None,
        }
    }

    fn result() -> CommitResult {
        CommitResult {
            committed: true,
            receipt: None,
            events: Vec::new(),
            cancelled_effect_ids: Vec::new(),
        }
    }

    struct BasicCommitter(FakeEventCommitter);
    #[async_trait]
    impl EventCommitter for BasicCommitter {
        async fn commit(&self, input: CommitRequest) -> Result<CommitResult> {
            self.0.commit(input).await
        }
        async fn commit_project(
            &self,
            event: ApplicationProjectEvent,
            receipt: crate::stores::receipts::CommandReceipt,
        ) -> Result<crate::stores::receipts::CommandReceipt> {
            self.0.commit_project(event, receipt).await
        }
        async fn events_by_command(
            &self,
            command: CommandId,
        ) -> Result<Vec<OrchestrationV2StoredEvent>> {
            self.0.events_by_command(command).await
        }
        fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ApplicationStoredEvent> {
            self.0.subscribe()
        }
        async fn latest_sequence(&self, thread: Option<ThreadId>) -> Result<u64> {
            self.0.latest_sequence(thread).await
        }
    }

    #[tokio::test]
    async fn old_committer_rejects_guards_and_callbacks_without_running_them() -> Result<()> {
        let db = TestDatabase::memory()?;
        let committer = BasicCommitter(FakeEventCommitter::new(db.stores));
        let cases = [
            CommitOptions {
                command_id: Some(CommandId("origin".into())),
                ..Default::default()
            },
            CommitOptions {
                guard_pending_user_input_cancellations: true,
                ..Default::default()
            },
            CommitOptions {
                unless_interrupt_requested: Some((
                    ThreadId("thread".into()),
                    "stop".into(),
                    "result".into(),
                )),
                ..Default::default()
            },
            CommitOptions {
                update_delegated_completion_only_run_id: Some("run".into()),
                ..Default::default()
            },
            CommitOptions {
                update_all_delegated_completions_only: true,
                ..Default::default()
            },
        ];
        for options in cases {
            assert!(matches!(
                committer.commit_with_options(request(), options).await,
                Err(Error::Unsupported("EventCommitter.commit_with_options"))
            ));
        }
        let invoked = Arc::new(AtomicU64::new(0));
        let observed = invoked.clone();
        assert!(matches!(
            committer
                .commit_prepared(
                    request(),
                    CommitOptions::default(),
                    Box::new(move |_, input| {
                        observed.fetch_add(1, Ordering::SeqCst);
                        Ok(input)
                    })
                )
                .await,
            Err(Error::Unsupported("EventCommitter.commit_prepared"))
        ));
        assert_eq!(invoked.load(Ordering::SeqCst), 0);
        committer.0.commit.push(Ok(result()));
        assert!(
            committer
                .commit_with_options(request(), CommitOptions::default())
                .await?
                .committed
        );
        Ok(())
    }

    #[tokio::test]
    async fn preparation_and_scripted_commit_failure_roll_back_together() -> Result<()> {
        let db = TestDatabase::memory()?;
        db.stores
            .transaction(|tx| {
                tx.execute_batch("CREATE TABLE seam_marker(value TEXT NOT NULL)")?;
                Ok(())
            })
            .await?;
        let committer: Arc<FakeEventCommitter> =
            Arc::new(FakeEventCommitter::new(db.stores.clone()));
        let port: Arc<dyn EventCommitter> = committer.clone();
        for callback_fails in [true, false] {
            if !callback_fails {
                committer.commit.push(Err(Error::Stale));
            }
            let error = port
                .commit_prepared(
                    request(),
                    CommitOptions::default(),
                    Box::new(move |tx, input| {
                        tx.execute("INSERT INTO seam_marker VALUES('uncommitted')", [])?;
                        if callback_fails {
                            return Err(Error::Cancelled);
                        }
                        Ok(input)
                    }),
                )
                .await;
            assert!(matches!(error, Err(Error::Cancelled | Error::Stale)));
            let count = db
                .stores
                .read(|connection| {
                    Ok(
                        connection.query_row("SELECT count(*) FROM seam_marker", [], |row| {
                            row.get::<_, i64>(0)
                        })?,
                    )
                })
                .await?;
            assert_eq!(count, 0);
        }
        committer.commit.push(Ok(result()));
        port.commit_prepared(
            request(),
            CommitOptions {
                command_id: Some(CommandId("provider-origin".into())),
                guard_pending_user_input_cancellations: true,
                unless_interrupt_requested: Some((
                    ThreadId("thread".into()),
                    "stop".into(),
                    "stop-result".into(),
                )),
                ..Default::default()
            },
            Box::new(|tx, mut input| {
                tx.execute("INSERT INTO seam_marker VALUES('committed')", [])?;
                input
                    .decision
                    .events
                    .push(detached_event("event", "thread")?);
                Ok(input)
            }),
        )
        .await?;
        let count = db
            .stores
            .read(|connection| {
                Ok(
                    connection.query_row("SELECT count(*) FROM seam_marker", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                )
            })
            .await?;
        assert_eq!(count, 1);
        committer.commit.next_call().await;
        let (prepared, options) = committer
            .commit
            .next_call()
            .await
            .expect("successful prepared call");
        assert_eq!(prepared.decision.events.len(), 1);
        assert_eq!(
            options.command_id,
            Some(CommandId("provider-origin".into()))
        );
        assert!(options.guard_pending_user_input_cancellations);
        assert_eq!(
            options.unless_interrupt_requested,
            Some((
                ThreadId("thread".into()),
                "stop".into(),
                "stop-result".into()
            ))
        );
        Ok(())
    }
}
