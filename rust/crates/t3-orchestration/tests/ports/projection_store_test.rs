#![allow(clippy::unwrap_used)]
const EVENT_SAMPLES: &[(&str, &str)] = &[
    (
        "thread.created",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.created.jsonl"
        ),
    ),
    (
        "thread.archived",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.archived.jsonl"
        ),
    ),
    (
        "thread.unarchived",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.unarchived.jsonl"
        ),
    ),
    (
        "thread.deleted",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.deleted.jsonl"
        ),
    ),
    (
        "thread.settled",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.settled.jsonl"
        ),
    ),
    (
        "thread.unsettled",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.unsettled.jsonl"
        ),
    ),
    (
        "thread.snoozed",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.snoozed.jsonl"
        ),
    ),
    (
        "thread.unsnoozed",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.unsnoozed.jsonl"
        ),
    ),
    (
        "thread.pinned",
        include_str!("../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.pinned.jsonl"),
    ),
    (
        "thread.auto-settle-set",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.auto-settle-set.jsonl"
        ),
    ),
    (
        "thread.unpinned",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.unpinned.jsonl"
        ),
    ),
    (
        "thread.pin-reordered",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.pin-reordered.jsonl"
        ),
    ),
    (
        "thread.active-reordered",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.active-reordered.jsonl"
        ),
    ),
    (
        "thread.tags-updated",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.tags-updated.jsonl"
        ),
    ),
    (
        "thread.visit-recorded",
        include_str!(
            "../../../../fixtures/stored/OrchestrationV2DomainEventJson.thread.visit-recorded.jsonl"
        ),
    ),
    (
        "thread.visited",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.visited.jsonl"
        ),
    ),
    (
        "thread.marked-unread",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.marked-unread.jsonl"
        ),
    ),
    (
        "thread.metadata-updated",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.metadata-updated.jsonl"
        ),
    ),
    (
        "thread.pull-request-synced",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.pull-request-synced.jsonl"
        ),
    ),
    (
        "thread.runtime-mode-updated",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.runtime-mode-updated.jsonl"
        ),
    ),
    (
        "thread.interaction-mode-updated",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.interaction-mode-updated.jsonl"
        ),
    ),
    (
        "thread.model-selection-updated",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.model-selection-updated.jsonl"
        ),
    ),
    (
        "thread.provider-switched",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.provider-switched.jsonl"
        ),
    ),
    (
        "thread.guest-share-updated",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.guest-share-updated.jsonl"
        ),
    ),
    (
        "thread.pull-request-link-updated",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.pull-request-link-updated.jsonl"
        ),
    ),
    (
        "run.created",
        include_str!("../../../../fixtures/samples/OrchestrationV2DomainEvent.run.created.jsonl"),
    ),
    (
        "run.updated",
        include_str!("../../../../fixtures/samples/OrchestrationV2DomainEvent.run.updated.jsonl"),
    ),
    (
        "run.background-work-cancelled",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.run.background-work-cancelled.jsonl"
        ),
    ),
    (
        "run-attempt.created",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.run-attempt.created.jsonl"
        ),
    ),
    (
        "run-attempt.updated",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.run-attempt.updated.jsonl"
        ),
    ),
    (
        "node.updated",
        include_str!("../../../../fixtures/samples/OrchestrationV2DomainEvent.node.updated.jsonl"),
    ),
    (
        "subagent.updated",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.subagent.updated.jsonl"
        ),
    ),
    (
        "provider-session.attached",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.provider-session.attached.jsonl"
        ),
    ),
    (
        "provider-session.updated",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.provider-session.updated.jsonl"
        ),
    ),
    (
        "provider-session.detached",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.provider-session.detached.jsonl"
        ),
    ),
    (
        "provider-thread.updated",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.provider-thread.updated.jsonl"
        ),
    ),
    (
        "provider-turn.updated",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.provider-turn.updated.jsonl"
        ),
    ),
    (
        "runtime-request.updated",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.runtime-request.updated.jsonl"
        ),
    ),
    (
        "message.updated",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.message.updated.jsonl"
        ),
    ),
    (
        "turn-item.updated",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.turn-item.updated.jsonl"
        ),
    ),
    (
        "plan.updated",
        include_str!("../../../../fixtures/samples/OrchestrationV2DomainEvent.plan.updated.jsonl"),
    ),
    (
        "checkpoint-scope.created",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.checkpoint-scope.created.jsonl"
        ),
    ),
    (
        "checkpoint.captured",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.checkpoint.captured.jsonl"
        ),
    ),
    (
        "checkpoint.rollback-requested",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.checkpoint.rollback-requested.jsonl"
        ),
    ),
    (
        "context-handoff.updated",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.context-handoff.updated.jsonl"
        ),
    ),
    (
        "context-transfer.created",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.context-transfer.created.jsonl"
        ),
    ),
    (
        "context-transfer.updated",
        include_str!(
            "../../../../fixtures/samples/OrchestrationV2DomainEvent.context-transfer.updated.jsonl"
        ),
    ),
];
use serde_json::Value;
use std::collections::BTreeSet;
use t3_contracts::orchestration_v2::{OrchestrationV2DomainEvent, OrchestrationV2StoredEvent};
use t3_orchestration::{projections::write, testing::TestDatabase};

#[tokio::test]
async fn every_fixture_event_variant_applies_to_migrated_sqlite() {
    let manifest: Value =
        serde_json::from_str(include_str!("../../../../fixtures/orchestration.json")).unwrap();
    let expected: BTreeSet<_> = manifest["OrchestrationV2DomainEvent"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["tag"].as_str().unwrap())
        .collect();
    let cases = EVENT_SAMPLES;
    assert_eq!(expected, cases.iter().map(|(tag, _)| *tag).collect());
    let oracle: Value = serde_json::from_str(TYPESCRIPT_PROJECTION_ROWS).unwrap();
    for &(tag, samples) in cases {
        let db = TestDatabase::memory().unwrap();
        let mut seed: Value = serde_json::from_str(
            include_str!(
                "../../../../fixtures/samples/OrchestrationV2DomainEvent.thread.created.jsonl"
            )
            .lines()
            .next()
            .unwrap(),
        )
        .unwrap();
        let raw: Value = serde_json::from_str(samples.lines().next().unwrap()).unwrap();
        seed["threadId"] = raw["threadId"].clone();
        seed["payload"]["id"] = raw["threadId"].clone();
        let event: OrchestrationV2DomainEvent = serde_json::from_value(raw.clone()).unwrap();
        let seed: OrchestrationV2DomainEvent = serde_json::from_value(seed).unwrap();
        db.stores
            .transaction(move |tx| {
                write::apply(
                    tx,
                    &OrchestrationV2StoredEvent {
                        sequence: 1,
                        command_id: None,
                        event: seed,
                    },
                )?;
                write::apply(
                    tx,
                    &OrchestrationV2StoredEvent {
                        sequence: 2,
                        command_id: None,
                        event,
                    },
                )
            })
            .await
            .unwrap_or_else(|error| panic!("{tag}: {error}"));
        let expected_rows = oracle[tag].clone();
        let thread_id = raw["threadId"].as_str().unwrap().to_owned();
        db.stores
            .read(move |conn| {
                let count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM orchestration_v2_projection_threads WHERE thread_id=?1",
                    [thread_id],
                    |row| row.get(0),
                )?;
                assert_eq!(count, 1);
                for (table, expected) in expected_rows.as_object().unwrap() {
                    let mut statement = conn.prepare(&format!(
                        "SELECT * FROM orchestration_v2_projection_{table}"
                    ))?;
                    let mut query = statement.query([])?;
                    let mut actual = Vec::new();
                    while let Some(row) = query.next()? {
                        let mut value = serde_json::Map::new();
                        for (index, name) in row.as_ref().column_names().iter().enumerate() {
                            use t3_persistence::rusqlite::types::ValueRef;
                            let field = match row.get_ref(index)? {
                                ValueRef::Null => Value::Null,
                                ValueRef::Integer(value) => value.into(),
                                ValueRef::Real(value) => serde_json::json!(value),
                                ValueRef::Text(value) => {
                                    String::from_utf8(value.to_vec()).unwrap().into()
                                }
                                ValueRef::Blob(_) => panic!("unexpected projection blob"),
                            };
                            value.insert((*name).into(), field);
                        }
                        actual.push(Value::Object(value));
                    }
                    let sorted = |values: &[Value]| {
                        let mut values = values
                            .iter()
                            .map(|value| serde_json::to_string(value).unwrap())
                            .collect::<Vec<_>>();
                        values.sort();
                        values
                    };
                    assert_eq!(
                        sorted(&actual),
                        sorted(expected.as_array().unwrap()),
                        "{tag}: {table}"
                    );
                }
                Ok(())
            })
            .await
            .unwrap();
    }
}

