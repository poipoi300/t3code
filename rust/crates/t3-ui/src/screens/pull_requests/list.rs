//! Port of apps/web/src/components/pullRequest/PullRequestsPage.tsx and PullRequestsColumn.tsx.

use futures::{FutureExt, future::LocalBoxFuture};
use nsh_core::prelude::*;
use std::{collections::BTreeMap, rc::Rc, time::Duration};
use t3_client::{
    composition::ClientRuntime,
    platform::Platform,
    types::{EnvironmentId, ProjectId},
    workspace::{
        pr_list::{self, ListSort},
        pr_views::preferences::ListSearch,
    },
};
use t3_contracts::pull_request::*;

use crate::{
    i18n::{t, tr},
    runtime::{rt, tasks::watch},
    state::{
        clock,
        layout::layout_class,
        nav::navigator,
        url::{UrlRequest, UrlState},
    },
    theme::tw,
    ui::{
        icon::{Icon, icon_with_class},
        input::{InputSize, text_field},
        menu,
    },
};

type ListRead = dyn Fn(
    EnvironmentId,
    PullRequestListInput,
) -> LocalBoxFuture<'static, Result<PullRequestListResult, String>>;

#[derive(Clone)]
pub struct ListClient(Rc<ListRead>);

pub fn bind<P: Platform + Clone + 'static>(client: &ClientRuntime<P>) -> ListClient {
    let client = client.clone();
    ListClient(Rc::new(move |environment, input| {
        let client = client.clone();
        async move {
            client
                .pull_request_list(&environment, input)
                .await
                .map_err(|e| e.into_client_error().to_string())
        }
        .boxed_local()
    }))
}

use crate::screens::panel::{self, Action, PanelRegistry, Surface, SurfaceContent};
use t3_client::types::{ThreadId, ThreadKey};

/// The utility route uses the same persistent panel key as rightPanelStore.ts.
fn panel_key() -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("pull-requests-panel".into()),
        thread: ThreadId("pull-requests-panel".into()),
    }
}

pub fn supported() -> Prop<bool> {
    let runtime = rt();
    Prop::bind(move || {
        runtime.store.track_environments();
        let snapshot = runtime.store.store().snapshot();
        runtime.environments().iter().any(|entry| {
            snapshot
                .environments
                .get(&entry.id)
                .is_some_and(|s| s.config.is_some())
        })
    })
}

type Choices = &'static [(&'static str, &'static str, Icon)];
const STATE: Choices = &[
    ("all", "All", Icon::Layers),
    ("open", "Open", Icon::GitPullRequest),
    ("closed", "Closed", Icon::GitPullRequestClosed),
    ("merged", "Merged", Icon::GitMerge),
];
const INVOLVEMENT: Choices = &[
    ("all", "All", Icon::Layers),
    ("reviewing", "Reviewing", Icon::Eye),
    ("authored", "Authored", Icon::PenLine),
];
const SORT: Choices = &[
    ("ready", "Merge readiness", Icon::ListChecks),
    ("blocked", "Blocked on me", Icon::UserLock),
    ("updated", "Recently updated", Icon::Clock),
    ("newest", "Newest shown", Icon::CalendarArrowDown),
    ("oldest", "Oldest shown", Icon::CalendarArrowUp),
    ("largest", "Largest shown", Icon::Maximize2),
    ("smallest", "Smallest shown", Icon::Minimize2),
];
const DRAFT: Choices = &[
    ("", "All", Icon::Layers),
    ("only", "Drafts only", Icon::GitPullRequestDraft),
    ("hide", "Hide drafts", Icon::EyeOff),
];
const REVIEW: Choices = &[
    ("", "All", Icon::Layers),
    ("approved", "Approved", Icon::CircleCheck),
    ("changes-requested", "Changes requested", Icon::CircleX),
    ("review-required", "Review required", Icon::CircleDashed),
    ("none", "No reviews", Icon::CircleSlash),
];
const CHECKS: Choices = &[
    ("", "All", Icon::Layers),
    ("passing", "Passing", Icon::CircleCheck),
    ("failing", "Failing", Icon::CircleX),
];

