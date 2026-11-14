//! The project setup script runner (`apps/server/src/project/ProjectSetupScriptRunner.ts`):
//! `t3_project::scripts::ProjectSetupScriptRunner` over the environment's one project service,
//! thread projections, settings and terminal manager. The script is the project's own
//! configured command, resolved from settings and the project record; it runs in the worktree
//! with `t3_workspace::project`'s runtime environment, in a terminal the user can see and stop.
//! Nothing a client or a forge sends becomes part of the command.
use std::{path::Path, sync::Arc};
use t3_contracts::base_schemas::ThreadId;
use t3_project::scripts::{ProjectSetupScriptRunner, SetupScriptInput};
use t3_workspace::scripts::{RunScriptResult, ScriptRunner, ScriptTrigger};

pub struct ProjectSetupAdapter {
    runner: ProjectSetupScriptRunner,
}

impl ProjectSetupAdapter {
    /// After projects, orchestration and the terminal manager, whose services it composes.
    pub fn start(
        context: &crate::services::DomainContext<'_>,
        projects: &crate::adapters::project::ProjectAdapter,
        orchestration: &crate::adapters::orchestration::OrchestrationAdapter,
        terminal: &crate::adapters::terminal::TerminalAdapter,
    ) -> Option<Arc<Self>> {
        let runtime = orchestration.composed()?;
        Some(Arc::new(Self {
            runner: ProjectSetupScriptRunner {
                projects: projects.service().clone(),
                projections: runtime.projections.clone(),
                settings: context.local.settings.clone(),
                runner: ScriptRunner::new(terminal.manager.clone()),
            },
        }))
    }

    /// The runner thread launch uses (`ThreadLaunchService`: observed completion, trigger
    /// `setup` or `settle`).
    pub fn runner(&self) -> &ProjectSetupScriptRunner {
        &self.runner
    }

    /// `GitManager.preparePullRequestThread`'s `maybeRunSetupScript`: starts the project's setup
    /// script for the thread in the worktree and returns once its terminal is open; the
    /// project is the one rooted at `project_cwd`.
    pub async fn run_for_worktree(
        &self,
        project_cwd: &Path,
        thread_id: &str,
        worktree: &Path,
    ) -> Result<(), t3_project::Error> {
        let thread_id = ThreadId(thread_id.into());
        let project_cwd = project_cwd.to_string_lossy();
        let worktree = worktree.to_string_lossy();
        let result = self
            .runner
            .run_for_thread(SetupScriptInput {
                thread_id: &thread_id,
                project: None,
                project_id: None,
                project_cwd: Some(&project_cwd),
                worktree_path: &worktree,
                trigger: ScriptTrigger::Setup,
                observe_completion: false,
                on_output_line: None,
            })
            .await?;
        if let RunScriptResult::Started(started) = result {
            tracing::debug!(terminal = %started.terminal_id, "project setup script started");
        }
        Ok(())
    }
}

/// The review host's and worktree host's setup step: failures are logged and dropped, as
/// TypeScript's `Effect.catch(logWarning)` does; the worktree is already created.
pub struct WorktreeSetup(pub Arc<ProjectSetupAdapter>);

#[async_trait::async_trait]
impl t3_forge::review_host::ReviewSetup for WorktreeSetup {
    async fn setup(
        &self,
        cwd: &Path,
        thread_id: &str,
        worktree: &Path,
    ) -> Result<(), t3_git::GitError> {
        if let Err(error) = self.0.run_for_worktree(cwd, thread_id, worktree).await {
            tracing::warn!(%error, "GitManager.preparePullRequestThread setup script failed");
        }
        Ok(())
    }
}

/// `vcs.createWorktree`'s folder and submodule settings with the setup step above.
pub struct SetupWorktreeHost {
    pub creation: Arc<dyn t3_git::WorktreeHost>,
    pub setup: WorktreeSetup,
}

#[async_trait::async_trait]
impl t3_git::WorktreeHost for SetupWorktreeHost {
    async fn creation_settings(&self, cwd: &Path) -> t3_git::WorktreeCreationSettings {
        self.creation.creation_settings(cwd).await
    }
    async fn setup(
        &self,
        cwd: &Path,
        thread_id: &str,
        worktree: &Path,
    ) -> Result<(), t3_git::GitError> {
        t3_forge::review_host::ReviewSetup::setup(&self.setup, cwd, thread_id, worktree).await
    }
}
