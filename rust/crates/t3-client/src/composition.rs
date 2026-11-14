//! The composition root: the one object a UI holds.
//!
//! [`ClientRuntime`] owns the saved environments' connections in the app's one [`ClientStore`],
//! the threads screens have open, and the per-environment message delivery. Screens read the
//! store and call these methods; they never touch connections, sessions or credentials. The
//! crate owns no executor, so the UI supplies a spawner for the futures it must keep running.
mod composer;
mod composer_attachments;
mod composer_updates;
mod settings;
use crate::{
    ClientError, ClientStore,
    authorization::RemoteAuthorization,
    connection::{
        ConnectedClient, ConnectionDiagnosis, ConnectionFailure, ConnectionMode, ConnectionProfile,
        ConnectionState, ConnectionStores, ConnectionTask, ConnectionVia, DiagnosisInput,
        PairingInput, Phase, Route, ThreadLease, Time, prepare_pairing_registration,
        prepare_ssh_registration, present_connection_diagnosis,
        runtime::{LearnRoutes, session_failure, supervise_saved},
    },
    environment::{EnvironmentRegistration, EnvironmentRegistry},
    platform::{NetworkStatus, Platform},
    relay::{DpopSigner, HostDpopSigner, ManagedRelayClient, RelayDiscovery, RelayDiscoveryState},
    rpc::{RpcEndpoint, RpcMethod, SessionEndpoint},
    services::{
        command_output::{
            CommandOutputCell, CommandOutputShared, SettledCache, drive_command_output,
        },
        resources::{Resource, ResourcePolicy},
        sessions::{
            EnvironmentStream, QueryConnection, QueryConnectionPhase, ServiceSession, drive_query,
        },
    },
    store::Scope,
    thread_views::composer::send::{PendingMessage, SendService},
    types::{EnvironmentId, Generation, ThreadKey},
};
use futures::{
    FutureExt, StreamExt,
    channel::mpsc,
    future::{AbortHandle, Abortable, LocalBoxFuture},
    lock::Mutex,
    stream::LocalBoxStream,
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;

/// Runs a future on the UI's executor until it finishes or is dropped by its owner.
pub type Spawner = Rc<dyn Fn(LocalBoxFuture<'static, ()>)>;

/// One saved environment as the connection layer sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct EnvironmentEntry {
    pub id: EnvironmentId,
    pub label: String,
    pub state: ConnectionState,
    /// Its saved routes in preference order: direct addresses (LAN, Tailscale, public), the
    /// relay, SSH. `connection::route_kind` names each.
    pub routes: Vec<Route>,
    /// A saved record of this environment (its profile, credential or token) could not be read;
    /// it may not connect until it is paired again or removed.
    pub damaged: bool,
}

pub use crate::services::storage_notice::StorageNotice;

/// T3 Connect: the relay that reaches environments linked to the signed-in account.
#[derive(Clone, Debug)]
pub struct RelayConfig {
    pub relay_url: String,
    pub client_id: t3_contracts::relay::RelayPublicClientId,
}

/// An SSH host the desktop launches T3 Code on (`Platform::ssh_provision`).
#[derive(Clone, Debug)]
pub struct SshPairingInput {
    pub target: t3_contracts::ipc::DesktopSshEnvironmentTargetSchema,
    pub label: Option<String>,
    pub expected_environment: Option<EnvironmentId>,
}

const RELAY_ROUTE_ID: &str = "relay";

struct Relay {
    discovery: RelayDiscovery,
    /// Ends discovery's credential and network watcher.
    task: AbortHandle,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    /// The environment has no live session; the message was not accepted.
    #[error("The environment is not connected.")]
    Disconnected,
    /// The client's validation or the server refused it, with the client's English reason.
    #[error("{0}")]
    Rejected(String),
}

struct Environment<P: Platform + 'static> {
    id: EnvironmentId,
    label: String,
    routes: Vec<Route>,
    client: Option<ConnectedClient>,
    send: Option<SendService<P, SessionEndpoint>>,
    /// Why the last connection attempt failed before a client existed.
    failure: Option<ConnectionFailure>,
    /// Ends this environment's delivery loop and session binding.
    tasks: Vec<AbortHandle>,
    /// Resolves when the connection driver has finished.
    finished: Option<futures::channel::oneshot::Receiver<()>>,
}

struct Inner<P: Platform + Clone + 'static> {
    platform: P,
    store: ClientStore,
    git_panel: Rc<crate::workspace::git_panel::GitPanelState>,
    project_files: Rc<crate::workspace::project_files::FilesState>,
    source: OrchestrationV2CreationSource,
    spawn: Spawner,
    environments: RefCell<Vec<Environment<P>>>,
    /// Open threads: how many opens each has, and its lease once its environment is connected.
    threads: RefCell<BTreeMap<ThreadKey, (usize, Option<ThreadLease>)>>,
    listeners: RefCell<Vec<mpsc::UnboundedSender<()>>>,
    /// `message_changes` subscribers.
    message_listeners: Rc<RefCell<Vec<mpsc::UnboundedSender<()>>>>,
    /// Mounted queries by identity, so every reader of one query shares one driver.
    queries: RefCell<BTreeMap<String, std::rc::Weak<dyn std::any::Any>>>,
    /// Serializes changes to the saved environments (pair, restore, reconnect, remove) so they
    /// apply in the order asked, as TypeScript's registry lease does.
    registry: Mutex<()>,
    thread_operations: Rc<crate::thread_views::thread_ops::ThreadOpsState>,
    terminals: Rc<crate::services::terminal_sessions::TerminalSessionsState>,
    /// Open command rows by identity, so every reader of one row shares one subscription.
    command_outputs: RefCell<BTreeMap<String, std::rc::Weak<CommandOutputShared>>>,
    /// Settled commands' final output, kept until the instant given after the last reader left.
    settled_outputs: crate::services::command_output::SettledCells,
    /// One authorization for every environment, as TypeScript's; relay-capable with a relay.
    authorization: RemoteAuthorization,
    relay: Option<Relay>,
    /// Environments with a saved record the catalog could not read when they last started.
    damaged: RefCell<std::collections::BTreeSet<EnvironmentId>>,
    /// Built by the first restore, taken once by the UI.
    storage_notice: RefCell<Option<StorageNotice>>,
    restored: std::cell::Cell<bool>,
    composer_drafts: Rc<crate::thread_views::composer::draft_service::ComposerDraftService<P>>,
    composer_preferences: Rc<crate::thread_views::composer::preferences::ComposerPreferences<P>>,
    composer_models: Rc<crate::thread_views::composer::model_service::ComposerModelState>,
    composer_updates: crate::services::updates::ServerUpdateService,
    composer_update_dismissals: crate::thread_views::composer::banners::UpdateDismissals,
    composer_stash: Rc<crate::thread_views::composer::stash_service::PromptStashState>,
    composer_questions: Rc<crate::thread_views::composer::questions_service::PendingUserInputState>,
    composer_attachments:
        RefCell<BTreeMap<(EnvironmentId, String), composer_attachments::AttachmentDriver>>,
    composer_task: AbortHandle,
    navigation: crate::navigation::UrlClient<P>,
    new_threads: Rc<crate::navigation::new_thread::NewThreadState>,
}

