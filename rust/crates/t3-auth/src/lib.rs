//! Native server authentication and authorization; does not target wasm32-unknown-unknown.
pub mod authorization;
pub mod clock;
pub mod dpop;
pub mod environment;
pub mod error;
pub mod oauth;
pub mod pairing;
pub mod secrets;
pub mod session;
pub mod utils;

pub use environment::{AuthConfig, EnvironmentAuth, Request};
pub use error::{Error, Result};
pub use pairing::PairingGrantStore;
pub use secrets::ServerSecretStore;
pub use session::SessionStore;

// SQLite blocking work survives a dropped caller; finish publishing its committed state too.
pub(crate) async fn finish_mutation<T: Send + 'static>(
    work: impl Future<Output = Result<T>> + Send + 'static,
) -> Result<T> {
    tokio::spawn(work).await?
}
