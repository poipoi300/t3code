use crate::{
    api::*,
    error::{Error, Result},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use t3_persistence::rusqlite::{Connection, OptionalExtension, Params, params};

fn date_sort_key(value: &str) -> (i32, &str) {
    let year_len = if value.starts_with(['+', '-']) { 7 } else { 4 };
    let year = value
        .get(..year_len)
        .and_then(|year| year.parse().ok())
        .unwrap_or(0);
    (year, value.get(year_len..).unwrap_or(value))
}
fn id_collator() -> Result<&'static icu_collator::CollatorBorrowed<'static>> {
    static COLLATOR: std::sync::OnceLock<
        std::result::Result<icu_collator::CollatorBorrowed<'static>, String>,
    > = std::sync::OnceLock::new();
    COLLATOR
        .get_or_init(|| {
            icu_collator::Collator::try_new(Default::default(), Default::default())
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|detail| Error::Decode {
            entity: "ID collation",
            detail: detail.clone(),
        })
}
pub fn id_text<T: AsRef<str>>(id: &IdOrIndex<T>) -> Result<&str> {
    match id {
        IdOrIndex::Id(id) => Ok(id.as_ref()),
        IdOrIndex::Index(_) => Err(Error::Decode {
            entity: "projection ID",
            detail: "compact index in persisted record".into(),
        }),
    }
}
pub fn integer(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| Error::Decode {
        entity: "projection integer",
        detail: "outside SQLite range".into(),
    })
}
pub fn rows<T: DeserializeOwned>(
    db: &Connection,
    sql: &str,
    params: impl Params,
) -> Result<Vec<T>> {
    let mut statement = db.prepare(sql)?;
    let values = statement.query_map(params, |row| row.get::<_, String>(0))?;
    values
        .map(|value| Ok(serde_json::from_str(&value?)?))
        .collect()
}
pub fn one<T: DeserializeOwned>(
    db: &Connection,
    sql: &str,
    params: impl Params,
) -> Result<Option<T>> {
    let raw: Option<String> = db.query_row(sql, params, |row| row.get(0)).optional()?;
    Ok(raw.map(|value| serde_json::from_str(&value)).transpose()?)
}
pub fn thread(db: &Connection, id: &ThreadId) -> Result<Option<OrchestrationV2AppThread>> {
    one(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_threads WHERE thread_id=?1",
        [&id.0],
    )
}
pub fn ensure_thread(db: &Connection, id: &ThreadId) -> Result<()> {
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM orchestration_v2_projection_threads WHERE thread_id=?1)",
        [&id.0],
        |row| row.get(0),
    )?;
    if exists {
        Ok(())
    } else {
        Err(Error::ThreadNotFound(id.clone()))
    }
}
pub fn require_thread(db: &Connection, id: &ThreadId) -> Result<OrchestrationV2AppThread> {
    thread(db, id)?.ok_or_else(|| Error::ThreadNotFound(id.clone()))
}
fn json_list<T: Serialize>(value: &Option<Vec<T>>) -> Result<Option<String>> {
    Ok(value.as_ref().map(serde_json::to_string).transpose()?)
}
const SETTLED_SUBAGENT: &str = "(subagent.status IN ('completed','failed','cancelled','interrupted') AND COALESCE(subagent.completion_delivery_state,'') IN ('acknowledged','delivered','disposed') AND NOT EXISTS (SELECT 1 FROM orchestration_v2_projection_runs AS run WHERE run.run_id=subagent.run_id AND run.status IN ('preparing','queued','starting','running','waiting')))";
pub(crate) const OWNED_PROVIDER_NODES: &str = "SELECT owner.node_id FROM orchestration_v2_projection_provider_threads AS owned INDEXED BY orchestration_v2_projection_provider_threads_owner_idx CROSS JOIN orchestration_v2_projection_nodes AS owner ON owner.node_id=owned.owner_node_id WHERE owned.owner_node_id IS NOT NULL AND owner.thread_id=?1";

pub fn records(
    db: &Connection,
    id: &ThreadId,
    fields: &[RecordKind],
    filter: &RecordFilter,
) -> Result<ProjectionRecords> {
    query_records(
        db,
        &ProjectionRecordQuery {
            thread_id: id.clone(),
            fields: fields.to_vec(),
            filter: filter.clone(),
            messages: MessageFilter::default(),
            run_statuses: None,
            include_latest_run: false,
        },
    )
}

fn message_predicates(
    filter: &MessageFilter,
    args: &mut Vec<t3_persistence::rusqlite::types::Value>,
) -> Result<String> {
    let mut sql = String::new();
    for (column, list) in [
        ("message_id", json_list(&filter.ids)?),
        ("role", json_list(&filter.roles)?),
        ("run_id", json_list(&filter.run_ids)?),
    ] {
        if let Some(list) = list {
            args.push(list.into());
            sql.push_str(&format!(
                " AND {column} IN (SELECT value FROM json_each(?{}))",
                args.len()
            ));
        }
    }
    if filter.monitor_notifications_only {
        sql.push_str(" AND json_extract(payload_json,'$.notification.source.kind')='monitor'");
    }
    Ok(sql)
}

pub fn message_count(db: &Connection, id: &ThreadId, filter: &MessageFilter) -> Result<u64> {
    let mut args = vec![t3_persistence::rusqlite::types::Value::Text(id.0.clone())];
    let predicates = message_predicates(filter, &mut args)?;
    let count:i64 = db.query_row(&format!("SELECT COUNT(*) FROM orchestration_v2_projection_messages WHERE thread_id=?1{predicates}"),t3_persistence::rusqlite::params_from_iter(args),|row|row.get(0))?;
    Ok(count as u64)
}

