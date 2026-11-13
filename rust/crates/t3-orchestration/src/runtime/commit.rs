use crate::{
    api::{
        Clock, CommitGuard, CommitPreparation, CommitRequest, CommitResult, EventCommitter,
        ProjectionWriter,
    },
    error::{Error, Result},
    ports::{
        runs_ingestion::run_metrics::{RunOutcome, RunOutcomeRecorder},
        transport::{
            live_stream_budget::{LiveBuffer, LiveStreamLimits, SharedLiveItem},
            wire_projection::WireEventCache,
        },
    },
    runtime::worker::{OutboxSignals, task_error},
    stores::{
        StoreDatabase,
        event_store::{EventFilter, EventPages, EventStore, timestamp},
        outbox, positions, projects,
        receipts::{self, AggregateId, CommandReceipt, ReceiptStatus},
    },
};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex as StdMutex, Weak},
};
use t3_contracts::{
    application_event::{ApplicationProjectEvent, ApplicationStoredEvent},
    base_schemas::{CommandId, ThreadId},
    orchestration_v2::{
        OrchestrationV2DomainEvent, OrchestrationV2ProviderThread, OrchestrationV2StoredEvent,
    },
};
use t3_persistence::rusqlite::{Connection, OptionalExtension, Transaction, params};
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Debug, Clone, Default)]
pub struct WriteOptions {
    pub command_id: Option<CommandId>,
    pub unless_native_thread_owned: Option<OrchestrationV2ProviderThread>,
    pub guard_pending_user_input_cancellations: bool,
    pub update_delegated_completion_only_run_id: Option<String>,
    pub update_all_delegated_completions_only: bool,
    pub unless_interrupt_requested: Option<(ThreadId, String, String)>,
}

pub(crate) struct BatchJob {
    pub owner: CancellationToken,
    pub scope: Option<CancellationToken>,
    pub request: CommitRequest,
    pub options: WriteOptions,
    pub prepare: Option<CommitPreparation>,
}

#[async_trait]
pub(crate) trait CommitBarrier: Send + Sync {
    async fn wait(&self) -> Result<()>;
}

#[derive(Clone)]
pub struct EventSink(Arc<SinkState>);

struct SinkState {
    stores: StoreDatabase,
    events: EventStore,
    projections: Arc<dyn ProjectionWriter>,
    clock: Arc<dyn Clock>,
    signals: OutboxSignals,
    publication: broadcast::Sender<ApplicationStoredEvent>,
    shared_publication: broadcast::Sender<Arc<ApplicationStoredEvent>>,
    lane: Arc<Mutex<()>>,
    commits: TaskTracker,
    barrier: StdMutex<Option<Weak<dyn CommitBarrier>>>,
    streams: StdMutex<Vec<Weak<StreamSubscription>>>,
    wire_cache: WireEventCache,
    outcomes: RunOutcomeRecorder,
    outcome_events: broadcast::Sender<RunOutcome>,
}

#[derive(Debug, Clone, Default)]
pub struct StreamOptions {
    pub thread_id: Option<ThreadId>,
    pub after_sequence: u64,
    pub event_type: Option<String>,
    pub bounded: bool,
}

struct StreamSubscription {
    options: StreamOptions,
    queue: StreamQueue,
}

enum StreamQueue {
    Internal(mpsc::UnboundedSender<Arc<OrchestrationV2StoredEvent>>),
    Bounded(Arc<LiveBuffer<OrchestrationV2StoredEvent>>),
}

impl StreamSubscription {
    fn matches(&self, event: &OrchestrationV2StoredEvent, thread: &ThreadId, kind: &str) -> bool {
        event.sequence > self.options.after_sequence
            && self
                .options
                .thread_id
                .as_ref()
                .is_none_or(|id| id == thread)
            && self
                .options
                .event_type
                .as_ref()
                .is_none_or(|expected| expected == kind)
    }
}

pub struct EventStream {
    subscription: Arc<StreamSubscription>,
    replay: EventPages<OrchestrationV2StoredEvent>,
    replay_done: bool,
    pending: VecDeque<OrchestrationV2StoredEvent>,
    live: Option<mpsc::UnboundedReceiver<Arc<OrchestrationV2StoredEvent>>>,
    through: u64,
    cache: Arc<SinkState>,
}

impl EventStream {
    pub async fn next(&mut self) -> Result<Option<OrchestrationV2StoredEvent>> {
        if let StreamQueue::Bounded(buffer) = &self.subscription.queue {
            let buffer = buffer.clone();
            tokio::select! {
                biased;
                _ = buffer.failed() => Err(Error::Lagged),
                result = self.next_inner() => result,
            }
        } else {
            self.next_inner().await
        }
    }

