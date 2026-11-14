use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationV2Actor {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "agent")]
    Agent,
    #[serde(rename = "system")]
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationV2CreationSource {
    #[serde(rename = "web")]
    Web,
    #[serde(rename = "mobile")]
    Mobile,
    #[serde(rename = "mcp")]
    Mcp,
    #[serde(rename = "provider")]
    Provider,
    #[serde(rename = "server")]
    Server,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationV2ThreadHistoryOrigin {
    #[serde(rename = "native")]
    Native,
    #[serde(rename = "v1_import")]
    V1Import,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2LinkOrigin {
    #[serde(rename = "sessionId")]
    pub session_id: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "label")]
    pub label: String,
    #[serde(
        rename = "access",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub access: Option<Option<crate::auth::AuthMcpClientAccess>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2DelegatedFrom {
    #[serde(rename = "environmentId")]
    pub environment_id: crate::base_schemas::EnvironmentId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "title")]
    pub title: String,
}

pub type OrchestrationV2LinkedSender = OrchestrationV2DelegatedFrom;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationV2NativeRefStrength {
    #[serde(rename = "strong")]
    Strong,
    #[serde(rename = "weak")]
    Weak,
    #[serde(rename = "none")]
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ProviderRef {
    #[serde(rename = "driver")]
    pub driver: crate::provider_instance::ProviderDriverKind,
    #[serde(rename = "nativeId", with = "crate::wire::nullable")]
    pub native_id: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(rename = "strength")]
    pub strength: OrchestrationV2NativeRefStrength,
    #[serde(
        rename = "fingerprint",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub fingerprint: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "ordinal",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub ordinal: Option<Option<crate::base_schemas::NonNegativeInt>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AppThreadLineageRelationshipToParent {
    #[serde(rename = "fork")]
    Fork,
    #[serde(rename = "subagent")]
    Subagent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2AppThreadLineage {
    #[serde(rename = "parentThreadId", with = "crate::wire::nullable")]
    pub parent_thread_id: Option<crate::base_schemas::ThreadId>,
    #[serde(rename = "relationshipToParent", with = "crate::wire::nullable")]
    pub relationship_to_parent: Option<AppThreadLineageRelationshipToParent>,
    #[serde(rename = "rootThreadId")]
    pub root_thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "sideChat",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub side_chat: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationV2ContextTransferType {
    #[serde(rename = "fork")]
    Fork,
    #[serde(rename = "provider_handoff")]
    ProviderHandoff,
    #[serde(rename = "merge_back")]
    MergeBack,
    #[serde(rename = "subagent_spawn")]
    SubagentSpawn,
    #[serde(rename = "subagent_result")]
    SubagentResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ContextSourcePoint {
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "checkpointId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub checkpoint_id: Option<Option<crate::base_schemas::CheckpointId>>,
    #[serde(
        rename = "turnItemId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub turn_item_id: Option<Option<crate::base_schemas::TurnItemId>>,
    #[serde(
        rename = "providerThreadRef",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_thread_ref: Option<Option<OrchestrationV2ProviderRef>>,
    #[serde(
        rename = "providerTurnRef",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_turn_ref: Option<Option<OrchestrationV2ProviderRef>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadForkSourcePointLatestStable {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadForkSourcePointRun {
    #[serde(rename = "runId")]
    pub run_id: crate::base_schemas::RunId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadForkSourcePointCheckpoint {
    #[serde(rename = "checkpointId")]
    pub checkpoint_id: crate::base_schemas::CheckpointId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OrchestrationV2ThreadForkSourcePoint {
    #[serde(rename = "latest_stable")]
    LatestStable(Box<ThreadForkSourcePointLatestStable>),
    #[serde(rename = "run")]
    Run(Box<ThreadForkSourcePointRun>),
    #[serde(rename = "checkpoint")]
    Checkpoint(Box<ThreadForkSourcePointCheckpoint>),
}
impl crate::wire::TaggedUnion for OrchestrationV2ThreadForkSourcePoint {
    const TAG: &'static str = "type";
    const KNOWN_TAGS: &'static [&'static str] = &["latest_stable", "run", "checkpoint"];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextTransferResolutionNativeFork {
    #[serde(rename = "providerThreadRef")]
    pub provider_thread_ref: OrchestrationV2ProviderRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextTransferResolutionPortableContext {
    #[serde(rename = "contextHandoffId")]
    pub context_handoff_id: crate::base_schemas::ContextHandoffId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "strategy")]
pub enum OrchestrationV2ContextTransferResolution {
    #[serde(rename = "native_fork")]
    NativeFork(Box<ContextTransferResolutionNativeFork>),
    #[serde(rename = "portable_context")]
    PortableContext(Box<ContextTransferResolutionPortableContext>),
    #[serde(rename = "delta_context")]
    DeltaContext(Box<ContextTransferResolutionPortableContext>),
    #[serde(rename = "fork_delta_context")]
    ForkDeltaContext(Box<ContextTransferResolutionPortableContext>),
    #[serde(rename = "checkpoint_context")]
    CheckpointContext(Box<ContextTransferResolutionPortableContext>),
}
impl crate::wire::TaggedUnion for OrchestrationV2ContextTransferResolution {
    const TAG: &'static str = "strategy";
    const KNOWN_TAGS: &'static [&'static str] = &[
        "native_fork",
        "portable_context",
        "delta_context",
        "fork_delta_context",
        "checkpoint_context",
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ProviderTurnTokenUsage {
    #[serde(rename = "usedTokens")]
    pub used_tokens: crate::base_schemas::NonNegativeInt,
    #[serde(
        rename = "maxTokens",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub max_tokens: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "inputTokens",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub input_tokens: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "cachedInputTokens",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cached_input_tokens: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "outputTokens",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub output_tokens: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "reasoningOutputTokens",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reasoning_output_tokens: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "promptCacheTtlMs",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub prompt_cache_ttl_ms: Option<Option<crate::base_schemas::PositiveInt>>,
    #[serde(
        rename = "autoCompactThreshold",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub auto_compact_threshold: Option<Option<crate::base_schemas::PositiveInt>>,
    #[serde(
        rename = "promptCacheExpiresAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub prompt_cache_expires_at: Option<Option<String>>,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ContextTransferStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "resolved_native")]
    ResolvedNative,
    #[serde(rename = "resolved_portable")]
    ResolvedPortable,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "consumed")]
    Consumed,
    #[serde(rename = "superseded")]
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ContextTransfer {
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ContextTransferId>,
    #[serde(rename = "type")]
    pub r#type: OrchestrationV2ContextTransferType,
    #[serde(rename = "sourceThreadId")]
    pub source_thread_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "targetThreadId")]
    pub target_thread_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "sourcePoint")]
    pub source_point: OrchestrationV2ContextSourcePoint,
    #[serde(rename = "basePoint", with = "crate::wire::nullable")]
    pub base_point: Option<OrchestrationV2ContextSourcePoint>,
    #[serde(rename = "sourceProviderInstanceId", with = "crate::wire::nullable")]
    pub source_provider_instance_id: Option<
        crate::orchestration_v2_id_table::IdOrIndex<crate::provider_instance::ProviderInstanceId>,
    >,
    #[serde(
        rename = "sourceTokenUsage",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub source_token_usage: Option<Option<OrchestrationV2ProviderTurnTokenUsage>>,
    #[serde(rename = "targetProviderInstanceId", with = "crate::wire::nullable")]
    pub target_provider_instance_id: Option<
        crate::orchestration_v2_id_table::IdOrIndex<crate::provider_instance::ProviderInstanceId>,
    >,
    #[serde(rename = "targetRunId", with = "crate::wire::nullable")]
    pub target_run_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "status")]
    pub status: ContextTransferStatus,
    #[serde(rename = "resolution", with = "crate::wire::nullable")]
    pub resolution: Option<OrchestrationV2ContextTransferResolution>,
    #[serde(rename = "createdBy")]
    pub created_by: OrchestrationV2Actor,
    #[serde(rename = "error", with = "crate::wire::nullable")]
    pub error: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: crate::wire::WireDateTime,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "consumedAt", with = "crate::wire::nullable")]
    pub consumed_at: Option<crate::wire::WireDateTime>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2SessionCapabilities {
    #[serde(rename = "supportsMultipleProviderThreadsPerSession")]
    pub supports_multiple_provider_threads_per_session: bool,
    #[serde(rename = "supportsModelSwitchInSession")]
    pub supports_model_switch_in_session: bool,
    #[serde(rename = "supportsProviderSwitchingViaHandoff")]
    pub supports_provider_switching_via_handoff: bool,
    #[serde(rename = "supportsRuntimeModeSwitchInSession")]
    pub supports_runtime_mode_switch_in_session: bool,
    #[serde(rename = "pendingRequestsSurviveRestart")]
    pub pending_requests_survive_restart: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ThreadCapabilities {
    #[serde(rename = "canCreateEmptyThread")]
    pub can_create_empty_thread: bool,
    #[serde(rename = "canReadThreadSnapshot")]
    pub can_read_thread_snapshot: bool,
    #[serde(rename = "canRollbackThread")]
    pub can_rollback_thread: bool,
    #[serde(rename = "canForkThread")]
    pub can_fork_thread: bool,
    #[serde(rename = "canForkFromTurn")]
    pub can_fork_from_turn: bool,
    #[serde(rename = "canForkFromSubagentThread")]
    pub can_fork_from_subagent_thread: bool,
    #[serde(rename = "exposesNativeThreadId")]
    pub exposes_native_thread_id: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2TurnCapabilities {
    #[serde(
        rename = "supportsNativeGoals",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub supports_native_goals: Option<Option<bool>>,
    #[serde(rename = "exposesNativeTurnId")]
    pub exposes_native_turn_id: bool,
    #[serde(rename = "emitsTurnStarted")]
    pub emits_turn_started: bool,
    #[serde(rename = "emitsTurnCompleted")]
    pub emits_turn_completed: bool,
    #[serde(rename = "supportsInterrupt")]
    pub supports_interrupt: bool,
    #[serde(rename = "supportsActiveSteering")]
    pub supports_active_steering: bool,
    #[serde(
        rename = "activeSteeringInterruptsTools",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub active_steering_interrupts_tools: Option<Option<bool>>,
    #[serde(
        rename = "activeSteeringAcceptsAttachments",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub active_steering_accepts_attachments: Option<Option<bool>>,
    #[serde(
        rename = "activeSteeringDuringCompaction",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub active_steering_during_compaction: Option<Option<bool>>,
    #[serde(rename = "supportsSteeringByInterruptRestart")]
    pub supports_steering_by_interrupt_restart: bool,
    #[serde(rename = "supportsQueuedMessages")]
    pub supports_queued_messages: bool,
    #[serde(rename = "terminalStatusQuality")]
    pub terminal_status_quality: OrchestrationV2NativeRefStrength,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2StreamingCapabilities {
    #[serde(rename = "streamsAssistantText")]
    pub streams_assistant_text: bool,
    #[serde(rename = "streamsReasoning")]
    pub streams_reasoning: bool,
    #[serde(rename = "streamsToolOutput")]
    pub streams_tool_output: bool,
    #[serde(rename = "streamsPlanText")]
    pub streams_plan_text: bool,
    #[serde(rename = "emitsMessageCompleted")]
    pub emits_message_completed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ToolCapabilities {
    #[serde(rename = "exposesToolItemIds")]
    pub exposes_tool_item_ids: bool,
    #[serde(rename = "emitsToolStarted")]
    pub emits_tool_started: bool,
    #[serde(rename = "emitsToolCompleted")]
    pub emits_tool_completed: bool,
    #[serde(rename = "emitsToolOutput")]
    pub emits_tool_output: bool,
    #[serde(rename = "supportsMcpTools")]
    pub supports_mcp_tools: bool,
    #[serde(rename = "supportsDynamicToolCallbacks")]
    pub supports_dynamic_tool_callbacks: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ApprovalCapabilities {
    #[serde(rename = "supportsCommandApproval")]
    pub supports_command_approval: bool,
    #[serde(rename = "supportsFileReadApproval")]
    pub supports_file_read_approval: bool,
    #[serde(rename = "supportsFileChangeApproval")]
    pub supports_file_change_approval: bool,
    #[serde(rename = "supportsApplyPatchApproval")]
    pub supports_apply_patch_approval: bool,
    #[serde(rename = "approvalsHaveNativeRequestIds")]
    pub approvals_have_native_request_ids: bool,
    #[serde(rename = "approvalCallbacksAreLiveOnly")]
    pub approval_callbacks_are_live_only: bool,
    #[serde(rename = "approvalsCanOriginateFromSubagents")]
    pub approvals_can_originate_from_subagents: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2PlanningCapabilities {
    #[serde(rename = "emitsPlanUpdated")]
    pub emits_plan_updated: bool,
    #[serde(rename = "emitsTodoList")]
    pub emits_todo_list: bool,
    #[serde(rename = "emitsProposedPlan")]
    pub emits_proposed_plan: bool,
    #[serde(rename = "supportsStructuredQuestions")]
    pub supports_structured_questions: bool,
    #[serde(rename = "planDeltasHaveItemIds")]
    pub plan_deltas_have_item_ids: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2SubagentCapabilities {
    #[serde(rename = "supportsSubagents")]
    pub supports_subagents: bool,
    #[serde(rename = "exposesSubagentThreadIds")]
    pub exposes_subagent_thread_ids: bool,
    #[serde(rename = "emitsSubagentLifecycle")]
    pub emits_subagent_lifecycle: bool,
    #[serde(rename = "canWaitForSubagents")]
    pub can_wait_for_subagents: bool,
    #[serde(rename = "canCloseSubagents")]
    pub can_close_subagents: bool,
    #[serde(rename = "canForkSubagentThread")]
    pub can_fork_subagent_thread: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ContextCapabilities {
    #[serde(rename = "acceptsSystemContext")]
    pub accepts_system_context: bool,
    #[serde(rename = "acceptsDeveloperContext")]
    pub accepts_developer_context: bool,
    #[serde(rename = "acceptsSyntheticUserContext")]
    pub accepts_synthetic_user_context: bool,
    #[serde(rename = "canGenerateSummaries")]
    pub can_generate_summaries: bool,
    #[serde(rename = "canConsumeHandoffSummaries")]
    pub can_consume_handoff_summaries: bool,
    #[serde(rename = "supportsDeltaHandoff")]
    pub supports_delta_handoff: bool,
    #[serde(rename = "supportsFullThreadHandoff")]
    pub supports_full_thread_handoff: bool,
    #[serde(rename = "maxRecommendedHandoffChars", with = "crate::wire::nullable")]
    pub max_recommended_handoff_chars: Option<crate::base_schemas::PositiveInt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2CheckpointCapabilities {
    #[serde(rename = "appCanCheckpointFilesystem")]
    pub app_can_checkpoint_filesystem: bool,
    #[serde(rename = "supportsNestedCheckpointScopes")]
    pub supports_nested_checkpoint_scopes: bool,
    #[serde(rename = "providerCanRollbackConversation")]
    pub provider_can_rollback_conversation: bool,
    #[serde(rename = "providerRollbackReturnsSnapshot")]
    pub provider_rollback_returns_snapshot: bool,
    #[serde(rename = "providerCanReadConversationSnapshot")]
    pub provider_can_read_conversation_snapshot: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RuntimePolicyCapabilitiesEnforcement {
    #[serde(rename = "native")]
    Native,
    #[serde(rename = "client-boundary")]
    ClientBoundary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2RuntimePolicyCapabilities {
    #[serde(rename = "enforcement")]
    pub enforcement: RuntimePolicyCapabilitiesEnforcement,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2IdentityCapabilities {
    #[serde(rename = "nativeThreadIds")]
    pub native_thread_ids: OrchestrationV2NativeRefStrength,
    #[serde(rename = "nativeTurnIds")]
    pub native_turn_ids: OrchestrationV2NativeRefStrength,
    #[serde(rename = "nativeItemIds")]
    pub native_item_ids: OrchestrationV2NativeRefStrength,
    #[serde(rename = "nativeRequestIds")]
    pub native_request_ids: OrchestrationV2NativeRefStrength,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ProviderCapabilities {
    #[serde(rename = "sessions")]
    pub sessions: OrchestrationV2SessionCapabilities,
    #[serde(rename = "threads")]
    pub threads: OrchestrationV2ThreadCapabilities,
    #[serde(rename = "turns")]
    pub turns: OrchestrationV2TurnCapabilities,
    #[serde(rename = "streaming")]
    pub streaming: OrchestrationV2StreamingCapabilities,
    #[serde(rename = "tools")]
    pub tools: OrchestrationV2ToolCapabilities,
    #[serde(rename = "approvals")]
    pub approvals: OrchestrationV2ApprovalCapabilities,
    #[serde(rename = "planning")]
    pub planning: OrchestrationV2PlanningCapabilities,
    #[serde(rename = "subagents")]
    pub subagents: OrchestrationV2SubagentCapabilities,
    #[serde(rename = "context")]
    pub context: OrchestrationV2ContextCapabilities,
    #[serde(rename = "checkpointing")]
    pub checkpointing: OrchestrationV2CheckpointCapabilities,
    #[serde(rename = "identity")]
    pub identity: OrchestrationV2IdentityCapabilities,
    #[serde(
        rename = "runtimePolicy",
        default = "provider_capabilities_runtime_policy_default"
    )]
    pub runtime_policy: OrchestrationV2RuntimePolicyCapabilities,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2LimitRecovery {
    #[serde(
        rename = "requestId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub request_id: Option<Option<crate::base_schemas::CommandId>>,
    #[serde(rename = "runId")]
    pub run_id: crate::base_schemas::RunId,
    #[serde(rename = "resetAt")]
    pub reset_at: crate::base_schemas::IsoDateTime,
    #[serde(rename = "autoResume")]
    pub auto_resume: bool,
    #[serde(
        rename = "snooze",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub snooze: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2LimitRecoveryUpdate {
    #[serde(rename = "runId")]
    pub run_id: crate::base_schemas::RunId,
    #[serde(rename = "resetAt")]
    pub reset_at: crate::base_schemas::IsoDateTime,
    #[serde(
        rename = "autoResume",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub auto_resume: Option<Option<bool>>,
    #[serde(
        rename = "snooze",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub snooze: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadOpensPicksItem {
    #[serde(rename = "query")]
    pub query: String,
    #[serde(rename = "at")]
    pub at: crate::wire::JsNumber,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ThreadOpens {
    #[serde(rename = "openedAt")]
    pub opened_at: crate::wire::JsNumber,
    #[serde(rename = "score", deserialize_with = "deserialize_open_score")]
    pub score: crate::wire::JsNumber,
    #[serde(rename = "picks")]
    pub picks: Vec<ThreadOpensPicksItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppThreadForkedFromRun {
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "runId")]
    pub run_id: crate::base_schemas::RunId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppThreadForkedFromNode {
    #[serde(rename = "nodeId")]
    pub node_id: crate::base_schemas::NodeId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppThreadForkedFromProviderThread {
    #[serde(rename = "providerThreadId")]
    pub provider_thread_id: crate::base_schemas::ProviderThreadId,
    #[serde(
        rename = "providerTurnId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_turn_id: Option<Option<crate::base_schemas::ProviderTurnId>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AppThreadForkedFrom {
    #[serde(rename = "run")]
    Run(Box<AppThreadForkedFromRun>),
    #[serde(rename = "node")]
    Node(Box<AppThreadForkedFromNode>),
    #[serde(rename = "provider_thread")]
    ProviderThread(Box<AppThreadForkedFromProviderThread>),
}
impl crate::wire::TaggedUnion for AppThreadForkedFrom {
    const TAG: &'static str = "type";
    const KNOWN_TAGS: &'static [&'static str] = &["run", "node", "provider_thread"];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AppThreadSettledOverride {
    #[serde(rename = "settled")]
    Settled,
    #[serde(rename = "active")]
    Active,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppThreadTitleRegeneration {
    #[serde(rename = "requestId")]
    pub request_id: crate::base_schemas::CommandId,
    #[serde(rename = "startedAt")]
    pub started_at: crate::wire::WireDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2LastTitleRegeneration {
    #[serde(rename = "status")]
    pub status: crate::thread_title::ThreadTitleRegenerationStatus,
    #[serde(
        rename = "message",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub message: Option<Option<String>>,
    #[serde(rename = "requestId")]
    pub request_id: crate::base_schemas::CommandId,
    #[serde(rename = "completedAt")]
    pub completed_at: crate::wire::WireDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppThreadRollbackFailure {
    #[serde(rename = "requestId")]
    pub request_id: crate::base_schemas::CommandId,
    #[serde(rename = "message")]
    pub message: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2AppThread {
    #[serde(rename = "createdBy")]
    pub created_by: OrchestrationV2Actor,
    #[serde(rename = "creationSource")]
    pub creation_source: OrchestrationV2CreationSource,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "projectId")]
    pub project_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProjectId>,
    #[serde(rename = "title")]
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "providerInstanceId")]
    pub provider_instance_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::provider_instance::ProviderInstanceId>,
    #[serde(rename = "modelSelection")]
    pub model_selection: crate::model_selection::ModelSelection,
    #[serde(rename = "runtimeMode")]
    pub runtime_mode: crate::provider_policy::RuntimeMode,
    #[serde(rename = "interactionMode")]
    pub interaction_mode: crate::provider_policy::ProviderInteractionMode,
    #[serde(rename = "branch", with = "crate::wire::nullable")]
    pub branch: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(rename = "worktreePath", with = "crate::wire::nullable")]
    pub worktree_path: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(
        rename = "linkedPullRequest",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub linked_pull_request: Option<Option<crate::thread_pull_request::ThreadLinkedPullRequest>>,
    #[serde(
        rename = "pullRequests",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub pull_requests: Option<Option<Vec<crate::thread_pull_request::ThreadPullRequestLink>>>,
    #[serde(
        rename = "branchPullRequest",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub branch_pull_request: Option<Option<crate::thread_pull_request::ThreadLinkedPullRequest>>,
    #[serde(rename = "activeProviderThreadId", with = "crate::wire::nullable")]
    pub active_provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(
        rename = "historyOrigin",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub history_origin: Option<Option<OrchestrationV2ThreadHistoryOrigin>>,
    #[serde(
        rename = "linkOrigin",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub link_origin: Option<Option<OrchestrationV2LinkOrigin>>,
    #[serde(
        rename = "delegatedFrom",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub delegated_from: Option<Option<OrchestrationV2DelegatedFrom>>,
    #[serde(rename = "lineage")]
    pub lineage: OrchestrationV2AppThreadLineage,
    #[serde(rename = "forkedFrom", with = "crate::wire::nullable")]
    pub forked_from: Option<AppThreadForkedFrom>,
    #[serde(rename = "createdAt")]
    pub created_at: crate::wire::WireDateTime,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "archivedAt", with = "crate::wire::nullable")]
    pub archived_at: Option<crate::wire::WireDateTime>,
    #[serde(
        rename = "settledOverride",
        with = "crate::wire::nullable",
        default = "app_thread_settled_override_default"
    )]
    pub settled_override: Option<AppThreadSettledOverride>,
    #[serde(
        rename = "settledAt",
        with = "crate::wire::nullable",
        default = "app_thread_settled_at_default"
    )]
    pub settled_at: Option<crate::wire::WireDateTime>,
    #[serde(
        rename = "unsettledAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub unsettled_at: Option<Option<crate::wire::WireDateTime>>,
    #[serde(
        rename = "snoozedUntil",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub snoozed_until: Option<Option<crate::wire::WireDateTime>>,
    #[serde(
        rename = "snoozedAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub snoozed_at: Option<Option<crate::wire::WireDateTime>>,
    #[serde(
        rename = "lastSnoozeWakeAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub last_snooze_wake_at: Option<Option<crate::wire::WireDateTime>>,
    #[serde(
        rename = "limitRecovery",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub limit_recovery: Option<Option<OrchestrationV2LimitRecovery>>,
    #[serde(
        rename = "guestShare",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub guest_share: Option<Option<crate::guest_share::ThreadGuestShare>>,
    #[serde(
        rename = "pinnedAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub pinned_at: Option<Option<crate::wire::WireDateTime>>,
    #[serde(
        rename = "autoSettleDisabledAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub auto_settle_disabled_at: Option<Option<crate::wire::WireDateTime>>,
    #[serde(
        rename = "pinOrderKey",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub pin_order_key: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "activeOrderKey",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub active_order_key: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "tags",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tags: Option<Option<crate::thread_tags::StoredThreadTags>>,
    #[serde(
        rename = "lastVisitedAt",
        with = "crate::wire::nullable",
        default = "app_thread_last_visited_at_default"
    )]
    pub last_visited_at: Option<crate::wire::WireDateTime>,
    #[serde(
        rename = "opens",
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_lenient_opens"
    )]
    pub opens: Option<Option<OrchestrationV2ThreadOpens>>,
    #[serde(
        rename = "titleRegeneration",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub title_regeneration: Option<Option<AppThreadTitleRegeneration>>,
    #[serde(
        rename = "lastTitleRegeneration",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub last_title_regeneration: Option<Option<OrchestrationV2LastTitleRegeneration>>,
    #[serde(
        rename = "rollbackRequestId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub rollback_request_id:
        Option<Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::CommandId>>>,
    #[serde(
        rename = "rollbackFailure",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub rollback_failure: Option<Option<AppThreadRollbackFailure>>,
    #[serde(rename = "deletedAt", with = "crate::wire::nullable")]
    pub deleted_at: Option<crate::wire::WireDateTime>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationV2RunStatus {
    #[serde(rename = "preparing")]
    Preparing,
    #[serde(rename = "queued")]
    Queued,
    #[serde(rename = "starting")]
    Starting,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "interrupted")]
    Interrupted,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "rolled_back")]
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationV2DelegatedCompletionTaskDeliveryState {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "claimed")]
    Claimed,
    #[serde(rename = "acknowledged")]
    Acknowledged,
    #[serde(rename = "delivered")]
    Delivered,
    #[serde(rename = "disposed")]
    Disposed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2DelegatedCompletionTaskDelivery {
    #[serde(rename = "state")]
    pub state: OrchestrationV2DelegatedCompletionTaskDeliveryState,
    #[serde(rename = "observedByRunId", with = "crate::wire::nullable")]
    pub observed_by_run_id: Option<crate::base_schemas::RunId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2DelegatedCompletionDelivery {
    #[serde(rename = "generation")]
    pub generation: crate::base_schemas::PositiveInt,
    #[serde(rename = "messageId")]
    pub message_id: crate::base_schemas::MessageId,
    #[serde(rename = "taskIds")]
    pub task_ids: Vec<crate::base_schemas::NodeId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DelegatedCompletionCohortDisposition {
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "stopped")]
    Stopped,
    #[serde(rename = "disposed")]
    Disposed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2DelegatedCompletionCohort {
    #[serde(rename = "disposition")]
    pub disposition: DelegatedCompletionCohortDisposition,
    #[serde(rename = "nextGeneration")]
    pub next_generation: crate::base_schemas::PositiveInt,
    #[serde(rename = "delivery", with = "crate::wire::nullable")]
    pub delivery: Option<OrchestrationV2DelegatedCompletionDelivery>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RestartCancelledBackgroundWorkKind {
    #[serde(rename = "subagent")]
    Subagent,
    #[serde(rename = "shell")]
    Shell,
    #[serde(rename = "monitor")]
    Monitor,
    #[serde(rename = "task")]
    Task,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "bool", into = "bool")]
pub struct LiteralTrue;
impl TryFrom<bool> for LiteralTrue {
    type Error = &'static str;
    fn try_from(value: bool) -> Result<Self, Self::Error> {
        if value {
            Ok(Self)
        } else {
            Err("invalid literal")
        }
    }
}
impl From<LiteralTrue> for bool {
    fn from(_: LiteralTrue) -> Self {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2RestartCancelledBackgroundWork {
    #[serde(rename = "kind")]
    pub kind: RestartCancelledBackgroundWorkKind,
    #[serde(rename = "label")]
    pub label: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "id",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "awaited",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub awaited: Option<Option<LiteralTrue>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2RunBackgroundWorkCancelled {
    #[serde(rename = "runId")]
    pub run_id: crate::base_schemas::RunId,
    #[serde(rename = "restartCancelledBackgroundWork")]
    pub restart_cancelled_background_work: Vec<OrchestrationV2RestartCancelledBackgroundWork>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadLaunchWorkspaceStrategyRoot {
    #[serde(
        rename = "branch",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub branch: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadLaunchWorkspaceStrategyExistingWorktree {
    #[serde(rename = "worktreePath")]
    pub worktree_path: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "branch",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub branch: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadLaunchWorkspaceStrategyWorktree {
    #[serde(rename = "baseRef")]
    pub base_ref: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "branch",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub branch: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "startFromOrigin",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub start_from_origin: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OrchestrationV2ThreadLaunchWorkspaceStrategy {
    #[serde(rename = "root")]
    Root(Box<ThreadLaunchWorkspaceStrategyRoot>),
    #[serde(rename = "existing_worktree")]
    ExistingWorktree(Box<ThreadLaunchWorkspaceStrategyExistingWorktree>),
    #[serde(rename = "worktree")]
    Worktree(Box<ThreadLaunchWorkspaceStrategyWorktree>),
}
impl crate::wire::TaggedUnion for OrchestrationV2ThreadLaunchWorkspaceStrategy {
    const TAG: &'static str = "type";
    const KNOWN_TAGS: &'static [&'static str] = &["root", "existing_worktree", "worktree"];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSourcePlanRef {
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "planId")]
    pub plan_id: crate::base_schemas::PlanId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2Run {
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::PositiveInt,
    #[serde(rename = "providerInstanceId")]
    pub provider_instance_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::provider_instance::ProviderInstanceId>,
    #[serde(rename = "modelSelection")]
    pub model_selection: crate::model_selection::ModelSelection,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "userMessageId")]
    pub user_message_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::MessageId>,
    #[serde(rename = "rootNodeId", with = "crate::wire::nullable")]
    pub root_node_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "activeAttemptId", with = "crate::wire::nullable")]
    pub active_attempt_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunAttemptId>>,
    #[serde(rename = "status")]
    pub status: OrchestrationV2RunStatus,
    #[serde(
        rename = "queuePosition",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub queue_position: Option<Option<crate::base_schemas::PositiveInt>>,
    #[serde(
        rename = "queueHeld",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub queue_held: Option<Option<bool>>,
    #[serde(
        rename = "steerWhenReadyRunId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub steer_when_ready_run_id:
        Option<Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>>,
    #[serde(rename = "requestedAt")]
    pub requested_at: crate::wire::WireDateTime,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "checkpointId", with = "crate::wire::nullable")]
    pub checkpoint_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::CheckpointId>>,
    #[serde(rename = "contextHandoffId", with = "crate::wire::nullable")]
    pub context_handoff_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ContextHandoffId>>,
    #[serde(
        rename = "restartContinuationOfRunId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub restart_continuation_of_run_id:
        Option<Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>>,
    #[serde(
        rename = "usageLimitContinuationOfRunId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub usage_limit_continuation_of_run_id:
        Option<Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>>,
    #[serde(
        rename = "delegatedTaskId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub delegated_task_id:
        Option<Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>>,
    #[serde(
        rename = "workStartedAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub work_started_at: Option<Option<crate::wire::WireDateTime>>,
    #[serde(
        rename = "restartCancelledBackgroundWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub restart_cancelled_background_work:
        Option<Option<Vec<OrchestrationV2RestartCancelledBackgroundWork>>>,
    #[serde(
        rename = "sourcePlanRef",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub source_plan_ref: Option<Option<RunSourcePlanRef>>,
    #[serde(
        rename = "delegatedCompletion",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub delegated_completion: Option<Option<OrchestrationV2DelegatedCompletionCohort>>,
    #[serde(
        rename = "workspacePreparation",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub workspace_preparation: Option<Option<OrchestrationV2ThreadLaunchWorkspaceStrategy>>,
    #[serde(
        rename = "freshContext",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub fresh_context: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RunAttemptReason {
    #[serde(rename = "initial")]
    Initial,
    #[serde(rename = "steering_restart")]
    SteeringRestart,
    #[serde(rename = "retry")]
    Retry,
    #[serde(rename = "provider_recovery")]
    ProviderRecovery,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RunAttemptStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "interrupted")]
    Interrupted,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "superseded")]
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2RunAttempt {
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunAttemptId>,
    #[serde(
        rename = "nativeThreadId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub native_thread_id: Option<Option<crate::orchestration_v2_id_table::IdOrIndex<String>>>,
    #[serde(rename = "runId")]
    pub run_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>,
    #[serde(rename = "attemptOrdinal")]
    pub attempt_ordinal: crate::base_schemas::PositiveInt,
    #[serde(rename = "rootNodeId")]
    pub root_node_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>,
    #[serde(rename = "providerInstanceId")]
    pub provider_instance_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::provider_instance::ProviderInstanceId>,
    #[serde(rename = "providerThreadId")]
    pub provider_thread_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "reason")]
    pub reason: RunAttemptReason,
    #[serde(rename = "status")]
    pub status: RunAttemptStatus,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExecutionNodeKind {
    #[serde(rename = "root_turn")]
    RootTurn,
    #[serde(rename = "assistant_message")]
    AssistantMessage,
    #[serde(rename = "reasoning")]
    Reasoning,
    #[serde(rename = "plan")]
    Plan,
    #[serde(rename = "todo_list")]
    TodoList,
    #[serde(rename = "tool_call")]
    ToolCall,
    #[serde(rename = "approval_request")]
    ApprovalRequest,
    #[serde(rename = "user_input_request")]
    UserInputRequest,
    #[serde(rename = "subagent")]
    Subagent,
    #[serde(rename = "hook")]
    Hook,
    #[serde(rename = "system")]
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExecutionNodeStatus {
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "interrupted")]
    Interrupted,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "rolled_back")]
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ExecutionNode {
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "parentNodeId", with = "crate::wire::nullable")]
    pub parent_node_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "rootNodeId")]
    pub root_node_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>,
    #[serde(rename = "kind")]
    pub kind: ExecutionNodeKind,
    #[serde(rename = "status")]
    pub status: ExecutionNodeStatus,
    #[serde(rename = "countsForRun")]
    pub counts_for_run: bool,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "runtimeRequestId", with = "crate::wire::nullable")]
    pub runtime_request_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RuntimeRequestId>>,
    #[serde(rename = "checkpointScopeId", with = "crate::wire::nullable")]
    pub checkpoint_scope_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::CheckpointScopeId>>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2WorkflowPhase {
    #[serde(rename = "index")]
    pub index: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "title")]
    pub title: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WorkflowAgentState {
    #[serde(rename = "queued")]
    Queued,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2WorkflowAgent {
    #[serde(rename = "index")]
    pub index: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "label")]
    pub label: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "agentId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub agent_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "childThreadId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub child_thread_id: Option<Option<crate::base_schemas::ThreadId>>,
    #[serde(rename = "state")]
    pub state: WorkflowAgentState,
    #[serde(
        rename = "phaseIndex",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub phase_index: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "phaseTitle",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub phase_title: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "model",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub model: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "effort",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub effort: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "speed",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub speed: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "attempt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub attempt: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "totalTokens",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub total_tokens: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "toolCalls",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_calls: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "durationMs",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub duration_ms: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "queuedAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub queued_at: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "startedAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub started_at: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "completedAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub completed_at: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "prompt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub prompt: Option<Option<String>>,
    #[serde(
        rename = "result",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub result: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2WorkflowRunHandles {
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "transcriptDir",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub transcript_dir: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "scriptPath",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub script_path: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2SubagentWorkflow {
    #[serde(
        rename = "name",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub name: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "runHandles",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_handles: Option<Option<OrchestrationV2WorkflowRunHandles>>,
    #[serde(rename = "phases")]
    pub phases: Vec<OrchestrationV2WorkflowPhase>,
    #[serde(rename = "agents")]
    pub agents: Vec<OrchestrationV2WorkflowAgent>,
    #[serde(
        rename = "totalTokens",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub total_tokens: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "toolCalls",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_calls: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "durationMs",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub duration_ms: Option<Option<crate::base_schemas::NonNegativeInt>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2RemoteTaskChild {
    #[serde(rename = "environmentId")]
    pub environment_id: crate::base_schemas::EnvironmentId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "label")]
    pub label: String,
    #[serde(
        rename = "title",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<String>>,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "stoppedThere",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub stopped_there: Option<Option<bool>>,
    #[serde(
        rename = "stopCount",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub stop_count: Option<Option<crate::base_schemas::NonNegativeInt>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SubagentOrigin {
    #[serde(rename = "provider_native")]
    ProviderNative,
    #[serde(rename = "app_owned")]
    AppOwned,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SubagentCompletionWake {
    #[serde(rename = "always")]
    Always,
    #[serde(rename = "settled_only")]
    SettledOnly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationV2TurnItemStatus {
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "interrupted")]
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2Subagent {
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "parentNodeId")]
    pub parent_node_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>,
    #[serde(rename = "origin")]
    pub origin: SubagentOrigin,
    #[serde(rename = "createdBy")]
    pub created_by: OrchestrationV2Actor,
    #[serde(rename = "driver")]
    pub driver: crate::provider_instance::ProviderDriverKind,
    #[serde(rename = "providerInstanceId")]
    pub provider_instance_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::provider_instance::ProviderInstanceId>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "childThreadId", with = "crate::wire::nullable")]
    pub child_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>>,
    #[serde(
        rename = "remoteChild",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub remote_child: Option<Option<OrchestrationV2RemoteTaskChild>>,
    #[serde(rename = "nativeTaskRef", with = "crate::wire::nullable")]
    pub native_task_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "prompt")]
    pub prompt: String,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "model", with = "crate::wire::nullable")]
    pub model: Option<String>,
    #[serde(
        rename = "modelSelection",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub model_selection: Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        rename = "completionWake",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub completion_wake: Option<Option<SubagentCompletionWake>>,
    #[serde(
        rename = "completionDelivery",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub completion_delivery: Option<Option<OrchestrationV2DelegatedCompletionTaskDelivery>>,
    #[serde(
        rename = "cancelledRunId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cancelled_run_id:
        Option<Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>>,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(
        rename = "progress",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub progress: Option<Option<String>>,
    #[serde(
        rename = "workflow",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub workflow: Option<Option<OrchestrationV2SubagentWorkflow>>,
    #[serde(rename = "result", with = "crate::wire::nullable")]
    pub result: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(
        rename = "promptOmitted",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub prompt_omitted: Option<LiteralTrue>,
    #[serde(
        rename = "resultOmitted",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub result_omitted: Option<LiteralTrue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CheckpointScopeKind {
    #[serde(rename = "root_run")]
    RootRun,
    #[serde(rename = "subagent")]
    Subagent,
    #[serde(rename = "tool")]
    Tool,
    #[serde(rename = "provider_thread")]
    ProviderThread,
    #[serde(rename = "manual")]
    Manual,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2CheckpointScope {
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::CheckpointScopeId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId")]
    pub node_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>,
    #[serde(rename = "parentScopeId", with = "crate::wire::nullable")]
    pub parent_scope_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::CheckpointScopeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "kind")]
    pub kind: CheckpointScopeKind,
    #[serde(rename = "ordinalWithinParent")]
    pub ordinal_within_parent: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "advancesAppRunCount")]
    pub advances_app_run_count: bool,
    #[serde(rename = "cwd")]
    pub cwd: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "createdAt")]
    pub created_at: crate::wire::WireDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProviderSessionStatus {
    #[serde(rename = "starting")]
    Starting,
    #[serde(rename = "ready")]
    Ready,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "stopped")]
    Stopped,
    #[serde(rename = "error")]
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderSessionExtensionStatusesItem {
    #[serde(rename = "key")]
    pub key: String,
    #[serde(rename = "text")]
    pub text: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ProviderSession {
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderSessionId>,
    #[serde(rename = "driver")]
    pub driver: crate::provider_instance::ProviderDriverKind,
    #[serde(rename = "providerInstanceId")]
    pub provider_instance_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::provider_instance::ProviderInstanceId>,
    #[serde(rename = "status")]
    pub status: ProviderSessionStatus,
    #[serde(rename = "cwd")]
    pub cwd: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "model", with = "crate::wire::nullable")]
    pub model: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(rename = "capabilities")]
    pub capabilities: OrchestrationV2ProviderCapabilities,
    #[serde(
        rename = "extensionStatuses",
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option",
        default
    )]
    pub extension_statuses: Option<Option<Vec<ProviderSessionExtensionStatusesItem>>>,
    #[serde(rename = "createdAt")]
    pub created_at: crate::wire::WireDateTime,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "lastError", with = "crate::wire::nullable")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ProviderSessionDetached {
    #[serde(rename = "providerSessionId")]
    pub provider_session_id: crate::base_schemas::ProviderSessionId,
    #[serde(rename = "detachedAt")]
    pub detached_at: crate::wire::WireDateTime,
    #[serde(
        rename = "reason",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reason: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingBackgroundTaskSubagent {
    #[serde(
        rename = "command",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub command: Option<Option<String>>,
    #[serde(
        rename = "itemId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub item_id: Option<Option<crate::base_schemas::TurnItemId>>,
    #[serde(
        rename = "sourceThreadId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub source_thread_id: Option<Option<crate::base_schemas::ThreadId>>,
    #[serde(rename = "taskId")]
    pub task_id: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "description",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub description: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "parentTaskId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub parent_task_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "nativeItemId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub native_item_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "awaited",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub awaited: Option<Option<LiteralTrue>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "childThreadId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub child_thread_id: Option<Option<crate::base_schemas::ThreadId>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OrchestrationV2PendingBackgroundTask {
    Subagent(Box<PendingBackgroundTaskSubagent>),
    Command(Box<PendingBackgroundTaskCommand>),
    Monitor(Box<PendingBackgroundTaskCommand>),
    BackgroundTask(Box<PendingBackgroundTaskCommand>),
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "OrchestrationV2PendingBackgroundTask")]
#[serde(tag = "kind")]
enum PendingBackgroundTaskFields {
    #[serde(rename = "subagent")]
    Subagent(Box<PendingBackgroundTaskSubagent>),
    #[serde(rename = "command")]
    Command(Box<PendingBackgroundTaskCommand>),
    #[serde(rename = "monitor")]
    Monitor(Box<PendingBackgroundTaskCommand>),
    #[serde(rename = "background_task")]
    BackgroundTask(Box<PendingBackgroundTaskCommand>),
}
impl crate::wire::TaggedUnion for OrchestrationV2PendingBackgroundTask {
    const TAG: &'static str = "kind";
    const KNOWN_TAGS: &'static [&'static str] =
        &["subagent", "command", "monitor", "background_task"];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct ProviderThreadNativeMetadataItemIdentityVersion;
