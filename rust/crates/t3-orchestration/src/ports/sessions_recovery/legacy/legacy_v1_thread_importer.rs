use crate::{
    api::{
        Clock, CommitGuard, CommitOptions, CommitRequest, Decision, EventCommitter, IdGenerator,
        IdKind,
    },
    error::{Error, Result},
    ports::{
        decisions::thread_command_executor::KeyedLanes,
        sessions_recovery::usage_limit_recovery_worker::epoch_millis,
    },
    stores::StoreDatabase,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};
use t3_contracts::{
    base_schemas::{IsoDateTime, ThreadId},
    chat_attachment::ChatAttachment,
    composer_context::OrchestrationMessageContext,
    model_selection::ModelSelection,
    orchestration_v2::{OrchestrationV2AppThread, OrchestrationV2DomainEvent},
    thread_pull_request::{ThreadLinkedPullRequest, ThreadPullRequestLink},
};
use t3_persistence::rusqlite::{Connection, OptionalExtension, Row, params};

const PREFIX: &str = "migration:v1";
const THREAD_FIELDS: &[&str] = &[
    "thread_id",
    "project_id",
    "title",
    "model_selection_json",
    "runtime_mode",
    "interaction_mode",
    "branch",
    "worktree_path",
    "created_at",
    "updated_at",
    "archived_at",
    "settled_override",
    "settled_at",
    "unsettled_at",
    "snoozed_until",
    "snoozed_at",
    "pinned_at",
    "auto_settle_disabled_at",
    "pin_order_key",
    "linked_pull_request_json",
    "branch_pull_request_json",
    "active_order_key",
    "deleted_at",
];
const MISSING_METADATA: &[&str] = &[
    "pinnedAt",
    "pinOrderKey",
    "snoozedUntil",
    "snoozedAt",
    "unsettledAt",
    "linkedPullRequest",
    "pullRequests",
    "branchPullRequest",
    "activeOrderKey",
];
const CREATED_MARKER: &str = "SELECT 1 FROM orchestration_events AS event INDEXED BY orchestration_events_v2_created_threads_idx WHERE event.application_event_version=2 AND event.aggregate_kind='thread' AND event.stream_id=thread.thread_id AND event.event_type='thread.created'";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LegacyV1ImportSummary {
    pub imported_thread_count: usize,
    pub imported_message_count: usize,
}

#[derive(Clone)]
struct LegacyMessage {
    id: String,
    thread: String,
    role: String,
    text: String,
    attachments: Option<String>,
    context: Option<String>,
    streaming: bool,
    created: String,
    updated: String,
    ordinal: u64,
}

pub struct LegacyV1ThreadImporter {
    stores: StoreDatabase,
    sink: Arc<dyn EventCommitter>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    lanes: KeyedLanes,
    confirmed: Mutex<BTreeSet<ThreadId>>,
}

impl LegacyV1ThreadImporter {
    pub fn new(
        stores: StoreDatabase,
        sink: Arc<dyn EventCommitter>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        Self {
            stores,
            sink,
            clock,
            ids,
            lanes: KeyedLanes::default(),
            confirmed: Mutex::new(BTreeSet::new()),
        }
    }

    pub async fn pending_thread_count(&self) -> Result<u64> {
        self.stores.read(|db| {
            let count: i64 = db.query_row(&format!("SELECT COUNT(*) FROM (SELECT thread.thread_id FROM projection_threads AS thread WHERE NOT EXISTS ({CREATED_MARKER}) UNION SELECT thread_id FROM orchestration_v2_legacy_imports WHERE transcript_imported_at IS NULL)"), [], |row| row.get(0))?;
            Ok(count as u64)
        }).await
    }

