mod adapter;
mod checkout;
mod decode;
mod read;
use crate::{FailureReason, Forge, ForgeError, Result, forge::*, http::*, model::*, support::*};
use reqwest::Method;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};
use tokio::sync::Mutex;
type RevisionCache = Arc<Mutex<HashMap<String, (tokio::time::Instant, ProviderFileRevisions)>>>;
#[derive(Clone)]
pub struct Bitbucket {
    http: ApiHttp,
    git: t3_git::Git,
    capabilities: PullRequestCapabilities,
    custom_host: bool,
    revisions: RevisionCache,
}
fn error(operation: &str, detail: &str) -> ForgeError {
    ForgeError::provider("bitbucket", operation, FailureReason::Failed, detail)
}
fn parse<T: serde::de::DeserializeOwned>(value: Value, operation: &str) -> Result<T> {
    crate::support::decode("bitbucket", operation, value)
}
fn repository(value: &str) -> Result<String> {
    let parts: Vec<_> = value.trim().split('/').collect();
    if parts.len() != 2
        || parts
            .iter()
            .any(|s| s.is_empty() || matches!(*s, "." | "..") || s.contains(['\0', '\n', '\r']))
    {
        return Err(error(
            "repository",
            "A Bitbucket repository must be workspace/repository.",
        ));
    }
    Ok(format!(
        "/repositories/{}/{}",
        encode(parts[0]),
        encode(parts[1])
    ))
}
fn request_path(input: &PullRequestInput) -> Result<String> {
    if input.number == 0 {
        return Err(error("reference", "Invalid pull request number."));
    }
    Ok(format!(
        "{}/pullrequests/{}",
        repository(&input.repo.repository)?,
        input.number
    ))
}
fn recover<T>(result: Result<T>, fallback: T) -> Result<T> {
    match result {
        Ok(v) => Ok(v),
        Err(e)
            if matches!(
                e.reason(),
                Some(
                    FailureReason::Unauthenticated
                        | FailureReason::MissingTool
                        | FailureReason::RateLimited
                )
            ) =>
        {
            Err(e)
        }
        Err(_) => Ok(fallback),
    }
}
fn literal(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
fn permissions(write: bool) -> Value {
    json!({"actions":if write {vec!["merge","close"]}else{vec!["close"]},"comment":true,"resolve":true,"verdicts":["comment","approve","request-changes"],"requestReviewers":true})
}
impl Bitbucket {
    pub fn new(http: ApiHttp, git: t3_git::Git) -> Result<Self> {
        let custom_host =
            http.base_url().as_str().trim_end_matches('/') != "https://api.bitbucket.org/2.0";
        Ok(Self {
            http,
            git,
            custom_host,
            revisions: Default::default(),
            capabilities: parse(
                json!({"diff":true,"comment":true,"actions":["merge","close"],"mergeMethods":["merge","squash","rebase"],"search":true,"reactions":false,"mentions":false,"referenceSigils":[],"review":{"inlineComment":true,"reply":true,"resolve":true,"verdicts":["comment","approve","request-changes"]},"reviewers":{"request":true,"listCandidates":true,"searchCandidates":false},"edit":{"changeRequest":true,"comment":true},"viewedFiles":"environment"}),
                "capabilities",
            )?,
        })
    }
    fn serves(&self, host: &str) -> bool {
        self.custom_host || host == "bitbucket.org" || host.ends_with(".bitbucket.org")
    }
    fn check_host(&self, repo: &ProviderRepositoryRef) -> Result<()> {
        if !self.serves(&repo.host) {
            return Err(error(
                "host",
                "This endpoint serves Bitbucket Cloud repositories only.",
            ));
        }
        Ok(())
    }
    async fn api(
        &self,
        operation: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        self.http.json(operation, method, path, body).await
    }
    async fn raw(&self, input: &PullRequestInput) -> Result<Value> {
        self.check_host(&input.repo)?;
        let raw = self
            .api(
                "getPullRequest",
                Method::GET,
                &format!("{}?fields=%2Bclosed_on", request_path(input)?),
                None,
            )
            .await?;
        decode::listing(&raw)?;
        Ok(raw)
    }
    async fn pages(
        &self,
        operation: &str,
        path: String,
        max_pages: usize,
    ) -> Result<(Vec<Value>, bool)> {
        let mut url = Some(path);
        let mut items = Vec::new();
        let mut visited = std::collections::HashSet::new();
        for _ in 0..max_pages {
            let Some(path) = url.take() else {
                break;
            };
            if !visited.insert(path.clone()) {
                return Err(error(
                    operation,
                    "The source control host repeated a pagination cursor.",
                ));
            }
            let raw = self.api(operation, Method::GET, &path, None).await?;
            let values = raw["values"]
                .as_array()
                .ok_or_else(|| error(operation, "Invalid paginated response."))?;
            items.extend(values.iter().cloned());
            url = text(&raw, "next").map(str::to_string);
        }
        if url.is_some() && max_pages != 10 {
            return Err(error(operation, "The pagination safety limit was reached."));
        }
        Ok((items, url.is_some()))
    }
    async fn invalidate(&self) {
        self.revisions.lock().await.clear();
    }
}
