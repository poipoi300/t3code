//! TL-SELECT-1: selection consumes the production block list's retained documents.

use std::rc::Rc;

use serde_json::json;
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRowsInput, derive_messages_timeline_rows_with_state,
};
use t3_client::view_logic::session::TimelineEntry;
use t3_ui::screens::thread::blocks::list::BlockListModel;
use t3_ui::screens::thread::selection::model::SelectionDocument;

#[test]
fn selection_uses_the_lists_complete_source_with_no_mounted_or_injected_rows() {
    let source = format!(
        "- Read\n- Write\n\n```rust\n{}```\n\nAfter 🦀",
        "let value = 1;\n".repeat(501)
    );
    let entry: TimelineEntry = serde_json::from_value(json!({
        "id": "answer", "kind": "message", "createdAt": "2026-11-14T12:00:00Z",
        "message": {"id": "answer", "role": "assistant", "text": source,
            "streaming": false, "runId": null, "createdAt": "2026-11-14T12:00:00Z",
            "updatedAt": "2026-11-14T12:00:00Z"}
    }))
    .expect("client timeline entry");
    let projection = derive_messages_timeline_rows_with_state(
        &MessagesTimelineRowsInput {
            timeline_entries: vec![Rc::new(entry)],
            ..Default::default()
        },
        None,
    );
    let mut list = BlockListModel::default();
    list.update(&projection.rows);
    let mut selection = SelectionDocument::default();
    let mut ids = Vec::new();
    for row in projection.rows.iter() {
        if let Some(document) = list.document(&row.id) {
            selection
                .retain_document(&row.id, document)
                .expect("same retained document");
            ids.push(row.id.clone());
        }
    }
    selection.set_order(&ids).expect("client order");
    assert!(selection.select_all());
    let selected = selection.selected_sources().expect("selection source");
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].source().as_deref(), Some(source.as_str()));
    assert!(selected[0].document == list.document(&ids[0]).expect("list source"));
    list.update(&Rc::new(Vec::new()));
    selection.set_order(&[]).expect("thread cleared");
    assert!(!selection.has_selection());
    assert!(selection.selected_sources().expect("cleared").is_empty());
}
