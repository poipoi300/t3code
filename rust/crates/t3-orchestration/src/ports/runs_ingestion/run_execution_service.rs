use super::context_handoff_delivery::PreparedDelivery;
use super::provider_event_ingestor::expanded_id;
use super::provider_event_ingestor::{IngestInput, ProviderEventIngestor};
use crate::ports::commit_worker::provider_commit_batch::{
    ConsumerOwner, MAX_EVENTS, PreparedCommit, ProviderCommitBatch, is_snapshot, snapshot_bytes,
};
use crate::ports::transport::assistant_streaming::AssistantStreamingFilter;
use crate::provider::{
    EventSubscription, InterruptInput, ProviderError, SessionRuntime, TurnInput,
};
use crate::{
    api::*,
    effects::{EffectRequest, PendingEffect},
    error::{Error, Result},
    provider::{ProviderEvent, ProviderEventKind, TerminalOutcome, ThreadDisposition},
};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::{future::Future, pin::Pin};
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use tokio::sync::oneshot;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Clone)]
pub struct StartRootRun {
    pub state: RootRunState,
    pub turn: TurnInput,
    pub session: Arc<dyn SessionRuntime>,
    pub inherited_background_items: HashMap<TurnItemId, RunId>,
    pub related_thread_ids: Vec<ThreadId>,
    pub related_provider_thread_ids: Vec<ProviderThreadId>,
}

#[async_trait]
pub trait RootTurnPreparation: Send + Sync {
    async fn prepare(
        &self,
        turn: TurnInput,
        compact: bool,
    ) -> Result<(TurnInput, PreparedDelivery)>;
}

pub struct AppModelContext {
    pub item_id: TurnItemId,
    pub text: String,
}

pub trait PreviewActivity: Send + Sync {
    fn end(self: Box<Self>);
}

struct PreviewGuard(Option<Box<dyn PreviewActivity>>);
impl Drop for PreviewGuard {
    fn drop(&mut self) {
        if let Some(activity) = self.0.take() {
            activity.end();
        }
    }
}

// seam-shim: the server binds MCP context/credentials, preview activity and lifecycle observers.
#[async_trait]
pub trait RunExecutionHost: Send + Sync {
    async fn app_context(&self, thread: ThreadId) -> Result<Vec<AppModelContext>>;
    async fn touch_mcp_credential(&self, thread: ThreadId) -> Result<()>;
    fn begin_preview(&self, thread: ThreadId) -> Box<dyn PreviewActivity>;
    async fn run_started(&self, context: ProviderTurnAnalyticsContext) -> Result<()>;
    async fn refresh_after_turn(&self, project: ProjectId) -> Result<()>;
}

struct StartOptions {
    policy: crate::provider::RuntimePolicyOptions,
    preparation: Option<Arc<dyn RootTurnPreparation>>,
    streaming: AssistantStreamingFilter,
}

type PendingProviderCommit = Pin<Box<dyn Future<Output = Result<Option<CommitResult>>> + Send>>;

fn event_thread_id(event: &ProviderEvent) -> Result<Option<ThreadId>> {
    match &event.kind {
        ProviderEventKind::Message(message) => Ok(Some(expanded_id(&message.thread_id)?)),
        ProviderEventKind::TurnItem { item, .. } => match item.as_ref() {
            OrchestrationV2TurnItem::AssistantMessage(item) => {
                Ok(Some(expanded_id(&item.thread_id)?))
            }
            OrchestrationV2TurnItem::Reasoning(item) => Ok(Some(expanded_id(&item.thread_id)?)),
            _ => Ok(None),
        },
        _ => Ok(None),
    }
}

fn run_command_id(run: &RunId) -> CommandId {
    CommandId(format!("command:effect:provider-turn.start:{}", run.0))
}

async fn flush_pending(pending: &mut VecDeque<PendingProviderCommit>) -> Result<()> {
    while let Some(reply) = pending.pop_front() {
        reply.await?;
    }
    Ok(())
}

fn root_analytics_context(
    input: &StartRootRun,
    segment: ProviderTurnSegment,
) -> Result<ProviderTurnAnalyticsContext> {
    Ok(ProviderTurnAnalyticsContext {
        run_id: Some(expanded_id(&input.state.run.id)?),
        model_selection: input.turn.model_selection.clone(),
        runtime_mode: Some(input.turn.runtime_policy.runtime_mode),
        interaction_mode: Some(input.turn.runtime_policy.interaction_mode),
        origin: Some(ProviderTurnOrigin {
            created_by: input.turn.message.created_by.clone(),
            creation_source: input
                .turn
                .message
                .creation_source
                .clone()
                .unwrap_or(OrchestrationV2CreationSource::Provider),
            scheduled: input.turn.message.scheduled_task_id.is_some(),
            subagent: input.turn.app_thread.lineage.relationship_to_parent
                == Some(AppThreadLineageRelationshipToParent::Subagent),
        }),
        segment: Some(segment),
        restart_continuation: input
            .turn
            .restart_continuation_of_run_id
            .as_ref()
            .map(|_| true),
    })
}

pub struct RunExecutionHandle {
    settled: oneshot::Receiver<Result<()>>,
}

impl RunExecutionHandle {
    pub async fn settled(self) -> Result<()> {
        self.settled.await.map_err(|_| Error::Service {
            operation: "RunExecutionService.settled",
            detail: "Run execution task ended unexpectedly.".into(),
        })?
    }
}

struct ExecutionScope {
    tasks: TaskTracker,
    cancellation: CancellationToken,
    closed: Mutex<bool>,
}

#[derive(Clone)]
pub struct RunExecutionService {
    pub ingestor: Arc<ProviderEventIngestor>,
    pub checkpoints: Option<Arc<dyn CheckpointBackend>>,
    scope: Arc<ExecutionScope>,
    host: Option<Arc<dyn RunExecutionHost>>,
    settings: Option<Arc<dyn SettingsService>>,
    batches: Option<ProviderCommitBatch>,
}

struct CancelStartup(Option<CancellationToken>);
impl Drop for CancelStartup {
    fn drop(&mut self) {
        if let Some(token) = &self.0 {
            token.cancel();
        }
    }
}

impl RunExecutionService {
    pub fn new(
        ingestor: Arc<ProviderEventIngestor>,
        checkpoints: Option<Arc<dyn CheckpointBackend>>,
    ) -> Self {
        Self {
            ingestor,
            checkpoints,
            host: None,
            settings: None,
            batches: None,
            scope: Arc::new(ExecutionScope {
                tasks: TaskTracker::new(),
                cancellation: CancellationToken::new(),
                closed: Mutex::new(false),
            }),
        }
    }

    pub fn with_host(mut self, host: Arc<dyn RunExecutionHost>) -> Self {
        self.host = Some(host);
        self
    }

    pub fn with_settings(mut self, settings: Arc<dyn SettingsService>) -> Self {
        self.settings = Some(settings);
        self
    }

    pub fn with_batches(mut self, batches: ProviderCommitBatch) -> Self {
        self.batches = Some(batches);
        self
    }

    pub async fn start_root_run(
        &self,
        input: StartRootRun,
        cancellation: CancellationToken,
    ) -> Result<RunExecutionHandle> {
        self.start_root_run_with_policy(
            input,
            crate::provider::RuntimePolicyOptions::default(),
            cancellation,
        )
        .await
    }
    pub async fn start_root_run_with_policy(
        &self,
        input: StartRootRun,
        policy_options: crate::provider::RuntimePolicyOptions,
        cancellation: CancellationToken,
    ) -> Result<RunExecutionHandle> {
        self.start_root_run_with_preparation(input, policy_options, None, cancellation)
            .await
    }

    pub async fn start_root_run_with_preparation(
        &self,
        input: StartRootRun,
        policy_options: crate::provider::RuntimePolicyOptions,
        preparation: Option<Arc<dyn RootTurnPreparation>>,
        cancellation: CancellationToken,
    ) -> Result<RunExecutionHandle> {
        let result = self
            .prepare_and_start(
                input.clone(),
                StartOptions {
                    policy: policy_options,
                    preparation,
                    streaming: AssistantStreamingFilter::new(false, 800),
                },
                cancellation,
            )
            .await;
        if let Err(error) = &result
            && !matches!(
                error,
                Error::Cancelled | Error::Stale | Error::Provider(ProviderError::Cancelled)
            )
        {
            let mut state = ExecutionState::new(input)?;
            let terminal = state.failed_terminal(error)?;
            self.finalize(&mut state, terminal, false, None).await?;
        }
        result
    }

