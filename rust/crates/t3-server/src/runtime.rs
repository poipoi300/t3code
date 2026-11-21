//! Composition root: startup barriers and ordered shutdown of one environment's server.
use crate::{
    adapters::config::{LocalServices, resolve},
    services::{DomainContext, Domains, Phase, ServiceRegistry},
};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

pub struct RunningServer {
    /// The bound address; with `--port 0` this carries the port the OS chose.
    pub address: SocketAddr,
    pub pairing_credential: String,
    /// RPC methods this server serves (registered, not `NotImplemented`), including Rust-only extensions.
    pub rpc_methods_served: usize,
    /// The contract methods this server answers with Effect's unknown-request-tag defect.
    pub unserved_methods: Vec<&'static str>,
    shutdown: CancellationToken,
    phase: watch::Receiver<Phase>,
    task: Option<tokio::task::JoinHandle<crate::Result<()>>>,
}
impl RunningServer {
    /// Cancelling it stops the server as [`Self::cancel`] does, from another thread.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }
    pub fn cancel(&self) {
        self.shutdown.cancel();
    }
    pub fn phase(&self) -> Phase {
        *self.phase.borrow()
    }
    /// Resolves once HTTP is served and commands are admitted; false if the server stopped first.
    pub async fn wait_ready(&self) -> bool {
        let mut phase = self.phase.clone();
        phase
            .wait_for(|p| matches!(p, Phase::Ready | Phase::Stopping))
            .await
            .is_ok_and(|p| *p == Phase::Ready)
    }
    /// Requests shutdown and waits until every connection is joined, every domain is shut
    /// down, the database is closed and ownership is released.
    pub async fn shutdown(mut self) -> crate::Result<()> {
        self.cancel();
        self.take_task()?.await?
    }
    pub async fn wait(mut self) -> crate::Result<()> {
        self.take_task()?.await?
    }
    /// Serves until a termination signal or an internal stop (such as lost ownership), then
    /// completes the same ordered shutdown as [`RunningServer::shutdown`].
    pub async fn run_until_signal(mut self) -> crate::Result<()> {
        let task = self.take_task()?;
        let signals = match crate::shutdown::cancel_on_signal(self.shutdown.clone()) {
            Ok(signals) => signals,
            Err(error) => {
                self.cancel();
                task.await??;
                return Err(error.into());
            }
        };
        let result = task.await;
        signals.abort();
        result?
    }
    fn take_task(&mut self) -> crate::Result<tokio::task::JoinHandle<crate::Result<()>>> {
        self.task
            .take()
            .ok_or_else(|| crate::Error::Service("server already awaited".into()))
    }
}
impl Drop for RunningServer {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// Starts the server and returns once it is ready. Barriers, in order: the listener is bound
/// first so the actual port is known everywhere; ownership is acquired before the database is
/// migrated (inside `LocalServices::open`); domain services finish recovery; HTTP listens;
/// then `activate` publishes `welcome`, activates domains and the runtime file, opens the
/// command gate and publishes `ready`.
/// What a host injects into the composition root beyond configuration.
#[derive(Default, Clone)]
pub struct StartOptions {
    /// Checkout coordination supplied by an embedder. Without it, and without `native_cleanup`,
    /// no cleanup worker or cleanup RPC starts.
    #[cfg(feature = "storage-cleanup")]
    pub cleanup_worktrees: Option<Arc<dyn crate::adapters::storage_cleanup::CleanupWorktrees>>,
    /// Opt in to storage cleanup with the native Git worker fence and fresh forge lookup. Off by
    /// default: the default binary never removes a checkout.
    #[cfg(all(feature = "storage-cleanup", feature = "forge"))]
    pub native_cleanup: bool,
    /// Admission shared by every host writer in this environment.
    pub writer_gate: Option<Arc<crate::writers::FolderGate>>,
    #[cfg(feature = "providers")]
    pub provider_effects: Option<Arc<crate::adapters::providers::ProviderEffects>>,
    /// Provider adapters for the orchestration runtime's registry. The binary passes none until
    /// provider registration (Replay included) is assembled; tests pass scripted stand-ins.
    #[cfg(feature = "orchestration")]
    pub providers: Vec<std::sync::Arc<dyn crate::adapters::orchestration::ProviderAdapter>>,
    /// Register the free Replay (dev) provider, as TypeScript's dev server does when run from
    /// source. The binary has no dev mode yet and leaves it off.
    #[cfg(feature = "orchestration")]
    pub dev_replay: bool,
    /// Project bindings for the pull-request application; `None` binds through the
    /// environment's forge registry. Tests pass a fake forge here.
    #[cfg(feature = "pull-requests")]
    pub pull_request_binder: Option<Arc<dyn crate::adapters::pull_requests::ProjectBinder>>,
    /// The per-thread MCP credential store; `None` creates one. Tests keep a clone to read the
    /// credential a provider session was opened with.
    #[cfg(feature = "mcp")]
    pub mcp_sessions: Option<Arc<crate::adapters::mcp::McpSessions>>,
    /// Provider discovery and availability supplied by a trusted MCP embedding or test fixture.
    #[cfg(feature = "mcp")]
    pub mcp_delegation_host: Option<Arc<dyn t3_mcp::adapters::delegation::DelegationHost>>,
    /// Test seam: the review-guide generator reports each finished cleanup's run id here.
    #[cfg(feature = "textgen-native")]
    pub review_guide_cleanups: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    /// The scheduled-task clock; `None` is the system clock. Tests drive due tasks with it.
    #[cfg(feature = "scheduler")]
    pub scheduler_clock: Option<Arc<dyn crate::adapters::scheduler::Clock>>,
    /// The editor process edge and a slot for the started adapter (`adapters/host_leftovers.rs`).
    #[cfg(feature = "host")]
    pub host_leftovers: crate::adapters::host_leftovers::HostLeftoversOptions,
    /// The upload ledger's clock (`adapters/attachments.rs`).
    pub attachments: crate::adapters::attachments::AttachmentOptions,
}

pub async fn start(inputs: crate::adapters::config::ConfigInputs) -> crate::Result<RunningServer> {
    start_with(inputs, StartOptions::default()).await
}

pub async fn start_with(
    inputs: crate::adapters::config::ConfigInputs,
    options: StartOptions,
) -> crate::Result<RunningServer> {
    start_configured(inputs, options, false).await
}

/// Starts the standalone binary's process-wide tracing subscriber after acquiring ownership.
pub async fn start_with_global_tracing(
    inputs: crate::adapters::config::ConfigInputs,
) -> crate::Result<RunningServer> {
    start_configured(inputs, StartOptions::default(), true).await
}

/// One line in the format of the server's log output, for lines logged before it exists.
fn print_migration_log(message: &str, fields: &[(&str, String)]) {
    use std::io::Write;
    let mut line = serde_json::Map::new();
    line.insert("message".into(), message.into());
    for (key, value) in fields {
        line.insert((*key).into(), value.as_str().into());
    }
    // A closed stderr must not panic the migration.
    let _ = writeln!(
        std::io::stderr(),
        "{} INFO {}",
        utc_timestamp(std::time::SystemTime::now()),
        serde_json::Value::Object(line)
    );
}

/// `2026-11-21T22:21:16.813Z`, from days since the epoch (Hinnant's civil-from-days).
fn utc_timestamp(now: std::time::SystemTime) -> String {
    let since = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let (days, seconds) = (since.as_secs() / 86_400, since.as_secs() % 86_400);
    let shifted = i64::try_from(days).unwrap_or(0) + 719_468;
    let era = shifted.div_euclid(146_097);
    let of_era = shifted.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1_460 + of_era / 36_524 - of_era / 146_096) / 365;
    let day_of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        seconds / 3_600,
        seconds % 3_600 / 60,
        seconds % 60,
        since.subsec_millis()
    )
}

