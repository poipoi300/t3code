//! Delegated tasks: `delegateTask`, `taskStatus`, `cancelTask` and `readTask` from
//! `apps/server/src/mcp/OrchestratorMcpService.ts`, over the caller's commands
//! (`CallerCommandSource`), so the runtime checks the caller's authority under the command lock.
//!
//! Authority, as TypeScript: only a live thread caller delegates (its active run, owned by this
//! provider session); the child's runtime and interaction modes are never broader than the
//! parent's (`resolveRuntimeMode`, `resolveInteractionMode`, refused again by the runtime as
//! `CallerRefused::Escalation`); the runtime caps them at the managed ceiling; a fenced link may
//! not start work outside its link (runtime `OutsideLink`); a task is read and acknowledged only by
//! the thread that owns it, and its result is acknowledged once (`completion-delivery.acknowledge`
//! with a stable command id).
//!
//! Not served yet, answered as `available: false` with the missing method: `mode: "wait"` (needs
//! the stored-event stream), a task whose finished result a restart may still hold
//! (`delegatedTaskResultPending`), cancellation (`cancelDelegatedTask`), remote targets
//! (`RemoteDelegation`) and `isolation: "worktree"`.
use super::{decoded, encoded, not_available, orchestration::OrchestrationAdapter};
use crate::{services::OrchestratorService, *};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};
use t3_contracts::{
    base_schemas::{NodeId, ProjectId, ThreadId},
    orchestration_v2::{OrchestrationV2Run, OrchestrationV2RunStatus, OrchestrationV2ThreadProjection},
    orchestrator_mcp::*,
};
use t3_orchestration::{
    api::*,
    ports::{
        decisions::orchestrator::{delegated_task_progress, derived_id},
        runs_ingestion::restart_background_note::{
            delegated_task_awaits_work, delegated_task_result_run_from, delegated_task_runs,
            run_outcome, run_ran_after,
        },
        thread_project::thread_management_service::{is_active_run, is_terminal_run},
    },
};

/// What delegation reads from the host: provider status snapshots (TS `ProviderRegistry`), the
/// instances with a turn adapter (TS `ProviderAdapterRegistryV2.list`) and per-project enablement.
#[async_trait]
pub trait DelegationHost: Send + Sync {
    /// `ServerProvider` snapshots, as JSON.
    async fn providers(&self) -> Result<Vec<Value>>;
    async fn orchestration_instances(&self) -> Result<BTreeSet<String>>;
    async fn enabled_for_project(&self, project: &ProjectId, instance: &str) -> Result<bool>;
}

pub struct DelegationAdapter {
    pub orchestration: Arc<OrchestrationAdapter>,
    pub host: Arc<dyn DelegationHost>,
}

fn failure(code: &str, message: impl Into<String>) -> McpError {
    McpError::failure(code, message)
}

fn management_failure(error: t3_orchestration::error::Error) -> McpError {
    match error {
        t3_orchestration::error::Error::ThreadNotFound(_) => {
            failure("thread_not_found", error.to_string())
        }
        t3_orchestration::error::Error::CallerRefused { refusal, .. } => {
            failure(refusal.code(), refusal.to_string())
        }
        error => failure("orchestration_error", error.to_string()),
    }
}

