#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};
use t3_claude_cli::message::*;

#[test]
fn replays_all_39_recordings_losslessly_in_recorded_order() {
    let manifest: BTreeMap<String, Vec<String>> =
        serde_json::from_str(include_str!("replay-sequences.json")).unwrap();
    assert_eq!(manifest.len(), 39);
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../apps/server/src/orchestration-v2/testkit/fixtures");
    let mut count = 0;
    let mut unknown = BTreeMap::<String, usize>::new();
    for (scenario, expected) in manifest {
        let transcript =
            fs::read_to_string(fixtures.join(&scenario).join("claude_transcript.ndjson")).unwrap();
        let mut sequence = Vec::new();
        for (line, entry) in transcript.lines().enumerate() {
            let entry: Value = serde_json::from_str(entry).unwrap();
            if entry["type"] != "emit_inbound" {
                continue;
            }
            let raw = entry["frame"].clone();
            let message: Message = serde_json::from_value(raw.clone())
                .unwrap_or_else(|e| panic!("{scenario}:{}: {e}", line + 1));
            assert_eq!(
                serde_json::to_value(&message).unwrap(),
                raw,
                "{scenario}:{}",
                line + 1
            );
            if !matches!(message, Message::Unknown(_)) {
                assert_message_fields(&message, &raw);
            }
            if let Message::Result(result) = &message
                && let Some(models) = &result.get().model_usage
            {
                for (name, usage) in models {
                    assert_eq!(
                        usage.cost_usd,
                        raw["modelUsage"][name]["costUSD"].as_f64(),
                        "typed model cost in {scenario}"
                    );
                }
            }
            let label = match &message {
                Message::System(v) => {
                    assert!(
                        !matches!(v.get(), SystemMessage::Unknown(_)),
                        "untyped system message in {scenario}"
                    );
                    format!("system:{}", system_label(v.get()))
                }
                Message::StreamEvent(v) => {
                    assert!(!matches!(v.get().event, StreamEvent::Unknown(_)));
                    let event = &v.get().event;
                    let mut label = format!("stream_event:{}", stream_label(event));
                    if let StreamEvent::ContentBlockDelta(delta) = event {
                        assert!(!matches!(delta.get().delta, Delta::Unknown(_)));
                        label.push(':');
                        label.push_str(delta_label(&delta.get().delta));
                    }
                    label
                }
                Message::Unknown(raw) => {
                    let kind = raw["type"].as_str().unwrap().to_owned();
                    *unknown.entry(kind.clone()).or_default() += 1;
                    kind
                }
                _ => message_label(&message).to_owned(),
            };
            if let Message::Assistant(v) | Message::User(v) = &message
                && let Content::Blocks(blocks) = &v.get().message.content
            {
                assert!(
                    blocks
                        .iter()
                        .all(|b| !matches!(b, ContentBlock::Unknown(_)))
                );
            }
            sequence.push(label);
            count += 1;
        }
        assert_eq!(sequence, expected, "{scenario}");
    }
    assert_eq!(count, 2261);
    // These are replay harness actions, rather than physical SDK output.
    assert_eq!(
        unknown,
        BTreeMap::from([
            ("permission.request".into(), 3),
            ("session.forked".into(), 5),
            ("subagent.found".into(), 1)
        ])
    );
}

