//! Thread launch's host (`apps/server/src/orchestration-v2/ThreadLaunchService.ts` and the
//! services it yields): the runtime's `LaunchHost` and `WorkspaceBackend`.
//!
//! Projects, folders and clone state come from `t3-project`; worktrees from the environment's
//! one Git root and the VCS group's folder settings (the server decides where a worktree lands);
//! branch names from the text-generation adapter; setup scripts from the project setup runner.
//! A launch into a new worktree publishes its progress into the VCS group's setup tracker, so
//! `subscribeWorktreeSetup` and `worktreeSetup.cancel` see it.
//!
//! The runtime is composed before projects and the setup runner exist, so those are bound once
//! they have started (`bind`), before the server accepts requests.
use crate::adapters::{git::GitAdapter, textgen::TextGenAdapter};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};
use t3_contracts::{
    base_schemas::{CommandId, ProjectId, ThreadId, TrimmedNonEmptyString},
    git::VcsCreateWorktreeInput,
    model_selection::ModelSelection,
    orchestration_v2::OrchestrationV2ThreadLaunchWorkspaceStrategy as Strategy,
    provider_instance::ProviderInstanceId,
    settings::{BranchNamingMode, ServerSettings},
    worktree_setup::{
        WorktreeSetupPhase, WorktreeSetupScript, WorktreeSetupStageId as Stage,
        WorktreeSetupStageStatus as Status,
    },
};
use t3_git::{GitWorkflowService, SetupTask, WorktreeCreationOptions, WorktreeSetupTracker};
use t3_orchestration::{
    api::{Workspace, WorkspaceBackend},
    error::{Error, Result},
    ports::checkpoints_forks::thread_launch_service::{
        GeneratedBranch, InitialMessage, LaunchHost, SetupInput, SetupRun,
    },
    stores::{StoreDatabase, projects, receipts},
};
use tokio_util::sync::CancellationToken;

/// Started before the orchestration runtime, whose launch ports it supplies.
pub struct LaunchAdapter {
    inner: Arc<Inner>,
}

struct Inner {
    this: std::sync::Weak<Inner>,
    git: Arc<GitAdapter>,
    /// The workflow template every launch checkout derives from with `.at(root)`.
    workflows: GitWorkflowService,
    settings: Arc<t3_config::settings::SettingsService>,
    database: StoreDatabase,
    textgen: Arc<TextGenAdapter>,
    terminal: Arc<crate::adapters::terminal::TerminalAdapter>,
    late: OnceLock<Late>,
    /// Worktree setups this host is publishing, by thread.
    setups: Mutex<HashMap<String, Active>>,
    /// Worktrees this host created whose launch may still abandon them, by path.
    created: Mutex<HashMap<PathBuf, Created>>,
}

/// A worktree only this launch may remove (TS `createdWorktreePath`/`createdBranch`).
struct Created {
    thread: ThreadId,
    project_root: PathBuf,
    branch: String,
    /// A cancelled setup's cancel waiter, released once the worktree is removed.
    ended: Option<CancellationToken>,
}

struct Late {
    projects: Arc<crate::adapters::project::ProjectAdapter>,
    setup: Option<Arc<crate::adapters::project_setup::ProjectSetupAdapter>>,
}

/// One tracked worktree preparation (TS `setupTracker.begin` … `finish`).
struct Active {
    /// Set once the worktree exists.
    worktree: Option<PathBuf>,
    /// A clone of the preparation's token: `worktreeSetup.cancel` cancels the whole preparation.
    preparation: CancellationToken,
    /// Cancelled when the setup finishes, which ends the tracker's cancel wait.
    ended: CancellationToken,
}

fn failure(operation: &'static str, detail: impl std::fmt::Display) -> Error {
    Error::Service {
        operation,
        detail: detail.to_string(),
    }
}

fn git_failure(operation: &'static str) -> impl Fn(t3_git::GitError) -> Error {
    // Git's own text can carry stderr, argv and URLs; the run's failure item gets a fixed text.
    move |error| {
        tracing::warn!(%error, operation, "thread launch git operation failed");
        failure(operation, "Git could not complete the requested operation.")
    }
}