#[derive(Clone, Copy)]
struct Filters {
    search: Signal<ListSearch>,
    urls: Option<UrlState>,
}

impl Filters {
    fn new() -> Self {
        let urls = use_context::<UrlState>();
        let search = signal(ListSearch::default());
        if let Some(urls) = urls {
            effect(move || {
                let location = urls.location.get();
                if location.route == crate::state::nav::Route::PullRequests {
                    search.set(ListSearch::parse(&location.search));
                }
            });
        }
        Self { search, urls }
    }

    fn select(self, surface: Option<&Surface>) {
        let mut next = self.search.get_untracked();
        for key in [
            "repository",
            "number",
            "selectedProjectId",
            "selectedEnvironmentId",
            "selectedHost",
            "selectedUrl",
        ] {
            next.set(key, "");
        }
        if let Some(Surface {
            content:
                SurfaceContent::PullRequest {
                    environment_id,
                    project_id,
                    host,
                    repository,
                    number,
                    ..
                },
            ..
        }) = surface
        {
            next.set("repository", repository);
            next.set("number", &number.to_string());
            next.set("selectedProjectId", project_id);
            next.set(
                "selectedEnvironmentId",
                environment_id.as_deref().unwrap_or_default(),
            );
            next.set("selectedHost", host.as_deref().unwrap_or_default());
        }
        self.replace(next);
    }

    fn replace(self, next: ListSearch) {
        if self.search.get_untracked() == next {
            return;
        }
        if let Some(urls) = self.urls {
            urls.request(UrlRequest::Replace(next.href()));
        }
        self.search.set(next);
    }

    fn set(self, key: &str, value: &str) {
        let mut next = self.search.get_untracked();
        if (if key == "draft" {
            next.effective_draft()
        } else {
            next.value(key)
        }) == value
        {
            return;
        }
        next.set(key, value);
        self.replace(next);
    }
}

fn radio_group(
    label: &'static str,
    key: &'static str,
    filters: Filters,
    options: Choices,
) -> AnyElement {
    menu::menu_radio_group(
        t(label),
        move || {
            filters.search.with(|s| {
                let v = if key == "draft" {
                    s.effective_draft()
                } else {
                    s.value(key)
                };
                if v.is_empty() && key == "state" {
                    "open".into()
                } else if v.is_empty() {
                    options[0].0.into()
                } else {
                    v.to_owned()
                }
            })
        },
        view().child(options.iter().map(|&(value, label, icon)| {
            menu::menu_radio_item(
                t(label),
                value.into(),
                Default::default(),
                view()
                    .app_class(tw("flex flex-row items-center gap-2"))
                    .child(icon_with_class(icon, "size-4"))
                    .child(text(t(label)))
                    .child(menu::menu_radio_item_indicator()),
                || {},
            )
        })),
        move |value| filters.set(key, &value),
    )
    .into_element()
}

fn submenu(
    label: &'static str,
    key: &'static str,
    filters: Filters,
    options: Choices,
) -> AnyElement {
    menu::menu_sub(
        menu::menu_sub_trigger(t(label), Default::default(), text(t(label))),
        || false,
        menu::menu_sub_popup(radio_group(label, key, filters, options)),
    )
    .into_element()
}

fn toolbar_menu(
    label: &'static str,
    title: &'static str,
    icon: Icon,
    content: impl IntoElement,
) -> AnyElement {
    menu::menu_button(
        t(label),
        || false,
        view()
            .app_class(tw(
                "flex flex-row items-center gap-2 rounded-md border border-input px-3 py-2",
            ))
            .child(icon_with_class(icon, "size-4"))
            .child(text(t(title))),
        menu::MenuOptions {
            align: Align::Start,
            ..Default::default()
        },
        menu::menu_popup(content),
    )
    .into_element()
}

fn action(label: &'static str, icon: Icon, on_press: impl Fn() + 'static) -> AnyElement {
    nsh_base::button()
        .accessibility_label(t(label))
        .on_press(on_press)
        .app_class(tw(
            "inline-flex items-center justify-center rounded-md border border-input p-2",
        ))
        .child(icon_with_class(icon, "size-4"))
        .into_element()
}

