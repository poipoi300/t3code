mod activity;
mod adapter;
mod checkout;
mod decode;
mod queries;
mod read;
mod write;
use crate::{FailureReason, ForgeError, Result, cli::*, forge::*, model::*, support::*};
use serde_json::{Value, json};
use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};
use tokio::sync::Mutex;
type DetailCache = Arc<Mutex<HashMap<String, (tokio::time::Instant, u64, Value)>>>;

#[derive(Clone)]
pub struct GitLab {
    cli: Arc<dyn ForgeCli>,
    git: t3_git::Git,
    environment: t3_process::Environment,
    configured_hosts: std::collections::BTreeSet<String>,
    capabilities: PullRequestCapabilities,
    details: DetailCache,
    projects: Arc<Mutex<HashMap<String, (tokio::time::Instant, Value)>>>,
    stop: t3_process::CancellationToken,
    tasks: tokio_util::task::TaskTracker,
    generation: Arc<std::sync::atomic::AtomicU64>,
    revision: u64,
}
fn error(operation: &str, detail: &str) -> ForgeError {
    ForgeError::provider("gitlab", operation, FailureReason::Failed, detail)
}
fn parse<T: serde::de::DeserializeOwned>(value: Value, operation: &str) -> Result<T> {
    crate::support::decode("gitlab", operation, value)
}
fn project(repository: &str) -> Result<String> {
    let repository = repository.trim();
    if repository
        .split('/')
        .any(|s| s.is_empty() || s == "." || s == "..")
        || repository.contains(['\0', '\r', '\n'])
    {
        return Err(error("repository", "Invalid GitLab project."));
    }
    Ok(format!("projects/{}", encode(repository)))
}
fn mr(input: &PullRequestInput) -> Result<String> {
    if input.number == 0 || input.number > i32::MAX as u64 {
        return Err(error("reference", "Invalid merge request number."));
    }
    Ok(format!(
        "{}/merge_requests/{}",
        project(&input.repo.repository)?,
        input.number
    ))
}
fn key(input: &PullRequestInput) -> String {
    format!(
        "{}\0{}\0{}\0{}",
        input.repo.cwd,
        input.repo.host.to_lowercase(),
        input.repo.repository,
        input.number
    )
}
impl GitLab {
    pub fn new(git: t3_git::Git) -> Result<Self> {
        Ok(Self {
            cli: Arc::new(ProcessForgeCli),
            git,
            environment: Default::default(),
            configured_hosts: ["gitlab.com".into()].into(),
            capabilities: parse(
                json!({"diff":true,"comment":true,"actions":["merge","ready","draft","close","reopen","delete-branch","update-branch","enable-auto-merge","disable-auto-merge","revert"],"mergeMethods":["merge","squash","rebase"],"deleteBranch":true,"updateMethods":["rebase"],"search":true,"reactions":true,"mentions":true,"referenceSigils":["#","!"],"viewedFiles":"environment","review":{"inlineComment":true,"rangeComment":true,"reply":true,"resolve":true,"verdicts":["comment","approve","request-changes"]},"reviewers":{"request":true,"listCandidates":true,"searchCandidates":true},"labels":true,"edit":{"changeRequest":true,"comment":true,"maintainerEdits":true}}),
                "capabilities",
            )?,
            details: Default::default(),
            projects: Default::default(),
            generation: Default::default(),
            revision: 0,
            stop: t3_process::CancellationToken::new(),
            tasks: {
                let tasks = tokio_util::task::TaskTracker::new();
                tasks.close();
                tasks
            },
        })
    }
    pub fn with_cli(mut self, cli: Arc<dyn ForgeCli>) -> Self {
        self.cli = cli;
        self.details = Default::default();
        self.projects = Default::default();
        self.revision = self.revision.wrapping_add(1);
        self
    }
    pub fn with_environment(mut self, environment: t3_process::Environment) -> Self {
        self.environment = environment;
        self.details = Default::default();
        self.projects = Default::default();
        self.revision = self.revision.wrapping_add(1);
        self
    }
    /// Hosts from server configuration, never from an RPC request or a forge response.
    pub fn with_configured_hosts(
        mut self,
        hosts: impl IntoIterator<Item = String>,
    ) -> Result<Self> {
        for host in hosts {
            let host = host.trim().to_ascii_lowercase();
            if !crate::github::transport::valid_host(&host) {
                return Err(error("configure", "Invalid GitLab host."));
            }
            self.configured_hosts.insert(host);
        }
        self.revision = self.revision.wrapping_add(1);
        Ok(self)
    }
    fn configured_host(&self) -> String {
        self.environment
            .get(std::ffi::OsStr::new("GITLAB_HOST"))
            .cloned()
            .unwrap_or_else(|| std::env::var_os("GITLAB_HOST"))
            .map(|v| v.to_string_lossy().trim().to_ascii_lowercase())
            .unwrap_or_else(|| "gitlab.com".into())
    }
    fn check_host(&self, host: &str, operation: &str) -> Result<String> {
        let host = host.trim().to_ascii_lowercase();
        if !crate::github::transport::valid_host(&host)
            || (!self.configured_hosts.contains(&host) && host != self.configured_host())
        {
            return Err(error(
                operation,
                "The GitLab host is not configured for this operation.",
            ));
        }
        Ok(host)
    }
    async fn execute(
        &self,
        cwd: &Path,
        host: Option<&str>,
        operation: &str,
        args: Vec<String>,
        stdin: Option<Value>,
        max_bytes: usize,
    ) -> Result<CliResponse> {
        let host = self.check_host(host.unwrap_or(&self.configured_host()), operation)?;
        let mut environment = self.environment.clone();
        environment.insert("GITLAB_HOST".into(), Some(host.into()));
        let response = self
            .cli
            .run(CliRequest {
                program: "glab".into(),
                provider: "gitlab",
                operation: operation.into(),
                cwd: cwd.into(),
                args,
                stdin: stdin.map(|v| v.to_string().into_bytes()),
                environment,
                max_bytes,
                cancellation: self.stop.clone(),
                timeout: Duration::from_secs(if max_bytes > 8 * 1024 * 1024 { 60 } else { 30 }),
            })
            .await?;
        if response.code != 0 {
            let stderr = String::from_utf8_lossy(&response.stderr).to_lowercase();
            let reason = if stderr.contains("glab auth login")
                || stderr.contains("invalid_grant")
                || stderr.contains("401")
            {
                FailureReason::Unauthenticated
            } else if stderr.contains("429") || stderr.contains("rate limit") {
                FailureReason::RateLimited
            } else if stderr.contains("404") || stderr.contains("not found") {
                FailureReason::NotFound
            } else if stderr.contains("403") {
                FailureReason::Forbidden
            } else {
                FailureReason::Failed
            };
            return Err(ForgeError::provider(
                "gitlab",
                operation,
                reason,
                "GitLab CLI could not complete the operation.",
            ));
        }
        if response.truncated {
            return Err(error(
                operation,
                "GitLab CLI exceeded the response byte limit.",
            ));
        }
        Ok(response)
    }
    async fn api_raw(
        &self,
        repo: &ProviderRepositoryRef,
        operation: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        let mut args = vec!["api".into(), format!("--method={method}")];
        if body.is_some() {
            args.extend([
                "--input=-".into(),
                "--header=Content-Type: application/json".into(),
            ]);
        }
        args.extend(["--".into(), path.into()]);
        Ok(self
            .execute(
                Path::new(&repo.cwd),
                Some(&repo.host),
                operation,
                args,
                body,
                max_bytes,
            )
            .await?
            .stdout)
    }
    async fn api(
        &self,
        repo: &ProviderRepositoryRef,
        operation: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let bytes = self
            .api_raw(repo, operation, method, path, body, 8 * 1024 * 1024)
            .await?;
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| error(operation, "GitLab returned invalid JSON."))
    }
    async fn graphql(
        &self,
        repo: &ProviderRepositoryRef,
        operation: &str,
        query: &str,
        variables: Value,
    ) -> Result<Value> {
        let value = self
            .api(
                repo,
                operation,
                "POST",
                "graphql",
                Some(json!({"query":query,"variables":variables})),
            )
            .await?;
        if rows(&value["errors"]).next().is_some() {
            return Err(error(
                operation,
                "GitLab could not complete the GraphQL operation.",
            ));
        }
        value
            .get("data")
            .filter(|v| !v.is_null())
            .cloned()
            .ok_or_else(|| error(operation, "GitLab returned no GraphQL data."))
    }
    fn vars(input: &PullRequestInput) -> Value {
        json!({"fullPath":input.repo.repository,"iid":input.number.to_string()})
    }
}
