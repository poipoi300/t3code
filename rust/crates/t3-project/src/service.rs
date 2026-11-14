use crate::{Error, Result, enrichment::ProjectEnrichmentService};
use futures_util::{StreamExt, TryStreamExt, stream};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use t3_contracts::{
    base_schemas::{CommandId, EventId, ProjectId},
    orchestration_project::OrchestrationProjectShell,
    project::{
        Project, ProjectMutation, ProjectMutationMember1, ProjectMutationMember2,
        ProjectMutationMember3, ProjectSnapshot,
    },
};
use t3_orchestration::{
    api::{
        Clock, CommitGuard, CommitRequest, EventCommitter, IdGenerator, IdKind, ProjectionReader,
        ShellQuery,
    },
    ports::{
        decisions::thread_command_executor::KeyedLanes,
        sessions_recovery::legacy::legacy_v1_thread_importer::LegacyV1ThreadImporter,
        thread_project::{
            project_commands::{
                ProjectChange, ProjectCommand, ProjectCommandRejection, ProjectCommandState,
                decode_project_command_rejection, encode_project_command_rejection,
                plan_project_command,
            },
            thread_deletion::plan_thread_deletion,
        },
    },
    runtime::commit::EventSink,
    stores::{
        StoreDatabase,
        projects::{self, ProjectRow},
        receipts::{AggregateId, CommandReceipt, ReceiptStatus},
    },
};
use tokio_util::task::TaskTracker;
use unicode_segmentation::UnicodeSegmentation;

pub struct ProjectServices {
    pub database: StoreDatabase,
    pub sink: EventSink,
    pub projections: Arc<dyn ProjectionReader>,
    pub legacy: Arc<LegacyV1ThreadImporter>,
    /// Must be the dispatcher's own lanes, so cascade deletion cannot race thread mutations.
    pub thread_lanes: Arc<KeyedLanes>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
    pub settings: Arc<t3_config::settings::SettingsService>,
    pub enrichment: ProjectEnrichmentService,
    pub clones: crate::clones::ProjectCloneTracker,
    pub home: PathBuf,
}
struct Inner {
    git: t3_git::Git,
    services: ProjectServices,
    project_lanes: KeyedLanes,
    workspace_lanes: KeyedLanes,
    admission: Mutex<bool>,
    mutations: TaskTracker,
}
#[derive(Clone)]
pub struct ProjectService(Arc<Inner>);

impl ProjectService {
    pub fn new(services: ProjectServices) -> Self {
        let git = t3_git::Git::new(&services.home);
        Self::new_with_git(services, git)
    }
    pub fn new_with_git(services: ProjectServices, git: t3_git::Git) -> Self {
        Self(Arc::new(Inner {
            git,
            services,
            project_lanes: KeyedLanes::default(),
            workspace_lanes: KeyedLanes::default(),
            admission: Mutex::new(true),
            mutations: TaskTracker::new(),
        }))
    }
    pub fn database(&self) -> &StoreDatabase {
        &self.0.services.database
    }
    pub fn git(&self) -> &t3_git::Git {
        &self.0.git
    }
    pub fn now(&self) -> t3_contracts::base_schemas::IsoDateTime {
        self.0.services.clock.now()
    }
    pub fn enrichment(&self) -> &ProjectEnrichmentService {
        &self.0.services.enrichment
    }
    pub fn clones(&self) -> &crate::clones::ProjectCloneTracker {
        &self.0.services.clones
    }
    pub async fn start_clone(
        &self,
        input: t3_contracts::project_clone::ProjectCloneStartInput,
    ) -> Result<t3_contracts::project_clone::ProjectCloneStartResult> {
        self.clones().start(input, Arc::new(self.clone())).await
    }

