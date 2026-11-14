use crate::{
    FailureReason, ForgeError, Result,
    rate_limit::{RateLimitKey, RateLimits, retry_at_from_header},
};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{Client, Method, redirect::Policy};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex, Semaphore};
tokio::task_local! { static INTERACTIVE: bool; }
pub async fn with_interactive<F: std::future::Future>(future: F) -> F::Output {
    INTERACTIVE.scope(true, future).await
}
fn interactive() -> bool {
    INTERACTIVE.try_with(|value| *value).unwrap_or(false)
}
pub(crate) fn is_interactive() -> bool {
    interactive()
}
pub(super) fn access_hint_for(source: &str) -> Option<String> {
    match source {
        "settings"=>Some("Your saved GitHub token can't see it. Give it access, or change it in Settings → Source Control.".into()),
        "env"=>Some("The server's environment token can't see it. Give it access, or unset it.".into()),
        "gh"=>Some("Your gh login can't see it. Give it access, or save a token in Settings → Source Control.".into()),
        _=>None,
    }
}
type Quotas = Arc<Mutex<HashMap<(String, String, String), Quota>>>;
#[derive(Clone)]
struct Quota {
    limit: f64,
    remaining: f64,
    reset: u64,
}
#[derive(Clone)]
pub struct Credential {
    pub(super) token: String,
    pub fingerprint: String,
    pub source: &'static str,
}
impl Credential {
    pub fn new(host: &str, token: String, source: &'static str) -> Self {
        let fingerprint = format!("{host}:{:x}", Sha256::digest(token.as_bytes()));
        Self {
            token,
            fingerprint,
            source,
        }
    }
}
impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}
#[async_trait]
pub trait Credentials: Send + Sync {
    fn revision(&self) -> u64 {
        0
    }
    async fn get(&self, host: &str, cwd: &Path) -> Result<Credential>;
    async fn saved(&self, _host: &str) -> Result<Option<Credential>> {
        Ok(None)
    }
    async fn invalidate(&self, _host: &str) {}
    async fn settings(&self) -> Option<CredentialSettings> {
        None
    }
}
#[derive(Default, Clone)]
pub struct CredentialSettings {
    pub tokens: BTreeMap<String, String>,
    pub accounts: BTreeMap<String, String>,
    pub disabled_hosts: HashSet<String>,
    pub environment: BTreeMap<String, String>,
}
type CredentialCache = HashMap<String, (u64, Result<Credential>)>;
pub struct GitHubCredentials {
    settings: tokio::sync::RwLock<CredentialSettings>,
    cache: Mutex<CredentialCache>,
    gates: super::KeyedGates,
    cli: Arc<dyn super::cli::GitHubCli>,
    revision: std::sync::atomic::AtomicU64,
}
impl GitHubCredentials {
    pub fn new(settings: CredentialSettings) -> Self {
        Self {
            settings: tokio::sync::RwLock::new(settings),
            cache: Mutex::new(HashMap::new()),
            gates: Default::default(),
            cli: Arc::new(super::cli::ProcessGitHubCli),
            revision: std::sync::atomic::AtomicU64::new(0),
        }
    }
    pub fn with_cli(mut self, cli: Arc<dyn super::cli::GitHubCli>) -> Self {
        self.cli = cli;
        self
    }
    pub async fn update_settings(&self, settings: CredentialSettings) {
        let mut current = self.settings.write().await;
        self.cache.lock().await.clear();
        *current = settings;
        self.revision
            .fetch_add(1, std::sync::atomic::Ordering::Release);
    }
    async fn lookup_cli(
        &self,
        host: &str,
        account: Option<&str>,
        cwd: &Path,
    ) -> Result<Credential> {
        let output = self.cli.run(cwd, &token_args(host, account)).await?;
        if output.code != 0 || output.stdout.trim().is_empty() {
            return Err(error(
                "credential",
                FailureReason::Unauthenticated,
                "No GitHub credential for this host. Run `gh auth login`.",
            ));
        }
        Ok(Credential::new(host, output.stdout.trim().into(), "gh"))
    }
}
fn token_args(host: &str, account: Option<&str>) -> Vec<String> {
    let mut args = vec!["auth".into(), "token".into(), format!("--hostname={host}")];
    if let Some(account) = account {
        args.push(format!("--user={account}"));
    }
    args
}
#[cfg(test)]
mod command_tests {
    #[test]
    fn cli_values_cannot_become_options() {
        assert_eq!(
            super::token_args("github.com", Some("--show-token")),
            vec![
                "auth",
                "token",
                "--hostname=github.com",
                "--user=--show-token"
            ]
        );
    }
}
#[async_trait]
impl Credentials for GitHubCredentials {
    fn revision(&self) -> u64 {
        self.revision.load(std::sync::atomic::Ordering::Acquire)
    }
    async fn settings(&self) -> Option<CredentialSettings> {
        Some(self.settings.read().await.clone())
    }
    async fn saved(&self, host: &str) -> Result<Option<Credential>> {
        let host = host.trim().to_lowercase();
        let settings = self.settings.read().await;
        if !valid_host(&host) || settings.disabled_hosts.contains(&host) {
            return Err(error(
                "credential",
                FailureReason::Unauthenticated,
                "This GitHub host is invalid or disabled.",
            ));
        }
        Ok(settings
            .tokens
            .get(&host)
            .filter(|s| !s.trim().is_empty())
            .map(|s| Credential::new(&host, s.trim().into(), "settings")))
    }
    async fn invalidate(&self, host: &str) {
        let prefix = format!("{}\0", host.trim().to_lowercase());
        self.cache
            .lock()
            .await
            .retain(|key, _| !key.starts_with(&prefix));
    }
    async fn get(&self, raw_host: &str, cwd: &Path) -> Result<Credential> {
        let host = raw_host.trim().to_lowercase();
        if let Some(saved) = self.saved(&host).await? {
            return Ok(saved);
        }
        let settings_guard = self.settings.read().await;
        let revision = self.revision();
        let settings = settings_guard.clone();
        drop(settings_guard);
        let account = settings.accounts.get(&host);
        let key = format!(
            "{host}\0{}\0{revision}",
            account.map(String::as_str).unwrap_or("")
        );
        let gate = {
            let mut gates = self.gates.lock().await;
            gates.retain(|_, weak| weak.strong_count() > 0);
            if let Some(gate) = gates.get(&key).and_then(std::sync::Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(Mutex::new(()));
                gates.insert(key.clone(), Arc::downgrade(&gate));
                gate
            }
        };
        let _gate = gate.lock().await;
        let cache = self.cache.lock().await;
        if let Some((_, value)) = cache.get(&key).filter(|(at, value)| {
            now().saturating_sub(*at) < if value.is_ok() { 300_000 } else { 10_000 }
        }) {
            return value.clone();
        }
        drop(cache);
        let env = &settings.environment;
        let variables: &[&str] = if host == "github.com" || host.ends_with(".ghe.com") {
            &["GH_TOKEN", "GITHUB_TOKEN"]
        } else if env
            .get("GH_HOST")
            .is_some_and(|v| v.trim().eq_ignore_ascii_case(&host))
        {
            &["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"]
        } else {
            &[]
        };
        let token = variables
            .iter()
            .find_map(|name| env.get(*name).filter(|v| !v.trim().is_empty()));
        let result = if let Some(token) = token {
            Ok(Credential::new(&host, token.trim().into(), "env"))
        } else {
            match self
                .lookup_cli(&host, account.map(String::as_str), cwd)
                .await
            {
                Err(error)
                    if account.is_some()
                        && error.reason() == Some(FailureReason::Unauthenticated) =>
                {
                    self.lookup_cli(&host, None, cwd).await
                }
                result => result,
            }
        };
        if result.as_ref().map_or_else(
            |error| {
                matches!(
                    error.reason(),
                    Some(FailureReason::MissingTool | FailureReason::Unauthenticated)
                )
            },
            |_| true,
        ) {
            let mut cache = self.cache.lock().await;
            if cache.len() >= 32 {
                cache.clear();
            }
            cache.insert(key, (now(), result.clone()));
        }
        result
    }
}
pub fn api_urls(host: &str) -> (String, String) {
    let host = host.trim().to_lowercase();
    if host == "github.com" {
        (
            "https://api.github.com".into(),
            "https://api.github.com/graphql".into(),
        )
    } else if host.ends_with(".ghe.com") {
        (
            format!("https://api.{host}"),
            format!("https://api.{host}/graphql"),
        )
    } else {
        (
            format!("https://{host}/api/v3"),
            format!("https://{host}/api/graphql"),
        )
    }
}
pub(crate) fn error(
    operation: &str,
    reason: FailureReason,
    detail: impl Into<String>,
) -> ForgeError {
    ForgeError::provider("github", operation, reason, detail)
}
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis().min(u64::MAX as u128) as u64)
}
#[derive(Clone)]
pub(crate) struct RestResponse {
    pub body: Vec<u8>,
    pub status: u16,
    pub truncated: bool,
    pub headers: BTreeMap<String, String>,
}
#[derive(Clone)]
pub struct GitHubHttp {
    client: Client,
    credentials: Arc<dyn Credentials>,
    limits: Arc<RateLimits>,
    slots: Arc<Semaphore>,
    endpoint: Option<String>,
    quotas: Quotas,
    app: Option<Arc<super::app::AppSignIn>>,
    pinned: bool,
    refused_reviewers: Arc<Mutex<HashMap<String, u64>>>,
    pub(super) batches: super::batch::Batches,
    validators: Arc<Mutex<HashMap<String, (u64, RestResponse)>>>,
}
impl GitHubHttp {
    pub fn new(credentials: Arc<dyn Credentials>, limits: Arc<RateLimits>) -> Result<Self> {
        let client = Client::builder()
            .redirect(Policy::none())
            .build()
            .map_err(|_| {
                error(
                    "http",
                    FailureReason::Failed,
                    "Could not create the HTTP client.",
                )
            })?;
        Ok(Self {
            client,
            credentials,
            limits,
            slots: Arc::new(Semaphore::new(8)),
            endpoint: None,
            quotas: Arc::new(Mutex::new(HashMap::new())),
            app: None,
            pinned: false,
            refused_reviewers: Default::default(),
            batches: Default::default(),
            validators: Default::default(),
        })
    }
    /// An explicit HTTP loopback endpoint for recorded-response integration tests.
    pub fn with_loopback_endpoint(mut self, endpoint: &str) -> Result<Self> {
        validate_loopback_endpoint(endpoint)?;
        let url = url::Url::parse(endpoint)
            .map_err(|_| error("http", FailureReason::Failed, "Invalid fixture endpoint."))?;
        if !matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"))
            || url.scheme() != "http"
        {
            return Err(error(
                "http",
                FailureReason::Failed,
                "Fixture endpoints must be HTTP loopback URLs.",
            ));
        }
        self.endpoint = Some(endpoint.trim_end_matches('/').into());
        Ok(self)
    }
    pub(crate) fn pinned(&self, host: String, credential: Credential) -> Self {
        let host = host.trim().to_lowercase();
        Self {
            client: self.client.clone(),
            credentials: Arc::new(PinnedCredentials { host, credential }),
            limits: self.limits.clone(),
            slots: self.slots.clone(),
            endpoint: self.endpoint.clone(),
            quotas: self.quotas.clone(),
            app: self.app.clone(),
            pinned: true,
            refused_reviewers: self.refused_reviewers.clone(),
            batches: self.batches.clone(),
            validators: self.validators.clone(),
        }
    }
    pub async fn credential(&self, host: &str, cwd: &Path) -> Result<Credential> {
        if self.pinned {
            return self.credentials.get(host, cwd).await;
        }
        if let Some(saved) = self.credentials.saved(host).await? {
            return Ok(saved);
        }
        if let Some(app) = &self.app {
            match app.token(host).await {
                Ok(Some(credential)) => return Ok(credential),
                Err(error) => return self.credentials.get(host, cwd).await.or(Err(error)),
                _ => {}
            }
        }
        self.credentials.get(host, cwd).await
    }
    pub(super) fn with_app(mut self, app: Arc<super::app::AppSignIn>) -> Self {
        self.app = Some(app);
        self
    }
    pub(super) async fn personal_credential(&self, host: &str, cwd: &Path) -> Result<Credential> {
        self.credentials.get(host, cwd).await
    }
    pub(super) fn client(&self) -> &Client {
        &self.client
    }
    pub(super) fn is_pinned(&self) -> bool {
        self.pinned
    }
    pub(super) fn settings_revision(&self) -> u64 {
        self.credentials.revision()
    }
    pub(super) async fn settings(&self) -> Option<CredentialSettings> {
        self.credentials.settings().await
    }
    pub(super) fn fixture_url(&self, url: &url::Url) -> String {
        self.endpoint
            .as_ref()
            .map(|endpoint| {
                format!(
                    "{endpoint}{}{}",
                    url.path(),
                    url.query().map(|q| format!("?{q}")).unwrap_or_default()
                )
            })
            .unwrap_or_else(|| url.to_string())
    }
    pub(crate) async fn rest(
        &self,
        host: &str,
        cwd: &str,
        operation: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<RestResponse> {
        if method == Method::GET
            && matches!(
                operation,
                "listUnfinishedWorkflowRuns" | "listWorkflowRunsRequiringApproval"
            )
        {
            return self.revalidated_rest(host, cwd, operation, path).await;
        }
        self.send(Request {
            host,
            cwd,
            operation,
            method: method.clone(),
            path,
            body,
            graphql: false,
            interactive: interactive() || method != Method::GET,
            accept: "application/vnd.github+json",
            max_bytes: 8 * 1024 * 1024,
            timeout: Duration::from_secs(30),
            if_none_match: None,
        })
        .await
    }
    async fn revalidated_rest(
        &self,
        host: &str,
        cwd: &str,
        operation: &str,
        path: &str,
    ) -> Result<RestResponse> {
        let credential = self.credential(host, Path::new(cwd)).await?;
        let key = format!(
            "{}\0{}\0{}",
            credential.fingerprint,
            host.to_lowercase(),
            path
        );
        let previous = self
            .validators
            .lock()
            .await
            .get(&key)
            .filter(|(at, _)| now().saturating_sub(*at) < 3_600_000)
            .map(|(_, response)| response.clone());
        let mut response = self
            .pinned(host.into(), credential)
            .send(Request {
                host,
                cwd,
                operation,
                method: Method::GET,
                path,
                body: None,
                graphql: false,
                interactive: interactive(),
                accept: "application/vnd.github+json",
                max_bytes: 8 * 1024 * 1024,
                timeout: Duration::from_secs(30),
                if_none_match: previous
                    .as_ref()
                    .and_then(|r| r.headers.get("etag").map(String::as_str)),
            })
            .await?;
        if response.status == 304
            && let Some(previous) = previous
        {
            response = previous;
        }
        let mut validators = self.validators.lock().await;
        if !response.truncated
            && response
                .headers
                .get("etag")
                .is_some_and(|v| !v.trim().is_empty())
            && serde_json::from_slice::<Value>(&response.body).is_ok()
        {
            if validators.len() >= 512 {
                validators.clear();
            }
            validators.insert(key, (now(), response.clone()));
        } else {
            validators.remove(&key);
        }
        Ok(response)
    }
    pub(crate) async fn graphql(
        &self,
        host: &str,
        cwd: &str,
        operation: &str,
        variables: Value,
    ) -> Result<Value> {
        let document = super::queries::document(operation).ok_or_else(|| {
            error(
                operation,
                FailureReason::Failed,
                "Unknown GraphQL operation.",
            )
        })?;
        if matches!(
            operation,
            "PullRequestSummary"
                | "PullRequestStats"
                | "PullRequestWatchFingerprint"
                | "PullRequestStackMembership"
                | "PullRequestHeads"
                | "PullRequestBranchAccess"
                | "PullRequestsByHead"
                | "PullRequestByNumber"
        ) {
            return self.batched_graphql(host, cwd, operation, variables).await;
        }
        self.graphql_document(host, cwd, operation, document, variables)
            .await
    }
    pub(crate) async fn graphql_document(
        &self,
        host: &str,
        cwd: &str,
        operation: &str,
        document: &str,
        variables: Value,
    ) -> Result<Value> {
        let credential = self.credential(host, Path::new(cwd)).await?;
        let key = format!(
            "{}\0{}\0{}\0{}",
            credential.fingerprint, variables["owner"], variables["name"], variables["number"]
        );
        let mut variables = variables;
        if variables.get("requestedReviewers").is_some()
            && self
                .refused_reviewers
                .lock()
                .await
                .get(&key)
                .is_some_and(|at| now().saturating_sub(*at) < 600_000)
        {
            variables["requestedReviewers"] = json!(0);
        }
        let mut value = self
            .graphql_answer(host, cwd, operation, document, variables.clone())
            .await?;
        if variables["requestedReviewers"]
            .as_u64()
            .is_some_and(|n| n > 0)
            && refused_reviewers_only(&value)
        {
            let mut cache = self.refused_reviewers.lock().await;
            if cache.len() >= 512 {
                cache.clear();
            }
            cache.insert(key, now());
            drop(cache);
            variables["requestedReviewers"] = json!(0);
            value = self
                .graphql_answer(host, cwd, operation, document, variables)
                .await?;
        }
        graphql_data(operation, value)
    }
    pub(super) async fn graphql_answer(
        &self,
        host: &str,
        cwd: &str,
        operation: &str,
        document: &str,
        variables: Value,
    ) -> Result<Value> {
        let response = self
            .send(Request {
                host,
                cwd,
                operation,
                method: Method::POST,
                path: "",
                body: Some(
                    json!({"query":document,"operationName":operation,"variables":variables}),
                ),
                graphql: true,
                interactive: interactive()
                    || document.trim_start().starts_with("mutation ")
                    || matches!(
                        operation,
                        "ReviewerCandidates"
                            | "LabelCandidates"
                            | "RepositoryLabels"
                            | "RepositoryAuthors"
                            | "RepositoryReferences"
                            | "ViewerPermissions"
                            | "PullRequestActionState"
                    ),
                accept: "application/json",
                max_bytes: 8 * 1024 * 1024,
                timeout: Duration::from_secs(30),
                if_none_match: None,
            })
            .await?;
        if response.truncated {
            return Err(error(
                operation,
                FailureReason::Failed,
                "GitHub's GraphQL answer exceeded the response byte limit.",
            ));
        }
        let mut value: Value = serde_json::from_slice(&response.body).map_err(|_| {
            error(
                operation,
                FailureReason::Failed,
                "GitHub returned invalid JSON.",
            )
        })?;
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "__t3CredentialSource".into(),
                json!(response.headers.get("x-t3-github-credential-source")),
            );
        }
        Ok(value)
    }
    pub(crate) async fn send(&self, input: Request<'_>) -> Result<RestResponse> {
        let host = input.host.trim().to_lowercase();
        if !valid_host(&host) {
            return Err(error(
                input.operation,
                FailureReason::Failed,
                "Invalid GitHub host.",
            ));
        }
        let mut credential = self.credential(&host, Path::new(input.cwd)).await?;
        for attempt in 0..2 {
            let mut response = self.send_credential(input.clone(), &credential).await?;
            let value: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
            let withheld = value["errors"].as_array().is_some_and(|errors| {
                errors
                    .iter()
                    .any(|e| matches!(e["type"].as_str(), Some("NOT_FOUND" | "FORBIDDEN")))
            });
            let refused = response.status == 404
                || (response.status == 403
                    && String::from_utf8_lossy(&response.body)
                        .to_lowercase()
                        .contains("not accessible by integration"))
                || (input.graphql && withheld);
            if !self.pinned
                && credential.source == "app"
                && attempt == 0
                && refused
                && let Ok(personal) = self.personal_credential(&host, Path::new(input.cwd)).await
            {
                credential = personal;
                continue;
            }
            if response.status == 401 {
                if let Some(app) = &self.app {
                    let _ = app.invalidate(&host, &credential).await;
                }
                self.credentials.invalidate(&host).await;
            }
            let reason = match response.status {
                200..=299 => None,
                304 if input.if_none_match.is_some() => None,
                401 => Some(FailureReason::Unauthenticated),
                404 => Some(FailureReason::NotFound),
                403 => Some(FailureReason::Forbidden),
                _ => Some(FailureReason::Failed),
            };
            if let Some(reason) = reason {
                let mut error = error(input.operation, reason, "GitHub refused the request.");
                if let ForgeError::Provider {
                    status,
                    access_hint,
                    diagnostics,
                    ..
                } = &mut error
                {
                    *status = Some(response.status);
                    if response.status == 404 {
                        *access_hint = access_hint_for(credential.source);
                    }
                    *diagnostics = response_diagnostics(&value).into_boxed_slice();
                }
                return Err(error);
            }
            response.headers.insert(
                "x-t3-github-credential-source".into(),
                credential.source.into(),
            );
            return Ok(response);
        }
        Err(error(
            input.operation,
            FailureReason::Failed,
            "GitHub refused the app and personal credentials.",
        ))
    }
    async fn send_credential(
        &self,
        input: Request<'_>,
        credential: &Credential,
    ) -> Result<RestResponse> {
        let host = input.host.trim().to_lowercase();
        let host_url = url::Url::parse(&format!("https://{host}")).map_err(|_| {
            error(
                input.operation,
                FailureReason::Failed,
                "Invalid GitHub host.",
            )
        })?;
        if host_url.host_str().is_none()
            || host_url.path() != "/"
            || !host_url.username().is_empty()
            || host_url.password().is_some()
            || host_url.query().is_some()
            || host_url.fragment().is_some()
        {
            return Err(error(
                input.operation,
                FailureReason::Failed,
                "Invalid GitHub host.",
            ));
        }
        let key = RateLimitKey {
            provider: "github".into(),
            host: host.clone(),
            credential_scope: credential.fingerprint.clone(),
        };
        let _permit = self
            .slots
            .acquire()
            .await
            .map_err(|_| error(input.operation, FailureReason::Failed, "Request cancelled."))?;
        let resource = if input.graphql { "graphql" } else { "core" };
        let quota_key = (
            host.clone(),
            resource.to_string(),
            credential.fingerprint.clone(),
        );
        if let Some(quota) = self.quotas.lock().await.get(&quota_key).filter(|q| {
            q.reset > now()
                && q.remaining
                    < if input.interactive {
                        1.0
                    } else {
                        q.limit * 0.1
                    }
        }) {
            return Err(ForgeError::Provider {
                provider: crate::SourceControlProviderKind("github".into()),
                operation: input.operation.into(),
                reason: FailureReason::RateLimited,
                detail: "GitHub quota is reserved until the limit resets.".into(),
                retry_at: Some(quota.reset),
                access_hint: None,
                status: None,
                diagnostics: Box::default(),
            });
        }
        let lease = self.limits.check(&key, now(), input.interactive)?;
        let (rest, graphql) = api_urls(&host);
        let url = if let Some(endpoint) = &self.endpoint {
            if input.graphql {
                format!("{endpoint}/graphql")
            } else {
                format!("{endpoint}/{}", input.path.trim_start_matches('/'))
            }
        } else if input.graphql {
            graphql
        } else {
            format!("{rest}/{}", input.path.trim_start_matches('/'))
        };
        let mut request = self
            .client
            .request(input.method, url)
            .header("user-agent", "t3code")
            .header("x-github-api-version", "2022-11-28")
            .header("accept", input.accept)
            .bearer_auth(&credential.token);
        if let Some(body) = input.body {
            request = request.json(&body);
        }
        if let Some(etag) = input.if_none_match {
            request = request.header("if-none-match", etag);
        }
        let exchange = async {
            let response = request.send().await.map_err(|_| {
                error(
                    input.operation,
                    FailureReason::Failed,
                    "GitHub HTTP request failed.",
                )
            })?;
            let status = response.status().as_u16();
            let headers: BTreeMap<String, String> = response
                .headers()
                .iter()
                .filter_map(|(k, v)| Some((k.as_str().into(), v.to_str().ok()?.into())))
                .collect();
            let mut stream = response.bytes_stream();
            let mut body = Vec::new();
            let mut truncated = false;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| {
                    error(
                        input.operation,
                        FailureReason::Failed,
                        "GitHub response could not be read.",
                    )
                })?;
                let keep = chunk.len().min(input.max_bytes.saturating_sub(body.len()));
                body.extend_from_slice(&chunk[..keep]);
                if keep < chunk.len() {
                    truncated = true;
                    break;
                }
            }
            Ok::<_, ForgeError>((
                status,
                RestResponse {
                    status,
                    body,
                    headers,
                    truncated,
                },
            ))
        };
        let (status, response) = tokio::time::timeout(input.timeout, exchange)
            .await
            .map_err(|_| {
                error(
                    input.operation,
                    FailureReason::Failed,
                    "GitHub request timed out.",
                )
            })??;
        if let (Some(resource), Some(limit), Some(remaining), Some(reset)) = (
            response.headers.get("x-ratelimit-resource"),
            response
                .headers
                .get("x-ratelimit-limit")
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v > 0.0),
            response
                .headers
                .get("x-ratelimit-remaining")
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v >= 0.0),
            response
                .headers
                .get("x-ratelimit-reset")
                .and_then(|v| v.parse::<u64>().ok())
                .and_then(|v| v.checked_mul(1000)),
        ) {
            let key = (
                host.clone(),
                resource.trim().to_string(),
                credential.fingerprint.clone(),
            );
            let mut quotas = self.quotas.lock().await;
            if quotas.get(&key).is_none_or(|q| q.reset <= reset) {
                quotas.insert(
                    key,
                    Quota {
                        limit,
                        remaining,
                        reset,
                    },
                );
            }
        }
        let parsed: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
        if is_rate_limited(
            status,
            &response.headers,
            &parsed,
            &String::from_utf8_lossy(&response.body),
        ) {
            let at = response
                .headers
                .get("retry-after")
                .and_then(|v| retry_at_from_header(v, now()))
                .or_else(|| {
                    response
                        .headers
                        .get("x-ratelimit-reset")
                        .and_then(|v| v.parse::<u64>().ok())
                        .and_then(|v| v.checked_mul(1000))
                        .filter(|v| *v > now())
                });
            self.limits.record_limit(&key, lease, at, now());
            return Err(ForgeError::Provider {
                provider: crate::SourceControlProviderKind("github".into()),
                operation: input.operation.into(),
                reason: FailureReason::RateLimited,
                detail: "GitHub API rate limit exceeded. Requests resume when the limit resets."
                    .into(),
                retry_at: at,
                access_hint: None,
                status: Some(status),
                diagnostics: Box::default(),
            });
        }
        self.limits.record_success(&key, lease, now());
        Ok(response)
    }
}
#[derive(Clone)]
pub(crate) struct Request<'a> {
    pub host: &'a str,
    pub cwd: &'a str,
    pub operation: &'a str,
    pub method: Method,
    pub path: &'a str,
    pub body: Option<Value>,
    pub graphql: bool,
    pub interactive: bool,
    pub accept: &'a str,
    pub max_bytes: usize,
    pub timeout: Duration,
    pub if_none_match: Option<&'a str>,
}
pub fn is_rate_limited(
    status: u16,
    headers: &BTreeMap<String, String>,
    value: &Value,
    body: &str,
) -> bool {
    let errors = value["errors"].as_array();
    let exhausted = headers
        .get("x-ratelimit-remaining")
        .is_some_and(|v| v == "0");
    let names_limit = |v: &str| {
        let v = v.to_lowercase();
        v.contains("rate limit") || v.contains("abuse detection mechanism")
    };
    status == 429
        || errors.is_some_and(|es| {
            !es.is_empty()
                && (exhausted
                    || es.iter().any(|e| {
                        e["type"] == "RATE_LIMITED"
                            || e["message"].as_str().is_some_and(|s| {
                                let s = s.to_lowercase();
                                s.contains("rate limit exceeded")
                                    || s.contains("rate limit already exceeded")
                            })
                    }))
        })
        || (status == 403
            && (exhausted
                || names_limit(body)
                || errors.is_some_and(|es| {
                    es.iter()
                        .any(|e| e["message"].as_str().is_some_and(names_limit))
                })))
}

