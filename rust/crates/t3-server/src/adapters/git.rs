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
    /// Changes when a grant may have narrowed; `None` when the authority has no signal.
    fn changes(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        None
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
        // Before any filesystem call: opening a UNC or WebDAV path authenticates to its host.
        if !local_disk_path(Path::new(cwd)) {
            return Err(not_local());
        }
        let cwd = resolve(PathBuf::from(cwd)).await?;
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
                && let Ok(root) = resolve(grant).await
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
            let repository_root = resolve(repository.root).await?;
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
            .map_err(|e| error(e, "vcs.init"))?;
        // The user asked the server to create this repository: it is trusted in the same
        // operation (keyed by the folder as the client names it, which is how it is recorded).
        #[cfg(feature = "local-services")]
        self.trust_repository(Path::new(&input.cwd.0)).await?;
        Ok(())
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
            if resolve(tree.path).await.is_ok_and(|p| p == target) {
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

    /// The user's explicit "trust this repository", or a repository the server itself just
    /// created in `folder` (Initialize, a worktree): re-reads the repository there (discovery
    /// only, fsmonitor off, no repository-configured helper), records it in t3-git's process
    /// identity and in the saved bindings, and drops the status broadcaster's cached handle that
    /// was pinned to the identity it replaces. The caller passes a project's or thread's own
    /// folder from its record, never a client path; a folder that is not a local directory is
    /// refused here.
    #[cfg(feature = "local-services")]
    pub async fn trust_repository(&self, folder: &Path) -> Result<(), Error> {
        const OPERATION: &str = "vcs.trustRepository";
        if !local_disk_path(folder) {
            return Err(not_local());
        }
        if !tokio::fs::metadata(folder).await.is_ok_and(|m| m.is_dir()) {
            return Err(denied());
        }
        self.git
            .at(folder)
            .rebind_repository()
            .await
            .map_err(|e| error(e, OPERATION))?;
        if let Some(vcs) = &self.vcs {
            if let Some(bindings) = vcs.saved_bindings() {
                bindings
                    .trust(folder)
                    .await
                    .map_err(|_| error(saved_bindings_unreadable(folder), OPERATION))?;
            }
            vcs.broadcaster().forget_checkout(folder).await;
        }
        Ok(())
    }

    #[cfg(feature = "local-services")]
    /// `socket` is the connection the subscription counts against (`None` outside a socket).
    pub async fn subscribe_vcs_branch(
        self: &Arc<Self>,
        session: &AuthSessionId,
        socket: Option<u64>,
        input: VcsStatusInput,
    ) -> Result<Revocable<t3_git::VcsBranchSubscription>, Error> {
        const OPERATION: &str = "subscribeVcsBranch";
        let vcs = self
            .status_services(session, &input.cwd.0, OPERATION)
            .await?;
        let permit = vcs.subscriptions.acquire(socket, OPERATION, &input.cwd.0)?;
        let subscription = vcs
            .application
            .subscribe_vcs_branch(&input)
            .await
            .map_err(|e| error(e, OPERATION))?;
        Ok(self.revocable(subscription, permit, session, input.cwd.0))
    }

    #[cfg(feature = "local-services")]
    fn revocable<S>(
        self: &Arc<Self>,
        subscription: S,
        permit: SubscriptionPermit,
        session: &AuthSessionId,
        cwd: String,
    ) -> Revocable<S> {
        Revocable {
            subscription: Some((subscription, permit)),
            changes: self
                .authority
                .as_ref()
                .and_then(|authority| authority.changes()),
            adapter: self.clone(),
            session: session.clone(),
            cwd,
        }
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
        self: &Arc<Self>,
        session: &AuthSessionId,
        socket: Option<u64>,
        input: VcsStatusSubscriptionInput,
    ) -> Result<Revocable<t3_git::VcsStatusSubscription>, Error> {
        const OPERATION: &str = "subscribeVcsStatus";
        let vcs = self
            .status_services(session, &input.cwd.0, OPERATION)
            .await?;
        let permit = vcs.subscriptions.acquire(socket, OPERATION, &input.cwd.0)?;
        let subscription = vcs
            .application
            .subscribe_vcs_status(&input)
            .await
            .map_err(|e| error(e, OPERATION))?;
        Ok(self.revocable(subscription, permit, session, input.cwd.0))
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
        let vcs = self
            .status_services(session, &input.cwd.0, OPERATION)
            .await?;
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
        let created = vcs
            .application
            .create_worktree(&VcsCreateWorktreeInput {
                cwd: input.cwd,
                ref_name: input.ref_name,
                new_ref_name: input.new_ref_name,
                base_ref_name: input.base_ref_name,
                path: None,
            })
            .await
            .map_err(|e| error(e, OPERATION))?;
        // The server created this checkout: trusted in the same operation.
        self.trust_repository(Path::new(&created.worktree.path.0))
            .await?;
        Ok(created)
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

/// A status or branch subscription that asks the workspace authority again before each value
/// and whenever the authority signals a change, so revoked access (a removed project) ends it
/// with the typed refusal, idle or not, releasing its slot and its status worker.
#[cfg(feature = "local-services")]
pub struct Revocable<S> {
    /// The subscription and its slot under the subscription limits, released together.
    subscription: Option<(S, SubscriptionPermit)>,
    /// The authority's change signal; `None` when it has none (or it closed).
    changes: Option<tokio::sync::watch::Receiver<u64>>,
    adapter: Arc<GitAdapter>,
    session: AuthSessionId,
    cwd: String,
}

/// The values a [`Revocable`] forwards: a t3-git subscription's next one.
pub trait StatusStream: Send {
    type Item: Send;
    fn next_value(&mut self) -> impl std::future::Future<Output = Option<Self::Item>> + Send;
}

impl StatusStream for t3_git::VcsBranchSubscription {
    type Item = VcsBranchStatus;
    fn next_value(&mut self) -> impl std::future::Future<Output = Option<Self::Item>> + Send {
        self.recv()
    }
}

impl StatusStream for t3_git::VcsStatusSubscription {
    type Item = VcsStatusStreamEvent;
    fn next_value(&mut self) -> impl std::future::Future<Output = Option<Self::Item>> + Send {
        self.recv()
    }
}

#[cfg(feature = "local-services")]
impl<S: StatusStream> Revocable<S> {
    pub async fn recv(&mut self) -> Option<Result<S::Item, Error>> {
        enum Next<T> {
            Value(Option<T>),
            Changed(bool),
        }
        loop {
            let next = {
                let subscription = &mut self.subscription.as_mut()?.0;
                let changes = &mut self.changes;
                tokio::select! {
                    value = subscription.next_value() => Next::Value(value),
                    open = async {
                        match changes {
                            Some(changes) => changes.changed().await.is_ok(),
                            None => std::future::pending().await,
                        }
                    } => Next::Changed(open),
                }
            };
            match next {
                Next::Value(value) => return self.still_granted(value?).await,
                // The authority dropped its signal: values alone re-check from now on.
                Next::Changed(false) => self.changes = None,
                Next::Changed(true) => {
                    if let Err(error) = self.adapter.directory(&self.session, &self.cwd).await {
                        self.subscription = None;
                        return Some(Err(error));
                    }
                }
            }
        }
    }

    async fn still_granted<T>(&mut self, value: T) -> Option<Result<T, Error>> {
        match self.adapter.directory(&self.session, &self.cwd).await {
            Ok(_) => Some(Ok(value)),
            Err(error) => {
                self.subscription = None;
                Some(Err(error))
            }
        }
    }
}

#[cfg(all(test, feature = "local-services"))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod revocable_tests {
    use super::*;
    use futures::FutureExt;

    struct Idle(tokio::sync::mpsc::UnboundedReceiver<u32>);
    impl StatusStream for Idle {
        type Item = u32;
        fn next_value(&mut self) -> impl std::future::Future<Output = Option<u32>> + Send {
            self.0.recv()
        }
    }

    /// The authority signals a change while the stream has nothing to deliver. The adapter here
    /// grants nothing, so the re-check refuses without I/O and the outcome is visible at the
    /// first poll: a bounded check, with no waiting.
    #[tokio::test]
    async fn an_idle_stream_ends_when_the_authority_signals_a_change() {
        let (_values, receiver) = tokio::sync::mpsc::unbounded_channel();
        let (signal, changes) = tokio::sync::watch::channel(0u64);
        let limits = Arc::<SubscriptionLimits>::default();
        let mut stream = Revocable {
            subscription: Some((
                Idle(receiver),
                limits
                    .acquire(Some(1), "subscribeVcsStatus", "repo")
                    .unwrap(),
            )),
            changes: Some(changes),
            adapter: Arc::new(GitAdapter {
                git: Git::new(std::env::temp_dir()),
                authority: None,
                vcs: None,
            }),
            session: AuthSessionId("session".into()),
            cwd: std::env::temp_dir().to_string_lossy().into_owned(),
        };
        // Idle and unchanged: still open.
        assert!(stream.recv().now_or_never().is_none());

        signal.send_modify(|revision| *revision += 1);
        let ended = stream
            .recv()
            .now_or_never()
            .expect("the change signal re-checks an idle stream at once");
        assert!(
            matches!(ended, Some(Err(Error::Authorization(_)))),
            "{ended:?}"
        );
        assert!(stream.recv().now_or_never().expect("ended").is_none());
        // The subscription (and so its status worker lease) and its slot are released.
        assert!(stream.subscription.is_none());
        assert_eq!(limits.counts.lock().unwrap().0, 0);
    }
}

/// Live status and branch subscriptions allowed per connection and per server. TypeScript has
/// no bound; each distinct checkout they name keeps a t3-git status worker (a HEAD watcher, and
/// background fetch while a lease asks), so one client cannot open workers without limit.
pub const STATUS_SUBSCRIPTIONS_PER_CONNECTION: usize = 128;
pub const STATUS_SUBSCRIPTIONS_PER_SERVER: usize = 512;

#[derive(Default)]
pub(crate) struct SubscriptionLimits {
    /// Live subscriptions in total and per connection (sockets with none are removed).
    counts: std::sync::Mutex<(usize, std::collections::HashMap<u64, usize>)>,
}

impl SubscriptionLimits {
    pub(crate) fn acquire(
        self: &Arc<Self>,
        socket: Option<u64>,
        operation: &str,
        cwd: &str,
    ) -> Result<SubscriptionPermit, Error> {
        let refused = |scope: &str, limit: usize| {
            Error::Git(Box::new(GitCommandError {
                operation: operation.into(),
                cwd: cwd.into(),
                detail: format!(
                    "Too many VCS status subscriptions on this {scope} (limit {limit}); close some before opening more."
                ),
                ..command_error(operation)
            }))
        };
        let mut counts = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        let (total, sockets) = &mut *counts;
        if let Some(socket) = socket
            && sockets.get(&socket).copied().unwrap_or(0) >= STATUS_SUBSCRIPTIONS_PER_CONNECTION
        {
            return Err(refused("connection", STATUS_SUBSCRIPTIONS_PER_CONNECTION));
        }
        if *total >= STATUS_SUBSCRIPTIONS_PER_SERVER {
            return Err(refused("server", STATUS_SUBSCRIPTIONS_PER_SERVER));
        }
        *total += 1;
        if let Some(socket) = socket {
            *sockets.entry(socket).or_default() += 1;
        }
        Ok(SubscriptionPermit {
            limits: self.clone(),
            socket,
        })
    }
}

pub(crate) struct SubscriptionPermit {
    limits: Arc<SubscriptionLimits>,
    socket: Option<u64>,
}

impl Drop for SubscriptionPermit {
    fn drop(&mut self) {
        let mut counts = self.limits.counts.lock().unwrap_or_else(|e| e.into_inner());
        let (total, sockets) = &mut *counts;
        *total = total.saturating_sub(1);
        if let Some(socket) = self.socket
            && let Some(count) = sockets.get_mut(&socket)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                sockets.remove(&socket);
            }
        }
    }
}

