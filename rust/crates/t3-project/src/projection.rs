use crate::Result;
use t3_contracts::{base_schemas::ThreadId, orchestration_v2::OrchestrationV2ThreadProjection};
use t3_orchestration::api::{ProjectionReader, ProjectionRecordSet, RecordFilter, RecordKind};

pub(crate) async fn selected_projection(
    reader: &dyn ProjectionReader,
    id: ThreadId,
    fields: Vec<RecordKind>,
) -> Result<OrchestrationV2ThreadProjection> {
    let records = reader.records(id, fields, RecordFilter::default()).await?;
    let mut projection = OrchestrationV2ThreadProjection {
        updated_at: records.thread.updated_at.clone(),
        thread: records.thread,
        runs: vec![],
        attempts: vec![],
        nodes: vec![],
        subagents: vec![],
        provider_sessions: vec![],
        provider_threads: vec![],
        provider_turns: vec![],
        runtime_requests: vec![],
        messages: vec![],
        plans: vec![],
        turn_items: vec![],
        checkpoint_scopes: vec![],
        checkpoints: vec![],
        context_handoffs: vec![],
        context_transfers: vec![],
        visible_turn_items: vec![],
    };
    for set in records.records {
        match set {
            ProjectionRecordSet::Runs(v) => projection.runs = v,
            ProjectionRecordSet::Attempts(v) => projection.attempts = v,
            ProjectionRecordSet::Nodes(v) => projection.nodes = v,
            ProjectionRecordSet::Subagents(v) => projection.subagents = v,
            ProjectionRecordSet::ProviderSessions(v) => projection.provider_sessions = v,
            ProjectionRecordSet::RuntimeRequests(v) => projection.runtime_requests = v,
            _ => {}
        }
    }
    Ok(projection)
}