impl TryFrom<u64> for ProviderThreadNativeMetadataItemIdentityVersion {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 2 {
            Ok(Self)
        } else {
            Err("invalid literal")
        }
    }
}
impl From<ProviderThreadNativeMetadataItemIdentityVersion> for u64 {
    fn from(_: ProviderThreadNativeMetadataItemIdentityVersion) -> Self {
        2
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderThreadNativeMetadataClaudeAttachmentBudget {
    #[serde(rename = "bytesUsed")]
    pub bytes_used: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "pagesUsed")]
    pub pages_used: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ProviderThreadNativeMetadata {
    #[serde(
        rename = "cursorStoreCwd",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cursor_store_cwd: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "modelSelection",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub model_selection: Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        rename = "title",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub title: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "updatedAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub updated_at: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "itemIdentityVersion",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub item_identity_version: Option<Option<ProviderThreadNativeMetadataItemIdentityVersion>>,
    #[serde(
        rename = "forkHeadMessageId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub fork_head_message_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "claudeAttachmentBudget",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub claude_attachment_budget:
        Option<Option<ProviderThreadNativeMetadataClaudeAttachmentBudget>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationV2ProviderGoalStatus {
    #[serde(rename = "active")]
    Active,
    #[serde(rename = "paused")]
    Paused,
    #[serde(rename = "blocked")]
    Blocked,
    #[serde(rename = "usage_limited")]
    UsageLimited,
    #[serde(rename = "budget_limited")]
    BudgetLimited,
    #[serde(rename = "complete")]
    Complete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ProviderGoal {
    #[serde(rename = "objective")]
    pub objective: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "status")]
    pub status: OrchestrationV2ProviderGoalStatus,
    #[serde(
        rename = "tokensUsed",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tokens_used: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "tokenBudget",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub token_budget: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "timeUsedSeconds",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub time_used_seconds: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "checks",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub checks: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "lastCheck",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub last_check: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProviderThreadStatus {
    #[serde(rename = "not_loaded")]
    NotLoaded,
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "active")]
    Active,
    #[serde(rename = "archived")]
    Archived,
    #[serde(rename = "closed")]
    Closed,
    #[serde(rename = "error")]
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderThreadForkedFrom {
    #[serde(rename = "providerThreadId")]
    pub provider_thread_id: crate::base_schemas::ProviderThreadId,
    #[serde(
        rename = "providerTurnId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_turn_id: Option<Option<crate::base_schemas::ProviderTurnId>>,
    #[serde(
        rename = "checkpointId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub checkpoint_id: Option<Option<crate::base_schemas::CheckpointId>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ProviderThread {
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>,
    #[serde(rename = "driver")]
    pub driver: crate::provider_instance::ProviderDriverKind,
    #[serde(rename = "providerInstanceId")]
    pub provider_instance_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::provider_instance::ProviderInstanceId>,
    #[serde(rename = "providerSessionId", with = "crate::wire::nullable")]
    pub provider_session_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderSessionId>>,
    #[serde(rename = "appThreadId", with = "crate::wire::nullable")]
    pub app_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>>,
    #[serde(rename = "ownerNodeId", with = "crate::wire::nullable")]
    pub owner_node_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "nativeThreadRef", with = "crate::wire::nullable")]
    pub native_thread_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "nativeConversationHeadRef", with = "crate::wire::nullable")]
    pub native_conversation_head_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "status")]
    pub status: ProviderThreadStatus,
    #[serde(rename = "firstRunOrdinal", with = "crate::wire::nullable")]
    pub first_run_ordinal: Option<crate::base_schemas::PositiveInt>,
    #[serde(rename = "lastRunOrdinal", with = "crate::wire::nullable")]
    pub last_run_ordinal: Option<crate::base_schemas::PositiveInt>,
    #[serde(rename = "handoffIds")]
    pub handoff_ids:
        Vec<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ContextHandoffId>>,
    #[serde(rename = "forkedFrom", with = "crate::wire::nullable")]
    pub forked_from: Option<ProviderThreadForkedFrom>,
    #[serde(
        rename = "pendingBackgroundTasks",
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option",
        default
    )]
    pub pending_background_tasks: Option<Option<Vec<OrchestrationV2PendingBackgroundTask>>>,
    #[serde(
        rename = "contextUsage",
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional",
        default
    )]
    pub context_usage: Option<Option<crate::provider_runtime::ThreadTokenUsageSnapshot>>,
    #[serde(
        rename = "nativeMetadata",
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional",
        default
    )]
    pub native_metadata: Option<Option<OrchestrationV2ProviderThreadNativeMetadata>>,
    #[serde(
        rename = "goal",
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional",
        default
    )]
    pub goal: Option<Option<OrchestrationV2ProviderGoal>>,
    #[serde(rename = "createdAt")]
    pub created_at: crate::wire::WireDateTime,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HistoricalMessageRole {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "assistant")]
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2HistoricalMessage {
    #[serde(rename = "role")]
    pub role: HistoricalMessageRole,
    #[serde(rename = "text")]
    pub text: String,
    #[serde(
        rename = "runStatus",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_status: Option<Option<String>>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::base_schemas::RunId>,
    #[serde(rename = "itemId")]
    pub item_id: crate::base_schemas::TurnItemId,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id: Option<crate::base_schemas::ProviderThreadId>,
    #[serde(rename = "status")]
    pub status: String,
    #[serde(rename = "kind")]
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextHandoffCoveredRunOrdinals {
    #[serde(rename = "from")]
    pub r#from: crate::base_schemas::PositiveInt,
    #[serde(rename = "to")]
    pub to: crate::base_schemas::PositiveInt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ContextHandoffStrategy {
    #[serde(rename = "delta_since_target_last_seen")]
    DeltaSinceTargetLastSeen,
    #[serde(rename = "fork_delta_summary")]
    ForkDeltaSummary,
    #[serde(rename = "full_thread_summary")]
    FullThreadSummary,
    #[serde(rename = "checkpoint_summary")]
    CheckpointSummary,
    #[serde(rename = "manual_context")]
    ManualContext,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ContextHandoffStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "ready")]
    Ready,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "superseded")]
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextHandoffHistory {
    #[serde(rename = "messages")]
    pub messages: Vec<OrchestrationV2HistoricalMessage>,
    #[serde(rename = "coverage")]
    pub coverage: String,
    #[serde(rename = "omittedItems")]
    pub omitted_items: crate::base_schemas::NonNegativeInt,
    #[serde(
        rename = "omittedItemIds",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub omitted_item_ids: Option<Option<Vec<crate::base_schemas::TurnItemId>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ContextHandoffDeliveryStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "injected")]
    Injected,
    #[serde(rename = "inline")]
    Inline,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextHandoffDelivery {
    #[serde(rename = "nativeThreadId")]
    pub native_thread_id: String,
    #[serde(rename = "status")]
    pub status: ContextHandoffDeliveryStatus,
    #[serde(rename = "itemIds")]
    pub item_ids: Vec<crate::base_schemas::TurnItemId>,
    #[serde(
        rename = "omittedItemIds",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub omitted_item_ids: Option<Option<Vec<crate::base_schemas::TurnItemId>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ContextHandoff {
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ContextHandoffId>,
    #[serde(
        rename = "transferId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub transfer_id: Option<
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ContextTransferId>>,
    >,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "targetRunId")]
    pub target_run_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>,
    #[serde(rename = "fromProviderThreadIds")]
    pub from_provider_thread_ids:
        Vec<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "toProviderThreadId")]
    pub to_provider_thread_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>,
    #[serde(rename = "coveredRunOrdinals")]
    pub covered_run_ordinals: ContextHandoffCoveredRunOrdinals,
    #[serde(rename = "strategy")]
    pub strategy: ContextHandoffStrategy,
    #[serde(rename = "status")]
    pub status: ContextHandoffStatus,
    #[serde(rename = "summaryMessageId", with = "crate::wire::nullable")]
    pub summary_message_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::MessageId>>,
    #[serde(rename = "summaryText")]
    pub summary_text: String,
    #[serde(
        rename = "history",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub history: Option<Option<ContextHandoffHistory>>,
    #[serde(
        rename = "delivery",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub delivery: Option<Option<ContextHandoffDelivery>>,
    #[serde(
        rename = "detailInTurnItem",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub detail_in_turn_item: Option<Option<LiteralTrue>>,
    #[serde(rename = "createdByProviderInstanceId", with = "crate::wire::nullable")]
    pub created_by_provider_instance_id: Option<
        crate::orchestration_v2_id_table::IdOrIndex<crate::provider_instance::ProviderInstanceId>,
    >,
    #[serde(rename = "createdAt")]
    pub created_at: crate::wire::WireDateTime,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProviderTurnStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "interrupted")]
    Interrupted,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ProviderTurn {
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>,
    #[serde(rename = "providerThreadId")]
    pub provider_thread_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>,
    #[serde(rename = "nodeId")]
    pub node_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>,
    #[serde(rename = "runAttemptId", with = "crate::wire::nullable")]
    pub run_attempt_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunAttemptId>>,
    #[serde(rename = "nativeTurnRef", with = "crate::wire::nullable")]
    pub native_turn_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::PositiveInt,
    #[serde(rename = "status")]
    pub status: ProviderTurnStatus,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(
        rename = "tokenUsage",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub token_usage: Option<Option<OrchestrationV2ProviderTurnTokenUsage>>,
    #[serde(
        rename = "turnTokenUsage",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub turn_token_usage: Option<Option<crate::provider_runtime::TurnTokenUsage>>,
    #[serde(
        rename = "model",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub model: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RuntimeRequestKindMember2 {
    #[serde(rename = "dynamic_tool_call")]
    DynamicToolCall,
    #[serde(rename = "user_input")]
    UserInput,
    #[serde(rename = "auth_refresh")]
    AuthRefresh,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RuntimeRequestKind {
    ProviderRequestKind(Box<crate::provider_policy::ProviderRequestKind>),
    Member2(Box<RuntimeRequestKindMember2>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RuntimeRequestStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "resolved")]
    Resolved,
    #[serde(rename = "expired")]
    Expired,
    #[serde(rename = "cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeRequestResponseCapabilityLive {
    #[serde(rename = "providerSessionId")]
    pub provider_session_id: crate::base_schemas::ProviderSessionId,
    #[serde(
        rename = "supportsFeedback",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub supports_feedback: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeRequestResponseCapabilityNotResumable {
    #[serde(rename = "reason")]
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RuntimeRequestResponseCapability {
    #[serde(rename = "live")]
    Live(Box<RuntimeRequestResponseCapabilityLive>),
    #[serde(rename = "message")]
    Message(Box<ThreadForkSourcePointLatestStable>),
    #[serde(rename = "not_resumable")]
    NotResumable(Box<RuntimeRequestResponseCapabilityNotResumable>),
}
impl crate::wire::TaggedUnion for RuntimeRequestResponseCapability {
    const TAG: &'static str = "type";
    const KNOWN_TAGS: &'static [&'static str] = &["live", "message", "not_resumable"];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2RuntimeRequest {
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RuntimeRequestId>,
    #[serde(rename = "nodeId")]
    pub node_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeRequestRef", with = "crate::wire::nullable")]
    pub native_request_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "kind")]
    pub kind: RuntimeRequestKind,
    #[serde(rename = "status")]
    pub status: RuntimeRequestStatus,
    #[serde(rename = "responseCapability")]
    pub response_capability: RuntimeRequestResponseCapability,
    #[serde(rename = "createdAt")]
    pub created_at: crate::wire::WireDateTime,
    #[serde(rename = "resolvedAt", with = "crate::wire::nullable")]
    pub resolved_at: Option<crate::wire::WireDateTime>,
    #[serde(
        rename = "decision",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub decision: Option<Option<crate::provider_policy::ProviderApprovalDecision>>,
    #[serde(
        rename = "feedback",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub feedback: Option<Option<String>>,
    #[serde(
        rename = "answers",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub answers: Option<Option<crate::provider_policy::ProviderUserInputAnswers>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NotificationSourceMember1Kind {
    #[serde(rename = "delegated_task")]
    DelegatedTask,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationSourceMember1 {
    #[serde(rename = "kind")]
    pub kind: NotificationSourceMember1Kind,
    #[serde(rename = "taskIds")]
    pub task_ids: Vec<crate::base_schemas::NodeId>,
    #[serde(
        rename = "childThreadId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub child_thread_id: Option<Option<crate::base_schemas::ThreadId>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NotificationSourceMember2Kind {
    #[serde(rename = "background_task")]
    BackgroundTask,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NotificationSourceMember2Work {
    #[serde(rename = "subagent")]
    Subagent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationSourceMember2 {
    #[serde(rename = "kind")]
    pub kind: NotificationSourceMember2Kind,
    #[serde(rename = "work")]
    pub work: NotificationSourceMember2Work,
    #[serde(
        rename = "childThreadId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub child_thread_id: Option<Option<crate::base_schemas::ThreadId>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NotificationSourceMember3Kind {
    #[serde(rename = "background_command")]
    BackgroundCommand,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationSourceMember3 {
    #[serde(rename = "kind")]
    pub kind: NotificationSourceMember3Kind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentNotificationSource {
    #[serde(rename = "kind")]
    pub kind: NotificationSourceMember2Work,
    #[serde(
        rename = "childThreadId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub child_thread_id: Option<Option<crate::base_schemas::ThreadId>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NotificationSourceMember6Kind {
    #[serde(rename = "monitor")]
    Monitor,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationV2NotificationOutcome {
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "updated")]
    Updated,
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullRequestWakeSection {
    #[serde(rename = "text")]
    pub text: String,
    #[serde(rename = "outcome")]
    pub outcome: OrchestrationV2NotificationOutcome,
    #[serde(rename = "summary")]
    pub summary: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorPullRequestStamp {
    #[serde(rename = "host")]
    pub host: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "repository")]
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "number")]
    pub number: crate::base_schemas::PositiveInt,
    #[serde(rename = "watchStartedAt")]
    pub watch_started_at: crate::base_schemas::IsoDateTime,
    #[serde(rename = "headSha", with = "crate::wire::nullable")]
    pub head_sha: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(
        rename = "watchEnded",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub watch_ended: Option<Option<bool>>,
    #[serde(
        rename = "section",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub section: Option<Option<PullRequestWakeSection>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationSourceMember6 {
    #[serde(rename = "kind")]
    pub kind: NotificationSourceMember6Kind,
    #[serde(
        rename = "pullRequest",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub pull_request: Option<Option<MonitorPullRequestStamp>>,
    #[serde(
        rename = "otherPullRequests",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub other_pull_requests: Option<Option<Vec<MonitorPullRequestStamp>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationSourceMember7 {
    #[serde(rename = "kind")]
    pub kind: NotificationSourceMember2Kind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NotificationSourceMember8Kind {
    #[serde(rename = "system")]
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationSourceMember8 {
    #[serde(rename = "kind")]
    pub kind: NotificationSourceMember8Kind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OrchestrationV2NotificationSource {
    Member1(Box<NotificationSourceMember1>),
    Member2(Box<NotificationSourceMember2>),
    Member3(Box<NotificationSourceMember3>),
    SubagentNotificationSource(Box<SubagentNotificationSource>),
    CommandNotificationSource(Box<CommandNotificationSource>),
    Member6(Box<NotificationSourceMember6>),
    Member7(Box<NotificationSourceMember7>),
    Member8(Box<NotificationSourceMember8>),
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "OrchestrationV2NotificationSource")]
#[serde(untagged)]
enum NotificationSourceFields {
    Member1(Box<NotificationSourceMember1>),
    Member2(Box<NotificationSourceMember2>),
    Member3(Box<NotificationSourceMember3>),
    SubagentNotificationSource(Box<SubagentNotificationSource>),
    CommandNotificationSource(Box<CommandNotificationSource>),
    Member6(Box<NotificationSourceMember6>),
    Member7(Box<NotificationSourceMember7>),
    Member8(Box<NotificationSourceMember8>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2Notification {
    #[serde(rename = "source")]
    pub source: OrchestrationV2NotificationSource,
    #[serde(rename = "outcome")]
    pub outcome: OrchestrationV2NotificationOutcome,
    #[serde(rename = "summary")]
    pub summary: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "detail",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub detail: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationV2IdleCompaction {
    #[serde(rename = "beforeCacheExpires")]
    BeforeCacheExpires,
    #[serde(rename = "onReturn")]
    OnReturn,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConversationMessageRole {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "assistant")]
    Assistant,
    #[serde(rename = "system")]
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationMessageDelegatedCompletion {
    #[serde(rename = "parentRunId")]
    pub parent_run_id: crate::base_schemas::RunId,
    #[serde(rename = "generation")]
    pub generation: crate::base_schemas::PositiveInt,
    #[serde(rename = "taskIds")]
    pub task_ids: Vec<crate::base_schemas::NodeId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ConversationMessage {
    #[serde(
        rename = "notification",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub notification: Option<Option<OrchestrationV2Notification>>,
    #[serde(rename = "createdBy")]
    pub created_by: OrchestrationV2Actor,
    #[serde(rename = "creationSource")]
    pub creation_source: OrchestrationV2CreationSource,
    #[serde(
        rename = "scheduledTaskId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub scheduled_task_id: Option<
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ScheduledTaskId>>,
    >,
    #[serde(
        rename = "senderThreadId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub sender_thread_id:
        Option<Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>>>,
    #[serde(
        rename = "linkedSender",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub linked_sender: Option<Option<OrchestrationV2LinkedSender>>,
    #[serde(
        rename = "idleCompaction",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub idle_compaction: Option<Option<OrchestrationV2IdleCompaction>>,
    #[serde(
        rename = "workflowScript",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub workflow_script: Option<Option<bool>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::MessageId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "role")]
    pub role: ConversationMessageRole,
    #[serde(rename = "text")]
    pub text: String,
    #[serde(
        rename = "context",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub context: Option<Option<crate::composer_context::OrchestrationMessageContext>>,
    #[serde(rename = "attachments")]
    pub attachments: Vec<crate::chat_attachment::ChatAttachment>,
    #[serde(rename = "streaming")]
    pub streaming: bool,
    #[serde(rename = "createdAt")]
    pub created_at: crate::wire::WireDateTime,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(
        rename = "delegatedCompletion",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub delegated_completion: Option<Option<ConversationMessageDelegatedCompletion>>,
    #[serde(
        rename = "followsThreadSelection",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub follows_thread_selection: Option<Option<LiteralTrue>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlanStepStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "completed")]
    Completed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2PlanStep {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "text")]
    pub text: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "status")]
    pub status: PlanStepStatus,
    #[serde(
        rename = "durationAnchorAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub duration_anchor_at: Option<Option<crate::base_schemas::IsoDateTime>>,
    #[serde(
        rename = "durationMs",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub duration_ms: Option<Option<crate::base_schemas::NonNegativeInt>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserInputQuestionOptionsItem {
    #[serde(rename = "label")]
    pub label: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "description")]
    pub description: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "value",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub value: Option<Option<String>>,
    #[serde(
        rename = "preview",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub preview: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2UserInputQuestion {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "header")]
    pub header: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "question")]
    pub question: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "options")]
    pub options: Vec<UserInputQuestionOptionsItem>,
    #[serde(
        rename = "multiSelect",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub multi_select: Option<Option<bool>>,
    #[serde(
        rename = "allowCustomAnswer",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub allow_custom_answer: Option<Option<bool>>,
    #[serde(
        rename = "initialAnswer",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub initial_answer: Option<Option<String>>,
    #[serde(
        rename = "required",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub required: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlanArtifactProposedPlanStatus {
    #[serde(rename = "draft")]
    Draft,
    #[serde(rename = "active")]
    Active,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "superseded")]
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanArtifactProposedPlan {
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::PlanId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId")]
    pub node_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>,
    #[serde(rename = "status")]
    pub status: PlanArtifactProposedPlanStatus,
    #[serde(
        rename = "detailInTurnItem",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub detail_in_turn_item: Option<Option<LiteralTrue>>,
    #[serde(rename = "markdown")]
    pub markdown: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanArtifactTodoList {
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::PlanId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId")]
    pub node_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>,
    #[serde(rename = "status")]
    pub status: PlanArtifactProposedPlanStatus,
    #[serde(
        rename = "detailInTurnItem",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub detail_in_turn_item: Option<Option<LiteralTrue>>,
    #[serde(rename = "steps")]
    pub steps: Vec<OrchestrationV2PlanStep>,
    #[serde(
        rename = "explanation",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub explanation: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum OrchestrationV2PlanArtifact {
    #[serde(rename = "proposed_plan")]
    ProposedPlan(Box<PlanArtifactProposedPlan>),
    #[serde(rename = "todo_list")]
    TodoList(Box<PlanArtifactTodoList>),
}
impl crate::wire::TaggedUnion for OrchestrationV2PlanArtifact {
    const TAG: &'static str = "kind";
    const KNOWN_TAGS: &'static [&'static str] = &["proposed_plan", "todo_list"];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2CheckpointFileSummary {
    #[serde(rename = "path")]
    pub path: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "kind")]
    pub kind: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "additions")]
    pub additions: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "deletions")]
    pub deletions: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2CheckpointFileTotals {
    #[serde(rename = "count")]
    pub count: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "additions")]
    pub additions: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "deletions")]
    pub deletions: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CheckpointStatus {
    #[serde(rename = "ready")]
    Ready,
    #[serde(rename = "missing")]
    Missing,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "stale")]
    Stale,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2Checkpoint {
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::CheckpointId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "scopeId")]
    pub scope_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::CheckpointScopeId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId")]
    pub node_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>,
    #[serde(rename = "parentCheckpointId", with = "crate::wire::nullable")]
    pub parent_checkpoint_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::CheckpointId>>,
    #[serde(rename = "ordinalWithinScope")]
    pub ordinal_within_scope: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "appRunOrdinal", with = "crate::wire::nullable")]
    pub app_run_ordinal: Option<crate::base_schemas::PositiveInt>,
    #[serde(rename = "ref")]
    pub r#ref: crate::base_schemas::CheckpointRef,
    #[serde(rename = "status")]
    pub status: CheckpointStatus,
    #[serde(rename = "files")]
    pub files: Vec<OrchestrationV2CheckpointFileSummary>,
    #[serde(
        rename = "filesTotal",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub files_total: Option<OrchestrationV2CheckpointFileTotals>,
    #[serde(rename = "capturedAt")]
    pub captured_at: crate::wire::WireDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2CheckpointRollbackRequest {
    #[serde(rename = "scopeId")]
    pub scope_id: crate::base_schemas::CheckpointScopeId,
    #[serde(rename = "checkpointId")]
    pub checkpoint_id: crate::base_schemas::CheckpointId,
    #[serde(rename = "requestedAt")]
    pub requested_at: crate::wire::WireDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CheckpointUnavailableErrorTag {
    #[serde(rename = "OrchestrationV2CheckpointUnavailableError")]
    OrchestrationV2CheckpointUnavailableError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2CheckpointUnavailableError {
    #[serde(rename = "_tag")]
    pub _tag: CheckpointUnavailableErrorTag,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "target")]
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2FileChangeDetail {
    #[serde(rename = "operation")]
    pub operation: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "path")]
    pub path: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "oldPath",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub old_path: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "fileType",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub file_type: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "mimeType",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub mime_type: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationV2ProviderFailureClass {
    #[serde(rename = "usage_limit")]
    UsageLimit,
    #[serde(rename = "provider_error")]
    ProviderError,
    #[serde(rename = "transport_error")]
    TransportError,
    #[serde(rename = "permission_error")]
    PermissionError,
    #[serde(rename = "validation_error")]
    ValidationError,
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ProviderFailure {
    #[serde(rename = "class")]
    pub class: OrchestrationV2ProviderFailureClass,
    #[serde(rename = "message")]
    pub message: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "code", with = "crate::wire::nullable")]
    pub code: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(rename = "retryable", with = "crate::wire::nullable")]
    pub retryable: Option<bool>,
    #[serde(
        rename = "resetAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub reset_at: Option<Option<crate::base_schemas::IsoDateTime>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ProviderRetry {
    #[serde(rename = "attempt")]
    pub attempt: crate::base_schemas::PositiveInt,
    #[serde(rename = "maxAttempts", with = "crate::wire::nullable")]
    pub max_attempts: Option<crate::base_schemas::PositiveInt>,
    #[serde(rename = "retryDelayMs", with = "crate::wire::nullable")]
    pub retry_delay_ms: Option<crate::base_schemas::NonNegativeInt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationV2UserMessageInputIntent {
    #[serde(rename = "turn_start")]
    TurnStart,
    #[serde(rename = "queued_turn")]
    QueuedTurn,
    #[serde(rename = "steer")]
    Steer,
    #[serde(rename = "promoted_queued_to_steer")]
    PromotedQueuedToSteer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ModelSwitch {
    #[serde(rename = "from")]
    pub r#from: crate::model_selection::ModelSelection,
    #[serde(rename = "to")]
    pub to: crate::model_selection::ModelSelection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2FileSearchResult {
    #[serde(rename = "fileName")]
    pub file_name: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "line",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub line: Option<Option<crate::base_schemas::PositiveInt>>,
    #[serde(
        rename = "column",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub column: Option<Option<crate::base_schemas::PositiveInt>>,
    #[serde(
        rename = "preview",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub preview: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2WebSearchResult {
    #[serde(
        rename = "title",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<String>>,
    #[serde(
        rename = "url",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub url: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "snippet",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub snippet: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationV2SecretRequestStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "saved")]
    Saved,
    #[serde(rename = "declined")]
    Declined,
    #[serde(rename = "cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationV2SecretRequestDelivery {
    #[serde(rename = "file")]
    File,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationV2LinkRequestStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "linked")]
    Linked,
    #[serde(rename = "declined")]
    Declined,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "failed")]
    Failed,
}

