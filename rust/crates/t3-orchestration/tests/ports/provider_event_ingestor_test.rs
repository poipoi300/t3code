use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use t3_contracts::application_event::{ApplicationProjectEvent, ApplicationStoredEvent};
use t3_contracts::provider_instance::ProviderInstanceId;
use t3_orchestration::{
    api::*,
    error::Result,
    ports::{
        decisions::thread_command_executor::KeyedLanes, runs_ingestion::provider_event_ingestor::*,
    },
    projections::write::SqliteProjectionWriter,
    provider::*,
    runtime::{commit::EventSink, ingestion::ProviderCommitter, worker::OutboxSignals},
    stores::event_store::{EventFilter, EventStore},
    testing::*,
};
use t3_orchestration::{error::Error, stores::receipts::CommandReceipt};

pub type Reader = t3_orchestration::projections::store::ProjectionStore;

#[derive(Default)]
struct Telemetry {
    completed: Mutex<Vec<CompletedProviderTurn>>,
    refresh: tokio::sync::Notify,
    cancellation: Mutex<Option<tokio_util::sync::CancellationToken>>,
}

#[async_trait]
impl ProviderTelemetry for Telemetry {
    async fn turn_completed(
        &self,
        input: CompletedProviderTurn,
        _cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        self.completed.lock().unwrap().push(input);
        Ok(())
    }
    async fn refresh_usage(
        &self,
        instance_id: ProviderInstanceId,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        assert_eq!(instance_id.0, "codex");
        *self.cancellation.lock().unwrap() = Some(cancellation.clone());
        self.refresh.notify_one();
        cancellation.cancelled().await;
        Ok(())
    }
}

#[derive(Default)]
struct HubResets {
    calls: Mutex<Vec<(String, String, Option<String>)>>,
    endpoints: Mutex<Vec<ProviderInstanceId>>,
}
#[async_trait]
impl UsageLimitResets for HubResets {
    async fn endpoint(&self, instance: ProviderInstanceId) -> Result<Option<String>> {
        self.endpoints.lock().unwrap().push(instance);
        Ok(Some("http://hub.test:8317".into()))
    }
    async fn reset_at(
        &self,
        driver: t3_contracts::provider_instance::ProviderDriverKind,
        endpoint: String,
        model: Option<String>,
    ) -> Result<Option<IsoDateTime>> {
        self.calls.lock().unwrap().push((driver.0, endpoint, model));
        Ok(Some(t3_contracts::wire::WireDateTime(
            "2026-11-13T18:00:00.000Z".into(),
        )))
    }
}

#[tokio::test]
async fn usage_limit_uses_the_failed_run_model_and_matching_hub_before_committed_refresh()
-> Result<()> {
    for mode in ["absent", "null", "reported", "validation", "missing-run"] {
        let mut h = Harness::new().await?;
        super::provider_turn_start_service_test::seed(&h).await?;
        let mut thread = h
            .reader
            .thread(ThreadId("thread:test".into()))
            .await?
            .unwrap();
        thread.model_selection.model = "current-selection".into();
        h.write(
            "thread.metadata-updated",
            t3_contracts::stored::to_value(&thread)?,
        )
        .await?;
        let hub = Arc::new(HubResets::default());
        h.ingestor.usage_limit_resets = Some(hub.clone());
        let telemetry = Arc::new(Telemetry::default());
        h.ingestor.telemetry = Some(telemetry.clone());
        let mut failure =
            json!({"class":"usage_limit","message":"limited","code":null,"retryable":null});
        if mode == "null" {
            failure["resetAt"] = Value::Null;
        }
        if mode == "reported" {
            failure["resetAt"] = "2026-11-13T19:00:00.000Z".into();
        }
        if mode == "validation" {
            failure["class"] = "validation_error".into();
        }
        let mut input = h.input(ProviderEventKind::Terminal {
            provider_thread_id: ProviderThreadId("pt:test".into()),
            provider_turn_id: ProviderTurnId("turn:test".into()),
            run_ordinal: 1,
            disposition: ThreadDisposition::Reusable,
            outcome: TerminalOutcome::Failed {
                failure: Box::new(decode(failure)),
                failure_item_ordinal: 101,
                retry: None,
                retry_started_at: None,
                child_turns_continue: false,
            },
        });
        input.event.driver = decode(json!("cursor"));
        if mode == "missing-run" {
            input.run_id = None;
        }
        let stored = h
            .ingestor
            .ingest_normalized(&input, CommitGuard::None)
            .await?;
        let event = t3_contracts::stored::to_value(&stored.last().unwrap().event)?;
        let expected = if mode == "reported" {
            json!("2026-11-13T19:00:00.000Z")
        } else if mode == "validation" {
            Value::Null
        } else {
            json!("2026-11-13T18:00:00.000Z")
        };
        assert_eq!(event["payload"]["failure"]["resetAt"], expected, "{mode}");
        if mode == "reported" || mode == "validation" {
            assert!(hub.calls.lock().unwrap().is_empty());
            assert!(hub.endpoints.lock().unwrap().is_empty());
        } else {
            assert_eq!(
                *hub.endpoints.lock().unwrap(),
                vec![ProviderInstanceId("codex".into())]
            );
            assert_eq!(
                *hub.calls.lock().unwrap(),
                vec![(
                    "cursor".into(),
                    "http://hub.test:8317".into(),
                    Some(
                        if mode == "missing-run" {
                            "current-selection"
                        } else {
                            "model:test"
                        }
                        .into()
                    )
                )]
            );
        }
        if mode != "validation" {
            telemetry.refresh.notified().await;
        } else {
            assert!(telemetry.cancellation.lock().unwrap().is_none());
        }
        h.ingestor.shutdown().await;
    }
    Ok(())
}

#[tokio::test]
async fn out_of_memory_failures_override_display_and_usage_refresh_starts_only_after_commit()
-> Result<()> {
    let mut h = Harness::new().await?;
    let scope = Arc::new(FakeAgentScope::default());
    scope.oom_killed.push(Ok(true));
    scope.oom_killed.push(Ok(true));
    h.ingestor.agent_scope = Some(scope);
    let telemetry = Arc::new(Telemetry::default());
    h.ingestor.telemetry = Some(telemetry.clone());
    let input = h.input(ProviderEventKind::Terminal {
        provider_thread_id: ProviderThreadId("pt:test".into()),
        provider_turn_id: ProviderTurnId("turn:test".into()),
        run_ordinal: 1,
        disposition: ThreadDisposition::Broken,
        outcome: TerminalOutcome::Failed {
            failure: Box::new(decode(
                json!({"class":"usage_limit","message":"limited","code":null,"retryable":null}),
            )),
            failure_item_ordinal: 101,
            retry: None,
            retry_started_at: None,
            child_turns_continue: false,
        },
    });
    assert!(
        h.ingestor
            .ingest_normalized(
                &input,
                CommitGuard::RunCurrent {
                    thread_id: input.thread_id.clone(),
                    run_id: RunId("missing".into()),
                    attempt_id: RunAttemptId("missing".into()),
                    status: OrchestrationV2RunStatus::Running,
                }
            )
            .await?
            .is_empty()
    );
    assert!(telemetry.cancellation.lock().unwrap().is_none());
    let stored = h
        .ingestor
        .ingest_normalized_with_context(
            &input,
            CommitGuard::None,
            IngestionContext {
                command_id: Some(CommandId("command:ingestion".into())),
                analytics: None,
            },
        )
        .await?;
    telemetry.refresh.notified().await;
    assert_eq!(
        h.sink
            .events_by_command(CommandId("command:ingestion".into()))
            .await?,
        stored
    );
    let OrchestrationV2DomainEvent::TurnItemUpdated(event) = &stored[0].event else {
        panic!("expected error item")
    };
    let OrchestrationV2TurnItem::Error(item) = &event.payload else {
        panic!("expected error item")
    };
    assert_eq!(item.failure.message.0, "Killed: out of memory.");
    assert_eq!(item.failure.code.as_ref().unwrap().0, "oom_kill");
    assert_eq!(
        item.failure.class,
        OrchestrationV2ProviderFailureClass::UsageLimit
    );
    let cancellation = telemetry.cancellation.lock().unwrap().clone().unwrap();
    h.ingestor.shutdown().await;
    assert!(cancellation.is_cancelled());
    Ok(())
}

