use std::collections::BTreeSet;
use t3_contracts::{base_schemas::*, orchestration_v2::*, orchestration_v2_id_table::IdOrIndex};

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum VisibilityId {
    Text(String),
    Index(u64),
}
fn visibility_id<T: AsRef<str>>(id: &IdOrIndex<T>) -> VisibilityId {
    match id {
        IdOrIndex::Id(id) => VisibilityId::Text(id.as_ref().into()),
        IdOrIndex::Index(index) => VisibilityId::Index(*index),
    }
}

pub struct Visibility {
    rolled_back: BTreeSet<VisibilityId>,
    queued_or_cancelled: BTreeSet<VisibilityId>,
    superseded: BTreeSet<(VisibilityId, VisibilityId)>,
    interrupt_runs: BTreeSet<VisibilityId>,
}
impl Visibility {
    pub fn new(p: &OrchestrationV2ThreadProjection) -> Self {
        let mut index = Self {
            rolled_back: BTreeSet::new(),
            queued_or_cancelled: BTreeSet::new(),
            superseded: BTreeSet::new(),
            interrupt_runs: BTreeSet::new(),
        };
        for run in &p.runs {
            let id = visibility_id(&run.id);
            if run.status == OrchestrationV2RunStatus::RolledBack {
                index.rolled_back.insert(id);
            } else {
                index.rolled_back.remove(&id);
            }
            let id = visibility_id(&run.id);
            if matches!(
                run.status,
                OrchestrationV2RunStatus::Queued | OrchestrationV2RunStatus::Cancelled
            ) {
                index.queued_or_cancelled.insert(id);
            } else {
                index.queued_or_cancelled.remove(&id);
            }
        }
        for attempt in &p.attempts {
            if attempt.status == RunAttemptStatus::Superseded {
                index.superseded.insert((
                    visibility_id(&attempt.run_id),
                    visibility_id(&attempt.root_node_id),
                ));
            }
        }
        if !index.superseded.is_empty() {
            for item in &p.turn_items {
                if matches!(item, OrchestrationV2TurnItem::RunInterruptRequest(_))
                    && let Some(run) = run_id(item)
                {
                    index.interrupt_runs.insert(visibility_id(run));
                }
            }
        }
        index
    }

