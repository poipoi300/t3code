use t3_contracts::{base_schemas::*, orchestration_v2::OrchestrationV2DomainEvent};
use t3_orchestration::{
    error::Result,
    stores::event_store::{EventFilter, EventRow, EventStore, canonical_date},
    testing::{TEST_TIME, TestDatabase, detached_event, project_event},
};

#[derive(serde::Deserialize)]
struct CompatibilityFixture {
    inputs: Vec<OrchestrationV2DomainEvent>,
    rows: Vec<EventRow>,
    numbers: Vec<NumberEncoding>,
    rejected: Vec<RejectedInput>,
}
#[derive(serde::Deserialize)]
struct RejectedInput {
    reason: String,
    input: serde_json::Value,
}
#[derive(serde::Deserialize)]
struct NumberEncoding {
    input: String,
    encoded: String,
}
#[derive(serde::Deserialize)]
struct ProjectCompatibilityFixture {
    inputs: Vec<t3_contracts::application_event::ApplicationProjectEvent>,
    rows: Vec<EventRow>,
}
#[tokio::test]
async fn every_project_payload_and_actor_matches_the_real_typescript_store() -> Result<()> {
    let database = TestDatabase::memory()?;
    database
        .stores
        .transaction(|tx| {
            let fixture: ProjectCompatibilityFixture =
                serde_json::from_str(include_str!("../fixtures/project-event-compatibility.json"))?;
            let store = EventStore::new();
            let mut stored = Vec::new();
            for event in fixture.inputs {
                stored.push(store.append_project(tx, &event)?);
            }
            assert_eq!(store.raw_rows(tx, &EventFilter::default())?, fixture.rows);
            let replayed = store.read_application(tx, &EventFilter::default())?;
            assert_eq!(
                replayed,
                stored
                    .into_iter()
                    .map(
                        |event| t3_contracts::application_event::ApplicationStoredEvent::Project(
                            Box::new(event)
                        )
                    )
                    .collect::<Vec<_>>()
            );
            assert_eq!(store.latest_agent_sequence(tx, None)?, 0);
            assert_eq!(store.latest_application_sequence(tx)?, 10);
            Ok(())
        })
        .await
}
#[tokio::test]
async fn every_event_and_turn_item_variant_matches_the_real_typescript_store() -> Result<()> {
    use std::collections::BTreeSet;
    use t3_contracts::{orchestration_v2::OrchestrationV2TurnItem, wire::TaggedUnion};
    let fixture = TestDatabase::memory()?;
    fixture
        .stores
        .transaction(|tx| {
            let fixture: CompatibilityFixture =
                serde_json::from_str(include_str!("../fixtures/event-compatibility.json"))?;
            let store = EventStore::new();
            let mut events = Vec::new();
            let mut tags = BTreeSet::new();
            let mut item_tags = BTreeSet::new();
            for batch in fixture.inputs.chunks(17) {
                events.extend(store.append(
                    tx,
                    Some(&CommandId("command:compat".into())),
                    batch,
                )?);
            }
            let rows = store.raw_rows(tx, &EventFilter::default())?;
            assert_eq!(rows.len(), fixture.rows.len());
            for (actual, expected) in rows.iter().zip(&fixture.rows) {
                tags.insert(actual.event_type.as_str());
                if actual.event_type == "turn-item.updated" {
                    let payload: serde_json::Value = serde_json::from_str(&actual.payload_json)?;
                    item_tags.insert(payload["type"].as_str().expect("item tag").to_owned());
                }
                assert_eq!(
                    actual, expected,
                    "event {} ({})",
                    actual.event_id, actual.event_type
                );
            }
            assert_eq!(
                tags,
                OrchestrationV2DomainEvent::KNOWN_TAGS
                    .iter()
                    .copied()
                    .collect()
            );
            assert_eq!(
                item_tags,
                OrchestrationV2TurnItem::KNOWN_TAGS
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            );
            assert_eq!(store.read(tx, &EventFilter::default())?, events);
            Ok(())
        })
        .await
}
#[tokio::test]
async fn javascript_numbers_are_encoded_in_native_payloads() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    fixture
        .stores
        .transaction(|tx| {
            let fixture: CompatibilityFixture =
                serde_json::from_str(include_str!("../fixtures/event-compatibility.json"))?;
            let store = EventStore::new();
            let template = fixture
                .inputs
                .iter()
                .map(serde_json::to_value)
                .collect::<std::result::Result<Vec<_>, _>>()?
                .into_iter()
                .find(|event| event["payload"]["id"] == "numeric:tool")
                .expect("numeric tool");
            for (index, case) in fixture.numbers.iter().enumerate() {
                let mut value = template.clone();
                value["id"] = format!("number:{index}").into();
                value["payload"]["input"] = serde_json::from_str(&case.input)?;
                store.append(tx, None, &[serde_json::from_value(value)?])?;
                let payload: String = tx.query_row(
                    "SELECT payload_json FROM orchestration_events WHERE event_id=?1",
                    [format!("number:{index}")],
                    |row| row.get(0),
                )?;
                assert!(
                    payload.contains(&format!("\"input\":{},", case.encoded))
                        || payload.contains(&format!("\"input\":{}}}", case.encoded)),
                    "input {}: {payload}",
                    case.input
                );
            }
            let mut value = template;
            value["id"] = "ordered-map".into();
            value["payload"]["input"] = serde_json::from_str(
                r#"{"z":0,"10":10,"2":2,"01":1,"0":0,"4294967294":4,"4294967295":5}"#,
            )?;
            store.append(tx, None, &[serde_json::from_value(value)?])?;
            let payload: String = tx.query_row(
                "SELECT payload_json FROM orchestration_events WHERE event_id='ordered-map'",
                [],
                |row| row.get(0),
            )?;
            assert!(payload.contains(
                r#""input":{"0":0,"2":2,"10":10,"4294967294":4,"z":0,"01":1,"4294967295":5}"#
            ));
            Ok(())
        })
        .await
}