    async fn prepare_and_start(
        &self,
        mut input: StartRootRun,
        mut options: StartOptions,
        cancellation: CancellationToken,
    ) -> Result<RunExecutionHandle> {
        if self.scope.cancellation.is_cancelled() || cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if let Some(settings) = &self.settings {
            let settings = t3_contracts::stored::to_value(&settings.get().await?)?;
            let project = expanded_id(&input.turn.app_thread.project_id)?;
            let mode = settings["projectSettingsOverrides"][&project.0]
                .get("responseStreamingMode")
                .filter(|mode| !mode.is_null())
                .unwrap_or(&settings["responseStreamingMode"]);
            options.streaming = AssistantStreamingFilter::new(mode == "turn", 800);
        }
        if input.turn.app_thread.lineage.side_chat != Some(Some(true))
            && let Some(checkpoints) = &self.checkpoints
        {
            let capture = checkpoints.capture(
                CaptureCheckpoint {
                    scope: input.state.checkpoint_scope.clone(),
                    ordinal: input.state.run.ordinal.saturating_sub(1),
                    message: format!("Baseline for run {}", expanded_id(&input.state.run.id)?.0),
                },
                cancellation.clone(),
            );
            tokio::select! {
                biased;
                _ = self.scope.cancellation.cancelled() => return Err(Error::Cancelled),
                _ = cancellation.cancelled() => return Err(Error::Cancelled),
                result = capture => {
                    if matches!(result, Err(Error::Cancelled | Error::Provider(ProviderError::Cancelled))) {
                        return Err(Error::Cancelled);
                    }
                }
            }
        }
        if !self.is_current(&input.state).await? {
            return Err(Error::Stale);
        }
        if input.state.attempt.reason == RunAttemptReason::Initial
            && let Some(host) = &self.host
        {
            let context = root_analytics_context(
                &input,
                ProviderTurnSegment::Attempt(RunAttemptReason::Initial),
            )?;
            let _ = host.run_started(context).await;
        }
        input.turn.provider_thread = input.state.provider_thread.clone();
        let mut subscription = tokio::select! {
            biased;
            _ = self.scope.cancellation.cancelled() => return Err(Error::Cancelled),
            _ = cancellation.cancelled() => return Err(Error::Cancelled),
            result = input.session.subscribe() => result?,
        };
        if !self.is_current(&input.state).await? {
            subscription.close();
            return Err(Error::Stale);
        }
        // A background completion can commit between the initial snapshot and subscription.
        let refreshed = self
            .ingestor
            .projections
            .records(
                expanded_id(&input.state.run.thread_id)?,
                vec![RecordKind::Runs, RecordKind::TurnItems],
                RecordFilter {
                    turn_item_statuses: Some(vec![
                        OrchestrationV2TurnItemStatus::Pending,
                        OrchestrationV2TurnItemStatus::Running,
                        OrchestrationV2TurnItemStatus::Waiting,
                    ]),
                    ..Default::default()
                },
            )
            .await?;
        let mut runs = Vec::new();
        let mut items = Vec::new();
        for records in refreshed.records {
            match records {
                ProjectionRecordSet::Runs(value) => runs = value,
                ProjectionRecordSet::TurnItems(value) => items = value,
                _ => {}
            }
        }
        input.inherited_background_items = select_inherited_background_turn_items(
            &expanded_id(&input.state.run.thread_id)?,
            &expanded_id(&input.state.provider_thread.id)?,
            input.state.run.ordinal,
            &runs,
            &items,
        )?;
        let preview = self
            .host
            .as_ref()
            .map(|host| PreviewGuard(Some(host.begin_preview(input.turn.thread_id.clone()))));
        let startup_cancel = self.scope.cancellation.child_token();
        let mut cancel_on_drop = CancelStartup(Some(startup_cancel.clone()));
        let (accepted, acceptance) = oneshot::channel();
        let (settled, completion) = oneshot::channel();
        {
            let closed = self
                .scope
                .closed
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if *closed {
                subscription.close();
                return Err(Error::Cancelled);
            }
            let service = self.clone();
            self.scope.tasks.spawn(async move {
                let result = service
                    .drive(
                        input,
                        &mut subscription,
                        accepted,
                        startup_cancel,
                        options,
                        preview,
                    )
                    .await;
                subscription.close();
                let _ = settled.send(result);
            });
        }
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(Error::Cancelled),
            result = acceptance => result.map_err(|_| Error::Service {
                operation: "RunExecutionService.startRootRun", detail: "Provider startup ended without an acceptance result.".into(),
            })?,
        };
        result?;
        cancel_on_drop.0 = None;
        Ok(RunExecutionHandle {
            settled: completion,
        })
    }

    pub async fn shutdown(&self) -> Result<()> {
        {
            let mut closed = self
                .scope
                .closed
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            *closed = true;
            self.scope.tasks.close();
            self.scope.cancellation.cancel();
        }
        self.scope.tasks.wait().await;
        Ok(())
    }

    async fn is_current(&self, state: &RootRunState) -> Result<bool> {
        Ok(self
            .ingestor
            .projections
            .run(
                expanded_id(&state.run.thread_id)?,
                expanded_id(&state.run.id)?,
            )
            .await?
            .is_some_and(|run| {
                run.active_attempt_id.as_ref() == Some(&state.attempt.id)
                    && run.status == OrchestrationV2RunStatus::Running
            }))
    }

    async fn drive(
        &self,
        input: StartRootRun,
        subscription: &mut EventSubscription,
        accepted: oneshot::Sender<Result<()>>,
        cancellation: CancellationToken,
        options: StartOptions,
        preview: Option<PreviewGuard>,
    ) -> Result<()> {
        let StartOptions {
            policy,
            preparation,
            streaming,
        } = options;
        let mut accepted = Some(accepted);
        let session = input.session.clone();
        let mut turn = input.turn.clone();
        let identity = input.state.clone();
        let start = async {
            if let Some(host) = &self.host {
                let _ = host.touch_mcp_credential(turn.thread_id.clone()).await;
                turn.app_context = host
                    .app_context(turn.thread_id.clone())
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|entry| {
                        let suffix = entry
                            .item_id
                            .0
                            .chars()
                            .map(|ch| {
                                if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-') {
                                    ch.to_string()
                                } else {
                                    "_".repeat(ch.len_utf16())
                                }
                            })
                            .collect::<String>();
                        (format!("mcp_app_{suffix}"), entry.text)
                    })
                    .collect();
            }
            let compact = turn.message.attachments.is_empty()
                && t3_contracts::wire::trim_js(&turn.message.text).eq_ignore_ascii_case("/compact");
            let (turn, delivery) = if let Some(preparation) = preparation {
                let (turn, delivery) = preparation.prepare(turn, compact).await?;
                (turn, Some(delivery))
            } else {
                (turn, None)
            };
            if !self.is_current(&identity).await? {
                return Err(Error::Stale);
            }
            let thread_id = turn.thread_id.clone();
            let provider_start = async {
                if compact {
                    let extensions = session
                        .extensions()
                        .filter(|extensions| extensions.capabilities().compact)
                        .ok_or(ProviderError::Unsupported("compact"))?;
                    extensions.compact(turn).await
                } else {
                    if policy.reasoning_effort.is_some() {
                        session.start_turn_with_policy(turn, policy).await
                    } else {
                        session.start_turn(turn).await
                    }
                }
            };
            let result = if let Some(host) = &self.host {
                tokio::select! {
                    result=provider_start=>result,
                    _=async { loop {tokio::time::sleep(std::time::Duration::from_secs(3600)).await;let _=host.touch_mcp_credential(thread_id.clone()).await;} }=>unreachable!("credential liveness runs until provider acceptance"),
                }
            } else {
                provider_start.await
            };
            match result {
                Ok(()) => {
                    // Accepted native input must keep ingesting even if bookkeeping fails.
                    if let Some(delivery) = delivery {
                        let _ = delivery.delivered().await;
                    }
                    Ok(())
                }
                Err(error) => {
                    if matches!(&error, ProviderError::Protocol { detail, .. } if detail == "Claude provider query ended before the turn could start.")
                        && let Some(delivery) = delivery
                    {
                        let _ = delivery.unsent().await;
                    }
                    Err(Error::Provider(error))
                }
            }
        };
        tokio::pin!(start);
        let mut state = ExecutionState::new(input)?;
        state.preview = preview;
        state.streaming = streaming;
        let mut started = false;
        let mut stream_done = false;
        let mut failure = None;
        let owner = ConsumerOwner::default();
        let mut pending: VecDeque<PendingProviderCommit> = VecDeque::new();
        loop {
            if started && stream_done {
                break;
            }
            let flush_at = state.streaming.next_idle_flush_at();
            let flush_wait = async {
                match flush_at {
                    Some(at) => {
                        tokio::time::sleep(std::time::Duration::from_millis(
                            at.saturating_sub(self.now_millis()).max(0) as u64,
                        ))
                        .await
                    }
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::pin!(flush_wait);
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    if let Some(sender) = accepted.take() { let _ = sender.send(Err(Error::Cancelled)); }
                    return Err(Error::Cancelled);
                }
                result = &mut start, if !started => {
                    match result {
                        Ok(()) => {
                            started = true;
                            if let Some(sender) = accepted.take() { let _ = sender.send(Ok(())); }
                        }
                        Err(error) => { failure = Some(error); break; }
                    }
                }
                _=&mut flush_wait, if !stream_done=>{
                    if let Err(error)=flush_pending(&mut pending).await {failure=Some(error);break;}
                    let at=self.now_millis().max(flush_at.unwrap_or_default());
                    let mut error=None;
                    for event in state.streaming.take_idle_flushes(at) {
                        let thread=match event_thread_id(&event) {Ok(thread)=>thread,Err(cause)=>{error=Some(cause);break;}};
                        if state.finalized && thread.as_ref()==Some(&state.identity.thread_id) {continue;}
                        let result=if let Some(batches)=&self.batches {batches.barrier(self.consume_event(&mut state,event,false)).await}
                            else {self.consume_event(&mut state,event,false).await};
                        match result {
                            Ok(done)=>stream_done|=done,
                            Err(cause)=>{error=Some(cause);break;}
                        }
                    }
                    if let Some(error)=error {failure=Some(error);break;}
                }
                event = subscription.events.recv(), if !stream_done => {
                    match event {
                        Some(Ok(event)) => match async {
                            if let Some(batches)=&self.batches && !state.finalized && is_snapshot(&event) {
                                let bytes=snapshot_bytes(&event)?;
                                if let Some(input)=self.prepare_snapshot(&mut state,event).await? {
                                    let reply=batches.enqueue_prepared_snapshot(&owner,bytes,input).await?;
                                    pending.push_back(Box::pin(reply.wait()));
                                }
                                if pending.len()>=MAX_EVENTS {pending.pop_front().expect("pending batch limit").await?;}
                                Ok(false)
                            } else {
                                flush_pending(&mut pending).await?;
                                if let Some(batches)=&self.batches {batches.barrier(self.consume(&mut state,event)).await}
                                else {self.consume(&mut state,event).await}
                            }
                        }.await {
                            Ok(done) => stream_done = done,
                            Err(error) => { failure = Some(error); break; }
                        },
                        Some(Err(error)) => { failure = Some(error.into()); break; }
                        None if state.finalized => {
                            match flush_pending(&mut pending).await {Ok(())=>stream_done=true,Err(error)=>{failure=Some(error);break;}}
                        },
                        None => {
                            if let Err(error)=flush_pending(&mut pending).await {failure=Some(error);break;}
                            failure = Some(Error::Service { operation: "RunExecutionService.ingest",
                                detail: "Provider event stream ended without a root terminal.".into() });
                            break;
                        }
                    }
                }
                result=async {match pending.front_mut(){Some(reply)=>reply.as_mut().await,None=>std::future::pending().await}}=>{
                    pending.pop_front();
                    if let Err(error)=result {failure=Some(error);break;}
                }
            }
        }
        if let Some(error) = failure {
            owner.cancel();
            if matches!(
                error,
                Error::Cancelled | Error::Provider(ProviderError::Cancelled)
            ) {
                if let Some(sender) = accepted.take() {
                    let _ = sender.send(Err(Error::Cancelled));
                }
                return Err(Error::Cancelled);
            }
            if !state.finalized {
                let abandoned = self.interrupt_abandoned(&state).await;
                let terminal = state.failed_terminal(&error)?;
                let mut delay = std::time::Duration::from_secs(1);
                loop {
                    match self
                        .finalize(&mut state, terminal.clone(), false, abandoned.clone())
                        .await
                    {
                        Ok(()) => break,
                        Err(write_error) if !started => {
                            if let Some(sender) = accepted.take() {
                                let _ = sender.send(Err(write_error));
                            }
                            return Err(error);
                        }
                        Err(_) => tokio::select! {
                            _ = cancellation.cancelled() => return Err(Error::Cancelled),
                            _ = tokio::time::sleep(delay) => { delay = (delay * 2).min(std::time::Duration::from_secs(30)); }
                        },
                    }
                }
            }
            if let Some(sender) = accepted.take() {
                let _ = sender.send(Ok(()));
            }
        }
        Ok(())
    }

    async fn consume(&self, state: &mut ExecutionState, event: ProviderEvent) -> Result<bool> {
        self.consume_event(state, event, true).await
    }

    async fn prepare_snapshot(
        &self,
        state: &mut ExecutionState,
        event: ProviderEvent,
    ) -> Result<Option<PreparedCommit>> {
        if !state.routing.route(&event.kind, &state.identity)? {
            state.detach_moved_child(&event)?;
            return Ok(None);
        }
        let delivered = state.streaming.filter(event.clone(), self.now_millis());
        let position = if delivered.is_none() {
            self.prepare_held_position(state, &event)?
        } else {
            None
        };
        if let ProviderEventKind::TurnItem { item, .. } = &event.kind {
            let item = t3_contracts::stored::to_value(item)?;
            if item["providerTurnId"]
                == t3_contracts::stored::to_value(&state.routing.root_provider_turn_id)?
            {
                state.latest_item_ordinal = state
                    .latest_item_ordinal
                    .max(item["ordinal"].as_u64().unwrap_or_default());
            }
        }
        state.track(&event)?;
        let Some(event) = delivered else {
            return Ok(position);
        };
        Ok(Some(self.ingestor.prepare_snapshot(
            IngestInput {
                session_id: expanded_id(&state.input.session.session().id)?,
                instance_id: expanded_id(&state.input.state.run.provider_instance_id)?,
                thread_id: state.identity.thread_id.clone(),
                run_id: Some(state.identity.run_id.clone()),
                node_id: Some(expanded_id(&state.input.state.root_node.id)?),
                raw_event_id: None,
                event,
            },
            CommitGuard::None,
            Some(run_command_id(&state.identity.run_id)),
        )?))
    }

    fn now_millis(&self) -> i64 {
        super::provider_event_ingestor::epoch_millis(&self.ingestor.clock.now().0)
            .unwrap_or_default()
    }

    async fn consume_event(
        &self,
        state: &mut ExecutionState,
        event: ProviderEvent,
        filter: bool,
    ) -> Result<bool> {
        if !state.routing.route(&event.kind, &state.identity)? {
            state.detach_moved_child(&event)?;
            return Ok(false);
        }
        if let ProviderEventKind::Turn { provider_turn, .. } = &event.kind
            && provider_turn.provider_thread_id == state.input.state.provider_thread.id
            && provider_turn.run_attempt_id.as_ref() == Some(&state.input.state.attempt.id)
        {
            state.latest_root_turn = Some(*provider_turn.clone());
        }
        let is_root_thread = matches!(&event.kind, ProviderEventKind::Thread { provider_thread, .. }
            if provider_thread.id == state.input.state.provider_thread.id);
        let root_start = matches!(&event.kind, ProviderEventKind::Turn { provider_turn, .. }
            if provider_turn.run_attempt_id.as_ref() == Some(&state.input.state.attempt.id)
                && provider_turn.provider_thread_id == state.input.state.provider_thread.id
                && provider_turn.status == ProviderTurnStatus::Running);
        let guard = if is_root_thread {
            if state.finalized {
                CommitGuard::ProviderThreadOwner {
                    provider_thread_id: state.identity.provider_thread_id.clone(),
                    run_id: state.identity.run_id.clone(),
                    attempt_id: state.identity.attempt_id.clone(),
                    last_run_ordinal: state.input.state.run.ordinal,
                }
            } else {
                state.guard()
            }
        } else {
            CommitGuard::None
        };
        let delivered = if filter {
            state.streaming.filter(event.clone(), self.now_millis())
        } else {
            Some(event.clone())
        };
        if delivered.is_none() {
            self.reserve_held_item(state, &event).await?;
        }
        let ingest = IngestInput {
            session_id: expanded_id(&state.input.session.session().id)?,
            instance_id: expanded_id(&state.input.state.run.provider_instance_id)?,
            thread_id: state.identity.thread_id.clone(),
            run_id: Some(state.identity.run_id.clone()),
            node_id: Some(expanded_id(&state.input.state.root_node.id)?),
            raw_event_id: None,
            event: delivered.clone().unwrap_or_else(|| event.clone()),
        };
        let analytics = if matches!(&event.kind, ProviderEventKind::Turn { .. }) {
            Some(state.analytics_context(&event)?)
        } else {
            None
        };
        let stored = if delivered.is_none() {
            vec![]
        } else if root_start && !state.ownership_checked {
            state.ownership_checked = true;
            // Stop and first native-turn publication must use the dispatcher's thread lane.
            let _lane = self
                .ingestor
                .lanes
                .acquire([state.identity.thread_id.0.clone()])
                .await;
            let stored = self
                .ingestor
                .ingest_normalized_with_context(
                    &ingest,
                    guard,
                    IngestionContext {
                        analytics,
                        command_id: Some(run_command_id(&state.identity.run_id)),
                    },
                )
                .await?;
            if !self.is_current(&state.input.state).await?
                && let Some(turn_id) = state.routing.root_provider_turn_id.clone()
            {
                let _ = state
                    .input
                    .session
                    .interrupt_turn(InterruptInput {
                        provider_thread: state.input.state.provider_thread.clone(),
                        provider_turn_id: turn_id,
                        request_runtime_restart: false,
                    })
                    .await;
            }
            stored
        } else {
            self.ingestor
                .ingest_normalized_with_context(
                    &ingest,
                    guard,
                    IngestionContext {
                        analytics,
                        command_id: Some(run_command_id(&state.identity.run_id)),
                    },
                )
                .await?
        };
        match &event.kind {
            ProviderEventKind::Thread {
                provider_thread,
                pending_background_work,
                ..
            } if is_root_thread => {
                if !stored.is_empty() {
                    state.input.state.provider_thread = *provider_thread.clone();
                    state.snapshot_pending_work = *pending_background_work == Some(true);
                } else if state.finalized {
                    state.owner_lost = true;
                }
            }
            ProviderEventKind::TurnItem { item, .. } => {
                let item = t3_contracts::stored::to_value(&item)?;
                if item["providerTurnId"]
                    == t3_contracts::stored::to_value(&state.routing.root_provider_turn_id)?
                {
                    state.latest_item_ordinal = state
                        .latest_item_ordinal
                        .max(item["ordinal"].as_u64().unwrap_or(0));
                }
            }
            ProviderEventKind::BackgroundWork {
                provider_turn_id,
                pending,
            } => {
                if *pending {
                    state.pending_turns.insert(provider_turn_id.clone());
                } else {
                    state.pending_turns.remove(provider_turn_id);
                }
            }
            ProviderEventKind::Terminal { .. } => {
                self.finalize(state, event.clone(), true, None).await?;
                state.terminal = Some(event.clone());
            }
            _ => {}
        }
        state.track(&event)?;
        if state.terminal.is_none() {
            return Ok(false);
        }
        if state.child_turns.is_empty() && state.child_subagents.is_empty() {
            state.preview.take();
        }
        if state.terminal.as_ref().is_some_and(|event| {
            matches!(
                &event.kind,
                ProviderEventKind::Terminal {
                    outcome: TerminalOutcome::Interrupted { .. }
                        | TerminalOutcome::Cancelled { .. },
                    ..
                } | ProviderEventKind::Terminal {
                    outcome: TerminalOutcome::Failed {
                        child_turns_continue: false,
                        ..
                    },
                    ..
                }
            )
        }) {
            return Ok(true);
        }
        if !state.pending_turns.is_empty()
            || !state.child_turns.is_empty()
            || !state.child_subagents.is_empty()
            || !state.background_items.is_empty()
        {
            return Ok(false);
        }
        if state.owner_lost {
            return Ok(true);
        }
        if state.snapshot_pending_work {
            return Ok(false);
        }
        if let Some(extensions) = state.input.session.extensions()
            && extensions
                .pending_background_work(Some(state.input.state.provider_thread.clone()), false)
                .await
                .unwrap_or(false)
        {
            return Ok(false);
        }
        Ok(true)
    }

    async fn finalize(
        &self,
        state: &mut ExecutionState,
        terminal: ProviderEvent,
        persisted_failure: bool,
        abandoned: Option<OrchestrationV2ProviderTurn>,
    ) -> Result<()> {
        if state.finalized {
            return Ok(());
        }
        let now = self.ingestor.clock.now();
        let mut terminal = terminal;
        if let ProviderEventKind::Terminal {
            outcome: TerminalOutcome::Interrupted { reason },
            ..
        } = &mut terminal.kind
        {
            let records = self
                .ingestor
                .projections
                .records(
                    state.identity.thread_id.clone(),
                    vec![RecordKind::TurnItems],
                    RecordFilter {
                        turn_item_types: Some(vec!["run_interrupt_request".into()]),
                        run_ids: Some(vec![state.identity.run_id.clone()]),
                        ..Default::default()
                    },
                )
                .await?;
            for set in records.records {
                if let ProjectionRecordSet::TurnItems(items) = set {
                    for item in items {
                        if let OrchestrationV2TurnItem::RunInterruptRequest(item) = item
                            && item.run_id.as_ref() == Some(&state.input.state.run.id)
                        {
                            *reason = Some(item.message.clone());
                            break;
                        }
                    }
                }
            }
        }
        if matches!(
            &terminal.kind,
            ProviderEventKind::Terminal {
                outcome: TerminalOutcome::Interrupted { .. },
                ..
            }
        ) && self
            .ingestor
            .projections
            .thread(state.identity.thread_id.clone())
            .await
            .ok()
            .flatten()
            .is_some_and(|thread| thread.worktree_path != state.input.turn.app_thread.worktree_path)
        {
            let mut handed_off = terminal.clone();
            if let ProviderEventKind::Terminal { outcome, .. } = &mut handed_off.kind {
                *outcome = TerminalOutcome::Completed { reason: None };
            }
            let mut decision = terminal_decision(
                &state.input.state,
                &handed_off,
                &state.open,
                &now,
                self.ingestor.ids.as_ref(),
                persisted_failure,
            )?;
            decision.events.splice(
                0..0,
                cascade_terminalize_run_owned_subagents(
                    &state.input.state.run,
                    &state.open,
                    "interrupted",
                    &now,
                    self.ingestor.ids.as_ref(),
                )?,
            );
            let request = super::provider_event_ingestor::derive_id(
                "turn-item",
                &[
                    "run",
                    &state.identity.run_id.0,
                    "signal",
                    "interrupt-request",
                ],
            );
            let result = super::provider_event_ingestor::derive_id(
                "turn-item",
                &[
                    "run",
                    &state.identity.run_id.0,
                    "signal",
                    "interrupt-result",
                ],
            );
            if self
                .ingestor
                .commit_with_retry_options(
                    CommitRequest {
                        receipt: None,
                        decision,
                        guard: state.guard(),
                    },
                    CommitOptions {
                        unless_interrupt_requested: Some((
                            state.identity.thread_id.clone(),
                            request,
                            result,
                        )),
                        ..Default::default()
                    },
                )
                .await?
                .committed
            {
                self.mark_finalized(state).await?;
                return Ok(());
            }
        }
        let mut decision = terminal_decision(
            &state.input.state,
            &terminal,
            &state.open,
            &now,
            self.ingestor.ids.as_ref(),
            persisted_failure,
        )?;
        if let Some(turn) = abandoned {
            decision.events.insert(0, crate::ports::checkpoints_forks::checkpoint_capture_service::domain_event(
                self.ingestor.ids.as_ref(), &state.identity.thread_id, &now, "provider-turn.updated", turn,
                serde_json::json!({"runId":state.identity.run_id,"nodeId":state.input.state.root_node.id,"providerInstanceId":state.input.state.run.provider_instance_id}))?);
        }
        let result = self
            .ingestor
            .commit_with_retry(CommitRequest {
                receipt: None,
                decision,
                guard: state.guard(),
            })
            .await?;
        if !result.committed
            && let ProviderEventKind::Terminal {
                outcome: TerminalOutcome::Interrupted { reason },
                ..
            } = &terminal.kind
        {
            let request = TurnItemId(super::provider_event_ingestor::derive_id(
                "turn-item",
                &[
                    "run",
                    &state.identity.run_id.0,
                    "signal",
                    "interrupt-request",
                ],
            ));
            let result = TurnItemId(super::provider_event_ingestor::derive_id(
                "turn-item",
                &[
                    "run",
                    &state.identity.run_id.0,
                    "signal",
                    "interrupt-result",
                ],
            ));
            if self
                .ingestor
                .projections
                .unpaired_interrupt(state.identity.thread_id.clone(), request, result)
                .await?
            {
                let event =
                    crate::ports::checkpoints_forks::checkpoint_capture_service::domain_event(
                        self.ingestor.ids.as_ref(),
                        &state.identity.thread_id,
                        &now,
                        "turn-item.updated",
                        make_interrupt_result_turn_item(
                            &state.input.state,
                            reason.as_deref(),
                            &now,
                        )?,
                        serde_json::json!({"runId":state.identity.run_id,"nodeId":state.input.state.root_node.id,"providerInstanceId":state.input.state.run.provider_instance_id}),
                    )?;
                self.ingestor
                    .commit_with_retry(CommitRequest {
                        receipt: None,
                        decision: Decision {
                            events: vec![event],
                            ..Default::default()
                        },
                        guard: CommitGuard::None,
                    })
                    .await?;
            }
        }
        self.mark_finalized(state).await?;
        Ok(())
    }

    fn prepare_held_position(
        &self,
        state: &mut ExecutionState,
        event: &ProviderEvent,
    ) -> Result<Option<PreparedCommit>> {
        let (thread, item, run) = match &event.kind {
            ProviderEventKind::TurnItem { item, .. } => match item.as_ref() {
                OrchestrationV2TurnItem::AssistantMessage(item) if item.streaming => (
                    expanded_id(&item.thread_id)?,
                    expanded_id(&item.id)?,
                    item.run_id.as_ref().map(expanded_id).transpose()?,
                ),
                OrchestrationV2TurnItem::Reasoning(item) if item.streaming => (
                    expanded_id(&item.thread_id)?,
                    expanded_id(&item.id)?,
                    item.run_id.as_ref().map(expanded_id).transpose()?,
                ),
                _ => return Ok(None),
            },
            _ => return Ok(None),
        };
        if state.reserved_held_items.contains(&item) {
            return Ok(None);
        }
        state.reserved_held_items.insert(item.clone());
        Ok(Some(PreparedCommit {
            request: CommitRequest {
                receipt: None,
                decision: Decision::default(),
                guard: CommitGuard::None,
            },
            options: CommitOptions::default(),
            prepare: Some(Box::new(move |tx, request| {
                crate::stores::positions::allocate(
                    tx,
                    &thread,
                    &item,
                    run.as_ref(),
                    None,
                    crate::stores::positions::Placement::Append,
                )?;
                Ok(request)
            })),
        }))
    }

    async fn reserve_held_item(
        &self,
        state: &mut ExecutionState,
        event: &ProviderEvent,
    ) -> Result<()> {
        if let Some(input) = self.prepare_held_position(state, event)? {
            self.ingestor
                .committer
                .commit_prepared(
                    input.request,
                    input.options,
                    input.prepare.expect("held position preparation"),
                )
                .await?;
        }
        Ok(())
    }

    async fn mark_finalized(&self, state: &mut ExecutionState) -> Result<()> {
        state.finalized = true;
        if let Some(host) = &self.host {
            let project = expanded_id(&state.input.turn.app_thread.project_id)?;
            let _ = host.refresh_after_turn(project).await;
        }
        Ok(())
    }

    async fn interrupt_abandoned(
        &self,
        state: &ExecutionState,
    ) -> Option<OrchestrationV2ProviderTurn> {
        let turn_id = state.routing.root_provider_turn_id.clone()?;
        state
            .input
            .session
            .interrupt_turn(InterruptInput {
                provider_thread: state.input.state.provider_thread.clone(),
                provider_turn_id: turn_id,
                request_runtime_restart: true,
            })
            .await
            .ok()?;
        let mut turn = state.latest_root_turn.clone()?;
        if turn.status == ProviderTurnStatus::Running {
            turn.status = ProviderTurnStatus::Interrupted;
            turn.completed_at = Some(self.ingestor.clock.now());
        }
        Some(turn)
    }
}