struct PinnedCredentials {
    host: String,
    credential: Credential,
}
#[async_trait]
impl Credentials for PinnedCredentials {
    async fn get(&self, host: &str, _cwd: &Path) -> Result<Credential> {
        if !host.trim().eq_ignore_ascii_case(&self.host) {
            return Err(error(
                "credential",
                FailureReason::Unauthenticated,
                "The verified credential belongs to another host.",
            ));
        }
        Ok(self.credential.clone())
    }
}

pub fn valid_host(host: &str) -> bool {
    static HOST: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)*(?::\d{1,5})?$",
        )
        .expect("static host regex")
    });
    HOST.is_match(host)
}

pub(super) fn validate_loopback_endpoint(endpoint: &str) -> Result<()> {
    let url = url::Url::parse(endpoint)
        .map_err(|_| error("http", FailureReason::Failed, "Invalid fixture endpoint."))?;
    if !matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"))
        || url.scheme() != "http"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(error(
            "http",
            FailureReason::Failed,
            "Fixture endpoints must be HTTP loopback URLs without credentials.",
        ));
    }
    Ok(())
}

fn missing_pull_ref(error: &Value) -> bool {
    error["type"] == "NOT_FOUND"
        && error["path"] == json!(["repository", "pullRequest", "baseRef", "compare"])
}
fn refused_reviewers_only(value: &Value) -> bool {
    value["errors"].as_array().is_some_and(|errors| {
        !errors.is_empty()
            && errors.iter().all(|error| {
                let path = &error["path"];
                missing_pull_ref(error)
                    || error["type"] == "FORBIDDEN"
                        && path.as_array().is_some_and(|p| p.len() == 6)
                        && path[0] == "repository"
                        && path[1] == "pullRequest"
                        && path[2] == "reviewRequests"
                        && path[3] == "nodes"
                        && path[4].as_u64().is_some()
                        && path[5] == "requestedReviewer"
            })
    })
}
pub(super) fn graphql_data(operation: &str, value: Value) -> Result<Value> {
    if let Some(errors) = value["errors"]
        .as_array()
        .filter(|v| !v.is_empty() && !v.iter().all(missing_pull_ref))
    {
        let all_not_found = errors.iter().all(|e| {
            e["type"] == "NOT_FOUND"
                && e.get("path").and_then(Value::as_array).is_none_or(|path| {
                    path.iter().skip(1).all(Value::is_number)
                        || (path.len() == 2
                            && path[0] == "repository"
                            && matches!(
                                path[1].as_str(),
                                Some("pullRequest" | "issueOrPullRequest")
                            ))
                })
        });
        let forbidden = errors.iter().any(|e| e["type"] == "FORBIDDEN")
            && errors
                .iter()
                .all(|e| matches!(e["type"].as_str(), Some("FORBIDDEN" | "NOT_FOUND")));
        let mut failure = error(
            operation,
            if all_not_found {
                FailureReason::NotFound
            } else if forbidden {
                FailureReason::Forbidden
            } else {
                FailureReason::Failed
            },
            "GitHub could not complete the GraphQL operation.",
        );
        if let ForgeError::Provider {
            status,
            access_hint,
            diagnostics,
            ..
        } = &mut failure
        {
            *status = Some(200);
            if all_not_found {
                *access_hint = value["__t3CredentialSource"]
                    .as_str()
                    .and_then(access_hint_for);
            }
            *diagnostics = response_diagnostics(&value).into_boxed_slice();
        }
        return Err(failure);
    }
    value
        .get("data")
        .filter(|v| !v.is_null())
        .cloned()
        .ok_or_else(|| {
            error(
                operation,
                FailureReason::Failed,
                "GitHub returned no GraphQL data.",
            )
        })
}

fn response_diagnostics(value: &Value) -> Vec<Value> {
    fn sanitized(value: &Value) -> Value {
        match value {
            Value::String(text) => json!(crate::remote::transport_safe_error_value(text)),
            Value::Array(values) => json!(values.iter().map(sanitized).collect::<Vec<_>>()),
            Value::Object(values) => Value::Object(
                values
                    .iter()
                    .filter(|(key, _)| {
                        matches!(key.as_str(), "type" | "message" | "path" | "field" | "code")
                    })
                    .map(|(key, value)| (key.clone(), sanitized(value)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    let mut diagnostics = value["errors"]
        .as_array()
        .map(|errors| errors.iter().take(100).map(sanitized).collect::<Vec<_>>())
        .unwrap_or_default();
    if let Some(message) = value["message"].as_str() {
        diagnostics.insert(
            0,
            json!({"message":crate::remote::transport_safe_error_value(message)}),
        );
    }
    diagnostics
}
