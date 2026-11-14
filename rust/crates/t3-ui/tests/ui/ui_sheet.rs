//! Caller-level sheet lifecycle, including a draft created by its actual content factory.
use crate::support::{DESKTOP, PHONE};
use nsh_core::{
    platform::{Idiom, Os},
    prelude::*,
};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::{i18n::Text, theme, ui::sheet::*};

#[derive(Clone, PartialEq, Props)]
struct FixtureProps {
    retained: bool,
    guarded: bool,
    side: u32,
    duration: u32,
}

#[component]
fn Fixture(props: FixtureProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let open = signal(false);
    let initial = element_ref();
    sheet(open, "Settings panel", move |state| {
        if props.guarded {
            state.on_open_change(move |next| {
                if next {
                    open.set(true);
                }
            });
        }
        view()
            .w_full()
            .h_full()
            .child(sheet_trigger(
                state,
                "Open panel",
                text(String::from("Open panel")),
            ))
            .child(text(move || format!("open {}", open.get())))
            .child(sheet_popup(
                state,
                PopupOptions {
                    keep_mounted: props.retained,
                    transition_duration_ms: props.duration,
                    initial_focus: Some(initial),
                    side: match props.side {
                        0 => Side::Right,
                        1 => Side::Left,
                        2 => Side::Top,
                        _ => Side::Bottom,
                    },
                    ..Default::default()
                },
                move || {
                    let draft = signal(String::new());
                    view()
                        .child(sheet_header(
                            false,
                            view()
                                .child(sheet_title(state, || Text::from("Settings panel")))
                                .child(sheet_description(state, || Text::from("Edit this value"))),
                        ))
                        .child(
                            nsh_base::input()
                                .element_ref(&initial)
                                .label("Draft")
                                .value(move || draft.get())
                                .on_change(move |value: &str| draft.set(value.to_owned())),
                        )
                        .child(text(move || format!("draft {}", draft.get())))
                },
            ))
    })
}

fn start(window: (f32, f32), retained: bool, guarded: bool, side: u32) -> Headless {
    start_timed(window, retained, guarded, side, 0)
}

fn start_timed(
    window: (f32, f32),
    retained: bool,
    guarded: bool,
    side: u32,
    duration: u32,
) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        clock: nsh_engine::Clock::manual(std::time::UNIX_EPOCH),
        ..EngineConfig::new(Root::new::<Fixture>(move || FixtureProps {
            retained,
            guarded,
            side,
            duration,
        }))
    };
    config.environment.os = Os::Web;
    config.environment.idiom = if window == PHONE {
        Idiom::Phone
    } else {
        Idiom::Desktop
    };
    let mut app = Headless::start(config).expect("sheet fixture");
    app.wait_for("Open panel").expect("trigger");
    app
}

#[test]
fn the_default_exit_keeps_content_mounted_and_inert_until_native_motion_finishes() {
    let mut app = start_timed(DESKTOP, false, false, 0, 200);
    app.press("role:button[Open panel]").expect("open");
    app.fast_forward(std::time::Duration::from_millis(200))
        .expect("settle entry on manual clock");
    app.type_text("label:Draft", "retained during exit")
        .expect("draft");
    // Headless::press settles animations; dispatch the actual backdrop event to inspect exit.
    let backdrop = app.screen().id("id:sheet-backdrop").expect("backdrop");
    app.engine().control().dispatch_event(
        backdrop,
        nsh_core::EventKind::Press,
        nsh_core::EventPayload::None,
    );
    app.wait_for("open false").expect("exit requested");
    assert!(
        app.screen()
            .layout_nodes()
            .iter()
            .any(|node| node.test_id.as_deref() == Some("sheet-popup"))
    );
    assert!(
        !app.screen()
            .enabled("role:button[Close]")
            .expect("closing content")
    );
    app.fast_forward(std::time::Duration::from_millis(200))
        .expect("complete native exit");
    assert!(!app.screen().shows("Edit this value"));
}

#[test]
fn outside_press_escape_close_button_and_focus_return_belong_to_the_sheet() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, false, false, 0);
        app.press_key("tab").expect("focus trigger");
        app.press_key("enter").expect("open");
        app.wait_for("Edit this value").expect("popup");
        app.press_key("a").expect("initial field focus");
        app.wait_for("draft a").expect("typed in actual field");
        app.press_key("escape").expect("escape in editor");
        app.wait_for("open false").expect("closed");
        app.press_key("enter").expect("restored trigger focus");
        app.wait_for("open true").expect("reopened");
        app.press_at("id:sheet-backdrop", point(px(2.0), px(2.0)))
            .expect("outside press");
        app.wait_for("open false").expect("outside dismissed");
        app.press("role:button[Open panel]").expect("open again");
        app.press("role:button[Close]").expect("close button");
        app.wait_for("open false").expect("button dismissed");
    }
}

#[test]
fn keep_mounted_preserves_draft_and_default_lifetime_recreates_it() {
    for retained in [false, true] {
        let mut app = start(DESKTOP, retained, false, 3);
        app.press("role:button[Open panel]").expect("open");
        app.type_text("label:Draft", "Keep my draft").expect("type");
        app.press("role:button[Close]").expect("close");
        app.wait_for("open false").expect("closed");
        assert!(
            !app.screen().shows("Edit this value"),
            "hidden content must leave accessibility tree"
        );
        app.press("role:button[Open panel]").expect("reopen");
        app.wait_for("Edit this value").expect("mounted");
        assert_eq!(app.screen().shows("draft Keep my draft"), retained);
    }
}

#[test]
fn controlled_dismissal_can_be_declined_for_unsaved_changes() {
    let mut app = start(DESKTOP, true, true, 1);
    app.press("role:button[Open panel]").expect("open");
    app.type_text("label:Draft", "unsaved").expect("draft");
    app.press_key("escape").expect("cancel request");
    assert!(app.screen().shows("open true"));
    app.press("role:button[Close]").expect("close request");
    assert!(app.screen().shows("draft unsaved"));
    app.press_at("id:sheet-backdrop", point(px(1278.0), px(2.0)))
        .expect("outside request");
    assert!(app.screen().shows("open true"));
}
