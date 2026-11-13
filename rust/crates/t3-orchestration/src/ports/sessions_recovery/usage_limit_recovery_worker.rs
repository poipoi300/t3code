use crate::{
    api::{
        Clock, CommandDispatcher, CommitGuard, CommitOptions, CommitRequest, Decision,
        DecisionContext, DecisionEngine, EventCommitter, IdGenerator, IdKind, MessageFilter,
        ProjectionRecordQuery, SettingsService,
    },
    error::{Error, Result},
    projections::store::ProjectionStore,
};
use async_trait::async_trait;
use serde_json::json;
use std::sync::{Arc, Mutex};
use t3_contracts::{base_schemas::*, orchestration_v2::*};

pub const MAX_OVERDUE_ARM_MS: i64 = 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone)]
pub struct LimitRecoveryCandidate {
    pub id: ThreadId,
    pub status: OrchestrationV2ShellThreadStatus,
    pub last_error_class: Option<OrchestrationV2ProviderFailureClass>,
    pub latest_run_id: Option<RunId>,
    pub usage_limit_reset_at: Option<String>,
    pub latest_run_completed_at: Option<IsoDateTime>,
    pub updated_at: IsoDateTime,
    pub archived_at: Option<IsoDateTime>,
    pub settled_override: Option<AppThreadSettledOverride>,
    pub pending_runtime_request: Option<OrchestrationV2PendingRuntimeRequestSummary>,
    pub limit_recovery: Option<OrchestrationV2LimitRecovery>,
    pub snoozed_until: Option<IsoDateTime>,
}

pub fn limit_recovery_command(
    thread: &LimitRecoveryCandidate,
    auto_resume: bool,
    now_ms: i64,
    snooze: bool,
) -> Result<Option<OrchestrationV2Command>> {
    if thread.status != OrchestrationV2ShellThreadStatus::Failed
        || thread.last_error_class != Some(OrchestrationV2ProviderFailureClass::UsageLimit)
        || thread.archived_at.is_some()
        || thread.settled_override == Some(AppThreadSettledOverride::Settled)
        || thread.pending_runtime_request.is_some()
    {
        return Ok(None);
    }
    let (Some(run_id), Some(reset_at)) = (&thread.latest_run_id, &thread.usage_limit_reset_at)
    else {
        return Ok(None);
    };
    let Some(reset_ms) = epoch_millis(reset_at) else {
        return Ok(None);
    };
    let failed_at = thread
        .latest_run_completed_at
        .as_ref()
        .unwrap_or(&thread.updated_at);
    if epoch_millis(&failed_at.0).is_none_or(|failed| reset_ms <= failed) {
        return Ok(None);
    }
    let identity = format!("{}:{}:{reset_ms}", thread.id.0, run_id.0);
    let recovery = thread
        .limit_recovery
        .as_ref()
        .filter(|r| &r.run_id == run_id && r.reset_at.0 == *reset_at);
    let Some(recovery) = recovery else {
        if now_ms.saturating_sub(reset_ms) > MAX_OVERDUE_ARM_MS
            || (!auto_resume && (!snooze || reset_ms <= now_ms))
        {
            return Ok(None);
        }
        return Ok(Some(serde_json::from_value(json!({
            "type":"thread.metadata.update", "commandId":format!("limit-arm:{identity}"),
            "threadId":thread.id, "limitRecovery": {
                "runId":run_id,"resetAt":reset_at,"autoResume":auto_resume,"snooze":snooze && reset_ms > now_ms
            }
        }))?));
    };
    if !recovery.auto_resume
        || reset_ms > now_ms
        || thread
            .snoozed_until
            .as_ref()
            .and_then(|v| epoch_millis(&v.0))
            .is_some_and(|t| t > now_ms)
    {
        return Ok(None);
    }
    let request = recovery.request_id.as_ref().and_then(|v| v.as_ref());
    let identity = format!(
        "{identity}:{}",
        request.map(|v| v.0.as_str()).unwrap_or("legacy")
    );
    let mut command = json!({
        "type":"message.dispatch", "commandId":format!("limit-resume:{identity}"),
        "messageId":format!("limit-resume:{identity}"), "threadId":thread.id,
        "usageLimitContinuationOfRunId":run_id,"text":"Continue where you left off.",
        "attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"agent","creationSource":"server"
    });
    if let Some(request) = request {
        command["usageLimitRecoveryRequestId"] = json!(request);
    }
    Ok(Some(serde_json::from_value(command)?))
}

