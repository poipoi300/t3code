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
    workspace::pr_views::{
        detail::{describe_checks, group_checks, resolve_legacy_merge_method_override},
        service::PullRequestReviewSession,
    },
};
use t3_contracts::{base_schemas::TrimmedNonEmptyString, pull_request::*};

type Read<T> =
    Rc<dyn Fn(EnvironmentId, PullRequestRef) -> LocalBoxFuture<'static, Result<T, String>>>;

type ReviewSession = PullRequestReviewSession;
type ReviewSessionFactory = Rc<dyn Fn(&EnvironmentId, PullRequestRef) -> ReviewSession>;
type MergeMethodRead = Rc<dyn Fn() -> LocalBoxFuture<'static, PullRequestMergeMethod>>;
type MergeMethodSave =
    Rc<dyn Fn(PullRequestMergeMethod) -> LocalBoxFuture<'static, Result<(), String>>>;
type LegacyDefaultMergeFn = Rc<
    dyn Fn(
        &[t3_client::workspace::projects::EnvironmentProject],
        &EnvironmentId,
        &t3_client::types::ProjectId,
    ) -> Option<PullRequestMergeMethod>,
>;

#[derive(Clone)]
struct Reads {
    detail: Read<PullRequestDetail>,
    review: ReviewSessionFactory,
    media: super::markdown::Media,
    links: Read<PullRequestLinkedThreadsResult>,
    read_merge_method: MergeMethodRead,
    save_merge_method: MergeMethodSave,
    legacy_default_merge_method: LegacyDefaultMergeFn,
}

