//! Find drives the real viewport and index from client timeline projections.

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
use t3_client::view_logic::session::{TimelineContent, TimelineEntry};
use t3_ui::runtime::Shared;
use t3_ui::screens::thread::blocks::flatten::MessageFlattener;
use t3_ui::screens::thread::find::{FindKind, ThreadFind, thread_find_bar};
use t3_ui::screens::thread::viewport::{TimelineViewport, timeline_row_keys};

#[component]
fn FindFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let entries: Vec<Rc<TimelineEntry>> = (0..200).map(|at| Rc::new(serde_json::from_value(json!({
        "kind":"message", "id":format!("m-{at}"), "createdAt":"2026-11-14T12:00:00Z",
        "message":{"id":format!("m-{at}"),"role":"user","text":if at == 0 { "needle needle".to_owned() } else if at == 150 { "needle".to_owned() } else { format!("Message {at}") },
            "streaming":false,"runId":null,"createdAt":"2026-11-14T12:00:00Z","updatedAt":"2026-11-14T12:00:00Z"}
    })).expect("client entry"))).collect();
    let input = MessagesTimelineRowsInput {
        timeline_entries: entries.clone(),
        ..Default::default()
    };
    let projection = derive_messages_timeline_rows_with_state(&input, None);
    let rows = computed(move || Shared(projection.rows.clone()));
    let viewport = TimelineViewport::new(
        ThreadKey {
            environment: EnvironmentId("find-fixture".into()),
            thread: ThreadId("thread".into()),
        },
        timeline_row_keys(rows),
    );
    let find = ThreadFind::new(viewport, |block| {
        block.message_id.strip_prefix("m-")?.parse().ok()
    });
    for (at, entry) in entries.iter().enumerate() {
        if let TimelineContent::Message { message, .. } = &entry.content {
            let mut flattener = MessageFlattener::new(&entry.id);
            find.apply(
                at,
                &entry.id,
                FindKind::User,
                &flattener.replace(&entry.id, &message.text),
            );
        }
    }
    let list = virtual_list(200usize, move |at| {
        view()
            .h(px(80.0))
            .test_id("find-row")
            .child(text(move || rows.with(|rows| rows[at.index()].id.clone())))
    })
    .flex_1()
    .test_id("find-list");
    view()
        .w_full()
        .h_full()
        .child(
            nsh_base::button()
                .test_id("open-find")
                .child(text("Find"))
                .on_press(move || find.open()),
        )
        .child(
            view()
                .flex_1()
                .min_h(px(0.0))
                .relative()
                .child(viewport.attach(list))
                .child(thread_find_bar(find)),
        )
}

fn start(window: (f32, f32)) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<FindFixture>())
    })
    .expect("find fixture");
    app.first_render().expect("render");
    assert!(app.settle());
    app
}

#[test]
fn offscreen_matches_cycle_wrap_and_close_at_both_sizes() {
    for size in [DESKTOP, PHONE] {
        let mut app = start(size);
        assert!(!app.screen().shows("m-0"), "target starts unmounted");
        app.press("id:open-find").expect("open");
        app.type_text("id:thread-find-input", "needle")
            .expect("query");
        app.wait_for("1/3").expect("all unmounted source counted");
        app.wait_for("m-0").expect("first match materialized");
        app.press_key("Enter").expect("next from the query field");
        app.wait_for("2/3").expect("Enter advances once");
        app.press_key("Shift+Enter")
            .expect("previous from the query field");
        app.wait_for("1/3").expect("Shift+Enter reverses once");
        app.press("role:button[Previous match]")
            .expect("wrap previous");
        app.wait_for("3/3").expect("wrapped count");
        app.wait_for("m-150").expect("distant match mounted");
        app.press("role:button[Next match]").expect("wrap next");
        app.wait_for("1/3").expect("wrapped start");
        app.press("role:button[Next match]")
            .expect("same row occurrence");
        app.wait_for("2/3").expect("second occurrence");
        app.press("role:button[Close find]").expect("close");
        assert!(!app.screen().shows("2/3"));
        assert!(app.settle());
    }
}

#[test]
fn an_empty_result_disables_navigation_and_reopening_clears_the_old_query() {
    let mut app = start(PHONE);
    app.press("id:open-find").expect("open");
    app.type_text("id:thread-find-input", "missing")
        .expect("query");
    app.wait_for("0/0").expect("empty");
    let _ = app.press("role:button[Next match]");
    assert!(app.screen().shows("0/0"));
    app.press("role:button[Close find]").expect("close");
    app.press("id:open-find").expect("reopen");
    assert!(!app.screen().shows("0/0"));
}
