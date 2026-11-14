//! CLIProxyAPI hub client (`apps/server/src/usage/cliproxyApi.ts`): the management API's account
//! list, each account's usage read through the hub's `api-call` relay, and Codex reset-credit
//! redemption. The host supplies the transport ([`HubHttp`]); the hub URL and management key are
//! the saved source's own (`limit_sources::SourceConfig`), never a client's.
use crate::{
    Error,
    limits::{self, LimitSourceProbe},
    service::Clock,
    sources::ProbeFuture,
};
use serde_json::{Map, Value, json};
use sha2::Digest;
use std::{sync::Arc, time::Duration};
use t3_contracts::provider_usage_limits::{
    ProviderConsumeResetCreditResult, UsageLimitSourceAccount,
};

/// TypeScript `Effect.timeout("15 seconds")` on each management request.
pub const HUB_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// The JSON node ceiling applied to every hub and relayed upstream answer.
const MAX_JSON_NODES: usize = 2_000_000;
const CODEX_BASE: &str = "https://chatgpt.com/backend-api/wham";
const GROK_USAGE_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
const GROK_CLI_VERSION: &str = "1.0.77";
const ANTIGRAVITY_SUMMARY_URL: &str =
    "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary";
const GOOGLE_USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v2/userinfo";
const CREDIT_REDEEM_NAMESPACE: [u8; 16] = [
    0x6f, 0x1c, 0x2a, 0x9e, 0x2d, 0x4b, 0x4c, 0x1e, 0x9a, 0x7f, 0x3b, 0x8d, 0x5e, 0x0c, 0x1a, 0x42,
];
// Extra CAs can fix untrusted chains; expiry/hostname failures report codes only.
const UNTRUSTED_CERTIFICATE_CODES: [&str; 4] = [
    "DEPTH_ZERO_SELF_SIGNED_CERT",
    "SELF_SIGNED_CERT_IN_CHAIN",
    "UNABLE_TO_GET_ISSUER_CERT_LOCALLY",
    "UNABLE_TO_VERIFY_LEAF_SIGNATURE",
];
const UNREADABLE_ACCOUNT: &str = "The hub could not read this account's usage.";

fn credit_url() -> String {
    format!("{CODEX_BASE}/rate-limit-reset-credits")
}

/// One management request. The key travels only in `authorization`, which `Debug` omits.
pub struct HubRequest {
    pub method: &'static str,
    pub url: String,
    pub authorization: String,
    /// A JSON body, sent with `content-type: application/json`.
    pub body: Option<Vec<u8>>,
}
impl std::fmt::Debug for HubRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}
pub struct HubResponse {
    pub status: u16,
    pub body: Vec<u8>,
}
/// A transport failure; `code` is a short error code (such as a TLS verification code) when the
/// transport can name one.
#[derive(Debug, Default)]
pub struct HubTransportError {
    pub code: Option<String>,
}
/// The host's bounded outbound HTTP (no ambient proxy, its own body bound). Redirects are not
/// followed.
pub trait HubHttp: Send + Sync {
    fn send(&self, request: HubRequest) -> HubFuture<'_>;
}
pub type HubFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<HubResponse, HubTransportError>> + Send + 'a>,
>;

/// A failure: a `UsageLimitSourceError` detail, or an answer that did not decode.
#[derive(Debug)]
enum Failure {
    Source(String),
    Schema,
}
type Outcome<T> = Result<T, Failure>;
fn source(detail: impl Into<String>) -> Failure {
    Failure::Source(detail.into())
}

