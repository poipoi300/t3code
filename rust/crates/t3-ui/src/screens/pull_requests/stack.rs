//! Stacked PR popovers, detail stack menu, layer navigation, and stack merge/rebase confirmation.

use std::rc::Rc;

use futures::future::LocalBoxFuture;
use nsh_core::prelude::*;
use t3_client::{
    types::EnvironmentId,
    workspace::pr_views::{
        detail::{can_perform_action, merge_method_label, readable_failure},
        service::{PullRequestActionOptions, PullRequestReviewSession},
    },
};
use t3_contracts::pull_request::*;

use crate::{
    i18n::{Text, Value, t, tr},
    runtime::tasks::{TaskOwner, watch},
    theme::tw,
    ui::{
        button::{ButtonContent, ButtonSize, ButtonVariant},
        dialog::{
            ActionVariant, FooterVariant, PopupOptions as DialogPopupOptions, dialog,
            dialog_action, dialog_close, dialog_description, dialog_footer, dialog_header,
            dialog_panel, dialog_popup, dialog_title,
        },
        icon::{Icon, icon_with_class},
        menu,
        toast::{ToastOptions, ToastUpdate, Variant, toast_manager},
    },
};

pub(super) type StackRead = Rc<
    dyn Fn(
        EnvironmentId,
        PullRequestRef,
    ) -> LocalBoxFuture<'static, Result<Option<PullRequestStack>, String>>,
>;

