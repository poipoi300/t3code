//! The only contact surface with `t3-pull-request`; compiled with the `pull-requests` feature.
//!
//! One `PullRequestService` per environment (`pullRequest/PullRequestService.ts`) over the
//! environment's own projects and forge registry: projects come from the project service's
//! authoritative list, never from a client path, and each is bound through the one registry
//! (`ForgeAdapter::registry`). A client-named host is reached only through a project bound to
//! it, or, for public github.com reads, through the registry's shared GitHub driver.
use futures::{StreamExt, TryStreamExt};
use std::sync::{Arc, Mutex};
use t3_contracts::{
    base_schemas::ProjectId,
    pull_request::{PullRequestLinkedThreadsResult, PullRequestRef},
    settings::ServerSettings,
    thread_pull_request::ThreadPullRequestKey,
};
use t3_pull_request::{
    Error, PullRequestService, Result,
    cache::{CacheStore, Clock, FileStore, MemoryStore, PullRequestReadCache, SystemClock},
    context::{Binding, Project, ProjectSource, SelectedProject},
    guide::{GuideGenerator, PreparedGuide, PullRequestReviewGuideService},
    host::Host,
    linked::Records,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub use t3_forge::bindings::ProjectRecord;

/// What the pull-request application needs from the environment's forge registry: a binding
/// per authoritative project, the canonical repository identity, and the shared public host.
#[async_trait::async_trait]
pub trait ProjectBinder: Send + Sync {
    /// `None` when the project's checkout has no recognised forge remote.
    async fn bind(&self, project: ProjectRecord) -> Result<Option<Binding>>;
    fn canonical_repository_key(&self, key: &str) -> String;
    /// The registry's GitHub driver, for public github.com reads from any project.
    fn public_host(&self) -> Option<Arc<dyn Host>>;
}

/// A registered driver and the one `ForgeHost` over it.
type SharedHost = (Arc<dyn t3_forge::Forge>, Arc<dyn Host>);

/// `ProjectBinder` over the one `ForgeRegistry`, sharing one `ForgeHost` per driver.
pub struct ForgeBinder {
    registry: Arc<t3_forge::registry::ForgeRegistry>,
    bindings: t3_forge::bindings::ForgeBindings,
    hosts: Mutex<Vec<SharedHost>>,
}
impl ForgeBinder {
    pub fn new(registry: Arc<t3_forge::registry::ForgeRegistry>) -> Self {
        Self {
            bindings: t3_forge::bindings::ForgeBindings::new(registry.clone()),
            registry,
            hosts: Mutex::new(Vec::new()),
        }
    }
    fn host(&self, forge: Arc<dyn t3_forge::Forge>) -> Arc<dyn Host> {
        let mut hosts = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, host)) = hosts.iter().find(|(known, _)| Arc::ptr_eq(known, &forge)) {
            return host.clone();
        }
        let host: Arc<dyn Host> = Arc::new(t3_pull_request::forge::ForgeHost::new(forge.clone()));
        hosts.push((forge, host.clone()));
        host
    }
}
#[async_trait::async_trait]
impl ProjectBinder for ForgeBinder {
    async fn bind(&self, project: ProjectRecord) -> Result<Option<Binding>> {
        let Some(bound) = self
            .bindings
            .bind(project)
            .await
            .map_err(t3_pull_request::forge::map_error)?
        else {
            return Ok(None);
        };
        Ok(Some(Binding {
            project: Project {
                id: bound.project.id,
                title: bound.project.title,
                workspace_root: bound.project.workspace_root.to_string_lossy().into(),
                host: bound.host,
                repository: bound.repository,
                canonical_repository_key: Some(bound.canonical_repository_key),
                remove_agent_credits_on_merge: bound.project.remove_agent_credits_on_merge,
            },
            host: self.host(bound.forge),
        }))
    }
    fn canonical_repository_key(&self, key: &str) -> String {
        t3_forge::bindings::canonical_repository_key(key)
    }
    fn public_host(&self) -> Option<Arc<dyn Host>> {
        self.registry
            .get("github")
            .ok()
            .map(|forge| self.host(forge))
    }
}

/// TypeScript's `REPOSITORY_CONCURRENCY`.
const BIND_CONCURRENCY: usize = 12;

