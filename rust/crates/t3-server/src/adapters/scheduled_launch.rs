//! Unbound scheduled tasks launch their thread (`ScheduledTaskService.ts`: `threadLaunch.launch`
//! when `threadId` is null). Bound tasks go to the scheduler's intake backend unchanged.
//!
//! A launch runs the task's stored model, modes and workspace strategy through the runtime's one
//! launch path: the managed ceiling refuses `thread.create` above it, and an agent-created task's
//! first message is limited to the task's modes. Saving or enabling an unbound task above the
//! ceiling is refused first, as TypeScript's `managedRuntimeModeDenial` does.
use std::sync::Arc;
use t3_contracts::base_schemas::{CommandId, MessageId};
use t3_orchestration::{
    composition::OrchestrationRuntime,
    error::Error as OrchestrationError,
    ports::{
        checkpoints_forks::thread_launch_service::{InitialMessage, LaunchInput},
        decisions::dispatch_mode_limit::{
            CeilingRefused, RuntimeModeCeiling, is_runtime_mode_above,
        },
    },
};
use t3_scheduler::{
    DispatchReceipt, DispatchRequest, Error, Orchestration, Result, intake::TaskRunMode,
};

/// `ThreadLaunchError.operation` in TypeScript; its message names the failing operation.
const LAUNCH_OPERATIONS: [&str; 10] = [
    "resolve-project",
    "read-receipt",
    "generate-metadata",
    "provision-worktree",
    "run-setup-script",
    "create-thread",
    "update-thread",
    "dispatch-message",
    "release-run",
    "fail-run",
];

pub struct LaunchingBackend {
    bound: Arc<dyn Orchestration>,
    runtime: Arc<OrchestrationRuntime>,
    ceiling: Option<RuntimeModeCeiling>,
}

impl LaunchingBackend {
    pub fn wrap(
        bound: Arc<dyn Orchestration>,
        runtime: Arc<OrchestrationRuntime>,
        managed_policy: Option<&t3_contracts::managed_policy::ManagedPolicyDocument>,
    ) -> Arc<dyn Orchestration> {
        Arc::new(Self {
            bound,
            runtime,
            ceiling: managed_policy.and_then(RuntimeModeCeiling::from_managed),
        })
    }
}

/// The run's recorded error, as TypeScript's `errorMessage` renders the launch failure.
fn launch_failure(command_id: &str, error: &OrchestrationError) -> Error {
    match error {
        OrchestrationError::Service { operation, .. } if LAUNCH_OPERATIONS.contains(operation) => {
            Error::Dispatch(format!(
                "Thread launch {command_id} failed during {operation}."
            ))
        }
        OrchestrationError::Service {
            operation: "ThreadManagementProviderInstanceDisabledError",
            detail,
        } => Error::Dispatch(detail.clone()),
        error => {
            tracing::warn!(%error, command_id, "scheduled thread launch failed");
            Error::Dispatch(format!("Thread launch {command_id} failed."))
        }
    }
}

#[async_trait::async_trait]
impl Orchestration for LaunchingBackend {
    async fn bound_thread_modes(
        &self,
        thread: &t3_contracts::base_schemas::ThreadId,
    ) -> Result<Option<TaskRunMode>> {
        self.bound.bound_thread_modes(thread).await
    }

    async fn validate(
        &self,
        task: &t3_contracts::scheduled_task::ScheduledTask,
        destination_changed: bool,
    ) -> Result<()> {
        if task.thread_id.is_some() {
            return self.bound.validate(task, destination_changed).await;
        }
        // A disabled task saves, so one stored above the ceiling can be turned off first.
        match &self.ceiling {
            Some(ceiling)
                if task.enabled
                    && is_runtime_mode_above(task.runtime_mode, ceiling.max_runtime_mode) =>
            {
                Err(Error::Invalid(ceiling.message(CeilingRefused::Mode)))
            }
            _ => Ok(()),
        }
    }

    async fn dispatch(&self, request: DispatchRequest) -> Result<DispatchReceipt> {
        if request.task.thread_id.is_some() {
            return self.bound.dispatch(request).await;
        }
        let task = request.task;
        let launched = self
            .runtime
            .launch_thread(LaunchInput {
                command_id: CommandId(request.command_id.clone()),
                thread_id: None,
                reuse_existing_thread: false,
                project_id: task.project_id,
                title: task.title.0,
                generate_title: false,
                model_selection: task.model_selection,
                runtime_mode: task.runtime_mode,
                interaction_mode: task.interaction_mode,
                workspace_strategy: task.workspace_strategy,
                initial_message: Some(InitialMessage {
                    message_id: Some(MessageId(request.message_id)),
                    scheduled_task_id: Some(task.id),
                    sender_thread_id: None,
                    text: request.prompt,
                    attachments: Vec::new(),
                    context: None,
                }),
                imported_native_thread: None,
                created_by: task.created_by,
                creation_source: task.creation_source,
                link_origin: None,
                delegated_from: None,
            })
            .await
            .map_err(|error| launch_failure(&request.command_id, &error))?;
        Ok(DispatchReceipt {
            thread_id: launched.thread_id,
        })
    }

    async fn has_queued_occurrence(
        &self,
        task: &t3_contracts::scheduled_task::ScheduledTask,
    ) -> Result<bool> {
        self.bound.has_queued_occurrence(task).await
    }

    async fn wait_for_failure(
        &self,
        receipt: DispatchReceipt,
        message_id: String,
    ) -> Result<Option<String>> {
        self.bound.wait_for_failure(receipt, message_id).await
    }

    async fn thread_exists(&self, thread: &t3_contracts::base_schemas::ThreadId) -> Result<bool> {
        self.bound.thread_exists(thread).await
    }

    async fn consume_secret(
        &self,
        reference: &str,
        project: &t3_contracts::base_schemas::ProjectId,
    ) -> Result<String> {
        self.bound.consume_secret(reference, project).await
    }
}
