use crate::types::{CommandId, EnvironmentId, ThreadId};
use std::collections::BTreeMap;
use t3_contracts::orchestration_v2::{OrchestrationV2ShellSnapshot, OrchestrationV2ThreadShell};

type Apply = Box<dyn Fn(&OrchestrationV2ThreadShell, bool) -> OrchestrationV2ThreadShell>;
struct PendingUpdate {
    command: CommandId,
    thread: ThreadId,
    sequence: Option<u64>,
    apply: Apply,
}

#[derive(Default)]
pub struct OptimisticThreadLifecycle {
    pending: BTreeMap<EnvironmentId, Vec<PendingUpdate>>,
}

impl OptimisticThreadLifecycle {
    pub fn begin(
        &mut self,
        environment: EnvironmentId,
        thread: ThreadId,
        command: CommandId,
        apply: impl Fn(&OrchestrationV2ThreadShell, bool) -> OrchestrationV2ThreadShell + 'static,
    ) {
        self.pending
            .entry(environment)
            .or_default()
            .push(PendingUpdate {
                command,
                thread,
                sequence: None,
                apply: Box::new(apply),
            });
    }

    pub fn accept(&mut self, environment: &EnvironmentId, command: &CommandId, sequence: u64) {
        if let Some(pending) = self
            .pending
            .get_mut(environment)
            .and_then(|updates| updates.iter_mut().find(|p| &p.command == command))
        {
            pending.sequence = Some(sequence);
        }
    }

    pub fn reject(&mut self, environment: &EnvironmentId, command: &CommandId) {
        if let Some(pending) = self.pending.get_mut(environment) {
            pending.retain(|p| &p.command != command);
        }
    }

    pub fn reconcile(
        &mut self,
        environment: &EnvironmentId,
        source: Option<&OrchestrationV2ShellSnapshot>,
    ) {
        if let Some(pending) = self.pending.get_mut(environment) {
            pending.retain(|p| {
                p.sequence
                    .is_none_or(|sequence| source.is_some_and(|s| s.snapshot_sequence < sequence))
            });
        }
    }

    pub fn pending_thread_ids(
        &self,
        environment: &EnvironmentId,
    ) -> std::collections::BTreeSet<&ThreadId> {
        self.pending
            .get(environment)
            .into_iter()
            .flatten()
            .map(|p| &p.thread)
            .collect()
    }

    pub fn snapshot(
        &self,
        environment: &EnvironmentId,
        source: &OrchestrationV2ShellSnapshot,
    ) -> Option<OrchestrationV2ShellSnapshot> {
        let pending: Vec<_> = self
            .pending
            .get(environment)?
            .iter()
            .filter(|p| p.sequence.is_none_or(|s| s > source.snapshot_sequence))
            .collect();
        if pending.is_empty() {
            return None;
        }
        let mut next = source.clone();
        for thread in &mut next.threads {
            for update in &pending {
                if update.thread == thread.id {
                    *thread = (update.apply)(thread, update.sequence.is_some());
                }
            }
        }
        Some(next)
    }
}