    async fn next_inner(&mut self) -> Result<Option<OrchestrationV2StoredEvent>> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                if !self.replay_done {
                    if let StreamQueue::Bounded(buffer) = &self.subscription.queue {
                        let event = self.cache.wire_cache.project(&Arc::new(event))?;
                        buffer.deliver_external(SharedLiveItem::new(
                            (*event).clone(),
                            event.sequence,
                        )?)?;
                        return Ok(Some((*event).clone()));
                    }
                    return Ok(Some(event));
                }
                if event.sequence > self.through {
                    return Ok(Some(event));
                }
                continue;
            }
            if !self.replay_done {
                match self.replay.next_page().await? {
                    Some(events) => self.pending.extend(events),
                    None => self.replay_done = true,
                }
                continue;
            }
            match &self.subscription.queue {
                StreamQueue::Internal(_) => {
                    let Some(live) = &mut self.live else {
                        return Ok(None);
                    };
                    match live.recv().await {
                        Some(event) if event.sequence > self.through => {
                            return Ok(Some((*event).clone()));
                        }
                        Some(_) => {}
                        None => return Ok(None),
                    }
                }
                StreamQueue::Bounded(buffer) => match buffer.pull().await? {
                    Some(events) => self
                        .pending
                        .extend(events.iter().map(|event| event.value.clone())),
                    None => return Ok(None),
                },
            }
        }
    }
}

impl EventSink {
    pub fn new(
        stores: StoreDatabase,
        projections: Arc<dyn ProjectionWriter>,
        clock: Arc<dyn Clock>,
        signals: OutboxSignals,
    ) -> Self {
        Self(Arc::new(SinkState {
            stores,
            events: EventStore::new(),
            projections,
            clock,
            signals,
            publication: broadcast::channel(1024).0,
            shared_publication: broadcast::channel(1024).0,
            lane: Arc::new(Mutex::new(())),
            commits: TaskTracker::new(),
            barrier: StdMutex::new(None),
            streams: StdMutex::new(Vec::new()),
            wire_cache: WireEventCache::default(),
            outcomes: RunOutcomeRecorder::default(),
            outcome_events: broadcast::channel(1024).0,
        }))
    }

    pub fn signals(&self) -> OutboxSignals {
        self.0.signals.clone()
    }

    pub fn subscribe_outcomes(&self) -> broadcast::Receiver<RunOutcome> {
        self.0.outcome_events.subscribe()
    }

    pub fn read_by_command_id(
        &self,
        command_id: CommandId,
    ) -> EventPages<OrchestrationV2StoredEvent> {
        self.0.events.read_paged(
            self.0.stores.clone(),
            EventFilter {
                command_id: Some(command_id),
                ..Default::default()
            },
        )
    }

    pub async fn stream(&self, options: StreamOptions) -> Result<EventStream> {
        let (queue, live) = if options.bounded {
            (
                StreamQueue::Bounded(Arc::new(LiveBuffer::new(LiveStreamLimits::default()))),
                None,
            )
        } else {
            let (sender, receiver) = mpsc::unbounded_channel();
            (StreamQueue::Internal(sender), Some(receiver))
        };
        let subscription = Arc::new(StreamSubscription {
            options: options.clone(),
            queue,
        });
        self.0
            .streams
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(Arc::downgrade(&subscription));
        let through = self.latest_sequence(None).await?;
        let replay = self.0.events.read_paged(
            self.0.stores.clone(),
            EventFilter {
                thread_id: options.thread_id,
                event_type: options.event_type,
                after_sequence: i64::try_from(options.after_sequence).map_err(|_| Error::Stale)?,
                through_sequence: Some(through as i64),
                ..Default::default()
            },
        );
        Ok(EventStream {
            subscription,
            replay,
            replay_done: false,
            pending: VecDeque::new(),
            live,
            through: through.max(options.after_sequence),
            cache: self.0.clone(),
        })
    }

    fn publish(&self, events: &[OrchestrationV2StoredEvent], outcomes: Vec<RunOutcome>) {
        self.0.outcomes.record(outcomes, |outcome| {
            let _ = self.0.outcome_events.send(outcome.clone());
        });
        let mut streams = self
            .0
            .streams
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        streams.retain(|stream| stream.strong_count() > 0);
        for event in events {
            let raw = Arc::new(event.clone());
            let mut wire = None;
            let (thread, kind) = stream_identity(&event.event);
            for stream in streams
                .iter()
                .filter_map(Weak::upgrade)
                .filter(|stream| stream.matches(event, thread, kind))
            {
                match &stream.queue {
                    StreamQueue::Internal(sender) => {
                        let _ = sender.send(raw.clone());
                    }
                    StreamQueue::Bounded(buffer) => {
                        let projected = wire.get_or_insert_with(|| {
                            self.0.wire_cache.project(&raw).and_then(|event| {
                                SharedLiveItem::new((*event).clone(), event.sequence)
                            })
                        });
                        match projected {
                            Ok(event) => {
                                let _ = buffer.push(event.clone(), None);
                            }
                            Err(_) => buffer.fail(),
                        }
                    }
                }
            }
            self.publish_application(ApplicationStoredEvent::Orchestration(Box::new(
                event.clone(),
            )));
        }
    }

