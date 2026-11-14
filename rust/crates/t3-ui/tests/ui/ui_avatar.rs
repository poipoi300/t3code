//! Port of ProjectFavicon.browser.test.tsx plus the image replacement/error contract.

use nsh_core::prelude::*;
use nsh_core::{EventKind, EventPayload};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::types::EnvironmentId;
use t3_ui::theme::{self, ThemeChoice};
use t3_ui::ui::avatar::{ProjectFaviconProject, project_favicon};

use crate::support::{DESKTOP, PHONE};

const FIRST: &str = "https://host.test/first.svg";
const SECOND: &str = "https://host.test/second.svg";
const DATA: &str =
    "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='16' height='16'/%3E";

#[component]
fn Fixture() -> impl IntoElement {
    theme::install(&ThemeChoice::default());
    let project = signal(ProjectFaviconProject {
        environment_id: EnvironmentId("environment".into()),
        workspace_root: "/repo".into(),
        title: "Rust UI".into(),
        favicon_path: None,
        project_icon: Some(
            serde_json::from_value(serde_json::json!({
                "kind":"monogram", "text":"ZZ", "color":"blue",
            }))
            .expect("override"),
        ),
    });
    let src = signal(Some(FIRST.to_owned()));
    let reads = signal(0usize);
    view()
        .child(project_favicon(
            move || project.get(),
            move |_| {
                reads.set(reads.get_untracked() + 1);
                src.get()
            },
            "",
            None,
        ))
        .child(text(move || format!("resource reads: {}", reads.get())))
        .child(
            nsh_base::button()
                .test_id("automatic")
                .child(text("automatic"))
                .on_press(move || project.update(|p| p.project_icon = None)),
        )
        .child(
            nsh_base::button()
                .test_id("emoji")
                .child(text("emoji"))
                .on_press(move || {
                    project.update(|p| {
                        p.project_icon = Some(
                            serde_json::from_value(
                                serde_json::json!({"kind":"emoji","emoji":"🌱"}),
                            )
                            .expect("emoji"),
                        );
                    })
                }),
        )
        .child(
            nsh_base::button()
                .test_id("lucide")
                .child(text("lucide"))
                .on_press(move || {
                    project.update(|p| {
                        p.project_icon = Some(
                            serde_json::from_value(
                                serde_json::json!({"kind":"lucide","name":"leaf","color":"green"}),
                            )
                            .expect("lucide"),
                        );
                    })
                }),
        )
        .child(
            nsh_base::button()
                .test_id("next")
                .child(text("next"))
                .on_press(move || src.set(Some(SECOND.into()))),
        )
        .child(
            nsh_base::button()
                .test_id("cached")
                .child(text("cached"))
                .on_press(move || src.set(Some(DATA.into()))),
        )
        .child(
            nsh_base::button()
                .test_id("missing")
                .child(text("missing"))
                .on_press(move || {
                    src.set(Some(
                        "https://host.test/project-favicon-missing?token=new".into(),
                    ))
                }),
        )
        .child(
            nsh_base::button()
                .test_id("path")
                .child(text("path"))
                .on_press(move || project.update(|p| p.favicon_path = Some("new.svg".into()))),
        )
}

// The frozen App does not mount UI-AVATAR yet; this root drives the real primitive directly.
fn start(window: (f32, f32)) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<Fixture>())
    })
    .expect("start favicon fixture");
    app.first_render().expect("render favicon fixture");
    app
}

fn loaded(app: &mut Headless) {
    let node = pending(app).expect("pending image");
    app.engine().control().dispatch_event(
        node,
        EventKind::Load,
        EventPayload::Image(nsh_core::Size {
            width: nsh_core::geometry::DevicePixels(16),
            height: nsh_core::geometry::DevicePixels(16),
        }),
    );
    app.first_render().expect("settle image load");
    app.wait_for("id:project-favicon-image")
        .expect("loaded image displayed");
}

fn pending(app: &Headless) -> Option<nsh_core::NodeId> {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some("project-favicon-preloader"))
        .map(|node| node.id)
}

#[test]
fn only_automatic_icons_activate_the_shared_resource() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window);
        assert!(app.screen().shows("resource reads: 0"));
        app.press("id:emoji").expect("emoji override");
        assert!(app.screen().shows("🌱"));
        assert!(app.screen().shows("resource reads: 0"));
        app.press("id:lucide").expect("Lucide override");
        assert!(app.screen().shows("resource reads: 0"));
        app.press("id:automatic").expect("remove override");
        assert!(app.screen().shows("resource reads: 1"));
        assert!(app.screen().shows("RU"));
        app.press("id:next").expect("URL refresh");
        assert!(app.screen().shows("resource reads: 2"));
        app.press("id:emoji").expect("restore override");
        app.press("id:cached").expect("update inactive resource");
        assert!(app.screen().shows("resource reads: 2"));
    }
}

#[test]
fn replacement_errors_keep_the_displayed_image_and_path_changes_reset_it() {
    let mut app = start(DESKTOP);
    app.press("id:automatic").expect("automatic");
    assert!(app.screen().shows("RU"));
    loaded(&mut app);
    assert!(!app.screen().shows("RU"));
    app.press("id:next").expect("replace URL");
    assert!(
        app.screen()
            .matches("id:project-favicon-image")
            .expect("old image visible")
    );
    app.fail_images(SECOND).expect("replacement fails");
    assert!(
        !app.screen().shows("RU"),
        "failed replacement must keep the previous image"
    );
    app.press("id:path").expect("change resource key");
    assert!(
        app.screen().shows("RU"),
        "a different favicon path resets displayedSrc"
    );
}

#[test]
fn a_superseded_load_cannot_replace_the_current_request_and_replacements_wait_for_load() {
    let mut app = start(DESKTOP);
    app.press("id:automatic").expect("automatic");
    let old = pending(&app).expect("first request");
    app.press("id:next").expect("supersede first request");
    app.engine().control().dispatch_event(
        old,
        EventKind::Load,
        EventPayload::Image(nsh_core::Size {
            width: nsh_core::geometry::DevicePixels(16),
            height: nsh_core::geometry::DevicePixels(16),
        }),
    );
    app.first_render().expect("settle superseded completion");
    assert!(
        app.screen().shows("RU"),
        "the current request has not loaded"
    );
    loaded(&mut app);
    assert!(!app.screen().shows("RU"));
    app.press("id:cached").expect("replacement data URL");
    assert!(
        !app.screen().shows("RU"),
        "keep the previous displayed image"
    );
    loaded(&mut app);
    assert!(
        app.screen()
            .matches("id:project-favicon-image")
            .expect("replacement visible")
    );
    assert!(pending(&app).is_none(), "replacement finished");
}

#[test]
fn cached_images_show_immediately_and_a_decode_error_or_missing_marker_uses_the_monogram() {
    let mut app = start(PHONE);
    app.press("id:cached").expect("cache URL");
    app.press("id:automatic").expect("automatic");
    assert!(
        !app.screen().shows("RU"),
        "data images are visible in the first commit"
    );
    // Data URIs become content-digest keys inside NSH, so target the mounted image's event.
    let node = app
        .screen()
        .id("id:project-favicon-image")
        .expect("cached image");
    app.engine().control().dispatch_event(
        node,
        EventKind::Error,
        EventPayload::Error {
            message: "Decode failed".into(),
        },
    );
    app.wait_for("RU").expect("decode failure reveals fallback");
    assert!(app.screen().shows("RU"));
    app.press("id:missing").expect("server missing sentinel");
    assert!(app.screen().shows("RU"));
    assert!(pending(&app).is_none(), "no sentinel image request");
}
