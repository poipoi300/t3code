use super::{ThreadOperations, actions::Action};
use crate::{
    ClientError,
    optimistic::ThreadLifecycleAction,
    platform::Platform,
    thread_views::threads::undo::{
        ThreadUndoRegistry, ThreadUndoStep, UndoAction, UndoToken, plan_thread_undo,
    },
    types::ThreadKey,
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use std::cell::{Cell, RefCell};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString, orchestration_v2::OrchestrationV2ThreadShell,
    wire::WireDateTime,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UndoNotice {
    pub action: String,
    pub count: usize,
}
#[derive(Debug, Default)]
pub struct UndoReport {
    pub restored: usize,
    pub failures: Vec<ClientError>,
}
struct Entry {
    key: ThreadKey,
    label: &'static str,
    claim: UndoToken,
    steps: Vec<Action>,
}
#[derive(Default)]
pub struct UndoState {
    registry: ThreadUndoRegistry,
    entries: RefCell<Vec<Entry>>,
    deadline: Cell<Option<u64>>,
    claimed: Cell<bool>,
}
pub(super) struct PendingUndo(Option<Entry>);
impl Drop for PendingUndo {
    fn drop(&mut self) {
        if let Some(entry) = &self.0 {
            entry.claim.finish();
        }
    }
}

impl<P: Platform + Clone + 'static> ThreadOperations<P> {
    pub(super) fn begin_thread_undo(
        &self,
        key: &ThreadKey,
        action: &Action,
        row: &OrchestrationV2ThreadShell,
        notice: bool,
    ) -> PendingUndo {
        use ThreadLifecycleAction::*;
        let kind = match action {
            Action::Lifecycle(Settle | Unsettle) => Some("settle"),
            Action::Lifecycle(Snooze { .. } | Unsnooze) => Some("snooze"),
            Action::Lifecycle(Pin { .. } | Unpin | ReorderPin { .. }) => Some("pin"),
            Action::Archive | Action::Unarchive => Some("archive"),
            _ => None,
        };
        if matches!(
            action,
            Action::Archive | Action::Delete | Action::Lifecycle(Pin { .. } | Settle)
        ) {
            for kind in ["settle", "snooze", "pin"] {
                self.state
                    .undo
                    .registry
                    .invalidate(kind.into(), key.clone());
            }
        }
        if matches!(action, Action::Delete) {
            self.state
                .undo
                .registry
                .invalidate("archive".into(), key.clone());
        }
        let Some(kind) = kind else {
            return PendingUndo(None);
        };
        self.state
            .undo
            .registry
            .invalidate(kind.into(), key.clone());
        if !notice {
            return PendingUndo(None);
        }
        let (label, steps) = match action {
            Action::Archive => ("Archived", vec![Action::Unarchive]),
            Action::Lifecycle(Settle | Snooze { .. } | Unpin) => {
                let (label, undo) = match action {
                    Action::Lifecycle(Settle) => ("Settled", UndoAction::Settle),
                    Action::Lifecycle(Unpin) => ("Unpinned", UndoAction::Unpin),
                    _ => ("Snoozed", UndoAction::Snooze),
                };
                let steps = plan_thread_undo(
                    undo,
                    row.pinned_at.as_ref().is_some_and(Option::is_some),
                    row.pin_order_key
                        .as_ref()
                        .and_then(Option::as_ref)
                        .map(|k| k.0.clone()),
                    row.snoozed_until
                        .as_ref()
                        .and_then(Option::as_ref)
                        .map(|s| s.0.clone()),
                )
                .into_iter()
                .map(|step| {
                    Action::Lifecycle(match step {
                        ThreadUndoStep::Unsettle => Unsettle,
                        ThreadUndoStep::Unsnooze => Unsnooze,
                        ThreadUndoStep::Pin { order_key } => Pin {
                            order_key: order_key.map(TrimmedNonEmptyString),
                        },
                        ThreadUndoStep::Snooze { snoozed_until } => Snooze {
                            until: WireDateTime(snoozed_until),
                        },
                    })
                })
                .collect();
                (label, steps)
            }
            _ => return PendingUndo(None),
        };
        let claim = self.state.undo.registry.begin(kind.into(), key.clone());
        self.state.actions.notify();
        PendingUndo(Some(Entry {
            key: key.clone(),
            label,
            claim,
            steps,
        }))
    }
    pub(super) fn commit_thread_undo(&self, mut pending: PendingUndo) {
        let Some(entry) = pending.0.take() else {
            return;
        };
        if !entry.claim.is_current() {
            return;
        }
        self.state.undo.entries.borrow_mut().push(entry);
        self.state
            .undo
            .deadline
            .set(Some(self.platform.monotonic_ms().saturating_add(5_000)));
        self.state.undo.claimed.set(true);
        self.state.actions.notify();
    }
    fn refresh_undo(&self) {
        let expired = self
            .state
            .undo
            .deadline
            .get()
            .is_some_and(|d| self.platform.monotonic_ms() >= d);
        let mut entries = self.state.undo.entries.borrow_mut();
        entries.retain(|e| {
            if expired {
                e.claim.finish();
                false
            } else {
                e.claim.is_current()
            }
        });
        if entries.is_empty() {
            self.state.undo.deadline.set(None);
            self.state.undo.claimed.set(false);
        }
    }
    pub fn thread_undo_notice(&self) -> Option<UndoNotice> {
        self.refresh_undo();
        let entries = self.state.undo.entries.borrow();
        let label = entries.last()?.label;
        Some(UndoNotice {
            action: label.into(),
            count: entries
                .iter()
                .rev()
                .take_while(|e| e.label == label)
                .count(),
        })
    }
    pub fn thread_undo_claims_shortcut(&self) -> bool {
        self.thread_undo_notice().is_some() && self.state.undo.claimed.get()
    }
    pub fn release_thread_undo_shortcut(&self) {
        self.state.undo.claimed.set(false);
    }
    pub fn thread_undo_notice_changes(&self) -> LocalBoxStream<'static, Option<UndoNotice>> {
        let service = self.clone();
        let changes = self.state.actions.changes();
        futures::stream::unfold(
            (service, changes, true),
            |(service, mut changes, first)| async move {
                if !first {
                    let timer = match service.state.undo.deadline.get() {
                        Some(d) => service
                            .platform
                            .sleep(d.saturating_sub(service.platform.monotonic_ms())),
                        None => futures::future::pending().boxed_local(),
                    };
                    let timer = timer.fuse();
                    futures::pin_mut!(timer);
                    futures::select! {_ = timer=>{}, event=changes.next().fuse()=>{event?;}}
                }
                let notice = service.thread_undo_notice();
                Some((notice, (service, changes, false)))
            },
        )
        .boxed_local()
    }
    pub fn undo_latest_thread_action(&self) -> LocalBoxFuture<'static, Result<usize, ClientError>> {
        self.undo_latest_thread_actions()
            .map(|mut report| {
                if report.failures.is_empty() {
                    Ok(report.restored)
                } else {
                    Err(report.failures.remove(0))
                }
            })
            .boxed_local()
    }
    pub fn undo_latest_thread_actions(&self) -> LocalBoxFuture<'static, UndoReport> {
        self.refresh_undo();
        let entries = {
            let mut entries = self.state.undo.entries.borrow_mut();
            let Some(label) = entries.last().map(|e| e.label) else {
                return futures::future::ready(UndoReport::default()).boxed_local();
            };
            let start = entries
                .iter()
                .rposition(|e| e.label != label)
                .map_or(0, |i| i + 1);
            entries.split_off(start)
        };
        for e in &entries {
            e.claim.finish();
        }
        self.state.actions.notify();
        let service = self.clone();
        async move {
            let results = futures::future::join_all(entries.into_iter().map(|entry| {
                let service = service.clone();
                async move {
                    for step in entry.steps {
                        if let Action::Lifecycle(ThreadLifecycleAction::Snooze { until }) = &step
                            && crate::time::epoch_ms(&until.0)
                                .is_some_and(|d| d <= service.platform.now_epoch_ms() as i64)
                        {
                            continue;
                        }
                        service.perform_impl(entry.key.clone(), step, false).await?;
                    }
                    Ok::<_, ClientError>(())
                }
            }))
            .await;
            let mut report = UndoReport::default();
            for result in results {
                match result {
                    Ok(()) => report.restored += 1,
                    Err(error) => {
                        if !matches!(error, ClientError::Cancelled) {
                            report.failures.push(error);
                        }
                    }
                }
            }
            service.refresh_undo();
            service.state.actions.notify();
            report
        }
        .boxed_local()
    }
}
