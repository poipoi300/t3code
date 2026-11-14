#![cfg(feature = "claude-adapter")]

use crate::provider::{ProviderError, ProviderOperation, ProviderResult, RuntimePolicy};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use t3_claude_cli::control::{
    PermissionDecision, PermissionMode, PermissionRule, PermissionUpdate, RuleBehavior,
    ToolPermission,
};
use t3_contracts::{
    orchestration_v2::*,
    provider_policy::{
        ProviderApprovalDecision, ProviderInteractionMode, ProviderUserInputAnswers, RuntimeMode,
    },
};

pub const CLAUDE_QUERY_ENDED_BEFORE_TURN_DETAIL: &str =
    "Claude provider query ended before the turn could start.";
pub const CLAUDE_READ_ONLY_ALLOWED_TOOLS: &[&str] = &["Read", "Glob", "Grep"];
pub const CLAUDE_T3_MCP_TOOL_TIMEOUT_MS: u64 = 65 * 60 * 1_000;
pub const CLAUDE_SEND_TO_USER_INSTRUCTIONS: &str = r#"
### Text the user must read mid-turn

Text you write before a tool call may reach the user only as a one-line summary. When the user must read something in full before your turn ends, such as a draft, a code block, or what a question you're about to ask refers to, send it with `send_to_user`. Put everything else in your final reply.
"#;
pub const CLAUDE_READ_ONLY_T3_MCP_ALLOWED_TOOLS: &[&str] = &[
    "mcp__t3-code__orchestrator_capabilities",
    "mcp__t3-code__list_scheduled_tasks",
    "mcp__t3-code__t3_background_wait",
    "mcp__t3-code__t3_thread_list",
    "mcp__t3-code__t3_thread_wait",
    "mcp__t3-code__t3_worktree_status",
    "mcp__t3-code__t3_worktree_list",
    "mcp__t3-code__t3_project_list",
    "mcp__t3-code__t3_project_read",
    "mcp__t3-code__t3_thread_search",
    "mcp__t3-code__t3_preview_list",
    "mcp__t3-code__t3_environment_read",
    "mcp__t3-code__t3_environment_links",
    "mcp__t3-code__t3_theme_read",
    "mcp__t3-code__html_preview",
    "mcp__t3-code__html_render",
    "mcp__t3-code__send_to_user",
];

pub(crate) fn decode<T: DeserializeOwned>(value: Value) -> ProviderResult<T> {
    serde_json::from_value(value).map_err(|_| ProviderError::Protocol {
        reason: None,
        detail: format!(
            "Invalid Claude provider projection: {}",
            std::any::type_name::<T>()
        ),
        payload: None,
    })
}
pub(crate) fn operation(operation: ProviderOperation, detail: impl Into<String>) -> ProviderError {
    ProviderError::Operation {
        operation,
        detail: detail.into(),
    }
}
pub(crate) fn protocol(error: t3_claude_cli::Error) -> ProviderError {
    ProviderError::Operation {
        operation: ProviderOperation::Stream,
        detail: error.to_string(),
    }
}

#[derive(Clone, Copy, Default)]
pub struct BypassAvailability {
    pub bypass_disabled: bool,
    pub auto_disabled: bool,
}
#[derive(Clone, PartialEq)]
pub struct ClaudeRuntimeQueryPolicy {
    pub permission_mode: PermissionMode,
    pub tools: Option<Vec<String>>,
    pub allowed_tools: Vec<String>,
    pub allow_dangerously_skip_permissions: bool,
    pub install_permission_callback: bool,
}
pub fn permission_mode_for_runtime_policy(policy: &RuntimePolicy) -> PermissionMode {
    use PermissionMode::*;
    if policy.interaction_mode == ProviderInteractionMode::Plan {
        return Plan;
    }
    let sandbox = policy
        .sandbox_policy
        .as_ref()
        .and_then(|v| v["type"].as_str());
    if policy
        .approval_policy
        .as_ref()
        .is_some_and(|v| v == "never")
    {
        return match sandbox {
            Some("readOnly") => DontAsk,
            Some("dangerFullAccess" | "externalSandbox") => BypassPermissions,
            _ => match policy.runtime_mode {
                RuntimeMode::ApprovalRequired => DontAsk,
                RuntimeMode::AutoAcceptEdits => AcceptEdits,
                _ => BypassPermissions,
            },
        };
    }
    if policy.approval_policy.is_some() {
        return Default;
    }
    match sandbox {
        Some("readOnly") => return DontAsk,
        Some("dangerFullAccess") => {
            return if policy.runtime_mode == RuntimeMode::ApprovalRequired {
                Default
            } else {
                BypassPermissions
            };
        }
        _ => {}
    }
    match policy.runtime_mode {
        RuntimeMode::ApprovalRequired => Default,
        RuntimeMode::AutoAcceptEdits => AcceptEdits,
        RuntimeMode::Auto => Auto,
        RuntimeMode::FullAccess => BypassPermissions,
    }
}
pub fn runtime_query_policy(
    policy: &RuntimePolicy,
    availability: BypassAvailability,
) -> ClaudeRuntimeQueryPolicy {
    let requested = permission_mode_for_runtime_policy(policy);
    let downgraded = requested == PermissionMode::BypassPermissions && availability.bypass_disabled;
    let permission_mode = if downgraded {
        if availability.auto_disabled {
            PermissionMode::Default
        } else {
            PermissionMode::Auto
        }
    } else {
        requested
    };
    let read_only = policy
        .sandbox_policy
        .as_ref()
        .is_some_and(|v| v["type"] == "readOnly");
    let global_reads = policy
        .sandbox_policy
        .as_ref()
        .is_some_and(|v| v["access"]["type"] == "fullAccess");
    let tools = read_only.then(|| {
        CLAUDE_READ_ONLY_ALLOWED_TOOLS
            .iter()
            .map(|s| (*s).into())
            .collect::<Vec<_>>()
    });
    let allowed_tools = if global_reads {
        tools.clone().unwrap_or_default()
    } else {
        vec![]
    };
    let callback = downgraded
        || match policy.approval_policy.as_ref() {
            None => matches!(
                policy.runtime_mode,
                RuntimeMode::ApprovalRequired | RuntimeMode::AutoAcceptEdits
            ),
            Some(v) => v != "never",
        };
    ClaudeRuntimeQueryPolicy {
        permission_mode,
        tools,
        allowed_tools,
        allow_dangerously_skip_permissions: permission_mode == PermissionMode::BypassPermissions,
        install_permission_callback: callback || permission_mode == PermissionMode::Plan,
    }
}

pub fn permission_result_from_decision(
    request: &ToolPermission,
    decision: ProviderApprovalDecision,
    feedback: Option<&str>,
    managed_ceiling: bool,
) -> PermissionDecision {
    use ProviderApprovalDecision::*;
    if matches!(decision, Accept | AcceptForSession) {
        let updated_permissions = if decision == AcceptForSession {
            let mut updates = request
                .permission_suggestions
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter(|v| !(managed_ceiling && v["type"] == "setMode"))
                .filter_map(|v| {
                    let mut value = v.clone();
                    value["destination"] = json!("session");
                    serde_json::from_value::<PermissionUpdate>(value).ok()
                })
                .collect::<Vec<_>>();
            if updates.is_empty() {
                updates.push(PermissionUpdate::AddRules {
                    rules: vec![PermissionRule {
                        tool_name: request.tool_name.clone(),
                        rule_content: None,
                    }],
                    behavior: RuleBehavior::Allow,
                    destination: "session".into(),
                });
            }
            Some(updates)
        } else {
            None
        };
        PermissionDecision::Allow {
            updated_input: request.input.clone(),
            updated_permissions,
            decision_classification: Some(
                if decision == AcceptForSession {
                    "user_permanent"
                } else {
                    "user_temporary"
                }
                .into(),
            ),
        }
    } else {
        PermissionDecision::Deny {
            message: feedback
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(if decision == Cancel {
                    "User cancelled tool execution."
                } else {
                    "User declined tool execution."
                })
                .into(),
            interrupt: (decision == Cancel).then_some(true),
            decision_classification: Some("user_reject".into()),
        }
    }
}

pub fn claude_user_input_questions(input: &Value) -> Vec<OrchestrationV2UserInputQuestion> {
    let input = if input["type"] == "record" {
        &input["value"]
    } else {
        input
    };
    input["questions"].as_array().into_iter().flatten().enumerate().filter_map(|(index, value)| {
        let question = value["question"].as_str()?.trim();
        if question.is_empty() { return None; }
        let default_header = format!("Question {}", index + 1);
        let header = value["header"].as_str().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(&default_header);
        let options = value["options"].as_array().into_iter().flatten().filter_map(|v| {
            let label = v["label"].as_str()?.trim(); if label.is_empty() { return None; }
            let description = v["description"].as_str().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(label);
            let mut option = json!({"label":label,"description":description});
            if v["preview"].is_string() { option["preview"] = v["preview"].clone(); }
            Some(option)
        }).collect::<Vec<_>>();
        decode(json!({"id":question,"header":header,"question":question,"options":options,"multiSelect":value["multiSelect"] == true})).ok()
    }).collect()
}

pub fn claude_sdk_user_input_answers(answers: &ProviderUserInputAnswers) -> Value {
    Value::Object(
        answers
            .iter()
            .map(|(key, value)| {
                let text = match value {
                    Value::Array(v) => v
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", "),
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                };
                (key.clone(), json!(text))
            })
            .collect(),
    )
}

pub fn merge_claude_stream_usage(current: &Value, delta: &Value) -> Value {
    let mut next = current.clone();
    for key in [
        "input_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
        "output_tokens",
    ] {
        if !delta[key].is_null() {
            next[key] = delta[key].clone();
        }
    }
    next
}

pub fn claude_provider_turn_token_usage(
    usage: &Value,
    max_tokens: Option<u64>,
    updated_at: &str,
    auto_compact_window: Option<u64>,
    requested_at: Option<&str>,
) -> ProviderResult<OrchestrationV2ProviderTurnTokenUsage> {
    let number = |key: &str| usage[key].as_u64().unwrap_or(0);
    let input = number("input_tokens")
        + number("cache_creation_input_tokens")
        + number("cache_read_input_tokens");
    let output = number("output_tokens");
    let mut value = json!({"usedTokens":input + output,"maxTokens":max_tokens,"inputTokens":input,
        "cachedInputTokens":number("cache_read_input_tokens"),"outputTokens":output,"reasoningOutputTokens":0,"updatedAt":updated_at});
    let cache = &usage["cache_creation"];
    let ttl = if cache["ephemeral_1h_input_tokens"].as_u64().unwrap_or(0) > 0 {
        Some(3_600_000)
    } else if cache["ephemeral_5m_input_tokens"].as_u64().unwrap_or(0) > 0 {
        Some(300_000)
    } else {
        None
    };
    if let Some(ttl) = ttl {
        value["promptCacheTtlMs"] = json!(ttl);
        if let Some(requested) = requested_at {
            value["promptCacheExpiresAt"] = json!(add_iso_millis(requested, ttl)?);
        }
    }
    if let Some(window) =
        auto_compact_window.filter(|v| *v > 0 && max_tokens.is_none_or(|max| *v < max))
    {
        value["autoCompactThreshold"] = json!(window as i64 - 33_000);
    }
    decode(value)
}

pub fn add_iso_millis(date: &str, delta: u64) -> ProviderResult<String> {
    let invalid = || operation(ProviderOperation::Stream, "Invalid ISO timestamp");
    if date.len() < 20 || !date.ends_with('Z') {
        return Err(invalid());
    }
    let part = |range: std::ops::Range<usize>| {
        date.get(range)
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or_else(invalid)
    };
    let mut year = part(0..4)?;
    let mut month = part(5..7)?;
    let mut day = part(8..10)?;
    let millis = if date.len() == 24 { part(20..23)? } else { 0 };
    let total =
        ((part(11..13)? * 60 + part(14..16)?) * 60 + part(17..19)?) * 1_000 + millis + delta;
    let mut days = total / 86_400_000;
    while days > 0 {
        let leap =
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
        let max = match month {
            2 => {
                if leap {
                    29
                } else {
                    28
                }
            }
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        if day + days <= max {
            day += days;
            break;
        }
        days -= max - day + 1;
        day = 1;
        month += 1;
        if month > 12 {
            month = 1;
            year += 1;
        }
    }
    let rem = total % 86_400_000;
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1_000 % 60,
        rem % 1_000
    ))
}

pub fn terminal_status_from_result(result: &Value, failure_hint: Option<&str>) -> &'static str {
    if matches!(
        result["terminal_reason"].as_str(),
        Some("aborted_tools" | "aborted_streaming")
    ) {
        return "interrupted";
    }
    if result["subtype"] == "success" {
        if result["api_error_status"] == 429
            || result["api_error_status"] == 529
            || terminal_result_error(result["terminal_reason"].as_str(), failure_hint).is_some()
            || (result["is_error"] == true && failure_hint.is_some())
        {
            "failed"
        } else {
            "completed"
        }
    } else {
        let errors = result["errors"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n")
            .to_lowercase();
        if errors.contains("interrupt") {
            "interrupted"
        } else if errors.contains("cancel") {
            "cancelled"
        } else {
            "failed"
        }
    }
}

pub fn terminal_result_error(reason: Option<&str>, hint: Option<&str>) -> Option<String> {
    Some(match reason? {
        "api_error" => hint.unwrap_or("Claude gave up after repeated API errors."),
        "malformed_tool_use_exhausted" => "Claude gave up after repeated malformed tool calls.",
        "budget_exhausted" => "Claude stopped: the turn's token budget was exhausted.",
        "structured_output_retry_exhausted" => "Claude could not produce the requested structured output.",
        "tool_deferred_unavailable" => "Claude could not resume a deferred tool call: the tool is no longer available.",
        "turn_setup_failed" => "Claude could not start the turn.",
        "blocking_limit" => hint.unwrap_or("Claude stopped: the prompt exceeds the model's context window. Run /compact or start a new thread."),
        "prompt_too_long" => "Claude stopped: the prompt exceeds the model's context window. Run /compact or start a new thread.",
        "rapid_refill_breaker" => "Claude stopped: the context refilled too quickly after compaction.",
        "image_error" => "Claude stopped: an image in the conversation could not be processed.",
        "model_error" => "Claude stopped: the model returned an error.", _ => return None,
    }.into())
}

