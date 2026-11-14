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
    /// The grants containing `candidate` (already resolved); an authority that can answer per
    /// path avoids gathering every grant.
    fn roots_for<'a>(
        &'a self,
        session: &'a AuthSessionId,
        _candidate: &'a Path,
    ) -> BoxFuture<'a, Result<Vec<PathBuf>, EnvironmentAuthorizationError>> {
        self.roots(session)
    }
}

pub struct GitAdapter {
    git: Git,
    authority: Option<Arc<dyn WorkspaceAuthority>>,
    /// The VCS status, branch and worktree services over this adapter's Git root.
    #[cfg(feature = "local-services")]
    vcs: Option<vcs::Vcs>,
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
    pub async fn start(
        context: &crate::services::DomainContext<'_>,
        domains: &crate::services::Domains,
    ) -> Arc<Self> {
        Self::from_context(context, None, domains).await
    }

    #[cfg(feature = "local-services")]
    pub async fn start_with_authority(
        context: &crate::services::DomainContext<'_>,
        authority: Arc<dyn WorkspaceAuthority>,
        domains: &crate::services::Domains,
    ) -> Arc<Self> {
        Self::from_context(context, Some(authority), domains).await
    }

    /// The environment's one Git root: every consumer derives its checkout with `.at(cwd)`, so
    /// caches, locks and the environment patch are shared (t3-git review L2).
    #[cfg(feature = "local-services")]
    async fn from_context(
        context: &crate::services::DomainContext<'_>,
        authority: Option<Arc<dyn WorkspaceAuthority>>,
        domains: &crate::services::Domains,
    ) -> Arc<Self> {
        let git = Git::new(&context.local.config.cwd)
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
            );
        // Status and worktree RPCs authorize their cwd, so they need the workspace authority.
        let vcs = match authority {
            Some(_) => Some(vcs::Vcs::start(context, domains, git.clone()).await),
            None => None,
        };
        Arc::new(Self {
            git,
            authority,
            vcs,
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
            #[cfg(feature = "local-services")]
            vcs: None,
        }
    }

    /// Internal composition access; RPCs must resolve their checkout through the authority.
    pub fn at(&self, cwd: impl Into<PathBuf>) -> Git {
        self.git.at(cwd)
    }

    pub fn is_available(&self) -> bool {
        self.authority.is_some()
    }