#[tokio::test]
async fn exact_typescript_rows_and_json_bytes() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    fixture
        .stores
        .transaction(|tx| {
            let events: Vec<OrchestrationV2DomainEvent> =
                serde_json::from_str(include_str!("../fixtures/event-input.json"))?;
            let store = EventStore::new();
            let stored = store.append(tx, Some(&CommandId("command:compat".into())), &events)?;
            assert_eq!(stored.len(), 3);
            let expected: Vec<EventRow> =
                serde_json::from_str(include_str!("../fixtures/ts-event-rows.json"))?;
            assert_eq!(store.raw_rows(tx, &EventFilter::default())?, expected);
            assert_eq!(store.read(tx, &EventFilter::default())?, stored);
            Ok(())
        })
        .await
}
#[tokio::test]
async fn project_and_agent_streams_share_sequence_with_zero_based_versions() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    fixture
        .stores
        .transaction(|tx| {
            let store = EventStore::new();
            store.append_project(tx, &project_event("project-event", "project")?)?;
            let stored = store.append(
                tx,
                None,
                &[
                    detached_event("one", "thread")?,
                    detached_event("two", "thread")?,
                    detached_event("three", "other")?,
                ],
            )?;
            assert_eq!(
                stored.iter().map(|e| e.sequence).collect::<Vec<_>>(),
                [2, 3, 4]
            );
            let rows = store.raw_rows(tx, &EventFilter::default())?;
            assert_eq!(
                rows.iter().map(|r| r.stream_version).collect::<Vec<_>>(),
                [0, 0, 1, 0]
            );
            assert_eq!(
                store.read_application(tx, &EventFilter::default())?.len(),
                4
            );
            assert_eq!(store.read(tx, &EventFilter::default())?.len(), 3);
            Ok(())
        })
        .await
}
fn subagent(id: &str, prompt: &str) -> Result<OrchestrationV2DomainEvent> {
    Ok(serde_json::from_value(
        serde_json::json!({"id":id,"threadId":"thread","type":"subagent.updated","occurredAt":TEST_TIME,"payload":{"id":"subagent","threadId":"thread","runId":null,"parentNodeId":"parent","origin":"provider_native","createdBy":"agent","driver":"claudeAgent","providerInstanceId":"claudeAgent","providerThreadId":null,"childThreadId":null,"nativeTaskRef":null,"prompt":prompt,"progress":"Reading","title":"Audit","model":null,"status":"running","result":null,"startedAt":TEST_TIME,"completedAt":null,"updatedAt":TEST_TIME}}),
    )?)
}
fn subagent_item(id: &str, prompt: &str) -> Result<OrchestrationV2DomainEvent> {
    Ok(serde_json::from_value(serde_json::json!({
        "id":id,"threadId":"thread","type":"turn-item.updated","occurredAt":TEST_TIME,
        "payload":{"id":"subagent","type":"subagent","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"running","title":"Audit","subagentId":"subagent","origin":"provider_native","driver":"claudeAgent","providerInstanceId":"claudeAgent","childThreadId":null,"prompt":prompt,"progress":"Reading","result":null,"startedAt":TEST_TIME,"completedAt":null,"updatedAt":TEST_TIME}
    }))?)
}

#[tokio::test]
async fn both_prompt_hint_keys_handle_separate_appends_rollback_and_deleted_anchors() -> Result<()>
{
    let fixture = TestDatabase::memory()?;
    let store = std::sync::Arc::new(EventStore::new());
    let rollback_store = store.clone();
    let rollback: Result<()> = fixture
        .stores
        .transaction(move |tx| {
            rollback_store.append(
                tx,
                None,
                &[
                    subagent("rolled-back:subagent", "prompt")?,
                    subagent_item("rolled-back:item", "prompt")?,
                ],
            )?;
            Err(t3_orchestration::error::Error::Cancelled)
        })
        .await;
    assert!(rollback.is_err());
    fixture
        .stores
        .transaction(move |tx| {
            let mut stored = Vec::new();
            let prompt = "Audit 🦀\n".repeat(200);
            for (index, text) in [&prompt, &prompt, &prompt, "changed", "changed"]
                .into_iter()
                .enumerate()
            {
                stored.extend(store.append(
                    tx,
                    None,
                    &[subagent(&format!("subagent:{index}"), text)?],
                )?);
                stored.extend(store.append(
                    tx,
                    None,
                    &[subagent_item(&format!("item:{index}"), text)?],
                )?);
            }
            let rows = store.raw_rows(tx, &EventFilter::default())?;
            let payloads = rows
                .iter()
                .map(|row| serde_json::from_str::<serde_json::Value>(&row.payload_json))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            assert_eq!(
                payloads
                    .iter()
                    .map(|payload| payload["promptSequence"].as_i64())
                    .collect::<Vec<_>>(),
                [
                    None,
                    None,
                    Some(1),
                    Some(2),
                    Some(1),
                    Some(2),
                    None,
                    None,
                    Some(7),
                    Some(8)
                ]
            );
            assert_eq!(store.read(tx, &EventFilter::default())?, stored);
            assert_eq!(
                store.read_application(tx, &EventFilter::default())?.len(),
                stored.len()
            );
            for agents in [false, true] {
                assert!(
                    store
                        .replay_stats(tx, &EventFilter::default(), agents, None)?
                        .raw_payload_bytes
                        > (prompt.len() * 6) as i64
                );
            }
            tx.execute(
                "DELETE FROM orchestration_events WHERE sequence IN (7,8)",
                [],
            )?;
            let after_deletion = [
                subagent("new:subagent", "changed")?,
                subagent_item("new:item", "changed")?,
            ];
            let new_rows = store.append(tx, None, &after_deletion)?;
            let filter = EventFilter {
                after_sequence: 10,
                ..Default::default()
            };
            assert!(
                store
                    .raw_rows(tx, &filter)?
                    .iter()
                    .all(|row| !row.payload_json.contains("promptSequence"))
            );
            assert_eq!(store.read(tx, &filter)?, new_rows);
            Ok(())
        })
        .await
}

#[tokio::test]
async fn prompt_hint_eviction_uses_insertion_order_and_refreshes_touched_entries() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    fixture
        .stores
        .transaction(|tx| {
            let store = EventStore::new();
            let event = |id: usize, suffix: &str| -> Result<OrchestrationV2DomainEvent> {
                let mut value =
                    serde_json::to_value(subagent(&format!("hint:{id}:{suffix}"), "prompt")?)?;
                value["payload"]["id"] = format!("agent:{id}").into();
                Ok(serde_json::from_value(value)?)
            };
            for id in 0..4096 {
                store.append(tx, None, &[event(id, "initial")?])?;
            }
            store.append(tx, None, &[event(0, "touch")?])?;
            store.append(tx, None, &[event(4096, "overflow")?])?;
            store.append(tx, None, &[event(0, "retained")?, event(1, "evicted")?])?;
            let rows = store.raw_rows(
                tx,
                &EventFilter {
                    after_sequence: 4098,
                    ..Default::default()
                },
            )?;
            assert!(rows[0].payload_json.contains("\"promptSequence\":1"));
            assert!(!rows[1].payload_json.contains("promptSequence"));
            Ok(())
        })
        .await
}

