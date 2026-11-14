use super::{
    bounded_snapshot_transport_test::{commit, seed, service},
    thread_stream_test::command,
};

use serde_json::json;
use std::sync::Arc;
use t3_contracts::orchestration_v2::*;
use t3_orchestration::{
    error::Result, ports::transport::thread_find::*, projections::store::ProjectionStore,
    testing::TestDatabase,
};

struct PlainText;
impl ThreadFindText for PlainText {
    fn segments(
        &self,
        item: &OrchestrationV2TurnItem,
        _cwd: Option<&str>,
        _skills: &[SearchThreadInputSkillsItem],
    ) -> Result<Vec<String>> {
        let value = serde_json::to_value(item)?;
        Ok(vec![value["text"].as_str().unwrap_or("").into()])
    }
    fn occurrences(&self, text: &str, query: &str) -> u64 {
        text.to_lowercase().matches(&query.to_lowercase()).count() as u64
    }
    fn embeds_result(&self, _item: &OrchestrationV2TurnItem) -> bool {
        false
    }
}
async fn fixture(texts: &[&str]) -> (TestDatabase, ThreadFind) {
    let fixture = TestDatabase::memory().expect("test value");
    let (_, sink) = service(&fixture);
    seed(&fixture, &sink).await.expect("test value");
    for (i, text) in texts.iter().enumerate() {
        let mut item = command(&format!("item-{i}"), i as u64 + 1);
        item["type"] = "assistant_message".into();
        item["messageId"] = format!("message-{i}").into();
        item["text"] = (*text).into();
        item["streaming"] = false.into();
        item["attachments"] = json!([]);
        commit(&sink,vec![serde_json::from_value(json!({"id":format!("event-{i}"),"threadId":"thread","type":"turn-item.updated","occurredAt":t3_orchestration::testing::TEST_TIME,"payload":item})).expect("test value")]).await.expect("test value");
    }
    let find = ThreadFind::new(
        Arc::new(ProjectionStore::new(fixture.stores.clone())),
        Arc::new(PlainText),
    );
    (fixture, find)
}
fn input(extra: serde_json::Value) -> OrchestrationV2SearchThreadInput {
    let mut value = json!({"threadId":"thread","query":"rust"});
    value
        .as_object_mut()
        .expect("test value")
        .extend(extra.as_object().expect("test value").clone());
    serde_json::from_value(value).expect("test value")
}

#[tokio::test]
async fn repeated_find_uses_completed_counts_and_a_new_revision_invalidates_them() {
    let (fixture, find) = fixture(&["rust rust"]).await;
    assert_eq!(
        find.search_thread(input(json!({})))
            .await
            .unwrap()
            .total_matches,
        2
    );
    let mut repeated = find.search_thread_stream(input(json!({}))).await.unwrap();
    let frame = repeated.pull().await.unwrap().unwrap();
    assert_eq!(frame.complete, None);
    assert_eq!(frame.total_matches, 2);
    assert!(repeated.pull().await.unwrap().is_none());
    let mut captured = find
        .search_thread_stream(input(json!({"query":"RUST"})))
        .await
        .unwrap();
    assert_eq!(
        captured.pull().await.unwrap().unwrap().complete,
        Some(false)
    );
    let (_, sink) = service(&fixture);
    let mut item = command("item-later", 2);
    item["type"] = "assistant_message".into();
    item["messageId"] = "message-later".into();
    item["text"] = "rust".into();
    item["streaming"] = false.into();
    item["attachments"] = json!([]);
    commit(&sink,vec![serde_json::from_value(json!({"id":"event-later","threadId":"thread","type":"turn-item.updated","occurredAt":t3_orchestration::testing::TEST_TIME,"payload":item})).unwrap()]).await.unwrap();
    let captured = captured.pull().await.unwrap().unwrap();
    assert_eq!(captured.total_matches, 2);
    let frame = find.search_thread(input(json!({}))).await.unwrap();
    assert_eq!(frame.total_matches, 3);
}
#[tokio::test]
async fn first_frame_finds_from_anchor_then_full_frame_counts_and_navigates() {
    let (_fixture, find) = fixture(&["rust rust", "nothing", "RUST"]).await;
    let mut stream = find
        .search_thread_stream(input(
            json!({"start":{"entryId":"message-2","occurrence":0}}),
        ))
        .await
        .unwrap();
    let first = stream.pull().await.unwrap().unwrap();
    assert_eq!(first.complete, Some(false));
    assert_eq!(first.total_matches, 1);
    assert_eq!(first.r#match.unwrap().entry_id.0, "message-2");
    let complete = stream.pull().await.unwrap().unwrap();
    assert_eq!(complete.total_matches, 3);
    assert_eq!(complete.active_index, 2);
    assert_eq!(complete.navigation.as_ref().unwrap().len(), 2);
    assert!(stream.pull().await.unwrap().is_none());
}
#[tokio::test]
async fn explicit_index_clamps_and_offsets_wrap_both_directions_without_partial_frames() {
    let (_fixture, find) = fixture(&["rust rust", "rust"]).await;
    for (extra, index, entry, occurrence) in [
        (json!({"index":99}), 2, "message-1", 0),
        (json!({"offset":-1}), 2, "message-1", 0),
        (json!({"offset":4}), 1, "message-0", 1),
    ] {
        let mut stream = find.search_thread_stream(input(extra)).await.unwrap();
        let frame = stream.pull().await.unwrap().unwrap();
        assert_eq!(frame.complete, None);
        assert_eq!(frame.active_index, index);
        let found = frame.r#match.unwrap();
        assert_eq!(found.entry_id.0, entry);
        assert_eq!(found.occurrence, occurrence);
        assert!(stream.pull().await.unwrap().is_none());
    }
}
#[tokio::test]
async fn navigation_is_a_wrapped_seventeen_entry_window_and_no_match_is_complete() {
    let texts = vec!["rust"; 25];
    let (_fixture, find) = fixture(&texts).await;
    let frame = find.search_thread(input(json!({"index":0}))).await.unwrap();
    let nav = frame.navigation.unwrap();
    assert_eq!(nav.len(), 17);
    assert_eq!(nav[0].entry_id.0, "message-17");
    assert_eq!(nav[8].entry_id.0, "message-0");
    let mut query = input(json!({}));
    query.query = "absent".to_owned().into();
    let frame = find.search_thread(query).await.unwrap();
    assert_eq!(frame.total_matches, 0);
    assert!(frame.r#match.is_none());
    assert_eq!(frame.navigation, Some(vec![]));
    let many = "rust ".repeat(3000);
    let (_fixture, find) = fixture(&[&many]).await;
    let frame = find
        .search_thread(input(json!({"index":1499})))
        .await
        .unwrap();
    assert_eq!(frame.total_matches, 3000);
    assert_eq!(frame.r#match.unwrap().occurrence, 1499);
    let navigation = frame.navigation.unwrap();
    assert_eq!(navigation.len(), 1);
    assert_eq!(navigation[0].count, 3000);
}
