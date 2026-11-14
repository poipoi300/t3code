//! TL-SELECT-3: the mounted key scope, retained source and native clipboard failure state.

use std::rc::Rc;

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRowsInput, derive_messages_timeline_rows_with_state,
};
use t3_client::view_logic::session::TimelineEntry;
use t3_ui::screens::thread::blocks::list::BlockListModel;
use t3_ui::screens::thread::selection::keys::{TimelineSelection, timeline_selection_scope};

use crate::support::{DESKTOP, PHONE};

#[component]
fn SelectionKeysFixture() -> impl IntoElement {
    let entry: TimelineEntry = serde_json::from_value(json!({
        "id": "reply", "kind": "message", "createdAt": "2026-11-14T12:00:00Z",
        "message": {"id": "reply", "role": "assistant", "text": format!("- First\n\n```rust\n{}```\n\nLast 🦀", "let value = 1;\n".repeat(500)),
            "streaming": false, "runId": null, "createdAt": "2026-11-14T12:00:00Z", "updatedAt": "2026-11-14T12:00:00Z"}
    })).expect("client entry");
    let projection = derive_messages_timeline_rows_with_state(
        &MessagesTimelineRowsInput {
            timeline_entries: vec![Rc::new(entry)],
            ..Default::default()
        },
        None,
    );
    let mut blocks = BlockListModel::default();
    blocks.update(&projection.rows);
    let selection = TimelineSelection::new();
    for row in projection.rows.iter() {
        if let Some(document) = blocks.document(&row.id) {
            selection
                .retain_document(&row.id, document)
                .expect("existing retained source");
        }
    }
    view()
        .w_full()
        .h_full()
        .child(text(move || {
            SharedString::from(if selection.has_selection() {
                "selected"
            } else {
                "none"
            })
        }))
        .child(text(move || {
            format!("copy {:?}", selection.copy_status.get())
        }))
        .child(timeline_selection_scope(
            selection,
            text(SharedString::from("Last 🦀")),
        ))
        .child(
            nsh_base::input()
                .test_id("composer")
                .label(SharedString::from("Composer")),
        )
}

fn fixture(window: (f32, f32)) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<SelectionKeysFixture>())
    };
    // NSH's headless driver delivers raw KeyCapture events only in its Linux simulation.
    config.environment.os = nsh_core::platform::Os::Linux;
    let mut app = Headless::start(config).expect("selection scope engine");
    app.first_render().expect("mounted");
    app
}

#[test]
fn select_all_and_copy_use_the_scope_and_do_not_fall_back_to_native_partial_text() {
    for size in [DESKTOP, PHONE] {
        let mut app = fixture(size);
        app.set_clipboard(Some("previous clipboard".into()));
        app.press_key("tab").expect("focus timeline");
        app.press_key("mod+a").expect("whole retained document");
        app.wait_for("selected").expect("model selected");
        app.press_key("mod+c").expect("copy request");
        app.wait_for("copy Failed(SerializerUnavailable)")
            .expect("honest missing serializer");
        assert_eq!(app.screen().clipboard(), Some("previous clipboard"));
        app.type_text("id:composer", "composer draft")
            .expect("move to composer");
        app.press_key("mod+a").expect("composer select all");
        app.press_key("mod+c").expect("composer copy");
        assert!(
            app.screen().shows("copy Failed(SerializerUnavailable)"),
            "composer commands never dispatch a timeline action"
        );
        assert!(
            app.screen().shows("selected"),
            "composer select-all does not rewrite timeline selection"
        );
        app.press_key("shift+tab").expect("focus timeline again");
        app.press_key("mod+a").expect("new timeline selection");
        app.wait_for("copy Idle")
            .expect("a real selection action resets copy failure");
    }
}
