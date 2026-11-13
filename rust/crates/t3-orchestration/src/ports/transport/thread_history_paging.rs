use super::wire_projection::encoded_bytes;
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use t3_contracts::{base_schemas::*, orchestration_v2::*, orchestration_v2_id_table::IdOrIndex};

pub const THREAD_HISTORY_INLINE_WORK_ITEM_LIMIT: usize = 150;
pub const OLDER_THREAD_USER_TURN_LIMIT: usize = 20;
pub const THREAD_HISTORY_MAX_RAW_TURNS: usize = 150;
pub const THREAD_HISTORY_SNAPSHOT_ROW_LIMIT: u64 = 77;
pub const THREAD_HISTORY_TURN_SNAPSHOT_ROW_LIMIT: u64 = 502;
#[derive(Debug, Clone, Copy)]
pub struct ThreadHistoryPagePolicy {
    pub max_user_turns: Option<usize>,
    pub max_items: usize,
    pub max_turn_items: usize,
    pub max_encoded_bytes: usize,
}
pub const THREAD_HISTORY_PAGE_POLICY: ThreadHistoryPagePolicy = ThreadHistoryPagePolicy {
    max_user_turns: Some(10),
    max_items: 75,
    max_turn_items: 500,
    max_encoded_bytes: 1_048_576,
};
impl Default for ThreadHistoryPagePolicy {
    fn default() -> Self {
        THREAD_HISTORY_PAGE_POLICY
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadHistoryCursorPayload {
    pub v: u64,
    pub seq: u64,
    pub st: String,
    pub si: String,
    pub p: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inclusive: Option<bool>,
}
fn invalid_cursor() -> Error {
    Error::Service {
        operation: "ThreadHistoryCursor",
        detail: "Invalid thread history cursor.".into(),
    }
}
pub fn base64url_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut encoded = String::with_capacity((bytes.len() * 4).div_ceil(3));
    for chunk in bytes.chunks(3) {
        let bits = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..chunk.len() + 1 {
            encoded.push(ALPHABET[((bits >> (18 - index * 6)) & 63) as usize] as char);
        }
    }
    encoded
}
fn base64url_decode(text: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut bits = 0u32;
    let mut count = 0;
    for byte in text.bytes().take_while(|b| *b != b'=') {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => continue,
        };
        bits = (bits << 6) | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            bytes.push((bits >> count) as u8);
        }
    }
    bytes
}
pub fn encode_thread_history_cursor(payload: &ThreadHistoryCursorPayload) -> Result<String> {
    Ok(base64url_encode(&serde_json::to_vec(payload)?))
}
pub fn decode_thread_history_cursor(cursor: &str) -> Result<ThreadHistoryCursorPayload> {
    if cursor.is_empty() || cursor.encode_utf16().count() > 4096 {
        return Err(invalid_cursor());
    }
    let bytes = base64url_decode(cursor);
    let value: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&bytes)).map_err(|_| invalid_cursor())?;
    let number = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_f64)
            .filter(|n| n.is_finite() && n.fract() == 0.0 && *n >= 0.0 && *n < (u64::MAX as f64))
            .map(|n| n as u64)
            .ok_or_else(invalid_cursor)
    };
    let string = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(invalid_cursor)
    };
    if number("v")? != 1 || value.get("inclusive").is_some_and(|v| !v.is_boolean()) {
        return Err(invalid_cursor());
    }
    Ok(ThreadHistoryCursorPayload {
        v: 1,
        seq: number("seq")?,
        st: string("st")?,
        si: string("si")?,
        p: number("p")?,
        inclusive: value.get("inclusive").and_then(serde_json::Value::as_bool),
    })
}
pub fn id_text<T: AsRef<str>>(id: &IdOrIndex<T>) -> String {
    match id {
        IdOrIndex::Id(id) => id.as_ref().into(),
        IdOrIndex::Index(index) => index.to_string(),
    }
}