#[tokio::test]
async fn completed_turn_analytics_dedupe_only_committed_turns_and_keep_the_callers_context()
-> Result<()> {
    let mut h = Harness::new().await?;
    let telemetry = Arc::new(Telemetry::default());
    h.ingestor.telemetry = Some(telemetry.clone());
    let mut turn = fixture("OrchestrationV2ProviderTurn");
    turn["id"] = "turn:analytics".into();
    turn["providerThreadId"] = "pt:test".into();
    turn["status"] = "completed".into();
    turn["tokenUsage"] = json!({"inputTokens":4000,"cachedInputTokens":3000,"outputTokens":100,"usedTokens":4100,"updatedAt":TEST_TIME});
    turn["turnTokenUsage"] = json!({"usageStatus":"complete","usageScope":"main_agent","hasSubagents":false,"inputTokens":40,"cachedInputTokens":30,"outputTokens":10});
    let input = h.input(ProviderEventKind::Turn {
        provider_turn: Box::new(decode(turn)),
        thread_id: None,
    });
    assert!(
        h.ingestor
            .ingest_normalized(
                &input,
                CommitGuard::RunCurrent {
                    thread_id: input.thread_id.clone(),
                    run_id: RunId("missing".into()),
                    attempt_id: RunAttemptId("missing".into()),
                    status: OrchestrationV2RunStatus::Running,
                }
            )
            .await?
            .is_empty()
    );
    assert!(telemetry.completed.lock().unwrap().is_empty());
    for _ in 0..2 {
        h.ingestor
            .ingest_normalized_with_context(
                &input,
                CommitGuard::None,
                IngestionContext {
                    command_id: None,
                    analytics: Some(ProviderTurnAnalyticsContext {
                        run_id: input.run_id.clone(),
                        model_selection: decode(json!({"instanceId":"codex","model":"test-model"})),
                        runtime_mode: None,
                        interaction_mode: None,
                        origin: None,
                        segment: Some(ProviderTurnSegment::Continuation),
                        restart_continuation: Some(true),
                    }),
                },
            )
            .await?;
    }
    {
        let completed = telemetry.completed.lock().unwrap();
        assert_eq!(completed.len(), 1);
        let billed = t3_contracts::stored::to_value(&completed[0].provider_turn)?;
        assert_eq!(billed["tokenUsage"]["inputTokens"], 4000);
        assert_eq!(billed["turnTokenUsage"]["inputTokens"], 40);
        assert_eq!(billed["turnTokenUsage"]["cachedInputTokens"], 30);
        assert_eq!(billed["turnTokenUsage"]["outputTokens"], 10);
        let context = completed[0].context.as_ref().unwrap();
        assert_eq!(context.run_id, input.run_id);
        assert_eq!(context.model_selection.model, "test-model");
        assert!(matches!(
            context.segment,
            Some(ProviderTurnSegment::Continuation)
        ));
        assert_eq!(context.restart_continuation, Some(true));
    }
    let failing = Arc::new(FakeProviderTelemetry::default());
    failing.turn_completed.push(Err(Error::Service {
        operation: "analytics",
        detail: "offline".into(),
    }));
    h.ingestor.telemetry = Some(failing);
    let mut next = input.clone();
    if let ProviderEventKind::Turn { provider_turn, .. } = &mut next.event.kind {
        provider_turn.id = t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(ProviderTurnId(
            "turn:offline-analytics".into(),
        ));
        provider_turn.ordinal = 2;
    }
    assert_eq!(
        h.ingestor
            .ingest_normalized(&next, CommitGuard::None)
            .await?
            .len(),
        1
    );
    h.ingestor.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn analytics_evicts_the_oldest_of_4096_turns_without_refreshing_duplicate_order() -> Result<()>
{
    let mut h = Harness::new().await?;
    let telemetry = Arc::new(Telemetry::default());
    h.ingestor.telemetry = Some(telemetry.clone());
    let mut turn = fixture("OrchestrationV2ProviderTurn");
    turn["providerThreadId"] = "pt:test".into();
    turn["status"] = "completed".into();
    for id in [0, 1, 0].into_iter().chain(2..=4096).chain([0]) {
        turn["id"] = format!("turn:{id}").into();
        turn["ordinal"] = json!(id + 1);
        let input = h.input(ProviderEventKind::Turn {
            provider_turn: Box::new(decode(turn.clone())),
            thread_id: None,
        });
        h.ingestor
            .ingest_normalized(&input, CommitGuard::None)
            .await?;
    }
    {
        let completed = telemetry.completed.lock().unwrap();
        assert_eq!(completed.len(), 4098);
        assert_eq!(
            expanded_id(&completed.last().unwrap().provider_turn.id)?.0,
            "turn:0"
        );
    }
    h.ingestor.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn provider_committer_forwards_provenance_and_prepared_transaction_options() -> Result<()> {
    let h = Harness::new().await?;
    let mut node = fixture("OrchestrationV2ExecutionNode");
    node["id"] = "node:options".into();
    node["threadId"] = "thread:test".into();
    let input = h.input(ProviderEventKind::Node(Box::new(decode(node))));
    let events = h.ingestor.normalize(&input).await?;
    let committer = ProviderCommitter((*h.sink).clone());
    for prepared in [false, true] {
        let command_id = CommandId(format!("command:provenance:{prepared}"));
        let request = CommitRequest {
            receipt: None,
            decision: Decision {
                events: events.clone(),
                ..Default::default()
            },
            guard: CommitGuard::None,
        };
        let options = CommitOptions {
            command_id: Some(command_id.clone()),
            ..Default::default()
        };
        let result = if prepared {
            committer
                .commit_prepared(
                    request,
                    options,
                    Box::new(|_, mut request| {
                        for event in &mut request.decision.events {
                            if let OrchestrationV2DomainEvent::NodeUpdated(event) = event {
                                event.id.0.push_str(":prepared");
                            }
                        }
                        Ok(request)
                    }),
                )
                .await?
        } else {
            committer.commit_with_options(request, options).await?
        };
        assert!(result.committed);
        let replay = committer.events_by_command(command_id).await?;
        assert_eq!(replay, result.events);
        assert_eq!(replay.len(), 1);
    }
    Ok(())
}
pub fn fixture(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/samples")
        .join(format!("{name}.jsonl"));
    serde_json::from_str(
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap()
}
pub fn decode<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}
pub fn item(kind: &str, id: &str) -> Value {
    json!({"type":kind,"id":id,"threadId":"thread:test","runId":"run:test","nodeId":"node:test","providerThreadId":"pt:test","providerTurnId":"turn:test","nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"running","title":null,"startedAt":TEST_TIME,"completedAt":null,"updatedAt":TEST_TIME})
}
pub struct Harness {
    pub database: TestDatabase,
    pub reader: Arc<Reader>,
    pub sink: Arc<EventSink>,
    pub ingestor: ProviderEventIngestor,
    pub clock: Arc<ManualClock>,
}

#[tokio::test]
async fn large_file_change_diff_is_bounded_in_both_sqlite_tables_and_small_changes_stay_exact()
-> Result<()> {
    let h = Harness::new().await?;
    let large = format!("@@ -0,0 +1 @@\n{}", "+generated line\n".repeat(150_000));
    let small = "@@ -1 +1 @@\n-old\n+new\n";
    for (id, ordinal, diff) in [("large", 1, large.as_str()), ("small", 2, small)] {
        let mut change = item("file_change", id);
        change["ordinal"] = ordinal.into();
        change["status"] = "completed".into();
        change["fileName"] = "src/generated.ts".into();
        change["diffStr"] = diff.into();
        h.ingestor
            .ingest_normalized(
                &h.input(ProviderEventKind::TurnItem {
                    item: Box::new(decode(change)),
                    session_scoped: None,
                }),
                CommitGuard::None,
            )
            .await?;
    }
    let (event_bytes,projection_bytes)=h.database.stores.read(|db|Ok(db.query_row("SELECT (SELECT SUM(LENGTH(payload_json)) FROM orchestration_events WHERE event_type='turn-item.updated'),(SELECT SUM(LENGTH(payload_json)) FROM orchestration_v2_projection_turn_items)",[],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?)))?)).await?;
    assert!(large.len() > 2_000_000);
    assert!(event_bytes < 100_000);
    assert!(projection_bytes < 100_000);
    let big = h
        .reader
        .turn_item(ThreadId("thread:test".into()), TurnItemId("large".into()))
        .await?
        .unwrap();
    let big = t3_contracts::stored::to_value(&big)?;
    assert_eq!(big["fileName"], "src/generated.ts");
    let bounded = big["diffStr"].as_str().unwrap();
    assert!(bounded.len() < 40_000);
    assert!(large.starts_with(bounded.split("\n…").next().unwrap()));
    let small_item = h
        .reader
        .turn_item(ThreadId("thread:test".into()), TurnItemId("small".into()))
        .await?
        .unwrap();
    assert_eq!(
        t3_contracts::stored::to_value(&small_item)?["diffStr"],
        small
    );
    Ok(())
}
impl Harness {
    pub async fn new() -> Result<Self> {
        let database = TestDatabase::memory()?;
        Self::with_database(database).await
    }
    pub async fn with_database(database: TestDatabase) -> Result<Self> {
        let clock = Arc::new(ManualClock::new(TEST_TIME));
        let reader = Arc::new(Reader::new(database.stores.clone()));
        let sink = Arc::new(EventSink::new(
            database.stores.clone(),
            Arc::new(SqliteProjectionWriter),
            clock.clone(),
            OutboxSignals::default(),
        ));
        let ingestor = ProviderEventIngestor::new(
            reader.clone(),
            Arc::new(ProviderCommitter((*sink).clone())),
            clock.clone(),
            Arc::new(SequenceIds::new("ingestion")),
            Arc::new(KeyedLanes::default()),
        );
        let harness = Self {
            database,
            reader,
            sink,
            ingestor,
            clock,
        };
        let mut thread = fixture("OrchestrationV2AppThread");
        thread["id"] = "thread:test".into();
        thread["projectId"] = "project:test".into();
        thread["providerInstanceId"] = "codex".into();
        thread["modelSelection"] = json!({"instanceId":"codex","model":"m"});
        thread["activeProviderThreadId"] = Value::Null;
        thread["title"] = "Saved title".into();
        thread["worktreePath"] = Value::Null;
        thread["branch"] = Value::Null;
        thread["deletedAt"] = Value::Null;
        thread["archivedAt"] = Value::Null;
        harness.write("thread.created", thread).await?;
        Ok(harness)
    }
    pub fn input(&self, kind: ProviderEventKind) -> IngestInput {
        IngestInput {
            session_id: ProviderSessionId("session:test".into()),
            instance_id: ProviderInstanceId("codex".into()),
            thread_id: ThreadId("thread:test".into()),
            run_id: Some(RunId("run:test".into())),
            node_id: Some(NodeId("node:test".into())),
            raw_event_id: None,
            event: ProviderEvent {
                driver: decode(json!("codex")),
                kind,
            },
        }
    }
    pub async fn write(
        &self,
        kind: &str,
        payload: Value,
    ) -> Result<Vec<OrchestrationV2StoredEvent>> {
        let input = self.input(ProviderEventKind::BackgroundWork {
            provider_turn_id: ProviderTurnId("turn:test".into()),
            pending: false,
        });
        let event = input.domain_event(
            self.ingestor.ids.as_ref(),
            &self.clock.now(),
            kind,
            payload.clone(),
            json!({"threadId":payload["threadId"]}),
        )?;
        Ok(self
            .sink
            .commit(CommitRequest {
                receipt: None,
                guard: CommitGuard::None,
                decision: Decision {
                    events: vec![event],
                    ..Default::default()
                },
            })
            .await?
            .events)
    }
    pub fn fake(
        &self,
    ) -> (
        FakeProviderAdapter,
        tokio::sync::mpsc::UnboundedReceiver<FakeCall>,
    ) {
        let mut session = fixture("OrchestrationV2ProviderSession");
        session["id"] = "session:test".into();
        let mut thread = fixture("OrchestrationV2ProviderThread");
        thread["id"] = "pt:test".into();
        thread["appThreadId"] = "thread:test".into();
        thread["providerSessionId"] = "session:test".into();
        thread["providerInstanceId"] = "codex".into();
        let session: OrchestrationV2ProviderSession = decode(session);
        FakeProviderAdapter::new(
            ProviderInstanceId("codex".into()),
            decode(json!("codex")),
            session.capabilities.clone(),
            session,
            ThreadSnapshot {
                provider_thread: decode(thread),
                provider_turns: vec![],
                messages: vec![],
                reasoning_message_ids: None,
                runtime_requests: vec![],
                provider_payload: None,
            },
        )
    }
}

#[tokio::test]
async fn fake_events_persist_canonical_rows_and_publish_only_committed_events() -> Result<()> {
    let h = Harness::new().await?;
    let (adapter, mut calls) = h.fake();
    let mut subscription = adapter.runtime.subscribe().await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    let mut published = h.sink.subscribe();
    let mut thread = fixture("OrchestrationV2ProviderThread");
    thread["id"] = "pt:test".into();
    thread["appThreadId"] = "thread:test".into();
    thread["providerSessionId"] = "session:test".into();
    let event = ProviderEvent {
        driver: decode(json!("codex")),
        kind: ProviderEventKind::Thread {
            provider_thread: Box::new(decode(thread)),
            pending_background_work: None,
            session_scoped: None,
            retained_native_turn_ids: None,
        },
    };
    adapter.runtime.emit(event.clone()).await?;
    let input = h.input(subscription.events.recv().await.unwrap()?.kind);
    let stored = h
        .ingestor
        .ingest_normalized(&input, CommitGuard::None)
        .await?;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].sequence, 2);
    let delivered = published.recv().await.unwrap();
    assert_eq!(
        delivered,
        t3_contracts::application_event::ApplicationStoredEvent::Orchestration(Box::new(
            stored[0].clone()
        ))
    );
    let rows = h
        .database
        .stores
        .read(|db| EventStore::new().raw_rows(db, &EventFilter::default()))
        .await?;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].event_type, "provider-thread.updated");
    assert_eq!(rows[1].stream_version, 1);
    assert_eq!(rows[1].actor_kind, "server");
    let provider = h
        .reader
        .records(
            ThreadId("thread:test".into()),
            vec![RecordKind::ProviderThreads],
            RecordFilter::default(),
        )
        .await?;
    assert!(
        matches!(&provider.records[0],ProjectionRecordSet::ProviderThreads(threads) if threads.len()==1)
    );
    subscription.close();
    adapter.runtime.close().await?;
    Ok(())
}