#[tokio::test]
async fn replay_stats_count_utf8_and_bound_raw_payloads_before_domain_decoding() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    fixture.stores.transaction(|tx| {
        let unicode = "{\"text\":\"🦊\"}";
        let oversized = format!("{{{}", "x".repeat(1_048_576));
        for (id, thread, version, tag, payload) in [
            ("first", "target", 2, "thread.created", unicode),
            ("other", "other", 2, "thread.metadata-updated", oversized.as_str()),
            ("legacy", "target", 1, "thread.metadata-updated", oversized.as_str()),
            ("head", "target", 2, "thread.metadata-updated", oversized.as_str()),
            ("later", "target", 2, "thread.metadata-updated", "{}"),
        ] {
            tx.execute("INSERT INTO orchestration_events(event_id,aggregate_kind,stream_id,stream_version,event_type,occurred_at,actor_kind,payload_json,metadata_json,application_event_version) VALUES(?1,'thread',?2,(SELECT COUNT(*) FROM orchestration_events),?3,?4,'server',?5,'{}',?6)", t3_persistence::rusqlite::params![id,thread,tag,TEST_TIME,payload,version])?;
        }
        let store = EventStore::new();
        let filter = EventFilter { thread_id: Some(ThreadId("target".into())), through_sequence: Some(4), ..Default::default() };
        let stats = store.replay_stats(tx, &filter, true, Some(128))?;
        assert_eq!((stats.event_count, stats.raw_payload_bytes, stats.has_create_event), (2, (unicode.len() + oversized.len()) as i64, true));
        let stats = store.replay_stats(tx, &EventFilter { after_sequence: 1, ..filter.clone() }, true, Some(128))?;
        assert_eq!((stats.event_count, stats.raw_payload_bytes, stats.has_create_event), (1, oversized.len() as i64, false));
        assert_eq!(store.replay_stats(tx, &EventFilter { through_sequence: Some(5), ..filter.clone() }, true, Some(1))?.event_count, 2);
        let empty = store.replay_stats(tx, &EventFilter { after_sequence: 5, ..filter }, true, Some(128))?;
        assert_eq!(empty, Default::default());
        let stats = store.replay_stats(tx, &EventFilter { through_sequence: Some(4), ..Default::default() }, false, None)?;
        assert_eq!((stats.event_count, stats.raw_payload_bytes), (3, (unicode.len() + 2 * oversized.len()) as i64));
        Ok(())
    }).await
}
#[tokio::test]
async fn prompt_anchors_restore_and_survive_rolled_back_hints() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    let store = std::sync::Arc::new(EventStore::new());
    let first = store.clone();
    let result: Result<()> = fixture
        .stores
        .transaction(move |tx| {
            first.append(
                tx,
                None,
                &[subagent("rolled-back", &"Long prompt 🦀".repeat(200))?],
            )?;
            Err(t3_orchestration::error::Error::Cancelled)
        })
        .await;
    assert!(result.is_err());
    fixture
        .stores
        .transaction(move |tx| {
            let events = [
                subagent("one", &"Long prompt 🦀".repeat(200))?,
                subagent("two", &"Long prompt 🦀".repeat(200))?,
                subagent("three", "changed")?,
            ];
            let stored = store.append(tx, None, &events)?;
            let rows = store.raw_rows(tx, &EventFilter::default())?;
            assert!(!rows[0].payload_json.contains("promptSequence"));
            assert!(rows[1].payload_json.contains("\"promptSequence\":1"));
            assert!(!rows[2].payload_json.contains("promptSequence"));
            assert_eq!(store.read(tx, &EventFilter::default())?, stored);
            let bytes = store
                .replay_stats(tx, &EventFilter::default(), true, None)?
                .raw_payload_bytes;
            assert!(
                bytes
                    > rows
                        .iter()
                        .map(|r| r.payload_json.len() as i64)
                        .sum::<i64>()
            );
            Ok(())
        })
        .await
}
#[tokio::test]
async fn unknown_tags_fail_internal_replay_and_skip_only_by_opt_in() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    fixture
        .stores
        .transaction(|tx| {
            let store = EventStore::new();
            store.append(
                tx,
                None,
                &[
                    detached_event("one", "thread")?,
                    detached_event("two", "thread")?,
                ],
            )?;
            tx.execute(
                "UPDATE orchestration_events SET event_type='future.event' WHERE sequence=1",
                [],
            )?;
            assert!(store.read(tx, &EventFilter::default()).is_err());
            let read = store.read(
                tx,
                &EventFilter {
                    skip_unknown_event_types: true,
                    ..Default::default()
                },
            )?;
            assert_eq!(read.len(), 1);
            assert_eq!(read[0].sequence, 2);
            Ok(())
        })
        .await
}

