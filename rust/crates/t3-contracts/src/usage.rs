use crate::base_schemas::TrimmedNonEmptyString;
use crate::speech::{SpeechNumber, True};
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub const USAGE_CONTRACT_VERSION: u64 = 6;
pub const USAGE_MERGE_COMPATIBLE_SINCE: u64 = 4;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UsageProviderKind {
    #[serde(rename = "claude")]
    Claude,
    #[serde(rename = "codex")]
    Codex,
    #[serde(rename = "grok")]
    Grok,
    #[serde(rename = "cursor")]
    Cursor,
    #[serde(rename = "opencode")]
    Opencode,
    #[serde(rename = "antigravity")]
    Antigravity,
    #[serde(rename = "pi")]
    Pi,
    #[serde(rename = "muse")]
    Muse,
}
crate::string_id!(UsageDay);
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UsageResolution {
    #[serde(rename = "day")]
    Day,
    #[serde(rename = "hour")]
    Hour,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UsageCostSource {
    #[serde(rename = "providerReported")]
    ProviderReported,
    #[serde(rename = "modelPriced")]
    ModelPriced,
    #[serde(rename = "unpriced")]
    Unpriced,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTokenTotals {
    pub uncached_input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_creation_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCategoryCost {
    pub input: SpeechNumber,
    pub cache_read: SpeechNumber,
    pub cache_write: SpeechNumber,
    pub output: SpeechNumber,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBucket {
    pub day: UsageDay,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub hour_start: Option<Option<TrimmedNonEmptyString>>,
    pub provider: UsageProviderKind,
    pub model: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub source_path: Option<Option<TrimmedNonEmptyString>>,
    pub totals: UsageTokenTotals,
    pub cost_usd: SpeechNumber,
    pub cache_savings_usd: SpeechNumber,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub category_cost_usd: Option<Option<UsageCategoryCost>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub fast_cost_usd: Option<Option<SpeechNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub ultrafast_cost_usd: Option<Option<SpeechNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub speed_premium_usd: Option<Option<SpeechNumber>>,
    pub cost_source: UsageCostSource,
    pub records: u64,
    pub unpriced_records: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub unpriced_tokens: Option<Option<u64>>,
    pub sessions: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSourceFingerprint {
    pub host_id: TrimmedNonEmptyString,
    pub provider: UsageProviderKind,
    pub resolved_home_path: TrimmedNonEmptyString,
    pub volume_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UsageSourceStatus {
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "missing")]
    Missing,
    #[serde(rename = "partial")]
    Partial,
    #[serde(rename = "failed")]
    Failed,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSource {
    pub fingerprint: UsageSourceFingerprint,
    pub status: UsageSourceStatus,
    pub scanned_files: u64,
    pub skipped_files: u64,
    pub malformed_records: u64,
    pub distinct_sessions: u64,
    #[serde(with = "crate::wire::nullable")]
    pub message: Option<TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub refreshing: Option<True>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UsagePricingStatus {
    #[serde(rename = "fresh")]
    Fresh,
    #[serde(rename = "cached")]
    Cached,
    #[serde(rename = "unavailable")]
    Unavailable,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePricing {
    pub status: UsagePricingStatus,
    pub source: TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub fetched_at: Option<String>,
    pub known_models: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageModelSelector {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider: Option<Option<TrimmedNonEmptyString>>,
    pub model: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageModelFilter {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub include: Option<Option<Vec<UsageModelSelector>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub exclude: Option<Option<Vec<UsageModelSelector>>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummaryInput {
    pub since_day: UsageDay,
    pub until_day: UsageDay,
    pub time_zone: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub resolution: Option<Option<UsageResolution>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub since_time: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub until_time: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub await_refresh: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provisional: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub model_filter: Option<Option<UsageModelFilter>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub contract_version: SpeechNumber,
    pub read_at: String,
    pub time_zone: TrimmedNonEmptyString,
    pub since_day: UsageDay,
    pub until_day: UsageDay,
    #[serde(deserialize_with = "crate::wire::deserialize_lenient_array")]
    pub buckets: Vec<UsageBucket>,
    #[serde(deserialize_with = "crate::wire::deserialize_lenient_array")]
    pub sources: Vec<UsageSource>,
    pub pricing: UsagePricing,
    pub scan_duration_ms: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub model_filter: Option<Option<UsageModelFilter>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UsageReadErrorReason {
    #[serde(rename = "scanFailed")]
    ScanFailed,
    #[serde(rename = "invalidWindow")]
    InvalidWindow,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UsageReadErrorTag {
    #[serde(rename = "UsageReadError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReadError {
    #[serde(rename = "_tag")]
    pub _tag: UsageReadErrorTag,
    pub reason: UsageReadErrorReason,
    pub detail: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<Value>>,
}
