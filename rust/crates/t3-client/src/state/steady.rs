use crate::types::{EnvironmentId, SyncStatus};
use std::{collections::BTreeMap, rc::Rc};
use t3_contracts::orchestration_v2::OrchestrationV2ShellSnapshot;

pub const RELEASE_WINDOW_MS: u64 = 2_000;

#[derive(Debug, Clone)]
pub struct SteadyShellInput {
    pub environment: EnvironmentId,
    pub snapshot: Option<Rc<OrchestrationV2ShellSnapshot>>,
    pub server_sequence: Option<u64>,
    pub status: SyncStatus,
    pub generation: u64,
    pub connecting: bool,
    pub row_actions: u64,
}

#[derive(Debug, Clone)]
pub struct SteadyShell {
    pub shown: Option<Rc<OrchestrationV2ShellSnapshot>>,
    pub shown_sequence: Option<u64>,
    pub shown_generation: u64,
    pub following: bool,
    pub caught_up: bool,
    pub row_actions: u64,
}

#[derive(Debug, Clone, Default)]
pub struct SteadyShellsState {
    pub shells: BTreeMap<EnvironmentId, Rc<SteadyShell>>,
    pub release_at: Option<u64>,
}

fn same_snapshot(
    a: &Option<Rc<OrchestrationV2ShellSnapshot>>,
    b: &Option<Rc<OrchestrationV2ShellSnapshot>>,
) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

fn show(input: &SteadyShellInput, caught_up: bool, current: Option<&SteadyShell>) -> SteadyShell {
    SteadyShell {
        shown: input.snapshot.clone(),
        shown_sequence: input.server_sequence,
        shown_generation: input.generation,
        following: input.status == SyncStatus::Live,
        caught_up,
        row_actions: current
            .filter(|_| input.status != SyncStatus::Live)
            .map_or(input.row_actions, |current| current.row_actions),
    }
}

fn retain(current: Option<&Rc<SteadyShell>>, next: SteadyShell) -> Rc<SteadyShell> {
    if let Some(current) = current
        && same_snapshot(&current.shown, &next.shown)
        && current.shown_sequence == next.shown_sequence
        && current.shown_generation == next.shown_generation
        && current.following == next.following
        && current.caught_up == next.caught_up
        && current.row_actions == next.row_actions
    {
        return current.clone();
    }
    Rc::new(next)
}

pub fn step(
    previous: &Rc<SteadyShellsState>,
    inputs: &[SteadyShellInput],
    now: u64,
    window_ms: u64,
) -> Rc<SteadyShellsState> {
    let mut shells = BTreeMap::new();
    let mut waiting = Vec::new();
    let mut others_owe_rows = false;
    for input in inputs {
        let current = previous.shells.get(&input.environment);
        let synchronizing = input.status == SyncStatus::Synchronizing;
        let next = if let Some(current) = current {
            let new_snapshot = input.generation > current.shown_generation;
            let local = current.shown.is_none()
                || (input.server_sequence == current.shown_sequence && !new_snapshot);
            let user_acted = input.row_actions != current.row_actions;
            if local || user_acted || (input.status == SyncStatus::Live && current.following) {
                show(
                    input,
                    synchronizing && (current.caught_up || new_snapshot),
                    Some(current),
                )
            } else if !synchronizing || new_snapshot {
                waiting.push(input);
                shells.insert(input.environment.clone(), current.clone());
                continue;
            } else {
                SteadyShell {
                    following: false,
                    ..current.as_ref().clone()
                }
            }
        } else {
            show(input, false, None)
        };
        others_owe_rows |= synchronizing && input.connecting && !next.caught_up;
        shells.insert(input.environment.clone(), retain(current, next));
    }
    let mut release_at = None;
    if !waiting.is_empty() {
        let deadline = previous
            .release_at
            .unwrap_or_else(|| now.saturating_add(window_ms));
        release_at = Some(deadline);
        if !others_owe_rows || now >= deadline {
            release_at = None;
            for input in waiting {
                let next = show(
                    input,
                    input.status == SyncStatus::Synchronizing,
                    shells.get(&input.environment).map(Rc::as_ref),
                );
                shells.insert(input.environment.clone(), Rc::new(next));
            }
        }
    }
    if release_at == previous.release_at
        && shells.len() == previous.shells.len()
        && shells.iter().all(|(id, shell)| {
            previous
                .shells
                .get(id)
                .is_some_and(|old| Rc::ptr_eq(old, shell))
        })
    {
        previous.clone()
    } else {
        Rc::new(SteadyShellsState { shells, release_at })
    }
}

impl SteadyShellsState {
    pub fn holding(&self, inputs: &[SteadyShellInput]) -> bool {
        inputs.iter().any(|input| {
            !same_snapshot(
                &self
                    .shells
                    .get(&input.environment)
                    .and_then(|shell| shell.shown.clone()),
                &input.snapshot,
            )
        })
    }
}
