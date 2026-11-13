use crate::state::sharing::{SharedRows, SharedThreadProjection};
use std::rc::Rc;
use t3_contracts::{
    base_schemas::TurnItemId, orchestration_v2::*, orchestration_v2_id_table::IdOrIndex,
};
macro_rules! same_fields {($a:expr,$b:expr,$($field:ident),+)=>{true $(&& $a.$field == $b.$field)+};}
macro_rules! same_shared_fields {($a:expr,$b:expr,$($field:ident),+)=>{true $(&& (Rc::ptr_eq(&$a.$field,&$b.$field) || $a.$field == $b.$field))+};}
macro_rules! same_item_base {
    ($a:expr,$b:expr) => {
        same_fields!(
            $a,
            $b,
            deferred_work,
            tool_non_execution_kind,
            tool_surface,
            tool_icon,
            tool_source,
            id,
            thread_id,
            run_id,
            node_id,
            provider_thread_id,
            provider_turn_id,
            native_item_ref,
            parent_item_id,
            ordinal,
            status,
            title,
            started_at,
            completed_at,
            streaming
        )
    };
}
pub fn same_turn_item_structure(a: &OrchestrationV2TurnItem, b: &OrchestrationV2TurnItem) -> bool {
    if std::ptr::eq(a, b) {
        return true;
    }
    match (a, b) {
        (
            OrchestrationV2TurnItem::AssistantMessage(a),
            OrchestrationV2TurnItem::AssistantMessage(b),
        ) => {
            same_item_base!(a, b)
                && same_fields!(a, b, phase, message_id, text_omitted, attachments)
        }
        (OrchestrationV2TurnItem::Reasoning(a), OrchestrationV2TurnItem::Reasoning(b)) => {
            same_item_base!(a, b)
        }
        _ => a == b,
    }
}
pub fn same_visible_turn_item_structure(
    a: &OrchestrationV2ProjectedTurnItem,
    b: &OrchestrationV2ProjectedTurnItem,
) -> bool {
    same_fields!(a, b, position, visibility, source_thread_id, source_item_id)
        && same_turn_item_structure(&a.item, &b.item)
}
fn same_rows<T>(
    a: &SharedRows<T>,
    b: &SharedRows<T>,
    replaced: Option<usize>,
    same: impl Fn(&T, &T) -> bool,
) -> bool {
    if Rc::ptr_eq(a, b) {
        return true;
    }
    if a.len() != b.len() {
        return false;
    }
    if let Some(index) = replaced {
        return a
            .get(index)
            .zip(b.get(index))
            .is_some_and(|(a, b)| same(a, b));
    }
    a.iter()
        .zip(b.iter())
        .all(|(a, b)| Rc::ptr_eq(a, b) || same(a, b))
}
#[derive(Debug, Clone, Copy)]
pub struct DetailReplacement {
    pub item: usize,
    pub visible: usize,
}
pub fn same_thread_structure(
    a: &SharedThreadProjection,
    b: &SharedThreadProjection,
    replaced: Option<DetailReplacement>,
) -> bool {
    if std::ptr::eq(a, b) {
        return true;
    }
    let mut thread = (*a.thread).clone();
    thread.updated_at = b.thread.updated_at.clone();
    thread.last_visited_at = b.thread.last_visited_at.clone();
    thread == *b.thread
        && same_shared_fields!(
            a,
            b,
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
            checkpoint_scopes,
            checkpoints,
            context_handoffs,
            context_transfers
        )
        && same_rows(
            &a.turn_items,
            &b.turn_items,
            replaced.map(|r| r.item),
            same_turn_item_structure,
        )
        && same_rows(
            &a.visible_turn_items,
            &b.visible_turn_items,
            replaced.map(|r| r.visible),
            same_visible_turn_item_structure,
        )
}
pub fn is_complete_turn_item_row(row: &OrchestrationV2ProjectedTurnItem) -> bool {
    !matches!(
        row.item,
        OrchestrationV2TurnItem::AssistantMessage(_) | OrchestrationV2TurnItem::Reasoning(_)
    )
}
pub fn derive_workspace_mutation_id(
    rows: &[Rc<OrchestrationV2ProjectedTurnItem>],
    checkpoint: Option<&str>,
) -> Result<Option<String>, serde_json::Error> {
    let id = rows
        .iter()
        .rev()
        .find(|row| {
            matches!(
                row.item,
                OrchestrationV2TurnItem::CommandExecution(_)
                    | OrchestrationV2TurnItem::FileChange(_)
                    | OrchestrationV2TurnItem::DynamicTool(_)
                    | OrchestrationV2TurnItem::Subagent(_)
            ) && !super::execution::work_active(item_field!(&row.item, status))
        })
        .map(|row| item_field!(&row.item, id));
    if id.is_none() && checkpoint.is_none() {
        Ok(None)
    } else {
        serde_json::to_string(&(id, checkpoint)).map(Some)
    }
}

