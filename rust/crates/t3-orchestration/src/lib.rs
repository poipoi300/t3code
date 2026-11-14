//! Native orchestration server core. This crate does not target wasm32-unknown-unknown.
pub mod adapters;
pub mod api;
pub mod composition;
pub mod decision;
pub mod effects;
pub mod error;
pub mod native_history;
pub mod ports;
pub mod projections;
pub mod provider;
pub mod runtime;
pub mod stores;
pub mod testing;
pub mod transport;
