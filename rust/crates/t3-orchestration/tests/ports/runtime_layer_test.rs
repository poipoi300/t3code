use super::provider_runtime_recovery_service_test::{projection, seed, service, sink};
use async_trait::async_trait;
use std::sync::Arc;
use t3_contracts::orchestration_v2::OrchestrationV2ServerCommand;
use t3_orchestration::{
    api::*,
    effects::*,
    error::{Error, Result},
    runtime::sessions::*,
    stores::{StoreDatabase, receipts::*},
    testing::*,
};
use tokio::sync::mpsc;

#[tokio::test]
async fn managed_stream_completes_a_turn_and_recovers_its_interrupted_successor_after_reopen()
-> Result<()> {
    use super::provider_session_manager_test::{fake, turn_input};
    use serde_json::json;
    use t3_contracts::{orchestration_v2::*, provider_instance::ProviderDriverKind};
    use t3_orchestration::{
        ports::{
            checkpoints_forks::{
                checkpoint_capture_service::CheckpointCaptureService,
                checkpoint_service::{CheckpointService, WorkspaceLocks},
            },
            decisions::thread_command_executor::KeyedLanes,
            runs_ingestion::{
                provider_event_ingestor::ProviderEventIngestor,
                provider_turn_start_service_testkit::NoRepositoryCheckpoints,
                run_execution_service::*,
            },
        },
        projections::store::ProjectionStore,
        provider::*,
        runtime::{ingestion::ProviderCommitter, worker::OutboxSignals},
    };
    use tokio_util::sync::CancellationToken;

    let directory = tempfile::tempdir()?;
    let path = directory.path().join("statev2.sqlite");
    {
        let stores = StoreDatabase(t3_persistence::Database::open(&path)?);
        let events = Arc::new(sink(stores.clone()));
        let reader = Arc::new(ProjectionStore::new(stores.clone()));
        let clock = Arc::new(ManualClock::new(TEST_TIME));
        let ids = Arc::new(SequenceIds::new("composed-session"));
        let (adapter, mut calls, input) = fake()?;
        let ingestor = Arc::new(ProviderEventIngestor::new(
            reader.clone(),
            Arc::new(ProviderCommitter((*events).clone())),
            clock.clone(),
            ids.clone(),
            Arc::new(KeyedLanes::default()),
        ));
        let sessions = ProviderSessionManager::with_options(
            Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
            Some(ingestor.clone()),
            SessionManagerOptions::default(),
            Some(SessionServices {
                persistence: Some(SessionPersistence {
                    reader: reader.clone(),
                    sink: events.clone(),
                    clock: clock.clone(),
                    ids: ids.clone(),
                }),
                credentials: None,
            }),
        );
        let recovery = Arc::new(ProviderRuntimeRecoveryService::new(
            stores.clone(),
            reader.clone(),
            events.clone(),
            Arc::new(sessions.clone()),
            clock.clone(),
            ids.clone(),
            OutboxSignals::default(),
        ));
        assert_eq!(recovery.recover().await?, RecoverySummary::default());
        let mut projection = projection()?;
        projection.thread.lineage.side_chat = Some(Some(true));
        seed(&events, &projection).await?;
        let runtime = sessions
            .attach(input.model_selection.instance_id.clone(), input.clone())
            .await?;
        assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
        assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
        let provider_thread = runtime
            .ensure_thread(EnsureThread {
                thread_id: input.thread_id.clone(),
                title: None,
                provider_session_id: Some(input.provider_session_id.clone()),
                force_new_native_thread: false,
                existing_provider_thread: Some(projection.provider_threads[0].clone()),
                model_selection: input.model_selection.clone(),
                runtime_policy: input.runtime_policy.clone(),
            })
            .await?;
        assert!(matches!(calls.recv().await, Some(FakeCall::Ensure(_))));
        let mut persisted = events.subscribe();
        adapter
            .runtime
            .emit(ProviderEvent {
                driver: ProviderDriverKind("codex".into()),
                kind: ProviderEventKind::Thread {
                    provider_thread: Box::new(provider_thread.clone()),
                    pending_background_work: None,
                    session_scoped: Some(true),
                    retained_native_turn_ids: None,
                },
            })
            .await?;
        loop {
            if let t3_contracts::application_event::ApplicationStoredEvent::Orchestration(stored) =
                persisted.recv().await.unwrap()
                && matches!(
                    stored.event,
                    OrchestrationV2DomainEvent::ProviderThreadUpdated(_)
                )
            {
                break;
            }
        }
        let execution = Arc::new(RunExecutionService::new(ingestor.clone(), None));
        let state = RootRunState {
            run: projection.runs[0].clone(),
            root_node: projection.nodes[0].clone(),
            provider_thread,
            attempt: projection.attempts[0].clone(),
            checkpoint_scope: serde_json::from_value(json!({
                "id":"scope:test","threadId":"thread:test","runId":"run:test","nodeId":"node:test",
                "parentScopeId":null,"providerThreadId":"provider-thread:test","kind":"root_run",
                "ordinalWithinParent":0,"advancesAppRunCount":true,"cwd":"/repo","createdAt":TEST_TIME,
            }))?,
        };
        events.commit(CommitRequest { receipt: None, guard: CommitGuard::None, decision: Decision {
            events: vec![serde_json::from_value(json!({"id":"scope:created","threadId":"thread:test","occurredAt":TEST_TIME,"type":"checkpoint-scope.created","payload":t3_contracts::stored::to_value(&state.checkpoint_scope)?}))?],
            ..Default::default()
        }}).await?;
        let mut turn = turn_input(&input, 1)?;
        turn.app_thread = projection.thread.clone();
        turn.run_id = RunId("run:test".into());
        turn.attempt_id = RunAttemptId("attempt:test".into());
        turn.root_node_id = NodeId("node:test".into());
        let start = |state: RootRunState, turn: TurnInput| StartRootRun {
            state,
            turn,
            session: runtime.clone(),
            inherited_background_items: Default::default(),
            related_thread_ids: vec![],
            related_provider_thread_ids: vec![],
        };
        let handle = execution
            .start_root_run(start(state.clone(), turn.clone()), CancellationToken::new())
            .await?;
        assert!(matches!(calls.recv().await, Some(FakeCall::Start(_))));
        adapter.runtime.emit(ProviderEvent {
            driver: ProviderDriverKind("codex".into()), kind: ProviderEventKind::Turn {
                provider_turn: Box::new(serde_json::from_value(json!({"id":"provider-turn:test","providerThreadId":"provider-thread:test","nodeId":"node:test","runAttemptId":"attempt:test","nativeTurnRef":null,"ordinal":1,"status":"running","startedAt":TEST_TIME,"completedAt":null}))?), thread_id: Some(input.thread_id.clone()),
            },
        }).await?;
        adapter.runtime.emit(ProviderEvent {
            driver: ProviderDriverKind("codex".into()), kind: ProviderEventKind::Message(Box::new(serde_json::from_value(json!({"createdBy":"agent","creationSource":"server","id":"message:reply","threadId":"thread:test","runId":"run:test","nodeId":"node:test","role":"assistant","text":"Turn completed","attachments":[],"streaming":false,"createdAt":TEST_TIME,"updatedAt":TEST_TIME}))?)),
        }).await?;
        adapter
            .runtime
            .emit(ProviderEvent {
                driver: ProviderDriverKind("codex".into()),
                kind: ProviderEventKind::Terminal {
                    provider_thread_id: ProviderThreadId("provider-thread:test".into()),
                    provider_turn_id: ProviderTurnId("provider-turn:test".into()),
                    run_ordinal: 1,
                    disposition: ThreadDisposition::Reusable,
                    outcome: TerminalOutcome::Completed { reason: None },
                },
            })
            .await?;
        handle.settled().await?;
        stores.read(|db| {
            assert_eq!(db.query_row("SELECT COUNT(*) FROM orchestration_events WHERE event_type='message.updated' AND json_extract(payload_json,'$.id')='message:reply'", [], |row| row.get::<_,i64>(0))?, 1);
            Ok(())
        }).await?;
        assert_eq!(
            reader
                .run(input.thread_id.clone(), RunId("run:test".into()))
                .await?
                .unwrap()
                .status,
            OrchestrationV2RunStatus::Waiting
        );
        CheckpointCaptureService {
            checkpoints: Arc::new(CheckpointService::new(
                Arc::new(NoRepositoryCheckpoints),
                Arc::new(NoRepositoryCheckpoints),
                WorkspaceLocks::workspace_leases(),
            )),
            projections: reader.clone(),
            events: events.clone(),
            clock: clock.clone(),
            ids: ids.clone(),
        }
        .execute(
            input.thread_id.clone(),
            RunId("run:test".into()),
            CheckpointScopeId("scope:test".into()),
            CancellationToken::new(),
        )
        .await?;
        assert_eq!(
            reader
                .run(input.thread_id.clone(), RunId("run:test".into()))
                .await?
                .unwrap()
                .status,
            OrchestrationV2RunStatus::Completed
        );

        let (stopped, mut order) = mpsc::unbounded_channel();
        let layer = RuntimeLayer::new(
            Arc::new(FakeCommandDispatcher::default()),
            recovery,
            Arc::new(Worker(stopped, None)),
            Arc::new(sessions.clone()),
        )
        .with_run_execution(execution.clone());
        layer.shutdown_worker().await?;
        assert_eq!(order.recv().await, Some("worker shutdown"));
        assert!(
            sessions
                .get(input.provider_session_id.clone())
                .await?
                .is_some()
        );
        assert!(calls.try_recv().is_err());
        assert!(layer.dispatch(serde_json::from_value(json!({"type":"thread.archive","commandId":"after:worker-stop","threadId":"thread:test"}))?).await.is_err());
        layer.shutdown_worker().await?;
        assert!(order.try_recv().is_err());
        layer.shutdown_sessions().await?;
        assert!(matches!(calls.recv().await, Some(FakeCall::Close)));
        assert!(!sessions.is_live(&input.provider_session_id));
        layer.shutdown().await?;
        assert!(order.try_recv().is_err());

        let (adapter, mut calls, _) = fake()?;
        let sessions = ProviderSessionManager::with_options(
            Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
            Some(ingestor.clone()),
            SessionManagerOptions::default(),
            Some(SessionServices {
                persistence: Some(SessionPersistence {
                    reader: reader.clone(),
                    sink: events.clone(),
                    clock: clock.clone(),
                    ids: ids.clone(),
                }),
                credentials: None,
            }),
        );
        let runtime = sessions
            .attach(input.model_selection.instance_id.clone(), input.clone())
            .await?;
        assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
        assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
        runtime
            .resume_thread(ResumeThread {
                provider_thread: state.provider_thread.clone(),
                thread_id: Some(input.thread_id.clone()),
                model_selection: Some(input.model_selection.clone()),
                runtime_policy: Some(input.runtime_policy.clone()),
            })
            .await?;
        assert!(matches!(calls.recv().await, Some(FakeCall::Resume(_))));
        let execution = RunExecutionService::new(ingestor.clone(), None);
        let recovery = ProviderRuntimeRecoveryService::new(
            stores.clone(),
            reader.clone(),
            events.clone(),
            Arc::new(sessions.clone()),
            clock,
            ids,
            OutboxSignals::default(),
        );
        let start = |state: RootRunState, turn: TurnInput| StartRootRun {
            state,
            turn,
            session: runtime.clone(),
            inherited_background_items: Default::default(),
            related_thread_ids: vec![],
            related_provider_thread_ids: vec![],
        };

        let mut next = state;
        next.run.id = serde_json::from_value(json!("run:next"))?;
        next.run.ordinal = 2;
        next.run.root_node_id = Some(serde_json::from_value(json!("node:next"))?);
        next.run.active_attempt_id = Some(serde_json::from_value(json!("attempt:next"))?);
        next.attempt.id = serde_json::from_value(json!("attempt:next"))?;
        next.attempt.run_id = next.run.id.clone();
        next.attempt.root_node_id = next.run.root_node_id.clone().expect("successor root");
        next.root_node.id = next.attempt.root_node_id.clone();
        next.root_node.root_node_id = next.attempt.root_node_id.clone();
        next.root_node.run_id = Some(next.run.id.clone());
        let mut event_values = Vec::new();
        for (kind, payload) in [
            ("run.created", t3_contracts::stored::to_value(&next.run)?),
            (
                "run-attempt.created",
                t3_contracts::stored::to_value(&next.attempt)?,
            ),
            (
                "node.updated",
                t3_contracts::stored::to_value(&next.root_node)?,
            ),
        ] {
            event_values.push(serde_json::from_value(json!({
                "id":format!("successor:{kind}"),"threadId":"thread:test","occurredAt":TEST_TIME,"type":kind,"payload":payload,
            }))?);
        }
        events
            .commit(CommitRequest {
                receipt: None,
                guard: CommitGuard::None,
                decision: Decision {
                    events: event_values,
                    ..Default::default()
                },
            })
            .await?;
        turn.run_id = RunId("run:next".into());
        turn.run_ordinal = 2;
        turn.provider_turn_ordinal = 2;
        turn.attempt_id = RunAttemptId("attempt:next".into());
        turn.root_node_id = NodeId("node:next".into());
        let interrupted = execution
            .start_root_run(start(next, turn), CancellationToken::new())
            .await?;
        assert!(matches!(calls.recv().await, Some(FakeCall::Start(_))));
        recovery.prepare_for_shutdown().await?;
        execution.shutdown().await?;
        assert!(matches!(interrupted.settled().await, Err(Error::Cancelled)));
        sessions.shutdown().await?;
        ingestor.shutdown().await;
        assert!(matches!(calls.recv().await, Some(FakeCall::Close)));
        assert!(calls.try_recv().is_err());
    }
    let stores = StoreDatabase(t3_persistence::Database::open(&path)?);
    let recovery = service(stores.clone());
    assert_eq!(recovery.recover().await?.terminalized_runs, 1);
    assert_eq!(recovery.recover().await?, RecoverySummary::default());
    let reader = ProjectionStore::new(stores);
    assert_eq!(
        reader
            .run(ThreadId("thread:test".into()), RunId("run:test".into()))
            .await?
            .unwrap()
            .status,
        OrchestrationV2RunStatus::Completed
    );
    assert_eq!(
        reader
            .run(ThreadId("thread:test".into()), RunId("run:next".into()))
            .await?
            .unwrap()
            .status,
        OrchestrationV2RunStatus::Cancelled
    );
    Ok(())
}

