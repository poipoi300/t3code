#![allow(clippy::unwrap_used)]
use crate::projection_store_test::{apply_values, fixture};
use t3_orchestration::{api::*, projections::store::ProjectionStore, testing::TestDatabase};

#[tokio::test]
async fn queue_readiness_excludes_held_inputs_and_never_bypasses_live_work() {
    let db = TestDatabase::memory().unwrap();
    let mut queued = fixture("run.created", "run:test");
    queued["payload"]["status"] = "queued".into();
    queued["payload"]["queueHeld"] = true.into();
    queued["payload"]["startedAt"] = serde_json::Value::Null;
    apply_values(
        &db,
        vec![fixture("thread.created", "thread:test"), queued.clone()],
    )
    .await;
    let store = ProjectionStore::new(db.stores.clone());
    let thread = ThreadId("thread:test".into());
    let run = RunId("run:test".into());
    assert!(
        !store
            .can_start_queued_run(thread.clone(), run.clone())
            .await
            .unwrap()
    );
    assert_eq!(
        store.held_queued_runs(thread.clone()).await.unwrap().len(),
        1
    );
    queued["payload"]["queueHeld"] = false.into();
    apply_values(&db, vec![queued]).await;
    assert!(
        store
            .can_start_queued_run(thread.clone(), run.clone())
            .await
            .unwrap()
    );
    let mut active = fixture("run.created", "run:active");
    active["payload"]["ordinal"] = 2.into();
    apply_values(&db, vec![active.clone()]).await;
    assert!(
        !store
            .can_start_queued_run(thread.clone(), run.clone())
            .await
            .unwrap()
    );
    active["payload"]["status"] = "completed".into();
    apply_values(&db, vec![active]).await;
    assert!(
        store
            .can_start_queued_run(thread.clone(), run.clone())
            .await
            .unwrap()
    );
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_threads SET archived_at='2026-11-13T10:00:00.000Z' WHERE thread_id='thread:test'",[])?;Ok(())}).await.unwrap();
    assert!(!store.can_start_queued_run(thread, run).await.unwrap());
}

#[tokio::test]
async fn turn_start_reads_stay_scoped_and_history_selects_only_turn_start_types() {
    let db = TestDatabase::memory().unwrap();
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            fixture("run.created", "run:test"),
            fixture("message.updated", "message:test"),
            fixture("turn-item.updated", "item:secret"),
        ],
    )
    .await;
    let store = ProjectionStore::new(db.stores.clone());
    let context = store
        .control(ControlQuery::TurnStart {
            thread_id: ThreadId("thread:test".into()),
            run_id: RunId("run:test".into()),
        })
        .await
        .unwrap()
        .unwrap();
    assert!(context.run.is_some());
    assert!(context.message.is_some());
    assert!(
        store
            .turn_start_history(ThreadId("thread:test".into()), Some(vec![]))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .turn_start_history(ThreadId("thread:test".into()), None)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn rich_turn_start_context_reads_requested_run_and_survives_obsolete_transcripts() {
    let db = TestDatabase::memory().unwrap();
    let mut user = fixture("message.updated", "message:test");
    user["payload"]["role"] = "user".into();
    user["payload"]["text"] = "\u{feff} /CoMpAcT \u{00a0}".into();
    let mut unrelated = fixture("message.updated", "message:old");
    unrelated["payload"]["runId"] = serde_json::Value::Null;
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            fixture("run.created", "run:test"),
            fixture("node.updated", "node:test"),
            user.clone(),
            unrelated,
        ],
    )
    .await;
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_messages SET payload_json='{}' WHERE message_id='message:old'",[])?;Ok(())}).await.unwrap();
    let store = ProjectionStore::new(db.stores.clone());
    let context = store
        .turn_start_context(ThreadId("thread:test".into()), RunId("run:test".into()))
        .await
        .unwrap();
    assert!(!context.has_conversation);
    assert_eq!(context.projection.messages.len(), 1);
    assert_eq!(context.projection.nodes.len(), 1);
    user["payload"]["text"] = "continue".into();
    apply_values(&db, vec![user]).await;
    assert!(
        store
            .turn_start_context(ThreadId("thread:test".into()), RunId("run:test".into()))
            .await
            .unwrap()
            .has_conversation
    );
}

