use crate::types::ThreadKey;
use std::cmp::Ordering;
use t3_contracts::{
    base_schemas::{MessageId, RunId},
    model_selection::ModelSelection,
    orchestration_v2::*,
    orchestration_v2_id_table::IdOrIndex,
    provider_instance::ProviderDriverKind,
    wire::WireDateTime,
};

pub fn activity_status(status: &OrchestrationV2RunStatus) -> bool {
    matches!(
        status,
        OrchestrationV2RunStatus::Preparing
            | OrchestrationV2RunStatus::Starting
            | OrchestrationV2RunStatus::Running
            | OrchestrationV2RunStatus::Waiting
    )
}
pub fn interruptible_status(status: &OrchestrationV2RunStatus) -> bool {
    matches!(
        status,
        OrchestrationV2RunStatus::Preparing
            | OrchestrationV2RunStatus::Starting
            | OrchestrationV2RunStatus::Running
    )
}
pub fn work_active(status: &OrchestrationV2TurnItemStatus) -> bool {
    matches!(
        status,
        OrchestrationV2TurnItemStatus::Pending
            | OrchestrationV2TurnItemStatus::Running
            | OrchestrationV2TurnItemStatus::Waiting
    )
}

pub fn derive_provider_extension_statuses(
    projection: Option<&OrchestrationV2ThreadProjection>,
) -> &[ProviderSessionExtensionStatusesItem] {
    let Some(p) = projection else { return &[] };
    let selected = IdOrIndex::Id(p.thread.model_selection.instance_id.clone());
    let active = p
        .provider_threads
        .iter()
        .find(|t| Some(&t.id) == p.thread.active_provider_thread_id.as_ref());
    if active.is_some_and(|t| t.provider_instance_id != selected) {
        return &[];
    }
    p.provider_sessions
        .iter()
        .rev()
        .find(|s| {
            s.provider_instance_id == selected
                && active.is_none_or(|t| t.provider_session_id.as_ref() == Some(&s.id))
        })
        .filter(|s| s.status != ProviderSessionStatus::Stopped)
        .and_then(|s| s.extension_statuses.as_ref()?.as_deref())
        .unwrap_or(&[])
}

pub fn derive_reported_model_selection(
    p: &OrchestrationV2ThreadProjection,
) -> Option<&ModelSelection> {
    p.provider_threads
        .iter()
        .find(|t| {
            Some(&t.id) == p.thread.active_provider_thread_id.as_ref()
                && t.provider_instance_id
                    == IdOrIndex::Id(p.thread.model_selection.instance_id.clone())
        })?
        .native_metadata
        .as_ref()?
        .as_ref()?
        .model_selection
        .as_ref()?
        .as_ref()
}

fn latest_matching(
    p: &OrchestrationV2ThreadProjection,
    predicate: impl Fn(&OrchestrationV2Run) -> bool,
) -> Option<&OrchestrationV2Run> {
    p.runs
        .iter()
        .filter(|r| predicate(r))
        .reduce(|a, b| if b.ordinal > a.ordinal { b } else { a })
}

pub fn latest_executed_run<'a>(
    runs: &'a [OrchestrationV2Run],
    millis: &impl Fn(&WireDateTime) -> i64,
) -> Option<&'a OrchestrationV2Run> {
    runs.iter()
        .filter(|r| {
            r.status != OrchestrationV2RunStatus::Queued
                && !(r.status == OrchestrationV2RunStatus::Cancelled && r.started_at.is_none())
        })
        .reduce(|a, b| {
            let cmp = match (&b.completed_at, &a.completed_at) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(b), Some(a)) => millis(b).cmp(&millis(a)),
            };
            if cmp.then(b.ordinal.cmp(&a.ordinal)).is_gt() {
                b
            } else {
                a
            }
        })
}

pub fn latest_root_provider_failure<'a>(
    run: Option<&OrchestrationV2Run>,
    items: &'a [OrchestrationV2TurnItem],
    millis: &impl Fn(&WireDateTime) -> i64,
) -> Option<&'a OrchestrationV2ProviderFailure> {
    let run = run.filter(|r| r.status == OrchestrationV2RunStatus::Failed)?;
    items
        .iter()
        .filter_map(|i| match i {
            OrchestrationV2TurnItem::Error(i)
                if i.status == OrchestrationV2TurnItemStatus::Failed
                    && i.run_id.as_ref() == Some(&run.id)
                    && i.node_id == run.root_node_id =>
            {
                Some(i.as_ref())
            }
            _ => None,
        })
        .max_by(|a, b| {
            millis(&a.updated_at)
                .cmp(&millis(&b.updated_at))
                .then(a.ordinal.cmp(&b.ordinal))
                .then_with(|| id_text(&a.id).cmp(&id_text(&b.id)))
        })
        .map(|i| &i.failure)
}

