//! Starting the app headless at a given window size. Shared; changed only by the integrator.

use nsh_core::prelude::{px, size};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

pub const PHONE: (f32, f32) = (390.0, 844.0);
pub const DESKTOP: (f32, f32) = (1280.0, 800.0);

pub fn start(window: (f32, f32)) -> Headless {
    Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<t3_ui::App>())
    })
    .expect("the engine starts")
}

/// The app as the web runtime runs it, at an address.
pub fn start_on_web(window: (f32, f32), address: &str) -> Headless {
    Headless::start_at_page(
        EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::of::<t3_ui::App>())
        },
        address,
    )
    .expect("the engine starts on a page")
}

/// Pairs with the demo server through the real pairing screen.
pub fn pair(app: &mut Headless) {
    app.wait_for("Connect to T3 Code").expect("pairing screen");
    app.press("role:button[Fill in the demo server]")
        .expect("fill");
    app.press("role:button[Connect]").expect("connect");
    app.wait_for("Build the shared Rust UI slice")
        .expect("threads listed");
}
