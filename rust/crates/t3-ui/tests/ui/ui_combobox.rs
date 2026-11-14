//! Real field calls, with no fixture-owned selection or dismissal logic.
use crate::support::{DESKTOP, PHONE};
use nsh_core::{
    platform::{Idiom, Os},
    prelude::*,
};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::{i18n::Text, theme, ui::combobox::*};

#[derive(Clone, PartialEq, Props)]
struct FixtureProps {
    mode: u32,
    controlled: bool,
}

#[component]
fn Fixture(props: FixtureProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let mut disabled = Item::new("two", "Two");
    disabled.disabled = true;
    let catalogue = signal(if props.mode == 3 {
        vec![
            Item::new("key", "Settings"),
            Item::new("content", Text::content("Settings")),
        ]
    } else {
        vec![
            Item::new("one", "One"),
            disabled,
            Item::new("three", "Three"),
        ]
    });
    let state = ComboboxState::new(
        move || catalogue.get(),
        Options {
            mode: match props.mode {
                1 => Mode::Multiple,
                2 => Mode::Autocomplete,
                _ => Mode::Single,
            },
            ..Options::default()
        },
    );
    if props.controlled {
        state.on_value_change(|_| {});
    }
    if props.mode == 4 {
        state.on_open_change(|_| {});
    }
    let mut input = InputOptions::new("Choose number");
    input.show_clear = true;
    view()
        .w_full()
        .h_full()
        .child(view().w(px(250.0)).child(combobox_field(
            state,
            input,
            PopupOptions::default(),
            "No matches",
        )))
        .child(text(move || {
            format!(
                "value {}",
                state
                    .value
                    .get()
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }))
        .child(text(move || format!("query {}", state.query.get())))
        .child(text(move || format!("open {}", state.open.get())))
        .child(text(move || {
            format!(
                "highlight {}",
                state.highlighted.get().as_deref().unwrap_or("none")
            )
        }))
        .child(
            nsh_base::button()
                .accessibility_label("French")
                .on_press(|| t3_ui::i18n::set_language("fr", &[]))
                .child(text(String::from("French"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label("Outside")
                .child(text(String::from("Outside"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label("Disable")
                .on_press(move || state.disabled.set(true))
                .child(text(String::from("Disable"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label("Replace")
                .on_press(move || catalogue.set(vec![Item::new("four", "Four")]))
                .child(text(String::from("Replace"))),
        )
}

fn start(window: (f32, f32), mode: u32, controlled: bool) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<Fixture>(move || FixtureProps {
            mode,
            controlled,
        }))
    };
    config.environment.os = Os::Web;
    config.environment.idiom = if window == PHONE {
        Idiom::Phone
    } else {
        Idiom::Desktop
    };
    let mut app = Headless::start(config).expect("field starts");
    app.wait_for("role:combobox[Choose number]").expect("field");
    app
}

#[test]
fn typing_filters_and_pointer_selection_closes_with_the_opaque_value() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, 0, false);
        app.type_text("role:combobox[Choose number]", "Thr")
            .expect("type query");
        app.wait_for("role:option[Three]").expect("filtered option");
        assert!(!app.screen().shows("role:option[One]"));
        app.press("role:option[Three]").expect("choose");
        app.wait_for("value three").expect("selected stable ID");
        app.wait_for("open false").expect("closed");
        app.press("role:button[Clear]").expect("clear");
        assert!(!app.screen().shows("value three"));
    }
}

#[test]
fn arrows_navigate_disabled_options_without_activating_them_and_escape_returns_focus() {
    let mut app = start(DESKTOP, 0, false);
    app.press("role:combobox[Choose number]").expect("focus");
    app.press_key("down").expect("first");
    app.wait_for("highlight one").expect("first highlighted");
    app.press_key("down").expect("disabled row");
    app.wait_for("highlight two").expect("disabled navigable");
    app.press_key("enter").expect("disabled choice");
    assert!(app.screen().shows("open true"));
    app.press_key("down").expect("third");
    app.press_key("enter").expect("choose third");
    app.wait_for("value three").expect("chosen");
    app.press_key("down").expect("reopen");
    app.press_key("escape").expect("cancel");
    app.wait_for("open false").expect("dismissed");
    app.press_key("up").expect("field retained focus");
    app.wait_for("open true").expect("reopened from field");
}

#[test]
fn multiple_selection_toggles_and_stays_open() {
    let mut app = start(PHONE, 1, false);
    app.press("role:button[Show suggestions]").expect("open");
    app.press("role:option[One]").expect("add");
    app.wait_for("value one").expect("one selected");
    app.press("role:option[Three]").expect("add third");
    app.wait_for("value one,three").expect("both");
    app.press("role:option[One]").expect("toggle off");
    app.wait_for("value three").expect("third remains");
    assert!(app.screen().shows("open true"));
}

#[test]
fn autocomplete_retains_unmatched_free_text_on_outside_press_and_escape() {
    let mut app = start(DESKTOP, 2, false);
    app.type_text("role:combobox[Choose number]", "custom value")
        .expect("free text");
    app.wait_for("No matches").expect("empty state");
    app.press("role:button[Outside]").expect("outside");
    app.wait_for("open false").expect("dismissed");
    assert!(app.screen().shows("query custom value"));
    app.press("role:combobox[Choose number]").expect("refocus");
    app.press_key("escape").expect("cancel");
    app.wait_for("open false").expect("closed");
    assert!(app.screen().shows("query custom value"));
}

#[test]
fn controlled_selection_can_decline_and_disabled_trigger_does_not_open() {
    let mut app = start(DESKTOP, 0, true);
    app.press("role:button[Show suggestions]").expect("open");
    app.press("role:option[One]").expect("request choice");
    app.wait_for("open false").expect("selection closes");
    assert!(!app.screen().shows("value one"));
    app.press("role:button[Disable]").expect("disable");
    let error = app
        .press("role:button[Show suggestions]")
        .expect_err("disabled trigger rejects presses");
    assert!(error.contains("disabled"));
    assert!(app.screen().shows("open false"));
}

#[test]
fn replacing_results_removes_stale_options() {
    let mut app = start(DESKTOP, 0, false);
    app.press("role:combobox[Choose number]").expect("focus");
    app.press_key("down").expect("highlight");
    app.wait_for("highlight one").expect("old highlight");
    app.press("role:button[Replace]").expect("replace results");
    app.press("role:button[Show suggestions]").expect("reopen");
    app.wait_for("role:option[Four]").expect("new result");
    assert!(!app.screen().shows("role:option[One]"));
    app.press("role:option[Four]").expect("choose new result");
    app.wait_for("value four").expect("new ID chosen");
}

#[test]
fn translation_keys_update_but_literal_option_names_keep_their_content() {
    let mut app = start(DESKTOP, 3, false);
    app.press("role:button[French]").expect("switch language");
    app.press("role:combobox[Choose number]")
        .expect("open translated options");
    app.wait_for("role:option[Paramètres]")
        .expect("key translated");
    app.wait_for("role:option[Settings]")
        .expect("content unchanged");
    app.press("role:option[Settings]").expect("choose content");
    app.wait_for("value content").expect("content ID preserved");
}

#[test]
fn controlled_open_refusal_prevents_typed_and_keyboard_choices() {
    let mut app = start(DESKTOP, 4, false);
    app.type_text("role:combobox[Choose number]", "One")
        .expect("type");
    assert!(app.screen().shows("open false"));
    assert!(!app.screen().shows("role:option[One]"));
    app.press_key("down").expect("request opening");
    app.press_key("enter").expect("no closed option activates");
    assert!(app.screen().shows("open false"));
    assert!(!app.screen().shows("value one"));
}