impl LaunchAdapter {
    pub fn start(
        context: &crate::services::DomainContext<'_>,
        git: Arc<GitAdapter>,
        textgen: Arc<TextGenAdapter>,
        terminal: Arc<crate::adapters::terminal::TerminalAdapter>,
    ) -> Option<Arc<Self>> {
        let vcs = git.vcs()?;
        let workflows = GitWorkflowService::new(git.at(&context.local.config.cwd))
            .with_worktrees_directory(context.local.config.paths.worktrees())
            .with_worktree_host(vcs.worktree_host());
        Some(Arc::new(Self {
            inner: Arc::new_cyclic(|this| Inner {
                this: this.clone(),
                git,
                workflows,
                settings: context.local.settings.clone(),
                database: StoreDatabase(context.database.clone()),
                textgen,
                terminal,
                late: OnceLock::new(),
                setups: Mutex::new(HashMap::new()),
                created: Mutex::new(HashMap::new()),
            }),
        }))
    }

    /// Projects and the setup runner start after the runtime; bound before requests are served.
    pub fn bind(
        &self,
        projects: Arc<crate::adapters::project::ProjectAdapter>,
        setup: Option<Arc<crate::adapters::project_setup::ProjectSetupAdapter>>,
    ) {
        let _ = self.inner.late.set(Late { projects, setup });
    }

    /// `OrchestrationAdapter::start`'s launch ports.
    pub fn ports(&self) -> crate::adapters::orchestration::LaunchPorts {
        (self.inner.clone(), self.inner.clone())
    }

    /// Checks what a client named before anything is created: refs and branch names that Git
    /// would read as options or that are not ref names, and an existing worktree that is not one
    /// of the project repository's registered worktrees. `Err` is the refusal's message.
    pub async fn check_client_strategy(
        &self,
        project: &ProjectId,
        strategy: &Strategy,
    ) -> std::result::Result<(), String> {
        match strategy {
            Strategy::Root(root) => check_branch(root.branch.as_ref().and_then(Option::as_ref)),
            Strategy::Worktree(worktree) => {
                check_revision(&worktree.base_ref.0)?;
                check_branch(worktree.branch.as_ref().and_then(Option::as_ref))
            }
            Strategy::ExistingWorktree(existing) => {
                check_branch(existing.branch.as_ref().and_then(Option::as_ref))?;
                self.inner
                    .registered_worktree(project, &existing.worktree_path.0)
                    .await
            }
        }
    }
}

/// The scheduler's pre-launch check (`adapters/scheduled_launch.rs`): an unbound task's stored
/// strategy is checked as a client's is, before its thread is created.
#[cfg(feature = "scheduler")]
#[async_trait::async_trait]
impl crate::adapters::scheduled_launch::StrategyCheck for LaunchAdapter {
    async fn check(
        &self,
        project: &ProjectId,
        strategy: &Strategy,
    ) -> std::result::Result<(), String> {
        self.check_client_strategy(project, strategy).await
    }
}

/// A revision a client names reaches Git after `--end-of-options` or as a ref argument; it may
/// never start with `-` (Git's previous-branch `-` excepted) or carry control characters.
fn check_revision(reference: &str) -> std::result::Result<(), String> {
    if (reference.starts_with('-') && reference != "-") || reference.chars().any(|c| c.is_control())
    {
        return Err(format!(
            "Base ref \"{reference}\" is not a valid Git revision."
        ));
    }
    Ok(())
}

/// `git check-ref-format --branch`'s rules for a new branch name.
fn check_branch(branch: Option<&TrimmedNonEmptyString>) -> std::result::Result<(), String> {
    let Some(branch) = branch else { return Ok(()) };
    let name = branch.0.as_str();
    let invalid = name.starts_with('-')
        || name == "@"
        || name.contains("..")
        || name.contains("@{")
        || name.contains("//")
        || name.ends_with('/')
        || name.ends_with('.')
        || name.starts_with('/')
        || name
            .split('/')
            .any(|part| part.starts_with('.') || part.ends_with(".lock"))
        || name
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || "~^:?*[\\".contains(c));
    if invalid {
        return Err(format!("Branch \"{name}\" is not a valid Git branch name."));
    }
    Ok(())
}

