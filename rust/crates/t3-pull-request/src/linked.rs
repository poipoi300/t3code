use crate::{Error, Result};
use async_trait::async_trait;
use std::{collections::HashMap, sync::Mutex};
use t3_contracts::{
    pull_request::PullRequestLinkedThreadsResult,
    pull_request_review_guide::PullRequestReviewGuide, thread_pull_request::ThreadPullRequestKey,
};

#[derive(Debug, Clone)]
pub struct ViewedScope {
    pub provider: String,
    pub host: String,
    pub repository: String,
    pub number: u64,
    pub viewer: String,
}
impl ViewedScope {
    pub fn key(&self) -> String {
        serde_json::json!([
            self.provider,
            self.host,
            self.repository,
            self.number,
            self.viewer
        ])
        .to_string()
    }
}
#[derive(Debug, Clone)]
pub struct ViewedMark {
    pub path: String,
    pub revision: Option<String>,
}

#[async_trait]
pub trait Records: Send + Sync {
    async fn linked_threads(
        &self,
        _key: &ThreadPullRequestKey,
    ) -> Result<PullRequestLinkedThreadsResult> {
        Err(Error::unsupported("linkedThreads"))
    }
    async fn viewed(&self, scope: &ViewedScope) -> Result<(Vec<ViewedMark>, bool)>;
    async fn set_viewed(
        &self,
        scope: &ViewedScope,
        marks: Vec<(ViewedMark, bool)>,
        viewed_at: String,
    ) -> Result<()>;
    async fn guide(&self, key: &ThreadPullRequestKey) -> Result<Option<PullRequestReviewGuide>>;
    async fn set_guide(
        &self,
        key: &ThreadPullRequestKey,
        guide: PullRequestReviewGuide,
    ) -> Result<()>;
}
#[derive(Default)]
pub struct MemoryRecords {
    viewed: Mutex<HashMap<String, Vec<ViewedMark>>>,
    guides: Mutex<HashMap<String, PullRequestReviewGuide>>,
}
pub fn key_string(key: &ThreadPullRequestKey) -> String {
    serde_json::json!([
        key.host.as_str().to_lowercase(),
        key.repository.as_str().to_lowercase(),
        key.number
    ])
    .to_string()
}
#[async_trait]
impl Records for MemoryRecords {
    async fn viewed(&self, scope: &ViewedScope) -> Result<(Vec<ViewedMark>, bool)> {
        Ok((
            self.viewed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&scope.key())
                .cloned()
                .unwrap_or_default(),
            false,
        ))
    }
    async fn set_viewed(
        &self,
        scope: &ViewedScope,
        marks: Vec<(ViewedMark, bool)>,
        _viewed_at: String,
    ) -> Result<()> {
        let mut held = self.viewed.lock().unwrap_or_else(|e| e.into_inner());
        let records = held.entry(scope.key()).or_default();
        for (mark, viewed) in marks {
            records.retain(|r| r.path != mark.path);
            if viewed {
                records.push(mark);
            }
        }
        Ok(())
    }
    async fn guide(&self, key: &ThreadPullRequestKey) -> Result<Option<PullRequestReviewGuide>> {
        Ok(self
            .guides
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key_string(key))
            .cloned())
    }
    async fn set_guide(
        &self,
        key: &ThreadPullRequestKey,
        guide: PullRequestReviewGuide,
    ) -> Result<()> {
        self.guides
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key_string(key), guide);
        Ok(())
    }
}
impl crate::PullRequestService {
    pub async fn linked_threads(
        &self,
        input: t3_contracts::pull_request::PullRequestRef,
    ) -> Result<PullRequestLinkedThreadsResult> {
        let context = self.context(&input).await?;
        self.records().linked_threads(&context.key()?).await
    }
}

/// Changes to the rows of `pullRequests.linkedThreads` (linkedThreads.ts).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LinkedChange {
    Changed,
    Reordered,
    None,
}

#[derive(PartialEq)]
struct LinkedView {
    keys: Vec<(String, String, u64)>,
    title: String,
    project:
        t3_contracts::orchestration_v2_id_table::IdOrIndex<t3_contracts::base_schemas::ProjectId>,
    archived: bool,
    settled: bool,
}

