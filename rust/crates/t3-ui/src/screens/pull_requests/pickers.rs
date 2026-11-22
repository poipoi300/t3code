//! Summary tab reviewer/label pickers and base branch stacked/freshness controls.

use std::rc::Rc;

use nsh_core::prelude::*;
use t3_client::workspace::pr_views::{
    detail::{base_freshness, is_stacked_base, readable_failure},
    reviewers::{
        ReviewerSearchInput, ReviewerSearchState, ReviewerTruncation, candidate_matches_label,
        host_reviewer_query, reviewer_candidate_view, reviewer_entries,
    },
    service::{PullRequestActionOptions, PullRequestReviewSession},
};
use t3_contracts::{base_schemas::TrimmedNonEmptyString, pull_request::*};

use crate::{
    i18n::{Text, Value, t, tr},
    runtime::tasks::{TaskOwner, watch},
    theme::tw,
    ui::{
        icon::{Icon, icon_with_class},
        popover::{PopoverOptions, popover, popover_create_handle, popover_popup, popover_trigger},
        toast::{ToastOptions, Variant, toast_manager},
    },
};

fn truncation_message(kind: ReviewerTruncation, error: Option<&str>) -> String {
    match kind {
        ReviewerTruncation::Unsearchable => t(
            "This repository has more people with access than are listed here. Ask for the rest on the host.",
        ),
        ReviewerTruncation::Searchable => t(
            "This repository has more people with access than are listed here. Search to find them.",
        ),
        ReviewerTruncation::SearchFailed => tr(
            "Only the people already listed are matched: the host search failed. {error}",
            &[("error", error.unwrap_or_default().into())],
        ),
        ReviewerTruncation::SearchTruncated => {
            t("More people match than the host returned. Keep typing to narrow it down.")
        }
    }
}

