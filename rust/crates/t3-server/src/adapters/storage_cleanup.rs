//! StorageCleanup (storageCleanup.ts). The embedding supplies the checkout operations that
//! must be coordinated with its writers. Ownership, settings and liveness come from this server.
use crate::{
    adapters::{orchestration::OrchestrationAdapter, terminal::TerminalAdapter},
    services::{DomainContext, DomainRuntime, Domains},
};
use async_trait::async_trait;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_contracts::{
    base_schemas::{ProjectId, ThreadId},
    orchestration_v2::{OrchestrationV2AppThread, OrchestrationV2ProviderSession},
    orchestration_v2_id_table::IdOrIndex,
    settings::{ServerSettings, StorageCleanupReport, StorageCleanupSettings},
    terminal::TerminalSessionStatus,
};
use t3_host::{
    Error, Result,
    background::SystemClock,
    storage_cleanup::{
        CleanupHost, CleanupSnapshot, CleanupThread, MergedPullRequest, StorageCleanup,
    },
};
use t3_orchestration::{
    api::{ProjectionRecords, ShellQuery},
    ports::checkpoints_forks::checkpoint_service::WorkspaceLocks,
    projections::read,
    stores::StoreDatabase,
};

/// A fresh, checkout-bound PR lookup and exclusion of Git, launch and terminal writers.
/// A successful permit must remain exclusive until dropped, including after a client disconnect.
#[async_trait]
pub trait CleanupWorktrees: Send + Sync {
    async fn permit(&self, cwd: &Path) -> Result<Box<dyn Send + Sync>>;
    async fn branch_pull_request(
        &self,
        cwd: &Path,
        branch: &str,
    ) -> Result<Option<MergedPullRequest>>;
    async fn invalidate_git_status(&self, project: &Path);
}

struct Host {
    db: StoreDatabase,
    settings: Arc<t3_config::settings::SettingsService>,
    runtime: Arc<t3_orchestration::composition::OrchestrationRuntime>,
    terminal: Arc<TerminalAdapter>,
    worktrees: Arc<dyn CleanupWorktrees>,
    default_worktrees: PathBuf,
    home: PathBuf,
}

fn ids(
    db: &t3_persistence::rusqlite::Connection,
    sql: &str,
) -> t3_orchestration::error::Result<BTreeSet<ThreadId>> {
    Ok(db
        .prepare(sql)?
        .query_map([], |r| Ok(ThreadId(r.get(0)?)))?
        .collect::<std::result::Result<_, _>>()?)
}

struct Ownership {
    threads: Vec<CleanupThread>,
    project_roots: Vec<PathBuf>,
    deleted_project_roots: Vec<PathBuf>,
    sessions: Vec<(t3_contracts::base_schemas::ProviderSessionId, PathBuf)>,
    pending_deletion_threads: BTreeSet<ThreadId>,
}

