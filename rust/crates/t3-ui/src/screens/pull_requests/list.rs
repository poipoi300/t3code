//! Port of apps/web/src/components/pullRequest/PullRequestsPage.tsx and PullRequestsColumn.tsx.

use futures::{FutureExt, future::LocalBoxFuture};
use nsh_core::prelude::*;
use std::{collections::BTreeMap, rc::Rc, time::Duration};
use t3_client::{
    composition::ClientRuntime,
    platform::Platform,
    types::{EnvironmentId, ProjectId},
    workspace::{
        pr_list::{self, AuthorFacet, EnvironmentPullRequestEntry, LabelFacet, ListSort},
        pr_views::preferences::ListSearch,
    },
};
use t3_contracts::pull_request::*;

use crate::{
    i18n::{Value, t, tr},
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
type FilterOptionsRead =
    dyn Fn(
        EnvironmentId,
        PullRequestFilterOptionsInput,
    ) -> LocalBoxFuture<'static, Result<PullRequestFilterOptions, String>>;

#[derive(Clone)]
pub struct ListClient {
    list: Rc<ListRead>,
    filter_options: Rc<FilterOptionsRead>,
    stack: super::stack::StackRead,
}

pub fn bind<P: Platform + Clone + 'static>(client: &ClientRuntime<P>) -> ListClient {
    let list_client = client.clone();
    let filter_client = client.clone();
    let stack_client = client.clone();
    ListClient {
        list: Rc::new(move |environment, input| {
            let client = list_client.clone();
            async move {
                client
                    .pull_request_list(&environment, input)
                    .await
                    .map_err(|e| e.into_client_error().to_string())
            }
            .boxed_local()
        }),
        filter_options: Rc::new(move |environment, input| {
            let client = filter_client.clone();
            async move {
                client
                    .pull_request_filter_options(&environment, input)
                    .await
                    .map_err(|e| e.into_client_error().to_string())
            }
            .boxed_local()
        }),
        stack: Rc::new(move |environment, reference| {
            let session = stack_client.pull_request_review_session(&environment, reference);
            async move {
                session
                    .stack()
                    .await
                    .map_err(|e| e.into_client_error().to_string())
            }
            .boxed_local()
        }),
    }
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

    fn set_pairs(self, pairs: &[(&str, &str)]) {
        let mut next = self.search.get_untracked();
        for &(key, value) in pairs {
            next.set(key, value);
        }
        self.replace(next);
    }

    fn selected_labels(self) -> Vec<String> {
        self.search.with(ListSearch::labels)
    }

    fn set_labels(self, labels: &[String]) {
        let mut next = self.search.get_untracked();
        next.set_labels(labels);
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
        let result = (client.list)(target.0.clone(), request.clone());
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
    let owner = crate::runtime::tasks::TaskOwner::here();
    let filters_open = signal(false);
    let author_query = signal(String::new());
    let debounced_author_query = signal(String::new());
    let label_query = signal(String::new());
    let debounced_label_query = signal(String::new());
    let hosted_authors = signal(Vec::<PullRequestActor>::new());
    let hosted_authors_error = signal(None::<String>);
    let hosted_authors_pending = signal(false);
    let hosted_labels = signal(Vec::<PullRequestLabel>::new());
    let hosted_labels_error = signal(None::<String>);
    let hosted_labels_pending = signal(false);

    super::bind_debounced_query(author_query, debounced_author_query);
    super::bind_debounced_query(label_query, debounced_label_query);

    let fetch_filter_options =
        move |kind: PullRequestFilterOptionKind,
              q_sig: Signal<String>,
              dq_sig: Signal<String>,
              pending_sig: Signal<bool>,
              err_sig: Signal<Option<String>>,
              on_done: Rc<dyn Fn(Vec<PullRequestFilterOptions>)>| {
            effect(move || {
                if !filters_open.get() {
                    return;
                }
                let Some(client) = service.get() else {
                    return;
                };
                let targets = scope.get().unwrap_or_default();
                let req = input.get();
                let dq = dq_sig.get();
                if dq != q_sig.get_untracked().trim() {
                    pending_sig.set(true);
                    return;
                }
                pending_sig.set(true);
                err_sig.set(None);
                let q = (!dq.is_empty()).then(|| {
                    Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                        t3_client::workspace::pr_views::markdown::bound_utf16(&dq, 100, ""),
                    ))
                });
                let on_done = on_done.clone();
                let kind = kind.clone();
                watch(async move {
                    let mut responses = Vec::new();
                    let mut first_error = None;
                    for ((env, project_ids), _) in targets {
                        let res = (client.filter_options)(
                            env,
                            PullRequestFilterOptionsInput {
                                kind: kind.clone(),
                                project_id: req.project_id.clone(),
                                project_ids: project_ids.map(Some),
                                host: req.host.clone(),
                                query: q.clone(),
                                sigil: None,
                            },
                        )
                        .await;
                        match res {
                            Ok(ok) => responses.push(ok),
                            Err(e) if first_error.is_none() => first_error = Some(e),
                            Err(_) => {}
                        }
                    }
                    on_done(responses);
                    err_sig.try_set(first_error);
                    pending_sig.try_set(false);
                });
            });
        };

    fetch_filter_options(
        PullRequestFilterOptionKind::Authors,
        author_query,
        debounced_author_query,
        hosted_authors_pending,
        hosted_authors_error,
        Rc::new(move |responses| {
            let merged = responses.into_iter().flat_map(|ok| ok.authors).collect();
            hosted_authors.try_set(merged);
        }),
    );
    fetch_filter_options(
        PullRequestFilterOptionKind::Labels,
        label_query,
        debounced_label_query,
        hosted_labels_pending,
        hosted_labels_error,
        Rc::new(move |responses| {
            let merged = responses.into_iter().flat_map(|ok| ok.labels).collect();
            hosted_labels.try_set(merged);
        }),
    );

    let facets = computed(move || {
        let state = input.with(|i| i.state.clone());
        replies.with(|map| {
            let entries: Vec<EnvironmentPullRequestEntry> = map
                .iter()
                .flat_map(|((env, _), page)| {
                    page.answers.iter().flat_map(move |answer| {
                        let env = env.clone();
                        answer.entries.iter().cloned().map(move |entry| {
                            EnvironmentPullRequestEntry {
                                environment: env.clone(),
                                entry,
                            }
                        })
                    })
                })
                .collect();
            pr_list::collect_facets(&entries, &state)
        })
    });

    let observed_hosts = computed(move || {
        replies.with(|map| {
            let mut list: Vec<PullRequestProviderSummary> = Vec::new();
            for page in map.values() {
                for answer in &page.answers {
                    for summary in &answer.providers {
                        if let Some(existing) =
                            list.iter_mut().find(|item| item.host == summary.host)
                        {
                            *existing = summary.clone();
                        } else {
                            list.push(summary.clone());
                        }
                    }
                }
            }
            list
        })
    });

    let env_labels = computed(move || {
        runtime.store.track_environments();
        runtime
            .environments()
            .into_iter()
            .map(|entry| (entry.id, entry.label))
            .collect::<BTreeMap<_, _>>()
    });

    let filters_label = t("Filters");
    let _filters_owner = owner;
    let filters_menu = menu::menu_button(
        filters_label.clone(),
        || false,
        view()
            .app_class(tw(
                "flex flex-row items-center gap-2 rounded-md border border-input px-3 py-2",
            ))
            .child(icon_with_class(Icon::ListFilter, "size-4"))
            .child(text(filters_label)),
        menu::MenuOptions {
            open: Some(filters_open),
            align: Align::Start,
            ..Default::default()
        },
        menu::menu_popup_with(move || {
            let hosts = observed_hosts.get();
            let servers = catalog.get();
            let labels_map = env_labels.get();
            let (projects, _) = checkouts.get();

            let author_sub = menu::menu_sub(
                menu::menu_sub_trigger(
                    t("Author"),
                    Default::default(),
                    view()
                        .app_class(tw("flex flex-row items-center gap-2"))
                        .child(icon_with_class(Icon::UserRound, "size-4"))
                        .child(text(t("Author"))),
                ),
                || false,
                menu::menu_sub_popup(
                    view()
                        .child(menu::menu_search_input(
                            author_query,
                            t("Search authors"),
                            t("Search authors"),
                            move || hosted_authors_pending.get(),
                        ))
                        .child(dynamic(move || {
                            let (author_facets, _) = facets.get();
                            let q = author_query.get();
                            let h_authors = hosted_authors.get();
                            let h_err = hosted_authors_error.get();
                            let h_pending = hosted_authors_pending.get();
                            let chosen = filters.search.with_untracked(|s| {
                                let a = s.value("author").trim().to_owned();
                                (!a.is_empty()).then_some(a)
                            });
                            let sel = pr_list::selected_author(
                                &author_facets,
                                &h_authors,
                                chosen.as_deref(),
                            );
                            let chosen_key =
                                chosen.as_deref().map(str::to_lowercase).unwrap_or_default();
                            let mut visible = Vec::<AuthorFacet>::new();
                            if let Some(selected_item) = sel.clone() {
                                visible.push(selected_item);
                            }
                            for option in pr_list::author_choices(&author_facets, &h_authors, &q) {
                                if option.actor.login.0.to_lowercase() != chosen_key {
                                    visible.push(option);
                                }
                                if visible.len() >= 30 {
                                    break;
                                }
                            }
                            let mut group = view().child(menu::menu_radio_item(
                                t("Anyone"),
                                String::new(),
                                Default::default(),
                                view()
                                    .app_class(tw("flex flex-row items-center gap-2"))
                                    .child(icon_with_class(Icon::Layers, "size-4"))
                                    .child(text(t("Anyone")))
                                    .child(menu::menu_radio_item_indicator()),
                                || {},
                            ));
                            for option in &visible {
                                let login = option.actor.login.0.clone();
                                let mut row = view()
                                    .app_class(tw("flex flex-row items-center gap-2"))
                                    .child(text(login.clone()))
                                    .child(menu::menu_radio_item_indicator());
                                if option.merged_count > 0 {
                                    row = row.child(
                                        text(tr(
                                            "{count} merges loaded",
                                            &[("count", Value::Int(option.merged_count as i64))],
                                        ))
                                        .app_class(tw("text-xs text-muted-foreground")),
                                    );
                                }
                                group = group.child(menu::menu_radio_item(
                                    login.clone(),
                                    login,
                                    Default::default(),
                                    row,
                                    || {},
                                ));
                            }
                            if visible.is_empty() && !h_pending {
                                let empty_msg = t(if h_err.is_none() {
                                    "No authors found"
                                } else {
                                    "Couldn't search authors on the host"
                                });
                                group = group.child(menu::menu_item(
                                    empty_msg.clone(),
                                    menu::ItemOptions {
                                        disabled: Prop::new(true),
                                        ..Default::default()
                                    },
                                    text(empty_msg),
                                    || {},
                                ));
                            }
                            let selected_val = filters.search.with(|s| {
                                sel.map(|v| v.actor.login.0).unwrap_or_else(|| {
                                    let a = s.value("author").trim().to_owned();
                                    if a.is_empty() { String::new() } else { a }
                                })
                            });
                            menu::menu_radio_group(
                                t("Author"),
                                move || selected_val.clone(),
                                group,
                                move |next| filters.set("author", &next),
                            )
                        })),
                ),
            );

            let labels_sub = menu::menu_sub(
                menu::menu_sub_trigger(
                    t("Labels"),
                    Default::default(),
                    view()
                        .app_class(tw("flex flex-row items-center gap-2"))
                        .child(icon_with_class(Icon::Tag, "size-4"))
                        .child(text(t("Labels"))),
                ),
                || false,
                menu::menu_sub_popup(
                    view()
                        .child(menu::menu_search_input(
                            label_query,
                            t("Search labels"),
                            t("Search labels"),
                            move || hosted_labels_pending.get(),
                        ))
                        .child(dynamic(move || {
                            let (_, label_facets) = facets.get();
                            let q = label_query.get();
                            let h_labels = hosted_labels.get();
                            let h_err = hosted_labels_error.get();
                            let h_pending = hosted_labels_pending.get();
                            let current = filters.selected_labels();
                            let choices = pr_list::label_choices(&label_facets, &h_labels, &q);
                            let mut visible = Vec::<LabelFacet>::new();
                            for name in &current {
                                let key = name.to_lowercase();
                                if !choices.iter().any(|o| o.label.name.0.to_lowercase() == key)
                                    && let Ok(trimmed) =
                                        t3_contracts::base_schemas::TrimmedNonEmptyString::checked(
                                            name,
                                        )
                                {
                                    visible.push(LabelFacet {
                                        label: PullRequestLabel {
                                            name: trimmed,
                                            color: None,
                                            description: None,
                                        },
                                        count: 0,
                                    });
                                }
                            }
                            visible.extend(choices);
                            let mut list = view();
                            if visible.is_empty() && !h_pending {
                                let empty_msg = t(if h_err.is_none() {
                                    "No labels found"
                                } else {
                                    "Couldn't search labels on the host"
                                });
                                list = list.child(menu::menu_item(
                                    empty_msg.clone(),
                                    menu::ItemOptions {
                                        disabled: Prop::new(true),
                                        ..Default::default()
                                    },
                                    text(empty_msg),
                                    || {},
                                ));
                            }
                            for option in visible {
                                let label_name = option.label.name.0.clone();
                                let key = label_name.to_lowercase();
                                let name_for_toggle = label_name.clone();
                                list = list.child(menu::menu_checkbox_item(
                                    label_name.clone(),
                                    move || {
                                        filters
                                            .selected_labels()
                                            .iter()
                                            .any(|item| item.to_lowercase() == key)
                                    },
                                    menu::CheckboxVariant::Icon,
                                    menu::ItemOptions {
                                        close_on_select: false,
                                        ..Default::default()
                                    },
                                    text(label_name),
                                    move |next| {
                                        let mut updated = filters.selected_labels();
                                        if next {
                                            if !updated
                                                .iter()
                                                .any(|v| v.eq_ignore_ascii_case(&name_for_toggle))
                                            {
                                                updated.push(name_for_toggle.clone());
                                            }
                                        } else {
                                            updated.retain(|v| {
                                                !v.eq_ignore_ascii_case(&name_for_toggle)
                                            });
                                        }
                                        filters.set_labels(&updated);
                                    },
                                ));
                            }
                            list
                        })),
                ),
            );

            let mut popup = view()
                .child(submenu("State", "state", filters, STATE))
                .child(submenu("Involvement", "involvement", filters, INVOLVEMENT))
                .child(menu::menu_separator())
                .child(author_sub)
                .child(labels_sub)
                .child(submenu("Draft", "draft", filters, DRAFT))
                .child(submenu("Review", "review", filters, REVIEW))
                .child(submenu("Checks", "checks", filters, CHECKS));

            if hosts.len() > 1 {
                let sharing_count =
                    |kind: &t3_contracts::source_control::SourceControlProviderKind| {
                        hosts.iter().filter(|h| &h.kind == kind).count()
                    };
                let mut host_items = view().child(menu::menu_radio_item(
                    t("All"),
                    String::new(),
                    Default::default(),
                    view()
                        .app_class(tw("flex flex-row items-center gap-2"))
                        .child(icon_with_class(Icon::Globe, "size-4"))
                        .child(text(t("All")))
                        .child(menu::menu_radio_item_indicator()),
                    || {},
                ));
                for entry in &hosts {
                    let label = if sharing_count(&entry.kind) > 1 {
                        entry.host.0.clone()
                    } else {
                        super::host_display_label(&entry.kind.0)
                    };
                    let icon = match entry.kind.0.as_str() {
                        "github" => Icon::Github,
                        "gitlab" => Icon::Gitlab,
                        _ => Icon::Globe,
                    };
                    let disabled = !entry.configured;
                    host_items = host_items.child(menu::menu_radio_item(
                        label.clone(),
                        entry.host.0.clone(),
                        menu::ItemOptions {
                            disabled: Prop::new(disabled),
                            ..Default::default()
                        },
                        view()
                            .app_class(tw("flex flex-row items-center gap-2"))
                            .child(icon_with_class(icon, "size-4"))
                            .child(text(label))
                            .child(menu::menu_radio_item_indicator()),
                        || {},
                    ));
                }
                popup = popup.child(menu::menu_separator()).child(menu::menu_sub(
                    menu::menu_sub_trigger(
                        t("Host"),
                        Default::default(),
                        view()
                            .app_class(tw("flex flex-row items-center gap-2"))
                            .child(icon_with_class(Icon::Globe, "size-4"))
                            .child(text(t("Host"))),
                    ),
                    || false,
                    menu::menu_sub_popup(menu::menu_radio_group(
                        t("Host"),
                        move || filters.search.with(|s| s.value("host").to_owned()),
                        host_items,
                        move |next| filters.set("host", &next),
                    )),
                ));
            }

            if servers.len() > 1 {
                let mut server_items = view().child(menu::menu_radio_item(
                    t("All servers"),
                    String::new(),
                    Default::default(),
                    view()
                        .app_class(tw("flex flex-row items-center gap-2"))
                        .child(icon_with_class(Icon::Layers, "size-4"))
                        .child(text(t("All servers")))
                        .child(menu::menu_radio_item_indicator()),
                    || {},
                ));
                for (env_id, _, _) in &servers {
                    let label = labels_map
                        .get(env_id)
                        .cloned()
                        .unwrap_or_else(|| env_id.0.clone());
                    server_items = server_items.child(menu::menu_radio_item(
                        label.clone(),
                        env_id.0.clone(),
                        Default::default(),
                        view()
                            .app_class(tw("flex flex-row items-center gap-2"))
                            .child(icon_with_class(Icon::Server, "size-4"))
                            .child(text(label))
                            .child(menu::menu_radio_item_indicator()),
                        || {},
                    ));
                }
                popup = popup.child(menu::menu_separator()).child(menu::menu_sub(
                    menu::menu_sub_trigger(
                        t("Server"),
                        Default::default(),
                        view()
                            .app_class(tw("flex flex-row items-center gap-2"))
                            .child(icon_with_class(Icon::Server, "size-4"))
                            .child(text(t("Server"))),
                    ),
                    || false,
                    menu::menu_sub_popup(menu::menu_radio_group(
                        t("Server"),
                        move || filters.search.with(|s| s.value("environmentId").to_owned()),
                        server_items,
                        move |next| {
                            filters.set_pairs(&[("environmentId", &next), ("projectId", "")])
                        },
                    )),
                ));
            }

            let projects_for_lookup = projects.clone();
            let mut project_items = view().child(menu::menu_radio_item(
                t("All projects"),
                String::new(),
                Default::default(),
                view()
                    .app_class(tw("flex flex-row items-center gap-2"))
                    .child(icon_with_class(Icon::Layers, "size-4"))
                    .child(text(t("All projects")))
                    .child(menu::menu_radio_item_indicator()),
                || {},
            ));
            for proj in &projects {
                if proj.is_scratch {
                    continue;
                }
                let title = proj.project.title.0.clone();
                let value = format!("{}\0{}", proj.environment.0, proj.project.id.0);
                project_items = project_items.child(menu::menu_radio_item(
                    title.clone(),
                    value,
                    Default::default(),
                    view()
                        .app_class(tw("flex flex-row items-center gap-2"))
                        .child(icon_with_class(Icon::FolderGit2, "size-4"))
                        .child(text(title))
                        .child(menu::menu_radio_item_indicator()),
                    || {},
                ));
            }
            popup.child(menu::menu_separator()).child(menu::menu_sub(
                menu::menu_sub_trigger(
                    t("Project"),
                    Default::default(),
                    view()
                        .app_class(tw("flex flex-row items-center gap-2"))
                        .child(icon_with_class(Icon::FolderGit2, "size-4"))
                        .child(text(t("Project"))),
                ),
                || false,
                menu::menu_sub_popup(menu::menu_radio_group(
                    t("Project"),
                    move || {
                        filters.search.with(|s| {
                            let p = s.value("projectId");
                            let e = s.value("environmentId");
                            if p.is_empty() {
                                String::new()
                            } else if e.is_empty() {
                                projects_for_lookup
                                    .iter()
                                    .find(|cand| cand.project.id.0 == p)
                                    .map_or_else(String::new, |cand| {
                                        format!("{}\0{}", cand.environment.0, cand.project.id.0)
                                    })
                            } else {
                                format!("{e}\0{p}")
                            }
                        })
                    },
                    project_items,
                    move |next| {
                        if let Some((env_id, proj_id)) = next.split_once('\0') {
                            filters.set_pairs(&[("projectId", proj_id), ("environmentId", env_id)]);
                        } else {
                            filters.set("projectId", "");
                        }
                    },
                )),
            ))
        }),
    );

    let controls = view()
        .app_class(tw("flex flex-row flex-wrap items-center gap-2"))
        .child(toolbar_menu(
            "Sort pull requests",
            "Sort",
            Icon::ArrowDownUp,
            radio_group("Sort pull requests", "sort", filters, SORT),
        ))
        .child(filters_menu)
        .child(action("Refresh pull requests", Icon::RefreshCw, reload));
    let content = dynamic(move || {
        let Some(client) = service.get() else {
            return view()
                .child(text(t("Pull requests unavailable")))
                .child(text(t(
                    "Update your T3 Code servers to browse pull requests.",
                )))
                .into_element();
        };
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
                        stack: client.stack,
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

#[derive(Clone)]
struct ListActions {
    filters: Filters,
    refresh: Signal<u64>,
    more: Signal<u64>,
    stack: super::stack::StackRead,
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
            let row_env = row.environment.clone();
            let host_str = e.host.0.clone();
            let proj_id = e.project_id.clone();
            let repo_str = e.repository.0.clone();
            let selected = Surface::pull_request(
                Some(&row_env.0),
                &proj_id.0,
                Some(&host_str),
                &repo_str,
                e.number,
            );
            let filters = actions.filters;
            let open_btn = nsh_base::button()
                .accessibility_label(tr(
                    "Open pull request #{number}",
                    &[("number", e.number.to_string().into())],
                ))
                .on_press(move || filters.select(Some(&selected)))
                .app_class(tw("min-w-0 flex-1"))
                .child(list_row(e));
            let stack_badge = e.stack.as_ref().and_then(Option::as_ref).map(|membership| {
                let pr_ref = PullRequestRef {
                    project_id: proj_id.clone(),
                    host: Some(Some(e.host.clone())),
                    repository: e.repository.clone(),
                    number: e.number,
                    expected_account_id: None,
                    allow_stale: None,
                };
                let nav_env = row_env.0.clone();
                let nav_proj = proj_id.0.clone();
                let nav_host = host_str.clone();
                let nav_repo = repo_str.clone();
                super::stack::row_stack_popover(
                    actions.stack.clone(),
                    row_env,
                    pr_ref,
                    membership.clone(),
                    Rc::new(move |number| {
                        let target = Surface::pull_request(
                            Some(&nav_env),
                            &nav_proj,
                            Some(&nav_host),
                            &nav_repo,
                            number,
                        );
                        filters.select(Some(&target));
                    }),
                )
            });
            body = body.child(
                view()
                    .app_class(tw("flex w-full flex-row items-center gap-1"))
                    .child(open_btn)
                    .child(stack_badge),
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
