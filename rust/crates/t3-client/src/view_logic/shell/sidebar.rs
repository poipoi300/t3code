//! Shared sidebar policy for web, desktop and mobile.
//!
//! Start with `workspace::sidebar::build_sidebar_snapshots` for logical project groups,
//! then [`presentation::present_sidebar`] for filtered, ordered shelves and folders.
//! Thread identity is always `(environment, thread)`; a route to a hidden child resolves
//! to its nearest listed ancestor only inside that environment. Archived/deleted rows,
//! side chats, subagents and tasks delegated under a parent have no independent row.
//!
//! Presentation takes canonical shelf/status inputs from thread views, parsed timestamps
//! via an injected parser, and an explicit clock. Server visits (including explicit null
//! for mark-unread) override legacy local visits. Completion is unread only after a
//! prior visit; failures on unopened threads remain unseen. Approval/input/guest requests
//! precede running state; folder dots use attention > failure > unread > working.
//!
//! Pinned keys precede keyless rows; Active keyless rows precede keys. Time-ordered inboxes
//! use return/completion times and Working uses the last authored send. Snoozed rows sort
//! by wake time; Settled by settlement/activity. Equal keys/times use thread then environment
//! identity. Project grouping is never reimplemented here; folder order follows the supplied
//! groups, with activity ordering available through `sort_logical_projects`.
//! Expand logical project scope keys to their member `ProjectKey`s before selecting rows;
//! `Options.scope = None` means all projects (an empty persisted scope also means all).
//! Resolve a hidden-child route with `resolve_row_thread_key` before passing `Options.route`.
//!
//! Geometry, locale/calendar conversion, subscriptions, timers and DOM focus/listeners
//! belong to the UI/host. The host schedules only the next snooze/hold deadline. Drop plans
//! are decisions, not RPC execution: recheck destination permission/connection before writing.
use crate::{
    thread_views::{
        shelves::{ShelfInput, effective_snoozed},
        threads::sort::plan_pinned_reorder,
    },
    types::{EnvironmentId, ThreadKey},
    workspace::{ProjectKey, sidebar::SidebarProjectSnapshot},
};
use futures::future::LocalBoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Section {
    Pinned,
    Active,
    Working,
    Snoozed,
    Settled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    PinnedHeader,
    ActivePlaceholder,
    SettledPlaceholder,
    PinnedDivider,
    WorkingHeader,
    SnoozedHeader,
    SettledHeader,
}
impl Marker {
    pub fn name(self) -> &'static str {
        match self {
            Self::PinnedHeader => "pinned-header",
            Self::ActivePlaceholder => "active-placeholder",
            Self::SettledPlaceholder => "settled-placeholder",
            Self::PinnedDivider => "pinned-divider",
            Self::WorkingHeader => "working-header",
            Self::SnoozedHeader => "snoozed-header",
            Self::SettledHeader => "settled-header",
        }
    }
    pub fn id(self) -> String {
        format!("sidebar-marker-{}", self.name())
    }
    pub fn shelf(self) -> bool {
        matches!(
            self,
            Self::WorkingHeader | Self::SnoozedHeader | Self::SettledHeader
        )
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListItem {
    Thread { key: String, section: Section },
    Marker(Marker),
}
impl ListItem {
    pub fn id(&self) -> String {
        match self {
            Self::Thread { key, .. } => key.clone(),
            Self::Marker(m) => m.id(),
        }
    }
    pub fn section(&self) -> Option<Section> {
        match self {
            Self::Thread { section, .. } => Some(*section),
            _ => None,
        }
    }
}
pub fn resolve_thread_section(snoozed: bool, settled: bool, pinned: bool) -> Section {
    if snoozed {
        Section::Snoozed
    } else if settled {
        Section::Settled
    } else if pinned {
        Section::Pinned
    } else {
        Section::Active
    }
}
fn section_at_slot(items: &[ListItem], index: usize) -> Section {
    let mut section = Section::Pinned;
    for item in items.iter().take(index) {
        if let ListItem::Marker(marker) = item {
            section = match marker {
                Marker::PinnedDivider => Section::Active,
                Marker::WorkingHeader => Section::Working,
                Marker::SnoozedHeader => Section::Snoozed,
                Marker::SettledHeader => Section::Settled,
                _ => section,
            };
        }
    }
    section
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropTarget {
    pub section: Section,
    pub pinned_order: Vec<String>,
    pub active_order: Vec<String>,
}
pub fn resolve_drop_target(items: &[ListItem], active: &str, over: &str) -> Option<DropTarget> {
    let active_index = items.iter().position(|i| i.id() == active)?;
    let over_index = items.iter().position(|i| i.id() == over)?;
    if !matches!(items[active_index], ListItem::Thread { .. }) {
        return None;
    }
    let mut moved = items.to_vec();
    let item = moved.remove(active_index);
    moved.insert(over_index, item);
    let section = section_at_slot(&moved, over_index);
    if matches!(section, Section::Working | Section::Snoozed) {
        return None;
    }
    let mut target = DropTarget {
        section,
        pinned_order: vec![],
        active_order: vec![],
    };
    let mut pinned = true;
    for item in moved {
        match item {
            ListItem::Marker(Marker::PinnedDivider) => pinned = false,
            ListItem::Marker(m) if m.shelf() => break,
            ListItem::Thread { key, .. } => {
                if pinned {
                    target.pinned_order.push(key)
                } else {
                    target.active_order.push(key)
                }
            }
            _ => {}
        }
    }
    Some(target)
}
pub fn resolve_drop_verb(from: Section, to: Option<Section>) -> Option<&'static str> {
    let to = to?;
    if from == to || matches!(to, Section::Working | Section::Snoozed) {
        return None;
    }
    Some(if to == Section::Pinned {
        "pin"
    } else if to == Section::Settled {
        "settle"
    } else if from == Section::Pinned {
        "unpin"
    } else if from == Section::Settled {
        "unsettle"
    } else {
        "wake"
    })
}
pub fn sweep_keys(
    ordered: &[String],
    origin: &str,
    target: &str,
    can_apply: impl Fn(&str) -> bool,
) -> Vec<String> {
    let (Some(a), Some(b)) = (
        ordered.iter().position(|k| k == origin),
        ordered.iter().position(|k| k == target),
    ) else {
        return vec![];
    };
    ordered[a.min(b)..=a.max(b)]
        .iter()
        .filter(|k| can_apply(k))
        .cloned()
        .collect()
}
pub fn thread_key_at_y<'a>(
    rows: &[(&'a str, f64)],
    y: f64,
    viewport: Option<(f64, f64)>,
) -> Option<&'a str> {
    let y = viewport.map_or(y, |(top, bottom)| y.max(top).min(bottom - 1.0));
    let mut key = None;
    for &(row, top) in rows {
        if key.is_some() && top > y {
            break;
        }
        key = Some(row);
    }
    key
}
pub fn reorderable_thread_keys(
    threads: &[ThreadKey],
    configured: &BTreeSet<EnvironmentId>,
    connected: &BTreeSet<EnvironmentId>,
) -> BTreeSet<ThreadKey> {
    threads
        .iter()
        .filter(|t| configured.contains(&t.environment) && connected.contains(&t.environment))
        .cloned()
        .collect()
}
pub type Assignment = (String, String);
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropPlan {
    None,
    ReorderPinned {
        order: Vec<String>,
        assignments: Vec<Assignment>,
    },
    Pin {
        order: Vec<String>,
        order_key: Option<String>,
        extra_assignments: Vec<Assignment>,
    },
    MoveActive {
        order: Option<Vec<String>>,
        assignments: Vec<Assignment>,
        unpin: bool,
        unsettle: bool,
        unsnooze: bool,
    },
    Settle,
}
pub struct DropInput<'a> {
    pub active_key: &'a str,
    pub active_section: Section,
    pub active_pinned: Option<bool>,
    pub active_settled: Option<bool>,
    pub supports_settlement: bool,
    pub target: &'a DropTarget,
    pub pinned_order: &'a [String],
    pub pinned_keys: &'a BTreeMap<String, Option<String>>,
    pub reorderable: Option<&'a BTreeSet<String>>,
    pub active_order: &'a [String],
    pub active_keys: &'a BTreeMap<String, Option<String>>,
    pub active_reorderable: Option<&'a BTreeSet<String>>,
    pub active_time_ordered: bool,
}
pub fn plan_thread_drop(input: DropInput<'_>) -> DropPlan {
    let from = input.active_section;
    let to = input.target.section;
    let pinned = input.active_pinned.unwrap_or(from == Section::Pinned);
    let settled = input.active_settled.unwrap_or(from == Section::Settled);
    if from == Section::Working
        || (!input.supports_settlement && (to == Section::Settled || settled))
    {
        return DropPlan::None;
    }
    let arrange = |order: &[String],
                   keys: &BTreeMap<String, Option<String>>,
                   writable: Option<&BTreeSet<String>>|
     -> Option<Vec<Assignment>> {
        let Some(writable) = writable else {
            return Some(plan_pinned_reorder(order, keys, input.active_key));
        };
        if !writable.contains(input.active_key) {
            return None;
        }
        let filtered = order
            .iter()
            .filter(|k| writable.contains(*k) || keys.get(*k).is_some_and(Option::is_some))
            .cloned()
            .collect::<Vec<_>>();
        let assignments = plan_pinned_reorder(&filtered, keys, input.active_key);
        if assignments.iter().any(|(id, _)| !writable.contains(id)) {
            return None;
        }
        let mut frozen_before = false;
        let mut assigned_before = false;
        for id in order {
            let frozen = !writable.contains(id) && keys.get(id).is_none_or(Option::is_none);
            let assigned = assignments.iter().any(|(key, _)| key == id);
            if (to == Section::Active && frozen && assigned_before)
                || (to == Section::Pinned && assigned && frozen_before)
            {
                return None;
            }
            frozen_before |= frozen;
            assigned_before |= assigned;
        }
        Some(assignments)
    };
    match to {
        Section::Active => {
            if input.active_time_ordered {
                return if from == Section::Active {
                    DropPlan::None
                } else {
                    DropPlan::MoveActive {
                        order: None,
                        assignments: vec![],
                        unpin: pinned,
                        unsettle: settled,
                        unsnooze: from == Section::Snoozed,
                    }
                };
            }
            let order = &input.target.active_order;
            if from == Section::Active && order == input.active_order {
                return DropPlan::None;
            }
            let Some(assignments) = arrange(order, input.active_keys, input.active_reorderable)
            else {
                return DropPlan::None;
            };
            DropPlan::MoveActive {
                order: Some(order.clone()),
                assignments,
                unpin: pinned,
                unsettle: settled,
                unsnooze: from == Section::Snoozed,
            }
        }
        Section::Settled => {
            if from == Section::Settled {
                DropPlan::None
            } else {
                DropPlan::Settle
            }
        }
        Section::Pinned => {
            let order = &input.target.pinned_order;
            if from == Section::Pinned && order == input.pinned_order {
                return DropPlan::None;
            }
            let Some(assignments) = arrange(order, input.pinned_keys, input.reorderable) else {
                return DropPlan::None;
            };
            if from == Section::Pinned {
                return if assignments.is_empty() {
                    DropPlan::None
                } else {
                    DropPlan::ReorderPinned {
                        order: order.clone(),
                        assignments,
                    }
                };
            }
            let order_key = assignments
                .iter()
                .find(|(id, _)| id == input.active_key)
                .map(|(_, key)| key.clone());
            DropPlan::Pin {
                order: order.clone(),
                order_key,
                extra_assignments: assignments
                    .into_iter()
                    .filter(|(id, _)| pinned || id != input.active_key)
                    .collect(),
            }
        }
        _ => DropPlan::None,
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lifecycle {
    pub pinned_at: Option<String>,
    pub pin_order_key: Option<String>,
    pub active_order_key: Option<String>,
    pub snoozed_at: Option<String>,
    pub snoozed_until: Option<String>,
    pub settled_at: Option<String>,
    pub settled_override: Option<String>,
    pub unsettled_at: Option<String>,
}
pub fn apply_thread_drop(
    mut thread: Lifecycle,
    section: Section,
    now: &str,
    order_key: Option<&str>,
) -> Lifecycle {
    let was_settled = thread.settled_override.as_deref() == Some("settled");
    thread.snoozed_at = None;
    thread.snoozed_until = None;
    if section == Section::Settled {
        thread.pinned_at = None;
        thread.pin_order_key = None;
        thread.active_order_key = None;
        thread.settled_override = Some("settled".into());
        thread.settled_at = Some(if was_settled {
            thread.settled_at.unwrap_or_else(|| now.into())
        } else {
            now.into()
        });
        thread.unsettled_at = None;
        return thread;
    }
    if was_settled {
        thread.settled_override = Some("active".into());
        thread.settled_at = None;
        thread.unsettled_at = Some(now.into());
    }
    if section == Section::Pinned {
        thread.pinned_at = Some(thread.pinned_at.unwrap_or_else(|| now.into()));
        if let Some(key) = order_key {
            thread.pin_order_key = Some(key.into());
        }
    } else {
        thread.pinned_at = None;
        thread.pin_order_key = None;
    }
    if section == Section::Active
        && let Some(key) = order_key
    {
        thread.active_order_key = Some(key.into());
    }
    thread
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadSelection {
    pub selected: Vec<String>,
    pub anchor: Option<String>,
}
impl ThreadSelection {
    pub fn toggle(&mut self, key: &str) {
        if let Some(i) = self.selected.iter().position(|k| k == key) {
            self.selected.remove(i);
        } else {
            self.selected.push(key.into());
            self.anchor = Some(key.into());
        }
    }
    pub fn range_select_to(&mut self, key: &str, ordered: &[String]) {
        let range = self
            .anchor
            .as_ref()
            .and_then(|a| ordered.iter().position(|k| k == a))
            .zip(ordered.iter().position(|k| k == key));
        if let Some((a, b)) = range {
            for key in &ordered[a.min(b)..=a.max(b)] {
                if !self.selected.contains(key) {
                    self.selected.push(key.clone());
                }
            }
        } else {
            if !self.selected.iter().any(|k| k == key) {
                self.selected.push(key.into());
            }
            self.anchor = Some(key.into());
        }
    }
    pub fn clear(&mut self) {
        self.selected.clear();
        self.anchor = None;
    }
    pub fn set_anchor(&mut self, key: String) {
        self.anchor = Some(key);
    }
    pub fn remove(&mut self, keys: &[String]) {
        let before = self.selected.len();
        self.selected.retain(|k| !keys.contains(k));
        if self.selected.len() != before && self.anchor.as_ref().is_some_and(|a| keys.contains(a)) {
            self.anchor = None;
        }
    }
}
pub fn keys_to_deselect_after_delete(
    selected: &[String],
    deleted: &BTreeSet<String>,
    has_thread: impl Fn(&str) -> bool,
) -> Vec<String> {
    selected
        .iter()
        .filter(|k| deleted.contains(*k) || !has_thread(k))
        .cloned()
        .collect()
}
pub enum DeleteResult<E> {
    Skipped,
    Success,
    Failure(E),
    Interrupted,
}
pub async fn delete_selected_entries<T, E>(
    entries: &[T],
    key: impl Fn(&T) -> String,
    delete: impl for<'a> Fn(&'a T, &'a BTreeSet<String>) -> LocalBoxFuture<'a, DeleteResult<E>>,
) -> (BTreeSet<String>, Option<E>) {
    let mut deleted = BTreeSet::new();
    let mut failure = None;
    for entry in entries {
        match delete(entry, &deleted).await {
            DeleteResult::Skipped => {}
            DeleteResult::Success => {
                deleted.insert(key(entry));
            }
            DeleteResult::Failure(error) => {
                if failure.is_none() {
                    failure = Some(error);
                }
            }
            DeleteResult::Interrupted => break,
        }
    }
    (deleted, failure)
}
pub fn should_navigate_after_park(
    key: &str,
    current: Option<&str>,
    settle: bool,
    settled: bool,
    thread: Option<&ShelfInput>,
    now: i64,
) -> bool {
    Some(key) == current
        && thread.is_some_and(|t| {
            if settle {
                settled
            } else {
                effective_snoozed(t, now)
            }
        })
}
pub fn should_create_new_thread_in_current_project(shift: bool, open: usize, total: usize) -> bool {
    shift || open == 1 || total == 0
}
pub fn row_accessibility(
    title: &str,
    status: Option<&str>,
    project: Option<&str>,
    active: bool,
) -> (String, Option<&'static str>) {
    (
        [Some(title), status, project]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        active.then_some("page"),
    )
}
pub fn lease_live_status(active: bool, near: bool, can_observe: bool) -> bool {
    active || near || !can_observe
}
pub const THREAD_JUMP_HINT_SHOW_DELAY_MS: u64 = 200;
/// Restart only when modifiers change, not when focus or picker visibility changes.
#[derive(Debug, Clone, Default)]
pub struct ModifierHold {
    modifiers: Option<super::keybindings::Modifiers>,
    deadline: Option<u64>,
}
impl ModifierHold {
    pub fn observe(&mut self, modifiers: super::keybindings::Modifiers, now: u64) {
        if self.modifiers != Some(modifiers) {
            self.modifiers = Some(modifiers);
            self.deadline = Some(now.saturating_add(THREAD_JUMP_HINT_SHOW_DELAY_MS));
        }
    }
    pub fn deadline(&self) -> Option<u64> {
        self.deadline
    }
    pub fn visible(
        &self,
        now: u64,
        bindings: &[t3_contracts::keybindings::ResolvedKeybindingRule],
        platform: &str,
        context: &super::keybindings::MatchContext,
    ) -> bool {
        self.deadline.is_some_and(|due| now >= due)
            && self.modifiers.is_some_and(|m| {
                super::keybindings::should_show_thread_jump_hints(m, bindings, platform, context)
            })
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}
pub fn is_trailing_double_click(detail: f64) -> bool {
    detail > 1.0
}
/// The host resolves anchor ancestry (including a text node's parent).
pub fn should_navigate_row(detail: f64, nested_link: bool) -> bool {
    !is_trailing_double_click(detail) && !nested_link
}
pub fn without_removed_checkout<'a>(
    branch: Option<&'a str>,
    worktree: Option<&'a str>,
    is_repo: Option<bool>,
) -> (Option<&'a str>, Option<&'a str>) {
    if is_repo == Some(false) {
        (None, None)
    } else {
        (branch, worktree)
    }
}
pub fn next_snooze_wake(wakes: &[i64], clock: i64) -> Option<i64> {
    wakes.iter().copied().filter(|w| *w > clock).min()
}
pub fn advance_snooze_clock(wakes: &[i64], clock: i64, now: i64) -> i64 {
    wakes
        .iter()
        .copied()
        .filter(|w| *w > clock && *w <= now)
        .max()
        .unwrap_or(clock)
}
#[derive(Debug, Clone, Default)]
pub struct RetainedValue<T> {
    retained: Option<(String, T)>,
}
impl<T: Clone> RetainedValue<T> {
    pub fn resolve(&mut self, key: Option<&str>, value: Option<T>) -> Option<T> {
        if let (Some(key), Some(value)) = (key, value.as_ref()) {
            self.retained = Some((key.into(), value.clone()));
        }
        value.or_else(|| {
            self.retained
                .as_ref()
                .filter(|(k, _)| Some(k.as_str()) == key)
                .map(|(_, v)| v.clone())
        })
    }
}
pub fn order_by_preferences<'a, T, Id: Ord + Clone>(
    items: &'a [T],
    preferred: &[Id],
    aliases: impl Fn(&T) -> Vec<Id>,
) -> Vec<&'a T> {
    let mut indices: BTreeMap<Id, Vec<usize>> = BTreeMap::new();
    for (i, item) in items.iter().enumerate() {
        for alias in aliases(item).into_iter().collect::<BTreeSet<_>>() {
            indices.entry(alias).or_default().push(i);
        }
    }
    let mut emitted = BTreeSet::new();
    let mut ordered = vec![];
    for id in preferred {
        if let Some(i) = indices
            .get(id)
            .and_then(|ix| ix.iter().find(|i| !emitted.contains(*i)))
        {
            emitted.insert(*i);
            ordered.push(&items[*i]);
        }
    }
    ordered.extend(
        items
            .iter()
            .enumerate()
            .filter(|(i, _)| !emitted.contains(i))
            .map(|(_, item)| item),
    );
    ordered
}
pub fn adjacent_thread<'a, T: PartialEq>(
    threads: &'a [T],
    current: Option<&T>,
    previous: bool,
) -> Option<&'a T> {
    let Some(current) = current else {
        return if previous {
            threads.last()
        } else {
            threads.first()
        };
    };
    let i = threads.iter().position(|t| t == current)?;
    if previous {
        threads.get(i.checked_sub(1)?)
    } else {
        threads.get(i + 1)
    }
}
pub fn group_threads_by_project<'a, 'b, T>(
    groups: &'a [SidebarProjectSnapshot],
    threads: &'b [T],
    project: impl Fn(&T) -> ProjectKey,
    keep_empty: impl Fn(&SidebarProjectSnapshot) -> bool,
) -> Vec<(&'a SidebarProjectSnapshot, Vec<&'b T>)> {
    let mut index = BTreeMap::new();
    for (i, g) in groups.iter().enumerate() {
        for key in &g.group.member_project_refs {
            index.insert(key.clone(), i);
        }
    }
    let mut folders = groups.iter().map(|g| (g, vec![])).collect::<Vec<_>>();
    for thread in threads {
        if let Some(&i) = index.get(&project(thread)) {
            folders[i].1.push(thread);
        }
    }
    folders
        .into_iter()
        .filter(|(g, rows)| !rows.is_empty() || keep_empty(g))
        .collect()
}

