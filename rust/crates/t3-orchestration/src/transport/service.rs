use crate::{
    api::{
        CommandDispatcher, EventCommitter, HistoryQuery, ProjectionReader, ProjectionSnapshot,
        SnapshotWindow,
    },
    error::{Error, Result},
    ports::transport::{
        live_stream_budget::*, thread_history_paging::*, thread_stream::*, wire_projection::*,
    },
    stores::{
        StoreDatabase,
        event_store::{EventFilter, EventStore},
    },
};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use t3_contracts::{
    application_event::ApplicationStoredEvent, base_schemas::*, orchestration_v2::*,
};
use tokio::{sync::broadcast, task::JoinHandle, time::Instant};
use tokio_util::sync::CancellationToken;

pub struct TransportService {
    pub projections: Arc<dyn ProjectionReader>,
    pub events: Arc<dyn EventCommitter>,
    pub database: StoreDatabase,
}
impl TransportService {
    pub fn new(
        projections: Arc<dyn ProjectionReader>,
        events: Arc<dyn EventCommitter>,
        database: StoreDatabase,
    ) -> Self {
        Self {
            projections,
            events,
            database,
        }
    }
    pub async fn dispatch(
        &self,
        dispatcher: &dyn CommandDispatcher,
        command: OrchestrationV2ServerCommand,
    ) -> Result<crate::stores::receipts::CommandReceipt> {
        dispatcher.dispatch(command).await
    }
    pub async fn thread_snapshot(&self, thread_id: ThreadId) -> Result<ProjectionSnapshot> {
        let mut snapshot = self.projections.snapshot(thread_id, None).await?;
        project_thread_projection_for_wire(&mut snapshot.projection)?;
        Ok(snapshot)
    }
    pub async fn bounded_thread_snapshot(
        &self,
        thread_id: ThreadId,
        defer_work: bool,
        options: SnapshotOptions,
    ) -> Result<OrchestrationV2ThreadBoundedSnapshot> {
        let snapshot = self
            .projections
            .snapshot(
                thread_id,
                Some(SnapshotWindow {
                    row_limit: THREAD_HISTORY_SNAPSHOT_ROW_LIMIT,
                    user_turn_limit: Some(10),
                    defer_work,
                    ..Default::default()
                }),
            )
            .await?;
        let item = build_bounded_thread_stream_snapshot(
            snapshot.snapshot_sequence,
            snapshot.projection,
            options,
        )?;
        let OrchestrationV2ThreadStreamItem::Snapshot(item) = item else {
            return Err(Error::Service {
                operation: "bounded_thread_snapshot",
                detail: "Expected snapshot".into(),
            });
        };
        Ok(OrchestrationV2ThreadBoundedSnapshot {
            snapshot_sequence: item.snapshot_sequence,
            projection: item.projection,
            history_cursor: item.history_cursor.flatten(),
            has_more_history: item.has_more_history.unwrap_or(false),
            latest_local_turn_ordinal: item.latest_local_turn_ordinal.flatten(),
            turn_items_omit_local_visible: item.turn_items_omit_local_visible,
            id_table: item.id_table,
            ids: item.ids,
        })
    }
    pub async fn history_page(
        &self,
        query: HistoryQuery,
        compact_ids: bool,
    ) -> Result<OrchestrationV2ThreadHistoryPage> {
        decode_thread_history_cursor(&query.cursor)?;
        let mut page = self.projections.history(query).await?;
        if compact_ids {
            page.id_table = Some(LiteralTrue);
        }
        Ok(page)
    }
    pub async fn search_thread(
        &self,
        query: OrchestrationV2SearchThreadInput,
    ) -> Result<OrchestrationV2SearchThreadResult> {
        self.projections.search(query).await
    }

    pub async fn subscribe_thread(
        &self,
        input: OrchestrationV2SubscribeThreadInput,
        limits: LiveStreamLimits,
    ) -> Result<ThreadSubscription> {
        // The drain starts before any database await, including a blocked high-water read.
        let receiver = self.events.subscribe();
        let live = Arc::new(LiveBuffer::new(limits));
        let through = Arc::new(AtomicU64::new(0));
        let cancellation = CancellationToken::new();
        let task = tokio::spawn(drain_thread_events(
            receiver,
            input.thread_id.clone(),
            live.clone(),
            through.clone(),
            cancellation.clone(),
        ));
        let mut subscription = ThreadSubscription {
            initial: VecDeque::new(),
            live,
            through,
            cancellation,
            task: Some(task),
            deduper: (input.accept_deduped_bodies == Some(true)).then(LiveBodyDeduper::default),
        };
        let prepared = tokio::select! {
            biased;
            _=subscription.live.failed()=>Err(Error::Lagged),
            result=self.prepare_thread(&input)=>result,
        }?;
        subscription.through.store(prepared.1, Ordering::Release);
        subscription.initial = prepared.0.into();
        Ok(subscription)
    }

