//! Native remote connectivity services. Does not target wasm32-unknown-unknown.
pub mod api;
pub mod cloud;
pub mod crypto;
pub mod direct;
pub mod guest;
pub mod http;
pub mod managed;
pub mod oauth;
pub mod peer;
pub mod reachability;
pub mod retry;
pub mod store;
pub mod tunnel;

use serde::{Deserialize, Serialize};
use std::{fmt, sync::Arc};
use zeroize::Zeroizing;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    Unauthorized,
    Forbidden,
    Rejected,
    Unavailable,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("remote request failed ({rejection:?})")]
    Remote {
        rejection: Rejection,
        retry_after_ms: Option<u64>,
    },
    #[error("managed endpoint is unavailable")]
    EndpointUnavailable { status: managed::EndpointStatus },
    #[error("invalid remote configuration")]
    Configuration,
    #[error("remote proof rejected")]
    Proof,
    #[error("remote proof replayed")]
    Replay,
    #[error("secret storage operation failed")]
    Storage,
    #[error("remote response has an invalid shape")]
    Protocol,
    #[error("remote MCP call refused ({code:?})")]
    PeerFailure {
        code: t3_contracts::orchestrator_mcp::OrchestratorMcpFailureCode,
    },
    #[error("environment is not linked")]
    NotLinked,
    #[error("remote operation was superseded")]
    Superseded,
    #[error("remote operation is disabled")]
    Disabled,
    #[error("remote identity or capabilities do not match")]
    Identity,
    #[error("remote credential expired")]
    Expired,
    #[error("cloud authorization must be renewed by signing in")]
    LoginRequired,
    #[error("guest access rejected ({0:?})")]
    Guest(guest::GuestFailure),
    #[error("owned connector process failed")]
    Process,
    #[error("connection I/O failed")]
    Io,
}

/// Explicit exposure is required at serialization or transport boundaries.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(#[serde(with = "secret_serde")] Zeroizing<String>);
impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}
mod secret_serde {
    use super::*;
    pub fn serialize<S: serde::Serializer>(v: &str, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(v)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> std::result::Result<Zeroizing<String>, D::Error> {
        String::deserialize(d).map(Zeroizing::new)
    }
}

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}
pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}
pub type SharedClock = Arc<dyn Clock>;
pub(crate) fn iso(ms: i64) -> Result<String> {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or(Error::Protocol)
}
pub(crate) fn decode<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> Result<T> {
    serde_json::from_value(v).map_err(|_| Error::Protocol)
}
pub(crate) fn encode<T: Serialize>(v: &T) -> Result<serde_json::Value> {
    serde_json::to_value(v).map_err(|_| Error::Protocol)
}
pub(crate) fn text(value: impl Into<String>) -> t3_contracts::base_schemas::TrimmedNonEmptyString {
    t3_contracts::base_schemas::TrimmedNonEmptyString(value.into())
}