#[tokio::test]
async fn undefined_metadata_is_omitted_and_stored_null_metadata_is_rejected() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    fixture
        .stores
        .transaction(|tx| {
            let store = EventStore::new();
            for key in [
                "runId",
                "nodeId",
                "driver",
                "providerInstanceId",
                "rawEventId",
            ] {
                // An RPC-dialect null decodes as TypeScript's undefined; the stored dialect omits it.
                let mut value = serde_json::to_value(detached_event(key, "thread")?)?;
                value[key] = serde_json::Value::Null;
                let event: OrchestrationV2DomainEvent = serde_json::from_value(value)?;
                store.append(tx, None, &[event])?;
            }
            let rows = store.raw_rows(tx, &EventFilter::default())?;
            assert!(rows.iter().all(|row| row.metadata_json == "{}"));
            assert!(rows.iter().all(|row| row.actor_kind == "server"));
            tx.execute(
                "UPDATE orchestration_events SET metadata_json='{\"rawEventId\":null}'",
                [],
            )?;
            assert!(store.read(tx, &EventFilter::default()).is_err());
            Ok(())
        })
        .await
}

#[tokio::test]
async fn paged_replay_skips_full_unknown_pages_counts_raw_limits_and_rejects_known_corruption()
-> Result<()> {
    for (agents_only, project) in [(true, false), (false, false), (false, true)] {
        let fixture = TestDatabase::memory()?;
        fixture.stores.transaction(move |tx| {
            let kind = if project { "project" } else { "thread" };
            let known = if project { "project.meta-updated" } else { "provider-session.detached" };
            let payload = if project {
                format!("{{\"projectId\":\"thread\",\"updatedAt\":\"{TEST_TIME}\"}}")
            } else {
                format!("{{\"providerSessionId\":\"session\",\"detachedAt\":\"{TEST_TIME}\"}}")
            };
            for index in 0..1005 {
                let (tag, body) = match index {
                    0 | 1002 | 1004 => (known, payload.as_str()),
                    1001 => (if project { "provider-session.detached" } else { "project.meta-updated" }, "{}"),
                    _ => ("future.event", "not JSON"),
                };
                tx.execute("INSERT INTO orchestration_events(event_id,aggregate_kind,stream_id,stream_version,event_type,occurred_at,actor_kind,payload_json,metadata_json,application_event_version) VALUES(?1,?2,'thread',?3,?4,?5,'server',?6,'{}',2)", t3_persistence::rusqlite::params![format!("event:{index}"), kind,index,tag,TEST_TIME,body])?;
            }
            Ok(())
        }).await?;
        let store = EventStore::new();
        let filter = EventFilter {
            through_sequence: Some(1004),
            skip_unknown_event_types: true,
            ..Default::default()
        };
        if agents_only {
            let mut pages = store.read_paged(fixture.stores.clone(), filter.clone());
            let mut sequences = Vec::new();
            while let Some(page) = pages.next_page().await? {
                sequences.extend(page.into_iter().map(|event| event.sequence));
            }
            assert_eq!(sequences, [1, 1003]);
            let mut pages = store.read_paged(
                fixture.stores.clone(),
                EventFilter {
                    limit: Some(2),
                    ..filter.clone()
                },
            );
            assert_eq!(
                pages.next_page().await?.expect("one retained event").len(),
                1
            );
            assert!(pages.next_page().await?.is_none());
            let mut pages = store.read_paged(
                fixture.stores.clone(),
                EventFilter {
                    skip_unknown_event_types: false,
                    ..filter
                },
            );
            assert!(pages.next_page().await.is_err());
        } else {
            let mut pages = store.read_application_paged(fixture.stores.clone(), filter.clone());
            let mut sequences = Vec::new();
            while let Some(page) = pages.next_page().await? {
                sequences.extend(page.into_iter().map(|event| {
                    serde_json::to_value(event).expect("event")["sequence"]
                        .as_u64()
                        .expect("sequence")
                }));
            }
            assert_eq!(sequences, [1, 1003]);
            let mut pages = store.read_application_paged(
                fixture.stores.clone(),
                EventFilter {
                    skip_unknown_event_types: false,
                    ..filter
                },
            );
            assert!(pages.next_page().await.is_err());
        }
        fixture.stores.transaction(move |tx| {
            tx.execute("UPDATE orchestration_events SET event_type=?1,payload_json='{}' WHERE sequence=1005", [if project { "project.created" } else { "thread.metadata-updated" }])?;
            Ok(())
        }).await?;
        let filter = EventFilter {
            after_sequence: 1004,
            through_sequence: Some(1005),
            skip_unknown_event_types: true,
            ..Default::default()
        };
        if agents_only {
            assert!(
                store
                    .read_paged(fixture.stores.clone(), filter)
                    .next_page()
                    .await
                    .is_err()
            );
        } else {
            assert!(
                store
                    .read_application_paged(fixture.stores.clone(), filter)
                    .next_page()
                    .await
                    .is_err()
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Store oracle (`tools/verify-event-encoding.ts`): every expected value below comes from the
// real TypeScript stores. Shared helpers are used by the other package-01 test modules.
// ---------------------------------------------------------------------------------------------

pub(crate) fn store_oracle() -> &'static serde_json::Value {
    static ORACLE: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    ORACLE.get_or_init(|| {
        serde_json::from_str(include_str!("../fixtures/store-oracle.json"))
            .expect("store oracle fixture")
    })
}

/// Equality of JSON as JavaScript sees it: numbers by value, object keys in any order.
pub(crate) fn same_json(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    use serde_json::Value;
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(l, r)| same_json(l, r))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .all(|(key, l)| right.get(key).is_some_and(|r| same_json(l, r)))
        }
        _ => left == right,
    }
}

