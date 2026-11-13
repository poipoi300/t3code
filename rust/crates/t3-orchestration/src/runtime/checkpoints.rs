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
            return Ok(());
        };
        let git = self
            .backend
            .git(PathBuf::from(project.workspace_root))
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
        Ok(())
    }
}

impl Default for GitCheckpointBackend {
    fn default() -> Self {
        Self {
            factory: Arc::new(Git::new),
        }
    }
}

impl GitCheckpointBackend {
    pub fn new(factory: impl Fn(PathBuf) -> Git + Send + Sync + 'static) -> Self {
        Self {
            factory: Arc::new(factory),
        }
    }

    fn git(&self, cwd: PathBuf) -> Git {
        (self.factory)(cwd)
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
            .with_cancellation(cancellation.clone());
        let result = git.capture_checkpoint(&reference.0).await;
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
        let git = self.git(PathBuf::from(&scope.cwd.0));
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
        let git = self.git(cwd);
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
        let git = self.git(cwd).with_cancellation(cancellation.clone());
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
            .prune_checkpoint_refs(&scope_id.0, &keep)
            .await
            .map_err(git_error)?;
        Ok(())
    }
}

#[async_trait]
impl CheckpointMetadata for GitCheckpointBackend {
    fn reference(&self, scope: &CheckpointScopeId, ordinal: u64) -> CheckpointRef {
        CheckpointRef(t3_git::checkpoint_ref(&scope.0, ordinal))
    }

    async fn is_repository(&self, cwd: PathBuf) -> Result<bool> {
        Ok(self
            .git(cwd)
            .detect_repository()
            .await
            .map_err(git_error)?
            .is_some())
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
        t3_git::GitWorkflowService::new(self.backend.git(cwd))
            .repository_root()
            .await
            .map_err(git_error)
    }
    async fn has_commit(&self, cwd: PathBuf, reference: &str) -> Result<bool> {
        self.backend
            .git(cwd)
            .has_commit(reference)
            .await
            .map_err(git_error)
    }
    async fn has_ref(&self, cwd: PathBuf, reference: &str) -> Result<bool> {
        self.backend
            .git(cwd)
            .has_ref_named(reference)
            .await
            .map_err(git_error)
    }
    async fn remote_for_base(&self, cwd: PathBuf, reference: &str) -> Result<Option<String>> {
        self.backend
            .git(cwd)
            .resolve_worktree_base_remote(reference)
            .await
            .map(|target| target.map(|(remote, _)| remote))
            .map_err(git_error)
    }
    async fn local_branches(&self, cwd: PathBuf) -> Result<Vec<String>> {
        self.backend
            .git(cwd)
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
            .git(cwd)
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
            let git = backend.git(root.clone());
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
        let git = repo.backend.git(worktree.clone());
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
}
