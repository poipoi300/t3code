#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use t3_acp::{SessionUpdate, TextBatcher};
use tokio::time::Instant;

fn update(session: &str, value: Value) -> SessionUpdate {
    SessionUpdate {
        session_id: session.into(),
        update: value.clone(),
        raw: json!({"sessionId":session,"update":value}),
    }
}
fn text(session: &str, kind: &str, value: &str) -> SessionUpdate {
    update(
        session,
        json!({"sessionUpdate":kind,"content":{"type":"text","text":value}}),
    )
}

#[tokio::test(start_paused = true)]
async fn first_immediate_later_snapshots_and_final_flush_are_ordered() {
    let mut batcher = TextBatcher::default();
    let now = Instant::now();
    let first = batcher.push(&text("s", "agent_message_chunk", "a"), now);
    assert_eq!(first[0].text, "a");
    assert!(!first[0].complete);
    assert!(
        batcher
            .push(&text("s", "agent_message_chunk", "b"), now)
            .is_empty()
    );
    assert!(
        batcher
            .push(&text("s", "agent_message_chunk", "c"), now)
            .is_empty()
    );
    assert_eq!(batcher.next_deadline(), Some(now + TextBatcher::INTERVAL));
    assert!(batcher.flush_due(now).is_empty());
    tokio::time::advance(TextBatcher::INTERVAL).await;
    assert_eq!(batcher.flush_due(Instant::now())[0].text, "abc");
    assert!(
        batcher
            .push(&text("s", "agent_message_chunk", "d"), Instant::now())
            .is_empty()
    );
    let final_text = batcher.push(
        &update("s", json!({"sessionUpdate":"tool_call","toolCallId":"t"})),
        Instant::now(),
    );
    assert_eq!(final_text[0].text, "abcd");
    assert!(final_text[0].complete);
    tokio::time::advance(TextBatcher::INTERVAL).await;
    assert!(batcher.flush_due(Instant::now()).is_empty());
    assert!(batcher.finish("s").is_empty());
}

#[test]
fn reasoning_message_identity_sessions_and_metadata_do_not_merge() {
    let now = Instant::now();
    let mut batcher = TextBatcher::default();
    batcher.push(&text("s", "agent_thought_chunk", "think"), now);
    let next = batcher.push(&text("s", "agent_message_chunk", "say"), now);
    assert_eq!(next.len(), 2);
    assert!(next[0].complete);
    assert_eq!(next[1].kind, "agent_message_chunk");
    assert!(
        batcher
            .push(
                &update(
                    "s",
                    json!({"sessionUpdate":"usage_update","used":1,"size":100})
                ),
                now
            )
            .is_empty()
    );
    let mut named = text("s", "agent_message_chunk", " more");
    named.update["messageId"] = json!("m1");
    assert!(batcher.push(&named, now).is_empty());
    let other = batcher.push(&text("other", "agent_message_chunk", "separate"), now);
    assert_eq!(other.len(), 1);
    named.update["messageId"] = json!("m2");
    let changed = batcher.push(&named, now);
    assert_eq!(changed.len(), 2);
    assert_eq!(changed[0].message_id.as_deref(), Some("m1"));
    assert_ne!(changed[0].segment, changed[1].segment);
    let finals = batcher.finish_all();
    assert_eq!(finals.len(), 2);
    assert!(finals.iter().all(|s| s.complete));
}

#[test]
fn named_messages_reopen_after_tools_but_not_across_turns() {
    let now = Instant::now();
    let mut batcher = TextBatcher::default();
    let mut chunk = text("s", "agent_message_chunk", "a");
    chunk.update["messageId"] = json!("m");
    let first = batcher.push(&chunk, now);
    batcher.push(
        &update("s", json!({"sessionUpdate":"tool_call","toolCallId":"t"})),
        now,
    );
    chunk.update["content"]["text"] = json!("b");
    let reopened = batcher.push(&chunk, now);
    assert_eq!(reopened[0].text, "ab");
    assert_eq!(reopened[0].segment, first[0].segment);
    batcher.end_turn("s");
    let next = batcher.push(&chunk, now);
    assert_eq!(next[0].text, "b");
    assert_ne!(next[0].segment, first[0].segment);
}
