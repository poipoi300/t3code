use std::collections::BTreeMap;
use t3_contracts::orchestration_v2::*;
use t3_contracts::orchestration_v2_id_table::IdOrIndex;

fn run_key(id: &IdOrIndex<t3_contracts::base_schemas::RunId>) -> String {
    match id {
        IdOrIndex::Id(id) => format!("id:{}", id.0),
        IdOrIndex::Index(index) => format!("index:{index}"),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThreadCheckpointSummary<'a> {
    pub checkpoint: &'a OrchestrationV2Checkpoint,
    pub assistant_message: Option<&'a OrchestrationV2ConversationMessage>,
}
pub fn derive_thread_checkpoint_summaries(
    p: &OrchestrationV2ThreadProjection,
) -> Vec<ThreadCheckpointSummary<'_>> {
    let mut ids: BTreeMap<_, _> = p
        .checkpoints
        .iter()
        .filter(|c| c.app_run_ordinal.is_some())
        .filter_map(|c| c.run_id.as_ref())
        .map(|id| (run_key(id), None))
        .collect();
    let mut next = p.messages.len();
    p.checkpoints
        .iter()
        .filter(|c| c.app_run_ordinal.is_some() && c.run_id.is_some())
        .map(|checkpoint| {
            let run = checkpoint.run_id.as_ref().map(run_key);
            while next > 0
                && run
                    .as_ref()
                    .and_then(|run| ids.get(run))
                    .is_some_and(Option::is_none)
            {
                next -= 1;
                let message = &p.messages[next];
                if message.role == ConversationMessageRole::Assistant
                    && let Some(slot) = message
                        .run_id
                        .as_ref()
                        .map(run_key)
                        .and_then(|id| ids.get_mut(&id))
                    && slot.is_none()
                {
                    *slot = Some(message);
                }
            }
            ThreadCheckpointSummary {
                checkpoint,
                assistant_message: run.as_ref().and_then(|run| ids.get(run)).copied().flatten(),
            }
        })
        .collect()
}
pub fn checkpoint_file_totals(
    checkpoint: &OrchestrationV2Checkpoint,
) -> OrchestrationV2CheckpointFileTotals {
    checkpoint
        .files_total
        .clone()
        .unwrap_or_else(|| OrchestrationV2CheckpointFileTotals {
            count: checkpoint.files.len() as u64,
            additions: checkpoint.files.iter().map(|f| f.additions).sum(),
            deletions: checkpoint.files.iter().map(|f| f.deletions).sum(),
        })
}
