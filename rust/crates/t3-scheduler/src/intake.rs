use crate::{DispatchReceipt, DispatchRequest, Error, Orchestration, Result};
use async_trait::async_trait;
use std::sync::Arc;
use t3_contracts::{
    base_schemas::{ProjectId, ThreadId},
    orchestration_v2::CommandMessageDispatch,
    provider_policy::{ProviderInteractionMode, RuntimeMode},
    scheduled_task::ScheduledTask,
};

pub const LAUNCH_UNAVAILABLE: &str =
    "Unbound scheduled tasks require a launch environment, which is not assembled.";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskRunMode {
    pub runtime_mode: RuntimeMode,
    pub interaction_mode: ProviderInteractionMode,
}
#[derive(Debug, Clone)]
pub struct BoundTaskThread {
    pub project_id: ProjectId,
    pub modes: TaskRunMode,
    pub archived: bool,
    pub deleted: bool,
}
#[derive(Debug, Clone)]
pub struct BoundTaskMessage {
    pub project_id: ProjectId,
    pub command: CommandMessageDispatch,
}
#[derive(Debug, thiserror::Error)]
pub enum IntakeError {
    #[error("The destination thread was not found in its project.")]
    ThreadNotFound,
    #[error(transparent)]
    Other(#[from] Error),
}
#[async_trait]
pub trait RuntimeTaskIntake: Send + Sync + 'static {
    async fn validate(&self, task: &ScheduledTask, destination_changed: bool) -> Result<()>;
    async fn thread(&self, id: &ThreadId) -> Result<Option<BoundTaskThread>>;
    async fn enqueue(
        &self,
        message: BoundTaskMessage,
    ) -> std::result::Result<DispatchReceipt, IntakeError>;
    async fn has_queued_occurrence(&self, task: &ScheduledTask) -> Result<bool>;
    async fn wait_for_failure(
        &self,
        receipt: DispatchReceipt,
        message_id: String,
    ) -> Result<Option<String>>;
    async fn consume_secret(&self, _reference: &str, _project: &ProjectId) -> Result<String> {
        Err(Error::Invalid(
            "Secret references require a secret-store adapter.".into(),
        ))
    }
}
pub struct IntakeDispatchBackend {
    intake: Arc<dyn RuntimeTaskIntake>,
}
impl IntakeDispatchBackend {
    pub fn new(intake: Arc<dyn RuntimeTaskIntake>) -> Self {
        Self { intake }
    }
}
#[async_trait]
impl Orchestration for IntakeDispatchBackend {
    async fn validate(&self, task: &ScheduledTask, destination_changed: bool) -> Result<()> {
        if task.thread_id.is_none() && task.enabled {
            return Err(Error::Invalid(LAUNCH_UNAVAILABLE.into()));
        }
        if let Some(id) = &task.thread_id
            && destination_changed
        {
            let thread =
                self.intake.thread(id).await?.ok_or_else(|| {
                    Error::Invalid("The destination thread was not found.".into())
                })?;
            if thread.deleted || thread.archived || thread.project_id != task.project_id {
                return Err(Error::Invalid(
                    "The destination thread is deleted, archived, or in another project.".into(),
                ));
            }
        }
        self.intake.validate(task, destination_changed).await
    }
    async fn dispatch(&self, request: DispatchRequest) -> Result<DispatchReceipt> {
        let Some(thread) = &request.task.thread_id else {
            return Err(Error::Dispatch(LAUNCH_UNAVAILABLE.into()));
        };
        let mut value = serde_json::json!({
            "commandId":request.command_id,"messageId":request.message_id,"threadId":thread,
            "text":request.prompt,"attachments":[],"dispatchMode":{"type":"queue_after_active"},
            "scheduledTaskId":request.task.id,"createdBy":request.task.created_by,"creationSource":request.task.creation_source
        });
        if request.fresh_context {
            value["freshContext"] = serde_json::json!(true);
        }
        let command = serde_json::from_value(value)
            .map_err(|_| Error::Dispatch("Invalid scheduled message command.".into()))?;
        let mut message = BoundTaskMessage {
            project_id: request.task.project_id,
            command,
        };
        for attempt in 0..=2 {
            match self.intake.enqueue(message.clone()).await {
                Ok(receipt) => return Ok(receipt),
                Err(IntakeError::Other(error)) => return Err(error),
                Err(IntakeError::ThreadNotFound) => {
                    if attempt == 2 {
                        break;
                    }
                    let Some(destination) = self.intake.thread(thread).await? else {
                        break;
                    };
                    if destination.deleted || destination.project_id == message.project_id {
                        break;
                    }
                    message.project_id = destination.project_id;
                }
            }
        }
        Err(Error::Dispatch(
            "The destination thread was not found in its project.".into(),
        ))
    }
    async fn bound_thread_modes(&self, id: &ThreadId) -> Result<Option<TaskRunMode>> {
        Ok(self
            .intake
            .thread(id)
            .await?
            .filter(|thread| !thread.deleted)
            .map(|thread| thread.modes))
    }
    async fn has_queued_occurrence(&self, task: &ScheduledTask) -> Result<bool> {
        self.intake.has_queued_occurrence(task).await
    }
    async fn wait_for_failure(
        &self,
        receipt: DispatchReceipt,
        message_id: String,
    ) -> Result<Option<String>> {
        self.intake.wait_for_failure(receipt, message_id).await
    }
    async fn thread_exists(&self, id: &ThreadId) -> Result<bool> {
        Ok(self
            .intake
            .thread(id)
            .await?
            .is_some_and(|thread| !thread.deleted))
    }
    async fn consume_secret(&self, reference: &str, project: &ProjectId) -> Result<String> {
        self.intake.consume_secret(reference, project).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{ScheduledTaskService, SchedulerOptions, TokioClock, Trigger};
    use serde_json::json;
    use std::{
        collections::VecDeque,
        sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
        },
    };
    use t3_contracts::{base_schemas::ScheduledTaskId, scheduled_task::*};
    struct Intake {
        threads: Mutex<VecDeque<Option<BoundTaskThread>>>,
        outcomes: Mutex<VecDeque<std::result::Result<DispatchReceipt, IntakeError>>>,
        sent: Mutex<Vec<BoundTaskMessage>>,
        queued: AtomicBool,
        fail_lookup: AtomicBool,
    }
    fn thread(project: &str) -> BoundTaskThread {
        BoundTaskThread {
            project_id: ProjectId(project.into()),
            modes: TaskRunMode {
                runtime_mode: RuntimeMode::FullAccess,
                interaction_mode: ProviderInteractionMode::Default,
            },
            archived: false,
            deleted: false,
        }
    }
    fn intake() -> Arc<Intake> {
        Arc::new(Intake {
            threads: Mutex::new([Some(thread("project"))].into()),
            outcomes: Mutex::default(),
            sent: Mutex::default(),
            queued: AtomicBool::new(false),
            fail_lookup: AtomicBool::new(false),
        })
    }
    #[async_trait]
    impl RuntimeTaskIntake for Intake {
        async fn validate(&self, _: &ScheduledTask, _: bool) -> Result<()> {
            Ok(())
        }
        async fn thread(&self, _: &ThreadId) -> Result<Option<BoundTaskThread>> {
            if self.fail_lookup.load(Ordering::Relaxed) {
                return Err(Error::Dispatch("lookup failed".into()));
            }
            let mut threads = self.threads.lock().unwrap();
            Ok(if threads.len() > 1 {
                threads.pop_front().flatten()
            } else {
                threads.front().cloned().flatten()
            })
        }
        async fn enqueue(
            &self,
            message: BoundTaskMessage,
        ) -> std::result::Result<DispatchReceipt, IntakeError> {
            self.sent.lock().unwrap().push(message);
            self.outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| {
                    Ok(DispatchReceipt {
                        thread_id: ThreadId("thread".into()),
                    })
                })
        }
        async fn has_queued_occurrence(&self, _: &ScheduledTask) -> Result<bool> {
            Ok(self.queued.load(Ordering::Relaxed))
        }
        async fn wait_for_failure(&self, _: DispatchReceipt, _: String) -> Result<Option<String>> {
            Ok(Some("provider failed".into()))
        }
    }
    fn task() -> ScheduledTask {
        serde_json::from_value(json!({"id":"task","title":"title","prompt":"prompt","enabled":true,"schedule":{"type":"interval","everyMs":60000},"projectId":"project","threadId":"thread","threadMode":"continue","workspaceStrategy":{"type":"root"},"modelSelection":{"instanceId":"stored-provider","model":"stored-model"},"runtimeMode":"approval-required","interactionMode":"plan","createdBy":"agent","creationSource":"mcp","createdAt":"2026-11-14T10:00:00Z","updatedAt":"2026-11-14T10:00:00Z","nextRunAt":null,"lastRunAt":null,"lastRunStatus":"never","lastRunError":null,"runCount":0})).unwrap()
    }
    fn request() -> DispatchRequest {
        DispatchRequest {
            task: task(),
            trigger: Trigger::Webhook,
            command_id: "stable-command".into(),
            message_id: "stable-message".into(),
            prompt: "rendered webhook".into(),
            fresh_context: true,
        }
    }
    #[tokio::test]
    async fn bound_dispatch_builds_a_queued_message_with_provenance_and_no_stored_selection() {
        let intake = intake();
        let backend = IntakeDispatchBackend::new(intake.clone());
        let receipt = backend.dispatch(request()).await.unwrap();
        assert_eq!(receipt.thread_id.0, "thread");
        let sent = intake.sent.lock().unwrap();
        let command = json!(sent[0].command);
        assert_eq!(command["commandId"], "stable-command");
        assert_eq!(command["messageId"], "stable-message");
        assert_eq!(command["scheduledTaskId"], "task");
        assert_eq!(command["text"], "rendered webhook");
        assert_eq!(command["createdBy"], "agent");
        assert_eq!(command["creationSource"], "mcp");
        assert_eq!(command["dispatchMode"]["type"], "queue_after_active");
        assert_eq!(command["freshContext"], true);
        assert_eq!(command["attachments"], json!([]));
        assert!(command.get("modelSelection").is_none());
    }
    #[tokio::test]
    async fn follows_two_project_moves_with_identical_command_and_message_ids() {
        let intake = intake();
        *intake.threads.lock().unwrap() =
            [Some(thread("moved")), Some(thread("moved-again"))].into();
        *intake.outcomes.lock().unwrap() = [
            Err(IntakeError::ThreadNotFound),
            Err(IntakeError::ThreadNotFound),
            Ok(DispatchReceipt {
                thread_id: ThreadId("thread".into()),
            }),
        ]
        .into();
        IntakeDispatchBackend::new(intake.clone())
            .dispatch(request())
            .await
            .unwrap();
        let sent = intake.sent.lock().unwrap();
        assert_eq!(
            sent.iter()
                .map(|s| s.project_id.0.as_str())
                .collect::<Vec<_>>(),
            ["project", "moved", "moved-again"]
        );
        assert!(
            sent.iter()
                .all(|s| json!(s.command) == json!(sent[0].command))
        );
    }
    #[tokio::test]
    async fn retries_only_moved_destinations_and_never_more_than_twice() {
        for moved in [false, true] {
            let intake = intake();
            *intake.threads.lock().unwrap() =
                [Some(thread("moved")), Some(thread("moved-again"))].into();
            *intake.outcomes.lock().unwrap() = if moved {
                [
                    Err(IntakeError::ThreadNotFound),
                    Err(IntakeError::ThreadNotFound),
                    Err(IntakeError::ThreadNotFound),
                ]
                .into()
            } else {
                [Err(IntakeError::Other(Error::Policy("denied".into())))].into()
            };
            assert!(
                IntakeDispatchBackend::new(intake.clone())
                    .dispatch(request())
                    .await
                    .is_err()
            );
            assert_eq!(intake.sent.lock().unwrap().len(), if moved { 3 } else { 1 });
        }
    }
    #[tokio::test]
    async fn task_modes_include_stored_and_live_bound_modes_and_fail_closed_on_lookup_errors() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fixture.sqlite");
        let database = tokio::task::spawn_blocking(move || t3_persistence::Database::open(path))
            .await
            .unwrap()
            .unwrap();
        let intake = intake();
        let backend = Arc::new(IntakeDispatchBackend::new(intake.clone()));
        let clock = Arc::new(TokioClock::new(
            crate::schedule::parse_date("2026-11-14T10:00:00Z").unwrap(),
        ));
        let service = ScheduledTaskService::new(
            database,
            clock,
            backend,
            SchedulerOptions {
                time_zone: chrono_tz::UTC,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let input:ScheduledTaskUpsertInput=serde_json::from_value(json!({"id":"task","title":"title","prompt":"prompt","enabled":true,"schedule":{"type":"interval","everyMs":60000},"projectId":"project","threadId":"thread","threadMode":"continue","workspaceStrategy":{"type":"root"},"modelSelection":{"instanceId":"stored-provider","model":"stored-model"},"runtimeMode":"approval-required","interactionMode":"plan"})).unwrap();
        service.upsert(input).await.unwrap();
        let id = ScheduledTaskId("task".into());
        assert_eq!(
            service.run_modes(&id).await.unwrap(),
            vec![
                TaskRunMode {
                    runtime_mode: RuntimeMode::ApprovalRequired,
                    interaction_mode: ProviderInteractionMode::Plan
                },
                thread("project").modes
            ]
        );
        intake.fail_lookup.store(true, Ordering::Relaxed);
        assert!(service.run_modes(&id).await.is_err());
        intake.fail_lookup.store(false, Ordering::Relaxed);
        intake.threads.lock().unwrap()[0] = Some(BoundTaskThread {
            archived: true,
            ..thread("project")
        });
        assert_eq!(service.run_modes(&id).await.unwrap().len(), 2);
        intake.threads.lock().unwrap()[0] = Some(BoundTaskThread {
            modes: TaskRunMode {
                runtime_mode: RuntimeMode::ApprovalRequired,
                interaction_mode: ProviderInteractionMode::Plan,
            },
            ..thread("project")
        });
        assert_eq!(service.run_modes(&id).await.unwrap().len(), 1);
        intake.threads.lock().unwrap()[0] = Some(BoundTaskThread {
            deleted: true,
            ..thread("project")
        });
        assert_eq!(service.run_modes(&id).await.unwrap().len(), 1);
        service.shutdown().await;
    }
    #[tokio::test]
    async fn unbound_tasks_can_be_saved_disabled_but_cannot_enable_or_launch() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fixture.sqlite");
        let database = tokio::task::spawn_blocking(move || t3_persistence::Database::open(path))
            .await
            .unwrap()
            .unwrap();
        let intake = intake();
        let service = ScheduledTaskService::new(
            database,
            Arc::new(TokioClock::new(
                crate::schedule::parse_date("2026-11-14T10:00:00Z").unwrap(),
            )),
            Arc::new(IntakeDispatchBackend::new(intake.clone())),
            SchedulerOptions {
                time_zone: chrono_tz::UTC,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let mut value = json!({"id":"unbound","title":"title","prompt":"prompt","enabled":true,"schedule":{"type":"interval","everyMs":60000},"projectId":"project","workspaceStrategy":{"type":"root"},"modelSelection":{"instanceId":"stored-provider","model":"stored-model"},"runtimeMode":"approval-required","interactionMode":"plan"});
        assert_eq!(
            service
                .upsert(serde_json::from_value(value.clone()).unwrap())
                .await
                .unwrap_err()
                .to_string(),
            LAUNCH_UNAVAILABLE
        );
        value["enabled"] = json!(false);
        let task = service
            .upsert(serde_json::from_value(value).unwrap())
            .await
            .unwrap();
        assert_eq!(
            service
                .set_enabled(ScheduledTaskSetEnabledInput {
                    id: task.id.clone(),
                    enabled: true
                })
                .await
                .unwrap_err()
                .to_string(),
            LAUNCH_UNAVAILABLE
        );
        assert!(!service.get(&task.id).await.unwrap().enabled);
        let result = service.run_now(&task.id).await.unwrap();
        assert!(result.dispatch_failed);
        assert_eq!(
            result.task.last_run_error.as_deref(),
            Some(LAUNCH_UNAVAILABLE)
        );
        assert!(result.thread_id.is_none());
        assert!(intake.sent.lock().unwrap().is_empty());
        service.shutdown().await;
    }
}