#[derive(Clone, Default, PartialEq)]
struct Pages {
    answers: Vec<Rc<PullRequestListResult>>,
    next: Option<PullRequestListInput>,
    error: Option<String>,
    loading: bool,
}

type Target = (EnvironmentId, Option<Vec<ProjectId>>);
type Replies = BTreeMap<Target, Pages>;

fn read_pages(
    client: ListClient,
    target: Target,
    initial: PullRequestListInput,
    replies: Signal<Replies>,
    more: Signal<u64>,
) {
    let first = stored(true);
    effect(move || {
        let _ = more.get();
        let initial_read = first.with(std::mem::take);
        let request = if initial_read {
            initial.clone()
        } else {
            let next = replies.with_untracked(|map| {
                map.get(&target)
                    .filter(|page| !page.loading)
                    .and_then(|page| page.next.clone())
            });
            let Some(next) = next else { return };
            next
        };
        replies.update(|map| map.entry(target.clone()).or_default().loading = true);
        let result = (client.0)(target.0.clone(), request.clone());
        let target = target.clone();
        watch(async move {
            let result = result.await;
            replies.update(|map| {
                let page = map.entry(target).or_default();
                page.loading = false;
                match result {
                    Ok(answer) => {
                        page.next = pr_list::next_list_input(&request, &answer);
                        if request.cursors.is_none() {
                            page.answers.clear();
                        }
                        page.answers.push(Rc::new(answer));
                        page.error = None;
                    }
                    Err(error) => page.error = Some(error),
                }
            });
        });
    });
}

#[derive(Clone, Copy)]
pub(super) struct ListRefresh {
    pub revision: Signal<u64>,
    pub owner: crate::runtime::tasks::TaskOwner,
}