async fn start_configured(
    mut inputs: crate::adapters::config::ConfigInputs,
    options: StartOptions,
    install_global_tracing: bool,
) -> crate::Result<RunningServer> {
    crate::cli::hydrate_bootstrap(&mut inputs)?;
    let mut config = resolve(&inputs)?;
    if config.private_network_only || config.tailscale_serve_enabled {
        return Err(crate::Error::Service(
            "LAN fencing and Tailscale are not yet ported".into(),
        ));
    }
    // Before binding (TypeScript `checkServerReachability`): loopback when no host is given, and
    // never a host the organisation's policy forbids, not even briefly.
    let policy = crate::adapters::config::managed_policy(&inputs.env)?;
    if let Some(document) = &policy {
        let unenforced = crate::adapters::config::unenforced_policy(document);
        if !unenforced.is_empty() {
            return Err(crate::Error::Service(format!(
                "{} manages this machine, and this server cannot enforce its policy yet ({}).",
                document.organization.0,
                unenforced.join(", ")
            )));
        }
    }
    let host = crate::adapters::config::reachable_host(policy.as_ref(), &config)?;
    // Keep the listener used for automatic selection: probing then rebinding races other servers.
    let listener = match tokio::net::TcpListener::bind((host.as_str(), config.port)).await {
        Ok(listener) => listener,
        Err(e) if config.needs_automatic_port && e.kind() == std::io::ErrorKind::AddrInUse => {
            config.port = 0;
            tokio::net::TcpListener::bind((host.as_str(), 0)).await?
        }
        Err(e) => return Err(e.into()),
    };
    let address = listener.local_addr()?;
    let (account_home, environment) = (inputs.home.clone(), inputs.env.clone());
    if install_global_tracing {
        // The tracing subscriber is installed after the database opens, so the migration lines
        // the desktop waits on would be lost: print them as the subscriber does.
        t3_persistence::migrations::set_log_sink(print_migration_log);
    }
    let local = LocalServices::open(config, address.port(), policy).await?;
    // Dev mode is `--dev-url` / `VITE_DEV_SERVER_URL`, as in TypeScript; it keeps the free
    // Replay (dev) provider (`seedDevReplayInstance` in ProviderInstanceRegistryHydration.ts).
    #[cfg(feature = "orchestration")]
    let options = {
        let mut options = options;
        options.dev_replay |= local.config.dev_url.is_some();
        if options.dev_replay {
            seed_dev_replay_instance(&local).await;
        }
        options
    };
    let shutdown = CancellationToken::new();
    let mut domains = Domains::default();
    let prepared = async {
        let pairing_credential = local.auth.startup_pairing_credential().await?;
        start_domains(
            &local,
            &shutdown,
            &mut domains,
            account_home,
            environment,
            &options,
            install_global_tracing,
        )
        .await?;
        let handlers = crate::handlers::registry_with(&domains)?;
        crate::Result::Ok((pairing_credential, handlers))
    }
    .await;
    let (pairing_credential, handlers) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            shutdown.cancel();
            let mut errors = vec![error.to_string()];
            errors.extend(domains.shutdown().await);
            drop(domains);
            if let Err(close) = local.shutdown().await {
                errors.push(format!("local services: {close}"));
            }
            return Err(crate::Error::Service(format!(
                "startup failed: {}",
                errors.join("; ")
            )));
        }
    };
    debug_assert_eq!(
        local.environment.capabilities.linked_thread_refreshes,
        handlers
            .disposition("pullRequests.subscribeLinkedThreadRefreshes")
            .filter(|served| *served)
    );
    #[cfg(feature = "storage-cleanup")]
    let local = {
        let mut local = local;
        local.environment.capabilities.storage_cleanup = handlers
            .disposition("server.getStorageCleanupReport")
            .filter(|served| *served);
        local.environment.capabilities.storage_cleanup_run = handlers
            .disposition("server.runStorageCleanup")
            .filter(|served| *served);
        local
    };
    let unserved_methods: Vec<&'static str> = t3_contracts::rpc::WS_METHODS
        .iter()
        .filter(|method| handlers.disposition(method.tag) != Some(true))
        .map(|method| method.tag)
        .collect();
    let rpc_methods_served = t3_contracts::rpc::WS_METHODS.len() - unserved_methods.len();
    tracing::info!(
        served = rpc_methods_served,
        total = t3_contracts::rpc::WS_METHODS.len(),
        "RPC methods served"
    );
    let services = Arc::new(ServiceRegistry {
        handlers,
        local,
        shutdown: shutdown.clone(),
        connections: Default::default(),
        lifecycle: Default::default(),
        domains,
    });
    follow_peer_links_setting(&services).await;
    let phase = services.lifecycle.subscribe();
    // The listener is bound, so the OS queues connections from here; the task accepts them.
    services.lifecycle.set_listening(true);
    let task = tokio::spawn(serve(services.clone(), listener));
    let activated = activate(&services).await;
    drop(services);
    if let Err(error) = activated {
        shutdown.cancel();
        let stopped = task.await;
        let cleanup = match stopped {
            Ok(Ok(())) => String::new(),
            Ok(Err(error)) => format!("; {error}"),
            Err(error) => format!("; {error}"),
        };
        return Err(crate::Error::Service(format!(
            "startup failed: {error}{cleanup}"
        )));
    }
    Ok(RunningServer {
        address,
        pairing_credential,
        rpc_methods_served,
        unserved_methods,
        shutdown,
        phase,
        task: Some(task),
    })
}