macro_rules! item_field {
    ($item:expr, $field:ident) => {
        match $item {
            OrchestrationV2TurnItem::Notification(v) => &v.$field,
            OrchestrationV2TurnItem::UserMessage(v) => &v.$field,
            OrchestrationV2TurnItem::AssistantMessage(v) => &v.$field,
            OrchestrationV2TurnItem::Reasoning(v) => &v.$field,
            OrchestrationV2TurnItem::ProposedPlan(v) => &v.$field,
            OrchestrationV2TurnItem::TodoList(v) => &v.$field,
            OrchestrationV2TurnItem::UserInputRequest(v) => &v.$field,
            OrchestrationV2TurnItem::FileChange(v) => &v.$field,
            OrchestrationV2TurnItem::CommandExecution(v) => &v.$field,
            OrchestrationV2TurnItem::FileSearch(v) => &v.$field,
            OrchestrationV2TurnItem::WebSearch(v) => &v.$field,
            OrchestrationV2TurnItem::ApprovalRequest(v) => &v.$field,
            OrchestrationV2TurnItem::Checkpoint(v) => &v.$field,
            OrchestrationV2TurnItem::RunInterruptRequest(v) => &v.$field,
            OrchestrationV2TurnItem::RunInterruptResult(v)
            | OrchestrationV2TurnItem::SystemNotice(v) => &v.$field,
            OrchestrationV2TurnItem::Error(v) => &v.$field,
            OrchestrationV2TurnItem::Compaction(v) => &v.$field,
            OrchestrationV2TurnItem::Handoff(v) => &v.$field,
            OrchestrationV2TurnItem::Fork(v) => &v.$field,
            OrchestrationV2TurnItem::ThreadCreated(v) => &v.$field,
            OrchestrationV2TurnItem::SecretRequest(v) => &v.$field,
            OrchestrationV2TurnItem::LinkRequest(v) => &v.$field,
            OrchestrationV2TurnItem::Subagent(v) => &v.$field,
            OrchestrationV2TurnItem::DynamicTool(v) => &v.$field,
        }
    };
}
pub fn item_id(item: &OrchestrationV2TurnItem) -> &IdOrIndex<TurnItemId> {
    item_field!(item, id)
}
pub fn item_thread_id(item: &OrchestrationV2TurnItem) -> &IdOrIndex<ThreadId> {
    item_field!(item, thread_id)
}
pub fn item_run_id(item: &OrchestrationV2TurnItem) -> Option<&IdOrIndex<RunId>> {
    item_field!(item, run_id).as_ref()
}
pub fn item_ordinal(item: &OrchestrationV2TurnItem) -> u64 {
    *item_field!(item, ordinal)
}
pub fn item_status(item: &OrchestrationV2TurnItem) -> &OrchestrationV2TurnItemStatus {
    item_field!(item, status)
}
pub fn is_conversation_history_item(item: &OrchestrationV2TurnItem) -> bool {
    !matches!(
        item,
        OrchestrationV2TurnItem::CommandExecution(_)
            | OrchestrationV2TurnItem::Reasoning(_)
            | OrchestrationV2TurnItem::FileChange(_)
            | OrchestrationV2TurnItem::DynamicTool(_)
    )
}
fn is_turn_start(item: &OrchestrationV2TurnItem) -> bool {
    matches!(item, OrchestrationV2TurnItem::UserMessage(v) if matches!(v.input_intent, OrchestrationV2UserMessageInputIntent::TurnStart | OrchestrationV2UserMessageInputIntent::QueuedTurn))
}
pub fn is_thread_history_user_turn(item: &OrchestrationV2TurnItem) -> bool {
    is_turn_start(item)
        && matches!(item, OrchestrationV2TurnItem::UserMessage(v) if v.created_by == OrchestrationV2Actor::User)
}
pub fn projected_row_encoded_bytes(row: &OrchestrationV2ProjectedTurnItem) -> Result<usize> {
    encoded_bytes(row)
}
pub fn projected_row_bounded_snapshot_encoded_bytes(
    row: &OrchestrationV2ProjectedTurnItem,
    thread: &IdOrIndex<ThreadId>,
) -> Result<usize> {
    Ok(encoded_bytes(row)?
        + if is_local_row(row, thread) {
            encoded_bytes(&row.item)?
        } else {
            0
        })
}
fn is_local_row(row: &OrchestrationV2ProjectedTurnItem, thread: &IdOrIndex<ThreadId>) -> bool {
    row.visibility == ProjectedTurnItemVisibility::Local || row.source_thread_id == *thread
}
fn renumber(items: &mut [OrchestrationV2ProjectedTurnItem]) {
    for (index, row) in items.iter_mut().enumerate() {
        row.position = index as u64;
    }
}
fn cursor_for(
    row: &OrchestrationV2ProjectedTurnItem,
    sequence: u64,
    inclusive: bool,
) -> Result<String> {
    encode_thread_history_cursor(&ThreadHistoryCursorPayload {
        v: 1,
        seq: sequence,
        st: id_text(&row.source_thread_id),
        si: id_text(&row.source_item_id),
        p: row.position,
        inclusive: inclusive.then_some(true),
    })
}

