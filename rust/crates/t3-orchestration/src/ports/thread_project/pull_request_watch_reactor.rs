use super::pull_request_watch::{
    PULL_REQUEST_WATCH_NO_CHECKS_MS, PullRequestWatchChange, PullRequestWatchWake,
    combine_pull_request_watch_wakes, evaluate_pull_request_watch, pull_request_watch_message,
};
use crate::{
    api::*,
    error::{Error, Result},
    projections::store::ProjectionStore,
    runtime::threads::{ThreadServiceScope, date_ms},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use t3_contracts::{pull_request::*, thread_pull_request::*};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::task::TaskTracker;

const QUIET_REREAD_MS: i64 = 10 * 60_000;
const FINGERPRINT_REREAD_MS: i64 = 30 * 60_000;

pub struct PullRequestWatchServices {
    pub projections: Arc<ProjectionStore>,
    pub commands: Arc<dyn CommandDispatcher>,
    pub pull_requests: Arc<dyn PullRequestBackend>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
}

#[derive(Clone)]
struct Target {
    thread: OrchestrationV2AppThread,
    link: ThreadPullRequestLink,
    watch: ThreadPullRequestWatch,
}
struct Group {
    key: String,
    targets: Vec<Target>,
    fingerprint: String,
}
#[derive(Clone)]
struct LastRead {
    at: i64,
    fingerprint: String,
    in_flight: bool,
    remarks_complete: bool,
    watches: BTreeSet<String>,
    host: Option<PullRequestWatchFingerprint>,
}
#[derive(Clone)]
struct Tail {
    count: u64,
    comments: Vec<PullRequestComment>,
}
#[derive(Clone)]
struct ThreadTails {
    fetched_at: i64,
    visible_through: i64,
    tails: BTreeMap<String, Tail>,
}
#[derive(Default)]
struct Cache {
    reads: BTreeMap<String, LastRead>,
    tails: BTreeMap<String, ThreadTails>,
}
struct HeldWake {
    key: String,
    target: Target,
    next: Option<ThreadPullRequestWatch>,
    ended: Option<ThreadPullRequestWatch>,
    wake: PullRequestWatchWake,
    remarks: bool,
}

pub struct PullRequestWatchReactor {
    services: PullRequestWatchServices,
    cache: Mutex<Cache>,
    passes: Mutex<()>,
    scope: ThreadServiceScope,
    tasks: TaskTracker,
    started: AtomicBool,
}

fn key(link: &ThreadPullRequestLink) -> String {
    format!(
        "{}/{}#{}",
        link.host.0.trim().to_lowercase(),
        link.repository.0.trim().to_lowercase(),
        link.number
    )
}
fn watch_key(target: &Target) -> String {
    format!("{} {}", json!(target.thread.id), target.watch.started_at.0)
}
fn snapshot_fingerprint(link: &ThreadPullRequestLink) -> Result<String> {
    let Some(snapshot) = &link.snapshot else {
        return Ok(String::new());
    };
    let snapshot = serde_json::to_value(snapshot)?;
    Ok([
        "state",
        "updatedAt",
        "checksState",
        "mergeability",
        "reviewDecision",
        "isDraft",
    ]
    .iter()
    .map(|field| match &snapshot[*field] {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        value => value.to_string(),
    })
    .collect::<Vec<_>>()
    .join(" "))
}
fn reference(target: &Target) -> Result<PullRequestRef> {
    Ok(serde_json::from_value(
        json!({"projectId":target.thread.project_id,"host":target.link.host,"repository":target.link.repository,"number":target.link.number,"allowStale":false}),
    )?)
}
fn plan(
    group: &Group,
    last: Option<&LastRead>,
    host: Option<&PullRequestWatchFingerprint>,
    now: i64,
) -> (bool, bool) {
    let Some(last) = last.filter(|last| {
        group
            .targets
            .iter()
            .all(|target| last.watches.contains(&watch_key(target)))
    }) else {
        return (true, true);
    };
    let checks_due = group.targets.iter().any(|target| {
        target.watch.checks_pending.is_none()
            || target
                .watch
                .checks_awaited_since
                .as_ref()
                .and_then(Option::as_ref)
                .and_then(|date| date_ms(&date.0))
                .is_some_and(|since| now - since >= PULL_REQUEST_WATCH_NO_CHECKS_MS)
    });
    match host {
        None => {
            let read = last.in_flight
                || !last.remarks_complete
                || last.fingerprint != group.fingerprint
                || now - last.at >= QUIET_REREAD_MS;
            (read || checks_due, read)
        }
        Some(host) => {
            let activity = last
                .host
                .as_ref()
                .is_none_or(|previous| previous.remarks != host.remarks)
                || !last.remarks_complete
                || now - last.at >= FINGERPRINT_REREAD_MS;
            (
                activity
                    || checks_due
                    || last
                        .host
                        .as_ref()
                        .is_none_or(|previous| previous.status != host.status)
                    || last.in_flight,
                activity,
            )
        }
    }
}
fn watches_equal(left: &ThreadPullRequestWatch, right: &ThreadPullRequestWatch) -> Result<bool> {
    let normalize = |watch: &ThreadPullRequestWatch| -> Result<Value> {
        let mut value = serde_json::to_value(watch)?;
        for field in ["owedChecks", "awaitingApproval"] {
            if value[field].is_null() {
                value[field] = json!([]);
            }
        }
        if value["approvalAnnounced"].is_null() {
            value["approvalAnnounced"] = json!(false);
        }
        Ok(value)
    };
    Ok(normalize(left)? == normalize(right)?)
}

impl PullRequestWatchReactor {
    pub fn new(services: PullRequestWatchServices) -> Self {
        Self {
            services,
            cache: Mutex::new(Cache::default()),
            passes: Mutex::new(()),
            scope: ThreadServiceScope::default(),
            tasks: TaskTracker::new(),
            started: AtomicBool::new(false),
        }
    }
    pub async fn start(self: &Arc<Self>) -> Result<()> {
        self.scope.run(async { self.start_inner() }).await
    }

    fn start_inner(self: &Arc<Self>) -> Result<()> {
        if self.started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let reactor = self.clone();
        self.tasks.spawn(async move {
            let cancellation = reactor.scope.cancellation();
            let mut cadence = tokio::time::interval(Duration::from_secs(120));
            cadence.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! { biased;
                    _ = cancellation.cancelled() => break,
                    _ = cadence.tick() => {
                        if let Err(error) = reactor.sweep().await
                            && !matches!(error, Error::Cancelled) {
                            eprintln!("pull request watch sweep failed: {error}");
                        }
                    },
                }
            }
        });
        Ok(())
    }
    pub async fn sweep(self: &Arc<Self>) -> Result<()> {
        self.scope.run(self.pass(None)).await
    }
    pub async fn read_on_change(self: &Arc<Self>, identity: ThreadPullRequestKey) -> Result<()> {
        let identity = format!(
            "{}/{}#{}",
            identity.host.0.trim().to_lowercase(),
            identity.repository.0.trim().to_lowercase(),
            identity.number
        );
        self.scope.run(self.pass(Some(identity))).await
    }
    pub async fn shutdown(&self) {
        self.scope.shutdown().await;
        self.tasks.close();
        self.tasks.wait().await;
    }

    async fn pass(self: &Arc<Self>, changed: Option<String>) -> Result<()> {
        // A pass snapshots and advances watches; a second pass must not reuse that snapshot.
        let _pass = self.passes.lock().await;
        let mut groups: BTreeMap<String, Vec<Target>> = BTreeMap::new();
        for thread in self
            .services
            .projections
            .threads_with_pull_requests(None)
            .await?
        {
            let links = thread.pull_requests.clone().flatten().unwrap_or_default();
            for link in links
                .into_iter()
                .filter(|link| link.source != ThreadPullRequestLinkSource::StackDismissed)
            {
                let Some(watch) = link.watch.clone().flatten() else {
                    continue;
                };
                if changed
                    .as_ref()
                    .is_some_and(|changed| changed != &key(&link))
                {
                    continue;
                }
                let target = Target {
                    thread: thread.clone(),
                    link,
                    watch,
                };
                let group_key = format!("{} {}", json!(thread.project_id), key(&target.link));
                let state = json!(thread.settled_override);
                if target
                    .link
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.state == PullRequestState::Merged)
                    || state == "settled"
                    || thread.settled_at.is_some()
                    || json!(thread.lineage)["relationshipToParent"] == "subagent"
                {
                    if let Err(error) = self.record(&target, None, None, None).await {
                        eprintln!("pull request watch stop failed: {error}");
                    }
                    continue;
                }
                groups.entry(group_key).or_default().push(target);
            }
        }
        if changed.is_none() {
            let mut cache = self.cache.lock().await;
            cache.reads.retain(|key, _| groups.contains_key(key));
            cache.tails.retain(|key, _| groups.contains_key(key));
        }
        let slots = Arc::new(Semaphore::new(4));
        let mut reads = Vec::new();
        for (key, targets) in groups {
            let fingerprint = targets
                .iter()
                .map(|target| snapshot_fingerprint(&target.link))
                .collect::<Result<BTreeSet<_>>>()?
                .into_iter()
                .collect::<Vec<_>>()
                .join("\n");
            let group = Group {
                key,
                targets,
                fingerprint,
            };
            let reactor = self.clone();
            let slots = slots.clone();
            reads.push(self.tasks.spawn(async move {
                reactor
                    .scope
                    .run(async {
                        let host = match reactor
                            .services
                            .pull_requests
                            .watch_fingerprint(reference(&group.targets[0])?)
                            .await
                        {
                            Ok(host) => host,
                            Err(Error::Unsupported(_)) => None,
                            Err(Error::Cancelled) => return Err(Error::Cancelled),
                            Err(error) => return Err(error),
                        };
                        let _permit = slots.acquire().await.map_err(|_| Error::Cancelled)?;
                        reactor.read_group(group, host).await
                    })
                    .await
            }));
        }
        let mut held = Vec::new();
        for read in reads {
            match read.await {
                Ok(Ok(wakes)) => held.extend(wakes),
                Ok(Err(Error::Cancelled)) => return Err(Error::Cancelled),
                Ok(Err(error)) => eprintln!("pull request watch check failed: {error}"),
                Err(error) => eprintln!("pull request watch task failed: {error}"),
            }
        }
        self.send_held(held).await
    }

    async fn read_group(
        &self,
        group: Group,
        host: Option<PullRequestWatchFingerprint>,
    ) -> Result<Vec<HeldWake>> {
        let now = self.services.clock.now();
        let now_ms = date_ms(&now.0).ok_or_else(|| Error::Decode {
            entity: "watch clock",
            detail: now.0.clone(),
        })?;
        let last = self.cache.lock().await.reads.get(&group.key).cloned();
        let (detail_due, activity_due) = plan(&group, last.as_ref(), host.as_ref(), now_ms);
        if !detail_due {
            return Ok(Vec::new());
        }
        if last.as_ref().is_none_or(|last| {
            group
                .targets
                .iter()
                .any(|target| !last.watches.contains(&watch_key(target)))
                || host.as_ref().is_some_and(|host| {
                    last.host
                        .as_ref()
                        .is_none_or(|previous| previous.remarks != host.remarks)
                })
        }) {
            self.cache.lock().await.tails.remove(&group.key);
        }
        let reference = reference(&group.targets[0])?;
        let (detail, activity) = tokio::join!(
            self.services.pull_requests.detail(reference.clone()),
            async {
                if activity_due {
                    self.services
                        .pull_requests
                        .activity(reference.clone())
                        .await
                        .map(Some)
                } else {
                    Ok(None)
                }
            }
        );
        let detail = match detail {
            Ok(detail) => detail,
            Err(error) => {
                self.cache.lock().await.reads.remove(&group.key);
                return Err(error);
            }
        };
        if detail.state != PullRequestState::Open {
            self.cache.lock().await.reads.remove(&group.key);
            let mut wakes = Vec::new();
            for target in group.targets {
                if detail.state == PullRequestState::Merged {
                    self.record(&target, None, None, None).await?;
                } else {
                    let number = target.link.number;
                    let wake = PullRequestWatchWake {
                        text: format!(
                            "Pull request #{number} ({}) was closed, so T3 Code stopped watching it. Call watch_pull_request if it reopens.",
                            target.link.url.0
                        ),
                        notification: serde_json::from_value(
                            json!({"source":{"kind":"monitor"},"outcome":"updated","summary":format!("#{number}: closed, stopped watching")}),
                        )?,
                    };
                    wakes.push(HeldWake {
                        key: group.key.clone(),
                        target,
                        next: None,
                        ended: None,
                        wake,
                        remarks: false,
                    });
                }
            }
            return Ok(wakes);
        }
        let activity = match activity {
            Ok(activity) => activity,
            Err(error) => {
                self.cache.lock().await.reads.remove(&group.key);
                return Err(error);
            }
        };
        let remarks = match activity {
            Some(activity) => {
                self.read_remarks(
                    &group.key,
                    reference,
                    activity,
                    now_ms,
                    if host.is_some() {
                        FINGERPRINT_REREAD_MS
                    } else {
                        QUIET_REREAD_MS
                    },
                )
                .await?
            }
            None => None,
        };
        let mut cache = self.cache.lock().await;
        let at = if activity_due || last.is_none() {
            cache
                .tails
                .get(&group.key)
                .map_or(now_ms, |tails| tails.fetched_at)
        } else {
            last.as_ref().map_or(now_ms, |last| last.at)
        };
        cache.reads.insert(
            group.key.clone(),
            LastRead {
                at,
                fingerprint: group.fingerprint,
                in_flight: detail.mergeability == PullRequestMergeability::Unknown
                    || detail
                        .checks
                        .iter()
                        .any(|check| check.status == PullRequestCheckStatus::Pending),
                remarks_complete: if activity_due {
                    remarks.is_some()
                } else {
                    last.as_ref().is_some_and(|last| last.remarks_complete)
                },
                watches: group.targets.iter().map(watch_key).collect(),
                host,
            },
        );
        drop(cache);
        let mut held = Vec::new();
        for target in group.targets {
            let report =
                evaluate_pull_request_watch(&target.watch, &detail, remarks.as_deref(), &now)?;
            if report.changes.is_empty() {
                if !watches_equal(&target.watch, &report.next)?
                    && let Err(error) = self.record(&target, Some(&report.next), None, None).await
                {
                    self.cache.lock().await.reads.remove(&group.key);
                    eprintln!("pull request watch update failed: {error}");
                }
            } else {
                let wake = pull_request_watch_message(
                    target.link.number,
                    &target.link.url.0,
                    &detail.base_branch.0,
                    report.next.head_sha.as_ref().map(|sha| sha.0.as_str()),
                    &report,
                )?;
                let remarks = report
                    .changes
                    .iter()
                    .any(|change| matches!(change, PullRequestWatchChange::Remarks { .. }));
                held.push(HeldWake {
                    key: group.key.clone(),
                    target,
                    next: (!report.exhausted).then(|| report.next.clone()),
                    ended: report.exhausted.then_some(report.next),
                    wake,
                    remarks,
                });
            }
        }
        Ok(held)
    }

    async fn read_remarks(
        &self,
        key: &str,
        reference: PullRequestRef,
        activity: PullRequestActivity,
        now: i64,
        reread: i64,
    ) -> Result<Option<Vec<PullRequestComment>>> {
        let result = self
            .read_remarks_inner(key, reference, activity, now, reread)
            .await;
        match result {
            Ok(Some(remarks)) => Ok(Some(remarks)),
            Err(Error::Cancelled) => Err(Error::Cancelled),
            other => {
                self.cache.lock().await.tails.remove(key);
                if let Err(error) = other {
                    eprintln!("pull request watch comment pagination failed: {error}");
                }
                Ok(None)
            }
        }
    }
    async fn read_remarks_inner(
        &self,
        key: &str,
        reference: PullRequestRef,
        activity: PullRequestActivity,
        now: i64,
        reread: i64,
    ) -> Result<Option<Vec<PullRequestComment>>> {
        if activity.comments_truncated && activity.review_threads_truncated.flatten() != Some(false)
        {
            return Ok(None);
        }
        let visible_through = activity
            .comments
            .iter()
            .filter_map(|comment| {
                date_ms(
                    &comment
                        .edited_at
                        .as_ref()
                        .and_then(Option::as_ref)
                        .unwrap_or(&comment.created_at)
                        .0,
                )
            })
            .max()
            .unwrap_or(0);
        let paginated: Vec<_> = activity
            .review_threads
            .iter()
            .filter(|thread| {
                thread
                    .next_comments_cursor
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_some()
            })
            .collect();
        let cached = self.cache.lock().await.tails.get(key).cloned();
        let reuse = cached.as_ref().filter(|cache| {
            now - cache.fetched_at < reread
                && cache.visible_through == visible_through
                && cache.tails.len() == paginated.len()
                && paginated.iter().all(|thread| {
                    cache.tails.get(&thread.id.0).is_some_and(|tail| {
                        tail.count == thread.comment_count.flatten().unwrap_or(0)
                    })
                })
        });
        let mut tails = BTreeMap::new();
        let mut remarks = activity.comments;
        for thread in paginated {
            let count = thread.comment_count.flatten().unwrap_or(0);
            let tail = match reuse
                .and_then(|cache| cache.tails.get(&thread.id.0))
                .cloned()
            {
                Some(tail) => tail,
                None => {
                    let mut comments = BTreeMap::new();
                    let mut cursors = BTreeSet::new();
                    let mut cursor = thread.next_comments_cursor.clone().flatten();
                    let mut hidden = 0;
                    while let Some(current) = cursor {
                        if !cursors.insert(current.0.clone()) {
                            return Ok(None);
                        }
                        let mut input = serde_json::to_value(&reference)?;
                        input["threadId"] = json!(thread.id);
                        input["cursor"] = json!(current);
                        let page = self
                            .services
                            .pull_requests
                            .thread_comments(serde_json::from_value(input)?)
                            .await?;
                        for comment in page.comments {
                            let id = comment.id.0.clone();
                            let mut comment = serde_json::to_value(comment)?;
                            comment["kind"] = json!("review-comment");
                            comment["path"] = json!(thread.path);
                            comment["reviewState"] = Value::Null;
                            comments
                                .insert(id, serde_json::from_value::<PullRequestComment>(comment)?);
                        }
                        hidden += page.hidden_count.flatten().unwrap_or(0);
                        cursor = page.next_cursor;
                    }
                    if thread.comments.len() as u64 + comments.len() as u64 + hidden < count {
                        return Ok(None);
                    }
                    Tail {
                        count,
                        comments: comments.into_values().collect(),
                    }
                }
            };
            remarks.extend(tail.comments.iter().cloned());
            tails.insert(thread.id.0.clone(), tail);
        }
        self.cache.lock().await.tails.insert(
            key.into(),
            ThreadTails {
                fetched_at: reuse.map_or(now, |cache| cache.fetched_at),
                visible_through,
                tails,
            },
        );
        remarks.sort_by(|left, right| left.created_at.0.cmp(&right.created_at.0));
        Ok(Some(remarks))
    }

    fn change(
        target: &Target,
        next: Option<&ThreadPullRequestWatch>,
        ended: Option<&ThreadPullRequestWatch>,
    ) -> Result<Value> {
        let mut change = json!({"host":target.link.host,"repository":target.link.repository,"number":target.link.number,"startedAt":target.watch.started_at,"watch":next});
        if let Some(pending) = serde_json::to_value(&target.watch)?.get("checksPending") {
            change["checksPendingBefore"] = pending.clone();
        }
        if let Some(ended) = ended {
            change["endedWatch"] = json!(ended);
        }
        Ok(change)
    }
    async fn record(
        &self,
        target: &Target,
        next: Option<&ThreadPullRequestWatch>,
        wake: Option<Value>,
        ended: Option<&ThreadPullRequestWatch>,
    ) -> Result<()> {
        let id = self.services.ids.next(IdKind::Command);
        let thread_id: ThreadId = serde_json::from_value(json!(target.thread.id))?;
        let mut command = Self::change(target, next, ended)?;
        command["type"] = json!("thread.pull-request-watch.sync");
        command["commandId"] = json!(format!("server:pr-watch:{}:{id}", thread_id.0));
        command["threadId"] = json!(target.thread.id);
        if let Some(mut wake) = wake {
            wake["messageId"] = json!(format!("message:pr-watch:{id}"));
            command["wake"] = wake;
        }
        self.services
            .commands
            .dispatch(serde_json::from_value(command)?)
            .await?;
        Ok(())
    }
    async fn send_held(&self, wakes: Vec<HeldWake>) -> Result<()> {
        let mut threads: BTreeMap<String, Vec<HeldWake>> = BTreeMap::new();
        for wake in wakes {
            threads
                .entry(json!(wake.target.thread.id).to_string())
                .or_default()
                .push(wake);
        }
        for wakes in threads.into_values() {
            if wakes.len() > 1 {
                match self.send_together(&wakes).await {
                    Ok(()) => continue,
                    Err(Error::Cancelled) => return Err(Error::Cancelled),
                    Err(_) => {}
                }
            }
            for item in wakes {
                let mut wake = serde_json::to_value(&item.wake)?;
                wake["includesRemarks"] = json!(item.remarks);
                if let Err(error) = self
                    .record(
                        &item.target,
                        item.next.as_ref(),
                        Some(wake),
                        item.ended.as_ref(),
                    )
                    .await
                {
                    if matches!(error, Error::Cancelled) {
                        return Err(error);
                    }
                    self.cache.lock().await.reads.remove(&item.key);
                    eprintln!("pull request watch update failed: {error}");
                }
            }
        }
        Ok(())
    }
    async fn send_together(&self, items: &[HeldWake]) -> Result<()> {
        let first = &items[0];
        let others = items[1..]
            .iter()
            .map(|item| {
                let mut change =
                    Self::change(&item.target, item.next.as_ref(), item.ended.as_ref())?;
                if item.remarks {
                    change["includesRemarks"] = json!(true);
                }
                Ok(change)
            })
            .collect::<Result<Vec<_>>>()?;
        let combined = combine_pull_request_watch_wakes(
            first.wake.clone(),
            &items[1..]
                .iter()
                .map(|item| item.wake.clone())
                .collect::<Vec<_>>(),
        )?;
        let id = self.services.ids.next(IdKind::Command);
        let thread_id: ThreadId = serde_json::from_value(json!(first.target.thread.id))?;
        let mut command = Self::change(&first.target, first.next.as_ref(), first.ended.as_ref())?;
        command["type"] = json!("thread.pull-request-watch.sync");
        command["commandId"] = json!(format!("server:pr-watch:{}:{id}", thread_id.0));
        command["threadId"] = json!(first.target.thread.id);
        command["others"] = json!(others);
        let mut wake = serde_json::to_value(combined)?;
        wake["messageId"] = json!(format!("message:pr-watch:{id}"));
        if first.remarks {
            wake["includesRemarks"] = json!(true);
        }
        wake["sections"] = json!(items.iter().map(|item| json!({"text":item.wake.text,"outcome":item.wake.notification.outcome,"summary":item.wake.notification.summary})).collect::<Vec<_>>());
        command["wake"] = wake;
        self.services
            .commands
            .dispatch(serde_json::from_value(command)?)
            .await?;
        Ok(())
    }
}
