use crate::{
    Error, FailureReason, Result,
    cache::{Clock, PullRequestReadCache},
    context::{self, Context, ProjectSource},
    host::*,
};
use futures_util::{
    FutureExt,
    future::{BoxFuture, WeakShared},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use t3_contracts::{
    base_schemas::ProjectId, pull_request::*, thread_pull_request::ThreadPullRequestKey,
};
use tokio::sync::{Mutex as AsyncMutex, broadcast, watch};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
mod listing;
mod mutations;
mod viewed;

#[derive(Clone)]
struct Held {
    value: Value,
    at: u64,
    ttl: u64,
    epoch: u64,
    order: u64,
    bytes: usize,
}
#[derive(Default)]
struct Memory {
    entries: HashMap<String, Held>,
    flights: HashMap<String, std::sync::Weak<AsyncMutex<()>>>,
    reads: HashMap<String, SharedRead>,
    revision_refreshing: std::collections::HashSet<String>,
    diff_sweep_armed: bool,
    sequence: u64,
}
type SharedRead = WeakShared<BoxFuture<'static, Result<Value>>>;
#[derive(Default)]
struct Pause {
    retry_at: u64,
    attempts: u32,
    generation: u64,
}
struct ReadPolicy {
    kind: &'static str,
    ttl: u64,
    bypass: bool,
    allow_stale: bool,
    persist: bool,
}
const REPORTED_STATE_CAPACITY: usize = 2_048;
/// Report time, state, draft flag and insertion order, keyed by pull request scope.
#[derive(Default)]
struct ReportedStates {
    entries: HashMap<String, (u64, PullRequestState, Option<bool>, u64)>,
    order: u64,
}
struct Inner {
    projects: Arc<dyn ProjectSource>,
    clock: Arc<dyn Clock>,
    persistent: Arc<PullRequestReadCache>,
    memory: Mutex<Memory>,
    list_epoch: AtomicU64,
    epoch: AtomicU64,
    versions: Mutex<HashMap<String, u64>>,
    pauses: Mutex<HashMap<String, Pause>>,
    refreshes: watch::Sender<u64>,
    linked: watch::Sender<u64>,
    merges: broadcast::Sender<MergeEvent>,
    states: broadcast::Sender<ThreadPullRequestKey>,
    reports: Mutex<ReportedStates>,
    closed: AtomicBool,
    cancel: CancellationToken,
    tasks: TaskTracker,
    maintenance: TaskTracker,
    records: Arc<dyn crate::linked::Records>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
#[derive(Debug, Clone)]
pub struct MergeEvent {
    pub reference: PullRequestRef,
    pub merged_at: String,
}
#[derive(Debug, Clone, Copy, Default)]
pub struct SummaryOptions {
    pub bypass_cache: bool,
    pub recover_transient_failure: Option<bool>,
}
#[derive(Clone)]
pub struct PullRequestService(Arc<Inner>);
fn preview_fields(value: impl Serialize) -> Result<PullRequestPreview> {
    let mut raw = serde_json::to_value(value)
        .map_err(|_| Error::invalid("preview", "Invalid preview data."))?;
    if let Some(object) = raw.as_object_mut() {
        object.remove("updatedAt");
        object.remove("observedAt");
    }
    crate::convert(raw)
}
impl PullRequestService {
    pub fn new(
        projects: Arc<dyn ProjectSource>,
        clock: Arc<dyn Clock>,
        persistent: Arc<PullRequestReadCache>,
    ) -> Self {
        Self::with_records(
            projects,
            clock,
            persistent,
            Arc::new(crate::linked::MemoryRecords::default()),
        )
    }
    pub fn with_records(
        projects: Arc<dyn ProjectSource>,
        clock: Arc<dyn Clock>,
        persistent: Arc<PullRequestReadCache>,
        records: Arc<dyn crate::linked::Records>,
    ) -> Self {
        let tasks = TaskTracker::new();
        tasks.close();
        let maintenance = TaskTracker::new();
        maintenance.close();
        Self(Arc::new(Inner {
            projects,
            clock,
            persistent,
            memory: Mutex::new(Memory::default()),
            list_epoch: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            versions: Mutex::new(HashMap::new()),
            pauses: Mutex::new(HashMap::new()),
            refreshes: watch::channel(0).0,
            linked: watch::channel(0).0,
            merges: broadcast::channel(64).0,
            states: broadcast::channel(64).0,
            reports: Mutex::new(ReportedStates::default()),
            closed: AtomicBool::new(false),
            cancel: CancellationToken::new(),
            tasks,
            maintenance,
            records,
        }))
    }
    pub fn subscribe_refreshes(&self) -> watch::Receiver<u64> {
        self.0.refreshes.subscribe()
    }
    pub fn subscribe_linked_thread_refreshes(&self) -> watch::Receiver<u64> {
        self.0.linked.subscribe()
    }
    pub fn subscribe_merges(&self) -> broadcast::Receiver<MergeEvent> {
        self.0.merges.subscribe()
    }
    pub fn subscribe_state_changes(&self) -> broadcast::Receiver<ThreadPullRequestKey> {
        self.0.states.subscribe()
    }
    pub fn notify_linked_threads_changed(&self) {
        self.0.linked.send_modify(|v| *v = v.saturating_add(1));
    }
    pub async fn shutdown(&self) {
        self.0.closed.store(true, Ordering::Release);
        self.0.cancel.cancel();
        self.0.tasks.close();
        self.0.tasks.wait().await;
        self.0.maintenance.wait().await;
        self.0.persistent.drain().await;
    }
    pub async fn drain(&self) {
        self.0.tasks.wait().await;
    }
    pub fn sweep_diff_cache(&self) -> usize {
        let now = self.0.clock.now_ms();
        let mut memory = self.0.memory.lock().unwrap_or_else(|e| e.into_inner());
        let before = memory.entries.len();
        memory.entries.retain(|key, value| {
            !key.starts_with("diff:") || now <= value.at.saturating_add(600_000)
        });
        before - memory.entries.len()
    }
    fn arm_diff_sweep(&self) {
        {
            let mut memory = self.0.memory.lock().unwrap_or_else(|e| e.into_inner());
            if memory.diff_sweep_armed {
                return;
            }
            memory.diff_sweep_armed = true;
        }
        let service = Arc::downgrade(&self.0);
        let cancel = self.0.cancel.clone();
        self.0.maintenance.spawn(async move{loop{
            tokio::select!{_ = cancel.cancelled()=>break,_ = tokio::time::sleep(std::time::Duration::from_secs(60))=>{}}
            let Some(inner)=service.upgrade()else{break;};let service=PullRequestService(inner);service.sweep_diff_cache();
            let mut memory=service.0.memory.lock().unwrap_or_else(|e|e.into_inner());if !memory.entries.keys().any(|key|key.starts_with("diff:")){memory.diff_sweep_armed=false;break;}
        }});
    }
    fn admit(&self) -> Result<tokio_util::task::task_tracker::TaskTrackerToken> {
        let token = self.0.tasks.token();
        if self.0.closed.load(Ordering::Acquire) {
            return Err(Error::invalid(
                "pullRequests",
                "Pull request service is shut down.",
            ));
        }
        Ok(token)
    }
    async fn current_projects(&self) -> Result<Vec<crate::context::Binding>> {
        let _activity = self.admit()?;
        tokio::select! {_ = self.0.cancel.cancelled()=>Err(Error::invalid("pullRequests","Pull request service is shut down.")),result=self.0.projects.projects()=>result}
    }
    fn bump_scope(&self, scope: &str) {
        let mut versions = self.0.versions.lock().unwrap_or_else(|e| e.into_inner());
        if versions.len() >= 2_048 && !versions.contains_key(scope) {
            versions.clear();
            self.0.epoch.fetch_add(1, Ordering::AcqRel);
        }
        let version = versions.entry(scope.into()).or_default();
        *version = version.saturating_add(1);
    }
    pub(crate) async fn project(&self, id: &ProjectId) -> Result<crate::context::Binding> {
        self.current_projects()
            .await?
            .into_iter()
            .find(|b| b.project.id == *id)
            .ok_or_else(|| {
                Error::invalid(
                    "resolveRepository",
                    "The selected project could not be read.",
                )
            })
    }
    pub async fn context(&self, input: &PullRequestRef) -> Result<Context> {
        let activity = Arc::new(self.0.tasks.token());
        if self.0.closed.load(Ordering::Acquire) {
            return Err(Error::invalid(
                "pullRequests",
                "Pull request service is shut down.",
            ));
        }
        let mut ctx = tokio::select! {_ = self.0.cancel.cancelled()=>return Err(Error::invalid("pullRequests","Pull request service is shut down.")),ctx=context::resolve(self.0.projects.as_ref(),input)=>ctx?};
        ctx.activity = Some(activity);
        Ok(ctx)
    }
    pub async fn batches_reads(&self, input: &PullRequestRef) -> bool {
        self.context(input)
            .await
            .is_ok_and(|c| c.host.batches_reads())
    }
    pub async fn changes(&self, input: &PullRequestRef) -> Result<Changes> {
        let context = self.context(input).await?;
        context.host.changes(&context).await
    }
    pub(crate) async fn protect<T>(
        &self,
        ctx: &Context,
        interactive: bool,
        read: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        let key = json!([
            ctx.host.kind(),
            ctx.project.host.to_lowercase(),
            ctx.credential_namespace
        ])
        .to_string();
        let now = self.0.clock.now_ms();
        let lease = {
            let mut pauses = self.0.pauses.lock().unwrap_or_else(|e| e.into_inner());
            let pause = pauses.entry(key.clone()).or_default();
            if !interactive && pause.retry_at > now {
                return Err(Error {
                    reason: FailureReason::RateLimited,
                    retry_at: Some(pause.retry_at),
                    ..Error::invalid("read", "This host has paused pull request reads.")
                });
            }
            pause.generation
        };
        let result = tokio::select! {_ = self.0.cancel.cancelled()=>Err(Error::invalid("pullRequests","Pull request service is shut down.")),result=read=>result};
        let mut pauses = self.0.pauses.lock().unwrap_or_else(|e| e.into_inner());
        let pause = pauses.entry(key).or_default();
        match &result {
            Ok(_) if pause.generation == lease => {
                pause.attempts = 0;
                pause.retry_at = 0;
            }
            Err(e) if e.reason == FailureReason::RateLimited => {
                pause.attempts = pause.attempts.saturating_add(1);
                pause.generation = pause.generation.saturating_add(1);
                let fallback = 30_000u64
                    .saturating_mul(1u64 << pause.attempts.saturating_sub(1).min(5))
                    .min(900_000);
                pause.retry_at = pause.retry_at.max(
                    e.retry_at
                        .filter(|t| *t > now)
                        .unwrap_or(now.saturating_add(fallback)),
                );
            }
            _ => {}
        }
        result
    }
    fn held(&self, key: &str) -> Option<Held> {
        let mut mem = self.0.memory.lock().unwrap_or_else(|e| e.into_inner());
        mem.sequence = mem.sequence.saturating_add(1);
        let order = mem.sequence;
        let entry = mem.entries.get_mut(key)?;
        if entry.epoch != self.0.epoch.load(Ordering::Acquire) {
            return None;
        }
        entry.order = order;
        Some(entry.clone())
    }
    fn cache_key(&self, ctx: &Context, kind: &str, suffix: &str) -> String {
        let versions = self.0.versions.lock().unwrap_or_else(|e| e.into_inner());
        let viewed = if kind == "viewed" {
            Some([
                versions.get("viewed:all").copied().unwrap_or(0),
                versions
                    .get(&format!("viewed:{}", ctx.scope()))
                    .copied()
                    .unwrap_or(0),
            ])
        } else {
            None
        };
        let scope = json!([
            ctx.cache_key(),
            versions.get(&ctx.scope()).copied().unwrap_or(0),
            versions
                .get(&format!("project:{}", ctx.project.id.0))
                .copied()
                .unwrap_or(0),
            viewed
        ]);
        format!("{kind}:{scope}:{suffix}")
    }
    async fn viewer_cached(&self, ctx: &Context) -> Result<String> {
        if let Some(viewer) = &ctx.viewer {
            return Ok(viewer.clone());
        }
        let next = ctx.clone();
        self.read(
            ctx.clone(),
            ReadPolicy {
                kind: "viewer",
                ttl: 600_000,
                bypass: false,
                allow_stale: false,
                persist: false,
            },
            String::new(),
            async move { next.host.viewer(&next.project).await },
        )
        .await
    }
    fn store(&self, key: String, kind: &str, value: Value, at: u64, ttl: u64, epoch: u64) {
        if epoch != self.0.epoch.load(Ordering::Acquire) {
            return;
        }
        let bytes = if kind == "diff" {
            value
                .get("patch")
                .and_then(Value::as_str)
                .map_or(0, |s| s.encode_utf16().count() * 2)
        } else {
            0
        };
        if bytes > 32 * 1024 * 1024 {
            return;
        }
        let mut mem = self.0.memory.lock().unwrap_or_else(|e| e.into_inner());
        if (kind == "summary" || kind == "detail")
            && let Some(held) = mem.entries.get(&key)
        {
            let current = &held.value;
            if current.get("state") == Some(&json!("merged"))
                && value.get("state") != Some(&json!("merged"))
            {
                return;
            }
            if value.get("updatedAt").and_then(Value::as_str)
                < current.get("updatedAt").and_then(Value::as_str)
            {
                return;
            }
        }
        mem.sequence = mem.sequence.saturating_add(1);
        let order = mem.sequence;
        mem.entries.insert(
            key,
            Held {
                value,
                at,
                ttl,
                epoch,
                order,
                bytes,
            },
        );
        let prefix = format!("{kind}:");
        let capacity = match kind {
            "list" => 64,
            "stats" | "viewer" => 32,
            "revisions" => 64,
            _ => 128,
        };
        loop {
            let entries: Vec<_> = mem
                .entries
                .iter()
                .filter(|(key, _)| key.starts_with(&prefix))
                .collect();
            if entries.len() <= capacity
                && entries.iter().map(|(_, e)| e.bytes).sum::<usize>() <= 32 * 1024 * 1024
            {
                break;
            }
            let oldest = entries
                .iter()
                .min_by_key(|(_, e)| e.order)
                .map(|(key, _)| (*key).clone());
            if let Some(key) = oldest {
                mem.entries.remove(&key);
            } else {
                break;
            }
        }
    }
    async fn read<T, F>(
        &self,
        ctx: Context,
        policy: ReadPolicy,
        suffix: String,
        lookup: F,
    ) -> Result<T>
    where
        T: Serialize + DeserializeOwned + Send + 'static,
        F: Future<Output = Result<T>> + Send + 'static,
    {
        let ReadPolicy {
            kind,
            ttl,
            bypass,
            allow_stale,
            persist,
        } = policy;
        let key = self.cache_key(&ctx, kind, &suffix);
        let now = self.0.clock.now_ms();
        let held = if bypass { None } else { self.held(&key) };
        if let Some(held) = &held
            && now < held.at.saturating_add(held.ttl)
        {
            return crate::convert(&held.value);
        }
        let service = self.clone();
        let key_read = key.clone();
        let summary_key = self.cache_key(&ctx, "summary", "");
        let stat_key = self.cache_key(&ctx, "stat", "");
        let checks_key = self.cache_key(&ctx, "checks", "");
        let fetch = async move {
            if !bypass
                && let Some(held) = service.held(&key_read)
                && service.0.clock.now_ms() < held.at.saturating_add(held.ttl)
            {
                return Ok(held.value);
            }
            let epoch = service.0.epoch.load(Ordering::Acquire);
            let at = service.0.clock.now_ms();
            let scopes = vec![
                ctx.scope(),
                format!("project:{}", ctx.project.id.0.as_str()),
                "all".into(),
            ];
            let value = if persist && !bypass {
                service
                    .0
                    .persistent
                    .get(&key_read, &scopes, service.protect(&ctx, false, lookup))
                    .await?
            } else {
                service.protect(&ctx, false, lookup).await?
            };
            let mut raw = serde_json::to_value(&value)
                .map_err(|_| Error::invalid(kind, "Invalid pull request result."))?;
            if matches!(kind, "detail" | "summary" | "checks")
                && let Some(object) = raw.as_object_mut()
            {
                object.entry("observedAt").or_insert(json!(at));
            }
            let ttl = if matches!(kind, "detail" | "summary")
                && raw.get("state") == Some(&json!("merged"))
            {
                600_000
            } else {
                ttl
            };
            if kind == "detail" {
                let summary: PullRequestSummary = crate::convert(&raw)?;
                service.store(summary_key, "summary", json!(summary), at, ttl, epoch);
                let stat = PullRequestDiffStat {
                    project_id: ctx.project.id.clone(),
                    repository: ctx.reference.repository.clone(),
                    number: ctx.reference.number,
                    additions: raw["additions"].as_u64().unwrap_or(0),
                    deletions: raw["deletions"].as_u64().unwrap_or(0),
                };
                service.store(stat_key, "stat", json!(stat), at, 60_000, epoch);
                if ctx.host.kind() == "github"
                    && service
                        .held(&checks_key)
                        .is_none_or(|h| at >= h.at.saturating_add(h.ttl))
                {
                    service.store(
                        checks_key,
                        "checks",
                        json!({"state":raw["state"],"checks":raw["checks"],"observedAt":at}),
                        at,
                        15_000,
                        epoch,
                    );
                }
            }
            service.store(key_read, kind, raw.clone(), at, ttl, epoch);
            if kind == "diff" {
                service.arm_diff_sweep();
            }
            Ok(raw)
        };
        let fetch = {
            let mut mem = self.0.memory.lock().unwrap_or_else(|e| e.into_inner());
            mem.reads.retain(|_, v| v.upgrade().is_some());
            let key = if bypass { format!("bypass:{key}") } else { key };
            match mem.reads.get(&key).and_then(WeakShared::upgrade) {
                Some(existing) => existing,
                None => {
                    let future = fetch.boxed().shared();
                    if let Some(weak) = future.downgrade() {
                        mem.reads.insert(key, weak);
                    }
                    future
                }
            }
        };
        if allow_stale
            && let Some(held) = held
            && now <= held.at.saturating_add(600_000)
        {
            let cancel = self.0.cancel.clone();
            self.0.tasks.spawn(async move {
                tokio::select! {_ = cancel.cancelled()=>{},_ = fetch=>{}}
            });
            return crate::convert(held.value);
        }
        tokio::select! {_ = self.0.cancel.cancelled()=>Err(Error::invalid(kind,"Pull request service is shut down.")),result=fetch=>result.and_then(crate::convert)}
    }
    pub async fn summary(
        &self,
        input: PullRequestRef,
        options: SummaryOptions,
    ) -> Result<PullRequestSummary> {
        let ctx = self.context(&input).await?;
        let next = ctx.clone();
        let stale = input.allow_stale != Some(Some(false))
            && options.recover_transient_failure != Some(false);
        self.read(
            ctx,
            ReadPolicy {
                kind: "summary",
                ttl: 60_000,
                bypass: options.bypass_cache,
                allow_stale: stale,
                persist: true,
            },
            String::new(),
            async move { next.host.summary(&next).await },
        )
        .await
    }
    pub async fn stack(
        &self,
        input: PullRequestRef,
        include_details: bool,
    ) -> Result<Option<PullRequestStack>> {
        let ctx = self.context(&input).await?;
        let next = ctx.clone();
        self.read(
            ctx,
            ReadPolicy {
                kind: "stack",
                ttl: 60_000,
                bypass: false,
                allow_stale: false,
                persist: true,
            },
            include_details.to_string(),
            async move { next.host.stack(&next, include_details).await },
        )
        .await
    }
    pub async fn detail(&self, input: PullRequestRef) -> Result<PullRequestDetail> {
        let mut ctx = self.context(&input).await?;
        ctx.viewer = self.viewer_cached(&ctx).await.ok();
        let next = ctx.clone();
        self.read(
            ctx,
            ReadPolicy {
                kind: "detail",
                ttl: 60_000,
                bypass: false,
                allow_stale: input.allow_stale != Some(Some(false)),
                persist: false,
            },
            String::new(),
            async move { next.host.detail(&next).await },
        )
        .await
    }
    pub async fn preview(&self, input: PullRequestRef) -> Result<PullRequestPreview> {
        let ctx = self.context(&input).await?;
        if let Some(held) = self.held(&self.cache_key(&ctx, "detail", ""))
            && self.0.clock.now_ms() < held.at.saturating_add(held.ttl)
        {
            return preview_fields(held.value);
        }
        let next = ctx.clone();
        self.read(
            ctx,
            ReadPolicy {
                kind: "preview",
                ttl: 60_000,
                bypass: false,
                allow_stale: false,
                persist: false,
            },
            String::new(),
            async move { next.host.preview(&next).await },
        )
        .await
    }
    pub async fn checks(&self, input: PullRequestRef) -> Result<Option<PullRequestChecks>> {
        let ctx = self.context(&input).await?;
        let next = ctx.clone();
        self.read(
            ctx,
            ReadPolicy {
                kind: "checks",
                ttl: 15_000,
                bypass: false,
                allow_stale: false,
                persist: false,
            },
            String::new(),
            async move { next.host.checks(&next).await },
        )
        .await
    }
    pub async fn watch_fingerprint(&self, input: PullRequestRef) -> Result<Option<Fingerprint>> {
        let ctx = self.context(&input).await?;
        let next = ctx.clone();
        self.read(
            ctx,
            ReadPolicy {
                kind: "fingerprint",
                ttl: 15_000,
                bypass: false,
                allow_stale: false,
                persist: false,
            },
            String::new(),
            async move { next.host.watch_fingerprint(&next).await },
        )
        .await
    }
    pub async fn activity(&self, input: PullRequestRef) -> Result<PullRequestActivity> {
        let ctx = self.context(&input).await?;
        let next = ctx.clone();
        self.read(
            ctx,
            ReadPolicy {
                kind: "activity",
                ttl: 60_000,
                bypass: false,
                allow_stale: false,
                persist: false,
            },
            String::new(),
            async move { next.host.activity(&next).await },
        )
        .await
    }
    pub async fn thread_comments(
        &self,
        input: PullRequestThreadCommentsInput,
    ) -> Result<PullRequestThreadCommentsResult> {
        let ctx = self.context(&crate::convert(&input)?).await?;
        self.protect(&ctx, false, ctx.host.thread_comments(&ctx, &input))
            .await
    }
    pub async fn diff(&self, input: PullRequestDiffInput) -> Result<PullRequestDiffResult> {
        let ctx = self.context(&crate::convert(&input)?).await?;
        if !ctx.host.capabilities().diff {
            return Err(Error::unsupported("diff"));
        }
        let summary_revision = if input.commit.as_ref().and_then(Option::as_ref).is_none() {
            self.held(&self.cache_key(&ctx, "summary", ""))
                .and_then(|held| held.value.get("updatedAt").cloned())
        } else {
            None
        };
        let suffix = json!([input.cursor, input.commit, summary_revision]).to_string();
        let ttl = if input.commit.as_ref().and_then(Option::as_ref).is_some() {
            600_000
        } else {
            60_000
        };
        let next = ctx.clone();
        self.read(
            ctx,
            ReadPolicy {
                kind: "diff",
                ttl,
                bypass: false,
                allow_stale: input.allow_stale != Some(Some(false)),
                persist: false,
            },
            suffix,
            async move { next.host.diff(&next, &input).await },
        )
        .await
    }
    pub async fn diff_file_contents(
        &self,
        input: PullRequestDiffFileContentsInput,
    ) -> Result<PullRequestDiffFileContentsResult> {
        let ctx = self.context(&crate::convert(&input)?).await?;
        if !ctx.host.capabilities().diff {
            return Err(Error::unsupported("diffFileContents"));
        }
        let mut diff: PullRequestDiffInput = crate::convert(&input)?;
        let mut cursors = std::collections::HashSet::new();
        loop {
            let paths = self
                .protect(&ctx, false, ctx.host.diff_paths(&ctx, &diff))
                .await?;
            if paths
                .files
                .iter()
                .any(|(old, new)| old == input.old_path.as_str() && new == input.new_path.as_str())
            {
                break;
            }
            let Some(cursor) = paths.next_cursor else {
                return Err(Error {
                    reason: FailureReason::NotFound,
                    ..Error::invalid(
                        "diffFileContents",
                        "This file is not part of the change request.",
                    )
                });
            };
            if !cursors.insert(cursor.clone()) {
                return Err(Error::invalid(
                    "diffFileContents",
                    "The host repeated a diff page cursor.",
                ));
            }
            diff.cursor = Some(Some(cursor.into()));
        }
        self.protect(&ctx, false, ctx.host.diff_file_contents(&ctx, &input))
            .await
    }
    pub async fn routing(&self, input: PullRequestRef) -> Result<PullRequestRoutingResult> {
        let ctx = self.context(&input).await?;
        let identity = self
            .protect(&ctx, true, ctx.host.routing_identity(&ctx.project))
            .await?;
        if identity.viewer.trim().is_empty() || identity.account_id.trim().is_empty() {
            return Err(Error::invalid(
                "routeIdentity",
                "The signed-in account could not be verified.",
            ));
        }
        crate::convert(
            json!({"host":ctx.project.host,"provider":ctx.host.kind(),"accountId":identity.account_id,"viewer":identity.viewer,"projectTitle":ctx.project.title,"workspaceRoot":ctx.project.workspace_root}),
        )
    }
    pub async fn routing_identity(
        &self,
        input: PullRequestRoutingIdentityInput,
    ) -> Result<PullRequestRoutingIdentityResult> {
        let _activity = self.admit()?;
        let bindings = self.current_projects().await?;
        let binding = bindings
            .iter()
            .find(|b| b.project.host.eq_ignore_ascii_case(input.host.as_str()))
            .ok_or_else(|| Error::unsupported("routeIdentity"))?;
        let identity = tokio::select! {_ = self.0.cancel.cancelled()=>return Err(Error::invalid("routeIdentity","Pull request service is shut down.")),result=binding.host.routing_identity(&binding.project)=>result?};
        crate::convert(
            json!({"host":binding.project.host,"provider":binding.host.kind(),"accountId":identity.account_id,"viewer":identity.viewer}),
        )
    }
    pub async fn invalidate(
        &self,
        input: PullRequestInvalidateInput,
        notify_readers: bool,
    ) -> Result<()> {
        let _activity = self.admit()?;
        let reference = input.reference.flatten();
        let scope = if let Some(reference) = &reference {
            Some(self.context(reference).await?)
        } else {
            None
        };
        if input.files_viewed_only == Some(Some(true)) {
            self.bump_scope(&format!(
                "viewed:{}",
                scope.as_ref().map_or_else(|| "all".into(), Context::scope)
            ));
            return Ok(());
        }
        if let Some(ctx) = scope {
            ctx.host.forget_reads(Some(&ctx)).await;
            self.0.persistent.invalidate(&ctx.scope()).await;
            self.bump_scope(&ctx.scope());
        } else {
            for binding in self.current_projects().await? {
                binding.host.forget_reads(None).await;
            }
            self.0.persistent.invalidate("all").await;
            self.0.epoch.fetch_add(1, Ordering::AcqRel);
        }
        if notify_readers {
            self.0.refreshes.send_modify(|v| *v = v.saturating_add(1));
        }
        Ok(())
    }
    pub async fn refresh_after_turn(&self, project_id: ProjectId, pushed: bool) -> Result<()> {
        let _activity = self.admit()?;
        for binding in self.current_projects().await? {
            binding.host.forget_reads(None).await;
        }
        self.0
            .persistent
            .invalidate(&format!("project:{}", project_id.0.as_str()))
            .await;
        self.bump_scope(&format!("project:{}", project_id.0.as_str()));
        if pushed {
            self.0
                .memory
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entries
                .retain(|k, _| !k.starts_with("list:"));
        }
        self.0.refreshes.send_modify(|v| *v = v.saturating_add(1));
        Ok(())
    }
    pub async fn report_state(&self, input: PullRequestReportStateInput) -> Result<()> {
        let ctx = self.context(&input.reference).await?;
        let changed = ctx.key()?;
        let now = self.0.clock.now_ms();
        let key = ctx.scope();
        let draft = input.is_draft.flatten();
        {
            let mut held = self.0.reports.lock().unwrap_or_else(|e| e.into_inner());
            let ReportedStates { entries, order } = &mut *held;
            entries.retain(|_, (at, ..)| now.saturating_sub(*at) < 300_000);
            if entries.get(&key).is_some_and(|(_, state, previous, _)| {
                *state == input.state && *previous == draft
            }) {
                return Ok(());
            }
            // TypeScript's `REF_EPOCH_CAPACITY`: the oldest report makes room for a new one.
            if entries.len() >= REPORTED_STATE_CAPACITY
                && !entries.contains_key(&key)
                && let Some(oldest) = entries
                    .iter()
                    .min_by_key(|(_, (.., inserted))| *inserted)
                    .map(|(scope, _)| scope.clone())
            {
                entries.remove(&oldest);
            }
            *order += 1;
            entries.insert(key, (now, input.state, draft, *order));
        }
        let _ = self.0.states.send(changed);
        Ok(())
    }
}
