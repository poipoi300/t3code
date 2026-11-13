use super::{shell::ShellState, thread::ThreadState};
use crate::{
    store::StoreSnapshot,
    types::{EnvironmentId, SyncStatus, ThreadKey},
};
use t3_contracts::{base_schemas::ProjectId, orchestration_v2::*};
/// Stable source order; grouping and shelf policy are supplied by thread_views.
pub fn sidebar_threads<'a>(
    shell: &'a ShellState,
    project: Option<&ProjectId>,
) -> Vec<&'a OrchestrationV2ThreadShell> {
    shell
        .snapshot
        .as_ref()
        .map(|s| {
            s.threads
                .iter()
                .filter(|t| {
                    t.deleted_at.is_none()
                        && t.archived_at.is_none()
                        && project.is_none_or(|id| id == &t.project_id)
                })
                .collect()
        })
        .unwrap_or_default()
}
pub fn timeline(thread: &ThreadState) -> &[OrchestrationV2ProjectedTurnItem] {
    thread
        .projection
        .as_ref()
        .map_or(&[], |p| p.visible_turn_items.as_slice())
}

pub fn shell<'a>(
    snapshot: &'a StoreSnapshot,
    environment: &EnvironmentId,
) -> Option<&'a ShellState> {
    snapshot
        .environments
        .get(environment)
        .map(|environment| environment.shell.as_ref())
}

pub fn thread<'a>(snapshot: &'a StoreSnapshot, key: &ThreadKey) -> Option<&'a ThreadState> {
    snapshot
        .environments
        .get(&key.environment)?
        .threads
        .get(&key.thread)
        .map(std::rc::Rc::as_ref)
}

pub fn project<'a>(
    shell: &'a ShellState,
    id: &ProjectId,
) -> Option<&'a t3_contracts::orchestration_project::OrchestrationProjectShell> {
    shell
        .snapshot
        .as_ref()?
        .projects
        .iter()
        .find(|project| &project.id == id)
}

pub fn thread_shell<'a>(
    shell: &'a ShellState,
    id: &t3_contracts::base_schemas::ThreadId,
) -> Option<&'a OrchestrationV2ThreadShell> {
    shell
        .snapshot
        .as_ref()?
        .threads
        .iter()
        .find(|thread| &thread.id == id)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvironmentShellSummary {
    pub has_snapshot: bool,
    pub has_synchronizing_shell: bool,
    pub first_error: Option<String>,
}

pub fn shell_summary<'a>(
    snapshot: &StoreSnapshot,
    enabled: impl IntoIterator<Item = &'a EnvironmentId>,
) -> EnvironmentShellSummary {
    let mut summary = EnvironmentShellSummary::default();
    for environment in enabled {
        if let Some(shell) = shell(snapshot, environment) {
            summary.has_snapshot |= shell.snapshot.is_some();
            summary.has_synchronizing_shell |= shell.status == SyncStatus::Synchronizing;
            if summary.first_error.is_none() {
                summary.first_error = shell.error.clone();
            }
        }
    }
    summary
}
