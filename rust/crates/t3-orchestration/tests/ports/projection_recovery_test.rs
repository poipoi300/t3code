#![allow(clippy::unwrap_used)]
use crate::projection_store_test::{apply_values, fixture};
use t3_orchestration::{
    api::*,
    projections::{read::RecoveryKind, store::ProjectionStore},
    testing::TestDatabase,
};

async fn reconcile_projection(
    db: &TestDatabase,
    store: &ProjectionStore,
    thread: ThreadId,
    trigger:t3_orchestration::ports::sessions_recovery::provider_runtime_recovery_service::RecoveryTrigger,
) -> RecoverySummary {
    use t3_orchestration::{
        ports::sessions_recovery::provider_runtime_recovery_service::plan_recovery,
        testing::SequenceIds,
    };
    let projection = store.runtime_recovery(thread).await.unwrap();
    let plan = plan_recovery(
        &projection,
        &[],
        trigger,
        &t3_contracts::wire::WireDateTime(crate::projection_store_test::NOW.into()),
        &SequenceIds::new("projection-recovery"),
        None,
    )
    .unwrap();
    apply_values(
        db,
        plan.commit
            .decision
            .events
            .iter()
            .map(|e| t3_contracts::stored::to_value(e).unwrap())
            .collect(),
    )
    .await;
    plan.summary
}

#[tokio::test]
async fn recovery_stops_detached_sessions_and_orphaned_provider_threads_idempotently() {
    use t3_orchestration::ports::sessions_recovery::provider_runtime_recovery_service::{
        RecoveryTrigger, plan_recovery,
    };
    use t3_orchestration::testing::SequenceIds;
    for trigger in [RecoveryTrigger::Startup, RecoveryTrigger::Shutdown] {
        let db = TestDatabase::memory().unwrap();
        let mut session = fixture("provider-session.attached", "session:test");
        session["payload"]["status"] = "ready".into();
        let mut detached = crate::projection_store_test::sample("provider-session.detached");
        detached["threadId"] = "thread:test".into();
        detached["payload"]["providerSessionId"] = "session:test".into();
        let mut provider = fixture("provider-thread.updated", "provider:test");
        provider["payload"]["providerSessionId"] = serde_json::Value::Null;
        apply_values(
            &db,
            vec![
                fixture("thread.created", "thread:test"),
                session,
                detached,
                provider,
            ],
        )
        .await;
        let store = ProjectionStore::new(db.stores.clone());
        let id = ThreadId("thread:test".into());
        assert_eq!(
            store.recovery_thread_ids().await.unwrap().as_slice(),
            std::slice::from_ref(&id)
        );
        assert_eq!(
            reconcile_projection(&db, &store, id.clone(), trigger)
                .await
                .stopped_sessions,
            1
        );
        db.stores.read(|c|{assert_eq!(c.query_row::<String,_,_>("SELECT status FROM orchestration_v2_projection_provider_sessions WHERE provider_session_id='session:test'",[],|r|r.get(0))?,"stopped");assert_eq!(c.query_row::<String,_,_>("SELECT status FROM orchestration_v2_projection_provider_threads WHERE provider_thread_id='provider:test'",[],|r|r.get(0))?,"idle");Ok(())}).await.unwrap();
        assert!(store.recovery_thread_ids().await.unwrap().is_empty());
        let state = store.runtime_recovery(id).await.unwrap();
        assert!(
            plan_recovery(
                &state,
                &[],
                trigger,
                &t3_contracts::wire::WireDateTime(crate::projection_store_test::NOW.into()),
                &SequenceIds::new("second-pass"),
                None
            )
            .unwrap()
            .commit
            .decision
            .events
            .is_empty()
        );
    }
}

