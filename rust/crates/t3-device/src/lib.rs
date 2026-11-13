//! Native device hosts, sessions and hub proxy. Does not target wasm32-unknown-unknown.

pub mod actions;
pub mod agent;
pub mod cloud;
pub mod error;
pub mod host;
pub mod hub;
pub mod io;
pub mod local;
#[cfg(feature = "mcp")]
pub mod mcp;
pub mod mjpeg;
pub mod project;
pub mod proxy;
pub mod service;
pub mod ssh;
pub mod tools;

pub use error::{Error, Result};
pub use service::DeviceService;
pub use t3_contracts::device as contracts;

pub const DEVICE_HUB_ROUTE_PREFIX: &str = "/api/device-hub";
