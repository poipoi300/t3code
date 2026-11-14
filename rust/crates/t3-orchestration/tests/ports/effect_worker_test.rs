use async_trait::async_trait;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use t3_contracts::{base_schemas::*, orchestration_v2::*};
use t3_orchestration::{
    api::{
        CancelEffects, CommitGuard, CommitRequest, Decision, EffectExecutor,
        EffectWorker as WorkerApi, EventCommitter, ProjectionWriter,
    },
    effects::*,
    error::{Error, Result},
    runtime::{
        commit::{EventSink, StreamOptions},
        worker::{EffectWorker, OutboxSignals, WorkerOptions},
    },
    stores::{
        StoreDatabase, outbox, projects,
        receipts::{self, AggregateId, CommandReceipt, ReceiptStatus},
    },
    testing::{ManualClock, TEST_TIME, TestDatabase, detached_event, project_event},
};
use t3_persistence::rusqlite::{Transaction, params};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

struct Projection;

impl ProjectionWriter for Projection {
    fn apply(&self, tx: &Transaction<'_>, event: &OrchestrationV2StoredEvent) -> Result<()> {
        let event = serde_json::to_value(&event.event)?;
        projects::apply(
            tx,
            &project_event(event["id"].as_str().expect("test value"), "projection")?,
        )?;
        tx.execute(
            "UPDATE projection_projects SET title=?1 WHERE project_id='projection'",
            [event["id"].as_str()],
        )?;
        Ok(())
    }
}

fn request(id: &str, thread: &str) -> CommitRequest {
    CommitRequest {
        receipt: Some(CommandReceipt {
            command_id: CommandId(id.into()),
            aggregate: AggregateId::Thread(ThreadId(thread.into())),
            command_type: "thread.stop".into(),
            accepted_at: TEST_TIME.into(),
            result_sequence: 0,
            status: ReceiptStatus::Accepted,
            error: None,
        }),
        decision: Decision {
            events: vec![detached_event(&format!("event:{id}"), thread).expect("test value")],
            effects: vec![pending(
                &format!("effect:{id}"),
                thread,
                EffectRequest::CleanupTerminals,
            )],
            cancellations: vec![],
        },
        guard: CommitGuard::None,
    }
}

fn pending(id: &str, thread: &str, request: EffectRequest) -> PendingEffect {
    PendingEffect {
        id: id.into(),
        command_id: CommandId(id.into()),
        thread_id: ThreadId(thread.into()),
        request,
        available_at: None,
    }
}

fn sink(db: &TestDatabase, signals: OutboxSignals) -> EventSink {
    EventSink::new(
        db.stores.clone(),
        Arc::new(Projection),
        Arc::new(ManualClock::new(TEST_TIME)),
        signals,
    )
}

async fn rows(stores: &StoreDatabase, id: &str) -> Result<EffectRow> {
    let id = id.to_owned();
    stores
        .read(move |db| Ok(outbox::get(db, &id)?.expect("effect")))
        .await
}

#[tokio::test]
async fn sink_rolls_back_all_four_surfaces_when_final_receipt_write_fails() -> Result<()> {
    let fixture = TestDatabase::temporary()?;
    let sink = sink(&fixture, OutboxSignals::default());
    let mut events = sink.subscribe();
    fixture.stores.transaction(|tx| {
        tx.execute_batch("CREATE TRIGGER fail_final_receipt BEFORE UPDATE ON orchestration_command_receipts BEGIN SELECT RAISE(ABORT,'forced midway failure'); END")?;
        Ok(())
    }).await?;
    let error = sink
        .commit(request("rollback", "thread"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("forced midway failure"));
    fixture.stores.read(|db| {
        for table in ["orchestration_command_receipts", "orchestration_events", "projection_projects", "orchestration_v2_effect_outbox"] {
            let count: i64 = db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))?;
            assert_eq!(count, 0, "{table} survived rollback");
        }
        let sequence: i64 = db.query_row("SELECT last_sequence FROM orchestration_v2_projection_metadata WHERE projection_name='thread-projections'", [], |row| row.get(0))?;
        assert_eq!(sequence, 0);
        Ok(())
    }).await?;
    assert!(matches!(
        events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    Ok(())
}

#[tokio::test]
async fn receipts_dedupe_concurrently_and_survive_pruned_events() -> Result<()> {
    let fixture = TestDatabase::temporary()?;
    let sink = sink(&fixture, OutboxSignals::default());
    let other = EventSink::new(
        StoreDatabase(t3_persistence::Database::open(
            fixture
                .directory
                .as_ref()
                .unwrap()
                .path()
                .join("statev2.sqlite"),
        )?),
        Arc::new(Projection),
        Arc::new(ManualClock::new(TEST_TIME)),
        OutboxSignals::default(),
    );
    let (first, second) = tokio::join!(
        sink.commit(request("same", "thread")),
        other.commit(request("same", "thread"))
    );
    let (first, second) = (first?, second?);
    assert_ne!(first.committed, second.committed);
    assert_eq!(first.receipt, second.receipt);
    assert_eq!(first.events, second.events);
    fixture
        .stores
        .transaction(|tx| {
            tx.execute("DELETE FROM orchestration_events", [])?;
            Ok(())
        })
        .await?;
    let duplicate = sink.commit(request("same", "thread")).await?;
    assert!(!duplicate.committed);
    assert!(duplicate.events.is_empty());
    assert_eq!(duplicate.receipt, first.receipt);
    assert_eq!(rows(&fixture.stores, "effect:same").await?.attempt_count, 0);
    Ok(())
}

#[tokio::test]
async fn subscribers_observe_monotone_sequences_after_all_surfaces_commit() -> Result<()> {
    let fixture = TestDatabase::temporary()?;
    let sink = sink(&fixture, OutboxSignals::default());
    let mut events = sink.subscribe();
    let mut shared = sink.subscribe_shared()?;
    let mut other_shared = sink.subscribe_shared()?;
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..20 {
        let sink = sink.clone();
        tasks.spawn(async move {
            sink.commit(request(&format!("command:{index}"), "thread"))
                .await
        });
    }
    let mut sequence = 0;
    for _ in 0..20 {
        let event = events.recv().await.unwrap();
        let shared_event = shared.recv().await.unwrap();
        let other_event = other_shared.recv().await.unwrap();
        assert!(Arc::ptr_eq(&shared_event, &other_event));
        assert_eq!(*shared_event, event);
        let t3_contracts::application_event::ApplicationStoredEvent::Orchestration(event) = event
        else {
            panic!("thread event")
        };
        assert!(event.sequence > sequence);
        sequence = event.sequence;
        let command = event.command_id.clone().unwrap();
        fixture
            .stores
            .read(move |db| {
                assert!(receipts::get_thread(db, &command)?.is_some());
                assert_eq!(
                    outbox::list_by_command(db, &CommandId(format!("effect:{}", command.0)))?.len(),
                    1
                );
                Ok(())
            })
            .await?;
    }
    while let Some(result) = tasks.join_next().await {
        assert!(result.unwrap()?.committed);
    }
    Ok(())
}

struct BlockProjection {
    started: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}

impl ProjectionWriter for BlockProjection {
    fn apply(&self, tx: &Transaction<'_>, event: &OrchestrationV2StoredEvent) -> Result<()> {
        if let Some(started) = self.started.lock().expect("test value").take() {
            let _ = started.send(());
        }
        self.release
            .lock()
            .expect("test value")
            .recv()
            .expect("test value");
        Projection.apply(tx, event)
    }
}

#[tokio::test]
async fn dropping_the_caller_during_sqlite_work_does_not_skip_publication() -> Result<()> {
    let fixture = TestDatabase::temporary()?;
    let (started, ready) = oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let sink = EventSink::new(
        fixture.stores.clone(),
        Arc::new(BlockProjection {
            started: Mutex::new(Some(started)),
            release: Mutex::new(wait),
        }),
        Arc::new(ManualClock::new(TEST_TIME)),
        OutboxSignals::default(),
    );
    let mut events = sink.subscribe();
    let caller = {
        let sink = sink.clone();
        tokio::spawn(async move { sink.commit(request("cancelled-caller", "thread")).await })
    };
    ready.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();
    events.recv().await.unwrap();
    sink.drain().await;
    assert_eq!(
        rows(&fixture.stores, "effect:cancelled-caller")
            .await?
            .status,
        EffectStatus::Pending
    );
    Ok(())
}

#[tokio::test]
async fn stale_guard_changes_nothing_and_effect_only_stop_uses_existing_high_water() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    let sink = sink(&fixture, OutboxSignals::default());
    let mut input = request("stale", "thread");
    input.guard = CommitGuard::RunCurrent {
        thread_id: ThreadId("thread".into()),
        run_id: RunId("missing".into()),
        attempt_id: RunAttemptId("old".into()),
        status: OrchestrationV2RunStatus::Running,
    };
    assert!(!sink.commit(input).await?.committed);
    assert_eq!(sink.latest_sequence(None).await?, 0);
    let initial = sink.commit(request("initial", "thread")).await?;
    let mut stop = request("stop", "thread");
    stop.decision.events.clear();
    let stop = sink.commit(stop).await?;
    assert!(stop.committed);
    assert_eq!(
        stop.receipt.unwrap().result_sequence,
        initial.receipt.unwrap().result_sequence
    );
    let mut empty = request("empty", "thread");
    empty.decision = Decision::default();
    assert!(sink.commit(empty).await.is_err());
    fixture
        .stores
        .read(|db| {
            assert!(receipts::get_thread(db, &CommandId("empty".into()))?.is_none());
            Ok(())
        })
        .await
}

struct Execution {
    row: EffectRow,
    finish: oneshot::Sender<Result<()>>,
}
struct ControlledExecutor {
    entered: mpsc::UnboundedSender<Execution>,
    cancelled: mpsc::UnboundedSender<(String, oneshot::Sender<()>)>,
    failed: mpsc::UnboundedSender<(String, bool)>,
}

#[async_trait]
impl EffectExecutor for ControlledExecutor {
    async fn execute(&self, effect: EffectRow, token: CancellationToken) -> Result<()> {
        let (finish, result) = oneshot::channel();
        self.entered
            .send(Execution {
                row: effect.clone(),
                finish,
            })
            .expect("test value");
        tokio::select! {
            biased;
            _ = token.cancelled() => {
                let (release, cleanup) = oneshot::channel();
                self.cancelled.send((effect.id, release)).expect("test value");
                cleanup.await.expect("test value");
                Err(Error::Cancelled)
            },
            result = result => result.expect("test value"),
        }
    }
    async fn failed(&self, effect: EffectRow, _: &Error, will_retry: bool) -> Result<()> {
        self.failed
            .send((effect.id, will_retry))
            .expect("test value");
        Ok(())
    }
}

type Controlled = (
    Arc<ControlledExecutor>,
    mpsc::UnboundedReceiver<Execution>,
    mpsc::UnboundedReceiver<(String, oneshot::Sender<()>)>,
    mpsc::UnboundedReceiver<(String, bool)>,
);
fn controlled() -> Controlled {
    let (entered, calls) = mpsc::unbounded_channel();
    let (cancelled, cancellations) = mpsc::unbounded_channel();
    let (failed, failures) = mpsc::unbounded_channel();
    (
        Arc::new(ControlledExecutor {
            entered,
            cancelled,
            failed,
        }),
        calls,
        cancellations,
        failures,
    )
}
fn worker(
    fixture: &TestDatabase,
    executor: Arc<dyn EffectExecutor>,
    clock: Arc<ManualClock>,
    signals: OutboxSignals,
    options: WorkerOptions,
) -> EffectWorker {
    EffectWorker::new(fixture.stores.clone(), executor, clock, signals, options)
}
async fn enqueue(fixture: &TestDatabase, effects: Vec<PendingEffect>) -> Result<()> {
    fixture
        .stores
        .transaction(move |tx| outbox::enqueue(tx, &effects, TEST_TIME))
        .await
}