    async fn prepare_thread(
        &self,
        input: &OrchestrationV2SubscribeThreadInput,
    ) -> Result<(
        Vec<Arc<SharedLiveItem<OrchestrationV2ThreadStreamItem>>>,
        u64,
    )> {
        let mut items = None;
        let mut through = 0;
        if let Some(after) = input.after_sequence {
            through = self
                .events
                .latest_sequence(Some(input.thread_id.clone()))
                .await?;
            let thread = input.thread_id.clone();
            let (pruned, stats) = self
                .database
                .read(move |db| {
                    let store = EventStore::new();
                    let pruned = store.pruned_through_sequence(db)?;
                    let stats = if after <= through && (after == through || after >= pruned as u64)
                    {
                        Some(store.replay_stats(
                            db,
                            &EventFilter {
                                thread_id: Some(thread),
                                after_sequence: integer(after)?,
                                through_sequence: Some(integer(through)?),
                                ..Default::default()
                            },
                            true,
                            Some(THREAD_RESUME_MAX_REPLAY_EVENTS as i64),
                        )?)
                    } else {
                        None
                    };
                    Ok((pruned, stats))
                })
                .await?;
            if let Some(stats) = stats
                && after <= through
                && (after == through || after >= pruned as u64)
                && stats.event_count <= THREAD_RESUME_MAX_REPLAY_EVENTS as i64
                && is_thread_replay_raw_payload_safe(stats.raw_payload_bytes as usize)
            {
                let recreated = stats.has_create_event
                    && self
                        .projections
                        .thread(input.thread_id.clone())
                        .await?
                        .is_some_and(|thread| thread.deleted_at.is_none());
                if !recreated {
                    let thread = input.thread_id.clone();
                    let replay = self
                        .database
                        .read(move |db| {
                            EventStore::new().read(
                                db,
                                &EventFilter {
                                    thread_id: Some(thread),
                                    after_sequence: integer(after)?,
                                    through_sequence: Some(integer(through)?),
                                    limit: Some(THREAD_RESUME_MAX_REPLAY_EVENTS as i64 + 1),
                                    skip_unknown_event_types: true,
                                    ..Default::default()
                                },
                            )
                        })
                        .await?;
                    let mut replay = replay
                        .into_iter()
                        .map(|mut stored| {
                            project_domain_event_for_wire(&mut stored.event)?;
                            Ok(thread_stream_event_item(stored))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    let pruned = self
                        .database
                        .read(|db| EventStore::new().pruned_through_sequence(db))
                        .await?;
                    replay = coalesce_live_updated_events(replay);
                    if (after == through || after >= pruned as u64)
                        && decide_thread_resume(
                            after,
                            through,
                            replay.len(),
                            thread_replay_encoded_bytes(
                                &replay,
                                THREAD_RESUME_MAX_REPLAY_ENCODED_BYTES,
                            )?,
                        ) != ThreadResumePlan::Snapshot
                    {
                        items = Some(replay);
                    }
                }
            }
        }
        let mut items = if let Some(items) = items {
            items
        } else {
            let bounded = input.accept_bounded_snapshot == Some(true);
            let snapshot = self
                .projections
                .snapshot(
                    input.thread_id.clone(),
                    bounded.then_some(SnapshotWindow {
                        row_limit: THREAD_HISTORY_SNAPSHOT_ROW_LIMIT,
                        user_turn_limit: Some(10),
                        defer_work: input.accept_deferred_work == Some(true),
                        ..Default::default()
                    }),
                )
                .await?;
            through = snapshot.snapshot_sequence;
            let item = if bounded {
                build_bounded_thread_stream_snapshot(
                    through,
                    snapshot.projection,
                    SnapshotOptions {
                        compact_turn_items: input.accept_compact_turn_items == Some(true),
                        dedupe_bodies: input.accept_deduped_bodies == Some(true),
                        compact_ids: input.accept_compact_ids == Some(true),
                    },
                )?
            } else {
                let mut projection = snapshot.projection;
                project_thread_projection_for_wire(&mut projection)?;
                if input.accept_deduped_bodies == Some(true) {
                    dedupe_snapshot_bodies(&mut projection);
                }
                OrchestrationV2ThreadStreamItem::Snapshot(Box::new(ThreadStreamItemMember2 {
                    kind: ThreadStreamItemMember2Kind::Snapshot,
                    snapshot_sequence: through,
                    projection,
                    history_cursor: None,
                    has_more_history: None,
                    latest_local_turn_ordinal: None,
                    turn_items_omit_local_visible: None,
                    id_table: None,
                    ids: None,
                }))
            };
            vec![item]
        };
        if input.request_completion_marker == Some(true) {
            items.push(synchronized());
        }
        let items = items
            .into_iter()
            .map(|item| {
                let sequence = match &item {
                    OrchestrationV2ThreadStreamItem::Event(v) => v.sequence,
                    _ => 0,
                };
                SharedLiveItem::new(item, sequence)
            })
            .collect::<Result<_>>()?;
        Ok((items, through))
    }
}
fn integer(sequence: u64) -> Result<i64> {
    i64::try_from(sequence).map_err(|_| Error::Service {
        operation: "sequence",
        detail: "Sequence exceeds SQLite integer range".into(),
    })
}

pub struct ThreadSubscription {
    initial: VecDeque<Arc<SharedLiveItem<OrchestrationV2ThreadStreamItem>>>,
    live: Arc<LiveBuffer<OrchestrationV2ThreadStreamItem>>,
    through: Arc<AtomicU64>,
    cancellation: CancellationToken,
    task: Option<JoinHandle<()>>,
    deduper: Option<LiveBodyDeduper>,
}
impl ThreadSubscription {
    pub fn usage(&self) -> LiveStreamUsage {
        self.live.usage()
    }
    pub async fn failed(&self) {
        self.live.failed().await;
    }
    pub async fn pull(
        &mut self,
    ) -> Result<Option<Vec<Arc<SharedLiveItem<OrchestrationV2ThreadStreamItem>>>>> {
        self.live.acknowledge();
        if let Some(item) = self.initial.pop_front() {
            if !matches!(item.value, OrchestrationV2ThreadStreamItem::Snapshot(_)) {
                self.live.deliver_external(item.clone())?;
            }
            return self.dedupe(vec![item]).map(Some);
        }
        loop {
            let Some(batch) = self.live.pull().await? else {
                return Ok(None);
            };
            let through = self.through.load(Ordering::Acquire);
            let batch: Vec<_> = batch
                .into_iter()
                .filter(|item| item.sequence > through)
                .collect();
            if batch.is_empty() {
                continue;
            }
            self.through.store(
                batch.last().map_or(through, |item| item.sequence),
                Ordering::Release,
            );
            return self.dedupe(batch).map(Some);
        }
    }
    fn dedupe(
        &mut self,
        items: Vec<Arc<SharedLiveItem<OrchestrationV2ThreadStreamItem>>>,
    ) -> Result<Vec<Arc<SharedLiveItem<OrchestrationV2ThreadStreamItem>>>> {
        let Some(deduper) = &mut self.deduper else {
            return Ok(items);
        };
        items
            .into_iter()
            .map(|item| {
                let mut value = item.value.clone();
                deduper.apply(&mut value);
                SharedLiveItem::new(value, item.sequence)
            })
            .collect()
    }
    pub async fn close(mut self) -> Result<()> {
        self.cancellation.cancel();
        self.live.acknowledge();
        if let Some(task) = self.task.take() {
            task.await.map_err(|error| Error::Service {
                operation: "thread_stream.close",
                detail: error.to_string(),
            })?;
        }
        Ok(())
    }
}
impl Drop for ThreadSubscription {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn drain_thread_events(
    mut receiver: broadcast::Receiver<ApplicationStoredEvent>,
    thread_id: ThreadId,
    live: Arc<LiveBuffer<OrchestrationV2ThreadStreamItem>>,
    through: Arc<AtomicU64>,
    cancellation: CancellationToken,
) {
    let mut keys = ThreadCoalescingKeys::default();
    let mut deadline = None;
    let mut pending = std::collections::BTreeSet::new();
    loop {
        let far_future = Instant::now() + Duration::from_secs(86400);
        tokio::select! {
            biased;
            _=cancellation.cancelled()=>break,
            _=live.failed()=>break,
            _=tokio::time::sleep_until(deadline.unwrap_or(far_future)),if deadline.is_some()=>{live.flush_pending();keys.flush();deadline=None;pending.clear();},
            received=receiver.recv()=>match received {
                Ok(ApplicationStoredEvent::Orchestration(mut stored))=>{
                    if event_thread_id(&stored.event)!=&thread_id || stored.sequence<=through.load(Ordering::Acquire){continue;}
                    if project_domain_event_for_wire(&mut stored.event).is_err(){live.fail();break;}
                    let sequence=stored.sequence;let value=thread_stream_event_item(*stored);let key=keys.key(&value);
                    let item=match SharedLiveItem::new(value,sequence){Ok(item)=>item,Err(_)=>{live.fail();break;}};
                    let result=if let Some(key)=key {
                        pending.insert(key.clone());deadline.get_or_insert_with(||Instant::now()+Duration::from_millis(50));live.push_deferred(item,key)
                    } else {live.flush_pending();deadline=None;pending.clear();live.push(item,None)};
                    if result.is_err(){break;}
                    if pending.len()>=512{live.flush_pending();keys.flush();deadline=None;pending.clear();}
                }
                Ok(ApplicationStoredEvent::Project(_))=>{},
                Err(broadcast::error::RecvError::Lagged(_))=>{live.fail();break;},
                Err(broadcast::error::RecvError::Closed)=>break,
            }
        }
    }
    live.close();
}
