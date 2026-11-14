//! Port of apps/web/src/confirmDialog.ts and components/ConfirmDialogHost.tsx ConfirmDialogHost.
//!
//! Mount `confirm_dialog_host()` once under App's persistent root. Call
//! `request_confirm_dialog(message, options)` before spawning the task that awaits its answer;
//! `None` means no host is mounted and must be treated as cancellation. TL-REWIND and other
//! features share this FIFO. A response resolves immediately; the next request waits for exit.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::LocalBoxFuture;
use nsh_core::prelude::*;

use super::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use super::dialog::{
    ActionVariant, AlertPopupOptions, FooterVariant, alert_dialog, alert_dialog_action,
    alert_dialog_close, alert_dialog_description, alert_dialog_footer, alert_dialog_header,
    alert_dialog_popup, alert_dialog_title,
};
use crate::i18n::t;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConfirmDialogVariant {
    #[default]
    Default,
    Destructive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmDialogInitialFocus {
    Confirm,
    Cancel,
}

/// Port of packages/contracts/src/ipc.ts ConfirmDialogOptions; this is UI-only state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConfirmDialogOptions {
    pub variant: ConfirmDialogVariant,
    pub initial_focus: Option<ConfirmDialogInitialFocus>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmDialogRequest {
    pub message: String,
    pub variant: ConfirmDialogVariant,
    pub initial_focus: Option<ConfirmDialogInitialFocus>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ConfirmDialogState {
    #[default]
    Idle,
    Confirming(ConfirmDialogRequest),
    Closing(ConfirmDialogRequest),
}

impl ConfirmDialogState {
    fn request(&self) -> Option<&ConfirmDialogRequest> {
        match self {
            Self::Idle => None,
            Self::Confirming(request) | Self::Closing(request) => Some(request),
        }
    }
}

pub type Confirmation = LocalBoxFuture<'static, bool>;

struct PendingConfirmation {
    request: ConfirmDialogRequest,
    resolve: oneshot::Sender<bool>,
}

#[derive(Default)]
struct CoordinatorState {
    state: ConfirmDialogState,
    active: Option<PendingConfirmation>,
    queued: VecDeque<PendingConfirmation>,
    listeners: BTreeMap<usize, Rc<dyn Fn()>>,
    next_listener: usize,
}

/// An independent coordinator is useful for an independently mounted client root or tests.
#[derive(Clone, Default)]
pub struct ConfirmDialogCoordinator(Rc<RefCell<CoordinatorState>>);

impl ConfirmDialogCoordinator {
    pub fn read_confirm_dialog_state(&self) -> ConfirmDialogState {
        self.0.borrow().state.clone()
    }

    pub fn subscribe_confirm_dialog_host(
        &self,
        listener: impl Fn() + 'static,
    ) -> ConfirmDialogHostSubscription {
        let mut inner = self.0.borrow_mut();
        let id = inner.next_listener;
        inner.next_listener += 1;
        inner.listeners.insert(id, Rc::new(listener));
        ConfirmDialogHostSubscription {
            coordinator: self.clone(),
            id: Some(id),
        }
    }

    pub fn request_confirm_dialog(
        &self,
        message: impl Into<String>,
        options: Option<ConfirmDialogOptions>,
    ) -> Option<Confirmation> {
        let mut inner = self.0.borrow_mut();
        if inner.listeners.is_empty() {
            return None;
        }
        let options = options.unwrap_or_default();
        let (resolve, answer) = oneshot::channel();
        let pending = PendingConfirmation {
            request: ConfirmDialogRequest {
                message: message.into(),
                variant: options.variant,
                initial_focus: options.initial_focus,
            },
            resolve,
        };
        let queued =
            inner.active.is_some() || matches!(inner.state, ConfirmDialogState::Closing(_));
        if queued {
            inner.queued.push_back(pending);
        } else {
            inner.state = ConfirmDialogState::Confirming(pending.request.clone());
            inner.active = Some(pending);
        }
        drop(inner);
        if !queued {
            self.publish();
        }
        Some(answer.map(|answer| answer.unwrap_or(false)).boxed_local())
    }

    pub fn is_confirm_dialog_active(&self) -> bool {
        let inner = self.0.borrow();
        inner.state != ConfirmDialogState::Idle
            || inner.active.is_some()
            || !inner.queued.is_empty()
    }

    pub fn respond_to_confirm_dialog(&self, confirmed: bool) {
        let mut inner = self.0.borrow_mut();
        if !matches!(inner.state, ConfirmDialogState::Confirming(_)) {
            return;
        }
        let Some(confirmation) = inner.active.take() else {
            return;
        };
        inner.state = ConfirmDialogState::Closing(confirmation.request);
        drop(inner);
        let _ = confirmation.resolve.send(confirmed);
        self.publish();
    }

    pub fn complete_confirm_dialog_close(&self) {
        let mut inner = self.0.borrow_mut();
        if !matches!(inner.state, ConfirmDialogState::Closing(_)) {
            return;
        }
        if let Some(next) = inner.queued.pop_front() {
            inner.state = ConfirmDialogState::Confirming(next.request.clone());
            inner.active = Some(next);
        } else {
            inner.state = ConfirmDialogState::Idle;
        }
        drop(inner);
        self.publish();
    }

    fn publish(&self) {
        let listeners: Vec<_> = self.0.borrow().listeners.values().cloned().collect();
        for listener in listeners {
            listener();
        }
    }
}

/// Dropping the last host cancels both the active request and its queue.
pub struct ConfirmDialogHostSubscription {
    coordinator: ConfirmDialogCoordinator,
    id: Option<usize>,
}

impl ConfirmDialogHostSubscription {
    pub fn unsubscribe(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };
        let mut inner = self.coordinator.0.borrow_mut();
        if inner.listeners.remove(&id).is_none() || !inner.listeners.is_empty() {
            return;
        }
        let active = inner.active.take();
        let queued = std::mem::take(&mut inner.queued);
        inner.state = ConfirmDialogState::Idle;
        drop(inner);
        for pending in active.into_iter().chain(queued) {
            let _ = pending.resolve.send(false);
        }
        self.coordinator.publish();
    }
}

impl Drop for ConfirmDialogHostSubscription {
    fn drop(&mut self) {
        self.unsubscribe();
    }
}

thread_local! {
    static COORDINATOR: ConfirmDialogCoordinator = ConfirmDialogCoordinator::default();
}

pub fn read_confirm_dialog_state() -> ConfirmDialogState {
    COORDINATOR.with(ConfirmDialogCoordinator::read_confirm_dialog_state)
}

pub fn request_confirm_dialog(
    message: impl Into<String>,
    options: Option<ConfirmDialogOptions>,
) -> Option<Confirmation> {
    COORDINATOR.with(|coordinator| coordinator.request_confirm_dialog(message, options))
}

pub fn is_confirm_dialog_active() -> bool {
    COORDINATOR.with(ConfirmDialogCoordinator::is_confirm_dialog_active)
}

#[derive(Debug, PartialEq, Eq)]
struct ConfirmationCopy {
    title: String,
    description: Option<String>,
}

/// Port of ConfirmDialogHost.tsx resolveConfirmDialogCopy.
fn resolve_confirm_dialog_copy(message: &str) -> ConfirmationCopy {
    let normalized = message.trim();
    let lines: Vec<_> = normalized.split('\n').collect();
    if let Some(question_line) = lines.iter().position(|line| line.trim().ends_with('?')) {
        let description = lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| (index != question_line).then_some(*line))
            .collect::<Vec<_>>()
            .join("\n");
        return ConfirmationCopy {
            title: lines[question_line].trim().into(),
            description: nonempty(description.trim()),
        };
    }
    if let Some(question_mark) = normalized.find('?') {
        return ConfirmationCopy {
            title: normalized[..=question_mark].trim().into(),
            description: nonempty(normalized[question_mark + 1..].trim()),
        };
    }
    ConfirmationCopy {
        title: "Confirm action".into(),
        description: Some(if normalized.is_empty() {
            "This action requires your confirmation.".into()
        } else {
            normalized.into()
        }),
    }
}

