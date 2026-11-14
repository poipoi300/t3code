//! The seam between the UI and the composed client core.
//!
//! The UI calls these methods and reads the `ClientStore`; it never applies server events,
//! retries commands or decides permissions. Production is `t3_client::composition::ClientRuntime`
//! (adapted below, started by `client.rs`); `demo::DemoBackend` drives the same store behind the
//! explicit demo switch (`host.rs`).

use futures::future::LocalBoxFuture;
use futures::stream::LocalBoxStream;
use futures::{FutureExt, StreamExt};
use t3_client::composition::ClientRuntime;
use t3_client::connection::{ConnectionFailure, FailureKind, PairingInput};
use t3_client::platform::Platform;
use t3_client::types::{EnvironmentId, ThreadKey};

pub use t3_client::composition::{EnvironmentEntry, SendError};

/// The client the UI runs: the composition root over this target's host platform (the one type
/// that differs by target). Service bindings in `runtime/services/` build their handles from it.
#[cfg(not(target_arch = "wasm32"))]
pub type HostClient = ClientRuntime<t3_platform::NativePlatform>;
#[cfg(target_arch = "wasm32")]
pub type HostClient = ClientRuntime<t3_platform::WebPlatform>;

pub trait Backend {
    /// Pairs with the server the user described and registers it. Validation and its messages
    /// are the client's (`t3_client::connection::resolve_pairing_target`).
    fn pair(
        &self,
        input: PairingInput,
    ) -> LocalBoxFuture<'static, Result<EnvironmentId, ConnectionFailure>>;

    /// The saved environments, in catalogue order.
    fn environments(&self) -> Vec<EnvironmentEntry>;

    /// Fires whenever [`Backend::environments`] would answer differently. Store data has its own
    /// subscription; this covers connection state, which the store does not hold.
    fn changes(&self) -> LocalBoxStream<'static, ()>;

    /// Starts following a thread (snapshot, then live events) while a screen shows it.
    fn open_thread(&self, key: &ThreadKey);

    /// Stops following a thread no screen shows. Its cached projection stays readable.
    fn close_thread(&self, key: &ThreadKey);

    /// Sends a user message as a new turn on `key`.
    fn send_message(
        &self,
        key: &ThreadKey,
        text: String,
    ) -> LocalBoxFuture<'static, Result<(), SendError>>;

    /// The client itself, for the service bindings in `runtime/services/` (ui-package-addendum.md,
    /// "Client services"). None for the demo and an unavailable host.
    fn client(&self) -> Option<HostClient> {
        None
    }
}

/// The composition root is the backend: each method is the client's own.
impl<P: Platform + Clone + 'static> Backend for ClientRuntime<P> {
    fn pair(
        &self,
        input: PairingInput,
    ) -> LocalBoxFuture<'static, Result<EnvironmentId, ConnectionFailure>> {
        ClientRuntime::pair(self, input)
    }

    fn environments(&self) -> Vec<EnvironmentEntry> {
        ClientRuntime::environments(self)
    }

    fn changes(&self) -> LocalBoxStream<'static, ()> {
        ClientRuntime::changes(self)
    }

    fn open_thread(&self, key: &ThreadKey) {
        ClientRuntime::open_thread(self, key);
    }

    fn close_thread(&self, key: &ThreadKey) {
        ClientRuntime::close_thread(self, key);
    }

    fn send_message(
        &self,
        key: &ThreadKey,
        text: String,
    ) -> LocalBoxFuture<'static, Result<(), SendError>> {
        ClientRuntime::send_message(self, key, text)
    }

    fn client(&self) -> Option<HostClient> {
        // This runtime over the host's platform; a test runtime over another platform is not one.
        (self as &dyn std::any::Any).downcast_ref::<HostClient>().cloned()
    }
}

/// The client could not start on this host (no app data directory, storage that would not
/// open). Pairing answers with why, so the connect screen shows the reason, not a dead button.
pub struct Unavailable(pub String);

impl Backend for Unavailable {
    fn pair(
        &self,
        _input: PairingInput,
    ) -> LocalBoxFuture<'static, Result<EnvironmentId, ConnectionFailure>> {
        futures::future::ready(Err(ConnectionFailure {
            kind: FailureKind::Unsupported,
            detail: self.0.clone(),
            http_status: None,
            trace_id: None,
            hop: None,
            via: None,
            cause: None,
            server_update_required: false,
            verification_code: None,
        }))
        .boxed_local()
    }

    fn environments(&self) -> Vec<EnvironmentEntry> {
        Vec::new()
    }

    fn changes(&self) -> LocalBoxStream<'static, ()> {
        futures::stream::empty().boxed_local()
    }

    fn open_thread(&self, _key: &ThreadKey) {}

    fn close_thread(&self, _key: &ThreadKey) {}

    fn send_message(
        &self,
        _key: &ThreadKey,
        _text: String,
    ) -> LocalBoxFuture<'static, Result<(), SendError>> {
        futures::future::ready(Err(SendError::Disconnected)).boxed_local()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use std::rc::Rc;
    use t3_client::ClientStore;
    use t3_client::connection::resolve_pairing_target;
    use t3_client::testing::FakePlatform;
    use t3_client::types::ThreadId;
    use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;

    fn client() -> Rc<dyn Backend> {
        Rc::new(ClientRuntime::new(
            FakePlatform::default(),
            ClientStore::default(),
            OrchestrationV2CreationSource::Web,
            Rc::new(|_| {}),
        ))
    }

    #[test]
    fn the_adapter_answers_with_the_composition_roots_own_results() {
        let backend = client();
        let input = PairingInput {
            pairing_url: Some("https://example.test/pair".into()),
            ..PairingInput::default()
        };
        let Err(expected) = resolve_pairing_target(&input) else {
            panic!("a link without a token is invalid");
        };
        let failure = block_on(backend.pair(input)).expect_err("the client's validation");
        assert_eq!(failure.detail, expected.detail);
        assert!(backend.environments().is_empty());
        let key = ThreadKey {
            environment: EnvironmentId("nowhere".into()),
            thread: ThreadId("thread".into()),
        };
        assert_eq!(
            block_on(backend.send_message(&key, "  ".into())),
            Err(SendError::Rejected("Enter a message.".into()))
        );
        assert_eq!(
            block_on(backend.send_message(&key, "hello".into())),
            Err(SendError::Disconnected)
        );
        backend.open_thread(&key);
        backend.close_thread(&key);
    }
}
