//! Port of apps/web/src/components/chat/useComposerResume.ts `onResume`: resumes an interrupted
//! run ("Continue where you left off.") and/or releases the turns held in the thread's queue.
//!
//! Not ported: `persistThreadSettingsForNextTurn` (runtime mode and branch for the next turn). The
//! Rust composer offers no runtime-mode control yet, so the thread's own settings are the ones the
//! web would persist; the branch-mismatch case waits on the worktree lane.
use t3_contracts::orchestration_v2::{CommandMessageDispatch, OrchestrationV2Command};
use t3_contracts::rpc::WsOrchestrationV2DispatchCommandRpc;

use crate::operations::{self, CommandOutcome, ComposerDispatchMode, OperationQueue};
use crate::optimistic::{ProjectionKey, ViewChange};
use crate::rpc::SessionEndpoint;
use crate::thread_views::threads::execution;
use crate::{ClientError, composition::ClientRuntime, platform::Platform, types::ThreadKey};

pub const RESUME_TEXT: &str = "Continue where you left off.";

async fn dispatch<P: Platform>(
    platform: &P,
    endpoint: &SessionEndpoint,
    key: &ThreadKey,
    command: OrchestrationV2Command,
) -> Result<(), ClientError> {
    if !endpoint.available::<WsOrchestrationV2DispatchCommandRpc>(&command) {
        return Err(ClientError::PermissionDenied {
            method: "orchestration.dispatchCommand".into(),
        });
    }
    let queue = OperationQueue::<()>::new(key.environment.clone());
    let id = queue
        .enqueue::<WsOrchestrationV2DispatchCommandRpc>(
            platform,
            &command,
            ProjectionKey::thread(key.environment.clone(), key.thread.clone()),
            0,
            ViewChange {
                apply: (),
                revert: (),
            },
        )
        .await
        .map_err(super::send_transport::operation_error)?;
    let report = queue
        .dispatch(platform, endpoint, &id)
        .await
        .map_err(super::send_transport::operation_error)?;
    if let Some(error) = report.persistence_error {
        return Err(error);
    }
    match report.outcome {
        CommandOutcome::Accepted { .. } => Ok(()),
        CommandOutcome::Rejected { reason } => Err(ClientError::Rejected {
            message: reason,
            detail: None,
        }),
        CommandOutcome::UnknownAfterDisconnect | CommandOutcome::NotSent => {
            Err(ClientError::Disconnected)
        }
    }
}

/// `onResume`. Refuses (Cancelled) when the thread cannot resume or a resume is already running.
pub async fn resume_thread<P: Platform + Clone + 'static>(
    runtime: &ClientRuntime<P>,
    platform: &P,
    key: &ThreadKey,
) -> Result<(), ClientError> {
    let endpoint = runtime
        .client(&key.environment)
        .and_then(|client| client.session())
        .ok_or(ClientError::Disconnected)?;
    let projection = runtime
        .store()
        .snapshot()
        .environments
        .get(&key.environment)
        .and_then(|environment| environment.threads.get(&key.thread))
        .and_then(|thread| thread.projection.clone())
        .ok_or(ClientError::Cancelled)?;
    let millis = |date: &t3_contracts::wire::WireDateTime| {
        crate::time::epoch_ms(&date.0).unwrap_or(0)
    };
    let summary = execution::derive_thread_structure_runtime(&projection, &millis);
    let resumable = execution::derive_resumable_run_id(&projection, summary.as_ref(), &millis);
    let held = projection
        .runs
        .iter()
        .any(|run| run.queue_held == Some(Some(true)));
    if !execution::can_resume_thread(&projection.runtime_requests, resumable.as_ref(), held)
    {
        return Err(ClientError::Cancelled);
    }
    let flags = runtime.composer_thread_flags();
    if !flags.begin_resume(key) {
        return Err(ClientError::Cancelled);
    }
    let queue_resume = || -> Result<OrchestrationV2Command, ClientError> {
        Ok(serde_json::from_value(serde_json::json!({
            "type": "queue.resume",
            "commandId": operations::allocate_command_id(platform, None)?,
            "threadId": key.thread,
        }))?)
    };
    let result = async {
        let Some(run) = resumable else {
            return dispatch(platform, &endpoint, key, queue_resume()?).await;
        };
        // Read before the first await, as the web does: the model the composer shows now.
        let selection = runtime.composer_models(key).snapshot().selection.clone();
        let input: CommandMessageDispatch = serde_json::from_value(serde_json::json!({
            "commandId": operations::allocate_command_id(platform, None)?,
            "threadId": key.thread,
            "messageId": operations::allocate_command_id(platform, None)?.0,
            "text": RESUME_TEXT,
            "attachments": [],
            "modelSelection": selection,
            "createdBy": "user",
            "creationSource": runtime.creation_source(),
            "dispatchMode": {"type": "start_immediately"},
            "manualContinuationOfRunId": run,
        }))?;
        dispatch(
            platform,
            &endpoint,
            key,
            operations::message_command(input, ComposerDispatchMode::Start),
        )
        .await?;
        if held {
            dispatch(platform, &endpoint, key, queue_resume()?).await?;
        }
        Ok(())
    }
    .await;
    flags.end_resume(key);
    result
}
