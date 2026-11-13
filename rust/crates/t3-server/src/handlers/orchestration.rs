//! Package D: orchestration RPC handlers. Source mappings: server plan, `D-orchestration`.
//!
//! `register_unavailable` declares all 33 owned methods unimplemented (the default build and
//! a server without an orchestration service). `register_with` (feature `orchestration`)
//! registers the methods `t3-orchestration` can serve today and keeps the rest unimplemented;
//! each of those waits for the service method named beside it.
use crate::registry::HandlerRegistry;
use t3_contracts::rpc::*;

/// Methods no orchestration service serves yet, with the service method each waits for.
fn register_pending(registry: &mut HandlerRegistry) -> crate::Result<()> {
    // ThreadMessageIntake::launch_thread (ThreadLaunchService::launch exists, its intake wrapper does not).
    registry.unimplemented::<WsOrchestrationV2LaunchThreadRpc>()?;
    // CommandOutputHub::subscribe (ports::transport::command_output_hub is empty).
    registry.unimplemented::<WsOrchestrationV2SubscribeCommandOutputRpc>()?;
    // ProviderSessionManager::subscribe_prompt_suggestion.
    registry.unimplemented::<WsOrchestrationV2SubscribePromptSuggestionRpc>()?;
    // ThreadSearch::search_threads (ports::transport::thread_search is empty).
    registry.unimplemented::<WsOrchestrationV2SearchThreadsRpc>()?;
    // ThreadFind::search_thread_stream (ports::transport::thread_find is empty).
    registry.unimplemented::<WsOrchestrationV2SearchThreadStreamRpc>()?;
    // ProjectionReader::{thread_titles, thread_link_metadata, older_settled_threads}.
    registry.unimplemented::<WsOrchestrationV2GetThreadTitlesRpc>()?;
    registry.unimplemented::<WsOrchestrationV2GetThreadLinkMetadataRpc>()?;
    registry.unimplemented::<WsOrchestrationV2GetOlderSettledThreadsRpc>()?;
    // CheckpointDiffQuery::{get_turn_diff, get_full_thread_diff}.
    registry.unimplemented::<WsOrchestrationV2GetTurnDiffRpc>()?;
    registry.unimplemented::<WsOrchestrationV2GetFullThreadDiffRpc>()?;
    // workflow_script_query::read_workflow_script, native_child_work_query::get_native_child_work.
    registry.unimplemented::<WsOrchestrationV2GetWorkflowScriptRpc>()?;
    registry.unimplemented::<WsOrchestrationV2GetNativeChildWorkRpc>()?;
    // ProjectCloneTracker::{start, cancel, retry, stream}.
    registry.unimplemented::<WsProjectCloneStartRpc>()?;
    registry.unimplemented::<WsProjectCloneCancelRpc>()?;
    registry.unimplemented::<WsProjectCloneRetryRpc>()?;
    registry.unimplemented::<WsSubscribeProjectClonesRpc>()?;
    // ProjectService::{mutate, ensure_scratch, create_new, create_from_thread}.
    registry.unimplemented::<WsProjectsMutateRpc>()?;
    registry.unimplemented::<WsProjectsEnsureScratchRpc>()?;
    registry.unimplemented::<WsProjectsCreateNewRpc>()?;
    registry.unimplemented::<WsProjectsCreateFromThreadRpc>()?;
    // AgentSessionScanner::scan, AgentSessionImporter::import_recent_agent_threads.
    registry.unimplemented::<WsAgentSessionsScanRpc>()?;
    registry.unimplemented::<WsAgentSessionsImportRpc>()?;
    Ok(())
}

/// Methods served once an orchestration adapter is present.
fn register_adapter_backed_unavailable(registry: &mut HandlerRegistry) -> crate::Result<()> {
    registry.unimplemented::<WsOrchestrationV2SubscribeShellRpc>()?;
    registry.unimplemented::<WsOrchestrationV2SubscribeThreadRpc>()?;
    registry.unimplemented::<WsOrchestrationV2GetThreadProjectionRpc>()?;
    registry.unimplemented::<WsOrchestrationV2GetTurnItemRpc>()?;
    registry.unimplemented::<WsOrchestrationV2SearchThreadRpc>()?;
    registry.unimplemented::<WsOrchestrationV2GetArchivedShellSnapshotRpc>()?;
    registry.unimplemented::<WsOrchestrationV2GetThreadFamilyRpc>()?;
    registry.unimplemented::<WsOrchestrationV2GetWorktreeThreadIdsRpc>()?;
    Ok(())
}