/// TypeScript's tail of `serverRuntimeStartup`: once recovery has finished and HTTP listens,
/// publish `welcome`, activate (domain schedulers, the runtime file), open the command gate,
/// then publish `ready`.
async fn activate(services: &ServiceRegistry) -> crate::Result<()> {
    #[cfg(feature = "project")]
    let bootstrap = match services.domain::<crate::adapters::project::ProjectAdapter>() {
        Some(projects) if services.local.config.auto_bootstrap_project_from_cwd => {
            projects.auto_bootstrap(&services.local.config.cwd).await?
        }
        _ => Default::default(),
    };
    #[cfg(feature = "project")]
    services.local.publish_welcome(
        bootstrap.project_id.as_ref().map(|id| id.0.as_str()),
        bootstrap.thread_id.as_ref().map(|id| id.0.as_str()),
    )?;
    #[cfg(not(feature = "project"))]
    services.local.publish_welcome(None, None)?;
    services.domains.activate().await?;
    if let Err(error) = services.local.publish_runtime_state().await {
        // As in TypeScript, a server that cannot write its runtime file still serves.
        tracing::warn!(%error, "failed to persist server runtime state");
    }
    services.lifecycle.advance(Phase::CommandReady);
    services.local.publish_ready()?;
    services.lifecycle.advance(Phase::Ready);
    tracing::info_span!("server.ready").in_scope(|| tracing::info!("server ready"));
    Ok(())
}

/// Settings → Labs "linked environments" gates peer-link client registration
/// (`McpOAuth.ts` reads `linkedEnvironmentsEnabled`). Subscribed before the first read so no
/// change is missed; the follower is a tracked task joined at shutdown.
async fn follow_peer_links_setting(services: &Arc<ServiceRegistry>) {
    let local = &services.local;
    let mut changes = local.settings.subscribe_changes();
    match local.settings.snapshot().await {
        Ok(settings) => local
            .auth
            .set_peer_links_enabled(settings.linked_environments_enabled),
        Err(error) => tracing::warn!(%error, "cannot read settings; peer links stay disabled"),
    }
    let follower = services.clone();
    services.connections.spawn(async move {
        loop {
            tokio::select! {
                () = follower.shutdown.cancelled() => break,
                changed = changes.recv() => match changed {
                    Some(settings) => follower
                        .local
                        .auth
                        .set_peer_links_enabled(settings.linked_environments_enabled),
                    None => break,
                },
            }
        }
    });
}

