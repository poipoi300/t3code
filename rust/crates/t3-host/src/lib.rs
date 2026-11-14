//! Native, transport-neutral host services. This crate does not target wasm.

pub mod background;
pub mod database_storage;
pub mod desktop_update;
pub mod editor;
pub mod environment_theme;
pub mod guest;
pub mod keep_awake;
pub mod npm_release;
pub mod pinned_runtime;
pub mod policy;
pub mod policy_templates;
pub mod review;
pub mod secret_requests;
pub mod secrets;
pub mod self_update;
pub mod service_launcher;
pub mod storage_cleanup;
pub mod triage;
pub mod worktrees;
pub mod zip_archive;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    ManagedPolicyRefused(#[from] policy::ManagedPolicyRefusedError),
    #[error("{0}")]
    ManagedUpdateRefused(String),
    #[error("Could not read the credential from the secret provider \"{}\". Unlock or fix it in Settings, then try again.",.0.provider)]
    SettingsSecretUnresolved(t3_contracts::settings_secret_value::SettingsSecretUnresolvedError),
    #[error("invalid timestamp")]
    Timestamp,
    #[error("host filesystem operation failed")]
    Io,
    #[error("host service operation failed")]
    Service,
    #[error("invalid input")]
    Invalid,
    #[error("operation refused: {0}")]
    Refused(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;
