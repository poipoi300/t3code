//! Shared T3 Code client UI. Must compile for `wasm32-unknown-unknown` on every platform.
//!
//! Every screen, component, token and piece of UI state lives here. Shells register
//! [`registries`] and mount [`App`]; they hold no feature code. See `ui-plan.md` in the
//! rewrite's notes for the architecture and the work packages.

pub mod app;
pub mod host;
pub mod i18n;
pub mod keys;
pub mod runtime;
pub mod screens;
pub mod state;
pub mod theme;
pub mod ui;

pub use app::App;
pub use nsh_core::prelude::{IntoElement, Registry};
use nsh_core::prelude::{assets, native_module};

#[native_module("t3/ui")]
fn native(registry: &mut Registry) {
    registry.assets(assets!());
    registry.register::<App>();
}

pub fn registries() -> Vec<Registry> {
    vec![nsh_base::registry(), registry()]
}