impl Inner {
    fn late(&self) -> Result<&Late> {
        self.late
            .get()
            .ok_or(Error::Unsupported("thread launch before projects started"))
    }

    async fn project(&self, id: &ProjectId) -> Result<projects::ProjectRow> {
        let id = id.clone();
        self.database
            .read(move |db| projects::get(db, &id, false))
            .await?
            .ok_or_else(|| failure("resolve-project", "Project not found."))
    }

    /// The project's settings as TypeScript's `resolveProjectSettings` gives them.
    async fn project_settings(&self, project: &ProjectId) -> Result<ServerSettings> {
        let settings = self
            .textgen
            .settings()
            .await
            .map_err(|e| failure("read-settings", e))?;
        t3_shared::project_settings::resolve_project_settings(&settings, Some(project), None)
            .map(|resolved| resolved.settings)
            .map_err(|e| failure("read-settings", e))
    }

    /// An existing worktree: a local-disk path (decided from its text before any filesystem
    /// call) that resolves without a link to a network target, a remote drive or a device name
    /// (the workspace rule, `t3_workspace::paths::canonicalize`), and that Git lists as a
    /// worktree of the project's repository, or, for the scratch project, a folder under the
    /// scratch root (the host's per-thread folders). Git runs only after the path passed.
    async fn registered_worktree(
        &self,
        project: &ProjectId,
        path: &str,
    ) -> std::result::Result<(), String> {
        const REFUSED: &str = "The worktree is not a registered worktree of this project.";
        if !crate::adapters::git::local_disk_path(Path::new(path)) {
            return Err(REFUSED.into());
        }
        let Ok(project) = self.project(project).await else {
            return Err("Project not found.".into());
        };
        let Some(target) = local_real_path(PathBuf::from(path)).await else {
            return Err(REFUSED.into());
        };
        if let Ok(late) = self.late()
            && late
                .projects
                .managed()
                .is_scratch_project(&project.project_id)
                .await
        {
            let scratch = match late.projects.managed().scratch_root().await {
                Some(root) => local_real_path(root).await,
                None => None,
            };
            return match scratch {
                Some(root) if t3_workspace::paths::within(&target, &root) && target != root => {
                    Ok(())
                }
                _ => Err(REFUSED.into()),
            };
        }
        let Ok(trees) = self.git.at(&project.workspace_root).list_worktrees().await else {
            return Err(REFUSED.into());
        };
        for tree in trees {
            if local_real_path(tree.path)
                .await
                .is_some_and(|tree| tree == target)
            {
                return Ok(());
            }
        }
        Err(REFUSED.into())
    }

    fn tracker(&self) -> Option<&WorktreeSetupTracker> {
        self.git.vcs().map(|vcs| vcs.setup_tracker())
    }