/// TypeScript `describeManagementFailure`: only the failure kind, status or short code, never a
/// body, which can hold credentials.
fn describe_transport(error: &HubTransportError) -> String {
    match error
        .code
        .as_deref()
        .filter(|code| (1..=64).contains(&code.len()))
        .filter(|code| code.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'))
    {
        Some(code) if UNTRUSTED_CERTIFICATE_CODES.contains(&code) => format!(
            "The hub's TLS certificate is not trusted ({code}). If it comes from a private CA, add that CA to NODE_EXTRA_CA_CERTS."
        ),
        Some(code) => format!("The hub request failed ({code})."),
        None => "The hub management request failed.".into(),
    }
}

/// `new URL("v0/management/<path>", base)` after dropping a trailing `management.html` and slash.
fn management_url(hub: &str, path: &str) -> Outcome<String> {
    let invalid = || source("The hub URL is not valid.");
    let mut base = url::Url::parse(hub).map_err(|_| invalid())?;
    let pathname = base.path().to_owned();
    let trimmed = pathname.strip_suffix('/').unwrap_or(&pathname);
    let trimmed = if trimmed.len() >= 16
        && trimmed.is_char_boundary(trimmed.len() - 16)
        && trimmed[trimmed.len() - 16..].eq_ignore_ascii_case("/management.html")
    {
        &trimmed[..trimmed.len() - 16]
    } else {
        trimmed
    };
    base.set_path(&format!("{trimmed}/"));
    let url = base
        .join(&format!("v0/management/{path}"))
        .map_err(|_| invalid())?;
    Ok(url.to_string())
}

fn parse_json(bytes: &[u8]) -> Option<Value> {
    crate::safety::json_shape_ok(bytes, MAX_JSON_NODES)
        .then(|| serde_json::from_slice(bytes).ok())
        .flatten()
}

// Effect Schema struct checks: an optional field may be absent but not null or mistyped.
fn optional<'a>(object: &'a Map<String, Value>, key: &str, ok: fn(&Value) -> bool) -> Outcome<Option<&'a Value>> {
    match object.get(key) {
        None => Ok(None),
        Some(value) if ok(value) => Ok(Some(value)),
        Some(_) => Err(Failure::Schema),
    }
}
fn required<'a>(object: &'a Map<String, Value>, key: &str, ok: fn(&Value) -> bool) -> Outcome<&'a Value> {
    optional(object, key, ok)?.ok_or(Failure::Schema)
}
fn object(value: &Value) -> Outcome<&Map<String, Value>> {
    value.as_object().ok_or(Failure::Schema)
}
fn is_string(v: &Value) -> bool {
    v.is_string()
}
fn is_number(v: &Value) -> bool {
    v.is_number()
}
fn is_bool(v: &Value) -> bool {
    v.is_boolean()
}
fn is_object(v: &Value) -> bool {
    v.is_object()
}
fn nullable_number(v: &Value) -> bool {
    v.is_null() || v.is_number()
}
fn nullable_string(v: &Value) -> bool {
    v.is_null() || v.is_string()
}
fn nullable_object(v: &Value) -> bool {
    v.is_null() || v.is_object()
}

/// TypeScript `AuthFile`.
#[derive(Debug, Clone)]
struct AuthFile {
    id: String,
    auth_index: String,
    provider: String,
    email: Option<String>,
    disabled: bool,
    project_id: Option<String>,
    chatgpt_account_id: Option<String>,
    chatgpt_plan_type: Option<String>,
    plan_type: Option<String>,
}
fn auth_files(value: &Value) -> Outcome<Vec<AuthFile>> {
    let files = required(object(value)?, "files", Value::is_array)?;
    files
        .as_array()
        .into_iter()
        .flatten()
        .map(|file| {
            let file = object(file)?;
            let text = |v: Option<&Value>| v.and_then(Value::as_str).map(str::to_owned);
            let token = optional(file, "id_token", is_object)?.and_then(Value::as_object);
            let token_field = |key: &str| -> Outcome<Option<String>> {
                match token {
                    Some(token) => Ok(text(optional(token, key, is_string)?)),
                    None => Ok(None),
                }
            };
            Ok(AuthFile {
                id: text(Some(required(file, "id", is_string)?)).unwrap_or_default(),
                auth_index: text(Some(required(file, "auth_index", is_string)?)).unwrap_or_default(),
                provider: text(Some(required(file, "provider", is_string)?)).unwrap_or_default(),
                email: text(optional(file, "email", is_string)?),
                disabled: optional(file, "disabled", is_bool)?.and_then(Value::as_bool) == Some(true),
                project_id: text(optional(file, "project_id", is_string)?),
                chatgpt_account_id: token_field("chatgpt_account_id")?,
                chatgpt_plan_type: token_field("chatgpt_plan_type")?,
                plan_type: token_field("plan_type")?,
            })
        })
        .collect()
}