/// `encodeURIComponent`.
fn stable_part(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// `stableCommandId`.
fn stable_command_id(scope: &InvocationScope, key: &str, operation: &str) -> String {
    format!(
        "command:mcp:{}:{}:{}",
        stable_part(&scope.request_namespace),
        stable_part(operation),
        stable_part(key)
    )
}

fn request_key(input: &Value) -> String {
    input["clientRequestId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}

fn runtime_rank(mode: &str) -> u8 {
    match mode {
        "approval-required" => 0,
        "auto-accept-edits" => 1,
        "auto" => 2,
        _ => 3,
    }
}

/// `resolveRuntimeMode`: the mode asked for, never broader than the parent's; an inherited mode
/// the provider does not offer steps down, an explicit one is refused.
pub fn resolve_runtime_mode(
    parent: &str,
    requested: Option<&str>,
    target: Option<(&str, &[String])>,
) -> Result<String> {
    let inherited = requested.is_none_or(|mode| mode == "inherit");
    let resolved = if inherited { parent } else { requested.unwrap_or(parent) };
    if runtime_rank(resolved) > runtime_rank(parent) {
        return Err(failure(
            "runtime_mode_escalation_denied",
            format!("Child runtime mode {resolved} is broader than parent mode {parent}."),
        ));
    }
    let Some((instance, supported)) = target.filter(|(_, s)| !s.is_empty()) else {
        return Ok(resolved.into());
    };
    if supported.iter().any(|mode| mode == resolved) {
        return Ok(resolved.into());
    }
    if !inherited {
        return Err(failure(
            "invalid_request",
            format!(
                "Provider {instance} does not offer runtime mode {resolved}, so it would run as approval-required. Use one of: {}.",
                supported.join(", ")
            ),
        ));
    }
    Ok(supported
        .iter()
        .filter(|mode| runtime_rank(mode) <= runtime_rank(resolved))
        .max_by_key(|mode| runtime_rank(mode))
        .cloned()
        .unwrap_or_else(|| "approval-required".into()))
}

/// `resolveInteractionMode`.
pub fn resolve_interaction_mode(parent: &str, requested: Option<&str>) -> Result<String> {
    let resolved = match requested {
        None | Some("inherit") => parent,
        Some(mode) => mode,
    };
    if resolved != "plan" && parent == "plan" {
        return Err(failure(
            "interaction_mode_escalation_denied",
            format!("Child interaction mode {resolved} is broader than parent mode {parent}."),
        ));
    }
    Ok(resolved.into())
}

/// `taskPrompt`.
fn task_prompt(input: &Value) -> String {
    let task = input["task"].as_str().unwrap_or_default();
    match input["role"].as_str() {
        None | Some("general") => task.into(),
        Some(role) => format!("Act as the {role} sub-agent for this task.\n\n{task}"),
    }
}

/// `providerConstraints`.
fn provider_constraints(provider: Option<&Value>, orchestration: bool) -> Vec<String> {
    let mut constraints = Vec::new();
    if !orchestration {
        constraints.push("No V2 provider adapter is registered.".into());
    }
    let Some(provider) = provider else {
        return constraints;
    };
    if provider["availability"] == "unavailable" {
        constraints.push(
            provider["unavailableReason"]
                .as_str()
                .unwrap_or("Provider driver is unavailable.")
                .into(),
        );
    }
    if provider["enabled"] != true {
        constraints.push("Provider instance is disabled.".into());
        return constraints;
    }
    if provider["installed"] != true {
        constraints.push("Provider executable is not installed.".into());
    }
    if matches!(provider["status"].as_str(), Some("error" | "disabled")) {
        constraints.push(provider["message"].as_str().map_or_else(
            || {
                format!(
                    "Provider status is {}.",
                    provider["status"].as_str().unwrap_or_default()
                )
            },
            str::to_owned,
        ));
    }
    if provider["auth"]["status"] == "unauthenticated" {
        constraints.push("Provider is not authenticated.".into());
    }
    constraints
}

fn advertised_model<'a>(provider: &'a Value, model: &str) -> Option<&'a Value> {
    let models = provider["models"].as_array()?;
    models.iter().find(|m| m["slug"] == model).or_else(|| {
        models.iter().find(|m| {
            m["aliases"].as_array().is_some_and(|aliases| {
                aliases
                    .iter()
                    .any(|a| a.as_str().is_some_and(|a| a.eq_ignore_ascii_case(model)))
            })
        })
    })
}

/// `withLegacyUltracodeEffort`.
fn with_legacy_ultracode(descriptors: Option<&Value>, selections: Option<Value>) -> Option<Value> {
    let selections = selections?;
    let list = selections.as_array()?;
    let value_of = |id: &str| list.iter().find(|s| s["id"] == id).map(|s| s["value"].clone());
    let has_ultracode = descriptors
        .and_then(Value::as_array)
        .is_some_and(|d| d.iter().any(|d| d["id"] == "ultracode" && d["type"] == "boolean"));
    if value_of("effort") != Some(json!("ultracode")) || !has_ultracode {
        return Some(selections);
    }
    let mut out: Vec<Value> = list
        .iter()
        .map(|s| {
            if s["id"] == "effort" && s["value"] == "ultracode" {
                json!({"id":"effort","value":"xhigh"})
            } else {
                s.clone()
            }
        })
        .collect();
    if value_of("ultracode").is_none() {
        out.push(json!({"id":"ultracode","value":true}));
    }
    Some(Value::Array(out))
}

/// `invalidOptionSelections`.
fn invalid_options(selections: &Value, descriptors: Option<&Value>) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    for selection in selections.as_array().into_iter().flatten() {
        let id = selection["id"].as_str().unwrap_or_default();
        if !seen.insert(id.to_owned()) {
            problems.push(format!("Option {id} was specified more than once."));
            continue;
        }
        let Some(descriptors) = descriptors.and_then(Value::as_array) else {
            continue;
        };
        let Some(descriptor) = descriptors.iter().find(|d| d["id"] == id) else {
            let known = descriptors
                .iter()
                .filter_map(|d| d["id"].as_str())
                .collect::<Vec<_>>()
                .join(", ");
            problems.push(format!(
                "Unknown option {id}; supported options: {}.",
                if known.is_empty() { "none" } else { &known }
            ));
            continue;
        };
        if descriptor["type"] == "boolean" && !selection["value"].is_boolean() {
            problems.push(format!("Option {id} expects a boolean value."));
            continue;
        }
        if descriptor["type"] == "select"
            && !descriptor["options"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|choice| choice["id"] == selection["value"])
        {
            let choices = descriptor["options"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c["id"].as_str())
                .collect::<Vec<_>>()
                .join(", ");
            problems.push(format!("Option {id} must be one of: {choices}."));
        }
    }
    problems
}

struct ResolvedTarget {
    model_selection: Value,
    supported_runtime_modes: Vec<String>,
}

fn task_status_for_run(run: Option<&OrchestrationV2Run>) -> &'static str {
    match run.map(run_outcome) {
        Some(OrchestrationV2RunStatus::Queued) => "queued",
        Some(OrchestrationV2RunStatus::Waiting) => "waiting",
        Some(OrchestrationV2RunStatus::Completed) => "completed",
        Some(OrchestrationV2RunStatus::Failed) => "failed",
        Some(OrchestrationV2RunStatus::Cancelled | OrchestrationV2RunStatus::RolledBack) => {
            "cancelled"
        }
        Some(OrchestrationV2RunStatus::Interrupted) => "interrupted",
        _ => "running",
    }
}

