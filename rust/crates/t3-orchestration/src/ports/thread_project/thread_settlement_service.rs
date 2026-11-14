use crate::{error::Result, runtime::threads::date_ms};
use serde::{Deserialize, Serialize};
use t3_contracts::{
    base_schemas::IsoDateTime,
    orchestration_v2::*,
    pull_request::PullRequestState,
    thread_pull_request::{ThreadPullRequestLink, ThreadPullRequestLinkSource},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettlementPullRequest {
    pub state: PullRequestState,
    pub closed_at: Option<String>,
    pub merged_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettlementCandidate {
    #[serde(flatten)]
    pub thread: OrchestrationV2ThreadShell,
    pub last_snooze_wake_at: Option<IsoDateTime>,
}

fn millis(date: Option<&IsoDateTime>) -> Option<i64> {
    date.and_then(|date| date_ms(&date.0))
}
fn value<T>(value: &Option<Option<T>>) -> Option<&T> {
    value.as_ref().and_then(Option::as_ref)
}

pub fn thread_has_queued_turn_start(thread: &OrchestrationV2ThreadShell, now_ms: i64) -> bool {
    let Some(message) = millis(thread.latest_user_message_at.as_ref()) else {
        return false;
    };
    if thread.status == OrchestrationV2ShellThreadStatus::Failed
        || (now_ms - message).unsigned_abs() > 120000
    {
        return false;
    }
    thread.latest_run_id.is_none()
        || [
            value(&thread.latest_run_requested_at),
            value(&thread.latest_run_started_at),
            value(&thread.latest_run_completed_at),
        ]
        .into_iter()
        .all(|date| millis(date).is_none_or(|date| date < message))
}

pub fn is_snoozed(thread: &OrchestrationV2ThreadShell, now_ms: i64) -> bool {
    let Some(until) = millis(value(&thread.snoozed_until)) else {
        return false;
    };
    if until <= now_ms || thread.pending_guest_message_count.flatten().unwrap_or(0) > 0 {
        return false;
    }
    let snoozed_at = millis(value(&thread.snoozed_at));
    if let Some(request) = &thread.pending_runtime_request
        && (!matches!(&request.kind, RuntimeRequestKind::Member2(kind) if **kind == RuntimeRequestKindMember2::UserInput)
            || snoozed_at.is_none()
            || date_ms(&request.created_at.0) > snoozed_at)
    {
        return false;
    }
    !(thread.status == OrchestrationV2ShellThreadStatus::Failed
        && (snoozed_at.is_none()
            || millis(value(&thread.latest_run_completed_at))
                .zip(snoozed_at)
                .is_some_and(|(completed, snoozed)| completed > snoozed)))
}

pub fn is_auto_settlement_candidate(thread: &OrchestrationV2ThreadShell, now_ms: i64) -> bool {
    if thread.archived_at.is_some()
        || thread.settled_override.is_some()
        || value(&thread.pinned_at).is_some()
        || value(&thread.auto_settle_disabled_at).is_some()
        || thread.pending_runtime_request.is_some()
        || thread.pending_guest_message_count.flatten().unwrap_or(0) > 0
        || value(&thread.activity_run_status).is_some()
    {
        return false;
    }
    if let Some(tasks) = value(&thread.pending_background_tasks) {
        for task in tasks {
            match task {
                OrchestrationV2PendingBackgroundTask::Subagent(_)
                | OrchestrationV2PendingBackgroundTask::BackgroundTask(_) => return false,
                OrchestrationV2PendingBackgroundTask::Command(task)
                | OrchestrationV2PendingBackgroundTask::Monitor(task) => {
                    if value(&task.awaited).is_some()
                        || task.task_id.0.starts_with("pull-request-watch:")
                    {
                        return false;
                    }
                }
            }
        }
    }
    !thread_has_queued_turn_start(thread, now_ms) && !is_snoozed(thread, now_ms)
}

fn terminal_at(link: &ThreadPullRequestLink) -> Option<i64> {
    let snapshot = link.snapshot.as_ref()?;
    let date = if snapshot.state == PullRequestState::Merged {
        snapshot.merged_at.as_ref()
    } else {
        snapshot.closed_at.as_ref()
    };
    date.and_then(Option::as_deref).and_then(date_ms)
}
fn latest<'a>(dates: impl IntoIterator<Item = Option<&'a IsoDateTime>>) -> Option<&'a IsoDateTime> {
    dates
        .into_iter()
        .flatten()
        .filter_map(|date| date_ms(&date.0).map(|ms| (date, ms)))
        .max_by_key(|(_, ms)| *ms)
        .map(|(date, _)| date)
}

