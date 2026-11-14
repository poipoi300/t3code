use super::*;
use crate::state::items;
use serde_json::json;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEntry {
    pub id: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<Rc<OrchestrationV2RunAttempt>>,
    #[serde(flatten)]
    pub content: TimelineContent,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TimelineContent {
    Message {
        message: Rc<ChatMessage>,
        #[serde(rename = "projectedItem", skip_serializing_if = "Option::is_none")]
        projected_item: Option<Rc<OrchestrationV2ProjectedTurnItem>>,
    },
    ProposedPlan {
        #[serde(rename = "proposedPlan")]
        proposed_plan: Value,
    },
    HtmlRender {
        #[serde(rename = "runId")]
        run_id: Option<IdOrIndex<RunId>>,
        #[serde(rename = "htmlRender")]
        html_render: Value,
    },
    /// What the agent showed the user mid-turn with `send_to_user`, in its own words.
    SentMessage {
        #[serde(rename = "runId")]
        run_id: Option<IdOrIndex<RunId>>,
        text: String,
    },
    McpApp {
        #[serde(rename = "runId")]
        run_id: Option<IdOrIndex<RunId>>,
        #[serde(rename = "sourceThreadId")]
        source_thread_id: IdOrIndex<ThreadId>,
        #[serde(rename = "itemId")]
        item_id: IdOrIndex<TurnItemId>,
        revision: String,
        #[serde(rename = "mcpApp")]
        mcp_app: Value,
    },
    Work {
        entry: Box<WorkLogEntry>,
    },
    Event {
        #[serde(rename = "projectedItem")]
        projected_item: Rc<OrchestrationV2ProjectedTurnItem>,
    },
}
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TimelineEntriesInput {
    pub visible_turn_items: Vec<Rc<OrchestrationV2ProjectedTurnItem>>,
    pub optimistic_messages: Vec<Rc<ChatMessage>>,
    pub anchored_messages: Vec<Rc<ChatMessage>>,
    pub attachment_url_by_id: BTreeMap<String, String>,
    pub attempts: Vec<Rc<OrchestrationV2RunAttempt>>,
    pub nodes: Vec<Rc<OrchestrationV2ExecutionNode>>,
    pub plans: Vec<OrchestrationV2PlanArtifact>,
}
#[derive(Debug, Clone)]
pub struct TimelineEntriesProjection {
    pub input: TimelineEntriesInput,
    pub entries: Rc<Vec<Rc<TimelineEntry>>>,
}
fn resolve_attempt(
    input: &TimelineEntriesInput,
    item: &OrchestrationV2TurnItem,
) -> Option<Rc<OrchestrationV2RunAttempt>> {
    let run = items::run_id(item).as_ref()?;
    let mut node_id = items::node_id(item).clone();
    let mut visited = BTreeSet::new();
    while let Some(id) = node_id.take() {
        if !visited.insert(id_key(&id)) {
            break;
        }
        let by_root =
            |id: &IdOrIndex<NodeId>| input.attempts.iter().rev().find(|a| a.root_node_id == *id);
        if let Some(attempt) = by_root(&id).filter(|a| a.run_id == *run) {
            return Some(attempt.clone());
        }
        let node = input.nodes.iter().rev().find(|n| n.id == id)?;
        if let Some(attempt) = by_root(&node.root_node_id).filter(|a| a.run_id == *run) {
            return Some(attempt.clone());
        }
        node_id.clone_from(&node.parent_node_id);
    }
    None
}
pub fn derive_timeline_entries_from_visible_turn_items(
    input: &TimelineEntriesInput,
) -> Vec<Rc<TimelineEntry>> {
    let mut retained = BTreeSet::new();
    let folded: BTreeMap<_, _> = input
        .visible_turn_items
        .iter()
        .filter_map(|row| {
            if let OrchestrationV2TurnItem::UserInputRequest(item) = &row.item {
                let answer = item_value(&row.item);
                answer
                    .get("questionAnswer")
                    .and_then(|a| a["requestId"].as_str())
                    .map(|id| (format!("async-answer:{id}"), item.run_id.clone()))
            } else {
                None
            }
        })
        .collect();
    let mut entries = Vec::new();
    for row in &input.visible_turn_items {
        let item = &row.item;
        match item {
            OrchestrationV2TurnItem::CommandExecution(v)
                if v.input == "Preparing workspace" && v.deferred_work.is_none() =>
            {
                continue;
            }
            OrchestrationV2TurnItem::Error(v)
                if v.status == OrchestrationV2TurnItemStatus::Cancelled
                    && item_value(item)["failure"]["code"] == "workspace_preparation_failed"
                    && v.deferred_work.is_none() =>
            {
                continue;
            }
            OrchestrationV2TurnItem::Checkpoint(_) => continue,
            OrchestrationV2TurnItem::UserMessage(v)
                if v.input_intent == OrchestrationV2UserMessageInputIntent::QueuedTurn
                    && v.status == OrchestrationV2TurnItemStatus::Pending =>
            {
                continue;
            }
            OrchestrationV2TurnItem::UserMessage(v)
                if folded.get(&id_key(&v.message_id)) == Some(&v.run_id) =>
            {
                continue;
            }
            _ => {}
        }
        let created_at = projected_created_at(row);
        let mut id = id_key(items::id(item));
        let attempt = resolve_attempt(input, item);
        let mut sent_message = match item {
            OrchestrationV2TurnItem::DynamicTool(v)
                if v.status == OrchestrationV2TurnItemStatus::Completed
                    && html_render_from_tool_item(v).is_none() =>
            {
                sent_message_from_tool_item(v)
            }
            _ => None,
        };
        let content = match item {
            OrchestrationV2TurnItem::UserMessage(_)
            | OrchestrationV2TurnItem::AssistantMessage(_) => {
                let value = item_value(item);
                let user = matches!(item, OrchestrationV2TurnItem::UserMessage(_));
                let (message_id, text, streaming, updated, attachments, intent) = match item {
                    OrchestrationV2TurnItem::UserMessage(v) => (
                        v.message_id.clone(),
                        v.text.clone(),
                        false,
                        v.updated_at.0.clone(),
                        v.attachments.clone(),
                        Some(v.input_intent.clone()),
                    ),
                    OrchestrationV2TurnItem::AssistantMessage(v) => (
                        v.message_id.clone(),
                        v.text.clone(),
                        v.streaming,
                        v.updated_at.0.clone(),
                        v.attachments.clone().flatten().unwrap_or_default(),
                        None,
                    ),
                    _ => unreachable!(),
                };
                let mut metadata = BTreeMap::new();
                if user {
                    for key in [
                        "context",
                        "createdBy",
                        "creationSource",
                        "senderThreadId",
                        "linkedSender",
                        "idleCompaction",
                        "scheduledTaskId",
                    ] {
                        if let Some(v) = value.get(key)
                            && (key != "context" || !v.is_null())
                        {
                            metadata.insert(key.into(), v.clone());
                        }
                    }
                }
                id = id_key(&message_id);
                retained.insert(id.clone());
                let attachments = (!attachments.is_empty()).then(|| {
                    Rc::new(
                        attachments
                            .into_iter()
                            .map(|attachment| {
                                let value = wire_value(&attachment);
                                let preview_url = value["id"]
                                    .as_str()
                                    .and_then(|id| input.attachment_url_by_id.get(id))
                                    .filter(|s| !s.is_empty())
                                    .cloned();
                                MessageAttachment {
                                    attachment,
                                    preview_url,
                                }
                            })
                            .collect(),
                    )
                });
                TimelineContent::Message {
                    message: Rc::new(ChatMessage {
                        id: message_id,
                        role: if user { "user" } else { "assistant" }.into(),
                        text,
                        run_id: items::run_id(item).clone(),
                        streaming,
                        created_at: created_at.clone(),
                        updated_at: updated,
                        attachments,
                        input_intent: intent,
                        metadata,
                    }),
                    projected_item: Some(row.clone()),
                }
            }
            OrchestrationV2TurnItem::ProposedPlan(v) => {
                let status = input
                    .plans
                    .iter()
                    .rev()
                    .find_map(|p| match p {
                        OrchestrationV2PlanArtifact::ProposedPlan(p) if p.id == v.plan_id => {
                            Some(wire_value(&p.status))
                        }
                        _ => None,
                    })
                    .unwrap_or(json!("active"));
                TimelineContent::ProposedPlan {
                    proposed_plan: json!({"id": v.plan_id, "runId": v.run_id, "planMarkdown": v.markdown, "status": status, "createdAt": created_at, "updatedAt": v.updated_at.0}),
                }
            }
            OrchestrationV2TurnItem::DynamicTool(v)
                if v.status == OrchestrationV2TurnItemStatus::Completed
                    && html_render_from_tool_item(v).is_some() =>
            {
                TimelineContent::HtmlRender {
                    run_id: v.run_id.clone(),
                    html_render: html_render_from_tool_item(v).unwrap_or(Value::Null),
                }
            }
            OrchestrationV2TurnItem::DynamicTool(v) if sent_message.is_some() => {
                TimelineContent::SentMessage {
                    run_id: v.run_id.clone(),
                    text: sent_message.take().unwrap_or_default(),
                }
            }
            OrchestrationV2TurnItem::Fork(_)
            | OrchestrationV2TurnItem::Handoff(_)
            | OrchestrationV2TurnItem::RunInterruptRequest(_)
            | OrchestrationV2TurnItem::RunInterruptResult(_)
            | OrchestrationV2TurnItem::SecretRequest(_)
            | OrchestrationV2TurnItem::LinkRequest(_)
            | OrchestrationV2TurnItem::Subagent(_) => TimelineContent::Event {
                projected_item: row.clone(),
            },
            _ => TimelineContent::Work {
                entry: Box::new(projected_work_entry(row.clone())),
            },
        };
        let html = matches!(
            content,
            TimelineContent::HtmlRender { .. } | TimelineContent::SentMessage { .. }
        );
        entries.push(Rc::new(TimelineEntry {
            id,
            created_at: created_at.clone(),
            attempt: attempt.clone(),
            content,
        }));
        if !html
            && let OrchestrationV2TurnItem::DynamicTool(v) = item
            && let Some(mcp_app) = mcp_app_from_tool_item(v)
        {
            entries.push(Rc::new(TimelineEntry {
                id: format!("mcp-app:{}", id_key(&v.id)),
                created_at,
                attempt,
                content: TimelineContent::McpApp {
                    run_id: v.run_id.clone(),
                    source_thread_id: row.source_thread_id.clone(),
                    item_id: row.source_item_id.clone(),
                    revision: if matches!(
                        v.status,
                        OrchestrationV2TurnItemStatus::Idle
                            | OrchestrationV2TurnItemStatus::Pending
                            | OrchestrationV2TurnItemStatus::Running
                            | OrchestrationV2TurnItemStatus::Waiting
                    ) {
                        "live".into()
                    } else {
                        v.updated_at.0.clone()
                    },
                    mcp_app,
                },
            }));
        }
    }
    retained.extend(folded.into_keys());
    for message in &input.anchored_messages {
        if !retained.insert(id_key(&message.id)) {
            continue;
        }
        let entry = Rc::new(TimelineEntry {
            id: id_key(&message.id),
            created_at: message.created_at.clone(),
            attempt: None,
            content: TimelineContent::Message {
                message: message.clone(),
                projected_item: None,
            },
        });
        let index = entries
            .iter()
            .position(|e| e.created_at > message.created_at)
            .unwrap_or(entries.len());
        entries.insert(index, entry);
    }
    for message in &input.optimistic_messages {
        if message.input_intent == Some(OrchestrationV2UserMessageInputIntent::QueuedTurn)
            || !retained.insert(id_key(&message.id))
        {
            continue;
        }
        entries.push(Rc::new(TimelineEntry {
            id: id_key(&message.id),
            created_at: message.created_at.clone(),
            attempt: None,
            content: TimelineContent::Message {
                message: message.clone(),
                projected_item: None,
            },
        }));
    }
    entries
}
pub fn timeline_entry_is_persistent_resource_card(entry: &TimelineEntry) -> bool {
    matches!(&entry.content, TimelineContent::Event {projected_item} if matches!(projected_item.item, OrchestrationV2TurnItem::Fork(_) | OrchestrationV2TurnItem::ThreadCreated(_) | OrchestrationV2TurnItem::SecretRequest(_) | OrchestrationV2TurnItem::LinkRequest(_)))
}
pub fn is_streaming_turn_item_text_update(
    previous: &OrchestrationV2ProjectedTurnItem,
    next: &OrchestrationV2ProjectedTurnItem,
) -> bool {
    if previous.position != next.position
        || previous.visibility != next.visibility
        || previous.source_thread_id != next.source_thread_id
        || previous.source_item_id != next.source_item_id
        || projected_created_at(previous) != projected_created_at(next)
    {
        return false;
    }
    fn metadata(item: &OrchestrationV2TurnItem) -> Option<Value> {
        let field = match item {
            OrchestrationV2TurnItem::ProposedPlan(v) if v.streaming => "markdown",
            OrchestrationV2TurnItem::AssistantMessage(v) if v.streaming => "text",
            OrchestrationV2TurnItem::Reasoning(v) if v.streaming => "text",
            _ => return None,
        };
        let mut value = item_value(item);
        if let Some(obj) = value.as_object_mut() {
            obj.remove(field);
            obj.remove("updatedAt");
        }
        Some(value)
    }
    metadata(&previous.item).is_some_and(|p| metadata(&next.item).as_ref() == Some(&p))
}
pub fn derive_timeline_entries_from_visible_turn_items_with_state(
    input: TimelineEntriesInput,
    previous: Option<&TimelineEntriesProjection>,
) -> TimelineEntriesProjection {
    if let Some(entries) = previous.and_then(|p| reuse_timeline_entries(&input, p)) {
        return TimelineEntriesProjection { input, entries };
    }
    let mut entries = derive_timeline_entries_from_visible_turn_items(&input);
    if let Some(previous) = previous {
        let by_id: BTreeMap<_, _> = previous
            .entries
            .iter()
            .map(|e| (e.id.as_str(), e))
            .collect();
        for entry in &mut entries {
            let Some(before) = by_id.get(entry.id.as_str()) else {
                continue;
            };
            if *entry == **before {
                *entry = (*before).clone();
                continue;
            }
            if input.attachment_url_by_id == previous.input.attachment_url_by_id
                && let (
                    TimelineContent::Message {
                        message: old_message,
                        projected_item: Some(old),
                    },
                    TimelineContent::Message {
                        message,
                        projected_item: Some(new),
                    },
                ) = (&before.content, &mut Rc::make_mut(entry).content)
            {
                if old == new {
                    *message = old_message.clone();
                } else if is_streaming_turn_item_text_update(old, new) {
                    Rc::make_mut(message)
                        .attachments
                        .clone_from(&old_message.attachments);
                }
            }
        }
    }
    TimelineEntriesProjection {
        input,
        entries: Rc::new(entries),
    }
}
fn same_plan_statuses(
    next: &[OrchestrationV2PlanArtifact],
    before: &[OrchestrationV2PlanArtifact],
) -> bool {
    next.len() == before.len()
        && next.iter().zip(before).all(|(next, before)| {
            let next = wire_value(next);
            let before = wire_value(before);
            next["id"] == before["id"]
                && next["kind"] == before["kind"]
                && next["status"] == before["status"]
        })
}
fn reuse_timeline_entries(
    input: &TimelineEntriesInput,
    previous: &TimelineEntriesProjection,
) -> Option<Rc<Vec<Rc<TimelineEntry>>>> {
    let before = &previous.input;
    if input.visible_turn_items.len() < before.visible_turn_items.len()
        || input.optimistic_messages != before.optimistic_messages
        || input.anchored_messages != before.anchored_messages
        || input.attachment_url_by_id != before.attachment_url_by_id
        || input.attempts != before.attempts
        || input.nodes != before.nodes
        || !same_plan_statuses(&input.plans, &before.plans)
    {
        return None;
    }
    let appended = input.visible_turn_items.len() > before.visible_turn_items.len();
    if appended && (!input.optimistic_messages.is_empty() || !input.anchored_messages.is_empty()) {
        return None;
    }
    if appended
        && input.visible_turn_items[before.visible_turn_items.len()..]
            .iter()
            .any(|row| match &row.item {
                OrchestrationV2TurnItem::UserInputRequest(v) => {
                    v.question_answer.as_ref().is_some_and(Option::is_some)
                }
                OrchestrationV2TurnItem::UserMessage(v) => {
                    id_key(&v.message_id).starts_with("async-answer:")
                }
                _ => false,
            })
    {
        return None;
    }
    let mut patches = BTreeMap::new();
    let mut changed = Vec::new();
    for (old, new) in before
        .visible_turn_items
        .iter()
        .zip(&input.visible_turn_items)
    {
        if Rc::ptr_eq(old, new) {
            continue;
        }
        if matches!(new.item, OrchestrationV2TurnItem::AssistantMessage(_))
            && is_streaming_turn_item_text_update(old, new)
        {
            patches.insert(id_key(items::id(&old.item)), new);
            continue;
        }
        if items::id(&old.item) != items::id(&new.item)
            || std::mem::discriminant(&old.item) != std::mem::discriminant(&new.item)
            || matches!(
                new.item,
                OrchestrationV2TurnItem::UserMessage(_)
                    | OrchestrationV2TurnItem::AssistantMessage(_)
                    | OrchestrationV2TurnItem::UserInputRequest(_)
            )
            || matches!(&old.item,OrchestrationV2TurnItem::DynamicTool(v) if mcp_app_from_tool_item(v).is_some())
            || (!input.anchored_messages.is_empty()
                && projected_created_at(old) != projected_created_at(new))
        {
            return None;
        }
        changed.push(new.clone());
    }
    let rederived = derive_timeline_entries_from_visible_turn_items(&TimelineEntriesInput {
        visible_turn_items: changed.clone(),
        optimistic_messages: vec![],
        anchored_messages: vec![],
        ..input.clone()
    });
    if rederived.len() != changed.len()
        || rederived
            .iter()
            .zip(&changed)
            .any(|(e, row)| e.id != id_key(items::id(&row.item)))
    {
        return None;
    }
    if patches.is_empty() && rederived.is_empty() && !appended {
        return Some(previous.entries.clone());
    }
    let mut replacements: BTreeMap<_, _> =
        rederived.into_iter().map(|e| (e.id.clone(), e)).collect();
    let entries: Option<Vec<_>> = previous
        .entries
        .iter()
        .map(|entry| {
            if let Some(new) = replacements.remove(&entry.id) {
                return Some(new);
            }
            let TimelineContent::Message {
                message,
                projected_item: Some(row),
            } = &entry.content
            else {
                return Some(entry.clone());
            };
            let Some(new) = patches.get(&id_key(items::id(&row.item))) else {
                return Some(entry.clone());
            };
            let OrchestrationV2TurnItem::AssistantMessage(item) = &new.item else {
                return None;
            };
            let mut message = (**message).clone();
            message.text.clone_from(&item.text);
            message.updated_at.clone_from(&item.updated_at.0);
            Some(Rc::new(TimelineEntry {
                content: TimelineContent::Message {
                    message: Rc::new(message),
                    projected_item: Some((*new).clone()),
                },
                ..(**entry).clone()
            }))
        })
        .collect();
    if !replacements.is_empty() {
        return None;
    }
    let mut entries = entries?;
    if appended {
        entries.extend(derive_timeline_entries_from_visible_turn_items(
            &TimelineEntriesInput {
                visible_turn_items: input.visible_turn_items[before.visible_turn_items.len()..]
                    .to_vec(),
                ..input.clone()
            },
        ));
    }
    Some(Rc::new(entries))
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointSummary {
    pub run_id: IdOrIndex<RunId>,
    pub status: CheckpointStatus,
    pub checkpoint_turn_count: u64,
}
pub fn derive_revert_turn_count_by_user_message_id(
    entries: &[Rc<TimelineEntry>],
    checkpoints: &[CheckpointSummary],
    thread_start_available: bool,
    available: Option<&BTreeSet<u64>>,
    unsettled: Option<&IdOrIndex<RunId>>,
) -> BTreeMap<String, u64> {
    let counts: BTreeMap<_, _> = checkpoints
        .iter()
        .filter(|c| {
            matches!(
                c.status,
                CheckpointStatus::Ready | CheckpointStatus::Missing
            )
        })
        .map(|c| (id_key(&c.run_id), c.checkpoint_turn_count))
        .collect();
    let mut targets = BTreeMap::new();
    let mut result = BTreeMap::new();
    let mut previous = None;
    for entry in entries {
        let (run, message, row) = match &entry.content {
            TimelineContent::Message {
                message,
                projected_item,
            } => (
                message.run_id.as_ref(),
                Some(message),
                projected_item.as_deref(),
            ),
            TimelineContent::Work { entry } => (
                if entry.item_type.as_deref() == Some("notification") {
                    entry.run_id.as_ref()
                } else {
                    None
                },
                None,
                entry.projected_item.as_deref(),
            ),
            _ => continue,
        };
        if row.is_some_and(|r| r.visibility == ProjectedTurnItemVisibility::Inherited) {
            continue;
        }
        let Some(run) = run else {
            continue;
        };
        let key = id_key(run);
        let target = targets.entry(key.clone()).or_insert_with(|| {
            let target = previous.as_ref().map_or_else(
                || thread_start_available.then_some(0),
                |run| counts.get(run).copied(),
            );
            previous = Some(key);
            target
        });
        let Some(message) = message else {
            continue;
        };
        if message.role != "user"
            || Some(run) == unsettled
            || !matches!(
                message.input_intent,
                Some(
                    OrchestrationV2UserMessageInputIntent::TurnStart
                        | OrchestrationV2UserMessageInputIntent::QueuedTurn
                )
            )
        {
            continue;
        }
        if let Some(target) =
            target.filter(|t| available.is_none_or(|available| available.contains(t)))
        {
            result.insert(id_key(&message.id), target);
        }
    }
    result
}
pub fn find_revert_checkpoint(
    checkpoints: &[OrchestrationV2Checkpoint],
    turn_count: u64,
) -> Option<&OrchestrationV2Checkpoint> {
    checkpoints.iter().rev().find(|c| {
        matches!(
            c.status,
            CheckpointStatus::Ready | CheckpointStatus::Missing
        ) && if turn_count == 0 {
            c.ordinal_within_scope == 0 && c.app_run_ordinal.is_none()
        } else {
            c.app_run_ordinal == Some(turn_count)
        }
    })
}
