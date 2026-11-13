//! The seam between the UI and the composed client core.
//!
//! The UI calls these methods and reads the `ClientStore`; it never applies server events,
//! retries commands or decides permissions. `t3-client` will provide the production
//! implementation (its composition root: catalog, pairing, supervisor, RPC session, shell and
//! thread sync, operations); until then `demo::DemoBackend` drives the same store.

use futures::future::LocalBoxFuture;
use futures::stream::LocalBoxStream;
use t3_client::connection::{ConnectionFailure, ConnectionState, PairingInput};
use t3_client::types::{EnvironmentId, ThreadKey};

/// One saved environment as the connection layer sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct EnvironmentEntry {
    pub id: EnvironmentId,
    pub label: String,
    pub state: ConnectionState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendError {
    /// The environment has no live session; the message was not accepted.
    Disconnected,
    /// The server or the client's validation refused it, with the client's English reason.
    Rejected(String),
}

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
}
