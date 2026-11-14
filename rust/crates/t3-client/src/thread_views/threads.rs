//! Thread presentation and lifecycle selectors added after the first wave.
pub mod presentation;
pub mod turn_item_presentation;

macro_rules! item_field {
    ($item:expr, $field:ident) => {
        match $item {
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::Notification(item) => {
                &item.$field
            }
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::UserMessage(item) => {
                &item.$field
            }
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::AssistantMessage(item) => {
                &item.$field
            }
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::Reasoning(item) => {
                &item.$field
            }
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::ProposedPlan(item) => {
                &item.$field
            }
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::TodoList(item) => &item.$field,
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::UserInputRequest(item) => {
                &item.$field
            }
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::FileChange(item) => {
                &item.$field
            }
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::CommandExecution(item) => {
                &item.$field
            }
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::FileSearch(item) => {
                &item.$field
            }
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::WebSearch(item) => {
                &item.$field
            }
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::ApprovalRequest(item) => {
                &item.$field
            }
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::Checkpoint(item) => {
                &item.$field
            }
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::RunInterruptRequest(item) => {
                &item.$field
            }
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::RunInterruptResult(item) => {
                &item.$field
            }
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::SystemNotice(item) => {
                &item.$field
            }
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::Error(item) => &item.$field,
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::Compaction(item) => {
                &item.$field
            }
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::Handoff(item) => &item.$field,
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::Fork(item) => &item.$field,
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::ThreadCreated(item) => {
                &item.$field
            }
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::SecretRequest(item) => {
                &item.$field
            }
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::LinkRequest(item) => {
                &item.$field
            }
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::Subagent(item) => &item.$field,
            t3_contracts::orchestration_v2::OrchestrationV2TurnItem::DynamicTool(item) => {
                &item.$field
            }
        }
    };
}
pub mod activity_announcement;
pub mod archived;
pub mod checkpoints;
pub mod context_window;
pub mod detail;
pub mod execution;
pub mod execution_presentation;
pub mod families;
pub mod find;
pub mod minimap;
pub mod feedback;
pub mod item_support;
pub mod lifecycle;
pub mod notification_center;
pub mod notifications;
pub mod older_settled;
pub mod pending_work;
pub mod queries;
pub mod search;
pub mod shell;
pub mod sort;
pub mod subagent_display;
pub mod subagent_runtime;
pub mod subagents;
#[cfg(test)]
mod tests;
pub mod title_regeneration;
pub mod undo;
pub mod workflows;
pub const THREAD_SNAPSHOT_IDLE_TTL_MS: u64 = 5 * 60_000;