struct ExecutionState {
    identity: ProviderEventRouteIdentity,
    routing: ProviderEventRoutingState,
    input: StartRootRun,
    finalized: bool,
    terminal: Option<ProviderEvent>,
    latest_root_turn: Option<OrchestrationV2ProviderTurn>,
    first_root_turn_id: Option<ProviderTurnId>,
    latest_item_ordinal: u64,
    ownership_checked: bool,
    owner_lost: bool,
    snapshot_pending_work: bool,
    pending_turns: HashSet<ProviderTurnId>,
    child_turns: HashSet<ProviderTurnId>,
    child_subagents: HashMap<NodeId, ThreadId>,
    background_items: HashMap<TurnItemId, ThreadId>,
    open: OpenRunOwnedSubagents,
    preview: Option<PreviewGuard>,
    streaming: AssistantStreamingFilter,
    reserved_held_items: HashSet<TurnItemId>,
}

impl ExecutionState {
    fn new(input: StartRootRun) -> Result<Self> {
        let identity = ProviderEventRouteIdentity {
            thread_id: expanded_id(&input.state.run.thread_id)?,
            run_id: expanded_id(&input.state.run.id)?,
            attempt_id: expanded_id(&input.state.attempt.id)?,
            provider_thread_id: expanded_id(&input.state.provider_thread.id)?,
        };
        let mut routing = ProviderEventRoutingState::new(
            &identity,
            input
                .state
                .attempt
                .provider_turn_id
                .as_ref()
                .map(expanded_id)
                .transpose()?,
        );
        routing
            .owned_thread_ids
            .extend(input.related_thread_ids.iter().cloned());
        routing
            .owned_provider_thread_ids
            .extend(input.related_provider_thread_ids.iter().cloned());
        routing.inherited_background_turn_items = input.inherited_background_items.clone();
        let background_items = input
            .inherited_background_items
            .keys()
            .map(|id| (id.clone(), identity.thread_id.clone()))
            .collect();
        let ordinal = input
            .turn
            .provider_turn_ordinal
            .checked_mul(100)
            .ok_or_else(|| Error::Service {
                operation: "RunExecutionService.startRootRun",
                detail: "Turn item ordinal overflow.".into(),
            })?;
        Ok(Self {
            identity,
            routing,
            input,
            finalized: false,
            terminal: None,
            latest_root_turn: None,
            first_root_turn_id: None,
            latest_item_ordinal: ordinal,
            ownership_checked: false,
            owner_lost: false,
            snapshot_pending_work: false,
            pending_turns: HashSet::new(),
            child_turns: HashSet::new(),
            child_subagents: HashMap::new(),
            background_items,
            open: OpenRunOwnedSubagents::default(),
            preview: None,
            streaming: AssistantStreamingFilter::new(false, 800),
            reserved_held_items: HashSet::new(),
        })
    }