#[track_caller]
pub(crate) fn assert_same_json(
    actual: &serde_json::Value,
    expected: &serde_json::Value,
    what: &str,
) {
    assert!(
        same_json(actual, expected),
        "{what}\n  actual: {actual}\nexpected: {expected}"
    );
}

/// Every column of every row, in rowid order, as the TypeScript snapshot records it.
pub(crate) fn snapshot(
    db: &t3_persistence::rusqlite::Connection,
    table: &str,
) -> Result<Vec<serde_json::Value>> {
    use t3_persistence::rusqlite::types::ValueRef;
    let mut statement = db.prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))?;
    let names: Vec<String> = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let rows = statement.query_map([], |row| {
        let mut object = serde_json::Map::new();
        for (index, name) in names.iter().enumerate() {
            let value = match row.get_ref(index)? {
                ValueRef::Null => serde_json::Value::Null,
                ValueRef::Integer(value) => value.into(),
                ValueRef::Real(value) => value.into(),
                ValueRef::Text(text) => String::from_utf8_lossy(text).into_owned().into(),
                ValueRef::Blob(_) => serde_json::Value::String("<blob>".into()),
            };
            object.insert(name.clone(), value);
        }
        Ok(serde_json::Value::Object(object))
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

#[track_caller]
pub(crate) fn assert_rows(actual: &[serde_json::Value], expected: &serde_json::Value, what: &str) {
    let expected = expected.as_array().expect("snapshot rows");
    assert_eq!(actual.len(), expected.len(), "{what}: row count");
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(actual, expected, "{what}");
    }
}

fn insert_event_rows(
    db: &t3_persistence::rusqlite::Connection,
    rows: &[serde_json::Value],
) -> Result<()> {
    for row in rows {
        db.execute(
            "INSERT INTO orchestration_events (sequence, event_id, aggregate_kind, stream_id, \
             stream_version, event_type, occurred_at, command_id, causation_event_id, \
             correlation_id, actor_kind, payload_json, metadata_json, application_event_version) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            t3_persistence::rusqlite::params![
                row["sequence"].as_i64(),
                row["event_id"].as_str(),
                row["aggregate_kind"].as_str(),
                row["stream_id"].as_str(),
                row["stream_version"].as_i64(),
                row["event_type"].as_str(),
                row["occurred_at"].as_str(),
                row["command_id"].as_str(),
                row["causation_event_id"].as_str(),
                row["correlation_id"].as_str(),
                row["actor_kind"].as_str(),
                row["payload_json"].as_str(),
                row["metadata_json"].as_str(),
                row["application_event_version"].as_i64(),
            ],
        )?;
    }
    Ok(())
}

