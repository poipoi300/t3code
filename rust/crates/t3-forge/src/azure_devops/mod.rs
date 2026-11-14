mod adapter;
mod decode;
mod patch;
mod read;
use crate::{FailureReason, Forge, ForgeError, Result, cli::*, forge::*, model::*, support::*};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Mutex, Semaphore};
#[derive(Clone)]
struct Location {
    project: String,
    repository: String,
}
#[derive(Clone)]
struct Iteration {
    id: u64,
    head: String,
    base: String,
}
#[derive(Clone)]
struct Change {
    path: String,
    old: String,
    kind: &'static str,
    oid: Option<String>,
}
type Locations = Arc<Mutex<VecDeque<(String, Location)>>>;
#[derive(Clone)]
pub struct AzureDevOps {
    cli: Arc<dyn ForgeCli>,
    environment: t3_process::Environment,
    capabilities: PullRequestCapabilities,
    stop: t3_process::CancellationToken,
    locations: Locations,
    spawns: Arc<Semaphore>,
    revision: u64,
}
fn error(op: &str, detail: &str) -> ForgeError {
    ForgeError::provider("azure-devops", op, FailureReason::Failed, detail)
}
fn parse<T: serde::de::DeserializeOwned>(value: Value, op: &str) -> Result<T> {
    crate::support::decode("azure-devops", op, value)
}
fn value(value: &str) -> Result<&str> {
    if value.starts_with('@') || value.contains('\0') {
        Err(error(
            "arguments",
            "Azure CLI file-indirection values are not allowed here.",
        ))
    } else {
        Ok(value)
    }
}
fn permissions() -> Value {
    json!({"actions":["merge","ready","draft","close","reopen","enable-auto-merge","disable-auto-merge"],"comment":false,"resolve":false,"verdicts":[],"requestReviewers":true})
}
impl AzureDevOps {
    pub fn new() -> Result<Self> {
        Ok(Self {
            cli: Arc::new(ProcessForgeCli),
            environment: Default::default(),
            stop: t3_process::CancellationToken::new(),
            locations: Default::default(),
            spawns: Arc::new(Semaphore::new(8)),
            revision: 0,
            capabilities: parse(
                json!({"diff":true,"comment":false,"actions":["merge","ready","draft","close","reopen","enable-auto-merge","disable-auto-merge"],"mergeMethods":["merge","squash"],"search":false,"reactions":false,"mentions":false,"referenceSigils":[],"review":{"inlineComment":false,"reply":false,"resolve":false,"verdicts":[]},"reviewers":{"request":true,"listCandidates":false,"searchCandidates":false},"edit":{"changeRequest":true,"comment":false},"viewedFiles":"environment"}),
                "capabilities",
            )?,
        })
    }
    pub fn with_cli(mut self, cli: Arc<dyn ForgeCli>) -> Self {
        self.cli = cli;
        self.locations = Default::default();
        self.revision = self.revision.wrapping_add(1);
        self
    }
    pub fn with_environment(mut self, environment: t3_process::Environment) -> Self {
        self.environment = environment;
        self.locations = Default::default();
        self.revision = self.revision.wrapping_add(1);
        self
    }
    async fn execute(
        &self,
        cwd: &Path,
        op: &str,
        args: Vec<String>,
        max_bytes: usize,
    ) -> Result<CliResponse> {
        let response = self
            .cli
            .run(CliRequest {
                program: "az",
                provider: "azure-devops",
                operation: op.into(),
                cwd: cwd.into(),
                args,
                stdin: None,
                environment: self.environment.clone(),
                max_bytes,
                timeout: Duration::from_secs(30),
                cancellation: self.stop.clone(),
            })
            .await?;
        if response.code != 0 {
            let stderr = String::from_utf8_lossy(&response.stderr).to_lowercase();
            let reason = if stderr.contains("az devops login") || stderr.contains("az login") {
                FailureReason::Unauthenticated
            } else if stderr.contains("tf400733")
                || stderr.contains("429")
                || stderr.contains("rate limit")
            {
                FailureReason::RateLimited
            } else if stderr.contains("azure-devops") && stderr.contains("extension") {
                FailureReason::MissingTool
            } else if stderr.contains("pull request")
                && (stderr.contains("not found") || stderr.contains("does not exist"))
            {
                FailureReason::NotFound
            } else {
                FailureReason::Failed
            };
            return Err(ForgeError::provider(
                "azure-devops",
                op,
                reason,
                "Azure CLI could not complete the operation.",
            ));
        }
        if response.truncated {
            return Err(error(op, "Azure CLI exceeded the response byte limit."));
        }
        Ok(response)
    }
    async fn json(
        &self,
        cwd: &Path,
        op: &str,
        mut args: Vec<String>,
        max_bytes: usize,
    ) -> Result<Value> {
        args.extend(["--only-show-errors".into(), "--output=json".into()]);
        let response = self.execute(cwd, op, args, max_bytes).await?;
        if response.stdout.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&response.stdout)
            .map_err(|_| error(op, "Azure CLI returned invalid JSON."))
    }
    async fn raw(&self, input: &PullRequestInput) -> Result<Value> {
        if input.number == 0 {
            return Err(error("getPullRequest", "Invalid pull request number."));
        }
        let raw = self
            .json(
                Path::new(&input.repo.cwd),
                "getPullRequest",
                vec![
                    "repos".into(),
                    "pr".into(),
                    "show".into(),
                    "--detect=true".into(),
                    format!("--id={}", input.number),
                ],
                8 * 1024 * 1024,
            )
            .await?;
        decode::listing(&raw)?;
        Ok(raw)
    }
    async fn invoke(
        &self,
        input: &PullRequestInput,
        location: &Location,
        resource: &str,
        queries: Vec<String>,
        iteration: Option<u64>,
        max_bytes: usize,
    ) -> Result<Value> {
        let mut args = vec![
            "devops".into(),
            "invoke".into(),
            "--detect=true".into(),
            "--area=git".into(),
            format!("--resource={resource}"),
            "--api-version=7.1".into(),
            "--route-parameters".into(),
            format!("project={}", location.project),
            format!("repositoryId={}", location.repository),
        ];
        if resource != "items" {
            args.push(format!("pullRequestId={}", input.number));
        }
        if let Some(iteration) = iteration {
            args.push(format!("iterationId={iteration}"));
        }
        if !queries.is_empty() {
            args.push("--query-parameters".into());
            args.extend(queries);
        }
        self.json(Path::new(&input.repo.cwd), resource, args, max_bytes)
            .await
    }
    async fn location(&self, input: &PullRequestInput) -> Result<Option<Location>> {
        let key = format!("{}:{}", input.repo.cwd, input.number);
        let mut cache = self.locations.lock().await;
        if let Some(index) = cache.iter().position(|(k, _)| k == &key) {
            let item = cache.remove(index).expect("position came from this queue");
            let location = item.1.clone();
            cache.push_back(item);
            return Ok(Some(location));
        }
        drop(cache);
        let raw = self.raw(input).await?;
        let location = decode::location(&raw);
        if let Some(location) = &location {
            let mut cache = self.locations.lock().await;
            if cache.len() >= 128 {
                cache.pop_front();
            }
            cache.push_back((key, location.clone()));
        }
        Ok(location)
    }
}
