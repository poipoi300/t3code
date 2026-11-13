use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2DeferredWork {
    #[serde(rename = "cursor")]
    pub cursor: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "itemCount")]
    pub item_count: crate::base_schemas::PositiveInt,
    #[serde(rename = "lastOrdinal")]
    pub last_ordinal: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemNotification {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
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
pub struct TurnItemUserMessage {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "createdBy")]
    pub created_by: OrchestrationV2Actor,
    #[serde(rename = "creationSource")]
    pub creation_source: OrchestrationV2CreationSource,
    #[serde(rename = "messageId")]
    pub message_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::MessageId>,
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
    #[serde(
        rename = "freshContext",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub fresh_context: Option<Option<bool>>,
    #[serde(rename = "inputIntent")]
    pub input_intent: OrchestrationV2UserMessageInputIntent,
    #[serde(rename = "text")]
    pub text: String,
    #[serde(
        rename = "textOmitted",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub text_omitted: Option<LiteralTrue>,
    #[serde(
        rename = "context",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub context: Option<Option<crate::composer_context::OrchestrationMessageContext>>,
    #[serde(rename = "attachments")]
    pub attachments: Vec<crate::chat_attachment::ChatAttachment>,
    #[serde(
        rename = "modelSwitch",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub model_switch: Option<Option<OrchestrationV2ModelSwitch>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AssistantMessagePhase {
    #[serde(rename = "commentary")]
    Commentary,
    #[serde(rename = "partial_answer")]
    PartialAnswer,
    #[serde(rename = "final_answer")]
    FinalAnswer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemAssistantMessage {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(
        rename = "phase",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub phase: Option<Option<AssistantMessagePhase>>,
    #[serde(rename = "messageId")]
    pub message_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::MessageId>,
    #[serde(rename = "text")]
    pub text: String,
    #[serde(
        rename = "textOmitted",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub text_omitted: Option<LiteralTrue>,
    #[serde(
        rename = "attachments",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub attachments: Option<Option<Vec<crate::chat_attachment::ChatAttachment>>>,
    #[serde(rename = "streaming")]
    pub streaming: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemReasoning {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "text")]
    pub text: String,
    #[serde(rename = "streaming")]
    pub streaming: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemProposedPlan {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "planId")]
    pub plan_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::PlanId>,
    #[serde(rename = "markdown")]
    pub markdown: String,
    #[serde(rename = "streaming")]
    pub streaming: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemTodoList {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "planId")]
    pub plan_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::PlanId>,
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
pub enum TurnItemUserInputRequestResponseMode {
    #[serde(rename = "message")]
    Message,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemUserInputRequest {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "requestId")]
    pub request_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RuntimeRequestId>,
    #[serde(rename = "questions")]
    pub questions: Vec<OrchestrationV2UserInputQuestion>,
    #[serde(
        rename = "questionAnswer",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub question_answer: Option<Option<crate::provider_policy::UserInputAttachmentAnswerPayload>>,
    #[serde(
        rename = "responseMode",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub response_mode: Option<Option<TurnItemUserInputRequestResponseMode>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemFileChange {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "fileName")]
    pub file_name: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "additions",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub additions: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "deletions",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub deletions: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "diffStr",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub diff_str: Option<Option<String>>,
    #[serde(
        rename = "oldStr",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub old_str: Option<Option<String>>,
    #[serde(
        rename = "newStr",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub new_str: Option<Option<String>>,
    #[serde(
        rename = "changes",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub changes: Option<Option<Vec<OrchestrationV2FileChangeDetail>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemFileSearch {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(
        rename = "pattern",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub pattern: Option<Option<String>>,
    #[serde(
        rename = "results",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub results: Option<Option<Vec<OrchestrationV2FileSearchResult>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemWebSearch {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(
        rename = "patterns",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub patterns: Option<Option<Vec<String>>>,
    #[serde(
        rename = "results",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub results: Option<Option<Vec<OrchestrationV2WebSearchResult>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemApprovalRequest {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "requestId")]
    pub request_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RuntimeRequestId>,
    #[serde(rename = "requestKind")]
    pub request_kind: crate::provider_policy::ProviderRequestKind,
    #[serde(
        rename = "prompt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub prompt: Option<Option<String>>,
    #[serde(
        rename = "reason",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reason: Option<Option<String>>,
    #[serde(
        rename = "fullAccessOverride",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub full_access_override:
        Option<Option<crate::provider_policy::ProviderApprovalFullAccessOverride>>,
    #[serde(
        rename = "appName",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub app_name: Option<Option<String>>,
    #[serde(
        rename = "options",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub options: Option<Option<Vec<crate::provider_policy::ProviderApprovalOption>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemCheckpoint {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "checkpointId")]
    pub checkpoint_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::CheckpointId>,
    #[serde(rename = "scopeId")]
    pub scope_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::CheckpointScopeId>,
    #[serde(rename = "files")]
    pub files: Vec<OrchestrationV2CheckpointFileSummary>,
    #[serde(
        rename = "filesTotal",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub files_total: Option<OrchestrationV2CheckpointFileTotals>,
    #[serde(
        rename = "filesOmitted",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub files_omitted: Option<LiteralTrue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemRunInterruptRequest {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "message")]
    pub message: String,
    #[serde(
        rename = "unresponsiveAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub unresponsive_at: Option<crate::wire::WireDateTime>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemRunInterruptResult {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "message")]
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemError {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "failure")]
    pub failure: OrchestrationV2ProviderFailure,
    #[serde(
        rename = "retry",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub retry: Option<Option<OrchestrationV2ProviderRetry>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemCompaction {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "driver", with = "crate::wire::nullable")]
    pub driver: Option<crate::provider_instance::ProviderDriverKind>,
    #[serde(
        rename = "summary",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub summary: Option<Option<String>>,
    #[serde(
        rename = "beforeTokenCount",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub before_token_count: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        rename = "afterTokenCount",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub after_token_count: Option<Option<crate::base_schemas::NonNegativeInt>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemHandoff {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "contextHandoffId")]
    pub context_handoff_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ContextHandoffId>,
    #[serde(rename = "fromProviderThreadIds")]
    pub from_provider_thread_ids:
        Vec<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "toProviderThreadId")]
    pub to_provider_thread_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>,
    #[serde(rename = "fromProviderInstanceIds")]
    pub from_provider_instance_ids: Vec<
        crate::orchestration_v2_id_table::IdOrIndex<crate::provider_instance::ProviderInstanceId>,
    >,
    #[serde(rename = "toProviderInstanceId")]
    pub to_provider_instance_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::provider_instance::ProviderInstanceId>,
    #[serde(
        rename = "fromModelSelections",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub from_model_selections: Option<Option<Vec<crate::model_selection::ModelSelection>>>,
    #[serde(
        rename = "toModel",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub to_model: Option<Option<String>>,
    #[serde(rename = "strategy")]
    pub strategy: ContextHandoffStrategy,
    #[serde(
        rename = "summary",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub summary: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemFork {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "providerThreadId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_thread_id: Option<
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    >,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "source")]
    pub source: AppThreadForkedFrom,
    #[serde(rename = "targetThreadId")]
    pub target_thread_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemThreadCreated {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "targetThreadId")]
    pub target_thread_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "targetRunId", with = "crate::wire::nullable")]
    pub target_run_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "targetProviderInstanceId")]
    pub target_provider_instance_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::provider_instance::ProviderInstanceId>,
    #[serde(rename = "targetModel")]
    pub target_model: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemSecretRequest {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "label")]
    pub label: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "reason")]
    pub reason: String,
    #[serde(
        rename = "url",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub url: Option<Option<String>>,
    #[serde(
        rename = "placeholder",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub placeholder: Option<Option<String>>,
    #[serde(
        rename = "delivery",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub delivery: Option<Option<OrchestrationV2SecretRequestDelivery>>,
    #[serde(rename = "secretStatus")]
    pub secret_status: OrchestrationV2SecretRequestStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemLinkRequest {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(
        rename = "url",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub url: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(rename = "reason")]
    pub reason: String,
    #[serde(
        rename = "environmentId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub environment_id: Option<
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::EnvironmentId>>,
    >,
    #[serde(
        rename = "label",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub label: Option<Option<String>>,
    #[serde(
        rename = "hint",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub hint: Option<Option<String>>,
    #[serde(
        rename = "requestedAccess",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub requested_access: Option<Option<crate::auth::AuthMcpClientAccess>>,
    #[serde(rename = "linkStatus")]
    pub link_status: OrchestrationV2LinkRequestStatus,
    #[serde(
        rename = "linkedEnvironmentId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub linked_environment_id: Option<
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::EnvironmentId>>,
    >,
    #[serde(
        rename = "linkedLabel",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub linked_label: Option<Option<String>>,
    #[serde(
        rename = "linkedAccess",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub linked_access: Option<Option<crate::auth::AuthMcpClientAccess>>,
    #[serde(
        rename = "failure",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub failure: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemSubagent {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "subagentId")]
    pub subagent_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>,
    #[serde(rename = "origin")]
    pub origin: SubagentOrigin,
    #[serde(rename = "driver")]
    pub driver: crate::provider_instance::ProviderDriverKind,
    #[serde(rename = "providerInstanceId")]
    pub provider_instance_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::provider_instance::ProviderInstanceId>,
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
    #[serde(rename = "prompt")]
    pub prompt: String,
    #[serde(
        rename = "progress",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub progress: Option<Option<String>>,
    #[serde(rename = "result", with = "crate::wire::nullable")]
    pub result: Option<String>,
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
pub struct TurnItemDynamicTool {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "toolName", with = "crate::wire::nullable")]
    pub tool_name: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(
        rename = "viewedImagePath",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub viewed_image_path: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(rename = "input")]
    pub input: serde_json::Value,
    #[serde(
        rename = "output",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub output: Option<Option<serde_json::Value>>,
    #[serde(
        rename = "outputOmitted",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub output_omitted: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OrchestrationV2TurnItem {
    #[serde(rename = "notification")]
    Notification(Box<TurnItemNotification>),
    #[serde(rename = "user_message")]
    UserMessage(Box<TurnItemUserMessage>),
    #[serde(rename = "assistant_message")]
    AssistantMessage(Box<TurnItemAssistantMessage>),
    #[serde(rename = "reasoning")]
    Reasoning(Box<TurnItemReasoning>),
    #[serde(rename = "proposed_plan")]
    ProposedPlan(Box<TurnItemProposedPlan>),
    #[serde(rename = "todo_list")]
    TodoList(Box<TurnItemTodoList>),
    #[serde(rename = "user_input_request")]
    UserInputRequest(Box<TurnItemUserInputRequest>),
    #[serde(rename = "file_change")]
    FileChange(Box<TurnItemFileChange>),
    #[serde(rename = "command_execution")]
    CommandExecution(Box<TurnItemCommandExecution>),
    #[serde(rename = "file_search")]
    FileSearch(Box<TurnItemFileSearch>),
    #[serde(rename = "web_search")]
    WebSearch(Box<TurnItemWebSearch>),
    #[serde(rename = "approval_request")]
    ApprovalRequest(Box<TurnItemApprovalRequest>),
    #[serde(rename = "checkpoint")]
    Checkpoint(Box<TurnItemCheckpoint>),
    #[serde(rename = "run_interrupt_request")]
    RunInterruptRequest(Box<TurnItemRunInterruptRequest>),
    #[serde(rename = "run_interrupt_result")]
    RunInterruptResult(Box<TurnItemRunInterruptResult>),
    #[serde(rename = "system_notice")]
    SystemNotice(Box<TurnItemRunInterruptResult>),
    #[serde(rename = "error")]
    Error(Box<TurnItemError>),
    #[serde(rename = "compaction")]
    Compaction(Box<TurnItemCompaction>),
    #[serde(rename = "handoff")]
    Handoff(Box<TurnItemHandoff>),
    #[serde(rename = "fork")]
    Fork(Box<TurnItemFork>),
    #[serde(rename = "thread_created")]
    ThreadCreated(Box<TurnItemThreadCreated>),
    #[serde(rename = "secret_request")]
    SecretRequest(Box<TurnItemSecretRequest>),
    #[serde(rename = "link_request")]
    LinkRequest(Box<TurnItemLinkRequest>),
    #[serde(rename = "subagent")]
    Subagent(Box<TurnItemSubagent>),
    #[serde(rename = "dynamic_tool")]
    DynamicTool(Box<TurnItemDynamicTool>),
}
impl crate::wire::TaggedUnion for OrchestrationV2TurnItem {
    const TAG: &'static str = "type";
    const KNOWN_TAGS: &'static [&'static str] = &[
        "notification",
        "user_message",
        "assistant_message",
        "reasoning",
        "proposed_plan",
        "todo_list",
        "user_input_request",
        "file_change",
        "command_execution",
        "file_search",
        "web_search",
        "approval_request",
        "checkpoint",
        "run_interrupt_request",
        "run_interrupt_result",
        "system_notice",
        "error",
        "compaction",
        "handoff",
        "fork",
        "thread_created",
        "secret_request",
        "link_request",
        "subagent",
        "dynamic_tool",
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProjectedTurnItemVisibility {
    #[serde(rename = "local")]
    Local,
    #[serde(rename = "inherited")]
    Inherited,
    #[serde(rename = "synthetic")]
    Synthetic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ProjectedTurnItem {
    #[serde(rename = "position")]
    pub position: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "visibility")]
    pub visibility: ProjectedTurnItemVisibility,
    #[serde(rename = "sourceThreadId")]
    pub source_thread_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "sourceItemId")]
    pub source_item_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "item")]
    pub item: OrchestrationV2TurnItem,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2PendingRuntimeRequestSummary {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::RuntimeRequestId,
    #[serde(rename = "kind")]
    pub kind: RuntimeRequestKind,
    #[serde(rename = "createdAt")]
    pub created_at: crate::wire::WireDateTime,
    #[serde(
        rename = "blocking",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub blocking: Option<Option<bool>>,
    #[serde(
        rename = "userOnly",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub user_only: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2LatestVisibleMessageSummary {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::MessageId,
    #[serde(rename = "role")]
    pub role: ConversationMessageRole,
    #[serde(rename = "text")]
    pub text: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2OlderSettledCount {
    #[serde(rename = "projectId")]
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(rename = "count")]
    pub count: crate::base_schemas::NonNegativeInt,
}

pub type OrchestrationV2AppThreadJson = OrchestrationV2AppThread;

pub type OrchestrationV2RunJson = OrchestrationV2Run;

pub type OrchestrationV2RunAttemptJson = OrchestrationV2RunAttempt;

pub type OrchestrationV2ExecutionNodeJson = OrchestrationV2ExecutionNode;

pub type OrchestrationV2SubagentJson = OrchestrationV2Subagent;

pub type OrchestrationV2CheckpointScopeJson = OrchestrationV2CheckpointScope;

pub type OrchestrationV2ProviderSessionJson = OrchestrationV2ProviderSession;

pub type OrchestrationV2ProviderSessionDetachedJson = OrchestrationV2ProviderSessionDetached;

pub type OrchestrationV2ProviderThreadJson = OrchestrationV2ProviderThread;

pub type OrchestrationV2ContextHandoffJson = OrchestrationV2ContextHandoff;

pub type OrchestrationV2ContextTransferJson = OrchestrationV2ContextTransfer;

pub type OrchestrationV2ProviderTurnJson = OrchestrationV2ProviderTurn;

pub type OrchestrationV2RuntimeRequestJson = OrchestrationV2RuntimeRequest;

pub type OrchestrationV2ConversationMessageJson = OrchestrationV2ConversationMessage;

pub type OrchestrationV2CheckpointJson = OrchestrationV2Checkpoint;

pub type OrchestrationV2CheckpointRollbackRequestJson = OrchestrationV2CheckpointRollbackRequest;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemJsonUserMessage {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "createdBy")]
    pub created_by: OrchestrationV2Actor,
    #[serde(rename = "creationSource")]
    pub creation_source: OrchestrationV2CreationSource,
    #[serde(rename = "messageId")]
    pub message_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::MessageId>,
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
    #[serde(
        rename = "freshContext",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub fresh_context: Option<Option<bool>>,
    #[serde(rename = "inputIntent")]
    pub input_intent: OrchestrationV2UserMessageInputIntent,
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
    #[serde(
        rename = "modelSwitch",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub model_switch: Option<Option<OrchestrationV2ModelSwitch>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemJsonAssistantMessage {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(
        rename = "phase",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub phase: Option<Option<AssistantMessagePhase>>,
    #[serde(rename = "messageId")]
    pub message_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::MessageId>,
    #[serde(rename = "text")]
    pub text: String,
    #[serde(
        rename = "attachments",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub attachments: Option<Option<Vec<crate::chat_attachment::ChatAttachment>>>,
    #[serde(rename = "streaming")]
    pub streaming: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemJsonCheckpoint {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "checkpointId")]
    pub checkpoint_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::CheckpointId>,
    #[serde(rename = "scopeId")]
    pub scope_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::CheckpointScopeId>,
    #[serde(rename = "files")]
    pub files: Vec<OrchestrationV2CheckpointFileSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnItemJsonSubagent {
    #[serde(
        rename = "deferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub deferred_work: Option<OrchestrationV2DeferredWork>,
    #[serde(
        rename = "toolNonExecutionKind",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_non_execution_kind: Option<Option<String>>,
    #[serde(
        rename = "toolSurface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_surface: Option<Option<crate::provider_runtime::ToolActivitySurface>>,
    #[serde(
        rename = "toolIcon",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_icon: Option<Option<crate::provider_runtime::ToolActivityIcon>>,
    #[serde(
        rename = "toolSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_source: Option<Option<crate::provider_runtime::ToolActivitySource>>,
    #[serde(rename = "id")]
    pub id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "threadId")]
    pub thread_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::RunId>>,
    #[serde(rename = "nodeId", with = "crate::wire::nullable")]
    pub node_id: Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>>,
    #[serde(rename = "providerThreadId", with = "crate::wire::nullable")]
    pub provider_thread_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderThreadId>>,
    #[serde(rename = "providerTurnId", with = "crate::wire::nullable")]
    pub provider_turn_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ProviderTurnId>>,
    #[serde(rename = "nativeItemRef", with = "crate::wire::nullable")]
    pub native_item_ref: Option<OrchestrationV2ProviderRef>,
    #[serde(rename = "parentItemId", with = "crate::wire::nullable")]
    pub parent_item_id:
        Option<crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>>,
    #[serde(rename = "ordinal")]
    pub ordinal: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "status")]
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(rename = "title", with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "completedAt", with = "crate::wire::nullable")]
    pub completed_at: Option<crate::wire::WireDateTime>,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::wire::WireDateTime,
    #[serde(rename = "subagentId")]
    pub subagent_id: crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::NodeId>,
    #[serde(rename = "origin")]
    pub origin: SubagentOrigin,
    #[serde(rename = "driver")]
    pub driver: crate::provider_instance::ProviderDriverKind,
    #[serde(rename = "providerInstanceId")]
    pub provider_instance_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::provider_instance::ProviderInstanceId>,
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
    #[serde(rename = "prompt")]
    pub prompt: String,
    #[serde(
        rename = "progress",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub progress: Option<Option<String>>,
    #[serde(rename = "result", with = "crate::wire::nullable")]
    pub result: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OrchestrationV2TurnItemJson {
    #[serde(rename = "notification")]
    Notification(Box<TurnItemNotification>),
    #[serde(rename = "user_message")]
    UserMessage(Box<TurnItemJsonUserMessage>),
    #[serde(rename = "assistant_message")]
    AssistantMessage(Box<TurnItemJsonAssistantMessage>),
    #[serde(rename = "reasoning")]
    Reasoning(Box<TurnItemReasoning>),
    #[serde(rename = "proposed_plan")]
    ProposedPlan(Box<TurnItemProposedPlan>),
    #[serde(rename = "todo_list")]
    TodoList(Box<TurnItemTodoList>),
    #[serde(rename = "user_input_request")]
    UserInputRequest(Box<TurnItemUserInputRequest>),
    #[serde(rename = "file_change")]
    FileChange(Box<TurnItemFileChange>),
    #[serde(rename = "command_execution")]
    CommandExecution(Box<TurnItemCommandExecution>),
    #[serde(rename = "file_search")]
    FileSearch(Box<TurnItemFileSearch>),
    #[serde(rename = "web_search")]
    WebSearch(Box<TurnItemWebSearch>),
    #[serde(rename = "approval_request")]
    ApprovalRequest(Box<TurnItemApprovalRequest>),
    #[serde(rename = "checkpoint")]
    Checkpoint(Box<TurnItemJsonCheckpoint>),
    #[serde(rename = "run_interrupt_request")]
    RunInterruptRequest(Box<TurnItemRunInterruptRequest>),
    #[serde(rename = "run_interrupt_result")]
    RunInterruptResult(Box<TurnItemRunInterruptResult>),
    #[serde(rename = "system_notice")]
    SystemNotice(Box<TurnItemRunInterruptResult>),
    #[serde(rename = "error")]
    Error(Box<TurnItemError>),
    #[serde(rename = "compaction")]
    Compaction(Box<TurnItemCompaction>),
    #[serde(rename = "handoff")]
    Handoff(Box<TurnItemHandoff>),
    #[serde(rename = "fork")]
    Fork(Box<TurnItemFork>),
    #[serde(rename = "thread_created")]
    ThreadCreated(Box<TurnItemThreadCreated>),
    #[serde(rename = "secret_request")]
    SecretRequest(Box<TurnItemSecretRequest>),
    #[serde(rename = "link_request")]
    LinkRequest(Box<TurnItemLinkRequest>),
    #[serde(rename = "subagent")]
    Subagent(Box<TurnItemJsonSubagent>),
    #[serde(rename = "dynamic_tool")]
    DynamicTool(Box<TurnItemDynamicTool>),
}
impl crate::wire::TaggedUnion for OrchestrationV2TurnItemJson {
    const TAG: &'static str = "type";
    const KNOWN_TAGS: &'static [&'static str] = &[
        "notification",
        "user_message",
        "assistant_message",
        "reasoning",
        "proposed_plan",
        "todo_list",
        "user_input_request",
        "file_change",
        "command_execution",
        "file_search",
        "web_search",
        "approval_request",
        "checkpoint",
        "run_interrupt_request",
        "run_interrupt_result",
        "system_notice",
        "error",
        "compaction",
        "handoff",
        "fork",
        "thread_created",
        "secret_request",
        "link_request",
        "subagent",
        "dynamic_tool",
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ProjectedTurnItemJson {
    #[serde(rename = "position")]
    pub position: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "visibility")]
    pub visibility: ProjectedTurnItemVisibility,
    #[serde(rename = "sourceThreadId")]
    pub source_thread_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::ThreadId>,
    #[serde(rename = "sourceItemId")]
    pub source_item_id:
        crate::orchestration_v2_id_table::IdOrIndex<crate::base_schemas::TurnItemId>,
    #[serde(rename = "item")]
    pub item: OrchestrationV2TurnItemJson,
}

pub type OrchestrationV2PendingRuntimeRequestSummaryJson =
    OrchestrationV2PendingRuntimeRequestSummary;

pub type OrchestrationV2LatestVisibleMessageSummaryJson =
    OrchestrationV2LatestVisibleMessageSummary;