struct Dispatcher {
    stores: StoreDatabase,
    calls: mpsc::UnboundedSender<&'static str>,
}
#[async_trait]
impl CommandDispatcher for Dispatcher {
    async fn dispatch(&self, _: OrchestrationV2ServerCommand) -> Result<CommandReceipt> {
        self.stores
            .read(|db| {
                let status: String = db.query_row(
                    "SELECT status FROM orchestration_v2_projection_runs WHERE run_id='run:test'",
                    [],
                    |r| r.get(0),
                )?;
                assert_eq!(status, "cancelled");
                Ok(())
            })
            .await?;
        self.calls.send("dispatch").expect("test value");
        Ok(CommandReceipt {
            command_id: CommandId("command".into()),
            aggregate: AggregateId::Thread(ThreadId("thread:test".into())),
            command_type: "thread.archive".into(),
            accepted_at: TEST_TIME.into(),
            result_sequence: 0,
            status: ReceiptStatus::Accepted,
            error: None,
        })
    }
}
struct Worker(
    mpsc::UnboundedSender<&'static str>,
    Option<Arc<tokio::sync::Notify>>,
);
#[async_trait]
impl EffectWorker for Worker {
    async fn run_once(&self, _: ClaimOptions) -> Result<bool> {
        Err(Error::Unsupported("unused"))
    }
    async fn run_recovery_once(&self) -> Result<bool> {
        Err(Error::Unsupported("unused"))
    }
    async fn drain(&self, _: usize) -> Result<usize> {
        Err(Error::Unsupported("unused"))
    }
    async fn await_work(&self) -> Result<()> {
        Err(Error::Unsupported("unused"))
    }
    async fn await_settled(&self, _: String) -> Result<()> {
        Err(Error::Unsupported("unused"))
    }
    async fn shutdown(&self) -> Result<()> {
        self.0.send("worker shutdown").expect("test value");
        if let Some(release) = &self.1 {
            release.notified().await;
        }
        Ok(())
    }
}
#[tokio::test]
async fn admission_opens_after_recovery_and_shutdown_closes_it_permanently() -> Result<()> {
    let database = TestDatabase::memory()?;
    seed(&sink(database.stores.clone()), &projection()?).await?;
    let (send, mut calls) = mpsc::unbounded_channel();
    let runtime = RuntimeLayer::new(
        Arc::new(Dispatcher {
            stores: database.stores.clone(),
            calls: send.clone(),
        }),
        Arc::new(service(database.stores.clone())),
        Arc::new(Worker(send, None)),
        Arc::new(ProviderSessionManager::new(
            Arc::new(AdapterRegistry::from_adapters(vec![])),
            None,
        )),
    );
    let command = || {
        serde_json::from_value(serde_json::json!({"type":"thread.archive","commandId":"command","threadId":"thread:test"})).expect("test value")
    };
    assert!(runtime.dispatch(command()).await.is_err());
    assert!(calls.try_recv().is_err());
    assert_eq!(runtime.start().await?.terminalized_runs, 1);
    runtime.dispatch(command()).await?;
    assert_eq!(calls.recv().await, Some("dispatch"));
    assert_eq!(runtime.start().await?, RecoverySummary::default());
    runtime.shutdown().await?;
    assert_eq!(calls.recv().await, Some("worker shutdown"));
    assert!(runtime.dispatch(command()).await.is_err());
    assert!(runtime.start().await.is_err());
    runtime.shutdown().await?;
    assert!(calls.try_recv().is_err());
    Ok(())
}

