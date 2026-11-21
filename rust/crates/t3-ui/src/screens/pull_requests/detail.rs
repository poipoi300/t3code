//! Read-only summary from PullRequestDetailPanel, PullRequestSummaryTab and PullRequestChecksPopover.
use std::rc::Rc;

use crate::{
    i18n::t,
    runtime::{services::links::external_link, tasks::watch},
    screens::{
        panel::{Action, PanelRegistration, SurfaceContent, SurfaceContext, SurfaceKind},
        thread::markdown::base::chat_markdown,
    },
    state::nav::{Route, navigator},
    theme::tw,
};
use futures::{FutureExt, future::LocalBoxFuture};
use nsh_core::prelude::*;
use t3_client::{
    composition::ClientRuntime,
    platform::Platform,
    types::{EnvironmentId, ThreadKey},
    workspace::pr_views::detail::{describe_checks, group_checks},
};
use t3_contracts::{base_schemas::TrimmedNonEmptyString, pull_request::*};

type Read<T> =
    Rc<dyn Fn(EnvironmentId, PullRequestRef) -> LocalBoxFuture<'static, Result<T, String>>>;

#[derive(Clone)]
struct Reads {
    detail: Read<PullRequestDetail>,
    links: Read<PullRequestLinkedThreadsResult>,
}

pub fn registration<P: Platform + Clone + 'static>(client: &ClientRuntime<P>) -> PanelRegistration {
    let detail_client = client.clone();
    let link_client = client.clone();
    let reads = Reads {
        detail: Rc::new(move |env, reference| {
            let client = detail_client.clone();
            async move {
                client
                    .pull_request_detail(&env, reference)
                    .await
                    .map_err(|e| e.into_client_error().to_string())
            }
            .boxed_local()
        }),
        links: Rc::new(move |env, reference| {
            let client = link_client.clone();
            async move {
                client
                    .pull_request_linked_threads(&env, reference)
                    .await
                    .map_err(|e| e.into_client_error().to_string())
            }
            .boxed_local()
        }),
    };
    let nav = navigator();
    PanelRegistration::new(
        SurfaceKind::PullRequest,
        Prop::new(None),
        move |_| nav.open(Route::PullRequests),
        move |context| surface(context, reads.clone()),
    )
}

fn surface(context: SurfaceContext, reads: Reads) -> AnyElement {
    let host = context.host;
    let key = stored(context.thread);
    let target = computed(move || {
        let surface = context.surface.get()?;
        let SurfaceContent::PullRequest {
            environment_id,
            project_id,
            host,
            repository,
            number,
            ..
        } = surface.content
        else {
            return None;
        };
        let env = environment_id
            .map(EnvironmentId)
            .unwrap_or_else(|| key.get().environment);
        Some((
            env,
            PullRequestRef {
                project_id: t3_client::types::ProjectId(project_id),
                host: host.map(|h| Some(TrimmedNonEmptyString(h))),
                repository: TrimmedNonEmptyString(repository),
                number,
                expected_account_id: None,
                allow_stale: None,
            },
        ))
    });
    dynamic(move || {
        let Some((env, reference)) = target.get() else {
            return view().into_element();
        };
        load(reads.clone(), env, reference, move || {
            host.apply(&key.get(), Action::Close)
        })
    })
    .into_element()
}

fn button(label: String, action: impl Fn() + 'static) -> AnyElement {
    nsh_base::button()
        .accessibility_label(label.clone())
        .child(text(label))
        .on_press(action)
        .into_element()
}

fn link(label: String, caption: String, url: String) -> AnyElement {
    external_link(move || Some(url.clone()), |_| {})
        .accessibility_label(label)
        .child(text(caption))
        .app_class(tw("text-sm text-muted-foreground underline"))
        .into_element()
}

