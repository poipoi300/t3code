#![cfg(feature = "git-backend")]

use crate::{
    api::*,
    error::{Error, Result},
    ports::checkpoints_forks::{
        checkpoint_restore_safety::expanded_id, checkpoint_service::CheckpointMetadata,
    },
};
use async_trait::async_trait;
use std::{collections::HashSet, path::PathBuf, sync::Arc};
use t3_git::{DiffFormat, DiffOptions, Git, GitError};
use tokio_util::sync::CancellationToken;

pub struct GitCheckpointBackend {
    factory: Arc<dyn Fn(PathBuf) -> Git + Send + Sync>,
    bindings: Option<Arc<dyn crate::api::RepositoryBindingStore>>,
}

pub struct GitRollbackWorkspaceRecovery {
    pub backend: Arc<GitCheckpointBackend>,
    pub database: crate::stores::StoreDatabase,
    pub managed_roots: Vec<PathBuf>,
}

#[async_trait]
impl crate::ports::checkpoints_forks::checkpoint_rollback_service::RollbackWorkspaceRecovery
    for GitRollbackWorkspaceRecovery
{
    async fn is_managed(&self, path: &std::path::Path) -> Result<bool> {
        crate::ports::checkpoints_forks::checkpoint_restore_safety::is_managed_worktree_path(
            path,
            &self.managed_roots,
        )
    }

    async fn recreate(
        &self,
        thread: &OrchestrationV2AppThread,
        cancellation: CancellationToken,
    ) -> Result<()> {
        let (Some(path), Some(branch)) = (&thread.worktree_path, &thread.branch) else {
            return Err(Error::Unsupported(
                "worktree recovery without a branch and path",
            ));
        };
        if !self.is_managed(std::path::Path::new(&path.0)).await? {
            return Err(Error::Unsupported("recovery outside managed worktrees"));
        }
        let project_id = expanded_id(&thread.project_id)?;
        let project = self
            .database
            .read(move |db| crate::stores::projects::get(db, &project_id, false))
            .await?;
        let Some(project) = project else {
            // Nothing was created, so nothing is bound (re-review 4 F1).
            return Ok(());
        };
        // Through the project root's saved binding: a repository planted there refuses the
        // recreation instead of supplying the checkout (re-review 4 F6).
        let git = self
            .backend
            .git(PathBuf::from(project.workspace_root))
            .await?
            .with_cancellation(cancellation.clone());
        git.execute(&["worktree", "prune"], Default::default())
            .await
            .map_err(git_error)?;
        git.add_worktree(t3_git::WorktreeOptions::new(
            PathBuf::from(&path.0),
            branch.0.clone(),
        ))
        .await
        .map_err(git_error)?;
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        // The checkout this recovery made: the one trusted rebind (re-review 4 F1).
        self.backend.bind_scope(PathBuf::from(&path.0), true).await
    }
}

/// What the bindings store keeps for a scope: the repository first seen there, or `None` when the
/// folder was not a repository then (re-review 4 F3).
#[derive(serde::Serialize, serde::Deserialize)]
struct SavedBinding {
    repository: Option<t3_git::RepositoryBinding>,
}

impl Default for GitCheckpointBackend {
    fn default() -> Self {
        Self {
            factory: Arc::new(Git::new),
            bindings: None,
        }
    }
}

impl GitCheckpointBackend {
    pub fn new(factory: impl Fn(PathBuf) -> Git + Send + Sync + 'static) -> Self {
        Self {
            factory: Arc::new(factory),
            bindings: None,
        }
    }

    /// Every operation runs against the repository saved for its scope (t3-git H-1). A scope seen
    /// for the first time is bound and saved; a planted repository then refuses, after restarts too.
    pub fn with_repository_bindings(
        mut self,
        bindings: Arc<dyn crate::api::RepositoryBindingStore>,
    ) -> Self {
        self.bindings = Some(bindings);
        self
    }

    /// A handle for reads outside checkpoint operations (launch preparation, recreation).
    fn handle(&self, cwd: PathBuf) -> Git {
        (self.factory)(cwd)
    }

    /// A recovery must use the project repository previously recorded for this environment.
    pub async fn worktree_source(&self, root: PathBuf) -> Result<(Git, t3_git::RepositoryBinding)> {
        let store = self
            .bindings
            .as_ref()
            .ok_or(Error::Unsupported("repository binding store"))?;
        let saved = match store.get(&root).await? {
            Some(saved) => saved,
            None => {
                self.bind_scope(root.clone(), false).await?;
                store
                    .get(&root)
                    .await?
                    .ok_or(Error::Unsupported("missing project binding"))?
            }
        };
        let saved: SavedBinding = serde_json::from_value(saved)?;
        let binding = saved.repository.ok_or(Error::Service {
            operation: crate::ports::checkpoints_forks::checkpoint_rollback_service::REPOSITORY_CHANGED_OPERATION,
            detail: "The project folder was recorded without a Git repository.".into(),
        })?;
        Ok((self.handle(root), binding))
    }

    async fn git(&self, cwd: PathBuf) -> Result<Git> {
        self.bound(cwd, false).await
    }

    async fn bound(&self, cwd: PathBuf, replace: bool) -> Result<Git> {
        let git = (self.factory)(cwd.clone());
        let Some(bindings) = &self.bindings else {
            return Ok(git);
        };
        if !replace && let Some(saved) = bindings.get(&cwd).await? {
            let saved: SavedBinding = serde_json::from_value(saved)?;
            return match saved.repository {
                Some(identity) => git.with_repository_binding(identity).await.map_err(git_error),
                // First seen outside any repository: one created there since is not trusted.
                None => match git.bind_repository().await.map_err(git_error)? {
                    None => Ok(git),
                    Some(_) => Err(Error::Service {
                        operation: crate::ports::checkpoints_forks::checkpoint_rollback_service::REPOSITORY_CHANGED_OPERATION,
                        detail: "A Git repository was created in this folder after T3 first saw it without one. T3 will not restore, capture or read it until you trust that repository.".into(),
                    }),
                },
            };
        }
        let repository = if replace {
            git.rebind_repository().await
        } else {
            git.bind_repository().await
        }
        .map_err(git_error)?;
        bindings
            .put(&cwd, serde_json::to_value(SavedBinding { repository })?)
            .await?;
        Ok(git)
    }

    fn reference(scope: &OrchestrationV2CheckpointScope, ordinal: u64) -> Result<CheckpointRef> {
        Ok(CheckpointRef(t3_git::checkpoint_ref(
            &expanded_id(&scope.id)?.0,
            ordinal,
        )))
    }
}

