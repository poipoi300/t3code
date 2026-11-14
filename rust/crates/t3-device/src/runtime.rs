use crate::{
    DeviceService, Error, Result,
    agent::Launchers,
    host::{DeviceHost, HostFactory},
    io::Http,
    proxy::{Authenticator, DeviceHubProxy, SocketConnector},
    service::{Accounts, Settings, ToolInstaller, wire_state},
};
use futures_util::Stream;
use std::sync::Arc;
use t3_contracts::{
    device::{DeviceServiceState, LOCAL_DEVICE_HOST_ID},
    settings::ServerSettings,
};
use tokio::sync::{Mutex, mpsc, watch};
use tokio_util::sync::CancellationToken;

pub struct RuntimeInputs {
    pub local_host: Arc<dyn DeviceHost>,
    pub host_factory: Arc<dyn HostFactory>,
    pub launchers: Arc<dyn Launchers>,
    pub installer: Arc<dyn ToolInstaller>,
    pub accounts: Arc<dyn Accounts>,
    pub settings: Arc<dyn Settings>,
    pub settings_changes: mpsc::UnboundedReceiver<ServerSettings>,
    pub http: Arc<dyn Http>,
    pub auth: Arc<dyn Authenticator>,
    pub sockets: Arc<dyn SocketConnector>,
}

#[derive(Clone)]
pub struct DeviceRuntime {
    pub service: DeviceService,
    pub proxy: DeviceHubProxy,
    scope: Arc<Scope>,
}
struct Scope {
    cancel: CancellationToken,
    worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
    shutdown: Mutex<bool>,
    status: watch::Receiver<Option<Error>>,
}
impl Drop for Scope {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl DeviceRuntime {
    /// Subscribe to settings changes before calling this startup barrier.
    pub async fn new(inputs: RuntimeInputs) -> Result<Self> {
        if inputs.local_host.id() != LOCAL_DEVICE_HOST_ID {
            return Err(crate::error::invalid("local device host id"));
        }
        let settings = inputs.settings;
        let service = DeviceService::new(
            vec![inputs.local_host],
            settings.clone(),
            Some(inputs.host_factory),
            Some(inputs.launchers),
            Some(inputs.installer),
            Some(inputs.accounts),
        )
        .await?;
        let proxy = DeviceHubProxy {
            service: service.clone(),
            http: inputs.http,
            auth: inputs.auth,
            sockets: inputs.sockets,
        };
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let updates = service.clone();
        let mut changes = inputs.settings_changes;
        let (status_tx, status) = watch::channel(None);
        let worker = tokio::spawn(async move {
            let mut seen = std::collections::BTreeSet::new();
            if let Err(error) = updates.preflight_pools(&mut seen).await {
                status_tx.send_replace(Some(error));
            }
            loop {
                tokio::select! {
                    biased;
                    _ = stop.cancelled() => break,
                    change = changes.recv() => {
                        if change.is_none() {
                            break;
                        }
                    }
                }
                // Notifications may be older than the initial snapshot; always read current settings.
                let result = async {
                    updates.reconcile(settings.read().await?.hosts).await?;
                    updates.preflight_pools(&mut seen).await
                }
                .await;
                status_tx.send_replace(result.err());
            }
        });
        Ok(Self {
            service,
            proxy,
            scope: Arc::new(Scope {
                cancel,
                worker: Mutex::new(Some(worker)),
                shutdown: Mutex::new(false),
                status,
            }),
        })
    }
    pub fn router(&self) -> axum::Router {
        self.proxy.clone().router()
    }
    /// Each observed notification publishes success (`None`) or a safe reconciliation error.
    pub fn settings_status(&self) -> watch::Receiver<Option<Error>> {
        self.scope.status.clone()
    }
    pub async fn state_stream(&self) -> impl Stream<Item = DeviceServiceState> + Send + 'static {
        let mut states = self.service.state_stream().await;
        async_stream::stream! {
            while let Some(state) = states.recv().await {
                yield wire_state(state);
            }
        }
    }
    #[cfg(feature = "mcp")]
    pub fn mcp(&self, threads: Arc<dyn crate::mcp::Threads>) -> crate::mcp::DeviceAdapter {
        crate::mcp::DeviceAdapter {
            service: self.service.clone(),
            threads,
        }
    }
    /// Close server admission first; this awaits reconciliation and retires every host.
    pub async fn shutdown(&self) -> Result<()> {
        let runtime = self.clone();
        tokio::spawn(async move { runtime.shutdown_inner().await })
            .await
            .map_err(|_| Error::Cancelled("device runtime shutdown"))?
    }
    async fn shutdown_inner(&self) -> Result<()> {
        let mut done = self.scope.shutdown.lock().await;
        if *done {
            return Ok(());
        }
        self.scope.cancel.cancel();
        let worker = if let Some(worker) = self.scope.worker.lock().await.take() {
            worker
                .await
                .map_err(|_| Error::Cancelled("device settings reconciliation"))
        } else {
            Ok(())
        };
        let stopped = self.service.stop().await;
        let result = worker.and(stopped);
        *done = result.is_ok();
        result
    }
}