    async fn row(&self, id: &ProjectId, deleted: bool) -> Result<Option<ProjectRow>> {
        let id = id.clone();
        Ok(self
            .database()
            .read(move |db| projects::get(db, &id, deleted))
            .await?)
    }
    async fn hydrate(&self, row: ProjectRow) -> Result<Project> {
        let enrichment = if row.deleted_at.is_none() {
            self.enrichment().get_available(&row.workspace_root).await
        } else {
            self.enrichment().peek(&row.workspace_root).await
        };
        let mut value = serde_json::to_value(&row)?;
        value["id"] = json!(row.project_id);
        value["repositoryIdentity"] = json!(enrichment.repository_identity);
        value["faviconPath"] = json!(row.favicon_path.or(enrichment.favicon_path));
        Ok(serde_json::from_value(value)?)
    }
    pub async fn get_by_id(
        &self,
        id: &ProjectId,
        include_deleted: bool,
    ) -> Result<Option<Project>> {
        match self.row(id, include_deleted).await? {
            Some(r) => Ok(Some(self.hydrate(r).await?)),
            None => Ok(None),
        }
    }
    pub async fn get_by_workspace_root(
        &self,
        root: &str,
        include_deleted: bool,
    ) -> Result<Option<Project>> {
        let root = self.normalize(root, false).await?;
        let row = self
            .database()
            .read(move |db| {
                if include_deleted {
                    Ok(projects::list(db, None, true)?
                        .into_iter()
                        .find(|r| r.workspace_root == root))
                } else {
                    projects::find_active_by_workspace_root(db, &root)
                }
            })
            .await?;
        match row {
            Some(r) => Ok(Some(self.hydrate(r).await?)),
            None => Ok(None),
        }
    }
    async fn normalize(&self, root: &str, create: bool) -> Result<String> {
        Ok(
            t3_workspace::paths::normalize_root(root, &self.0.services.home, create)
                .await?
                .to_string_lossy()
                .into(),
        )
    }
    pub async fn snapshot(&self) -> Result<ProjectSnapshot> {
        let rows = self
            .database()
            .read(|db| projects::list(db, None, false))
            .await?;
        let projects = stream::iter(rows)
            .map(|row| self.hydrate(row))
            .buffered(8)
            .try_collect()
            .await?;
        Ok(ProjectSnapshot {
            projects,
            updated_at: self.0.services.clock.now(),
        })
    }
    pub async fn list_page(
        &self,
        cursor: usize,
        limit: usize,
    ) -> Result<(Vec<Project>, Option<usize>)> {
        let rows = self
            .database()
            .read(|db| projects::list(db, None, false))
            .await?;
        let end = cursor.saturating_add(limit);
        let next = (end < rows.len()).then_some(end);
        let result = stream::iter(rows.into_iter().skip(cursor).take(limit))
            .map(|row| async move {
                let mut project = self.hydrate(row).await?;
                if project
                    .repository_identity
                    .as_ref()
                    .and_then(|i| i.as_ref())
                    .is_none()
                {
                    project.repository_identity = Some(
                        self.enrichment()
                            .read_repository_identity(&project.workspace_root.0)
                            .await,
                    );
                }
                Ok::<_, Error>(project)
            })
            .buffered(8)
            .try_collect()
            .await?;
        Ok((result, next))
    }
    pub async fn get_shell(&self, id: &ProjectId) -> Result<Option<OrchestrationProjectShell>> {
        let id = id.clone();
        let shell = self
            .database()
            .read(move |db| projects::get_shell(db, &id))
            .await?;
        match shell {
            Some(s) => Ok(Some(self.enrich_shell(s).await)),
            None => Ok(None),
        }
    }
    pub async fn list_shells(
        &self,
        ids: Option<Vec<ProjectId>>,
    ) -> Result<Vec<OrchestrationProjectShell>> {
        let shells = self
            .database()
            .read(move |db| projects::list_shells(db, ids.as_deref()))
            .await?;
        let result = stream::iter(shells)
            .map(|shell| self.enrich_shell(shell))
            .buffered(16)
            .collect()
            .await;
        Ok(result)
    }
    async fn enrich_shell(
        &self,
        mut shell: OrchestrationProjectShell,
    ) -> OrchestrationProjectShell {
        shell.repository_identity = Some(
            self.enrichment()
                .get_available(&shell.workspace_root.0)
                .await
                .repository_identity,
        );
        shell
    }
    pub async fn bootstrap(&self, input: ProjectMutationMember1) -> Result<(Project, bool)> {
        if let Some(existing) = self
            .get_by_workspace_root(&input.workspace_root.0, false)
            .await?
        {
            return Ok((existing, false));
        }
        Ok((self.create(input).await?, true))
    }
    pub async fn bootstrap_automatic(
        &self,
        input: ProjectMutationMember1,
    ) -> Result<Option<(Project, bool)>> {
        if let Some(existing) = self
            .get_by_workspace_root(&input.workspace_root.0, false)
            .await?
        {
            return Ok(Some((existing, false)));
        }
        let root = self
            .normalize(
                &input.workspace_root.0,
                input.create_workspace_root_if_missing.flatten() == Some(true),
            )
            .await?;
        if self.git().at(&root).is_worktree_container().await {
            return Ok(None);
        }
        self.create_at_root(input, root)
            .await
            .map(|project| Some((project, true)))
    }
    pub async fn create(&self, input: ProjectMutationMember1) -> Result<Project> {
        let root = self
            .normalize(
                &input.workspace_root.0,
                input.create_workspace_root_if_missing.flatten() == Some(true),
            )
            .await?;
        self.reject_worktree_container(&root).await?;
        self.create_at_root(input, root).await
    }
    async fn reject_worktree_container(&self, root: &str) -> Result<()> {
        if self.git().at(root).is_worktree_container().await {
            return Err(Error::Invalid(format!(
                "{root} holds a repository's worktrees but is not a checkout itself. Add one of its worktrees as the project instead."
            )));
        }
        Ok(())
    }
    async fn create_at_root(&self, input: ProjectMutationMember1, root: String) -> Result<Project> {
        self.commit(ProjectCommand::Create {
            command_id: input.command_id,
            project_id: input.project_id.clone(),
            title: input.title.0,
            workspace_root: root.clone(),
            scripts: input.scripts.flatten(),
        })
        .await?;
        self.enrichment().invalidate([root]).await;
        self.committed(&input.project_id).await
    }
    pub async fn update(&self, input: ProjectMutationMember2) -> Result<Project> {
        let previous = self
            .row(&input.project_id, false)
            .await?
            .ok_or_else(|| Error::NotFound(input.project_id.0.clone()))?;
        let root = match input.workspace_root.flatten() {
            Some(root) => self.normalize(&root.0, false).await?,
            None => previous.workspace_root.clone(),
        };
        if root != previous.workspace_root {
            self.reject_worktree_container(&root).await?;
        }
        let invalidate = root != previous.workspace_root || input.favicon_path.is_some();
        self.commit(ProjectCommand::Update {
            command_id: input.command_id,
            project_id: input.project_id.clone(),
            change: Box::new(ProjectChange {
                title: input.title.flatten().map(|s| s.0),
                workspace_root: (root != previous.workspace_root).then_some(root.clone()),
                default_model_selection: input.default_model_selection,
                default_thread_env_mode: input.default_thread_env_mode,
                auto_pull: input.auto_pull.flatten(),
                favicon_path: input.favicon_path.map(|s| s.map(|s| s.0)),
                project_icon: input.project_icon,
                scripts: input.scripts.flatten(),
                pinned: input.pinned.flatten(),
                archived: input.archived.flatten(),
            }),
        })
        .await?;
        if invalidate {
            self.enrichment()
                .invalidate([previous.workspace_root, root])
                .await;
        }
        self.committed(&input.project_id).await
    }
    pub async fn delete(&self, input: ProjectMutationMember3) -> Result<Project> {
        let existing = self
            .row(&input.project_id, true)
            .await?
            .ok_or_else(|| Error::NotFound(input.project_id.0.clone()))?;
        let first = existing.deleted_at.is_none();
        if first {
            self.delete_children(&input).await?;
        }
        self.commit(ProjectCommand::Delete {
            command_id: input.command_id,
            project_id: input.project_id.clone(),
        })
        .await?;
        if first {
            let patch = serde_json::from_value(
                json!({"projectSettingsOverrides":{input.project_id.0.clone():null}}),
            )?;
            if let Err(error) = self.0.services.settings.update(&patch).await {
                tracing::warn!(project_id = ?input.project_id.0, %error, "Failed to clear settings overrides for deleted project.");
            }
        }
        self.enrichment()
            .invalidate([existing.workspace_root])
            .await;
        self.committed(&input.project_id).await
    }
    async fn delete_children(&self, input: &ProjectMutationMember3) -> Result<()> {
        let s = &self.0.services;
        let snapshot = s
            .projections
            .shell_snapshot(Some(vec![input.project_id.clone()]), None)
            .await?;
        let mut threads = snapshot
            .threads
            .into_iter()
            .chain(snapshot.archived_threads)
            .collect::<Vec<_>>();
        threads.sort_by(|a, b| b.created_at.0.cmp(&a.created_at.0));
        if !threads.is_empty() && input.force.flatten() != Some(true) {
            return Err(Error::NotEmpty(input.project_id.0.clone()));
        }
        for thread in threads {
            let _guard = s.thread_lanes.acquire([thread.id.0.clone()]).await;
            s.legacy.ensure_transcript(thread.id.clone()).await?;
            use t3_orchestration::api::RecordKind;
            let projection = crate::projection::selected_projection(
                s.projections.as_ref(),
                thread.id.clone(),
                vec![
                    RecordKind::Runs,
                    RecordKind::Attempts,
                    RecordKind::Nodes,
                    RecordKind::RuntimeRequests,
                    RecordKind::Subagents,
                    RecordKind::ProviderSessions,
                ],
            )
            .await?;
            if projection.thread.deleted_at.is_some()||t3_orchestration::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(&projection.thread.project_id)?!=input.project_id {continue;}
            let command_id = CommandId(format!(
                "{}:delete-thread:{}",
                input.command_id.0, thread.id.0
            ));
            let now = s.clock.now();
            let attachments = s.projections.attachment_ids(thread.id.clone()).await?;
            let decision = plan_thread_deletion(
                &command_id,
                &thread.id,
                &projection,
                &attachments,
                &now,
                s.ids.as_ref(),
            )?;
            let expected = AggregateId::Thread(thread.id.clone());
            let result = s
                .sink
                .commit(CommitRequest {
                    receipt: Some(CommandReceipt {
                        command_id,
                        aggregate: expected.clone(),
                        command_type: "thread.delete".into(),
                        accepted_at: now.0,
                        result_sequence: 0,
                        status: ReceiptStatus::Accepted,
                        error: None,
                    }),
                    decision,
                    guard: CommitGuard::None,
                })
                .await?;
            let receipt = result
                .receipt
                .ok_or_else(|| Error::Invalid("Missing thread deletion receipt".into()))?;
            if receipt.aggregate != expected || receipt.command_type != "thread.delete" {
                return Err(Error::Invalid(
                    "The thread deletion command ID belongs to a different command.".into(),
                ));
            }
            if receipt.status == ReceiptStatus::Rejected {
                return Err(Error::Invalid(receipt.error.unwrap_or_else(|| {
                    "Thread deletion was previously rejected.".into()
                })));
            }
        }
        Ok(())
    }
    async fn committed(&self, id: &ProjectId) -> Result<Project> {
        self.get_by_id(id, true).await?.ok_or_else(|| {
            Error::Invalid("The accepted project command did not produce a project row.".into())
        })
    }
    async fn commit(&self, command: ProjectCommand) -> Result<()> {
        let value = serde_json::to_value(&command)?;
        let id: ProjectId = serde_json::from_value(value["projectId"].clone())?;
        let command_id: CommandId = serde_json::from_value(value["commandId"].clone())?;
        let kind = value["type"]
            .as_str()
            .ok_or_else(|| Error::Invalid("Missing command type".into()))?
            .to_owned();
        let root = value["workspaceRoot"].as_str().map(str::to_owned);
        let _project = self.0.project_lanes.acquire([id.0.clone()]).await;
        let _workspace = self.0.workspace_lanes.acquire(root.clone()).await;
        let project = self.row(&id, true).await?;
        let owner = match root {
            Some(root) => {
                self.database()
                    .read(move |db| projects::find_active_by_workspace_root(db, &root))
                    .await?
            }
            None => None,
        };
        let now = self.0.services.clock.now();
        let plan = plan_project_command(
            &command,
            ProjectCommandState {
                project: project.as_ref(),
                workspace_owner: owner.as_ref(),
            },
            EventId(self.0.services.ids.next(IdKind::Event)),
            &now,
            |s| s.graphemes(true).count(),
        )?;
        let mut receipt = CommandReceipt {
            command_id,
            aggregate: AggregateId::Project(id.clone()),
            command_type: kind.clone(),
            accepted_at: now.0,
            result_sequence: 0,
            status: ReceiptStatus::Accepted,
            error: None,
        };
        let receipt = match plan {
            Ok(event) => self.0.services.sink.commit_project(event, receipt).await?,
            Err(rejection) => {
                receipt.error = Some(encode_project_command_rejection(&rejection)?);
                self.0.services.sink.commit_rejected(receipt).await?
            }
        };
        if receipt.aggregate != AggregateId::Project(id) || receipt.command_type != kind {
            return Err(Error::Invalid(
                "Command ID was already used by a different command.".into(),
            ));
        }
        if receipt.status == ReceiptStatus::Accepted {
            return Ok(());
        }
        match receipt
            .error
            .as_deref()
            .and_then(decode_project_command_rejection)
        {
            Some(ProjectCommandRejection::WorkspaceConflict {
                workspace_root,
                conflicting_project_id,
            }) => Err(Error::Conflict {
                workspace_root,
                conflicting_project_id: conflicting_project_id.0,
            }),
            Some(ProjectCommandRejection::MissingProject { project_id, .. }) => {
                Err(Error::NotFound(project_id.0))
            }
            Some(ProjectCommandRejection::Invariant { detail, .. }) => Err(Error::Invalid(detail)),
            None => {
                Err(Error::Invalid(receipt.error.unwrap_or_else(|| {
                    "The command was previously rejected.".into()
                })))
            }
        }
    }
    pub async fn mutate(&self, mutation: ProjectMutation) -> Result<Project> {
        let task = {
            let admission = self.0.admission.lock().unwrap_or_else(|e| e.into_inner());
            if !*admission {
                return Err(Error::Unavailable("ProjectService is shut down"));
            }
            let this = self.clone();
            self.0.mutations.spawn(async move {
                match mutation {
                    ProjectMutation::Create(m) => this.create(m).await,
                    ProjectMutation::Update(m) => this.update(m).await,
                    ProjectMutation::Delete(m) => {
                        let id = m.project_id.clone();
                        let project = this.delete(m).await?;
                        this.0.services.clones.discard(&id).await?;
                        Ok(project)
                    }
                }
            })
        };
        task.await?
    }
    pub async fn workspace_scan_directory(
        &self,
        cwd: &str,
        managed_worktrees: &std::path::Path,
    ) -> String {
        let root = PathBuf::from(cwd);
        if t3_workspace::paths::normalize_root(cwd, &self.0.services.home, false)
            .await
            .is_ok()
        {
            return cwd.into();
        }
        if !t3_workspace::paths::within(&root, managed_worktrees) || root == managed_worktrees {
            return cwd.into();
        }
        let result = async {
            let snapshot = self
                .0
                .services
                .projections
                .thread_shells(ShellQuery {
                    worktree_path: Some(cwd.into()),
                    ..Default::default()
                })
                .await?;
            for thread in snapshot
                .threads
                .into_iter()
                .chain(snapshot.archived_threads)
            {
                if thread.branch.is_some()
                    && let Some(project) = self.row(&thread.project_id, false).await?
                {
                    return Ok::<_, Error>(project.workspace_root);
                }
            }
            Ok(cwd.into())
        }
        .await;
        result.unwrap_or_else(|_| cwd.into())
    }
    pub async fn shutdown(&self) {
        *self.0.admission.lock().unwrap_or_else(|e| e.into_inner()) = false;
        self.0.mutations.close();
        self.0.mutations.wait().await;
    }
}

#[async_trait::async_trait]
impl crate::clones::CloneHooks for ProjectService {
    async fn create_project(
        &self,
        input: &t3_contracts::project_clone::ProjectCloneStartInput,
        prepared: &crate::clones::PreparedClone,
    ) -> Result<()> {
        self.create(serde_json::from_value(json!({"type":"project.create","commandId":format!("project-clone-create:{}",input.project_id.0),"projectId":input.project_id,"title":input.title,"workspaceRoot":prepared.destination_path,"createWorkspaceRootIfMissing":true}))?).await?;
        Ok(())
    }
    async fn on_cloned(&self, id: &ProjectId, root: &str) -> Result<()> {
        self.enrichment().refresh_repository_identity(root).await;
        self.update(serde_json::from_value(json!({"type":"project.update","commandId":format!("project-clone-done:{}",id.0),"projectId":id}))?).await?;
        self.enrichment().invalidate([root.into()]).await;
        self.enrichment().request(root).await;
        Ok(())
    }
}
