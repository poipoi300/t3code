use super::{decoded, encoded, not_available};
use crate::{
    services::{OrchestratorService, ProjectService, ThreadService},
    *,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use t3_contracts::{
    base_schemas::{CommandId, MessageId, ProjectId, ThreadId},
    orchestration_v2::OrchestrationV2ServerCommand,
    orchestrator_mcp::*,
    thread_metadata_mcp::ThreadMetadataMcpUpdateResult,
};
use t3_orchestration::composition::CommandCaller;
use t3_orchestration::ports::runs_ingestion::restart_background_note::run_outcome;
use t3_orchestration::ports::runs_ingestion::thread_message_intake::ThreadMessageIntake;
use t3_orchestration::{
    api::*,
    ports::{
        decisions::dispatch_mode_limit::DispatchModes,
        thread_project::thread_management_service::{
            InterruptResult, SendInput, SendMode, ThreadManagementService,
            ThreadManagementServices, ThreadStopService, WaitInput,
        },
    },
    runtime::dispatch::Dispatcher,
    stores::receipts::CommandReceipt,
};

pub struct OrchestrationAdapter {
    pub reader: Arc<dyn ProjectionReader>,
    pub database: t3_orchestration::stores::StoreDatabase,
    pub commands: Arc<dyn CallerCommandSource>,
}

/// One MCP caller's way into orchestration: every write goes through a dispatcher that checks the
/// caller's authority (mode limit, requested escalation, link fence) under the command lock.
pub trait CallerCommandSource: Send + Sync {
    fn threads(&self, caller: CommandCaller) -> ThreadManagementService;
    fn dispatcher(&self, caller: CommandCaller) -> Arc<dyn CommandDispatcher>;
    /// Sends with attachments; `None` where no attachment backend is bound.
    fn intake(&self, caller: CommandCaller) -> Option<ThreadMessageIntake>;
    fn stop(&self) -> Option<&ThreadStopService>;
}

/// The composed runtime: `commands_for`, behind the admission gate client commands pass.
impl CallerCommandSource for t3_orchestration::composition::OrchestrationRuntime {
    fn threads(&self, caller: CommandCaller) -> ThreadManagementService {
        self.commands_for(caller).threads()
    }
    fn dispatcher(&self, caller: CommandCaller) -> Arc<dyn CommandDispatcher> {
        Arc::new(self.commands_for(caller))
    }
    fn intake(&self, caller: CommandCaller) -> Option<ThreadMessageIntake> {
        Some(self.commands_for(caller).intake())
    }
    fn stop(&self) -> Option<&ThreadStopService> {
        Some(&self.stop)
    }
}

/// A bare dispatcher limited per call (`dispatch_with_limit`), for hosts and tests without the
/// composed runtime. It checks the mode limit but not the link fence.
pub struct LimitedCommands {
    pub services: Arc<ThreadManagementServices>,
    pub dispatcher: Dispatcher,
    pub stop: Option<Arc<ThreadStopService>>,
    pub attachments: Option<Arc<dyn AttachmentBackend>>,
}
impl CallerCommandSource for LimitedCommands {
    fn threads(&self, caller: CommandCaller) -> ThreadManagementService {
        let s = &self.services;
        ThreadManagementService::new(ThreadManagementServices {
            reader: s.reader.clone(),
            dispatcher: self.dispatcher(caller),
            committer: s.committer.clone(),
            database: s.database.clone(),
            settings: s.settings.clone(),
            hydrator: s.hydrator.clone(),
            worktree_restore: s.worktree_restore.clone(),
            signals: s.signals.clone(),
            clock: s.clock.clone(),
        })
    }
    fn dispatcher(&self, caller: CommandCaller) -> Arc<dyn CommandDispatcher> {
        Arc::new(LimitedDispatcher {
            dispatcher: self.dispatcher.clone(),
            limit: caller.limit,
        })
    }
    fn intake(&self, caller: CommandCaller) -> Option<ThreadMessageIntake> {
        Some(ThreadMessageIntake {
            threads: self.threads(caller),
            launches: None,
            attachments: self.attachments.clone()?,
            projections: self.services.reader.clone(),
        })
    }
    fn stop(&self) -> Option<&ThreadStopService> {
        self.stop.as_deref()
    }
}

/// `CommandCaller` for this call: the calling thread, its limits, and the link that fences it
/// (only a link paired below full access fences).
pub fn command_caller(context: &CallContext) -> Result<CommandCaller> {
    let link_fence = context
        .link_origin
        .as_ref()
        .filter(|origin| origin.access != ClientAccess::FullAccess)
        .map(|origin| {
            decoded(json!({"sessionId": origin.session_id, "label": origin.label, "access": origin.access}))
        })
        .transpose()?;
    Ok(CommandCaller {
        thread_id: context.scope.thread.as_ref().map(|t| t.thread_id.clone()),
        limit: modes(context),
        link_fence,
    })
}

#[async_trait]
impl crate::services::AttachmentService for OrchestrationAdapter {
    async fn t3_thread_send_attachments(
        &self,
        context: CallContext,
        input: DecodedInput,
    ) -> Result<Value> {
        use t3_orchestration::ports::runs_ingestion::attachment_claims::attachment_is_pending_upload;
        let intake = self
            .commands
            .intake(command_caller(&context)?)
            .ok_or_else(|| {
                not_available(
                    "t3_thread_send_attachments",
                    "AttachmentBackend::claim_pending",
                )
            })?;
        let input = input.into_value();
        let thread = target(&context, &input)?;
        let records = self
            .records(&context, &input, vec![RecordKind::Messages])
            .await?;
        if !records["thread"]["archivedAt"].is_null() {
            return Err(McpError::failure(
                "invalid_request",
                "Unarchive the target thread before sending attachments.",
            ));
        }
        let mut attachments: Vec<t3_contracts::chat_attachment::ChatAttachment> =
            decoded(required(&input, "attachments")?)?;
        let stored = records["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|m| m["attachments"].as_array().into_iter().flatten())
            .collect::<Vec<_>>();
        for attachment in &mut attachments {
            if attachment_is_pending_upload(attachment) {
                continue;
            }
            let value = encoded(&*attachment)?;
            *attachment=stored.iter().rev().find(|a|a["id"]==value["id"]).cloned().cloned().map(decoded).transpose()?.ok_or_else(||McpError::failure("invalid_request","Attachments must be pending uploads or already belong to the target thread."))?;
        }
        let id = command_id();
        let result = intake
            .send_to_thread(SendInput {
                project_id: decoded(records["thread"]["projectId"].clone())?,
                command_id: id.clone(),
                thread_id: thread.clone(),
                message_id: MessageId(id.0),
                text: input["message"].as_str().unwrap_or("").into(),
                attachments,
                mode: SendMode::Auto,
                fresh_context: false,
                model_selection: None,
                scheduled_task_id: None,
                sender_thread_id: context.scope.thread.as_ref().map(|t| t.thread_id.clone()),
                linked_sender: None,
                created_by: decoded(json!("agent"))?,
                creation_source: decoded(json!("mcp"))?,
            })
            .await
            // TS: a claim failure is `orchestration_error` with its message; anything else is
            // "could not be completed". An unbound backend names this tool.
            .map_err(|error| match error {
                t3_orchestration::error::Error::Unsupported(method) => {
                    not_available("t3_thread_send_attachments", method)
                }
                t3_orchestration::error::Error::Service { operation, detail }
                    if operation == "AttachmentClaims.claimPendingAttachments" =>
                {
                    McpError::failure("orchestration_error", detail)
                }
                _ => McpError::unavailable(),
            })?;
        Ok(
            json!({"threadId":thread,"messageId":result.message.id,"runId":result.run.id,"status":result.run.status}),
        )
    }
}

fn failure(error: t3_orchestration::error::Error) -> McpError {
    match error {
        t3_orchestration::error::Error::Unsupported(method) => {
            not_available("orchestration", method)
        }
        t3_orchestration::error::Error::ThreadNotFound(_) => {
            McpError::failure("thread_not_found", "The thread was not found.")
        }
        // TS `dispatchFailure` (threadAccess.ts): the cause, cut at 1,000 characters.
        t3_orchestration::error::Error::Rejected { reason, .. } if !reason.is_empty() => {
            McpError::failure("orchestration_error", reason.chars().take(1000).collect::<String>())
        }
        t3_orchestration::error::Error::Service { operation, detail }
            if operation == "ThreadManagementProviderInstanceDisabledError" =>
        {
            McpError::failure("invalid_request", detail)
        }
        t3_orchestration::error::Error::CallerRefused { refusal, .. } => caller_refused(&refusal),
        _ => McpError::unavailable(),
    }
}

/// TypeScript's codes and texts for a refusal under the command lock: a target that rose above the
/// caller while the call ran is `escalationDenied` (McpToolAccess.ts); the rest keep the
/// runtime's text.
fn caller_refused(refusal: &t3_orchestration::error::CallerRefusal) -> McpError {
    match refusal {
        t3_orchestration::error::CallerRefusal::AboveModeLimit {
            thread_id,
            runtime_mode,
            interaction_mode,
            ..
        } => McpError::failure(
            refusal.code(),
            format!(
                "Thread {} now runs in {runtime_mode}/{interaction_mode} mode, above this caller's. Its user changed it while this call ran.",
                thread_id.0
            ),
        ),
        refusal => McpError::failure(refusal.code(), refusal.to_string()),
    }
}
/// `t3_workflow_stop` (orchestrator handlers.ts): the stop error's reason picks the code and its
/// TypeScript message is the text.
fn workflow_stop_failure(error: t3_orchestration::error::Error) -> McpError {
    // The stop service reports `StopWorkflow.<reason>`; the RPC layer types it the same way.
    let reason = match &error {
        t3_orchestration::error::Error::StopWorkflow(stop) => encoded(&stop.reason)
            .ok()
            .and_then(|reason| reason.as_str().map(str::to_owned))
            .unwrap_or_default(),
        t3_orchestration::error::Error::Service { operation, .. }
            if operation.starts_with("StopWorkflow.") =>
        {
            operation.trim_start_matches("StopWorkflow.").to_owned()
        }
        _ => return failure(error),
    };
    let message = match reason.as_str() {
        "not-running" => "This workflow is no longer running.",
        "unsupported" => "This provider cannot stop a workflow.",
        "unavailable" => "This workflow has no live provider task.",
        _ => "Failed to stop the workflow.",
    };
    McpError::failure(
        if matches!(reason.as_str(), "unavailable" | "stop-failed") {
            "orchestration_error"
        } else {
            "invalid_request"
        },
        message,
    )
}

/// `t3_background_wait`: `unsupported` and `unmatched` are the caller's to fix.
fn background_wait_failure(error: t3_orchestration::error::Error) -> McpError {
    match error {
        t3_orchestration::error::Error::Service { operation, detail }
            if operation.starts_with("ThreadManagementBackgroundWaitError.") =>
        {
            McpError::failure(
                if operation.ends_with(".unsupported") || operation.ends_with(".unmatched") {
                    "invalid_request"
                } else {
                    "orchestration_error"
                },
                detail,
            )
        }
        error => failure(error),
    }
}
/// TS `userFacingDispatchErrorMessage`: the most specific message of the cause.
pub fn user_facing(error: &t3_orchestration::error::Error) -> String {
    match error {
        t3_orchestration::error::Error::Rejected { reason, .. } => reason.clone(),
        t3_orchestration::error::Error::CallerRefused { refusal, .. } => refusal.to_string(),
        t3_orchestration::error::Error::Service { detail, .. } => detail.clone(),
        error => error.to_string(),
    }
}

/// A thread-management error (TS `isThreadManagementError`), mapped by `failure`.
fn is_management_error(error: &t3_orchestration::error::Error) -> bool {
    match error {
        t3_orchestration::error::Error::ThreadNotFound(_)
        | t3_orchestration::error::Error::CallerRefused { .. } => true,
        t3_orchestration::error::Error::Service { operation, .. } => {
            operation.starts_with("ThreadManagement")
        }
        _ => false,
    }
}

fn command_id() -> CommandId {
    CommandId(format!("mcp:{}", uuid::Uuid::new_v4()))
}
fn component(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(byte) {
                char::from(*byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}
fn request_key(input: &Value) -> String {
    input["clientRequestId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}
fn modes(context: &CallContext) -> DispatchModes {
    DispatchModes {
        runtime_mode: context.limits.runtime_mode,
        interaction_mode: context.limits.interaction_mode,
    }
}
fn target(context: &CallContext, input: &Value) -> Result<ThreadId> {
    input
        .get("threadId")
        .and_then(Value::as_str)
        .map(|id| ThreadId(id.into()))
        .or_else(|| context.scope.thread.as_ref().map(|t| t.thread_id.clone()))
        .ok_or_else(|| {
            McpError::failure(
                "target_required",
                "Pass threadId: this MCP client is not running inside a T3 thread.",
            )
        })
}
fn required(input: &Value, key: &str) -> Result<Value> {
    input
        .get(key)
        .cloned()
        .ok_or_else(|| McpError::failure("invalid_request", format!("This action needs `{key}`.")))
}
fn link(context: &CallContext, id: &ThreadId, title: &str) -> String {
    let encode = |value: &str| {
        percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
    };
    format!(
        "[{title}](t3-thread://v2/{}/{})",
        encode(&context.scope.environment_id.0),
        encode(&id.0)
    )
}

/// A bare `dispatch_with_limit` reports a target that rose above the limit as text; the composed
/// runtime returns the typed `CallerRefused`, which `failure` maps.
fn legacy_mode_refusal(context: &CallContext, error: t3_orchestration::error::Error) -> McpError {
    if let t3_orchestration::error::Error::Rejected { reason, .. } = &error
        && let Some(rest) = reason.split(" now runs in ").nth(1)
        && let Some((runtime, interaction)) = rest
            .split_whitespace()
            .next()
            .and_then(|v| v.trim_end_matches(',').split_once('/'))
        && let (Ok(runtime_mode), Ok(interaction_mode)) = (
            serde_json::from_value(json!(runtime)),
            serde_json::from_value(json!(interaction)),
        )
    {
        return crate::access::check_modes(
            context.limits,
            Modes {
                runtime_mode,
                interaction_mode,
            },
        )
        .err()
        .unwrap_or_else(|| failure(error));
    }
    failure(error)
}

struct LimitedDispatcher {
    dispatcher: Dispatcher,
    limit: DispatchModes,
}
#[async_trait]
impl CommandDispatcher for LimitedDispatcher {
    fn command_lanes(
        &self,
    ) -> t3_orchestration::error::Result<
        Arc<t3_orchestration::ports::decisions::thread_command_executor::KeyedLanes>,
    > {
        self.dispatcher.command_lanes()
    }
    async fn dispatch(
        &self,
        command: OrchestrationV2ServerCommand,
    ) -> t3_orchestration::error::Result<CommandReceipt> {
        self.dispatcher
            .dispatch_with_limit(command, Some(self.limit))
            .await
    }
}

impl OrchestrationAdapter {
    fn management(&self, context: &CallContext) -> Result<ThreadManagementService> {
        Ok(self.commands.threads(command_caller(context)?))
    }
    pub async fn shell(&self, id: ThreadId) -> Result<Value> {
        let shell = self
            .reader
            .shell(id)
            .await
            .map_err(failure)?
            .filter(|t| t.deleted_at.is_none())
            .ok_or_else(|| McpError::failure("thread_not_found", "The thread was not found."))?;
        encoded(shell)
    }
    /// TS `loadTargetThread`: "Thread X was not found.", with `threadNotFound`'s hint for a bare
    /// uuid of an `mcp:` thread.
    async fn target_shell(&self, id: ThreadId) -> Result<Value> {
        if let Some(shell) = self
            .reader
            .shell(id.clone())
            .await
            .map_err(failure)?
            .filter(|t| t.deleted_at.is_none())
        {
            return encoded(shell);
        }
        let prefixed = ThreadId(format!("mcp:{}", id.0));
        let live = self
            .reader
            .shell(prefixed.clone())
            .await
            .ok()
            .flatten()
            .is_some_and(|shell| shell.deleted_at.is_none());
        let message = format!("Thread {} was not found.", id.0);
        Err(McpError::failure(
            "thread_not_found",
            if live {
                format!("{message} Did you mean `{}`?", prefixed.0)
            } else {
                message
            },
        ))
    }
    async fn records(
        &self,
        context: &CallContext,
        input: &Value,
        fields: Vec<RecordKind>,
    ) -> Result<Value> {
        encoded(
            self.management(context)?
                .records(target(context, input)?, fields, RecordFilter::default())
                .await
                .map_err(failure)?,
        )
    }
    async fn dispatch(&self, context: &CallContext, mut command: Value) -> Result<Value> {
        if command.get("commandId").is_none() {
            command["commandId"] = encoded(command_id())?;
        }
        let receipt = self
            .commands
            .dispatcher(command_caller(context)?)
            .dispatch(decoded(command)?)
            .await
            .map_err(|error| legacy_mode_refusal(context, error))?;
        Ok(json!({"sequence":receipt.result_sequence}))
    }
    pub fn install(self: Arc<Self>, services: &mut Services) {
        services.orchestrator = self.clone();
        services.thread = self.clone();
        services.project = self;
    }
}

#[async_trait]
impl AccessStore for OrchestrationAdapter {
    async fn thread_shell(&self, id: &ThreadId) -> Result<Option<ThreadShell>> {
        projection_shell(self.reader.as_ref(), id).await
    }
}

/// Access rules over the projections alone, for a host that has no dispatcher to hand over.
pub struct ProjectionAccess(pub Arc<dyn ProjectionReader>);
#[async_trait]
impl AccessStore for ProjectionAccess {
    async fn thread_shell(&self, id: &ThreadId) -> Result<Option<ThreadShell>> {
        projection_shell(self.0.as_ref(), id).await
    }
}

async fn projection_shell(
    reader: &dyn ProjectionReader,
    id: &ThreadId,
) -> Result<Option<ThreadShell>> {
    let Some(shell) = reader.shell(id.clone()).await.map_err(failure)? else {
        return Ok(None);
    };
    let value = encoded(&shell)?;
    let background_subagent_providers = value["pendingBackgroundTasks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["kind"] == "subagent")
        .filter_map(|t| t["providerInstanceId"].as_str())
        .map(|id| t3_contracts::provider_instance::ProviderInstanceId(id.into()))
        .collect();
    let link_origin = value
        .get("linkOrigin")
        .filter(|v| !v.is_null())
        .map(|v| {
            Ok(LinkOrigin {
                session_id: v["sessionId"].as_str().ok_or(McpError::Internal)?.into(),
                label: v["label"].as_str().ok_or(McpError::Internal)?.into(),
                access: decoded(v["access"].clone())?,
            })
        })
        .transpose()?;
    Ok(Some(ThreadShell {
        id: id.clone(),
        modes: Modes {
            runtime_mode: shell.runtime_mode,
            interaction_mode: shell.interaction_mode,
        },
        deleted: shell.deleted_at.is_some(),
        archived: shell.archived_at.is_some(),
        active: shell.active_run_id.is_some(),
        provider_instance_id: Some(shell.provider_instance_id),
        background_subagent_providers,
        link_origin,
    }))
}

#[async_trait]
impl OrchestratorService for OrchestrationAdapter {
    async fn t3_thread_send(
        &self,
        context: CallContext,
        input: OrchestratorMcpThreadSendInput,
    ) -> Result<OrchestratorMcpThreadSendResult> {
        let input = encoded(input)?;
        let thread = target(&context, &input)?;
        let shell = self.target_shell(thread.clone()).await?;
        let key = request_key(&input);
        let command = CommandId(format!(
            "command:mcp:{}:thread-send:{}",
            component(&context.scope.request_namespace),
            component(&key)
        ));
        let message = MessageId(format!(
            "message:mcp:{}:thread-send:{}",
            component(&context.scope.request_namespace),
            component(&key)
        ));
        let result = self
            .management(&context)?
            .send_to_thread(SendInput {
                project_id: decoded(shell["projectId"].clone())?,
                command_id: command.clone(),
                thread_id: thread.clone(),
                message_id: message,
                text: input["message"].as_str().ok_or(McpError::Internal)?.into(),
                attachments: vec![],
                mode: input
                    .get("mode")
                    .map(|v| decoded(v.clone()))
                    .transpose()?
                    .unwrap_or(SendMode::Auto),
                fresh_context: input["freshContext"] == true,
                model_selection: None,
                scheduled_task_id: None,
                sender_thread_id: context.scope.thread.as_ref().map(|t| t.thread_id.clone()),
                linked_sender: context
                    .scope
                    .client
                    .as_ref()
                    .and_then(|c| c.sender_thread.clone())
                    .map(decoded)
                    .transpose()?,
                created_by: decoded(json!("agent"))?,
                creation_source: decoded(json!("mcp"))?,
            })
            .await
            .map_err(|error| {
                if is_management_error(&error) {
                    failure(error)
                } else {
                    McpError::failure(
                        "orchestration_error",
                        format!("Unable to send to thread {}: {}", thread.0, user_facing(&error)),
                    )
                }
            })?;
        let run = encoded(&result.run)?;
        let mut answer = json!({"threadId":thread,"link":link(&context,&thread,shell["title"].as_str().unwrap_or("Thread")),"messageId":result.message.id,"runId":result.run.id});
        // A follow-up to a delegated child runs for its task (TS `taskId`).
        if let Some(task) = run.get("delegatedTaskId").filter(|task| !task.is_null()) {
            answer["taskId"] = task.clone();
        }
        answer["status"] = encoded(&result.run.status)?;
        answer["delivery"] = encoded(&result.delivery)?;
        decoded(answer)
    }
    async fn t3_thread_wait(
        &self,
        context: CallContext,
        input: OrchestratorMcpThreadWaitInput,
    ) -> Result<OrchestratorMcpThreadWaitResult> {
        let input = encoded(input)?;
        let thread = target(&context, &input)?;
        let shell = self.target_shell(thread.clone()).await?;
        let timeout = input["timeoutMs"]
            .as_f64()
            .unwrap_or(600_000.0)
            .clamp(1.0, 3_600_000.0);
        let result = self
            .management(&context)?
            .wait_for_thread(
                WaitInput {
                    project_id: decoded(shell["projectId"].clone())?,
                    thread_id: thread.clone(),
                    run_id: input.get("runId").cloned().map(decoded).transpose()?,
                    timeout: Duration::from_millis(timeout as u64),
                    until_delegated_work_settled: input["untilDelegatedWorkSettled"] == true,
                },
                context.cancellation.clone(),
            )
            .await
            .map_err(failure)?;
        let mut answer = json!({"threadId":thread,"runId":result.run.as_ref().map(|r|&r.id),"status":result.run.as_ref().map(|r|encoded(run_outcome(r))).transpose()?.unwrap_or(json!("idle")),"timedOut":result.timed_out});
        if let Some(run) = result.result_run {
            answer["resultRun"] = json!({"runId":run.id,"status":run_outcome(&run)});
        }
        decoded(answer)
    }
    async fn t3_thread_interrupt(
        &self,
        context: CallContext,
        input: OrchestratorMcpThreadInterruptInput,
    ) -> Result<OrchestratorMcpThreadInterruptResult> {
        let input = encoded(input)?;
        let thread = target(&context, &input)?;
        let shell = self.target_shell(thread.clone()).await?;
        let operation = if input["stop"] == true {
            "thread-stop"
        } else {
            "thread-interrupt"
        };
        let id = CommandId(format!(
            "command:mcp:{}:{}:{}",
            component(&context.scope.request_namespace),
            operation,
            component(&request_key(&input))
        ));
        let reason = input["reason"].as_str().map_or_else(
            || "Stopped by an agent".into(),
            |reason| format!("Stopped by an agent: {reason}"),
        );
        if input["stop"] == true {
            let management = self.management(&context)?;
            management
                .stop_thread(id.clone(), thread.clone(), Some(reason.clone()), true)
                .await
                .map_err(failure)?;
            management
                .stop_delegated_tasks(thread.clone(), id.clone(), Some(reason), None)
                .await
                .map_err(failure)?;
            let target = thread.clone();
            let events = self
                .database
                .read(move |db| {
                    t3_orchestration::stores::event_store::EventStore::new().read(
                        db,
                        &t3_orchestration::stores::event_store::EventFilter {
                            thread_id: Some(target),
                            command_id: Some(id),
                            ..Default::default()
                        },
                    )
                })
                .await
                .map_err(failure)?;
            let events = events
                .into_iter()
                .map(encoded)
                .collect::<Result<Vec<_>>>()?;
            let interrupted = events
                .iter()
                .find(|row| {
                    let event = &row["event"];
                    event["type"] == "run.updated" && event["payload"]["status"] == "interrupted"
                        || event["type"] == "turn-item.updated"
                            && event["payload"]["type"] == "run_interrupt_request"
                            && !event["payload"]["providerTurnId"].is_null()
                })
                .map(|row| row["event"]["runId"].clone())
                .filter(|id| !id.is_null());
            return decoded(
                json!({"threadId":thread,"runId":interrupted,"status":if interrupted.is_some() {"interrupt_requested"} else {"no_active_run"}}),
            );
        }
        let result = self
            .management(&context)?
            .interrupt_thread(
                decoded(shell["projectId"].clone())?,
                thread.clone(),
                id,
                input.get("runId").cloned().map(decoded).transpose()?,
                Some(reason),
            )
            .await
            .map_err(failure)?;
        let (id, status) = match result {
            InterruptResult::NoActiveRun => (Value::Null, json!("no_active_run")),
            InterruptResult::AlreadyTerminal(run) => (encoded(run.id)?, encoded(run.status)?),
            InterruptResult::Requested { run, .. } => {
                (encoded(run.id)?, json!("interrupt_requested"))
            }
        };
        decoded(json!({"threadId":thread,"runId":id,"status":status}))
    }
    async fn t3_thread_update(
        &self,
        context: CallContext,
        input: DecodedInput,
    ) -> Result<ThreadMetadataMcpUpdateResult> {
        let input = input.into_value();
        let thread = target(&context, &input)?;
        let id = CommandId(format!(
            "command:mcp:{}:thread-update:{}:{}:{}",
            component(&context.scope.request_namespace),
            component(&thread.0),
            component(input["action"].as_str().ok_or(McpError::Internal)?),
            component(&request_key(&input))
        ));
        let mut command = json!({"type":"thread.metadata.update","threadId":thread,"commandId":id});
        match input["action"].as_str() {
            Some("rename") => command["title"] = required(&input, "title")?,
            _ => command["regenerateTitle"] = json!(true),
        };
        let result = self.dispatch(&context, command).await?;
        let shell = self.shell(thread.clone()).await?;
        decoded(
            json!({"threadId":thread,"action":input["action"],"commandId":id,"sequence":result["sequence"],"title":shell["title"],"titleRegeneration":shell["titleRegeneration"],"linkedPullRequest":shell["linkedPullRequest"],"updatedAt":shell["updatedAt"]}),
        )
    }
    async fn t3_workflow_stop(&self, context: CallContext, input: DecodedInput) -> Result<Value> {
        let stop = self
            .commands
            .stop()
            .ok_or_else(|| not_available("t3_workflow_stop", "ThreadStopService::stop_workflow"))?;
        let mut input = input.into_value();
        input["threadId"] = encoded(target(&context, &input)?)?;
        input["commandId"] = encoded(command_id())?;
        stop.stop_workflow(decoded(input)?, Some(modes(&context)))
            .await
            .map_err(workflow_stop_failure)?;
        Ok(json!({}))
    }
    async fn t3_background_wait(&self, context: CallContext, input: DecodedInput) -> Result<Value> {
        let stop = self.commands.stop().ok_or_else(|| {
            not_available(
                "t3_background_wait",
                "ThreadStopService::await_background_tasks",
            )
        })?;
        let input = input.into_value();
        let thread = target(&context, &input)?;
        let (matched, unmatched) = stop
            .await_background_tasks(
                thread,
                decoded(required(&input, "taskIds")?)?,
                input["waiting"] != false,
            )
            .await
            .map_err(background_wait_failure)?;
        Ok(json!({"matched":matched,"unmatched":unmatched}))
    }
}

#[async_trait]
impl ThreadService for OrchestrationAdapter {
    async fn t3_thread_configure(
        &self,
        context: CallContext,
        input: DecodedInput,
    ) -> Result<Value> {
        let input = input.into_value();
        let thread = target(&context, &input)?;
        let shell = self.shell(thread.clone()).await?;
        let Some(selection) = input.get("modelSelection") else {
            return Ok(
                json!({"threadId":thread,"modelSelection":shell["modelSelection"],"runtimeMode":shell["runtimeMode"],"interactionMode":shell["interactionMode"]}),
            );
        };
        let kind = if selection["instanceId"] == shell["providerInstanceId"] {
            "thread.model-selection.set"
        } else {
            "provider.switch"
        };
        self.dispatch(
            &context,
            json!({"type":kind,"threadId":thread,"modelSelection":selection}),
        )
        .await
    }
    async fn t3_thread_organize(&self, context: CallContext, input: DecodedInput) -> Result<Value> {
        let input = input.into_value();
        let thread = target(&context, &input)?;
        if input["action"] == "settle" {
            return Err(not_available(
                "t3_thread_organize",
                "ThreadManagementService::settle_thread (deferred own-agent settlement)",
            ));
        }
        let action = input["action"].as_str().ok_or(McpError::Internal)?;
        let mut command =
            json!({"type":format!("thread.{}",action.replace('_',"-")),"threadId":thread});
        if action == "snooze" {
            command["snoozedUntil"] = required(&input, "snoozedUntil")?;
        }
        if matches!(action, "unsnooze" | "unsettle") {
            command["reason"] = json!("user");
        }
        self.dispatch(&context, command).await
    }
    async fn t3_pending_request(&self, context: CallContext, input: DecodedInput) -> Result<Value> {
        let input = input.into_value();
        let records = self
            .records(
                &context,
                &input,
                vec![RecordKind::RuntimeRequests, RecordKind::TurnItems],
            )
            .await?;
        let requests = records["runtimeRequests"]
            .as_array()
            .ok_or(McpError::Internal)?;
        if input["action"] == "list" {
            return Ok(
                json!({"requestIds":requests.iter().filter(|r|r["kind"]=="user_input"&&r["status"]=="pending").map(|r|r["id"].clone()).collect::<Vec<_>>()}),
            );
        }
        let request_id = required(&input, "requestId")?;
        let request = requests
            .iter()
            .find(|r| r["id"] == request_id && r["kind"] == "user_input");
        let question = records["turnItems"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|i| i["type"] == "user_input_request" && i["requestId"] == request_id);
        let (Some(request), Some(_)) = (request, question) else {
            return Err(McpError::failure(
                "invalid_request",
                "The pending user-input request was not found.",
            ));
        };
        if request["status"] != "pending" {
            return Err(McpError::failure(
                "invalid_request",
                if request["status"] == "resolved" {
                    "The user-input request was already answered.".to_owned()
                } else {
                    format!(
                        "The user-input request is no longer pending ({}).",
                        request["status"].as_str().unwrap_or_default()
                    )
                },
            ));
        }
        if input["action"] == "respond" {
            return self.dispatch(&context,json!({"type":"runtime-request.respond","threadId":target(&context,&input)?,"requestId":request["id"],"answers":required(&input,"answers")?})).await;
        }
        let item = records["turnItems"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|i| i["type"] == "user_input_request" && i["requestId"] == request_id)
            .ok_or_else(|| {
                McpError::failure(
                    "invalid_request",
                    "The pending user-input request was not found.",
                )
            })?;
        Ok(json!({"requestId":request_id,"questions":item["questions"]}))
    }
    async fn t3_thread_transfer(&self, context: CallContext, input: DecodedInput) -> Result<Value> {
        let input = input.into_value();
        let thread = target(&context, &input)?;
        if input["action"] == "status" {
            let records = self
                .records(&context, &input, vec![RecordKind::ContextTransfers])
                .await?;
            return Ok(
                json!({"transfers":records["contextTransfers"].as_array().into_iter().flatten().map(|t|json!({"id":t["id"],"sourceThreadId":t["sourceThreadId"],"targetThreadId":t["targetThreadId"],"status":t["status"]})).collect::<Vec<_>>()}),
            );
        }
        let id = command_id();
        let merging = input["action"] == "merge_back";
        let target = if merging {
            required(&input, "targetThreadId")?
        } else {
            json!(format!("{}:fork", id.0))
        };
        let mut command = json!({"type":if merging {"thread.merge_back"} else {"thread.fork"},"commandId":id,"sourceThreadId":thread,"targetThreadId":target,"sourcePoint":input.get("sourcePoint").cloned().unwrap_or(json!({"type":"latest_stable"})),"createdBy":"agent","creationSource":"mcp"});
        if let Some(title) = input.get("title") {
            command["title"] = title.clone();
        }
        let mut result = self.dispatch(&context, command).await?;
        result["targetThreadId"] = target;
        Ok(result)
    }
    async fn t3_queue(&self, context: CallContext, input: DecodedInput) -> Result<Value> {
        let input = input.into_value();
        let action = input["action"].as_str().ok_or(McpError::Internal)?;
        if matches!(action, "list" | "read") {
            let projection = self
                .management(&context)?
                .records(
                    target(&context, &input)?,
                    vec![RecordKind::Runs, RecordKind::Messages],
                    RecordFilter::default(),
                )
                .await
                .map_err(failure)?;
            let runs =
                t3_orchestration::ports::decisions::queued_run_order::queued_runs_in_delivery_order(
                    &projection,
                );
            let entry = |run: &t3_contracts::orchestration_v2::OrchestrationV2Run,
                         limit: usize|
             -> Result<Option<Value>> {
                let message = projection
                    .messages
                    .iter()
                    .find(|m| m.id == run.user_message_id);
                message.map(|m| {
                    let value=encoded(m)?;
                    let text=value["text"].as_str().ok_or(McpError::Internal)?;
                    let characters=text.chars().collect::<Vec<_>>();
                    Ok(json!({"queuedRunId":run.id,"text":characters.iter().take(limit).collect::<String>(),"truncated":characters.len()>limit}))
                }).transpose()
            };
            if action == "read" {
                let id = required(&input, "queuedRunId")?;
                let run = runs
                    .iter()
                    .find(|r| encoded(&r.id).is_ok_and(|v| v == id))
                    .ok_or_else(|| {
                        McpError::failure("invalid_request", "The queued message was not found.")
                    })?;
                return entry(run, 16000)?.ok_or_else(|| {
                    McpError::failure("invalid_request", "The queued message was not found.")
                });
            }
            let cursor = input["cursor"].as_u64().unwrap_or(0) as usize;
            let limit = input["limit"].as_u64().unwrap_or(20) as usize;
            let items = runs
                .iter()
                .skip(cursor)
                .take(limit)
                .map(|r| entry(r, 1000))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            return Ok(
                json!({"items":items,"nextCursor":if cursor.saturating_add(limit)<runs.len() {Some(cursor+limit)} else {None}}),
            );
        }
        let run = required(&input, "queuedRunId")?;
        let mut command = json!({"threadId":target(&context,&input)?,"runId":run});
        command["type"] = json!(match action {
            "edit" => "queued-run.edit",
            "reorder" => "queued-run.reorder",
            "promote_to_steer" => "queued-message.promote-to-steer",
            _ => "queued-run.cancel",
        });
        match action {
            "edit" => command["text"] = required(&input, "text")?,
            "reorder" => command["beforeRunId"] = required(&input, "beforeRunId")?,
            "promote_to_steer" => {
                command
                    .as_object_mut()
                    .ok_or(McpError::Internal)?
                    .remove("runId");
                command["queuedRunId"] = run;
                command["targetRunId"] = required(&input, "targetRunId")?;
            }
            _ => {}
        }
        self.dispatch(&context, command).await
    }
}

#[async_trait]
impl ProjectService for OrchestrationAdapter {
    async fn t3_project_read(&self, _context: CallContext, input: DecodedInput) -> Result<Value> {
        let id: ProjectId = decoded(required(input.value(), "projectId")?)?;
        let project = self
            .database
            .read(move |db| t3_orchestration::stores::projects::get(db, &id, false))
            .await
            .map_err(failure)?
            .ok_or_else(|| McpError::failure("invalid_request", "The project was not found."))?;
        project_output(project)
    }
    async fn t3_project_list(&self, _context: CallContext, input: DecodedInput) -> Result<Value> {
        let projects = self
            .database
            .read(|db| t3_orchestration::stores::projects::list(db, None, false))
            .await
            .map_err(failure)?;
        let cursor = input.value()["cursor"].as_u64().unwrap_or(0) as usize;
        let limit = input.value()["limit"].as_u64().unwrap_or(20) as usize;
        let total = projects.len();
        let end = cursor.saturating_add(limit).min(total);
        Ok(
            json!({"projects":projects.into_iter().skip(cursor).take(limit).map(project_output).collect::<Result<Vec<_>>>()?,"nextCursor":if end<total {Some(end)} else {None}}),
        )
    }
}

fn project_output(row: t3_orchestration::stores::projects::ProjectRow) -> Result<Value> {
    let mut value = encoded(row)?;
    value["id"] = value["projectId"].take();
    value["repositoryIdentity"] = Value::Null;
    let project: t3_contracts::project::Project = decoded(value)?;
    encoded(project)
}
