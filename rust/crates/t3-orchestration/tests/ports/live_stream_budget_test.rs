use serde_json::json;
use std::sync::Arc;
use t3_orchestration::ports::transport::live_stream_budget::*;
fn item(sequence: u64, text: &str) -> Arc<SharedLiveItem<serde_json::Value>> {
    SharedLiveItem::new(json!({"sequence":sequence,"text":text}), sequence).expect("test value")
}
#[tokio::test]
async fn overflow_releases_queued_items_but_keeps_unacknowledged_delivery_charged() {
    let buffer = LiveBuffer::new(LiveStreamLimits {
        max_items: 3,
        ..Default::default()
    });
    buffer.push(item(1, "first"), None).unwrap();
    let batch = buffer.pull().await.unwrap().unwrap();
    buffer.push(item(2, "second"), None).unwrap();
    buffer.push(item(3, "third"), None).unwrap();
    assert_eq!(buffer.usage().retained_items, 3);
    assert!(buffer.push(item(4, "fourth"), None).is_err());
    buffer.failed().await;
    assert_eq!(buffer.usage().retained_items, 1);
    assert_eq!(batch[0].sequence, 1);
    assert!(buffer.pull().await.is_err());
    assert_eq!(buffer.usage(), LiveStreamUsage::default());
}
#[tokio::test]
async fn an_unacknowledged_full_state_cannot_be_replaced_under_either_limit() {
    for byte_limit in [false, true] {
        let first = item(1, &"é".repeat(20));
        let next = item(2, &"é".repeat(20));
        let limits = if byte_limit {
            LiveStreamLimits {
                max_serialized_bytes: first.serialized_bytes,
                ..Default::default()
            }
        } else {
            LiveStreamLimits {
                max_items: 1,
                ..Default::default()
            }
        };
        let buffer = LiveBuffer::new(limits);
        buffer.push(first, Some("same".into())).unwrap();
        assert_eq!(buffer.pull().await.unwrap().unwrap()[0].sequence, 1);
        assert!(buffer.push(next, Some("same".into())).is_err());
        assert!(buffer.pull().await.is_err());
    }
}
#[tokio::test]
async fn queued_replacements_reuse_their_budget_and_sort_survivors_by_sequence() {
    let buffer = LiveBuffer::new(LiveStreamLimits {
        max_items: 2,
        ..Default::default()
    });
    buffer.push(item(1, "old"), Some("a".into())).unwrap();
    buffer.push(item(2, "middle"), Some("b".into())).unwrap();
    buffer.push(item(3, "latest"), Some("a".into())).unwrap();
    buffer.push(item(1, "stale"), Some("a".into())).unwrap();
    let batch = buffer.pull().await.unwrap().unwrap();
    assert_eq!(
        batch.iter().map(|v| v.sequence).collect::<Vec<_>>(),
        vec![2, 3]
    );
    assert_eq!(buffer.usage().retained_items, 2);
    buffer.close();
    assert!(buffer.pull().await.unwrap().is_none());
    assert_eq!(buffer.usage(), LiveStreamUsage::default());
}
#[tokio::test]
async fn finite_replay_can_exceed_limits_when_charged_one_delivery_at_a_time() {
    let buffer = LiveBuffer::new(LiveStreamLimits {
        max_items: 2,
        max_serialized_bytes: 100,
    });
    for sequence in 1..=20 {
        buffer
            .push(item(sequence, "historical event"), None)
            .unwrap();
        assert_eq!(buffer.pull().await.unwrap().unwrap()[0].sequence, sequence);
        buffer.acknowledge();
    }
    buffer.close();
    assert!(buffer.pull().await.unwrap().is_none());
}
#[tokio::test]
async fn failure_wakes_a_blocked_pull_without_polling() {
    let buffer = Arc::new(LiveBuffer::<serde_json::Value>::new(Default::default()));
    let (started, waiting) = tokio::sync::oneshot::channel();
    let reader = {
        let buffer = buffer.clone();
        tokio::spawn(async move {
            started.send(()).unwrap();
            buffer.pull().await
        })
    };
    waiting.await.unwrap();
    buffer.fail();
    assert!(reader.await.unwrap().is_err());
}

