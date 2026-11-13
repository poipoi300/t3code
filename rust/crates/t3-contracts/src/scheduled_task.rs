use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScheduledTaskWebhookSignatureEncoding {
    #[serde(rename = "hex")]
    Hex,
    #[serde(rename = "base64")]
    Base64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskWebhookSignature {
    #[serde(rename = "header")]
    pub header: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "encoding")]
    pub encoding: ScheduledTaskWebhookSignatureEncoding,
    #[serde(rename = "prefix")]
    pub prefix: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskWebhookFilterEquals {
    #[serde(rename = "path")]
    pub path: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "value")]
    pub value: String,
}

pub type WebhookFilterListValue = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskWebhookFilterIn {
    #[serde(rename = "path")]
    pub path: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "value")]
    pub value: Vec<WebhookFilterListValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskWebhookFilterExists {
    #[serde(rename = "path")]
    pub path: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum ScheduledTaskWebhookFilter {
    #[serde(rename = "equals")]
    Equals(Box<ScheduledTaskWebhookFilterEquals>),
    #[serde(rename = "notEquals")]
    NotEquals(Box<ScheduledTaskWebhookFilterEquals>),
    #[serde(rename = "startsWith")]
    StartsWith(Box<ScheduledTaskWebhookFilterEquals>),
    #[serde(rename = "in")]
    In(Box<ScheduledTaskWebhookFilterIn>),
    #[serde(rename = "exists")]
    Exists(Box<ScheduledTaskWebhookFilterExists>),
}
impl crate::wire::TaggedUnion for ScheduledTaskWebhookFilter {
    const TAG: &'static str = "op";
    const KNOWN_TAGS: &'static [&'static str] =
        &["equals", "notEquals", "startsWith", "in", "exists"];
}