fn text(path: &Path) -> Result<t3_contracts::base_schemas::TrimmedNonEmptyString, Error> {
    Ok(path.to_str().ok_or_else(denied)?.to_owned().into())
}

/// An absolute path on a local disk, decided from its text alone: on Windows a drive letter then
/// the root (not UNC, device, verbatim or drive-relative forms), elsewhere the root.
pub(crate) fn local_disk_path(path: &Path) -> bool {
    use std::path::Component;
    let mut components = path.components();
    match components.next() {
        Some(Component::Prefix(prefix)) => {
            matches!(prefix.kind(), std::path::Prefix::Disk(_))
                && matches!(components.next(), Some(Component::RootDir))
        }
        Some(Component::RootDir) => !cfg!(windows),
        _ => false,
    }
}

/// The workspace resolver (`t3_workspace::paths::canonicalize`): it refuses a network drive,
/// and a link to a network target before following it, then resolves. Its own refusals carry
/// no OS error code; an OS failure (missing, unreadable) is the ordinary denial.
async fn resolve(path: PathBuf) -> Result<PathBuf, Error> {
    match tokio::task::spawn_blocking(move || t3_workspace::paths::canonicalize(path)).await {
        Ok(Ok(real)) => Ok(real),
        Ok(Err(error))
            if error.kind() == std::io::ErrorKind::PermissionDenied
                && error.raw_os_error().is_none() =>
        {
            Err(not_local())
        }
        _ => Err(denied()),
    }
}