pub fn encoded_part(value: &str) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            result.push(byte as char);
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}
pub fn provider_id(prefix: &str, label: &str, native: &str) -> String {
    format!(
        "{prefix}:provider:claudeAgent:{label}:{}",
        encoded_part(native)
    )
}
pub fn native_ref(native: &str, strength: &str) -> Value {
    json!({"driver":"claudeAgent","nativeId":native,"strength":strength})
}

pub fn tool_classification(name: &str) -> (&'static str, &'static str) {
    let normalized = name
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '_' | '-'))
        .flat_map(char::to_lowercase)
        .collect::<String>();
    match normalized.as_str() {
        "bash" => ("command_execution", "command"),
        "edit" | "multiedit" | "notebookedit" | "write" => ("file_change", "file-change"),
        "glob" | "grep" | "ls" | "read" => ("dynamic_tool", "file-read"),
        "webfetch" | "websearch" => ("web_search", "command"),
        _ => ("dynamic_tool", "command"),
    }
}

use crate::{
    api::{Clock, IdGenerator, IdKind},
    provider::*,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};
use t3_contracts::{
    base_schemas::*,
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
};

fn usage_count(value: &Value) -> Option<u64> {
    let count = value.as_f64()?;
    (count.is_finite() && count >= 0.0 && count.round() < u64::MAX as f64)
        .then(|| count.round() as u64)
}

type ClaudeModelLedger = BTreeMap<String, (u64, u64)>;
fn model_ledger(value: &Value) -> Option<ClaudeModelLedger> {
    let entries = value.as_object().filter(|entries| !entries.is_empty())?;
    entries
        .iter()
        .map(|(model, entry)| {
            Some((
                model.clone(),
                (
                    usage_count(&entry["inputTokens"])?
                        .checked_add(usage_count(&entry["cacheReadInputTokens"]).unwrap_or(0))?
                        .checked_add(
                            usage_count(&entry["cacheCreationInputTokens"]).unwrap_or(0),
                        )?,
                    usage_count(&entry["outputTokens"])?,
                ),
            ))
        })
        .collect()
}
fn ledger_delta(
    previous: Option<&ClaudeModelLedger>,
    current: Option<&ClaudeModelLedger>,
) -> Option<(u64, u64)> {
    let (previous, current) = (previous?, current?);
    for (model, before) in previous {
        let after = current.get(model)?;
        if after.0 < before.0 || after.1 < before.1 {
            return None;
        }
    }
    current
        .iter()
        .try_fold((0_u64, 0_u64), |sum, (model, after)| {
            let before = previous.get(model).copied().unwrap_or_default();
            Some((
                sum.0.checked_add(after.0 - before.0)?,
                sum.1.checked_add(after.1 - before.1)?,
            ))
        })
}

fn turn_token_usage(
    result: &Value,
    context: &ProjectedTurn,
    status: &str,
    has_subagents: bool,
) -> ProviderResult<t3_contracts::provider_runtime::TurnTokenUsage> {
    let mut usage = result["usage"].clone();
    for field in [
        "input_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
        "output_tokens",
    ] {
        if !context.earlier_usage.is_empty() {
            let counts = std::iter::once(&result["usage"][field])
                .chain(context.earlier_usage.iter().map(|usage| &usage[field]))
                .filter_map(usage_count)
                .collect::<Vec<_>>();
            if !counts.is_empty() {
                if !usage.is_object() {
                    usage = json!({});
                }
                usage[field] = json!(
                    counts
                        .iter()
                        .try_fold(0_u64, |sum, count| sum.checked_add(*count))
                );
            }
        }
    }
    let thinking = std::iter::once(&result["usage"]["output_tokens_details"]["thinking_tokens"])
        .chain(
            context
                .earlier_usage
                .iter()
                .map(|usage| &usage["output_tokens_details"]["thinking_tokens"]),
        )
        .filter_map(usage_count)
        .collect::<Vec<_>>();
    let uncached = usage_count(&usage["input_tokens"]);
    let cached = usage_count(&usage["cache_read_input_tokens"]);
    let created = usage_count(&usage["cache_creation_input_tokens"]);
    let output = usage_count(&usage["output_tokens"]);
    let contribution = |field: &str, count: Option<u64>| {
        if usage[field].is_null() {
            Some(0)
        } else {
            count
        }
    };
    let input = uncached
        .zip(contribution("cache_read_input_tokens", cached))
        .zip(contribution("cache_creation_input_tokens", created))
        .and_then(|((a, b), c)| a.checked_add(b)?.checked_add(c));
    let mut value = json!({"usageScope":"main_agent","hasSubagents":has_subagents});
    if !context.root_models.is_empty() {
        value["models"] = json!(context.root_models);
    }
    if result["local_command"]
        .as_str()
        .is_some_and(|command| !command.is_empty())
    {
        value["localCommand"] = json!(true);
    }
    let known = [uncached, cached, created, output];
    if known.iter().all(Option::is_none)
        || (result["subtype"] != "success" && known.iter().flatten().all(|count| *count == 0))
    {
        value["usageStatus"] = json!("unavailable");
        return decode(value);
    }
    if let Some(cached) = cached {
        value["cachedInputTokens"] = json!(cached);
    }
    if let Some(created) = created {
        value["cacheCreationTokens"] = json!(created);
    }
    if let Some(output) = output {
        value["outputTokens"] = json!(output);
        if !thinking.is_empty() {
            value["reasoningTokens"] = json!(
                output.min(
                    thinking
                        .iter()
                        .fold(0_u64, |sum, count| sum.saturating_add(*count))
                )
            );
        }
    }
    if let Some(input) = input {
        value["inputTokens"] = json!(input);
    }
    let current_ledger = result["_t3_ledger_usage"]
        .as_array()
        .and_then(|counts| Some((counts.first()?.as_u64()?, counts.get(1)?.as_u64()?)));
    if let (Some(before), Some(current), Some(input), Some(output)) =
        (context.earlier_ledger, current_ledger, input, output)
        && let Some(total) = before
            .0
            .checked_add(current.0)
            .zip(before.1.checked_add(current.1))
        && total.0 >= input
        && total.1 >= output
    {
        value["otherUsage"] = json!({"inputTokens":total.0-input,"outputTokens":total.1-output});
    }
    let complete = status == "completed"
        && result["subtype"] == "success"
        && input.is_some()
        && output.is_some()
        && usage_count(&result["usage"]["input_tokens"]).is_some()
        && usage_count(&result["usage"]["output_tokens"]).is_some()
        && [
            "input_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
            "output_tokens",
        ]
        .iter()
        .all(|field| {
            result["usage"][field].is_null() || usage_count(&result["usage"][field]).is_some()
        });
    value["usageStatus"] = json!(if complete { "complete" } else { "partial" });
    decode(value)
}

#[derive(Clone)]
struct ProjectedTurn {
    input: TurnInput,
    turn: Value,
    started_at: String,
    stream_message_id: String,
    stream_usage: Value,
    stream_blocks: BTreeMap<u64, (String, String)>,
    ordinals: BTreeMap<String, u64>,
    interrupted: bool,
    compaction: Option<Value>,
    assistant_texts: BTreeSet<String>,
    rate_limits: BTreeSet<String>,
    failure_hint: Option<String>,
    root_models: Vec<String>,
    earlier_usage: Vec<Value>,
    earlier_ledger: Option<(u64, u64)>,
    steered: bool,
}

