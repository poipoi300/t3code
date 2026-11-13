use crate::base_schemas::TrimmedNonEmptyString;
use crate::provider_instance::{ProviderDriverKind, ProviderInstanceEnvironmentVariableName};
use crate::provider_policy::RuntimeMode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedPolicySetting {
    pub value: Value,
    pub locked: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedProviderEnvironmentVariable {
    pub name: ProviderInstanceEnvironmentVariableName,
    pub value: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedProviderInstance {
    pub locked: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub environment: Option<Vec<ManagedProviderEnvironmentVariable>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub binary_path: Option<TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub launch_args: Option<TrimmedNonEmptyString>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedProviderLimits {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub allowed_drivers: Option<Vec<ProviderDriverKind>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub instances: Option<BTreeMap<ProviderDriverKind, ManagedProviderInstance>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ManagedUpdatesMode {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "off")]
    Off,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedUpdatesPolicy {
    pub mode: ManagedUpdatesMode,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub feed_url: Option<TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub allow_nightly: Option<bool>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ManagedPolicyClient {
    #[serde(rename = "desktop")]
    Desktop,
    #[serde(rename = "local-browser")]
    LocalBrowser,
    #[serde(rename = "remote")]
    Remote,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedPolicyLimits {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub max_runtime_mode: Option<RuntimeMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub clients: Option<Vec<ManagedPolicyClient>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub connect: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub webhook_tasks: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub telemetry: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub browser_cookie_import: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub providers: Option<ManagedProviderLimits>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub updates: Option<ManagedUpdatesPolicy>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedPolicySummaryProviderLimits {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub allowed_drivers: Option<Vec<ProviderDriverKind>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub locked_instance_fields: Option<BTreeMap<ProviderDriverKind, Vec<String>>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedPolicySummaryUpdates {
    pub mode: ManagedUpdatesMode,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub allow_nightly: Option<bool>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedPolicySummaryLimits {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub max_runtime_mode: Option<RuntimeMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub clients: Option<Vec<ManagedPolicyClient>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub connect: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub webhook_tasks: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub telemetry: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub browser_cookie_import: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub providers: Option<ManagedPolicySummaryProviderLimits>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub updates: Option<ManagedPolicySummaryUpdates>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedPolicyDocument {
    pub schema_version: ManagedPolicyVersion,
    pub organization: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub support_url: Option<TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub settings: Option<BTreeMap<String, ManagedPolicySetting>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub limits: Option<ManagedPolicyLimits>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct ManagedPolicyVersion;
impl TryFrom<u64> for ManagedPolicyVersion {
    type Error = &'static str;
    fn try_from(v: u64) -> Result<Self, Self::Error> {
        if v == 1 {
            Ok(Self)
        } else {
            Err("unsupported managed policy version")
        }
    }
}
impl From<ManagedPolicyVersion> for u64 {
    fn from(_: ManagedPolicyVersion) -> u64 {
        1
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedPolicySummary {
    pub organization: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub support_url: Option<TrimmedNonEmptyString>,
    pub locked_settings: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub limits: Option<ManagedPolicySummaryLimits>,
}
