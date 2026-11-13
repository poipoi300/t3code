use crate::{FailureReason, ForgeError, Result};
use std::{collections::HashMap, sync::Mutex};
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RateLimitKey {
    pub provider: String,
    pub host: String,
    pub credential_scope: String,
}
impl RateLimitKey {
    fn normalized(&self) -> Self {
        Self {
            host: self.host.trim().to_lowercase(),
            ..self.clone()
        }
    }
}
#[derive(Debug, Clone, Default)]
struct Entry {
    attempt: u32,
    generation: u64,
    retry_at: u64,
}
#[derive(Default)]
pub struct RateLimits {
    entries: Mutex<HashMap<RateLimitKey, Entry>>,
}
impl RateLimits {
    pub fn check(&self, key: &RateLimitKey, now: u64, allow_paused: bool) -> Result<u64> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = entries.get(&key.normalized());
        if let Some(entry) = entry.filter(|entry| entry.retry_at > now && !allow_paused) {
            return Err(ForgeError::Provider {
                provider: crate::SourceControlProviderKind(key.provider.clone()),
                operation: "rateLimit".into(),
                reason: FailureReason::RateLimited,
                detail: format!(
                    "{} requests to {} are paused until the rate limit resets.",
                    key.provider,
                    key.host.trim().to_lowercase()
                )
                .into(),
                retry_at: Some(entry.retry_at),
                access_hint: None,
                status: None,
                diagnostics: Box::default(),
            });
        }
        Ok(entry.map_or(0, |e| e.generation))
    }
    pub fn record_limit(&self, key: &RateLimitKey, lease: u64, retry_at: Option<u64>, now: u64) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let previous = entries.entry(key.normalized()).or_default();
        if previous.generation > lease {
            previous.retry_at = previous.retry_at.max(retry_at.unwrap_or(0));
            return;
        }
        previous.attempt = previous.attempt.saturating_add(1);
        let cooldown = 30_000u64
            .saturating_mul(1u64 << previous.attempt.saturating_sub(1).min(5))
            .min(900_000);
        let proposed = retry_at
            .filter(|at| *at > now)
            .unwrap_or(now.saturating_add(cooldown));
        previous.retry_at = proposed.max(if previous.retry_at > now {
            previous.retry_at
        } else {
            0
        });
        previous.generation = previous.generation.max(lease).saturating_add(1);
    }
    pub fn record_success(&self, key: &RateLimitKey, lease: u64, now: u64) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = entries
            .get_mut(&key.normalized())
            .filter(|e| e.generation == lease && e.retry_at <= now)
        {
            entry.attempt = 0;
            entry.retry_at = 0;
        }
    }
}
pub fn retry_at_from_header(value: &str, now: u64) -> Option<u64> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        let at = now.checked_add(value.parse::<u64>().ok()?.checked_mul(1000)?)?;
        return (at <= 9_007_199_254_740_991).then_some(at);
    }
    let parsed = chrono::DateTime::parse_from_rfc2822(value)
        .ok()?
        .timestamp_millis();
    let at = u64::try_from(parsed).ok()?;
    (at > now).then_some(at)
}
