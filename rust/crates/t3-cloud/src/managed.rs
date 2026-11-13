use crate::{
    Result, SharedClock,
    http::{Http, Sensitive},
    store::SecretStore,
    tunnel::*,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use t3_contracts::relay::{RelayManagedEndpointProviderKind, RelayManagedEndpointRuntimeConfig};
use tokio::sync::{Mutex, broadcast};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum EndpointStatus {
    Disabled,
    Running {
        provider_kind: RelayManagedEndpointProviderKind,
        pid: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        tunnel_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tunnel_name: Option<String>,
    },
    Failed {
        provider_kind: RelayManagedEndpointProviderKind,
        failure: String,
        reason: String,
    },
    Unsupported {
        provider_kind: RelayManagedEndpointProviderKind,
    },
}
#[derive(Debug, Clone)]
pub enum RuntimeNotice {
    Connected,
    ReadinessChanged,
    RecoveryRequired,
}

#[async_trait]
pub trait ManagedEndpoint: Send + Sync {
    async fn apply(
        &self,
        config: Option<Sensitive<RelayManagedEndpointRuntimeConfig>>,
    ) -> Result<EndpointStatus>;
    async fn inspect(&self) -> Option<ConnectorFacts>;
    fn subscribe(&self) -> broadcast::Receiver<RuntimeNotice>;
}
struct Active {
    config: Sensitive<RelayManagedEndpointRuntimeConfig>,
    handle: ConnectorHandle,
    monitor: tokio::task::JoinHandle<()>,
}
impl Drop for Active {
    fn drop(&mut self) {
        self.monitor.abort();
    }
}

pub struct NativeManagedEndpoint {
    executable: PathBuf,
    http: Http,
    secrets: SecretStore,
    clock: SharedClock,
    active: Mutex<Option<Active>>,
    notices: broadcast::Sender<RuntimeNotice>,
}
impl NativeManagedEndpoint {
    pub fn new(
        executable: PathBuf,
        http: Http,
        secrets: SecretStore,
        clock: SharedClock,
    ) -> Arc<Self> {
        Arc::new(Self {
            executable,
            http,
            secrets,
            clock,
            active: Mutex::new(None),
            notices: broadcast::channel(64).0,
        })
    }
}
#[async_trait]
impl ManagedEndpoint for NativeManagedEndpoint {
    async fn apply(
        &self,
        config: Option<Sensitive<RelayManagedEndpointRuntimeConfig>>,
    ) -> Result<EndpointStatus> {
        if let Some(config) = &config {
            validate_runtime(config.expose())?;
        }
        let mut active = self.active.lock().await;
        if let (Some(current), Some(next)) = (active.as_ref(), config.as_ref()) {
            if current.config.expose() == next.expose() {
                if let Some(pid) = current.handle.facts.borrow().pid {
                    return Ok(running(next.expose(), pid));
                }
            }
        }
        if let Some(mut old) = active.take() {
            old.monitor.abort();
            old.handle.cancel_and_wait().await?;
            let _ = (&mut old.monitor).await;
        }
        let Some(config) = config else {
            return Ok(EndpointStatus::Disabled);
        };
        let mut handle = start_connector(
            self.executable.clone(),
            Sensitive::new(config.expose().clone()),
            self.clock.clone(),
            ConnectorServices {
                http: self.http.clone(),
                secrets: self.secrets.clone(),
            },
        )?;
        let started = handle
            .facts
            .wait_for(|facts| facts.pid.is_some())
            .await
            .map(|facts| facts.pid)
            .ok()
            .flatten();
        let Some(pid) = started else {
            let _ = handle.stop().await;
            return Ok(EndpointStatus::Failed {
                provider_kind: RelayManagedEndpointProviderKind::CloudflareTunnel,
                failure: "spawn-failed".into(),
                reason: "Could not start the tunnel client.".into(),
            });
        };
        let mut events = handle.events.clone();
        let mut facts = handle.facts.clone();
        let notices = self.notices.clone();
        let monitor = tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = events.changed() => {
                        if result.is_err() { break; }
                        let notice = match *events.borrow_and_update() {
                            Some(ConnectorEvent::Connected) => RuntimeNotice::Connected,
                            Some(ConnectorEvent::RecoveryRequired) => RuntimeNotice::RecoveryRequired,
                            _ => RuntimeNotice::ReadinessChanged,
                        };
                        let _ = notices.send(notice);
                    },
                    result = facts.changed() => { if result.is_err() { break; } let _ = notices.send(RuntimeNotice::ReadinessChanged); },
                }
            }
        });
        let status = running(config.expose(), pid);
        *active = Some(Active {
            config,
            handle,
            monitor,
        });
        Ok(status)
    }
    async fn inspect(&self) -> Option<ConnectorFacts> {
        self.active
            .lock()
            .await
            .as_ref()
            .map(|active| active.handle.facts.borrow().clone())
    }
    fn subscribe(&self) -> broadcast::Receiver<RuntimeNotice> {
        self.notices.subscribe()
    }
}
fn running(config: &RelayManagedEndpointRuntimeConfig, pid: u32) -> EndpointStatus {
    EndpointStatus::Running {
        provider_kind: config.provider_kind,
        pid,
        tunnel_id: config.tunnel_id.clone().flatten().map(|v| v.0),
        tunnel_name: config.tunnel_name.clone().flatten().map(|v| v.0),
    }
}