    fn publish_application(&self, event: ApplicationStoredEvent) {
        let event = Arc::new(event);
        if self.0.publication.receiver_count() > 0 {
            let _ = self.0.publication.send((*event).clone());
        }
        let _ = self.0.shared_publication.send(event);
    }

    pub(crate) fn attach_barrier(&self, barrier: Weak<dyn CommitBarrier>) {
        *self
            .0
            .barrier
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(barrier);
    }

    async fn wait_for_provider_prefix(&self) -> Result<()> {
        let barrier = self
            .0
            .barrier
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .and_then(Weak::upgrade);
        if let Some(barrier) = barrier {
            barrier.wait().await?;
        }
        Ok(())
    }

    pub async fn drain(&self) {
        // TaskTracker can be reopened; callers stop admitting their batch producers before draining.
        self.0.commits.close();
        self.0.commits.wait().await;
        self.0.commits.reopen();
    }

    pub async fn commit_with_options(
        &self,
        input: CommitRequest,
        options: WriteOptions,
    ) -> Result<CommitResult> {
        self.wait_for_provider_prefix().await?;
        let mut results = self
            .commit_batch(vec![BatchJob {
                owner: CancellationToken::new(),
                scope: None,
                request: input,
                options,
                prepare: None,
            }])
            .await?;
        results.pop().ok_or(Error::Service {
            operation: "commit",
            detail: "Missing commit result".into(),
        })?
    }

    pub(crate) async fn commit_batch(
        &self,
        jobs: Vec<BatchJob>,
    ) -> Result<Vec<Result<CommitResult>>> {
        let sink = self.clone();
        // The task owns the lane through publication, even if the awaiting caller is dropped.
        self.0
            .commits
            .spawn(async move {
                let _lane = sink.0.lane.clone().lock_owned().await;
                let state = sink.0.clone();
                let now = state.clock.now().0;
                let isolate = jobs.len() > 1;
                let (results, outcomes) = sink
                    .0
                    .stores
                    .transaction(move |tx| {
                        let mut results = Vec::with_capacity(jobs.len());
                        let mut outcomes = Vec::with_capacity(jobs.len());
                        for job in jobs {
                            if job.owner.is_cancelled()
                                || job
                                    .scope
                                    .as_ref()
                                    .is_some_and(CancellationToken::is_cancelled)
                            {
                                results.push(Err(Error::Cancelled));
                                outcomes.push(Vec::new());
                                continue;
                            }
                            if isolate {
                                tx.execute_batch("SAVEPOINT provider_snapshot")?;
                            }
                            let outcome = commit_in_transaction(&state, tx, job.request, job.options, job.prepare, &now).and_then(|result| {
                                let outcomes = if result.committed {
                                    RunOutcomeRecorder::prepare(&result.events, |thread| {
                                        Ok(tx.query_row("SELECT COALESCE(json_extract(payload_json,'$.lineage.relationshipToParent')='subagent',0) FROM orchestration_v2_projection_threads WHERE thread_id=?1", [&thread.0], |row| row.get::<_, bool>(0)).optional()?.unwrap_or(false))
                                    })?
                                } else { Vec::new() };
                                Ok((result, outcomes))
                            });
                            if outcome.is_err() {
                                job.owner.cancel();
                            }
                            if isolate {
                                if outcome.is_err() {
                                    tx.execute_batch("ROLLBACK TO provider_snapshot")?;
                                }
                                tx.execute_batch("RELEASE provider_snapshot")?;
                            } else if let Err(error) = outcome {
                                return Err(error);
                            }
                            match outcome {
                                Ok((result, prepared)) => { results.push(Ok(result)); outcomes.push(prepared); },
                                Err(error) => { results.push(Err(error)); outcomes.push(Vec::new()); },
                            }
                        }
                        Ok((results, outcomes))
                    })
                    .await?;
                for (result, outcomes) in results.iter().zip(outcomes) {
                    let Ok(result) = result else { continue };
                    if !result.committed { continue }
                    sink.0.signals.cancel(&result.cancelled_effect_ids);
                    sink.0.signals.notify();
                    sink.publish(&result.events, outcomes);
                }
                Ok(results)
            })
            .await
            .map_err(task_error)?
    }