// StoreDatabase::read supplies one deferred transaction for all projection and outbox reads.
fn read_ownership(
    db: &t3_persistence::rusqlite::Connection,
) -> t3_orchestration::error::Result<Ownership> {
    let projects: BTreeMap<ProjectId, (PathBuf, bool)> = db
        .prepare(
            "SELECT project_id, workspace_root, deleted_at IS NOT NULL FROM projection_projects",
        )?
        .query_map([], |r| {
            Ok((
                ProjectId(r.get(0)?),
                (PathBuf::from(r.get::<_, String>(1)?), r.get(2)?),
            ))
        })?
        .collect::<std::result::Result<_, _>>()?;
    let with_project = |shell: t3_contracts::orchestration_v2::OrchestrationV2ThreadShell,
                        deleted| {
        let project_path = projects
            .get(&shell.project_id)
            .filter(|(_, project_deleted)| deleted || !project_deleted)
            .map(|(path, _)| path.clone());
        CleanupThread {
            shell,
            deleted,
            project_path,
        }
    };
    let live = read::thread_shells(
        db,
        &ShellQuery {
            archived: true,
            ..Default::default()
        },
    )?;
    let mut threads = live
        .threads
        .into_iter()
        .chain(live.archived_threads)
        .map(|shell| with_project(shell, false))
        .collect::<Vec<_>>();
    let mut deleted = db.prepare("SELECT thread_id, project_id, payload_json FROM orchestration_v2_projection_threads WHERE deleted_at IS NOT NULL ORDER BY thread_id")?;
    for row in deleted.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (id, project, json) = row?;
        let thread: OrchestrationV2AppThread = serde_json::from_str(&json)?;
        if thread.id != IdOrIndex::Id(ThreadId(id))
            || thread.project_id != IdOrIndex::Id(ProjectId(project))
        {
            return Err(t3_orchestration::error::Error::Unsupported(
                "inconsistent cleanup thread ownership",
            ));
        }
        let projection =
            t3_orchestration::runtime::dispatch::projection_from_records(ProjectionRecords {
                thread,
                records: vec![],
            });
        threads.push(with_project(
            read::shell_from_projection(&projection)?,
            true,
        ));
    }
    let mut sessions = Vec::new();
    let mut non_stopped = db.prepare(
        "SELECT provider_session_id, payload_json FROM orchestration_v2_projection_provider_sessions WHERE status != 'stopped'",
    )?;
    for row in non_stopped.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (stored_id, json) = row?;
        let session: OrchestrationV2ProviderSession = serde_json::from_str(&json)?;
        let IdOrIndex::Id(id) = session.id else {
            return Err(t3_orchestration::error::Error::Unsupported(
                "unexpanded cleanup session ID",
            ));
        };
        if stored_id != id.0 {
            return Err(t3_orchestration::error::Error::Unsupported(
                "inconsistent cleanup session ownership",
            ));
        }
        sessions.push((id, PathBuf::from(session.cwd.0)));
    }
    let pending = ids(
        db,
        "SELECT DISTINCT thread_id FROM orchestration_v2_effect_outbox WHERE status NOT IN ('succeeded','cancelled') AND thread_id IS NOT NULL",
    )?;
    let (active, deleted): (Vec<_>, Vec<_>) =
        projects.into_values().partition(|(_, deleted)| !deleted);
    Ok(Ownership {
        threads,
        project_roots: active.into_iter().map(|(p, _)| p).collect(),
        deleted_project_roots: deleted.into_iter().map(|(p, _)| p).collect(),
        sessions,
        pending_deletion_threads: pending,
    })
}

