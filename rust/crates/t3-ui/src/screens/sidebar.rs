//! Port of apps/web/src/components/Sidebar.tsx Sidebar (list and project scope).

pub mod actions;
pub mod chrome;
pub mod drag;
pub mod projects;
pub mod row;
pub mod selection;
pub mod shelves;
pub mod snooze;
pub mod swipe;

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::Duration;

use nsh_core::prelude::*;
use t3_client::services::config::timestamp_ms;
use t3_client::services::models::present_thread_shell;
use t3_client::state::sharing::SharedShellSnapshot;
use t3_client::thread_views::threads::shell::{EnvironmentShells, ThreadShellViews};
use t3_client::thread_views::threads::sort::{
    SortOrder, ThreadSortInput, get_thread_sort_timestamp, resolve_thread_activity_timestamp,
};
use t3_client::types::{EnvironmentId, ThreadKey};
use t3_client::view_logic::shell::sidebar::{
    Marker, RouteReveal, Section, SidebarThread,
    presentation::{
        ActiveOrder, Options, SidebarFolder, SidebarRow, SidebarRows, ThreadInput, present_sidebar,
        sidebar_rows,
    },
    project_activity, resolve_row_thread_key, sort_logical_projects,
};
use t3_client::workspace::{
    ProjectKey,
    projects::{EnvironmentProject, GroupingSettings, LogicalProjectKey, ProjectGroup},
    sidebar::{SidebarProjectSnapshot, build_sidebar_snapshots, resolve_scopes},
};
use t3_contracts::orchestration_project::OrchestrationProjectShell;
use t3_contracts::orchestration_v2::OrchestrationV2ThreadShell;

use t3_client::view_logic::shell::sidebar_list::{
    SETTLED_TAIL_INITIAL_COUNT, SETTLED_TAIL_PAGE_COUNT, ShelfExpansion, SidebarListItem,
    SidebarListItems, sidebar_list_items,
};

use crate::i18n::{Text, Value, t, tr};
use crate::runtime::{Shared, rt};
use crate::state::nav::{Route, navigator};
use crate::theme::{Scope, tw, tw_with};
use crate::ui::icon::{Icon, icon};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowVariant {
    Card,
    Slim,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SidebarItem {
    pub key: ThreadKey,
    pub title: String,
    pub project: String,
    pub activity_at: Option<String>,
    pub active: bool,
    pub variant: RowVariant,
}

impl SidebarItem {
    pub fn list_key(&self) -> SharedString {
        selection::row_key(&self.key).into()
    }
}

/// Shared observations for SB-SELECT and the new-thread destination controller.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidebarState {
    pub project_scope: Vec<LogicalProjectKey>,
    pub selected: BTreeSet<ThreadKey>,
    pub new_thread_projects: Vec<ProjectKey>,
}

pub fn sidebar_state() -> Signal<SidebarState> {
    global::<SidebarState>()
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListRow {
    pub source: Shared<SidebarRow>,
    pub item: SidebarItem,
    pub project: Option<Shared<OrchestrationProjectShell>>,
}

/// SB-SHELVES can insert markers from `source.section` and `folders` without sorting rows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidebarList {
    pub rows: Vec<Rc<ListRow>>,
    pub keys: Rc<Vec<SharedString>>,
    pub groups: Vec<SidebarProjectSnapshot>,
    pub scope: Vec<LogicalProjectKey>,
    pub scoped_projects: Vec<ProjectKey>,
    pub next_snooze_wake: Option<i64>,
    pub folders: Vec<SidebarFolder>,
    environments: BTreeMap<EnvironmentId, SidebarRows>,
}

fn thread_inputs<'a>(
    environment: &EnvironmentId,
    shell: &'a SharedShellSnapshot,
) -> Vec<SidebarThread<'a>> {
    shell
        .threads
        .iter()
        .map(|thread| SidebarThread {
            key: ThreadKey {
                environment: environment.clone(),
                thread: thread.id.clone(),
            },
            project: ProjectKey {
                environment: environment.clone(),
                project: thread.project_id.clone(),
            },
            shell: thread,
        })
        .collect()
}

fn date(value: &Option<Option<t3_contracts::wire::WireDateTime>>) -> Option<&str> {
    value
        .as_ref()
        .and_then(Option::as_ref)
        .map(|date| date.0.as_str())
}

fn activity(thread: &OrchestrationV2ThreadShell) -> Option<String> {
    resolve_thread_activity_timestamp(
        thread
            .latest_user_message_at
            .as_ref()
            .map(|date| date.0.as_str()),
        &[
            date(&thread.latest_run_requested_at),
            date(&thread.latest_run_started_at),
            date(&thread.latest_run_completed_at),
        ],
        &thread.updated_at.0,
        &timestamp_ms,
    )
    .map(str::to_owned)
}