#[tokio::test]
async fn cancellation_commits_before_signal_and_waits_for_native_cleanup() -> Result<()> {
    let fixture = TestDatabase::temporary()?;
    let signals = OutboxSignals::default();
    let sink = sink(&fixture, signals.clone());
    let (executor, mut calls, mut cancellations, mut failures) = controlled();
    let worker = worker(
        &fixture,
        executor,
        Arc::new(ManualClock::new(TEST_TIME)),
        signals,
        WorkerOptions::default(),
    );
    enqueue(
        &fixture,
        vec![pending("active", "thread", EffectRequest::CleanupTerminals)],
    )
    .await?;
    let running = {
        let worker = worker.clone();
        tokio::spawn(async move { worker.run_once(ClaimOptions::default()).await })
    };
    let execution = calls.recv().await.unwrap();
    let mut stop = request("stop", "thread");
    stop.decision.effects.clear();
    stop.decision.cancellations.push(CancelEffects {
        thread_id: ThreadId("thread".into()),
        effect_types: vec!["terminal.cleanup".into()],
        reason: "Stop".into(),
    });
    assert_eq!(sink.commit(stop).await?.cancelled_effect_ids, ["active"]);
    let (id, release) = cancellations.recv().await.unwrap();
    assert_eq!(id, "active");
    assert_eq!(
        rows(&fixture.stores, &id).await?.status,
        EffectStatus::Cancelled
    );
    assert!(!running.is_finished());
    release.send(()).unwrap();
    assert!(running.await.unwrap()?);
    assert!(failures.try_recv().is_err());
    drop(execution);
    worker.await_settled(id).await?;
    Ok(())
}

#[tokio::test]
async fn cancelled_during_claim_is_never_executed_even_without_notification() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    enqueue(
        &fixture,
        vec![pending("active", "thread", EffectRequest::CleanupTerminals)],
    )
    .await?;
    fixture.stores.transaction(|tx| {
        tx.execute_batch("CREATE TRIGGER cancel_on_claim AFTER UPDATE OF status ON orchestration_v2_effect_outbox WHEN NEW.status='running' BEGIN UPDATE orchestration_v2_effect_outbox SET status='cancelled',lease_owner=NULL WHERE effect_id=NEW.effect_id; END")?;
        Ok(())
    }).await?;
    let (executor, mut calls, _, _) = controlled();
    let worker = worker(
        &fixture,
        executor,
        Arc::new(ManualClock::new(TEST_TIME)),
        OutboxSignals::default(),
        WorkerOptions::default(),
    );
    assert!(worker.run_once(ClaimOptions::default()).await?);
    assert!(calls.try_recv().is_err());
    Ok(())
}

#[tokio::test]
async fn failed_pre_execution_read_requeues_claim_without_execution() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    enqueue(
        &fixture,
        vec![pending("active", "thread", EffectRequest::CleanupTerminals)],
    )
    .await?;
    fixture.stores.transaction(|tx| {
        tx.execute_batch("CREATE TRIGGER corrupt_after_claim AFTER UPDATE OF status ON orchestration_v2_effect_outbox WHEN NEW.status='running' BEGIN UPDATE orchestration_v2_effect_outbox SET payload_json='invalid json' WHERE effect_id=NEW.effect_id; END")?;
        Ok(())
    }).await?;
    let (executor, mut calls, _, _) = controlled();
    let worker = worker(
        &fixture,
        executor,
        Arc::new(ManualClock::new(TEST_TIME)),
        OutboxSignals::default(),
        WorkerOptions::default(),
    );
    assert!(worker.run_once(ClaimOptions::default()).await.is_err());
    assert!(calls.try_recv().is_err());
    fixture
        .stores
        .read(|db| {
            let (status, owner, error): (String, Option<String>, String) = db.query_row(
                "SELECT status,lease_owner,last_error FROM orchestration_v2_effect_outbox",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            assert_eq!(status, "pending");
            assert!(owner.is_none());
            assert!(error.contains("Worker failed before settling"));
            Ok(())
        })
        .await
}

#[tokio::test]
async fn success_settlement_failure_terminalizes_process_bound_and_requeues_replay_safe()
-> Result<()> {
    for (request, expected) in [
        (
            EffectRequest::Start {
                run_id: RunId("run".into()),
            },
            EffectStatus::Failed,
        ),
        (EffectRequest::CleanupTerminals, EffectStatus::Pending),
    ] {
        let fixture = TestDatabase::memory()?;
        enqueue(&fixture, vec![pending("active", "thread", request)]).await?;
        fixture.stores.transaction(|tx| { tx.execute_batch("CREATE TRIGGER fail_success BEFORE UPDATE OF status ON orchestration_v2_effect_outbox WHEN NEW.status='succeeded' BEGIN SELECT RAISE(ABORT,'simulated success settlement failure'); END")?; Ok(()) }).await?;
        let (executor, mut calls, _, _) = controlled();
        let worker = worker(
            &fixture,
            executor,
            Arc::new(ManualClock::new(TEST_TIME)),
            OutboxSignals::default(),
            WorkerOptions::default(),
        );
        let running = tokio::spawn(async move { worker.run_once(ClaimOptions::default()).await });
        calls.recv().await.unwrap().finish.send(Ok(())).unwrap();
        assert!(running.await.unwrap().is_err());
        let row = rows(&fixture.stores, "active").await?;
        assert_eq!(row.status, expected);
        assert!(
            row.last_error
                .unwrap()
                .contains("simulated success settlement failure")
        );
    }
    Ok(())
}

struct AlwaysFails {
    message: &'static str,
    calls: AtomicUsize,
    failures: Mutex<Vec<bool>>,
}

struct SelectiveFailure {
    cancelled: bool,
    broken_hook: bool,
}

#[async_trait]
impl EffectExecutor for SelectiveFailure {
    async fn execute(&self, effect: EffectRow, _: CancellationToken) -> Result<()> {
        if effect.id != "bad" {
            return Ok(());
        }
        Err(if self.cancelled {
            Error::Cancelled
        } else {
            Error::Unsupported("persistent failure")
        })
    }
    async fn failed(&self, _: EffectRow, _: &Error, _: bool) -> Result<()> {
        if self.broken_hook {
            return Err(Error::Unsupported("failure hook"));
        }
        Ok(())
    }
}

