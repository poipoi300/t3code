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

use crate::{
    api::{
        Clock, CommandDispatcher, EventCommitter, IdGenerator, IdKind, ProjectionReader,
        SettingsService, ThreadId,
    },
    projections::store::ProjectionStore,
    runtime::threads::{ThreadProjectWorker, ThreadServiceScope},
};
use std::{sync::Arc, time::Duration};
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

pub struct ThreadSettlementService {
    services: Arc<ThreadSettlementServices>,
    full_sweeps: ThreadProjectWorker<(), Option<ThreadId>>,
    thread_sweeps: ThreadProjectWorker<ThreadId, Option<ThreadId>>,
    tasks: TaskTracker,
    scope: ThreadServiceScope,
    cancellation: CancellationToken,
    started: Mutex<bool>,
}

impl ThreadSettlementService {
    pub fn new(services: ThreadSettlementServices) -> Self {
        let services = Arc::new(services);
        let make_worker = |concurrency| {
            let services = services.clone();
            ThreadProjectWorker::new(
                concurrency,
                |_: &mut Option<ThreadId>, _| {},
                move |thread, _| {
                    let services = services.clone();
                    async move {
                        if let Err(error) = sweep(&services, thread).await {
                            eprintln!("automatic thread settlement sweep failed: {error}");
                        }
                    }
                },
            )
        };
        // A targeted sweep must not queue behind a full sweep's external reads.
        let full_sweeps = make_worker(1);
        let thread_services = services.clone();
        let thread_sweeps = ThreadProjectWorker::new(
            8,
            |_: &mut Option<ThreadId>, _| {},
            move |thread, _| {
                let services = thread_services.clone();
                async move {
                    if let Err(error) = sweep(&services, thread).await {
                        eprintln!("automatic thread settlement sweep failed: {error}");
                    }
                }
            },
        );
        let scope = ThreadServiceScope::default();
        let cancellation = scope.cancellation();
        Self {
            services,
            full_sweeps,
            thread_sweeps,
            tasks: TaskTracker::new(),
            scope,
            cancellation,
            started: Mutex::new(false),
        }
    }

    pub async fn sweep(&self, thread_id: Option<ThreadId>) -> Result<()> {
        self.scope.run(sweep(&self.services, thread_id)).await
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
        let mut settings = self.services.settings.subscribe()?;
        let mut events = self.services.committer.subscribe();
        let mut settings_key = auto_settlement_settings_key(&self.services.settings.get().await?)?;
        self.enqueue(None).await?;
        *started = true;
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
        let event = serde_json::to_value(event)?;
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
        self.scope.shutdown().await;
        self.tasks.close();
        self.tasks.wait().await;
        tokio::join!(self.full_sweeps.shutdown(), self.thread_sweeps.shutdown());
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

async fn sweep(services: &ThreadSettlementServices, thread_id: Option<ThreadId>) -> Result<()> {
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
        let id = ThreadId(thread.id.0.clone());
        let Some(app_thread) = services.projections.thread(id.clone()).await? else {
            continue;
        };
        let candidate = SettlementCandidate {
            thread,
            last_snooze_wake_at: app_thread.last_snooze_wake_at.flatten(),
        };
        let decide = |settings: &t3_contracts::settings::ServerSettings, now| -> Result<_> {
            let (days, merge, keep) =
                settlement_settings(settings, &candidate.thread.project_id.0)?;
            Ok(resolve_auto_settlement_at(
                &candidate, None, now, days, merge, keep,
            ))
        };
        if !subagent && decide(&settings, now)?.is_none() {
            continue;
        }
        let current_settings = services.settings.get().await?;
        let Some(now) = date_ms(&services.clock.now().0) else {
            continue;
        };
        let settled_at = decide(&current_settings, now)?;
        if !subagent && settled_at.is_none() {
            continue;
        }
        let mut command = serde_json::json!({"type":"thread.auto-settle","commandId":format!("server:auto-settle:{}:{}",id.0,services.ids.next(IdKind::Command)),"threadId":id,"snapshotAt":candidate.thread.updated_at});
        if !subagent {
            command["settledAt"] = serde_json::to_value(settled_at)?;
        }
        if let Err(error) = services
            .dispatcher
            .dispatch(serde_json::from_value(command)?)
            .await
        {
            if matches!(error, crate::error::Error::Cancelled) {
                return Err(error);
            }
            eprintln!("automatic thread settlement skipped: {error}");
        }
    }
    Ok(())
}