pub fn affects_limit_recovery(event: &OrchestrationV2DomainEvent) -> bool {
    use OrchestrationV2DomainEvent::*;
    match event {
        ThreadCreated(_)
        | ThreadArchived(_)
        | ThreadUnarchived(_)
        | ThreadDeleted(_)
        | ThreadSettled(_)
        | ThreadUnsettled(_)
        | ThreadSnoozed(_)
        | ThreadUnsnoozed(_)
        | ThreadPinned(_)
        | ThreadAutoSettleSet(_)
        | ThreadUnpinned(_)
        | ThreadPinReordered(_)
        | ThreadActiveReordered(_)
        | ThreadTagsUpdated(_)
        | ThreadVisited(_)
        | ThreadMarkedUnread(_)
        | ThreadMetadataUpdated(_)
        | ThreadPullRequestSynced(_)
        | ThreadRuntimeModeUpdated(_)
        | ThreadInteractionModeUpdated(_)
        | ThreadModelSelectionUpdated(_)
        | ThreadProviderSwitched(_)
        | ThreadGuestShareUpdated(_)
        | ThreadPullRequestLinkUpdated(_)
        | RunCreated(_)
        | RunUpdated(_)
        | RuntimeRequestUpdated(_)
        | ProviderSessionAttached(_)
        | ProviderSessionUpdated(_)
        | ProviderSessionDetached(_) => true,
        TurnItemUpdated(event) => matches!(event.payload, OrchestrationV2TurnItem::Error(_)),
        _ => false,
    }
}

#[derive(Default)]
struct DueState {
    version: u64,
    wake_at: Option<i64>,
}

#[derive(Default)]
pub struct UsageLimitRecoveryWake(Mutex<DueState>);
impl UsageLimitRecoveryWake {
    pub fn invalidate(&self) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.version = state.version.wrapping_add(1);
        state.wake_at = None;
    }
    pub fn on_event(&self, event: &OrchestrationV2DomainEvent) {
        if affects_limit_recovery(event) {
            self.invalidate();
        }
    }
    pub fn begin_sweep(&self, now_ms: i64) -> Option<u64> {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        (!state.wake_at.is_some_and(|wake| now_ms < wake)).then_some(state.version)
    }
    pub fn record_idle(&self, version: u64, wake_at: Option<i64>) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.version == version {
            state.wake_at = Some(wake_at.unwrap_or(i64::MAX));
        }
    }
}

#[derive(Clone, Copy)]
pub struct LimitRecoveryPreferences {
    pub auto_resume: bool,
    pub snooze: bool,
}

#[async_trait]
pub trait LimitRecoveryState: Send + Sync {
    async fn preferences(&self) -> Result<LimitRecoveryPreferences>;
    async fn candidates(
        &self,
        now: IsoDateTime,
        preferences: LimitRecoveryPreferences,
    ) -> Result<Vec<LimitRecoveryCandidate>>;
    async fn wake_at(&self, now: IsoDateTime) -> Result<Option<i64>>;
    async fn auto_arm(&self, command: CommandThreadMetadataUpdate) -> Result<()>;
}

#[async_trait]
pub trait LimitRecoveryArmer: Send + Sync {
    /// Rechecks the current failed run and recovery identity before committing metadata.
    async fn auto_arm(&self, command: CommandThreadMetadataUpdate) -> Result<()>;
}