#[path = "sidebar_gestures.rs"]
pub mod gestures;
#[path = "presentation.rs"]
pub mod presentation;
#[path = "sidebar_snooze.rs"]
pub mod snooze;
#[path = "sidebar_status.rs"]
pub mod status;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteReveal {
    pub observed: Option<ThreadKey>,
    pub pending: Option<ThreadKey>,
}
impl RouteReveal {
    pub fn pending(&mut self, route: Option<&ThreadKey>) -> Option<ThreadKey> {
        let route = route?;
        if self.observed.as_ref() != Some(route) {
            self.observed = Some(route.clone());
            self.pending = Some(route.clone());
        }
        self.pending.clone()
    }
    pub fn complete(&mut self) {
        self.pending = None;
    }
    pub fn resolve(
        &mut self,
        route: Option<&ThreadKey>,
        searching: bool,
        scoped: bool,
        visible_in_scope: impl Fn(&ThreadKey) -> bool,
        visible_unscoped: impl Fn(&ThreadKey) -> bool,
    ) -> (Option<ThreadKey>, bool) {
        let pending = self.pending(route);
        let widen = !searching
            && scoped
            && pending
                .as_ref()
                .is_some_and(|key| !visible_in_scope(key) && visible_unscoped(key));
        (pending, widen)
    }
}
pub fn is_hidden_thread(
    lineage: &t3_contracts::orchestration_v2::OrchestrationV2AppThreadLineage,
) -> bool {
    use t3_contracts::orchestration_v2::AppThreadLineageRelationshipToParent;
    lineage.relationship_to_parent == Some(AppThreadLineageRelationshipToParent::Subagent)
        || lineage.side_chat == Some(Some(true))
}
pub struct SidebarThread<'a> {
    pub key: ThreadKey,
    pub project: ProjectKey,
    pub shell: &'a t3_contracts::orchestration_v2::OrchestrationV2ThreadShell,
}
pub fn resolve_row_thread_key(thread: &ThreadKey, threads: &[SidebarThread<'_>]) -> ThreadKey {
    let mut visited = BTreeSet::new();
    let mut current = thread.clone();
    while let Some(row) = threads.iter().find(|t| t.key == current) {
        if !is_hidden_thread(&row.shell.lineage) {
            return current;
        }
        let Some(parent) = row.shell.lineage.parent_thread_id.as_ref() else {
            return thread.clone();
        };
        if !visited.insert(current.thread.clone()) {
            return thread.clone();
        }
        current.thread = parent.clone();
    }
    thread.clone()
}
pub fn visible_threads<'a>(
    threads: &'a [SidebarThread<'a>],
    scope: Option<&BTreeSet<ProjectKey>>,
    delegated: &BTreeSet<ThreadKey>,
) -> Vec<&'a SidebarThread<'a>> {
    threads
        .iter()
        .filter(|t| {
            t.shell.archived_at.is_none()
                && !is_hidden_thread(&t.shell.lineage)
                && !delegated.contains(&t.key)
                && scope.is_none_or(|s| s.contains(&t.project))
        })
        .collect()
}
pub fn folder_expansion_keys(group_key: &str, members: &[(String, String)]) -> Vec<String> {
    std::iter::once(group_key.into())
        .chain(members.iter().map(|(key, _)| key.clone()))
        .chain(members.iter().map(|(_, cwd)| {
            format!(
                "legacy-project-cwd:{}",
                crate::workspace::projects::normalize_project_path_for_comparison(cwd)
            )
        }))
        .collect()
}
pub fn resolve_project_expanded(preferences: &BTreeMap<String, bool>, aliases: &[String]) -> bool {
    aliases
        .iter()
        .find_map(|key| preferences.get(key))
        .copied()
        .or_else(|| preferences.get("legacy-project-expansion-default").copied())
        .unwrap_or(true)
}
pub fn first_valid_timestamp(
    candidates: &[Option<&str>],
    parse: &impl Fn(&str) -> Option<i64>,
) -> i64 {
    candidates
        .iter()
        .flatten()
        .find_map(|s| parse(s))
        .unwrap_or(0)
}
pub fn resolve_working_started_at<'a>(
    run: Option<&'a crate::view_logic::session::RunView>,
    runtime: Option<&'a crate::view_logic::session::RuntimeView>,
    valid: impl Fn(&str) -> bool,
) -> Option<&'a str> {
    crate::view_logic::session::derive_active_work_started_at(run, runtime, None, valid)
}
pub fn fallback_thread_after_delete<'a>(
    threads: &'a [SidebarThread<'_>],
    deleted: &ThreadKey,
    deleting: &BTreeSet<ThreadKey>,
    snoozed: impl Fn(&SidebarThread<'_>) -> bool,
    timestamp: impl Fn(&SidebarThread<'_>) -> Option<i64>,
) -> Option<&'a ThreadKey> {
    let source = threads.iter().find(|t| &t.key == deleted)?;
    threads
        .iter()
        .filter(|t| {
            t.project == source.project
                && &t.key != deleted
                && !deleting.contains(&t.key)
                && t.shell.settled_override
                    != Some(t3_contracts::orchestration_v2::AppThreadSettledOverride::Settled)
                && !snoozed(t)
                && !is_hidden_thread(&t.shell.lineage)
        })
        .max_by(|a, b| {
            timestamp(a)
                .cmp(&timestamp(b))
                .then(a.key.thread.cmp(&b.key.thread))
        })
        .map(|t| &t.key)
}
pub fn filter_project_scope_items<'a, T>(
    items: &'a [T],
    query: &str,
    value: impl Fn(&T) -> &str,
    matches: impl Fn(&T, &str) -> bool,
) -> Vec<&'a T> {
    use crate::view_logic::JsTrim;
    let query = query.js_trim();
    items
        .iter()
        .filter(|item| query.is_empty() || (value(item) != "all" && matches(item, query)))
        .collect()
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopeMenuState {
    pub open: bool,
    pub query: String,
}
pub enum ScopeMenuAction {
    Query(String),
    Open(bool),
    ProjectSettingsOpened,
}
impl ScopeMenuState {
    pub fn apply(&mut self, action: ScopeMenuAction) {
        match action {
            ScopeMenuAction::Query(query) => self.query = query,
            ScopeMenuAction::Open(open) => {
                self.open = open;
                self.query.clear();
            }
            ScopeMenuAction::ProjectSettingsOpened => {
                self.open = false;
                self.query.clear();
            }
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkMenuItem {
    pub id: &'static str,
    pub label: String,
    pub disabled: bool,
}
pub fn bulk_title_regeneration_item(supported: usize, actionable: usize) -> Option<BulkMenuItem> {
    (supported > 0).then(|| BulkMenuItem {
        id: "regenerate-title",
        label: if actionable == 0 {
            format!("Regenerating… ({supported})")
        } else {
            format!("Regenerate titles ({actionable})")
        },
        disabled: actionable == 0,
    })
}
pub fn bulk_unpin_item(pinned: usize) -> Option<BulkMenuItem> {
    (pinned > 0).then(|| BulkMenuItem {
        id: "unpin",
        label: format!("Unpin ({pinned})"),
        disabled: false,
    })
}

pub struct ProjectActivity<'a> {
    pub group: &'a SidebarProjectSnapshot,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub thread_timestamps: Vec<Option<i64>>,
}
pub fn project_activity<'a>(
    groups: &'a [SidebarProjectSnapshot],
    threads: &[SidebarThread<'_>],
    visible_only: bool,
    delegated: &BTreeSet<ThreadKey>,
    parse: &impl Fn(&str) -> Option<i64>,
    thread_timestamp: impl Fn(&SidebarThread<'_>) -> Option<i64>,
) -> Vec<ProjectActivity<'a>> {
    let mut by_project: BTreeMap<&ProjectKey, Vec<Option<i64>>> = BTreeMap::new();
    for thread in threads.iter().filter(|t| {
        t.shell.archived_at.is_none()
            && (!visible_only
                || (!is_hidden_thread(&t.shell.lineage) && !delegated.contains(&t.key)))
    }) {
        by_project
            .entry(&thread.project)
            .or_default()
            .push(thread_timestamp(thread));
    }
    groups
        .iter()
        .map(|group| {
            let representative = &group.group.representative.project;
            ProjectActivity {
                group,
                created_at: parse(&representative.created_at.0),
                updated_at: parse(&representative.updated_at.0),
                thread_timestamps: group
                    .group
                    .member_project_refs
                    .iter()
                    .filter_map(|key| by_project.get(key))
                    .flatten()
                    .copied()
                    .collect(),
            }
        })
        .collect()
}
/// The caller supplies the host's locale comparator, as JavaScript `localeCompare` is locale-dependent.
pub fn sort_logical_projects<'a>(
    projects: &'a [ProjectActivity<'a>],
    manual: bool,
    created: bool,
    compare: impl Fn(&SidebarProjectSnapshot, &SidebarProjectSnapshot) -> std::cmp::Ordering,
) -> Vec<&'a SidebarProjectSnapshot> {
    let stamp = |p: &ProjectActivity<'_>| {
        if !p.thread_timestamps.is_empty() {
            p.thread_timestamps.iter().copied().flatten().max()
        } else if created {
            p.created_at
        } else {
            p.updated_at.or(p.created_at)
        }
    };
    let mut rows = projects
        .iter()
        .map(|p| (p.group, stamp(p)))
        .collect::<Vec<_>>();
    if !manual {
        rows.sort_by(|(a, at), (b, bt)| bt.cmp(at).then_with(|| compare(a, b)));
    }
    rows.into_iter().map(|(g, _)| g).collect()
}
pub struct SearchResult<'a, T> {
    pub title_matches: Vec<&'a T>,
    pub content_matches: Vec<&'a T>,
}
/// Ranking is shared with the palette; content keys retain server ranking and are never re-sorted.
pub fn search_threads<'a, T>(
    threads: &'a [T],
    content_keys: &[ThreadKey],
    key: impl Fn(&T) -> ThreadKey,
    query: &str,
    subject: impl Fn(&T) -> super::search::ThreadSubject,
    now: f64,
) -> SearchResult<'a, T> {
    let query = super::search::fold_query(query);
    if query.is_empty() {
        return SearchResult {
            title_matches: vec![],
            content_matches: vec![],
        };
    }
    let subjects = threads
        .iter()
        .map(|thread| (thread, subject(thread)))
        .collect::<Vec<_>>();
    let title = super::search::rank_items(&subjects, |(_, subject), typos| {
        super::search::score_thread(&query, subject, now, typos, false)
    });
    let listed = title
        .iter()
        .map(|((t, _), _)| key(t))
        .collect::<BTreeSet<_>>();
    let content = content_keys
        .iter()
        .filter(|k| !listed.contains(*k))
        .filter_map(|k| threads.iter().rev().find(|t| key(t) == *k))
        .collect();
    SearchResult {
        title_matches: title.into_iter().map(|((thread, _), _)| *thread).collect(),
        content_matches: content,
    }
}

/// Uses the canonical PR subjects from `threads::search::thread_search_subject`.
pub fn pull_request_matches_query(prs: &[super::search::SearchPullRequest], query: &str) -> bool {
    use unicode_general_category::{GeneralCategory as C, get_general_category};
    use unicode_normalization::UnicodeNormalization;
    let normalize = |s: &str| {
        s.nfkd()
            .filter(|c| {
                !matches!(
                    get_general_category(*c),
                    C::NonspacingMark | C::SpacingMark | C::EnclosingMark
                )
            })
            .collect::<String>()
            .to_lowercase()
            .split(crate::view_logic::is_js_whitespace)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let query = normalize(query);
    if query.is_empty() {
        return false;
    }
    let haystack = normalize(
        &prs.iter()
            .flat_map(|pr| {
                std::iter::once(format!("#{}", pr.number))
                    .chain(pr.references.clone())
                    .chain(pr.title.clone())
            })
            .collect::<Vec<_>>()
            .join(" "),
    );
    query.split(' ').all(|token| haystack.contains(token))
}