/// Renders the header base branch with stacked indicator and out-of-date branch update controls.
pub(super) fn base_branch_indicator(
    pr: Rc<PullRequestDetail>,
    session: PullRequestReviewSession,
    writable: Signal<bool>,
    refresh: Signal<u64>,
    owner: TaskOwner,
) -> AnyElement {
    let refs_loaded = signal(Vec::<t3_contracts::git::VcsRef>::new());
    let cwd = pr.workspace_root.clone();
    let ref_session = session.clone();
    effect(move || {
        let session = ref_session.clone();
        let cwd = cwd.clone();
        watch(async move {
            if let Ok(result) = session.list_refs(cwd).await {
                refs_loaded.try_set(result.refs);
            }
        });
    });

    let base_name = pr.base_branch.0.clone();
    let freshness = base_freshness(&pr);
    let stacked = computed(move || refs_loaded.with(|refs| is_stacked_base(&base_name, refs)));

    if let Some((behind_by, methods)) = freshness {
        let summary = match behind_by {
            Some(1) => tr(
                "This branch is out-of-date with {base} by {count} commit.",
                &[
                    ("base", pr.base_branch.0.clone().into()),
                    ("count", Value::Int(1)),
                ],
            ),
            Some(count) => tr(
                "This branch is out-of-date with {base} by {count} commits.",
                &[
                    ("base", pr.base_branch.0.clone().into()),
                    ("count", Value::Int(count as i64)),
                ],
            ),
            None => tr(
                "This branch is out-of-date with {base}.",
                &[("base", pr.base_branch.0.clone().into())],
            ),
        };
        let handle = popover_create_handle();
        let pending = signal(false);
        let list = use_context::<super::list::ListRefresh>();
        let branch_label = pr.base_branch.0.clone();
        let trigger = popover_trigger(
            handle,
            summary.clone(),
            || false,
            view()
                .app_class(tw(
                    "inline-flex min-w-0 items-center gap-1 rounded-sm text-xs",
                ))
                .child(dynamic(move || {
                    stacked
                        .get()
                        .then(|| icon_with_class(Icon::Layers, "size-3 shrink-0"))
                }))
                .child(text(branch_label))
                .child(icon_with_class(Icon::TriangleAlert, "size-3.5 shrink-0")),
        );
        return popover(handle, PopoverOptions::default(), trigger, move || {
            let mut buttons = view().app_class(tw("flex flex-row flex-wrap gap-1.5"));
            for method in &methods {
                let method = method.clone();
                let label = t(if method == PullRequestUpdateMethod::Rebase {
                    "Update with rebase"
                } else {
                    "Update branch"
                });
                let session = session.clone();
                buttons = buttons.child(
                    nsh_base::button()
                        .accessibility_label(label.clone())
                        .disabled(move || pending.get() || !writable.get())
                        .on_press(move || {
                            if pending.get_untracked() || !session.can_write() {
                                return;
                            }
                            pending.set(true);
                            handle.set_open(false);
                            let session = session.clone();
                            let method = method.clone();
                            let toasts = toast_manager();
                            owner.spawn(async move {
                                let result = session
                                    .run_action_with(PullRequestActionOptions {
                                        action: PullRequestAction::UpdateBranch,
                                        merge_method: None,
                                        update_method: Some(method.clone()),
                                        number_override: None,
                                        stack_number: None,
                                        expected_stack_heads: None,
                                    })
                                    .await;
                                pending.try_set(false);
                                match result {
                                    Ok(_) => {
                                        if let Some(list) = list {
                                            super::update_if_alive(list.revision, |n| *n += 1);
                                        }
                                        if let Some(rev) = refresh.try_get() {
                                            refresh.set(rev + 1);
                                        }
                                    }
                                    Err(error) => {
                                        let reason = super::rejection_reason(error);
                                        let hint = if method == PullRequestUpdateMethod::Rebase {
                                            "The host refused the rebase. A rebase stops at the first commit that does not apply cleanly."
                                        } else {
                                            "The host refused it. Check that you have write access and that the base branch still exists."
                                        };
                                        toasts.add(ToastOptions {
                                            variant: Variant::Error,
                                            title: Text::content(t("Could not update this branch")),
                                            description: Some(Text::content(readable_failure(
                                                &reason,
                                                &t(hint),
                                            ))),
                                            ..Default::default()
                                        });
                                    }
                                }
                            });
                        })
                        .child(icon_with_class(Icon::GitMerge, "size-3"))
                        .child(text(label)),
                );
            }
            popover_popup(
                handle,
                PopoverOptions::default(),
                view()
                    .app_class(tw("flex max-w-80 flex-col gap-1.5 p-2 text-xs"))
                    .child(text(summary.clone()))
                    .child(
                        text(t("Changes can be cleanly merged."))
                            .app_class(tw("text-muted-foreground")),
                    )
                    .child(buttons),
            )
            .into_element()
        })
        .into_element();
    }

    let branch_text = pr.base_branch.0.clone();
    view()
        .app_class(tw("inline-flex min-w-0 items-center gap-1"))
        .child(dynamic(move || {
            stacked
                .get()
                .then(|| icon_with_class(Icon::Layers, "size-3 shrink-0"))
        }))
        .child(text(branch_text))
        .into_element()
}