/// TypeScript `listedIdentity`.
fn listed_identity(account: &AuthFile) -> Map<String, Value> {
    let mut identity = Map::new();
    if let Some(email) = account.email.as_deref().filter(|e| !e.is_empty()) {
        identity.insert("email".into(), json!(email));
    }
    if let Some(workspace) = account
        .chatgpt_account_id
        .as_deref()
        .map(t3_contracts::wire::trim_js)
        .filter(|w| !w.is_empty())
    {
        identity.insert("workspaceId".into(), json!(workspace));
    }
    identity
}

/// TypeScript `codexPlanLabel` (`apps/server/src/provider/CodexProvider.ts`).
pub fn codex_plan_label(plan_type: Option<&str>) -> Option<&'static str> {
    Some(match plan_type? {
        "free" => "ChatGPT Free Subscription",
        "go" => "ChatGPT Go Subscription",
        "plus" => "ChatGPT Plus Subscription",
        "pro" => "ChatGPT Pro 200 Subscription",
        "prolite" => "ChatGPT Pro 100 Subscription",
        "promax" => "ChatGPT Pro 500 Subscription",
        "self_serve_business_prolite" => "ChatGPT Business Premium Subscription",
        "team" | "self_serve_business_usage_based" | "business" => "ChatGPT Business Subscription",
        "ent26" | "enterprise_cbp_automation" | "enterprise_cbp_usage_based" | "enterprise" => {
            "ChatGPT Enterprise Subscription"
        }
        "edu" | "edu_plus" | "edu_pro" => "ChatGPT Edu Subscription",
        "unknown" => "ChatGPT Subscription",
        _ => return None,
    })
}

/// TypeScript `creditRedeemRequestId`: UUIDv5 per account and credit, so retries (also from
/// another T3 environment) redeem at most once.
pub fn credit_redeem_request_id(account_id: &str, credit_id: &str) -> String {
    uuid::Uuid::new_v5(
        &uuid::Uuid::from_bytes(CREDIT_REDEEM_NAMESPACE),
        format!("{account_id}:{credit_id}").as_bytes(),
    )
    .to_string()
}

#[derive(Clone, Copy, PartialEq)]
enum HubProvider {
    Codex,
    Claude,
    Xai,
    Antigravity,
}
impl HubProvider {
    /// Keyed by the auth-files `provider`; accounts of any other provider are skipped.
    fn of(provider: &str) -> Option<Self> {
        Some(match provider {
            "codex" => Self::Codex,
            "claude" => Self::Claude,
            "xai" => Self::Xai,
            "antigravity" => Self::Antigravity,
            _ => return None,
        })
    }
    fn driver(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claudeAgent",
            Self::Xai => "grok",
            Self::Antigravity => "antigravity",
        }
    }
    fn header(self, account: &AuthFile, platform: &str, architecture: &str) -> Value {
        match self {
            Self::Codex => {
                let mut header = json!({"Authorization":"Bearer $TOKEN$","Content-Type":"application/json","OpenAI-Beta":"codex-1","Originator":"Codex Desktop"});
                if let Some(id) = account.chatgpt_account_id.as_deref().filter(|id| !id.is_empty()) {
                    header["Chatgpt-Account-Id"] = json!(id);
                }
                header
            }
            Self::Claude => json!({"Authorization":"Bearer $TOKEN$","anthropic-beta":"oauth-2025-04-20","User-Agent":"claude-code/2.1.273"}),
            Self::Xai => json!({"Authorization":"Bearer $TOKEN$","x-xai-token-auth":"xai-grok-cli","x-grok-client-version":GROK_CLI_VERSION,"User-Agent":format!("grok-cli/{GROK_CLI_VERSION}")}),
            Self::Antigravity => json!({"Authorization":"Bearer $TOKEN$","Content-Type":"application/json","User-Agent":format!("antigravity/hub/2.24.0 (aidev_client; os_type={platform}; arch={architecture}; cl=963137146)")}),
        }
    }
}

/// The CLIProxyAPI client. `platform` and `architecture` are Node's spellings (`win32`, `x64`).
pub struct CliproxyApi {
    http: Arc<dyn HubHttp>,
    clock: Arc<dyn Clock>,
    platform: String,
    architecture: String,
}

