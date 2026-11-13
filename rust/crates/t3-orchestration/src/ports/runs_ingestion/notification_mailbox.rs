use crate::api::*;
use t3_contracts::orchestration_v2_id_table::IdOrIndex;

pub fn is_buffered_provider_notification(message: &OrchestrationV2ConversationMessage) -> bool {
    message.created_by == OrchestrationV2Actor::Agent
        && message.creation_source == OrchestrationV2CreationSource::Provider
        && message.notification.is_some()
        && message.delegated_completion.is_none()
}

pub fn is_undelivered_mailbox_steer(
    messages: &[OrchestrationV2ConversationMessage],
    runs: &[OrchestrationV2Run],
    turns: &[OrchestrationV2ProviderTurn],
    message_id: &MessageId,
) -> bool {
    let Some(message) = messages
        .iter()
        .find(|message| message.id == IdOrIndex::Id(message_id.clone()))
    else {
        return false;
    };
    if message.delegated_completion.is_none() {
        return false;
    }
    let Some(run) = runs
        .iter()
        .find(|run| Some(&run.id) == message.run_id.as_ref())
    else {
        return false;
    };
    if run.user_message_id == message.id {
        return false;
    }
    let latest = turns
        .iter()
        .filter(|turn| {
            run.active_attempt_id.is_some() && turn.run_attempt_id == run.active_attempt_id
        })
        .fold(
            None,
            |latest: Option<&OrchestrationV2ProviderTurn>, turn| {
                if latest.is_none_or(|latest| turn.ordinal > latest.ordinal) {
                    Some(turn)
                } else {
                    latest
                }
            },
        );
    matches!(
        run.status,
        OrchestrationV2RunStatus::Completed
            | OrchestrationV2RunStatus::Failed
            | OrchestrationV2RunStatus::Interrupted
            | OrchestrationV2RunStatus::Cancelled
            | OrchestrationV2RunStatus::RolledBack
    ) || latest.is_some_and(|turn| turn.status == ProviderTurnStatus::Completed)
}
