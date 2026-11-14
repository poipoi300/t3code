//! Bindings of the client's services to the UI packages' service traits, one file per group
//! (briefs/client-services-package.md). Each builds its handles from `Backend::client()` and only
//! delegates; rules belong in the client.

pub mod composer;
pub mod connection;
pub mod diff;
pub mod files;
pub mod git_panel;
pub mod navigation;
pub mod settings;
pub mod terminal;
pub mod thread_ops;
pub mod shell;