fn is_terminal_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled" | "interrupted")
}

fn id_value(value: &impl serde::Serialize) -> Value {
    encoded(value).unwrap_or(Value::Null)
}

/// `delegatedTaskRun`.
fn delegated_task_run<'a>(
    child: &'a OrchestrationV2ThreadProjection,
    task: &Value,
) -> Option<&'a OrchestrationV2Run> {
    if let Some(run) = child
        .runs
        .iter()
        .find(|run| id_value(&run.delegated_task_id) == task["id"])
    {
        return Some(run);
    }
    let transfers = child
        .context_transfers
        .iter()
        .map(id_value)
        .collect::<Vec<_>>();
    let spawn = transfers.iter().find(|t| {
        t["type"] == "subagent_spawn"
            && t["sourceThreadId"] == task["threadId"]
            && t["targetThreadId"] == task["childThreadId"]
    });
    match spawn {
        None => child.runs.first(),
        Some(spawn) if spawn["targetRunId"].is_null() => None,
        Some(spawn) => child.runs.iter().find(|run| id_value(&run.id) == spawn["targetRunId"]),
    }
}

/// `latestTerminalResultRun`.
fn latest_terminal_result_run<'a>(
    child: &'a OrchestrationV2ThreadProjection,
    delegated: Option<&OrchestrationV2Run>,
) -> Option<&'a OrchestrationV2Run> {
    let monitor_runs: Vec<Value> = child
        .messages
        .iter()
        .map(id_value)
        .filter(|m| m["notification"]["source"]["kind"] == "monitor")
        .map(|m| m["runId"].clone())
        .collect();
    child
        .runs
        .iter()
        .filter(|run| {
            is_terminal_run(run)
                && !monitor_runs.contains(&id_value(&run.id))
                && run.status != OrchestrationV2RunStatus::RolledBack
                && (delegated.is_some_and(|d| d.id == run.id) || run.started_at.is_some())
        })
        .fold(None, |latest: Option<&OrchestrationV2Run>, run| match latest {
            Some(latest) if !run_ran_after(run, latest) => Some(latest),
            _ => Some(run),
        })
}

fn can_expose(run: Option<&OrchestrationV2Run>) -> bool {
    run.is_some_and(|run| {
        run.status != OrchestrationV2RunStatus::RolledBack
            && is_terminal_status(task_status_for_run(Some(run)))
    })
}

/// `subagentResultForRun(...).text`.
fn subagent_result_text(child: &OrchestrationV2ThreadProjection, run: &OrchestrationV2Run) -> String {
    let run_id = id_value(&run.id);
    let items: Vec<Value> = child.turn_items.iter().map(id_value).collect();
    let turn_item = items
        .iter()
        .filter(|i| {
            i["runId"] == run_id
                && i["type"] == "assistant_message"
                && i["text"].as_str().is_some_and(|t| !t.trim().is_empty())
        })
        .max_by_key(|i| i["ordinal"].as_u64());
    let messages: Vec<Value> = child
        .messages
        .iter()
        .map(id_value)
        .filter(|m| {
            m["runId"] == run_id
                && m["role"] == "assistant"
                && m["text"].as_str().is_some_and(|t| !t.trim().is_empty())
        })
        .collect();
    let message = turn_item
        .and_then(|item| messages.iter().find(|m| m["id"] == item["messageId"]))
        .or_else(|| {
            messages.iter().max_by(|l, r| {
                l["updatedAt"]
                    .as_str()
                    .cmp(&r["updatedAt"].as_str())
                    .then(l["createdAt"].as_str().cmp(&r["createdAt"].as_str()))
            })
        });
    let failure = (run.status == OrchestrationV2RunStatus::Failed)
        .then(|| {
            items
                .iter()
                .filter(|i| i["runId"] == run_id && i["type"] == "error")
                .max_by_key(|i| i["ordinal"].as_u64())
        })
        .flatten();
    let report_before_failure = match (failure, turn_item) {
        (Some(failure), Some(item)) if item["ordinal"].as_u64() < failure["ordinal"].as_u64() => {
            message
                .and_then(|m| m["text"].as_str())
                .or_else(|| item["text"].as_str())
        }
        _ => None,
    };
    let text = failure
        .map(|failure| {
            let message = failure["failure"]["message"].as_str().unwrap_or_default();
            match report_before_failure {
                None => message.to_owned(),
                Some(report) => format!(
                    "{message}\n\nLast assistant message before the run failed:\n\n{report}"
                ),
            }
        })
        .or_else(|| message.and_then(|m| m["text"].as_str()).map(str::to_owned))
        .or_else(|| turn_item.and_then(|i| i["text"].as_str()).map(str::to_owned))
        .unwrap_or_else(|| {
            if run.status == OrchestrationV2RunStatus::Completed {
                "Child task completed without an assistant result.".into()
            } else {
                format!(
                    "Child task ended with status {}.",
                    id_value(&run.status).as_str().unwrap_or_default()
                )
            }
        });
    let executed = child
        .provider_turns
        .iter()
        .map(id_value)
        .filter(|t| !id_value(&run.active_attempt_id).is_null() && t["runAttemptId"] == id_value(&run.active_attempt_id))
        .max_by_key(|t| t["ordinal"].as_u64())
        .and_then(|t| t["model"].as_str().map(str::to_owned));
    match executed {
        // `withExecutedModelNote`.
        Some(executed) if executed != run.model_selection.model => format!(
            "Requested model {}; the provider reported running {executed}.\n\n{text}",
            run.model_selection.model
        ),
        _ => text,
    }
}

