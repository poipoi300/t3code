//! UI-POPOVER: isolated primitive roots; no app route mounts these primitives yet.

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::i18n::t;
use t3_ui::ui::{popover as p, tooltip as tt};

use crate::support::{DESKTOP, PHONE};

fn start(root: Root, window: (f32, f32), touch: bool) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        clock: nsh_engine::Clock::manual(std::time::UNIX_EPOCH),
        ..EngineConfig::new(root)
    };
    config.environment.os = if touch {
        nsh_core::platform::Os::Android
    } else {
        nsh_core::platform::Os::Linux
    };
    config.environment.idiom = if touch {
        nsh_core::platform::Idiom::Phone
    } else {
        nsh_core::platform::Idiom::Desktop
    };
    config.environment.reduce_motion = true;
    let mut app = Headless::start(config).expect("primitive root starts");
    app.first_render().expect("first render");
    app
}

#[derive(Clone, PartialEq, Props)]
struct PopoverFixtureProps {
    retained: bool,
    disabled: bool,
}

#[component]
fn PopoverFixture(props: PopoverFixtureProps) -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let handle = p::popover_create_handle();
    let presses = signal(0);
    view()
        .w_full()
        .h_full()
        .test_id("popover-fixture")
        .p(px(40.0))
        .child(p::popover(
            handle,
            p::PopoverOptions {
                keep_mounted: props.retained,
                touch_backdrop: true,
                width: p::Size::Md,
                ..Default::default()
            },
            p::popover_trigger(
                handle,
                "Open details",
                move || props.disabled,
                text(t("Details")),
            ),
            move || {
                let draft = signal(String::new());
                view()
                    .child(p::popover_title("Details"))
                    .child(
                        nsh_base::input()
                            .label(t("Draft"))
                            .value(move || draft.get())
                            .on_change(move |value: &str| draft.set(value.to_owned())),
                    )
                    .child(p::popover_close(handle, "Close details", text(t("Close"))))
                    .into_element()
            },
        ))
        .child(
            nsh_base::button()
                .accessibility_label(Some(t("Outside action").into()))
                .on_press(move || presses.update(|count| *count += 1))
                .mt(px(400.0))
                .child(text(move || {
                    SharedString::from(format!("Outside presses: {}", presses.get()))
                })),
        )
}

fn popover_root(retained: bool, disabled: bool) -> Root {
    Root::new::<PopoverFixture>(move || PopoverFixtureProps { retained, disabled })
}

#[test]
fn popover_close_and_escape_restore_the_trigger_and_only_retained_forms_keep_their_draft() {
    for window in [PHONE, DESKTOP] {
        for retained in [false, true] {
            let mut app = start(popover_root(retained, false), window, false);
            assert!(!app.screen().matches("label:Draft").expect("draft query"));
            app.press("role:button[Open details]").expect("open");
            app.type_text("label:Draft", "Unsubmitted work")
                .expect("type draft");
            app.press("role:button[Close details]").expect("close");
            assert!(!app.screen().matches("label:Draft").expect("hidden draft"));
            app.press_key("enter").expect("trigger regained focus");
            app.wait_for("label:Draft").expect("reopened from trigger");
            assert_eq!(
                app.screen().input_text("label:Draft").expect("input"),
                Some(if retained { "Unsubmitted work" } else { "" })
            );
            app.press_key("escape").expect("escape");
            assert!(
                !app.screen()
                    .matches("label:Draft")
                    .expect("closed by Escape")
            );
        }
    }
}

#[test]
fn a_disabled_popover_cannot_be_opened() {
    for retained in [false, true] {
        let mut app = start(popover_root(retained, true), DESKTOP, false);
        assert!(
            !app.screen()
                .enabled("role:button[Open details]")
                .expect("disabled state")
        );
        let _ = app.press("role:button[Open details]");
        assert!(!app.screen().matches("label:Draft").expect("stays closed"));
    }
}

#[test]
fn a_touch_backdrop_consumes_the_outside_tap_before_an_action_beneath_it_can_run() {
    for retained in [false, true] {
        let mut app = start(popover_root(retained, false), PHONE, true);
        app.press("role:button[Open details]").expect("open");
        let outside = app
            .screen()
            .id("role:button[Outside action]")
            .expect("outside id");
        let root = app.screen().id("id:popover-fixture").expect("fixture root");
        let nodes = app.screen().layout_nodes();
        let point = nodes
            .iter()
            .find(|n| n.id == outside)
            .expect("outside bounds")
            .bounds
            .center();
        let backdrop = nodes
            .iter()
            .find(|n| {
                n.parent == nsh_core::ROOT_ID
                    && n.id != root
                    && n.bounds.size == size(px(PHONE.0), px(PHONE.1))
                    && n.bounds.contains(&point)
            })
            .expect("backdrop covers the outside control");
        // The headless driver does not route a covered control selector to a pressable
        // backdrop. Tap the rendered backdrop at that control's actual coordinates.
        app.press_at(
            &format!("view:{}", backdrop.id),
            point - backdrop.bounds.origin,
        )
        .expect("tap outside");
        assert!(
            !app.screen().matches("label:Draft").expect("dismissed"),
            "retained: {retained}"
        );
        assert!(app.screen().shows("Outside presses: 0"));
        app.press("role:button[Outside action]")
            .expect("next tap runs action");
        assert!(app.screen().shows("Outside presses: 1"));
    }
}

