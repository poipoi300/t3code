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
    pub fn key(&self) -> Result<ThreadPullRequestKey> {
        let (host, repository) = self
            .project
            .canonical_repository_key
            .as_deref()
            .and_then(|key| key.split_once('/'))
            .unwrap_or((&self.project.host, &self.project.repository));
        crate::convert(serde_json::json!({"host":host.to_lowercase(),"repository":repository.to_lowercase(),"number":self.reference.number})).map_err(|_| {
            Error::invalid(
                "resolveRepository",
                "The change request's repository could not be identified.",
            )
        })
    }
}

/// A project from the authoritative list, whether or not its checkout has a forge binding.
#[derive(Debug, Clone)]
pub struct SelectedProject {
    pub id: ProjectId,
    pub title: TrimmedNonEmptyString,
    pub workspace_root: String,
    pub remove_agent_credits_on_merge: bool,
}
impl SelectedProject {
    pub fn of(project: &Project) -> Self {
        Self {
            id: project.id.clone(),
            title: project.title.clone(),
            workspace_root: project.workspace_root.clone(),
            remove_agent_credits_on_merge: project.remove_agent_credits_on_merge,
        }
    }
}

/// Implementations must return current authorized project records, never client-supplied cwd grants.
#[async_trait]
pub trait ProjectSource: Send + Sync {
    async fn projects(&self) -> Result<Vec<Binding>>;
    /// The selected project from the same authoritative list, including one with no recognised
    /// remote (TypeScript's `getShell` in `findByPublicHost`).
    async fn selected_project(&self, id: &ProjectId) -> Result<Option<SelectedProject>> {
        Ok(self
            .projects()
            .await?
            .iter()
            .find(|b| b.project.id == *id)
            .map(|b| SelectedProject::of(&b.project)))
    }
    /// One shared host for public reads that is not tied to a project (the registry's GitHub
    /// driver); used only for hosts its `reads_public_host` accepts.
    fn public_host(&self) -> Option<Arc<dyn Host>> {
        None
    }
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
            let Some(target) = host.as_ref() else {
                return Err(match own {
                    Some(_) => Error::invalid(
                        "resolveRepository",
                        "The change request does not belong to the selected project.",
                    ),
                    None => Error::unsupported("resolveRepository"),
                });
            };
            public_binding(source, own, &reference.project_id, target, repository).await?
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

/// `findByPublicHost`: a public host read goes through the shared public host for any project in
/// the authoritative list, even one with no remote; failing that, through the selected project's
/// own binding when it reads that host publicly. Every other unrouted host is refused.
async fn public_binding(
    source: &dyn ProjectSource,
    own: Option<&Binding>,
    project_id: &ProjectId,
    target: &str,
    repository: &str,
) -> Result<Binding> {
    let bind = |selected: SelectedProject, host: Arc<dyn Host>| Binding {
        project: Project {
            id: selected.id,
            title: selected.title,
            workspace_root: selected.workspace_root,
            host: target.into(),
            repository: repository.into(),
            canonical_repository_key: None,
            remove_agent_credits_on_merge: selected.remove_agent_credits_on_merge,
        },
        host,
    };
    if let Some(public) = source
        .public_host()
        .filter(|public| public.reads_public_host(target))
    {
        let selected = source
            .selected_project(project_id)
            .await
            .map_err(|_| {
                Error::invalid(
                    "resolveRepository",
                    "The selected project could not be read.",
                )
            })?
            .ok_or_else(|| Error::unsupported("resolveRepository"))?;
        return Ok(bind(selected, public));
    }
    match own.filter(|own| own.host.reads_public_host(target)) {
        Some(own) => Ok(bind(SelectedProject::of(&own.project), own.host.clone())),
        None => Err(Error::unsupported("resolveRepository")),
    }
}
