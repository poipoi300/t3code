use super::{
    execution::work_active, subagent_runtime::is_terminal_subagent_status,
    workflows::resolve_active_thread_run,
};
use t3_contracts::{
    base_schemas::RunId, orchestration_v2::*, orchestration_v2_id_table::IdOrIndex,
    wire::WireDateTime,
};

pub struct ThreadTurnSubagents<'a> {
    pub run_id: Option<&'a IdOrIndex<RunId>>,
    pub turn_active: bool,
    pub latest_turn: bool,
    pub subagents: Vec<&'a OrchestrationV2Subagent>,
    pub live_count: usize,
    pub settled_count: usize,
}
pub fn derive_thread_turn_subagents<'a>(
    p: &'a OrchestrationV2ThreadProjection,
    millis: impl Fn(&WireDateTime) -> i64,
) -> Option<ThreadTurnSubagents<'a>> {
    let eligible: Vec<_> = p
        .subagents
        .iter()
        .filter(|a| {
            !p.runs.iter().any(|r| {
                Some(&r.id) == a.run_id.as_ref() && r.status == OrchestrationV2RunStatus::RolledBack
            })
        })
        .collect();
    let mut latest = *eligible.first()?;
    for &agent in &eligible {
        if millis(&agent.updated_at) > millis(&latest.updated_at) {
            latest = agent;
        }
    }
    let active = resolve_active_thread_run(p);
    let started = p
        .runs
        .iter()
        .filter(|r| r.started_at.is_some() && r.status != OrchestrationV2RunStatus::RolledBack)
        .max_by_key(|r| r.ordinal);
    let run_id = active
        .map(|r| &r.id)
        .or_else(|| {
            started
                .filter(|r| eligible.iter().any(|a| a.run_id.as_ref() == Some(&r.id)))
                .map(|r| &r.id)
        })
        .or(latest.run_id.as_ref());
    let mut subagents: Vec<_> = eligible
        .into_iter()
        .filter(|a| {
            a.run_id.as_ref() == run_id
                || work_active(&a.status)
                || a.workflow.as_ref().is_some_and(Option::is_some)
        })
        .collect();
    subagents.sort_by(|a, b| {
        millis(a.started_at.as_ref().unwrap_or(&a.updated_at))
            .cmp(&millis(b.started_at.as_ref().unwrap_or(&b.updated_at)))
            .then_with(|| super::execution::id_text(&a.id).cmp(&super::execution::id_text(&b.id)))
    });
    if subagents.is_empty() {
        return None;
    }
    Some(ThreadTurnSubagents {
        run_id,
        turn_active: active.is_some(),
        latest_turn: run_id.is_none() || started.is_none_or(|r| Some(&r.id) == run_id),
        live_count: subagents.iter().filter(|a| work_active(&a.status)).count(),
        settled_count: subagents
            .iter()
            .filter(|a| is_terminal_subagent_status(&a.status))
            .count(),
        subagents,
    })
}
pub fn resolve_subagent_pill_segment(
    turn: Option<&ThreadTurnSubagents<'_>>,
) -> Option<(String, String)> {
    let turn = turn?;
    if !turn.turn_active
        && !turn.latest_turn
        && turn.live_count == 0
        && !turn
            .subagents
            .iter()
            .any(|a| a.workflow.as_ref().is_some_and(Option::is_some))
    {
        return None;
    }
    Some(if turn.live_count > 0 {
        (
            format!("{}/{}", turn.live_count, turn.subagents.len()),
            format!(
                "{} of {} agents working",
                turn.live_count,
                turn.subagents.len()
            ),
        )
    } else {
        (
            format!("{} done", turn.settled_count),
            format!(
                "{} agent{} done",
                turn.settled_count,
                if turn.settled_count == 1 { "" } else { "s" }
            ),
        )
    })
}
