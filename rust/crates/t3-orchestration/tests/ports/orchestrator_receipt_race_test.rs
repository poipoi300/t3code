#![allow(clippy::unwrap_used)]

use super::command_policy_test::command;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use t3_contracts::provider_instance::ProviderInstanceId;
use t3_orchestration::{
    api::*,
    decision::engine::PureDecisionEngine,
    error::{Error, Result},
    projections::store::ProjectionStore,
    provider::ProviderAdapter,
    runtime::{commit::EventSink, dispatch::*, worker::OutboxSignals},
    stores::receipts::{AggregateId, CommandReceipt, ReceiptStatus},
    testing::{ManualClock, SequenceIds, TEST_TIME, TestDatabase},
};
use tokio::sync::Barrier;

struct NoProviders;

pub(super) struct DispatchMetrics(
    pub tokio::sync::mpsc::UnboundedSender<(String, DispatchOutcome)>,
);
impl DispatchObserver for DispatchMetrics {
    fn record(&self, command_type: &str, outcome: DispatchOutcome, _duration: std::time::Duration) {
        let _ = self.0.send((command_type.to_owned(), outcome));
    }
}
#[async_trait]
impl ProviderRegistry for NoProviders {
    async fn adapter(&self, _instance: ProviderInstanceId) -> Result<Arc<dyn ProviderAdapter>> {
        Err(Error::Unsupported("test does not start providers"))
    }
}
#[derive(Default)]
pub(super) struct Recovery(AtomicUsize);
#[async_trait]
impl RecoveryService for Recovery {
    async fn recover(&self) -> Result<RecoverySummary> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(RecoverySummary::default())
    }
    async fn reconcile(&self, _thread: Option<ThreadId>) -> Result<RecoverySummary> {
        Ok(RecoverySummary::default())
    }
    async fn prepare_for_shutdown(&self) -> Result<()> {
        Ok(())
    }
}
struct RacingInputs {
    base: ProjectionCommandInputs,
    barrier: Arc<Barrier>,
}
#[async_trait]
impl CommandInputs for RacingInputs {
    async fn lock_keys(&self, command: &OrchestrationV2ServerCommand) -> Result<Vec<ThreadId>> {
        self.base.lock_keys(command).await
    }
    async fn prepare(&self, command: &OrchestrationV2ServerCommand) -> Result<PreparedCommand> {
        let value = serde_json::to_value(command)?;
        if value["commandId"]
            .as_str()
            .is_some_and(|id| id.starts_with("race:"))
        {
            self.barrier.wait().await;
        }
        self.base.prepare(command).await
    }
}