#[test]
fn retains_future_messages_nested_blocks_events_and_null_fields() {
    for raw in [
        json!({"type":"future","secret_shape":{"nullable":null}}),
        json!({"type":"system","subtype":"future","new_key":null}),
        json!({"type":"assistant","uuid":"a","message":{"role":"assistant","content":[{"type":"future_block","x":42}],"usage":{"input_tokens":0,"output_tokens":null,"future":1}},"future":null}),
        json!({"type":"stream_event","event":{"type":"future_event","a":true}}),
        json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"future_delta","a":true}}}),
    ] {
        let decoded: Message = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), raw);
    }
    assert!(matches!(
        serde_json::from_value::<Message>(json!({"type":"assistant","message":"wrong"})).unwrap(),
        Message::Unknown(_)
    ));
    assert!(serde_json::from_value::<Message>(json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":7}}})).unwrap().has_unknown());
}

#[test]
fn server_tools_and_all_result_families_preserve_structured_output() {
    for kind in [
        "tool_result",
        "mcp_tool_result",
        "bash_code_execution_tool_result",
        "code_execution_tool_result",
        "advisor_tool_result",
        "text_editor_code_execution_tool_result",
        "tool_search_tool_result",
        "web_fetch_tool_result",
        "web_search_tool_result",
    ] {
        let raw = json!({"type":kind,"tool_use_id":"tool","content":{"type":"future_tool_result_error","payload":[{"type":"image","source":{"data":"abc"}}]},"is_error":true});
        let block: ContentBlock = serde_json::from_value(raw.clone()).unwrap();
        assert!(!matches!(block, ContentBlock::Unknown(_)));
        assert_eq!(serde_json::to_value(block).unwrap(), raw);
    }
}

// Read typed members directly: serializing Decoded would only prove raw retention.
fn assert_values(typed: &Value, raw: &Value) {
    match (typed, raw) {
        (Value::Number(a), Value::Number(b)) => assert_eq!(a.as_f64(), b.as_f64()),
        (Value::Object(a), Value::Object(b)) => {
            for (key, value) in b {
                assert_values(a.get(key).unwrap_or(&Value::Null), value);
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len());
            for (a, b) in a.iter().zip(b) {
                assert_values(a, b);
            }
        }
        _ => assert_eq!(typed, raw),
    }
}
macro_rules! assert_fields {
    ($body:expr, $raw:expr, $( $field:ident => $key:literal ),* $(,)?) => {
        $(if let Some(raw)=$raw.get($key) { assert_values(&serde_json::to_value(&$body.$field).unwrap(),raw); })*
    };
}

fn fields_api_retry(body: &ApiRetry, raw: &Value) {
    assert_fields!(body, raw, attempt => "attempt", max_retries => "max_retries", retry_delay_ms => "retry_delay_ms", error_status => "error_status", error => "error");
}

fn fields_auth_status(body: &AuthStatus, raw: &Value) {
    assert_fields!(body, raw, is_authenticating => "isAuthenticating", output => "output", error => "error");
}

fn fields_background_tasks(body: &BackgroundTasks, raw: &Value) {
    assert_fields!(body, raw, tasks => "tasks");
}

fn fields_block_delta(body: &BlockDelta, raw: &Value) {
    assert_fields!(body, raw, index => "index", delta => "delta");
    assert_delta_fields(&body.delta, &raw["delta"]);
}

fn fields_block_start(body: &BlockStart, raw: &Value) {
    assert_fields!(body, raw, index => "index", content_block => "content_block");
    assert_content_block_fields(&body.content_block, &raw["content_block"]);
}

fn fields_block_stop(body: &BlockStop, raw: &Value) {
    assert_fields!(body, raw, index => "index");
}

fn fields_cache_creation(body: &CacheCreation, raw: &Value) {
    assert_fields!(body, raw, ephemeral_5m_input_tokens => "ephemeral_5m_input_tokens", ephemeral_1h_input_tokens => "ephemeral_1h_input_tokens");
}

fn fields_citation_delta(body: &CitationDelta, raw: &Value) {
    assert_fields!(body, raw, citation => "citation");
}

fn fields_command_lifecycle(body: &CommandLifecycle, raw: &Value) {
    assert_fields!(body, raw, command_uuid => "command_uuid", state => "state");
}

fn fields_commands_changed(body: &CommandsChanged, raw: &Value) {
    assert_fields!(body, raw, commands => "commands");
}

fn fields_compact_boundary(body: &CompactBoundary, raw: &Value) {
    assert_fields!(body, raw, compact_metadata => "compact_metadata");
    fields_compact_metadata(&body.compact_metadata, &raw["compact_metadata"]);
}

fn fields_compact_metadata(body: &CompactMetadata, raw: &Value) {
    assert_fields!(body, raw, trigger => "trigger", pre_tokens => "pre_tokens", post_tokens => "post_tokens", preserved_segment => "preserved_segment", preserved_messages => "preserved_messages");
}

fn fields_compaction_block(body: &CompactionBlock, raw: &Value) {
    assert_fields!(body, raw, content => "content");
}

fn fields_control_progress(body: &ControlProgress, raw: &Value) {
    assert_fields!(body, raw, request_id => "request_id", status => "status", attempt => "attempt", max_retries => "max_retries", retry_delay_ms => "retry_delay_ms", error_status => "error_status");
}

fn fields_conversation(body: &Conversation, raw: &Value) {
    assert_fields!(body, raw, message => "message", uuid => "uuid", session_id => "session_id", parent_tool_use_id => "parent_tool_use_id", error => "error", api_error_params => "api_error_params", tool_use_meta => "tool_use_meta", tool_use_result => "tool_use_result", tool_result_meta => "tool_result_meta", user_message_uuid => "user_message_uuid", user_message_uuids => "user_message_uuids");
    fields_conversation_message(&body.message, &raw["message"]);
}

fn fields_conversation_message(body: &ConversationMessage, raw: &Value) {
    assert_fields!(body, raw, role => "role", content => "content", id => "id", model => "model", usage => "usage", stop_reason => "stop_reason");
    assert_content(&body.content, &raw["content"]);
    if let Some(value) = &body.usage {
        fields_usage(value, &raw["usage"]);
    }
}

fn fields_conversation_reset(body: &ConversationReset, raw: &Value) {
    assert_fields!(body, raw, new_conversation_id => "new_conversation_id", trigger => "trigger", timestamp => "timestamp", user_message_uuid => "user_message_uuid");
}

fn fields_elicitation_complete(body: &ElicitationComplete, raw: &Value) {
    assert_fields!(body, raw, mcp_server_name => "mcp_server_name", elicitation_id => "elicitation_id");
}

fn fields_files_persisted(body: &FilesPersisted, raw: &Value) {
    assert_fields!(body, raw, files => "files", failed => "failed", processed_at => "processed_at");
}

fn fields_hook_message(body: &HookMessage, raw: &Value) {
    assert_fields!(body, raw, hook_id => "hook_id", hook_name => "hook_name", hook_event => "hook_event", stdout => "stdout", stderr => "stderr", output => "output", exit_code => "exit_code", outcome => "outcome");
}

fn fields_init_message(body: &InitMessage, raw: &Value) {
    assert_fields!(body, raw, session_id => "session_id", model => "model", claude_code_version => "claude_code_version", permission_mode => "permissionMode", capabilities => "capabilities", tools => "tools", mcp_servers => "mcp_servers", slash_commands => "slash_commands", cwd => "cwd");
}

fn fields_json_delta(body: &JsonDelta, raw: &Value) {
    assert_fields!(body, raw, partial_json => "partial_json");
}

fn fields_media_block(body: &MediaBlock, raw: &Value) {
    assert_fields!(body, raw, source => "source");
}

fn fields_memory_recall(body: &MemoryRecall, raw: &Value) {
    assert_fields!(body, raw, mode => "mode", memories => "memories");
}

fn fields_message_delta(body: &MessageDelta, raw: &Value) {
    assert_fields!(body, raw, delta => "delta", usage => "usage");
    if let Some(value) = &body.usage {
        fields_usage(value, &raw["usage"]);
    }
}

fn fields_message_start(body: &MessageStart, raw: &Value) {
    assert_fields!(body, raw, message => "message");
    fields_conversation_message(&body.message, &raw["message"]);
}

fn fields_mirror_error(body: &MirrorError, raw: &Value) {
    assert_fields!(body, raw, error => "error", key => "key");
}

fn fields_model_refusal(body: &ModelRefusal, raw: &Value) {
    assert_fields!(body, raw, content => "content", original_model => "original_model", fallback_model => "fallback_model", trigger => "trigger", direction => "direction", scope => "scope", request_id => "request_id", api_refusal_category => "api_refusal_category", api_refusal_explanation => "api_refusal_explanation", retracted_message_uuids => "retracted_message_uuids");
}

fn fields_model_usage(body: &ModelUsage, raw: &Value) {
    assert_fields!(body, raw, input_tokens => "inputTokens", output_tokens => "outputTokens", cache_read_input_tokens => "cacheReadInputTokens", cache_creation_input_tokens => "cacheCreationInputTokens", cost_usd => "costUSD", context_window => "contextWindow", max_output_tokens => "maxOutputTokens");
}

fn fields_notice(body: &Notice, raw: &Value) {
    assert_fields!(body, raw, content => "content", level => "level", prevent_continuation => "prevent_continuation");
}

fn fields_notification(body: &Notification, raw: &Value) {
    assert_fields!(body, raw, key => "key", text => "text", priority => "priority", color => "color", timeout_ms => "timeout_ms");
}

fn fields_open_block(body: &OpenBlock, raw: &Value) {
    assert_values(&serde_json::to_value(&body.fields).unwrap(), raw);
}

fn fields_output_token_details(body: &OutputTokenDetails, raw: &Value) {
    assert_fields!(body, raw, thinking_tokens => "thinking_tokens");
}

fn fields_partial_message(body: &PartialMessage, raw: &Value) {
    assert_fields!(body, raw, event => "event", uuid => "uuid", session_id => "session_id", parent_tool_use_id => "parent_tool_use_id", user_message_uuid => "user_message_uuid", user_message_uuids => "user_message_uuids");
    assert_stream_fields(&body.event, &raw["event"]);
}

fn fields_permission_denied(body: &PermissionDenied, raw: &Value) {
    assert_fields!(body, raw, tool_name => "tool_name", tool_use_id => "tool_use_id", message => "message", agent_id => "agent_id", decision_reason => "decision_reason", decision_reason_type => "decision_reason_type");
}

fn fields_plugin_install(body: &PluginInstall, raw: &Value) {
    assert_fields!(body, raw, status => "status", name => "name", error => "error");
}

fn fields_prompt_suggestion(body: &PromptSuggestion, raw: &Value) {
    assert_fields!(body, raw, suggestion => "suggestion");
}

fn fields_rate_limit(body: &RateLimit, raw: &Value) {
    assert_fields!(body, raw, rate_limit_info => "rateLimitInfo");
    fields_rate_limit_info(&body.rate_limit_info, &raw["rateLimitInfo"]);
}

fn fields_rate_limit_info(body: &RateLimitInfo, raw: &Value) {
    assert_fields!(body, raw, status => "status", resets_at => "resetsAt", rate_limit_type => "rateLimitType", utilization => "utilization", overage_status => "overageStatus", overage_resets_at => "overageResetsAt", overage_disabled_reason => "overageDisabledReason", is_using_overage => "isUsingOverage", overage_in_use => "overageInUse", overage_enabled => "overageEnabled");
}

fn fields_redacted_thinking_block(body: &RedactedThinkingBlock, raw: &Value) {
    assert_fields!(body, raw, data => "data");
}

fn fields_result_message(body: &ResultMessage, raw: &Value) {
    assert_fields!(body, raw, subtype => "subtype", is_error => "is_error", result => "result", errors => "errors", terminal_reason => "terminal_reason", api_error_status => "api_error_status", num_turns => "num_turns", usage => "usage", model_usage => "modelUsage", duration_ms => "duration_ms", duration_api_ms => "duration_api_ms", total_cost_usd => "total_cost_usd", uuid => "uuid", session_id => "session_id", origin => "origin", local_command => "local_command", user_message_uuid => "user_message_uuid", user_message_uuids => "user_message_uuids");
    if let Some(value) = &body.usage {
        fields_usage(value, &raw["usage"]);
    }
    if let Some(models) = &body.model_usage {
        for (name, model) in models {
            fields_model_usage(model, &raw["modelUsage"][name]);
        }
    }
}

fn fields_session_state(body: &SessionState, raw: &Value) {
    assert_fields!(body, raw, state => "state");
}

fn fields_session_writes_lost(body: &SessionWritesLost, raw: &Value) {
    assert_fields!(body, raw, error_code => "error_code", lost => "lost", transcript_session_ids => "transcript_session_ids");
}

fn fields_signature_delta(body: &SignatureDelta, raw: &Value) {
    assert_fields!(body, raw, signature => "signature");
}

fn fields_status_message(body: &StatusMessage, raw: &Value) {
    assert_fields!(body, raw, status => "status", permission_mode => "permissionMode", compact_result => "compact_result", compact_error => "compact_error");
}

fn fields_task_message(body: &TaskMessage, raw: &Value) {
    assert_fields!(body, raw, task_id => "task_id", tool_use_id => "tool_use_id", description => "description", prompt => "prompt", task_type => "task_type", spawn_depth => "spawn_depth", parent_task_id => "parent_task_id", is_backgrounded => "is_backgrounded", status => "status", summary => "summary", output_file => "output_file", usage => "usage", workflow_progress => "workflow_progress", patch => "patch");
}

fn fields_text_block(body: &TextBlock, raw: &Value) {
    assert_fields!(body, raw, text => "text");
}

fn fields_thinking_block(body: &ThinkingBlock, raw: &Value) {
    assert_fields!(body, raw, thinking => "thinking", signature => "signature");
}

fn fields_thinking_tokens(body: &ThinkingTokens, raw: &Value) {
    assert_fields!(body, raw, estimated_tokens => "estimated_tokens", estimated_tokens_delta => "estimated_tokens_delta", user_message_uuid => "user_message_uuid");
}

fn fields_tool_progress(body: &ToolProgress, raw: &Value) {
    assert_fields!(body, raw, tool_use_id => "tool_use_id", tool_name => "tool_name", parent_tool_use_id => "parent_tool_use_id", elapsed_time_seconds => "elapsed_time_seconds", task_id => "task_id", heartbeat => "heartbeat", subagent_type => "subagent_type", subagent_retry => "subagent_retry");
}

fn fields_tool_result_block(body: &ToolResultBlock, raw: &Value) {
    assert_fields!(body, raw, tool_use_id => "tool_use_id", content => "content", is_error => "is_error");
}

fn fields_tool_summary(body: &ToolSummary, raw: &Value) {
    assert_fields!(body, raw, summary => "summary", preceding_tool_use_ids => "preceding_tool_use_ids");
}

fn fields_tool_use_block(body: &ToolUseBlock, raw: &Value) {
    assert_fields!(body, raw, id => "id", name => "name", input => "input");
}

fn fields_usage(body: &Usage, raw: &Value) {
    assert_fields!(body, raw, input_tokens => "input_tokens", output_tokens => "output_tokens", cache_creation_input_tokens => "cache_creation_input_tokens", cache_read_input_tokens => "cache_read_input_tokens", cache_creation => "cache_creation", output_tokens_details => "output_tokens_details");
    if let Some(value) = &body.cache_creation {
        fields_cache_creation(value, &raw["cache_creation"]);
    }
    if let Some(value) = &body.output_tokens_details {
        fields_output_token_details(value, &raw["output_tokens_details"]);
    }
}

fn fields_worker_shutdown(body: &WorkerShutdown, raw: &Value) {
    assert_fields!(body, raw, reason => "reason");
}

fn message_label(message: &Message) -> &'static str {
    match message {
        Message::System(_) => "system",
        Message::Assistant(_) => "assistant",
        Message::User(_) => "user",
        Message::Result(_) => "result",
        Message::StreamEvent(_) => "stream_event",
        Message::RateLimit(_) => "rate_limit_event",
        Message::CommandLifecycle(_) => "command_lifecycle",
        Message::PromptSuggestion(_) => "prompt_suggestion",
        Message::ActiveGoal(_) => "active_goal",
        Message::AuthStatus(_) => "auth_status",
        Message::ToolProgress(_) => "tool_progress",
        Message::ToolUseSummary(_) => "tool_use_summary",
        Message::ConversationReset(_) => "conversation_reset",
        Message::KeepAlive(_) => "keep_alive",
        Message::TranscriptMirror(_) => "transcript_mirror",
        Message::Unknown(_) => "unknown",
    }
}
fn assert_message_fields(message: &Message, raw: &Value) {
    assert_eq!(Some(message_label(message)), raw["type"].as_str());
    match message {
        Message::System(value) => assert_system_fields(value.get(), raw),
        Message::Assistant(value) => fields_conversation(value.get(), raw),
        Message::User(value) => fields_conversation(value.get(), raw),
        Message::Result(value) => fields_result_message(value.get(), raw),
        Message::StreamEvent(value) => fields_partial_message(value.get(), raw),
        Message::RateLimit(value) => fields_rate_limit(value.get(), raw),
        Message::CommandLifecycle(value) => fields_command_lifecycle(value.get(), raw),
        Message::PromptSuggestion(value) => fields_prompt_suggestion(value.get(), raw),
        Message::ActiveGoal(value) => fields_open_block(value.get(), raw),
        Message::AuthStatus(value) => fields_auth_status(value.get(), raw),
        Message::ToolProgress(value) => fields_tool_progress(value.get(), raw),
        Message::ToolUseSummary(value) => fields_tool_summary(value.get(), raw),
        Message::ConversationReset(value) => fields_conversation_reset(value.get(), raw),
        Message::KeepAlive(value) => fields_open_block(value.get(), raw),
        Message::TranscriptMirror(value) => fields_open_block(value.get(), raw),
        Message::Unknown(_) => panic!("untyped Message"),
    }
}
fn system_label(message: &SystemMessage) -> &'static str {
    match message {
        SystemMessage::Init(_) => "init",
        SystemMessage::Status(_) => "status",
        SystemMessage::CompactBoundary(_) => "compact_boundary",
        SystemMessage::ApiRetry(_) => "api_retry",
        SystemMessage::TaskStarted(_) => "task_started",
        SystemMessage::TaskProgress(_) => "task_progress",
        SystemMessage::TaskUpdated(_) => "task_updated",
        SystemMessage::TaskNotification(_) => "task_notification",
        SystemMessage::BackgroundTasksChanged(_) => "background_tasks_changed",
        SystemMessage::ModelRefusalFallback(_) => "model_refusal_fallback",
        SystemMessage::ModelRefusalNoFallback(_) => "model_refusal_no_fallback",
        SystemMessage::Informational(_) => "informational",
        SystemMessage::LocalCommandOutput(_) => "local_command_output",
        SystemMessage::HookStarted(_) => "hook_started",
        SystemMessage::HookProgress(_) => "hook_progress",
        SystemMessage::HookResponse(_) => "hook_response",
        SystemMessage::PluginInstall(_) => "plugin_install",
        SystemMessage::ThinkingTokens(_) => "thinking_tokens",
        SystemMessage::SessionStateChanged(_) => "session_state_changed",
        SystemMessage::WorkerShuttingDown(_) => "worker_shutting_down",
        SystemMessage::CommandsChanged(_) => "commands_changed",
        SystemMessage::Notification(_) => "notification",
        SystemMessage::FilesPersisted(_) => "files_persisted",
        SystemMessage::MemoryRecall(_) => "memory_recall",
        SystemMessage::ElicitationComplete(_) => "elicitation_complete",
        SystemMessage::PermissionDenied(_) => "permission_denied",
        SystemMessage::MirrorError(_) => "mirror_error",
        SystemMessage::ConversationReset(_) => "conversation_reset",
        SystemMessage::SessionWritesLost(_) => "session_writes_lost",
        SystemMessage::ControlRequestProgress(_) => "control_request_progress",
        SystemMessage::AutocompactState(_) => "autocompact_state",
        SystemMessage::UiPanes(_) => "ui_panes",
        SystemMessage::Unknown(_) => "unknown",
    }
}
fn assert_system_fields(message: &SystemMessage, raw: &Value) {
    assert_eq!(Some(system_label(message)), raw["subtype"].as_str());
    match message {
        SystemMessage::Init(value) => fields_init_message(value.get(), raw),
        SystemMessage::Status(value) => fields_status_message(value.get(), raw),
        SystemMessage::CompactBoundary(value) => fields_compact_boundary(value.get(), raw),
        SystemMessage::ApiRetry(value) => fields_api_retry(value.get(), raw),
        SystemMessage::TaskStarted(value) => fields_task_message(value.get(), raw),
        SystemMessage::TaskProgress(value) => fields_task_message(value.get(), raw),
        SystemMessage::TaskUpdated(value) => fields_task_message(value.get(), raw),
        SystemMessage::TaskNotification(value) => fields_task_message(value.get(), raw),
        SystemMessage::BackgroundTasksChanged(value) => fields_background_tasks(value.get(), raw),
        SystemMessage::ModelRefusalFallback(value) => fields_model_refusal(value.get(), raw),
        SystemMessage::ModelRefusalNoFallback(value) => fields_model_refusal(value.get(), raw),
        SystemMessage::Informational(value) => fields_notice(value.get(), raw),
        SystemMessage::LocalCommandOutput(value) => fields_notice(value.get(), raw),
        SystemMessage::HookStarted(value) => fields_hook_message(value.get(), raw),
        SystemMessage::HookProgress(value) => fields_hook_message(value.get(), raw),
        SystemMessage::HookResponse(value) => fields_hook_message(value.get(), raw),
        SystemMessage::PluginInstall(value) => fields_plugin_install(value.get(), raw),
        SystemMessage::ThinkingTokens(value) => fields_thinking_tokens(value.get(), raw),
        SystemMessage::SessionStateChanged(value) => fields_session_state(value.get(), raw),
        SystemMessage::WorkerShuttingDown(value) => fields_worker_shutdown(value.get(), raw),
        SystemMessage::CommandsChanged(value) => fields_commands_changed(value.get(), raw),
        SystemMessage::Notification(value) => fields_notification(value.get(), raw),
        SystemMessage::FilesPersisted(value) => fields_files_persisted(value.get(), raw),
        SystemMessage::MemoryRecall(value) => fields_memory_recall(value.get(), raw),
        SystemMessage::ElicitationComplete(value) => fields_elicitation_complete(value.get(), raw),
        SystemMessage::PermissionDenied(value) => fields_permission_denied(value.get(), raw),
        SystemMessage::MirrorError(value) => fields_mirror_error(value.get(), raw),
        SystemMessage::ConversationReset(value) => fields_conversation_reset(value.get(), raw),
        SystemMessage::SessionWritesLost(value) => fields_session_writes_lost(value.get(), raw),
        SystemMessage::ControlRequestProgress(value) => fields_control_progress(value.get(), raw),
        SystemMessage::AutocompactState(value) => fields_open_block(value.get(), raw),
        SystemMessage::UiPanes(value) => fields_open_block(value.get(), raw),
        SystemMessage::Unknown(_) => panic!("untyped SystemMessage"),
    }
}
fn stream_label(message: &StreamEvent) -> &'static str {
    match message {
        StreamEvent::MessageStart(_) => "message_start",
        StreamEvent::MessageDelta(_) => "message_delta",
        StreamEvent::MessageStop(_) => "message_stop",
        StreamEvent::ContentBlockStart(_) => "content_block_start",
        StreamEvent::ContentBlockDelta(_) => "content_block_delta",
        StreamEvent::ContentBlockStop(_) => "content_block_stop",
        StreamEvent::Ping(_) => "ping",
        StreamEvent::Error(_) => "error",
        StreamEvent::Unknown(_) => "unknown",
    }
}
fn assert_stream_fields(message: &StreamEvent, raw: &Value) {
    assert_eq!(Some(stream_label(message)), raw["type"].as_str());
    match message {
        StreamEvent::MessageStart(value) => fields_message_start(value.get(), raw),
        StreamEvent::MessageDelta(value) => fields_message_delta(value.get(), raw),
        StreamEvent::MessageStop(value) => fields_open_block(value.get(), raw),
        StreamEvent::ContentBlockStart(value) => fields_block_start(value.get(), raw),
        StreamEvent::ContentBlockDelta(value) => fields_block_delta(value.get(), raw),
        StreamEvent::ContentBlockStop(value) => fields_block_stop(value.get(), raw),
        StreamEvent::Ping(value) => fields_open_block(value.get(), raw),
        StreamEvent::Error(value) => fields_open_block(value.get(), raw),
        StreamEvent::Unknown(_) => panic!("untyped StreamEvent"),
    }
}
fn delta_label(message: &Delta) -> &'static str {
    match message {
        Delta::Text(_) => "text_delta",
        Delta::Thinking(_) => "thinking_delta",
        Delta::InputJson(_) => "input_json_delta",
        Delta::Signature(_) => "signature_delta",
        Delta::Citations(_) => "citations_delta",
        Delta::Compaction(_) => "compaction_delta",
        Delta::Unknown(_) => "unknown",
    }
}
fn assert_delta_fields(message: &Delta, raw: &Value) {
    assert_eq!(Some(delta_label(message)), raw["type"].as_str());
    match message {
        Delta::Text(value) => fields_text_block(value.get(), raw),
        Delta::Thinking(value) => fields_thinking_block(value.get(), raw),
        Delta::InputJson(value) => fields_json_delta(value.get(), raw),
        Delta::Signature(value) => fields_signature_delta(value.get(), raw),
        Delta::Citations(value) => fields_citation_delta(value.get(), raw),
        Delta::Compaction(value) => fields_compaction_block(value.get(), raw),
        Delta::Unknown(_) => panic!("untyped Delta"),
    }
}
fn content_block_label(message: &ContentBlock) -> &'static str {
    match message {
        ContentBlock::Text(_) => "text",
        ContentBlock::Thinking(_) => "thinking",
        ContentBlock::RedactedThinking(_) => "redacted_thinking",
        ContentBlock::ToolUse(_) => "tool_use",
        ContentBlock::ServerToolUse(_) => "server_tool_use",
        ContentBlock::McpToolUse(_) => "mcp_tool_use",
        ContentBlock::ToolResult(_) => "tool_result",
        ContentBlock::McpToolResult(_) => "mcp_tool_result",
        ContentBlock::BashResult(_) => "bash_code_execution_tool_result",
        ContentBlock::CodeResult(_) => "code_execution_tool_result",
        ContentBlock::AdvisorResult(_) => "advisor_tool_result",
        ContentBlock::EditorResult(_) => "text_editor_code_execution_tool_result",
        ContentBlock::ToolSearchResult(_) => "tool_search_tool_result",
        ContentBlock::WebFetchResult(_) => "web_fetch_tool_result",
        ContentBlock::WebSearchResult(_) => "web_search_tool_result",
        ContentBlock::Image(_) => "image",
        ContentBlock::Document(_) => "document",
        ContentBlock::Audio(_) => "audio",
        ContentBlock::Container(_) => "container",
        ContentBlock::Compaction(_) => "compaction",
        ContentBlock::Unknown(_) => "unknown",
    }
}
fn assert_content_block_fields(message: &ContentBlock, raw: &Value) {
    assert_eq!(Some(content_block_label(message)), raw["type"].as_str());
    match message {
        ContentBlock::Text(value) => fields_text_block(value.get(), raw),
        ContentBlock::Thinking(value) => fields_thinking_block(value.get(), raw),
        ContentBlock::RedactedThinking(value) => fields_redacted_thinking_block(value.get(), raw),
        ContentBlock::ToolUse(value) => fields_tool_use_block(value.get(), raw),
        ContentBlock::ServerToolUse(value) => fields_tool_use_block(value.get(), raw),
        ContentBlock::McpToolUse(value) => fields_tool_use_block(value.get(), raw),
        ContentBlock::ToolResult(value) => fields_tool_result_block(value.get(), raw),
        ContentBlock::McpToolResult(value) => fields_tool_result_block(value.get(), raw),
        ContentBlock::BashResult(value) => fields_tool_result_block(value.get(), raw),
        ContentBlock::CodeResult(value) => fields_tool_result_block(value.get(), raw),
        ContentBlock::AdvisorResult(value) => fields_tool_result_block(value.get(), raw),
        ContentBlock::EditorResult(value) => fields_tool_result_block(value.get(), raw),
        ContentBlock::ToolSearchResult(value) => fields_tool_result_block(value.get(), raw),
        ContentBlock::WebFetchResult(value) => fields_tool_result_block(value.get(), raw),
        ContentBlock::WebSearchResult(value) => fields_tool_result_block(value.get(), raw),
        ContentBlock::Image(value) => fields_media_block(value.get(), raw),
        ContentBlock::Document(value) => fields_media_block(value.get(), raw),
        ContentBlock::Audio(value) => fields_open_block(value.get(), raw),
        ContentBlock::Container(value) => fields_open_block(value.get(), raw),
        ContentBlock::Compaction(value) => fields_compaction_block(value.get(), raw),
        ContentBlock::Unknown(_) => panic!("untyped ContentBlock"),
    }
}
fn assert_content(content: &Content, raw: &Value) {
    match content {
        Content::Text(text) => assert_eq!(Some(text.as_str()), raw.as_str()),
        Content::Blocks(blocks) => {
            assert_eq!(blocks.len(), raw.as_array().unwrap().len());
            for (block, raw) in blocks.iter().zip(raw.as_array().unwrap()) {
                assert_content_block_fields(block, raw);
            }
        }
    }
}

#[test]
fn constructed_sdk_examples_cover_every_modeled_message_and_system_variant() {
    let samples: Value = serde_json::from_str(include_str!("constructed-messages.json")).unwrap();
    assert_eq!(samples["provenance"], "constructed, not recorded");
    let mut types = std::collections::BTreeSet::new();
    let mut systems = std::collections::BTreeSet::new();
    for sample in samples["samples"].as_array().unwrap() {
        let raw = &sample["frame"];
        let message = Message::from_value(raw.clone()).unwrap();
        assert!(
            !matches!(message, Message::Unknown(_)),
            "untyped constructed sample: {}",
            sample["label"]
        );
        assert_message_fields(&message, raw);
        types.insert(message_label(&message));
        if let Message::System(system) = &message {
            systems.insert(system_label(system.get()));
        }
    }
    assert_eq!(types.len(), 15);
    assert_eq!(systems.len(), 32);
}