    fn guard(&self) -> CommitGuard {
        CommitGuard::RunCurrent {
            thread_id: self.identity.thread_id.clone(),
            run_id: self.identity.run_id.clone(),
            attempt_id: self.identity.attempt_id.clone(),
            status: OrchestrationV2RunStatus::Running,
        }
    }

    fn analytics_context(&mut self, event: &ProviderEvent) -> Result<ProviderTurnAnalyticsContext> {
        let native_child = matches!(&event.kind, ProviderEventKind::Turn { provider_turn, .. }
            if provider_turn.provider_thread_id != self.input.state.provider_thread.id);
        let mut segment = ProviderTurnSegment::Attempt(self.input.state.attempt.reason.clone());
        if native_child {
            segment = ProviderTurnSegment::NativeChild;
        } else if let ProviderEventKind::Turn { provider_turn, .. } = &event.kind {
            let id = expanded_id(&provider_turn.id)?;
            if self.first_root_turn_id.get_or_insert(id.clone()) != &id {
                segment = ProviderTurnSegment::Continuation;
            }
        }
        let mut context = root_analytics_context(&self.input, segment)?;
        if native_child {
            context.origin = Some(ProviderTurnOrigin {
                created_by: OrchestrationV2Actor::Agent,
                creation_source: OrchestrationV2CreationSource::Provider,
                scheduled: false,
                subagent: true,
            });
        }
        Ok(context)
    }

