use super::provider_runtime_recovery_service_test::{projection, seed, service, sink};
use async_trait::async_trait;
use std::sync::Arc;
use t3_contracts::orchestration_v2::{
    OrchestrationV2ServerCommand, OrchestrationV2ThreadProjection,
};
use t3_contracts::orchestration_v2_id_table::IdOrIndex::Id;
use t3_orchestration::{
    api::*,
    effects::*,
    error::{Error, Result},
    runtime::sessions::*,
    stores::{StoreDatabase, receipts::*},
    testing::*,
};
use tokio::sync::mpsc;

struct DriftRuntime {
    database: TestDatabase,
    events: Arc<t3_orchestration::runtime::commit::EventSink>,
    reader: Arc<t3_orchestration::projections::store::ProjectionStore>,
    commands: t3_orchestration::runtime::dispatch::Dispatcher,
}

impl DriftRuntime {
    async fn new() -> Result<Self> {
        use t3_orchestration::{
            decision::engine::PureDecisionEngine, provider::*, runtime::dispatch::*,
        };
        let database = TestDatabase::memory()?;
        let reader = Arc::new(t3_orchestration::projections::store::ProjectionStore::new(
            database.stores.clone(),
        ));
        let events = Arc::new(sink(database.stores.clone()));
        let (adapter, _calls, _) = super::provider_session_manager_test::fake()?;
        let provider = projection()?.provider_threads.remove(0);
        let (alternate, _calls) = FakeProviderAdapter::new(
            t3_contracts::provider_instance::ProviderInstanceId("alternate".into()),
            t3_contracts::provider_instance::ProviderDriverKind("codex".into()),
            adapter.runtime.session().capabilities.clone(),
            adapter.runtime.session().clone(),
            ThreadSnapshot {
                provider_thread: provider,
                provider_turns: vec![],
                messages: vec![],
                reasoning_message_ids: None,
                runtime_requests: vec![],
                provider_payload: None,
            },
        );
        let clock = Arc::new(ManualClock::new(TEST_TIME));
        let inputs = Arc::new(ProjectionCommandInputs {
            reader: reader.clone(),
            providers: Arc::new(AdapterRegistry::from_adapters(vec![
                adapter,
                Arc::new(alternate),
            ])),
            clock: clock.clone(),
            ids: Arc::new(SequenceIds::new("drift")),
        });
        let commands = t3_orchestration::runtime::dispatch::Dispatcher::new(
            DispatchServices {
                database: database.stores.clone(),
                inputs,
                engine: Arc::new(PureDecisionEngine),
                committer: events.clone(),
                recovery: Arc::new(service(database.stores.clone())),
                clock,
            },
            None,
        );
        let runtime = Self {
            database,
            events,
            reader,
            commands,
        };
        runtime.dispatch(serde_json::json!({"type":"thread.create","commandId":"create","threadId":"thread:test","projectId":"project:test","title":"Drift","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":"/isolated/drift","createdBy":"user","creationSource":"web"})).await?;
        Ok(runtime)
    }
    async fn dispatch(&self, command: serde_json::Value) -> Result<CommandReceipt> {
        self.commands
            .dispatch(serde_json::from_value(command)?)
            .await
    }
    async fn projection(&self) -> Result<OrchestrationV2ThreadProjection> {
        self.reader
            .projection(ThreadId("thread:test".into()))
            .await?
            .ok_or(Error::Stale)
    }
    async fn message(&self, id: &str, queued: bool, extra: serde_json::Value) -> Result<()> {
        let mut command = serde_json::json!({"type":"message.dispatch","commandId":id,"threadId":"thread:test","messageId":id,"text":"Scheduled prompt","attachments":[],"dispatchMode":{"type":if queued {"queue_after_active"} else {"start_immediately"}},"createdBy":"user","creationSource":"web"});
        command
            .as_object_mut()
            .expect("command")
            .extend(extra.as_object().expect("extra fields").clone());
        self.dispatch(command).await?;
        Ok(())
    }
}