fn git_error(error: GitError) -> Error {
    match error {
        GitError::Vcs(ref failure) if matches!(&failure.error, t3_contracts::vcs::VcsError::Exit(exit) if exit.detail == t3_git::INDEX_LOCKED_DETAIL) => Error::Service {
            operation: crate::ports::checkpoints_forks::checkpoint_rollback_service::INDEX_LOCKED_OPERATION,
            detail: t3_git::INDEX_LOCKED_DETAIL.into(),
        },
        GitError::Unsupported(_) => Error::Unsupported("checkpoint filesystem capture"),
        // The folder is left out: clients that may roll back see the refusal, not the server's
        // layout (re-review 4 L4).
        GitError::RepositoryChanged { detail, .. } => Error::Service {
            operation: crate::ports::checkpoints_forks::checkpoint_rollback_service::REPOSITORY_CHANGED_OPERATION,
            detail: format!(
                "This folder no longer belongs to the repository this thread started in: {detail} T3 will not restore, capture or read it until the folder is restored."
            ),
        },
        error => Error::Service {
            operation: "checkpoint.git",
            detail: error.to_string(),
        },
    }
}

#[async_trait]
impl CheckpointBackend for GitCheckpointBackend {
    fn checkpoint_ref(&self, scope_id: &CheckpointScopeId, ordinal: u64) -> Result<CheckpointRef> {
        Ok(CheckpointMetadata::reference(self, scope_id, ordinal))
    }

    async fn is_git_repository(&self, cwd: PathBuf) -> Result<bool> {
        CheckpointMetadata::is_repository(self, cwd).await
    }

    async fn delete_checkpoint_refs(
        &self,
        scope: OrchestrationV2CheckpointScope,
        checkpoints: Vec<OrchestrationV2Checkpoint>,
    ) -> Result<()> {
        CheckpointMetadata::delete_refs(self, scope, checkpoints).await
    }

    async fn capture(
        &self,
        input: CaptureCheckpoint,
        cancellation: CancellationToken,
    ) -> Result<CheckpointRef> {
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let reference = Self::reference(&input.scope, input.ordinal)?;
        let cwd = PathBuf::from(&input.scope.cwd.0);
        let git = self
            .git(cwd.clone())
            .await?
            .with_cancellation(cancellation.clone());
        let previous = input
            .ordinal
            .checked_sub(1)
            .map(|ordinal| Self::reference(&input.scope, ordinal))
            .transpose()?;
        let result = git
            .capture_checkpoint_with_previous(
                &reference.0,
                previous.as_ref().map(|reference| reference.0.as_str()),
            )
            .await;
        git.drain_operations().await;
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        result.map_err(git_error)?;
        Ok(reference)
    }

    async fn lookup(
        &self,
        scope: OrchestrationV2CheckpointScope,
        ordinal: u64,
    ) -> Result<Option<CheckpointRef>> {
        let reference = Self::reference(&scope, ordinal)?;
        let git = self.git(PathBuf::from(&scope.cwd.0)).await?;
        if git.detect_repository().await.map_err(git_error)?.is_none() {
            return Ok(None);
        }
        Ok(git
            .has_checkpoint(&reference.0)
            .await
            .map_err(git_error)?
            .then_some(reference))
    }

    async fn diff(
        &self,
        cwd: PathBuf,
        from: CheckpointRef,
        to: CheckpointRef,
    ) -> Result<CheckpointDiff> {
        let git = self.git(cwd).await?;
        let mut options = DiffOptions::new(&from.0, &to.0);
        options.ignore_whitespace = false;
        options.format = DiffFormat::Numstat;
        let numstat = git.diff_checkpoints(options).await.map_err(git_error)?;
        let authored = git
            .list_authored_paths(&from.0, &to.0)
            .await
            .unwrap_or(None);
        let files = t3_git::parse_numstat(&numstat)
            .into_iter()
            .filter(|file| {
                authored.as_ref().is_none_or(|paths| {
                    paths.contains(&file.path)
                        || file
                            .previous_path
                            .as_ref()
                            .is_some_and(|previous| paths.contains(previous))
                })
            })
            .map(|file| OrchestrationV2CheckpointFileSummary {
                path: TrimmedNonEmptyString(file.path),
                kind: TrimmedNonEmptyString("modified".into()),
                additions: file.additions,
                deletions: file.deletions,
            })
            .collect();
        let mut patch_options = DiffOptions::new(&from.0, &to.0);
        patch_options.ignore_whitespace = false;
        let patch = git
            .diff_checkpoints(patch_options)
            .await
            .map_err(git_error)?;
        Ok(CheckpointDiff { files, patch })
    }

    async fn restore(
        &self,
        scope: OrchestrationV2CheckpointScope,
        ordinal: u64,
        cancellation: CancellationToken,
    ) -> Result<()> {
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let reference = Self::reference(&scope, ordinal)?;
        let cwd = PathBuf::from(&scope.cwd.0);
        let git = self.git(cwd).await?.with_cancellation(cancellation.clone());
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let restored = git
            .restore_checkpoint(&reference.0, false)
            .await
            .map_err(git_error)?;
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if !restored {
            return Err(Error::Service {
                operation: "checkpoint.restore",
                detail: "Checkpoint ref is unavailable.".into(),
            });
        }
        Ok(())
    }

    async fn prune(&self, scope: OrchestrationV2CheckpointScope, preserve: Vec<u64>) -> Result<()> {
        let scope_id = expanded_id(&scope.id)?;
        let keep = preserve
            .into_iter()
            .map(|ordinal| t3_git::checkpoint_ref(&scope_id.0, ordinal))
            .collect::<HashSet<_>>();
        self.git(PathBuf::from(scope.cwd.0))
            .await?
            .prune_checkpoint_refs(&scope_id.0, &keep)
            .await
            .map_err(git_error)?;
        Ok(())
    }
}

#[async_trait]
impl CheckpointMetadata for GitCheckpointBackend {
    async fn query_diff(
        &self,
        cwd: PathBuf,
        from: CheckpointRef,
        to: CheckpointRef,
        ignore_whitespace: bool,
    ) -> Result<String> {
        self.git(cwd)
            .await?
            .filtered_checkpoint_diff(&from.0, &to.0, ignore_whitespace, t3_git::DIFF_MAX_BYTES)
            .await
            .map_err(git_error)
    }

    fn reference(&self, scope: &CheckpointScopeId, ordinal: u64) -> CheckpointRef {
        CheckpointRef(t3_git::checkpoint_ref(&scope.0, ordinal))
    }

    async fn is_repository(&self, cwd: PathBuf) -> Result<bool> {
        Ok(self
            .git(cwd)
            .await?
            .detect_repository()
            .await
            .map_err(git_error)?
            .is_some())
    }

    async fn bind_scope(&self, cwd: PathBuf, replace: bool) -> Result<()> {
        self.bound(cwd, replace).await.map(|_| ())
    }

