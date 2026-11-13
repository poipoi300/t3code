//! Native Codex app-server JSONL client. This crate does not target wasm.
//!
//! Connect arbitrary Tokio reader/writer halves with [`Client::connect`]. Drain
//! [`Events`] while making requests: approvals must not block response routing.
//! Process ownership, stderr/exit diagnostics and T3 event projection belong to
//! the caller. Dropping all clients, or calling [`Client::shutdown`], stops I/O.

mod client;
mod error;
pub mod launch;
pub mod types;
pub mod wire;

pub use client::{
    Client, ClientOptions, Event, Events, ServerRequest, TypedRequest, TypedServerRequest,
};
pub use error::{Error, Result, RpcError};
pub use types::*;