async fn queued_scheduled_case(
    newer: serde_json::Value,
    pin: Option<serde_json::Value>,
    fresh: bool,
) -> Result<()> {
    use serde_json::json;
    use t3_contracts::orchestration_v2::*;
    let runtime = DriftRuntime::new().await?;
    runtime.message("active", false, json!({})).await?;
    let active = runtime.projection().await?.runs[0].clone();
    let mut extra = json!({"scheduledTaskId":"task:test","freshContext":fresh});
    if let Some(pin) = &pin {
        extra["modelSelection"] = pin.clone();
    }
    runtime.message("scheduled", true, extra).await?;
    let queued = runtime.projection().await?;
    let scheduled = queued
        .runs
        .iter()
        .find(|run| run.user_message_id == Id(MessageId("scheduled".into())))
        .expect("queued scheduled run");
    assert_eq!(scheduled.status, OrchestrationV2RunStatus::Queued);
    let reserved = scheduled.provider_thread_id.clone();
    if fresh {
        let provider = queued
            .provider_threads
            .iter()
            .find(|provider| Some(&provider.id) == reserved.as_ref())
            .expect("fresh queue reserves a provider thread");
        assert_eq!(
            serde_json::to_value(&provider.provider_instance_id)?,
            "codex"
        );
        assert!(provider.first_run_ordinal.is_none());
        assert!(provider.native_thread_ref.is_none());
    }
    runtime.dispatch(json!({"type":"run.interrupt","commandId":"stop","threadId":"thread:test","runId":active.id,"holdQueue":true})).await?;
    runtime.dispatch(json!({"type":"thread.model-selection.set","commandId":"select","threadId":"thread:test","modelSelection":newer})).await?;
    runtime
        .dispatch(json!({"type":"queue.resume","commandId":"resume","threadId":"thread:test"}))
        .await?;
    runtime
        .commands
        .resume_queue(ThreadId("thread:test".into()), None)
        .await?;
    let after = runtime.projection().await?;
    let delivered = after
        .runs
        .iter()
        .find(|run| run.user_message_id == Id(MessageId("scheduled".into())))
        .expect("delivered scheduled run");
    let expected = pin.clone().unwrap_or(newer);
    assert_eq!(delivered.status, OrchestrationV2RunStatus::Starting);
    assert_eq!(serde_json::to_value(&delivered.model_selection)?, expected);
    assert_eq!(
        serde_json::to_value(&after.thread.model_selection)?,
        expected
    );
    assert_eq!(
        serde_json::to_value(
            &after
                .attempts
                .iter()
                .find(|attempt| Some(&attempt.id) == delivered.active_attempt_id.as_ref())
                .expect("started run attempt")
                .provider_instance_id
        )?,
        expected["instanceId"]
    );
    let message = after
        .messages
        .iter()
        .find(|message| message.id == Id(MessageId("scheduled".into())))
        .expect("scheduled message");
    assert_eq!(
        serde_json::to_value(message)?.get("followsThreadSelection"),
        pin.is_none().then_some(&json!(true))
    );
    if fresh {
        assert_ne!(delivered.provider_thread_id, reserved);
        let old = after
            .provider_threads
            .iter()
            .find(|provider| Some(&provider.id) == reserved.as_ref())
            .expect("original reservation is retained");
        assert_eq!(serde_json::to_value(&old.provider_instance_id)?, "codex");
        let new = after
            .provider_threads
            .iter()
            .find(|provider| Some(&provider.id) == delivered.provider_thread_id.as_ref())
            .expect("replacement provider thread");
        assert_eq!(
            serde_json::to_value(&new.provider_instance_id)?,
            "alternate"
        );
    }
    Ok(())
}

#[tokio::test]
async fn drift_queued_scheduled_delivery_uses_current_model_and_options() -> Result<()> {
    queued_scheduled_case(serde_json::json!({"instanceId":"codex","model":"new","options":[{"id":"reasoningEffort","value":"high"}]}), None, false).await
}

#[tokio::test]
async fn drift_queued_scheduled_delivery_uses_current_provider() -> Result<()> {
    queued_scheduled_case(
        serde_json::json!({"instanceId":"alternate","model":"new"}),
        None,
        false,
    )
    .await
}

#[tokio::test]
async fn drift_queued_scheduled_delivery_preserves_explicit_model_pin() -> Result<()> {
    queued_scheduled_case(
        serde_json::json!({"instanceId":"alternate","model":"new"}),
        Some(serde_json::json!({"instanceId":"codex","model":"pinned"})),
        false,
    )
    .await
}

#[tokio::test]
async fn drift_fresh_scheduled_delivery_replaces_the_old_instances_reserved_thread() -> Result<()> {
    queued_scheduled_case(
        serde_json::json!({"instanceId":"alternate","model":"new"}),
        None,
        true,
    )
    .await
}

