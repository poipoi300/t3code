//! UI-QR: the code as callers mount it (pairing dialog, mobile-app row), named by its title.
use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::{
    i18n::Text,
    theme,
    ui::qr_code::{Level, QrCodeOptions, qr_code_svg},
};

#[component]
fn Fixture() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let link = signal(String::from("https://app.t3.codes/pair#token=first"));
    view()
        .child(
            nsh_base::button()
                .child(text("Rotate link"))
                .on_press(move || link.set(String::from("https://app.t3.codes/pair#token=second"))),
        )
        .child(text(move || link.get()))
        .child(
            view()
                .app_class(theme::tw("rounded-lg bg-white p-1.5"))
                .child(qr_code_svg(
                    move || link.get(),
                    QrCodeOptions {
                        size: 132.0,
                        level: Level::M,
                        margin_size: 2,
                        title: Some(Text::from("Pairing link. Scan to open on another device")),
                        ..QrCodeOptions::default()
                    },
                )),
        )
        .child(qr_code_svg("decorative", QrCodeOptions::default()))
}

fn start(window: (f32, f32)) -> Headless {
    Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<Fixture>())
    })
    .expect("the engine starts")
}

#[test]
fn a_titled_code_is_one_named_image_that_follows_its_value() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window);
        let code = "role:image[Pairing link. Scan to open on another device]";
        app.wait_for(code).expect("the titled code is an image");
        assert_eq!(
            app.screen().count("role:image").expect("count"),
            1,
            "the untitled code is decorative"
        );
        let first = app.screen().id(code).expect("node");
        app.press("role:button[Rotate link]").expect("rotate");
        app.wait_for("text:https://app.t3.codes/pair#token=second")
            .expect("rotated");
        assert_eq!(
            app.screen().id(code).expect("node"),
            first,
            "the same view redraws in place"
        );
    }
}
