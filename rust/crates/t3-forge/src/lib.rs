//! Native source-control forge adapters; not a wasm32-unknown-unknown dependency.
pub mod cli;
pub mod clone_progress;
pub mod discovery;
pub mod error;
pub mod forge;
pub mod github;
pub mod gitlab;
pub mod model;
pub mod rate_limit;
pub mod registry;
pub mod remote;
pub mod review_host;
pub mod services;
pub use error::{FailureReason, ForgeError, Result};
pub use forge::Forge;
pub use model::*;
mod support;
