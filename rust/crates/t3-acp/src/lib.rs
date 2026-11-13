//! Native ACP client over caller-owned Tokio pipes. Does not target wasm32.
//!
//! `Client::connect` returns a cloneable request handle and an ordered event stream.
//! Handle events serially while prompting. `Events::next` acknowledges the previous
//! event; call `Events::acknowledge` before awaiting a dependent request instead.
//! Bounded queues backpressure the reader. `shutdown` closes pipes, not a process.

mod batch;
mod client;
mod compat;
mod raw;
mod transport;
mod types;

pub use agent_client_protocol::{schema, v1, v2};
pub use async_trait::async_trait;
pub use batch::{TextBatcher, TextSnapshot};
pub use client::{Client, ClientOptions, Events};
pub use raw::RawNotifications;
pub use types::*;