/// Domain services in TypeScript's dependency order (`server.ts`): git and terminal under the
/// stores, usage beside providers, orchestration, then the scheduler that dispatches into it.
/// Each `start` returns after its own recovery; shutdown runs in reverse.
/// Adds the `devReplay` instance to settings unless one exists, so a user's edits (such as
/// turning it off) survive restarts. TypeScript also writes its agent script path; the Rust
/// Replay provider runs in-process and needs none.
#[cfg(feature = "orchestration")]
async fn seed_dev_replay_instance(local: &LocalServices) {
    const INSTANCE: &str = "devReplay";
    let existing = match local.settings.snapshot().await {
        Ok(settings) => serde_json::to_value(&settings)
            .ok()
            .and_then(|value| {
                value
                    .pointer(&format!("/providerInstances/{INSTANCE}"))
                    .cloned()
            })
            .is_some_and(|entry| !entry.is_null()),
        Err(error) => {
            tracing::warn!(%error, "cannot read settings; Replay (dev) is not seeded");
            return;
        }
    };
    if existing {
        return;
    }
    let mutation = serde_json::from_value(serde_json::json!({
        "operation": "upsert",
        "instanceId": INSTANCE,
        "instance": {"driver": "acpRegistry", "displayName": "Replay (dev)"},
    }));
    let seeded = match mutation {
        Ok(mutation) => local
            .settings
            .update_provider_instance(&mutation, &Default::default())
            .await
            .map(|_| ())
            .map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    };
    if let Err(error) = seeded {
        tracing::warn!(%error, "cannot seed the Replay (dev) provider instance");
    }
}

#[cfg(all(feature = "orchestration", feature = "terminal"))]
fn terminal_writers(
    domains: &Domains,
) -> crate::Result<Arc<dyn t3_orchestration::api::ThreadWriters>> {
    domains
        .get::<crate::adapters::terminal::TerminalAdapter>()
        .map(|terminal| {
            Arc::new(crate::adapters::terminal::TerminalWriters(terminal))
                as Arc<dyn t3_orchestration::api::ThreadWriters>
        })
        .ok_or_else(|| crate::Error::Service("Terminal writer admission is missing.".into()))
}

#[cfg(all(test, feature = "orchestration", feature = "terminal"))]
#[test]
fn startup_requires_terminal_writer_admission_when_terminals_are_enabled() {
    assert!(terminal_writers(&Domains::default()).is_err());
}