struct Hub<'a> {
    url: &'a str,
    key: &'a str,
}

impl CliproxyApi {
    pub fn new(
        http: Arc<dyn HubHttp>,
        clock: Arc<dyn Clock>,
        platform: impl Into<String>,
        architecture: impl Into<String>,
    ) -> Self {
        Self {
            http,
            clock,
            platform: platform.into(),
            architecture: architecture.into(),
        }
    }

    async fn management(&self, hub: &Hub<'_>, path: &str, body: Option<Value>) -> Outcome<Value> {
        let url = management_url(hub.url, path)?;
        let request = HubRequest {
            method: if body.is_some() { "POST" } else { "GET" },
            url,
            authorization: format!("Bearer {}", hub.key),
            body: body.map(|body| body.to_string().into_bytes()),
        };
        let response = tokio::time::timeout(HUB_REQUEST_TIMEOUT, self.http.send(request))
            .await
            .map_err(|_| source("The hub did not answer within 15 seconds."))?
            .map_err(|error| source(describe_transport(&error)))?;
        if !(200..300).contains(&response.status) {
            return Err(source(format!("The hub answered with HTTP {}.", response.status)));
        }
        parse_json(&response.body).ok_or_else(|| source("The hub management request failed."))
    }

    async fn auth_files(&self, hub: &Hub<'_>) -> Outcome<Vec<AuthFile>> {
        auth_files(&self.management(hub, "auth-files", None).await?)
    }

    /// One upstream request with the account's token, as GET or as a JSON POST of `data`.
    async fn api_call(
        &self,
        hub: &Hub<'_>,
        account: &AuthFile,
        provider: HubProvider,
        url: &str,
        data: Option<Value>,
    ) -> Outcome<String> {
        let mut body = json!({
            "auth_index": account.auth_index,
            "method": if data.is_some() { "POST" } else { "GET" },
            "url": url,
            "header": provider.header(account, &self.platform, &self.architecture),
        });
        if let Some(data) = data {
            body["data"] = json!(data.to_string());
        }
        let raw = self.management(hub, "api-call", Some(body)).await?;
        let raw = object(&raw)?;
        let status = required(raw, "status_code", is_number)?.as_f64().unwrap_or(0.0);
        let text = required(raw, "body", is_string)?.as_str().unwrap_or_default();
        if !(200.0..300.0).contains(&status) {
            return Err(source(format!(
                "The provider refused the hub request (HTTP {}).",
                js_number(status)
            )));
        }
        Ok(text.to_owned())
    }

    fn checked_at(&self) -> String {
        crate::iso(self.clock.now_ms())
    }

    async fn read_account(&self, hub: &Hub<'_>, account: &AuthFile, provider: HubProvider) -> Value {
        let checked_at = self.checked_at();
        let reading = match self.read(hub, account, provider, &checked_at).await {
            Ok(reading) => reading,
            Err(_) => {
                let mut reading = listed_identity(account);
                reading.insert(
                    "usageLimits".into(),
                    json!(
                        limits::unavailable(&checked_at, "probeFailed", Some(UNREADABLE_ACCOUNT))
                            .ok()
                    ),
                );
                reading
            }
        };
        let mut account_value = Map::new();
        account_value.insert("id".into(), json!(account.id));
        account_value.insert("driver".into(), json!(provider.driver()));
        account_value.extend(reading);
        Value::Object(account_value)
    }