#[derive(Debug, Default)]
pub struct ThreadHistoryPrefix {
    pub has_user_turn: bool,
    pub started_runs: BTreeSet<String>,
}
#[derive(Debug)]
pub struct SelectedTimelinePage {
    pub items: Vec<OrchestrationV2ProjectedTurnItem>,
    pub next_cursor: Option<String>,
    pub has_more_history: bool,
    pub oldest_scanned: Option<OrchestrationV2ProjectedTurnItem>,
}

pub fn select_older_timeline_page(
    items: &[OrchestrationV2ProjectedTurnItem],
    end: usize,
    sequence: u64,
    policy: ThreadHistoryPagePolicy,
    row_cost: impl Fn(&OrchestrationV2ProjectedTurnItem) -> Result<usize>,
    prefix: Option<&ThreadHistoryPrefix>,
) -> Result<SelectedTimelinePage> {
    let end = end.min(items.len());
    let mut started = prefix.map(|p| p.started_runs.clone()).unwrap_or_default();
    let starts: Vec<_> = items[..end]
        .iter()
        .map(|row| {
            if !matches!(
                row.item,
                OrchestrationV2TurnItem::UserMessage(_) | OrchestrationV2TurnItem::Notification(_)
            ) {
                return false;
            }
            let first = item_run_id(&row.item).is_some_and(|id| started.insert(id_text(id)));
            if matches!(row.item, OrchestrationV2TurnItem::Notification(_)) {
                first
            } else {
                is_turn_start(&row.item)
            }
        })
        .collect();
    let turn_limit = if prefix.is_some_and(|p| p.has_user_turn)
        || items[..end]
            .iter()
            .any(|row| is_thread_history_user_turn(&row.item))
    {
        policy.max_user_turns
    } else {
        None
    };
    let max_items = if turn_limit.is_some() {
        policy.max_turn_items
    } else {
        policy.max_items
    };
    let mut selected = Vec::new();
    let mut bytes = 0;
    let mut user_turns = 0;
    let mut raw_turns = 0;
    for index in (0..end).rev() {
        if !selected.is_empty()
            && (selected.len() >= max_items
                || turn_limit.is_some_and(|limit| {
                    user_turns >= limit || raw_turns >= THREAD_HISTORY_MAX_RAW_TURNS
                }))
        {
            break;
        }
        let row = &items[index];
        let cost = row_cost(row)?;
        if !selected.is_empty() && bytes + cost > policy.max_encoded_bytes {
            break;
        }
        selected.push(row.clone());
        bytes += cost;
        user_turns += usize::from(is_thread_history_user_turn(&row.item));
        raw_turns += usize::from(starts[index]);
    }
    let oldest_scanned = prefix.and_then(|_| selected.last().cloned());
    if user_turns == 0 {
        selected.truncate(policy.max_items);
    }
    selected.reverse();
    let oldest_index = selected.first().and_then(|old| {
        items.iter().position(|row| {
            row.source_thread_id == old.source_thread_id && row.source_item_id == old.source_item_id
        })
    });
    let has_more_history = oldest_index.is_some_and(|index| index > 0);
    let next_cursor = if has_more_history {
        selected
            .first()
            .map(|row| cursor_for(row, sequence, false))
            .transpose()?
    } else {
        None
    };
    renumber(&mut selected);
    Ok(SelectedTimelinePage {
        items: selected,
        next_cursor,
        has_more_history,
        oldest_scanned,
    })
}
pub fn select_recent_timeline_window(
    items: &[OrchestrationV2ProjectedTurnItem],
    sequence: u64,
    policy: ThreadHistoryPagePolicy,
) -> Result<SelectedTimelinePage> {
    select_older_timeline_page(
        items,
        items.len(),
        sequence,
        policy,
        projected_row_encoded_bytes,
        None,
    )
}
pub fn find_cursor_end_index(
    items: &[OrchestrationV2ProjectedTurnItem],
    cursor: &ThreadHistoryCursorPayload,
) -> usize {
    if let Some(index) = items.iter().position(|row| {
        id_text(&row.source_thread_id) == cursor.st && id_text(&row.source_item_id) == cursor.si
    }) {
        return index + usize::from(cursor.inclusive == Some(true));
    }
    items
        .iter()
        .position(|row| {
            if cursor.inclusive == Some(true) {
                id_text(&row.source_thread_id) == cursor.st && item_ordinal(&row.item) > cursor.p
            } else {
                row.position >= cursor.p
            }
        })
        .unwrap_or(items.len())
}
pub fn older_history_page_policy(
    items: &[OrchestrationV2ProjectedTurnItem],
    cursor: &str,
    policy: Option<ThreadHistoryPagePolicy>,
    through_entry_id: Option<&str>,
) -> Result<ThreadHistoryPagePolicy> {
    let mut policy = policy.unwrap_or(ThreadHistoryPagePolicy {
        max_user_turns: Some(OLDER_THREAD_USER_TURN_LIMIT),
        ..THREAD_HISTORY_PAGE_POLICY
    });
    let Some(target) = through_entry_id else {
        return Ok(policy);
    };
    let end = find_cursor_end_index(items, &decode_thread_history_cursor(cursor)?);
    let Some(target_index) = items
        .iter()
        .position(|row| entry_id(&row.item) == target)
        .filter(|index| *index < end)
    else {
        return Ok(policy);
    };
    let turns = items[target_index..end]
        .iter()
        .filter(|row| is_thread_history_user_turn(&row.item))
        .count();
    policy.max_user_turns = Some(
        (turns + usize::from(!is_thread_history_user_turn(&items[target_index].item))).clamp(
            OLDER_THREAD_USER_TURN_LIMIT,
            OLDER_THREAD_USER_TURN_LIMIT * 2,
        ),
    );
    policy.max_items *= 2;
    policy.max_turn_items *= 2;
    Ok(policy)
}
pub fn entry_id(item: &OrchestrationV2TurnItem) -> String {
    match item {
        OrchestrationV2TurnItem::UserMessage(v) => id_text(&v.message_id),
        OrchestrationV2TurnItem::AssistantMessage(v) => id_text(&v.message_id),
        _ => id_text(item_id(item)),
    }
}
pub fn select_history_page_from_cursor(
    items: &[OrchestrationV2ProjectedTurnItem],
    cursor: &str,
    sequence: u64,
    through_entry_id: Option<&str>,
) -> Result<SelectedTimelinePage> {
    let end = find_cursor_end_index(items, &decode_thread_history_cursor(cursor)?);
    select_older_timeline_page(
        items,
        end,
        sequence,
        older_history_page_policy(items, cursor, None, through_entry_id)?,
        projected_row_encoded_bytes,
        None,
    )
}
pub fn missing_interrupt_requests(
    items: &[OrchestrationV2ProjectedTurnItem],
    page: &[OrchestrationV2ProjectedTurnItem],
) -> Vec<OrchestrationV2ProjectedTurnItem> {
    let mut runs = BTreeSet::new();
    for row in page {
        if matches!(row.item, OrchestrationV2TurnItem::RunInterruptResult(_))
            && let Some(run) = item_run_id(&row.item)
        {
            runs.insert(id_text(run));
        }
    }
    for row in page {
        if matches!(row.item, OrchestrationV2TurnItem::RunInterruptRequest(_))
            && let Some(run) = item_run_id(&row.item)
        {
            runs.remove(&id_text(run));
        }
    }
    items
        .iter()
        .filter(|row| {
            matches!(row.item, OrchestrationV2TurnItem::RunInterruptRequest(_))
                && item_run_id(&row.item).is_some_and(|run| runs.contains(&id_text(run)))
        })
        .cloned()
        .collect()
}
pub fn compute_latest_local_turn_ordinal(items: &[OrchestrationV2TurnItem]) -> Option<u64> {
    items.iter().map(item_ordinal).max()
}