pub struct ClaudeProjection {
    pub thread: Option<OrchestrationV2ProviderThread>,
    pub turns: BTreeMap<String, OrchestrationV2ProviderTurn>,
    pub messages: BTreeMap<String, OrchestrationV2ConversationMessage>,
    pub requests: BTreeMap<String, OrchestrationV2RuntimeRequest>,
    pub items: BTreeMap<String, OrchestrationV2TurnItem>,
    active: Option<ProjectedTurn>,
    settled: Option<ProjectedTurn>,
    tasks: BTreeMap<String, Value>,
    launches: BTreeMap<String, Value>,
    launch_order: VecDeque<String>,
    launch_parents: BTreeMap<String, Option<String>>,
    launch_contexts: BTreeMap<String, ProjectedTurn>,
    task_tools: BTreeMap<String, String>,
    task_contexts: BTreeMap<String, ProjectedTurn>,
    task_roots: BTreeMap<String, NodeId>,
    task_calls: BTreeMap<String, String>,
    children: BTreeMap<String, ProjectedTurn>,
    child_threads: BTreeMap<String, Value>,
    routed: Option<(String, ProjectedTurn)>,
    buffered_children: BTreeMap<String, Vec<Value>>,
    tool_owners: BTreeMap<String, String>,
    request_threads: BTreeMap<String, ThreadId>,
    background: BTreeMap<String, Value>,
    known_background: BTreeMap<String, Value>,
    owed_background: BTreeMap<String, Value>,
    completed_background: BTreeSet<String>,
    background_calls: BTreeMap<String, (String, Value)>,
    running_monitors: BTreeSet<String>,
    steers: BTreeMap<String, SteerInput>,
    instance_id: ProviderInstanceId,
    session_id: ProviderSessionId,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    process_ledger: Option<ClaudeModelLedger>,
    pub max_tokens: Option<u64>,
    pub auto_compact_window: Option<u64>,
}
impl ClaudeProjection {
    pub fn new(
        instance_id: ProviderInstanceId,
        session_id: ProviderSessionId,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        Self {
            thread: None,
            turns: BTreeMap::new(),
            messages: BTreeMap::new(),
            requests: BTreeMap::new(),
            items: BTreeMap::new(),
            active: None,
            settled: None,
            tasks: BTreeMap::new(),
            launches: BTreeMap::new(),
            launch_order: VecDeque::new(),
            launch_parents: BTreeMap::new(),
            launch_contexts: BTreeMap::new(),
            task_tools: BTreeMap::new(),
            task_contexts: BTreeMap::new(),
            task_roots: BTreeMap::new(),
            task_calls: BTreeMap::new(),
            children: BTreeMap::new(),
            child_threads: BTreeMap::new(),
            routed: None,
            buffered_children: BTreeMap::new(),
            tool_owners: BTreeMap::new(),
            request_threads: BTreeMap::new(),
            background: BTreeMap::new(),
            known_background: BTreeMap::new(),
            owed_background: BTreeMap::new(),
            completed_background: BTreeSet::new(),
            background_calls: BTreeMap::new(),
            running_monitors: BTreeSet::new(),
            steers: BTreeMap::new(),
            instance_id,
            session_id,
            clock,
            ids,
            process_ledger: Some(BTreeMap::new()),
            max_tokens: None,
            auto_compact_window: None,
        }
    }
    pub fn active_turn_id(&self) -> Option<ProviderTurnId> {
        self.active
            .as_ref()
            .and_then(|v| v.turn["id"].as_str())
            .map(|s| ProviderTurnId(s.into()))
    }
    pub fn active_policy(&self) -> Option<&RuntimePolicy> {
        self.active.as_ref().map(|v| &v.input.runtime_policy)
    }
    pub fn active_input(&self) -> Option<&TurnInput> {
        self.active.as_ref().map(|v| &v.input)
    }
    pub fn pending_background_work(&self, include_subagents: bool) -> bool {
        !self.background.is_empty()
            || self.has_running_monitor()
            || (include_subagents
                && self.tasks.values().any(|v| {
                    matches!(
                        v["status"].as_str(),
                        Some("running" | "pending" | "waiting")
                    )
                }))
    }
    pub fn has_running_monitor(&self) -> bool {
        !self.running_monitors.is_empty()
    }
    pub fn has_unconsumed_native_wake(&self) -> bool {
        !self.owed_background.is_empty()
    }
    pub fn idle_native_wake_frame(&self, frame: &Value) -> bool {
        self.active.is_none()
            && (self.pending_background_work(true) || self.has_unconsumed_native_wake())
            && frame["parent_tool_use_id"].is_null()
            && (frame["type"] == "system" && frame["subtype"] == "init"
                || matches!(
                    frame["type"].as_str(),
                    Some("assistant" | "stream_event" | "user" | "result")
                ))
    }
    pub fn native_wake_notification(
        &self,
        frame: &Value,
    ) -> ProviderResult<Option<OrchestrationV2Notification>> {
        if frame["type"] != "system" || frame["subtype"] != "task_notification" {
            return Ok(None);
        }
        let Some(task) = frame["task_id"]
            .as_str()
            .and_then(|id| self.owed_background.get(id))
        else {
            return Ok(None);
        };
        let kind = task["kind"].as_str().unwrap_or("command");
        let mut source = json!({"kind":kind});
        if kind == "subagent" {
            source["childThreadId"] = task["childThreadId"].clone();
        }
        let outcome = match frame["status"].as_str() {
            Some("failed") => "failed",
            Some("stopped" | "cancelled") => "cancelled",
            _ => "completed",
        };
        let title = match kind {
            "monitor" => "Monitor",
            "subagent" => "Subagent",
            "background_task" => "Background task",
            _ => "Command",
        };
        let verb = match outcome {
            "failed" => "failed",
            "cancelled" => "stopped",
            _ => "finished",
        };
        let label = task["description"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(|label| format!(" \"{label}\""))
            .unwrap_or_default();
        Ok(Some(decode(
            json!({"source":source,"outcome":outcome,"summary":format!("{title}{label} {verb}")}),
        )?))
    }
    pub fn consume_native_wake(&mut self) -> ProviderResult<Vec<ProviderEvent>> {
        for id in self.owed_background.keys() {
            self.background.remove(id);
            self.completed_background.insert(id.clone());
        }
        self.owed_background.clear();
        let mut events = Vec::new();
        self.update_thread(
            if self.active.is_some() {
                "active"
            } else {
                "idle"
            },
            &self.clock.now().0,
            &mut events,
        )?;
        Ok(events)
    }
    pub fn ensure(
        &mut self,
        input: &EnsureThread,
        native_id: &str,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        if let Some(existing) = &input.existing_provider_thread
            && !input.force_new_native_thread
            && existing.native_thread_ref.is_some()
        {
            self.thread = Some(existing.clone());
            return Ok(existing.clone());
        }
        let now = self.clock.now();
        let thread = decode(
            json!({"id":provider_id("provider-thread","native-thread", native_id),"driver":"claudeAgent",
            "providerInstanceId":self.instance_id,"providerSessionId":self.session_id,"appThreadId":input.thread_id,"ownerNodeId":null,
            "nativeThreadRef":native_ref(native_id,"strong"),"nativeConversationHeadRef":null,"status":"idle","firstRunOrdinal":null,
            "lastRunOrdinal":null,"handoffIds":[],"forkedFrom":null,"createdAt":now,"updatedAt":now}),
        )?;
        self.thread = Some(thread);
        self.thread.clone().ok_or(ProviderError::SessionStopped)
    }
    pub fn snapshot(&self, thread: OrchestrationV2ProviderThread) -> ThreadSnapshot {
        ThreadSnapshot {
            provider_thread: thread,
            provider_turns: self.turns.values().cloned().collect(),
            messages: self.messages.values().cloned().collect(),
            reasoning_message_ids: None,
            runtime_requests: self.requests.values().cloned().collect(),
            provider_payload: None,
        }
    }
    pub fn reset_after_rollback(&mut self, thread: OrchestrationV2ProviderThread) {
        self.thread = Some(thread);
        self.active = None;
        self.settled = None;
        self.turns.clear();
        self.messages.clear();
        self.requests.clear();
        self.items.clear();
        self.tasks.clear();
        self.launches.clear();
        self.launch_order.clear();
        self.launch_parents.clear();
        self.launch_contexts.clear();
        self.task_tools.clear();
        self.task_contexts.clear();
        self.task_roots.clear();
        self.task_calls.clear();
        self.children.clear();
        self.child_threads.clear();
        self.routed = None;
        self.buffered_children.clear();
        self.tool_owners.clear();
        self.request_threads.clear();
        self.background.clear();
        self.known_background.clear();
        self.owed_background.clear();
        self.completed_background.clear();
        self.background_calls.clear();
        self.running_monitors.clear();
        self.steers.clear();
    }
    pub fn begin(&mut self, input: TurnInput) -> ProviderResult<Vec<ProviderEvent>> {
        if self.active.is_some() {
            return Err(ProviderError::Protocol {
                reason: Some(ProtocolReason::PreviousTurnActive),
                detail: "Claude has an active turn".into(),
                payload: None,
            });
        }
        let now = self.clock.now().0;
        let native = format!("turn:{}", self.ids.next(IdKind::ProviderTurn));
        let turn = json!({"id":provider_id("provider-turn","native-turn",&native),"providerThreadId":input.provider_thread.id,
            "nodeId":input.root_node_id,"runAttemptId":input.attempt_id,"nativeTurnRef":native_ref(&native,"weak"),
            "ordinal":input.provider_turn_ordinal,"status":"running","startedAt":now,"completedAt":null,"model":input.model_selection.model});
        self.thread = Some(input.provider_thread.clone());
        self.active = Some(ProjectedTurn {
            input,
            turn: turn.clone(),
            started_at: now.clone(),
            stream_message_id: String::new(),
            stream_usage: Value::Null,
            stream_blocks: BTreeMap::new(),
            ordinals: BTreeMap::new(),
            interrupted: false,
            compaction: None,
            assistant_texts: BTreeSet::new(),
            rate_limits: BTreeSet::new(),
            failure_hint: None,
            root_models: Vec::new(),
            earlier_usage: Vec::new(),
            earlier_ledger: Some((0, 0)),
            steered: false,
        });
        let mut events = Vec::new();
        self.emit_turn(turn, &mut events)?;
        self.update_thread("active", &now, &mut events)?;
        Ok(events)
    }
    pub fn remember_steer(
        &mut self,
        uuid: String,
        input: SteerInput,
    ) -> ProviderResult<Vec<ProviderEvent>> {
        if self.active_turn_id().as_ref() != Some(&input.provider_turn_id) {
            return Err(ProviderError::SteerDeclined(input.provider_turn_id));
        }
        let event = Self::event(ProviderEventKind::Steer {
            thread_id: input.thread_id.clone(),
            run_id: input.run_id.clone(),
            message_id: input.message.message_id.clone(),
            status: SteerStatus::Pending,
        });
        self.context_mut()?.steered = true;
        self.steers.insert(uuid, input);
        Ok(vec![event])
    }
    pub fn interrupt(&mut self) {
        if let Some(turn) = self.active.as_mut() {
            turn.interrupted = true;
        }
    }
    pub fn query_ended(&mut self) -> ProviderResult<Vec<ProviderEvent>> {
        let Some(active) = &self.active else {
            return Ok(vec![]);
        };
        let interrupted = active.interrupted;
        let mut events = Vec::new();
        self.finish(
            &json!({"type":"result","subtype":"error_during_execution",
            "errors":[CLAUDE_QUERY_ENDED_BEFORE_TURN_DETAIL]}),
            &self.clock.now().0,
            &mut events,
        )?;
        if !interrupted {
            for event in &mut events {
                if let ProviderEventKind::Terminal { disposition, .. } = &mut event.kind {
                    *disposition = ThreadDisposition::Broken;
                }
            }
        }
        Ok(events)
    }
    fn event(kind: ProviderEventKind) -> ProviderEvent {
        ProviderEvent {
            driver: ProviderDriverKind("claudeAgent".into()),
            kind,
        }
    }
    fn context(&self) -> ProviderResult<&ProjectedTurn> {
        self.routed
            .as_ref()
            .map(|(_, context)| context)
            .or(self.active.as_ref().or(self.settled.as_ref()))
            .ok_or(ProviderError::SessionStopped)
    }
    fn context_mut(&mut self) -> ProviderResult<&mut ProjectedTurn> {
        self.routed
            .as_mut()
            .map(|(_, context)| context)
            .or(self.active.as_mut().or(self.settled.as_mut()))
            .ok_or(ProviderError::SessionStopped)
    }
    fn ordinal(&mut self, key: &str) -> ProviderResult<u64> {
        let child = self.routed.is_some();
        let context = self.context_mut()?;
        if let Some(ordinal) = context.ordinals.get(key) {
            return Ok(*ordinal);
        }
        let base = if child {
            Some(101)
        } else {
            context
                .input
                .provider_turn_ordinal
                .checked_mul(100)
                .and_then(|base| base.checked_add(1))
        };
        let next = base
            .and_then(|base| base.checked_add(context.ordinals.len() as u64))
            .ok_or_else(|| operation(ProviderOperation::Stream, "Claude item ordinal overflow"))?;
        context.ordinals.insert(key.into(), next);
        Ok(next)
    }
    fn base_item(
        &mut self,
        native: &str,
        item_type: &str,
        status: &str,
        now: &str,
    ) -> ProviderResult<Value> {
        let ordinal = self.ordinal(native)?;
        let context = self.context()?;
        let mut item = json!({"id":provider_id("turn-item","native-item",native),"threadId":context.input.thread_id,
            "runId":context.input.run_id,"nodeId":context.input.root_node_id,"providerThreadId":context.input.provider_thread.id,
            "providerTurnId":context.turn["id"],"nativeItemRef":native_ref(native,"strong"),"parentItemId":null,
            "ordinal":ordinal,"type":item_type,"status":status,"title":null,"startedAt":context.started_at,
            "completedAt":if status == "running" || status == "waiting" { Value::Null } else { json!(now) },"updatedAt":now});
        if self.routed.is_some() {
            for key in ["runId", "providerThreadId", "providerTurnId"] {
                item[key] = Value::Null;
            }
        }
        Ok(item)
    }
    fn emit_item(&mut self, value: Value, events: &mut Vec<ProviderEvent>) -> ProviderResult<()> {
        let id = value["id"]
            .as_str()
            .ok_or_else(|| operation(ProviderOperation::Stream, "Missing item identity"))?
            .to_owned();
        let item: OrchestrationV2TurnItem = decode(value)?;
        self.items.insert(id, item.clone());
        events.push(Self::event(ProviderEventKind::TurnItem {
            item: Box::new(item),
            session_scoped: None,
        }));
        Ok(())
    }
    fn emit_turn(&mut self, value: Value, events: &mut Vec<ProviderEvent>) -> ProviderResult<()> {
        if self.routed.is_some() {
            return Ok(());
        }
        let turn: OrchestrationV2ProviderTurn = decode(value)?;
        let key = serde_json::to_value(&turn.id)
            .map_err(|_| operation(ProviderOperation::Stream, "Invalid turn identity"))?;
        self.turns
            .insert(key.as_str().unwrap_or_default().into(), turn.clone());
        events.push(Self::event(ProviderEventKind::Turn {
            provider_turn: Box::new(turn),
            thread_id: self.context().ok().map(|v| v.input.thread_id.clone()),
        }));
        Ok(())
    }
    fn update_thread(
        &mut self,
        status: &str,
        now: &str,
        events: &mut Vec<ProviderEvent>,
    ) -> ProviderResult<()> {
        if self.routed.is_some() {
            return Ok(());
        }
        if let Some(thread) = &self.thread {
            let mut value = serde_json::to_value(thread)
                .map_err(|_| operation(ProviderOperation::Stream, "Invalid thread"))?;
            value["status"] = json!(status);
            value["updatedAt"] = json!(now);
            if let Ok(context) = self.context() {
                if value["firstRunOrdinal"].is_null() {
                    value["firstRunOrdinal"] = json!(context.input.run_ordinal);
                }
                value["lastRunOrdinal"] = json!(context.input.run_ordinal);
            }
            let mut roster = self.background.clone();
            roster.extend(
                self.owed_background
                    .iter()
                    .filter(|(_, task)| task["kind"] != "subagent")
                    .map(|(id, task)| (id.clone(), task.clone())),
            );
            for task in roster.values_mut() {
                if let Some(task) = task.as_object_mut() {
                    task.remove("nativeKind");
                }
            }
            value["pendingBackgroundTasks"] = json!(roster.values().collect::<Vec<_>>());
            let thread = decode(value)?;
            self.thread = Some(thread);
            events.push(Self::event(ProviderEventKind::Thread {
                provider_thread: Box::new(
                    self.thread.clone().ok_or(ProviderError::SessionStopped)?,
                ),
                pending_background_work: Some(self.pending_background_work(true)),
                session_scoped: Some(self.active.is_none()),
                retained_native_turn_ids: None,
            }));
        }
        Ok(())
    }
    fn assistant(
        &mut self,
        native: &str,
        text: &str,
        streaming: bool,
        now: &str,
        events: &mut Vec<ProviderEvent>,
    ) -> ProviderResult<()> {
        if text.is_empty() {
            return Ok(());
        }
        let context = self.context()?.clone();
        let node_id = provider_id("node", "native-item", native);
        let message_id = provider_id("message", "native-item", native);
        let status = if streaming { "running" } else { "completed" };
        let completed = if streaming { Value::Null } else { json!(now) };
        let mut node = json!({"id":node_id,"threadId":context.input.thread_id,"runId":context.input.run_id,
            "parentNodeId":context.input.root_node_id,"rootNodeId":context.input.root_node_id,"kind":"assistant_message","status":status,
            "countsForRun":false,"providerThreadId":context.input.provider_thread.id,"providerTurnId":context.turn["id"],
            "nativeItemRef":native_ref(native,"strong"),"runtimeRequestId":null,"checkpointScopeId":null,"startedAt":context.started_at,"completedAt":completed});
        if self.routed.is_some() {
            for key in ["runId", "providerThreadId", "providerTurnId"] {
                node[key] = Value::Null;
            }
        }
        let node = decode(node)?;
        let created_at = self
            .messages
            .get(&message_id)
            .map(|v| v.created_at.0.as_str())
            .unwrap_or(now);
        let mut message = json!({"id":message_id,"threadId":context.input.thread_id,
            "runId":context.input.run_id,"nodeId":node_id,"role":"assistant","text":text,"attachments":[],"streaming":streaming,
            "createdBy":"agent","creationSource":"provider","createdAt":created_at,"updatedAt":now});
        if self.routed.is_some() {
            message["runId"] = Value::Null;
        }
        let message: OrchestrationV2ConversationMessage = decode(message)?;
        self.messages.insert(message_id.clone(), message.clone());
        let mut item = self.base_item(native, "assistant_message", status, now)?;
        item["nodeId"] = json!(node_id);
        item["messageId"] = json!(message_id);
        item["text"] = json!(text);
        item["streaming"] = json!(streaming);
        events.push(Self::event(ProviderEventKind::Node(Box::new(node))));
        events.push(Self::event(ProviderEventKind::Message(Box::new(message))));
        self.emit_item(item, events)?;
        if !streaming {
            self.context_mut()?
                .assistant_texts
                .insert(text.split_whitespace().collect::<Vec<_>>().join(" "));
        }
        Ok(())
    }
    fn reasoning(
        &mut self,
        native: &str,
        text: &str,
        streaming: bool,
        now: &str,
        events: &mut Vec<ProviderEvent>,
    ) -> ProviderResult<()> {
        let mut item = self.base_item(
            native,
            "reasoning",
            if streaming { "running" } else { "completed" },
            now,
        )?;
        item["title"] = json!("Thinking");
        item["text"] = json!(text);
        item["streaming"] = json!(streaming);
        self.emit_item(item, events)
    }
    fn usage(
        &mut self,
        usage: &Value,
        now: &str,
        events: &mut Vec<ProviderEvent>,
    ) -> ProviderResult<()> {
        let requested = self.context()?.started_at.clone();
        let token = claude_provider_turn_token_usage(
            usage,
            self.max_tokens,
            now,
            self.auto_compact_window,
            Some(&requested),
        )?;
        let mut turn = self.context()?.turn.clone();
        turn["tokenUsage"] = json!(token);
        self.context_mut()?.turn = turn.clone();
        self.emit_turn(turn, events)
    }
    pub fn handle_message(
        &mut self,
        message: &t3_claude_cli::message::Message,
    ) -> ProviderResult<Vec<ProviderEvent>> {
        self.handle_frame(message.raw())
    }
    pub fn reset_process_usage(&mut self) {
        self.process_ledger = Some(BTreeMap::new());
    }
    pub fn observe_result(&mut self, frame: &Value) -> Value {
        let mut frame = frame.clone();
        if frame["type"] == "result" {
            let ledger = model_ledger(&frame["modelUsage"]);
            let delta = ledger_delta(self.process_ledger.as_ref(), ledger.as_ref());
            self.process_ledger = if delta.is_none() && self.process_ledger.is_some() {
                None
            } else {
                ledger
            };
            frame["_t3_ledger_usage"] = json!(delta.map(|(input, output)| [input, output]));
        }
        frame
    }
    pub fn handle_frame(&mut self, frame: &Value) -> ProviderResult<Vec<ProviderEvent>> {
        let observed;
        let frame = if frame["type"] == "result" && frame.get("_t3_ledger_usage").is_none() {
            observed = self.observe_result(frame);
            &observed
        } else {
            frame
        };
        let mut events = Vec::new();
        let now = self.clock.now().0;
        if self.routed.is_some() && frame["type"] == "result" {
            return Ok(events);
        }
        if frame["type"] == "prompt_suggestion" {
            if self.active.is_none()
                && let (Some(settled), Some(text)) = (
                    &self.settled,
                    frame["suggestion"]
                        .as_str()
                        .map(str::trim)
                        .filter(|v| !v.is_empty()),
                )
            {
                events.push(Self::event(ProviderEventKind::PromptSuggestion {
                    thread_id: settled.input.thread_id.clone(),
                    run_id: settled.input.run_id.clone(),
                    text: text.into(),
                }));
            }
            return Ok(events);
        }
        if self.active.is_none() && self.settled.is_none() {
            return Ok(events);
        }
        if frame["type"] == "system"
            && frame["subtype"] == "init"
            && let Some(native) = frame["session_id"].as_str()
            && let Some(thread) = &mut self.thread
        {
            thread.native_thread_ref = Some(decode(native_ref(native, "strong"))?);
        }
        if frame["type"] == "user"
            && let Some(uuid) = frame["uuid"].as_str()
            && let Some(steer) = self.steers.remove(uuid)
        {
            events.push(Self::event(ProviderEventKind::Steer {
                thread_id: steer.thread_id,
                run_id: steer.run_id,
                message_id: steer.message.message_id,
                status: SteerStatus::Completed,
            }));
        }
        let parent = frame["parent_tool_use_id"].as_str();
        if parent.is_some() {
            return self.subagent_frame(frame, &now);
        }
        match frame["type"].as_str() {
            Some("stream_event") => self.stream(&frame["event"], &now, &mut events)?,
            Some("assistant") => {
                let body = &frame["message"];
                if self.routed.is_none()
                    && let Some(model) = body["model"]
                        .as_str()
                        .filter(|model| *model != "<synthetic>")
                {
                    let context = self.context_mut()?;
                    if !context.root_models.iter().any(|observed| observed == model) {
                        context.root_models.push(model.into());
                    }
                }
                if frame["error"].is_string() {
                    let text = body["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|block| block["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !text.trim().is_empty() {
                        self.context_mut()?.failure_hint = Some(text);
                    }
                }
                let native = body["id"]
                    .as_str()
                    .or_else(|| frame["uuid"].as_str())
                    .unwrap_or("assistant");
                if body["model"] != "<synthetic>" && body["usage"].is_object() {
                    let stream = &self.context()?.stream_usage;
                    if stream.is_null() || self.context()?.stream_message_id != native {
                        self.usage(&body["usage"], &now, &mut events)?;
                    }
                }
                let mut text = String::new();
                if let Some(content) = body["content"].as_array() {
                    for (index, block) in content.iter().enumerate() {
                        match block["type"].as_str() {
                            Some("text") => {
                                text.push_str(block["text"].as_str().unwrap_or_default())
                            }
                            Some("thinking") => self.reasoning(
                                &format!("{native}:thinking:{index}"),
                                block["thinking"].as_str().unwrap_or_default(),
                                false,
                                &now,
                                &mut events,
                            )?,
                            Some("redacted_thinking") => {}
                            Some("tool_use" | "server_tool_use" | "mcp_tool_use") => {
                                self.tool_start(block, &now, &mut events)?
                            }
                            Some(kind) if kind.ends_with("tool_result") => self.tool_result(
                                block,
                                frame.get("tool_use_result"),
                                &now,
                                &mut events,
                            )?,
                            _ => {}
                        }
                    }
                }
                self.assistant(native, &text, false, &now, &mut events)?;
                if let Some(uuid) = frame["uuid"].as_str() {
                    let mut turn = self.context()?.turn.clone();
                    turn["nativeTurnRef"] = native_ref(uuid, "weak");
                    self.context_mut()?.turn = turn;
                    if self.routed.is_none()
                        && let Some(thread) = &mut self.thread
                    {
                        thread.native_conversation_head_ref =
                            Some(decode(native_ref(uuid, "strong"))?);
                    }
                }
            }
            Some("user") => {
                if let Some(content) = frame["message"]["content"].as_array() {
                    for block in content {
                        if block["type"]
                            .as_str()
                            .is_some_and(|v| v.ends_with("tool_result"))
                        {
                            self.tool_result(
                                block,
                                frame.get("tool_use_result"),
                                &now,
                                &mut events,
                            )?;
                        }
                    }
                }
            }
            Some("system") => self.system(frame, &now, &mut events)?,
            Some("rate_limit_event") => {
                let info = &frame["rate_limit_info"];
                let overage = info["isUsingOverage"] == true
                    || info["overageInUse"] == true
                    || matches!(
                        info["overageStatus"].as_str(),
                        Some("allowed" | "allowed_warning")
                    );
                if info["status"] == "rejected" && !overage {
                    let key = format!("{}:{}", info["rateLimitType"], info["resetsAt"]);
                    if self.context_mut()?.rate_limits.insert(key.clone()) {
                        let text = "Claude usage limit reached";
                        self.context_mut()?.failure_hint = Some(text.into());
                        let mut item = self.base_item(
                            &format!("usage-limit:{key}"),
                            "system_notice",
                            "completed",
                            &now,
                        )?;
                        item["title"] = json!(text);
                        item["message"] = json!(text);
                        self.emit_item(item, &mut events)?;
                    }
                }
            }
            Some("result") if self.active.is_some() => self.finish(frame, &now, &mut events)?,
            _ => {}
        }
        Ok(events)
    }
    fn stream(
        &mut self,
        event: &Value,
        now: &str,
        events: &mut Vec<ProviderEvent>,
    ) -> ProviderResult<()> {
        let index = event["index"].as_u64().unwrap_or(0);
        match event["type"].as_str() {
            Some("message_start") => {
                let context = self.context_mut()?;
                context.stream_message_id =
                    event["message"]["id"].as_str().unwrap_or_default().into();
                context.stream_blocks.clear();
                context.stream_usage = event["message"]["usage"].clone();
                if event["message"]["usage"].is_object() {
                    self.usage(&event["message"]["usage"], now, events)?;
                }
            }
            Some("message_delta") => {
                if event["usage"].is_object() {
                    let usage =
                        merge_claude_stream_usage(&self.context()?.stream_usage, &event["usage"]);
                    self.context_mut()?.stream_usage = usage.clone();
                    self.usage(&usage, now, events)?;
                }
            }
            Some("content_block_start") => {
                let block = &event["content_block"];
                if matches!(block["type"].as_str(), Some("text" | "thinking")) {
                    let kind = block["type"].as_str().unwrap_or_default();
                    let text = block[if kind == "thinking" {
                        "thinking"
                    } else {
                        "text"
                    }]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                    self.context_mut()?
                        .stream_blocks
                        .insert(index, (kind.into(), text));
                    self.emit_stream_block(index, true, now, events)?;
                }
            }
            Some("content_block_delta") => {
                let delta = &event["delta"];
                let text = if delta["type"] == "thinking_delta" {
                    delta["thinking"].as_str()
                } else {
                    delta["text"].as_str()
                };
                if let Some(text) = text {
                    if let Some((_, current)) = self.context_mut()?.stream_blocks.get_mut(&index) {
                        current.push_str(text);
                    }
                    self.emit_stream_block(index, true, now, events)?;
                }
            }
            Some("content_block_stop") => self.emit_stream_block(index, false, now, events)?,
            _ => {}
        }
        Ok(())
    }
    fn emit_stream_block(
        &mut self,
        index: u64,
        streaming: bool,
        now: &str,
        events: &mut Vec<ProviderEvent>,
    ) -> ProviderResult<()> {
        let context = self.context()?;
        let Some((kind, text)) = context.stream_blocks.get(&index).cloned() else {
            return Ok(());
        };
        let id = context.stream_message_id.clone();
        if kind == "thinking" {
            self.reasoning(
                &format!("{id}:thinking:{index}"),
                &text,
                streaming,
                now,
                events,
            )
        } else {
            let text = context
                .stream_blocks
                .values()
                .filter(|(kind, _)| kind == "text")
                .map(|(_, text)| text.as_str())
                .collect::<String>();
            self.assistant(&id, &text, streaming, now, events)
        }
    }
    fn tool_start(
        &mut self,
        block: &Value,
        now: &str,
        events: &mut Vec<ProviderEvent>,
    ) -> ProviderResult<()> {
        let Some(native) = block["id"].as_str() else {
            return Ok(());
        };
        let name = block["name"].as_str().unwrap_or("Tool");
        if let Some((task, _)) = &self.routed {
            self.tool_owners.insert(native.into(), task.clone());
        }
        let input = &block["input"];
        if matches!(name, "Bash" | "Monitor" | "ScheduleWakeup") {
            self.background_calls
                .insert(native.into(), (name.into(), input.clone()));
        }
        if matches!(name, "Agent" | "Task" | "SendMessage") {
            let launch_context = self
                .routed
                .as_ref()
                .and_then(|(id, _)| self.task_contexts.get(id))
                .cloned()
                .unwrap_or(self.context()?.clone());
            self.launch_contexts
                .entry(native.into())
                .or_insert(launch_context);
            self.launches.insert(native.into(), input.clone());
            self.launch_parents.insert(
                native.into(),
                self.routed.as_ref().map(|(id, _)| id.clone()),
            );
            if !self.launch_order.iter().any(|id| id == native) {
                self.launch_order.push_back(native.into());
            }
            while self.launches.len() > 64 {
                if let Some(oldest) = self.launch_order.pop_front() {
                    self.launches.remove(&oldest);
                    self.launch_parents.remove(&oldest);
                    self.launch_contexts.remove(&oldest);
                }
            }
            return Ok(());
        }
        if matches!(name, "TodoWrite" | "ExitPlanMode") {
            return self.plan(native, name, input, now, events);
        }
        let (kind, _) = tool_classification(name);
        let mut item = self.base_item(native, kind, "running", now)?;
        item["title"] = json!(input["description"].as_str().unwrap_or(name));
        match kind {
            "command_execution" => {
                item["input"] = json!(input["command"].as_str().unwrap_or_default());
                item["output"] = json!("");
                item["exitCode"] = Value::Null;
            }
            "file_change" => {
                item["fileName"] = json!(
                    input["file_path"]
                        .as_str()
                        .or_else(|| input["notebook_path"].as_str())
                        .unwrap_or(name)
                );
                if let Some(old) = input.get("old_string") {
                    item["oldStr"] = old.clone();
                }
                if let Some(new) = input.get("new_string").or_else(|| input.get("content")) {
                    item["newStr"] = new.clone();
                }
            }
            "web_search" => {
                item["patterns"] = json!([input["query"]
                    .as_str()
                    .or_else(|| input["url"].as_str())
                    .unwrap_or_default()]);
            }
            _ => {
                item["toolName"] = json!(name);
                item["input"] = input.clone();
            }
        }
        self.emit_tool_item(item, events)
    }
    fn tool_result(
        &mut self,
        block: &Value,
        structured: Option<&Value>,
        now: &str,
        events: &mut Vec<ProviderEvent>,
    ) -> ProviderResult<()> {
        let Some(native) = block["tool_use_id"].as_str() else {
            return Ok(());
        };
        if let Some(task_id) = self.task_tools.get(native).cloned()
            && let Some(result) = structured
                .filter(|result| result["status"] == "completed" && result["content"].is_array())
        {
            return self.task(
                &json!({"subtype":"task_notification","task_id":task_id,"tool_use_id":native,
                    "status":"completed","summary":output_text(&result["content"])}),
                now,
                events,
            );
        }
        let id = provider_id("turn-item", "native-item", native);
        let Some(prior) = self.items.get(&id) else {
            return Ok(());
        };
        let mut item = serde_json::to_value(prior)
            .map_err(|_| operation(ProviderOperation::Stream, "Invalid tool item"))?;
        let failed = block["is_error"] == true
            || block["content"]["type"]
                .as_str()
                .is_some_and(|s| s.ends_with("_error"));
        item["status"] = json!(if failed { "failed" } else { "completed" });
        item["completedAt"] = json!(now);
        item["updatedAt"] = json!(now);
        let output = structured
            .filter(|v| !v.is_null())
            .unwrap_or(&block["content"]);
        match item["type"].as_str() {
            Some("command_execution") => {
                item["output"] = json!(output_text(output));
                item["exitCode"] = structured
                    .and_then(|v| v.get("exitCode").or_else(|| v.get("exit_code")))
                    .cloned()
                    .unwrap_or(Value::Null);
            }
            Some("dynamic_tool") => item["output"] = output.clone(),
            Some("web_search") => {
                let results = output["results"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|result| result["content"].as_array())
                    .flatten()
                    .filter(|result| result["title"].is_string() && result["url"].is_string())
                    .map(|result| json!({"title":result["title"],"url":result["url"]}))
                    .collect::<Vec<_>>();
                if !results.is_empty() {
                    item["results"] = json!(results);
                }
            }
            _ => {}
        }
        self.emit_tool_item(item, events)
    }
    fn emit_tool_item(
        &mut self,
        mut item: Value,
        events: &mut Vec<ProviderEvent>,
    ) -> ProviderResult<()> {
        let context = self.context()?;
        let native = item["nativeItemRef"]["nativeId"]
            .as_str()
            .ok_or_else(|| operation(ProviderOperation::Stream, "Missing tool identity"))?;
        item["nodeId"] = json!(provider_id("node", "native-item", native));
        let node = decode(json!({
            "id": item["nodeId"], "threadId": item["threadId"], "runId": item["runId"],
            "parentNodeId": context.input.root_node_id, "rootNodeId": context.input.root_node_id,
            "kind": "tool_call", "status": item["status"], "countsForRun": false,
            "providerThreadId": item["providerThreadId"], "providerTurnId": item["providerTurnId"],
            "nativeItemRef": item["nativeItemRef"], "runtimeRequestId": null, "checkpointScopeId": null,
            "startedAt": item["startedAt"], "completedAt": item["completedAt"]
        }))?;
        events.push(Self::event(ProviderEventKind::Node(Box::new(node))));
        self.emit_item(item, events)
    }
    pub fn plan(
        &mut self,
        native: &str,
        name: &str,
        input: &Value,
        now: &str,
        events: &mut Vec<ProviderEvent>,
    ) -> ProviderResult<()> {
        let kind = if name == "ExitPlanMode" {
            "proposed_plan"
        } else {
            "todo_list"
        };
        let mut item = self.base_item(native, kind, "completed", now)?;
        let id = format!("plan:{}", encoded_part(native));
        item["planId"] = json!(id);
        let context = self.context()?;
        let mut plan = json!({"id":id,"threadId":context.input.thread_id,"runId":context.input.run_id,"nodeId":context.input.root_node_id,
            "providerTurnId":context.turn["id"],"kind":kind,"createdAt":now,"updatedAt":now,"status":"completed"});
        if kind == "proposed_plan" {
            item["streaming"] = json!(false);
            let Some(markdown) = input["plan"].as_str().filter(|s| !s.trim().is_empty()) else {
                return Ok(());
            };
            item["markdown"] = json!(markdown);
            plan["markdown"] = json!(markdown);
            plan["steps"] = json!([]);
        } else {
            let steps = input["todos"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .filter_map(|(index, v)| {
                    let text = v["content"].as_str()?.trim();
                    if text.is_empty() {
                        return None;
                    }
                    let status = match v["status"].as_str() {
                        Some("completed") => "completed",
                        Some("in_progress") => "running",
                        _ => "pending",
                    };
                    Some(json!({"id":format!("step:{}",index+1),"text":text,"status":status}))
                })
                .collect::<Vec<_>>();
            item["steps"] = json!(steps);
            plan["steps"] = json!(steps);
            plan["markdown"] = Value::Null;
        }
        events.push(Self::event(ProviderEventKind::Plan(Box::new(decode(
            plan,
        )?))));
        self.emit_item(item, events)
    }
    fn system(
        &mut self,
        frame: &Value,
        now: &str,
        events: &mut Vec<ProviderEvent>,
    ) -> ProviderResult<()> {
        let subtype = frame["subtype"].as_str().unwrap_or_default();
        let native = frame["uuid"].as_str().unwrap_or(subtype);
        match subtype {
            "task_started" | "task_progress" | "task_notification" => {
                self.task(frame, now, events)?
            }
            "background_tasks_changed" => {
                if let Some(tasks) = frame["tasks"]
                    .as_array()
                    .or_else(|| frame["background_tasks"].as_array())
                {
                    let previous = std::mem::take(&mut self.background);
                    for task in tasks {
                        if let Some(id) = task["task_id"].as_str().or_else(|| task["id"].as_str()) {
                            if task["task_type"] != "local_bash"
                                || self.completed_background.contains(id)
                            {
                                continue;
                            }
                            let known=self.known_background.get(id).cloned().unwrap_or_else(||json!({"taskId":id,"kind":"command","nativeKind":task["task_type"].as_str().unwrap_or("local_bash"),"description":task["description"].as_str().unwrap_or("Background task")}));
                            if let Ok(context) = self.context() {
                                let context = context.clone();
                                self.task_contexts.entry(id.into()).or_insert(context);
                            }
                            self.background.insert(id.into(), known.clone());
                            self.known_background.insert(id.into(), known);
                        }
                    }
                    for (id, task) in previous {
                        if !self.background.contains_key(&id) {
                            self.owed_background.entry(id).or_insert(task);
                        }
                    }
                }
                self.update_thread(
                    if self.active.is_some() {
                        "active"
                    } else {
                        "idle"
                    },
                    now,
                    events,
                )?;
            }
            "compact_boundary" | "status" => {
                if subtype == "compact_boundary" || frame["status"] == "compacting" {
                    let mut item = if let Some(item) = self.context()?.compaction.clone() {
                        item
                    } else {
                        self.base_item(native, "compaction", "running", now)?
                    };
                    item["driver"] = json!("claudeAgent");
                    item["title"] = json!("Compacting context");
                    if subtype == "compact_boundary" {
                        item["status"] = json!("completed");
                        item["completedAt"] = json!(now);
                        item["title"] = json!("Context compacted");
                        for (source, target) in [
                            ("pre_tokens", "beforeTokenCount"),
                            ("post_tokens", "afterTokenCount"),
                        ] {
                            if frame["compact_metadata"][source].is_number() {
                                item[target] = frame["compact_metadata"][source].clone();
                            }
                        }
                        self.context_mut()?.compaction = None;
                    } else {
                        self.context_mut()?.compaction = Some(item.clone());
                    }
                    self.emit_item(item, events)?;
                } else if let Some(mut item) = self.context_mut()?.compaction.take() {
                    let status = match frame["compact_result"].as_str() {
                        Some("success") => "completed",
                        Some("failed") => "failed",
                        _ => "interrupted",
                    };
                    item["status"] = json!(status);
                    item["completedAt"] = json!(now);
                    item["updatedAt"] = json!(now);
                    item["title"] = json!(match status {
                        "completed" => "Context compacted",
                        "failed" => "Context compaction failed",
                        _ => "Context compaction stopped",
                    });
                    if frame["compact_error"].is_string() {
                        item["summary"] = frame["compact_error"].clone();
                    }
                    self.emit_item(item, events)?;
                }
            }
            "informational" | "model_refusal_fallback" if frame["level"] != "info" => {
                let text = frame["content"].as_str().unwrap_or_default();
                if text.starts_with("Stop says: ") && frame["prevent_continuation"] != true {
                    return Ok(());
                }
                let mut item = self.base_item(native, "system_notice", "completed", now)?;
                item["title"] = json!(text);
                item["message"] = json!(text);
                self.emit_item(item, events)?;
            }
            _ => {}
        }
        Ok(())
    }
    fn task(
        &mut self,
        frame: &Value,
        now: &str,
        events: &mut Vec<ProviderEvent>,
    ) -> ProviderResult<()> {
        let Some(id) = frame["task_id"].as_str() else {
            return Ok(());
        };
        let tool = frame["tool_use_id"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| self.task_calls.get(id).cloned())
            .or_else(|| {
                self.task_tools
                    .iter()
                    .find_map(|(tool, task)| (task == id).then(|| tool.clone()))
            });
        let parent_task = tool
            .as_ref()
            .and_then(|tool| self.launch_parents.get(tool))
            .and_then(|id| id.as_ref());
        let subtype = frame["subtype"].as_str().unwrap_or_default();
        let new_call = subtype == "task_started"
            && tool
                .as_ref()
                .is_some_and(|tool| self.task_calls.get(id) != Some(tool));
        if subtype == "task_started"
            && self
                .tasks
                .get(id)
                .is_some_and(|task| task["status"] != "running")
            && !new_call
        {
            return Ok(());
        }
        let resume_tool = if self.tasks.contains_key(id) && new_call {
            tool.clone()
        } else {
            None
        };
        let launch_context = if subtype == "task_started" {
            tool.as_ref()
                .and_then(|tool| self.launch_contexts.get(tool))
        } else {
            None
        };
        let context = launch_context
            .or_else(|| self.task_contexts.get(id))
            .or_else(|| parent_task.and_then(|id| self.task_contexts.get(id)))
            .cloned()
            .unwrap_or(self.context()?.clone());
        self.task_contexts.insert(id.into(), context.clone());
        let kind = frame["task_type"]
            .as_str()
            .or_else(|| {
                self.known_background
                    .get(id)
                    .and_then(|t| t["nativeKind"].as_str())
            })
            .unwrap_or("local_agent");
        if kind == "local_bash" {
            if subtype == "task_started" {
                self.completed_background.remove(id);
                let monitor = tool
                    .as_ref()
                    .and_then(|id| self.background_calls.get(id))
                    .is_some_and(|(name, _)| name == "Monitor");
                if monitor {
                    self.running_monitors.insert(id.into());
                }
                let entry = json!({"taskId":id,"kind":if monitor {"monitor"}else{"command"},"nativeKind":kind,"description":frame["description"].as_str().unwrap_or("Background task")});
                if frame["is_backgrounded"] != false {
                    self.background.insert(id.into(), entry.clone());
                }
                self.known_background.insert(id.into(), entry);
            } else if subtype == "task_notification" {
                self.running_monitors.remove(id);
                self.background.remove(id);
                if !self.completed_background.contains(id)
                    && self
                        .active
                        .as_ref()
                        .is_none_or(|active| active.turn["id"] != context.turn["id"])
                {
                    if let Some(task) = self.known_background.get(id) {
                        self.owed_background.insert(id.into(), task.clone());
                    }
                } else {
                    self.owed_background.remove(id);
                    self.completed_background.insert(id.into());
                }
            }
            return self.update_thread(
                if self.active.is_some() {
                    "active"
                } else {
                    "idle"
                },
                now,
                events,
            );
        }
        let was_running = self.tasks.get(id).is_some_and(|task| {
            matches!(
                task["status"].as_str(),
                Some("running" | "pending" | "waiting")
            )
        });
        self.task_roots
            .entry(id.into())
            .or_insert_with(|| context.input.root_node_id.clone());
        let native = format!("task:{id}");
        let launch = frame["tool_use_id"]
            .as_str()
            .and_then(|key| self.launches.get(key))
            .cloned()
            .unwrap_or(Value::Null);
        let mut task = self.tasks.get(id).cloned().unwrap_or_else(|| json!({"id":provider_id("node","native-item",&native),
            "threadId":context.input.thread_id,"runId":context.input.run_id,"parentNodeId":context.input.root_node_id,
            "origin":"provider_native","createdBy":"agent","driver":"claudeAgent","providerInstanceId":self.instance_id,
            "providerThreadId":null,"childThreadId":provider_id("thread","native-thread",&format!("{}:{id}",serde_json::to_value(&context.input.provider_thread.id).ok().and_then(|v|v.as_str().map(str::to_owned)).unwrap_or_default())),"nativeTaskRef":native_ref(id,"strong"),
            "prompt":launch["prompt"].as_str().unwrap_or_default(),"title":frame["description"],"model":launch["model"],
            "status":"running","result":null,"startedAt":now,"completedAt":null,"updatedAt":now}));
        if let Some(parent) = parent_task.and_then(|id| self.tasks.get(id)) {
            task["parentNodeId"] = parent["id"].clone();
            if task["model"].is_null() {
                task["model"] = parent["model"].clone();
            }
        }
        if task["model"].is_null() {
            task["model"] = json!(context.input.model_selection.model);
        }
        task["modelSelection"] = json!({"instanceId":self.instance_id,"model":task["model"]});
        if let Some(tool) = &tool {
            self.task_tools.insert(tool.clone(), id.into());
        }
        if subtype == "task_started" {
            if let Some(tool) = &tool {
                self.task_calls.insert(id.into(), tool.clone());
            }
            task["runId"] = json!(context.input.run_id);
            task["status"] = json!("running");
            task["completedAt"] = Value::Null;
            if resume_tool.is_some() {
                task["result"] = Value::Null;
                task["startedAt"] = json!(now);
                if let Some(object) = task.as_object_mut() {
                    object.remove("progress");
                }
            }
            if frame["prompt"].is_string() {
                task["prompt"] = frame["prompt"].clone();
            }
        } else if subtype == "task_notification" {
            task["status"] = json!(match frame["status"].as_str() {
                Some("failed") => "failed",
                Some("stopped" | "cancelled") => "cancelled",
                _ => "completed",
            });
            task["result"] = frame["summary"].clone();
            task["completedAt"] = json!(now);
        } else if frame["description"].is_string() {
            task["progress"] = frame["description"].clone();
        }
        task["updatedAt"] = json!(now);
        use super::claude_workflow_progress::*;
        let previous = task
            .get("workflow")
            .and_then(|v| serde_json::from_value(v.clone()).ok());
        let workflow = merge_claude_workflow_progress(
            previous.as_ref(),
            frame["workflow_name"].as_str(),
            None,
            frame,
        );
        if let Some(workflow) = workflow {
            task["workflow"] = json!(workflow);
        }
        self.tasks.insert(id.into(), task.clone());
        if subtype == "task_notification"
            && was_running
            && self
                .active
                .as_ref()
                .is_none_or(|active| active.turn["id"] != context.turn["id"])
            && task["parentNodeId"] == json!(context.input.root_node_id)
        {
            self.owed_background.insert(id.into(),json!({"taskId":id,"kind":"subagent","childThreadId":task["childThreadId"],"description":task["title"]}));
        }
        self.project_child(id, &task, &context, now, resume_tool.as_deref(), events)?;
        let node = json!({"id":task["id"],"threadId":task["threadId"],"runId":task["runId"],"parentNodeId":task["parentNodeId"],"rootNodeId":self.task_roots.get(id),"kind":"subagent","status":task["status"],"countsForRun":false,"providerThreadId":context.input.provider_thread.id,"providerTurnId":context.turn["id"],"nativeItemRef":task["nativeTaskRef"],"runtimeRequestId":null,"checkpointScopeId":null,"startedAt":task["startedAt"],"completedAt":task["completedAt"]});
        events.push(Self::event(ProviderEventKind::Node(Box::new(decode(
            node,
        )?))));
        events.push(Self::event(ProviderEventKind::Subagent(Box::new(decode(
            task.clone(),
        )?))));
        let item_native = format!("{native}:subagent");
        let mut item = if let Some(prior) =
            self.items
                .get(&provider_id("turn-item", "native-item", &item_native))
        {
            serde_json::to_value(prior)
                .map_err(|_| operation(ProviderOperation::Stream, "Invalid subagent item"))?
        } else {
            self.base_item(
                &item_native,
                "subagent",
                task["status"].as_str().unwrap_or("running"),
                now,
            )?
        };
        item["status"] = task["status"].clone();
        item["updatedAt"] = json!(now);
        item["nativeItemRef"] = task["nativeTaskRef"].clone();
        item["threadId"] = task["threadId"].clone();
        item["runId"] = task["runId"].clone();
        item["nodeId"] = task["id"].clone();
        item["providerThreadId"] = json!(context.input.provider_thread.id);
        item["providerTurnId"] = context.turn["id"].clone();
        item["startedAt"] = task["startedAt"].clone();
        item["completedAt"] = task["completedAt"].clone();
        for key in [
            "origin",
            "driver",
            "providerInstanceId",
            "childThreadId",
            "prompt",
            "result",
            "title",
            "progress",
        ] {
            if let Some(value) = task.get(key) {
                item[key] = value.clone();
            }
        }
        item["subagentId"] = task["id"].clone();
        self.emit_item(item, events)?;
        events.push(Self::event(ProviderEventKind::BackgroundWork {
            provider_turn_id: decode(context.turn["id"].clone())?,
            pending: self.pending_background_work(true),
        }));
        if let Some(tool) = tool
            && let Some(buffered) = self.buffered_children.remove(&tool)
        {
            for frame in buffered {
                events.extend(self.subagent_frame(&frame, now)?);
            }
        }
        Ok(())
    }
    fn project_child(
        &mut self,
        id: &str,
        task: &Value,
        context: &ProjectedTurn,
        now: &str,
        resume_tool: Option<&str>,
        events: &mut Vec<ProviderEvent>,
    ) -> ProviderResult<()> {
        let child = task["childThreadId"]
            .as_str()
            .ok_or(ProviderError::SessionStopped)?;
        let root = provider_id("node", "native-item", &format!("task:{id}:thread-root"));
        let fresh = !self.child_threads.contains_key(id);
        let mut thread = if let Some(thread) = self.child_threads.get(id) {
            thread.clone()
        } else {
            let mut thread = serde_json::to_value(&context.input.app_thread)
                .map_err(|_| operation(ProviderOperation::Stream, "Invalid parent thread"))?;
            thread["id"] = json!(child);
            thread["title"] = json!(
                task["title"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or("Subagent")
            );
            thread["createdBy"] = json!("agent");
            thread["creationSource"] = json!("provider");
            thread["createdAt"] = task["startedAt"].clone();
            thread["activeProviderThreadId"] = Value::Null;
            thread["lineage"] = json!({"parentThreadId":context.input.thread_id,"relationshipToParent":"subagent","rootThreadId":context.input.app_thread.lineage.root_thread_id});
            thread["forkedFrom"] = json!({"type":"node","nodeId":task["id"]});
            for key in [
                "titleRegeneration",
                "linkedPullRequest",
                "historyOrigin",
                "archivedAt",
                "settledOverride",
                "settledAt",
                "snoozedUntil",
                "snoozedAt",
                "pinnedAt",
                "pinOrderKey",
                "autoSettleDisabledAt",
                "guestShare",
                "lastVisitedAt",
                "deletedAt",
            ] {
                thread[key] = Value::Null;
            }
            thread["pullRequests"] = json!([]);
            thread
        };
        thread["providerInstanceId"] = json!(self.instance_id);
        thread["modelSelection"] = task["modelSelection"].clone();
        thread["updatedAt"] = json!(now);
        if fresh
            || self
                .child_threads
                .get(id)
                .is_some_and(|old| old["modelSelection"] != thread["modelSelection"])
        {
            events.push(Self::event(ProviderEventKind::AppThread {
                app_thread: Box::new(decode(thread.clone())?),
                create_only: None,
            }));
        }
        self.child_threads.insert(id.into(), thread);
        events.push(Self::event(ProviderEventKind::Node(Box::new(decode(json!({"id":root,"threadId":child,"runId":null,"parentNodeId":null,"rootNodeId":root,"kind":"root_turn","status":task["status"],"countsForRun":false,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":task["nativeTaskRef"],"runtimeRequestId":null,"checkpointScopeId":null,"startedAt":task["startedAt"],"completedAt":task["completedAt"]}))?))));
        if fresh {
            let mut child_context = context.clone();
            child_context.input.thread_id = ThreadId(child.into());
            child_context.input.root_node_id = NodeId(root.clone());
            child_context.ordinals.clear();
            child_context.stream_blocks.clear();
            child_context.stream_message_id.clear();
            child_context.assistant_texts.clear();
            child_context.input.model_selection = decode(task["modelSelection"].clone())?;
            self.children.insert(id.into(), child_context);
        } else if resume_tool.is_some()
            && let Some(child) = self.children.get_mut(id)
        {
            child.input.run_id = context.input.run_id.clone();
            child.input.runtime_policy = context.input.runtime_policy.clone();
            child.input.model_selection = decode(task["modelSelection"].clone())?;
            child.turn = context.turn.clone();
            child.started_at = now.into();
            child.stream_blocks.clear();
            child.stream_message_id.clear();
            child.stream_usage = Value::Null;
            child.assistant_texts.clear();
        }
        if fresh || resume_tool.is_some() {
            let native = resume_tool
                .map(|tool| format!("task:{id}:prompt:{tool}"))
                .unwrap_or_else(|| format!("task:{id}:prompt"));
            let ordinal = if fresh {
                100
            } else {
                let context = self
                    .children
                    .get_mut(id)
                    .ok_or(ProviderError::SessionStopped)?;
                let ordinal = (context.ordinals.len() as u64)
                    .checked_add(101)
                    .ok_or_else(|| {
                        operation(ProviderOperation::Stream, "Claude child ordinal overflow")
                    })?;
                context.ordinals.insert(native.clone(), ordinal);
                ordinal
            };
            let message_id = provider_id("message", "native-item", &native);
            let message = decode(
                json!({"id":message_id,"threadId":child,"runId":null,"nodeId":root,"role":"user","senderThreadId":context.input.thread_id,"text":task["prompt"],"attachments":[],"streaming":false,"createdBy":"agent","creationSource":"provider","createdAt":now,"updatedAt":now}),
            )?;
            events.push(Self::event(ProviderEventKind::Message(Box::new(message))));
            let item = json!({"id":provider_id("turn-item","native-item",&native),"threadId":child,"runId":null,"nodeId":root,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":native_ref(&native,"strong"),"parentItemId":null,"ordinal":ordinal,"type":"user_message","status":"completed","title":null,"startedAt":now,"completedAt":now,"updatedAt":now,"messageId":message_id,"text":task["prompt"],"attachments":[],"createdBy":"agent","creationSource":"provider","inputIntent":"turn_start"});
            self.emit_item(item, events)?;
        }
        if task["status"] != "running"
            && let Some(text) = task["result"].as_str().filter(|s| !s.is_empty())
            && !self.children.get(id).is_some_and(|child| {
                child
                    .assistant_texts
                    .contains(&text.split_whitespace().collect::<Vec<_>>().join(" "))
            })
            && let Some(child_context) = self.children.remove(id)
        {
            self.routed = Some((id.into(), child_context));
            let result = self.assistant(&format!("task:{id}:result"), text, false, now, events);
            if let Some((id, context)) = self.routed.take() {
                self.children.insert(id, context);
            }
            result?;
        }
        Ok(())
    }
    fn subagent_frame(&mut self, frame: &Value, now: &str) -> ProviderResult<Vec<ProviderEvent>> {
        let parent = frame["parent_tool_use_id"].as_str().unwrap_or_default();
        let Some(task_id) = self.task_tools.get(parent).cloned() else {
            let buffered = self.buffered_children.entry(parent.into()).or_default();
            if buffered.len() < 256 {
                buffered.push(frame.clone());
            }
            return Ok(vec![]);
        };
        let mut events = Vec::new();
        if frame["type"] == "assistant"
            && let Some(model) = frame["message"]["model"]
                .as_str()
                .filter(|m| *m != "<synthetic>")
            && let Some(mut task) = self.tasks.get(&task_id).cloned()
        {
            let model = model
                .rsplit_once('-')
                .filter(|(_, suffix)| {
                    suffix.len() == 8 && suffix.bytes().all(|b| b.is_ascii_digit())
                })
                .map(|(base, _)| base)
                .unwrap_or(model);
            task["model"] = json!(model);
            task["modelSelection"] = json!({"instanceId":self.instance_id,"model":model});
            task["updatedAt"] = json!(now);
            if let Some(context) = self.task_contexts.get(&task_id).cloned() {
                self.project_child(&task_id, &task, &context, now, None, &mut events)?;
            }
            self.tasks.insert(task_id.clone(), task.clone());
            events.push(Self::event(ProviderEventKind::Subagent(Box::new(decode(
                task,
            )?))));
        }
        let Some(child) = self.children.remove(&task_id) else {
            return Ok(events);
        };
        self.routed = Some((task_id, child));
        let mut child_frame = frame.clone();
        child_frame["parent_tool_use_id"] = Value::Null;
        let result = self.handle_frame(&child_frame);
        if let Some((id, context)) = self.routed.take() {
            self.children.insert(id, context);
        }
        events.extend(result?);
        Ok(events)
    }
    pub fn complete_absorbed_wake(&mut self) -> ProviderResult<Vec<ProviderEvent>> {
        let mut events = Vec::new();
        self.finish_result(&Value::Null, &self.clock.now().0, &mut events, true)?;
        Ok(events)
    }
    pub fn finish(
        &mut self,
        result: &Value,
        now: &str,
        events: &mut Vec<ProviderEvent>,
    ) -> ProviderResult<()> {
        self.finish_result(result, now, events, false)
    }
    fn finish_result(
        &mut self,
        result: &Value,
        now: &str,
        events: &mut Vec<ProviderEvent>,
        absorbed: bool,
    ) -> ProviderResult<()> {
        let Some(context) = self.active.as_ref() else {
            return Ok(());
        };
        if let Some(echoed) = result["user_message_uuids"].as_array()
            && !echoed.is_empty()
            && !echoed.iter().any(|v| {
                v.as_str() == Some(&context.input.message.message_id.0)
                    || v.as_str().is_some_and(|s| self.steers.contains_key(s))
            })
        {
            return Ok(());
        }
        if context.steered
            && !context.interrupted
            && matches!(
                result["terminal_reason"].as_str(),
                Some("aborted_streaming" | "aborted_tools")
            )
        {
            let growth = result["_t3_ledger_usage"]
                .as_array()
                .and_then(|counts| Some((counts.first()?.as_u64()?, counts.get(1)?.as_u64()?)));
            let context = self.context_mut()?;
            context.earlier_usage.push(result["usage"].clone());
            context.earlier_ledger = context
                .earlier_ledger
                .zip(growth)
                .and_then(|(a, b)| Some((a.0.checked_add(b.0)?, a.1.checked_add(b.1)?)));
            return Ok(());
        }
        let context = self.context()?;
        let hint = context.failure_hint.clone();
        let status = if context.interrupted {
            "interrupted"
        } else if absorbed {
            "completed"
        } else {
            terminal_status_from_result(result, hint.as_deref())
        };
        if let Some(text) = result["result"].as_str().filter(|s| !s.trim().is_empty()) {
            let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if status == "completed" && !context.assistant_texts.contains(&normalized) {
                let native = result["uuid"].as_str().unwrap_or("result");
                self.assistant(native, text, false, now, events)?;
            }
        }
        let mut turn = self.context()?.turn.clone();
        turn["status"] = json!(status);
        turn["completedAt"] = json!(now);
        let run = &self.context()?.input.run_id;
        let has_subagents = self.tasks.values().any(|task| task["runId"] == json!(run));
        turn["turnTokenUsage"] = json!(turn_token_usage(
            result,
            self.context()?,
            status,
            has_subagents
        )?);
        self.context_mut()?.turn = turn.clone();
        self.emit_turn(turn.clone(), events)?;
        self.update_thread("idle", now, events)?;
        let outcome = match status {
            "interrupted" => TerminalOutcome::Interrupted { reason: None },
            "cancelled" => TerminalOutcome::Cancelled { reason: None },
            "failed" => {
                let message =
                    terminal_result_error(result["terminal_reason"].as_str(), hint.as_deref())
                        .or_else(|| {
                            result["errors"].as_array().and_then(|v| {
                                v.iter()
                                    .filter_map(Value::as_str)
                                    .find(|s| !s.starts_with("[ede_diagnostic]"))
                                    .map(str::to_owned)
                            })
                        })
                        .or(hint)
                        .unwrap_or_else(|| "Claude turn failed".into());
                let api_status = result["api_error_status"].as_i64();
                let message = match api_status {
                    Some(401) => "Claude could not authenticate. For subscription login, run `claude auth login` on this environment's machine, then start a new thread. For API-key authentication, check this instance's configured credentials.".into(),
                    Some(429) => "Claude API rate limit reached. Try again later.".into(),
                    Some(529) => "Claude API is overloaded (529). Try again shortly.".into(),
                    _ => message,
                };
                let code = api_status
                    .map(|status| format!("api_error_{status}"))
                    .or_else(|| {
                        result[if result["subtype"] == "success" {
                            "terminal_reason"
                        } else {
                            "subtype"
                        }]
                        .as_str()
                        .map(str::to_owned)
                    });
                TerminalOutcome::Failed {
                    failure: Box::new(decode(
                        json!({"class":if api_status == Some(429) {"usage_limit"}else{"provider_error"},"message":message,
                            "code":code,"retryable":if matches!(api_status,Some(429|529)) {json!(true)}else{Value::Null}}),
                    )?),
                    failure_item_ordinal: self.ordinal("terminal-failure")?,
                    retry: None,
                    retry_started_at: None,
                    child_turns_continue: self.pending_background_work(true),
                }
            }
            _ => TerminalOutcome::Completed { reason: None },
        };
        let context = self.context()?;
        events.push(Self::event(ProviderEventKind::Terminal {
            provider_thread_id: decode(turn["providerThreadId"].clone())?,
            provider_turn_id: decode(turn["id"].clone())?,
            run_ordinal: context.input.run_ordinal,
            disposition: ThreadDisposition::Reusable,
            outcome,
        }));
        for (_, steer) in std::mem::take(&mut self.steers) {
            events.push(Self::event(ProviderEventKind::Steer {
                thread_id: steer.thread_id,
                run_id: steer.run_id,
                message_id: steer.message.message_id,
                status: SteerStatus::Cancelled,
            }));
        }
        self.settled = self.active.take();
        Ok(())
    }
    pub fn pending_request(
        &mut self,
        native: &str,
        name: &str,
        input: &Value,
        kind: &str,
        prompt: &str,
    ) -> ProviderResult<(RuntimeRequestId, Vec<ProviderEvent>)> {
        let now = self.clock.now().0;
        let request_id = RuntimeRequestId(self.ids.next(IdKind::RuntimeRequest));
        let node_id = format!("node:runtime-request:{}", encoded_part(&request_id.0));
        let context = self.context()?.clone();
        let request: OrchestrationV2RuntimeRequest = decode(
            json!({"id":request_id,"nodeId":node_id,"providerTurnId":context.turn["id"],
            "nativeRequestRef":native_ref(native,"strong"),"kind":kind,"status":"pending","responseCapability":{"type":"live","providerSessionId":self.session_id,"supportsFeedback":true},
            "createdAt":now,"resolvedAt":null}),
        )?;
        let mut node = json!({"id":node_id,"threadId":context.input.thread_id,"runId":context.input.run_id,
            "parentNodeId":context.input.root_node_id,"rootNodeId":context.input.root_node_id,"kind":if kind == "user_input" {"user_input_request"}else{"approval_request"},"status":"waiting",
            "countsForRun":false,"providerThreadId":context.input.provider_thread.id,"providerTurnId":context.turn["id"],
            "nativeItemRef":native_ref(native,"strong"),"runtimeRequestId":request_id,"checkpointScopeId":null,"startedAt":now,"completedAt":null});
        if self.routed.is_some() {
            for key in ["runId", "providerThreadId", "providerTurnId"] {
                node[key] = Value::Null;
            }
        }
        let node = decode(node)?;
        self.requests.insert(request_id.0.clone(), request.clone());
        self.request_threads
            .insert(request_id.0.clone(), context.input.thread_id.clone());
        let mut item = self.base_item(
            &format!("request:{}", request_id.0),
            if kind == "user_input" {
                "user_input_request"
            } else {
                "approval_request"
            },
            "waiting",
            &now,
        )?;
        item["requestId"] = json!(request_id);
        item["nodeId"] = json!(node_id);
        item["title"] = json!(name);
        if kind == "user_input" {
            item["questions"] = json!(claude_user_input_questions(input));
        } else {
            item["requestKind"] = json!(kind);
            item["prompt"] = json!(prompt);
        }
        let mut events = vec![
            Self::event(ProviderEventKind::Node(Box::new(node))),
            Self::event(ProviderEventKind::RuntimeRequest {
                request: Box::new(request),
                thread_id: Some(context.input.thread_id),
            }),
        ];
        self.emit_item(item, &mut events)?;
        Ok((request_id, events))
    }
    pub fn has_permission_owner(&self, agent: Option<&str>, tool: &str) -> bool {
        let owner = agent.or_else(|| self.tool_owners.get(tool).map(String::as_str));
        match owner {
            Some(id) => {
                self.children.contains_key(id)
                    && self
                        .tasks
                        .get(id)
                        .is_some_and(|task| task["status"] == "running")
            }
            None => self.active.is_some(),
        }
    }
    pub fn pending_request_for_agent(
        &mut self,
        agent: Option<&str>,
        native: &str,
        name: &str,
        input: &Value,
        kind: &str,
        prompt: &str,
    ) -> ProviderResult<(RuntimeRequestId, Vec<ProviderEvent>)> {
        let owner = agent
            .filter(|id| self.children.contains_key(*id))
            .map(str::to_owned)
            .or_else(|| self.tool_owners.get(native).cloned());
        let Some((id, child)) =
            owner.and_then(|id| self.children.remove(&id).map(|child| (id, child)))
        else {
            return self.pending_request(native, name, input, kind, prompt);
        };
        self.routed = Some((id, child));
        let result = self.pending_request(native, name, input, kind, prompt);
        if let Some((id, child)) = self.routed.take() {
            self.children.insert(id, child);
        }
        result
    }
    pub fn resolve_request(
        &mut self,
        id: &RuntimeRequestId,
        response: Option<&RuntimeResponse>,
    ) -> ProviderResult<Vec<ProviderEvent>> {
        let now = self.clock.now().0;
        let Some(request) = self.requests.get_mut(&id.0) else {
            return Ok(vec![]);
        };
        if request.status != RuntimeRequestStatus::Pending {
            return Ok(vec![]);
        }
        request.status = if response.is_some() {
            RuntimeRequestStatus::Resolved
        } else {
            RuntimeRequestStatus::Cancelled
        };
        request.resolved_at = Some(t3_contracts::wire::WireDateTime(now.clone()));
        if let Some(response) = response {
            request.decision = response.decision.map(Some);
            request.answers = response.answers.clone().map(Some);
            request.feedback = response.feedback.clone().map(Some);
        }
        let mut events = vec![Self::event(ProviderEventKind::RuntimeRequest {
            request: Box::new(request.clone()),
            thread_id: self.request_threads.get(&id.0).cloned(),
        })];
        let items = self
            .items
            .values()
            .filter_map(|item| serde_json::to_value(item).ok())
            .filter(|v| v["requestId"] == json!(id))
            .collect::<Vec<_>>();
        for mut item in items {
            item["status"] = json!(if response.is_some() {
                "completed"
            } else {
                "cancelled"
            });
            item["completedAt"] = json!(now);
            item["updatedAt"] = json!(now);
            self.emit_item(item, &mut events)?;
        }
        Ok(events)
    }
}

pub fn output_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(values) => values
            .iter()
            .filter_map(|v| v["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(_) => {
            let mut parts = ["stdout", "stderr"]
                .into_iter()
                .filter_map(|key| value[key].as_str())
                .collect::<Vec<_>>();
            if let Some(text) = value["text"].as_str() {
                parts.push(text);
            }
            if parts.is_empty() {
                value.to_string()
            } else {
                parts.join("\n")
            }
        }
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub fn claude_turn_cursor(turn: &OrchestrationV2ProviderTurn) -> Option<String> {
    let native = turn.native_turn_ref.as_ref()?;
    (native.driver.0 == "claudeAgent").then_some(())?;
    native
        .native_id
        .as_ref()
        .map(|id| id.0.clone())
        .filter(|id| !id.starts_with("turn:"))
}

pub fn claude_fork_cursor(input: &ForkInput) -> ProviderResult<Option<String>> {
    let (Some(target), Some(turns)) = (&input.provider_turn_id, &input.source_provider_turns)
    else {
        return Ok(None);
    };
    let mut turns = turns
        .iter()
        .filter(|turn| turn.provider_thread_id == input.source_provider_thread.id)
        .collect::<Vec<_>>();
    turns.sort_by_key(|turn| turn.ordinal);
    let index = turns
        .iter()
        .position(|turn| serde_json::to_value(&turn.id).ok() == serde_json::to_value(target).ok())
        .ok_or_else(|| {
            operation(
                ProviderOperation::Fork,
                "Source turn was not found in this provider thread",
            )
        })?;
    if let Some(cursor) = claude_turn_cursor(turns[index]) {
        return Ok(Some(cursor));
    }
    if turns[index + 1..]
        .iter()
        .any(|turn| turn.status != ProviderTurnStatus::Running)
    {
        return Err(operation(
            ProviderOperation::Fork,
            "No SDK assistant message cursor was recorded for this prior turn",
        ));
    }
    Ok(input
        .source_provider_thread
        .native_conversation_head_ref
        .as_ref()
        .and_then(|r| r.native_id.as_ref())
        .map(|id| id.0.clone()))
}

pub struct ClaudeForkTranscript {
    pub entries: Vec<Value>,
    pub head_message_id: Option<String>,
}
fn queued_source(row: &Value) -> Option<&str> {
    (row["type"] == "attachment" && row["attachment"]["type"] == "queued_command")
        .then(|| row["attachment"]["source_uuid"].as_str())
        .flatten()
}
fn transcript_uuid(row: &Value) -> &str {
    row["uuid"].as_str().unwrap_or_default()
}
fn transcript_blocks<'a>(row: &'a Value, kind: &str, key: &str) -> Vec<&'a str> {
    row["message"]["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == kind)
        .filter_map(|b| b[key].as_str())
        .collect()
}
fn visible_claude_history(rows: &[Value]) -> (Vec<String>, Option<String>) {
    let mut by_id = rows
        .iter()
        .map(|row| (transcript_uuid(row).to_owned(), row.clone()))
        .collect::<BTreeMap<_, _>>();
    for row in rows
        .iter()
        .filter(|r| r["type"] == "system" && r["subtype"] == "compact_boundary")
    {
        let metadata = &row["compactMetadata"];
        if let Some(ids) = metadata["preservedMessages"]["uuids"]
            .as_array()
            .filter(|ids| {
                !ids.is_empty()
                    && ids
                        .iter()
                        .all(|id| id.as_str().is_some_and(|id| by_id.contains_key(id)))
            })
        {
            let anchor = &metadata["preservedMessages"]["anchorUuid"];
            let mut parent = anchor.clone();
            for id in ids {
                if let Some(row) = id.as_str().and_then(|id| by_id.get_mut(id)) {
                    row["parentUuid"] = parent;
                    parent = id.clone();
                }
            }
            for (id, row) in &mut by_id {
                if row["parentUuid"] == *anchor
                    && Some(id.as_str()) != ids.first().and_then(Value::as_str)
                {
                    row["parentUuid"] = parent.clone();
                }
            }
        } else if metadata["preservedSegment"].is_object() {
            let segment = &metadata["preservedSegment"];
            if let Some(row) = segment["headUuid"]
                .as_str()
                .and_then(|id| by_id.get_mut(id))
            {
                row["parentUuid"] = segment["anchorUuid"].clone();
            }
            for (id, row) in &mut by_id {
                if row["parentUuid"] == segment["anchorUuid"]
                    && Some(id.as_str()) != segment["headUuid"].as_str()
                {
                    row["parentUuid"] = segment["tailUuid"].clone();
                }
            }
        }
    }
    let eligible = |row: &Value| {
        row["isSidechain"] != true
            && row["teamName"].as_str().is_none_or(str::is_empty)
            && row["type"] != "progress"
            && !(row["type"] == "attachment" && row["attachment"]["type"] == "fork_briefing")
    };
    let parents = by_id
        .values()
        .filter(|r| eligible(r))
        .filter_map(|r| r["parentUuid"].as_str())
        .collect::<BTreeSet<_>>();
    let mut leaf = None;
    let mut terminal = None;
    for row in rows
        .iter()
        .rev()
        .filter_map(|row| by_id.get(transcript_uuid(row)))
        .filter(|row| eligible(row) && !parents.contains(transcript_uuid(row)))
    {
        let mut current = Some(row);
        let mut seen = BTreeSet::new();
        while let Some(row) = current {
            if !seen.insert(transcript_uuid(row)) {
                break;
            }
            if matches!(row["type"].as_str(), Some("user" | "assistant")) {
                leaf = Some(transcript_uuid(row).to_owned());
                break;
            }
            current = row["parentUuid"].as_str().and_then(|id| by_id.get(id));
        }
        if leaf.is_some() {
            terminal = Some(transcript_uuid(row).to_owned());
            break;
        }
    }
    if leaf.is_none() {
        leaf = rows
            .iter()
            .rev()
            .find(|r| {
                matches!(r["type"].as_str(), Some("user" | "assistant"))
                    && eligible(r)
                    && r["isMeta"] != true
            })
            .map(|r| transcript_uuid(r).to_owned())
            .or_else(|| {
                rows.iter()
                    .rev()
                    .find(|r| matches!(r["type"].as_str(), Some("user" | "assistant")))
                    .map(|r| transcript_uuid(r).to_owned())
            });
    }
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = leaf.as_deref().and_then(|id| by_id.get(id));
    while let Some(row) = current {
        let id = transcript_uuid(row);
        if !seen.insert(id.to_owned()) {
            break;
        }
        chain.push(id.to_owned());
        current = row["parentUuid"].as_str().and_then(|id| by_id.get(id));
    }
    chain.reverse();
    // SDK assistant chunks share an API message id; sibling results can sit off the parent chain.
    let assistant_groups = chain
        .iter()
        .filter_map(|id| by_id.get(id))
        .filter(|r| r["type"] == "assistant")
        .filter_map(|r| r["message"]["id"].as_str())
        .collect::<BTreeSet<_>>();
    let mut extra = BTreeMap::<String, Vec<String>>::new();
    for group in assistant_groups {
        let group_rows = rows
            .iter()
            .filter(|r| r["type"] == "assistant" && r["message"]["id"] == group)
            .collect::<Vec<_>>();
        let ids = group_rows
            .iter()
            .map(|r| transcript_uuid(r))
            .collect::<BTreeSet<_>>();
        let tools = group_rows
            .iter()
            .flat_map(|r| transcript_blocks(r, "tool_use", "id"))
            .collect::<BTreeSet<_>>();
        let Some(anchor) = chain.iter().rev().find(|id| ids.contains(id.as_str())) else {
            continue;
        };
        let mut siblings = rows
            .iter()
            .filter(|r| {
                !seen.contains(transcript_uuid(r))
                    && (ids.contains(transcript_uuid(r))
                        || (r["type"] == "user"
                            && (r["parentUuid"].as_str().is_some_and(|id| ids.contains(id))
                                || r["sourceToolAssistantUUID"]
                                    .as_str()
                                    .is_some_and(|id| ids.contains(id))
                                || transcript_blocks(r, "tool_result", "tool_use_id")
                                    .iter()
                                    .any(|id| tools.contains(id)))))
            })
            .collect::<Vec<_>>();
        siblings.sort_by(|a, b| {
            a["timestamp"]
                .as_str()
                .unwrap_or_default()
                .cmp(b["timestamp"].as_str().unwrap_or_default())
        });
        extra.insert(
            anchor.clone(),
            siblings
                .into_iter()
                .map(|r| transcript_uuid(r).to_owned())
                .collect(),
        );
    }
    let mut result = Vec::new();
    for id in chain {
        result.push(id.clone());
        if let Some(ids) = extra.remove(&id) {
            for id in ids {
                if seen.insert(id.clone()) {
                    result.push(id);
                }
            }
        }
    }
    (result, terminal)
}
fn prune_claude_fork(rows: Vec<Value>) -> Vec<Value> {
    let Some(last) = rows.last() else { return rows };
    let mut neutral = rows.clone();
    for row in &mut neutral {
        row["isSidechain"] = json!(false);
        if let Some(row) = row.as_object_mut() {
            row.remove("teamName");
        }
    }
    neutral.push(json!({"type":"system","uuid":"fork-boundary-sentinel","parentUuid":last["uuid"],"sessionId":last["sessionId"]}));
    let (expected, _) = visible_claude_history(&neutral);
    let expected = expected.into_iter().collect::<BTreeSet<_>>();
    let index = rows
        .iter()
        .enumerate()
        .map(|(i, r)| (transcript_uuid(r), i))
        .collect::<BTreeMap<_, _>>();
    let Some(start) = rows
        .iter()
        .position(|r| expected.contains(transcript_uuid(r)))
    else {
        return rows;
    };
    let by_id = rows
        .iter()
        .map(|r| (transcript_uuid(r), r))
        .collect::<BTreeMap<_, _>>();
    let mut live = BTreeSet::new();
    for id in expected
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(transcript_uuid(last)))
    {
        let mut row = by_id.get(id).copied();
        while let Some(current) = row {
            if !live.insert(transcript_uuid(current)) {
                break;
            }
            row = current["parentUuid"]
                .as_str()
                .and_then(|id| by_id.get(id).copied());
        }
    }
    let removable = |id: &str| !live.contains(id) && index.get(id).is_some_and(|i| *i >= start);
    let mut removed = BTreeSet::new();
    for row in rows.iter().filter(|r| {
        r["type"] == "system" && r["subtype"] == "compact_boundary" && removable(transcript_uuid(r))
    }) {
        let metadata = &row["compactMetadata"];
        let retained = metadata["preservedMessages"]["uuids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .chain(
                [
                    metadata["preservedSegment"]["headUuid"].as_str(),
                    metadata["preservedSegment"]["tailUuid"].as_str(),
                ]
                .into_iter()
                .flatten(),
            )
            .any(|id| live.contains(id));
        if !retained {
            let mut row = Some(row);
            while let Some(current) = row {
                let id = transcript_uuid(current);
                if !removable(id) || !removed.insert(id) {
                    break;
                }
                row = current["parentUuid"]
                    .as_str()
                    .and_then(|id| by_id.get(id).copied());
            }
        }
    }
    loop {
        let before = removed.len();
        for row in &rows {
            if row["parentUuid"]
                .as_str()
                .is_some_and(|id| removed.contains(id))
            {
                removed.insert(transcript_uuid(row));
            }
        }
        if before == removed.len() {
            break;
        }
    }
    rows.iter()
        .filter(|r| !removed.contains(transcript_uuid(r)))
        .cloned()
        .collect()
}

pub fn fork_claude_records(
    records: &[Value],
    source_session: &str,
    new_session: &str,
    up_to: Option<&str>,
    now: &str,
    mut uuid: impl FnMut() -> String,
) -> ProviderResult<ClaudeForkTranscript> {
    let mut rows = records
        .iter()
        .filter(|r| {
            matches!(
                r["type"].as_str(),
                Some("user" | "assistant" | "attachment" | "system" | "progress")
            ) && r["uuid"].is_string()
                && r["isSidechain"] != true
        })
        .cloned()
        .collect::<Vec<_>>();
    if let Some(cursor) = up_to {
        let (visible, _) = visible_claude_history(&rows);
        let index = rows
            .iter()
            .enumerate()
            .map(|(i, r)| (transcript_uuid(r).to_owned(), i))
            .collect::<BTreeMap<_, _>>();
        let tail = visible.iter().filter_map(|id| index.get(id)).max().copied();
        let mut candidates = visible
            .iter()
            .filter_map(|id| index.get(id).copied())
            .chain(
                rows.iter()
                    .enumerate()
                    .filter(|(i, _)| tail.is_none_or(|tail| *i > tail))
                    .map(|(i, _)| i),
            );
        let boundary = candidates
            .find(|i| {
                transcript_uuid(&rows[*i]) == cursor || queued_source(&rows[*i]) == Some(cursor)
            })
            .or_else(|| {
                rows.iter()
                    .position(|r| transcript_uuid(r) == cursor || queued_source(r) == Some(cursor))
            })
            .ok_or_else(|| {
                operation(
                    ProviderOperation::Fork,
                    "Fork message cursor was not found in the native transcript",
                )
            })?;
        rows.truncate(boundary + 1);
        rows = prune_claude_fork(rows);
    }
    let mapping = rows
        .iter()
        .map(|r| (transcript_uuid(r).to_owned(), uuid()))
        .collect::<BTreeMap<_, _>>();
    let by_id = rows
        .iter()
        .map(|r| (transcript_uuid(r), r))
        .collect::<BTreeMap<_, _>>();
    let mut entries = Vec::new();
    if records.iter().any(|r| r["type"] == "history-suppression") {
        entries.push(json!({"type":"history-suppression","sessionId":new_session,"cause":"fork_inherit","ts":now}));
    }
    let retained = rows
        .iter()
        .filter(|r| r["type"] != "progress")
        .collect::<Vec<_>>();
    if retained.is_empty() {
        return Err(operation(
            ProviderOperation::Fork,
            "Source session has no messages to fork",
        ));
    }
    let retained_ids = retained
        .iter()
        .map(|r| transcript_uuid(r))
        .collect::<BTreeSet<_>>();
    for (index, source) in retained.iter().enumerate() {
        let mut row = (*source).clone();
        let old = transcript_uuid(source);
        let mut parent = source["parentUuid"].as_str();
        let mut visited = BTreeSet::new();
        let mut resolved = None;
        while let Some(id) = parent {
            let Some(ancestor) = by_id.get(id) else { break };
            if ancestor["type"] != "progress" || !visited.insert(id) {
                resolved = mapping.get(id).cloned();
                break;
            }
            parent = ancestor["parentUuid"].as_str();
        }
        row["uuid"] = json!(mapping[old]);
        row["parentUuid"] = json!(resolved);
        row["sessionId"] = json!(new_session);
        row["isSidechain"] = json!(false);
        if row.get("logicalParentUuid").is_some() && !row["logicalParentUuid"].is_null() {
            row["logicalParentUuid"] = json!(
                row["logicalParentUuid"]
                    .as_str()
                    .and_then(|id| mapping.get(id))
            );
        }
        if index + 1 == retained.len() {
            row["timestamp"] = json!(now);
        }
        row["forkedFrom"] = json!({"sessionId":source_session,"messageUuid":old});
        if row["type"] == "system" && row["subtype"] == "model_refusal_fallback" {
            row["neutralizedByFork"] = json!(true);
        }
        if let Some(object) = row.as_object_mut() {
            for key in [
                "teamName",
                "agentName",
                "sessionKind",
                "slug",
                "sourceToolAssistantUUID",
                "summarizedTurnPromptUuids",
                "droppedPromptUuids",
                "turnAttributionKey",
            ] {
                object.remove(key);
            }
        }
        if row["attachment"]["type"] == "deferred_tools_record"
            && let Some(ids) = row["attachment"]["nameOnlyAnnouncements"].as_array()
        {
            row["attachment"]["nameOnlyAnnouncements"] = json!(
                ids.iter()
                    .filter_map(Value::as_str)
                    .filter_map(|id| mapping.get(id))
                    .collect::<Vec<_>>()
            );
        }
        if let Some(source_id) = queued_source(source).and_then(|id| mapping.get(id)) {
            row["attachment"]["source_uuid"] = json!(source_id);
        }
        if row["type"] == "system"
            && let Some(metadata) = row["compactMetadata"].as_object_mut()
        {
            metadata.remove("runningTurnSends");
            for (section, keys) in [
                (
                    "preservedMessages",
                    &["anchorUuid", "uuids", "allUuids"][..],
                ),
                (
                    "preservedSegment",
                    &["headUuid", "anchorUuid", "tailUuid"][..],
                ),
            ] {
                if let Some(section) = metadata.get_mut(section).and_then(Value::as_object_mut) {
                    for key in keys {
                        if let Some(value) = section.get_mut(*key) {
                            let remap = |v: &mut Value| {
                                if let Some(id) = v
                                    .as_str()
                                    .filter(|id| retained_ids.contains(id))
                                    .and_then(|id| mapping.get(id))
                                {
                                    *v = json!(id);
                                }
                            };
                            if let Some(values) = value.as_array_mut() {
                                for value in values {
                                    remap(value);
                                }
                            } else {
                                remap(value);
                            }
                        }
                    }
                }
            }
        }
        entries.push(row);
    }
    let source_records = records
        .iter()
        .filter(|r| r["sessionId"] == source_session)
        .collect::<Vec<_>>();
    let replacements = source_records
        .iter()
        .filter(|r| r["type"] == "content-replacement")
        .flat_map(|r| r["replacements"].as_array().into_iter().flatten())
        .filter(|r| r.is_object())
        .cloned()
        .collect::<Vec<_>>();
    if !replacements.is_empty() {
        entries.push(json!({"type":"content-replacement","sessionId":new_session,"replacements":replacements,"uuid":uuid(),"timestamp":now}));
    }
    for (kind, key) in [
        ("atis-latch", "atis"),
        ("tools-used", "tools"),
        ("relocated", "relocatedCwd"),
    ] {
        if let Some(value) = source_records
            .iter()
            .rev()
            .filter(|r| r["type"] == kind)
            .find_map(|r| r.get(key))
            .filter(|v| match kind {
                "atis-latch" => v
                    .as_str()
                    .is_some_and(|s| s.bytes().all(|b| (33..=126).contains(&b))),
                "tools-used" => v.is_array(),
                _ => v.as_str().is_some_and(|s| !s.is_empty()),
            })
        {
            entries.push(json!({"type":kind,"sessionId":new_session,key:value}));
        }
    }
    let title = source_records
        .iter()
        .rev()
        .find_map(|r| r["customTitle"].as_str().filter(|s| !s.is_empty()))
        .or_else(|| {
            source_records
                .iter()
                .rev()
                .find_map(|r| r["aiTitle"].as_str().filter(|s| !s.is_empty()))
        })
        .unwrap_or("Forked session");
    entries.push(json!({"type":"custom-title","sessionId":new_session,"customTitle":format!("{title} (fork)"),"uuid":uuid(),"timestamp":now}));
    let head_message_id = visible_claude_history(&rows)
        .0
        .iter()
        .rev()
        .find(|id| {
            by_id
                .get(id.as_str())
                .is_some_and(|r| matches!(r["type"].as_str(), Some("user" | "assistant")))
        })
        .and_then(|id| mapping.get(id))
        .cloned();
    Ok(ClaudeForkTranscript {
        entries,
        head_message_id,
    })
}

pub fn rollback_resume_cursor(input: &RollbackInput) -> ProviderResult<Option<String>> {
    match &input.target {
        RollbackTarget::ThreadStart { .. } => Ok(serde_json::to_value(&input.provider_thread)
            .map_err(|_| {
                operation(
                    ProviderOperation::Rollback,
                    "Invalid Claude provider thread",
                )
            })?["nativeMetadata"]["forkHeadMessageId"]
            .as_str()
            .map(str::to_owned)),
        RollbackTarget::ProviderTurn {
            provider_turn,
            message,
            ..
        } => {
            if serde_json::to_value(&provider_turn.provider_thread_id).ok()
                != serde_json::to_value(&input.provider_thread.id).ok()
            {
                return Err(operation(
                    ProviderOperation::Rollback,
                    "Target turn belongs to another provider thread",
                ));
            }
            if let Some(cursor) = claude_turn_cursor(provider_turn) {
                return Ok(Some(cursor));
            }
            let later = input.provider_thread_turns.iter().any(|turn| {
                turn.ordinal > provider_turn.ordinal
                    && matches!(
                        turn.status,
                        ProviderTurnStatus::Completed
                            | ProviderTurnStatus::Interrupted
                            | ProviderTurnStatus::Cancelled
                            | ProviderTurnStatus::Failed
                    )
            });
            if !later {
                return Ok(None);
            }
            if message.as_ref().is_some_and(|message| {
                message.text.trim().eq_ignore_ascii_case("/compact")
                    && message.attachments.is_empty()
            }) && let Some(cursor) = input
                .provider_thread_turns
                .iter()
                .filter(|turn| turn.ordinal < provider_turn.ordinal)
                .max_by_key(|turn| turn.ordinal)
                .and_then(claude_turn_cursor)
            {
                return Ok(Some(cursor));
            }
            Err(operation(
                ProviderOperation::Rollback,
                "No SDK assistant message cursor was recorded for this prior turn",
            ))
        }
    }
}
