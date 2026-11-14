use crate::{
    Clock, Error, Result, SchedulerSource,
    schedule::{self, iso, parse_date},
    store::{self, Store},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};
use t3_contracts::{
    base_schemas::{ProjectId, ScheduledTaskId, ThreadId, TrimmedNonEmptyString},
    scheduled_task::*,
};
use t3_persistence::{
    Database,
    rusqlite::{OptionalExtension, params},
};
use tokio::sync::watch;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    Scheduled,
    Manual,
    Webhook,
}
/// A bound destination must queue using its current provider and follow project moves.
#[derive(Debug, Clone)]
pub struct DispatchRequest {
    pub task: ScheduledTask,
    pub trigger: Trigger,
    pub command_id: String,
    pub message_id: String,
    pub prompt: String,
    pub fresh_context: bool,
}
#[derive(Debug, Clone)]
pub struct DispatchReceipt {
    pub thread_id: ThreadId,
}
/// The future orchestration adapter owns thread validation, policy and provider execution.
#[async_trait]
pub trait Orchestration: Send + Sync + 'static {
    async fn bound_thread_modes(
        &self,
        _thread: &ThreadId,
    ) -> Result<Option<crate::intake::TaskRunMode>> {
        Err(Error::Invalid(
            "Bound task run modes require an orchestration lookup.".into(),
        ))
    }
    /// Enforce current policy, destination existence/project/archive and scratch-workspace rules.
    async fn validate(&self, _task: &ScheduledTask, _destination_changed: bool) -> Result<()> {
        Ok(())
    }
    /// Must deduplicate command/message ids, including replay after a webhook recovery.
    async fn dispatch(&self, request: DispatchRequest) -> Result<DispatchReceipt>;
    async fn has_queued_occurrence(&self, _task: &ScheduledTask) -> Result<bool> {
        Ok(false)
    }
    /// Return the eventual provider failure, or None for completion/cancelled steer/unknown run.
    async fn wait_for_failure(
        &self,
        _receipt: DispatchReceipt,
        _message_id: String,
    ) -> Result<Option<String>> {
        Ok(None)
    }
    /// None means the thread disappeared before its first binding landed.
    async fn thread_exists(&self, _thread: &ThreadId) -> Result<bool> {
        Ok(true)
    }
    async fn consume_secret(&self, _reference: &str, _project: &ProjectId) -> Result<String> {
        Err(Error::Invalid(
            "Secret references require a secret-store adapter.".into(),
        ))
    }
}
#[derive(Debug, Clone)]
pub struct SchedulerOptions {
    pub time_zone: Tz,
    /// Relay URL already scoped to its endpoint key, such as https://relay/v1/hooks/<key>.
    pub relay_hook_base_url: Option<String>,
    pub webhook_policy_block: Option<String>,
}
impl Default for SchedulerOptions {
    fn default() -> Self {
        let time_zone = std::env::var("TZ")
            .ok()
            .or_else(|| iana_time_zone::get_timezone().ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(chrono_tz::UTC);
        Self {
            time_zone,
            relay_hook_base_url: None,
            webhook_policy_block: None,
        }
    }
}
#[derive(Debug, Clone)]
pub struct RunNowResult {
    pub task: ScheduledTask,
    pub thread_id: Option<ThreadId>,
    pub dispatch_failed: bool,
}
pub(crate) struct Inner {
    pub store: Store,
    pub clock: Arc<dyn Clock>,
    pub backend: Arc<dyn Orchestration>,
    pub options: SchedulerOptions,
    pub changes: watch::Sender<u64>,
    pub active: Mutex<HashSet<String>>,
    cleanup_pending: Mutex<HashMap<String, String>>,
    pub admission: tokio::sync::Mutex<()>,
    pub rates: Mutex<HashMap<String, crate::webhook_service::Rates>>,
    pub webhook_workers: Mutex<HashSet<String>>,
    pub pending_workers: Mutex<HashSet<String>>,
    pub queues:
        Mutex<HashMap<String, std::collections::VecDeque<crate::webhook_service::QueuedWork>>>,
    pub tracker: TaskTracker,
    pub cancel: CancellationToken,
    pub due_hint: Mutex<crate::DueHint>,
    pub pending_hint: Mutex<crate::DueHint>,
}
pub struct ScheduledTaskService {
    pub(crate) inner: Arc<Inner>,
}
pub struct TaskSubscription {
    inner: std::sync::Weak<Inner>,
    changes: watch::Receiver<u64>,
    initial: Option<ScheduledTaskListResult>,
}
impl TaskSubscription {
    pub async fn next(&mut self) -> Result<Option<ScheduledTaskListResult>> {
        if let Some(initial) = self.initial.take() {
            return Ok(Some(initial));
        }
        if self.changes.changed().await.is_err() {
            return Ok(None);
        }
        let Some(inner) = self.inner.upgrade() else {
            return Ok(None);
        };
        let tasks = inner.store.read(store::all).await?;
        Ok(Some(ScheduledTaskListResult {
            tasks: tasks
                .into_iter()
                .map(|t| ScheduledTaskService::present(&inner, t))
                .collect(),
            time_zone: Some(Some(TrimmedNonEmptyString(
                inner.options.time_zone.to_string(),
            ))),
        }))
    }
}
impl ScheduledTaskService {
    /// Runs recovery before returning; no loop starts until registered with an activated Scheduler.
    pub async fn new(
        database: Database,
        clock: Arc<dyn Clock>,
        backend: Arc<dyn Orchestration>,
        options: SchedulerOptions,
    ) -> Result<Self> {
        let service = Self {
            inner: Arc::new(Inner {
                store: Store(database),
                clock,
                backend,
                options,
                changes: watch::channel(0).0,
                active: Mutex::new(HashSet::new()),
                cleanup_pending: Mutex::new(HashMap::new()),
                admission: tokio::sync::Mutex::new(()),
                rates: Mutex::new(HashMap::new()),
                webhook_workers: Mutex::new(HashSet::new()),
                pending_workers: Mutex::new(HashSet::new()),
                queues: Mutex::new(HashMap::new()),
                tracker: TaskTracker::new(),
                cancel: CancellationToken::new(),
                due_hint: Mutex::new(crate::DueHint::default()),
                pending_hint: Mutex::new(crate::DueHint::default()),
            }),
        };
        if let Err(error) = service.recover().await {
            tracing::warn!(%error, "Could not reset interrupted schedule task runs");
        }
        Ok(service)
    }
    pub(crate) fn notify(inner: &Inner) {
        inner
            .due_hint
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .invalidate();
        inner.changes.send_modify(|v| *v = v.wrapping_add(1));
    }
    pub(crate) fn present(inner: &Inner, mut task: ScheduledTask) -> ScheduledTask {
        if matches!(task.schedule, ScheduledTaskSchedule::Webhook(_)) {
            task.blocked_by_policy = inner.options.webhook_policy_block.clone();
            if let Some(Some(endpoint)) = &mut task.webhook {
                endpoint.url = inner.options.relay_hook_base_url.as_ref().map(|base| {
                    TrimmedNonEmptyString(format!(
                        "{}/{}",
                        base.trim_end_matches('/'),
                        endpoint.path.0.trim_start_matches("/api/hooks/")
                    ))
                });
            }
        }
        task
    }
    pub async fn list(&self) -> Result<ScheduledTaskListResult> {
        let tasks = self.inner.store.read(store::all).await?;
        Ok(ScheduledTaskListResult {
            tasks: tasks
                .into_iter()
                .map(|t| Self::present(&self.inner, t))
                .collect(),
            time_zone: Some(Some(TrimmedNonEmptyString(
                self.inner.options.time_zone.to_string(),
            ))),
        })
    }
    /// Subscribe before list(); watch coalesces dirty notifications for slow subscribers.
    pub fn subscribe_changes(&self) -> watch::Receiver<u64> {
        self.inner.changes.subscribe()
    }
    pub async fn subscribe_list(&self) -> Result<TaskSubscription> {
        let changes = self.inner.changes.subscribe();
        let initial = Some(self.list().await?);
        Ok(TaskSubscription {
            inner: Arc::downgrade(&self.inner),
            changes,
            initial,
        })
    }
    pub async fn get(&self, id: &ScheduledTaskId) -> Result<ScheduledTask> {
        let id = id.0.clone();
        self.inner
            .store
            .read(move |c| store::load(c, &id))
            .await
            .map(|t| Self::present(&self.inner, t))
    }
    pub async fn run_modes(&self, id: &ScheduledTaskId) -> Result<Vec<crate::intake::TaskRunMode>> {
        let task = self.get(id).await?;
        let mut modes = vec![crate::intake::TaskRunMode {
            runtime_mode: task.runtime_mode,
            interaction_mode: task.interaction_mode,
        }];
        if let Some(thread) = task.thread_id
            && let Some(mode) = self.inner.backend.bound_thread_modes(&thread).await?
            && !modes.contains(&mode)
        {
            modes.push(mode);
        }
        Ok(modes)
    }
    pub async fn upsert(&self, input: ScheduledTaskUpsertInput) -> Result<ScheduledTask> {
        crate::validation::validate(&input)?;
        if input.enabled
            && matches!(input.schedule, ScheduledTaskUpsertSchedule::Webhook(_))
            && let Some(reason) = &self.inner.options.webhook_policy_block
        {
            return Err(Error::Policy(reason.clone()));
        }
        let now = self.inner.clock.now();
        let id = input.id.clone().flatten().unwrap_or_else(|| {
            ScheduledTaskId(format!(
                "scheduled-task:{}",
                input
                    .command_id
                    .as_ref()
                    .and_then(Option::as_ref)
                    .map(|s| s.0.clone())
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
            ))
        });
        let key = id.0.clone();
        let existing = self.inner.store.read(move |c| store::find(c, &key)).await?;
        if input.require_existing.flatten() == Some(true) && existing.is_none() {
            return Err(Error::NotFound(id.0));
        }
        let stored = existing
            .as_ref()
            .and_then(|t| t.thread_mode.clone().flatten())
            .unwrap_or(ScheduledTaskThreadMode::NewThread);
        let mode = input.thread_mode.clone().flatten().unwrap_or_else(|| {
            if stored == ScheduledTaskThreadMode::NewThread
                && input.thread_id.as_ref().and_then(Option::as_ref).is_some()
            {
                ScheduledTaskThreadMode::Continue
            } else {
                stored
            }
        });
        let kept = input
            .thread_id
            .clone()
            .flatten()
            .or_else(|| existing.as_ref().and_then(|t| t.thread_id.clone()));
        let moved = existing
            .as_ref()
            .is_some_and(|t| t.project_id != input.project_id && kept == t.thread_id);
        let thread_id = if mode == ScheduledTaskThreadMode::NewThread || moved {
            None
        } else {
            kept
        };
        let (schedule, mut secret, secret_ref) =
            Self::convert_schedule(&input.schedule, existing.as_ref())?;
        let unchanged = existing.as_ref().is_some_and(|t| {
            t.enabled == input.enabled && schedule::is_same_schedule(&t.schedule, &schedule)
        });
        let next = if unchanged {
            existing.as_ref().and_then(|t| t.next_run_at.clone())
        } else {
            next_run(input.enabled, &schedule, now, self.inner.options.time_zone)
        };
        if !unchanged {
            require_future_once(input.enabled, &schedule, next.as_ref())?;
        }
        let timestamp = t3_contracts::wire::WireDateTime(iso(now));
        let mut task = ScheduledTask {
            id,
            title: input.title,
            prompt: input.prompt,
            enabled: input.enabled,
            schedule,
            project_id: input.project_id,
            thread_id,
            thread_mode: Some(Some(mode)),
            workspace_strategy: input.workspace_strategy,
            model_selection: input.model_selection,
            runtime_mode: input.runtime_mode,
            interaction_mode: input.interaction_mode,
            created_by: existing
                .as_ref()
                .map(|t| t.created_by.clone())
                .or(input.created_by.flatten())
                .unwrap_or(t3_contracts::orchestration_v2::OrchestrationV2Actor::User),
            creation_source: input
                .creation_source
                .flatten()
                .unwrap_or(t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web),
            created_at: existing
                .as_ref()
                .map(|t| t.created_at.clone())
                .unwrap_or(timestamp.clone()),
            updated_at: timestamp,
            next_run_at: next,
            last_run_at: existing.as_ref().and_then(|t| t.last_run_at.clone()),
            last_run_status: existing
                .as_ref()
                .map(|t| t.last_run_status.clone())
                .unwrap_or(ScheduledTaskRunStatus::Never),
            last_run_error: existing.as_ref().and_then(|t| t.last_run_error.clone()),
            run_count: existing.as_ref().map_or(0, |t| t.run_count),
            webhook: None,
            blocked_by_policy: None,
        };
        let changed = task.thread_id.is_some()
            && task.thread_id != existing.as_ref().and_then(|t| t.thread_id.clone());
        self.inner.backend.validate(&task, changed).await?;
        if let Some(reference) = secret_ref {
            match self
                .inner
                .backend
                .consume_secret(&reference, &task.project_id)
                .await
            {
                Ok(value) => secret = Some(Some(value)),
                Err(_) if input.command_id.flatten().is_some() && existing.is_some() => {}
                Err(e) => return Err(e),
            }
        }
        let key = task.id.0.clone();
        let _admission = self.inner.admission.lock().await;
        task = self
            .inner
            .store
            .write(move |c| {
                let current = store::find(c, &key)?;
                if existing.is_some() && current.is_none() {
                    return Err(Error::NotFound(key));
                }
                let credentials: Option<(Option<String>, Option<String>)> = c.query_row("SELECT webhook_token,webhook_secret FROM scheduled_tasks WHERE task_id=?1", [&key], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
                let (token, old_secret) = credentials.unwrap_or_default();
                let is_hook = matches!(task.schedule, ScheduledTaskSchedule::Webhook(_));
                let token = if is_hook {
                    Some(match token {
                        Some(token) => token,
                        None => new_token()?,
                    })
                } else {
                    None
                };
                let secret = if is_hook { secret.unwrap_or(old_secret) } else { None };
                if matches!(&task.schedule,ScheduledTaskSchedule::Webhook(s) if s.signature.is_some()) && secret.is_none() {
                    return Err(Error::Invalid("A webhook signature check needs a signing secret.".into()));
                }
                if let Some(current) = &current {
                    if unchanged
                        && existing
                            .as_ref()
                            .is_some_and(|t| schedule::is_same_schedule(&t.schedule, &current.schedule) && t.enabled == current.enabled)
                    {
                        task.next_run_at = current.next_run_at.clone();
                    }
                    task.last_run_at = current.last_run_at.clone();
                    task.last_run_status = current.last_run_status.clone();
                    task.last_run_error = current.last_run_error.clone();
                    task.run_count = current.run_count;
                }
                store::save(c, &task, token.as_deref(), secret.as_deref())?;
                store::load(c, &key)
            })
            .await?;
        Self::notify(&self.inner);
        Ok(Self::present(&self.inner, task))
    }
    fn convert_schedule(
        input: &ScheduledTaskUpsertSchedule,
        existing: Option<&ScheduledTask>,
    ) -> Result<(
        ScheduledTaskSchedule,
        Option<Option<String>>,
        Option<String>,
    )> {
        Ok(match input {
            ScheduledTaskUpsertSchedule::Interval(v) => {
                (ScheduledTaskSchedule::Interval(v.clone()), None, None)
            }
            ScheduledTaskUpsertSchedule::FixedTime(v) => {
                (ScheduledTaskSchedule::FixedTime(v.clone()), None, None)
            }
            ScheduledTaskUpsertSchedule::Once(v) => {
                let date = parse_date(&v.run_at.0)
                    .ok_or_else(|| Error::Invalid("Invalid one-time run time.".into()))?;
                (
                    ScheduledTaskSchedule::Once(Box::new(ScheduledTaskScheduleOnce {
                        run_at: TrimmedNonEmptyString(iso(date)),
                    })),
                    None,
                    None,
                )
            }
            ScheduledTaskUpsertSchedule::Webhook(v) => {
                let signature = v.signature.as_ref().and_then(Option::as_ref);
                let old = existing.and_then(|t| {
                    if let ScheduledTaskSchedule::Webhook(s) = &t.schedule {
                        Some(s)
                    } else {
                        None
                    }
                });
                let secret_ref = signature
                    .and_then(|s| s.secret_ref.as_ref().and_then(Option::as_ref))
                    .map(|s| s.0.clone());
                let secret = if signature.is_none() {
                    Some(None)
                } else if secret_ref.is_some() {
                    None
                } else {
                    signature
                        .and_then(|s| s.secret.as_ref().and_then(Option::as_ref))
                        .map(|s| Some(s.0.clone()))
                };
                let schedule =
                    ScheduledTaskSchedule::Webhook(Box::new(ScheduledTaskScheduleWebhook {
                        signature: signature.map(|s| ScheduledTaskWebhookSignature {
                            header: TrimmedNonEmptyString(s.header.0.to_lowercase()),
                            encoding: s.encoding.clone(),
                            prefix: s.prefix.clone(),
                        }),
                        max_delivery_age_minutes: v.max_delivery_age_minutes,
                        dedupe: v
                            .dedupe
                            .clone()
                            .or_else(|| old.and_then(|s| s.dedupe.clone())),
                        filters: v
                            .filters
                            .clone()
                            .or_else(|| old.and_then(|s| s.filters.clone())),
                    }));
                (schedule, secret, secret_ref)
            }
        })
    }
    pub async fn set_enabled(&self, input: ScheduledTaskSetEnabledInput) -> Result<ScheduledTask> {
        let mut task = self.get(&input.id).await?;
        if task.enabled == input.enabled {
            return Ok(task);
        }
        if input.enabled
            && matches!(task.schedule, ScheduledTaskSchedule::Webhook(_))
            && let Some(reason) = &self.inner.options.webhook_policy_block
        {
            return Err(Error::Policy(reason.clone()));
        }
        if input.enabled {
            task.enabled = true;
            self.inner.backend.validate(&task, false).await?;
        }
        let now = self.inner.clock.now();
        let zone = self.inner.options.time_zone;
        let task = self
            .inner
            .store
            .write(move |c| {
                let mut task = store::load(c, &input.id.0)?;
                let next = next_run(input.enabled, &task.schedule, now, zone);
                require_future_once(input.enabled, &task.schedule, next.as_ref())?;
                c.execute("UPDATE scheduled_tasks SET enabled=?2,next_run_at=?3,updated_at=?4 WHERE task_id=?1", params![input.id.0, input.enabled, next.as_ref().map(|v| &v.0), iso(now)])?;
                task.enabled = input.enabled;
                task.next_run_at = next;
                task.updated_at = t3_contracts::wire::WireDateTime(iso(now));
                Ok(task)
            })
            .await?;
        Self::notify(&self.inner);
        Ok(Self::present(&self.inner, task))
    }
    pub async fn delete(&self, id: &ScheduledTaskId) -> Result<()> {
        let id = id.0.clone();
        let key = id.clone();
        self.inner
            .store
            .write(move |c| {
                c.execute(
                    "DELETE FROM scheduled_task_webhook_deliveries WHERE task_id=?1",
                    [&id],
                )?;
                c.execute("DELETE FROM scheduled_tasks WHERE task_id=?1", [&id])?;
                Ok(())
            })
            .await?;
        self.inner
            .rates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&key);
        Self::notify(&self.inner);
        Ok(())
    }
    pub async fn delete_for_thread(&self, thread: &ThreadId) -> Result<()> {
        delete_thread_tasks(&self.inner, thread.0.clone()).await
    }
    pub async fn move_thread_tasks(&self, threads: &[ThreadId], project: &ProjectId) -> Result<()> {
        let threads = threads.to_vec();
        let project = project.0.clone();
        let moved = self
            .inner
            .store
            .write(move |c| {
                let mut moved = 0;
                for thread in threads {
                    moved += c.execute("UPDATE scheduled_tasks SET project_id=?2 WHERE thread_id=?1 AND project_id<>?2", params![thread.0, project])?;
                }
                Ok(moved)
            })
            .await?;
        if moved > 0 {
            Self::notify(&self.inner);
        }
        Ok(())
    }
    pub async fn run_now(&self, id: &ScheduledTaskId) -> Result<RunNowResult> {
        let task = self.get(id).await?;
        if matches!(task.schedule, ScheduledTaskSchedule::Webhook(_)) {
            return Err(Error::Invalid(
                "Webhook tasks run when their URL receives a request.".into(),
            ));
        }
        run_task(self.inner.clone(), task, Trigger::Manual, None).await
    }
    pub async fn run_due_tasks(&self) -> Result<()> {
        run_due(self.inner.clone()).await
    }
    async fn recover(&self) -> Result<()> {
        let now = self.inner.clock.now();
        let zone = self.inner.options.time_zone;
        self.inner
            .store
            .write(move |c| {
                let mut stmt = c.prepare("SELECT * FROM scheduled_tasks WHERE last_run_status='running'")?;
                let recovered = stmt
                    .query_map([], |row| {
                        let id: String = row.get("task_id")?;
                        let next = store::decode_task(row).ok().map(|t| next_after_run(&t, now, zone));
                        Ok((id, next))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                drop(stmt);
                for (id, next) in recovered {
                    let count: i64 = c.query_row("SELECT CASE WHEN EXISTS (SELECT 1 FROM scheduled_task_webhook_pending WHERE task_id=?1 AND state='running' AND delivery_id=(SELECT last_run_webhook_delivery_id FROM scheduled_tasks WHERE task_id=?1)) THEN 0 ELSE 1 END", [&id], |r| r.get(0))?;
                    if let Some(next) = next {
                        c.execute("UPDATE scheduled_tasks SET last_run_status='failed',last_run_error='Run was interrupted by a server restart.',updated_at=?2,next_run_at=?3,run_count=run_count+?4 WHERE task_id=?1 AND last_run_status='running'", params![id, iso(now), next.as_ref().map(|v| &v.0), count])?;
                    } else {
                        c.execute("UPDATE scheduled_tasks SET last_run_status='failed',last_run_error='Run was interrupted by a server restart.',updated_at=?2,run_count=run_count+?3 WHERE task_id=?1 AND last_run_status='running'", params![id, iso(now), count])?;
                    }
                }
                let calendar_tasks = {
                    let mut stmt = c.prepare("SELECT * FROM scheduled_tasks WHERE enabled=1 AND next_run_at IS NOT NULL AND last_run_status<>'running'")?;
                    stmt.query_map([], |row| Ok(store::decode_task(row).ok()))?.collect::<std::result::Result<Vec<_>, _>>()?
                };
                for task in calendar_tasks.into_iter().flatten().filter(|task| matches!(task.schedule, ScheduledTaskSchedule::FixedTime(_))) {
                    let Some(due) = task.next_run_at.as_ref().and_then(|v| parse_date(&v.0)) else {
                        continue;
                    };
                    // Keep valid overdue slots for the missed-run grace, but discard stale tzdb slots.
                    let valid_slot = due.checked_sub_signed(chrono::TimeDelta::milliseconds(1))
                        .is_some_and(|before| schedule::next_scheduled_run_at(&task.schedule, before, zone) == Some(due));
                    if due > now || !valid_slot {
                        let next = next_run(true, &task.schedule, now, zone);
                        if task.next_run_at != next {
                            c.execute("UPDATE scheduled_tasks SET next_run_at=?2 WHERE task_id=?1", params![task.id.0, next.as_ref().map(|v| &v.0)])?;
                        }
                    }
                }
                Ok(())
            })
            .await?;
        Self::notify(&self.inner);
        Ok(())
    }
    pub async fn shutdown(&self) {
        self.inner.cancel.cancel();
        self.inner.tracker.close();
        self.inner.tracker.wait().await;
    }
    pub async fn drain_background(&self) {
        self.inner.tracker.close();
        self.inner.tracker.wait().await;
        self.inner.tracker.reopen();
    }
    /// Separate sources keep a slow scheduled dispatch from holding webhook windows.
    pub fn sources(&self) -> [Arc<dyn SchedulerSource>; 2] {
        [
            Arc::new(TaskSource(Arc::downgrade(&self.inner))),
            Arc::new(crate::webhook_service::WebhookSource(Arc::downgrade(
                &self.inner,
            ))),
        ]
    }
}
impl Drop for ScheduledTaskService {
    fn drop(&mut self) {
        self.inner.cancel.cancel();
        self.inner.tracker.close();
    }
}
async fn delete_thread_tasks(inner: &Inner, thread: String) -> Result<()> {
    let removed = inner
        .store
        .write(move |c| {
            c.execute("DELETE FROM scheduled_task_webhook_deliveries WHERE task_id IN (SELECT task_id FROM scheduled_tasks WHERE thread_id=?1)", [&thread])?;
            Ok(c.prepare("DELETE FROM scheduled_tasks WHERE thread_id=?1 RETURNING task_id")?
                .query_map([thread], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?)
        })
        .await?;
    if !removed.is_empty() {
        let mut rates = inner.rates.lock().unwrap_or_else(|e| e.into_inner());
        for id in removed {
            rates.remove(&id);
        }
        ScheduledTaskService::notify(inner);
    }
    Ok(())
}
struct TaskSource(std::sync::Weak<Inner>);
#[async_trait]
impl SchedulerSource for TaskSource {
    async fn run_due_work(&self) -> Result<()> {
        if let Some(inner) = self.0.upgrade() {
            run_due(inner).await
        } else {
            Ok(())
        }
    }
}
pub(crate) fn next_run(
    enabled: bool,
    schedule: &ScheduledTaskSchedule,
    from: DateTime<Utc>,
    zone: Tz,
) -> Option<t3_contracts::wire::WireDateTime> {
    enabled
        .then(|| schedule::next_scheduled_run_at(schedule, from, zone))
        .flatten()
        .map(|d| t3_contracts::wire::WireDateTime(iso(d)))
}
fn next_after_run(
    task: &ScheduledTask,
    now: DateTime<Utc>,
    zone: Tz,
) -> Option<t3_contracts::wire::WireDateTime> {
    let from = if matches!(task.schedule, ScheduledTaskSchedule::Once(_)) {
        task.last_run_at
            .as_ref()
            .and_then(|d| parse_date(&d.0))
            .unwrap_or(now)
    } else {
        now
    };
    next_run(task.enabled, &task.schedule, from, zone)
}
fn require_future_once(
    enabled: bool,
    schedule: &ScheduledTaskSchedule,
    next: Option<&t3_contracts::wire::WireDateTime>,
) -> Result<()> {
    if enabled && matches!(schedule, ScheduledTaskSchedule::Once(_)) && next.is_none() {
        Err(Error::Invalid(
            "The run time has passed. Pick a time in the future.".into(),
        ))
    } else {
        Ok(())
    }
}
pub(crate) fn new_token() -> Result<String> {
    use base64::Engine;
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|e| Error::Invalid(format!("Could not generate webhook token: {e}")))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}
pub(crate) fn frame_id() -> Result<String> {
    let mut bytes = [0u8; 6];
    getrandom::fill(&mut bytes)
        .map_err(|e| Error::Invalid(format!("Could not generate frame id: {e}")))?;
    Ok(hex::encode(bytes))
}
async fn run_due(inner: Arc<Inner>) -> Result<()> {
    let now = inner.clock.now();
    let now_iso = iso(now);
    let version = {
        let hint = inner.due_hint.lock().unwrap_or_else(|e| e.into_inner());
        if hint.is_idle(now.timestamp_millis()) {
            return Ok(());
        }
        hint.version()
    };
    let (tasks, next) = inner
        .store
        .read(move |c| {
            let next: Option<String> = c.query_row("SELECT MIN(next_run_at) FROM scheduled_tasks WHERE enabled=1 AND next_run_at IS NOT NULL AND last_run_status<>'running'", [], |r| r.get(0))?;
            if next.as_ref().is_none_or(|s| s > &now_iso) {
                return Ok((
                    Vec::new(),
                    match next {
                        None => Some(i64::MAX),
                        Some(s) => parse_date(&s).map(|d| d.timestamp_millis()),
                    },
                ));
            }
            Ok((store::due(c, &now_iso)?, None))
        })
        .await?;
    if let Some(next) = next {
        inner
            .due_hint
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .record(version, next, now.timestamp_millis());
    }
    for task in tasks {
        if let Some(due) = task.next_run_at.as_ref().and_then(|d| parse_date(&d.0))
            && schedule::is_missed_fixed_time_run(&task.schedule, due, now)
        {
            if let Err(error) = reschedule(
                &inner,
                task,
                now,
                Some("Missed a scheduled run because the server was not running at that time."),
            )
            .await
            {
                tracing::warn!(%error,"Could not reschedule missed task");
            }
        } else {
            if let Err(error) = run_task(inner.clone(), task, Trigger::Scheduled, None).await {
                tracing::warn!(%error,"Scheduled task run failed");
            }
        }
    }
    Ok(())
}
pub(crate) async fn reschedule(
    inner: &Inner,
    task: ScheduledTask,
    now: DateTime<Utc>,
    error: Option<&str>,
) -> Result<()> {
    let next = next_run(task.enabled, &task.schedule, now, inner.options.time_zone);
    let error = error.map(str::to_owned);
    let changed = inner
        .store
        .write(move |c| {
            let changed = c.execute(
                "UPDATE scheduled_tasks SET next_run_at=?2,updated_at=?3,last_run_status=CASE WHEN ?4 IS NULL THEN last_run_status ELSE 'failed' END,last_run_error=CASE WHEN ?4 IS NULL THEN last_run_error ELSE ?4 END WHERE task_id=?1 AND next_run_at IS ?5 AND last_run_at IS ?6",
                params![
                    task.id.0,
                    next.as_ref().map(|v| &v.0),
                    iso(now),
                    error,
                    task.next_run_at.as_ref().map(|v| &v.0),
                    task.last_run_at.as_ref().map(|v| &v.0)
                ],
            )?;
            Ok(changed)
        })
        .await?;
    if changed > 0 {
        ScheduledTaskService::notify(inner);
    }
    Ok(())
}
struct RunGuard {
    inner: Arc<Inner>,
    id: String,
    started: Option<String>,
    claimed: Arc<std::sync::atomic::AtomicBool>,
}
async fn clear_interrupted(inner: &Inner, id: String, started: String) -> Result<()> {
    let _admission = inner.admission.lock().await;
    if inner
        .cleanup_pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        != Some(&started)
    {
        return Ok(());
    }
    let now = inner.clock.now();
    let zone = inner.options.time_zone;
    let key = id.clone();
    let start = started.clone();
    inner
        .store
        .write(move |c| {
            let Some(task) = store::find(c, &key)? else {
                return Ok(());
            };
            let next = next_after_run(&task, now, zone);
            c.execute("UPDATE scheduled_tasks SET last_run_status='failed',last_run_error='Run was interrupted.',next_run_at=?3,updated_at=?4,run_count=run_count+CASE WHEN EXISTS (SELECT 1 FROM scheduled_task_webhook_pending WHERE task_id=?1 AND state='running' AND delivery_id=scheduled_tasks.last_run_webhook_delivery_id) THEN 0 ELSE 1 END WHERE task_id=?1 AND last_run_status='running' AND last_run_at=?2", params![key, start, next.as_ref().map(|v| &v.0), iso(now)])?;
            Ok(())
        })
        .await?;
    let mut cleanup = inner
        .cleanup_pending
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if cleanup.get(&id) == Some(&started) {
        cleanup.remove(&id);
    }
    ScheduledTaskService::notify(inner);
    Ok(())
}
impl Drop for RunGuard {
    fn drop(&mut self) {
        if let Some(started) = self
            .started
            .take()
            .filter(|_| self.claimed.load(std::sync::atomic::Ordering::Acquire))
        {
            self.inner
                .cleanup_pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(self.id.clone(), started.clone());
            let inner = self.inner.clone();
            let id = self.id.clone();
            self.inner.tracker.spawn(async move {
                for attempt in 0..3 {
                    match clear_interrupted(&inner, id.clone(), started.clone()).await {
                        Ok(()) => break,
                        Err(error) => tracing::warn!(%error, attempt, "Could not clear interrupted scheduled task run"),
                    }
                }
            });
        }
        self.inner
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}
pub(crate) async fn run_task(
    inner: Arc<Inner>,
    snapshot: ScheduledTask,
    trigger: Trigger,
    webhook: Option<(String, String)>,
) -> Result<RunNowResult> {
    let id = snapshot.id.0.clone();
    {
        let mut active = inner.active.lock().unwrap_or_else(|e| e.into_inner());
        if !active.insert(id.clone()) {
            return if trigger == Trigger::Scheduled {
                Ok(RunNowResult {
                    task: snapshot,
                    thread_id: None,
                    dispatch_failed: false,
                })
            } else {
                Err(Error::AlreadyRunning(id))
            };
        }
    }
    let mut guard = RunGuard {
        inner: inner.clone(),
        id: id.clone(),
        started: None,
        claimed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let interrupted = inner
        .cleanup_pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .cloned();
    if let Some(started) = interrupted
        && let Err(error) = clear_interrupted(&inner, id.clone(), started).await
    {
        tracing::warn!(%error, "Scheduled task cleanup is still pending");
        return Err(Error::AlreadyRunning(id));
    }
    let now = inner.clock.now();
    let started = iso(now);
    let key = id.clone();
    let current = inner.store.read(move |c| store::find(c, &key)).await?;
    let Some(task) = current else {
        return if trigger == Trigger::Scheduled {
            Ok(RunNowResult {
                task: snapshot,
                thread_id: None,
                dispatch_failed: false,
            })
        } else {
            Err(Error::NotFound(id))
        };
    };
    if trigger == Trigger::Scheduled
        && (!task.enabled
            || task
                .next_run_at
                .as_ref()
                .and_then(|d| parse_date(&d.0))
                .is_none_or(|d| d > now))
    {
        return Ok(RunNowResult {
            task,
            thread_id: None,
            dispatch_failed: false,
        });
    }
    if webhook.is_some()
        && let Some(reason) = &inner.options.webhook_policy_block
    {
        return Err(Error::Skipped(reason.clone()));
    }
    if webhook.is_some()
        && (task.created_at != snapshot.created_at
            || !task.enabled
            || !matches!(task.schedule, ScheduledTaskSchedule::Webhook(_)))
    {
        let reason = if task.created_at != snapshot.created_at {
            "The task was replaced before this delivery ran."
        } else if !matches!(task.schedule, ScheduledTaskSchedule::Webhook(_)) {
            "The task's trigger changed before this delivery ran."
        } else {
            "The task was paused before this delivery ran."
        };
        return Err(Error::Skipped(reason.into()));
    }
    if trigger == Trigger::Scheduled
        && !matches!(task.schedule, ScheduledTaskSchedule::Once(_))
        && task.thread_id.is_some()
        && inner
            .backend
            .has_queued_occurrence(&task)
            .await
            .unwrap_or(false)
    {
        reschedule(&inner, task.clone(), now, None).await?;
        return Ok(RunNowResult {
            task,
            thread_id: None,
            dispatch_failed: false,
        });
    }
    let key = id.clone();
    let start = started.clone();
    let created = task.created_at.0.clone();
    let delivery = webhook.as_ref().map(|(id, _)| id.clone());
    // Arm cleanup before awaiting a non-cancellable SQLite closure.
    guard.started = Some(started.clone());
    let expected_next = task.next_run_at.as_ref().map(|v| v.0.clone());
    let scheduled = trigger == Trigger::Scheduled;
    let claimed = guard.claimed.clone();
    inner
        .store
        .write(move |c| {
            let updated = c.execute("UPDATE scheduled_tasks SET updated_at=?2,last_run_at=?2,last_run_webhook_delivery_id=?3,last_run_status='running',last_run_error=NULL WHERE task_id=?1 AND created_at=?4 AND (last_run_status<>'running' OR (?3 IS NOT NULL AND last_run_webhook_delivery_id=?3 AND EXISTS (SELECT 1 FROM scheduled_task_webhook_pending WHERE delivery_id=?3 AND state='running'))) AND (?5=0 OR next_run_at IS ?6)", params![key, start, delivery, created, scheduled, expected_next])?;
            if updated == 0 {
                return Err(Error::AlreadyRunning(key));
            }
            claimed.store(true, std::sync::atomic::Ordering::Release);
            if let Some(delivery) = delivery {
                c.execute("UPDATE scheduled_task_webhook_pending SET state='running' WHERE delivery_id=?1", [delivery])?;
            }
            Ok(())
        })
        .await?;
    ScheduledTaskService::notify(&inner);
    let fire = webhook.as_ref().map_or_else(
        || {
            format!(
                "{id}:{}:{}",
                now.timestamp_millis(),
                match trigger {
                    Trigger::Scheduled => "scheduled",
                    Trigger::Manual => "manual",
                    Trigger::Webhook => "webhook",
                }
            )
        },
        |(delivery, _)| format!("{id}:webhook:{delivery}"),
    );
    let mode = task.thread_mode.as_ref().and_then(Option::as_ref);
    let message_id = format!("scheduled-task-message:{fire}");
    let request = DispatchRequest {
        task: task.clone(),
        trigger,
        command_id: format!("scheduled-task:{fire}"),
        message_id: message_id.clone(),
        prompt: webhook
            .as_ref()
            .map_or_else(|| task.prompt.0.clone(), |(_, p)| p.clone()),
        fresh_context: mode == Some(&ScheduledTaskThreadMode::FreshSession)
            && task.thread_id.is_some(),
    };
    let result = std::panic::AssertUnwindSafe(inner.backend.dispatch(request))
        .catch_unwind()
        .await
        .unwrap_or_else(|panic| {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "Orchestration dispatch panicked.".into());
            Err(Error::Dispatch(message))
        });
    if let Ok(receipt) = &result
        && task.thread_id.is_none()
        && mode != Some(&ScheduledTaskThreadMode::NewThread)
    {
        let key = id.clone();
        let created = task.created_at.0.clone();
        let project = task.project_id.0.clone();
        let thread = receipt.thread_id.0.clone();
        if let Err(error) = inner
            .store
            .write(move |c| {
                c.execute("UPDATE scheduled_tasks SET thread_id=?2 WHERE task_id=?1 AND created_at=?3 AND project_id=?4 AND thread_id IS NULL AND thread_mode!='new_thread'", params![key, thread, created, project])?;
                Ok(())
            })
            .await
        {
            tracing::warn!(%error,"Could not bind scheduled task to its first thread");
        }
    }
    let dispatch_failed = result.is_err();
    let completed = inner.clock.now();
    let zone = inner.options.time_zone;
    let thread = result.as_ref().ok().map(|r| r.thread_id.clone());
    let failure = result.as_ref().err().map(ToString::to_string);
    let key = id.clone();
    let start = started.clone();
    let original = task.clone();
    let hook = webhook.as_ref().map(|(d, _)| d.clone());
    let recorded = inner
        .store
        .write(move |c| {
            let current = store::find(c, &key)?;
            let mut record = current.clone().unwrap_or(original);
            let next = next_run(
                record.enabled,
                &record.schedule,
                if matches!(record.schedule, ScheduledTaskSchedule::Once(_)) { now } else { completed },
                zone,
            );
            let written = c.execute(
                "UPDATE scheduled_tasks SET updated_at=?3,next_run_at=?4,last_run_status=?5,last_run_error=?6,run_count=run_count+1 WHERE task_id=?1 AND last_run_status='running' AND last_run_at=?2",
                params![key, start, iso(completed), next.as_ref().map(|v| &v.0), if failure.is_some() { "failed" } else { "succeeded" }, failure],
            )?;
            if let Some(hook) = hook {
                c.execute("UPDATE scheduled_task_webhook_pending SET state=?2 WHERE delivery_id=?1", params![hook, if failure.is_some() { "failed" } else { "completed" }])?;
            }
            if written > 0 || current.is_none() {
                record.updated_at = t3_contracts::wire::WireDateTime(iso(completed));
                record.next_run_at = next;
                record.last_run_at = Some(t3_contracts::wire::WireDateTime(start));
                record.last_run_status = if failure.is_some() { ScheduledTaskRunStatus::Failed } else { ScheduledTaskRunStatus::Succeeded };
                record.last_run_error = failure;
                record.run_count += 1;
            }
            Ok(record)
        })
        .await
        .map_err(|error| if dispatch_failed { error } else { Error::CompletionAfterDispatch(Box::new(error)) })?;
    guard.started = None;
    ScheduledTaskService::notify(&inner);
    if let Ok(receipt) = result {
        if task.thread_id.is_none()
            && mode != Some(&ScheduledTaskThreadMode::NewThread)
            && !inner
                .backend
                .thread_exists(&receipt.thread_id)
                .await
                .unwrap_or(true)
            && let Err(error) = delete_thread_tasks(&inner, receipt.thread_id.0.clone()).await
        {
            tracing::warn!(%error,"Could not clean up scheduled tasks for a deleted thread");
        }
        let follow = inner.clone();
        let start = started;
        inner.tracker.spawn(async move {
            tokio::select! {
                            _=follow.cancel.cancelled()=>{
            }
            ,
                            _=follow.clock.sleep(Duration::from_secs(24*60*60))=>{
            }
            ,
                            outcome=follow.backend.wait_for_failure(receipt,message_id)=>{
                                if let Ok(Some(error))=outcome {
                                    let _=follow.store.write(move |c| {
             c.execute("UPDATE scheduled_tasks SET last_run_status='failed',last_run_error=?3 WHERE task_id=?1 AND last_run_at=?2 AND last_run_status='succeeded'",params![id,start,error])?;
             Ok(()) }
            ).await;
                                    ScheduledTaskService::notify(&follow);
                                }
                            }
                        }
        });
    }
    Ok(RunNowResult {
        task: ScheduledTaskService::present(&inner, recorded),
        thread_id: thread,
        dispatch_failed,
    })
}
