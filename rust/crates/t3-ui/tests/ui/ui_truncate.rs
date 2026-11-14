//! MiddleTruncate refits on data, font and layout changes, without a frame callback.

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::i18n::t;
use t3_ui::theme;
use t3_ui::ui::middle_truncate::*;

#[component]
fn Harness() -> impl IntoElement {
    theme::install(&Default::default());
    let value = signal(SharedString::from("t3/investigate-release-build-times"));
    let wide = signal(false);
    let larger = signal(false);
    view()
        .child(
            view()
                .test_id("branch-container")
                .w(move || px(if wide.get() { 800.0 } else { 80.0 }))
                .app_class(theme::tw("max-w-full"))
                .child(middle_truncate_with_font(
                    move || value.get(),
                    Options::default(),
                    move || Font {
                        family: theme::INTER.family.into(),
                        size: if larger.get() { 24.0 } else { 12.0 },
                        weight: FontWeight::NORMAL,
                    },
                )),
        )
        .child(
            nsh_base::button()
                .accessibility_label(t("Short branch"))
                .on_press(move || value.set("main".into()))
                .child(text(t("Short branch"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(t("Replacement branch"))
                .on_press(move || value.set("t3/clone-repo-to-other-machine".into()))
                .child(text(t("Replacement branch"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(t("Toggle width"))
                .on_press(move || wide.update(|wide| *wide = !*wide))
                .child(text(t("Toggle width"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(t("Larger font"))
                .on_press(move || larger.set(true))
                .child(text(t("Larger font"))),
        )
}

fn start(window: (f32, f32)) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<Harness>())
    })
    .expect("truncate fixture");
    app.first_render().expect("render truncate");
    app
}

#[test]
fn a_short_replacement_loses_the_previous_clip_and_a_new_head_refits_at_the_same_width() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window);
        app.wait_for("…times").expect("long branch overflows");
        app.press("role:button[Short branch]")
            .expect("replace long branch");
        app.wait_for("text:main").expect("short value visible");
        assert!(!app.screen().shows("…times"));
        assert!(app.screen().shows("main"));
        app.press("role:button[Replacement branch]")
            .expect("new head in same container");
        app.wait_for("…machine").expect("refitted new head");
        assert!(!app.screen().shows("…times"));
    }
}

#[test]
fn growing_and_shrinking_the_container_clears_and_restores_the_ellipsis() {
    let mut app = start(DESKTOP);
    app.wait_for("…times").expect("narrow");
    app.press("role:button[Toggle width]").expect("grow");
    for node in app.screen().layout_nodes().into_iter().take(12) {
        eprintln!(
            "node {} parent {} id {:?} bounds {:?} text {:?}",
            node.id, node.parent, node.test_id, node.bounds, node.text
        );
    }
    app.wait_for("text:times")
        .expect("full tail without ellipsis");
    assert!(!app.screen().shows("…times"));
    app.press("role:button[Toggle width]").expect("shrink");
    app.wait_for("…times").expect("ellipsis returns");
    app.press("role:button[Larger font]").expect("font changes");
    app.wait_for("…times").expect("still fitted with new font");
    app.press("role:button[Short branch]")
        .expect("short after font change");
    app.wait_for("text:main")
        .expect("font update did not leave stale clipping");
}