fn reviewer_picker(
    pr: &PullRequestDetail,
    session: PullRequestReviewSession,
    writable: Signal<bool>,
    refresh: Signal<u64>,
) -> AnyElement {
    let allowed = pr.viewer_permissions.request_reviewers;
    let can_write = session.can_write();
    let searchable = pr.capabilities.reviewers.search_candidates.flatten() == Some(true);
    let label = t("Request a review");

    let owner = TaskOwner::here();
    let handle = popover_create_handle();
    let query = signal(String::new());
    let debounced = signal(String::new());
    let loaded = signal(None::<Result<PullRequestReviewerCandidateList, String>>);
    let searched = signal(None::<(String, Result<PullRequestReviewerCandidateList, String>)>);
    let pending_id = signal(None::<String>);

    super::bind_debounced_query(query, debounced);

    let load_session = session.clone();
    effect(move || {
        if !handle.is_open() {
            return;
        }
        let session = load_session.clone();
        watch(async move {
            let result = session
                .reviewer_candidates(None)
                .await
                .map_err(|e| e.into_client_error().to_string());
            loaded.try_set(Some(result));
        });
    });

    let loaded_truncated = computed(move || {
        loaded.with(|l| {
            l.as_ref()
                .and_then(|r| r.as_ref().ok())
                .map(|v| v.truncated)
        })
    });
    let search_session = session.clone();
    effect(move || {
        if !handle.is_open() {
            return;
        }
        let q = query.get();
        let dq = debounced.get();
        let _ready = loaded_truncated.get();
        let host_query = loaded.with_untracked(|l| {
            let list = l.as_ref().and_then(|r| r.as_ref().ok());
            host_reviewer_query(&ReviewerSearchInput {
                loaded: list,
                searchable,
                query: &q,
                debounced_query: &dq,
            })
        });
        let Some(hq) = host_query else {
            return;
        };
        let session = search_session.clone();
        let key = hq.clone();
        watch(async move {
            let result = session
                .reviewer_candidates(Some(TrimmedNonEmptyString(hq)))
                .await
                .map_err(|e| e.into_client_error().to_string());
            searched.try_set(Some((key, result)));
        });
    });

    let search_label = t("Search people with access");
    let trigger = popover_trigger(
        handle,
        label,
        move || !allowed || !writable.get(),
        icon_with_class(Icon::UserPlus, "size-3.5"),
    );

    popover(handle, PopoverOptions::default(), trigger, move || {
        let search_placeholder = search_label.clone();
        let search_aria = search_label.clone();
        let session = session.clone();
        let rows = dynamic(move || {
            let q = query.get();
            let dq = debounced.get();
            let loaded_res = loaded.get();
            let searched_res = searched.get();
            let loaded_ok = loaded_res.as_ref().and_then(|r| r.as_ref().ok());
            let loaded_err = loaded_res.as_ref().and_then(|r| r.as_ref().err());
            let host_q = host_reviewer_query(&ReviewerSearchInput {
                loaded: loaded_ok,
                searchable,
                query: &q,
                debounced_query: &dq,
            });
            let matching_search = host_q
                .as_deref()
                .and_then(|hq| searched_res.as_ref().filter(|(k, _)| k == hq).map(|(_, r)| r));
            let search_state = ReviewerSearchState {
                data: matching_search.and_then(|r| r.as_ref().ok()),
                is_pending: host_q.is_some() && matching_search.is_none(),
                error: matching_search.and_then(|r| r.as_ref().err()).map(String::as_str),
            };
            let view_model = reviewer_candidate_view(
                &ReviewerSearchInput {
                    loaded: loaded_ok,
                    searchable,
                    query: &q,
                    debounced_query: &dq,
                },
                &search_state,
            );
            let initial_pending = (loaded_res.is_none()) || view_model.search_pending;
            let error = if loaded_ok.is_none() {
                loaded_err.cloned()
            } else if view_model.candidates.is_empty() {
                view_model.search_error.clone()
            } else {
                None
            };

            let has_error = error.is_some();
            let mut container = view().app_class(tw("flex flex-col gap-1"));
            if initial_pending {
                container = container.child(
                    text(t("Loading…")).app_class(tw("p-2 text-xs text-muted-foreground")),
                );
            } else if let Some(err) = error {
                container = container.child(
                    text(format!(
                        "{} {}",
                        t("The people with access could not be read."),
                        err
                    ))
                    .app_class(tw("p-2 text-xs text-muted-foreground")),
                );
            } else if view_model.candidates.is_empty() {
                let msg = if q.trim().is_empty() {
                    t("Nobody else has access to this repository.")
                } else {
                    t("Nobody with access matches that.")
                };
                container = container.child(text(msg).app_class(tw("p-2 text-xs text-muted-foreground")));
            } else {
                for candidate in view_model.candidates {
                    let login = candidate.login.0.clone();
                    let next_requested = !candidate.is_requested;
                    let candidate_id = candidate.id.clone();
                    let candidate_kind = candidate.kind.clone();
                    let session = session.clone();
                    let display = if let Some(name) = candidate.name.as_deref()
                        && !name.is_empty()
                        && name != login
                    {
                        format!("{name} (@{login})")
                    } else {
                        login.clone()
                    };
                    container = container.child(
                        nsh_base::button()
                            .accessibility_label(login.clone())
                            .disabled(move || {
                                !can_write || !writable.get() || pending_id.with(Option::is_some)
                            })
                            .on_press(move || {
                                if !session.can_write()
                                    || !writable.get_untracked()
                                    || pending_id.with_untracked(Option::is_some)
                                {
                                    return;
                                }
                                pending_id.set(Some(candidate_id.0.clone()));
                                let session = session.clone();
                                let id = candidate_id.clone();
                                let kind = candidate_kind.clone();
                                let login = login.clone();
                                let toasts = toast_manager();
                                owner.spawn(async move {
                                    let result = session
                                        .request_reviewers(
                                            vec![PullRequestReviewerRequestInputReviewers {
                                                id: id.clone(),
                                                kind: kind.clone(),
                                            }],
                                            next_requested,
                                        )
                                        .await;
                                    pending_id.try_set(None);
                                    match result {
                                        Ok(()) => {
                                            super::update_if_alive(loaded, |state| {
                                                if let Some(Ok(list)) = state {
                                                    for item in &mut list.candidates {
                                                        if item.id == id && item.kind == kind {
                                                            item.is_requested = next_requested;
                                                        }
                                                    }
                                                }
                                            });
                                            super::update_if_alive(searched, |state| {
                                                if let Some((_, Ok(list))) = state {
                                                    for item in &mut list.candidates {
                                                        if item.id == id && item.kind == kind {
                                                            item.is_requested = next_requested;
                                                        }
                                                    }
                                                }
                                            });
                                            if let Some(rev) = refresh.try_get() {
                                                refresh.set(rev + 1);
                                            }
                                            let title = if next_requested {
                                                tr(
                                                    "Review requested from {login}",
                                                    &[("login", login.into())],
                                                )
                                            } else {
                                                tr(
                                                    "Review request to {login} taken back",
                                                    &[("login", login.into())],
                                                )
                                            };
                                            toasts.add(ToastOptions {
                                                variant: Variant::Success,
                                                title: Text::content(title),
                                                ..Default::default()
                                            });
                                        }
                                        Err(error) => {
                                            let reason = super::rejection_reason(error);
                                            let title = if next_requested {
                                                tr(
                                                    "Could not ask {login} for a review",
                                                    &[("login", login.into())],
                                                )
                                            } else {
                                                tr(
                                                    "Could not take back the review request to {login}",
                                                    &[("login", login.into())],
                                                )
                                            };
                                            toasts.add(ToastOptions {
                                                variant: Variant::Error,
                                                title: Text::content(title),
                                                description: Some(Text::content(readable_failure(
                                                    &reason,
                                                    &t("The host refused it. Check that you have write access on this repository, and that they still have access to it."),
                                                ))),
                                                ..Default::default()
                                            });
                                        }
                                    }
                                });
                            })
                            .app_class(tw(
                                "flex w-full flex-row items-center gap-2 rounded-sm px-2 py-1 text-left text-xs hover:bg-accent",
                            ))
                            .child(text(display).app_class(tw("flex-1 truncate")))
                            .child((candidate.kind == PullRequestReviewerKind::Team).then(|| {
                                text(t("team")).app_class(tw("shrink-0 text-muted-foreground"))
                            }))
                            .child(
                                candidate
                                    .is_requested
                                    .then(|| icon_with_class(Icon::Check, "size-3.5 shrink-0")),
                            ),
                    );
                }
            }
            if !initial_pending && !has_error
                && let Some(kind) = view_model.truncation
            {
                let msg = truncation_message(kind, view_model.search_error.as_deref());
                container = container.child(
                    text(msg).app_class(tw("px-2 py-1 text-xs text-muted-foreground")),
                );
            }
            view()
                .app_class(tw("flex max-h-72 flex-col"))
                .child(crate::ui::scroll_area::scroll_area(false, container))
        });

        popover_popup(
            handle,
            PopoverOptions::default(),
            view()
                .w(px(288.0))
                .app_class(tw("flex flex-col gap-1 p-2"))
                .child(
                    nsh_base::input()
                        .value(move || query.get())
                        .on_change(move |value: &str| query.set(value.to_owned()))
                        .placeholder(move || search_placeholder.clone())
                        .label(move || search_aria.clone())
                        .app_class(tw(
                            "h-8 w-full rounded-md border border-input bg-transparent px-2 text-xs text-foreground outline-none",
                        )),
                )
                .child(rows),
        )
        .into_element()
    })
    .into_element()
}

