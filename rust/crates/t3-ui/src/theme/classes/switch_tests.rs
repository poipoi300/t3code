use std::cell::Cell;

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_theme::palette::AppearanceMode;

use crate::theme::{Scope, ThemeChoice, install, tokens_for, tw_with};

#[component]
fn SwitchFixture() -> impl IntoElement {
    install(&choice(AppearanceMode::Light));
    let builds = Cell::new(0);
    view()
        .child(dynamic(move || {
            builds.set(builds.get() + 1);
            let class = tw_with(Scope::Sidebar, "text-foreground", &[]);
            view()
                .child(text("Sample").app_class(class))
                .child(text(format!("Build {}", builds.get())))
        }))
        .child(
            nsh_base::button()
                .on_press(|| install(&choice(AppearanceMode::Dark)))
                .child(text("Dark")),
        )
        .child(
            nsh_base::button()
                .on_press(|| install(&choice(AppearanceMode::Light)))
                .child(text("Light")),
        )
}

fn choice(mode: AppearanceMode) -> ThemeChoice {
    ThemeChoice {
        theme: if mode == AppearanceMode::Dark {
            "ocean".into()
        } else {
            String::new()
        },
        mode,
        ..Default::default()
    }
}

#[test]
fn switching_appearance_repaints_the_class_without_rebuilding_its_parent() {
    let mut app = Headless::start(EngineConfig {
        ui_sets: crate::registries(),
        ..EngineConfig::new(Root::of::<SwitchFixture>())
    })
    .expect("theme fixture");
    app.wait_for("Sample").expect("styled text");
    let color = |app: &Headless| {
        app.screen()
            .span_style("text:Sample")
            .expect("painted text")
            .color
    };
    let builds = |app: &Headless| {
        app.screen()
            .texts()
            .into_iter()
            .find(|t| t.starts_with("Build "))
            .expect("build count")
            .to_owned()
    };
    let tokens = tokens_for(&ThemeChoice::default());
    let light = tokens
        .color("sb-foreground")
        .expect("stock sidebar foreground")
        .light;
    let dark_color = tokens_for(&choice(AppearanceMode::Dark))
        .color("sb-foreground")
        .expect("ocean sidebar foreground")
        .dark;
    assert_ne!(light, dark_color);
    assert_eq!(color(&app), light.into());
    let before = builds(&app);
    app.press("Dark").expect("select dark");
    assert_eq!(color(&app), dark_color.into());
    let dark = builds(&app);
    app.press("Light").expect("select light");
    assert_eq!(color(&app), light.into());
    assert_eq!([dark, builds(&app)], [before.clone(), before]);
}