async fn start_domains(
    local: &LocalServices,
    shutdown: &CancellationToken,
    domains: &mut Domains,
    account_home: std::path::PathBuf,
    environment: std::collections::BTreeMap<String, String>,
    options: &StartOptions,
    install_global_tracing: bool,
) -> crate::Result<()> {
    let context = DomainContext {
        local,
        database: local.database().clone(),
        shutdown: shutdown.clone(),
        account_home,
        environment,
    };
    domains.insert_runtime(
        crate::adapters::telemetry::TelemetryAdapter::start(&context, install_global_tracing)
            .await?,
    )?;
    #[cfg(feature = "host")]
    domains.insert_runtime(crate::adapters::host::HostAdapter::start(&context).await?)?;
    // Terminals, setup scripts, file writes and Git actions wait while a restore holds their
    // folder (t3-git H-2).
    let writer_gate = options.writer_gate.clone().unwrap_or_default();
    domains.insert(writer_gate.clone())?;
    #[cfg(feature = "workspace")]
    let workspaces = crate::workspaces::EnvironmentWorkspaces::new(&context);
    #[cfg(any(feature = "providers", feature = "forge"))]
    domains.insert(workspaces.clone())?;
    // After the host: the VCS status policy reads its background-activity leases.
    #[cfg(all(feature = "git", feature = "workspace"))]
    {
        let git = crate::adapters::git::GitAdapter::start_with_authority(
            &context,
            workspaces.clone(),
            domains,
        )
        .await;
        // The workspace authority lists worktrees through the same Git root.
        workspaces.use_git(git.at(&context.local.config.cwd));
        domains.insert_runtime(git)?;
    }
    #[cfg(all(feature = "git", not(feature = "workspace")))]
    {
        let git = crate::adapters::git::GitAdapter::start(&context, domains).await;
        domains.insert_runtime(git)?;
    }
    #[cfg(feature = "cloud")]
    domains.insert_runtime(crate::adapters::cloud::CloudAdapter::start(&context).await?)?;
    // Device hosts resolve a thread's workspace through the same authority.
    #[cfg(feature = "device")]
    let device_workspaces = workspaces.clone();
    // Follows deletions and settings once orchestration has started (below).
    #[cfg(all(feature = "workspace", feature = "orchestration"))]
    let authority = workspaces.clone();
    #[cfg(feature = "workspace")]
    domains.insert(crate::workspaces::workspace_adapter(&context, workspaces).await?)?;
    // Pending uploads, their issuers and the runtime's claim port (`adapters/attachments.rs`).
    #[cfg(all(feature = "workspace", feature = "orchestration"))]
    let attachments = match domains.get::<crate::adapters::workspace::WorkspaceAdapter>() {
        Some(workspace) => {
            let attachments = crate::adapters::attachments::AttachmentsAdapter::new(
                workspace.attachments.clone(),
                options.attachments.clone(),
            );
            domains.insert(attachments.clone())?;
            Some(attachments)
        }
        None => None,
    };
    #[cfg(feature = "terminal")]
    {
        domains.insert_runtime(crate::adapters::terminal::TerminalAdapter::start(
            &context,
            writer_gate.clone(),
        )?)?;
        domains.insert(crate::workspaces::terminal_transport())?;
    }
    #[cfg(feature = "preview")]
    {
        let preview = crate::adapters::preview::PreviewAdapter::start(&context).await?;
        #[cfg(feature = "terminal")]
        if let Some(terminal) = domains.get::<crate::adapters::terminal::TerminalAdapter>() {
            preview.observe_terminals(terminal);
        }
        domains.insert_runtime(preview)?;
    }
    // Before orchestration: provider sessions receive their MCP credential from it.
    #[cfg(feature = "mcp")]
    let mcp_sessions = {
        let sessions = options.mcp_sessions.clone().unwrap_or_default();
        sessions.bind(&context)?;
        domains.insert(sessions.clone())?;
        Some(sessions.port())
    };
    #[cfg(all(feature = "orchestration", not(feature = "mcp")))]
    let mcp_sessions = None;
    // Before orchestration: it supplies the title port; providers are bound once they start.
    #[cfg(feature = "textgen")]
    let titles = {
        let textgen = crate::adapters::textgen::TextGenAdapter::start(&context);
        domains.insert(textgen.clone())?;
        Some(textgen.title_generator())
    };
    #[cfg(all(feature = "orchestration", not(feature = "textgen")))]
    let titles = None;
    // Before orchestration, which composes it; projects and setup scripts are bound below.
    #[cfg(all(feature = "git-actions", feature = "local-services"))]
    let launch = match (
        domains.get::<crate::adapters::git::GitAdapter>(),
        domains.get::<crate::adapters::textgen::TextGenAdapter>(),
        domains.get::<crate::adapters::terminal::TerminalAdapter>(),
    ) {
        (Some(git), Some(textgen), Some(terminal)) => {
            crate::adapters::launch::LaunchAdapter::start(&context, git, textgen, terminal)
        }
        _ => None,
    };
    #[cfg(all(feature = "git-actions", feature = "local-services"))]
    let launch_ports = launch.as_ref().map(|launch| launch.ports());
    #[cfg(all(feature = "git-actions", feature = "local-services"))]
    if let Some(launch) = &launch {
        domains.insert(launch.clone())?;
    }
    #[cfg(all(
        feature = "orchestration",
        not(all(feature = "git-actions", feature = "local-services"))
    ))]
    let launch_ports = None;
    // A file restore holds the thread's terminals, setup scripts included (t3-git H-2).
    #[cfg(all(feature = "orchestration", feature = "terminal"))]
    let writers = terminal_writers(domains)?;
    #[cfg(all(feature = "orchestration", not(feature = "terminal")))]
    let writers = Arc::new(crate::writers::RpcWriters(writer_gate.clone()))
        as Arc<dyn t3_orchestration::api::ThreadWriters>;
    #[cfg(all(feature = "orchestration", feature = "workspace"))]
    let managed_worktrees = authority.managed_directories().await;
    #[cfg(all(feature = "orchestration", not(feature = "workspace")))]
    let managed_worktrees = vec![context.local.config.paths.worktrees()];
    #[cfg(all(feature = "workspace", feature = "orchestration"))]
    let attachment_port = attachments
        .clone()
        .map(|a| a as Arc<dyn t3_orchestration::api::AttachmentBackend>);
    #[cfg(all(feature = "orchestration", not(feature = "workspace")))]
    let attachment_port = None;
    #[cfg(all(feature = "workspace", feature = "orchestration"))]
    let acp_attachments = domains
        .get::<crate::adapters::workspace::WorkspaceAdapter>()
        .map(|workspace| {
            t3_orchestration::adapters::acp::AcpAttachmentSource::new(
                workspace.attachments.base.clone(),
                workspace.attachments.delivery.clone(),
            )
        });
    #[cfg(all(feature = "orchestration", not(feature = "workspace")))]
    let acp_attachments = None;
    #[cfg(feature = "orchestration")]
    domains.insert_runtime(
        crate::adapters::orchestration::OrchestrationAdapter::start(
            &context,
            options.providers.clone(),
            options.dev_replay,
            mcp_sessions,
            titles,
            launch_ports,
            crate::adapters::orchestration::RestorePorts {
                writers,
                managed_worktrees,
                #[cfg(feature = "workspace")]
                authority: authority.clone(),
            },
            attachment_port,
            acp_attachments.clone(),
        )
        .await?,
    )?;
    #[cfg(all(feature = "workspace", feature = "orchestration", feature = "terminal"))]
    if let (Some(terminal), Some(runtime)) = (
        domains.get::<crate::adapters::terminal::TerminalAdapter>(),
        domains
            .get::<crate::adapters::orchestration::OrchestrationAdapter>()
            .and_then(|adapter| adapter.composed().cloned()),
    ) {
        terminal.bind_worktree_recovery(&runtime);
    }
    #[cfg(all(feature = "workspace", feature = "orchestration"))]
    if let (Some(attachments), Some(runtime)) = (
        &attachments,
        domains
            .get::<crate::adapters::orchestration::OrchestrationAdapter>()
            .and_then(|orchestration| orchestration.composed().cloned()),
    ) {
        attachments.bind_projections(runtime.projections.clone());
    }
    #[cfg(all(feature = "workspace", feature = "orchestration"))]
    if let Some(events) = domains
        .get::<crate::adapters::orchestration::OrchestrationAdapter>()
        .and_then(|orchestration| orchestration.subscribe_events())
    {
        authority.follow(events, context.shutdown.clone());
    }
    // VCS status reads apply each folder's saved repository binding (t3-git re-review 4).
    #[cfg(all(feature = "git", feature = "orchestration"))]
    if let (Some(git), Some(orchestration)) = (
        domains.get::<crate::adapters::git::GitAdapter>(),
        domains.get::<crate::adapters::orchestration::OrchestrationAdapter>(),
    ) && let Some(vcs) = git.vcs()
    {
        vcs.use_saved_bindings(orchestration);
    }
    #[cfg(feature = "providers")]
    {
        let providers = crate::adapters::providers::ProvidersAdapter::start(
            &context,
            domains,
            options.provider_effects.clone(),
        )
        .await?;
        domains.insert(providers.domain.clone())?;
        domains.insert(providers.domain.secrets.clone())?;
        #[cfg(feature = "textgen")]
        if let Some(textgen) = domains.get::<crate::adapters::textgen::TextGenAdapter>() {
            textgen.bind_providers(providers.domain.clone());
        }
        domains.insert_runtime(providers)?;
    }
    // After the providers: the price table is fetched over their shared HTTP transport.
    #[cfg(feature = "usage")]
    domains.insert(crate::adapters::usage::UsageAdapter::start_for(&context, domains).await?)?;
    // The orchestration runtime offers each thread the modes its provider instance lists.
    #[cfg(all(feature = "providers", feature = "orchestration"))]
    if let (Some(orchestration), Some(providers)) = (
        domains.get::<crate::adapters::orchestration::OrchestrationAdapter>(),
        domains.get::<t3_provider::domain::ProviderDomain>(),
    ) {
        orchestration
            .follow_providers(providers.providers.clone(), context.shutdown.clone())
            .await;
        if let Some(source) = orchestration.provider_source().cloned() {
            // Each native session reads its thread's T3 MCP server from here.
            #[cfg(feature = "mcp")]
            let t3_mcp = domains
                .get::<crate::adapters::mcp::McpSessions>()
                .map(|sessions| sessions as Arc<dyn t3_orchestration::provider::T3McpSessions>);
            #[cfg(not(feature = "mcp"))]
            let t3_mcp = None;
            // Each native session holds preview port discovery's running-agent lease.
            #[cfg(feature = "preview")]
            let track_agent = domains
                .get::<crate::adapters::preview::PreviewAdapter>()
                .map(|preview| {
                    let discovery = preview.discovery.clone();
                    Arc::new(move || {
                        let lease = discovery.track_agent();
                        Box::new(move || drop(lease)) as Box<dyn FnOnce() + Send>
                    }) as t3_orchestration::adapters::codex::CodexAgentTracker
                });
            #[cfg(not(feature = "preview"))]
            let track_agent = None;
            source.bind(crate::adapters::native_providers::NativeProviders::new(
                providers.clone(),
                &context,
                Arc::new(crate::adapters::orchestration::SystemClock),
                Arc::new(t3_orchestration::composition::RandomIds),
                t3_mcp,
                track_agent,
                acp_attachments,
            ));
            // Shuts down before the providers: sessions stop while the domain is alive.
            domains.insert_runtime(Arc::new(
                crate::adapters::native_providers::NativeProvidersShutdown {
                    orchestration: orchestration.clone(),
                    source,
                },
            ))?;
        }
    }
    // After the providers: devices read secret references through the same resolver.
    #[cfg(feature = "device")]
    domains.insert_runtime(
        crate::adapters::device::DeviceAdapter::start(
            &context,
            device_workspaces,
            domains.get::<t3_provider::secrets::SecretResolver>(),
        )
        .await?,
    )?;
    // The one forge registry; source control, project clones and pull requests derive from it.
    #[cfg(feature = "forge")]
    if let Some(forge) = crate::adapters::forge::ForgeAdapter::start(&context, domains).await? {
        #[cfg(feature = "textgen-native")]
        if let Some(textgen) = domains.get::<crate::adapters::textgen::TextGenAdapter>() {
            textgen.bind_forge(forge.registry());
        }
        domains.insert_runtime(forge)?;
    }
    // Before projects: promoting a scratch thread moves its scheduled tasks.
    #[cfg(feature = "scheduler")]
    let scheduler = crate::adapters::scheduler::SchedulerAdapter::start_for(
        &context,
        domains,
        options.scheduler_clock.clone(),
    )
    .await?;
    #[cfg(feature = "scheduler")]
    domains.insert_runtime(scheduler.clone())?;
    #[cfg(feature = "project")]
    if let Some(orchestration) =
        domains.get::<crate::adapters::orchestration::OrchestrationAdapter>()
    {
        #[cfg(feature = "scheduler")]
        let scheduled_tasks: Option<Arc<dyn t3_project::promotion::MoveThreadTasks>> =
            Some(scheduler.service());
        #[cfg(not(feature = "scheduler"))]
        let scheduled_tasks = None;
        #[cfg(feature = "forge")]
        let clones = domains
            .get::<crate::adapters::forge::ForgeAdapter>()
            .map(|forge| forge.clone_backend());
        #[cfg(not(feature = "forge"))]
        let clones = None;
        domains.insert_runtime(crate::adapters::project::ProjectAdapter::start(
            &context,
            &orchestration,
            scheduled_tasks,
            clones,
        )?)?;
    }
    // After projects and the forge registry, whose project list and drivers it binds.
    #[cfg(feature = "pull-requests")]
    if let (Some(projects), Some(binder)) = (
        domains.get::<crate::adapters::project::ProjectAdapter>(),
        crate::adapters::pull_requests::binder(domains, options),
    ) {
        // The review-guide generator: text generation and the Git root; bound to the service below.
        #[cfg(feature = "textgen-native")]
        let guides = domains
            .get::<crate::adapters::textgen::TextGenAdapter>()
            .zip(domains.get::<crate::adapters::git::GitAdapter>())
            .map(|(textgen, git)| {
                crate::adapters::review_guide::ReviewGuideGenerator::new(
                    &context,
                    textgen,
                    git,
                    projects.service().clone(),
                )
            });
        #[cfg(feature = "textgen-native")]
        let generator = guides
            .clone()
            .map(|guides| guides as Arc<dyn t3_pull_request::guide::GuideGenerator>);
        #[cfg(not(feature = "textgen-native"))]
        let generator = None;
        let adapter = crate::adapters::pull_requests::PullRequestAdapter::start(
            &context, &projects, binder, generator,
        )
        .await?;
        let events = domains
            .get::<crate::adapters::orchestration::OrchestrationAdapter>()
            .and_then(|orchestration| orchestration.subscribe_events())
            .ok_or_else(|| {
                crate::Error::Service(
                    "Pull request subscriptions require the orchestration event stream.".into(),
                )
            })?;
        let thread_port = domains.get::<t3_orchestration::ports::thread_project::thread_pull_request_service::ThreadPullRequestService>();
        adapter.follow_linked_threads(thread_port.as_deref(), events);
        #[cfg(feature = "textgen-native")]
        if let Some(guides) = guides {
            if let Some(sender) = &options.review_guide_cleanups {
                guides.report_cleanups(sender.clone());
            }
            guides.bind(adapter.service.clone());
            domains.insert(guides)?;
        }
        domains.insert_runtime(adapter)?;
    }
    // After pull requests (pushed refreshes), projects, terminals and the forge registry.
    #[cfg(feature = "git-actions")]
    if let (Some(projects), Some(orchestration), Some(terminal)) = (
        domains.get::<crate::adapters::project::ProjectAdapter>(),
        domains.get::<crate::adapters::orchestration::OrchestrationAdapter>(),
        domains.get::<crate::adapters::terminal::TerminalAdapter>(),
    ) && let Some(setup) = crate::adapters::project_setup::ProjectSetupAdapter::start(
        &context,
        &projects,
        &orchestration,
        &terminal,
    ) {
        domains.insert(setup)?;
        if let Some(actions) =
            crate::adapters::git_actions::GitActionsAdapter::start(&context, domains)
        {
            domains.insert(actions)?;
        }
    }
    #[cfg(all(feature = "git-actions", feature = "local-services"))]
    if let (Some(launch), Some(projects), Some(orchestration)) = (
        launch,
        domains.get::<crate::adapters::project::ProjectAdapter>(),
        domains.get::<crate::adapters::orchestration::OrchestrationAdapter>(),
    ) {
        launch.bind(
            projects,
            domains.get::<crate::adapters::project_setup::ProjectSetupAdapter>(),
            orchestration,
        );
    }
    // After providers (secret resolver), cloud, orchestration and the workspace authority.
    #[cfg(feature = "host")]
    domains.insert_runtime(
        crate::adapters::host_leftovers::HostLeftoversAdapter::start(
            &context,
            domains,
            &options.host_leftovers,
        )
        .await?,
    )?;
    // Last: it binds the tools of every domain above.
    #[cfg(feature = "mcp")]
    if let Some(mcp) = crate::adapters::mcp::McpAdapter::start(
        &context,
        domains,
        options.mcp_delegation_host.clone(),
    )
    .await?
    {
        domains.insert_runtime(mcp)?;
    }
    #[cfg(feature = "storage-cleanup")]
    #[cfg_attr(not(feature = "forge"), allow(unused_mut))]
    let mut worktrees = options.cleanup_worktrees.clone();
    #[cfg(all(feature = "storage-cleanup", feature = "forge"))]
    if worktrees.is_none() && options.native_cleanup {
        worktrees = crate::adapters::storage_cleanup::NativeWorktrees::from_domains(domains);
    }
    #[cfg(feature = "storage-cleanup")]
    if let Some(worktrees) = worktrees {
        let cleanup =
            crate::adapters::storage_cleanup::CleanupAdapter::new(&context, domains, worktrees)?;
        domains.insert_runtime(cleanup)?;
    }
    let _ = (&context, domains, options);
    Ok(())
}

