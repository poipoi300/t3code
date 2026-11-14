mod adapter;
mod checkout;
pub mod credentials;
mod decode;
mod discovery;
mod list;
mod read;
mod write;
use crate::{FailureReason, Forge, ForgeError, Result, forge::*, http::*, model::*, support::*};
use credentials::GitCafeCredentials;
use reqwest::Method;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};
use tokio::sync::Mutex;
#[derive(Clone)]
pub struct GitCafe {
    http: BTreeMap<String, ApiHttp>,
    credentials: Arc<GitCafeCredentials>,
    git: t3_git::Git,
    cli: Arc<dyn crate::cli::ForgeCli>,
    capabilities: PullRequestCapabilities,
    known_authors: Arc<Mutex<HashMap<String, String>>>,
    line_totals: Arc<Mutex<HashMap<String, Value>>>,
    stats: Arc<Mutex<HashMap<String, Value>>>,
    stop: t3_process::CancellationToken,
}
pub(super) fn valid_host(host: &str) -> bool {
    matches!(
        host.trim().to_ascii_lowercase().as_str(),
        "git.cafe" | "staging.git.cafe"
    )
}
fn error(operation: &str, detail: &str) -> ForgeError {
    ForgeError::provider("gitcafe", operation, FailureReason::Failed, detail)
}
fn parse<T: serde::de::DeserializeOwned>(value: Value, operation: &str) -> Result<T> {
    crate::support::decode("gitcafe", operation, value)
}
fn repository(raw: &str) -> Result<String> {
    let parts: Vec<_> = raw.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|p| {
            p.is_empty()
                || matches!(*p, "." | "..")
                || !p
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        })
    {
        return Err(error(
            "repository",
            "GitCafe repositories are named owner/name.",
        ));
    }
    Ok(format!("/repos/{raw}"))
}
fn pull_path(input: &PullRequestInput) -> Result<String> {
    if !valid_host(&input.repo.host) || input.number == 0 {
        return Err(error(
            "reference",
            "Invalid GitCafe host or pull request number.",
        ));
    }
    Ok(format!(
        "{}/pulls/{}",
        repository(&input.repo.repository)?,
        input.number
    ))
}
fn id(value: &str) -> Result<&str> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(error(
            "identifier",
            "GitCafe identifiers must be opaque slugs.",
        ));
    }
    Ok(value)
}
fn request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn query(params: &[(&str, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{key}={}", encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}
impl GitCafe {
    pub fn new(
        git: t3_git::Git,
        credentials: Arc<GitCafeCredentials>,
        limits: Arc<crate::rate_limit::RateLimits>,
    ) -> Result<Self> {
        let mut http = BTreeMap::new();
        for host in ["git.cafe", "staging.git.cafe"] {
            http.insert(
                host.into(),
                ApiHttp::new(
                    "gitcafe",
                    format!("https://{host}/api")
                        .parse()
                        .map_err(|_| error("configure", "Invalid GitCafe API origin."))?,
                    ApiCredential::Anonymous,
                    limits.clone(),
                )?
                .with_credentials(credentials.clone())
                .with_redirect_prefix("/api/"),
            );
        }
        Ok(Self {
            http,
            credentials,
            git,
            cli: Arc::new(crate::cli::ProcessForgeCli),
            known_authors: Default::default(),
            line_totals: Default::default(),
            stats: Default::default(),
            stop: t3_process::CancellationToken::new(),
            capabilities: parse(
                json!({"diff":true,"comment":true,"actions":["ready","draft","close","reopen","merge","update-branch"],"mergeMethods":["merge","squash","rebase"],"updateMethods":["rebase"],"search":true,"stacks":true,"stackActions":true,"reactions":true,"mentions":true,"referenceSigils":[],"labels":true,"review":{"inlineComment":false,"reply":true,"resolve":true,"verdicts":["approve","request-changes","comment"]},"reviewers":{"request":true,"listCandidates":true,"searchCandidates":false},"edit":{"changeRequest":true,"comment":true}}),
                "capabilities",
            )?,
        })
    }
    pub fn with_cli(mut self, cli: Arc<dyn crate::cli::ForgeCli>) -> Self {
        self.cli = cli;
        self
    }
    pub fn with_loopback_endpoint(mut self, host: &str, endpoint: url::Url) -> Result<Self> {
        if !valid_host(host)
            || !matches!(
                endpoint.host_str(),
                Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
            )
        {
            return Err(error(
                "configure",
                "Test endpoints must use a loopback address.",
            ));
        }
        let source: Arc<dyn ApiCredentials> = Arc::new(HostCredentials {
            host: host.into(),
            source: self.credentials.clone(),
        });
        self.http.insert(
            host.into(),
            ApiHttp::new(
                "gitcafe",
                endpoint,
                ApiCredential::Anonymous,
                Arc::new(crate::rate_limit::RateLimits::default()),
            )?
            .with_credentials(source)
            .with_redirect_prefix("/api/"),
        );
        Ok(self)
    }
    async fn api(
        &self,
        host: &str,
        operation: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let normalized_host = host.trim().to_ascii_lowercase();
        let http = self.http.get(&normalized_host).ok_or_else(|| {
            error(
                operation,
                "GitCafe serves only git.cafe and staging.git.cafe.",
            )
        })?;
        let mutation = method != Method::GET;
        let raw = http.json(operation, method, path, body).await?;
        if mutation {
            match operation {
                "toggleList" | "updateChangeRequest" if raw["version"].as_u64().is_none() => {
                    return Err(error(operation, "Invalid mutation version."));
                }
                "comment"
                | "updateComment"
                | "submitReview"
                | "replyToThread"
                | "setThreadResolution"
                    if text(&raw, "id").is_none_or(|v| id(v).is_err()) =>
                {
                    return Err(error(operation, "Invalid mutation identifier."));
                }
                "createChangeRequest" if raw["number"].as_u64().is_none_or(|n| n == 0) => {
                    return Err(error(operation, "Invalid created pull request number."));
                }
                _ => {}
            }
        }
        Ok(raw)
    }
    async fn raw(&self, input: &PullRequestInput, operation: &str) -> Result<Value> {
        let raw = self
            .api(
                &input.repo.host,
                operation,
                Method::GET,
                &pull_path(input)?,
                None,
            )
            .await?;
        decode::listing(&raw, &input.repo)?;
        if raw["version"].as_u64().is_none() || !raw["capabilities"].is_object() {
            return Err(error(operation, "Invalid versioned GitCafe pull request."));
        }
        Ok(raw)
    }
    async fn pages(
        &self,
        repo: &ProviderRepositoryRef,
        operation: &str,
        path: &str,
    ) -> Result<(Vec<Value>, Option<String>)> {
        let mut items = vec![];
        let mut after = None::<String>;
        for _ in 0..10 {
            let mut params = vec![("limit", "100".into())];
            if let Some(after) = &after {
                params.push(("after", after.clone()));
            }
            let raw = self
                .api(
                    &repo.host,
                    operation,
                    Method::GET,
                    &format!("{path}?{}", query(&params)),
                    None,
                )
                .await?;
            let page = raw["items"]
                .as_array()
                .ok_or_else(|| error(operation, "Invalid paginated GitCafe response."))?;
            items.extend(page.iter().cloned());
            let next = text(&raw, "next").map(str::to_string);
            let stop = next.is_none() || next == after || page.is_empty();
            after = next;
            if stop {
                break;
            }
        }
        Ok((items, after))
    }
    async fn remember(cache: &Mutex<HashMap<String, Value>>, key: String, value: Value) {
        let mut cache = cache.lock().await;
        if cache.len() >= 256 {
            cache.clear();
        }
        cache.insert(key, value);
    }
}
struct HostCredentials {
    host: String,
    source: Arc<GitCafeCredentials>,
}
#[async_trait::async_trait]
impl ApiCredentials for HostCredentials {
    fn revision(&self) -> u64 {
        self.source.revision()
    }
    async fn get(&self, _host: &str) -> Result<ApiCredential> {
        self.source.get(&self.host).await
    }
    async fn invalidate(&self, _host: &str) {
        self.source.invalidate(&self.host).await;
    }
}