#[derive(Default)]
pub struct ThreadDetailViews {
    current: Option<Rc<SharedThreadProjection>>,
    quiet: Option<Rc<SharedThreadProjection>>,
    timeline: SharedRows<OrchestrationV2ProjectedTurnItem>,
    questions: crate::thread_views::requests::QuestionHistoryProjector,
    pub visible: SharedRows<OrchestrationV2ProjectedTurnItem>,
    quiet_questions: crate::thread_views::requests::QuestionHistoryProjector,
    pub quiet_visible: SharedRows<OrchestrationV2ProjectedTurnItem>,
    notice: Option<super::execution_presentation::UsageLimitNoticeRows>,
    pub queue: Option<Rc<QueueWorkflowView>>,
    pub checkpoint_summaries: Rc<[CheckpointSummaryView]>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct QueuedRunView {
    pub run: Rc<OrchestrationV2Run>,
    pub message: Option<Rc<OrchestrationV2ConversationMessage>>,
    pub text: String,
}
#[derive(Debug, Clone, PartialEq)]
pub struct QueueWorkflowView {
    pub active_run: Option<Rc<OrchestrationV2Run>>,
    pub queued_runs: Vec<QueuedRunView>,
    pub is_held: bool,
    pub can_promote_to_steer: bool,
    pub promote_skips_compaction: bool,
    pub can_send_now: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub struct CheckpointSummaryView {
    pub checkpoint: Rc<OrchestrationV2Checkpoint>,
    pub assistant_message_id: Option<IdOrIndex<t3_contracts::base_schemas::MessageId>>,
}
fn queue_view(p: &SharedThreadProjection) -> QueueWorkflowView {
    let source = p.source();
    let view = super::workflows::derive_thread_queue_workflow_state(&source);
    QueueWorkflowView {
        active_run: view
            .active_run
            .and_then(|r| p.runs.iter().find(|row| row.id == r.id))
            .cloned(),
        queued_runs: view
            .queued_runs
            .into_iter()
            .filter_map(|q| {
                Some(QueuedRunView {
                    run: p.runs.iter().find(|r| r.id == q.run.id)?.clone(),
                    message: q
                        .message
                        .and_then(|m| p.messages.iter().find(|row| row.id == m.id))
                        .cloned(),
                    text: q.text.into(),
                })
            })
            .collect(),
        is_held: view.is_held,
        can_promote_to_steer: view.can_promote_to_steer,
        promote_skips_compaction: view.promote_skips_compaction,
        can_send_now: view.can_send_now,
    }
}
impl ThreadDetailViews {
    pub fn update(
        &mut self,
        next: Option<Rc<SharedThreadProjection>>,
        replaced: Option<DetailReplacement>,
        parse: &impl Fn(&t3_contracts::wire::WireDateTime) -> i64,
    ) {
        let Some(next) = next else {
            *self = Self::default();
            return;
        };
        if !self
            .current
            .as_ref()
            .is_some_and(|old| same_thread_structure(old, &next, replaced))
        {
            self.quiet = Some(next.clone());
            self.quiet_visible = self.quiet_questions.project(
                next.visible_turn_items.clone(),
                next.runtime_requests.clone(),
                replaced.map(|r| r.visible),
            );
            self.notice = super::execution_presentation::derive_usage_limit_notice_rows(
                &next.source(),
                parse,
            );
        }
        let mut rows = self.questions.project(
            next.visible_turn_items.clone(),
            next.runtime_requests.clone(),
            replaced.map(|r| r.visible),
        );
        if let Some(notice) = &self.notice {
            let hidden = |row: &OrchestrationV2ProjectedTurnItem| {
                notice.item_ids.contains(item_field!(&row.item, id))
                    || matches!(&row.item,OrchestrationV2TurnItem::AssistantMessage(i) if Some(&i.id)==notice.provider_notice_item_id.as_ref() && super::execution_presentation::is_provider_limit_notice(&i.text))
            };
            if rows.iter().any(|row| hidden(row)) {
                rows = rows
                    .iter()
                    .filter(|row| !hidden(row))
                    .cloned()
                    .collect::<Vec<_>>()
                    .into();
            }
        }
        let replaced = if rows.len() == next.visible_turn_items.len() {
            replaced
        } else {
            None
        };
        let same = same_rows(
            &self.timeline,
            &rows,
            replaced.map(|r| r.visible),
            |a, b| {
                matches!((&a.item,&b.item),(OrchestrationV2TurnItem::AssistantMessage(a),OrchestrationV2TurnItem::AssistantMessage(b)) if a.streaming && b.streaming)
                    && same_visible_turn_item_structure(a, b)
            },
        );
        if !same {
            self.timeline = rows.clone();
        }
        self.visible = rows;
        if self.current.as_ref().is_none_or(|old| {
            !same_shared_fields!(
                old,
                next,
                thread,
                runs,
                messages,
                provider_threads,
                provider_sessions,
                provider_turns
            )
        }) {
            let queue = queue_view(&next);
            if self.queue.as_deref() != Some(&queue) {
                self.queue = Some(Rc::new(queue));
            }
        }
        if self
            .current
            .as_ref()
            .is_none_or(|old| !same_shared_fields!(old, next, checkpoints, messages))
        {
            let source = next.source();
            let summaries: Vec<_> = super::checkpoints::derive_thread_checkpoint_summaries(&source)
                .into_iter()
                .filter_map(|row| {
                    Some(CheckpointSummaryView {
                        checkpoint: next
                            .checkpoints
                            .iter()
                            .find(|c| c.id == row.checkpoint.id)?
                            .clone(),
                        assistant_message_id: row.assistant_message.map(|m| m.id.clone()),
                    })
                })
                .collect();
            if *self.checkpoint_summaries != summaries {
                self.checkpoint_summaries = summaries.into();
            }
        }
        self.current = Some(next);
    }
    /// A retained structure view; streamed text is read through `assistant_message_text`.
    pub fn quiet(&self) -> Option<Rc<SharedThreadProjection>> {
        self.quiet.clone()
    }
    pub fn timeline(&self) -> SharedRows<OrchestrationV2ProjectedTurnItem> {
        self.timeline.clone()
    }
    pub fn assistant_message_text(&self, id: &IdOrIndex<TurnItemId>) -> Option<&str> {
        self.visible.iter().find_map(|r| match &r.item {
            OrchestrationV2TurnItem::AssistantMessage(i) if &i.id == id => Some(i.text.as_str()),
            _ => None,
        })
    }
}
