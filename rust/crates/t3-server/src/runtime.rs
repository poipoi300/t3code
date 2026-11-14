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
    /// RPC methods this server serves (registered, not `NotImplemented`), of the contract's 220.
    pub rpc_methods_served: usize,
    /// The contract methods this server answers with Effect's unknown-request-tag defect.
    pub unserved_methods: Vec<&'static str>,
    shutdown: CancellationToken,
    phase: watch::Receiver<Phase>,
    task: Option<tokio::task::JoinHandle<crate::Result<()>>>,
}
impl RunningServer {
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

async fn start_configured(
    inputs: crate::adapters::config::ConfigInputs,
    options: StartOptions,
    install_global_tracing: bool,
) -> crate::Result<RunningServer> {
    let mut config = resolve(&inputs)?;
    if config.private_network_only
        || config.bootstrap_fd.is_some()
        || config.tailscale_serve_enabled
    {
        return Err(crate::Error::Service(
            "LAN fencing, bootstrap-fd and Tailscale are not yet ported".into(),
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
    #[cfg(feature = "workspace")]
    let workspaces = crate::workspaces::EnvironmentWorkspaces::new(&context);
    #[cfg(feature = "providers")]
    domains.insert(workspaces.clone())?;
    #[cfg(all(feature = "git", feature = "workspace"))]
    domains.insert_runtime(crate::adapters::git::GitAdapter::start_with_authority(
        &context,
        workspaces.clone(),
    ))?;
    #[cfg(all(feature = "git", not(feature = "workspace")))]
    domains.insert_runtime(crate::adapters::git::GitAdapter::start(&context))?;
    #[cfg(feature = "cloud")]
    domains.insert_runtime(crate::adapters::cloud::CloudAdapter::start(&context).await?)?;
    // Device hosts resolve a thread's workspace through the same authority.
    #[cfg(feature = "device")]
    let device_workspaces = workspaces.clone();
    #[cfg(feature = "workspace")]
    domains.insert(crate::workspaces::workspace_adapter(&context, workspaces).await?)?;
    #[cfg(feature = "terminal")]
    {
        domains.insert_runtime(crate::adapters::terminal::TerminalAdapter::start(&context)?)?;
        domains.insert(crate::workspaces::terminal_transport())?;
    }
    #[cfg(feature = "preview")]
    if let Some(preview) = crate::adapters::preview::PreviewAdapter::start(&context).await? {
        #[cfg(feature = "terminal")]
        if let Some(terminal) = domains.get::<crate::adapters::terminal::TerminalAdapter>() {
            preview.observe_terminals(terminal);
        }
        domains.insert_runtime(preview)?;
    }
    #[cfg(feature = "usage")]
    domains.insert(crate::adapters::usage::UsageAdapter::start(&context).await?)?;
    #[cfg(feature = "orchestration")]
    domains.insert_runtime(
        crate::adapters::orchestration::OrchestrationAdapter::start(
            &context,
            options.providers.clone(),
            options.dev_replay,
        )
        .await?,
    )?;
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
        domains.insert_runtime(providers)?;
    }
    // After the providers: devices read secret references through the same resolver.
    #[cfg(feature = "device")]
    if let Some(device) = crate::adapters::device::DeviceAdapter::start(
        &context,
        device_workspaces,
        domains.get::<t3_provider::secrets::SecretResolver>(),
    )
    .await?
    {
        domains.insert_runtime(device)?;
    }
    // Before projects: promoting a scratch thread moves its scheduled tasks.
    #[cfg(feature = "scheduler")]
    let scheduler = crate::adapters::scheduler::SchedulerAdapter::start(&context).await?;
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
        domains.insert_runtime(crate::adapters::project::ProjectAdapter::start(
            &context,
            &orchestration,
            scheduled_tasks,
        )?)?;
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
