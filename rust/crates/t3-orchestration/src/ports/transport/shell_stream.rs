use super::thread_stream::event_thread_id;
use crate::api::{ActiveShellIndexEntry, ProjectionReader, ShellIndexSelection};
use crate::ports::decisions::orchestrator::{epoch_millis, iso_from_epoch_millis};
use crate::{
    error::Result,
    stores::{
        StoreDatabase,
        event_store::{EventFilter, EventStore},
    },
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use t3_contracts::{
    application_event::{ApplicationProjectEvent, ApplicationStoredEvent},
    base_schemas::*,
    orchestration_project::OrchestrationProjectShell,
    orchestration_v2::*,
};

#[derive(Clone, Copy, Default)]
pub struct ShellSubscriberOptions {
    pub omit_idle_subagents: bool,
    pub omit_older_settled: bool,
}

struct TrimFacts {
    parents: BTreeSet<ThreadId>,
    subagents: BTreeSet<ThreadId>,
    remote: BTreeSet<ThreadId>,
    before: Option<i64>,
    counts: Vec<OrchestrationV2OlderSettledCount>,
}
fn older(row: &ActiveShellIndexEntry, before: i64) -> bool {
    is_listed_thread_lineage(&row.lineage)
        && row.settled_override == Some(AppThreadSettledOverride::Settled)
        && row
            .settled_at
            .as_ref()
            .and_then(|date| epoch_millis(&date.0))
            .is_some_and(|at| at < before)
}
fn trim_facts(rows: &[ActiveShellIndexEntry]) -> TrimFacts {
    let mut facts = TrimFacts {
        parents: BTreeSet::new(),
        subagents: BTreeSet::new(),
        remote: BTreeSet::new(),
        before: None,
        counts: vec![],
    };
    let mut dates = vec![];
    for row in rows {
        if row.has_remote_tasks {
            facts.remote.insert(row.id.clone());
        }
        if row.lineage.relationship_to_parent
            == Some(AppThreadLineageRelationshipToParent::Subagent)
        {
            facts.subagents.insert(row.id.clone());
            facts.parents.extend(row.lineage.parent_thread_id.clone());
        } else if is_listed_thread_lineage(&row.lineage)
            && row.settled_override == Some(AppThreadSettledOverride::Settled)
        {
            dates.extend(
                row.settled_at
                    .as_ref()
                    .and_then(|date| epoch_millis(&date.0)),
            );
        }
    }
    dates.sort_unstable_by(|a, b| b.cmp(a));
    facts.before = dates.get(24).or(dates.last()).copied();
    if let Some(before) = facts.before {
        for row in rows.iter().filter(|row| older(row, before)) {
            if let Some(count) = facts
                .counts
                .iter_mut()
                .find(|count| count.project_id == row.project_id)
            {
                count.count += 1;
            } else {
                facts.counts.push(OrchestrationV2OlderSettledCount {
                    project_id: row.project_id.clone(),
                    count: 1,
                });
            }
        }
    }
    facts
}
fn hidden_ancestors<'a>(
    lineages: impl Iterator<Item = &'a OrchestrationV2AppThreadLineage>,
) -> BTreeSet<ThreadId> {
    let mut ids = BTreeSet::new();
    for lineage in lineages.filter(|lineage| !is_listed_thread_lineage(lineage)) {
        ids.extend(lineage.parent_thread_id.clone());
        ids.insert(lineage.root_thread_id.clone());
    }
    ids
}
pub fn shell_subscriber_thread_ids(
    rows: &[ActiveShellIndexEntry],
    options: ShellSubscriberOptions,
) -> Vec<ThreadId> {
    let facts = trim_facts(rows);
    let kept: BTreeSet<_> = rows
        .iter()
        .filter(|row| {
            facts.subagents.contains(&row.id)
                && (!options.omit_idle_subagents
                    || row.may_be_live
                    || row.has_remote_tasks
                    || facts.parents.contains(&row.id))
        })
        .map(|row| row.id.clone())
        .collect();
    let ancestors = hidden_ancestors(
        rows.iter()
            .filter(|row| kept.contains(&row.id) || row.lineage.side_chat == Some(Some(true)))
            .map(|row| &row.lineage),
    );
    rows.iter()
        .filter(|row| {
            if facts.subagents.contains(&row.id) {
                kept.contains(&row.id)
            } else {
                !options.omit_older_settled
                    || facts.before.is_none_or(|before| !older(row, before))
                    || row.pinned_at.as_ref().and_then(Option::as_ref).is_some()
                    || row
                        .snoozed_until
                        .as_ref()
                        .and_then(Option::as_ref)
                        .is_some()
                    || row.may_be_live
                    || row.has_remote_tasks
                    || ancestors.contains(&row.id)
            }
        })
        .map(|row| row.id.clone())
        .collect()
}
pub async fn load_subscriber_shell_snapshot(
    reader: &dyn ProjectionReader,
    options: ShellSubscriberOptions,
    now_ms: i64,
) -> Result<(OrchestrationV2ShellSnapshot, BTreeSet<ThreadId>)> {
    let select: Option<ShellIndexSelection> =
        (options.omit_idle_subagents || options.omit_older_settled).then(|| {
            Box::new(move |rows: &[ActiveShellIndexEntry]| {
                shell_subscriber_thread_ids(rows, options)
            }) as ShellIndexSelection
        });
    let read = reader.subscriber_shell_snapshot(select).await?;
    let mut snapshot = read.snapshot;
    let fallback: Vec<_> = snapshot
        .threads
        .iter()
        .map(|row| ActiveShellIndexEntry {
            id: row.id.clone(),
            project_id: row.project_id.clone(),
            lineage: row.lineage.clone(),
            settled_override: row.settled_override.clone(),
            settled_at: row.settled_at.clone(),
            pinned_at: row.pinned_at.clone(),
            snoozed_until: row.snoozed_until.clone(),
            may_be_live: is_live_subagent_shell(row),
            has_remote_tasks: false,
        })
        .collect();
    let facts = trim_facts(read.index.as_deref().unwrap_or(&fallback));
    if options.omit_idle_subagents {
        snapshot.threads.retain(|row| {
            !facts.subagents.contains(&row.id)
                || is_live_subagent_shell(row)
                || facts.parents.contains(&row.id)
                || facts.remote.contains(&row.id)
        });
    }
    if options.omit_older_settled {
        let before = facts.before.unwrap_or(now_ms);
        let ancestors = hidden_ancestors(snapshot.threads.iter().map(|row| &row.lineage));
        snapshot.threads.retain(|row| {
            let is_older = is_listed_thread_lineage(&row.lineage)
                && row.settled_override == Some(AppThreadSettledOverride::Settled)
                && row
                    .settled_at
                    .as_ref()
                    .and_then(|date| epoch_millis(&date.0))
                    .is_some_and(|at| at < before);
            !is_older
                || row.pinned_at.as_ref().and_then(Option::as_ref).is_some()
                || row
                    .snoozed_until
                    .as_ref()
                    .and_then(Option::as_ref)
                    .and_then(|date| epoch_millis(&date.0))
                    .is_some_and(|at| at > now_ms)
                || is_live_subagent_shell(row)
                || ancestors.contains(&row.id)
                || facts.remote.contains(&row.id)
        });
        snapshot.older_settled = Some(OrchestrationV2OlderSettledThreads {
            before: t3_contracts::wire::WireDateTime(iso_from_epoch_millis(before)),
            as_of_sequence: snapshot.snapshot_sequence,
            counts: facts.counts,
        });
    }
    snapshot.archived_threads.clear();
    Ok((snapshot, facts.subagents))
}
pub fn project_identities_digest(projects: &[OrchestrationProjectShell]) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for project in projects {
        hash.update(serde_json::to_vec(&(
            &project.id,
            &project.repository_identity,
        ))?);
    }
    Ok(super::thread_history_paging::base64url_encode(
        &hash.finalize(),
    ))
}
pub fn resume_identity_roots(
    projects: &[OrchestrationProjectShell],
    resolved: Vec<String>,
    known: Option<&str>,
) -> Result<Vec<String>> {
    if known != Some(project_identities_digest(projects)?.as_str()) {
        return Ok(resolved);
    }
    let absent: BTreeSet<_> = projects
        .iter()
        .filter(|project| {
            project
                .repository_identity
                .as_ref()
                .and_then(Option::as_ref)
                .is_none()
        })
        .map(|project| &project.workspace_root.0)
        .collect();
    Ok(resolved
        .into_iter()
        .filter(|root| absent.contains(root))
        .collect())
}