fn not_local() -> Error {
    Error::Authorization(Box::new(EnvironmentAuthorizationError {
        _tag: EnvironmentAuthorizationErrorTag::EnvironmentAuthorizationError,
        message: "The requested directory must be a folder on this server's local disks.".into(),
        required_scope: AuthEnvironmentScope::OrchestrationRead,
        required_permission: None,
    }))
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
/// The typed status of a folder whose repository no longer matches its binding, named as
/// orchestration names it (`checkpoint.git.repository-changed`); never "not a repository".
pub const REPOSITORY_CHANGED_OPERATION: &str = "checkpoint.git.repository-changed";

/// What the saved bindings hold for a folder (t3-git re-review 4 F3, F4).
#[derive(Debug, Clone)]
pub enum SavedRepository {
    /// Never recorded: the first read binds what is there (t3-git's process identity).
    NotRecorded,
    /// Recorded when the folder was not a repository: one created there since is not trusted.
    NotRepository,
    Repository(t3_git::RepositoryBinding),
}

/// The saved repository bindings (the orchestration adapter's store, which starts after Git).
pub trait SavedRepositoryBindings: Send + Sync {
    fn saved_repository<'a>(
        &'a self,
        scope: &'a Path,
    ) -> BoxFuture<'a, Result<SavedRepository, String>>;
    /// Records the repository now in `scope` as trusted.
    fn trust<'a>(&'a self, scope: &'a Path) -> BoxFuture<'a, Result<(), String>>;
}