#[tokio::test]
async fn skips_only_nodes_equal_to_the_stored_snapshot_and_keeps_ephemeral_events_out_of_sqlite()
-> Result<()> {
    let h = Harness::new().await?;
    let mut node = fixture("OrchestrationV2ExecutionNode");
    node["id"] = "node:test".into();
    node["threadId"] = "thread:test".into();
    node["runId"] = "run:test".into();
    let input = h.input(ProviderEventKind::Node(Box::new(decode(node.clone()))));
    assert_eq!(
        h.ingestor
            .ingest_normalized(&input, CommitGuard::None)
            .await?
            .len(),
        1
    );
    assert!(
        h.ingestor
            .ingest_normalized(&input, CommitGuard::None)
            .await?
            .is_empty()
    );
    node["status"] = "waiting".into();
    h.write("node.updated", node).await?;
    assert_eq!(
        h.ingestor
            .ingest_normalized(&input, CommitGuard::None)
            .await?
            .len(),
        1
    );
    let before = h.sink.latest_sequence(None).await?;
    for kind in [
        ProviderEventKind::BackgroundWork {
            provider_turn_id: ProviderTurnId("turn:test".into()),
            pending: true,
        },
        ProviderEventKind::CommandOutput {
            thread_id: ThreadId("thread:test".into()),
            item_id: TurnItemId("item".into()),
            chunk: "bytes".into(),
        },
        ProviderEventKind::PromptSuggestion {
            thread_id: ThreadId("thread:test".into()),
            run_id: RunId("run:test".into()),
            text: "next".into(),
        },
    ] {
        assert!(
            h.ingestor
                .ingest_normalized(&h.input(kind), CommitGuard::None)
                .await?
                .is_empty()
        );
    }
    assert_eq!(h.sink.latest_sequence(None).await?, before);
    Ok(())
}

