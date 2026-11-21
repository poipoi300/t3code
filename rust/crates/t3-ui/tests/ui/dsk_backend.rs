//! DSK-BACKEND: the local server's startup state and the "could not start" recovery window.
//! The desktop shell supplies a `LocalBackendHandle`; here a key script plays its states.
use crate::support::DESKTOP;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use std::rc::Rc;
use t3_client::view_logic::shell::startup_recovery::{RecoveryOffer, StartupFailure};
use t3_ui::screens::local_backend::{
    FailedStartup, LocalBackendHandle, LocalBackendStatus, RecoveryAction,
};
use t3_ui::theme;

#[derive(Clone, PartialEq, Props)]
struct BackendProps {
    offer: i32,
}

fn failed(offer: i32, output: &str) -> LocalBackendStatus {
    LocalBackendStatus::Failed(Box::new(FailedStartup {
        failure: StartupFailure::exited("exit code 1", offer == 3, output),
        offer: match offer {
            0 => RecoveryOffer::Update {
                version: "1.3.0".into(),
            },
            1 => RecoveryOffer::Reinstall {
                version: "1.1.0".into(),
            },
            2 => RecoveryOffer::Releases,
            _ => RecoveryOffer::ManagedPolicy,
        },
        app_version: "1.2.0".into(),
        log_dir: "/home/user/.t3/logs".into(),
    }))
}

#[component]
fn Backend(props: BackendProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let status = signal(LocalBackendStatus::Starting);
    let acted = signal(String::new());
    let offer = props.offer;
    on_key("ctrl+shift+r", move |_| {
        status.set(LocalBackendStatus::Restarting { attempt: 2 })
    });
    on_key("ctrl+shift+f", move |_| {
        status.set(failed(offer, "listen EADDRINUSE: address already in use"))
    });
    on_key("ctrl+shift+e", move |_| status.set(failed(offer, "   ")));
    on_key("ctrl+shift+k", move |_| {
        status.set(LocalBackendStatus::Ready)
    });
    provide_context(LocalBackendHandle {
        status,
        act: Rc::new(move |action: RecoveryAction| acted.set(format!("acted {action:?}"))),
    });
    view()
        .w_full()
        .h_full()
        .child(text(move || {
            SharedString::from(format!("main {}", acted.get()))
        }))
        .child(untracked(t3_ui::screens::local_backend::overlay))
}

fn start(offer: i32) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(DESKTOP.0), px(DESKTOP.1)),
        ..EngineConfig::new(Root::new::<Backend>(move || BackendProps { offer }))
    };
    config.environment.os = nsh_core::platform::Os::Linux;
    config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    Headless::start(config).expect("headless backend status")
}

#[test]
fn startup_states_then_the_recovery_window_and_back_to_ready() {
    let mut app = start(2);
    app.wait_for("Starting T3 Code…").expect("first launch");
    app.press_key("ctrl+shift+r").expect("exit before ready");
    app.wait_for("The server stopped. Restarting (attempt 2)…")
        .expect("restart scheduled, failure not yet reported");
    assert_eq!(app.screen().count("T3 Code could not start").unwrap(), 0);
    app.press_key("ctrl+shift+f").expect("third restart");
    app.wait_for("T3 Code could not start").expect("reported");
    app.wait_for("The server could not start (exit code 1).")
        .expect("reason");
    app.wait_for("listen EADDRINUSE: address already in use")
        .expect("the server's last output");
    app.wait_for("Version 1.2.0. T3 Code tries again in the background. Logs: /home/user/.t3/logs")
        .expect("footer");
    // The server answered on a later retry: the window goes away.
    app.press_key("ctrl+shift+k").expect("ready");
    app.wait_for_gone("T3 Code could not start", std::time::Duration::from_secs(2))
        .expect("window closes");
    assert_eq!(app.screen().count("Starting T3 Code…").unwrap(), 0);
}

#[test]
fn each_offer_has_its_sentence_and_only_the_actionable_ones_a_button() {
    for (offer, sentence, button) in [
        (
            0,
            "Version 1.3.0 is available. It can contain a fix.",
            Some("Update to 1.3.0"),
        ),
        (
            1,
            "Version 1.1.0 started correctly on this computer. Download it and install it to go back.",
            Some("Download 1.1.0"),
        ),
        (
            2,
            "Get a different version from the releases page.",
            Some("Open releases"),
        ),
        (
            3,
            "Your organization's managed policy stopped T3 Code from starting. Send the details below to your IT administrator.",
            None,
        ),
    ] {
        let mut app = start(offer);
        app.wait_for("Starting T3 Code…").expect("mounted");
        app.press_key("ctrl+shift+f").expect("failure");
        app.wait_for(sentence).expect("offer sentence");
        if let Some(button) = button {
            app.wait_for(&format!("role:button[{button}]"))
                .expect("offer button");
        } else {
            assert_eq!(
                app.screen().count("role:button[Update to 1.3.0]").unwrap(),
                0
            );
            assert_eq!(app.screen().count("role:button[Open releases]").unwrap(), 0);
        }
        app.wait_for("role:button[Copy details]").expect("always");
        app.wait_for("role:button[Quit]").expect("always");
    }
}

#[component]
fn Bare() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let pressed = signal(false);
    view()
        .w_full()
        .h_full()
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from("Under the overlay")))
                .on_press(move || pressed.set(true))
                .child(text("Under the overlay")),
        )
        .child(text(move || {
            SharedString::from(if pressed.get() { "pressed" } else { "idle" })
        }))
        .child(untracked(t3_ui::screens::local_backend::overlay))
}

#[test]
fn without_a_handle_the_overlay_is_inert() {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(DESKTOP.0), px(DESKTOP.1)),
        ..EngineConfig::new(Root::of::<Bare>())
    };
    config.environment.os = nsh_core::platform::Os::Linux;
    config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    let mut app = Headless::start(config).expect("headless bare");
    app.wait_for("idle").expect("mounted");
    app.press("role:button[Under the overlay]")
        .expect("reachable");
    app.wait_for("pressed")
        .expect("the page underneath is interactive");
    assert_eq!(app.screen().count("Starting T3 Code…").unwrap(), 0);
    assert_eq!(app.screen().count("T3 Code could not start").unwrap(), 0);
}

#[test]
fn buttons_reach_the_shell_and_empty_output_says_so() {
    let mut app = start(0);
    app.wait_for("Starting T3 Code…").expect("mounted");
    app.press_key("ctrl+shift+e")
        .expect("failure without output");
    app.wait_for("The server wrote no output.")
        .expect("empty output");
    app.press("role:button[Update to 1.3.0]").expect("update");
    app.wait_for("main acted Update")
        .expect("shell got the action");
    app.press("role:button[Copy details]").expect("copy");
    app.wait_for("main acted Copy")
        .expect("copy reaches the shell");
    app.press("role:button[Quit]").expect("quit");
    app.wait_for("main acted Quit")
        .expect("quit reaches the shell");
}