/// SQL joins supply these facts before the wire projection drops them.
#[derive(Debug, Clone)]
pub struct WorkHistoryRow {
    pub row: OrchestrationV2ProjectedTurnItem,
    pub run_status: Option<String>,
    pub attempt_id: Option<String>,
    pub attempt_status: Option<String>,
    pub persistent_output: bool,
}
fn is_deferrable_work_item(row: &WorkHistoryRow) -> bool {
    item_status(&row.row.item) == &OrchestrationV2TurnItemStatus::Completed
        && !row.persistent_output
        && matches!(
            row.row.item,
            OrchestrationV2TurnItem::CommandExecution(_)
                | OrchestrationV2TurnItem::Reasoning(_)
                | OrchestrationV2TurnItem::FileChange(_)
                | OrchestrationV2TurnItem::FileSearch(_)
                | OrchestrationV2TurnItem::WebSearch(_)
                | OrchestrationV2TurnItem::DynamicTool(_)
        )
}
pub fn is_foldable_work_item(row: &WorkHistoryRow) -> bool {
    item_run_id(&row.row.item).is_some()
        && (row.run_status.as_deref() == Some("completed")
            || row.attempt_status.as_deref() == Some("superseded"))
        && is_deferrable_work_item(row)
}
fn set_deferred_work(item: &mut OrchestrationV2TurnItem, deferred: OrchestrationV2DeferredWork) {
    match item {
        OrchestrationV2TurnItem::CommandExecution(v) => v.deferred_work = Some(deferred),
        OrchestrationV2TurnItem::Reasoning(v) => v.deferred_work = Some(deferred),
        OrchestrationV2TurnItem::FileChange(v) => v.deferred_work = Some(deferred),
        OrchestrationV2TurnItem::FileSearch(v) => v.deferred_work = Some(deferred),
        OrchestrationV2TurnItem::WebSearch(v) => v.deferred_work = Some(deferred),
        OrchestrationV2TurnItem::DynamicTool(v) => v.deferred_work = Some(deferred),
        _ => {}
    }
}
pub fn defer_work_history_rows(mut rows: Vec<WorkHistoryRow>) -> Result<Vec<WorkHistoryRow>> {
    let blocked:BTreeSet<_>=rows.iter().filter(|row|matches!(row.row.item,OrchestrationV2TurnItem::RunInterruptRequest(_)|OrchestrationV2TurnItem::RunInterruptResult(_))||matches!(&row.row.item,OrchestrationV2TurnItem::Error(v) if v.status==OrchestrationV2TurnItemStatus::Failed&&v.parent_item_id.is_none())).filter_map(|row|item_run_id(&row.row.item).map(id_text)).collect();
    let mut segments = std::collections::BTreeMap::<String, usize>::new();
    let mut groups = std::collections::BTreeMap::<String, Vec<usize>>::new();
    for (index, row) in rows.iter().enumerate() {
        let Some(run) = item_run_id(&row.row.item).map(id_text) else {
            continue;
        };
        let segment = segments.entry(run.clone()).or_default();
        if matches!(row.row.item, OrchestrationV2TurnItem::UserMessage(_)) {
            *segment += 1;
        }
        if blocked.contains(&run) || !is_foldable_work_item(row) {
            continue;
        }
        groups
            .entry(format!(
                "{}:{run}:{segment}:{}",
                id_text(&row.row.source_thread_id),
                row.attempt_id.as_deref().unwrap_or("")
            ))
            .or_default()
            .push(index);
    }
    let mut omitted = BTreeSet::new();
    for group in groups
        .values()
        .filter(|group| group.len() > THREAD_HISTORY_INLINE_WORK_ITEM_LIMIT)
    {
        let first = group[0];
        let last = &rows[group[group.len() - 1]].row;
        let ordinal = item_ordinal(&last.item);
        let cursor = encode_thread_history_cursor(&ThreadHistoryCursorPayload {
            v: 1,
            seq: 0,
            st: id_text(&last.source_thread_id),
            si: id_text(&last.source_item_id),
            p: ordinal,
            inclusive: Some(true),
        })?;
        let deferred = OrchestrationV2DeferredWork {
            cursor: TrimmedNonEmptyString(cursor),
            item_count: group.len() as u64,
            last_ordinal: ordinal,
        };
        set_deferred_work(&mut rows[first].row.item, deferred);
        omitted.extend(group.iter().skip(1).copied());
    }
    Ok(rows
        .into_iter()
        .enumerate()
        .filter_map(|(index, row)| (!omitted.contains(&index)).then_some(row))
        .collect())
}
pub fn select_deferred_work_page(
    rows: &[WorkHistoryRow],
    cursor: &str,
    through_entry_id: &str,
    sequence: u64,
) -> Result<SelectedTimelinePage> {
    let cursor = decode_thread_history_cursor(cursor)?;
    let items: Vec<_> = rows.iter().map(|row| row.row.clone()).collect();
    let end = find_cursor_end_index(&items, &cursor);
    let start = rows
        .iter()
        .position(|row| id_text(&row.row.source_item_id) == through_entry_id)
        .filter(|start| *start < end)
        .ok_or_else(invalid_cursor)?;
    let anchor = &rows[start];
    let items: Vec<_> = rows[start..end]
        .iter()
        .filter(|row| {
            row.row.source_thread_id == anchor.row.source_thread_id
                && item_run_id(&row.row.item) == item_run_id(&anchor.row.item)
                && row.attempt_id == anchor.attempt_id
                && is_deferrable_work_item(row)
        })
        .map(|row| row.row.clone())
        .collect();
    select_recent_timeline_window(&items, sequence, THREAD_HISTORY_PAGE_POLICY)
}
pub fn bounded_timeline_encoded_bytes(
    visible: &[OrchestrationV2ProjectedTurnItem],
    local: &[OrchestrationV2TurnItem],
) -> Result<usize> {
    visible
        .iter()
        .map(encoded_bytes)
        .chain(local.iter().map(encoded_bytes))
        .sum()
}