#[tokio::test]
async fn child_creation_is_serialized_and_updates_preserve_saved_metadata() -> Result<()> {
    let h = Harness::new().await?;
    let mut thread = fixture("OrchestrationV2AppThread");
    thread["id"] = "child".into();
    thread["worktreePath"] = Value::Null;
    thread["branch"] = Value::Null;
    thread["title"] = "Offered".into();
    let input = h.input(ProviderEventKind::AppThread {
        app_thread: Box::new(decode(thread.clone())),
        create_only: Some(true),
    });
    let (a, b) = tokio::join!(
        h.ingestor.ingest_normalized(&input, CommitGuard::None),
        h.ingestor.ingest_normalized(&input, CommitGuard::None)
    );
    assert_eq!(a?.len() + b?.len(), 1);
    let mut saved =
        serde_json::to_value(h.reader.thread(ThreadId("child".into())).await?.unwrap())?;
    saved["archivedAt"] = json!(TEST_TIME);
    saved["title"] = "Custom".into();
    h.write("thread.metadata-updated", saved.clone()).await?;
    assert!(
        h.ingestor
            .ingest_normalized(&input, CommitGuard::None)
            .await?
            .is_empty()
    );
    let input = h.input(ProviderEventKind::AppThread {
        app_thread: Box::new(decode(thread)),
        create_only: Some(false),
    });
    assert_eq!(
        h.ingestor
            .ingest_normalized(&input, CommitGuard::None)
            .await?
            .len(),
        1
    );
    let updated = h.reader.thread(ThreadId("child".into())).await?.unwrap();
    assert_eq!(updated.title.0, "Offered");
    assert!(updated.archived_at.is_some());
    let mut response = fixture("OrchestrationV2ConversationMessage");
    response["id"] = "child-response".into();
    response["threadId"] = "child".into();
    response["runId"] = Value::Null;
    response["nodeId"] = "child-root".into();
    response["text"] = "Subagent result".into();
    response["attachments"] = json!([]);
    response["streaming"] = false.into();
    let events = h
        .ingestor
        .normalize(&h.input(ProviderEventKind::Message(Box::new(decode(response)))))
        .await?;
    let OrchestrationV2DomainEvent::MessageUpdated(event) = &events[0] else {
        panic!("child message event")
    };
    assert_eq!(event.thread_id.0, "child");
    assert_eq!(event.payload.text, "Subagent result");
    Ok(())
}

#[tokio::test]
async fn inherited_and_late_background_terminals_keep_the_old_run_and_persist_through_one_router()
-> Result<()> {
    use t3_orchestration::ports::runs_ingestion::run_execution_service::{
        ProviderEventRouteIdentity, ProviderEventRoutingState,
        select_inherited_background_turn_items,
    };
    for status in ["interrupted", "completed"] {
        let h = Harness::new().await?;
        let prior = ProviderEventRouteIdentity {
            thread_id: ThreadId("thread:test".into()),
            run_id: RunId("run:prior".into()),
            attempt_id: RunAttemptId("attempt:prior".into()),
            provider_thread_id: ProviderThreadId("pt:test".into()),
        };
        let current = ProviderEventRouteIdentity {
            run_id: RunId("run:current".into()),
            attempt_id: RunAttemptId("attempt:current".into()),
            ..prior.clone()
        };
        let mut running = item("command_execution", "background:prior");
        running["runId"] = "run:prior".into();
        running["input"] = "sleep 60".into();
        let input = h.input(ProviderEventKind::TurnItem {
            item: Box::new(decode(running.clone())),
            session_scoped: None,
        });
        h.ingestor
            .ingest_normalized(&input, CommitGuard::None)
            .await?;
        let mut previous = fixture("OrchestrationV2Run");
        previous["id"] = "run:prior".into();
        previous["threadId"] = "thread:test".into();
        previous["ordinal"] = 1.into();
        previous["status"] = status.into();
        let mut active = previous.clone();
        active["id"] = "run:current".into();
        active["ordinal"] = 2.into();
        active["status"] = "running".into();
        let inherited = select_inherited_background_turn_items(
            &current.thread_id,
            &current.provider_thread_id,
            2,
            &[decode(previous), decode(active)],
            &[decode(running.clone())],
        )?;
        running["status"] = "completed".into();
        running["completedAt"] = json!(h.clock.now());
        let terminal = ProviderEventKind::TurnItem {
            item: Box::new(decode(running)),
            session_scoped: None,
        };
        let mut new_router = ProviderEventRoutingState::new(&current, None);
        new_router.inherited_background_turn_items = inherited;
        let mut routers = vec![(current, new_router)];
        if status == "completed" {
            routers.push((
                prior.clone(),
                ProviderEventRoutingState::new(&prior, Some(ProviderTurnId("turn:test".into()))),
            ));
        }
        let mut accepted = vec![];
        for (identity, router) in &mut routers {
            if router.route(&terminal, identity)? {
                accepted.push(identity.run_id.clone());
                let mut input = h.input(terminal.clone());
                input.run_id = Some(identity.run_id.clone());
                assert_eq!(
                    h.ingestor
                        .ingest_normalized(&input, CommitGuard::None)
                        .await?
                        .len(),
                    1
                );
            }
        }
        assert_eq!(
            accepted,
            vec![RunId(
                if status == "completed" {
                    "run:prior"
                } else {
                    "run:current"
                }
                .into()
            )]
        );
        let persisted = h
            .reader
            .turn_item(
                ThreadId("thread:test".into()),
                TurnItemId("background:prior".into()),
            )
            .await?
            .unwrap();
        let persisted = t3_contracts::stored::to_value(&persisted)?;
        assert_eq!(persisted["runId"], "run:prior");
        assert_eq!(persisted["threadId"], "thread:test");
        assert_eq!(persisted["status"], "completed");
        let count=h.database.stores.read(|db|Ok(db.query_row("SELECT COUNT(*) FROM orchestration_events WHERE event_type='turn-item.updated' AND json_extract(payload_json,'$.id')='background:prior' AND json_extract(payload_json,'$.status')='completed'",[],|row|row.get::<_,i64>(0))?)).await?;
        assert_eq!(count, 1);
    }
    Ok(())
}

