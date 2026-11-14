//! UI-FORM: behavioral acceptance cases from the TSX and Base UI control contracts.
//! The application has no form showcase route; mount these primitives directly rather
//! than modifying the frozen app/router. Use the same PHONE and DESKTOP viewports.

use nsh_core::{Checked, prelude::*};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::i18n::t;
use t3_ui::ui::slider::{
    NumberFieldProps, Size as NumberSize, SliderProps, number_field_control, slider,
};
use t3_ui::ui::switch::{
    ControlState, Size, Variant, checkbox, checkbox_mark, label, radio, radio_group, switch,
};

use crate::support::{DESKTOP, PHONE};

#[component]
fn FormControls() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let enabled = signal(false);
    let mixed = signal(true);
    let checked = signal(false);
    let selection = signal(0u8);
    let requests = signal(0u32);
    let locked = signal(false);
    let amount = signal(Some(1.0));
    let empty = signal(None::<f64>);
    let contrast = signal(50.0);
    let released = signal(0.0);
    let releases = signal(0u32);
    let changes = signal(0u32);
    let state = ControlState {
        checked: enabled.into(),
        mixed: mixed.into(),
        disabled: locked.into(),
        ..Default::default()
    };
    let mut numeric = NumberFieldProps::new("Quantity", amount);
    numeric.min = Some(0.0).into();
    numeric.max = Some(2.0).into();
    numeric.step = 0.25.into();
    numeric.disabled = locked.into();
    let mut readonly = NumberFieldProps::new("Read only quantity", amount);
    readonly.readonly = true.into();
    let mut range = SliderProps::new("Contrast", contrast);
    range.min = 20.0.into();
    range.max = 80.0.into();
    range.step = 5.0.into();
    range.disabled = locked.into();
    view()
        .w_full()
        .h_full()
        .p(px(24.0))
        .gap(px(16.0))
        .child(switch(
            "Enable feature",
            state,
            Size::Default,
            move |next| {
                requests.update(|n| *n += 1);
                enabled.set(next);
                mixed.set(false);
            },
        ))
        .child(switch(
            "Refuse change",
            ControlState::default(),
            Size::Sm,
            move |_| requests.update(|n| *n += 1),
        ))
        .child(checkbox(
            "Select all",
            ControlState {
                checked: checked.into(),
                mixed: mixed.into(),
                disabled: locked.into(),
                ..Default::default()
            },
            Variant::Contrast,
            move |next| {
                requests.update(|n| *n += 1);
                checked.set(next);
                mixed.set(false);
            },
        ))
        .child(checkbox_mark(checked, Variant::Default))
        .child(label(text(t("Selection label")), move || {
            if !locked.get_untracked() {
                checked.update(|value| *value = !*value);
            }
        }))
        .child(radio_group(
            "Layout",
            view()
                .gap(px(12.0))
                .child(radio(
                    "First layout",
                    ControlState {
                        checked: Prop::bind(move || selection.get() == 0),
                        disabled: locked.into(),
                        ..Default::default()
                    },
                    move |_| selection.set(0),
                ))
                .child(radio(
                    "Unavailable layout",
                    ControlState {
                        disabled: true.into(),
                        ..Default::default()
                    },
                    move |_| selection.set(1),
                ))
                .child(radio(
                    "Last layout",
                    ControlState {
                        checked: Prop::bind(move || selection.get() == 2),
                        disabled: locked.into(),
                        ..Default::default()
                    },
                    move |_| selection.set(2),
                )),
        ))
        .child(number_field_control(
            numeric,
            NumberSize::Default,
            move |value| amount.set(value),
        ))
        .child(number_field_control(readonly, NumberSize::Sm, move |_| {
            requests.update(|n| *n += 1)
        }))
        .child(number_field_control(
            NumberFieldProps::new("Empty quantity", empty),
            NumberSize::Lg,
            move |value| empty.set(value),
        ))
        .child(view().w(px(200.0)).child(slider(
            range,
            move |value| {
                contrast.set(value);
                changes.update(|n| *n += 1);
            },
            move |value| {
                released.set(value);
                releases.update(|n| *n += 1);
            },
        )))
        .child(
            nsh_base::button()
                .on_press(move || locked.update(|v| *v = !*v))
                .child(text(t("Toggle disabled"))),
        )
        .child(
            nsh_base::button()
                .on_press(move || mixed.set(true))
                .child(text(t("Restore mixed"))),
        )
        .child(text(move || t(&format!("Requests {}", requests.get()))))
        .child(text(move || t(&format!("Selection {}", selection.get()))))
        .child(text(move || t(&format!("Amount {:?}", amount.get()))))
        .child(text(move || t(&format!("Empty {:?}", empty.get()))))
        .child(text(move || {
            t(&format!(
                "Contrast {} released {} ends {}",
                contrast.get(),
                released.get(),
                releases.get()
            ))
        }))
        .child(text(move || {
            t(if changes.get() > 1 {
                "Live slider changes"
            } else {
                "No live slider changes"
            })
        }))
}

