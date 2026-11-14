use super::provider_event_ingestor_test::{decode, fixture, item};
use serde_json::{Value, json};
use t3_orchestration::{
    api::*, error::Result, ports::runs_ingestion::run_execution_service::*, provider::*,
};

struct LifecycleHost {
    events: std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
    fail_context: bool,
}
struct PreviewLog(std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>);
impl PreviewActivity for PreviewLog {
    fn end(self: Box<Self>) {
        self.0.lock().expect("test value").push("preview-end");
    }
}
#[async_trait::async_trait]
impl RunExecutionHost for LifecycleHost {
    async fn app_context(&self, _thread: ThreadId) -> Result<Vec<AppModelContext>> {
        self.events.lock().expect("test value").push("app-context");
        if self.fail_context {
            return Err(t3_orchestration::error::Error::Unsupported(
                "fixture MCP context failure",
            ));
        }
        Ok(vec![AppModelContext {
            item_id: TurnItemId("unsafe\"<🧪".into()),
            text: "Keep this context verbatim".into(),
        }])
    }
    async fn touch_mcp_credential(&self, _thread: ThreadId) -> Result<()> {
        self.events.lock().expect("test value").push("touch");
        if self.fail_context {
            return Err(t3_orchestration::error::Error::Unsupported(
                "fixture credential refresh failure",
            ));
        }
        Ok(())
    }
    fn begin_preview(&self, _thread: ThreadId) -> Box<dyn PreviewActivity> {
        self.events
            .lock()
            .expect("test value")
            .push("preview-begin");
        Box::new(PreviewLog(self.events.clone()))
    }
    async fn run_started(&self, context: ProviderTurnAnalyticsContext) -> Result<()> {
        assert_eq!(context.run_id, Some(RunId("run:test".into())));
        assert!(matches!(
            context.segment,
            Some(ProviderTurnSegment::Attempt(RunAttemptReason::Initial))
        ));
        self.events.lock().expect("test value").push("run-started");
        Ok(())
    }
    async fn refresh_after_turn(&self, _project: ProjectId) -> Result<()> {
        self.events
            .lock()
            .expect("test value")
            .push("refresh-after-turn");
        Err(t3_orchestration::error::Error::Unsupported(
            "fixture refresh failure",
        ))
    }
}

