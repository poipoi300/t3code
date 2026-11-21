use std::cell::Cell;

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_theme::palette::AppearanceMode;

use crate::theme::{Scope, ThemeChoice, install, tokens_for, tw_with};

#[component]
fn SwitchFixture() -> impl IntoElement {
    install(&choice(AppearanceMode::Light));
    switch_controls(
        || install(&choice(AppearanceMode::Dark)),
        || install(&choice(AppearanceMode::Light)),
    )
}

#[component]
fn SavedSwitchFixture() -> impl IntoElement {
    use crate::state::nav::{Navigator, Route};
    use crate::theme::{Edit, ThemeState};
    use t3_client::{ClientStore, composition::ClientRuntime, testing::FakePlatform};
    use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
    provide_context(Navigator::new(Route::Home));
    let owner = crate::runtime::tasks::TaskOwner::here();
    let client = ClientRuntime::new(
        FakePlatform::default(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        std::rc::Rc::new(move |future| owner.spawn(future)),
    );
    crate::theme::bind(&client, None);
    crate::theme::with_saved_theme(|| {
        let state = use_context::<ThemeState>().expect("root theme state");
        switch_controls(
            move || {
                state.choose(Edit::Whole("ocean".into()));
                state.choose(Edit::Mode("dark".into()));
            },
            move || {
                state.choose(Edit::Whole("light".into()));
                state.choose(Edit::Mode("light".into()));
            },
        )
        .into_element()
    })
}

fn switch_controls(dark: impl Fn() + 'static, light: impl Fn() + 'static) -> impl IntoElement {
    let builds = Cell::new(0);
    view()
        .child(dynamic(move || {
            builds.set(builds.get() + 1);
            let class = tw_with(Scope::Sidebar, "text-foreground", &[]);
            view()
                .child(text("Sample").app_class(class))
                .child(text(format!("Build {}", builds.get())))
        }))
        .child(nsh_base::button().on_press(dark).child(text("Dark")))
        .child(nsh_base::button().on_press(light).child(text("Light")))
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
    assert_repaint_without_parent_builds(Root::of::<SwitchFixture>());
}

#[test]
fn saved_theme_edits_repaint_without_rebuilding_the_parent() {
    assert_repaint_without_parent_builds(Root::of::<SavedSwitchFixture>());
}

fn assert_repaint_without_parent_builds(root: Root) {
    let mut app = Headless::start(EngineConfig {
        ui_sets: crate::registries(),
        ..EngineConfig::new(root)
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
    app.wait_until("Ocean dark paint", nsh_testing::WAIT, |screen| {
        screen
            .span_style("text:Sample")
            .is_ok_and(|style| style.color == dark_color.into())
    })
    .expect("saved dark palette");
    assert_eq!(color(&app), dark_color.into());
    let dark = builds(&app);
    app.press("Light").expect("select light");
    app.wait_until("stock light paint", nsh_testing::WAIT, |screen| {
        screen
            .span_style("text:Sample")
            .is_ok_and(|style| style.color == light.into())
    })
    .expect("saved light palette");
    assert_eq!(color(&app), light.into());
    assert_eq!([dark, builds(&app)], [before.clone(), before]);
}