    fn failed_terminal(&self, error: &Error) -> Result<ProviderEvent> {
        Ok(ProviderEvent {
            driver: self.input.state.provider_thread.driver.clone(),
            kind: ProviderEventKind::Terminal {
                provider_thread_id: self.identity.provider_thread_id.clone(),
                provider_turn_id: self
                    .routing
                    .root_provider_turn_id
                    .clone()
                    .unwrap_or_else(|| {
                        ProviderTurnId(super::provider_event_ingestor::derive_id(
                            "provider-turn",
                            &[
                                "provider",
                                &self.input.state.provider_thread.driver.0,
                                "native-turn",
                                &format!("failed:{}", self.identity.attempt_id.0),
                            ],
                        ))
                    }),
                run_ordinal: self.input.state.run.ordinal,
                disposition: ThreadDisposition::Reusable,
                outcome: TerminalOutcome::Failed {
                    failure: Box::new(serde_json::from_value(
                        serde_json::json!({"class":"provider_error",
                "message":super::provider_turn_start_service::provider_failure_message(error),"code":null,"retryable":null}),
                    )?),
                    failure_item_ordinal: self
                        .latest_item_ordinal
                        .checked_add(1)
                        .ok_or(Error::Unsupported("turn item ordinal overflow"))?,
                    retry: None,
                    retry_started_at: None,
                    child_turns_continue: false,
                },
            },
        })
    }