#[tokio::test]
async fn fork_compaction_counts_native_history_and_only_delivered_portable_messages() {
    use serde_json::{Value, json};
    let db = TestDatabase::memory().unwrap();
    let mut fork = fixture("thread.created", "thread:fork");
    fork["threadId"] = "thread:fork".into();
    fork["payload"]["forkedFrom"] =
        json!({"type":"run","threadId":"thread:test","runId":"run:test"});
    let mut fork_run = fixture("run.created", "run:fork");
    fork_run["threadId"] = "thread:fork".into();
    fork_run["payload"]["threadId"] = "thread:fork".into();
    fork_run["payload"]["userMessageId"] = "message:fork".into();
    fork_run["payload"]["status"] = "queued".into();
    let mut compact = fixture("message.updated", "message:fork");
    compact["threadId"] = "thread:fork".into();
    compact["payload"]["threadId"] = "thread:fork".into();
    compact["payload"]["runId"] = "run:fork".into();
    compact["payload"]["role"] = "user".into();
    compact["payload"]["text"] = "/compact".into();
    compact["payload"]["attachments"] = json!([]);
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            fixture("run.created", "run:test"),
            fork,
            fork_run,
            compact,
        ],
    )
    .await;
    let store = ProjectionStore::new(db.stores.clone());
    let thread = ThreadId("thread:fork".into());
    let run = RunId("run:fork".into());
    assert!(
        store
            .turn_start_context(thread.clone(), run.clone())
            .await
            .unwrap()
            .has_conversation
    );
    let mut transfer = fixture("context-transfer.created", "transfer:fork");
    transfer["threadId"] = "thread:fork".into();
    transfer["payload"]["type"] = "fork".into();
    transfer["payload"]["sourceThreadId"] = "thread:test".into();
    transfer["payload"]["targetThreadId"] = "thread:fork".into();
    transfer["payload"]["targetRunId"] = "run:fork".into();
    transfer["payload"]["status"] = "pending".into();
    transfer["payload"]["resolution"] = json!({"strategy":"native_fork","providerThreadRef":{"driver":"claudeAgent","nativeId":"native-fork-source","strength":"strong"}});
    apply_values(&db, vec![transfer.clone()]).await;
    assert!(
        store
            .turn_start_context(thread.clone(), run.clone())
            .await
            .unwrap()
            .has_conversation
    );
    transfer["payload"]["resolution"] =
        json!({"strategy":"portable_context","contextHandoffId":"handoff:fork"});
    apply_values(&db, vec![transfer]).await;
    assert!(
        !store
            .turn_start_context(thread.clone(), run.clone())
            .await
            .unwrap()
            .has_conversation
    );
    let mut handoff = fixture("context-handoff.updated", "handoff:fork");
    handoff["threadId"] = "thread:fork".into();
    handoff["payload"]["threadId"] = "thread:fork".into();
    handoff["payload"]["targetRunId"] = "run:fork".into();
    handoff["payload"]["history"] = json!({"messages":[],"coverage":"empty","omittedItems":1});
    apply_values(&db, vec![handoff.clone()]).await;
    assert!(
        !store
            .turn_start_context(thread.clone(), run.clone())
            .await
            .unwrap()
            .has_conversation
    );
    let message: Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2HistoricalMessage.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    handoff["payload"]["history"]["messages"] = json!([message]);
    apply_values(&db, vec![handoff]).await;
    assert!(
        store
            .turn_start_context(thread, run)
            .await
            .unwrap()
            .has_conversation
    );
}
