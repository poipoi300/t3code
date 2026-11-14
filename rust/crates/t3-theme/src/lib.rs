//! T3 Code design system as plain data and pure functions.
//!
//! Must compile to `wasm32-unknown-unknown`: no I/O, threads or clocks.

pub mod color;
pub mod css;
pub mod fonts;
pub mod import;
pub mod motion;
pub mod palette;
pub mod resolved;
pub mod syntax;
pub mod tokens;
pub mod ui_data;
