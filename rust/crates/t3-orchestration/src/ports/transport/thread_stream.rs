use super::{
    thread_history_paging::{
        BoundedProjectionResult, THREAD_HISTORY_PAGE_POLICY, build_bounded_thread_projection,
        id_text,
    },
    wire_projection::{
        cap_projection_checkpoint_files_for_wire, encoded_bytes, project_thread_projection_for_wire,
    },
};
use crate::error::Result;
use serde::Serialize;
use t3_contracts::orchestration_v2::*;

pub const THREAD_RESUME_MAX_REPLAY_EVENTS: usize = 128;
pub const THREAD_RESUME_MAX_REPLAY_ENCODED_BYTES: usize = 1_048_576;
pub const THREAD_RESUME_MAX_RAW_PAYLOAD_BYTES: usize = 1_048_576;
pub fn is_thread_replay_raw_payload_safe(bytes: usize) -> bool {
    bytes <= THREAD_RESUME_MAX_RAW_PAYLOAD_BYTES
}
pub fn thread_replay_encoded_bytes<T: Serialize>(items: &[T], limit: usize) -> Result<usize> {
    let mut total = 0;
    for item in items {
        total += encoded_bytes(item)?;
        if total > limit {
            break;
        }
    }
    Ok(total)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadResumePlan {
    Replay {
        after_sequence: u64,
        through_sequence: u64,
    },
    Snapshot,
}
pub fn decide_thread_resume(
    after_sequence: u64,
    high_water: u64,
    event_count: usize,
    encoded_bytes: usize,
) -> ThreadResumePlan {
    if after_sequence > high_water
        || event_count > THREAD_RESUME_MAX_REPLAY_EVENTS
        || encoded_bytes > THREAD_RESUME_MAX_REPLAY_ENCODED_BYTES
    {
        ThreadResumePlan::Snapshot
    } else {
        ThreadResumePlan::Replay {
            after_sequence,
            through_sequence: high_water,
        }
    }
}
#[derive(Debug, Default, Clone, Copy)]
pub struct SnapshotOptions {
    pub compact_turn_items: bool,
    pub dedupe_bodies: bool,
    pub compact_ids: bool,
}

pub fn bounded_snapshot_response_fields(
    mut bounded: BoundedProjectionResult,
    sequence: u64,
    options: SnapshotOptions,
) -> OrchestrationV2ThreadStreamItem {
    cap_projection_checkpoint_files_for_wire(&mut bounded.projection);
    if options.dedupe_bodies {
        dedupe_snapshot_bodies(&mut bounded.projection);
    }
    let compacted =
        options.compact_turn_items && omit_local_visible_turn_items(&mut bounded.projection);
    OrchestrationV2ThreadStreamItem::Snapshot(Box::new(ThreadStreamItemMember2 {
        kind: ThreadStreamItemMember2Kind::Snapshot,
        snapshot_sequence: sequence,
        projection: bounded.projection,
        history_cursor: Some(
            bounded
                .history_cursor
                .map(t3_contracts::base_schemas::TrimmedNonEmptyString),
        ),
        has_more_history: Some(bounded.has_more_history),
        latest_local_turn_ordinal: Some(bounded.latest_local_turn_ordinal),
        turn_items_omit_local_visible: compacted.then_some(LiteralTrue),
        id_table: options.compact_ids.then_some(LiteralTrue),
        ids: None,
    }))
}
pub fn build_bounded_thread_stream_snapshot(
    sequence: u64,
    mut projection: OrchestrationV2ThreadProjection,
    options: SnapshotOptions,
) -> Result<OrchestrationV2ThreadStreamItem> {
    project_thread_projection_for_wire(&mut projection)?;
    let bounded =
        build_bounded_thread_projection(projection, sequence, THREAD_HISTORY_PAGE_POLICY)?;
    Ok(bounded_snapshot_response_fields(bounded, sequence, options))
}
pub fn synchronized() -> OrchestrationV2ThreadStreamItem {
    OrchestrationV2ThreadStreamItem::Synchronized(Box::new(ThreadStreamItemMember1 {
        kind: ThreadStreamItemMember1Kind::Synchronized,
        ids: None,
    }))
}

pub fn coalescible_update_key(item: &OrchestrationV2ThreadStreamItem) -> Option<(String, String)> {
    let OrchestrationV2ThreadStreamItem::Event(item) = item else {
        return None;
    };
    match &item.event {
        OrchestrationV2DomainEvent::MessageUpdated(event)
            if event.payload.role == ConversationMessageRole::Assistant
                && event.payload.streaming =>
        {
            Some((
                "message.updated".into(),
                format!(
                    "{}\0{}\0message.updated\0{}",
                    event.thread_id.0,
                    event
                        .run_id
                        .as_ref()
                        .and_then(Option::as_ref)
                        .map_or("", |id| id.0.as_str()),
                    id_text(&event.payload.id)
                ),
            ))
        }
        OrchestrationV2DomainEvent::TurnItemUpdated(event)
            if super::thread_history_paging::item_status(&event.payload)
                == &OrchestrationV2TurnItemStatus::Running =>
        {
            let kind = match &event.payload {
                OrchestrationV2TurnItem::AssistantMessage(v) if v.streaming => "assistant_message",
                OrchestrationV2TurnItem::Reasoning(v) if v.streaming => "reasoning",
                OrchestrationV2TurnItem::CommandExecution(_) => "command_execution",
                OrchestrationV2TurnItem::FileChange(_) => "file_change",
                OrchestrationV2TurnItem::FileSearch(_) => "file_search",
                OrchestrationV2TurnItem::WebSearch(_) => "web_search",
                OrchestrationV2TurnItem::DynamicTool(_) => "dynamic_tool",
                _ => return None,
            };
            Some((
                "turn-item.updated".into(),
                format!(
                    "{}\0{}\0turn-item.updated\0{kind}\0{}",
                    event.thread_id.0,
                    event
                        .run_id
                        .as_ref()
                        .and_then(Option::as_ref)
                        .map_or("", |id| id.0.as_str()),
                    id_text(super::thread_history_paging::item_id(&event.payload))
                ),
            ))
        }
        _ => None,
    }
}

#[derive(Default)]
pub struct ThreadCoalescingKeys {
    generation: u64,
    families: std::collections::BTreeMap<String, String>,
}
impl ThreadCoalescingKeys {
    pub fn key(&mut self, item: &OrchestrationV2ThreadStreamItem) -> Option<String> {
        let Some((family, key)) = coalescible_update_key(item) else {
            self.flush();
            return None;
        };
        if self.families.get(&family).is_some_and(|old| *old != key) {
            self.generation += 1;
        }
        self.families.insert(family, key.clone());
        Some(format!("{}:{key}", self.generation))
    }
    pub fn flush(&mut self) {
        self.generation += 1;
        self.families.clear();
    }
}
pub fn coalesce_live_updated_events(
    items: Vec<OrchestrationV2ThreadStreamItem>,
) -> Vec<OrchestrationV2ThreadStreamItem> {
    let mut keys = ThreadCoalescingKeys::default();
    let mut latest = std::collections::BTreeMap::new();
    let mut superseded = std::collections::BTreeSet::new();
    for (index, item) in items.iter().enumerate() {
        if let Some(key) = keys.key(item)
            && let Some(previous) = latest.insert(key, index)
        {
            superseded.insert(previous);
        }
    }
    items
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| (!superseded.contains(&index)).then_some(item))
        .collect()
}