pub fn registration<P: Platform + Clone + 'static>(client: &ClientRuntime<P>) -> PanelRegistration {
    let detail_client = client.clone();
    let link_client = client.clone();
    let review_client = client.clone();
    let read_client = client.clone();
    let save_client = client.clone();
    let legacy_client = client.clone();
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
        read_merge_method: Rc::new(move || {
            let client = read_client.clone();
            async move {
                client
                    .pull_request_merge_method()
                    .await
                    .unwrap_or(PullRequestMergeMethod::Merge)
            }
            .boxed_local()
        }),
        save_merge_method: Rc::new(move |method| {
            let client = save_client.clone();
            async move {
                client
                    .set_pull_request_merge_method(method)
                    .await
                    .map_err(|error| error.to_string())
            }
            .boxed_local()
        }),
        legacy_default_merge_method: Rc::new(move |projects, env, project| {
            let settings = legacy_client.settings().client_settings();
            resolve_legacy_merge_method_override(&settings, projects, env, project)
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
        let select_env = env.clone();
        let select_ref = reference.clone();
        let on_select_layer: Rc<dyn Fn(u64)> = Rc::new(move |number| {
            host.apply(
                &key.get(),
                Action::OpenSurface(crate::screens::panel::Surface::pull_request(
                    Some(&select_env.0),
                    &select_ref.project_id.0,
                    select_ref
                        .host
                        .as_ref()
                        .and_then(Option::as_ref)
                        .map(|h| h.0.as_str()),
                    &select_ref.repository.0,
                    number,
                )),
            );
        });
        load(reads.clone(), env, reference, on_select_layer, move || {
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
    on_select_layer: Rc<dyn Fn(u64)>,
    back: impl Fn() + Copy + 'static,
) -> AnyElement {
    let revision = signal(0u64);
    let loaded_detail = signal(None::<Rc<PullRequestDetail>>);
    let detail_error = signal(None::<String>);
    let linked = signal(None::<Result<PullRequestLinkedThreadsResult, String>>);
    let remembered_method = signal(PullRequestMergeMethod::Merge);
    let remembered_touched = signal(false);
    let read_merge = (reads.read_merge_method)();
    watch(async move {
        let loaded = read_merge.await;
        if !remembered_touched.with_untracked(|touched| *touched) {
            remembered_method.try_set(loaded);
        }
    });
    let thread_env = env.clone();
    let review_env = env.clone();
    let session = (reads.review)(&env, reference.clone());
    let conversation = super::review::Conversation::new(session.clone());
    let links_env = env.clone();
    let links_ref = reference.clone();
    let runtime = crate::runtime::rt();
    let config = runtime.store.environment_config(env.clone());
    let shell = runtime.store.shell(env.clone());
    let legacy_resolver = reads.legacy_default_merge_method.clone();
    let project = reference.project_id.clone();
    let env_for_default = env.clone();
    let default_method = computed(move || {
        let scoped = config.with(|c| {
            c.as_ref().and_then(|c| {
                t3_client::services::settings::scoped::resolve_project_settings(
                    &Rc::new(c.0.settings.clone()),
                    Some(&project),
                )
                .settings
                .pull_request_merge_method
                .clone()
            })
        });
        if scoped.is_some() {
            return scoped;
        }
        let scratch = config.with(|c| {
            c.as_ref()
                .and_then(|c| c.0.scratch_workspace_root.as_ref())
                .map(|r| r.0.clone())
        });
        let env_projects = shell.with(|s| {
            s.as_ref()
                .map(|s| {
                    s.projects
                        .iter()
                        .map(|p| t3_client::workspace::projects::EnvironmentProject {
                            environment: env_for_default.clone(),
                            project: p.as_ref().clone(),
                            is_scratch:
                                t3_client::workspace::git_panel::details::is_scratch_project(
                                    &p.workspace_root.0,
                                    scratch.as_deref(),
                                ),
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        });
        (legacy_resolver)(&env_projects, &env_for_default, &project)
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
        let result = (reads.detail)(env.clone(), reference.clone());
        watch(async move {
            match result.await {
                Ok(pr) => {
                    detail_error.try_set(None);
                    loaded_detail.try_set(Some(Rc::new(pr)));
                }
                Err(err) => {
                    detail_error.try_set(Some(err));
                }
            }
        });
    });
    effect(move || {
        links_revision.get();
        linked.set(None);
        if !supports_links.get() {
            return;
        }
        let Some(pr) = loaded_detail.get() else {
            return;
        };
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
    let ready = computed(move || {
        if loaded_detail.with(Option::is_some) {
            1u8
        } else if detail_error.with(Option::is_some) {
            2u8
        } else {
            0u8
        }
    });
    let retry = move || {
        detail_error.set(None);
        revision.update(|n| *n += 1);
    };
    let nav = navigator();
    scroll()
        .flex_1()
        .min_h(px(0.))
        .child(
            view()
                .app_class(tw("flex min-w-0 flex-col gap-3 p-4"))
                .child(button(t("Back to pull requests"), back))
                .child(dynamic(move || {
                    if loaded_detail.with(Option::is_none) {
                        return view().into_element();
                    }
                    let Some(err) = detail_error.get() else {
                        return view().into_element();
                    };
                    view()
                        .app_class(tw(
                            "flex flex-row flex-wrap items-center justify-between gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive",
                        ))
                        .child(
                            view()
                                .app_class(tw("flex min-w-0 flex-1 flex-col gap-0.5"))
                                .child(text(t("Could not refresh pull request")))
                                .child(
                                    (!err.is_empty()).then(|| {
                                        text(err).app_class(tw("text-muted-foreground"))
                                    }),
                                ),
                        )
                        .child(
                            nsh_base::button()
                                .accessibility_label(t("Retry pull request refresh"))
                                .child(text(t("Retry")))
                                .on_press(retry),
                        )
                        .into_element()
                }))
                .child(dynamic(move || match ready.get() {
                    1 => {
                        let initial = loaded_detail.with_untracked(|p| p.clone()).expect("loaded");
                        let markdown = reads.media.scope(review_env.clone(), &initial);
                        detail_tabs(
                            loaded_detail,
                            session.clone(),
                            tab,
                            conversation,
                            markdown,
                            PanelOptions {
                                revision,
                                guides,
                                supports_thread_prs: supports_links,
                                default_method,
                                remembered_method,
                                remembered_touched,
                                save_merge_method: reads.save_merge_method.clone(),
                                on_select_layer: on_select_layer.clone(),
                            },
                        )
                    }
                    2 => view()
                        .child(text(t("Could not load pull request")))
                        .child(button(t("Retry"), retry))
                        .into_element(),
                    _ => text(t("Loading pull request…")).into_element(),
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
#[derive(Clone)]
struct PanelOptions {
    revision: Signal<u64>,
    guides: Memo<bool>,
    supports_thread_prs: Memo<bool>,
    default_method: Memo<Option<PullRequestMergeMethod>>,
    remembered_method: Signal<PullRequestMergeMethod>,
    remembered_touched: Signal<bool>,
    save_merge_method: MergeMethodSave,
    on_select_layer: Rc<dyn Fn(u64)>,
}
fn detail_tabs(
    pr_detail: Signal<Option<Rc<PullRequestDetail>>>,
    session: ReviewSession,
    tab: Signal<Tab>,
    conversation: super::review::Conversation,
    markdown: super::markdown::PrMarkdown,
    options: PanelOptions,
) -> AnyElement {
    let initial_pr = pr_detail.with_untracked(|p| p.clone()).expect("loaded");
    let owner = crate::runtime::tasks::TaskOwner::here();
    let armed = signal(None);
    let activity = signal(None::<Rc<PullRequestActivity>>);
    let activity_session = session.clone();
    effect(move || {
        let _rev = options.revision.get();
        let session = activity_session.clone();
        watch(async move {
            if let Ok(value) = session.activity().await {
                activity.try_set(Some(Rc::new(value)));
            }
        });
    });
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
        if value != Tab::Code || initial_pr.capabilities.diff {
            tabs = tabs.child(button(t(label), move || select_tab(value)));
        }
    }
    tabs = tabs.child(dynamic(move || {
        options
            .guides
            .get()
            .then(|| button(t("Guide"), move || select_tab(Tab::Guide)))
    }));
    let host_label = super::host_display_label(&initial_pr.provider.0);
    let header_session = session.clone();
    let header_view = dynamic(move || {
        let Some(pr) = pr_detail.get() else {
            return view().into_element();
        };
        let base_branch = super::pickers::base_branch_indicator(
            pr.clone(),
            header_session.clone(),
            conversation.writable,
            options.revision,
            owner,
        );
        header(&pr, base_branch)
    });
    let controls_session = session.clone();
    let controls_host = host_label.clone();
    let controls_options = options.clone();
    let controls = dynamic(move || {
        let Some(pr) = pr_detail.get() else {
            return view().into_element();
        };
        super::actions::controls(super::actions::ControlsInput {
            pr,
            client: controls_session.clone(),
            writable: conversation.writable,
            refresh: controls_options.revision,
            supports_thread_prs: controls_options.supports_thread_prs,
            default: controls_options.default_method,
            remembered: controls_options.remembered_method,
            remembered_touched: controls_options.remembered_touched,
            save_method: controls_options.save_merge_method.clone(),
            armed,
            owner,
            host_label: controls_host.clone(),
            on_select_layer: controls_options.on_select_layer.clone(),
        })
    });
    let composer = super::review::panel_composer(session.clone(), conversation, &initial_pr);
    view()
        .app_class(tw("flex min-w-0 flex-col gap-2"))
        .child(header_view)
        .child(controls)
        .child(tabs)
        .child(dynamic(move || {
            if tab.get() == Tab::Summary {
                return view()
                    .app_class(tw("flex min-w-0 flex-col gap-3"))
                    .child(super::pickers::summary_metadata(
                        pr_detail,
                        activity,
                        session.clone(),
                        conversation.writable,
                        options.revision,
                        owner,
                    ))
                    .child(dynamic({
                        let markdown = markdown.clone();
                        move || {
                            let Some(pr) = pr_detail.get() else {
                                return view().into_element();
                            };
                            description(&pr, &markdown)
                        }
                    }))
                    .into_element();
            }
            let session = session.clone();
            let pr = pr_detail
                .with_untracked(|p| p.clone())
                .unwrap_or_else(|| initial_pr.clone());
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

fn header(pr: &PullRequestDetail, base_branch: AnyElement) -> AnyElement {
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
                .child(
                    view()
                        .app_class(tw("flex flex-row items-center gap-1"))
                        .child(text(pr.head_branch.0.clone()))
                        .child(text(t("→")))
                        .child(base_branch),
                )
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