    async fn delete_refs(
        &self,
        scope: OrchestrationV2CheckpointScope,
        checkpoints: Vec<OrchestrationV2Checkpoint>,
    ) -> Result<()> {
        let mut refs = Vec::with_capacity(checkpoints.len());
        for checkpoint in checkpoints {
            if checkpoint.scope_id != scope.id
                || checkpoint.thread_id != scope.thread_id
                || checkpoint.r#ref != Self::reference(&scope, checkpoint.ordinal_within_scope)?
            {
                return Err(Error::Service {
                    operation: "checkpoint.delete-stale-refs",
                    detail: "Checkpoint ref is not owned by this scope".into(),
                });
            }
            refs.push(checkpoint.r#ref.0);
        }
        self.git(PathBuf::from(scope.cwd.0))
            .await?
            .delete_checkpoint_refs(&refs)
            .await
            .map_err(git_error)
    }
}

use crate::ports::checkpoints_forks::thread_launch_service::{
    GeneratedBranch, InitialMessage, LaunchEnvironment, LaunchHost, SetupInput, SetupRun,
};

pub struct GitLaunchEnvironment {
    backend: Arc<GitCheckpointBackend>,
    host: Arc<dyn LaunchHost>,
}
impl GitLaunchEnvironment {
    pub fn new(backend: Arc<GitCheckpointBackend>, host: Arc<dyn LaunchHost>) -> Self {
        Self { backend, host }
    }
}
#[async_trait]
impl LaunchEnvironment for GitLaunchEnvironment {
    async fn is_scratch(&self, project: &ProjectId) -> Result<bool> {
        self.host.is_scratch(project).await
    }
    async fn reject_during_clone(&self, project: &ProjectId) -> Result<()> {
        self.host.reject_during_clone(project).await
    }
    async fn assert_provider_enabled(
        &self,
        project: &ProjectId,
        command: &CommandId,
        instance: &t3_contracts::provider_instance::ProviderInstanceId,
    ) -> Result<()> {
        self.host
            .assert_provider_enabled(project, command, instance)
            .await
    }
    async fn folder_for_thread(
        &self,
        project: &ProjectId,
        thread: &ThreadId,
        text: &str,
    ) -> Result<Option<PathBuf>> {
        self.host.folder_for_thread(project, thread, text).await
    }
    async fn repository_root(&self, cwd: PathBuf) -> Result<Option<PathBuf>> {
        t3_git::GitWorkflowService::new(self.backend.handle(cwd))
            .repository_root()
            .await
            .map_err(git_error)
    }
    async fn has_commit(&self, cwd: PathBuf, reference: &str) -> Result<bool> {
        self.backend
            .handle(cwd)
            .has_commit(reference)
            .await
            .map_err(git_error)
    }
    async fn has_ref(&self, cwd: PathBuf, reference: &str) -> Result<bool> {
        self.backend
            .handle(cwd)
            .has_ref_named(reference)
            .await
            .map_err(git_error)
    }
    async fn remote_for_base(&self, cwd: PathBuf, reference: &str) -> Result<Option<String>> {
        self.backend
            .handle(cwd)
            .resolve_worktree_base_remote(reference)
            .await
            .map(|target| target.map(|(remote, _)| remote))
            .map_err(git_error)
    }
    async fn fetch_worktree_base(
        &self,
        cwd: PathBuf,
        reference: &str,
        remote: &str,
    ) -> Result<bool> {
        let git = self.backend.handle(cwd);
        let Some((_, branch)) = git
            .resolve_worktree_base_remote(reference)
            .await
            .map_err(git_error)?
        else {
            return Ok(false);
        };
        git.fetch_worktree_base_commit(reference, Some((remote, &branch)))
            .await
            .map(|commit| commit.is_some())
            .map_err(git_error)
    }
    async fn local_branches(&self, cwd: PathBuf) -> Result<Vec<String>> {
        self.backend
            .handle(cwd)
            .list_local_branch_names()
            .await
            .map_err(git_error)
    }
    async fn temporary_branch(&self, project: &ProjectId) -> Result<String> {
        self.host.temporary_branch(project).await
    }
    async fn generate_branch(
        &self,
        project: &ProjectId,
        cwd: PathBuf,
        message: &InitialMessage,
        cancellation: CancellationToken,
    ) -> Result<GeneratedBranch> {
        self.host
            .generate_branch(project, cwd, message, cancellation)
            .await
    }
    async fn rename_branch(
        &self,
        cwd: PathBuf,
        old: &str,
        generated: GeneratedBranch,
        cancellation: CancellationToken,
    ) -> Result<String> {
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        self.backend
            .handle(cwd)
            .with_cancellation(cancellation)
            .rename_branch(old, &generated.branch, generated.exact_name)
            .await
            .map_err(git_error)
    }
    async fn setup(&self, input: SetupInput, cancellation: CancellationToken) -> Result<SetupRun> {
        self.host.setup(input, cancellation).await
    }
    async fn close_terminal(&self, thread: &ThreadId, terminal: &str) -> Result<()> {
        self.host.close_terminal(thread, terminal).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::checkpoints_forks::checkpoint_service::{
        PrepareRootScope, prepare_root_run_scope,
    };
    use std::{collections::BTreeMap, ffi::OsString, path::Path};
    use t3_contracts::orchestration_v2_id_table::IdOrIndex;

    struct Repository {
        _temp: tempfile::TempDir,
        root: PathBuf,
        git: Git,
        backend: Arc<GitCheckpointBackend>,
    }
    impl Repository {
        async fn new() -> Result<Self> {
            let temp = tempfile::tempdir()?;
            let root = temp.path().join("repo");
            std::fs::create_dir(&root)?;
            let home = temp.path().join("home");
            std::fs::create_dir(&home)?;
            let mut environment = BTreeMap::<OsString, Option<OsString>>::new();
            for key in ["HOME", "USERPROFILE", "XDG_CONFIG_HOME"] {
                environment.insert(key.into(), Some(home.clone().into_os_string()));
            }
            for key in ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM"] {
                environment.insert(key.into(), Some(home.join("no-config").into_os_string()));
            }
            for (key, value) in [
                ("GIT_CONFIG_NOSYSTEM", "1"),
                ("GIT_CONFIG_COUNT", "0"),
                ("GIT_AUTHOR_NAME", "Test"),
                ("GIT_AUTHOR_EMAIL", "test@example.invalid"),
                ("GIT_COMMITTER_NAME", "Test"),
                ("GIT_COMMITTER_EMAIL", "test@example.invalid"),
            ] {
                environment.insert(key.into(), Some(value.into()));
            }
            for key in ["GIT_CONFIG_PARAMETERS", "GIT_CONFIG", "GIT_TEMPLATE_DIR"] {
                environment.insert(key.into(), None);
            }
            let backend = Arc::new(GitCheckpointBackend::new(move |cwd| {
                Git::new(cwd).with_environment(environment.clone())
            }));
            let git = backend.handle(root.clone());
            let repo = Self {
                _temp: temp,
                root,
                git,
                backend,
            };
            repo.command(&["init", "--initial-branch=main"]).await?;
            repo.command(&["config", "core.autocrlf", "false"]).await?;
            repo.command(&["commit", "--allow-empty", "-m", "initial"])
                .await?;
            Ok(repo)
        }
        async fn command(&self, args: &[&str]) -> Result<String> {
            Ok(self
                .git
                .execute(args, Default::default())
                .await
                .map_err(git_error)?
                .stdout_text())
        }
        fn scope(&self, id: &str) -> OrchestrationV2CheckpointScope {
            let mut scope = prepare_root_run_scope(PrepareRootScope {
                thread_id: ThreadId("thread:test".into()),
                run_id: RunId("run:test".into()),
                root_node_id: NodeId("node:test".into()),
                provider_thread_id: ProviderThreadId("provider:test".into()),
                cwd: self.root.clone(),
                created_at: t3_contracts::wire::WireDateTime(crate::testing::TEST_TIME.into()),
            });
            scope.id = IdOrIndex::Id(CheckpointScopeId(id.into()));
            scope
        }
        async fn capture(
            &self,
            scope: OrchestrationV2CheckpointScope,
            ordinal: u64,
        ) -> Result<CheckpointRef> {
            self.backend
                .capture(
                    CaptureCheckpoint {
                        scope,
                        ordinal,
                        message: "checkpoint".into(),
                    },
                    CancellationToken::new(),
                )
                .await
        }
    }

