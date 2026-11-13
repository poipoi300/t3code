use std::collections::HashSet;
use t3_contracts::{
    base_schemas::MessageId, orchestration_v2::*, orchestration_v2_id_table::IdOrIndex,
};

#[derive(Hash, PartialEq, Eq)]
enum MessageKey<'a> {
    Id(&'a MessageId),
    Index(u64),
}

fn message_key(id: &IdOrIndex<MessageId>) -> MessageKey<'_> {
    match id {
        IdOrIndex::Id(id) => MessageKey::Id(id),
        IdOrIndex::Index(index) => MessageKey::Index(*index),
    }
}

pub fn is_automatic_completion_run(
    projection: &OrchestrationV2ThreadProjection,
    run: &OrchestrationV2Run,
) -> bool {
    projection
        .messages
        .iter()
        .any(|message| message.id == run.user_message_id && message.delegated_completion.is_some())
}

pub fn queued_runs_in_delivery_order(
    projection: &OrchestrationV2ThreadProjection,
) -> Vec<&OrchestrationV2Run> {
    let automatic: HashSet<_> = projection
        .messages
        .iter()
        .filter(|message| message.delegated_completion.is_some())
        .map(|message| message_key(&message.id))
        .collect();
    let mut runs: Vec<_> = projection
        .runs
        .iter()
        .filter(|run| run.status == OrchestrationV2RunStatus::Queued)
        .collect();
    runs.sort_by(|left, right| {
        automatic
            .contains(&message_key(&right.user_message_id))
            .cmp(&automatic.contains(&message_key(&left.user_message_id)))
            .then_with(|| {
                left.queue_position
                    .flatten()
                    .unwrap_or(left.ordinal)
                    .cmp(&right.queue_position.flatten().unwrap_or(right.ordinal))
            })
            .then_with(|| left.ordinal.cmp(&right.ordinal))
    });
    runs
}
