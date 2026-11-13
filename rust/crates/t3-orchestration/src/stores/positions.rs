use super::event_store::statement;
use crate::error::{Error, Result};
use t3_contracts::base_schemas::{RunId, ThreadId, TurnItemId};
use t3_contracts::orchestration_v2::OrchestrationV2TurnItem;
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use t3_persistence::rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, Clone, Copy, Default)]
pub enum Placement {
    #[default]
    Append,
    RunStart,
}

const EXISTING: &str = "SELECT ordinal FROM orchestration_v2_turn_item_positions \
    WHERE thread_id = ?1 AND turn_item_id = ?2 LIMIT 1";
const RUN_ORDINAL: &str = "SELECT ordinal FROM orchestration_v2_projection_runs \
    WHERE thread_id = ?1 AND run_id = ?2 LIMIT 1";
const INSERT_APPEND: &str = "INSERT INTO orchestration_v2_turn_item_positions \
    (thread_id, turn_item_id, ordinal) \
    SELECT ?1, ?2, COALESCE(MAX(ordinal), ?3) + 1 FROM orchestration_v2_turn_item_positions \
    WHERE thread_id = ?1 AND ordinal >= ?3 AND ordinal <= ?4 \
    ON CONFLICT (thread_id, turn_item_id) DO NOTHING";
const INSERT_RUN_START: &str = "INSERT INTO orchestration_v2_turn_item_positions \
    (thread_id, turn_item_id, ordinal) \
    SELECT ?1, ?2, CASE WHEN COALESCE(MIN(ordinal), ?4) > ?3 THEN ?3 ELSE MAX(ordinal) + 1 END \
    FROM orchestration_v2_turn_item_positions \
    WHERE thread_id = ?1 AND ordinal >= ?3 AND ordinal <= ?4 \
    ON CONFLICT (thread_id, turn_item_id) DO NOTHING";

pub fn allocate(
    db: &Connection,
    thread_id: &ThreadId,
    item_id: &TurnItemId,
    run_id: Option<&RunId>,
    run_ordinal: Option<i64>,
    placement: Placement,
) -> Result<i64> {
    let existing = statement(db, EXISTING)?
        .query_row(params![thread_id.0, item_id.0], |row| row.get(0))
        .optional()?;
    if let Some(ordinal) = existing {
        return Ok(ordinal);
    }
    let stored_ordinal = match run_id {
        Some(run_id) => statement(db, RUN_ORDINAL)?
            .query_row(params![thread_id.0, run_id.0], |row| row.get::<_, i64>(0))
            .optional()?,
        None => None,
    };
    let lower = run_ordinal.or(stored_ordinal).unwrap_or(0) * 1_000_000;
    let upper = lower + 999_999;
    let insert = match placement {
        Placement::Append => INSERT_APPEND,
        Placement::RunStart => INSERT_RUN_START,
    };
    statement(db, insert)?.execute(params![thread_id.0, item_id.0, lower, upper])?;
    Ok(statement(db, EXISTING)?.query_row(params![thread_id.0, item_id.0], |row| row.get(0))?)
}

/// Every turn-item variant carries the base fields; an added variant fails to compile here.
macro_rules! base_fields {
    ($item:expr, |$fields:ident| $body:expr) => {
        match $item {
            OrchestrationV2TurnItem::Notification($fields) => $body,
            OrchestrationV2TurnItem::UserMessage($fields) => $body,
            OrchestrationV2TurnItem::AssistantMessage($fields) => $body,
            OrchestrationV2TurnItem::Reasoning($fields) => $body,
            OrchestrationV2TurnItem::ProposedPlan($fields) => $body,
            OrchestrationV2TurnItem::TodoList($fields) => $body,
            OrchestrationV2TurnItem::UserInputRequest($fields) => $body,
            OrchestrationV2TurnItem::FileChange($fields) => $body,
            OrchestrationV2TurnItem::CommandExecution($fields) => $body,
            OrchestrationV2TurnItem::FileSearch($fields) => $body,
            OrchestrationV2TurnItem::WebSearch($fields) => $body,
            OrchestrationV2TurnItem::ApprovalRequest($fields) => $body,
            OrchestrationV2TurnItem::Checkpoint($fields) => $body,
            OrchestrationV2TurnItem::RunInterruptRequest($fields) => $body,
            OrchestrationV2TurnItem::RunInterruptResult($fields) => $body,
            OrchestrationV2TurnItem::SystemNotice($fields) => $body,
            OrchestrationV2TurnItem::Error($fields) => $body,
            OrchestrationV2TurnItem::Compaction($fields) => $body,
            OrchestrationV2TurnItem::Handoff($fields) => $body,
            OrchestrationV2TurnItem::Fork($fields) => $body,
            OrchestrationV2TurnItem::ThreadCreated($fields) => $body,
            OrchestrationV2TurnItem::SecretRequest($fields) => $body,
            OrchestrationV2TurnItem::LinkRequest($fields) => $body,
            OrchestrationV2TurnItem::Subagent($fields) => $body,
            OrchestrationV2TurnItem::DynamicTool($fields) => $body,
        }
    };
}

/// Positions are keyed by real ids; compact id-table indices never reach storage.
fn expanded<T: Clone>(field: &'static str, id: &IdOrIndex<T>) -> Result<T> {
    match id {
        IdOrIndex::Id(id) => Ok(id.clone()),
        IdOrIndex::Index(index) => Err(Error::Decode {
            entity: "turn item",
            detail: format!("{field} is the compact index {index}; expand ids before storing"),
        }),
    }
}

pub fn normalize(
    db: &Connection,
    mut item: OrchestrationV2TurnItem,
    run_ordinal: Option<i64>,
) -> Result<OrchestrationV2TurnItem> {
    let (thread_id, item_id, run_id) = base_fields!(&item, |fields| (
        expanded("threadId", &fields.thread_id)?,
        expanded("id", &fields.id)?,
        fields
            .run_id
            .as_ref()
            .map(|run_id| expanded("runId", run_id))
            .transpose()?,
    ));
    let ordinal = allocate(
        db,
        &thread_id,
        &item_id,
        run_id.as_ref(),
        run_ordinal,
        Placement::Append,
    )?;
    let ordinal = u64::try_from(ordinal).map_err(|_| Error::Decode {
        entity: "turn item position",
        detail: format!("negative ordinal {ordinal}"),
    })?;
    base_fields!(&mut item, |fields| fields.ordinal = ordinal);
    Ok(item)
}