#[derive(Default)]
pub struct LinkedThreadsDetector {
    // A known unlinked thread has a None view. An absent thread may have links from a prior process.
    views: HashMap<t3_contracts::base_schemas::ThreadId, Option<(LinkedView, i64)>>,
}
impl LinkedThreadsDetector {
    fn activity(
        &mut self,
        id: &t3_contracts::base_schemas::ThreadId,
        at: &t3_contracts::wire::WireDateTime,
    ) -> LinkedChange {
        match self.views.get_mut(id) {
            None => LinkedChange::Reordered,
            Some(Some((_, previous))) => {
                let now = epoch_millis(at);
                if now > *previous {
                    *previous = now;
                    LinkedChange::Reordered
                } else {
                    LinkedChange::None
                }
            }
            Some(None) => LinkedChange::None,
        }
    }
    pub fn observe(
        &mut self,
        event: &t3_contracts::orchestration_v2::OrchestrationV2DomainEvent,
    ) -> LinkedChange {
        use t3_contracts::orchestration_v2::OrchestrationV2DomainEvent as E;
        let full = match event {
            E::ThreadCreated(e)
            | E::ThreadArchived(e)
            | E::ThreadUnarchived(e)
            | E::ThreadDeleted(e)
            | E::ThreadSettled(e)
            | E::ThreadUnsettled(e)
            | E::ThreadSnoozed(e)
            | E::ThreadUnsnoozed(e)
            | E::ThreadPinned(e)
            | E::ThreadAutoSettleSet(e)
            | E::ThreadUnpinned(e)
            | E::ThreadPinReordered(e)
            | E::ThreadActiveReordered(e)
            | E::ThreadTagsUpdated(e)
            | E::ThreadVisited(e)
            | E::ThreadMarkedUnread(e)
            | E::ThreadMetadataUpdated(e)
            | E::ThreadPullRequestSynced(e)
            | E::ThreadRuntimeModeUpdated(e)
            | E::ThreadInteractionModeUpdated(e)
            | E::ThreadModelSelectionUpdated(e)
            | E::ThreadProviderSwitched(e)
            | E::ThreadGuestShareUpdated(e) => e,
            E::ThreadVisitRecorded(_) => return LinkedChange::None,
            E::ThreadPullRequestLinkUpdated(_) => return LinkedChange::None,
            E::RunCreated(e) | E::RunUpdated(e) => {
                return self.activity(&e.thread_id, &e.occurred_at);
            }
            E::RunBackgroundWorkCancelled(e) => return self.activity(&e.thread_id, &e.occurred_at),
            E::RunAttemptCreated(e) | E::RunAttemptUpdated(e) => {
                return self.activity(&e.thread_id, &e.occurred_at);
            }
            E::NodeUpdated(e) => return self.activity(&e.thread_id, &e.occurred_at),
            E::SubagentUpdated(e) => return self.activity(&e.thread_id, &e.occurred_at),
            E::ProviderSessionAttached(e) | E::ProviderSessionUpdated(e) => {
                return self.activity(&e.thread_id, &e.occurred_at);
            }
            E::ProviderSessionDetached(e) => return self.activity(&e.thread_id, &e.occurred_at),
            E::ProviderThreadUpdated(e) => return self.activity(&e.thread_id, &e.occurred_at),
            E::ProviderTurnUpdated(e) => return self.activity(&e.thread_id, &e.occurred_at),
            E::RuntimeRequestUpdated(e) => return self.activity(&e.thread_id, &e.occurred_at),
            E::MessageUpdated(e) => return self.activity(&e.thread_id, &e.occurred_at),
            E::TurnItemUpdated(e) => return self.activity(&e.thread_id, &e.occurred_at),
            E::PlanUpdated(e) => return self.activity(&e.thread_id, &e.occurred_at),
            E::CheckpointScopeCreated(e) => return self.activity(&e.thread_id, &e.occurred_at),
            E::CheckpointCaptured(e) => return self.activity(&e.thread_id, &e.occurred_at),
            E::CheckpointRollbackRequested(e) => {
                return self.activity(&e.thread_id, &e.occurred_at);
            }
            E::ContextHandoffUpdated(e) => return self.activity(&e.thread_id, &e.occurred_at),
            E::ContextTransferCreated(e) | E::ContextTransferUpdated(e) => {
                return self.activity(&e.thread_id, &e.occurred_at);
            }
        };
        let thread = &full.payload;
        let id = &full.thread_id;
        let mut keys = Vec::new();
        if thread.deleted_at.is_none() && thread.lineage.side_chat.flatten() != Some(true) {
            for link in thread
                .pull_requests
                .as_ref()
                .and_then(Option::as_ref)
                .into_iter()
                .flatten()
            {
                if link.source != t3_contracts::thread_pull_request::ThreadPullRequestLinkSource::StackDismissed {
                    keys.push((link.host.0.trim().to_lowercase(), link.repository.0.trim().to_lowercase(), link.number));
                }
            }
        }
        keys.sort_unstable();
        let view = (!keys.is_empty()).then(|| LinkedView {
            keys,
            title: thread.title.0.clone(),
            project: thread.project_id.clone(),
            archived: thread.archived_at.is_some(),
            settled: thread.settled_at.is_some()
                || thread.settled_override
                    == Some(t3_contracts::orchestration_v2::AppThreadSettledOverride::Settled),
        });
        let prior = self.views.get(id);
        let known = prior.is_some() || matches!(event, E::ThreadCreated(_));
        let changed =
            !known || prior.and_then(|entry| entry.as_ref().map(|(view, _)| view)) != view.as_ref();
        if matches!(event, E::ThreadDeleted(_)) {
            self.views.remove(id);
        } else if changed || prior.is_none() {
            self.views.insert(
                id.clone(),
                view.map(|view| (view, epoch_millis(&thread.updated_at))),
            );
        } else {
            return self.activity(id, &thread.updated_at);
        }
        if changed {
            LinkedChange::Changed
        } else {
            LinkedChange::None
        }
    }
}
fn epoch_millis(at: &t3_contracts::wire::WireDateTime) -> i64 {
    chrono::DateTime::parse_from_rfc3339(&at.0).map_or(i64::MIN, |date| date.timestamp_millis())
}