#[tokio::test]
async fn snapshot_batches_normalize_inside_the_writer_and_flush_before_root_completion()
-> Result<()> {
    use super::{
        provider_event_ingestor_test::Harness,
        provider_turn_start_service_test::{finish, policy, seed, service, sessions},
    };
    use std::sync::Arc;
    use t3_orchestration::ports::commit_worker::provider_commit_batch::ProviderCommitBatch;
    use tokio_util::sync::CancellationToken;
    let h = Harness::new().await?;
    let state = seed(&h).await?;
    let batches = ProviderCommitBatch::new((*h.sink).clone(), true);
    let mut stats = batches.subscribe_stats();
    let (adapter, _calls) = h.fake();
    let adapter = Arc::new(adapter);
    let mut start = service(&h, sessions(adapter.clone()));
    start.execution = Arc::new((*start.execution).clone().with_batches(batches.clone()));
    let handle = start
        .start(
            ThreadId("thread:test".into()),
            RunId("run:test".into()),
            policy(),
            false,
            CancellationToken::new(),
        )
        .await?
        .unwrap();
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: adapter.driver(),
            kind: turn("pt:test", Some("attempt:test"), "turn:test"),
        })
        .await?;
    let mut node = state.root_node;
    node.id = t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(NodeId("node:batch".into()));
    node.kind = ExecutionNodeKind::ToolCall;
    node.status = ExecutionNodeStatus::Completed;
    node.completed_at = Some(h.clock.now());
    for _ in 0..2 {
        adapter
            .runtime
            .emit(ProviderEvent {
                driver: adapter.driver(),
                kind: ProviderEventKind::Node(Box::new(node.clone())),
            })
            .await?;
    }
    for index in 0..8 {
        let mut tool = item("dynamic_tool", &format!("item:batch:{index}"));
        tool["toolName"] = "record".into();
        tool["input"] = json!({"index":index});
        tool["status"] = "completed".into();
        tool["completedAt"] = json!(h.clock.now());
        adapter
            .runtime
            .emit(ProviderEvent {
                driver: adapter.driver(),
                kind: ProviderEventKind::TurnItem {
                    item: Box::new(decode(tool)),
                    session_scoped: None,
                },
            })
            .await?;
    }
    finish(&adapter, &h, handle).await?;
    let mut jobs = 0;
    while jobs < 10 {
        let report = stats.recv().await.unwrap();
        assert!(report.committed);
        assert!(report.jobs <= 64);
        assert!(report.bytes <= 1024 * 1024);
        jobs += report.jobs;
    }
    assert_eq!(jobs, 10);
    let stored = h
        .sink
        .events_by_command(CommandId(
            "command:effect:provider-turn.start:run:test".into(),
        ))
        .await?;
    let values = stored
        .iter()
        .map(|stored| t3_contracts::stored::to_value(&stored.event))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    assert_eq!(
        values
            .iter()
            .filter(
                |event| event["type"] == "node.updated" && event["payload"]["id"] == "node:batch"
            )
            .count(),
        1
    );
    assert_eq!(
        values
            .iter()
            .filter(|event| event["type"] == "turn-item.updated"
                && event["payload"]["id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("item:batch:")))
            .count(),
        8
    );
    let projection = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .unwrap();
    assert_eq!(projection.runs[0].status, OrchestrationV2RunStatus::Waiting);
    assert_eq!(
        projection
            .turn_items
            .iter()
            .filter(|item| t3_contracts::stored::to_value(item).unwrap()["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("item:batch:")))
            .count(),
        8
    );
    start.execution.shutdown().await?;
    batches.shutdown().await
}

#[tokio::test(start_paused = true)]
async fn held_streaming_text_flushes_after_idle_and_keeps_its_reserved_position_before_later_items()
-> Result<()> {
    use super::{
        provider_event_ingestor_test::Harness,
        provider_turn_start_service_test::{finish, policy, seed, service, sessions},
    };
    use std::sync::Arc;
    use t3_contracts::application_event::ApplicationStoredEvent;
    use t3_orchestration::composition::StaticSettings;
    use tokio_util::sync::CancellationToken;
    for batched in [false, true] {
        let h = Harness::new().await?;
        let state = seed(&h).await?;
        let batches = batched.then(|| {
            t3_orchestration::ports::commit_worker::provider_commit_batch::ProviderCommitBatch::new(
                (*h.sink).clone(),
                true,
            )
        });
        let (adapter, _calls) = h.fake();
        let adapter = Arc::new(adapter);
        let mut start = service(&h, sessions(adapter.clone()));
        if let Some(batches) = &batches {
            start.execution = Arc::new((*start.execution).clone().with_batches(batches.clone()));
        }
        let mut settings = serde_json::to_value(t3_contracts::settings::ServerSettings::default())?;
        settings["responseStreamingMode"] = "turn".into();
        settings["projectSettingsOverrides"]["project:test"] =
            json!({"responseStreamingMode":"paragraph"});
        start.execution = Arc::new((*start.execution).clone().with_settings(Arc::new(
            StaticSettings::new(serde_json::from_value(settings)?),
        )));
        let handle = start
            .start(
                ThreadId("thread:test".into()),
                RunId("run:test".into()),
                policy(),
                false,
                CancellationToken::new(),
            )
            .await?
            .unwrap();
        let mut events = h.sink.subscribe();
        adapter
            .runtime
            .emit(ProviderEvent {
                driver: adapter.driver(),
                kind: turn("pt:test", Some("attempt:test"), "turn:test"),
            })
            .await?;
        let mut ghost = state.root_node;
        ghost.id =
            t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(NodeId("node:ghost".into()));
        ghost.kind = ExecutionNodeKind::AssistantMessage;
        ghost.status = ExecutionNodeStatus::Running;
        adapter
            .runtime
            .emit(ProviderEvent {
                driver: adapter.driver(),
                kind: ProviderEventKind::Node(Box::new(ghost)),
            })
            .await?;
        let mut held = item("assistant_message", "item:held");
        held["messageId"] = "message:held".into();
        held["providerTurnId"] = "turn:test".into();
        held["text"] = "Incomplete paragraph".into();
        held["streaming"] = true.into();
        held["attachments"] = json!([]);
        adapter
            .runtime
            .emit(ProviderEvent {
                driver: adapter.driver(),
                kind: ProviderEventKind::TurnItem {
                    item: Box::new(decode(held.clone())),
                    session_scoped: None,
                },
            })
            .await?;
        let mut later = item("dynamic_tool", "item:later");
        later["toolName"] = "marker".into();
        later["input"] = json!({});
        later["status"] = "completed".into();
        later["completedAt"] = json!(h.clock.now());
        adapter
            .runtime
            .emit(ProviderEvent {
                driver: adapter.driver(),
                kind: ProviderEventKind::TurnItem {
                    item: Box::new(decode(later)),
                    session_scoped: None,
                },
            })
            .await?;
        loop {
            if let ApplicationStoredEvent::Orchestration(stored) = events.recv().await.unwrap()
                && let OrchestrationV2DomainEvent::TurnItemUpdated(event) = stored.event
                && t3_contracts::stored::to_value(&event.payload)?["id"] == "item:later"
            {
                break;
            }
        }
        assert!(
            h.reader
                .turn_item(
                    ThreadId("thread:test".into()),
                    TurnItemId("item:held".into())
                )
                .await?
                .is_none()
        );
        let nodes = h
            .reader
            .records(
                ThreadId("thread:test".into()),
                vec![RecordKind::Nodes],
                RecordFilter::default(),
            )
            .await?;
        assert!(!nodes.records.iter().any(|records|matches!(records,ProjectionRecordSet::Nodes(nodes) if nodes.iter().any(|node|node.id==t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(NodeId("node:ghost".into()))))));
        let positions=h.database.stores.read(|db| {
        let position=|id:&str|db.query_row("SELECT ordinal FROM orchestration_v2_turn_item_positions WHERE thread_id='thread:test' AND turn_item_id=?1",[id],|row|row.get::<_,i64>(0));
        Ok((position("item:held")?,position("item:later")?))
    }).await?;
        assert!(positions.0 < positions.1);
        h.clock.set("2026-09-03T00:00:00.800Z");
        tokio::time::advance(std::time::Duration::from_millis(800)).await;
        loop {
            if let ApplicationStoredEvent::Orchestration(stored) = events.recv().await.unwrap()
                && let OrchestrationV2DomainEvent::TurnItemUpdated(event) = stored.event
                && t3_contracts::stored::to_value(&event.payload)?["id"] == "item:held"
            {
                break;
            }
        }
        let saved = t3_contracts::stored::to_value(
            &h.reader
                .turn_item(
                    ThreadId("thread:test".into()),
                    TurnItemId("item:held".into()),
                )
                .await?
                .unwrap(),
        )?;
        assert_eq!(saved["text"], "Incomplete paragraph");
        assert_eq!(saved["ordinal"], positions.0);
        held["text"] = "Incomplete paragraph completed.".into();
        held["streaming"] = false.into();
        held["status"] = "completed".into();
        held["completedAt"] = json!(h.clock.now());
        adapter
            .runtime
            .emit(ProviderEvent {
                driver: adapter.driver(),
                kind: ProviderEventKind::TurnItem {
                    item: Box::new(decode(held)),
                    session_scoped: None,
                },
            })
            .await?;
        finish(&adapter, &h, handle).await?;
        start.execution.shutdown().await?;
        if let Some(batches) = batches {
            batches.shutdown().await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn context_and_liveness_failures_do_not_block_a_restart_and_preview_ends_on_scope_cancellation()
-> Result<()> {
    use super::{
        provider_event_ingestor_test::Harness,
        provider_turn_start_service_test::{call, finish, policy, seed, service, sessions},
    };
    use std::sync::{Arc, Mutex};
    use t3_orchestration::{error::Error, testing::FakeCall};
    use tokio_util::sync::CancellationToken;
    for cancel in [false, true] {
        let h = Harness::new().await?;
        let mut state = seed(&h).await?;
        state.attempt.reason = RunAttemptReason::SteeringRestart;
        h.write(
            "run-attempt.updated",
            t3_contracts::stored::to_value(&state.attempt)?,
        )
        .await?;
        let (adapter, mut calls) = h.fake();
        let adapter = Arc::new(adapter);
        let mut service = service(&h, sessions(adapter.clone()));
        let lifecycle = Arc::new(Mutex::new(vec![]));
        service.execution = Arc::new((*service.execution).clone().with_host(Arc::new(
            LifecycleHost {
                events: lifecycle.clone(),
                fail_context: true,
            },
        )));
        let handle = service
            .start(
                ThreadId("thread:test".into()),
                RunId("run:test".into()),
                policy(),
                false,
                CancellationToken::new(),
            )
            .await?
            .unwrap();
        assert!(matches!(call(&mut calls).await, FakeCall::Open(_)));
        assert!(matches!(call(&mut calls).await, FakeCall::Ensure(_)));
        assert!(matches!(call(&mut calls).await, FakeCall::Subscribe));
        let FakeCall::Start(turn) = call(&mut calls).await else {
            panic!("expected start despite context failure")
        };
        assert!(turn.app_context.is_empty());
        if cancel {
            service.execution.shutdown().await?;
            assert!(matches!(handle.settled().await, Err(Error::Cancelled)));
        } else {
            finish(&adapter, &h, handle).await?;
            service.execution.shutdown().await?;
        }
        let events = lifecycle.lock().unwrap();
        assert!(!events.contains(&"run-started"));
        assert_eq!(
            events
                .iter()
                .filter(|&&event| event == "preview-end")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|&&event| event == "refresh-after-turn")
                .count(),
            usize::from(!cancel)
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_provider_turn_persists_items_drains_finalization_and_recovers_after_reopening_sqlite()
-> Result<()> {
    use super::{
        provider_event_ingestor_test::Harness,
        provider_turn_start_service_test::{call, policy, seed, service, sessions},
    };
    use async_trait::async_trait;
    use std::sync::Arc;
    use t3_orchestration::{
        effects::{ClaimOptions, EffectRequest, EffectRow},
        ports::{
            checkpoints_forks::{
                checkpoint_capture_service::CheckpointCaptureService,
                checkpoint_service::{CheckpointService, WorkspaceLocks},
            },
            runs_ingestion::{
                provider_turn_start_service_testkit::NoRepositoryCheckpoints,
                run_finalization_service::{
                    FinalizationObserver, FinalizationTarget, RunFinalizationService,
                },
            },
            sessions_recovery::{
                provider_adapter_registry::AdapterRegistry,
                provider_runtime_recovery_service::ProviderRuntimeRecoveryService,
                provider_session_manager::ProviderSessionManager,
            },
        },
        projections::store::ProjectionStore,
        runtime::worker::{EffectWorker as Worker, OutboxSignals, WorkerOptions},
        stores::StoreDatabase,
        testing::{FakeCall, ManualClock, SequenceIds, TEST_TIME, TestDatabase},
    };
    use tokio_util::sync::CancellationToken;
    struct Refresh;
    #[async_trait]
    impl FinalizationObserver for Refresh {
        async fn refresh(&self, _cwd: String, _thread: ThreadId, _run: RunId) -> Result<()> {
            Ok(())
        }
    }
    struct Executor(RunFinalizationService);
    #[async_trait]
    impl EffectExecutor for Executor {
        async fn execute(&self, row: EffectRow, cancellation: CancellationToken) -> Result<()> {
            let EffectRequest::Capture { run_id, scope_id } = row.request else {
                return Err(t3_orchestration::error::Error::Unsupported(
                    "unexpected lifecycle effect",
                ));
            };
            self.0
                .finalize(
                    FinalizationTarget {
                        thread_id: row.thread_id,
                        run_id,
                        scope_id,
                    },
                    cancellation,
                )
                .await
                .map_err(|error| *error.cause)
        }
        async fn failed(
            &self,
            _row: EffectRow,
            _error: &t3_orchestration::error::Error,
            _retry: bool,
        ) -> Result<()> {
            Ok(())
        }
    }
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("statev2.sqlite");
    let database = TestDatabase {
        stores: StoreDatabase(t3_persistence::Database::open(&path)?),
        directory: None,
    };
    let h = Harness::with_database(database).await?;
    let mut state = seed(&h).await?;
    state.attempt.reason = RunAttemptReason::Initial;
    h.write(
        "run-attempt.updated",
        t3_contracts::stored::to_value(&state.attempt)?,
    )
    .await?;
    let (adapter, mut calls) = h.fake();
    let adapter = Arc::new(adapter);
    let mut start = service(&h, sessions(adapter.clone()));
    let lifecycle = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    start.execution = Arc::new(
        (*start.execution)
            .clone()
            .with_host(Arc::new(LifecycleHost {
                events: lifecycle.clone(),
                fail_context: false,
            })),
    );
    let handle = start
        .start(
            ThreadId("thread:test".into()),
            RunId("run:test".into()),
            policy(),
            false,
            CancellationToken::new(),
        )
        .await?
        .unwrap();
    assert!(matches!(call(&mut calls).await, FakeCall::Open(_)));
    assert!(matches!(call(&mut calls).await, FakeCall::Ensure(_)));
    assert!(matches!(call(&mut calls).await, FakeCall::Subscribe));
    let FakeCall::Start(turn_input) = call(&mut calls).await else {
        panic!("expected provider start")
    };
    assert_eq!(
        turn_input.app_context,
        vec![(
            "mcp_app_unsafe____".into(),
            "Keep this context verbatim".into()
        )]
    );
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: adapter.driver(),
            kind: turn("pt:test", Some("attempt:test"), "turn:test"),
        })
        .await?;
    let mut text = item("assistant_message", "item:reply");
    text["messageId"] = "message:reply".into();
    text["text"] = "Replayed reply".into();
    text["attachments"] = json!([]);
    text["streaming"] = false.into();
    text["status"] = "completed".into();
    text["completedAt"] = TEST_TIME.into();
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: adapter.driver(),
            kind: ProviderEventKind::TurnItem {
                item: Box::new(decode(text)),
                session_scoped: None,
            },
        })
        .await?;
    let mut completed_turn = turn("pt:test", Some("attempt:test"), "turn:test");
    if let ProviderEventKind::Turn { provider_turn, .. } = &mut completed_turn {
        provider_turn.status = ProviderTurnStatus::Completed;
        provider_turn.completed_at = Some(h.clock.now());
    }
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: adapter.driver(),
            kind: completed_turn,
        })
        .await?;
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: adapter.driver(),
            kind: terminal(),
        })
        .await?;
    handle.settled().await?;
    assert_eq!(
        *lifecycle.lock().unwrap(),
        vec![
            "run-started",
            "preview-begin",
            "touch",
            "app-context",
            "refresh-after-turn",
            "preview-end"
        ]
    );
    assert_eq!(
        h.reader
            .run(ThreadId("thread:test".into()), RunId("run:test".into()))
            .await?
            .unwrap()
            .status,
        OrchestrationV2RunStatus::Waiting
    );
    let capture = Arc::new(CheckpointCaptureService {
        checkpoints: Arc::new(CheckpointService::new(
            Arc::new(NoRepositoryCheckpoints),
            Arc::new(NoRepositoryCheckpoints),
            WorkspaceLocks::workspace_leases(),
        )),
        projections: h.reader.clone(),
        events: h.sink.clone(),
        ids: Arc::new(SequenceIds::new("capture")),
        clock: h.clock.clone(),
    });
    let worker = Worker::new(
        h.database.stores.clone(),
        Arc::new(Executor(RunFinalizationService {
            capture,
            projections: h.reader.clone(),
            observer: Arc::new(Refresh),
        })),
        h.clock.clone(),
        h.sink.signals(),
        WorkerOptions::default(),
    );
    assert_eq!(worker.drain(1).await?, 1);
    assert!(!worker.run_once(ClaimOptions::default()).await?);
    let projection = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .unwrap();
    assert_eq!(
        projection.runs[0].status,
        OrchestrationV2RunStatus::Completed
    );
    assert_eq!(projection.attempts[0].status, RunAttemptStatus::Completed);
    assert!(projection.turn_items.iter().any(|item| matches!(item, OrchestrationV2TurnItem::AssistantMessage(item) if item.text == "Replayed reply")));
    worker.shutdown().await?;
    start.execution.shutdown().await?;
    drop(worker);
    drop(start);
    drop(h);
    let reopened = StoreDatabase(t3_persistence::Database::open(&path)?);
    let reader = Arc::new(ProjectionStore::new(reopened.clone()));
    let clock = Arc::new(ManualClock::new(TEST_TIME));
    let signals = OutboxSignals::default();
    let events = Arc::new(t3_orchestration::runtime::commit::EventSink::new(
        reopened.clone(),
        Arc::new(t3_orchestration::projections::write::SqliteProjectionWriter),
        clock.clone(),
        signals.clone(),
    ));
    let sessions = Arc::new(ProviderSessionManager::new(
        Arc::new(AdapterRegistry::from_adapters(vec![])),
        None,
    ));
    let recovery = ProviderRuntimeRecoveryService::new(
        reopened,
        reader.clone(),
        events,
        sessions.clone(),
        clock,
        Arc::new(SequenceIds::new("recovery")),
        signals,
    );
    assert_eq!(recovery.recover().await?.terminalized_runs, 0);
    assert_eq!(
        reader
            .run(ThreadId("thread:test".into()), RunId("run:test".into()))
            .await?
            .unwrap()
            .status,
        OrchestrationV2RunStatus::Completed
    );
    assert!(
        reader
            .turn_item(
                ThreadId("thread:test".into()),
                TurnItemId("item:reply".into())
            )
            .await?
            .is_some()
    );
    sessions.shutdown().await
}