async fn serve(
    services: Arc<ServiceRegistry>,
    listener: tokio::net::TcpListener,
) -> crate::Result<()> {
    let app = crate::http::router(services.clone());
    let served = accept(app, listener, &services).await;
    services.lifecycle.advance(Phase::Stopping);
    services.lifecycle.set_listening(false);
    services.shutdown.cancel();
    services.connections.close();
    services.connections.wait().await;
    let mut errors = Vec::new();
    if let Err(error) = served {
        errors.push(format!("http: {error}"));
    }
    match Arc::try_unwrap(services) {
        Ok(services) => errors.extend(close(services).await),
        Err(_) => errors.push(
            "service handles retained after every connection was joined; \
             the database closes and ownership is released when the last one drops"
                .into(),
        ),
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(crate::Error::Service(format!(
            "shutdown completed with errors: {}",
            errors.join("; ")
        )))
    }
}

/// Node's `headersTimeout` default: a client must send its request headers within a minute.
const HEADER_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// Node's `requestTimeout` default (300 s), applied here to each wait for request-body data.
const REQUEST_BODY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// The HTTP accept loop. It owns every HTTP connection task, so shutdown ends them with no
/// grace, as TypeScript does (`HTTP_PREEMPTIVE_SHUTDOWN_GRACE_MS = 0`): a client reading a
/// download slowly cannot hold the home lock. Upgraded WebSockets leave their connection task
/// and close through the shutdown token, joined by the connection tracker.
async fn accept(
    app: axum::Router,
    listener: tokio::net::TcpListener,
    services: &ServiceRegistry,
) -> std::io::Result<()> {
    use hyper_util::{
        rt::{TokioExecutor, TokioIo, TokioTimer},
        server::conn::auto::Builder,
        service::TowerToHyperService,
    };
    let mut builder = Builder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT);
    let mut connections = tokio::task::JoinSet::new();
    let stop = async {
        tokio::select! {
            () = services.shutdown.cancelled() => {}
            () = services.local.ownership_lost() => {
                tracing::error!("server ownership lost; shutting down");
            }
        }
    };
    tokio::pin!(stop);
    loop {
        let (stream, remote) = tokio::select! {
            biased;
            () = &mut stop => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => continue,
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                // Per-connection failures (reset before accept, descriptor limits) do not stop
                // the server; the next accept proceeds.
                Err(error) => {
                    tracing::warn!(%error, "accept failed");
                    continue;
                }
            },
        };
        let _ = stream.set_nodelay(true);
        let service = TowerToHyperService::new(tower::ServiceExt::map_request(
            app.clone(),
            move |mut request: axum::http::Request<hyper::body::Incoming>| {
                request
                    .extensions_mut()
                    .insert(axum::extract::ConnectInfo(remote));
                request.map(|body| {
                    axum::body::Body::new(tower_http::timeout::TimeoutBody::new(
                        REQUEST_BODY_TIMEOUT,
                        body,
                    ))
                })
            },
        ));
        let builder = builder.clone();
        connections.spawn(async move {
            let _ = builder
                .serve_connection_with_upgrades(TokioIo::new(stream), service)
                .await;
        });
    }
    // Close the command gate before the listener stops accepting.
    services.lifecycle.advance(Phase::Stopping);
    services.shutdown.cancel();
    drop(listener);
    connections.shutdown().await;
    Ok(())
}