#[tokio::test]
async fn failures_and_executor_cancellation_exhaust_the_budget_without_blocking_other_threads()
-> Result<()> {
    for (cancelled, broken_hook) in [(false, false), (true, false), (false, true)] {
        let fixture = TestDatabase::memory()?;
        let clock = Arc::new(ManualClock::new(TEST_TIME));
        let worker = worker(
            &fixture,
            Arc::new(SelectiveFailure {
                cancelled,
                broken_hook,
            }),
            clock.clone(),
            OutboxSignals::default(),
            WorkerOptions {
                max_attempts: 2,
                ..Default::default()
            },
        );
        enqueue(
            &fixture,
            vec![
                pending("bad", "blocked-thread", EffectRequest::CleanupPreviews),
                pending(
                    "same-thread",
                    "blocked-thread",
                    EffectRequest::CleanupPreviews,
                ),
                pending("healthy", "other-thread", EffectRequest::CleanupPreviews),
            ],
        )
        .await?;
        assert!(worker.run_once(ClaimOptions::default()).await?);
        let retry = rows(&fixture.stores, "bad").await?;
        assert_eq!(
            retry.status,
            EffectStatus::Pending,
            "cancelled={cancelled}, hook={broken_hook}"
        );
        assert_eq!(retry.attempt_count, 1);
        assert!(retry.lease_owner.is_none());
        assert!(worker.run_once(ClaimOptions::default()).await?);
        assert_eq!(
            rows(&fixture.stores, "healthy").await?.status,
            EffectStatus::Succeeded
        );
        assert_eq!(rows(&fixture.stores, "same-thread").await?.attempt_count, 0);
        clock.set(&retry.available_at);
        assert!(worker.run_once(ClaimOptions::default()).await?);
        let failed = rows(&fixture.stores, "bad").await?;
        assert_eq!(failed.status, EffectStatus::Failed);
        assert_eq!(failed.attempt_count, 2);
        assert!(failed.completed_at.is_some());
        worker.await_settled("bad".into()).await?;
        assert!(worker.run_once(ClaimOptions::default()).await?);
        assert_eq!(
            rows(&fixture.stores, "same-thread").await?.status,
            EffectStatus::Succeeded
        );
        worker.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn undecodable_claim_stays_owned_and_blocks_only_its_lane_while_worker_executes_healthy_rows()
-> Result<()> {
    let fixture = TestDatabase::temporary()?;
    let worker = worker(
        &fixture,
        Arc::new(SelectiveFailure {
            cancelled: false,
            broken_hook: false,
        }),
        Arc::new(ManualClock::new(TEST_TIME)),
        OutboxSignals::default(),
        WorkerOptions {
            worker_id: "poison-worker".into(),
            ..Default::default()
        },
    );
    enqueue(
        &fixture,
        vec![
            pending("poison", "poisoned-thread", EffectRequest::CleanupPreviews),
            pending(
                "poison-successor",
                "poisoned-thread",
                EffectRequest::CleanupPreviews,
            ),
            pending("healthy", "other-thread", EffectRequest::CleanupPreviews),
        ],
    )
    .await?;
    fixture.stores.transaction(|tx| { tx.execute("UPDATE orchestration_v2_effect_outbox SET payload_json='invalid json' WHERE effect_id='poison'", [])?; Ok(()) }).await?;
    assert!(worker.run_once(ClaimOptions::default()).await?);
    assert_eq!(
        rows(&fixture.stores, "healthy").await?.status,
        EffectStatus::Succeeded
    );
    assert!(!worker.run_once(ClaimOptions::default()).await?);
    fixture.stores.read(|db| {
        let (status, attempts, owner, completed): (String, i64, Option<String>, Option<String>) = db.query_row("SELECT status,attempt_count,lease_owner,completed_at FROM orchestration_v2_effect_outbox WHERE effect_id='poison'", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
        assert_eq!(status, "running");
        assert_eq!(attempts, 1);
        assert_eq!(owner.as_deref(), Some("poison-worker"));
        assert!(completed.is_none());
        assert_eq!(outbox::get(db, "poison-successor")?.unwrap().attempt_count, 0);
        Ok(())
    }).await?;
    assert!(
        worker.drain(usize::MAX).await.is_err(),
        "drain must report an unexecutable claim instead of waiting forever"
    );
    worker.shutdown().await
}
#[async_trait]
impl EffectExecutor for AlwaysFails {
    async fn execute(&self, _: EffectRow, _: CancellationToken) -> Result<()> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Err(Error::Service {
            operation: "test",
            detail: self.message.into(),
        })
    }
    async fn failed(&self, _: EffectRow, _: &Error, retry: bool) -> Result<()> {
        self.failures.lock().expect("test value").push(retry);
        Ok(())
    }
}

#[tokio::test]
async fn retries_use_exponential_persisted_deadlines_and_fail_on_fifth_attempt() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    enqueue(
        &fixture,
        vec![
            pending("first", "thread", EffectRequest::CleanupTerminals),
            pending("later", "thread", EffectRequest::CleanupPreviews),
        ],
    )
    .await?;
    let clock = Arc::new(ManualClock::new(TEST_TIME));
    let executor = Arc::new(AlwaysFails {
        message: "provider failed",
        calls: AtomicUsize::new(0),
        failures: Mutex::new(Vec::new()),
    });
    let worker = worker(
        &fixture,
        executor.clone(),
        clock.clone(),
        OutboxSignals::default(),
        WorkerOptions::default(),
    );
    for (attempt, expected) in [
        "2026-09-03T00:00:00.100Z",
        "2026-09-03T00:00:00.300Z",
        "2026-09-03T00:00:00.700Z",
        "2026-09-03T00:00:01.500Z",
    ]
    .into_iter()
    .enumerate()
    {
        assert!(worker.run_once(ClaimOptions::default()).await?);
        let row = rows(&fixture.stores, "first").await?;
        assert_eq!(row.attempt_count, attempt as i64 + 1);
        assert_eq!(row.status, EffectStatus::Pending);
        assert_eq!(row.available_at, expected);
        assert!(!worker.run_once(ClaimOptions::default()).await?);
        clock.set(expected);
    }
    assert!(worker.run_once(ClaimOptions::default()).await?);
    assert_eq!(
        rows(&fixture.stores, "first").await?.status,
        EffectStatus::Failed
    );
    assert_eq!(
        *executor.failures.lock().unwrap(),
        [true, true, true, true, false]
    );
    assert_eq!(rows(&fixture.stores, "later").await?.attempt_count, 0);
    Ok(())
}

fn interrupt() -> EffectRequest {
    EffectRequest::Interrupt {
        provider_session_id: ProviderSessionId("s".into()),
        provider_thread_id: ProviderThreadId("p".into()),
        provider_turn_id: ProviderTurnId("t".into()),
        subagent: None,
    }
}

#[tokio::test]
async fn inactive_interrupt_succeeds_but_restart_with_same_error_retries() -> Result<()> {
    for (request, expected) in [
        (interrupt(), EffectStatus::Succeeded),
        (
            EffectRequest::Restart {
                provider_session_id: ProviderSessionId("s".into()),
                provider_thread_id: ProviderThreadId("p".into()),
                provider_turn_id: ProviderTurnId("t".into()),
                interrupted_attempt_id: RunAttemptId("a".into()),
                run_id: RunId("r".into()),
                session_transition: None,
            },
            EffectStatus::Pending,
        ),
    ] {
        let fixture = TestDatabase::memory()?;
        enqueue(&fixture, vec![pending("active", "thread", request)]).await?;
        let executor = Arc::new(AlwaysFails {
            message: "Provider turn IS NOT ACTIVE",
            calls: AtomicUsize::new(0),
            failures: Mutex::new(Vec::new()),
        });
        let worker = worker(
            &fixture,
            executor,
            Arc::new(ManualClock::new(TEST_TIME)),
            OutboxSignals::default(),
            WorkerOptions::default(),
        );
        assert!(worker.run_once(ClaimOptions::default()).await?);
        assert_eq!(rows(&fixture.stores, "active").await?.status, expected);
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn slots_release_after_thirty_seconds_without_expiring_running_ownership() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    enqueue(
        &fixture,
        vec![
            pending("first", "thread", EffectRequest::CleanupTerminals),
            pending("blocked", "thread", EffectRequest::CleanupPreviews),
        ],
    )
    .await?;
    let (executor, mut calls, mut cancellations, _) = controlled();
    let worker = worker(
        &fixture,
        executor,
        Arc::new(ManualClock::new(TEST_TIME)),
        OutboxSignals::default(),
        WorkerOptions {
            concurrency: 1,
            ..Default::default()
        },
    );
    let daemon = {
        let worker = worker.clone();
        tokio::spawn(async move { worker.run_daemon().await })
    };
    let first = calls.recv().await.unwrap();
    tokio::time::advance(std::time::Duration::from_secs(30)).await;
    enqueue(
        &fixture,
        vec![pending(
            "other",
            "elsewhere",
            EffectRequest::CleanupTerminals,
        )],
    )
    .await?;
    worker.signals().notify();
    let other = calls.recv().await.unwrap();
    assert_eq!(other.row.id, "other");
    let row = rows(&fixture.stores, "first").await?;
    assert_eq!(row.status, EffectStatus::Running);
    assert_eq!(row.attempt_count, 1);
    assert_eq!(rows(&fixture.stores, "blocked").await?.attempt_count, 0);
    assert!(cancellations.try_recv().is_err());
    other.finish.send(Ok(())).unwrap();
    first.finish.send(Ok(())).unwrap();
    calls.recv().await.unwrap().finish.send(Ok(())).unwrap();
    worker.drain(100).await?;
    worker.shutdown().await?;
    daemon.await.unwrap()?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn delegated_stop_timeout_awaits_cleanup_then_retries() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    enqueue(
        &fixture,
        vec![pending(
            "stop",
            "thread",
            EffectRequest::StopDelegated { reason: None },
        )],
    )
    .await?;
    let (executor, mut calls, mut cancellations, mut failures) = controlled();
    let worker = worker(
        &fixture,
        executor,
        Arc::new(ManualClock::new(TEST_TIME)),
        OutboxSignals::default(),
        WorkerOptions::default(),
    );
    let running = {
        let worker = worker.clone();
        tokio::spawn(async move { worker.run_once(ClaimOptions::default()).await })
    };
    let execution = calls.recv().await.unwrap();
    tokio::time::advance(std::time::Duration::from_secs(30)).await;
    let (_, release) = cancellations.recv().await.unwrap();
    assert!(!running.is_finished());
    release.send(()).unwrap();
    assert!(running.await.unwrap()?);
    assert_eq!(failures.recv().await.unwrap(), ("stop".into(), true));
    let row = rows(&fixture.stores, "stop").await?;
    assert_eq!(row.status, EffectStatus::Pending);
    assert!(row.last_error.unwrap().contains("Timed out after 30s"));
    drop(execution);
    Ok(())
}

#[tokio::test]
async fn dropped_run_once_remains_tracked_and_drain_waits_until_execution_settles() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    enqueue(
        &fixture,
        vec![pending("active", "thread", EffectRequest::CleanupTerminals)],
    )
    .await?;
    let (executor, mut calls, _, _) = controlled();
    let worker = worker(
        &fixture,
        executor,
        Arc::new(ManualClock::new(TEST_TIME)),
        OutboxSignals::default(),
        WorkerOptions::default(),
    );
    let caller = {
        let worker = worker.clone();
        tokio::spawn(async move { worker.run_once(ClaimOptions::default()).await })
    };
    let execution = calls.recv().await.unwrap();
    caller.abort();
    let _ = caller.await;
    let (ready, seen) = oneshot::channel();
    let drain = {
        let worker = worker.clone();
        tokio::spawn(async move {
            ready.send(()).unwrap();
            worker.drain(100).await
        })
    };
    seen.await.unwrap();
    assert!(worker.executing() >= 1);
    assert_eq!(rows(&fixture.stores, "active").await?.attempt_count, 1);
    assert!(!drain.is_finished());
    execution.finish.send(Ok(())).unwrap();
    assert_eq!(drain.await.unwrap()?, 0);
    assert_eq!(
        rows(&fixture.stores, "active").await?.status,
        EffectStatus::Succeeded
    );
    Ok(())
}

#[tokio::test]
async fn shutdown_cancels_operations_awaits_cleanup_and_leaves_running_rows_for_recovery()
-> Result<()> {
    let fixture = TestDatabase::memory()?;
    enqueue(
        &fixture,
        vec![pending("active", "thread", EffectRequest::CleanupTerminals)],
    )
    .await?;
    let (executor, mut calls, mut cancellations, _) = controlled();
    let worker = worker(
        &fixture,
        executor,
        Arc::new(ManualClock::new(TEST_TIME)),
        OutboxSignals::default(),
        WorkerOptions::default(),
    );
    let running = {
        let worker = worker.clone();
        tokio::spawn(async move { worker.run_once(ClaimOptions::default()).await })
    };
    let execution = calls.recv().await.unwrap();
    let shutdown = {
        let worker = worker.clone();
        tokio::spawn(async move { worker.shutdown().await })
    };
    let (_, release) = cancellations.recv().await.unwrap();
    assert!(!shutdown.is_finished());
    release.send(()).unwrap();
    shutdown.await.unwrap()?;
    running.await.unwrap()?;
    assert_eq!(
        rows(&fixture.stores, "active").await?.status,
        EffectStatus::Running
    );
    assert!(worker.run_once(ClaimOptions::default()).await.is_err());
    drop(execution);
    Ok(())
}

#[tokio::test]
async fn lost_lease_is_reported_without_overwriting_the_new_owner() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    enqueue(
        &fixture,
        vec![pending("active", "thread", EffectRequest::CleanupTerminals)],
    )
    .await?;
    let (executor, mut calls, _, _) = controlled();
    let worker = worker(
        &fixture,
        executor,
        Arc::new(ManualClock::new(TEST_TIME)),
        OutboxSignals::default(),
        WorkerOptions::default(),
    );
    let running = tokio::spawn(async move { worker.run_once(ClaimOptions::default()).await });
    let execution = calls.recv().await.unwrap();
    fixture.stores.transaction(|tx| { tx.execute("UPDATE orchestration_v2_effect_outbox SET lease_owner='other' WHERE effect_id='active'", [])?; Ok(()) }).await?;
    execution.finish.send(Ok(())).unwrap();
    assert!(matches!(
        running.await.unwrap(),
        Err(Error::LeaseLost { .. })
    ));
    let row = rows(&fixture.stores, "active").await?;
    assert_eq!(row.status, EffectStatus::Running);
    assert_eq!(row.lease_owner.as_deref(), Some("other"));
    Ok(())
}

#[tokio::test]
async fn recovery_skips_continuations_as_candidates_and_predecessors() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    enqueue(
        &fixture,
        vec![
            pending(
                "continue",
                "thread",
                EffectRequest::Continue {
                    source_run_id: RunId("run".into()),
                },
            ),
            pending("cleanup", "thread", EffectRequest::CleanupTerminals),
        ],
    )
    .await?;
    let (executor, mut calls, _, _) = controlled();
    let worker = worker(
        &fixture,
        executor,
        Arc::new(ManualClock::new(TEST_TIME)),
        OutboxSignals::default(),
        WorkerOptions::default(),
    );
    let running = {
        let worker = worker.clone();
        tokio::spawn(async move { worker.run_recovery_once().await })
    };
    let execution = calls.recv().await.unwrap();
    assert_eq!(execution.row.id, "cleanup");
    execution.finish.send(Ok(())).unwrap();
    assert!(running.await.unwrap()?);
    assert!(!worker.run_recovery_once().await?);
    assert_eq!(
        rows(&fixture.stores, "continue").await?.status,
        EffectStatus::Pending
    );
    Ok(())
}

#[tokio::test]
async fn daemon_reserves_one_of_four_slots_for_turn_work_instead_of_titles() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    let mut effects = (0..4)
        .map(|index| {
            pending(
                &format!("a-title:{index}"),
                &format!("thread:{index}"),
                EffectRequest::GenerateTitle {
                    kind: TitleKind::Regenerate,
                },
            )
        })
        .collect::<Vec<_>>();
    effects.push(pending(
        "z-turn",
        "turn-thread",
        EffectRequest::Start {
            run_id: RunId("run".into()),
        },
    ));
    enqueue(&fixture, effects).await?;
    let (executor, mut calls, _, _) = controlled();
    let worker = worker(
        &fixture,
        executor,
        Arc::new(ManualClock::new(TEST_TIME)),
        OutboxSignals::default(),
        WorkerOptions::default(),
    );
    let daemon = {
        let worker = worker.clone();
        tokio::spawn(async move { worker.run_daemon().await })
    };
    let mut executions = Vec::new();
    for _ in 0..4 {
        executions.push(calls.recv().await.unwrap());
    }
    assert_eq!(
        executions
            .iter()
            .filter(|call| matches!(call.row.request, EffectRequest::GenerateTitle { .. }))
            .count(),
        3
    );
    assert_eq!(
        executions
            .iter()
            .filter(|call| matches!(call.row.request, EffectRequest::Start { .. }))
            .count(),
        1
    );
    for execution in executions {
        execution.finish.send(Ok(())).unwrap();
    }
    calls.recv().await.unwrap().finish.send(Ok(())).unwrap();
    worker.drain(100).await?;
    worker.shutdown().await?;
    daemon.await.unwrap()?;
    Ok(())
}

#[tokio::test]
async fn multiple_settled_waiters_observe_durable_success() -> Result<()> {
    let fixture = TestDatabase::temporary()?;
    enqueue(
        &fixture,
        vec![pending("active", "thread", EffectRequest::CleanupTerminals)],
    )
    .await?;
    let (executor, mut calls, _, _) = controlled();
    let worker = worker(
        &fixture,
        executor,
        Arc::new(ManualClock::new(TEST_TIME)),
        OutboxSignals::default(),
        WorkerOptions::default(),
    );
    let running = {
        let worker = worker.clone();
        tokio::spawn(async move { worker.run_once(ClaimOptions::default()).await })
    };
    let execution = calls.recv().await.unwrap();
    let mut waiters = tokio::task::JoinSet::new();
    for _ in 0..10 {
        let worker = worker.clone();
        waiters.spawn(async move { worker.await_settled("active".into()).await });
    }
    execution.finish.send(Ok(())).unwrap();
    running.await.unwrap()?;
    while let Some(waiter) = waiters.join_next().await {
        waiter.unwrap()?;
    }
    worker.await_settled("missing".into()).await?;
    Ok(())
}