impl DelegationAdapter {
    fn threads(&self, context: &CallContext) -> Result<t3_orchestration::ports::thread_project::thread_management_service::ThreadManagementService> {
        Ok(self
            .orchestration
            .commands
            .threads(super::orchestration::command_caller(context)?))
    }

    async fn task_record(&self, context: &CallContext, parent: &ThreadId, task: &NodeId) -> Result<Option<Value>> {
        let records = self
            .threads(context)?
            .records(
                parent.clone(),
                vec![RecordKind::Subagents],
                RecordFilter {
                    subagent_ids: Some(vec![task.clone()]),
                    ..Default::default()
                },
            )
            .await
            .map_err(|error| {
                failure(
                    "orchestration_error",
                    format!("Unable to read thread {}: {error}", parent.0),
                )
            })?;
        Ok(records
            .subagents
            .iter()
            .map(id_value)
            .find(|t| t["id"] == id_value(task) && t["origin"] == "app_owned" && t["threadId"] == id_value(parent)))
    }

    /// `readTask`.
    async fn read_task(
        &self,
        context: &CallContext,
        task_id: &NodeId,
        wait_timed_out: bool,
        acknowledge_terminal: bool,
    ) -> Result<Value> {
        let caller = context.scope.require_thread("task_status")?;
        let parent = caller.thread_id.clone();
        let task = self
            .task_record(context, &parent, task_id)
            .await?
            .filter(|t| !t["childThreadId"].is_null() || t.get("remoteChild").is_some_and(|r| !r.is_null()))
            .ok_or_else(|| {
                failure(
                    "task_not_found",
                    format!(
                        "Delegated task {} does not belong to thread {}.",
                        task_id.0, parent.0
                    ),
                )
            })?;
        let Some(child_id) = task["childThreadId"].as_str().map(|id| ThreadId(id.into())) else {
            return Err(not_available("task_status", "RemoteDelegation::read"));
        };
        let threads = self.threads(context)?;
        let controls = threads
            .query_records(ProjectionRecordQuery {
                thread_id: child_id.clone(),
                fields: vec![
                    RecordKind::Runs,
                    RecordKind::Messages,
                    RecordKind::ContextTransfers,
                    RecordKind::Subagents,
                    RecordKind::ProviderThreads,
                    RecordKind::ProviderTurns,
                    RecordKind::RuntimeRequests,
                    RecordKind::Nodes,
                ],
                filter: RecordFilter {
                    omit_settled_leaf_nodes: true,
                    omit_settled_subagents: true,
                    ..Default::default()
                },
                messages: MessageFilter {
                    roles: Some(vec![decoded(json!("user"))?]),
                    ..Default::default()
                },
                run_statuses: None,
                include_latest_run: false,
            })
            .await
            .map_err(management_failure)?;
        let child_run = delegated_task_run(&controls, &task).cloned();
        let terminal_run = latest_terminal_result_run(&controls, child_run.as_ref()).cloned();
        let progress = delegated_task_progress(&controls).map_err(management_failure)?;
        let followups = if child_run
            .as_ref()
            .is_some_and(|run| id_value(&run.delegated_task_id) == task["id"])
        {
            Some(
                delegated_task_runs(&controls.runs, &controls.messages, task_id)
                    .map_err(management_failure)?,
            )
        } else {
            None
        };
        let task_result_run = match &followups {
            None => progress.result_run.cloned(),
            Some(runs) => delegated_task_result_run_from(runs).cloned(),
        };
        let task_work_state = match &followups {
            None => progress.state,
            Some(runs) => {
                if !is_terminal_status(task_status_for_run(task_result_run.as_ref())) {
                    "working"
                } else if delegated_task_awaits_work(runs, &controls.subagents, &controls.provider_threads) {
                    "waiting_for_children"
                } else {
                    "result_available"
                }
            }
        };
        let mut result_run_ids: Vec<t3_contracts::base_schemas::RunId> = Vec::new();
        for run in [task_result_run.as_ref(), terminal_run.as_ref()].into_iter().flatten() {
            let id = decoded(id_value(&run.id))?;
            if !result_run_ids.contains(&id) {
                result_run_ids.push(id);
            }
        }
        let result_records = threads
            .query_records(ProjectionRecordQuery {
                thread_id: child_id.clone(),
                fields: vec![RecordKind::Messages, RecordKind::TurnItems],
                filter: RecordFilter {
                    turn_item_run_ids: Some(result_run_ids.iter().cloned().map(Some).collect()),
                    turn_item_types: Some(vec!["assistant_message".into(), "error".into()]),
                    ..Default::default()
                },
                messages: MessageFilter {
                    roles: Some(vec![decoded(json!("assistant"))?]),
                    run_ids: Some(result_run_ids.clone()),
                    ..Default::default()
                },
                run_statuses: None,
                include_latest_run: false,
            })
            .await
            .map_err(management_failure)?;
        let mut child = controls.clone();
        child.messages.extend(result_records.messages);
        child.turn_items = result_records.turn_items;
        let task_result = task.get("result").filter(|r| !r.is_null()).cloned();
        if task_result.is_none() && task_work_state == "result_available" {
            // TS `delegatedTaskResultPending`: a restart may still hold this result.
            return Err(not_available(
                "task_status",
                "ThreadManagementService::delegated_task_result_pending",
            ));
        }
        let work_state = if task_result.is_some() {
            "result_available"
        } else {
            task_work_state
        };
        let status: &str = if task_result.is_some() {
            if matches!(
                task["status"].as_str(),
                Some("completed" | "failed" | "cancelled" | "interrupted")
            ) {
                match task["status"].as_str() {
                    Some("completed") => "completed",
                    Some("failed") => "failed",
                    Some("cancelled") => "cancelled",
                    _ => "interrupted",
                }
            } else {
                task_status_for_run(child_run.as_ref())
            }
        } else if work_state == "result_available" {
            let run = task_result_run.as_ref().or(child_run.as_ref());
            if task.get("cancelledRunId").is_some_and(|v| !v.is_null())
                && task_status_for_run(run) == "completed"
            {
                "interrupted"
            } else {
                task_status_for_run(run)
            }
        } else if task_status_for_run(task_result_run.as_ref().or(child_run.as_ref())) == "queued" {
            "queued"
        } else {
            "running"
        };
        let derived_result = match (&task_result, &task_result_run) {
            (Some(result), _) => result.clone(),
            (None, Some(run)) if is_terminal_status(status) => {
                Value::String(subagent_result_text(&child, run))
            }
            _ => Value::Null,
        };
        let transfers: Vec<Value> = controls.context_transfers.iter().map(id_value).collect();
        let parent_value = id_value(&parent);
        let result_transfers: Vec<&Value> = transfers
            .iter()
            .filter(|t| {
                t["type"] == "subagent_result"
                    && t["sourceThreadId"] == task["childThreadId"]
                    && t["targetThreadId"] == parent_value
            })
            .collect();
        let transfer_for_run = |run: Option<&OrchestrationV2Run>| -> Value {
            let Some(run) = run.filter(|run| can_expose(Some(run))) else {
                return Value::Null;
            };
            let run_id = id_value(&run.id);
            result_transfers
                .iter()
                .find(|t| t["sourcePoint"]["runId"] == run_id)
                .or_else(|| {
                    (child_run.as_ref().is_some_and(|c| c.id == run.id))
                        .then(|| {
                            result_transfers
                                .iter()
                                .find(|t| t["sourcePoint"].get("runId").is_none_or(Value::is_null))
                        })
                        .flatten()
                })
                .map_or(Value::Null, |t| t["id"].clone())
        };
        // `taskResultTransfer`: the transfer the child sent for this task's runs.
        let original_run = child_run.as_ref();
        let result_run_set: Vec<Value> = child
            .runs
            .iter()
            .filter(|run| {
                id_value(&run.delegated_task_id) == task["id"]
                    || (original_run.is_none_or(|o| id_value(&o.delegated_task_id).is_null())
                        && id_value(&run.delegated_task_id).is_null())
            })
            .map(|run| id_value(&run.id))
            .chain(original_run.map(|run| id_value(&run.id)))
            .collect();
        let result_transfer = result_transfers
            .iter()
            .find(|t| {
                let source_run = &t["sourcePoint"]["runId"];
                (!source_run.is_null() && result_run_set.contains(source_run))
                    || (source_run.is_null()
                        && original_run.is_none_or(|o| id_value(&o.delegated_task_id).is_null()))
            })
            .map_or(Value::Null, |t| t["id"].clone());
        let terminal_status = terminal_run.as_ref().map(|run| task_status_for_run(Some(run)));
        let latest_terminal_summary = if can_expose(terminal_run.as_ref()) {
            let run = terminal_run.as_ref().expect("exposable run");
            if child_run.as_ref().is_some_and(|c| c.id == run.id) {
                derived_result.clone()
            } else {
                Value::String(subagent_result_text(&child, run))
            }
        } else {
            Value::Null
        };
        let latest_transfer = transfer_for_run(terminal_run.as_ref());
        let execution_run = if task_result.is_none() {
            task_result_run.as_ref().or(child_run.as_ref())
        } else {
            controls
                .runs
                .iter()
                .find(|run| {
                    !result_transfer.is_null()
                        && result_transfers
                            .iter()
                            .any(|t| t["id"] == result_transfer && t["sourcePoint"]["runId"] == id_value(&run.id))
                })
                .or(child_run.as_ref())
        };
        let selection = match execution_run {
            Some(run) => id_value(&run.model_selection),
            None => id_value(&controls.thread.model_selection),
        };
        let executed = execution_run
            .and_then(|run| {
                let attempt = id_value(&run.active_attempt_id);
                child
                    .provider_turns
                    .iter()
                    .map(id_value)
                    .filter(|t| !attempt.is_null() && t["runAttemptId"] == attempt)
                    .max_by_key(|t| t["ordinal"].as_u64())
                    .map(|t| t["model"].clone())
            })
            .unwrap_or(Value::Null);
        let requests: Vec<Value> = controls.runtime_requests.iter().map(id_value).collect();
        let has_pending_question = !is_terminal_status(status)
            && requests.iter().any(|r| {
                r["kind"] == "user_input"
                    && r["status"] == "pending"
                    && r["responseCapability"]["type"] == "live"
            });
        let disposed = task["completionDelivery"]["state"] == "disposed";
        let pending_approval = if is_terminal_status(status) || disposed {
            Value::Null
        } else {
            pending_task_approval(&controls, &task["id"], child_run.as_ref())
        };
        let has_pending_child_runs = child.runs.iter().any(|run| {
            !is_terminal_run(run)
                && !(run.status == OrchestrationV2RunStatus::Queued
                    && id_value(&run.queue_held) == json!(true))
                && child_run.as_ref().is_none_or(|d| run.ordinal > d.ordinal)
        });
        let title = controls.thread.title.0.clone();
        let mut result = json!({
            "taskId": task["id"],
            "childThreadId": task["childThreadId"],
            "link": format!(
                "[{}](t3-thread://v2/{}/{})",
                title.replace(['[', ']', '\\', '\r', '\n'], " ").split_whitespace().collect::<Vec<_>>().join(" "),
                context.scope.environment_id.0,
                child_id.0
            ),
            "childRunId": child_run.as_ref().map_or(Value::Null, |run| id_value(&run.id)),
            "childNodeId": task["id"],
            "status": status,
            "workState": work_state,
            "hasPendingChildRuns": has_pending_child_runs,
            "providerInstanceId": selection["instanceId"],
            "model": selection["model"],
            "executedModel": executed,
            "options": selection.get("options").cloned().unwrap_or_else(|| json!([])),
            "summary": derived_result,
            "resultContextTransferId": result_transfer,
            "latestTerminalRunId": terminal_run.as_ref().map_or(Value::Null, |run| id_value(&run.id)),
            "latestTerminalStatus": terminal_status.filter(|s| is_terminal_status(s)).map_or(Value::Null, |s| json!(s)),
            "latestTerminalSummary": if latest_terminal_summary == derived_result { Value::Null } else { latest_terminal_summary },
            "latestTerminalResultContextTransferId": if latest_transfer == result_transfer { Value::Null } else { latest_transfer },
            "hasPendingQuestion": has_pending_question,
            "pendingApproval": pending_approval,
            "waitTimedOut": wait_timed_out,
        });
        let ends_wait = has_pending_question || !result["pendingApproval"].is_null() || is_terminal_status(status);
        if !ends_wait && !disposed {
            result["next"] = json!(ORCHESTRATOR_MCP_RUNNING_TASK_NEXT);
        }
        let delivered = matches!(task["completionDelivery"]["state"].as_str(), Some("acknowledged" | "disposed"));
        if acknowledge_terminal && is_terminal_status(status) && !delivered {
            // The parent's active run owned by this provider observed the result.
            let runs = threads
                .records(parent.clone(), vec![RecordKind::Runs], RecordFilter::default())
                .await
                .map_err(management_failure)?
                .runs;
            let observing = runs
                .iter()
                .filter(|run| is_active_run(run) && id_value(&run.provider_instance_id) == id_value(&caller.provider_instance_id))
                .max_by_key(|run| run.ordinal)
                .map_or(Value::Null, |run| id_value(&run.id));
            let command = json!({
                "type": "delegated_task.completion-delivery.acknowledge",
                "commandId": stable_command_id(&context.scope, &uuid::Uuid::new_v4().to_string(), "task-status-acknowledge"),
                "parentThreadId": parent,
                "taskId": task["id"],
                "observedByRunId": observing,
            });
            threads
                .dispatch(decoded(command)?)
                .await
                .map_err(|error| {
                    failure(
                        "orchestration_error",
                        format!("Unable to acknowledge delegated task {}: {error}", task_id.0),
                    )
                })?;
        }
        Ok(result)
    }

