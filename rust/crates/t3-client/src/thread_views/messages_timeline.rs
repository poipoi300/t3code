//! Display policy transcribed from `MessagesTimeline.logic.ts`.
//!
//! Feed shared timeline entries, not a retained store snapshot. Expansion keys (including
//! synthetic runless and steer keys) are returned by the rows and must be passed back verbatim.
use crate::{
    state::sharing::SharedThreadProjection,
    view_logic::session::{self, ChatMessage, TimelineEntriesInput, TimelineEntry, WorkLogEntry},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_contracts::orchestration_v2::OrchestrationV2ProjectedTurnItem;

mod folds;
mod helpers;
mod presentation;
mod rows;
pub mod viewport;
pub mod work_entry;
pub use helpers::{
    AssistantMessageCopyState, live_answer_preview, live_work_entry_label,
    resolve_assistant_message_copy_state, thread_read_label_prefix, thread_read_target_id,
    thread_read_target_title, timeline_entry_run_id, timeline_entry_starts_response,
    work_entry_display_label, work_entry_is_visible_in_group, work_entry_read_output,
};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineLatestRun {
    pub run_id: String,
    pub status: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_ended: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MessagesTimelineRowsInput {
    pub timeline_entries: Vec<Rc<TimelineEntry>>,
    pub pending_approval_request_ids: BTreeSet<String>,
    pub latest_run: Option<TimelineLatestRun>,
    pub running_run_id: Option<String>,
    pub expanded_run_ids: BTreeSet<String>,
    pub expanded_attempt_ids: BTreeSet<String>,
    pub expanded_work_group_ids: BTreeSet<String>,
    pub is_working: bool,
    pub runless_work_active: bool,
    pub active_turn_started_at: Option<String>,
    pub turn_diff_summaries: Vec<Value>,
    pub supports_conversation_rollback: bool,
    pub thread_start_checkpoint_available: bool,
    pub available_revert_turn_counts: Option<BTreeSet<u64>>,
    pub worktree_setup: Option<Value>,
    pub tool_call_visibility: String,
    pub revealed_message_id: Option<String>,
}

impl MessagesTimelineRowsInput {
    pub fn entries_from_thread(thread: &SharedThreadProjection) -> TimelineEntriesInput {
        TimelineEntriesInput {
            visible_turn_items: thread.visible_turn_items.to_vec(),
            attempts: thread.attempts.to_vec(),
            nodes: thread.nodes.to_vec(),
            plans: thread.plans.iter().map(|p| p.as_ref().clone()).collect(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagesTimelineRow {
    pub id: String,
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continues_work_log: Option<bool>,
    #[serde(flatten)]
    pub content: MessagesTimelineRowContent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum MessagesTimelineRowContent {
    WorktreeSetup {
        snapshot: Value,
        embedded: bool,
    },
    Work {
        grouped_entries: Vec<Rc<WorkLogEntry>>,
        is_expanded_tool_group: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_label: Option<String>,
    },
    WorkLive {
        entry: Rc<WorkLogEntry>,
        grouped_entries: Vec<Rc<WorkLogEntry>>,
        group_id: String,
        expanded: bool,
        active: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought: Option<Rc<WorkLogEntry>>,
    },
    Working {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        compacting: Option<bool>,
    },
    Thinking {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expanded: Option<bool>,
    },
    WorkToggle {
        run_id: Option<String>,
        group_id: String,
        hidden_count: usize,
        expanded: bool,
        summary: String,
        summary_kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_surface: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_icon: Option<Value>,
        has_failure: bool,
    },
    ViewedImages {
        paths: Vec<String>,
    },
    TurnFold {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deferred_work_item_ids: Option<Vec<String>>,
        run_id: String,
        label: String,
        expanded: bool,
    },
    AttemptFold {
        deferred_work_item_ids: Vec<String>,
        run_id: String,
        attempt_id: String,
        label: String,
        expanded: bool,
    },
    ContextCompaction {
        label: String,
        token_counts: Value,
        active: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failed: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<String>,
    },
    Message {
        message: Rc<ChatMessage>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        projected_item: Option<Rc<OrchestrationV2ProjectedTurnItem>>,
        show_assistant_meta: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        suppress_empty_response: Option<bool>,
        show_assistant_copy_button: bool,
        assistant_copy_streaming: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        assistant_turn_diff_summary: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revert_turn_count: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        grouped_with_next: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        grouped_with_previous: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        continues_steer: Option<bool>,
    },
    AssistantMeta {
        message: Rc<ChatMessage>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        projected_item: Option<Rc<OrchestrationV2ProjectedTurnItem>>,
        show_assistant_copy_button: bool,
        assistant_copy_streaming: bool,
    },
    Event {
        projected_item: Rc<OrchestrationV2ProjectedTurnItem>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagents: Option<Vec<Rc<OrchestrationV2ProjectedTurnItem>>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resource_summary: Option<bool>,
    },
    ProposedPlan {
        proposed_plan: Value,
    },
    HtmlRender {
        html_render: Value,
    },
    SentMessage {
        text: String,
    },
    McpApp {
        source_thread_id: String,
        item_id: String,
        revision: String,
        mcp_app: Value,
    },
}

#[derive(Debug, Clone)]
struct Entry {
    value: Value,
    work: Option<WorkLogEntry>,
}
impl Entry {
    fn new(value: Value) -> Self {
        let work = (value["kind"] == "work")
            .then(|| serde_json::from_value(value["entry"].clone()).expect("timeline work entry"));
        Self { value, work }
    }
    fn id(&self) -> &str {
        s(&self.value["id"])
    }
    fn kind(&self) -> &str {
        s(&self.value["kind"])
    }
    fn at(&self) -> &str {
        s(&self.value["createdAt"])
    }
    fn w(&self) -> &Value {
        &self.value["entry"]
    }
    fn message(&self) -> &Value {
        &self.value["message"]
    }
    fn projected(&self) -> &Value {
        if self.kind() == "work" {
            &self.w()["projectedItem"]
        } else {
            &self.value["projectedItem"]
        }
    }
    fn item(&self) -> &Value {
        &self.projected()["item"]
    }
    fn attempt(&self) -> &Value {
        &self.value["attempt"]
    }
    fn end(&self) -> &str {
        if self.kind() == "message" {
            s(&self.message()["updatedAt"])
        } else {
            self.at()
        }
    }
    fn compaction(&self) -> bool {
        self.kind() == "work" && self.w()["sourceActivityKind"] == "context-compaction"
    }
    fn persistent(&self) -> bool {
        matches!(
            s(&self.item()["type"]),
            "fork" | "thread_created" | "secret_request" | "link_request"
        ) && self.kind() == "event"
    }
}
fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}
fn optional_id(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::to_owned)
        .or_else(|| v.as_u64().map(|n| n.to_string()))
}
fn truth(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.),
        _ => true,
    }
}
fn value<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).expect("timeline values serialize")
}
#[cfg(test)]
fn strings(v: &Value) -> impl Iterator<Item = &str> {
    v.as_array().into_iter().flatten().filter_map(Value::as_str)
}
fn elapsed(a: &str, b: &str) -> Option<f64> {
    Some((crate::time::epoch_ms(b)? - crate::time::epoch_ms(a)?).max(0) as f64)
}
fn max_iso<'a>(a: Option<&'a str>, b: Option<&'a str>) -> Option<&'a str> {
    match (a, b) {
        (None, b) => b,
        (a, None) => a,
        (Some(a), Some(b)) => match (crate::time::epoch_ms(a), crate::time::epoch_ms(b)) {
            (None, _) => Some(b),
            (_, None) => Some(a),
            (Some(x), Some(y)) => Some(if y > x { b } else { a }),
        },
    }
}
fn group_id(id: &str) -> String {
    format!("work-group:{id}")
}
fn entry_run_id(e: &Entry) -> Option<String> {
    match e.kind() {
        "message" if e.message()["role"] == "assistant" => optional_id(&e.message()["runId"]),
        "proposed-plan" => optional_id(&e.value["proposedPlan"]["runId"]),
        "html-render" | "mcp-app" | "sent-message" => optional_id(&e.value["runId"]),
        "work" => optional_id(&e.w()["runId"]),
        _ => None,
    }
}
fn steer(e: &Entry) -> bool {
    e.kind() == "message"
        && e.message()["role"] == "user"
        && matches!(
            s(&e.message()["inputIntent"]),
            "steer" | "promoted_queued_to_steer"
        )
}
fn starts_response(e: &Entry) -> bool {
    (e.kind() == "message" && e.message()["role"] == "user" && !steer(e))
        || (e.kind() == "work" && e.w()["itemType"] == "notification")
}
fn boundary(entries: &[Entry]) -> Option<usize> {
    entries.iter().rposition(starts_response)
}
fn unsettled(input: &MessagesTimelineRowsInput) -> Option<String> {
    input.running_run_id.clone().or_else(|| {
        input
            .latest_run
            .as_ref()
            .filter(|r| {
                r.completed_at.is_none()
                    || matches!(r.status.as_str(), "running" | "starting" | "waiting")
            })
            .map(|r| r.run_id.clone())
    })
}
fn active_runs(entries: &[Entry], input: &MessagesTimelineRowsInput) -> BTreeSet<String> {
    let Some(run) = unsettled(input) else {
        return BTreeSet::new();
    };
    let mut ids = BTreeSet::from([run]);
    if input.is_working {
        ids.extend(
            entries
                .iter()
                .skip(boundary(entries).map_or(0, |i| i + 1))
                .filter_map(entry_run_id),
        );
    }
    ids
}

pub fn derive_messages_timeline_rows(
    input: &MessagesTimelineRowsInput,
) -> Vec<Rc<MessagesTimelineRow>> {
    let mut rows: Vec<MessagesTimelineRow> = rows::derive(input, &raw_entries(input))
        .into_iter()
        .map(|row| serde_json::from_value(row).expect("derived timeline row"))
        .collect();
    restore_shared_rows(&mut rows, input);
    rows.into_iter().map(Rc::new).collect()
}
fn projected_key(p: &OrchestrationV2ProjectedTurnItem) -> (String, String) {
    (
        value(&p.source_thread_id).to_string(),
        value(&p.source_item_id).to_string(),
    )
}
fn restore_shared_rows(rows: &mut [MessagesTimelineRow], input: &MessagesTimelineRowsInput) {
    let mut messages = BTreeMap::new();
    let mut projected = BTreeMap::new();
    let mut work = BTreeMap::new();
    for e in &input.timeline_entries {
        let item = match &e.content {
            session::TimelineContent::Message {
                message,
                projected_item,
            } => {
                messages.insert(value(&message.id).to_string(), message);
                projected_item.as_ref()
            }
            session::TimelineContent::Work { entry } => {
                work.insert(entry.id.as_str(), entry.as_ref());
                entry.projected_item.as_ref()
            }
            session::TimelineContent::Event { projected_item } => Some(projected_item),
            _ => None,
        };
        if let Some(item) = item {
            projected.insert(projected_key(item), item);
        }
    }
    let restore_item = |item: &mut Rc<OrchestrationV2ProjectedTurnItem>| {
        if let Some(source) = projected
            .get(&projected_key(item))
            .filter(|s| s.as_ref() == item.as_ref())
        {
            *item = (*source).clone();
        }
    };
    let mut work_cache = BTreeMap::new();
    let mut restore_work = |entry: &mut Rc<WorkLogEntry>| {
        let key = (entry.id.clone(), entry.tool_lifecycle_status.clone());
        let shared = work_cache.entry(key).or_insert_with(|| {
            let mut next = entry.as_ref().clone();
            if let Some(source) = work.get(entry.id.as_str()) {
                let mut original = (*source).clone();
                original
                    .tool_lifecycle_status
                    .clone_from(&entry.tool_lifecycle_status);
                if original == next {
                    next = original;
                }
            }
            if let Some(item) = &mut next.projected_item {
                restore_item(item);
            }
            Rc::new(next)
        });
        *entry = shared.clone();
    };
    for row in rows {
        match &mut row.content {
            MessagesTimelineRowContent::Message {
                message,
                projected_item,
                ..
            }
            | MessagesTimelineRowContent::AssistantMeta {
                message,
                projected_item,
                ..
            } => {
                if let Some(source) = messages
                    .get(&value(&message.id).to_string())
                    .filter(|s| s.as_ref() == message.as_ref())
                {
                    *message = (*source).clone();
                }
                if let Some(item) = projected_item {
                    restore_item(item);
                }
            }
            MessagesTimelineRowContent::Work {
                grouped_entries, ..
            } => {
                for entry in grouped_entries {
                    restore_work(entry);
                }
            }
            MessagesTimelineRowContent::WorkLive {
                entry,
                grouped_entries,
                thought,
                ..
            } => {
                restore_work(entry);
                for entry in grouped_entries {
                    restore_work(entry);
                }
                if let Some(thought) = thought {
                    restore_work(thought);
                }
            }
            MessagesTimelineRowContent::Event {
                projected_item,
                subagents,
                ..
            } => {
                restore_item(projected_item);
                if let Some(subagents) = subagents {
                    for item in subagents {
                        restore_item(item);
                    }
                }
            }
            _ => {}
        }
    }
}
fn raw_entries(input: &MessagesTimelineRowsInput) -> Vec<Entry> {
    input
        .timeline_entries
        .iter()
        .map(|e| Entry::new(value(e)))
        .collect()
}

pub fn timeline_turn_fold_run_ids_by_entry_id(
    input: &MessagesTimelineRowsInput,
) -> BTreeMap<String, String> {
    let entries = helpers::shown_entries(input, &raw_entries(input));
    folds::turn_folds(&entries, input)
        .into_iter()
        .flat_map(|f| f.hidden.into_iter().map(move |id| (id, f.run_id.clone())))
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct StableMessagesTimelineRowsState {
    pub result: Rc<Vec<Rc<MessagesTimelineRow>>>,
}
pub fn compute_stable_messages_timeline_rows(
    rows: Vec<Rc<MessagesTimelineRow>>,
    previous: &StableMessagesTimelineRowsState,
) -> StableMessagesTimelineRowsState {
    let by_id: BTreeMap<_, _> = previous.result.iter().map(|r| (&r.id, r)).collect();
    let result: Vec<_> = rows
        .into_iter()
        .map(|r| {
            by_id
                .get(&r.id)
                .filter(|old| helpers::row_unchanged(old, &r))
                .map_or(r.clone(), |old| (*old).clone())
        })
        .collect();
    if result.len() == previous.result.len()
        && result
            .iter()
            .zip(previous.result.iter())
            .all(|(a, b)| Rc::ptr_eq(a, b))
    {
        previous.clone()
    } else {
        StableMessagesTimelineRowsState {
            result: Rc::new(result),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MessagesTimelineRowsProjection {
    pub input: MessagesTimelineRowsInput,
    pub rows: Rc<Vec<Rc<MessagesTimelineRow>>>,
}
pub fn same_rows_input(
    left: &MessagesTimelineRowsInput,
    right: &MessagesTimelineRowsInput,
) -> bool {
    left.timeline_entries.len() == right.timeline_entries.len()
        && left
            .timeline_entries
            .iter()
            .zip(&right.timeline_entries)
            .all(|(a, b)| Rc::ptr_eq(a, b))
        && same_rows_context(left, right)
}
fn same_rows_context(left: &MessagesTimelineRowsInput, right: &MessagesTimelineRowsInput) -> bool {
    let mut left = left.clone();
    left.timeline_entries.clear();
    let mut right = right.clone();
    right.timeline_entries.clear();
    left == right
}
pub fn derive_messages_timeline_rows_with_state(
    input: &MessagesTimelineRowsInput,
    previous: Option<&MessagesTimelineRowsProjection>,
) -> MessagesTimelineRowsProjection {
    if let Some(previous) = previous {
        if same_rows_input(&previous.input, input) {
            return MessagesTimelineRowsProjection {
                input: input.clone(),
                rows: previous.rows.clone(),
            };
        }
        if let Some(rows) = helpers::patch_streaming(input, previous) {
            let unchanged = rows.len() == previous.rows.len()
                && rows
                    .iter()
                    .zip(previous.rows.iter())
                    .all(|(a, b)| Rc::ptr_eq(a, b));
            return MessagesTimelineRowsProjection {
                input: input.clone(),
                rows: if unchanged {
                    previous.rows.clone()
                } else {
                    Rc::new(rows)
                },
            };
        }
    }
    let stable = compute_stable_messages_timeline_rows(
        derive_messages_timeline_rows(input),
        &StableMessagesTimelineRowsState {
            result: previous.map_or_else(|| Rc::new(vec![]), |p| p.rows.clone()),
        },
    );
    MessagesTimelineRowsProjection {
        input: input.clone(),
        rows: stable.result,
    }
}

pub fn handoff_runs_signature(runs: &[Value]) -> String {
    runs.iter()
        .map(|r| {
            format!(
                "{}\0{}\0{}\0{}\0{}",
                s(&r["id"]),
                s(&r["providerInstanceId"]),
                r["ordinal"],
                s(&r["modelSelection"]["instanceId"]),
                s(&r["modelSelection"]["model"])
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests;