    pub async fn commit_rejected(&self, mut receipt: CommandReceipt) -> Result<CommandReceipt> {
        self.wait_for_provider_prefix().await?;
        let sink = self.clone();
        self.0
            .commits
            .spawn(async move {
                let _lane = sink.0.lane.clone().lock_owned().await;
                let state = sink.0.clone();
                sink.0
                    .stores
                    .transaction(move |tx| {
                        if let Some(existing) = receipts::get(tx, &receipt.command_id)? {
                            ensure_receipt_kind(&receipt, &existing)?;
                            return Ok(existing);
                        }
                        receipt.status = ReceiptStatus::Rejected;
                        receipt.accepted_at = timestamp("acceptedAt", &receipt.accepted_at)?;
                        receipt.result_sequence = match &receipt.aggregate {
                            AggregateId::Thread(id) => {
                                state.events.latest_agent_sequence(tx, Some(id))?
                            }
                            AggregateId::Project(_) => {
                                state.events.latest_application_sequence(tx)?
                            }
                        };
                        receipts::insert_if_absent(tx, &receipt)?;
                        Ok(receipt)
                    })
                    .await
            })
            .await
            .map_err(task_error)?
    }
}

#[async_trait]
impl EventCommitter for EventSink {
    async fn commit(&self, input: CommitRequest) -> Result<CommitResult> {
        self.commit_with_options(input, WriteOptions::default())
            .await
    }

    async fn commit_with_options(
        &self,
        input: CommitRequest,
        options: WriteOptions,
    ) -> Result<CommitResult> {
        EventSink::commit_with_options(self, input, options).await
    }

    async fn commit_prepared(
        &self,
        input: CommitRequest,
        options: WriteOptions,
        prepare: CommitPreparation,
    ) -> Result<CommitResult> {
        self.wait_for_provider_prefix().await?;
        self.commit_batch(vec![BatchJob {
            owner: CancellationToken::new(),
            scope: None,
            request: input,
            options,
            prepare: Some(prepare),
        }])
        .await?
        .pop()
        .ok_or(Error::Service {
            operation: "commit_prepared",
            detail: "Missing commit result".into(),
        })?
    }

    async fn commit_project(
        &self,
        event: ApplicationProjectEvent,
        mut receipt: CommandReceipt,
    ) -> Result<CommandReceipt> {
        self.wait_for_provider_prefix().await?;
        if !matches!(receipt.aggregate, AggregateId::Project(_)) {
            return Err(Error::Rejected {
                command_id: receipt.command_id,
                reason: "Expected a project receipt".into(),
            });
        }
        let sink = self.clone();
        self.0
            .commits
            .spawn(async move {
                let _lane = sink.0.lane.clone().lock_owned().await;
                let state = sink.0.clone();
                let (receipt, event) = sink
                    .0
                    .stores
                    .transaction(move |tx| {
                        receipt.accepted_at = timestamp("acceptedAt", &receipt.accepted_at)?;
                        if !receipts::insert_if_absent(tx, &receipt)? {
                            let existing = existing_receipt(tx, &receipt)?;
                            return Ok((existing, None));
                        }
                        let stored = state.events.append_project(tx, &event)?;
                        projects::apply(tx, &stored)?;
                        let value = serde_json::to_value(&stored)?;
                        receipt.result_sequence =
                            value["sequence"].as_i64().ok_or(Error::Decode {
                                entity: "project event",
                                detail: "Missing sequence".into(),
                            })?;
                        receipts::upsert(tx, &receipt)?;
                        Ok((receipt, Some(stored)))
                    })
                    .await?;
                if let Some(event) = event {
                    sink.publish_application(ApplicationStoredEvent::Project(Box::new(event)));
                }
                Ok(receipt)
            })
            .await
            .map_err(task_error)?
    }

    async fn events_by_command(
        &self,
        command_id: CommandId,
    ) -> Result<Vec<OrchestrationV2StoredEvent>> {
        let state = self.0.clone();
        self.0
            .stores
            .read(move |db| {
                state.events.read(
                    db,
                    &EventFilter {
                        command_id: Some(command_id),
                        ..Default::default()
                    },
                )
            })
            .await
    }

    fn subscribe(&self) -> broadcast::Receiver<ApplicationStoredEvent> {
        self.0.publication.subscribe()
    }

    fn subscribe_shared(&self) -> Result<broadcast::Receiver<Arc<ApplicationStoredEvent>>> {
        Ok(self.0.shared_publication.subscribe())
    }

