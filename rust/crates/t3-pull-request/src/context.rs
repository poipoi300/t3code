use crate::{Error, Result, host::Host};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use t3_contracts::{
    base_schemas::{ProjectId, TrimmedNonEmptyString},
    pull_request::PullRequestRef,
    thread_pull_request::ThreadPullRequestKey,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub title: TrimmedNonEmptyString,
    pub workspace_root: String,
    pub host: String,
    pub repository: String,
    pub canonical_repository_key: Option<String>,
    pub remove_agent_credits_on_merge: bool,
}
#[derive(Clone)]
pub struct Binding {
    pub project: Project,
    pub host: Arc<dyn Host>,
}
#[derive(Clone)]
pub struct Context {
    pub project: Project,
    pub host: Arc<dyn Host>,
    pub reference: PullRequestRef,
    pub credential_namespace: String,
    pub viewer: Option<String>,
    pub(crate) activity: Option<Arc<tokio_util::task::task_tracker::TaskTrackerToken>>,
}
impl Context {
    pub fn scope(&self) -> String {
        serde_json::json!([
            self.host.kind(),
            self.project.host.to_lowercase(),
            self.project.repository.to_lowercase(),
            self.reference.number
        ])
        .to_string()
    }
    pub fn cache_key(&self) -> String {
        serde_json::json!([
            self.scope(),
            self.project.id,
            self.project.workspace_root,
            self.host.revision(),
            self.credential_namespace
        ])
        .to_string()
    }
    pub fn key(&self) -> ThreadPullRequestKey {
        let (host, repository) = self
            .project
            .canonical_repository_key
            .as_deref()
            .and_then(|key| key.split_once('/'))
            .unwrap_or((&self.project.host, &self.project.repository));
        crate::convert(serde_json::json!({"host":host.to_lowercase(),"repository":repository.to_lowercase(),"number":self.reference.number})).expect("resolved project has a valid key")
    }
}

/// Implementations must return current authorized project records, never client-supplied cwd grants.
#[async_trait]
pub trait ProjectSource: Send + Sync {
    async fn projects(&self) -> Result<Vec<Binding>>;
    fn canonical_repository_key(&self, key: &str) -> String {
        key.to_lowercase()
    }
}

pub async fn resolve(source: &dyn ProjectSource, reference: &PullRequestRef) -> Result<Context> {
    if reference.number == 0 || reference.number > 9_007_199_254_740_991 {
        return Err(Error::invalid(
            "resolveRepository",
            "Invalid pull request number.",
        ));
    }
    let projects = source.projects().await?;
    let own = projects
        .iter()
        .find(|p| p.project.id == reference.project_id);
    let repository = reference.repository.as_str().trim();
    let host = reference
        .host
        .as_ref()
        .and_then(Option::as_ref)
        .map(|s| s.as_str().trim().to_lowercase());
    let matches = |p: &&Binding| {
        p.project.repository.eq_ignore_ascii_case(repository)
            && host
                .as_ref()
                .is_none_or(|h| *h == p.project.host.to_lowercase())
    };
    let route = own.filter(matches).or_else(|| {
        let host = host.as_ref()?;
        let key = source.canonical_repository_key(&format!("{host}/{}", repository.to_lowercase()));
        projects
            .iter()
            .find(|p| {
                p.project
                    .canonical_repository_key
                    .as_ref()
                    .is_some_and(|k| k.eq_ignore_ascii_case(&key))
            })
            .or_else(|| {
                projects
                    .iter()
                    .filter(|p| {
                        p.project.host.eq_ignore_ascii_case(host)
                            && p.project.canonical_repository_key.is_none()
                    })
                    .find(|p| p.project.repository.eq_ignore_ascii_case(repository))
            })
            .or_else(|| {
                projects.iter().find(|p| {
                    p.project.host.eq_ignore_ascii_case(host)
                        && p.project.canonical_repository_key.is_none()
                })
            })
    });
    let binding = match route {
        Some(p) => p.clone(),
        None => {
            let selected = own.ok_or_else(|| Error::unsupported("resolveRepository"))?;
            let target = host.as_ref().ok_or_else(|| {
                Error::invalid(
                    "resolveRepository",
                    "The change request does not belong to the selected project.",
                )
            })?;
            if !selected.host.reads_public_host(target) {
                return Err(Error::unsupported("resolveRepository"));
            }
            let mut binding = selected.clone();
            binding.project.host = target.clone();
            binding.project.repository = repository.into();
            binding.project.canonical_repository_key = None;
            binding
        }
    };
    let mut project = binding.project;
    if project.canonical_repository_key.is_none() {
        project.repository = repository.into();
    }
    let mut host_api = binding.host;
    let mut viewer = None;
    let credential_namespace = if let Some(expected) = reference
        .expected_account_id
        .as_ref()
        .and_then(Option::as_ref)
    {
        if host
            .as_ref()
            .is_none_or(|h| !h.eq_ignore_ascii_case(&project.host))
        {
            return Err(Error::invalid(
                "routeIdentity",
                "The signed-in account could not be verified before starting the operation.",
            ));
        }
        let verified = host_api.verify(&project).await?;
        if verified.account_id != expected.as_str() {
            return Err(Error::invalid(
                "routeIdentity",
                "The signed-in account could not be verified before starting the operation.",
            ));
        }
        viewer = Some(verified.viewer);
        host_api = verified.host;
        verified.credential_fingerprint
    } else {
        String::new()
    };
    let mut reference = reference.clone();
    reference.project_id = project.id.clone();
    reference.host = Some(Some(project.host.clone().into()));
    reference.repository = project.repository.clone().into();
    Ok(Context {
        project,
        host: host_api,
        reference,
        credential_namespace,
        viewer,
        activity: None,
    })
}
