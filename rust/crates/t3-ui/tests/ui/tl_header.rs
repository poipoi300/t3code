//! Header controls run against the real demo projection and PNL-HOST's client reducer.

use std::rc::Rc;

use nsh_core::element::{SignalEquals, signal_with};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::types::EnvironmentId;
use t3_client::view_logic::shell::terminal_state::TerminalStore;
use t3_ui::runtime::demo::demo_thread;
use t3_ui::screens::panel::{PanelHost, PanelStore};
use t3_ui::screens::thread::header::{HeaderLayout, TerminalControl};

use crate::support::{DEMO_LINK, DESKTOP, PHONE, pair, start};

const FIRST: &str = "role:button[Build the shared Rust UI slice, t3code]";
const DETAILS: &str = "role:button[Toggle thread details panel]";
const TERMINAL: &str = "role:button[Toggle terminal drawer]";
const WORKSPACE: &str = "role:button[Toggle right panel]";

#[component]
fn Workspace() -> impl IntoElement {
    let host = PanelHost::new(PanelStore::default(), |_| {});
    provide_context(host);
    let key = stored(demo_thread(&EnvironmentId("demo-demo.t3.codes".into())));
    let state = host.panel(key.get());
    let inline = signal(true);
    let available = signal(true);
    let terminal = signal_with(TerminalStore::default(), SignalEquals::Never);
    provide_context(HeaderLayout {
        details_inline: Prop::bind(move || inline.get()),
        terminal: Some(TerminalControl {
            available: Prop::bind(move || available.get()),
            open: Prop::bind(move || {
                terminal.with(|store| {
                    store
                        .by_thread
                        .get(&key.get())
                        .is_some_and(|s| s.terminal_open)
                })
            }),
            on_toggle: Rc::new(move || {
                terminal.update(|store| {
                    let open = store
                        .by_thread
                        .get(&key.get())
                        .is_some_and(|s| s.terminal_open);
                    store.set_open(&key.get(), !open);
                })
            }),
        }),
    });
    view()
        .flex_1()
        .child(t3_ui::app::app())
        .child(text(move || {
            format!(
                "panels:{}:{}:{}",
                host.thread_panel_open(&key.get(), inline.get()),
                terminal.with(|store| store
                    .by_thread
                    .get(&key.get())
                    .is_some_and(|s| s.terminal_open)),
                state.with(|state| state.is_open)
            )
        }))
        .child(
            nsh_base::button()
                .accessibility_label("Use popover")
                .on_press(move || inline.set(false)),
        )
        .child(
            nsh_base::button()
                .accessibility_label("Disconnect terminal")
                .on_press(move || available.set(false)),
        )
}

fn workspace(window: (f32, f32)) -> Headless {
    let config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<Workspace>().url_schemes(["t3code"]))
    };
    let mut app = Headless::start_with_link(config, DEMO_LINK).expect("workspace starts");
    pair(&mut app);
    app.press(FIRST).expect("open seeded thread");
    app.wait_for("I added clamp next to sum and the tests pass.")
        .expect("thread projection");
    app
}

#[test]
fn the_actual_app_keeps_missing_services_inert_at_both_widths() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window);
        pair(&mut app);
        app.press(FIRST).expect("open seeded thread");
        app.wait_for(DETAILS).expect("header controls");
        for target in [
            DETAILS,
            TERMINAL,
            WORKSPACE,
            "role:button[New thread in t3code]",
        ] {
            assert!(
                app.screen()
                    .accessibility_state(target)
                    .expect("control")
                    .disabled,
                "{target}"
            );
        }
    }
}

#[test]
fn header_toggles_change_independent_client_targets_and_reverse_them() {
    for window in [DESKTOP, PHONE] {
        let mut app = workspace(window);
        app.wait_for("text:panels:true:false:false")
            .expect("client defaults");
        assert_eq!(
            app.screen()
                .accessibility_state(DETAILS)
                .expect("details toggle")
                .pressed,
            Some(Checked::True)
        );
        assert_eq!(
            app.screen()
                .accessibility_state(WORKSPACE)
                .expect("workspace toggle")
                .pressed,
            Some(Checked::False)
        );
        app.press(DETAILS).expect("close details");
        app.wait_for("text:panels:false:false:false")
            .expect("only details changes");
        app.press(TERMINAL).expect("open drawer");
        app.wait_for("text:panels:false:true:false")
            .expect("only terminal changes");
        app.press(WORKSPACE).expect("open workspace");
        app.wait_for("text:panels:false:true:true")
            .expect("independent workspace");
        assert_eq!(
            app.screen()
                .accessibility_state(DETAILS)
                .expect("details toggle")
                .pressed,
            Some(Checked::False)
        );
        assert_eq!(
            app.screen()
                .accessibility_state(TERMINAL)
                .expect("terminal toggle")
                .pressed,
            Some(Checked::True)
        );
        assert_eq!(
            app.screen()
                .accessibility_state(WORKSPACE)
                .expect("workspace toggle")
                .pressed,
            Some(Checked::True)
        );
        app.press(DETAILS).expect("reopen details");
        app.press(TERMINAL).expect("close drawer");
        app.press(WORKSPACE).expect("close workspace");
        app.wait_for("text:panels:true:false:false")
            .expect("all reversed");
        app.press("role:button[Use popover]")
            .expect("canvas uses popover");
        app.wait_for("text:panels:false:false:false")
            .expect("separate popover default");
        app.press(DETAILS).expect("open popover");
        app.wait_for("text:panels:true:false:false")
            .expect("popover state");
        app.press("role:button[Disconnect terminal]")
            .expect("capability revoked");
        assert!(
            app.screen()
                .accessibility_state(TERMINAL)
                .expect("drawer control")
                .disabled
        );
        let _ = app.press(TERMINAL);
        assert!(app.screen().shows("panels:true:false:false"));
    }
}