pub fn resolve_auto_settlement_at(
    candidate: &SettlementCandidate,
    pull_request: Option<&SettlementPullRequest>,
    now_ms: i64,
    auto_settle_after_days: Option<f64>,
    auto_settle_on_merge: bool,
    keep_pull_requests: bool,
) -> Option<IsoDateTime> {
    let thread = &candidate.thread;
    let links: Vec<_> = value(&thread.pull_requests)
        .into_iter()
        .flatten()
        .filter(|link| {
            link.source != ThreadPullRequestLinkSource::StackDismissed
                && (link
                    .snapshot
                    .as_ref()
                    .is_none_or(|snapshot| snapshot.state == PullRequestState::Open)
                    || date_ms(&link.linked_at.0)
                        .is_none_or(|linked| terminal_at(link).is_some_and(|at| at >= linked)))
        })
        .collect();
    if links.iter().any(|link| link.snapshot.is_none()) {
        return None;
    }
    let has_open_link = links.iter().any(|link| {
        link.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.state == PullRequestState::Open)
    });
    let from_link = links
        .iter()
        .reduce(|current, candidate| {
            if terminal_at(candidate) > terminal_at(current) {
                candidate
            } else {
                current
            }
        })
        .and_then(|link| link.snapshot.as_ref())
        .map(|snapshot| SettlementPullRequest {
            state: snapshot.state.clone(),
            closed_at: snapshot.closed_at.clone().flatten(),
            merged_at: snapshot.merged_at.clone().flatten(),
        });
    let pull_request = if has_open_link {
        None
    } else {
        from_link.as_ref().or(pull_request)
    };
    if !is_auto_settlement_candidate(thread, now_ms) {
        return None;
    }
    let activity = latest([
        thread.latest_user_message_at.as_ref(),
        value(&thread.latest_run_requested_at),
        value(&thread.latest_run_started_at),
        value(&thread.latest_run_completed_at),
    ])
    .unwrap_or(&thread.created_at);
    if let Some(pr) = pull_request {
        let terminal = match pr.state {
            PullRequestState::Closed => pr.closed_at.as_deref(),
            PullRequestState::Merged if auto_settle_on_merge => pr.merged_at.as_deref(),
            _ => None,
        }
        .and_then(date_ms);
        let anchor = latest([
            Some(&thread.created_at),
            value(&thread.latest_user_authored_message_at),
        ])
        .and_then(|date| date_ms(&date.0));
        if terminal
            .zip(anchor)
            .is_some_and(|(terminal, anchor)| terminal >= anchor)
        {
            return Some(activity.clone());
        }
    }
    let has_pull_request = has_open_link
        || pull_request.is_some()
        || value(&thread.linked_pull_request).is_some()
        || value(&thread.branch_pull_request).is_some();
    if keep_pull_requests && has_pull_request {
        return None;
    }
    let days = auto_settle_after_days?;
    let timer_wake = value(&thread.snoozed_until)
        .filter(|date| millis(Some(date)).is_some_and(|at| at <= now_ms));
    let inactivity = latest([
        Some(activity),
        candidate.last_snooze_wake_at.as_ref(),
        timer_wake,
    ])
    .unwrap_or(activity);
    ((date_ms(&inactivity.0)? as f64) < now_ms as f64 - days * 86400000.0)
        .then(|| inactivity.clone())
}

pub fn auto_settlement_settings_key(
    settings: &t3_contracts::settings::ServerSettings,
) -> Result<String> {
    let settings = serde_json::to_value(settings)?;
    let mut projects: Vec<_> = settings["projectSettingsOverrides"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, entry)| {
            [
                "sidebarAutoSettleOnMerge",
                "sidebarAutoSettleAfterDays",
                "sidebarAutoSettleKeepPullRequests",
            ]
            .iter()
            .any(|key| entry.get(key).is_some())
        })
        .collect();
    projects.sort_by_key(|(id, _)| *id);
    let projects: Vec<_> = projects
        .into_iter()
        .map(|(id, entry)| {
            serde_json::json!([
                id,
                entry
                    .get("sidebarAutoSettleOnMerge")
                    .filter(|value| !value.is_null())
                    .cloned()
                    .unwrap_or("inherit".into()),
                entry
                    .get("sidebarAutoSettleAfterDays")
                    .cloned()
                    .unwrap_or("inherit".into()),
                entry
                    .get("sidebarAutoSettleKeepPullRequests")
                    .filter(|value| !value.is_null())
                    .cloned()
                    .unwrap_or("inherit".into())
            ])
        })
        .collect();
    Ok(serde_json::to_string(&serde_json::json!([
        settings["sidebarAutoSettleOnMerge"],
        settings["sidebarAutoSettleAfterDays"],
        settings["sidebarAutoSettleKeepPullRequests"],
        projects
    ]))?)
}