#[tokio::test]
async fn recovery_keeps_attached_errors_but_does_not_restore_detached_or_replaced_errors() {
    use t3_orchestration::ports::sessions_recovery::provider_runtime_recovery_service::RecoveryTrigger;
    for state in ["attached", "detached", "replaced"] {
        let db = TestDatabase::memory().unwrap();
        let mut session = fixture("provider-session.attached", "session:test");
        session["payload"]["status"] = "error".into();
        session["payload"]["lastError"] = "Rate limited".into();
        session["payload"]["extensionStatuses"] =
            serde_json::json!([{"key":"watch","text":"Watching"}]);
        let mut events = vec![fixture("thread.created", "thread:test"), session.clone()];
        if state != "attached" {
            let mut detached = crate::projection_store_test::sample("provider-session.detached");
            detached["threadId"] = "thread:test".into();
            detached["payload"]["providerSessionId"] = "session:test".into();
            events.push(detached);
        }
        if state == "replaced" {
            session["payload"]["id"] = "session:replacement".into();
            session["payload"]["status"] = "ready".into();
            session["payload"]["lastError"] = serde_json::Value::Null;
            session["payload"]["extensionStatuses"] = serde_json::json!([]);
            events.push(session);
        }
        apply_values(&db, events).await;
        let store = ProjectionStore::new(db.stores.clone());
        let thread = ThreadId("thread:test".into());
        assert_eq!(
            store.recovery_thread_ids().await.unwrap().len(),
            usize::from(state != "detached")
        );
        let before = store.runtime_recovery(thread.clone()).await.unwrap();
        let plan=t3_orchestration::ports::sessions_recovery::provider_runtime_recovery_service::plan_recovery(&before,&[],RecoveryTrigger::Startup,&t3_contracts::wire::WireDateTime(crate::projection_store_test::NOW.into()),&t3_orchestration::testing::SequenceIds::new("session-errors"),None).unwrap();
        let updates = plan
            .commit
            .decision
            .events
            .iter()
            .filter_map(|e| match e {
                OrchestrationV2DomainEvent::ProviderSessionUpdated(e) => {
                    Some(t3_contracts::stored::to_value(&e.payload).unwrap())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if state == "detached" {
            assert!(updates.is_empty());
        } else {
            assert_eq!(updates.len(), 1);
            assert_eq!(
                updates[0]["id"],
                if state == "attached" {
                    "session:test"
                } else {
                    "session:replacement"
                }
            );
            assert_eq!(
                updates[0]["status"],
                if state == "attached" {
                    "error"
                } else {
                    "stopped"
                }
            );
            assert_eq!(
                updates[0]["lastError"],
                if state == "attached" {
                    serde_json::json!("Rate limited")
                } else {
                    serde_json::Value::Null
                }
            );
            assert_eq!(updates[0]["extensionStatuses"], serde_json::json!([]));
        }
        apply_values(
            &db,
            plan.commit
                .decision
                .events
                .iter()
                .map(|e| t3_contracts::stored::to_value(e).unwrap())
                .collect(),
        )
        .await;
        assert!(store.recovery_thread_ids().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn startup_and_shutdown_cancel_buffered_wakes_and_hold_durable_queued_inputs() {
    use serde_json::{Value, json};
    use t3_orchestration::{
        ports::sessions_recovery::provider_runtime_recovery_service::{
            RecoveryTrigger, plan_recovery,
        },
        testing::SequenceIds,
    };
    for trigger in [RecoveryTrigger::Startup, RecoveryTrigger::Shutdown] {
        let db = TestDatabase::memory().unwrap();
        let store = ProjectionStore::new(db.stores.clone());
        for kind in ["buffered", "user", "notification", "delegated"] {
            let id = format!("thread:{kind}");
            let mut thread = fixture("thread.created", &id);
            thread["threadId"] = id.clone().into();
            let mut run = fixture("run.created", &format!("run:{kind}:1"));
            run["threadId"] = id.clone().into();
            run["payload"]["threadId"] = id.clone().into();
            run["payload"]["ordinal"] = 1.into();
            run["payload"]["status"] = "queued".into();
            run["payload"]["startedAt"] = Value::Null;
            run["payload"]["completedAt"] = Value::Null;
            run["payload"]["queuePosition"] = 1.into();
            run["payload"]["userMessageId"] = format!("message:{kind}").into();
            let mut completed = run.clone();
            completed["payload"]["id"] = format!("run:{kind}:2").into();
            completed["payload"]["ordinal"] = 2.into();
            completed["payload"]["status"] = "completed".into();
            completed["payload"]["startedAt"] = crate::projection_store_test::NOW.into();
            completed["payload"]["completedAt"] = crate::projection_store_test::NOW.into();
            let mut message = fixture("message.updated", &format!("message:{kind}"));
            message["threadId"] = id.clone().into();
            message["payload"]["threadId"] = id.clone().into();
            message["payload"]["runId"] = format!("run:{kind}:1").into();
            message["payload"]["role"] = "user".into();
            message["payload"]["streaming"] = false.into();
            message["payload"]["createdBy"] = if kind == "user" { "user" } else { "agent" }.into();
            message["payload"]["creationSource"] = if kind == "buffered" {
                "provider"
            } else {
                "server"
            }
            .into();
            if matches!(kind, "buffered" | "notification") {
                message["payload"]["notification"] = json!({"source":{"kind":"background_task"},"outcome":"updated","summary":"Background activity updated"});
            }
            if kind == "delegated" {
                message["payload"]["delegatedCompletion"] =
                    json!({"parentRunId":format!("run:{kind}:1"),"generation":1,"taskIds":[]});
            }
            apply_values(&db, vec![thread, run, completed, message]).await;
        }
        assert_eq!(store.recovery_thread_ids().await.unwrap().len(), 4);
        let now = t3_contracts::wire::WireDateTime(crate::projection_store_test::NOW.into());
        let ids = SequenceIds::new("queued-recovery");
        let mut terminalized = 0;
        for kind in ["buffered", "user", "notification", "delegated"] {
            let thread_id = ThreadId(format!("thread:{kind}"));
            let mut projection = store.runtime_recovery(thread_id.clone()).await.unwrap();
            let queued = store
                .query_records(ProjectionRecordQuery {
                    thread_id: thread_id.clone(),
                    fields: vec![RecordKind::Messages],
                    filter: RecordFilter::default(),
                    messages: MessageFilter {
                        ids: Some(vec![MessageId(format!("message:{kind}"))]),
                        ..Default::default()
                    },
                    run_statuses: None,
                    include_latest_run: false,
                })
                .await
                .unwrap();
            for records in queued.records {
                if let ProjectionRecordSet::Messages(messages) = records {
                    projection.messages.extend(messages);
                }
            }
            let plan = plan_recovery(&projection, &[], trigger, &now, &ids, None).unwrap();
            terminalized += plan.summary.terminalized_runs;
            apply_values(
                &db,
                plan.commit
                    .decision
                    .events
                    .iter()
                    .map(|e| t3_contracts::stored::to_value(e).unwrap())
                    .collect(),
            )
            .await;
            let projection = store.snapshot(thread_id, None).await.unwrap().projection;
            let runs = t3_contracts::stored::to_value(&projection.runs).unwrap();
            assert_eq!(runs[1]["status"], "completed");
            if kind == "buffered" {
                assert_eq!(runs[0]["status"], "cancelled");
                assert!(!runs[0]["completedAt"].is_null());
                assert!(runs[0]["queuePosition"].is_null());
            } else {
                assert_eq!(runs[0]["status"], "queued");
                assert_eq!(runs[0]["queueHeld"], true);
            }
        }
        assert_eq!(terminalized, 1);
        assert!(store.recovery_thread_ids().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn streaming_runtime_recovery_selects_live_runs_and_orders_only_their_messages() {
    let db = TestDatabase::memory().unwrap();
    let mut other = fixture("thread.created", "thread:other");
    other["threadId"] = "thread:other".into();
    let mut empty = fixture("thread.created", "thread:empty");
    empty["threadId"] = "thread:empty".into();
    let mut events = vec![fixture("thread.created", "thread:test"), other, empty];
    let statuses = [
        "queued",
        "preparing",
        "starting",
        "running",
        "waiting",
        "completed",
        "interrupted",
        "rolled_back",
        "failed",
        "cancelled",
    ];
    for (index, status) in statuses.into_iter().enumerate() {
        let run_id = format!("run:recovery:{index}");
        let mut run = fixture("run.created", &run_id);
        run["payload"]["ordinal"] = (index as u64 + 1).into();
        run["payload"]["status"] = status.into();
        events.push(run);
        for suffix in ["b", "a", "settled", "wrong-thread"] {
            let mut message = fixture(
                "message.updated",
                &format!("message:recovery:{index}:{suffix}"),
            );
            message["payload"]["role"] = "assistant".into();
            message["payload"]["runId"] = run_id.clone().into();
            message["payload"]["streaming"] = (suffix != "settled").into();
            message["payload"]["createdAt"] =
                format!("2026-11-13T09:00:{:02}.000Z", 59 - index).into();
            if suffix == "wrong-thread" {
                message["threadId"] = "thread:other".into();
                message["payload"]["threadId"] = "thread:other".into();
            }
            events.push(message);
        }
    }
    let mut foreign = fixture("run.created", "run:foreign");
    foreign["threadId"] = "thread:other".into();
    foreign["payload"]["threadId"] = "thread:other".into();
    events.push(foreign);
    apply_values(&db, events).await;
    db.stores.transaction(|tx|{
        tx.execute("INSERT INTO orchestration_v2_projection_messages(message_id,thread_id,run_id,role,streaming,created_at,updated_at,payload_json) VALUES('message:foreign','thread:test','run:foreign','assistant',1,'','','{}'),('message:old','thread:empty',NULL,'assistant',1,'','','{}')",[])?;Ok(())
    }).await.unwrap();
    let store = ProjectionStore::new(db.stores);
    let recovered = store
        .runtime_recovery(ThreadId("thread:test".into()))
        .await
        .unwrap();
    let expected = (0..5)
        .rev()
        .flat_map(|i| {
            [
                format!("message:recovery:{i}:a"),
                format!("message:recovery:{i}:b"),
            ]
        })
        .collect::<Vec<_>>();
    assert_eq!(
        recovered
            .messages
            .iter()
            .map(|m| serde_json::to_value(&m.id)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned())
            .collect::<Vec<_>>(),
        expected
    );
    assert!(recovered.plans.is_empty());
    assert!(recovered.checkpoints.is_empty());
    assert!(recovered.visible_turn_items.is_empty());
    assert!(
        store
            .runtime_recovery(ThreadId("thread:empty".into()))
            .await
            .unwrap()
            .messages
            .is_empty()
    );
}

#[tokio::test]
async fn recovery_selects_work_without_decoding_settled_histories() {
    let db = TestDatabase::memory().unwrap();
    let mut held = fixture("run.created", "run:held");
    held["payload"]["ordinal"] = 2.into();
    held["payload"]["status"] = "queued".into();
    held["payload"]["queueHeld"] = true.into();
    let mut settled = fixture("run.created", "run:done");
    settled["payload"]["status"] = "completed".into();
    apply_values(
        &db,
        vec![fixture("thread.created", "thread:test"), settled, held],
    )
    .await;
    let store = ProjectionStore::new(db.stores.clone());
    assert!(store.recovery_thread_ids().await.unwrap().is_empty());
    apply_values(
        &db,
        vec![fixture("provider-session.attached", "session:test")],
    )
    .await;
    assert_eq!(
        store.recovery_thread_ids().await.unwrap(),
        vec![ThreadId("thread:test".into())]
    );
    assert_eq!(
        store
            .recovery_candidates(RecoveryKind::RestartContinuations, None, None)
            .await
            .unwrap(),
        vec![ThreadId("thread:test".into())]
    );
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_runs SET payload_json='{}' WHERE run_id='run:done'",[])?;Ok(())}).await.unwrap();
    assert_eq!(store.recovery_thread_ids().await.unwrap().len(), 1);
    assert!(
        store
            .recovery_candidates(RecoveryKind::Runtime, Some(vec![]), None)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn unreadability_propagates_missing_or_corrupt_fork_sources_and_shared_bindings() {
    let db = TestDatabase::memory().unwrap();
    let source = fixture("thread.created", "thread:test");
    let mut child = fixture("thread.created", "thread:child");
    child["threadId"] = "thread:child".into();
    child["payload"]["forkedFrom"] =
        serde_json::json!({"type":"run","threadId":"thread:test","runId":"run:test"});
    let mut grandchild = child.clone();
    grandchild["threadId"] = "thread:grandchild".into();
    grandchild["payload"]["id"] = "thread:grandchild".into();
    grandchild["payload"]["forkedFrom"]["threadId"] = "thread:child".into();
    apply_values(&db, vec![source, child, grandchild]).await;
    let store = ProjectionStore::new(db.stores.clone());
    assert!(store.unreadable_thread_ids().await.unwrap().is_empty());
    db.stores
        .transaction(|tx| {
            tx.execute(
                "DELETE FROM orchestration_v2_projection_threads WHERE thread_id='thread:test'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        store.unreadable_thread_ids().await.unwrap(),
        vec![
            ThreadId("thread:child".into()),
            ThreadId("thread:grandchild".into())
        ]
    );
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            fixture("provider-session.attached", "session:test"),
        ],
    )
    .await;
    db.stores
        .transaction(|tx| {
            tx.execute(
                "UPDATE orchestration_v2_projection_provider_sessions SET payload_json='{}'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(store.unreadable_thread_ids().await.unwrap().len(), 3);
}

#[tokio::test]
async fn recovery_rosters_keep_the_executed_owner_before_queued_and_resumed_runs() {
    use serde_json::Value;
    let db = TestDatabase::memory().unwrap();
    let mut events = Vec::new();
    for name in ["queued", "resumed", "quiet"] {
        let id = format!("thread:{name}");
        let provider_id = format!("provider:{name}");
        let mut thread = fixture("thread.created", &id);
        thread["threadId"] = id.clone().into();
        events.push(thread);
        for ordinal in 1..=2 {
            let run_id = format!("run:{name}:{ordinal}");
            let mut run = fixture("run.created", &run_id);
            run["threadId"] = id.clone().into();
            run["payload"]["threadId"] = id.clone().into();
            run["payload"]["ordinal"] = ordinal.into();
            run["payload"]["providerThreadId"] = provider_id.clone().into();
            run["payload"]["status"] = "completed".into();
            run["payload"]["completedAt"] = if ordinal == 1 {
                "2026-11-13T10:05:00.000Z"
            } else {
                "2026-11-13T10:00:00.000Z"
            }
            .into();
            if ordinal == 2 && name != "resumed" {
                run["payload"]["status"] = "queued".into();
                run["payload"]["startedAt"] = Value::Null;
                run["payload"]["completedAt"] = Value::Null;
            }
            events.push(run);
        }
        if name != "quiet" {
            let mut provider = fixture("provider-thread.updated", &provider_id);
            provider["threadId"] = id.clone().into();
            provider["payload"]["appThreadId"] = id.into();
            provider["payload"]["status"] = "idle".into();
            provider["payload"]["pendingBackgroundTasks"] = serde_json::json!([{"taskId":"background","description":"Still running","kind":"command"}]);
            events.push(provider);
        }
    }
    apply_values(&db, events).await;
    let store = ProjectionStore::new(db.stores.clone());
    for name in ["queued", "resumed", "quiet"] {
        let state = store
            .runtime_recovery(ThreadId(format!("thread:{name}")))
            .await
            .unwrap();
        let runs = state
            .runs
            .iter()
            .map(|r| {
                t3_contracts::stored::to_value(&r.id)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            runs,
            if name == "quiet" {
                vec![format!("run:{name}:2")]
            } else {
                vec![format!("run:{name}:1"), format!("run:{name}:2")]
            }
        );
    }
}

#[tokio::test]
async fn runtime_recovery_follows_native_ownership_and_leaves_delegated_child_rosters_alone() {
    use serde_json::{Value, json};
    let db = TestDatabase::memory().unwrap();
    let mut node = fixture("node.updated", "node:owned");
    node["payload"]["runId"] = Value::Null;
    node["payload"]["kind"] = "root_turn".into();
    node["payload"]["status"] = "running".into();
    let mut events = vec![fixture("thread.created", "thread:test"), node];
    for (name, app, owner, roster, active) in [
        ("root", Some("thread:test"), None, false, true),
        (
            "owned",
            Some("thread:foreign"),
            Some("node:owned"),
            true,
            false,
        ),
        ("native", Some("thread:native-child"), None, true, false),
        (
            "delegated",
            Some("thread:delegated-child"),
            None,
            true,
            false,
        ),
        ("foreign", Some("thread:foreign"), None, true, true),
        ("quiet", Some("thread:test"), None, false, false),
    ] {
        let mut provider = fixture("provider-thread.updated", &format!("provider:{name}"));
        provider["payload"]["appThreadId"] = json!(app);
        provider["payload"]["ownerNodeId"] = json!(owner);
        provider["payload"]["status"] = if active { "active" } else { "idle" }.into();
        provider["payload"]["pendingBackgroundTasks"] = if roster {
            json!([{"taskId":format!("task:{name}"),"kind":"command"}])
        } else {
            json!([])
        };
        events.push(provider);
        if matches!(name, "native" | "delegated") {
            let mut task = fixture("subagent.updated", &format!("task:{name}"));
            task["payload"]["runId"] = Value::Null;
            task["payload"]["origin"] = if name == "native" {
                "provider_native"
            } else {
                "app_owned"
            }
            .into();
            task["payload"]["status"] = "completed".into();
            task["payload"]["providerThreadId"] = format!("provider:{name}").into();
            task["payload"]["childThreadId"] = if name == "delegated" {
                "thread:delegated-child".into()
            } else {
                Value::Null
            };
            events.push(task);
        }
    }
    apply_values(&db, events).await;
    let state = ProjectionStore::new(db.stores)
        .runtime_recovery(ThreadId("thread:test".into()))
        .await
        .unwrap();
    assert_eq!(
        state
            .provider_threads
            .iter()
            .map(|t| t3_contracts::stored::to_value(&t.id)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned())
            .collect::<std::collections::BTreeSet<_>>(),
        ["provider:root", "provider:owned", "provider:native"]
            .map(str::to_owned)
            .into_iter()
            .collect()
    );
    assert_eq!(state.nodes.len(), 1);
    assert!(state.subagents.is_empty());
}

#[tokio::test]
async fn delegated_candidate_indexes_tolerate_legacy_json_and_rebuild_after_payload_transitions() {
    use serde_json::json;
    let db = TestDatabase::memory().unwrap();
    let mut child = fixture("thread.created", "thread:child");
    child["threadId"] = "thread:child".into();
    let original = child["payload"].clone();
    let mut run = fixture("run.created", "run:child");
    run["threadId"] = "thread:child".into();
    run["payload"]["threadId"] = "thread:child".into();
    run["payload"]["status"] = "completed".into();
    apply_values(
        &db,
        vec![fixture("thread.created", "thread:test"), child, run],
    )
    .await;
    let store = ProjectionStore::new(db.stores.clone());
    let mut legacy = original.clone();
    legacy["lineage"] = json!({"parentThreadId":"thread:test","relationshipToParent":"subagent"});
    legacy["forkedFrom"] = json!({"type":"provider_thread"});
    let mut node = legacy.clone();
    node["forkedFrom"] = json!({"type":"node","nodeId":"node:index"});
    let mut self_parent = node.clone();
    self_parent["lineage"]["parentThreadId"] = "thread:child".into();
    for (payload, included) in [
        ("{broken".to_owned(), false),
        (legacy.to_string(), false),
        (node.to_string(), true),
        (self_parent.to_string(), true),
        (original.to_string(), false),
    ] {
        db.stores.transaction(move|tx|{tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json=?1 WHERE thread_id='thread:child'",[payload])?;Ok(())}).await.unwrap();
        assert_eq!(
            store
                .recovery_candidates(RecoveryKind::SubagentResults, None, None)
                .await
                .unwrap(),
            if included {
                vec![ThreadId("thread:child".into())]
            } else {
                vec![]
            }
        );
    }
    for (payload, included) in [
        ("{broken", false),
        ("{}", false),
        ("{\"delegatedCompletion\":{\"delivery\":null}}", false),
        ("{\"delegatedCompletion\":{\"delivery\":[]}}", false),
        ("{\"delegatedCompletion\":{\"delivery\":{}}}", true),
        ("{}", false),
    ] {
        db.stores.transaction(move|tx|{tx.execute("UPDATE orchestration_v2_projection_runs SET payload_json=?1 WHERE run_id='run:child'",[payload])?;Ok(())}).await.unwrap();
        assert_eq!(
            store
                .recovery_candidates(RecoveryKind::DelegatedCompletions, None, None)
                .await
                .unwrap(),
            if included {
                vec![ThreadId("thread:child".into())]
            } else {
                vec![]
            }
        );
    }
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json='{broken' WHERE thread_id='thread:test'",[])?;tx.execute("UPDATE orchestration_v2_projection_runs SET payload_json='{broken' WHERE run_id='run:child'",[])?;Ok(())}).await.unwrap();
    let mut existing = fixture("thread.created", "thread:existing");
    existing["threadId"] = "thread:existing".into();
    existing["payload"]["lineage"] = json!({"parentThreadId":"thread:child","relationshipToParent":"subagent","rootThreadId":"thread:test"});
    existing["payload"]["forkedFrom"] = json!({"type":"node","nodeId":"node:existing"});
    let mut run = fixture("run.created", "run:existing");
    run["threadId"] = "thread:existing".into();
    run["payload"]["threadId"] = "thread:existing".into();
    run["payload"]["status"] = "completed".into();
    run["payload"]["delegatedCompletion"] = json!({"disposition":"open","nextGeneration":2,"delivery":{"generation":1,"messageId":"message:existing","taskIds":[]}});
    apply_values(&db, vec![existing, run]).await;
    db.stores
        .transaction(|tx| {
            tx.execute(
                "DROP INDEX orchestration_v2_projection_threads_subagent_result_idx",
                [],
            )?;
            tx.execute(
                "DROP INDEX orchestration_v2_projection_runs_delegated_delivery_idx",
                [],
            )?;
            tx.execute_batch(include_str!(
                "../../../../crates/t3-persistence/sql/079.sql"
            ))?;
            Ok(())
        })
        .await
        .unwrap();
    for kind in [
        RecoveryKind::SubagentResults,
        RecoveryKind::DelegatedCompletions,
    ] {
        assert_eq!(
            store.recovery_candidates(kind, None, None).await.unwrap(),
            [ThreadId("thread:existing".into())]
        );
    }
}

#[tokio::test]
async fn subagent_results_require_terminal_work_and_an_unrecorded_transfer_or_explicit_followup() {
    use serde_json::{Value, json};
    let db = TestDatabase::memory().unwrap();
    let mut events = vec![fixture("thread.created", "thread:test")];
    for name in [
        "terminal", "archived", "deleted", "running", "held", "queued",
    ] {
        let id = format!("child:{name}");
        let mut child = fixture("thread.created", &id);
        child["threadId"] = id.clone().into();
        child["payload"]["lineage"] = json!({"parentThreadId":"thread:test","relationshipToParent":"subagent","rootThreadId":"thread:test"});
        child["payload"]["forkedFrom"] = json!({"type":"node","nodeId":format!("node:{name}")});
        if name == "archived" {
            child["payload"]["archivedAt"] = crate::projection_store_test::NOW.into();
        }
        if name == "deleted" {
            child["payload"]["deletedAt"] = crate::projection_store_test::NOW.into();
        }
        let mut run = fixture("run.created", &format!("run:{name}:1"));
        run["threadId"] = id.clone().into();
        run["payload"]["threadId"] = id.clone().into();
        run["payload"]["status"] = if name == "running" {
            "running"
        } else {
            "completed"
        }
        .into();
        run["payload"]["ordinal"] = 1.into();
        events.extend([child, run]);
        if matches!(name, "held" | "queued") {
            let mut run = fixture("run.created", &format!("run:{name}:2"));
            run["threadId"] = id.clone().into();
            run["payload"]["threadId"] = id.into();
            run["payload"]["status"] = "queued".into();
            run["payload"]["ordinal"] = 2.into();
            run["payload"]["startedAt"] = Value::Null;
            run["payload"]["queueHeld"] = (name == "held").into();
            events.push(run);
        }
    }
    apply_values(&db, events).await;
    let store = ProjectionStore::new(db.stores.clone());
    let candidates = || store.recovery_candidates(RecoveryKind::SubagentResults, None, None);
    assert_eq!(
        candidates()
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.0)
            .collect::<std::collections::BTreeSet<_>>(),
        ["child:terminal", "child:archived", "child:held"]
            .map(str::to_owned)
            .into_iter()
            .collect()
    );
    let mut transfer = fixture("context-transfer.created", "transfer:result");
    transfer["payload"]["type"] = "subagent_result".into();
    transfer["payload"]["sourceThreadId"] = "child:terminal".into();
    transfer["payload"]["targetThreadId"] = "thread:test".into();
    transfer["payload"]["targetRunId"] = Value::Null;
    transfer["payload"]["resolution"] = Value::Null;
    apply_values(&db, vec![transfer]).await;
    assert_eq!(
        candidates()
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.0)
            .collect::<std::collections::BTreeSet<_>>(),
        ["child:archived", "child:held"]
            .map(str::to_owned)
            .into_iter()
            .collect()
    );
    for (ordinal, delegated, status) in [
        (2, false, "completed"),
        (3, true, "completed"),
        (4, false, "running"),
    ] {
        let mut run = fixture("run.created", &format!("run:terminal:{ordinal}"));
        run["threadId"] = "child:terminal".into();
        run["payload"]["threadId"] = "child:terminal".into();
        run["payload"]["ordinal"] = ordinal.into();
        run["payload"]["status"] = status.into();
        if delegated {
            run["payload"]["delegatedTaskId"] = "node:followup".into();
        }
        apply_values(&db, vec![run]).await;
    }
    assert!(
        !candidates()
            .await
            .unwrap()
            .contains(&ThreadId("child:terminal".into()))
    );
    let mut task = fixture("subagent.updated", "node:followup");
    task["payload"]["origin"] = "app_owned".into();
    task["payload"]["runId"] = Value::Null;
    task["payload"]["childThreadId"] = "child:terminal".into();
    task["payload"]["status"] = "running".into();
    task["payload"]["result"] = Value::Null;
    apply_values(&db, vec![task.clone()]).await;
    assert!(
        candidates()
            .await
            .unwrap()
            .contains(&ThreadId("child:terminal".into()))
    );
    task["payload"]["status"] = "completed".into();
    task["payload"]["result"] = "Follow-up result.".into();
    apply_values(&db, vec![task]).await;
    assert!(
        !candidates()
            .await
            .unwrap()
            .contains(&ThreadId("child:terminal".into()))
    );
    assert!(store.unreadable_thread_ids().await.unwrap().is_empty());
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_context_transfers SET payload_json='{}' WHERE context_transfer_id='transfer:result'",[])?;Ok(())}).await.unwrap();
    assert_eq!(
        store
            .unreadable_thread_ids()
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.0)
            .collect::<std::collections::BTreeSet<_>>(),
        ["thread:test", "child:terminal"]
            .map(str::to_owned)
            .into_iter()
            .collect()
    );
}

#[tokio::test]
async fn restart_continuations_use_the_run_whose_shell_or_native_subagent_was_lost() {
    use serde_json::{Value, json};
    use t3_orchestration::{
        effects::EffectRequest,
        ports::{
            runs_ingestion::restart_continuation::restart_continuation_run,
            sessions_recovery::provider_runtime_recovery_service::{
                RecoveryTrigger, plan_recovery,
            },
        },
        testing::SequenceIds,
    };
    for kind in ["subagent-shell", "native", "delegated-shell"] {
        let db = TestDatabase::memory().unwrap();
        let mut thread = fixture("thread.created", "thread:test");
        thread["payload"]["providerInstanceId"] = "codex".into();
        thread["payload"]["modelSelection"] = json!({"instanceId":"codex","model":"test-model"});
        let mut run = fixture("run.created", "run:test");
        run["payload"]["providerInstanceId"] = "codex".into();
        run["payload"]["modelSelection"] = json!({"instanceId":"codex","model":"test-model"});
        run["payload"]["ordinal"] = 1.into();
        run["payload"]["status"] = "completed".into();
        run["payload"]["providerThreadId"] = "provider:root".into();
        run["payload"]["completedAt"] = crate::projection_store_test::NOW.into();
        let mut session = fixture("provider-session.attached", "session:test");
        session["payload"]["providerInstanceId"] = "codex".into();
        session["payload"]["driver"] = "codex".into();
        session["payload"]["status"] = "ready".into();
        let mut provider = fixture("provider-thread.updated", "provider:root");
        provider["payload"]["providerInstanceId"] = "codex".into();
        provider["payload"]["driver"] = "codex".into();
        provider["payload"]["status"] = "idle".into();
        provider["payload"]["nativeThreadRef"] =
            json!({"driver":"codex","nativeId":"native-thread","strength":"strong"});
        provider["payload"]["pendingBackgroundTasks"] = match kind {
            "subagent-shell" => {
                json!([{"taskId":"shell","description":"sleep 150","kind":"command","parentTaskId":"agent"}])
            }
            "delegated-shell" => {
                json!([{"taskId":"wait","description":"sleep 600","kind":"command","awaited":true}])
            }
            _ => json!([]),
        };
        let mut events = vec![thread, run.clone(), session, provider];
        if kind == "subagent-shell" {
            run["payload"]["id"] = "run:cut-wake".into();
            run["payload"]["ordinal"] = 2.into();
            run["payload"]["status"] = "starting".into();
            run["payload"]["startedAt"] = Value::Null;
            run["payload"]["completedAt"] = Value::Null;
            events.push(run);
        }
        if kind == "native" {
            let mut child = fixture("provider-thread.updated", "provider:child");
            child["payload"]["appThreadId"] = "thread:child".into();
            child["payload"]["providerInstanceId"] = "codex".into();
            child["payload"]["driver"] = "codex".into();
            child["payload"]["status"] = "idle".into();
            child["payload"]["nativeThreadRef"] =
                json!({"driver":"codex","nativeId":"agent-thread","strength":"strong"});
            let mut task = fixture("subagent.updated", "node:native");
            task["payload"]["runId"] = "run:test".into();
            task["payload"]["origin"] = "provider_native".into();
            task["payload"]["providerInstanceId"] = "codex".into();
            task["payload"]["driver"] = "codex".into();
            task["payload"]["providerThreadId"] = "provider:child".into();
            task["payload"]["childThreadId"] = Value::Null;
            task["payload"]["status"] = "running".into();
            task["payload"]["nativeTaskRef"] =
                json!({"driver":"codex","nativeId":"call_1:agent-thread","strength":"strong"});
            let mut item =
                crate::projection_store_test::conversation_item("item:native", Some("run:test"), 1);
            item["payload"].as_object_mut().unwrap().extend(json!({"type":"subagent","nodeId":"node:native","subagentId":"node:native","providerThreadId":"provider:child","nativeItemRef":{"driver":"codex","nativeId":"call_1:agent-thread","strength":"strong"},"status":"running","origin":"provider_native","driver":"codex","providerInstanceId":"codex","childThreadId":null,"prompt":"audit src","title":"audit","result":null}).as_object().unwrap().clone());
            events.extend([child, task, item]);
        }
        apply_values(&db, events).await;
        let store = ProjectionStore::new(db.stores.clone());
        let state = store
            .runtime_recovery(ThreadId("thread:test".into()))
            .await
            .unwrap();
        let now = t3_contracts::wire::WireDateTime(crate::projection_store_test::NOW.into());
        let source = restart_continuation_run(&state, &now).unwrap().map(|r| {
            RunId(
                t3_contracts::stored::to_value(&r.id)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned(),
            )
        });
        assert_eq!(source, Some(RunId("run:test".into())), "{kind}");
        let plan = plan_recovery(
            &state,
            &[],
            RecoveryTrigger::Startup,
            &now,
            &SequenceIds::new("lost-work"),
            source,
        )
        .unwrap();
        assert_eq!(plan.commit.decision.effects.len(), 1);
        let effect = &plan.commit.decision.effects[0];
        assert_eq!(effect.id, "effect:restart-continuation:run:test");
        assert!(
            matches!(&effect.request,EffectRequest::Continue{source_run_id} if source_run_id.0=="run:test")
        );
        apply_values(
            &db,
            plan.commit
                .decision
                .events
                .iter()
                .map(|e| t3_contracts::stored::to_value(e).unwrap())
                .collect(),
        )
        .await;
        let run = t3_contracts::stored::to_value(
            &store
                .run(ThreadId("thread:test".into()), RunId("run:test".into()))
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let expected = match kind {
            "subagent-shell" => {
                json!([{"kind":"subagent","label":"sleep 150 (subagent agent)","id":"agent"}])
            }
            "native" => {
                json!([{"kind":"subagent","label":"audit (id agent-thread)","id":"item:native"}])
            }
            _ => json!([{"kind":"shell","label":"sleep 600 (id wait)","id":"wait","awaited":true}]),
        };
        assert_eq!(run["restartCancelledBackgroundWork"], expected, "{kind}");
    }
}

#[tokio::test]
async fn startup_cancels_a_native_childs_runless_root_and_its_provider_turn() {
    use serde_json::Value;
    use t3_orchestration::ports::sessions_recovery::provider_runtime_recovery_service::RecoveryTrigger;
    let db = TestDatabase::memory().unwrap();
    let mut child = fixture("thread.created", "thread:child");
    child["threadId"] = "thread:child".into();
    let mut root = fixture("node.updated", "node:child-root");
    root["threadId"] = "thread:child".into();
    root["payload"]["threadId"] = "thread:child".into();
    root["payload"]["runId"] = Value::Null;
    root["payload"]["rootNodeId"] = "node:child-root".into();
    root["payload"]["parentNodeId"] = Value::Null;
    root["payload"]["kind"] = "root_turn".into();
    root["payload"]["status"] = "running".into();
    root["payload"]["providerThreadId"] = "provider:child".into();
    root["payload"]["providerTurnId"] = "turn:child".into();
    let mut turn = fixture("provider-turn.updated", "turn:child");
    turn["threadId"] = "thread:child".into();
    turn["payload"]["providerThreadId"] = "provider:child".into();
    turn["payload"]["nodeId"] = "node:child-root".into();
    turn["payload"]["runAttemptId"] = Value::Null;
    turn["payload"]["status"] = "running".into();
    let mut task = fixture("subagent.updated", "node:parent-task");
    task["payload"]["runId"] = Value::Null;
    task["payload"]["origin"] = "provider_native".into();
    task["payload"]["providerThreadId"] = "provider:child".into();
    task["payload"]["childThreadId"] = "thread:child".into();
    task["payload"]["status"] = "running".into();
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            child,
            root,
            turn,
            task,
        ],
    )
    .await;
    let store = ProjectionStore::new(db.stores.clone());
    assert!(
        store
            .recovery_thread_ids()
            .await
            .unwrap()
            .contains(&ThreadId("thread:child".into()))
    );
    assert_eq!(
        store
            .runtime_recovery(ThreadId("thread:child".into()))
            .await
            .unwrap()
            .provider_turns
            .len(),
        1
    );
    reconcile_projection(
        &db,
        &store,
        ThreadId("thread:test".into()),
        RecoveryTrigger::Startup,
    )
    .await;
    reconcile_projection(
        &db,
        &store,
        ThreadId("thread:child".into()),
        RecoveryTrigger::Startup,
    )
    .await;
    let projection = store
        .snapshot(ThreadId("thread:child".into()), None)
        .await
        .unwrap()
        .projection;
    assert_eq!(
        t3_contracts::stored::to_value(&projection.nodes[0]).unwrap()["status"],
        "cancelled"
    );
    assert_eq!(
        t3_contracts::stored::to_value(&projection.provider_turns[0]).unwrap()["status"],
        "cancelled"
    );
}