/// Publish committed linked-thread changes and coalesce activity over ten-second windows.
pub async fn follow_changes(
    mut events: tokio::sync::broadcast::Receiver<
        std::sync::Arc<t3_contracts::application_event::ApplicationStoredEvent>,
    >,
    stop: tokio_util::sync::CancellationToken,
    notify: impl Fn(),
) {
    use t3_contracts::application_event::ApplicationStoredEvent;
    use tokio::{
        sync::broadcast::error::RecvError,
        time::{Duration, Instant, sleep_until},
    };

    let mut detector = LinkedThreadsDetector::default();
    let mut next_reorder = Instant::now();
    let mut pending = false;
    loop {
        tokio::select! {
            biased;
            _ = stop.cancelled() => break,
            _ = sleep_until(next_reorder), if pending => {
                notify();
                pending = false;
                next_reorder = Instant::now() + Duration::from_secs(10);
            }
            event = events.recv() => match event {
                Ok(event) => if let ApplicationStoredEvent::Orchestration(stored) = event.as_ref() {
                    match detector.observe(&stored.event) {
                        LinkedChange::Changed => notify(),
                        LinkedChange::Reordered => pending = true,
                        LinkedChange::None => {}
                    }
                },
                Err(RecvError::Closed) => break,
                Err(RecvError::Lagged(_)) => {
                    detector = LinkedThreadsDetector::default();
                    notify();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::{sync::Arc, time::Duration};
    use t3_contracts::{
        application_event::ApplicationStoredEvent, orchestration_v2::OrchestrationV2StoredEvent,
    };

    fn sample(raw: &str) -> Value {
        serde_json::from_str(raw.lines().next().expect("wire fixture")).expect("JSON")
    }
    fn envelope(event: Value) -> Arc<ApplicationStoredEvent> {
        Arc::new(ApplicationStoredEvent::Orchestration(Box::new(
            OrchestrationV2StoredEvent {
                event: serde_json::from_value(event).expect("domain event"),
                sequence: 1,
                command_id: None,
            },
        )))
    }
    fn created(id: &str, linked: bool) -> Arc<ApplicationStoredEvent> {
        let mut event = sample(include_str!(
            "../../../fixtures/samples/OrchestrationV2DomainEvent.thread.created.jsonl"
        ));
        event["threadId"] = json!(id);
        event["payload"]["deletedAt"] = Value::Null;
        event["payload"]["updatedAt"] = json!("2026-01-01T00:00:00Z");
        event["payload"]["lineage"]["sideChat"] = json!(false);
        event["payload"]["pullRequests"] = if linked {
            json!([{"host":"github.com","repository":"owner/repo","number":7,
            "url":"https://github.com/owner/repo/pull/7","source":"manual","linkedAt":"2026-01-01T00:00:00Z","snapshot":null,"stack":null}])
        } else {
            json!([])
        };
        envelope(event)
    }
    fn activity(id: &str, second: u8) -> Arc<ApplicationStoredEvent> {
        let mut event = sample(include_str!(
            "../../../fixtures/samples/OrchestrationV2DomainEvent.message.updated.jsonl"
        ));
        event["threadId"] = json!(id);
        event["occurredAt"] = json!(format!("2026-01-01T00:00:{second:02}Z"));
        envelope(event)
    }

    #[tokio::test(start_paused = true)]
    async fn lag_resets_known_unlinked_rows_and_deletion_forgets_the_thread() {
        let (tx, rx) = tokio::sync::broadcast::channel(2);
        let (publish, mut revisions) = tokio::sync::mpsc::unbounded_channel();
        let worker = tokio::spawn(follow_changes(rx, Default::default(), move || {
            publish.send(()).expect("subscriber")
        }));
        tx.send(created("old", false)).expect("receiver");
        tx.send(created("barrier", true)).expect("receiver");
        revisions.recv().await.expect("initial barrier");
        // On this current-thread runtime the follower cannot consume any part of this burst.
        for _ in 0..3 {
            tx.send(activity("old", 1)).expect("receiver");
        }
        drop(tx);
        worker.await.expect("follower");
        assert_eq!(
            std::iter::from_fn(|| revisions.try_recv().ok()).count(),
            2,
            "lag invalidation and the now-unknown thread's activity"
        );

        let mut detector = LinkedThreadsDetector::default();
        let event = created("deleted", true);
        let ApplicationStoredEvent::Orchestration(row) = event.as_ref() else {
            unreachable!()
        };
        assert!(matches!(
            detector.observe(&row.event),
            LinkedChange::Changed
        ));
        let mut deleted = serde_json::to_value(&row.event).expect("event");
        deleted["type"] = json!("thread.deleted");
        deleted["payload"]["deletedAt"] = json!("2026-01-01T00:00:10Z");
        assert!(matches!(
            detector.observe(&serde_json::from_value(deleted).expect("deletion")),
            LinkedChange::Changed
        ));
        let event = activity("deleted", 2);
        let ApplicationStoredEvent::Orchestration(row) = event.as_ref() else {
            unreachable!()
        };
        assert!(matches!(
            detector.observe(&row.event),
            LinkedChange::Reordered
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn linked_activity_is_coalesced_and_unlinked_activity_is_silent() {
        let (tx, events) = tokio::sync::broadcast::channel(32);
        let (publish, mut revisions) = tokio::sync::mpsc::unbounded_channel();
        let stop = tokio_util::sync::CancellationToken::new();
        let worker = tokio::spawn(follow_changes(events, stop.clone(), move || {
            publish.send(()).expect("subscriber")
        }));
        tx.send(created("unlinked", false)).expect("receiver");
        tx.send(activity("unlinked", 1)).expect("receiver");
        tx.send(created("linked", true)).expect("receiver");
        assert_eq!(revisions.recv().await, Some(()));
        assert!(revisions.try_recv().is_err());
        tx.send(activity("linked", 1)).expect("receiver");
        assert_eq!(revisions.recv().await, Some(()));
        tx.send(activity("linked", 1)).expect("receiver");
        tx.send(activity("linked", 2)).expect("receiver");
        tx.send(activity("unknown", 3)).expect("receiver");
        tx.send(created("second", true)).expect("receiver");
        // This immediate row change is a barrier behind the burst above.
        assert_eq!(revisions.recv().await, Some(()));
        tokio::time::advance(Duration::from_secs(9)).await;
        assert!(revisions.try_recv().is_err());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(revisions.recv().await, Some(()));
        tx.send(activity("unlinked", 4)).expect("receiver");
        tx.send(activity("linked", 2)).expect("receiver");
        tokio::time::advance(Duration::from_secs(11)).await;
        tokio::task::yield_now().await;
        assert!(revisions.try_recv().is_err());
        stop.cancel();
        worker.await.expect("worker");
    }
}