/// The environment's current projects, re-read on every call, bound through the registry.
struct EnvironmentProjects {
    projects: t3_project::ProjectService,
    settings: Arc<t3_config::settings::SettingsService>,
    binder: Arc<dyn ProjectBinder>,
}
impl EnvironmentProjects {
    async fn settings(&self) -> Result<ServerSettings> {
        self.settings
            .snapshot()
            .await
            .map_err(|_| Error::invalid("listProjects", "The project list could not be read."))
    }
}
/// `resolveProjectSettings(...).removeAgentCreditsOnMerge`: the project's override, else the
/// environment's setting.
fn remove_agent_credits(settings: &ServerSettings, id: &ProjectId) -> bool {
    settings
        .project_settings_overrides
        .get(id)
        .and_then(|overrides| overrides.remove_agent_credits_on_merge)
        .unwrap_or(settings.remove_agent_credits_on_merge)
}
#[async_trait::async_trait]
impl ProjectSource for EnvironmentProjects {
    async fn projects(&self) -> Result<Vec<Binding>> {
        let shells =
            self.projects.list_shells(None).await.map_err(|_| {
                Error::invalid("listProjects", "The project list could not be read.")
            })?;
        let settings = self.settings().await?;
        let bound: Vec<Option<Binding>> = futures::stream::iter(shells)
            .map(|shell| {
                let record = ProjectRecord {
                    remove_agent_credits_on_merge: remove_agent_credits(&settings, &shell.id),
                    id: shell.id,
                    title: shell.title,
                    workspace_root: shell.workspace_root.0.into(),
                    repository_identity: shell.repository_identity.flatten(),
                };
                let id = record.id.clone();
                async move {
                    // One unreadable checkout must not hide every other project's pull requests.
                    match self.binder.bind(record).await {
                        Ok(binding) => Ok::<_, Error>(binding),
                        Err(error) => {
                            tracing::debug!(project = %id.0.as_str(), operation = %error.operation, "pull request binding skipped");
                            Ok(None)
                        }
                    }
                }
            })
            .buffered(BIND_CONCURRENCY)
            .try_collect()
            .await?;
        Ok(bound.into_iter().flatten().collect())
    }
    async fn selected_project(&self, id: &ProjectId) -> Result<Option<SelectedProject>> {
        let unreadable = || {
            Error::invalid(
                "resolveRepository",
                "The selected project could not be read.",
            )
        };
        let Some(shell) = self
            .projects
            .get_shell(id)
            .await
            .map_err(|_| unreadable())?
        else {
            return Ok(None);
        };
        let settings = self.settings().await.map_err(|_| unreadable())?;
        Ok(Some(SelectedProject {
            remove_agent_credits_on_merge: remove_agent_credits(&settings, &shell.id),
            id: shell.id,
            title: shell.title,
            workspace_root: shell.workspace_root.0,
        }))
    }
    fn public_host(&self) -> Option<Arc<dyn Host>> {
        self.binder.public_host()
    }
    fn canonical_repository_key(&self, key: &str) -> String {
        self.binder.canonical_repository_key(key)
    }
}

/// Review-guide generation needs the provider text-generation backend and an isolated checkout
/// service, neither of which exists in this server yet, so `pullRequests.generateReviewGuide`
/// stays unserved and nothing reaches this generator. Reading stored guides needs none of it.
struct GenerationUnavailable;
fn generation_unavailable() -> Error {
    Error::invalid(
        "generateReviewGuide",
        "Review guide generation is not available on this server yet.",
    )
}
#[async_trait::async_trait]
impl GuideGenerator for GenerationUnavailable {
    async fn check_enabled(&self, _: &t3_pull_request::context::Context) -> Result<()> {
        Err(generation_unavailable())
    }
    async fn prepare(
        &self,
        _: &t3_pull_request::context::Context,
        _: t3_contracts::pull_request::PullRequestDetail,
        _: CancellationToken,
    ) -> Result<PreparedGuide> {
        Err(generation_unavailable())
    }
    async fn generate(
        &self,
        _: &PreparedGuide,
        _: CancellationToken,
    ) -> Result<(
        Vec<t3_contracts::pull_request_review_guide::PullRequestReviewGuideStop>,
        t3_contracts::model_selection::ModelSelection,
    )> {
        Err(generation_unavailable())
    }
    async fn cleanup(&self, _: PreparedGuide) {}
}

pub struct PullRequestAdapter {
    pub service: PullRequestService,
    pub guides: PullRequestReviewGuideService,
    records: Arc<dyn Records>,
    maintenance: TaskTracker,
    stop: CancellationToken,
}

