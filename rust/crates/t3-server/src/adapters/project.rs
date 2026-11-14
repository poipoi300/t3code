//! The only contact surface with `t3-project`; compiled with the `project` feature.
//!
//! `ProjectService` (projects.mutate) and `ManagedProjectFolders` (projects.ensureScratch) over
//! the composed orchestration runtime: they commit through its event sink, share its thread
//! lanes, clock and ids, and read its projections.
use std::{path::Path, sync::Arc};
use t3_contracts::{
    project::{Project, ProjectEnsureScratchResult, ProjectMutation},
    project_clone::ProjectCloneStartInput,
};
use t3_project::{
    ProjectService, ProjectServices,
    clones::{CloneProgress, CloneRepository, PreparedClone, ProjectCloneTracker},
    enrichment::ProjectEnrichmentService,
    identity::RepositoryIdentityResolver,
    managed::{ManagedFolderIo, ManagedProjectFolders},
    promotion::{MoveThreadTasks, ScratchThreadPromotion},
};

pub use t3_project::Error as ProjectError;

pub struct ProjectAdapter {
    service: ProjectService,
    managed: ManagedProjectFolders,
    /// Present when scheduled tasks exist to move with a promoted thread.
    promotion: Option<ScratchThreadPromotion>,
}

/// Cloning a remote repository needs the clone workflow (source-control sign-in, progress,
/// cleanup), which is not assembled; a clone request is refused instead of guessed.
struct CloningUnavailable;
#[async_trait::async_trait]
impl CloneRepository for CloningUnavailable {
    async fn prepare(&self, _: &ProjectCloneStartInput) -> t3_project::Result<PreparedClone> {
        Err(ProjectError::Unavailable(
            "Cloning a repository is not available on this server yet.",
        ))
    }
    async fn clone_repository(
        &self,
        _: &ProjectCloneStartInput,
        _: &PreparedClone,
        _: Arc<dyn Fn(CloneProgress) + Send + Sync>,
        _: t3_process::CancellationToken,
    ) -> t3_project::Result<()> {
        Err(ProjectError::Unavailable(
            "Cloning a repository is not available on this server yet.",
        ))
    }
    async fn discard(&self, _: &str) -> t3_project::Result<()> {
        Ok(())
    }
}

fn io(operation: &'static str) -> impl Fn(std::io::Error) -> ProjectError {
    move |error| ProjectError::Operation {
        operation,
        detail: error.to_string(),
    }
}

/// Managed project folders on the host filesystem. Removal refuses a link in place of the
/// claimed directory, so it can never delete outside the managed root.
struct HostFolders;
#[async_trait::async_trait]
impl ManagedFolderIo for HostFolders {
    async fn create_directory(&self, path: &Path, recursive: bool) -> t3_project::Result<bool> {
        let existed = tokio::fs::symlink_metadata(path).await.is_ok();
        let created = if recursive {
            tokio::fs::create_dir_all(path).await
        } else {
            tokio::fs::create_dir(path).await
        };
        match created {
            Ok(()) => Ok(!existed),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(io("create managed folder")(error)),
        }
    }
    async fn write_file(&self, path: &Path, contents: &str) -> t3_project::Result<()> {
        tokio::fs::write(path, contents)
            .await
            .map_err(io("write managed folder file"))
    }
    async fn remove_directory(&self, path: &Path) -> t3_project::Result<()> {
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || {
            let metadata = std::fs::symlink_metadata(&path)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(std::io::Error::other(
                    "refusing to remove a non-directory or a link",
                ));
            }
            std::fs::remove_dir_all(&path)
        })
        .await
        .map_err(std::io::Error::other)
        .and_then(|removed| removed)
        .map_err(io("remove managed folder"))
    }
}

impl ProjectAdapter {
    #[cfg(feature = "local-services")]
    pub fn start(
        context: &crate::services::DomainContext<'_>,
        orchestration: &crate::adapters::orchestration::OrchestrationAdapter,
        scheduled_tasks: Option<Arc<dyn MoveThreadTasks>>,
    ) -> crate::Result<Arc<Self>> {
        let runtime = orchestration.composed().ok_or_else(|| {
            crate::Error::Service("projects need the composed orchestration runtime".into())
        })?;
        let clock = runtime.clock();
        let service = ProjectService::new(ProjectServices {
            database: t3_orchestration::stores::StoreDatabase(context.database.clone()),
            sink: runtime.events.clone(),
            projections: runtime.projections.clone(),
            legacy: runtime.legacy.clone(),
            thread_lanes: runtime.thread_lanes(),
            clock: clock.clone(),
            ids: runtime.ids(),
            settings: context.local.settings.clone(),
            enrichment: ProjectEnrichmentService::new(
                RepositoryIdentityResolver::default(),
                Arc::new(t3_workspace::project::FaviconResolver::new(
                    context.account_home.clone(),
                )),
            ),
            clones: ProjectCloneTracker::new(Arc::new(CloningUnavailable), clock),
            home: context.account_home.clone(),
        });
        let managed = ManagedProjectFolders::new(
            context.local.config.paths.base_dir.clone(),
            service.clone(),
            Arc::new(HostFolders),
            Default::default(),
        );
        let promotion = scheduled_tasks.map(|tasks| {
            ScratchThreadPromotion::new(
                service.clone(),
                managed.clone(),
                runtime.projections.clone(),
                runtime.command_dispatcher(),
                tasks,
            )
        });
        Ok(Arc::new(Self {
            service,
            managed,
            promotion,
        }))
    }

    /// `projects.createFromThread`; `None` without scheduled tasks to move.
    pub async fn create_from_thread(
        &self,
        input: t3_contracts::project::ProjectCreateFromThreadInput,
    ) -> Option<t3_project::Result<t3_contracts::project::ProjectCreateFromThreadResult>> {
        Some(self.promotion.as_ref()?.create_from_thread(input).await)
    }
    pub fn promotes_threads(&self) -> bool {
        self.promotion.is_some()
    }

    /// `projects.ensureScratch`.
    pub async fn ensure_scratch(&self) -> t3_project::Result<ProjectEnsureScratchResult> {
        self.managed.ensure_scratch().await
    }

    /// `projects.createNew`: a named project folder under the managed projects root.
    pub async fn create_new(
        &self,
        input: t3_contracts::project::ProjectCreateNewInput,
    ) -> t3_project::Result<t3_contracts::project::ProjectCreateNewResult> {
        self.managed.create_new(input).await
    }

    /// `projects.mutate`.
    pub async fn mutate(&self, mutation: ProjectMutation) -> t3_project::Result<Project> {
        self.service.mutate(mutation).await
    }
}

impl crate::services::DomainRuntime for ProjectAdapter {
    fn name(&self) -> &'static str {
        "project"
    }
    /// Joins admitted promotions (including detached callers) while SQLite is still open.
    fn shutdown(&self) -> futures::future::BoxFuture<'_, crate::Result<()>> {
        Box::pin(async move {
            if let Some(promotion) = &self.promotion {
                promotion.shutdown().await;
            }
            Ok(())
        })
    }
}
