//! Project commands and pure workspace policy shared by all clients.
#[path = "project_ops/actions.rs"]
pub mod actions;
#[path = "project_ops/commands.rs"]
pub mod commands;
#[path = "project_ops/editor.rs"]
pub mod editor;
#[path = "project_ops/entities.rs"]
pub mod entities;
#[path = "project_ops/icons.rs"]
pub mod icons;
#[path = "project_ops/paths.rs"]
pub mod paths;
#[path = "project_ops/scripts.rs"]
pub mod scripts;
#[path = "project_ops/source_control.rs"]
pub mod source_control;
#[path = "project_ops/subscriptions.rs"]
pub mod subscriptions;
#[path = "project_ops/wsl.rs"]
pub mod wsl;

pub use commands::*;
pub use entities::*;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[path = "project_ops/tests.rs"]
mod tests;