    pub async fn reconcile_shells(&self) -> Result<LegacyV1ImportSummary> {
        let _lane = self.lanes.acquire(["shells".into()]).await;
        let now = self.clock.now();
        let repairs = self.stores.read(|db| {
            let missing = MISSING_METADATA.iter().map(|field| format!("json_type(projection.payload_json,'$.{field}') IS NULL")).collect::<Vec<_>>().join(" OR ");
            thread_rows(db, &format!("INNER JOIN orchestration_v2_legacy_imports AS legacy_import ON legacy_import.thread_id=thread.thread_id INNER JOIN orchestration_v2_projection_threads AS projection ON projection.thread_id=thread.thread_id WHERE {missing}"), true)
        }).await?;
        let mut summary = LegacyV1ImportSummary::default();
        for row in repairs {
            let Some(raw) = row["payload_json"]
                .as_str()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
            else {
                continue;
            };
            if serde_json::from_value::<OrchestrationV2AppThread>(raw.clone()).is_err() {
                continue;
            }
            let legacy = imported_thread(&row, &now)?;
            let mut repaired = raw.clone();
            for field in MISSING_METADATA
                .iter()
                .copied()
                .chain(["autoSettleDisabledAt"])
            {
                if raw.get(field).is_none() {
                    repaired[field] = legacy[field].clone();
                }
            }
            if raw.get("pullRequests").is_none() {
                repaired["pullRequests"] = if raw.get("linkedPullRequest") == Some(&Value::Null) {
                    json!([])
                } else if legacy["pullRequests"]
                    .as_array()
                    .is_some_and(|links| !links.is_empty())
                {
                    legacy["pullRequests"].clone()
                } else {
                    legacy_links(&repaired["linkedPullRequest"])?
                };
            }
            let thread = text(&row, "thread_id")?;
            let event = event(
                &format!(
                    "{PREFIX}:thread:{thread}:metadata-repair:{}",
                    self.ids.next(IdKind::Event)
                ),
                "thread.metadata-updated",
                thread,
                &now.0,
                repaired,
            )?;
            self.sink
                .commit(CommitRequest {
                    receipt: None,
                    guard: CommitGuard::None,
                    decision: Decision {
                        events: vec![event],
                        ..Default::default()
                    },
                })
                .await?;
            summary.imported_thread_count += 1;
        }
        let rows = self
            .stores
            .read(|db| thread_rows(db, &format!("WHERE NOT EXISTS ({CREATED_MARKER})"), false))
            .await?;
        for row in rows {
            let thread = imported_thread(&row, &now)?;
            let id = text(&row, "thread_id")?.to_owned();
            let lookup = id.clone();
            let previews = self
                .stores
                .read(move |db| messages(db, &lookup, true))
                .await?;
            let mut events = vec![event(
                &format!("{PREFIX}:thread:{id}:created"),
                "thread.created",
                &id,
                text(&thread, "createdAt")?,
                thread.clone(),
            )?];
            for message in &previews {
                events.extend(message_events(message, &now)?);
            }
            events.push(event(
                &format!("{PREFIX}:thread:{id}:shell"),
                "thread.metadata-updated",
                &id,
                text(&thread, "updatedAt")?,
                thread.clone(),
            )?);
            let count = previews.len();
            let source_updated = text(&row, "updated_at")?.to_owned();
            let imported_at = now.0.clone();
            self.sink.commit_prepared(CommitRequest { receipt: None, guard: CommitGuard::None, decision: Decision { events, ..Default::default() } }, CommitOptions::default(), Box::new(move |tx, request| {
                insert_positions(tx, &previews)?;
                tx.execute("INSERT INTO orchestration_v2_legacy_imports(thread_id,source_updated_at,shell_imported_at,transcript_imported_at,imported_message_count,last_error) VALUES (?1,?2,?3,NULL,?4,NULL) ON CONFLICT(thread_id) DO NOTHING", params![id, source_updated, imported_at, count as i64])?;
                Ok(request)
            })).await?;
            summary.imported_thread_count += 1;
            summary.imported_message_count += count;
        }
        Ok(summary)
    }

