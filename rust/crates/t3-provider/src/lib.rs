//! Native, transport-neutral coding-provider configuration and discovery.
//! This crate does not target wasm32-unknown-unknown. Adapters live in t3-orchestration.

pub mod account_usage;
pub mod auth;
pub mod callback;
pub mod catalog;
pub mod chatgpt;
pub mod claude_catalog;
pub mod claude_probe_cache;
pub mod claude_skills;
pub mod codex_installation;
pub mod credential_lease;
pub mod cursor_catalog;
pub mod discovery;
pub mod environment;
pub mod fx;
pub mod handoff;
pub mod homes;
pub mod identity;
pub mod installations;
pub mod instances;
pub mod maintenance;
pub mod maintenance_resolution;
pub mod managed_codex;
pub mod models;
pub mod modes;
pub mod native_probe;
pub mod native_provider;
pub mod policy;
pub mod probe_models;
pub mod registry;
pub mod reset_credits;
pub mod secrets;
pub mod sensitive;
pub mod service;
pub mod settings;
pub mod text_generators;
pub mod token_broker;
pub mod transport;
pub mod usage;
pub mod versions;

#[derive(thiserror::Error)]
pub enum Error {
    #[error("The provider command failed.")]
    Process(#[from] t3_process::ProcessError),
    #[error("The provider returned invalid configuration or data.")]
    Json(#[from] serde_json::Error),
    #[error("Provider files could not be read or written.")]
    Io(#[from] std::io::Error),
    #[error("The provider request failed.")]
    Http(#[from] reqwest::Error),
    #[error("Invalid provider configuration ({context}).")]
    Invalid {
        context: &'static str,
        detail: String,
    },
    #[error("provider operation is not ported: {0}")]
    NotPorted(&'static str),
}
impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn invalid(context: &'static str, detail: impl Into<String>) -> Error {
    Error::Invalid {
        context,
        detail: detail.into(),
    }
}
