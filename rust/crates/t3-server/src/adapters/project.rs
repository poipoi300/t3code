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
    projections: Arc<t3_orchestration::projections::store::ProjectionStore>,
}

/// What the start-up bootstrap selected, for the `welcome` event.
#[derive(Default)]
pub struct BootstrapTargets {
    pub project_id: Option<t3_contracts::base_schemas::ProjectId>,
    pub thread_id: Option<t3_contracts::base_schemas::ThreadId>,
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
        // The forge registry's clone backend (`adapters/forge.rs`); refused without it.
        clones: Option<Arc<dyn CloneRepository>>,
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
            clones: ProjectCloneTracker::new(
                clones.unwrap_or_else(|| Arc::new(CloningUnavailable)),
                clock,
            ),
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
            projections: runtime.projections.clone(),
        }))
    }

    /// `resolveAutoBootstrapWelcomeTargets`: the project for the server's working directory,
    /// created when missing, and its first listed thread. A folder the project rules refuse is
    /// skipped so the server still starts there. Launching a first thread needs the launch
    /// environment, which this server does not have yet: as TypeScript does when a launch
    /// fails, start-up continues with the project alone.
    pub async fn auto_bootstrap(&self, cwd: &Path) -> crate::Result<BootstrapTargets> {
        use t3_contracts::project::{ProjectMutationMember1, ProjectMutationMember1Type};
        use t3_orchestration::api::ProjectionReader;
        let title = cwd
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("project");
        let id = || uuid::Uuid::new_v4().to_string();
        let input = ProjectMutationMember1 {
            r#type: ProjectMutationMember1Type::ProjectCreate,
            command_id: t3_contracts::base_schemas::CommandId(id()),
            project_id: t3_contracts::base_schemas::ProjectId(id()),
            title: t3_contracts::base_schemas::TrimmedNonEmptyString(title.into()),
            workspace_root: t3_contracts::base_schemas::TrimmedNonEmptyString(
                cwd.to_string_lossy().into_owned(),
            ),
            create_workspace_root_if_missing: None,
            default_model_selection: None,
            scripts: None,
        };
        let project = match self.service.bootstrap(input).await {
            Ok((project, _)) => project,
            Err(ProjectError::Invalid(detail)) => {
                tracing::warn!(%detail, "Skipped bootstrap project");
                return Ok(BootstrapTargets::default());
            }
            Err(error) => return Err(crate::Error::Service(error.to_string())),
        };
        let shells = self
            .projections
            .thread_shells(t3_orchestration::api::ShellQuery {
                project_id: Some(project.id.clone()),
                ..Default::default()
            })
            .await
            .map_err(|e| crate::Error::Service(e.to_string()))?;
        let thread = shells
            .threads
            .into_iter()
            .find(|shell| {
                shell.project_id == project.id
                    && t3_orchestration::ports::transport::shell_stream::is_listed_thread_lineage(
                        &shell.lineage,
                    )
            })
            .map(|shell| shell.id);
        if thread.is_none() {
            tracing::warn!(
                project = %project.id.0,
                "Failed to launch bootstrap thread: thread launch is not available on this server"
            );
        }
        Ok(BootstrapTargets {
            project_id: Some(project.id),
            thread_id: thread,
        })
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

    /// The project service, for the project clone handlers (`handlers/project_clone.rs`).
    pub fn service(&self) -> &ProjectService {
        &self.service
    }

    /// Scratch projects and per-thread folders, for thread launch (`adapters/launch.rs`).
    pub fn managed(&self) -> &ManagedProjectFolders {
        &self.managed
    }
}

impl crate::services::DomainRuntime for ProjectAdapter {
    fn name(&self) -> &'static str {
        "project"
    }
    /// Joins admitted promotions (including detached callers) while SQLite is still open.
    fn shutdown(&self) -> futures::future::BoxFuture<'_, crate::Result<()>> {
        Box::pin(async move {
            // Clone producers stop before the project service and the forge registry.
            self.service.clones().shutdown().await;
            if let Some(promotion) = &self.promotion {
                promotion.shutdown().await;
            }
            Ok(())
        })
    }
}
