#![allow(clippy::unwrap_used)]
use crate::projection_store_test::{apply_values, fixture};
use t3_orchestration::{api::*, projections::store::ProjectionStore, testing::TestDatabase};

#[tokio::test]
async fn remote_task_discovery_keeps_missing_results_and_unpropagated_stops() {
    let db = TestDatabase::memory().unwrap();
    db.stores.transaction(|tx|{
        for (id,status,result,stopped,child,origin) in [
            ("pending","running",None,None,None,"app_owned"),
            ("finished","completed",Some("done"),None,None,"app_owned"),
            ("cancelled","cancelled",Some("done"),None,None,"app_owned"),
            ("stopped","cancelled",Some("done"),Some(true),None,"app_owned"),
            ("unstopped","completed",Some("done"),Some(false),None,"app_owned"),
            ("completed-stop","completed",Some("done"),Some(true),None,"app_owned"),
            ("local","running",None,None,Some("thread:child"),"app_owned"),
            ("native","running",None,None,None,"provider_native"),
        ] {
            let mut payload=serde_json::json!({"status":status,"result":result,"remoteChild":{},"unusedMalformedField":37});
            if let Some(stopped)=stopped {payload["remoteChild"]["stoppedThere"]=stopped.into();}
            tx.execute("INSERT INTO orchestration_v2_projection_subagents(subagent_id,thread_id,parent_node_id,provider,origin,status,child_thread_id,updated_at,payload_json) VALUES(?1,'thread:parent','node:parent','codex',?2,?3,?4,?5,?6)",t3_persistence::rusqlite::params![id,origin,status,child,crate::projection_store_test::NOW,payload.to_string()])?;
        }
        Ok(())
    }).await.unwrap();
    let tasks = ProjectionStore::new(db.stores)
        .open_remote_delegated_tasks()
        .await
        .unwrap();
    assert!(
        tasks
            .iter()
            .all(|t| t.parent_thread_id.0 == "thread:parent")
    );
    assert_eq!(
        tasks
            .into_iter()
            .map(|t| t.task_id.0)
            .collect::<std::collections::BTreeSet<_>>(),
        ["pending", "cancelled", "unstopped"]
            .map(str::to_owned)
            .into_iter()
            .collect()
    );
}

#[tokio::test]
async fn delegated_progress_uses_execution_order_waits_for_children_and_ignores_monitor_acknowledgements()
 {
    use serde_json::json;
    use t3_orchestration::ports::decisions::orchestrator::delegated_task_progress;
    let db = TestDatabase::memory().unwrap();
    let mut run = fixture("run.created", "run:test");
    run["payload"]["ordinal"] = 1.into();
    run["payload"]["status"] = "completed".into();
    run["payload"]["completedAt"] = crate::projection_store_test::NOW.into();
    apply_values(
        &db,
        vec![fixture("thread.created", "thread:test"), run.clone()],
    )
    .await;
    let base = ProjectionStore::new(db.stores)
        .snapshot(ThreadId("thread:test".into()), None)
        .await
        .unwrap()
        .projection;
    let mut state = base.clone();
    state.runs = [
        (4, "2026-07-24T10:00:00.000Z"),
        (2, "2026-07-24T10:05:00.000Z"),
        (3, "2026-07-24T10:10:00.000Z"),
    ]
    .map(|(ordinal, at)| {
        let mut r = run["payload"].clone();
        r["id"] = format!("run:{ordinal}").into();
        r["ordinal"] = ordinal.into();
        r["completedAt"] = at.into();
        serde_json::from_value(r).unwrap()
    })
    .to_vec();
    let progress = delegated_task_progress(&state).unwrap();
    assert_eq!(progress.state, "result_available");
    assert_eq!(progress.result_run.unwrap().ordinal, 3);
    for (status, delivery, expected) in [
        ("running", None, "waiting_for_children"),
        ("completed", None, "result_available"),
        ("idle", None, "result_available"),
        ("idle", Some("pending"), "result_available"),
        ("idle", Some("claimed"), "result_available"),
        ("completed", Some("pending"), "waiting_for_children"),
        ("completed", Some("claimed"), "waiting_for_children"),
        ("completed", Some("acknowledged"), "result_available"),
        ("completed", Some("delivered"), "result_available"),
        ("completed", Some("disposed"), "result_available"),
    ] {
        let mut state = base.clone();
        let mut task = fixture("subagent.updated", "task:child")["payload"].clone();
        task["status"] = status.into();
        if let Some(delivery) = delivery {
            task["completionDelivery"] = json!({"state":delivery,"observedByRunId":null});
        }
        state.subagents = vec![serde_json::from_value(task).unwrap()];
        assert_eq!(
            delegated_task_progress(&state).unwrap().state,
            expected,
            "{status} {delivery:?}"
        );
    }
    for awaited in [true, false] {
        let mut state = base.clone();
        let mut provider = fixture("provider-thread.updated", "provider:test")["payload"].clone();
        let mut command = json!({"taskId":"background-audit","kind":"command"});
        if awaited {
            command["awaited"] = true.into();
        }
        provider["pendingBackgroundTasks"] =
            json!([command,{"taskId":"log-watcher","kind":"monitor"}]);
        state.provider_threads = vec![serde_json::from_value(provider).unwrap()];
        assert_eq!(
            delegated_task_progress(&state).unwrap().state,
            if awaited {
                "waiting_for_children"
            } else {
                "result_available"
            }
        );
    }
    let mut queued = run["payload"].clone();
    queued["id"] = "run:followup".into();
    queued["ordinal"] = 2.into();
    queued["status"] = "queued".into();
    queued["startedAt"] = serde_json::Value::Null;
    let mut state = base.clone();
    state
        .runs
        .push(serde_json::from_value(queued.clone()).unwrap());
    assert_eq!(delegated_task_progress(&state).unwrap().state, "working");
    state.runs[0].status = OrchestrationV2RunStatus::Cancelled;
    queued["queueHeld"] = true.into();
    state.runs[1] = serde_json::from_value(queued).unwrap();
    let progress = delegated_task_progress(&state).unwrap();
    assert_eq!(progress.state, "result_available");
    assert_eq!(
        t3_contracts::stored::to_value(&progress.result_run.unwrap().id).unwrap(),
        "run:test"
    );
    let mut report = run["payload"].clone();
    report["id"] = "run:report".into();
    report["ordinal"] = 2.into();
    let mut monitor = report.clone();
    monitor["id"] = "run:monitor".into();
    monitor["ordinal"] = 3.into();
    let mut state = base;
    state.runs.extend([
        serde_json::from_value(report).unwrap(),
        serde_json::from_value(monitor).unwrap(),
    ]);
    let mut message = fixture("message.updated", "message:monitor")["payload"].clone();
    message["runId"] = "run:monitor".into();
    message["role"] = "user".into();
    message["notification"] =
        json!({"source":{"kind":"monitor"},"outcome":"updated","summary":"Monitor update"});
    state
        .messages
        .push(serde_json::from_value(message).unwrap());
    let progress = delegated_task_progress(&state).unwrap();
    assert_eq!(progress.state, "result_available");
    assert_eq!(
        t3_contracts::stored::to_value(&progress.result_run.unwrap().id).unwrap(),
        "run:report"
    );
}