fn omit_local_visible_turn_items(projection: &mut OrchestrationV2ThreadProjection) -> bool {
    let local: Vec<_> = projection
        .visible_turn_items
        .iter()
        .filter(|row| {
            row.visibility == ProjectedTurnItemVisibility::Local
                || row.source_thread_id == projection.thread.id
        })
        .map(|row| &row.item)
        .collect();
    // Rust owns value copies; equality and prefix order replace JavaScript object identity.
    if local.is_empty()
        || local
            .iter()
            .enumerate()
            .any(|(index, item)| projection.turn_items.get(index) != Some(*item))
    {
        return false;
    }
    let count = local.len();
    projection.turn_items.drain(..count);
    true
}

pub fn dedupe_snapshot_bodies(projection: &mut OrchestrationV2ThreadProjection) {
    let messages: std::collections::BTreeMap<_, _> = projection
        .messages
        .iter()
        .map(|v| (id_text(&v.id), v.text.as_str()))
        .collect();
    let checkpoints: std::collections::BTreeMap<_, _> = projection
        .checkpoints
        .iter()
        .map(|v| (id_text(&v.id), &v.files))
        .collect();
    let subagents: std::collections::BTreeMap<_, _> = projection
        .subagents
        .iter()
        .map(|v| (id_text(&v.id), (&v.prompt, &v.result)))
        .collect();
    let dedupe = |item: &mut OrchestrationV2TurnItem| {
        macro_rules! message {
            ($v:expr) => {
                if !$v.text.is_empty()
                    && messages.get(&id_text(&$v.message_id)) == Some(&$v.text.as_str())
                {
                    $v.text.clear();
                    $v.text_omitted = Some(LiteralTrue);
                }
            };
        }
        match item {
            OrchestrationV2TurnItem::UserMessage(v) => message!(v),
            OrchestrationV2TurnItem::AssistantMessage(v) => message!(v),
            OrchestrationV2TurnItem::Checkpoint(v) => {
                if !v.files.is_empty()
                    && checkpoints
                        .get(&id_text(&v.checkpoint_id))
                        .is_some_and(|files| **files == v.files)
                {
                    v.files.clear();
                    v.files_omitted = Some(LiteralTrue);
                }
            }
            OrchestrationV2TurnItem::Subagent(v) => {
                if let Some((prompt, result)) = subagents.get(&id_text(&v.subagent_id)) {
                    if !v.prompt.is_empty() && v.prompt == **prompt {
                        v.prompt.clear();
                        v.prompt_omitted = Some(LiteralTrue);
                    }
                    if v.result.is_some() && v.result == **result {
                        v.result = None;
                        v.result_omitted = Some(LiteralTrue);
                    }
                }
            }
            _ => {}
        }
    };
    projection.turn_items.iter_mut().for_each(dedupe);
    projection
        .visible_turn_items
        .iter_mut()
        .filter(|row| {
            row.visibility == ProjectedTurnItemVisibility::Local
                || row.source_thread_id == projection.thread.id
        })
        .for_each(|row| dedupe(&mut row.item));
}

