//! The application runtime: one store, one backend, one adapter, owned by `App`.

pub mod backend;
pub mod demo;
pub mod host;
pub mod store;

use std::rc::Rc;

use futures::StreamExt;
use nsh_core::prelude::*;
use t3_client::ClientStore;

use backend::{Backend, EnvironmentEntry};
pub use store::{Shared, StoreSignals, Timeline};

/// The runtime as a `Copy` context handle.
#[derive(Clone, Copy)]
pub struct Rt {
    pub store: StoreSignals,
    backend: Stored<Rc<dyn Backend>>,
    environments: Signal<Vec<EnvironmentEntry>>,
}

impl Rt {
    /// Builds the runtime under the current owner (`App`) and provides it.
    pub fn boot() -> Self {
        let store = ClientStore::default();
        let backend = host::backend(store.clone());
        let rt = Self::new(store, backend);
        provide_context(rt);
        rt
    }

    pub fn new(store: ClientStore, backend: Rc<dyn Backend>) -> Self {
        let signals = StoreSignals::attach(store);
        let environments = signal(backend.environments());
        let mut changes = backend.changes();
        let source = backend.clone();
        spawn(async move {
            while changes.next().await.is_some() {
                environments.set(source.environments());
            }
        });
        Self {
            store: signals,
            backend: stored(backend),
            environments,
        }
    }

    pub fn backend(&self) -> Rc<dyn Backend> {
        self.backend.get()
    }

    /// The saved environments with their connection state; reading it subscribes.
    pub fn environments(&self) -> Vec<EnvironmentEntry> {
        self.environments.get()
    }

    pub fn environments_signal(&self) -> Signal<Vec<EnvironmentEntry>> {
        self.environments
    }
}

pub fn rt() -> Rt {
    use_context::<Rt>().expect("App provides the runtime")
}