pub fn screen() -> AnyElement {
    let runtime = rt();
    let service = use_context::<ListClient>();
    let filters = Filters::new();
    let query = signal(filters.search.with_untracked(|s| s.value("q").to_owned()));
    let location_query = computed(move || filters.search.with(|s| s.value("q").to_owned()));
    effect(move || {
        let q = location_query.get();
        if q != query.get_untracked() {
            query.set(q);
        }
    });
    effect(move || {
        let typed = query.get();
        if typed.trim() != filters.search.with_untracked(|s| s.value("q").to_owned()) {
            set_timeout(Duration::from_millis(250), move || {
                filters.set("q", typed.trim())
            });
        }
    });
    let requested = computed(move || filters.search.with(|s| s.input(s.value("q"))));
    let named_environment =
        computed(move || filters.search.with(|s| s.value("environmentId").to_owned()));
    let catalog = computed(move || {
        runtime.store.track_environments();
        let snapshot = runtime.store.store().snapshot();
        let mut configured = runtime
            .environments()
            .into_iter()
            .filter_map(|entry| {
                let state = snapshot.environments.get(&entry.id)?;
                let config = state.config.as_ref()?;
                Some((
                    entry.id,
                    state.session,
                    config.scratch_workspace_root.clone(),
                ))
            })
            .collect::<Vec<_>>();
        configured.sort_by(|a, b| a.0.cmp(&b.0));
        configured
    });
    let shells = computed(move || {
        catalog
            .get()
            .into_iter()
            .map(|(env, session, scratch)| {
                let shell = runtime.store.shell(env.clone());
                (env, session, scratch, shell)
            })
            .collect::<Vec<_>>()
    });
    let checkouts = computed(move || {
        use t3_client::workspace::{
            git_panel::details::is_scratch_project, projects::EnvironmentProject,
        };
        let shells = shells.get();
        let mut projects = Vec::new();
        let mut known = true;
        for (env, _, scratch, shell) in &shells {
            if let Some(shell) = shell.get() {
                projects.extend(shell.projects.iter().map(|p| EnvironmentProject {
                    environment: env.clone(),
                    project: p.as_ref().clone(),
                    is_scratch: is_scratch_project(
                        &p.workspace_root.0,
                        scratch.as_ref().map(|v| v.0.as_str()),
                    ),
                }));
            } else {
                known = false;
            }
        }
        (projects, known)
    });
    let input = computed(move || {
        let mut query = requested.get();
        if query.project_id.is_some() {
            checkouts.with(|(projects, known)| {
                query.project_id = pr_list::resolve_project_scope(
                    query.project_id.take().flatten(),
                    projects,
                    *known,
                )
                .map(Some);
            });
        }
        query
    });
    let scope = computed(move || {
        let shells = shells.get();
        let environments = shells
            .iter()
            .map(|(env, ..)| env.clone())
            .collect::<Vec<_>>();
        if environments.is_empty() {
            return None;
        }
        let (projects, known) = checkouts.get();
        let project = input.with(|i| i.project_id.clone().flatten());
        let targets = pr_list::list_targets(
            &environments,
            &projects,
            known,
            &named_environment.get(),
            project.as_ref(),
        )?;
        Some(
            targets
                .into_iter()
                .filter_map(|target| {
                    let (_, session, ..) =
                        shells.iter().find(|(e, ..)| *e == target.environment)?;
                    Some(((target.environment, target.project_ids), *session))
                })
                .collect::<Vec<_>>(),
        )
    });
    let host = panel::workspace_panel_host();
    let panel_key = stored(panel_key());
    let panel_state = host.panel(panel_key.get());
    // Hydrate a shared link only after the configured server owns its project.
    let route_target = computed(move || {
        filters.search.with(|search| {
            checkouts.with(|(projects, known)| search.detail_surface(projects, *known))
        })
    });
    let last_route = stored(None::<Option<Surface>>);
    effect(move || {
        let wanted = route_target.get();
        let active = panel_state.with(|p| p.active().cloned());
        if last_route.get().as_ref() != Some(&wanted) {
            last_route.with(|last| *last = Some(wanted.clone()));
            if active.as_ref().map(|s| &s.id) != wanted.as_ref().map(|s| &s.id) {
                if let Some(s) = wanted {
                    host.apply(&panel_key.get(), Action::OpenSurface(s));
                } else if active.is_some() {
                    host.apply(&panel_key.get(), Action::Close);
                }
            }
        } else if active.as_ref().map(|s| &s.id) != wanted.as_ref().map(|s| &s.id) {
            filters.select(active.as_ref());
        }
    });
    let replies = signal(Replies::new());
    let more = signal(0u64);
    let refresh = signal(0u64);
    provide_context(ListRefresh {
        revision: refresh,
        owner: crate::runtime::tasks::TaskOwner::here(),
    });
    let reload = move || refresh.update(|r| *r += 1);
    let service = stored(service);
    effect(move || {
        let targets = scope.get().unwrap_or_default();
        let input = input.get();
        replies.set(BTreeMap::new());
        let Some(client) = service.get() else { return };
        effect(move || {
            refresh.get();
            for (target, _) in &targets {
                let mut request = input.clone();
                request.project_ids = target.1.clone().map(Some);
                read_pages(client.clone(), target.clone(), request, replies, more);
            }
        });
    });
    let nav = navigator();
    let controls = view()
        .app_class(tw("flex flex-row flex-wrap items-center gap-2"))
        .child(toolbar_menu(
            "Sort pull requests",
            "Sort",
            Icon::ArrowDownUp,
            radio_group("Sort pull requests", "sort", filters, SORT),
        ))
        .child(toolbar_menu(
            "Filters",
            "Filters",
            Icon::ListFilter,
            view()
                .child(submenu("State", "state", filters, STATE))
                .child(submenu("Involvement", "involvement", filters, INVOLVEMENT))
                .child(menu::menu_separator())
                .child(submenu("Draft", "draft", filters, DRAFT))
                .child(submenu("Review", "review", filters, REVIEW))
                .child(submenu("Checks", "checks", filters, CHECKS)),
        ))
        .child(action("Refresh pull requests", Icon::RefreshCw, reload));
    let content = dynamic(move || {
        if service.with(|s| s.is_none()) {
            return view()
                .child(text(t("Pull requests unavailable")))
                .child(text(t(
                    "Update your T3 Code servers to browse pull requests.",
                )))
                .into_element();
        }
        let targets = scope.get();
        let input = input.get();
        let sort = filters.search.with(ListSearch::sort);
        let typed = location_query.get();
        replies.with(|replies| {
            untracked(|| {
                list_body(
                    replies,
                    targets.as_ref().map(Vec::len),
                    &input,
                    &typed,
                    sort,
                    ListActions {
                        filters,
                        refresh,
                        more,
                    },
                )
            })
        })
    });
    let workspace = view()
        .flex_1()
        .min_h(px(0.0))
        .app_class(tw("flex min-w-0 flex-col bg-background"))
        .child(
            view()
                .app_class(tw(
                    "flex flex-row items-center gap-2 border-b border-border px-4 py-2",
                ))
                .child(when(
                    move || layout_class().is_compact(),
                    move || action("Back", Icon::ArrowLeft, move || nav.back()),
                ))
                .child(text(t("Pull requests"))),
        )
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .app_class(tw("mx-auto flex w-full flex-col gap-4 p-4"))
                    .child(
                        view()
                            .app_class(tw("flex flex-col gap-2"))
                            .child(text_field(
                                query,
                                t("Search pull requests, or label:bug"),
                                t("Search pull requests"),
                                InputSize::Default,
                                move || filters.set("q", query.get_untracked().trim()),
                            ))
                            .child(controls),
                    )
                    .child(content),
            ),
        )
        .into_element();
    let Some(registry) = use_context::<PanelRegistry>() else {
        return workspace;
    };
    let width = signal(540f32);
    let row = signal(0f32);
    view()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .on_layout(move |b: &Bounds<Points>| row.set(b.size.width.0))
        .child(
            panel::chat_right_panel(
                host,
                panel_key.get(),
                Prop::bind(move || row.get()),
                Prop::bind(move || width.get()),
                150,
                move |w| width.set(w),
            )
            .fill("workspace", workspace)
            .child(panel::right_panel_tabs(
                host,
                panel_key.get(),
                registry,
                view(),
            )),
        )
        .into_element()
}