/// Domains first (while the database lives), then every handle that may hold a database
/// clone, then the database itself; ownership is released last, inside `LocalServices`.
async fn close(services: ServiceRegistry) -> Vec<String> {
    let ServiceRegistry {
        handlers,
        shutdown: _,
        connections: _,
        lifecycle: _,
        mut domains,
        local,
    } = services;
    let mut errors = domains.shutdown().await;
    drop(handlers);
    drop(domains);
    if let Err(error) = local.shutdown().await {
        errors.push(format!("local services: {error}"));
    }
    errors
}

#[cfg(test)]
mod migration_log_tests {
    use super::utc_timestamp;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn timestamps_are_utc_iso_with_milliseconds() {
        let at =
            |seconds: u64, millis: u32| UNIX_EPOCH + Duration::new(seconds, millis * 1_000_000);
        assert_eq!(utc_timestamp(at(0, 0)), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            utc_timestamp(at(1_700_000_000, 5)),
            "2023-11-14T22:13:20.005Z"
        );
        // A leap day, and the end of a year.
        assert_eq!(
            utc_timestamp(at(1_709_164_800, 0)),
            "2024-02-29T00:00:00.000Z"
        );
        assert_eq!(
            utc_timestamp(at(1_767_225_599, 999)),
            "2025-12-31T23:59:59.999Z"
        );
    }
}