pub const SHELL_RESUME_MAX_GAP: u64 = 1000;
pub const SHELL_REPLAY_PAYLOAD_BUDGET_BYTES: u64 = 8 * 1024 * 1024;
pub async fn can_resume_shell_from(
    database: &StoreDatabase,
    after: u64,
    head: u64,
) -> Result<bool> {
    if after == head {
        return Ok(true);
    }
    if after > head || head - after > SHELL_RESUME_MAX_GAP {
        return Ok(false);
    }
    database
        .read(move |db| {
            let events = EventStore::new();
            if after < events.pruned_through_sequence(db)? as u64 {
                return Ok(false);
            }
            let stats = events.replay_stats(
                db,
                &EventFilter {
                    after_sequence: after as i64,
                    through_sequence: Some(head as i64),
                    ..Default::default()
                },
                false,
                None,
            )?;
            Ok(stats.raw_payload_bytes as u64 <= SHELL_REPLAY_PAYLOAD_BUDGET_BYTES)
        })
        .await
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum ShellApplicationEvent {
    Project {
        #[serde(rename = "aggregateKind")]
        aggregate_kind: &'static str,
        #[serde(rename = "aggregateId")]
        aggregate_id: ProjectId,
        #[serde(rename = "type")]
        event_type: String,
        sequence: u64,
    },
    Thread {
        sequence: u64,
        event: ShellThreadRef,
    },
}
#[derive(Debug, Clone, Serialize)]
pub struct ShellThreadRef {
    #[serde(rename = "threadId")]
    pub thread_id: ThreadId,
    #[serde(skip)]
    pub transcript_only: bool,
}
impl ShellApplicationEvent {
    pub fn sequence(&self) -> u64 {
        match self {
            Self::Project { sequence, .. } | Self::Thread { sequence, .. } => *sequence,
        }
    }
    pub fn key(&self) -> String {
        match self {
            Self::Project { aggregate_id, .. } => format!("project:{}", aggregate_id.0),
            Self::Thread { event, .. } => format!("thread:{}", event.thread_id.0),
        }
    }
}
pub fn to_shell_application_event(stored: &ApplicationStoredEvent) -> ShellApplicationEvent {
    match stored {
        ApplicationStoredEvent::Project(project) => {
            let (id, sequence, kind) = match project.as_ref() {
                ApplicationProjectEvent::Created(v) => {
                    (v.aggregate_id.clone(), v.sequence, "project.created")
                }
                ApplicationProjectEvent::MetaUpdated(v) => {
                    (v.aggregate_id.clone(), v.sequence, "project.meta-updated")
                }
                ApplicationProjectEvent::Deleted(v) => {
                    (v.aggregate_id.clone(), v.sequence, "project.deleted")
                }
            };
            ShellApplicationEvent::Project {
                aggregate_kind: "project",
                aggregate_id: id,
                event_type: kind.into(),
                sequence,
            }
        }
        ApplicationStoredEvent::Orchestration(stored) => ShellApplicationEvent::Thread {
            sequence: stored.sequence,
            event: ShellThreadRef {
                thread_id: event_thread_id(&stored.event).clone(),
                transcript_only: matches!(
                    &stored.event,
                    OrchestrationV2DomainEvent::NodeUpdated(_)
                        | OrchestrationV2DomainEvent::ProviderTurnUpdated(_)
                        | OrchestrationV2DomainEvent::MessageUpdated(_)
                ) || matches!(&stored.event, OrchestrationV2DomainEvent::TurnItemUpdated(event) if !matches!(event.payload,
                        OrchestrationV2TurnItem::SecretRequest(_) | OrchestrationV2TurnItem::LinkRequest(_) | OrchestrationV2TurnItem::UserMessage(_))),
            },
        },
    }
}
pub fn coalesce_shell_application_events(
    events: Vec<ShellApplicationEvent>,
) -> Vec<ShellApplicationEvent> {
    let mut latest = BTreeMap::new();
    for event in events {
        latest.insert(event.key(), event);
    }
    let mut events: Vec<_> = latest.into_values().collect();
    events.sort_by_key(ShellApplicationEvent::sequence);
    events
}
pub fn should_publish_shell_application_event(
    stored: &ApplicationStoredEvent,
    subagent_ids: &mut BTreeSet<ThreadId>,
) -> bool {
    let ApplicationStoredEvent::Orchestration(stored) = stored else {
        return true;
    };
    match &stored.event {
        OrchestrationV2DomainEvent::ThreadCreated(event) => {
            if event.payload.lineage.relationship_to_parent
                == Some(AppThreadLineageRelationshipToParent::Subagent)
            {
                subagent_ids.insert(event.thread_id.clone());
            }
            true
        }
        OrchestrationV2DomainEvent::ThreadDeleted(_) => true,
        OrchestrationV2DomainEvent::TurnItemUpdated(event)
            if matches!(
                event.payload,
                OrchestrationV2TurnItem::SecretRequest(_)
                    | OrchestrationV2TurnItem::LinkRequest(_)
                    | OrchestrationV2TurnItem::UserMessage(_)
            ) =>
        {
            true
        }
        OrchestrationV2DomainEvent::NodeUpdated(_)
        | OrchestrationV2DomainEvent::TurnItemUpdated(_)
        | OrchestrationV2DomainEvent::ProviderTurnUpdated(_)
        | OrchestrationV2DomainEvent::MessageUpdated(_) => {
            !subagent_ids.contains(event_thread_id(&stored.event))
        }
        _ => true,
    }
}
pub fn is_live_subagent_shell(thread: &OrchestrationV2ThreadShell) -> bool {
    thread
        .activity_run_status
        .as_ref()
        .and_then(Option::as_ref)
        .is_some()
        || matches!(
            thread.status,
            OrchestrationV2ShellThreadStatus::Queued
                | OrchestrationV2ShellThreadStatus::Preparing
                | OrchestrationV2ShellThreadStatus::Starting
                | OrchestrationV2ShellThreadStatus::Running
                | OrchestrationV2ShellThreadStatus::Waiting
        )
        || thread.pending_runtime_request.is_some()
        || thread
            .pending_background_tasks
            .as_ref()
            .and_then(Option::as_ref)
            .is_some_and(|tasks| !tasks.is_empty())
}
pub fn is_listed_thread_lineage(lineage: &OrchestrationV2AppThreadLineage) -> bool {
    lineage.relationship_to_parent != Some(AppThreadLineageRelationshipToParent::Subagent)
        && lineage.side_chat != Some(Some(true))
}
pub fn active_shell_rows_for_subscriber(
    threads: Vec<OrchestrationV2ThreadShell>,
    omit_idle_subagents: bool,
    remote_task_parent_ids: &BTreeSet<ThreadId>,
) -> (Vec<OrchestrationV2ThreadShell>, BTreeSet<ThreadId>) {
    let subagent_ids = threads
        .iter()
        .filter(|t| {
            t.lineage.relationship_to_parent == Some(AppThreadLineageRelationshipToParent::Subagent)
        })
        .map(|t| t.id.clone())
        .collect();
    let parents: BTreeSet<_> = threads
        .iter()
        .filter(|t| {
            t.lineage.relationship_to_parent == Some(AppThreadLineageRelationshipToParent::Subagent)
        })
        .filter_map(|t| t.lineage.parent_thread_id.clone())
        .collect();
    let threads = threads
        .into_iter()
        .filter(|t| {
            !omit_idle_subagents
                || t.lineage.relationship_to_parent
                    != Some(AppThreadLineageRelationshipToParent::Subagent)
                || is_live_subagent_shell(t)
                || parents.contains(&t.id)
                || remote_task_parent_ids.contains(&t.id)
        })
        .collect();
    (threads, subagent_ids)
}
pub fn shell_stream_item_from_thread_shell(
    sequence: u64,
    thread_id: ThreadId,
    shell: Option<OrchestrationV2ThreadShell>,
) -> OrchestrationV2ShellStreamItem {
    if let Some(thread) = shell
        && thread.archived_at.is_none()
    {
        return OrchestrationV2ShellStreamItem::ThreadUpdated(Box::new(
            ShellStreamItemThreadUpdated {
                sequence,
                location: ShellStreamItemThreadUpdatedLocation::Active,
                thread,
            },
        ));
    }
    OrchestrationV2ShellStreamItem::ThreadRemoved(Box::new(ShellStreamItemThreadRemoved {
        sequence,
        location: ShellStreamItemThreadUpdatedLocation::Active,
        thread_id,
    }))
}
pub fn shell_delta_key(item: &OrchestrationV2ShellStreamItem) -> Option<String> {
    match item {
        OrchestrationV2ShellStreamItem::ProjectUpdated(v) => {
            Some(format!("project:{}", v.project.id.0))
        }
        OrchestrationV2ShellStreamItem::ProjectRemoved(v) => {
            Some(format!("project:{}", v.project_id.0))
        }
        OrchestrationV2ShellStreamItem::ThreadUpdated(v) => {
            Some(format!("thread:{:?}:{}", v.location, v.thread.id.0))
        }
        OrchestrationV2ShellStreamItem::ThreadRemoved(v) => {
            Some(format!("thread:{:?}:{}", v.location, v.thread_id.0))
        }
        _ => None,
    }
}
pub fn enrichment_snapshot(
    mut snapshot: OrchestrationV2ShellSnapshot,
    roots: Vec<String>,
) -> OrchestrationV2ShellStreamItem {
    let mut seen = BTreeSet::new();
    let roots: Vec<_> = roots
        .into_iter()
        .filter(|root| seen.insert(root.clone()))
        .collect();
    snapshot
        .projects
        .retain(|project| seen.contains(&project.workspace_root.0));
    snapshot.threads.clear();
    snapshot.archived_threads.clear();
    OrchestrationV2ShellStreamItem::Snapshot(Box::new(ShellStreamItemSnapshot {
        snapshot,
        resolved_repository_identity_roots: Some(roots),
    }))
}
pub fn shell_stream_items_from_initial_snapshot(
    snapshot: OrchestrationV2ShellSnapshot,
    roots: Vec<String>,
) -> Vec<OrchestrationV2ShellStreamItem> {
    let enrichment = (!roots.is_empty()).then(|| enrichment_snapshot(snapshot.clone(), roots));
    let mut items = vec![OrchestrationV2ShellStreamItem::Snapshot(Box::new(
        ShellStreamItemSnapshot {
            snapshot,
            resolved_repository_identity_roots: None,
        },
    ))];
    items.extend(enrichment);
    items
}
pub fn shell_stream_items_from_resume_snapshot(
    snapshot: OrchestrationV2ShellSnapshot,
    roots: Vec<String>,
) -> Vec<OrchestrationV2ShellStreamItem> {
    if roots.is_empty() {
        vec![]
    } else {
        vec![enrichment_snapshot(snapshot, roots)]
    }
}

#[derive(Default)]
pub struct ShellEnrichmentDeduper {
    previous: Option<(Vec<OrchestrationProjectShell>, BTreeSet<String>)>,
}
impl ShellEnrichmentDeduper {
    pub fn accept(&mut self, item: &OrchestrationV2ShellStreamItem) -> bool {
        let OrchestrationV2ShellStreamItem::Snapshot(item) = item else {
            if matches!(
                item,
                OrchestrationV2ShellStreamItem::ProjectUpdated(_)
                    | OrchestrationV2ShellStreamItem::ProjectRemoved(_)
            ) {
                self.previous = None;
            }
            return true;
        };
        let Some(roots) = &item.resolved_repository_identity_roots else {
            self.previous = None;
            return true;
        };
        if !item.snapshot.threads.is_empty() || !item.snapshot.archived_threads.is_empty() {
            self.previous = None;
            return true;
        }
        let roots: BTreeSet<_> = roots.iter().cloned().collect();
        if self.previous.as_ref().is_some_and(|(projects, previous)| {
            *projects == item.snapshot.projects && *previous == roots
        }) {
            return false;
        }
        self.previous = Some((item.snapshot.projects.clone(), roots));
        true
    }
}
#[derive(Default)]
pub struct UnchangedThreadShellFilter {
    sent: BTreeMap<ThreadId, (OrchestrationV2ThreadShell, i64)>,
}

use super::live_stream_budget::{LiveBuffer, LiveStreamLimits, LiveStreamUsage, SharedLiveItem};
use crate::{error::Error, transport::service::TransportService};
use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::broadcast, task::JoinHandle, time::Instant};
use tokio_util::sync::CancellationToken;

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as i64)
}
pub struct ShellSubscription {
    initial: VecDeque<Arc<SharedLiveItem<OrchestrationV2ShellStreamItem>>>,
    live: Arc<LiveBuffer<OrchestrationV2ShellStreamItem>>,
    raw: Arc<LiveBuffer<ShellApplicationEvent>>,
    lost: Arc<AtomicBool>,
    error: Arc<Mutex<Option<Error>>>,
    cancellation: CancellationToken,
    tasks: Vec<JoinHandle<()>>,
    ended: bool,
}
impl ShellSubscription {
    pub fn usage(&self) -> LiveStreamUsage {
        self.live.usage()
    }
    pub async fn pull(
        &mut self,
    ) -> Result<Option<Vec<Arc<SharedLiveItem<OrchestrationV2ShellStreamItem>>>>> {
        self.live.acknowledge();
        if self.ended {
            return Ok(None);
        }
        if self.lost.load(Ordering::Acquire) {
            self.ended = true;
            self.cancellation.cancel();
            self.initial.clear();
            return Ok(Some(vec![SharedLiveItem::new(
                OrchestrationV2ShellStreamItem::SnapshotRequired(Box::new(
                    ThreadForkSourcePointLatestStable {},
                )),
                0,
            )?]));
        }
        if let Some(item) = self.initial.pop_front() {
            return Ok(Some(vec![item]));
        }
        let result = self.live.pull().await;
        if self.lost.load(Ordering::Acquire) {
            self.ended = true;
            self.cancellation.cancel();
            return Ok(Some(vec![SharedLiveItem::new(
                OrchestrationV2ShellStreamItem::SnapshotRequired(Box::new(
                    ThreadForkSourcePointLatestStable {},
                )),
                0,
            )?]));
        }
        if result.is_err()
            && let Some(error) = self.error.lock().unwrap_or_else(|e| e.into_inner()).take()
        {
            return Err(error);
        }
        result
    }
    pub async fn close(mut self) -> Result<()> {
        self.cancellation.cancel();
        self.live.acknowledge();
        self.raw.acknowledge();
        for task in self.tasks.drain(..) {
            task.await.map_err(|error| Error::Service {
                operation: "shell_stream.close",
                detail: error.to_string(),
            })?;
        }
        Ok(())
    }
}
impl Drop for ShellSubscription {
    fn drop(&mut self) {
        self.cancellation.cancel();
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl TransportService {
    pub async fn subscriber_shell_snapshot(
        &self,
        options: ShellSubscriberOptions,
    ) -> Result<OrchestrationV2ShellSnapshot> {
        Ok(
            load_subscriber_shell_snapshot(self.projections.as_ref(), options, now_ms())
                .await?
                .0,
        )
    }
    pub async fn subscribe_shell(
        &self,
        input: OrchestrationV2SubscribeShellInput,
        limits: LiveStreamLimits,
    ) -> Result<ShellSubscription> {
        let receiver = self.events.subscribe_shared()?;
        let raw = Arc::new(LiveBuffer::new(limits));
        let live = Arc::new(LiveBuffer::new(limits));
        let cancellation = CancellationToken::new();
        let lost = Arc::new(AtomicBool::new(false));
        let subagents = Arc::new(Mutex::new(BTreeSet::new()));
        let mut subscription = ShellSubscription {
            initial: VecDeque::new(),
            live: live.clone(),
            raw: raw.clone(),
            lost: lost.clone(),
            error: Arc::new(Mutex::new(None)),
            cancellation: cancellation.clone(),
            tasks: vec![],
            ended: false,
        };
        subscription.tasks.push(tokio::spawn(drain_shell_references(
            receiver,
            raw.clone(),
            live.clone(),
            lost,
            subagents.clone(),
            cancellation.clone(),
        )));
        let prepared = tokio::select! {
            biased;
            _=raw.failed()=>Err(Error::Lagged),
            result=self.prepare_shell(&input)=>result,
        };
        let prepared = match prepared {
            Err(_) if subscription.lost.load(Ordering::Acquire) => (
                vec![OrchestrationV2ShellStreamItem::SnapshotRequired(Box::new(
                    ThreadForkSourcePointLatestStable {},
                ))],
                0,
                BTreeSet::new(),
                true,
            ),
            result => result?,
        };
        subagents
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(prepared.2);
        subscription.initial = prepared
            .0
            .into_iter()
            .map(|item| SharedLiveItem::new(item, 0))
            .collect::<Result<_>>()?;
        if prepared.3 {
            cancellation.cancel();
            subscription.live.close();
        } else {
            let projections = self.projections.clone();
            let database = self.database.clone();
            let error = subscription.error.clone();
            subscription.tasks.push(tokio::spawn(async move {
                let work = async {
                    let mut unchanged = UnchangedThreadShellFilter::default();
                    loop {
                        let Some(batch) = raw.pull().await? else { break; };
                        let refs = batch.iter().filter(|item| item.sequence > prepared.1)
                            .filter(|item| !matches!(&item.value, ShellApplicationEvent::Thread {event,..} if event.transcript_only && subagents.lock().unwrap_or_else(|e|e.into_inner()).contains(&event.thread_id)))
                            .map(|item| item.value.clone()).collect();
                        let items = project_shell_events(projections.as_ref(), &database, refs).await?;
                        for item in items {
                            if !unchanged.accept(&item, now_ms()) { continue; }
                            let sequence = shell_sequence(&item);
                            let key = shell_delta_key(&item);
                            live.push(SharedLiveItem::new(item,sequence)?, key)?;
                        }
                    }
                    Ok::<_,Error>(())
                };
                tokio::select! {
                    biased;
                    _=cancellation.cancelled()=>{},
                    result=work=>if let Err(failure)=result { *error.lock().unwrap_or_else(|e| e.into_inner())=Some(failure);live.fail(); },
                }
                live.close();
            }));
        }
        Ok(subscription)
    }
    async fn prepare_shell(
        &self,
        input: &OrchestrationV2SubscribeShellInput,
    ) -> Result<(
        Vec<OrchestrationV2ShellStreamItem>,
        u64,
        BTreeSet<ThreadId>,
        bool,
    )> {
        let options = ShellSubscriberOptions {
            omit_idle_subagents: input.omit_idle_subagents == Some(true),
            omit_older_settled: input.omit_older_settled == Some(true),
        };
        if let Some(after) = input.after_sequence {
            let head = self
                .database
                .read(|db| EventStore::new().latest_application_sequence(db))
                .await? as u64;
            if can_resume_shell_from(&self.database, after, head).await? {
                // A trimmed index still carries every subagent id, including omitted children.
                let read = self
                    .projections
                    .subscriber_shell_snapshot(Some(Box::new(|_| vec![])))
                    .await?;
                let ids = trim_facts(read.index.as_deref().unwrap_or(&[])).subagents;
                let replay = self
                    .database
                    .read(move |db| {
                        EventStore::new().read_application(
                            db,
                            &EventFilter {
                                after_sequence: after as i64,
                                through_sequence: Some(head as i64),
                                skip_unknown_event_types: true,
                                ..Default::default()
                            },
                        )
                    })
                    .await?;
                let pruned = self
                    .database
                    .read(|db| EventStore::new().pruned_through_sequence(db))
                    .await? as u64;
                if after == head || after >= pruned {
                    let mut filtered_ids = ids.clone();
                    let refs = replay
                        .iter()
                        .filter(|event| {
                            should_publish_shell_application_event(event, &mut filtered_ids)
                        })
                        .map(to_shell_application_event)
                        .collect();
                    let mut items =
                        project_shell_events(self.projections.as_ref(), &self.database, refs)
                            .await?;
                    let mut unchanged = UnchangedThreadShellFilter::default();
                    items.retain(|item| unchanged.accept(item, now_ms()));
                    if input.request_completion_marker == Some(true) {
                        items.push(OrchestrationV2ShellStreamItem::Synchronized(Box::new(
                            ThreadForkSourcePointLatestStable {},
                        )));
                    }
                    return Ok((items, head, filtered_ids, false));
                }
            }
            if input.http_snapshot_fallback.is_some() {
                return Ok((
                    vec![OrchestrationV2ShellStreamItem::SnapshotRequired(Box::new(
                        ThreadForkSourcePointLatestStable {},
                    ))],
                    head,
                    BTreeSet::new(),
                    true,
                ));
            }
        }
        let (snapshot, ids) =
            load_subscriber_shell_snapshot(self.projections.as_ref(), options, now_ms()).await?;
        let through = snapshot.snapshot_sequence;
        let mut items = shell_stream_items_from_initial_snapshot(snapshot, vec![]);
        if input.request_completion_marker == Some(true) {
            items.push(OrchestrationV2ShellStreamItem::Synchronized(Box::new(
                ThreadForkSourcePointLatestStable {},
            )));
        }
        Ok((items, through, ids, false))
    }
}
fn shell_sequence(item: &OrchestrationV2ShellStreamItem) -> u64 {
    match item {
        OrchestrationV2ShellStreamItem::ThreadUpdated(v) => v.sequence,
        OrchestrationV2ShellStreamItem::ThreadRemoved(v) => v.sequence,
        OrchestrationV2ShellStreamItem::ProjectUpdated(v) => v.sequence,
        OrchestrationV2ShellStreamItem::ProjectRemoved(v) => v.sequence,
        _ => 0,
    }
}
async fn project_shell_events(
    reader: &dyn ProjectionReader,
    database: &StoreDatabase,
    events: Vec<ShellApplicationEvent>,
) -> Result<Vec<OrchestrationV2ShellStreamItem>> {
    let mut items = vec![];
    let events = coalesce_shell_application_events(events);
    let thread_ids: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ShellApplicationEvent::Thread { event, .. } => Some(event.thread_id.clone()),
            _ => None,
        })
        .collect();
    let mut shells = if thread_ids.len() > 1 {
        reader
            .thread_shells(crate::api::ShellQuery {
                thread_ids: Some(thread_ids),
                ..Default::default()
            })
            .await
            .ok()
            .map(|snapshot| {
                snapshot
                    .threads
                    .into_iter()
                    .chain(snapshot.archived_threads)
                    .map(|shell| (shell.id.clone(), shell))
                    .collect::<BTreeMap<_, _>>()
            })
    } else {
        None
    };
    for event in events {
        items.push(match event {
            ShellApplicationEvent::Thread { sequence, event } => {
                let shell = if let Some(shells) = &mut shells {
                    shells.remove(&event.thread_id)
                } else {
                    reader.shell(event.thread_id.clone()).await?
                };
                shell_stream_item_from_thread_shell(sequence, event.thread_id, shell)
            }
            ShellApplicationEvent::Project {
                aggregate_id,
                event_type,
                sequence,
                ..
            } => {
                let id = aggregate_id.clone();
                let project = if event_type == "project.deleted" {
                    None
                } else {
                    database
                        .read(move |db| crate::stores::projects::get_shell(db, &id))
                        .await?
                };
                if let Some(project) = project {
                    OrchestrationV2ShellStreamItem::ProjectUpdated(Box::new(
                        ShellStreamItemProjectUpdated { sequence, project },
                    ))
                } else {
                    OrchestrationV2ShellStreamItem::ProjectRemoved(Box::new(
                        ShellStreamItemProjectRemoved {
                            sequence,
                            project_id: aggregate_id,
                        },
                    ))
                }
            }
        });
    }
    Ok(items)
}
async fn drain_shell_references(
    mut receiver: broadcast::Receiver<Arc<ApplicationStoredEvent>>,
    raw: Arc<LiveBuffer<ShellApplicationEvent>>,
    live: Arc<LiveBuffer<OrchestrationV2ShellStreamItem>>,
    lost: Arc<AtomicBool>,
    ids: Arc<Mutex<BTreeSet<ThreadId>>>,
    cancellation: CancellationToken,
) {
    let mut deadline = None;
    let mut count = 0;
    loop {
        tokio::select! {
            biased;
            _=cancellation.cancelled()=>break,
            _=live.failed()=>break,
            _=tokio::time::sleep_until(deadline.unwrap_or_else(|| Instant::now()+Duration::from_secs(86400))),if deadline.is_some()=>{raw.flush_pending();deadline=None;count=0;},
            received=receiver.recv()=>match received {
                Ok(event)=>{
                    if !should_publish_shell_application_event(&event,&mut ids.lock().unwrap_or_else(|e| e.into_inner())) { continue; }
                    let event=to_shell_application_event(&event);
                    let sequence=event.sequence();
                    let mut key=event.key();
                    if matches!(&event,ShellApplicationEvent::Thread {event,..} if event.transcript_only) {key.push_str(":transcript");}
                    let result=SharedLiveItem::new(event,sequence).and_then(|item|raw.push_deferred(item,key));
                    if result.is_err() { live.fail();break; }
                    count+=1;deadline.get_or_insert_with(||Instant::now()+Duration::from_millis(50));
                    if count>=512 { raw.flush_pending();deadline=None;count=0; }
                },
                Err(broadcast::error::RecvError::Lagged(_))=>{lost.store(true,Ordering::Release);raw.fail();live.fail();break;},
                Err(broadcast::error::RecvError::Closed)=>break,
            }
        }
    }
    raw.close();
}
impl UnchangedThreadShellFilter {
    pub fn accept(&mut self, item: &OrchestrationV2ShellStreamItem, now: i64) -> bool {
        if let OrchestrationV2ShellStreamItem::ThreadRemoved(item) = item {
            self.sent.remove(&item.thread_id);
            return true;
        }
        let OrchestrationV2ShellStreamItem::ThreadUpdated(item) = item else {
            return true;
        };
        if let Some((previous, at)) = self.sent.get(&item.thread.id) {
            let mut candidate = item.thread.clone();
            candidate.updated_at = previous.updated_at.clone();
            if now - *at < 5000 && candidate == *previous {
                return false;
            }
        }
        self.sent
            .insert(item.thread.id.clone(), (item.thread.clone(), now));
        true
    }
}

