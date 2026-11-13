//! Native, transport-neutral provider usage readers, pricing, aggregation and scan caching.
//! This server crate does not target `wasm32-unknown-unknown`.

pub mod aggregation;
pub mod cache;
pub mod codex;
mod collation;
pub mod database;
pub mod limit_sources;
pub mod limits;
pub mod pricing;
pub mod reader;
mod safety;
pub mod service;
mod snapshot;
pub mod sources;

use serde::{Deserialize, Serialize};
pub use t3_contracts::usage::{UsageProviderKind, UsageTokenTotals};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid usage window: {0}")]
    InvalidWindow(String),
    #[error("Usage scan failed: {0}")]
    ScanFailed(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error("Provider usage probe failed: {0}")]
    Probe(String),
    #[error("Usage worker failed: {0}")]
    Worker(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageSpeed {
    #[default]
    Standard,
    Fast,
    Ultrafast,
}

impl UsageSpeed {
    pub fn from_tier(tier: &str) -> Self {
        match tier {
            "priority" | "fast" => Self::Fast,
            "ultrafast" => Self::Ultrafast,
            _ => Self::Standard,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRecord {
    pub provider: UsageProviderKind,
    pub timestamp_ms: f64,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_model: Option<String>,
    pub session_id: String,
    pub totals: UsageTokenTotals,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation1h_tokens: Option<u64>,
    pub reported_cost_usd: Option<f64>,
    pub speed: UsageSpeed,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unresolved_service_tier_turn_id: Option<String>,
    pub dedupe_key: Option<String>,
}
impl UsageRecord {
    pub(crate) fn retained_size(&self) -> usize {
        self.model.len()
            + self.rate_model.as_ref().map_or(0, String::len)
            + self.session_id.len()
            + self.dedupe_key.as_ref().map_or(0, String::len)
            + self
                .unresolved_service_tier_turn_id
                .as_ref()
                .map_or(0, String::len)
            + 384
    }
}

pub fn empty_totals() -> UsageTokenTotals {
    UsageTokenTotals {
        uncached_input_tokens: 0,
        cached_input_tokens: 0,
        cache_creation_tokens: 0,
        output_tokens: 0,
        reasoning_tokens: 0,
    }
}

pub fn total_tokens(t: &UsageTokenTotals) -> u64 {
    t.uncached_input_tokens
        .saturating_add(t.cached_input_tokens)
        .saturating_add(t.cache_creation_tokens)
        .saturating_add(t.output_tokens)
}

pub(crate) fn tokens(v: &serde_json::Value) -> u64 {
    v.as_f64()
        .filter(|n| n.is_finite() && *n > 0.0)
        .unwrap_or(0.0)
        .trunc() as u64
}

pub fn timestamp_ms(value: &str) -> Option<f64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|d| d.timestamp_millis() as f64)
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|d| d.and_utc().timestamp_millis() as f64)
        })
}

pub(crate) fn text(v: &serde_json::Value) -> &str {
    v.as_str().unwrap_or("")
}

pub fn provider_name(p: &UsageProviderKind) -> &'static str {
    match p {
        UsageProviderKind::Claude => "claude",
        UsageProviderKind::Codex => "codex",
        UsageProviderKind::Grok => "grok",
        UsageProviderKind::Cursor => "cursor",
        UsageProviderKind::Opencode => "opencode",
        UsageProviderKind::Antigravity => "antigravity",
        UsageProviderKind::Pi => "pi",
        UsageProviderKind::Muse => "muse",
    }
}

pub(crate) fn iso(ms: f64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_default()
}
