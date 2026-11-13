use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
};
use t3_contracts::{
    application_event::{ApplicationProjectEvent, ApplicationStoredEvent},
    base_schemas::{CommandId, ThreadId},
    orchestration_v2::{OrchestrationV2DomainEvent, OrchestrationV2StoredEvent},
    wire::TaggedUnion,
};
use t3_persistence::rusqlite::{
    CachedStatement, Connection, Row, params, params_from_iter, types::Value as SqlValue,
};

#[path = "event_json.rs"]
mod event_json;
pub use event_json::canonical_date;
pub(crate) use event_json::{decode as decode_json, encode_json, parse as parse_json};

pub const EVENT_LOG_PRUNED_THROUGH: &str = "event-log-pruned-through";
pub const READ_PAGE_SIZE: i64 = 500;
/// `Number.MAX_SAFE_INTEGER`, the TypeScript store's bound for an unlimited read.
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
const PROMPT_HINT_CAPACITY: usize = 4096;
/// Store statements reused on every append, claim and projection write.
const STATEMENT_CACHE_CAPACITY: usize = 64;
const METADATA_KEYS: [&str; 5] = [
    "runId",
    "nodeId",
    "driver",
    "providerInstanceId",
    "rawEventId",
];
const COLUMNS: &str = "sequence, event_id, aggregate_kind, stream_id, stream_version, event_type, \
    occurred_at, command_id, causation_event_id, correlation_id, actor_kind, payload_json, \
    metadata_json, application_event_version";
const RESTORED: &str = "CASE
    WHEN event_type IN ('subagent.updated', 'turn-item.updated')
        AND json_type(payload_json, '$.promptSequence') = 'integer'
    THEN json_set(
        json_remove(payload_json, '$.promptSequence'),
        '$.prompt',
        (SELECT json_extract(anchor.payload_json, '$.prompt')
         FROM orchestration_events AS anchor
         WHERE anchor.sequence = json_extract(orchestration_events.payload_json, '$.promptSequence'))
    )
    ELSE payload_json
END";
const ANCHORED_PAYLOAD: &str = "SELECT CASE WHEN EXISTS (
        SELECT 1 FROM orchestration_events
        WHERE sequence = ?1
            AND json_type(payload_json, '$.promptSequence') IS NULL
            AND json_extract(payload_json, '$.prompt') = ?2
    )
    THEN json_set(?3, '$.prompt', '', '$.promptSequence', CAST(?1 AS INTEGER))
    ELSE ?3 END";
const INSERT_THREAD_EVENT: &str = "INSERT INTO orchestration_events (
        event_id, aggregate_kind, stream_id, stream_version, event_type, occurred_at,
        command_id, causation_event_id, correlation_id, actor_kind, payload_json,
        metadata_json, application_event_version
    ) VALUES (
        ?1, 'thread', ?2,
        COALESCE((SELECT MAX(stream_version) + 1 FROM orchestration_events
                  WHERE aggregate_kind = 'thread' AND stream_id = ?2), 0),
        ?3, ?4, ?5, NULL, ?5, ?6, ?7, ?8, 2
    )
    RETURNING sequence, json_type(payload_json, '$.promptSequence') IS NULL";
const INSERT_PROJECT_EVENT: &str = "INSERT INTO orchestration_events (
        event_id, aggregate_kind, stream_id, stream_version, event_type, occurred_at,
        command_id, causation_event_id, correlation_id, actor_kind, payload_json,
        metadata_json, application_event_version
    ) VALUES (
        ?1, 'project', ?2,
        COALESCE((SELECT MAX(stream_version) + 1 FROM orchestration_events
                  WHERE aggregate_kind = 'project' AND stream_id = ?2), 0),
        ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 2
    )
    RETURNING sequence";

/// Store timestamps are `DateTime.formatIso` text in TypeScript, and SQL compares them as text,
/// so a caller's timestamp is canonicalised before it is written or compared.
pub(crate) fn timestamp(field: &'static str, value: &str) -> Result<String> {
    canonical_date(value).ok_or_else(|| Error::Decode {
        entity: "timestamp",
        detail: format!("{field} is not a valid date: {value:?}"),
    })
}