    fn isolated_git(home: &Path) -> impl Fn(PathBuf) -> Git + Send + Sync + 'static {
        let mut environment = BTreeMap::<OsString, Option<OsString>>::new();
        for key in ["HOME", "USERPROFILE", "XDG_CONFIG_HOME"] {
            environment.insert(key.into(), Some(home.as_os_str().to_owned()));
        }
        for key in ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM"] {
            environment.insert(key.into(), Some(home.join("no-config").into_os_string()));
        }
        for (key, value) in [
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_COUNT", "0"),
            ("GIT_AUTHOR_NAME", "Test"),
            ("GIT_AUTHOR_EMAIL", "test@example.invalid"),
            ("GIT_COMMITTER_NAME", "Test"),
            ("GIT_COMMITTER_EMAIL", "test@example.invalid"),
        ] {
            environment.insert(key.into(), Some(value.into()));
        }
        for key in ["GIT_CONFIG_PARAMETERS", "GIT_CONFIG", "GIT_TEMPLATE_DIR"] {
            environment.insert(key.into(), None);
        }
        move |cwd| Git::new(cwd).with_environment(environment.clone())
    }

    async fn capture_in(
        backend: &GitCheckpointBackend,
        scope: &OrchestrationV2CheckpointScope,
        ordinal: u64,
    ) -> Result<CheckpointRef> {
        backend
            .capture(
                CaptureCheckpoint {
                    scope: scope.clone(),
                    ordinal,
                    message: "checkpoint".into(),
                },
                CancellationToken::new(),
            )
            .await
    }

    #[tokio::test]
    async fn a_recreated_checkout_retries_only_the_identity_whose_save_failed() -> Result<()> {
        let repo = Repository::new().await?;
        let file = repo._temp.path().join("bindings.json");
        let backend = GitCheckpointBackend::new(isolated_git(&repo._temp.path().join("home")))
            .with_repository_bindings(Arc::new(crate::runtime::repository_bindings::FileRepositoryBindings::new(file.clone())));
        let path = repo._temp.path().join("checkout");
        std::fs::create_dir(&path)?;
        backend.bind_scope(path.clone(), false).await?;
        std::fs::remove_dir(&path)?;
        repo.git.add_worktree(t3_git::WorktreeOptions { new_branch: Some("recovered".into()), ..t3_git::WorktreeOptions::new(&path, "HEAD") }).await.map_err(git_error)?;
        let temporary = file.with_extension("json.tmp");
        std::fs::create_dir(&temporary)?;
        assert!(backend.bind_scope(path.clone(), true).await.is_err());
        std::fs::remove_dir(temporary)?;
        assert!(backend.is_repository(path.clone()).await?);
        let saved = std::fs::read_to_string(file)?;
        assert!(saved.contains("common_dir"));
        Ok(())
    }

    const PLANTED_CHILD: &str = "T3_ORCHESTRATION_PLANTED_REPOSITORY_CHILD";

    /// The after-restart half, in a fresh process: t3-git's first-use memory is empty, so only
    /// the saved binding can refuse the planted repository.
    async fn planted_after_restart(temp: PathBuf) -> Result<()> {
        let scope_dir = temp.join("repo").join("pkg");
        let bound = GitCheckpointBackend::new(isolated_git(&temp.join("home")))
            .with_repository_bindings(Arc::new(
                crate::runtime::repository_bindings::FileRepositoryBindings::new(
                    temp.join("state").join("repository-bindings.json"),
                ),
            ));
        let mut scope = prepare_root_run_scope(PrepareRootScope {
            thread_id: ThreadId("thread:test".into()),
            run_id: RunId("run:test".into()),
            root_node_id: NodeId("node:test".into()),
            provider_thread_id: ProviderThreadId("provider:test".into()),
            cwd: scope_dir.clone(),
            created_at: t3_contracts::wire::WireDateTime(crate::testing::TEST_TIME.into()),
        });
        scope.id = IdOrIndex::Id(CheckpointScopeId("scope:planted".into()));
        match capture_in(&bound, &scope, 1).await {
            Err(Error::Service { operation, detail })
                if operation
                    == crate::ports::checkpoints_forks::checkpoint_rollback_service::REPOSITORY_CHANGED_OPERATION =>
            {
                assert!(detail.contains("no longer belongs to the repository"), "{detail}");
                // Re-review 4 L4: clients see the refusal, not the server's folder layout.
                assert!(
                    !detail.contains(&*scope_dir.to_string_lossy()),
                    "{detail}"
                );
            }
            other => panic!("the planted repository must be refused, got {other:?}"),
        }
        assert!(matches!(
            bound.restore(scope.clone(), 0, CancellationToken::new()).await,
            Err(Error::Service { operation, .. })
                if operation
                    == crate::ports::checkpoints_forks::checkpoint_rollback_service::REPOSITORY_CHANGED_OPERATION
        ));
        // Control: without the saved binding the same capture would run in the planted
        // repository, which is the failure the binding prevents.
        let unbound = GitCheckpointBackend::new(isolated_git(&temp.join("home")));
        capture_in(&unbound, &scope, 2).await?;
        Ok(())
    }

    /// Review t3-git H-1: a scope bound when its project was bound refuses a repository planted
    /// in it later, after a server restart too.
    #[tokio::test]
    async fn a_repository_planted_after_binding_is_refused_after_a_restart() -> Result<()> {
        if let Some(temp) = std::env::var_os(PLANTED_CHILD) {
            return planted_after_restart(PathBuf::from(temp)).await;
        }
        let repo = Repository::new().await?;
        let temp = repo.root.parent().ok_or(Error::Stale)?.to_path_buf();
        let scope_dir = repo.root.join("pkg");
        std::fs::create_dir(&scope_dir)?;
        std::fs::write(scope_dir.join("f"), "user work\n")?;
        repo.command(&["add", "pkg/f"]).await?;
        repo.command(&["commit", "-m", "pkg"]).await?;
        let bound = GitCheckpointBackend::new(isolated_git(&temp.join("home")))
            .with_repository_bindings(Arc::new(
                crate::runtime::repository_bindings::FileRepositoryBindings::new(
                    temp.join("state").join("repository-bindings.json"),
                ),
            ));
        bound.bind_scope(scope_dir.clone(), false).await?;
        let planted = isolated_git(&temp.join("home"))(scope_dir.clone());
        for args in [
            &["init", "--initial-branch=main"][..],
            &["add", "f"],
            &["commit", "-m", "planted"],
        ] {
            planted
                .execute(args, Default::default())
                .await
                .map_err(git_error)?;
        }
        let output = std::process::Command::new(std::env::current_exe()?)
            .args([
                "runtime::checkpoints::tests::a_repository_planted_after_binding_is_refused_after_a_restart",
                "--exact",
                "--nocapture",
            ])
            .env(PLANTED_CHILD, &temp)
            .output()?;
        assert!(
            output.status.success(),
            "after restart: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        Ok(())
    }

    #[tokio::test]
    async fn git_backend_capture_diff_restore_and_scoped_prune_preserve_other_homes() -> Result<()>
    {
        let repo = Repository::new().await?;
        let own = repo.scope("scope:own");
        let foreign = repo.scope("scope:another-home");
        std::fs::write(repo.root.join("file.txt"), "before\n")?;
        repo.command(&["add", "file.txt"]).await?;
        let index = std::fs::read(repo.root.join(".git/index"))?;
        let head = repo.command(&["rev-parse", "HEAD"]).await?;
        let first = repo.capture(own.clone(), 0).await?;
        let other = repo.capture(foreign.clone(), 0).await?;
        assert_eq!(std::fs::read(repo.root.join(".git/index"))?, index);
        assert_eq!(repo.command(&["rev-parse", "HEAD"]).await?, head);
        std::fs::write(repo.root.join("file.txt"), "after\nextra\n")?;
        let second = repo.capture(own.clone(), 1).await?;
        assert_eq!(
            repo.backend.lookup(own.clone(), 0).await?,
            Some(first.clone())
        );
        let diff = repo
            .backend
            .diff(repo.root.clone(), first.clone(), second.clone())
            .await?;
        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].path.0, "file.txt");
        assert_eq!(diff.files[0].kind.0, "modified");
        assert_eq!(diff.files[0].additions, 2);
        assert_eq!(diff.files[0].deletions, 1);
        assert!(diff.patch.contains("+after"));
        std::fs::write(repo.root.join("untracked.txt"), "remove")?;
        repo.backend
            .restore(own.clone(), 0, CancellationToken::new())
            .await?;
        assert_eq!(
            std::fs::read_to_string(repo.root.join("file.txt"))?,
            "before\n"
        );
        assert!(!repo.root.join("untracked.txt").exists());
        assert_eq!(repo.command(&["rev-parse", "HEAD"]).await?, head);
        repo.backend.prune(own.clone(), vec![0]).await?;
        assert!(repo.git.has_checkpoint(&first.0).await.map_err(git_error)?);
        assert!(
            !repo
                .git
                .has_checkpoint(&second.0)
                .await
                .map_err(git_error)?
        );
        assert!(repo.git.has_checkpoint(&other.0).await.map_err(git_error)?);
        assert!(
            repo.backend
                .delete_refs(
                    own.clone(),
                    vec![OrchestrationV2Checkpoint {
                        id: IdOrIndex::Id(CheckpointId("foreign".into())),
                        thread_id: own.thread_id.clone(),
                        scope_id: foreign.id,
                        run_id: None,
                        node_id: own.node_id.clone(),
                        parent_checkpoint_id: None,
                        ordinal_within_scope: 0,
                        app_run_ordinal: None,
                        r#ref: other.clone(),
                        status: CheckpointStatus::Ready,
                        files: vec![],
                        files_total: None,
                        captured_at: own.created_at
                    }]
                )
                .await
                .is_err()
        );
        assert!(repo.git.has_checkpoint(&other.0).await.map_err(git_error)?);
        Ok(())
    }

    #[tokio::test]
    async fn missing_checkpoint_never_falls_back_to_head_and_cancellation_does_not_write()
    -> Result<()> {
        let repo = Repository::new().await?;
        let scope = repo.scope("scope:missing");
        let file = repo.root.join("keep.txt");
        std::fs::write(&file, "keep")?;
        assert!(
            repo.backend
                .restore(scope.clone(), 99, CancellationToken::new())
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&file)?, "keep");
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(matches!(
            repo.backend
                .capture(
                    CaptureCheckpoint {
                        scope: scope.clone(),
                        ordinal: 0,
                        message: String::new()
                    },
                    cancellation.clone()
                )
                .await,
            Err(Error::Cancelled)
        ));
        assert!(matches!(
            repo.backend.restore(scope.clone(), 0, cancellation).await,
            Err(Error::Cancelled)
        ));
        assert_eq!(repo.backend.lookup(scope, 0).await?, None);
        assert_eq!(std::fs::read_to_string(file)?, "keep");
        Ok(())
    }

    #[cfg(windows)]
    async fn junction(link: &Path, target: &Path) -> Result<()> {
        let output = Git::new(link.parent().expect("parent"))
            .with_program("cmd.exe")
            .execute(
                &[
                    "/d",
                    "/c",
                    "mklink",
                    "/J",
                    &link.to_string_lossy(),
                    &target.to_string_lossy(),
                ],
                Default::default(),
            )
            .await
            .map_err(git_error)?;
        assert_eq!(output.code, Some(0));
        Ok(())
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn junction_matrix_skips_ignored_links_and_preserves_restore_refusal() -> Result<()> {
        for (link_name, ignored, external) in [
            ("escape", false, true),
            ("node_modules", true, true),
            ("internal-deps", false, false),
            ("internal-deps", true, false),
        ] {
            let repo = Repository::new().await?;
            let scope = repo.scope("scope:junction");
            std::fs::write(repo.root.join("file.txt"), "tracked workspace content")?;
            let escape = repo.root.join(link_name);
            std::fs::create_dir(&escape)?;
            std::fs::write(escape.join("owned.txt"), "checkpoint content")?;
            let outside = if external {
                repo._temp.path().join("outside")
            } else {
                repo.root.join("dependency-store")
            };
            std::fs::create_dir(&outside)?;
            let file = outside.join("owned.txt");
            std::fs::write(&file, "outside original")?;
            std::fs::write(outside.join("new.txt"), "outside new work")?;
            if ignored {
                std::fs::write(repo.root.join(".gitignore"), format!("{link_name}/\n"))?;
            }
            repo.capture(scope.clone(), 0).await?;
            std::fs::remove_dir_all(&escape)?;
            junction(&escape, &outside).await?;
            let reference = t3_git::checkpoint_ref("scope:junction", 1);
            repo.git
                .capture_checkpoint(&reference)
                .await
                .map_err(git_error)?;
            assert!(
                repo.git
                    .has_checkpoint(&reference)
                    .await
                    .map_err(git_error)?
            );
            let entries = repo
                .command(&["ls-tree", "-r", "--name-only", &reference])
                .await?;
            assert!(
                !entries
                    .lines()
                    .any(|path| path == link_name || path.starts_with(&format!("{link_name}/"))),
                "junction target leaked into checkpoint: {entries}"
            );
            assert!(entries.lines().any(|path| path == "file.txt"));
            if ignored {
                let probe_env = BTreeMap::from([(
                    OsString::from("GIT_INDEX_FILE"),
                    Some(repo._temp.path().join("probe-index").into_os_string()),
                )]);
                repo.git
                    .execute(
                        &["read-tree", "HEAD"],
                        t3_git::ExecuteOptions {
                            env: probe_env.clone(),
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(git_error)?;
                // TypeScript's first stageFiles([]) uses no junction exclusion pathspec.
                let ts_stage = repo
                    .git
                    .execute(
                        &["-c", "core.safecrlf=false", "add", "-A", "--", "."],
                        t3_git::ExecuteOptions {
                            env: probe_env.clone(),
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(git_error)?;
                assert_eq!(ts_stage.code, Some(0));
                let ts_tree = repo
                    .git
                    .execute(
                        &["write-tree"],
                        t3_git::ExecuteOptions {
                            env: probe_env,
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(git_error)?
                    .stdout_text()
                    .trim()
                    .to_owned();
                let ts_paths = repo
                    .command(&["ls-tree", "-r", "--name-only", &ts_tree])
                    .await?;
                assert!(!ts_paths.lines().any(|path|path==link_name||path.starts_with(&format!("{link_name}/"))));
                assert!(ts_paths.lines().any(|path| path == "file.txt"));
                assert_eq!(std::fs::read_to_string(&file)?, "outside original");
                eprintln!(
                    "TypeScript stageFiles([]): skipped ignored {link_name} junction; target unchanged"
                );
            }
            let before = repo.command(&["rev-parse", "HEAD"]).await?;
            let index = std::fs::read(repo.root.join(".git/index"))?;
            std::fs::write(repo.root.join("local.txt"), "untracked user work")?;
            assert!(
                repo.backend
                    .restore(scope, 0, CancellationToken::new())
                    .await
                    .is_err()
            );
            assert_eq!(repo.command(&["rev-parse", "HEAD"]).await?, before);
            assert_eq!(std::fs::read(repo.root.join(".git/index"))?, index);
            assert_eq!(
                std::fs::read_to_string(repo.root.join("local.txt"))?,
                "untracked user work"
            );
            assert_eq!(std::fs::read_to_string(file)?, "outside original");
            assert_eq!(
                std::fs::read_to_string(outside.join("new.txt"))?,
                "outside new work"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn scoped_restore_does_not_scan_or_change_sibling_workspaces() -> Result<()> {
        let repo = Repository::new().await?;
        let nested = repo.root.join("nested");
        std::fs::create_dir(&nested)?;
        std::fs::write(nested.join("file.txt"), "before")?;
        std::fs::write(repo.root.join("sibling.txt"), "sibling")?;
        #[cfg(windows)]
        {
            let outside = repo._temp.path().join("sibling-store");
            std::fs::create_dir(&outside)?;
            junction(&repo.root.join("sibling-dependencies"), &outside).await?;
        }
        let mut scope = repo.scope("scope:nested");
        scope.cwd = TrimmedNonEmptyString(nested.to_string_lossy().into_owned());
        repo.capture(scope.clone(), 0).await?;
        std::fs::write(nested.join("file.txt"), "after")?;
        std::fs::write(repo.root.join("sibling.txt"), "sibling changed")?;
        repo.backend
            .restore(scope, 0, CancellationToken::new())
            .await?;
        assert_eq!(std::fs::read_to_string(nested.join("file.txt"))?, "before");
        assert_eq!(
            std::fs::read_to_string(repo.root.join("sibling.txt"))?,
            "sibling changed"
        );
        Ok(())
    }

    struct Host;
    #[async_trait]
    impl LaunchHost for Host {
        async fn is_scratch(&self, _: &ProjectId) -> Result<bool> {
            Ok(false)
        }
        async fn reject_during_clone(&self, _: &ProjectId) -> Result<()> {
            Ok(())
        }
        async fn assert_provider_enabled(
            &self,
            _: &ProjectId,
            _: &CommandId,
            _: &t3_contracts::provider_instance::ProviderInstanceId,
        ) -> Result<()> {
            Ok(())
        }
        async fn folder_for_thread(
            &self,
            _: &ProjectId,
            _: &ThreadId,
            _: &str,
        ) -> Result<Option<PathBuf>> {
            Ok(None)
        }
        async fn temporary_branch(&self, _: &ProjectId) -> Result<String> {
            Ok("t3/1234abcd".into())
        }
        async fn generate_branch(
            &self,
            _: &ProjectId,
            _: PathBuf,
            _: &InitialMessage,
            _: CancellationToken,
        ) -> Result<GeneratedBranch> {
            Err(Error::Unsupported("test naming"))
        }
        async fn setup(&self, _: SetupInput, _: CancellationToken) -> Result<SetupRun> {
            Ok(SetupRun::Skipped)
        }
        async fn close_terminal(&self, _: &ThreadId, _: &str) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn launch_git_adapter_resolves_bases_and_preserves_exact_branch_names() -> Result<()> {
        let repo = Repository::new().await?;
        let environment = GitLaunchEnvironment::new(repo.backend.clone(), Arc::new(Host));
        let original = repo
            .git
            .current_branch()
            .await
            .map_err(git_error)?
            .expect("branch");
        repo.command(&["branch", "existing"]).await?;
        assert!(environment.has_commit(repo.root.clone(), "HEAD").await?);
        assert!(!environment.has_commit(repo.root.clone(), "missing").await?);
        assert!(environment.has_ref(repo.root.clone(), "existing").await?);
        assert!(!environment.has_ref(repo.root.clone(), "missing").await?);
        assert_eq!(
            std::fs::canonicalize(
                environment
                    .repository_root(repo.root.clone())
                    .await?
                    .expect("root")
            )?,
            std::fs::canonicalize(&repo.root)?
        );
        assert!(
            environment
                .remote_for_base(repo.root.clone(), "main")
                .await?
                .is_none()
        );
        repo.command(&[
            "remote",
            "add",
            "origin",
            "https://example.invalid/repository.git",
        ])
        .await?;
        assert_eq!(
            environment
                .remote_for_base(repo.root.clone(), "origin/main")
                .await?
                .as_deref(),
            Some("origin")
        );
        let generated = |exact_name| GeneratedBranch {
            branch: "existing".into(),
            exact_name,
        };
        assert!(
            environment
                .rename_branch(
                    repo.root.clone(),
                    &original,
                    generated(true),
                    CancellationToken::new()
                )
                .await
                .is_err()
        );
        assert_eq!(
            repo.git
                .current_branch()
                .await
                .map_err(git_error)?
                .as_deref(),
            Some(original.as_str())
        );
        let renamed = environment
            .rename_branch(
                repo.root.clone(),
                &original,
                generated(false),
                CancellationToken::new(),
            )
            .await?;
        assert_eq!(renamed, "existing-1");
        assert_eq!(
            repo.git
                .current_branch()
                .await
                .map_err(git_error)?
                .as_deref(),
            Some("existing-1")
        );
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(matches!(
            environment
                .rename_branch(
                    repo.root.clone(),
                    "existing-1",
                    GeneratedBranch {
                        branch: "cancelled".into(),
                        exact_name: true
                    },
                    cancelled
                )
                .await,
            Err(Error::Cancelled)
        ));
        assert!(
            !environment
                .local_branches(repo.root.clone())
                .await?
                .iter()
                .any(|branch| branch == "cancelled")
        );
        Ok(())
    }

    #[test]
    fn checkpoint_ref_is_the_typescript_hash_encoding() {
        assert_eq!(
            t3_git::checkpoint_ref("scope:test", 0),
            "refs/t3/orchestration-v2/checkpoints/MzMxZTk4MGJlNzEyOGJmMDFkYjI4NTI0YjRkZDQxMjg/ordinal/0"
        );
    }

    #[tokio::test]
    async fn managed_worktree_recovery_reuses_the_recorded_branch_and_common_checkpoint_refs()
    -> Result<()> {
        use crate::ports::checkpoints_forks::checkpoint_rollback_service::RollbackWorkspaceRecovery;
        let repo = Repository::new().await?;
        let database = crate::testing::TestDatabase::memory()?;
        let mut event = serde_json::to_value(crate::testing::project_event(
            "project:event",
            "project:test",
        )?)?;
        event["payload"]["workspaceRoot"] = serde_json::to_value(&repo.root)?;
        let event = serde_json::from_value(event)?;
        database
            .stores
            .transaction(move |db| crate::stores::projects::apply(db, &event))
            .await?;
        let managed = repo._temp.path().join("managed");
        let worktree = managed.join("repo").join("feature");
        let recovery = GitRollbackWorkspaceRecovery {
            backend: repo.backend.clone(),
            database: database.stores,
            managed_roots: vec![managed],
        };
        let mut thread: OrchestrationV2AppThread = serde_json::from_str(
            include_str!("../../../../fixtures/samples/OrchestrationV2AppThread.jsonl")
                .lines()
                .next()
                .expect("thread fixture"),
        )?;
        thread.project_id = IdOrIndex::Id(ProjectId("project:test".into()));
        thread.branch = Some(TrimmedNonEmptyString("feature".into()));
        thread.worktree_path = Some(TrimmedNonEmptyString(
            worktree.to_string_lossy().into_owned(),
        ));
        repo.command(&["branch", "feature"]).await?;
        recovery.recreate(&thread, CancellationToken::new()).await?;
        assert!(worktree.join(".git").is_file());
        let git = repo.backend.handle(worktree.clone());
        assert_eq!(
            git.current_branch().await.map_err(git_error)?,
            Some("feature".into())
        );
        let mut scope = repo.scope("scope:recovery");
        scope.cwd = TrimmedNonEmptyString(worktree.to_string_lossy().into_owned());
        std::fs::write(worktree.join("file.txt"), "snapshot")?;
        let reference = repo.capture(scope.clone(), 0).await?;
        repo.git
            .remove_worktree(&worktree, true)
            .await
            .map_err(git_error)?;
        assert!(!worktree.exists());
        recovery.recreate(&thread, CancellationToken::new()).await?;
        repo.backend
            .restore(scope, 0, CancellationToken::new())
            .await?;
        assert_eq!(
            std::fs::read_to_string(worktree.join("file.txt"))?,
            "snapshot"
        );
        assert!(
            repo.git
                .has_checkpoint(&reference.0)
                .await
                .map_err(git_error)?
        );
        Ok(())
    }
    #[tokio::test]
    async fn backend_supplies_the_previous_scope_ref_for_branch_aware_query_diffs() -> Result<()> {
        let repo = Repository::new().await?;
        std::fs::write(repo.root.join("f.txt"), "base\n")?;
        repo.command(&["add", "."]).await?;
        repo.command(&["commit", "-m", "base"]).await?;
        repo.command(&["branch", "b"]).await?;
        repo.command(&["checkout", "-b", "a"]).await?;
        std::fs::write(repo.root.join("only-a.txt"), "branch A\n")?;
        repo.command(&["add", "."]).await?;
        repo.command(&["commit", "-m", "a"]).await?;
        let scope = repo.scope("scope:branch-query");
        let from = repo.capture(scope.clone(), 0).await?;
        repo.command(&["checkout", "b"]).await?;
        std::fs::write(repo.root.join("f.txt"), "agent edit\n")?;
        let to = repo.capture(scope, 1).await?;
        let patch = repo
            .backend
            .query_diff(repo.root.clone(), from, to.clone(), false)
            .await?;
        assert!(patch.contains("-base\n+agent edit\n"));
        assert!(!patch.contains("only-a.txt"));
        assert_eq!(
            repo.command(&["log", "-1", "--format=%P", &to.0])
                .await?
                .split_whitespace()
                .count(),
            2
        );
        Ok(())
    }

    /// Plain `git` with the isolated configuration, as an agent runs it in its folder (t3-git
    /// would apply this process's saved bindings instead).
    fn agent_git(home: &Path, cwd: &Path, args: &[&str]) -> Result<()> {
        let mut command = std::process::Command::new("git");
        command.current_dir(cwd).args(args);
        for key in ["HOME", "USERPROFILE", "XDG_CONFIG_HOME"] {
            command.env(key, home);
        }
        for key in ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM"] {
            command.env(key, home.join("no-config"));
        }
        for (key, value) in [
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "Agent"),
            ("GIT_AUTHOR_EMAIL", "agent@example.invalid"),
            ("GIT_COMMITTER_NAME", "Agent"),
            ("GIT_COMMITTER_EMAIL", "agent@example.invalid"),
        ] {
            command.env(key, value);
        }
        for key in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_COMMON_DIR",
            "GIT_INDEX_FILE",
            "GIT_CONFIG_PARAMETERS",
            "GIT_CONFIG_COUNT",
        ] {
            command.env_remove(key);
        }
        let output = command.output()?;
        if !output.status.success() {
            return Err(Error::Service {
                operation: "agent git",
                detail: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(())
    }

    /// An agent's repository in `cwd` whose clean filter leaves `marker` whenever Git runs it.
    fn plant_filtered_repository(home: &Path, cwd: &Path, marker: &Path) -> Result<()> {
        agent_git(home, cwd, &["init", "--initial-branch=main"])?;
        let target = marker.to_string_lossy().replace('\\', "/");
        agent_git(
            home,
            cwd,
            &[
                "config",
                "filter.planted.clean",
                &format!("touch '{target}'; cat"),
            ],
        )?;
        std::fs::write(cwd.join(".gitattributes"), "* filter=planted\n")?;
        agent_git(home, cwd, &["add", "-A"])?;
        agent_git(home, cwd, &["commit", "-m", "planted"])?;
        if marker.exists() {
            std::fs::remove_file(marker)?;
        }
        Ok(())
    }

    fn scope_at(cwd: &Path, id: &str) -> OrchestrationV2CheckpointScope {
        let mut scope = prepare_root_run_scope(PrepareRootScope {
            thread_id: ThreadId("thread:test".into()),
            run_id: RunId("run:test".into()),
            root_node_id: NodeId("node:test".into()),
            provider_thread_id: ProviderThreadId("provider:test".into()),
            cwd: cwd.to_path_buf(),
            created_at: t3_contracts::wire::WireDateTime(crate::testing::TEST_TIME.into()),
        });
        scope.id = IdOrIndex::Id(CheckpointScopeId(id.into()));
        scope
    }

    fn bindings_at(
        path: PathBuf,
    ) -> Arc<crate::runtime::repository_bindings::FileRepositoryBindings> {
        Arc::new(crate::runtime::repository_bindings::FileRepositoryBindings::new(path))
    }

    fn refused_as_changed<T: std::fmt::Debug>(result: Result<T>) -> String {
        match result {
            Err(Error::Service { operation, detail })
                if operation
                    == crate::ports::checkpoints_forks::checkpoint_rollback_service::REPOSITORY_CHANGED_OPERATION =>
            {
                detail
            }
            other => panic!("expected the changed-repository refusal, got {other:?}"),
        }
    }

    /// Re-review 4 F3: a folder first seen outside any repository is recorded so. A repository
    /// an agent creates there later is refused, and its filters never run as the server user,
    /// until the user trusts it (`bind_scope(.., true)`).
    #[tokio::test]
    async fn a_repository_created_after_first_sight_is_refused_until_trusted() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("home");
        std::fs::create_dir(&home)?;
        let folder = temp.path().join("plain");
        std::fs::create_dir(&folder)?;
        std::fs::write(folder.join("notes.txt"), "user work\n")?;
        let backend = GitCheckpointBackend::new(isolated_git(&home)).with_repository_bindings(
            bindings_at(temp.path().join("state").join("repository-bindings.json")),
        );
        backend.bind_scope(folder.clone(), false).await?;
        let marker = temp.path().join("filter-ran");
        plant_filtered_repository(&home, &folder, &marker)?;
        let scope = scope_at(&folder, "scope:first-sight");
        refused_as_changed(capture_in(&backend, &scope, 1).await);
        assert!(
            !marker.exists(),
            "the agent's filter ran as the server user"
        );
        refused_as_changed(backend.is_repository(folder.clone()).await);
        backend.bind_scope(folder.clone(), true).await?;
        capture_in(&backend, &scope, 2).await?;
        Ok(())
    }

    fn project_thread(project: &str, worktree: &Path) -> Result<OrchestrationV2AppThread> {
        let mut thread: OrchestrationV2AppThread = serde_json::from_str(
            include_str!("../../../../fixtures/samples/OrchestrationV2AppThread.jsonl")
                .lines()
                .next()
                .expect("thread fixture"),
        )?;
        thread.project_id = IdOrIndex::Id(ProjectId(project.into()));
        thread.branch = Some(TrimmedNonEmptyString("feature".into()));
        thread.worktree_path = Some(TrimmedNonEmptyString(
            worktree.to_string_lossy().into_owned(),
        ));
        Ok(thread)
    }

    async fn project_rooted_at(root: &Path) -> Result<crate::stores::StoreDatabase> {
        let database = crate::testing::TestDatabase::memory()?;
        let mut event = serde_json::to_value(crate::testing::project_event(
            "project:event",
            "project:test",
        )?)?;
        event["payload"]["workspaceRoot"] = serde_json::to_value(root)?;
        let event = serde_json::from_value(event)?;
        database
            .stores
            .transaction(move |db| crate::stores::projects::apply(db, &event))
            .await?;
        Ok(database.stores)
    }

    /// Re-review 4 F6: a missing worktree is recreated through the project root's saved binding.
    /// A repository planted at the project root refuses the recreation instead of supplying it.
    #[tokio::test]
    async fn worktree_recreation_refuses_a_repository_planted_at_the_project_root() -> Result<()> {
        use crate::ports::checkpoints_forks::checkpoint_rollback_service::RollbackWorkspaceRecovery;
        let repo = Repository::new().await?;
        let temp = repo._temp.path().to_path_buf();
        let home = temp.join("home");
        let project_root = repo.root.join("pkg");
        std::fs::create_dir(&project_root)?;
        std::fs::write(project_root.join("f"), "user work\n")?;
        repo.command(&["add", "pkg/f"]).await?;
        repo.command(&["commit", "-m", "pkg"]).await?;
        repo.command(&["branch", "feature"]).await?;
        let bound = Arc::new(
            GitCheckpointBackend::new(isolated_git(&home))
                .with_repository_bindings(bindings_at(temp.join("state").join("bindings.json"))),
        );
        bound.bind_scope(project_root.clone(), false).await?;
        plant_filtered_repository(&home, &project_root, &temp.join("filter-ran"))?;
        agent_git(&home, &project_root, &["branch", "feature"])?;
        let managed = temp.join("managed");
        let worktree = managed.join("repo").join("feature");
        let recovery = GitRollbackWorkspaceRecovery {
            backend: bound,
            database: project_rooted_at(&project_root).await?,
            managed_roots: vec![managed],
        };
        refused_as_changed(
            recovery
                .recreate(
                    &project_thread("project:test", &worktree)?,
                    CancellationToken::new(),
                )
                .await,
        );
        assert!(!worktree.exists());
        Ok(())
    }

    /// Re-review 4 F1: the recovery that recreated a worktree rebinds it (the trusted case); one
    /// whose project is gone creates nothing and binds nothing.
    #[tokio::test]
    async fn a_recreated_worktree_is_rebound_and_a_missing_project_binds_nothing() -> Result<()> {
        use crate::{
            api::RepositoryBindingStore,
            ports::checkpoints_forks::checkpoint_rollback_service::RollbackWorkspaceRecovery,
        };
        let repo = Repository::new().await?;
        let temp = repo._temp.path().to_path_buf();
        repo.command(&["branch", "feature"]).await?;
        let store = bindings_at(temp.join("state").join("bindings.json"));
        let bound = Arc::new(
            GitCheckpointBackend::new(isolated_git(&temp.join("home")))
                .with_repository_bindings(store.clone()),
        );
        let managed = temp.join("managed");
        let recovery = GitRollbackWorkspaceRecovery {
            backend: bound,
            database: project_rooted_at(&repo.root).await?,
            managed_roots: vec![managed.clone()],
        };
        let worktree = managed.join("repo").join("feature");
        recovery
            .recreate(
                &project_thread("project:test", &worktree)?,
                CancellationToken::new(),
            )
            .await?;
        assert!(worktree.join(".git").is_file());
        assert!(store.get(&worktree).await?.is_some(), "not rebound");
        let other = managed.join("repo").join("other");
        recovery
            .recreate(
                &project_thread("project:gone", &other)?,
                CancellationToken::new(),
            )
            .await?;
        assert!(!other.exists());
        assert!(store.get(&other).await?.is_none());
        Ok(())
    }
}