#[tokio::test]
async fn project_and_rejected_receipts_preserve_idempotency_and_aggregate_kind() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    let sink = sink(&fixture, OutboxSignals::default());
    let mut published = sink.subscribe_shared()?;
    let mut receipt = request("project-command", "thread").receipt.unwrap();
    receipt.aggregate = AggregateId::Project(ProjectId("project".into()));
    receipt.command_type = "project.create".into();
    receipt.accepted_at = "2026-09-03T01:00:00.000+01:00".into();
    let event = project_event("project-event", "project")?;
    let committed = sink.commit_project(event.clone(), receipt.clone()).await?;
    assert_eq!(committed.accepted_at, TEST_TIME);
    assert_eq!(committed.result_sequence, 1);
    let publication = published.recv().await.unwrap();
    assert!(matches!(
        publication.as_ref(),
        t3_contracts::application_event::ApplicationStoredEvent::Project(_)
    ));
    assert_eq!(sink.commit_project(event, receipt).await?, committed);
    assert!(matches!(
        published.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(
        sink.commit(request("project-command", "thread"))
            .await
            .is_err()
    );
    let mut rejected = request("rejected", "thread").receipt.unwrap();
    rejected.accepted_at = "2026-09-03T01:00:00.000+01:00".into();
    rejected.error = Some("Cannot stop".into());
    let rejected = sink.commit_rejected(rejected).await?;
    assert_eq!(rejected.status, ReceiptStatus::Rejected);
    assert_eq!(rejected.accepted_at, TEST_TIME);
    let duplicate = sink.commit(request("rejected", "thread")).await?;
    assert!(!duplicate.committed);
    assert!(duplicate.events.is_empty());
    assert_eq!(duplicate.receipt, Some(rejected));
    let mut accepted = request("thread-offset", "thread");
    accepted.receipt.as_mut().unwrap().accepted_at = "2026-09-03T01:00:00.000+01:00".into();
    let committed = sink.commit(accepted.clone()).await?.receipt;
    assert_eq!(committed.as_ref().unwrap().accepted_at, TEST_TIME);
    assert_eq!(sink.commit(accepted).await?.receipt, committed);
    fixture
        .stores
        .read(|db| {
            assert!(receipts::get_thread(db, &CommandId("project-command".into()))?.is_none());
            assert!(receipts::get_project(db, &CommandId("project-command".into()))?.is_some());
            Ok(())
        })
        .await
}

fn item_value() -> serde_json::Value {
    let mut item: serde_json::Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2TurnItem.jsonl")
            .lines()
            .next()
            .expect("test value"),
    )
    .expect("test value");
    item["id"] = "item".into();
    item["threadId"] = "thread".into();
    item["runId"] = "run".into();
    item["ordinal"] = 999.into();
    item
}

#[tokio::test]
async fn sink_normalizes_positions_using_runs_in_same_transaction_and_reuses_existing_items()
-> Result<()> {
    let fixture = TestDatabase::memory()?;
    let sink = sink(&fixture, OutboxSignals::default());
    let mut run: serde_json::Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2Run.jsonl")
            .lines()
            .next()
            .unwrap(),
    )?;
    run["id"] = "run".into();
    run["threadId"] = "thread".into();
    run["ordinal"] = 3.into();
    let make_event = |id: &str, kind: &str, payload: serde_json::Value| {
        serde_json::from_value::<OrchestrationV2DomainEvent>(
            serde_json::json!({"id":id,"threadId":"thread","type":kind,"occurredAt":TEST_TIME,"payload":payload}),
        )
    };
    let mut input = request("positions", "thread");
    let mut second = item_value();
    second["id"] = "item:second".into();
    input.decision.events = vec![
        make_event("run-event", "run.created", run)?,
        make_event("item:first", "turn-item.updated", item_value())?,
        make_event("item:updated", "turn-item.updated", item_value())?,
        make_event("item:second", "turn-item.updated", second)?,
    ];
    let result = sink.commit(input).await?;
    let ordinals = result
        .events
        .iter()
        .skip(1)
        .map(|event| {
            Ok(serde_json::to_value(&event.event)?["payload"]["ordinal"]
                .as_i64()
                .unwrap())
        })
        .collect::<Result<Vec<_>>>()?;
    assert_eq!(ordinals, [3_000_001, 3_000_001, 3_000_002]);
    fixture
        .stores
        .read(|db| {
            let count: i64 = db.query_row(
                "SELECT COUNT(*) FROM orchestration_v2_turn_item_positions",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(count, 2);
            Ok(())
        })
        .await
}

#[tokio::test]
async fn run_attempt_provider_owner_and_full_payload_guards_are_transactional() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    let sink = sink(&fixture, OutboxSignals::default());
    let mut provider: serde_json::Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2ProviderThread.jsonl")
            .lines()
            .next()
            .unwrap(),
    )?;
    provider["id"] = "provider".into();
    provider["appThreadId"] = "thread".into();
    let provider: OrchestrationV2ProviderThread = serde_json::from_value(provider)?;
    let item: OrchestrationV2TurnItem = serde_json::from_value(item_value())?;
    let provider_json = t3_contracts::stored::to_string(&provider)?;
    let item_json = t3_contracts::stored::to_string(&item)?;
    fixture.stores.transaction(move |tx| {
        tx.execute("INSERT INTO orchestration_v2_projection_runs(run_id,thread_id,ordinal,provider,status,requested_at,payload_json) VALUES('run','thread',1,'codex','running',?1,'{\"activeAttemptId\":\"attempt\"}')", [TEST_TIME])?;
        tx.execute("INSERT INTO orchestration_v2_projection_provider_threads(provider_thread_id,thread_id,provider,status,last_run_ordinal,updated_at,payload_json) VALUES('provider','thread','codex','idle',1,?1,?2)", params![TEST_TIME,provider_json])?;
        tx.execute("INSERT INTO orchestration_v2_projection_turn_items(turn_item_id,thread_id,ordinal,type,status,updated_at,payload_json) VALUES('item','thread',999,'approval_request','interrupted',?1,?2)", params![TEST_TIME,item_json])?;
        Ok(())
    }).await?;
    let mut changed_provider = serde_json::to_value(&provider)?;
    changed_provider["nativeThreadRef"]["nativeId"] = "changed".into();
    let changed_provider = serde_json::from_value(changed_provider)?;
    let mut changed_item = item_value();
    changed_item["ordinal"] = 1000.into();
    for (index, (guard, expected)) in [
        (
            CommitGuard::RunCurrent {
                thread_id: ThreadId("thread".into()),
                run_id: RunId("run".into()),
                attempt_id: RunAttemptId("attempt".into()),
                status: OrchestrationV2RunStatus::Running,
            },
            true,
        ),
        (
            CommitGuard::RunCurrent {
                thread_id: ThreadId("thread".into()),
                run_id: RunId("run".into()),
                attempt_id: RunAttemptId("old".into()),
                status: OrchestrationV2RunStatus::Running,
            },
            false,
        ),
        (
            CommitGuard::ProviderThreadOwner {
                provider_thread_id: ProviderThreadId("provider".into()),
                run_id: RunId("run".into()),
                attempt_id: RunAttemptId("attempt".into()),
                last_run_ordinal: 1,
            },
            true,
        ),
        (
            CommitGuard::ProviderThreadOwner {
                provider_thread_id: ProviderThreadId("provider".into()),
                run_id: RunId("run".into()),
                attempt_id: RunAttemptId("attempt".into()),
                last_run_ordinal: 2,
            },
            false,
        ),
        (
            CommitGuard::ProviderThreadUnchanged(Box::new(provider)),
            true,
        ),
        (
            CommitGuard::ProviderThreadUnchanged(Box::new(changed_provider)),
            false,
        ),
        (CommitGuard::TurnItemUnchanged(Box::new(item)), true),
        (
            CommitGuard::TurnItemUnchanged(Box::new(serde_json::from_value(changed_item)?)),
            false,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mut input = request(&format!("guard:{index}"), "thread");
        input.guard = guard;
        let result = sink.commit(input).await?;
        assert_eq!(result.committed, expected, "guard {index}");
        if !expected {
            assert!(result.receipt.is_none());
            assert!(result.events.is_empty());
        }
    }
    Ok(())
}

#[tokio::test]
async fn stale_user_input_cleanup_preserves_answered_requests_and_their_nodes() -> Result<()> {
    use t3_orchestration::runtime::commit::WriteOptions;
    for (stored_status, stored_capability, stored_turn, expected) in [
        ("resolved", "live", "turn", 0),
        ("pending", "message", "turn", 0),
        ("pending", "live", "new-turn", 0),
        ("pending", "live", "turn", 2),
    ] {
        let fixture = TestDatabase::memory()?;
        let sink = sink(&fixture, OutboxSignals::default());
        let mut runtime: serde_json::Value = serde_json::from_str(
            include_str!("../../../../fixtures/samples/OrchestrationV2RuntimeRequest.jsonl")
                .lines()
                .next()
                .unwrap(),
        )?;
        runtime["id"] = "request".into();
        runtime["nodeId"] = "node".into();
        runtime["providerTurnId"] = "turn".into();
        runtime["kind"] = "user_input".into();
        runtime["status"] = "cancelled".into();
        runtime["responseCapability"] =
            serde_json::json!({"type":"live","providerSessionId":"session"});
        let mut stored = runtime.clone();
        stored["status"] = stored_status.into();
        stored["providerTurnId"] = stored_turn.into();
        if stored_capability == "message" {
            stored["responseCapability"] = serde_json::json!({"type":"message"});
        }
        fixture.stores.transaction(move |tx| {
            tx.execute("INSERT INTO orchestration_v2_projection_runtime_requests(runtime_request_id,thread_id,node_id,kind,status,created_at,payload_json) VALUES('request','thread','node','user_input',?1,?2,?3)", params![stored_status,TEST_TIME,serde_json::to_string(&stored)?])?;
            Ok(())
        }).await?;
        let mut node: serde_json::Value = serde_json::from_str(
            include_str!("../../../../fixtures/samples/OrchestrationV2ExecutionNode.jsonl")
                .lines()
                .next()
                .unwrap(),
        )?;
        node["id"] = "node".into();
        node["threadId"] = "thread".into();
        node["status"] = "cancelled".into();
        let mut input = request("cancel-request", "thread");
        input.receipt = None;
        input.decision.effects.clear();
        input.decision.events = vec![
            serde_json::from_value(
                serde_json::json!({"id":"request-cancelled","threadId":"thread","type":"runtime-request.updated","occurredAt":TEST_TIME,"payload":runtime}),
            )?,
            serde_json::from_value(
                serde_json::json!({"id":"node-cancelled","threadId":"thread","type":"node.updated","occurredAt":TEST_TIME,"payload":node}),
            )?,
        ];
        let result = sink
            .commit_with_options(
                input,
                WriteOptions {
                    guard_pending_user_input_cancellations: true,
                    ..Default::default()
                },
            )
            .await?;
        assert_eq!(result.events.len(), expected);
    }
    Ok(())
}