#[cfg(test)]
mod subscription_tests {
    use super::*;
    #[tokio::test]
    async fn broadcast_loss_emits_snapshot_required_and_terminates_instead_of_skipping() {
        let (sender, receiver) = broadcast::channel(1);
        for sequence in 1..=2 {
            sender
                .send(Arc::new(ApplicationStoredEvent::Orchestration(Box::new(
                    OrchestrationV2StoredEvent {
                        sequence,
                        command_id: None,
                        event: crate::testing::detached_event(&format!("lag-{sequence}"), "thread")
                            .unwrap(),
                    },
                ))))
                .unwrap();
        }
        let live = Arc::new(LiveBuffer::new(Default::default()));
        let raw = Arc::new(LiveBuffer::new(Default::default()));
        let lost = Arc::new(AtomicBool::new(false));
        let cancellation = CancellationToken::new();
        drain_shell_references(
            receiver,
            raw.clone(),
            live.clone(),
            lost.clone(),
            Arc::new(Mutex::new(BTreeSet::new())),
            cancellation.clone(),
        )
        .await;
        let mut stream = ShellSubscription {
            initial: VecDeque::new(),
            live,
            raw,
            lost,
            error: Arc::new(Mutex::new(None)),
            cancellation: cancellation.clone(),
            tasks: vec![],
            ended: false,
        };
        assert!(matches!(
            stream.pull().await.unwrap().unwrap()[0].value,
            OrchestrationV2ShellStreamItem::SnapshotRequired(_)
        ));
        assert!(stream.pull().await.unwrap().is_none());
        assert!(cancellation.is_cancelled());
    }
    #[tokio::test]
    async fn closing_shell_cancels_its_owner_and_awaits_event_receiver_release() {
        let (sender, receiver) = broadcast::channel(1);
        let live = Arc::new(LiveBuffer::new(Default::default()));
        let raw = Arc::new(LiveBuffer::new(Default::default()));
        let lost = Arc::new(AtomicBool::new(false));
        let cancellation = CancellationToken::new();
        let task = tokio::spawn(drain_shell_references(
            receiver,
            raw.clone(),
            live.clone(),
            lost.clone(),
            Arc::new(Mutex::new(BTreeSet::new())),
            cancellation.clone(),
        ));
        let stream = ShellSubscription {
            initial: VecDeque::new(),
            live,
            raw,
            lost,
            error: Arc::new(Mutex::new(None)),
            cancellation: cancellation.clone(),
            tasks: vec![task],
            ended: false,
        };
        stream.close().await.unwrap();
        assert!(cancellation.is_cancelled());
        assert_eq!(sender.receiver_count(), 0);
    }
}