pub fn event_thread_id(
    event: &OrchestrationV2DomainEvent,
) -> &t3_contracts::base_schemas::ThreadId {
    match event {
        OrchestrationV2DomainEvent::ThreadCreated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadArchived(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadUnarchived(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadDeleted(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadSettled(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadUnsettled(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadSnoozed(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadUnsnoozed(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadPinned(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadAutoSettleSet(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadUnpinned(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadPinReordered(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadActiveReordered(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadTagsUpdated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadVisited(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadVisitRecorded(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadMarkedUnread(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadMetadataUpdated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadPullRequestSynced(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadRuntimeModeUpdated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadInteractionModeUpdated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadModelSelectionUpdated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadProviderSwitched(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadGuestShareUpdated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ThreadPullRequestLinkUpdated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::RunCreated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::RunUpdated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::RunBackgroundWorkCancelled(event) => &event.thread_id,
        OrchestrationV2DomainEvent::RunAttemptCreated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::RunAttemptUpdated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::NodeUpdated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::SubagentUpdated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ProviderSessionAttached(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ProviderSessionUpdated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ProviderSessionDetached(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ProviderThreadUpdated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ProviderTurnUpdated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::RuntimeRequestUpdated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::MessageUpdated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::TurnItemUpdated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::PlanUpdated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::CheckpointScopeCreated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::CheckpointCaptured(event) => &event.thread_id,
        OrchestrationV2DomainEvent::CheckpointRollbackRequested(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ContextHandoffUpdated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ContextTransferCreated(event) => &event.thread_id,
        OrchestrationV2DomainEvent::ContextTransferUpdated(event) => &event.thread_id,
    }
}