/// The binder the composition root starts the adapter with: an injected one (tests), else the
/// environment's forge registry; `None` without a registry.
pub fn binder(
    domains: &crate::services::Domains,
    options: &crate::runtime::StartOptions,
) -> Option<Arc<dyn ProjectBinder>> {
    if let Some(binder) = &options.pull_request_binder {
        return Some(binder.clone());
    }
    let forge = domains.get::<crate::adapters::forge::ForgeAdapter>()?;
    Some(Arc::new(ForgeBinder::new(forge.registry())))
}

impl PullRequestAdapter {
    pub async fn start(
        context: &crate::services::DomainContext<'_>,
        projects: &crate::adapters::project::ProjectAdapter,
        binder: Arc<dyn ProjectBinder>,
        generator: Option<Arc<dyn GuideGenerator>>,
    ) -> crate::Result<Arc<Self>> {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let stop = context.shutdown.child_token();
        let maintenance = TaskTracker::new();
        let store: Arc<dyn CacheStore> = match FileStore::open(
            context.local.config.paths.pull_request_cache(),
        )
        .await
        {
            Ok(store) => {
                let store = Arc::new(store);
                // `PullRequestReadCache`'s hourly prune of expired files.
                let (pruned, clock, stop) = (store.clone(), clock.clone(), stop.clone());
                maintenance.spawn(async move {
                    let mut hourly = tokio::time::interval(std::time::Duration::from_secs(3_600));
                    loop {
                        tokio::select! {
                            () = stop.cancelled() => break,
                            _ = hourly.tick() => {
                                if pruned.prune(clock.now_ms()).await.is_err() {
                                    tracing::debug!("pull request cache prune failed");
                                }
                            }
                        }
                    }
                });
                store
            }
            Err(error) => {
                tracing::warn!(%error, "pull request cache directory unavailable; caching in memory");
                Arc::new(MemoryStore::default())
            }
        };
        maintenance.close();
        let records: Arc<dyn Records> = Arc::new(t3_pull_request::sqlite::SqliteRecords::new(
            context.database.clone(),
        ));
        let source = Arc::new(EnvironmentProjects {
            projects: projects.service().clone(),
            settings: context.local.settings.clone(),
            binder,
        });
        let service = PullRequestService::with_records(
            source,
            clock.clone(),
            Arc::new(PullRequestReadCache::new(store, clock.clone())),
            records.clone(),
        );
        let guides = PullRequestReviewGuideService::new(
            service.clone(),
            generator.unwrap_or_else(|| Arc::new(GenerationUnavailable)),
            records.clone(),
            clock,
        );
        Ok(Arc::new(Self {
            service,
            guides,
            records,
            maintenance,
            stop,
        }))
    }

    /// `pullRequests.linkedThreads` (`ws.ts`, `resolvePullRequestSyncKey`): a reference naming
    /// its host and `owner/name` repository keys itself; otherwise the selected project's
    /// binding supplies the canonical key. Unresolvable references have no linked threads.
    pub async fn linked_threads(
        &self,
        mut reference: PullRequestRef,
    ) -> Result<PullRequestLinkedThreadsResult> {
        let named = reference.host.clone().flatten().and_then(|host| {
            key(
                host.as_str(),
                reference.repository.as_str(),
                reference.number,
            )
        });
        let key = match named {
            Some(key) if reference.repository.as_str().contains('/') => Some(key),
            named => {
                // The linked-thread list is not an account-bound read.
                reference.expected_account_id = None;
                match self.service.context(&reference).await {
                    Ok(context) => context.key().ok(),
                    Err(_) => named,
                }
            }
        };
        let Some(key) = key else {
            return Ok(PullRequestLinkedThreadsResult {
                threads: Vec::new(),
            });
        };
        self.records
            .linked_threads(&key)
            .await
            .map_err(|_| Error::invalid("linkedThreads", "Could not load linked threads."))
    }
}

/// `normalizeThreadPullRequestKey`: trimmed, lower-case host and repository.
fn key(host: &str, repository: &str, number: u64) -> Option<ThreadPullRequestKey> {
    serde_json::from_value(serde_json::json!({
        "host": host.trim().to_lowercase(),
        "repository": repository.trim().to_lowercase(),
        "number": number,
    }))
    .ok()
}

impl crate::services::DomainRuntime for PullRequestAdapter {
    fn name(&self) -> &'static str {
        "pull-requests"
    }
    /// Cancels and joins admitted reads, writes and revalidations, then the cache prune, while
    /// the database and the forge registry are still open.
    fn shutdown(&self) -> futures::future::BoxFuture<'_, crate::Result<()>> {
        Box::pin(async move {
            self.guides.shutdown().await;
            self.service.shutdown().await;
            self.stop.cancel();
            self.maintenance.wait().await;
            Ok(())
        })
    }
}