#[tokio::test]
async fn steering_changes_status_only_after_provider_consumption_and_never_revives_cancelled_input()
-> Result<()> {
    let h = Harness::new().await?;
    let mut user = item(
        "user_message",
        &derive_id("turn-item", &["message", "message:test"]),
    );
    user["status"] = "pending".into();
    user["createdBy"] = "user".into();
    user["creationSource"] = "web".into();
    user["messageId"] = "message:test".into();
    user["text"] = "Steer".into();
    user["attachments"] = json!([]);
    user["inputIntent"] = "steer".into();
    h.write("turn-item.updated", user.clone()).await?;
    for (status, expected) in [
        (SteerStatus::Pending, 0),
        (SteerStatus::Completed, 1),
        (SteerStatus::Cancelled, 1),
        (SteerStatus::Completed, 0),
    ] {
        let input = h.input(ProviderEventKind::Steer {
            thread_id: ThreadId("thread:test".into()),
            run_id: RunId("run:test".into()),
            message_id: MessageId("message:test".into()),
            status,
        });
        let events = h
            .ingestor
            .ingest_normalized(&input, CommitGuard::None)
            .await?;
        assert_eq!(events.len(), expected);
    }
    let saved = h
        .reader
        .turn_item(ThreadId("thread:test".into()), decode(user["id"].clone()))
        .await?
        .unwrap();
    assert_eq!(serde_json::to_value(saved)?["status"], "cancelled");
    Ok(())
}

#[tokio::test]
async fn terminal_dismisses_native_questions_but_preserves_message_questions_answers_and_approvals()
-> Result<()> {
    for control in [false, true] {
        let h = Harness::new().await?;
        for (index, key) in [
            "live",
            "lost",
            "message",
            "answered",
            "other-turn",
            "approval",
        ]
        .into_iter()
        .enumerate()
        {
            let request = json!({"id":key,"nodeId":key,"providerTurnId":if key=="other-turn" {"turn:other"} else {"turn:test"},"nativeRequestRef":null,"kind":if key=="approval" {"command"} else {"user_input"},"status":if key=="answered" {"resolved"} else {"pending"},"responseCapability":match key {"message"=>json!({"type":"message"}),"lost"=>json!({"type":"not_resumable","reason":"Ended"}),_=>json!({"type":"live","providerSessionId":"session:test"})},"createdAt":TEST_TIME,"resolvedAt":null});
            let mut node = fixture("OrchestrationV2ExecutionNode");
            node["id"] = key.into();
            node["threadId"] = "thread:test".into();
            node["runId"] = Value::Null;
            node["kind"] = if key == "approval" {
                "approval_request"
            } else {
                "user_input_request"
            }
            .into();
            node["runtimeRequestId"] = key.into();
            node["status"] = "waiting".into();
            let mut question = item(
                if key == "approval" {
                    "approval_request"
                } else {
                    "user_input_request"
                },
                key,
            );
            question["requestId"] = key.into();
            question["questions"] = json!([]);
            question["requestKind"] = "command".into();
            question["providerTurnId"] = request["providerTurnId"].clone();
            question["nodeId"] = key.into();
            question["ordinal"] = json!(index);
            h.write("runtime-request.updated", request).await?;
            h.write("node.updated", node).await?;
            h.write("turn-item.updated", question).await?;
        }
        let kind = if control {
            ProviderEventKind::Terminal {
                provider_thread_id: ProviderThreadId("pt:test".into()),
                provider_turn_id: ProviderTurnId("turn:test".into()),
                run_ordinal: 1,
                disposition: ThreadDisposition::Reusable,
                outcome: TerminalOutcome::Completed { reason: None },
            }
        } else {
            let mut turn = fixture("OrchestrationV2ProviderTurn");
            turn["id"] = "turn:test".into();
            turn["status"] = "interrupted".into();
            ProviderEventKind::Turn {
                provider_turn: Box::new(decode(turn)),
                thread_id: None,
            }
        };
        let events = h
            .ingestor
            .ingest_normalized(&h.input(kind), CommitGuard::None)
            .await?;
        assert_eq!(events.len(), if control { 6 } else { 7 });
        for (key, status) in [
            ("live", "cancelled"),
            ("lost", "cancelled"),
            ("message", "pending"),
            ("answered", "resolved"),
            ("other-turn", "pending"),
            ("approval", "pending"),
        ] {
            let request = h
                .reader
                .runtime_request(ThreadId("thread:test".into()), RuntimeRequestId(key.into()))
                .await?
                .unwrap();
            assert_eq!(serde_json::to_value(request)?["status"], status);
        }
    }
    Ok(())
}

