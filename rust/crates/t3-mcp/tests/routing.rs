#![allow(clippy::unwrap_used)]
// Generated routing coverage over real TypeScript codec witnesses.
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use t3_mcp::{services::*, *};
struct Recorder(Arc<Mutex<Vec<String>>>);
#[async_trait]
impl AttachmentService for Recorder {
    async fn t3_attachment_prepare_upload(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<t3_contracts::assets::AttachmentCreateUploadUrlResult> {
        self.0
            .lock()
            .unwrap()
            .push("t3_attachment_prepare_upload".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_attachment_discard(
        &self,
        _context: CallContext,
        _input: t3_contracts::assets::AttachmentDeleteInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("t3_attachment_discard".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_thread_send_attachments(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0
            .lock()
            .unwrap()
            .push("t3_thread_send_attachments".into());
        Err(McpError::failure("recorded", "recorded"))
    }
}
#[async_trait]
impl DeviceService for Recorder {
    async fn device_list(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<t3_contracts::device::DeviceToolListResult> {
        self.0.lock().unwrap().push("device_list".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn device_open(
        &self,
        _context: CallContext,
        _input: t3_contracts::device::DeviceToolOpenInput,
    ) -> Result<t3_contracts::device::DeviceToolOpenResult> {
        self.0.lock().unwrap().push("device_open".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn device_screenshot(
        &self,
        _context: CallContext,
        _input: t3_contracts::device::DeviceToolTargetInput,
    ) -> Result<t3_contracts::device::DeviceToolScreenshotResult> {
        self.0.lock().unwrap().push("device_screenshot".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn device_close(
        &self,
        _context: CallContext,
        _input: t3_contracts::device::DeviceToolCloseInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("device_close".into());
        Err(McpError::failure("recorded", "recorded"))
    }
}
#[async_trait]
impl EnvironmentService for Recorder {
    async fn t3_environment_read(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("t3_environment_read".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_environment_preferences_update(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0
            .lock()
            .unwrap()
            .push("t3_environment_preferences_update".into());
        Err(McpError::failure("recorded", "recorded"))
    }
}
#[async_trait]
impl HtmlService for Recorder {
    async fn html_preview(&self, _context: CallContext, _input: DecodedInput) -> Result<Value> {
        self.0.lock().unwrap().push("html_preview".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn html_render(&self, _context: CallContext, _input: DecodedInput) -> Result<Value> {
        self.0.lock().unwrap().push("html_render".into());
        Err(McpError::failure("recorded", "recorded"))
    }
}
#[async_trait]
impl OrchestratorService for Recorder {
    async fn orchestrator_capabilities(
        &self,
        _context: CallContext,
        _input: t3_contracts::orchestrator_mcp::OrchestratorMcpCapabilitiesInput,
    ) -> Result<t3_contracts::orchestrator_mcp::OrchestratorMcpCapabilitiesResult> {
        self.0
            .lock()
            .unwrap()
            .push("orchestrator_capabilities".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn delegate_task(
        &self,
        _context: CallContext,
        _input: t3_contracts::orchestrator_mcp::OrchestratorMcpDelegateTaskInput,
    ) -> Result<t3_contracts::orchestrator_mcp::OrchestratorMcpDelegateTaskResult> {
        self.0.lock().unwrap().push("delegate_task".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn task_status(
        &self,
        _context: CallContext,
        _input: t3_contracts::orchestrator_mcp::OrchestratorMcpTaskStatusInput,
    ) -> Result<t3_contracts::orchestrator_mcp::OrchestratorMcpDelegateTaskResult> {
        self.0.lock().unwrap().push("task_status".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn task_cancel(
        &self,
        _context: CallContext,
        _input: t3_contracts::orchestrator_mcp::OrchestratorMcpTaskCancelInput,
    ) -> Result<t3_contracts::orchestrator_mcp::OrchestratorMcpTaskCancelResult> {
        self.0.lock().unwrap().push("task_cancel".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_workflow_stop(&self, _context: CallContext, _input: DecodedInput) -> Result<Value> {
        self.0.lock().unwrap().push("t3_workflow_stop".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_background_wait(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("t3_background_wait".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn schedule_task(
        &self,
        _context: CallContext,
        _input: t3_contracts::orchestrator_mcp::OrchestratorMcpSaveScheduledTaskInput,
    ) -> Result<t3_contracts::orchestrator_mcp::OrchestratorMcpScheduleTaskResult> {
        self.0.lock().unwrap().push("schedule_task".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn list_scheduled_tasks(
        &self,
        _context: CallContext,
        _input: t3_contracts::orchestrator_mcp::OrchestratorMcpListScheduledTasksInput,
    ) -> Result<t3_contracts::orchestrator_mcp::OrchestratorMcpListScheduledTasksResult> {
        self.0.lock().unwrap().push("list_scheduled_tasks".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn delete_scheduled_task(
        &self,
        _context: CallContext,
        _input: t3_contracts::orchestrator_mcp::OrchestratorMcpDeleteScheduledTaskInput,
    ) -> Result<t3_contracts::orchestrator_mcp::OrchestratorMcpDeleteScheduledTaskResult> {
        self.0.lock().unwrap().push("delete_scheduled_task".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn request_secret(
        &self,
        _context: CallContext,
        _input: t3_contracts::orchestrator_mcp::OrchestratorMcpRequestSecretInput,
    ) -> Result<t3_contracts::orchestrator_mcp::OrchestratorMcpRequestSecretResult> {
        self.0.lock().unwrap().push("request_secret".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn create_threads(
        &self,
        _context: CallContext,
        _input: t3_contracts::orchestrator_mcp::OrchestratorMcpCreateThreadsInput,
    ) -> Result<t3_contracts::orchestrator_mcp::OrchestratorMcpCreateThreadsResult> {
        self.0.lock().unwrap().push("create_threads".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_thread_list(
        &self,
        _context: CallContext,
        _input: t3_contracts::orchestrator_mcp::OrchestratorMcpThreadListInput,
    ) -> Result<t3_contracts::orchestrator_mcp::OrchestratorMcpThreadListResult> {
        self.0.lock().unwrap().push("t3_thread_list".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_thread_read(
        &self,
        _context: CallContext,
        _input: t3_contracts::orchestrator_mcp::OrchestratorMcpThreadReadInput,
    ) -> Result<t3_contracts::orchestrator_mcp::OrchestratorMcpThreadReadResult> {
        self.0.lock().unwrap().push("t3_thread_read".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_thread_update(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<t3_contracts::thread_metadata_mcp::ThreadMetadataMcpUpdateResult> {
        self.0.lock().unwrap().push("t3_thread_update".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_thread_send(
        &self,
        _context: CallContext,
        _input: t3_contracts::orchestrator_mcp::OrchestratorMcpThreadSendInput,
    ) -> Result<t3_contracts::orchestrator_mcp::OrchestratorMcpThreadSendResult> {
        self.0.lock().unwrap().push("t3_thread_send".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_thread_wait(
        &self,
        _context: CallContext,
        _input: t3_contracts::orchestrator_mcp::OrchestratorMcpThreadWaitInput,
    ) -> Result<t3_contracts::orchestrator_mcp::OrchestratorMcpThreadWaitResult> {
        self.0.lock().unwrap().push("t3_thread_wait".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_thread_interrupt(
        &self,
        _context: CallContext,
        _input: t3_contracts::orchestrator_mcp::OrchestratorMcpThreadInterruptInput,
    ) -> Result<t3_contracts::orchestrator_mcp::OrchestratorMcpThreadInterruptResult> {
        self.0.lock().unwrap().push("t3_thread_interrupt".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_environment_links(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<t3_contracts::orchestrator_mcp::OrchestratorMcpEnvironmentLinksResult> {
        self.0.lock().unwrap().push("t3_environment_links".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_environment_link(
        &self,
        _context: CallContext,
        _input: t3_contracts::orchestrator_mcp::OrchestratorMcpEnvironmentLinkInput,
    ) -> Result<t3_contracts::orchestrator_mcp::OrchestratorMcpEnvironmentLinkResult> {
        self.0.lock().unwrap().push("t3_environment_link".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_environment_unlink(
        &self,
        _context: CallContext,
        _input: t3_contracts::orchestrator_mcp::OrchestratorMcpEnvironmentUnlinkInput,
    ) -> Result<t3_contracts::orchestrator_mcp::OrchestratorMcpEnvironmentUnlinkResult> {
        self.0.lock().unwrap().push("t3_environment_unlink".into());
        Err(McpError::failure("recorded", "recorded"))
    }
}
#[async_trait]
impl PreviewService for Recorder {
    async fn preview_dialog(
        &self,
        _context: CallContext,
        _input: t3_contracts::preview_automation::PreviewAutomationDialogInput,
    ) -> Result<t3_contracts::preview_automation::PreviewAutomationStatus> {
        self.0.lock().unwrap().push("preview_dialog".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn preview_status(
        &self,
        _context: CallContext,
        _input: t3_contracts::preview_automation::PreviewAutomationTabTargetInput,
    ) -> Result<t3_contracts::preview_automation::PreviewAutomationStatus> {
        self.0.lock().unwrap().push("preview_status".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn preview_open(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<t3_contracts::preview_automation::PreviewAutomationStatus> {
        self.0.lock().unwrap().push("preview_open".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn preview_navigate(
        &self,
        _context: CallContext,
        _input: t3_contracts::preview_automation::PreviewAutomationNavigateInput,
    ) -> Result<t3_contracts::preview_automation::PreviewAutomationStatus> {
        self.0.lock().unwrap().push("preview_navigate".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn preview_resize(
        &self,
        _context: CallContext,
        _input: t3_contracts::preview_automation::PreviewAutomationResizeInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("preview_resize".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn preview_set_appearance(
        &self,
        _context: CallContext,
        _input: t3_contracts::preview_automation::PreviewAutomationSetColorSchemeInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("preview_set_appearance".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn preview_snapshot(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<t3_contracts::preview_automation::PreviewAutomationSnapshot> {
        self.0.lock().unwrap().push("preview_snapshot".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn preview_act(
        &self,
        _context: CallContext,
        _input: t3_contracts::preview_automation::PreviewAutomationActInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("preview_act".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn preview_evaluate(
        &self,
        _context: CallContext,
        _input: t3_contracts::preview_automation::PreviewAutomationEvaluateInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("preview_evaluate".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn preview_wait_for(
        &self,
        _context: CallContext,
        _input: t3_contracts::preview_automation::PreviewAutomationWaitForInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("preview_wait_for".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn preview_recording_start(
        &self,
        _context: CallContext,
        _input: t3_contracts::preview_automation::PreviewAutomationTabTargetInput,
    ) -> Result<Value> {
        self.0
            .lock()
            .unwrap()
            .push("preview_recording_start".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn preview_recording_stop(
        &self,
        _context: CallContext,
        _input: t3_contracts::preview_automation::PreviewAutomationTabTargetInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("preview_recording_stop".into());
        Err(McpError::failure("recorded", "recorded"))
    }
}
#[async_trait]
impl PreviewControlsService for Recorder {
    async fn t3_preview_list(&self, _context: CallContext, _input: DecodedInput) -> Result<Value> {
        self.0.lock().unwrap().push("t3_preview_list".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_preview_close(&self, _context: CallContext, _input: DecodedInput) -> Result<Value> {
        self.0.lock().unwrap().push("t3_preview_close".into());
        Err(McpError::failure("recorded", "recorded"))
    }
}
#[async_trait]
impl ProjectService for Recorder {
    async fn t3_thread_launch(&self, _context: CallContext, _input: DecodedInput) -> Result<Value> {
        self.0.lock().unwrap().push("t3_thread_launch".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_project_list(&self, _context: CallContext, _input: DecodedInput) -> Result<Value> {
        self.0.lock().unwrap().push("t3_project_list".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_project_read(&self, _context: CallContext, _input: DecodedInput) -> Result<Value> {
        self.0.lock().unwrap().push("t3_project_read".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_project_create(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("t3_project_create".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_project_update(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("t3_project_update".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_project_delete(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("t3_project_delete".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_project_clone(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<t3_contracts::source_control::SourceControlCloneRepositoryResult> {
        self.0.lock().unwrap().push("t3_project_clone".into());
        Err(McpError::failure("recorded", "recorded"))
    }
}
#[async_trait]
impl PullRequestsService for Recorder {
    async fn link_pull_request(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("link_pull_request".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn link_pull_requests(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("link_pull_requests".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn unlink_pull_request(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("unlink_pull_request".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn unlink_pull_requests(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("unlink_pull_requests".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn list_thread_pull_requests(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0
            .lock()
            .unwrap()
            .push("list_thread_pull_requests".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn watch_pull_request(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("watch_pull_request".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn unwatch_pull_request(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("unwatch_pull_request".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn write_pull_request_guides(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0
            .lock()
            .unwrap()
            .push("write_pull_request_guides".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn read_pull_request_guide(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0
            .lock()
            .unwrap()
            .push("read_pull_request_guide".into());
        Err(McpError::failure("recorded", "recorded"))
    }
}
#[async_trait]
impl ThreadService for Recorder {
    async fn run_scheduled_task_now(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("run_scheduled_task_now".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_thread_search(&self, _context: CallContext, _input: DecodedInput) -> Result<Value> {
        self.0.lock().unwrap().push("t3_thread_search".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_thread_configure(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("t3_thread_configure".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_thread_transfer(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("t3_thread_transfer".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_thread_organize(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("t3_thread_organize".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_pending_request(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<Value> {
        self.0.lock().unwrap().push("t3_pending_request".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_queue(&self, _context: CallContext, _input: DecodedInput) -> Result<Value> {
        self.0.lock().unwrap().push("t3_queue".into());
        Err(McpError::failure("recorded", "recorded"))
    }
}
#[async_trait]
impl ToolSearchService for Recorder {
    async fn tool_search(&self, _context: CallContext, _input: DecodedInput) -> Result<Value> {
        self.0.lock().unwrap().push("tool_search".into());
        Err(McpError::failure("recorded", "recorded"))
    }
}
#[async_trait]
impl WorktreeService for Recorder {
    async fn t3_worktree_handoff(
        &self,
        _context: CallContext,
        _input: t3_contracts::worktree_mcp::WorktreeMcpHandoffInput,
    ) -> Result<t3_contracts::worktree_mcp::WorktreeMcpHandoffResult> {
        self.0.lock().unwrap().push("t3_worktree_handoff".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_worktree_status(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<t3_contracts::worktree_mcp::WorktreeMcpStatusResult> {
        self.0.lock().unwrap().push("t3_worktree_status".into());
        Err(McpError::failure("recorded", "recorded"))
    }
    async fn t3_worktree_list(
        &self,
        _context: CallContext,
        _input: DecodedInput,
    ) -> Result<t3_contracts::git::VcsListRefsResult> {
        self.0.lock().unwrap().push("t3_worktree_list".into());
        Err(McpError::failure("recorded", "recorded"))
    }
}
struct Store;
#[async_trait]
impl AccessStore for Store {
    async fn thread_shell(
        &self,
        _id: &t3_contracts::base_schemas::ThreadId,
    ) -> Result<Option<ThreadShell>> {
        Ok(None)
    }
}
#[tokio::test]
async fn every_tool_decodes_and_dispatches_to_its_own_service_method_once() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let services = Services {
        attachment: Arc::new(Recorder(calls.clone())),
        device: Arc::new(Recorder(calls.clone())),
        environment: Arc::new(Recorder(calls.clone())),
        html: Arc::new(Recorder(calls.clone())),
        orchestrator: Arc::new(Recorder(calls.clone())),
        preview: Arc::new(Recorder(calls.clone())),
        preview_controls: Arc::new(Recorder(calls.clone())),
        project: Arc::new(Recorder(calls.clone())),
        pull_requests: Arc::new(Recorder(calls.clone())),
        thread: Arc::new(Recorder(calls.clone())),
        tool_search: Arc::new(Recorder(calls.clone())),
        worktree: Arc::new(Recorder(calls.clone())),
    };
    let scope:InvocationScope=serde_json::from_value(json!({"environmentId":"environment","requestNamespace":"fixture","issuedAt":0,"capabilities":["orchestration"],"client":{"sessionId":"fixture","label":"fixture","access":"full-access"}})).unwrap();
    let context = access::authorize(
        Arc::new(Store),
        scope,
        AccessRule::Reads,
        "probe",
        &json!({}),
        false,
    )
    .await
    .unwrap();
    let witnesses: Value = serde_json::from_str(include_str!("../fixtures/decoders.json")).unwrap();
    for (name, cases) in witnesses.as_object().unwrap() {
        let case = &cases[0];
        if case["invalidAction"] == true {
            continue;
        }
        let input = decode_input(name, case["input"].clone())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let before = calls.lock().unwrap().len();
        let error = services
            .dispatch(name, context.clone(), input)
            .await
            .unwrap_err();
        assert!(
            matches!(error,McpError::Failure {ref code,..} if code=="recorded"),
            "{name}: {error}"
        );
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), before + 1, "{name}");
        assert_eq!(calls.last().unwrap(), name);
    }
}