fn identity() -> ProviderEventRouteIdentity {
    ProviderEventRouteIdentity {
        thread_id: ThreadId("thread:test".into()),
        run_id: RunId("run:test".into()),
        attempt_id: RunAttemptId("attempt:test".into()),
        provider_thread_id: ProviderThreadId("pt:test".into()),
    }
}
fn terminal() -> ProviderEventKind {
    ProviderEventKind::Terminal {
        provider_thread_id: ProviderThreadId("pt:test".into()),
        provider_turn_id: ProviderTurnId("turn:test".into()),
        run_ordinal: 1,
        disposition: ThreadDisposition::Reusable,
        outcome: TerminalOutcome::Completed { reason: None },
    }
}
fn child(id: &str, parent: &str) -> ProviderEventKind {
    let mut thread = fixture("OrchestrationV2AppThread");
    thread["id"] = id.into();
    thread["lineage"] = json!({"parentThreadId":parent,"relationshipToParent":"subagent","rootThreadId":"thread:test"});
    ProviderEventKind::AppThread {
        app_thread: Box::new(decode(thread)),
        create_only: None,
    }
}
fn agent(child: &str, run: &str) -> Value {
    let mut agent = fixture("OrchestrationV2Subagent");
    agent["id"] = format!("node:{child}").into();
    agent["threadId"] = "thread:test".into();
    agent["runId"] = run.into();
    agent["childThreadId"] = child.into();
    agent["providerThreadId"] = format!("pt:{child}").into();
    agent["origin"] = "provider_native".into();
    agent["status"] = "running".into();
    agent
        .as_object_mut()
        .expect("test value")
        .shift_remove("workflow");
    agent
}
fn message(thread: &str, run: Option<&str>) -> ProviderEventKind {
    let mut message = fixture("OrchestrationV2ConversationMessage");
    message["threadId"] = thread.into();
    message["runId"] = json!(run);
    ProviderEventKind::Message(Box::new(decode(message)))
}
fn turn(thread: &str, attempt: Option<&str>, id: &str) -> ProviderEventKind {
    let mut turn = fixture("OrchestrationV2ProviderTurn");
    turn["id"] = id.into();
    turn["providerThreadId"] = thread.into();
    turn["runAttemptId"] = json!(attempt);
    turn["status"] = "running".into();
    ProviderEventKind::Turn {
        provider_turn: Box::new(decode(turn)),
        thread_id: None,
    }
}

#[test]
fn shared_runtime_events_only_reach_the_owning_run_and_attempt() -> Result<()> {
    let first = identity();
    let second = ProviderEventRouteIdentity {
        thread_id: ThreadId("other".into()),
        run_id: RunId("other".into()),
        attempt_id: RunAttemptId("other".into()),
        provider_thread_id: ProviderThreadId("other".into()),
    };
    let mut a = ProviderEventRoutingState::new(&first, None);
    let mut b = ProviderEventRoutingState::new(&second, None);
    for event in [
        turn("pt:test", Some("attempt:test"), "turn:test"),
        message("thread:test", Some("run:test")),
        terminal(),
    ] {
        assert!(a.route(&event, &first)?);
        assert!(!b.route(&event, &second)?);
    }
    let newer = ProviderEventRouteIdentity {
        attempt_id: RunAttemptId("attempt:new".into()),
        ..first
    };
    let mut state = ProviderEventRoutingState::new(&newer, None);
    assert!(!state.route(&turn("pt:test", Some("attempt:test"), "turn:test"), &newer)?);
    Ok(())
}

#[test]
fn late_workflow_members_require_the_enrolled_parent_while_existing_children_keep_routing()
-> Result<()> {
    let id = identity();
    let mut state = ProviderEventRoutingState::new(&id, Some(ProviderTurnId("turn:test".into())));
    assert!(state.route(&child("early", "thread:test"), &id)?);
    assert!(state.route(&child("descendant", "early"), &id)?);
    assert!(state.route(&terminal(), &id)?);
    assert!(!state.route(&child("late", "thread:test"), &id)?);
    assert!(!state.route(&child("member", "early"), &id)?);
    assert!(state.route(&message("early", None), &id)?);
    let mut coordinator = agent("early", "run:test");
    coordinator["workflow"] = json!({"phases":[],"agents":[{"index":0,"label":"Reviewer","state":"running","childThreadId":"member"}]});
    let mut foreign = coordinator.clone();
    foreign["threadId"] = "early".into();
    foreign["childThreadId"] = "descendant".into();
    foreign["runId"] = "run:foreign".into();
    assert!(state.route(&ProviderEventKind::Subagent(Box::new(decode(foreign))), &id)?);
    assert!(!state.route(&child("member", "descendant"), &id)?);
    assert!(state.route(
        &ProviderEventKind::Subagent(Box::new(decode(coordinator))),
        &id
    )?);
    assert!(!state.route(&child("member", "thread:test"), &id)?);
    assert!(state.route(&child("member", "early"), &id)?);
    assert!(!state.route(&child("unrelated", "early"), &id)?);
    assert!(
        !state
            .owned_thread_ids
            .contains(&ThreadId("unrelated".into()))
    );
    Ok(())
}

