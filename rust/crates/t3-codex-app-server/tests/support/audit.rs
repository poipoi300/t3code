use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use t3_codex_app_server::*;

pub trait Audit {
    fn audit(&self, found: &mut BTreeSet<String>);
}
macro_rules! leaves { ($($t:ty),*) => { $(impl Audit for $t { fn audit(&self, _: &mut BTreeSet<String>) {} })* }; }
leaves!(String, bool, i64, u32, f64, Value);
impl<T: Audit> Audit for Box<T> {
    fn audit(&self, f: &mut BTreeSet<String>) {
        self.as_ref().audit(f);
    }
}
impl<T: Audit> Audit for Option<T> {
    fn audit(&self, f: &mut BTreeSet<String>) {
        if let Some(v) = self {
            v.audit(f);
        }
    }
}
impl<T: Audit> Audit for Field<T> {
    fn audit(&self, f: &mut BTreeSet<String>) {
        if let Self::Value(v) = self {
            v.audit(f);
        }
    }
}
impl<T: Audit> Audit for Vec<T> {
    fn audit(&self, f: &mut BTreeSet<String>) {
        for v in self {
            v.audit(f);
        }
    }
}
impl<T: Audit> Audit for BTreeMap<String, T> {
    fn audit(&self, f: &mut BTreeSet<String>) {
        for v in self.values() {
            v.audit(f);
        }
    }
}
macro_rules! dto {
    ($t:ty, $($field:ident),* $(,)?) => { impl Audit for $t {
        fn audit(&self, found: &mut BTreeSet<String>) {
            for key in self.extra.keys() { found.insert(format!("{}.{}", stringify!($t), key)); }
            $(self.$field.audit(found);)*
        }
    } };
}
dto!(Empty,);
dto!(ClientInfo, name, title, version);
dto!(
    InitializeCapabilities,
    extensions,
    opt_out_notification_methods,
    experimental_api,
    explicit_gateway_oauth,
    mcp_server_openai_form_elicitation,
    request_attestation
);
dto!(InitializeParams, capabilities, client_info);
dto!(
    InitializeResponse,
    codex_home,
    platform_family,
    platform_os,
    user_agent
);
dto!(
    ThreadStartParams,
    approval_policy,
    approvals_reviewer,
    base_instructions,
    config,
    cwd,
    developer_instructions,
    ephemeral,
    model,
    model_provider,
    personality,
    sandbox,
    service_name,
    service_tier,
    session_start_source,
    thread_source
);
dto!(
    ThreadResumeParams,
    approval_policy,
    approvals_reviewer,
    base_instructions,
    config,
    cwd,
    developer_instructions,
    model,
    model_provider,
    personality,
    sandbox,
    service_tier,
    thread_id,
    exclude_turns
);
dto!(
    ThreadForkParams,
    approval_policy,
    approvals_reviewer,
    base_instructions,
    config,
    cwd,
    developer_instructions,
    last_turn_id,
    model,
    model_provider,
    sandbox,
    service_tier,
    thread_id,
    thread_source,
    ephemeral,
    exclude_turns
);
dto!(ThreadIdParams, thread_id);
dto!(ThreadReadParams, include_turns, thread_id);
dto!(ThreadRevertParams, before_turn_id, thread_id);
dto!(
    ThreadTurnsListParams,
    cursor,
    items_view,
    limit,
    sort_direction,
    thread_id
);
dto!(ThreadInjectItemsParams, thread_id, items);
dto!(
    ThreadGoalSetParams,
    objective,
    status,
    thread_id,
    token_budget
);
dto!(
    Thread,
    agent_nickname,
    agent_role,
    cli_version,
    created_at,
    cwd,
    ephemeral,
    forked_from_id,
    git_info,
    id,
    model,
    model_provider,
    name,
    originator,
    parent_thread_id,
    path,
    preview,
    project_id,
    reasoning_effort,
    recency_at,
    section,
    section_entered_at,
    session_id,
    source,
    status,
    thread_source,
    turns,
    updated_at,
    history_mode
);
dto!(
    ThreadResponse,
    thread,
    reasoning_effort,
    service_tier,
    collaboration_mode,
    items_backwards_cursor,
    turns_backwards_cursor,
    approval_policy,
    sandbox,
    approvals_reviewer,
    cwd,
    disabled_plugin_ids,
    instruction_sources,
    model,
    model_provider
);
dto!(ThreadTurnsListResponse, backwards_cursor, data, next_cursor);
dto!(
    ThreadGoal,
    created_at,
    objective,
    status,
    thread_id,
    time_used_seconds,
    token_budget,
    tokens_used,
    updated_at
);
dto!(ThreadGoalResponse, goal);
dto!(ThreadGoalClearResponse, cleared);
dto!(
    Turn,
    completed_at,
    duration_ms,
    error,
    id,
    items,
    started_at,
    status,
    items_view
);
dto!(
    TurnError,
    additional_details,
    codex_error_info,
    message,
    misalignment
);
dto!(TurnResponse, turn);
dto!(CollaborationMode, mode, settings);
dto!(AdditionalContextEntry, kind, value);
dto!(
    TurnStartParams,
    approval_policy,
    approvals_reviewer,
    client_user_message_id,
    cwd,
    disabled_plugin_ids,
    effort,
    input,
    model,
    personality,
    sandbox_policy,
    service_tier,
    service_tier_for_turn,
    summary,
    thread_id,
    tool_output,
    turn_trigger,
    output_schema,
    collaboration_mode,
    additional_context,
    cyber_access_program
);
dto!(
    TurnSteerParams,
    client_user_message_id,
    expected_turn_id,
    input,
    thread_id
);
dto!(TurnSteerResponse, turn_id);
dto!(TurnInterruptParams, thread_id, turn_id);
dto!(ConfigReadParams, cwd, include_layers);
dto!(ConfigReadResponse, config, layers, origins);
dto!(
    FeedbackUploadParams,
    classification,
    extra_log_files,
    reason,
    tags,
    thread_id,
    include_logs
);
dto!(FeedbackUploadResponse, prompt_hash, thread_id);
dto!(ModelListParams, cursor, include_hidden, limit);
dto!(ReasoningEffortOption, description, reasoning_effort);
dto!(
    Model,
    availability_nux,
    available_access_programs,
    default_reasoning_effort,
    default_service_tier,
    description,
    display_name,
    hidden,
    id,
    is_default,
    model,
    model_specialty,
    multi_agent_version,
    supported_reasoning_efforts,
    upgrade,
    upgrade_info,
    additional_speed_tiers,
    input_modalities,
    service_tiers,
    supports_personality
);
dto!(ModelListResponse, data, next_cursor);
dto!(GetAccountParams, refresh_token);
dto!(GetAccountResponse, account, requires_openai_auth);
dto!(
    RateLimitWindow,
    resets_at,
    used_percent,
    window_duration_mins
);
dto!(
    GetAccountRateLimitsResponse,
    account_id,
    ordinary_usage_allowed,
    rate_limit_reset_credits,
    rate_limits_by_limit_id,
    rate_limits,
    rate_limit_upsell
);
dto!(ConsumeResetCreditParams, credit_id, idempotency_key);
dto!(ConsumeResetCreditResponse, outcome);
dto!(SkillsListParams, cwds, force_reload);
dto!(
    Skill,
    dependencies,
    description,
    enabled,
    interface,
    name,
    path,
    plugin_id,
    scope,
    short_description
);
dto!(SkillsListEntry, cwd, errors, skills);
dto!(SkillsListResponse, data);
dto!(
    ListMcpServerStatusParams,
    cursor,
    detail,
    limit,
    server_name,
    thread_id
);
dto!(
    McpServerStatus,
    auth_status,
    http_origin,
    name,
    plugin_id,
    resource_templates,
    resources,
    runtime_status,
    server_info,
    tools,
    tools_error,
    server_capabilities
);
dto!(ListMcpServerStatusResponse, data, next_cursor);
dto!(
    McpServerToolCallParams,
    server,
    thread_id,
    tool,
    meta,
    arguments
);
dto!(
    McpServerToolCallResponse,
    content,
    is_error,
    meta,
    structured_content
);
dto!(
    McpResourceReadParams,
    connector_id,
    origin_call_id,
    server,
    target,
    thread_id,
    uri
);
dto!(McpResourceReadResponse, contents, origin_call_id);
dto!(BackgroundTerminalsListParams, thread_id, cursor);
dto!(BackgroundTerminal, process_id);
dto!(BackgroundTerminalsListResponse, data, next_cursor);
dto!(BackgroundTerminalsTerminateParams, thread_id, process_id);
dto!(BackgroundTerminalsTerminateResponse, terminated);
dto!(
    ThreadSampleParams,
    thread_id,
    expected_turn_id,
    prompt,
    effort
);
dto!(ThreadSampleResponse, text);
dto!(ToolError, message);
dto!(FileChange, diff, kind, path);
dto!(
    ItemDelta,
    delta,
    item_id,
    thread_id,
    turn_id,
    summary_index,
    content_index
);
dto!(
    ItemNotification,
    item,
    thread_id,
    turn_id,
    started_at_ms,
    completed_at_ms
);
dto!(TurnNotification, thread_id, turn);
dto!(ThreadNotification, thread_id);
dto!(PlanStep, status, step);
dto!(TurnPlanUpdated, explanation, plan, thread_id, turn_id);
dto!(
    TokenUsageBreakdown,
    cached_input_tokens,
    input_tokens,
    output_tokens,
    reasoning_output_tokens,
    total_tokens,
    cache_write_input_tokens
);
dto!(TokenUsage, last, model_context_window, total);
dto!(ThreadTokenUsageUpdated, thread_id, token_usage, turn_id);
dto!(
    ThreadSettings,
    active_permission_profile,
    approval_policy,
    approvals_reviewer,
    collaboration_mode,
    cwd,
    effort,
    model,
    model_provider,
    personality,
    sandbox_policy,
    service_tier,
    summary,
    disabled_plugin_ids
);
dto!(ThreadSettingsUpdated, thread_id, thread_settings);
dto!(
    ModelRerouted,
    from_model,
    reason,
    thread_id,
    to_model,
    turn_id
);
dto!(
    McpStartupStatusUpdated,
    error,
    failure_reason,
    name,
    status,
    thread_id
);
dto!(
    HookRun,
    completed_at,
    display_order,
    duration_ms,
    entries,
    event_name,
    execution_mode,
    handler_type,
    id,
    scope,
    source_path,
    started_at,
    status,
    status_message,
    source
);
dto!(HookCompleted, run, thread_id, turn_id);
dto!(ThreadGoalUpdated, goal, thread_id, turn_id);
dto!(ErrorNotification, error, thread_id, turn_id, will_retry);
dto!(AccountRateLimitsUpdated, rate_limits);
dto!(
    CommandApprovalParams,
    approval_id,
    command,
    command_actions,
    cwd,
    environment_id,
    item_id,
    network_approval_context,
    proposed_execpolicy_amendment,
    proposed_network_policy_amendments,
    reason,
    started_at_ms,
    thread_id,
    turn_id,
    kind
);
dto!(
    FileChangeApprovalParams,
    grant_root,
    item_id,
    reason,
    started_at_ms,
    thread_id,
    turn_id
);
dto!(
    PermissionsApprovalParams,
    cwd,
    environment_id,
    item_id,
    permissions,
    reason,
    started_at_ms,
    thread_id,
    turn_id
);
dto!(ApprovalResponse, decision);
dto!(
    PermissionsApprovalResponse,
    permissions,
    scope,
    strict_auto_review
);
dto!(UserInputOption, description, label);
dto!(
    UserInputQuestion,
    header,
    id,
    options,
    question,
    is_other,
    is_secret
);
dto!(
    UserInputParams,
    auto_resolution_ms,
    is_blocking,
    item_id,
    questions,
    thread_id,
    turn_id
);
dto!(UserInputAnswer, answers);
dto!(UserInputResponse, answers);
dto!(ElicitationResponse, action, meta, content);
dto!(
    LegacyExecApprovalParams,
    approval_id,
    call_id,
    command,
    conversation_id,
    cwd,
    parsed_cmd,
    reason
);
dto!(
    LegacyPatchApprovalParams,
    call_id,
    conversation_id,
    file_changes,
    grant_root,
    reason
);
dto!(FileChangeApprovalResponse, decision);
dto!(LegacyExecApprovalResponse, decision);
dto!(LegacyPatchApprovalResponse, decision);
dto!(
    CollaborationSettings,
    model,
    reasoning_effort,
    developer_instructions
);
dto!(
    RateLimitSnapshot,
    limit_id,
    plan_type,
    rate_limit_reached_type,
    primary,
    secondary,
    individual_limit,
    credits,
    limit_name,
    normal_model_slug,
    spend_control_reached
);
impl Audit for ThreadItem {
    fn audit(&self, f: &mut BTreeSet<String>) {
        self.body.audit(f);
    }
}
impl Audit for UserInput {
    fn audit(&self, found: &mut BTreeSet<String>) {
        match self {
            Self::Text {
                text,
                text_elements,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("UserInput.text.{key}"));
                }
                text.audit(found);
                text_elements.audit(found);
            }
            Self::Image { detail, url, extra } => {
                for key in extra.keys() {
                    found.insert(format!("UserInput.image.{key}"));
                }
                detail.audit(found);
                url.audit(found);
            }
            Self::ImageFile {
                detail,
                file_id,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("UserInput.image.{key}"));
                }
                detail.audit(found);
                file_id.audit(found);
            }
            Self::LocalImage {
                detail,
                path,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("UserInput.localImage.{key}"));
                }
                detail.audit(found);
                path.audit(found);
            }
            Self::Audio { url, extra } => {
                for key in extra.keys() {
                    found.insert(format!("UserInput.audio.{key}"));
                }
                url.audit(found);
            }
            Self::LocalAudio { path, extra } => {
                for key in extra.keys() {
                    found.insert(format!("UserInput.localAudio.{key}"));
                }
                path.audit(found);
            }
            Self::Skill { name, path, extra } => {
                for key in extra.keys() {
                    found.insert(format!("UserInput.skill.{key}"));
                }
                name.audit(found);
                path.audit(found);
            }
            Self::Mention { name, path, extra } => {
                for key in extra.keys() {
                    found.insert(format!("UserInput.mention.{key}"));
                }
                name.audit(found);
                path.audit(found);
            }
            Self::Other { kind, .. } => {
                found.insert(format!("UserInput.{kind}:Other"));
            }
        }
    }
}
impl Audit for ItemBody {
    fn audit(&self, found: &mut BTreeSet<String>) {
        match self {
            Self::UserMessage {
                client_id,
                content,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("ItemBody.userMessage.{key}"));
                }
                client_id.audit(found);
                content.audit(found);
            }
            Self::HookPrompt { fragments, extra } => {
                for key in extra.keys() {
                    found.insert(format!("ItemBody.hookPrompt.{key}"));
                }
                fragments.audit(found);
            }
            Self::AgentMessage {
                delivery,
                memory_citation,
                phase,
                questions,
                text,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("ItemBody.agentMessage.{key}"));
                }
                delivery.audit(found);
                memory_citation.audit(found);
                phase.audit(found);
                questions.audit(found);
                text.audit(found);
            }
            Self::FunctionCallOutput {
                name,
                namespace,
                output,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("ItemBody.functionCallOutput.{key}"));
                }
                name.audit(found);
                namespace.audit(found);
                output.audit(found);
            }
            Self::Plan { text, extra } => {
                for key in extra.keys() {
                    found.insert(format!("ItemBody.plan.{key}"));
                }
                text.audit(found);
            }
            Self::Reasoning {
                content,
                summary,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("ItemBody.reasoning.{key}"));
                }
                content.audit(found);
                summary.audit(found);
            }
            Self::CommandExecution {
                aggregated_output,
                command,
                command_actions,
                cwd,
                duration_ms,
                exit_code,
                plugin_id,
                process_id,
                script_path,
                source,
                status,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("ItemBody.commandExecution.{key}"));
                }
                aggregated_output.audit(found);
                command.audit(found);
                command_actions.audit(found);
                cwd.audit(found);
                duration_ms.audit(found);
                exit_code.audit(found);
                plugin_id.audit(found);
                process_id.audit(found);
                script_path.audit(found);
                source.audit(found);
                status.audit(found);
            }
            Self::FileChange {
                changes,
                status,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("ItemBody.fileChange.{key}"));
                }
                changes.audit(found);
                status.audit(found);
            }
            Self::McpToolCall {
                app_context,
                arguments,
                duration_ms,
                error,
                mcp_app_resource_uri,
                mcp_app_ui,
                plugin_id,
                read_only_hint,
                result,
                server,
                status,
                tool,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("ItemBody.mcpToolCall.{key}"));
                }
                app_context.audit(found);
                arguments.audit(found);
                duration_ms.audit(found);
                error.audit(found);
                mcp_app_resource_uri.audit(found);
                mcp_app_ui.audit(found);
                plugin_id.audit(found);
                read_only_hint.audit(found);
                result.audit(found);
                server.audit(found);
                status.audit(found);
                tool.audit(found);
            }
            Self::DynamicToolCall {
                arguments,
                content_items,
                duration_ms,
                namespace,
                status,
                success,
                tool,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("ItemBody.dynamicToolCall.{key}"));
                }
                arguments.audit(found);
                content_items.audit(found);
                duration_ms.audit(found);
                namespace.audit(found);
                status.audit(found);
                success.audit(found);
                tool.audit(found);
            }
            Self::CollabAgentToolCall {
                agents_states,
                model,
                prompt,
                reasoning_effort,
                receiver_thread_ids,
                sender_thread_id,
                status,
                tool,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("ItemBody.collabAgentToolCall.{key}"));
                }
                agents_states.audit(found);
                model.audit(found);
                prompt.audit(found);
                reasoning_effort.audit(found);
                receiver_thread_ids.audit(found);
                sender_thread_id.audit(found);
                status.audit(found);
                tool.audit(found);
            }
            Self::SubAgentActivity {
                agent_path,
                agent_thread_id,
                kind,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("ItemBody.subAgentActivity.{key}"));
                }
                agent_path.audit(found);
                agent_thread_id.audit(found);
                kind.audit(found);
            }
            Self::WebSearch {
                action,
                query,
                results,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("ItemBody.webSearch.{key}"));
                }
                action.audit(found);
                query.audit(found);
                results.audit(found);
            }
            Self::ImageView { path, extra } => {
                for key in extra.keys() {
                    found.insert(format!("ItemBody.imageView.{key}"));
                }
                path.audit(found);
            }
            Self::Sleep { duration_ms, extra } => {
                for key in extra.keys() {
                    found.insert(format!("ItemBody.sleep.{key}"));
                }
                duration_ms.audit(found);
            }
            Self::ImageGeneration {
                failure,
                result,
                revised_prompt,
                saved_path,
                status,
                transparent_background,
                file_id,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("ItemBody.imageGeneration.{key}"));
                }
                failure.audit(found);
                result.audit(found);
                revised_prompt.audit(found);
                saved_path.audit(found);
                status.audit(found);
                transparent_background.audit(found);
                file_id.audit(found);
            }
            Self::EnteredReviewMode { review, extra } => {
                for key in extra.keys() {
                    found.insert(format!("ItemBody.enteredReviewMode.{key}"));
                }
                review.audit(found);
            }
            Self::ExitedReviewMode { review, extra } => {
                for key in extra.keys() {
                    found.insert(format!("ItemBody.exitedReviewMode.{key}"));
                }
                review.audit(found);
            }
            Self::ContextCompaction { extra } => {
                for key in extra.keys() {
                    found.insert(format!("ItemBody.contextCompaction.{key}"));
                }
            }
            Self::Other { kind, .. } => {
                found.insert(format!("ItemBody.{kind}:Other"));
            }
        }
    }
}
impl Audit for FileChangeKind {
    fn audit(&self, found: &mut BTreeSet<String>) {
        match self {
            Self::Add { extra } => {
                for key in extra.keys() {
                    found.insert(format!("FileChangeKind.add.{key}"));
                }
            }
            Self::Delete { extra } => {
                for key in extra.keys() {
                    found.insert(format!("FileChangeKind.delete.{key}"));
                }
            }
            Self::Update { move_path, extra } => {
                for key in extra.keys() {
                    found.insert(format!("FileChangeKind.update.{key}"));
                }
                move_path.audit(found);
            }
            Self::Other { kind, .. } => {
                found.insert(format!("FileChangeKind.{kind}:Other"));
            }
        }
    }
}
impl Audit for ElicitationParams {
    fn audit(&self, found: &mut BTreeSet<String>) {
        match self {
            Self::Form {
                server_name,
                thread_id,
                turn_id,
                meta,
                message,
                requested_schema,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("ElicitationParams.form.{key}"));
                }
                server_name.audit(found);
                thread_id.audit(found);
                turn_id.audit(found);
                meta.audit(found);
                message.audit(found);
                requested_schema.audit(found);
            }
            Self::OpenaiForm {
                server_name,
                thread_id,
                turn_id,
                meta,
                message,
                requested_schema,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("ElicitationParams.openai/form.{key}"));
                }
                server_name.audit(found);
                thread_id.audit(found);
                turn_id.audit(found);
                meta.audit(found);
                message.audit(found);
                requested_schema.audit(found);
            }
            Self::LegacyOpenaiForm {
                server_name,
                thread_id,
                turn_id,
                meta,
                message,
                requested_schema,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("ElicitationParams.openaiForm.{key}"));
                }
                server_name.audit(found);
                thread_id.audit(found);
                turn_id.audit(found);
                meta.audit(found);
                message.audit(found);
                requested_schema.audit(found);
            }
            Self::Url {
                server_name,
                thread_id,
                turn_id,
                meta,
                message,
                elicitation_id,
                url,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("ElicitationParams.url.{key}"));
                }
                server_name.audit(found);
                thread_id.audit(found);
                turn_id.audit(found);
                meta.audit(found);
                message.audit(found);
                elicitation_id.audit(found);
                url.audit(found);
            }
            Self::Other { kind, .. } => {
                found.insert(format!("ElicitationParams.{kind}:Other"));
            }
        }
    }
}
impl Audit for Account {
    fn audit(&self, found: &mut BTreeSet<String>) {
        match self {
            Self::ApiKey { extra } => {
                for key in extra.keys() {
                    found.insert(format!("Account.apiKey.{key}"));
                }
            }
            Self::Chatgpt {
                email,
                plan_type,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("Account.chatgpt.{key}"));
                }
                email.audit(found);
                plan_type.audit(found);
            }
            Self::AmazonBedrock {
                uses_codex_managed_credentials,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("Account.amazonBedrock.{key}"));
                }
                uses_codex_managed_credentials.audit(found);
            }
            Self::Other { kind, .. } => {
                found.insert(format!("Account.{kind}:Other"));
            }
        }
    }
}
impl Audit for ThreadStatus {
    fn audit(&self, found: &mut BTreeSet<String>) {
        match self {
            Self::NotLoaded { extra } => {
                for key in extra.keys() {
                    found.insert(format!("ThreadStatus.notLoaded.{key}"));
                }
            }
            Self::Idle { extra } => {
                for key in extra.keys() {
                    found.insert(format!("ThreadStatus.idle.{key}"));
                }
            }
            Self::SystemError { extra } => {
                for key in extra.keys() {
                    found.insert(format!("ThreadStatus.systemError.{key}"));
                }
            }
            Self::Active {
                active_flags,
                extra,
            } => {
                for key in extra.keys() {
                    found.insert(format!("ThreadStatus.active.{key}"));
                }
                active_flags.audit(found);
            }
            Self::Other { kind, .. } => {
                found.insert(format!("ThreadStatus.{kind}:Other"));
            }
        }
    }
}

impl Audit for t3_codex_app_server::wire::RequestId {
    fn audit(&self, _: &mut BTreeSet<String>) {}
}
dto!(RequestResolved, request_id, thread_id);
impl Audit for FunctionCallOutputBody {
    fn audit(&self, found: &mut BTreeSet<String>) {
        if matches!(self, Self::Other(_)) {
            found.insert("FunctionCallOutputBody:Other".into());
        }
    }
}

// JavaScript has one JSON number type: 61 and 61.0 are equal; keys and nulls remain exact.
pub fn numbers(value: Value) -> Value {
    match value {
        Value::Number(n) => serde_json::json!(n.as_f64().unwrap()),
        Value::Array(a) => Value::Array(a.into_iter().map(numbers).collect()),
        Value::Object(o) => Value::Object(o.into_iter().map(|(k, v)| (k, numbers(v))).collect()),
        other => other,
    }
}