fn load(
    reads: Reads,
    env: EnvironmentId,
    reference: PullRequestRef,
    back: impl Fn() + Copy + 'static,
) -> AnyElement {
    let revision = signal(0u64);
    let detail = signal(None::<Result<Rc<PullRequestDetail>, String>>);
    let linked = signal(None::<Result<PullRequestLinkedThreadsResult, String>>);
    let thread_env = env.clone();
    effect(move || {
        revision.get();
        detail.set(None);
        linked.set(None);
        let result = (reads.detail)(env.clone(), reference.clone());
        watch(async move {
            detail.set(Some(result.await.map(Rc::new)));
        });
        let result = (reads.links)(env.clone(), reference.clone());
        watch(async move {
            linked.set(Some(result.await));
        });
    });
    let retry = move || revision.update(|n| *n += 1);
    let nav = navigator();
    scroll()
        .flex_1()
        .min_h(px(0.))
        .child(
            view()
                .app_class(tw("flex min-w-0 flex-col gap-3 p-4"))
                .child(button(t("Back to pull requests"), back))
                .child(dynamic(move || match detail.get() {
                    Some(Ok(pr)) => summary(pr),
                    Some(Err(error)) => view()
                        .child(text(t("Could not load pull request")))
                        .child(text(error))
                        .child(button(t("Retry"), retry))
                        .into_element(),
                    None => text(t("Loading pull request…")).into_element(),
                }))
                .child(dynamic(move || {
                    let mut body = view().app_class(tw("flex min-w-0 flex-col gap-1"));
                    match linked.get() {
                        Some(Ok(result)) if !result.threads.is_empty() => {
                            body = body.child(text(t("Linked threads")));
                            for thread in result.threads {
                                let key = ThreadKey {
                                    environment: thread_env.clone(),
                                    thread: thread.id,
                                };
                                let title = if thread.title.is_empty() {
                                    t("Untitled thread")
                                } else {
                                    thread.title
                                };
                                body = body.child(button(title, move || {
                                    nav.open(Route::Thread(key.clone()))
                                }));
                            }
                        }
                        Some(Err(error)) => {
                            body = body
                                .child(text(t("Could not load linked threads")))
                                .child(text(error))
                                .child(button(t("Retry linked threads"), retry));
                        }
                        _ => {}
                    }
                    body
                })),
        )
        .into_element()
}

fn summary(pr: Rc<PullRequestDetail>) -> AnyElement {
    let state = match pr.state {
        PullRequestState::Merged => "Merged",
        PullRequestState::Closed => "Closed",
        _ if pr.is_draft => "Draft",
        _ => "Open",
    };
    let term = if pr.provider.0 == "gitlab" {
        "merge request"
    } else {
        "pull request"
    };
    let number = format!("#{}", pr.number);
    let host_label = format!("{} {} {number} {}", t("Open"), t(term), t("on host"));
    let repo = match t3_client::workspace::pr_views::detail::repository_link(&pr.url.0, pr.number) {
        Some(url) => link(pr.repository.0.clone(), pr.repository.0.clone(), url),
        None => text(pr.repository.0.clone()).into_element(),
    };
    let body = if pr.body.trim().is_empty() {
        format!("_{}_", t("No description provided."))
    } else {
        pr.body.clone()
    };
    let check_label = describe_checks(&pr.checks);
    let expanded = signal(false);
    let mut items = view().app_class(tw("flex min-w-0 flex-col gap-1"));
    let (attention, running, complete) = group_checks(&pr.checks);
    for check in attention.into_iter().chain(running).chain(complete) {
        let name = check.name.0.clone();
        let mut row = view().app_class(tw("flex min-w-0 flex-row items-center gap-2"));
        row = row.child(match &check.url {
            Some(url) => link(name.clone(), name, url.clone()),
            None => text(name).into_element(),
        });
        row = row.child(text(t(match check.status {
            PullRequestCheckStatus::Success => "Passed",
            PullRequestCheckStatus::Failure => "Failed",
            PullRequestCheckStatus::Pending => "Running",
            PullRequestCheckStatus::Cancelled => "Cancelled",
            PullRequestCheckStatus::ActionRequired => "Action required",
            PullRequestCheckStatus::Neutral => "Neutral",
            PullRequestCheckStatus::Skipped => "Skipped",
        })));
        if let Some(value) = &check.description {
            row = row.child(text(value.clone()));
        }
        items = items.child(row);
    }
    view()
        .app_class(tw("flex min-w-0 flex-col gap-2"))
        .child(
            view()
                .app_class(tw(
                    "flex flex-row items-center gap-1 text-sm text-muted-foreground",
                ))
                .child(repo)
                .child(link(host_label, number, pr.url.0.clone())),
        )
        .child(text(pr.title.0.clone()).app_class(tw("text-lg font-semibold")))
        .child(
            view()
                .app_class(tw("flex flex-row flex-wrap gap-2"))
                .child(text(t(state)))
                .child(text(
                    pr.author
                        .as_ref()
                        .map_or_else(|| t("ghost"), |a| a.login.0.clone()),
                ))
                .child(text(format!("{} → {}", pr.head_branch.0, pr.base_branch.0)))
                .child(text(format!("+{} −{}", pr.additions, pr.deletions)))
                .child(text(format!(
                    "{} {}",
                    pr.changed_files,
                    t(if pr.changed_files == 1 {
                        "file"
                    } else {
                        "files"
                    })
                ))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(format!("{}: {check_label}", t("Checks")))
                .child(text(check_label))
                .on_press(move || expanded.update(|v| *v = !*v)),
        )
        .child(
            view()
                .app_class(move || {
                    Some(tw(if expanded.get() {
                        "flex flex-col"
                    } else {
                        "hidden"
                    }))
                })
                .child(items),
        )
        .child(text(t("Description")).app_class(tw("text-sm font-semibold")))
        .child(chat_markdown(move || body.clone(), || false))
        .into_element()
}