    fn tracked(&self, thread: &ThreadId) -> bool {
        self.setups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&thread.0)
    }

    fn stage(&self, thread: &ThreadId, stage: Stage, status: Status, detail: Option<String>) {
        if let Some(tracker) = self.tracker() {
            tracker.stage_status(thread, stage, status, detail.map(Some));
        }
    }

    /// TS `setupTracker.finish`: once per setup. It ends the cancel wait, except for a
    /// cancelled setup whose worktree the launch removes next: that wait ends with the removal.
    fn finish(&self, thread: &ThreadId, phase: WorktreeSetupPhase, error: Option<&str>) {
        let active = self
            .setups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&thread.0);
        let Some(active) = active else { return };
        if let Some(tracker) = self.tracker() {
            tracker.finish(thread, phase, error);
        }
        let mut created = self.created.lock().unwrap_or_else(|e| e.into_inner());
        match (active.worktree.as_ref(), phase) {
            (Some(path), WorktreeSetupPhase::Done) => {
                created.remove(path);
            }
            (Some(path), WorktreeSetupPhase::Cancelled) => {
                if let Some(entry) = created.get_mut(path) {
                    entry.ended = Some(active.ended);
                    return;
                }
            }
            _ => {}
        }
        active.ended.cancel();
    }

    /// TS `markUncancellable` then the agent stage: the run is released next.
    fn agent_started(&self, thread: &ThreadId) {
        if let Some(tracker) = self.tracker() {
            tracker.mark_uncancellable(thread);
        }
        self.stage(thread, Stage::Agent, Status::Running, None);
        self.stage(thread, Stage::Agent, Status::Done, None);
    }

    fn begin(
        &self,
        thread: &ThreadId,
        branch: Option<String>,
        base_ref: String,
        preparation: CancellationToken,
    ) {
        let Some(tracker) = self.tracker() else {
            return;
        };
        let ended = CancellationToken::new();
        let (task, _) = SetupTask::spawn({
            let preparation = preparation.clone();
            let ended = ended.clone();
            move |cancel: CancellationToken| async move {
                tokio::select! {
                    () = cancel.cancelled() => {
                        preparation.cancel();
                        ended.cancelled().await;
                    }
                    () = ended.cancelled() => {}
                }
            }
        });
        self.setups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                thread.0.clone(),
                Active {
                    worktree: None,
                    preparation,
                    ended,
                },
            );
        tracker.begin(
            thread.clone(),
            branch,
            Some(base_ref),
            &[
                Stage::Fetch,
                Stage::Checkout,
                Stage::Submodules,
                Stage::SetupScript,
                Stage::Agent,
            ],
            Some(task),
        );
    }

    async fn create_worktree(
        &self,
        thread: &ThreadId,
        root: &Path,
        base_ref: &str,
        branch: &str,
        start_from_origin: bool,
        cancellation: CancellationToken,
    ) -> Result<Workspace> {
        let vcs = self
            .git
            .vcs()
            .ok_or(Error::Unsupported("worktrees without the VCS services"))?;
        if let Some(setting) = vcs.worktree_directory_refusal().await {
            return Err(failure(
                "provision-worktree",
                format!(
                    "The worktree location \"{setting}\" must be an absolute folder on this machine, not a drive root. Change it in Settings → Worktrees."
                ),
            ));
        }
        let git = self.git.at(root).with_cancellation(cancellation.clone());
        let mut start = base_ref.to_owned();
        let mut base_name = base_ref.to_owned();
        let remote = if start_from_origin {
            git.resolve_worktree_base_remote(base_ref)
                .await
                .map_err(git_failure("provision-worktree"))?
        } else {
            None
        };
        self.stage(
            thread,
            Stage::Fetch,
            if remote.is_some() {
                Status::Running
            } else {
                Status::Skipped
            },
            None,
        );
        if let Some((remote, remote_branch)) = &remote {
            if let Some((commit, remote_ref)) = git
                .fetch_worktree_base_commit(base_ref, Some((remote, remote_branch)))
                .await
                .map_err(git_failure("provision-worktree"))?
            {
                start = commit;
                base_name = format!("refs/remotes/{remote_ref}");
            }
            self.stage(thread, Stage::Fetch, Status::Done, None);
        }
        self.stage(thread, Stage::Checkout, Status::Running, None);
        let created = self
            .workflows
            .at(root)
            .with_cancellation(cancellation)
            .create_worktree_with_options(
                &VcsCreateWorktreeInput {
                    cwd: TrimmedNonEmptyString(root.to_string_lossy().into_owned()),
                    ref_name: TrimmedNonEmptyString(start),
                    new_ref_name: Some(Some(TrimmedNonEmptyString(branch.to_owned()))),
                    base_ref_name: Some(Some(TrimmedNonEmptyString(base_name))),
                    path: None,
                },
                WorktreeCreationOptions {
                    submodules: None,
                    progress: self.tracker().map(|t| t.worktree_reporter(thread.clone())),
                },
            )
            .await
            .map_err(git_failure("provision-worktree"))?;
        let path = PathBuf::from(&created.worktree.path.0);
        let branch = created.worktree.ref_name.0;
        self.created
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                path.clone(),
                Created {
                    thread: thread.clone(),
                    project_root: root.to_owned(),
                    branch: branch.clone(),
                    ended: None,
                },
            );
        if let Some(active) = self
            .setups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&thread.0)
        {
            active.worktree = Some(path.clone());
        }
        if let Some(tracker) = self.tracker() {
            let (worktree, name) = (path.to_string_lossy().into_owned(), branch.clone());
            tracker.update(thread, |snapshot| {
                snapshot.worktree_path = Some(TrimmedNonEmptyString(worktree));
                snapshot.branch = Some(TrimmedNonEmptyString(name));
                for stage in &mut snapshot.stages {
                    if stage.id == Stage::Submodules && stage.status == Status::Pending {
                        stage.status = Status::Skipped;
                        stage.detail = Some("none".into());
                    }
                }
            });
        }
        self.stage(thread, Stage::Checkout, Status::Done, None);
        let _ = vcs.broadcaster().refresh_status(root, false).await;
        Ok(Workspace {
            cwd: path.clone(),
            branch: Some(branch),
            worktree_path: Some(path),
        })
    }

    /// TS `generateBranchNameFor`: the project's writer model and naming settings.
    async fn writer_selection(
        &self,
        settings: &ServerSettings,
        project: &ProjectId,
    ) -> ModelSelection {
        let Some(selection) = &settings.source_control_writer_model_selection else {
            return settings.text_generation_model_selection.clone();
        };
        let id = selection.instance_id.0.as_str();
        let providers = self.textgen.providers().await;
        let usable = settings
            .text_generators
            .contains_key(&selection.instance_id)
            || providers.iter().any(|provider| {
                let snapshot = serde_json::to_value(provider).unwrap_or_default();
                snapshot["instanceId"] == id
                    && snapshot["enabled"] == true
                    && snapshot["availability"] != "unavailable"
                    && snapshot["supportsTextGeneration"] != false
            });
        let enabled = serde_json::to_value(settings).is_ok_and(|json| {
            t3_shared::project_settings::text_generation_selection_enabled(
                &json,
                Some(project.0.as_str()),
                id,
            )
        });
        if usable && enabled {
            selection.clone()
        } else {
            settings.text_generation_model_selection.clone()
        }
    }
}

