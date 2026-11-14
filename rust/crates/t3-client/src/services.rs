//! Resource lifetimes and client-held service projections, shared by every shell.
pub mod assets;
pub mod command_output;
pub mod config;
pub mod default_theme;
pub mod filesystem;
pub mod http;
pub mod models;
pub mod policy;
pub mod providers;
pub mod resources;
pub mod review;
pub mod runtime;
pub mod scheduling;
pub mod server;
pub mod sessions;
pub mod settings;
pub mod terminal;
pub mod updates;
pub mod usage;
pub mod views;

use crate::types::EnvironmentId;
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResourceKey {
    pub environment: EnvironmentId,
    pub resource: String,
    pub arguments: String,
}