pub(super) struct Harness {
    pub database: TestDatabase,
    pub reader: Arc<ProjectionStore>,
    pub sink: Arc<EventSink>,
    pub clock: Arc<ManualClock>,
    pub dispatcher: Dispatcher,
}
impl Harness {
    pub fn new(
        ceiling: Option<
            t3_orchestration::ports::decisions::dispatch_mode_limit::RuntimeModeCeiling,
        >,
        racing: bool,
    ) -> Self {
        Self::with_providers(ceiling, racing, Arc::new(NoProviders))
    }
    pub fn with_providers(
        ceiling: Option<
            t3_orchestration::ports::decisions::dispatch_mode_limit::RuntimeModeCeiling,
        >,
        racing: bool,
        providers: Arc<dyn ProviderRegistry>,
    ) -> Self {
        let database = TestDatabase::memory().unwrap();
        let reader = Arc::new(ProjectionStore::new(database.stores.clone()));
        let clock = Arc::new(ManualClock::new(TEST_TIME));
        let sink = Arc::new(EventSink::new(
            database.stores.clone(),
            reader.clone(),
            clock.clone(),
            OutboxSignals::default(),
        ));
        let base = ProjectionCommandInputs {
            reader: reader.clone(),
            providers,
            clock: clock.clone(),
            ids: Arc::new(SequenceIds::new("decision-test")),
        };
        let inputs: Arc<dyn CommandInputs> = if racing {
            Arc::new(RacingInputs {
                base,
                barrier: Arc::new(Barrier::new(2)),
            })
        } else {
            Arc::new(base)
        };
        let dispatcher = Dispatcher::with_command_lanes(
            DispatchServices {
                database: database.stores.clone(),
                inputs,
                engine: Arc::new(PureDecisionEngine),
                committer: sink.clone(),
                recovery: Arc::new(Recovery::default()),
                clock: clock.clone(),
            },
            ceiling,
            Arc::new(
                t3_orchestration::ports::decisions::thread_command_executor::KeyedLanes::default(),
            ),
        );
        Self {
            database,
            reader,
            sink,
            clock,
            dispatcher,
        }
    }
    pub async fn create(&self, id: &str) -> Result<CommandReceipt> {
        self.dispatch(json!({"type":"thread.create","commandId":format!("create:{id}"),"threadId":id,"projectId":"project","title":"Before","modelSelection":{"instanceId":"codex","model":"a"},"runtimeMode":"approval-required","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"})).await
    }
    pub async fn dispatch(&self, value: Value) -> Result<CommandReceipt> {
        self.dispatcher.dispatch(command(value)).await
    }
    pub async fn count_events(&self) -> i64 {
        self.database
            .stores
            .read(|db| {
                Ok(
                    db.query_row("SELECT count(*) FROM orchestration_events", [], |row| {
                        row.get(0)
                    })?,
                )
            })
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn command_id_races_across_threads_accept_only_one_and_replay_the_winner() {
    let harness = Harness::new(None, true);
    harness.create("a").await.unwrap();
    harness.create("b").await.unwrap();
    let pin = |thread: &str| json!({"type":"thread.pin","commandId":"race:pin","threadId":thread});
    let (a, b) = tokio::join!(harness.dispatch(pin("a")), harness.dispatch(pin("b")));
    assert_ne!(a.is_ok(), b.is_ok());
    let (winner, receipt) = if let Ok(receipt) = a {
        ("a", receipt)
    } else {
        ("b", b.unwrap())
    };
    let loser = if winner == "a" { "b" } else { "a" };
    assert!(
        harness
            .reader
            .thread(ThreadId(loser.into()))
            .await
            .unwrap()
            .unwrap()
            .pinned_at
            .flatten()
            .is_none()
    );
    assert!(
        harness
            .reader
            .thread(ThreadId(winner.into()))
            .await
            .unwrap()
            .unwrap()
            .pinned_at
            .flatten()
            .is_some()
    );
    let before = harness.count_events().await;
    assert_eq!(harness.dispatch(pin(winner)).await.unwrap(), receipt);
    assert_eq!(harness.count_events().await, before);
    assert!(
        !harness
            .sink
            .events_by_command(CommandId("race:pin".into()))
            .await
            .unwrap()
            .is_empty()
    );

    let close = |thread: &str| json!({"type":"thread.guest-share.close","commandId":"race:noop","threadId":thread});
    let (a, b) = tokio::join!(harness.dispatch(close("a")), harness.dispatch(close("b")));
    assert_ne!(a.is_ok(), b.is_ok());
    let (winner, receipt) = if let Ok(receipt) = a {
        ("a", receipt)
    } else {
        ("b", b.unwrap())
    };
    assert_eq!(harness.dispatch(close(winner)).await.unwrap(), receipt);
    assert!(
        harness
            .sink
            .events_by_command(CommandId("race:noop".into()))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn project_receipts_never_replay_as_thread_receipts_and_pruned_logs_still_dedupe() {
    let harness = Harness::new(None, false);
    harness.create("thread").await.unwrap();
    harness
        .database
        .stores
        .transaction(|tx| {
            t3_orchestration::stores::receipts::insert_if_absent(
                tx,
                &CommandReceipt {
                    command_id: CommandId("project-command".into()),
                    aggregate: AggregateId::Project(ProjectId("project".into())),
                    command_type: "project.create".into(),
                    accepted_at: TEST_TIME.into(),
                    result_sequence: 0,
                    status: ReceiptStatus::Accepted,
                    error: None,
                },
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        harness
            .dispatch(
                json!({"type":"thread.pin","commandId":"project-command","threadId":"thread"})
            )
            .await
            .is_err()
    );
    let pin = json!({"type":"thread.pin","commandId":"pin","threadId":"thread"});
    let receipt = harness.dispatch(pin.clone()).await.unwrap();
    harness
        .database
        .stores
        .transaction(|tx| {
            tx.execute(
                "DELETE FROM orchestration_events WHERE command_id='pin'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let before = harness.count_events().await;
    assert_eq!(harness.dispatch(pin).await.unwrap(), receipt);
    assert_eq!(harness.count_events().await, before);
}

#[tokio::test]
async fn weak_lanes_keep_waiters_alive_and_acquire_cross_thread_keys_in_sorted_order() {
    let lanes = Arc::new(
        t3_orchestration::ports::decisions::thread_command_executor::KeyedLanes::default(),
    );
    let lane = lanes.lane("thread");
    let held = lane.clone().lock_owned().await;
    let waiter = lanes.lane("thread");
    drop(lane);
    lanes.prune();
    assert!(Arc::ptr_eq(&waiter, &lanes.lane("thread")));
    assert_eq!(lanes.live_lanes(), 1);
    drop(held);
    drop(waiter);
    lanes.prune();
    assert_eq!(lanes.live_lanes(), 0);
    let one = lanes.clone();
    let two = lanes.clone();
    let first = tokio::spawn(async move {
        one.acquire(["b".into(), "a".into(), "b".into()])
            .await
            .len()
    });
    let second = tokio::spawn(async move { two.acquire(["a".into(), "b".into()]).await.len() });
    assert_eq!(first.await.unwrap(), 2);
    assert_eq!(second.await.unwrap(), 2);
}

struct GatedInputs {
    base: ProjectionCommandInputs,
    entered: tokio::sync::mpsc::UnboundedSender<()>,
    release: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

#[async_trait]
impl CommandInputs for GatedInputs {
    async fn lock_keys(&self, command: &OrchestrationV2ServerCommand) -> Result<Vec<ThreadId>> {
        self.base.lock_keys(command).await
    }
    async fn prepare(&self, command: &OrchestrationV2ServerCommand) -> Result<PreparedCommand> {
        let release = self.release.lock().unwrap().take();
        if let Some(release) = release {
            self.entered.send(()).unwrap();
            release.await.unwrap();
        }
        self.base.prepare(command).await
    }
}

#[tokio::test]
async fn dropping_the_caller_keeps_the_command_alive_through_durable_commit() {
    let harness = Harness::new(None, false);
    harness.create("thread").await.unwrap();
    let (entered, mut arrivals) = tokio::sync::mpsc::unbounded_channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let dispatcher = Dispatcher::new(
        DispatchServices {
            database: harness.database.stores.clone(),
            inputs: Arc::new(GatedInputs {
                base: ProjectionCommandInputs {
                    reader: harness.reader.clone(),
                    providers: Arc::new(NoProviders),
                    clock: harness.clock.clone(),
                    ids: Arc::new(SequenceIds::new("drop-caller")),
                },
                entered,
                release: std::sync::Mutex::new(Some(released)),
            }),
            engine: Arc::new(PureDecisionEngine),
            committer: harness.sink.clone(),
            recovery: Arc::new(Recovery::default()),
            clock: harness.clock.clone(),
        },
        None,
    );
    let (metrics, mut observations) = tokio::sync::mpsc::unbounded_channel();
    dispatcher
        .set_dispatch_observer(Arc::new(DispatchMetrics(metrics)))
        .unwrap();
    let mut committed = harness.sink.subscribe();
    let caller = tokio::spawn(async move {
        dispatcher
            .dispatch(command(
                json!({"type":"thread.pin","commandId":"drop-caller","threadId":"thread"}),
            ))
            .await
    });
    arrivals.recv().await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();
    committed.recv().await.unwrap();
    assert_eq!(
        observations.recv().await.unwrap(),
        ("thread.pin".into(), DispatchOutcome::Success)
    );
    let receipt = harness
        .database
        .stores
        .read(|db| t3_orchestration::stores::receipts::get(db, &CommandId("drop-caller".into())))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Accepted);
    assert!(
        harness
            .reader
            .thread(ThreadId("thread".into()))
            .await
            .unwrap()
            .unwrap()
            .pinned_at
            .flatten()
            .is_some()
    );
}