#[derive(Clone, Copy)]
struct ListActions {
    filters: Filters,
    refresh: Signal<u64>,
    more: Signal<u64>,
}

fn list_body(
    replies: &Replies,
    expected: Option<usize>,
    input: &PullRequestListInput,
    typed: &str,
    sort: ListSort,
    actions: ListActions,
) -> AnyElement {
    let reload = move || actions.refresh.update(|r| *r += 1);
    let load_more = move || actions.more.update(|r| *r += 1);
    let Some(expected) = expected else {
        return text(t("Loading pull requests…")).into_element();
    };
    if expected == 0 {
        let nav = navigator();
        return view()
            .child(text(t("No projects in this workspace")))
            .child(text(t(
                "Add a project, and the pull requests from its repository appear here.",
            )))
            .child(
                nsh_base::button()
                    .accessibility_label(t("Add project"))
                    .on_press(move || nav.open(crate::state::nav::Route::AddProject))
                    .child(text(t("Add project"))),
            )
            .into_element();
    }
    let mut body = view().app_class(tw("flex flex-col gap-2"));
    let answers: Vec<_> = replies
        .iter()
        .flat_map(|(env, page)| {
            page.answers
                .iter()
                .map(move |answer| (env.0.clone(), answer.as_ref().clone()))
        })
        .collect();
    for error in replies.values().filter_map(|v| v.error.as_ref()) {
        body = body
            .child(text(t("Could not load pull requests")))
            .child(text(error.to_owned()))
            .child(action("Retry", Icon::RefreshCw, reload));
    }
    let Some(merged) = pr_list::merge_lists(&answers) else {
        if replies.is_empty() || replies.len() < expected || replies.values().any(|p| p.loading) {
            body = body.child(text(t("Loading pull requests…")));
        }
        return body.into_element();
    };
    for (_, error) in &merged.errors {
        body = body.child(text(format!(
            "{} · {}",
            error.project_title.0, error.message.0
        )));
    }
    let involvement = input
        .involvement
        .clone()
        .flatten()
        .unwrap_or(PullRequestInvolvement::All);
    let groups = if involvement == PullRequestInvolvement::All {
        pr_list::group_by_involvement(&merged.entries, &merged.viewers)
    } else {
        vec![pr_list::ListGroup {
            key: pr_list::GroupKey::Others,
            entries: merged.entries.iter().collect(),
        }]
    };
    let mut groups = groups;
    let query = input
        .query
        .as_ref()
        .and_then(Option::as_ref)
        .map_or("", |q| q.0.as_str());
    pr_list::sort_groups(
        &mut groups,
        sort,
        query,
        |r| r.entry.additions > 0 || r.entry.deletions > 0,
        &involvement,
    );
    if merged.entries.is_empty()
        && replies.len() == expected
        && merged.errors.is_empty()
        && replies.values().all(|p| !p.loading && p.error.is_none())
    {
        let empty = if !typed.is_empty() {
            tr(
                "Nothing matches “{query}”",
                &[(
                    "query",
                    t3_client::workspace::pr_views::markdown::bound_utf16(typed, 49, "…").into(),
                )],
            )
        } else if input.state != PullRequestListState::Open
            || involvement != PullRequestInvolvement::All
            || input.filters.is_some()
        {
            t("Nothing under these filters")
        } else {
            t("No pull requests")
        };
        body = body
            .child(text(empty))
            .child(action("Check again", Icon::RefreshCw, reload));
    }
    for group in groups {
        if involvement == PullRequestInvolvement::All {
            body = body.child(
                text(t(group.key.label()))
                    .app_class(tw("text-xs font-medium text-muted-foreground")),
            );
        }
        for row in group.entries {
            let e = &row.entry;
            let selected = Surface::pull_request(
                Some(&row.environment.0),
                &e.project_id.0,
                Some(&e.host.0),
                &e.repository.0,
                e.number,
            );
            body = body.child(
                nsh_base::button()
                    .accessibility_label(tr(
                        "Open pull request #{number}",
                        &[("number", e.number.to_string().into())],
                    ))
                    .on_press(move || actions.filters.select(Some(&selected)))
                    .child(list_row(e)),
            );
        }
    }
    if replies.values().any(|p| p.next.is_some()) {
        body = body.child(
            nsh_base::button()
                .accessibility_label(t("Load more pull requests"))
                .disabled(replies.values().any(|p| p.loading))
                .on_press(load_more)
                .child(text(t("Load more pull requests"))),
        );
    }
    body.into_element()
}

