#![allow(clippy::unwrap_used)]
use crate::projection_store_test::{NOW, apply_values, fixture};
use serde_json::{Value, json};
use t3_orchestration::{api::*, projections::store::ProjectionStore, testing::TestDatabase};

fn thread(id: &str, overrides: Value) -> Value {
    let mut event = fixture("thread.created", id);
    event["threadId"] = id.into();
    event["payload"]
        .as_object_mut()
        .unwrap()
        .extend(overrides.as_object().unwrap().clone());
    event
}
fn run(thread: &str, id: &str, status: &str) -> Value {
    let mut event = fixture("run.created", id);
    event["threadId"] = thread.into();
    event["payload"]["threadId"] = thread.into();
    event["payload"]["ordinal"] = 1.into();
    event["payload"]["status"] = status.into();
    event["payload"]["completedAt"] = NOW.into();
    if status == "queued" {
        event["payload"]["startedAt"] = Value::Null;
    }
    event
}
fn ids(rows: Vec<OrchestrationV2ThreadShell>) -> Vec<String> {
    rows.into_iter()
        .map(|r| {
            serde_json::to_value(r.id)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

#[tokio::test]
async fn settlement_candidates_exclude_overrides_live_work_and_unfinished_parents() {
    let db = TestDatabase::memory().unwrap();
    let mut events = vec![
        thread("thread:idle", json!({})),
        thread("thread:completed", json!({})),
        run("thread:completed", "run:completed", "completed"),
        thread("thread:queued", json!({})),
        run("thread:queued", "run:queued", "queued"),
    ];
    for (name, overrides) in [
        ("archived", json!({"archivedAt":NOW})),
        ("deleted", json!({"deletedAt":NOW})),
        (
            "settled",
            json!({"settledOverride":"settled","settledAt":NOW}),
        ),
        ("active", json!({"settledOverride":"active"})),
        ("pinned", json!({"pinnedAt":NOW})),
        ("disabled", json!({"autoSettleDisabledAt":NOW})),
    ] {
        events.push(thread(&format!("thread:{name}"), overrides));
    }
    for status in ["preparing", "starting", "running", "waiting"] {
        let id = format!("thread:{status}");
        events.push(thread(&id, json!({})));
        events.push(run(&id, &format!("run:{status}"), status));
    }
    events.push(thread("thread:blocked", json!({})));
    let mut request = fixture("runtime-request.updated", "request:blocked");
    request["threadId"] = "thread:blocked".into();
    request["payload"]["status"] = "pending".into();
    events.push(request);
    events.push(thread("thread:parent", json!({})));
    events.push(thread("thread:child",json!({"lineage":{"parentThreadId":"thread:parent","relationshipToParent":"subagent","rootThreadId":"thread:parent"}})));
    events.push(thread("thread:orphan",json!({"lineage":{"parentThreadId":"thread:missing","relationshipToParent":"subagent","rootThreadId":"thread:parent"}})));
    apply_values(&db, events).await;
    let store = ProjectionStore::new(db.stores.clone());
    assert_eq!(
        ids(store.settlement_candidates(None).await.unwrap()),
        vec![
            "thread:completed",
            "thread:idle",
            "thread:orphan",
            "thread:parent",
            "thread:queued"
        ]
    );
    assert_eq!(
        ids(store
            .settlement_candidates(Some(ThreadId("thread:completed".into())))
            .await
            .unwrap()),
        vec!["thread:completed"]
    );
    for status in ["thread.settled", "thread.archived", "thread.deleted"] {
        let mut event = fixture(status, "thread:parent");
        event["threadId"] = "thread:parent".into();
        event["payload"] = thread(
            "thread:parent",
            match status {
                "thread.settled" => json!({"settledOverride":"settled","settledAt":NOW}),
                "thread.archived" => json!({"archivedAt":NOW}),
                _ => json!({"deletedAt":NOW}),
            },
        )["payload"]
            .clone();
        apply_values(&db, vec![event]).await;
        let candidates = ids(store.settlement_candidates(None).await.unwrap());
        assert!(candidates.contains(&"thread:child".to_owned()), "{status}");
        assert!(
            !candidates.contains(&"thread:parent".to_owned()),
            "{status}"
        );
    }
    let mut held = run("thread:held", "run:held", "queued");
    held["payload"]["ordinal"] = 2.into();
    held["payload"]["queueHeld"] = true.into();
    let mut provider = fixture("provider-thread.updated", "provider:held");
    provider["threadId"] = "thread:held".into();
    provider["payload"]["appThreadId"] = "thread:held".into();
    provider["payload"]["pendingBackgroundTasks"] =
        json!([{"taskId":"pending:held","kind":"command"}]);
    apply_values(
        &db,
        vec![
            thread("thread:held", json!({})),
            run("thread:held", "run:executed", "completed"),
            held,
            provider,
            thread("thread:rewound", json!({})),
            run("thread:rewound", "run:rewound", "rolled_back"),
        ],
    )
    .await;
    let held = store
        .settlement_candidates(Some(ThreadId("thread:held".into())))
        .await
        .unwrap();
    let held = t3_contracts::stored::to_value(&held[0]).unwrap();
    assert_eq!(held["latestRunId"], "run:held");
    assert_eq!(held["status"], "queued");
    assert_eq!(held["pendingBackgroundTasks"], json!([]));
    let rewound = store
        .settlement_candidates(Some(ThreadId("thread:rewound".into())))
        .await
        .unwrap();
    assert_eq!(
        rewound[0].status,
        OrchestrationV2ShellThreadStatus::RolledBack
    );
    db.stores
        .transaction(|tx| {
            tx.execute(
                "DROP TABLE orchestration_v2_projection_turn_item_counts",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        store
            .settlement_candidates(Some(ThreadId("thread:held".into())))
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn pending_approvals_take_priority_over_newer_questions_and_wake_snoozes() {
    use t3_orchestration::ports::thread_project::thread_settlement_service::is_snoozed;
    let db = TestDatabase::memory().unwrap();
    apply_values(
        &db,
        vec![thread(
            "thread:test",
            json!({"snoozedAt":NOW,"snoozedUntil":"2026-11-14T10:00:00.000Z"}),
        )],
    )
    .await;
    let store = ProjectionStore::new(db.stores.clone());
    let mut question = fixture("runtime-request.updated", "request:question");
    question["payload"]["kind"] = "user_input".into();
    question["payload"]["status"] = "pending".into();
    question["payload"]["responseCapability"] = json!({"type":"message"});
    let mut approval = question.clone();
    approval["payload"]["id"] = "request:approval".into();
    approval["payload"]["kind"] = "command".into();
    approval["payload"]["createdAt"] = "2026-11-01T10:00:00.000Z".into();
    apply_values(&db, vec![question]).await;
    let now = 1794564000000;
    let shell = store
        .shell(ThreadId("thread:test".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&shell.pending_runtime_request).unwrap()["id"],
        "request:question"
    );
    assert!(is_snoozed(&shell, now));
    apply_values(&db, vec![approval.clone()]).await;
    let shell = store
        .shell(ThreadId("thread:test".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&shell.pending_runtime_request).unwrap()["id"],
        "request:approval"
    );
    assert!(!is_snoozed(&shell, now));
    approval["payload"]["status"] = "resolved".into();
    approval["payload"]["resolvedAt"] = NOW.into();
    apply_values(&db, vec![approval]).await;
    let shell = store
        .shell(ThreadId("thread:test".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&shell.pending_runtime_request).unwrap()["id"],
        "request:question"
    );
    assert!(is_snoozed(&shell, now));
}

#[tokio::test]
async fn settlement_and_shell_dates_distinguish_user_messages_from_agent_notifications() {
    let db = TestDatabase::memory().unwrap();
    let mut written = fixture("message.updated", "message:written");
    written["payload"]["role"] = "user".into();
    written["payload"]["createdBy"] = "user".into();
    written["payload"]["updatedAt"] = "2026-11-13T08:00:00.000Z".into();
    let mut notification = written.clone();
    notification["payload"]["id"] = "message:notification".into();
    notification["payload"]["createdBy"] = "agent".into();
    notification["payload"]["creationSource"] = "provider".into();
    notification["payload"]["updatedAt"] = NOW.into();
    apply_values(
        &db,
        vec![
            thread("thread:test", json!({})),
            run("thread:test", "run:test", "completed"),
            written,
            notification,
        ],
    )
    .await;
    let store = ProjectionStore::new(db.stores.clone());
    for shell in [
        store
            .shell(ThreadId("thread:test".into()))
            .await
            .unwrap()
            .unwrap(),
        store.settlement_candidates(None).await.unwrap().remove(0),
    ] {
        let value = serde_json::to_value(shell).unwrap();
        assert_eq!(value["latestUserMessageAt"], NOW);
        assert_eq!(
            value["latestUserAuthoredMessageAt"],
            "2026-11-13T08:00:00.000Z"
        );
    }
}

#[tokio::test]
async fn pull_request_discovery_and_unsettled_shell_filters_keep_their_distinct_scopes() {
    let db = TestDatabase::memory().unwrap();
    let link = json!({"host":"github.com","repository":"owner/repo","number":1,"url":"https://github.com/owner/repo/pull/1","source":"manual","linkedAt":NOW,"snapshot":null,"stack":null});
    apply_values(&db,vec![thread("thread:empty",json!({"pullRequests":[]})),thread("thread:settled",json!({"pullRequests":[link.clone()],"settledOverride":"settled","settledAt":NOW,"updatedAt":"2026-11-01T10:00:00.000Z"})),thread("thread:open",json!({"pullRequests":[link.clone()]})),thread("thread:reopened",json!({"settledOverride":"active"})),thread("thread:auto",json!({"settledAt":NOW})),thread("thread:archived",json!({"archivedAt":NOW,"pullRequests":[link]}))]).await;
    let store = ProjectionStore::new(db.stores.clone());
    let linked = store.threads_with_pull_requests(None).await.unwrap();
    assert_eq!(
        linked
            .iter()
            .map(|t| serde_json::to_value(&t.id).unwrap())
            .collect::<Vec<_>>(),
        vec![json!("thread:settled"), json!("thread:open")]
    );
    assert_eq!(
        store
            .threads_with_pull_requests(Some(ThreadId("thread:open".into())))
            .await
            .unwrap()
            .len(),
        1
    );
    let shells = store
        .thread_shells(ShellQuery {
            unsettled_only: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        ids(shells.threads),
        vec!["thread:empty", "thread:open", "thread:reopened"]
    );
    assert!(shells.archived_threads.is_empty());
}

#[tokio::test]
async fn settlement_discovery_matches_shells_for_rosters_monitors_rollbacks_and_snoozes() {
    use t3_orchestration::{
        ports::thread_project::thread_settlement_service::{
            SettlementCandidate, is_auto_settlement_candidate, resolve_auto_settlement_at,
        },
        projections::read::shell_from_projection,
    };
    let db = TestDatabase::memory().unwrap();
    let mut events = Vec::new();
    for name in [
        "idle-work",
        "persistent",
        "rolled",
        "roster",
        "snoozed",
        "woke",
        "recent",
    ] {
        let id = format!("thread:{name}");
        let overrides = if matches!(name, "snoozed" | "woke") {
            json!({"snoozedAt":if name=="woke"{"2026-11-01T10:00:00.000Z"}else{NOW},"snoozedUntil":"2026-11-14T10:00:00.000Z"})
        } else {
            json!({})
        };
        events.push(thread(&id, overrides));
        events.push(run(
            &id,
            &format!("run:{name}"),
            if name == "woke" {
                "failed"
            } else if name == "rolled" {
                "rolled_back"
            } else {
                "completed"
            },
        ));
        if matches!(name, "idle-work" | "persistent" | "rolled") {
            let mut item = crate::projection_store_test::conversation_item(
                &format!("item:{name}"),
                Some(&format!("run:{name}")),
                1,
            );
            item["threadId"] = id.clone().into();
            item["payload"]["threadId"] = id.clone().into();
            item["payload"]["type"] = "dynamic_tool".into();
            item["payload"]["toolName"] = "server.watch".into();
            item["payload"]["input"] = json!({"persistent":name=="persistent"});
            item["payload"]["status"] = if name == "idle-work" {
                "idle"
            } else {
                "running"
            }
            .into();
            events.push(item);
        }
        if name == "rolled" {
            let mut completed = run(&id, "run:rolled:later", "completed");
            completed["payload"]["ordinal"] = 2.into();
            events.push(completed);
        }
        if name == "roster" {
            let mut provider = fixture("provider-thread.updated", "provider:roster");
            provider["threadId"] = id.clone().into();
            provider["payload"]["appThreadId"] = id.clone().into();
            provider["payload"]["status"] = "idle".into();
            provider["payload"]["pendingBackgroundTasks"] =
                json!([{"taskId":"running-task","kind":"subagent"}]);
            events.push(provider);
        }
        if name == "recent" {
            let mut message = fixture("message.updated", "message:recent");
            message["threadId"] = id.clone().into();
            message["payload"]["threadId"] = id.into();
            message["payload"]["role"] = "user".into();
            message["payload"]["createdBy"] = "user".into();
            message["payload"]["updatedAt"] = "2026-11-13T10:00:01.000Z".into();
            events.push(message);
        }
    }
    apply_values(&db, events).await;
    let store = ProjectionStore::new(db.stores.clone());
    let candidates = store.settlement_candidates(None).await.unwrap();
    let shells = store
        .thread_shells(ShellQuery::default())
        .await
        .unwrap()
        .threads;
    let eligible = |rows: &[OrchestrationV2ThreadShell]| {
        rows.iter()
            .filter(|t| is_auto_settlement_candidate(t, 1794564000000))
            .map(|t| t.id.0.clone())
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(
        eligible(&candidates),
        [
            "thread:idle-work",
            "thread:persistent",
            "thread:rolled",
            "thread:woke"
        ]
        .map(str::to_owned)
        .into_iter()
        .collect()
    );
    assert_eq!(eligible(&candidates), eligible(&shells));
    for candidate in candidates {
        let expected = shells.iter().find(|t| t.id == candidate.id).unwrap();
        assert_eq!(
            candidate.pending_background_tasks,
            expected.pending_background_tasks
        );
        assert_eq!(
            shell_from_projection(
                &store
                    .snapshot(candidate.id.clone(), None)
                    .await
                    .unwrap()
                    .projection
            )
            .unwrap(),
            *expected
        );
        let candidate = SettlementCandidate {
            thread: candidate,
            last_snooze_wake_at: None,
        };
        let expected = SettlementCandidate {
            thread: expected.clone(),
            last_snooze_wake_at: None,
        };
        assert_eq!(
            resolve_auto_settlement_at(&candidate, None, 1794564000000, Some(7.0), false, false),
            resolve_auto_settlement_at(&expected, None, 1794564000000, Some(7.0), false, false)
        );
    }
}