#[tokio::test]
async fn drift_visits_persist_only_read_state_and_stale_visits_persist_nothing() -> Result<()> {
    use serde_json::{Value, json};
    let runtime = DriftRuntime::new().await?;
    let before = runtime.projection().await?.thread;
    runtime.dispatch(json!({"type":"thread.visit","commandId":"visit","threadId":"thread:test","visitedAt":"2026-11-13T12:00:00.000Z"})).await?;
    let visited = runtime.projection().await?.thread;
    assert_eq!(visited.updated_at, before.updated_at);
    assert_eq!(
        serde_json::to_value(&visited)?["lastVisitedAt"],
        "2026-11-13T12:00:00.000Z"
    );
    runtime
        .database
        .stores
        .read(|db| {
            let (kind, payload): (String, String) = db.query_row(
                "SELECT event_type,payload_json FROM orchestration_events WHERE command_id='visit'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!(kind, "thread.visit-recorded");
            let value: Value = serde_json::from_str(&payload)?;
            assert!(
                value
                    .as_object()
                    .unwrap()
                    .keys()
                    .all(|key| matches!(key.as_str(), "lastVisitedAt" | "opens" | "threadId"))
            );
            Ok(())
        })
        .await?;
    runtime.dispatch(json!({"type":"thread.visit","commandId":"stale-visit","threadId":"thread:test","visitedAt":"2026-11-13T11:00:00.000Z"})).await?;
    runtime
        .database
        .stores
        .read(|db| {
            assert_eq!(
                db.query_row(
                    "SELECT COUNT(*) FROM orchestration_events WHERE command_id='stale-visit'",
                    [],
                    |row| row.get::<_, i64>(0)
                )?,
                0
            );
            Ok(())
        })
        .await?;
    assert_eq!(
        runtime.projection().await?.thread.last_visited_at,
        visited.last_visited_at
    );
    let shell = runtime
        .reader
        .shell(ThreadId("thread:test".into()))
        .await?
        .unwrap();
    assert_eq!(
        serde_json::to_value(shell)?["lastVisitedAt"],
        "2026-11-13T12:00:00.000Z"
    );
    Ok(())
}

async fn snooze_queued_case(kind: &str) -> Result<()> {
    use serde_json::json;
    let runtime = DriftRuntime::new().await?;
    let mut state = projection()?;
    let mut active = serde_json::to_value(&state.runs[0])?;
    active["status"] = json!(if kind == "limited-user" {
        "failed"
    } else {
        "cancelled"
    });
    active["completedAt"] = json!(TEST_TIME);
    let mut queued = active.clone();
    queued["id"] = json!("queued");
    queued["status"] = json!("queued");
    queued["ordinal"] = json!(2);
    queued["userMessageId"] = json!("scheduled");
    queued["queueHeld"] = json!(kind != "limited-user");
    queued["completedAt"] = serde_json::Value::Null;
    let mut message: serde_json::Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2ConversationMessage.jsonl")
            .lines()
            .next()
            .expect("message fixture"),
    )?;
    message["id"] = json!("scheduled");
    message["threadId"] = json!("thread:test");
    message["runId"] = json!("queued");
    message["createdBy"] = json!(if kind == "user" || kind == "limited-user" {
        "user"
    } else {
        "agent"
    });
    message["creationSource"] = json!(if kind == "user" || kind == "limited-user" {
        "web"
    } else {
        "server"
    });
    if kind == "notification" {
        message["notification"] = json!({"source":{"kind":"background_task"},"outcome":"updated","summary":"Background updated"});
    }
    if kind == "delegated-completion" {
        message["delegatedCompletion"] =
            json!({"parentRunId":active["id"],"generation":1,"taskIds":["task:test"]});
    }
    state.runs = vec![
        serde_json::from_value(active)?,
        serde_json::from_value(queued)?,
    ];
    state.messages = vec![serde_json::from_value(message)?];
    if kind == "limited-user" {
        state.provider_sessions.clear();
        let mut item = super::provider_runtime_recovery_service_test::item("limit", "error");
        item["status"] = json!("failed");
        item["completedAt"] = json!(TEST_TIME);
        item["failure"] = json!({"class":"usage_limit","message":"Claude API rate limit reached. Try again later.","code":"api_error_429","retryable":true,"resetAt":null});
        state.turn_items = vec![serde_json::from_value(item)?];
    }
    seed(&runtime.events, &state).await?;
    let result = runtime.dispatch(json!({"type":"thread.snooze","commandId":"snooze","threadId":"thread:test","snoozedUntil":"2099-07-25T09:00:00.000Z"})).await;
    if kind == "user" {
        assert!(
            matches!(result, Err(Error::Rejected { reason, .. }) if reason.contains("queued run"))
        );
    } else {
        result?;
        assert!(
            runtime.projection().await?.thread.snoozed_until.is_some(),
            "{kind}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn drift_snooze_allows_queued_notifications() -> Result<()> {
    snooze_queued_case("notification").await
}

#[tokio::test]
async fn drift_snooze_allows_queued_delegated_completions() -> Result<()> {
    snooze_queued_case("delegated-completion").await
}

#[tokio::test]
async fn drift_snooze_refuses_a_queued_user_message() -> Result<()> {
    snooze_queued_case("user").await
}

#[tokio::test]
async fn drift_snooze_allows_a_queued_user_message_held_by_usage_limit_without_reset() -> Result<()>
{
    snooze_queued_case("limited-user").await
}

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
