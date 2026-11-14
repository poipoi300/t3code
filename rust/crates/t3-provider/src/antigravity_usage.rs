use crate::{
    sensitive::{Sensitive, ServiceError, ServiceResult},
    transport::{HttpRequest, HttpTransport, ProviderClock},
    usage::make_usage_limits,
};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use t3_contracts::{
    provider_usage_limits::{ServerProviderUsageLimits, ServerProviderUsageWindow},
    wire::WireDateTime,
};
use tokio::sync::Mutex;
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Quota {
    #[serde(default, deserialize_with = "present")]
    remaining_fraction: Option<f64>,
    #[serde(default, deserialize_with = "present")]
    reset_time: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bucket {
    #[serde(flatten)]
    quota: Quota,
    bucket_id: String,
    #[serde(default, deserialize_with = "present")]
    window: Option<String>,
    #[serde(default, deserialize_with = "present")]
    display_name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    description: Option<String>,
    #[serde(default, deserialize_with = "present")]
    disabled: Option<bool>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Group {
    #[serde(default, deserialize_with = "present")]
    display_name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    buckets: Option<Vec<Bucket>>,
}
#[derive(Deserialize)]
struct Summary {
    #[serde(default, deserialize_with = "present")]
    groups: Option<Vec<Group>>,
    #[serde(default, deserialize_with = "present")]
    buckets: Option<Vec<Bucket>>,
}
fn stamp(ms: i64) -> ServiceResult<String> {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or(ServiceError::Response)
}
fn parse_time(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.timestamp_millis())
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)
                .map(|date| date.and_utc().timestamp_millis())
        })
}
fn percent(value: &t3_contracts::speech::SpeechNumber) -> f64 {
    match value {
        t3_contracts::speech::SpeechNumber::Finite(value) => value.as_f64().unwrap_or(0.0),
        _ => 0.0,
    }
}
fn quota(
    id: &str,
    label: &str,
    quota: &Quota,
    kind: &str,
    duration: Option<u64>,
) -> Option<ServerProviderUsageWindow> {
    let remaining = quota
        .remaining_fraction
        .filter(|remaining| remaining.is_finite())?;
    let mut value = json!({"id":id,"label":label,"kind":kind,"usedPercent":(1.0-remaining.clamp(0.0,1.0))*100.0});
    if let Some(reset) = quota
        .reset_time
        .as_deref()
        .and_then(parse_time)
        .and_then(|ms| stamp(ms).ok())
    {
        value["resetsAt"] = json!(reset);
    }
    if let Some(duration) = duration {
        value["windowDurationMins"] = json!(duration);
    }
    serde_json::from_value(value).ok()
}
pub fn summary_windows(
    value: &Value,
    requested_at: i64,
) -> ServiceResult<Vec<ServerProviderUsageWindow>> {
    let summary: Summary =
        serde_json::from_value(value.clone()).map_err(|_| ServiceError::Response)?;
    let groups = summary
        .groups
        .filter(|groups| {
            groups.iter().any(|group| {
                group
                    .buckets
                    .as_ref()
                    .is_some_and(|buckets| !buckets.is_empty())
            })
        })
        .unwrap_or_else(|| {
            vec![Group {
                display_name: None,
                buckets: summary.buckets,
            }]
        });
    let mut windows: Vec<ServerProviderUsageWindow> = vec![];
    for group in groups {
        for mut bucket in group.buckets.into_iter().flatten() {
            if bucket.bucket_id.is_empty() {
                return Err(ServiceError::Response);
            }
            if bucket.disabled == Some(true) {
                continue;
            }
            let group_name = group
                .display_name
                .as_deref()
                .map(t3_contracts::wire::trim_js)
                .filter(|name| !name.is_empty());
            let family = if bucket.bucket_id.starts_with("gemini-argon-") {
                group_name.unwrap_or("Gemini Argon")
            } else if bucket.bucket_id.starts_with("gemini-carbon-") {
                group_name.unwrap_or("Gemini Carbon")
            } else if bucket.bucket_id.starts_with("gemini-") {
                "Gemini"
            } else if bucket.bucket_id.starts_with("3p-") {
                "Claude & GPT (shared)"
            } else {
                group_name.unwrap_or(&bucket.bucket_id)
            };
            let (kind, duration, label) = match bucket.window.as_deref() {
                Some("weekly") => ("weekly", Some(10080), "Weekly"),
                Some("5h") => ("session", Some(300), "5-hour"),
                _ => (
                    "other",
                    None,
                    bucket
                        .display_name
                        .as_deref()
                        .map(t3_contracts::wire::trim_js)
                        .filter(|name| !name.is_empty())
                        .unwrap_or("Quota"),
                ),
            };
            if bucket.quota.remaining_fraction == Some(1.0)
                && bucket.description.is_none()
                && let Some(duration) = duration
                && bucket
                    .quota
                    .reset_time
                    .as_deref()
                    .and_then(parse_time)
                    .is_some_and(|reset| {
                        (reset as i128 - requested_at as i128 - duration as i128 * 60000).abs()
                            <= 60000
                    })
            {
                bucket.quota.reset_time = None;
            }
            if let Some(window) = quota(
                &bucket.bucket_id,
                &format!("{family} · {label}"),
                &bucket.quota,
                kind,
                duration,
            ) {
                if let Some(previous) = windows.iter_mut().find(|previous| previous.id == window.id)
                {
                    *previous = window;
                } else {
                    windows.push(window);
                }
            }
        }
    }
    Ok(windows)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Model {
    #[serde(default, deserialize_with = "present")]
    model_provider: Option<String>,
    #[serde(default, deserialize_with = "present")]
    quota_info: Option<Quota>,
    #[serde(default, deserialize_with = "present")]
    weekly_quota_info: Option<Quota>,
}
#[derive(Deserialize)]
struct Models {
    models: BTreeMap<String, Model>,
}
pub fn model_windows(value: &Value) -> ServiceResult<Vec<ServerProviderUsageWindow>> {
    let models: Models =
        serde_json::from_value(value.clone()).map_err(|_| ServiceError::Response)?;
    let mut windows: Vec<ServerProviderUsageWindow> = vec![];
    for (id, model) in models.models {
        let family = if model.model_provider.as_deref() == Some("MODEL_PROVIDER_GOOGLE")
            || id.starts_with("gemini-")
        {
            "Gemini"
        } else if model.model_provider.as_deref() == Some("MODEL_PROVIDER_ANTHROPIC")
            || id.starts_with("claude-")
        {
            "Claude"
        } else if model.model_provider.as_deref() == Some("MODEL_PROVIDER_OPENAI")
            || id.starts_with("gpt-")
        {
            "GPT"
        } else {
            continue;
        };
        for (suffix, value) in [
            ("quota", model.quota_info),
            ("weekly", model.weekly_quota_info),
        ] {
            let Some(value) = value else {
                continue;
            };
            let weekly = suffix == "weekly";
            if let Some(window) = quota(
                &format!("{}-{suffix}", family.to_lowercase()),
                &format!(
                    "{family} · {}",
                    if weekly { "Weekly" } else { "Reported quota" }
                ),
                &value,
                if weekly { "weekly" } else { "other" },
                weekly.then_some(10080),
            ) {
                if let Some(previous) = windows.iter_mut().find(|previous| previous.id == window.id)
                {
                    if percent(&window.used_percent) > percent(&previous.used_percent) {
                        *previous = window;
                    }
                } else {
                    windows.push(window);
                }
            }
        }
    }
    Ok(windows)
}
pub fn account_fingerprint(value: &Value) -> ServiceResult<String> {
    let id = value["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or(ServiceError::Response)?;
    Ok(format!(
        "{:x}",
        Sha256::digest(format!("google-account\0{id}").as_bytes())
    ))
}
#[derive(Deserialize)]
struct Token {
    client_id: String,
    client_secret: String,
    refresh_token: String,
    #[serde(default, deserialize_with = "present")]
    project_id: Option<String>,
}
#[derive(Clone)]
struct Cached {
    fingerprint: String,
    account: String,
    access: Sensitive<String>,
    expires: i64,
}
pub struct AntigravityUsage {
    pub token_path: PathBuf,
    pub method: String,
    pub platform: String,
    pub architecture: String,
    http: Arc<dyn HttpTransport>,
    clock: Arc<dyn ProviderClock>,
    cache: Mutex<Option<Cached>>,
    gate: Mutex<()>,
    generation: AtomicU64,
}
impl AntigravityUsage {
    pub fn new(
        token_path: PathBuf,
        method: String,
        platform: String,
        architecture: String,
        http: Arc<dyn HttpTransport>,
        clock: Arc<dyn ProviderClock>,
    ) -> Self {
        Self {
            token_path,
            method,
            platform,
            architecture,
            http,
            clock,
            cache: Mutex::new(None),
            gate: Mutex::new(()),
            generation: AtomicU64::new(0),
        }
    }
    pub async fn clear(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        *self.cache.lock().await = None;
    }
    async fn request(&self, request: HttpRequest) -> ServiceResult<Sensitive<Value>> {
        let response = tokio::time::timeout(Duration::from_secs(10), self.http.send(request))
            .await
            .map_err(|_| ServiceError::Network)??;
        if !(200..300).contains(&response.status) {
            return Err(ServiceError::Network);
        }
        Ok(response.body)
    }
    fn unavailable(
        &self,
        reason: &str,
        message: Option<&str>,
    ) -> ServiceResult<ServerProviderUsageLimits> {
        let mut value = json!({"checkedAt":stamp(self.clock.now_ms())?,"windows":[],"unavailable":{"reason":reason}});
        if let Some(message) = message {
            value["unavailable"]["message"] = json!(message);
        }
        serde_json::from_value(value).map_err(|_| ServiceError::Response)
    }
    pub async fn probe(&self) -> ServiceResult<ServerProviderUsageLimits> {
        match tokio::time::timeout(Duration::from_secs(30), self.read()).await {
            Ok(Ok(limits)) => Ok(limits),
            _ => self.unavailable(
                "probeFailed",
                Some("Could not read Antigravity limits. Refresh provider status to retry."),
            ),
        }
    }
    async fn read(&self) -> ServiceResult<ServerProviderUsageLimits> {
        let _gate = self.gate.lock().await;
        let generation = self.generation.load(Ordering::SeqCst);
        let checked = stamp(self.clock.now_ms())?;
        if !matches!(self.method.as_str(), "oauth-personal" | "oauth-business") {
            return self.unavailable("unsupported", None);
        }
        let metadata = match tokio::fs::metadata(&self.token_path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                *self.cache.lock().await = None;
                return self.unavailable(
                    "unsupported",
                    Some("Sign in with Google to see Antigravity limits."),
                );
            }
            Err(_) => return Err(ServiceError::Storage),
        };
        if metadata.len() > 1024 * 1024 {
            return Err(ServiceError::Response);
        }
        let token: Token = serde_json::from_slice(
            &tokio::fs::read(&self.token_path)
                .await
                .map_err(|_| ServiceError::Storage)?,
        )
        .map_err(|_| ServiceError::Response)?;
        if [&token.client_id, &token.client_secret, &token.refresh_token]
            .iter()
            .any(|value| value.is_empty())
        {
            return Err(ServiceError::Response);
        }
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(
                format!(
                    "{}\0{}\0{}",
                    token.client_id,
                    token.refresh_token,
                    token.project_id.as_deref().unwrap_or_default()
                )
                .as_bytes()
            )
        );
        let now = self.clock.now_ms();
        let cached = self.cache.lock().await.clone().filter(|cached| {
            cached.fingerprint == fingerprint && cached.expires > now.saturating_add(60000)
        });
        let cached = if let Some(cached) = cached {
            cached
        } else {
            *self.cache.lock().await = None;
            let form = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("client_id", &token.client_id)
                .append_pair("client_secret", &token.client_secret)
                .append_pair("refresh_token", &token.refresh_token)
                .append_pair("grant_type", "refresh_token")
                .finish();
            let response = self
                .request(HttpRequest {
                    method: "POST",
                    url: "https://oauth2.googleapis.com/token".into(),
                    headers: BTreeMap::from([(
                        "content-type".into(),
                        Sensitive::new("application/x-www-form-urlencoded".into()),
                    )]),
                    body: Some(Sensitive::new(form)),
                })
                .await?;
            let access = response.expose()["access_token"]
                .as_str()
                .filter(|access| !access.is_empty())
                .ok_or(ServiceError::Response)?;
            let expires = response.expose()["expires_in"]
                .as_f64()
                .filter(|expires| expires.is_finite() && *expires > 0.0)
                .ok_or(ServiceError::Response)?;
            let account = self
                .request(HttpRequest {
                    method: "GET",
                    url: "https://www.googleapis.com/oauth2/v2/userinfo".into(),
                    headers: BTreeMap::from([(
                        "Authorization".into(),
                        Sensitive::new(format!("Bearer {access}")),
                    )]),
                    body: None,
                })
                .await
                .ok()
                .and_then(|value| account_fingerprint(value.expose()).ok())
                .unwrap_or_else(|| fingerprint.clone());
            let cached = Cached {
                fingerprint,
                account,
                access: Sensitive::new(access.into()),
                expires: now.saturating_add((expires * 1000.0) as i64),
            };
            if generation == self.generation.load(Ordering::SeqCst) {
                *self.cache.lock().await = Some(cached.clone());
            }
            cached
        };
        let endpoints = if self.method == "oauth-business" {
            [
                "https://cloudcode-pa.googleapis.com",
                "https://daily-cloudcode-pa.googleapis.com",
            ]
        } else {
            [
                "https://daily-cloudcode-pa.googleapis.com",
                "https://cloudcode-pa.googleapis.com",
            ]
        };
        let fetch = async |operation: &str| {
            let request = async |endpoint: &str| {
                let requested = self.clock.now_ms();
                let response=self.request(HttpRequest{method:"POST",url:format!("{endpoint}/v1internal:{operation}"),headers:BTreeMap::from([("Authorization".into(),Sensitive::new(format!("Bearer {}",cached.access.expose()))),("User-Agent".into(),Sensitive::new(format!("antigravity/hub/2.24.0 (aidev_client; os_type={}; arch={}; cl=963137146)",self.platform,self.architecture)))]),body:Some(Sensitive::new(serde_json::to_string(&token.project_id.as_ref().filter(|project|!project.is_empty()).map(|project|json!({"project":project})).unwrap_or_else(||json!({}))).map_err(|_|ServiceError::Response)?))}).await?;
                if operation == "retrieveUserQuotaSummary" {
                    summary_windows(response.expose(), requested)
                } else {
                    model_windows(response.expose())
                }
            };
            let (a, b) = tokio::join!(request(endpoints[0]), request(endpoints[1]));
            let mut windows: Vec<ServerProviderUsageWindow> = vec![];
            for window in a
                .unwrap_or_default()
                .into_iter()
                .chain(b.unwrap_or_default())
            {
                if !windows.iter().any(|previous| previous.id == window.id) {
                    windows.push(window);
                }
            }
            Ok::<_, ServiceError>(windows)
        };
        let mut windows = fetch("retrieveUserQuotaSummary").await?;
        if windows.is_empty() {
            windows = fetch("fetchAvailableModels").await?;
        }
        if generation != self.generation.load(Ordering::SeqCst) {
            return Err(ServiceError::Inactive);
        }
        if windows.is_empty() {
            return self.unavailable(
                "probeFailed",
                Some("Antigravity did not return usable quota information."),
            );
        }
        let limits = make_usage_limits(windows, WireDateTime(checked));
        let mut limits = serde_json::to_value(limits).map_err(|_| ServiceError::Response)?;
        limits["credentialFingerprint"] = json!(cached.account);
        serde_json::from_value(limits).map_err(|_| ServiceError::Response)
    }
}
