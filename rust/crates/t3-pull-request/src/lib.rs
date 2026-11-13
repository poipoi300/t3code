//! Native-only pull-request application services; not a wasm32-unknown-unknown crate.
pub mod cache;
pub mod context;
pub mod error;
#[cfg(feature = "forge")]
pub mod forge;
pub mod guide;
pub mod host;
pub mod linked;
#[cfg(feature = "orchestration")]
pub mod orchestration;
pub mod service;
#[cfg(feature = "sqlite")]
pub mod sqlite;
pub mod watch;
pub use error::{Error, FailureReason, Result};
pub use service::PullRequestService;
pub use t3_contracts::pull_request;

pub(crate) fn convert<T: serde::de::DeserializeOwned>(value: impl serde::Serialize) -> Result<T> {
    serde_json::from_value(
        serde_json::to_value(value)
            .map_err(|_| Error::invalid("decode", "Invalid pull request data."))?,
    )
    .map_err(|_| Error::invalid("decode", "Invalid pull request data."))
}