    /// `resolveTarget`, without the one-instance refresh TypeScript retries with.
    async fn resolve_target(
        &self,
        parent: &Value,
        target: &Value,
        skip_enablement: bool,
    ) -> Result<ResolvedTarget> {
        let providers = self.host.providers().await?;
        let capable = self.host.orchestration_instances().await?;
        let inherited = &parent["modelSelection"];
        let requested_driver = target["driverKind"].as_str();
        let mut instance = target["providerInstanceId"].as_str().map(str::to_owned);
        if instance.is_none() && let Some(driver) = requested_driver {
            let candidates: Vec<&Value> = providers
                .iter()
                .filter(|p| p["driver"] == driver && p["instanceId"].as_str().is_some_and(|id| capable.contains(id)))
                .collect();
            if candidates.is_empty() {
                return Err(failure("provider_unavailable", format!("No V2 provider adapter is registered for driver {driver}.")));
            }
            let healthy: Vec<&Value> = candidates.into_iter().filter(|c| provider_constraints(Some(c), true).is_empty()).collect();
            let mut ordered: Vec<&Value> = healthy.iter().copied().filter(|c| c["instanceId"] == inherited["instanceId"]).collect();
            ordered.extend(healthy.iter().copied().filter(|c| c["instanceId"] != inherited["instanceId"]));
            let project = ProjectId(parent["projectId"].as_str().unwrap_or_default().into());
            for candidate in ordered {
                let id = candidate["instanceId"].as_str().unwrap_or_default();
                if skip_enablement || self.host.enabled_for_project(&project, id).await? {
                    instance = Some(id.into());
                    break;
                }
            }
            if instance.is_none() {
                return Err(failure(
                    "provider_unavailable",
                    if healthy.is_empty() {
                        format!("No available V2 provider instance for driver {driver}.")
                    } else {
                        format!("No instance of driver {driver} is enabled for this project.")
                    },
                ));
            }
        }
        let instance = instance.unwrap_or_else(|| inherited["instanceId"].as_str().unwrap_or_default().into());
        let provider = providers
            .iter()
            .find(|p| p["instanceId"] == instance.as_str())
            .ok_or_else(|| failure("provider_unavailable", format!("Provider instance {instance} is not registered.")))?;
        if let Some(driver) = requested_driver
            && provider["driver"] != driver
        {
            return Err(failure(
                "invalid_request",
                format!(
                    "Provider instance {instance} uses driver {}, not {driver}.",
                    provider["driver"].as_str().unwrap_or_default()
                ),
            ));
        }
        let constraints = provider_constraints(Some(provider), capable.contains(&instance));
        if !constraints.is_empty() {
            return Err(failure("provider_unavailable", format!("Provider {instance} cannot run a child task: {}", constraints.join(" "))));
        }
        let requested_model = target["model"].as_str();
        let model = requested_model.map(str::to_owned).or_else(|| {
            if inherited["instanceId"] == instance.as_str() {
                inherited["model"].as_str().map(str::to_owned)
            } else {
                provider["models"][0]["slug"].as_str().map(str::to_owned)
            }
        });
        let Some(model) = model else {
            return Err(failure("model_unavailable", format!("Provider {instance} has no model available for inheritance.")));
        };
        if let Some(requested) = requested_model
            && provider["models"].as_array().is_some_and(|m| !m.is_empty())
            && advertised_model(provider, requested).is_none()
        {
            return Err(failure("model_unavailable", format!("Model {requested} is not advertised by provider {instance}.")));
        }
        let advertised = advertised_model(provider, &model);
        let descriptors = advertised.map(|a| &a["capabilities"]["optionDescriptors"]).filter(|d| d.is_array());
        let requested_options = with_legacy_ultracode(descriptors, target.get("options").filter(|o| !o.is_null()).cloned());
        if let Some(options) = &requested_options {
            let invalid = invalid_options(options, descriptors);
            if !invalid.is_empty() {
                return Err(failure("invalid_request", format!("Model {model} on provider {instance} rejected options: {}", invalid.join(" "))));
            }
        }
        let inherits = inherited["instanceId"] == instance.as_str()
            && advertised.and_then(|a| a["slug"].as_str()).unwrap_or(&model)
                == advertised_model(provider, inherited["model"].as_str().unwrap_or_default())
                    .and_then(|a| a["slug"].as_str())
                    .unwrap_or(inherited["model"].as_str().unwrap_or_default());
        let mut options: Option<Vec<Value>> = if inherits {
            with_legacy_ultracode(descriptors, inherited.get("options").filter(|o| !o.is_null()).cloned())
                .and_then(|o| o.as_array().cloned())
                .map(|o| match descriptors.and_then(Value::as_array) {
                    None => o,
                    Some(d) => o.into_iter().filter(|s| d.iter().any(|d| d["id"] == s["id"])).collect(),
                })
        } else {
            None
        };
        if let Some(requested) = requested_options.as_ref().and_then(Value::as_array) {
            let mut merged: Vec<Value> = if requested.is_empty() { Vec::new() } else { options.clone().unwrap_or_default() };
            for selection in requested {
                merged.retain(|s| s["id"] != selection["id"]);
                merged.push(selection.clone());
            }
            options = Some(merged);
        }
        let mut model_selection = json!({"instanceId": instance, "model": model});
        if let Some(options) = options {
            model_selection["options"] = Value::Array(options);
        }
        Ok(ResolvedTarget {
            model_selection,
            supported_runtime_modes: provider["supportedRuntimeModes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m.as_str().map(str::to_owned))
                .collect(),
        })
    }
}

/// `pendingTaskApproval`: the oldest live approval request in the task's own runs.
fn pending_task_approval(child: &OrchestrationV2ThreadProjection, task_id: &Value, original: Option<&OrchestrationV2Run>) -> Value {
    let task_runs: Vec<Value> = child
        .runs
        .iter()
        .filter(|run| {
            !is_terminal_run(run)
                && (id_value(&run.delegated_task_id) == *task_id
                    || (id_value(&run.delegated_task_id).is_null() && original.is_some_and(|o| o.id == run.id)))
        })
        .map(|run| id_value(&run.id))
        .collect();
    let nodes: Vec<Value> = child.nodes.iter().map(id_value).collect();
    let mut approval = Value::Null;
    let mut oldest: Option<String> = None;
    for request in child.runtime_requests.iter().map(id_value) {
        let approval_kind = matches!(
            request["kind"].as_str(),
            Some("command" | "file-read" | "file-change" | "permission" | "mcp-elicitation")
        );
        if request["status"] != "pending" || request["responseCapability"]["type"] != "live" || !approval_kind {
            continue;
        }
        let Some(run_id) = nodes.iter().find(|n| n["id"] == request["nodeId"]).map(|n| n["runId"].clone()) else {
            continue;
        };
        if run_id.is_null() || !task_runs.contains(&run_id) {
            continue;
        }
        let created = request["createdAt"].as_str().unwrap_or_default().to_owned();
        if oldest.as_ref().is_none_or(|o| created < *o) {
            oldest = Some(created);
            approval = json!({"requestId": request["id"], "kind": request["kind"], "runId": run_id});
        }
    }
    approval
}

#[async_trait]
impl OrchestratorService for DelegationAdapter {
    async fn delegate_task(
        &self,
        context: CallContext,
        input: OrchestratorMcpDelegateTaskInput,
    ) -> Result<OrchestratorMcpDelegateTaskResult> {
        let input = encoded(input)?;
        let target = input.get("target").cloned().unwrap_or(Value::Null);
        if target["environmentId"]
            .as_str()
            .is_some_and(|id| id != context.scope.environment_id.0)
        {
            return Err(not_available("delegate_task", "RemoteDelegation::delegate"));
        }
        if input["mode"] == "wait" {
            return Err(not_available(
                "delegate_task",
                "ThreadManagementService::stored_events_from",
            ));
        }
        if input["isolation"] == "worktree" {
            return Err(not_available("delegate_task", "ThreadLaunchService::prepare_run"));
        }
        let caller = context.scope.require_thread("delegate_task")?.clone();
        let threads = self.threads(&context)?;
        let parent = threads
            .records(caller.thread_id.clone(), vec![RecordKind::Runs], RecordFilter::default())
            .await
            .map_err(|error| failure("orchestration_error", format!("Unable to read thread {}: {error}", caller.thread_id.0)))?;
        let parent_thread = id_value(&parent.thread);
        if let Some(project) = target["projectId"].as_str()
            && parent_thread["projectId"] != project
        {
            return Err(failure(
                "invalid_request",
                "A local task runs in this thread's project. Omit target.projectId, or pass target.environmentId to run it in that environment's project.",
            ));
        }
        let parent_run = parent
            .runs
            .iter()
            .filter(|run| is_active_run(run))
            .max_by_key(|run| run.ordinal)
            .filter(|run| {
                !id_value(&run.root_node_id).is_null()
                    && id_value(&run.provider_instance_id) == id_value(&caller.provider_instance_id)
            })
            .ok_or_else(|| {
                failure(
                    "parent_not_active",
                    "Delegated tasks require an active run owned by this MCP provider session.",
                )
            })?;
        let key = request_key(&input);
        let command_id = stable_command_id(&context.scope, &key, "delegate-task");
        let retry = threads
            .has_receipt(t3_contracts::base_schemas::CommandId(command_id.clone()))
            .await;
        let resolved = self.resolve_target(&parent_thread, &target, retry).await?;
        let runtime_mode = resolve_runtime_mode(
            parent_thread["runtimeMode"].as_str().unwrap_or("approval-required"),
            input["runtimeMode"].as_str(),
            Some((
                resolved.model_selection["instanceId"].as_str().unwrap_or_default(),
                &resolved.supported_runtime_modes,
            )),
        )?;
        let interaction_mode = resolve_interaction_mode(
            parent_thread["interactionMode"].as_str().unwrap_or("default"),
            input["interactionMode"].as_str(),
        )?;
        let mut command = json!({
            "type": "delegated_task.request",
            "createdBy": "agent",
            "creationSource": "mcp",
            "commandId": command_id,
            "parentThreadId": caller.thread_id,
            "parentRunId": parent_run.id,
            "parentNodeId": parent_run.root_node_id,
            "task": task_prompt(&input),
            "modelSelection": resolved.model_selection,
            "runtimeMode": runtime_mode,
            "interactionMode": interaction_mode,
            "completionWake": "always",
        });
        if let Some(title) = input.get("title").filter(|t| !t.is_null()) {
            command["title"] = title.clone();
        }
        threads
            .dispatch(decoded(command)?)
            .await
            .map_err(|error| match error {
                t3_orchestration::error::Error::CallerRefused { refusal, .. } => failure(refusal.code(), refusal.to_string()),
                error => failure("orchestration_error", format!("Unable to create delegated task: {error}")),
            })?;
        // The engine names the task after the command (`derived_id`); the projection confirms it.
        let task_id = NodeId(derived_id("node", &["delegated-task", &command_id]));
        if self
            .task_record(&context, &caller.thread_id, &task_id)
            .await?
            .is_none()
        {
            return Err(failure(
                "orchestration_error",
                "Delegated task command did not produce a task projection.",
            ));
        }
        decoded(self.read_task(&context, &task_id, false, true).await?)
    }

    async fn task_status(
        &self,
        context: CallContext,
        input: OrchestratorMcpTaskStatusInput,
    ) -> Result<OrchestratorMcpDelegateTaskResult> {
        context.scope.require_thread("task_status")?;
        let input = encoded(input)?;
        let task = NodeId(input["taskId"].as_str().unwrap_or_default().into());
        decoded(self.read_task(&context, &task, false, true).await?)
    }
}