#[tokio::test]
async fn a_failure_watcher_cannot_consume_a_delivery_wakeup() {
    use std::{future::Future, task::Poll};
    let buffer = Arc::new(LiveBuffer::new(Default::default()));
    let failed = buffer.failed();
    tokio::pin!(failed);
    std::future::poll_fn(|cx| {
        assert!(failed.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    let (started, waiting) = tokio::sync::oneshot::channel();
    let reader = {
        let buffer = buffer.clone();
        tokio::spawn(async move {
            started.send(()).unwrap();
            buffer.pull().await
        })
    };
    waiting.await.unwrap();
    buffer.push(item(1, "delivered"), None).unwrap();
    assert_eq!(reader.await.unwrap().unwrap().unwrap()[0].sequence, 1);
}

#[tokio::test]
async fn overflow_cancels_a_blocked_high_water_read_and_releases_the_live_subscription() {
    use super::bounded_snapshot_transport_test::{commit, seed, service};
    use std::sync::Mutex;
    use t3_contracts::{
        application_event::{ApplicationProjectEvent, ApplicationStoredEvent},
        base_schemas::{CommandId, ThreadId},
        orchestration_v2::OrchestrationV2StoredEvent,
    };
    use t3_orchestration::{
        api::{CommitRequest, CommitResult, EventCommitter},
        error::Result,
        stores::receipts::CommandReceipt,
        testing::{TestDatabase, detached_event},
        transport::service::TransportService,
    };
    struct Blocked {
        sink: t3_orchestration::runtime::commit::EventSink,
        started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        cancelled: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    }
    struct ReadGuard(Option<tokio::sync::oneshot::Sender<()>>);
    impl Drop for ReadGuard {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }
    #[async_trait::async_trait]
    impl EventCommitter for Blocked {
        async fn commit(&self, input: CommitRequest) -> Result<CommitResult> {
            self.sink.commit(input).await
        }
        async fn commit_project(
            &self,
            event: ApplicationProjectEvent,
            receipt: CommandReceipt,
        ) -> Result<CommandReceipt> {
            self.sink.commit_project(event, receipt).await
        }
        async fn events_by_command(
            &self,
            id: CommandId,
        ) -> Result<Vec<OrchestrationV2StoredEvent>> {
            self.sink.events_by_command(id).await
        }
        fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ApplicationStoredEvent> {
            self.sink.subscribe()
        }
        async fn latest_sequence(&self, _thread: Option<ThreadId>) -> Result<u64> {
            let _guard = ReadGuard(self.cancelled.lock().unwrap().take());
            if let Some(started) = self.started.lock().unwrap().take() {
                let _ = started.send(());
            }
            std::future::pending().await
        }
    }
    let fixture = TestDatabase::memory().unwrap();
    let (base, sink) = service(&fixture);
    let sequence = seed(&fixture, &sink).await.unwrap();
    let (started, wait_started) = tokio::sync::oneshot::channel();
    let (cancelled, wait_cancelled) = tokio::sync::oneshot::channel();
    let service = TransportService::new(
        base.projections,
        Arc::new(Blocked {
            sink: sink.clone(),
            started: Mutex::new(Some(started)),
            cancelled: Mutex::new(Some(cancelled)),
        }),
        fixture.stores.clone(),
    );
    let reader = tokio::spawn(async move {
        service
            .subscribe_thread(
                serde_json::from_value(json!({"threadId":"thread","afterSequence":sequence}))
                    .unwrap(),
                LiveStreamLimits {
                    max_items: 2,
                    ..Default::default()
                },
            )
            .await
    });
    wait_started.await.unwrap();
    commit(
        &sink,
        vec![
            detached_event("one", "thread").unwrap(),
            detached_event("two", "thread").unwrap(),
            detached_event("overflow", "thread").unwrap(),
        ],
    )
    .await
    .unwrap();
    assert!(reader.await.unwrap().is_err());
    wait_cancelled.await.unwrap();
}

#[tokio::test]
async fn held_replay_delivery_overflows_then_resumes_without_loss_or_duplicates() {
    use super::bounded_snapshot_transport_test::{commit, seed, service};
    use t3_orchestration::{
        ports::transport::wire_projection::thread_stream_event_item,
        testing::{TestDatabase, detached_event},
    };
    for bytes in [false, true] {
        let fixture = TestDatabase::memory().unwrap();
        let (service, sink) = service(&fixture);
        let base = seed(&fixture, &sink).await.unwrap();
        let event = detached_event("first", "thread").unwrap();
        let first = commit(&sink, vec![event.clone()]).await.unwrap();
        let serialized = serde_json::to_vec(&thread_stream_event_item(
            t3_contracts::orchestration_v2::OrchestrationV2StoredEvent {
                sequence: first,
                command_id: None,
                event,
            },
        ))
        .unwrap()
        .len();
        let limits = if bytes {
            LiveStreamLimits {
                max_serialized_bytes: serialized,
                ..Default::default()
            }
        } else {
            LiveStreamLimits {
                max_items: 1,
                ..Default::default()
            }
        };
        let input =
            serde_json::from_value(json!({"threadId":"thread","afterSequence":base})).unwrap();
        let mut stream = service.subscribe_thread(input, limits).await.unwrap();
        assert_eq!(stream.pull().await.unwrap().unwrap()[0].sequence, first);
        let next = commit(&sink, vec![detached_event("next", "thread").unwrap()])
            .await
            .unwrap();
        stream.failed().await;
        assert!(stream.pull().await.is_err());
        stream.close().await.unwrap();
        let input =
            serde_json::from_value(json!({"threadId":"thread","afterSequence":first})).unwrap();
        let mut resumed = service
            .subscribe_thread(input, Default::default())
            .await
            .unwrap();
        assert_eq!(
            resumed
                .pull()
                .await
                .unwrap()
                .unwrap()
                .iter()
                .map(|v| v.sequence)
                .collect::<Vec<_>>(),
            vec![next]
        );
        resumed.close().await.unwrap();
    }
}