/// Adapts shared shells to client selectors; shelf, ordering and grouping policy stay in t3-client.
pub fn project_sidebar(
    shells: &[(EnvironmentId, Shared<SharedShellSnapshot>)],
    state: &SidebarState,
    route: Option<&ThreadKey>,
    now: i64,
    clear_missing_scope: bool,
    previous: Option<&SidebarList>,
) -> SidebarList {
    let settings = GroupingSettings::default();
    let projects: Vec<_> = shells
        .iter()
        .flat_map(|(environment, shell)| {
            shell
                .projects
                .iter()
                .map(move |project| EnvironmentProject {
                    environment: environment.clone(),
                    project: (**project).clone(),
                    // Scratch identity needs the configured scratchWorkspaceRoot (runtime seam).
                    is_scratch: false,
                })
        })
        .collect();
    let threads: Vec<_> = shells
        .iter()
        .flat_map(|(environment, shell)| thread_inputs(environment, shell))
        .collect();
    let mut shell_views = ThreadShellViews::default();
    shell_views.update(
        &shells
            .iter()
            .map(|(environment, shell)| EnvironmentShells {
                environment,
                enabled: true,
                threads: &shell.threads,
            })
            .collect::<Vec<_>>(),
        BTreeSet::new(),
    );
    let delegated = shell_views.delegated_under_parent;
    let groups = build_sidebar_snapshots(
        &projects,
        &settings,
        true,
        None,
        |_| None,
        |_| false,
        |_| false,
    );
    let activities = project_activity(
        &groups,
        &threads,
        true,
        &delegated,
        &timestamp_ms,
        |thread| {
            let source = thread.shell;
            get_thread_sort_timestamp(
                &ThreadSortInput {
                    created_at: &source.created_at.0,
                    updated_at: &source.updated_at.0,
                    latest_user_message_at: source
                        .latest_user_message_at
                        .as_ref()
                        .map(|date| date.0.as_str()),
                    latest_user_authored_message_at: source
                        .latest_user_authored_message_at
                        .as_ref()
                        .map(|date| date.as_ref().map(|date| date.0.as_str())),
                    messages: &[],
                },
                SortOrder::LatestUserMessage,
                &timestamp_ms,
            )
        },
    );
    // NSH has no locale collator; deterministic label/key ties replace localeCompare.
    let groups: Vec<_> = sort_logical_projects(&activities, false, false, |a, b| {
        a.group
            .label
            .cmp(&b.group.label)
            .then(a.group.key.cmp(&b.group.key))
    })
    .into_iter()
    .cloned()
    .collect();
    let scope = resolve_scopes(
        &groups,
        &state.project_scope,
        &settings,
        clear_missing_scope,
    );
    let scoped_projects: BTreeSet<_> = groups
        .iter()
        .filter(|group| scope.contains(&group.group.key))
        .flat_map(|group| group.group.member_project_refs.iter().cloned())
        .collect();
    let route = route.map(|key| resolve_row_thread_key(key, &threads));
    let options = Options {
        now,
        scope: (!scope.is_empty()).then_some(&scoped_projects),
        delegated_under_parent: &delegated,
        selected: &state.selected,
        route: route.as_ref(),
        working_shelf: true,
        active_order: ActiveOrder::InboxReturn,
        inbox: None,
    };
    let environments: BTreeMap<_, _> = shells
        .iter()
        .map(|(environment, shell)| {
            (
                environment.clone(),
                sidebar_rows(
                    environment,
                    shell,
                    &groups,
                    &options,
                    previous.and_then(|previous| previous.environments.get(environment)),
                ),
            )
        })
        .collect();
    let by_key: BTreeMap<_, _> = environments
        .values()
        .flat_map(|view| view.rows.iter())
        .map(|row| (row.key.clone(), row))
        .collect();
    // sidebar_rows accepts one environment. Its underlying client selector merges their order
    // and folders; concatenating independently sorted shelves would change cross-environment order.
    let views: Vec<_> = shells
        .iter()
        .flat_map(|(environment, shell)| {
            shell
                .threads
                .iter()
                .map(move |thread| present_thread_shell(environment.clone(), thread.clone(), None))
        })
        .collect();
    let inputs: Vec<_> = views
        .iter()
        .map(|view| ThreadInput::from_environment_shell(view, None, &timestamp_ms))
        .collect();
    let ordered = present_sidebar(&groups, &inputs, &options, &timestamp_ms, |_| false);
    let project_index: BTreeMap<_, _> = shells
        .iter()
        .flat_map(|(environment, shell)| {
            shell.projects.iter().map(move |project| {
                (
                    ProjectKey {
                        environment: environment.clone(),
                        project: project.id.clone(),
                    },
                    project,
                )
            })
        })
        .collect();
    let labels: BTreeMap<_, _> = groups
        .iter()
        .flat_map(|group| {
            group
                .group
                .member_project_refs
                .iter()
                .map(move |key| (key, &group.group.label))
        })
        .collect();
    let prior: BTreeMap<_, _> = previous
        .into_iter()
        .flat_map(|previous| previous.rows.iter())
        .map(|row| (&row.item.key, row))
        .collect();
    let rows: Vec<_> = ordered
        .rows
        .iter()
        .filter_map(|row| {
            let source = (*by_key.get(&row.source.thread.key)?).clone();
            let project = project_index
                .get(&row.source.thread.project)
                .map(|project| Shared((*project).clone()));
            let item = SidebarItem {
                key: source.key.clone(),
                title: source.thread.title.clone(),
                project: labels
                    .get(&row.source.thread.project)
                    .map(|label| (*label).clone())
                    .or_else(|| project.as_ref().map(|project| project.title.0.clone()))
                    .unwrap_or_default(),
                activity_at: activity(&source.thread),
                active: source.active,
                variant: match source.section {
                    Section::Pinned | Section::Active | Section::Working => RowVariant::Card,
                    Section::Snoozed | Section::Settled => RowVariant::Slim,
                },
            };
            let next = ListRow {
                source: Shared(source),
                item,
                project,
            };
            Some(match prior.get(&next.item.key) {
                Some(old) if old.as_ref() == &next => (*old).clone(),
                _ => Rc::new(next),
            })
        })
        .collect();
    let same_keys = previous.is_some_and(|previous| {
        previous.rows.len() == rows.len()
            && previous
                .rows
                .iter()
                .zip(&rows)
                .all(|(old, row)| old.item.key == row.item.key)
    });
    let keys = previous.filter(|_| same_keys).map_or_else(
        || Rc::new(rows.iter().map(|row| row.item.list_key()).collect()),
        |previous| previous.keys.clone(),
    );
    let folders = ordered
        .folders
        .iter()
        .map(|folder| SidebarFolder {
            group: folder.group.clone(),
            rows: folder.rows.clone(),
            dot: folder.dot,
        })
        .collect();
    let next_snooze_wake = ordered.next_snooze_wake;
    SidebarList {
        rows,
        keys,
        groups,
        scope,
        scoped_projects: scoped_projects.into_iter().collect(),
        next_snooze_wake,
        folders,
        environments,
    }
}