#[test]
fn moved_child_loses_provider_turn_ownership_while_sibling_and_new_run_continue() -> Result<()> {
    let id = identity();
    let mut old = ProviderEventRoutingState::new(&id, Some(ProviderTurnId("turn:test".into())));
    old.owned_thread_ids
        .extend([ThreadId("resumed".into()), ThreadId("sibling".into())]);
    old.owned_provider_thread_ids.extend([
        ProviderThreadId("pt:resumed".into()),
        ProviderThreadId("pt:sibling".into()),
    ]);
    assert!(old.route(&terminal(), &id)?);
    assert!(old.route(&turn("pt:resumed", None, "child-turn"), &id)?);
    let moved = ProviderEventKind::Subagent(Box::new(decode(agent("resumed", "run:new"))));
    assert!(!old.route(&moved, &id)?);
    for event in [
        message("resumed", None),
        turn("pt:resumed", None, "child-turn"),
        turn("pt:resumed", None, "new-child-turn"),
    ] {
        assert!(!old.route(&event, &id)?);
    }
    assert!(old.route(&message("sibling", None), &id)?);
    assert!(old.route(&turn("pt:sibling", None, "sibling-turn"), &id)?);
    let new_id = ProviderEventRouteIdentity {
        run_id: RunId("run:new".into()),
        attempt_id: RunAttemptId("attempt:new".into()),
        ..id
    };
    let mut new = ProviderEventRoutingState::new(&new_id, None);
    assert!(!new.route(&message("resumed", None), &new_id)?);
    assert!(new.route(&moved, &new_id)?);
    assert!(new.route(&message("resumed", None), &new_id)?);
    Ok(())
}

#[test]
fn inherited_background_items_require_exact_thread_run_identity_and_kind_then_retire() -> Result<()>
{
    let id = identity();
    let mut state = ProviderEventRoutingState::new(&id, None);
    state
        .inherited_background_turn_items
        .insert(TurnItemId("item".into()), RunId("run:prior".into()));
    let mut background = item("dynamic_tool", "item");
    background["toolName"] = "background".into();
    background["input"] = json!({});
    background["runId"] = "run:prior".into();
    let event = |value| ProviderEventKind::TurnItem {
        item: Box::new(decode(value)),
        session_scoped: None,
    };
    assert!(state.route(&event(background.clone()), &id)?);
    for (key, value) in [
        ("threadId", "foreign"),
        ("runId", "foreign"),
        ("id", "foreign"),
    ] {
        let mut foreign = background.clone();
        foreign[key] = value.into();
        assert!(!state.route(&event(foreign), &id)?);
    }
    let mut ordinary = background.clone();
    ordinary["type"] = "reasoning".into();
    ordinary["text"] = "thinking".into();
    ordinary["streaming"] = true.into();
    assert!(!state.route(&event(ordinary), &id)?);
    background["status"] = "completed".into();
    assert!(state.route(&event(background.clone()), &id)?);
    assert!(!state.route(&event(background), &id)?);
    Ok(())
}

#[test]
fn selects_live_background_only_from_noncompleted_prior_runs_and_same_native_lineage() -> Result<()>
{
    let id = identity();
    let mut runs = Vec::new();
    let mut items = Vec::new();
    for (ordinal, status) in [
        (1, "interrupted"),
        (2, "failed"),
        (3, "cancelled"),
        (4, "completed"),
        (5, "running"),
    ] {
        let run_id = format!("run:{ordinal}");
        let mut run = fixture("OrchestrationV2Run");
        run["id"] = run_id.clone().into();
        run["threadId"] = "thread:test".into();
        run["ordinal"] = json!(ordinal);
        run["status"] = status.into();
        runs.push(decode(run));
        let mut tool = item("dynamic_tool", &format!("item:{ordinal}"));
        tool["toolName"] = "background".into();
        tool["input"] = json!({});
        tool["runId"] = run_id.into();
        items.push(decode(tool));
    }
    let selected = select_inherited_background_turn_items(
        &id.thread_id,
        &id.provider_thread_id,
        6,
        &runs,
        &items,
    )?;
    assert_eq!(selected.len(), 3);
    for index in 1..=3 {
        assert!(selected.contains_key(&TurnItemId(format!("item:{index}"))));
    }
    assert!(
        select_inherited_background_turn_items(
            &id.thread_id,
            &ProviderThreadId("foreign".into()),
            6,
            &runs,
            &items
        )?
        .is_empty()
    );
    assert!(can_route_related_subagent(
        &OrchestrationV2TurnItemStatus::Completed
    ));
    for status in [
        OrchestrationV2TurnItemStatus::Running,
        OrchestrationV2TurnItemStatus::Interrupted,
        OrchestrationV2TurnItemStatus::Failed,
    ] {
        assert!(!can_route_related_subagent(&status));
    }
    Ok(())
}

pub fn root_state() -> RootRunState {
    let mut run = fixture("OrchestrationV2Run");
    run["id"] = "run:test".into();
    run["threadId"] = "thread:test".into();
    run["ordinal"] = 1.into();
    run["rootNodeId"] = "node:test".into();
    run["activeAttemptId"] = "attempt:test".into();
    run["providerThreadId"] = "pt:test".into();
    run["providerInstanceId"] = "codex".into();
    run["status"] = "running".into();
    run["completedAt"] = Value::Null;
    let mut node = fixture("OrchestrationV2ExecutionNode");
    node["id"] = "node:test".into();
    node["threadId"] = "thread:test".into();
    node["runId"] = "run:test".into();
    node["providerThreadId"] = "pt:test".into();
    node["providerTurnId"] = "turn:test".into();
    node["status"] = "running".into();
    node["completedAt"] = Value::Null;
    let mut attempt = fixture("OrchestrationV2RunAttempt");
    attempt["id"] = "attempt:test".into();
    attempt["runId"] = "run:test".into();
    attempt["rootNodeId"] = "node:test".into();
    attempt["providerThreadId"] = "pt:test".into();
    attempt["providerTurnId"] = "turn:test".into();
    attempt["providerInstanceId"] = "codex".into();
    attempt["status"] = "running".into();
    let mut provider_thread = fixture("OrchestrationV2ProviderThread");
    provider_thread["id"] = "pt:test".into();
    provider_thread["appThreadId"] = "thread:test".into();
    provider_thread["providerSessionId"] = "session:test".into();
    provider_thread["providerInstanceId"] = "codex".into();
    let mut scope = fixture("OrchestrationV2CheckpointScope");
    scope["id"] = "scope:test".into();
    scope["threadId"] = "thread:test".into();
    RootRunState {
        run: decode(run),
        root_node: decode(node),
        attempt: decode(attempt),
        provider_thread: decode(provider_thread),
        checkpoint_scope: decode(scope),
    }
}

#[tokio::test]
async fn root_completion_records_provider_end_separately_and_durably_queues_checkpoint_capture()
-> Result<()> {
    use super::provider_event_ingestor_test::Harness;
    use t3_orchestration::{stores::outbox, testing::TEST_TIME};
    let h = Harness::new().await?;
    let state = root_state();
    h.write("run.created", serde_json::to_value(&state.run)?)
        .await?;
    h.write("run-attempt.created", serde_json::to_value(&state.attempt)?)
        .await?;
    h.write("node.updated", serde_json::to_value(&state.root_node)?)
        .await?;
    h.write(
        "provider-thread.updated",
        serde_json::to_value(&state.provider_thread)?,
    )
    .await?;
    let terminal = ProviderEvent {
        driver: decode(json!("codex")),
        kind: terminal(),
    };
    let decision = terminal_decision(
        &state,
        &terminal,
        &OpenRunOwnedSubagents::default(),
        &h.clock.now(),
        h.ingestor.ids.as_ref(),
        false,
    )?;
    let result = h
        .sink
        .commit(CommitRequest {
            receipt: None,
            guard: CommitGuard::RunCurrent {
                thread_id: ThreadId("thread:test".into()),
                run_id: RunId("run:test".into()),
                attempt_id: RunAttemptId("attempt:test".into()),
                status: OrchestrationV2RunStatus::Running,
            },
            decision,
        })
        .await?;
    assert!(result.committed);
    let projection = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .unwrap();
    assert_eq!(projection.runs[0].status, OrchestrationV2RunStatus::Waiting);
    assert!(projection.runs[0].completed_at.is_none());
    assert_eq!(
        serde_json::to_value(&projection.attempts[0])?["completedAt"],
        TEST_TIME
    );
    assert_eq!(projection.nodes[0].status, ExecutionNodeStatus::Waiting);
    let effect = h
        .database
        .stores
        .read(|db| outbox::get(db, "effect:checkpoint.capture:run:test"))
        .await?
        .unwrap();
    assert_eq!(
        effect.status,
        t3_orchestration::effects::EffectStatus::Pending
    );
    Ok(())
}