    async fn latest_sequence(&self, thread_id: Option<ThreadId>) -> Result<u64> {
        let state = self.0.clone();
        self.0
            .stores
            .read(move |db| Ok(state.events.latest_agent_sequence(db, thread_id.as_ref())? as u64))
            .await
    }
}

fn empty_result() -> CommitResult {
    CommitResult {
        committed: false,
        receipt: None,
        events: Vec::new(),
        cancelled_effect_ids: Vec::new(),
    }
}

fn ensure_receipt_kind(input: &CommandReceipt, existing: &CommandReceipt) -> Result<()> {
    if input.aggregate.kind() == existing.aggregate.kind() {
        return Ok(());
    }
    Err(Error::Rejected {
        command_id: input.command_id.clone(),
        reason: format!(
            "Command was already used by a {} command",
            existing.aggregate.kind()
        ),
    })
}

fn existing_receipt(db: &Connection, input: &CommandReceipt) -> Result<CommandReceipt> {
    let existing = receipts::get(db, &input.command_id)?.ok_or(Error::Service {
        operation: "commit",
        detail: "Command receipt disappeared during its transaction".into(),
    })?;
    ensure_receipt_kind(input, &existing)?;
    Ok(existing)
}

fn commit_in_transaction(
    state: &SinkState,
    tx: &Transaction<'_>,
    input: CommitRequest,
    options: WriteOptions,
    prepare: Option<CommitPreparation>,
    now: &str,
) -> Result<CommitResult> {
    if let Some(receipt) = &input.receipt {
        if let Some(existing) = receipts::get(tx, &receipt.command_id)? {
            ensure_receipt_kind(receipt, &existing)?;
            let events = state.events.read(
                tx,
                &EventFilter {
                    command_id: Some(receipt.command_id.clone()),
                    ..Default::default()
                },
            )?;
            return Ok(CommitResult {
                receipt: Some(existing),
                events,
                ..empty_result()
            });
        }
        if !matches!(receipt.aggregate, AggregateId::Thread(_)) {
            return Err(Error::Rejected {
                command_id: receipt.command_id.clone(),
                reason: "Expected a thread receipt".into(),
            });
        }
    }
    if !guard_matches(tx, &input.guard)? || !extra_guard_matches(tx, &options)? {
        return Ok(empty_result());
    }
    let mut input = match prepare {
        Some(prepare) => {
            let receipt = input.receipt.clone();
            let guard = input.guard.clone();
            let prepared = prepare(tx, input)?;
            if prepared.receipt != receipt
                || !guard_matches(tx, &guard)?
                || !guard_matches(tx, &prepared.guard)?
                || !extra_guard_matches(tx, &options)?
            {
                // Returning an error also rolls back preparation's writes and savepoint.
                return Err(Error::Stale);
            }
            prepared
        }
        None => input,
    };
    if let Some(receipt) = &mut input.receipt {
        receipt.accepted_at = timestamp("acceptedAt", &receipt.accepted_at)?;
        receipts::insert_if_absent(tx, receipt)?;
    }
    let events = prepare_events(tx, input.decision.events, &options)?;
    let command = input
        .receipt
        .as_ref()
        .map(|receipt| &receipt.command_id)
        .or(options.command_id.as_ref());
    let events = state.events.append(tx, command, &events)?;
    for event in &events {
        state.projections.apply(tx, event)?;
    }
    if let Some(event) = events.last() {
        tx.execute("INSERT INTO orchestration_v2_projection_metadata(projection_name,schema_version,last_sequence,updated_at) VALUES('thread-projections',2,?1,?2) ON CONFLICT(projection_name) DO UPDATE SET schema_version=excluded.schema_version,last_sequence=excluded.last_sequence,updated_at=excluded.updated_at", params![event.sequence as i64, now])?;
    }
    outbox::enqueue(tx, &input.decision.effects, now)?;
    let receipt = if let Some(mut receipt) = input.receipt {
        receipt.result_sequence = if let Some(last) = events.last() {
            last.sequence as i64
        } else if receipt.status == ReceiptStatus::Rejected || !input.decision.effects.is_empty() {
            let AggregateId::Thread(thread_id) = &receipt.aggregate else {
                return Err(Error::Stale);
            };
            state.events.latest_agent_sequence(tx, Some(thread_id))?
        } else {
            return Err(Error::Rejected {
                command_id: receipt.command_id,
                reason: "Command produced no orchestration events".into(),
            });
        };
        receipts::upsert(tx, &receipt)?;
        Some(receipt)
    } else {
        None
    };
    let mut cancelled_effect_ids = Vec::new();
    for cancellation in input.decision.cancellations {
        cancelled_effect_ids.extend(outbox::cancel_unsettled(
            tx,
            &cancellation.thread_id,
            &cancellation
                .effect_types
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            &cancellation.reason,
            now,
        )?);
    }
    Ok(CommitResult {
        committed: true,
        receipt,
        events,
        cancelled_effect_ids,
    })
}