#[derive(Debug)]
pub struct BoundedProjectionResult {
    pub projection: OrchestrationV2ThreadProjection,
    pub history_cursor: Option<String>,
    pub has_more_history: bool,
    pub latest_local_turn_ordinal: Option<u64>,
}
fn plan_id(plan: &OrchestrationV2PlanArtifact) -> String {
    match plan {
        OrchestrationV2PlanArtifact::ProposedPlan(v) => id_text(&v.id),
        OrchestrationV2PlanArtifact::TodoList(v) => id_text(&v.id),
    }
}
fn plan_active(plan: &OrchestrationV2PlanArtifact) -> bool {
    match plan {
        OrchestrationV2PlanArtifact::ProposedPlan(v) => {
            v.status == PlanArtifactProposedPlanStatus::Active
        }
        OrchestrationV2PlanArtifact::TodoList(v) => {
            v.status == PlanArtifactProposedPlanStatus::Active
        }
    }
}
fn retained_dependency(item: &OrchestrationV2TurnItem, pending: &BTreeSet<String>) -> bool {
    match item {
        OrchestrationV2TurnItem::RunInterruptRequest(_) => true,
        OrchestrationV2TurnItem::UserInputRequest(v) => pending.contains(&id_text(&v.request_id)),
        OrchestrationV2TurnItem::ApprovalRequest(v) => pending.contains(&id_text(&v.request_id)),
        _ => false,
    }
}

