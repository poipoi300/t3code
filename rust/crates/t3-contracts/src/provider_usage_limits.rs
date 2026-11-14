use crate::base_schemas::IsoDateTime;
use crate::base_schemas::TrimmedNonEmptyString;
use crate::provider_instance::{ProviderDriverKind, ProviderInstanceId};
use crate::speech::SpeechNumber;
use crate::usage_limit_source_id::UsageLimitSourceId;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerProviderUsageWindowKind {
    #[serde(rename = "session")]
    Session,
    #[serde(rename = "weekly")]
    Weekly,
    #[serde(rename = "monthly")]
    Monthly,
    #[serde(rename = "other")]
    Other,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderUsageWindow {
    pub id: TrimmedNonEmptyString,
    pub kind: ServerProviderUsageWindowKind,
    pub label: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub model_ids: Option<Option<Vec<TrimmedNonEmptyString>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_ids: Option<Option<Vec<TrimmedNonEmptyString>>>,
    pub used_percent: SpeechNumber,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub resets_at: Option<Option<IsoDateTime>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub window_duration_mins: Option<Option<u64>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderResetCreditOption {
    pub credit_id: TrimmedNonEmptyString,
    pub label: TrimmedNonEmptyString,
    pub description: TrimmedNonEmptyString,
    pub available_count: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expires_at: Option<Option<IsoDateTime>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderResetCredits {
    pub available_count: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub next_expires_at: Option<Option<IsoDateTime>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub next_credit_id: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_credit_options"
    )]
    pub options: Option<Option<Vec<ServerProviderResetCreditOption>>>,
}
fn deserialize_credit_options<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Option<Vec<ServerProviderResetCreditOption>>>, D::Error> {
    Ok(Some(Option::<Vec<serde_json::Value>>::deserialize(d)?.map(
        |values| {
            values
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect()
        },
    )))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderExternalUsage {
    pub label: TrimmedNonEmptyString,
    pub url: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerProviderUsageUnavailableReason {
    #[serde(rename = "unsupported")]
    Unsupported,
    #[serde(rename = "probeFailed")]
    ProbeFailed,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderUsageUnavailable {
    pub reason: ServerProviderUsageUnavailableReason,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub message: Option<Option<TrimmedNonEmptyString>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderUsageLimits {
    pub checked_at: IsoDateTime,
    #[serde(deserialize_with = "crate::wire::deserialize_lenient_array")]
    pub windows: Vec<ServerProviderUsageWindow>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub credential_fingerprint: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reset_credits: Option<Option<ServerProviderResetCredits>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub external_usage: Option<Option<ServerProviderExternalUsage>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub unavailable: Option<Option<ServerProviderUsageUnavailable>>,
    #[serde(
        rename = "zeroSpendCap",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub zero_spend_cap: Option<Option<bool>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageLimitsUpdate {
    pub windows: Vec<ServerProviderUsageWindow>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimitSourceAccount {
    pub id: TrimmedNonEmptyString,
    pub driver: ProviderDriverKind,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub email: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub plan: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub workspace_id: Option<Option<TrimmedNonEmptyString>>,
    pub usage_limits: ServerProviderUsageLimits,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UsageLimitSourceKind {
    #[serde(rename = "cliproxy")]
    Cliproxy,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimitSourceSnapshot {
    pub id: UsageLimitSourceId,
    pub kind: UsageLimitSourceKind,
    pub label: TrimmedNonEmptyString,
    pub checked_at: IsoDateTime,
    #[serde(deserialize_with = "crate::wire::deserialize_lenient_array")]
    pub accounts: Vec<UsageLimitSourceAccount>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub error: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        rename = "origin",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub origin: Option<Option<TrimmedNonEmptyString>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UsageLimitSourceSnapshots(
    #[serde(deserialize_with = "crate::wire::deserialize_lenient_array")]
    pub  Vec<UsageLimitSourceSnapshot>,
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimitSourceConsumeResetCreditInput {
    pub source_id: UsageLimitSourceId,
    pub account_id: TrimmedNonEmptyString,
    pub credit_id: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInstanceConsumeResetCreditInput {
    pub instance_id: ProviderInstanceId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub credit_id: Option<Option<TrimmedNonEmptyString>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProviderConsumeResetCreditInput {
    ProviderInstanceConsumeResetCreditInput(ProviderInstanceConsumeResetCreditInput),
    UsageLimitSourceConsumeResetCreditInput(UsageLimitSourceConsumeResetCreditInput),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UsageLimitSourceErrorTag {
    #[serde(rename = "UsageLimitSourceError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimitSourceError {
    #[serde(rename = "_tag")]
    pub _tag: UsageLimitSourceErrorTag,
    pub detail: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProviderConsumeResetCreditOutcome {
    #[serde(rename = "reset")]
    Reset,
    #[serde(rename = "nothingToReset")]
    NothingToReset,
    #[serde(rename = "noCredit")]
    NoCredit,
    #[serde(rename = "alreadyRedeemed")]
    AlreadyRedeemed,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConsumeResetCreditResult {
    pub outcome: ProviderConsumeResetCreditOutcome,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub warning: Option<Option<TrimmedNonEmptyString>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimitsReportAccount {
    pub id: TrimmedNonEmptyString,
    pub driver: ProviderDriverKind,
    pub label: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub plan: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub email: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub source_label: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub instance_id: Option<Option<ProviderInstanceId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reset_credit_input: Option<Option<ProviderConsumeResetCreditInput>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub display_name: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub accent_color: Option<Option<String>>,
    pub limits: ServerProviderUsageLimits,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimitsReport {
    pub created_at: IsoDateTime,
    pub accounts: Vec<UsageLimitsReportAccount>,
    pub notices: Vec<String>,
}