fn list_row(entry: &PullRequestListEntry) -> AnyElement {
    let glyph = match entry.state {
        PullRequestState::Closed => Icon::GitPullRequestClosed,
        PullRequestState::Merged => Icon::GitMerge,
        _ if entry.is_draft => Icon::GitPullRequestDraft,
        _ => Icon::GitPullRequest,
    };
    let updated = entry.updated_at.0.clone();
    view().app_class(tw("flex w-full flex-row items-center gap-2 rounded-md py-1 pr-1 text-left"))
        .child(icon_with_class(glyph,"size-4 shrink-0"))
        .child(view().app_class(tw("flex min-w-0 flex-1 flex-col"))
            .child(view().app_class(tw("flex min-w-0 flex-row items-center gap-1.5"))
                .child(text(format!("#{}",entry.number)).app_class(tw("font-mono text-xs text-muted-foreground")))
                .child(text(entry.title.0.clone()).app_class(tw("min-w-0 truncate text-sm"))))
            .child(view().app_class(tw("flex min-w-0 flex-row items-center gap-1.5 text-2xs text-muted-foreground"))
                .child(text(entry.repository.0.clone())).child(text(entry.author.as_ref().map_or("ghost",|a|a.login.0.as_str()).to_owned()))
                .child(text(format!("{} → {}",entry.head_branch.0,entry.base_branch.0)))
                .child(text(move ||crate::screens::sidebar::row::relative_label(&updated,clock::now_ms())))))
        .into_element()
}