pub fn build_bounded_thread_projection(
    mut projection: OrchestrationV2ThreadProjection,
    sequence: u64,
    policy: ThreadHistoryPagePolicy,
) -> Result<BoundedProjectionResult> {
    let latest_local_turn_ordinal = compute_latest_local_turn_ordinal(&projection.turn_items);
    let pending: BTreeSet<_> = projection
        .runtime_requests
        .iter()
        .filter(|r| r.status == RuntimeRequestStatus::Pending)
        .map(|r| id_text(&r.id))
        .collect();
    let mut latest_todo = None;
    let mut latest_todo_ordinal = None;
    for plan in &projection.plans {
        if let OrchestrationV2PlanArtifact::TodoList(plan) = plan
            && plan.status != PlanArtifactProposedPlanStatus::Superseded
            && let Some(ordinal) = plan
                .run_id
                .as_ref()
                .and_then(|id| projection.runs.iter().find(|run| run.id == *id))
                .map(|run| run.ordinal)
            && latest_todo_ordinal.is_none_or(|old| ordinal >= old)
        {
            latest_todo = Some(id_text(&plan.id));
            latest_todo_ordinal = Some(ordinal);
        }
    }
    let mut all_plans = std::mem::take(&mut projection.plans);
    projection.plans = all_plans
        .iter()
        .filter(|p| plan_active(p) || latest_todo.as_ref() == Some(&plan_id(p)))
        .cloned()
        .collect();
    let mut all_handoffs = std::mem::take(&mut projection.context_handoffs);
    projection.context_handoffs = all_handoffs
        .iter()
        .filter(|h| {
            matches!(
                h.status,
                ContextHandoffStatus::Pending | ContextHandoffStatus::Ready
            )
        })
        .cloned()
        .collect();
    let dependencies: Vec<_> = projection
        .turn_items
        .iter()
        .filter(|i| retained_dependency(i, &pending))
        .cloned()
        .collect();
    let reserve: usize = dependencies.iter().map(encoded_bytes).sum::<Result<_>>()?;
    let turn_items = std::mem::take(&mut projection.turn_items);
    let visible = std::mem::take(&mut projection.visible_turn_items);
    let messages = std::mem::take(&mut projection.messages);
    let control_bytes = encoded_bytes(&projection)?;
    let window_budget = (policy.max_encoded_bytes / 2).saturating_sub(reserve).max(
        policy
            .max_encoded_bytes
            .saturating_sub(reserve + control_bytes + 1024),
    );
    let window = select_older_timeline_page(
        &visible,
        visible.len(),
        sequence,
        ThreadHistoryPagePolicy {
            max_encoded_bytes: window_budget,
            ..policy
        },
        |row| projected_row_bounded_snapshot_encoded_bytes(row, &projection.thread.id),
        None,
    )?;
    projection.visible_turn_items = window.items;
    let mut retained: BTreeSet<_> = BTreeSet::new();
    for row in &projection.visible_turn_items {
        if is_local_row(row, &projection.thread.id) {
            retained.insert(id_text(item_id(&row.item)));
            projection.turn_items.push(row.item.clone());
        }
    }
    for item in dependencies {
        if retained.insert(id_text(item_id(&item))) {
            projection.turn_items.push(item);
        }
    }
    let visible_plans: BTreeSet<_> = projection
        .turn_items
        .iter()
        .filter_map(|item| match item {
            OrchestrationV2TurnItem::ProposedPlan(v) => Some(id_text(&v.plan_id)),
            OrchestrationV2TurnItem::TodoList(v) => Some(id_text(&v.plan_id)),
            _ => None,
        })
        .collect();
    let visible_handoffs: BTreeSet<_> = projection
        .turn_items
        .iter()
        .filter_map(|item| match item {
            OrchestrationV2TurnItem::Handoff(v) => Some(id_text(&v.context_handoff_id)),
            _ => None,
        })
        .collect();
    all_plans.retain(|p| {
        plan_active(p)
            || latest_todo.as_ref() == Some(&plan_id(p))
            || visible_plans.contains(&plan_id(p))
    });
    for plan in &mut all_plans {
        if !plan_active(plan) && latest_todo.as_ref() != Some(&plan_id(plan)) {
            match plan {
                OrchestrationV2PlanArtifact::ProposedPlan(v) => {
                    v.markdown.clear();
                    v.detail_in_turn_item = Some(Some(LiteralTrue));
                }
                OrchestrationV2PlanArtifact::TodoList(v) => {
                    v.explanation = None;
                    v.detail_in_turn_item = Some(Some(LiteralTrue));
                }
            }
        }
    }
    all_handoffs.retain(|h| {
        matches!(
            h.status,
            ContextHandoffStatus::Pending | ContextHandoffStatus::Ready
        ) || visible_handoffs.contains(&id_text(&h.id))
    });
    for handoff in &mut all_handoffs {
        if !matches!(
            handoff.status,
            ContextHandoffStatus::Pending | ContextHandoffStatus::Ready
        ) {
            handoff.summary_text.clear();
            handoff.detail_in_turn_item = Some(Some(LiteralTrue));
        }
    }
    projection.plans = all_plans;
    projection.context_handoffs = all_handoffs;
    let mut message_ids: BTreeSet<_> = projection
        .turn_items
        .iter()
        .filter_map(|item| match item {
            OrchestrationV2TurnItem::UserMessage(v) => Some(id_text(&v.message_id)),
            OrchestrationV2TurnItem::AssistantMessage(v) => Some(id_text(&v.message_id)),
            _ => None,
        })
        .collect();
    let latest_run = projection.runs.iter().reduce(|latest, run| {
        if run.ordinal > latest.ordinal {
            run
        } else {
            latest
        }
    });
    let mut run_ids = BTreeSet::new();
    for run in &projection.runs {
        if latest_run.is_some_and(|latest| latest.id == run.id)
            || matches!(
                run.status,
                OrchestrationV2RunStatus::Preparing
                    | OrchestrationV2RunStatus::Starting
                    | OrchestrationV2RunStatus::Running
                    | OrchestrationV2RunStatus::Waiting
                    | OrchestrationV2RunStatus::Queued
            )
        {
            run_ids.insert(id_text(&run.id));
            message_ids.insert(id_text(&run.user_message_id));
        }
    }
    projection.messages = messages
        .into_iter()
        .filter(|m| {
            message_ids.contains(&id_text(&m.id))
                || m.run_id
                    .as_ref()
                    .is_some_and(|id| run_ids.contains(&id_text(id)))
                || m.delegated_completion.is_some()
                || m.workflow_script == Some(Some(true))
        })
        .collect();
    drop(turn_items);
    Ok(BoundedProjectionResult {
        projection,
        history_cursor: window.next_cursor,
        has_more_history: window.has_more_history,
        latest_local_turn_ordinal,
    })
}
