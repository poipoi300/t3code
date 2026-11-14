mod adapter;
mod checkout;
mod decode;
mod read;
pub mod transport;
use crate::{FailureReason, Forge, ForgeError, Result, forge::*, model::*, support::*};
use reqwest::Method;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, sync::Arc};
use transport::{ResolveInput, Target, Transport};
#[derive(Clone)]
pub struct Forgejo {
    transport: Arc<Transport>,
    capabilities: PullRequestCapabilities,
    revision: u64,
}
fn error(op: &str, detail: &str) -> ForgeError {
    ForgeError::provider("forgejo", op, FailureReason::Failed, detail)
}
fn parse<T: serde::de::DeserializeOwned>(value: Value, op: &str) -> Result<T> {
    crate::support::decode("forgejo", op, value)
}
fn repo_path(repo: &str) -> String {
    format!(
        "repos/{}",
        repo.split('/').map(encode).collect::<Vec<_>>().join("/")
    )
}
fn pull_path(input: &PullRequestInput) -> String {
    format!(
        "{}/pulls/{}",
        repo_path(&input.repo.repository),
        input.number
    )
}
fn issue_path(input: &PullRequestInput) -> String {
    format!(
        "{}/issues/{}",
        repo_path(&input.repo.repository),
        input.number
    )
}
impl Forgejo {
    pub fn new(git: t3_git::Git, config: transport::ForgejoConfig) -> Result<Self> {
        Ok(Self {
            transport: Arc::new(Transport::new(git, config)),
            revision: 0,
            capabilities: parse(
                json!({"diff":true,"viewedFiles":"environment","comment":true,"actions":["merge","close","reopen","update-branch"],"mergeMethods":["merge","squash","rebase","fast-forward"],"updateMethods":["merge","rebase"],"search":false,"reactions":true,"mentions":true,"referenceSigils":[],"labels":true,"review":{"inlineComment":true,"reply":false,"resolve":false,"verdicts":["comment","approve","request-changes"]},"reviewers":{"request":true,"listCandidates":true,"searchCandidates":false},"edit":{"changeRequest":true,"comment":true}}),
                "capabilities",
            )?,
        })
    }
    pub fn with_cli(mut self, cli: Arc<dyn crate::cli::ForgeCli>) -> Self {
        let mut transport =
            Transport::new(self.transport.git.clone(), self.transport.config.clone());
        transport.cli = cli;
        self.transport = Arc::new(transport);
        self.revision = self.revision.wrapping_add(1);
        self
    }
    async fn target(&self, repo: &ProviderRepositoryRef) -> Result<Target> {
        self.transport
            .resolve(
                ResolveInput {
                    cwd: Path::new(&repo.cwd),
                    context: None,
                    repository: Some(&repo.repository),
                    reference: None,
                    host: (!repo.host.is_empty()).then_some(repo.host.as_str()),
                },
                false,
            )
            .await
    }
    async fn request(
        &self,
        repo: &ProviderRepositoryRef,
        path: &str,
        method: Method,
        body: Option<Value>,
    ) -> Result<crate::cli::CliResponse> {
        let target = if path.trim_start_matches('/') == "user" && method == Method::GET {
            self.transport
                .resolve(
                    ResolveInput {
                        cwd: Path::new(&repo.cwd),
                        context: None,
                        repository: (!repo.repository.is_empty())
                            .then_some(repo.repository.as_str()),
                        reference: None,
                        host: (!repo.host.is_empty()).then_some(repo.host.as_str()),
                    },
                    true,
                )
                .await?
        } else {
            self.target(repo).await?
        };
        let prefix = repo_path(&repo.repository);
        let normalized = if repo.repository != target.repository
            && (path == prefix
                || path.starts_with(&format!("{prefix}/"))
                || path.starts_with(&format!("{prefix}?")))
        {
            format!("{}{}", repo_path(&target.repository), &path[prefix.len()..])
        } else {
            path.into()
        };
        self.transport
            .request(&target, Path::new(&repo.cwd), &normalized, method, body)
            .await
    }
    async fn api(
        &self,
        repo: &ProviderRepositoryRef,
        path: &str,
        method: Method,
        body: Option<Value>,
    ) -> Result<(Value, Option<String>)> {
        let response = self.request(repo, path, method, body).await?;
        if response.truncated {
            return Err(error("api", "Forgejo exceeded the response byte limit."));
        }
        let raw = if response.stdout.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&response.stdout)
                .map_err(|_| error("api", "Invalid Forgejo API response."))?
        };
        let link = String::from_utf8_lossy(&response.stderr)
            .lines()
            .find_map(|s| {
                s.split_once(':')
                    .filter(|(k, _)| k.eq_ignore_ascii_case("link"))
                    .map(|(_, v)| v.trim().to_string())
            });
        Ok((raw, link))
    }
    async fn raw(&self, input: &PullRequestInput) -> Result<Value> {
        if input.number == 0 {
            return Err(error("getPullRequest", "Invalid pull request number."));
        }
        let raw = self
            .api(&input.repo, &pull_path(input), Method::GET, None)
            .await?
            .0;
        decode::listing(&raw)?;
        Ok(raw)
    }
    async fn page(
        &self,
        repo: &ProviderRepositoryRef,
        path: &str,
        limit: usize,
    ) -> Result<(Vec<Value>, bool)> {
        let mut items = vec![];
        for index in 1..=10_000 {
            let (raw, link) = self
                .api(
                    repo,
                    &format!(
                        "{path}{}limit=50&page={index}",
                        if path.contains('?') { "&" } else { "?" }
                    ),
                    Method::GET,
                    None,
                )
                .await?;
            let raw = if raw.is_null() {
                vec![]
            } else {
                raw.as_array()
                    .cloned()
                    .ok_or_else(|| error("page", "Invalid paginated array."))?
            };
            let more = !raw.is_empty() && link.as_ref().is_none_or(|l| has_next(l));
            items.extend(raw);
            if !more {
                return Ok((items, false));
            }
            if items.len() >= limit {
                return Ok((items, true));
            }
        }
        Err(error("page", "The pagination safety limit was reached."))
    }
    async fn viewer(&self, repo: &ProviderRepositoryRef) -> Result<String> {
        let (raw, _) = self.api(repo, "user", Method::GET, None).await?;
        text(&raw, "login")
            .map(str::to_string)
            .ok_or_else(|| error("getViewer", "No Forgejo account was returned."))
    }
}
fn has_next(link: &str) -> bool {
    let lower = link.to_ascii_lowercase();
    lower.contains("rel=\"next\"") || lower.contains("rel=next")
}