impl<P: Platform + Clone + 'static> Drop for Inner<P> {
    fn drop(&mut self) {
        self.composer_task.abort();
        if let Some(relay) = &self.relay {
            relay.task.abort();
        }
        self.threads.get_mut().clear();
        for environment in self.environments.get_mut().drain(..) {
            drop(shut_down(environment));
        }
    }
}

/// Stops everything the environment runs; returns the signal of its driver finishing.
fn shut_down<P: Platform + Clone + 'static>(
    environment: Environment<P>,
) -> Option<futures::channel::oneshot::Receiver<()>> {
    for task in environment.tasks {
        task.abort();
    }
    if let Some(send) = environment.send {
        send.dispose();
    }
    if let Some(client) = environment.client {
        let _ = client.close();
    }
    environment.finished
}

#[derive(Clone)]
pub struct ClientRuntime<P: Platform + Clone + 'static> {
    inner: Rc<Inner<P>>,
}

impl<P: Platform + Clone + 'static> ClientRuntime<P> {
    /// `source` names this client in the threads it creates (web, mobile). Direct, Tailscale
    /// and SSH environments only; [`Self::with_relay`] adds T3 Connect.
    pub fn new(
        platform: P,
        store: ClientStore,
        source: OrchestrationV2CreationSource,
        spawn: Spawner,
    ) -> Self {
        let authorization = RemoteAuthorization::direct(Rc::new(platform.clone()));
        Self::build(platform, store, source, spawn, authorization, None)
    }

    /// [`Self::new`] plus T3 Connect: environments linked to the account the host is signed in
    /// to (`Platform::cloud_token`) are discovered through the relay and connect through it
    /// with the host's DPoP key.
    pub fn with_relay(
        platform: P,
        store: ClientStore,
        source: OrchestrationV2CreationSource,
        spawn: Spawner,
        relay: RelayConfig,
    ) -> Self {
        let shared: Rc<dyn Platform> = Rc::new(platform.clone());
        let signer: Rc<dyn DpopSigner> = Rc::new(HostDpopSigner::new(shared.clone()));
        let client = ManagedRelayClient::new(
            shared.clone(),
            signer.clone(),
            &relay.relay_url,
            relay.client_id,
        );
        let authorization = RemoteAuthorization::new(shared.clone(), signer, client.clone());
        let discovery = RelayDiscovery::new(client);
        let (task, registration) = AbortHandle::new_pair();
        let watcher = discovery.clone();
        spawn(
            Abortable::new(async move { watcher.run(shared).await }, registration)
                .map(|_| ())
                .boxed_local(),
        );
        Self::build(
            platform,
            store,
            source,
            spawn,
            authorization,
            Some(Relay { discovery, task }),
        )
    }

    fn build(
        platform: P,
        store: ClientStore,
        source: OrchestrationV2CreationSource,
        spawn: Spawner,
        authorization: RemoteAuthorization,
        relay: Option<Relay>,
    ) -> Self {
        let composer_drafts = Rc::new(
            crate::thread_views::composer::draft_service::ComposerDraftService::new(Rc::new(
                platform.clone(),
            )),
        );
        let composer_preferences = Rc::new(
            crate::thread_views::composer::preferences::ComposerPreferences::new(Rc::new(
                platform.clone(),
            )),
        );
        let (composer_task, composer_registration) = AbortHandle::new_pair();
        let draft_driver = composer_drafts.clone().run();
        let preferences_driver = composer_preferences.clone().run();
        spawn(
            Abortable::new(
                async move {
                    futures::join!(draft_driver, preferences_driver);
                },
                composer_registration,
            )
            .map(|_| ())
            .boxed_local(),
        );
        Self {
            inner: Rc::new(Inner {
                navigation: crate::navigation::UrlClient::new(platform.clone()),
                new_threads: Rc::default(),
                composer_drafts,
                composer_preferences,
                composer_task,
                composer_models: Rc::default(),
                composer_updates: Default::default(),
                composer_update_dismissals: Rc::default(),
                composer_stash: Rc::default(),
                composer_questions: Rc::default(),
                composer_attachments: RefCell::default(),
                platform,
                store,
                git_panel: Rc::default(),
                project_files: Rc::default(),
                source,
                spawn,
                environments: RefCell::new(Vec::new()),
                threads: RefCell::new(BTreeMap::new()),
                listeners: RefCell::new(Vec::new()),
                message_listeners: Rc::default(),
                queries: RefCell::new(BTreeMap::new()),
                registry: Mutex::new(()),
                thread_operations: Rc::default(),
                terminals: Rc::default(),
                command_outputs: RefCell::new(BTreeMap::new()),
                settled_outputs: Rc::default(),
                authorization,
                relay,
                damaged: RefCell::default(),
                storage_notice: RefCell::default(),
                restored: std::cell::Cell::default(),
            }),
        }
    }

    /// The agent sign-in consent of the environment that serves the page at `origin`
    /// (`/connect-agent`), with the browser's own session.
    pub fn agent_consent(
        &self,
        origin: &str,
    ) -> crate::connection::agent_consent::AgentConsent<P> {
        crate::connection::agent_consent::AgentConsent::new(self.inner.platform.clone(), origin)
    }

    pub fn navigation(&self) -> crate::navigation::UrlClient<P> {
        self.inner.navigation.clone()
    }

    pub fn new_threads(&self) -> crate::navigation::new_thread::NewThreadRuntime<P> {
        crate::navigation::new_thread::NewThreadRuntime::new(
            self.clone(),
            self.inner.platform.clone(),
            self.inner.new_threads.clone(),
        )
    }

    pub fn palette(&self) -> crate::navigation::palette_service::PaletteService<P> {
        crate::navigation::palette_service::PaletteService::new(
            self.clone(),
            self.inner.platform.clone(),
        )
    }

    pub fn add_project(&self) -> crate::navigation::project_flow::AddProjectSession<P> {
        let runtime = self.clone();
        crate::navigation::project_flow::AddProjectSession::new(
            self.clone(),
            self.inner.platform.clone(),
            Rc::new(move |future| {
                runtime.spawn(future);
            }),
        )
    }

    pub fn ignored_files(
        &self,
        project: crate::workspace::ProjectKey,
    ) -> crate::navigation::ignored_selection::IgnoredFilesSession<P> {
        crate::navigation::ignored_selection::IgnoredFilesSession::new(
            self.clone(),
            self.inner.platform.clone(),
            project,
        )
    }

    pub fn store(&self) -> &ClientStore {
        &self.inner.store
    }

    /// The workspace Files panel: tree, access, workspace target and file reads.
    pub fn project_files(&self) -> crate::workspace::project_files::ProjectFiles<P> {
        crate::workspace::project_files::ProjectFiles::new(
            self.clone(),
            self.inner.platform.clone(),
            self.inner.project_files.clone(),
        )
    }

    pub fn git_panel(&self) -> crate::workspace::git_panel::GitPanel<P> {
        crate::workspace::git_panel::GitPanel::new(
            self.clone(),
            self.inner.platform.clone(),
            self.inner.spawn.clone(),
            self.inner.git_panel.clone(),
        )
    }

    /// The right panel's diff view (DiffPanel.tsx), sharing the git panel's diff selection.
    pub fn diff_panel(&self) -> crate::workspace::diff_panel::DiffPanelService<P> {
        crate::workspace::diff_panel::DiffPanelService::new(
            self.clone(),
            self.inner.platform.clone(),
            self.inner.git_panel.clone(),
        )
    }

    pub fn branch_selector(
        &self,
        key: &ThreadKey,
    ) -> crate::workspace::git_panel::branches::BranchSelectorLease<P> {
        self.git_panel().branch_selector(key)
    }

    pub fn editor_picker(
        &self,
        key: &ThreadKey,
    ) -> crate::workspace::git_panel::editors::EditorPicker<P> {
        self.git_panel().editor_picker(key)
    }

    pub fn project_scripts(
        &self,
        key: &ThreadKey,
    ) -> crate::workspace::git_panel::scripts::ProjectScripts<P> {
        self.git_panel().project_scripts(key)
    }

    pub fn thread_details(&self) -> crate::workspace::git_panel::GitPanel<P> {
        self.git_panel()
    }

    pub fn approval_services(&self) -> crate::thread_views::thread_ops::ThreadOperations<P> {
        crate::thread_views::thread_ops::ThreadOperations::new(
            self.clone(),
            self.inner.platform.clone(),
            self.inner.thread_operations.clone(),
        )
    }

    pub fn plan_services(&self) -> crate::thread_views::thread_ops::ThreadOperations<P> {
        self.approval_services()
    }

    pub fn thread_actions(&self) -> crate::thread_views::thread_ops::ThreadOperations<P> {
        self.approval_services()
    }

    pub fn project_actions(&self) -> crate::thread_views::thread_ops::ThreadOperations<P> {
        self.approval_services()
    }

    pub fn thread_header(&self) -> crate::thread_views::thread_ops::ThreadOperations<P> {
        self.approval_services()
    }

    pub fn thread_metadata(&self) -> crate::thread_views::thread_ops::ThreadOperations<P> {
        self.approval_services()
    }

    pub fn timeline_rewind(&self) -> crate::thread_views::thread_ops::ThreadOperations<P> {
        self.approval_services()
    }

    /// The setup card of a thread's new worktree (useWorktreeSetup): snapshot, cancel, terminal.
    pub fn worktree_setup(&self) -> crate::thread_views::worktree_setup::WorktreeSetup<P> {
        crate::thread_views::worktree_setup::WorktreeSetup::new(
            self.clone(),
            self.inner.platform.clone(),
        )
    }

    /// Signed asset URLs (useAssetUrlState), shared by every reader of one resource.
    pub fn asset_urls(&self) -> crate::services::asset_urls::AssetUrls<P> {
        crate::services::asset_urls::AssetUrls::new(self.clone(), self.inner.platform.clone())
    }

    /// The thread sidebar's stored width (`chat_thread_sidebar_width`).
    pub fn sidebar_width(&self) -> crate::view_logic::shell::sidebar_width::SidebarWidth<P> {
        crate::view_logic::shell::sidebar_width::SidebarWidth::new(self.inner.platform.clone())
    }

    /// The sidebar shelves' expanded flags (`t3code:sidebar:{working,snoozed,settled}-expanded`).
    pub fn sidebar_shelves(&self) -> crate::view_logic::shell::sidebar_list::SidebarShelves<P> {
        crate::view_logic::shell::sidebar_list::SidebarShelves::new(self.inner.platform.clone())
    }

    /// The address this host would open for `raw` (`external_links::safe_external_url` for its
    /// surface), or `None` when the policy refuses it: what a UI link may carry as its `href`.
    pub fn external_link(&self, raw: &str) -> Option<String> {
        crate::persistence::external_links::safe_external_url(
            raw,
            self.inner.platform.external_url_surface(),
        )
    }

    /// Opens an ordinary link outside the app through the host's policy (`Platform::open_url`);
    /// a refused address is an error and nothing opens.
    pub fn open_external_url(
        &self,
        url: String,
    ) -> LocalBoxFuture<'static, Result<(), crate::platform::PlatformError>> {
        let platform = self.inner.platform.clone();
        async move { platform.open_url(url).await }.boxed_local()
    }

    /// Threads' terminals: tabs and groups, sessions, known sessions and commands.
    pub fn terminals(&self) -> crate::services::terminal_sessions::TerminalSessions<P> {
        crate::services::terminal_sessions::TerminalSessions::new(
            self.clone(),
            self.inner.platform.clone(),
            self.inner.terminals.clone(),
        )
    }

    /// Supervises every environment saved by an earlier run, in catalogue order, with its saved
    /// credential. Returns without waiting for any server: each one connects, or retries with
    /// backoff, in the background. A disabled environment is listed and left disconnected.
    pub fn restore(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let this = self.clone();
        self.owned(
            async move {
                let _lease = this.inner.registry.lock().await;
                let registry = this.load_registry().await?;
                let reset_catalog = registry.reset_catalog();
                let saved: Vec<_> = registry
                    .entries()
                    .map(|registration| (registration.id.clone(), registration.label.clone()))
                    .collect();
                for (id, label) in saved {
                    if let Err(error) = this.start_saved(&id).await {
                        this.list(id.clone(), label);
                        this.fail(&id, session_failure(&error));
                    }
                }
                if !this.inner.restored.replace(true) {
                    this.follow_route_policy();
                    let notice = StorageNotice {
                        storage: this.inner.platform.storage_report(),
                        reset_catalog,
                        damaged: this.inner.damaged.borrow().iter().cloned().collect(),
                    };
                    // Saved until the UI takes it, with any an earlier launch never showed.
                    let notice =
                        crate::services::storage_notice::remember(&this.inner.platform, notice)
                            .await;
                    if notice != StorageNotice::default() {
                        this.inner.storage_notice.replace(Some(notice));
                    }
                }
                drop(_lease);
                this.refresh_relay().await;
                Ok(())
            }
            .boxed_local(),
            || Err(ClientError::Cancelled),
        )
    }

    /// Saved data the first restore could not keep (a reset environment list, unreadable
    /// records, discarded credentials), once; `None` after it was taken or when nothing was lost.
    pub fn take_storage_notice(&self) -> Option<StorageNotice> {
        let notice = self.inner.storage_notice.take();
        if notice.is_some() {
            let platform = self.inner.platform.clone();
            (self.inner.spawn)(
                async move { crate::services::storage_notice::acknowledge(&platform).await }
                    .boxed_local(),
            );
        }
        notice
    }

    /// The route policy (`RoutePolicy::for_client`) once the first restore has read the saved
    /// environments; `None` before, when no route guard may act on a guess.
    pub fn route_policy(&self) -> Option<crate::navigation::RoutePolicy> {
        self.inner.restored.get().then(|| {
            crate::navigation::RoutePolicy::for_client(!self.inner.environments.borrow().is_empty())
        })
    }

    /// Hands the platform the route policy now and on every change while the runtime lives, so
    /// the host's location snapshots carry it.
    fn follow_route_policy(&self) {
        let Some(mut last) = self.route_policy() else {
            return;
        };
        self.inner.platform.set_route_policy(last);
        let inner = Rc::downgrade(&self.inner);
        let mut changes = self.changes();
        (self.inner.spawn)(
            async move {
                while changes.next().await.is_some() {
                    let Some(this) = inner.upgrade().map(|inner| Self { inner }) else {
                        break;
                    };
                    if let Some(policy) = this.route_policy().filter(|policy| *policy != last) {
                        this.inner.platform.set_route_policy(policy);
                        last = policy;
                    }
                }
            }
            .boxed_local(),
        );
    }

    /// Pairing requests from links the OS hands the running app (mobile deep links), for the
    /// UI to confirm and pass to [`Self::pair`].
    pub fn pairing_links(&self) -> LocalBoxStream<'static, PairingInput> {
        self.inner
            .platform
            .events()
            .filter_map(|event| async move {
                match event {
                    crate::platform::PlatformEvent::OpenedLink(link) => {
                        crate::connection::pairing_link(&link)
                    }
                    _ => None,
                }
            })
            .boxed_local()
    }

    /// Launches T3 Code on an SSH host through the desktop (`Platform::ssh_provision`), saves it
    /// with the token it returns, then connects in the background through the tunnel the
    /// desktop keeps (`Platform::ssh_prepare`). Ports TypeScript's `registerSsh`.
    pub fn pair_ssh(
        &self,
        input: SshPairingInput,
    ) -> LocalBoxFuture<'static, Result<EnvironmentId, ConnectionFailure>> {
        let this = self.clone();
        self.owned(
            async move {
                let _lease = this.inner.registry.lock().await;
                let platform = this.inner.platform.clone();
                let prepared = prepare_ssh_registration(
                    &platform,
                    input.target,
                    input.label.as_deref(),
                    input.expected_environment.as_ref(),
                )
                .await?;
                let id = prepared.registration.id.clone();
                let mut registry = this
                    .load_registry()
                    .await
                    .map_err(|error| session_failure(&error))?;
                registry
                    .register_prepared(
                        &platform,
                        prepared,
                        false,
                        false,
                        NetworkStatus::Unknown,
                        Time::from_platform(&platform),
                    )
                    .await
                    .map_err(|error| session_failure(&error))?;
                this.start_saved(&id)
                    .await
                    .map_err(|error| session_failure(&error))?;
                Ok(id)
            }
            .boxed_local(),
            || {
                Err(ConnectionFailure::new(
                    crate::connection::FailureKind::Transport,
                    "the client stopped before pairing finished",
                ))
            },
        )
    }

    /// What relay discovery knows: the account's linked environments and their health. `None`
    /// without T3 Connect.
    pub fn relay_environments(&self) -> Option<RelayDiscoveryState> {
        Some(self.inner.relay.as_ref()?.discovery.snapshot())
    }

    /// Fires when [`Self::relay_environments`] may answer differently; never without T3 Connect.
    pub fn relay_changes(&self) -> LocalBoxStream<'static, ()> {
        match &self.inner.relay {
            Some(relay) => relay.discovery.changes().map(|_| ()).boxed_local(),
            None => futures::stream::pending().boxed_local(),
        }
    }

    /// Lists the signed-in account's environments again (discovery also refreshes when the
    /// host's credentials change).
    pub fn refresh_relay(&self) -> LocalBoxFuture<'static, ()> {
        let Some(discovery) = self
            .inner
            .relay
            .as_ref()
            .map(|relay| relay.discovery.clone())
        else {
            return futures::future::ready(()).boxed_local();
        };
        let platform = self.inner.platform.clone();
        async move {
            match platform.cloud_token().await {
                Ok(token) => {
                    discovery
                        .refresh(token.map(crate::authorization::Sensitive))
                        .await
                }
                Err(_) => discovery.refresh(None).await,
            }
        }
        .boxed_local()
    }

    /// Saves a discovered environment's relay route (no stored credential: each connection is
    /// minted for this device) and connects through it. An environment already saved with
    /// direct routes keeps them and gains the relay as another route.
    pub fn add_relay_environment(
        &self,
        environment: &EnvironmentId,
        label: &str,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let this = self.clone();
        let environment = environment.clone();
        let label = label.to_owned();
        self.owned(
            async move {
                if this.inner.relay.is_none() {
                    return Err(ClientError::Unsupported("T3 Connect is not configured"));
                }
                let _lease = this.inner.registry.lock().await;
                let platform = this.inner.platform.clone();
                let mut registry = this.load_registry().await?;
                registry.register(
                    EnvironmentRegistration {
                        id: environment.clone(),
                        label,
                        routes: vec![Route {
                            id: RELAY_ROUTE_ID.into(),
                            environment: environment.clone(),
                            mode: ConnectionMode::Relay,
                            http_base: String::new(),
                            ws_base: String::new(),
                        }],
                        enabled: true,
                    },
                    false,
                    NetworkStatus::Unknown,
                    Time::from_platform(&platform),
                )?;
                registry.save(&platform).await?;
                this.start_saved(&environment).await
            }
            .boxed_local(),
            || Err(ClientError::Cancelled),
        )
    }

    /// After signing out of T3 Connect: drops every relay route. An environment reachable only
    /// through the relay is removed as [`Self::remove`] does; one with direct routes keeps them
    /// and reconnects over them. Ports TypeScript's `removeRelayEnvironments`.
    pub fn remove_relay_environments(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let this = self.clone();
        self.owned(
            async move {
                let _lease = this.inner.registry.lock().await;
                let platform = this.inner.platform.clone();
                let mut registry = this.load_registry().await?;
                let relayed: Vec<_> = registry
                    .entries()
                    .filter(|registration| {
                        registration
                            .routes
                            .iter()
                            .any(|route| route.mode == ConnectionMode::Relay)
                    })
                    .map(|registration| (registration.id.clone(), registration.routes.len() == 1))
                    .collect();
                // Drop the relay routes of environments that keep a direct route and save that
                // first: each removal below loads and saves the catalog itself, which would leave
                // this registry's copy stale.
                let (only_relay, kept): (Vec<_>, Vec<_>) =
                    relayed.into_iter().partition(|(_, only_route)| *only_route);
                for (environment, _) in &kept {
                    registry.remove_route(
                        environment,
                        RELAY_ROUTE_ID,
                        Time::from_platform(&platform),
                    )?;
                }
                if !kept.is_empty() {
                    registry.save(&platform).await?;
                }
                for (environment, _) in only_relay {
                    this.remove_locked(&environment).await?;
                }
                for (environment, _) in kept {
                    this.start_saved(&environment).await?;
                }
                Ok(())
            }
            .boxed_local(),
            || Err(ClientError::Cancelled),
        )
    }

    /// What failed on the way to `environment`, as Settings shows it, or `None` while it is
    /// connected, off, or has not failed. Ports the web's `useConnectionDiagnosis`: the relay's
    /// health counts only when the relay is the environment's one route.
    pub fn diagnosis(
        &self,
        environment: &EnvironmentId,
        client_version: Option<&str>,
    ) -> Option<ConnectionDiagnosis> {
        let entry = self
            .environments()
            .into_iter()
            .find(|entry| &entry.id == environment)?;
        if !entry.state.desired || entry.state.phase == Phase::Connected {
            return None;
        }
        let relay_only =
            matches!(entry.routes.as_slice(), [route] if route.mode == ConnectionMode::Relay);
        let relay_status = relay_only
            .then(|| self.relay_environments())
            .flatten()
            .and_then(|state| state.environments.get(environment)?.status.clone());
        let server_version = self
            .inner
            .store
            .snapshot()
            .environments
            .get(environment)
            .and_then(|state| state.config.as_ref())
            .map(|config| config.environment.server_version.0.clone());
        present_connection_diagnosis(DiagnosisInput {
            host_label: &entry.label,
            state: &entry.state,
            via: if relay_only {
                ConnectionVia::Relay
            } else {
                ConnectionVia::Direct
            },
            relay_status: relay_status.as_ref(),
            client_version,
            server_version: server_version.as_deref(),
        })
    }

    /// Pairs with the server the user described and saves it, then connects in the background.
    /// The address and the expected environment are checked before the one-time code is spent,
    /// and a server that needs an update is still saved so it can be updated from here
    /// (`connection::prepare_pairing_registration`, as TypeScript's onboarding).
    pub fn pair(
        &self,
        input: PairingInput,
    ) -> LocalBoxFuture<'static, Result<EnvironmentId, ConnectionFailure>> {
        let this = self.clone();
        self.owned(
            async move {
                let _lease = this.inner.registry.lock().await;
                let platform = this.inner.platform.clone();
                let mut registry = this
                    .load_registry()
                    .await
                    .map_err(|error| session_failure(&error))?;
                let prepared = prepare_pairing_registration(&platform, &input, |route| {
                    registry.assert_known_address(route)
                })
                .await?;
                let id = prepared.registration.id.clone();
                registry
                    .register_prepared(
                        &platform,
                        prepared,
                        input.expected_environment.is_none(),
                        true,
                        NetworkStatus::Unknown,
                        Time::from_platform(&platform),
                    )
                    .await
                    .map_err(|error| session_failure(&error))?;
                this.start_saved(&id)
                    .await
                    .map_err(|error| session_failure(&error))?;
                Ok(id)
            }
            .boxed_local(),
            || {
                Err(ConnectionFailure::new(
                    crate::connection::FailureKind::Transport,
                    "the client stopped before pairing finished",
                ))
            },
        )
    }

    /// Asks `environment` to connect again now. A connection whose supervisor stopped on an
    /// error is started afresh from the saved registration.
    pub fn reconnect(
        &self,
        environment: &EnvironmentId,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let this = self.clone();
        let environment = environment.clone();
        self.owned(
            async move {
                let _lease = this.inner.registry.lock().await;
                if let Some(client) = this.client(&environment)
                    && client.reconnect().is_ok()
                {
                    return Ok(());
                }
                if this.load_registry().await?.get(&environment).is_none() {
                    return Err(ClientError::EnvironmentMismatch);
                }
                this.start_saved(&environment).await
            }
            .boxed_local(),
            || Err(ClientError::Cancelled),
        )
    }

    /// Runs a change of several steps on the runtime's own spawner and returns a future that
    /// only observes it, so a caller dropped part-way (its screen disposed, a navigation, a
    /// closing window, an app sent to the background) cannot leave it half done: a removal
    /// stopped at the driver's end left the environment's config and session in the store.
    /// `stopped` answers if the runtime itself stops first.
    fn owned<T: 'static>(
        &self,
        work: LocalBoxFuture<'static, T>,
        stopped: fn() -> T,
    ) -> LocalBoxFuture<'static, T> {
        let (done, result) = futures::channel::oneshot::channel();
        (self.inner.spawn)(
            async move {
                let _ = done.send(work.await);
            }
            .boxed_local(),
        );
        async move { result.await.unwrap_or_else(|_| stopped()) }.boxed_local()
    }

    /// Disconnects `environment`, ends its follows and delivery, removes its saved
    /// registration and credentials, and drops its data from the store.
    pub fn remove(
        &self,
        environment: &EnvironmentId,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let this = self.clone();
        let environment = environment.clone();
        self.owned(
            async move {
                let _lease = this.inner.registry.lock().await;
                this.remove_locked(&environment).await
            }
            .boxed_local(),
            || Err(ClientError::Cancelled),
        )
    }

    /// The saved environments, in catalogue order.
    pub fn environments(&self) -> Vec<EnvironmentEntry> {
        self.inner
            .environments
            .borrow()
            .iter()
            .map(|entry| EnvironmentEntry {
                id: entry.id.clone(),
                label: entry.label.clone(),
                state: match (&entry.client, &entry.failure) {
                    (Some(client), _) => client.state(),
                    (None, failure) => ConnectionState {
                        phase: Phase::Blocked,
                        last_failure: failure.clone(),
                        ..ConnectionState::default()
                    },
                },
                routes: entry.routes.clone(),
                damaged: self.inner.damaged.borrow().contains(&entry.id),
            })
            .collect()
    }

    /// Fires whenever [`Self::environments`] may answer differently: the list changed or an
    /// environment's connection state changed. Coalesces; read `environments` on each item.
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        let (sender, receiver) = mpsc::unbounded();
        self.inner.listeners.borrow_mut().push(sender);
        let connections = self
            .inner
            .store
            .subscribe()
            .filter_map(|change| async move {
                change
                    .scopes
                    .iter()
                    .any(|scope| matches!(scope, Scope::All | Scope::Environment(_)))
                    .then_some(())
            });
        futures::stream::select(receiver, connections).boxed_local()
    }

    /// Follows a thread while a screen shows it: snapshot, then live events, on every session of
    /// its environment. Opens are counted; [`Self::close_thread`] undoes one.
    pub fn open_thread(&self, key: &ThreadKey) {
        let lease = {
            let mut threads = self.inner.threads.borrow_mut();
            let entry = threads.entry(key.clone()).or_insert((0, None));
            entry.0 += 1;
            if entry.0 > 1 {
                return;
            }
            self.client(&key.environment)
                .map(|client| client.open_thread(key.thread.clone()))
        };
        if let Some(lease) = lease
            && let Some(entry) = self.inner.threads.borrow_mut().get_mut(key)
        {
            entry.1 = Some(lease);
        }
    }

    /// Stops following a thread no screen shows. Its projection stays readable in the store.
    pub fn close_thread(&self, key: &ThreadKey) {
        let released = {
            let mut threads = self.inner.threads.borrow_mut();
            match threads.get_mut(key) {
                Some(entry) if entry.0 > 1 => {
                    entry.0 -= 1;
                    None
                }
                Some(_) => threads.remove(key),
                None => None,
            }
        };
        drop(released);
    }

    /// Sends a user message as a new turn on `key`, through the durable outbox. Accepted means
    /// written to the outbox; delivery and its outcome follow in the store and `SendService`.
    pub fn send_message(
        &self,
        key: &ThreadKey,
        text: String,
    ) -> LocalBoxFuture<'static, Result<(), SendError>> {
        let key = key.clone();
        let service = self
            .inner
            .environments
            .borrow()
            .iter()
            .find(|entry| entry.id == key.environment)
            .filter(|entry| {
                entry
                    .client
                    .as_ref()
                    .is_some_and(|client| client.session().is_some())
            })
            .and_then(|entry| entry.send.clone());
        async move {
            if text.trim().is_empty() {
                return Err(SendError::Rejected("Enter a message.".into()));
            }
            let service = service.ok_or(SendError::Disconnected)?;
            match service.send_message(&key, text).await {
                Ok(_) => Ok(()),
                Err(ClientError::Disconnected) => Err(SendError::Disconnected),
                Err(error) => Err(SendError::Rejected(error.to_string())),
            }
        }
        .boxed_local()
    }

    /// The message delivery for `environment`, for pending messages and their states.
    pub fn messages(&self, environment: &EnvironmentId) -> Option<SendService<P, SessionEndpoint>> {
        self.inner
            .environments
            .borrow()
            .iter()
            .find(|entry| &entry.id == environment)
            .and_then(|entry| entry.send.clone())
    }

    /// The connected client for `environment`, for services that need its guarded session.
    pub fn client(&self, environment: &EnvironmentId) -> Option<ConnectedClient> {
        self.inner
            .environments
            .borrow()
            .iter()
            .find(|entry| &entry.id == environment)
            .and_then(|entry| entry.client.clone())
    }

    /// A server query for `environment` as a resource screens read. Every lease of the same
    /// method and input shares one driver, which runs while any lease is held: it waits for a
    /// session, re-runs on each new session, and follows the policy's staleness and refresh.
    pub fn query<M>(
        &self,
        environment: &EnvironmentId,
        input: M::Input,
        policy: ResourcePolicy,
    ) -> QueryLease<M::Output>
    where
        M: RpcMethod + 'static,
        M::Input: Clone + serde::Serialize,
        M::Output: PartialEq + serde::Serialize + 'static,
    {
        let identity = format!(
            "{}\u{0}{}\u{0}{}",
            environment.0,
            M::NAME,
            serde_json::to_string(&input).unwrap_or_default()
        );
        let existing = self
            .inner
            .queries
            .borrow()
            .get(&identity)
            .and_then(std::rc::Weak::upgrade);
        if let Some(shared) =
            existing.and_then(|shared| shared.downcast::<QueryShared<M::Output>>().ok())
        {
            return QueryLease { shared };
        }
        let resource = Rc::new(Resource::new(policy, Generation(0)));
        let connections = self.query_connections(environment);
        let driver = {
            let resource = resource.clone();
            let platform = self.inner.platform.clone();
            let environment = environment.clone();
            async move {
                let _ = drive_query::<M, SessionEndpoint, P>(
                    &resource,
                    &platform,
                    environment,
                    connections,
                    input,
                    None,
                    |_| {},
                )
                .await;
                resource.close();
            }
            .boxed_local()
        };
        let shared = Rc::new(QueryShared {
            resource,
            driver: self.spawn(driver),
        });
        let erased: Rc<dyn std::any::Any> = shared.clone();
        let mut queries = self.inner.queries.borrow_mut();
        queries.retain(|_, query| query.strong_count() > 0);
        queries.insert(identity, Rc::downgrade(&erased));
        QueryLease { shared }
    }

    /// The diff of a thread's turns, `orchestration.getTurnDiff`.
    pub fn turn_diff(
        &self,
        key: &ThreadKey,
        from_turn_count: u64,
        to_turn_count: u64,
        ignore_whitespace: bool,
    ) -> QueryLease<t3_contracts::checkpoint_diff::OrchestrationGetFullThreadDiffResult> {
        self.query::<t3_contracts::rpc::WsOrchestrationV2GetTurnDiffRpc>(
            &key.environment,
            t3_contracts::checkpoint_diff::OrchestrationGetTurnDiffInput {
                from_turn_count,
                to_turn_count,
                thread_id: key.thread.clone(),
                ignore_whitespace: ignore_whitespace.then_some(true),
            },
            ResourcePolicy::default(),
        )
    }

    /// The environment's connection as query drivers read it: the current state first, then
    /// each change of phase, failure or session, with one endpoint object per session.
    /// Whether `environment`'s server streams command output (its config's
    /// `commandOutputStreaming`); open a [`Self::command_output`] lease only when it does.
    pub fn command_output_streaming(&self, environment: &EnvironmentId) -> bool {
        self.inner
            .store
            .snapshot()
            .environments
            .get(environment)
            .and_then(|state| state.config.as_ref())
            .is_some_and(|config| {
                config.environment.capabilities.command_output_streaming == Some(true)
            })
    }

    /// The output of one command row while the row is open: streamed while the command runs,
    /// then final. Readers of one row share one subscription, which ends with the last lease
    /// or once the command settles; a settled command's output is kept for five minutes after
    /// its last reader leaves, so reopening the row does not fetch it again.
    pub fn command_output(
        &self,
        key: &ThreadKey,
        item: &t3_contracts::base_schemas::TurnItemId,
    ) -> CommandOutputLease {
        let identity = format!("{}\u{0}{}\u{0}{}", key.environment.0, key.thread.0, item.0);
        let live = self
            .inner
            .command_outputs
            .borrow()
            .get(&identity)
            .and_then(std::rc::Weak::upgrade);
        if let Some(shared) = live {
            return CommandOutputLease { shared };
        }
        let now = self.inner.platform.monotonic_ms();
        let settled = {
            let mut settled = self.inner.settled_outputs.borrow_mut();
            settled.retain(|_, (_, until)| *until > now);
            settled.get(&identity).map(|(cell, _)| cell.clone())
        };
        let platform = self.inner.platform.clone();
        let (cell, driver) = match settled {
            Some(cell) => (cell, None),
            None => {
                let cell = Rc::new(CommandOutputCell::default());
                let input =
                    t3_contracts::orchestration_v2::OrchestrationV2SubscribeCommandOutputInput {
                        thread_id: key.thread.clone(),
                        item_id: item.clone(),
                    };
                let sessions = self
                    .query_connections(&key.environment)
                    .map(|connection| connection.session)
                    .boxed_local();
                let driven = cell.clone();
                let environment = key.environment.clone();
                let host = self.inner.platform.clone();
                let driver = self.spawn(
                    async move {
                        type Method = t3_contracts::rpc::WsOrchestrationV2SubscribeCommandOutputRpc;
                        // A stream that ends or fails to decode on a healthy socket resubscribes
                        // with the shared bounded retry (WP5), as TypeScript's subscriptions do.
                        let frames = EnvironmentStream::recovering(
                            environment,
                            sessions,
                            &host,
                            <Method as RpcMethod>::NAME,
                            move |session| session.endpoint.subscribe::<Method>(input.clone()),
                        );
                        drive_command_output(driven, frames).await;
                    }
                    .boxed_local(),
                );
                (cell, Some(driver))
            }
        };
        let shared = Rc::new(CommandOutputShared {
            cell,
            driver,
            cache: Some(SettledCache {
                identity: identity.clone(),
                cells: self.inner.settled_outputs.clone(),
                now: Rc::new(move || platform.monotonic_ms()),
            }),
        });
        let mut outputs = self.inner.command_outputs.borrow_mut();
        outputs.retain(|_, output| output.strong_count() > 0);
        outputs.insert(identity, Rc::downgrade(&shared));
        CommandOutputLease { shared }
    }

    pub(crate) fn query_connections(
        &self,
        environment: &EnvironmentId,
    ) -> LocalBoxStream<'static, QueryConnection<SessionEndpoint>> {
        let runtime = Rc::downgrade(&self.inner);
        let environment = environment.clone();
        let changes = self.inner.store.subscribe_environment(&environment);
        let mut last: Option<(QueryConnectionPhase, Option<String>, Option<Generation>)> = None;
        let mut session: Option<ServiceSession<SessionEndpoint>> = None;
        changes
            .filter_map(move |_| {
                let client = runtime.upgrade().and_then(|inner| {
                    inner
                        .environments
                        .borrow()
                        .iter()
                        .find(|entry| entry.id == environment)
                        .and_then(|entry| entry.client.clone())
                });
                let state = client
                    .as_ref()
                    .map(ConnectedClient::state)
                    .unwrap_or_default();
                let endpoint = client.as_ref().and_then(ConnectedClient::session);
                let generation = endpoint.as_ref().map(|_| state.generation);
                if session.as_ref().map(|session| session.identity.generation) != generation {
                    session = endpoint.map(|endpoint| ServiceSession {
                        identity: crate::environment::SessionIdentity {
                            environment: environment.clone(),
                            generation: state.generation,
                            account: None,
                            dpop_thumbprint: None,
                        },
                        endpoint: Rc::new(endpoint),
                    });
                }
                let phase = match state.phase {
                    Phase::Available => QueryConnectionPhase::Available,
                    Phase::Offline => QueryConnectionPhase::Offline,
                    Phase::Connecting => QueryConnectionPhase::Connecting,
                    Phase::Backoff => QueryConnectionPhase::Backoff,
                    Phase::Connected => QueryConnectionPhase::Connected,
                    Phase::Blocked => QueryConnectionPhase::Blocked,
                };
                let failure = state.last_failure.map(|failure| failure.detail);
                let next = (phase, failure.clone(), generation);
                let changed = last.as_ref() != Some(&next);
                last = Some(next);
                let connection = changed.then(|| QueryConnection {
                    phase,
                    failure,
                    session: session.clone(),
                });
                async move { connection }
            })
            .boxed_local()
    }

    /// The thread's messages this client has not seen projected yet: queued, sending, waiting
    /// to retry (with the reason), and refused (`SendState::Restored`, with the server's reason)
    /// until [`Self::dismiss_message`]. Empty when the environment has no delivery.
    pub fn pending_messages(&self, key: &ThreadKey) -> Vec<PendingMessage> {
        self.messages(&key.environment)
            .map(|send| send.pending_messages(key))
            .unwrap_or_default()
    }

    /// Fires when any environment's pending messages may have changed, and when an
    /// environment's delivery starts or ends. Coalesce, then read `pending_messages`.
    pub fn message_changes(&self) -> LocalBoxStream<'static, ()> {
        let (sender, receiver) = mpsc::unbounded();
        self.inner.message_listeners.borrow_mut().push(sender);
        receiver.boxed_local()
    }

    /// Stops showing a refused message; its text stays in the thread's draft.
    pub fn dismiss_message(&self, key: &ThreadKey, id: &t3_contracts::base_schemas::MessageId) {
        if let Some(send) = self.messages(&key.environment) {
            send.dismiss_message(id);
        }
    }

    /// The draft a refused message of `key` was restored into, for the composer.
    /// Why `environment`'s delivery is stalled (its saved messages could not be read or
    /// written), while it waits to try again.
    pub fn delivery_problem(&self, environment: &EnvironmentId) -> Option<String> {
        self.messages(environment)?.delivery_problem()
    }

    pub fn draft_text(&self, key: &ThreadKey) -> Option<String> {
        self.messages(&key.environment)?.draft_text(key)
    }

    fn notify_messages(&self) {
        notify_all(&self.inner.message_listeners);
    }

    fn notify(&self) {
        self.inner
            .listeners
            .borrow_mut()
            .retain(|listener| listener.unbounded_send(()).is_ok());
    }

    /// The saved catalog, bound to the shared authorization so a removal forgets the
    /// environment's sessions and a later registration allows it again.
    async fn load_registry(&self) -> Result<EnvironmentRegistry<SessionEndpoint>, ClientError> {
        let platform = &self.inner.platform;
        let mut registry = EnvironmentRegistry::load(
            platform,
            NetworkStatus::Unknown,
            Time::from_platform(platform),
        )
        .await?;
        registry.bind_authorization(self.inner.authorization.clone());
        Ok(registry)
    }

    /// Saves the direct addresses an environment announces, under the registry lease so it
    /// cannot race a pairing or a removal. Weak, so a running driver does not keep the client
    /// alive. A failure keeps the routes as they were, as TypeScript logs and goes on.
    fn route_learner(&self) -> LearnRoutes {
        let inner = Rc::downgrade(&self.inner);
        Rc::new(move |environment, active_route, reported| {
            let inner = inner.clone();
            async move {
                let this = Self {
                    inner: inner.upgrade()?,
                };
                let _lease = this.inner.registry.lock().await;
                // Plain HTTP is unusable from an HTTPS page (mixed content), as in TypeScript.
                let https_page = this
                    .inner
                    .platform
                    .app_url()
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|url| url.to_ascii_lowercase().starts_with("https:"));
                let mut registry = this.load_registry().await.ok()?;
                registry
                    .learn_routes(
                        &this.inner.platform,
                        &environment,
                        &active_route,
                        &reported,
                        !https_page,
                    )
                    .await
                    .ok()
                    .flatten()
            }
            .boxed_local()
        })
    }

    /// Starts supervising a saved environment and lists it; the caller holds the registry lease.
    async fn start_saved(&self, id: &EnvironmentId) -> Result<(), ClientError> {
        let registry = self.load_registry().await?;
        let Some(registration) = registry.get(id).map(|handle| handle.registration().clone())
        else {
            return Ok(());
        };
        if registry.is_damaged(id) {
            self.inner.damaged.borrow_mut().insert(id.clone());
        } else {
            self.inner.damaged.borrow_mut().remove(id);
        }
        let (client, driver) = supervise_saved(
            self.inner.platform.clone(),
            self.inner.store.clone(),
            self.inner.authorization.clone(),
            id,
            Some(self.route_learner()),
        )
        .await?;
        self.attach(client, driver, registration.label, registration.routes);
        Ok(())
    }

    /// Disconnects and forgets `environment`: follows, delivery, saved registration and
    /// credentials, queued messages, store data, and a managed SSH backend. The caller holds
    /// the registry lease.
    async fn remove_locked(&self, environment: &EnvironmentId) -> Result<(), ClientError> {
        self.inner.damaged.borrow_mut().remove(environment);
        self.inner
            .threads
            .borrow_mut()
            .retain(|key, _| &key.environment != environment);
        let removed = {
            let mut environments = self.inner.environments.borrow_mut();
            environments
                .iter()
                .position(|entry| &entry.id == environment)
                .map(|index| environments.remove(index))
        };
        // The driver publishes its last connection state while it stops; clear the store after.
        if let Some(finished) = removed.and_then(shut_down) {
            let _ = finished.await;
        }
        self.inner.store.remove_environment(environment);
        self.inner.thread_operations.forget_environment(environment);
        self.notify();
        self.notify_messages();
        let platform = self.inner.platform.clone();
        let mut registry = self.load_registry().await?;
        let mut ssh_targets = Vec::new();
        let mut secrets = Vec::new();
        let mut connections = Vec::new();
        if let Some(handle) = registry.get(environment) {
            let stores = ConnectionStores::new(Rc::new(platform.clone()));
            for route in handle.registration().routes.clone() {
                // The relay route's id is the shared word "relay", not an identifier.
                if route.mode != ConnectionMode::Relay {
                    connections.push(route.id.clone());
                }
                if let Ok(Some(credential)) = stores.credential(environment, &route.id).await {
                    secrets.push(credential.token().expose().clone());
                }
                if route.mode == ConnectionMode::Ssh
                    && let Ok(Some(ConnectionProfile::Ssh { target, .. })) =
                        stores.profile(environment, &route.id).await
                {
                    ssh_targets.push(target);
                }
            }
            registry.remove(environment, Time::from_platform(&platform))?;
            registry.save(&platform).await?;
        }
        // The supervisor owned only the session; the SSH backend and its tunnel outlive it.
        // TypeScript logs and ignores a failed teardown, and so does this.
        for target in ssh_targets {
            let _ = platform.ssh_disconnect(target).await;
        }
        crate::persistence::purge_environment_copies(
            &platform,
            environment,
            &connections,
            &secrets,
        )
        .await
        .map_err(|_| ClientError::InvalidPayload("could not purge imported copies".into()))?;
        // A later pairing of the same server must not deliver what was queued for this one.
        SendService::<P, SessionEndpoint>::forget_saved(Rc::new(platform), environment).await
    }

    /// The supervisor ended on an error, not a disconnect: show it as blocked with that error
    /// until [`Self::reconnect`] starts a new one.
    fn stopped(&self, environment: &EnvironmentId, serial: u64, failure: ConnectionFailure) {
        let state = self
            .client(environment)
            .filter(|client| client.serial() == serial)
            .map(|client| client.state());
        if let Some(state) = state {
            self.inner.store.publish_connection(
                environment,
                serial,
                ConnectionState {
                    phase: Phase::Blocked,
                    last_failure: Some(failure),
                    retry_at: None,
                    ..state
                },
            );
        }
    }

    fn list(&self, id: EnvironmentId, label: String) {
        let mut environments = self.inner.environments.borrow_mut();
        if !environments.iter().any(|entry| entry.id == id) {
            environments.push(Environment {
                id,
                label,
                routes: Vec::new(),
                client: None,
                send: None,
                failure: None,
                tasks: Vec::new(),
                finished: None,
            });
        }
        drop(environments);
        self.notify();
    }

    fn fail(&self, id: &EnvironmentId, failure: ConnectionFailure) {
        if let Some(entry) = self
            .inner
            .environments
            .borrow_mut()
            .iter_mut()
            .find(|entry| &entry.id == id)
        {
            entry.failure = Some(failure);
        }
        self.notify();
    }

    pub(crate) fn spawn(&self, future: LocalBoxFuture<'static, ()>) -> AbortHandle {
        let (abort, registration) = AbortHandle::new_pair();
        (self.inner.spawn)(
            Abortable::new(future, registration)
                .map(|_| ())
                .boxed_local(),
        );
        abort
    }

    /// Installs a connected environment: its driver, its message delivery and the binding of
    /// delivery to each new session, and the leases of threads already open on it.
    fn attach(
        &self,
        client: ConnectedClient,
        driver: ConnectionTask,
        label: String,
        routes: Vec<Route>,
    ) {
        let id = client.environment().clone();
        let (done, finished) = futures::channel::oneshot::channel();
        let runtime = Rc::downgrade(&self.inner);
        let (environment, serial) = (id.clone(), client.serial());
        (self.inner.spawn)(
            async move {
                if let Err(error) = driver.await
                    && let Some(inner) = runtime.upgrade()
                {
                    ClientRuntime { inner }.stopped(&environment, serial, session_failure(&error));
                }
                let _ = done.send(());
            }
            .boxed_local(),
        );
        let send = SendService::new(
            id.clone(),
            Rc::new(self.inner.platform.clone()),
            self.inner.store.clone(),
            self.inner.source.clone(),
        );
        send.attach_web_drafts(self.composer_drafts());
        send.attach_background_spawner(self.inner.spawn.clone());
        let delivery = send.run();
        let mut message_changes = send.changes();
        let message_listeners = self.inner.message_listeners.clone();
        let tasks = vec![
            self.spawn(
                async move {
                    while message_changes.next().await.is_some() {
                        notify_all(&message_listeners);
                    }
                }
                .boxed_local(),
            ),
            self.spawn(
                async move {
                    let _ = delivery.await;
                }
                .boxed_local(),
            ),
            self.spawn(bind_sessions(
                self.inner.store.clone(),
                client.clone(),
                send.clone(),
            )),
        ];
        let replaced = {
            let mut environments = self.inner.environments.borrow_mut();
            let entry = Environment {
                id: id.clone(),
                label,
                routes,
                client: Some(client.clone()),
                send: Some(send),
                failure: None,
                tasks,
                finished: Some(finished),
            };
            match environments.iter_mut().find(|entry| entry.id == id) {
                Some(existing) => Some(std::mem::replace(existing, entry)),
                None => {
                    environments.push(entry);
                    None
                }
            }
        };
        if let Some(replaced) = replaced {
            drop(shut_down(replaced));
        }
        for (key, (_, lease)) in self.inner.threads.borrow_mut().iter_mut() {
            if key.environment == id {
                *lease = Some(client.open_thread(key.thread.clone()));
            }
        }
        self.notify();
        self.notify_messages();
    }
}

