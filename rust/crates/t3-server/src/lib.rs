//! Native server composition and transports; does not target wasm32-unknown-unknown.
pub mod adapters;
#[cfg(feature = "local-services")]
pub mod cli;
pub mod handlers;
pub mod http;
pub mod registry;
#[cfg(feature = "local-services")]
pub mod runtime;
pub mod services;
pub mod shutdown;
#[cfg(all(
    feature = "local-services",
    any(feature = "workspace", feature = "terminal")
))]
pub mod workspaces;
pub mod writers;
pub mod ws;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("service startup or shutdown failed: {0}")]
    Service(String),
    #[error("invalid handler registry: {0}")]
    Registry(String),
    #[error("server task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    /// Another server owns this T3 home. Supervisors must not restart (TypeScript exit 78).
    #[error("{0}")]
    StateDirOwned(String),
    /// The organisation's policy cannot be used. Supervisors must not restart (TypeScript
    /// `ManagedPolicyFileError`, exit 77).
    #[error("{0}")]
    ManagedPolicy(String),
}
pub type Result<T> = std::result::Result<T, Error>;