    pub async fn ensure_transcript(&self, thread: ThreadId) -> Result<LegacyV1ImportSummary> {
        if self
            .confirmed
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(&thread)
        {
            return Ok(LegacyV1ImportSummary::default());
        }
        let _lane = self
            .lanes
            .acquire([format!("transcript:{}", thread.0)])
            .await;
        if self
            .confirmed
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(&thread)
        {
            return Ok(LegacyV1ImportSummary::default());
        }
        let lookup = thread.clone();
        let pending = self.stores.read(move |db| {
            Ok(db.query_row("SELECT transcript_imported_at IS NULL FROM orchestration_v2_legacy_imports WHERE thread_id=?1", [&lookup.0], |row| row.get::<_, bool>(0)).optional()?.unwrap_or(false))
        }).await?;
        if !pending {
            self.confirmed
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(thread);
            return Ok(LegacyV1ImportSummary::default());
        }
        let lookup = thread.clone();
        let (messages, existing) = self.stores.read(move |db| {
            let existing = db.prepare("SELECT event_id FROM orchestration_events INDEXED BY idx_orchestration_events_agent_stream_sequence WHERE application_event_version=2 AND aggregate_kind='thread' AND stream_id=?1 AND event_id LIKE 'migration:v1:message:%'")?.query_map([&lookup.0], |row| row.get::<_, String>(0))?.collect::<std::result::Result<BTreeSet<_>, _>>()?;
            Ok((messages(db, &lookup.0, false)?, existing))
        }).await?;
        let total = messages.len();
        let missing = messages
            .into_iter()
            .filter(|message| !existing.contains(&format!("{PREFIX}:message:{}", message.id)))
            .collect::<Vec<_>>();
        let now = self.clock.now();
        for batch in missing.chunks(50) {
            let events = batch
                .iter()
                .map(|message| message_events(message, &now))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .collect();
            let batch = batch.to_vec();
            self.sink
                .commit_prepared(
                    CommitRequest {
                        receipt: None,
                        guard: CommitGuard::None,
                        decision: Decision {
                            events,
                            ..Default::default()
                        },
                    },
                    CommitOptions::default(),
                    Box::new(move |tx, request| {
                        insert_positions(tx, &batch)?;
                        Ok(request)
                    }),
                )
                .await?;
        }
        let at = self.clock.now();
        let lookup = thread.clone();
        self.stores.transaction(move |tx| {
            tx.execute("UPDATE orchestration_v2_legacy_imports SET transcript_imported_at=?2,imported_message_count=?3,last_error=NULL WHERE thread_id=?1", params![lookup.0, at.0, total as i64])?;
            Ok(())
        }).await?;
        self.confirmed
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(thread);
        Ok(LegacyV1ImportSummary {
            imported_thread_count: 1,
            imported_message_count: missing.len(),
        })
    }