fn label_picker(
    pr: &PullRequestDetail,
    session: PullRequestReviewSession,
    writable: Signal<bool>,
    refresh: Signal<u64>,
) -> AnyElement {
    let allowed = pr.viewer_permissions.labels != Some(Some(false));
    let can_write = session.can_write();
    let label = t("Change labels");
    let owner = TaskOwner::here();
    let handle = popover_create_handle();
    let query = signal(String::new());
    let loaded = signal(None::<Result<PullRequestLabelCandidateList, String>>);
    let pending_name = signal(None::<String>);
    let list = use_context::<super::list::ListRefresh>();

    let load_session = session.clone();
    effect(move || {
        if !handle.is_open() {
            return;
        }
        let session = load_session.clone();
        watch(async move {
            let result = session
                .label_candidates()
                .await
                .map_err(|e| e.into_client_error().to_string());
            loaded.try_set(Some(result));
        });
    });

    let search_label = t("Search labels");
    let trigger = popover_trigger(
        handle,
        label,
        move || !allowed || !writable.get(),
        icon_with_class(Icon::Tag, "size-3.5"),
    );

    popover(handle, PopoverOptions::default(), trigger, move || {
        let search_placeholder = search_label.clone();
        let search_aria = search_label.clone();
        let session = session.clone();
        let rows = dynamic(move || {
            let q = query.get();
            let loaded_res = loaded.get();
            let mut container = view().app_class(tw("flex flex-col gap-1"));
            match &loaded_res {
                None => {
                    container = container.child(
                        text(t("Loading…")).app_class(tw("p-2 text-xs text-muted-foreground")),
                    );
                }
                Some(Err(err)) => {
                    container = container.child(
                        text(format!(
                            "{} {}",
                            t("The labels could not be read."),
                            err
                        ))
                        .app_class(tw("p-2 text-xs text-muted-foreground")),
                    );
                }
                Some(Ok(list_data)) => {
                    let matches: Vec<_> = list_data
                        .candidates
                        .iter()
                        .filter(|c| candidate_matches_label(c, &q))
                        .cloned()
                        .collect();
                    if matches.is_empty() {
                        let msg = if q.trim().is_empty() {
                            t("This repository has no labels.")
                        } else {
                            t("No label matches that.")
                        };
                        container = container.child(
                            text(msg).app_class(tw("p-2 text-xs text-muted-foreground")),
                        );
                    } else {
                        for candidate in matches {
                            let name = candidate.name.0.clone();
                            let name_trimmed = candidate.name.clone();
                            let next_applied = !candidate.is_applied;
                            let session = session.clone();
                            let title = match candidate.description.as_deref() {
                                Some(desc) if !desc.trim().is_empty() => format!("{name} · {desc}"),
                                _ => name.clone(),
                            };
                            container = container.child(
                                nsh_base::button()
                                    .accessibility_label(name.clone())
                                    .disabled(move || {
                                        !can_write
                                            || !writable.get()
                                            || pending_name.with(Option::is_some)
                                    })
                                    .on_press(move || {
                                        if !session.can_write()
                                            || !writable.get_untracked()
                                            || pending_name.with_untracked(Option::is_some)
                                        {
                                            return;
                                        }
                                        pending_name.set(Some(name.clone()));
                                        let session = session.clone();
                                        let name_val = name_trimmed.clone();
                                        let label_name = name.clone();
                                        let toasts = toast_manager();
                                        owner.spawn(async move {
                                            let result = session
                                                .set_labels(vec![name_val.clone()], next_applied)
                                                .await;
                                            pending_name.try_set(None);
                                            match result {
                                                Ok(()) => {
                                                    super::update_if_alive(loaded, |state| {
                                                        if let Some(Ok(data)) = state {
                                                            for entry in &mut data.candidates {
                                                                if entry.name == name_val {
                                                                    entry.is_applied = next_applied;
                                                                }
                                                            }
                                                        }
                                                    });
                                                    if let Some(list) = list {
                                                        super::update_if_alive(
                                                            list.revision,
                                                            |n| *n += 1,
                                                        );
                                                    }
                                                    if let Some(rev) = refresh.try_get() {
                                                        refresh.set(rev + 1);
                                                    }
                                                }
                                                Err(error) => {
                                                    let reason = super::rejection_reason(error);
                                                    let title = if next_applied {
                                                        tr(
                                                            "Could not put {name} on",
                                                            &[("name", label_name.into())],
                                                        )
                                                    } else {
                                                        tr(
                                                            "Could not take {name} off",
                                                            &[("name", label_name.into())],
                                                        )
                                                    };
                                                    toasts.add(ToastOptions {
                                                        variant: Variant::Error,
                                                        title: Text::content(title),
                                                        description: Some(Text::content(
                                                            readable_failure(
                                                                &reason,
                                                                &t("The host refused it. Check that you have triage access on this repository."),
                                                            ),
                                                        )),
                                                        ..Default::default()
                                                    });
                                                }
                                            }
                                        });
                                    })
                                    .app_class(tw(
                                        "flex w-full flex-row items-center gap-2 rounded-sm px-2 py-1 text-left text-xs hover:bg-accent",
                                    ))
                                    .child(text(title).app_class(tw("flex-1 truncate")))
                                    .child(
                                        candidate
                                            .is_applied
                                            .then(|| icon_with_class(Icon::Check, "size-3.5 shrink-0")),
                                    ),
                            );
                        }
                    }
                    if list_data.truncated {
                        container = container.child(
                            text(t(
                                "This repository has more labels than are listed here. Apply the rest on the host.",
                            ))
                            .app_class(tw("px-2 py-1 text-xs text-muted-foreground")),
                        );
                    }
                }
            }
            view()
                .app_class(tw("flex max-h-72 flex-col"))
                .child(crate::ui::scroll_area::scroll_area(false, container))
        });

        popover_popup(
            handle,
            PopoverOptions::default(),
            view()
                .w(px(288.0))
                .app_class(tw("flex flex-col gap-1 p-2"))
                .child(
                    nsh_base::input()
                        .value(move || query.get())
                        .on_change(move |value: &str| query.set(value.to_owned()))
                        .placeholder(move || search_placeholder.clone())
                        .label(move || search_aria.clone())
                        .app_class(tw(
                            "h-8 w-full rounded-md border border-input bg-transparent px-2 text-xs text-foreground outline-none",
                        )),
                )
                .child(rows),
        )
        .into_element()
    })
    .into_element()
}