/// `t3_workspace::paths::canonicalize` off the reactor: `None` for a path it refuses.
async fn local_real_path(path: PathBuf) -> Option<PathBuf> {
    tokio::task::spawn_blocking(move || t3_workspace::paths::canonicalize(path).ok())
        .await
        .ok()
        .flatten()
}

fn naming(settings: &ServerSettings) -> t3_textgen::prompts::BranchNaming {
    use t3_textgen::prompts::BranchNaming;
    match settings.branch_naming_mode {
        BranchNamingMode::Semantic => BranchNaming::Semantic,
        BranchNamingMode::Static => BranchNaming::Static {
            prefix: settings.branch_name_prefix.0.clone(),
        },
        BranchNamingMode::Custom => BranchNaming::Custom {
            instructions: settings.branch_name_instructions.0.clone(),
        },
    }
}

#[async_trait::async_trait]
impl LaunchHost for Inner {
    async fn is_scratch(&self, project: &ProjectId) -> Result<bool> {
        Ok(self
            .late()?
            .projects
            .managed()
            .is_scratch_project(project)
            .await)
    }

    async fn reject_during_clone(&self, project: &ProjectId) -> Result<()> {
        self.late()?
            .projects
            .service()
            .clones()
            .reject_commands_during_clone(
                &serde_json::json!({"type": "thread.create", "projectId": project}),
            )
            .map_err(|e| failure("resolve-project", e))
    }

    /// TS `ThreadManagementService.assertProviderInstanceEnabled`: a replayed command passes.
    async fn assert_provider_enabled(
        &self,
        project: &ProjectId,
        command: &CommandId,
        instance: &ProviderInstanceId,
    ) -> Result<()> {
        let id = command.clone();
        if self
            .database
            .read(move |db| receipts::get(db, &id))
            .await
            .ok()
            .flatten()
            .is_some()
        {
            return Ok(());
        }
        let settings = self
            .settings
            .snapshot()
            .await
            .map_err(|_| Error::Unsupported("server settings unreadable"))?;
        if t3_orchestration::ports::thread_project::thread_management_service::provider_instance_enabled(
            &settings, project, instance,
        )? {
            return Ok(());
        }
        Err(failure(
            "ThreadManagementProviderInstanceDisabledError",
            format!(
                "Provider instance \"{}\" is disabled for project {}.",
                instance.0, project.0
            ),
        ))
    }