#[tokio::test]
async fn rolled_back_cancellation_never_signals_a_running_operation() -> Result<()> {
    let fixture = TestDatabase::temporary()?;
    let signals = OutboxSignals::default();
    let sink = sink(&fixture, signals.clone());
    let (executor, mut calls, mut cancellations, _) = controlled();
    let worker = worker(
        &fixture,
        executor,
        Arc::new(ManualClock::new(TEST_TIME)),
        signals,
        WorkerOptions::default(),
    );
    enqueue(
        &fixture,
        vec![pending("active", "thread", EffectRequest::CleanupTerminals)],
    )
    .await?;
    let running = {
        let worker = worker.clone();
        tokio::spawn(async move { worker.run_once(ClaimOptions::default()).await })
    };
    let execution = calls.recv().await.unwrap();
    fixture.stores.transaction(|tx| {
        tx.execute_batch("CREATE TRIGGER fail_cancel BEFORE UPDATE OF status ON orchestration_v2_effect_outbox WHEN NEW.status='cancelled' BEGIN SELECT RAISE(ABORT,'forced cancellation failure'); END")?;
        Ok(())
    }).await?;
    let mut stop = request("stop", "thread");
    stop.decision.effects.clear();
    stop.decision.cancellations.push(CancelEffects {
        thread_id: ThreadId("thread".into()),
        effect_types: vec!["terminal.cleanup".into()],
        reason: "stop".into(),
    });
    assert!(sink.commit(stop).await.is_err());
    assert_eq!(
        rows(&fixture.stores, "active").await?.status,
        EffectStatus::Running
    );
    assert!(cancellations.try_recv().is_err());
    execution.finish.send(Ok(())).unwrap();
    running.await.unwrap()?;
    assert_eq!(
        rows(&fixture.stores, "active").await?.status,
        EffectStatus::Succeeded
    );
    Ok(())
}

#[tokio::test]
async fn sink_replay_pages_commands_and_transitions_to_filtered_live_without_duplicates()
-> Result<()> {
    let fixture = TestDatabase::temporary()?;
    let sink = sink(&fixture, OutboxSignals::default());
    let mut input = request("paged", "thread");
    input.decision.events = (0..1201)
        .map(|index| detached_event(&format!("paged:{index}"), "thread"))
        .collect::<Result<_>>()?;
    let stored = sink.commit(input).await?.events;
    let mut replay = sink.read_by_command_id(CommandId("paged".into()));
    let mut recovered = Vec::new();
    while let Some(page) = replay.next_page().await? {
        assert!(page.len() <= 500);
        recovered.extend(page);
    }
    assert_eq!(recovered, stored);
    let mut stream = sink
        .stream(StreamOptions {
            thread_id: Some(ThreadId("thread".into())),
            event_type: Some("provider-session.detached".into()),
            after_sequence: stored[1198].sequence,
            ..Default::default()
        })
        .await?;
    sink.commit(request("other-thread", "other")).await?;
    let live = sink.commit(request("matching", "thread")).await?.events;
    assert_eq!(stream.next().await?, Some(stored[1199].clone()));
    assert_eq!(stream.next().await?, Some(stored[1200].clone()));
    assert_eq!(stream.next().await?, Some(live[0].clone()));
    let mut future = sink
        .stream(StreamOptions {
            after_sequence: live[0].sequence + 1,
            ..Default::default()
        })
        .await?;
    sink.commit(request("at-cursor", "thread")).await?;
    let after = sink.commit(request("after-cursor", "thread")).await?.events;
    assert_eq!(future.next().await?, Some(after[0].clone()));
    fixture
        .stores
        .transaction(|tx| {
            tx.execute(
                "UPDATE orchestration_events SET event_type='future.event' WHERE sequence=1",
                [],
            )?;
            Ok(())
        })
        .await?;
    for bounded in [false, true] {
        let mut strict = sink
            .stream(StreamOptions {
                bounded,
                ..Default::default()
            })
            .await?;
        assert!(
            strict.next().await.is_err(),
            "sink replay must surface undecodable persisted events"
        );
    }
    Ok(())
}

#[tokio::test]
async fn sink_internal_subscribers_survive_rpc_overflow_and_filter_before_retaining() -> Result<()>
{
    let fixture = TestDatabase::memory()?;
    let sink = sink(&fixture, OutboxSignals::default());
    let mut internal = sink.stream(StreamOptions::default()).await?;
    let mut bounded = sink
        .stream(StreamOptions {
            bounded: true,
            ..Default::default()
        })
        .await?;
    let mut filtered = sink
        .stream(StreamOptions {
            thread_id: Some(ThreadId("target".into())),
            bounded: true,
            ..Default::default()
        })
        .await?;
    let mut input = request("overflow", "thread");
    input.decision.events = (0..1100)
        .map(|index| detached_event(&format!("overflow:{index}"), "thread"))
        .collect::<Result<_>>()?;
    let events = sink.commit(input).await?.events;
    assert!(matches!(bounded.next().await, Err(Error::Lagged)));
    for expected in events {
        assert_eq!(internal.next().await?, Some(expected));
    }
    let target = sink.commit(request("target", "target")).await?.events;
    assert_eq!(internal.next().await?, Some(target[0].clone()));
    assert_eq!(filtered.next().await?, Some(target[0].clone()));
    Ok(())
}

