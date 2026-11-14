//! Select's real NSH behavior; a local harness mounts the primitive before consuming screens land.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use nsh_core::platform::{Idiom, Os};
use nsh_core::prelude::*;
use nsh_core::{MountItem, NodeId};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::i18n::t;
use t3_ui::theme;
use t3_ui::ui::select::{self, SelectItem, SelectProps, Value};

use crate::support::{DESKTOP, PHONE};

#[derive(Clone, PartialEq, Props)]
struct HarnessProps {
    controlled: bool,
    follow: bool,
    initial: Option<String>,
    controlled_open: bool,
}

#[component]
fn Harness(props: HarnessProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let value = signal(props.initial.clone().map(SharedString::from));
    let disabled = signal(false);
    let open = signal(props.controlled_open.then_some(false));
    let changes = signal(0usize);
    let open_requests = signal(0usize);
    let button_presses = signal(0usize);
    on_key("f8", move |_| disabled.set(true));
    let items = signal(vec![
        SelectItem::new("one", t("One")).group(t("Numbers")),
        SelectItem::new("two", t("Two"))
            .group(t("Numbers"))
            .disabled(true),
        SelectItem::new("three", t("Three")).group(t("Numbers")),
    ]);
    let mut config = SelectProps::new("Choose a number");
    config.items = items.into();
    config.disabled = disabled.into();
    config.placeholder = t3_ui::i18n::Text::from("Choose").into();
    config.default_value = props.initial.clone().map(SharedString::from);
    if props.controlled {
        config.value = Prop::bind(move || Value::Controlled(value.get()));
    }
    if props.controlled_open {
        config.open = open.into();
    }
    let follow = props.follow;
    config.on_open_change = Some(std::rc::Rc::new(move |next, _| {
        open_requests.update(|requests| *requests += 1);
        if follow {
            open.set(Some(next));
        }
    }));
    config.empty = Some(std::rc::Rc::new(|| text(t("No options")).into_element()));
    view()
        .w_full()
        .h_full()
        .child(
            view()
                .w(px(240.0))
                .child(select::select(config, move |next| {
                    changes.update(|count| *count += 1);
                    if follow {
                        value.set(next);
                    }
                })),
        )
        .child(text(move || format!("Changes: {}", changes.get())))
        .child(text(move || {
            format!("Open requests: {}", open_requests.get())
        }))
        .child(select::select_button(
            text(t("Standalone choice")),
            "Standalone choice",
            select::Size::Default,
            disabled.into(),
            move || button_presses.update(|count| *count += 1),
        ))
        .child(text(move || {
            format!("Button presses: {}", button_presses.get())
        }))
        .child(text(move || {
            format!("Value: [{}]", value.get().as_deref().unwrap_or("none"))
        }))
        .child(
            nsh_base::button()
                .accessibility_label(t("Toggle disabled"))
                .on_press(move || {
                    disabled.update(|disabled| *disabled = !*disabled);
                })
                .child(text(t("Toggle disabled"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(t("Replace options"))
                .on_press(move || {
                    items.set(vec![SelectItem::new("four", t("Four"))]);
                })
                .child(text(t("Replace options"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(t("Empty options"))
                .on_press(move || {
                    items.set(Vec::new());
                })
                .child(text(t("Empty options"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(t("Clear value"))
                .on_press(move || {
                    value.set(None);
                })
                .child(text(t("Clear value"))),
        )
}

fn start(window: (f32, f32), controlled: bool, follow: bool, initial: Option<&str>) -> Headless {
    start_case(
        window,
        HarnessProps {
            controlled,
            follow,
            initial: initial.map(str::to_owned),
            controlled_open: false,
        },
    )
}

fn start_case(window: (f32, f32), props: HarnessProps) -> Headless {
    start_case_observed(window, props).0
}

type AccessibleValues = Arc<Mutex<BTreeMap<NodeId, Option<String>>>>;

fn start_case_observed(window: (f32, f32), props: HarnessProps) -> (Headless, AccessibleValues) {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<Harness>(move || props.clone()))
    };
    config.environment.os = if window == PHONE {
        Os::Android
    } else {
        // NSH's headless Windows host does not simulate Tab navigation.
        Os::Web
    };
    config.environment.idiom = if window == PHONE {
        Idiom::Phone
    } else {
        Idiom::Desktop
    };
    let mut app = Headless::start(config).expect("select harness starts");
    let values = AccessibleValues::default();
    let observed = values.clone();
    app.observe(move |_, batch| {
        let mut values = observed.lock().expect("accessible values");
        for item in &batch.items {
            if let MountItem::ViewProps { id, props } = item {
                values.insert(
                    *id,
                    props
                        .accessibility
                        .value
                        .as_ref()
                        .and_then(|value| value.text.as_deref())
                        .map(str::to_owned),
                );
            }
        }
    });
    app.wait_for("role:combobox[Choose a number]")
        .expect("trigger");
    (app, values)
}

fn assert_accessible_value(app: &mut Headless, values: &AccessibleValues, expected: &str) {
    app.wait_until(
        "the selected label or real placeholder reaches accessibility",
        std::time::Duration::from_secs(5),
        |screen| {
            let id = screen
                .id("role:combobox[Choose a number]")
                .expect("trigger");
            values
                .lock()
                .expect("accessible values")
                .get(&id)
                .and_then(Option::as_deref)
                == Some(expected)
        },
    )
    .expect("accessible value");
}

fn open(app: &mut Headless) {
    app.press("role:combobox[Choose a number]").expect("open");
    app.wait_for("role:option[One]").expect("options");
}

fn refuses_disabled_press(app: &mut Headless, selector: &str) {
    let error = app
        .press(selector)
        .expect_err("disabled control refuses input");
    assert!(error.contains("is disabled"), "{error}");
}

#[test]
fn choosing_updates_the_value_closes_and_reopens_with_the_selection_on_both_layouts() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, true, true, None);
        open(&mut app);
        assert!(
            !app.screen()
                .enabled("role:option[Two]")
                .expect("disabled option")
        );
        refuses_disabled_press(&mut app, "role:option[Two]");
        assert!(app.screen().shows("Changes: 0"));
        app.press("role:option[Three]").expect("choose");
        app.wait_for("Value: [three]").expect("callback value");
        app.wait_for("Changes: 1").expect("one callback");
        app.wait_for("text:Three").expect("trigger follows");
        app.wait_for_gone("role:listbox", std::time::Duration::from_secs(5))
            .expect("closed");
        open(&mut app);
        assert!(
            app.screen()
                .selected("role:option[Three]")
                .expect("selected option")
        );
        app.press("role:option[Three]").expect("same value");
        app.wait_for_gone("role:listbox", std::time::Duration::from_secs(5))
            .expect("closed again");
        assert!(
            app.screen().shows("Changes: 1"),
            "choosing the current value does not emit a change"
        );
        app.press("role:button[Clear value]")
            .expect("clear controlled value");
        app.wait_for("text:Choose").expect("placeholder restored");
        assert!(app.screen().shows("Value: [none]"));
        assert!(
            app.screen().shows("Changes: 1"),
            "a parent value change does not emit a user change"
        );
    }
}

#[test]
fn controlled_value_can_decline_a_change_and_unknown_values_are_preserved() {
    for initial in ["one", "unknown", ""] {
        let (mut app, values) = start_case_observed(
            DESKTOP,
            HarnessProps {
                controlled: true,
                follow: false,
                initial: Some(initial.to_owned()),
                controlled_open: false,
            },
        );
        assert_accessible_value(
            &mut app,
            &values,
            if initial == "one" { "One" } else { "Choose" },
        );
        if initial == "unknown" {
            app.wait_for("text:unknown")
                .expect("unknown value is shown verbatim");
        }
        if initial.is_empty() {
            app.wait_for("text:Choose")
                .expect("empty-string value uses the placeholder");
        }
        open(&mut app);
        app.press("role:option[Three]").expect("choose");
        app.wait_for("Changes: 1").expect("change reported");
        assert!(app.screen().shows(&format!("Value: [{initial}]")));
        open(&mut app);
        assert_eq!(
            app.screen().selected("role:option[One]"),
            Ok(initial == "one")
        );
        assert_eq!(app.screen().selected("role:option[Three]"), Ok(false));
    }
}

#[test]
fn a_controlled_open_request_can_be_declined() {
    let mut app = start_case(
        DESKTOP,
        HarnessProps {
            controlled: true,
            follow: false,
            initial: None,
            controlled_open: true,
        },
    );
    app.press("role:combobox[Choose a number]")
        .expect("request open");
    app.wait_for("Open requests: 1").expect("open requested");
    assert_eq!(app.screen().matches("role:listbox"), Ok(false));
    assert!(app.screen().shows("Changes: 0"));
}

#[test]
fn uncontrolled_default_value_follows_choices_without_parent_feedback() {
    let mut app = start(DESKTOP, false, false, Some("one"));
    open(&mut app);
    assert_eq!(app.screen().selected("role:option[One]"), Ok(true));
    app.press("role:option[Three]").expect("choose");
    app.wait_for("Changes: 1").expect("change");
    open(&mut app);
    assert_eq!(app.screen().selected("role:option[Three]"), Ok(true));
    assert!(
        app.screen().shows("Value: [one]"),
        "the parent did not follow the callback"
    );
}

#[test]
fn disabled_trigger_is_inert_and_items_and_empty_state_update_reactively() {
    let (mut app, values) = start_case_observed(
        DESKTOP,
        HarnessProps {
            controlled: true,
            follow: true,
            initial: None,
            controlled_open: false,
        },
    );
    app.press("role:button[Standalone choice]")
        .expect("standalone button");
    app.wait_for("Button presses: 1").expect("button callback");
    app.press("role:button[Toggle disabled]").expect("disable");
    app.wait_until(
        "disabled trigger",
        std::time::Duration::from_secs(5),
        |screen| screen.enabled("role:combobox[Choose a number]") == Ok(false),
    )
    .expect("disabled");
    refuses_disabled_press(&mut app, "role:combobox[Choose a number]");
    assert_eq!(app.screen().matches("role:listbox"), Ok(false));
    refuses_disabled_press(&mut app, "role:button[Standalone choice]");
    assert!(app.screen().shows("Button presses: 1"));
    app.press("role:button[Toggle disabled]").expect("enable");
    app.press("role:button[Replace options]").expect("replace");
    app.press("role:combobox[Choose a number]")
        .expect("open new options");
    app.wait_for("role:option[Four]").expect("new item");
    assert_eq!(app.screen().matches("role:option[One]"), Ok(false));
    app.press("role:option[Four]").expect("choose new item");
    app.wait_for("Value: [four]").expect("new value");
    assert_accessible_value(&mut app, &values, "Four");
    app.press("role:button[Empty options]")
        .expect("clear items");
    app.wait_for("text:four")
        .expect("missing item's value is shown like Base UI's fallback");
    assert_accessible_value(&mut app, &values, "Choose");
    app.press("role:combobox[Choose a number]")
        .expect("open empty");
    app.wait_for("No options").expect("empty slot");
    assert!(
        app.screen().shows("Value: [four]"),
        "removing an item never clears the stored value"
    );
}

#[test]
fn escape_cancels_and_keyboard_navigation_skips_disabled_options() {
    let mut app = start(DESKTOP, true, true, Some("one"));
    app.press_key("tab").expect("focus trigger");
    app.press_key("down").expect("open");
    app.wait_for("role:option[One]").expect("options");
    app.press_key("down").expect("skip disabled Two");
    app.press_key("enter").expect("select Three");
    app.wait_for("Value: [three]").expect("keyboard selection");
    app.press_key("down").expect("reopen");
    app.wait_for("role:option[One]").expect("options");
    app.press_key("home").expect("highlight One");
    app.press_key("escape").expect("cancel");
    app.wait_for_gone("role:listbox", std::time::Duration::from_secs(5))
        .expect("dismissed");
    assert!(app.screen().shows("Changes: 1"));
    assert!(app.screen().shows("Value: [three]"));
}

#[test]
fn disabling_an_open_select_does_not_change_its_value() {
    let mut app = start(DESKTOP, false, true, Some("one"));
    open(&mut app);
    app.press_key("f8").expect("disable while open");
    app.wait_until(
        "disabled trigger",
        std::time::Duration::from_secs(5),
        |screen| screen.enabled("role:combobox[Choose a number]") == Ok(false),
    )
    .expect("disabled");
    app.press("role:option[Three]").expect("press stale option");
    app.wait_for_gone("role:listbox", std::time::Duration::from_secs(5))
        .expect("closed");
    assert!(app.screen().shows("Changes: 0"));
    assert!(app.screen().shows("Value: [one]"));
    app.press("role:button[Toggle disabled]").expect("enable");
    open(&mut app);
    assert_eq!(app.screen().selected("role:option[One]"), Ok(true));
}

/// Rule 4 of fidelity review 2: the label and placeholder take `Text`; content ("Settings", a
/// catalogue key) is shown as given, a key translates ("Paramètres" in French). Item titles are
/// final text the caller supplies (a native language name stays as it is).
#[component]
fn ContentSelect() -> impl IntoElement {
    theme::install(&Default::default());
    t3_ui::i18n::set_language("fr", &[]);
    let mut content = SelectProps::new(t3_ui::i18n::Text::content("Settings"));
    content.items = vec![SelectItem::new("fr", "Français")].into();
    content.placeholder = t3_ui::i18n::Text::content("Settings").into();
    let mut key = SelectProps::new("Choose a number");
    key.placeholder = t3_ui::i18n::Text::from("Settings").into();
    view()
        .w_full()
        .h_full()
        .child(select::select(content, |_| {}))
        .child(select::select(key, |_| {}))
}

#[test]
fn content_slots_are_shown_verbatim_and_key_slots_translated() {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(PHONE.0), px(PHONE.1)),
        ..EngineConfig::new(Root::of::<ContentSelect>())
    })
    .expect("content select");
    app.wait_for("Paramètres")
        .expect("the key placeholder translates");
    let screen = app.screen();
    assert!(
        screen.texts().contains(&"Settings"),
        "the content placeholder is shown as given: {:?}",
        screen.texts()
    );
}