#[tokio::test]
async fn shutdown_rejects_dispatch_while_waiting_for_worker_cleanup() -> Result<()> {
    use std::{future::Future, task::Poll};
    let database = TestDatabase::memory()?;
    seed(&sink(database.stores.clone()), &projection()?).await?;
    let (send, mut calls) = mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let runtime = Arc::new(RuntimeLayer::new(
        Arc::new(Dispatcher {
            stores: database.stores.clone(),
            calls: send.clone(),
        }),
        Arc::new(service(database.stores)),
        Arc::new(Worker(send, Some(release.clone()))),
        Arc::new(ProviderSessionManager::new(
            Arc::new(AdapterRegistry::from_adapters(vec![])),
            None,
        )),
    ));
    runtime.start().await?;
    let shutdown = {
        let runtime = runtime.clone();
        tokio::spawn(async move { runtime.shutdown().await })
    };
    assert_eq!(calls.recv().await, Some("worker shutdown"));
    let dispatch=runtime.dispatch(serde_json::from_value(serde_json::json!({"type":"thread.archive","commandId":"shutdown:dispatch","threadId":"thread:test"}))?);
    tokio::pin!(dispatch);
    assert!(matches!(
        std::future::poll_fn(|cx| Poll::Ready(dispatch.as_mut().poll(cx))).await,
        Poll::Ready(Err(Error::Service {
            operation: "runtime admission",
            ..
        }))
    ));
    assert!(calls.try_recv().is_err());
    release.notify_one();
    shutdown.await.expect("shutdown task")?;
    Ok(())
}