fn nonempty(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.into())
}

/// Port of ConfirmDialogHost.tsx ConfirmDialogHost, using the app-wide FIFO.
pub fn confirm_dialog_host() -> impl IntoElement {
    COORDINATOR.with(|coordinator| confirm_dialog_host_with(coordinator.clone()))
}

pub fn confirm_dialog_host_with(coordinator: ConfirmDialogCoordinator) -> impl IntoElement {
    let snapshot = signal(coordinator.read_confirm_dialog_state());
    let reading = coordinator.clone();
    let subscription = coordinator.subscribe_confirm_dialog_host(move || {
        let _ = snapshot.try_set(reading.read_confirm_dialog_state());
    });
    cleanup(move || drop(subscription));
    let open = signal(false);
    // The external coordinator owns request state; the modal owns its retained exit lifecycle.
    effect(move || open.set(matches!(snapshot.get(), ConfirmDialogState::Confirming(_))));
    alert_dialog(open, move |state| {
        let cancel = coordinator.clone();
        state.on_open_change(move |opened| {
            if !opened {
                cancel.respond_to_confirm_dialog(false);
            }
        });
        let closing = coordinator.clone();
        state.on_open_change_complete(move |opened| {
            if !opened {
                closing.complete_confirm_dialog_close();
            }
        });
        alert_dialog_popup(state, AlertPopupOptions::default(), move || {
            let current = snapshot.get_untracked();
            let request = current.request();
            let copy = resolve_confirm_dialog_copy(request.map_or("", |request| &request.message));
            let variant = request.map_or(ConfirmDialogVariant::Default, |request| request.variant);
            let focus_confirm = request.is_some_and(|request| {
                request.initial_focus == Some(ConfirmDialogInitialFocus::Confirm)
            });
            let confirm = coordinator.clone();
            let action = if focus_confirm && variant == ConfirmDialogVariant::Default {
                alert_dialog_action(
                    state,
                    ButtonContent::Label(t("Confirm")),
                    ActionVariant::Default,
                    ButtonSize::Default,
                    || false,
                    move || confirm.respond_to_confirm_dialog(true),
                )
                .into_element()
            } else {
                button(
                    ButtonContent::Label(t("Confirm")),
                    match variant {
                        ConfirmDialogVariant::Default => ButtonVariant::Default,
                        ConfirmDialogVariant::Destructive => ButtonVariant::Destructive,
                    },
                    ButtonSize::Default,
                    || false,
                    move || {
                        if open.get_untracked() {
                            confirm.respond_to_confirm_dialog(true);
                        }
                    },
                )
                .into_element()
            };
            let mut header =
                vec![alert_dialog_title(state, move || copy.title.clone()).into_element()];
            if let Some(description) = copy.description {
                // whitespace-pre-line: NSH preserves newlines; its text wraps natively.
                header.push(
                    alert_dialog_description(state, move || description.clone()).into_element(),
                );
            }
            vec![
                alert_dialog_header(state, header).into_element(),
                alert_dialog_footer(
                    state,
                    FooterVariant::Default,
                    vec![
                        alert_dialog_close(
                            state,
                            ButtonContent::Label(t("Cancel")),
                            ButtonVariant::Outline,
                            ButtonSize::Default,
                            || false,
                        )
                        .into_element(),
                        action,
                    ],
                )
                .into_element(),
            ]
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;

    fn request(
        message: &str,
        initial_focus: Option<ConfirmDialogInitialFocus>,
    ) -> ConfirmDialogRequest {
        ConfirmDialogRequest {
            message: message.into(),
            variant: ConfirmDialogVariant::Default,
            initial_focus,
        }
    }

    #[test]
    fn serializes_concurrent_confirmations() {
        let coordinator = ConfirmDialogCoordinator::default();
        let _host = coordinator.subscribe_confirm_dialog_host(|| {});
        let first = coordinator
            .request_confirm_dialog(
                "Delete the project?",
                Some(ConfirmDialogOptions {
                    initial_focus: Some(ConfirmDialogInitialFocus::Confirm),
                    ..ConfirmDialogOptions::default()
                }),
            )
            .expect("host");
        let mut second = coordinator
            .request_confirm_dialog("Delete the worktree too?", None)
            .expect("host");
        assert!(coordinator.is_confirm_dialog_active());
        assert!(second.as_mut().now_or_never().is_none());
        coordinator.respond_to_confirm_dialog(false);
        assert!(!block_on(first));
        assert_eq!(
            coordinator.read_confirm_dialog_state(),
            ConfirmDialogState::Closing(request(
                "Delete the project?",
                Some(ConfirmDialogInitialFocus::Confirm)
            ))
        );
        assert!(coordinator.is_confirm_dialog_active());
        assert!(second.as_mut().now_or_never().is_none());
        coordinator.complete_confirm_dialog_close();
        assert_eq!(
            coordinator.read_confirm_dialog_state(),
            ConfirmDialogState::Confirming(request("Delete the worktree too?", None))
        );
        coordinator.respond_to_confirm_dialog(true);
        assert!(block_on(second));
        coordinator.complete_confirm_dialog_close();
        assert_eq!(
            coordinator.read_confirm_dialog_state(),
            ConfirmDialogState::Idle
        );
        assert!(!coordinator.is_confirm_dialog_active());
    }

    #[test]
    fn cancels_active_and_queued_confirmations_if_the_last_host_unmounts() {
        let coordinator = ConfirmDialogCoordinator::default();
        let mut host = coordinator.subscribe_confirm_dialog_host(|| {});
        let active = coordinator
            .request_confirm_dialog("Delete the thread?", None)
            .expect("host");
        let queued = coordinator
            .request_confirm_dialog("Delete the worktree too?", None)
            .expect("host");
        host.unsubscribe();
        host.unsubscribe();
        assert_eq!(
            block_on(futures::future::join(active, queued)),
            (false, false)
        );
        assert_eq!(
            coordinator.read_confirm_dialog_state(),
            ConfirmDialogState::Idle
        );
        assert!(!coordinator.is_confirm_dialog_active());
    }

    #[test]
    fn has_no_host_until_one_subscribes() {
        let coordinator = ConfirmDialogCoordinator::default();
        assert!(
            coordinator
                .request_confirm_dialog("Before mount?", None)
                .is_none()
        );
        assert_eq!(
            coordinator.read_confirm_dialog_state(),
            ConfirmDialogState::Idle
        );
        assert!(!coordinator.is_confirm_dialog_active());
        let host = coordinator.subscribe_confirm_dialog_host(|| {});
        let answer = coordinator
            .request_confirm_dialog("Continue?", None)
            .expect("host");
        assert_eq!(
            coordinator.read_confirm_dialog_state(),
            ConfirmDialogState::Confirming(request("Continue?", None))
        );
        drop(host);
        assert!(!block_on(answer));
        assert!(
            coordinator
                .request_confirm_dialog("Continue?", None)
                .is_none()
        );
    }

    #[test]
    fn ignores_responses_after_the_active_dialog_has_been_closed() {
        let coordinator = ConfirmDialogCoordinator::default();
        let _host = coordinator.subscribe_confirm_dialog_host(|| {});
        let confirmation = coordinator
            .request_confirm_dialog("Continue?", None)
            .expect("host");
        coordinator.respond_to_confirm_dialog(true);
        coordinator.respond_to_confirm_dialog(false);
        coordinator.complete_confirm_dialog_close();
        assert_eq!(
            coordinator.read_confirm_dialog_state(),
            ConfirmDialogState::Idle
        );
        assert!(block_on(confirmation));
    }

    #[test]
    fn only_the_last_host_cancels_and_closing_requests_remain_fifo() {
        let coordinator = ConfirmDialogCoordinator::default();
        let first_host = coordinator.subscribe_confirm_dialog_host(|| {});
        let last_host = coordinator.subscribe_confirm_dialog_host(|| {});
        let first = coordinator
            .request_confirm_dialog("First?", None)
            .expect("host");
        drop(first_host);
        coordinator.complete_confirm_dialog_close();
        assert_eq!(
            coordinator.read_confirm_dialog_state(),
            ConfirmDialogState::Confirming(request("First?", None))
        );
        coordinator.respond_to_confirm_dialog(true);
        assert!(block_on(first));
        let second = coordinator
            .request_confirm_dialog("Second?", None)
            .expect("host");
        let third = coordinator
            .request_confirm_dialog("Third?", None)
            .expect("host");
        coordinator.complete_confirm_dialog_close();
        assert_eq!(
            coordinator.read_confirm_dialog_state(),
            ConfirmDialogState::Confirming(request("Second?", None))
        );
        coordinator.respond_to_confirm_dialog(false);
        assert!(!block_on(second));
        drop(last_host);
        assert!(!block_on(third));
        coordinator.complete_confirm_dialog_close();
        assert_eq!(
            coordinator.read_confirm_dialog_state(),
            ConfirmDialogState::Idle
        );
    }

    #[test]
    fn listeners_observe_published_state_without_a_borrow_conflict() {
        let coordinator = ConfirmDialogCoordinator::default();
        let observed = Rc::new(RefCell::new(Vec::new()));
        let values = observed.clone();
        let source = coordinator.clone();
        let _host = coordinator.subscribe_confirm_dialog_host(move || {
            values.borrow_mut().push(source.read_confirm_dialog_state())
        });
        let answer = coordinator
            .request_confirm_dialog("Continue?", None)
            .expect("host");
        coordinator.respond_to_confirm_dialog(true);
        coordinator.complete_confirm_dialog_close();
        assert!(block_on(answer));
        assert_eq!(
            *observed.borrow(),
            [
                ConfirmDialogState::Confirming(request("Continue?", None)),
                ConfirmDialogState::Closing(request("Continue?", None)),
                ConfirmDialogState::Idle
            ]
        );
    }

    #[test]
    fn confirmation_copy_preserves_the_webs_question_and_fallback_rules() {
        for (message, title, description) in [
            ("  Delete project?  ", "Delete project?", None),
            (
                "Before\n  Delete project? \n\nAfter? detail",
                "Delete project?",
                Some("Before\n\nAfter? detail"),
            ),
            (
                "Delete project? This cannot be undone.",
                "Delete project?",
                Some("This cannot be undone."),
            ),
            ("Really?\nDelete it?", "Really?", Some("Delete it?")),
            (
                "  Irreversible operation  ",
                "Confirm action",
                Some("Irreversible operation"),
            ),
            (
                " \n ",
                "Confirm action",
                Some("This action requires your confirmation."),
            ),
            (
                "Supprimer l’élément? Suite",
                "Supprimer l’élément?",
                Some("Suite"),
            ),
        ] {
            assert_eq!(
                resolve_confirm_dialog_copy(message),
                ConfirmationCopy {
                    title: title.into(),
                    description: description.map(String::from)
                },
                "{message:?}"
            );
        }
    }
}