#[test]
fn stopped_runs_capture_checkpoints_failed_runs_do_not_and_interrupts_keep_their_reason()
-> Result<()> {
    let state = root_state();
    let ids = t3_orchestration::testing::SequenceIds::new("terminal");
    let now = t3_contracts::wire::WireDateTime(t3_orchestration::testing::TEST_TIME.into());
    for outcome in [
        TerminalOutcome::Interrupted {
            reason: Some("Agent requested stop".into()),
        },
        TerminalOutcome::Cancelled { reason: None },
        TerminalOutcome::Failed {
            failure: Box::new(decode(fixture("OrchestrationV2ProviderFailure"))),
            failure_item_ordinal: 101,
            retry: None,
            retry_started_at: None,
            child_turns_continue: false,
        },
    ] {
        let failed = matches!(outcome, TerminalOutcome::Failed { .. });
        let interrupted = matches!(outcome, TerminalOutcome::Interrupted { .. });
        let event = ProviderEvent {
            driver: decode(json!("codex")),
            kind: ProviderEventKind::Terminal {
                provider_thread_id: ProviderThreadId("pt:test".into()),
                provider_turn_id: ProviderTurnId("turn:test".into()),
                run_ordinal: 1,
                disposition: ThreadDisposition::Broken,
                outcome,
            },
        };
        let decision = terminal_decision(
            &state,
            &event,
            &OpenRunOwnedSubagents::default(),
            &now,
            &ids,
            false,
        )?;
        assert_eq!(decision.effects.len(), usize::from(!failed));
        for event in &decision.events {
            let event = serde_json::to_value(event)?;
            if event["type"] == "run.updated" {
                assert_eq!(event["payload"]["completedAt"], now.0);
            }
            if event["type"] == "provider-thread.updated" {
                assert_eq!(event["payload"]["status"], "error");
            }
            if event["payload"]["type"] == "run_interrupt_result" {
                assert_eq!(event["payload"]["message"], "Agent requested stop");
                assert_eq!(event["payload"]["ordinal"], 198);
            }
        }
        assert_eq!(
            decision
                .events
                .iter()
                .filter(|event| matches!(event, OrchestrationV2DomainEvent::TurnItemUpdated(_)))
                .count(),
            usize::from(failed || interrupted)
        );
    }
    Ok(())
}

#[tokio::test]
async fn cascades_through_lifetime_child_links_unless_the_failed_provider_keeps_children_running()
-> Result<()> {
    use super::provider_event_ingestor_test::Harness;
    for children_continue in [false, true] {
        let h = Harness::new().await?;
        let state = root_state();
        let mut child = fixture("OrchestrationV2AppThread");
        child["id"] = "child".into();
        child["deletedAt"] = Value::Null;
        h.write("thread.created", child).await?;
        let mut finished = agent("child", "run:test");
        finished["status"] = "completed".into();
        let mut child_node = fixture("OrchestrationV2ExecutionNode");
        child_node["id"] = "child-node".into();
        child_node["threadId"] = "child".into();
        child_node["runId"] = Value::Null;
        child_node["status"] = "running".into();
        h.write("node.updated", child_node.clone()).await?;
        let mut child_item = item("assistant_message", "child-text");
        child_item["threadId"] = "child".into();
        child_item["runId"] = Value::Null;
        child_item["messageId"] = "child-message".into();
        child_item["text"] = "Partial text".into();
        child_item["attachments"] = json!([]);
        child_item["streaming"] = true.into();
        h.write("turn-item.updated", child_item.clone()).await?;
        let open = OpenRunOwnedSubagents {
            subagents: vec![decode(finished)],
            nodes: vec![decode(child_node)],
            child_turn_items: vec![decode(child_item)],
            ..Default::default()
        };
        let terminal = ProviderEvent {
            driver: decode(json!("codex")),
            kind: ProviderEventKind::Terminal {
                provider_thread_id: ProviderThreadId("pt:test".into()),
                provider_turn_id: ProviderTurnId("turn:test".into()),
                run_ordinal: 1,
                disposition: ThreadDisposition::Reusable,
                outcome: TerminalOutcome::Failed {
                    failure: Box::new(decode(fixture("OrchestrationV2ProviderFailure"))),
                    failure_item_ordinal: 101,
                    retry: None,
                    retry_started_at: None,
                    child_turns_continue: children_continue,
                },
            },
        };
        let decision = terminal_decision(
            &state,
            &terminal,
            &open,
            &h.clock.now(),
            h.ingestor.ids.as_ref(),
            true,
        )?;
        assert!(
            !decision
                .events
                .iter()
                .any(|event| matches!(event, OrchestrationV2DomainEvent::SubagentUpdated(_)))
        );
        h.sink
            .commit(CommitRequest {
                receipt: None,
                guard: CommitGuard::None,
                decision,
            })
            .await?;
        let item = h
            .reader
            .turn_item(ThreadId("child".into()), TurnItemId("child-text".into()))
            .await?
            .unwrap();
        let raw = serde_json::to_value(item)?;
        assert_eq!(
            raw["status"],
            if children_continue {
                "running"
            } else {
                "failed"
            }
        );
        assert_eq!(raw["streaming"], children_continue);
    }
    Ok(())
}

#[tokio::test]
async fn root_terminal_keeps_ingesting_background_items_until_their_completion() -> Result<()> {
    use super::provider_turn_start_service_test::{policy, seed, service, sessions};
    use std::sync::Arc;
    use t3_contracts::application_event::ApplicationStoredEvent;
    use tokio_util::sync::CancellationToken;
    let h = super::provider_event_ingestor_test::Harness::new().await?;
    seed(&h).await?;
    let (adapter, _calls) = h.fake();
    let adapter = Arc::new(adapter);
    let service = service(&h, sessions(adapter.clone()));
    let handle = service
        .start(
            ThreadId("thread:test".into()),
            RunId("run:test".into()),
            policy(),
            false,
            CancellationToken::new(),
        )
        .await?
        .unwrap();
    let mut events = h.sink.subscribe();
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: adapter.driver(),
            kind: turn("pt:test", Some("attempt:test"), "turn:test"),
        })
        .await?;
    let mut background = item("dynamic_tool", "background");
    background["toolName"] = "background".into();
    background["input"] = json!({});
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: adapter.driver(),
            kind: ProviderEventKind::TurnItem {
                item: Box::new(decode(background.clone())),
                session_scoped: None,
            },
        })
        .await?;
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: adapter.driver(),
            kind: terminal(),
        })
        .await?;
    loop {
        let ApplicationStoredEvent::Orchestration(stored) = events.recv().await.unwrap() else {
            continue;
        };
        if let OrchestrationV2DomainEvent::RunUpdated(event) = &stored.event
            && event.payload.status == OrchestrationV2RunStatus::Waiting
        {
            break;
        }
    }
    h.clock.set("2026-09-03T00:01:00.000Z");
    background["status"] = "completed".into();
    background["completedAt"] = json!(h.clock.now());
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: adapter.driver(),
            kind: ProviderEventKind::TurnItem {
                item: Box::new(decode(background)),
                session_scoped: None,
            },
        })
        .await?;
    handle.settled().await?;
    let saved = h
        .reader
        .turn_item(
            ThreadId("thread:test".into()),
            TurnItemId("background".into()),
        )
        .await?
        .unwrap();
    assert_eq!(serde_json::to_value(saved)?["status"], "completed");
    let projection = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .unwrap();
    assert_eq!(projection.runs[0].status, OrchestrationV2RunStatus::Waiting);
    assert_eq!(
        projection.attempts[0].completed_at.as_ref().unwrap().0,
        t3_orchestration::testing::TEST_TIME
    );
    service.execution.shutdown().await
}

#[tokio::test]
async fn background_completion_between_initial_read_and_subscription_does_not_pin_the_new_run()
-> Result<()> {
    use super::{
        provider_event_ingestor_test::Harness,
        provider_turn_start_service_test::{Sessions, policy, seed, service},
    };
    use std::sync::Arc;
    use t3_contracts::application_event::ApplicationStoredEvent;
    use tokio::sync::{Notify, mpsc};
    use tokio_util::sync::CancellationToken;
    let h = Harness::new().await?;
    let mut state = seed(&h).await?;
    state.run.ordinal = 2;
    h.write("run.updated", t3_contracts::stored::to_value(&state.run)?)
        .await?;
    let mut previous = t3_contracts::stored::to_value(&state.run)?;
    previous["id"] = "run:prior".into();
    previous["ordinal"] = 1.into();
    previous["status"] = "interrupted".into();
    h.write("run.created", previous).await?;
    let mut background = item("command_execution", "old-background");
    background["runId"] = "run:prior".into();
    background["input"] = "sleep 60".into();
    h.write("turn-item.updated", background.clone()).await?;
    let (adapter, _calls) = h.fake();
    let adapter = Arc::new(adapter);
    let (opening, mut opened) = mpsc::unbounded_channel();
    let resume = Arc::new(Notify::new());
    let start = service(
        &h,
        Arc::new(Sessions {
            adapter: adapter.clone(),
            failure: None,
            opening: Some(opening),
            resume_open: Some(resume.clone()),
        }),
    );
    let completion = async {
        opened.recv().await.unwrap();
        background["status"] = "completed".into();
        background["completedAt"] = json!(h.clock.now());
        h.write("turn-item.updated", background).await?;
        resume.notify_one();
        Ok::<_, t3_orchestration::error::Error>(())
    };
    let (handle, completed) = tokio::join!(
        start.start(
            ThreadId("thread:test".into()),
            RunId("run:test".into()),
            policy(),
            false,
            CancellationToken::new()
        ),
        completion
    );
    completed?;
    let handle = handle?.unwrap();
    let mut events = h.sink.subscribe();
    let mut turn = fixture("OrchestrationV2ProviderTurn");
    turn["id"] = "turn:test".into();
    turn["providerThreadId"] = "pt:test".into();
    turn["nodeId"] = "node:test".into();
    turn["runAttemptId"] = "attempt:test".into();
    turn["status"] = "completed".into();
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: adapter.driver(),
            kind: ProviderEventKind::Turn {
                provider_turn: Box::new(decode(turn)),
                thread_id: None,
            },
        })
        .await?;
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: adapter.driver(),
            kind: ProviderEventKind::Terminal {
                provider_thread_id: ProviderThreadId("pt:test".into()),
                provider_turn_id: ProviderTurnId("turn:test".into()),
                run_ordinal: 2,
                disposition: ThreadDisposition::Reusable,
                outcome: TerminalOutcome::Completed { reason: None },
            },
        })
        .await?;
    let mut barrier = item("assistant_message", "after-settlement");
    barrier["messageId"] = "barrier-message".into();
    barrier["text"] = "Must not be read after settlement".into();
    barrier["attachments"] = json!([]);
    barrier["streaming"] = false.into();
    barrier["status"] = "completed".into();
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: adapter.driver(),
            kind: ProviderEventKind::TurnItem {
                item: Box::new(decode(barrier)),
                session_scoped: None,
            },
        })
        .await?;
    let unexpected = async {
        loop {
            if let ApplicationStoredEvent::Orchestration(stored) = events.recv().await.unwrap()
                && let OrchestrationV2DomainEvent::TurnItemUpdated(event) = stored.event
                && t3_contracts::stored::to_value(&event.payload).unwrap()["id"]
                    == "after-settlement"
            {
                break;
            }
        }
    };
    let result = tokio::select! {result=handle.settled()=>result,_=unexpected=>Err(t3_orchestration::error::Error::Unsupported("stale inherited work kept ingestion open"))};
    start.execution.shutdown().await?;
    result
}

