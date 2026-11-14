//! UI-WIZARD: a wizard dialog, a standalone page and a discovery list as their callers mount them.
use std::rc::Rc;

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::{
    i18n::Text,
    theme,
    ui::{
        dialog::{PopupOptions, dialog},
        wizard::*,
    },
};

#[derive(Clone, PartialEq, Props)]
struct WizardFixtureProps {
    wide: bool,
}

/// LinkRequestCard's shape: two steps with a summary, a held panel while busy, a leading footer.
#[component]
fn WizardFixture(props: WizardFixtureProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let open = signal(true);
    let step = signal(0usize);
    let busy = signal(false);
    let tall = signal(false);
    let pressed = signal(Vec::<usize>::new());
    view()
        .w_full()
        .h_full()
        .child(
            nsh_base::button()
                .child(text("Toggle busy"))
                .on_press(move || busy.update(|b| *b = !*b)),
        )
        .child(
            nsh_base::button()
                .child(text("Toggle tall"))
                .on_press(move || tall.update(|t| *t = !*t)),
        )
        .child(text(move || format!("pressed {:?}", pressed.get())))
        .child(dialog(open, move |state| {
            wizard_popup(
                state,
                if props.wide {
                    WizardSize::Wide
                } else {
                    WizardSize::Default
                },
                PopupOptions::default(),
                move || {
                    vec![
                        wizard_header(
                            state,
                            || Text::from("Link this thread"),
                            Some(Rc::new(|| Text::from("Choose a machine, then access."))),
                            Some(text(String::from("Identity card")).into_element()),
                            Some(
                                wizard_steps(
                                    || vec![Text::from("Machine"), Text::from("Access")],
                                    move || step.get(),
                                    WizardStepsOptions {
                                        summaries: Some(
                                            (move || {
                                                vec![Some(SharedString::from("Studio")), None]
                                            })
                                            .into_reactive(),
                                        ),
                                        show_summaries: true,
                                        is_step_disabled: Some(Rc::new(move |index| {
                                            let _ = index;
                                            busy.get()
                                        })),
                                        on_step_change: Some(Rc::new(move |index| {
                                            pressed.update(|p| p.push(index));
                                            step.set(index);
                                        })),
                                    },
                                )
                                .into_element(),
                            ),
                        )
                        .into_element(),
                        wizard_panel(
                            state,
                            move || busy.get(),
                            false,
                            view().child(dynamic(move || {
                                if tall.get() {
                                    view().h(px(300.0)).child(text(String::from("Tall step")))
                                } else {
                                    view().h(px(80.0)).child(text(String::from("Short step")))
                                }
                            })),
                        )
                        .into_element(),
                        wizard_footer(
                            state,
                            Some(text(String::from("Leading note")).into_element()),
                            nsh_base::button()
                                .child(text("Continue"))
                                .on_press(move || step.set(1)),
                        )
                        .into_element(),
                    ]
                },
            )
        }))
}

fn start_wizard(window: (f32, f32), wide: bool) -> Headless {
    Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<WizardFixture>(move || WizardFixtureProps {
            wide,
        }))
    })
    .expect("the engine starts")
}

fn bounds(app: &Headless, test_id: &str) -> Bounds<Points> {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some(test_id))
        .unwrap_or_else(|| panic!("{test_id} is laid out"))
        .bounds
}

#[test]
fn steps_move_on_press_announce_progress_and_refuse_while_disabled() {
    for window in [PHONE, DESKTOP] {
        let mut app = start_wizard(window, false);
        app.wait_for("role:list[Setup progress]")
            .expect("progress list");
        app.wait_for("label:Machine, step 1")
            .expect("first step, no summary yet");
        assert_eq!(
            app.screen()
                .accessibility_state("id:wizard-step-0")
                .expect("state")
                .current,
            Some(nsh_core::AccessibilityCurrent::Step)
        );
        app.press("label:Access, step 2").expect("go to access");
        app.wait_for("pressed [1]").expect("step change reported");
        // A completed step's name carries its summary, in its label and its visible name.
        app.wait_for("label:Machine, step 1, Studio")
            .expect("summary in label");
        if window == DESKTOP {
            app.wait_for("Machine: Studio").expect("summary shown");
        }
        assert_eq!(
            app.screen()
                .accessibility_state("id:wizard-step-1")
                .expect("state")
                .current,
            Some(nsh_core::AccessibilityCurrent::Step)
        );
        app.press("role:button[Toggle busy]").expect("busy");
        assert!(
            !app.screen()
                .enabled("label:Machine, step 1, Studio")
                .expect("enabled")
        );
        let _ = app.press("label:Machine, step 1, Studio");
        app.settle();
        assert!(app.shows("pressed [1]"), "a disabled step does not change");
    }
}

