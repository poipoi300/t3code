//! State shared by every transport and handler of one running environment.
use crate::registry::HandlerRegistry;
use futures::future::BoxFuture;
use std::{any::Any, sync::Arc};
use tokio::sync::watch;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub struct ServiceRegistry {
    pub handlers: HandlerRegistry,
    pub shutdown: CancellationToken,
    pub connections: TaskTracker,
    pub lifecycle: Lifecycle,
    pub domains: Domains,
    #[cfg(feature = "local-services")]
    pub local: crate::adapters::config::LocalServices,
}
impl ServiceRegistry {
    /// The started domain service of type `T`; `None` when its feature is off or it was not started.
    pub fn domain<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.domains.get::<T>()
    }
    /// The started orchestration adapter, for the orchestration HTTP routes and handlers.
    #[cfg(feature = "orchestration")]
    pub fn orchestration(
        &self,
    ) -> Option<Arc<crate::adapters::orchestration::OrchestrationAdapter>> {
        self.domain()
    }
}

/// Startup and shutdown phases. Listening is tracked separately: commands may be ready before
/// HTTP accepts, and HTTP may accept (pairing, config) while commands are still recovering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    Starting,
    CommandReady,
    Ready,
    Stopping,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandAdmissionError {
    #[error("the server is still starting and does not accept commands yet")]
    NotReady,
    #[error("the server is shutting down and no longer accepts commands")]
    Stopping,
}

pub struct Lifecycle {
    phase: watch::Sender<Phase>,
    listening: watch::Sender<bool>,
}
impl Default for Lifecycle {
    fn default() -> Self {
        Self {
            phase: watch::channel(Phase::Starting).0,
            listening: watch::channel(false).0,
        }
    }
}
impl Lifecycle {
    pub fn phase(&self) -> Phase {
        *self.phase.borrow()
    }
    pub fn subscribe(&self) -> watch::Receiver<Phase> {
        self.phase.subscribe()
    }
    pub fn is_command_ready(&self) -> bool {
        matches!(self.phase(), Phase::CommandReady | Phase::Ready)
    }
    pub fn is_listening(&self) -> bool {
        *self.listening.borrow()
    }
    /// The command gate: dispatch paths call this before admitting a new command.
    pub fn admit_command(&self) -> Result<(), CommandAdmissionError> {
        match self.phase() {
            Phase::Starting => Err(CommandAdmissionError::NotReady),
            Phase::CommandReady | Phase::Ready => Ok(()),
            Phase::Stopping => Err(CommandAdmissionError::Stopping),
        }
    }
    /// Resolves once the server is fully ready; returns false if it began stopping first.
    pub async fn wait_ready(&self) -> bool {
        let mut phase = self.subscribe();
        phase
            .wait_for(|p| matches!(p, Phase::Ready | Phase::Stopping))
            .await
            .is_ok_and(|p| *p == Phase::Ready)
    }
    #[cfg_attr(not(feature = "local-services"), allow(dead_code))]
    pub(crate) fn advance(&self, next: Phase) {
        // Phases only move forward; a late barrier can never reopen a closed command gate.
        self.phase.send_if_modified(|phase| {
            let advance = next > *phase;
            if advance {
                *phase = next;
            }
            advance
        });
    }
    #[cfg_attr(not(feature = "local-services"), allow(dead_code))]
    pub(crate) fn set_listening(&self, listening: bool) {
        self.listening.send_replace(listening);
    }
}

/// A domain service that takes part in shutdown (orchestration worker, terminal manager, ...).
///
/// Adapters in `adapters/<crate>.rs` build their service from a [`DomainContext`]; the start
/// future completes only after the domain's recovery barrier, so command readiness waits for it.
/// Services that need no shutdown step are inserted with [`Domains::insert`] alone.
pub trait DomainRuntime: Any + Send + Sync {
    fn name(&self) -> &'static str;
    /// Runs once, in reverse start order, after every connection is joined and while the
    /// database is still open. Errors are reported and shutdown continues.
    fn shutdown(&self) -> BoxFuture<'_, crate::Result<()>>;
    /// Runs once, in start order, after every domain has recovered and HTTP is listening and
    /// before commands are admitted (TypeScript's `ServerActivation`). Schedulers start here.
    fn activate(&self) -> BoxFuture<'_, crate::Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

/// What a domain adapter receives when the composition root starts it.
#[cfg(feature = "local-services")]
pub struct DomainContext<'a> {
    pub local: &'a crate::adapters::config::LocalServices,
    pub database: crate::adapters::persistence::Database,
    /// Cancelled when shutdown begins; long-running domain tasks select on it.
    pub shutdown: CancellationToken,
    /// The account home captured at startup (not the T3 home), as TypeScript's `os.homedir()`.
    pub account_home: std::path::PathBuf,
    /// The process environment captured at startup.
    pub environment: std::collections::BTreeMap<String, String>,
}

