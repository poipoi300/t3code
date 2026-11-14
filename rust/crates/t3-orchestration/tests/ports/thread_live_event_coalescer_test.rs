use super::provider_event_ingestor_test::{decode, fixture};
use serde_json::json;
use t3_orchestration::{
    api::*,
    error::Result,
    ports::{
        runs_ingestion::thread_live_event_coalescer::*,
        transport::live_stream_budget::LiveStreamLimits,
    },
};
fn update(sequence: u64, id: &str, streaming: bool) -> OrchestrationV2ThreadStreamItem {
    let mut message = fixture("OrchestrationV2ConversationMessage");
    message["id"] = id.into();
    message["role"] = "assistant".into();
    message["streaming"] = streaming.into();
    message["text"] = format!("stream {sequence}").into();
    decode(
        json!({"kind":"event","sequence":sequence,"event":{"id":format!("event:{sequence}"),"threadId":"thread:test","runId":"run:test","occurredAt":"2026-09-08T00:00:00.000Z","type":"message.updated","payload":message}}),
    )
}
fn sequences(events: &[OrchestrationV2ThreadStreamItem]) -> Vec<u64> {
    events
        .iter()
        .filter_map(|item| {
            if let OrchestrationV2ThreadStreamItem::Event(item) = item {
                Some(item.sequence)
            } else {
                None
            }
        })
        .collect()
}
fn text_item(
    sequence: u64,
    id: &str,
    kind: &str,
    text: &str,
    completed: bool,
) -> OrchestrationV2ThreadStreamItem {
    let mut payload = super::provider_event_ingestor_test::item(kind, id);
    payload["text"] = text.into();
    payload["streaming"] = (!completed).into();
    payload["status"] = if completed { "completed" } else { "running" }.into();
    if kind == "assistant_message" {
        payload["messageId"] = id.into();
        payload["attachments"] = json!([]);
    }
    decode(
        json!({"kind":"event","sequence":sequence,"event":{"id":format!("event:{sequence}"),"threadId":"thread:test","runId":"run:test","occurredAt":"2026-09-08T00:00:00.000Z","type":"turn-item.updated","payload":payload}}),
    )
}
fn latest_states(
    events: &[OrchestrationV2ThreadStreamItem],
) -> std::collections::BTreeMap<String, serde_json::Value> {
    events
        .iter()
        .filter_map(|item| {
            let OrchestrationV2ThreadStreamItem::Event(item) = item else {
                return None;
            };
            let event = t3_contracts::stored::to_value(&item.event).expect("test value");
            Some((
                format!("{}:{}", event["type"], event["payload"]["id"]),
                event["payload"].clone(),
            ))
        })
        .collect()
}
#[test]
fn paired_message_item_families_and_every_surviving_replay_cursor_preserve_latest_state()
-> Result<()> {
    let events = vec![
        update(1, "shared", true),
        text_item(2, "shared", "assistant_message", "H", false),
        text_item(3, "reasoning", "reasoning", "Think", false),
        text_item(4, "unique", "reasoning", "Keep me", false),
        update(5, "shared", true),
        text_item(6, "shared", "assistant_message", "Hello", false),
        text_item(7, "reasoning", "reasoning", "Thinking done", false),
        update(8, "shared", false),
        text_item(9, "shared", "assistant_message", "Hello!", true),
    ];
    let survivors = coalesce_live_updated_events(events.clone())?;
    assert_eq!(survivors, events);
    let expected = latest_states(&events);
    for index in 0..survivors.len() {
        let mut received = survivors[..=index].to_vec();
        let cursor = sequences(&received).last().copied().unwrap();
        received.extend(coalesce_live_updated_events(
            events
                .iter()
                .filter(|item| sequences(std::slice::from_ref(item))[0] > cursor)
                .cloned()
                .collect(),
        )?);
        assert_eq!(latest_states(&received), expected, "cursor {cursor}");
    }
    let mut other_run = text_item(2, "same", "reasoning", "other turn", false);
    if let OrchestrationV2ThreadStreamItem::Event(item) = &mut other_run {
        let mut event = t3_contracts::stored::to_value(&item.event)?;
        event["runId"] = "run:other".into();
        event["payload"]["runId"] = "run:other".into();
        item.event = decode(event);
    }
    assert_eq!(
        sequences(&coalesce_live_updated_events(vec![
            text_item(1, "same", "reasoning", "old turn", false),
            other_run
        ])?),
        vec![1, 2]
    );
    Ok(())
}
#[test]
fn survivors_keep_sequence_and_identity_switches_preserve_timeline_order() -> Result<()> {
    assert_eq!(
        sequences(&coalesce_live_updated_events(vec![
            update(1, "a", true),
            update(2, "a", true),
            update(3, "a", false)
        ])?),
        vec![2, 3]
    );
    assert_eq!(
        sequences(&coalesce_live_updated_events(vec![
            update(1, "a", true),
            update(2, "b", true),
            update(3, "a", true)
        ])?),
        vec![1, 2, 3]
    );
    Ok(())
}
#[tokio::test(start_paused = true)]
async fn window_flushes_at_fifty_ms_and_synchronization_cannot_overtake_a_source_batch()
-> Result<()> {
    let coalescer = ThreadLiveEventCoalescer::new(LiveStreamLimits::default());
    coalescer.offer_all(vec![update(1, "a", true), update(2, "a", true)])?;
    tokio::time::advance(std::time::Duration::from_millis(50)).await;
    let batch = coalescer.pull().await?.unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(sequences(&[batch[0].value.clone()]), vec![2]);
    let marker = decode(json!({"kind":"synchronized"}));
    coalescer.offer_all(vec![update(3, "a", true), update(4, "a", true), marker])?;
    let batch = coalescer.pull().await?.unwrap();
    assert_eq!(batch.len(), 2);
    assert_eq!(
        sequences(
            &batch
                .iter()
                .map(|item| item.value.clone())
                .collect::<Vec<_>>()
        ),
        vec![4]
    );
    assert!(matches!(
        batch[1].value,
        OrchestrationV2ThreadStreamItem::Synchronized(_)
    ));
    coalescer.close().await;
    assert_eq!(coalescer.usage().retained_items, 0);
    Ok(())
}
#[tokio::test]
async fn replacement_is_charged_after_releasing_old_state_and_end_drains_the_final_batch()
-> Result<()> {
    let coalescer = ThreadLiveEventCoalescer::new(LiveStreamLimits {
        max_items: 1,
        max_serialized_bytes: 100_000,
    });
    coalescer.offer(update(1, "a", true))?;
    coalescer.offer(update(2, "a", true))?;
    assert_eq!(coalescer.usage().retained_items, 1);
    coalescer.end();
    let batch = coalescer.pull().await?.unwrap();
    assert_eq!(sequences(&[batch[0].value.clone()]), vec![2]);
    assert!(coalescer.pull().await?.is_none());
    assert_eq!(coalescer.usage().retained_items, 0);
    coalescer.close().await;
    Ok(())
}
#[tokio::test]
async fn overflow_discards_pending_items_and_close_releases_in_flight_charges() -> Result<()> {
    let coalescer = ThreadLiveEventCoalescer::new(LiveStreamLimits {
        max_items: 1,
        max_serialized_bytes: 100_000,
    });
    coalescer.offer(update(1, "a", true))?;
    assert!(coalescer.offer(update(2, "b", true)).is_err());
    assert!(coalescer.failed().await);
    assert_eq!(coalescer.usage().retained_items, 0);
    coalescer.close().await;
    let coalescer = ThreadLiveEventCoalescer::new(LiveStreamLimits::default());
    coalescer.offer(update(1, "a", false))?;
    assert_eq!(coalescer.pull().await?.unwrap().len(), 1);
    assert_eq!(coalescer.usage().retained_items, 1);
    coalescer.close().await;
    assert_eq!(coalescer.usage().retained_items, 0);
    assert!(coalescer.pull().await?.is_none());
    Ok(())
}