    async fn read(
        &self,
        hub: &Hub<'_>,
        account: &AuthFile,
        provider: HubProvider,
        checked_at: &str,
    ) -> Outcome<Map<String, Value>> {
        let mut reading = listed_identity(account);
        match provider {
            HubProvider::Codex => {
                let usage = self
                    .relay(hub, account, provider, &format!("{CODEX_BASE}/usage"), None)
                    .await?;
                let usage = object(&usage)?;
                let plan = optional(usage, "plan_type", is_string)?.and_then(Value::as_str);
                let rate_limit = required(usage, "rate_limit", nullable_object)?;
                let window = |key: &str| -> Outcome<Value> {
                    let Some(rate) = rate_limit.as_object() else {
                        return Ok(Value::Null);
                    };
                    let Some(window) = optional(rate, key, nullable_object)?.and_then(Value::as_object) else {
                        return Ok(Value::Null);
                    };
                    let used = required(window, "used_percent", is_number)?;
                    let reset = optional(window, "reset_at", nullable_number)?;
                    let seconds = optional(window, "limit_window_seconds", is_number)?;
                    let mut out = json!({"usedPercent":used,"resetsAt":reset.cloned().unwrap_or(Value::Null)});
                    if let Some(seconds) = seconds.and_then(Value::as_f64) {
                        out["windowDurationMins"] = js_value(seconds / 60.0);
                    }
                    Ok(out)
                };
                let (primary, secondary) = (window("primary_window")?, window("secondary_window")?);
                let available = self.credits(hub, account).await.ok();
                let plan_type = plan
                    .map(str::to_owned)
                    .or_else(|| account.plan_type.clone())
                    .or_else(|| account.chatgpt_plan_type.clone());
                if let Some(label) = codex_plan_label(plan_type.as_deref()) {
                    reading.insert("plan".into(), json!(label));
                }
                let snapshot = json!({"planType":plan_type,"primary":primary,"secondary":secondary});
                let mut usage_limits = serde_json::to_value(
                    limits::codex_limits(&snapshot, None, None, checked_at).map_err(|_| Failure::Schema)?,
                )
                .map_err(|_| Failure::Schema)?;
                if let Some(available) = available {
                    let mut credits = json!({"availableCount":available.len()});
                    if let Some((id, expires)) = available.first() {
                        credits["nextCreditId"] = json!(id);
                        credits["nextExpiresAt"] = json!(crate::iso(*expires));
                    }
                    usage_limits["resetCredits"] = credits;
                }
                reading.insert("usageLimits".into(), usage_limits);
            }
            HubProvider::Claude => {
                let usage = self
                    .relay(hub, account, provider, "https://api.anthropic.com/api/oauth/usage", None)
                    .await?;
                let usage = object(&usage)?;
                let claude_window = |key: &str| -> Outcome<Value> {
                    match optional(usage, key, nullable_object)?.and_then(Value::as_object) {
                        None => Ok(Value::Null),
                        Some(window) => {
                            required(window, "utilization", is_number)?;
                            required(window, "resets_at", nullable_string)?;
                            Ok(Value::Object(window.clone()))
                        }
                    }
                };
                let (five_hour, seven_day) = (claude_window("five_hour")?, claude_window("seven_day")?);
                let extra = match optional(usage, "extra_usage", nullable_object)?.and_then(Value::as_object) {
                    None => Value::Null,
                    Some(extra) => {
                        required(extra, "is_enabled", is_bool)?;
                        for key in ["monthly_limit", "used_credits", "utilization"] {
                            required(extra, key, nullable_number)?;
                        }
                        Value::Object(extra.clone())
                    }
                };
                let mut model_scoped = Vec::new();
                for limit in optional(usage, "limits", Value::is_array)?
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let limit = object(limit)?;
                    let kind = required(limit, "kind", is_string)?;
                    let percent = optional(limit, "percent", nullable_number)?;
                    let resets_at = optional(limit, "resets_at", nullable_string)?;
                    let scope = optional(limit, "scope", nullable_object)?.and_then(Value::as_object);
                    let model = match scope {
                        Some(scope) => optional(scope, "model", nullable_object)?.and_then(Value::as_object),
                        None => None,
                    };
                    let name = match model {
                        Some(model) => Some(required(model, "display_name", is_string)?),
                        None => None,
                    };
                    if kind == "weekly_scoped"
                        && let (Some(name), Some(percent)) = (name, percent.filter(|p| p.is_number()))
                    {
                        model_scoped.push(json!({"display_name":name,"utilization":percent,"resets_at":resets_at.cloned().unwrap_or(Value::Null)}));
                    }
                }
                let mut rate_limits = json!({"five_hour":five_hour,"seven_day":seven_day,"model_scoped":model_scoped,"extra_usage":extra});
                if let Some(spend) = usage.get("spend") {
                    rate_limits["spend"] = spend.clone();
                }
                reading.insert("plan".into(), json!("Claude Subscription"));
                let usage_limits = limits::claude_limits(
                    &json!({"rate_limits_available":true,"rate_limits":rate_limits}),
                    checked_at,
                )
                .map_err(|_| Failure::Schema)?;
                reading.insert("usageLimits".into(), serde_json::to_value(usage_limits).map_err(|_| Failure::Schema)?);
            }
            HubProvider::Xai => {
                let usage = self.relay(hub, account, provider, GROK_USAGE_URL, None).await?;
                let usage_object = object(&usage)?;
                if let Some(config) = optional(usage_object, "config", is_object)?.and_then(Value::as_object) {
                    optional(config, "creditUsagePercent", is_number)?;
                    if let Some(period) = optional(config, "currentPeriod", is_object)?.and_then(Value::as_object) {
                        optional(period, "type", is_string)?;
                        optional(period, "end", is_string)?;
                    }
                }
                let usage_limits = limits::grok_limits(&usage, checked_at).map_err(|_| Failure::Schema)?;
                reading.insert("usageLimits".into(), serde_json::to_value(usage_limits).map_err(|_| Failure::Schema)?);
            }
            HubProvider::Antigravity => {
                let Some(project) = account
                    .project_id
                    .as_deref()
                    .map(t3_contracts::wire::trim_js)
                    .filter(|p| !p.is_empty())
                else {
                    return Err(source("No Antigravity project reported."));
                };
                let requested_at = self.clock.now_ms();
                let summary = self
                    .relay(hub, account, provider, ANTIGRAVITY_SUMMARY_URL, Some(json!({"project":project})))
                    .await?;
                object(&summary)?;
                let usage_limits = limits::antigravity_limits(&summary, checked_at, requested_at)
                    .map_err(|_| Failure::Schema)?;
                if usage_limits.windows.is_empty() {
                    return Err(source("No Antigravity quota reported."));
                }
                let mut usage_limits = serde_json::to_value(usage_limits).map_err(|_| Failure::Schema)?;
                // Local Antigravity reports no e-mail, so the Google account fingerprint pools the two.
                let fingerprint = self
                    .relay(hub, account, provider, GOOGLE_USERINFO_URL, None)
                    .await
                    .ok()
                    .and_then(|info| {
                        info.get("id")
                            .and_then(Value::as_str)
                            .filter(|id| !id.is_empty())
                            .map(|id| hex(&sha2::Sha256::digest(format!("google-account\0{id}"))))
                    });
                if let Some(fingerprint) = fingerprint {
                    usage_limits["credentialFingerprint"] = json!(fingerprint);
                    reading = Map::new();
                }
                reading.insert("usageLimits".into(), usage_limits);
            }
        }
        Ok(reading)
    }

    /// One relayed upstream request whose answer is JSON.
    async fn relay(
        &self,
        hub: &Hub<'_>,
        account: &AuthFile,
        provider: HubProvider,
        url: &str,
        data: Option<Value>,
    ) -> Outcome<Value> {
        let body = self.api_call(hub, account, provider, url, data).await?;
        parse_json(body.as_bytes()).ok_or(Failure::Schema)
    }

    /// Available Codex reset credits, earliest expiry first.
    async fn credits(&self, hub: &Hub<'_>, account: &AuthFile) -> Outcome<Vec<(String, f64)>> {
        let response = self
            .relay(hub, account, HubProvider::Codex, &credit_url(), None)
            .await?;
        let now = self.clock.now_ms();
        let mut available = Vec::new();
        for credit in required(object(&response)?, "credits", Value::is_array)?
            .as_array()
            .into_iter()
            .flatten()
        {
            let credit = object(credit)?;
            let field = |key: &str| -> Outcome<&str> {
                Ok(required(credit, key, is_string)?.as_str().unwrap_or_default())
            };
            let (id, status, kind, expires) = (field("id")?, field("status")?, field("reset_type")?, field("expires_at")?);
            if kind == "codex_rate_limits"
                && status == "available"
                && let Some(expires) = crate::timestamp_ms(expires).filter(|at| *at > now)
            {
                available.push((id.to_owned(), expires));
            }
        }
        available.sort_by(|a, b| a.1.total_cmp(&b.1));
        Ok(available)
    }

    async fn read_accounts(&self, hub: &Hub<'_>) -> Outcome<Vec<UsageLimitSourceAccount>> {
        let files = self.auth_files(hub).await.map_err(|error| match error {
            Failure::Source(detail) => Failure::Source(detail),
            Failure::Schema => source("The hub could not list accounts."),
        })?;
        let readable: Vec<(AuthFile, HubProvider)> = files
            .into_iter()
            .filter(|account| !account.disabled)
            .filter_map(|account| HubProvider::of(&account.provider).map(|p| (account, p)))
            .collect();
        // TypeScript reads four accounts at a time; here in groups of four, in listing order.
        let mut values = Vec::with_capacity(readable.len());
        for group in readable.chunks(4) {
            let mut reads = Vec::with_capacity(group.len());
            for (account, provider) in group {
                reads.push(self.read_account(hub, account, *provider));
            }
            values.extend(futures_util::future::join_all(reads).await);
        }
        values
            .into_iter()
            .map(|value| serde_json::from_value(value).map_err(|_| source("The hub could not list accounts.")))
            .collect()
    }

    async fn consume_credit(
        &self,
        hub: &Hub<'_>,
        account_id: &str,
        credit_id: &str,
    ) -> Outcome<ProviderConsumeResetCreditResult> {
        let account = self
            .auth_files(hub)
            .await?
            .into_iter()
            .find(|account| account.id == account_id)
            .filter(|account| !account.disabled && account.provider == "codex")
            .ok_or_else(|| source("The Codex hub account is missing or disabled."))?;
        let redeem = credit_redeem_request_id(
            account.chatgpt_account_id.as_deref().unwrap_or(&account.id),
            credit_id,
        );
        let body = self
            .api_call(
                hub,
                &account,
                HubProvider::Codex,
                &format!("{}/consume", credit_url()),
                Some(json!({"redeem_request_id":redeem,"credit_id":credit_id})),
            )
            .await?;
        let code = parse_json(body.as_bytes())
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|o| o.get("code"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(Failure::Schema)?;
        let outcome = match code.as_str() {
            "reset" => "reset",
            "nothing_to_reset" => "nothingToReset",
            "no_credit" => "noCredit",
            "already_redeemed" => "alreadyRedeemed",
            _ => return Err(Failure::Schema),
        };
        let mut result = json!({"outcome":outcome});
        if matches!(outcome, "reset" | "alreadyRedeemed")
            && self
                .management(hub, "reset-quota", Some(json!({"auth_index":account.auth_index})))
                .await
                .is_err()
        {
            result["warning"] = json!("Credit redeemed, but the hub cooldown could not be cleared. Routing may resume after its cooldown expires.");
        }
        serde_json::from_value(result).map_err(|_| Failure::Schema)
    }
}