fn guard_matches(db: &Connection, guard: &CommitGuard) -> Result<bool> {
    Ok(match guard {
        CommitGuard::None => true,
        CommitGuard::RunCurrent { thread_id, run_id, attempt_id, status } => {
            let status = serde_json::to_value(status)?;
            db.query_row("SELECT COALESCE(status=?3 AND json_extract(payload_json,'$.activeAttemptId')=?4,0) FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND run_id=?2 LIMIT 1", params![thread_id.0, run_id.0, status.as_str(), attempt_id.0], |row| row.get::<_, bool>(0)).optional()?.unwrap_or(false)
        },
        CommitGuard::ProviderThreadOwner { provider_thread_id, run_id, attempt_id, last_run_ordinal } => {
            db.query_row("SELECT COALESCE(json_extract(r.payload_json,'$.activeAttemptId')=?3 AND p.last_run_ordinal=?4,0) FROM orchestration_v2_projection_provider_threads p JOIN orchestration_v2_projection_runs r ON r.run_id=?2 AND r.thread_id=p.thread_id WHERE p.provider_thread_id=?1 LIMIT 1", params![provider_thread_id.0, run_id.0, attempt_id.0, i64::try_from(*last_run_ordinal).map_err(|_| Error::Stale)?], |row| row.get::<_, bool>(0)).optional()?.unwrap_or(false)
        },
        CommitGuard::ProviderThreadUnchanged(expected) => payload_matches(db, "orchestration_v2_projection_provider_threads", "provider_thread_id", expected.as_ref())?,
        CommitGuard::TurnItemUnchanged(expected) => payload_matches(db, "orchestration_v2_projection_turn_items", "turn_item_id", expected.as_ref())?,
    })
}

fn payload_matches(
    db: &Connection,
    table: &str,
    id_column: &str,
    expected: &impl Serialize,
) -> Result<bool> {
    let value = t3_contracts::stored::to_value(expected)?;
    let stored: Option<String> = db
        .query_row(
            &format!("SELECT payload_json FROM {table} WHERE {id_column}=?1 LIMIT 1"),
            [value["id"].as_str()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(stored.as_deref() == Some(crate::stores::event_store::encode_json(&value)?.as_str()))
}

fn extra_guard_matches(db: &Connection, options: &WriteOptions) -> Result<bool> {
    if let Some(expected) = &options.unless_native_thread_owned {
        let value = t3_contracts::stored::to_value(expected)?;
        if let Some(native_id) = value
            .pointer("/nativeThreadRef/nativeId")
            .and_then(Value::as_str)
        {
            let owned: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM orchestration_v2_projection_provider_threads WHERE driver=?1 AND provider_instance_id=?2 AND thread_id IS NOT ?3 AND json_extract(payload_json,'$.nativeThreadRef.nativeId')=?4)", params![value["driver"].as_str(),value["providerInstanceId"].as_str(),value["appThreadId"].as_str(),native_id], |row| row.get(0))?;
            if owned {
                return Ok(false);
            }
        }
    }
    if let Some((thread, request, result)) = &options.unless_interrupt_requested {
        let pending: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 AND turn_item_id=?2) AND NOT EXISTS(SELECT 1 FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 AND turn_item_id=?3)", params![thread.0, request, result], |row| row.get(0))?;
        if pending {
            return Ok(false);
        }
    }
    Ok(true)
}

