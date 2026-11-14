//! Interactive Badge and noninteractive Kbd behavior, through their public APIs.

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::i18n::{Text, t};
use t3_ui::theme;
use t3_ui::ui::{
    badge::{Child, Props, Variant, badge_button, badge_link},
    icon::Icon,
    kbd,
};

#[component]
fn Harness() -> impl IntoElement {
    theme::install(&Default::default());
    let disabled = signal(false);
    let presses = signal(0);
    let opened = signal(String::new());
    view()
        .child(badge_button(
            Props {
                variant: Variant::Outline,
                ..Default::default()
            },
            "Open details".into(),
            vec![Child::Icon(Icon::Info), Text::from("Details").into()],
            move || disabled.get(),
            move || presses.update(|n| *n += 1),
        ))
        .child(badge_link(
            Props::default(),
            "Documentation".into(),
            || SharedString::from("https://example.test/docs"),
            vec![Text::from("Documentation").into()],
            move || disabled.get(),
            move |url| opened.set(url.to_owned()),
        ))
        .child(kbd::kbd_group(
            view()
                .flex_row()
                .child(kbd::kbd(
                    kbd::Variant::Outline,
                    vec![Child::Icon(Icon::Command)],
                ))
                .child(kbd::kbd(
                    kbd::Variant::Default,
                    vec![Text::content("K").into()],
                )),
        ))
        .child(
            nsh_base::button()
                .accessibility_label(t("Disable badges"))
                .on_press(move || disabled.set(true))
                .child(text(t("Disable badges"))),
        )
        .child(text(move || {
            format!("presses:{};opened:{}", presses.get(), opened.get())
        }))
}

fn start(window: (f32, f32)) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<Harness>())
    };
    config.environment.os = nsh_core::platform::Os::MacOs;
    if window == PHONE {
        config.environment.idiom = nsh_core::platform::Idiom::Phone;
    }
    let mut app = Headless::start(config).expect("badge fixture");
    app.first_render().expect("render badges");
    app
}

#[test]
fn button_and_link_badges_activate_from_the_keyboard_and_disabled_badges_are_inert() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window);
        app.press_key("tab").expect("focus badge button");
        app.press_key("enter").expect("activate button");
        app.wait_for("text:presses:1;opened:")
            .expect("one button activation");
        app.press_key("tab").expect("focus link");
        app.press_key("enter").expect("activate link");
        app.wait_for("text:presses:1;opened:https://example.test/docs")
            .expect("link reports destination");
        app.press("role:button[Disable badges]").expect("disable");
        assert!(app.press("role:button[Open details]").is_err());
        assert!(app.press("role:link[Documentation]").is_err());
        assert!(
            app.screen()
                .shows("presses:1;opened:https://example.test/docs")
        );
    }
}

#[test]
fn shortcut_hints_are_not_press_targets_or_keyboard_stops() {
    let mut app = start(DESKTOP);
    assert!(app.press("text:K").is_err(), "Kbd is read-only");
    app.press_key("tab").expect("button");
    app.press_key("tab").expect("link");
    app.press_key("tab").expect("skip shortcut hints");
    app.press_key("enter").expect("disable button");
    assert!(
        app.press("role:button[Open details]").is_err(),
        "hints did not take focus"
    );
}