#[tokio::test]
async fn message_order_follows_item_ordinals_and_records_filter_subagent_identity() {
    let db = TestDatabase::memory().unwrap();
    let mut later = fixture("message.updated", "message:later");
    later["payload"]["createdAt"] = "2020-01-01T00:00:00.000Z".into();
    let user_item = |id: &str, message: &str, ordinal: u64| {
        let mut event = fixture("turn-item.updated", id);
        event["payload"]["type"] = "assistant_message".into();
        event["payload"]["messageId"] = message.into();
        event["payload"]["text"] = "response".into();
        event["payload"]["streaming"] = false.into();
        event["payload"]["ordinal"] = ordinal.into();
        event
    };
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            fixture("message.updated", "message:first"),
            later,
            user_item("item:first", "message:first", 1),
            user_item("item:later", "message:later", 2),
            fixture("subagent.updated", "task:first"),
            fixture("subagent.updated", "task:other"),
        ],
    )
    .await;
    let store = ProjectionStore::new(db.stores.clone());
    let projection = store
        .projection(ThreadId("thread:test".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&projection.messages[0]).unwrap()["id"],
        "message:first"
    );
    let records = store
        .records(
            ThreadId("thread:test".into()),
            vec![RecordKind::Subagents],
            RecordFilter {
                subagent_ids: Some(vec![NodeId("task:first".into())]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let ProjectionRecordSet::Subagents(tasks) = &records.records[0] else {
        panic!("wrong record set");
    };
    assert_eq!(tasks.len(), 1);
    assert_eq!(serde_json::to_value(&tasks[0]).unwrap()["id"], "task:first");
    assert_eq!(projection.messages.len(), 2);
    let mut negative = fixture("message.updated", "message:negative");
    negative["payload"]["createdAt"] = "-000001-01-01T00:00:00.000Z".into();
    let mut future = fixture("message.updated", "message:future");
    future["payload"]["createdAt"] = "+010000-01-01T00:00:00.000Z".into();
    apply_values(&db, vec![future, negative]).await;
    let projection = store
        .projection(ThreadId("thread:test".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&projection.messages[2]).unwrap()["id"],
        "message:negative"
    );
    assert_eq!(
        serde_json::to_value(&projection.messages[3]).unwrap()["id"],
        "message:future"
    );
}