fn search_rows(list: &SidebarList, query: &str, now: i64) -> Vec<Rc<ListRow>> {
    use t3_client::thread_views::{threads::search::thread_search_subject, visits::ThreadOpens};
    use t3_client::view_logic::shell::sidebar::search_threads;
    let opens = ThreadOpens::default();
    search_threads(
        &list.rows,
        &[],
        |row| row.item.key.clone(),
        query,
        |row| {
            thread_search_subject(
                &row.item.key,
                &row.source.thread,
                Some(&row.item.project),
                None,
                &opens,
                &timestamp_ms,
            )
        },
        now as f64,
    )
    .title_matches
    .into_iter()
    .cloned()
    .collect()
}

fn empty_message(list: &SidebarList) -> String {
    if list.groups.is_empty() {
        return t("No projects yet");
    }
    if list.scope.is_empty() {
        return t("No threads yet");
    }
    let name = list
        .groups
        .iter()
        .find(|group| list.scope.contains(&group.group.key))
        .map(|group| group.group.label.clone());
    let settled = list
        .rows
        .iter()
        .any(|row| row.source.section == Section::Settled);
    match (list.scope.len(), name, settled) {
        (1, Some(project), true) => tr(
            "No active threads in {project}",
            &[("project", Value::Text(project))],
        ),
        (_, _, true) => t("No active threads in these projects"),
        (1, Some(project), false) => tr(
            "No threads in {project} yet",
            &[("project", Value::Text(project))],
        ),
        _ => t("No threads in these projects yet"),
    }
}

/// A list entry's key: the row's own, or the marker's stable name.
fn list_item_key(item: &SidebarListItem, rows: &[Rc<ListRow>]) -> SharedString {
    match item {
        SidebarListItem::Thread(index) => rows
            .get(*index)
            .map(|row| row.item.list_key())
            .unwrap_or_default(),
        SidebarListItem::Marker(marker) => format!("marker:{}", marker.name()).into(),
        SidebarListItem::SettledMore => "settled:more".into(),
    }
}

type ShelfCount = fn(&SidebarListItems) -> usize;
type ShelfAnchored = fn(&SidebarListItems) -> bool;

/// Port of Sidebar.tsx's marker switch (SidebarListMarkers.tsx). The drag-only markers stay
/// closed: SB-DRAG has not landed, so no drag is ever in progress.
fn shelf_marker(
    marker: Marker,
    items: Memo<Shared<SidebarListItems>>,
    expansion: Signal<ShelfExpansion>,
    store: Stored<Option<crate::runtime::services::shell::SidebarShelvesStore>>,
) -> AnyElement {
    let tasks = crate::runtime::tasks::TaskOwner::here();
    let header = |label: &'static str, count: ShelfCount, anchored: ShelfAnchored| {
        let class_name = items
            .with_untracked(|list| anchored(list))
            .then(|| "mt-auto".to_owned());
        shelves::sidebar_section_header(shelves::SidebarSectionHeaderProps {
            marker,
            label: Text::from(label),
            count: Prop::bind(move || items.with(|list| count(list))),
            class_name,
            dragging: false.into(),
            is_drop_target: false.into(),
            toggle: crate::ui::collapsible::CollapsibleHandle::controlled(
                move || expansion.with(|expansion| expansion.of(marker).unwrap_or(false)),
                || false,
                move |expanded| {
                    crate::runtime::services::shell::toggle_sidebar_shelf(
                        expansion,
                        store.get(),
                        marker,
                        expanded,
                        tasks,
                    )
                },
            ),
        })
        .into_element()
    };
    match marker {
        Marker::PinnedHeader => {
            shelves::sidebar_drag_boundary(marker, "Pinned".into(), false.into(), false.into())
                .into_element()
        }
        Marker::PinnedDivider => {
            shelves::sidebar_drag_boundary(marker, "Active".into(), false.into(), false.into())
                .into_element()
        }
        Marker::ActivePlaceholder => shelves::sidebar_section_placeholder(
            marker,
            "Active".into(),
            false.into(),
            false.into(),
        )
        .into_element(),
        Marker::SettledPlaceholder => shelves::sidebar_section_placeholder(
            marker,
            "Settled".into(),
            false.into(),
            false.into(),
        )
        .into_element(),
        Marker::WorkingHeader => header("Working", |list| list.working_count, |_| true),
        Marker::SnoozedHeader => header(
            "Snoozed",
            |list| list.snoozed_count,
            |list| list.working_count == 0,
        ),
        Marker::SettledHeader => header(
            "Settled",
            |list| list.settled_count,
            |list| list.working_count + list.snoozed_count == 0,
        ),
    }
}