#[tokio::test]
async fn ingestion_failure_stops_the_native_turn_and_repairs_its_last_observed_snapshot()
-> Result<()> {
    use super::{
        provider_event_ingestor_test::{Harness, InterceptCommitter},
        provider_turn_start_service_test::{policy, seed, service, sessions},
    };
    use std::sync::Arc;
    use t3_contracts::application_event::ApplicationStoredEvent;
    use t3_orchestration::{error::Error, testing::FakeCall};
    use tokio_util::sync::CancellationToken;
    for status in ["running", "completed", "failed", "terminal-ingest"] {
        let h = Harness::new().await?;
        seed(&h).await?;
        let (adapter, mut calls) = h.fake();
        let adapter = Arc::new(adapter);
        let mut start = service(&h, sessions(adapter.clone()));
        let ingestor =
            Arc::get_mut(&mut Arc::get_mut(&mut start.execution).unwrap().ingestor).unwrap();
        let intercepted = InterceptCommitter::new(ingestor.committer.clone());
        ingestor.committer = intercepted.clone();
        let handle = start
            .start(
                ThreadId("thread:test".into()),
                RunId("run:test".into()),
                policy(),
                false,
                CancellationToken::new(),
            )
            .await?
            .unwrap();
        let mut events = h.sink.subscribe();
        let mut turn = fixture("OrchestrationV2ProviderTurn");
        turn["id"] = "turn:broken".into();
        turn["providerThreadId"] = "pt:test".into();
        turn["runAttemptId"] = "attempt:test".into();
        turn["nodeId"] = "node:test".into();
        turn["status"] = if status == "terminal-ingest" {
            "completed"
        } else {
            status
        }
        .into();
        turn["completedAt"] = if status == "running" {
            Value::Null
        } else {
            json!(h.clock.now())
        };
        let failure = || Error::Service {
            operation: "publish",
            detail: "(SQLITE_CANTOPEN) unable to open database file".into(),
        };
        if status != "terminal-ingest" {
            intercepted.fail_next(failure());
        }
        h.clock.set("2026-09-03T00:00:01.000Z");
        adapter
            .runtime
            .emit(ProviderEvent {
                driver: adapter.driver(),
                kind: ProviderEventKind::Turn {
                    provider_turn: Box::new(decode(turn)),
                    thread_id: None,
                },
            })
            .await?;
        if status == "terminal-ingest" {
            loop {
                if let ApplicationStoredEvent::Orchestration(stored) = events.recv().await.unwrap()
                    && matches!(
                        stored.event,
                        OrchestrationV2DomainEvent::ProviderTurnUpdated(_)
                    )
                {
                    break;
                }
            }
            intercepted.fail_next(failure());
            adapter
                .runtime
                .emit(ProviderEvent {
                    driver: adapter.driver(),
                    kind: ProviderEventKind::Terminal {
                        provider_thread_id: ProviderThreadId("pt:test".into()),
                        provider_turn_id: ProviderTurnId("turn:broken".into()),
                        run_ordinal: 1,
                        disposition: ThreadDisposition::Reusable,
                        outcome: TerminalOutcome::Completed { reason: None },
                    },
                })
                .await?;
        }
        handle.settled().await?;
        start.execution.shutdown().await?;
        let mut interrupts = vec![];
        while let Ok(call) = calls.try_recv() {
            if let FakeCall::Interrupt(input) = call {
                interrupts.push(input);
            }
        }
        assert_eq!(interrupts.len(), 1, "{status}");
        assert_eq!(interrupts[0].provider_turn_id.0, "turn:broken");
        assert!(interrupts[0].request_runtime_restart);
        let projection = h
            .reader
            .projection(ThreadId("thread:test".into()))
            .await?
            .unwrap();
        assert_eq!(projection.runs[0].status, OrchestrationV2RunStatus::Failed);
        let repaired = projection
            .provider_turns
            .iter()
            .find(|turn| t3_contracts::stored::to_value(&turn.id).unwrap() == "turn:broken")
            .expect("native turn snapshot must be repaired after publication failure");
        assert_eq!(
            t3_contracts::stored::to_value(&repaired.status)?,
            if status == "running" {
                "interrupted"
            } else if status == "terminal-ingest" {
                "completed"
            } else {
                status
            }
        );
        assert_eq!(
            repaired.completed_at.as_ref().unwrap().0,
            if status == "running" {
                "2026-09-03T00:00:01.000Z"
            } else {
                "2026-09-03T00:00:00.000Z"
            }
        );
        let failure = projection
            .turn_items
            .iter()
            .find_map(|item| {
                if let OrchestrationV2TurnItem::Error(item) = item {
                    Some(item)
                } else {
                    None
                }
            })
            .unwrap();
        assert!(
            failure.failure.message.0.contains("SQLITE_CANTOPEN"),
            "{status}: {:?}",
            failure.failure
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_clean_provider_stream_without_a_root_terminal_fails_the_run_and_closes_its_attempt()
-> Result<()> {
    use super::provider_turn_start_service_test::{policy, seed, service, sessions};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;
    let h = super::provider_event_ingestor_test::Harness::new().await?;
    seed(&h).await?;
    let (adapter, _calls) = h.fake();
    let adapter = Arc::new(adapter);
    let service = service(&h, sessions(adapter.clone()));
    let handle = service
        .start(
            ThreadId("thread:test".into()),
            RunId("run:test".into()),
            policy(),
            false,
            CancellationToken::new(),
        )
        .await?
        .unwrap();
    adapter.runtime.close().await?;
    handle.settled().await?;
    let projection = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .unwrap();
    assert_eq!(projection.runs[0].status, OrchestrationV2RunStatus::Failed);
    assert_eq!(projection.attempts[0].status, RunAttemptStatus::Failed);
    let failure = projection
        .turn_items
        .iter()
        .find_map(|item| match item {
            OrchestrationV2TurnItem::Error(item) => Some(item),
            _ => None,
        })
        .unwrap();
    assert!(
        failure
            .failure
            .message
            .0
            .contains("ended without a root terminal")
    );
    assert!(
        h.database
            .stores
            .read(|db| t3_orchestration::stores::outbox::get(
                db,
                "effect:checkpoint.capture:run:test"
            ))
            .await?
            .is_none()
    );
    service.execution.shutdown().await
}

#[tokio::test]
async fn unsupported_compaction_fails_durably_without_starting_a_regular_turn() -> Result<()> {
    use super::provider_turn_start_service_test::{policy, seed, service, sessions};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;
    let h = super::provider_event_ingestor_test::Harness::new().await?;
    let state = seed(&h).await?;
    let mut message = h
        .reader
        .run_message(ThreadId("thread:test".into()), RunId("run:test".into()))
        .await?
        .unwrap();
    let mut previous = message.clone();
    previous.id = t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(MessageId(
        "message:previous".into(),
    ));
    previous.run_id = None;
    h.write(
        "message.updated",
        t3_contracts::stored::to_value(&previous)?,
    )
    .await?;
    message.text = "  /COMPACT  ".into();
    h.write("message.updated", json!(message)).await?;
    let (adapter, mut calls) = h.fake();
    let service = service(&h, sessions(Arc::new(adapter)));
    let handle = service
        .start(
            ThreadId("thread:test".into()),
            RunId("run:test".into()),
            policy(),
            false,
            CancellationToken::new(),
        )
        .await?
        .unwrap();
    handle.settled().await?;
    let projection = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .unwrap();
    assert_eq!(projection.runs[0].id, state.run.id);
    assert_eq!(projection.runs[0].status, OrchestrationV2RunStatus::Failed);
    assert!(projection.turn_items.iter().any(|item| matches!(item, OrchestrationV2TurnItem::Error(item) if item.failure.message.0.contains("compact"))));
    while let Ok(call) = calls.try_recv() {
        assert!(!matches!(
            call,
            t3_orchestration::testing::FakeCall::Start(_)
        ));
    }
    service.execution.shutdown().await
}

#[tokio::test]
async fn workspace_handoff_completes_the_run_unless_a_persisted_stop_request_takes_precedence()
-> Result<()> {
    use super::provider_turn_start_service_test::{policy, seed, service, sessions};
    use std::sync::Arc;
    use t3_orchestration::ports::runs_ingestion::provider_event_ingestor::derive_id;
    use tokio_util::sync::CancellationToken;
    for stop in [false, true] {
        let h = super::provider_event_ingestor_test::Harness::new().await?;
        seed(&h).await?;
        let (adapter, _calls) = h.fake();
        let adapter = Arc::new(adapter);
        let service = service(&h, sessions(adapter.clone()));
        let handle = service
            .start(
                ThreadId("thread:test".into()),
                RunId("run:test".into()),
                policy(),
                false,
                CancellationToken::new(),
            )
            .await?
            .unwrap();
        let mut thread = h
            .reader
            .thread(ThreadId("thread:test".into()))
            .await?
            .unwrap();
        thread.worktree_path = Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
            "C:/handoff".into(),
        ));
        h.write("thread.metadata-updated", json!(thread)).await?;
        if stop {
            let mut request = item(
                "run_interrupt_request",
                &derive_id(
                    "turn-item",
                    &["run", "run:test", "signal", "interrupt-request"],
                ),
            );
            request["message"] = "Stopped by user".into();
            h.write("turn-item.updated", request).await?;
        }
        adapter
            .runtime
            .emit(ProviderEvent {
                driver: adapter.driver(),
                kind: turn("pt:test", Some("attempt:test"), "turn:test"),
            })
            .await?;
        adapter
            .runtime
            .emit(ProviderEvent {
                driver: adapter.driver(),
                kind: ProviderEventKind::Terminal {
                    provider_thread_id: ProviderThreadId("pt:test".into()),
                    provider_turn_id: ProviderTurnId("turn:test".into()),
                    run_ordinal: 1,
                    disposition: ThreadDisposition::Reusable,
                    outcome: TerminalOutcome::Interrupted {
                        reason: Some("Workspace moved".into()),
                    },
                },
            })
            .await?;
        handle.settled().await?;
        let projection = h
            .reader
            .projection(ThreadId("thread:test".into()))
            .await?
            .unwrap();
        assert_eq!(
            projection.runs[0].status,
            if stop {
                OrchestrationV2RunStatus::Interrupted
            } else {
                OrchestrationV2RunStatus::Waiting
            }
        );
        assert_eq!(
            projection.attempts[0].status,
            if stop {
                RunAttemptStatus::Interrupted
            } else {
                RunAttemptStatus::Completed
            }
        );
        let result = projection.turn_items.iter().find_map(|item| match item {
            OrchestrationV2TurnItem::RunInterruptResult(item) => Some(item),
            _ => None,
        });
        assert_eq!(result.is_some(), stop);
        if let Some(result) = result {
            assert_eq!(result.message, "Stopped by user");
        }
        service.execution.shutdown().await?;
    }
    Ok(())
}

struct BaselineGate {
    started: tokio::sync::mpsc::UnboundedSender<()>,
    release: std::sync::Arc<tokio::sync::Notify>,
    captures: std::sync::atomic::AtomicUsize,
    failure: Option<&'static str>,
}
#[async_trait::async_trait]
impl CheckpointBackend for BaselineGate {
    async fn capture(
        &self,
        input: CaptureCheckpoint,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<CheckpointRef> {
        assert_eq!(input.ordinal, 0);
        self.captures
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.started.send(()).expect("test value");
        if let Some(failure) = self.failure {
            return Err(if failure == "cancelled" {
                t3_orchestration::error::Error::Cancelled
            } else {
                t3_orchestration::error::Error::Unsupported("fixture baseline failure")
            });
        }
        tokio::select! {
            _ = cancellation.cancelled() => Err(t3_orchestration::error::Error::Cancelled),
            _ = self.release.notified() => Ok(CheckpointRef("refs/t3/baseline".into())),
        }
    }
    async fn lookup(
        &self,
        _scope: OrchestrationV2CheckpointScope,
        _ordinal: u64,
    ) -> Result<Option<CheckpointRef>> {
        Ok(None)
    }
    async fn diff(
        &self,
        _cwd: std::path::PathBuf,
        _from: CheckpointRef,
        _to: CheckpointRef,
    ) -> Result<CheckpointDiff> {
        unreachable!()
    }
    async fn restore(
        &self,
        _scope: OrchestrationV2CheckpointScope,
        _ordinal: u64,
        _cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        unreachable!()
    }
    async fn prune(
        &self,
        _scope: OrchestrationV2CheckpointScope,
        _preserve: Vec<u64>,
    ) -> Result<()> {
        unreachable!()
    }
}

#[tokio::test]
async fn forked_baseline_overlaps_session_open_but_provider_prompt_waits_for_capture() -> Result<()>
{
    use super::provider_turn_start_service_test::{Sessions, finish, policy, seed, service};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::{Notify, mpsc};
    use tokio_util::sync::CancellationToken;
    let h = super::provider_event_ingestor_test::Harness::new().await?;
    seed(&h).await?;
    let (adapter, mut calls) = h.fake();
    let adapter = Arc::new(adapter);
    let (started, mut capturing) = mpsc::unbounded_channel();
    let release = Arc::new(Notify::new());
    let checkpoint = Arc::new(BaselineGate {
        started,
        release: release.clone(),
        captures: AtomicUsize::new(0),
        failure: None,
    });
    let (opening, mut opened) = mpsc::unbounded_channel();
    let resume_open = Arc::new(Notify::new());
    let mut start = service(
        &h,
        Arc::new(Sessions {
            adapter: adapter.clone(),
            failure: None,
            opening: Some(opening),
            resume_open: Some(resume_open.clone()),
        }),
    );
    Arc::get_mut(&mut start.execution).unwrap().checkpoints = Some(checkpoint.clone());
    let start = Arc::new(start);
    let startup = {
        let start = start.clone();
        tokio::spawn(async move {
            start
                .start(
                    ThreadId("thread:test".into()),
                    RunId("run:test".into()),
                    policy(),
                    false,
                    CancellationToken::new(),
                )
                .await
        })
    };
    tokio::select! {
        biased;
        result=capturing.recv()=>assert_eq!(result,Some(())),
        _=opened.recv()=>panic!("session preparation began before the baseline was forked"),
    }
    assert_eq!(opened.recv().await, Some(()));
    resume_open.notify_one();
    // The committed running run proves session preparation finished while capture is held.
    let mut events = h.sink.subscribe();
    loop {
        if let t3_contracts::application_event::ApplicationStoredEvent::Orchestration(event) =
            events.recv().await.unwrap()
            && let OrchestrationV2DomainEvent::RunUpdated(event) = event.event
            && event.payload.status == OrchestrationV2RunStatus::Running
        {
            break;
        }
    }
    while let Ok(call) = calls.try_recv() {
        assert!(!matches!(
            call,
            t3_orchestration::testing::FakeCall::Start(_)
        ));
    }
    release.notify_one();
    let handle = startup.await.unwrap()?.unwrap();
    assert_eq!(checkpoint.captures.load(Ordering::SeqCst), 1);
    finish(&adapter, &h, handle).await?;
    start.execution.shutdown().await
}

struct StreamSession {
    inner: std::sync::Arc<t3_orchestration::testing::FakeSessionRuntime>,
    events: std::sync::Mutex<Option<tokio::sync::mpsc::Receiver<ProviderResult<ProviderEvent>>>>,
}
#[async_trait::async_trait]
impl SessionRuntime for StreamSession {
    fn session(&self) -> &OrchestrationV2ProviderSession {
        self.inner.session()
    }
    fn can_resume_after_restart(&self) -> bool {
        self.inner.can_resume_after_restart()
    }
    fn background_tasks_end_with_process(&self) -> bool {
        true
    }
    async fn subscribe(&self) -> ProviderResult<EventSubscription> {
        Ok(EventSubscription {
            events: self
                .events
                .lock()
                .expect("test value")
                .take()
                .expect("test value"),
            cancellation: tokio_util::sync::CancellationToken::new(),
        })
    }
    async fn close(&self) -> ProviderResult<()> {
        self.inner.close().await
    }
    async fn ensure_thread(
        &self,
        input: EnsureThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.inner.ensure_thread(input).await
    }
    async fn resume_thread(
        &self,
        input: ResumeThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.inner.resume_thread(input).await
    }
    async fn start_turn(&self, input: TurnInput) -> ProviderResult<()> {
        self.inner.start_turn(input).await
    }
    async fn steer_turn(&self, input: SteerInput) -> ProviderResult<()> {
        self.inner.steer_turn(input).await
    }
    async fn interrupt_turn(&self, input: InterruptInput) -> ProviderResult<()> {
        self.inner.interrupt_turn(input).await
    }
    async fn respond(&self, input: RuntimeResponse) -> ProviderResult<()> {
        self.inner.respond(input).await
    }
    async fn snapshot(
        &self,
        input: OrchestrationV2ProviderThread,
    ) -> ProviderResult<ThreadSnapshot> {
        self.inner.snapshot(input).await
    }
    async fn rollback(&self, input: RollbackInput) -> ProviderResult<ThreadSnapshot> {
        self.inner.rollback(input).await
    }
    async fn fork(&self, input: ForkInput) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.inner.fork(input).await
    }
}
struct StreamDirectory(std::sync::Arc<StreamSession>);
#[async_trait::async_trait]
impl SessionDirectory for StreamDirectory {
    async fn get(
        &self,
        _id: ProviderSessionId,
    ) -> Result<Option<std::sync::Arc<dyn SessionRuntime>>> {
        Ok(Some(self.0.clone()))
    }
    async fn attach(
        &self,
        _instance: t3_contracts::provider_instance::ProviderInstanceId,
        _input: OpenSession,
    ) -> Result<std::sync::Arc<dyn SessionRuntime>> {
        Ok(self.0.clone())
    }
    async fn detach(&self, _id: ProviderSessionId, _revoke: bool) -> Result<()> {
        self.0.close().await?;
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        self.0.close().await?;
        Ok(())
    }
}
struct ProvedDetach;
impl ProviderStreamContext for ProvedDetach {
    fn detached_thread_id(&self, error: &ProviderError) -> Option<ThreadId> {
        if let ProviderError::Protocol {
            payload: Some(payload),
            ..
        } = error
        {
            payload["detachedThreadId"]
                .as_str()
                .map(|id| ThreadId(id.into()))
        } else {
            None
        }
    }
}
#[tokio::test]
async fn detached_workspace_handoff_settles_without_interrupt_but_unmarked_stream_loss_still_fails()
-> Result<()> {
    use super::provider_turn_start_service_test::{policy, seed, service};
    use std::sync::{Arc, Mutex};
    use tokio_util::sync::CancellationToken;
    for mode in [
        "detached-moved",
        "wrong-thread",
        "unmarked",
        "ordinary-eof",
        "unchanged",
        "typed-detached-moved",
        "typed-wrong-thread",
        "typed-unmarked",
        "typed-unchanged",
        "typed-stop",
    ] {
        let h = super::provider_event_ingestor_test::Harness::new().await?;
        seed(&h).await?;
        let (adapter, mut calls) = h.fake();
        let (sender, events) = tokio::sync::mpsc::channel(8);
        let runtime = Arc::new(StreamSession {
            inner: adapter.runtime.clone(),
            events: Mutex::new(Some(events)),
        });
        let mut start = service(&h, Arc::new(StreamDirectory(runtime)));
        if !mode.starts_with("typed-") {
            start.execution = Arc::new(
                (*start.execution)
                    .clone()
                    .with_stream_context(Arc::new(ProvedDetach)),
            );
        }
        let handle = start
            .start(
                ThreadId("thread:test".into()),
                RunId("run:test".into()),
                policy(),
                false,
                CancellationToken::new(),
            )
            .await?
            .unwrap();
        let mut published = h.sink.subscribe();
        sender
            .send(Ok(ProviderEvent {
                driver: adapter.driver(),
                kind: turn("pt:test", Some("attempt:test"), "turn:test"),
            }))
            .await
            .unwrap();
        loop {
            if let t3_contracts::application_event::ApplicationStoredEvent::Orchestration(stored) =
                published.recv().await.unwrap()
                && let OrchestrationV2DomainEvent::ProviderTurnUpdated(event) = stored.event
                && event.payload.status == ProviderTurnStatus::Running
            {
                break;
            }
        }
        if !mode.ends_with("unchanged") {
            let mut thread = h
                .reader
                .thread(ThreadId("thread:test".into()))
                .await?
                .unwrap();
            thread.worktree_path = Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "C:/handoff".into(),
            ));
            h.write(
                "thread.metadata-updated",
                t3_contracts::stored::to_value(&thread)?,
            )
            .await?;
        }
        if mode == "typed-stop" {
            let id = t3_orchestration::ports::runs_ingestion::provider_event_ingestor::derive_id(
                "turn-item",
                &["run", "run:test", "signal", "interrupt-request"],
            );
            let mut request = item("run_interrupt_request", &id);
            request["message"] = "Stopped by user".into();
            h.write("turn-item.updated", request).await?;
        }
        if mode != "ordinary-eof" {
            let error = if mode.starts_with("typed-") {
                ProviderError::EventStream {
                    provider_session_id: ProviderSessionId("session:test".into()),
                    detached_thread_id: if mode.ends_with("unmarked") {
                        None
                    } else {
                        Some(ThreadId(
                            if mode.ends_with("wrong-thread") {
                                "thread:other"
                            } else {
                                "thread:test"
                            }
                            .into(),
                        ))
                    },
                    detail: "Session closed during detach".into(),
                }
            } else {
                ProviderError::Protocol {
                    reason: None,
                    detail: "Session closed during detach".into(),
                    payload: if mode == "unmarked" {
                        None
                    } else {
                        Some(
                            json!({"detachedThreadId":if mode=="wrong-thread" {"thread:other"} else {"thread:test"}}),
                        )
                    },
                }
            };
            sender.send(Err(error)).await.unwrap();
        }
        drop(sender);
        handle.settled().await?;
        let projection = h
            .reader
            .projection(ThreadId("thread:test".into()))
            .await?
            .unwrap();
        let handed_off = matches!(mode, "detached-moved" | "typed-detached-moved");
        assert_eq!(
            projection.runs[0].status,
            if handed_off {
                OrchestrationV2RunStatus::Waiting
            } else if mode == "typed-stop" {
                OrchestrationV2RunStatus::Interrupted
            } else {
                OrchestrationV2RunStatus::Failed
            },
            "{mode}"
        );
        assert_eq!(
            projection.provider_turns[0].status,
            ProviderTurnStatus::Interrupted,
            "{mode}"
        );
        if handed_off {
            assert!(!projection.turn_items.iter().any(|item| matches!(
                item,
                OrchestrationV2TurnItem::Error(_) | OrchestrationV2TurnItem::RunInterruptResult(_)
            )));
        }
        let mut interrupted = false;
        while let Ok(call) = calls.try_recv() {
            interrupted |= matches!(call, t3_orchestration::testing::FakeCall::Interrupt(_));
        }
        assert_eq!(interrupted, !(handed_off || mode == "typed-stop"), "{mode}");
        if mode == "typed-stop" {
            assert!(projection.turn_items.iter().any(|item| matches!(item, OrchestrationV2TurnItem::RunInterruptResult(item) if item.message == "Stopped by user")));
        }
        start.execution.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn forked_baselines_skip_side_chats_continue_after_capture_failure_and_propagate_interruption()
-> Result<()> {
    use super::provider_turn_start_service_test::{finish, policy, seed, service, sessions};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio_util::sync::CancellationToken;
    for mode in ["failure", "side-chat", "cancelled"] {
        let h = super::provider_event_ingestor_test::Harness::new().await?;
        seed(&h).await?;
        if mode == "side-chat" {
            let mut thread = h
                .reader
                .thread(ThreadId("thread:test".into()))
                .await?
                .unwrap();
            thread.lineage.side_chat = Some(Some(true));
            h.write(
                "thread.metadata-updated",
                t3_contracts::stored::to_value(&thread)?,
            )
            .await?;
        }
        let (adapter, mut calls) = h.fake();
        let adapter = Arc::new(adapter);
        let (started, _capturing) = tokio::sync::mpsc::unbounded_channel();
        let checkpoint = Arc::new(BaselineGate {
            started,
            release: Arc::new(tokio::sync::Notify::new()),
            captures: AtomicUsize::new(0),
            failure: Some(mode),
        });
        let mut start = service(&h, sessions(adapter.clone()));
        Arc::get_mut(&mut start.execution).unwrap().checkpoints = Some(checkpoint.clone());
        let result = start
            .start(
                ThreadId("thread:test".into()),
                RunId("run:test".into()),
                policy(),
                false,
                CancellationToken::new(),
            )
            .await;
        if mode == "cancelled" {
            assert!(matches!(
                result,
                Err(t3_orchestration::error::Error::Cancelled)
            ));
        } else {
            finish(&adapter, &h, result?.unwrap()).await?;
        }
        let mut prompts = 0;
        while let Ok(call) = calls.try_recv() {
            prompts += usize::from(matches!(
                call,
                t3_orchestration::testing::FakeCall::Start(_)
            ));
        }
        assert_eq!(prompts, usize::from(mode != "cancelled"));
        assert_eq!(
            checkpoint.captures.load(Ordering::SeqCst),
            usize::from(mode != "side-chat")
        );
        start.execution.shutdown().await?;
    }
    Ok(())
}
