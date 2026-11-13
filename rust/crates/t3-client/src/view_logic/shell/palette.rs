//! Command-palette decisions. Items carry action values/targets; rendering, icons and command
//! execution belong to the UI adapter. Time and canonical search subjects are explicit inputs.
use super::{
    search::{self, SearchMatch, ThreadSubject},
    sidebar,
};
use crate::{
    types::{EnvironmentId, ThreadKey},
    view_logic::JsTrim,
    workspace::ProjectKey,
};
use std::collections::{BTreeMap, BTreeSet};
pub const RECENT_THREAD_LIMIT: usize = 12;
pub const THREAD_LIMIT: usize = 20;
pub const SECONDARY_LIMIT: usize = 3;
/// Connection classification and machine resolution come from the canonical catalog adapter.
pub struct EnvironmentLookupInput<'a> {
    pub id: &'a EnvironmentId,
    pub label: &'a str,
    pub primary: bool,
    pub desktop_local: bool,
    pub machine: t3_contracts::environment::EnvironmentMachineKind,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentLocation {
    pub local: bool,
    pub label: String,
    pub machine: t3_contracts::environment::EnvironmentMachineKind,
}
pub type EnvironmentLookups = (
    BTreeMap<EnvironmentId, String>,
    BTreeMap<EnvironmentId, EnvironmentLocation>,
);
pub fn environment_lookups(environments: &[EnvironmentLookupInput<'_>]) -> EnvironmentLookups {
    let labels = environments
        .iter()
        .map(|e| (e.id.clone(), e.label.into()))
        .collect();
    let locations = environments
        .iter()
        .map(|e| {
            (
                e.id.clone(),
                EnvironmentLocation {
                    local: e.primary || e.desktop_local,
                    label: if e.primary {
                        "Local".into()
                    } else if e.desktop_local {
                        format!("{} (Local)", e.label)
                    } else {
                        e.label.into()
                    },
                    machine: e.machine.clone(),
                },
            )
        })
        .collect();
    (labels, locations)
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OverlayMode {
    #[default]
    Command,
    Files,
    Content,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    AddProject,
    NewThreadIn,
    ChangeTheme,
    Search {
        query: String,
        linked_threads: Option<Vec<ThreadKey>>,
    },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnContext {
    pub query: String,
    pub linked_search: Option<Intent>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UiState {
    pub open: bool,
    pub mode: OverlayMode,
    pub intent: Option<Intent>,
    pub session: u64,
    pub parent_search: Option<ReturnContext>,
    pub restored_search: Option<ReturnContext>,
}
pub enum UiAction {
    SetOpen(bool),
    ToggleMode(OverlayMode),
    OpenIntent(Intent),
    OpenChild {
        mode: OverlayMode,
        context: ReturnContext,
    },
    BackToCommand,
}
impl UiState {
    pub fn apply(&mut self, action: UiAction) {
        let session = self.session;
        let next = match action {
            UiAction::SetOpen(open) => Self {
                open,
                mode: if open {
                    OverlayMode::Command
                } else {
                    self.mode
                },
                intent: open.then(|| self.intent.clone()).flatten(),
                session,
                ..Default::default()
            },
            UiAction::ToggleMode(mode) => Self {
                open: !(self.open && self.mode == mode),
                mode: if self.open && self.mode == mode {
                    self.mode
                } else {
                    mode
                },
                session,
                ..Default::default()
            },
            UiAction::OpenIntent(intent) => Self {
                open: true,
                mode: OverlayMode::Command,
                intent: Some(intent),
                session: session + 1,
                ..Default::default()
            },
            UiAction::OpenChild { mode, context } => Self {
                open: true,
                mode,
                intent: None,
                session,
                parent_search: Some(context),
                restored_search: None,
            },
            UiAction::BackToCommand => {
                if !self.open || self.mode == OverlayMode::Command {
                    return;
                }
                Self {
                    open: true,
                    mode: OverlayMode::Command,
                    session,
                    restored_search: self.parent_search.clone(),
                    ..Default::default()
                }
            }
        };
        *self = next;
    }
}
#[derive(Debug, Clone, Default)]
pub struct MountState {
    pub mounted: usize,
    pub state: UiState,
}
impl MountState {
    pub fn mount(&mut self) {
        self.mounted += 1;
    }
    pub fn unmount(&mut self) {
        self.mounted = self.mounted.saturating_sub(1);
        if self.mounted == 0 {
            self.state = UiState {
                session: self.state.session + 1,
                ..Default::default()
            };
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnFocus {
    Destination,
    Opener,
    Default,
}
pub fn return_focus(
    captured: bool,
    same_href: bool,
    same_focus_request: bool,
    opener_connected: bool,
    opener_disabled: bool,
) -> ReturnFocus {
    if !captured || !same_href || !same_focus_request {
        ReturnFocus::Destination
    } else if opener_connected && !opener_disabled {
        ReturnFocus::Opener
    } else {
        ReturnFocus::Default
    }
}
#[derive(Debug, Clone)]
pub struct Item {
    pub value: String,
    pub title: String,
    pub search_title: Option<String>,
    pub styled_title: bool,
    pub description: Option<String>,
    pub search_terms: Vec<String>,
    pub thread_search: Option<ThreadSubject>,
    pub thread: Option<ThreadKey>,
    pub project: Option<ProjectKey>,
    pub match_hint: Option<search::MatchHint>,
    pub disabled: bool,
    pub secondary: bool,
    pub shortcut: Option<String>,
    pub keep_open: bool,
    pub checked: Option<bool>,
    pub groups: Option<Vec<Group>>,
}
impl Item {
    pub fn action(value: impl Into<String>, title: impl Into<String>) -> Self {
        let title = title.into();
        Self {
            value: value.into(),
            search_terms: vec![title.clone()],
            title,
            search_title: None,
            styled_title: false,
            description: None,
            thread_search: None,
            thread: None,
            project: None,
            match_hint: None,
            disabled: false,
            secondary: false,
            shortcut: None,
            keep_open: false,
            checked: None,
            groups: None,
        }
    }
}
#[derive(Debug, Clone)]
pub struct Group {
    pub value: String,
    pub label: String,
    pub items: Vec<Item>,
}
#[derive(Debug, Clone)]
pub struct View {
    pub groups: Vec<Group>,
    pub initial_query: Option<String>,
}
#[derive(Debug, Clone, Default)]
pub struct SearchState {
    pub query: String,
    pub views: Vec<(View, String)>,
}
pub enum SearchAction {
    Query(String),
    Push(View),
    Back,
    Reset(Option<String>),
}
impl SearchState {
    pub fn apply(&mut self, action: SearchAction) {
        match action {
            SearchAction::Query(q) => self.query = q,
            SearchAction::Push(view) => {
                let query = view.initial_query.clone().unwrap_or_default();
                self.views
                    .push((view, std::mem::replace(&mut self.query, query)));
            }
            SearchAction::Back => {
                if let Some((_, query)) = self.views.pop() {
                    self.query = query;
                }
            }
            SearchAction::Reset(query) => {
                self.query = query.unwrap_or_default();
                self.views.clear();
            }
        }
    }
}
pub enum Row<'a> {
    Label {
        key: String,
        label: &'a str,
        first: bool,
    },
    Item {
        key: String,
        item: &'a Item,
        index: Option<usize>,
    },
}
pub struct Rows<'a> {
    pub rows: Vec<Row<'a>>,
    pub item_values: Vec<&'a str>,
    pub row_index_by_item_index: Vec<usize>,
}
pub fn build_rows(groups: &[Group]) -> Rows<'_> {
    let mut result = Rows {
        rows: vec![],
        item_values: vec![],
        row_index_by_item_index: vec![],
    };
    for group in groups {
        if !group.label.is_empty() {
            result.rows.push(Row::Label {
                key: format!("group:{}", group.value),
                label: &group.label,
                first: result.rows.is_empty(),
            });
        }
        for item in &group.items {
            let index = if item.disabled {
                None
            } else {
                let index = result.item_values.len();
                result.item_values.push(&item.value);
                result.row_index_by_item_index.push(result.rows.len());
                Some(index)
            };
            result.rows.push(Row::Item {
                key: format!("{}:{}", group.value, item.value),
                item,
                index,
            });
        }
    }
    result
}
pub fn highlighted<'a>(groups: &'a [Group], value: Option<&str>) -> Option<&'a Item> {
    groups
        .iter()
        .flat_map(|g| &g.items)
        .find(|i| !i.disabled && Some(i.value.as_str()) == value)
}
pub fn enumerate(items: &mut [Item]) {
    for (i, item) in items.iter_mut().enumerate() {
        item.shortcut = super::keybindings::thread_jump_command_for_index(i);
    }
}
pub fn linked_thread_items(
    environment: &EnvironmentId,
    threads: &[(&str, String, bool)],
    query: &str,
) -> Vec<Item> {
    threads
        .iter()
        .map(|(id, title, archived)| {
            let mut item = Item::action(
                format!("thread:{}:{id}", environment.0),
                if title.is_empty() {
                    "Untitled thread"
                } else {
                    title
                },
            );
            item.thread = Some(ThreadKey {
                environment: environment.clone(),
                thread: crate::types::ThreadId((*id).into()),
            });
            item.description = Some(
                if *archived {
                    "Archived thread"
                } else {
                    "Linked thread"
                }
                .into(),
            );
            item.search_terms = vec![query.into(), title.clone()];
            item
        })
        .collect()
}
pub fn project_items(
    projects: &[crate::workspace::projects::EnvironmentProject],
    prefix: &str,
    display: impl Fn(&crate::workspace::projects::EnvironmentProject) -> String,
    extra: impl Fn(&crate::workspace::projects::EnvironmentProject) -> Vec<String>,
) -> Vec<Item> {
    projects
        .iter()
        .map(|p| {
            let title = display(p);
            let mut item = Item::action(
                format!("{prefix}:{}:{}", p.environment.0, p.project.id.0),
                &title,
            );
            item.project = Some(p.key());
            item.description = Some(p.project.workspace_root.0.clone());
            item.search_terms = vec![
                title,
                p.project.title.0.clone(),
                p.project.workspace_root.0.clone(),
            ];
            item.search_terms.extend(extra(p));
            item
        })
        .collect()
}
/// Caller orders recents by last use and search results by rank. Subject retains server opens/picks.
pub fn thread_items(
    threads: &[sidebar::SidebarThread<'_>],
    active: Option<&ThreadKey>,
    projects: &BTreeMap<ProjectKey, String>,
    subject: impl Fn(&sidebar::SidebarThread<'_>, Option<&str>) -> ThreadSubject,
) -> Vec<Item> {
    threads
        .iter()
        .filter(|t| t.shell.archived_at.is_none() && !sidebar::is_hidden_thread(&t.shell.lineage))
        .map(|t| {
            let s = t.shell;
            let project = projects.get(&t.project);
            let mut description = vec![];
            if let Some(p) = project.filter(|p| !p.is_empty()) {
                description.push(p.clone());
            }
            if let Some(branch) = &s.branch {
                description.push(format!("#{}", branch.0));
            }
            let current = active == Some(&t.key);
            if current {
                description.push("Current thread".into());
            }
            let mut item = Item::action(
                format!("thread:{}:{}", t.key.environment.0, t.key.thread.0),
                &s.title,
            );
            item.thread = Some(t.key.clone());
            item.description = Some(description.join(" · "));
            item.thread_search = Some(subject(t, project.map(String::as_str)));
            item.secondary = current;
            item
        })
        .collect()
}
pub struct FilterInput<'a> {
    pub active: &'a [Group],
    pub query: &'a str,
    pub submenu: bool,
    pub root: Option<&'a [Group]>,
    pub nested: &'a [Item],
    pub projects: &'a [Item],
    pub settings: &'a [Item],
    pub threads: &'a [Item],
    pub messages: &'a [Item],
    pub now: f64,
}
fn score(item: &Item, query: &str, now: f64, typos: bool) -> Option<SearchMatch> {
    let mut m = if let Some(thread) = &item.thread_search {
        search::score_thread(query, thread, now, typos, true)
    } else {
        let name = if item.styled_title {
            item.search_title.as_deref()
        } else {
            Some(item.title.as_str())
        };
        let mut terms = item.search_terms.clone();
        if let Some(name) = name
            && terms.first().map(String::as_str) != Some(name)
        {
            terms.insert(0, name.into());
        }
        let mut m = search::score_terms(query, &terms, typos)?;
        if item.styled_title || terms.first() != Some(&item.title) {
            m.ranges.clear();
        }
        Some(m)
    }?;
    if item.secondary {
        m.group_quality = Some(0.0);
    }
    Some(m)
}
pub fn filter_groups(input: &FilterInput<'_>) -> Vec<Group> {
    if input.submenu
        && let Some(root) = input.root
    {
        let scoped = filter_groups(&FilterInput {
            root: None,
            ..*input
        });
        if !scoped.is_empty() || input.query.js_trim().is_empty() {
            return scoped;
        }
        return filter_groups(&FilterInput {
            active: root,
            submenu: false,
            ..*input
        });
    }
    let actions = input.query.starts_with('>');
    let query = search::fold_query(if actions {
        &input.query[1..]
    } else {
        input.query
    });
    if query.is_empty() {
        return input
            .active
            .iter()
            .filter(|g| !actions || g.value == "actions")
            .cloned()
            .collect();
    }
    let all = !input.submenu && !actions;
    let mut groups = input
        .active
        .iter()
        .filter(|g| {
            if actions {
                g.value == "actions"
            } else {
                input.submenu || g.value != "recent-threads"
            }
        })
        .cloned()
        .collect::<Vec<_>>();
    if all {
        for (value, label, items) in [
            ("nested-search", "Suggestions", input.nested),
            ("threads-search", "Threads", input.threads),
            ("projects-search", "Projects", input.projects),
            ("settings-search", "Settings", input.settings),
        ] {
            if !items.is_empty() {
                groups.push(Group {
                    value: value.into(),
                    label: label.into(),
                    items: items.to_vec(),
                });
            }
        }
    }
    let rank = |typos| {
        groups
            .iter()
            .enumerate()
            .map(|(index, g)| {
                let mut ranked = g
                    .items
                    .iter()
                    .enumerate()
                    .filter_map(|(i, item)| {
                        score(item, &query, input.now, typos).map(|m| (i, item, m))
                    })
                    .collect::<Vec<_>>();
                ranked.sort_by(|(ai, _, a), (bi, _, b)| {
                    search::compare_matches(a, b).then(ai.cmp(bi))
                });
                (index, g, ranked)
            })
            .collect::<Vec<_>>()
    };
    let mut ranked = rank(false);
    if !ranked
        .iter()
        .any(|(_, _, rows)| rows.iter().any(|(_, _, m)| m.strong))
    {
        ranked = rank(true);
    }
    ranked.retain(|(_, _, rows)| !rows.is_empty());
    let best = |rows: &[(usize, &Item, SearchMatch)]| {
        rows.iter()
            .map(|(_, _, m)| search::match_class(m.group_quality.unwrap_or(m.quality)))
            .max()
            .unwrap_or(0)
    };
    ranked.sort_by(|(ai, _, a), (bi, _, b)| best(b).cmp(&best(a)).then(ai.cmp(bi)));
    let threads_match = ranked.iter().any(|(_, g, _)| g.value == "threads-search");
    let mut result = ranked
        .into_iter()
        .map(|(_, g, mut rows)| {
            rows.sort_by_key(|(_, item, _)| item.secondary);
            let limit = if g.value == "threads-search" {
                THREAD_LIMIT
            } else if g.value == "nested-search" || (all && threads_match) {
                SECONDARY_LIMIT
            } else {
                rows.len()
            };
            let items = rows
                .into_iter()
                .take(limit)
                .map(|(_, item, m)| {
                    let mut item = item.clone();
                    if m.ranges.is_empty() && m.hint.is_some() {
                        item.match_hint = m.hint;
                    }
                    item
                })
                .collect();
            Group {
                value: g.value.clone(),
                label: g.label.clone(),
                items,
            }
        })
        .collect::<Vec<_>>();
    if all {
        let listed = result
            .iter()
            .flat_map(|g| g.items.iter().map(|i| i.value.as_str()))
            .collect::<BTreeSet<_>>();
        let items = input
            .messages
            .iter()
            .filter(|i| !listed.contains(i.value.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !items.is_empty() {
            result.push(Group {
                value: "threads-messages".into(),
                label: "In messages".into(),
                items,
            });
        }
    }
    result
}
pub fn nested_items(items: &[Item], query: &str) -> Vec<Item> {
    let names = items
        .iter()
        .map(|i| {
            if i.styled_title {
                i.search_title.as_deref()
            } else {
                Some(i.title.as_str())
            }
        })
        .collect::<BTreeSet<_>>();
    let query = search::fold_query(query);
    let mut result = vec![];
    for parent in items {
        let Some(groups) = &parent.groups else {
            continue;
        };
        if parent.styled_title {
            continue;
        }
        let stem = parent
            .title
            .strip_suffix("...")
            .or_else(|| parent.title.strip_suffix('…'))
            .unwrap_or(&parent.title);
        if search::fold_query(stem).starts_with(&query) {
            continue;
        }
        let prefix = if stem == parent.title {
            format!("{stem}: ")
        } else {
            format!("{} ", stem.js_trim_end())
        };
        for child in groups.iter().flat_map(|g| &g.items) {
            if child.groups.is_some() {
                continue;
            }
            let Some(name) = (if child.styled_title {
                child.search_title.as_deref()
            } else {
                Some(child.title.as_str())
            }) else {
                continue;
            };
            let title = format!("{prefix}{name}");
            if names.contains(&Some(title.as_str())) {
                continue;
            }
            let mut item = child.clone();
            item.title = title.clone();
            item.styled_title = false;
            item.search_title = None;
            item.search_terms = vec![title];
            result.push(item);
        }
    }
    result
}
pub fn root_groups(actions: Vec<Item>, recent: Vec<Item>) -> Vec<Group> {
    [
        ("actions", "Actions", actions),
        ("recent-threads", "Recent Threads", recent),
    ]
    .into_iter()
    .filter(|(_, _, items)| !items.is_empty())
    .map(|(value, label, items)| Group {
        value: value.into(),
        label: label.into(),
        items,
    })
    .collect()
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddProjectEntry {
    ConnectEnvironment,
    ChooseEnvironment,
    ChooseSource(EnvironmentId),
}
pub fn add_project_entry(count: usize, default: Option<&EnvironmentId>) -> AddProjectEntry {
    if count == 0 {
        AddProjectEntry::ConnectEnvironment
    } else if count > 1 || default.is_none() {
        AddProjectEntry::ChooseEnvironment
    } else {
        default
            .cloned()
            .map(AddProjectEntry::ChooseSource)
            .unwrap_or(AddProjectEntry::ChooseEnvironment)
    }
}
pub fn overlay_mode(command: Option<&str>) -> Option<OverlayMode> {
    match command {
        Some("commandPalette.toggle") => Some(OverlayMode::Command),
        Some("filePicker.toggle") => Some(OverlayMode::Files),
        Some("projectSearch.toggle") => Some(OverlayMode::Content),
        _ => None,
    }
}
pub fn no_project_index(entries: &[(bool, bool, bool)]) -> usize {
    if let Some(at) = entries.iter().position(|(scratch, _, _)| *scratch)
        && (entries[at].1 || entries[at].2)
    {
        return entries[..at]
            .iter()
            .filter(|(scratch, _, _)| !*scratch)
            .count();
    }
    let real = entries
        .iter()
        .filter(|(scratch, _, _)| !*scratch)
        .collect::<Vec<_>>();
    (real.iter().filter(|(_, pinned, _)| *pinned).count() + 1).min(real.len())
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Root,
    RootBrowse,
    Submenu,
    SubmenuBrowse,
}
pub fn mode(submenu: bool, browsing: bool) -> Mode {
    match (submenu, browsing) {
        (false, false) => Mode::Root,
        (false, true) => Mode::RootBrowse,
        (true, false) => Mode::Submenu,
        (true, true) => Mode::SubmenuBrowse,
    }
}
pub fn placeholder(mode: Mode) -> &'static str {
    match mode {
        Mode::Root => "Search commands, projects, and threads...",
        Mode::RootBrowse => "Enter project path (e.g. ~/projects/my-app)",
        Mode::Submenu => "Search...",
        Mode::SubmenuBrowse => "Enter path (e.g. ~/projects/my-app)",
    }
}
pub fn browse_platform<'a>(os: Option<&str>, host: &'a str) -> &'a str {
    match os {
        Some("windows") => "Win32",
        Some("darwin") => "MacIntel",
        Some("linux") => "Linux",
        _ => host,
    }
}
pub fn browse_groups(
    entries: &[t3_contracts::filesystem::FilesystemBrowseEntry],
    query: &str,
    up: bool,
) -> Vec<Group> {
    let mut items = vec![];
    if up {
        let mut item = Item::action("browse:up", "..");
        item.search_terms = vec![query.into(), "..".into()];
        item.keep_open = true;
        items.push(item);
    }
    for e in entries {
        let mut item = Item::action(format!("browse:{}", e.full_path.0), &e.name.0);
        item.search_terms = vec![query.into(), e.full_path.0.clone(), e.name.0.clone()];
        item.keep_open = true;
        items.push(item);
    }
    vec![Group {
        value: "directories".into(),
        label: "Directories".into(),
        items,
    }]
}
pub fn project_metadata(
    projects: &[crate::workspace::projects::EnvironmentProject],
    labels: &BTreeMap<EnvironmentId, String>,
) -> (Vec<String>, Vec<String>) {
    let mut terms = vec![];
    let mut seen = BTreeSet::new();
    let mut environments = vec![];
    for p in projects {
        let label = labels.get(&p.environment).map_or("Remote", String::as_str);
        terms.extend([
            p.project.title.0.clone(),
            p.project.workspace_root.0.clone(),
            label.into(),
        ]);
        if seen.insert(label) {
            environments.push(label.into());
        }
    }
    (terms, environments)
}
pub fn resolve_project<'a, T>(
    projects: &'a BTreeMap<ProjectKey, T>,
    key: Option<&ProjectKey>,
) -> Option<&'a T> {
    projects.get(key?)
}
pub fn remote_placeholder(
    repository_step: bool,
    source: &str,
    label: &str,
    path_hint: &str,
) -> Option<String> {
    if !repository_step {
        None
    } else if source == "url" {
        Some("Enter Git clone URL".into())
    } else {
        Some(format!("Enter {label} repository ({path_hint})"))
    }
}
pub fn remote_readiness(
    discovery: Option<&t3_contracts::source_control::SourceControlDiscoveryResult>,
    source: &str,
) -> crate::operations::RemoteSourceReadiness {
    let mut result = crate::operations::add_project_remote_source_readiness(discovery, source);
    result.hint = result.hint.map(|s| {
        s.replacen(
            "Open source control settings",
            "Open Settings -> Source control",
            1,
        )
    });
    result
}
pub fn error_message(message: Option<&str>) -> &str {
    message
        .filter(|s| !s.js_trim().is_empty())
        .unwrap_or("An error occurred.")
}
pub fn filter_pinned_browse<'a>(
    entries: &'a [t3_contracts::filesystem::FilesystemBrowseEntry],
    query: &str,
    pinned: &str,
    case_sensitive: bool,
) -> (
    Vec<&'a t3_contracts::filesystem::FilesystemBrowseEntry>,
    Option<&'a t3_contracts::filesystem::FilesystemBrowseEntry>,
) {
    let same = |a: &str, b: &str| {
        if case_sensitive {
            a == b
        } else {
            a.to_lowercase() == b.to_lowercase()
        }
    };
    let visible = if same(query, pinned) { "" } else { query };
    let rows = entries
        .iter()
        .filter(|e| {
            e.name.0.to_lowercase().starts_with(&visible.to_lowercase())
                && (visible.starts_with('.') || !e.name.0.starts_with('.'))
        })
        .collect();
    let exact = (!query.is_empty())
        .then(|| entries.iter().find(|e| same(&e.name.0, query)))
        .flatten();
    (rows, exact)
}