#[async_trait]
impl CleanupHost for Host {
    async fn snapshot(&self) -> Result<CleanupSnapshot> {
        let settings = self.settings.snapshot().await.map_err(|_| Error::Service)?;
        let mut managed_roots = vec![self.default_worktrees.clone()];
        for dir in std::iter::once(&settings.worktrees_directory)
            .chain(&settings.previous_worktrees_directories)
        {
            if let Some(path) = t3_config::home::resolve_worktrees_directory(
                &dir.0,
                &self.default_worktrees,
                &self.home,
            ) && !managed_roots.contains(&path)
            {
                managed_roots.push(path);
            }
        }
        let Ownership {
            threads,
            project_roots,
            deleted_project_roots,
            sessions,
            pending_deletion_threads,
        } = self.db.read(read_ownership).await.map_err(|error| {
            tracing::warn!(%error,"could not read storage cleanup ownership");
            Error::Refused("cleanup ownership snapshot is unavailable")
        })?;
        let live_provider_paths = sessions
            .into_iter()
            .filter(|(id, _)| self.runtime.sessions.is_live(id))
            .map(|(_, path)| path)
            .collect();
        let mut metadata = self.terminal.manager.subscribe_metadata();
        let Some(
            t3_contracts::terminal::TerminalMetadataStreamEvent::TerminalMetadataSnapshotEvent(
                snapshot,
            ),
        ) = metadata.recv().await
        else {
            return Err(Error::Refused("terminal snapshot is unavailable"));
        };
        let terminal_paths = snapshot
            .terminals
            .into_iter()
            .filter(|t| {
                matches!(
                    t.status,
                    TerminalSessionStatus::Starting | TerminalSessionStatus::Running
                )
            })
            .flat_map(|t| {
                std::iter::once(PathBuf::from(t.cwd))
                    .chain(t.worktree_path.map(|p| PathBuf::from(p.0)))
            })
            .collect();
        Ok(CleanupSnapshot {
            settings,
            threads,
            project_roots,
            deleted_project_roots,
            terminal_paths,
            live_provider_paths,
            pending_deletion_threads,
            managed_roots,
        })
    }
    async fn enabled_scheduled_threads(&self) -> Result<BTreeSet<ThreadId>> {
        self.db.read(|db| ids(db,"SELECT DISTINCT thread_id FROM scheduled_tasks WHERE enabled=1 AND thread_id IS NOT NULL")).await.map_err(|_|Error::Service)
    }
    async fn settings(&self) -> Result<ServerSettings> {
        self.settings.snapshot().await.map_err(|_| Error::Service)
    }
    async fn deletion_can_finish(&self, thread: &ThreadId) -> Result<bool> {
        let thread = thread.0.clone();
        self.db.read(move |db| Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM orchestration_v2_effect_outbox WHERE thread_id=?1 AND status NOT IN ('succeeded','cancelled','failed'))",[thread],|r| r.get(0))?)).await.map_err(|_|Error::Service)
    }
    async fn storage_cleanup_settings(&self) -> Result<StorageCleanupSettings> {
        Ok(self
            .settings
            .snapshot()
            .await
            .map_err(|_| Error::Service)?
            .storage_cleanup)
    }
    async fn acquire_workspace_lease(
        &self,
        path: &Path,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>> {
        WorkspaceLocks::workspace_leases()
            .acquire(path)
            .await
            .map_err(|_| Error::Refused("workspace is busy"))
    }
    async fn acquire_removal_permit(&self, path: &Path) -> Result<Box<dyn Send + Sync>> {
        let writers = self.worktrees.permit(path).await?;
        Ok(Box::new((
            writers,
            self.acquire_workspace_lease(path).await?,
        )))
    }
    async fn branch_pull_request(
        &self,
        cwd: &Path,
        branch: &str,
    ) -> Result<Option<MergedPullRequest>> {
        self.worktrees.branch_pull_request(cwd, branch).await
    }
    async fn invalidate_git_status(&self, project: &Path) {
        self.worktrees.invalidate_git_status(project).await;
    }
}

pub struct CleanupAdapter {
    pub service: Arc<StorageCleanup>,
    host: Arc<Host>,
    jobs: tokio_util::task::TaskTracker,
    shutdown: tokio_util::sync::CancellationToken,
}
impl CleanupAdapter {
    pub(crate) fn new(
        context: &DomainContext<'_>,
        domains: &Domains,
        worktrees: Arc<dyn CleanupWorktrees>,
    ) -> crate::Result<Arc<Self>> {
        let missing = || {
            crate::Error::Service(
                "storage cleanup requires terminal and orchestration services".into(),
            )
        };
        let orchestration = domains.get::<OrchestrationAdapter>().ok_or_else(missing)?;
        let host = Arc::new(Host {
            db: StoreDatabase(context.database.clone()),
            settings: context.local.settings.clone(),
            runtime: orchestration.composed().cloned().ok_or_else(missing)?,
            terminal: domains.get().ok_or_else(missing)?,
            worktrees,
            default_worktrees: context.local.config.paths.worktrees(),
            home: context.account_home.clone(),
        });
        let service = Arc::new(StorageCleanup::new(
            context.local.config.paths.state_dir.clone(),
            host.clone(),
            Arc::new(SystemClock),
        ));
        Ok(Arc::new(Self {
            service,
            host,
            jobs: Default::default(),
            shutdown: context.shutdown.child_token(),
        }))
    }
    pub async fn run(&self) -> Result<StorageCleanupReport> {
        let service = self.service.clone();
        self.jobs
            .spawn(async move { service.run_now().await })
            .await
            .map_err(|_| Error::Service)?
    }
}

fn changed(old: &ServerSettings, new: &ServerSettings) -> bool {
    old.storage_cleanup != new.storage_cleanup
        || old.worktree_cleanup != new.worktree_cleanup
        || old
            .project_settings_overrides
            .keys()
            .chain(new.project_settings_overrides.keys())
            .any(|id| {
                old.project_settings_overrides
                    .get(id)
                    .and_then(|p| p.worktree_cleanup.as_ref())
                    != new
                        .project_settings_overrides
                        .get(id)
                        .and_then(|p| p.worktree_cleanup.as_ref())
            })
}

fn deletes_worktrees(settings: &ServerSettings) -> bool {
    std::iter::once(&ProjectId(String::new()))
        .chain(settings.project_settings_overrides.keys())
        .any(|id| {
            t3_host::storage_cleanup::resolve_rules(settings, id)
                .is_ok_and(|r| r.worktree_on_delete)
        })
}

async fn event_needs_sweep(
    event: &t3_contracts::application_event::ApplicationStoredEvent,
    settings: &ServerSettings,
    runtime: &t3_orchestration::composition::OrchestrationRuntime,
    cleanup: &StorageCleanup,
) -> bool {
    use t3_contracts::{
        application_event::ApplicationStoredEvent,
        orchestration_v2::{OrchestrationV2DomainEvent, ProviderSessionStatus},
    };
    let ApplicationStoredEvent::Orchestration(stored) = event else {
        return false;
    };
    match &stored.event {
        OrchestrationV2DomainEvent::ThreadDeleted(_) => {
            cleanup.note_thread_deletion();
            deletes_worktrees(settings)
        }
        OrchestrationV2DomainEvent::ProviderSessionUpdated(event) => {
            let session = &event.payload;
            let stopped = match session.status {
                ProviderSessionStatus::Stopped => true,
                ProviderSessionStatus::Error => {
                    matches!(&session.id,IdOrIndex::Id(id) if !runtime.sessions.is_live(id))
                }
                _ => false,
            };
            stopped
                && deletes_worktrees(settings)
                && cleanup
                    .provider_stop_may_unblock(Path::new(&session.cwd.0))
                    .await
        }
        _ => false,
    }
}

impl DomainRuntime for CleanupAdapter {
    fn name(&self) -> &'static str {
        "storage cleanup"
    }
    fn activate(&self) -> futures::future::BoxFuture<'_, crate::Result<()>> {
        Box::pin(async move {
            use t3_orchestration::api::EventCommitter;
            let runtime = self.host.runtime.clone();
            let mut events = runtime
                .events
                .subscribe_shared()
                .map_err(super::persistence::service_error)?;
            let mut changes = self.host.settings.subscribe_changes();
            let mut old = self
                .host
                .settings
                .snapshot()
                .await
                .map_err(super::persistence::service_error)?;
            let stop = self.shutdown.clone();
            let service = self.service.clone();
            let worker = service.clone();
            let shutdown = stop.clone();
            self.jobs
                .spawn(async move { worker.run_automatic_until(shutdown.cancelled()).await });
            self.jobs.spawn(async move {
                loop {
                    let requested = tokio::select! {
                        biased;
                        () = stop.cancelled() => break,
                        next = changes.recv() => {
                            let Some(new) = next else { break };
                            let relevant = changed(&old, &new);
                            old = new;
                            relevant
                        }
                        event = events.recv() => match event {
                            Ok(event) => event_needs_sweep(&event, &old, &runtime, &service).await,
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => deletes_worktrees(&old),
                        }
                    };
                    if !requested {continue;}
                    service.request_automatic_sweep();
                }
            });
            Ok(())
        })
    }
    fn shutdown(&self) -> futures::future::BoxFuture<'_, crate::Result<()>> {
        Box::pin(async move {
            self.shutdown.cancel();
            self.jobs.close();
            self.jobs.wait().await;
            Ok(())
        })
    }
}
