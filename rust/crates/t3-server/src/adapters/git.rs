//! Git service boundary. Workspace grants come from authoritative project state.
use futures::future::BoxFuture;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_contracts::{auth::*, base_schemas::AuthSessionId, git::*, review::*, vcs::*};
pub use t3_git::{ExecuteOptions, Git, WorktreeOptions};
use t3_git::{GitError, VcsDriverRegistry, VcsSelection, resolve_vcs_kind};

/// Resolve current grants on every request, including worktree removal targets.
/// Implementations must never derive grants from client paths or repository discovery.
pub trait WorkspaceAuthority: Send + Sync {
    fn roots<'a>(
        &'a self,
        session: &'a AuthSessionId,
    ) -> BoxFuture<'a, Result<Vec<PathBuf>, EnvironmentAuthorizationError>>;
}

pub struct GitAdapter {
    git: Git,
    authority: Option<Arc<dyn WorkspaceAuthority>>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Project workspace access denied")]
    Authorization(Box<EnvironmentAuthorizationError>),
    #[error("Git operation failed")]
    Git(Box<GitCommandError>),
    #[error("VCS operation failed")]
    Vcs(Box<VcsError>),
}

impl GitAdapter {
    /// Startup remains closed until the composition root can supply project/session grants.
    #[cfg(feature = "local-services")]
    pub fn start(context: &crate::services::DomainContext<'_>) -> Arc<Self> {
        Self::from_context(context, None)
    }

    #[cfg(feature = "local-services")]
    pub fn start_with_authority(
        context: &crate::services::DomainContext<'_>,
        authority: Arc<dyn WorkspaceAuthority>,
    ) -> Arc<Self> {
        Self::from_context(context, Some(authority))
    }