#[test]
fn identity_replaces_the_visible_title_but_still_names_the_dialog() {
    let mut app = start_wizard(DESKTOP, false);
    app.wait_for("Identity card").expect("identity");
    app.wait_for("Choose a machine, then access.")
        .expect("description");
    assert_eq!(
        app.screen()
            .count("role:dialog[Link this thread]")
            .expect("count"),
        1,
        "the title labels the dialog"
    );
    assert_eq!(
        app.screen().count("text:Link this thread").expect("count"),
        0,
        "the title is not drawn"
    );
}

#[test]
fn the_popup_is_xl_or_three_xl_wide() {
    let mut app = start_wizard(DESKTOP, false);
    app.wait_for("Identity card").expect("open");
    app.settle();
    assert_eq!(bounds(&app, "dialog-popup").size.width.0, 576.0, "max-w-xl");
    let mut wide = start_wizard(DESKTOP, true);
    wide.wait_for("Identity card").expect("open");
    wide.settle();
    assert_eq!(
        bounds(&wide, "dialog-popup").size.width.0,
        768.0,
        "max-w-3xl"
    );
}

#[test]
fn a_held_panel_keeps_its_height_until_released() {
    let mut app = start_wizard(DESKTOP, false);
    app.wait_for("Short step").expect("short");
    app.settle();
    let short = bounds(&app, "animated-height").size.height.0;
    app.press("role:button[Toggle busy]").expect("hold");
    app.press("role:button[Toggle tall]").expect("tall");
    app.wait_for("Tall step").expect("tall content");
    app.settle();
    assert_eq!(bounds(&app, "animated-height").size.height.0, short, "held");
    app.press("role:button[Toggle busy]").expect("release");
    app.settle();
    assert!(
        bounds(&app, "animated-height").size.height.0 >= 300.0,
        "follows its content again"
    );
}

#[test]
fn a_leading_footer_puts_the_note_at_the_start_and_actions_at_the_end() {
    let mut app = start_wizard(DESKTOP, false);
    app.wait_for("Leading note").expect("footer");
    app.settle();
    let popup = bounds(&app, "dialog-popup");
    let nodes = app.screen().layout_nodes();
    let note = app.screen().id("text:Leading note").expect("note");
    let note = nodes
        .iter()
        .find(|node| node.id == note)
        .expect("laid out")
        .bounds;
    let action = app.screen().id("role:button[Continue]").expect("action");
    let action = nodes
        .iter()
        .find(|node| node.id == action)
        .expect("laid out")
        .bounds;
    assert!(
        note.origin.x.0 - popup.origin.x.0 <= 25.0,
        "note at the start (px-6)"
    );
    let right = popup.origin.x.0 + popup.size.width.0;
    assert!(
        right - (action.origin.x.0 + action.size.width.0) <= 25.0,
        "action at the end"
    );
}

#[component]
fn DiscoveryFixture() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let opened = signal(Vec::<&'static str>::new());
    view()
        .child(text(move || format!("opened {:?}", opened.get())))
        .child(standalone_page(
            standalone_page_header(
                Some(Text::from("Agent sign-in")),
                || Text::from("Pair with this environment"),
                || Text::from("Validating the pairing link and preparing your session."),
            )
            .into_iter()
            .chain([discovery_list(vec![
                discovery_list_row(
                    text(String::from("A")).into_element(),
                    Text::content("Laptop"),
                    Text::content("192.168.1.4"),
                    None,
                    false,
                    move || opened.update(|o| o.push("laptop")),
                )
                .into_element(),
                discovery_list_row(
                    text(String::from("B")).into_element(),
                    Text::content("Server"),
                    Text::content("Offline"),
                    None,
                    true,
                    move || opened.update(|o| o.push("server")),
                )
                .into_element(),
            ])
            .into_element()])
            .collect::<Vec<_>>(),
        ))
}

#[test]
fn discovery_rows_press_unless_disabled_on_a_standalone_page() {
    for window in [PHONE, DESKTOP] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::of::<DiscoveryFixture>())
        })
        .expect("the engine starts");
        app.wait_for("role:header[Pair with this environment]")
            .expect("title");
        app.wait_for("role:image[T3 Code]").expect("brand mark");
        app.press("role:button[/Laptop/]").expect("laptop");
        app.wait_for("opened [\"laptop\"]").expect("pressed");
        assert!(
            !app.screen()
                .enabled("role:button[/Server/]")
                .expect("enabled")
        );
        let _ = app.press("role:button[/Server/]");
        app.settle();
        assert!(
            app.shows("opened [\"laptop\"]"),
            "a disabled row does nothing"
        );
    }
}