/// Commands served by the composed runtime (package A wires `OrchestrationAdapter::start`).
fn register_commands_unavailable(registry: &mut HandlerRegistry) -> crate::Result<()> {
    registry.unimplemented::<WsOrchestrationV2DispatchCommandRpc>()?;
    registry.unimplemented::<WsOrchestrationV2StopWorkflowRpc>()?;
    registry.unimplemented::<WsOrchestrationV2ForceStopThreadRpc>()?;
    Ok(())
}

pub fn register_unavailable(registry: &mut HandlerRegistry) -> crate::Result<()> {
    register_pending(registry)?;
    register_commands_unavailable(registry)?;
    register_adapter_backed_unavailable(registry)
}

#[cfg(feature = "orchestration")]
pub use live::register_with;

#[cfg(feature = "orchestration")]
mod live {
    use super::{HandlerRegistry, register_pending};
    use crate::{
        adapters::orchestration::{OrchestrationAdapter, OrchestrationError},
        registry::Failure,
    };
    use futures::StreamExt;
    use serde_json::json;
    use std::sync::Arc;
    use t3_contracts::{base_schemas::ThreadId, orchestration_v2::*, rpc::*};

    fn cause(error: &OrchestrationError) -> Option<Option<serde_json::Value>> {
        Some(Some(json!({"message": error.to_string()})))
    }
    fn thread_error(
        thread_id: ThreadId,
        message: impl Into<String>,
        error: &OrchestrationError,
    ) -> Failure<WsOrchestrationV2GetThreadProjectionError> {
        Failure::Typed(
            WsOrchestrationV2GetThreadProjectionError::OrchestrationV2GetThreadProjectionError(
                Box::new(OrchestrationV2GetThreadProjectionError {
                    _tag: GetThreadProjectionErrorTag::OrchestrationV2GetThreadProjectionError,
                    thread_id,
                    message: message.into(),
                    cause: cause(error),
                }),
            ),
        )
    }
    fn shell_error(
        message: &str,
        error: &OrchestrationError,
    ) -> Failure<WsOrchestrationV2GetArchivedShellSnapshotError> {
        Failure::Typed(
            WsOrchestrationV2GetArchivedShellSnapshotError::OrchestrationV2GetShellSnapshotError(
                Box::new(OrchestrationV2GetShellSnapshotError {
                    _tag: GetShellSnapshotErrorTag::OrchestrationV2GetShellSnapshotError,
                    message: message.into(),
                    cause: cause(error),
                }),
            ),
        )
    }
    fn search_error(error: &OrchestrationError) -> Failure<WsOrchestrationV2SearchThreadError> {
        Failure::Typed(
            WsOrchestrationV2SearchThreadError::OrchestrationV2SearchThreadError(Box::new(
                OrchestrationV2SearchThreadError {
                    _tag: SearchThreadErrorTag::OrchestrationV2SearchThreadError,
                    cause: json!({"message": error.to_string()}),
                },
            )),
        )
    }
    /// The client command passes the command gate (`Lifecycle::admit_command`), then intake.
    fn register_commands(
        registry: &mut HandlerRegistry,
        adapter: &Arc<OrchestrationAdapter>,
    ) -> crate::Result<()> {
        fn internal<E>(error: &dyn std::fmt::Display) -> Failure<E> {
            Failure::Defect(crate::registry::ServerDefect::Internal {
                message: error.to_string(),
            })
        }
        let a = adapter.clone();
        registry.register::<WsOrchestrationV2DispatchCommandRpc, _, _>(move |ctx, command| {
            let a = a.clone();
            async move {
                let value = serde_json::to_value(&command).unwrap_or_default();
                let refuse = |message: String, detail: Option<String>| {
                    let error = json!({
                        "_tag": "OrchestrationV2DispatchCommandError",
                        "commandId": value["commandId"],
                        "commandType": value["type"],
                        "message": message,
                        "detail": detail,
                    });
                    match serde_json::from_value(error) {
                        Ok(error) => Failure::Typed(
                            WsOrchestrationV2DispatchCommandError::OrchestrationV2DispatchCommandError(
                                Box::new(error),
                            ),
                        ),
                        Err(error) => internal(&error),
                    }
                };
                if let Err(refused) = ctx.services.lifecycle.admit_command() {
                    return Err(refuse(refused.to_string(), None));
                }
                a.dispatch_command(command).await.map_err(|error| {
                    refuse(
                        "Failed to dispatch orchestration V2 command".into(),
                        Some(error.to_string()),
                    )
                })
            }
        })?;
        let a = adapter.clone();
        registry.register::<WsOrchestrationV2StopWorkflowRpc, _, _>(move |_, input| {
            let a = a.clone();
            async move {
                a.stop_workflow(input)
                    .await
                    .map_err(|error| internal(&error))
            }
        })?;
        let a = adapter.clone();
        registry.register::<WsOrchestrationV2ForceStopThreadRpc, _, _>(move |_, input| {
            let a = a.clone();
            async move {
                a.force_stop_thread(input)
                    .await
                    .map_err(|error| internal(&error))
            }
        })?;
        Ok(())
    }
    pub fn register_with(
        registry: &mut HandlerRegistry,
        adapter: Arc<OrchestrationAdapter>,
    ) -> crate::Result<()> {
        register_pending(registry)?;
        register_commands(registry, &adapter)?;
        let a = adapter.clone();
        registry.register_stream::<WsOrchestrationV2SubscribeShellRpc, _, _, _>(
            move |_, input| {
                let a = a.clone();
                async move {
                    Ok(a.subscribe_shell(input).map(|item| {
                        item.map_err(|e| {
                            shell_error("Failed while streaming the orchestration V2 shell", &e)
                        })
                    }))
                }
            },
        )?;
        let a = adapter.clone();
        registry.register_stream::<WsOrchestrationV2SubscribeThreadRpc, _, _, _>(
            move |_, input| {
                let a = a.clone();
                async move {
                    let thread_id = input.thread_id.clone();
                    Ok(a.subscribe_thread(input).map(move |item| {
                        item.map_err(|e| {
                            thread_error(
                                thread_id.clone(),
                                format!(
                                    "Failed while streaming orchestration V2 thread {}",
                                    thread_id.0
                                ),
                                &e,
                            )
                        })
                    }))
                }
            },
        )?;
        let a = adapter.clone();
        registry.register::<WsOrchestrationV2GetThreadProjectionRpc, _, _>(move |_, input| {
            let a = a.clone();
            async move {
                a.thread_projection(input.thread_id.clone())
                    .await
                    .map_err(|e| {
                        thread_error(
                            input.thread_id.clone(),
                            format!(
                                "Failed to load orchestration V2 thread {}",
                                input.thread_id.0
                            ),
                            &e,
                        )
                    })
            }
        })?;
        let a = adapter.clone();
        registry.register::<WsOrchestrationV2GetTurnItemRpc, _, _>(move |_, input| {
            let a = a.clone();
            async move {
                let thread_id = input.thread_id.clone();
                a.turn_item(input)
                    .await
                    .map_err(|e| thread_error(thread_id, "Failed to load turn item", &e))
            }
        })?;
        let a = adapter.clone();
        registry.register::<WsOrchestrationV2SearchThreadRpc, _, _>(move |_, input| {
            let a = a.clone();
            async move { a.search_thread(input).await.map_err(|e| search_error(&e)) }
        })?;
        let a = adapter.clone();
        registry.register::<WsOrchestrationV2GetArchivedShellSnapshotRpc, _, _>(move |_, _| {
            let a = a.clone();
            async move {
                a.archived_shell_snapshot()
                    .await
                    .map_err(|e| shell_error("Failed to load archived thread snapshot", &e))
            }
        })?;
        let a = adapter.clone();
        registry.register::<WsOrchestrationV2GetThreadFamilyRpc, _, _>(move |_, input| {
            let a = a.clone();
            async move {
                a.thread_family(input.thread_id)
                    .await
                    .map_err(|e| shell_error("Failed to load the thread family", &e))
            }
        })?;
        let a = adapter;
        registry.register::<WsOrchestrationV2GetWorktreeThreadIdsRpc, _, _>(move |_, input| {
            let a = a.clone();
            async move {
                a.worktree_thread_ids(input.worktree_path.0)
                    .await
                    .map_err(|e| shell_error("Failed to load the worktree's threads", &e))
            }
        })?;
        Ok(())
    }
}