fn start_form(viewport: (f32, f32)) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(viewport.0), px(viewport.1)),
        ..EngineConfig::new(Root::of::<FormControls>())
    };
    // Match NSH's roving-focus tests: Tab stops on pressable views on this host.
    config.environment.os = nsh_core::platform::Os::MacOs;
    let mut app = Headless::start(config).expect("form engine");
    app.wait_for("Requests 0").expect("form is ready");
    app
}

#[test]
fn mixed_switch_reports_mixed_then_toggles_the_controlled_value_at_both_sizes() {
    for viewport in [PHONE, DESKTOP] {
        let mut app = start_form(viewport);
        assert_eq!(
            app.screen()
                .accessibility_state("label:Enable feature")
                .expect("switch state")
                .checked,
            Some(Checked::Mixed)
        );
        app.press("label:Enable feature").expect("toggle on");
        app.wait_for("Requests 1").expect("one request");
        assert_eq!(
            app.screen()
                .accessibility_state("label:Enable feature")
                .expect("switch state")
                .checked,
            Some(Checked::True)
        );
        app.press("Restore mixed").expect("mixed over checked");
        assert_eq!(
            app.screen()
                .accessibility_state("label:Enable feature")
                .expect("mixed switch")
                .checked,
            Some(Checked::Mixed)
        );
        app.press("label:Enable feature")
            .expect("mixed checked toggles off");
        app.wait_for("Requests 2").expect("second request");
        assert_eq!(
            app.screen()
                .accessibility_state("label:Enable feature")
                .expect("switch state")
                .checked,
            Some(Checked::False)
        );
        app.press("label:Refuse change")
            .expect("request controlled change");
        app.wait_for("Requests 3").expect("callback ran");
        assert_eq!(
            app.screen()
                .accessibility_state("label:Refuse change")
                .expect("controlled switch")
                .checked,
            Some(Checked::False)
        );
    }
}

#[test]
fn indeterminate_checkbox_checks_then_unchecks_and_a_label_activates_its_control() {
    let mut app = start_form(DESKTOP);
    assert_eq!(
        app.screen()
            .accessibility_state("label:Select all")
            .expect("checkbox")
            .checked,
        Some(Checked::Mixed)
    );
    app.press("label:Select all").expect("select all");
    app.wait_for("Requests 1").expect("callback");
    assert_eq!(
        app.screen()
            .accessibility_state("label:Select all")
            .expect("checkbox")
            .checked,
        Some(Checked::True)
    );
    app.press("label:Select all").expect("unselect");
    app.wait_for("Requests 2").expect("callback");
    assert_eq!(
        app.screen()
            .accessibility_state("label:Select all")
            .expect("checkbox")
            .checked,
        Some(Checked::False)
    );
    app.press("Selection label").expect("associated label");
    assert_eq!(
        app.screen()
            .accessibility_state("label:Select all")
            .expect("checkbox")
            .checked,
        Some(Checked::True)
    );
    assert_eq!(
        app.screen().count("role:checkbox").expect("checkbox count"),
        1,
        "CheckboxMark is decorative"
    );
    app.press("Restore mixed")
        .expect("indeterminate over checked");
    app.press("label:Select all")
        .expect("resolve indeterminate to checked");
    app.wait_for("Requests 3").expect("third checkbox request");
    assert_eq!(
        app.screen()
            .accessibility_state("label:Select all")
            .expect("checkbox")
            .checked,
        Some(Checked::True)
    );
}

