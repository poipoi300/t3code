use crate::{
    api::*,
    error::{Error, PullRequestHostError, Result},
    projections::{
        read::PullRequestThread, store::ProjectionStore, write::PullRequestIdentityRegistry,
    },
    runtime::threads::{
        ThreadProjectWorker, ThreadServiceScope, date_ms, join_thread_reads,
        normalize_pull_request_key,
    },
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
    sync::Arc,
    time::Duration,
};
use t3_contracts::{pull_request::*, thread_pull_request::*};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::task::TaskTracker;

#[derive(Default)]
pub struct CommandObservation {
    pub created: Vec<(ThreadPullRequestKey, String)>,
    pub pushes: bool,
    pub closes: bool,
}

pub trait PullRequestSyncPolicy: Send + Sync {
    fn sibling_url(&self, url: &str, number: u64) -> Option<String>;
    fn avatar_identity(&self, url: &str) -> String;
    fn command(&self, input: &str, output: &str) -> CommandObservation;
}

pub struct PullRequestSyncServices {
    pub projections: Arc<ProjectionStore>,
    pub commands: Arc<dyn CommandDispatcher>,
    pub committer: Arc<dyn EventCommitter>,
    pub pull_requests: Arc<dyn PullRequestBackend>,
    pub identities: Arc<dyn PullRequestIdentityRegistry>,
    pub policy: Arc<dyn PullRequestSyncPolicy>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
}
#[derive(Clone)]
struct Entry {
    thread: PullRequestThread,
    link: ThreadPullRequestLink,
}
#[derive(Default)]
struct State {
    last: BTreeMap<String, i64>,
    requested: BTreeSet<String>,
    pending: BTreeSet<String>,
    retry_stacks: BTreeSet<String>,
    missing: BTreeSet<String>,
    pauses: BTreeMap<String, i64>,
    close_commands: BTreeMap<ThreadId, BTreeSet<String>>,
}
struct Context {
    services: PullRequestSyncServices,
    state: Mutex<State>,
    sweep: Mutex<()>,
}
pub struct PullRequestSyncReactor {
    context: Arc<Context>,
    worker: ThreadProjectWorker<(), bool>,
    scope: ThreadServiceScope,
    tasks: TaskTracker,
    started: Mutex<bool>,
}
fn key(key: &ThreadPullRequestKey) -> String {
    format!("{}/{}#{}", key.host.0, key.repository.0, key.number)
}
fn link_key(link: &ThreadPullRequestLink) -> ThreadPullRequestKey {
    serde_json::from_value(
        json!({"host":link.host,"repository":link.repository,"number":link.number}),
    )
    .expect("link key")
}
fn unsettled(thread: &PullRequestThread) -> bool {
    json!(thread.settled_override) != "settled" && thread.settled_at.is_none()
}
fn reference(entry: &Entry) -> Result<PullRequestRef> {
    Ok(serde_json::from_value(
        json!({"projectId":entry.thread.project_id,"host":entry.link.host,"repository":entry.link.repository,"number":entry.link.number,"allowStale":false}),
    )?)
}
fn group_key(entry: &Entry) -> String {
    format!(
        "{}\0{}",
        entry.thread.project_id.0,
        key(&link_key(&entry.link))
    )
}
fn pause_key(entry: &Entry) -> String {
    format!("{}\0{}", entry.thread.project_id.0, entry.link.host.0)
}
fn due(state: &State, group: &str, entries: &[Entry], now: i64) -> bool {
    if state.requested.contains(group) {
        return true;
    }
    let active: Vec<_> = entries
        .iter()
        .filter(|entry| unsettled(&entry.thread))
        .collect();
    if active.is_empty() {
        return false;
    }
    let since = state
        .last
        .get(group)
        .map_or(i64::MAX, |last| now.saturating_sub(*last));
    if state.missing.contains(group) {
        return since >= 900_000;
    }
    if state.retry_stacks.contains(group)
        || active.iter().any(|entry| entry.link.snapshot.is_none())
    {
        return true;
    }
    if active.iter().all(|entry| {
        entry
            .link
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| match snapshot.state {
                PullRequestState::Merged => snapshot
                    .merged_at
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_some(),
                PullRequestState::Closed => snapshot
                    .closed_at
                    .as_ref()
                    .and_then(Option::as_ref)
                    .and_then(|at| date_ms(at))
                    .is_some_and(|at| now - at >= 86_400_000),
                _ => false,
            })
    }) {
        return false;
    }
    let open: Vec<_> = active
        .iter()
        .filter_map(|entry| entry.link.snapshot.as_ref())
        .filter(|snapshot| snapshot.state == PullRequestState::Open)
        .collect();
    if !open.is_empty() {
        return open.iter().any(|snapshot| {
            snapshot
                .updated_at
                .as_ref()
                .and_then(|at| date_ms(&at.0))
                .is_none_or(|at| now - at < 1_800_000)
        }) || since >= 300_000;
    }
    since >= 900_000
}
fn snapshot_fields(summary: &PullRequestSummary) -> Value {
    let summary = json!(summary);
    let mut fields = json!({"state":summary["state"],"title":summary["title"],"headBranch":summary["headBranch"],"baseBranch":summary["baseBranch"],"isDraft":summary["isDraft"].as_bool().unwrap_or(false),"updatedAt":summary["updatedAt"],"closedAt":summary["closedAt"],"mergedAt":summary["mergedAt"]});
    for name in [
        "author",
        "additions",
        "deletions",
        "changedFiles",
        "reviewDecision",
        "checksState",
        "mergeability",
    ] {
        if let Some(value) = summary.get(name) {
            fields[name] = value.clone();
        }
    }
    fields
}
fn known_mergeability(
    previous: &Option<ThreadPullRequestSnapshot>,
    mut fields: Value,
    summary: &PullRequestSummary,
) -> Value {
    let previous = json!(previous);
    if json!(summary.provider) == "github"
        && fields["state"] == "open"
        && fields["mergeability"] == "unknown"
        && previous["updatedAt"] == fields["updatedAt"]
        && previous["state"] == "open"
        && previous
            .get("mergeability")
            .is_some_and(|value| !value.is_null() && value != "unknown")
    {
        fields["mergeability"] = previous["mergeability"].clone();
    }
    fields
}
fn fields_equal(left: &Value, right: &Value, policy: &dyn PullRequestSyncPolicy) -> bool {
    let normalize = |value: &Value| {
        let mut value = value.clone();
        if let Some(object) = value.as_object_mut() {
            object.remove("syncedAt");
        }
        for name in ["closedAt", "mergedAt", "reviewDecision", "checksState"] {
            if value.get(name).is_none() {
                value[name] = Value::Null;
            }
        }
        let login = value["author"]["login"].clone();
        let avatar = value["author"]["avatarUrl"]
            .as_str()
            .map(|url| policy.avatar_identity(url));
        value["author"] = json!({"login":login,"avatarUrl":avatar});
        value
    };
    normalize(left) == normalize(right)
}
impl PullRequestSyncReactor {
    pub fn new(services: PullRequestSyncServices) -> Self {
        let context = Arc::new(Context {
            services,
            state: Mutex::new(State::default()),
            sweep: Mutex::new(()),
        });
        let work = context.clone();
        let worker = ThreadProjectWorker::new(
            1,
            |all: &mut bool, next| *all |= next,
            move |all, _| {
                let context = work.clone();
                async move {
                    if let Err(error) = context.synchronize(all).await {
                        eprintln!("pull request sync sweep failed: {error}");
                    }
                }
            },
        );
        Self {
            context,
            worker,
            scope: ThreadServiceScope::default(),
            tasks: TaskTracker::new(),
            started: Mutex::new(false),
        }
    }
    pub async fn sweep(&self) -> Result<()> {
        self.scope.run(self.context.synchronize(true)).await
    }
    pub async fn request_sync(&self, key: ThreadPullRequestKey) -> Result<()> {
        self.scope
            .run(async {
                let normalized = normalize_pull_request_key(
                    self.context.services.identities.as_ref(),
                    &key,
                    None,
                )?;
                self.context.state.lock().await.pending.insert(
                    crate::ports::thread_project::pull_request_sync_reactor::key(&normalized),
                );
                self.worker.enqueue((), false).await
            })
            .await
    }
    pub async fn drain(&self) {
        self.worker.drain().await;
    }
    pub async fn start(self: &Arc<Self>) -> Result<()> {
        self.scope.run(async{
        let mut started=self.started.lock().await; if *started{return Ok(());}
        let mut events=self.context.services.committer.subscribe();
        self.worker.enqueue((),true).await?; *started=true;
        let service=self.clone(); let cancellation=self.scope.cancellation();
        self.tasks.spawn(async move {
            let mut interval=tokio::time::interval_at(tokio::time::Instant::now()+Duration::from_secs(60),Duration::from_secs(60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                let result=tokio::select!{biased;
                    _=cancellation.cancelled()=>break,
                    _=interval.tick()=>service.worker.enqueue((),true).await,
                    event=events.recv()=>match event{
                        Ok(t3_contracts::application_event::ApplicationStoredEvent::Orchestration(event))=>service.process_event(&event.event).await,
                        Ok(_)=>Ok(()),Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>service.worker.enqueue((),true).await,
                        Err(tokio::sync::broadcast::error::RecvError::Closed)=>break,
                    }
                };
                if let Err(error)=result {if matches!(error,Error::Cancelled){break;}eprintln!("pull request sync event failed: {error}");}
            }
        });
        Ok(())
    }).await
    }
    pub async fn process_event(&self, event: &OrchestrationV2DomainEvent) -> Result<()> {
        self.scope.run(self.process_event_inner(event)).await
    }
    async fn refresh_unmerged(&self, id: ThreadId) -> Result<()> {
        for thread in self
            .context
            .services
            .projections
            .pull_request_snapshot(Some(id), false)
            .await?
            .threads
        {
            for link in thread
                .pull_requests
                .iter()
                .filter(|link| link.source != ThreadPullRequestLinkSource::StackDismissed)
            {
                if link.snapshot.as_ref().is_some_and(|snapshot| {
                    matches!(
                        snapshot.state,
                        PullRequestState::Open | PullRequestState::Closed
                    )
                }) {
                    self.request_sync(link_key(link)).await?;
                }
            }
        }
        Ok(())
    }
    async fn process_event_inner(&self, event: &OrchestrationV2DomainEvent) -> Result<()> {
        let event = json!(event);
        let kind = event["type"].as_str().unwrap_or_default();
        let id: ThreadId = serde_json::from_value(event["threadId"].clone())?;
        let payload = &event["payload"];
        match kind {
            "thread.pull-request-link-updated" | "thread.pull-request-synced" => {
                for thread in self
                    .context
                    .services
                    .projections
                    .pull_request_snapshot(Some(id), false)
                    .await?
                    .threads
                {
                    for mut link in thread
                        .pull_requests
                        .iter()
                        .filter(|link| {
                            link.source != ThreadPullRequestLinkSource::StackDismissed
                                && link.snapshot.is_none()
                        })
                        .cloned()
                    {
                        let normalized = normalize_pull_request_key(
                            self.context.services.identities.as_ref(),
                            &link_key(&link),
                            Some(&link.url.0),
                        )?;
                        link.host = normalized.host;
                        link.repository = normalized.repository;
                        let entry = Entry {
                            thread: thread.clone(),
                            link,
                        };
                        if !self
                            .context
                            .state
                            .lock()
                            .await
                            .missing
                            .contains(&group_key(&entry))
                        {
                            self.request_sync(link_key(&entry.link)).await?;
                        }
                    }
                }
            }
            "thread.settled" => self.refresh_unmerged(id).await?,
            "run.updated"
                if matches!(
                    payload["status"].as_str(),
                    Some("completed" | "failed" | "cancelled" | "interrupted" | "rolled_back")
                ) =>
            {
                if self
                    .context
                    .state
                    .lock()
                    .await
                    .close_commands
                    .remove(&id)
                    .is_some()
                {
                    self.refresh_unmerged(id).await?;
                }
            }
            "turn-item.updated" if payload["type"] == "command_execution" => {
                let observation = self.context.services.policy.command(
                    payload["input"].as_str().unwrap_or_default(),
                    payload["output"].as_str().unwrap_or_default(),
                );
                let active = matches!(
                    payload["status"].as_str(),
                    Some("pending" | "running" | "waiting")
                );
                if observation.closes {
                    self.context
                        .state
                        .lock()
                        .await
                        .close_commands
                        .entry(id.clone())
                        .or_default();
                }
                if active {
                    return Ok(());
                }
                let item = payload["id"].as_str().ok_or(Error::Decode {
                    entity: "command item",
                    detail: "missing id".into(),
                })?;
                for (created, url) in observation.created {
                    let created = normalize_pull_request_key(
                        self.context.services.identities.as_ref(),
                        &created,
                        Some(&url),
                    )?;
                    if let Err(error) = self.context.services.commands.dispatch(serde_json::from_value(json!({"type":"thread.pull-request.link","commandId":format!("server:pr-created-link:{}:{item}:{}",id.0,key(&created)),"threadId":id,"host":created.host,"repository":created.repository,"number":created.number,"url":url,"source":"created"}))?).await {
                        if matches!(error,Error::Cancelled) {return Err(error);}
                        eprintln!("linking created pull request failed: {error}");
                    }
                }
                if observation.pushes {
                    let answered = self
                        .context
                        .services
                        .projections
                        .pull_request_snapshot(Some(id.clone()), false)
                        .await?
                        .threads
                        .iter()
                        .any(|thread| {
                            thread
                                .pull_requests
                                .iter()
                                .any(|link| json!(link.watch).get("checksPending").is_some())
                        });
                    if answered {
                        if let Err(error) = self.context.services.commands.dispatch(serde_json::from_value(json!({"type":"thread.pull-request-watch.checks-reopen","commandId":format!("server:pr-push-checks:{}:{item}",id.0),"threadId":id}))?).await {
                            if matches!(error,Error::Cancelled) {return Err(error);}
                            eprintln!("reopening watch checks failed: {error}");
                        }
                    }
                }
                if observation.closes {
                    let fresh = self
                        .context
                        .state
                        .lock()
                        .await
                        .close_commands
                        .entry(id.clone())
                        .or_default()
                        .insert(item.into());
                    if fresh {
                        self.refresh_unmerged(id).await?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub async fn shutdown(&self) {
        self.scope.shutdown().await;
        self.worker.shutdown().await;
        self.tasks.close();
        self.tasks.wait().await;
    }
}
impl Context {
    async fn synchronize(&self, all: bool) -> Result<()> {
        let _sweep = self.sweep.lock().await;
        let snapshot = self
            .services
            .projections
            .pull_request_snapshot(None, false)
            .await?;
        let now = self.services.clock.now();
        let now_ms = date_ms(&now.0).ok_or(Error::Decode {
            entity: "sync clock",
            detail: now.0.clone(),
        })?;
        let mut groups: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
        for thread in snapshot.threads {
            for mut link in thread
                .pull_requests
                .iter()
                .filter(|link| link.source != ThreadPullRequestLinkSource::StackDismissed)
                .cloned()
            {
                let normalized = normalize_pull_request_key(
                    self.services.identities.as_ref(),
                    &link_key(&link),
                    Some(&link.url.0),
                )?;
                link.host = normalized.host;
                link.repository = normalized.repository;
                let entry = Entry {
                    thread: thread.clone(),
                    link,
                };
                groups.entry(group_key(&entry)).or_default().push(entry);
            }
        }
        let mut state = self.state.lock().await;
        let pending = std::mem::take(&mut state.pending);
        for (key, entries) in &groups {
            if pending.contains(
                &crate::ports::thread_project::pull_request_sync_reactor::key(&link_key(
                    &entries[0].link,
                )),
            ) {
                state.requested.insert(key.clone());
            }
        }
        state.last.retain(|key, _| groups.contains_key(key));
        state.retry_stacks.retain(|key| groups.contains_key(key));
        state.missing.retain(|key| groups.contains_key(key));
        state.requested.retain(|key| groups.contains_key(key));
        let mut hosts: BTreeMap<String, Vec<(&String, &Vec<Entry>)>> = BTreeMap::new();
        for (key, entries) in &groups {
            if all || state.requested.contains(key) {
                hosts
                    .entry(pause_key(&entries[0]))
                    .or_default()
                    .push((key, entries));
            }
        }
        drop(state);
        let batched = Arc::new(Semaphore::new(100));
        let unbatched = Arc::new(Semaphore::new(25));
        let linked = Mutex::new(BTreeSet::new());
        let stacks = Mutex::new(BTreeMap::new());
        let persistence = Mutex::new(());
        let mut reads: Vec<Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>> = Vec::new();
        for (host, host_groups) in hosts {
            let batched = batched.clone();
            let unbatched = unbatched.clone();
            let groups = &groups;
            let now = &now;
            let linked = &linked;
            let stacks = &stacks;
            let persistence = &persistence;
            reads.push(Box::pin(async move {
                let state = self.state.lock().await;
                if state.pauses.get(&host).is_some_and(|until| *until > now_ms)
                    || !host_groups
                        .iter()
                        .any(|(key, entries)| due(&state, key, entries, now_ms))
                {
                    return Ok(());
                }
                drop(state);
                let batches = self
                    .services
                    .pull_requests
                    .batches_reads(reference(&host_groups[0].1[0])?)
                    .await?;
                let slots = if batches { batched } else { unbatched };
                let mut host_reads: Vec<Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>> =
                    Vec::new();
                for (key, entries) in host_groups {
                    let slots = slots.clone();
                    let host = &host;
                    host_reads.push(Box::pin(async move {
                        let _permit = slots.acquire().await.map_err(|_| Error::Cancelled)?;
                        let state = self.state.lock().await;
                        let clock = date_ms(&self.services.clock.now().0).unwrap_or(now_ms);
                        if !due(&state, key, entries, now_ms)
                            || state.pauses.get(host).is_some_and(|until| *until > clock)
                        {
                            return Ok(());
                        }
                        drop(state);
                        if let Err(error) = self
                            .sync_group(
                                key,
                                entries,
                                groups,
                                now,
                                now_ms,
                                linked,
                                stacks,
                                persistence,
                            )
                            .await
                        {
                            if matches!(error, Error::Cancelled) {
                                return Err(error);
                            }
                            if let Error::PullRequestHost(PullRequestHostError::RateLimited {
                                retry_at: Some(at),
                                ..
                            }) = &error
                                && let Some(at) = date_ms(&at.0)
                            {
                                let mut state = self.state.lock().await;
                                let pause = state.pauses.entry(host.clone()).or_default();
                                *pause = (*pause).max(at);
                            }
                            eprintln!("pull request sync skipped: {key}: {error}");
                        }
                        Ok(())
                    }));
                }
                join_thread_reads(host_reads).await
            }));
        }
        join_thread_reads(reads).await
    }
    async fn sync_group(
        &self,
        key: &str,
        entries: &[Entry],
        groups: &BTreeMap<String, Vec<Entry>>,
        now: &t3_contracts::base_schemas::IsoDateTime,
        now_ms: i64,
        linked: &Mutex<BTreeSet<String>>,
        stacks: &Mutex<BTreeMap<String, Option<ThreadPullRequestStack>>>,
        persistence: &Mutex<()>,
    ) -> Result<()> {
        let reference = reference(&entries[0])?;
        let state = self.state.lock().await;
        let requested = state.requested.contains(key);
        let invalidated = requested
            || state.retry_stacks.contains(key)
            || entries.iter().any(|entry| {
                entry.link.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot.state == PullRequestState::Merged
                        && snapshot
                            .merged_at
                            .as_ref()
                            .and_then(Option::as_ref)
                            .is_none()
                })
            });
        drop(state);
        if invalidated {
            self.services
                .pull_requests
                .invalidate(
                    serde_json::from_value(json!({"reference":reference}))?,
                    false,
                )
                .await?;
        }
        let summary = match self.services.pull_requests.summary(reference.clone()).await {
            Ok(summary) => summary,
            Err(error) => {
                if matches!(
                    &error,
                    Error::PullRequestHost(PullRequestHostError::NotFound { .. })
                ) {
                    let mut state = self.state.lock().await;
                    state.missing.insert(key.into());
                    state.last.insert(key.into(), now_ms);
                    state.requested.remove(key);
                }
                return Err(error);
            }
        };
        let fields = snapshot_fields(&summary);
        let mut state = self.state.lock().await;
        state.missing.remove(key);
        let summary_value = json!(summary);
        let needs_stack = requested
            || state.retry_stacks.contains(key)
            || entries.iter().any(|entry| {
                entry.link.snapshot.as_ref().is_none_or(|previous| {
                    !fields_equal(
                        &json!(previous),
                        &known_mergeability(&entry.link.snapshot, fields.clone(), &summary),
                        self.services.policy.as_ref(),
                    )
                }) || summary_value
                    .get("stack")
                    .is_some_and(|stack| stack["number"] != json!(entry.link.stack)["number"])
            });
        let unstacked = summary_value.get("stack") == Some(&Value::Null)
            && entries.iter().all(|entry| entry.link.stack.is_none());
        let invalidate_stack = !invalidated && state.retry_stacks.contains(key) && !unstacked;
        if needs_stack {
            state.retry_stacks.remove(key);
        }
        drop(state);
        if invalidate_stack {
            self.services
                .pull_requests
                .invalidate(
                    serde_json::from_value(json!({"reference":reference}))?,
                    false,
                )
                .await?;
        }
        let fetched = if !needs_stack {
            None
        } else if unstacked {
            Some(None)
        } else {
            match self.services.pull_requests.stack(reference, false).await {
                Ok(stack) => Some(stack),
                Err(error) => {
                    self.state.lock().await.retry_stacks.insert(key.into());
                    return Err(error);
                }
            }
        };
        if let Some(fetched) = &fetched {
            let mut read = stacks.lock().await;
            read.insert(key.into(), fetched.clone());
            if let Some(fetched) = fetched {
                let first = &entries[0];
                let mut state = self.state.lock().await;
                for (other, siblings) in groups {
                    if other == key || siblings[0].thread.project_id != first.thread.project_id {
                        continue;
                    }
                    let stale = siblings.iter().any(|entry| {
                        let stack = read.get(other).unwrap_or(&entry.link.stack);
                        stack.as_ref().is_some_and(|stack| {
                            stack.id == fetched.id
                                && entry.link.host == first.link.host
                                && entry.link.repository == first.link.repository
                                && stack
                                    .layers
                                    .iter()
                                    .map(|layer| layer.number)
                                    .collect::<Vec<_>>()
                                    != fetched
                                        .layers
                                        .iter()
                                        .map(|layer| layer.number)
                                        .collect::<Vec<_>>()
                        })
                    });
                    if stale {
                        state.retry_stacks.insert(other.clone());
                    }
                }
            }
        }
        {
            let mut state = self.state.lock().await;
            state.last.insert(key.into(), now_ms);
            state.requested.remove(key);
        }
        for entry in entries {
            let _persist = persistence.lock().await;
            if let Err(error) = self
                .sync_entry(
                    entry,
                    fields.clone(),
                    fetched.as_ref(),
                    &summary,
                    now,
                    linked,
                )
                .await
            {
                if matches!(error, Error::Cancelled) {
                    return Err(error);
                }
                self.state.lock().await.retry_stacks.insert(key.into());
                eprintln!("pull request sync persistence skipped: {error}");
            }
        }
        Ok(())
    }
    async fn sync_entry(
        &self,
        entry: &Entry,
        fields: Value,
        fetched: Option<&Option<ThreadPullRequestStack>>,
        summary: &PullRequestSummary,
        now: &t3_contracts::base_schemas::IsoDateTime,
        linked: &Mutex<BTreeSet<String>>,
    ) -> Result<()> {
        let fields = known_mergeability(&entry.link.snapshot, fields, summary);
        let stack = fetched.unwrap_or(&entry.link.stack);
        if unsettled(&entry.thread)
            && let Some(Some(stack)) = fetched
        {
            for layer in &stack.layers {
                let layer_key: ThreadPullRequestKey = serde_json::from_value(
                    json!({"host":entry.link.host,"repository":entry.link.repository,"number":layer.number}),
                )?;
                let dedupe = format!("{}:{}", entry.thread.id.0, key(&layer_key));
                if linked.lock().await.contains(&dedupe) {
                    continue;
                }
                let mut present = false;
                for existing in &entry.thread.pull_requests {
                    if normalize_pull_request_key(
                        self.services.identities.as_ref(),
                        &link_key(existing),
                        Some(&existing.url.0),
                    )? == layer_key
                    {
                        present = true;
                        break;
                    }
                }
                if present {
                    continue;
                }
                let Some(url) = self
                    .services
                    .policy
                    .sibling_url(&entry.link.url.0, layer.number)
                else {
                    continue;
                };
                self.services.commands.dispatch(serde_json::from_value(json!({"type":"thread.pull-request.link","commandId":format!("server:pr-stack-link:{}:{}",entry.thread.id.0,self.services.ids.next(IdKind::Command)),"threadId":entry.thread.id,"host":layer_key.host,"repository":layer_key.repository,"number":layer_key.number,"url":url,"source":"stack"}))?).await?;
                linked.lock().await.insert(dedupe);
            }
        }
        if entry.link.snapshot.as_ref().is_none_or(|previous| {
            !fields_equal(&json!(previous), &fields, self.services.policy.as_ref())
        }) || &entry.link.stack != stack
        {
            let mut snapshot = fields;
            snapshot["syncedAt"] = json!(now);
            self.services.commands.dispatch(serde_json::from_value(json!({"type":"thread.pull-request-link.sync","commandId":format!("server:pr-sync:{}:{}",entry.thread.id.0,self.services.ids.next(IdKind::Command)),"threadId":entry.thread.id,"host":entry.link.host,"repository":entry.link.repository,"number":entry.link.number,"snapshot":snapshot,"stack":stack}))?).await?;
        }
        Ok(())
    }
}