    async fn folder_for_thread(
        &self,
        project: &ProjectId,
        thread: &ThreadId,
        text: &str,
    ) -> Result<Option<PathBuf>> {
        self.late()?
            .projects
            .managed()
            .folder_for_thread(project, thread, text)
            .await
            .map_err(|e| failure("provision-worktree", e))
    }

    /// TS `buildTemporaryWorktreeBranchName`, in the static naming form when the project uses it.
    async fn temporary_branch(&self, project: &ProjectId) -> Result<String> {
        let token: String = uuid::Uuid::new_v4().simple().to_string()[..8].to_owned();
        let branch = format!("t3/{token}");
        let Ok(settings) = self.project_settings(project).await else {
            return Ok(branch);
        };
        if settings.branch_naming_mode == BranchNamingMode::Static {
            return t3_textgen::text::format_generated_branch(&token, Some(&naming(&settings)))
                .map_err(|e| failure("provision-worktree", e));
        }
        Ok(branch)
    }

    async fn generate_branch(
        &self,
        project: &ProjectId,
        cwd: PathBuf,
        message: &InitialMessage,
        cancellation: CancellationToken,
    ) -> Result<GeneratedBranch> {
        use t3_textgen::generation::{Generated, GenerationContext, GenerationInput};
        let settings = self.project_settings(project).await?;
        let model_selection = self.writer_selection(&settings, project).await;
        let naming = naming(&settings);
        let exact_name = settings.branch_naming_mode == BranchNamingMode::Custom;
        let generated = self
            .textgen
            .generate(
                GenerationContext {
                    cwd,
                    model_selection,
                    project_id: Some(project.clone()),
                },
                GenerationInput::Branch(t3_textgen::prompts::BranchInput {
                    message: message.text.clone(),
                    naming: Some(naming),
                    attachments: message.attachments.clone(),
                    policy: Default::default(),
                }),
                cancellation,
            )
            .await
            .map_err(|e| failure("generate-branch", e))?;
        match generated {
            Generated::Branch { branch } => Ok(GeneratedBranch { branch, exact_name }),
            _ => Err(failure("generate-branch", "Unexpected generation result.")),
        }
    }

