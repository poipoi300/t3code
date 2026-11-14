use crate::{
    Capability, ClientAccess, InvocationScope, McpError, Modes, Result, scope::runtime_rank,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use t3_contracts::{
    base_schemas::ThreadId,
    provider_instance::ProviderInstanceId,
    provider_policy::{ProviderInteractionMode, RuntimeMode},
};

#[derive(Clone, Debug)]
pub struct LinkOrigin {
    pub session_id: String,
    pub label: String,
    pub access: ClientAccess,
}

#[derive(Clone, Debug)]
pub struct ThreadShell {
    pub id: ThreadId,
    pub modes: Modes,
    pub deleted: bool,
    pub archived: bool,
    pub active: bool,
    pub provider_instance_id: Option<ProviderInstanceId>,
    pub background_subagent_providers: Vec<ProviderInstanceId>,
    pub link_origin: Option<LinkOrigin>,
}

#[async_trait]
pub trait AccessStore: Send + Sync {
    async fn thread_shell(&self, id: &ThreadId) -> Result<Option<ThreadShell>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessRule {
    Reads,
    ReadsAsCaller,
    ActsAsCaller,
    Writes,
    WritesWithCapability,
    WritesThreads,
    StartsThreads,
    WritesEnvironment,
}

pub fn tool_rule(name: &str, input: &Value) -> AccessRule {
    use AccessRule::*;
    match name {
        "preview_status" | "preview_snapshot" | "preview_wait_for" | "device_list"
        | "device_screenshot" | "t3_preview_list" | "t3_worktree_status" | "html_preview"
        | "tool_search" => ReadsAsCaller,
        "delegate_task"
        | "task_status"
        | "task_cancel"
        | "t3_background_wait"
        | "request_secret"
        | "create_threads"
        | "send_to_user"
        | "t3_worktree_handoff"
        | "html_render"
        | "t3_preview_close"
        | "device_open"
        | "device_close" => ActsAsCaller,
        name if name.starts_with("preview_") => ActsAsCaller,
        "t3_environment_preferences_update"
        | "t3_project_create"
        | "t3_project_update"
        | "t3_project_delete"
        | "t3_project_clone"
        | "run_scheduled_task_now"
        | "t3_environment_link"
        | "t3_environment_unlink" => WritesEnvironment,
        "t3_theme_set" => WritesEnvironment,
        "t3_thread_launch" => StartsThreads,
        "schedule_task" if input.get("scheduledTaskId").is_none_or(Value::is_null) => StartsThreads,
        "schedule_task"
        | "delete_scheduled_task"
        | "t3_attachment_prepare_upload"
        | "t3_attachment_discard" => Writes,
        "write_pull_request_guides" => WritesWithCapability,
        "t3_queue" if matches!(input["action"].as_str(), Some("list" | "read")) => Reads,
        "t3_pending_request" if input["action"] != "respond" => Reads,
        "t3_thread_configure" if input.get("modelSelection").is_none_or(Value::is_null) => Reads,
        "t3_thread_transfer" if input["action"] == "status" => Reads,
        "t3_thread_update"
        | "t3_thread_send"
        | "t3_thread_interrupt"
        | "t3_workflow_stop"
        | "t3_thread_send_attachments"
        | "t3_thread_organize"
        | "t3_queue"
        | "t3_pending_request"
        | "t3_thread_configure"
        | "t3_thread_transfer"
        | "link_pull_request"
        | "link_pull_requests"
        | "unlink_pull_request"
        | "unlink_pull_requests"
        | "watch_pull_request"
        | "unwatch_pull_request" => WritesThreads,
        _ => Reads,
    }
}

pub fn check_modes(limits: Modes, target: Modes) -> Result<()> {
    if runtime_rank(target.runtime_mode) > runtime_rank(limits.runtime_mode) {
        return Err(McpError::failure(
            "runtime_mode_escalation_denied",
            format!(
                "Child runtime mode {} is broader than parent mode {}.",
                mode_text(target.runtime_mode),
                mode_text(limits.runtime_mode)
            ),
        ));
    }
    if target.interaction_mode == ProviderInteractionMode::Default
        && limits.interaction_mode == ProviderInteractionMode::Plan
    {
        return Err(McpError::failure(
            "interaction_mode_escalation_denied",
            "Child interaction mode default is broader than parent mode plan.",
        ));
    }
    Ok(())
}

fn mode_text(mode: RuntimeMode) -> &'static str {
    match mode {
        RuntimeMode::ApprovalRequired => "approval-required",
        RuntimeMode::AutoAcceptEdits => "auto-accept-edits",
        RuntimeMode::Auto => "auto",
        RuntimeMode::FullAccess => "full-access",
    }
}

#[derive(Clone)]
pub struct CallContext {
    pub scope: InvocationScope,
    pub limits: Modes,
    pub started_modes: Option<Modes>,
    pub link_origin: Option<LinkOrigin>,
    store: Arc<dyn AccessStore>,
    rule: AccessRule,
    tool: String,
    refusal: Arc<Mutex<Option<McpError>>>,
}

impl CallContext {
    /// Call under the service's command lock, before deciding or committing a write.
    pub fn check_dispatch(&self, target: &ThreadShell) -> Result<()> {
        let result = check_modes(self.limits, target.modes).and_then(|()| self.check_link(target));
        if let Err(error) = &result {
            let mapped = match error {
                McpError::Failure { code, .. } if code.ends_with("mode_escalation_denied") => {
                    McpError::failure(
                        code,
                        format!(
                            "Thread {} now runs in {}/{} mode, above this caller's. Its user changed it while this call ran.",
                            target.id.0,
                            mode_text(target.modes.runtime_mode),
                            if target.modes.interaction_mode == ProviderInteractionMode::Plan {
                                "plan"
                            } else {
                                "default"
                            }
                        ),
                    )
                }
                _ => error.clone(),
            };
            *self.refusal.lock().map_err(|_| McpError::Internal)? = Some(mapped.clone());
            return Err(mapped);
        }
        Ok(())
    }
    pub fn recorded_refusal(&self) -> Option<McpError> {
        self.refusal.lock().ok()?.clone()
    }
    /// Rechecks caller liveness and full access after waiting for an environment-write lock.
    pub async fn recheck_environment(&self) -> Result<()> {
        authorize(
            self.store.clone(),
            self.scope.clone(),
            self.rule,
            &self.tool,
            &serde_json::json!({}),
            false,
        )
        .await
        .map(|_| ())
    }
    fn check_link(&self, target: &ThreadShell) -> Result<()> {
        if let Some(origin) = self
            .link_origin
            .as_ref()
            .filter(|o| o.access != ClientAccess::FullAccess)
            && target
                .link_origin
                .as_ref()
                .is_none_or(|other| other.session_id != origin.session_id)
        {
            return Err(outside_link(
                origin,
                "it may change only threads that link started.",
            ));
        }
        Ok(())
    }
}

fn outside_link(origin: &LinkOrigin, message: &str) -> McpError {
    McpError::failure(
        "capability_denied",
        format!(
            "This work was started from {} through a linked environment, so {message} A link paired with full access is not limited this way.",
            origin.label
        ),
    )
}

pub async fn authorize(
    store: Arc<dyn AccessStore>,
    scope: InvocationScope,
    rule: AccessRule,
    tool: &str,
    input: &Value,
    remote: bool,
) -> Result<CallContext> {
    use AccessRule::*;
    if matches!(rule, ReadsAsCaller | ActsAsCaller) {
        if remote {
            return Err(McpError::failure(
                "invalid_request",
                "This action acts as the calling thread, which is here. Omit environmentId.",
            ));
        }
        scope.require_thread("This tool")?;
    }
    let writing = !matches!(rule, Reads | ReadsAsCaller);
    if writing
        && scope
            .client
            .as_ref()
            .is_some_and(|c| c.access == ClientAccess::ReadOnly)
    {
        return Err(McpError::failure(
            "capability_denied",
            "This tool changes the environment, and this MCP client was approved for read-only access.",
        ));
    }
    if matches!(rule, Writes | WritesEnvironment)
        && !scope.capabilities.contains(&Capability::Orchestration)
    {
        return Err(McpError::failure(
            "capability_denied",
            "This credential cannot control threads.",
        ));
    }
    let caller = if writing {
        if let Some(caller) = &scope.thread {
            let shell = store
                .thread_shell(&caller.thread_id)
                .await
                .map_err(|_| McpError::unavailable())?
                .filter(|s| !s.deleted)
                .ok_or_else(|| {
                    McpError::failure("thread_not_found", "The calling thread was not found.")
                })?;
            if shell.archived
                || shell.provider_instance_id.as_ref() != Some(&caller.provider_instance_id)
                || !(shell.active
                    || shell
                        .background_subagent_providers
                        .contains(&caller.provider_instance_id))
            {
                return Err(McpError::failure(
                    "parent_not_active",
                    "The calling provider no longer owns an active thread run.",
                ));
            }
            Some(shell)
        } else {
            None
        }
    } else {
        None
    };
    let limits = caller
        .as_ref()
        .map(|s| s.modes)
        .or_else(|| scope.client.as_ref().map(|c| c.ceiling()))
        .unwrap_or(Modes {
            runtime_mode: RuntimeMode::ApprovalRequired,
            interaction_mode: ProviderInteractionMode::Default,
        });
    let link_origin = caller
        .as_ref()
        .and_then(|s| s.link_origin.clone())
        .or_else(|| {
            scope
                .client
                .as_ref()
                .filter(|c| c.linked)
                .map(|c| LinkOrigin {
                    session_id: c.session_id.clone(),
                    label: c.label.clone(),
                    access: c.access,
                })
        });
    let context = CallContext {
        scope,
        limits,
        started_modes: None,
        link_origin,
        store,
        rule,
        tool: tool.into(),
        refusal: Arc::new(Mutex::new(None)),
    };
    let allow_link = matches!(
        tool,
        "t3_attachment_prepare_upload"
            | "t3_attachment_discard"
            | "t3_project_create"
            | "t3_project_clone"
    );
    if !remote
        && matches!(
            rule,
            Writes | WritesWithCapability | WritesEnvironment | StartsThreads
        )
        && !allow_link
        && tool != "t3_thread_launch"
        && let Some(origin) = context
            .link_origin
            .as_ref()
            .filter(|o| o.access != ClientAccess::FullAccess)
    {
        let what = match rule {
            WritesEnvironment => "change projects or environment settings",
            Writes => "change this environment's scheduled tasks",
            StartsThreads => "schedule tasks in this environment",
            _ => "start work here that is not its link's",
        };
        return Err(outside_link(origin, &format!("it cannot {what}.")));
    }
    if rule == WritesEnvironment && limits != Modes::full() {
        return Err(McpError::failure(
            "capability_denied",
            "Changing projects or environment settings needs a live full-access/default calling thread or a full-access client.",
        ));
    }
    if rule == WritesThreads && !remote {
        for key in ["threadId", "targetThreadId"] {
            if let Some(id) = input[key].as_str() {
                let id = ThreadId(id.into());
                if context
                    .scope
                    .thread
                    .as_ref()
                    .is_some_and(|t| t.thread_id == id)
                {
                    continue;
                }
                if let Some(target) = context
                    .store
                    .thread_shell(&id)
                    .await
                    .map_err(|_| McpError::unavailable())?
                    .filter(|s| !s.deleted)
                {
                    check_modes(limits, target.modes)?;
                    context.check_link(&target)?;
                }
            }
        }
    }
    if rule == StartsThreads {
        let mut context = context;
        let runtime = input
            .get("runtimeMode")
            .filter(|v| !v.is_null() && *v != "inherit")
            .map(|v| serde_json::from_value::<RuntimeMode>(v.clone()))
            .transpose()
            .map_err(|e| McpError::InvalidInput(e.to_string()))?
            .unwrap_or(limits.runtime_mode);
        let interaction = input
            .get("interactionMode")
            .filter(|v| !v.is_null() && *v != "inherit")
            .map(|v| serde_json::from_value::<ProviderInteractionMode>(v.clone()))
            .transpose()
            .map_err(|e| McpError::InvalidInput(e.to_string()))?
            .unwrap_or(limits.interaction_mode);
        let modes = Modes {
            runtime_mode: runtime,
            interaction_mode: interaction,
        };
        check_modes(limits, modes)?;
        context.started_modes = Some(modes);
        return Ok(context);
    }
    Ok(context)
}
