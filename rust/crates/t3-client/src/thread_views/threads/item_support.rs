use crate::state::items;
use t3_contracts::{
    base_schemas::TurnItemId, orchestration_v2::*, orchestration_v2_id_table::IdOrIndex,
};

#[derive(Default, PartialEq)]
pub struct ItemSupport<'a> {
    pub item: Option<&'a OrchestrationV2TurnItem>,
    pub run: Option<&'a OrchestrationV2Run>,
    pub attempts: Vec<&'a OrchestrationV2RunAttempt>,
    pub node: Option<&'a OrchestrationV2ExecutionNode>,
    pub provider_session: Option<&'a OrchestrationV2ProviderSession>,
    pub provider_thread: Option<&'a OrchestrationV2ProviderThread>,
    pub provider_turn: Option<&'a OrchestrationV2ProviderTurn>,
    pub runtime_request: Option<&'a OrchestrationV2RuntimeRequest>,
    pub checkpoint: Option<&'a OrchestrationV2Checkpoint>,
    pub subagent: Option<&'a OrchestrationV2Subagent>,
    pub context_handoff: Option<&'a OrchestrationV2ContextHandoff>,
    pub context_transfer: Option<&'a OrchestrationV2ContextTransfer>,
}
pub fn resolve_item_support<'a>(
    p: &'a OrchestrationV2ThreadProjection,
    id: &IdOrIndex<TurnItemId>,
) -> ItemSupport<'a> {
    let Some(item) = p
        .turn_items
        .iter()
        .find(|i| items::id(i) == id)
        .or_else(|| {
            p.visible_turn_items
                .iter()
                .find(|r| &r.source_item_id == id)
                .map(|r| &r.item)
        })
    else {
        return ItemSupport::default();
    };
    let node = items::node_id(item)
        .as_ref()
        .and_then(|id| p.nodes.iter().find(|n| &n.id == id));
    let provider_thread = (match item {
        OrchestrationV2TurnItem::Notification(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::UserMessage(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::AssistantMessage(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::Reasoning(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::ProposedPlan(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::TodoList(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::UserInputRequest(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::FileChange(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::CommandExecution(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::FileSearch(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::WebSearch(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::ApprovalRequest(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::Checkpoint(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::RunInterruptRequest(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::RunInterruptResult(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::SystemNotice(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::Error(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::Compaction(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::Handoff(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::Fork(i) => i.provider_thread_id.as_ref().and_then(Option::as_ref),
        OrchestrationV2TurnItem::ThreadCreated(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::SecretRequest(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::LinkRequest(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::Subagent(i) => i.provider_thread_id.as_ref(),
        OrchestrationV2TurnItem::DynamicTool(i) => i.provider_thread_id.as_ref(),
    })
    .and_then(|id| p.provider_threads.iter().find(|t| &t.id == id));
    let request = match item {
        OrchestrationV2TurnItem::ApprovalRequest(i) => Some(&i.request_id),
        OrchestrationV2TurnItem::UserInputRequest(i) => Some(&i.request_id),
        _ => node.and_then(|n| n.runtime_request_id.as_ref()),
    };
    let context_handoff = match item {
        OrchestrationV2TurnItem::Handoff(i) => p
            .context_handoffs
            .iter()
            .find(|h| h.id == i.context_handoff_id),
        _ => None,
    };
    ItemSupport {
        item: Some(item),
        run: items::run_id(item)
            .as_ref()
            .and_then(|id| p.runs.iter().find(|r| &r.id == id)),
        attempts: p
            .attempts
            .iter()
            .filter(|a| Some(&a.run_id) == items::run_id(item).as_ref())
            .collect(),
        node,
        provider_session: provider_thread
            .and_then(|t| t.provider_session_id.as_ref())
            .and_then(|id| p.provider_sessions.iter().find(|s| &s.id == id)),
        provider_thread,
        provider_turn: item_field!(item, provider_turn_id)
            .as_ref()
            .and_then(|id| p.provider_turns.iter().find(|t| &t.id == id)),
        runtime_request: request.and_then(|id| p.runtime_requests.iter().find(|r| &r.id == id)),
        checkpoint: match item {
            OrchestrationV2TurnItem::Checkpoint(i) => {
                p.checkpoints.iter().find(|c| c.id == i.checkpoint_id)
            }
            _ => None,
        },
        subagent: match item {
            OrchestrationV2TurnItem::Subagent(i) => {
                p.subagents.iter().find(|s| s.id == i.subagent_id)
            }
            _ => None,
        },
        context_transfer: context_handoff
            .and_then(|h| h.transfer_id.as_ref().and_then(Option::as_ref))
            .and_then(|id| p.context_transfers.iter().find(|t| &t.id == id)),
        context_handoff,
    }
}