/// A statement compiled once per connection. The default cache of 16 is smaller than the set
/// of fixed store statements, so it is raised before use.
pub(crate) fn statement<'c>(db: &'c Connection, sql: &str) -> Result<CachedStatement<'c>> {
    db.set_prepared_statement_cache_capacity(STATEMENT_CACHE_CAPACITY);
    Ok(db.prepare_cached(sql)?)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventRow {
    pub sequence: i64,
    pub event_id: String,
    pub aggregate_kind: String,
    pub stream_id: String,
    pub stream_version: i64,
    pub event_type: String,
    pub occurred_at: String,
    pub command_id: Option<String>,
    pub causation_event_id: Option<String>,
    pub correlation_id: Option<String>,
    pub actor_kind: String,
    pub payload_json: String,
    pub metadata_json: String,
    pub application_event_version: i64,
}
impl EventRow {
    fn from_row(row: &Row<'_>) -> t3_persistence::rusqlite::Result<Self> {
        Ok(Self {
            sequence: row.get(0)?,
            event_id: row.get(1)?,
            aggregate_kind: row.get(2)?,
            stream_id: row.get(3)?,
            stream_version: row.get(4)?,
            event_type: row.get(5)?,
            occurred_at: row.get(6)?,
            command_id: row.get(7)?,
            causation_event_id: row.get(8)?,
            correlation_id: row.get(9)?,
            actor_kind: row.get(10)?,
            payload_json: row.get(11)?,
            metadata_json: row.get(12)?,
            application_event_version: row.get(13)?,
        })
    }
    /// Mirrors `rowToV2StoredEvent`: the row is checked against the persisted event schema
    /// (as the TypeScript decoder checks it) before typed decoding.
    pub fn decode_agent(&self) -> Result<OrchestrationV2StoredEvent> {
        let mut event = Map::new();
        event.insert("id".into(), self.event_id.clone().into());
        event.insert("threadId".into(), self.stream_id.clone().into());
        event.insert("type".into(), self.event_type.clone().into());
        event.insert("occurredAt".into(), self.occurred_at.clone().into());
        event.insert("payload".into(), parse_json(&self.payload_json)?);
        let metadata = parse_json(&self.metadata_json)?;
        if let Some(metadata) = metadata.as_object() {
            validate_metadata(metadata)?;
        }
        for key in METADATA_KEYS {
            if let Some(value) = metadata.get(key) {
                event.insert(key.into(), value.clone());
            }
        }
        let mut event = Value::Object(event);
        event_json::persisted_event(&mut event)?;
        Ok(OrchestrationV2StoredEvent {
            sequence: self.sequence()?,
            command_id: decode_json(self.command_id.clone().into())?,
            event: decode_json(event)?,
        })
    }
    pub fn decode_application(&self) -> Result<ApplicationStoredEvent> {
        if self.aggregate_kind != "project" {
            return Ok(ApplicationStoredEvent::Orchestration(Box::new(
                self.decode_agent()?,
            )));
        }
        let mut event = Map::new();
        event.insert("sequence".into(), self.sequence.into());
        event.insert("eventId".into(), self.event_id.clone().into());
        event.insert("type".into(), self.event_type.clone().into());
        event.insert("aggregateKind".into(), "project".into());
        event.insert("aggregateId".into(), self.stream_id.clone().into());
        event.insert("occurredAt".into(), self.occurred_at.clone().into());
        event.insert("commandId".into(), self.command_id.clone().into());
        event.insert(
            "causationEventId".into(),
            self.causation_event_id.clone().into(),
        );
        event.insert("correlationId".into(), self.correlation_id.clone().into());
        event.insert("payload".into(), parse_json(&self.payload_json)?);
        event.insert("metadata".into(), parse_json(&self.metadata_json)?);
        Ok(ApplicationStoredEvent::Project(Box::new(decode_json(
            Value::Object(event),
        )?)))
    }
    fn sequence(&self) -> Result<u64> {
        u64::try_from(self.sequence).map_err(|_| Error::Decode {
            entity: "event",
            detail: format!("negative sequence {}", self.sequence),
        })
    }
    fn known(&self) -> bool {
        if self.aggregate_kind == "project" {
            matches!(
                self.event_type.as_str(),
                "project.created" | "project.meta-updated" | "project.deleted"
            )
        } else {
            OrchestrationV2DomainEvent::KNOWN_TAGS.contains(&self.event_type.as_str())
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    pub after_sequence: i64,
    pub through_sequence: Option<i64>,
    pub thread_id: Option<ThreadId>,
    pub command_id: Option<CommandId>,
    pub event_type: Option<String>,
    pub limit: Option<i64>,
    pub skip_unknown_event_types: bool,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplayStats {
    pub event_count: i64,
    pub raw_payload_bytes: i64,
    pub has_create_event: bool,
}

pub struct EventPages<T> {
    database: super::StoreDatabase,
    filter: EventFilter,
    remaining: i64,
    agents_only: bool,
    finished: bool,
    decode: fn(&EventRow) -> Result<T>,
}
impl<T> EventPages<T> {
    pub async fn next_page(&mut self) -> Result<Option<Vec<T>>> {
        while !self.finished {
            let page_limit = self.remaining.min(READ_PAGE_SIZE);
            if page_limit == 0 {
                self.finished = true;
                return Ok(None);
            }
            let mut filter = self.filter.clone();
            filter.limit = Some(page_limit);
            let agents_only = self.agents_only;
            let rows = self
                .database
                .read(move |db| EventStore::new().rows(db, &filter, agents_only, true))
                .await;
            let rows = match rows {
                Ok(rows) => rows,
                Err(error) => {
                    self.finished = true;
                    return Err(error);
                }
            };
            self.remaining -= rows.len() as i64;
            let Some(last) = rows.last() else {
                self.finished = true;
                return Ok(None);
            };
            self.filter.after_sequence = last.sequence;
            self.finished = rows.len() < page_limit as usize
                || self.remaining == 0
                || self
                    .filter
                    .through_sequence
                    .is_some_and(|through| last.sequence >= through);
            let page = rows
                .iter()
                .filter(|row| !self.filter.skip_unknown_event_types || row.known())
                .map(self.decode)
                .collect::<Result<Vec<_>>>();
            match page {
                Ok(page) if !page.is_empty() => return Ok(Some(page)),
                Ok(_) => {}
                Err(error) => {
                    self.finished = true;
                    return Err(error);
                }
            }
        }
        Ok(None)
    }
}

#[derive(Default)]
struct PromptHints {
    entries: HashMap<String, (i64, u64)>,
    order: VecDeque<(String, u64)>,
    stamp: u64,
}
impl PromptHints {
    fn get(&self, key: &str) -> Option<i64> {
        self.entries.get(key).map(|(sequence, _)| *sequence)
    }
    /// `Map.delete` then `Map.set`: a refreshed key moves to the newest position.
    fn refresh(&mut self, key: String, sequence: i64) {
        self.stamp += 1;
        self.entries.insert(key.clone(), (sequence, self.stamp));
        self.order.push_back((key, self.stamp));
        while self.entries.len() > PROMPT_HINT_CAPACITY {
            let Some((oldest, stamp)) = self.order.pop_front() else {
                break;
            };
            if self
                .entries
                .get(&oldest)
                .is_some_and(|(_, live)| *live == stamp)
            {
                self.entries.remove(&oldest);
            }
        }
        if self.order.len() > 2 * PROMPT_HINT_CAPACITY {
            let entries = &self.entries;
            self.order
                .retain(|(key, stamp)| entries.get(key).is_some_and(|(_, live)| live == stamp));
        }
    }
}

#[derive(Default)]
pub struct EventStore {
    prompt_hints: Mutex<PromptHints>,
}
impl EventStore {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn read_paged(
        &self,
        database: super::StoreDatabase,
        filter: EventFilter,
    ) -> EventPages<OrchestrationV2StoredEvent> {
        self.pages(database, filter, true, EventRow::decode_agent)
    }
    pub fn read_application_paged(
        &self,
        database: super::StoreDatabase,
        filter: EventFilter,
    ) -> EventPages<ApplicationStoredEvent> {
        self.pages(database, filter, false, EventRow::decode_application)
    }
    fn pages<T>(
        &self,
        database: super::StoreDatabase,
        filter: EventFilter,
        agents_only: bool,
        decode: fn(&EventRow) -> Result<T>,
    ) -> EventPages<T> {
        EventPages {
            remaining: filter.limit.unwrap_or(MAX_SAFE_INTEGER).max(0),
            database,
            filter,
            agents_only,
            finished: false,
            decode,
        }
    }
    /// Returns the caller's events with their sequences, as the TypeScript store does. When the
    /// persisted form differs (a date was canonicalised, a number has a different JavaScript
    /// value, a key the TypeScript schema lacks was dropped), the returned event is the persisted
    /// one, so it equals what a later read returns.
    pub fn append(
        &self,
        db: &Connection,
        command_id: Option<&CommandId>,
        events: &[OrchestrationV2DomainEvent],
    ) -> Result<Vec<OrchestrationV2StoredEvent>> {
        events
            .iter()
            .map(|event| self.append_one(db, command_id, event))
            .collect()
    }
    fn append_one(
        &self,
        db: &Connection,
        command_id: Option<&CommandId>,
        event: &OrchestrationV2DomainEvent,
    ) -> Result<OrchestrationV2StoredEvent> {
        let original = t3_contracts::stored::to_value(event)?;
        let mut value = original.clone();
        event_json::persisted_event(&mut value)?;
        let mut metadata = Map::new();
        for key in METADATA_KEYS {
            if let Some(field) = value.get(key) {
                metadata.insert(key.into(), field.clone());
            }
        }
        validate_metadata(&metadata)?;
        let event_type = string(&value, "type")?;
        let payload = &value["payload"];
        let key = if event_type == "subagent.updated"
            || (event_type == "turn-item.updated" && payload["type"] == "subagent")
        {
            Some(format!("{event_type}:{}", string(payload, "id")?))
        } else {
            None
        };
        let payload_json = event_json::encode_json(payload)?;
        let mut hints = key.as_ref().map(|_| {
            self.prompt_hints
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        });
        let hint = key
            .as_deref()
            .zip(hints.as_deref())
            .and_then(|(key, hints)| hints.get(key));
        // A rolled-back hint must be verified in SQL before it can anchor a future row.
        let stored_payload = if let Some(hint) = hint {
            statement(db, ANCHORED_PAYLOAD)?.query_row(
                params![hint, payload["prompt"].as_str(), payload_json],
                |row| row.get::<_, String>(0),
            )?
        } else {
            payload_json
        };
        let command = command_id.map(|id| id.0.as_str());
        let (sequence, stores_full_prompt) = statement(db, INSERT_THREAD_EVENT)?.query_row(
            params![
                string(&value, "id")?,
                string(&value, "threadId")?,
                event_type,
                string(&value, "occurredAt")?,
                command,
                if value.get("rawEventId").is_some() {
                    "provider"
                } else {
                    "server"
                },
                stored_payload,
                event_json::encode_json(&Value::Object(metadata))?,
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, bool>(1)?)),
        )?;
        if let (Some(key), Some(hints)) = (key, hints.as_mut()) {
            hints.refresh(
                key,
                if stores_full_prompt {
                    sequence
                } else {
                    hint.unwrap_or(sequence)
                },
            );
        }
        drop(hints);
        event_json::normalize_numbers(&mut value);
        let event = if value == original {
            event.clone()
        } else {
            event_json::decode(value)?
        };
        Ok(OrchestrationV2StoredEvent {
            sequence: u64::try_from(sequence).map_err(|_| Error::Decode {
                entity: "event",
                detail: format!("negative sequence {sequence}"),
            })?,
            command_id: command_id.cloned(),
            event,
        })
    }
    /// The input's sequence is ignored; SQLite assigns the application sequence.
    pub fn append_project(
        &self,
        db: &Connection,
        event: &ApplicationProjectEvent,
    ) -> Result<ApplicationProjectEvent> {
        let value = t3_contracts::stored::to_value(event)?;
        let command = value["commandId"].as_str();
        let metadata = &value["metadata"];
        let actor = if command.is_some_and(|id| id.starts_with("provider:")) {
            "provider"
        } else if command.is_some_and(|id| id.starts_with("server:")) {
            "server"
        } else if ["providerTurnId", "providerItemId", "adapterKey"]
            .iter()
            .any(|key| metadata.get(key).is_some())
        {
            "provider"
        } else if command.is_none() {
            "server"
        } else {
            "client"
        };
        let sequence = statement(db, INSERT_PROJECT_EVENT)?.query_row(
            params![
                string(&value, "eventId")?,
                string(&value, "aggregateId")?,
                string(&value, "type")?,
                string(&value, "occurredAt")?,
                command,
                value["causationEventId"].as_str(),
                value["correlationId"].as_str(),
                actor,
                encode_json(&value["payload"])?,
                encode_json(metadata)?,
            ],
            |row| row.get::<_, i64>(0),
        )?;
        let sequence = u64::try_from(sequence).map_err(|_| Error::Decode {
            entity: "event",
            detail: format!("negative sequence {sequence}"),
        })?;
        let mut stored = event.clone();
        match &mut stored {
            ApplicationProjectEvent::Created(event) => event.sequence = sequence,
            ApplicationProjectEvent::MetaUpdated(event) => event.sequence = sequence,
            ApplicationProjectEvent::Deleted(event) => event.sequence = sequence,
        }
        Ok(stored)
    }
    /// Finite read for callers that already hold a transaction. Each statement reads at most
    /// `READ_PAGE_SIZE` raw rows; callers that need bounded memory use `read_paged`.
    pub fn read(
        &self,
        db: &Connection,
        filter: &EventFilter,
    ) -> Result<Vec<OrchestrationV2StoredEvent>> {
        self.read_all(db, filter, true, EventRow::decode_agent)
    }
    pub fn read_application(
        &self,
        db: &Connection,
        filter: &EventFilter,
    ) -> Result<Vec<ApplicationStoredEvent>> {
        self.read_all(db, filter, false, EventRow::decode_application)
    }
    fn read_all<T>(
        &self,
        db: &Connection,
        filter: &EventFilter,
        agents_only: bool,
        decode: fn(&EventRow) -> Result<T>,
    ) -> Result<Vec<T>> {
        let mut remaining = filter.limit.unwrap_or(MAX_SAFE_INTEGER).max(0);
        let mut page = filter.clone();
        let mut events = Vec::new();
        while remaining > 0 {
            let page_limit = remaining.min(READ_PAGE_SIZE);
            page.limit = Some(page_limit);
            let rows = self.rows(db, &page, agents_only, true)?;
            remaining -= rows.len() as i64;
            for row in rows
                .iter()
                .filter(|row| !filter.skip_unknown_event_types || row.known())
            {
                events.push(decode(row)?);
            }
            match rows.last() {
                Some(last) if rows.len() as i64 == page_limit => {
                    page.after_sequence = last.sequence
                }
                _ => break,
            }
        }
        Ok(events)
    }
    pub fn raw_rows(&self, db: &Connection, filter: &EventFilter) -> Result<Vec<EventRow>> {
        self.rows(db, filter, false, false)
    }
    fn rows(
        &self,
        db: &Connection,
        f: &EventFilter,
        agents_only: bool,
        restore: bool,
    ) -> Result<Vec<EventRow>> {
        let (sql, args) = Self::row_query(f, agents_only, restore);
        Ok(statement(db, &sql)?
            .query_map(params_from_iter(args), EventRow::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }
    /// Optional filters add predicates rather than binding NULL so each variant keeps its index;
    /// the cached statement per SQL text makes the variants compile once.
    fn row_query(f: &EventFilter, agents_only: bool, restore: bool) -> (String, Vec<SqlValue>) {
        let columns = if restore {
            COLUMNS.replace("payload_json", &format!("{RESTORED} AS payload_json"))
        } else {
            COLUMNS.to_owned()
        };
        let index = if f.command_id.is_some() {
            " INDEXED BY idx_orch_events_command_id"
        } else if agents_only {
            ""
        } else {
            " INDEXED BY idx_orchestration_events_application_high_water"
        };
        let scope = if agents_only {
            "application_event_version = 2 AND aggregate_kind = 'thread'"
        } else {
            "aggregate_kind = 'project' OR (application_event_version = 2 AND aggregate_kind = 'thread')"
        };
        let mut sql = format!(
            "SELECT {columns} FROM orchestration_events{index} \
             WHERE sequence > ? AND sequence <= ? AND ({scope})"
        );
        let mut args: Vec<SqlValue> = vec![
            f.after_sequence.into(),
            f.through_sequence.unwrap_or(MAX_SAFE_INTEGER).into(),
        ];
        for (column, value) in [
            ("stream_id", f.thread_id.as_ref().map(|id| &id.0)),
            ("command_id", f.command_id.as_ref().map(|id| &id.0)),
            ("event_type", f.event_type.as_ref()),
        ] {
            if let Some(value) = value {
                sql.push_str(&format!(" AND {column} = ?"));
                args.push(value.clone().into());
            }
        }
        sql.push_str(" ORDER BY sequence ASC LIMIT ?");
        args.push(f.limit.unwrap_or(MAX_SAFE_INTEGER).max(0).into());
        (sql, args)
    }
    pub fn latest_agent_sequence(
        &self,
        db: &Connection,
        thread_id: Option<&ThreadId>,
    ) -> Result<i64> {
        const SCOPED: &str = "SELECT COALESCE(MAX(sequence), 0) FROM orchestration_events \
            INDEXED BY idx_orchestration_events_agent_stream_sequence \
            WHERE application_event_version = 2 AND aggregate_kind = 'thread' AND stream_id = ?1";
        const ALL: &str = "SELECT COALESCE(MAX(sequence), 0) FROM orchestration_events \
            INDEXED BY idx_orchestration_events_application_sequence \
            WHERE application_event_version = 2 AND aggregate_kind = 'thread'";
        Ok(match thread_id {
            Some(id) => statement(db, SCOPED)?.query_row([&id.0], |row| row.get(0))?,
            None => statement(db, ALL)?.query_row([], |row| row.get(0))?,
        })
    }
    pub fn latest_application_sequence(&self, db: &Connection) -> Result<i64> {
        const SQL: &str = "SELECT COALESCE(MAX(sequence), 0) FROM orchestration_events \
            INDEXED BY idx_orchestration_events_application_high_water \
            WHERE aggregate_kind = 'project' \
                OR (application_event_version = 2 AND aggregate_kind = 'thread')";
        Ok(statement(db, SQL)?.query_row([], |row| row.get(0))?)
    }
    pub fn pruned_through_sequence(&self, db: &Connection) -> Result<i64> {
        const SQL: &str = "SELECT COALESCE((SELECT last_sequence \
            FROM orchestration_v2_projection_metadata WHERE projection_name = ?1), 0)";
        Ok(statement(db, SQL)?.query_row([EVENT_LOG_PRUNED_THROUGH], |row| row.get(0))?)
    }
    pub fn replay_stats(
        &self,
        db: &Connection,
        filter: &EventFilter,
        agents_only: bool,
        max_events: Option<i64>,
    ) -> Result<ReplayStats> {
        let mut f = filter.clone();
        f.limit = max_events.map(|n| n.max(0).saturating_add(1));
        let (query, args) = Self::row_query(&f, agents_only, true);
        let sql = format!(
            "SELECT COUNT(*), COALESCE(SUM(length(CAST(payload_json AS BLOB))), 0), \
             COALESCE(MAX(event_type = 'thread.created'), 0) FROM ({query})"
        );
        Ok(
            statement(db, &sql)?.query_row(params_from_iter(args), |row| {
                Ok(ReplayStats {
                    event_count: row.get(0)?,
                    raw_payload_bytes: row.get(1)?,
                    has_create_event: row.get(2)?,
                })
            })?,
        )
    }
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Decode {
            entity: "event",
            detail: format!("missing string {key}"),
        })
}

fn validate_metadata(metadata: &Map<String, Value>) -> Result<()> {
    for key in METADATA_KEYS {
        if metadata.get(key).is_some_and(|value| !value.is_string()) {
            return Err(Error::Decode {
                entity: "event metadata",
                detail: format!("{key} must be a string when present"),
            });
        }
    }
    Ok(())
}
