use crate::types::{Generation, SyncAction, SyncStatus};
use std::{collections::BTreeMap, rc::Rc};
use t3_contracts::{
    base_schemas::ThreadId, orchestration_project::OrchestrationProjectShell, orchestration_v2::*,
};
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShellState {
    pub snapshot: Option<Rc<OrchestrationV2ShellSnapshot>>,
    pub status: SyncStatus,
    pub error: Option<String>,
    pub session: Generation,
    pub generation: u64,
    pub removed_at: BTreeMap<ThreadId, u64>,
    pub resync_required: bool,
}
impl ShellState {
    pub fn begin_session(&mut self, session: Generation) {
        self.session = session;
        self.status = SyncStatus::Synchronizing;
        self.error = None;
    }
    pub fn disconnect(&mut self) {
        self.status = if self.snapshot.is_some() {
            SyncStatus::Cached
        } else {
            SyncStatus::Empty
        };
    }
    pub fn resume_sequence(&self) -> Option<u64> {
        if self.resync_required {
            None
        } else {
            self.snapshot.as_ref().map(|s| s.snapshot_sequence)
        }
    }
    pub fn delivery_gap(&mut self) -> SyncAction {
        self.resync_required = true;
        self.status = SyncStatus::Synchronizing;
        SyncAction::ReloadSnapshot
    }
    pub fn install(&mut self, mut next: OrchestrationV2ShellSnapshot) {
        if let Some(previous) = &self.snapshot {
            for project in &mut next.projects {
                retain_identity(
                    previous.projects.iter().find(|p| p.id == project.id),
                    project,
                );
            }
            for row in &mut next.threads {
                if row.pull_requests.is_none()
                    && let Some(prior) = previous.threads.iter().find(|p| p.id == row.id)
                {
                    row.pull_requests = prior.pull_requests.clone();
                }
            }
        }
        self.snapshot = Some(Rc::new(next));
        self.status = SyncStatus::Synchronizing;
        self.error = None;
        self.generation += 1;
        self.removed_at.clear();
        self.resync_required = false;
    }
    pub fn apply(
        &mut self,
        session: Generation,
        item: OrchestrationV2ShellStreamItem,
    ) -> SyncAction {
        if session != self.session {
            return SyncAction::StaleSession;
        }
        use OrchestrationV2ShellStreamItem::*;
        match item {
            Snapshot(v) => {
                if let Some(roots) = v.resolved_repository_identity_roots {
                    if let Some(current) = &mut self.snapshot {
                        let changed = current.projects.iter().any(|project| {
                            v.snapshot
                                .projects
                                .iter()
                                .find(|next| {
                                    next.id == project.id
                                        && next.workspace_root == project.workspace_root
                                })
                                .is_some_and(|next| {
                                    project.repository_identity != next.repository_identity
                                        && (roots.contains(&project.workspace_root.0)
                                            || (project
                                                .repository_identity
                                                .as_ref()
                                                .and_then(Option::as_ref)
                                                .is_none()
                                                && next
                                                    .repository_identity
                                                    .as_ref()
                                                    .and_then(Option::as_ref)
                                                    .is_some()))
                                })
                        });
                        if !changed {
                            return SyncAction::None;
                        }
                        let snapshot = Rc::make_mut(current);
                        for project in &mut snapshot.projects {
                            if let Some(next) = v.snapshot.projects.iter().find(|p| {
                                p.id == project.id && p.workspace_root == project.workspace_root
                            }) && (roots.contains(&project.workspace_root.0)
                                || (project
                                    .repository_identity
                                    .as_ref()
                                    .and_then(|v| v.as_ref())
                                    .is_none()
                                    && next
                                        .repository_identity
                                        .as_ref()
                                        .and_then(|v| v.as_ref())
                                        .is_some()))
                            {
                                project.repository_identity = next.repository_identity.clone();
                            }
                        }
                    } else {
                        return SyncAction::None;
                    }
                } else {
                    self.install(v.snapshot);
                }
                SyncAction::Applied
            }
            Synchronized(_) => {
                if self.snapshot.is_some() && !self.resync_required {
                    self.status = SyncStatus::Live;
                    self.generation = self.generation.max(1);
                    SyncAction::Applied
                } else {
                    self.delivery_gap()
                }
            }
            SnapshotRequired(_) => self.delivery_gap(),
            delta => {
                if self.resync_required {
                    return SyncAction::ReloadSnapshot;
                }
                let Some(snapshot) = &mut self.snapshot else {
                    return self.delivery_gap();
                };
                let sequence = match &delta {
                    ProjectUpdated(v) => v.sequence,
                    ProjectRemoved(v) => v.sequence,
                    ThreadUpdated(v) => v.sequence,
                    ThreadRemoved(v) => v.sequence,
                    _ => return SyncAction::None,
                };
                if sequence <= snapshot.snapshot_sequence {
                    return SyncAction::Duplicate;
                }
                let snapshot = Rc::make_mut(snapshot);
                match delta {
                    ProjectUpdated(mut v) => {
                        retain_identity(
                            snapshot.projects.iter().find(|p| p.id == v.project.id),
                            &mut v.project,
                        );
                        upsert(&mut snapshot.projects, v.project, |p| &p.id);
                    }
                    ProjectRemoved(v) => snapshot.projects.retain(|p| p.id != v.project_id),
                    ThreadUpdated(v) => {
                        let id = v.thread.id.clone();
                        if v.location == ShellStreamItemThreadUpdatedLocation::Active {
                            upsert(&mut snapshot.threads, v.thread, |t| &t.id);
                            self.removed_at.remove(&id);
                        } else {
                            snapshot.threads.retain(|t| t.id != id);
                            self.removed_at.insert(id.clone(), sequence);
                        }
                        snapshot.archived_threads.retain(|t| t.id != id);
                    }
                    ThreadRemoved(v) => {
                        snapshot.threads.retain(|t| t.id != v.thread_id);
                        snapshot.archived_threads.retain(|t| t.id != v.thread_id);
                        self.removed_at.insert(v.thread_id, sequence);
                    }
                    _ => {}
                }
                snapshot.snapshot_sequence = sequence;
                SyncAction::Applied
            }
        }
    }
}
fn retain_identity(
    previous: Option<&OrchestrationProjectShell>,
    next: &mut OrchestrationProjectShell,
) {
    if next
        .repository_identity
        .as_ref()
        .and_then(|v| v.as_ref())
        .is_none()
        && let Some(previous) = previous
        && previous.workspace_root == next.workspace_root
        && previous
            .repository_identity
            .as_ref()
            .and_then(|v| v.as_ref())
            .is_some()
    {
        next.repository_identity = previous.repository_identity.clone();
    }
}
pub(super) fn upsert<T, K: PartialEq>(rows: &mut Vec<T>, item: T, key: impl Fn(&T) -> &K) {
    if let Some(index) = rows.iter().position(|v| key(v) == key(&item)) {
        rows[index] = item;
    } else {
        rows.push(item);
    }
}
