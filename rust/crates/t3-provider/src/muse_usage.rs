use crate::{
    managed_provider::{ManagedProvider, RuntimeUsageUpdate},
    sensitive::{ServiceError, ServiceResult},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Weak};
use t3_contracts::wire::WireDateTime;
use tokio::sync::Mutex;
const LATEST_DATE_MS: f64 = 8.64e15;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Window {
    used_percent: f64,
    resets_at_ms: f64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    used_percent: f64,
    resets_at_ms: f64,
    window_duration_mins: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Usage {
    observed_at_ms: f64,
    tier: String,
    window: Session,
    weekly: Window,
}
fn iso(ms: f64) -> ServiceResult<String> {
    if !ms.is_finite() || ms <= 0.0 || ms > LATEST_DATE_MS {
        return Err(ServiceError::Response);
    }
    chrono::DateTime::from_timestamp_millis(ms.trunc() as i64)
        .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or(ServiceError::Response)
}
pub fn report(value: &Value) -> ServiceResult<RuntimeUsageUpdate> {
    let usage: Usage = serde_json::from_value(value.clone()).map_err(
        crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Response),
    )?;
    if !usage.window.used_percent.is_finite()
        || usage.window.used_percent < 0.0
        || !usage.weekly.used_percent.is_finite()
        || usage.weekly.used_percent < 0.0
        || usage.window.window_duration_mins == 0
    {
        return Err(ServiceError::Response);
    }
    let windows=serde_json::from_value(json!([
        {"id":"session","kind":"session","label":"Session","usedPercent":usage.window.used_percent.clamp(0.0,100.0),"resetsAt":iso(usage.window.resets_at_ms)?,"windowDurationMins":usage.window.window_duration_mins},
        {"id":"weekly","kind":"weekly","label":"Weekly","usedPercent":usage.weekly.used_percent.clamp(0.0,100.0),"resetsAt":iso(usage.weekly.resets_at_ms)?,"windowDurationMins":10080}
    ])).map_err(crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Response))?;
    let tier = t3_contracts::wire::trim_js(&usage.tier);
    Ok(RuntimeUsageUpdate {
        windows,
        checked_at: WireDateTime(iso(usage.observed_at_ms)?),
        complete: true,
        auth_label: (!tier.is_empty()).then(|| tier.into()),
    })
}
pub struct MuseUsageIngestion {
    provider: Weak<ManagedProvider>,
    latest: Mutex<Option<i64>>,
}
impl MuseUsageIngestion {
    pub fn new(provider: &Arc<ManagedProvider>) -> Self {
        Self {
            provider: Arc::downgrade(provider),
            latest: Mutex::new(None),
        }
    }
    pub async fn apply(&self, value: &Value) -> ServiceResult<bool> {
        let update = report(value)?;
        let timestamp = chrono::DateTime::parse_from_rfc3339(&update.checked_at.0)
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ))?
            .timestamp_millis();
        let mut latest = self.latest.lock().await;
        if latest.is_some_and(|latest| timestamp < latest) {
            return Ok(false);
        }
        let provider = self.provider.upgrade().ok_or(ServiceError::Inactive)?;
        provider.apply_usage(update).await;
        *latest = Some(timestamp);
        Ok(true)
    }
}
pub fn is_usage_limit_failure(message: Option<&str>) -> bool {
    message.is_some_and(|message| {
        regex::Regex::new(r"\b429\b")
            .expect("fixed quota status")
            .is_match(message)
            && regex::Regex::new(r"(?i)quota|usage limit")
                .expect("fixed quota classifier")
                .is_match(message)
    })
}
pub fn limit_reset_at(
    usage: Option<&Value>,
    now_ms: f64,
    refusal: Option<&Value>,
) -> Option<String> {
    let mut resets = vec![];
    if let Some(usage) = usage {
        for window in [&usage["window"], &usage["weekly"]] {
            if window["usedPercent"]
                .as_f64()
                .is_some_and(|used| used >= 100.0)
            {
                resets.extend(window["resetsAtMs"].as_f64());
            }
        }
    }
    if let Some(refusal) = refusal {
        resets.extend(refusal["quota"]["resetsAtMs"].as_f64());
        if let Some(message) = refusal["message"].as_str() {
            let pattern=regex::Regex::new(r"(?i)\bresets at (\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2}))").expect("fixed reset timestamp");
            if let Some(capture) = pattern
                .captures(message)
                .and_then(|captures| captures.get(1))
            {
                resets.extend(
                    chrono::DateTime::parse_from_rfc3339(capture.as_str())
                        .ok()
                        .map(|date| date.timestamp_millis() as f64),
                );
            }
        }
    }
    resets
        .into_iter()
        .filter(|ms| ms.is_finite() && *ms > now_ms && *ms <= LATEST_DATE_MS)
        .max_by(f64::total_cmp)
        .and_then(|ms| iso(ms).ok())
}
