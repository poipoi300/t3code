use super::provider_session_manager_test::{fake, sample};
use serde_json::{Value, json};
use std::sync::Arc;
use t3_contracts::{base_schemas::*, orchestration_v2::*, wire::WireDateTime};
use t3_orchestration::provider::SessionRuntime;
use t3_orchestration::{
    api::*,
    effects::*,
    error::Result,
    projections::store::ProjectionStore,
    runtime::{commit::EventSink, sessions::*, worker::OutboxSignals},
    stores::{StoreDatabase, outbox},
    testing::*,
};

pub(super) const RECOVERED_AT: &str = "2026-09-04T00:00:00.000Z";

#[tokio::test]
async fn native_restart_selection_honors_project_opt_out_and_unreadable_settings() -> Result<()> {
    for (global, project_override, unreadable, expected) in [
        (true, None, false, true),
        (false, Some(true), false, true),
        (true, Some(false), false, false),
        (true, None, true, false),
    ] {
        let database = TestDatabase::memory()?;
        let events = Arc::new(sink(database.stores.clone()));
        let mut projection = projection()?;
        projection.provider_threads[0].native_thread_ref = Some(serde_json::from_value(
            json!({"driver":"codex","nativeId":"native:test","strength":"strong"}),
        )?);
        let mut turn: Value = serde_json::from_str(
            include_str!("../../../../fixtures/samples/OrchestrationV2ProviderTurn.jsonl")
                .lines()
                .next()
                .unwrap(),
        )?;
        for (field, value) in [
            ("id", json!("provider-turn:test")),
            ("providerThreadId", json!("provider-thread:test")),
            ("runAttemptId", json!("attempt:test")),
            ("status", json!("running")),
            ("ordinal", json!(1)),
            ("startedAt", json!(TEST_TIME)),
            ("completedAt", Value::Null),
        ] {
            turn[field] = value;
        }
        projection.provider_turns = vec![serde_json::from_value(turn)?];
        seed(&events, &projection).await?;
        let settings = Arc::new(FakeSettingsService::default());
        let mut configured = t3_contracts::settings::ServerSettings {
            continue_threads_after_server_update: global,
            ..Default::default()
        };
        if let Some(enabled) = project_override {
            configured.project_settings_overrides.insert(
                ProjectId("project:test".into()),
                serde_json::from_value(json!({"continueThreadsAfterServerUpdate":enabled}))?,
            );
        }
        for _ in 0..2 {
            settings.get.push(if unreadable {
                Err(t3_orchestration::error::Error::Unsupported(
                    "settings offline",
                ))
            } else {
                Ok(configured.clone())
            });
        }
        let recovery = service(database.stores.clone()).with_restart_settings(settings);
        recovery.prepare_for_shutdown().await?;
        database
            .stores
            .read(move |db| {
                assert_eq!(
                    outbox::get(db, "effect:restart-continuation:run:test")?.is_some(),
                    expected
                );
                Ok(())
            })
            .await?;
        assert_eq!(recovery.recover().await?.terminalized_runs, 1);
        database
            .stores
            .read(move |db| {
                assert_eq!(
                    outbox::get(db, "effect:restart-continuation:run:test")?.is_some(),
                    expected
                );
                Ok(())
            })
            .await?;
    }
    Ok(())
}
pub(super) fn projection() -> Result<OrchestrationV2ThreadProjection> {
    let mut thread = serde_json::to_value(sample::<OrchestrationV2AppThread>(
        "OrchestrationV2AppThread",
    )?)?;
    for (key, value) in [
        ("id", json!("thread:test")),
        ("projectId", json!("project:test")),
        ("title", json!("Recovery")),
        ("providerInstanceId", json!("codex")),
        (
            "modelSelection",
            json!({"instanceId":"codex","model":"test"}),
        ),
        ("runtimeMode", json!("full-access")),
        ("interactionMode", json!("default")),
        (
            "lineage",
            json!({"parentThreadId":null,"relationshipToParent":null,"rootThreadId":"thread:test"}),
        ),
        ("activeProviderThreadId", json!("provider-thread:test")),
        ("createdAt", json!(TEST_TIME)),
        ("updatedAt", json!(TEST_TIME)),
        ("archivedAt", json!(null)),
        ("deletedAt", json!(null)),
        ("settledOverride", json!(null)),
        ("snoozedUntil", json!(null)),
        ("createdBy", json!("user")),
        ("creationSource", json!("web")),
    ] {
        thread[key] = value;
    }
    let run = json!({"id":"run:test","threadId":"thread:test","ordinal":1,"providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"test"},"providerThreadId":"provider-thread:test","userMessageId":"message:test","rootNodeId":"node:test","activeAttemptId":"attempt:test","status":"running","requestedAt":TEST_TIME,"startedAt":TEST_TIME,"completedAt":null,"checkpointId":null,"contextHandoffId":null});
    let attempt = json!({"id":"attempt:test","runId":"run:test","attemptOrdinal":1,"rootNodeId":"node:test","providerInstanceId":"codex","providerThreadId":"provider-thread:test","providerTurnId":"provider-turn:test","reason":"initial","status":"running","startedAt":TEST_TIME,"completedAt":null});
    let node = json!({"id":"node:test","threadId":"thread:test","runId":"run:test","parentNodeId":null,"rootNodeId":"node:test","kind":"root_turn","status":"running","countsForRun":true,"providerThreadId":"provider-thread:test","providerTurnId":"provider-turn:test","nativeItemRef":null,"runtimeRequestId":null,"checkpointScopeId":null,"startedAt":TEST_TIME,"completedAt":null});
    let (adapter, _, _) = fake()?;
    let mut provider_thread = serde_json::to_value(sample::<OrchestrationV2ProviderThread>(
        "OrchestrationV2ProviderThread",
    )?)?;
    for (key, value) in [
        ("id", json!("provider-thread:test")),
        ("driver", json!("codex")),
        ("providerInstanceId", json!("codex")),
        ("providerSessionId", json!("session:test")),
        ("appThreadId", json!("thread:test")),
        ("ownerNodeId", json!(null)),
        ("status", json!("active")),
        ("pendingBackgroundTasks", json!([])),
        ("createdAt", json!(TEST_TIME)),
        ("updatedAt", json!(TEST_TIME)),
    ] {
        provider_thread[key] = value;
    }
    Ok(serde_json::from_value(
        json!({"thread":thread,"runs":[run],"attempts":[attempt],"nodes":[node],"subagents":[],"providerSessions":[adapter.runtime.session()],"providerThreads":[provider_thread],"providerTurns":[],"runtimeRequests":[],"messages":[],"plans":[],"turnItems":[],"checkpointScopes":[],"checkpoints":[],"contextHandoffs":[],"contextTransfers":[],"visibleTurnItems":[],"updatedAt":TEST_TIME}),
    )?)
}

pub(super) fn item(id: &str, kind: &str) -> Value {
    let mut item = json!({"id":id,"threadId":"thread:test","runId":"run:test","nodeId":"node:test","providerThreadId":"provider-thread:test","providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"running","title":"Work","startedAt":TEST_TIME,"completedAt":null,"updatedAt":TEST_TIME,"type":kind});
    match kind {
        "dynamic_tool" => {
            item["toolName"] = json!("monitor");
            item["input"] = json!({"persistent":false});
        }
        "secret_request" => {
            item["label"] = json!("Token");
            item["reason"] = json!("Sign in");
            item["secretStatus"] = json!("pending");
        }
        "link_request" => {
            item["reason"] = json!("Connect");
            item["linkStatus"] = json!("pending");
        }
        "user_input_request" => {
            item["requestId"] = json!(id);
            item["questions"] =
                json!([{"id":"q","header":"Choice","question":"What next?","options":[]}]);
        }
        "subagent" => {
            item["subagentId"] = json!(id);
            item["origin"] = json!("app_owned");
            item["driver"] = json!("codex");
            item["providerInstanceId"] = json!("codex");
            item["childThreadId"] = json!("child:test");
            item["prompt"] = json!("Work");
            item["result"] = Value::Null;
        }
        _ => {}
    }
    item
}

pub(super) fn sink(stores: StoreDatabase) -> EventSink {
    EventSink::new(
        stores.clone(),
        Arc::new(ProjectionStore::new(stores)),
        Arc::new(ManualClock::new(RECOVERED_AT)),
        OutboxSignals::default(),
    )
}
pub(super) async fn seed(
    sink: &EventSink,
    projection: &OrchestrationV2ThreadProjection,
) -> Result<()> {
    let value = serde_json::to_value(projection)?;
    let thread_id = &value["thread"]["id"];
    let key = thread_id.as_str().expect("expanded thread ID");
    let mut events = vec![serde_json::from_value(
        json!({"id":format!("seed:{key}:thread"),"threadId":thread_id,"type":"thread.created","occurredAt":TEST_TIME,"payload":value["thread"]}),
    )?];
    for (field, kind) in [
        ("runs", "run.created"),
        ("attempts", "run-attempt.created"),
        ("nodes", "node.updated"),
        ("subagents", "subagent.updated"),
        ("providerSessions", "provider-session.attached"),
        ("providerThreads", "provider-thread.updated"),
        ("providerTurns", "provider-turn.updated"),
        ("runtimeRequests", "runtime-request.updated"),
        ("messages", "message.updated"),
        ("turnItems", "turn-item.updated"),
    ] {
        for (index, payload) in value[field]
            .as_array()
            .expect("test value")
            .iter()
            .enumerate()
        {
            events.push(serde_json::from_value(json!({"id":format!("seed:{key}:{field}:{index}"),"threadId":thread_id,"type":kind,"occurredAt":TEST_TIME,"payload":payload}))?);
        }
    }
    sink.commit(CommitRequest {
        receipt: None,
        guard: CommitGuard::None,
        decision: Decision {
            events,
            ..Decision::default()
        },
    })
    .await?;
    Ok(())
}
pub(super) fn service(stores: StoreDatabase) -> ProviderRuntimeRecoveryService {
    let signals = OutboxSignals::default();
    let committer = EventSink::new(
        stores.clone(),
        Arc::new(ProjectionStore::new(stores.clone())),
        Arc::new(ManualClock::new(RECOVERED_AT)),
        signals.clone(),
    );
    ProviderRuntimeRecoveryService::new(
        stores.clone(),
        Arc::new(ProjectionStore::new(stores.clone())),
        Arc::new(committer),
        Arc::new(ProviderSessionManager::new(
            Arc::new(AdapterRegistry::from_adapters(vec![])),
            None,
        )),
        Arc::new(ManualClock::new(RECOVERED_AT)),
        Arc::new(SequenceIds::new("recovery")),
        signals,
    )
}
pub(super) async fn payload(
    stores: &StoreDatabase,
    table: &str,
    key: &str,
    id: &str,
) -> Result<Value> {
    let sql = format!("SELECT payload_json FROM {table} WHERE {key}=?1");
    let id = id.to_owned();
    stores
        .read(move |db| {
            Ok(serde_json::from_str(
                &db.query_row(&sql, [id], |r| r.get::<_, String>(0))?,
            )?)
        })
        .await
}

#[tokio::test]
async fn targeted_reconciliation_classifies_all_effects_without_touching_another_threads_writer()
-> Result<()> {
    let database = TestDatabase::memory()?;
    seed(&sink(database.stores.clone()), &projection()?).await?;
    let requests=vec![
        json!({"type":"provider-runtime.continue","sourceRunId":"run:test"}),
        json!({"type":"provider-session.detach","providerSessionId":"session:test"}),
        json!({"type":"provider-session.apply-model-selection","providerSessionId":"session:test","providerThreadId":"provider-thread:test","modelSelection":{"instanceId":"codex","model":"test"}}),
        json!({"type":"provider-turn.start","runId":"run:test"}),
        json!({"type":"provider-turn.interrupt","providerSessionId":"session:test","providerThreadId":"provider-thread:test","providerTurnId":"provider-turn:test"}),
        json!({"type":"provider-turn.steer","providerSessionId":"session:test","providerThreadId":"provider-thread:test","providerTurnId":"provider-turn:test","messageId":"message:test"}),
        json!({"type":"provider-turn.restart","providerSessionId":"session:test","providerThreadId":"provider-thread:test","providerTurnId":"provider-turn:test","interruptedAttemptId":"attempt:test","runId":"run:test"}),
        json!({"type":"runtime-request.respond","providerSessionId":"session:test","requestId":"request:test"}),
        json!({"type":"provider-thread.rollback","providerThreadId":"provider-thread:test","checkpointId":"checkpoint:test","scopeId":"scope:test"}),
        json!({"type":"checkpoint.capture","runId":"run:test","scopeId":"scope:test"}),
        json!({"type":"scheduled-tasks.cleanup"}),json!({"type":"terminal.cleanup"}),json!({"type":"preview.cleanup"}),
        json!({"type":"attachment.cleanup","attachmentIds":[]}),json!({"type":"thread-title.generate","kind":{"type":"regenerate"}}),json!({"type":"delegated-tasks.stop"}),
    ].into_iter().enumerate().map(|(index,request)|Ok(PendingEffect {id:format!("effect:{index}"),command_id:CommandId("effect:command".into()),thread_id:ThreadId("thread:test".into()),request:serde_json::from_value(request)?,available_at:None})).collect::<Result<Vec<_>>>()?;
    let expected = requests
        .iter()
        .map(|effect| (effect.id.clone(), effect.request.process_bound()))
        .collect::<Vec<_>>();
    database.stores.transaction(move|tx|{
        outbox::enqueue(tx,&requests,TEST_TIME)?;
        outbox::enqueue(tx,&[PendingEffect {id:"other:writer".into(),command_id:CommandId("other".into()),thread_id:ThreadId("other:thread".into()),request:EffectRequest::Start {run_id:RunId("other:run".into())},available_at:None}],TEST_TIME)?;
        tx.execute("UPDATE orchestration_v2_effect_outbox SET status='running',lease_owner='original-process'",[])?;Ok(())
    }).await?;
    let summary = service(database.stores.clone())
        .reconcile(Some(ThreadId("thread:test".into())))
        .await?;
    assert_eq!(summary.retired_effects, 6);
    assert_eq!(summary.requeued_effects, 10);
    database
        .stores
        .read(move |db| {
            for (id, process_bound) in expected {
                let row = outbox::get(db, &id)?.expect("test value");
                assert_eq!(
                    row.status,
                    if process_bound {
                        EffectStatus::Cancelled
                    } else {
                        EffectStatus::Pending
                    },
                    "{id}"
                );
                assert!(row.lease_owner.is_none());
            }
            let other = outbox::get(db, "other:writer")?.expect("test value");
            assert_eq!(other.status, EffectStatus::Running);
            assert_eq!(other.lease_owner.as_deref(), Some("original-process"));
            Ok(())
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn restart_reopens_database_then_reconciles_inflight_work_without_reopening_provider()
-> Result<()> {
    let database = TestDatabase::temporary()?;
    let directory = database.directory.expect("test value");
    let stores = database.stores;
    {
        let sink = sink(stores.clone());
        seed(&sink, &projection()?).await?;
        stores.transaction(|tx|{
            outbox::enqueue(tx,&[
                PendingEffect {id:"start:pending".into(),command_id:CommandId("start".into()),thread_id:ThreadId("thread:test".into()),request:EffectRequest::Start {run_id:RunId("run:test".into())},available_at:None},
                PendingEffect {id:"title:running".into(),command_id:CommandId("title".into()),thread_id:ThreadId("thread:test".into()),request:EffectRequest::GenerateTitle {kind:TitleKind::Regenerate},available_at:None},
            ],TEST_TIME)?;
            tx.execute("UPDATE orchestration_v2_effect_outbox SET status='running',lease_owner='lost-process' WHERE effect_id='title:running'",[])?;Ok(())
        }).await?;
        sink.drain().await;
    }
    drop(stores);
    let reopened = StoreDatabase(t3_persistence::Database::open(
        directory.path().join("statev2.sqlite"),
    )?);
    let summary = service(reopened.clone()).recover().await?;
    assert_eq!(summary.terminalized_runs, 1);
    assert_eq!(summary.stopped_sessions, 1);
    assert_eq!(summary.retired_effects, 1);
    assert_eq!(summary.requeued_effects, 1);
    for (table, key, id) in [
        ("runs", "run_id", "run:test"),
        ("run_attempts", "attempt_id", "attempt:test"),
        ("nodes", "node_id", "node:test"),
    ] {
        assert_eq!(
            payload(
                &reopened,
                &format!("orchestration_v2_projection_{table}"),
                key,
                id
            )
            .await?["status"],
            "cancelled"
        );
    }
    assert_eq!(
        payload(
            &reopened,
            "orchestration_v2_projection_provider_sessions",
            "provider_session_id",
            "session:test"
        )
        .await?["status"],
        "stopped"
    );
    reopened
        .read(|db| {
            let start = outbox::get(db, "start:pending")?.expect("test value");
            let title = outbox::get(db, "title:running")?.expect("test value");
            assert_eq!(start.status, EffectStatus::Cancelled);
            assert_eq!(title.status, EffectStatus::Pending);
            assert!(title.lease_owner.is_none());
            assert_eq!(title.available_at, RECOVERED_AT);
            Ok(())
        })
        .await?;
    assert_eq!(
        service(reopened.clone()).recover().await?,
        RecoverySummary::default()
    );
    Ok(())
}

#[tokio::test]
async fn replayable_checkpoint_preserves_waiting_run_but_cancels_process_background_work()
-> Result<()> {
    let database = TestDatabase::memory()?;
    let mut projection = projection()?;
    projection.runs[0].status = OrchestrationV2RunStatus::Waiting;
    projection
        .turn_items
        .push(serde_json::from_value(item("background", "dynamic_tool"))?);
    seed(&sink(database.stores.clone()), &projection).await?;
    database
        .stores
        .transaction(|tx| {
            outbox::enqueue(
                tx,
                &[PendingEffect {
                    id: "checkpoint".into(),
                    command_id: CommandId("command:effect:checkpoint.capture:run:test".into()),
                    thread_id: ThreadId("thread:test".into()),
                    request: EffectRequest::Capture {
                        run_id: RunId("run:test".into()),
                        scope_id: CheckpointScopeId("scope".into()),
                    },
                    available_at: None,
                }],
                TEST_TIME,
            )
        })
        .await?;
    let summary = service(database.stores.clone()).recover().await?;
    assert_eq!(summary.terminalized_runs, 0);
    assert_eq!(
        payload(
            &database.stores,
            "orchestration_v2_projection_runs",
            "run_id",
            "run:test"
        )
        .await?["status"],
        "waiting"
    );
    assert_eq!(
        payload(
            &database.stores,
            "orchestration_v2_projection_turn_items",
            "turn_item_id",
            "background"
        )
        .await?["status"],
        "cancelled"
    );
    assert_eq!(
        payload(
            &database.stores,
            "orchestration_v2_projection_runs",
            "run_id",
            "run:test"
        )
        .await?["restartCancelledBackgroundWork"][0]["kind"],
        "task"
    );
    Ok(())
}

#[tokio::test]
async fn questions_survive_startup_and_shutdown_while_live_callbacks_expire() -> Result<()> {
    for trigger in [RecoveryTrigger::Startup, RecoveryTrigger::Shutdown] {
        let database = TestDatabase::memory()?;
        let mut projection = projection()?;
        for (id, kind, capability) in [
            ("question", "user_input", "live"),
            ("approval", "permission", "live"),
            ("async", "user_input", "message"),
        ] {
            let response = if capability == "live" {
                json!({"type":"live","providerSessionId":"session:test"})
            } else {
                json!({"type":"message"})
            };
            projection.runtime_requests.push(serde_json::from_value(json!({"id":id,"nodeId":"node:test","providerTurnId":null,"nativeRequestRef":null,"kind":kind,"status":"pending","responseCapability":response,"createdAt":TEST_TIME,"resolvedAt":null}))?);
        }
        projection.turn_items.push(serde_json::from_value(item(
            "question",
            "user_input_request",
        ))?);
        seed(&sink(database.stores.clone()), &projection).await?;
        let service = service(database.stores.clone());
        let summary = if trigger == RecoveryTrigger::Startup {
            service.recover().await?
        } else {
            service.reconcile_shutdown().await?
        };
        assert_eq!(summary.closed_requests, 1);
        for id in ["question", "async"] {
            let request = payload(
                &database.stores,
                "orchestration_v2_projection_runtime_requests",
                "runtime_request_id",
                id,
            )
            .await?;
            assert_eq!(request["status"], "pending");
            assert_eq!(request["responseCapability"]["type"], "message");
        }
        assert_eq!(
            payload(
                &database.stores,
                "orchestration_v2_projection_runtime_requests",
                "runtime_request_id",
                "approval"
            )
            .await?["status"],
            if trigger == RecoveryTrigger::Startup {
                "expired"
            } else {
                "cancelled"
            }
        );
        assert_eq!(
            payload(
                &database.stores,
                "orchestration_v2_projection_nodes",
                "node_id",
                "node:test"
            )
            .await?["status"],
            "running"
        );
    }
    Ok(())
}

#[tokio::test]
async fn recovery_closes_secret_form_leaves_link_and_delegation_cards_open() -> Result<()> {
    let database = TestDatabase::memory()?;
    let mut projection = projection()?;
    for (id, kind) in [
        ("secret", "secret_request"),
        ("link", "link_request"),
        ("child", "subagent"),
    ] {
        projection
            .turn_items
            .push(serde_json::from_value(item(id, kind))?);
    }
    seed(&sink(database.stores.clone()), &projection).await?;
    service(database.stores.clone()).recover().await?;
    let secret = payload(
        &database.stores,
        "orchestration_v2_projection_turn_items",
        "turn_item_id",
        "secret",
    )
    .await?;
    assert_eq!(secret["status"], "cancelled");
    assert_eq!(secret["secretStatus"], "cancelled");
    for id in ["link", "child"] {
        assert_eq!(
            payload(
                &database.stores,
                "orchestration_v2_projection_turn_items",
                "turn_item_id",
                id
            )
            .await?["status"],
            "running"
        );
    }
    Ok(())
}

#[tokio::test]
async fn queued_runs_stay_held_while_all_inflight_statuses_cancel() -> Result<()> {
    for status in ["queued", "preparing", "starting", "running", "waiting"] {
        let database = TestDatabase::memory()?;
        let mut projection = projection()?;
        projection.runs[0].status = serde_json::from_value(json!(status))?;
        seed(&sink(database.stores.clone()), &projection).await?;
        let summary = service(database.stores.clone()).recover().await?;
        let run = payload(
            &database.stores,
            "orchestration_v2_projection_runs",
            "run_id",
            "run:test",
        )
        .await?;
        if status == "queued" {
            assert_eq!(summary.terminalized_runs, 0);
            assert_eq!(run["status"], "queued");
            assert_eq!(run["queueHeld"], true);
        } else {
            assert_eq!(summary.terminalized_runs, 1);
            assert_eq!(run["status"], "cancelled");
        }
    }
    Ok(())
}

#[tokio::test]
async fn commits_publish_only_after_recovery_projection_and_effect_cancellation_are_durable()
-> Result<()> {
    let database = TestDatabase::memory()?;
    let projection = projection()?;
    let sink = sink(database.stores.clone());
    seed(&sink, &projection).await?;
    let mut events = sink.subscribe();
    let plan = plan_recovery(
        &projection,
        &[],
        RecoveryTrigger::Startup,
        &WireDateTime(RECOVERED_AT.into()),
        &SequenceIds::new("recovery"),
        None,
    )?;
    let result = sink.commit(plan.commit).await?;
    assert!(result.committed);
    assert!(!result.events.is_empty());
    let published = events.recv().await.expect("committed recovery event");
    assert_eq!(
        serde_json::to_value(published)?["sequence"],
        json!(result.events[0].sequence)
    );
    assert_eq!(
        payload(
            &database.stores,
            "orchestration_v2_projection_runs",
            "run_id",
            "run:test"
        )
        .await?["status"],
        "cancelled"
    );
    Ok(())
}

#[tokio::test]
async fn shutdown_prepares_restart_intent_before_cancelling_provider_work() -> Result<()> {
    let database = TestDatabase::memory()?;
    seed(&sink(database.stores.clone()), &projection()?).await?;
    let recovery = service(database.stores.clone())
        .with_restart_selector(Arc::new(|_, _| Ok(Some(RunId("run:test".into())))));
    recovery.prepare_for_shutdown().await?;
    recovery.prepare_for_shutdown().await?;
    database
        .stores
        .read(|db| {
            let row =
                outbox::get(db, "effect:restart-continuation:run:test")?.expect("restart intent");
            assert_eq!(row.status, EffectStatus::Pending);
            assert!(matches!(row.request, EffectRequest::Continue { .. }));
            Ok(())
        })
        .await?;
    recovery.reconcile_shutdown().await?;
    database
        .stores
        .read(|db| {
            assert_eq!(
                outbox::get(db, "effect:restart-continuation:run:test")?
                    .expect("restart intent")
                    .status,
                EffectStatus::Pending
            );
            Ok(())
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn shutdown_preparation_contains_thread_failures_and_uses_selective_reads() -> Result<()> {
    let database = TestDatabase::memory()?;
    let reader = Arc::new(FakeProjectionReader::default());
    reader.recovery_thread_ids.push(Ok(vec![
        ThreadId("bad".into()),
        ThreadId("thread:test".into()),
    ]));
    reader
        .runtime_recovery
        .push(Err(t3_orchestration::error::Error::Service {
            operation: "read fixture",
            detail: "unreadable first thread".into(),
        }));
    reader.runtime_recovery.push(Ok(projection()?));
    let recovery = ProviderRuntimeRecoveryService::new(
        database.stores.clone(),
        reader.clone(),
        Arc::new(sink(database.stores.clone())),
        Arc::new(ProviderSessionManager::new(
            Arc::new(AdapterRegistry::from_adapters(vec![])),
            None,
        )),
        Arc::new(ManualClock::new(RECOVERED_AT)),
        Arc::new(SequenceIds::new("shutdown")),
        OutboxSignals::default(),
    )
    .with_restart_selector(Arc::new(|_, _| Ok(Some(RunId("run:test".into())))));
    recovery.prepare_for_shutdown().await?;
    assert_eq!(
        reader
            .runtime_recovery
            .next_call()
            .await
            .expect("first read")
            .0,
        ThreadId("bad".into())
    );
    assert_eq!(
        reader
            .runtime_recovery
            .next_call()
            .await
            .expect("second read")
            .0,
        ThreadId("thread:test".into())
    );
    database
        .stores
        .read(|db| {
            assert!(outbox::get(db, "effect:restart-continuation:run:test")?.is_some());
            Ok(())
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn shared_sessions_keep_original_liveness_for_each_threads_restart_decision() -> Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let database = TestDatabase::memory()?;
    let first = projection()?;
    let second: OrchestrationV2ThreadProjection = serde_json::from_str(
        &serde_json::to_string(&first)?
            .replace("thread:test", "thread:other")
            .replace("run:test", "run:other")
            .replace("attempt:test", "attempt:other")
            .replace("node:test", "node:other"),
    )?;
    let sink = sink(database.stores.clone());
    seed(&sink, &first).await?;
    seed(&sink, &second).await?;
    let selected = Arc::new(AtomicUsize::new(0));
    let observed = selected.clone();
    let recovery =
        service(database.stores.clone()).with_restart_selector(Arc::new(move |projection, _| {
            if projection
                .provider_sessions
                .iter()
                .any(|session| session.status == ProviderSessionStatus::Ready)
            {
                observed.fetch_add(1, Ordering::SeqCst);
                return Ok(Some(serde_json::from_value(serde_json::to_value(
                    &projection.runs[0].id,
                )?)?));
            }
            Ok(None)
        }));
    let summary = recovery.recover().await?;
    assert_eq!(summary.terminalized_runs, 2);
    assert_eq!(summary.stopped_sessions, 1);
    assert_eq!(selected.load(Ordering::SeqCst), 2);
    database
        .stores
        .read(|db| {
            for run in ["run:test", "run:other"] {
                assert!(outbox::get(db, &format!("effect:restart-continuation:{run}"))?.is_some());
            }
            Ok(())
        })
        .await?;
    Ok(())
}