#[test]
fn disabled_controls_are_inert_and_can_be_enabled_again() {
    let mut app = start_form(PHONE);
    app.press("Toggle disabled").expect("disable");
    for selector in [
        "label:Enable feature",
        "label:Select all",
        "label:First layout",
        "label:Quantity",
        "label:Contrast",
    ] {
        assert!(
            !app.screen().enabled(selector).expect("control exists"),
            "{selector} is disabled"
        );
        assert!(
            app.press(selector).is_err(),
            "disabled control rejects a press"
        );
    }
    app.press("Selection label").expect("disabled label action");
    assert!(app.screen().shows("Requests 0"));
    assert!(app.screen().shows("Amount Some(1.0)"));
    assert!(app.screen().shows("Contrast 50 released 0 ends 0"));
    app.press("Toggle disabled").expect("enable again");
    app.press("label:Select all").expect("enabled checkbox");
    app.wait_for("Requests 1").expect("enabled callback");
}

#[test]
fn radio_group_arrows_skip_disabled_options_and_checked_radio_stays_checked() {
    let mut app = start_form(DESKTOP);
    for _ in 0..4 {
        app.press_key("tab").expect("Tab to the selected radio");
    }
    app.press_key("right").expect("next enabled radio");
    app.wait_for("Selection 2").expect("last selected");
    assert_eq!(
        app.screen()
            .accessibility_state("label:First layout")
            .expect("first radio")
            .checked,
        Some(Checked::False)
    );
    assert_eq!(
        app.screen()
            .accessibility_state("label:Last layout")
            .expect("last radio")
            .checked,
        Some(Checked::True)
    );
    app.press("label:Last layout").expect("already selected");
    assert!(app.screen().shows("Selection 2"));
    app.press_key("right").expect("wrap around");
    app.wait_for("Selection 0").expect("first selected again");
}

#[test]
fn number_field_steps_decimals_clamps_on_commit_and_preserves_empty() {
    for viewport in [PHONE, DESKTOP] {
        let mut app = start_form(viewport);
        app.type_text("label:Quantity", "").expect("focus number");
        app.press_key("up").expect("step");
        app.wait_for("Amount Some(1.25)").expect("decimal step");
        app.press_key("pageup").expect("large step");
        app.wait_for("Amount Some(2.0)").expect("bounded step");
        app.clear("label:Quantity").expect("clear");
        app.wait_for("Amount None").expect("empty retained");
        app.type_text("label:Quantity", "9")
            .expect("type beyond bound");
        app.wait_for("Amount Some(9.0)")
            .expect("draft retained while editing");
        app.press_key("enter").expect("commit");
        app.wait_for("Amount Some(2.0)")
            .expect("committed value clamped");
        assert_eq!(
            app.screen()
                .input_text("label:Quantity")
                .expect("number input"),
            Some("2")
        );
    }
}

#[test]
fn slider_reports_live_values_and_release_at_bounds_and_on_steps() {
    for viewport in [PHONE, DESKTOP] {
        let mut app = start_form(viewport);
        app.drag("label:Contrast", point(px(200.0), px(0.0)))
            .expect("drag past maximum");
        app.wait_for("Contrast 80 released 80 ends 1")
            .expect("clamped drag and release");
        assert!(app.screen().shows("Live slider changes"));
        app.drag("label:Contrast", point(px(-200.0), px(0.0)))
            .expect("drag past minimum");
        app.wait_for("Contrast 20 released 20 ends 2")
            .expect("reverse drag and release");
        app.drag("label:Contrast", point(px(46.0), px(0.0)))
            .expect("snap to step");
        app.wait_for("Contrast 35 released 35 ends 3")
            .expect("snapped change");
    }
}

#[test]
fn number_field_accepts_an_initially_empty_value_and_readonly_never_reports_changes() {
    let mut app = start_form(DESKTOP);
    assert_eq!(
        app.screen()
            .input_text("label:Empty quantity")
            .expect("empty input"),
        Some("")
    );
    app.type_text("label:Empty quantity", "3.5")
        .expect("type first value");
    app.wait_for("Empty Some(3.5)")
        .expect("first edit reaches caller");
    app.clear("label:Empty quantity").expect("empty again");
    app.wait_for("Empty None").expect("empty reaches caller");
    let _ = app.type_text("label:Read only quantity", "8");
    assert_eq!(
        app.screen()
            .input_text("label:Read only quantity")
            .expect("readonly input"),
        Some("1")
    );
    assert!(app.screen().shows("Requests 0"));
}