pub fn sidebar() -> impl IntoElement {
    let runtime = rt();
    let nav = navigator();
    let undo_notice = row::thread_actions().map(actions::footer_undo);
    let snooze_dialog = snooze::dialog_host();
    let selection_store = selection::use_thread_selection_store();
    selection::install_escape(selection_store);
    let state = sidebar_state();
    let environments = runtime.environments_signal();
    let ids = computed(move || {
        environments.with(|list| {
            list.iter()
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>()
        })
    });
    let shells = computed(move || {
        ids.get()
            .into_iter()
            .map(|id| {
                let shell = runtime.store.shell(id.clone());
                (id, shell)
            })
            .collect::<Vec<_>>()
    });
    let loaded = computed(move || {
        shells.with(|shells| {
            shells
                .iter()
                .filter_map(|(id, shell)| shell.get().map(|shell| (id.clone(), shell)))
                .collect::<Vec<_>>()
        })
    });
    let ready = computed(move || {
        environments.with(|environments| {
            !environments.is_empty()
                && environments
                    .iter()
                    .all(|entry| entry.state.phase == t3_client::connection::Phase::Connected)
        }) && loaded.with(|shells| shells.len()) == ids.with(|ids| ids.len())
    });
    let clock = signal(nsh_core::time::unix_ms() as i64);
    let previous = stored(None::<Rc<SidebarList>>);
    let model = computed(move || {
        let route = match nav.top() {
            Route::Thread(key) | Route::ThreadPanel(key, _) => Some(key),
            _ => None,
        };
        let next = previous.with(|previous| {
            loaded.with(|shells| {
                project_sidebar(
                    shells,
                    &state.get(),
                    route.as_ref(),
                    clock.get(),
                    ready.get(),
                    previous.as_deref(),
                )
            })
        });
        let next = Rc::new(next);
        previous.replace(Some(next.clone()));
        Shared(next)
    });
    let wake = computed(move || model.with(|model| model.next_snooze_wake));
    // The timer belongs to the effect: NSH cancels it when the wake changes or the list goes.
    effect(move || {
        if let Some(wake) = wake.get() {
            let delay = wake.saturating_sub(nsh_core::time::unix_ms() as i64).max(1) as u64;
            set_timeout(Duration::from_millis(delay), move || {
                clock.set(nsh_core::time::unix_ms() as i64)
            });
        }
    });
    let query = signal(String::new());
    let active_result = signal(0usize);
    let searching = computed(move || !query.get().trim().is_empty());
    let results = computed(move || {
        model.with(|model| search_rows(model, &query.get(), untracked(|| clock.get())))
    });
    // SB-SHELVES: the shelves' markers and settled tail around the client's ordered rows
    // (`sidebar_list::sidebar_list_items`); search lists matching rows without markers.
    let shelves_store = use_context::<crate::runtime::services::shell::SidebarShelvesStore>();
    let expansion = signal(ShelfExpansion::default());
    crate::runtime::services::shell::load_sidebar_shelves(expansion, shelves_store.clone());
    let shelves_store = stored(shelves_store);
    let settled_visible = signal(SETTLED_TAIL_INITIAL_COUNT);
    // `settledTail` resets to its first page when the project scope changes.
    let scope_key = computed(move || state.with(|state| state.project_scope.clone()));
    effect(move || {
        scope_key.with(|_| ());
        settled_visible.set(SETTLED_TAIL_INITIAL_COUNT);
    });
    let sections = computed(move || {
        model.with(|model| {
            (
                model
                    .rows
                    .iter()
                    .map(|row| row.source.section)
                    .collect::<Vec<_>>(),
                model.rows.iter().position(|row| row.source.active),
            )
        })
    });
    let shelf_items = computed(move || {
        sections.with(|(sections, route)| {
            Shared(Rc::new(sidebar_list_items(
                sections,
                *route,
                expansion.get(),
                settled_visible.get(),
            )))
        })
    });
    let visible_threads = computed(move || {
        if searching.get() {
            return results.with(|rows| {
                rows.iter()
                    .map(|row| (row.item.key.clone(), row.source.section))
                    .collect()
            });
        }
        shelf_items.with(|list| {
            model.with(|model| {
                list.items
                    .iter()
                    .filter_map(|item| {
                        let SidebarListItem::Thread(index) = *item else {
                            return None;
                        };
                        let row = model.rows.get(index)?;
                        Some((row.item.key.clone(), row.source.section))
                    })
                    .collect::<Vec<_>>()
            })
        })
    });
    let available_threads = computed(move || {
        model.with(|model| {
            model
                .rows
                .iter()
                .map(|row| row.item.key.clone())
                .collect::<Vec<_>>()
        })
    });
    let interactions = selection::Interactions {
        store: selection_store,
        visible_threads,
        available_threads,
        tasks: crate::runtime::tasks::TaskOwner::here(),
    };
    provide_context(interactions);
    let count = computed(move || {
        if searching.get() {
            results.with(|rows| rows.len())
        } else {
            shelf_items.with(|list| list.items.len())
        }
    });
    let normal_keys = computed(move || {
        shelf_items.with(|list| {
            model.with(|model| {
                Shared(Rc::new(
                    list.items
                        .iter()
                        .map(|item| list_item_key(item, &model.rows))
                        .collect::<Vec<_>>(),
                ))
            })
        })
    });
    let keys = computed(move || {
        if searching.get() {
            Shared(Rc::new(results.with(|rows| {
                rows.iter().map(|row| row.item.list_key()).collect()
            })))
        } else {
            normal_keys.get()
        }
    });
    let list_ref = element_ref();
    let environment_labels = computed(move || {
        environments.with(|entries| {
            entries
                .iter()
                .map(|entry| (entry.id.clone(), entry.label.clone()))
                .collect::<BTreeMap<_, _>>()
        })
    });
    let show_environments = computed(move || {
        model.with(|model| t3_client::workspace::sidebar::groups_span_environments(&model.groups))
    });
    let list = virtual_list(count, move |at| {
        // The list can move a cell to another shelf while its old row is resolving a memo.
        // Publish its next kind and contents after the list bindings have settled. Reading
        // this signal cannot pull the list layout and dispose the row halfway through a read.
        let project_cell = move || {
            if searching.get() {
                return (
                    None,
                    results.with(|rows| rows.get(at.index()).cloned().map(Shared)),
                );
            }
            match shelf_items.with(|list| list.items.get(at.index()).copied()) {
                Some(SidebarListItem::Thread(index)) => (
                    None,
                    model.with(|model| model.rows.get(index).cloned().map(Shared)),
                ),
                marker => (marker, None),
            }
        };
        let cell = signal(untracked(project_cell));
        effect(move || cell.set(project_cell()));
        let kind = computed(move || cell.with(|(kind, _)| *kind));
        let data = computed(move || cell.with(|(_, row)| row.clone()));
        let item = computed(move || data.with(|row| row.as_ref().map(|row| row.item.clone())));
        let thread = computed(move || {
            data.with(|row| row.as_ref().map(|row| Shared(row.source.thread.clone())))
        });
        let project =
            computed(move || data.with(|row| row.as_ref().and_then(|row| row.project.clone())));
        let extras = computed(move || {
            data.with(|row| {
                row.as_ref()
                    .map(|row| row::RowExtras {
                        selected: row.source.selected
                            || selection_store.is_selected(&selection::row_key(&row.item.key))
                            || (searching.get() && active_result.get() == at.index()),
                        environment_label: show_environments
                            .get()
                            .then(|| {
                                environment_labels
                                    .with(|labels| labels.get(&row.item.key.environment).cloned())
                            })
                            .flatten(),
                        woke_at: row
                            .source
                            .woke
                            .then(|| date(&row.source.thread.snoozed_until).map(str::to_owned))
                            .flatten(),
                        snooze_wake_label: (row.source.section == Section::Snoozed).then(|| {
                            let label = t3_client::thread_views::shelves::snooze_wake_label(
                                date(&row.source.thread.snoozed_until).and_then(timestamp_ms),
                                Some(crate::state::clock::now_ms().max(clock.get())),
                            );
                            if label == "now" { t("now") } else { label }
                        }),
                        ..Default::default()
                    })
                    .unwrap_or_default()
            })
        });
        // A recycled cell rebuilds only when its kind changes (row, marker, Show more); a row
        // cell reads its row through the memos above.
        dynamic(move || match kind.get() {
            Some(SidebarListItem::Marker(marker)) => {
                untracked(|| shelf_marker(marker, shelf_items, expansion, shelves_store))
            }
            Some(_) => untracked(|| {
                shelves::sidebar_settled_more(
                    Prop::bind(move || expansion.get().settled),
                    Prop::bind(move || shelf_items.with(|list| list.hidden_settled)),
                    shelves::OlderSettledState::Ready.into(),
                    move || settled_visible.update(|count| *count += SETTLED_TAIL_PAGE_COUNT),
                )
                .into_element()
            }),
            None => untracked(|| {
                // The direct API avoids thread_row's full-shell lookup when a cell is recycled.
                view()
                    .app_class(tw("pb-px"))
                    .child(row::sidebar_thread_row(item, thread, project, extras, nav))
                    .into_element()
            }),
        })
    })
    .keys(move || keys.with(|keys| keys.0.as_ref().clone()))
    .element_ref(&list_ref)
    .estimated_item_size(px(82.0))
    .content_padding_x(px(8.0))
    .flex_1()
    .min_h(px(0.0))
    .test_id("sidebar-threads");
    let reveal = stored(RouteReveal::default());
    // History/deep links are external navigation: the client widens scope and reveals once.
    effect(move || {
        let route = nav.top().thread().cloned();
        let searching = searching.get();
        let list = model.get();
        let (pending, widen) = reveal.with(|reveal| {
            reveal.resolve(
                route.as_ref(),
                searching,
                !list.scope.is_empty(),
                |_| list.rows.iter().any(|row| row.source.active),
                |_| {
                    let mut unscoped = state.get_untracked();
                    unscoped.project_scope.clear();
                    loaded
                        .with(|shells| {
                            project_sidebar(
                                shells,
                                &unscoped,
                                route.as_ref(),
                                untracked(|| clock.get()),
                                false,
                                None,
                            )
                        })
                        .rows
                        .iter()
                        .any(|row| row.source.active)
                },
            )
        });
        if widen {
            selection_store.clear_selection();
            state.update(|state| {
                state.project_scope.clear();
                state.selected.clear();
            });
        } else if pending.is_some()
            && !searching
            && let Some(row) = list.rows.iter().position(|row| row.source.active)
            && let Some(index) = shelf_items.with_untracked(|items| {
                items
                    .items
                    .iter()
                    .position(|item| *item == SidebarListItem::Thread(row))
            })
            && let Ok(index) = u32::try_from(index)
        {
            list_ref.scroll_to_index(index, percentage(0.5), px(0.0), false);
            reveal.with(RouteReveal::complete);
        }
    });
    let groups = computed(move || {
        model.with(|model| {
            model
                .groups
                .iter()
                .map(|group| group.group.clone())
                .collect::<Vec<ProjectGroup>>()
        })
    });
    let selected = computed(move || model.with(|model| model.scope.clone()));
    let mut props = chrome::SidebarThreadHeaderProps::new(
        query,
        Prop::bind(move || groups.with(|groups| !groups.is_empty())),
    );
    props.new_thread_disabled = Prop::bind(move || {
        groups.with(|groups| groups.is_empty())
            || model.with(|model| !model.scope.is_empty() && model.scoped_projects.is_empty())
    });
    props.on_clear_search = Rc::new(move || {
        batch(|| {
            query.set(String::new());
            active_result.set(0);
        });
        list_ref.scroll_to_offset(px(0.0), false);
    });
    props.search_result_count = Prop::bind(move || results.with(|rows| rows.len()));
    props.active_search_result_index = Prop::bind(move || active_result.get());
    props.search_results_ref = Prop::bind(move || searching.get().then_some(list_ref));
    props.on_search_query_change = Rc::new(move |value| {
        batch(|| {
            query.set(value);
            active_result.set(0);
        });
        list_ref.scroll_to_offset(px(0.0), false);
    });
    props.on_search_key_down = Rc::new(move |event| {
        if !searching.get_untracked() {
            return;
        }
        let count = count.get_untracked();
        match event.key.as_ref() {
            "Escape" => query.set(String::new()),
            "ArrowDown" if count > 0 => active_result.update(|index| *index = (*index + 1) % count),
            "ArrowUp" if count > 0 => {
                active_result.update(|index| *index = (*index + count - 1) % count)
            }
            "Enter" => {
                if let Some(key) = results.with(|rows| {
                    rows.get(active_result.get_untracked())
                        .map(|row| row.item.key.clone())
                }) {
                    query.set(String::new());
                    selection_store.clear_selection();
                    selection_store.set_anchor(&selection::row_key(&key));
                    nav.open(Route::Thread(key));
                }
            }
            _ => {}
        }
    });
    props.on_new_thread = Rc::new(move || {
        state.update(|state| {
            state.new_thread_projects = model.with(|model| model.scoped_projects.clone())
        });
        nav.open(Route::NewThread);
    });
    let scope_props = chrome::ProjectScopeProps {
        groups,
        selected,
        favicon_source: Rc::new(|_| None),
        on_change: Rc::new(move |keys| {
            selection_store.clear_selection();
            state.update(|state| {
                state.project_scope = keys;
                state.selected.clear();
            });
            list_ref.scroll_to_offset(px(0.0), false);
        }),
    };
    let empty = computed(move || {
        ready.get()
            && if searching.get() {
                count.get() == 0
            } else {
                model.with(|model| {
                    model
                        .rows
                        .iter()
                        .all(|row| row.source.section == Section::Settled)
                        && (model.rows.is_empty() || !model.scope.is_empty())
                })
            }
    });
    view().w_full().h_full()
        .app_class(tw_with(Scope::Sidebar, "bg-sidebar text-foreground", &[]))
        .child(chrome::sidebar_chrome_header(Default::default()))
        .child(view().app_class(tw("relative z-[1] flex w-full min-w-0 flex-col p-[var(--sidebar-content-inset)]"))
            .child(chrome::sidebar_thread_header(props, move || chrome::sidebar_project_scope(scope_props.clone()))))
        .child(when(move || empty.get(), move || {
            view().app_class(tw("flex flex-col items-center gap-2 px-2 py-6 text-center text-xs text-muted-foreground/60"))
                .child(text(move || if searching.get() { t("No threads found") } else { model.with(|model| empty_message(model)) }))
                .child(when(move || !searching.get() && groups.with(|groups| groups.is_empty()), move || {
                    nsh_base::button().accessibility_label(t("Add project")).on_press(move || nav.open(Route::AddProject))
                        .app_class(tw_with(Scope::Sidebar, "inline-flex cursor-pointer items-center gap-1.5 rounded-md border border-sidebar-border px-2.5 py-1 text-2xs font-medium text-sidebar-muted-foreground transition-colors hover:bg-sidebar-row-hover hover:text-sidebar-foreground", &[]))
                        .child(icon(Icon::Plus, 12.0, move || crate::theme::palette().sidebar_icon)).child(text(t("Add project")))
                }))
                .child(when(move || !searching.get() && selected.with(|keys| !keys.is_empty()), move || {
                    nsh_base::button().accessibility_label(t("Show all projects"))
                        .on_press(move || { state.update(|state| {state.project_scope.clear(); state.selected.clear();}); selection_store.clear_selection(); list_ref.scroll_to_offset(px(0.0), false); })
                        .app_class(tw_with(Scope::Sidebar, "inline-flex cursor-pointer items-center gap-1.5 rounded-md border border-sidebar-border px-2.5 py-1 text-2xs font-medium text-sidebar-muted-foreground transition-colors hover:bg-sidebar-row-hover hover:text-sidebar-foreground", &[]))
                        .child(text(t("Show all projects")))
                }))
        }))
        .child(list)
        .child(snooze_dialog)
        .child(chrome::sidebar_chrome_footer(false.into(), Route::Home.into(), chrome::ChromeFooterSlots {
            undo_notice,
            ..Default::default()
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use t3_client::ClientStore;
    use t3_client::types::ThreadId;

    fn key(environment: &str, thread: &str) -> ThreadKey {
        ThreadKey {
            environment: EnvironmentId(environment.into()),
            thread: ThreadId(thread.into()),
        }
    }

    fn thread(id: &str, extra: Value) -> Value {
        let mut value = json!({
            "id":id,"projectId":"p","title":id,
            "providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},
            "runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,
            "lineage":{"rootThreadId":id,"parentThreadId":null,"relationshipToParent":null},
            "forkedFrom":null,"activeProviderThreadId":null,"latestRunId":null,"activeRunId":null,
            "status":"idle","pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":null,
            "hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,
            "createdBy":"user","creationSource":"web","createdAt":"2026-11-13T10:00:00.000Z",
            "updatedAt":"2026-11-13T10:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"deletedAt":null
        });
        value
            .as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("extra").clone());
        value
    }

    fn project(id: &str, title: &str, root: &str, extra: Value) -> Value {
        let mut value = json!({"id":id,"title":title,"workspaceRoot":root,"defaultModelSelection":null,
            "scripts":[],"createdAt":"2026-11-13T10:00:00.000Z","updatedAt":"2026-11-13T10:00:00.000Z"});
        value
            .as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("extra").clone());
        value
    }

    fn install(
        store: &ClientStore,
        environment: &str,
        threads: Vec<Value>,
        projects: Vec<Value>,
    ) -> (EnvironmentId, Shared<SharedShellSnapshot>) {
        let environment = EnvironmentId(environment.into());
        let session = store.begin_session(environment.clone());
        store.apply_shell(&environment, session, vec![serde_json::from_value(json!({
            "kind":"snapshot","snapshot":{"schemaVersion":1,"snapshotSequence":1,"threads":threads,"projects":projects,"archivedThreads":[]}
        })).expect("canonical snapshot")]);
        let shell = store.shared_shell(&environment).expect("shared shell");
        (environment, Shared(shell))
    }

    fn p() -> Value {
        project("p", "Project", "/repo", json!({}))
    }
    fn now() -> i64 {
        timestamp_ms("2026-11-13T12:00:00.000Z").expect("time")
    }

    #[test]
    fn merges_environments_in_client_shelf_order_and_keeps_scoped_ids() {
        let store = ClientStore::default();
        let a = install(
            &store,
            "a",
            vec![
                thread(
                    "same",
                    json!({"latestRunId":"run","status":"running","latestRunStartedAt":"2026-11-13T11:00:00.000Z"}),
                ),
                thread(
                    "pin-a",
                    json!({"pinnedAt":"2026-11-13T10:00:00.000Z","pinOrderKey":"b"}),
                ),
                thread(
                    "settled",
                    json!({"settledOverride":"settled","settledAt":"2026-11-13T11:30:00.000Z"}),
                ),
            ],
            vec![p()],
        );
        let b = install(
            &store,
            "b",
            vec![
                thread("same", json!({})),
                thread(
                    "pin-b",
                    json!({"pinnedAt":"2026-11-13T10:00:00.000Z","pinOrderKey":"a"}),
                ),
                thread(
                    "nap",
                    json!({"snoozedAt":"2026-11-13T10:30:00.000Z","snoozedUntil":"2026-11-14T10:00:00.000Z"}),
                ),
            ],
            vec![p()],
        );
        let result = project_sidebar(
            &[a, b],
            &SidebarState::default(),
            Some(&key("b", "same")),
            now(),
            true,
            None,
        );
        assert_eq!(
            result
                .rows
                .iter()
                .map(|row| (row.item.key.clone(), row.source.section))
                .collect::<Vec<_>>(),
            vec![
                (key("b", "pin-b"), Section::Pinned),
                (key("a", "pin-a"), Section::Pinned),
                (key("b", "same"), Section::Active),
                (key("a", "same"), Section::Working),
                (key("b", "nap"), Section::Snoozed),
                (key("a", "settled"), Section::Settled),
            ]
        );
        assert!(result.rows[2].source.active);
        assert_eq!(result.rows[4].item.variant, RowVariant::Slim);
        assert_eq!(
            result.next_snooze_wake,
            timestamp_ms("2026-11-14T10:00:00.000Z")
        );
        assert_eq!(
            result
                .keys
                .iter()
                .map(|key| key.as_str())
                .collect::<BTreeSet<_>>()
                .len(),
            6
        );
        assert_eq!(
            result.groups.len(),
            2,
            "identical project ids on different hosts are distinct"
        );
    }

    #[test]
    fn scope_expands_repository_members_and_never_fails_open_while_reconnecting() {
        let store = ClientStore::default();
        let repo = json!({"canonicalKey":"repo:shared","rootPath":"/repo", "locator":{"source":"git-remote","remoteName":"origin","remoteUrl":"https://example.test/shared.git"}});
        let a = install(
            &store,
            "a",
            vec![thread("a", json!({}))],
            vec![project(
                "p",
                "Shared",
                "/repo",
                json!({"repositoryIdentity":repo}),
            )],
        );
        let b = install(
            &store,
            "b",
            vec![
                thread("b", json!({})),
                thread("other", json!({"projectId":"q"})),
            ],
            vec![
                project("p", "Shared", "/repo", json!({"repositoryIdentity":repo})),
                project("q", "Other", "/other", json!({})),
            ],
        );
        let shells = [a, b];
        let all = project_sidebar(&shells, &SidebarState::default(), None, now(), true, None);
        let shared = all
            .groups
            .iter()
            .find(|group| group.group.members.len() == 2)
            .expect("repository group");
        let state = SidebarState {
            project_scope: vec![shared.group.key.clone()],
            ..Default::default()
        };
        let scoped = project_sidebar(&shells, &state, None, now(), true, Some(&all));
        assert_eq!(scoped.scoped_projects.len(), 2);
        assert_eq!(
            scoped
                .rows
                .iter()
                .map(|row| row.item.key.clone())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([key("a", "a"), key("b", "b")])
        );
        let missing = SidebarState {
            project_scope: vec![LogicalProjectKey::Repository("missing".into())],
            ..Default::default()
        };
        let disconnected = project_sidebar(&shells, &missing, None, now(), false, None);
        assert!(
            disconnected.rows.is_empty(),
            "missing scope must not show every project during reconnect"
        );
        assert_eq!(disconnected.scope, missing.project_scope);
        let ready = project_sidebar(&shells, &missing, None, now(), true, None);
        assert!(ready.scope.is_empty());
        assert_eq!(
            ready.rows.len(),
            3,
            "confirmed removed scope clears only after loading"
        );
    }

    #[test]
    fn project_picker_uses_recent_activity_excluding_archived_and_hidden_threads() {
        let store = ClientStore::default();
        let shell = install(
            &store,
            "a",
            vec![
                thread(
                    "old",
                    json!({"projectId":"old","updatedAt":"2026-11-13T10:01:00.000Z"}),
                ),
                thread(
                    "new",
                    json!({"projectId":"new","updatedAt":"2026-11-13T10:05:00.000Z"}),
                ),
                thread(
                    "hidden",
                    json!({"projectId":"old","updatedAt":"2026-11-13T11:00:00.000Z","lineage":{"rootThreadId":"old","parentThreadId":"old","relationshipToParent":"subagent"}}),
                ),
                thread(
                    "archived",
                    json!({"projectId":"old","updatedAt":"2026-11-13T11:30:00.000Z","archivedAt":"2026-11-13T11:40:00.000Z"}),
                ),
            ],
            vec![
                project("old", "A old", "/old", json!({})),
                project("new", "Z new", "/new", json!({})),
            ],
        );
        let result = project_sidebar(&[shell], &SidebarState::default(), None, now(), true, None);
        assert_eq!(
            result
                .groups
                .iter()
                .map(|group| group.group.label.as_str())
                .collect::<Vec<_>>(),
            vec!["Z new", "A old"]
        );
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn unchanged_rows_and_keys_survive_metadata_updates_and_selection_is_scoped() {
        let store = ClientStore::default();
        let a = install(
            &store,
            "a",
            vec![thread("one", json!({})), thread("two", json!({}))],
            vec![p()],
        );
        let first = project_sidebar(
            std::slice::from_ref(&a),
            &SidebarState::default(),
            None,
            now(),
            true,
            None,
        );
        let again = project_sidebar(
            std::slice::from_ref(&a),
            &SidebarState::default(),
            None,
            now(),
            true,
            Some(&first),
        );
        assert!(Rc::ptr_eq(&first.keys, &again.keys));
        assert!(
            first
                .rows
                .iter()
                .zip(&again.rows)
                .all(|(a, b)| Rc::ptr_eq(a, b))
        );
        let mut state = SidebarState::default();
        state.selected.insert(key("a", "one"));
        let selected = project_sidebar(
            std::slice::from_ref(&a),
            &state,
            None,
            now(),
            true,
            Some(&again),
        );
        assert!(Rc::ptr_eq(&again.keys, &selected.keys));
        for row in &selected.rows {
            let old = again
                .rows
                .iter()
                .find(|old| old.item.key == row.item.key)
                .expect("old row");
            assert_eq!(row.source.selected, row.item.key == key("a", "one"));
            assert_eq!(Rc::ptr_eq(old, row), row.item.key != key("a", "one"));
        }
        let session = store.snapshot().environments[&a.0].session;
        store.apply_shell(
            &a.0,
            session,
            vec![
                serde_json::from_value(json!({
                    "kind":"thread.updated", "sequence":2, "location":"active",
                    "thread":thread("two", json!({"title":"Renamed without reordering"})),
                }))
                .expect("canonical update"),
            ],
        );
        let shell = (
            a.0.clone(),
            Shared(store.shared_shell(&a.0).expect("updated shell")),
        );
        let changed = project_sidebar(&[shell], &state, None, now(), true, Some(&selected));
        assert!(Rc::ptr_eq(&selected.keys, &changed.keys));
        for row in &changed.rows {
            let old = selected
                .rows
                .iter()
                .find(|old| old.item.key == row.item.key)
                .expect("old row");
            assert_eq!(Rc::ptr_eq(old, row), row.item.key != key("a", "two"));
            if row.item.key == key("a", "two") {
                assert_eq!(row.item.title, "Renamed without reordering");
            }
        }
        let mut first = changed.rows[0].item.clone();
        let mut second = first.clone();
        first.key = key("a:b", "c");
        second.key = key("a", "b:c");
        assert_ne!(
            first.list_key(),
            second.list_key(),
            "scoped keys are collision-free"
        );
    }

    #[test]
    fn local_search_uses_client_title_branch_and_project_ranking() {
        let store = ClientStore::default();
        let shell = install(
            &store,
            "a",
            vec![
                thread("Implement sidebar", json!({})),
                thread("Different", json!({"branch":"feature/sidebar"})),
            ],
            vec![p()],
        );
        let result = project_sidebar(&[shell], &SidebarState::default(), None, now(), true, None);
        assert_eq!(search_rows(&result, "sidebar", now()).len(), 2);
        assert_eq!(search_rows(&result, "Project", now()).len(), 2);
        assert!(search_rows(&result, "nothing-matches-this", now()).is_empty());
    }

    #[test]
    fn a_snooze_deadline_moves_the_row_using_client_effective_snooze() {
        let store = ClientStore::default();
        let shell = install(
            &store,
            "a",
            vec![thread(
                "nap",
                json!({"snoozedAt":"2026-11-13T10:30:00.000Z","snoozedUntil":"2026-11-13T12:00:00.000Z"}),
            )],
            vec![p()],
        );
        let before = project_sidebar(
            std::slice::from_ref(&shell),
            &SidebarState::default(),
            None,
            now() - 1,
            true,
            None,
        );
        assert_eq!(before.rows[0].source.section, Section::Snoozed);
        assert_eq!(before.next_snooze_wake, Some(now()));
        let after = project_sidebar(
            &[shell],
            &SidebarState::default(),
            None,
            now(),
            true,
            Some(&before),
        );
        assert_eq!(after.rows[0].source.section, Section::Active);
        assert_eq!(after.next_snooze_wake, None);
    }
}