fn prepare_events(
    db: &Connection,
    events: Vec<OrchestrationV2DomainEvent>,
    options: &WriteOptions,
) -> Result<Vec<OrchestrationV2DomainEvent>> {
    let mut values = events
        .iter()
        .map(t3_contracts::stored::to_value)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if options.update_all_delegated_completions_only
        || options.update_delegated_completion_only_run_id.is_some()
    {
        for event in &mut values {
            if event["type"] != "run.updated" {
                continue;
            }
            let id = event["payload"]["id"].as_str();
            if !options.update_all_delegated_completions_only
                && id != options.update_delegated_completion_only_run_id.as_deref()
            {
                continue;
            }
            let current: Option<String> = db.query_row("SELECT payload_json FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND run_id=?2", params![event["threadId"].as_str(),id], |row| row.get(0)).optional()?;
            if let Some(current) = current {
                let mut current: Value = serde_json::from_str(&current)?;
                if let Some(completion) = event["payload"].get("delegatedCompletion") {
                    current["delegatedCompletion"] = completion.clone();
                } else if let Some(object) = current.as_object_mut() {
                    object.shift_remove("delegatedCompletion");
                }
                event["payload"] = current;
            }
        }
    }
    if options.guard_pending_user_input_cancellations {
        values = guard_user_input_cancellations(db, values)?;
    }
    let run_ordinals = values
        .iter()
        .filter(|event| event["type"] == "run.created" || event["type"] == "run.updated")
        .filter_map(|event| {
            Some((
                event["payload"]["id"].as_str()?.to_owned(),
                event["payload"]["ordinal"].as_i64()?,
            ))
        })
        .collect::<HashMap<_, _>>();
    for event in &mut values {
        if event["type"] != "turn-item.updated" {
            continue;
        }
        let item = &mut event["payload"];
        let thread = serde_json::from_value(item["threadId"].clone())?;
        let id = serde_json::from_value(item["id"].clone())?;
        let run: Option<t3_contracts::base_schemas::RunId> =
            serde_json::from_value(item["runId"].clone())?;
        let ordinal = item["runId"]
            .as_str()
            .and_then(|id| run_ordinals.get(id))
            .copied();
        item["ordinal"] = positions::allocate(
            db,
            &thread,
            &id,
            run.as_ref(),
            ordinal,
            positions::Placement::Append,
        )?
        .into();
    }
    values
        .into_iter()
        .map(|value| serde_json::from_value(value).map_err(Into::into))
        .collect()
}

fn guard_user_input_cancellations(db: &Connection, values: Vec<Value>) -> Result<Vec<Value>> {
    let mut requests = Vec::new();
    let mut nodes = Vec::new();
    for event in &values {
        let payload = &event["payload"];
        if event["type"] != "runtime-request.updated"
            || payload["kind"] != "user_input"
            || payload["status"] != "cancelled"
        {
            continue;
        }
        let current: Option<String> = db.query_row("SELECT payload_json FROM orchestration_v2_projection_runtime_requests WHERE thread_id=?1 AND runtime_request_id=?2 LIMIT 1", params![event["threadId"].as_str(), payload["id"].as_str()], |row| row.get(0)).optional()?;
        let current = current
            .map(|text| serde_json::from_str::<Value>(&text))
            .transpose()?
            .unwrap_or(Value::Null);
        if current["status"] != "pending"
            || current["kind"] != "user_input"
            || current["providerTurnId"] != payload["providerTurnId"]
            || current["responseCapability"]["type"] == "message"
        {
            requests.push(payload["id"].clone());
            nodes.push(payload["nodeId"].clone());
        }
    }
    Ok(values
        .into_iter()
        .filter(|event| {
            let payload = &event["payload"];
            if payload["status"] != "cancelled" {
                return true;
            }
            match event["type"].as_str() {
                Some("runtime-request.updated") => !requests.contains(&payload["id"]),
                Some("node.updated") => !nodes.contains(&payload["id"]),
                Some("turn-item.updated") if payload["type"] == "user_input_request" => {
                    !requests.contains(&payload["requestId"])
                }
                _ => true,
            }
        })
        .collect())
}