#[derive(Clone, PartialEq, Props)]
struct TooltipFixtureProps {
    delay: f64,
}

#[component]
fn TooltipFixture(props: TooltipFixtureProps) -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let target = element_ref();
    view()
        .w_full()
        .h_full()
        .p(px(40.0))
        .child(
            view()
                .h(px(128.0))
                .child(tt::tooltip_provider(props.delay, move || {
                    tt::tooltip_scroll_dismiss_area(target, move || {
                        let handle = tt::tooltip_create_handle();
                        scroll()
                            .element_ref(&target)
                            .test_id("tooltip-scroll-dismiss-area")
                            .h(px(128.0))
                            .child(view().h(px(500.0)).child(tt::tooltip_trigger(
                                handle,
                                "Timeline action",
                                || false,
                                tt::tooltip(handle, Default::default(), text(t("Description"))),
                                text(t("Timeline action")),
                            )))
                            .into_element()
                    })
                    .into_element()
                })),
        )
        .child(view().test_id("tooltip-away").h(px(100.0)))
}

fn scroll_without_changing_focus(app: &mut Headless) {
    let area = app
        .screen()
        .id("id:tooltip-scroll-dismiss-area")
        .expect("scroll area");
    // The web test sets scrollTop without changing input modality or keyboard focus.
    app.engine()
        .control()
        .scroll(area, point(px(0.0), px(20.0)), 0);
    assert!(app.settle(), "programmatic scroll finishes");
}

#[test]
fn scrolling_dismisses_a_mouse_focused_hovered_tooltip() {
    let mut app = start(Root::of::<TooltipFixture>(), DESKTOP, false);
    app.press("role:button[Timeline action]")
        .expect("mouse focus");
    app.hover("id:tooltip-away").expect("leave pressed trigger");
    app.hover("role:button[Timeline action]").expect("hover");
    app.wait_for("Description").expect("hint opens");
    scroll_without_changing_focus(&mut app);
    assert!(!app.screen().shows("Description"));
}

#[test]
fn scrolling_keeps_a_keyboard_focused_hovered_tooltip_open() {
    for hover_first in [false, true] {
        let mut app = start(Root::of::<TooltipFixture>(), DESKTOP, false);
        if hover_first {
            app.hover("role:button[Timeline action]")
                .expect("hover before focus");
            app.wait_for("Description").expect("hover hint opens");
        }
        app.press_key("tab").expect("keyboard focus");
        app.wait_for("Description").expect("focus hint opens");
        app.hover("role:button[Timeline action]")
            .expect("hover focused trigger");
        scroll_without_changing_focus(&mut app);
        assert!(
            app.screen().shows("Description"),
            "hover first: {hover_first}"
        );
        app.press_key("escape").expect("dismiss hint");
        assert!(!app.screen().shows("Description"));
    }
}

#[test]
fn scrolling_also_cancels_a_pending_hover_open_until_the_trigger_is_hovered_again() {
    let mut app = start(
        Root::new::<TooltipFixture>(|| TooltipFixtureProps { delay: 600.0 }),
        DESKTOP,
        false,
    );
    app.hover("role:button[Timeline action]")
        .expect("start delayed hover");
    assert!(!app.screen().shows("Description"));
    scroll_without_changing_focus(&mut app);
    app.fast_forward(std::time::Duration::from_millis(600))
        .expect("advance the manual hint clock");
    assert!(!app.screen().shows("Description"));
    app.hover("id:tooltip-away").expect("leave");
    app.hover("role:button[Timeline action]")
        .expect("hover again");
    app.fast_forward(std::time::Duration::from_millis(600))
        .expect("advance the new hover delay");
    app.wait_for("Description")
        .expect("new hover opens normally");
}

#[component]
fn PreviewFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    view().w_full().h_full().p(px(40.0)).child(p::preview_card(
        p::PreviewCardOptions {
            open_delay: 0.0,
            close_delay: 0.0,
            ..Default::default()
        },
        p::preview_card_trigger(
            "Preview thread",
            None,
            nsh_base::LinkOpenWith::App,
            |_| {},
            text(t("Thread")),
        ),
        || text(t("Thread preview")).into_element(),
    ))
}

#[test]
fn preview_cards_open_on_hover_or_touch_and_dismiss_with_escape_or_back() {
    for (window, touch) in [(DESKTOP, false), (PHONE, true)] {
        let mut app = start(Root::of::<PreviewFixture>(), window, touch);
        assert!(!app.screen().shows("Thread preview"));
        if touch {
            app.press("role:link[Preview thread]").expect("tap preview");
        } else {
            app.hover("role:link[Preview thread]")
                .expect("hover preview");
        }
        app.wait_for("Thread preview").expect("preview shown");
        if touch {
            app.back().expect("back");
        } else {
            app.press_key("escape").expect("escape");
        }
        assert!(!app.screen().shows("Thread preview"));
    }
}
