use crate::{
    native_probe::{NativeProbe, ProbeLaunch},
    sensitive::{Sensitive, ServiceResult},
    transport::ProviderClock,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;
struct State {
    key: String,
    expires: i64,
    cached: Option<Sensitive<Value>>,
    last_good: Option<(Sensitive<Value>, Option<String>)>,
}
pub struct ClaudeProbeCache {
    inner: Arc<dyn NativeProbe>,
    clock: Arc<dyn ProviderClock>,
    account: std::path::PathBuf,
    state: Mutex<State>,
}
impl ClaudeProbeCache {
    pub fn new(
        inner: Arc<dyn NativeProbe>,
        clock: Arc<dyn ProviderClock>,
        account: std::path::PathBuf,
    ) -> Self {
        Self {
            inner,
            clock,
            account,
            state: Mutex::new(State {
                key: String::new(),
                expires: 0,
                cached: None,
                last_good: None,
            }),
        }
    }
    pub async fn invalidate(&self) {
        let mut state = self.state.lock().await;
        state.cached = None;
        state.expires = 0;
    }
}
fn organization(path: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value["oauthAccount"]["organizationUuid"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
#[async_trait]
impl NativeProbe for ClaudeProbeCache {
    async fn consume_codex_reset(
        &self,
        launch: ProbeLaunch,
        key: String,
    ) -> ServiceResult<t3_contracts::provider_usage_limits::ProviderConsumeResetCreditOutcome> {
        self.inner.consume_codex_reset(launch, key).await
    }
    async fn codex(&self, launch: ProbeLaunch) -> ServiceResult<Sensitive<Value>> {
        self.inner.codex(launch).await
    }
    async fn claude(&self, launch: ProbeLaunch) -> ServiceResult<Sensitive<Value>> {
        if !launch.include_status {
            return self.inner.claude(launch).await;
        }
        let key = serde_json::to_string(&(
            &launch.executable,
            launch.environment.get("CLAUDE_CONFIG_DIR"),
            launch.environment.get("HOME"),
            launch.environment.get("USERPROFILE"),
        ))
        .map_err(|_| crate::sensitive::ServiceError::Response)?;
        let mut state = self.state.lock().await;
        if state.key != key {
            state.key = key;
            state.cached = None;
            state.last_good = None;
            state.expires = 0;
        }
        if state.expires > self.clock.now_ms()
            && let Some(cached) = state.cached.as_ref()
        {
            return Ok(cached.clone());
        }
        let organization = organization(&self.account);
        match self.inner.claude(launch).await {
            Ok(raw) => {
                let mut raw = raw.into_inner();
                if let Some(organization) = organization.as_ref() {
                    raw["workspaceId"] = Value::String(organization.clone());
                }
                raw["usageCheckedAt"] =
                    chrono::DateTime::from_timestamp_millis(self.clock.now_ms())
                        .map(|d| {
                            Value::String(d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
                        })
                        .unwrap_or(Value::Null);
                let authenticated =
                    crate::probe_models::claude_auth(&raw["initialize"]["account"], false)
                        .is_ok_and(|(_, status)| status != "error");
                let raw = Sensitive::new(raw);
                state.expires = if raw.expose().get("usage").is_some() && authenticated {
                    self.clock.now_ms().saturating_add(300_000)
                } else {
                    0
                };
                state.cached = authenticated.then(|| raw.clone());
                state.last_good = authenticated.then(|| (raw.clone(), organization));
                Ok(raw)
            }
            Err(error) => {
                state.cached = None;
                state.expires = 0;
                if let Some((good, previous)) = state
                    .last_good
                    .take()
                    .filter(|(_, previous)| *previous == organization)
                {
                    let mut value = good.into_inner();
                    if let Some(map) = value.as_object_mut() {
                        map.remove("usage");
                        map.remove("usageCheckedAt");
                    }
                    let _ = previous;
                    return Ok(Sensitive::new(value));
                }
                Err(error)
            }
        }
    }
}