/// A JavaScript number's text: integral values print without a fraction.
fn js_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 {
        format!("{}", n as i64)
    } else {
        n.to_string()
    }
}
/// An integral number stays an integer in JSON, as `JSON.stringify` writes it.
fn js_value(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 {
        json!(n as i64)
    } else {
        json!(n)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl LimitSourceProbe for CliproxyApi {
    fn accounts<'a>(&'a self, url: &'a str, key: &'a str) -> ProbeFuture<'a, Vec<UsageLimitSourceAccount>> {
        Box::pin(async move {
            self.read_accounts(&Hub { url, key }).await.map_err(|error| match error {
                Failure::Source(detail) => Error::Probe(detail),
                Failure::Schema => Error::Probe("The hub could not list accounts.".into()),
            })
        })
    }
    fn consume<'a>(
        &'a self,
        url: &'a str,
        key: &'a str,
        account: &'a str,
        credit: &'a str,
    ) -> ProbeFuture<'a, ProviderConsumeResetCreditResult> {
        Box::pin(async move {
            self.consume_credit(&Hub { url, key }, account, credit)
                .await
                .map_err(|error| match error {
                    Failure::Source(detail) => Error::Probe(detail),
                    Failure::Schema => {
                        Error::Probe("The hub returned an unexpected reset-credit response.".into())
                    }
                })
        })
    }
}
