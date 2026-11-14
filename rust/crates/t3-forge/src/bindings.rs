//! Forge work behind the host's authoritative ProjectSource, watch and checkout adapters.
use crate::{
    FailureReason, Forge, ForgeError, Result, forge::*, model::*, registry::ForgeRegistry, remote,
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use t3_contracts::{
    base_schemas::{IsoDateTime, ProjectId, TrimmedNonEmptyString},
    thread_pull_request::{
        ThreadPullRequestKey, ThreadPullRequestLink, ThreadPullRequestLinkSource,
    },
};

#[derive(Clone)]
pub struct ProjectRecord {
    pub id: ProjectId,
    pub title: TrimmedNonEmptyString,
    pub workspace_root: PathBuf,
    pub repository_identity: Option<RepositoryIdentity>,
    pub remove_agent_credits_on_merge: bool,
}
#[derive(Clone)]
pub struct ProjectBinding {
    pub project: ProjectRecord,
    pub forge: Arc<dyn Forge>,
    pub context: CheckoutContext,
    pub host: String,
    pub repository: String,
    pub canonical_repository_key: String,
}
#[derive(Default)]
pub struct WatchTarget {
    pub url: Option<String>,
    pub host: Option<String>,
    pub repository: Option<String>,
    pub number: Option<u64>,
}
pub struct ResolvedWatchTarget {
    pub key: ThreadPullRequestKey,
    pub url: String,
}
pub struct ForgeBindings {
    registry: Arc<ForgeRegistry>,
}
fn failed(operation: &str, detail: &str) -> ForgeError {
    ForgeError::provider("unknown", operation, FailureReason::Failed, detail)
}
pub fn canonical_repository_key(key: &str) -> String {
    let key = key
        .trim()
        .strip_prefix("git:")
        .unwrap_or(key.trim())
        .to_ascii_lowercase();
    remote::azure_canonical_repository_key(&key).unwrap_or(key)
}
fn repository_valid(repository: &str) -> bool {
    repository.split('/').count() >= 2
        && repository.split('/').all(|s| {
            !s.is_empty()
                && !matches!(s, "." | "..")
                && !s.contains(['?', '#', '\\', '\0', '\r', '\n'])
        })
}
fn url_for(
    kind: &str,
    host: &str,
    repository: &str,
    number: u64,
    web: Option<&str>,
) -> Result<String> {
    if !crate::github::transport::valid_host(host)
        || !repository_valid(repository)
        || number == 0
        || number > 9_007_199_254_740_991
    {
        return Err(failed("resolveTarget", "Invalid pull request target."));
    }
    let suffix = match kind {
        "github" => "pull",
        "gitlab" => "-/merge_requests",
        "bitbucket" => "pull-requests",
        "azure-devops" => "pullrequest",
        "gitcafe" | "forgejo" => "pulls",
        "cursor-origin" => "pull",
        _ => {
            return Err(failed(
                "resolveTarget",
                "The host has no known pull request URL shape.",
            ));
        }
    };
    if kind == "cursor-origin" {
        return Ok(format!(
            "https://cursor.com/codebase/{repository}/pull/{number}"
        ));
    }
    if let Some(web) = web {
        let mut url = url::Url::parse(web)
            .map_err(|_| failed("resolveTarget", "Invalid repository web URL."))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.authority() != host
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(failed("resolveTarget", "Invalid repository web authority."));
        }
        url.set_query(None);
        url.set_fragment(None);
        return Ok(format!(
            "{}/{suffix}/{number}",
            url.as_str().trim_end_matches('/')
        ));
    }
    Ok(format!("https://{host}/{repository}/{suffix}/{number}"))
}
impl ForgeBindings {
    pub fn new(registry: Arc<ForgeRegistry>) -> Self {
        Self { registry }
    }
    pub async fn bind(&self, mut project: ProjectRecord) -> Result<Option<ProjectBinding>> {
        if !project.workspace_root.is_absolute() {
            return Err(failed(
                "bindProject",
                "An authorized absolute workspace root is required.",
            ));
        }
        if let Some(identity) = project.repository_identity.as_mut() {
            identity.root_path = Some(TrimmedNonEmptyString(
                project.workspace_root.to_string_lossy().into(),
            ));
        }
        let context = project.repository_identity.as_ref().and_then(|identity| {
            let mut provider = remote::recognize_remote(&identity.locator.remote_url.0)?;
            if let Some(kind) = identity.provider.as_ref().filter(|k| k.0 != "unknown") {
                provider.kind = kind.clone();
            }
            Some(CheckoutContext {
                provider,
                remote_name: identity.locator.remote_name.0.clone(),
                remote_url: identity.locator.remote_url.0.clone(),
                requested_host: None,
            })
        });
        let context = context.filter(|c| c.provider.kind.0 != "unknown");
        let handle = self
            .registry
            .resolve(&project.workspace_root, context)
            .await?;
        let Some(context) = handle.context else {
            return Ok(None);
        };
        if context.provider.kind.0 == "unknown" {
            return Ok(None);
        }
        let mut identity = project.repository_identity.take();
        if handle.forge.supports("refine_repository_identity")
            && let Some(current) = identity.take()
        {
            identity = Some(handle.forge.refine_repository_identity(current).await?);
        }
        let parsed = remote::parse_remote(&context.remote_url)
            .ok_or_else(|| failed("bindProject", "Invalid repository remote."))?;
        let mut host = parsed.host;
        let mut repository = handle
            .forge
            .repository_name_from_remote_url(&context.remote_url)
            .ok_or_else(|| failed("bindProject", "Missing repository name."))?;
        if handle.forge.supports("resolve_remote_web_host")
            && let Some(serving) = handle
                .forge
                .resolve_remote_web_host(CheckoutInput {
                    cwd: project.workspace_root.clone(),
                    context: Some(context.clone()),
                })
                .await?
        {
            host = serving;
        }
        let mut raw_key = identity
            .as_ref()
            .map(|i| i.canonical_key.0.clone())
            .unwrap_or_else(|| format!("{host}/{repository}"));
        if handle.forge.kind().0 == "forgejo"
            && let Some(web) = identity.as_ref().and_then(|i| i.web_url.as_ref())
        {
            let web = url::Url::parse(&web.0)
                .map_err(|_| failed("bindProject", "Invalid repository web URL."))?;
            if !matches!(web.scheme(), "http" | "https")
                || !web.username().is_empty()
                || web.password().is_some()
            {
                return Err(failed("bindProject", "Invalid repository web authority."));
            }
            host = web.authority().into();
            raw_key = format!("{}/{}", host, web.path().trim_matches('/'));
        }
        let canonical = canonical_repository_key(&raw_key);
        if handle.forge.kind().0 == "azure-devops" {
            if let Some((authority, _)) = canonical.split_once('/') {
                host = authority.into();
            }
            if let Some(selector) = identity
                .as_ref()
                .and_then(remote::azure_repository_selector)
            {
                repository = selector;
            } else {
                repository = repository.rsplit('/').next().unwrap_or(&repository).into();
            }
        }
        project.repository_identity = identity;
        Ok(Some(ProjectBinding {
            project,
            forge: handle.forge,
            context,
            host,
            repository,
            canonical_repository_key: canonical,
        }))
    }
    pub async fn account(&self, binding: &ProjectBinding, expected: &str) -> Result<VerifiedForge> {
        let verified = binding
            .forge
            .with_verified_credential(VerifiedCredentialInput {
                cwd: binding.project.workspace_root.clone(),
                host: binding.host.clone(),
                repository: Some(binding.repository.clone()),
            })
            .await?;
        if verified.identity.account_id != expected {
            return Err(ForgeError::provider(
                &binding.forge.kind().0,
                "routeIdentity",
                FailureReason::Forbidden,
                "The signed-in account could not be verified before starting the operation.",
            ));
        }
        Ok(verified)
    }
    pub fn resolve_watch_target(
        &self,
        binding: &ProjectBinding,
        target: WatchTarget,
    ) -> Result<ResolvedWatchTarget> {
        if let Some(value) = target.url {
            let link = remote::parse_pull_request_url(&value).ok_or_else(|| {
                failed(
                    "resolveTarget",
                    "This is not a recognized pull request URL.",
                )
            })?;
            let host = if link.provider == "cursor-origin" {
                link.host
            } else {
                link.authority
            };
            let key = canonical_repository_key(&format!("{host}/{}", link.repository));
            let (host, repository) = key
                .split_once('/')
                .ok_or_else(|| failed("resolveTarget", "Invalid pull request key."))?;
            let url = if link.provider == "gitcafe" {
                url_for("gitcafe", host, repository, link.number, None)?
            } else {
                value
            };
            return Ok(ResolvedWatchTarget {
                key: crate::support::decode(
                    "unknown",
                    "resolveTarget",
                    json!({"host":host,"repository":repository,"number":link.number}),
                )?,
                url,
            });
        }
        let repository = target
            .repository
            .ok_or_else(|| failed("resolveTarget", "Pass a URL, or repository and number."))?
            .trim()
            .to_ascii_lowercase();
        let number = target
            .number
            .ok_or_else(|| failed("resolveTarget", "Pass a URL, or repository and number."))?;
        let host = target
            .host
            .unwrap_or_else(|| binding.host.clone())
            .trim()
            .to_ascii_lowercase();
        let provider = remote::recognize_remote(&format!("https://{host}/{repository}"))
            .filter(|p| p.kind.0 != "unknown")
            .map(|p| p.kind)
            .unwrap_or_else(|| binding.forge.kind());
        let own = canonical_repository_key(&format!("{host}/{repository}"))
            == binding.canonical_repository_key
            || host.eq_ignore_ascii_case(&binding.host)
                && repository.eq_ignore_ascii_case(&binding.repository);
        let web = own
            .then(|| {
                binding
                    .project
                    .repository_identity
                    .as_ref()?
                    .web_url
                    .as_ref()
                    .map(|s| s.0.as_str())
            })
            .flatten();
        let key = if own {
            binding.canonical_repository_key.clone()
        } else {
            canonical_repository_key(&format!("{host}/{repository}"))
        };
        let (_, url_repository) = key
            .split_once('/')
            .ok_or_else(|| failed("resolveTarget", "Invalid target key."))?;
        let web = web.filter(|url| {
            url::Url::parse(url)
                .ok()
                .is_some_and(|url| url.authority() == host)
        });
        let url = url_for(&provider.0, &host, url_repository, number, web)?;
        let (host, repository) = key
            .split_once('/')
            .ok_or_else(|| failed("resolveTarget", "Invalid target key."))?;
        Ok(ResolvedWatchTarget {
            key: crate::support::decode(
                "unknown",
                "resolveTarget",
                json!({"host":host,"repository":repository,"number":number}),
            )?,
            url,
        })
    }
    pub async fn discover(
        &self,
        binding: &ProjectBinding,
        branch: String,
    ) -> Result<Vec<ChangeRequest>> {
        binding
            .forge
            .checkout_list_change_requests(CheckoutListInput {
                checkout: CheckoutInput {
                    cwd: binding.project.workspace_root.clone(),
                    context: Some(binding.context.clone()),
                },
                source: None,
                head_selector: branch,
                state: None,
                limit: Some(100),
            })
            .await
    }
    pub async fn discover_links(
        &self,
        binding: &ProjectBinding,
        branch: String,
        at: IsoDateTime,
        source: ThreadPullRequestLinkSource,
    ) -> Result<Vec<ThreadPullRequestLink>> {
        let pulls = self.discover(binding, branch).await?;
        let mut links = vec![];
        for pull in pulls {
            let target = self.resolve_watch_target(
                binding,
                WatchTarget {
                    url: Some(pull.url.clone()),
                    ..Default::default()
                },
            )?;
            links.push(crate::support::decode("unknown","discover",json!({"host":target.key.host,"repository":target.key.repository,"number":pull.number,"url":target.url,"source":source,"linkedAt":at,"snapshot":checkout_snapshot(&pull,&at),"stack":null}))?);
        }
        Ok(links)
    }
    pub async fn refresh_links(
        &self,
        binding: &ProjectBinding,
        links: Vec<ThreadPullRequestLink>,
        at: IsoDateTime,
    ) -> Result<Vec<ThreadPullRequestLink>> {
        let mut results = vec![];
        for mut link in links {
            let key = canonical_repository_key(&format!("{}/{}", link.host.0, link.repository.0));
            if !link.host.0.eq_ignore_ascii_case(&binding.host)
                || (binding.forge.kind().0 == "azure-devops"
                    && key != binding.canonical_repository_key)
            {
                return Err(ForgeError::provider(
                    &binding.forge.kind().0,
                    "refresh",
                    FailureReason::Forbidden,
                    "The linked pull request requires its own authorized project binding.",
                ));
            }
            let pr = PullRequestInput {
                repo: ProviderRepositoryRef {
                    cwd: binding.project.workspace_root.to_string_lossy().into(),
                    host: binding.host.clone(),
                    repository: if key == binding.canonical_repository_key {
                        binding.repository.clone()
                    } else {
                        link.repository.0.clone()
                    },
                },
                number: link.number,
            };
            let summary = if binding.forge.supports("get_change_request_summary") {
                binding.forge.get_change_request_summary(pr).await?
            } else {
                let detail = binding.forge.get_change_request(pr).await?;
                crate::support::decode(
                    "unknown",
                    "refresh",
                    serde_json::to_value(detail)
                        .map_err(|_| failed("refresh", "Invalid pull request summary."))?,
                )?
            };
            let mut value = serde_json::to_value(summary)
                .map_err(|_| failed("refresh", "Invalid pull request summary."))?;
            value["syncedAt"] = json!(at);
            value["isDraft"] = json!(value["isDraft"].as_bool().unwrap_or(false));
            link.snapshot = Some(crate::support::decode("unknown", "refresh", value)?);
            results.push(link);
        }
        Ok(results)
    }
}
fn checkout_snapshot(pull: &ChangeRequest, at: &IsoDateTime) -> Value {
    let updated = serde_json::to_value(&pull.updated_at).unwrap_or(Value::Null);
    let mut snapshot = json!({"state":pull.state,"title":pull.title,"headBranch":pull.head_ref_name,"baseBranch":pull.base_ref_name,"isDraft":pull.is_draft.flatten().unwrap_or(false),"updatedAt":updated.get("value").and_then(Value::as_str),"syncedAt":at});
    if let Some(closed) = &pull.closed_at {
        snapshot["closedAt"] = json!(closed);
    }
    if let Some(merged) = &pull.merged_at {
        snapshot["mergedAt"] = json!(merged);
    }
    snapshot
}