use super::thread_pull_request_service::pull_request_cwd;
use crate::{
    api::{
        BranchPullRequestQuery, GitBranchPullRequest, PullRequestBackend, ResourceCleanup,
        SettleScriptInput, SettleScriptRun, SettleScriptRunner, WorkspaceBackend,
    },
    error::Error,
    projections::write::PullRequestIdentityRegistry,
    stores::projects,
};
use crate::{
    api::{
        Clock, CommandDispatcher, EventCommitter, IdGenerator, IdKind, ProjectionReader,
        SettingsService, ThreadId,
    },
    projections::store::ProjectionStore,
    runtime::threads::{ThreadProjectWorker, ThreadServiceScope, join_thread_reads},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use t3_contracts::{
    orchestration_project::OrchestrationProjectShell, pull_request::PullRequestRef,
};
use tokio::sync::Mutex;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub struct ThreadSettlementServices {
    pub projections: Arc<ProjectionStore>,
    pub dispatcher: Arc<dyn CommandDispatcher>,
    pub committer: Arc<dyn EventCommitter>,
    pub settings: Arc<dyn SettingsService>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
}

pub type CloseCommandMatcher = dyn Fn(&str) -> bool + Send + Sync;

pub struct ThreadSettlementBackends {
    pub workspaces: Arc<dyn WorkspaceBackend>,
    pub pull_requests: Arc<dyn PullRequestBackend>,
    pub identities: Arc<dyn PullRequestIdentityRegistry>,
    pub resources: Arc<dyn ResourceCleanup>,
    pub scripts: Arc<dyn SettleScriptRunner>,
    pub closes_pull_request: Arc<CloseCommandMatcher>,
}

struct LookupState {
    ports: ThreadSettlementBackends,
    shell_closed: Mutex<BTreeSet<ThreadId>>,
    cleaned: Mutex<BTreeMap<ThreadId, i64>>,
    rechecks: Mutex<BTreeMap<String, (i64, bool)>>,
    tasks: TaskTracker,
    cancellation: CancellationToken,
}

pub struct ThreadSettlementService {
    services: Arc<ThreadSettlementServices>,
    lookups: Option<Arc<LookupState>>,
    full_sweeps: ThreadProjectWorker<(), Option<ThreadId>>,
    thread_sweeps: ThreadProjectWorker<ThreadId, Option<ThreadId>>,
    tasks: TaskTracker,
    scope: ThreadServiceScope,
    cancellation: CancellationToken,
    started: Mutex<bool>,
}

impl ThreadSettlementService {
    pub fn new(services: ThreadSettlementServices) -> Self {
        Self::build(services, None)
    }

    pub fn with_backends(
        services: ThreadSettlementServices,
        backends: ThreadSettlementBackends,
    ) -> Self {
        Self::build(services, Some(backends))
    }

    fn build(
        services: ThreadSettlementServices,
        backends: Option<ThreadSettlementBackends>,
    ) -> Self {
        let lookups = backends.map(|ports| {
            Arc::new(LookupState {
                ports,
                shell_closed: Mutex::new(BTreeSet::new()),
                cleaned: Mutex::new(BTreeMap::new()),
                rechecks: Mutex::new(BTreeMap::new()),
                tasks: TaskTracker::new(),
                cancellation: CancellationToken::new(),
            })
        });
        let services = Arc::new(services);
        let make_worker = |concurrency| {
            let services = services.clone();
            let lookups = lookups.clone();
            ThreadProjectWorker::new(
                concurrency,
                |_: &mut Option<ThreadId>, _| {},
                move |thread, _| {
                    let services = services.clone();
                    let lookups = lookups.clone();
                    async move {
                        if let Err(error) =
                            sweep(&services, lookups.as_deref(), thread, false).await
                        {
                            eprintln!("automatic thread settlement sweep failed: {error}");
                        }
                    }
                },
            )
        };
        // A targeted sweep must not queue behind a full sweep's external reads.
        let full_sweeps = make_worker(1);
        let thread_services = services.clone();
        let thread_lookups = lookups.clone();
        let thread_sweeps = ThreadProjectWorker::new(
            8,
            |_: &mut Option<ThreadId>, _| {},
            move |thread, _| {
                let services = thread_services.clone();
                let lookups = thread_lookups.clone();
                async move {
                    if let Err(error) = sweep(&services, lookups.as_deref(), thread, false).await {
                        eprintln!("automatic thread settlement sweep failed: {error}");
                    }
                }
            },
        );
        let scope = ThreadServiceScope::default();
        let cancellation = scope.cancellation();
        Self {
            services,
            lookups,
            full_sweeps,
            thread_sweeps,
            tasks: TaskTracker::new(),
            scope,
            cancellation,
            started: Mutex::new(false),
        }
    }

    pub async fn sweep(&self, thread_id: Option<ThreadId>) -> Result<()> {
        self.scope
            .run(sweep(
                &self.services,
                self.lookups.as_deref(),
                thread_id,
                false,
            ))
            .await
    }

    pub async fn enqueue(&self, thread_id: Option<ThreadId>) -> Result<()> {
        self.scope
            .run(async {
                match thread_id {
                    Some(id) => self.thread_sweeps.enqueue(id.clone(), Some(id)).await,
                    None => self.full_sweeps.enqueue((), None).await,
                }
            })
            .await
    }

    pub async fn drain(&self) {
        // A running full sweep can cause targeted work and vice versa through committed events.
        self.full_sweeps.drain().await;
        self.thread_sweeps.drain().await;
    }

    pub async fn start(self: &Arc<Self>) -> Result<()> {
        self.scope.run(self.start_inner()).await
    }

    async fn start_inner(self: &Arc<Self>) -> Result<()> {
        let mut started = self.started.lock().await;
        if *started {
            return Ok(());
        }
        let merges = match &self.lookups {
            Some(lookups) => Some(lookups.ports.pull_requests.subscribe_merges().await?),
            None => None,
        };
        let mut settings = self.services.settings.subscribe()?;
        let mut events = self.services.committer.subscribe();
        let mut settings_key = auto_settlement_settings_key(&self.services.settings.get().await?)?;
        self.enqueue(None).await?;
        *started = true;
        if let Some(mut merges) = merges {
            let service = self.clone();
            self.tasks.spawn(async move {
                loop {
                    tokio::select! { biased;
                        _ = service.cancellation.cancelled() => break,
                        event = merges.recv() => match event {
                            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                if let Err(error) = service.scope.run(sweep(&service.services, service.lookups.as_deref(), None, true)).await {
                                    if matches!(error, Error::Cancelled) { break; }
                                    eprintln!("merge settlement sweep failed: {error}");
                                }
                            },
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            });
        }
        let service = self.clone();
        self.tasks.spawn(async move {
            let mut cadence = tokio::time::interval_at(tokio::time::Instant::now() + Duration::from_secs(60), Duration::from_secs(60));
            cadence.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                let result = tokio::select! {
                    _ = service.cancellation.cancelled() => break,
                    _ = cadence.tick() => service.enqueue(None).await,
                    result = settings.changed() => {
                        if result.is_err() { break; }
                        let current = settings.borrow_and_update().clone();
                        match auto_settlement_settings_key(&current) {
                            Ok(key) if key != settings_key => { settings_key = key; service.enqueue(None).await },
                            Ok(_) => Ok(()),
                            Err(error) => Err(error),
                        }
                    },
                    result = events.recv() => match result {
                        Ok(t3_contracts::application_event::ApplicationStoredEvent::Orchestration(stored)) => service.process_event(&stored.event).await,
                        Ok(_) => Ok(()),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => service.enqueue(None).await,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    },
                };
                if let Err(error) = result { eprintln!("automatic thread settlement event failed: {error}"); }
            }
        });
        Ok(())
    }

    pub async fn process_event(&self, event: &OrchestrationV2DomainEvent) -> Result<()> {
        self.scope.run(self.process_event_inner(event)).await
    }

    async fn process_event_inner(&self, event: &OrchestrationV2DomainEvent) -> Result<()> {
        let event = serde_json::to_value(event)?;
        if let Some(lookups) = &self.lookups {
            let id: ThreadId = serde_json::from_value(event["threadId"].clone())?;
            if event["type"] == "thread.settled" {
                if let Err(error) = lookups.cleanup(&self.services, id).await {
                    if matches!(error, Error::Cancelled) {
                        return Err(error);
                    }
                    eprintln!("settled thread cleanup failed: {error}");
                }
            } else if event["type"] == "turn-item.updated"
                && event["payload"]["type"] == "command_execution"
                && !matches!(
                    event["payload"]["status"].as_str(),
                    Some("pending" | "running" | "waiting")
                )
                && (lookups.ports.closes_pull_request)(
                    event["payload"]["input"].as_str().unwrap_or_default(),
                )
            {
                lookups.shell_closed.lock().await.insert(id);
            }
        }
        let kind = event["type"].as_str().unwrap_or_default();
        let payload = &event["payload"];
        let terminal = !matches!(
            payload["status"].as_str(),
            Some("pending" | "running" | "waiting")
        );
        let full = matches!(
            kind,
            "thread.settled" | "thread.archived" | "thread.deleted"
        );
        let targeted = match kind {
            "thread.pull-request-link-updated"
            | "thread.pull-request-synced"
            | "provider-session.detached"
            | "run.background-work-cancelled" => true,
            "subagent.updated" => terminal,
            "provider-thread.updated" => !payload["pendingBackgroundTasks"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|task| {
                    task["awaited"] == true
                        || matches!(task["kind"].as_str(), Some("subagent" | "background_task"))
                }),
            "turn-item.updated" => {
                terminal
                    && matches!(
                        payload["type"].as_str(),
                        Some("command_execution" | "dynamic_tool" | "subagent")
                    )
            }
            "provider-session.updated" => {
                !matches!(payload["status"].as_str(), Some("starting" | "running"))
            }
            "run.updated" => matches!(
                payload["status"].as_str(),
                Some("completed" | "interrupted" | "failed" | "cancelled" | "rolled_back")
            ),
            _ => false,
        };
        if full {
            self.enqueue(None).await?;
        } else if targeted {
            self.enqueue(Some(serde_json::from_value(event["threadId"].clone())?))
                .await?;
        }
        Ok(())
    }

    pub async fn shutdown(&self) {
        if let Some(lookups) = &self.lookups {
            lookups.cancellation.cancel();
        }
        self.scope.shutdown().await;
        self.tasks.close();
        self.tasks.wait().await;
        tokio::join!(self.full_sweeps.shutdown(), self.thread_sweeps.shutdown());
        if let Some(lookups) = &self.lookups {
            lookups.tasks.close();
            lookups.tasks.wait().await;
        }
    }
}

fn settlement_settings(
    settings: &t3_contracts::settings::ServerSettings,
    project: &str,
) -> Result<(Option<f64>, bool, bool)> {
    let settings = serde_json::to_value(settings)?;
    let overrides = &settings["projectSettingsOverrides"][project];
    let get = |key: &str| overrides.get(key).unwrap_or(&settings[key]);
    Ok((
        get("sidebarAutoSettleAfterDays").as_f64(),
        get("sidebarAutoSettleOnMerge").as_bool().unwrap_or(false),
        get("sidebarAutoSettleKeepPullRequests")
            .as_bool()
            .unwrap_or(false),
    ))
}

fn decision(
    candidate: &SettlementCandidate,
    pr: Option<&SettlementPullRequest>,
    settings: &t3_contracts::settings::ServerSettings,
    now: i64,
) -> Result<Option<IsoDateTime>> {
    let (days, merge, keep) = settlement_settings(settings, &candidate.thread.project_id.0)?;
    Ok(resolve_auto_settlement_at(
        candidate, pr, now, days, merge, keep,
    ))
}

async fn settle(
    services: &ThreadSettlementServices,
    candidate: &SettlementCandidate,
    pr: Option<&SettlementPullRequest>,
) -> Result<bool> {
    let subagent =
        serde_json::to_value(&candidate.thread.lineage)?["relationshipToParent"] == "subagent";
    let settings = services.settings.get().await?;
    let Some(now) = date_ms(&services.clock.now().0) else {
        return Ok(false);
    };
    let settled_at = decision(candidate, pr, &settings, now)?;
    if !subagent && settled_at.is_none() {
        return Ok(false);
    }
    let id = &candidate.thread.id;
    let mut command = serde_json::json!({"type":"thread.auto-settle","commandId":format!("server:auto-settle:{}:{}",id.0,services.ids.next(IdKind::Command)),"threadId":id,"snapshotAt":candidate.thread.updated_at});
    if !subagent {
        command["settledAt"] = serde_json::to_value(settled_at)?;
    }
    services
        .dispatcher
        .dispatch(serde_json::from_value(command)?)
        .await?;
    Ok(true)
}

async fn sweep(
    services: &ThreadSettlementServices,
    lookups: Option<&LookupState>,
    thread_id: Option<ThreadId>,
    merged: bool,
) -> Result<()> {
    let settings = services.settings.get().await?;
    let settings_value = serde_json::to_value(&settings)?;
    let configured = settings_value["sidebarAutoSettleOnMerge"] == true
        || settings_value["sidebarAutoSettleAfterDays"]
            .as_f64()
            .is_some()
        || settings_value["projectSettingsOverrides"]
            .as_object()
            .into_iter()
            .flatten()
            .any(|(_, value)| {
                value["sidebarAutoSettleOnMerge"] == true
                    || value["sidebarAutoSettleAfterDays"].as_f64().is_some()
            });
    let targeted = thread_id.is_some();
    let mut pending = Vec::new();
    for thread in services
        .projections
        .settlement_candidates(thread_id)
        .await?
    {
        let subagent = serde_json::to_value(&thread.lineage)?["relationshipToParent"] == "subagent";
        let Some(now) = date_ms(&services.clock.now().0) else {
            continue;
        };
        if (!configured && !subagent) || !is_auto_settlement_candidate(&thread, now) {
            continue;
        }
        let Some(app_thread) = services
            .projections
            .thread(ThreadId(thread.id.0.clone()))
            .await?
        else {
            continue;
        };
        let candidate = SettlementCandidate {
            thread,
            last_snooze_wake_at: app_thread.last_snooze_wake_at.flatten(),
        };
        if subagent || decision(&candidate, None, &settings, now)?.is_some() {
            match settle(services, &candidate, None).await {
                Ok(true) => continue,
                Ok(false) => {}
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(error) => {
                    eprintln!("automatic thread settlement skipped: {error}");
                    continue;
                }
            }
        }
        let thread = &candidate.thread;
        if value(&thread.pull_requests)
            .into_iter()
            .flatten()
            .any(|link| link.source != ThreadPullRequestLinkSource::StackDismissed)
            || (thread.branch.is_none()
                && value(&thread.linked_pull_request).is_none()
                && value(&thread.branch_pull_request).is_none())
        {
            continue;
        }
        pending.push(candidate);
    }
    if let Some(lookups) = lookups {
        lookups
            .sweep(services, pending, targeted || merged, merged)
            .await?;
    }
    Ok(())
}

struct LookupGroup {
    candidates: Vec<SettlementCandidate>,
    cwd: Option<PathBuf>,
}

fn branch_result(result: &GitBranchPullRequest) -> Result<SettlementPullRequest> {
    Ok(SettlementPullRequest {
        state: serde_json::from_value(serde_json::json!(result.pull_request.state))?,
        closed_at: result.closed_at.as_ref().map(|date| date.0.clone()),
        merged_at: result.merged_at.as_ref().map(|date| date.0.clone()),
    })
}

impl LookupState {
    async fn would_settle(
        &self,
        services: &ThreadSettlementServices,
        group: &LookupGroup,
        pr: &SettlementPullRequest,
    ) -> Result<bool> {
        let settings = services.settings.get().await?;
        let Some(now) = date_ms(&services.clock.now().0) else {
            return Ok(false);
        };
        for candidate in &group.candidates {
            if decision(candidate, Some(pr), &settings, now)?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn matches_project(
        &self,
        request: &GitBranchPullRequest,
        project: &OrchestrationProjectShell,
    ) -> Result<bool> {
        let Some(key) = &request.repository_key else {
            return Ok(false);
        };
        let Some(identity) = self
            .ports
            .workspaces
            .repository_identity_with_refresh(project.workspace_root.0.clone().into(), false)
            .await?
        else {
            return Ok(false);
        };
        Ok(self.ports.identities.canonical_repository_key(key)
            == self
                .ports
                .identities
                .canonical_repository_key(&identity.canonical_key.0))
    }

    async fn lookup(
        &self,
        services: &ThreadSettlementServices,
        group: &LookupGroup,
        projects: &BTreeMap<String, OrchestrationProjectShell>,
        key: &str,
        scheduled: bool,
    ) -> Result<Option<Option<SettlementPullRequest>>> {
        let thread = &group.candidates[0].thread;
        if let Some(reference) =
            value(&thread.linked_pull_request).or(value(&thread.branch_pull_request))
        {
            if !projects.contains_key(&reference.project_id.0) {
                return Err(Error::Unsupported("linked pull request project not found"));
            }
            let reference: PullRequestRef = serde_json::from_value(
                serde_json::json!({"projectId":reference.project_id,"repository":reference.repository,"number":reference.number}),
            )?;
            let summary = self.ports.pull_requests.summary(reference).await?;
            let terminal = SettlementPullRequest {
                state: summary.state.clone(),
                closed_at: summary.closed_at.flatten(),
                merged_at: summary.merged_at.flatten(),
            };
            if summary.state != PullRequestState::Open
                && let (Some(branch), Some(cwd)) = (&thread.branch, &group.cwd)
            {
                if !self.would_settle(services, group, &terminal).await? {
                    return Ok(None);
                }
                let current = self
                    .ports
                    .workspaces
                    .branch_pull_request(BranchPullRequestQuery {
                        cwd: cwd.clone(),
                        branch: branch.0.clone(),
                        refresh: true,
                        confirm_terminal: true,
                        fresh_git_state: false,
                    })
                    .await?;
                if let Some(current) = current
                    && branch_result(&current)?.state == PullRequestState::Open
                    && let Some(project) = projects.get(&thread.project_id.0)
                    && self.matches_project(&current, project).await?
                {
                    return Ok(Some(Some(branch_result(&current)?)));
                }
            }
            return Ok(Some(Some(terminal)));
        }
        let Some(branch) = &thread.branch else {
            return Ok(Some(None));
        };
        let Some(cwd) = &group.cwd else {
            return Err(Error::Unsupported("thread project not found"));
        };
        let mut closed = self.shell_closed.lock().await;
        let mut refresh = false;
        for candidate in &group.candidates {
            refresh |= closed.remove(&ThreadId(candidate.thread.id.0.clone()));
        }
        drop(closed);
        if !refresh && !scheduled {
            return Ok(None);
        }
        let query = BranchPullRequestQuery {
            cwd: cwd.clone(),
            branch: branch.0.clone(),
            refresh,
            confirm_terminal: false,
            fresh_git_state: false,
        };
        let remembered = self
            .ports
            .workspaces
            .branch_pull_request(query.clone())
            .await?;
        if let Some(now) = date_ms(&services.clock.now().0) {
            self.rechecks
                .lock()
                .await
                .insert(key.into(), (now, remembered.is_some()));
        }
        let Some(remembered) = remembered else {
            return Ok(Some(None));
        };
        let terminal = branch_result(&remembered)?;
        if refresh || terminal.state == PullRequestState::Open {
            return Ok(Some(Some(terminal)));
        }
        if !self.would_settle(services, group, &terminal).await? {
            return Ok(None);
        }
        let fresh = self
            .ports
            .workspaces
            .branch_pull_request(BranchPullRequestQuery {
                fresh_git_state: true,
                confirm_terminal: true,
                ..query
            })
            .await?;
        Ok(Some(fresh.as_ref().map(branch_result).transpose()?))
    }

    async fn sweep(
        &self,
        services: &ThreadSettlementServices,
        candidates: Vec<SettlementCandidate>,
        immediate: bool,
        merged: bool,
    ) -> Result<()> {
        let projects: BTreeMap<_, _> = services
            .projections
            .database
            .read(|db| projects::list_shells(db, None))
            .await?
            .into_iter()
            .map(|project| (project.id.0.clone(), project))
            .collect();
        let mut groups: BTreeMap<String, LookupGroup> = BTreeMap::new();
        for candidate in candidates {
            let thread = &candidate.thread;
            let cwd = match (projects.get(&thread.project_id.0), &thread.branch) {
                (Some(project), Some(_)) => Some(
                    pull_request_cwd(
                        thread.worktree_path.as_ref().map(|path| path.0.as_str()),
                        &project.workspace_root.0,
                    )
                    .await?,
                ),
                _ => None,
            };
            let key = if let Some(reference) =
                value(&thread.linked_pull_request).or(value(&thread.branch_pull_request))
            {
                serde_json::json!([
                    "linked",
                    reference.project_id,
                    reference.repository,
                    reference.number,
                    cwd,
                    thread.branch
                ])
                .to_string()
            } else {
                serde_json::json!(["branch", cwd, thread.branch]).to_string()
            };
            groups
                .entry(key)
                .or_insert_with(|| LookupGroup {
                    candidates: Vec::new(),
                    cwd,
                })
                .candidates
                .push(candidate);
        }
        if merged {
            let cwds: BTreeSet<_> = groups
                .values()
                .filter_map(|group| group.cwd.clone())
                .collect();
            for cwd in cwds {
                self.ports.workspaces.invalidate_status(cwd).await?;
            }
        }
        let now = date_ms(&services.clock.now().0).unwrap_or_default();
        let mut scheduled = BTreeSet::new();
        if !immediate {
            let mut reads = self.rechecks.lock().await;
            let mut due = Vec::new();
            for (key, group) in &groups {
                if !key.starts_with("[\"branch\",") {
                    continue;
                }
                let activity = group
                    .candidates
                    .iter()
                    .map(|candidate| {
                        let thread = &candidate.thread;
                        latest([
                            Some(&thread.created_at),
                            thread.latest_user_message_at.as_ref(),
                            value(&thread.latest_run_requested_at),
                            value(&thread.latest_run_started_at),
                            value(&thread.latest_run_completed_at),
                        ])
                        .and_then(|date| date_ms(&date.0))
                        .unwrap_or_default()
                    })
                    .max()
                    .unwrap_or_default();
                if let Some((at, found)) = reads.get(key) {
                    if *found {
                        scheduled.insert(key.clone());
                        continue;
                    }
                    let delay = ((now - activity) / 12).clamp(1_800_000, 86_400_000);
                    if now - *at < delay {
                        continue;
                    }
                }
                due.push((
                    reads.get(key).map_or(i64::MIN, |(at, _)| *at),
                    std::cmp::Reverse(activity),
                    key.clone(),
                ));
            }
            due.sort();
            for (_, _, key) in due.into_iter().take(4) {
                reads.insert(key.clone(), (now, false));
                scheduled.insert(key);
            }
            reads.retain(|key, _| groups.contains_key(key));
        }
        // Scoped futures borrow the snapshot; dropping the sweep cancels every group read.
        let slots = tokio::sync::Semaphore::new(8);
        let mut reads: Vec<
            std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>>,
        > = Vec::new();
        for (key, group) in &groups {
            reads.push(Box::pin(async {
                let _permit = slots.acquire().await.map_err(|_| Error::Cancelled)?;
                let result = self
                    .lookup(
                        services,
                        group,
                        &projects,
                        key,
                        immediate || scheduled.contains(key),
                    )
                    .await;
                match result {
                    Ok(Some(pr)) => {
                        for candidate in &group.candidates {
                            if let Err(error) = settle(services, candidate, pr.as_ref()).await {
                                if matches!(error, Error::Cancelled) {
                                    return Err(error);
                                }
                                eprintln!("automatic thread settlement skipped: {error}");
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(Error::Cancelled) => return Err(Error::Cancelled),
                    Err(error) => {
                        eprintln!("automatic thread settlement lookup failed: {error}")
                    }
                }
                Ok(())
            }));
        }
        join_thread_reads(reads).await?;
        Ok(())
    }

    async fn cleanup(&self, services: &ThreadSettlementServices, id: ThreadId) -> Result<()> {
        let Some(settled) = services.projections.thread(id.clone()).await? else {
            return Ok(());
        };
        if serde_json::json!(settled.settled_override) != "settled" {
            return Ok(());
        }
        self.ports.resources.close_on_settle(id.clone()).await?;
        let Some(worktree) = settled.worktree_path else {
            return Ok(());
        };
        let path = PathBuf::from(&worktree.0);
        if !tokio::task::spawn_blocking(move || path.try_exists())
            .await
            .map_err(|error| Error::Service {
                operation: "worktree exists",
                detail: error.to_string(),
            })??
        {
            return Ok(());
        }
        let Some(thread) = services.projections.thread(id.clone()).await? else {
            return Ok(());
        };
        if serde_json::json!(thread.settled_override) != "settled" {
            return Ok(());
        }
        let Some(stamp) = thread.settled_at.as_ref().and_then(|at| date_ms(&at.0)) else {
            return Ok(());
        };
        let mut cleaned = self.cleaned.lock().await;
        if cleaned.get(&id) == Some(&stamp) {
            return Ok(());
        }
        let lineage = serde_json::json!(thread.lineage);
        if lineage["relationshipToParent"] == "subagent"
            && let Some(parent) = lineage["parentThreadId"].as_str()
            && let Some(parent) = services.projections.thread(ThreadId(parent.into())).await?
            && parent.worktree_path == Some(worktree.clone())
        {
            return Ok(());
        }
        let run = self
            .ports
            .scripts
            .run(
                SettleScriptInput {
                    thread_id: id.clone(),
                    project_id: serde_json::from_value(serde_json::json!(thread.project_id))?,
                    worktree_path: worktree.0.into(),
                },
                self.cancellation.clone(),
            )
            .await?;
        cleaned.insert(id, stamp);
        if let SettleScriptRun::Started { completion } = run {
            let cancellation = self.cancellation.clone();
            self.tasks.spawn(async move {
                tokio::select! { biased;
                    _ = cancellation.cancelled() => {},
                    result = completion => match result {
                        Ok(Ok(())) => {},
                        other => eprintln!("settle script completion failed: {other:?}"),
                    }
                }
            });
        }
        Ok(())
    }
}