#[derive(Clone, PartialEq)]
pub(super) enum StackMembership {
    Pending,
    Failed,
    Single,
    Stack(Rc<PullRequestStack>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StackConfirm {
    Merge,
    Rebase,
}

fn state_icon_and_label(state: &PullRequestState, is_draft: bool) -> (Icon, &'static str) {
    match state {
        PullRequestState::Closed => (Icon::GitPullRequestClosed, "Closed"),
        PullRequestState::Merged => (Icon::GitMerge, "Merged"),
        _ if is_draft => (Icon::GitPullRequestDraft, "Draft"),
        _ => (Icon::GitPullRequest, "Open"),
    }
}

fn stack_label(number: u64, position: u64, size: u64) -> String {
    tr(
        "Stack {number}, layer {position} of {size}",
        &[
            ("number", Value::Int(number as i64)),
            ("position", Value::Int(position as i64)),
            ("size", Value::Int(size as i64)),
        ],
    )
}

fn stack_layers_menu(
    stack: &PullRequestStack,
    current_number: u64,
    disabled: bool,
    on_select: Rc<dyn Fn(u64)>,
) -> AnyElement {
    let mut items = view().app_class(tw("flex flex-col"));
    for layer in stack.layers.iter().rev() {
        let number = layer.number;
        let is_current = number == current_number;
        let is_draft = layer.is_draft.flatten() == Some(true);
        let (icon, state_label) = state_icon_and_label(&layer.state, is_draft);
        let title = layer
            .title
            .as_ref()
            .and_then(Option::as_ref)
            .filter(|t| !t.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| layer.head_branch.0.clone());
        let subtitle = format!("#{} · {} · {}", number, layer.head_branch.0, t(state_label));
        let row = view()
            .app_class(tw("flex min-w-0 flex-1 flex-row items-center gap-2"))
            .child(icon_with_class(icon, "size-4 shrink-0"))
            .child(
                view()
                    .app_class(tw("flex min-w-0 flex-1 flex-col"))
                    .child(text(title.clone()).app_class(tw("truncate")))
                    .child(text(subtitle).app_class(tw("truncate text-xs text-muted-foreground"))),
            )
            .child(is_current.then(|| icon_with_class(Icon::Check, "size-3.5 shrink-0")));
        let select_layer = on_select.clone();
        items = items.child(menu::menu_item(
            title,
            menu::ItemOptions {
                disabled: Prop::new(disabled),
                ..Default::default()
            },
            row,
            move || select_layer(number),
        ));
    }
    let base_label = format!("↳ {}", stack.base.0);
    view()
        .app_class(tw("flex max-h-80 flex-col"))
        .child(crate::ui::scroll_area::scroll_area(
            false,
            items.child(menu::menu_group_label(base_label, false)),
        ))
        .into_element()
}

/// Lazily loads and shows the stack layers for a list row badge without fetching stacks upfront.
pub(super) fn row_stack_popover(
    read_stack: StackRead,
    env: EnvironmentId,
    reference: PullRequestRef,
    membership: PullRequestStackMembership,
    on_select: Rc<dyn Fn(u64)>,
) -> AnyElement {
    let open = signal(false);
    let revision = signal(0u64);
    let state = signal(None::<Result<Option<PullRequestStack>, String>>);
    let env_for_effect = env.clone();
    let ref_for_effect = reference.clone();
    effect(move || {
        if !open.get() {
            return;
        }
        revision.get();
        state.set(None);
        let future = (read_stack)(env_for_effect.clone(), ref_for_effect.clone());
        watch(async move {
            let result = future.await;
            state.try_set(Some(result));
        });
    });

    let trigger_label = stack_label(membership.number, membership.position, membership.size);
    let ratio = format!("{}/{}", membership.position, membership.size);
    let current_number = reference.number;
    let header_number = membership.number;

    menu::menu_button(
        trigger_label,
        || false,
        view()
            .app_class(tw(
                "inline-flex shrink-0 items-center gap-1 rounded-md px-1.5 py-0.5 text-xs text-muted-foreground hover:bg-muted",
            ))
            .child(icon_with_class(Icon::Layers, "size-3"))
            .child(text(ratio)),
        menu::MenuOptions {
            open: Some(open),
            align: Align::Start,
            ..Default::default()
        },
        menu::menu_popup_with(move || match state.get() {
            Some(Ok(Some(stack))) => {
                let heading = tr(
                    "Stack #{number}",
                    &[("number", Value::Int(stack.number as i64))],
                );
                let select_cb = on_select.clone();
                view()
                    .child(menu::menu_group_label(heading, false))
                    .child(stack_layers_menu(
                        &stack,
                        current_number,
                        false,
                        Rc::new(move |num| {
                            open.set(false);
                            select_cb(num);
                        }),
                    ))
                    .into_element()
            }
            Some(Ok(None)) => {
                let heading = tr(
                    "Stack #{number}",
                    &[("number", Value::Int(header_number as i64))],
                );
                view()
                    .child(menu::menu_group_label(heading, false))
                    .child(menu::menu_group_label(
                        t("This pull request is no longer in a stack."),
                        false,
                    ))
                    .into_element()
            }
            Some(Err(err)) => {
                let heading = tr(
                    "Stack #{number}",
                    &[("number", Value::Int(header_number as i64))],
                );
                let retry_label = t("Retry stack refresh");
                view()
                    .child(menu::menu_group_label(heading, false))
                    .child(menu::menu_item(
                        retry_label.clone(),
                        Default::default(),
                        text(retry_label),
                        move || revision.update(|n| *n += 1),
                    ))
                    .child(menu::menu_group_label(err, false))
                    .into_element()
            }
            None => {
                let heading = tr(
                    "Stack #{number}",
                    &[("number", Value::Int(header_number as i64))],
                );
                view()
                    .child(menu::menu_group_label(heading, false))
                    .child(menu::menu_group_label(t("Loading stack…"), false))
                    .into_element()
            }
        }),
    )
    .into_element()
}

pub(super) struct DetailStackInput {
    pub detail: Rc<PullRequestDetail>,
    pub session: PullRequestReviewSession,
    pub stack: Signal<StackMembership>,
    pub stack_revision: Signal<u64>,
    pub writable: Signal<bool>,
    pub method: Memo<PullRequestMergeMethod>,
    pub methods_available: bool,
    pub stack_actions: bool,
    pub host_label: String,
    pub term_singular: &'static str,
    pub term_plural: &'static str,
    pub list: Option<super::list::ListRefresh>,
    pub refresh: Signal<u64>,
    pub owner: TaskOwner,
    pub on_select: Rc<dyn Fn(u64)>,
}

fn count_term(count: usize, singular: &'static str, plural: &'static str) -> String {
    if count == 1 { t(singular) } else { t(plural) }
}

struct StackActionPlan {
    layers: Vec<PullRequestStackLayers>,
    expected_heads: Vec<PullRequestStackHead>,
    target_number: Option<u64>,
    selected_open: bool,
    has_closed: bool,
    has_draft: bool,
    blocked: bool,
}

fn build_action_plan(stack: &PullRequestStack, pr_number: u64, is_merge: bool) -> StackActionPlan {
    let position_idx = stack.layers.iter().position(|l| l.number == pr_number);
    let position = position_idx.map(|i| i + 1);
    let layers: Vec<_> = if is_merge {
        position.map_or_else(Vec::new, |p| {
            stack
                .layers
                .iter()
                .take(p)
                .filter(|l| l.state != PullRequestState::Merged)
                .cloned()
                .collect()
        })
    } else {
        stack
            .layers
            .iter()
            .filter(|l| l.state != PullRequestState::Merged)
            .cloned()
            .collect()
    };
    let expected_heads: Vec<_> = layers
        .iter()
        .filter_map(|l| {
            l.head_sha
                .as_ref()
                .and_then(Option::as_ref)
                .map(|sha| PullRequestStackHead {
                    number: l.number,
                    head_sha: sha.clone(),
                })
        })
        .collect();
    let has_closed = layers.iter().any(|l| l.state != PullRequestState::Open);
    let has_draft = layers.iter().any(|l| l.is_draft.flatten() == Some(true));
    let target = if is_merge {
        position_idx.and_then(|i| stack.layers.get(i))
    } else {
        stack.layers.last()
    };
    let selected_open = position_idx
        .and_then(|i| stack.layers.get(i))
        .is_some_and(|l| l.state == PullRequestState::Open);
    let blocked = if is_merge {
        !selected_open
            || expected_heads.len() != layers.len()
            || has_closed
            || has_draft
            || layers.is_empty()
    } else {
        expected_heads.len() != layers.len() || has_closed || layers.is_empty() || target.is_none()
    };
    StackActionPlan {
        layers,
        expected_heads,
        target_number: target.map(|l| l.number),
        selected_open,
        has_closed,
        has_draft,
        blocked,
    }
}

/// Renders `PullRequestStackMenu`, optional `Merge stack` primary button, and confirmation dialogs.
pub(super) fn detail_stack_controls(input: DetailStackInput) -> AnyElement {
    let stack_data = match input.stack.get() {
        StackMembership::Stack(s) => s,
        _ => return view().into_element(),
    };
    let pr = input.detail;
    let position_idx = stack_data.layers.iter().position(|l| l.number == pr.number);
    let position = position_idx.map(|i| i + 1);
    let total = stack_data.layers.len().max(1);
    let trigger_label = stack_label(
        stack_data.number,
        position.unwrap_or(0) as u64,
        total as u64,
    );
    let ratio = position.map_or_else(|| format!("-/{total}"), |p| format!("{p}/{total}"));

    let merge_plan = build_action_plan(&stack_data, pr.number, true);
    let rebase_plan = build_action_plan(&stack_data, pr.number, false);

    let can_merge = input.stack_actions
        && position_idx.is_some()
        && can_perform_action(&pr, &PullRequestAction::Merge)
        && input.methods_available;
    let can_rebase = input.stack_actions && pr.viewer_permissions.stack_rebase == Some(Some(true));

    let pending = signal(false);
    let menu_open = signal(false);
    let confirmation = signal(None::<StackConfirm>);
    let last_confirm = signal(StackConfirm::Merge);
    let dialog_open = signal(false);
    let dialog_stale = signal(None::<String>);
    let dialog_revalidating = signal(false);
    let dialog_seq = signal(0u64);
    effect(move || {
        dialog_open.set(confirmation.get().is_some());
    });

    let merge_count = merge_plan.layers.len();
    let selected_open = merge_plan.selected_open;
    let merge_blocked_static = merge_plan.blocked;
    let rebase_blocked_static = rebase_plan.blocked;
    let merge_has_closed = merge_plan.has_closed;
    let merge_has_draft = merge_plan.has_draft;
    let expected_merge_heads = merge_plan.expected_heads.clone();
    let expected_rebase_heads = rebase_plan.expected_heads.clone();
    let confirm_layers_merge = merge_plan.layers;
    let confirm_layers_rebase = rebase_plan.layers;
    let selected_for_run = merge_plan.target_number;
    let top_for_run = rebase_plan.target_number;
    let can_write_session = input.session.can_write();
    let writable = input.writable;
    let merge_blocked =
        move || pending.get() || !writable.get() || !can_write_session || merge_blocked_static;
    let rebase_blocked =
        move || pending.get() || !writable.get() || !can_write_session || rebase_blocked_static;

    let stack_for_menu = stack_data.clone();
    let on_select = input.on_select.clone();
    let stack_number = stack_data.number;
    let stack_base = stack_data.base.0.clone();
    let current_number = pr.number;
    let merge_item_blocked = merge_blocked;
    let rebase_item_blocked = rebase_blocked;
    let show_merge_rule_hint = merge_has_closed || merge_has_draft;
    let stack_actions_enabled = input.stack_actions;

    let open_confirm_session = input.session.clone();
    let open_merge_heads = expected_merge_heads.clone();
    let open_rebase_heads = expected_rebase_heads.clone();
    let open_confirm_owner = input.owner;
    let open_confirm = move |kind: StackConfirm| {
        let is_merge = kind == StackConfirm::Merge;
        if pending.get_untracked()
            || !writable.get_untracked()
            || !open_confirm_session.can_write()
            || if is_merge {
                merge_blocked_static
            } else {
                rebase_blocked_static
            }
        {
            return;
        }
        let Some(expected_target) = (if is_merge {
            selected_for_run
        } else {
            top_for_run
        }) else {
            return;
        };
        let expected_heads = if is_merge {
            open_merge_heads.clone()
        } else {
            open_rebase_heads.clone()
        };
        last_confirm.set(kind);
        confirmation.set(Some(kind));
        dialog_stale.set(None);
        dialog_revalidating.set(true);
        let seq = dialog_seq.get_untracked() + 1;
        dialog_seq.set(seq);
        let session = open_confirm_session.clone();
        open_confirm_owner.spawn(async move {
            let result = session.stack().await;
            if dialog_seq.try_get() != Some(seq) {
                return;
            }
            dialog_revalidating.try_set(false);
            match result {
                Ok(Some(fresh)) => {
                    let plan = build_action_plan(&fresh, current_number, is_merge);
                    if fresh.number != stack_number
                        || plan.blocked
                        || plan.target_number != Some(expected_target)
                        || plan.expected_heads != expected_heads
                    {
                        dialog_stale.try_set(Some(t(
                            "Stack data may be stale. Refresh before running this action.",
                        )));
                    }
                }
                Ok(None) | Err(_) => {
                    dialog_stale
                        .try_set(Some(t("Stack data may be stale. We couldn’t refresh it.")));
                }
            }
        });
    };

    let open_from_merge_item = open_confirm.clone();
    let open_from_rebase_item = open_confirm.clone();
    let open_from_primary = open_confirm;

    let menu_btn = menu::menu_button(
        trigger_label,
        || false,
        view()
            .app_class(tw(
                "inline-flex items-center gap-1 rounded-md border border-input px-2 py-1 text-xs",
            ))
            .child(icon_with_class(Icon::Layers, "size-3.5"))
            .child(text(ratio)),
        menu::MenuOptions {
            open: Some(menu_open),
            align: Align::Start,
            ..Default::default()
        },
        menu::menu_popup_with(move || {
            let heading = tr(
                "Stack #{number}",
                &[("number", Value::Int(stack_number as i64))],
            );
            let on_select = on_select.clone();
            let mut body =
                view()
                    .child(menu::menu_group_label(heading, false))
                    .child(stack_layers_menu(
                        &stack_for_menu,
                        current_number,
                        pending.get(),
                        Rc::new(move |number| {
                            menu_open.set(false);
                            on_select(number);
                        }),
                    ));
            if stack_actions_enabled && (can_merge || can_rebase) {
                body = body.child(menu::menu_separator());
                if can_merge {
                    let label = tr(
                        "Merge stack ({count})",
                        &[("count", Value::Int(merge_count as i64))],
                    );
                    let blocked = merge_item_blocked;
                    let open = open_from_merge_item.clone();
                    body = body.child(menu::menu_item(
                        label.clone(),
                        menu::ItemOptions {
                            disabled: Prop::bind(blocked),
                            ..Default::default()
                        },
                        view()
                            .app_class(tw("flex flex-row items-center gap-2"))
                            .child(icon_with_class(Icon::GitMerge, "size-3.5"))
                            .child(text(label)),
                        move || open(StackConfirm::Merge),
                    ));
                }
                if can_rebase {
                    let label = t("Rebase stack");
                    let blocked = rebase_item_blocked;
                    let open = open_from_rebase_item.clone();
                    body = body.child(menu::menu_item(
                        label.clone(),
                        menu::ItemOptions {
                            disabled: Prop::bind(blocked),
                            ..Default::default()
                        },
                        view()
                            .app_class(tw("flex flex-row items-center gap-2"))
                            .child(icon_with_class(Icon::RefreshCw, "size-3.5"))
                            .child(text(label)),
                        move || open(StackConfirm::Rebase),
                    ));
                }
                if show_merge_rule_hint {
                    body = body.child(
                        text(t(
                            "Every layer being merged must be open and ready for review.",
                        ))
                        .app_class(tw("px-2 py-1 text-xs text-muted-foreground")),
                    );
                }
            }
            body
        }),
    );

    let merge_primary_btn = (can_merge && selected_open).then(|| {
        let blocked = merge_blocked;
        nsh_base::button()
            .accessibility_label(t("Merge stack"))
            .disabled(blocked)
            .on_press(move || open_from_primary(StackConfirm::Merge))
            .app_class(tw(
                "inline-flex items-center gap-1 rounded-md bg-primary px-2.5 py-1 text-xs text-primary-foreground",
            ))
            .child(icon_with_class(Icon::GitMerge, "size-3.5"))
            .child(text(t("Merge stack")))
    });

    let method = input.method;
    let host_label_for_run = input.host_label.clone();
    let host_label_for_dialog = input.host_label;
    let term_singular = input.term_singular;
    let term_plural = input.term_plural;
    let session = input.session;
    let stack_rev = input.stack_revision;
    let refresh = input.refresh;
    let list = input.list;
    let owner = input.owner;
    let merge_heads_run = expected_merge_heads;
    let rebase_heads_run = expected_rebase_heads;

    let run_confirmed = move || {
        let Some(kind) = confirmation.get_untracked() else {
            return;
        };
        let is_merge = kind == StackConfirm::Merge;
        if pending.get_untracked()
            || dialog_revalidating.get_untracked()
            || dialog_stale.with_untracked(Option::is_some)
        {
            return;
        }
        if !writable.get_untracked()
            || !session.can_write()
            || if is_merge {
                merge_blocked_static
            } else {
                rebase_blocked_static
            }
        {
            dialog_stale.set(Some(t("This stack action is currently blocked.")));
            return;
        }
        let Some(target_number) = (if is_merge {
            selected_for_run
        } else {
            top_for_run
        }) else {
            return;
        };
        let action_heads = if is_merge {
            merge_heads_run.clone()
        } else {
            rebase_heads_run.clone()
        };
        pending.set(true);
        let toasts = toast_manager();
        let session = session.clone();
        let host_label = host_label_for_run.clone();
        let chosen_method = is_merge.then(|| method.get_untracked());
        owner.spawn(async move {
            let preflight = session.stack().await;
            let preflight_ok = match preflight {
                Ok(Some(fresh)) => {
                    let plan = build_action_plan(&fresh, current_number, is_merge);
                    fresh.number == stack_number
                        && !plan.blocked
                        && plan.target_number == Some(target_number)
                        && plan.expected_heads == action_heads
                        && writable.get_untracked()
                        && session.can_write()
                }
                Ok(None) | Err(_) => false,
            };
            if !preflight_ok {
                pending.try_set(false);
                dialog_stale.try_set(Some(t(
                    "Stack data may be stale. We couldn’t refresh it.",
                )));
                return;
            }
            confirmation.try_set(None);
            let toast_id = toasts.add(ToastOptions {
                variant: Variant::Loading,
                title: Text::content(t(if is_merge {
                    "Merging stack..."
                } else {
                    "Rebasing stack..."
                })),
                ..Default::default()
            });
            let result = session
                .run_action_with(PullRequestActionOptions {
                    action: if is_merge {
                        PullRequestAction::Merge
                    } else {
                        PullRequestAction::UpdateBranch
                    },
                    merge_method: chosen_method,
                    update_method: (!is_merge).then_some(PullRequestUpdateMethod::Rebase),
                    number_override: Some(target_number),
                    stack_number: Some(stack_number),
                    expected_stack_heads: Some(action_heads),
                })
                .await;
            pending.try_set(false);
            if let Some(rev) = stack_rev.try_get() {
                stack_rev.set(rev + 1);
            }
            if let Some(rev) = refresh.try_get() {
                refresh.set(rev + 1);
            }
            if let Some(list) = list {
                super::update_if_alive(list.revision, |n| *n += 1);
            }
            match result {
                Ok(_) => {
                    let success_desc = is_merge.then(|| {
                        Text::content(tr(
                            "{host} merged the stack or added it to its merge queue.",
                            &[("host", host_label.into())],
                        ))
                    });
                    toasts.update(
                        toast_id,
                        ToastUpdate {
                            variant: Some(Variant::Success),
                            title: Some(Text::content(t(if is_merge {
                                "Stack merge request completed"
                            } else {
                                "Stack rebased"
                            }))),
                            description: Some(success_desc),
                            ..Default::default()
                        },
                    );
                }
                Err(error) => {
                    let reason = super::rejection_reason(error);
                    let hint = if is_merge {
                        "The host refused the merge. Check that you have write access, that the checks it requires have passed, and that the branch is not conflicting."
                    } else {
                        "The host refused the rebase. A rebase stops at the first commit that does not apply cleanly."
                    };
                    toasts.update(
                        toast_id,
                        ToastUpdate {
                            variant: Some(Variant::Error),
                            title: Some(Text::content(t(if is_merge {
                                "Could not merge the stack"
                            } else {
                                "Could not rebase the stack"
                            }))),
                            description: Some(Some(Text::content(readable_failure(
                                &reason,
                                &t(hint),
                            )))),
                            ..Default::default()
                        },
                    );
                }
            }
        });
    };

    let confirm_dialog = dialog(dialog_open, move |dlg| {
        dlg.on_open_change(move |next| {
            if !next {
                let seq = dialog_seq.get_untracked() + 1;
                dialog_seq.set(seq);
                dialog_revalidating.set(false);
                dialog_stale.set(None);
                confirmation.set(None);
            }
        });
        let merge_layers_list = confirm_layers_merge.clone();
        let rebase_layers_list = confirm_layers_rebase.clone();
        let stack_base_desc = stack_base.clone();
        let host_for_desc = host_label_for_dialog.clone();
        let run = run_confirmed.clone();
        dialog_popup(dlg, DialogPopupOptions::default(), move || {
            let is_merge =
                confirmation.get().unwrap_or_else(|| last_confirm.get()) == StackConfirm::Merge;
            let layers = if is_merge {
                merge_layers_list.clone()
            } else {
                rebase_layers_list.clone()
            };
            let title_layers_count = layers.len();
            let base_for_title = stack_base_desc.clone();
            let host_for_text = host_for_desc.clone();
            let mut list_view = view().app_class(tw("flex flex-col gap-1"));
            for layer in &layers {
                let is_draft = layer.is_draft.flatten() == Some(true);
                let (icon, state_label) = state_icon_and_label(&layer.state, is_draft);
                let title = layer
                    .title
                    .as_ref()
                    .and_then(Option::as_ref)
                    .filter(|t| !t.trim().is_empty())
                    .cloned()
                    .unwrap_or_else(|| layer.head_branch.0.clone());
                list_view = list_view.child(
                    view()
                        .app_class(tw(
                            "flex flex-row items-center gap-2 rounded-md bg-muted/50 px-3 py-2 text-sm",
                        ))
                        .child(icon_with_class(icon, "size-4 shrink-0"))
                        .child(
                            view()
                                .app_class(tw("flex min-w-0 flex-1 flex-col"))
                                .child(text(title).app_class(tw("truncate")))
                                .child(
                                    text(format!(
                                        "#{} · {}",
                                        layer.number,
                                        t(state_label)
                                    ))
                                    .app_class(tw("text-xs text-muted-foreground")),
                                ),
                        ),
                );
            }
            let action_label = if is_merge {
                t("Merge stack")
            } else {
                t("Rebase stack")
            };
            view()
                .test_id("pr-stack-confirm-dialog")
                .child(dialog_header(
                    dlg,
                    dialog_title(dlg, move || {
                        let term = count_term(title_layers_count, term_singular, term_plural);
                        Text::content(if is_merge {
                            tr(
                                "Merge {count} {term}?",
                                &[
                                    ("count", Value::Int(title_layers_count as i64)),
                                    ("term", term.into()),
                                ],
                            )
                        } else {
                            tr(
                                "Rebase {count} {term}?",
                                &[
                                    ("count", Value::Int(title_layers_count as i64)),
                                    ("term", term.into()),
                                ],
                            )
                        })
                    }),
                ))
                .child(dialog_panel(
                    dlg,
                    view()
                        .app_class(tw("flex flex-col gap-2"))
                        .child(dialog_description(dlg, move || {
                            Text::content(if is_merge {
                                tr(
                                    "Merge #{number} and its unmerged layers below into {base} using {method}. {host} checks their rules before merging or queueing them and rebases the remaining stack after merging.",
                                    &[
                                        ("number", Value::Int(current_number as i64)),
                                        ("base", base_for_title.clone().into()),
                                        ("method", merge_method_label(&method.get()).into()),
                                        ("host", host_for_text.clone().into()),
                                    ],
                                )
                            } else {
                                tr(
                                    "Rebase the remote branches from bottom to top onto {base}. This rewrites branch history and may restart checks. If a layer fails, earlier updates remain.",
                                    &[("base", base_for_title.clone().into())],
                                )
                            })
                        }))
                        .child(dynamic(move || {
                            dialog_stale.get().map(|notice| {
                                text(notice).app_class(tw("text-xs text-destructive"))
                            })
                        }))
                        .child(
                            view()
                                .app_class(tw("flex max-h-48 flex-col"))
                                .child(crate::ui::scroll_area::scroll_area(false, list_view)),
                        ),
                ))
                .child(dialog_footer(
                    dlg,
                    FooterVariant::Default,
                    view()
                        .child(dialog_close(
                            dlg,
                            ButtonContent::Label(t("Cancel")),
                            ButtonVariant::Outline,
                            ButtonSize::Default,
                            || false,
                        ))
                        .child(dialog_action(
                            dlg,
                            ButtonContent::Label(action_label),
                            ActionVariant::Default,
                            ButtonSize::Default,
                            move || {
                                pending.get()
                                    || dialog_revalidating.get()
                                    || dialog_stale.with(Option::is_some)
                                    || !writable.get()
                                    || !can_write_session
                            },
                            run.clone(),
                        )),
                ))
        })
    });

    view()
        .test_id("pr-detail-stack")
        .app_class(tw("flex flex-row items-center gap-1"))
        .child(menu_btn)
        .child(merge_primary_btn)
        .child(confirm_dialog)
        .into_element()
}
