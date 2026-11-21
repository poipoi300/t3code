//! Header controls from PullRequestPrimaryAction and PullRequestActionsMenu.
use super::detail::button;
use crate::{
    i18n::{Text, t, tr},
    runtime::tasks::{TaskOwner, watch},
    theme::tw,
    ui::{
        menu,
        toast::{ToastOptions, Variant, toast_manager},
    },
};
use nsh_core::prelude::*;
use std::{rc::Rc, time::Duration};
use t3_client::workspace::pr_views::{
    detail::{
        ChecksState, PrimaryControlInput, allowed_merge_methods, can_perform_action,
        classify_checks, merge_method_label, primary_control, readable_failure,
        resolve_merge_method,
    },
    service::PullRequestReviewSession,
};
use t3_contracts::pull_request::*;

#[derive(Clone, Default, PartialEq)]
struct LastMethod(Option<PullRequestMergeMethod>);

#[derive(Clone, Copy)]
struct Controls {
    detail: Stored<Rc<PullRequestDetail>>,
    session: Stored<PullRequestReviewSession>,
    term: &'static str,
    pending: Signal<Option<PullRequestAction>>,
    armed: Signal<Option<PullRequestAction>>,
    open: Signal<bool>,
    writable: Signal<bool>,
    method: Memo<PullRequestMergeMethod>,
    selected: Signal<Option<PullRequestMergeMethod>>,
    last: Signal<LastMethod>,
    stack: Signal<Membership>,
    list: Option<super::list::ListRefresh>,
    owner: TaskOwner,
    refresh: Signal<u64>,
}
#[derive(Clone, PartialEq)]
enum Membership {
    Pending,
    Failed,
    Single,
    Stack(Rc<PullRequestStack>),
}
impl Controls {
    fn disabled(self) -> bool {
        !self.writable.get() || self.pending.with(Option::is_some)
    }
    fn label(self, action: &PullRequestAction) -> String {
        match action {
            PullRequestAction::Merge if self.pending.get() == Some(PullRequestAction::Merge) => {
                t("Merging...")
            }
            PullRequestAction::Merge if self.armed.get() == Some(PullRequestAction::Merge) => {
                t("Confirm merge?")
            }
            PullRequestAction::Merge => t(merge_method_label(&self.method.get())),
            PullRequestAction::Close if self.armed.get() == Some(PullRequestAction::Close) => {
                t("Confirm close?")
            }
            PullRequestAction::Close => tr("Close {term}", &[("term", t(self.term).into())]),
            PullRequestAction::Reopen => tr("Reopen {term}", &[("term", t(self.term).into())]),
            PullRequestAction::Draft => t("Convert to draft"),
            PullRequestAction::Ready => t("Ready for review"),
            _ => unreachable!(),
        }
    }
    fn activate(self, action: PullRequestAction) {
        let client = self.session.get();
        if self.disabled()
            || !client.can_write()
            || !self.detail.with(|d| can_perform_action(d, &action))
        {
            return;
        }
        if action == PullRequestAction::Merge && self.stack.get() != Membership::Single {
            return;
        }
        if matches!(action, PullRequestAction::Merge | PullRequestAction::Close)
            && self.armed.get() != Some(action.clone())
        {
            self.armed.set(Some(action));
            return;
        }
        self.armed.set(None);
        self.open.set(false);
        self.pending.set(Some(action.clone()));
        let method = (action == PullRequestAction::Merge).then(|| self.method.get());
        let toasts = toast_manager();
        self.owner.spawn(async move {
            let result = client.run_action(action.clone(), method).await;
            self.pending.try_set(None);
            match result {
                Ok(_) => {
                    if let Some(list) = self.list {
                        list.revision.update(|n| *n += 1);
                    }
                    if let Some(revision) = self.refresh.try_get() {
                        self.refresh.set(revision + 1);
                    }
                }
                Err(error) => {
                    let (title, hint) = failure_copy(&action, self.term);
                    let reason = match error.into_client_error() {
                        t3_client::ClientError::Rejected {
                            detail, message, ..
                        } => detail.unwrap_or(message),
                        _ => String::new(),
                    };
                    toasts.add(ToastOptions {
                        variant: Variant::Error,
                        title: Text::content(title),
                        description: Some(Text::content(readable_failure(&reason, &t(hint)))),
                        ..Default::default()
                    });
                }
            }
        });
    }
    fn confirmation_target(self, action: PullRequestAction, child: impl IntoElement) -> AnyElement {
        let anchor = element_ref();
        let target = action.clone();
        anchor.on_focus_out(move || {
            if self.armed.get_untracked().as_ref() == Some(&target) {
                self.armed.set(None);
            }
        });
        view()
            .element_ref(&anchor)
            .child(child)
            .child(when(
                move || self.armed.get().as_ref() == Some(&action),
                move || {
                    overlay()
                        .anchor(&anchor)
                        .on_press_outside(move |_: &Point<Points>| {
                            self.armed.set(None);
                        })
                },
            ))
            .into_element()
    }
    fn menu_action(self, action: PullRequestAction) -> AnyElement {
        let confirm = matches!(action, PullRequestAction::Close | PullRequestAction::Merge);
        let a = action.clone();
        let label = move || {
            if a == PullRequestAction::Merge && self.armed.get() != Some(PullRequestAction::Merge) {
                t("Merge now")
            } else {
                self.label(&a)
            }
        };
        // Keep the menu open for the first press. The confirmed handler closes it.
        let target = action.clone();
        let item = dynamic(move || {
            let caption = label();
            let action = action.clone();
            menu::menu_item(
                caption.clone(),
                menu::ItemOptions {
                    close_on_select: !confirm,
                    disabled: Prop::bind(move || self.disabled()),
                    ..Default::default()
                },
                text(caption),
                move || self.activate(action.clone()),
            )
        });
        if confirm {
            self.confirmation_target(target, item)
        } else {
            item.into_element()
        }
    }
}
fn failure_copy(action: &PullRequestAction, term: &str) -> (String, &'static str) {
    let name = [("term", t(term).into())];
    match action {
        PullRequestAction::Merge => (
            tr("Could not merge this {term}", &name),
            "The host refused the merge. Check that you have write access, that the checks it requires have passed, and that the branch is not conflicting.",
        ),
        PullRequestAction::Close => (
            tr("Could not close this {term}", &name),
            "The host refused it. Check that you have write access, or that you opened it.",
        ),
        PullRequestAction::Reopen => (
            tr("Could not reopen this {term}", &name),
            "The host refused it. Check that you have write access, and that the branch still exists.",
        ),
        PullRequestAction::Ready => (
            t("Could not mark this ready for review"),
            "The host refused it. Check that you have write access to this repository.",
        ),
        PullRequestAction::Draft => (
            t("Could not convert this to a draft"),
            "The host refused it. Check that you have write access to this repository.",
        ),
        _ => unreachable!("only lifecycle actions have header controls"),
    }
}
pub(super) fn controls(
    pr: Rc<PullRequestDetail>,
    client: PullRequestReviewSession,
    writable: Signal<bool>,
    refresh: Signal<u64>,
    default: Memo<Option<PullRequestMergeMethod>>,
    armed: Signal<Option<PullRequestAction>>,
) -> AnyElement {
    let stack_actions = pr.capabilities.stacks == Some(Some(true))
        && pr.capabilities.stack_actions == Some(Some(true));
    let stack = signal(if stack_actions {
        Membership::Pending
    } else {
        Membership::Single
    });
    let revision = signal(0u64);
    if stack_actions {
        let session = client.clone();
        effect(move || {
            revision.get();
            stack.set(Membership::Pending);
            let session = session.clone();
            watch(async move {
                stack.set(match session.stack().await {
                    Ok(Some(s)) => Membership::Stack(Rc::new(s)),
                    Ok(None) => Membership::Single,
                    Err(_) => Membership::Failed,
                });
            });
        });
    }
    let methods = stored(allowed_merge_methods(&pr));
    let selected = signal(None);
    let last = global::<LastMethod>();
    let method = computed(move || {
        resolve_merge_method(
            &methods.get(),
            selected.get().as_ref(),
            default.get().as_ref(),
            last.get()
                .0
                .as_ref()
                .unwrap_or(&PullRequestMergeMethod::Merge),
        )
    });
    let term = if pr.provider.0 == "gitlab" {
        "merge request"
    } else {
        "pull request"
    };
    let list = use_context::<super::list::ListRefresh>();
    let c = Controls {
        detail: stored(pr.clone()),
        session: stored(client),
        term,
        pending: signal(None),
        armed,
        open: signal(false),
        writable,
        method,
        selected,
        last,
        stack,
        list,
        owner: list.map_or_else(TaskOwner::here, |list| list.owner),
        refresh,
    };
    effect(move || {
        if c.armed.get().is_some() {
            on_key("escape", move |_| c.armed.set(None));
            set_timeout(Duration::from_secs(3), move || c.armed.set(None));
        }
    });
    effect(move || {
        if nsh_core::element::desktop::window_state().is_some_and(|state| !state.focused) {
            c.armed.set(None);
        }
    });
    effect(move || {
        let _ = c.open.get();
        c.armed.set(None);
    });
    let can = |a| can_perform_action(&pr, &a);
    let open = pr.state == PullRequestState::Open;
    let close = open && can(PullRequestAction::Close);
    let reopen = pr.state == PullRequestState::Closed && can(PullRequestAction::Reopen);
    let draft = open && !pr.is_draft && can(PullRequestAction::Draft);
    let methods_visible = open
        && !pr.is_draft
        && pr.mergeability != PullRequestMergeability::Conflicting
        && can(PullRequestAction::Merge)
        && methods.with(|m| m.len() > 1);
    let more = tr("More {term} actions", &[("term", t(term).into())]);
    let primary = computed(move || {
        let checks = match classify_checks(&pr.checks) {
            ChecksState::None => None,
            ChecksState::Passing => Some(PullRequestChecksState::Passing),
            ChecksState::Pending => Some(PullRequestChecksState::Pending),
            ChecksState::Failing => Some(PullRequestChecksState::Failing),
        };
        let single = !matches!(stack.get(), Membership::Stack(_));
        primary_control(PrimaryControlInput {
            state: &pr.state,
            is_draft: pr.is_draft,
            mergeability: &pr.mergeability,
            checks: checks.as_ref(),
            auto_merge_enabled: pr.auto_merge_enabled.flatten(),
            has_merge_method: methods.with(|m| !m.is_empty()),
            can_merge: single && can_perform_action(&pr, &PullRequestAction::Merge),
            can_mark_ready: can_perform_action(&pr, &PullRequestAction::Ready),
            can_enable_auto_merge: single
                && can_perform_action(&pr, &PullRequestAction::EnableAutoMerge),
        })
    });
    let merge_now = computed(move || {
        stack.get() == Membership::Single
            && matches!(
                primary.get(),
                Some("enable-auto-merge" | "auto-merge-armed")
            )
            && c.detail.with(|p| {
                !p.is_draft
                    && p.mergeability != PullRequestMergeability::Conflicting
                    && can_perform_action(p, &PullRequestAction::Merge)
            })
            && methods.with(|m| !m.is_empty())
    });
    let ready_menu = computed(move || {
        primary.get() != Some("ready")
            && c.detail.with(|p| {
                p.state == PullRequestState::Open
                    && p.is_draft
                    && can_perform_action(p, &PullRequestAction::Ready)
            })
    });
    view()
        .app_class(tw("flex flex-row flex-wrap items-center gap-1"))
        .child(dynamic(move || match stack.get() {
            Membership::Failed => button(t("Retry stack lookup"), move || {
                revision.update(|n| *n += 1)
            }),
            _ => view().into_element(),
        }))
        .child(dynamic(move || {
            let a = match primary.get() {
                Some("merge") => PullRequestAction::Merge,
                Some("ready") => PullRequestAction::Ready,
                _ => return view().into_element(),
            };
            let click = a.clone();
            let caption = a.clone();
            let target = a.clone();
            let control = nsh_base::button()
                .accessibility_label(move || SharedString::from(c.label(&a)))
                .child(text(move || c.label(&caption)))
                .disabled(move || {
                    c.disabled()
                        || primary.get() == Some("merge") && stack.get() != Membership::Single
                })
                .on_press(move || c.activate(click.clone()));
            c.confirmation_target(target, control)
        }))
        .child(dynamic(move || match (primary.get(), stack.get()) {
            (Some("merge"), Membership::Pending) => {
                text(t("Checking stack membership…")).into_element()
            }
            (Some("merge"), Membership::Failed) => {
                text(t("Couldn't check stack membership")).into_element()
            }
            _ => view().into_element(),
        }))
        .child(menu::menu_button(
            more.clone(),
            || false,
            text(more),
            menu::MenuOptions {
                open: Some(c.open),
                ..Default::default()
            },
            menu::menu_popup_with(move || {
                let mut items = view();
                if draft {
                    items = items.child(c.menu_action(PullRequestAction::Draft));
                }
                if ready_menu.get() {
                    items = items.child(c.menu_action(PullRequestAction::Ready));
                }
                if merge_now.get() {
                    items = items.child(c.menu_action(PullRequestAction::Merge));
                }
                if methods_visible {
                    let choices = view().child(methods.get().into_iter().map(move |m| {
                        let label = t(merge_method_label(&m));
                        menu::menu_radio_item(
                            label.clone(),
                            merge_method_label(&m).into(),
                            menu::ItemOptions {
                                disabled: Prop::bind(move || c.disabled()),
                                ..Default::default()
                            },
                            text(label),
                            move || {
                                c.selected.set(Some(m.clone()));
                                c.last.set(LastMethod(Some(m.clone())));
                            },
                        )
                    }));
                    items = items.child(menu::menu_radio_group(
                        t("Merge method"),
                        move || merge_method_label(&method.get()).into(),
                        choices,
                        |_| {},
                    ));
                }
                if close {
                    items = items.child(c.menu_action(PullRequestAction::Close));
                }
                if reopen {
                    items = items.child(c.menu_action(PullRequestAction::Reopen));
                }
                items
            }),
        ))
        .into_element()
}