fn stream_identity(event: &OrchestrationV2DomainEvent) -> (&ThreadId, &str) {
    match event {
        OrchestrationV2DomainEvent::ThreadCreated(event) => (&event.thread_id, "thread.created"),
        OrchestrationV2DomainEvent::ThreadArchived(event) => (&event.thread_id, "thread.archived"),
        OrchestrationV2DomainEvent::ThreadUnarchived(event) => {
            (&event.thread_id, "thread.unarchived")
        }
        OrchestrationV2DomainEvent::ThreadDeleted(event) => (&event.thread_id, "thread.deleted"),
        OrchestrationV2DomainEvent::ThreadSettled(event) => (&event.thread_id, "thread.settled"),
        OrchestrationV2DomainEvent::ThreadUnsettled(event) => {
            (&event.thread_id, "thread.unsettled")
        }
        OrchestrationV2DomainEvent::ThreadSnoozed(event) => (&event.thread_id, "thread.snoozed"),
        OrchestrationV2DomainEvent::ThreadUnsnoozed(event) => {
            (&event.thread_id, "thread.unsnoozed")
        }
        OrchestrationV2DomainEvent::ThreadPinned(event) => (&event.thread_id, "thread.pinned"),
        OrchestrationV2DomainEvent::ThreadAutoSettleSet(event) => {
            (&event.thread_id, "thread.auto-settle-set")
        }
        OrchestrationV2DomainEvent::ThreadUnpinned(event) => (&event.thread_id, "thread.unpinned"),
        OrchestrationV2DomainEvent::ThreadPinReordered(event) => {
            (&event.thread_id, "thread.pin-reordered")
        }
        OrchestrationV2DomainEvent::ThreadActiveReordered(event) => {
            (&event.thread_id, "thread.active-reordered")
        }
        OrchestrationV2DomainEvent::ThreadTagsUpdated(event) => {
            (&event.thread_id, "thread.tags-updated")
        }
        OrchestrationV2DomainEvent::ThreadVisited(event) => (&event.thread_id, "thread.visited"),
        OrchestrationV2DomainEvent::ThreadMarkedUnread(event) => {
            (&event.thread_id, "thread.marked-unread")
        }
        OrchestrationV2DomainEvent::ThreadMetadataUpdated(event) => {
            (&event.thread_id, "thread.metadata-updated")
        }
        OrchestrationV2DomainEvent::ThreadPullRequestSynced(event) => {
            (&event.thread_id, "thread.pull-request-synced")
        }
        OrchestrationV2DomainEvent::ThreadRuntimeModeUpdated(event) => {
            (&event.thread_id, "thread.runtime-mode-updated")
        }
        OrchestrationV2DomainEvent::ThreadInteractionModeUpdated(event) => {
            (&event.thread_id, "thread.interaction-mode-updated")
        }
        OrchestrationV2DomainEvent::ThreadModelSelectionUpdated(event) => {
            (&event.thread_id, "thread.model-selection-updated")
        }
        OrchestrationV2DomainEvent::ThreadProviderSwitched(event) => {
            (&event.thread_id, "thread.provider-switched")
        }
        OrchestrationV2DomainEvent::ThreadGuestShareUpdated(event) => {
            (&event.thread_id, "thread.guest-share-updated")
        }
        OrchestrationV2DomainEvent::ThreadPullRequestLinkUpdated(event) => {
            (&event.thread_id, "thread.pull-request-link-updated")
        }
        OrchestrationV2DomainEvent::RunCreated(event) => (&event.thread_id, "run.created"),
        OrchestrationV2DomainEvent::RunUpdated(event) => (&event.thread_id, "run.updated"),
        OrchestrationV2DomainEvent::RunBackgroundWorkCancelled(event) => {
            (&event.thread_id, "run.background-work-cancelled")
        }
        OrchestrationV2DomainEvent::RunAttemptCreated(event) => {
            (&event.thread_id, "run-attempt.created")
        }
        OrchestrationV2DomainEvent::RunAttemptUpdated(event) => {
            (&event.thread_id, "run-attempt.updated")
        }
        OrchestrationV2DomainEvent::NodeUpdated(event) => (&event.thread_id, "node.updated"),
        OrchestrationV2DomainEvent::SubagentUpdated(event) => {
            (&event.thread_id, "subagent.updated")
        }
        OrchestrationV2DomainEvent::ProviderSessionAttached(event) => {
            (&event.thread_id, "provider-session.attached")
        }
        OrchestrationV2DomainEvent::ProviderSessionUpdated(event) => {
            (&event.thread_id, "provider-session.updated")
        }
        OrchestrationV2DomainEvent::ProviderSessionDetached(event) => {
            (&event.thread_id, "provider-session.detached")
        }
        OrchestrationV2DomainEvent::ProviderThreadUpdated(event) => {
            (&event.thread_id, "provider-thread.updated")
        }
        OrchestrationV2DomainEvent::ProviderTurnUpdated(event) => {
            (&event.thread_id, "provider-turn.updated")
        }
        OrchestrationV2DomainEvent::RuntimeRequestUpdated(event) => {
            (&event.thread_id, "runtime-request.updated")
        }
        OrchestrationV2DomainEvent::MessageUpdated(event) => (&event.thread_id, "message.updated"),
        OrchestrationV2DomainEvent::TurnItemUpdated(event) => {
            (&event.thread_id, "turn-item.updated")
        }
        OrchestrationV2DomainEvent::PlanUpdated(event) => (&event.thread_id, "plan.updated"),
        OrchestrationV2DomainEvent::CheckpointScopeCreated(event) => {
            (&event.thread_id, "checkpoint-scope.created")
        }
        OrchestrationV2DomainEvent::CheckpointCaptured(event) => {
            (&event.thread_id, "checkpoint.captured")
        }
        OrchestrationV2DomainEvent::CheckpointRollbackRequested(event) => {
            (&event.thread_id, "checkpoint.rollback-requested")
        }
        OrchestrationV2DomainEvent::ContextHandoffUpdated(event) => {
            (&event.thread_id, "context-handoff.updated")
        }
        OrchestrationV2DomainEvent::ContextTransferCreated(event) => {
            (&event.thread_id, "context-transfer.created")
        }
        OrchestrationV2DomainEvent::ContextTransferUpdated(event) => {
            (&event.thread_id, "context-transfer.updated")
        }
    }
}