#[tokio::test]
async fn failure_item_uses_native_identity_retry_start_and_usage_limit_title() -> Result<()> {
    let h = Harness::new().await?;
    let mut failure = fixture("OrchestrationV2ProviderFailure");
    failure["class"] = "usage_limit".into();
    failure["message"] = "Quota".into();
    let input = h.input(ProviderEventKind::Terminal {
        provider_thread_id: ProviderThreadId("pt:test".into()),
        provider_turn_id: ProviderTurnId("turn:test".into()),
        run_ordinal: 1,
        disposition: ThreadDisposition::Reusable,
        outcome: TerminalOutcome::Failed {
            failure: Box::new(decode(failure)),
            failure_item_ordinal: 201,
            retry: None,
            retry_started_at: Some(t3_contracts::wire::WireDateTime(
                "2026-09-02T23:59:00.000Z".into(),
            )),
            child_turns_continue: false,
        },
    });
    let stored = h
        .ingestor
        .ingest_normalized(&input, CommitGuard::None)
        .await?;
    assert_eq!(stored.len(), 1);
    let event = serde_json::to_value(&stored[0].event)?;
    assert_eq!(
        event["payload"]["id"],
        "turn-item:provider:codex:native-item:terminal-failure%3Aturn%3Atest"
    );
    assert_eq!(event["payload"]["title"], "Usage limit reached");
    assert_eq!(event["payload"]["ordinal"], 1);
    assert_eq!(event["payload"]["startedAt"], "2026-09-02T23:59:00.000Z");
    assert!(
        h.ingestor
            .ingest_normalized(
                &input,
                CommitGuard::RunCurrent {
                    thread_id: input.thread_id.clone(),
                    run_id: RunId("missing".into()),
                    attempt_id: RunAttemptId("missing".into()),
                    status: OrchestrationV2RunStatus::Running
                }
            )
            .await?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn stores_utf8_bounded_diff_in_the_event_and_projection_without_mutating_the_input()
-> Result<()> {
    let h = Harness::new().await?;
    let mut file = item("file_change", "diff");
    file["fileName"] = "file.rs".into();
    file["diffStr"] = "🦀".repeat(20000).into();
    file["oldStr"] = "a".repeat(40000).into();
    let input = h.input(ProviderEventKind::TurnItem {
        item: Box::new(decode(file.clone())),
        session_scoped: None,
    });
    let stored = h
        .ingestor
        .ingest_normalized(&input, CommitGuard::None)
        .await?;
    let payload = serde_json::to_value(&stored[0].event)?["payload"].clone();
    assert!(payload["diffStr"].as_str().unwrap().len() <= 32768);
    assert!(
        payload["diffStr"]
            .as_str()
            .unwrap()
            .ends_with("\n… output truncated for transport")
    );
    let projection = h
        .reader
        .turn_item(input.thread_id, TurnItemId("diff".into()))
        .await?
        .unwrap();
    assert_eq!(
        serde_json::to_value(projection)?["diffStr"],
        payload["diffStr"]
    );
    let ProviderEventKind::TurnItem { item, .. } = input.event.kind else {
        unreachable!()
    };
    assert_eq!(serde_json::to_value(item)?["diffStr"], file["diffStr"]);
    Ok(())
}

#[tokio::test]
async fn strips_unserved_image_bodies_but_retains_the_first_eight_served_images() -> Result<()> {
    let h = Harness::new().await?;
    let body = "a".repeat(128);
    let mut content = vec![json!({"type":"image","mimeType":"image/png","data":body}); 9];
    content.push(json!({"image":{"type":"base64","media_type":"image/png","data":body}}));
    let mut tool = item("dynamic_tool", "images");
    tool["toolName"] = "view".into();
    tool["input"] = json!({});
    tool["output"] =
        json!({"content":content,"structuredContent":{"type":"image/png","blob":body}});
    let stored = h
        .ingestor
        .ingest_normalized(
            &h.input(ProviderEventKind::TurnItem {
                item: Box::new(decode(tool.clone())),
                session_scoped: None,
            }),
            CommitGuard::None,
        )
        .await?;
    let output = serde_json::to_value(&stored[0].event)?["payload"]["output"].clone();
    for index in 0..8 {
        assert_eq!(output["content"][index]["data"], body);
    }
    assert!(output["content"][8].get("data").is_none());
    assert_eq!(output["content"][8]["sizeBytes"], 96);
    assert!(output["content"][9]["image"].get("data").is_none());
    assert_eq!(output["structuredContent"]["sizeBytes"], 96);
    assert_eq!(tool["output"]["content"][8]["data"], body);
    Ok(())
}

#[tokio::test]
async fn todo_durations_follow_text_identity_fallback_anchors_and_completed_timing() -> Result<()> {
    let h = Harness::new().await?;
    let mut plan = fixture("OrchestrationV2PlanArtifact");
    plan["kind"] = "todo_list".into();
    plan["id"] = "plan:test".into();
    plan["threadId"] = "thread:test".into();
    plan["runId"] = "run:test".into();
    plan["status"] = "draft".into();
    plan["steps"] = json!([{"id":"1","text":"First","status":"running"},{"id":"2","text":"Second","status":"pending"}]);
    h.ingestor
        .ingest_normalized(
            &h.input(ProviderEventKind::Plan(Box::new(decode(plan.clone())))),
            CommitGuard::None,
        )
        .await?;
    h.clock.set("2026-09-03T00:00:01.500Z");
    plan["steps"][0]["status"] = "completed".into();
    plan["steps"][1]["status"] = "running".into();
    let events = h
        .ingestor
        .ingest_normalized(
            &h.input(ProviderEventKind::Plan(Box::new(decode(plan.clone())))),
            CommitGuard::None,
        )
        .await?;
    let saved = serde_json::to_value(&events[0].event)?;
    assert_eq!(saved["payload"]["steps"][0]["durationMs"], 1500);
    assert!(
        saved["payload"]["steps"][0]
            .get("durationAnchorAt")
            .is_none()
    );
    assert_eq!(
        saved["payload"]["steps"][1]["durationAnchorAt"],
        h.clock.now().0
    );
    h.clock.set("2026-09-03T00:00:02.000Z");
    plan["steps"][1]["status"] = "completed".into();
    plan["steps"][1]["text"] = "Replacement".into();
    let events = h
        .ingestor
        .ingest_normalized(
            &h.input(ProviderEventKind::Plan(Box::new(decode(plan)))),
            CommitGuard::None,
        )
        .await?;
    let saved = serde_json::to_value(&events[0].event)?;
    assert_eq!(saved["payload"]["steps"][0]["durationMs"], 1500);
    assert!(saved["payload"]["steps"][1].get("durationMs").is_none());
    Ok(())
}

pub(crate) struct InterceptCommitter {
    inner: Arc<dyn EventCommitter>,
    errors: Mutex<VecDeque<Error>>,
    calls: AtomicUsize,
    gate: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
}
impl InterceptCommitter {
    pub(crate) fn new(inner: Arc<dyn EventCommitter>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            errors: Mutex::new(VecDeque::new()),
            calls: AtomicUsize::new(0),
            gate: Mutex::new(None),
        })
    }
    pub(crate) fn fail_next(&self, error: Error) {
        self.errors.lock().unwrap().push_back(error);
    }
}
#[async_trait]
impl EventCommitter for InterceptCommitter {
    async fn commit(&self, input: CommitRequest) -> Result<CommitResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let gate = self.gate.lock().unwrap().take();
        if let Some((entered, release)) = gate {
            let _ = entered.send(());
            release.await.map_err(|_| Error::Cancelled)?;
        }
        let error = self.errors.lock().unwrap().pop_front();
        if let Some(error) = error {
            return Err(error);
        }
        self.inner.commit(input).await
    }
    async fn commit_project(
        &self,
        event: ApplicationProjectEvent,
        receipt: CommandReceipt,
    ) -> Result<CommandReceipt> {
        self.inner.commit_project(event, receipt).await
    }
    async fn events_by_command(&self, id: CommandId) -> Result<Vec<OrchestrationV2StoredEvent>> {
        self.inner.events_by_command(id).await
    }
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ApplicationStoredEvent> {
        self.inner.subscribe()
    }
    async fn latest_sequence(&self, id: Option<ThreadId>) -> Result<u64> {
        self.inner.latest_sequence(id).await
    }
}
fn busy() -> Error {
    Error::Sql(t3_persistence::rusqlite::Error::SqliteFailure(
        t3_persistence::rusqlite::ffi::Error::new(t3_persistence::rusqlite::ffi::SQLITE_BUSY),
        Some("database busy".into()),
    ))
}