pub fn count_threads(db: &Connection) -> Result<u64> {
    Ok(db.query_row::<i64, _, _>(
        "SELECT COUNT(*) FROM orchestration_v2_projection_threads WHERE deleted_at IS NULL",
        [],
        |r| r.get(0),
    )? as u64)
}
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDelegatedTask {
    pub parent_thread_id: ThreadId,
    pub task_id: NodeId,
}
pub fn open_remote_delegated_tasks(db: &Connection) -> Result<Vec<RemoteDelegatedTask>> {
    let mut statement=db.prepare("SELECT thread_id,subagent_id FROM orchestration_v2_projection_subagents WHERE origin='app_owned' AND child_thread_id IS NULL AND json_type(payload_json,'$.remoteChild')='object' AND (json_type(payload_json,'$.result')='null' OR (json_extract(payload_json,'$.status') IN('cancelled','interrupted') AND json_type(payload_json,'$.remoteChild.stoppedThere') IS NOT 'true') OR json_type(payload_json,'$.remoteChild.stoppedThere')='false')")?;
    Ok(statement
        .query_map([], |r| {
            Ok(RemoteDelegatedTask {
                parent_thread_id: ThreadId(r.get(0)?),
                task_id: NodeId(r.get(1)?),
            })
        })?
        .collect::<std::result::Result<_, _>>()?)
}
pub fn active_subagent_thread_ids(db: &Connection) -> Result<Vec<ThreadId>> {
    let mut statement=db.prepare("SELECT thread_id FROM orchestration_v2_projection_threads WHERE deleted_at IS NULL AND json_extract(payload_json,'$.archivedAt') IS NULL AND json_extract(payload_json,'$.lineage.relationshipToParent')='subagent'")?;
    Ok(statement
        .query_map([], |r| Ok(ThreadId(r.get(0)?)))?
        .collect::<std::result::Result<_, _>>()?)
}
pub fn find_guest_share_thread_id(
    db: &Connection,
    share_id: &GuestShareId,
) -> Result<Option<ThreadId>> {
    Ok(db.query_row("SELECT thread_id FROM orchestration_v2_projection_threads WHERE json_extract(payload_json,'$.guestShare.id')=?1 LIMIT 1",[&share_id.0],|r|Ok(ThreadId(r.get(0)?))).optional()?)
}
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadWorktreePath {
    pub thread_id: ThreadId,
    pub worktree_path: String,
}
pub fn thread_worktree_paths(
    db: &Connection,
    include_archived: bool,
) -> Result<Vec<ThreadWorktreePath>> {
    let mut statement=db.prepare("SELECT thread_id,json_extract(payload_json,'$.worktreePath') FROM orchestration_v2_projection_threads WHERE deleted_at IS NULL AND (?1 OR json_extract(payload_json,'$.archivedAt') IS NULL) AND json_extract(payload_json,'$.worktreePath') IS NOT NULL")?;
    Ok(statement
        .query_map([include_archived], |r| {
            Ok(ThreadWorktreePath {
                thread_id: ThreadId(r.get(0)?),
                worktree_path: r.get(1)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?)
}
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveThreadListEntry {
    pub id: ThreadId,
    pub project_id: ProjectId,
    pub title: String,
    pub lineage: OrchestrationV2AppThreadLineage,
    pub settled_override: Option<AppThreadSettledOverride>,
    pub settled_at: Option<IsoDateTime>,
    pub snoozed_until: Option<IsoDateTime>,
    pub updated_at: IsoDateTime,
}
pub fn list_active_threads(
    db: &Connection,
    project_id: Option<&ProjectId>,
) -> Result<Vec<ActiveThreadListEntry>> {
    let project_filter = if project_id.is_some() {
        " AND t.project_id=?1"
    } else {
        ""
    };
    let entries: Vec<Value> = rows(
        db,
        &format!(
            r#"SELECT json_object(
      'id',t.thread_id,'projectId',t.project_id,'title',t.title,'updatedAt',t.updated_at,
      'lineage',json_extract(t.payload_json,'$.lineage'),
      'settledOverride',json_extract(t.payload_json,'$.settledOverride'),
      'settledAt',json_extract(t.payload_json,'$.settledAt'),
      'snoozedUntil',json_extract(t.payload_json,'$.snoozedUntil'))
      FROM orchestration_v2_projection_threads t
      WHERE t.deleted_at IS NULL AND t.archived_at IS NULL{project_filter}"#
        ),
        t3_persistence::rusqlite::params_from_iter(project_id.map(|id| &id.0)),
    )?;
    entries
        .into_iter()
        .map(|row| {
            Ok(ActiveThreadListEntry {
                id: serde_json::from_value(row["id"].clone())?,
                project_id: serde_json::from_value(row["projectId"].clone())?,
                title: serde_json::from_value(row["title"].clone())?,
                updated_at: serde_json::from_value(row["updatedAt"].clone())?,
                lineage: shell_index_lineage(&row["lineage"])?,
                settled_override: shell_index_settled_override(&row["settledOverride"]),
                settled_at: serde_json::from_value(row["settledAt"].clone())?,
                snoozed_until: serde_json::from_value(row["snoozedUntil"].clone())?,
            })
        })
        .collect()
}

fn shell_index_lineage(lineage: &Value) -> Result<OrchestrationV2AppThreadLineage> {
    let mut normalized = serde_json::json!({
        "parentThreadId": lineage["parentThreadId"],
        "relationshipToParent": match lineage["relationshipToParent"].as_str() {
            Some("fork" | "subagent") => lineage["relationshipToParent"].clone(),
            _ => Value::Null,
        },
        "rootThreadId": lineage["rootThreadId"],
    });
    if !lineage["sideChat"].is_null() {
        normalized["sideChat"] = (lineage["sideChat"] == true || lineage["sideChat"] == 1).into();
    }
    Ok(serde_json::from_value(normalized)?)
}

fn shell_index_settled_override(value: &Value) -> Option<AppThreadSettledOverride> {
    match value.as_str() {
        Some("settled") => Some(AppThreadSettledOverride::Settled),
        Some("active") => Some(AppThreadSettledOverride::Active),
        _ => None,
    }
}

pub fn thread_titles(db: &Connection, ids: &[ThreadId]) -> Result<OrchestrationV2ThreadTitles> {
    let mut statement=db.prepare("SELECT thread_id,title FROM orchestration_v2_projection_threads WHERE deleted_at IS NULL AND thread_id IN(SELECT value FROM json_each(?1)) ORDER BY thread_id")?;
    Ok(statement
        .query_map([serde_json::to_string(ids)?], |row| {
            Ok((ThreadId(row.get(0)?), row.get(1)?))
        })?
        .collect::<std::result::Result<_, _>>()?)
}

pub fn thread_link_metadata(
    db: &Connection,
    ids: &[ThreadId],
) -> Result<BTreeMap<ThreadId, OrchestrationV2ThreadLinkMetadata>> {
    let mut statement=db.prepare("SELECT thread_id,title,project_id FROM orchestration_v2_projection_threads WHERE deleted_at IS NULL AND thread_id IN(SELECT value FROM json_each(?1)) ORDER BY thread_id")?;
    Ok(statement
        .query_map([serde_json::to_string(ids)?], |row| {
            Ok((
                ThreadId(row.get(0)?),
                OrchestrationV2ThreadLinkMetadata {
                    title: row.get(1)?,
                    project_id: Some(ProjectId(row.get(2)?)),
                },
            ))
        })?
        .collect::<std::result::Result<_, _>>()?)
}

pub struct OlderSettledRows {
    pub thread_ids: Vec<ThreadId>,
    pub counts: Vec<OrchestrationV2OlderSettledCount>,
}
pub fn list_older_settled_threads(
    db: &Connection,
    input: &OrchestrationV2GetOlderSettledThreadsInput,
) -> Result<OlderSettledRows> {
    let older = "WITH older AS(SELECT thread_id,project_id,json_extract(payload_json,'$.settledAt') settled_at FROM orchestration_v2_projection_threads WHERE deleted_at IS NULL AND json_extract(payload_json,'$.archivedAt') IS NULL AND json_extract(payload_json,'$.lineage.relationshipToParent') IS NOT 'subagent' AND json_extract(payload_json,'$.lineage.sideChat') IS NOT 1 AND json_extract(payload_json,'$.settledOverride')='settled' AND json_extract(payload_json,'$.settledAt') IS NOT NULL AND (?1 IS NULL OR project_id IN(SELECT value FROM json_each(?1))))";
    let predicate = " WHERE (settled_at<?2 OR (?3 IS NOT NULL AND settled_at=?2 AND thread_id>?3))";
    let projects = json_list(&input.project_ids)?;
    let args = params![
        projects,
        input.before.settled_at.0,
        input.before.thread_id.as_ref().map(|id| &id.0),
        input.limit
    ];
    if input.limit == 0 {
        let mut statement=db.prepare(&format!("{older} SELECT project_id,COUNT(*) FROM older{predicate} AND ?4=0 GROUP BY project_id ORDER BY project_id"))?;
        let counts = statement
            .query_map(args, |row| {
                Ok(OrchestrationV2OlderSettledCount {
                    project_id: ProjectId(row.get(0)?),
                    count: row.get::<_, i64>(1)? as u64,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(OlderSettledRows {
            thread_ids: vec![],
            counts,
        })
    } else {
        let mut statement=db.prepare(&format!("{older} SELECT thread_id FROM older{predicate} ORDER BY settled_at DESC,thread_id ASC LIMIT ?4"))?;
        let thread_ids = statement
            .query_map(args, |row| Ok(ThreadId(row.get(0)?)))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(OlderSettledRows {
            thread_ids,
            counts: vec![],
        })
    }
}

pub struct RawOlderSettledPage {
    page: OlderSettledRows,
    shells: RawShellSnapshot,
}
pub fn read_older_settled_rows(
    db: &Connection,
    input: &OrchestrationV2GetOlderSettledThreadsInput,
) -> Result<RawOlderSettledPage> {
    let page = list_older_settled_threads(db, input)?;
    let shells = read_shell_rows(
        db,
        &ShellQuery {
            thread_ids: Some(page.thread_ids.clone()),
            ..Default::default()
        },
        true,
    )?;
    Ok(RawOlderSettledPage { page, shells })
}
pub fn decode_older_settled_rows(
    raw: RawOlderSettledPage,
) -> Result<OrchestrationV2OlderSettledThreadsPage> {
    let shells = decode_shell_rows(raw.shells)?;
    let mut by_id = shells
        .threads
        .into_iter()
        .map(|shell| (shell.id.0.clone(), shell))
        .collect::<BTreeMap<_, _>>();
    let threads = raw
        .page
        .thread_ids
        .into_iter()
        .filter_map(|id| by_id.remove(&id.0))
        .collect();
    Ok(OrchestrationV2OlderSettledThreadsPage {
        snapshot_sequence: shells.snapshot_sequence,
        threads,
        counts: raw.page.counts,
    })
}
pub fn older_settled_threads(
    db: &Connection,
    input: &OrchestrationV2GetOlderSettledThreadsInput,
) -> Result<OrchestrationV2OlderSettledThreadsPage> {
    decode_older_settled_rows(read_older_settled_rows(db, input)?)
}

pub fn query_records(db: &Connection, query: &ProjectionRecordQuery) -> Result<ProjectionRecords> {
    let ProjectionRecordQuery {
        thread_id: id,
        fields,
        filter,
        messages: message_filter,
        run_statuses,
        include_latest_run,
    } = query;
    let recent = if let Some((keep_runs, keep_message)) = &filter.since_last_compaction {
        let mut statement=db.prepare("WITH compaction AS (SELECT run.ordinal,run.requested_at FROM orchestration_v2_projection_turn_items AS item INNER JOIN orchestration_v2_projection_runs AS run ON run.run_id=item.run_id WHERE item.thread_id=?1 AND item.type='compaction' AND item.status='completed' AND run.status<>'rolled_back' ORDER BY item.ordinal DESC LIMIT 1) SELECT run.run_id FROM compaction INNER JOIN orchestration_v2_projection_runs AS run ON run.thread_id=?1 WHERE run.ordinal>=compaction.ordinal OR run.completed_at IS NULL OR run.completed_at>=compaction.requested_at OR run.status IN ('queued','preparing','starting','running','waiting') UNION SELECT message.run_id FROM compaction INNER JOIN orchestration_v2_projection_messages AS message ON message.message_id=?2 WHERE message.thread_id=?1 AND message.run_id IS NOT NULL")?;
        let mut ids = statement
            .query_map(
                t3_persistence::rusqlite::params![id.0, keep_message.0],
                |row| row.get::<_, String>(0),
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if ids.is_empty() {
            None
        } else {
            ids.extend(keep_runs.iter().map(|id| id.0.clone()));
            Some(serde_json::to_string(&ids)?)
        }
    } else {
        None
    };
    let thread = require_thread(db, id)?;
    let mut records = Vec::with_capacity(fields.len());
    for kind in fields {
        let provider_base_scope = format!(
            "(thread_id=?1 OR owner_node_id IN({OWNED_PROVIDER_NODES}) OR provider_thread_id IN(SELECT provider_thread_id FROM orchestration_v2_projection_subagents WHERE thread_id=?1 AND provider_thread_id IS NOT NULL))"
        );
        let (table, scope, order) = match kind {
            RecordKind::Runs => ("runs", "thread_id=?1", "ordinal ASC"),
            RecordKind::Attempts => (
                "run_attempts",
                "thread_id=?1",
                "run_id ASC,attempt_ordinal ASC",
            ),
            RecordKind::Nodes => (
                "nodes",
                "thread_id=?1",
                "COALESCE(started_at,''),node_id ASC",
            ),
            RecordKind::Subagents => (
                "subagents",
                "thread_id=?1",
                "COALESCE(started_at,''),subagent_id ASC",
            ),
            RecordKind::ProviderSessions => (
                "provider_sessions",
                "provider_session_id IN (SELECT provider_session_id FROM orchestration_v2_projection_provider_session_bindings WHERE thread_id=?1)",
                "updated_at ASC,provider_session_id ASC",
            ),
            RecordKind::ProviderThreads => (
                "provider_threads",
                provider_base_scope.as_str(),
                "COALESCE(first_run_ordinal,0),provider_thread_id ASC",
            ),
            RecordKind::ProviderTurns => (
                "provider_turns",
                "thread_id=?1",
                "provider_thread_id ASC,ordinal ASC",
            ),
            RecordKind::RuntimeRequests => (
                "runtime_requests",
                "thread_id=?1",
                "created_at ASC,runtime_request_id ASC",
            ),
            RecordKind::Messages => ("messages", "thread_id=?1", "created_at ASC,message_id ASC"),
            RecordKind::Plans => ("plans", "thread_id=?1", "plan_id ASC"),
            RecordKind::TurnItems => ("turn_items", "thread_id=?1", "ordinal ASC,turn_item_id ASC"),
            RecordKind::CheckpointScopes => (
                "checkpoint_scopes",
                "thread_id=?1",
                "ordinal_within_parent ASC,scope_id ASC",
            ),
            RecordKind::Checkpoints => (
                "checkpoints",
                "thread_id=?1",
                "scope_id ASC,ordinal_within_scope ASC",
            ),
            RecordKind::ContextHandoffs => ("context_handoffs", "thread_id=?1", "rowid ASC"),
            RecordKind::ContextTransfers => (
                "context_transfers",
                "(source_thread_id=?1 OR target_thread_id=?1)",
                "rowid ASC",
            ),
        };
        let provider_scope = format!(
            "(thread_id=?1 OR owner_node_id IN ({OWNED_PROVIDER_NODES}) OR provider_thread_id IN (SELECT provider_thread_id FROM orchestration_v2_projection_subagents AS subagent WHERE thread_id=?1 AND NOT {SETTLED_SUBAGENT} AND provider_thread_id IS NOT NULL) OR (provider_session_id IN (SELECT provider_session_id FROM orchestration_v2_projection_provider_session_bindings WHERE thread_id=?1) AND EXISTS (SELECT 1 FROM orchestration_v2_projection_subagents AS owner WHERE owner.provider_thread_id=record.provider_thread_id AND owner.thread_id=?1)))"
        );
        let leaf = "kind IN ('tool_call','assistant_message','reasoning') AND status IN ('completed','interrupted','failed','cancelled','rolled_back') AND run_id IS NOT NULL";
        let node_scope = format!(
            "node.rowid IN(SELECT rowid FROM orchestration_v2_projection_nodes INDEXED BY {} WHERE thread_id=?1 AND ({leaf})=0 {} UNION ALL SELECT rowid FROM orchestration_v2_projection_nodes INDEXED BY orchestration_v2_projection_nodes_thread_run_idx WHERE thread_id=?1 AND run_id IN(SELECT run_id FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND status IN('preparing','queued','starting','running','waiting')) AND ({leaf}))",
            if recent.is_some() {
                "orchestration_v2_projection_nodes_thread_run_idx"
            } else {
                "orchestration_v2_projection_nodes_settled_leaf_idx"
            },
            if filter.omit_settled_subagent_records {
                format!(
                    "EXCEPT SELECT settled_node.rowid FROM orchestration_v2_projection_subagents AS subagent INDEXED BY orchestration_v2_projection_subagents_settled_idx CROSS JOIN orchestration_v2_projection_nodes AS settled_node ON settled_node.node_id=subagent.subagent_id WHERE subagent.thread_id=?1 AND {SETTLED_SUBAGENT}"
                )
            } else {
                String::new()
            }
        );
        let scope = if *kind == RecordKind::Nodes && filter.omit_settled_leaf_nodes {
            node_scope.as_str()
        } else if *kind == RecordKind::ProviderThreads && filter.omit_settled_subagent_records {
            provider_scope.as_str()
        } else {
            scope
        };
        let mut sql = format!(
            "SELECT payload_json FROM orchestration_v2_projection_{table} AS {} WHERE {scope}",
            if *kind == RecordKind::Subagents {
                "subagent"
            } else if *kind == RecordKind::Nodes {
                "node"
            } else {
                "record"
            }
        );
        let has_key_filter = match kind {
            RecordKind::Runs => filter.run_ids.is_some(),
            RecordKind::Attempts => filter.attempt_ids.is_some(),
            RecordKind::Nodes => filter.node_ids.is_some(),
            RecordKind::Subagents => filter.subagent_ids.is_some(),
            RecordKind::Messages => message_filter.ids.is_some(),
            _ => false,
        };
        if has_key_filter {
            sql = sql.replace("WHERE thread_id=?1", "WHERE +thread_id=?1");
        }
        let mut args = vec![t3_persistence::rusqlite::types::Value::Text(id.0.clone())];
        let mut add_list = |column: &str, list: Option<String>| {
            if let Some(list) = list {
                args.push(list.into());
                sql.push_str(&format!(
                    " AND {column} IN (SELECT value FROM json_each(?{}))",
                    args.len()
                ));
            }
        };
        match kind {
            RecordKind::Runs => add_list("run_id", json_list(&filter.run_ids)?),
            RecordKind::Attempts => {
                add_list("run_id", json_list(&filter.run_ids)?);
                add_list("attempt_id", json_list(&filter.attempt_ids)?);
            }
            RecordKind::Nodes => add_list("node_id", json_list(&filter.node_ids)?),
            RecordKind::Subagents => {
                add_list("subagent_id", json_list(&filter.subagent_ids)?);
                add_list("run_id", json_list(&filter.subagent_run_ids)?);
            }
            RecordKind::ProviderThreads => add_list(
                "provider_thread_id",
                json_list(&filter.provider_thread_ids)?,
            ),
            RecordKind::ProviderTurns => {
                add_list(
                    "provider_thread_id",
                    json_list(&filter.provider_turn_thread_ids)?,
                );
                add_list(
                    "run_attempt_id",
                    json_list(&filter.provider_turn_attempt_ids)?,
                );
            }
            RecordKind::RuntimeRequests => {
                add_list("status", json_list(&filter.runtime_request_statuses)?)
            }
            RecordKind::ContextTransfers => {
                add_list("status", json_list(&filter.context_transfer_statuses)?)
            }
            RecordKind::TurnItems => {
                add_list(
                    "json_extract(payload_json,'$.requestId')",
                    json_list(&filter.turn_item_request_ids)?,
                );
                add_list("type", json_list(&filter.turn_item_types)?);
                add_list("status", json_list(&filter.turn_item_statuses)?);
                add_list("node_id", json_list(&filter.turn_item_node_ids)?);
            }
            _ => {}
        }
        if *kind == RecordKind::TurnItems {
            if let Some(run_id) = &filter.turn_item_run_id {
                args.push(
                    run_id
                        .as_ref()
                        .map(|id| t3_persistence::rusqlite::types::Value::Text(id.0.clone()))
                        .unwrap_or(t3_persistence::rusqlite::types::Value::Null),
                );
                sql.push_str(&format!(" AND run_id IS ?{}", args.len()));
            }
            if let Some(run_ids) = &filter.turn_item_run_ids {
                args.push(serde_json::to_string(run_ids)?.into());
                sql.push_str(&format!(
                    " AND (run_id IN (SELECT value FROM json_each(?{})){})",
                    args.len(),
                    if run_ids.iter().any(Option::is_none) {
                        " OR run_id IS NULL"
                    } else {
                        ""
                    }
                ));
            }
        }
        if *kind == RecordKind::Messages {
            sql.push_str(&message_predicates(message_filter, &mut args)?);
        }
        if *kind == RecordKind::Runs
            && let Some(statuses) = run_statuses
        {
            args.push(serde_json::to_string(statuses)?.into());
            sql.push_str(&format!(
                " AND (status IN (SELECT value FROM json_each(?{}))",
                args.len()
            ));
            if *include_latest_run {
                sql.push_str(" OR run_id=(SELECT run_id FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND (status<>'cancelled' OR json_extract(payload_json,'$.startedAt') IS NOT NULL) ORDER BY ordinal DESC LIMIT 1)");
            }
            sql.push(')');
        }
        if let Some(recent) = &recent {
            let column = match kind {
                RecordKind::Attempts | RecordKind::Nodes | RecordKind::Messages => Some("run_id"),
                _ => None,
            };
            if let Some(column) = column {
                args.push(recent.clone().into());
                sql.push_str(&format!(
                    " AND {column} IN (SELECT value FROM json_each(?{}))",
                    args.len()
                ));
            }
            if *kind == RecordKind::ProviderTurns {
                args.push(recent.clone().into());
                sql.push_str(&format!(" AND run_attempt_id IN (SELECT attempt_id FROM orchestration_v2_projection_run_attempts WHERE thread_id=?1 AND run_id IN (SELECT value FROM json_each(?{})))",args.len()));
            }
        }
        if *kind == RecordKind::Subagents && filter.omit_settled_subagents {
            args.push(serde_json::to_string(&filter.except_subagent_ids)?.into());
            sql.push_str(&format!(" AND (subagent_id IN (SELECT value FROM json_each(?{})) OR NOT {SETTLED_SUBAGENT})",args.len()));
        }
        if *kind == RecordKind::ProviderTurns && filter.latest_provider_turn_usage_only {
            sql.push_str(" AND json_extract(payload_json,'$.tokenUsage') IS NOT NULL ORDER BY julianday(json_extract(payload_json,'$.tokenUsage.updatedAt')) DESC,provider_thread_id ASC,ordinal ASC LIMIT 1");
        } else {
            let order =
                if *kind == RecordKind::Runs && run_statuses.is_some() && !include_latest_run {
                    "+ordinal ASC"
                } else {
                    order
                };
            sql.push_str(&format!(" ORDER BY {order}"));
        }
        let params = t3_persistence::rusqlite::params_from_iter(args);
        records.push(match kind {
            RecordKind::Runs => ProjectionRecordSet::Runs(rows(db, &sql, params)?),
            RecordKind::Attempts => ProjectionRecordSet::Attempts(rows(db, &sql, params)?),
            RecordKind::Nodes => ProjectionRecordSet::Nodes(rows(db, &sql, params)?),
            RecordKind::Subagents => ProjectionRecordSet::Subagents(rows(db, &sql, params)?),
            RecordKind::ProviderSessions => {
                ProjectionRecordSet::ProviderSessions(rows(db, &sql, params)?)
            }
            RecordKind::ProviderThreads => {
                ProjectionRecordSet::ProviderThreads(rows(db, &sql, params)?)
            }
            RecordKind::ProviderTurns => {
                ProjectionRecordSet::ProviderTurns(rows(db, &sql, params)?)
            }
            RecordKind::RuntimeRequests => {
                ProjectionRecordSet::RuntimeRequests(rows(db, &sql, params)?)
            }
            RecordKind::Messages => ProjectionRecordSet::Messages(rows(db, &sql, params)?),
            RecordKind::Plans => ProjectionRecordSet::Plans(rows(db, &sql, params)?),
            RecordKind::TurnItems => ProjectionRecordSet::TurnItems(rows(db, &sql, params)?),
            RecordKind::CheckpointScopes => {
                ProjectionRecordSet::CheckpointScopes(rows(db, &sql, params)?)
            }
            RecordKind::Checkpoints => ProjectionRecordSet::Checkpoints(rows(db, &sql, params)?),
            RecordKind::ContextHandoffs => {
                ProjectionRecordSet::ContextHandoffs(rows(db, &sql, params)?)
            }
            RecordKind::ContextTransfers => {
                ProjectionRecordSet::ContextTransfers(rows(db, &sql, params)?)
            }
        });
    }
    if let Some(index) = records
        .iter()
        .position(|set| matches!(set, ProjectionRecordSet::Messages(_)))
        && let ProjectionRecordSet::Messages(messages) = &mut records[index]
    {
        order_messages(db, id, messages)?;
    }
    Ok(ProjectionRecords { thread, records })
}

fn order_messages(
    db: &Connection,
    id: &ThreadId,
    messages: &mut Vec<OrchestrationV2ConversationMessage>,
) -> Result<()> {
    if messages.len() < 2 {
        return Ok(());
    }
    let ids = messages
        .iter()
        .map(|m| id_text(&m.id))
        .collect::<Result<Vec<_>>>()?;
    let user_only = messages
        .iter()
        .all(|m| m.role == ConversationMessageRole::User);
    let predicate = if user_only {
        "type='user_message'"
    } else {
        "type IN('user_message','assistant_message')"
    };
    let mut statement=db.prepare(&format!("SELECT json_extract(payload_json,'$.messageId'),MIN(ordinal) FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 AND {predicate} AND json_extract(payload_json,'$.messageId') IN(SELECT value FROM json_each(?2)) GROUP BY json_extract(payload_json,'$.messageId')"))?;
    let ordinals: BTreeMap<String, i64> = statement
        .query_map(params![id.0, serde_json::to_string(&ids)?], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<std::result::Result<_, _>>()?;
    let mut keyed = messages
        .drain(..)
        .map(|message| Ok((id_text(&message.id)?.to_owned(), message)))
        .collect::<Result<Vec<_>>>()?;
    let collator = id_collator()?;
    keyed.sort_by(|(a_id, a), (b_id, b)| {
        ordinals
            .get(a_id)
            .copied()
            .unwrap_or(i64::MAX)
            .cmp(&ordinals.get(b_id).copied().unwrap_or(i64::MAX))
            .then_with(|| date_sort_key(&a.created_at.0).cmp(&date_sort_key(&b.created_at.0)))
            .then_with(|| collator.compare(a_id, b_id))
    });
    messages.extend(keyed.into_iter().map(|(_, message)| message));
    Ok(())
}

const ALL_RECORDS: &[RecordKind] = &[
    RecordKind::Runs,
    RecordKind::Attempts,
    RecordKind::Nodes,
    RecordKind::Subagents,
    RecordKind::ProviderSessions,
    RecordKind::ProviderThreads,
    RecordKind::ProviderTurns,
    RecordKind::RuntimeRequests,
    RecordKind::Messages,
    RecordKind::Plans,
    RecordKind::TurnItems,
    RecordKind::CheckpointScopes,
    RecordKind::Checkpoints,
    RecordKind::ContextHandoffs,
    RecordKind::ContextTransfers,
];
pub fn canonical(db: &Connection, id: &ThreadId) -> Result<OrchestrationV2ThreadProjection> {
    let loaded = records(db, id, ALL_RECORDS, &RecordFilter::default())?;
    Ok(from_records(loaded))
}
fn from_records(loaded: ProjectionRecords) -> OrchestrationV2ThreadProjection {
    let mut projection = OrchestrationV2ThreadProjection {
        updated_at: loaded.thread.updated_at.clone(),
        thread: loaded.thread,
        runs: vec![],
        attempts: vec![],
        nodes: vec![],
        subagents: vec![],
        provider_sessions: vec![],
        provider_threads: vec![],
        provider_turns: vec![],
        runtime_requests: vec![],
        messages: vec![],
        plans: vec![],
        turn_items: vec![],
        checkpoint_scopes: vec![],
        checkpoints: vec![],
        context_handoffs: vec![],
        context_transfers: vec![],
        visible_turn_items: vec![],
    };
    for record in loaded.records {
        match record {
            ProjectionRecordSet::Runs(values) => projection.runs = values,
            ProjectionRecordSet::Attempts(values) => projection.attempts = values,
            ProjectionRecordSet::Nodes(values) => projection.nodes = values,
            ProjectionRecordSet::Subagents(values) => projection.subagents = values,
            ProjectionRecordSet::ProviderSessions(values) => projection.provider_sessions = values,
            ProjectionRecordSet::ProviderThreads(values) => projection.provider_threads = values,
            ProjectionRecordSet::ProviderTurns(values) => projection.provider_turns = values,
            ProjectionRecordSet::RuntimeRequests(values) => projection.runtime_requests = values,
            ProjectionRecordSet::Messages(values) => projection.messages = values,
            ProjectionRecordSet::Plans(values) => projection.plans = values,
            ProjectionRecordSet::TurnItems(values) => projection.turn_items = values,
            ProjectionRecordSet::CheckpointScopes(values) => projection.checkpoint_scopes = values,
            ProjectionRecordSet::Checkpoints(values) => projection.checkpoints = values,
            ProjectionRecordSet::ContextHandoffs(values) => projection.context_handoffs = values,
            ProjectionRecordSet::ContextTransfers(values) => projection.context_transfers = values,
        }
    }
    projection
}

pub(crate) fn visible(
    item: &Value,
    runs: &[Value],
    attempts: &[Value],
    items: &[Value],
    inherited: bool,
) -> bool {
    let run = runs
        .iter()
        .find(|run| !item["runId"].is_null() && run["id"] == item["runId"]);
    let status = run.and_then(|run| run["status"].as_str());
    if status == Some("rolled_back") && !inherited {
        return false;
    }
    let cancelled = inherited
        && status == Some("rolled_back")
        && attempts
            .iter()
            .any(|attempt| attempt["runId"] == item["runId"] && attempt["status"] == "cancelled");
    if (matches!(status, Some("queued" | "cancelled")) || cancelled)
        && item["type"] == "user_message"
        && item["inputIntent"] == "queued_turn"
    {
        return false;
    }
    !(item["type"] == "run_interrupt_result"
        && !item["runId"].is_null()
        && !item["nodeId"].is_null()
        && attempts.iter().any(|attempt| {
            attempt["runId"] == item["runId"]
                && attempt["rootNodeId"] == item["nodeId"]
                && attempt["status"] == "superseded"
        })
        && !items.iter().any(|candidate| {
            candidate["type"] == "run_interrupt_request" && candidate["runId"] == item["runId"]
        }))
}
pub(crate) fn item_row(
    item: OrchestrationV2TurnItem,
    visibility: ProjectedTurnItemVisibility,
) -> Result<OrchestrationV2ProjectedTurnItem> {
    let value = serde_json::to_value(&item)?;
    Ok(OrchestrationV2ProjectedTurnItem {
        position: 0,
        visibility,
        source_thread_id: IdOrIndex::Id(serde_json::from_value(value["threadId"].clone())?),
        source_item_id: IdOrIndex::Id(serde_json::from_value(value["id"].clone())?),
        item,
    })
}
fn values<T: Serialize>(items: &[T]) -> Result<Vec<Value>> {
    Ok(items
        .iter()
        .map(t3_contracts::stored::to_value)
        .collect::<std::result::Result<_, _>>()?)
}
pub fn projection(db: &Connection, id: &ThreadId) -> Result<OrchestrationV2ThreadProjection> {
    projection_inner(db, id, &mut BTreeSet::new())
}
fn projection_inner(
    db: &Connection,
    id: &ThreadId,
    seen: &mut BTreeSet<String>,
) -> Result<OrchestrationV2ThreadProjection> {
    let mut projection = canonical(db, id)?;
    let runs = values(&projection.runs)?;
    let attempts = values(&projection.attempts)?;
    let items = values(&projection.turn_items)?;
    let thread = serde_json::to_value(&projection.thread)?;
    let mut inherited = Vec::new();
    if thread["forkedFrom"]["type"] == "run" && thread["lineage"]["sideChat"] != true {
        let source_id: ThreadId = serde_json::from_value(thread["forkedFrom"]["threadId"].clone())?;
        if !seen.contains(&source_id.0) {
            seen.insert(id.0.clone());
            let source = projection_inner(db, &source_id, seen)?;
            seen.remove(&id.0);
            if let Some(source_run)=source.runs.iter().find(|run|match &run.id {IdOrIndex::Id(id)=>Some(id.0.as_str()),IdOrIndex::Index(_)=>None}==thread["forkedFrom"]["runId"].as_str()) {
                inherited.extend(source.visible_turn_items.iter().filter(|row|row.source_thread_id!=IdOrIndex::Id(source_id.clone()) || matches!(&row.item,OrchestrationV2TurnItem::Fork(_))).cloned().map(|mut row|{row.visibility=ProjectedTurnItemVisibility::Inherited;row}));
                let source_runs=values(&source.runs)?;
                let source_attempts=values(&source.attempts)?;
                let source_items=values(&source.turn_items)?;
                for (item,value) in source.turn_items.iter().zip(&source_items) {
                    let ordinal=source.runs.iter().find(|run|match &run.id {
                        IdOrIndex::Id(id)=>Some(id.0.as_str())==value["runId"].as_str(),
                        IdOrIndex::Index(_)=>false,
                    }).filter(|run|!(run.status==OrchestrationV2RunStatus::RolledBack && run.completed_at.as_ref().is_some_and(|time|date_sort_key(&time.0)<date_sort_key(&projection.thread.created_at.0)))).map(|run|run.ordinal);
                    let within=if value["runId"].is_null(){source.thread.history_origin==Some(Some(OrchestrationV2ThreadHistoryOrigin::V1Import))} else {ordinal.is_some_and(|ordinal|ordinal<=source_run.ordinal)};
                    if within && visible(value,&source_runs,&source_attempts,&source_items,true) { inherited.push(item_row(item.clone(),ProjectedTurnItemVisibility::Inherited)?); }
                }
            }
            let marker = serde_json::json!({"id":format!("turn-item:fork:{}",id.0),"threadId":id,"runId":null,"nodeId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":"Forked from conversation","startedAt":null,"completedAt":thread["createdAt"],"updatedAt":thread["createdAt"],"type":"fork","source":{"type":"run","threadId":source_id,"runId":thread["forkedFrom"]["runId"]},"targetThreadId":id});
            let mut row = item_row(
                serde_json::from_value(marker)?,
                ProjectedTurnItemVisibility::Synthetic,
            )?;
            row.source_thread_id = IdOrIndex::Id(source_id);
            inherited.push(row);
        }
    }
    for (item, value) in projection.turn_items.iter().zip(&items) {
        if visible(value, &runs, &attempts, &items, false) {
            inherited.push(item_row(item.clone(), ProjectedTurnItemVisibility::Local)?);
        }
    }
    for (position, row) in inherited.iter_mut().enumerate() {
        row.position = position as u64;
    }
    projection.visible_turn_items = inherited;
    Ok(projection)
}

fn find_payloads(db: &Connection, rows: &[TimelineRow]) -> Result<Vec<String>> {
    let mut payloads = BTreeMap::new();
    for chunk in rows.chunks(128) {
        let wanted = chunk
            .iter()
            .map(|row| serde_json::json!({"threadId":row.source.0,"id":row.id.0}))
            .collect::<Vec<_>>();
        let mut statement=db.prepare("SELECT item.thread_id,item.turn_item_id,item.payload_json FROM json_each(?1) wanted CROSS JOIN orchestration_v2_projection_turn_items item ON item.thread_id=json_extract(wanted.value,'$.threadId') AND item.turn_item_id=json_extract(wanted.value,'$.id')")?;
        for row in statement.query_map([serde_json::to_string(&wanted)?], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (thread, id, json) = row?;
            payloads.insert((thread, id), json);
        }
    }
    rows.iter()
        .map(|row| {
            payloads
                .get(&(row.source.0.clone(), row.id.0.clone()))
                .cloned()
                .ok_or_else(|| Error::Decode {
                    entity: "thread find item",
                    detail: "item disappeared during snapshot".into(),
                })
        })
        .collect()
}
pub fn thread_find_snapshot(
    db: &Connection,
    id: &ThreadId,
) -> Result<crate::ports::transport::thread_find::FindSnapshot> {
    use crate::ports::transport::thread_find::{FindPayload, FindSnapshot};
    let cwd:Option<Option<String>>=db.query_row("SELECT COALESCE(json_extract(t.payload_json,'$.worktreePath'),p.workspace_root) FROM orchestration_v2_projection_threads t JOIN projection_projects p ON p.project_id=t.project_id WHERE t.thread_id=?1 AND t.deleted_at IS NULL AND p.deleted_at IS NULL",[&id.0],|r|r.get(0)).optional()?;
    let cwd = cwd.ok_or_else(|| Error::ThreadNotFound(id.clone()))?;
    let index = timeline_rows(db, id, &mut BTreeSet::new())?;
    let sources = index
        .iter()
        .map(|row| row.source.0.clone())
        .chain([id.0.clone()])
        .collect::<BTreeSet<_>>();
    let sequence=db.query_row::<i64,_,_>("SELECT COALESCE(MAX(sequence),0) FROM orchestration_events INDEXED BY idx_orch_events_stream_sequence WHERE application_event_version=2 AND aggregate_kind='thread' AND stream_id IN(SELECT value FROM json_each(?1))",[serde_json::to_string(&sources)?],|r|r.get(0))? as u64;
    let candidates = index
        .iter()
        .filter(|row| {
            matches!(
                row.item["type"].as_str(),
                Some("user_message" | "assistant_message" | "proposed_plan" | "user_input_request")
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let payloads = find_payloads(db, &candidates)?;
    let mut empty_runs = BTreeSet::new();
    let payloads = candidates
        .iter()
        .zip(payloads)
        .map(|(row, payload)| {
            let value: Value = serde_json::from_str(&payload)?;
            let run_id: Option<RunId> = serde_json::from_value(value["runId"].clone())?;
            if value["type"] == "assistant_message"
                && value["text"] == ""
                && let Some(run) = &run_id
            {
                empty_runs.insert(run.0.clone());
            }
            Ok(FindPayload {
                position: row.position,
                entry_id: value["messageId"].as_str().unwrap_or(&row.id.0).to_owned(),
                run_id,
                item_type: value["type"].as_str().unwrap_or_default().to_owned(),
                payload,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let tools = index
        .into_iter()
        .filter(|row| {
            row.item["type"] == "dynamic_tool"
                && row.item["runId"]
                    .as_str()
                    .is_some_and(|run| empty_runs.contains(run))
        })
        .collect::<Vec<_>>();
    Ok(FindSnapshot {
        cwd,
        sequence,
        payloads,
        tool_payloads: find_payloads(db, &tools)?,
    })
}

pub fn snapshot(db: &Connection, id: &ThreadId) -> Result<ProjectionSnapshot> {
    let projection = projection(db, id)?;
    let snapshot_sequence=db.query_row::<i64,_,_>("SELECT COALESCE(MAX(sequence),0) FROM orchestration_events WHERE application_event_version=2 AND aggregate_kind='thread' AND stream_id=?1",[&id.0],|row|row.get(0))?;
    Ok(ProjectionSnapshot {
        schema_version: 2,
        snapshot_sequence: snapshot_sequence as u64,
        projection,
    })
}

#[derive(Clone)]
struct TimelineRow {
    source: ThreadId,
    id: TurnItemId,
    visibility: ProjectedTurnItemVisibility,
    item: Value,
    synthetic: bool,
    position: u64,
}

fn timeline_rows(
    db: &Connection,
    id: &ThreadId,
    seen: &mut BTreeSet<String>,
) -> Result<Vec<TimelineRow>> {
    let thread = require_thread(db, id)?;
    let runs: Vec<Value> = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_runs WHERE thread_id=?1 ORDER BY ordinal",
        [&id.0],
    )?;
    let attempts: Vec<Value> = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_run_attempts WHERE thread_id=?1 ORDER BY run_id,attempt_ordinal",
        [&id.0],
    )?;
    let items: Vec<Value> = rows(
        db,
        "SELECT json_object('id',turn_item_id,'threadId',thread_id,'runId',run_id,'nodeId',node_id,'parentItemId',parent_item_id,'type',type,'status',status,'ordinal',ordinal,'inputIntent',CASE WHEN type='user_message' THEN json_extract(payload_json,'$.inputIntent') END,'createdBy',CASE WHEN type='user_message' THEN json_extract(payload_json,'$.createdBy') END,'messageId',CASE WHEN type IN ('user_message','assistant_message') THEN json_extract(payload_json,'$.messageId') END) FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 ORDER BY ordinal,turn_item_id",
        [&id.0],
    )?;
    let raw = serde_json::to_value(&thread)?;
    let mut result = Vec::new();
    if raw["forkedFrom"]["type"] == "run" && raw["lineage"]["sideChat"] != true {
        let source: ThreadId = serde_json::from_value(raw["forkedFrom"]["threadId"].clone())?;
        if !seen.contains(&source.0) {
            seen.insert(id.0.clone());
            let source_rows = timeline_rows(db, &source, seen)?;
            seen.remove(&id.0);
            let source_runs: Vec<Value> = rows(
                db,
                "SELECT payload_json FROM orchestration_v2_projection_runs WHERE thread_id=?1 ORDER BY ordinal",
                [&source.0],
            )?;
            if let Some(cutoff) = source_runs
                .iter()
                .find(|r| r["id"] == raw["forkedFrom"]["runId"])
            {
                let source_attempts: Vec<Value> = rows(
                    db,
                    "SELECT payload_json FROM orchestration_v2_projection_run_attempts WHERE thread_id=?1 ORDER BY run_id,attempt_ordinal",
                    [&source.0],
                )?;
                let source_items: Vec<Value> = rows(
                    db,
                    "SELECT json_object('id',turn_item_id,'threadId',thread_id,'runId',run_id,'nodeId',node_id,'parentItemId',parent_item_id,'type',type,'status',status,'ordinal',ordinal,'inputIntent',CASE WHEN type='user_message' THEN json_extract(payload_json,'$.inputIntent') END) FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 ORDER BY ordinal,turn_item_id",
                    [&source.0],
                )?;
                let source_thread = serde_json::to_value(require_thread(db, &source)?)?;
                result.extend(
                    source_rows
                        .into_iter()
                        .filter(|r| r.source != source || r.item["type"] == "fork"),
                );
                for item in &source_items {
                    let eligible = if item["runId"].is_null() {
                        source_thread["historyOrigin"] == "v1_import"
                    } else {
                        source_runs
                            .iter()
                            .find(|r| r["id"] == item["runId"])
                            .is_some_and(|r| {
                                r["ordinal"].as_u64() <= cutoff["ordinal"].as_u64()
                                    && !(r["status"] == "rolled_back"
                                        && r["completedAt"].as_str().is_some_and(|t| {
                                            date_sort_key(t) < date_sort_key(&thread.created_at.0)
                                        }))
                            })
                    };
                    if eligible
                        && visible(item, &source_runs, &source_attempts, &source_items, true)
                    {
                        result.push(TimelineRow {
                            source: source.clone(),
                            id: serde_json::from_value(item["id"].clone())?,
                            visibility: ProjectedTurnItemVisibility::Inherited,
                            item: item.clone(),
                            synthetic: false,
                            position: 0,
                        });
                    }
                }
                for row in &mut result {
                    row.visibility = ProjectedTurnItemVisibility::Inherited;
                }
            }
            let marker = serde_json::json!({"id":format!("turn-item:fork:{}",id.0),"threadId":id,"runId":null,"nodeId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":"Forked from conversation","startedAt":null,"completedAt":raw["createdAt"],"updatedAt":raw["createdAt"],"type":"fork","source":{"type":"run","threadId":source,"runId":raw["forkedFrom"]["runId"]},"targetThreadId":id});
            result.push(TimelineRow {
                source,
                id: serde_json::from_value(marker["id"].clone())?,
                visibility: ProjectedTurnItemVisibility::Synthetic,
                item: marker,
                synthetic: true,
                position: 0,
            });
        }
    }
    for item in &items {
        if visible(item, &runs, &attempts, &items, false) {
            result.push(TimelineRow {
                source: id.clone(),
                id: serde_json::from_value(item["id"].clone())?,
                visibility: ProjectedTurnItemVisibility::Local,
                item: item.clone(),
                synthetic: false,
                position: 0,
            });
        }
    }
    for (position, row) in result.iter_mut().enumerate() {
        row.position = position as u64;
    }
    Ok(result)
}

fn retained_window_items(
    db: &Connection,
    id: &ThreadId,
    window: &SnapshotWindow,
    fork_at: Option<&str>,
) -> Result<Vec<OrchestrationV2TurnItem>> {
    use t3_persistence::rusqlite::{params_from_iter, types::Value as SqlValue};
    let mut args = vec![
        SqlValue::Text(id.0.clone()),
        SqlValue::Integer(i64::from(window.defer_work)),
        window
            .anchor_item_id
            .as_ref()
            .map(|v| SqlValue::Text(v.0.clone()))
            .unwrap_or(SqlValue::Null),
        window
            .required_run_id
            .as_ref()
            .map(|v| SqlValue::Text(v.0.clone()))
            .unwrap_or(SqlValue::Null),
        fork_at
            .map(|v| SqlValue::Text(v.into()))
            .unwrap_or(SqlValue::Null),
        window
            .user_turn_limit
            .map(integer)
            .transpose()?
            .map(SqlValue::Integer)
            .unwrap_or(SqlValue::Null),
        integer(window.row_limit)?.into(),
        integer(window.user_turn_limit.unwrap_or(0).saturating_add(2))?.into(),
        0.into(),
        SqlValue::Null,
        0.into(),
        0.into(),
        integer(window.row_limit.max(502))?.into(),
    ];
    let (boundary, latest): (i64, Option<i64>) =
        db.query_row(WINDOW_ANCHORS_SQL, params_from_iter(args.iter()), |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    args[8] = boundary.into();
    args[9] = latest.map(SqlValue::Integer).unwrap_or(SqlValue::Null);
    if window.defer_work && latest.is_some() {
        let (tail, long): (Option<i64>, i64) =
            db.query_row(WINDOW_TAIL_SQL, params_from_iter(args.iter()), |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
        args[10] = boundary.max(tail.unwrap_or(0)).into();
        args[11] = long.into();
    } else {
        args[10] = boundary.into();
    }
    let sql = format!(
        "{WINDOW_RETAINED_SQL} SELECT item.payload_json,tail.turn_item_id,deferred.last_ordinal,deferred.item_count FROM retained CROSS JOIN orchestration_v2_projection_turn_items item ON item.rowid=retained.item_rowid LEFT JOIN deferred_work deferred ON deferred.run_id=item.run_id AND deferred.first_ordinal=item.ordinal LEFT JOIN orchestration_v2_projection_turn_items tail ON tail.turn_item_id=(SELECT turn_item_id FROM orchestration_v2_projection_turn_items WHERE thread_id=item.thread_id AND ordinal=deferred.last_ordinal ORDER BY turn_item_id DESC LIMIT 1) ORDER BY retained.ordinal,retained.turn_item_id"
    );
    let mut stmt = db.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, Option<i64>>(3)?,
        ))
    })?;
    let mut result = Vec::new();
    for row in rows {
        let (raw, last_id, last_ordinal, count) = row?;
        let mut value: Value = serde_json::from_str(&raw)?;
        if let (Some(last_id), Some(last), Some(count)) = (last_id, last_ordinal, count) {
            let position = u64::try_from(last).map_err(|_| Error::Decode {
                entity: "deferred work ordinal",
                detail: "negative ordinal".into(),
            })?;
            let cursor =
                crate::ports::transport::thread_history_paging::encode_thread_history_cursor(
                    &crate::ports::transport::thread_history_paging::ThreadHistoryCursorPayload {
                        v: 1,
                        seq: 0,
                        st: id.0.clone(),
                        si: last_id,
                        p: position,
                        inclusive: Some(true),
                    },
                )?;
            value["deferredWork"] =
                serde_json::json!({"cursor":cursor,"itemCount":count,"lastOrdinal":last});
        }
        result.push(serde_json::from_value(value)?);
    }
    Ok(result)
}

fn window_rows<T: DeserializeOwned>(
    db: &Connection,
    id: &ThreadId,
    cohort: &str,
    table: &str,
    condition: &str,
    order: &str,
) -> Result<Vec<T>> {
    rows(
        db,
        &format!(
            "SELECT payload_json FROM orchestration_v2_projection_{table} WHERE thread_id=?1 AND ({condition}) ORDER BY {order}"
        ),
        params![id.0, cohort],
    )
}
fn canonical_window(
    db: &Connection,
    id: &ThreadId,
    window: &SnapshotWindow,
    fork_at: Option<&str>,
) -> Result<OrchestrationV2ThreadProjection> {
    let thread = require_thread(db, id)?;
    let items = retained_window_items(db, id, window, fork_at)?;
    let mut cohort = serde_json::Map::new();
    for key in [
        "runId",
        "nodeId",
        "providerThreadId",
        "providerTurnId",
        "messageId",
        "planId",
        "checkpointId",
        "contextHandoffId",
    ] {
        cohort.insert(
            key.into(),
            values(&items)?
                .into_iter()
                .filter_map(|v| v[key].as_str().map(|s| Value::String(s.into())))
                .collect::<Vec<_>>()
                .into(),
        );
    }
    cohort.insert(
        "requiredRunId".into(),
        serde_json::to_value(&window.required_run_id)?,
    );
    let mut cohort = serde_json::to_string(&cohort)?;
    let latest_todo:Value=one(db,"WITH owner AS MATERIALIZED(SELECT plan.plan_id,r.run_id,r.provider_thread_id FROM orchestration_v2_projection_plans plan JOIN orchestration_v2_projection_runs r ON r.run_id=plan.run_id AND r.thread_id=plan.thread_id WHERE plan.thread_id=?1 AND plan.kind='todo_list' AND plan.status<>'superseded' ORDER BY r.ordinal DESC,plan.plan_id DESC LIMIT 1) SELECT json_object('planId',COALESCE((SELECT plan.plan_id FROM orchestration_v2_projection_turn_items item CROSS JOIN orchestration_v2_projection_plans plan ON plan.plan_id=json_extract(item.payload_json,'$.planId') WHERE item.run_id=owner.run_id AND item.thread_id=?1 AND item.type='todo_list' AND plan.run_id=owner.run_id AND plan.thread_id=?1 AND plan.kind='todo_list' AND plan.status<>'superseded' ORDER BY item.ordinal DESC LIMIT 1),owner.plan_id),'runId',owner.run_id,'providerThreadId',owner.provider_thread_id,'lastPlanId',(SELECT MAX(plan_id) FROM orchestration_v2_projection_plans WHERE thread_id=?1 AND run_id=owner.run_id)) FROM owner",[&id.0])?.unwrap_or(Value::Null);
    let mut enriched: Value = serde_json::from_str(&cohort)?;
    enriched["latestTodo"] = latest_todo.clone();
    if window.anchor_item_id.is_none() && window.required_run_id.is_none() {
        let presented: Value = one(db, &presented_run_sql(), [&id.0])?.unwrap_or(Value::Null);
        enriched["presentedRunId"] = presented["id"].clone();
    }
    cohort = serde_json::to_string(&enriched)?;
    let node_ids: Vec<String> = {
        let mut s=db.prepare("WITH RECURSIVE retained(node_id) AS(SELECT value FROM json_each(?2,'$.nodeId') UNION SELECT node_id FROM orchestration_v2_projection_nodes live INDEXED BY orchestration_v2_projection_nodes_live_idx WHERE thread_id=?1 AND status IN('pending','starting','running','waiting') AND NOT(kind='todo_list' AND run_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM orchestration_v2_projection_runs r WHERE r.run_id=live.run_id AND r.status IN('queued','preparing','starting','running','waiting'))) UNION SELECT root_node_id FROM orchestration_v2_projection_run_attempts WHERE thread_id=?1 AND(status IN('pending','starting','running','waiting') OR run_id IN(SELECT value FROM json_each(?2,'$.runId')) OR run_id=json_extract(?2,'$.requiredRunId')) UNION SELECT json_extract(payload_json,'$.rootNodeId') FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND(status IN('queued','preparing','starting','running','waiting') OR run_id IN(SELECT value FROM json_each(?2,'$.runId')) OR run_id=json_extract(?2,'$.requiredRunId')) UNION SELECT node_id FROM orchestration_v2_projection_runtime_requests WHERE thread_id=?1 AND status IN('pending','waiting') UNION SELECT parent_node_id FROM orchestration_v2_projection_subagents WHERE thread_id=?1 AND status IN('pending','starting','running','waiting') UNION SELECT node_id FROM orchestration_v2_projection_provider_turns WHERE thread_id=?1 AND status IN('starting','running','waiting') UNION SELECT parent.parent_node_id FROM orchestration_v2_projection_nodes parent JOIN retained ON parent.node_id=retained.node_id WHERE parent.thread_id=?1 AND parent.parent_node_id IS NOT NULL) SELECT retained.node_id FROM retained LEFT JOIN orchestration_v2_projection_nodes node ON node.node_id=retained.node_id AND node.thread_id=?1 WHERE retained.node_id IS NOT NULL ORDER BY COALESCE(node.started_at,''),retained.node_id")?;
        s.query_map(params![id.0, cohort], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?
    };
    enriched["nodeId"] = serde_json::to_value(node_ids)?;
    cohort = serde_json::to_string(&enriched)?;
    let nodes = rows(
        db,
        "SELECT node.payload_json FROM json_each(?2,'$.nodeId') wanted CROSS JOIN orchestration_v2_projection_nodes node ON node.node_id=wanted.value WHERE node.thread_id=?1 ORDER BY wanted.key",
        params![id.0, cohort],
    )?;
    let runs = window_rows(
        db,
        id,
        &cohort,
        "runs",
        "status IN('queued','preparing','starting','running','waiting') OR run_id IN(SELECT value FROM json_each(?2,'$.runId')) OR run_id=json_extract(?2,'$.requiredRunId') OR run_id=json_extract(?2,'$.presentedRunId') OR run_id=json_extract(?2,'$.latestTodo.runId') OR run_id=(SELECT run_id FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND provider_thread_id IS NOT NULL AND json_extract(payload_json,'$.startedAt') IS NOT NULL AND json_extract(?2,'$.latestTodo.planId') IS NOT NULL ORDER BY ordinal DESC LIMIT 1)",
        "ordinal",
    )?;
    let attempts = window_rows(
        db,
        id,
        &cohort,
        "run_attempts",
        "status IN('pending','starting','running','waiting') OR run_id IN(SELECT value FROM json_each(?2,'$.runId'))",
        "run_id,attempt_ordinal",
    )?;
    let subagents = window_rows(
        db,
        id,
        &cohort,
        "subagents",
        "status IN('pending','starting','running','waiting') OR run_id IN(SELECT value FROM json_each(?2,'$.runId')) OR parent_node_id IN(SELECT value FROM json_each(?2,'$.nodeId'))",
        "COALESCE(started_at,''),subagent_id",
    )?;
    let provider_sessions = rows(
        db,
        "SELECT DISTINCT sessions.payload_json FROM orchestration_v2_projection_provider_sessions sessions JOIN orchestration_v2_projection_provider_session_bindings bindings ON bindings.provider_session_id=sessions.provider_session_id LEFT JOIN orchestration_v2_projection_provider_threads threads ON threads.provider_session_id=sessions.provider_session_id WHERE bindings.thread_id=?1 AND(sessions.status IN('starting','running','waiting') OR threads.provider_thread_id IN(SELECT value FROM json_each(?2,'$.providerThreadId'))) ORDER BY sessions.updated_at,sessions.provider_session_id",
        params![id.0, cohort],
    )?;
    let provider_threads = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_provider_threads WHERE(thread_id=?1 AND status='active') OR provider_thread_id IN(SELECT value FROM json_each(?2,'$.providerThreadId')) OR owner_node_id IN(SELECT value FROM json_each(?2,'$.nodeId')) OR(thread_id=?1 AND provider_thread_id=json_extract(?2,'$.latestTodo.providerThreadId')) ORDER BY COALESCE(first_run_ordinal,0),provider_thread_id",
        params![id.0, cohort],
    )?;
    let provider_turns = window_rows(
        db,
        id,
        &cohort,
        "provider_turns",
        "status IN('starting','running','waiting') OR provider_turn_id IN(SELECT value FROM json_each(?2,'$.providerTurnId')) OR node_id IN(SELECT value FROM json_each(?2,'$.nodeId'))",
        "provider_thread_id,ordinal",
    )?;
    let runtime_requests = window_rows(
        db,
        id,
        &cohort,
        "runtime_requests",
        "status IN('pending','waiting') OR node_id IN(SELECT value FROM json_each(?2,'$.nodeId')) OR provider_turn_id IN(SELECT value FROM json_each(?2,'$.providerTurnId'))",
        "created_at,runtime_request_id",
    )?;
    let mut messages = rows(
        db,
        "SELECT payload_json FROM(SELECT message.payload_json,message.created_at,message.message_id FROM json_each(?2,'$.messageId') wanted CROSS JOIN orchestration_v2_projection_messages message ON message.message_id=wanted.value WHERE message.thread_id=?1 UNION SELECT message.payload_json,message.created_at,message.message_id FROM orchestration_v2_projection_runs run CROSS JOIN orchestration_v2_projection_messages message ON message.run_id=run.run_id WHERE run.thread_id=?1 AND run.status IN('queued','preparing','starting','running','waiting') AND message.thread_id=?1) ORDER BY created_at,message_id",
        params![id.0, cohort],
    )?;
    order_messages(db, id, &mut messages)?;
    let plans = window_rows(
        db,
        id,
        &cohort,
        "plans",
        "(status='active' AND NOT(kind='todo_list' AND run_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM orchestration_v2_projection_runs r WHERE r.run_id=orchestration_v2_projection_plans.run_id AND r.status IN('queued','preparing','starting','running','waiting')))) OR plan_id=json_extract(?2,'$.latestTodo.planId') OR plan_id IN(SELECT value FROM json_each(?2,'$.planId'))",
        "CASE WHEN plan_id=json_extract(?2,'$.latestTodo.planId') THEN json_extract(?2,'$.latestTodo.lastPlanId') ELSE plan_id END,plan_id=json_extract(?2,'$.latestTodo.planId')",
    )?;
    let checkpoint_scopes = window_rows(
        db,
        id,
        &cohort,
        "checkpoint_scopes",
        "run_id IN(SELECT value FROM json_each(?2,'$.runId')) OR node_id IN(SELECT value FROM json_each(?2,'$.nodeId'))",
        "ordinal_within_parent,scope_id",
    )?;
    let checkpoints = window_rows(
        db,
        id,
        &cohort,
        "checkpoints",
        "status IN('pending','capturing') OR(run_id IS NULL AND ordinal_within_scope=0 AND status='ready') OR checkpoint_id=(SELECT checkpoint_id FROM orchestration_v2_projection_checkpoints WHERE thread_id=?1 AND status='ready' AND app_run_ordinal IS NOT NULL ORDER BY app_run_ordinal DESC LIMIT 1) OR checkpoint_id IN(SELECT value FROM json_each(?2,'$.checkpointId')) OR run_id IN(SELECT value FROM json_each(?2,'$.runId')) OR node_id IN(SELECT value FROM json_each(?2,'$.nodeId'))",
        "scope_id,ordinal_within_scope",
    )?;
    let context_handoffs = rows(
        db,
        "SELECT json_remove(payload_json,'$.history','$.delivery') FROM orchestration_v2_projection_context_handoffs WHERE thread_id=?1 AND(status IN('pending','ready') OR context_handoff_id IN(SELECT value FROM json_each(?2,'$.contextHandoffId'))) ORDER BY rowid",
        params![id.0, cohort],
    )?;
    let context_transfers = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_context_transfers WHERE(source_thread_id=?1 OR target_thread_id=?1) AND(status IN('pending','running','waiting') OR target_run_id IN(SELECT value FROM json_each(?2,'$.runId')) OR json_extract(payload_json,'$.contextHandoffId') IN(SELECT value FROM json_each(?2,'$.contextHandoffId'))) ORDER BY rowid",
        params![id.0, cohort],
    )?;
    Ok(OrchestrationV2ThreadProjection {
        updated_at: thread.updated_at.clone(),
        thread,
        runs,
        attempts,
        nodes,
        subagents,
        provider_sessions,
        provider_threads,
        provider_turns,
        runtime_requests,
        messages,
        plans,
        turn_items: items,
        checkpoint_scopes,
        checkpoints,
        context_handoffs,
        context_transfers,
        visible_turn_items: vec![],
    })
}

pub fn snapshot_window(
    db: &Connection,
    id: &ThreadId,
    window: &SnapshotWindow,
) -> Result<ProjectionSnapshot> {
    let mut window = window.clone();
    if window.anchor_thread_id.is_none()
        && let Some(anchor) = &window.anchor_item_id
    {
        window.anchor_thread_id=db.query_row("SELECT thread_id FROM orchestration_v2_projection_turn_items WHERE turn_item_id=?1 LIMIT 1",[&anchor.0],|r|Ok(ThreadId(r.get(0)?))).optional()?.or_else(||Some(id.clone()));
    }
    let mut projection = bounded_projection(db, id, &window, None, &mut BTreeSet::new())?;
    let cards: Vec<OrchestrationV2TurnItem> = rows(
        db,
        "SELECT item.payload_json FROM orchestration_v2_projection_runtime_requests request CROSS JOIN orchestration_v2_projection_turn_items item ON item.node_id=request.node_id AND item.type IN('approval_request','user_input_request') AND json_extract(item.payload_json,'$.requestId')=request.runtime_request_id WHERE request.thread_id=?1 AND request.status='pending' AND item.thread_id=?1",
        [&id.0],
    )?;
    let mut added_cards = false;
    for card in cards {
        let value = serde_json::to_value(&card)?;
        if !projection
            .turn_items
            .iter()
            .any(|i| serde_json::to_value(i).is_ok_and(|i| i["id"] == value["id"]))
        {
            projection.turn_items.push(card);
            added_cards = true;
        }
    }
    if added_cards {
        use crate::ports::transport::thread_history_paging::{id_text, item_id, item_ordinal};
        let collator = id_collator()?;
        projection.turn_items.sort_by(|a, b| {
            item_ordinal(a)
                .cmp(&item_ordinal(b))
                .then_with(|| collator.compare(&id_text(item_id(a)), &id_text(item_id(b))))
        });
    }
    Ok(ProjectionSnapshot {
        schema_version: 2,
        snapshot_sequence: thread_sequence(db, id)?,
        projection,
    })
}
fn thread_sequence(db: &Connection, id: &ThreadId) -> Result<u64> {
    Ok(db.query_row::<i64,_,_>("SELECT COALESCE(MAX(sequence),0) FROM orchestration_events WHERE application_event_version=2 AND aggregate_kind='thread' AND stream_id=?1",[&id.0],|r|r.get(0))? as u64)
}

fn history_user(row: &TimelineRow) -> bool {
    row.item["type"] == "user_message"
        && row.item["createdBy"] == "user"
        && matches!(
            row.item["inputIntent"].as_str(),
            Some("turn_start" | "queued_turn")
        )
}
fn history_cursor(row: &TimelineRow, sequence: u64) -> Result<String> {
    crate::ports::transport::thread_history_paging::encode_thread_history_cursor(
        &crate::ports::transport::thread_history_paging::ThreadHistoryCursorPayload {
            v: 1,
            seq: sequence,
            st: row.source.0.clone(),
            si: row.id.0.clone(),
            p: row.position,
            inclusive: None,
        },
    )
}
fn history_end(
    rows: &[TimelineRow],
    cursor: &crate::ports::transport::thread_history_paging::ThreadHistoryCursorPayload,
) -> usize {
    rows.iter()
        .position(|r| r.source.0 == cursor.st && r.id.0 == cursor.si)
        .map(|i| i + usize::from(cursor.inclusive == Some(true)))
        .unwrap_or_else(|| {
            rows.iter()
                .position(|r| {
                    if cursor.inclusive == Some(true) {
                        r.source.0 == cursor.st
                            && r.item["ordinal"].as_u64().is_some_and(|v| v > cursor.p)
                    } else {
                        r.position >= cursor.p
                    }
                })
                .unwrap_or(rows.len())
        })
}
fn history_selection(
    rows: &[TimelineRow],
    end: usize,
    policy: crate::ports::transport::thread_history_paging::ThreadHistoryPagePolicy,
) -> Vec<TimelineRow> {
    history_selection_with_prefix(rows, end, policy, &HistoryPrefix::default())
}
#[derive(Default)]
struct HistoryPrefix {
    has_user_turn: bool,
    started_runs: BTreeSet<String>,
}
fn history_selection_with_prefix(
    rows: &[TimelineRow],
    end: usize,
    policy: crate::ports::transport::thread_history_paging::ThreadHistoryPagePolicy,
    prefix: &HistoryPrefix,
) -> Vec<TimelineRow> {
    history_selection_scan(rows, end, policy, prefix).0
}
fn history_selection_scan(
    rows: &[TimelineRow],
    end: usize,
    policy: crate::ports::transport::thread_history_paging::ThreadHistoryPagePolicy,
    prefix: &HistoryPrefix,
) -> (Vec<TimelineRow>, Option<TimelineRow>) {
    let end = end.min(rows.len());
    let rows = &rows[..end];
    let has_turns = prefix.has_user_turn || rows.iter().any(history_user);
    let limit = if has_turns {
        policy.max_turn_items
    } else {
        policy.max_items
    };
    let mut starts = prefix.started_runs.clone();
    let raw_starts = rows
        .iter()
        .map(|r| match r.item["type"].as_str() {
            Some("user_message") => {
                if let Some(run) = r.item["runId"].as_str() {
                    starts.insert(run.to_owned());
                }
                matches!(
                    r.item["inputIntent"].as_str(),
                    Some("turn_start" | "queued_turn")
                )
            }
            Some("notification") => r.item["runId"]
                .as_str()
                .is_some_and(|run| starts.insert(run.into())),
            _ => false,
        })
        .collect::<Vec<_>>();
    let mut count = 0;
    let mut raw = 0;
    let mut selected = Vec::new();
    for (index, row) in rows.iter().enumerate().rev() {
        if !selected.is_empty()
            && (selected.len() >= limit
                || (has_turns
                    && policy
                        .max_user_turns
                        .is_some_and(|l| count >= l || raw >= 150)))
        {
            break;
        }
        selected.push(row.clone());
        count += usize::from(history_user(row));
        raw += usize::from(raw_starts[index]);
    }
    let oldest_scanned = selected.last().cloned();
    if count == 0 {
        selected.truncate(policy.max_items);
    }
    selected.reverse();
    (selected, oldest_scanned)
}
fn work_rows(db: &Connection, rows: &mut [TimelineRow]) -> Result<()> {
    let sources: Vec<_> = rows
        .iter()
        .filter(|r| !r.synthetic)
        .map(|r| serde_json::json!({"threadId":r.source.0,"id":r.id.0}))
        .collect();
    let metadata: Vec<Value> = crate::projections::read::rows(
        db,
        "SELECT json_object('id',item.turn_item_id,'threadId',item.thread_id,'runStatus',run.status,'attemptId',attempt.attempt_id,'attemptStatus',attempt.status,'persistentOutput',item.status='completed' AND item.type='dynamic_tool' AND(item.payload_json LIKE '%htmlRender%' OR item.payload_json LIKE '%t3McpApp%' OR item.payload_json LIKE '%\"viewedImagePath\"%' OR item.payload_json LIKE '%\"toolName\":\"%delegate_task\"%' OR item.payload_json LIKE '%\"toolName\":\"%send_to_user\"%' OR item.payload_json LIKE '%sentMessage%')) FROM json_each(?1) wanted CROSS JOIN orchestration_v2_projection_turn_items item ON item.turn_item_id=json_extract(wanted.value,'$.id') AND item.thread_id=json_extract(wanted.value,'$.threadId') LEFT JOIN orchestration_v2_projection_runs run ON run.run_id=item.run_id LEFT JOIN orchestration_v2_projection_nodes node ON node.node_id=item.node_id LEFT JOIN orchestration_v2_projection_run_attempts attempt ON attempt.attempt_id=(SELECT attempt_id FROM orchestration_v2_projection_run_attempts WHERE run_id=item.run_id AND root_node_id=COALESCE(node.root_node_id,item.node_id) ORDER BY attempt_ordinal DESC LIMIT 1)",
        [serde_json::to_string(&sources)?],
    )?;
    let metadata: BTreeMap<_, _> = metadata
        .into_iter()
        .map(|v| {
            (
                (
                    v["threadId"].as_str().unwrap_or("").to_owned(),
                    v["id"].as_str().unwrap_or("").to_owned(),
                ),
                v,
            )
        })
        .collect();
    for row in rows {
        if let Some(meta) = metadata.get(&(row.source.0.clone(), row.id.0.clone())) {
            for key in [
                "runStatus",
                "attemptId",
                "attemptStatus",
                "persistentOutput",
            ] {
                row.item[key] = meta[key].clone();
            }
        }
    }
    Ok(())
}

const LOCAL_TIMELINE_COLUMNS: &str = "json_object('id',turn_item_id,'threadId',thread_id,'runId',run_id,'nodeId',node_id,'parentItemId',parent_item_id,'type',type,'status',status,'ordinal',ordinal,'inputIntent',CASE WHEN type='user_message' THEN json_extract(payload_json,'$.inputIntent') END,'createdBy',CASE WHEN type='user_message' THEN json_extract(payload_json,'$.createdBy') END,'messageId',CASE WHEN type IN('user_message','assistant_message') THEN json_extract(payload_json,'$.messageId') END)";
fn local_timeline_rows(
    db: &Connection,
    id: &ThreadId,
    predicate: &str,
    args: impl Params,
) -> Result<Vec<TimelineRow>> {
    let items: Vec<Value> = rows(
        db,
        &format!(
            "SELECT {LOCAL_TIMELINE_COLUMNS} FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 AND {predicate} ORDER BY ordinal,turn_item_id"
        ),
        args,
    )?;
    items
        .into_iter()
        .map(|item| {
            Ok(TimelineRow {
                source: id.clone(),
                id: serde_json::from_value(item["id"].clone())?,
                visibility: ProjectedTurnItemVisibility::Local,
                item,
                synthetic: false,
                position: 0,
            })
        })
        .collect()
}
struct HistoryIndex {
    rows: Vec<TimelineRow>,
    prefix: HistoryPrefix,
    interrupt_requests: Vec<TimelineRow>,
}
struct LocalVisibility {
    runs: Vec<Value>,
    attempts: Vec<Value>,
    stops: Vec<Value>,
    hidden: Vec<Value>,
}
impl LocalVisibility {
    fn contains(&self, item: &Value) -> bool {
        visible(item, &self.runs, &self.attempts, &self.stops, false)
    }
    fn before(&self, db: &Connection, id: &ThreadId, ordinal: i64, item_id: &str) -> Result<u64> {
        let raw:i64=db.query_row("SELECT COUNT(*) FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 AND (ordinal,turn_item_id)<(?2,?3)",params![id.0,ordinal,item_id],|r|r.get(0))?;
        let hidden = self
            .hidden
            .iter()
            .filter(|i| {
                i["ordinal"].as_i64().is_some_and(|o| {
                    o < ordinal
                        || (o == ordinal
                            && i["id"]
                                .as_str()
                                .is_some_and(|v| v.as_bytes() < item_id.as_bytes()))
                })
            })
            .count();
        Ok((raw as u64).saturating_sub(hidden as u64))
    }
}
fn local_visibility(db: &Connection, id: &ThreadId) -> Result<LocalVisibility> {
    let loaded = from_records(records(
        db,
        id,
        &[RecordKind::Runs, RecordKind::Attempts],
        &RecordFilter::default(),
    )?);
    let runs = values(&loaded.runs)?;
    let attempts = values(&loaded.attempts)?;
    let stops: Vec<Value> = rows(
        db,
        "SELECT json_object('type','run_interrupt_request','runId',run_id) FROM orchestration_v2_projection_turn_items INDEXED BY orchestration_v2_projection_turn_items_stop_request_idx WHERE thread_id=?1 AND type='run_interrupt_request'",
        [&id.0],
    )?;
    let by_status = |statuses: &[&str]| {
        runs.iter()
            .filter(|r| r["status"].as_str().is_some_and(|s| statuses.contains(&s)))
            .map(|r| r["id"].clone())
            .collect::<Vec<_>>()
    };
    let rolled = by_status(&["rolled_back"]);
    let held = by_status(&["queued", "cancelled"]);
    let steered = attempts
        .iter()
        .filter(|a| {
            a["status"] == "superseded"
                && !stops.iter().any(|s| s["runId"] == a["runId"])
                && !rolled.contains(&a["runId"])
        })
        .map(|a| a["runId"].clone())
        .collect::<Vec<_>>();
    let columns = "json_object('id',item.turn_item_id,'runId',item.run_id,'nodeId',item.node_id,'type',item.type,'ordinal',item.ordinal,'inputIntent',CASE WHEN item.type='user_message' THEN json_extract(item.payload_json,'$.inputIntent') END)";
    let mut candidates = if rolled.is_empty() {
        vec![]
    } else {
        rows::<Value>(
            db,
            &format!(
                "SELECT {columns} FROM json_each(?2) AS wanted CROSS JOIN orchestration_v2_projection_turn_items AS item INDEXED BY orchestration_v2_projection_turn_items_thread_run_idx ON item.thread_id=?1 AND item.run_id=wanted.value"
            ),
            params![id.0, serde_json::to_string(&rolled)?],
        )?
    };
    if !held.is_empty() || !steered.is_empty() {
        candidates.extend(rows::<Value>(db,&format!("SELECT {columns} FROM orchestration_v2_projection_turn_items AS item INDEXED BY orchestration_v2_projection_turn_items_conversation_idx WHERE item.thread_id=?1 AND item.type NOT IN('command_execution','reasoning','file_change','file_search','web_search','dynamic_tool') AND ((item.type='user_message' AND item.run_id IN(SELECT value FROM json_each(?2))) OR (item.type='run_interrupt_result' AND item.run_id IN(SELECT value FROM json_each(?3))))"),params![id.0,serde_json::to_string(&held)?,serde_json::to_string(&steered)?])?);
    }
    let hidden = candidates
        .into_iter()
        .filter(|i| !visible(i, &runs, &attempts, &stops, false))
        .collect();
    Ok(LocalVisibility {
        runs,
        attempts,
        stops,
        hidden,
    })
}
fn local_history_index(
    db: &Connection,
    thread: &OrchestrationV2AppThread,
    query: &HistoryQuery,
    cursor: &crate::ports::transport::thread_history_paging::ThreadHistoryCursorPayload,
) -> Result<Option<HistoryIndex>> {
    let raw = t3_contracts::stored::to_value(thread)?;
    if (raw["forkedFrom"]["type"] == "run" && raw["lineage"]["sideChat"] != true)
        || cursor.st != query.thread_id.0
        || (matches!(query.work_mode, Some(HistoryWorkMode::Work))
            != query.through_entry_id.is_some())
    {
        return Ok(None);
    }
    let id = &query.thread_id;
    let upper:Option<i64>=db.query_row("SELECT ordinal FROM orchestration_v2_projection_turn_items WHERE turn_item_id=?1 AND thread_id=?2",params![cursor.si,id.0],|r|r.get(0)).optional()?;
    let Some(upper) = upper else {
        return Ok(None);
    };
    let local = local_visibility(db, id)?;
    let runs = &local.runs;
    let attempts = &local.attempts;
    let stop_rows = local_timeline_rows(db, id, "type='run_interrupt_request'", [&id.0])?;
    let stops = &local.stops;
    let hidden = &local.hidden;
    let base_before = |lower| local.before(db, id, lower, "");
    let mut count = 502i64;
    loop {
        let trusted:Option<i64>=db.query_row("SELECT ordinal FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 AND ordinal<?2 ORDER BY ordinal DESC LIMIT 1 OFFSET ?3",params![id.0,upper,count-1],|r|r.get(0)).optional()?;
        let mut lower = if matches!(query.work_mode, Some(HistoryWorkMode::Work)) {
            db.query_row("SELECT ordinal FROM orchestration_v2_projection_turn_items WHERE turn_item_id=?1 AND thread_id=?2",params![query.through_entry_id,id.0],|r|r.get(0)).optional()?
        } else {
            trusted
        };
        if matches!(query.work_mode, Some(HistoryWorkMode::Work)) && lower.is_none() {
            return Ok(None);
        }
        let mut higher = upper;
        let mut rows = local_timeline_rows(
            db,
            id,
            "ordinal>=?2 AND ordinal<=?3",
            params![id.0, lower.unwrap_or(i64::MIN), higher],
        )?;
        if query.work_mode.is_some() {
            work_rows(db, &mut rows)?;
        }
        if matches!(query.work_mode, Some(HistoryWorkMode::Deferred)) {
            let folding = rows
                .iter()
                .filter(|r| {
                    deferrable(r)
                        && (r.item["runStatus"] == "completed"
                            || r.item["attemptStatus"] == "superseded")
                })
                .filter_map(|r| r.item["runId"].as_str())
                .collect::<BTreeSet<_>>();
            if !folding.is_empty() {
                let extent:(Option<i64>,Option<i64>)=db.query_row("SELECT MIN(ordinal),MAX(ordinal) FROM orchestration_v2_projection_turn_items WHERE +thread_id=?1 AND run_id IN(SELECT value FROM json_each(?2))",params![id.0,serde_json::to_string(&folding)?],|r|Ok((r.get(0)?,r.get(1)?)))?;
                let next_lower = lower.map(|l| l.min(extent.0.unwrap_or(l)));
                let next_higher = higher.max(extent.1.unwrap_or(higher));
                if next_lower != lower || next_higher != higher {
                    lower = next_lower;
                    higher = next_higher;
                    rows = local_timeline_rows(
                        db,
                        id,
                        "ordinal>=?2 AND ordinal<=?3",
                        params![id.0, lower.unwrap_or(i64::MIN), higher],
                    )?;
                    work_rows(db, &mut rows)?;
                }
            }
        }
        rows.retain(|r| visible(&r.item, runs, attempts, stops, false));
        if !rows.iter().any(|r| r.id.0 == cursor.si) {
            return Ok(None);
        }
        let base = lower.map(base_before).transpose()?.unwrap_or(0);
        for (offset, row) in rows.iter_mut().enumerate() {
            row.position = base + offset as u64;
        }
        let end = history_end(&rows, cursor);
        let has_user_turn = lower.is_some()
            && !rows[..end].iter().any(history_user)
            && {
                let mut statement = db.prepare("SELECT run_id,node_id,json_extract(payload_json,'$.inputIntent') FROM orchestration_v2_projection_turn_items INDEXED BY orchestration_v2_projection_turn_items_user_message_idx WHERE thread_id=?1 AND type='user_message' AND ordinal<?2 AND json_extract(payload_json,'$.inputIntent') IN('turn_start','queued_turn') AND json_extract(payload_json,'$.createdBy')='user' ORDER BY ordinal DESC,turn_item_id DESC LIMIT ?3")?;
                let older = statement.query_map(params![id.0, lower, hidden.len() as i64 + 1], |r| {
                    Ok(serde_json::json!({"type":"user_message","runId":r.get::<_,Option<String>>(0)?,"nodeId":r.get::<_,Option<String>>(1)?,"inputIntent":r.get::<_,String>(2)?}))
                })?.collect::<std::result::Result<Vec<_>,_>>()?;
                older
                    .iter()
                    .any(|item| visible(item, runs, attempts, stops, false))
            };
        let mut started_runs = BTreeSet::new();
        if let Some(lower) = lower {
            let mut seen = BTreeSet::new();
            let unknown = rows
                .iter()
                .filter(|r| {
                    matches!(
                        r.item["type"].as_str(),
                        Some("user_message" | "notification")
                    )
                })
                .filter_map(|r| {
                    let run = r.item["runId"].as_str()?;
                    let first = seen.insert(run.to_owned());
                    (first && r.item["type"] == "notification").then_some(run)
                })
                .collect::<BTreeSet<_>>();
            if !unknown.is_empty() {
                let mut statement = db.prepare("SELECT wanted.value,(SELECT COUNT(*) FROM orchestration_v2_projection_turn_items AS input INDEXED BY orchestration_v2_projection_turn_items_conversation_idx WHERE input.thread_id=?1 AND input.ordinal>=(SELECT MIN(ordinal) FROM orchestration_v2_projection_turn_items WHERE run_id=wanted.value) AND input.ordinal<?2 AND input.type NOT IN('command_execution','reasoning','file_change','file_search','web_search','dynamic_tool') AND input.type IN('user_message','notification') AND input.run_id=wanted.value) FROM json_each(?3) AS wanted")?;
                let counts = statement
                    .query_map(
                        params![id.0, lower, serde_json::to_string(&unknown)?],
                        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
                    )?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                for (run, count) in counts {
                    let hidden_inputs = hidden
                        .iter()
                        .filter(|row| {
                            row["runId"] == run
                                && row["ordinal"].as_i64().is_some_and(|v| v < lower)
                                && matches!(
                                    row["type"].as_str(),
                                    Some("user_message" | "notification")
                                )
                        })
                        .count();
                    if count as usize > hidden_inputs {
                        started_runs.insert(run);
                    }
                }
            }
        }
        let prefix = HistoryPrefix {
            has_user_turn,
            started_runs,
        };
        let folded = if matches!(query.work_mode, Some(HistoryWorkMode::Deferred)) {
            deferred_rows(&rows)?
        } else {
            rows.clone()
        };
        if !folded.iter().any(|r| r.id.0 == cursor.si) {
            return Ok(None);
        }
        let (_, oldest_scanned) = history_selection_scan(
            &folded,
            history_end(&folded, cursor),
            crate::ports::transport::thread_history_paging::ThreadHistoryPagePolicy {
                max_user_turns: Some(20),
                ..Default::default()
            },
            &prefix,
        );
        if lower.is_none()
            || matches!(query.work_mode, Some(HistoryWorkMode::Work))
            || oldest_scanned.as_ref().is_some_and(|r| {
                r.position > base
                    && trusted.is_some_and(|t| r.item["ordinal"].as_i64().is_some_and(|v| v >= t))
            })
        {
            return Ok(Some(HistoryIndex {
                rows,
                prefix,
                interrupt_requests: stop_rows,
            }));
        }
        count = count.saturating_mul(2);
    }
}
fn deferrable(row: &TimelineRow) -> bool {
    row.item["status"] == "completed"
        && row.item["persistentOutput"] != 1
        && matches!(
            row.item["type"].as_str(),
            Some(
                "command_execution"
                    | "reasoning"
                    | "file_change"
                    | "file_search"
                    | "web_search"
                    | "dynamic_tool"
            )
        )
}
fn deferred_rows(rows: &[TimelineRow]) -> Result<Vec<TimelineRow>> {
    let blocked: BTreeSet<_> = rows
        .iter()
        .filter(|r| {
            matches!(
                r.item["type"].as_str(),
                Some("run_interrupt_request" | "run_interrupt_result")
            ) || (r.item["type"] == "error"
                && r.item["status"] == "failed"
                && r.item["parentItemId"].is_null())
        })
        .filter_map(|r| r.item["runId"].as_str())
        .collect();
    let mut segments = BTreeMap::<String, u64>::new();
    let mut groups = BTreeMap::<(String, String, u64, String), Vec<usize>>::new();
    for (index, row) in rows.iter().enumerate() {
        let Some(run) = row.item["runId"].as_str() else {
            continue;
        };
        let segment = segments.entry(run.into()).or_default();
        if row.item["type"] == "user_message" {
            *segment += 1;
        }
        if !blocked.contains(run)
            && deferrable(row)
            && (row.item["runStatus"] == "completed" || row.item["attemptStatus"] == "superseded")
        {
            groups
                .entry((
                    row.source.0.clone(),
                    run.into(),
                    *segment,
                    row.item["attemptId"].as_str().unwrap_or("").into(),
                ))
                .or_default()
                .push(index);
        }
    }
    let mut result = rows.to_vec();
    let mut omitted = BTreeSet::new();
    for group in groups.values().filter(|g| g.len() > 150) {
        let last = &rows[group[group.len() - 1]];
        let ordinal = last.item["ordinal"].as_u64().unwrap_or(0);
        let cursor = crate::ports::transport::thread_history_paging::encode_thread_history_cursor(
            &crate::ports::transport::thread_history_paging::ThreadHistoryCursorPayload {
                v: 1,
                seq: 0,
                st: last.source.0.clone(),
                si: last.id.0.clone(),
                p: ordinal,
                inclusive: Some(true),
            },
        )?;
        result[group[0]].item["deferredWork"] =
            serde_json::json!({"cursor":cursor,"itemCount":group.len(),"lastOrdinal":ordinal});
        omitted.extend(group.iter().skip(1).copied());
    }
    Ok(result
        .into_iter()
        .enumerate()
        .filter_map(|(i, r)| (!omitted.contains(&i)).then_some(r))
        .collect())
}
pub fn history(db: &Connection, query: &HistoryQuery) -> Result<OrchestrationV2ThreadHistoryPage> {
    history_with_index(db, query, true)
}
pub fn history_from_full_index(
    db: &Connection,
    query: &HistoryQuery,
) -> Result<OrchestrationV2ThreadHistoryPage> {
    history_with_index(db, query, false)
}
fn history_with_index(
    db: &Connection,
    query: &HistoryQuery,
    use_local: bool,
) -> Result<OrchestrationV2ThreadHistoryPage> {
    use crate::ports::transport::{thread_history_paging as paging, wire_projection};
    let thread = require_thread(db, &query.thread_id)?;
    if thread.deleted_at.is_some() {
        return Err(Error::ThreadNotFound(query.thread_id.clone()));
    }
    let cursor = paging::decode_thread_history_cursor(&query.cursor)?;
    let sequence = thread_sequence(db, &query.thread_id)?;
    let local = if use_local {
        local_history_index(db, &thread, query, &cursor)?
    } else {
        None
    };
    let (mut indexed, prefix, interrupt_requests) = if let Some(local) = local {
        (local.rows, local.prefix, local.interrupt_requests)
    } else {
        let rows = timeline_rows(db, &query.thread_id, &mut BTreeSet::new())?;
        let stops = rows
            .iter()
            .filter(|r| r.item["type"] == "run_interrupt_request")
            .cloned()
            .collect();
        (rows, HistoryPrefix::default(), stops)
    };
    if query.work_mode.is_some()
        && indexed
            .iter()
            .any(|r| !r.synthetic && r.item.get("persistentOutput").is_none())
    {
        work_rows(db, &mut indexed)?;
    }
    let mut policy = paging::ThreadHistoryPagePolicy {
        max_user_turns: Some(20),
        ..paging::THREAD_HISTORY_PAGE_POLICY
    };
    let cursor_end = history_end(&indexed, &cursor);
    if let Some(target) = &query.through_entry_id
        && let Some(start) = indexed
            .iter()
            .position(|r| r.item["messageId"].as_str().unwrap_or(&r.id.0) == target)
            .filter(|i| *i < cursor_end)
    {
        let turns = indexed[start..cursor_end]
            .iter()
            .filter(|r| history_user(r))
            .count();
        policy.max_user_turns =
            Some((turns + usize::from(!history_user(&indexed[start]))).clamp(20, 40));
        policy.max_items *= 2;
        policy.max_turn_items *= 2;
    }
    let history = if matches!(query.work_mode, Some(HistoryWorkMode::Deferred)) {
        deferred_rows(&indexed)?
    } else {
        indexed.clone()
    };
    let end = indexed
        .get(cursor_end)
        .and_then(|row| history.iter().position(|r| r.position >= row.position))
        .unwrap_or(history.len());
    let mut page_history_start = history.first().map(|r| r.position);
    let selection = if matches!(query.work_mode, Some(HistoryWorkMode::Work)) {
        policy = paging::THREAD_HISTORY_PAGE_POLICY;
        let target = query
            .through_entry_id
            .as_ref()
            .ok_or_else(|| Error::Decode {
                entity: "work history cursor",
                detail: "missing through entry ID".into(),
            })?;
        let start = indexed
            .iter()
            .position(|r| r.id.0 == *target)
            .filter(|i| *i < cursor_end)
            .ok_or_else(|| Error::Decode {
                entity: "work history cursor",
                detail: "target not before cursor".into(),
            })?;
        let anchor = &indexed[start];
        let rows: Vec<_> = indexed[start..cursor_end]
            .iter()
            .filter(|r| {
                r.source == anchor.source
                    && r.item["runId"] == anchor.item["runId"]
                    && r.item["attemptId"] == anchor.item["attemptId"]
                    && deferrable(r)
            })
            .cloned()
            .collect();
        page_history_start = rows.first().map(|r| r.position);
        history_selection(&rows, rows.len(), policy)
    } else {
        history_selection_with_prefix(&history, end, policy, &prefix)
    };
    let mut has_more = selection
        .first()
        .is_some_and(|first| page_history_start.is_some_and(|start| start < first.position));
    let mut next = if has_more {
        selection
            .first()
            .map(|r| history_cursor(r, sequence))
            .transpose()?
    } else {
        None
    };
    let selected_end = selection.last().map_or(0, |r| r.position + 1);
    let has_user = query.conversation_only
        && (prefix.has_user_turn
            || indexed
                .iter()
                .any(|r| r.position < selected_end && history_user(r)));
    let mut materialized = Vec::new();
    for row in selection.iter().filter(|r| {
        !has_user
            || !matches!(
                r.item["type"].as_str(),
                Some("command_execution" | "reasoning" | "file_change" | "dynamic_tool")
            )
    }) {
        let mut item = materialize(db, row)?;
        if let Some(deferred) = row.item.get("deferredWork") {
            let mut value = serde_json::to_value(&item.item)?;
            value["deferredWork"] = deferred.clone();
            item.item = serde_json::from_value(value)?;
        }
        wire_projection::project_turn_item_for_wire(&mut item.item)?;
        materialized.push(item);
    }
    let bounded = paging::select_recent_timeline_window(&materialized, sequence, policy)?;
    if bounded.has_more_history {
        has_more = true;
        next = bounded.next_cursor;
    }
    let mut items = if matches!(query.work_mode, Some(HistoryWorkMode::Work)) {
        vec![]
    } else {
        paging::missing_interrupt_requests(
            &interrupt_requests
                .iter()
                .filter(|r| r.item["type"] == "run_interrupt_request")
                .map(|r| materialize(db, r))
                .collect::<Result<Vec<_>>>()?,
            &bounded.items,
        )
    };
    items.extend(bounded.items);
    for (position, row) in items.iter_mut().enumerate() {
        row.position = position as u64;
    }
    if query.conversation_only {
        items.retain(|r| paging::is_conversation_history_item(&r.item));
    }
    Ok(OrchestrationV2ThreadHistoryPage {
        snapshot_sequence: sequence,
        items,
        next_cursor: next.map(TrimmedNonEmptyString),
        has_more_history: has_more,
        id_table: None,
        ids: None,
    })
}

fn bounded_projection(
    db: &Connection,
    id: &ThreadId,
    window: &SnapshotWindow,
    fork_at: Option<&str>,
    seen: &mut BTreeSet<String>,
) -> Result<OrchestrationV2ThreadProjection> {
    let mut local = window.clone();
    if window
        .anchor_thread_id
        .as_ref()
        .is_some_and(|anchor| anchor != id)
    {
        local.row_limit = 0;
        local.anchor_item_id = None;
    }
    let mut projection = canonical_window(db, id, &local, fork_at)?;
    let raw = serde_json::to_value(&projection.thread)?;
    let runs = values(&projection.runs)?;
    let attempts = values(&projection.attempts)?;
    let items = values(&projection.turn_items)?;
    let mut visible_items = Vec::new();
    if raw["forkedFrom"]["type"] == "run" && raw["lineage"]["sideChat"] != true {
        let source: ThreadId = serde_json::from_value(raw["forkedFrom"]["threadId"].clone())?;
        if !seen.contains(&source.0) {
            seen.insert(id.0.clone());
            let cutoff: RunId = serde_json::from_value(raw["forkedFrom"]["runId"].clone())?;
            let anchor:Option<TurnItemId>=db.query_row("SELECT turn_item_id FROM orchestration_v2_projection_turn_items item LEFT JOIN orchestration_v2_projection_runs r ON r.run_id=item.run_id WHERE item.thread_id=?1 AND r.ordinal<=(SELECT ordinal FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND run_id=?2) AND(?3 IS NULL OR item.ordinal<=COALESCE((SELECT ordinal FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 AND turn_item_id=?3),9223372036854775807)) ORDER BY item.ordinal DESC,item.turn_item_id DESC LIMIT 1",params![source.0,cutoff.0,window.anchor_item_id.as_ref().filter(|_|window.anchor_thread_id.as_ref()==Some(&source)).map(|i|&i.0)],|r|Ok(TurnItemId(r.get(0)?))).optional()?;
            let source_window = SnapshotWindow {
                row_limit: if anchor.is_some() {
                    window.row_limit
                } else {
                    0
                },
                required_run_id: Some(cutoff),
                anchor_item_id: anchor,
                anchor_thread_id: window.anchor_thread_id.clone().filter(|a| *a != *id),
                ..window.clone()
            };
            let source_projection =
                bounded_projection(db, &source, &source_window, raw["createdAt"].as_str(), seen)?;
            seen.remove(&id.0);
            let source_runs = values(&source_projection.runs)?;
            let source_attempts = values(&source_projection.attempts)?;
            let source_items = values(&source_projection.turn_items)?;
            visible_items.extend(
                source_projection
                    .visible_turn_items
                    .into_iter()
                    .filter(|r| {
                        r.source_thread_id != IdOrIndex::Id(source.clone())
                            || matches!(r.item, OrchestrationV2TurnItem::Fork(_))
                    })
                    .map(|mut r| {
                        r.visibility = ProjectedTurnItemVisibility::Inherited;
                        r
                    }),
            );
            for (item, value) in source_projection.turn_items.into_iter().zip(&source_items) {
                if visible(value, &source_runs, &source_attempts, &source_items, true) {
                    visible_items.push(item_row(item, ProjectedTurnItemVisibility::Inherited)?);
                }
            }
            let marker = serde_json::json!({"id":format!("turn-item:fork:{}",id.0),"threadId":id,"runId":null,"nodeId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":"Forked from conversation","startedAt":null,"completedAt":raw["createdAt"],"updatedAt":raw["createdAt"],"type":"fork","source":{"type":"run","threadId":source,"runId":raw["forkedFrom"]["runId"]},"targetThreadId":id});
            let mut row = item_row(
                serde_json::from_value(marker)?,
                ProjectedTurnItemVisibility::Synthetic,
            )?;
            row.source_thread_id = IdOrIndex::Id(source);
            visible_items.push(row);
        }
    }
    for (item, value) in projection.turn_items.iter().zip(&items) {
        if visible(
            value,
            &runs,
            &attempts,
            &items,
            window.required_run_id.is_some(),
        ) {
            visible_items.push(item_row(item.clone(), ProjectedTurnItemVisibility::Local)?);
        }
    }
    for (position, row) in visible_items.iter_mut().enumerate() {
        row.position = position as u64;
    }
    projection.visible_turn_items = visible_items;
    Ok(projection)
}

fn materialize(db: &Connection, row: &TimelineRow) -> Result<OrchestrationV2ProjectedTurnItem> {
    let item = if row.synthetic {
        serde_json::from_value(row.item.clone())?
    } else {
        one(db,"SELECT payload_json FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 AND turn_item_id=?2",params![row.source.0,row.id.0])?.ok_or_else(||Error::Decode { entity:"timeline item",detail:"item disappeared during snapshot".into() })?
    };
    Ok(OrchestrationV2ProjectedTurnItem {
        position: row.position,
        visibility: row.visibility.clone(),
        source_thread_id: IdOrIndex::Id(row.source.clone()),
        source_item_id: IdOrIndex::Id(row.id.clone()),
        item,
    })
}

fn match_local_timeline(
    db: &Connection,
    query: &TimelineQuery,
    local: &LocalVisibility,
    total: u64,
) -> Result<(Vec<TimelineRow>, bool)> {
    let id = &query.thread_id;
    if let Some(item_id) = &query.item_id {
        let mut found = local_timeline_rows(db, id, "turn_item_id=?2", params![id.0, item_id.0])?;
        found.retain(|r| local.contains(&r.item));
        for row in &mut found {
            row.position =
                local.before(db, id, row.item["ordinal"].as_i64().unwrap_or(0), &row.id.0)?;
        }
        return Ok((found, false));
    }
    let first = query.after_position.map_or(0, |p| p.saturating_add(1));
    let start:Option<i64>=db.query_row("SELECT ordinal FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 ORDER BY ordinal,turn_item_id LIMIT 1 OFFSET ?2",params![id.0,integer(first)?],|r|r.get(0)).optional()?;
    let Some(start) = start else {
        return Ok((vec![], false));
    };
    let base = local.before(db, id, start, "")?;
    let hidden_after = local
        .hidden
        .iter()
        .filter(|i| i["ordinal"].as_i64().is_some_and(|o| o >= start))
        .count() as u64;
    let limit = first
        .saturating_sub(base)
        .saturating_add(query.limit.min(total))
        .saturating_add(1)
        .saturating_add(hidden_after);
    let messages_only = !matches!(query.view, Some(TimelineView::Activity));
    let selected: Vec<Value> = rows(
        db,
        &format!(
            "SELECT {LOCAL_TIMELINE_COLUMNS} FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 AND ordinal>=?2 {} ORDER BY ordinal,turn_item_id LIMIT ?3",
            if messages_only {
                "AND type IN('user_message','assistant_message','proposed_plan')"
            } else {
                ""
            }
        ),
        params![id.0, start, integer(limit)?],
    )?;
    let selected = selected
        .into_iter()
        .filter(|i| local.contains(i))
        .collect::<Vec<_>>();
    let mut positions = BTreeMap::new();
    if messages_only && let Some(last) = selected.last() {
        let mut stmt=db.prepare("SELECT turn_item_id FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 AND ordinal>=?2 AND ordinal<=?3 ORDER BY ordinal,turn_item_id")?;
        let span = stmt
            .query_map(params![id.0, start, last["ordinal"].as_i64()], |r| {
                r.get::<_, String>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let hidden = local
            .hidden
            .iter()
            .filter_map(|i| i["id"].as_str())
            .collect::<BTreeSet<_>>();
        for item_id in span {
            if !hidden.contains(item_id.as_str()) {
                let position = base + positions.len() as u64;
                positions.insert(item_id, position);
            }
        }
    }
    let mut candidates = Vec::new();
    for (offset, item) in selected.into_iter().enumerate() {
        let item_id: TurnItemId = serde_json::from_value(item["id"].clone())?;
        let position = if messages_only {
            *positions.get(&item_id.0).ok_or_else(|| Error::Decode {
                entity: "timeline position",
                detail: "selected row disappeared".into(),
            })?
        } else {
            base + offset as u64
        };
        candidates.push(TimelineRow {
            source: id.clone(),
            id: item_id,
            visibility: ProjectedTurnItemVisibility::Local,
            item,
            synthetic: false,
            position,
        });
    }
    let before = candidates.len();
    candidates.retain(|r| query.after_position.is_none_or(|p| r.position > p));
    let has_earlier = candidates.len() < before
        || (base > 0
            && (!messages_only
                || {
                    let earlier: Vec<Value> = rows(
                        db,
                        &format!(
                            "SELECT {LOCAL_TIMELINE_COLUMNS} FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 AND ordinal<?2 AND type IN('user_message','assistant_message','proposed_plan') ORDER BY ordinal,turn_item_id LIMIT ?3"
                        ),
                        params![id.0, start, local.hidden.len() as i64 + 1],
                    )?;
                    earlier.iter().any(|i| local.contains(i))
                }));
    Ok((candidates, has_earlier))
}
fn local_timeline_page(db: &Connection, query: &TimelineQuery) -> Result<Option<TimelinePage>> {
    let thread = require_thread(db, &query.thread_id)?;
    if thread.deleted_at.is_some() {
        return Err(Error::ThreadNotFound(query.thread_id.clone()));
    }
    let raw = t3_contracts::stored::to_value(&thread)?;
    if raw["forkedFrom"]["type"] == "run" && raw["lineage"]["sideChat"] != true {
        return Ok(None);
    }
    let local = local_visibility(db, &query.thread_id)?;
    let raw_total: i64 = db.query_row(
        "SELECT COUNT(*) FROM orchestration_v2_projection_turn_items WHERE thread_id=?1",
        [&query.thread_id.0],
        |r| r.get(0),
    )?;
    let total = (raw_total as u64).saturating_sub(local.hidden.len() as u64);
    let limit = usize::try_from(query.limit).unwrap_or(usize::MAX);
    let (page, has_more, has_earlier) =
        if query.before_position.is_none() || query.item_id.is_some() {
            let (mut matching, earlier) = match_local_timeline(db, query, &local, total)?;
            let more = matching.len() > limit;
            matching.truncate(limit);
            (matching, more, earlier)
        } else {
            let after = query.after_position.map_or(-1, i128::from);
            let end = query.before_position.unwrap_or(total).min(total);
            let mut span = query.limit.max(1);
            loop {
                let from = after.max(i128::from(end) - i128::from(span) - 1);
                let (matching, earlier) = match_local_timeline(
                    db,
                    &TimelineQuery {
                        after_position: (from >= 0).then_some(from as u64),
                        limit: span,
                        ..query.clone()
                    },
                    &local,
                    total,
                )?;
                let more = matching.iter().any(|r| r.position >= end);
                let mut before = matching
                    .into_iter()
                    .filter(|r| r.position < end)
                    .collect::<Vec<_>>();
                if before.len() >= limit || from == after {
                    let skip = before.len().saturating_sub(limit);
                    let page = before.drain(skip..).collect();
                    break (page, more, earlier || skip > 0);
                }
                span = span.saturating_mul(2);
            }
        };
    Ok(Some(TimelinePage {
        items: page
            .iter()
            .map(|r| materialize(db, r))
            .collect::<Result<_>>()?,
        total_items: total,
        has_more,
        has_earlier,
    }))
}
pub fn timeline(db: &Connection, query: &TimelineQuery) -> Result<TimelinePage> {
    if query.attachment_id.is_none()
        && query.limit <= 9_007_199_254_740_991
        && query
            .after_position
            .is_none_or(|p| p <= 9_007_199_254_740_991)
        && query
            .before_position
            .is_none_or(|p| p <= 9_007_199_254_740_991 || p == u64::MAX)
        && let Some(page) = local_timeline_page(db, query)?
    {
        return Ok(page);
    }
    timeline_from_full_index(db, query)
}
pub fn timeline_from_full_index(db: &Connection, query: &TimelineQuery) -> Result<TimelinePage> {
    let indexed = timeline_rows(db, &query.thread_id, &mut BTreeSet::new())?;
    let mut in_view = Vec::new();
    for row in &indexed {
        if query.item_id.as_ref().map_or_else(
            || {
                matches!(query.view, Some(TimelineView::Activity))
                    || matches!(
                        row.item["type"].as_str(),
                        Some("user_message" | "assistant_message" | "proposed_plan")
                    )
            },
            |id| *id == row.id,
        ) {
            if let Some(attachment) = &query.attachment_id {
                let owns:bool = db.query_row("SELECT EXISTS(SELECT 1 FROM orchestration_v2_projection_turn_items AS item,json_each(item.payload_json,'$.attachments') AS attachment WHERE item.thread_id=?1 AND item.turn_item_id=?2 AND item.type='user_message' AND json_extract(attachment.value,'$.id')=?3)",params![row.source.0,row.id.0,attachment],|r|r.get(0))?;
                if !owns {
                    continue;
                }
            }
            in_view.push(row);
        }
    }
    let matching: Vec<_> = in_view
        .iter()
        .copied()
        .filter(|r| {
            query.item_id.is_some()
                || (query.after_position.is_none_or(|p| r.position > p)
                    && query.before_position.is_none_or(|p| r.position < p))
        })
        .collect();
    let limit = usize::try_from(query.limit).unwrap_or(usize::MAX);
    let page = if query.before_position.is_some() {
        &matching[matching.len().saturating_sub(limit)..]
    } else {
        &matching[..matching.len().min(limit)]
    };
    let start = page
        .first()
        .map(|r| r.position)
        .unwrap_or_else(|| query.after_position.map_or(0, |p| p.saturating_add(1)));
    Ok(TimelinePage {
        items: page
            .iter()
            .map(|r| materialize(db, r))
            .collect::<Result<_>>()?,
        total_items: indexed.len() as u64,
        has_more: query
            .before_position
            .map_or(matching.len() > page.len(), |p| {
                in_view.last().is_some_and(|r| r.position >= p)
            }),
        has_earlier: in_view.first().is_some_and(|r| r.position < start),
    })
}

fn idle_compaction_sql(alias: &str) -> String {
    format!(
        "({alias}.role='user' AND lower(trim(json_extract({alias}.payload_json,'$.text'),char(9,10,11,12,13,32,160,5760,8192,8193,8194,8195,8196,8197,8198,8199,8200,8201,8202,8232,8233,8239,8287,12288,65279)))='/compact' AND coalesce(json_array_length({alias}.payload_json,'$.attachments'),0)=0 AND (json_extract({alias}.payload_json,'$.idleCompaction') IS NOT NULL OR (json_extract({alias}.payload_json,'$.createdBy')='agent' AND json_extract({alias}.payload_json,'$.creationSource')='server')))"
    )
}
fn silent_run_sql(alias: &str) -> String {
    format!(
        "({alias}.status<>'failed' AND EXISTS(SELECT 1 FROM orchestration_v2_projection_messages prompt WHERE prompt.message_id=json_extract({alias}.payload_json,'$.userMessageId') AND {}) AND NOT EXISTS(SELECT 1 FROM orchestration_v2_projection_turn_items compaction WHERE compaction.thread_id={alias}.thread_id AND compaction.run_id={alias}.run_id AND compaction.type='compaction' AND compaction.status='failed'))",
        idle_compaction_sql("prompt")
    )
}
fn presented_run_sql() -> String {
    presented_run_select("candidate.payload_json", "?1")
}
fn presented_run_select(column: &str, thread: &str) -> String {
    format!(
        "SELECT {column} FROM orchestration_v2_projection_runs candidate WHERE candidate.thread_id={thread} AND NOT(candidate.status='queued' AND json_extract(candidate.payload_json,'$.queueHeld') IS 1) AND NOT(candidate.status='cancelled' AND json_extract(candidate.payload_json,'$.startedAt') IS NULL AND candidate.completed_at IS NOT NULL AND EXISTS(SELECT 1 FROM orchestration_v2_projection_runs earlier WHERE earlier.thread_id=candidate.thread_id AND earlier.ordinal<candidate.ordinal AND earlier.status<>'queued' AND NOT {} AND (earlier.completed_at IS NULL OR julianday(earlier.completed_at)>=julianday(candidate.completed_at)))) AND NOT {} ORDER BY candidate.ordinal DESC,candidate.run_id DESC LIMIT 1",
        silent_run_sql("earlier"),
        silent_run_sql("candidate")
    )
}
fn tool_id(name: &str) -> Option<&str> {
    if let Some(name) = name.strip_prefix("mcp__") {
        let (server, tool) = name.split_once("__")?;
        return matches!(
            server,
            "t3-code" | "t3_code" | "t3code" | "t3-code-direct" | "t3_code_direct"
        )
        .then_some(tool);
    }
    if let Some((server, tool)) = name.rsplit_once('.') {
        return matches!(
            server,
            "t3-code" | "t3_code" | "t3code" | "t3-code-direct" | "t3_code_direct"
        )
        .then_some(tool);
    }
    Some(name)
}
fn blocked_on_wait(active: &Value, items: &[Value]) -> bool {
    if active["status"] != "running" {
        return false;
    }
    let mut waiting = false;
    for item in items {
        if item["runId"] != active["id"]
            || !matches!(
                item["type"].as_str(),
                Some(
                    "command_execution"
                        | "dynamic_tool"
                        | "subagent"
                        | "file_change"
                        | "file_search"
                        | "web_search"
                        | "compaction"
                )
            )
            || !matches!(item["status"].as_str(), Some("running" | "waiting"))
            || (item["type"] == "subagent" && item["origin"] == "app_owned")
        {
            continue;
        }
        if item["type"] != "dynamic_tool" {
            return false;
        }
        let tool = item["toolName"].as_str().and_then(tool_id);
        if tool != Some("t3_thread_wait")
            && !(tool == Some("delegate_task") && item["input"]["mode"] == "wait")
        {
            return false;
        }
        waiting = true;
    }
    waiting
}
fn pending_background(
    latest: &Value,
    active: bool,
    thread: &Value,
    providers: &[Value],
    items: &[Value],
) -> Vec<Value> {
    if active
        || (!latest.is_null()
            && !matches!(
                latest["status"].as_str(),
                Some("cancelled" | "completed" | "failed" | "interrupted" | "waiting")
            ))
    {
        return vec![];
    }
    let mut tasks: Vec<Value> = Vec::new();
    if !latest.is_null() {
        for provider in providers.iter().filter(|p| {
            thread["activeProviderThreadId"].is_null()
                || p["id"] == thread["activeProviderThreadId"]
        }) {
            for task in provider["pendingBackgroundTasks"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if task["taskId"].as_str().is_none_or(str::is_empty)
                    || tasks.iter().any(|t| t["taskId"] == task["taskId"])
                {
                    continue;
                }
                let mut task = task.clone();
                if task["kind"] == "subagent" {
                    task["providerInstanceId"] = provider["providerInstanceId"].clone();
                }
                if let Some(description) = task["description"]
                    .as_str()
                    .map(str::trim)
                    .map(str::to_owned)
                {
                    if description.is_empty() {
                        task.as_object_mut().map(|t| t.remove("description"));
                    } else {
                        task["description"] = description.into();
                    }
                }
                tasks.push(task);
            }
        }
        for item in items {
            if !matches!(
                item["type"].as_str(),
                Some("command_execution" | "dynamic_tool" | "subagent")
            ) || !matches!(
                item["status"].as_str(),
                Some("pending" | "running" | "waiting")
            ) || (item["type"] == "dynamic_tool" && item["input"]["persistent"] == true)
            {
                continue;
            }
            let task_id = item["nativeItemRef"]["nativeId"]
                .as_str()
                .filter(|v| !v.is_empty())
                .or_else(|| item["id"].as_str())
                .unwrap_or("");
            if tasks.iter().any(|t| t["taskId"] == task_id) {
                continue;
            }
            let kind = if item["type"] == "subagent" {
                "subagent"
            } else if item["type"] == "command_execution" {
                if item["monitor"] == true {
                    "monitor"
                } else {
                    "command"
                }
            } else {
                "background_task"
            };
            let mut task = serde_json::json!({"taskId":task_id,"kind":kind});
            let description = item["title"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| match item["type"].as_str() {
                    Some("command_execution") => item["input"].as_str(),
                    Some("dynamic_tool") => item["toolName"].as_str(),
                    Some("subagent") => item["prompt"].as_str(),
                    _ => None,
                })
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if let Some(description) = description {
                task["description"] = description.into();
            }
            if kind == "subagent" {
                if !item["owningProviderInstanceId"].is_null() {
                    task["providerInstanceId"] = item["owningProviderInstanceId"].clone();
                }
                if !item["childThreadId"].is_null() {
                    task["childThreadId"] = item["childThreadId"].clone();
                }
            }
            tasks.push(task);
        }
    }
    for link in thread["pullRequests"].as_array().into_iter().flatten() {
        if link.get("watch").is_none()
            || link["source"] == "stack-dismissed"
            || link["snapshot"]["state"] == "merged"
        {
            continue;
        }
        let mut task = serde_json::json!({"taskId":format!("pull-request-watch:{}/{}#{}",link["host"].as_str().unwrap_or("").to_lowercase(),link["repository"].as_str().unwrap_or("").to_lowercase(),link["number"]),"description":format!("Watching pull request #{}",link["number"]),"kind":"monitor"});
        if link["watch"]["checksPending"] != false {
            task["awaited"] = true.into();
        }
        if let Some(old) = tasks.iter_mut().find(|t| t["taskId"] == task["taskId"]) {
            *old = task;
        } else {
            tasks.push(task);
        }
    }
    tasks
}

fn shell_item_counts(db: &Connection, id: &ThreadId) -> Result<u64> {
    let count:i64=db.query_row("SELECT COALESCE(SUM(CASE WHEN counts.run_key='' OR r.status<>'rolled_back' THEN counts.item_count-CASE WHEN r.status IN ('queued','cancelled') THEN(SELECT COUNT(*) FROM orchestration_v2_projection_turn_items held INDEXED BY orchestration_v2_projection_turn_items_queued_input_idx WHERE held.run_id=counts.run_key AND held.type='user_message' AND json_extract(held.payload_json,'$.inputIntent')='queued_turn') ELSE 0 END ELSE 0 END),0) FROM orchestration_v2_projection_turn_item_counts counts LEFT JOIN orchestration_v2_projection_runs r INDEXED BY orchestration_v2_projection_runs_run_status_idx ON r.run_id=counts.run_key WHERE counts.thread_id=?1",[&id.0],|r|r.get(0))?;
    Ok(count as u64)
}
fn shell_visible_count(db: &Connection, id: &ThreadId, seen: &mut BTreeSet<String>) -> Result<u64> {
    let count = shell_item_counts(db, id)?;
    let t: Value = one(db,"SELECT json_object('forkedFrom',json_extract(payload_json,'$.forkedFrom'),'lineage',json_extract(payload_json,'$.lineage'),'createdAt',created_at) FROM orchestration_v2_projection_threads WHERE thread_id=?1 AND deleted_at IS NULL",[&id.0])?.ok_or_else(||Error::ThreadNotFound(id.clone()))?;
    if t["forkedFrom"]["type"] != "run"
        || t["lineage"]["sideChat"] == true
        || !seen.insert(id.0.clone())
    {
        return Ok(count);
    }
    let source: ThreadId = serde_json::from_value(t["forkedFrom"]["threadId"].clone())?;
    let parent: Option<Value> = one(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_threads WHERE thread_id=?1 AND deleted_at IS NULL",
        [&source.0],
    )?;
    let Some(parent) = parent else {
        seen.remove(&id.0);
        return Ok(count);
    };
    let cutoff: Option<i64> = db
        .query_row(
            "SELECT ordinal FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND run_id=?2",
            params![source.0, t["forkedFrom"]["runId"].as_str()],
            |r| r.get(0),
        )
        .optional()?;
    let inherited: i64 = if let Some(cutoff) = cutoff {
        db.query_row("SELECT COALESCE(SUM(counts.item_count-CASE WHEN r.status IN ('queued','cancelled') OR (r.status='rolled_back' AND EXISTS(SELECT 1 FROM orchestration_v2_projection_run_attempts a WHERE a.run_id=r.run_id AND a.status='cancelled')) THEN(SELECT COUNT(*) FROM orchestration_v2_projection_turn_items held INDEXED BY orchestration_v2_projection_turn_items_queued_input_idx WHERE held.run_id=counts.run_key AND held.type='user_message' AND json_extract(held.payload_json,'$.inputIntent')='queued_turn') ELSE 0 END),0) FROM orchestration_v2_projection_turn_item_counts counts LEFT JOIN orchestration_v2_projection_runs r ON r.run_id=counts.run_key WHERE counts.thread_id=?1 AND ((counts.run_key='' AND ?4) OR (r.ordinal<=?2 AND NOT COALESCE(r.status='rolled_back' AND julianday(r.completed_at)<julianday(?3),0)))",params![source.0,cutoff,t["createdAt"].as_str(),parent["historyOrigin"]=="v1_import"],|r|r.get(0))?
    } else {
        0
    };
    let prefix =
        shell_visible_count(db, &source, seen)?.saturating_sub(shell_item_counts(db, &source)?);
    seen.remove(&id.0);
    Ok(count + prefix + inherited as u64 + 1)
}

fn idle_compaction(message: &Value) -> bool {
    message["role"] == "user"
        && message["text"]
            .as_str()
            .is_some_and(|text| text.trim_matches(|c|matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}')).eq_ignore_ascii_case("/compact"))
        && message["attachments"].as_array().is_none_or(Vec::is_empty)
        && (!message["idleCompaction"].is_null()
            || (message["createdBy"] == "agent" && message["creationSource"] == "server"))
}

pub fn shell_from_projection(
    projection: &OrchestrationV2ThreadProjection,
) -> Result<OrchestrationV2ThreadShell> {
    let mut output = t3_contracts::stored::to_value(&projection.thread)?;
    output["pullRequests"] = super::write::pull_request_links(&output, None).into();
    let messages = values(&projection.messages)?;
    let items = values(&projection.turn_items)?;
    let attempts = values(&projection.attempts)?;
    let all_runs = values(&projection.runs)?;
    let runs = all_runs
        .iter()
        .filter(|run| {
            run["status"] == "failed"
                || !messages
                    .iter()
                    .any(|m| m["id"] == run["userMessageId"] && idle_compaction(m))
                || items.iter().any(|i| {
                    i["runId"] == run["id"] && i["type"] == "compaction" && i["status"] == "failed"
                })
        })
        .cloned()
        .collect::<Vec<_>>();
    let sessions = values(&projection.provider_sessions)?;
    let session = sessions
        .iter()
        .filter(|s| s["providerInstanceId"] == output["providerInstanceId"])
        .max_by_key(|s| date_sort_key(s["updatedAt"].as_str().unwrap_or_default()));
    let session_error = session.map(|s| &s["lastError"]).unwrap_or(&Value::Null);
    let failure_for = |run: &Value| -> Value {
        if run["status"] != "failed" {
            return Value::Null;
        }
        items
            .iter()
            .filter(|i| {
                i["type"] == "error"
                    && i["status"] == "failed"
                    && i["runId"] == run["id"]
                    && i["nodeId"] == run["rootNodeId"]
            })
            .max_by(|a, b| {
                date_sort_key(a["updatedAt"].as_str().unwrap_or_default())
                    .cmp(&date_sort_key(b["updatedAt"].as_str().unwrap_or_default()))
                    .then_with(|| a["ordinal"].as_u64().cmp(&b["ordinal"].as_u64()))
                    .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
            })
            .map(|i| i["failure"].clone())
            .unwrap_or(Value::Null)
    };
    let executed = runs
        .iter()
        .filter(|r| {
            r["status"] != "queued" && !(r["status"] == "cancelled" && r["startedAt"].is_null())
        })
        .max_by(|a, b| {
            a["completedAt"]
                .is_null()
                .cmp(&b["completedAt"].is_null())
                .then_with(|| {
                    date_sort_key(a["completedAt"].as_str().unwrap_or_default()).cmp(
                        &date_sort_key(b["completedAt"].as_str().unwrap_or_default()),
                    )
                })
                .then_with(|| a["ordinal"].as_u64().cmp(&b["ordinal"].as_u64()))
        });
    let limited = executed.filter(|r| {
        let failure = failure_for(r);
        failure["class"] == "usage_limit"
            && (session_error.is_null() || *session_error == failure["message"])
            && runs
                .iter()
                .any(|other| other["ordinal"].as_u64() > r["ordinal"].as_u64())
    });
    let latest = limited
        .or_else(|| {
            runs.iter()
                .filter(|r| {
                    !(r["status"] == "queued" && r["queueHeld"] == true)
                        && !(r["status"] == "cancelled"
                            && r["startedAt"].is_null()
                            && !r["completedAt"].is_null()
                            && runs.iter().any(|earlier| {
                                earlier["ordinal"].as_u64() < r["ordinal"].as_u64()
                                    && earlier["status"] != "queued"
                                    && (earlier["completedAt"].is_null()
                                        || date_sort_key(
                                            earlier["completedAt"].as_str().unwrap_or_default(),
                                        ) >= date_sort_key(
                                            r["completedAt"].as_str().unwrap_or_default(),
                                        ))
                            }))
                })
                .max_by_key(|r| r["ordinal"].as_u64())
        })
        .unwrap_or(&Value::Null);
    let active = runs
        .iter()
        .filter(|r| {
            matches!(
                r["status"].as_str(),
                Some("preparing" | "starting" | "running")
            )
        })
        .max_by_key(|r| r["ordinal"].as_u64())
        .unwrap_or(&Value::Null);
    let activity = runs
        .iter()
        .filter(|r| {
            matches!(
                r["status"].as_str(),
                Some("preparing" | "starting" | "running" | "waiting")
            )
        })
        .max_by_key(|r| r["ordinal"].as_u64())
        .unwrap_or(&Value::Null);
    let failure = failure_for(latest);
    let current = if !session_error.is_null() && *session_error != failure["message"] {
        &Value::Null
    } else {
        &failure
    };
    for (key, field) in [
        ("latestRunId", "id"),
        ("latestRunRequestedAt", "requestedAt"),
        ("latestRunStartedAt", "startedAt"),
        ("latestRunCompletedAt", "completedAt"),
    ] {
        output[key] = latest[field].clone();
    }
    output["status"] = latest
        .get("status")
        .cloned()
        .unwrap_or_else(|| "idle".into());
    output["activeRunId"] = active["id"].clone();
    output["activityRunStatus"] = activity["status"].clone();
    output["activityRunStartedAt"] = activity
        .get("workStartedAt")
        .filter(|v| !v.is_null())
        .or_else(|| activity.get("startedAt").filter(|v| !v.is_null()))
        .or_else(|| activity.get("requestedAt"))
        .cloned()
        .unwrap_or(Value::Null);
    output["lastError"] = if session_error.is_null() {
        failure["message"].clone()
    } else {
        session_error.clone()
    };
    output["lastErrorClass"] = current["class"].clone();
    output["lastErrorCode"] = current["code"].clone();
    output["usageLimitResetAt"] = if current["class"] == "usage_limit" {
        current["resetAt"].clone()
    } else {
        Value::Null
    };
    output["latestRunStopRequested"] = (!latest.is_null()
        && items
            .iter()
            .any(|i| i["runId"] == latest["id"] && i["type"] == "run_interrupt_request"))
    .into();
    output["latestRunBackgroundWake"] = (!latest.is_null()
        && messages.iter().any(|m| {
            m["id"] == latest["userMessageId"]
                && m.get("delegatedCompletion").is_none()
                && m["notification"]["outcome"] == "updated"
                && matches!(
                    m["notification"]["source"]["kind"].as_str(),
                    Some("background_task" | "background_command" | "command" | "monitor")
                )
                && m["notification"]["source"]["work"].is_null()
        })
        && !messages
            .iter()
            .any(|m| m["runId"] == latest["id"] && m["createdBy"] == "user"))
    .into();
    let mut user_messages = messages
        .iter()
        .filter(|m| m["role"] == "user" && !idle_compaction(m))
        .collect::<Vec<_>>();
    user_messages.sort_by_key(|m| {
        std::cmp::Reverse(date_sort_key(m["updatedAt"].as_str().unwrap_or_default()))
    });
    output["latestUserMessageAt"] = user_messages
        .first()
        .map(|m| m["updatedAt"].clone())
        .unwrap_or(Value::Null);
    output["latestUserAuthoredMessageAt"] = user_messages
        .iter()
        .find(|m| m["createdBy"] == "user")
        .map(|m| m["updatedAt"].clone())
        .unwrap_or(Value::Null);
    let requests = values(&projection.runtime_requests)?;
    let request = requests
        .iter()
        .filter(|r| r["status"] == "pending")
        .min_by(|a, b| {
            (a["kind"] == "user_input")
                .cmp(&(b["kind"] == "user_input"))
                .then_with(|| {
                    date_sort_key(b["createdAt"].as_str().unwrap_or_default())
                        .cmp(&date_sort_key(a["createdAt"].as_str().unwrap_or_default()))
                })
        });
    let mut pending = request.map(|r|serde_json::json!({"id":r["id"],"kind":r["kind"],"createdAt":r["createdAt"],"blocking":r["responseCapability"]["type"]!="message"})).unwrap_or(Value::Null);
    if pending.is_null() || pending["kind"] == "user_input" {
        let card = items
            .iter()
            .filter(|i| {
                i["status"] == "waiting"
                    && ((i["type"] == "link_request"
                        && (output["latestUserAuthoredMessageAt"].is_null()
                            || date_sort_key(
                                output["latestUserAuthoredMessageAt"]
                                    .as_str()
                                    .unwrap_or_default(),
                            ) <= date_sort_key(
                                i.get("startedAt")
                                    .filter(|v| !v.is_null())
                                    .unwrap_or(&i["updatedAt"])
                                    .as_str()
                                    .unwrap_or_default(),
                            )))
                        || (i["type"] == "secret_request"
                            && !i["runId"].is_null()
                            && runs.iter().any(|r| {
                                r["id"] == i["runId"]
                                    && matches!(
                                        r["status"].as_str(),
                                        Some("preparing" | "starting" | "running" | "waiting")
                                    )
                            })))
            })
            .max_by_key(|i| date_sort_key(i["updatedAt"].as_str().unwrap_or_default()));
        if let Some(card) = card.filter(|c| !c["nodeId"].is_null()) {
            let at = card
                .get("startedAt")
                .filter(|v| !v.is_null())
                .unwrap_or(&card["updatedAt"]);
            if pending.is_null()
                || date_sort_key(at.as_str().unwrap_or_default())
                    >= date_sort_key(pending["createdAt"].as_str().unwrap_or_default())
            {
                pending = serde_json::json!({"id":card["id"],"kind":"user_input","createdAt":at,"blocking":true,"userOnly":true});
            }
        }
    }
    output["pendingRuntimeRequest"] = pending;
    output["latestVisibleMessage"] = Value::Null;
    output["pendingGuestMessageCount"] = output["guestShare"]["messages"]
        .as_array()
        .map_or(0, |ms| {
            ms.iter().filter(|m| m["status"] == "pending").count()
        })
        .into();
    output["hasActionableProposedPlan"] = values(&projection.plans)?
        .iter()
        .any(|p| p["kind"] == "proposed_plan" && p["status"] == "active")
        .into();
    let providers = values(&projection.provider_threads)?;
    let mut history = Vec::new();
    for p in provider_history_order(&providers)? {
        if p["appThreadId"] == output["id"]
            && p["ownerNodeId"].is_null()
            && !history.contains(&p["providerInstanceId"])
        {
            history.push(p["providerInstanceId"].clone());
        }
    }
    output["providerInstanceHistory"] = history.into();
    output["goal"] = providers
        .iter()
        .find(|p| p["id"] == output["activeProviderThreadId"])
        .map(|p| p["goal"].clone())
        .unwrap_or(Value::Null);
    let live_items = items
        .iter()
        .filter(|i| {
            i["runId"].is_null()
                || !all_runs
                    .iter()
                    .any(|r| r["id"] == i["runId"] && r["status"] == "rolled_back")
        })
        .map(|i| {
            let mut i = i.clone();
            i["owningProviderInstanceId"] = providers
                .iter()
                .find(|p| p["id"] == i["providerThreadId"])
                .map(|p| p["providerInstanceId"].clone())
                .unwrap_or(Value::Null);
            i
        })
        .collect::<Vec<_>>();
    output["blockedOnThreadWait"] = blocked_on_wait(active, &live_items).into();
    output["pendingBackgroundTasks"] =
        pending_background(latest, !active.is_null(), &output, &providers, &live_items).into();
    output["itemCount"] = items
        .iter()
        .filter(|i| visible(i, &all_runs, &attempts, &items, false))
        .count()
        .into();
    output["visibleItemCount"] = projection.visible_turn_items.len().into();
    for key in [
        "unsettledAt",
        "snoozedUntil",
        "snoozedAt",
        "pinnedAt",
        "autoSettleDisabledAt",
        "pinOrderKey",
        "opens",
        "titleRegeneration",
        "lastTitleRegeneration",
        "limitRecovery",
    ] {
        if output.get(key).is_none() {
            output[key] = Value::Null;
        }
    }
    Ok(serde_json::from_value(output)?)
}

#[derive(Debug)]
struct RawShellRow {
    id: String,
    thread_json: String,
    facts: Value,
}
#[derive(Debug)]
pub struct RawShellSnapshot {
    sequence: u64,
    rows: Vec<RawShellRow>,
    providers: Vec<(String, String)>,
    items: Vec<(String, String, Option<String>)>,
}
fn shell_run_metadata(alias: &str) -> String {
    format!(
        "CASE WHEN {alias}.run_id IS NULL THEN NULL ELSE json_object('id',{alias}.run_id,'status',{alias}.status,'requestedAt',{alias}.requested_at,'startedAt',json_extract({alias}.payload_json,'$.startedAt'),'completedAt',{alias}.completed_at,'workStartedAt',json_extract({alias}.payload_json,'$.workStartedAt'),'userMessageId',json_extract({alias}.payload_json,'$.userMessageId')) END"
    )
}
fn shell_failure_sql(alias: &str) -> String {
    format!(
        "CASE WHEN {alias}.status='failed' THEN (SELECT error.payload_json FROM orchestration_v2_projection_turn_items error WHERE error.thread_id=t.thread_id AND error.run_id={alias}.run_id AND error.type='error' AND error.status='failed' AND error.node_id IS json_extract({alias}.payload_json,'$.rootNodeId') ORDER BY error.updated_at DESC,error.ordinal DESC,error.turn_item_id DESC LIMIT 1) ELSE NULL END"
    )
}
fn shell_stop_sql(alias: &str) -> String {
    format!(
        "EXISTS(SELECT 1 FROM orchestration_v2_projection_turn_items item WHERE item.thread_id=t.thread_id AND item.run_id={alias}.run_id AND item.type='run_interrupt_request')"
    )
}
fn shell_wake_sql(alias: &str) -> String {
    format!(
        "(EXISTS(SELECT 1 FROM orchestration_v2_projection_messages wake WHERE wake.message_id=json_extract({alias}.payload_json,'$.userMessageId') AND json_extract(wake.payload_json,'$.notification.source.kind') IN ('background_task','background_command','command','monitor') AND json_extract(wake.payload_json,'$.notification.source.work') IS NULL AND json_extract(wake.payload_json,'$.notification.outcome')='updated' AND json_extract(wake.payload_json,'$.delegatedCompletion') IS NULL) AND NOT EXISTS(SELECT 1 FROM orchestration_v2_projection_messages authored WHERE authored.run_id={alias}.run_id AND json_extract(authored.payload_json,'$.createdBy')='user'))"
    )
}
fn provider_history_order(providers: &[Value]) -> Result<Vec<&Value>> {
    let collator = id_collator()?;
    let mut ordered: Vec<_> = providers.iter().collect();
    ordered.sort_by(|a, b| {
        date_sort_key(a["createdAt"].as_str().unwrap_or_default())
            .cmp(&date_sort_key(b["createdAt"].as_str().unwrap_or_default()))
            .then_with(|| {
                collator.compare(
                    a["id"].as_str().unwrap_or_default(),
                    b["id"].as_str().unwrap_or_default(),
                )
            })
    });
    Ok(ordered)
}
pub fn read_shell_rows(
    db: &Connection,
    query: &ShellQuery,
    include_counts: bool,
) -> Result<RawShellSnapshot> {
    let mut statement=db.prepare("SELECT thread_id FROM orchestration_v2_projection_threads t WHERE deleted_at IS NULL AND (?1 OR archived_at IS NULL) AND(?2 IS NULL OR project_id=?2) AND(NOT ?3 OR(json_extract(payload_json,'$.settledAt') IS NULL AND json_extract(payload_json,'$.settledOverride') IS NOT 'settled')) AND(?4 IS NULL OR json_extract(payload_json,'$.lineage.rootThreadId')=(SELECT json_extract(payload_json,'$.lineage.rootThreadId') FROM orchestration_v2_projection_threads WHERE thread_id=?4)) AND(?5 IS NULL OR json_extract(payload_json,'$.worktreePath')=?5) AND(?6 IS NULL OR thread_id IN(SELECT value FROM json_each(?6))) ORDER BY updated_at,thread_id")?;
    let ids = statement
        .query_map(
            params![
                query.archived,
                query.project_id.as_ref().map(|id| &id.0),
                query.unsettled_only,
                query.family_of.as_ref().map(|id| &id.0),
                query.worktree_path,
                json_list(&query.thread_ids)?
            ],
            |r| r.get::<_, String>(0),
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let sequence=db.query_row::<i64,_,_>("SELECT COALESCE(MAX(sequence),0) FROM orchestration_events WHERE application_event_version=2 AND aggregate_kind='thread'",[],|r|r.get(0))? as u64;
    if ids.is_empty() {
        return Ok(RawShellSnapshot {
            sequence,
            rows: vec![],
            providers: vec![],
            items: vec![],
        });
    }
    let selection = serde_json::to_string(&ids)?;
    let presented = presented_run_select("candidate.run_id", "t.thread_id");
    let active = format!(
        "SELECT r.run_id FROM orchestration_v2_projection_runs r INDEXED BY orchestration_v2_projection_runs_thread_status_idx WHERE r.thread_id=t.thread_id AND r.status IN ('preparing','starting','running') AND NOT {} ORDER BY r.ordinal DESC,r.run_id DESC LIMIT 1",
        silent_run_sql("r")
    );
    let activity = format!(
        "SELECT r.run_id FROM orchestration_v2_projection_runs r INDEXED BY orchestration_v2_projection_runs_thread_status_idx WHERE r.thread_id=t.thread_id AND r.status IN ('preparing','starting','running','waiting') AND NOT {} ORDER BY r.ordinal DESC,r.run_id DESC LIMIT 1",
        silent_run_sql("r")
    );
    let executed = "SELECT r.run_id FROM orchestration_v2_projection_runs r WHERE r.thread_id=t.thread_id AND r.status<>'queued' AND NOT(r.status='cancelled' AND json_extract(r.payload_json,'$.startedAt') IS NULL) ORDER BY r.completed_at IS NULL DESC,r.completed_at DESC,r.ordinal DESC,r.run_id DESC LIMIT 1";
    let count = if include_counts {
        "(SELECT COALESCE(SUM(CASE WHEN counts.run_key='' OR run.status<>'rolled_back' THEN counts.item_count-CASE WHEN run.status IN ('queued','cancelled') THEN(SELECT COUNT(*) FROM orchestration_v2_projection_turn_items held INDEXED BY orchestration_v2_projection_turn_items_queued_input_idx WHERE held.run_id=counts.run_key AND held.type='user_message' AND json_extract(held.payload_json,'$.inputIntent')='queued_turn') ELSE 0 END ELSE 0 END),0) FROM orchestration_v2_projection_turn_item_counts counts LEFT JOIN orchestration_v2_projection_runs run INDEXED BY orchestration_v2_projection_runs_run_status_idx ON run.run_id=counts.run_key WHERE counts.thread_id=t.thread_id)"
    } else {
        "0"
    };
    let user_date = format!(
        "SELECT message.updated_at FROM orchestration_v2_projection_messages message WHERE message.thread_id=t.thread_id AND message.role='user' AND NOT {}",
        idle_compaction_sql("message")
    );
    let facts = format!(
        r#"json_object(
      'presented',{},'active',{},'activity',{},'executed',{},
      'presentedFailure',{},'executedFailure',{},
      'presentedStop',{},'executedStop',{},'presentedWake',{},'executedWake',{},
      'sessionError',(SELECT json_extract(s.payload_json,'$.lastError') FROM orchestration_v2_projection_provider_session_bindings b CROSS JOIN orchestration_v2_projection_provider_sessions s ON s.provider_session_id=b.provider_session_id WHERE b.thread_id=t.thread_id AND s.provider_instance_id=json_extract(t.payload_json,'$.providerInstanceId') ORDER BY s.updated_at DESC,s.provider_session_id DESC LIMIT 1),
      'userDate',({user_date} ORDER BY message.updated_at DESC,message.message_id DESC LIMIT 1),
      'authoredDate',({user_date} AND json_extract(message.payload_json,'$.createdBy')='user' ORDER BY message.updated_at DESC,message.message_id DESC LIMIT 1),
      'request',(SELECT payload_json FROM orchestration_v2_projection_runtime_requests WHERE thread_id=t.thread_id AND status='pending' ORDER BY(kind='user_input'),created_at DESC,runtime_request_id DESC LIMIT 1),
      'secretCard',(SELECT item.payload_json FROM orchestration_v2_projection_turn_items item WHERE item.thread_id=t.thread_id AND item.type='secret_request' AND item.status='waiting' AND item.run_id IN(SELECT run_id FROM orchestration_v2_projection_runs WHERE thread_id=t.thread_id AND status IN('preparing','starting','running','waiting')) ORDER BY item.updated_at DESC,item.turn_item_id DESC LIMIT 1),
      'linkCard',(SELECT item.payload_json FROM orchestration_v2_projection_turn_items item WHERE item.thread_id=t.thread_id AND item.type='link_request' AND item.status='waiting' ORDER BY item.updated_at DESC,item.turn_item_id DESC LIMIT 1),
      'actionablePlan',EXISTS(SELECT 1 FROM orchestration_v2_projection_plans WHERE thread_id=t.thread_id AND kind='proposed_plan' AND status='active'),
      'itemCount',{count},'forked',json_extract(t.payload_json,'$.forkedFrom.type')='run' AND json_extract(t.payload_json,'$.lineage.sideChat') IS NOT 1)"#,
        shell_run_metadata("presented"),
        shell_run_metadata("active"),
        shell_run_metadata("activity"),
        shell_run_metadata("executed"),
        shell_failure_sql("presented"),
        shell_failure_sql("executed"),
        shell_stop_sql("presented"),
        shell_stop_sql("executed"),
        shell_wake_sql("presented"),
        shell_wake_sql("executed")
    );
    let sql = format!(
        "SELECT t.thread_id,t.payload_json,{facts} FROM orchestration_v2_projection_threads t LEFT JOIN orchestration_v2_projection_runs presented ON presented.run_id=({presented}) LEFT JOIN orchestration_v2_projection_runs active ON active.run_id=({active}) LEFT JOIN orchestration_v2_projection_runs activity ON activity.run_id=({activity}) LEFT JOIN orchestration_v2_projection_runs executed ON executed.run_id=({executed}) WHERE t.thread_id IN(SELECT value FROM json_each(?1)) ORDER BY t.updated_at,t.thread_id"
    );
    let mut statement = db.prepare(&sql)?;
    let mut rows = Vec::new();
    for row in statement.query_map([&selection], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (id, thread_json, json) = row?;
        let mut facts: Value = serde_json::from_str(&json)?;
        facts["visibleItemCount"] = facts["itemCount"].clone();
        if include_counts && facts["forked"] == 1 {
            facts["visibleItemCount"] =
                shell_visible_count(db, &ThreadId(id.clone()), &mut BTreeSet::new())?.into();
        }
        rows.push(RawShellRow {
            id,
            thread_json,
            facts,
        });
    }
    let mut statement=db.prepare("SELECT thread_id,payload_json FROM orchestration_v2_projection_provider_threads WHERE thread_id IN(SELECT value FROM json_each(?1)) ORDER BY COALESCE(first_run_ordinal,0),provider_thread_id")?;
    let providers = statement
        .query_map([&selection], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut statement=db.prepare("SELECT i.thread_id,i.payload_json,(SELECT p.provider_instance_id FROM orchestration_v2_projection_provider_threads p WHERE p.provider_thread_id=i.provider_thread_id) FROM orchestration_v2_projection_turn_items i LEFT JOIN orchestration_v2_projection_runs r ON r.run_id=i.run_id WHERE i.thread_id IN(SELECT value FROM json_each(?1)) AND i.type IN('command_execution','dynamic_tool','subagent','file_change','file_search','web_search','compaction') AND i.status IN('pending','running','waiting') AND(i.run_id IS NULL OR r.status<>'rolled_back') ORDER BY i.ordinal,i.turn_item_id")?;
    let items = statement
        .query_map([&selection], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(RawShellSnapshot {
        sequence,
        rows,
        providers,
        items,
    })
}
fn decode_shell_row(
    row: RawShellRow,
    providers: Vec<Value>,
    items: Vec<Value>,
) -> Result<OrchestrationV2ThreadShell> {
    let t: OrchestrationV2AppThread = serde_json::from_str(&row.thread_json)?;
    let facts = row.facts;
    let mut output = t3_contracts::stored::to_value(&t)?;
    output["pullRequests"] = super::write::pull_request_links(&output, None).into();
    let mut latest = facts["presented"].clone();
    let active = &facts["active"];
    let activity = &facts["activity"];
    let session_error = facts["sessionError"].as_str().map(str::to_owned);
    let failure_from = |key: &str| -> Result<Value> {
        facts[key]
            .as_str()
            .map(|json| {
                Ok(t3_contracts::stored::to_value(
                    &serde_json::from_str::<OrchestrationV2TurnItem>(json)?,
                )?["failure"]
                    .clone())
            })
            .transpose()
            .map(|failure| failure.unwrap_or(Value::Null))
    };
    let mut failure = failure_from("presentedFailure")?;
    let executed = facts["executed"].clone();
    let blocking_failure = failure_from("executedFailure")?;
    let mut use_executed = false;
    if blocking_failure["class"] == "usage_limit"
        && session_error
            .as_deref()
            .is_none_or(|e| Some(e) == blocking_failure["message"].as_str())
    {
        use_executed = true;
        latest = executed;
        failure = blocking_failure;
    }
    let current_failure = if session_error
        .as_deref()
        .is_some_and(|e| Some(e) != failure["message"].as_str())
    {
        Value::Null
    } else {
        failure.clone()
    };
    for (key, field) in [
        ("latestRunId", "id"),
        ("latestRunRequestedAt", "requestedAt"),
        ("latestRunStartedAt", "startedAt"),
        ("latestRunCompletedAt", "completedAt"),
    ] {
        output[key] = latest[field].clone();
    }
    output["status"] = latest
        .get("status")
        .cloned()
        .unwrap_or_else(|| "idle".into());
    output["activeRunId"] = active["id"].clone();
    output["activityRunStatus"] = activity["status"].clone();
    output["activityRunStartedAt"] = activity
        .get("workStartedAt")
        .filter(|v| !v.is_null())
        .or_else(|| activity.get("startedAt").filter(|v| !v.is_null()))
        .or_else(|| activity.get("requestedAt"))
        .cloned()
        .unwrap_or(Value::Null);
    output["lastError"] = session_error
        .map(Value::String)
        .unwrap_or_else(|| failure["message"].clone());
    output["lastErrorClass"] = current_failure["class"].clone();
    output["lastErrorCode"] = current_failure["code"].clone();
    output["usageLimitResetAt"] = if current_failure["class"] == "usage_limit" {
        current_failure["resetAt"].clone()
    } else {
        Value::Null
    };
    output["latestRunStopRequested"] = facts[if use_executed {
        "executedStop"
    } else {
        "presentedStop"
    }]
    .as_i64()
    .is_some_and(|v| v != 0)
    .into();
    output["latestRunBackgroundWake"] = facts[if use_executed {
        "executedWake"
    } else {
        "presentedWake"
    }]
    .as_i64()
    .is_some_and(|v| v != 0)
    .into();
    output["latestUserMessageAt"] = facts["userDate"].clone();
    output["latestUserAuthoredMessageAt"] = facts["authoredDate"].clone();
    let request: Value = facts["request"]
        .as_str()
        .map(|json| -> Result<Value> {
            Ok(t3_contracts::stored::to_value(&serde_json::from_str::<
                OrchestrationV2RuntimeRequest,
            >(json)?)?)
        })
        .transpose()?
        .unwrap_or(Value::Null);
    let mut pending = if request.is_null() {
        Value::Null
    } else {
        serde_json::json!({"id":request["id"],"kind":request["kind"],"createdAt":request["createdAt"],"blocking":request["responseCapability"]["type"]!="message"})
    };
    if pending.is_null() || pending["kind"] == "user_input" {
        let mut cards = ["secretCard", "linkCard"]
            .into_iter()
            .filter_map(|key| facts[key].as_str())
            .map(|json| {
                Ok(t3_contracts::stored::to_value(&serde_json::from_str::<
                    OrchestrationV2TurnItem,
                >(json)?)?)
            })
            .collect::<Result<Vec<_>>>()?;
        // Secret requests win equal update times, matching the source's stable sort.
        cards.sort_by(|a, b| b["updatedAt"].as_str().cmp(&a["updatedAt"].as_str()));
        if let Some(card) = cards.iter().find(|c| {
            c["type"] != "link_request"
                || output["latestUserAuthoredMessageAt"].is_null()
                || output["latestUserAuthoredMessageAt"].as_str()
                    <= c.get("startedAt")
                        .filter(|v| !v.is_null())
                        .unwrap_or(&c["updatedAt"])
                        .as_str()
        }) {
            let at = card
                .get("startedAt")
                .filter(|v| !v.is_null())
                .unwrap_or(&card["updatedAt"]);
            if !card["nodeId"].is_null()
                && (pending.is_null() || at.as_str() >= pending["createdAt"].as_str())
            {
                pending = serde_json::json!({"id":card["id"],"kind":"user_input","createdAt":at,"blocking":true,"userOnly":true});
            }
        }
    }
    output["pendingRuntimeRequest"] = pending;
    output["latestVisibleMessage"] = Value::Null;
    output["pendingGuestMessageCount"] = output["guestShare"]["messages"]
        .as_array()
        .map_or(0, |m| m.iter().filter(|m| m["status"] == "pending").count())
        .into();
    output["hasActionableProposedPlan"] = facts["actionablePlan"]
        .as_i64()
        .is_some_and(|v| v != 0)
        .into();
    output["blockedOnThreadWait"] = blocked_on_wait(active, &items).into();
    output["pendingBackgroundTasks"] =
        pending_background(&latest, !active.is_null(), &output, &providers, &items).into();
    let mut history = Vec::new();
    for provider in provider_history_order(&providers)? {
        if provider["appThreadId"] == output["id"]
            && provider["ownerNodeId"].is_null()
            && !history.contains(&provider["providerInstanceId"])
        {
            history.push(provider["providerInstanceId"].clone());
        }
    }
    output["providerInstanceHistory"] = history.into();
    output["goal"] = providers
        .iter()
        .find(|p| p["id"] == output["activeProviderThreadId"])
        .map(|p| p["goal"].clone())
        .unwrap_or(Value::Null);
    output["itemCount"] = facts["itemCount"].clone();
    output["visibleItemCount"] = facts["visibleItemCount"].clone();
    for key in [
        "unsettledAt",
        "snoozedUntil",
        "snoozedAt",
        "pinnedAt",
        "autoSettleDisabledAt",
        "pinOrderKey",
        "opens",
        "titleRegeneration",
        "lastTitleRegeneration",
        "limitRecovery",
    ] {
        if output.get(key).is_none() {
            output[key] = Value::Null;
        }
    }
    Ok(serde_json::from_value(output)?)
}

pub fn decode_shell_rows(raw: RawShellSnapshot) -> Result<OrchestrationV2ThreadShellSnapshot> {
    let mut providers: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for (id, json) in raw.providers {
        providers
            .entry(id)
            .or_default()
            .push(t3_contracts::stored::to_value(&serde_json::from_str::<
                OrchestrationV2ProviderThread,
            >(&json)?)?);
    }
    let mut items: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for (id, json, owner) in raw.items {
        let mut item = t3_contracts::stored::to_value(&serde_json::from_str::<
            OrchestrationV2TurnItem,
        >(&json)?)?;
        item["owningProviderInstanceId"] = owner.map(Value::String).unwrap_or(Value::Null);
        items.entry(id).or_default().push(item);
    }
    let mut threads = Vec::new();
    let mut archived_threads = Vec::new();
    for row in raw.rows {
        let id = &row.id;
        let providers = providers.remove(id).unwrap_or_default();
        let items = items.remove(id).unwrap_or_default();
        let shell = decode_shell_row(row, providers, items)?;
        if shell.archived_at.is_none() {
            threads.push(shell);
        } else {
            archived_threads.push(shell);
        }
    }
    Ok(OrchestrationV2ThreadShellSnapshot {
        schema_version: 2,
        snapshot_sequence: raw.sequence,
        threads,
        archived_threads,
    })
}
pub fn shell(db: &Connection, id: &ThreadId) -> Result<Option<OrchestrationV2ThreadShell>> {
    shell_with_counts(db, id, true)
}
fn shell_with_counts(
    db: &Connection,
    id: &ThreadId,
    include_counts: bool,
) -> Result<Option<OrchestrationV2ThreadShell>> {
    let raw = read_shell_rows(
        db,
        &ShellQuery {
            archived: true,
            thread_ids: Some(vec![id.clone()]),
            ..Default::default()
        },
        include_counts,
    )?;
    let snapshot = decode_shell_rows(raw)?;
    Ok(snapshot
        .threads
        .into_iter()
        .chain(snapshot.archived_threads)
        .next())
}
pub fn thread_shells(
    db: &Connection,
    query: &ShellQuery,
) -> Result<OrchestrationV2ThreadShellSnapshot> {
    decode_shell_rows(read_shell_rows(db, query, true)?)
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestThread {
    pub id: ThreadId,
    pub project_id: ProjectId,
    pub lineage: OrchestrationV2AppThreadLineage,
    pub created_at: IsoDateTime,
    pub updated_at: IsoDateTime,
    #[serde(with = "t3_contracts::wire::nullable")]
    pub branch: Option<TrimmedNonEmptyString>,
    #[serde(with = "t3_contracts::wire::nullable")]
    pub worktree_path: Option<TrimmedNonEmptyString>,
    #[serde(with = "t3_contracts::wire::nullable")]
    pub archived_at: Option<IsoDateTime>,
    #[serde(default)]
    pub settled_override: Option<AppThreadSettledOverride>,
    #[serde(default)]
    pub settled_at: Option<IsoDateTime>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::nullable_optional"
    )]
    pub linked_pull_request:
        Option<Option<t3_contracts::thread_pull_request::ThreadLinkedPullRequest>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::nullable_optional"
    )]
    pub branch_pull_request:
        Option<Option<t3_contracts::thread_pull_request::ThreadLinkedPullRequest>>,
    pub pull_requests: Vec<t3_contracts::thread_pull_request::ThreadPullRequestLink>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct PullRequestRead {
    pub snapshot_sequence: u64,
    pub threads: Vec<PullRequestThread>,
}
pub fn pull_request_snapshot(
    db: &Connection,
    id: Option<&ThreadId>,
    unsettled_only: bool,
) -> Result<PullRequestRead> {
    let raw: Vec<Value> = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_threads WHERE deleted_at IS NULL AND ((?1 IS NULL AND archived_at IS NULL) OR thread_id=?1) AND (NOT ?2 OR (json_extract(payload_json,'$.settledAt') IS NULL AND json_extract(payload_json,'$.settledOverride') IS NOT 'settled')) ORDER BY updated_at,thread_id",
        params![id.map(|id| &id.0), unsettled_only],
    )?;
    let threads = raw
        .into_iter()
        .map(|mut t| {
            t["pullRequests"] = super::write::pull_request_links(&t, None).into();
            Ok(serde_json::from_value(t)?)
        })
        .collect::<Result<_>>()?;
    let snapshot_sequence=db.query_row::<i64,_,_>("SELECT COALESCE(MAX(sequence),0) FROM orchestration_events WHERE application_event_version=2 AND aggregate_kind='thread'",[],|r|r.get(0))? as u64;
    Ok(PullRequestRead {
        snapshot_sequence,
        threads,
    })
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAwarenessThread {
    pub id: ThreadId,
    pub project_id: ProjectId,
    pub creation_source: OrchestrationV2CreationSource,
    pub archived_at: Option<IsoDateTime>,
    pub active_run_id: Option<RunId>,
    pub latest_run_id: Option<RunId>,
    pub has_actionable_proposed_plan: bool,
    pub title: TrimmedNonEmptyString,
    pub lineage: OrchestrationV2AppThreadLineage,
    pub model_selection: t3_contracts::model_selection::ModelSelection,
    pub settled_override: Option<AppThreadSettledOverride>,
    pub updated_at: IsoDateTime,
    pub status: OrchestrationV2ShellThreadStatus,
    pub activity_run_status: Option<ThreadShellActivityRunStatus>,
    pub latest_run_completed_at: Option<IsoDateTime>,
    pub latest_run_stop_requested: bool,
    pub latest_run_background_wake: bool,
    pub pending_runtime_request: Option<OrchestrationV2PendingRuntimeRequestSummary>,
    pub pending_background_tasks: Vec<OrchestrationV2PendingBackgroundTask>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct AgentAwarenessRead {
    pub snapshot_sequence: u64,
    pub threads: Vec<AgentAwarenessThread>,
}
pub fn read_awareness_rows(db: &Connection, id: Option<&ThreadId>) -> Result<RawShellSnapshot> {
    let mut statement=db.prepare("SELECT thread_id FROM orchestration_v2_projection_threads WHERE deleted_at IS NULL AND ((?1 IS NOT NULL AND thread_id=?1) OR (?1 IS NULL AND archived_at IS NULL AND json_extract(payload_json,'$.lineage.relationshipToParent') IS NOT 'subagent' AND json_extract(payload_json,'$.lineage.sideChat') IS NOT 1)) ORDER BY updated_at,thread_id")?;
    let ids = statement
        .query_map([id.map(|id| &id.0)], |r| Ok(ThreadId(r.get(0)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    read_shell_rows(
        db,
        &ShellQuery {
            archived: true,
            thread_ids: Some(ids),
            ..Default::default()
        },
        false,
    )
}
pub fn decode_awareness_rows(raw: RawShellSnapshot) -> Result<AgentAwarenessRead> {
    let snapshot = decode_shell_rows(raw)?;
    let mut threads = snapshot
        .threads
        .into_iter()
        .chain(snapshot.archived_threads)
        .map(|shell| {
            Ok(serde_json::from_value(t3_contracts::stored::to_value(
                &shell,
            )?)?)
        })
        .collect::<Result<Vec<AgentAwarenessThread>>>()?;
    threads.sort_by(|a, b| {
        a.updated_at
            .cmp(&b.updated_at)
            .then_with(|| a.id.0.cmp(&b.id.0))
    });
    Ok(AgentAwarenessRead {
        snapshot_sequence: snapshot.snapshot_sequence,
        threads,
    })
}
pub fn agent_awareness_snapshot(
    db: &Connection,
    id: Option<&ThreadId>,
) -> Result<AgentAwarenessRead> {
    decode_awareness_rows(read_awareness_rows(db, id)?)
}

pub fn threads_with_pull_requests(
    db: &Connection,
    id: Option<&ThreadId>,
) -> Result<Vec<OrchestrationV2AppThread>> {
    rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_threads WHERE deleted_at IS NULL AND archived_at IS NULL AND json_array_length(payload_json,'$.pullRequests')>0 AND (?1 IS NULL OR thread_id=?1) ORDER BY updated_at,thread_id",
        [id.map(|id| &id.0)],
    )
}

pub fn active_shell_index(db: &Connection) -> Result<Vec<ActiveShellIndexEntry>> {
    let entries: Vec<Value> = rows(
        db,
        r#"SELECT json_object(
      'id',t.thread_id,'projectId',t.project_id,'lineage',json_extract(t.payload_json,'$.lineage'),
      'settledOverride',json_extract(t.payload_json,'$.settledOverride'),
      'settledAt',json_extract(t.payload_json,'$.settledAt'),
      'pinnedAt',json_extract(t.payload_json,'$.pinnedAt'),
      'snoozedUntil',json_extract(t.payload_json,'$.snoozedUntil'),
      'mayBeLive',(
        EXISTS(SELECT 1 FROM orchestration_v2_projection_runs r WHERE r.thread_id=t.thread_id AND r.status IN('queued','preparing','starting','running','waiting'))
        OR EXISTS(SELECT 1 FROM orchestration_v2_projection_runtime_requests request WHERE request.thread_id=t.thread_id AND request.status='pending')
        OR EXISTS(SELECT 1 FROM orchestration_v2_projection_turn_items item WHERE item.thread_id=t.thread_id AND item.type IN('command_execution','dynamic_tool','subagent') AND item.status NOT IN('completed','interrupted','failed','cancelled'))
        OR EXISTS(SELECT 1 FROM orchestration_v2_projection_turn_items link_request WHERE link_request.thread_id=t.thread_id AND link_request.type='link_request' AND link_request.status='waiting')
        OR EXISTS(SELECT 1 FROM orchestration_v2_projection_provider_threads provider_thread WHERE provider_thread.thread_id=t.thread_id AND json_array_length(provider_thread.payload_json,'$.pendingBackgroundTasks')>0)
        OR EXISTS(SELECT 1 FROM json_each(t.payload_json,'$.pullRequests') link WHERE json_type(link.value,'$.watch') IS NOT NULL)),
      'hasRemoteTasks',t.thread_id IN(SELECT subagent.thread_id FROM orchestration_v2_projection_subagents subagent WHERE subagent.origin='app_owned' AND subagent.child_thread_id IS NULL AND json_type(subagent.payload_json,'$.remoteChild')='object'))
      FROM orchestration_v2_projection_threads t WHERE t.deleted_at IS NULL AND t.archived_at IS NULL ORDER BY t.updated_at,t.thread_id"#,
        [],
    )?;
    entries
        .into_iter()
        .map(|row| {
            Ok(ActiveShellIndexEntry {
                id: serde_json::from_value(row["id"].clone())?,
                project_id: serde_json::from_value(row["projectId"].clone())?,
                lineage: shell_index_lineage(&row["lineage"])?,
                settled_override: shell_index_settled_override(&row["settledOverride"]),
                settled_at: serde_json::from_value(row["settledAt"].clone())?,
                pinned_at: Some(serde_json::from_value(row["pinnedAt"].clone())?),
                snoozed_until: Some(serde_json::from_value(row["snoozedUntil"].clone())?),
                may_be_live: row["mayBeLive"] == 1,
                has_remote_tasks: row["hasRemoteTasks"] == 1,
            })
        })
        .collect()
}

pub fn settlement_candidates(
    db: &Connection,
    id: Option<&ThreadId>,
) -> Result<Vec<OrchestrationV2ThreadShell>> {
    let sql = format!(
        "SELECT t.thread_id FROM orchestration_v2_projection_threads t WHERE t.deleted_at IS NULL AND (?1 IS NULL OR t.thread_id=?1) AND t.archived_at IS NULL AND json_extract(t.payload_json,'$.settledOverride') IS NULL AND json_extract(t.payload_json,'$.pinnedAt') IS NULL AND json_extract(t.payload_json,'$.autoSettleDisabledAt') IS NULL AND (json_extract(t.payload_json,'$.lineage.relationshipToParent') IS NOT 'subagent' OR NOT EXISTS(SELECT 1 FROM orchestration_v2_projection_threads parent WHERE parent.thread_id=json_extract(t.payload_json,'$.lineage.parentThreadId') AND parent.deleted_at IS NULL AND parent.archived_at IS NULL AND json_extract(parent.payload_json,'$.settledOverride') IS NOT 'settled')) AND NOT EXISTS(SELECT 1 FROM orchestration_v2_projection_runs active WHERE active.thread_id=t.thread_id AND active.status IN('preparing','starting','running','waiting')) AND NOT EXISTS(SELECT 1 FROM orchestration_v2_projection_runtime_requests request WHERE request.thread_id=t.thread_id AND request.status='pending') AND NOT EXISTS(SELECT 1 FROM orchestration_v2_projection_turn_items link INDEXED BY orchestration_v2_projection_turn_items_waiting_link_idx WHERE link.thread_id=t.thread_id AND link.type='link_request' AND link.status='waiting' AND NOT EXISTS(SELECT 1 FROM orchestration_v2_projection_messages message WHERE message.thread_id=t.thread_id AND message.role='user' AND json_extract(message.payload_json,'$.createdBy')='user' AND NOT {} AND message.updated_at>COALESCE(json_extract(link.payload_json,'$.startedAt'),link.updated_at))) ORDER BY t.updated_at,t.thread_id",
        idle_compaction_sql("message")
    );
    let mut statement = db.prepare(&sql)?;
    let ids = statement
        .query_map([id.map(|id| &id.0)], |row| Ok(ThreadId(row.get(0)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut raw = read_shell_rows(
        db,
        &ShellQuery {
            thread_ids: Some(ids.clone()),
            ..Default::default()
        },
        false,
    )?;
    let latest_rows: Vec<Value> = rows(
        db,
        &format!(
            "SELECT json_object('threadId',run.thread_id,'run',{}) FROM orchestration_v2_projection_runs run WHERE run.thread_id IN(SELECT value FROM json_each(?1)) AND run.run_id=(SELECT candidate.run_id FROM orchestration_v2_projection_runs candidate WHERE candidate.thread_id=run.thread_id AND NOT {} ORDER BY candidate.ordinal DESC,candidate.run_id DESC LIMIT 1)",
            shell_run_metadata("run"),
            silent_run_sql("candidate")
        ),
        [serde_json::to_string(&ids)?],
    )?;
    let mut latest = latest_rows
        .into_iter()
        .map(|row| {
            (
                row["threadId"].as_str().unwrap_or_default().to_owned(),
                row["run"].clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for row in &mut raw.rows {
        row.facts["presented"] = latest.remove(&row.id).unwrap_or(Value::Null);
        for key in [
            "active",
            "activity",
            "presentedFailure",
            "executedFailure",
            "sessionError",
            "request",
            "secretCard",
            "linkCard",
        ] {
            row.facts[key] = Value::Null;
        }
        row.facts["presentedStop"] = 0.into();
        row.facts["presentedWake"] = 0.into();
    }
    Ok(decode_shell_rows(raw)?.threads)
}

fn tool_output_data(
    value: &Value,
    depth: usize,
    bytes: &mut usize,
    nodes: &mut usize,
    failed: &mut bool,
    exceeded: &mut bool,
) -> Option<Value> {
    if depth > 4 || *nodes == 0 {
        *exceeded = true;
        return None;
    }
    *nodes -= 1;
    if let Some(text) = value.as_str() {
        if text.len() > *bytes {
            *exceeded = true;
            return None;
        }
        *bytes -= text.len();
        return serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|v| tool_output_data(&v, depth + 1, bytes, nodes, failed, exceeded));
    }
    if let Some(array) = value.as_array() {
        if array.len() > 32 {
            *exceeded = true;
            return None;
        }
        let mut data = None;
        for block in array {
            let text = block.get("text").unwrap_or(&Value::Null);
            let text = if text.is_object() {
                text.get("text").unwrap_or(text)
            } else {
                text
            };
            let found = tool_output_data(text, depth + 1, bytes, nodes, failed, exceeded);
            if data.is_none() {
                data = found;
            }
            if *exceeded {
                break;
            }
        }
        return data;
    }
    if !value.is_object() {
        return None;
    }
    *failed |= value["isError"] == true
        || value["is_error"] == true
        || value["_tag"] == "OrchestratorMcpFailure"
        || !value["error"].is_null();
    if let Some(content) = value
        .get("structuredContent")
        .filter(|v| !v.is_null())
        .or_else(|| value.get("content"))
    {
        return tool_output_data(content, depth + 1, bytes, nodes, failed, exceeded);
    }
    Some(value.clone())
}
fn thread_attachment_segment(id: &str) -> String {
    let mut out = String::new();
    for c in id.trim().to_lowercase().chars() {
        let c = if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            c
        } else {
            '-'
        };
        if c != '-' || !out.ends_with('-') {
            out.push(c);
        }
    }
    let mut out = out
        .trim_matches(['-', '_'])
        .chars()
        .take(80)
        .collect::<String>();
    out.truncate(out.trim_end_matches(['-', '_']).len());
    if out == "pending" {
        "_pending".into()
    } else {
        out
    }
}
fn attachment_owned(id: &str, segment: &str) -> bool {
    if segment.is_empty() {
        return false;
    }
    let id = id.to_ascii_lowercase();
    let Some(tail) = id.strip_prefix(&format!("{segment}-")) else {
        return false;
    };
    if tail.len() < 36 {
        return false;
    }
    let uuid = &tail.as_bytes()[..36];
    if !uuid.iter().enumerate().all(|(i, c)| {
        if matches!(i, 8 | 13 | 18 | 23) {
            *c == b'-'
        } else {
            c.is_ascii_hexdigit()
        }
    }) {
        return false;
    }
    tail.len() == 36
        || tail
            .get(36..)
            .and_then(|s| s.strip_prefix('-'))
            .is_some_and(|s| {
                !s.is_empty() && s.len() <= 10 && s.bytes().all(|c| c.is_ascii_alphanumeric())
            })
}
pub fn attachment_ids(db: &Connection, id: &ThreadId) -> Result<Vec<String>> {
    let shown = |column: &str| {
        format!(
            "EXISTS(SELECT 1 FROM orchestration_v2_projection_threads fork JOIN orchestration_v2_projection_runs fork_run ON fork_run.run_id=json_extract(fork.payload_json,'$.forkedFrom.runId') LEFT JOIN orchestration_v2_projection_runs item_run ON item_run.run_id={column} WHERE fork_run.thread_id=?1 AND fork.deleted_at IS NULL AND json_extract(fork.payload_json,'$.forkedFrom.type')='run' AND json_extract(fork.payload_json,'$.forkedFrom.threadId')=?1 AND json_extract(fork.payload_json,'$.lineage.sideChat') IS NOT 1 AND({column} IS NULL OR item_run.ordinal<=fork_run.ordinal))"
        )
    };
    let queries=[format!("SELECT DISTINCT json_extract(attachment.value,'$.id') FROM orchestration_v2_projection_messages message,json_each(message.payload_json,'$.attachments') attachment WHERE message.thread_id=?1 AND NOT {}",shown("message.run_id")),"SELECT DISTINCT json_extract(attachment.value,'$.id') FROM orchestration_v2_projection_threads thread,json_each(thread.payload_json,'$.guestShare.messages') message,json_each(message.value,'$.attachments') attachment WHERE thread.thread_id=?1 AND json_extract(message.value,'$.status')='pending'".into(),format!("SELECT DISTINCT json_extract(attachment.value,'$.id') FROM orchestration_v2_projection_turn_items item,json_each(item.payload_json,'$.questionAnswer.attachmentsByQuestionId') question,json_each(question.value) attachment WHERE item.thread_id=?1 AND item.type='user_input_request' AND NOT {}",shown("item.run_id"))];
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    for query in queries {
        let mut stmt = db.prepare(&query)?;
        for id in stmt.query_map([&id.0], |r| r.get::<_, String>(0))? {
            let id = id?;
            if seen.insert(id.clone()) {
                result.push(id);
            }
        }
    }
    let renders: Vec<OrchestrationV2TurnItem> = rows(
        db,
        &format!(
            "SELECT item.payload_json FROM orchestration_v2_projection_turn_items item WHERE item.thread_id=?1 AND item.type='dynamic_tool' AND(item.payload_json LIKE '%htmlRender%' OR item.payload_json LIKE '%t3McpApp%' OR item.payload_json LIKE '%preview_recording_stop%') AND NOT {}",
            shown("item.run_id")
        ),
        [&id.0],
    )?;
    let segment = thread_attachment_segment(&id.0);
    for mut item in renders {
        let raw = serde_json::to_value(&item)?;
        crate::ports::transport::wire_projection::project_turn_item_for_wire(&mut item)?;
        let compact = serde_json::to_value(item)?;
        let output = &compact["output"];
        let name = raw["toolName"].as_str().unwrap_or("");
        let html = if (name == "execute" || tool_id(name) == Some("html_render"))
            && output["isError"] != true
        {
            output["htmlRender"]["attachmentId"].as_str()
        } else {
            None
        };
        let app = &output["t3McpApp"];
        let server = app["server"].as_str().unwrap_or("");
        let tool = app["tool"].as_str().unwrap_or("");
        let claude_part = |s: &str| {
            s.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
        };
        let app_id = if name == format!("{server}.{tool}")
            || name == format!("mcp__{}__{}", claude_part(server), claude_part(tool))
        {
            app["attachmentId"].as_str()
        } else {
            None
        };
        let mut failed = false;
        let mut exceeded = false;
        let recording = if tool_id(name) == Some("preview_recording_stop") {
            tool_output_data(
                &raw["output"],
                0,
                &mut 16384,
                &mut 128,
                &mut failed,
                &mut exceeded,
            )
            .filter(|_| !failed && !exceeded)
            .and_then(|v| {
                v["id"]
                    .as_str()
                    .filter(|s| s.encode_utf16().count() <= 256 && !s.trim().is_empty())
                    .map(str::to_owned)
            })
        } else {
            None
        };
        if let Some(attachment) = html
            .or(app_id)
            .or(recording.as_deref())
            .filter(|v| attachment_owned(v, &segment))
            && seen.insert(attachment.into())
        {
            result.push(attachment.into());
        }
    }
    Ok(result)
}

#[derive(Debug, Clone, Copy)]
pub enum RecoveryKind {
    QueuedRuns,
    DelegatedCompletions,
    SubagentResults,
    RestartContinuations,
    Runtime,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitRecoveryCandidate {
    pub id: ThreadId,
    pub status: OrchestrationV2ShellThreadStatus,
    pub last_error_class: Option<OrchestrationV2ProviderFailureClass>,
    pub usage_limit_reset_at: Option<IsoDateTime>,
    pub latest_run_id: RunId,
    pub latest_run_completed_at: Option<IsoDateTime>,
    pub updated_at: IsoDateTime,
    pub archived_at: Option<IsoDateTime>,
    pub settled_override: Option<AppThreadSettledOverride>,
    pub pending_runtime_request: Option<OrchestrationV2PendingRuntimeRequestSummary>,
    pub limit_recovery: Option<OrchestrationV2LimitRecovery>,
    pub snoozed_until: Option<IsoDateTime>,
}
pub fn limit_recovery_candidates(
    db: &Connection,
    now: &IsoDateTime,
    auto_resume: bool,
    snooze: bool,
) -> Result<Vec<LimitRecoveryCandidate>> {
    let mut statement = db.prepare(LIMIT_RECOVERY_SQL)?;
    let rows = statement.query_map(params![now.0, auto_resume, snooze], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<String>>(4)?,
        ))
    })?;
    let mut candidates = Vec::new();
    for row in rows {
        let (thread, run, completed, item, last_error) = row?;
        let thread: OrchestrationV2AppThread = serde_json::from_str(&thread)?;
        let item: OrchestrationV2TurnItem = serde_json::from_str(&item)?;
        let OrchestrationV2TurnItem::Error(item) = item else {
            continue;
        };
        if item.failure.class != OrchestrationV2ProviderFailureClass::UsageLimit
            || last_error
                .as_deref()
                .is_some_and(|e| e != item.failure.message.0)
        {
            continue;
        }
        let thread = t3_contracts::stored::to_value(&thread)?;
        let mut candidate = serde_json::json!({"id":thread["id"],"status":"failed","lastErrorClass":"usage_limit","usageLimitResetAt":item.failure.reset_at,"latestRunId":run,"latestRunCompletedAt":completed,"updatedAt":thread["updatedAt"],"archivedAt":thread["archivedAt"],"settledOverride":thread["settledOverride"],"pendingRuntimeRequest":null});
        candidate["limitRecovery"] = thread["limitRecovery"].clone();
        candidate["snoozedUntil"] = thread["snoozedUntil"].clone();
        candidates.push(serde_json::from_value(candidate)?);
    }
    Ok(candidates)
}
pub fn limit_recovery_wake_at(db: &Connection, now: &IsoDateTime) -> Result<Option<i64>> {
    let (reset,snooze):(Option<f64>,Option<f64>)=db.query_row("SELECT MIN(CASE WHEN julianday(reset_at)>julianday(?1) THEN(julianday(reset_at)-2440587.5)*86400000 END),MIN(CASE WHEN julianday(snoozed_until)>julianday(?1) THEN(julianday(snoozed_until)-2440587.5)*86400000 END) FROM(SELECT json_extract(payload_json,'$.limitRecovery.resetAt') reset_at,json_extract(payload_json,'$.snoozedUntil') snoozed_until FROM orchestration_v2_projection_threads WHERE deleted_at IS NULL AND json_extract(payload_json,'$.limitRecovery.autoResume')=1)",[&now.0],|r|Ok((r.get(0)?,r.get(1)?)))?;
    Ok(reset
        .into_iter()
        .chain(snooze)
        .reduce(f64::min)
        .map(|ms| ms.round() as i64))
}
pub fn recovery_thread_ids(
    db: &Connection,
    kind: RecoveryKind,
    include: Option<&[ProjectId]>,
    exclude: Option<&[ProjectId]>,
) -> Result<Vec<ThreadId>> {
    let candidates = match kind {
        RecoveryKind::QueuedRuns => {
            "SELECT thread_id FROM orchestration_v2_projection_runs WHERE status = 'queued' AND NOT EXISTS ( SELECT 1 FROM orchestration_v2_projection_runs AS active WHERE active.thread_id = orchestration_v2_projection_runs.thread_id AND active.status IN ('preparing', 'starting', 'running', 'waiting') )"
        }
        RecoveryKind::DelegatedCompletions => {
            "SELECT thread_id FROM orchestration_v2_projection_runs WHERE CASE WHEN json_valid(payload_json) THEN json_type(payload_json, '$.delegatedCompletion.delivery') = 'object' ELSE 0 END"
        }
        RecoveryKind::SubagentResults => {
            "SELECT child.thread_id FROM orchestration_v2_projection_threads AS child WHERE CASE WHEN json_valid(child.payload_json) THEN json_extract(child.payload_json, '$.lineage.relationshipToParent') = 'subagent' AND json_extract(child.payload_json, '$.lineage.parentThreadId') IS NOT NULL AND json_extract(child.payload_json, '$.forkedFrom.type') = 'node' ELSE 0 END AND (EXISTS ( SELECT 1 FROM orchestration_v2_projection_runs AS requested JOIN orchestration_v2_projection_subagents AS task ON task.subagent_id = json_extract(requested.payload_json, '$.delegatedTaskId') WHERE requested.thread_id = child.thread_id AND requested.status IN ('completed', 'interrupted', 'failed', 'cancelled', 'rolled_back') AND task.child_thread_id = child.thread_id AND json_extract(task.payload_json, '$.origin') = 'app_owned' AND json_extract(task.payload_json, '$.result') IS NULL ) OR ( (SELECT status FROM orchestration_v2_projection_runs WHERE thread_id = child.thread_id AND NOT (status = 'queued' AND json_extract(payload_json, '$.queueHeld') IS 1) ORDER BY ordinal DESC LIMIT 1 ) IN ('completed', 'interrupted', 'failed', 'cancelled', 'rolled_back') AND NOT EXISTS ( SELECT 1 FROM orchestration_v2_projection_context_transfers WHERE source_thread_id = child.thread_id AND target_thread_id = json_extract(child.payload_json, '$.lineage.parentThreadId') AND type = 'subagent_result' )))"
        }
        RecoveryKind::RestartContinuations => {
            "SELECT bindings.thread_id FROM orchestration_v2_projection_provider_sessions AS sessions JOIN orchestration_v2_projection_provider_session_bindings AS bindings ON bindings.provider_session_id = sessions.provider_session_id WHERE sessions.status NOT IN ('stopped', 'error') UNION SELECT thread_id FROM orchestration_v2_projection_provider_sessions AS sessions WHERE status NOT IN ('stopped', 'error') AND thread_id IS NOT NULL AND NOT EXISTS ( SELECT 1 FROM orchestration_v2_projection_provider_session_bindings AS bindings WHERE bindings.provider_session_id = sessions.provider_session_id ) UNION SELECT thread_id FROM orchestration_v2_projection_runs WHERE status = 'starting' AND CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.restartContinuationOfRunId') IS NOT NULL ELSE 0 END"
        }
        RecoveryKind::Runtime => {
            "WITH pending_provider_threads AS MATERIALIZED ( SELECT provider_thread_id, thread_id, owner_node_id FROM orchestration_v2_projection_provider_threads WHERE status = 'active' OR CASE WHEN json_valid(payload_json) THEN json_array_length(payload_json, '$.pendingBackgroundTasks') > 0 ELSE 0 END ) SELECT thread_id FROM orchestration_v2_projection_runs WHERE status IN ('preparing', 'starting', 'running', 'waiting') UNION SELECT thread_id FROM orchestration_v2_projection_runs WHERE status = 'queued' AND CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.queueHeld') IS NOT 1 ELSE 1 END UNION SELECT thread_id FROM orchestration_v2_projection_runtime_requests WHERE status = 'pending' UNION SELECT bindings.thread_id FROM orchestration_v2_projection_provider_sessions AS sessions CROSS JOIN orchestration_v2_projection_provider_session_bindings AS bindings ON bindings.provider_session_id = sessions.provider_session_id WHERE sessions.status NOT IN ('stopped', 'error') OR (sessions.status = 'error' AND CASE WHEN json_valid(sessions.payload_json) THEN json_array_length(sessions.payload_json, '$.extensionStatuses') > 0 ELSE 0 END) UNION SELECT sessions.thread_id FROM orchestration_v2_projection_provider_sessions AS sessions WHERE sessions.status NOT IN ('stopped', 'error') AND sessions.thread_id IS NOT NULL AND NOT EXISTS ( SELECT 1 FROM orchestration_v2_projection_provider_session_bindings AS bindings WHERE bindings.provider_session_id = sessions.provider_session_id ) UNION SELECT thread_id FROM pending_provider_threads UNION SELECT nodes.thread_id FROM pending_provider_threads CROSS JOIN orchestration_v2_projection_nodes AS nodes ON nodes.node_id = pending_provider_threads.owner_node_id UNION SELECT subagents.thread_id FROM pending_provider_threads CROSS JOIN orchestration_v2_projection_subagents AS subagents ON subagents.provider_thread_id = pending_provider_threads.provider_thread_id WHERE subagents.origin = 'provider_native' UNION SELECT subagents.child_thread_id FROM orchestration_v2_projection_subagents AS subagents WHERE subagents.child_thread_id IS NOT NULL AND EXISTS ( SELECT 1 FROM orchestration_v2_projection_nodes AS node INDEXED BY orchestration_v2_projection_nodes_live_idx WHERE node.thread_id = subagents.child_thread_id AND node.status IN ('pending', 'starting', 'running', 'waiting') AND node.run_id IS NULL AND node.kind = 'root_turn' ) UNION SELECT thread_id FROM orchestration_v2_projection_subagents WHERE origin = 'provider_native' AND status IN ('pending', 'running', 'waiting') UNION SELECT item.thread_id FROM orchestration_v2_projection_turn_items AS item WHERE NOT EXISTS ( SELECT 1 FROM orchestration_v2_projection_runs AS run WHERE run.run_id = item.run_id AND run.status = 'rolled_back' ) AND type IN ('command_execution', 'dynamic_tool', 'subagent') AND status IN ('pending', 'running', 'waiting') UNION SELECT thread_id FROM orchestration_v2_effect_outbox WHERE status IN ('pending', 'running')"
        }
    };
    let sql = format!(
        "SELECT thread_id FROM orchestration_v2_projection_threads WHERE deleted_at IS NULL AND thread_id IN ({candidates}) AND (?1 IS NULL OR project_id IN (SELECT value FROM json_each(?1))) AND (?2 IS NULL OR project_id NOT IN (SELECT value FROM json_each(?2))) {} ORDER BY updated_at ASC,thread_id ASC",
        if matches!(kind, RecoveryKind::QueuedRuns) {
            "AND archived_at IS NULL"
        } else {
            ""
        }
    );
    let mut stmt = db.prepare(&sql)?;
    Ok(stmt
        .query_map(
            t3_persistence::rusqlite::params![
                include.map(serde_json::to_string).transpose()?,
                exclude.map(serde_json::to_string).transpose()?
            ],
            |row| Ok(ThreadId(row.get(0)?)),
        )?
        .collect::<std::result::Result<_, _>>()?)
}

fn validate_record(table: &str, raw: &str) -> Result<()> {
    match table {
        "threads" => {
            serde_json::from_str::<OrchestrationV2AppThread>(raw)?;
        }
        "runs" => {
            serde_json::from_str::<OrchestrationV2Run>(raw)?;
        }
        "run_attempts" => {
            serde_json::from_str::<OrchestrationV2RunAttempt>(raw)?;
        }
        "nodes" => {
            serde_json::from_str::<OrchestrationV2ExecutionNode>(raw)?;
        }
        "subagents" => {
            serde_json::from_str::<OrchestrationV2Subagent>(raw)?;
        }
        "provider_sessions" => {
            serde_json::from_str::<OrchestrationV2ProviderSession>(raw)?;
        }
        "provider_threads" => {
            serde_json::from_str::<OrchestrationV2ProviderThread>(raw)?;
        }
        "provider_turns" => {
            serde_json::from_str::<OrchestrationV2ProviderTurn>(raw)?;
        }
        "runtime_requests" => {
            serde_json::from_str::<OrchestrationV2RuntimeRequest>(raw)?;
        }
        "messages" => {
            serde_json::from_str::<OrchestrationV2ConversationMessage>(raw)?;
        }
        "plans" => {
            serde_json::from_str::<OrchestrationV2PlanArtifact>(raw)?;
        }
        "turn_items" => {
            serde_json::from_str::<OrchestrationV2TurnItem>(raw)?;
        }
        "checkpoint_scopes" => {
            serde_json::from_str::<OrchestrationV2CheckpointScope>(raw)?;
        }
        "checkpoints" => {
            serde_json::from_str::<OrchestrationV2Checkpoint>(raw)?;
        }
        "context_handoffs" => {
            serde_json::from_str::<OrchestrationV2ContextHandoff>(raw)?;
        }
        "context_transfers" => {
            serde_json::from_str::<OrchestrationV2ContextTransfer>(raw)?;
        }
        _ => return Err(Error::Unsupported("projection table validator")),
    }
    Ok(())
}
pub fn unreadable_thread_ids(db: &Connection) -> Result<Vec<ThreadId>> {
    let mut actual = BTreeSet::new();
    let mut unreadable = BTreeSet::new();
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (table, key) in [
        ("threads", "thread_id"),
        ("runs", "run_id"),
        ("run_attempts", "attempt_id"),
        ("nodes", "node_id"),
        ("subagents", "subagent_id"),
        ("provider_sessions", "provider_session_id"),
        ("provider_threads", "provider_thread_id"),
        ("provider_turns", "provider_turn_id"),
        ("runtime_requests", "runtime_request_id"),
        ("messages", "message_id"),
        ("plans", "plan_id"),
        ("turn_items", "turn_item_id"),
        ("checkpoint_scopes", "scope_id"),
        ("checkpoints", "checkpoint_id"),
        ("context_handoffs", "context_handoff_id"),
        ("context_transfers", "context_transfer_id"),
    ] {
        let source = if table == "context_transfers" {
            "source_thread_id"
        } else {
            "thread_id"
        };
        let target = if table == "context_transfers" {
            "target_thread_id"
        } else {
            "NULL"
        };
        let mut after = 0_i64;
        loop {
            let mut stmt=db.prepare(&format!("SELECT rowid,{key},{source},{target},payload_json FROM orchestration_v2_projection_{table} WHERE rowid>?1 ORDER BY rowid ASC LIMIT 500"))?;
            let page = stmt
                .query_map([after], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for (rowid, entity, source, target, raw) in &page {
                after = *rowid;
                if table == "threads" {
                    actual.insert(entity.clone());
                }
                if validate_record(table, raw).is_ok() {
                    if table == "threads" {
                        let value: Value = serde_json::from_str(raw)?;
                        if value["forkedFrom"]["type"] == "run"
                            && let Some(parent) = value["forkedFrom"]["threadId"].as_str()
                        {
                            children
                                .entry(parent.to_owned())
                                .or_default()
                                .push(entity.clone());
                        }
                    }
                    continue;
                }
                if table == "provider_sessions" {
                    let mut stmt=db.prepare("SELECT thread_id FROM orchestration_v2_projection_provider_session_bindings WHERE provider_session_id=?1")?;
                    for id in stmt.query_map([entity], |row| row.get::<_, String>(0))? {
                        unreadable.insert(id?);
                    }
                } else {
                    unreadable.extend(source.iter().cloned());
                    unreadable.extend(target.iter().cloned());
                }
                if table == "provider_threads" {
                    let mut stmt=db.prepare("SELECT thread_id FROM orchestration_v2_projection_nodes WHERE node_id=(SELECT owner_node_id FROM orchestration_v2_projection_provider_threads WHERE provider_thread_id=?1) UNION SELECT thread_id FROM orchestration_v2_projection_subagents WHERE provider_thread_id=?1")?;
                    for id in stmt.query_map([entity], |row| row.get::<_, String>(0))? {
                        unreadable.insert(id?);
                    }
                }
            }
            if page.len() < 500 {
                break;
            }
        }
    }
    let mut pending: Vec<_> = unreadable
        .iter()
        .cloned()
        .chain(children.keys().filter(|id| !actual.contains(*id)).cloned())
        .collect();
    let mut index = 0;
    while index < pending.len() {
        if let Some(descendants) = children.get(&pending[index]) {
            for child in descendants {
                if unreadable.insert(child.clone()) {
                    pending.push(child.clone());
                }
            }
        }
        index += 1;
    }
    Ok(unreadable
        .into_iter()
        .filter(|id| actual.contains(id))
        .map(ThreadId)
        .collect())
}

const WINDOW_ANCHORS_SQL: &str = r#"
WITH config AS (SELECT ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13), eligible AS NOT MATERIALIZED (
                  SELECT item.rowid AS item_rowid, item.payload_json, item.ordinal, item.turn_item_id,
                    item.run_id, item.node_id, item.type, item.status, run.status AS run_status,
                    attempt.attempt_id, attempt.status AS attempt_status
                  FROM orchestration_v2_projection_turn_items AS item
                  LEFT JOIN orchestration_v2_projection_runs AS run
                    ON run.run_id = item.run_id
                  LEFT JOIN orchestration_v2_projection_nodes AS node
                    ON ?2 = 1 AND node.node_id = item.node_id
                  LEFT JOIN orchestration_v2_projection_run_attempts AS attempt
                    ON ?2 = 1 AND attempt.attempt_id = (
                      SELECT attempt_id FROM orchestration_v2_projection_run_attempts
                      WHERE run_id = item.run_id AND root_node_id = COALESCE(node.root_node_id, item.node_id)
                      ORDER BY attempt_ordinal DESC LIMIT 1
                    )
                  WHERE item.thread_id = ?1
                    AND item.ordinal <= COALESCE(
                      (
                        SELECT ordinal
                        FROM orchestration_v2_projection_turn_items
                        WHERE thread_id = ?1
                          AND turn_item_id = ?3
                        LIMIT 1
                      ),
                      9223372036854775807
                    )
                    AND (
                      ?4 IS NOT NULL
                      OR run.status IS NULL
                      OR run.status <> 'rolled_back'
                    )
                    AND NOT (
                      item.type = 'user_message'
                      AND (
                        COALESCE(run.status IN ('queued', 'cancelled'), 0)
                        OR (
                          run.status = 'rolled_back'
                          AND EXISTS (
                            SELECT 1
                            FROM orchestration_v2_projection_run_attempts AS attempt
                            WHERE attempt.run_id = run.run_id
                              AND attempt.status = 'cancelled'
                          )
                        )
                      )
                      AND json_extract(item.payload_json, '$.inputIntent') = 'queued_turn'
                    )
                    AND (
                      ?4 IS NULL
                      OR (
                        item.run_id IS NULL
                        AND EXISTS (
                          SELECT 1
                          FROM orchestration_v2_projection_threads AS source_thread
                          WHERE source_thread.thread_id = item.thread_id
                            AND json_extract(source_thread.payload_json, '$.historyOrigin') = 'v1_import'
                        )
                      )
                      OR (
                        run.ordinal <= (
                          SELECT required_run.ordinal
                          FROM orchestration_v2_projection_runs AS required_run
                          WHERE required_run.thread_id = item.thread_id
                            AND required_run.run_id = ?4
                          LIMIT 1
                        )
                        AND NOT COALESCE(
                          run.status = 'rolled_back'
                            AND julianday(run.completed_at) < julianday(?5),
                          0
                        )
                      )
                    )
                    AND NOT (
                      item.type = 'run_interrupt_result'
                      AND EXISTS (
                        SELECT 1
                        FROM orchestration_v2_projection_run_attempts AS attempt
                        WHERE attempt.run_id = item.run_id
                          AND attempt.root_node_id = item.node_id
                          AND attempt.status = 'superseded'
                      )
                      AND NOT EXISTS (
                        SELECT 1
                        FROM orchestration_v2_projection_turn_items AS request
                        WHERE request.thread_id = item.thread_id
                          AND request.run_id = item.run_id
                          AND request.type = 'run_interrupt_request'
                      )
                    )
                ), run_starts AS (
                  SELECT start.ordinal, start.type
                  FROM orchestration_v2_projection_runs AS recent_run
                  CROSS JOIN eligible AS start ON start.item_rowid = (
                    SELECT first_input.rowid
                    FROM orchestration_v2_projection_turn_items AS first_input
                    WHERE first_input.run_id = recent_run.run_id
                      AND first_input.type IN ('user_message', 'notification')
                    ORDER BY first_input.ordinal
                    LIMIT 1
                  )
                  WHERE recent_run.thread_id = ?1
                    AND recent_run.ordinal <= COALESCE(
                      (
                        SELECT anchor_run.ordinal
                        FROM orchestration_v2_projection_turn_items AS anchor_item
                        JOIN orchestration_v2_projection_runs AS anchor_run
                          ON anchor_run.run_id = anchor_item.run_id
                        WHERE anchor_item.thread_id = ?1
                          AND anchor_item.turn_item_id = ?3
                        LIMIT 1
                      ),
                      9223372036854775807
                    )
                    AND ?6 IS NOT NULL
                  ORDER BY recent_run.ordinal DESC
                  LIMIT 152
                ), turn_anchors AS (
                  SELECT ordinal, payload_json
                  FROM (
                    SELECT ordinal, payload_json
                    FROM eligible
                    WHERE type = 'user_message'
                      AND json_extract(payload_json, '$.inputIntent') IN ('turn_start', 'queued_turn')
                      AND ?6 IS NOT NULL
                    ORDER BY ordinal DESC
                    LIMIT 152
                  )
                  UNION ALL
                  SELECT ordinal, NULL FROM run_starts WHERE type = 'notification'
                  ORDER BY ordinal DESC
                  LIMIT 152
                ), user_anchors AS (
                  SELECT ordinal
                  FROM turn_anchors
                  WHERE json_extract(payload_json, '$.createdBy') = 'user'
                  ORDER BY ordinal DESC
                  LIMIT ?8
                ), boundary AS (
                  SELECT CASE
                    WHEN COUNT(*) >= ?8 THEN MIN(ordinal)
                    WHEN (SELECT COUNT(*) FROM turn_anchors) >= 152
                      THEN (SELECT MIN(ordinal) FROM turn_anchors)
                    ELSE 0
                  END AS ordinal
                  FROM user_anchors
                )
                SELECT ordinal, (SELECT MAX(ordinal) FROM user_anchors) AS latest FROM boundary
"#;

const WINDOW_TAIL_SQL: &str = r#"
WITH config AS (SELECT ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13), eligible AS NOT MATERIALIZED (
                  SELECT item.rowid AS item_rowid, item.payload_json, item.ordinal, item.turn_item_id,
                    item.run_id, item.node_id, item.type, item.status, run.status AS run_status,
                    attempt.attempt_id, attempt.status AS attempt_status
                  FROM orchestration_v2_projection_turn_items AS item
                  LEFT JOIN orchestration_v2_projection_runs AS run
                    ON run.run_id = item.run_id
                  LEFT JOIN orchestration_v2_projection_nodes AS node
                    ON ?2 = 1 AND node.node_id = item.node_id
                  LEFT JOIN orchestration_v2_projection_run_attempts AS attempt
                    ON ?2 = 1 AND attempt.attempt_id = (
                      SELECT attempt_id FROM orchestration_v2_projection_run_attempts
                      WHERE run_id = item.run_id AND root_node_id = COALESCE(node.root_node_id, item.node_id)
                      ORDER BY attempt_ordinal DESC LIMIT 1
                    )
                  WHERE item.thread_id = ?1
                    AND item.ordinal <= COALESCE(
                      (
                        SELECT ordinal
                        FROM orchestration_v2_projection_turn_items
                        WHERE thread_id = ?1
                          AND turn_item_id = ?3
                        LIMIT 1
                      ),
                      9223372036854775807
                    )
                    AND (
                      ?4 IS NOT NULL
                      OR run.status IS NULL
                      OR run.status <> 'rolled_back'
                    )
                    AND NOT (
                      item.type = 'user_message'
                      AND (
                        COALESCE(run.status IN ('queued', 'cancelled'), 0)
                        OR (
                          run.status = 'rolled_back'
                          AND EXISTS (
                            SELECT 1
                            FROM orchestration_v2_projection_run_attempts AS attempt
                            WHERE attempt.run_id = run.run_id
                              AND attempt.status = 'cancelled'
                          )
                        )
                      )
                      AND json_extract(item.payload_json, '$.inputIntent') = 'queued_turn'
                    )
                    AND (
                      ?4 IS NULL
                      OR (
                        item.run_id IS NULL
                        AND EXISTS (
                          SELECT 1
                          FROM orchestration_v2_projection_threads AS source_thread
                          WHERE source_thread.thread_id = item.thread_id
                            AND json_extract(source_thread.payload_json, '$.historyOrigin') = 'v1_import'
                        )
                      )
                      OR (
                        run.ordinal <= (
                          SELECT required_run.ordinal
                          FROM orchestration_v2_projection_runs AS required_run
                          WHERE required_run.thread_id = item.thread_id
                            AND required_run.run_id = ?4
                          LIMIT 1
                        )
                        AND NOT COALESCE(
                          run.status = 'rolled_back'
                            AND julianday(run.completed_at) < julianday(?5),
                          0
                        )
                      )
                    )
                    AND NOT (
                      item.type = 'run_interrupt_result'
                      AND EXISTS (
                        SELECT 1
                        FROM orchestration_v2_projection_run_attempts AS attempt
                        WHERE attempt.run_id = item.run_id
                          AND attempt.root_node_id = item.node_id
                          AND attempt.status = 'superseded'
                      )
                      AND NOT EXISTS (
                        SELECT 1
                        FROM orchestration_v2_projection_turn_items AS request
                        WHERE request.thread_id = item.thread_id
                          AND request.run_id = item.run_id
                          AND request.type = 'run_interrupt_request'
                      )
                    )
                ), live_run AS (
                  SELECT live.run_id FROM orchestration_v2_projection_runs AS live
                  WHERE live.thread_id = ?1
                    AND live.status IN ('queued','preparing','starting','running','waiting')
                    AND ?4 IS NULL
                    AND NOT EXISTS (
                      SELECT 1 FROM orchestration_v2_projection_run_attempts
                      WHERE run_id = live.run_id AND status = 'superseded'
                    )
                  ORDER BY +live.ordinal ASC LIMIT 1
                ), visible_tail AS (
                  SELECT ordinal FROM (
                    SELECT ordinal FROM eligible
                    WHERE ordinal >= ?9
                      AND type NOT IN ('command_execution','reasoning','file_change','file_search','web_search','dynamic_tool')
                    ORDER BY ordinal DESC
                    LIMIT ?13
                  )
                  UNION
                  SELECT ordinal FROM (
                    SELECT live_item.ordinal
                    FROM orchestration_v2_projection_turn_items AS live_item
                    WHERE live_item.run_id = (SELECT run_id FROM live_run)
                      AND +live_item.thread_id = ?1
                      AND live_item.ordinal >= ?9
                      AND live_item.ordinal <= COALESCE(
                        (
                          SELECT ordinal FROM orchestration_v2_projection_turn_items
                          WHERE thread_id = ?1
                            AND turn_item_id = ?3
                          LIMIT 1
                        ),
                        9223372036854775807
                      )
                    ORDER BY live_item.ordinal DESC
                    LIMIT ?13
                  )
                  ORDER BY ordinal DESC
                  LIMIT ?13
                ), turn AS (
                  SELECT (
                    SELECT COUNT(*) FROM visible_tail WHERE ordinal >= ?10
                  ) > 500 AS long_turn
                ), tail AS (
                  SELECT visible_tail.ordinal FROM visible_tail
                  ORDER BY visible_tail.ordinal DESC
                  LIMIT 1 OFFSET (
                    SELECT MAX(CASE WHEN long_turn THEN ?7 ELSE ?13 END, 1) - 1
                    FROM turn
                  )
                ), tail_runs AS (
                  SELECT DISTINCT item.run_id
                  FROM tail CROSS JOIN eligible AS item
                  WHERE item.ordinal >= tail.ordinal AND item.run_id IS NOT NULL
                )
                SELECT MIN(MIN(CASE
                  WHEN (
                    SELECT status FROM orchestration_v2_projection_runs
                    WHERE run_id = tail_runs.run_id
                  ) IS NOT 'completed' AND NOT EXISTS (
                    SELECT 1 FROM orchestration_v2_projection_run_attempts
                    WHERE run_id = tail_runs.run_id AND status = 'superseded'
                  ) THEN tail.ordinal
                  ELSE COALESCE(
                    (
                      SELECT start.ordinal FROM eligible AS start
                      WHERE start.run_id = tail_runs.run_id AND start.type = 'user_message'
                        AND start.ordinal <= tail.ordinal
                      ORDER BY start.ordinal DESC LIMIT 1
                    ),
                    (
                      SELECT MIN(first.ordinal) FROM orchestration_v2_projection_turn_items AS first
                      WHERE first.thread_id = ?1 AND first.run_id = tail_runs.run_id
                    ),
                    tail.ordinal
                  )
                END, tail.ordinal)) AS ordinal, (SELECT long_turn FROM turn) AS long_turn
                FROM tail LEFT JOIN tail_runs
"#;

const WINDOW_RETAINED_SQL: &str = r#"
WITH config AS (SELECT ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13), eligible AS NOT MATERIALIZED (
                  SELECT item.rowid AS item_rowid, item.payload_json, item.ordinal, item.turn_item_id,
                    item.run_id, item.node_id, item.type, item.status, run.status AS run_status,
                    attempt.attempt_id, attempt.status AS attempt_status
                  FROM orchestration_v2_projection_turn_items AS item
                  LEFT JOIN orchestration_v2_projection_runs AS run
                    ON run.run_id = item.run_id
                  LEFT JOIN orchestration_v2_projection_nodes AS node
                    ON ?2 = 1 AND node.node_id = item.node_id
                  LEFT JOIN orchestration_v2_projection_run_attempts AS attempt
                    ON ?2 = 1 AND attempt.attempt_id = (
                      SELECT attempt_id FROM orchestration_v2_projection_run_attempts
                      WHERE run_id = item.run_id AND root_node_id = COALESCE(node.root_node_id, item.node_id)
                      ORDER BY attempt_ordinal DESC LIMIT 1
                    )
                  WHERE item.thread_id = ?1
                    AND item.ordinal <= COALESCE(
                      (
                        SELECT ordinal
                        FROM orchestration_v2_projection_turn_items
                        WHERE thread_id = ?1
                          AND turn_item_id = ?3
                        LIMIT 1
                      ),
                      9223372036854775807
                    )
                    AND (
                      ?4 IS NOT NULL
                      OR run.status IS NULL
                      OR run.status <> 'rolled_back'
                    )
                    AND NOT (
                      item.type = 'user_message'
                      AND (
                        COALESCE(run.status IN ('queued', 'cancelled'), 0)
                        OR (
                          run.status = 'rolled_back'
                          AND EXISTS (
                            SELECT 1
                            FROM orchestration_v2_projection_run_attempts AS attempt
                            WHERE attempt.run_id = run.run_id
                              AND attempt.status = 'cancelled'
                          )
                        )
                      )
                      AND json_extract(item.payload_json, '$.inputIntent') = 'queued_turn'
                    )
                    AND (
                      ?4 IS NULL
                      OR (
                        item.run_id IS NULL
                        AND EXISTS (
                          SELECT 1
                          FROM orchestration_v2_projection_threads AS source_thread
                          WHERE source_thread.thread_id = item.thread_id
                            AND json_extract(source_thread.payload_json, '$.historyOrigin') = 'v1_import'
                        )
                      )
                      OR (
                        run.ordinal <= (
                          SELECT required_run.ordinal
                          FROM orchestration_v2_projection_runs AS required_run
                          WHERE required_run.thread_id = item.thread_id
                            AND required_run.run_id = ?4
                          LIMIT 1
                        )
                        AND NOT COALESCE(
                          run.status = 'rolled_back'
                            AND julianday(run.completed_at) < julianday(?5),
                          0
                        )
                      )
                    )
                    AND NOT (
                      item.type = 'run_interrupt_result'
                      AND EXISTS (
                        SELECT 1
                        FROM orchestration_v2_projection_run_attempts AS attempt
                        WHERE attempt.run_id = item.run_id
                          AND attempt.root_node_id = item.node_id
                          AND attempt.status = 'superseded'
                      )
                      AND NOT EXISTS (
                        SELECT 1
                        FROM orchestration_v2_projection_turn_items AS request
                        WHERE request.thread_id = item.thread_id
                          AND request.run_id = item.run_id
                          AND request.type = 'run_interrupt_request'
                      )
                    )
                ), blocked_runs AS MATERIALIZED (
                  SELECT DISTINCT run_id FROM orchestration_v2_projection_turn_items
                  WHERE thread_id = ?1 AND run_id IS NOT NULL
                    AND ?2 = 1
                    AND run_id IN (
                      SELECT run_id FROM eligible
                      WHERE ordinal >= ?11 AND run_id IS NOT NULL
                        AND (run_status = 'completed' OR attempt_status = 'superseded')
                    )
                    AND (type IN ('run_interrupt_request', 'run_interrupt_result')
                      OR (type = 'error' AND status = 'failed' AND parent_item_id IS NULL))
                ), work_segments AS (
                  SELECT item_rowid, ordinal, turn_item_id, run_id, type, attempt_id,
                    MAX(CASE WHEN type = 'user_message' THEN ordinal ELSE 0 END)
                      OVER (PARTITION BY run_id ORDER BY ordinal) AS segment,
                    CASE WHEN status = 'completed' AND (run_status = 'completed' OR attempt_status = 'superseded')
                      AND type IN ('command_execution','reasoning','file_change','file_search','web_search','dynamic_tool')
                      AND NOT (item.type = 'dynamic_tool' AND (item.payload_json LIKE '%htmlRender%' OR item.payload_json LIKE '%t3McpApp%' OR item.payload_json LIKE '%"viewedImagePath"%' OR item.payload_json LIKE '%"toolName":"%delegate_task"%' OR item.payload_json LIKE '%"toolName":"%send_to_user"%' OR item.payload_json LIKE '%sentMessage%'))
                      AND run_id NOT IN (SELECT run_id FROM blocked_runs)
                      THEN 1 ELSE 0 END AS can_defer
                  FROM eligible AS item
                  WHERE ?2 = 1
                    AND (?10 IS NOT NULL) = 1
                    AND ordinal >= ?11
                ), deferred_work AS (
                  SELECT run_id, segment, attempt_id, MIN(ordinal) AS first_ordinal,
                    MAX(ordinal) AS last_ordinal, COUNT(*) AS item_count
                  FROM work_segments WHERE can_defer = 1
                  GROUP BY run_id, segment, attempt_id HAVING COUNT(*) > 150
                ), window_items AS NOT MATERIALIZED (
                  SELECT item_rowid, ordinal, turn_item_id, run_id, type
                  FROM eligible WHERE ?2 = 0
                    OR (?10 IS NULL) = 1
                  UNION ALL
                  SELECT item.item_rowid, item.ordinal, item.turn_item_id, item.run_id, item.type
                  FROM work_segments item LEFT JOIN deferred_work deferred
                    ON deferred.run_id = item.run_id AND deferred.segment = item.segment
                    AND deferred.attempt_id IS item.attempt_id
                  WHERE item.can_defer = 0 OR deferred.run_id IS NULL OR item.ordinal = deferred.first_ordinal
                ), latest_user_turn AS (
                  SELECT COUNT(*) AS row_count
                  FROM (
                    SELECT 1
                    FROM window_items
                    WHERE ordinal >= ?10
                    LIMIT 501
                  )
                ), selected AS (
                  SELECT item_rowid, ordinal, turn_item_id, run_id, type
                  FROM window_items
                  WHERE ordinal >= ?9
                  ORDER BY ordinal DESC, turn_item_id DESC
                  LIMIT CASE
                    WHEN ?7 = 0 THEN 0
                    WHEN ?12 = 1 THEN ?7
                    WHEN (SELECT row_count FROM latest_user_turn)
                      BETWEEN 1 AND 500
                      THEN 502
                    ELSE ?7
                  END
                ), retained AS MATERIALIZED (
                  SELECT item_rowid, ordinal, turn_item_id FROM selected
                  UNION
                  SELECT request.rowid AS item_rowid, request.ordinal, request.turn_item_id
                  FROM orchestration_v2_projection_turn_items AS request
                  WHERE request.run_id IN (
                      SELECT run_id FROM selected
                      WHERE type = 'run_interrupt_result' AND run_id IS NOT NULL
                    )
                    AND request.type = 'run_interrupt_request'
                  UNION
                  SELECT latest.item_rowid, latest.ordinal, latest.turn_item_id
                  FROM (
                    SELECT rowid AS item_rowid, ordinal, turn_item_id
                    FROM orchestration_v2_projection_turn_items
                    WHERE thread_id = ?1
                      AND ?3 IS NULL
                      AND ?4 IS NULL
                    ORDER BY ordinal DESC, turn_item_id DESC
                    LIMIT 1
                  ) AS latest
                  ORDER BY ordinal ASC, turn_item_id ASC
                )
"#;

const LIMIT_RECOVERY_SQL: &str = r#"
          SELECT t.payload_json, r.run_id, r.completed_at,
            item.payload_json AS failure_payload_json,
            (
              SELECT json_extract(session.payload_json, '$.lastError')
              -- Keep bindings outermost: one provider instance can hold every session.
              FROM orchestration_v2_projection_provider_session_bindings binding
              CROSS JOIN orchestration_v2_projection_provider_sessions session
                ON session.provider_session_id = binding.provider_session_id
              WHERE binding.thread_id = t.thread_id
                AND session.provider_instance_id = t.provider_instance_id
              ORDER BY session.updated_at DESC, session.provider_session_id DESC
              LIMIT 1
            ) AS last_error
          FROM orchestration_v2_projection_threads t
          INNER JOIN orchestration_v2_projection_runs r ON r.run_id = (
            SELECT latest.run_id FROM orchestration_v2_projection_runs latest
            WHERE latest.thread_id = t.thread_id
              AND latest.status <> 'queued'
              AND NOT (
                latest.status = 'cancelled'
                AND json_extract(latest.payload_json, '$.startedAt') IS NULL
              )
            -- latestExecutedRun: the run that ended last (runRanAfter).
            ORDER BY latest.completed_at IS NULL DESC, latest.completed_at DESC,
              latest.ordinal DESC, latest.run_id DESC
            LIMIT 1
          ) AND r.status = 'failed'
          INNER JOIN orchestration_v2_projection_turn_items item ON item.turn_item_id = (
            SELECT error.turn_item_id FROM orchestration_v2_projection_turn_items error
            WHERE error.thread_id = t.thread_id AND error.run_id = r.run_id
              AND error.type = 'error' AND error.status = 'failed'
              AND error.node_id IS json_extract(r.payload_json, '$.rootNodeId')
            ORDER BY error.updated_at DESC, error.ordinal DESC, error.turn_item_id DESC
            LIMIT 1
          )
          WHERE t.deleted_at IS NULL
            -- The sweep runs every scheduler tick. Without this list the latest-run
            -- lookup above sorts every run of every thread, tens of milliseconds
            -- for one long thread; only threads with a usage-limit failure qualify.
            AND t.thread_id IN (
              SELECT failure.thread_id FROM orchestration_v2_projection_turn_items failure
              WHERE failure.type = 'error' AND failure.status = 'failed'
                AND json_extract(failure.payload_json, '$.failure.class') = 'usage_limit'
            )
            AND json_extract(t.payload_json, '$.archivedAt') IS NULL
            AND json_extract(t.payload_json, '$.settledOverride') IS NOT 'settled'
            AND json_extract(item.payload_json, '$.failure.class') = 'usage_limit'
            AND json_extract(item.payload_json, '$.failure.resetAt') IS NOT NULL
            AND julianday(json_extract(item.payload_json, '$.failure.resetAt')) > julianday(COALESCE(r.completed_at, json_extract(t.payload_json, '$.updatedAt')))
            AND (
              (
                json_extract(t.payload_json, '$.limitRecovery.runId') IS r.run_id
                AND json_extract(t.payload_json, '$.limitRecovery.resetAt') IS json_extract(item.payload_json, '$.failure.resetAt')
                AND json_extract(t.payload_json, '$.limitRecovery.autoResume') = 1
                AND julianday(json_extract(item.payload_json, '$.failure.resetAt')) <= julianday(?1)
                AND (
                  json_extract(t.payload_json, '$.snoozedUntil') IS NULL
                  OR julianday(json_extract(t.payload_json, '$.snoozedUntil')) <= julianday(?1)
                )
              )
              OR (
                (
                  json_extract(t.payload_json, '$.limitRecovery.runId') IS NOT r.run_id
                  OR json_extract(t.payload_json, '$.limitRecovery.resetAt') IS NOT json_extract(item.payload_json, '$.failure.resetAt')
                )
                AND (
                  ?2
                  OR (?3 AND julianday(json_extract(item.payload_json, '$.failure.resetAt')) > julianday(?1))
                )
                -- A subagent of a settled, archived or deleted parent is abandoned work.
                AND NOT EXISTS (
                  SELECT 1 FROM orchestration_v2_projection_threads parent
                  WHERE json_extract(t.payload_json, '$.lineage.relationshipToParent') = 'subagent'
                    AND parent.thread_id = json_extract(t.payload_json, '$.lineage.parentThreadId')
                    AND (
                      json_extract(parent.payload_json, '$.settledOverride') = 'settled'
                      OR json_extract(parent.payload_json, '$.archivedAt') IS NOT NULL
                      OR parent.deleted_at IS NOT NULL
                    )
                )
              )
            )
            AND NOT EXISTS (
              SELECT 1 FROM orchestration_v2_projection_runtime_requests request
              WHERE request.thread_id = t.thread_id AND request.status = 'pending'
            )
          ORDER BY t.thread_id
        "#;
