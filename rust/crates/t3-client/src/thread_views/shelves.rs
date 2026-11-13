//! Shelf policy accepts parsed epoch timestamps; time and rendering remain caller-owned.
use crate::types::ThreadKey;
pub use crate::view_logic::shell::sidebar::snooze::{
    Calendar as SnoozeCalendar, PickDecision, Preset as SnoozePreset,
    pick_preset as pick_snooze_preset, preset_id as snooze_preset_id,
    resolve_preset as resolve_snooze_preset, resolve_presets as resolve_snooze_presets,
    wake_label as snooze_wake_label,
};
use std::collections::BTreeMap;

pub fn resolve_custom_snooze_duration(amount: &str, unit_ms: u64, now: i64) -> Option<i64> {
    let amount = t3_contracts::wire::trim_js(amount).parse::<f64>().ok()?;
    let wake = now as f64 + amount * unit_ms as f64;
    if amount.is_finite()
        && amount > 0.0
        && wake.is_finite()
        && wake.abs() <= 8_640_000_000_000_000.0
        && wake.trunc() > now as f64
    {
        Some(wake.trunc() as i64)
    } else {
        None
    }
}

pub fn resolve_custom_snooze_date(
    date: &str,
    time: &str,
    now: i64,
    local_epoch: impl Fn(&str, &str) -> Option<i64>,
    local_date_time: impl Fn(i64) -> (String, String),
) -> Option<i64> {
    let shaped = |s: &str, separators: &[(usize, u8)], len| {
        s.len() == len
            && s.bytes().enumerate().all(|(i, b)| {
                separators
                    .iter()
                    .find(|(index, _)| *index == i)
                    .map_or_else(|| b.is_ascii_digit(), |(_, expected)| b == *expected)
            })
    };
    if !shaped(date, &[(4, b'-'), (7, b'-')], 10) || !shaped(time, &[(2, b':')], 5) {
        return None;
    }
    let wake = local_epoch(date, time)?;
    (wake > now && local_date_time(wake) == (date.into(), time.into())).then_some(wake)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Preparing,
    Queued,
    Starting,
    Running,
    Waiting,
    Completed,
    Interrupted,
    Failed,
    Cancelled,
    RolledBack,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeState {
    Preparing,
    Queued,
    Starting,
    Running,
    Waiting,
    Idle,
    Error,
    Failed,
    Other,
}
#[derive(Debug, Clone, Default)]
pub struct Run {
    pub state: Option<RunState>,
    pub requested_at: Option<i64>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub is_runtime_active_run: bool,
}
#[derive(Debug, Clone, Default)]
pub struct ShelfInput {
    pub runtime: Option<RuntimeState>,
    pub runtime_updated_at: Option<i64>,
    pub legacy_session_error: bool,
    pub latest_run: Option<Run>,
    pub latest_user_message_at: Option<i64>,
    pub approvals: bool,
    pub questions: bool,
    pub pending_non_question_request: bool,
    pub question_at: Option<i64>,
    pub subagent_request: bool,
    pub guest_messages: usize,
    pub snoozed_at: Option<i64>,
    pub snoozed_until: Option<i64>,
    pub plan_mode: bool,
    pub actionable_plan: bool,
}
pub const QUEUED_START_GRACE_MS: i64 = 120_000;
pub fn has_queued_turn_start(shell: &ShelfInput, now: i64) -> bool {
    if matches!(
        shell.runtime,
        Some(RuntimeState::Preparing | RuntimeState::Queued | RuntimeState::Starting)
    ) {
        return true;
    }
    let Some(message) = shell.latest_user_message_at else {
        return false;
    };
    if shell.legacy_session_error || now.abs_diff(message) > QUEUED_START_GRACE_MS as u64 {
        return false;
    }
    shell.latest_run.as_ref().is_none_or(|r| {
        [r.requested_at, r.started_at, r.completed_at]
            .iter()
            .all(|t| t.is_none_or(|t| t < message))
    })
}
fn failure_edge(shell: &ShelfInput) -> Option<Option<i64>> {
    if !matches!(
        shell.runtime,
        Some(RuntimeState::Error | RuntimeState::Failed)
    ) {
        return None;
    }
    Some(
        if let Some(run) = shell
            .latest_run
            .as_ref()
            .filter(|r| r.state == Some(RunState::Failed))
        {
            run.completed_at
        } else {
            shell.runtime_updated_at
        },
    )
}
pub fn raised_hand_while_snoozed(shell: &ShelfInput) -> bool {
    if shell.approvals || shell.pending_non_question_request {
        return true;
    }
    if shell.questions
        && (shell.snoozed_at.is_none()
            || shell.question_at.is_none()
            || shell.question_at > shell.snoozed_at)
    {
        return true;
    }
    if shell.subagent_request || shell.guest_messages > 0 {
        return true;
    }
    failure_edge(shell).is_some_and(|failed| {
        shell.snoozed_at.is_none() || failed.zip(shell.snoozed_at).is_some_and(|(f, s)| f > s)
    })
}
pub fn can_snooze(shell: &ShelfInput, now: i64) -> bool {
    !shell.approvals
        && !shell.pending_non_question_request
        && !shell.subagent_request
        && shell.guest_messages == 0
        && !has_queued_turn_start(shell, now)
}
pub fn effective_snoozed(shell: &ShelfInput, now: i64) -> bool {
    shell.snoozed_until.is_some_and(|wake| wake > now) && !raised_hand_while_snoozed(shell)
}
pub fn woke_at(shell: &ShelfInput, now: i64) -> Option<i64> {
    let wake = shell.snoozed_until?;
    if raised_hand_while_snoozed(shell) {
        let failure = failure_edge(shell).flatten();
        let fresh_failure = failure.filter(|f| shell.snoozed_at.is_none_or(|s| *f > s));
        let fresh_question = shell
            .question_at
            .filter(|q| shell.snoozed_at.is_some_and(|s| *q > s));
        if let Some(f) = fresh_failure
            && fresh_question.is_none_or(|q| f >= q)
        {
            return Some(f);
        }
        return fresh_question
            .or(shell.runtime_updated_at)
            .or(shell.snoozed_at);
    }
    (wake <= now).then_some(wake)
}
pub fn is_woke_unseen(
    woke: Option<i64>,
    settled: bool,
    last_visited: Option<i64>,
    completed: Option<i64>,
) -> bool {
    woke.is_some_and(|w| !settled && [last_visited, completed].iter().flatten().all(|s| *s < w))
}
pub fn is_working(shell: &ShelfInput) -> bool {
    if shell.approvals || shell.questions || shell.subagent_request {
        return false;
    }
    if !matches!(
        shell.runtime,
        Some(
            RuntimeState::Preparing
                | RuntimeState::Queued
                | RuntimeState::Starting
                | RuntimeState::Running
                | RuntimeState::Waiting
                | RuntimeState::Idle
        )
    ) {
        return false;
    }
    let settled = shell.latest_run.as_ref().is_some_and(|r| {
        !matches!(
            r.state,
            Some(
                RunState::Preparing
                    | RunState::Queued
                    | RunState::Starting
                    | RunState::Running
                    | RunState::Waiting
            )
        ) && !r.is_runtime_active_run
    });
    !(shell.plan_mode && shell.actionable_plan && settled)
}
#[derive(Debug, Clone)]
pub struct ShelfRow {
    pub key: ThreadKey,
    pub created_at: Option<i64>,
    pub unsettled_at: Option<i64>,
    pub run_requested_at: Option<i64>,
    pub run_completed_at: Option<i64>,
    pub user_authored_at: Option<Option<i64>>,
}
pub fn sort_inbox(rows: &mut [ShelfRow], returned: &BTreeMap<ThreadKey, i64>) {
    let stamp = |r: &ShelfRow| {
        [
            r.created_at,
            r.unsettled_at,
            r.run_requested_at,
            r.run_completed_at,
            returned.get(&r.key).copied(),
        ]
        .into_iter()
        .flatten()
        .fold(0, i64::max)
    };
    rows.sort_by(|a, b| {
        stamp(b)
            .cmp(&stamp(a))
            .then_with(|| a.key.thread.cmp(&b.key.thread))
            .then_with(|| a.key.environment.cmp(&b.key.environment))
    });
}
pub fn sort_working(rows: &mut [ShelfRow]) {
    let stamp = |r: &ShelfRow| {
        r.created_at.unwrap_or(0).max(
            r.user_authored_at
                .unwrap_or(r.run_requested_at)
                .unwrap_or(0),
        )
    };
    rows.sort_by(|a, b| {
        stamp(b)
            .cmp(&stamp(a))
            .then_with(|| a.key.thread.cmp(&b.key.thread))
            .then_with(|| a.key.environment.cmp(&b.key.environment))
    });
}

pub const WORKING_EXIT_GRACE_MS: i64 = 2_000;
pub const STALE_EXIT_MS: i64 = 60_000;
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InboxPlacement {
    pub archived_at: Option<String>,
    pub deleted_at: Option<String>,
    pub pinned_at: Option<String>,
    pub settled_override: Option<String>,
    pub settled_at: Option<String>,
    pub snoozed_at: Option<String>,
    pub snoozed_until: Option<String>,
    pub active_order_key: Option<String>,
}
pub struct InboxTrackedThread<'a> {
    pub key: &'a ThreadKey,
    pub shelf: &'a ShelfInput,
    pub placement: InboxPlacement,
    pub has_runtime_error: bool,
}
#[derive(Debug, Clone)]
struct InboxState {
    working: bool,
    deadline: Option<i64>,
    placement: Option<InboxPlacement>,
}
#[derive(Debug, Clone, Default)]
pub struct InboxObservation {
    working: BTreeMap<ThreadKey, bool>,
    returns: BTreeMap<ThreadKey, i64>,
}
impl InboxObservation {
    pub fn is_working(&self, key: &ThreadKey, shelf: &ShelfInput) -> bool {
        self.working
            .get(key)
            .copied()
            .unwrap_or_else(|| is_working(shelf))
    }
    pub fn returned_at(&self, key: &ThreadKey) -> Option<i64> {
        self.returns.get(key).copied()
    }
}
#[derive(Default)]
pub struct InboxReturnTracker {
    states: BTreeMap<ThreadKey, InboxState>,
    returns: BTreeMap<ThreadKey, i64>,
    pub revision: u64,
}
impl InboxReturnTracker {
    pub fn deadline(&self) -> Option<i64> {
        self.states.values().filter_map(|s| s.deadline).min()
    }
    pub fn observe(
        &mut self,
        threads: Option<&[InboxTrackedThread<'_>]>,
        now: i64,
    ) -> InboxObservation {
        let Some(threads) = threads else {
            self.states.clear();
            self.returns.clear();
            return InboxObservation::default();
        };
        let mut present = std::collections::BTreeSet::new();
        for thread in threads {
            present.insert(thread.key);
            let previous = self.states.get(thread.key);
            let actual = is_working(thread.shelf);
            let placement =
                (actual || previous.is_some_and(|p| p.working)).then(|| thread.placement.clone());
            let actionable = thread.shelf.approvals
                || thread.shelf.questions
                || thread.shelf.subagent_request
                || thread.shelf.guest_messages > 0
                || (thread.shelf.plan_mode && thread.shelf.actionable_plan)
                || thread.has_runtime_error
                || thread
                    .shelf
                    .latest_run
                    .as_ref()
                    .is_some_and(|r| r.state == Some(RunState::Failed));
            let moved = previous.is_some_and(|p| p.placement != placement);
            let excluded = thread.placement.archived_at.is_some()
                || thread.placement.deleted_at.is_some()
                || thread.placement.pinned_at.is_some()
                || thread.placement.settled_override.as_deref() == Some("settled")
                || effective_snoozed(thread.shelf, now);
            let stale = thread
                .shelf
                .latest_run
                .as_ref()
                .and_then(|r| r.completed_at)
                .is_some_and(|end| end <= now.saturating_sub(STALE_EXIT_MS));
            let deadline = if !actual
                && previous.is_some_and(|p| p.working)
                && !actionable
                && !moved
                && !excluded
                && !stale
            {
                Some(
                    previous
                        .and_then(|p| p.deadline)
                        .unwrap_or_else(|| now.saturating_add(WORKING_EXIT_GRACE_MS)),
                )
            } else {
                None
            };
            let working = actual || deadline.is_some_and(|d| d > now);
            if previous.is_some_and(|p| p.working) && !working {
                self.returns.insert(thread.key.clone(), now);
            }
            self.states.insert(
                thread.key.clone(),
                InboxState {
                    working,
                    deadline: deadline.filter(|_| working),
                    placement,
                },
            );
        }
        self.states.retain(|key, _| present.contains(key));
        self.returns.retain(|key, _| present.contains(key));
        InboxObservation {
            working: self
                .states
                .iter()
                .map(|(key, s)| (key.clone(), s.working))
                .collect(),
            returns: self.returns.clone(),
        }
    }
    pub fn tick(&mut self, now: i64) -> bool {
        let mut expired = false;
        for (key, state) in &mut self.states {
            if state.deadline.is_some_and(|d| d <= now) {
                state.working = false;
                state.deadline = None;
                self.returns.insert(key.clone(), now);
                expired = true;
            }
        }
        if expired {
            self.revision += 1;
        }
        expired
    }
    pub fn is_working(&self, key: &ThreadKey, shelf: &ShelfInput) -> bool {
        self.states
            .get(key)
            .map_or_else(|| is_working(shelf), |s| s.working)
    }
    pub fn returned_at(&self, key: &ThreadKey) -> Option<i64> {
        self.returns.get(key).copied()
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn pending_start_clock_skew_and_adoption() {
        let mut s = ShelfInput {
            latest_user_message_at: Some(1_000),
            ..Default::default()
        };
        assert!(has_queued_turn_start(&s, 0));
        assert!(!has_queued_turn_start(&s, 121_001));
        assert!(!has_queued_turn_start(&s, -120_001));
        s.latest_run = Some(Run {
            requested_at: Some(1000),
            ..Default::default()
        });
        assert!(!has_queued_turn_start(&s, 1001));
        s.runtime = Some(RuntimeState::Preparing);
        assert!(has_queued_turn_start(&s, i64::MAX));
    }
    #[test]
    fn existing_questions_wait_but_new_requests_wake() {
        let mut s = ShelfInput {
            questions: true,
            question_at: Some(5),
            snoozed_at: Some(10),
            snoozed_until: Some(100),
            ..Default::default()
        };
        assert!(effective_snoozed(&s, 20));
        assert!(can_snooze(&s, 20));
        s.question_at = Some(15);
        assert!(!effective_snoozed(&s, 20));
        assert_eq!(woke_at(&s, 101), Some(15));
        s.approvals = true;
        assert!(!can_snooze(&s, 20));
    }
    #[test]
    fn fresh_failure_wake_does_not_jump_to_retired_timer() {
        let s = ShelfInput {
            runtime: Some(RuntimeState::Failed),
            runtime_updated_at: Some(30),
            latest_run: Some(Run {
                state: Some(RunState::Failed),
                completed_at: Some(20),
                ..Default::default()
            }),
            snoozed_at: Some(10),
            snoozed_until: Some(100),
            ..Default::default()
        };
        assert_eq!(woke_at(&s, 101), Some(20));
        assert!(!is_woke_unseen(Some(20), false, Some(21), None));
        assert!(!is_woke_unseen(Some(20), false, None, Some(20)));
        assert!(is_woke_unseen(Some(20), false, None, None));
    }
    #[test]
    fn working_ignores_attention_and_settled_plan() {
        let mut s = ShelfInput {
            runtime: Some(RuntimeState::Idle),
            ..Default::default()
        };
        assert!(is_working(&s));
        s.questions = true;
        assert!(!is_working(&s));
        s.questions = false;
        s.plan_mode = true;
        s.actionable_plan = true;
        s.latest_run = Some(Run {
            state: Some(RunState::Completed),
            ..Default::default()
        });
        assert!(!is_working(&s));
        s.latest_run.as_mut().unwrap().is_runtime_active_run = true;
        assert!(is_working(&s));
    }
}
