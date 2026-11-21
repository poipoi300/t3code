//! Whole-PR diff pages and per-file host/environment viewed state.
use crate::{
    i18n::{t, tr},
    runtime::tasks::{TaskOwner, watch},
    screens::panel::diff::{diff_panel_loading_state, diff_stat_label},
    theme::tw,
};
use nsh_core::prelude::*;
use std::{collections::BTreeMap, rc::Rc};
use t3_client::workspace::{
    diff_panel::files::DiffLineStat,
    pr_views::{detail::diff_file_revisions, service::PullRequestReviewSession},
};
use t3_contracts::{base_schemas::TrimmedNonEmptyString, pull_request::*};

#[derive(Default, Clone, PartialEq)]
struct Pages {
    patch: String,
    next: Option<TrimmedNonEmptyString>,
    omitted: Vec<String>,
    truncated: bool,
    loaded: bool,
    revisions: BTreeMap<String, String>,
}
#[derive(Clone, Copy)]
struct Viewed {
    rows: Signal<Option<Rc<PullRequestFilesViewedResult>>>,
    pending: Signal<bool>,
    writable: Signal<bool>,
    error: Signal<Option<String>>,
    refresh: Signal<u64>,
    owner: TaskOwner,
}
pub(super) fn files(
    session: PullRequestReviewSession,
    writable: Signal<bool>,
    provider: String,
    viewed_enabled: bool,
) -> AnyElement {
    let pages = signal(Rc::new(Pages::default()));
    let pending = signal(true);
    let request = signal(0u64);
    let error = signal(None::<(String, String)>);
    let name = match provider.as_str() {
        "github" => "GitHub",
        "gitlab" => "GitLab",
        "forgejo" => "Forgejo",
        "bitbucket" => "Bitbucket",
        "azure-devops" => "Azure DevOps",
        other => other,
    }
    .to_owned();
    let term = if provider == "gitlab" {
        "merge request"
    } else {
        "pull request"
    };
    let viewed = Viewed {
        writable,
        rows: signal(None),
        pending: signal(false),
        error: signal(None),
        refresh: signal(0),
        owner: TaskOwner::here(),
    };
    let client = session.clone();
    effect(move || {
        request.get();
        let cursor = untracked(|| pages.get().next.clone());
        pending.set(true);
        error.set(None);
        let client = client.clone();
        let name = name.clone();
        watch(async move {
            match client.diff(cursor.clone()).await {
                Ok(page) => pages.update(|shared| {
                    let data = Rc::make_mut(shared);
                    data.revisions.extend(diff_file_revisions(&page.patch));
                    if !data.patch.is_empty() && !data.patch.ends_with('\n') {
                        data.patch.push('\n');
                    }
                    data.patch.push_str(&page.patch);
                    data.truncated |= page.truncated;
                    data.next = page.next_cursor.filter(|c| Some(c) != cursor.as_ref());
                    data.omitted.extend(
                        page.omitted_file_stats
                            .flatten()
                            .unwrap_or_default()
                            .into_iter()
                            .map(|s| s.path.0),
                    );
                    data.loaded = true;
                }),
                Err(e) => error.set(Some(
                    t3_client::workspace::pr_views::diff_http::describe_diff_error(&e, &name, term),
                )),
            }
            pending.set(false);
        });
    });
    if viewed_enabled {
        let client = session.clone();
        effect(move || {
            viewed.refresh.get();
            let client = client.clone();
            watch(async move {
                match client.files_viewed().await {
                    Ok(rows) => {
                        viewed.rows.set(Some(Rc::new(rows)));
                        viewed.error.set(None);
                    }
                    Err(_) => {
                        viewed.error.set(Some(t("Your ticks could not be read")));
                    }
                }
            });
        });
    }
    let diff = component_ref::<nsh_base::DiffView>();
    let load = move || {
        if !pending.get() {
            request.update(|n| *n += 1);
        }
    };
    view()
        .app_class(tw("flex min-w-0 flex-col gap-2"))
        .child(dynamic(move || {
            error.get().map(|(title, description)| {
                view()
                    .child(text(title))
                    .child(text(description))
                    .child(super::detail::button(t("Retry"), load))
            })
        }))
        .child(dynamic(move || {
            viewed.error.get().map(|e| {
                view()
                    .child(text(e))
                    .child(super::detail::button(t("Retry viewed files"), move || {
                        viewed.refresh.update(|n| *n += 1)
                    }))
            })
        }))
        .child(dynamic(move || {
            pending
                .get()
                .then(|| diff_panel_loading_state(t("Loading changed files…")))
        }))
        .child(dynamic(move || {
            let page = pages.get();
            if !page.loaded {
                return view().into_element();
            }
            let mut body = view().app_class(tw("flex min-w-0 flex-col gap-1"));
            if page.patch.trim().is_empty() && page.omitted.is_empty() {
                body = body.child(text(t("No changed files")));
            } else if !page.patch.trim().is_empty() {
                let session = session.clone();
                let data = page.clone();
                body = body.child(
                    nsh_base::diff_view()
                        .patch(page.patch.clone())
                        .layout(nsh_base::DiffLayout::Unified)
                        .inline(nsh_base::DiffInline::None)
                        .soft_wrap(true)
                        .render_file_header(move |file: &nsh_base::DiffFile| {
                            let path = file.path.to_string();
                            let toggle_path = path.clone();
                            let mut header = view()
                                .app_class(tw("flex min-w-0 flex-row flex-wrap items-center gap-2"))
                                .child(super::detail::button(path.clone(), move || {
                                    use nsh_base::DiffViewApi;
                                    if let Some(api) = diff.api::<DiffViewApi>() {
                                        api.toggle_file(toggle_path.clone());
                                    }
                                }))
                                .child(diff_stat_label(
                                    DiffLineStat {
                                        additions: u64::from(file.added),
                                        deletions: u64::from(file.deleted),
                                    },
                                    "",
                                ));
                            if viewed_enabled {
                                let revision = data.revisions.get(&path).cloned();
                                header = header.child(viewed_button(
                                    session.clone(),
                                    viewed,
                                    path,
                                    revision,
                                ));
                            }
                            header
                        })
                        .component_ref(&diff)
                        .w_full()
                        .h(px(380.)),
                );
            }
            for path in &page.omitted {
                body = body.child(text(format!("{} · {}", path, t("Diff omitted"))));
            }
            if page.next.is_some() {
                body = body.child(
                    nsh_base::button()
                        .accessibility_label(t("Load more files"))
                        .child(text(t("Load more files")))
                        .disabled(move || pending.get())
                        .on_press(load),
                );
            } else if page.truncated {
                body = body.child(text(t("Diff truncated")));
            }
            body.into_element()
        }))
        .into_element()
}
fn viewed_button(
    session: PullRequestReviewSession,
    state: Viewed,
    path: String,
    revision: Option<String>,
) -> AnyElement {
    dynamic(move || {
        let Some(rows) = state.rows.get() else {
            return view().into_element();
        };
        let record = rows
            .files
            .iter()
            .find(|row| row.path == path)
            .map(|row| &row.state);
        let checked = record == Some(&PullRequestFileViewedState::Viewed);
        let caption = t(if checked {
            "Viewed"
        } else if record == Some(&PullRequestFileViewedState::Dismissed) {
            "Changed"
        } else {
            "Unviewed"
        });
        let label = tr(
            if checked {
                "Mark {path} as unviewed"
            } else {
                "Mark {path} as viewed"
            },
            &[("path", path.as_str().into())],
        );
        let session = session.clone();
        let path = path.clone();
        let revision = revision.clone();
        nsh_base::button()
            .accessibility_label(label)
            .child(text(caption))
            .disabled(move || state.pending.get() || !state.writable.get())
            .on_press(move || {
                if state.pending.get() || !session.can_write() {
                    return;
                }
                let file = PullRequestSetFilesViewedInputFiles {
                    path: path.clone(),
                    viewed: !checked,
                    revision: revision
                        .clone()
                        .map(|value| Some(TrimmedNonEmptyString(value))),
                };
                state.pending.set(true);
                state.error.set(None);
                let client = session.clone();
                state.owner.spawn(async move {
                    match client.set_viewed(file).await {
                        Ok(()) => match client.files_viewed().await {
                            Ok(rows) => state.rows.set(Some(Rc::new(rows))),
                            Err(_) => {
                                state.error.set(Some(t("Your ticks could not be read")));
                            }
                        },
                        Err(_) => super::review::feedback_error("Could not update viewed files"),
                    }
                    state.pending.set(false);
                });
            })
            .into_element()
    })
    .into_element()
}