/// Renders the Reviewers and Labels metadata rows (with inline pickers) at the top of Summary.
pub(super) fn summary_metadata(
    pr: &Rc<PullRequestDetail>,
    activity: Signal<Option<Rc<PullRequestActivity>>>,
    session: PullRequestReviewSession,
    writable: Signal<bool>,
    refresh: Signal<u64>,
) -> AnyElement {
    let pr_chips = pr.clone();
    let chips = dynamic(move || {
        let act = activity.get();
        let (comments, commits) = match act.as_deref() {
            Some(a) => (a.comments.as_slice(), a.commits.as_slice()),
            None => (&[][..], &[][..]),
        };
        let entries = reviewer_entries(&pr_chips, comments, commits);
        let mut list = view().app_class(tw("flex min-w-0 flex-wrap items-center gap-1.5"));
        if entries.is_empty() {
            list = list.child(text(t("None")).app_class(tw("text-xs text-muted-foreground")));
        } else {
            for entry in entries {
                let login = entry
                    .actor
                    .as_ref()
                    .map_or_else(|| t("ghost"), |a| a.login.0.clone());
                let mut label = match entry.actor.as_ref().and_then(|a| a.name.as_deref()) {
                    Some(name) if !name.is_empty() && name != login => format!("{name} (@{login})"),
                    _ => login,
                };
                if let Some(outcome) = entry.outcome {
                    let status = match (outcome, entry.stale) {
                        (t3_client::workspace::pr_views::detail::ReviewOutcome::Approved, true) => {
                            t("Approved before latest push")
                        }
                        (
                            t3_client::workspace::pr_views::detail::ReviewOutcome::Approved,
                            false,
                        ) => t("Approved"),
                        (
                            t3_client::workspace::pr_views::detail::ReviewOutcome::ChangesRequested,
                            true,
                        ) => t("Requested changes before latest push"),
                        (
                            t3_client::workspace::pr_views::detail::ReviewOutcome::ChangesRequested,
                            false,
                        ) => t("Changes requested"),
                        (t3_client::workspace::pr_views::detail::ReviewOutcome::Dismissed, _) => {
                            t("Dismissed")
                        }
                    };
                    label.push_str(": ");
                    label.push_str(&status);
                }
                list = list.child(
                    text(label).app_class(tw("rounded-sm bg-muted/60 px-1.5 py-0.5 text-xs")),
                );
            }
        }
        list
    });
    let mut reviewer_chips = view()
        .app_class(tw("flex min-w-0 flex-wrap items-center gap-1.5"))
        .child(chips);
    if pr.capabilities.reviewers.request && pr.capabilities.reviewers.list_candidates {
        reviewer_chips =
            reviewer_chips.child(reviewer_picker(pr, session.clone(), writable, refresh));
    }

    let mut rows = view().app_class(tw("flex min-w-0 flex-col gap-2")).child(
        view()
            .app_class(tw("flex flex-row items-center gap-3 text-xs"))
            .child(
                view()
                    .w(px(96.0))
                    .app_class(tw(
                        "flex shrink-0 flex-row items-center gap-1.5 text-muted-foreground",
                    ))
                    .child(icon_with_class(Icon::Users, "size-3.5"))
                    .child(text(t("Reviewers"))),
            )
            .child(reviewer_chips),
    );

    if !pr.labels.is_empty() || pr.capabilities.labels == Some(Some(true)) {
        let mut label_chips = view().app_class(tw("flex min-w-0 flex-wrap items-center gap-1"));
        if pr.labels.is_empty() {
            label_chips =
                label_chips.child(text(t("None")).app_class(tw("text-xs text-muted-foreground")));
        } else {
            for label in &pr.labels {
                label_chips = label_chips.child(
                    text(label.name.0.clone())
                        .app_class(tw("rounded-sm bg-muted/60 px-1.5 py-0.5 text-xs")),
                );
            }
        }
        if pr.capabilities.labels == Some(Some(true)) {
            label_chips = label_chips.child(label_picker(pr, session, writable, refresh));
        }
        rows = rows.child(
            view()
                .app_class(tw("flex flex-row items-center gap-3 text-xs"))
                .child(
                    view()
                        .w(px(96.0))
                        .app_class(tw(
                            "flex shrink-0 flex-row items-center gap-1.5 text-muted-foreground",
                        ))
                        .child(icon_with_class(Icon::Tag, "size-3.5"))
                        .child(text(t("Labels"))),
                )
                .child(label_chips),
        );
    }

    rows.into_element()
}