pub(crate) fn id_text<T: AsRef<str>>(id: &IdOrIndex<T>) -> String {
    match id {
        IdOrIndex::Id(id) => id.as_ref().into(),
        IdOrIndex::Index(index) => index.to_string(),
    }
}

fn latest_unheld_run<'a>(
    runs: &'a [OrchestrationV2Run],
    millis: &impl Fn(&WireDateTime) -> i64,
) -> Option<&'a OrchestrationV2Run> {
    runs.iter()
        .filter(|run| {
            !(run.status == OrchestrationV2RunStatus::Queued && run.queue_held == Some(Some(true)))
                && !(run.status == OrchestrationV2RunStatus::Cancelled
                    && run.started_at.is_none()
                    && run.completed_at.as_ref().is_some_and(|end| {
                        runs.iter().any(|earlier| {
                            earlier.ordinal < run.ordinal
                                && earlier.status != OrchestrationV2RunStatus::Queued
                                && earlier
                                    .completed_at
                                    .as_ref()
                                    .is_none_or(|t| millis(t) >= millis(end))
                        })
                    }))
        })
        .reduce(|a, b| if b.ordinal > a.ordinal { b } else { a })
}

fn session(p: &OrchestrationV2ThreadProjection) -> Option<&OrchestrationV2ProviderSession> {
    p.provider_sessions
        .iter()
        .rev()
        .find(|s| s.provider_instance_id == p.thread.provider_instance_id)
}

