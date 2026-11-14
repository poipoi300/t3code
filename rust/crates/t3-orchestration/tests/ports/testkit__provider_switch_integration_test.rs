#![allow(
    dead_code,
    reason = "Source held-session fixture and shared pending-work binding are required."
)]

use serde_json::json;
use std::path::Path;
use t3_contracts::{
    application_event::ApplicationStoredEvent,
    base_schemas::{ThreadId, TurnItemId},
    orchestration_v2::{OrchestrationV2PendingBackgroundTask, OrchestrationV2ThreadProjection},
};
use t3_orchestration::{
    error::{Error, Result},
    ports::acp_replay::testkit::orchestrator_scenario::ScenarioServices,
};

// Local shim: the shared owner supplies derivePendingBackgroundWork with the full projection.
pub type PendingWork = dyn Fn(&OrchestrationV2ThreadProjection) -> Result<Vec<OrchestrationV2PendingBackgroundTask>>
    + Send
    + Sync;

pub async fn assert_handoff_releases_running_tool_and_completes_run(
    services: &ScenarioServices,
    thread_id: ThreadId,
    tool_call_id: TurnItemId,
    worktree_path: &Path,
    pending_work: &PendingWork,
) -> Result<()> {
    let before = services
        .projections
        .projection(thread_id.clone())
        .await?
        .ok_or_else(|| Error::ThreadNotFound(thread_id.clone()))?;
    let before_value = serde_json::to_value(&before)?;
    let held = before_value["turnItems"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["id"] == tool_call_id.0))
        .expect("held handoff tool");
    assert_eq!(held["status"], "running");
    let mut events = services.committer.subscribe();
    services.dispatcher.dispatch(serde_json::from_value(json!({"type":"thread.metadata.update","commandId":"command:handoff-cuts-off-tool-call:release","threadId":thread_id,"branch":"t3/handoff-cuts-off-tool-call","worktreePath":worktree_path,"expectedWorktreePath":null}))?).await?;
    services.worker.drain(usize::MAX).await?;
    loop {
        let event = events.recv().await.map_err(|error| Error::Service {
            operation: "handoff waiting event",
            detail: error.to_string(),
        })?;
        if let ApplicationStoredEvent::Orchestration(event) = event {
            let event = serde_json::to_value(event)?;
            if event["event"]["type"] == "run.updated"
                && event["event"]["threadId"] == thread_id.0
                && event["event"]["payload"]["status"] == "waiting"
            {
                break;
            }
        }
    }
    services.worker.drain(usize::MAX).await?;
    let projection = services
        .projections
        .projection(thread_id.clone())
        .await?
        .ok_or_else(|| Error::ThreadNotFound(thread_id))?;
    let value = serde_json::to_value(&projection)?;
    assert_eq!(
        value["runs"]
            .as_array()
            .and_then(|runs| runs.last())
            .expect("handoff run")["status"],
        "completed"
    );
    let tool = value["turnItems"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["id"] == tool_call_id.0))
        .expect("handoff tool after release");
    assert_eq!(tool["status"], "interrupted");
    assert!(!tool["completedAt"].is_null());
    assert!(pending_work(&projection)?.is_empty());
    Ok(())
}
