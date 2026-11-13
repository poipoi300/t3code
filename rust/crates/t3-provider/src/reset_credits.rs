use crate::{
    environment::Platform,
    sensitive::{SecretString, Sensitive, ServiceError},
    transport::{HttpRequest, HttpTransport, ProviderClock},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    path::Path,
    sync::{Arc, LazyLock},
    time::Duration,
};
use t3_contracts::provider_usage_limits::{
    ProviderConsumeResetCreditOutcome as Outcome, ServerProviderResetCredits,
};
use tokio::sync::Mutex;
type Account = Arc<Mutex<Option<String>>>;
#[derive(Default)]
pub struct ResetCreditCoordinator {
    accounts: Mutex<BTreeMap<String, Account>>,
}
impl ResetCreditCoordinator {
    pub fn shared() -> Arc<Self> {
        static SHARED: LazyLock<Arc<ResetCreditCoordinator>> =
            LazyLock::new(|| Arc::new(ResetCreditCoordinator::default()));
        SHARED.clone()
    }
    pub async fn redeem<E, F, Fut>(
        &self,
        account: &str,
        consume: F,
        settled: impl Fn(&E) -> bool,
    ) -> Result<Outcome, E>
    where
        F: FnOnce(String) -> Fut,
        Fut: Future<Output = Result<Outcome, E>>,
    {
        let state = self
            .accounts
            .lock()
            .await
            .entry(account.into())
            .or_insert_with(|| Arc::new(Mutex::new(None)))
            .clone();
        let mut pending = state.lock().await;
        let key = pending
            .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        let result = consume(key).await;
        if result.as_ref().err().is_none_or(settled) {
            *pending = None;
        }
        result
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ClaudeResetError {
    #[error("Claude returned a malformed reset credit.")]
    MalformedCredit,
    #[error("Claude could not read its login.")]
    LoginUnreadable,
    #[error("Claude could not read its account.")]
    AccountUnreadable,
    #[error("Sign in to Claude again to redeem resets.")]
    SignedOut,
    #[error("Claude is rate limiting resets. Try again soon.")]
    RateLimited,
    #[error("Claude resets are cooling down. Try again later.")]
    CoolingDown,
    #[error("Claude could not confirm the reset. If you are still limited in a moment, try again.")]
    Unconfirmed,
    #[error("Claude could not redeem the reset.")]
    RequestFailed,
}
impl ClaudeResetError {
    pub fn settled(&self) -> bool {
        !matches!(self, Self::Unconfirmed | Self::RequestFailed)
    }
}
static GRANT: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[a-z0-9_-]{1,40}$").expect("grant ID"));
static REQUEST: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[A-Za-z0-9_-]{1,64}$").expect("request ID"));
static TIMESTAMP: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$")
        .expect("complete timestamp")
});
fn expiry(grant: &Value) -> Option<String> {
    let raw = grant["ends_at"].as_str()?;
    chrono::DateTime::parse_from_rfc3339(raw).ok().map(|d| {
        d.to_utc()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    })
}
fn kind(grant: &Value) -> &'static str {
    if grant["gives_back_weekly"] == true {
        return "fable";
    }
    match grant["type"].as_str() {
        Some("session") => return "session",
        Some("weekly") => return "weekly",
        Some("full") => return "full",
        _ => {}
    }
    let limits = grant["clears"].as_array();
    let session = limits.is_some_and(|a| a.contains(&json!("five_hour")));
    let other = limits.is_some_and(|a| a.iter().any(|v| v != "five_hour"));
    match (session, other) {
        (true, false) => "session",
        (false, true) => "weekly",
        _ => "full",
    }
}
pub fn claude_credits(block: &Value, now: i64) -> Option<ServerProviderResetCredits> {
    if block["eligible"] != true
        || block.get("grants").is_some_and(|v| !v.is_array())
        || block
            .get("next_grant_id")
            .is_some_and(|v| !v.is_null() && !v.is_string())
    {
        return None;
    }
    let live: Vec<_> = block["grants"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|g| {
            g["id"].as_str().is_some_and(|s| GRANT.is_match(s))
                && g["resets_left"].as_u64().is_some()
                && ["paused", "usable_now", "gives_back_weekly"]
                    .iter()
                    .all(|k| g.get(*k).is_none_or(Value::is_boolean))
                && g.get("clears").is_none_or(Value::is_array)
                && g["paused"] != true
                && g["usable_now"] == true
                && match g.get("ends_at") {
                    None | Some(Value::Null) => true,
                    Some(Value::String(s)) => {
                        TIMESTAMP.is_match(s)
                            && crate::catalog::date_millis(s).is_some_and(|expiry| expiry > now)
                    }
                    _ => false,
                }
        })
        .collect();
    let next = live.iter().find(|g| g["id"] == block["next_grant_id"]);
    let sum = |grants: &[&Value]| {
        grants.iter().fold(0u64, |sum, g| {
            sum.saturating_add(g["resets_left"].as_u64().unwrap_or(0))
        })
    };
    let mut value = json!({"availableCount":if next.is_some(){sum(&live)}else{0}});
    if let Some(next) = next {
        value["nextCreditId"] = next["id"].clone();
        if let Some(expiry) = expiry(next) {
            value["nextExpiresAt"] = json!(expiry);
        }
        let mut options = vec![];
        for (kind_id, label, description) in [
            ("session", "5-hour reset", "Refills your 5-hour limit."),
            ("weekly", "Weekly reset", "Refills your weekly limit."),
            (
                "fable",
                "Fable reset",
                "Refills your Fable allowance and gives back what Fable used on your weekly limit. It doesn't reset your whole week.",
            ),
            (
                "full",
                "Full reset",
                "Refills your 5-hour and weekly limits.",
            ),
        ] {
            let grants: Vec<_> = live
                .iter()
                .copied()
                .filter(|g| g["resets_left"].as_u64().is_some_and(|n| n > 0) && kind(g) == kind_id)
                .collect();
            let Some(target) = grants.first() else {
                continue;
            };
            let mut option = json!({"creditId":target["id"],"label":label,"description":description,"availableCount":sum(&grants)});
            if let Some(expiry) = expiry(target) {
                option["expiresAt"] = json!(expiry);
            }
            options.push(option);
        }
        value["options"] = json!(options);
    }
    serde_json::from_value(value).ok()
}
fn json_file(path: &Path) -> Result<Value, ServiceError> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() <= 8 * 1024 * 1024 => {
            serde_json::from_slice(&bytes).map_err(|_| ServiceError::Storage)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        _ => Err(ServiceError::Storage),
    }
}
pub struct ClaudeResetClient {
    pub http: Arc<dyn HttpTransport>,
    pub clock: Arc<dyn ProviderClock>,
    pub platform: Platform,
}
impl ClaudeResetClient {
    fn token(&self, config: &Path) -> Result<Option<SecretString>, ServiceError> {
        if self.platform == Platform::Macos {
            return Ok(None);
        }
        let value = json_file(&config.join(".credentials.json"))?;
        Ok(value["claudeAiOauth"]["accessToken"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| Sensitive::new(s.to_owned())))
    }
    fn headers(token: &SecretString, version: &str) -> BTreeMap<String, SecretString> {
        BTreeMap::from([
            (
                "authorization".into(),
                Sensitive::new(format!("Bearer {}", token.expose())),
            ),
            (
                "anthropic-beta".into(),
                Sensitive::new("oauth-2025-04-20".into()),
            ),
            (
                "user-agent".into(),
                Sensitive::new(format!("claude-cli/{version} (external, cli)")),
            ),
        ])
    }
    pub async fn read(&self, config: &Path, version: &str) -> Option<ServerProviderResetCredits> {
        let work = async {
            let token = self.token(config).ok()??;
            let response = self
                .http
                .send(HttpRequest {
                    method: "GET",
                    url: "https://api.anthropic.com/api/oauth/usage?cedar_ember=1&skip_spend=1"
                        .into(),
                    headers: Self::headers(&token, version),
                    body: None,
                })
                .await
                .ok()?;
            if !(200..300).contains(&response.status) {
                return None;
            }
            claude_credits(&response.body.expose()["cedar_ember"], self.clock.now_ms())
        };
        tokio::time::timeout(Duration::from_secs(10), work)
            .await
            .ok()
            .flatten()
    }
    pub async fn consume(
        &self,
        config: &Path,
        account: &Path,
        version: &str,
        grant: &str,
        request: &str,
    ) -> Result<Outcome, ClaudeResetError> {
        if !GRANT.is_match(grant) || !REQUEST.is_match(request) {
            return Err(ClaudeResetError::MalformedCredit);
        }
        let token = self
            .token(config)
            .map_err(|_| ClaudeResetError::LoginUnreadable)?
            .ok_or(ClaudeResetError::SignedOut)?;
        let account = json_file(account).map_err(|_| ClaudeResetError::AccountUnreadable)?;
        let org = account["oauthAccount"]["organizationUuid"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or(ClaudeResetError::SignedOut)?;
        let mut url = url::Url::parse("https://api.anthropic.com")
            .map_err(|_| ClaudeResetError::RequestFailed)?;
        url.path_segments_mut()
            .map_err(|_| ClaudeResetError::RequestFailed)?
            .extend(["api", "organizations", org, "reset_rate_limits"]);
        let mut headers = Self::headers(&token, version);
        headers.insert(
            "content-type".into(),
            Sensitive::new("application/json".into()),
        );
        let response = tokio::time::timeout(
            Duration::from_secs(25),
            self.http.send(HttpRequest {
                method: "POST",
                url: url.into(),
                headers,
                body: Some(Sensitive::new(
                    json!({"program":"cedar_ember","grant_id":grant,"request_id":request})
                        .to_string(),
                )),
            }),
        )
        .await
        .map_err(|_| ClaudeResetError::RequestFailed)?
        .map_err(|_| ClaudeResetError::RequestFailed)?;
        match response.status {
            429 => return Err(ClaudeResetError::RateLimited),
            401 | 403 => return Err(ClaudeResetError::SignedOut),
            200..300 => {}
            _ => return Err(ClaudeResetError::RequestFailed),
        }
        match response.body.expose()["result"].as_str() {
            Some("reset") => Ok(Outcome::Reset),
            Some("not_limited") => Ok(Outcome::NothingToReset),
            Some("already_used") => Ok(Outcome::AlreadyRedeemed),
            Some("ineligible") => Ok(Outcome::NoCredit),
            Some("cooldown") => Err(ClaudeResetError::CoolingDown),
            Some("unavailable") => Err(ClaudeResetError::Unconfirmed),
            _ => Err(ClaudeResetError::RequestFailed),
        }
    }
}