pub type ScheduledTaskIntervalMs = i64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskScheduleInterval {
    #[serde(rename = "everyMs")]
    pub every_ms: ScheduledTaskIntervalMs,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskScheduleFixedTime {
    #[serde(rename = "timeOfDay")]
    pub time_of_day: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "weekdays",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub weekdays: Option<Option<Vec<i64>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskScheduleOnce {
    #[serde(rename = "runAt")]
    pub run_at: crate::base_schemas::TrimmedNonEmptyString,
}

pub type WebhookMaxDeliveryAgeMinutes = i64;

pub type WebhookDedupeTemplate = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskWebhookDedupe {
    #[serde(
        rename = "replayKey",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub replay_key: Option<Option<WebhookDedupeTemplate>>,
    #[serde(
        rename = "coalesceKey",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub coalesce_key: Option<Option<WebhookDedupeTemplate>>,
    #[serde(
        rename = "coalesceWindowSeconds",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub coalesce_window_seconds: Option<Option<i64>>,
}

pub type ScheduledTaskWebhookFilters = Vec<ScheduledTaskWebhookFilter>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskScheduleWebhook {
    #[serde(rename = "signature", with = "crate::wire::nullable")]
    pub signature: Option<ScheduledTaskWebhookSignature>,
    #[serde(
        rename = "maxDeliveryAgeMinutes",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub max_delivery_age_minutes: Option<Option<WebhookMaxDeliveryAgeMinutes>>,
    #[serde(
        rename = "dedupe",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub dedupe: Option<Option<ScheduledTaskWebhookDedupe>>,
    #[serde(
        rename = "filters",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub filters: Option<Option<ScheduledTaskWebhookFilters>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ScheduledTaskSchedule {
    #[serde(rename = "interval")]
    Interval(Box<ScheduledTaskScheduleInterval>),
    #[serde(rename = "fixed_time")]
    FixedTime(Box<ScheduledTaskScheduleFixedTime>),
    #[serde(rename = "once")]
    Once(Box<ScheduledTaskScheduleOnce>),
    #[serde(rename = "webhook")]
    Webhook(Box<ScheduledTaskScheduleWebhook>),
}
impl crate::wire::TaggedUnion for ScheduledTaskSchedule {
    const TAG: &'static str = "type";
    const KNOWN_TAGS: &'static [&'static str] = &["interval", "fixed_time", "once", "webhook"];
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskUpsertScheduleWebhookSignature {
    #[serde(rename = "header")]
    pub header: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "encoding")]
    pub encoding: ScheduledTaskWebhookSignatureEncoding,
    #[serde(rename = "prefix")]
    pub prefix: String,
    #[serde(
        rename = "secret",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub secret: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        rename = "secretRef",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub secret_ref: Option<Option<crate::base_schemas::SecretRef>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskUpsertScheduleWebhook {
    #[serde(
        rename = "signature",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub signature: Option<Option<ScheduledTaskUpsertScheduleWebhookSignature>>,
    #[serde(
        rename = "maxDeliveryAgeMinutes",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub max_delivery_age_minutes: Option<Option<WebhookMaxDeliveryAgeMinutes>>,
    #[serde(
        rename = "dedupe",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub dedupe: Option<Option<ScheduledTaskWebhookDedupe>>,
    #[serde(
        rename = "filters",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub filters: Option<Option<ScheduledTaskWebhookFilters>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ScheduledTaskUpsertSchedule {
    #[serde(rename = "interval")]
    Interval(Box<ScheduledTaskScheduleInterval>),
    #[serde(rename = "fixed_time")]
    FixedTime(Box<ScheduledTaskScheduleFixedTime>),
    #[serde(rename = "once")]
    Once(Box<ScheduledTaskScheduleOnce>),
    #[serde(rename = "webhook")]
    Webhook(Box<ScheduledTaskUpsertScheduleWebhook>),
}
impl crate::wire::TaggedUnion for ScheduledTaskUpsertSchedule {
    const TAG: &'static str = "type";
    const KNOWN_TAGS: &'static [&'static str] = &["interval", "fixed_time", "once", "webhook"];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScheduledTaskRunStatus {
    #[serde(rename = "never")]
    Never,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "succeeded")]
    Succeeded,
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskWebhookEndpoint {
    #[serde(rename = "path")]
    pub path: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "url", with = "crate::wire::nullable")]
    pub url: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(rename = "hasSecret")]
    pub has_secret: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScheduledTaskThreadMode {
    #[serde(rename = "new_thread")]
    NewThread,
    #[serde(rename = "continue")]
    Continue,
    #[serde(rename = "fresh_session")]
    FreshSession,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTask {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::ScheduledTaskId,
    #[serde(rename = "title")]
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "prompt")]
    pub prompt: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "enabled")]
    pub enabled: bool,
    #[serde(rename = "schedule")]
    pub schedule: ScheduledTaskSchedule,
    #[serde(rename = "projectId")]
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(rename = "threadId", with = "crate::wire::nullable")]
    pub thread_id: Option<crate::base_schemas::ThreadId>,
    #[serde(
        rename = "threadMode",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub thread_mode: Option<Option<ScheduledTaskThreadMode>>,
    #[serde(rename = "workspaceStrategy")]
    pub workspace_strategy: crate::orchestration_v2::OrchestrationV2ThreadLaunchWorkspaceStrategy,
    #[serde(rename = "modelSelection")]
    pub model_selection: crate::model_selection::ModelSelection,
    #[serde(rename = "runtimeMode")]
    pub runtime_mode: crate::provider_policy::RuntimeMode,
    #[serde(rename = "interactionMode")]
    pub interaction_mode: crate::provider_policy::ProviderInteractionMode,
    #[serde(rename = "createdBy")]
    pub created_by: crate::orchestration_v2::OrchestrationV2Actor,
    #[serde(rename = "creationSource")]
    pub creation_source: crate::orchestration_v2::OrchestrationV2CreationSource,
    #[serde(rename = "createdAt")]
    pub created_at: crate::base_schemas::IsoDateTime,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::base_schemas::IsoDateTime,
    #[serde(rename = "nextRunAt", with = "crate::wire::nullable")]
    pub next_run_at: Option<crate::base_schemas::IsoDateTime>,
    #[serde(rename = "lastRunAt", with = "crate::wire::nullable")]
    pub last_run_at: Option<crate::base_schemas::IsoDateTime>,
    #[serde(rename = "lastRunStatus")]
    pub last_run_status: ScheduledTaskRunStatus,
    #[serde(rename = "lastRunError", with = "crate::wire::nullable")]
    pub last_run_error: Option<String>,
    #[serde(rename = "runCount")]
    pub run_count: i64,
    #[serde(
        rename = "webhook",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub webhook: Option<Option<ScheduledTaskWebhookEndpoint>>,
    #[serde(
        rename = "blockedByPolicy",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub blocked_by_policy: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskListInput {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskListResult {
    #[serde(
        rename = "tasks",
        deserialize_with = "crate::wire::deserialize_lenient_array"
    )]
    pub tasks: Vec<ScheduledTask>,
    #[serde(
        rename = "timeZone",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub time_zone: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskUpsertInput {
    #[serde(
        rename = "id",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub id: Option<Option<crate::base_schemas::ScheduledTaskId>>,
    #[serde(
        rename = "requireExisting",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub require_existing: Option<Option<bool>>,
    #[serde(
        rename = "commandId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub command_id: Option<Option<crate::base_schemas::CommandId>>,
    #[serde(rename = "title")]
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "prompt")]
    pub prompt: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "enabled")]
    pub enabled: bool,
    #[serde(rename = "schedule")]
    pub schedule: ScheduledTaskUpsertSchedule,
    #[serde(rename = "projectId")]
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        rename = "threadId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub thread_id: Option<Option<crate::base_schemas::ThreadId>>,
    #[serde(
        rename = "threadMode",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub thread_mode: Option<Option<ScheduledTaskThreadMode>>,
    #[serde(rename = "workspaceStrategy")]
    pub workspace_strategy: crate::orchestration_v2::OrchestrationV2ThreadLaunchWorkspaceStrategy,
    #[serde(rename = "modelSelection")]
    pub model_selection: crate::model_selection::ModelSelection,
    #[serde(rename = "runtimeMode")]
    pub runtime_mode: crate::provider_policy::RuntimeMode,
    #[serde(rename = "interactionMode")]
    pub interaction_mode: crate::provider_policy::ProviderInteractionMode,
    #[serde(
        rename = "createdBy",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub created_by: Option<Option<crate::orchestration_v2::OrchestrationV2Actor>>,
    #[serde(
        rename = "creationSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub creation_source: Option<Option<crate::orchestration_v2::OrchestrationV2CreationSource>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskSetEnabledInput {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::ScheduledTaskId,
    #[serde(rename = "enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskDeleteInput {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::ScheduledTaskId,
}