    pub async fn import_pending_transcripts(&self) -> Result<LegacyV1ImportSummary> {
        let rows = match self.stores.read(|db| {
            Ok(db.prepare("SELECT thread_id FROM orchestration_v2_legacy_imports WHERE transcript_imported_at IS NULL ORDER BY shell_imported_at,thread_id")?.query_map([], |row| row.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?)
        }).await {
            Ok(rows) => rows,
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(_) => return Ok(LegacyV1ImportSummary::default()),
        };
        let mut summary = LegacyV1ImportSummary::default();
        for thread in rows {
            match self.ensure_transcript(ThreadId(thread.clone())).await {
                Ok(result) => {
                    summary.imported_thread_count += result.imported_thread_count;
                    summary.imported_message_count += result.imported_message_count;
                }
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(_) => {
                    let result = self.stores.transaction(move |tx| {
                        tx.execute("UPDATE orchestration_v2_legacy_imports SET last_error='Transcript hydration failed; retry on next open.' WHERE thread_id=?1", [thread])?;
                        Ok(())
                    }).await;
                    if matches!(result, Err(Error::Cancelled)) {
                        return Err(Error::Cancelled);
                    }
                }
            }
        }
        Ok(summary)
    }
}

fn thread_rows(db: &Connection, predicate: &str, repair: bool) -> Result<Vec<Value>> {
    let fields = THREAD_FIELDS
        .iter()
        .map(|field| format!("thread.{field}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT {fields}, (SELECT json_group_array(json_object('host',pr.host,'repository',pr.repository,'number',pr.number,'url',pr.url,'source',pr.source,'linkedAt',pr.linked_at,'snapshot',json(pr.snapshot_json),'stack',json(pr.stack_json))) FROM projection_thread_pull_requests AS pr WHERE pr.thread_id=thread.thread_id) AS pull_requests_json {} FROM projection_threads AS thread {predicate} ORDER BY thread.created_at,thread.thread_id",
        if repair {
            ",projection.payload_json"
        } else {
            ""
        }
    );
    Ok(db
        .prepare(&sql)?
        .query_map([], |row| {
            let mut value = serde_json::Map::new();
            for (index, field) in THREAD_FIELDS
                .iter()
                .chain([&"pull_requests_json"])
                .enumerate()
            {
                value.insert(
                    (*field).into(),
                    row.get::<_, Option<String>>(index)?
                        .map_or(Value::Null, Value::String),
                );
            }
            if repair {
                value.insert(
                    "payload_json".into(),
                    Value::String(row.get(THREAD_FIELDS.len() + 1)?),
                );
            }
            Ok(Value::Object(value))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?)
}

fn messages(db: &Connection, thread: &str, shell: bool) -> Result<Vec<LegacyMessage>> {
    fn decode(row: &Row<'_>) -> t3_persistence::rusqlite::Result<LegacyMessage> {
        Ok(LegacyMessage {
            id: row.get(0)?,
            thread: row.get(1)?,
            role: row.get(2)?,
            text: row.get(3)?,
            attachments: row.get(4)?,
            context: row.get(5)?,
            streaming: row.get::<_, i64>(6)? == 1,
            created: row.get(7)?,
            updated: row.get(8)?,
            ordinal: row.get::<_, i64>(9)? as u64,
        })
    }
    if shell {
        let select = "SELECT message.message_id,message.thread_id,message.role,message.text,message.attachments_json,message.context_json,message.is_streaming,message.created_at,message.updated_at,(SELECT COUNT(*) FROM projection_thread_messages AS earlier WHERE earlier.thread_id=message.thread_id AND earlier.role IN ('user','assistant') AND (earlier.created_at<message.created_at OR (earlier.created_at=message.created_at AND earlier.message_id<=message.message_id))) AS ordinal FROM projection_thread_messages AS message WHERE message.thread_id=?1";
        let latest = db.query_row(&format!("{select} AND message.role IN ('user','assistant') ORDER BY message.created_at DESC,message.message_id DESC LIMIT 1"),[thread],decode).optional()?;
        let user = db.query_row(&format!("{select} AND message.role='user' ORDER BY message.created_at DESC,message.message_id DESC LIMIT 1"),[thread],decode).optional()?;
        let mut selected = Vec::new();
        if let Some(user) = user {
            selected.push(user);
        }
        if let Some(latest) = latest
            && selected.iter().all(|message| message.id != latest.id)
        {
            selected.push(latest);
        }
        return Ok(selected);
    }
    let select = "SELECT message_id,thread_id,role,text,attachments_json,context_json,is_streaming,created_at,updated_at,ROW_NUMBER() OVER (ORDER BY created_at,message_id) AS ordinal FROM projection_thread_messages WHERE thread_id=?1 AND role IN ('user','assistant')";
    Ok(db
        .prepare(&format!("{select} ORDER BY created_at,message_id"))?
        .query_map([thread], decode)?
        .collect::<std::result::Result<Vec<_>, _>>()?)
}

fn insert_positions(db: &Connection, messages: &[LegacyMessage]) -> Result<()> {
    for message in messages {
        db.execute("INSERT INTO orchestration_v2_turn_item_positions(thread_id,turn_item_id,ordinal) VALUES (?1,?2,?3) ON CONFLICT(thread_id,turn_item_id) DO NOTHING", params![message.thread, format!("{PREFIX}:turn-item:{}", message.id), message.ordinal as i64])?;
    }
    Ok(())
}

fn text<'a>(row: &'a Value, key: &str) -> Result<&'a str> {
    row[key].as_str().ok_or_else(|| Error::Decode {
        entity: "legacy row",
        detail: format!("missing {key}"),
    })
}

fn decoded<T: serde::de::DeserializeOwned + serde::Serialize>(value: &Value) -> Option<Value> {
    value
        .as_str()
        .and_then(|value| serde_json::from_str::<T>(value).ok())
        .and_then(|value| t3_contracts::stored::to_value(&value).ok())
}

fn trimmed(value: &Value) -> Value {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map_or(Value::Null, |value| json!(value))
}

fn imported_thread(row: &Value, now: &IsoDateTime) -> Result<Value> {
    let created = date_time_or(
        &row["created_at"],
        &date_time_or(&row["updated_at"], &now.0),
    );
    let updated = date_time_or(&row["updated_at"], &created);
    let selection = decoded::<ModelSelection>(&row["model_selection_json"]).unwrap_or_else(
        || json!({"instanceId":"codex","model":t3_contracts::model::DEFAULT_MODEL}),
    );
    let linked =
        decoded::<ThreadLinkedPullRequest>(&row["linked_pull_request_json"]).unwrap_or(Value::Null);
    let mut links = decoded::<Vec<ThreadPullRequestLink>>(&row["pull_requests_json"])
        .unwrap_or_else(|| json!([]));
    for legacy in legacy_links(&linked)?.as_array().into_iter().flatten() {
        if !links.as_array().is_some_and(|links| {
            links.iter().any(|link| {
                link["host"].as_str().map(str::to_lowercase)
                    == legacy["host"].as_str().map(str::to_lowercase)
                    && link["repository"].as_str().map(|s| s.trim().to_lowercase())
                        == legacy["repository"]
                            .as_str()
                            .map(|s| s.trim().to_lowercase())
                    && link["number"] == legacy["number"]
            })
        }) && let Some(links) = links.as_array_mut()
        {
            links.push(legacy.clone());
        }
    }
    let mut thread = json!({
        "createdBy":"system","creationSource":"server","id":row["thread_id"],"projectId":row["project_id"],
        "title":if row["title"].as_str().is_none_or(|title| title.trim().is_empty()) { json!("Untitled thread") } else { row["title"].clone() },
        "providerInstanceId":selection["instanceId"],"modelSelection":selection,
        "runtimeMode":row["runtime_mode"].as_str().filter(|value| matches!(*value, "approval-required" | "auto-accept-edits" | "auto" | "full-access")).unwrap_or("full-access"),
        "interactionMode":if row["interaction_mode"] == "plan" { "plan" } else { "default" },
        "branch":trimmed(&row["branch"]),"worktreePath":trimmed(&row["worktree_path"]),"linkedPullRequest":linked,
        "pullRequests":links,"branchPullRequest":decoded::<ThreadLinkedPullRequest>(&row["branch_pull_request_json"]),
        "activeOrderKey":trimmed(&row["active_order_key"]),"activeProviderThreadId":null,"historyOrigin":"v1_import",
        "lineage":{"parentThreadId":null,"relationshipToParent":null,"rootThreadId":row["thread_id"]},"forkedFrom":null,
        "createdAt":created,"updatedAt":updated,"settledOverride":match row["settled_override"].as_str() { Some(value @ ("settled" | "active")) => json!(value), _ => Value::Null },
        "pinOrderKey":trimmed(&row["pin_order_key"]),"lastVisitedAt":null,
    });
    for (source, target) in [
        ("archived_at", "archivedAt"),
        ("settled_at", "settledAt"),
        ("unsettled_at", "unsettledAt"),
        ("snoozed_until", "snoozedUntil"),
        ("snoozed_at", "snoozedAt"),
        ("pinned_at", "pinnedAt"),
        ("auto_settle_disabled_at", "autoSettleDisabledAt"),
        ("deleted_at", "deletedAt"),
    ] {
        thread[target] = if row[source].is_null() {
            Value::Null
        } else {
            json!(date_time_or(&row[source], &updated))
        };
    }
    Ok(t3_contracts::stored::to_value(&serde_json::from_value::<
        OrchestrationV2AppThread,
    >(thread)?)?)
}

fn legacy_links(linked: &Value) -> Result<Value> {
    if linked.is_null() {
        return Ok(json!([]));
    }
    let url = text(linked, "url")?;
    let authority = url
        .split_once("://")
        .and_then(|(_, rest)| rest.split(['/', '?', '#']).next())
        .unwrap_or("unknown");
    let host = authority
        .rsplit('@')
        .next()
        .unwrap_or("unknown")
        .split(':')
        .next()
        .unwrap_or("unknown")
        .trim()
        .to_lowercase();
    let link = json!({"host":if host.is_empty() { "unknown" } else { &host },"repository":text(linked,"repository")?.trim().to_lowercase(),"number":linked["number"],"url":url,"source":"manual","linkedAt":"1970-01-01T00:00:00.000Z","snapshot":null,"stack":null});
    Ok(json!([serde_json::from_value::<ThreadPullRequestLink>(
        link
    )?]))
}

fn event(
    id: &str,
    kind: &str,
    thread: &str,
    at: &str,
    payload: Value,
) -> Result<OrchestrationV2DomainEvent> {
    let mut value =
        json!({"id":id,"type":kind,"threadId":thread,"occurredAt":at,"payload":payload});
    if kind.starts_with("thread.") {
        value["providerInstanceId"] = value["payload"]["providerInstanceId"].clone();
    }
    Ok(serde_json::from_value(value)?)
}

fn message_events(
    row: &LegacyMessage,
    now: &IsoDateTime,
) -> Result<Vec<OrchestrationV2DomainEvent>> {
    let created = date_time_or(
        &json!(row.created),
        &date_time_or(&json!(row.updated), &now.0),
    );
    let updated = date_time_or(&json!(row.updated), &created);
    let attachments = row
        .attachments
        .as_ref()
        .and_then(|value| serde_json::from_str::<Vec<ChatAttachment>>(value).ok())
        .unwrap_or_default();
    let context = row
        .context
        .as_ref()
        .and_then(|value| serde_json::from_str::<OrchestrationMessageContext>(value).ok());
    let mut message = json!({"createdBy":if row.role == "user" { "user" } else { "agent" },"creationSource":"server","id":row.id,"threadId":row.thread,"runId":null,"nodeId":null,"role":row.role,"text":row.text,"attachments":attachments,"streaming":false,"createdAt":created,"updatedAt":updated});
    let mut item = json!({"id":format!("{PREFIX}:turn-item:{}",row.id),"threadId":row.thread,"runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":row.ordinal,"status":if row.streaming { "interrupted" } else { "completed" },"title":null,"startedAt":created,"completedAt":updated,"updatedAt":updated,"messageId":row.id,"text":row.text});
    if let Some(context) = context {
        message["context"] = json!(context);
        item["context"] = json!(context);
    }
    if row.role == "user" {
        item["createdBy"] = json!("user");
        item["creationSource"] = json!("server");
        item["type"] = json!("user_message");
        item["inputIntent"] = json!("turn_start");
        item["attachments"] = json!(attachments);
    } else {
        item["type"] = json!("assistant_message");
        item["streaming"] = json!(false);
    }
    Ok(vec![
        event(
            &format!("{PREFIX}:message:{}", row.id),
            "message.updated",
            &row.thread,
            &updated,
            message,
        )?,
        event(
            &format!("{PREFIX}:turn-item:{}", row.id),
            "turn-item.updated",
            &row.thread,
            &updated,
            item,
        )?,
    ])
}

fn date_time_or(value: &Value, fallback: &str) -> String {
    let Some(millis) = value.as_str().and_then(epoch_millis) else {
        return fallback.into();
    };
    let days = millis.div_euclid(86_400_000);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let year = if (0..=9999).contains(&year) {
        format!("{year:04}")
    } else {
        format!("{year:+07}")
    };
    let time = millis.rem_euclid(86_400_000);
    format!(
        "{year}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        time / 3_600_000,
        time / 60_000 % 60,
        time / 1_000 % 60,
        time % 1_000
    )
}
