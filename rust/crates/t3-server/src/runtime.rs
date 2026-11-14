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
        || config.dev_url.is_some()
    {
        return Err(crate::Error::Service(
            "LAN fencing, bootstrap-fd, dev proxy and Tailscale are not yet ported".into(),
        ));
    }
    let host = config.host.clone().unwrap_or_else(|| "0.0.0.0".into());
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
    let local = LocalServices::open(config, address.port()).await?;
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
    let rpc_methods_served = t3_contracts::rpc::WS_METHODS
        .iter()
        .filter(|method| handlers.disposition(method.tag) == Some(true))
        .count();
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
        shutdown,
        phase,
        task: Some(task),
    })
}

/// TypeScript's tail of `serverRuntimeStartup`: once recovery has finished and HTTP listens,
/// publish `welcome`, activate (domain schedulers, the runtime file), open the command gate,
/// then publish `ready`.
async fn activate(services: &ServiceRegistry) -> crate::Result<()> {
    services.local.publish_welcome()?;
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
    #[cfg(feature = "workspace")]
    let workspaces = crate::workspaces::EnvironmentWorkspaces::new(&context);
    #[cfg(all(feature = "git", feature = "workspace"))]
    domains.insert_runtime(crate::adapters::git::GitAdapter::start_with_authority(
        &context,
        workspaces.clone(),
    ))?;
    #[cfg(all(feature = "git", not(feature = "workspace")))]
    domains.insert_runtime(crate::adapters::git::GitAdapter::start(&context))?;
    #[cfg(feature = "workspace")]
    domains.insert(crate::workspaces::workspace_adapter(&context, workspaces).await?)?;
    #[cfg(feature = "terminal")]
    {
        domains.insert_runtime(crate::adapters::terminal::TerminalAdapter::start(&context)?)?;
        domains.insert(crate::workspaces::terminal_transport())?;
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
    #[cfg(feature = "scheduler")]
    domains.insert_runtime(crate::adapters::scheduler::SchedulerAdapter::start(&context).await?)?;
    let _ = (&context, domains, options);
    Ok(())
}

async fn serve(
    services: Arc<ServiceRegistry>,
    listener: tokio::net::TcpListener,
) -> crate::Result<()> {
    let app = crate::http::router(services.clone());
    let watcher = services.clone();
    let served = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        tokio::select! {
            () = watcher.shutdown.cancelled() => {}
            () = watcher.local.ownership_lost() => {
                tracing::error!("server ownership lost; shutting down");
            }
        }
        // Close the command gate before the listener stops accepting.
        watcher.lifecycle.advance(Phase::Stopping);
        watcher.shutdown.cancel();
    })
    .await;
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