#[tokio::test]
async fn child_branch_recheck_observes_the_committed_child_after_parent_moves_and_contains_host_failures()
-> Result<()> {
    struct BranchHost {
        current: Option<String>,
        failure: &'static str,
        expected: std::path::PathBuf,
        calls: AtomicUsize,
        committer: Arc<InterceptCommitter>,
    }
    #[async_trait]
    impl WorkspaceBackend for BranchHost {
        async fn prepare(
            &self,
            _project: ProjectId,
            _thread: ThreadId,
            _strategy: OrchestrationV2ThreadLaunchWorkspaceStrategy,
            _cancel: tokio_util::sync::CancellationToken,
        ) -> Result<Workspace> {
            Err(Error::Unsupported("unused workspace prepare"))
        }
        async fn remove(
            &self,
            _workspace: Workspace,
            _cancel: tokio_util::sync::CancellationToken,
        ) -> Result<()> {
            Err(Error::Unsupported("unused workspace removal"))
        }
        async fn current_branch(&self, cwd: std::path::PathBuf) -> Result<Option<String>> {
            assert_eq!(cwd, self.expected);
            assert_eq!(self.committer.calls.load(Ordering::SeqCst), 1);
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.failure == "git" {
                return Err(Error::Unsupported("git read failed"));
            }
            if self.failure == "write" {
                self.committer
                    .errors
                    .lock()
                    .unwrap()
                    .push_back(Error::Unsupported("correction write failed"));
            }
            Ok(self.current.clone())
        }
    }
    for (moved, failure, old, current) in [
        (false, "", "t3/abcd1234", Some("generated-branch")),
        (true, "", "t3/abcd1234", Some("generated-branch")),
        (false, "write", "t3/abcd1234", Some("generated-branch")),
        (false, "git", "t3/abcd1234", Some("generated-branch")),
        (false, "", "my-prefix/abcd1234", Some("generated-branch")),
        (false, "", "child-feature", Some("child-feature")),
        (false, "", "t3/abcd1234", None),
    ] {
        let mut h = Harness::new().await?;
        let workspace = tempfile::tempdir()?;
        let mut parent = t3_contracts::stored::to_value(
            &h.reader
                .thread(ThreadId("thread:test".into()))
                .await?
                .unwrap(),
        )?;
        parent["branch"] = old.into();
        parent["worktreePath"] = json!(workspace.path());
        h.write("thread.metadata-updated", parent.clone()).await?;
        let mut child = parent.clone();
        child["id"] = "child:branch".into();
        child["lineage"] = json!({"rootThreadId":"thread:test","parentThreadId":"thread:test","relationshipToParent":"subagent"});
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let committer = Arc::new(InterceptCommitter {
            inner: h.ingestor.committer.clone(),
            errors: Mutex::new(VecDeque::new()),
            calls: AtomicUsize::new(0),
            gate: Mutex::new(Some((entered_tx, release_rx))),
        });
        let host = Arc::new(BranchHost {
            current: current.map(str::to_owned),
            failure,
            expected: workspace.path().into(),
            calls: AtomicUsize::new(0),
            committer: committer.clone(),
        });
        h.ingestor.committer = committer.clone();
        h.ingestor.workspace = Some(host.clone());
        let input = h.input(ProviderEventKind::AppThread {
            app_thread: Box::new(decode(child)),
            create_only: None,
        });
        let rename = async {
            entered_rx.await.unwrap();
            parent["branch"] = if moved {
                "user/checkout"
            } else {
                "generated-branch"
            }
            .into();
            if moved {
                parent["worktreePath"] = json!(workspace.path().with_extension("moved"));
            }
            h.write("thread.metadata-updated", parent).await?;
            release_tx.send(()).unwrap();
            Ok::<_, Error>(())
        };
        let (creation, renamed) = tokio::join!(
            h.ingestor.ingest_normalized(&input, CommitGuard::None),
            rename
        );
        renamed?;
        let stored = creation?;
        assert!(matches!(
            stored[0].event,
            OrchestrationV2DomainEvent::ThreadCreated(_)
        ));
        let corrected = failure.is_empty() && current.is_some_and(|current| current != old);
        let child = h
            .reader
            .thread(ThreadId("child:branch".into()))
            .await?
            .unwrap();
        assert_eq!(
            child.branch.as_ref().unwrap().0,
            if corrected { current.unwrap() } else { old }
        );
        assert_eq!(
            child.worktree_path.as_ref().unwrap().0,
            workspace.path().to_string_lossy()
        );
        assert_eq!(
            committer.calls.load(Ordering::SeqCst),
            1 + usize::from(failure != "git" && current.is_some_and(|current| current != old))
        );
        assert_eq!(host.calls.load(Ordering::SeqCst), 1);
        assert!(
            h.ingestor
                .ingest_normalized(&input, CommitGuard::None)
                .await?
                .is_empty()
        );
        assert_eq!(host.calls.load(Ordering::SeqCst), 1);
    }
    Ok(())
}

#[tokio::test]
async fn provider_writes_retry_busy_twice_for_every_guard_and_stop_on_exhaustion_or_other_errors()
-> Result<()> {
    for route in 0..5 {
        let mut h = Harness::new().await?;
        let failures = if route == 3 {
            vec![busy(), busy(), busy()]
        } else if route == 4 {
            vec![Error::Unsupported("constraint failure")]
        } else {
            vec![busy(), busy()]
        };
        let intercept = Arc::new(InterceptCommitter {
            inner: h.ingestor.committer.clone(),
            errors: Mutex::new(failures.into()),
            calls: AtomicUsize::new(0),
            gate: Mutex::new(None),
        });
        h.ingestor.committer = intercept.clone();
        let mut turn = fixture("OrchestrationV2ProviderTurn");
        turn["id"] = "turn:locked".into();
        turn["status"] = "completed".into();
        let input = h.input(ProviderEventKind::Turn {
            provider_turn: Box::new(decode(turn)),
            thread_id: None,
        });
        let guard = match route {
            1 => CommitGuard::RunCurrent {
                thread_id: input.thread_id.clone(),
                run_id: RunId("missing".into()),
                attempt_id: RunAttemptId("missing".into()),
                status: OrchestrationV2RunStatus::Running,
            },
            2 => CommitGuard::ProviderThreadOwner {
                provider_thread_id: ProviderThreadId("missing".into()),
                run_id: RunId("missing".into()),
                attempt_id: RunAttemptId("missing".into()),
                last_run_ordinal: 1,
            },
            _ => CommitGuard::None,
        };
        let result = h.ingestor.ingest_normalized(&input, guard).await;
        assert_eq!(
            intercept.calls.load(Ordering::SeqCst),
            if route == 4 { 1 } else { 3 }
        );
        if route >= 3 {
            assert!(result.is_err());
        } else {
            assert_eq!(result?.len(), usize::from(route == 0));
        }
        let turns = h
            .reader
            .records(
                input.thread_id,
                vec![RecordKind::ProviderTurns],
                RecordFilter::default(),
            )
            .await?;
        assert!(
            matches!(&turns.records[0],ProjectionRecordSet::ProviderTurns(turns) if turns.len()==usize::from(route==0))
        );
    }
    Ok(())
}

#[tokio::test]
async fn terminal_cancellation_cannot_overwrite_a_user_answer_committed_after_normalization()
-> Result<()> {
    let mut h = Harness::new().await?;
    let request = json!({"id":"question","nodeId":"question","providerTurnId":"turn:test","nativeRequestRef":null,"kind":"user_input","status":"pending","responseCapability":{"type":"live","providerSessionId":"session:test"},"createdAt":TEST_TIME,"resolvedAt":null});
    h.write("runtime-request.updated", request.clone()).await?;
    let mut node = fixture("OrchestrationV2ExecutionNode");
    node["id"] = "question".into();
    node["threadId"] = "thread:test".into();
    node["status"] = "waiting".into();
    h.write("node.updated", node.clone()).await?;
    let mut question = item("user_input_request", "question");
    question["nodeId"] = "question".into();
    question["requestId"] = "question".into();
    question["questions"] = json!([]);
    question["status"] = "waiting".into();
    h.write("turn-item.updated", question.clone()).await?;
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    h.ingestor.committer = Arc::new(InterceptCommitter {
        inner: h.ingestor.committer.clone(),
        errors: Mutex::new(VecDeque::new()),
        calls: AtomicUsize::new(0),
        gate: Mutex::new(Some((entered_tx, release_rx))),
    });
    let input = h.input(ProviderEventKind::Terminal {
        provider_thread_id: ProviderThreadId("pt:test".into()),
        provider_turn_id: ProviderTurnId("turn:test".into()),
        run_ordinal: 1,
        disposition: ThreadDisposition::Reusable,
        outcome: TerminalOutcome::Completed { reason: None },
    });
    let answer = async {
        entered_rx.await.unwrap();
        let mut resolved = request;
        resolved["status"] = "resolved".into();
        resolved["resolvedAt"] = json!(TEST_TIME);
        h.write("runtime-request.updated", resolved).await?;
        node["status"] = "completed".into();
        h.write("node.updated", node).await?;
        question["status"] = "completed".into();
        h.write("turn-item.updated", question).await?;
        release_tx.send(()).unwrap();
        Ok::<_, Error>(())
    };
    let (terminal, answered) = tokio::join!(
        h.ingestor.ingest_normalized(&input, CommitGuard::None),
        answer
    );
    answered?;
    assert!(terminal?.is_empty());
    let request = h
        .reader
        .runtime_request(input.thread_id.clone(), RuntimeRequestId("question".into()))
        .await?
        .unwrap();
    assert_eq!(request.status, RuntimeRequestStatus::Resolved);
    let item = h
        .reader
        .turn_item(input.thread_id, TurnItemId("question".into()))
        .await?
        .unwrap();
    assert_eq!(serde_json::to_value(item)?["status"], "completed");
    Ok(())
}

