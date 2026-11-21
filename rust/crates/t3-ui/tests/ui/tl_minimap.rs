//! Minimap controls share the production viewport and the client's timeline projection.

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use std::rc::Rc;
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRowsInput, derive_messages_timeline_rows_with_state,
};
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_client::view_logic::session::TimelineEntry;
use t3_ui::runtime::Shared;
use t3_ui::screens::thread::minimap::{TimelineMinimap, timeline_minimap};
use t3_ui::screens::thread::viewport::{TimelineViewport, timeline_row_keys};

#[component]
fn MinimapFixture() -> impl IntoElement {
    // Headless hit boxes omit transforms; a short rail keeps both controls addressable.
    build_minimap_fixture(20)
}

#[component]
fn DenseMinimapFixture() -> impl IntoElement {
    build_minimap_fixture(600)
}

fn build_minimap_fixture(turns: usize) -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let entries: Vec<Rc<TimelineEntry>> = (0..turns * 2).map(|at| Rc::new(serde_json::from_value(json!({
        "kind":"message", "id":format!("m-{at}"), "createdAt":"2026-11-14T12:00:00Z",
        "message":{"id":format!("m-{at}"),"role":if at % 2 == 0 { "user" } else { "assistant" },
            "text":format!("Turn {at}"),"streaming":false,"runId":null,"createdAt":"2026-11-14T12:00:00Z","updatedAt":"2026-11-14T12:00:00Z"}
    })).expect("client entry"))).collect();
    let input = MessagesTimelineRowsInput {
        timeline_entries: entries,
        ..Default::default()
    };
    let projection = derive_messages_timeline_rows_with_state(&input, None);
    let rows = computed(move || Shared(projection.rows.clone()));
    let viewport = TimelineViewport::new(
        ThreadKey {
            environment: EnvironmentId("minimap-fixture".into()),
            thread: ThreadId("thread".into()),
        },
        timeline_row_keys(rows),
    );
    let map = TimelineMinimap::new(viewport);
    rows.with(|rows| map.replace_rows(rows));
    map.set_content_width(736.0);
    let list = virtual_list(computed(move || rows.with(|rows| rows.len())), move |at| {
        view()
            .h(px(80.0))
            .child(text(move || rows.with(|rows| rows[at.index()].id.clone())))
    })
    .flex_1()
    .test_id("minimap-list");
    view().w_full().h_full().child(
        view()
            .flex_1()
            .min_h(px(0.0))
            .relative()
            .child(map.attach(viewport.attach(list)))
            .child(timeline_minimap(map)),
    )
}

fn start(window: (f32, f32)) -> Headless {
    start_root(window, Root::of::<MinimapFixture>())
}

fn start_root(window: (f32, f32), root: Root) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(root)
    })
    .expect("minimap fixture");
    app.set_environment("fine desktop pointer", |env| {
        // The pinned headless backend delivers raw view keys only in Linux mode.
        env.os = nsh_core::platform::Os::Linux;
        env.idiom = nsh_core::platform::Idiom::Desktop;
    })
    .expect("input environment");
    app.first_render().expect("render");
    assert!(app.settle());
    app
}

#[test]
fn previous_next_and_rail_navigation_materialize_distant_turns() {
    let mut app = start(DESKTOP);
    app.wait_for("m-39").expect("initial end");
    let before = text_y(&app, "m-38").expect("last user turn");
    app.hover("role:button[Previous turn]")
        .expect("show previous");
    app.press("role:button[Previous turn]")
        .expect("previous turn");
    assert!(app.settle());
    let after_previous = text_y(&app, "m-38").expect("last turn remains in overscan");
    assert!(after_previous > before, "previous really moved the list");
    app.hover("role:button[Next turn]").expect("show next");
    app.press("role:button[Next turn]").expect("next turn");
    assert!(app.settle());
    assert!(
        text_y(&app, "m-38").expect("last turn remains in overscan") < after_previous,
        "next reverses the navigation"
    );
    app.press_at("id:timeline-minimap-rail", point(px(8.0), px(1.0)))
        .expect("rail start");
    app.wait_for("m-0").expect("first turn materialized");
    assert!(app.settle());
    app.press_key("Tab")
        .expect("focus the rail after its pointer blur");
    app.press_key("End").expect("last item");
    app.press_key("Enter").expect("jump to the last item");
    app.wait_for("m-39")
        .expect("keyboard materialized the last turn");
    app.press_key("Home").expect("first item");
    app.press_key("Space").expect("jump using space");
    app.wait_for("m-0")
        .expect("keyboard returned to the first turn");
    app.press_key("ArrowDown").expect("next rail item");
    app.press_key("Enter").expect("activate next rail item");
    app.wait_for("m-2").expect("second user turn");
}

fn text_y(app: &Headless, content: &str) -> Option<f32> {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| {
            node.text
                .as_ref()
                .is_some_and(|text| text.text.text == content)
        })
        .map(|node| node.bounds.origin.y.0)
}

#[test]
fn dense_threads_keep_a_bounded_native_strip_tree() {
    let app = start_root(DESKTOP, Root::of::<DenseMinimapFixture>());
    let strips = app
        .screen()
        .layout_nodes()
        .iter()
        .filter(|node| node.test_id.as_deref() == Some("minimap-strip"))
        .count();
    assert_eq!(strips, 256, "600 turns paint a fixed strip budget");
}

#[test]
fn narrow_layout_has_no_intercepting_rail_without_a_gutter() {
    let app = start(PHONE);
    let rail = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some("timeline-minimap-rail"));
    assert!(rail.is_none_or(|node| node.bounds.size.width <= px(0.0)));
}
