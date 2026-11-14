mod adapter;
mod checkout;
mod decode;
mod read;
mod transport;
use crate::{
    FailureReason, Forge, ForgeError, Result, cli::*, forge::*, http::*, model::*, support::*,
};
use reqwest::Method;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct CursorOriginConfig {
    pub authentication_cwd: PathBuf,
    pub environment: t3_process::Environment,
}
#[derive(Clone)]
pub struct CursorOrigin {
    transport: Arc<transport::Transport>,
    git: t3_git::Git,
    capabilities: PullRequestCapabilities,
    cache: Arc<Mutex<HashMap<String, (Instant, Value)>>>,
    avatars: Arc<Mutex<HashMap<String, String>>>,
    avatar_lock: Arc<Mutex<()>>,
    tokens: Arc<Mutex<BTreeMap<String, String>>>,
    generation: Arc<AtomicU64>,
}
fn error(op: &str, detail: &str) -> ForgeError {
    ForgeError::provider("cursor-origin", op, FailureReason::Failed, detail)
}
fn parse<T: serde::de::DeserializeOwned>(value: Value, op: &str) -> Result<T> {
    crate::support::decode("cursor-origin", op, value)
}
fn repo_path(repo: &str) -> String {
    format!(
        "v1/origin/repos/{}",
        repo.split('/').map(encode).collect::<Vec<_>>().join("/")
    )
}
fn pull_path(pr: &PullRequestInput) -> String {
    format!("{}/pulls/{}", repo_path(&pr.repo.repository), pr.number)
}
fn protected(error: &ForgeError, not_found: bool) -> bool {
    matches!(
        error.reason(),
        Some(FailureReason::Unauthenticated | FailureReason::RateLimited)
    ) || (not_found && error.reason() == Some(FailureReason::NotFound))
}
fn recover<T>(result: Result<T>, fallback: T, not_found: bool) -> Result<T> {
    match result {
        Err(e) if !protected(&e, not_found) => Ok(fallback),
        result => result,
    }
}
fn permissions() -> Value {
    json!({"actions":["merge","ready","draft","close","reopen"],"comment":true,"resolve":true,"verdicts":["comment","approve","request-changes"],"requestReviewers":false})
}
impl CursorOrigin {
    pub fn new(git: t3_git::Git, config: CursorOriginConfig) -> Result<Self> {
        Self::with_api_base(
            git,
            config,
            url::Url::parse("https://api.cursor.com/")
                .map_err(|_| error("configure", "Invalid API URL."))?,
        )
    }
    pub fn with_api_base(
        git: t3_git::Git,
        config: CursorOriginConfig,
        base: url::Url,
    ) -> Result<Self> {
        Ok(Self {
            transport: Arc::new(transport::Transport::new(config, base)?),
            git,
            capabilities: parse(
                json!({"diff":true,"viewedFiles":"environment","comment":true,"actions":["merge","ready","draft","close","reopen"],"mergeMethods":["merge","squash"],"search":false,"reactions":false,"review":{"inlineComment":true,"reply":true,"resolve":true,"verdicts":["comment","approve","request-changes"]},"reviewers":{"request":false,"listCandidates":false},"edit":{"changeRequest":true,"comment":true}}),
                "capabilities",
            )?,
            cache: Arc::default(),
            avatars: Arc::default(),
            avatar_lock: Arc::default(),
            tokens: Arc::default(),
            generation: Arc::new(AtomicU64::new(0)),
        })
    }
    pub fn with_cli(mut self, cli: Arc<dyn ForgeCli>) -> Self {
        self.transport = Arc::new(self.transport.with_cli(cli));
        self
    }
    async fn api(
        &self,
        repo: &ProviderRepositoryRef,
        op: &str,
        path: &str,
        method: Method,
        body: Option<Value>,
    ) -> Result<Value> {
        self.transport
            .api(Path::new(&repo.cwd), op, path, method, body)
            .await
    }
    fn key(&self, repo: &ProviderRepositoryRef, path: &str) -> String {
        format!(
            "{}\0{}\0{}\0{}\0{}",
            self.transport.revision(),
            self.generation.load(Ordering::Acquire),
            repo.cwd,
            repo.repository,
            path
        )
    }
    async fn cached_api(
        &self,
        repo: &ProviderRepositoryRef,
        op: &str,
        path: &str,
        ttl: u64,
    ) -> Result<Value> {
        let key = self.key(repo, path);
        if let Some((_, value)) = self
            .cache
            .lock()
            .await
            .get(&key)
            .filter(|(until, _)| *until > Instant::now())
        {
            return Ok(value.clone());
        }
        let value = self.api(repo, op, path, Method::GET, None).await?;
        self.keep(key, value.clone(), ttl).await;
        Ok(value)
    }
    async fn keep(&self, key: String, value: Value, ttl: u64) {
        let mut cache = self.cache.lock().await;
        cache.retain(|_, (until, _)| *until > Instant::now());
        if cache.len() >= 512 {
            cache.clear();
        }
        cache.insert(key, (Instant::now() + Duration::from_secs(ttl), value));
    }
    async fn raw(&self, pr: &PullRequestInput) -> Result<Value> {
        if pr.number == 0 {
            return Err(error("getChangeRequest", "Invalid pull request number."));
        }
        let value = self
            .cached_api(&pr.repo, "getChangeRequest", &pull_path(pr), 1)
            .await?;
        decode::listing(&value, &HashMap::new())?;
        Ok(value)
    }
    async fn write(
        &self,
        pr: &PullRequestInput,
        op: &str,
        path: &str,
        method: Method,
        body: Value,
    ) -> Result<()> {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let result = self
            .api(&pr.repo, op, path, method, Some(body))
            .await
            .map(|_| ());
        self.generation.fetch_add(1, Ordering::AcqRel);
        result
    }
    async fn pages(
        &self,
        repo: &ProviderRepositoryRef,
        op: &str,
        path: &str,
        field: &str,
    ) -> Result<(Vec<Value>, bool)> {
        let mut all = vec![];
        let mut token: Option<String> = None;
        let mut seen = HashSet::new();
        for _ in 0..10 {
            let path = {
                let mut query = url::form_urlencoded::Serializer::new(String::new());
                query.append_pair("pageSize", "100");
                if let Some(token) = &token {
                    query.append_pair("pageToken", token);
                }
                format!("{path}?{}", query.finish())
            };
            let page = self.api(repo, op, &path, Method::GET, None).await?;
            if !page[field].is_null() && !page[field].is_array() {
                return Err(error(op, "Invalid paginated collection."));
            }
            all.extend(rows(&page[field]).cloned());
            let Some(next) = text(&page, "nextPageToken") else {
                return Ok((all, false));
            };
            if !seen.insert(next.to_string()) {
                return Ok((all, true));
            }
            token = Some(next.to_string());
        }
        Ok((all, true))
    }
    async fn read_avatars(
        &self,
        repo: &ProviderRepositoryRef,
        args: Vec<String>,
    ) -> HashMap<String, String> {
        let _guard = self.avatar_lock.lock().await;
        if let Ok(response) = self
            .transport
            .execute(Path::new(&repo.cwd), "readAvatars", args)
            .await
            && let Ok(value) = serde_json::from_slice::<Value>(&response.stdout)
        {
            decode::avatars(&value, &mut *self.avatars.lock().await);
        }
        self.avatars.lock().await.clone()
    }
    async fn avatars_for(
        &self,
        pr: &PullRequestInput,
        actors: &[Value],
        fields: &str,
    ) -> HashMap<String, String> {
        let missing = {
            let cache = self.avatars.lock().await;
            actors
                .iter()
                .filter_map(decode::actor_id)
                .filter(|id| !cache.contains_key(*id))
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        if !missing.is_empty() {
            self.read_avatars(
                &pr.repo,
                vec![
                    "pr".into(),
                    "view".into(),
                    pr.number.to_string(),
                    "-R".into(),
                    pr.repo.repository.clone(),
                    "--json".into(),
                    fields.into(),
                ],
            )
            .await;
            let mut cache = self.avatars.lock().await;
            for id in missing {
                cache.entry(id).or_default();
            }
        }
        self.avatars.lock().await.clone()
    }
}