    pub fn visible(&self, item: &OrchestrationV2TurnItem) -> bool {
        let Some(run) = run_id(item) else {
            return true;
        };
        let run = visibility_id(run);
        if self.rolled_back.contains(&run)
            || (queued_input(item) && self.queued_or_cancelled.contains(&run))
        {
            return false;
        }
        if matches!(item, OrchestrationV2TurnItem::RunInterruptResult(_))
            && let Some(node) = node_id(item)
        {
            let paired = self.interrupt_runs.contains(&run);
            return !self.superseded.contains(&(run, visibility_id(node))) || paired;
        }
        true
    }
}
pub fn id(item: &OrchestrationV2TurnItem) -> &IdOrIndex<TurnItemId> {
    match item {
        OrchestrationV2TurnItem::Notification(v) => &v.id,
        OrchestrationV2TurnItem::UserMessage(v) => &v.id,
        OrchestrationV2TurnItem::AssistantMessage(v) => &v.id,
        OrchestrationV2TurnItem::Reasoning(v) => &v.id,
        OrchestrationV2TurnItem::ProposedPlan(v) => &v.id,
        OrchestrationV2TurnItem::TodoList(v) => &v.id,
        OrchestrationV2TurnItem::UserInputRequest(v) => &v.id,
        OrchestrationV2TurnItem::FileChange(v) => &v.id,
        OrchestrationV2TurnItem::CommandExecution(v) => &v.id,
        OrchestrationV2TurnItem::FileSearch(v) => &v.id,
        OrchestrationV2TurnItem::WebSearch(v) => &v.id,
        OrchestrationV2TurnItem::ApprovalRequest(v) => &v.id,
        OrchestrationV2TurnItem::Checkpoint(v) => &v.id,
        OrchestrationV2TurnItem::RunInterruptRequest(v) => &v.id,
        OrchestrationV2TurnItem::RunInterruptResult(v) => &v.id,
        OrchestrationV2TurnItem::SystemNotice(v) => &v.id,
        OrchestrationV2TurnItem::Error(v) => &v.id,
        OrchestrationV2TurnItem::Compaction(v) => &v.id,
        OrchestrationV2TurnItem::Handoff(v) => &v.id,
        OrchestrationV2TurnItem::Fork(v) => &v.id,
        OrchestrationV2TurnItem::ThreadCreated(v) => &v.id,
        OrchestrationV2TurnItem::SecretRequest(v) => &v.id,
        OrchestrationV2TurnItem::LinkRequest(v) => &v.id,
        OrchestrationV2TurnItem::Subagent(v) => &v.id,
        OrchestrationV2TurnItem::DynamicTool(v) => &v.id,
    }
}
pub fn thread_id(item: &OrchestrationV2TurnItem) -> &IdOrIndex<ThreadId> {
    match item {
        OrchestrationV2TurnItem::Notification(v) => &v.thread_id,
        OrchestrationV2TurnItem::UserMessage(v) => &v.thread_id,
        OrchestrationV2TurnItem::AssistantMessage(v) => &v.thread_id,
        OrchestrationV2TurnItem::Reasoning(v) => &v.thread_id,
        OrchestrationV2TurnItem::ProposedPlan(v) => &v.thread_id,
        OrchestrationV2TurnItem::TodoList(v) => &v.thread_id,
        OrchestrationV2TurnItem::UserInputRequest(v) => &v.thread_id,
        OrchestrationV2TurnItem::FileChange(v) => &v.thread_id,
        OrchestrationV2TurnItem::CommandExecution(v) => &v.thread_id,
        OrchestrationV2TurnItem::FileSearch(v) => &v.thread_id,
        OrchestrationV2TurnItem::WebSearch(v) => &v.thread_id,
        OrchestrationV2TurnItem::ApprovalRequest(v) => &v.thread_id,
        OrchestrationV2TurnItem::Checkpoint(v) => &v.thread_id,
        OrchestrationV2TurnItem::RunInterruptRequest(v) => &v.thread_id,
        OrchestrationV2TurnItem::RunInterruptResult(v) => &v.thread_id,
        OrchestrationV2TurnItem::SystemNotice(v) => &v.thread_id,
        OrchestrationV2TurnItem::Error(v) => &v.thread_id,
        OrchestrationV2TurnItem::Compaction(v) => &v.thread_id,
        OrchestrationV2TurnItem::Handoff(v) => &v.thread_id,
        OrchestrationV2TurnItem::Fork(v) => &v.thread_id,
        OrchestrationV2TurnItem::ThreadCreated(v) => &v.thread_id,
        OrchestrationV2TurnItem::SecretRequest(v) => &v.thread_id,
        OrchestrationV2TurnItem::LinkRequest(v) => &v.thread_id,
        OrchestrationV2TurnItem::Subagent(v) => &v.thread_id,
        OrchestrationV2TurnItem::DynamicTool(v) => &v.thread_id,
    }
}
pub fn run_id(item: &OrchestrationV2TurnItem) -> &Option<IdOrIndex<RunId>> {
    match item {
        OrchestrationV2TurnItem::Notification(v) => &v.run_id,
        OrchestrationV2TurnItem::UserMessage(v) => &v.run_id,
        OrchestrationV2TurnItem::AssistantMessage(v) => &v.run_id,
        OrchestrationV2TurnItem::Reasoning(v) => &v.run_id,
        OrchestrationV2TurnItem::ProposedPlan(v) => &v.run_id,
        OrchestrationV2TurnItem::TodoList(v) => &v.run_id,
        OrchestrationV2TurnItem::UserInputRequest(v) => &v.run_id,
        OrchestrationV2TurnItem::FileChange(v) => &v.run_id,
        OrchestrationV2TurnItem::CommandExecution(v) => &v.run_id,
        OrchestrationV2TurnItem::FileSearch(v) => &v.run_id,
        OrchestrationV2TurnItem::WebSearch(v) => &v.run_id,
        OrchestrationV2TurnItem::ApprovalRequest(v) => &v.run_id,
        OrchestrationV2TurnItem::Checkpoint(v) => &v.run_id,
        OrchestrationV2TurnItem::RunInterruptRequest(v) => &v.run_id,
        OrchestrationV2TurnItem::RunInterruptResult(v) => &v.run_id,
        OrchestrationV2TurnItem::SystemNotice(v) => &v.run_id,
        OrchestrationV2TurnItem::Error(v) => &v.run_id,
        OrchestrationV2TurnItem::Compaction(v) => &v.run_id,
        OrchestrationV2TurnItem::Handoff(v) => &v.run_id,
        OrchestrationV2TurnItem::Fork(v) => &v.run_id,
        OrchestrationV2TurnItem::ThreadCreated(v) => &v.run_id,
        OrchestrationV2TurnItem::SecretRequest(v) => &v.run_id,
        OrchestrationV2TurnItem::LinkRequest(v) => &v.run_id,
        OrchestrationV2TurnItem::Subagent(v) => &v.run_id,
        OrchestrationV2TurnItem::DynamicTool(v) => &v.run_id,
    }
}
pub fn node_id(item: &OrchestrationV2TurnItem) -> &Option<IdOrIndex<NodeId>> {
    match item {
        OrchestrationV2TurnItem::Notification(v) => &v.node_id,
        OrchestrationV2TurnItem::UserMessage(v) => &v.node_id,
        OrchestrationV2TurnItem::AssistantMessage(v) => &v.node_id,
        OrchestrationV2TurnItem::Reasoning(v) => &v.node_id,
        OrchestrationV2TurnItem::ProposedPlan(v) => &v.node_id,
        OrchestrationV2TurnItem::TodoList(v) => &v.node_id,
        OrchestrationV2TurnItem::UserInputRequest(v) => &v.node_id,
        OrchestrationV2TurnItem::FileChange(v) => &v.node_id,
        OrchestrationV2TurnItem::CommandExecution(v) => &v.node_id,
        OrchestrationV2TurnItem::FileSearch(v) => &v.node_id,
        OrchestrationV2TurnItem::WebSearch(v) => &v.node_id,
        OrchestrationV2TurnItem::ApprovalRequest(v) => &v.node_id,
        OrchestrationV2TurnItem::Checkpoint(v) => &v.node_id,
        OrchestrationV2TurnItem::RunInterruptRequest(v) => &v.node_id,
        OrchestrationV2TurnItem::RunInterruptResult(v) => &v.node_id,
        OrchestrationV2TurnItem::SystemNotice(v) => &v.node_id,
        OrchestrationV2TurnItem::Error(v) => &v.node_id,
        OrchestrationV2TurnItem::Compaction(v) => &v.node_id,
        OrchestrationV2TurnItem::Handoff(v) => &v.node_id,
        OrchestrationV2TurnItem::Fork(v) => &v.node_id,
        OrchestrationV2TurnItem::ThreadCreated(v) => &v.node_id,
        OrchestrationV2TurnItem::SecretRequest(v) => &v.node_id,
        OrchestrationV2TurnItem::LinkRequest(v) => &v.node_id,
        OrchestrationV2TurnItem::Subagent(v) => &v.node_id,
        OrchestrationV2TurnItem::DynamicTool(v) => &v.node_id,
    }
}
pub fn ordinal(item: &OrchestrationV2TurnItem) -> u64 {
    match item {
        OrchestrationV2TurnItem::Notification(v) => v.ordinal,
        OrchestrationV2TurnItem::UserMessage(v) => v.ordinal,
        OrchestrationV2TurnItem::AssistantMessage(v) => v.ordinal,
        OrchestrationV2TurnItem::Reasoning(v) => v.ordinal,
        OrchestrationV2TurnItem::ProposedPlan(v) => v.ordinal,
        OrchestrationV2TurnItem::TodoList(v) => v.ordinal,
        OrchestrationV2TurnItem::UserInputRequest(v) => v.ordinal,
        OrchestrationV2TurnItem::FileChange(v) => v.ordinal,
        OrchestrationV2TurnItem::CommandExecution(v) => v.ordinal,
        OrchestrationV2TurnItem::FileSearch(v) => v.ordinal,
        OrchestrationV2TurnItem::WebSearch(v) => v.ordinal,
        OrchestrationV2TurnItem::ApprovalRequest(v) => v.ordinal,
        OrchestrationV2TurnItem::Checkpoint(v) => v.ordinal,
        OrchestrationV2TurnItem::RunInterruptRequest(v) => v.ordinal,
        OrchestrationV2TurnItem::RunInterruptResult(v) => v.ordinal,
        OrchestrationV2TurnItem::SystemNotice(v) => v.ordinal,
        OrchestrationV2TurnItem::Error(v) => v.ordinal,
        OrchestrationV2TurnItem::Compaction(v) => v.ordinal,
        OrchestrationV2TurnItem::Handoff(v) => v.ordinal,
        OrchestrationV2TurnItem::Fork(v) => v.ordinal,
        OrchestrationV2TurnItem::ThreadCreated(v) => v.ordinal,
        OrchestrationV2TurnItem::SecretRequest(v) => v.ordinal,
        OrchestrationV2TurnItem::LinkRequest(v) => v.ordinal,
        OrchestrationV2TurnItem::Subagent(v) => v.ordinal,
        OrchestrationV2TurnItem::DynamicTool(v) => v.ordinal,
    }
}
pub fn deferred(item: &OrchestrationV2TurnItem) -> &Option<OrchestrationV2DeferredWork> {
    match item {
        OrchestrationV2TurnItem::Notification(v) => &v.deferred_work,
        OrchestrationV2TurnItem::UserMessage(v) => &v.deferred_work,
        OrchestrationV2TurnItem::AssistantMessage(v) => &v.deferred_work,
        OrchestrationV2TurnItem::Reasoning(v) => &v.deferred_work,
        OrchestrationV2TurnItem::ProposedPlan(v) => &v.deferred_work,
        OrchestrationV2TurnItem::TodoList(v) => &v.deferred_work,
        OrchestrationV2TurnItem::UserInputRequest(v) => &v.deferred_work,
        OrchestrationV2TurnItem::FileChange(v) => &v.deferred_work,
        OrchestrationV2TurnItem::CommandExecution(v) => &v.deferred_work,
        OrchestrationV2TurnItem::FileSearch(v) => &v.deferred_work,
        OrchestrationV2TurnItem::WebSearch(v) => &v.deferred_work,
        OrchestrationV2TurnItem::ApprovalRequest(v) => &v.deferred_work,
        OrchestrationV2TurnItem::Checkpoint(v) => &v.deferred_work,
        OrchestrationV2TurnItem::RunInterruptRequest(v) => &v.deferred_work,
        OrchestrationV2TurnItem::RunInterruptResult(v) => &v.deferred_work,
        OrchestrationV2TurnItem::SystemNotice(v) => &v.deferred_work,
        OrchestrationV2TurnItem::Error(v) => &v.deferred_work,
        OrchestrationV2TurnItem::Compaction(v) => &v.deferred_work,
        OrchestrationV2TurnItem::Handoff(v) => &v.deferred_work,
        OrchestrationV2TurnItem::Fork(v) => &v.deferred_work,
        OrchestrationV2TurnItem::ThreadCreated(v) => &v.deferred_work,
        OrchestrationV2TurnItem::SecretRequest(v) => &v.deferred_work,
        OrchestrationV2TurnItem::LinkRequest(v) => &v.deferred_work,
        OrchestrationV2TurnItem::Subagent(v) => &v.deferred_work,
        OrchestrationV2TurnItem::DynamicTool(v) => &v.deferred_work,
    }
}
pub fn set_deferred(
    item: &mut OrchestrationV2TurnItem,
    deferred: Option<OrchestrationV2DeferredWork>,
) {
    match item {
        OrchestrationV2TurnItem::Notification(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::UserMessage(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::AssistantMessage(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::Reasoning(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::ProposedPlan(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::TodoList(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::UserInputRequest(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::FileChange(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::CommandExecution(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::FileSearch(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::WebSearch(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::ApprovalRequest(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::Checkpoint(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::RunInterruptRequest(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::RunInterruptResult(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::SystemNotice(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::Error(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::Compaction(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::Handoff(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::Fork(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::ThreadCreated(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::SecretRequest(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::LinkRequest(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::Subagent(v) => v.deferred_work = deferred,
        OrchestrationV2TurnItem::DynamicTool(v) => v.deferred_work = deferred,
    }
}
pub fn queued_input(item: &OrchestrationV2TurnItem) -> bool {
    matches!(item,OrchestrationV2TurnItem::UserMessage(v) if v.input_intent==OrchestrationV2UserMessageInputIntent::QueuedTurn)
}
pub fn status(item: &OrchestrationV2TurnItem) -> &OrchestrationV2TurnItemStatus {
    macro_rules! status { ($($variant:ident),*) => { match item { $(OrchestrationV2TurnItem::$variant(v) => &v.status,)* } }; }
    status!(
        Notification,
        UserMessage,
        AssistantMessage,
        Reasoning,
        ProposedPlan,
        TodoList,
        UserInputRequest,
        FileChange,
        CommandExecution,
        FileSearch,
        WebSearch,
        ApprovalRequest,
        Checkpoint,
        RunInterruptRequest,
        RunInterruptResult,
        SystemNotice,
        Error,
        Compaction,
        Handoff,
        Fork,
        ThreadCreated,
        SecretRequest,
        LinkRequest,
        Subagent,
        DynamicTool
    )
}

pub fn active(item: &OrchestrationV2TurnItem) -> bool {
    matches!(
        status(item),
        OrchestrationV2TurnItemStatus::Pending
            | OrchestrationV2TurnItemStatus::Running
            | OrchestrationV2TurnItemStatus::Waiting
    )
}

pub fn pending_background(p: &OrchestrationV2ThreadProjection) -> Vec<&OrchestrationV2TurnItem> {
    p.turn_items.iter().filter(|item| {
        matches!(item, OrchestrationV2TurnItem::CommandExecution(_) | OrchestrationV2TurnItem::DynamicTool(_) | OrchestrationV2TurnItem::Subagent(_))
            && active(item)
            && !matches!(item, OrchestrationV2TurnItem::DynamicTool(v) if v.input.get("persistent").and_then(serde_json::Value::as_bool) == Some(true))
            && !p.runs.iter().any(|run| Some(&run.id) == run_id(item).as_ref() && run.status == OrchestrationV2RunStatus::RolledBack)
    }).collect()
}

pub fn settle(
    p: &mut OrchestrationV2ThreadProjection,
    terminal: &[OrchestrationV2TurnItem],
) -> bool {
    let mut changed = false;
    let mut update = |item: &mut OrchestrationV2TurnItem| {
        if active(item)
            && let Some(server) = terminal.iter().find(|server| {
                id(server) == id(item) && thread_id(server) == thread_id(item) && !active(server)
            })
        {
            *item = server.clone();
            changed = true;
        }
    };
    for item in &mut p.turn_items {
        update(item);
    }
    for row in &mut p.visible_turn_items {
        if row.source_thread_id == p.thread.id {
            update(&mut row.item);
        }
    }
    changed
}
pub fn visible(p: &OrchestrationV2ThreadProjection, item: &OrchestrationV2TurnItem) -> bool {
    let run = run_id(item);
    if let Some(run) = run
        && p.runs.iter().any(|r| {
            &r.id == run
                && (r.status == OrchestrationV2RunStatus::RolledBack
                    || (queued_input(item)
                        && matches!(
                            r.status,
                            OrchestrationV2RunStatus::Queued | OrchestrationV2RunStatus::Cancelled
                        )))
        })
    {
        return false;
    }
    if matches!(item, OrchestrationV2TurnItem::RunInterruptResult(_))
        && let (Some(run), Some(node)) = (run, node_id(item))
    {
        return !p.attempts.iter().any(|a| {
            &a.run_id == run && &a.root_node_id == node && a.status == RunAttemptStatus::Superseded
        }) || p.turn_items.iter().any(|i| {
            matches!(i, OrchestrationV2TurnItem::RunInterruptRequest(_))
                && run_id(i).as_ref() == Some(run)
        });
    }
    true
}
pub fn renumber(rows: &mut [OrchestrationV2ProjectedTurnItem]) {
    for (position, row) in rows.iter_mut().enumerate() {
        row.position = position as u64;
    }
}
pub fn restore_subagent(
    value: &mut OrchestrationV2Subagent,
    previous: Option<&OrchestrationV2Subagent>,
) {
    let Some(previous) = previous else {
        return;
    };
    if value.prompt_omitted.is_some() && previous.prompt_omitted.is_none() {
        value.prompt = previous.prompt.clone();
        value.prompt_omitted = None;
    }
    if value.result_omitted.is_some() && previous.result_omitted.is_none() {
        value.result = previous.result.clone();
        value.result_omitted = None;
    }
}
pub fn restore_body(item: &mut OrchestrationV2TurnItem, p: &OrchestrationV2ThreadProjection) {
    match item {
        OrchestrationV2TurnItem::UserMessage(v) => {
            if v.text_omitted.is_some()
                && let Some(m) = p.messages.iter().rev().find(|m| m.id == v.message_id)
            {
                v.text = m.text.clone();
                v.text_omitted = None;
            }
        }
        OrchestrationV2TurnItem::AssistantMessage(v) => {
            if v.text_omitted.is_some()
                && let Some(m) = p.messages.iter().rev().find(|m| m.id == v.message_id)
            {
                v.text = m.text.clone();
                v.text_omitted = None;
            }
        }
        OrchestrationV2TurnItem::Checkpoint(v) => {
            if v.files_omitted.is_some()
                && let Some(c) = p.checkpoints.iter().rev().find(|c| c.id == v.checkpoint_id)
            {
                v.files = c.files.clone();
                v.files_omitted = None;
            }
        }
        OrchestrationV2TurnItem::Subagent(v) => {
            if let Some(a) = p.subagents.iter().rev().find(|a| a.id == v.subagent_id) {
                if v.prompt_omitted.is_some() && a.prompt_omitted.is_none() {
                    v.prompt = a.prompt.clone();
                    v.prompt_omitted = None;
                }
                if v.result_omitted.is_some() && a.result_omitted.is_none() {
                    v.result = a.result.clone();
                    v.result_omitted = None;
                }
            }
        }
        _ => {}
    }
}
pub fn restore_snapshot(p: &mut OrchestrationV2ThreadProjection) {
    // Move rows out so body lookups borrow only the source record collections.
    let mut local = std::mem::take(&mut p.turn_items);
    for item in &mut local {
        restore_body(item, p);
    }
    let mut rows = std::mem::take(&mut p.visible_turn_items);
    for row in &mut rows {
        if row.visibility == ProjectedTurnItemVisibility::Local
            || row.source_thread_id == p.thread.id
        {
            restore_body(&mut row.item, p);
        }
    }
    p.turn_items = local;
    p.visible_turn_items = rows;
}