#[cfg(feature = "orchestration")]
impl SavedRepositoryBindings for crate::adapters::orchestration::OrchestrationAdapter {
    fn saved_repository<'a>(
        &'a self,
        scope: &'a Path,
    ) -> BoxFuture<'a, Result<SavedRepository, String>> {
        Box::pin(async move {
            Ok(
                match self
                    .repository_binding(scope)
                    .await
                    .map_err(|error| error.to_string())?
                {
                    None => SavedRepository::NotRecorded,
                    Some(None) => SavedRepository::NotRepository,
                    Some(Some(binding)) => SavedRepository::Repository(binding),
                },
            )
        })
    }
    fn trust<'a>(&'a self, scope: &'a Path) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.trust_repository(scope)
                .await
                .map_err(|error| error.to_string())
        })
    }
}

/// An unreadable bindings store cannot prove the folder's repository, so reads fail closed.
fn saved_bindings_unreadable(path: &Path) -> GitError {
    GitError::RepositoryChanged {
        path: path.to_owned(),
        detail: "T3 Code cannot read its saved repository bindings.",
    }
}

fn error(error: GitError, operation: &str) -> Error {
    match error {
        // The changed path is left out, as orchestration does (re-review 4 L4).
        GitError::RepositoryChanged { detail, .. } => Error::Git(Box::new(GitCommandError {
            operation: REPOSITORY_CHANGED_OPERATION.into(),
            detail: format!(
                "This folder no longer belongs to the repository it was bound to: {detail} T3 Code will not read it until the folder is restored or you trust the repository again."
            ),
            ..command_error(operation)
        })),
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
        GitApplicationService, GitWorkflowFactory, GitWorkflowService, VcsStatusBroadcaster,
        VcsStatusPolicy, WorktreeCreationSettings, WorktreeHost, WorktreeSetupTracker,
    };
    use t3_shared::project_settings::{has_project_settings_overrides, resolve_project_settings};
    use tokio_util::sync::CancellationToken;

    pub struct Vcs {
        pub(super) application: GitApplicationService,
        broadcaster: VcsStatusBroadcaster,
        setup: WorktreeSetupTracker,
        pub(super) worktrees: Arc<Worktrees>,
        pub(super) subscriptions: Arc<SubscriptionLimits>,
        workflows: Arc<CheckoutWorkflows>,
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
            let workflows = Arc::new(CheckoutWorkflows {
                root: workflow.clone(),
                forge: std::sync::OnceLock::new(),
                bindings: std::sync::OnceLock::new(),
            });
            let broadcaster = status_broadcaster(workflow.clone(), workflows.clone(), policy);
            let setup = WorktreeSetupTracker::new();
            let application =
                GitApplicationService::new(workflow, broadcaster.clone(), setup.clone())
                    .with_workflow_factory(workflows.clone());
            Self {
                application,
                broadcaster,
                setup,
                worktrees,
                subscriptions: Arc::default(),
                workflows,
                stop,
                follower: std::sync::Mutex::new(Some(follower)),
            }
        }

        /// Binds the forge registry's per-checkout workflows (`adapters/forge.rs`), so status
        /// carries each checkout's pull request; `factory` receives the root workflow to clone.
        /// The forge starts after Git; until it binds, checkouts use the root workflow alone.
        pub fn bind_workflow_factory(
            &self,
            factory: impl FnOnce(GitWorkflowService) -> Arc<dyn GitWorkflowFactory>,
        ) {
            let _ = self
                .workflows
                .forge
                .set(factory(self.workflows.root.clone()));
        }

        /// Installs the saved repository bindings every status, branch and worktree read of a
        /// folder is pinned to (the orchestration adapter starts after Git; nothing reads before).
        pub fn use_saved_bindings(&self, bindings: Arc<dyn SavedRepositoryBindings>) {
            let _ = self.workflows.bindings.set(bindings);
        }

        pub(super) fn saved_bindings(&self) -> Option<&Arc<dyn SavedRepositoryBindings>> {
            self.workflows.bindings.get()
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

    /// The one status broadcaster: every checkout's handle comes from `workflows`, so every read
    /// (the status methods, launch and Git actions alike) gets the folder's saved binding.
    fn status_broadcaster(
        root: GitWorkflowService,
        workflows: Arc<CheckoutWorkflows>,
        policy: Arc<dyn VcsStatusPolicy>,
    ) -> VcsStatusBroadcaster {
        VcsStatusBroadcaster::new(root)
            .with_workflow_factory(workflows)
            .with_policy(policy)
    }

    /// Each checkout's workflow for the status broadcaster and the facade: the forge's factory once
    /// bound, else the root workflow at that checkout.
    pub(super) struct CheckoutWorkflows {
        pub(super) root: GitWorkflowService,
        pub(super) forge: std::sync::OnceLock<Arc<dyn GitWorkflowFactory>>,
        /// The saved bindings applied to every checkout's handle (re-review 4 H-1 item 4).
        pub(super) bindings: std::sync::OnceLock<Arc<dyn SavedRepositoryBindings>>,
    }
    #[async_trait::async_trait]
    impl GitWorkflowFactory for CheckoutWorkflows {
        async fn for_checkout(&self, cwd: &Path) -> Result<GitWorkflowService, GitError> {
            let workflow = match self.forge.get() {
                Some(forge) => forge.for_checkout(cwd).await?,
                None => self.root.at(cwd),
            };
            let Some(bindings) = self.bindings.get() else {
                return Ok(workflow);
            };
            // Re-review 4 H-1 item 4: every status, branch and worktree read of the folder runs
            // on a handle pinned to its saved binding, whoever asks.
            match saved_repository(bindings.as_ref(), cwd).await? {
                SavedRepository::NotRecorded => Ok(workflow),
                SavedRepository::Repository(binding) => {
                    workflow.with_repository_binding(binding).await
                }
                SavedRepository::NotRepository => {
                    if workflow.git().bind_repository().await?.is_some() {
                        return Err(GitError::RepositoryChanged {
                            path: cwd.to_owned(),
                            detail: "A Git repository was created in this folder after T3 Code first saw it without one.",
                        });
                    }
                    Ok(workflow)
                }
            }
        }
    }

    /// The saved state of `cwd` (the store finds a folder under its given or canonical spelling).
    async fn saved_repository(
        bindings: &dyn SavedRepositoryBindings,
        cwd: &Path,
    ) -> Result<SavedRepository, GitError> {
        bindings
            .saved_repository(cwd)
            .await
            .map_err(|_| saved_bindings_unreadable(cwd))
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
            self.interval.store(
                millis(resolved.automatic_git_fetch_interval),
                Ordering::Relaxed,
            );
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
                submodules: self
                    .submodules(&settings, cwd)
                    .await
                    .map(|choice| match choice {
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

    #[cfg(test)]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    mod saved_binding_tests {
        use super::*;

        fn isolated(path: &Path, home: &Path) -> Git {
            Git::new(path).with_environment(std::collections::BTreeMap::from([
                (
                    "GIT_CONFIG_GLOBAL".into(),
                    Some(home.join("no-global-config").into_os_string()),
                ),
                ("GIT_CONFIG_NOSYSTEM".into(), Some("1".into())),
                ("GIT_AUTHOR_NAME".into(), Some("t3".into())),
                ("GIT_AUTHOR_EMAIL".into(), Some("t3@example.invalid".into())),
                ("GIT_COMMITTER_NAME".into(), Some("t3".into())),
                (
                    "GIT_COMMITTER_EMAIL".into(),
                    Some("t3@example.invalid".into()),
                ),
            ]))
        }

        async fn repository(path: &Path, home: &Path) {
            std::fs::create_dir_all(path).unwrap();
            let git = isolated(path, home);
            for args in [
                &["init", "--initial-branch=main"][..],
                &["commit", "--allow-empty", "-m", "init"][..],
            ] {
                git.execute(args, Default::default()).await.unwrap();
            }
        }

        struct Fixed(SavedRepository);
        impl SavedRepositoryBindings for Fixed {
            fn saved_repository<'a>(
                &'a self,
                _: &'a Path,
            ) -> BoxFuture<'a, Result<SavedRepository, String>> {
                let saved = self.0.clone();
                Box::pin(async move { Ok(saved) })
            }
            fn trust<'a>(&'a self, _: &'a Path) -> BoxFuture<'a, Result<(), String>> {
                Box::pin(async { Ok(()) })
            }
        }

        /// Launch (`launch.rs`) and Git actions (`git_actions.rs`) refresh through
        /// `vcs.broadcaster().refresh_status(..)` directly, not through the status methods; the
        /// broadcaster built as `Vcs::start` builds it must still refuse a folder whose saved
        /// repository changed.
        #[tokio::test]
        async fn direct_broadcaster_reads_apply_the_saved_binding() {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path();
            let earlier = home.join("earlier");
            let changed = home.join("changed");
            repository(&earlier, home).await;
            repository(&changed, home).await;
            let saved = isolated(&earlier, home)
                .bind_repository()
                .await
                .unwrap()
                .unwrap();
            let root = GitWorkflowService::new(isolated(home, home));
            let workflows = Arc::new(CheckoutWorkflows {
                root: root.clone(),
                forge: std::sync::OnceLock::new(),
                bindings: std::sync::OnceLock::new(),
            });
            let _ = workflows
                .bindings
                .set(Arc::new(Fixed(SavedRepository::Repository(saved))));
            let broadcaster =
                status_broadcaster(root, workflows, Arc::new(t3_git::DisabledVcsPolling));

            let refreshed = broadcaster.refresh_status(&changed, false).await;
            let Err(refused) = refreshed else {
                panic!("a direct refresh skipped the saved binding: {refreshed:?}");
            };
            let Error::Git(mapped) = error(refused, "refresh") else {
                panic!("not mapped to a typed command error");
            };
            assert_eq!(mapped.operation, REPOSITORY_CHANGED_OPERATION);
            assert!(broadcaster.refresh_branch_status(&changed).await.is_err());
            broadcaster.shutdown().await;
        }
    }
}