    async fn setup(&self, input: SetupInput, _cancellation: CancellationToken) -> Result<SetupRun> {
        use t3_workspace::scripts::{RunScriptResult, ScriptTrigger};
        let setup = self
            .late()?
            .setup
            .clone()
            .ok_or(Error::Unsupported("project setup scripts"))?;
        let thread = input.thread_id.clone();
        let tracked = self.tracked(&thread);
        if tracked {
            self.stage(&thread, Stage::SetupScript, Status::Running, None);
        }
        let on_output_line: Option<Arc<dyn Fn(String) + Send + Sync>> = match self.tracker() {
            Some(tracker) if tracked => {
                let tracker = tracker.clone();
                let thread = thread.clone();
                Some(Arc::new(move |line: String| {
                    tracker.append_tail(&thread, Stage::SetupScript, &line)
                }))
            }
            _ => None,
        };
        let (project_cwd, worktree) = (
            input.project_cwd.to_string_lossy().into_owned(),
            input.worktree_path.to_string_lossy().into_owned(),
        );
        let result = setup
            .runner()
            .run_for_thread(t3_project::scripts::SetupScriptInput {
                thread_id: &thread,
                project: None,
                project_id: Some(&input.project_id),
                project_cwd: Some(&project_cwd),
                worktree_path: &worktree,
                trigger: ScriptTrigger::Setup,
                observe_completion: tracked,
                on_output_line,
            })
            .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                if tracked {
                    self.finish(
                        &thread,
                        WorktreeSetupPhase::Failed,
                        Some(&error.to_string()),
                    );
                }
                return Err(failure("run-setup-script", error));
            }
        };
        let started = match result {
            RunScriptResult::SkippedForLink | RunScriptResult::NoScript => {
                if tracked {
                    self.stage(&thread, Stage::SetupScript, Status::Skipped, None);
                    self.agent_started(&thread);
                    self.finish(&thread, WorktreeSetupPhase::Done, None);
                }
                return Ok(SetupRun::Skipped);
            }
            RunScriptResult::Started(started) => *started,
        };
        if !tracked {
            return Ok(SetupRun::Started {
                terminal_id: started.terminal_id,
                asynchronous: started.asynchronous,
                completion: None,
            });
        }
        if let Some(tracker) = self.tracker() {
            let script = WorktreeSetupScript {
                name: started.script.name.clone(),
                command: started.script.command.clone(),
                terminal_id: TrimmedNonEmptyString(started.terminal_id.clone()),
            };
            tracker.update(&thread, |snapshot| snapshot.setup_script = Some(script));
        }
        let asynchronous = started.asynchronous;
        let Some(completion) = started.completion else {
            self.stage(&thread, Stage::SetupScript, Status::Done, None);
            self.agent_started(&thread);
            self.finish(&thread, WorktreeSetupPhase::Done, None);
            return Ok(SetupRun::Started {
                terminal_id: started.terminal_id,
                asynchronous,
                completion: None,
            });
        };
        if asynchronous {
            // The run is released without waiting; the setup finishes with the script.
            self.agent_started(&thread);
        }
        let (mut sender, receiver) = tokio::sync::oneshot::channel();
        let host = self.setups_handle(&thread);
        tokio::spawn(async move {
            let Some((inner, preparation)) = host else {
                return;
            };
            let completed = tokio::select! {
                completed = completion => Some(completed.ok().map(|c| c.exit_code)),
                () = sender.closed() => None,
            };
            // `None`: the launch stopped waiting, cancelled or failed elsewhere.
            let Some(exit) = completed else {
                let phase = if preparation.is_cancelled() {
                    WorktreeSetupPhase::Cancelled
                } else {
                    WorktreeSetupPhase::Failed
                };
                inner.finish(&thread, phase, None);
                return;
            };
            let code = exit
                .flatten()
                .map_or_else(|| "no exit code".to_owned(), |code| code.to_string());
            let succeeded = exit == Some(Some(0));
            inner.stage(
                &thread,
                Stage::SetupScript,
                if succeeded {
                    Status::Done
                } else {
                    Status::Failed
                },
                Some(format!("exited with {code}")),
            );
            if succeeded || asynchronous {
                if !asynchronous {
                    inner.agent_started(&thread);
                }
                inner.finish(&thread, WorktreeSetupPhase::Done, None);
            } else {
                inner.finish(
                    &thread,
                    WorktreeSetupPhase::Failed,
                    Some(&format!("Setup script exited with {code}.")),
                );
            }
            let _ = sender.send(exit.ok_or_else(|| {
                failure(
                    "run-setup-script",
                    "The setup script ended without an exit status.",
                )
            }));
        });
        Ok(SetupRun::Started {
            terminal_id: started.terminal_id,
            asynchronous,
            completion: Some(receiver),
        })
    }

    async fn close_terminal(&self, thread: &ThreadId, terminal: &str) -> Result<()> {
        self.terminal
            .manager
            .close(t3_contracts::terminal::TerminalCloseInput {
                thread_id: TrimmedNonEmptyString(thread.0.clone()),
                terminal_id: Some(Some(TrimmedNonEmptyString(terminal.to_owned()))),
                delete_history: Some(Some(true)),
            })
            .await
            .map_err(|e| failure("close-terminal", e))
    }
}

impl Inner {
    /// The host and the preparation token for a tracked thread, for the completion watcher.
    fn setups_handle(&self, thread: &ThreadId) -> Option<(Arc<Inner>, CancellationToken)> {
        let preparation = self
            .setups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&thread.0)?
            .preparation
            .clone();
        Some((self.this.upgrade()?, preparation))
    }
}