#[tokio::test]
async fn finite_sources_flush_before_eof_and_upstream_failure_reaches_a_waiting_consumer()
-> Result<()> {
    for failure in [false, true] {
        let coalescer = ThreadLiveEventCoalescer::new(LiveStreamLimits::default());
        let (sender, source) = tokio::sync::mpsc::channel(2);
        coalescer.consume_source(source);
        sender
            .send(Ok(vec![update(1, "a", true), update(2, "a", true)]))
            .await
            .unwrap();
        if failure {
            sender
                .send(Err(t3_orchestration::error::Error::Unsupported(
                    "upstream failed",
                )))
                .await
                .unwrap();
        }
        drop(sender);
        if failure {
            assert!(matches!(
                coalescer.pull().await,
                Err(t3_orchestration::error::Error::Unsupported(
                    "upstream failed"
                ))
            ));
            assert!(coalescer.pull().await?.is_none());
        } else {
            let batch = coalescer.pull().await?.unwrap();
            assert_eq!(
                sequences(
                    &batch
                        .iter()
                        .map(|item| item.value.clone())
                        .collect::<Vec<_>>()
                ),
                vec![2]
            );
            assert!(coalescer.pull().await?.is_none());
        }
        coalescer.close().await;
        assert_eq!(coalescer.usage().retained_items, 0);
    }
    Ok(())
}
#[tokio::test]
async fn dropping_the_owner_cancels_its_source_and_does_not_leave_a_strong_task_cycle() -> Result<()>
{
    let coalescer = ThreadLiveEventCoalescer::new(LiveStreamLimits::default());
    let weak = std::sync::Arc::downgrade(&coalescer);
    let (sender, source) = tokio::sync::mpsc::channel(1);
    coalescer.consume_source(source);
    coalescer.offer(update(1, "a", true))?;
    drop(coalescer);
    assert!(weak.upgrade().is_none());
    sender.closed().await;
    Ok(())
}
#[tokio::test]
async fn repeated_tool_snapshots_exceeding_eight_mib_in_total_charge_only_the_latest_and_reject_an_oversized_replacement()
-> Result<()> {
    let coalescer = ThreadLiveEventCoalescer::new(LiveStreamLimits {
        max_items: 1,
        max_serialized_bytes: 100_000,
    });
    for sequence in 1..=128 {
        let mut event = update(sequence, "a", true);
        if let OrchestrationV2ThreadStreamItem::Event(event) = &mut event
            && let OrchestrationV2DomainEvent::MessageUpdated(event) = &mut event.event
        {
            event.payload.text = "x".repeat(70_000);
        }
        coalescer.offer(event)?;
    }
    assert_eq!(coalescer.usage().retained_items, 1);
    let mut event = update(129, "a", true);
    if let OrchestrationV2ThreadStreamItem::Event(event) = &mut event
        && let OrchestrationV2DomainEvent::MessageUpdated(event) = &mut event.event
    {
        event.payload.text = "x".repeat(100_000);
    }
    assert!(coalescer.offer(event).is_err());
    assert!(coalescer.failed().await);
    coalescer.close().await;
    Ok(())
}