fn stored_json(
    event: &t3_contracts::orchestration_v2::OrchestrationV2StoredEvent,
) -> Result<serde_json::Value> {
    Ok(serde_json::json!({
        "sequence": event.sequence,
        "commandId": event.command_id,
        "event": serde_json::to_value(&event.event)?,
    }))
}

/// `file!()` is workspace-relative in a workspace build and absolute when the tests are included
/// by an absolute path, so the root is found by walking up to this crate's oracle tool.
fn repository_root() -> std::path::PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    [manifest.to_path_buf(), manifest.join(file!())]
        .iter()
        .flat_map(|start| start.ancestors())
        .find(|dir| {
            dir.join("rust/crates/t3-orchestration/tools/verify-event-encoding.ts")
                .is_file()
        })
        .map(std::path::Path::to_path_buf)
        .expect("repository root")
}

/// FNV-1a over the oracle's TypeScript inputs, CR stripped, exactly as the tool computes it.
fn fnv(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes().filter(|byte| *byte != b'\r') {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[test]
fn checked_in_oracle_output_matches_the_typescript_sources_and_the_generated_shape() -> Result<()> {
    let oracle = store_oracle();
    let root = repository_root();
    let mut sources = String::new();
    for file in oracle["sourceFiles"].as_array().expect("source files") {
        let file = file.as_str().expect("source path");
        sources.push_str(file);
        sources.push('\n');
        sources.push_str(&std::fs::read_to_string(root.join(file))?);
        sources.push('\n');
    }
    assert_eq!(
        fnv(&sources),
        oracle["sourceHash"],
        "a TypeScript input of the store oracle changed: run \
         `node rust/crates/t3-orchestration/tools/verify-event-encoding.ts --refresh`"
    );
    let shape = include_str!("../../src/stores/persisted-event-shape.json");
    assert_eq!(
        fnv(shape),
        oracle["shapeHash"],
        "persisted-event-shape.json is stale"
    );
    Ok(())
}

/// Inputs Rust accepts only through V8's legacy date parser fallback, which the store does not
/// reproduce. They are rejected, never stored differently; no producer writes these forms.
const LEGACY_ONLY_DATES: [&str; 2] = ["Thu, 03 Sep 2026 00:00:00 GMT", "2026-09-3"];

#[test]
fn dates_are_canonicalised_exactly_as_effect_date_time_utc_from_string() {
    let mut legacy = Vec::new();
    for case in store_oracle()["dates"].as_array().expect("dates") {
        let input = case["input"].as_str().expect("input");
        let expected = case["canonical"].as_str();
        let actual = canonical_date(input);
        match (actual.as_deref(), expected) {
            (actual, expected) if actual == expected => {}
            (None, Some(_)) => legacy.push(input),
            (actual, expected) => panic!("{input:?}: Rust {actual:?}, TypeScript {expected:?}"),
        }
    }
    assert_eq!(legacy, LEGACY_ONLY_DATES);
}

#[tokio::test]
async fn raw_inputs_are_written_as_the_typescript_store_writes_them() -> Result<()> {
    let database = TestDatabase::memory()?;
    database
        .stores
        .transaction(|tx| {
            let events = &store_oracle()["events"];
            let store = EventStore::new();
            let mut appended = Vec::new();
            for input in events["inputs"].as_array().expect("inputs") {
                let command = input["commandId"].as_str().map(|id| CommandId(id.into()));
                let event: OrchestrationV2DomainEvent =
                    serde_json::from_value(input["event"].clone())?;
                appended.extend(store.append(tx, command.as_ref(), &[event])?);
            }
            assert_rows(
                &snapshot(tx, "orchestration_events")?,
                &events["rows"],
                "raw input rows",
            );
            let reads = events["reads"].as_array().expect("reads");
            let read = store.read(tx, &EventFilter::default())?;
            assert_eq!(read.len(), reads.len());
            for ((read, appended), expected) in read.iter().zip(&appended).zip(reads) {
                assert_same_json(&stored_json(read)?, expected, "Rust read of a raw input");
                assert_same_json(
                    &stored_json(appended)?,
                    expected,
                    "append result of a raw input",
                );
            }
            Ok(())
        })
        .await
}

#[tokio::test]
async fn rows_written_by_typescript_read_as_typescript_reads_them() -> Result<()> {
    let database = TestDatabase::memory()?;
    database
        .stores
        .transaction(|tx| {
            let events = &store_oracle()["events"];
            insert_event_rows(tx, events["rows"].as_array().expect("rows"))?;
            let read = EventStore::new().read(tx, &EventFilter::default())?;
            let reads = events["reads"].as_array().expect("reads");
            assert_eq!(read.len(), reads.len());
            for (read, expected) in read.iter().zip(reads) {
                assert_same_json(
                    &stored_json(read)?,
                    expected,
                    "Rust read of a TypeScript row",
                );
            }
            Ok(())
        })
        .await?;
    // The 693-row corpus: the tool asserts the TypeScript read of each row encodes to its input.
    let database = TestDatabase::memory()?;
    database
        .stores
        .transaction(|tx| {
            let fixture: serde_json::Value =
                serde_json::from_str(include_str!("../fixtures/event-compatibility.json"))?;
            let rows = fixture["rows"].as_array().expect("rows");
            insert_event_rows(tx, rows)?;
            let read = EventStore::new().read(tx, &EventFilter::default())?;
            let inputs = fixture["inputs"].as_array().expect("inputs");
            assert_eq!(read.len(), inputs.len());
            for (read, input) in read.iter().zip(inputs) {
                assert_same_json(
                    &serde_json::to_value(&read.event)?,
                    input,
                    "Rust read of a TypeScript row",
                );
                assert_eq!(read.command_id, Some(CommandId("command:compat".into())));
            }
            Ok(())
        })
        .await
}

/// Inputs the TypeScript event decoder rejects. An explicit null on an optional key is the RPC
/// dialect's spelling of `undefined`: Rust must write exactly the row TypeScript wrote for the
/// same event without the nulls (stored dialect). Anything else must be refused, or written in a
/// form the store's own read (the TypeScript-derived persisted shape) accepts; the TypeScript
/// decoder checks those rows through `rows_written_by_rust_for_the_typescript_decoder`.
#[tokio::test]
async fn inputs_the_typescript_decoder_rejects_are_refused_or_written_in_the_stored_dialect()
-> Result<()> {
    let fixture: CompatibilityFixture =
        serde_json::from_str(include_str!("../fixtures/event-compatibility.json"))?;
    let typescript_rows: std::collections::HashMap<String, EventRow> = fixture
        .rows
        .iter()
        .map(|row| (row.event_id.clone(), row.clone()))
        .collect();
    let raw = store_oracle()["events"]["rejected"]
        .as_array()
        .expect("rejected")
        .iter()
        .map(|case| {
            (
                case["reason"].as_str().expect("reason").to_owned(),
                case["event"].clone(),
            )
        });
    let cases: Vec<(String, serde_json::Value)> = fixture
        .rejected
        .into_iter()
        .map(|case| (case.reason, case.input))
        .chain(raw)
        .collect();
    assert!(
        cases.len() > 100,
        "the oracle keeps a sample of every rejected shape"
    );
    let mut compared = 0;
    for (reason, input) in &cases {
        let Ok(event) = serde_json::from_str::<OrchestrationV2DomainEvent>(&input.to_string())
        else {
            continue;
        };
        let id = input["id"].as_str().expect("id").to_owned();
        let expected = typescript_rows.get(&id).cloned();
        let database = TestDatabase::memory()?;
        let outcome = database
            .stores
            .transaction(move |tx| {
                let store = EventStore::new();
                let Ok(_) = store.append(tx, Some(&CommandId("command:compat".into())), &[event])
                else {
                    assert!(store.raw_rows(tx, &EventFilter::default())?.is_empty());
                    return Ok(None);
                };
                let row = store.raw_rows(tx, &EventFilter::default())?.remove(0);
                store.read(tx, &EventFilter::default())?;
                Ok(Some(row))
            })
            .await?;
        // A prompt stored by reference to an earlier corpus row has no lone-append equivalent.
        let expected = expected.filter(|row| !row.payload_json.contains("\"promptSequence\""));
        if let (Some(row), Some(expected), true) =
            (&outcome, &expected, reason.starts_with("explicit null"))
        {
            compared += 1;
            for (column, actual, wanted) in [
                ("payload_json", &row.payload_json, &expected.payload_json),
                ("metadata_json", &row.metadata_json, &expected.metadata_json),
                ("occurred_at", &row.occurred_at, &expected.occurred_at),
                ("actor_kind", &row.actor_kind, &expected.actor_kind),
            ] {
                assert_eq!(actual, wanted, "{id} {column}: {reason}");
            }
        } else if reason.starts_with("explicit null") && expected.is_some() {
            panic!("{id}: {reason}: Rust refused an event TypeScript stores without its nulls");
        }
    }
    assert!(
        compared > 30,
        "only {compared} explicit-null inputs were compared"
    );
    Ok(())
}

/// Every row Rust writes for the corpus, the raw inputs, the rejected inputs it accepts and every
/// labelled event sample (stored and RPC dialect), checked here by Rust's own read. With
/// `T3_STORE_ROWS_OUT` set the rows are written as JSON lines for
/// `node rust/crates/t3-orchestration/tools/verify-event-encoding.ts --verify-rust-rows <file>`,
/// which reads them back through the real TypeScript store.
#[tokio::test]
async fn rows_written_by_rust_for_the_typescript_decoder() -> Result<()> {
    let fixtures = repository_root().join("rust/fixtures");
    let corpus: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/event-compatibility.json"))?;
    let mut inputs: Vec<(String, serde_json::Value)> = Vec::new();
    for (source, values) in [
        ("corpus", corpus["inputs"].clone()),
        (
            "rejected",
            serde_json::json!(
                corpus["rejected"]
                    .as_array()
                    .expect("rejected")
                    .iter()
                    .map(|case| case["input"].clone())
                    .collect::<Vec<_>>()
            ),
        ),
        (
            "raw",
            serde_json::json!(
                store_oracle()["events"]["inputs"]
                    .as_array()
                    .expect("raw")
                    .iter()
                    .map(|case| case["event"].clone())
                    .collect::<Vec<_>>()
            ),
        ),
    ] {
        inputs.extend(
            values
                .as_array()
                .expect("events")
                .iter()
                .map(|event| (source.to_owned(), event.clone())),
        );
    }
    for (directory, label) in [("stored", "stored"), ("samples", "rpc")] {
        let mut files: Vec<_> = std::fs::read_dir(fixtures.join(directory))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("OrchestrationV2DomainEventJson")
                            && name.ends_with(".jsonl")
                    })
            })
            .collect();
        files.sort();
        assert!(
            !files.is_empty(),
            "no {label} event samples under {}",
            fixtures.display()
        );
        for file in files {
            for line in std::fs::read_to_string(&file)?
                .lines()
                .filter(|line| !line.is_empty())
            {
                inputs.push((label.to_owned(), serde_json::from_str(line)?));
            }
        }
    }
    let total = inputs.len();
    let database = TestDatabase::memory()?;
    let (rows, refused) = database
        .stores
        .transaction(move |tx| {
            let store = EventStore::new();
            let mut refused: std::collections::BTreeMap<String, usize> = Default::default();
            for (index, (source, mut input)) in inputs.into_iter().enumerate() {
                input["id"] = format!("rust:{index}").into();
                let appended =
                    serde_json::from_str::<OrchestrationV2DomainEvent>(&input.to_string())
                        .map_err(t3_orchestration::error::Error::from)
                        .and_then(|event| {
                            store.append(tx, Some(&CommandId("command:rust".into())), &[event])
                        });
                if appended.is_err() {
                    assert!(
                        source != "corpus" && source != "raw" && source != "stored",
                        "{source} input {index} was refused: {appended:?}"
                    );
                    *refused.entry(source).or_default() += 1;
                }
            }
            store.read(tx, &EventFilter::default())?;
            Ok((store.raw_rows(tx, &EventFilter::default())?, refused))
        })
        .await?;
    assert!(rows.len() + refused.values().sum::<usize>() == total);
    if let Ok(path) = std::env::var("T3_STORE_ROWS_OUT") {
        let mut out = String::new();
        for row in &rows {
            out.push_str(&serde_json::to_string(row)?);
            out.push('\n');
        }
        std::fs::write(&path, out)?;
        eprintln!(
            "{} Rust rows written to {path}; refused by source: {refused:?}",
            rows.len()
        );
    }
    Ok(())
}
