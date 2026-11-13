use super::{decoded, encoded, not_available};
use crate::{
    services::{OrchestratorService, ThreadService},
    *,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use t3_contracts::{
    base_schemas::ScheduledTaskId, orchestrator_mcp::*, scheduled_task::ScheduledTask,
};

pub struct SchedulerAdapter {
    pub service: Arc<t3_scheduler::ScheduledTaskService>,
    #[cfg(feature = "orchestration")]
    pub reader: Option<Arc<dyn t3_orchestration::api::ProjectionReader>>,
}
fn failure(error: t3_scheduler::Error) -> McpError {
    let code = match error {
        t3_scheduler::Error::NotFound(_) => "task_not_found",
        t3_scheduler::Error::Invalid(_) | t3_scheduler::Error::Policy(_) => "invalid_request",
        _ => "orchestration_error",
    };
    McpError::failure(code, error.to_string())
}
fn summary(task: ScheduledTask, may_run: bool) -> Result<OrchestratorMcpScheduledTask> {
    let mut task = encoded(task)?;
    task["scheduledTaskId"] = task["id"].clone();
    task["boundThreadId"] = task["threadId"].clone();
    task["freshSession"] = json!(task["threadMode"] == "fresh_session");
    if may_run && task.get("blockedByPolicy").is_none() && task["webhook"]["url"].is_string() {
        task["webhookUrl"] = task["webhook"]["url"].clone();
    }
    if !task["webhook"].is_null() {
        task["webhookSignature"] = json!(if task["webhook"]["hasSecret"] == true {
            "set"
        } else {
            "none"
        });
    }
    decoded(task)
}
impl SchedulerAdapter {
    async fn check_task(&self, context: &CallContext, task: &ScheduledTask) -> Result<()> {
        crate::access::check_modes(
            context.limits,
            Modes {
                runtime_mode: task.runtime_mode,
                interaction_mode: task.interaction_mode,
            },
        )?;
        if let Some(id) = &task.thread_id {
            #[cfg(feature = "orchestration")]
            {
                let reader = self.reader.as_ref().ok_or_else(|| {
                    not_available(
                        "schedule_task",
                        "ProjectionReader::shell (bound task modes)",
                    )
                })?;
                if let Some(shell) = reader
                    .shell(id.clone())
                    .await
                    .map_err(|_| McpError::unavailable())?
                    .filter(|t| t.deleted_at.is_none())
                {
                    crate::access::check_modes(
                        context.limits,
                        Modes {
                            runtime_mode: shell.runtime_mode,
                            interaction_mode: shell.interaction_mode,
                        },
                    )?;
                }
            }
            #[cfg(not(feature = "orchestration"))]
            {
                let _ = id;
                return Err(not_available(
                    "schedule_task",
                    "ProjectionReader::shell (enable orchestration)",
                ));
            }
        }
        Ok(())
    }
    async fn loaded(&self, context: &CallContext, id: &ScheduledTaskId) -> Result<ScheduledTask> {
        let task = self.service.get(id).await.map_err(failure)?;
        self.check_task(context, &task).await?;
        Ok(task)
    }
}
#[async_trait]
impl OrchestratorService for SchedulerAdapter {
    async fn list_scheduled_tasks(
        &self,
        context: CallContext,
        input: OrchestratorMcpListScheduledTasksInput,
    ) -> Result<OrchestratorMcpListScheduledTasksResult> {
        let input = encoded(input)?;
        let project_id = input.get("projectId").filter(|v| !v.is_null()).cloned();
        let project_id = if project_id.is_none()
            && let Some(caller) = &context.scope.thread
        {
            #[cfg(feature = "orchestration")]
            {
                let reader = self.reader.as_ref().ok_or_else(|| {
                    not_available(
                        "list_scheduled_tasks",
                        "ProjectionReader::shell (caller project)",
                    )
                })?;
                let shell = reader
                    .shell(caller.thread_id.clone())
                    .await
                    .map_err(|_| McpError::unavailable())?
                    .filter(|shell| shell.deleted_at.is_none())
                    .ok_or_else(|| {
                        McpError::failure("thread_not_found", "The calling thread was not found.")
                    })?;
                Some(encoded(shell.project_id)?)
            }
            #[cfg(not(feature = "orchestration"))]
            {
                let _ = caller;
                return Err(not_available(
                    "list_scheduled_tasks",
                    "ProjectionReader::shell (enable orchestration for caller project)",
                ));
            }
        } else {
            project_id
        };
        let mut tasks = Vec::new();
        for task in self.service.list().await.map_err(failure)?.tasks {
            if project_id
                .as_ref()
                .is_some_and(|id| id != &json!(task.project_id))
            {
                continue;
            }
            let may_run = self.check_task(&context, &task).await.is_ok()
                && context
                    .scope
                    .client
                    .as_ref()
                    .is_none_or(|c| c.access != ClientAccess::ReadOnly);
            tasks.push(summary(task, may_run)?);
        }
        Ok(OrchestratorMcpListScheduledTasksResult { tasks })
    }
    async fn delete_scheduled_task(
        &self,
        context: CallContext,
        input: OrchestratorMcpDeleteScheduledTaskInput,
    ) -> Result<OrchestratorMcpDeleteScheduledTaskResult> {
        let input = encoded(input)?;
        let id: ScheduledTaskId = decoded(input["scheduledTaskId"].clone())?;
        self.loaded(&context, &id).await?;
        self.service.delete(&id).await.map_err(failure)?;
        Ok(OrchestratorMcpDeleteScheduledTaskResult {
            scheduled_task_id: id,
            deleted: true,
        })
    }
    async fn schedule_task(
        &self,
        context: CallContext,
        input: OrchestratorMcpSaveScheduledTaskInput,
    ) -> Result<OrchestratorMcpScheduleTaskResult> {
        let input = encoded(input)?;
        let Some(id) = input.get("scheduledTaskId") else {
            return Err(not_available(
                "schedule_task",
                "OrchestratorMcpService::schedule_task (project defaults and Git base/workspace selection)",
            ));
        };
        let existing = self.loaded(&context, &decoded(id.clone())?).await?;
        if input.get("bindToCurrentThread").is_some() || input.get("freshSession").is_some() {
            return Err(not_available(
                "schedule_task",
                "OrchestratorMcpService::scheduled_task_workspace_strategy (binding transitions)",
            ));
        }
        let mut upsert = encoded(existing)?;
        upsert["requireExisting"] = json!(true);
        for key in ["prompt", "title", "enabled", "schedule"] {
            if let Some(value) = input.get(key) {
                upsert[key] = value.clone();
            }
        }
        let task = self
            .service
            .upsert(decoded(upsert)?)
            .await
            .map_err(failure)?;
        summary(task, true)
    }
}
#[async_trait]
impl ThreadService for SchedulerAdapter {
    async fn run_scheduled_task_now(
        &self,
        context: CallContext,
        input: DecodedInput,
    ) -> Result<Value> {
        let id: ScheduledTaskId = decoded(input.value()["taskId"].clone())?;
        self.loaded(&context, &id).await?;
        let result = self.service.run_now(&id).await.map_err(failure)?;
        let task = result.task;
        Ok(
            json!({"taskId":task.id,"threadId":result.thread_id,"lastRunStatus":task.last_run_status,"runCount":task.run_count,"nextRunAt":task.next_run_at}),
        )
    }
}