    #[cfg(feature = "local-services")]
    fn from_context(
        context: &crate::services::DomainContext<'_>,
        authority: Option<Arc<dyn WorkspaceAuthority>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            git: Git::new(&context.local.config.cwd)
                .with_environment(
                    context
                        .environment
                        .iter()
                        .map(|(key, value)| (key.into(), Some(value.into())))
                        .collect(),
                )
                .with_cancellation(context.shutdown.child_token())
                .with_internal_paths(
                    context.local.config.paths.state_dir.clone(),
                    context.local.config.paths.secrets(),
                ),
            authority,
        })
    }

    /// The template carries the environment's process policy and cancellation token.
    pub fn new(
        git: Git,
        authority: Arc<dyn WorkspaceAuthority>,
        state_dir: PathBuf,
        secrets_dir: PathBuf,
    ) -> Self {
        Self {
            git: git.with_internal_paths(state_dir, secrets_dir),
            authority: Some(authority),
        }
    }

    /// Internal composition access; RPCs must resolve their checkout through the authority.
    pub fn at(&self, cwd: impl Into<PathBuf>) -> Git {
        self.git.at(cwd)
    }

    pub fn is_available(&self) -> bool {
        self.authority.is_some()
    }

    async fn directory(
        &self,
        session: &AuthSessionId,
        cwd: &str,
    ) -> Result<(PathBuf, Vec<PathBuf>), Error> {
        let grants = self
            .authority
            .as_ref()
            .ok_or_else(denied)?
            .roots(session)
            .await
            .map_err(|e| Error::Authorization(Box::new(e)))?;
        if !Path::new(cwd).is_absolute() {
            return Err(denied());
        }
        let cwd = tokio::fs::canonicalize(cwd).await.map_err(|_| denied())?;
        if !tokio::fs::metadata(&cwd).await.is_ok_and(|m| m.is_dir()) {
            return Err(denied());
        }
        let mut roots = Vec::new();
        for grant in grants {
            if grant.is_absolute()
                && let Ok(root) = tokio::fs::canonicalize(grant).await
                && cwd.starts_with(&root)
            {
                roots.push(root);
            }
        }
        if roots.is_empty() {
            return Err(denied());
        }
        Ok((cwd, roots))
    }

    async fn checkout(
        &self,
        session: &AuthSessionId,
        cwd: &str,
        operation: &str,
    ) -> Result<Git, Error> {
        let (cwd, roots) = self.directory(session, cwd).await?;
        let git = self.git.at(cwd);
        let (selection, _) = resolve_vcs_kind(git.cwd(), VcsSelection::Auto).await;
        if let VcsSelection::Kind(kind) = selection
            && kind != VcsDriverKind::Unknown
        {
            VcsDriverRegistry::new(git.clone())
                .get(kind)
                .map_err(|e| error(e, operation))?;
        }
        if let Some(repository) = git
            .detect_repository()
            .await
            .map_err(|e| error(e, operation))?
        {
            let repository_root = tokio::fs::canonicalize(repository.root)
                .await
                .map_err(|_| denied())?;
            // Git discovers ancestors; a project subdirectory alone cannot authorize
            // repository-wide mutations or reads in its parent checkout.
            if !roots.iter().any(|root| repository_root.starts_with(root)) {
                return Err(denied());
            }
        }
        Ok(git)
    }

    pub async fn pull(
        &self,
        session: &AuthSessionId,
        input: VcsPullInput,
    ) -> Result<VcsPullResult, Error> {
        self.checkout(session, &input.cwd.0, "vcs.pull")
            .await?
            .pull_current_branch()
            .await
            .map_err(|e| error(e, "vcs.pull"))
    }

    pub async fn list_refs(
        &self,
        session: &AuthSessionId,
        mut input: VcsListRefsInput,
    ) -> Result<VcsListRefsResult, Error> {
        let git = self.checkout(session, &input.cwd.0, "vcs.listRefs").await?;
        input.cwd = text(git.cwd())?;
        git.list_branches(&input)
            .await
            .map_err(|e| error(e, "vcs.listRefs"))
    }

    pub async fn create_ref(
        &self,
        session: &AuthSessionId,
        input: VcsCreateRefInput,
    ) -> Result<VcsCreateRefResult, Error> {
        let git = self
            .checkout(session, &input.cwd.0, "vcs.createRef")
            .await?;
        let name = git
            .create_ref(
                &input.ref_name.0,
                input.switch_ref.flatten().unwrap_or(false),
            )
            .await
            .map_err(|e| error(e, "vcs.createRef"))?;
        Ok(VcsCreateRefResult {
            ref_name: name.into(),
        })
    }

    pub async fn switch_ref(
        &self,
        session: &AuthSessionId,
        input: VcsSwitchRefInput,
    ) -> Result<VcsSwitchRefResult, Error> {
        let git = self
            .checkout(session, &input.cwd.0, "vcs.switchRef")
            .await?;
        let name = git
            .switch_ref(&input.ref_name.0)
            .await
            .map_err(|e| error(e, "vcs.switchRef"))?;
        Ok(VcsSwitchRefResult {
            ref_name: name.map(Into::into),
        })
    }

    pub async fn init(&self, session: &AuthSessionId, input: VcsInitInput) -> Result<(), Error> {
        let (cwd, _) = self.directory(session, &input.cwd.0).await?;
        let registry = VcsDriverRegistry::new(self.git.at(cwd));
        registry
            .get(input.kind.flatten().unwrap_or(VcsDriverKind::Git))
            .map_err(|e| error(e, "vcs.init"))?
            .init_repository()
            .await
            .map_err(|e| error(e, "vcs.init"))
    }

    pub async fn remove_worktree(
        &self,
        session: &AuthSessionId,
        input: VcsRemoveWorktreeInput,
    ) -> Result<(), Error> {
        let git = self
            .checkout(session, &input.cwd.0, "vcs.removeWorktree")
            .await?;
        let (target, _) = self.directory(session, &input.path.0).await?;
        let trees = git
            .list_worktrees()
            .await
            .map_err(|e| error(e, "vcs.removeWorktree"))?;
        // Git lists its main worktree first. Even force must not delete it, or a
        // directory unrelated to this repository's registered linked worktrees.
        let mut linked = false;
        for tree in trees.into_iter().skip(1) {
            if tokio::fs::canonicalize(tree.path)
                .await
                .is_ok_and(|p| p == target)
            {
                linked = true;
                break;
            }
        }
        if !linked {
            return Err(denied());
        }
        git.remove_worktree(&target, input.force.flatten().unwrap_or(false))
            .await
            .map_err(|e| error(e, "vcs.removeWorktree"))
    }

    pub async fn branch_commits(
        &self,
        session: &AuthSessionId,
        mut input: ReviewBranchCommitsInput,
    ) -> Result<ReviewBranchCommitsResult, Error> {
        let git = self
            .checkout(session, &input.cwd.0, "review.getBranchCommits")
            .await?;
        input.cwd = text(git.cwd())?;
        git.review_branch_commits(&input)
            .await
            .map_err(|e| error(e, "review.getBranchCommits"))
    }

    pub async fn diff_preview(
        &self,
        session: &AuthSessionId,
        mut input: ReviewDiffPreviewInput,
    ) -> Result<ReviewDiffPreviewResult, Error> {
        let git = self
            .checkout(session, &input.cwd.0, "review.getDiffPreview")
            .await?;
        input.cwd = text(git.cwd())?;
        git.review_diff_preview(&input)
            .await
            .map_err(|e| error(e, "review.getDiffPreview"))
    }

    pub async fn diff_file_contents(
        &self,
        session: &AuthSessionId,
        mut input: ReviewDiffFileContentsInput,
    ) -> Result<ReviewDiffFileContentsResult, Error> {
        let git = self
            .checkout(session, &input.cwd.0, "review.getDiffFileContents")
            .await?;
        input.cwd = text(git.cwd())?;
        git.review_diff_file_contents(&input)
            .await
            .map_err(|e| error(e, "review.getDiffFileContents"))
    }

    pub async fn drain(&self) {
        self.git.drain_operations().await;
    }
}