fn notify_all(listeners: &RefCell<Vec<mpsc::UnboundedSender<()>>>) {
    listeners
        .borrow_mut()
        .retain(|listener| listener.unbounded_send(()).is_ok());
}

/// Binds message delivery to each new session of `client`, and unbinds it while there is none.
/// Uploads go to that session's own HTTP address, whatever the route (TypeScript resolves them
/// against the prepared connection at transfer time).
fn bind_sessions<P: Platform + 'static>(
    store: ClientStore,
    client: ConnectedClient,
    send: SendService<P, SessionEndpoint>,
) -> LocalBoxFuture<'static, ()> {
    // Holds the client until the composition root aborts this task (removal, replacement, drop).
    async move {
        let mut changes = store.subscribe_environment(client.environment());
        let mut bound: Option<Generation> = None;
        while changes.next().await.is_some() {
            let environment = client.environment().clone();
            let current = store
                .snapshot()
                .environments
                .get(&environment)
                .and_then(|state| Some((state.session, state.config.clone()?, client.session()?)));
            match current {
                Some((generation, config, endpoint)) if bound != Some(generation) => {
                    let connected = match client.http_base_url().filter(|base| !base.is_empty()) {
                        Some(base) => send.connect_with_upload_base(
                            generation,
                            Rc::new(endpoint),
                            config,
                            &base,
                        ),
                        None => send.connect(generation, Rc::new(endpoint), config),
                    };
                    if connected.is_ok() {
                        bound = Some(generation);
                    }
                }
                None if bound.is_some() => {
                    send.disconnect();
                    bound = None;
                }
                _ => {}
            }
        }
    }
    .boxed_local()
}

pub use crate::services::command_output::CommandOutputLease;

struct QueryShared<T> {
    resource: Rc<Resource<T>>,
    driver: AbortHandle,
}

impl<T> Drop for QueryShared<T> {
    fn drop(&mut self) {
        self.driver.abort();
        self.resource.close();
    }
}

/// Keeps a query's driver running while held; read the value through [`QueryLease::resource`].
#[must_use = "the query runs only while its lease is held"]
pub struct QueryLease<T: 'static> {
    shared: Rc<QueryShared<T>>,
}

impl<T: 'static> Clone for QueryLease<T> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}

impl<T: 'static> QueryLease<T> {
    pub fn resource(&self) -> &Rc<Resource<T>> {
        &self.shared.resource
    }
}
