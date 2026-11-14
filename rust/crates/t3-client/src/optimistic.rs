use crate::{
    operations::{CommandReceipt, CommandStatus},
    types::{CommandId, EnvironmentId, ThreadId},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectionKey {
    pub environment: EnvironmentId,
    pub thread: Option<ThreadId>,
}
impl ProjectionKey {
    pub fn shell(environment: EnvironmentId) -> Self {
        Self {
            environment,
            thread: None,
        }
    }
    pub fn thread(environment: EnvironmentId, thread: ThreadId) -> Self {
        Self {
            environment,
            thread: Some(thread),
        }
    }
}
#[derive(Debug, Clone)]
pub struct PendingIntent<T> {
    pub command: CommandId,
    pub intent: T,
    pub status: CommandStatus,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewChange<T> {
    pub apply: T,
    pub revert: T,
}
#[derive(Debug, Clone)]
struct Overlay {
    projection: ProjectionKey,
    generation: u64,
    revalidating: bool,
}
#[derive(Debug, Clone, Copy)]
struct Progress {
    generation: u64,
    sequence: u64,
}
#[derive(Debug, Clone)]
pub struct OptimisticState<T> {
    pub pending: BTreeMap<CommandId, PendingIntent<T>>,
    overlays: BTreeMap<CommandId, Overlay>,
    order: Vec<CommandId>,
    progress: BTreeMap<ProjectionKey, Progress>,
}
impl<T> Default for OptimisticState<T> {
    fn default() -> Self {
        Self {
            pending: BTreeMap::new(),
            overlays: BTreeMap::new(),
            order: Vec::new(),
            progress: BTreeMap::new(),
        }
    }
}
impl<T> OptimisticState<T> {
    pub fn insert(
        &mut self,
        command: CommandId,
        projection: ProjectionKey,
        generation: u64,
        intent: T,
    ) -> bool {
        if self.pending.contains_key(&command) {
            return false;
        }
        self.overlays.insert(
            command.clone(),
            Overlay {
                projection,
                generation,
                revalidating: false,
            },
        );
        self.order.push(command.clone());
        self.pending.insert(
            command.clone(),
            PendingIntent {
                command,
                intent,
                status: CommandStatus::Pending,
            },
        );
        true
    }
    pub fn reject(&mut self, command: &CommandId) {
        self.pending.remove(command);
        self.overlays.remove(command);
        self.order.retain(|id| id != command);
    }
    pub fn unknown(&mut self, command: &CommandId) {
        if let Some(pending) = self.pending.get_mut(command)
            && pending.status == CommandStatus::Pending
        {
            pending.status = CommandStatus::UnknownDelivery;
        }
    }
    pub fn accepted(&mut self, receipt: &CommandReceipt) -> bool {
        let Some(overlay) = self.overlays.get(&receipt.command) else {
            return false;
        };
        if overlay.projection.environment != receipt.environment || overlay.revalidating {
            return false;
        }
        let projection = overlay.projection.clone();
        let generation = overlay.generation;
        if let Some(pending) = self.pending.get_mut(&receipt.command) {
            pending.status = CommandStatus::Accepted {
                sequence: receipt.committed_sequence,
            };
        }
        if self.progress.get(&projection).is_some_and(|progress| {
            progress.generation == generation && progress.sequence >= receipt.committed_sequence
        }) {
            self.reject(&receipt.command);
        }
        true
    }
    pub fn accepted_entity(&mut self, command: &CommandId, environment: &EnvironmentId) -> bool {
        if !self.overlays.get(command).is_some_and(|overlay| {
            &overlay.projection.environment == environment && !overlay.revalidating
        }) {
            return false;
        }
        if let Some(pending) = self.pending.get_mut(command) {
            pending.status = CommandStatus::AcceptedAwaitingEntity;
        }
        true
    }
    /// Explicit evidence from a receipt lookup after an authoritative lineage reset.
    pub fn revalidated(&mut self, receipt: &CommandReceipt, generation: u64) -> bool {
        let Some(overlay) = self.overlays.get_mut(&receipt.command) else {
            return false;
        };
        if overlay.projection.environment != receipt.environment {
            return false;
        }
        let Some(progress) = self.progress.get(&overlay.projection) else {
            return false;
        };
        if progress.generation != generation {
            return false;
        }
        overlay.generation = generation;
        overlay.revalidating = false;
        self.accepted(receipt)
    }
    pub fn matching_entity(
        &mut self,
        command: &CommandId,
        projection: &ProjectionKey,
        generation: u64,
    ) -> bool {
        if !self.overlays.get(command).is_some_and(|overlay| {
            &overlay.projection == projection
                && overlay.generation == generation
                && !overlay.revalidating
        }) {
            return false;
        }
        self.reject(command);
        true
    }
    pub fn revalidated_entity(
        &mut self,
        command: &CommandId,
        projection: &ProjectionKey,
        generation: u64,
    ) -> bool {
        let valid = self
            .overlays
            .get(command)
            .is_some_and(|overlay| &overlay.projection == projection && overlay.revalidating)
            && self
                .progress
                .get(projection)
                .is_some_and(|progress| progress.generation == generation);
        if valid {
            self.reject(command);
        }
        valid
    }
    pub fn disconnect(&mut self, environment: &EnvironmentId) {
        for (command, overlay) in &self.overlays {
            if &overlay.projection.environment == environment
                && let Some(intent) = self.pending.get_mut(command)
                && intent.status == CommandStatus::Pending
            {
                intent.status = CommandStatus::UnknownDelivery;
            }
        }
    }
    /// `generation` is the projection lineage, not the socket session generation.
    pub fn observe(&mut self, projection: &ProjectionKey, generation: u64, sequence: u64) {
        if self
            .progress
            .get(projection)
            .is_some_and(|p| p.generation != generation || sequence < p.sequence)
        {
            return;
        }
        self.progress.insert(
            projection.clone(),
            Progress {
                generation,
                sequence,
            },
        );
        let confirmed: Vec<_> = self.order.iter().filter(|id| {
            self.overlays.get(*id).is_some_and(|overlay| &overlay.projection == projection && overlay.generation == generation && !overlay.revalidating)
                && self.pending.get(*id).is_some_and(|pending| matches!(pending.status, CommandStatus::Accepted { sequence: committed } if committed <= sequence))
        }).cloned().collect();
        for command in confirmed {
            self.reject(&command);
        }
    }
    /// Starts a new lineage for `projection`. Its overlays wait for revalidation, except the
    /// `unsent` commands: no request left for them, so there is nothing to be uncertain about.
    pub fn reset(
        &mut self,
        projection: &ProjectionKey,
        generation: u64,
        sequence: u64,
        unsent: &BTreeSet<CommandId>,
    ) {
        self.progress.insert(
            projection.clone(),
            Progress {
                generation,
                sequence,
            },
        );
        for (command, overlay) in &mut self.overlays {
            if &overlay.projection == projection && !unsent.contains(command) {
                overlay.revalidating = true;
                if let Some(intent) = self.pending.get_mut(command) {
                    intent.status = CommandStatus::UnknownDelivery;
                }
            }
        }
    }
    pub fn needs_revalidation(&self, command: &CommandId) -> bool {
        self.overlays
            .get(command)
            .is_some_and(|overlay| overlay.revalidating)
    }
    /// Applies view-only data in enqueue order; rejecting one intent never rewinds another.
    pub fn view<V: Clone>(
        &self,
        authoritative: &Rc<V>,
        projection: &ProjectionKey,
        mut apply: impl FnMut(&mut V, &T, &CommandStatus),
    ) -> Rc<V> {
        let mut view = authoritative.clone();
        for command in &self.order {
            if !self
                .overlays
                .get(command)
                .is_some_and(|overlay| &overlay.projection == projection && !overlay.revalidating)
            {
                continue;
            }
            if let Some(pending) = self.pending.get(command) {
                apply(Rc::make_mut(&mut view), &pending.intent, &pending.status);
            }
        }
        view
    }
    /// Legacy unscoped overlays only; production overlays reconcile through `observe`.
    pub fn reconcile(&mut self, sequence: u64) {
        self.pending.retain(|command, pending| self.overlays.contains_key(command) || !matches!(pending.status, CommandStatus::Accepted { sequence: committed } if committed <= sequence));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ThreadLifecycleAction {
    Settle,
    Unsettle,
    Snooze {
        until: t3_contracts::wire::WireDateTime,
    },
    Unsnooze,
    SetAutoSettle {
        enabled: bool,
    },
    Pin {
        order_key: Option<t3_contracts::base_schemas::TrimmedNonEmptyString>,
    },
    Unpin,
    ReorderPin {
        order_key: t3_contracts::base_schemas::TrimmedNonEmptyString,
    },
    ReorderActive {
        order_key: t3_contracts::base_schemas::TrimmedNonEmptyString,
    },
    SetModel {
        selection: t3_contracts::model_selection::ModelSelection,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadLifecycleIntent {
    pub thread: ThreadId,
    pub action: ThreadLifecycleAction,
    pub now: t3_contracts::wire::WireDateTime,
    pub now_epoch_ms: u64,
}
impl ThreadLifecycleIntent {
    pub fn apply(
        &self,
        snapshot: &mut t3_contracts::orchestration_v2::OrchestrationV2ShellSnapshot,
        status: &CommandStatus,
    ) {
        use ThreadLifecycleAction::*;
        use t3_contracts::orchestration_v2::*;
        let Some(thread) = snapshot
            .threads
            .iter_mut()
            .find(|thread| thread.id == self.thread)
        else {
            return;
        };
        let accepted = matches!(
            status,
            CommandStatus::Accepted { .. } | CommandStatus::AcceptedAwaitingEntity
        );
        match &self.action {
            Settle => {
                if !accepted
                    && (thread.pending_runtime_request.is_some()
                        || matches!(
                            thread.status,
                            OrchestrationV2ShellThreadStatus::Preparing
                                | OrchestrationV2ShellThreadStatus::Queued
                                | OrchestrationV2ShellThreadStatus::Starting
                                | OrchestrationV2ShellThreadStatus::Running
                                | OrchestrationV2ShellThreadStatus::Waiting
                        ))
                {
                    return;
                }
                thread.pending_runtime_request = None;
                if thread.settled_override != Some(AppThreadSettledOverride::Settled)
                    || thread.settled_at.is_none()
                {
                    thread.settled_at = Some(self.now.clone());
                }
                thread.settled_override = Some(AppThreadSettledOverride::Settled);
                thread.unsettled_at = Some(None);
                thread.active_order_key = Some(None);
                thread.pinned_at = Some(None);
                thread.pin_order_key = Some(None);
                thread.snoozed_at = Some(None);
                thread.snoozed_until = Some(None);
            }
            Unsettle => {
                if thread.settled_override != Some(AppThreadSettledOverride::Active) {
                    thread.unsettled_at = Some(Some(self.now.clone()));
                }
                thread.settled_override = Some(AppThreadSettledOverride::Active);
                thread.settled_at = None;
            }
            Snooze { until } => {
                let blocked_request = thread.pending_runtime_request.as_ref().is_some_and(|request| !matches!(&request.kind, RuntimeRequestKind::Member2(kind) if **kind == RuntimeRequestKindMember2::UserInput));
                if (!accepted
                    && (blocked_request
                        || matches!(
                            thread.status,
                            OrchestrationV2ShellThreadStatus::Preparing
                                | OrchestrationV2ShellThreadStatus::Queued
                                | OrchestrationV2ShellThreadStatus::Starting
                        )))
                    || !crate::time::epoch_ms(&until.0).is_some_and(|millis| {
                        u64::try_from(millis).is_ok_and(|millis| millis > self.now_epoch_ms)
                    })
                {
                    return;
                }
                thread.snoozed_until = Some(Some(until.clone()));
                thread.snoozed_at = Some(Some(self.now.clone()));
            }
            Unsnooze => {
                thread.snoozed_until = Some(None);
                thread.snoozed_at = Some(None);
            }
            SetAutoSettle { enabled } => {
                thread.auto_settle_disabled_at = Some(if *enabled {
                    None
                } else {
                    thread
                        .auto_settle_disabled_at
                        .clone()
                        .flatten()
                        .or_else(|| Some(self.now.clone()))
                });
            }
            Pin { order_key } => {
                if thread.pinned_at.as_ref().and_then(|v| v.as_ref()).is_none() {
                    thread.pinned_at = Some(Some(self.now.clone()));
                    thread.pin_order_key = Some(order_key.clone());
                }
                if thread.settled_override == Some(AppThreadSettledOverride::Settled) {
                    thread.settled_override = Some(AppThreadSettledOverride::Active);
                    thread.settled_at = None;
                    thread.unsettled_at = Some(Some(self.now.clone()));
                }
                thread.snoozed_at = Some(None);
                thread.snoozed_until = Some(None);
            }
            Unpin => {
                thread.pinned_at = Some(None);
                thread.pin_order_key = Some(None);
            }
            ReorderPin { order_key } => thread.pin_order_key = Some(Some(order_key.clone())),
            ReorderActive { order_key } => thread.active_order_key = Some(Some(order_key.clone())),
            SetModel { selection } => thread.model_selection = selection.clone(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn id(value: &str) -> CommandId {
        CommandId(value.into())
    }
    fn key(env: &str) -> ProjectionKey {
        ProjectionKey::shell(EnvironmentId(env.into()))
    }
    fn receipt(command: &str, env: &str, sequence: u64) -> CommandReceipt {
        CommandReceipt {
            command: id(command),
            environment: EnvironmentId(env.into()),
            committed_sequence: sequence,
        }
    }
    #[test]
    fn receipt_waits_for_its_projection_and_handles_projection_before_reply() {
        let mut state = OptimisticState::default();
        state.insert(id("first"), key("a"), 0, 1);
        assert!(!state.accepted(&receipt("first", "b", 8)));
        assert!(state.accepted(&receipt("first", "a", 8)));
        state.observe(&key("b"), 0, 99);
        state.observe(&key("a"), 0, 7);
        assert!(state.pending.contains_key(&id("first")));
        state.observe(&key("a"), 0, 8);
        assert!(state.pending.is_empty());
        state.insert(id("later"), key("a"), 0, 2);
        state.observe(&key("a"), 0, 10);
        state.accepted(&receipt("later", "a", 9));
        assert!(state.pending.is_empty());
    }
    #[test]
    fn queued_inverse_keeps_enqueue_order_and_rejection_only_removes_its_overlay() {
        let mut state = OptimisticState::default();
        let base = Rc::new(0);
        state.insert(id("z-first"), key("a"), 0, 1);
        state.insert(id("a-undo"), key("a"), 0, 2);
        let view = |state: &OptimisticState<i32>| {
            state.view(&base, &key("a"), |view, value, _| *view = *value)
        };
        assert_eq!(*view(&state), 2);
        state.reject(&id("z-first"));
        assert_eq!(*view(&state), 2);
        state.reject(&id("a-undo"));
        assert!(Rc::ptr_eq(&view(&state), &base));
        assert_eq!(*base, 0);
    }
    #[test]
    fn rejected_undo_reveals_confirmed_action_without_replacing_authority() {
        let mut state = OptimisticState::default();
        state.insert(id("settle"), key("a"), 0, 1);
        state.insert(id("undo"), key("a"), 0, 0);
        state.accepted(&receipt("settle", "a", 5));
        state.observe(&key("a"), 0, 5);
        let base = Rc::new(1);
        assert_eq!(*state.view(&base, &key("a"), |v, i, _| *v = *i), 0);
        state.reject(&id("undo"));
        assert!(Rc::ptr_eq(
            &state.view(&base, &key("a"), |v, i, _| *v = *i),
            &base
        ));
    }
    #[test]
    fn reset_suspends_old_receipts_until_explicit_revalidation() {
        let mut state = OptimisticState::default();
        state.insert(id("c"), key("a"), 0, 1);
        state.accepted(&receipt("c", "a", 100));
        state.reset(&key("a"), 1, 2, &BTreeSet::new());
        state.observe(&key("a"), 0, 1000);
        assert!(state.needs_revalidation(&id("c")));
        assert!(!state.accepted(&receipt("c", "a", 100)));
        let base = Rc::new(0);
        assert!(Rc::ptr_eq(
            &base,
            &state.view(&base, &key("a"), |v, i, _| *v = *i)
        ));
        assert!(!state.revalidated(&receipt("c", "a", 3), 0));
        assert!(state.revalidated(&receipt("c", "a", 3), 1));
        state.observe(&key("a"), 1, 3);
        assert!(state.pending.is_empty());
    }
    #[test]
    fn disconnect_retains_ambiguous_intents_but_does_not_erase_known_acceptance() {
        let mut state = OptimisticState::default();
        for (id_value, env) in [("pending", "a"), ("accepted", "a"), ("other", "b")] {
            state.insert(id(id_value), key(env), 0, 1);
        }
        state.accepted(&receipt("accepted", "a", 5));
        state.disconnect(&EnvironmentId("a".into()));
        assert_eq!(
            state.pending[&id("pending")].status,
            CommandStatus::UnknownDelivery
        );
        assert_eq!(
            state.pending[&id("accepted")].status,
            CommandStatus::Accepted { sequence: 5 }
        );
        assert_eq!(state.pending[&id("other")].status, CommandStatus::Pending);
        state.observe(&key("a"), 0, 9);
        assert!(state.pending.contains_key(&id("pending")));
    }
    #[test]
    fn matching_evidence_is_scoped_to_environment_thread_and_lineage() {
        let mut state = OptimisticState::default();
        let thread = ProjectionKey::thread(EnvironmentId("a".into()), ThreadId("t".into()));
        state.insert(id("c"), thread.clone(), 4, 1);
        assert!(!state.matching_entity(&id("c"), &key("a"), 4));
        assert!(!state.matching_entity(&id("c"), &thread, 3));
        assert!(state.matching_entity(&id("c"), &thread, 4));
        assert!(state.pending.is_empty());
        state.insert(id("reset"), thread.clone(), 4, 2);
        state.reset(&thread, 5, 1, &BTreeSet::new());
        assert!(!state.matching_entity(&id("reset"), &thread, 5));
        assert!(!state.revalidated_entity(&id("reset"), &thread, 4));
        assert!(state.revalidated_entity(&id("reset"), &thread, 5));
        assert!(state.pending.is_empty());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod lifecycle_tests {
    use super::*;
    use serde_json::{Value, json};
    use t3_contracts::{orchestration_v2::*, wire::WireDateTime};
    const NOW: &str = "2026-09-12T12:00:00.000Z";
    fn shell() -> OrchestrationV2ShellSnapshot {
        serde_json::from_value(json!({"schemaVersion":1,"snapshotSequence":1,"projects":[],"archivedThreads":[],"threads":[{"id":"t","projectId":"p","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"t","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":NOW,"updatedAt":NOW,"archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null,"latestRunId":null,"activeRunId":null,"status":"idle","pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0}]})).unwrap()
    }
    fn apply(
        shell: &mut OrchestrationV2ShellSnapshot,
        action: ThreadLifecycleAction,
        accepted: bool,
    ) {
        ThreadLifecycleIntent {
            thread: ThreadId("t".into()),
            action,
            now: WireDateTime(NOW.into()),
            now_epoch_ms: 1,
        }
        .apply(
            shell,
            &if accepted {
                CommandStatus::Accepted { sequence: 2 }
            } else {
                CommandStatus::Pending
            },
        );
    }
    #[test]
    fn lifecycle_actions_and_reverse_actions_preserve_authoritative_cursor() {
        use ThreadLifecycleAction::*;
        let mut data = shell();
        apply(
            &mut data,
            Pin {
                order_key: Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                    "a".into(),
                )),
            },
            false,
        );
        assert_eq!(
            data.threads[0]
                .pin_order_key
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .0,
            "a"
        );
        apply(
            &mut data,
            ReorderPin {
                order_key: t3_contracts::base_schemas::TrimmedNonEmptyString("b".into()),
            },
            false,
        );
        apply(&mut data, Unpin, false);
        assert_eq!(data.threads[0].pinned_at, Some(None));
        apply(
            &mut data,
            Snooze {
                until: WireDateTime("2099-01-01T00:00:00.000Z".into()),
            },
            false,
        );
        assert_eq!(
            data.threads[0].snoozed_at,
            Some(Some(WireDateTime(NOW.into())))
        );
        apply(&mut data, Unsnooze, false);
        assert_eq!(data.threads[0].snoozed_at, Some(None));
        apply(&mut data, Settle, false);
        assert_eq!(
            data.threads[0].settled_override,
            Some(AppThreadSettledOverride::Settled)
        );
        apply(&mut data, Unsettle, false);
        assert_eq!(
            data.threads[0].settled_override,
            Some(AppThreadSettledOverride::Active)
        );
        apply(&mut data, SetAutoSettle { enabled: false }, false);
        assert_eq!(
            data.threads[0].auto_settle_disabled_at,
            Some(Some(WireDateTime(NOW.into())))
        );
        apply(&mut data, SetAutoSettle { enabled: true }, false);
        assert_eq!(data.threads[0].auto_settle_disabled_at, Some(None));
        apply(
            &mut data,
            ReorderActive {
                order_key: t3_contracts::base_schemas::TrimmedNonEmptyString("c".into()),
            },
            false,
        );
        assert_eq!(data.snapshot_sequence, 1);
    }
    #[test]
    fn settle_waits_for_acceptance_while_busy_and_never_resurrects_removed_rows() {
        let mut data = shell();
        data.threads[0].status = OrchestrationV2ShellThreadStatus::Running;
        apply(&mut data, ThreadLifecycleAction::Settle, false);
        assert_eq!(data.threads[0].settled_override, None);
        apply(&mut data, ThreadLifecycleAction::Settle, true);
        assert_eq!(
            data.threads[0].settled_override,
            Some(AppThreadSettledOverride::Settled)
        );
        data.threads.clear();
        apply(&mut data, ThreadLifecycleAction::Unsettle, true);
        assert!(data.threads.is_empty());
    }
    #[test]
    fn snooze_restamps_an_old_question_but_preserves_it_and_waits_on_approval() {
        let mut data = shell();
        let request = |kind| {
            serde_json::from_value::<OrchestrationV2PendingRuntimeRequestSummary>(
                json!({"id":"request","kind":kind,"createdAt":NOW,"title":null,"description":null}),
            )
            .unwrap()
        };
        data.threads[0].pending_runtime_request = Some(request("user_input"));
        data.threads[0].snoozed_at = Some(Some(WireDateTime("2000-01-01T00:00:00.000Z".into())));
        let action = || ThreadLifecycleAction::Snooze {
            until: WireDateTime("2099-01-01T00:00:00.000Z".into()),
        };
        apply(&mut data, action(), false);
        assert_eq!(
            data.threads[0].snoozed_at,
            Some(Some(WireDateTime(NOW.into())))
        );
        assert!(data.threads[0].pending_runtime_request.is_some());
        data.threads[0].pending_runtime_request = Some(request("command"));
        data.threads[0].snoozed_at = Some(None);
        apply(&mut data, action(), false);
        assert_eq!(data.threads[0].snoozed_at, Some(None));
        apply(&mut data, action(), true);
        assert_eq!(
            data.threads[0].snoozed_at,
            Some(Some(WireDateTime(NOW.into())))
        );
        apply(&mut data, ThreadLifecycleAction::Settle, true);
        assert!(data.threads[0].pending_runtime_request.is_none());
        assert_eq!(
            serde_json::to_value(&data.threads[0]).unwrap()["snoozedUntil"],
            Value::Null
        );
    }
}