pub struct GuardedLimitRecoveryArmer {
    committer: Arc<dyn EventCommitter>,
    engine: Arc<dyn DecisionEngine>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl GuardedLimitRecoveryArmer {
    pub fn new(
        committer: Arc<dyn EventCommitter>,
        engine: Arc<dyn DecisionEngine>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        Self {
            committer,
            engine,
            clock,
            ids,
        }
    }
}

#[async_trait]
impl LimitRecoveryArmer for GuardedLimitRecoveryArmer {
    async fn auto_arm(&self, mut command: CommandThreadMetadataUpdate) -> Result<()> {
        let value = t3_contracts::stored::to_value(&command)?;
        if command
            .limit_recovery
            .as_ref()
            .and_then(Option::as_ref)
            .is_none()
            || value.as_object().is_none_or(|object| {
                object
                    .keys()
                    .any(|key| !matches!(key.as_str(), "commandId" | "threadId" | "limitRecovery"))
            })
        {
            return Err(Error::Decode {
                entity: "automatic limit recovery",
                detail: "requires only a non-null limitRecovery metadata update".into(),
            });
        }
        let now = self.clock.now();
        let engine = self.engine.clone();
        let ids = self.ids.clone();
        let already_current = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let skipped = already_current.clone();
        let receipt = crate::stores::receipts::CommandReceipt {
            command_id: command.command_id.clone(),
            aggregate: crate::stores::receipts::AggregateId::Thread(command.thread_id.clone()),
            command_type: "thread.metadata.update".into(),
            accepted_at: now.0.clone(),
            result_sequence: 0,
            status: crate::stores::receipts::ReceiptStatus::Accepted,
            error: None,
        };
        let result = self.committer.commit_prepared(
            CommitRequest { receipt: Some(receipt), decision: Decision::default(), guard: CommitGuard::None },
            CommitOptions { command_id: Some(command.command_id.clone()), ..Default::default() },
            Box::new(move |tx, mut request| {
                let thread = crate::projections::read::require_thread(tx, &command.thread_id)?;
                let requested = command.limit_recovery.as_mut().and_then(Option::as_mut).ok_or(Error::Stale)?;
                if thread.limit_recovery.as_ref().and_then(Option::as_ref).is_some_and(|recovery| recovery.run_id == requested.run_id && recovery.reset_at == requested.reset_at) {
                    // Shim: preparation cannot skip a receipt; roll back this known no-op atomically.
                    skipped.store(true, std::sync::atomic::Ordering::Release);
                    return Err(Error::Stale);
                }
                // The writer transaction serializes this recheck with Stop and explicit recovery choices.
                let stopped: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 AND run_id=?2 AND type='run_interrupt_request')",
                    [&command.thread_id.0, &requested.run_id.0], |row| row.get(0))?;
                if stopped { requested.auto_resume = Some(Some(false)); requested.snooze = Some(Some(false)); }
                let server_command = OrchestrationV2ServerCommand::Client(Box::new(OrchestrationV2Command::ThreadMetadataUpdate(Box::new(command.clone()))));
                let (fields, filter) = crate::decision::engine::required_records(&server_command)?;
                let projection = crate::runtime::dispatch::projection_from_records(crate::projections::read::query_records(tx, &ProjectionRecordQuery {
                    thread_id: command.thread_id.clone(), fields, filter, messages: MessageFilter::default(), run_statuses: None, include_latest_run: true,
                })?);
                request.decision = engine.decide(&DecisionContext {
                    projection: Some(projection), now,
                    ids: (0..8).map(|_| (IdKind::Event, ids.next(IdKind::Event))).collect(), capabilities: None,
                }, &server_command)?;
                Ok(request)
            }),
        ).await;
        match result {
            Err(Error::Stale) if already_current.load(std::sync::atomic::Ordering::Acquire) => {
                Ok(())
            }
            result => result.map(|_| ()),
        }
    }
}

pub struct ProjectionLimitRecoveryState {
    projections: ProjectionStore,
    settings: Arc<dyn SettingsService>,
    armer: Arc<dyn LimitRecoveryArmer>,
}

impl ProjectionLimitRecoveryState {
    pub fn new(
        projections: ProjectionStore,
        settings: Arc<dyn SettingsService>,
        armer: Arc<dyn LimitRecoveryArmer>,
    ) -> Self {
        Self {
            projections,
            settings,
            armer,
        }
    }
}

#[async_trait]
impl LimitRecoveryState for ProjectionLimitRecoveryState {
    async fn preferences(&self) -> Result<LimitRecoveryPreferences> {
        let settings = self.settings.get().await?;
        Ok(LimitRecoveryPreferences {
            auto_resume: settings.auto_resume_limited_threads,
            snooze: settings.snooze_limited_threads,
        })
    }
    async fn candidates(
        &self,
        now: IsoDateTime,
        preferences: LimitRecoveryPreferences,
    ) -> Result<Vec<LimitRecoveryCandidate>> {
        Ok(self
            .projections
            .limit_recovery_candidates(now, preferences.auto_resume, preferences.snooze)
            .await?
            .into_iter()
            .map(|candidate| LimitRecoveryCandidate {
                id: candidate.id,
                status: candidate.status,
                last_error_class: candidate.last_error_class,
                latest_run_id: Some(candidate.latest_run_id),
                usage_limit_reset_at: candidate.usage_limit_reset_at.map(|date| date.0),
                latest_run_completed_at: candidate.latest_run_completed_at,
                updated_at: candidate.updated_at,
                archived_at: candidate.archived_at,
                settled_override: candidate.settled_override,
                pending_runtime_request: candidate.pending_runtime_request,
                limit_recovery: candidate.limit_recovery,
                snoozed_until: candidate.snoozed_until,
            })
            .collect())
    }
    async fn wake_at(&self, now: IsoDateTime) -> Result<Option<i64>> {
        self.projections.limit_recovery_wake_at(now).await
    }
    async fn auto_arm(&self, command: CommandThreadMetadataUpdate) -> Result<()> {
        self.armer.auto_arm(command).await
    }
}