/// Started domain services, looked up by concrete type. Built before the handler table so
/// handler modules can capture the services they serve (`handlers::registry_with`).
#[derive(Default)]
pub struct Domains {
    services: Vec<Arc<dyn Any + Send + Sync>>,
    runtimes: Vec<Arc<dyn DomainRuntime>>,
}
impl Domains {
    pub fn get<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.services
            .iter()
            .find_map(|service| service.clone().downcast::<T>().ok())
    }
    pub fn names(&self) -> Vec<&'static str> {
        self.runtimes.iter().map(|d| d.name()).collect()
    }
    /// Makes a started service available to handlers; one instance per type.
    pub fn insert<T: Any + Send + Sync>(&mut self, service: Arc<T>) -> crate::Result<()> {
        if self.get::<T>().is_some() {
            return Err(crate::Error::Service(format!(
                "{} started twice",
                std::any::type_name::<T>()
            )));
        }
        self.services.push(service);
        Ok(())
    }
    /// Inserts a service that also needs a shutdown step.
    pub fn insert_runtime<T: DomainRuntime>(&mut self, runtime: Arc<T>) -> crate::Result<()> {
        self.insert(runtime.clone())?;
        self.runtimes.push(runtime);
        Ok(())
    }
    /// Activates every runtime in start order; the first failure aborts startup.
    #[cfg_attr(not(feature = "local-services"), allow(dead_code))]
    pub(crate) async fn activate(&self) -> crate::Result<()> {
        for runtime in &self.runtimes {
            runtime.activate().await.map_err(|error| {
                crate::Error::Service(format!("{} activation: {error}", runtime.name()))
            })?;
        }
        Ok(())
    }
    /// Shuts every runtime down in reverse start order, continuing after failures, then
    /// drops every service handle this registry holds.
    #[cfg_attr(not(feature = "local-services"), allow(dead_code))]
    pub(crate) async fn shutdown(&mut self) -> Vec<String> {
        let mut errors = Vec::new();
        while let Some(runtime) = self.runtimes.pop() {
            if let Err(error) = runtime.shutdown().await {
                errors.push(format!("{}: {error}", runtime.name()));
            }
        }
        self.services.clear();
        errors
    }
}

// Every transport holds `Arc<ServiceRegistry>` across awaits; keep that future `Send`.
const _: fn() = || {
    fn shared<T: Send + Sync>() {}
    shared::<ServiceRegistry>();
};

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn the_command_gate_opens_at_command_ready_and_never_reopens_after_stopping() {
        let lifecycle = Lifecycle::default();
        assert_eq!(
            lifecycle.admit_command(),
            Err(CommandAdmissionError::NotReady)
        );
        lifecycle.advance(Phase::CommandReady);
        assert_eq!(lifecycle.admit_command(), Ok(()));
        lifecycle.advance(Phase::Stopping);
        lifecycle.advance(Phase::Ready);
        assert_eq!(lifecycle.phase(), Phase::Stopping);
        assert_eq!(
            lifecycle.admit_command(),
            Err(CommandAdmissionError::Stopping)
        );
    }

    struct Recorder {
        name: &'static str,
        fails: bool,
        log: Arc<Mutex<Vec<&'static str>>>,
    }
    impl DomainRuntime for Recorder {
        fn name(&self) -> &'static str {
            self.name
        }
        fn shutdown(&self) -> BoxFuture<'_, crate::Result<()>> {
            Box::pin(async move {
                self.log
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(self.name);
                if self.fails {
                    Err(crate::Error::Service("boom".into()))
                } else {
                    Ok(())
                }
            })
        }
    }
    struct Other;

    #[tokio::test]
    async fn domains_shut_down_in_reverse_order_and_continue_after_a_failure() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut domains = Domains::default();
        for (name, fails) in [("stores", false), ("orchestration", true)] {
            let recorder = Recorder {
                name,
                fails,
                log: log.clone(),
            };
            if name == "stores" {
                domains.insert_runtime(Arc::new(recorder)).unwrap();
            } else {
                // A second runtime of the same type is a composition bug, so wrap it.
                struct Wrapped(Recorder);
                impl DomainRuntime for Wrapped {
                    fn name(&self) -> &'static str {
                        self.0.name()
                    }
                    fn shutdown(&self) -> BoxFuture<'_, crate::Result<()>> {
                        self.0.shutdown()
                    }
                }
                domains.insert_runtime(Arc::new(Wrapped(recorder))).unwrap();
            }
        }
        domains.insert(Arc::new(Other)).unwrap();
        assert!(domains.insert(Arc::new(Other)).is_err());
        assert!(domains.get::<Other>().is_some());
        assert_eq!(domains.names(), ["stores", "orchestration"]);
        let errors = domains.shutdown().await;
        assert_eq!(
            errors,
            ["orchestration: service startup or shutdown failed: boom"]
        );
        assert_eq!(*log.lock().unwrap(), ["orchestration", "stores"]);
        assert!(domains.get::<Other>().is_none());
    }
}
