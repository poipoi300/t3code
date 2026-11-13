//! Shared client logic for native and `wasm32-unknown-unknown`.
//! The shell drives local futures and supplies all I/O; the UI adapts store revisions to signals.

pub mod archive;
pub mod authorization;
pub mod connection;
pub mod device;
pub mod environment;
pub mod error;
pub mod mcp_apps;
pub mod operations;
pub mod optimistic;
pub mod permissions;
pub mod persistence;
pub mod platform;
pub mod preview;
pub mod relay;
pub mod rpc;
pub mod services;
pub mod state;
pub mod store;
pub mod testing;
pub mod thread_views;
pub mod time;
pub mod types;
pub mod view_logic;
pub mod voice;
pub mod work_log;
pub mod workspace;

pub use error::ClientError;
pub use store::ClientStore;