impl crate::services::DomainRuntime for GitAdapter {
    fn name(&self) -> &'static str {
        "git"
    }
    fn shutdown(&self) -> BoxFuture<'_, crate::Result<()>> {
        Box::pin(async move {
            self.drain().await;
            Ok(())
        })
    }
}

fn text(path: &Path) -> Result<t3_contracts::base_schemas::TrimmedNonEmptyString, Error> {
    Ok(path.to_str().ok_or_else(denied)?.to_owned().into())
}

fn denied() -> Error {
    Error::Authorization(Box::new(EnvironmentAuthorizationError {
        _tag: EnvironmentAuthorizationErrorTag::EnvironmentAuthorizationError,
        message: "The requested directory is outside this session's authorized project workspaces."
            .into(),
        required_scope: AuthEnvironmentScope::OrchestrationRead,
        required_permission: None,
    }))
}

/// Raw GitError Display can contain stderr, argv, URLs and filesystem diagnostics.
fn error(error: GitError, operation: &str) -> Error {
    match error {
        GitError::Vcs(failure) => Error::Vcs(Box::new(failure.error)),
        GitError::Command(failure) => Error::Git(Box::new(
            failure
                .to_contract()
                .unwrap_or_else(|_| command_error(operation)),
        )),
        _ => invalid_output(operation),
    }
}

fn invalid_output(operation: &str) -> Error {
    Error::Git(Box::new(command_error(operation)))
}

pub fn command_error(operation: &str) -> GitCommandError {
    GitCommandError {
        _tag: GitCommandErrorTag::GitCommandError,
        operation: operation.into(),
        command: "git".into(),
        cwd: String::new(),
        argument_count: None,
        exit_code: None,
        stdout_length: None,
        stderr_length: None,
        output_length: None,
        reason: None,
        detail: "Git could not complete the requested operation.".into(),
        cause: None,
    }
}
