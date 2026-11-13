//! Scheduling, settlement and service routing from orchestration-v2/EffectWorker.ts.
pub use crate::runtime::worker::{
    EffectWorker, OutboxSignals, WorkerOptions, non_retryable_interrupt, retry_delay_ms,
};
use crate::{
    api::*,
    effects::{EffectExecutionOptions, EffectRequest, EffectRow, SessionTransition},
    error::{Error, Result},
    ports::checkpoints_forks::checkpoint_rollback_service::rollback_failure_client_message,
    ports::runs_ingestion::provider_turn_control_service::ProviderTurnControlError,
    provider::{ProviderError, RuntimeResponse, TerminalTaskStatus},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn cancelled(error: &Error) -> bool {
    match error {
        Error::Cancelled | Error::Provider(ProviderError::Cancelled) => true,
        Error::ProviderTurnControl(error) => cancelled(&error.cause),
        _ => false,
    }
}

fn control_failure(error: &Error) -> Option<&ProviderTurnControlError> {
    let Error::ProviderTurnControl(error) = error else {
        return None;
    };
    Some(error.as_ref())
}

fn steer_declined(error: &Error) -> bool {
    matches!(error, Error::Provider(ProviderError::SteerDeclined(_)))
        || control_failure(error).is_some_and(|error| error.steer_declined)
}

pub struct ServiceExecutor {
    pub threads: Arc<dyn ThreadOperations>,
    pub dispatcher: Arc<dyn CommandDispatcher>,
    pub projections: Arc<dyn ProjectionReader>,
    pub sessions: Arc<dyn SessionDirectory>,
    pub control: Arc<dyn ProviderTurnControl>,
    pub rollback: Arc<dyn CheckpointRollback>,
    pub requests: Arc<dyn RuntimeRequestResponder>,
    pub cleanup: Arc<dyn ResourceCleanup>,
}

impl ServiceExecutor {
    async fn steer_records(
        &self,
        thread_id: ThreadId,
        message_id: MessageId,
    ) -> Result<ProjectionRecords> {
        self.projections
            .query_records(ProjectionRecordQuery {
                thread_id,
                fields: vec![RecordKind::Messages, RecordKind::Runs],
                filter: RecordFilter::default(),
                messages: MessageFilter {
                    ids: Some(vec![message_id]),
                    ..Default::default()
                },
                run_statuses: None,
                include_latest_run: false,
            })
            .await
    }
    async fn dispatch(&self, command: Value) -> Result<()> {
        self.dispatcher
            .dispatch(serde_json::from_value(command)?)
            .await?;
        Ok(())
    }

    async fn route(
        &self,
        effect: &EffectRow,
        token: CancellationToken,
        options: EffectExecutionOptions,
    ) -> Result<()> {
        let thread = effect.thread_id.clone();
        match &effect.request {
            EffectRequest::Continue { source_run_id } => {
                self.threads
                    .continue_run_with_options(thread, source_run_id.clone(), options)
                    .await
            }
            EffectRequest::Detach {
                provider_session_id,
                detail,
                revoke_mcp_credential,
            } => {
                self.sessions
                    .detach_thread(DetachSession {
                        provider_session_id: provider_session_id.clone(),
                        thread_id: thread,
                        detail: detail.clone(),
                        revoke_credential: revoke_mcp_credential.unwrap_or(false),
                    })
                    .await
            }
            EffectRequest::ApplySelection {
                provider_session_id,
                provider_thread_id,
                model_selection,
            } => {
                self.control
                    .apply_selection(
                        ApplyModelSelection {
                            thread_id: thread,
                            provider_session_id: provider_session_id.clone(),
                            provider_thread_id: provider_thread_id.clone(),
                            model_selection: model_selection.clone(),
                        },
                        token,
                    )
                    .await
            }
            EffectRequest::Start { run_id } => {
                self.threads
                    .start_with_options(thread, run_id.clone(), options)
                    .await
            }
            EffectRequest::Interrupt {
                provider_session_id,
                provider_thread_id,
                provider_turn_id,
                subagent,
            } => {
                let input = InterruptTurn {
                    target: ProviderControlTarget {
                        thread_id: thread.clone(),
                        provider_session_id: provider_session_id.clone(),
                        provider_thread_id: provider_thread_id.clone(),
                        provider_turn_id: provider_turn_id.clone(),
                    },
                    subagent: subagent.clone(),
                    requested_at: subagent
                        .as_ref()
                        .map(|_| t3_contracts::wire::WireDateTime(effect.created_at.clone())),
                };
                let result = self.control.interrupt_with_outcome(input, token).await;
                let (unresponsive, outcome) = match result {
                    Ok(outcome) => (false, outcome),
                    Err(Error::Provider(ProviderError::InterruptTimeout { .. }))
                        if subagent.is_none() =>
                    {
                        (true, None)
                    }
                    Err(error)
                        if subagent.is_none()
                            && control_failure(&error).is_some_and(|error| error.unresponsive) =>
                    {
                        (true, None)
                    }
                    Err(error)
                        if subagent.is_none()
                            && non_retryable_interrupt(&effect.request, &error.to_string()) =>
                    {
                        (false, None)
                    }
                    Err(error) => return Err(error),
                };
                if subagent.is_some() && outcome.is_none() {
                    return Ok(());
                }
                let mut command = json!({
                    "type": if unresponsive { "thread.interrupt.unresponsive" } else { "thread.background-work.settle" },
                    "commandId": format!("{}:{}", effect.id, if unresponsive { "unresponsive" } else { "background-work-settled" }),
                    "threadId": thread, "providerThreadId": provider_thread_id, "providerTurnId": provider_turn_id,
                });
                if let (Some(subagent), Some(outcome)) = (subagent, outcome) {
                    let mut value = json!({"status":match outcome.status { TerminalTaskStatus::Completed => "completed", TerminalTaskStatus::Failed => "failed", TerminalTaskStatus::Cancelled => "cancelled" },"completedAt":outcome.completed_at});
                    if let Some(result) = outcome.result {
                        value["result"] = result.into();
                    }
                    command["subagent"] =
                        json!({"id":subagent.id,"stoppedAt":effect.created_at,"outcome":value});
                }
                self.dispatch(command).await
            }
            EffectRequest::Steer {
                provider_session_id,
                provider_thread_id,
                provider_turn_id,
                message_id,
                model_selection,
                steer_follow_up_after_run_ordinal,
            } => {
                let input = SteerTurn {
                    target: ProviderControlTarget {
                        thread_id: thread.clone(),
                        provider_session_id: provider_session_id.clone(),
                        provider_thread_id: provider_thread_id.clone(),
                        provider_turn_id: provider_turn_id.clone(),
                    },
                    message_id: message_id.clone(),
                    model_selection: model_selection.clone(),
                };
                let result = self.control.steer(input, token).await;
                match result {
                    Ok(()) => {
                        let records = self
                            .steer_records(thread.clone(), message_id.clone())
                            .await?;
                        let delegated = records.records.iter().any(|set| match set {
                            ProjectionRecordSet::Messages(messages) => {
                                messages.iter().any(|message| {
                                    t3_contracts::stored::to_value(message).is_ok_and(|message| {
                                        message["id"] == message_id.0
                                            && message.get("delegatedCompletion").is_some()
                                    })
                                })
                            }
                            _ => false,
                        });
                        if delegated {
                            self.dispatch(json!({"type":"notification.delivery.accept","commandId":format!("command:mailbox-accepted:{}",effect.id),"threadId":thread,"messageId":message_id})).await?;
                        }
                        Ok(())
                    }
                    Err(error)
                        if steer_declined(&error)
                            || control_failure(&error)
                                .is_some_and(|error| error.turn_completed) =>
                    {
                        let records = self
                            .steer_records(thread.clone(), message_id.clone())
                            .await?;
                        let mut message = None;
                        let mut runs = Vec::new();
                        for set in records.records {
                            match set {
                                ProjectionRecordSet::Messages(messages) => {
                                    message = messages
                                        .iter()
                                        .map(t3_contracts::stored::to_value)
                                        .collect::<std::result::Result<Vec<_>, _>>()?
                                        .into_iter()
                                        .find(|message| message["id"] == message_id.0);
                                }
                                ProjectionRecordSet::Runs(rows) => runs = rows,
                                _ => {}
                            }
                        }
                        let Some(message) = message else {
                            return Err(error);
                        };
                        let run = runs
                            .iter()
                            .map(t3_contracts::stored::to_value)
                            .collect::<std::result::Result<Vec<_>, _>>()?
                            .into_iter()
                            .find(|run| run["id"] == message["runId"]);
                        let Some(run) = run else { return Err(error) };
                        let declined = steer_declined(&error);
                        let delegated = message.get("delegatedCompletion").is_some();
                        let queued = declined || delegated || message.get("notification").is_some();
                        let mut command = json!({"type":"message.dispatch","commandId":format!("command:steer-follow-up:{}",effect.id),"threadId":thread,"messageId":message_id,"text":message["text"],"attachments":message["attachments"],"createdBy":message["createdBy"],"creationSource":message["creationSource"],"dispatchMode":{"type":if queued {"queue_after_active"} else {"start_immediately"}}});
                        for key in [
                            "context",
                            "delegatedCompletion",
                            "notification",
                            "scheduledTaskId",
                            "senderThreadId",
                            "linkedSender",
                        ] {
                            if let Some(value) = message.get(key) {
                                command[key] = value.clone();
                            }
                        }
                        if declined {
                            command["steerFollowUpOfRunId"] = run["id"].clone();
                            if let Some(ordinal) = steer_follow_up_after_run_ordinal {
                                command["steerFollowUpAfterRunOrdinal"] = (*ordinal).into();
                            }
                        }
                        if declined && let Some(selection) = model_selection {
                            command["modelSelection"] = t3_contracts::stored::to_value(selection)?;
                        } else if delegated {
                            command["modelSelection"] = run["modelSelection"].clone();
                        }
                        match self.dispatch(command).await {
                            Err(Error::Service {
                                operation: "ThreadManagementProviderInstanceDisabledError",
                                ..
                            }) => Ok(()),
                            result => result,
                        }
                    }
                    Err(error) => Err(error),
                }
            }
            EffectRequest::Restart {
                provider_session_id,
                provider_thread_id,
                provider_turn_id,
                interrupted_attempt_id,
                run_id,
                session_transition,
            } => {
                let input = RestartTurn {
                    target: ProviderControlTarget {
                        thread_id: thread.clone(),
                        provider_session_id: provider_session_id.clone(),
                        provider_thread_id: provider_thread_id.clone(),
                        provider_turn_id: provider_turn_id.clone(),
                    },
                    replacement_provider_session_id: match session_transition {
                        Some(SessionTransition::Replace {
                            replacement_provider_session_id,
                        }) => Some(replacement_provider_session_id.clone()),
                        _ => None,
                    },
                    interrupted_attempt_id: interrupted_attempt_id.clone(),
                };
                self.control
                    .interrupt_and_await_terminal(input, token.clone())
                    .await?;
                cancel_service(token, async {
                    if let Some(transition) = session_transition {
                        self.sessions
                            .detach_thread(DetachSession {
                                provider_session_id: provider_session_id.clone(),
                                thread_id: thread.clone(),
                                revoke_credential: false,
                                detail: Some(
                                    match transition {
                                        SessionTransition::Replace { .. } => {
                                            "Selection change requires a provider session restart."
                                        }
                                        SessionTransition::Detach => {
                                            "Provider thread handoff replaced this session binding."
                                        }
                                    }
                                    .into(),
                                ),
                            })
                            .await?;
                    }
                    self.threads
                        .start_with_options(thread, run_id.clone(), options)
                        .await
                })
                .await
            }
            EffectRequest::Respond {
                provider_session_id,
                request_id,
                decision,
                feedback,
                answers,
            } => {
                self.requests
                    .respond(
                        thread,
                        provider_session_id.clone(),
                        RuntimeResponse {
                            request_id: request_id.clone(),
                            decision: *decision,
                            feedback: feedback.clone(),
                            answers: answers.clone(),
                        },
                        token,
                    )
                    .await
            }
            EffectRequest::Rollback {
                restore_files,
                provider_thread_id,
                checkpoint_id,
                scope_id,
            } => {
                self.rollback
                    .rollback(
                        RollbackRequest {
                            thread_id: thread,
                            provider_thread_id: provider_thread_id.clone(),
                            checkpoint_id: checkpoint_id.clone(),
                            scope_id: scope_id.clone(),
                            restore_files: restore_files.unwrap_or(true),
                        },
                        token,
                    )
                    .await
            }
            EffectRequest::Capture { run_id, scope_id } => {
                self.threads
                    .finalize(thread.clone(), run_id.clone(), scope_id.clone())
                    .await?;
                self.recover_delegated(thread, None, Some(run_id.clone()))
                    .await?;
                Ok(())
            }
            EffectRequest::CleanupScheduledTasks => {
                self.cleanup
                    .cleanup(thread, ResourceKind::ScheduledTasks)
                    .await
            }
            EffectRequest::CleanupTerminals => {
                let (terminals, devices) = tokio::join!(
                    self.cleanup
                        .cleanup(thread.clone(), ResourceKind::Terminals),
                    self.cleanup.cleanup(thread, ResourceKind::Devices)
                );
                terminals.and(devices)
            }
            EffectRequest::CleanupPreviews => {
                self.cleanup.cleanup(thread, ResourceKind::Previews).await
            }
            EffectRequest::CleanupAttachments { attachment_ids } => {
                self.cleanup
                    .cleanup(thread, ResourceKind::Attachments(attachment_ids.clone()))
                    .await
            }
            EffectRequest::GenerateTitle { kind } => {
                self.threads
                    .generate_title_for_effect(thread, effect.command_id.clone(), kind.clone())
                    .await
            }
            EffectRequest::StopDelegated { reason } => {
                self.threads
                    .stop_delegated_for_effect(thread, effect.command_id.clone(), reason.clone())
                    .await
            }
        }
    }
}

#[async_trait]
impl EffectExecutor for ServiceExecutor {
    async fn execute(&self, effect: EffectRow, cancellation: CancellationToken) -> Result<()> {
        self.execute_with_options(effect, cancellation, EffectExecutionOptions::default())
            .await
    }
    async fn execute_with_options(
        &self,
        effect: EffectRow,
        cancellation: CancellationToken,
        options: EffectExecutionOptions,
    ) -> Result<()> {
        self.execute_routed(effect, cancellation, options).await
    }
    async fn failed(&self, _effect: EffectRow, _error: &Error, _will_retry: bool) -> Result<()> {
        Ok(())
    }
}

impl ServiceExecutor {
    async fn recover_delegated(
        &self,
        thread: ThreadId,
        source: Option<RunId>,
        completed: Option<RunId>,
    ) -> Result<()> {
        match self
            .threads
            .recover_delegated_task(thread.clone(), source, completed)
            .await
        {
            // Integration shim until the host supplies delegated repair; ordinary threads need none.
            Err(Error::Unsupported("ThreadOperations.recover_delegated_task")) => {
                if self
                    .projections
                    .pending_delegated_completion(thread)
                    .await?
                {
                    return Err(Error::Unsupported(
                        "ThreadOperations.recover_delegated_task",
                    ));
                }
                Ok(())
            }
            result => result,
        }
    }
    async fn execute_routed(
        &self,
        effect: EffectRow,
        cancellation: CancellationToken,
        options: EffectExecutionOptions,
    ) -> Result<()> {
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let route = self.route(&effect, cancellation.clone(), options);
        let result = if matches!(
            effect.request,
            EffectRequest::ApplySelection { .. }
                | EffectRequest::Interrupt { .. }
                | EffectRequest::Steer { .. }
                | EffectRequest::Restart { .. }
                | EffectRequest::Respond { .. }
                | EffectRequest::Rollback { .. }
        ) {
            route.await
        } else {
            cancel_service(cancellation.clone(), route).await
        };
        if result.as_ref().is_err_and(cancelled) {
            return Err(Error::Cancelled);
        }
        if let Err(error) = &result
            && !cancellation.is_cancelled()
            && !matches!(
                error,
                Error::Cancelled | Error::Provider(ProviderError::Cancelled)
            )
        {
            cancel_service(
                cancellation,
                self.record_failure(effect, error, options.will_retry),
            )
            .await?;
        }
        result
    }
}

async fn cancel_service(
    token: CancellationToken,
    work: impl std::future::Future<Output = Result<()>>,
) -> Result<()> {
    // These service seams carry no token; dropping their future interrupts the owned work.
    tokio::select! {
        biased;
        _ = token.cancelled() => Err(Error::Cancelled),
        result = work => result,
    }
}

impl ServiceExecutor {
    async fn record_failure(
        &self,
        effect: EffectRow,
        error: &Error,
        will_retry: bool,
    ) -> Result<()> {
        if will_retry
            || matches!(
                error,
                Error::Cancelled | Error::Provider(ProviderError::Cancelled)
            )
        {
            return Ok(());
        }
        let command = match effect.request {
            EffectRequest::Continue { source_run_id } => {
                self.recover_delegated(effect.thread_id, Some(source_run_id), None)
                    .await?;
                None
            }
            EffectRequest::Respond { request_id, .. } => Some(
                json!({"type":"runtime-request.delivery.fail","commandId":format!("{}:delivery-failed",effect.command_id.0),"threadId":effect.thread_id,"requestId":request_id}),
            ),
            EffectRequest::Rollback { .. } => Some(
                json!({"type":"checkpoint.rollback.fail","commandId":format!("{}:rollback-failed",effect.command_id.0),"threadId":effect.thread_id,"requestId":effect.command_id,"message":rollback_failure_client_message(error)}),
            ),
            _ => None,
        };
        if let Some(command) = command {
            let _ = self.dispatch(command).await;
        }
        Ok(())
    }
}
