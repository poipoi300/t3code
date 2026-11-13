use t3_contracts::{
    base_schemas::RunId, orchestration_v2::*, orchestration_v2_id_table::IdOrIndex,
};

pub fn turn_item_is_workspace_preparation(item: &OrchestrationV2TurnItem) -> bool {
    match item {
        OrchestrationV2TurnItem::CommandExecution(item) => item.input == "Preparing workspace",
        OrchestrationV2TurnItem::Error(item) => {
            item.status == OrchestrationV2TurnItemStatus::Cancelled && preparation_failure(item)
        }
        _ => false,
    }
}

fn preparation_failure(item: &TurnItemError) -> bool {
    item.failure.code.as_ref().map(|code| code.0.as_str()) == Some("workspace_preparation_failed")
}

pub fn workspace_preparation_retry_run_ids(
    runs: &[OrchestrationV2Run],
    items: &[OrchestrationV2TurnItem],
) -> Vec<IdOrIndex<RunId>> {
    let failed: Vec<_> = runs
        .iter()
        .filter(|run| {
            run.status == OrchestrationV2RunStatus::Failed && run.workspace_preparation.is_some()
        })
        .map(|run| &run.id)
        .collect();
    let mut retryable = Vec::new();
    for id in items.iter().filter_map(|item| match item {
        OrchestrationV2TurnItem::Error(item)
            if item.status == OrchestrationV2TurnItemStatus::Failed
                && preparation_failure(item) =>
        {
            item.run_id
                .as_ref()
                .filter(|id| failed.contains(id))
                .cloned()
        }
        _ => None,
    }) {
        if !retryable.contains(&id) {
            retryable.push(id);
        }
    }
    retryable
}
