//! Native agent callback server. This crate does not target wasm32.

pub mod access;
pub mod adapters;
pub mod apps;
pub mod authorization;
pub mod bridge;
pub mod catalog;
pub mod decode;
pub mod error;
pub mod exposure;
pub mod instructions;
#[cfg(feature = "auth")]
pub mod oauth;
pub mod output;
pub mod provider_sessions;
pub mod scope;
pub mod server;
pub mod services;
pub mod session;
pub mod transport;

pub use access::{AccessRule, AccessStore, CallContext, LinkOrigin, ThreadShell};
pub use decode::{DecodedInput, decode_input};
pub use error::{McpError, Result};
pub use scope::{Capability, ClientAccess, ClientCaller, InvocationScope, Modes, ThreadCaller};
pub use server::McpServer;
pub use services::Services;
pub use session::{CredentialRequest, ProviderSessionConfig, SessionRegistry};