    pub(crate) async fn directory(
        &self,
        session: &AuthSessionId,
        cwd: &str,
    ) -> Result<(PathBuf, Vec<PathBuf>), Error> {
        let authority = self.authority.as_ref().ok_or_else(denied)?;
        if !Path::new(cwd).is_absolute() {
            return Err(denied());
        }
        let cwd = tokio::fs::canonicalize(cwd).await.map_err(|_| denied())?;
        if !tokio::fs::metadata(&cwd).await.is_ok_and(|m| m.is_dir()) {
            return Err(denied());
        }
        let grants = authority
            .roots_for(session, &cwd)
            .await
            .map_err(|e| Error::Authorization(Box::new(e)))?;
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

    pub(crate) async fn checkout(
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

    /// The VCS status, branch and worktree services; absent without a workspace authority.
    #[cfg(feature = "local-services")]
    pub fn vcs(&self) -> Option<&vcs::Vcs> {
        self.vcs.as_ref()
    }

    /// Authorizes the client's cwd, then hands it on as sent: the broadcaster shares workers by
    /// physical path and keeps the raw alias for the background and auto-pull policy.
    #[cfg(feature = "local-services")]
    async fn status_services(
        &self,
        session: &AuthSessionId,
        cwd: &str,
        operation: &str,
    ) -> Result<&vcs::Vcs, Error> {
        let vcs = self.vcs.as_ref().ok_or_else(denied)?;
        self.checkout(session, cwd, operation).await?;
        Ok(vcs)
    }

    #[cfg(feature = "local-services")]
    pub async fn subscribe_vcs_branch(
        &self,
        session: &AuthSessionId,
        input: VcsStatusInput,
    ) -> Result<t3_git::VcsBranchSubscription, Error> {
        const OPERATION: &str = "subscribeVcsBranch";
        self.status_services(session, &input.cwd.0, OPERATION)
            .await?
            .application
            .subscribe_vcs_branch(&input)
            .await
            .map_err(|e| error(e, OPERATION))
    }

    #[cfg(feature = "local-services")]
    pub async fn refresh_branch(
        &self,
        session: &AuthSessionId,
        input: VcsStatusInput,
    ) -> Result<(), Error> {
        const OPERATION: &str = "vcs.refreshBranch";
        self.status_services(session, &input.cwd.0, OPERATION)
            .await?
            .application
            .refresh_branch(&input)
            .await
            .map_err(|e| error(e, OPERATION))
    }

    #[cfg(feature = "local-services")]
    pub async fn subscribe_vcs_status(
        &self,
        session: &AuthSessionId,
        input: VcsStatusSubscriptionInput,
    ) -> Result<t3_git::VcsStatusSubscription, Error> {
        const OPERATION: &str = "subscribeVcsStatus";
        self.status_services(session, &input.cwd.0, OPERATION)
            .await?
            .application
            .subscribe_vcs_status(&input)
            .await
            .map_err(|e| error(e, OPERATION))
    }

    #[cfg(feature = "local-services")]
    pub async fn refresh_status(
        &self,
        session: &AuthSessionId,
        input: t3_contracts::rpc::WsVcsRefreshStatusPayload,
    ) -> Result<VcsStatusResult, Error> {
        const OPERATION: &str = "vcs.refreshStatus";
        self.status_services(session, &input.cwd.0, OPERATION)
            .await?
            .application
            .refresh_status(&input)
            .await
            .map_err(|e| error(e, OPERATION))
    }

    #[cfg(feature = "local-services")]
    pub fn subscribe_worktree_setup(
        &self,
        input: &t3_contracts::worktree_setup::WorktreeSetupSubscribeInput,
    ) -> Result<t3_git::WorktreeSetupSubscription, Error> {
        let vcs = self.vcs.as_ref().ok_or_else(denied)?;
        Ok(vcs.application.subscribe_worktree_setup(input))
    }

    #[cfg(feature = "local-services")]
    pub async fn cancel_worktree_setup(
        &self,
        input: &t3_contracts::worktree_setup::WorktreeSetupCancelInput,
    ) -> Result<t3_contracts::worktree_setup::WorktreeSetupCancelResult, Error> {
        let vcs = self.vcs.as_ref().ok_or_else(denied)?;
        Ok(vcs.application.cancel_worktree_setup(input).await)
    }

    /// The client cannot name the destination (the contract's `path` is null): it is a folder
    /// under the managed worktree directory the server's settings resolve.
    #[cfg(feature = "local-services")]
    pub async fn create_worktree(
        &self,
        session: &AuthSessionId,
        input: t3_contracts::rpc::WsVcsCreateWorktreePayload,
    ) -> Result<VcsCreateWorktreeResult, Error> {
        const OPERATION: &str = "vcs.createWorktree";
        let vcs = self.status_services(session, &input.cwd.0, OPERATION).await?;
        if let Some(setting) = vcs.worktrees.directory_refusal().await {
            return Err(Error::Git(Box::new(GitCommandError {
                operation: "GitVcsDriver.createWorktree".into(),
                command: "git worktree add".into(),
                cwd: input.cwd.0.clone(),
                detail: format!(
                    "The worktree location \"{setting}\" must be an absolute folder on this machine, not a drive root. Change it in Settings → Worktrees."
                ),
                ..command_error(OPERATION)
            })));
        }
        vcs.application
            .create_worktree(&VcsCreateWorktreeInput {
                cwd: input.cwd,
                ref_name: input.ref_name,
                new_ref_name: input.new_ref_name,
                base_ref_name: input.base_ref_name,
                path: None,
            })
            .await
            .map_err(|e| error(e, OPERATION))
    }
}

impl crate::services::DomainRuntime for GitAdapter {
    fn name(&self) -> &'static str {
        "git"
    }
    /// Status workers stop (and are awaited) before the Git operations drain.
    fn shutdown(&self) -> BoxFuture<'_, crate::Result<()>> {
        Box::pin(async move {
            #[cfg(feature = "local-services")]
            if let Some(vcs) = &self.vcs {
                vcs.shutdown().await;
            }
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

/// `VcsStatusBroadcaster`, `WorktreeSetupTracker` and `vcs.createWorktree` over the
/// environment's Git root, with the settings-backed policy TypeScript layers in
/// (`layerAutomaticFetchInterval`, `BackgroundPolicy.shouldRunScopeWork`, `layerAutoPullPolicy`).
#[cfg(feature = "local-services")]
pub mod vcs {
    use super::*;
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };
    use t3_config::settings::SettingsService;
    use t3_contracts::{
        base_schemas::ProjectId, environment::WorktreeSubmodules, settings::ServerSettings,
    };
    use t3_git::{
        GitApplicationService, GitWorkflowService, VcsStatusBroadcaster, VcsStatusPolicy,
        WorktreeCreationSettings, WorktreeHost, WorktreeSetupTracker,
    };
    use t3_shared::project_settings::{has_project_settings_overrides, resolve_project_settings};
    use tokio_util::sync::CancellationToken;

    pub struct Vcs {
        pub(super) application: GitApplicationService,
        broadcaster: VcsStatusBroadcaster,
        setup: WorktreeSetupTracker,
        pub(super) worktrees: Arc<Worktrees>,
        stop: CancellationToken,
        follower: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    }

    impl Vcs {
        pub(super) async fn start(
            context: &crate::services::DomainContext<'_>,
            domains: &crate::services::Domains,
            git: Git,
        ) -> Self {
            let settings = context.local.settings.clone();
            let projects = Projects {
                #[cfg(feature = "orchestration")]
                database: t3_orchestration::stores::StoreDatabase(context.database.clone()),
            };
            let default_worktrees = context.local.config.paths.worktrees();
            let worktrees = Arc::new(Worktrees {
                settings: settings.clone(),
                default: default_worktrees.clone(),
                home: context.account_home.clone(),
                projects: projects.clone(),
            });
            let policy = Arc::new(StatusPolicy {
                settings: settings.clone(),
                interval: AtomicU64::new(millis(
                    t3_shared::background_activity::DEFAULT_AUTOMATIC_GIT_FETCH_INTERVAL,
                )),
                #[cfg(feature = "host")]
                host: domains.get::<crate::adapters::host::HostAdapter>(),
                projects,
            });
            let _ = domains;
            // Subscribed before the first snapshot, so no change between them is lost.
            let changes = settings.subscribe_changes();
            if let Ok(snapshot) = settings.snapshot().await {
                policy.observe(&snapshot);
            }
            let stop = context.shutdown.child_token();
            let follower = tokio::spawn(follow(policy.clone(), changes, stop.clone()));
            let workflow = GitWorkflowService::new(git)
                .with_worktrees_directory(default_worktrees)
                .with_worktree_host(worktrees.clone());
            let broadcaster = VcsStatusBroadcaster::new(workflow.clone()).with_policy(policy);
            let setup = WorktreeSetupTracker::new();
            let application =
                GitApplicationService::new(workflow, broadcaster.clone(), setup.clone());
            Self {
                application,
                broadcaster,
                setup,
                worktrees,
                stop,
                follower: std::sync::Mutex::new(Some(follower)),
            }
        }

        /// The environment's one setup tracker: thread launch must publish into this instance
        /// for `subscribeWorktreeSetup` and `worktreeSetup.cancel` to see its setups.
        pub fn setup_tracker(&self) -> &WorktreeSetupTracker {
            &self.setup
        }

        /// For orchestration's branch synchronization and refresh after Git mutations.
        pub fn broadcaster(&self) -> &VcsStatusBroadcaster {
            &self.broadcaster
        }

        /// `vcs.createWorktree`'s folder and submodule settings, for other worktree creators.
        pub fn worktree_host(&self) -> Arc<dyn WorktreeHost> {
            self.worktrees.clone()
        }

        /// The refusal `vcs.createWorktree` answers for an unusable configured folder.
        pub async fn worktree_directory_refusal(&self) -> Option<String> {
            self.worktrees.directory_refusal().await
        }

        pub(super) async fn shutdown(&self) {
            self.stop.cancel();
            let follower = self
                .follower
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            if let Some(follower) = follower {
                let _ = follower.await;
            }
            self.broadcaster.shutdown().await;
        }
    }

    async fn follow(
        policy: Arc<StatusPolicy>,
        mut changes: tokio::sync::mpsc::UnboundedReceiver<ServerSettings>,
        stop: CancellationToken,
    ) {
        loop {
            tokio::select! {
                biased;
                _ = stop.cancelled() => return,
                next = changes.recv() => match next {
                    Some(settings) => policy.observe(&settings),
                    None => return,
                },
            }
        }
    }

    /// `u64::MAX` stands for an infinite interval.
    fn millis(interval: Duration) -> u64 {
        u64::try_from(interval.as_millis()).unwrap_or(u64::MAX)
    }

    #[derive(Clone)]
    struct Projects {
        #[cfg(feature = "orchestration")]
        database: t3_orchestration::stores::StoreDatabase,
    }
    impl Projects {
        /// `ProjectStore.findActiveByWorkspaceRoot`: the live project rooted exactly at `cwd`.
        async fn at(&self, cwd: &Path) -> Option<ProjectId> {
            #[cfg(feature = "orchestration")]
            {
                let root = cwd.to_str()?.to_owned();
                self.database
                    .read(move |db| {
                        t3_orchestration::stores::projects::find_active_by_workspace_root(db, &root)
                    })
                    .await
                    .ok()
                    .flatten()
                    .map(|project| project.project_id)
            }
            #[cfg(not(feature = "orchestration"))]
            {
                let _ = cwd;
                None
            }
        }
    }

    struct StatusPolicy {
        settings: Arc<SettingsService>,
        /// The resolved `automaticGitFetchInterval` in milliseconds, kept live from settings.
        interval: AtomicU64,
        #[cfg(feature = "host")]
        host: Option<Arc<crate::adapters::host::HostAdapter>>,
        projects: Projects,
    }
    impl StatusPolicy {
        fn observe(&self, settings: &ServerSettings) {
            let resolved =
                t3_shared::background_activity::resolve_server_background_activity_settings(
                    settings,
                );
            self.interval
                .store(millis(resolved.automatic_git_fetch_interval), Ordering::Relaxed);
        }
    }

    #[async_trait::async_trait]
    impl VcsStatusPolicy for StatusPolicy {
        fn automatic_fetch_interval(&self) -> Duration {
            match self.interval.load(Ordering::Relaxed) {
                u64::MAX => Duration::MAX,
                millis => Duration::from_millis(millis),
            }
        }

        /// Background work for a cwd runs only while a client's activity lease asks for its
        /// `vcs-status` scope; without the host's policy no lease exists, so none runs.
        async fn should_run(&self, cwd: &Path) -> bool {
            #[cfg(feature = "host")]
            if let Some(host) = &self.host
                && let Ok(settings) = self.settings.snapshot().await
            {
                let activity = t3_host::background::ActivitySettings::resolve(&settings);
                let scope = t3_contracts::background::BackgroundScope::VcsStatus {
                    cwd: cwd.to_string_lossy().into_owned(),
                };
                return host
                    .background
                    .should_run_scope_work(&scope, &activity)
                    .await
                    .unwrap_or(false);
            }
            let _ = cwd;
            false
        }

        async fn auto_pull_enabled(&self, cwd: &Path) -> bool {
            let Some(project) = self.projects.at(cwd).await else {
                return false;
            };
            let Ok(settings) = self.settings.snapshot().await else {
                return false;
            };
            resolve_project_settings(&settings, Some(&project), None)
                .is_ok_and(|resolved| resolved.settings.default_auto_pull)
        }
    }

    /// `GitManager.createWorktree`'s settings: the worktree folder and the project's submodule
    /// choice (`null` leaves the `t3.json` fallback to the driver).
    pub(crate) struct Worktrees {
        settings: Arc<SettingsService>,
        default: PathBuf,
        home: PathBuf,
        projects: Projects,
    }
    impl Worktrees {
        /// The configured worktree location when it is not an absolute, non-root folder on
        /// this machine (`resolveWorktreesDirectory` answers null).
        pub(super) async fn directory_refusal(&self) -> Option<String> {
            let setting = self.settings.snapshot().await.ok()?.worktrees_directory.0;
            t3_config::home::resolve_worktrees_directory(&setting, &self.default, &self.home)
                .is_none()
                .then_some(setting)
        }

        async fn submodules(
            &self,
            settings: &ServerSettings,
            cwd: &Path,
        ) -> Option<WorktreeSubmodules> {
            if !has_project_settings_overrides(settings).unwrap_or(false) {
                return settings.worktree_submodules.clone();
            }
            let project = self.projects.at(cwd).await;
            resolve_project_settings(settings, project.as_ref(), None)
                .ok()?
                .settings
                .worktree_submodules
        }
    }

    #[async_trait::async_trait]
    impl WorktreeHost for Worktrees {
        async fn creation_settings(&self, cwd: &Path) -> WorktreeCreationSettings {
            let Ok(settings) = self.settings.snapshot().await else {
                return WorktreeCreationSettings::default();
            };
            WorktreeCreationSettings {
                directory: t3_config::home::resolve_worktrees_directory(
                    &settings.worktrees_directory.0,
                    &self.default,
                    &self.home,
                ),
                submodules: self.submodules(&settings, cwd).await.map(|choice| match choice {
                    WorktreeSubmodules::Recursive => t3_git::WorktreeSubmodules::Recursive,
                    WorktreeSubmodules::TopLevel => t3_git::WorktreeSubmodules::TopLevel,
                    WorktreeSubmodules::None => t3_git::WorktreeSubmodules::None,
                }),
            }
        }

        /// Project setup scripts run with thread launch (`ThreadLaunchService.ts`), not ported;
        /// `vcs.createWorktree` never runs them.
        async fn setup(&self, _: &Path, _: &str, _: &Path) -> Result<(), GitError> {
            Err(GitError::Unsupported(
                "project setup is not available on this server".into(),
            ))
        }
    }
}