#[tokio::test]
async fn sink_records_outcomes_only_after_commit_and_before_publication() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    let sink = sink(&fixture, OutboxSignals::default());
    let mut outcomes = sink.subscribe_outcomes();
    let mut published = sink.subscribe();
    let mut run: serde_json::Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2Run.jsonl")
            .lines()
            .next()
            .unwrap(),
    )?;
    run["id"] = "outcome-run".into();
    run["threadId"] = "thread".into();
    run["providerInstanceId"] = "codex".into();
    run["modelSelection"]["model"] = "gpt-6".into();
    run["status"] = "completed".into();
    run["requestedAt"] = "2026-11-03T00:00:00.000Z".into();
    run["startedAt"] = "2026-11-03T00:00:02.000Z".into();
    run["completedAt"] = "2026-11-03T00:00:05.000Z".into();
    let make_input = |id: &str| -> Result<CommitRequest> {
        let mut input = request(id, "thread");
        input.decision.events = vec![serde_json::from_value(
            serde_json::json!({"id":id,"threadId":"thread","type":"run.updated","occurredAt":TEST_TIME,"payload":run}),
        )?];
        Ok(input)
    };
    fixture.stores.transaction(|tx| {
        tx.execute_batch("CREATE TRIGGER fail_outcome BEFORE UPDATE ON orchestration_command_receipts BEGIN SELECT RAISE(ABORT,'outcome rollback'); END")?;
        Ok(())
    }).await?;
    assert!(
        sink.commit(make_input("rolled-back-outcome")?)
            .await
            .is_err()
    );
    assert!(matches!(
        outcomes.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    fixture
        .stores
        .transaction(|tx| {
            tx.execute_batch("DROP TRIGGER fail_outcome")?;
            Ok(())
        })
        .await?;
    sink.commit(make_input("committed-outcome")?).await?;
    published.recv().await.unwrap();
    let outcome = outcomes
        .try_recv()
        .expect("outcome must precede publication");
    assert_eq!(outcome.run_id, RunId("outcome-run".into()));
    assert_eq!(outcome.outcome, "completed");
    assert_eq!(outcome.model_family, Some("gpt"));
    assert_eq!(outcome.queue_wait_ms, Some(2000));
    assert_eq!(outcome.duration_ms, Some(3000));
    sink.commit(make_input("later-update")?).await?;
    assert!(matches!(
        outcomes.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    Ok(())
}

#[tokio::test]
async fn prepared_commit_shares_sqlite_transaction_and_skips_duplicate_or_stale_callbacks()
-> Result<()> {
    let fixture = TestDatabase::temporary()?;
    let sink = sink(&fixture, OutboxSignals::default());
    fixture
        .stores
        .transaction(|tx| {
            tx.execute_batch("CREATE TABLE prepared_markers(value TEXT NOT NULL)")?;
            Ok(())
        })
        .await?;
    let called = Arc::new(AtomicUsize::new(0));
    let prepare = |called: Arc<AtomicUsize>| -> t3_orchestration::api::CommitPreparation {
        Box::new(move |tx, mut input| {
            called.fetch_add(1, Ordering::SeqCst);
            tx.execute("INSERT INTO prepared_markers VALUES ('prepared')", [])?;
            input
                .decision
                .events
                .push(detached_event("prepared:event", "thread")?);
            Ok(input)
        })
    };
    let mut events = sink.subscribe();
    let committed = sink
        .commit_prepared(
            request("prepared", "thread"),
            Default::default(),
            prepare(called.clone()),
        )
        .await?;
    assert_eq!(committed.events.len(), 2);
    events.recv().await.unwrap();
    fixture
        .stores
        .read(|db| {
            assert_eq!(
                db.query_row("SELECT COUNT(*) FROM prepared_markers", [], |row| row
                    .get::<_, i64>(0))?,
                1
            );
            Ok(())
        })
        .await?;
    let duplicate = sink
        .commit_prepared(
            request("prepared", "thread"),
            Default::default(),
            prepare(called.clone()),
        )
        .await?;
    assert!(!duplicate.committed);
    assert_eq!(duplicate.events, committed.events);
    let mut stale = request("prepared-stale", "thread");
    stale.guard = CommitGuard::RunCurrent {
        thread_id: ThreadId("thread".into()),
        run_id: RunId("missing".into()),
        attempt_id: RunAttemptId("attempt".into()),
        status: OrchestrationV2RunStatus::Running,
    };
    assert!(
        !sink
            .commit_prepared(stale, Default::default(), prepare(called.clone()))
            .await?
            .committed
    );
    assert_eq!(called.load(Ordering::SeqCst), 1);
    fixture.stores.transaction(|tx| { tx.execute_batch("CREATE TRIGGER prepared_failure BEFORE UPDATE ON orchestration_command_receipts BEGIN SELECT RAISE(ABORT,'prepared rollback'); END")?; Ok(()) }).await?;
    let error = sink
        .commit_prepared(
            request("prepared-failed", "thread"),
            Default::default(),
            Box::new(|tx, input| {
                tx.execute("INSERT INTO prepared_markers VALUES ('must roll back')", [])?;
                Ok(input)
            }),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("prepared rollback"));
    fixture
        .stores
        .read(|db| {
            assert_eq!(
                db.query_row("SELECT COUNT(*) FROM prepared_markers", [], |row| row
                    .get::<_, i64>(0))?,
                1
            );
            assert!(receipts::get(db, &CommandId("prepared-failed".into()))?.is_none());
            assert!(outbox::get(db, "effect:prepared-failed")?.is_none());
            Ok(())
        })
        .await
}

#[tokio::test]
async fn preparation_cannot_replace_receipt_identity_or_bypass_commit_guards() -> Result<()> {
    for mode in [
        "replace-receipt",
        "remove-receipt",
        "add-receipt",
        "new-guard",
        "weaken-guard",
    ] {
        let fixture = TestDatabase::memory()?;
        let sink = sink(&fixture, OutboxSignals::default());
        let mut published = sink.subscribe();
        fixture.stores.transaction(|tx| {
            tx.execute_batch("CREATE TABLE prepared_markers(value TEXT);
                INSERT INTO orchestration_v2_projection_runs(run_id,thread_id,ordinal,provider,status,requested_at,payload_json)
                VALUES('run','thread',1,'codex','running','2026-11-03T00:00:00.000Z','{\"activeAttemptId\":\"attempt\"}');")?;
            Ok(())
        }).await?;
        let mut input = request("guarded-preparation", "thread");
        if mode == "add-receipt" {
            input.receipt = None;
        }
        if mode == "weaken-guard" {
            input.guard = CommitGuard::RunCurrent {
                thread_id: ThreadId("thread".into()),
                run_id: RunId("run".into()),
                attempt_id: RunAttemptId("attempt".into()),
                status: OrchestrationV2RunStatus::Running,
            };
        }
        let result = sink.commit_prepared(input, Default::default(), Box::new(move |tx, mut input| {
            tx.execute("INSERT INTO prepared_markers VALUES ('must roll back')", [])?;
            match mode {
                "replace-receipt" => input.receipt.as_mut().unwrap().command_id = CommandId("replacement".into()),
                "remove-receipt" => input.receipt = None,
                "add-receipt" => input.receipt = request("replacement", "thread").receipt,
                "new-guard" => input.guard = CommitGuard::RunCurrent {
                    thread_id: ThreadId("thread".into()), run_id: RunId("run".into()),
                    attempt_id: RunAttemptId("stale".into()), status: OrchestrationV2RunStatus::Running,
                },
                "weaken-guard" => {
                    tx.execute("UPDATE orchestration_v2_projection_runs SET status='completed' WHERE run_id='run'", [])?;
                    input.guard = CommitGuard::None;
                },
                _ => unreachable!(),
            }
            Ok(input)
        })).await;
        assert!(matches!(result, Err(Error::Stale)), "{mode}: {result:?}");
        fixture
            .stores
            .read(|db| {
                for table in [
                    "prepared_markers",
                    "orchestration_command_receipts",
                    "orchestration_events",
                    "orchestration_v2_effect_outbox",
                    "projection_projects",
                ] {
                    assert_eq!(
                        db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                            .get::<_, i64>(0))?,
                        0,
                        "{table}"
                    );
                }
                assert_eq!(
                    db.query_row(
                        "SELECT status FROM orchestration_v2_projection_runs WHERE run_id='run'",
                        [],
                        |row| row.get::<_, String>(0)
                    )?,
                    "running"
                );
                Ok(())
            })
            .await?;
        assert!(matches!(
            published.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }
    Ok(())
}

mod executor_routes {
    use super::*;
    use serde_json::{Value, json};
    use std::sync::atomic::AtomicBool;
    use t3_orchestration::{
        api::*,
        ports::commit_worker::effect_worker::ServiceExecutor,
        provider::{ProviderError, RuntimeResponse},
        testing::{
            FakeCheckpointRollback, FakeProjectionReader, FakeProviderTurnControl,
            FakeRuntimeRequestResponder, FakeSessionDirectory, FakeThreadOperations,
        },
    };

    #[derive(Default)]
    struct Dispatch {
        commands: Mutex<Vec<Value>>,
        fail: AtomicBool,
    }
    #[async_trait]
    impl CommandDispatcher for Dispatch {
        async fn dispatch(&self, command: OrchestrationV2ServerCommand) -> Result<CommandReceipt> {
            let command = serde_json::to_value(command)?;
            self.commands
                .lock()
                .expect("test value")
                .push(command.clone());
            if self.fail.load(Ordering::SeqCst) {
                return Err(Error::Unsupported("failure recorder unavailable"));
            }
            Ok(CommandReceipt {
                command_id: serde_json::from_value(command["commandId"].clone())?,
                aggregate: AggregateId::Thread(serde_json::from_value(
                    command["threadId"].clone(),
                )?),
                command_type: command["type"].as_str().expect("test value").into(),
                accepted_at: TEST_TIME.into(),
                result_sequence: 1,
                status: ReceiptStatus::Accepted,
                error: None,
            })
        }
    }
    struct Cleanup;
    #[async_trait]
    impl ResourceCleanup for Cleanup {
        async fn cleanup(&self, _thread: ThreadId, _kind: ResourceKind) -> Result<()> {
            Ok(())
        }
    }
    struct Fixture {
        db: TestDatabase,
        executor: ServiceExecutor,
        control: Arc<FakeProviderTurnControl>,
        rollback: Arc<FakeCheckpointRollback>,
        requests: Arc<FakeRuntimeRequestResponder>,
        threads: Arc<FakeThreadOperations>,
        sessions: Arc<FakeSessionDirectory>,
        dispatch: Arc<Dispatch>,
    }
    impl Fixture {
        fn new() -> Result<Self> {
            Self::with_database(TestDatabase::memory()?)
        }
        fn with_database(db: TestDatabase) -> Result<Self> {
            let control = Arc::new(FakeProviderTurnControl::default());
            let rollback = Arc::new(FakeCheckpointRollback::default());
            let requests = Arc::new(FakeRuntimeRequestResponder::default());
            let threads = Arc::new(FakeThreadOperations::default());
            let sessions = Arc::new(FakeSessionDirectory::default());
            let dispatch = Arc::new(Dispatch::default());
            let executor = ServiceExecutor {
                threads: threads.clone(),
                sessions: sessions.clone(),
                dispatcher: dispatch.clone(),
                projections: Arc::new(FakeProjectionReader::default()),
                control: control.clone(),
                rollback: rollback.clone(),
                requests: requests.clone(),
                cleanup: Arc::new(Cleanup),
            };
            Ok(Self {
                db,
                executor,
                control,
                rollback,
                requests,
                threads,
                sessions,
                dispatch,
            })
        }
        async fn claim(&self, request: EffectRequest) -> Result<EffectRow> {
            self.db
                .stores
                .transaction(move |tx| {
                    outbox::enqueue(
                        tx,
                        &[pending("routed-effect", "thread", request)],
                        TEST_TIME,
                    )?;
                    Ok(outbox::claim_next(
                        tx,
                        &ClaimOptions {
                            worker_id: "route-worker".into(),
                            ..Default::default()
                        },
                        TEST_TIME,
                    )?
                    .expect("test value"))
                })
                .await
        }
        fn commands(&self) -> Vec<Value> {
            self.dispatch.commands.lock().expect("test value").clone()
        }
    }
    fn failure(detail: &str) -> Error {
        Error::Service {
            operation: "sentinel",
            detail: detail.into(),
        }
    }

    struct RuntimeCleanup {
        stores: StoreDatabase,
        failed: AtomicBool,
    }
    #[async_trait]
    impl ResourceCleanup for RuntimeCleanup {
        async fn cleanup(&self, thread: ThreadId, kind: ResourceKind) -> Result<()> {
            self.stores.read(move |db| {
                assert!(receipts::get_thread(db, &CommandId("real-runtime".into()))?.is_some());
                assert_eq!(db.query_row("SELECT title FROM orchestration_v2_projection_threads WHERE thread_id=?1", [&thread.0], |row| row.get::<_, String>(0))?, "Real projection");
                Ok(())
            }).await?;
            if matches!(kind, ResourceKind::Terminals) && !self.failed.swap(true, Ordering::SeqCst)
            {
                return Err(failure("retry the cleanup"));
            }
            Ok(())
        }
    }

    fn created_thread_event(id: &str, title: &str) -> Result<OrchestrationV2DomainEvent> {
        let mut thread: Value = serde_json::from_str(
            include_str!("../../../../fixtures/samples/OrchestrationV2AppThread.jsonl")
                .lines()
                .next()
                .expect("test value"),
        )?;
        thread["id"] = "thread".into();
        thread["projectId"] = "project".into();
        thread["title"] = title.into();
        thread["providerInstanceId"] = "codex".into();
        thread["modelSelection"] = json!({"instanceId":"codex","model":"m"});
        for key in [
            "deletedAt",
            "archivedAt",
            "activeProviderThreadId",
            "branch",
            "worktreePath",
        ] {
            thread[key] = Value::Null;
        }
        Ok(serde_json::from_value(
            json!({"id":id,"threadId":"thread","type":"thread.created","occurredAt":TEST_TIME,"payload":thread}),
        )?)
    }

    type ControlShim = FakeProviderTurnControl;

    struct NoTitleCheckpoints;

    #[async_trait]
    impl CheckpointBackend for NoTitleCheckpoints {
        async fn capture(
            &self,
            _: CaptureCheckpoint,
            _: CancellationToken,
        ) -> Result<CheckpointRef> {
            unreachable!("title generation does not capture checkpoints")
        }
        async fn lookup(
            &self,
            _: OrchestrationV2CheckpointScope,
            _: u64,
        ) -> Result<Option<CheckpointRef>> {
            unreachable!("title generation does not look up checkpoints")
        }
        async fn diff(
            &self,
            _: std::path::PathBuf,
            _: CheckpointRef,
            _: CheckpointRef,
        ) -> Result<CheckpointDiff> {
            unreachable!("title generation does not diff checkpoints")
        }
        async fn restore(
            &self,
            _: OrchestrationV2CheckpointScope,
            _: u64,
            _: CancellationToken,
        ) -> Result<()> {
            unreachable!("title generation does not restore checkpoints")
        }
        async fn prune(&self, _: OrchestrationV2CheckpointScope, _: Vec<u64>) -> Result<()> {
            unreachable!("title generation does not prune checkpoints")
        }
    }

    #[async_trait]
    impl t3_orchestration::ports::checkpoints_forks::checkpoint_service::CheckpointMetadata
        for NoTitleCheckpoints
    {
        fn reference(&self, _: &CheckpointScopeId, _: u64) -> CheckpointRef {
            unreachable!("title generation does not name checkpoint refs")
        }
        async fn is_repository(&self, _: std::path::PathBuf) -> Result<bool> {
            unreachable!("title generation does not inspect repositories")
        }
        async fn delete_refs(
            &self,
            _: OrchestrationV2CheckpointScope,
            _: Vec<OrchestrationV2Checkpoint>,
        ) -> Result<()> {
            unreachable!("title generation does not delete checkpoint refs")
        }
    }

    #[tokio::test]
    async fn composed_worker_generates_and_persists_a_title_through_the_real_title_service()
    -> Result<()> {
        use t3_orchestration::{
            composition::{OrchestrationRuntime, RuntimePorts, StaticSettings},
            ports::sessions_recovery::provider_adapter_registry::AdapterRegistry,
            testing::FakeTitleGenerator,
        };

        for configured in [false, true] {
            let fixture = Fixture::with_database(TestDatabase::temporary()?)?;
            let clock = Arc::new(ManualClock::new(TEST_TIME));
            let generator = Arc::new(FakeTitleGenerator::default());
            generator
                .generate_with_input
                .push(Ok("Generated by the title service".into()));
            let mut ports = RuntimePorts::new(
                fixture.db.stores.clone(),
                Arc::new(AdapterRegistry::from_adapters(vec![])),
                Arc::new(NoTitleCheckpoints),
                clock.clone(),
            );
            ports.settings = Arc::new(StaticSettings::new(
                t3_contracts::settings::ServerSettings::default(),
            ));
            if configured {
                ports.titles = Some(generator.clone());
            }
            ports.worker.max_attempts = 1;
            let runtime = OrchestrationRuntime::start(ports).await?;
            fixture
                .db
                .stores
                .transaction(|db| projects::apply(db, &project_event("title-project", "project")?))
                .await?;
            let message = json!({"id":"title-message","threadId":"thread","runId":null,"nodeId":null,"role":"user","text":"Investigate the title routing","attachments":[],"streaming":false,"createdBy":"user","creationSource":"web","createdAt":TEST_TIME,"updatedAt":TEST_TIME});
            runtime.events.commit(CommitRequest {
            receipt: None,
            decision: Decision {
                events: vec![
                    created_thread_event("title-thread", "Original title")?,
                    serde_json::from_value(json!({"id":"title-message-event","threadId":"thread","type":"message.updated","occurredAt":TEST_TIME,"payload":message}))?,
                ],
                ..Default::default()
            },
            guard: CommitGuard::None,
        }).await?;
            runtime.dispatch_command(serde_json::from_value(json!({"type":"thread.metadata.update","threadId":"thread","commandId":"regenerate-title","regenerateTitle":true}))?).await?;

            let effect_id = fixture.db.stores.read(|db| {
            Ok(db.query_row("SELECT effect_id FROM orchestration_v2_effect_outbox WHERE command_id='regenerate-title'", [], |row| row.get::<_, String>(0))?)
        }).await?;
            // Observes the composed worker's settlement signal without claiming its work.
            let observer = worker(
                &fixture.db,
                Arc::new(fixture.executor),
                clock,
                runtime.events.signals(),
                WorkerOptions::default(),
            );
            observer.await_settled(effect_id.clone()).await?;
            let effect = rows(&fixture.db.stores, &effect_id).await?;
            assert_eq!(
                effect.status,
                if configured {
                    EffectStatus::Succeeded
                } else {
                    EffectStatus::Failed
                },
                "{effect:?}"
            );
            assert_eq!(effect.attempt_count, 1);
            let thread = runtime
                .projections
                .thread(ThreadId("thread".into()))
                .await?
                .unwrap();
            let thread = t3_contracts::stored::to_value(&thread)?;
            if configured {
                assert_eq!(thread["title"], "Generated by the title service");
                assert!(thread["titleRegeneration"].is_null());
                assert_eq!(thread["lastTitleRegeneration"]["status"], "renamed");
                assert_eq!(
                    thread["lastTitleRegeneration"]["requestId"],
                    "regenerate-title"
                );
                let (input, _) = generator.generate_with_input.next_call().await.unwrap();
                assert_eq!(input.thread_id, ThreadId("thread".into()));
                assert_eq!(input.previous_title.as_deref(), Some("Original title"));
                assert!(input.message.contains("Investigate the title routing"));
            } else {
                assert_eq!(
                    effect.last_error.as_deref(),
                    Some("unsupported operation: TitleGenerator")
                );
                assert_eq!(thread["title"], "Original title");
                assert_eq!(thread["titleRegeneration"]["requestId"], "regenerate-title");
            }
            observer.shutdown().await?;
            runtime.shutdown().await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn confirmed_subagent_outcomes_repair_only_confirmed_evidence_through_the_shared_seam()
    -> Result<()> {
        use t3_orchestration::provider::{SubagentOutcome, TerminalTaskStatus};
        for confirmed in [true, false] {
            let fixture = Fixture::new()?;
            let mut input = interrupt();
            let EffectRequest::Interrupt { subagent, .. } = &mut input else {
                unreachable!()
            };
            *subagent = Some(InterruptSubagent {
                id: NodeId("subagent".into()),
                native_task_id: "native-task".into(),
            });
            let effect = fixture.claim(input).await?;
            let control = Arc::new(ControlShim::default());
            control
                .interrupt_with_outcome
                .push(Ok(confirmed.then(|| SubagentOutcome {
                    status: TerminalTaskStatus::Cancelled,
                    completed_at: t3_contracts::wire::WireDateTime(TEST_TIME.into()),
                    result: Some("Stopped by Claude".into()),
                })));
            let dispatch = fixture.dispatch.clone();
            let mut executor = fixture.executor;
            executor.control = control.clone();
            executor
                .execute(effect.clone(), CancellationToken::new())
                .await?;
            {
                let commands = dispatch.commands.lock().unwrap();
                assert_eq!(commands.len(), usize::from(confirmed));
                if confirmed {
                    assert_eq!(commands[0]["type"], "thread.background-work.settle");
                    assert_eq!(
                        commands[0]["subagent"],
                        json!({"id":"subagent","stoppedAt":effect.created_at,"outcome":{"status":"cancelled","completedAt":TEST_TIME,"result":"Stopped by Claude"}})
                    );
                }
            }
            let (input, _) = control.interrupt_with_outcome.next_call().await.unwrap();
            assert_eq!(input.subagent.unwrap().native_task_id, "native-task");
        }
        Ok(())
    }

    #[tokio::test]
    async fn typed_control_failures_preserve_flags_causes_and_completed_turn_followups()
    -> Result<()> {
        use t3_orchestration::ports::runs_ingestion::provider_turn_control_service::ProviderTurnControlError;
        for mode in ["completed", "declined", "unrelated", "cancelled"] {
            let fixture = Fixture::new()?;
            let effect = fixture
                .claim(EffectRequest::Steer {
                    provider_session_id: ProviderSessionId("session".into()),
                    provider_thread_id: ProviderThreadId("provider-thread".into()),
                    provider_turn_id: ProviderTurnId("turn".into()),
                    message_id: MessageId("message".into()),
                    model_selection: None,
                    steer_follow_up_after_run_ordinal: Some(8),
                })
                .await?;
            let control = Arc::new(ControlShim::default());
            let error = Error::ProviderTurnControl(Box::new(ProviderTurnControlError {
                thread_id: ThreadId("thread".into()),
                provider_turn_id: ProviderTurnId("turn".into()),
                operation: "steer",
                turn_completed: mode == "completed",
                steer_declined: mode == "declined",
                unresponsive: false,
                cause: Box::new(if mode == "cancelled" {
                    Error::Cancelled
                } else {
                    failure("original typed cause")
                }),
            }));
            control.steer.push(Err(error));
            let reader = Arc::new(FakeProjectionReader::default());
            let OrchestrationV2DomainEvent::ThreadCreated(created) =
                created_thread_event("thread-event", "Thread")?
            else {
                unreachable!()
            };
            let mut run: Value = serde_json::from_str(
                include_str!("../../../../fixtures/samples/OrchestrationV2Run.jsonl")
                    .lines()
                    .next()
                    .unwrap(),
            )?;
            run["id"] = "run".into();
            run["threadId"] = "thread".into();
            let mut message: Value = serde_json::from_str(
                include_str!(
                    "../../../../fixtures/samples/OrchestrationV2ConversationMessage.jsonl"
                )
                .lines()
                .next()
                .unwrap(),
            )?;
            message["id"] = "message".into();
            message["threadId"] = "thread".into();
            message["runId"] = "run".into();
            for key in ["delegatedCompletion", "notification"] {
                message[key] = Value::Null;
            }
            reader.query_records.push(Ok(ProjectionRecords {
                thread: created.payload,
                records: vec![
                    ProjectionRecordSet::Runs(vec![serde_json::from_value(run)?]),
                    ProjectionRecordSet::Messages(vec![serde_json::from_value(message)?]),
                ],
            }));
            let dispatch = fixture.dispatch.clone();
            let mut executor = fixture.executor;
            executor.projections = reader;
            executor.control = control;
            let result = executor.execute(effect, CancellationToken::new()).await;
            let commands = dispatch.commands.lock().unwrap();
            if mode == "cancelled" {
                assert!(matches!(result, Err(Error::Cancelled)));
                assert!(commands.is_empty());
            } else if mode == "unrelated" {
                let Error::ProviderTurnControl(cause) = result.unwrap_err() else {
                    panic!("typed error shim")
                };
                assert!(cause.cause.to_string().contains("original typed cause"));
                assert!(commands.is_empty());
            } else {
                result?;
                assert_eq!(commands.len(), 1);
                assert_eq!(
                    commands[0]["dispatchMode"]["type"],
                    if mode == "declined" {
                        "queue_after_active"
                    } else {
                        "start_immediately"
                    }
                );
                assert_eq!(
                    commands[0].get("steerFollowUpOfRunId"),
                    (mode == "declined").then_some(&json!("run"))
                );
                if mode == "declined" {
                    assert_eq!(commands[0]["steerFollowUpAfterRunOrdinal"], 8);
                }
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn delegated_recovery_runs_after_capture_and_only_after_the_last_continuation_failure()
    -> Result<()> {
        for capture in [false, true] {
            let fixture = Fixture::new()?;
            let effect = fixture
                .claim(if capture {
                    EffectRequest::Capture {
                        run_id: RunId("run".into()),
                        scope_id: CheckpointScopeId("scope".into()),
                    }
                } else {
                    EffectRequest::Continue {
                        source_run_id: RunId("run".into()),
                    }
                })
                .await?;
            for will_retry in [true, false] {
                if capture {
                    fixture.threads.finalize.push(Ok(()));
                } else {
                    fixture
                        .threads
                        .continue_run_with_options
                        .push(Err(failure("provider removed")));
                }
                if capture || !will_retry {
                    fixture.threads.recover_delegated_task.push(Ok(()));
                }
                let result = fixture
                    .executor
                    .execute_with_options(
                        effect.clone(),
                        CancellationToken::new(),
                        EffectExecutionOptions { will_retry },
                    )
                    .await;
                if capture {
                    result?;
                } else {
                    assert!(result.unwrap_err().to_string().contains("provider removed"));
                }
                if capture || !will_retry {
                    let (thread, source, completed) = fixture
                        .threads
                        .recover_delegated_task
                        .next_call()
                        .await
                        .unwrap();
                    assert_eq!(
                        (thread, source, completed),
                        (
                            ThreadId("thread".into()),
                            (!capture).then(|| RunId("run".into())),
                            capture.then(|| RunId("run".into()))
                        )
                    );
                }
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn real_projections_and_service_executor_publish_commit_retry_settle_and_reopen()
    -> Result<()> {
        use t3_orchestration::projections::{
            store::ProjectionStore, write::SqliteProjectionWriter,
        };
        let directory = {
            let mut fixture = Fixture::with_database(TestDatabase::temporary()?)?;
            let clock = Arc::new(ManualClock::new(TEST_TIME));
            let signals = OutboxSignals::default();
            let reader = Arc::new(ProjectionStore::new(fixture.db.stores.clone()));
            fixture.executor.projections = reader.clone();
            fixture.executor.cleanup = Arc::new(RuntimeCleanup {
                stores: fixture.db.stores.clone(),
                failed: AtomicBool::new(false),
            });
            let sink = EventSink::new(
                fixture.db.stores.clone(),
                Arc::new(SqliteProjectionWriter),
                clock.clone(),
                signals.clone(),
            );
            let mut raw = sink.subscribe();
            let mut shared = sink.subscribe_shared()?;
            let mut input = request("real-runtime", "thread");
            input.decision.events = vec![
                created_thread_event("real-created", "Real projection")?,
                detached_event("real-detached", "thread")?,
            ];
            let result = sink.commit(input).await?;
            for stored in &result.events {
                let publication = raw.recv().await.unwrap();
                assert_eq!(publication, *shared.recv().await.unwrap());
                assert_eq!(
                    publication,
                    t3_contracts::application_event::ApplicationStoredEvent::Orchestration(
                        Box::new(stored.clone())
                    )
                );
            }
            assert_eq!(
                reader
                    .thread(ThreadId("thread".into()))
                    .await?
                    .unwrap()
                    .title
                    .0,
                "Real projection"
            );
            assert_eq!(result.events.len(), 2);
            let worker = worker(
                &fixture.db,
                Arc::new(fixture.executor),
                clock.clone(),
                signals,
                WorkerOptions::default(),
            );
            assert!(worker.run_once(ClaimOptions::default()).await?);
            let retry = rows(&fixture.db.stores, "effect:real-runtime").await?;
            assert_eq!(retry.status, EffectStatus::Pending);
            clock.set(&retry.available_at);
            assert!(worker.run_once(ClaimOptions::default()).await?);
            worker.await_settled("effect:real-runtime".into()).await?;
            assert_eq!(
                rows(&fixture.db.stores, "effect:real-runtime")
                    .await?
                    .status,
                EffectStatus::Succeeded
            );
            assert_eq!(worker.drain(usize::MAX).await?, 0);
            worker.shutdown().await?;
            sink.drain().await;
            fixture.db.directory.take().unwrap()
        };
        let reopened = StoreDatabase(t3_persistence::Database::open(
            directory.path().join("statev2.sqlite"),
        )?);
        reopened
            .transaction(|tx| {
                assert_eq!(
                    outbox::reconcile_after_process_loss(tx, TEST_TIME)?,
                    Reconciliation::default()
                );
                assert_eq!(
                    outbox::get(tx, "effect:real-runtime")?.unwrap().status,
                    EffectStatus::Succeeded
                );
                assert_eq!(
                    receipts::get_thread(tx, &CommandId("real-runtime".into()))?
                        .unwrap()
                        .result_sequence,
                    2
                );
                Ok(())
            })
            .await
    }

    #[tokio::test]
    async fn continuation_receives_the_remaining_retry_budget() -> Result<()> {
        let fixture = Fixture::new()?;
        let effect = fixture
            .claim(EffectRequest::Continue {
                source_run_id: RunId("source-run".into()),
            })
            .await?;
        for will_retry in [true, false] {
            fixture.threads.continue_run_with_options.push(Ok(()));
            fixture
                .executor
                .execute_with_options(
                    effect.clone(),
                    CancellationToken::new(),
                    EffectExecutionOptions { will_retry },
                )
                .await?;
            let (thread, run, options) = fixture
                .threads
                .continue_run_with_options
                .next_call()
                .await
                .unwrap();
            assert_eq!(thread, ThreadId("thread".into()));
            assert_eq!(run, RunId("source-run".into()));
            assert_eq!(options.will_retry, will_retry);
        }
        Ok(())
    }

    struct DropCleanup {
        entered: mpsc::UnboundedSender<()>,
        dropped: Arc<AtomicUsize>,
    }
    struct DropMarker(Arc<AtomicUsize>);
    impl Drop for DropMarker {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    #[async_trait]
    impl ResourceCleanup for DropCleanup {
        async fn cleanup(&self, _: ThreadId, _: ResourceKind) -> Result<()> {
            let _marker = DropMarker(self.dropped.clone());
            self.entered.send(()).expect("test value");
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn real_executor_shutdown_interrupts_tokenless_service_and_leaves_recovery_owned_intent()
    -> Result<()> {
        let mut fixture = Fixture::new()?;
        let (entered, mut calls) = mpsc::unbounded_channel();
        let dropped = Arc::new(AtomicUsize::new(0));
        fixture.executor.cleanup = Arc::new(DropCleanup {
            entered,
            dropped: dropped.clone(),
        });
        enqueue(
            &fixture.db,
            vec![
                pending("shutdown-real", "thread", EffectRequest::CleanupPreviews),
                pending(
                    "z-never-admitted",
                    "other-thread",
                    EffectRequest::CleanupPreviews,
                ),
            ],
        )
        .await?;
        let worker = worker(
            &fixture.db,
            Arc::new(fixture.executor),
            Arc::new(ManualClock::new(TEST_TIME)),
            OutboxSignals::default(),
            WorkerOptions::default(),
        );
        let running = tokio::spawn({
            let worker = worker.clone();
            async move { worker.run_once(ClaimOptions::default()).await }
        });
        calls.recv().await.unwrap();
        worker.shutdown().await?;
        assert!(running.await.unwrap()?);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        assert_eq!(worker.executing(), 0);
        assert!(matches!(
            worker.run_once(ClaimOptions::default()).await,
            Err(Error::Cancelled)
        ));
        assert_eq!(
            rows(&fixture.db.stores, "shutdown-real").await?.status,
            EffectStatus::Running
        );
        assert_eq!(
            rows(&fixture.db.stores, "z-never-admitted")
                .await?
                .attempt_count,
            0
        );
        fixture
            .db
            .stores
            .transaction(|tx| {
                assert_eq!(
                    outbox::reconcile_after_process_loss(tx, TEST_TIME)?.requeued,
                    1
                );
                assert_eq!(
                    outbox::get(tx, "shutdown-real")?.unwrap().status,
                    EffectStatus::Pending
                );
                Ok(())
            })
            .await
    }

    #[tokio::test]
    async fn concrete_control_service_preserves_its_real_typed_failure() -> Result<()> {
        use t3_orchestration::{
            ports::runs_ingestion::provider_turn_control_service::ProviderTurnControlService,
            projections::store::ProjectionStore,
        };
        let fixture = Fixture::new()?;
        let service = ProviderTurnControlService {
            projections: Arc::new(ProjectionStore::new(fixture.db.stores.clone())),
            sessions: fixture.sessions.clone(),
            events: Arc::new(sink(&fixture.db, OutboxSignals::default())),
        };
        let error = ProviderTurnControl::interrupt_with_outcome(
            &service,
            InterruptTurn {
                target: ProviderControlTarget {
                    thread_id: ThreadId("missing".into()),
                    provider_session_id: ProviderSessionId("session".into()),
                    provider_thread_id: ProviderThreadId("provider-thread".into()),
                    provider_turn_id: ProviderTurnId("turn".into()),
                },
                subagent: None,
                requested_at: None,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        let Error::ProviderTurnControl(control) = error else {
            panic!("typed source transport")
        };
        assert_eq!(control.thread_id, ThreadId("missing".into()));
        assert_eq!(control.operation, "interrupt");
        assert!(!control.unresponsive);
        assert!(matches!(
            control.cause.as_ref(),
            Error::ThreadNotFound(_) | Error::Service { .. }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn root_interrupt_settles_missing_turn_but_flags_unconfirmed_timeout() -> Result<()> {
        for unresponsive in [false, true] {
            let fixture = Fixture::new()?;
            let effect = fixture.claim(interrupt()).await?;
            fixture
                .control
                .interrupt_with_outcome
                .push(Err(if unresponsive {
                    Error::Provider(ProviderError::InterruptTimeout {
                        turn_id: ProviderTurnId("turn".into()),
                        timeout_ms: 30_000,
                    })
                } else {
                    failure("Provider turn is not active.")
                }));
            fixture
                .executor
                .execute(effect.clone(), CancellationToken::new())
                .await?;
            let commands = fixture.commands();
            assert_eq!(commands.len(), 1);
            let kind = if unresponsive {
                "thread.interrupt.unresponsive"
            } else {
                "thread.background-work.settle"
            };
            assert_eq!(commands[0]["type"], kind);
            assert_eq!(
                commands[0]["commandId"],
                format!(
                    "{}:{}",
                    effect.id,
                    if unresponsive {
                        "unresponsive"
                    } else {
                        "background-work-settled"
                    }
                )
            );
            assert_eq!(commands[0]["threadId"], "thread");
            assert!(commands[0].get("subagent").is_none());
            let (call, _) = fixture
                .control
                .interrupt_with_outcome
                .next_call()
                .await
                .unwrap();
            assert_eq!(
                call.target.provider_turn_id,
                match effect.request {
                    EffectRequest::Interrupt {
                        provider_turn_id, ..
                    } => provider_turn_id,
                    _ => unreachable!(),
                }
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn rollback_reports_only_final_failure_with_safe_client_detail() -> Result<()> {
        const LOCKED: &str =
            "Git's index is locked. Wait for other Git operations to finish, then try again.";
        for (operation, detail, expected) in [
            ("checkpoint.git.index-locked", LOCKED, LOCKED),
            ("checkpoint.restore", "error: could not clean /private/repo/secret.txt", t3_orchestration::ports::checkpoints_forks::checkpoint_rollback_service::ROLLBACK_FAILED_MESSAGE),
        ] {
            let fixture = Fixture::new()?;
            let effect = fixture.claim(EffectRequest::Rollback { restore_files: Some(true), provider_thread_id: ProviderThreadId("provider-thread".into()), checkpoint_id: CheckpointId("checkpoint".into()), scope_id: CheckpointScopeId("scope".into()) }).await?;
            for will_retry in [true, false] {
                fixture.rollback.rollback.push(Err(Error::Service { operation, detail: detail.into() }));
                let error = fixture.executor.execute_with_options(effect.clone(), CancellationToken::new(), EffectExecutionOptions { will_retry }).await.unwrap_err();
                assert!(error.to_string().contains(detail));
                assert_eq!(fixture.commands().len(), usize::from(!will_retry));
            }
            assert_eq!(fixture.commands()[0]["message"], expected);
            assert_eq!(fixture.commands()[0]["commandId"], "routed-effect:rollback-failed");
            assert_eq!(fixture.commands()[0]["requestId"], "routed-effect");
            let (call, _) = fixture.rollback.rollback.next_call().await.unwrap();
            assert!(call.restore_files);
            assert_eq!(call.checkpoint_id, CheckpointId("checkpoint".into()));
        }
        Ok(())
    }

    #[tokio::test]
    async fn interrupted_answer_has_no_failure_report_and_report_failure_preserves_original_error()
    -> Result<()> {
        for interrupted in [true, false] {
            let fixture = Fixture::new()?;
            let effect = fixture
                .claim(EffectRequest::Respond {
                    provider_session_id: ProviderSessionId("session".into()),
                    request_id: RuntimeRequestId("request".into()),
                    decision: None,
                    feedback: Some("feedback".into()),
                    answers: Some(serde_json::from_value(json!({"next":"Ship it"}))?),
                })
                .await?;
            fixture.requests.respond.push(Err(if interrupted {
                Error::Cancelled
            } else {
                failure("original answer failure")
            }));
            fixture.dispatch.fail.store(true, Ordering::SeqCst);
            let error = fixture
                .executor
                .execute_with_options(
                    effect,
                    CancellationToken::new(),
                    EffectExecutionOptions { will_retry: false },
                )
                .await
                .unwrap_err();
            assert_eq!(fixture.commands().len(), usize::from(!interrupted));
            if interrupted {
                assert!(matches!(error, Error::Cancelled))
            } else {
                assert!(error.to_string().contains("original answer failure"));
                assert_eq!(
                    fixture.commands()[0]["type"],
                    "runtime-request.delivery.fail"
                );
            }
            let (thread, session, response, _) =
                fixture.requests.respond.next_call().await.unwrap();
            assert_eq!(thread, ThreadId("thread".into()));
            assert_eq!(session, ProviderSessionId("session".into()));
            let RuntimeResponse {
                answers, feedback, ..
            } = response;
            assert_eq!(serde_json::to_value(answers)?, json!({"next":"Ship it"}));
            assert_eq!(feedback.as_deref(), Some("feedback"));
        }
        Ok(())
    }

    #[tokio::test]
    async fn unconfirmed_subagent_stop_does_not_repair_and_failed_stop_preserves_failure()
    -> Result<()> {
        for failed in [false, true] {
            let fixture = Fixture::new()?;
            let mut request = interrupt();
            let EffectRequest::Interrupt { subagent, .. } = &mut request else {
                unreachable!()
            };
            *subagent = Some(InterruptSubagent {
                id: NodeId("subagent".into()),
                native_task_id: "native-task".into(),
            });
            let effect = fixture.claim(request).await?;
            fixture.control.interrupt_with_outcome.push(if failed {
                Err(failure("SDK stop failed"))
            } else {
                Ok(None)
            });
            let result = fixture
                .executor
                .execute(effect.clone(), CancellationToken::new())
                .await;
            assert_eq!(result.is_err(), failed);
            assert!(fixture.commands().is_empty());
            let (input, _) = fixture
                .control
                .interrupt_with_outcome
                .next_call()
                .await
                .unwrap();
            assert_eq!(input.subagent.unwrap().native_task_id, "native-task");
            assert_eq!(input.requested_at.unwrap().0, effect.created_at);
        }
        Ok(())
    }

    #[tokio::test]
    async fn restart_retries_compound_operation_and_passes_attempt_retry_policy() -> Result<()> {
        for replace in [false, true] {
            let fixture = Fixture::new()?;
            let transition = if replace {
                SessionTransition::Replace {
                    replacement_provider_session_id: ProviderSessionId("new-session".into()),
                }
            } else {
                SessionTransition::Detach
            };
            let effect = fixture
                .claim(EffectRequest::Restart {
                    provider_session_id: ProviderSessionId("old-session".into()),
                    provider_thread_id: ProviderThreadId("provider-thread".into()),
                    provider_turn_id: ProviderTurnId("turn".into()),
                    interrupted_attempt_id: RunAttemptId("attempt".into()),
                    run_id: RunId("run".into()),
                    session_transition: Some(transition),
                })
                .await?;
            for will_retry in [true, false] {
                fixture.control.interrupt_and_await_terminal.push(Ok(()));
                fixture.sessions.detach_thread.push(Ok(()));
                fixture.threads.start_with_options.push(if will_retry {
                    Err(failure("start is not active"))
                } else {
                    Ok(())
                });
                let result = fixture
                    .executor
                    .execute_with_options(
                        effect.clone(),
                        CancellationToken::new(),
                        EffectExecutionOptions { will_retry },
                    )
                    .await;
                assert_eq!(result.is_err(), will_retry);
                let (input, _) = fixture
                    .control
                    .interrupt_and_await_terminal
                    .next_call()
                    .await
                    .unwrap();
                assert_eq!(
                    input.replacement_provider_session_id,
                    replace.then(|| ProviderSessionId("new-session".into()))
                );
                let (detach,) = fixture.sessions.detach_thread.next_call().await.unwrap();
                assert_eq!(
                    detach.provider_session_id,
                    ProviderSessionId("old-session".into())
                );
                assert_eq!(detach.thread_id, ThreadId("thread".into()));
                let (thread, run, options) = fixture
                    .threads
                    .start_with_options
                    .next_call()
                    .await
                    .unwrap();
                assert_eq!(
                    (thread, run),
                    (ThreadId("thread".into()), RunId("run".into()))
                );
                assert_eq!(options.will_retry, will_retry);
            }
        }
        Ok(())
    }
}
