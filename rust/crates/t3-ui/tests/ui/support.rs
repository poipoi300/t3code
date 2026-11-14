//! Starting the app headless at a given window size. Shared; changed only by the integrator.

use nsh_core::prelude::{px, size};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

pub const PHONE: (f32, f32) = (390.0, 844.0);
pub const DESKTOP: (f32, f32) = (1280.0, 800.0);

/// Launched by an app link carrying the demo switch, so the app runs `DemoBackend`. A native
/// launch link reaches the app only in a scheme it claims (`Root::url_schemes`).
pub const DEMO_LINK: &str = "t3code://pair?backend=demo";

/// Headless windows start as an iOS phone; `DESKTOP` is a Linux desktop window, which has a mouse.
fn config(window: (f32, f32)) -> EngineConfig {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<t3_ui::App>().url_schemes(["t3code"]))
    };
    if window == DESKTOP {
        config.environment.os = nsh_core::platform::Os::Linux;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    config
}

/// The app on the demo backend.
pub fn start(window: (f32, f32)) -> Headless {
    Headless::start_with_link(config(window), DEMO_LINK).expect("the engine starts")
}

/// The app as a plain launch starts it: the real client. Headless keeps no app data.
pub fn start_without_switch(window: (f32, f32)) -> Headless {
    Headless::start(config(window)).expect("the engine starts")
}

/// The app as the web runtime runs it, at an address.
pub fn start_on_web(window: (f32, f32), address: &str) -> Headless {
    Headless::start_at_page(config(window), address).expect("the engine starts on a page")
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