pub type ScheduledTaskRunNowInput = ScheduledTaskDeleteInput;

pub type ScheduledTaskRotateWebhookTokenInput = ScheduledTaskDeleteInput;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScheduledTaskWebhookDeliveryId(pub crate::base_schemas::TrimmedNonEmptyString);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScheduledTaskWebhookDeliveryOutcome {
    #[serde(rename = "accepted")]
    Accepted,
    #[serde(rename = "dispatch_failed")]
    DispatchFailed,
    #[serde(rename = "duplicate")]
    Duplicate,
    #[serde(rename = "coalesced")]
    Coalesced,
    #[serde(rename = "rejected_signature")]
    RejectedSignature,
    #[serde(rename = "disabled")]
    Disabled,
    #[serde(rename = "rate_limited")]
    RateLimited,
    #[serde(rename = "expired")]
    Expired,
    #[serde(rename = "filtered")]
    Filtered,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScheduledTaskWebhookDeliverySummaryOutcome {
    Known(ScheduledTaskWebhookDeliveryOutcome),
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskWebhookDeliverySummary {
    #[serde(rename = "id")]
    pub id: ScheduledTaskWebhookDeliveryId,
    #[serde(rename = "taskId")]
    pub task_id: crate::base_schemas::ScheduledTaskId,
    #[serde(rename = "receivedAt")]
    pub received_at: crate::base_schemas::IsoDateTime,
    #[serde(rename = "method")]
    pub method: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "contentType", with = "crate::wire::nullable")]
    pub content_type: Option<String>,
    #[serde(rename = "bodyBytes")]
    pub body_bytes: i64,
    #[serde(rename = "outcome")]
    pub outcome: ScheduledTaskWebhookDeliverySummaryOutcome,
    #[serde(rename = "signatureVerified")]
    pub signature_verified: bool,
    #[serde(
        rename = "pendingWindow",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub pending_window: Option<Option<bool>>,
    #[serde(rename = "missingFields")]
    pub missing_fields: Vec<String>,
    #[serde(rename = "error", with = "crate::wire::nullable")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskWebhookDelivery {
    #[serde(rename = "id")]
    pub id: ScheduledTaskWebhookDeliveryId,
    #[serde(rename = "taskId")]
    pub task_id: crate::base_schemas::ScheduledTaskId,
    #[serde(rename = "receivedAt")]
    pub received_at: crate::base_schemas::IsoDateTime,
    #[serde(rename = "method")]
    pub method: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "contentType", with = "crate::wire::nullable")]
    pub content_type: Option<String>,
    #[serde(rename = "bodyBytes")]
    pub body_bytes: i64,
    #[serde(rename = "outcome")]
    pub outcome: ScheduledTaskWebhookDeliverySummaryOutcome,
    #[serde(rename = "signatureVerified")]
    pub signature_verified: bool,
    #[serde(
        rename = "pendingWindow",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub pending_window: Option<Option<bool>>,
    #[serde(rename = "missingFields")]
    pub missing_fields: Vec<String>,
    #[serde(rename = "error", with = "crate::wire::nullable")]
    pub error: Option<String>,
    #[serde(rename = "query")]
    pub query: String,
    #[serde(rename = "headers")]
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(rename = "body")]
    pub body: String,
    #[serde(rename = "bodyTruncated")]
    pub body_truncated: bool,
    #[serde(rename = "renderedPrompt", with = "crate::wire::nullable")]
    pub rendered_prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskListWebhookDeliveriesInput {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::ScheduledTaskId,
    #[serde(
        rename = "acceptAnyOutcome",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub accept_any_outcome: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskListWebhookDeliveriesResult {
    #[serde(rename = "deliveries")]
    pub deliveries: Vec<ScheduledTaskWebhookDeliverySummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskGetWebhookDeliveryInput {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::ScheduledTaskId,
    #[serde(rename = "deliveryId")]
    pub delivery_id: ScheduledTaskWebhookDeliveryId,
    #[serde(
        rename = "acceptAnyOutcome",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub accept_any_outcome: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskGetWebhookDeliveryResult {
    #[serde(rename = "delivery")]
    pub delivery: ScheduledTaskWebhookDelivery,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskMutationResult {
    #[serde(rename = "task")]
    pub task: ScheduledTask,
}

pub type ScheduledTaskDeleteResult = ScheduledTaskDeleteInput;

pub type ScheduledTaskRunNowResult = ScheduledTaskMutationResult;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScheduledTaskErrorTag {
    #[serde(rename = "ScheduledTaskError")]
    ScheduledTaskError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskError {
    #[serde(rename = "_tag")]
    pub _tag: ScheduledTaskErrorTag,
    #[serde(rename = "message")]
    pub message: String,
    #[serde(
        rename = "taskId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub task_id: Option<Option<crate::base_schemas::ScheduledTaskId>>,
    #[serde(
        rename = "cause",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

pub const MIN_SCHEDULED_TASK_INTERVAL_MS: i64 = 60_000;
pub const MAX_WEBHOOK_DELIVERY_AGE_MINUTES: i64 = 24 * 60;
pub const MAX_WEBHOOK_FILTERS: usize = 16;

crate::redacted_debug!(ScheduledTaskUpsertScheduleWebhookSignature { header, encoding, prefix, secret_ref } redact { secret });