fn presented_usage_limit_run<'a>(
    p: &'a OrchestrationV2ThreadProjection,
    millis: &impl Fn(&WireDateTime) -> i64,
) -> Option<&'a OrchestrationV2Run> {
    let run = latest_executed_run(&p.runs, millis)?;
    let failure = latest_root_provider_failure(Some(run), &p.turn_items, millis)?;
    let error = session(p).and_then(|s| s.last_error.as_deref());
    (run.status == OrchestrationV2RunStatus::Failed
        && failure.class == OrchestrationV2ProviderFailureClass::UsageLimit
        && error.is_none_or(|e| e == failure.message.0)
        && p.runs.iter().any(|r| r.ordinal > run.ordinal))
    .then_some(run)
}
fn presented_latest<'a>(
    p: &'a OrchestrationV2ThreadProjection,
    millis: &impl Fn(&WireDateTime) -> i64,
) -> Option<&'a OrchestrationV2Run> {
    presented_usage_limit_run(p, millis).or_else(|| latest_unheld_run(&p.runs, millis))
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunSummary {
    pub run_id: IdOrIndex<RunId>,
    pub status: OrchestrationV2RunStatus,
    pub requested_at: WireDateTime,
    pub started_at: Option<WireDateTime>,
    pub completed_at: Option<WireDateTime>,
    pub assistant_message_id: Option<IdOrIndex<MessageId>>,
    pub turn_ended: bool,
    pub provider_starting_label: Option<String>,
    pub source_plan_ref: Option<Option<RunSourcePlanRef>>,
}
fn summarize(p: &OrchestrationV2ThreadProjection, run: &OrchestrationV2Run) -> RunSummary {
    let turn = activity_status(&run.status)
        .then(|| {
            p.provider_turns
                .iter()
                .filter(|t| {
                    run.active_attempt_id.is_some() && t.run_attempt_id == run.active_attempt_id
                })
                .max_by_key(|t| t.ordinal)
        })
        .flatten();
    let driver = p
        .provider_sessions
        .iter()
        .rev()
        .find(|s| s.provider_instance_id == run.provider_instance_id)
        .map(|s| s.driver.0.as_str());
    let name = match driver {
        Some("claudeAgent") => Some("Claude"),
        Some("codex") => Some("Codex"),
        Some("cursor") => Some("Cursor"),
        Some("grok") => Some("Grok"),
        Some("opencode") => Some("OpenCode"),
        Some("antigravity") => Some("Antigravity"),
        Some("kiro") => Some("Kiro"),
        Some("pi") => Some("Pi"),
        Some("museCode") => Some("Muse Code"),
        Some("fx") => Some("fx"),
        _ => None,
    };
    RunSummary {
        run_id: run.id.clone(),
        status: run.status.clone(),
        requested_at: run.requested_at.clone(),
        started_at: run.started_at.clone(),
        completed_at: run.completed_at.clone(),
        assistant_message_id: p
            .messages
            .iter()
            .rev()
            .find(|m| {
                m.run_id.as_ref() == Some(&run.id) && m.role == ConversationMessageRole::Assistant
            })
            .map(|m| m.id.clone()),
        turn_ended: turn.is_some_and(|t| {
            !matches!(
                t.status,
                ProviderTurnStatus::Pending | ProviderTurnStatus::Running
            )
        }),
        provider_starting_label: (turn.is_none()
            && matches!(
                run.status,
                OrchestrationV2RunStatus::Starting | OrchestrationV2RunStatus::Running
            ))
        .then(|| name.map_or_else(|| "Starting".into(), |name| format!("Starting {name}"))),
        source_plan_ref: run.source_plan_ref.clone(),
    }
}
pub fn derive_latest_thread_run(
    p: &OrchestrationV2ThreadProjection,
    millis: &impl Fn(&WireDateTime) -> i64,
) -> Option<RunSummary> {
    presented_latest(p, millis).map(|run| summarize(p, run))
}
pub fn derive_thread_activity_run(
    p: &OrchestrationV2ThreadProjection,
    millis: &impl Fn(&WireDateTime) -> i64,
) -> Option<RunSummary> {
    latest_matching(p, |r| activity_status(&r.status))
        .or_else(|| presented_latest(p, millis))
        .map(|r| summarize(p, r))
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderSubagentStatus {
    pub status: ExecutionNodeStatus,
    pub started_at: Option<WireDateTime>,
    pub completed_at: Option<WireDateTime>,
}
pub fn derive_provider_subagent_status(
    p: &OrchestrationV2ThreadProjection,
) -> Option<ProviderSubagentStatus> {
    if p.thread.creation_source != OrchestrationV2CreationSource::Provider
        || p.thread.lineage.relationship_to_parent
            != Some(AppThreadLineageRelationshipToParent::Subagent)
    {
        return None;
    }
    let n = p
        .nodes
        .iter()
        .rev()
        .find(|n| n.kind == ExecutionNodeKind::RootTurn && n.run_id.is_none())?;
    Some(ProviderSubagentStatus {
        status: n.status.clone(),
        started_at: n.started_at.clone(),
        completed_at: n.completed_at.clone(),
    })
}
pub fn derive_runless_work_started_at(p: &OrchestrationV2ThreadProjection) -> Option<WireDateTime> {
    derive_provider_subagent_status(p)
        .filter(|n| {
            matches!(
                n.status,
                ExecutionNodeStatus::Pending
                    | ExecutionNodeStatus::Running
                    | ExecutionNodeStatus::Waiting
            )
        })
        .and_then(|n| n.started_at)
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeSummary {
    pub status: Option<OrchestrationV2RunStatus>,
    pub active_run_id: Option<IdOrIndex<RunId>>,
    pub activity_started_at: Option<WireDateTime>,
    pub provider_name: Option<ProviderDriverKind>,
    pub last_error: Option<String>,
    pub last_error_class: Option<OrchestrationV2ProviderFailureClass>,
    pub last_error_code: Option<String>,
    pub usage_limit_reset_at: Option<String>,
}
pub fn derive_thread_structure_runtime(
    p: &OrchestrationV2ThreadProjection,
    millis: &impl Fn(&WireDateTime) -> i64,
) -> Option<RuntimeSummary> {
    let latest = presented_latest(p, millis);
    let holds = super::pending_work::background_work_holds_completion(
        &super::pending_work::derive_pending_background_work(p, latest),
    );
    if latest.is_none() && !holds {
        return None;
    }
    let session = session(p);
    let failure = latest_root_provider_failure(latest, &p.turn_items, millis);
    let session_error = session.and_then(|s| s.last_error.as_deref());
    let current = failure.filter(|f| session_error.is_none_or(|e| e == f.message.0));
    let active = latest_matching(p, |r| interruptible_status(&r.status));
    let activity = latest_matching(p, |r| activity_status(&r.status));
    Some(RuntimeSummary {
        status: if presented_usage_limit_run(p, millis).is_some() {
            Some(OrchestrationV2RunStatus::Failed)
        } else if holds && latest.is_none_or(|r| r.status != OrchestrationV2RunStatus::Failed) {
            None
        } else {
            activity.or(latest).map(|r| r.status.clone())
        },
        active_run_id: active.map(|r| r.id.clone()),
        activity_started_at: activity.map(|r| {
            r.work_started_at
                .as_ref()
                .and_then(Option::as_ref)
                .or(r.started_at.as_ref())
                .unwrap_or(&r.requested_at)
                .clone()
        }),
        provider_name: session.map(|s| s.driver.clone()),
        last_error: session_error
            .map(str::to_owned)
            .or_else(|| failure.map(|f| f.message.0.clone())),
        last_error_class: current.map(|f| f.class.clone()),
        last_error_code: current.and_then(|f| f.code.as_ref()).map(|c| c.0.clone()),
        usage_limit_reset_at: current
            .filter(|f| f.class == OrchestrationV2ProviderFailureClass::UsageLimit)
            .and_then(|f| f.reset_at.as_ref()?.as_ref())
            .map(|r| r.0.clone()),
    })
}
pub fn derive_resumable_run_id(
    p: &OrchestrationV2ThreadProjection,
    runtime: Option<&RuntimeSummary>,
    millis: &impl Fn(&WireDateTime) -> i64,
) -> Option<IdOrIndex<RunId>> {
    let run = latest_executed_run(&p.runs, millis)?;
    (run.status == OrchestrationV2RunStatus::Interrupted
        || (run.status == OrchestrationV2RunStatus::Failed
            && runtime.and_then(|r| r.last_error_class.clone())
                == Some(OrchestrationV2ProviderFailureClass::UsageLimit)
            && latest_root_provider_failure(Some(run), &p.turn_items, millis)
                .is_some_and(|f| f.class == OrchestrationV2ProviderFailureClass::UsageLimit)))
    .then(|| run.id.clone())
}
pub fn can_resume_thread(
    requests: &[OrchestrationV2RuntimeRequest],
    resumable: Option<&IdOrIndex<RunId>>,
    queue_held: bool,
) -> bool {
    if resumable.is_none() {
        queue_held
    } else {
        !requests
            .iter()
            .any(|r| r.status == RuntimeRequestStatus::Pending)
    }
}
pub fn thread_runtime_has_interruptible_run(runtime: Option<&RuntimeSummary>) -> bool {
    runtime.is_some_and(|r| {
        r.status
            .as_ref()
            .is_some_and(|s| activity_status(s) || *s == OrchestrationV2RunStatus::Queued)
            && r.active_run_id.is_some()
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopState {
    Stopping,
    Unresponsive,
}
#[derive(Debug, Clone, PartialEq)]
pub struct RunStopPresentation {
    pub state: StopState,
    pub run_id: IdOrIndex<RunId>,
    pub requested_at: WireDateTime,
    pub background: bool,
}
pub fn present_run_stop(p: &OrchestrationV2ThreadProjection) -> Option<RunStopPresentation> {
    let active = latest_matching(p, |r| interruptible_status(&r.status));
    let run =
        active.or_else(|| latest_matching(p, |r| r.status != OrchestrationV2RunStatus::Queued))?;
    let mut request = None;
    for item in &p.turn_items {
        if item_field!(item, run_id).as_ref() != Some(&run.id) {
            continue;
        }
        match item {
            OrchestrationV2TurnItem::RunInterruptResult(_) => return None,
            OrchestrationV2TurnItem::RunInterruptRequest(r) => request = Some(r.as_ref()),
            _ => {}
        }
    }
    let r = request?;
    if active.is_none() && r.unresponsive_at.is_none() {
        return None;
    }
    Some(RunStopPresentation {
        state: if r.unresponsive_at.is_some() {
            StopState::Unresponsive
        } else {
            StopState::Stopping
        },
        run_id: run.id.clone(),
        requested_at: r.started_at.as_ref().unwrap_or(&r.updated_at).clone(),
        background: active.is_none(),
    })
}

pub const BACKGROUND_WORK_STOP_RETRY_AFTER_MS: u64 = 10_000;
#[derive(Debug, Clone)]
pub struct BackgroundWorkStopRequest {
    pub thread: ThreadKey,
    pub task_ids: Vec<String>,
    pub retry_at: Option<u64>,
}
pub fn is_background_work_stop_pending(
    request: Option<&BackgroundWorkStopRequest>,
    thread: &ThreadKey,
    task_ids: &[&str],
    retry_expired: bool,
) -> bool {
    request.is_some_and(|r| {
        &r.thread == thread
            && !retry_expired
            && (r.retry_at.is_none()
                || task_ids
                    .iter()
                    .any(|id| r.task_ids.iter().any(|pending| pending == id)))
    })
}

pub fn limit_recovery_owns_snooze(
    run_id: Option<&IdOrIndex<RunId>>,
    reset_at: Option<&str>,
    recovery: Option<&OrchestrationV2LimitRecovery>,
    snoozed_until: Option<&str>,
    parse: impl Fn(&str) -> Option<i64>,
) -> bool {
    recovery.is_some_and(|r| {
        r.snooze == Some(Some(true))
            && run_id.is_some_and(|id| id == &IdOrIndex::Id(r.run_id.clone()))
            && reset_at.is_some_and(|reset| {
                reset == r.reset_at.0
                    && snoozed_until
                        .is_some_and(|s| parse(s).zip(parse(reset)).is_some_and(|(a, b)| a == b))
            })
    })
}