    fn track(&mut self, event: &ProviderEvent) -> Result<()> {
        match &event.kind {
            ProviderEventKind::Turn { provider_turn, .. } => {
                if provider_turn.run_attempt_id.as_ref() != Some(&self.input.state.attempt.id) {
                    let id = expanded_id(&provider_turn.id)?;
                    if provider_turn.status == ProviderTurnStatus::Running {
                        self.child_turns.insert(id);
                    } else {
                        self.child_turns.remove(&id);
                    }
                }
            }
            ProviderEventKind::Subagent(agent) => {
                let id = expanded_id(&agent.id)?;
                let thread_id = expanded_id(&agent.thread_id)?;
                let active = work_active(&t3_contracts::stored::to_value(&agent.status)?);
                if active {
                    self.child_subagents.insert(id.clone(), thread_id.clone());
                } else {
                    self.child_subagents.remove(&id);
                }
                if let Some(child) = &agent.child_thread_id {
                    self.open
                        .linked_child_thread_ids
                        .insert(expanded_id(child)?);
                }
                self.open
                    .subagents
                    .retain(|agent| agent.id != IdOrIndex::Id(id.clone()));
                if active {
                    self.open.subagents.push(*agent.clone());
                }
            }
            ProviderEventKind::Node(node) if node.id != self.input.state.root_node.id => {
                self.open.nodes.retain(|saved| saved.id != node.id);
                if work_active(&t3_contracts::stored::to_value(&node.status)?) {
                    self.open.nodes.push(*node.clone());
                }
            }
            ProviderEventKind::TurnItem { item, .. } => {
                let value = t3_contracts::stored::to_value(&item)?;
                let id: TurnItemId = serde_json::from_value(value["id"].clone())?;
                let thread = serde_json::from_value(value["threadId"].clone())?;
                let active = work_active(&value["status"]);
                if background_capable(&value) {
                    if active {
                        self.background_items.insert(id.clone(), thread);
                    } else {
                        self.background_items.remove(&id);
                    }
                }
                for index in (0..self.open.child_turn_items.len()).rev() {
                    if t3_contracts::stored::to_value(&self.open.child_turn_items[index])?["id"]
                        == id.0
                    {
                        self.open.child_turn_items.remove(index);
                    }
                }
                if value["threadId"] != self.identity.thread_id.0 && active {
                    self.open.child_turn_items.push(*item.clone());
                }
                if let OrchestrationV2TurnItem::Subagent(agent) = item.as_ref() {
                    self.open
                        .turn_items
                        .retain(|saved| saved.subagent_id != agent.subagent_id);
                    if let Some(child) = &agent.child_thread_id {
                        self.open
                            .linked_child_thread_ids
                            .insert(expanded_id(child)?);
                    }
                    if active {
                        self.open.turn_items.push(*agent.clone());
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn detach_moved_child(&mut self, event: &ProviderEvent) -> Result<()> {
        if let ProviderEventKind::Subagent(agent) = &event.kind
            && agent.origin == SubagentOrigin::ProviderNative
            && let Some(child) = &agent.child_thread_id
        {
            let child = expanded_id(child)?;
            if self.routing.owned_thread_ids.contains(&child) {
                return Ok(());
            }
            self.open.linked_child_thread_ids.remove(&child);
            self.open.subagents.retain(|saved| {
                saved.id != agent.id && saved.thread_id != IdOrIndex::Id(child.clone())
            });
            self.open.nodes.retain(|saved| {
                saved.thread_id != IdOrIndex::Id(child.clone()) && saved.id != agent.id
            });
            self.open
                .turn_items
                .retain(|saved| saved.subagent_id != agent.id);
            for index in (0..self.open.child_turn_items.len()).rev() {
                if t3_contracts::stored::to_value(&self.open.child_turn_items[index])?["threadId"]
                    == child.0
                {
                    self.open.child_turn_items.remove(index);
                }
            }
            self.background_items.retain(|_, thread| thread != &child);
            let agent_id = expanded_id(&agent.id)?;
            self.child_subagents
                .retain(|id, thread| id != &agent_id && thread != &child);
            self.child_turns
                .retain(|id| self.routing.owned_provider_turns.contains_key(id));
            self.pending_turns
                .retain(|id| self.routing.owned_provider_turns.contains_key(id));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ProviderEventRouteIdentity {
    pub thread_id: ThreadId,
    pub run_id: RunId,
    pub attempt_id: RunAttemptId,
    pub provider_thread_id: ProviderThreadId,
}

#[derive(Debug, Clone)]
pub struct ProviderEventRoutingState {
    pub owned_thread_ids: HashSet<ThreadId>,
    pub root_turn_ended: bool,
    pub workflow_member_parents: HashMap<ThreadId, ThreadId>,
    pub owned_provider_thread_ids: HashSet<ProviderThreadId>,
    pub owned_provider_turns: HashMap<ProviderTurnId, ProviderThreadId>,
    pub inherited_background_turn_items: HashMap<TurnItemId, RunId>,
    pub root_provider_turn_id: Option<ProviderTurnId>,
}

impl ProviderEventRoutingState {
    pub fn new(
        identity: &ProviderEventRouteIdentity,
        provider_turn_id: Option<ProviderTurnId>,
    ) -> Self {
        Self {
            owned_thread_ids: HashSet::from([identity.thread_id.clone()]),
            root_turn_ended: false,
            workflow_member_parents: HashMap::new(),
            owned_provider_thread_ids: HashSet::from([identity.provider_thread_id.clone()]),
            owned_provider_turns: provider_turn_id
                .clone()
                .map(|id| (id, identity.provider_thread_id.clone()))
                .into_iter()
                .collect(),
            inherited_background_turn_items: HashMap::new(),
            root_provider_turn_id: provider_turn_id,
        }
    }

    pub fn route(
        &mut self,
        event: &ProviderEventKind,
        input: &ProviderEventRouteIdentity,
    ) -> Result<bool> {
        let owns_child = |state: &Self, thread_id: &ThreadId| {
            *thread_id != input.thread_id && state.owned_thread_ids.contains(thread_id)
        };
        let owns_run = |run: &Option<IdOrIndex<RunId>>| {
            run.as_ref() == Some(&IdOrIndex::Id(input.run_id.clone()))
        };
        Ok(match event {
            ProviderEventKind::Session(_)
            | ProviderEventKind::CommandOutput { .. }
            | ProviderEventKind::PromptSuggestion { .. } => false,
            ProviderEventKind::AppThread { app_thread, .. } => {
                let id = expanded_id(&app_thread.id)?;
                if id == input.thread_id {
                    return Ok(true);
                }
                let lineage = t3_contracts::stored::to_value(&app_thread.lineage)?;
                let parent = lineage["parentThreadId"]
                    .as_str()
                    .map(|id| ThreadId(id.into()));
                let belongs = lineage["relationshipToParent"] == "subagent"
                    && parent.as_ref().is_some_and(|parent| {
                        self.owned_thread_ids.contains(parent)
                            && (!self.root_turn_ended
                                || self.workflow_member_parents.get(&id) == Some(parent))
                    });
                if belongs {
                    self.owned_thread_ids.insert(id);
                }
                belongs
            }
            ProviderEventKind::Thread {
                provider_thread, ..
            } => {
                let id = expanded_id(&provider_thread.id)?;
                let app_id = provider_thread
                    .app_thread_id
                    .as_ref()
                    .map(expanded_id)
                    .transpose()?;
                let belongs = self.owned_provider_thread_ids.contains(&id)
                    || app_id
                        .as_ref()
                        .is_some_and(|id| self.owned_thread_ids.contains(id));
                if belongs {
                    self.owned_provider_thread_ids.insert(id);
                }
                belongs
            }
            ProviderEventKind::Turn {
                provider_turn,
                thread_id,
            } => {
                let id = expanded_id(&provider_turn.id)?;
                let thread = expanded_id(&provider_turn.provider_thread_id)?;
                let root =
                    provider_turn.run_attempt_id == Some(IdOrIndex::Id(input.attempt_id.clone()));
                let belongs = root
                    || (thread != input.provider_thread_id
                        && self.owned_provider_thread_ids.contains(&thread))
                    || self.owned_provider_turns.contains_key(&id)
                    || thread_id.as_ref().is_some_and(|id| owns_child(self, id));
                if belongs {
                    self.owned_provider_turns.insert(id.clone(), thread);
                    if root {
                        self.root_provider_turn_id = Some(id);
                    }
                }
                belongs
            }
            ProviderEventKind::Node(node) => {
                let belongs =
                    owns_run(&node.run_id) || owns_child(self, &expanded_id(&node.thread_id)?);
                if belongs && let Some(thread) = &node.provider_thread_id {
                    self.owned_provider_thread_ids.insert(expanded_id(thread)?);
                }
                belongs
            }
            ProviderEventKind::Subagent(agent) => {
                let thread = expanded_id(&agent.thread_id)?;
                let child = agent
                    .child_thread_id
                    .as_ref()
                    .map(expanded_id)
                    .transpose()?;
                let native = t3_contracts::stored::to_value(&agent.origin)? == "provider_native";
                if native
                    && thread == input.thread_id
                    && agent.run_id.is_some()
                    && !owns_run(&agent.run_id)
                    && child.as_ref().is_some_and(|child| owns_child(self, child))
                {
                    if let Some(child) = child {
                        self.owned_thread_ids.remove(&child);
                    }
                    if let Some(thread) = &agent.provider_thread_id {
                        let thread = expanded_id(thread)?;
                        self.owned_provider_thread_ids.remove(&thread);
                        self.owned_provider_turns
                            .retain(|_, owned| owned != &thread);
                    }
                    return Ok(false);
                }
                let belongs = owns_run(&agent.run_id) || owns_child(self, &thread);
                if owns_run(&agent.run_id)
                    && !self.root_turn_ended
                    && native
                    && work_active(&t3_contracts::stored::to_value(&agent.status)?)
                    && let Some(child) = &child
                {
                    self.owned_thread_ids.insert(child.clone());
                }
                let workflow = t3_contracts::stored::to_value(&agent.workflow)?;
                if owns_run(&agent.run_id)
                    && let Some(child) = &child
                    && let Some(agents) = workflow.get("agents").and_then(Value::as_array)
                {
                    for member in agents {
                        if let Some(member) = member.get("childThreadId").and_then(Value::as_str) {
                            self.workflow_member_parents
                                .insert(ThreadId(member.into()), child.clone());
                        }
                    }
                    return Ok(true);
                }
                belongs
            }
            ProviderEventKind::Message(message) => {
                owns_run(&message.run_id) || owns_child(self, &expanded_id(&message.thread_id)?)
            }
            ProviderEventKind::Steer { run_id, .. } => *run_id == input.run_id,
            ProviderEventKind::TurnItem { item, .. } => {
                let item = t3_contracts::stored::to_value(&item)?;
                let thread: ThreadId = serde_json::from_value(item["threadId"].clone())?;
                let run: Option<RunId> = serde_json::from_value(item["runId"].clone())?;
                if run.as_ref() == Some(&input.run_id) || owns_child(self, &thread) {
                    return Ok(true);
                }
                let id: TurnItemId = serde_json::from_value(item["id"].clone())?;
                let inherited = thread == input.thread_id
                    && run.is_some()
                    && run.as_ref() == self.inherited_background_turn_items.get(&id)
                    && background_capable(&item);
                if inherited && !work_active(&item["status"]) {
                    self.inherited_background_turn_items.remove(&id);
                }
                inherited
            }
            ProviderEventKind::Plan(plan) => {
                let plan = t3_contracts::stored::to_value(&plan)?;
                let run: Option<RunId> = serde_json::from_value(plan["runId"].clone())?;
                run.as_ref() == Some(&input.run_id)
                    || owns_child(self, &serde_json::from_value(plan["threadId"].clone())?)
            }
            ProviderEventKind::RuntimeRequest { request, thread_id } => {
                thread_id.as_ref().is_some_and(|id| owns_child(self, id))
                    || request
                        .provider_turn_id
                        .as_ref()
                        .map(expanded_id)
                        .transpose()?
                        .as_ref()
                        .is_some_and(|id| self.owned_provider_turns.contains_key(id))
            }
            ProviderEventKind::BackgroundWork {
                provider_turn_id, ..
            } => self.owned_provider_turns.contains_key(provider_turn_id),
            ProviderEventKind::Terminal {
                provider_turn_id, ..
            } => {
                let root = self.root_provider_turn_id.as_ref() == Some(provider_turn_id);
                if root {
                    self.root_turn_ended = true;
                }
                root
            }
        })
    }
}

pub fn work_active(status: &Value) -> bool {
    matches!(status.as_str(), Some("pending" | "running" | "waiting"))
}

fn background_capable(item: &Value) -> bool {
    matches!(
        item["type"].as_str(),
        Some("command_execution" | "dynamic_tool" | "subagent")
    )
}

pub fn can_route_related_subagent(status: &OrchestrationV2TurnItemStatus) -> bool {
    *status == OrchestrationV2TurnItemStatus::Completed
}

pub fn select_inherited_background_turn_items(
    thread_id: &ThreadId,
    provider_thread_id: &ProviderThreadId,
    ordinal: u64,
    runs: &[OrchestrationV2Run],
    items: &[OrchestrationV2TurnItem],
) -> Result<HashMap<TurnItemId, RunId>> {
    let prior: HashSet<_> = runs
        .iter()
        .filter(|run| {
            run.thread_id == IdOrIndex::Id(thread_id.clone())
                && run.ordinal < ordinal
                && matches!(
                    run.status,
                    OrchestrationV2RunStatus::Interrupted
                        | OrchestrationV2RunStatus::Failed
                        | OrchestrationV2RunStatus::Cancelled
                )
        })
        .map(|run| expanded_id(&run.id))
        .collect::<Result<_>>()?;
    let mut inherited = HashMap::new();
    for item in items {
        let value = t3_contracts::stored::to_value(&item)?;
        if value["threadId"] == thread_id.0
            && value["providerThreadId"] == provider_thread_id.0
            && background_capable(&value)
            && work_active(&value["status"])
            && let Some(run_id) = value["runId"].as_str()
        {
            let run_id = RunId(run_id.into());
            if prior.contains(&run_id) {
                inherited.insert(serde_json::from_value(value["id"].clone())?, run_id);
            }
        }
    }
    Ok(inherited)
}

#[derive(Debug, Clone)]
pub struct RootRunState {
    pub run: OrchestrationV2Run,
    pub root_node: OrchestrationV2ExecutionNode,
    pub checkpoint_scope: OrchestrationV2CheckpointScope,
    pub provider_thread: OrchestrationV2ProviderThread,
    pub attempt: OrchestrationV2RunAttempt,
}

#[derive(Debug, Clone, Default)]
pub struct OpenRunOwnedSubagents {
    pub subagents: Vec<OrchestrationV2Subagent>,
    pub turn_items: Vec<TurnItemSubagent>,
    pub child_turn_items: Vec<OrchestrationV2TurnItem>,
    pub nodes: Vec<OrchestrationV2ExecutionNode>,
    pub linked_child_thread_ids: HashSet<ThreadId>,
}

pub fn cascade_terminalize_run_owned_subagents(
    run: &OrchestrationV2Run,
    open: &OpenRunOwnedSubagents,
    status: &str,
    now: &IsoDateTime,
    ids: &dyn IdGenerator,
) -> Result<Vec<OrchestrationV2DomainEvent>> {
    if !["interrupted", "failed", "cancelled"].contains(&status) {
        return Err(Error::Unsupported("nonterminal subagent cascade"));
    }
    use crate::ports::checkpoints_forks::checkpoint_capture_service::domain_event;
    let mut child_threads = open.linked_child_thread_ids.clone();
    for child in open
        .subagents
        .iter()
        .filter_map(|agent| agent.child_thread_id.as_ref())
        .chain(
            open.turn_items
                .iter()
                .filter_map(|item| item.child_thread_id.as_ref()),
        )
    {
        child_threads.insert(expanded_id(child)?);
    }
    let mut keys = Vec::new();
    for key in open
        .subagents
        .iter()
        .map(|agent| &agent.id)
        .chain(open.turn_items.iter().map(|item| &item.subagent_id))
        .chain(open.nodes.iter().map(|node| &node.id))
    {
        if !keys.contains(key) {
            keys.push(key.clone());
        }
    }
    let mut events = Vec::new();
    for key in keys {
        if let Some(agent) = open.subagents.iter().find(|agent| agent.id == key)
            && work_active(&t3_contracts::stored::to_value(&agent.status)?)
        {
            let mut payload = t3_contracts::stored::to_value(&agent)?;
            payload["status"] = status.into();
            payload["completedAt"] = t3_contracts::stored::to_value(&now)?;
            payload["updatedAt"] = t3_contracts::stored::to_value(&now)?;
            events.push(domain_event(ids,&expanded_id(&agent.thread_id)?,now,"subagent.updated",payload,serde_json::json!({"runId":run.id,"nodeId":agent.id,"driver":agent.driver,"providerInstanceId":agent.provider_instance_id}))?);
        }
        if let Some(node) = open.nodes.iter().find(|node| node.id == key)
            && ((node.thread_id == run.thread_id && node.run_id.as_ref() == Some(&run.id))
                || child_threads.contains(&expanded_id(&node.thread_id)?))
            && matches!(
                node.status,
                ExecutionNodeStatus::Pending
                    | ExecutionNodeStatus::Running
                    | ExecutionNodeStatus::Waiting
            )
        {
            let mut payload = t3_contracts::stored::to_value(&node)?;
            payload["status"] = status.into();
            payload["completedAt"] = t3_contracts::stored::to_value(&now)?;
            events.push(domain_event(ids,&expanded_id(&node.thread_id)?,now,"node.updated",payload,serde_json::json!({"runId":node.run_id.as_ref().unwrap_or(&run.id),"nodeId":node.id,"providerInstanceId":run.provider_instance_id}))?);
        }
        if let Some(item) = open.turn_items.iter().find(|item| item.subagent_id == key)
            && item.run_id.as_ref() == Some(&run.id)
            && work_active(&t3_contracts::stored::to_value(&item.status)?)
        {
            let mut payload = t3_contracts::stored::to_value(&OrchestrationV2TurnItem::Subagent(
                Box::new(item.clone()),
            ))?;
            payload["status"] = status.into();
            payload["completedAt"] = t3_contracts::stored::to_value(&now)?;
            payload["updatedAt"] = t3_contracts::stored::to_value(&now)?;
            let mut metadata =
                serde_json::json!({"runId":run.id,"providerInstanceId":run.provider_instance_id});
            if let Some(node) = &item.node_id {
                metadata["nodeId"] = t3_contracts::stored::to_value(&node)?;
            }
            events.push(domain_event(
                ids,
                &expanded_id(&item.thread_id)?,
                now,
                "turn-item.updated",
                payload,
                metadata,
            )?);
        }
    }
    for item in &open.child_turn_items {
        let mut payload = t3_contracts::stored::to_value(&item)?;
        let thread_id: ThreadId = serde_json::from_value(payload["threadId"].clone())?;
        if !child_threads.contains(&thread_id) || !work_active(&payload["status"]) {
            continue;
        }
        let mut metadata = serde_json::json!({"runId":if payload["runId"].is_null() {t3_contracts::stored::to_value(&run.id)?}else{payload["runId"].clone()},"providerInstanceId":run.provider_instance_id});
        if !payload["nodeId"].is_null() {
            metadata["nodeId"] = payload["nodeId"].clone();
        }
        if payload.get("streaming").is_some() {
            payload["streaming"] = false.into();
        }
        payload["status"] = status.into();
        payload["completedAt"] = t3_contracts::stored::to_value(&now)?;
        payload["updatedAt"] = t3_contracts::stored::to_value(&now)?;
        events.push(domain_event(
            ids,
            &thread_id,
            now,
            "turn-item.updated",
            payload,
            metadata,
        )?);
    }
    Ok(events)
}

pub const USER_STOP_REASON: &str = "Stopped by user";

pub fn make_interrupt_result_turn_item(
    state: &RootRunState,
    reason: Option<&str>,
    now: &IsoDateTime,
) -> Result<OrchestrationV2TurnItem> {
    use super::provider_event_ingestor::derive_id;
    let run_id = expanded_id(&state.run.id)?;
    Ok(serde_json::from_value(
        serde_json::json!({"id":derive_id("turn-item", &["run",&run_id.0,"signal","interrupt-result"]),"threadId":state.run.thread_id,"runId":run_id,"nodeId":state.root_node.id,"providerThreadId":state.provider_thread.id,"providerTurnId":state.root_node.provider_turn_id,"nativeItemRef":null,"parentItemId":derive_id("turn-item", &["run",&run_id.0,"signal","interrupt-request"]),"ordinal":state.run.ordinal.checked_mul(100).and_then(|value|value.checked_add(98)).ok_or_else(||Error::Decode {entity:"interrupt ordinal",detail:"integer overflow".into()})?,"status":"interrupted","title":"Interrupted","startedAt":now,"completedAt":now,"updatedAt":now,"type":"run_interrupt_result","message":reason.unwrap_or("Stopped by the provider")}),
    )?)
}

pub fn terminal_decision(
    state: &RootRunState,
    terminal: &ProviderEvent,
    open: &OpenRunOwnedSubagents,
    now: &IsoDateTime,
    ids: &dyn IdGenerator,
    failure_item_persisted: bool,
) -> Result<Decision> {
    use super::provider_event_ingestor::terminal_failure_item;
    use crate::ports::checkpoints_forks::checkpoint_capture_service::domain_event;
    let ProviderEventKind::Terminal {
        outcome,
        disposition,
        ..
    } = &terminal.kind
    else {
        return Err(Error::Unsupported("nonterminal run finalization"));
    };
    let thread_id = expanded_id(&state.run.thread_id)?;
    let run_id = expanded_id(&state.run.id)?;
    let node_id = expanded_id(&state.root_node.id)?;
    let status = match outcome {
        TerminalOutcome::Completed { .. } => "completed",
        TerminalOutcome::Interrupted { .. } => "interrupted",
        TerminalOutcome::Cancelled { .. } => "cancelled",
        TerminalOutcome::Failed { .. } => "failed",
    };
    let persisted = if status == "completed" {
        "waiting"
    } else {
        status
    };
    let children_continue = matches!(
        outcome,
        TerminalOutcome::Failed {
            child_turns_continue: true,
            ..
        }
    );
    let mut decision = Decision::default();
    if status != "completed" && !children_continue {
        decision
            .events
            .extend(cascade_terminalize_run_owned_subagents(
                &state.run, open, status, now, ids,
            )?);
    }
    let metadata = serde_json::json!({"runId":run_id,"nodeId":node_id,"providerInstanceId":state.run.provider_instance_id});
    let mut attempt = t3_contracts::stored::to_value(&state.attempt)?;
    attempt["status"] = status.into();
    attempt["completedAt"] = t3_contracts::stored::to_value(&now)?;
    decision.events.push(domain_event(
        ids,
        &thread_id,
        now,
        "run-attempt.updated",
        attempt,
        metadata.clone(),
    )?);
    if let TerminalOutcome::Interrupted { reason } = outcome {
        decision.events.push(domain_event(
            ids,
            &thread_id,
            now,
            "turn-item.updated",
            make_interrupt_result_turn_item(state, reason.as_deref(), now)?,
            metadata.clone(),
        )?);
    }
    if status == "failed"
        && !failure_item_persisted
        && let Some(item) =
            terminal_failure_item(terminal, &thread_id, Some(&run_id), Some(&node_id), now)?
    {
        decision.events.push(domain_event(
            ids,
            &thread_id,
            now,
            "turn-item.updated",
            item,
            metadata.clone(),
        )?);
    }
    let mut run = t3_contracts::stored::to_value(&state.run)?;
    run.as_object_mut()
        .ok_or_else(|| Error::Decode {
            entity: "run",
            detail: "expected object".into(),
        })?
        .shift_remove("delegatedCompletion");
    run["status"] = persisted.into();
    run["completedAt"] = if status == "completed" {
        Value::Null
    } else {
        t3_contracts::stored::to_value(&now)?
    };
    let mut node = t3_contracts::stored::to_value(&state.root_node)?;
    node["status"] = persisted.into();
    node["completedAt"] = run["completedAt"].clone();
    node["checkpointScopeId"] = t3_contracts::stored::to_value(&state.checkpoint_scope.id)?;
    let mut thread = t3_contracts::stored::to_value(&state.provider_thread)?;
    thread["status"] = if *disposition == ThreadDisposition::Broken {
        "error"
    } else {
        "idle"
    }
    .into();
    thread["updatedAt"] = t3_contracts::stored::to_value(&now)?;
    decision.events.push(domain_event(
        ids,
        &thread_id,
        now,
        "run.updated",
        run,
        metadata.clone(),
    )?);
    decision.events.push(domain_event(
        ids,
        &thread_id,
        now,
        "node.updated",
        node,
        metadata,
    )?);
    decision.events.push(domain_event(
        ids,
        &thread_id,
        now,
        "provider-thread.updated",
        thread,
        serde_json::json!({"providerInstanceId":state.run.provider_instance_id}),
    )?);
    if status != "failed" {
        decision.effects.push(PendingEffect {
            id: format!("effect:checkpoint.capture:{}", run_id.0),
            command_id: CommandId(format!("command:effect:checkpoint.capture:{}", run_id.0)),
            thread_id,
            request: EffectRequest::Capture {
                run_id,
                scope_id: expanded_id(&state.checkpoint_scope.id)?,
            },
            available_at: None,
        });
    }
    Ok(decision)
}