#[tokio::test]
async fn late_native_subagent_models_update_only_model_selection_and_preserve_matching_options()
-> Result<()> {
    let h = Harness::new().await?;
    let mut thread = fixture("OrchestrationV2AppThread");
    thread["id"] = "child".into();
    thread["modelSelection"] = json!({"instanceId":"codex","model":"old","options":[{"id":"reasoningEffort","value":"xhigh"}]});
    h.write("thread.created", thread).await?;
    let mut agent = fixture("OrchestrationV2Subagent");
    agent["id"] = "agent".into();
    agent["threadId"] = "thread:test".into();
    agent["childThreadId"] = "child".into();
    agent["origin"] = "provider_native".into();
    agent["model"] = "new".into();
    agent
        .as_object_mut()
        .unwrap()
        .shift_remove("modelSelection");
    let input = h.input(ProviderEventKind::Subagent(Box::new(decode(agent.clone()))));
    let stored = h
        .ingestor
        .ingest_normalized(&input, CommitGuard::None)
        .await?;
    assert_eq!(stored.len(), 2);
    assert_eq!(
        h.ingestor
            .ingest_normalized(&input, CommitGuard::None)
            .await?
            .len(),
        1
    );
    let child = h.reader.thread(ThreadId("child".into())).await?.unwrap();
    assert_eq!(
        serde_json::to_value(child.model_selection)?,
        json!({"instanceId":"codex","model":"new"})
    );
    agent["modelSelection"] = json!({"instanceId":"codex","model":"new","options":[{"id":"reasoningEffort","value":"low"}]});
    assert_eq!(
        h.ingestor
            .ingest_normalized(
                &h.input(ProviderEventKind::Subagent(Box::new(decode(agent)))),
                CommitGuard::None
            )
            .await?
            .len(),
        2
    );
    assert_eq!(
        h.ingestor
            .ingest_normalized(&input, CommitGuard::None)
            .await?
            .len(),
        1
    );
    let child = h.reader.thread(ThreadId("child".into())).await?.unwrap();
    assert!(child.model_selection.options.is_some());
    Ok(())
}

#[tokio::test]
async fn native_rewind_retires_only_abandoned_runs_and_checkpoints_without_renumbering_or_rewriting_attempts()
-> Result<()> {
    let h = Harness::new().await?;
    for (ordinal, status, thread, native) in [
        (1, "completed", "foreign", Some("foreign")),
        (2, "completed", "pt:test", Some("kept")),
        (3, "completed", "pt:test", Some("abandoned")),
        (4, "interrupted", "pt:test", None),
        (5, "running", "pt:test", None),
    ] {
        let run_id = format!("run:{ordinal}");
        let node_id = format!("node:{ordinal}");
        let attempt_id = format!("attempt:{ordinal}");
        let turn_id = format!("turn:{ordinal}");
        let mut run = fixture("OrchestrationV2Run");
        run["id"] = run_id.clone().into();
        run["threadId"] = "thread:test".into();
        run["providerThreadId"] = thread.into();
        run["activeAttemptId"] = attempt_id.clone().into();
        run["rootNodeId"] = node_id.clone().into();
        run["status"] = status.into();
        run["ordinal"] = ordinal.into();
        h.write("run.created", run).await?;
        let mut attempt = fixture("OrchestrationV2RunAttempt");
        attempt["id"] = attempt_id.clone().into();
        attempt["runId"] = run_id.clone().into();
        attempt["providerThreadId"] = thread.into();
        attempt["providerTurnId"] = turn_id.clone().into();
        attempt["status"] = status.into();
        h.write("run-attempt.created", attempt).await?;
        let mut turn = fixture("OrchestrationV2ProviderTurn");
        turn["id"] = turn_id.clone().into();
        turn["providerThreadId"] = thread.into();
        turn["runAttemptId"] = attempt_id.into();
        turn["status"] = status.into();
        turn["ordinal"] = ordinal.into();
        turn["nodeId"] = node_id.clone().into();
        turn["nativeTurnRef"] = json!({"driver":"codex","nativeId":native.unwrap_or("synthetic"),"strength":if native.is_some(){"strong"}else{"weak"}});
        h.write("provider-turn.updated", turn).await?;
        let mut node = fixture("OrchestrationV2ExecutionNode");
        node["id"] = node_id.clone().into();
        node["threadId"] = "thread:test".into();
        node["runId"] = run_id.clone().into();
        node["status"] = status.into();
        h.write("node.updated", node).await?;
        if status == "completed" {
            let mut checkpoint = fixture("OrchestrationV2Checkpoint");
            checkpoint["id"] = format!("checkpoint:{ordinal}").into();
            checkpoint["threadId"] = "thread:test".into();
            checkpoint["runId"] = run_id.into();
            checkpoint["nodeId"] = node_id.into();
            checkpoint["ordinalWithinScope"] = ordinal.into();
            checkpoint["status"] = "ready".into();
            h.write("checkpoint.captured", checkpoint).await?;
        }
    }
    let mut thread = fixture("OrchestrationV2ProviderThread");
    thread["id"] = "pt:test".into();
    thread["appThreadId"] = "thread:test".into();
    thread["lastRunOrdinal"] = 5.into();
    let mut input = h.input(ProviderEventKind::Thread {
        provider_thread: Box::new(decode(thread)),
        pending_background_work: None,
        session_scoped: None,
        retained_native_turn_ids: Some(vec!["kept".into()]),
    });
    input.run_id = Some(RunId("run:5".into()));
    let events = h
        .ingestor
        .ingest_normalized(&input, CommitGuard::None)
        .await?;
    assert_eq!(events.len(), 6);
    let projection = h.reader.projection(input.thread_id).await?.unwrap();
    for run in &projection.runs {
        let id = expanded_id(&run.id)?.0;
        let expected = match id.as_str() {
            "run:1" | "run:2" => "completed",
            "run:3" | "run:4" => "rolled_back",
            "run:5" => "running",
            _ => panic!("run"),
        };
        assert_eq!(serde_json::to_value(run)?["status"], expected);
        assert_eq!(
            run.ordinal,
            id.strip_prefix("run:").unwrap().parse::<u64>().unwrap()
        );
    }
    assert_eq!(
        projection
            .attempts
            .iter()
            .find(|attempt| expanded_id(&attempt.id).unwrap().0 == "attempt:3")
            .unwrap()
            .status,
        RunAttemptStatus::Completed
    );
    for checkpoint in projection.checkpoints {
        let raw = serde_json::to_value(checkpoint)?;
        assert_eq!(
            raw["status"],
            if raw["id"] == "checkpoint:3" {
                "stale"
            } else {
                "ready"
            }
        );
    }
    assert_eq!(
        projection
            .provider_threads
            .iter()
            .find(|thread| expanded_id(&thread.id).unwrap().0 == "pt:test")
            .unwrap()
            .last_run_ordinal,
        Some(5)
    );
    Ok(())
}

#[tokio::test]
async fn omitted_tool_output_remains_omitted_after_normalization() -> Result<()> {
    let h = Harness::new().await?;
    let mut tool = item("dynamic_tool", "tool:no-output");
    tool["toolName"] = "command".into();
    tool["input"] = json!({});
    let normalized = h
        .ingestor
        .normalize(&h.input(ProviderEventKind::TurnItem {
            item: Box::new(decode(tool)),
            session_scoped: None,
        }))
        .await?;
    assert!(
        serde_json::to_value(&normalized[0])?["payload"]
            .get("output")
            .is_none()
    );
    Ok(())
}