#[async_trait::async_trait]
impl WorkspaceBackend for Inner {
    async fn prepare(
        &self,
        project_id: ProjectId,
        thread_id: ThreadId,
        strategy: Strategy,
        cancellation: CancellationToken,
    ) -> Result<Workspace> {
        let project = self.project(&project_id).await?;
        let root = PathBuf::from(&project.workspace_root);
        match strategy {
            // TS `launchBranch`: a project-folder launch naming no branch records the checkout.
            // Every launch path reaches here (client, scheduler, MCP, a retried preparation): the
            // branch and an existing worktree are checked again, whatever checked them before.
            Strategy::Root(root_strategy) => {
                check_branch(root_strategy.branch.as_ref().and_then(Option::as_ref))
                    .map_err(|reason| failure("provision-worktree", reason))?;
                let branch = match root_strategy.branch.flatten() {
                    Some(branch) => Some(branch.0),
                    None => self.current_branch(root.clone()).await.ok().flatten(),
                };
                Ok(Workspace {
                    cwd: root,
                    branch,
                    worktree_path: None,
                })
            }
            Strategy::ExistingWorktree(existing) => {
                check_branch(existing.branch.as_ref().and_then(Option::as_ref))
                    .map_err(|reason| failure("provision-worktree", reason))?;
                self.registered_worktree(&project_id, &existing.worktree_path.0)
                    .await
                    .map_err(|reason| failure("provision-worktree", reason))?;
                let path = PathBuf::from(&existing.worktree_path.0);
                Ok(Workspace {
                    cwd: path.clone(),
                    branch: existing.branch.flatten().map(|b| b.0),
                    worktree_path: Some(path),
                })
            }
            Strategy::Worktree(worktree) => {
                let branch = worktree
                    .branch
                    .clone()
                    .flatten()
                    .map(|b| b.0)
                    .ok_or_else(|| failure("provision-worktree", "Missing branch"))?;
                check_revision(&worktree.base_ref.0)
                    .and_then(|()| check_branch(Some(&TrimmedNonEmptyString(branch.clone()))))
                    .map_err(|e| failure("provision-worktree", e))?;
                self.begin(
                    &thread_id,
                    Some(branch.clone()),
                    worktree.base_ref.0.clone(),
                    cancellation.clone(),
                );
                let created = self
                    .create_worktree(
                        &thread_id,
                        &root,
                        &worktree.base_ref.0,
                        &branch,
                        worktree.start_from_origin.flatten() == Some(true),
                        cancellation.clone(),
                    )
                    .await;
                if let Err(error) = &created {
                    let (phase, detail) = if cancellation.is_cancelled() {
                        (WorktreeSetupPhase::Cancelled, None)
                    } else {
                        (WorktreeSetupPhase::Failed, Some(error.to_string()))
                    };
                    self.finish(&thread_id, phase, detail.as_deref());
                }
                created
            }
        }
    }

    /// Only a worktree this host created for a launch is removed: TS
    /// `removeWorktree({force: true})` from the project root, then its branch.
    async fn remove(&self, workspace: Workspace, _cancellation: CancellationToken) -> Result<()> {
        let path = workspace
            .worktree_path
            .ok_or_else(|| failure("remove-worktree", "Not a worktree."))?;
        let created = self
            .created
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&path);
        let Some(created) = created else {
            return Err(failure(
                "remove-worktree",
                "The worktree was not created by this launch.",
            ));
        };
        let cancelled = self
            .setups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&created.thread.0)
            .map(|active| active.preparation.is_cancelled());
        // Cleanup completes during shutdown too (TS removes in an uninterruptible finalizer);
        // the root's own token is the server's shutdown.
        let git = self
            .git
            .at(&created.project_root)
            .with_cancellation(CancellationToken::new());
        let removed = git
            .remove_worktree(&path, true)
            .await
            .map_err(git_failure("remove-worktree"));
        if removed.is_ok()
            && let Err(error) = git.delete_local_branch(&created.branch, true).await
        {
            tracing::warn!(%error, "failed to delete an abandoned thread worktree branch");
        }
        // Still running: the launch failed or was cancelled before its setup step.
        match cancelled {
            Some(true) => self.finish(&created.thread, WorktreeSetupPhase::Cancelled, None),
            Some(false) => self.finish(
                &created.thread,
                WorktreeSetupPhase::Failed,
                Some("Workspace preparation failed."),
            ),
            None => {}
        }
        if let Some(ended) = created.ended {
            ended.cancel();
        }
        removed
    }

    async fn current_branch(&self, cwd: PathBuf) -> Result<Option<String>> {
        self.git
            .at(cwd)
            .current_branch()
            .await
            .map_err(git_failure("current-branch"))
    }
}
