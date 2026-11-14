use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use t3_contracts::{base_schemas::*, orchestration_v2::*, provider_instance::ProviderDriverKind};
use t3_orchestration::{
    api::{CommitGuard, CommitRequest, Decision, EventCommitter, ProjectionWriter},
    error::{Error, Result},
    ports::commit_worker::provider_commit_batch::{
        ConsumerOwner, MAX_BYTES, PreparedCommit, ProviderCommitBatch,
    },
    provider::{ProviderError, ProviderEvent, ProviderEventKind},
    runtime::{commit::EventSink, worker::OutboxSignals},
    stores::event_store::{EventFilter, EventStore},
    testing::{ManualClock, TEST_TIME, TestDatabase, detached_event},
};
use t3_persistence::rusqlite::Transaction;
use tokio::sync::{mpsc, oneshot};

struct Writer {
    blocked: Mutex<Option<(oneshot::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
}
impl ProjectionWriter for Writer {
    fn apply(&self, tx: &Transaction<'_>, event: &OrchestrationV2StoredEvent) -> Result<()> {
        if let Some((started, release)) = self.blocked.lock().expect("test value").take() {
            started.send(()).expect("test value");
            release.recv().expect("test value");
        }
        let value = serde_json::to_value(&event.event)?;
        if value["id"] == "fail" {
            tx.execute(
                "INSERT INTO projection_projects(project_id) VALUES ('invalid')",
                [],
            )?;
        }
        if value["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("outer:"))
        {
            tx.execute("INSERT INTO batch_child(parent_id) VALUES ('missing')", [])?;
        }
        Ok(())
    }
}
fn request(id: &str) -> CommitRequest {
    CommitRequest {
        receipt: None,
        decision: Decision {
            events: vec![detached_event(id, "thread").expect("test value")],
            ..Default::default()
        },
        guard: CommitGuard::None,
    }
}
fn make(fixture: &TestDatabase, writer: Writer, enabled: bool) -> (EventSink, ProviderCommitBatch) {
    let sink = EventSink::new(
        fixture.stores.clone(),
        Arc::new(writer),
        Arc::new(ManualClock::new(TEST_TIME)),
        OutboxSignals::default(),
    );
    let coordinator = ProviderCommitBatch::new(sink.clone(), enabled);
    (sink, coordinator)
}
type Blocked = (Writer, oneshot::Receiver<()>, std::sync::mpsc::Sender<()>);
fn blocked() -> Blocked {
    let (started, receiver) = oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    (
        Writer {
            blocked: Mutex::new(Some((started, wait))),
        },
        receiver,
        release,
    )
}
fn writer() -> Writer {
    Writer {
        blocked: Mutex::new(None),
    }
}

#[tokio::test]
async fn isolated_snapshot_is_published_immediately_without_clock_advance() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    let (sink, coordinator) = make(&fixture, writer(), true);
    let mut events = sink.subscribe();
    let owner = ConsumerOwner::default();
    let reply = coordinator
        .enqueue_snapshot(&owner, 100, request("one"))
        .await?;
    let published = events.recv().await.unwrap();
    assert!(matches!(
        published,
        t3_contracts::application_event::ApplicationStoredEvent::Orchestration(_)
    ));
    assert!(reply.wait().await?.unwrap().committed);
    coordinator.shutdown().await
}

#[tokio::test]
async fn accumulated_snapshots_share_transactions_bounded_by_count_and_bytes() -> Result<()> {
    for sizes in [
        vec![100; 129],
        vec![100, MAX_BYTES + 100, 100],
        vec![600_000, 600_000, 100],
    ] {
        let fixture = TestDatabase::memory()?;
        let (writer, started, release) = blocked();
        let (sink, coordinator) = make(&fixture, writer, true);
        let mut reports = coordinator.subscribe_stats();
        let owner = ConsumerOwner::default();
        let leading = coordinator
            .enqueue_snapshot(&owner, 100, request("leading"))
            .await?;
        started.await.unwrap();
        let (filled, full) = oneshot::channel();
        let count = sizes.len();
        let producer = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move {
                let owner = ConsumerOwner::default();
                let mut replies = Vec::new();
                let mut filled = Some(filled);
                for (index, bytes) in sizes.into_iter().enumerate() {
                    replies.push(
                        coordinator
                            .enqueue_snapshot(&owner, bytes, request(&format!("event:{index}")))
                            .await?,
                    );
                    if index + 1 == count.min(64) {
                        filled.take().unwrap().send(()).unwrap();
                    }
                }
                for reply in replies {
                    reply.wait().await?;
                }
                Ok::<_, Error>(())
            })
        };
        full.await.unwrap();
        release.send(()).unwrap();
        leading.wait().await?;
        producer.await.unwrap()?;
        coordinator.drain().await?;
        let mut all = Vec::new();
        let mut jobs = 0;
        while jobs < count + 1 {
            let report = reports.recv().await.unwrap();
            jobs += report.jobs;
            assert!(report.jobs <= 64);
            assert!(report.jobs == 1 || report.bytes <= MAX_BYTES);
            assert!(report.committed);
            all.push(report.jobs);
        }
        if count == 129 {
            assert!(all.iter().any(|jobs| *jobs > 1));
        }
        let events = sink.events_by_command(CommandId("missing".into())).await?;
        assert!(events.is_empty());
        fixture
            .stores
            .read(move |db| {
                let events = EventStore::new().read(db, &EventFilter::default())?;
                assert_eq!(events.len(), count + 1);
                for (index, event) in events.iter().skip(1).enumerate() {
                    assert_eq!(
                        serde_json::to_value(&event.event)?["id"],
                        format!("event:{index}")
                    );
                }
                Ok(())
            })
            .await?;
        coordinator.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn sources_share_backlogged_commit_and_cancelled_source_writes_nothing() -> Result<()> {
    for cancel in [false, true] {
        let fixture = TestDatabase::memory()?;
        let (writer, started, release) = blocked();
        let (_, coordinator) = make(&fixture, writer, true);
        let mut reports = coordinator.subscribe_stats();
        let leading_owner = ConsumerOwner::default();
        let leading = coordinator
            .enqueue_snapshot(&leading_owner, 100, request("leading"))
            .await?;
        started.await.unwrap();
        let owners = (0..10)
            .map(|_| ConsumerOwner::default())
            .collect::<Vec<_>>();
        let mut replies = Vec::new();
        for (index, owner) in owners.iter().enumerate() {
            replies.push(
                coordinator
                    .enqueue_snapshot(owner, 100, request(&format!("event:{index}")))
                    .await?,
            );
        }
        if cancel {
            owners[0].cancel();
        }
        release.send(()).unwrap();
        leading.wait().await?;
        for (index, reply) in replies.into_iter().enumerate() {
            assert_eq!(reply.wait().await.is_err(), cancel && index == 0);
        }
        assert_eq!(reports.recv().await.unwrap().jobs, 1);
        assert_eq!(
            reports.recv().await.unwrap().jobs,
            if cancel { 9 } else { 10 }
        );
        coordinator.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn failing_snapshot_uses_savepoint_and_cancels_only_its_source() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    fixture
        .stores
        .transaction(|tx| {
            tx.execute_batch("CREATE TABLE batch_markers(value INTEGER)")?;
            Ok(())
        })
        .await?;
    let (writer, started, release) = blocked();
    let (_, coordinator) = make(&fixture, writer, true);
    let leader = ConsumerOwner::default();
    let leading = coordinator
        .enqueue_snapshot(&leader, 10, request("leading"))
        .await?;
    started.await.unwrap();
    let valid = ConsumerOwner::default();
    let invalid = ConsumerOwner::default();
    let good = coordinator
        .enqueue_snapshot(&valid, 10, request("good"))
        .await?;
    let mut partial = request("partial");
    partial
        .decision
        .events
        .push(detached_event("fail", "thread")?);
    let bad = coordinator
        .enqueue_prepared_snapshot(
            &invalid,
            10,
            PreparedCommit {
                request: partial,
                options: Default::default(),
                prepare: Some(Box::new(|tx, input| {
                    tx.execute("INSERT INTO batch_markers VALUES (99)", [])?;
                    Ok(input)
                })),
            },
        )
        .await?;
    let skipped = coordinator
        .enqueue_snapshot(&invalid, 10, request("skipped"))
        .await?;
    let good_after = coordinator
        .enqueue_prepared_snapshot(
            &valid,
            10,
            PreparedCommit {
                request: request("good-after"),
                options: Default::default(),
                prepare: Some(Box::new(|tx, input| {
                    let preceding: i64 =
                        tx.query_row("SELECT COUNT(*) FROM orchestration_events", [], |row| {
                            row.get(0)
                        })?;
                    assert_eq!(
                        preceding, 2,
                        "preparation sees earlier jobs on the writer transaction"
                    );
                    assert_eq!(
                        tx.query_row("SELECT COUNT(*) FROM batch_markers", [], |row| row
                            .get::<_, i64>(0))?,
                        0,
                        "failed preparation rolled back its writes"
                    );
                    tx.execute("INSERT INTO batch_markers VALUES (?1)", [preceding])?;
                    Ok(input)
                })),
            },
        )
        .await?;
    release.send(()).unwrap();
    leading.wait().await?;
    good.wait().await?;
    assert!(
        bad.wait()
            .await
            .unwrap_err()
            .to_string()
            .contains("NOT NULL")
    );
    assert!(skipped.wait().await.is_err());
    good_after.wait().await?;
    assert!(!valid.is_cancelled());
    assert!(invalid.is_cancelled());
    fixture
        .stores
        .read(|db| {
            let ids = EventStore::new()
                .read(db, &EventFilter::default())?
                .into_iter()
                .map(|event| {
                    Ok(serde_json::to_value(&event.event)?["id"]
                        .as_str()
                        .unwrap()
                        .to_owned())
                })
                .collect::<Result<Vec<_>>>()?;
            assert_eq!(ids, ["leading", "good", "good-after"]);
            assert_eq!(
                db.query_row("SELECT value FROM batch_markers", [], |row| row
                    .get::<_, i64>(0))?,
                2
            );
            Ok(())
        })
        .await?;
    coordinator
        .barrier(async { Err::<(), _>(Error::Unsupported("original error")) })
        .await
        .unwrap_err();
    assert_eq!(
        coordinator.barrier(async { Ok("recovered") }).await?,
        "recovered"
    );
    coordinator.shutdown().await
}

#[tokio::test]
async fn outer_transaction_failure_fails_every_source_and_publishes_nothing_from_batch()
-> Result<()> {
    let fixture = TestDatabase::memory()?;
    fixture.stores.transaction(|tx| {
        tx.execute_batch("CREATE TABLE batch_parent(id TEXT PRIMARY KEY); CREATE TABLE batch_child(parent_id TEXT REFERENCES batch_parent(id) DEFERRABLE INITIALLY DEFERRED)")?;
        Ok(())
    }).await?;
    let (writer, started, release) = blocked();
    let (sink, coordinator) = make(&fixture, writer, true);
    let mut events = sink.subscribe();
    let owner = ConsumerOwner::default();
    let leading = coordinator
        .enqueue_snapshot(&owner, 10, request("leading"))
        .await?;
    started.await.unwrap();
    let mut replies = Vec::new();
    let owners = [ConsumerOwner::default(), ConsumerOwner::default()];
    for (index, owner) in owners.iter().enumerate() {
        replies.push(
            coordinator
                .enqueue_snapshot(owner, 10, request(&format!("outer:{index}")))
                .await?,
        );
    }
    release.send(()).unwrap();
    leading.wait().await?;
    for reply in replies {
        assert!(reply.wait().await.is_err());
    }
    assert_eq!(sink.latest_sequence(None).await?, 1);
    events.recv().await.unwrap();
    assert!(events.try_recv().is_err());
    coordinator.shutdown().await
}

fn event(index: usize) -> ProviderEvent {
    let message: OrchestrationV2ConversationMessage = serde_json::from_value(serde_json::json!({
        "id":"single-message","threadId":"thread","runId":null,"nodeId":null,"createdBy":"agent","creationSource":"provider","role":"assistant","text":format!("delta:{index}"),"attachments":[],"streaming":true,"createdAt":TEST_TIME,"updatedAt":TEST_TIME,
    })).expect("test value");
    ProviderEvent {
        driver: ProviderDriverKind("codex".into()),
        kind: ProviderEventKind::Message(Box::new(message)),
    }
}
fn control() -> ProviderEvent {
    ProviderEvent {
        driver: ProviderDriverKind("codex".into()),
        kind: ProviderEventKind::BackgroundWork {
            provider_turn_id: ProviderTurnId("turn".into()),
            pending: false,
        },
    }
}
fn event_request(event: ProviderEvent) -> Result<Option<CommitRequest>> {
    match event.kind {
        ProviderEventKind::Message(message) => Ok(Some(request(&message.text))),
        _ => Ok(None),
    }
}

#[tokio::test]
async fn source_control_flushes_prefix_and_stops_before_following_snapshots() -> Result<()> {
    for enabled in [false, true] {
        let fixture = TestDatabase::memory()?;
        let (sink, coordinator) = make(&fixture, writer(), enabled);
        let (sender, receiver) = mpsc::channel(8);
        for input in [event(1), event(2), control(), event(3)] {
            sender.send(Ok(input)).await.unwrap();
        }
        drop(sender);
        let reached_control = Arc::new(AtomicBool::new(false));
        let seen = reached_control.clone();
        coordinator
            .consume(
                receiver,
                move |event| {
                    seen.store(
                        matches!(event.kind, ProviderEventKind::BackgroundWork { .. }),
                        Ordering::Relaxed,
                    );
                    async { event_request(event) }
                },
                || async { Ok(reached_control.load(Ordering::Relaxed)) },
                || async { true },
            )
            .await?;
        fixture
            .stores
            .read(|db| {
                let events = EventStore::new().read(db, &EventFilter::default())?;
                assert_eq!(events.len(), 2);
                assert_eq!(serde_json::to_value(&events[0].event)?["id"], "delta:1");
                assert_eq!(serde_json::to_value(&events[1].event)?["id"], "delta:2");
                Ok(())
            })
            .await?;
        assert_eq!(sink.latest_sequence(None).await?, 2);
        coordinator.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn source_failure_commits_prefix_and_returns_original_error() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    let (sink, coordinator) = make(&fixture, writer(), true);
    let (sender, receiver) = mpsc::channel(8);
    sender.send(Ok(event(1))).await.unwrap();
    sender.send(Ok(event(2))).await.unwrap();
    sender
        .send(Err(ProviderError::Operation {
            operation: t3_orchestration::provider::ProviderOperation::Stream,
            detail: "provider failed".into(),
        }))
        .await
        .unwrap();
    drop(sender);
    let error = coordinator
        .consume(
            receiver,
            |event| async { event_request(event) },
            || async { Ok(false) },
            || async { true },
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("provider failed"));
    assert_eq!(sink.latest_sequence(None).await?, 2);
    coordinator.shutdown().await
}

#[tokio::test]
async fn eof_flushes_partial_batch_and_snapshot_failure_does_not_wait_for_eof() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    let (sink, coordinator) = make(&fixture, writer(), true);
    let (sender, receiver) = mpsc::channel(8);
    sender.send(Ok(event(1))).await.unwrap();
    drop(sender);
    coordinator
        .consume(
            receiver,
            |event| async { event_request(event) },
            || async { Ok(false) },
            || async { true },
        )
        .await?;
    assert_eq!(sink.latest_sequence(None).await?, 1);
    let (sender, receiver) = mpsc::channel(8);
    sender.send(Ok(event(2))).await.unwrap();
    let error = coordinator
        .consume(
            receiver,
            |_| async { Ok(Some(request("fail"))) },
            || async { Ok(false) },
            || async { true },
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("NOT NULL"));
    drop(sender);
    coordinator.shutdown().await
}

#[tokio::test]
async fn control_background_check_does_not_hold_worker_or_deadlock_thread_lock_holder() -> Result<()>
{
    let fixture = TestDatabase::memory()?;
    let (_, coordinator) = make(&fixture, writer(), true);
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    let held = lock.clone().lock_owned().await;
    let (waiting, ready) = oneshot::channel();
    let waiting = Arc::new(Mutex::new(Some(waiting)));
    let (sender, receiver) = mpsc::channel(2);
    sender.send(Ok(control())).await.unwrap();
    drop(sender);
    let consumer = {
        let coordinator = coordinator.clone();
        let lock = lock.clone();
        tokio::spawn(async move {
            coordinator
                .consume(
                    receiver,
                    move |_| {
                        let lock = lock.clone();
                        let waiting = waiting.clone();
                        async move {
                            waiting.lock().unwrap().take().unwrap().send(()).unwrap();
                            let _held = lock.lock().await;
                            Ok(None)
                        }
                    },
                    || async { Ok(true) },
                    || async { true },
                )
                .await
        })
    };
    ready.await.unwrap();
    assert_eq!(
        coordinator
            .barrier(async { Ok("command holding thread lock can commit") })
            .await?,
        "command holding thread lock can commit"
    );
    drop(held);
    consumer.await.unwrap()?;
    coordinator.shutdown().await
}

#[tokio::test]
async fn shutdown_settles_backlogged_calls_and_rejects_subsequent_barriers() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    let (writer, started, release) = blocked();
    let (_, coordinator) = make(&fixture, writer, true);
    let owner = ConsumerOwner::default();
    let leading = coordinator
        .enqueue_snapshot(&owner, 10, request("leading"))
        .await?;
    started.await.unwrap();
    let queued = coordinator
        .enqueue_snapshot(&owner, 10, request("queued"))
        .await?;
    coordinator.cancellation_token().cancel();
    let shutdown = {
        let coordinator = coordinator.clone();
        tokio::spawn(async move { coordinator.shutdown().await })
    };
    release.send(()).unwrap();
    leading.wait().await?;
    shutdown.await.unwrap()?;
    assert!(queued.wait().await.is_err());
    assert!(
        coordinator
            .barrier(async {
                panic!("must not run after shutdown");
                #[allow(unreachable_code)]
                Ok(())
            })
            .await
            .is_err()
    );
    Ok(())
}

struct Immediate;

#[async_trait::async_trait]
impl t3_orchestration::api::EffectExecutor for Immediate {
    async fn execute(
        &self,
        _: t3_orchestration::effects::EffectRow,
        _: tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        Ok(())
    }
    async fn failed(
        &self,
        _: t3_orchestration::effects::EffectRow,
        _: &Error,
        _: bool,
    ) -> Result<()> {
        panic!("unexpected failure")
    }
}

#[tokio::test]
async fn worker_drain_includes_accepted_batch_jobs_and_the_effects_they_enqueue() -> Result<()> {
    use t3_orchestration::{
        api::EffectWorker as _,
        effects::*,
        runtime::worker::{EffectWorker, WorkerOptions},
        stores::outbox,
    };
    let fixture = TestDatabase::temporary()?;
    let (writer, started, release) = blocked();
    let (sink, coordinator) = make(&fixture, writer, true);
    let worker = EffectWorker::new(
        fixture.stores.clone(),
        Arc::new(Immediate),
        Arc::new(ManualClock::new(TEST_TIME)),
        sink.signals(),
        WorkerOptions::default(),
    );
    let owner = ConsumerOwner::default();
    let leading = coordinator
        .enqueue_snapshot(&owner, 10, request("leading"))
        .await?;
    started.await.unwrap();
    let mut input = request("queued");
    input.decision.effects.push(PendingEffect {
        id: "queued-effect".into(),
        command_id: CommandId("command".into()),
        thread_id: ThreadId("thread".into()),
        request: EffectRequest::CleanupTerminals,
        available_at: None,
    });
    let queued = coordinator.enqueue_snapshot(&owner, 10, input).await?;
    assert_eq!(sink.signals().pending_batch_jobs(), 2);
    let drained = {
        let worker = worker.clone();
        tokio::spawn(async move { worker.drain(100).await })
    };
    release.send(()).unwrap();
    leading.wait().await?;
    queued.wait().await?;
    assert_eq!(drained.await.unwrap()?, 1);
    assert_eq!(sink.signals().pending_batch_jobs(), 0);
    fixture
        .stores
        .read(|db| {
            assert_eq!(
                outbox::get(db, "queued-effect")?.unwrap().status,
                EffectStatus::Succeeded
            );
            Ok(())
        })
        .await?;
    worker.shutdown().await?;
    coordinator.shutdown().await
}

#[tokio::test]
async fn ordinary_sink_commands_cannot_overtake_snapshots_waiting_in_the_provider_queue()
-> Result<()> {
    let fixture = TestDatabase::memory()?;
    let (writer, started, release) = blocked();
    let (sink, coordinator) = make(&fixture, writer, true);
    let mut events = sink.subscribe();
    let owner = ConsumerOwner::default();
    let leading = coordinator
        .enqueue_snapshot(&owner, 10, request("leading"))
        .await?;
    started.await.unwrap();
    let queued = coordinator
        .enqueue_snapshot(&owner, 10, request("queued"))
        .await?;
    let command = {
        let sink = sink.clone();
        tokio::spawn(async move { sink.commit(request("command")).await })
    };
    release.send(()).unwrap();
    leading.wait().await?;
    queued.wait().await?;
    command.await.unwrap()?;
    let mut ids = Vec::new();
    for _ in 0..3 {
        let t3_contracts::application_event::ApplicationStoredEvent::Orchestration(event) =
            events.recv().await.unwrap()
        else {
            panic!("thread event")
        };
        ids.push(
            serde_json::to_value(&event.event)?["id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    assert_eq!(ids, ["leading", "queued", "command"]);
    coordinator.shutdown().await
}