#[derive(Debug, Default)]
pub struct LimitRecoverySweep {
    pub acted: usize,
    pub failures: Vec<(ThreadId, String)>,
}

pub struct UsageLimitRecoveryWorker {
    state: Arc<dyn LimitRecoveryState>,
    dispatcher: Arc<dyn CommandDispatcher>,
    clock: Arc<dyn Clock>,
    pub wake: UsageLimitRecoveryWake,
}
impl UsageLimitRecoveryWorker {
    pub fn new(
        state: Arc<dyn LimitRecoveryState>,
        dispatcher: Arc<dyn CommandDispatcher>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            state,
            dispatcher,
            clock,
            wake: UsageLimitRecoveryWake::default(),
        }
    }
    pub async fn sweep(&self) -> Result<LimitRecoverySweep> {
        let now = self.clock.now();
        let now_ms = epoch_millis(&now.0).ok_or_else(|| Error::Decode {
            entity: "usage-limit clock",
            detail: now.0.clone(),
        })?;
        let Some(version) = self.wake.begin_sweep(now_ms) else {
            return Ok(LimitRecoverySweep::default());
        };
        let preferences = self.state.preferences().await?;
        let candidates = self.state.candidates(now.clone(), preferences).await?;
        let mut report = LimitRecoverySweep::default();
        for thread in candidates {
            let Some(command) = limit_recovery_command(
                &thread,
                preferences.auto_resume,
                now_ms,
                preferences.snooze,
            )?
            else {
                continue;
            };
            report.acted += 1;
            let result = match command {
                OrchestrationV2Command::ThreadMetadataUpdate(command) => {
                    self.state.auto_arm(*command).await
                }
                command => self
                    .dispatcher
                    .dispatch(OrchestrationV2ServerCommand::Client(Box::new(command)))
                    .await
                    .map(|_| ()),
            };
            if let Err(error) = result {
                report.failures.push((thread.id, error.to_string()));
            }
        }
        if report.acted == 0 {
            self.wake
                .record_idle(version, self.state.wake_at(now).await?);
        }
        Ok(report)
    }
}

pub(crate) fn epoch_millis(value: &str) -> Option<i64> {
    let (date, time) = value.split_once('T')?;
    let (year, rest) = date.rsplit_once('-')?;
    let day: i64 = rest.parse().ok()?;
    let (year, month) = year.rsplit_once('-')?;
    let mut year: i64 = year.parse().ok()?;
    if !(-271_821..=275_760).contains(&year) {
        return None;
    }
    let month: i64 = month.parse().ok()?;
    let (time, offset) = if let Some(time) = time.strip_suffix('Z') {
        (time, 0)
    } else {
        let index = time.find(['+', '-'])?;
        let (time, zone) = time.split_at(index);
        let (h, m) = zone.get(1..)?.split_once(':')?;
        let h: i64 = h.parse().ok()?;
        let m: i64 = m.parse().ok()?;
        if !(0..=23).contains(&h) || !(0..=59).contains(&m) {
            return None;
        }
        (
            time,
            (h * 60 + m) * if zone.starts_with('-') { -1 } else { 1 },
        )
    };
    let mut parts = time.split(':');
    let hour: i64 = parts.next()?.parse().ok()?;
    let minute: i64 = parts.next()?.parse().ok()?;
    let seconds = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let (seconds, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let seconds: i64 = seconds.parse().ok()?;
    if !fraction.bytes().all(|v| v.is_ascii_digit()) {
        return None;
    }
    let millis: i64 = format!("{fraction:0<3}").get(..3)?.parse().ok()?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return None,
    };
    if !(1..=days_in_month).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&seconds)
    {
        return None;
    }
    year -= i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let days = era
        .checked_mul(146097)?
        .checked_add(yoe * 365 + yoe / 4 - yoe / 100 + doy)?
        .checked_sub(719468)?;
    days.checked_mul(86400000)?
        .checked_add((hour * 3600 + minute * 60 + seconds - offset * 60) * 1000 + millis)
        .filter(|value| value.abs() <= 8_640_000_000_000_000)
}
