//! Read-only summary from PullRequestDetailPanel, PullRequestSummaryTab and PullRequestChecksPopover.
use std::rc::Rc;

use crate::{
    i18n::{t, tr},
    runtime::{services::links::external_link, tasks::watch},
    screens::panel::{Action, PanelRegistration, SurfaceContent, SurfaceContext, SurfaceKind},
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

type ReviewSession = t3_client::workspace::pr_views::service::PullRequestReviewSession;
type ReviewSessionFactory = Rc<dyn Fn(&EnvironmentId, PullRequestRef) -> ReviewSession>;

#[derive(Clone)]
struct Reads {
    detail: Read<PullRequestDetail>,
    review: ReviewSessionFactory,
    media: super::markdown::Media,
    links: Read<PullRequestLinkedThreadsResult>,
}

pub fn registration<P: Platform + Clone + 'static>(client: &ClientRuntime<P>) -> PanelRegistration {
    let detail_client = client.clone();
    let link_client = client.clone();
    let review_client = client.clone();
    let reads = Reads {
        media: super::markdown::Media::new(client),
        review: Rc::new(move |env, reference| {
            review_client.pull_request_review_session(env, reference)
        }),
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

pub(super) fn button(label: String, action: impl Fn() + 'static) -> AnyElement {
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
    let review_env = env.clone();
    let session = (reads.review)(&env, reference.clone());
    let conversation = super::review::Conversation::new(session.clone());
    let links_env = env.clone();
    let links_ref = reference.clone();
    let config = crate::runtime::rt().store.environment_config(env.clone());
    let project = reference.project_id.clone();
    let default_method = computed(move || {
        config.with(|c| {
            c.as_ref().and_then(|c| {
                t3_client::services::settings::scoped::resolve_project_settings(
                    &Rc::new(c.0.settings.clone()),
                    Some(&project),
                )
                .settings
                .pull_request_merge_method
                .clone()
            })
        })
    });
    let guides = computed(move || {
        config.with(|c| {
            c.as_ref()
                .is_some_and(|c| c.0.settings.review_guides_enabled)
        })
    });
    let supports_links = computed(move || config.with(|config| config.is_some()));
    let links_revision = signal(0u64);
    let tab = signal(Tab::Summary);
    effect(move || {
        revision.get();
        detail.set(None);
        let result = (reads.detail)(env.clone(), reference.clone());
        watch(async move {
            detail.set(Some(result.await.map(Rc::new)));
        });
    });
    effect(move || {
        links_revision.get();
        linked.set(None);
        if !supports_links.get() {
            return;
        }
        let Some(Ok(pr)) = detail.get() else { return };
        let Some(reference) =
            t3_client::workspace::pr_views::detail::linked_thread_reference(&pr, &links_ref)
        else {
            return;
        };
        let result = (reads.links)(links_env.clone(), reference);
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
                    Some(Ok(pr)) => {
                        let markdown = reads.media.scope(review_env.clone(), &pr);
                        detail_tabs(
                            pr,
                            session.clone(),
                            tab,
                            conversation,
                            markdown,
                            PanelOptions {
                                revision,
                                guides,
                                default_method,
                            },
                        )
                    }
                    Some(Err(_)) => view()
                        .child(text(t("Could not load pull request")))
                        .child(button(t("Retry"), retry))
                        .into_element(),
                    None => text(t("Loading pull request…")).into_element(),
                }))
                .child(dynamic(move || {
                    let mut body = view().app_class(tw("flex min-w-0 flex-col gap-1"));
                    if tab.get() != Tab::Summary {
                        return body;
                    }
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
                        Some(Err(_)) => {
                            body = body.child(text(t("Could not load linked threads"))).child(
                                nsh_base::button()
                                    .accessibility_label(t("Retry linked threads"))
                                    .child(text(t("Retry")))
                                    .on_press(move || links_revision.update(|n| *n += 1)),
                            );
                        }
                        _ => {}
                    }
                    body
                })),
        )
        .into_element()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Summary,
    Timeline,
    Code,
    Guide,
}
#[derive(Clone, Copy)]
struct PanelOptions {
    revision: Signal<u64>,
    guides: Memo<bool>,
    default_method: Memo<Option<PullRequestMergeMethod>>,
}
fn detail_tabs(
    pr: Rc<PullRequestDetail>,
    session: ReviewSession,
    tab: Signal<Tab>,
    conversation: super::review::Conversation,
    markdown: super::markdown::PrMarkdown,
    options: PanelOptions,
) -> AnyElement {
    let armed = signal(None);
    let select_tab = move |value| {
        armed.set(None);
        tab.set(value);
    };
    let mut tabs = view().app_class(tw("flex flex-row flex-wrap gap-2"));
    for (value, label) in [
        (Tab::Summary, "Summary"),
        (Tab::Timeline, "Timeline"),
        (Tab::Code, "Code"),
    ] {
        if value != Tab::Code || pr.capabilities.diff {
            tabs = tabs.child(button(t(label), move || select_tab(value)));
        }
    }
    tabs = tabs.child(dynamic(move || {
        options
            .guides
            .get()
            .then(|| button(t("Guide"), move || select_tab(Tab::Guide)))
    }));
    let controls = super::actions::controls(
        pr.clone(),
        session.clone(),
        conversation.writable,
        options.revision,
        options.default_method,
        armed,
    );
    let composer = super::review::panel_composer(session.clone(), conversation, &pr);
    view()
        .app_class(tw("flex min-w-0 flex-col gap-2"))
        .child(header(&pr))
        .child(controls)
        .child(tabs)
        .child(dynamic(move || {
            if tab.get() == Tab::Summary {
                return description(&pr, &markdown);
            }
            let session = session.clone();
            match tab.get() {
                Tab::Timeline => {
                    super::review::timeline(session, &pr, conversation, markdown.clone())
                }
                Tab::Code => super::files::files(
                    session,
                    conversation.writable,
                    pr.provider.0.clone(),
                    pr.capabilities
                        .viewed_files
                        .as_ref()
                        .and_then(Option::as_ref)
                        .is_some(),
                ),
                Tab::Guide if options.guides.get() => {
                    super::guide::guide(session, markdown.clone(), move || tab.set(Tab::Code))
                }
                Tab::Guide => description(&pr, &markdown),
                Tab::Summary => unreachable!(),
            }
        }))
        .child(composer)
        .into_element()
}

fn header(pr: &PullRequestDetail) -> AnyElement {
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
    let host_label = tr(
        "Open {term} {number} on host",
        &[("term", t(term).into()), ("number", number.clone().into())],
    );
    let repo = match t3_client::workspace::pr_views::detail::repository_link(&pr.url.0, pr.number) {
        Some(url) => link(pr.repository.0.clone(), pr.repository.0.clone(), url),
        None => text(pr.repository.0.clone()).into_element(),
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
                .child(text(tr(
                    if pr.changed_files == 1 {
                        "{count} file"
                    } else {
                        "{count} files"
                    },
                    &[("count", pr.changed_files.to_string().into())],
                ))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(tr(
                    "Checks: {label}",
                    &[("label", check_label.clone().into())],
                ))
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
        .into_element()
}
fn description(pr: &PullRequestDetail, markdown: &super::markdown::PrMarkdown) -> AnyElement {
    let body = if pr.body.trim().is_empty() {
        format!("_{}_", t("No description provided."))
    } else {
        pr.body.clone()
    };
    view()
        .app_class(tw("flex min-w-0 flex-col gap-2"))
        .child(text(t("Description")).app_class(tw("text-sm font-semibold")))
        .child(markdown.render(&body))
        .into_element()
}