impl<'de> Deserialize<'de> for OrchestrationV2PendingBackgroundTask {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use crate::wire::TaggedUnion;
        let mut value = serde_json::Value::deserialize(deserializer)?;
        let fields = value
            .as_object_mut()
            .ok_or_else(|| serde::de::Error::custom("expected a background task"))?;
        match fields.get("kind") {
            Some(serde_json::Value::String(kind)) if Self::KNOWN_TAGS.contains(&kind.as_str()) => {}
            None | Some(serde_json::Value::Null) | Some(serde_json::Value::String(_)) => {
                fields.retain(|key, _| {
                    ["taskId", "description", "parentTaskId"].contains(&key.as_str())
                });
                fields.insert("kind".into(), "background_task".into());
            }
            _ => return Err(serde::de::Error::custom("expected a string task kind")),
        }
        PendingBackgroundTaskFields::deserialize(value).map_err(serde::de::Error::custom)
    }
}
impl Serialize for OrchestrationV2PendingBackgroundTask {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        PendingBackgroundTaskFields::serialize(self, serializer)
    }
}
impl<'de> Deserialize<'de> for OrchestrationV2NotificationSource {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = serde_json::Value::deserialize(deserializer)?;
        let fields = value
            .as_object_mut()
            .ok_or_else(|| serde::de::Error::custom("expected a notification source"))?;
        match fields.get("kind").and_then(serde_json::Value::as_str) {
            Some("command") => {
                fields.insert("kind".into(), "background_command".into());
            }
            Some("subagent") => {
                fields.insert("kind".into(), "background_task".into());
                fields.insert("work".into(), "subagent".into());
            }
            Some(
                "delegated_task" | "background_task" | "background_command" | "monitor" | "system",
            ) => {}
            _ if fields
                .get("kind")
                .is_none_or(|kind| kind.is_null() || kind.is_string()) =>
            {
                fields.clear();
                fields.insert("kind".into(), "background_task".into());
            }
            _ => {
                return Err(serde::de::Error::custom(
                    "expected a string notification kind",
                ));
            }
        }
        NotificationSourceFields::deserialize(value).map_err(serde::de::Error::custom)
    }
}
impl Serialize for OrchestrationV2NotificationSource {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::CommandNotificationSource(_) => NotificationSourceMember3 {
                kind: NotificationSourceMember3Kind::BackgroundCommand,
            }
            .serialize(serializer),
            Self::SubagentNotificationSource(source) => NotificationSourceMember2 {
                kind: NotificationSourceMember2Kind::BackgroundTask,
                work: NotificationSourceMember2Work::Subagent,
                child_thread_id: source.child_thread_id.clone(),
            }
            .serialize(serializer),
            _ => NotificationSourceFields::serialize(self, serializer),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationV2ThreadVisit {
    #[serde(
        default = "app_thread_last_visited_at_default",
        deserialize_with = "crate::wire::utc_datetime::deserialize_nullable"
    )]
    pub last_visited_at: Option<crate::wire::WireDateTime>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_lenient_opens"
    )]
    pub opens: Option<Option<OrchestrationV2ThreadOpens>>,
}
pub type OrchestrationV2ThreadVisitJson = OrchestrationV2ThreadVisit;

fn deserialize_open_score<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<crate::wire::JsNumber, D::Error> {
    let score = crate::wire::JsNumber::deserialize(d)?;
    if score.value() < 0.0 {
        Err(serde::de::Error::custom(
            "thread open score must be non-negative",
        ))
    } else {
        Ok(score)
    }
}
