use t3_contracts::{
    base_schemas::RunId, orchestration_v2::*, orchestration_v2_id_table::IdOrIndex,
    wire::WireDateTime,
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadActivityStatus {
    Working,
    Approval,
    Input,
    Idle,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadActivitySnapshot {
    pub status: ThreadActivityStatus,
    pub request_id: Option<String>,
    pub run_id: Option<IdOrIndex<RunId>>,
    pub run_status: Option<OrchestrationV2RunStatus>,
}
#[derive(Debug, Clone, PartialEq)]
pub enum TurnRun {
    Known(IdOrIndex<RunId>),
    Unknown,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadAnnouncementMemory {
    pub snapshot: ThreadActivitySnapshot,
    pub turn_run: Option<TurnRun>,
    pub settled_run_id: Option<IdOrIndex<RunId>>,
    pub idle_run_id: Option<IdOrIndex<RunId>>,
}
pub fn run_outcome_text(status: Option<&OrchestrationV2RunStatus>) -> Option<&'static str> {
    match status {
        Some(OrchestrationV2RunStatus::Completed) => Some("Done"),
        Some(OrchestrationV2RunStatus::Failed) => Some("Failed"),
        Some(OrchestrationV2RunStatus::Interrupted | OrchestrationV2RunStatus::Cancelled) => {
            Some("Stopped")
        }
        _ => None,
    }
}
pub fn is_detail_behind_shell(
    detail: Option<(&IdOrIndex<RunId>, &OrchestrationV2RunStatus)>,
    shell: Option<(&IdOrIndex<RunId>, &OrchestrationV2RunStatus)>,
) -> bool {
    shell.is_some() && detail != shell
}
pub fn announced_run<'a>(
    activity: Option<&'a OrchestrationV2Run>,
    runs: &'a [OrchestrationV2Run],
    millis: &impl Fn(&WireDateTime) -> i64,
) -> Option<&'a OrchestrationV2Run> {
    if activity
        .is_some_and(|r| r.status == OrchestrationV2RunStatus::Cancelled && r.started_at.is_none())
    {
        super::execution::latest_executed_run(runs, millis)
    } else {
        activity
    }
}
pub fn next_thread_announcement(
    previous: Option<&ThreadAnnouncementMemory>,
    next: ThreadActivitySnapshot,
) -> (ThreadAnnouncementMemory, Option<&'static str>) {
    let idle = next.status == ThreadActivityStatus::Idle;
    let starting = matches!(
        next.run_status,
        Some(
            OrchestrationV2RunStatus::Queued
                | OrchestrationV2RunStatus::Preparing
                | OrchestrationV2RunStatus::Starting
        )
    );
    let idle_run_id = if idle && !starting {
        next.run_id.clone()
    } else {
        previous.and_then(|m| m.idle_run_id.clone())
    };
    let new_run_id = (!idle
        && next
            .run_status
            .as_ref()
            .is_some_and(super::execution::activity_status)
        && next.run_id != idle_run_id)
        .then(|| next.run_id.clone())
        .flatten();
    let Some(previous) = previous else {
        return (
            ThreadAnnouncementMemory {
                turn_run: (!idle).then(|| new_run_id.map_or(TurnRun::Unknown, TurnRun::Known)),
                settled_run_id: run_outcome_text(next.run_status.as_ref()).and(next.run_id.clone()),
                idle_run_id,
                snapshot: next,
            },
            None,
        );
    };
    let mut memory = previous.clone();
    memory.snapshot = next.clone();
    memory.idle_run_id = idle_run_id;
    if !idle {
        let started = previous.turn_run.is_none()
            || new_run_id.as_ref().is_some_and(
                |id| matches!(&previous.turn_run,Some(TurnRun::Known(old)) if old!=id),
            );
        if started || previous.turn_run == Some(TurnRun::Unknown) {
            memory.turn_run = Some(new_run_id.map_or(TurnRun::Unknown, TurnRun::Known));
        }
        let announcement = match next.status {
            ThreadActivityStatus::Approval
                if previous.snapshot.status != next.status
                    || previous.snapshot.request_id != next.request_id =>
            {
                Some("Needs approval")
            }
            ThreadActivityStatus::Input
                if previous.snapshot.status != next.status
                    || previous.snapshot.request_id != next.request_id =>
            {
                Some("Needs your input")
            }
            ThreadActivityStatus::Working if started => Some("Working"),
            _ => None,
        };
        return (memory, announcement);
    }
    let outcome = run_outcome_text(next.run_status.as_ref());
    if previous.turn_run.is_none() {
        if outcome.is_some() {
            memory.settled_run_id = next.run_id;
        }
        return (memory, None);
    }
    if next.run_status == Some(OrchestrationV2RunStatus::RolledBack) {
        memory.turn_run = None;
        memory.settled_run_id = next.run_id;
        return (memory, None);
    }
    if outcome.is_none() {
        return (memory, None);
    }
    if next.run_id == previous.settled_run_id {
        memory.turn_run = None;
        return (memory, None);
    }
    if previous.turn_run == Some(TurnRun::Unknown) && next.run_id == previous.idle_run_id {
        memory.settled_run_id = next.run_id;
        return (memory, None);
    }
    memory.turn_run = None;
    memory.settled_run_id = next.run_id;
    (memory, outcome)
}
