use super::{
    bounded_snapshot_transport_test::{commit, seed, service},
    thread_stream_test::projection_value,
};
use serde_json::json;
use std::sync::Arc;
use t3_contracts::thread_search::*;
use t3_orchestration::{
    error::Result,
    ports::transport::{message_search_index::*, thread_search::*},
    testing::{TEST_TIME, TestDatabase},
};

struct PlainPolicy;
impl ThreadSearchPolicy for PlainPolicy {
    fn message_search(&self, query: &str, index: bool) -> Option<MessageSearchRequest> {
        (query.len() >= 3).then(|| MessageSearchRequest {
            match_query: index.then(|| format!("\"{query}\"")),
            substrings: if index { vec![] } else { vec![query.into()] },
            short_spellings: vec![],
        })
    }
    fn shown_message(&self, text: &str) -> String {
        text.into()
    }
    fn match_message(&self, text: &str, query: &str) -> Option<(f64, usize)> {
        text.to_lowercase()
            .find(&query.to_lowercase())
            .map(|offset| (1.0, text[..offset].encode_utf16().count()))
    }
    fn rank_messages(
        &self,
        hits: &[MessageHit],
        _query: &str,
        _now: i64,
        _picks: bool,
    ) -> Result<Vec<usize>> {
        Ok((0..hits.len()).collect())
    }
    fn rank_titles(
        &self,
        rows: &[TitleCandidate],
        query: &str,
        _environment: Option<&str>,
        _now: i64,
        _picks: bool,
    ) -> Result<Vec<usize>> {
        Ok(rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                row.thread["title"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&query.to_lowercase())
            })
            .map(|(i, _)| i)
            .collect())
    }
}
async fn message(
    sink: &t3_orchestration::runtime::commit::EventSink,
    id: &str,
    thread: &str,
    role: &str,
    text: &str,
    agent: bool,
    streaming: bool,
) {
    let payload = json!({"id":id,"threadId":thread,"runId":null,"nodeId":null,"role":role,"text":text,"attachments":[],"streaming":streaming,"createdBy":if agent{"agent"}else{"user"},"creationSource":"web","createdAt":TEST_TIME,"updatedAt":TEST_TIME});
    commit(sink,vec![serde_json::from_value(json!({"id":format!("event-{id}-{text}"),"threadId":thread,"type":"message.updated","occurredAt":TEST_TIME,"payload":payload})).expect("test value")]).await.expect("test value");
}
#[tokio::test]
async fn indexed_and_scan_search_pick_user_messages_and_exclude_streams_notices_and_hidden_threads()
{
    for indexed in [true, false] {
        let fixture = TestDatabase::memory().unwrap();
        let (_, sink) = service(&fixture);
        seed(&fixture, &sink).await.unwrap();
        let mut child = projection_value(0)["thread"].clone();
        child["id"] = "child".into();
        child["lineage"]["relationshipToParent"] = "subagent".into();
        child["lineage"]["parentThreadId"] = "thread".into();
        commit(&sink,vec![serde_json::from_value(json!({"id":"child-created","threadId":"child","type":"thread.created","occurredAt":TEST_TIME,"payload":child})).unwrap()]).await.unwrap();
        message(
            &sink,
            "assistant",
            "thread",
            "assistant",
            "needle in an answer",
            true,
            false,
        )
        .await;
        message(
            &sink,
            "user",
            "thread",
            "user",
            "newer needle question",
            false,
            false,
        )
        .await;
        message(
            &sink,
            "notice",
            "thread",
            "user",
            "needle notice",
            true,
            false,
        )
        .await;
        message(
            &sink,
            "stream",
            "thread",
            "assistant",
            "needle streaming",
            true,
            true,
        )
        .await;
        message(
            &sink,
            "hidden",
            "child",
            "assistant",
            "needle hidden",
            true,
            false,
        )
        .await;
        message(
            &sink,
            "async-answer:1",
            "thread",
            "user",
            "needle folded answer",
            false,
            false,
        )
        .await;
        if !indexed {
            fixture.stores.transaction(|tx|{tx.execute_batch("DROP TRIGGER orchestration_v2_message_search_insert;DROP TRIGGER orchestration_v2_message_search_update;DROP TRIGGER orchestration_v2_message_search_delete;DROP TABLE orchestration_v2_message_search;")?;Ok(())}).await.unwrap();
        }
        let search = ThreadSearch::new(fixture.stores.clone(), Arc::new(PlainPolicy));
        let matches = search
            .search(OrchestrationSearchThreadsInput {
                query: "needle".into(),
                limit: None,
            })
            .await
            .unwrap()
            .matches;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].source, OrchestrationThreadSearchSource::User);
        assert_eq!(matches[0].message_id.as_ref().unwrap().0, "user");
        assert_eq!(matches[0].snippet, "newer needle question");
        assert_eq!(serde_json::to_value(&matches[0]).unwrap()["hits"], 2);
        assert!(
            search
                .search(OrchestrationSearchThreadsInput {
                    query: "ne%le".into(),
                    limit: None
                })
                .await
                .unwrap()
                .matches
                .is_empty()
        );
    }
}
#[tokio::test]
async fn triggers_rebuild_exclude_bootstrap_and_notice_rows_and_remove_settled_rewrites() {
    let fixture = TestDatabase::memory().unwrap();
    let (_, sink) = service(&fixture);
    seed(&fixture, &sink).await.unwrap();
    fixture
        .stores
        .transaction(|tx| {
            recreate_message_search_triggers(tx)?;
            Ok(())
        })
        .await
        .unwrap();
    let bootstrap = "[User attached one or more files without additional text. Respond using the conversation context and the attached files.]";
    for (i, text) in [
        bootstrap,
        &format!("Ultrathink:\n{bootstrap}"),
        &format!("uLtRaThInK:\r\n\t\u{a0}{bootstrap}"),
    ]
    .iter()
    .enumerate()
    {
        message(
            &sink,
            &format!("placeholder-{i}"),
            "thread",
            "user",
            text,
            false,
            false,
        )
        .await;
    }
    message(
        &sink,
        "ordinary",
        "thread",
        "user",
        "Review the attached files in context",
        false,
        false,
    )
    .await;
    fixture
        .stores
        .transaction(rebuild_message_search)
        .await
        .unwrap();
    let search = ThreadSearch::new(fixture.stores.clone(), Arc::new(PlainPolicy));
    let hits = search
        .search(OrchestrationSearchThreadsInput {
            query: "attached".into(),
            limit: None,
        })
        .await
        .unwrap()
        .matches;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].message_id.as_ref().unwrap().0, "ordinary");
    message(
        &sink,
        "ordinary",
        "thread",
        "user",
        "different words",
        false,
        false,
    )
    .await;
    assert!(
        search
            .search(OrchestrationSearchThreadsInput {
                query: "attached".into(),
                limit: None
            })
            .await
            .unwrap()
            .matches
            .is_empty()
    );
}
#[test]
fn snippets_match_typescript_whitespace_marks_late_match_and_surrogate_expectations() {
    assert_eq!(
        build_search_snippet("Use **bold** and `code` here", 0),
        "Use bold and code here"
    );
    assert_eq!(
        build_search_snippet("Call __init__ first", 0),
        "Call __init__ first"
    );
    assert_eq!(
        build_search_snippet(&format!("{}needle", "word ".repeat(20)), 100),
        "…word word word word needle"
    );
    let text = format!("x{}needle", "🚀".repeat(200));
    let output = build_search_snippet(&text, 401);
    assert!(output.ends_with("needle"));
    assert!(!output.contains('\u{fffd}'));
}