pub fn sample(tag: &str) -> Value {
    serde_json::from_str(
        EVENT_SAMPLES
            .iter()
            .find(|(name, _)| *name == tag)
            .unwrap()
            .1
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap()
}
pub const NOW: &str = "2026-11-13T10:00:00.000Z";
pub fn fixture(tag: &str, id: &str) -> Value {
    let mut event = sample(tag);
    event["id"] = format!("event:{tag}:{id}").into();
    event["threadId"] = "thread:test".into();
    event["occurredAt"] = NOW.into();
    let payload = &mut event["payload"];
    if payload.get("id").is_some() {
        payload["id"] = id.into();
    }
    if payload.get("threadId").is_some() {
        payload["threadId"] = "thread:test".into();
    }
    for field in ["createdAt", "updatedAt", "requestedAt"] {
        if payload.get(field).is_some() {
            payload[field] = NOW.into();
        }
    }
    match tag {
        "thread.created" => {
            payload["lineage"] = serde_json::json!({"rootThreadId":id,"parentThreadId":null,"relationshipToParent":null});
            payload["forkedFrom"] = Value::Null;
            payload["settledOverride"] = Value::Null;
            payload["settledAt"] = Value::Null;
            payload["activeProviderThreadId"] = Value::Null;
        }
        "run.created" | "run.updated" => {
            payload["providerThreadId"] = "provider:test".into();
            payload["userMessageId"] = "message:test".into();
            payload["rootNodeId"] = "node:test".into();
            payload["activeAttemptId"] = "attempt:test".into();
            payload["status"] = "running".into();
            payload["startedAt"] = NOW.into();
            payload["contextHandoffId"] = Value::Null;
        }
        "run-attempt.created" => {
            payload["runId"] = "run:test".into();
            payload["rootNodeId"] = "node:test".into();
            payload["providerThreadId"] = "provider:test".into();
            payload["providerTurnId"] = "turn:test".into();
            payload["attemptOrdinal"] = 1.into();
            payload["completedAt"] = Value::Null;
        }
        "provider-thread.updated" => {
            payload["appThreadId"] = "thread:test".into();
            payload["providerSessionId"] = "session:test".into();
            payload["ownerNodeId"] = Value::Null;
            payload["status"] = "active".into();
            payload["firstRunOrdinal"] = 1.into();
        }
        "provider-turn.updated" => {
            payload["providerThreadId"] = "provider:test".into();
            payload["nodeId"] = "node:test".into();
            payload["runAttemptId"] = "attempt:test".into();
            payload["completedAt"] = Value::Null;
        }
        "node.updated" => {
            payload["runId"] = "run:test".into();
            payload["rootNodeId"] = "node:test".into();
            payload["providerThreadId"] = "provider:test".into();
            payload["providerTurnId"] = "turn:test".into();
            payload["kind"] = "root_turn".into();
            payload["status"] = "running".into();
            payload["completedAt"] = Value::Null;
        }
        "message.updated" => {
            payload["runId"] = "run:test".into();
            payload["nodeId"] = "node:test".into();
            payload["text"] = "hello projection".into();
            payload["streaming"] = false.into();
        }
        "runtime-request.updated" => {
            payload["nodeId"] = "node:test".into();
            payload["providerTurnId"] = "turn:test".into();
            payload["resolvedAt"] = Value::Null;
            payload["responseCapability"] =
                serde_json::json!({"type":"live","providerSessionId":"session:test"});
        }
        _ => {}
    }
    event
}
pub async fn apply_values(db: &TestDatabase, values: Vec<Value>) {
    db.stores
        .transaction(move |tx| {
            for (index, value) in values.into_iter().enumerate() {
                write::apply(
                    tx,
                    &OrchestrationV2StoredEvent {
                        sequence: index as u64 + 1,
                        command_id: None,
                        event: serde_json::from_value(value)?,
                    },
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn record_queries_filter_messages_and_statuses_before_decoding() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    let mut monitor = fixture("message.updated", "message:monitor");
    monitor["payload"]["role"] = "user".into();
    monitor["payload"]["notification"] = serde_json::json!({"source":{"kind":"monitor"},"outcome":"updated","summary":"monitor update"});
    let mut completed = fixture("run.created", "run:completed");
    completed["payload"]["ordinal"] = 2.into();
    completed["payload"]["status"] = "completed".into();
    let mut cancelled = fixture("run.created", "run:cancelled");
    cancelled["payload"]["ordinal"] = 3.into();
    cancelled["payload"]["status"] = "cancelled".into();
    cancelled["payload"]["startedAt"] = Value::Null;
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            fixture("run.created", "run:test"),
            completed,
            cancelled,
            fixture("message.updated", "message:test"),
            monitor,
        ],
    )
    .await;
    let store = ProjectionStore::new(db.stores.clone());
    let messages = MessageFilter {
        ids: Some(vec![MessageId("message:monitor".into())]),
        roles: Some(vec![ConversationMessageRole::User]),
        run_ids: Some(vec![RunId("run:test".into())]),
        monitor_notifications_only: true,
    };
    assert_eq!(
        ProjectionReader::message_count(&store, ThreadId("thread:test".into()), messages.clone())
            .await
            .unwrap(),
        1
    );
    let query = ProjectionRecordQuery {
        thread_id: ThreadId("thread:test".into()),
        fields: vec![RecordKind::Runs, RecordKind::Messages],
        filter: RecordFilter::default(),
        messages,
        run_statuses: Some(vec![OrchestrationV2RunStatus::Running]),
        include_latest_run: true,
    };
    let records = store.query_records(query.clone()).await.unwrap();
    assert_eq!(records.records.len(), 2);
    let ProjectionRecordSet::Runs(runs) = &records.records[0] else {
        panic!("runs")
    };
    assert_eq!(
        runs.iter()
            .map(|r| serde_json::to_value(&r.id).unwrap())
            .collect::<Vec<_>>(),
        vec![
            serde_json::json!("run:test"),
            serde_json::json!("run:completed")
        ]
    );
    let ProjectionRecordSet::Messages(messages) = &records.records[1] else {
        panic!("messages")
    };
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text, "hello projection");
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_messages SET payload_json='{}' WHERE message_id='message:test'",[])?;Ok(())}).await.unwrap();
    assert!(store.query_records(query.clone()).await.is_ok());
    let result = store
        .query_records(ProjectionRecordQuery {
            messages: MessageFilter {
                ids: Some(vec![]),
                ..Default::default()
            },
            run_statuses: Some(vec![]),
            include_latest_run: false,
            ..query
        })
        .await
        .unwrap();
    assert!(matches!(&result.records[0],ProjectionRecordSet::Runs(values) if values.is_empty()));
    assert!(
        matches!(&result.records[1],ProjectionRecordSet::Messages(values) if values.is_empty())
    );
    let mut first = conversation_item("filter:first", Some("run:test"), 1);
    first["payload"]["nodeId"] = "node:first".into();
    let mut other = conversation_item("filter:other", Some("run:other"), 2);
    other["payload"]["nodeId"] = "node:other".into();
    let mut runless = conversation_item("filter:runless", None, 3);
    runless["payload"]["nodeId"] = "node:first".into();
    let mut second_attempt = fixture("run-attempt.created", "attempt:other");
    second_attempt["payload"]["attemptOrdinal"] = 2.into();
    let mut second_turn = fixture("provider-turn.updated", "turn:other");
    second_turn["payload"]["runAttemptId"] = "attempt:other".into();
    second_turn["payload"]["ordinal"] = 2.into();
    apply_values(
        &db,
        vec![
            first,
            other,
            runless,
            fixture("run-attempt.created", "attempt:test"),
            second_attempt,
            fixture("provider-turn.updated", "turn:test"),
            second_turn,
        ],
    )
    .await;
    for (filter, expected) in [
        (
            RecordFilter {
                turn_item_run_id: Some(Some(RunId("run:test".into()))),
                ..Default::default()
            },
            vec!["filter:first"],
        ),
        (
            RecordFilter {
                turn_item_run_id: Some(None),
                ..Default::default()
            },
            vec!["filter:runless"],
        ),
        (
            RecordFilter {
                turn_item_run_ids: Some(vec![Some(RunId("run:other".into())), None]),
                ..Default::default()
            },
            vec!["filter:other", "filter:runless"],
        ),
        (
            RecordFilter {
                turn_item_node_ids: Some(vec![NodeId("node:first".into())]),
                ..Default::default()
            },
            vec!["filter:first", "filter:runless"],
        ),
        (
            RecordFilter {
                turn_item_node_ids: Some(vec![NodeId("node:other".into())]),
                turn_item_run_ids: Some(vec![None]),
                ..Default::default()
            },
            vec![],
        ),
        (
            RecordFilter {
                turn_item_run_ids: Some(vec![]),
                ..Default::default()
            },
            vec![],
        ),
        (
            RecordFilter {
                turn_item_node_ids: Some(vec![]),
                ..Default::default()
            },
            vec![],
        ),
    ] {
        let records = store
            .records(
                ThreadId("thread:test".into()),
                vec![RecordKind::TurnItems],
                filter,
            )
            .await
            .unwrap();
        let ProjectionRecordSet::TurnItems(items) = &records.records[0] else {
            panic!("turn items");
        };
        assert_eq!(
            items
                .iter()
                .map(|i| t3_contracts::stored::to_value(i).unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_owned())
                .collect::<Vec<_>>(),
            expected
        );
    }
    let records = store
        .records(
            ThreadId("thread:test".into()),
            vec![RecordKind::Attempts, RecordKind::ProviderTurns],
            RecordFilter {
                attempt_ids: Some(vec![RunAttemptId("attempt:test".into())]),
                provider_turn_attempt_ids: Some(vec![RunAttemptId("attempt:other".into())]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let ProjectionRecordSet::Attempts(attempts) = &records.records[0] else {
        panic!("attempts");
    };
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        t3_contracts::stored::to_value(&attempts[0]).unwrap()["id"],
        "attempt:test"
    );
    let ProjectionRecordSet::ProviderTurns(turns) = &records.records[1] else {
        panic!("turns");
    };
    assert_eq!(turns.len(), 1);
    assert_eq!(
        t3_contracts::stored::to_value(&turns[0]).unwrap()["id"],
        "turn:other"
    );
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_turn_items SET payload_json='{}' WHERE turn_item_id='filter:other'",[])?;Ok(())}).await.unwrap();
    assert!(
        store
            .records(
                ThreadId("thread:test".into()),
                vec![RecordKind::TurnItems],
                RecordFilter {
                    turn_item_run_ids: Some(vec![None]),
                    ..Default::default()
                }
            )
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn command_record_filters_keep_active_leaf_work_and_unsettled_subagent_owners() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    let mut old = fixture("run.created", "run:done");
    old["payload"]["status"] = "completed".into();
    old["payload"]["ordinal"] = 1.into();
    let mut live = fixture("run.created", "run:live");
    live["payload"]["ordinal"] = 2.into();
    let mut events = vec![fixture("thread.created", "thread:test"), old, live];
    for (id, run, kind, status) in [
        ("old-root", "run:done", "root_turn", "completed"),
        ("old-tool", "run:done", "tool_call", "completed"),
        ("old-message", "run:done", "assistant_message", "failed"),
        ("old-reasoning", "run:done", "reasoning", "cancelled"),
        ("old-subagent", "run:done", "subagent", "completed"),
        ("old-live", "run:done", "tool_call", "running"),
        ("old-todo", "run:done", "todo_list", "running"),
        ("active-tool", "run:live", "tool_call", "completed"),
        ("active-todo", "run:live", "todo_list", "running"),
    ] {
        let mut node = fixture("node.updated", id);
        node["payload"]["runId"] = run.into();
        node["payload"]["kind"] = kind.into();
        node["payload"]["status"] = status.into();
        events.push(node);
    }
    let tasks = [
        ("delivered", "run:done", "completed", Some("delivered")),
        ("acknowledged", "run:done", "failed", Some("acknowledged")),
        ("disposed", "run:done", "cancelled", Some("disposed")),
        ("pending-delivery", "run:done", "completed", Some("pending")),
        ("claimed", "run:done", "completed", Some("claimed")),
        ("no-delivery", "run:done", "interrupted", None),
        ("running", "run:done", "running", None),
        (
            "active-run-delivered",
            "run:live",
            "completed",
            Some("delivered"),
        ),
    ];
    for (id, run, status, delivery) in tasks {
        let mut task = fixture("subagent.updated", id);
        task["payload"]["runId"] = run.into();
        task["payload"]["origin"] = "app_owned".into();
        task["payload"]["status"] = status.into();
        task["payload"]["providerThreadId"] = format!("provider:{id}").into();
        if let Some(delivery) = delivery {
            task["payload"]["completionDelivery"] =
                serde_json::json!({"state":delivery,"observedByRunId":null});
        }
        let mut node = fixture("node.updated", id);
        node["payload"]["runId"] = run.into();
        node["payload"]["kind"] = "subagent".into();
        node["payload"]["status"] = status.into();
        let mut provider = fixture("provider-thread.updated", &format!("provider:{id}"));
        provider["payload"]["appThreadId"] = format!("child:{id}").into();
        provider["payload"]["ownerNodeId"] = Value::Null;
        provider["payload"]["providerSessionId"] = if id == "acknowledged" {
            "session:shared".into()
        } else {
            Value::Null
        };
        events.extend([task, node, provider]);
    }
    apply_values(&db, events).await;
    db.stores.transaction(|tx|{tx.execute("INSERT INTO orchestration_v2_projection_provider_session_bindings(provider_session_id,thread_id) VALUES('session:shared','thread:test')",[])?;Ok(())}).await.unwrap();
    let store = ProjectionStore::new(db.stores.clone());
    let thread = ThreadId("thread:test".into());
    let all = store
        .records(
            thread.clone(),
            vec![RecordKind::Nodes],
            RecordFilter::default(),
        )
        .await
        .unwrap();
    let ids = |records: ProjectionRecords| {
        records
            .records
            .into_iter()
            .flat_map(|set| match set {
                ProjectionRecordSet::Nodes(v) => v
                    .into_iter()
                    .map(|v| {
                        t3_contracts::stored::to_value(&v.id)
                            .unwrap()
                            .as_str()
                            .unwrap()
                            .to_owned()
                    })
                    .collect::<Vec<_>>(),
                ProjectionRecordSet::Subagents(v) => v
                    .into_iter()
                    .map(|v| {
                        t3_contracts::stored::to_value(&v.id)
                            .unwrap()
                            .as_str()
                            .unwrap()
                            .to_owned()
                    })
                    .collect(),
                ProjectionRecordSet::ProviderThreads(v) => v
                    .into_iter()
                    .map(|v| {
                        t3_contracts::stored::to_value(&v.id)
                            .unwrap()
                            .as_str()
                            .unwrap()
                            .to_owned()
                    })
                    .collect(),
                _ => panic!("unexpected records"),
            })
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(
        ids(store
            .records(
                thread.clone(),
                vec![RecordKind::Nodes],
                RecordFilter {
                    omit_settled_subagent_records: true,
                    ..Default::default()
                }
            )
            .await
            .unwrap()),
        ids(all)
    );
    let filter = RecordFilter {
        omit_settled_leaf_nodes: true,
        omit_settled_subagent_records: true,
        ..Default::default()
    };
    let expected = [
        "active-run-delivered",
        "claimed",
        "no-delivery",
        "pending-delivery",
        "running",
    ];
    let mut expected_nodes = expected
        .map(str::to_owned)
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    expected_nodes.extend(
        [
            "old-root",
            "old-subagent",
            "old-live",
            "old-todo",
            "active-tool",
            "active-todo",
        ]
        .map(str::to_owned),
    );
    assert_eq!(
        ids(store
            .records(thread.clone(), vec![RecordKind::Nodes], filter.clone())
            .await
            .unwrap()),
        expected_nodes
    );
    let mut expected_providers = expected
        .map(|id| format!("provider:{id}"))
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    expected_providers.insert("provider:acknowledged".into());
    assert_eq!(
        ids(store
            .records(
                thread.clone(),
                vec![RecordKind::ProviderThreads],
                filter.clone()
            )
            .await
            .unwrap()),
        expected_providers
    );
    assert_eq!(
        ids(store
            .records(
                thread.clone(),
                vec![RecordKind::Subagents],
                RecordFilter {
                    omit_settled_subagents: true,
                    ..Default::default()
                }
            )
            .await
            .unwrap()),
        expected.map(str::to_owned).into_iter().collect()
    );
    let with_exception = ids(store
        .records(
            thread.clone(),
            vec![RecordKind::Subagents],
            RecordFilter {
                omit_settled_subagents: true,
                except_subagent_ids: vec![NodeId("delivered".into())],
                ..Default::default()
            },
        )
        .await
        .unwrap());
    assert!(with_exception.contains("delivered"));
    assert_eq!(with_exception.len(), 6);
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_subagents SET payload_json='{}' WHERE subagent_id IN('delivered','acknowledged','disposed')",[])?;tx.execute("UPDATE orchestration_v2_projection_nodes SET payload_json='{}' WHERE node_id IN('delivered','acknowledged','disposed','old-tool','old-message','old-reasoning')",[])?;tx.execute("UPDATE orchestration_v2_projection_provider_threads SET payload_json='{}' WHERE provider_thread_id IN('provider:delivered','provider:disposed')",[])?;Ok(())}).await.unwrap();
    assert_eq!(
        ids(store
            .records(thread, vec![RecordKind::Nodes], filter)
            .await
            .unwrap()),
        expected_nodes
    );
}

pub fn conversation_item(id: &str, run_id: Option<&str>, ordinal: u64) -> Value {
    let mut event = fixture("turn-item.updated", id);
    event["payload"] = serde_json::json!({"id":id,"threadId":"thread:test","runId":run_id,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":ordinal,"status":"completed","title":null,"startedAt":null,"completedAt":NOW,"updatedAt":NOW,"type":"user_message","messageId":format!("message:{id}"),"text":id,"attachments":[],"inputIntent":"turn_start","createdBy":"user","creationSource":"web"});
    event
}

#[tokio::test]
async fn title_and_link_metadata_queries_ignore_payloads_and_bound_variable_limits() {
    use t3_contracts::base_schemas::ThreadId;
    use t3_orchestration::projections::store::ProjectionStore;
    let db = TestDatabase::memory().unwrap();
    let mut archived = fixture("thread.created", "thread:archived");
    archived["threadId"] = "thread:archived".into();
    archived["payload"]["archivedAt"] = NOW.into();
    let mut deleted = fixture("thread.created", "thread:deleted");
    deleted["threadId"] = "thread:deleted".into();
    deleted["payload"]["deletedAt"] = NOW.into();
    apply_values(
        &db,
        vec![fixture("thread.created", "thread:test"), archived, deleted],
    )
    .await;
    let expected=db.stores.read(|db|{
        let title=db.query_row("SELECT title FROM orchestration_v2_projection_threads WHERE thread_id='thread:test'",[],|r|r.get::<_,String>(0))?;
        Ok(title)
    }).await.unwrap();
    db.stores
        .transaction(|tx| {
            tx.execute(
                "UPDATE orchestration_v2_projection_threads SET payload_json='not json'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let concrete = ProjectionStore::new(db.stores);
    let store: &dyn t3_orchestration::api::ProjectionReader = &concrete;
    let mut ids = (0..1200)
        .map(|n| ThreadId(format!("missing:{n}")))
        .collect::<Vec<_>>();
    ids.extend([
        ThreadId("thread:test".into()),
        ThreadId("thread:archived".into()),
        ThreadId("thread:deleted".into()),
        ThreadId("thread:test".into()),
    ]);
    let titles = store.thread_titles(ids.clone()).await.unwrap();
    assert_eq!(titles.len(), 2);
    assert_eq!(titles[&ThreadId("thread:test".into())], expected);
    let metadata = store.thread_link_metadata(ids).await.unwrap();
    assert_eq!(metadata.len(), 2);
    assert_eq!(metadata[&ThreadId("thread:test".into())].title, expected);
    assert!(
        metadata[&ThreadId("thread:test".into())]
            .project_id
            .is_some()
    );
    assert!(store.thread_titles(vec![]).await.unwrap().is_empty());
    assert!(store.thread_link_metadata(vec![]).await.unwrap().is_empty());
}

#[tokio::test]
async fn compaction_filters_keep_overlapping_and_explicit_runs_without_narrowing_items_or_runs() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    let mut events = vec![fixture("thread.created", "thread:test")];
    let names = ["old", "retry", "overlap", "compact", "later"];
    for (index, name) in names.iter().enumerate() {
        let run_id = format!("run:{name}");
        let node_id = format!("node:{name}");
        let attempt_id = format!("attempt:{name}");
        let mut run = fixture("run.created", &run_id);
        run["payload"]["ordinal"] = (index + 1).into();
        run["payload"]["status"] = "completed".into();
        run["payload"]["rootNodeId"] = node_id.clone().into();
        run["payload"]["completedAt"] = if index < 2 {
            "2026-11-13T08:00:00.000Z"
        } else {
            "2026-11-13T11:00:00.000Z"
        }
        .into();
        let mut attempt = fixture("run-attempt.created", &attempt_id);
        attempt["payload"]["runId"] = run_id.clone().into();
        attempt["payload"]["rootNodeId"] = node_id.clone().into();
        let mut node = fixture("node.updated", &node_id);
        node["payload"]["runId"] = run_id.clone().into();
        let mut message = fixture("message.updated", &format!("message:{name}"));
        message["payload"]["runId"] = run_id.clone().into();
        let mut turn = fixture("provider-turn.updated", &format!("turn:{name}"));
        turn["payload"]["runAttemptId"] = attempt_id.into();
        turn["payload"]["nodeId"] = node_id.into();
        turn["payload"]["ordinal"] = (index + 1).into();
        events.extend([
            run,
            attempt,
            node,
            message,
            turn,
            conversation_item(&format!("item:{name}"), Some(&run_id), index as u64),
        ]);
    }
    let mut compact = conversation_item("compaction", Some("run:compact"), 100);
    compact["payload"]["type"] = "compaction".into();
    compact["payload"]["driver"] = "codex".into();
    events.push(compact);
    apply_values(&db, events).await;
    let store = ProjectionStore::new(db.stores.clone());
    for (keep_runs, keep_message, expected) in [
        (vec![], "absent", vec!["compact", "later", "overlap"]),
        (
            vec![RunId("run:old".into())],
            "message:retry",
            vec!["compact", "later", "old", "overlap", "retry"],
        ),
    ] {
        let query = store
            .records(
                ThreadId("thread:test".into()),
                vec![
                    RecordKind::Runs,
                    RecordKind::TurnItems,
                    RecordKind::Attempts,
                    RecordKind::Nodes,
                    RecordKind::Messages,
                    RecordKind::ProviderTurns,
                ],
                RecordFilter {
                    since_last_compaction: Some((keep_runs, MessageId(keep_message.into()))),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        for records in query.records {
            let ids = match records {
                ProjectionRecordSet::Runs(values) => {
                    assert_eq!(values.len(), 5);
                    continue;
                }
                ProjectionRecordSet::TurnItems(values) => {
                    assert_eq!(values.len(), 6);
                    continue;
                }
                ProjectionRecordSet::Attempts(values) => values
                    .into_iter()
                    .map(|v| {
                        t3_contracts::stored::to_value(&v.id)
                            .unwrap()
                            .as_str()
                            .unwrap()
                            .to_owned()
                    })
                    .collect::<Vec<_>>(),
                ProjectionRecordSet::Nodes(values) => values
                    .into_iter()
                    .map(|v| {
                        t3_contracts::stored::to_value(&v.id)
                            .unwrap()
                            .as_str()
                            .unwrap()
                            .to_owned()
                    })
                    .collect(),
                ProjectionRecordSet::Messages(values) => values
                    .into_iter()
                    .map(|v| {
                        t3_contracts::stored::to_value(&v.id)
                            .unwrap()
                            .as_str()
                            .unwrap()
                            .to_owned()
                    })
                    .collect(),
                ProjectionRecordSet::ProviderTurns(values) => values
                    .into_iter()
                    .map(|v| {
                        t3_contracts::stored::to_value(&v.id)
                            .unwrap()
                            .as_str()
                            .unwrap()
                            .to_owned()
                    })
                    .collect(),
                _ => panic!("unexpected records"),
            };
            assert_eq!(
                ids.iter()
                    .map(|id| id.split(':').next_back().unwrap())
                    .collect::<std::collections::BTreeSet<_>>(),
                expected.iter().copied().collect()
            );
        }
    }
}

#[tokio::test]
async fn message_ties_match_node_locale_collation_for_mixed_case_and_punctuation() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    let mut events = vec![fixture("thread.created", "thread:test")];
    for id in [
        "message:a/1",
        "message:a.1",
        "message:a:1",
        "message:a-1",
        "message:a_1",
        "message:A",
        "message:a",
        "message:a2",
        "message:a10",
        "message:a1",
    ] {
        events.push(fixture("message.updated", id));
    }
    apply_values(&db, events).await;
    let result = ProjectionStore::new(db.stores)
        .records(
            ThreadId("thread:test".into()),
            vec![RecordKind::Messages],
            RecordFilter::default(),
        )
        .await
        .unwrap();
    let ProjectionRecordSet::Messages(messages) = &result.records[0] else {
        panic!("messages")
    };
    assert_eq!(
        messages
            .iter()
            .map(|m| serde_json::to_value(&m.id)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned())
            .collect::<Vec<_>>(),
        [
            "message:a",
            "message:A",
            "message:a_1",
            "message:a-1",
            "message:a:1",
            "message:a.1",
            "message:a/1",
            "message:a1",
            "message:a10",
            "message:a2"
        ]
    );
}

#[tokio::test]
async fn limit_recovery_obeys_root_errors_binding_precedence_due_times_and_explicit_choices() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    let thread = ThreadId("thread:test".into());
    let now = t3_contracts::wire::WireDateTime(NOW.into());
    let reset = "2099-01-01T00:00:00.000Z";
    let due = t3_contracts::wire::WireDateTime(reset.into());
    let mut run = fixture("run.created", "run:test");
    run["payload"]["ordinal"] = 1.into();
    run["payload"]["status"] = "failed".into();
    run["payload"]["completedAt"] = NOW.into();
    let mut error = conversation_item("item:limit", Some("run:test"), 2);
    error["payload"]["type"] = "error".into();
    error["payload"]["nodeId"] = "node:test".into();
    error["payload"]["status"] = "failed".into();
    error["payload"]["failure"] = serde_json::json!({"class":"usage_limit","message":"Plan limit reached.","resetAt":reset,"code":"usageLimitExceeded","retryable":null});
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            run.clone(),
            error.clone(),
        ],
    )
    .await;
    let store = ProjectionStore::new(db.stores.clone());
    let candidates = store
        .limit_recovery_candidates(now.clone(), true, false)
        .await
        .unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].latest_run_id, RunId("run:test".into()));
    let shell = serde_json::to_value(store.shell(thread.clone()).await.unwrap().unwrap()).unwrap();
    assert_eq!(
        t3_orchestration::projections::read::shell_from_projection(
            &store
                .snapshot(thread.clone(), None)
                .await
                .unwrap()
                .projection
        )
        .unwrap(),
        store.shell(thread.clone()).await.unwrap().unwrap()
    );
    let candidate = serde_json::to_value(&candidates[0]).unwrap();
    for field in [
        "id",
        "status",
        "lastErrorClass",
        "usageLimitResetAt",
        "latestRunId",
        "latestRunCompletedAt",
        "updatedAt",
        "archivedAt",
        "settledOverride",
        "pendingRuntimeRequest",
        "limitRecovery",
        "snoozedUntil",
    ] {
        assert_eq!(candidate[field], shell[field], "{field}");
    }
    assert!(
        store
            .limit_recovery_candidates(now.clone(), false, false)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .limit_recovery_candidates(now.clone(), false, true)
            .await
            .unwrap()
            .len(),
        1
    );
    for (id, status, ordinal, completed) in [
        ("run:queued", "queued", 2, None),
        ("run:cancelled", "cancelled", 3, Some(NOW)),
        (
            "run:ahead",
            "completed",
            4,
            Some("2026-11-13T09:55:00.000Z"),
        ),
    ] {
        let mut later = run.clone();
        later["payload"]["id"] = id.into();
        later["payload"]["ordinal"] = ordinal.into();
        later["payload"]["status"] = status.into();
        later["payload"]["completedAt"] = serde_json::json!(completed);
        if status != "completed" {
            later["payload"]["startedAt"] = Value::Null;
        }
        apply_values(&db, vec![later]).await;
        assert_eq!(
            store
                .limit_recovery_candidates(now.clone(), true, false)
                .await
                .unwrap()[0]
                .latest_run_id,
            RunId("run:test".into())
        );
        assert_eq!(
            serde_json::to_value(store.shell(thread.clone()).await.unwrap().unwrap()).unwrap()["latestRunId"],
            "run:test"
        );
        assert_eq!(
            t3_orchestration::projections::read::shell_from_projection(
                &store
                    .snapshot(thread.clone(), None)
                    .await
                    .unwrap()
                    .projection
            )
            .unwrap(),
            store.shell(thread.clone()).await.unwrap().unwrap()
        );
    }
    for (field, value) in [
        ("archivedAt", serde_json::json!(NOW)),
        ("settledOverride", serde_json::json!("settled")),
    ] {
        db.stores.transaction(move|tx|{tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json=json_set(payload_json,?1,json(?2)) WHERE thread_id='thread:test'",[format!("$.{field}"),value.to_string()])?;Ok(())}).await.unwrap();
        assert!(
            store
                .limit_recovery_candidates(now.clone(), true, false)
                .await
                .unwrap()
                .is_empty()
        );
        db.stores.transaction(move|tx|{tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json=json_set(payload_json,?1,NULL) WHERE thread_id='thread:test'",[format!("$.{field}")])?;Ok(())}).await.unwrap();
    }
    let recovery = serde_json::json!({"runId":"run:test","resetAt":reset,"autoResume":true,"requestId":"recovery:choice"});
    let recovery_json = recovery.to_string();
    db.stores.transaction(move|tx|{tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json=json_set(payload_json,'$.limitRecovery',json(?1)) WHERE thread_id='thread:test'",[recovery_json])?;Ok(())}).await.unwrap();
    assert!(
        store
            .limit_recovery_candidates(now.clone(), true, false)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.limit_recovery_wake_at(now.clone()).await.unwrap(),
        Some(4070908800000)
    );
    assert!(
        store
            .limit_recovery_wake_at(due.clone())
            .await
            .unwrap()
            .is_none()
    );
    let candidate = store
        .limit_recovery_candidates(due.clone(), false, false)
        .await
        .unwrap();
    assert_eq!(candidate.len(), 1);
    assert_eq!(
        t3_contracts::stored::to_value(&candidate[0].limit_recovery).unwrap(),
        recovery
    );
    db.stores.transaction(|tx|{tx.execute("INSERT INTO orchestration_v2_projection_runtime_requests(runtime_request_id,thread_id,node_id,kind,status,created_at,payload_json) VALUES('limit:pending','thread:test','node:test','command','pending',?1,'{}')",[NOW])?;Ok(())}).await.unwrap();
    assert!(
        store
            .limit_recovery_candidates(due.clone(), false, false)
            .await
            .unwrap()
            .is_empty()
    );
    db.stores.transaction(|tx|{tx.execute("DELETE FROM orchestration_v2_projection_runtime_requests WHERE runtime_request_id='limit:pending'",[])?;tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json=json_set(payload_json,'$.snoozedUntil','2099-01-01T00:01:00.000Z')",[])?;Ok(())}).await.unwrap();
    assert!(
        store
            .limit_recovery_candidates(due.clone(), false, false)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.limit_recovery_wake_at(due.clone()).await.unwrap(),
        Some(4070908860000)
    );
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json=json_set(payload_json,'$.snoozedUntil',NULL,'$.limitRecovery.autoResume',json('false'))",[])?;Ok(())}).await.unwrap();
    assert!(
        store
            .limit_recovery_wake_at(now.clone())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .limit_recovery_candidates(due, true, false)
            .await
            .unwrap()
            .is_empty()
    );
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json=json_remove(payload_json,'$.limitRecovery')",[])?;Ok(())}).await.unwrap();
    let mut session = fixture("provider-session.attached", "session:test");
    session["payload"]["providerInstanceId"] = serde_json::to_value(
        db.stores
            .read(|db| {
                Ok(t3_orchestration::projections::read::require_thread(
                    db,
                    &ThreadId("thread:test".into()),
                )?
                .provider_instance_id)
            })
            .await
            .unwrap(),
    )
    .unwrap();
    session["payload"]["lastError"] = "Provider process exited.".into();
    apply_values(&db, vec![session.clone()]).await;
    assert!(
        store
            .limit_recovery_candidates(now.clone(), true, false)
            .await
            .unwrap()
            .is_empty()
    );
    session["payload"]["lastError"] = Value::Null;
    apply_values(&db, vec![session]).await;
    assert_eq!(
        store
            .limit_recovery_candidates(now.clone(), true, false)
            .await
            .unwrap()
            .len(),
        1
    );
    let mut child = error.clone();
    child["payload"]["id"] = "item:child:error".into();
    child["payload"]["nodeId"] = "node:child".into();
    child["payload"]["ordinal"] = 3.into();
    child["payload"]["failure"]["class"] = "provider_error".into();
    child["payload"]["failure"]["message"] = "Child failed.".into();
    apply_values(&db, vec![child]).await;
    assert_eq!(
        store
            .limit_recovery_candidates(now.clone(), true, false)
            .await
            .unwrap()
            .len(),
        1
    );
    let mut replacement = error.clone();
    replacement["payload"]["id"] = "item:replacement".into();
    replacement["payload"]["ordinal"] = 4.into();
    replacement["payload"]["failure"]["class"] = "provider_error".into();
    apply_values(&db, vec![replacement]).await;
    assert!(
        store
            .limit_recovery_candidates(now.clone(), true, false)
            .await
            .unwrap()
            .is_empty()
    );
    apply_values(&db, vec![error]).await;
    db.stores.transaction(|tx|{tx.execute("DELETE FROM orchestration_v2_projection_turn_items WHERE turn_item_id='item:replacement'",[])?;tx.execute("DELETE FROM orchestration_v2_projection_runs WHERE run_id<>'run:test'",[])?;Ok(())}).await.unwrap();
    for status in [
        "running",
        "completed",
        "interrupted",
        "cancelled",
        "rolled_back",
    ] {
        let mut updated = run.clone();
        updated["payload"]["status"] = status.into();
        apply_values(&db, vec![updated]).await;
        assert!(
            store
                .limit_recovery_candidates(now.clone(), true, false)
                .await
                .unwrap()
                .is_empty()
        );
    }
    run["payload"]["rootNodeId"] = "node:other".into();
    apply_values(&db, vec![run]).await;
    assert!(
        store
            .limit_recovery_candidates(now, true, false)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn older_settled_queries_continue_timestamp_ties_and_count_scoped_projects() {
    use t3_orchestration::{
        api::*,
        projections::{read, store::ProjectionStore},
    };
    let db = TestDatabase::memory().unwrap();
    let mut events = Vec::new();
    for (name, day, project) in [
        ("a", Some(5), "project:a"),
        ("b", Some(4), "project:a"),
        ("c", Some(4), "project:a"),
        ("d", Some(3), "project:b"),
        ("snoozed", Some(2), "project:a"),
        ("recent", Some(20), "project:a"),
        ("unsettled", None, "project:a"),
        ("child", Some(1), "project:a"),
        ("archived", Some(1), "project:a"),
        ("side", Some(1), "project:a"),
    ] {
        let id = format!("thread:older:{name}");
        let mut event = fixture("thread.created", &id);
        event["threadId"] = id.clone().into();
        event["payload"]["projectId"] = project.into();
        if let Some(day) = day {
            event["payload"]["settledAt"] = format!("2026-10-{day:02}T00:00:00.000Z").into();
            event["payload"]["settledOverride"] = "settled".into();
        }
        if name == "child" {
            event["payload"]["lineage"] = serde_json::json!({"rootThreadId":"thread:older:b","parentThreadId":"thread:older:b","relationshipToParent":"subagent"});
        }
        if name == "archived" {
            event["payload"]["archivedAt"] = NOW.into();
        }
        if name == "side" {
            event["payload"]["lineage"]["sideChat"] = true.into();
        }
        if name == "snoozed" {
            event["payload"]["snoozedUntil"] = "2026-10-30T00:00:00.000Z".into();
        }
        events.push(event);
    }
    apply_values(&db, events).await;
    let input = OrchestrationV2GetOlderSettledThreadsInput {
        before: OrchestrationV2OlderSettledCursor {
            settled_at: t3_contracts::wire::WireDateTime("2026-10-10T00:00:00.000Z".into()),
            thread_id: None,
        },
        project_ids: None,
        limit: 2,
    };
    let concrete = ProjectionStore::new(db.stores.clone());
    let store: &dyn ProjectionReader = &concrete;
    let first = store.older_settled_threads(input.clone()).await.unwrap();
    assert_eq!(
        first
            .threads
            .iter()
            .map(|t| serde_json::to_value(&t.id).unwrap())
            .collect::<Vec<_>>(),
        vec![
            serde_json::json!("thread:older:a"),
            serde_json::json!("thread:older:b")
        ]
    );
    assert!(first.counts.is_empty());
    let counts = store
        .older_settled_threads(OrchestrationV2GetOlderSettledThreadsInput {
            limit: 0,
            ..input.clone()
        })
        .await
        .unwrap();
    assert!(counts.threads.is_empty());
    assert_eq!(
        counts
            .counts
            .iter()
            .map(|c| (c.project_id.0.as_str(), c.count))
            .collect::<Vec<_>>(),
        vec![("project:a", 4), ("project:b", 1)]
    );
    let before = OrchestrationV2OlderSettledCursor {
        settled_at: t3_contracts::wire::WireDateTime("2026-10-04T00:00:00.000Z".into()),
        thread_id: None,
    };
    let strict = store
        .older_settled_threads(OrchestrationV2GetOlderSettledThreadsInput {
            before: before.clone(),
            limit: 0,
            ..input.clone()
        })
        .await
        .unwrap();
    assert_eq!(
        strict
            .counts
            .iter()
            .map(|c| (c.project_id.0.as_str(), c.count))
            .collect::<Vec<_>>(),
        vec![("project:a", 1), ("project:b", 1)]
    );
    let next = store
        .older_settled_threads(OrchestrationV2GetOlderSettledThreadsInput {
            before: OrchestrationV2OlderSettledCursor {
                thread_id: Some(ThreadId("thread:older:b".into())),
                ..before
            },
            limit: 10,
            ..input.clone()
        })
        .await
        .unwrap();
    assert_eq!(
        next.threads
            .iter()
            .map(|t| serde_json::to_value(&t.id).unwrap())
            .collect::<Vec<_>>(),
        vec![
            serde_json::json!("thread:older:c"),
            serde_json::json!("thread:older:d"),
            serde_json::json!("thread:older:snoozed")
        ]
    );
    let empty = store
        .older_settled_threads(OrchestrationV2GetOlderSettledThreadsInput {
            project_ids: Some(vec![]),
            limit: 25,
            ..input.clone()
        })
        .await
        .unwrap();
    assert!(empty.threads.is_empty());
    assert!(empty.counts.is_empty());
    let scoped = store
        .older_settled_threads(OrchestrationV2GetOlderSettledThreadsInput {
            project_ids: Some(vec![ProjectId("project:b".into())]),
            limit: 0,
            ..input.clone()
        })
        .await
        .unwrap();
    assert_eq!(scoped.counts.len(), 1);
    assert_eq!(scoped.counts[0].count, 1);
    db.stores
        .read(move |db| {
            let rows = read::list_older_settled_threads(db, &input)?;
            assert_eq!(
                rows.thread_ids,
                vec![
                    ThreadId("thread:older:a".into()),
                    ThreadId("thread:older:b".into())
                ]
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn limit_recovery_respects_parent_put_away_state_but_honors_an_existing_choice() {
    use t3_orchestration::projections::store::ProjectionStore;
    let db = TestDatabase::memory().unwrap();
    let reset = "2099-01-01T00:00:00.000Z";
    let mut events = Vec::new();
    for state in ["settled", "archived", "deleted", "active"] {
        let parent = format!("parent:{state}");
        let child = format!("child:{state}");
        let run_id = format!("run:{state}");
        let mut p = fixture("thread.created", &parent);
        p["threadId"] = parent.clone().into();
        match state {
            "settled" => {
                p["payload"]["settledOverride"] = "settled".into();
                p["payload"]["settledAt"] = NOW.into();
            }
            "archived" => p["payload"]["archivedAt"] = NOW.into(),
            "deleted" => p["payload"]["deletedAt"] = NOW.into(),
            _ => {}
        }
        let mut c = fixture("thread.created", &child);
        c["threadId"] = child.clone().into();
        c["payload"]["lineage"] = serde_json::json!({"parentThreadId":parent,"relationshipToParent":"subagent","rootThreadId":parent});
        let mut run = fixture("run.created", &run_id);
        run["threadId"] = child.clone().into();
        run["payload"]["threadId"] = child.clone().into();
        run["payload"]["ordinal"] = 1.into();
        run["payload"]["status"] = "failed".into();
        run["payload"]["completedAt"] = NOW.into();
        let mut error = conversation_item(&format!("error:{state}"), Some(&run_id), 1);
        error["threadId"] = child.clone().into();
        error["payload"]["threadId"] = child.into();
        error["payload"]["nodeId"] = "node:test".into();
        error["payload"]["type"] = "error".into();
        error["payload"]["status"] = "failed".into();
        error["payload"]["failure"] = serde_json::json!({"class":"usage_limit","message":"Plan limit reached.","resetAt":reset,"code":"usageLimitExceeded","retryable":null});
        events.extend([p, c, run, error]);
    }
    apply_values(&db, events).await;
    let store = ProjectionStore::new(db.stores.clone());
    for (auto_resume, snooze) in [(true, false), (false, true)] {
        let candidates = store
            .limit_recovery_candidates(
                t3_contracts::wire::WireDateTime(NOW.into()),
                auto_resume,
                snooze,
            )
            .await
            .unwrap();
        assert_eq!(
            candidates.into_iter().map(|c| c.id.0).collect::<Vec<_>>(),
            ["child:active"]
        );
    }
    let armed =
        serde_json::json!({"runId":"run:settled","resetAt":reset,"autoResume":true}).to_string();
    db.stores.transaction(move|tx|{tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json=json_set(payload_json,'$.limitRecovery',json(?1)) WHERE thread_id='child:settled'",[armed])?;Ok(())}).await.unwrap();
    assert!(
        store
            .limit_recovery_candidates(
                t3_contracts::wire::WireDateTime("2099-01-01T02:00:00.000Z".into()),
                true,
                false
            )
            .await
            .unwrap()
            .iter()
            .any(|c| c.id.0 == "child:settled")
    );
}

#[tokio::test]
async fn metadata_reads_keep_archive_delete_guest_and_minimal_pull_request_scopes() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    let mut events = Vec::new();
    for (name, archived, deleted, subagent) in [
        ("open", false, false, false),
        ("archived", true, false, true),
        ("deleted", false, true, true),
        ("child", false, false, true),
    ] {
        let id = format!("thread:{name}");
        let mut event = fixture("thread.created", &id);
        event["threadId"] = id.clone().into();
        event["payload"]["worktreePath"] = format!("/work/{name}").into();
        if archived {
            event["payload"]["archivedAt"] = NOW.into();
        }
        if deleted {
            event["payload"]["deletedAt"] = NOW.into();
        }
        if subagent {
            event["payload"]["lineage"] = serde_json::json!({"parentThreadId":"thread:open","relationshipToParent":"subagent","rootThreadId":"thread:open"});
        }
        events.push(event);
    }
    apply_values(&db, events).await;
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json=json_set(payload_json,'$.title',37,'$.guestShare',json('{\"id\":\"guest:deleted\"}')) WHERE thread_id='thread:deleted'",[])?;Ok(())}).await.unwrap();
    let store = ProjectionStore::new(db.stores.clone());
    assert_eq!(store.count_threads().await.unwrap(), 3);
    assert_eq!(
        store.active_subagent_thread_ids().await.unwrap(),
        [ThreadId("thread:child".into())]
    );
    assert_eq!(
        store
            .thread_worktree_paths(false)
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.worktree_path)
            .collect::<std::collections::BTreeSet<_>>(),
        ["/work/open".to_owned(), "/work/child".to_owned()]
            .into_iter()
            .collect()
    );
    assert_eq!(store.thread_worktree_paths(true).await.unwrap().len(), 3);
    assert_eq!(
        store
            .find_guest_share_thread_id(GuestShareId("guest:deleted".into()))
            .await
            .unwrap(),
        Some(ThreadId("thread:deleted".into()))
    );
    assert_eq!(
        store
            .find_guest_share_thread_id(GuestShareId("absent".into()))
            .await
            .unwrap(),
        None
    );
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json=json_set(payload_json,'$.title',37) WHERE thread_id='thread:open'",[])?;Ok(())}).await.unwrap();
    assert!(store.thread(ThreadId("thread:open".into())).await.is_err());
    assert_eq!(
        store
            .pull_request_snapshot(None, false)
            .await
            .unwrap()
            .threads
            .len(),
        2
    );
    assert_eq!(
        store
            .pull_request_snapshot(Some(ThreadId("thread:archived".into())), false)
            .await
            .unwrap()
            .threads
            .len(),
        1
    );
    assert!(
        store
            .pull_request_snapshot(None, false)
            .await
            .unwrap()
            .threads
            .iter()
            .all(|thread| thread.updated_at.0 == NOW)
    );
}

#[tokio::test]
async fn memory_and_sql_shells_present_activity_cards_provider_history_and_background_work() {
    use t3_orchestration::{
        api::*,
        projections::{read, store::ProjectionStore},
    };
    let db = TestDatabase::memory().unwrap();
    let mut thread = fixture("thread.created", "thread:test");
    thread["payload"]["activeProviderThreadId"] = "provider:A".into();
    let mut run = fixture("run.created", "run:test");
    run["payload"]["ordinal"] = 1.into();
    run["payload"]["status"] = "completed".into();
    run["payload"]["workStartedAt"] = "2026-11-13T09:59:00.000Z".into();
    let mut providers = Vec::new();
    for (id, instance, owner) in [
        ("provider:child", "hidden", Some("node:child")),
        ("provider:a", "codex", None),
        ("provider:A", "claude", None),
    ] {
        let mut p = fixture("provider-thread.updated", id);
        p["payload"]["appThreadId"] = "thread:test".into();
        p["payload"]["providerInstanceId"] = instance.into();
        p["payload"]["ownerNodeId"] = serde_json::json!(owner);
        p["payload"]["createdAt"] = NOW.into();
        if id == "provider:A" {
            p["payload"]["pendingBackgroundTasks"] = serde_json::json!([{"taskId":"bg:1","kind":"command","description":"  still running  "}]);
            p["payload"]["goal"] =
                serde_json::json!({"objective":"Ship it","status":"active","tokensUsed":10});
        }
        providers.push(p);
    }
    let mut user = fixture("message.updated", "message:human");
    user["payload"]["createdBy"] = "user".into();
    user["payload"]["role"] = "user".into();
    let mut events = vec![thread, run.clone(), user];
    events.extend(providers);
    apply_values(&db, events).await;
    let store = ProjectionStore::new(db.stores.clone());
    let thread_id = ThreadId("thread:test".into());
    for status in [
        "completed",
        "queued",
        "preparing",
        "starting",
        "running",
        "waiting",
        "interrupted",
        "failed",
        "cancelled",
        "rolled_back",
    ] {
        run["payload"]["status"] = status.into();
        apply_values(&db, vec![run.clone()]).await;
        let sql = store.shell(thread_id.clone()).await.unwrap().unwrap();
        let memory = read::shell_from_projection(
            &store
                .snapshot(thread_id.clone(), None)
                .await
                .unwrap()
                .projection,
        )
        .unwrap();
        assert_eq!(memory, sql, "status {status}");
        let raw = serde_json::to_value(sql).unwrap();
        assert_eq!(
            raw["providerInstanceHistory"],
            serde_json::json!(["codex", "claude"])
        );
        assert_eq!(
            raw["goal"],
            serde_json::json!({"objective":"Ship it","status":"active","tokensUsed":10})
        );
        assert_eq!(
            !raw["activeRunId"].is_null(),
            matches!(status, "preparing" | "starting" | "running")
        );
        assert_eq!(
            !raw["activityRunStatus"].is_null(),
            matches!(status, "preparing" | "starting" | "running" | "waiting")
        );
        if matches!(
            status,
            "completed" | "interrupted" | "failed" | "cancelled" | "waiting"
        ) {
            assert_eq!(
                raw["pendingBackgroundTasks"][0]["description"],
                "still running"
            );
        } else {
            assert_eq!(raw["pendingBackgroundTasks"], serde_json::json!([]));
        }
    }
    run["payload"]["status"] = "running".into();
    let mut secret = conversation_item("card:secret", Some("run:test"), 1);
    secret["payload"]["type"] = "secret_request".into();
    secret["payload"]["nodeId"] = "node:test".into();
    secret["payload"]["status"] = "waiting".into();
    secret["payload"]["label"] = "TOKEN".into();
    secret["payload"]["secretStatus"] = "pending".into();
    secret["payload"]["reason"] = "Needed".into();
    let mut question = fixture("runtime-request.updated", "request:question");
    question["payload"]["kind"] = "user_input".into();
    question["payload"]["status"] = "pending".into();
    question["payload"]["createdAt"] = "2026-11-13T09:00:00.000Z".into();
    apply_values(&db, vec![run.clone(), secret.clone(), question]).await;
    for status in ["running", "completed"] {
        run["payload"]["status"] = status.into();
        apply_values(&db, vec![run.clone()]).await;
        let sql = store.shell(thread_id.clone()).await.unwrap().unwrap();
        assert_eq!(
            read::shell_from_projection(
                &store
                    .snapshot(thread_id.clone(), None)
                    .await
                    .unwrap()
                    .projection
            )
            .unwrap(),
            sql
        );
        assert_eq!(
            serde_json::to_value(sql).unwrap()["pendingRuntimeRequest"]["id"],
            if status == "running" {
                "card:secret"
            } else {
                "request:question"
            }
        );
    }
}

#[tokio::test]
async fn shell_subagent_tasks_use_the_latest_global_provider_owner_and_ignore_rolled_back_work() {
    use t3_orchestration::{
        api::*,
        projections::{read, store::ProjectionStore},
    };
    let db = TestDatabase::memory().unwrap();
    let mut events = Vec::new();
    for name in ["test", "other"] {
        let id = format!("thread:{name}");
        let mut thread = fixture("thread.created", &id);
        thread["threadId"] = id.clone().into();
        let mut run = fixture("run.created", &format!("run:{name}"));
        run["threadId"] = id.clone().into();
        run["payload"]["threadId"] = id.clone().into();
        run["payload"]["status"] = "completed".into();
        run["payload"]["completedAt"] = NOW.into();
        let mut item = conversation_item(&format!("item:{name}"), Some(&format!("run:{name}")), 1);
        item["threadId"] = id.clone().into();
        item["payload"]["threadId"] = id.into();
        item["payload"]["providerThreadId"] = "provider:shared".into();
        item["payload"]["type"] = "subagent".into();
        item["payload"]["status"] = "running".into();
        item["payload"]["subagentId"] = format!("node:{name}").into();
        item["payload"]["origin"] = "provider_native".into();
        item["payload"]["driver"] = "codex".into();
        item["payload"]["providerInstanceId"] = "codex".into();
        item["payload"]["childThreadId"] = Value::Null;
        item["payload"]["nativeItemRef"] =
            serde_json::json!({"driver":"codex","nativeId":"task:shared","strength":"strong"});
        item["payload"]["title"] = "Review".into();
        item["payload"]["prompt"] = "Review".into();
        item["payload"]["result"] = Value::Null;
        events.extend([thread, run, item]);
    }
    for (id, instance, pending) in [
        ("thread:other", "claude", true),
        ("thread:test", "codex", false),
    ] {
        let mut provider = fixture("provider-thread.updated", "provider:shared");
        provider["threadId"] = id.into();
        provider["payload"]["appThreadId"] = id.into();
        provider["payload"]["providerInstanceId"] = instance.into();
        provider["payload"]["pendingBackgroundTasks"] = if pending {
            serde_json::json!([{"taskId":"obsolete:owner","kind":"subagent"}])
        } else {
            serde_json::json!([])
        };
        events.push(provider);
    }
    apply_values(&db, events).await;
    let store = ProjectionStore::new(db.stores.clone());
    assert!(
        store
            .snapshot(ThreadId("thread:other".into()), None)
            .await
            .unwrap()
            .projection
            .provider_threads
            .is_empty()
    );
    for id in ["thread:test", "thread:other"] {
        let shell = store.shell(ThreadId(id.into())).await.unwrap().unwrap();
        let raw = t3_contracts::stored::to_value(&shell).unwrap();
        assert_eq!(
            raw["pendingBackgroundTasks"],
            serde_json::json!([{"taskId":"task:shared","description":"Review","kind":"subagent","providerInstanceId":"codex"}])
        );
        assert!(raw["activeRunId"].is_null());
    }
    db.stores.transaction(|tx|{
        tx.execute("UPDATE orchestration_v2_projection_turn_items SET status='cancelled',payload_json=json_set(payload_json,'$.status','cancelled') WHERE turn_item_id='item:test'",[])?;
        tx.execute("UPDATE orchestration_v2_projection_runs SET status='rolled_back',payload_json=json_set(payload_json,'$.status','rolled_back') WHERE run_id='run:other'",[])?;
        Ok(())
    }).await.unwrap();
    for id in ["thread:test", "thread:other"] {
        let shell = store.shell(ThreadId(id.into())).await.unwrap().unwrap();
        assert!(
            shell
                .pending_background_tasks
                .as_ref()
                .and_then(Option::as_ref)
                .is_none_or(Vec::is_empty)
        );
        let projection = store
            .snapshot(ThreadId(id.into()), None)
            .await
            .unwrap()
            .projection;
        assert!(
            read::shell_from_projection(&projection)
                .unwrap()
                .pending_background_tasks
                .as_ref()
                .and_then(Option::as_ref)
                .is_none_or(Vec::is_empty)
        );
    }
}

#[tokio::test]
async fn awareness_reads_listed_or_targeted_threads_without_counts_or_inherited_history() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    let mut events = Vec::new();
    for name in ["root", "fork", "side", "child", "archived"] {
        let id = format!("thread:{name}");
        let mut event = fixture("thread.created", &id);
        event["threadId"] = id.clone().into();
        if matches!(name, "fork" | "side") {
            event["payload"]["forkedFrom"] =
                serde_json::json!({"type":"run","threadId":"thread:missing","runId":"run:missing"});
        }
        if name == "side" {
            event["payload"]["lineage"]["sideChat"] = true.into();
        }
        if name == "child" {
            event["payload"]["lineage"] = serde_json::json!({"parentThreadId":"thread:root","relationshipToParent":"subagent","rootThreadId":"thread:root"});
        }
        if name == "archived" {
            event["payload"]["archivedAt"] = NOW.into();
        }
        events.push(event);
    }
    apply_values(&db, events).await;
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
    let store = ProjectionStore::new(db.stores.clone());
    let all = store.agent_awareness_snapshot(None).await.unwrap();
    assert_eq!(
        all.threads
            .iter()
            .map(|t| t.id.0.as_str())
            .collect::<Vec<_>>(),
        ["thread:fork", "thread:root"]
    );
    for name in ["side", "child", "archived"] {
        let id = ThreadId(format!("thread:{name}"));
        let scoped = store
            .agent_awareness_snapshot(Some(id.clone()))
            .await
            .unwrap();
        assert_eq!(scoped.threads.len(), 1);
        assert_eq!(scoped.threads[0].id, id);
    }
    assert!(
        store
            .agent_awareness_snapshot(Some(ThreadId("absent".into())))
            .await
            .unwrap()
            .threads
            .is_empty()
    );
}

#[tokio::test]
async fn local_history_pages_match_the_full_index_and_do_not_parse_older_payloads() {
    use t3_orchestration::{
        api::*,
        ports::transport::thread_history_paging::{
            ThreadHistoryCursorPayload, encode_thread_history_cursor,
        },
        projections::store::ProjectionStore,
    };
    let db = TestDatabase::memory().unwrap();
    let mut run = fixture("run.created", "run:test");
    run["payload"]["ordinal"] = 1.into();
    run["payload"]["status"] = "completed".into();
    let mut events = vec![fixture("thread.created", "thread:test"), run];
    for n in 0..1600 {
        events.push(conversation_item(
            &format!("page:{n:04}"),
            Some("run:test"),
            n,
        ));
    }
    apply_values(&db, events).await;
    let store = ProjectionStore::new(db.stores.clone());
    let cursor = encode_thread_history_cursor(&ThreadHistoryCursorPayload {
        v: 1,
        seq: 0,
        st: "thread:test".into(),
        si: "page:1599".into(),
        p: 1599,
        inclusive: None,
    })
    .unwrap();
    let query = HistoryQuery {
        thread_id: ThreadId("thread:test".into()),
        cursor,
        through_entry_id: None,
        conversation_only: false,
        work_mode: None,
    };
    let expected = store.history(query.clone()).await.unwrap();
    let full = store
        .history(HistoryQuery {
            through_entry_id: Some("missing-entry".into()),
            ..query.clone()
        })
        .await
        .unwrap();
    assert_eq!(expected, full);
    assert_eq!(expected.items.len(), 20);
    db.stores.transaction(|tx|{
        // This expression index rejects malformed JSON before a reader can encounter it.
        tx.execute("DROP INDEX orchestration_v2_projection_turn_items_queued_input_idx",[])?;
        tx.execute("UPDATE orchestration_v2_projection_turn_items SET payload_json='not json' WHERE turn_item_id='page:0000'",[])?;
        Ok(())
    }).await.unwrap();
    let actual = store.history(query).await.unwrap();
    assert_eq!(actual, expected);
    assert_eq!(
        serde_json::to_value(&actual.items[0].source_item_id).unwrap(),
        "page:1579"
    );
    let next = store
        .history(HistoryQuery {
            thread_id: ThreadId("thread:test".into()),
            cursor: actual.next_cursor.unwrap().0,
            through_entry_id: None,
            conversation_only: false,
            work_mode: None,
        })
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&next.items[0].source_item_id).unwrap(),
        "page:1559"
    );
}

#[tokio::test]
async fn windowed_history_and_timeline_match_full_indexes_across_hidden_tied_and_folded_rows() {
    use t3_orchestration::{
        api::*,
        ports::transport::thread_history_paging::{
            ThreadHistoryCursorPayload, encode_thread_history_cursor,
        },
        projections::{read, store::ProjectionStore},
    };
    let mut seed = 0x5eedu32;
    let mut random = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(seed) / 4_294_967_296.0
    };
    let mut groups_read = 0;
    for round in 0..5 {
        let db = TestDatabase::memory().unwrap();
        let mut events = vec![fixture("thread.created", "thread:test")];
        let kinds = if round == 3 {
            let mut kinds = vec!["rolled", "late"];
            kinds.extend(std::iter::repeat_n("one", 149));
            kinds
        } else if round == 4 {
            vec!["short", "cancelled", "fold", "long", "stopped"]
        } else {
            vec![
                "rolled",
                "cancelled",
                "held",
                "replaced",
                "stopped",
                "fold",
                "fold",
                "short",
                "short",
            ]
        };
        let mut ordinal = 0;
        let mut item_count = 0;
        let mut page_ends = Vec::new();
        for (index, kind) in kinds.iter().chain(std::iter::once(&"active")).enumerate() {
            let run_id = format!("run:matrix:{index}");
            let node_id = format!("node:matrix:{index}");
            let status = match *kind {
                "active" => "running",
                "rolled" => "rolled_back",
                "cancelled" => "cancelled",
                "held" => "queued",
                _ => "completed",
            };
            let mut run = fixture("run.created", &run_id);
            run["payload"]["ordinal"] = (index + 1).into();
            run["payload"]["status"] = status.into();
            run["payload"]["rootNodeId"] = node_id.clone().into();
            let mut attempt = fixture("run-attempt.updated", &format!("attempt:matrix:{index}"));
            attempt["payload"]["runId"] = run_id.clone().into();
            attempt["payload"]["rootNodeId"] = node_id.clone().into();
            attempt["payload"]["status"] = if *kind == "replaced" || random() < 0.2 {
                "superseded"
            } else {
                "completed"
            }
            .into();
            events.extend([run, attempt]);
            let agent_driven = round == 1 && index > 0;
            let mut add = |fields: Value, owner: Option<usize>, tie: bool| {
                if !tie {
                    ordinal += 1;
                }
                let id = format!("matrix:{item_count:05}");
                item_count += 1;
                let mut item = conversation_item(
                    &id,
                    Some(
                        &owner
                            .map(|i| format!("run:matrix:{i}"))
                            .unwrap_or_else(|| run_id.clone()),
                    ),
                    ordinal,
                );
                item["payload"]["nodeId"] = owner
                    .map(|i| format!("node:matrix:{i}"))
                    .unwrap_or_else(|| node_id.clone())
                    .into();
                item["payload"]
                    .as_object_mut()
                    .unwrap()
                    .extend(fields.as_object().unwrap().clone());
                events.push(item);
                id
            };
            let notification = serde_json::json!({"type":"notification","source":{"kind":"system"},"outcome":"completed","summary":"Wake"});
            let first = if agent_driven
                || (*kind == "one" && index % 10 != 0)
                || (round == 4 && matches!(*kind, "fold" | "long" | "stopped"))
            {
                notification.clone()
            } else {
                serde_json::json!({"inputIntent":if matches!(*kind,"cancelled"|"held"){"queued_turn"}else{"turn_start"}})
            };
            add(
                first,
                None,
                index > 0 && kinds.get(index - 1) == Some(&"rolled"),
            );
            let work = match *kind {
                "fold" => 180,
                "long" => 300,
                "stopped" => 350,
                "late" => 440,
                "rolled" => 550,
                "one" => 0,
                _ => 15,
            };
            for n in 0..work {
                let owner = if (*kind == "long" && n < 140) || *kind == "late" {
                    Some(index - 1)
                } else {
                    None
                };
                let fields = if random() < 0.5 {
                    serde_json::json!({"type":"command_execution","input":"echo","output":"ok","exitCode":0})
                } else {
                    serde_json::json!({"type":"reasoning","text":"","streaming":false})
                };
                add(fields, owner, random() < 0.04);
                if (*kind == "stopped" && n == 200)
                    || (matches!(*kind, "long" | "late") && n % 5 == 0)
                    || random() < 0.02
                {
                    let id = add(
                        serde_json::json!({"type":"assistant_message","text":"Reply","streaming":false}),
                        None,
                        false,
                    );
                    if *kind == "stopped" && n == 200 {
                        page_ends.push(id);
                    }
                }
                if *kind != "late" && random() < 0.01 {
                    add(notification.clone(), None, false);
                }
            }
            if *kind == "late" {
                add(notification.clone(), None, false);
            }
            if *kind == "stopped" {
                add(
                    serde_json::json!({"type":"run_interrupt_request","message":"Stop"}),
                    None,
                    false,
                );
            }
            if matches!(*kind, "stopped" | "replaced") {
                add(
                    serde_json::json!({"type":"run_interrupt_result","message":"Stopped"}),
                    None,
                    false,
                );
            }
        }
        apply_values(&db, events).await;
        let store = ProjectionStore::new(db.stores.clone());
        let thread_id = ThreadId("thread:test".into());
        let first_cursor = encode_thread_history_cursor(&ThreadHistoryCursorPayload {
            v: 1,
            seq: 0,
            st: thread_id.0.clone(),
            si: format!("matrix:{:05}", item_count - 1),
            p: 0,
            inclusive: Some(true),
        })
        .unwrap();
        let mut groups = std::collections::BTreeMap::new();
        for (conversation_only, work_mode) in [
            (false, Some(HistoryWorkMode::Deferred)),
            (false, None),
            (true, None),
        ] {
            let mut cursor = Some(first_cursor.clone());
            let mut seen = std::collections::BTreeSet::new();
            while let Some(current) = cursor.take() {
                assert!(seen.insert(current.clone()), "cursor cycle round {round}");
                let query = HistoryQuery {
                    thread_id: thread_id.clone(),
                    cursor: current,
                    through_entry_id: None,
                    conversation_only,
                    work_mode,
                };
                let actual = store.history(query.clone()).await.unwrap();
                let expected = store
                    .read_snapshot(move |c| read::history_from_full_index(c, &query))
                    .await
                    .unwrap();
                assert_eq!(
                    actual, expected,
                    "history round {round} mode {work_mode:?} conversation {conversation_only}"
                );
                for row in &actual.items {
                    let item = t3_contracts::stored::to_value(&row.item).unwrap();
                    if let Some(cursor) = item["deferredWork"]["cursor"].as_str() {
                        groups.insert(item["id"].as_str().unwrap().to_owned(), cursor.to_owned());
                    }
                }
                cursor = actual.next_cursor.map(|c| c.0);
            }
        }
        for id in page_ends {
            let cursor = encode_thread_history_cursor(&ThreadHistoryCursorPayload {
                v: 1,
                seq: 0,
                st: thread_id.0.clone(),
                si: id,
                p: 0,
                inclusive: None,
            })
            .unwrap();
            let q = HistoryQuery {
                thread_id: thread_id.clone(),
                cursor,
                through_entry_id: None,
                conversation_only: false,
                work_mode: Some(HistoryWorkMode::Deferred),
            };
            let actual = store.history(q.clone()).await.unwrap();
            assert_eq!(
                actual,
                store
                    .read_snapshot(move |c| read::history_from_full_index(c, &q))
                    .await
                    .unwrap(),
                "stopped round {round}"
            );
        }
        for (anchor, cursor) in groups {
            groups_read += 1;
            let mut cursor = Some(cursor);
            let mut seen = std::collections::BTreeSet::new();
            while let Some(current) = cursor.take() {
                assert!(seen.insert(current.clone()));
                let q = HistoryQuery {
                    thread_id: thread_id.clone(),
                    cursor: current,
                    through_entry_id: Some(anchor.clone()),
                    conversation_only: false,
                    work_mode: Some(HistoryWorkMode::Work),
                };
                let actual = store.history(q.clone()).await.unwrap();
                assert_eq!(
                    actual,
                    store
                        .read_snapshot(move |c| read::history_from_full_index(c, &q))
                        .await
                        .unwrap(),
                    "work round {round}"
                );
                cursor = actual.next_cursor.map(|c| c.0);
            }
        }
        let total = store
            .timeline(TimelineQuery {
                thread_id: thread_id.clone(),
                before_position: None,
                after_position: None,
                limit: 0,
                item_id: None,
                attachment_id: None,
                view: Some(TimelineView::Activity),
            })
            .await
            .unwrap()
            .total_items;
        for view in [None, Some(TimelineView::Activity)] {
            for after in [None, Some(3), Some(total / 3), Some(total - 5)] {
                for before in [None, Some(40), Some(total / 3), Some(u64::MAX)] {
                    if after.zip(before).is_some_and(|(a, b)| a >= b) {
                        continue;
                    }
                    for limit in [1, 20] {
                        let q = TimelineQuery {
                            thread_id: thread_id.clone(),
                            before_position: before,
                            after_position: after,
                            limit,
                            item_id: None,
                            attachment_id: None,
                            view,
                        };
                        let actual = store.timeline(q.clone()).await.unwrap();
                        let expected = store
                            .read_snapshot(move |c| read::timeline_from_full_index(c, &q))
                            .await
                            .unwrap();
                        assert_eq!(
                            (
                                actual.items,
                                actual.total_items,
                                actual.has_more,
                                actual.has_earlier
                            ),
                            (
                                expected.items,
                                expected.total_items,
                                expected.has_more,
                                expected.has_earlier
                            ),
                            "timeline round {round} view {view:?} after {after:?} before {before:?} limit {limit}"
                        );
                    }
                }
            }
        }
    }
    assert!(groups_read >= 4);
}

#[tokio::test]
async fn bounded_snapshots_and_history_keep_turn_boundaries_without_decoding_old_payloads() {
    use t3_orchestration::{
        api::*,
        ports::transport::thread_history_paging::{
            ThreadHistoryCursorPayload, encode_thread_history_cursor,
        },
        projections::store::ProjectionStore,
    };
    let db = TestDatabase::memory().unwrap();
    let mut events = vec![fixture("thread.created", "thread:test")];
    for n in 0..30 {
        let id = format!("run:{n:02}");
        let mut run = fixture("run.created", &id);
        run["payload"]["ordinal"] = (n + 1).into();
        run["payload"]["status"] = "completed".into();
        run["payload"]["completedAt"] = NOW.into();
        run["payload"]["rootNodeId"] = Value::Null;
        run["payload"]["activeAttemptId"] = Value::Null;
        events.push(run);
        events.push(conversation_item(
            &format!("item:{n:02}"),
            Some(&id),
            n * 1000000,
        ));
    }
    apply_values(&db, events).await;
    let store = ProjectionStore::new(db.stores.clone());
    let thread = ThreadId("thread:test".into());
    let full = store.snapshot(thread.clone(), None).await.unwrap();
    assert_eq!(full.projection.turn_items.len(), 30);
    let window = SnapshotWindow {
        row_limit: 100,
        user_turn_limit: Some(2),
        ..Default::default()
    };
    let snapshot = store
        .snapshot(thread.clone(), Some(window.clone()))
        .await
        .unwrap();
    let item_ids = |p: OrchestrationV2ThreadProjection| {
        p.turn_items
            .into_iter()
            .map(|i| {
                serde_json::to_value(i).unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        item_ids(snapshot.projection),
        vec!["item:26", "item:27", "item:28", "item:29"]
    );
    let anchored = store
        .snapshot(
            thread.clone(),
            Some(SnapshotWindow {
                anchor_item_id: Some(TurnItemId("item:04".into())),
                ..window
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        item_ids(anchored.projection),
        vec!["item:01", "item:02", "item:03", "item:04"]
    );
    let cursor = encode_thread_history_cursor(&ThreadHistoryCursorPayload {
        v: 1,
        seq: 0,
        st: thread.0.clone(),
        si: "item:29".into(),
        p: 29,
        inclusive: None,
    })
    .unwrap();
    let page = store
        .history(HistoryQuery {
            thread_id: thread.clone(),
            cursor,
            through_entry_id: None,
            conversation_only: false,
            work_mode: None,
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 20);
    assert!(page.has_more_history);
    assert_eq!(
        serde_json::to_value(&page.items[0].source_item_id).unwrap(),
        "item:09"
    );
    assert_eq!(
        serde_json::to_value(&page.items[19].source_item_id).unwrap(),
        "item:28"
    );
    let older = store
        .history(HistoryQuery {
            thread_id: thread.clone(),
            cursor: page.next_cursor.unwrap().0,
            through_entry_id: None,
            conversation_only: false,
            work_mode: None,
        })
        .await
        .unwrap();
    assert_eq!(older.items.len(), 9);
    assert!(!older.has_more_history);
    assert!(older.next_cursor.is_none());
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_turn_items SET payload_json='{}' WHERE turn_item_id='item:00'",[])?;Ok(())}).await.unwrap();
    assert!(store.snapshot(thread.clone(), None).await.is_err());
    assert_eq!(
        store
            .snapshot(
                thread,
                Some(SnapshotWindow {
                    row_limit: 2,
                    ..Default::default()
                })
            )
            .await
            .unwrap()
            .projection
            .turn_items
            .len(),
        2
    );
}

#[tokio::test]
async fn bounded_snapshots_keep_pending_request_cards_outside_the_selected_turns() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    let mut card = conversation_item("item:question", None, 1);
    let payload = &mut card["payload"];
    payload["type"] = "user_input_request".into();
    payload["nodeId"] = "node:question".into();
    payload["requestId"] = "request:question".into();
    payload["questions"] = serde_json::json!([]);
    payload["status"] = "waiting".into();
    let mut request = fixture("runtime-request.updated", "request:question");
    request["payload"]["nodeId"] = "node:question".into();
    request["payload"]["kind"] = "user_input".into();
    request["payload"]["status"] = "pending".into();
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            card,
            request,
            conversation_item("item:latest", None, 2000),
        ],
    )
    .await;
    let snapshot = ProjectionStore::new(db.stores)
        .snapshot(
            ThreadId("thread:test".into()),
            Some(SnapshotWindow {
                row_limit: 1,
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    let ids = snapshot
        .projection
        .turn_items
        .iter()
        .map(|i| {
            serde_json::to_value(i).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["item:question", "item:latest"]);
}

#[tokio::test]
async fn raw_shell_reads_release_the_reader_before_decode_and_apply_project_scopes_first() {
    use t3_orchestration::{
        api::*,
        projections::{read, store::ProjectionStore},
    };
    let db = TestDatabase::memory().unwrap();
    let mut thread = fixture("thread.created", "thread:test");
    thread["payload"]["projectId"] = "project:test".into();
    apply_values(&db, vec![thread, fixture("run.created", "run:test")]).await;
    let store = ProjectionStore::new(db.stores.clone());
    let raw = store
        .read_snapshot(|db| read::read_shell_rows(db, &ShellQuery::default(), true))
        .await
        .unwrap();
    let mut later = fixture("thread.created", "thread:later");
    later["threadId"] = "thread:later".into();
    later["payload"]["projectId"] = "project:other".into();
    apply_values(&db, vec![later]).await;
    let frozen = read::decode_shell_rows(raw).unwrap();
    assert_eq!(frozen.threads.len(), 1);
    assert_eq!(
        t3_contracts::stored::to_value(&frozen.threads[0]).unwrap()["id"],
        "thread:test"
    );
    assert_eq!(
        store
            .thread_shells(ShellQuery::default())
            .await
            .unwrap()
            .threads
            .len(),
        2
    );
    db.stores.transaction(|tx|{
        tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json='{}' WHERE thread_id='thread:later'",[])?;
        tx.execute("UPDATE orchestration_v2_projection_runs SET payload_json='{}' WHERE run_id='run:test'",[])?;
        Ok(())
    }).await.unwrap();
    let raw = store
        .read_snapshot(|db| read::read_shell_rows(db, &ShellQuery::default(), true))
        .await
        .unwrap();
    assert!(read::decode_shell_rows(raw).is_err());
    assert!(store.shell_snapshot(None, None).await.is_err());
    let scoped = store
        .shell_snapshot(Some(vec![ProjectId("project:test".into())]), None)
        .await
        .unwrap();
    assert_eq!(scoped.threads.len(), 1);
    let shell = t3_contracts::stored::to_value(&scoped.threads[0]).unwrap();
    assert_eq!(shell["latestRunId"], "run:test");
    assert_eq!(shell["status"], "running");
    assert!(
        store
            .snapshot(ThreadId("thread:test".into()), None)
            .await
            .is_err()
    );
    assert!(
        store
            .shell_snapshot(Some(vec![]), None)
            .await
            .unwrap()
            .threads
            .is_empty()
    );
    let mut bulk = Vec::new();
    for n in 0..1200 {
        let id = format!("thread:bulk:{n:04}");
        let mut event = fixture("thread.created", &id);
        event["threadId"] = id.into();
        event["payload"]["projectId"] = "project:bulk".into();
        bulk.push(event);
    }
    apply_values(&db, bulk).await;
    let shells = store
        .thread_shells(ShellQuery {
            project_id: Some(ProjectId("project:bulk".into())),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(shells.threads.len(), 1200);
    assert!(shells.archived_threads.is_empty());
}

#[tokio::test]
async fn subscriber_shell_selection_uses_the_active_index_before_decoding_transcripts() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    let mut other = fixture("thread.created", "thread:other");
    other["threadId"] = "thread:other".into();
    let mut archived = fixture("thread.created", "thread:archived");
    archived["threadId"] = "thread:archived".into();
    archived["payload"]["archivedAt"] = NOW.into();
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            fixture("run.created", "run:test"),
            other,
            archived,
        ],
    )
    .await;
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_runs SET payload_json='{}' WHERE run_id='run:test'",[])?;Ok(())}).await.unwrap();
    let store = ProjectionStore::new(db.stores);
    let result = store
        .subscriber_shell_snapshot(Some(Box::new(|index| {
            assert_eq!(index.len(), 2);
            assert_eq!(
                index
                    .iter()
                    .filter(|row| row.may_be_live)
                    .map(|row| row.id.0.as_str())
                    .collect::<Vec<_>>(),
                vec!["thread:test"]
            );
            vec![ThreadId("thread:other".into())]
        })))
        .await
        .unwrap();
    assert_eq!(result.index.unwrap().len(), 2);
    assert_eq!(result.snapshot.threads.len(), 1);
    assert_eq!(
        serde_json::to_value(&result.snapshot.threads[0].id).unwrap(),
        "thread:other"
    );
    assert!(result.snapshot.archived_threads.is_empty());
}

#[tokio::test]
async fn subscriber_shell_snapshot_uses_the_global_project_and_thread_high_water() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    apply_values(&db, vec![fixture("thread.created", "thread:test")]).await;
    db.stores.transaction(|tx|{tx.execute("INSERT INTO orchestration_events(sequence,event_id,aggregate_kind,stream_id,stream_version,event_type,occurred_at,actor_kind,payload_json,metadata_json,application_event_version) VALUES(41,'project:sequence','project','project:test',0,'project.meta-updated',?1,'system','{}','{}',2)",[NOW])?;Ok(())}).await.unwrap();
    let result = ProjectionStore::new(db.stores)
        .subscriber_shell_snapshot(None)
        .await
        .unwrap();
    assert_eq!(result.snapshot.snapshot_sequence, 41);
}

#[tokio::test]
async fn registry_pull_request_updates_resolve_url_authority_without_merging_unrelated_links() {
    use t3_contracts::thread_pull_request::ThreadPullRequestKey;
    use t3_orchestration::{
        api::*,
        projections::{
            read,
            write::{ParsedPullRequestKey, PullRequestIdentityRegistry, RegistryProjectionWriter},
        },
    };
    struct Registry;
    impl PullRequestIdentityRegistry for Registry {
        fn parse_change_request_url(&self, url: &str) -> Option<ParsedPullRequestKey> {
            (url == "https://dev.azure.com/org/project/_git/repo/pullrequest/42").then(|| {
                ParsedPullRequestKey {
                    key: ThreadPullRequestKey {
                        host: TrimmedNonEmptyString("azure-devops".into()),
                        repository: TrimmedNonEmptyString("org/project/repo".into()),
                        number: 42,
                    },
                    authority: Some("dev.azure.com".into()),
                }
            })
        }
        fn canonical_repository_key(&self, key: &str) -> String {
            key.replace("azure-alias/", "dev.azure.com/")
        }
        fn contains_repository(&self, key: &str) -> bool {
            key.starts_with("dev.azure.com/")
        }
    }
    let db = TestDatabase::memory().unwrap();
    let link = serde_json::json!({"host":"azure-devops","repository":"org/project/repo","number":42,"url":"https://dev.azure.com/org/project/_git/repo/pullrequest/42","source":"manual","linkedAt":NOW,"snapshot":null,"stack":null});
    let unrelated = serde_json::json!({"host":"other.example","repository":"org/project/repo","number":42,"url":"https://other.example/org/project/repo/pull/42","source":"agent","linkedAt":NOW,"snapshot":null,"stack":null});
    let mut seed = fixture("thread.created", "thread:test");
    seed["payload"]["pullRequests"] = serde_json::json!([link.clone(), unrelated.clone()]);
    apply_values(&db, vec![seed]).await;
    let mut update = fixture("thread.pull-request-link-updated", "thread:test");
    update["payload"] = link.clone();
    update["payload"]["host"] = "azure-alias".into();
    update["payload"]["source"] = "created".into();
    let event: OrchestrationV2DomainEvent = serde_json::from_value(update).unwrap();
    let before = db
        .stores
        .read(|db| read::canonical(db, &ThreadId("thread:test".into())))
        .await
        .unwrap();
    let expected_update = serde_json::to_value(&event).unwrap()["payload"].clone();
    let memory =
        write::apply_to_projection_with_registry(&before, &event, Some(&Registry)).unwrap();
    db.stores
        .transaction(move |tx| {
            RegistryProjectionWriter::new(std::sync::Arc::new(Registry)).apply(
                tx,
                &OrchestrationV2StoredEvent {
                    sequence: 100,
                    command_id: None,
                    event,
                },
            )
        })
        .await
        .unwrap();
    let sql = db
        .stores
        .read(|db| read::canonical(db, &ThreadId("thread:test".into())))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&sql.thread).unwrap()["pullRequests"],
        serde_json::json!([expected_update, unrelated])
    );
    assert_eq!(sql.thread, memory.thread);
    assert_eq!(sql.thread.updated_at, before.thread.updated_at);
}

#[tokio::test]
async fn attachment_collection_keeps_live_fork_files_and_collects_pending_guest_and_answer_files() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    let file = |id: &str| serde_json::json!({"type":"file","id":id,"name":"a.txt","mimeType":"text/plain","sizeBytes":4});
    let mut run = fixture("run.created", "run:test");
    run["payload"]["ordinal"] = 1.into();
    run["payload"]["status"] = "completed".into();
    let mut next = run.clone();
    next["payload"]["id"] = "run:next".into();
    next["payload"]["ordinal"] = 2.into();
    let mut message = fixture("message.updated", "message:source");
    message["payload"]["attachments"] = serde_json::json!([file("file:inherited")]);
    let mut latest = message.clone();
    latest["payload"]["id"] = "message:latest".into();
    latest["payload"]["runId"] = "run:next".into();
    latest["payload"]["attachments"] = serde_json::json!([file("file:latest")]);
    let mut fork = fixture("thread.created", "thread:fork");
    fork["threadId"] = "thread:fork".into();
    fork["payload"]["forkedFrom"] =
        serde_json::json!({"type":"run","threadId":"thread:test","runId":"run:test"});
    let mut answer = conversation_item("item:answer", Some("run:next"), 10);
    answer["payload"]["type"] = "user_input_request".into();
    answer["payload"]["requestId"] = "request:answer".into();
    answer["payload"]["questions"] = serde_json::json!([]);
    answer["payload"]["questionAnswer"] = serde_json::json!({"requestId":"request:answer","answers":{},"attachmentsByQuestionId":{"question":[file("file:answer"),file("file:latest")]}});
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            run,
            next,
            message,
            latest,
            fork,
            answer,
        ],
    )
    .await;
    db.stores.transaction(move|tx|{let guests=serde_json::json!({"messages":[{"status":"pending","attachments":[file("file:guest"),file("file:latest")]},{"status":"accepted","attachments":[file("file:accepted")]}]});tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json=json_set(payload_json,'$.guestShare',json(?1)) WHERE thread_id='thread:test'",[guests.to_string()])?;Ok(())}).await.unwrap();
    let store = ProjectionStore::new(db.stores.clone());
    let ids = store
        .attachment_ids(ThreadId("thread:test".into()))
        .await
        .unwrap();
    assert_eq!(
        ids.iter().map(String::as_str).collect::<BTreeSet<_>>(),
        BTreeSet::from(["file:latest", "file:guest", "file:answer"])
    );
    assert_eq!(ids.len(), 3);
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_threads SET deleted_at=?1 WHERE thread_id='thread:fork'",[NOW])?;Ok(())}).await.unwrap();
    assert!(
        store
            .attachment_ids(ThreadId("thread:test".into()))
            .await
            .unwrap()
            .contains(&"file:inherited".into())
    );
}

#[tokio::test]
async fn deferred_snapshot_work_carries_a_checked_inclusive_history_cursor() {
    use t3_orchestration::{
        api::*,
        ports::transport::thread_history_paging::{
            ThreadHistoryCursorPayload, decode_thread_history_cursor, encode_thread_history_cursor,
        },
        projections::store::ProjectionStore,
    };
    let db = TestDatabase::memory().unwrap();
    let mut run = fixture("run.created", "run:test");
    run["payload"]["ordinal"] = 1.into();
    run["payload"]["status"] = "completed".into();
    let mut next = run.clone();
    next["payload"]["id"] = "run:next".into();
    next["payload"]["ordinal"] = 2.into();
    let mut events = vec![
        fixture("thread.created", "thread:test"),
        run,
        next,
        conversation_item("item:start", Some("run:test"), 0),
    ];
    for n in 0..160 {
        let mut item = conversation_item(&format!("item:work:{n:03}"), Some("run:test"), n + 1);
        let p = &mut item["payload"];
        p["type"] = "dynamic_tool".into();
        p["toolName"] = "shell.tool".into();
        p["input"] = Value::Null;
        p["output"] = "work output".into();
        events.push(item);
    }
    events.push(conversation_item("item:next", Some("run:next"), 1000000));
    apply_values(&db, events).await;
    let store = ProjectionStore::new(db.stores);
    let snapshot = store
        .snapshot(
            ThreadId("thread:test".into()),
            Some(SnapshotWindow {
                row_limit: 502,
                user_turn_limit: Some(10),
                defer_work: true,
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    let items = snapshot
        .projection
        .turn_items
        .iter()
        .map(|i| serde_json::to_value(i).unwrap())
        .collect::<Vec<_>>();
    let deferred = items
        .iter()
        .filter_map(|i| i.get("deferredWork"))
        .collect::<Vec<_>>();
    assert_eq!(deferred.len(), 1);
    assert_eq!(deferred[0]["itemCount"], 160);
    assert_eq!(deferred[0]["lastOrdinal"], 160);
    let cursor = decode_thread_history_cursor(deferred[0]["cursor"].as_str().unwrap()).unwrap();
    assert_eq!(cursor.p, 160);
    assert_eq!(cursor.si, "item:work:159");
    assert_eq!(cursor.inclusive, Some(true));
    assert_eq!(items.len(), 3);
    let markdown = "## Draft\n\n**Exactly** this Markdown.\n\n```rust\nlet ready = true;\n```";
    let mut omitted_sent_messages = Vec::new();
    for (name, output) in [
        ("t3-code.delegate_task", None),
        ("mcp__t3_code__delegate_task", None),
        ("mcp__t3-code__send_to_user", None),
        (
            "mcp__t3-code__send_to_user",
            Some(serde_json::json!({"sentMessage":markdown}).to_string()),
        ),
        (
            "mcp__t3_code__send_to_user",
            Some(serde_json::json!({"sentMessage":markdown}).to_string()),
        ),
        (
            "send_to_user",
            Some(serde_json::json!({"sentMessage":markdown}).to_string()),
        ),
        (
            "execute",
            Some(serde_json::json!({"sentMessage":markdown}).to_string()),
        ),
    ] {
        let expected_output = output.clone();
        store.database.transaction(move|tx|{tx.execute("UPDATE orchestration_v2_projection_turn_items SET payload_json=json_set(payload_json,'$.toolName',?1,'$.output',?2) WHERE turn_item_id='item:work:000'",t3_persistence::rusqlite::params![name,output])?;Ok(())}).await.unwrap();
        let snapshot = store
            .snapshot(
                ThreadId("thread:test".into()),
                Some(SnapshotWindow {
                    row_limit: 502,
                    user_turn_limit: Some(10),
                    defer_work: true,
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        let items = t3_contracts::stored::to_value(&snapshot.projection.turn_items).unwrap();
        let inline = items
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["ordinal"] == 1)
            .unwrap();
        assert!(
            inline.get("deferredWork").is_none(),
            "{name} must remain inline"
        );
        assert_eq!(inline["output"], serde_json::json!(expected_output));
        assert_eq!(
            items
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row.get("deferredWork").is_some())
                .unwrap()["ordinal"],
            2
        );
        let cursor = encode_thread_history_cursor(&ThreadHistoryCursorPayload {
            v: 1,
            seq: 0,
            st: "thread:test".into(),
            si: "item:next".into(),
            p: 161,
            inclusive: None,
        })
        .unwrap();
        for through in [None, Some("item:work:000".into())] {
            let page = store
                .history(HistoryQuery {
                    thread_id: ThreadId("thread:test".into()),
                    cursor: cursor.clone(),
                    through_entry_id: through,
                    conversation_only: false,
                    work_mode: Some(HistoryWorkMode::Deferred),
                })
                .await
                .unwrap();
            let rows = t3_contracts::stored::to_value(&page.items).unwrap();
            let inline = rows
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["sourceItemId"] == "item:work:000")
                .unwrap();
            assert!(
                inline["item"].get("deferredWork").is_none(),
                "{name} history must remain inline"
            );
            let expected = if expected_output.is_some() {
                serde_json::json!({"sentMessage":markdown})
            } else {
                Value::Null
            };
            if inline["item"]["output"] != expected {
                omitted_sent_messages.push(name);
            }
        }
    }
    assert!(
        omitted_sent_messages.is_empty(),
        "transport wire compaction lost exact sentMessage for {omitted_sent_messages:?}"
    );
}

#[tokio::test]
async fn shell_queries_filter_lineage_worktrees_projects_and_empty_id_scopes() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    let mut parent = fixture("thread.created", "thread:test");
    parent["payload"]["projectId"] = "project:one".into();
    parent["payload"]["worktreePath"] = "C:/Work".into();
    let mut child = fixture("thread.created", "thread:child");
    child["threadId"] = "thread:child".into();
    child["payload"]["projectId"] = "project:one".into();
    child["payload"]["worktreePath"] = "C:/Other".into();
    child["payload"]["lineage"] = serde_json::json!({"parentThreadId":"thread:test","relationshipToParent":"subagent","rootThreadId":"thread:test"});
    let mut archived = child.clone();
    archived["threadId"] = "thread:archived".into();
    archived["payload"]["id"] = "thread:archived".into();
    archived["payload"]["archivedAt"] = NOW.into();
    let mut unrelated = fixture("thread.created", "thread:unrelated");
    unrelated["threadId"] = "thread:unrelated".into();
    unrelated["payload"]["projectId"] = "project:two".into();
    unrelated["payload"]["worktreePath"] = "C:/Work".into();
    apply_values(&db, vec![parent, child, archived, unrelated]).await;
    let store = ProjectionStore::new(db.stores);
    let ids = |rows: Vec<OrchestrationV2ThreadShell>| {
        rows.into_iter()
            .map(|r| {
                serde_json::to_value(r.id)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<BTreeSet<_>>()
    };
    let family = store
        .thread_shells(ShellQuery {
            archived: true,
            family_of: Some(ThreadId("thread:test".into())),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        ids(family.threads),
        BTreeSet::from(["thread:test".into(), "thread:child".into()])
    );
    assert_eq!(
        ids(family.archived_threads),
        BTreeSet::from(["thread:archived".into()])
    );
    let worktree = store
        .thread_shells(ShellQuery {
            worktree_path: Some("C:/Work".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        ids(worktree.threads),
        BTreeSet::from(["thread:test".into(), "thread:unrelated".into()])
    );
    let project = store
        .thread_shells(ShellQuery {
            project_id: Some(ProjectId("project:one".into())),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        ids(project.threads),
        BTreeSet::from(["thread:test".into(), "thread:child".into()])
    );
    let empty = store
        .thread_shells(ShellQuery {
            archived: true,
            thread_ids: Some(vec![]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(empty.threads.is_empty());
    assert!(empty.archived_threads.is_empty());
}

#[tokio::test]
async fn timeline_pages_keep_global_positions_and_attachment_owners() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    let mut events = vec![fixture("thread.created", "thread:test")];
    for n in 0..6 {
        let mut item = fixture("turn-item.updated", &format!("item:{n}"));
        item["payload"] = serde_json::json!({
            "id":format!("item:{n}"), "threadId":"thread:test", "runId":null,
            "nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,
            "ordinal":n,"status":"completed","title":null,"startedAt":null,
            "completedAt":NOW,"updatedAt":NOW,"type":"user_message",
            "messageId":format!("message:{n}"),"text":"hello", "attachments":[],
            "inputIntent":"turn_start","createdBy":"user","creationSource":"web"
        });
        if n == 3 {
            item["payload"]["attachments"] = serde_json::json!([{"type":"file","id":"file:one","name":"a.txt","mimeType":"text/plain","sizeBytes":4}]);
        }
        events.push(item);
    }
    apply_values(&db, events).await;
    let store = ProjectionStore::new(db.stores);
    let query = TimelineQuery {
        thread_id: ThreadId("thread:test".into()),
        before_position: None,
        after_position: Some(1),
        limit: 2,
        item_id: None,
        attachment_id: None,
        view: None,
    };
    let page = store.timeline(query.clone()).await.unwrap();
    assert_eq!(
        page.items.iter().map(|r| r.position).collect::<Vec<_>>(),
        [2, 3]
    );
    assert_eq!(page.total_items, 6);
    assert!(page.has_more);
    assert!(page.has_earlier);
    let page = store
        .timeline(TimelineQuery {
            before_position: Some(4),
            after_position: None,
            ..query.clone()
        })
        .await
        .unwrap();
    assert_eq!(
        page.items.iter().map(|r| r.position).collect::<Vec<_>>(),
        [2, 3]
    );
    assert!(page.has_more);
    assert!(page.has_earlier);
    let page = store
        .timeline(TimelineQuery {
            attachment_id: Some("file:one".into()),
            after_position: None,
            ..query.clone()
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].position, 3);
    let page = store
        .timeline(TimelineQuery {
            item_id: Some(TurnItemId("item:0".into())),
            ..query
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].position, 0);
    use t3_orchestration::ports::transport::thread_find::{
        ThreadFindSnapshotReader, ThreadFindText,
    };
    struct PlainFindText(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    impl ThreadFindText for PlainFindText {
        fn segments(
            &self,
            item: &OrchestrationV2TurnItem,
            _cwd: Option<&str>,
            _skills: &[SearchThreadInputSkillsItem],
        ) -> t3_orchestration::error::Result<Vec<String>> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(match item {
                OrchestrationV2TurnItem::UserMessage(item) => vec![item.text.clone()],
                _ => vec![],
            })
        }
        fn occurrences(&self, text: &str, query: &str) -> u64 {
            text.matches(query).count() as u64
        }
        fn embeds_result(&self, _item: &OrchestrationV2TurnItem) -> bool {
            false
        }
    }
    store.database.transaction(|tx|{
        tx.execute("INSERT INTO projection_projects(project_id,title,workspace_root,scripts_json,created_at,updated_at) SELECT project_id,'Find','/find','[]',?1,?1 FROM orchestration_v2_projection_threads WHERE thread_id='thread:test'",[NOW])?;
        tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json=json_set(payload_json,'$.worktreePath',NULL) WHERE thread_id='thread:test'",[])?;
        tx.execute("INSERT INTO orchestration_v2_projection_turn_items(turn_item_id,thread_id,ordinal,type,status,updated_at,payload_json) VALUES('tool:unread','thread:test',999,'dynamic_tool','completed',?1,'{}')",[NOW])?;
        Ok(())
    }).await.unwrap();
    let snapshot = ThreadFindSnapshotReader::snapshot(&store, ThreadId("thread:test".into()))
        .await
        .unwrap();
    assert_eq!(snapshot.cwd.as_deref(), Some("/find"));
    assert_eq!(
        snapshot
            .payloads
            .iter()
            .map(|row| row.position)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4, 5]
    );
    assert!(snapshot.tool_payloads.is_empty());
    let input: OrchestrationV2SearchThreadInput =
        serde_json::from_value(serde_json::json!({"threadId":"thread:test","query":"hello"}))
            .unwrap();
    assert!(matches!(
        ProjectionReader::search(&store, input.clone()).await,
        Err(t3_orchestration::error::Error::Unsupported(
            "thread find text backend"
        ))
    ));
    let rendered = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let search = store
        .clone()
        .with_thread_find_text(std::sync::Arc::new(PlainFindText(rendered.clone())));
    let result = ProjectionReader::search(&search, input.clone())
        .await
        .unwrap();
    assert_eq!(result.total_matches, 6);
    assert_eq!(result.r#match.unwrap().entry_id.0, "message:0");
    let rendered_before = rendered.load(std::sync::atomic::Ordering::SeqCst);
    assert!(rendered_before > 0);
    let mut stream = search.search_thread_stream(input.clone()).await.unwrap();
    let mut final_result = None;
    while let Some(result) = stream.pull().await.unwrap() {
        final_result = Some(result);
    }
    assert_eq!(final_result.unwrap().total_matches, 6);
    assert_eq!(
        rendered.load(std::sync::atomic::Ordering::SeqCst),
        rendered_before,
        "progressive Find must reuse the normal search cache"
    );
    assert!(matches!(
        store.search_thread_stream(input).await,
        Err(t3_orchestration::error::Error::Unsupported(
            "thread find text backend"
        ))
    ));
    store
        .database
        .transaction(|tx| {
            tx.execute("UPDATE projection_projects SET deleted_at=?1", [NOW])?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(matches!(
        ThreadFindSnapshotReader::snapshot(&store, ThreadId("thread:test".into())).await,
        Err(t3_orchestration::error::Error::ThreadNotFound(_))
    ));
}

#[tokio::test]
async fn projection_upserts_preserve_recorded_fields_and_detach_only_one_binding() {
    let db = TestDatabase::memory().unwrap();
    let mut run = fixture("run.created", "run:test");
    run["payload"]["delegatedCompletion"] =
        serde_json::json!({"nextGeneration":1,"disposition":"open","delivery":null});
    let mut cancellation = sample("run.background-work-cancelled");
    cancellation["threadId"] = "thread:test".into();
    cancellation["payload"]["runId"] = "run:test".into();
    let expected_work = cancellation["payload"]["restartCancelledBackgroundWork"].clone();
    let mut agent = fixture("subagent.updated", "agent:test");
    agent["payload"]["completionDelivery"] =
        serde_json::json!({"state":"claimed","observedByRunId":"run:test"});
    let stale_agent = fixture("subagent.updated", "agent:test");
    let mut session = fixture("provider-session.attached", "session:test");
    let mut shared = session.clone();
    shared["threadId"] = "thread:other".into();
    let mut detached = sample("provider-session.detached");
    detached["threadId"] = "thread:test".into();
    detached["payload"]["providerSessionId"] = "session:test".into();
    session["payload"]["status"] = "running".into();
    let mut stale_run = fixture("run.updated", "run:test");
    stale_run["payload"]["status"] = "completed".into();
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            run,
            cancellation,
            stale_run,
            agent,
            stale_agent,
            session,
            shared,
            detached,
        ],
    )
    .await;
    db.stores.read(move|conn|{
        let run:String=conn.query_row("SELECT payload_json FROM orchestration_v2_projection_runs WHERE run_id='run:test'",[],|row|row.get(0))?;let run:Value=serde_json::from_str(&run)?;
        assert_eq!(run["status"],"completed");assert_eq!(run["delegatedCompletion"]["disposition"],"open");assert_eq!(run["restartCancelledBackgroundWork"],expected_work);
        let delivery:String=conn.query_row("SELECT completion_delivery_state FROM orchestration_v2_projection_subagents WHERE subagent_id='agent:test'",[],|row|row.get(0))?;assert_eq!(delivery,"claimed");
        let bindings:i64=conn.query_row("SELECT COUNT(*) FROM orchestration_v2_projection_provider_session_bindings WHERE provider_session_id='session:test' AND thread_id='thread:other'",[],|row|row.get(0))?;assert_eq!(bindings,1);
        let session:i64=conn.query_row("SELECT COUNT(*) FROM orchestration_v2_projection_provider_sessions WHERE provider_session_id='session:test'",[],|row|row.get(0))?;assert_eq!(session,1);Ok(())
    }).await.unwrap();
}

#[tokio::test]
async fn counts_and_search_follow_upsert_move_stream_settlement_and_rollback() {
    let db = TestDatabase::memory().unwrap();
    let mut item = fixture("turn-item.updated", "item:test");
    item["payload"]["runId"] = "run:test".into();
    let mut moved = item.clone();
    moved["payload"]["runId"] = "run:other".into();
    let mut streaming = fixture("message.updated", "message:test");
    streaming["payload"]["streaming"] = true.into();
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            item.clone(),
            item,
            moved,
            streaming,
        ],
    )
    .await;
    db.stores.read(|conn|{let count:i64=conn.query_row("SELECT item_count FROM orchestration_v2_projection_turn_item_counts WHERE thread_id='thread:test' AND run_key='run:other'",[],|row|row.get(0))?;assert_eq!(count,1);let old:i64=conn.query_row("SELECT COUNT(*) FROM orchestration_v2_projection_turn_item_counts WHERE run_key='run:test'",[],|row|row.get(0))?;assert_eq!(old,0);let search:i64=conn.query_row("SELECT COUNT(*) FROM orchestration_v2_message_search",[],|row|row.get(0))?;assert_eq!(search,0);Ok(())}).await.unwrap();
    apply_values(&db, vec![fixture("message.updated", "message:test")]).await;
    db.stores.read(|conn|{let search:i64=conn.query_row("SELECT COUNT(*) FROM orchestration_v2_message_search WHERE orchestration_v2_message_search MATCH 'projection'",[],|row|row.get(0))?;assert_eq!(search,1);Ok(())}).await.unwrap();
    let failed: t3_orchestration::error::Result<()> = db
        .stores
        .transaction(|tx| {
            tx.execute("DELETE FROM orchestration_v2_projection_messages", [])?;
            tx.execute("DELETE FROM orchestration_v2_projection_turn_items", [])?;
            Err(t3_orchestration::error::Error::Stale)
        })
        .await;
    assert!(failed.is_err());
    db.stores
        .read(|conn| {
            let count: i64 = conn.query_row(
                "SELECT SUM(item_count) FROM orchestration_v2_projection_turn_item_counts",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(count, 1);
            let search: i64 = conn.query_row(
                "SELECT COUNT(*) FROM orchestration_v2_message_search",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(search, 1);
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn full_projection_uses_reader_snapshot_without_uncommitted_writer_state() {
    use t3_contracts::base_schemas::ThreadId;
    use t3_orchestration::{api::ProjectionReader, projections::store::ProjectionStore};
    let db = TestDatabase::temporary().unwrap();
    apply_values(&db, vec![fixture("thread.created", "thread:test")]).await;
    let old = ProjectionStore::new(db.stores.clone())
        .projection(ThreadId("thread:test".into()))
        .await
        .unwrap()
        .unwrap();
    let stores = db.stores.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let writer = tokio::spawn(async move {
        stores.transaction(move|tx|{
        tx.execute("UPDATE orchestration_v2_projection_threads SET title='committed later',payload_json=json_set(payload_json,'$.title','committed later') WHERE thread_id='thread:test'",[])?;
        ready_tx.send(()).unwrap();release_rx.recv().unwrap();Ok(())
    }).await.unwrap();
    });
    ready_rx.await.unwrap();
    let store = ProjectionStore::new(db.stores.clone());
    let visible = store
        .projection(ThreadId("thread:test".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(visible.thread.title, old.thread.title);
    release_tx.send(()).unwrap();
    writer.await.unwrap();
    assert_eq!(
        store
            .projection(ThreadId("thread:test".into()))
            .await
            .unwrap()
            .unwrap()
            .thread
            .title
            .0,
        "committed later"
    );
}

#[tokio::test]
async fn persisted_unknown_turn_items_fail_internal_reads() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            fixture("turn-item.updated", "item:test"),
        ],
    )
    .await;
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_turn_items SET payload_json=json_set(payload_json,'$.type','future-item') WHERE turn_item_id='item:test'",[])?;Ok(())}).await.unwrap();
    let store = ProjectionStore::new(db.stores.clone());
    assert!(
        store
            .projection(ThreadId("thread:test".into()))
            .await
            .is_err()
    );
    assert!(
        store
            .turn_item(
                ThreadId("thread:test".into()),
                TurnItemId("item:test".into())
            )
            .await
            .is_err()
    );
    db.stores.transaction(|tx|{tx.execute("INSERT INTO orchestration_v2_projection_plans(plan_id,thread_id,node_id,kind,status,payload_json) VALUES('plan:corrupt','thread:test','node:test','proposed_plan','active','{broken')",[])?;Ok(())}).await.unwrap();
    assert!(matches!(
        store
            .plan(
                ThreadId("thread:test".into()),
                PlanId("plan:corrupt".into())
            )
            .await,
        Err(t3_orchestration::error::Error::Json(_))
    ));
}

#[tokio::test]
async fn fork_shells_count_imported_runless_history_while_side_chats_keep_only_local_rows() {
    use t3_orchestration::{
        api::*,
        projections::{read, store::ProjectionStore},
    };
    let db = TestDatabase::memory().unwrap();
    let mut source = fixture("thread.created", "thread:test");
    source["payload"]["historyOrigin"] = "v1_import".into();
    let mut run = fixture("run.created", "run:test");
    run["payload"]["ordinal"] = 1.into();
    run["payload"]["status"] = "completed".into();
    let mut events = vec![source, run];
    for (name, side) in [("fork", false), ("side", true)] {
        let id = format!("thread:{name}");
        let mut event = fixture("thread.created", &id);
        event["threadId"] = id.clone().into();
        event["payload"]["forkedFrom"] =
            serde_json::json!({"type":"run","threadId":"thread:test","runId":"run:test"});
        event["payload"]["lineage"] = serde_json::json!({"parentThreadId":"thread:test","relationshipToParent":"fork","rootThreadId":"thread:test","sideChat":side});
        events.push(event);
    }
    for (id, run, ordinal) in [
        ("imported:1", None, 1),
        ("imported:2", None, 2),
        ("native", Some("run:test"), 3),
    ] {
        events.push(conversation_item(id, run, ordinal));
    }
    apply_values(&db, events).await;
    let store = ProjectionStore::new(db.stores.clone());
    for (id, count) in [("thread:fork", 4), ("thread:side", 0)] {
        let id = ThreadId(id.into());
        let shell = store.shell(id.clone()).await.unwrap().unwrap();
        let raw = t3_contracts::stored::to_value(&shell).unwrap();
        assert_eq!(raw["itemCount"], 0);
        assert_eq!(raw["visibleItemCount"], count);
        let projection = store.snapshot(id.clone(), None).await.unwrap().projection;
        assert_eq!(projection.visible_turn_items.len(), count as usize);
        assert_eq!(read::shell_from_projection(&projection).unwrap(), shell);
        assert_eq!(
            store
                .timeline(TimelineQuery {
                    thread_id: id,
                    before_position: None,
                    after_position: None,
                    limit: 20,
                    item_id: None,
                    attachment_id: None,
                    view: Some(TimelineView::Activity)
                })
                .await
                .unwrap()
                .total_items,
            count as u64
        );
    }
}

#[tokio::test]
async fn fork_visibility_survives_source_deletion_and_later_rollback() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    let mut first = fixture("run.created", "run:first");
    first["payload"]["status"] = "completed".into();
    let mut second = fixture("run.created", "run:second");
    second["payload"]["ordinal"] = 2.into();
    second["payload"]["status"] = "completed".into();
    let mut first_item = fixture("turn-item.updated", "item:first");
    first_item["payload"]["runId"] = "run:first".into();
    first_item["payload"]["ordinal"] = 1.into();
    let mut second_item = fixture("turn-item.updated", "item:second");
    second_item["payload"]["runId"] = "run:second".into();
    second_item["payload"]["ordinal"] = 2.into();
    let mut child = fixture("thread.created", "thread:child");
    child["threadId"] = "thread:child".into();
    child["payload"]["forkedFrom"] =
        serde_json::json!({"type":"run","threadId":"thread:test","runId":"run:first"});
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            first.clone(),
            second,
            first_item,
            second_item,
            child.clone(),
        ],
    )
    .await;
    let store = ProjectionStore::new(db.stores.clone());
    let visible = store
        .projection(ThreadId("thread:child".into()))
        .await
        .unwrap()
        .unwrap()
        .visible_turn_items;
    assert_eq!(visible.len(), 2);
    assert_eq!(
        serde_json::to_value(&visible[0].item).unwrap()["id"],
        "item:first"
    );
    assert_eq!(
        visible[0].visibility,
        ProjectedTurnItemVisibility::Inherited
    );
    assert_eq!(
        visible[1].visibility,
        ProjectedTurnItemVisibility::Synthetic
    );
    first["payload"]["status"] = "rolled_back".into();
    first["payload"]["completedAt"] = "2026-11-13T11:00:00.000Z".into();
    apply_values(&db, vec![first.clone()]).await;
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_threads SET deleted_at='2026-11-13T11:00:00.000Z',payload_json=json_set(payload_json,'$.deletedAt','2026-11-13T11:00:00.000Z') WHERE thread_id='thread:test'",[])?;Ok(())}).await.unwrap();
    assert_eq!(
        store
            .projection(ThreadId("thread:child".into()))
            .await
            .unwrap()
            .unwrap()
            .visible_turn_items,
        visible
    );
    first["payload"]["completedAt"] = "2026-11-13T09:00:00.000Z".into();
    apply_values(&db, vec![first]).await;
    assert_eq!(
        store
            .projection(ThreadId("thread:child".into()))
            .await
            .unwrap()
            .unwrap()
            .visible_turn_items
            .len(),
        1
    );
    child["payload"]["lineage"]["sideChat"] = true.into();
    apply_values(&db, vec![child]).await;
    assert!(
        store
            .projection(ThreadId("thread:child".into()))
            .await
            .unwrap()
            .unwrap()
            .visible_turn_items
            .is_empty()
    );
}

#[tokio::test]
async fn fork_history_excludes_undelivered_inputs_and_pre_fork_rewinds_but_keeps_later_rewinds() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    let mut root = fixture("thread.created", "thread:test");
    root["payload"]["createdAt"] = "2026-11-13T06:00:00.000Z".into();
    let mut events = vec![root];
    for (n, name, status, intent, rollback) in [
        (1, "first", "completed", "turn_start", None),
        (2, "queued", "queued", "queued_turn", None),
        (3, "later", "completed", "turn_start", None),
        (4, "cancelled", "cancelled", "queued_turn", None),
        (
            5,
            "rewound-before",
            "rolled_back",
            "turn_start",
            Some("2026-11-13T08:00:00.000Z"),
        ),
        (
            6,
            "rewound-after",
            "rolled_back",
            "turn_start",
            Some("2026-11-13T11:00:00.000Z"),
        ),
        (
            7,
            "rewound-held",
            "rolled_back",
            "queued_turn",
            Some("2026-11-13T11:00:00.000Z"),
        ),
    ] {
        let run_id = format!("fork-run:{name}");
        let mut run = fixture("run.created", &run_id);
        run["payload"]["ordinal"] = n.into();
        run["payload"]["status"] = status.into();
        run["payload"]["requestedAt"] = "2026-11-13T07:00:00.000Z".into();
        run["payload"]["startedAt"] = if matches!(name, "queued" | "cancelled" | "rewound-held") {
            Value::Null
        } else {
            "2026-11-13T07:00:00.000Z".into()
        };
        run["payload"]["completedAt"] = if status == "queued" {
            Value::Null
        } else {
            rollback.unwrap_or("2026-11-13T09:00:00.000Z").into()
        };
        let mut item = conversation_item(&format!("fork-item:{name}"), Some(&run_id), n * 1000000);
        item["payload"]["inputIntent"] = intent.into();
        events.extend([run, item]);
        if name == "rewound-held" {
            let mut attempt = fixture("run-attempt.created", "fork-attempt:held");
            attempt["payload"]["runId"] = run_id.into();
            attempt["payload"]["status"] = "cancelled".into();
            events.push(attempt);
        }
    }
    let mut fork = fixture("thread.created", "thread:fork-queued");
    fork["threadId"] = "thread:fork-queued".into();
    fork["payload"]["lineage"] = serde_json::json!({"parentThreadId":"thread:test","relationshipToParent":"fork","rootThreadId":"thread:test"});
    fork["payload"]["forkedFrom"] =
        serde_json::json!({"type":"run","threadId":"thread:test","runId":"fork-run:rewound-held"});
    events.push(fork);
    apply_values(&db, events).await;
    let store = ProjectionStore::new(db.stores);
    let id = ThreadId("thread:fork-queued".into());
    let projection = store.snapshot(id.clone(), None).await.unwrap().projection;
    let expected = vec![
        "fork-item:first",
        "fork-item:later",
        "fork-item:rewound-after",
        "turn-item:fork:thread:fork-queued",
    ];
    assert_eq!(
        projection
            .visible_turn_items
            .iter()
            .map(
                |row| t3_contracts::stored::to_value(&row.item).unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            )
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        store
            .shell(id.clone())
            .await
            .unwrap()
            .unwrap()
            .visible_item_count,
        4
    );
    let bounded = store
        .snapshot(
            id.clone(),
            Some(SnapshotWindow {
                row_limit: 100,
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .projection;
    assert_eq!(bounded.visible_turn_items, projection.visible_turn_items);
    let timeline = store
        .timeline(TimelineQuery {
            thread_id: id,
            before_position: None,
            after_position: None,
            limit: 100,
            item_id: None,
            attachment_id: None,
            view: Some(TimelineView::Activity),
        })
        .await
        .unwrap();
    assert_eq!(timeline.items, projection.visible_turn_items);
    assert_eq!(timeline.total_items, 4);
}

#[tokio::test]
async fn provider_usage_and_placeholder_identity_are_preserved() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    let mut provider = fixture("provider-thread.updated", "provider:test");
    provider["payload"]["status"] = "not_loaded".into();
    provider["payload"]["firstRunOrdinal"] = Value::Null;
    provider["payload"]["providerSessionId"] = Value::Null;
    apply_values(
        &db,
        vec![fixture("thread.created", "thread:test"), provider],
    )
    .await;
    let store = ProjectionStore::new(db.stores.clone());
    assert!(
        store
            .thread(ThreadId("thread:test".into()))
            .await
            .unwrap()
            .unwrap()
            .active_provider_thread_id
            .is_none()
    );
    apply_values(
        &db,
        vec![fixture("provider-thread.updated", "provider:test")],
    )
    .await;
    assert!(
        store
            .thread(ThreadId("thread:test".into()))
            .await
            .unwrap()
            .unwrap()
            .active_provider_thread_id
            .is_some()
    );
    let mut usage = fixture("provider-turn.updated", "turn:test")["payload"].clone();
    usage["tokenUsage"] = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2ProviderTurnTokenUsage.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    usage["id"] = "turn:test".into();
    usage["providerThreadId"] = "provider:test".into();
    usage["nodeId"] = "node:test".into();
    usage["ordinal"] = 1.into();
    let expected = usage["tokenUsage"].clone();
    let mut event = fixture("provider-turn.updated", "turn:test");
    event["payload"] = usage;
    apply_values(
        &db,
        vec![event, fixture("provider-turn.updated", "turn:test")],
    )
    .await;
    let values = store
        .records(
            ThreadId("thread:test".into()),
            vec![RecordKind::ProviderTurns],
            RecordFilter::default(),
        )
        .await
        .unwrap();
    let ProjectionRecordSet::ProviderTurns(turns) = &values.records[0] else {
        panic!("wrong record set");
    };
    assert_eq!(
        serde_json::to_value(&turns[0]).unwrap()["tokenUsage"],
        expected
    );
    let mut newest = fixture("provider-turn.updated", "turn:newest-usage");
    newest["payload"]["ordinal"] = 0.into();
    newest["payload"]["tokenUsage"] = expected;
    newest["payload"]["tokenUsage"]["updatedAt"] = "2099-01-01T00:00:00.000Z".into();
    apply_values(&db, vec![newest]).await;
    let records = store
        .records(
            ThreadId("thread:test".into()),
            vec![RecordKind::ProviderTurns],
            RecordFilter {
                latest_provider_turn_usage_only: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let ProjectionRecordSet::ProviderTurns(turns) = &records.records[0] else {
        panic!("wrong records")
    };
    assert_eq!(turns.len(), 1);
    assert_eq!(
        t3_contracts::stored::to_value(&turns[0].id).unwrap(),
        "turn:newest-usage"
    );
    let records = store
        .records(
            ThreadId("thread:test".into()),
            vec![RecordKind::ProviderTurns],
            RecordFilter {
                provider_turn_thread_ids: Some(vec![]),
                latest_provider_turn_usage_only: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let ProjectionRecordSet::ProviderTurns(turns) = &records.records[0] else {
        panic!("wrong records")
    };
    assert!(turns.is_empty());
}

#[tokio::test]
async fn every_turn_item_variant_matches_typescript_payload_bytes() {
    use t3_contracts::{orchestration_v2::OrchestrationV2TurnItem, wire::TaggedUnion};
    let oracle: Vec<Value> = serde_json::from_str(TYPESCRIPT_TURN_ITEM_PAYLOADS).unwrap();
    let mut tags = BTreeSet::new();
    for case in oracle {
        let event = case["event"].clone();
        tags.insert(event["payload"]["type"].as_str().unwrap().to_owned());
        let db = TestDatabase::memory().unwrap();
        apply_values(&db, vec![event]).await;
        db.stores
            .read(move |conn| {
                let raw: String = conn.query_row(
                    "SELECT payload_json FROM orchestration_v2_projection_turn_items",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(raw, case["payload_json"].as_str().unwrap());
                Ok(())
            })
            .await
            .unwrap();
    }
    assert_eq!(
        tags,
        OrchestrationV2TurnItem::KNOWN_TAGS
            .iter()
            .map(|tag| (*tag).to_owned())
            .collect()
    );
}

#[tokio::test]
async fn upstream_active_listing_reads_metadata_without_shells_or_transcripts() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    let mut events = Vec::new();
    for (name, project, archived, deleted) in [
        ("root", "project:one", false, false),
        ("child", "project:one", false, false),
        ("other", "project:two", false, false),
        ("archived", "project:one", true, false),
        ("deleted", "project:one", false, true),
    ] {
        let id = format!("thread:list:{name}");
        let mut event = fixture("thread.created", &id);
        event["threadId"] = id.clone().into();
        event["payload"]["projectId"] = project.into();
        event["payload"]["title"] = name.into();
        if archived {
            event["payload"]["archivedAt"] = NOW.into();
        }
        if deleted {
            event["payload"]["deletedAt"] = NOW.into();
        }
        if name == "child" {
            event["payload"]["lineage"] = serde_json::json!({"rootThreadId":"thread:list:root","parentThreadId":"thread:list:root","relationshipToParent":"subagent","sideChat":true});
            event["payload"]["snoozedUntil"] = "2026-11-14T10:00:00.000Z".into();
            event["payload"]["settledOverride"] = "settled".into();
            event["payload"]["settledAt"] = NOW.into();
        }
        events.push(event);
    }
    apply_values(&db, events).await;
    db.stores.transaction(|tx|{
        tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json=json_set(payload_json,'$.modelSelection',37,'$.title',37) WHERE deleted_at IS NULL AND archived_at IS NULL",[])?;
        tx.execute("DROP TABLE orchestration_v2_projection_turn_item_counts",[])?;
        Ok(())
    }).await.unwrap();
    let store = ProjectionStore::new(db.stores.clone());
    assert!(store.thread_shells(ShellQuery::default()).await.is_err());
    let all = store.list_active_threads(None).await.unwrap();
    let ids = all
        .iter()
        .map(|row| row.id.0.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        ids,
        ["thread:list:root", "thread:list:child", "thread:list:other"]
            .into_iter()
            .collect()
    );
    let root = all.iter().find(|row| row.title == "root").unwrap();
    assert_eq!(root.updated_at.0, NOW);
    assert_eq!(root.lineage.side_chat, None);
    let child = all.iter().find(|row| row.title == "child").unwrap();
    assert_eq!(
        t3_contracts::stored::to_value(&child.lineage).unwrap(),
        serde_json::json!({"parentThreadId":"thread:list:root","relationshipToParent":"subagent","rootThreadId":"thread:list:root","sideChat":true})
    );
    assert_eq!(
        child.settled_override,
        Some(AppThreadSettledOverride::Settled)
    );
    assert_eq!(
        child.snoozed_until.as_ref().unwrap().0,
        "2026-11-14T10:00:00.000Z"
    );
    let scoped = store
        .list_active_threads(Some(ProjectId("project:two".into())))
        .await
        .unwrap();
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].title, "other");
    store.database.transaction(|tx| {
        tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json=json_set(payload_json,'$.lineage.relationshipToParent','unknown','$.lineage.sideChat',2,'$.settledOverride','unknown') WHERE thread_id='thread:list:root'", [])?;
        Ok(())
    }).await.unwrap();
    let all = store.list_active_threads(None).await.unwrap();
    let root = all.iter().find(|row| row.title == "root").unwrap();
    assert_eq!(root.lineage.relationship_to_parent, None);
    assert_eq!(root.lineage.side_chat, Some(Some(false)));
    assert_eq!(root.settled_override, None);
    let index = store
        .read_snapshot(t3_orchestration::projections::read::active_shell_index)
        .await
        .unwrap();
    let root_index = index.iter().find(|row| row.id == root.id).unwrap();
    assert_eq!(root_index.lineage, root.lineage);
    assert_eq!(root_index.settled_override, None);
}

#[tokio::test]
async fn upstream_message_selection_flag_survives_sql_and_memory_projection() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    let created = fixture("thread.created", "thread:test");
    let mut memory =
        write::empty_projection(&serde_json::from_value(created.clone()).unwrap()).unwrap();
    apply_values(&db, vec![created]).await;
    let store = ProjectionStore::new(db.stores.clone());
    for flag in [Some(true), None] {
        let mut message = fixture("message.updated", "message:selection");
        if let Some(flag) = flag {
            message["payload"]["followsThreadSelection"] = flag.into();
        } else {
            message["payload"]
                .as_object_mut()
                .unwrap()
                .remove("followsThreadSelection");
        }
        let event = serde_json::from_value(message.clone()).unwrap();
        memory = write::apply_to_projection(&memory, &event).unwrap();
        apply_values(&db, vec![message]).await;
        let projected = store
            .projection(ThreadId("thread:test".into()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(projected, memory);
        let payload=db.stores.read(|db|Ok(db.query_row::<String,_,_>("SELECT payload_json FROM orchestration_v2_projection_messages WHERE message_id='message:selection'",[],|r|r.get(0))?)).await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&payload)
                .unwrap()
                .get("followsThreadSelection")
                .cloned(),
            flag.map(Value::Bool)
        );
    }
}

#[tokio::test]
async fn activity_clocks_and_memory_projection_preserve_thread_state() {
    use t3_orchestration::{api::*, projections::store::ProjectionStore};
    let db = TestDatabase::memory().unwrap();
    let mut created = fixture("thread.created", "thread:test");
    created["payload"]["opens"] = serde_json::json!({"openedAt":123,"score":2,"picks":[]});
    let mut run = fixture("run.created", "run:test");
    run["occurredAt"] = "2026-11-13T12:00:00.000Z".into();
    let mut memory =
        write::empty_projection(&serde_json::from_value(created.clone()).unwrap()).unwrap();
    memory =
        write::apply_to_projection(&memory, &serde_json::from_value(run.clone()).unwrap()).unwrap();
    apply_values(&db, vec![created, run]).await;
    let store = ProjectionStore::new(db.stores.clone());
    let projected = store
        .projection(ThreadId("thread:test".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(projected, memory);
    let mut visited = serde_json::json!({"id":"event:visited","type":"thread.visited","threadId":"thread:test","occurredAt":"2026-11-13T13:00:00.000Z","payload":serde_json::to_value(&projected.thread).unwrap()});
    visited["payload"]["lastVisitedAt"] = "2026-11-13T13:00:00.000Z".into();
    apply_values(&db, vec![visited.clone()]).await;
    memory =
        write::apply_to_projection(&memory, &serde_json::from_value(visited).unwrap()).unwrap();
    let projected = store
        .projection(ThreadId("thread:test".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(projected, memory);
    assert_eq!(projected.updated_at.0, "2026-11-13T12:00:00.000Z");
    let visits = include_str!(
        "../../../../fixtures/stored/OrchestrationV2DomainEventJson.thread.visit-recorded.jsonl"
    );
    let mut payloads = visits
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str::<Value>(line).unwrap()["payload"].clone())
        .collect::<Vec<_>>();
    payloads.extend([
        serde_json::json!({"lastVisitedAt":NOW,"opens":{"openedAt":456,"score":3,"picks":[]}}),
        serde_json::json!({"lastVisitedAt":null}),
        serde_json::json!({"lastVisitedAt":NOW,"opens":null}),
    ]);
    for (n, payload) in payloads.into_iter().enumerate() {
        let raw = serde_json::json!({"id":format!("event:compact-visit:{n}"),"threadId":"thread:test","occurredAt":"2026-11-13T15:00:00.000Z","type":"thread.visit-recorded","payload":payload});
        let event: OrchestrationV2DomainEvent = serde_json::from_value(raw.clone()).unwrap();
        let encoded = t3_contracts::stored::to_value(&event).unwrap();
        let before = t3_contracts::stored::to_value(&memory).unwrap();
        apply_values(&db, vec![raw]).await;
        let sql = store
            .projection(ThreadId("thread:test".into()))
            .await
            .unwrap()
            .unwrap();
        let actual = t3_contracts::stored::to_value(&sql).unwrap();
        assert_eq!(
            actual["thread"]["lastVisitedAt"], encoded["payload"]["lastVisitedAt"],
            "compact visit {n}"
        );
        assert_eq!(
            actual["thread"]["opens"],
            encoded["payload"]
                .get("opens")
                .unwrap_or(&before["thread"]["opens"])
                .clone()
        );
        let mut unchanged = actual.clone();
        unchanged["thread"]["lastVisitedAt"] = before["thread"]["lastVisitedAt"].clone();
        if let Some(opens) = before["thread"].get("opens") {
            unchanged["thread"]["opens"] = opens.clone();
        } else {
            unchanged["thread"].as_object_mut().unwrap().remove("opens");
        }
        assert_eq!(
            unchanged, before,
            "visit must preserve every other projected field"
        );
        memory = write::apply_to_projection(&memory, &event).unwrap();
        assert_eq!(sql, memory);
        let clocks=db.stores.read(|db|Ok(db.query_row::<String,_,_>("SELECT updated_at FROM orchestration_v2_projection_threads WHERE thread_id='thread:test'",[],|row|row.get(0))?)).await.unwrap();
        assert_eq!(clocks, "2026-11-13T12:00:00.000Z");
    }
}

#[tokio::test]
async fn search_indexes_readable_citations_and_preserves_invalid_links() {
    let db = TestDatabase::memory().unwrap();
    let mut message = fixture("message.updated", "message:test");
    message["payload"]["text"]="[Assistant quote](t3-citation://v1/env/thread/message?text=Caf%C3%A9+quote&start=0&end=10&prefix&suffix=&comment=hello)".into();
    apply_values(
        &db,
        vec![fixture("thread.created", "thread:test"), message.clone()],
    )
    .await;
    db.stores.read(|conn|{let text:String=conn.query_row("SELECT search_text FROM orchestration_v2_projection_messages WHERE message_id='message:test'",[],|row|row.get(0))?;assert_eq!(text,"Café quote\nComment: hello");Ok(())}).await.unwrap();
    message["payload"]["text"]="[Assistant quote](t3-citation://v1/env/thread/message?text=quote&start=0&end=5&prefix=&suffix=&text=duplicate)".into();
    apply_values(&db, vec![message]).await;
    db.stores.read(|conn|{let text:Option<String>=conn.query_row("SELECT search_text FROM orchestration_v2_projection_messages WHERE message_id='message:test'",[],|row|row.get(0))?;assert_eq!(text,None);Ok(())}).await.unwrap();
}

#[tokio::test]
async fn compact_identity_is_rejected_before_a_durable_projection_is_written() {
    let db = TestDatabase::memory().unwrap();
    let mut raw = fixture("thread.created", "thread:test");
    raw["payload"]["id"] = 0.into();
    let error = db
        .stores
        .transaction(move |tx| {
            write::apply(
                tx,
                &OrchestrationV2StoredEvent {
                    sequence: 1,
                    command_id: None,
                    event: serde_json::from_value(raw)?,
                },
            )
        })
        .await;
    assert!(matches!(
        error,
        Err(t3_orchestration::error::Error::Decode { .. })
    ));
    db.stores
        .read(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM orchestration_v2_projection_threads",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(count, 0);
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn projection_native_json_uses_the_shared_typescript_number_and_key_encoder() {
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/event-compatibility.json")).unwrap();
    let event = fixture["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["payload"]["id"] == "numeric:tool")
        .unwrap()
        .clone();
    let row = fixture["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["event_id"] == event["id"])
        .unwrap()
        .clone();
    let expected = row["payload_json"].as_str().unwrap().to_owned();
    let stored = serde_json::from_value::<t3_orchestration::stores::event_store::EventRow>(row)
        .unwrap()
        .decode_agent()
        .unwrap();
    let db = TestDatabase::memory().unwrap();
    db.stores
        .transaction(move |tx| write::apply(tx, &stored))
        .await
        .unwrap();
    db.stores.read(|conn| {
        let items:Vec<t3_contracts::orchestration_v2::OrchestrationV2TurnItem>=t3_orchestration::projections::read::rows(conn,"SELECT payload_json FROM orchestration_v2_projection_turn_items WHERE turn_item_id='numeric:tool'",[])?;
        assert_eq!(items.len(),1);
        Ok(())
    }).await.unwrap();
    db.stores.read(move|conn|{let actual:String=conn.query_row("SELECT payload_json FROM orchestration_v2_projection_turn_items WHERE turn_item_id='numeric:tool'",[],|row|row.get(0))?;if actual!=expected {let at=actual.bytes().zip(expected.bytes()).position(|(a,b)|a!=b).unwrap_or(actual.len().min(expected.len()));panic!("projection JSON differs at byte {at}: actual {:?}, expected {:?}",actual.get(at.saturating_sub(45)..(at+100).min(actual.len())),expected.get(at.saturating_sub(45)..(at+100).min(expected.len())));}Ok(())}).await.unwrap();
}

#[tokio::test]
async fn projection_json_columns_use_stored_dialect_and_export_rust_rows_for_typescript() {
    let mut exported = Vec::new();
    let mut tables = BTreeSet::new();
    let mut undefined_cases = 0;
    for &(tag, samples) in EVENT_SAMPLES {
        let original: Value = serde_json::from_str(samples.lines().next().unwrap()).unwrap();
        let undefined_key = match tag {
            "thread.created" => Some("historyOrigin"),
            "run.created" => Some("queueHeld"),
            "run-attempt.created" => Some("nativeThreadId"),
            "subagent.updated" => Some("progress"),
            "provider-session.attached" => Some("extensionStatuses"),
            "provider-thread.updated" => Some("pendingBackgroundTasks"),
            "provider-turn.updated" => Some("model"),
            "runtime-request.updated" => Some("feedback"),
            "message.updated" => Some("workflowScript"),
            "turn-item.updated" => Some("toolSurface"),
            "context-handoff.updated" => Some("detailInTurnItem"),
            "context-transfer.updated" => Some("sourceTokenUsage"),
            _ => None,
        };
        for undefined in [false, true] {
            if undefined && undefined_key.is_none() {
                continue;
            }
            let db = TestDatabase::memory().unwrap();
            let mut raw = original.clone();
            if undefined {
                raw["payload"][undefined_key.unwrap()] = Value::Null;
                undefined_cases += 1;
            }
            let event: OrchestrationV2DomainEvent = serde_json::from_value(raw.clone()).unwrap();
            if undefined {
                let rpc = serde_json::to_value(&event).unwrap();
                assert!(
                    rpc["payload"].get(undefined_key.unwrap()).is_some(),
                    "{tag} must exercise an explicit RPC undefined value"
                );
                assert_eq!(rpc["payload"][undefined_key.unwrap()], Value::Null);
                assert!(
                    t3_contracts::stored::to_value(&event).unwrap()["payload"]
                        .get(undefined_key.unwrap())
                        .is_none(),
                    "{tag}: undefined must be absent in stored dialect"
                );
            }
            let mut seed = sample("thread.created");
            seed["threadId"] = raw["threadId"].clone();
            seed["payload"]["id"] = raw["threadId"].clone();
            apply_values(&db, vec![seed, raw]).await;
            let case = format!("{tag}:{}", if undefined { "undefined" } else { "sample" });
            let rows=db.stores.read(move|conn|{
                let mut rows=Vec::new();
                for table in ["threads","runs","run_attempts","nodes","subagents","provider_sessions","provider_threads","provider_turns","runtime_requests","messages","plans","turn_items","checkpoint_scopes","checkpoints","context_handoffs","context_transfers"] {
                    let mut statement=conn.prepare(&format!("SELECT payload_json FROM orchestration_v2_projection_{table} ORDER BY rowid"))?;
                    for json in statement.query_map([],|r|r.get::<_,String>(0))? {
                        let json=json?;
                        if undefined {
                            assert!(serde_json::from_str::<Value>(&json)?.get(undefined_key.unwrap()).is_none_or(|value| !value.is_null()),"{case}: {table} wrote undefined as null");
                        }
                        rows.push(serde_json::json!({"case":case,"table":table,"column":"payload_json","payload_json":json}));
                    }
                }
                Ok(rows)
            }).await.unwrap();
            for row in &rows {
                tables.insert(row["table"].as_str().unwrap().to_owned());
            }
            exported.extend(rows);
        }
    }
    assert_eq!(tables.len(), 16);
    assert_eq!(undefined_cases, 12);
    // Optional artifact export is the same Rust-to-TypeScript check used by the event store.
    if let Ok(path) = std::env::var("T3_PROJECTION_ROWS_OUT") {
        let rows = exported
            .iter()
            .map(|row| t3_contracts::stored::to_string(row).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, rows + "\n").unwrap();
        std::fs::write(
            std::path::Path::new(&path).with_extension("verify.mjs"),
            TYPESCRIPT_PROJECTION_DECODER,
        )
        .unwrap();
    }
}

const TYPESCRIPT_PROJECTION_DECODER: &str = r#"
import * as FS from 'node:fs/promises';
import { resolve } from 'node:path';
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';
const [repo, file] = process.argv.slice(2);
if (!repo || !file) throw new Error('usage: node rows.verify.mjs <repository> <rows.jsonl>');
const require=createRequire(resolve(repo,'apps/server/package.json'));
const Schema=await import(pathToFileURL(require.resolve('effect/Schema')).href);
const contracts=await import(pathToFileURL(resolve(repo,'packages/contracts/src/index.ts')).href);
const names={threads:'AppThreadJson',runs:'RunJson',run_attempts:'RunAttemptJson',nodes:'ExecutionNodeJson',subagents:'SubagentJson',provider_sessions:'ProviderSessionJson',provider_threads:'ProviderThreadJson',provider_turns:'ProviderTurnJson',runtime_requests:'RuntimeRequestJson',messages:'ConversationMessageJson',plans:'PlanArtifact',turn_items:'TurnItemJson',checkpoint_scopes:'CheckpointScopeJson',checkpoints:'CheckpointJson',context_handoffs:'ContextHandoffJson',context_transfers:'ContextTransferJson'};
const rows=(await FS.readFile(file,'utf8')).trim().split('\n').map(JSON.parse);
const covered=new Set();const failures=[];
for(const row of rows){
  const schema=contracts['OrchestrationV2'+names[row.table]];
  assert(schema,`unknown projection table ${row.table}`);
  try{Schema.decodeUnknownSync(Schema.fromJsonString(schema))(row.payload_json);covered.add(row.table+'.'+row.column);}
  catch(error){failures.push(`${row.case} ${row.table}.${row.column}: ${error.message}`);}
}
assert.equal(covered.size,16,'every projection JSON column must be checked');
console.log(`Rust projection rows: ${rows.length} decoded by the real TypeScript schemas across ${covered.size} JSON columns; ${failures.length} failures.`);
for(const failure of failures.slice(0,20))console.error(failure);
assert.equal(failures.length,0);
"#;

// Captured from the real TypeScript ProjectionStore against migrated in-memory SQLite.
const TYPESCRIPT_PROJECTION_ROWS: &str = r###"{
  "thread.created": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.archived": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.unarchived": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.deleted": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.settled": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.unsettled": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.snoozed": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.unsnoozed": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.pinned": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.auto-settle-set": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.unpinned": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.pin-reordered": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.active-reordered": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.tags-updated": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.visited": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.marked-unread": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.metadata-updated": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.pull-request-synced": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.runtime-mode-updated": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.interaction-mode-updated": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.model-selection-updated": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.provider-switched": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.guest-share-updated": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      },
      {
        "thread_id": "R",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"R\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.pull-request-link-updated": {
    "threads": [
      {
        "thread_id": "\u0000",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"\\u0000\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"pullRequests\":[],\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "thread.visit-recorded": {
    "threads": [
      {
        "thread_id": "0",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+119264-02-25T06:19:34.002Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"0\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+119264-02-25T06:19:34.002Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":\"1969-12-31T23:59:59.991Z\",\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "run.created": {
    "threads": [
      {
        "thread_id": "B",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+051689-02-27T09:57:11.653Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"B\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+051689-02-27T09:57:11.653Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [
      {
        "run_id": "@",
        "thread_id": "W",
        "ordinal": 1,
        "provider": "D",
        "provider_thread_id": "%",
        "status": "starting",
        "requested_at": "-271595-09-07T02:19:10.232Z",
        "completed_at": null,
        "payload_json": "{\"id\":\"@\",\"threadId\":\"W\",\"ordinal\":1,\"providerInstanceId\":\"D\",\"modelSelection\":{\"instanceId\":\"m\",\"model\":\"0\"},\"providerThreadId\":\"%\",\"userMessageId\":\"4\",\"rootNodeId\":null,\"activeAttemptId\":null,\"status\":\"starting\",\"requestedAt\":\"-271595-09-07T02:19:10.232Z\",\"startedAt\":null,\"completedAt\":null,\"checkpointId\":null,\"contextHandoffId\":\"8\"}",
        "provider_instance_id": "D"
      }
    ],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "run.updated": {
    "threads": [
      {
        "thread_id": "B",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "+051689-02-27T09:57:11.653Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"B\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"+051689-02-27T09:57:11.653Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [
      {
        "run_id": "@",
        "thread_id": "W",
        "ordinal": 1,
        "provider": "D",
        "provider_thread_id": "%",
        "status": "starting",
        "requested_at": "-271595-09-07T02:19:10.232Z",
        "completed_at": null,
        "payload_json": "{\"id\":\"@\",\"threadId\":\"W\",\"ordinal\":1,\"providerInstanceId\":\"D\",\"modelSelection\":{\"instanceId\":\"m\",\"model\":\"0\"},\"providerThreadId\":\"%\",\"userMessageId\":\"4\",\"rootNodeId\":null,\"activeAttemptId\":null,\"status\":\"starting\",\"requestedAt\":\"-271595-09-07T02:19:10.232Z\",\"startedAt\":null,\"completedAt\":null,\"checkpointId\":null,\"contextHandoffId\":\"8\"}",
        "provider_instance_id": "D"
      }
    ],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "run.background-work-cancelled": {
    "threads": [
      {
        "thread_id": "w",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "1970-01-01T00:00:00.041Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"w\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"1970-01-01T00:00:00.041Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "run-attempt.created": {
    "threads": [
      {
        "thread_id": "w",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "1970-01-01T00:00:00.041Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"w\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"1970-01-01T00:00:00.041Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [
      {
        "attempt_id": "G",
        "thread_id": "w",
        "run_id": "�",
        "attempt_ordinal": 2,
        "root_node_id": "x",
        "provider": "P",
        "provider_thread_id": "�",
        "provider_turn_id": "{",
        "status": "running",
        "payload_json": "{\"id\":\"G\",\"runId\":\"�\",\"attemptOrdinal\":2,\"rootNodeId\":\"x\",\"providerInstanceId\":\"P\",\"providerThreadId\":\"�\",\"providerTurnId\":\"{\",\"reason\":\"retry\",\"status\":\"running\",\"startedAt\":null,\"completedAt\":\"1970-01-01T00:00:00.048Z\"}",
        "provider_instance_id": "P"
      }
    ],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "run-attempt.updated": {
    "threads": [
      {
        "thread_id": "w",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "1970-01-01T00:00:00.041Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"w\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"1970-01-01T00:00:00.041Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [
      {
        "attempt_id": "G",
        "thread_id": "w",
        "run_id": "�",
        "attempt_ordinal": 2,
        "root_node_id": "x",
        "provider": "P",
        "provider_thread_id": "�",
        "provider_turn_id": "{",
        "status": "running",
        "payload_json": "{\"id\":\"G\",\"runId\":\"�\",\"attemptOrdinal\":2,\"rootNodeId\":\"x\",\"providerInstanceId\":\"P\",\"providerThreadId\":\"�\",\"providerTurnId\":\"{\",\"reason\":\"retry\",\"status\":\"running\",\"startedAt\":null,\"completedAt\":\"1970-01-01T00:00:00.048Z\"}",
        "provider_instance_id": "P"
      }
    ],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "node.updated": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "-063919-09-28T13:30:59.155Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"-063919-09-28T13:30:59.155Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [
      {
        "node_id": "V",
        "thread_id": "R",
        "run_id": null,
        "parent_node_id": null,
        "root_node_id": "*",
        "kind": "system",
        "status": "interrupted",
        "provider_thread_id": "4",
        "provider_turn_id": null,
        "runtime_request_id": null,
        "checkpoint_scope_id": null,
        "started_at": null,
        "completed_at": "+016487-08-28T22:29:02.210Z",
        "payload_json": "{\"id\":\"V\",\"threadId\":\"R\",\"runId\":null,\"parentNodeId\":null,\"rootNodeId\":\"*\",\"kind\":\"system\",\"status\":\"interrupted\",\"countsForRun\":false,\"providerThreadId\":\"4\",\"providerTurnId\":null,\"nativeItemRef\":null,\"runtimeRequestId\":\")\",\"checkpointScopeId\":\"5\",\"startedAt\":null,\"completedAt\":\"+016487-08-28T22:29:02.210Z\"}"
      }
    ],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "subagent.updated": {
    "threads": [
      {
        "thread_id": "\u0000",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "3715-01-23T20:54:33.620Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"\\u0000\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"3715-01-23T20:54:33.620Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [
      {
        "subagent_id": "9",
        "thread_id": "E",
        "run_id": "\u0000",
        "parent_node_id": "0",
        "provider": "i",
        "provider_thread_id": "M",
        "child_thread_id": null,
        "origin": "provider_native",
        "status": "running",
        "started_at": null,
        "completed_at": null,
        "updated_at": "-004289-12-04T14:53:32.586Z",
        "payload_json": "{\"id\":\"9\",\"threadId\":\"E\",\"runId\":\"\\u0000\",\"parentNodeId\":\"0\",\"origin\":\"provider_native\",\"createdBy\":\"agent\",\"driver\":\"w\",\"providerInstanceId\":\"i\",\"providerThreadId\":\"M\",\"childThreadId\":null,\"nativeTaskRef\":null,\"prompt\":\"\",\"title\":null,\"model\":\"\",\"status\":\"running\",\"result\":null,\"startedAt\":null,\"completedAt\":null,\"updatedAt\":\"-004289-12-04T14:53:32.586Z\"}",
        "driver": "w",
        "provider_instance_id": "i",
        "completion_delivery_state": null
      }
    ],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "provider-session.attached": {
    "threads": [
      {
        "thread_id": "w",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "1970-01-01T00:00:00.041Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"w\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"1970-01-01T00:00:00.041Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [
      {
        "provider_session_id": "G",
        "thread_id": "w",
        "provider": "w",
        "status": "waiting",
        "model": null,
        "updated_at": "+077087-10-14T23:29:12.065Z",
        "payload_json": "{\"id\":\"G\",\"driver\":\"y\",\"providerInstanceId\":\"w\",\"status\":\"waiting\",\"cwd\":\"I\",\"model\":\"�\",\"capabilities\":{\"sessions\":{\"supportsMultipleProviderThreadsPerSession\":false,\"supportsModelSwitchInSession\":true,\"supportsProviderSwitchingViaHandoff\":true,\"supportsRuntimeModeSwitchInSession\":true,\"pendingRequestsSurviveRestart\":true},\"threads\":{\"canCreateEmptyThread\":true,\"canReadThreadSnapshot\":true,\"canRollbackThread\":true,\"canForkThread\":true,\"canForkFromTurn\":false,\"canForkFromSubagentThread\":true,\"exposesNativeThreadId\":true},\"turns\":{\"exposesNativeTurnId\":false,\"emitsTurnStarted\":false,\"emitsTurnCompleted\":false,\"supportsInterrupt\":false,\"supportsActiveSteering\":false,\"supportsSteeringByInterruptRestart\":false,\"supportsQueuedMessages\":false,\"terminalStatusQuality\":\"strong\"},\"streaming\":{\"streamsAssistantText\":true,\"streamsReasoning\":true,\"streamsToolOutput\":false,\"streamsPlanText\":false,\"emitsMessageCompleted\":false},\"tools\":{\"exposesToolItemIds\":false,\"emitsToolStarted\":false,\"emitsToolCompleted\":true,\"emitsToolOutput\":true,\"supportsMcpTools\":true,\"supportsDynamicToolCallbacks\":false},\"approvals\":{\"supportsCommandApproval\":true,\"supportsFileReadApproval\":true,\"supportsFileChangeApproval\":false,\"supportsApplyPatchApproval\":false,\"approvalsHaveNativeRequestIds\":false,\"approvalCallbacksAreLiveOnly\":false,\"approvalsCanOriginateFromSubagents\":true},\"planning\":{\"emitsPlanUpdated\":true,\"emitsTodoList\":true,\"emitsProposedPlan\":true,\"supportsStructuredQuestions\":false,\"planDeltasHaveItemIds\":true},\"subagents\":{\"supportsSubagents\":false,\"exposesSubagentThreadIds\":true,\"emitsSubagentLifecycle\":true,\"canWaitForSubagents\":true,\"canCloseSubagents\":false,\"canForkSubagentThread\":false},\"context\":{\"acceptsSystemContext\":false,\"acceptsDeveloperContext\":true,\"acceptsSyntheticUserContext\":false,\"canGenerateSummaries\":true,\"canConsumeHandoffSummaries\":false,\"supportsDeltaHandoff\":true,\"supportsFullThreadHandoff\":true,\"maxRecommendedHandoffChars\":null},\"checkpointing\":{\"appCanCheckpointFilesystem\":true,\"supportsNestedCheckpointScopes\":true,\"providerCanRollbackConversation\":false,\"providerRollbackReturnsSnapshot\":false,\"providerCanReadConversationSnapshot\":false},\"identity\":{\"nativeThreadIds\":\"none\",\"nativeTurnIds\":\"weak\",\"nativeItemIds\":\"strong\",\"nativeRequestIds\":\"none\"},\"runtimePolicy\":{\"enforcement\":\"native\"}},\"extensionStatuses\":[],\"createdAt\":\"-225427-06-13T07:15:27.044Z\",\"updatedAt\":\"+077087-10-14T23:29:12.065Z\",\"lastError\":\"\"}",
        "driver": "y",
        "provider_instance_id": "w"
      }
    ],
    "provider_session_bindings": [
      {
        "provider_session_id": "G",
        "thread_id": "w"
      }
    ],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "provider-session.updated": {
    "threads": [
      {
        "thread_id": "w",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "1970-01-01T00:00:00.041Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"w\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"1970-01-01T00:00:00.041Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [
      {
        "provider_session_id": "G",
        "thread_id": "w",
        "provider": "w",
        "status": "waiting",
        "model": null,
        "updated_at": "+077087-10-14T23:29:12.065Z",
        "payload_json": "{\"id\":\"G\",\"driver\":\"y\",\"providerInstanceId\":\"w\",\"status\":\"waiting\",\"cwd\":\"I\",\"model\":\"�\",\"capabilities\":{\"sessions\":{\"supportsMultipleProviderThreadsPerSession\":false,\"supportsModelSwitchInSession\":true,\"supportsProviderSwitchingViaHandoff\":true,\"supportsRuntimeModeSwitchInSession\":true,\"pendingRequestsSurviveRestart\":true},\"threads\":{\"canCreateEmptyThread\":true,\"canReadThreadSnapshot\":true,\"canRollbackThread\":true,\"canForkThread\":true,\"canForkFromTurn\":false,\"canForkFromSubagentThread\":true,\"exposesNativeThreadId\":true},\"turns\":{\"exposesNativeTurnId\":false,\"emitsTurnStarted\":false,\"emitsTurnCompleted\":false,\"supportsInterrupt\":false,\"supportsActiveSteering\":false,\"supportsSteeringByInterruptRestart\":false,\"supportsQueuedMessages\":false,\"terminalStatusQuality\":\"strong\"},\"streaming\":{\"streamsAssistantText\":true,\"streamsReasoning\":true,\"streamsToolOutput\":false,\"streamsPlanText\":false,\"emitsMessageCompleted\":false},\"tools\":{\"exposesToolItemIds\":false,\"emitsToolStarted\":false,\"emitsToolCompleted\":true,\"emitsToolOutput\":true,\"supportsMcpTools\":true,\"supportsDynamicToolCallbacks\":false},\"approvals\":{\"supportsCommandApproval\":true,\"supportsFileReadApproval\":true,\"supportsFileChangeApproval\":false,\"supportsApplyPatchApproval\":false,\"approvalsHaveNativeRequestIds\":false,\"approvalCallbacksAreLiveOnly\":false,\"approvalsCanOriginateFromSubagents\":true},\"planning\":{\"emitsPlanUpdated\":true,\"emitsTodoList\":true,\"emitsProposedPlan\":true,\"supportsStructuredQuestions\":false,\"planDeltasHaveItemIds\":true},\"subagents\":{\"supportsSubagents\":false,\"exposesSubagentThreadIds\":true,\"emitsSubagentLifecycle\":true,\"canWaitForSubagents\":true,\"canCloseSubagents\":false,\"canForkSubagentThread\":false},\"context\":{\"acceptsSystemContext\":false,\"acceptsDeveloperContext\":true,\"acceptsSyntheticUserContext\":false,\"canGenerateSummaries\":true,\"canConsumeHandoffSummaries\":false,\"supportsDeltaHandoff\":true,\"supportsFullThreadHandoff\":true,\"maxRecommendedHandoffChars\":null},\"checkpointing\":{\"appCanCheckpointFilesystem\":true,\"supportsNestedCheckpointScopes\":true,\"providerCanRollbackConversation\":false,\"providerRollbackReturnsSnapshot\":false,\"providerCanReadConversationSnapshot\":false},\"identity\":{\"nativeThreadIds\":\"none\",\"nativeTurnIds\":\"weak\",\"nativeItemIds\":\"strong\",\"nativeRequestIds\":\"none\"},\"runtimePolicy\":{\"enforcement\":\"native\"}},\"extensionStatuses\":[],\"createdAt\":\"-225427-06-13T07:15:27.044Z\",\"updatedAt\":\"+077087-10-14T23:29:12.065Z\",\"lastError\":\"\"}",
        "driver": "y",
        "provider_instance_id": "w"
      }
    ],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "provider-session.detached": {
    "threads": [
      {
        "thread_id": "w",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "1970-01-01T00:00:00.041Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"w\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"1970-01-01T00:00:00.041Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "provider-thread.updated": {
    "threads": [
      {
        "thread_id": "\u0000",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "3715-01-23T20:54:33.620Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"\\u0000\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"3715-01-23T20:54:33.620Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [
      {
        "provider_thread_id": "9",
        "thread_id": null,
        "owner_node_id": "0",
        "provider": "C",
        "provider_session_id": null,
        "status": "not_loaded",
        "first_run_ordinal": null,
        "last_run_ordinal": 1,
        "updated_at": "1969-12-31T23:59:59.982Z",
        "payload_json": "{\"id\":\"9\",\"driver\":\"U\",\"providerInstanceId\":\"C\",\"providerSessionId\":null,\"appThreadId\":null,\"ownerNodeId\":\"0\",\"nativeThreadRef\":null,\"nativeConversationHeadRef\":null,\"status\":\"not_loaded\",\"firstRunOrdinal\":null,\"lastRunOrdinal\":1,\"handoffIds\":[],\"forkedFrom\":null,\"pendingBackgroundTasks\":[],\"contextUsage\":null,\"nativeMetadata\":null,\"goal\":null,\"createdAt\":\"+075099-09-16T06:29:17.744Z\",\"updatedAt\":\"1969-12-31T23:59:59.982Z\"}",
        "driver": "U",
        "provider_instance_id": "C"
      }
    ],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "provider-turn.updated": {
    "threads": [
      {
        "thread_id": "w",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "1970-01-01T00:00:00.041Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"w\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"1970-01-01T00:00:00.041Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [
      {
        "provider_turn_id": "G",
        "thread_id": "w",
        "provider_thread_id": "�",
        "node_id": "�",
        "run_attempt_id": null,
        "ordinal": 1,
        "status": "running",
        "started_at": null,
        "completed_at": "-115683-01-26T00:02:27.351Z",
        "payload_json": "{\"id\":\"G\",\"providerThreadId\":\"�\",\"nodeId\":\"�\",\"runAttemptId\":null,\"nativeTurnRef\":null,\"ordinal\":1,\"status\":\"running\",\"startedAt\":null,\"completedAt\":\"-115683-01-26T00:02:27.351Z\"}"
      }
    ],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "runtime-request.updated": {
    "threads": [
      {
        "thread_id": "w",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "1970-01-01T00:00:00.041Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"w\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"1970-01-01T00:00:00.041Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [
      {
        "runtime_request_id": "G",
        "thread_id": "w",
        "node_id": "�",
        "provider_turn_id": "_",
        "kind": "file-read",
        "status": "pending",
        "created_at": "-115683-01-26T00:02:27.351Z",
        "resolved_at": "1970-01-01T00:00:00.048Z",
        "payload_json": "{\"id\":\"G\",\"nodeId\":\"�\",\"providerTurnId\":\"_\",\"nativeRequestRef\":null,\"kind\":\"file-read\",\"status\":\"pending\",\"responseCapability\":{\"type\":\"not_resumable\",\"reason\":\"\"},\"createdAt\":\"-115683-01-26T00:02:27.351Z\",\"resolvedAt\":\"1970-01-01T00:00:00.048Z\"}"
      }
    ],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "message.updated": {
    "threads": [
      {
        "thread_id": "w",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "1970-01-01T00:00:00.041Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"w\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"1970-01-01T00:00:00.041Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [
      {
        "message_id": "�",
        "thread_id": "�",
        "run_id": null,
        "node_id": null,
        "role": "assistant",
        "streaming": 1,
        "created_at": "-094730-09-19T06:57:00.439Z",
        "updated_at": "+136736-08-06T05:20:37.427Z",
        "payload_json": "{\"createdBy\":\"agent\",\"creationSource\":\"mcp\",\"id\":\"�\",\"threadId\":\"�\",\"runId\":null,\"nodeId\":null,\"role\":\"assistant\",\"text\":\"\",\"attachments\":[],\"streaming\":true,\"createdAt\":\"-094730-09-19T06:57:00.439Z\",\"updatedAt\":\"+136736-08-06T05:20:37.427Z\"}",
        "search_text": null
      }
    ],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "turn-item.updated": {
    "threads": [
      {
        "thread_id": "G",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "-063919-09-28T13:30:59.155Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"G\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"-063919-09-28T13:30:59.155Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [
      {
        "turn_item_id": "S",
        "thread_id": "n",
        "run_id": null,
        "node_id": null,
        "provider_thread_id": "W",
        "provider_turn_id": null,
        "parent_item_id": ")",
        "ordinal": 0,
        "type": "secret_request",
        "status": "pending",
        "updated_at": "-077756-04-10T21:22:20.750Z",
        "payload_json": "{\"id\":\"S\",\"threadId\":\"n\",\"runId\":null,\"nodeId\":null,\"providerThreadId\":\"W\",\"providerTurnId\":null,\"nativeItemRef\":{\"driver\":\"L\",\"nativeId\":null,\"strength\":\"weak\"},\"parentItemId\":\")\",\"ordinal\":0,\"status\":\"pending\",\"title\":null,\"startedAt\":\"+016487-08-28T22:29:02.210Z\",\"completedAt\":\"+034892-12-01T14:16:37.543Z\",\"updatedAt\":\"-077756-04-10T21:22:20.750Z\",\"type\":\"secret_request\",\"label\":\"J\",\"reason\":\"\",\"secretStatus\":\"cancelled\"}"
      }
    ],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": [
      {
        "thread_id": "n",
        "run_key": "",
        "item_count": 1
      }
    ]
  },
  "plan.updated": {
    "threads": [
      {
        "thread_id": "�",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "-271821-04-20T00:00:00.019Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"�\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"-271821-04-20T00:00:00.019Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [
      {
        "plan_id": "�",
        "thread_id": "\"",
        "run_id": "\u0000",
        "node_id": "t",
        "kind": "proposed_plan",
        "status": "active",
        "payload_json": "{\"id\":\"�\",\"threadId\":\"\\\"\",\"runId\":\"\\u0000\",\"nodeId\":\"t\",\"status\":\"active\",\"kind\":\"proposed_plan\",\"markdown\":\"\"}"
      }
    ],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "checkpoint-scope.created": {
    "threads": [
      {
        "thread_id": "w",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "1970-01-01T00:00:00.041Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"w\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"1970-01-01T00:00:00.041Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [
      {
        "scope_id": "G",
        "thread_id": "�",
        "run_id": "_",
        "node_id": "I",
        "parent_scope_id": null,
        "provider_thread_id": "{",
        "kind": "provider_thread",
        "ordinal_within_parent": 0,
        "advances_app_run_count": 1,
        "created_at": "-063367-09-13T13:43:17.971Z",
        "payload_json": "{\"id\":\"G\",\"threadId\":\"�\",\"runId\":\"_\",\"nodeId\":\"I\",\"parentScopeId\":\"�\",\"providerThreadId\":\"{\",\"kind\":\"provider_thread\",\"ordinalWithinParent\":0,\"advancesAppRunCount\":true,\"cwd\":\"\\u0000\",\"createdAt\":\"-063367-09-13T13:43:17.971Z\"}"
      }
    ],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "checkpoint.captured": {
    "threads": [
      {
        "thread_id": "w",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "1970-01-01T00:00:00.041Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"w\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"1970-01-01T00:00:00.041Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [
      {
        "checkpoint_id": "G",
        "thread_id": "�",
        "scope_id": "�",
        "run_id": null,
        "node_id": "I",
        "parent_checkpoint_id": null,
        "ordinal_within_scope": 1,
        "app_run_ordinal": null,
        "status": "error",
        "captured_at": "1969-12-31T23:59:59.999Z",
        "payload_json": "{\"id\":\"G\",\"threadId\":\"�\",\"scopeId\":\"�\",\"runId\":null,\"nodeId\":\"I\",\"parentCheckpointId\":\"�\",\"ordinalWithinScope\":1,\"appRunOrdinal\":null,\"ref\":\"9\",\"status\":\"error\",\"files\":[],\"capturedAt\":\"1969-12-31T23:59:59.999Z\"}"
      }
    ],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "checkpoint.rollback-requested": {
    "threads": [
      {
        "thread_id": "w",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "1970-01-01T00:00:00.041Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"w\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"1970-01-01T00:00:00.041Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "context-handoff.updated": {
    "threads": [
      {
        "thread_id": "w",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "1970-01-01T00:00:00.041Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"w\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"1970-01-01T00:00:00.041Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [
      {
        "context_handoff_id": "G",
        "thread_id": "�",
        "target_run_id": "�",
        "to_provider_thread_id": "x",
        "strategy": "fork_delta_summary",
        "status": "superseded",
        "updated_at": "1969-12-31T23:59:59.971Z",
        "payload_json": "{\"id\":\"G\",\"threadId\":\"�\",\"targetRunId\":\"�\",\"fromProviderThreadIds\":[],\"toProviderThreadId\":\"x\",\"coveredRunOrdinals\":{\"from\":1,\"to\":2},\"strategy\":\"fork_delta_summary\",\"status\":\"superseded\",\"summaryMessageId\":null,\"summaryText\":\"\",\"createdByProviderInstanceId\":\"I\",\"createdAt\":\"1970-01-01T00:00:00.048Z\",\"updatedAt\":\"1969-12-31T23:59:59.971Z\"}"
      }
    ],
    "context_transfers": [],
    "turn_item_counts": []
  },
  "context-transfer.created": {
    "threads": [
      {
        "thread_id": "\u0000",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "1969-12-31T23:59:59.984Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"\\u0000\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"1969-12-31T23:59:59.984Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [
      {
        "context_transfer_id": "\u0000",
        "source_thread_id": "b",
        "target_thread_id": "�",
        "target_run_id": null,
        "type": "subagent_spawn",
        "status": "resolved_portable",
        "source_provider": null,
        "target_provider": null,
        "updated_at": "-271821-04-20T00:00:00.036Z",
        "payload_json": "{\"id\":\"\\u0000\",\"type\":\"subagent_spawn\",\"sourceThreadId\":\"b\",\"targetThreadId\":\"�\",\"sourcePoint\":{\"threadId\":\"�\"},\"basePoint\":{\"threadId\":\"5\"},\"sourceProviderInstanceId\":\"O\",\"targetProviderInstanceId\":\"s\",\"targetRunId\":null,\"status\":\"resolved_portable\",\"resolution\":{\"strategy\":\"checkpoint_context\",\"contextHandoffId\":\"'\"},\"createdBy\":\"system\",\"error\":null,\"createdAt\":\"+139437-04-08T12:56:15.012Z\",\"updatedAt\":\"-271821-04-20T00:00:00.036Z\",\"consumedAt\":null}",
        "source_provider_instance_id": null,
        "target_provider_instance_id": null
      }
    ],
    "turn_item_counts": []
  },
  "context-transfer.updated": {
    "threads": [
      {
        "thread_id": "\u0000",
        "project_id": "g",
        "title": "*",
        "default_provider": "e",
        "runtime_mode": "auto",
        "interaction_mode": "plan",
        "active_provider_thread_id": null,
        "created_at": "-015330-10-12T21:32:11.979Z",
        "updated_at": "1969-12-31T23:59:59.984Z",
        "archived_at": null,
        "deleted_at": null,
        "payload_json": "{\"createdBy\":\"system\",\"creationSource\":\"mcp\",\"id\":\"\\u0000\",\"projectId\":\"g\",\"title\":\"*\",\"providerInstanceId\":\"e\",\"modelSelection\":{\"instanceId\":\"E\",\"model\":\"4\"},\"runtimeMode\":\"auto\",\"interactionMode\":\"plan\",\"branch\":\")\",\"worktreePath\":\"5\",\"activeProviderThreadId\":null,\"lineage\":{\"parentThreadId\":\"O\",\"relationshipToParent\":\"fork\",\"rootThreadId\":\"S\"},\"forkedFrom\":{\"type\":\"provider_thread\",\"providerThreadId\":\"y\"},\"createdAt\":\"-015330-10-12T21:32:11.979Z\",\"updatedAt\":\"1969-12-31T23:59:59.984Z\",\"archivedAt\":null,\"settledOverride\":\"settled\",\"settledAt\":\"+275760-09-12T23:59:59.988Z\",\"lastVisitedAt\":null,\"deletedAt\":null}",
        "provider_instance_id": "e"
      }
    ],
    "runs": [],
    "run_attempts": [],
    "nodes": [],
    "subagents": [],
    "provider_sessions": [],
    "provider_session_bindings": [],
    "provider_threads": [],
    "provider_turns": [],
    "runtime_requests": [],
    "messages": [],
    "plans": [],
    "turn_items": [],
    "checkpoint_scopes": [],
    "checkpoints": [],
    "context_handoffs": [],
    "context_transfers": [
      {
        "context_transfer_id": "\u0000",
        "source_thread_id": "b",
        "target_thread_id": "�",
        "target_run_id": null,
        "type": "subagent_spawn",
        "status": "resolved_portable",
        "source_provider": null,
        "target_provider": null,
        "updated_at": "-271821-04-20T00:00:00.036Z",
        "payload_json": "{\"id\":\"\\u0000\",\"type\":\"subagent_spawn\",\"sourceThreadId\":\"b\",\"targetThreadId\":\"�\",\"sourcePoint\":{\"threadId\":\"�\"},\"basePoint\":{\"threadId\":\"5\"},\"sourceProviderInstanceId\":\"O\",\"targetProviderInstanceId\":\"s\",\"targetRunId\":null,\"status\":\"resolved_portable\",\"resolution\":{\"strategy\":\"checkpoint_context\",\"contextHandoffId\":\"'\"},\"createdBy\":\"system\",\"error\":null,\"createdAt\":\"+139437-04-08T12:56:15.012Z\",\"updatedAt\":\"-271821-04-20T00:00:00.036Z\",\"consumedAt\":null}",
        "source_provider_instance_id": null,
        "target_provider_instance_id": null
      }
    ],
    "turn_item_counts": []
  }
}"###;

const TYPESCRIPT_TURN_ITEM_PAYLOADS: &str = r###"[
  {
    "event": {
      "id": "compat:150",
      "threadId": "compat:thread:0",
      "occurredAt": "-063919-09-28T13:30:59.155Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "S",
        "threadId": "n",
        "runId": null,
        "nodeId": null,
        "providerThreadId": "W",
        "providerTurnId": null,
        "nativeItemRef": {
          "driver": "L",
          "nativeId": null,
          "strength": "weak"
        },
        "parentItemId": ")",
        "ordinal": 0,
        "status": "pending",
        "title": null,
        "startedAt": "+016487-08-28T22:29:02.210Z",
        "completedAt": "+034892-12-01T14:16:37.543Z",
        "updatedAt": "-077756-04-10T21:22:20.750Z",
        "type": "secret_request",
        "label": "J",
        "reason": "",
        "secretStatus": "cancelled"
      }
    },
    "payload_json": "{\"id\":\"S\",\"threadId\":\"n\",\"runId\":null,\"nodeId\":null,\"providerThreadId\":\"W\",\"providerTurnId\":null,\"nativeItemRef\":{\"driver\":\"L\",\"nativeId\":null,\"strength\":\"weak\"},\"parentItemId\":\")\",\"ordinal\":0,\"status\":\"pending\",\"title\":null,\"startedAt\":\"+016487-08-28T22:29:02.210Z\",\"completedAt\":\"+034892-12-01T14:16:37.543Z\",\"updatedAt\":\"-077756-04-10T21:22:20.750Z\",\"type\":\"secret_request\",\"label\":\"J\",\"reason\":\"\",\"secretStatus\":\"cancelled\"}"
  },
  {
    "event": {
      "id": "compat:1040",
      "threadId": "compat:thread:2",
      "occurredAt": "1970-01-01T00:00:00.029Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "Q",
        "threadId": "�",
        "runId": null,
        "nodeId": "z",
        "providerThreadId": "I",
        "providerTurnId": null,
        "nativeItemRef": null,
        "parentItemId": "<",
        "ordinal": 1,
        "status": "pending",
        "title": "",
        "startedAt": "+093273-07-17T18:42:12.174Z",
        "completedAt": null,
        "updatedAt": "1969-12-31T23:59:59.970Z",
        "type": "system_notice",
        "message": ""
      }
    },
    "payload_json": "{\"id\":\"Q\",\"threadId\":\"�\",\"runId\":null,\"nodeId\":\"z\",\"providerThreadId\":\"I\",\"providerTurnId\":null,\"nativeItemRef\":null,\"parentItemId\":\"<\",\"ordinal\":1,\"status\":\"pending\",\"title\":\"\",\"startedAt\":\"+093273-07-17T18:42:12.174Z\",\"completedAt\":null,\"updatedAt\":\"1969-12-31T23:59:59.970Z\",\"type\":\"system_notice\",\"message\":\"\"}"
  },
  {
    "event": {
      "id": "compat:1041",
      "threadId": "compat:thread:0",
      "occurredAt": "+101376-07-17T22:26:56.486Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "[",
        "threadId": "/",
        "runId": null,
        "nodeId": null,
        "providerThreadId": "�",
        "providerTurnId": null,
        "nativeItemRef": null,
        "parentItemId": null,
        "ordinal": 1,
        "status": "completed",
        "title": null,
        "startedAt": "+199900-04-16T06:56:57.787Z",
        "completedAt": "+275760-09-12T23:59:59.956Z",
        "updatedAt": "1969-12-31T23:59:59.964Z",
        "type": "file_search"
      }
    },
    "payload_json": "{\"id\":\"[\",\"threadId\":\"/\",\"runId\":null,\"nodeId\":null,\"providerThreadId\":\"�\",\"providerTurnId\":null,\"nativeItemRef\":null,\"parentItemId\":null,\"ordinal\":1,\"status\":\"completed\",\"title\":null,\"startedAt\":\"+199900-04-16T06:56:57.787Z\",\"completedAt\":\"+275760-09-12T23:59:59.956Z\",\"updatedAt\":\"1969-12-31T23:59:59.964Z\",\"type\":\"file_search\"}"
  },
  {
    "event": {
      "id": "compat:1042",
      "threadId": "compat:thread:1",
      "occurredAt": "+098151-06-25T14:53:34.533Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "?",
        "threadId": "0",
        "runId": null,
        "nodeId": "4",
        "providerThreadId": null,
        "providerTurnId": null,
        "nativeItemRef": {
          "driver": "N",
          "nativeId": "C",
          "strength": "weak"
        },
        "parentItemId": null,
        "ordinal": 1,
        "status": "completed",
        "title": "",
        "startedAt": null,
        "completedAt": "+134002-03-22T16:28:16.949Z",
        "updatedAt": "-033789-08-11T12:52:24.073Z",
        "type": "subagent",
        "subagentId": "p",
        "origin": "provider_native",
        "driver": "c",
        "providerInstanceId": "b",
        "childThreadId": null,
        "prompt": "",
        "result": ""
      }
    },
    "payload_json": "{\"id\":\"?\",\"threadId\":\"0\",\"runId\":null,\"nodeId\":\"4\",\"providerThreadId\":null,\"providerTurnId\":null,\"nativeItemRef\":{\"driver\":\"N\",\"nativeId\":\"C\",\"strength\":\"weak\"},\"parentItemId\":null,\"ordinal\":1,\"status\":\"completed\",\"title\":\"\",\"startedAt\":null,\"completedAt\":\"+134002-03-22T16:28:16.949Z\",\"updatedAt\":\"-033789-08-11T12:52:24.073Z\",\"type\":\"subagent\",\"subagentId\":\"p\",\"origin\":\"provider_native\",\"driver\":\"c\",\"providerInstanceId\":\"b\",\"childThreadId\":null,\"prompt\":\"\",\"result\":\"\"}"
  },
  {
    "event": {
      "id": "compat:1043",
      "threadId": "compat:thread:2",
      "occurredAt": "+159705-12-24T10:22:19.261Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "a",
        "threadId": "0",
        "runId": null,
        "nodeId": "P",
        "providerThreadId": "O0",
        "providerTurnId": "o@",
        "nativeItemRef": null,
        "parentItemId": null,
        "ordinal": 0,
        "status": "cancelled",
        "title": null,
        "startedAt": null,
        "completedAt": null,
        "updatedAt": "1969-12-31T23:59:59.986Z",
        "type": "dynamic_tool",
        "toolName": "k+KS",
        "input": {
          "{fI": true,
          "": "Ho8o",
          "4VUt": [],
          "😀": {}
        }
      }
    },
    "payload_json": "{\"id\":\"a\",\"threadId\":\"0\",\"runId\":null,\"nodeId\":\"P\",\"providerThreadId\":\"O0\",\"providerTurnId\":\"o@\",\"nativeItemRef\":null,\"parentItemId\":null,\"ordinal\":0,\"status\":\"cancelled\",\"title\":null,\"startedAt\":null,\"completedAt\":null,\"updatedAt\":\"1969-12-31T23:59:59.986Z\",\"type\":\"dynamic_tool\",\"toolName\":\"k+KS\",\"input\":{\"{fI\":true,\"\":\"Ho8o\",\"4VUt\":[],\"😀\":{}}}"
  },
  {
    "event": {
      "id": "compat:1044",
      "threadId": "compat:thread:0",
      "occurredAt": "-142678-01-12T01:21:34.921Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "6",
        "threadId": "0",
        "runId": "s",
        "nodeId": "0",
        "providerThreadId": "�",
        "providerTurnId": "%",
        "nativeItemRef": {
          "driver": "BKBf",
          "nativeId": "😀",
          "strength": "strong"
        },
        "parentItemId": "L",
        "ordinal": 0,
        "status": "pending",
        "title": "@?",
        "startedAt": "1970-01-01T00:00:00.030Z",
        "completedAt": "-271821-04-20T00:00:00.001Z",
        "updatedAt": "-137145-12-28T07:05:21.038Z",
        "type": "reasoning",
        "text": "}",
        "streaming": false
      }
    },
    "payload_json": "{\"id\":\"6\",\"threadId\":\"0\",\"runId\":\"s\",\"nodeId\":\"0\",\"providerThreadId\":\"�\",\"providerTurnId\":\"%\",\"nativeItemRef\":{\"driver\":\"BKBf\",\"nativeId\":\"😀\",\"strength\":\"strong\"},\"parentItemId\":\"L\",\"ordinal\":0,\"status\":\"pending\",\"title\":\"@?\",\"startedAt\":\"1970-01-01T00:00:00.030Z\",\"completedAt\":\"-271821-04-20T00:00:00.001Z\",\"updatedAt\":\"-137145-12-28T07:05:21.038Z\",\"type\":\"reasoning\",\"text\":\"}\",\"streaming\":false}"
  },
  {
    "event": {
      "id": "compat:1046",
      "threadId": "compat:thread:2",
      "occurredAt": "+220895-08-17T10:36:51.193Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "😀",
        "threadId": ">9/C",
        "runId": "d",
        "nodeId": "VUzP",
        "providerThreadId": null,
        "providerTurnId": "d",
        "nativeItemRef": null,
        "parentItemId": null,
        "ordinal": 10,
        "status": "failed",
        "title": " ",
        "startedAt": null,
        "completedAt": null,
        "updatedAt": "-071990-02-20T14:49:50.932Z",
        "type": "todo_list",
        "planId": "0",
        "steps": []
      }
    },
    "payload_json": "{\"id\":\"😀\",\"threadId\":\">9/C\",\"runId\":\"d\",\"nodeId\":\"VUzP\",\"providerThreadId\":null,\"providerTurnId\":\"d\",\"nativeItemRef\":null,\"parentItemId\":null,\"ordinal\":10,\"status\":\"failed\",\"title\":\" \",\"startedAt\":null,\"completedAt\":null,\"updatedAt\":\"-071990-02-20T14:49:50.932Z\",\"type\":\"todo_list\",\"planId\":\"0\",\"steps\":[]}"
  },
  {
    "event": {
      "id": "compat:1060",
      "threadId": "compat:thread:1",
      "occurredAt": "2026-09-03T00:00:00.000Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "T",
        "threadId": "7",
        "runId": null,
        "nodeId": null,
        "providerThreadId": null,
        "providerTurnId": "�",
        "nativeItemRef": {
          "driver": "i",
          "nativeId": null,
          "strength": "weak"
        },
        "parentItemId": "�",
        "ordinal": 1,
        "status": "interrupted",
        "title": null,
        "startedAt": "-178053-07-29T23:10:07.667Z",
        "completedAt": "-063367-09-13T13:43:17.971Z",
        "updatedAt": "1969-12-31T23:59:59.963Z",
        "type": "approval_request",
        "requestId": "0",
        "requestKind": "permission"
      }
    },
    "payload_json": "{\"id\":\"T\",\"threadId\":\"7\",\"runId\":null,\"nodeId\":null,\"providerThreadId\":null,\"providerTurnId\":\"�\",\"nativeItemRef\":{\"driver\":\"i\",\"nativeId\":null,\"strength\":\"weak\"},\"parentItemId\":\"�\",\"ordinal\":1,\"status\":\"interrupted\",\"title\":null,\"startedAt\":\"-178053-07-29T23:10:07.667Z\",\"completedAt\":\"-063367-09-13T13:43:17.971Z\",\"updatedAt\":\"1969-12-31T23:59:59.963Z\",\"type\":\"approval_request\",\"requestId\":\"0\",\"requestKind\":\"permission\"}"
  },
  {
    "event": {
      "id": "compat:1062",
      "threadId": "compat:thread:0",
      "occurredAt": "2026-09-03T00:00:00.000Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "N",
        "threadId": "_",
        "runId": "@",
        "nodeId": null,
        "providerThreadId": ".",
        "providerTurnId": null,
        "nativeItemRef": {
          "driver": "M",
          "nativeId": "A",
          "strength": "weak"
        },
        "parentItemId": null,
        "ordinal": 1,
        "status": "idle",
        "title": "",
        "startedAt": "-271821-04-20T00:00:00.018Z",
        "completedAt": null,
        "updatedAt": "-239959-08-16T11:43:56.761Z",
        "type": "file_change",
        "fileName": "�"
      }
    },
    "payload_json": "{\"id\":\"N\",\"threadId\":\"_\",\"runId\":\"@\",\"nodeId\":null,\"providerThreadId\":\".\",\"providerTurnId\":null,\"nativeItemRef\":{\"driver\":\"M\",\"nativeId\":\"A\",\"strength\":\"weak\"},\"parentItemId\":null,\"ordinal\":1,\"status\":\"idle\",\"title\":\"\",\"startedAt\":\"-271821-04-20T00:00:00.018Z\",\"completedAt\":null,\"updatedAt\":\"-239959-08-16T11:43:56.761Z\",\"type\":\"file_change\",\"fileName\":\"�\"}"
  },
  {
    "event": {
      "id": "compat:1065",
      "threadId": "compat:thread:0",
      "occurredAt": "2026-09-03T00:00:00.000Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "x",
        "threadId": "GG&q",
        "runId": null,
        "nodeId": null,
        "providerThreadId": null,
        "providerTurnId": null,
        "nativeItemRef": {
          "driver": "K",
          "nativeId": null,
          "strength": "weak"
        },
        "parentItemId": "h",
        "ordinal": 2,
        "status": "failed",
        "title": null,
        "startedAt": "+275760-09-12T23:59:59.965Z",
        "completedAt": "-180458-09-10T04:51:12.294Z",
        "updatedAt": "+044779-11-12T20:19:07.526Z",
        "type": "link_request",
        "reason": "0",
        "linkStatus": "failed"
      }
    },
    "payload_json": "{\"id\":\"x\",\"threadId\":\"GG&q\",\"runId\":null,\"nodeId\":null,\"providerThreadId\":null,\"providerTurnId\":null,\"nativeItemRef\":{\"driver\":\"K\",\"nativeId\":null,\"strength\":\"weak\"},\"parentItemId\":\"h\",\"ordinal\":2,\"status\":\"failed\",\"title\":null,\"startedAt\":\"+275760-09-12T23:59:59.965Z\",\"completedAt\":\"-180458-09-10T04:51:12.294Z\",\"updatedAt\":\"+044779-11-12T20:19:07.526Z\",\"type\":\"link_request\",\"reason\":\"0\",\"linkStatus\":\"failed\"}"
  },
  {
    "event": {
      "id": "compat:1070",
      "threadId": "compat:thread:2",
      "occurredAt": "2026-09-03T00:00:00.000Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "<",
        "threadId": "7",
        "runId": null,
        "nodeId": null,
        "providerThreadId": "{",
        "providerTurnId": "X",
        "nativeItemRef": null,
        "parentItemId": null,
        "ordinal": 0,
        "status": "interrupted",
        "title": "",
        "startedAt": "+112292-09-27T07:56:38.957Z",
        "completedAt": null,
        "updatedAt": "1970-01-01T00:00:00.002Z",
        "type": "assistant_message",
        "messageId": "1",
        "text": "",
        "streaming": false
      }
    },
    "payload_json": "{\"id\":\"<\",\"threadId\":\"7\",\"runId\":null,\"nodeId\":null,\"providerThreadId\":\"{\",\"providerTurnId\":\"X\",\"nativeItemRef\":null,\"parentItemId\":null,\"ordinal\":0,\"status\":\"interrupted\",\"title\":\"\",\"startedAt\":\"+112292-09-27T07:56:38.957Z\",\"completedAt\":null,\"updatedAt\":\"1970-01-01T00:00:00.002Z\",\"type\":\"assistant_message\",\"messageId\":\"1\",\"text\":\"\",\"streaming\":false}"
  },
  {
    "event": {
      "id": "compat:1072",
      "threadId": "compat:thread:1",
      "occurredAt": "2026-09-03T00:00:00.000Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "<",
        "threadId": "7",
        "runId": null,
        "nodeId": null,
        "providerThreadId": "{",
        "providerTurnId": "X",
        "nativeItemRef": null,
        "parentItemId": null,
        "ordinal": 0,
        "status": "interrupted",
        "title": "",
        "startedAt": "+112292-09-27T07:56:38.957Z",
        "completedAt": null,
        "updatedAt": "1970-01-01T00:00:00.002Z",
        "type": "proposed_plan",
        "planId": "1",
        "markdown": "",
        "streaming": false
      }
    },
    "payload_json": "{\"id\":\"<\",\"threadId\":\"7\",\"runId\":null,\"nodeId\":null,\"providerThreadId\":\"{\",\"providerTurnId\":\"X\",\"nativeItemRef\":null,\"parentItemId\":null,\"ordinal\":0,\"status\":\"interrupted\",\"title\":\"\",\"startedAt\":\"+112292-09-27T07:56:38.957Z\",\"completedAt\":null,\"updatedAt\":\"1970-01-01T00:00:00.002Z\",\"type\":\"proposed_plan\",\"planId\":\"1\",\"markdown\":\"\",\"streaming\":false}"
  },
  {
    "event": {
      "id": "compat:1074",
      "threadId": "compat:thread:0",
      "occurredAt": "2026-09-03T00:00:00.000Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "<",
        "threadId": "7",
        "runId": null,
        "nodeId": null,
        "providerThreadId": "{",
        "providerTurnId": "X",
        "nativeItemRef": null,
        "parentItemId": null,
        "ordinal": 0,
        "status": "interrupted",
        "title": "",
        "startedAt": "+112292-09-27T07:56:38.957Z",
        "completedAt": null,
        "updatedAt": "1970-01-01T00:00:00.002Z",
        "type": "user_input_request",
        "requestId": "1",
        "questions": []
      }
    },
    "payload_json": "{\"id\":\"<\",\"threadId\":\"7\",\"runId\":null,\"nodeId\":null,\"providerThreadId\":\"{\",\"providerTurnId\":\"X\",\"nativeItemRef\":null,\"parentItemId\":null,\"ordinal\":0,\"status\":\"interrupted\",\"title\":\"\",\"startedAt\":\"+112292-09-27T07:56:38.957Z\",\"completedAt\":null,\"updatedAt\":\"1970-01-01T00:00:00.002Z\",\"type\":\"user_input_request\",\"requestId\":\"1\",\"questions\":[]}"
  },
  {
    "event": {
      "id": "compat:1080",
      "threadId": "compat:thread:0",
      "occurredAt": "2026-09-03T00:00:00.000Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "$",
        "threadId": "B",
        "runId": null,
        "nodeId": null,
        "providerThreadId": null,
        "providerTurnId": null,
        "nativeItemRef": {
          "driver": "e",
          "nativeId": null,
          "strength": "weak"
        },
        "parentItemId": "\u0000",
        "ordinal": 0,
        "status": "failed",
        "title": "",
        "startedAt": "-151510-09-09T10:29:56.287Z",
        "completedAt": null,
        "updatedAt": "-271821-04-20T00:00:00.000Z",
        "type": "checkpoint",
        "checkpointId": "k",
        "scopeId": "i",
        "files": []
      }
    },
    "payload_json": "{\"id\":\"$\",\"threadId\":\"B\",\"runId\":null,\"nodeId\":null,\"providerThreadId\":null,\"providerTurnId\":null,\"nativeItemRef\":{\"driver\":\"e\",\"nativeId\":null,\"strength\":\"weak\"},\"parentItemId\":\"\\u0000\",\"ordinal\":0,\"status\":\"failed\",\"title\":\"\",\"startedAt\":\"-151510-09-09T10:29:56.287Z\",\"completedAt\":null,\"updatedAt\":\"-271821-04-20T00:00:00.000Z\",\"type\":\"checkpoint\",\"checkpointId\":\"k\",\"scopeId\":\"i\",\"files\":[]}"
  },
  {
    "event": {
      "id": "compat:1081",
      "threadId": "compat:thread:1",
      "occurredAt": "2026-09-03T00:00:00.000Z",
      "type": "turn-item.updated",
      "payload": {
        "deferredWork": {
          "cursor": ")",
          "itemCount": 1,
          "lastOrdinal": 0
        },
        "id": ")",
        "threadId": ")",
        "runId": null,
        "nodeId": null,
        "providerThreadId": null,
        "providerTurnId": null,
        "nativeItemRef": null,
        "parentItemId": null,
        "ordinal": 0,
        "status": "interrupted",
        "title": null,
        "startedAt": null,
        "completedAt": null,
        "updatedAt": "-217480-05-28T21:51:18.757Z",
        "type": "run_interrupt_request",
        "message": "",
        "unresponsiveAt": "-217480-05-28T21:51:18.757Z"
      }
    },
    "payload_json": "{\"deferredWork\":{\"cursor\":\")\",\"itemCount\":1,\"lastOrdinal\":0},\"id\":\")\",\"threadId\":\")\",\"runId\":null,\"nodeId\":null,\"providerThreadId\":null,\"providerTurnId\":null,\"nativeItemRef\":null,\"parentItemId\":null,\"ordinal\":0,\"status\":\"interrupted\",\"title\":null,\"startedAt\":null,\"completedAt\":null,\"updatedAt\":\"-217480-05-28T21:51:18.757Z\",\"type\":\"run_interrupt_request\",\"message\":\"\",\"unresponsiveAt\":\"-217480-05-28T21:51:18.757Z\"}"
  },
  {
    "event": {
      "id": "compat:1082",
      "threadId": "compat:thread:2",
      "occurredAt": "2026-09-03T00:00:00.000Z",
      "type": "turn-item.updated",
      "payload": {
        "deferredWork": {
          "cursor": ")",
          "itemCount": 1,
          "lastOrdinal": 0
        },
        "id": ")",
        "threadId": ")",
        "runId": null,
        "nodeId": null,
        "providerThreadId": null,
        "providerTurnId": null,
        "nativeItemRef": null,
        "parentItemId": null,
        "ordinal": 0,
        "status": "interrupted",
        "title": null,
        "startedAt": null,
        "completedAt": null,
        "updatedAt": "-217480-05-28T21:51:18.757Z",
        "type": "run_interrupt_result",
        "message": ""
      }
    },
    "payload_json": "{\"deferredWork\":{\"cursor\":\")\",\"itemCount\":1,\"lastOrdinal\":0},\"id\":\")\",\"threadId\":\")\",\"runId\":null,\"nodeId\":null,\"providerThreadId\":null,\"providerTurnId\":null,\"nativeItemRef\":null,\"parentItemId\":null,\"ordinal\":0,\"status\":\"interrupted\",\"title\":null,\"startedAt\":null,\"completedAt\":null,\"updatedAt\":\"-217480-05-28T21:51:18.757Z\",\"type\":\"run_interrupt_result\",\"message\":\"\"}"
  },
  {
    "event": {
      "id": "compat:1086",
      "threadId": "compat:thread:0",
      "occurredAt": "2026-09-03T00:00:00.000Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "$",
        "threadId": "B",
        "runId": null,
        "nodeId": null,
        "providerThreadId": null,
        "providerTurnId": null,
        "nativeItemRef": {
          "driver": "e",
          "nativeId": null,
          "strength": "weak"
        },
        "parentItemId": "\u0000",
        "ordinal": 0,
        "status": "failed",
        "title": "",
        "startedAt": "-151510-09-09T10:29:56.287Z",
        "completedAt": null,
        "updatedAt": "-271821-04-20T00:00:00.000Z",
        "type": "handoff",
        "contextHandoffId": "k",
        "fromProviderThreadIds": [],
        "toProviderThreadId": "i",
        "fromProviderInstanceIds": [],
        "toProviderInstanceId": "Y",
        "strategy": "fork_delta_summary"
      }
    },
    "payload_json": "{\"id\":\"$\",\"threadId\":\"B\",\"runId\":null,\"nodeId\":null,\"providerThreadId\":null,\"providerTurnId\":null,\"nativeItemRef\":{\"driver\":\"e\",\"nativeId\":null,\"strength\":\"weak\"},\"parentItemId\":\"\\u0000\",\"ordinal\":0,\"status\":\"failed\",\"title\":\"\",\"startedAt\":\"-151510-09-09T10:29:56.287Z\",\"completedAt\":null,\"updatedAt\":\"-271821-04-20T00:00:00.000Z\",\"type\":\"handoff\",\"contextHandoffId\":\"k\",\"fromProviderThreadIds\":[],\"toProviderThreadId\":\"i\",\"fromProviderInstanceIds\":[],\"toProviderInstanceId\":\"Y\",\"strategy\":\"fork_delta_summary\"}"
  },
  {
    "event": {
      "id": "compat:1088",
      "threadId": "compat:thread:2",
      "occurredAt": "2026-09-03T00:00:00.000Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "$",
        "threadId": "B",
        "runId": null,
        "nodeId": null,
        "providerThreadId": null,
        "providerTurnId": null,
        "nativeItemRef": {
          "driver": "e",
          "nativeId": null,
          "strength": "weak"
        },
        "parentItemId": "\u0000",
        "ordinal": 0,
        "status": "failed",
        "title": "",
        "startedAt": "-151510-09-09T10:29:56.287Z",
        "completedAt": null,
        "updatedAt": "-271821-04-20T00:00:00.000Z",
        "type": "thread_created",
        "targetThreadId": "k",
        "targetRunId": null,
        "targetProviderInstanceId": "A",
        "targetModel": "8"
      }
    },
    "payload_json": "{\"id\":\"$\",\"threadId\":\"B\",\"runId\":null,\"nodeId\":null,\"providerThreadId\":null,\"providerTurnId\":null,\"nativeItemRef\":{\"driver\":\"e\",\"nativeId\":null,\"strength\":\"weak\"},\"parentItemId\":\"\\u0000\",\"ordinal\":0,\"status\":\"failed\",\"title\":\"\",\"startedAt\":\"-151510-09-09T10:29:56.287Z\",\"completedAt\":null,\"updatedAt\":\"-271821-04-20T00:00:00.000Z\",\"type\":\"thread_created\",\"targetThreadId\":\"k\",\"targetRunId\":null,\"targetProviderInstanceId\":\"A\",\"targetModel\":\"8\"}"
  },
  {
    "event": {
      "id": "compat:1131",
      "threadId": "compat:thread:0",
      "occurredAt": "2026-09-03T00:00:00.000Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "O",
        "threadId": "\u0000",
        "runId": null,
        "nodeId": "�",
        "providerThreadId": "E",
        "providerTurnId": "\u0000",
        "nativeItemRef": null,
        "parentItemId": "0",
        "ordinal": 1,
        "status": "pending",
        "title": null,
        "startedAt": "3737-01-02T02:12:01.911Z",
        "completedAt": null,
        "updatedAt": "+205693-08-27T15:37:59.245Z",
        "type": "notification",
        "source": {
          "kind": "background_task",
          "work": "subagent"
        },
        "outcome": "failed",
        "summary": "\\"
      }
    },
    "payload_json": "{\"id\":\"O\",\"threadId\":\"\\u0000\",\"runId\":null,\"nodeId\":\"�\",\"providerThreadId\":\"E\",\"providerTurnId\":\"\\u0000\",\"nativeItemRef\":null,\"parentItemId\":\"0\",\"ordinal\":1,\"status\":\"pending\",\"title\":null,\"startedAt\":\"3737-01-02T02:12:01.911Z\",\"completedAt\":null,\"updatedAt\":\"+205693-08-27T15:37:59.245Z\",\"type\":\"notification\",\"source\":{\"kind\":\"background_task\",\"work\":\"subagent\"},\"outcome\":\"failed\",\"summary\":\"\\\\\"}"
  },
  {
    "event": {
      "id": "compat:1133",
      "threadId": "compat:thread:2",
      "occurredAt": "2026-09-03T00:00:00.000Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "O",
        "threadId": "\u0000",
        "runId": null,
        "nodeId": "�",
        "providerThreadId": "E",
        "providerTurnId": "\u0000",
        "nativeItemRef": null,
        "parentItemId": "0",
        "ordinal": 1,
        "status": "pending",
        "title": null,
        "startedAt": "3737-01-02T02:12:01.911Z",
        "completedAt": null,
        "updatedAt": "+205693-08-27T15:37:59.245Z",
        "createdBy": "agent",
        "creationSource": "mobile",
        "type": "user_message",
        "messageId": "\\",
        "inputIntent": "steer",
        "text": "",
        "attachments": []
      }
    },
    "payload_json": "{\"id\":\"O\",\"threadId\":\"\\u0000\",\"runId\":null,\"nodeId\":\"�\",\"providerThreadId\":\"E\",\"providerTurnId\":\"\\u0000\",\"nativeItemRef\":null,\"parentItemId\":\"0\",\"ordinal\":1,\"status\":\"pending\",\"title\":null,\"startedAt\":\"3737-01-02T02:12:01.911Z\",\"completedAt\":null,\"updatedAt\":\"+205693-08-27T15:37:59.245Z\",\"createdBy\":\"agent\",\"creationSource\":\"mobile\",\"type\":\"user_message\",\"messageId\":\"\\\\\",\"inputIntent\":\"steer\",\"text\":\"\",\"attachments\":[]}"
  },
  {
    "event": {
      "id": "compat:1147",
      "threadId": "compat:thread:1",
      "occurredAt": "2026-09-03T00:00:00.000Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "O",
        "threadId": "\u0000",
        "runId": null,
        "nodeId": "�",
        "providerThreadId": "E",
        "providerTurnId": "\u0000",
        "nativeItemRef": null,
        "parentItemId": "0",
        "ordinal": 1,
        "status": "pending",
        "title": null,
        "startedAt": "3737-01-02T02:12:01.911Z",
        "completedAt": null,
        "updatedAt": "+205693-08-27T15:37:59.245Z",
        "type": "command_execution",
        "input": ""
      }
    },
    "payload_json": "{\"id\":\"O\",\"threadId\":\"\\u0000\",\"runId\":null,\"nodeId\":\"�\",\"providerThreadId\":\"E\",\"providerTurnId\":\"\\u0000\",\"nativeItemRef\":null,\"parentItemId\":\"0\",\"ordinal\":1,\"status\":\"pending\",\"title\":null,\"startedAt\":\"3737-01-02T02:12:01.911Z\",\"completedAt\":null,\"updatedAt\":\"+205693-08-27T15:37:59.245Z\",\"type\":\"command_execution\",\"input\":\"\"}"
  },
  {
    "event": {
      "id": "compat:1151",
      "threadId": "compat:thread:2",
      "occurredAt": "2026-09-03T00:00:00.000Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "O",
        "threadId": "\u0000",
        "runId": null,
        "nodeId": "�",
        "providerThreadId": "E",
        "providerTurnId": "\u0000",
        "nativeItemRef": null,
        "parentItemId": "0",
        "ordinal": 1,
        "status": "pending",
        "title": null,
        "startedAt": "3737-01-02T02:12:01.911Z",
        "completedAt": null,
        "updatedAt": "+205693-08-27T15:37:59.245Z",
        "type": "web_search"
      }
    },
    "payload_json": "{\"id\":\"O\",\"threadId\":\"\\u0000\",\"runId\":null,\"nodeId\":\"�\",\"providerThreadId\":\"E\",\"providerTurnId\":\"\\u0000\",\"nativeItemRef\":null,\"parentItemId\":\"0\",\"ordinal\":1,\"status\":\"pending\",\"title\":null,\"startedAt\":\"3737-01-02T02:12:01.911Z\",\"completedAt\":null,\"updatedAt\":\"+205693-08-27T15:37:59.245Z\",\"type\":\"web_search\"}"
  },
  {
    "event": {
      "id": "compat:1163",
      "threadId": "compat:thread:2",
      "occurredAt": "2026-09-03T00:00:00.000Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "O",
        "threadId": "\u0000",
        "runId": null,
        "nodeId": "�",
        "providerThreadId": "E",
        "providerTurnId": "\u0000",
        "nativeItemRef": null,
        "parentItemId": "0",
        "ordinal": 1,
        "status": "pending",
        "title": null,
        "startedAt": "3737-01-02T02:12:01.911Z",
        "completedAt": null,
        "updatedAt": "+205693-08-27T15:37:59.245Z",
        "type": "error",
        "failure": {
          "class": "transport_error",
          "message": "�",
          "code": null,
          "retryable": null
        }
      }
    },
    "payload_json": "{\"id\":\"O\",\"threadId\":\"\\u0000\",\"runId\":null,\"nodeId\":\"�\",\"providerThreadId\":\"E\",\"providerTurnId\":\"\\u0000\",\"nativeItemRef\":null,\"parentItemId\":\"0\",\"ordinal\":1,\"status\":\"pending\",\"title\":null,\"startedAt\":\"3737-01-02T02:12:01.911Z\",\"completedAt\":null,\"updatedAt\":\"+205693-08-27T15:37:59.245Z\",\"type\":\"error\",\"failure\":{\"class\":\"transport_error\",\"message\":\"�\",\"code\":null,\"retryable\":null}}"
  },
  {
    "event": {
      "id": "compat:1165",
      "threadId": "compat:thread:1",
      "occurredAt": "2026-09-03T00:00:00.000Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "O",
        "threadId": "\u0000",
        "runId": null,
        "nodeId": "�",
        "providerThreadId": "E",
        "providerTurnId": "\u0000",
        "nativeItemRef": null,
        "parentItemId": "0",
        "ordinal": 1,
        "status": "pending",
        "title": null,
        "startedAt": "3737-01-02T02:12:01.911Z",
        "completedAt": null,
        "updatedAt": "+205693-08-27T15:37:59.245Z",
        "type": "compaction",
        "driver": "m"
      }
    },
    "payload_json": "{\"id\":\"O\",\"threadId\":\"\\u0000\",\"runId\":null,\"nodeId\":\"�\",\"providerThreadId\":\"E\",\"providerTurnId\":\"\\u0000\",\"nativeItemRef\":null,\"parentItemId\":\"0\",\"ordinal\":1,\"status\":\"pending\",\"title\":null,\"startedAt\":\"3737-01-02T02:12:01.911Z\",\"completedAt\":null,\"updatedAt\":\"+205693-08-27T15:37:59.245Z\",\"type\":\"compaction\",\"driver\":\"m\"}"
  },
  {
    "event": {
      "id": "compat:1169",
      "threadId": "compat:thread:2",
      "occurredAt": "2026-09-03T00:00:00.000Z",
      "type": "turn-item.updated",
      "payload": {
        "id": "O",
        "threadId": "\u0000",
        "runId": null,
        "nodeId": "�",
        "providerTurnId": "E",
        "nativeItemRef": {
          "driver": "c",
          "nativeId": null,
          "strength": "strong"
        },
        "parentItemId": "g",
        "ordinal": 0,
        "status": "failed",
        "title": "",
        "startedAt": "+014041-01-03T21:29:58.228Z",
        "completedAt": null,
        "updatedAt": "1969-12-31T23:59:59.982Z",
        "type": "fork",
        "source": {
          "type": "provider_thread",
          "providerThreadId": "a"
        },
        "targetThreadId": "L"
      }
    },
    "payload_json": "{\"id\":\"O\",\"threadId\":\"\\u0000\",\"runId\":null,\"nodeId\":\"�\",\"providerTurnId\":\"E\",\"nativeItemRef\":{\"driver\":\"c\",\"nativeId\":null,\"strength\":\"strong\"},\"parentItemId\":\"g\",\"ordinal\":0,\"status\":\"failed\",\"title\":\"\",\"startedAt\":\"+014041-01-03T21:29:58.228Z\",\"completedAt\":null,\"updatedAt\":\"1969-12-31T23:59:59.982Z\",\"type\":\"fork\",\"source\":{\"type\":\"provider_thread\",\"providerThreadId\":\"a\"},\"targetThreadId\":\"L\"}"
  }
]"###;
