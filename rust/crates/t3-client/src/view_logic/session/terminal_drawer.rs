//! The non-visual parts of the terminal drawer: ports of
//! `apps/web/src/components/ThreadTerminalDrawer.tsx` (groups, labels, height, output
//! synchronization), `PersistentThreadTerminals.tsx` (server reconciliation),
//! `terminal/useThreadTerminalActions.ts` (id allocation), `lib/terminalCloseConfirm.ts`, and
//! `packages/shared/src/{terminalLabels,projectScripts}.ts`.
use crate::{
    services::terminal::{
        TerminalOutputCursor, TerminalOutputState, TerminalOutputUpdate, TerminalStatus,
    },
    view_logic::{
        JsTrim,
        shell::terminal_state::{DEFAULT_HEIGHT, Group, MAX_PER_GROUP},
    },
};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

pub const MIN_DRAWER_HEIGHT: f64 = 180.0;
pub const MAX_DRAWER_HEIGHT_RATIO: f64 = 0.75;
/// Avoid formatting unbounded scrollback selections on every output update.
pub const MAX_COMPARED_SELECTION_CELLS: usize = 3_000;

/// JavaScript's `Math.round`: halves go up.
fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DrawerLayout {
    pub viewport_height: f64,
    pub chat_column_height: Option<f64>,
    pub header_height: Option<f64>,
    pub composer_height: Option<f64>,
}

pub fn resolve_terminal_drawer_max_height(layout: &DrawerLayout) -> f64 {
    let window_cap =
        MIN_DRAWER_HEIGHT.max((layout.viewport_height * MAX_DRAWER_HEIGHT_RATIO).floor());
    match layout.chat_column_height {
        None => window_cap,
        // The composer overlays the timeline, so layout cannot reserve its height; without this
        // cap a tall drawer pushes it into the header.
        Some(column) => window_cap.min(
            (column - layout.header_height.unwrap_or(0.0) - layout.composer_height.unwrap_or(0.0))
                .max(0.0),
        ),
    }
}

pub fn clamp_drawer_height(height: f64, max_height: f64) -> f64 {
    let safe = if height.is_finite() {
        height
    } else {
        DEFAULT_HEIGHT
    };
    js_round(safe).max(MIN_DRAWER_HEIGHT).min(max_height)
}

/// A drag of the drawer's top edge. The store hears the height once, when the drag ends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawerDrag {
    pub start_y: f64,
    pub start_height: f64,
    pub height: f64,
}

impl DrawerDrag {
    pub fn start(y: f64, drawer_height: f64) -> Self {
        Self {
            start_y: y,
            start_height: drawer_height,
            height: drawer_height,
        }
    }
    /// The live height after the pointer moved to `y`, when it changed.
    pub fn move_to(&mut self, y: f64, max_height: f64) -> Option<f64> {
        let clamped = clamp_drawer_height(self.start_height + (self.start_y - y), max_height);
        (clamped != self.height).then(|| {
            self.height = clamped;
            clamped
        })
    }
    /// The window or column resized under the drag: the live height, when it changed.
    pub fn relayout(&mut self, max_height: f64) -> Option<f64> {
        let live = clamp_drawer_height(self.height, max_height);
        (live != self.height).then(|| {
            self.height = live;
            live
        })
    }
    /// The height to store. A drag that ends where it started keeps the stored preference,
    /// which may be taller than the capped height the drag started from.
    pub fn end(&self, stored_height: f64) -> Option<f64> {
        (self.height != self.start_height && self.height != stored_height).then_some(self.height)
    }
}

/// `term-<n>` and `terminal-<n>`, optionally followed by a UUID suffix.
fn terminal_number(terminal_id: &str) -> Option<&str> {
    let lower = terminal_id.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("terminal-")
        .or_else(|| lower.strip_prefix("term-"))?;
    let prefix = terminal_id.len() - rest.len();
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let tail = &rest[digits..];
    let uuid = tail.strip_prefix('-').is_some_and(|uuid| {
        let parts: Vec<_> = uuid.split('-').collect();
        parts.len() == 5
            && parts
                .iter()
                .zip([8, 4, 4, 4, 12])
                .all(|(part, len)| part.len() == len && part.bytes().all(|b| b.is_ascii_hexdigit()))
    });
    (tail.is_empty() || uuid).then(|| &terminal_id[prefix..prefix + digits])
}

/// A terminal's name: "Terminal {number}" for the ids the app allocates, else content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalLabel {
    Numbered(String),
    Text(String),
}

pub fn terminal_label(terminal_id: &str) -> TerminalLabel {
    match terminal_number(terminal_id) {
        Some(number) => TerminalLabel::Numbered(number.into()),
        None => TerminalLabel::Text(terminal_id.into()),
    }
}

pub fn resolve_terminal_session_label(terminal_id: &str, label: Option<&str>) -> TerminalLabel {
    match label.map(JsTrim::js_trim).filter(|label| !label.is_empty()) {
        Some(label) => TerminalLabel::Text(label.into()),
        None => terminal_label(terminal_id),
    }
}

/// Append a UUID when metadata is unavailable to avoid reusing unseen host sessions.
pub fn next_terminal_id(existing: &[String], unique_suffix: Option<&str>) -> String {
    let used: BTreeSet<_> = existing
        .iter()
        .filter_map(|id| terminal_number(id))
        .collect();
    let mut next = 1u64;
    while used.contains(next.to_string().as_str()) {
        next += 1;
    }
    match unique_suffix {
        Some(suffix) => format!("term-{next}-{suffix}"),
        None => format!("term-{next}"),
    }
}

/// Server lists lag and colliding ids share a PTY; without read access, suffix ids to avoid
/// unseen sessions.
pub fn allocate_terminal_id(
    known: Option<&[String]>,
    drawer: &[String],
    panel: &[String],
    can_read: bool,
    unique_suffix: impl FnOnce() -> String,
) -> String {
    let mut ids: Vec<String> = Vec::new();
    for id in known.unwrap_or_default().iter().chain(drawer).chain(panel) {
        if !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    if known.is_some() && can_read {
        next_terminal_id(&ids, None)
    } else {
        next_terminal_id(&ids, Some(&unique_suffix()))
    }
}

pub fn normalize_terminal_ids(ids: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    ids.iter()
        .map(|id| id.js_trim())
        .filter(|id| !id.is_empty() && seen.insert(*id))
        .map(str::to_owned)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    Single,
    Stacked,
    SideBySide,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGroup {
    pub id: String,
    pub terminal_ids: Vec<String>,
    pub vertical: bool,
}

impl ResolvedGroup {
    pub fn kind(&self) -> GroupKind {
        match (self.terminal_ids.len() > 1, self.vertical) {
            (false, _) => GroupKind::Single,
            (true, true) => GroupKind::Stacked,
            (true, false) => GroupKind::SideBySide,
        }
    }
    /// The terminal its header activates: the active one when it is in the group.
    pub fn active_terminal<'a>(&'a self, active: &'a str) -> &'a str {
        if self.terminal_ids.iter().any(|id| id == active) {
            active
        } else {
            self.terminal_ids.first().map_or(active, String::as_str)
        }
    }
}

/// The drawer's groups: the stored ones repaired against the ids, then one group per
/// ungrouped id, ordered by their first terminal.
pub fn resolve_terminal_groups(ids: &[String], groups: &[Group]) -> Vec<ResolvedGroup> {
    if ids.is_empty() {
        return Vec::new();
    }
    let valid: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    let mut assigned = BTreeSet::new();
    let mut used = BTreeSet::new();
    let mut unique = |base: String| {
        if used.insert(base.clone()) {
            return base;
        }
        let mut suffix = 2;
        while used.contains(&format!("{base}-{suffix}")) {
            suffix += 1;
        }
        let id = format!("{base}-{suffix}");
        used.insert(id.clone());
        id
    };
    let mut next = Vec::new();
    for group in groups {
        let mut seen = BTreeSet::new();
        let terminal_ids: Vec<String> = group
            .terminal_ids
            .iter()
            .map(|id| id.js_trim())
            .filter(|id| {
                !id.is_empty() && seen.insert(*id) && valid.contains(id) && !assigned.contains(*id)
            })
            .map(str::to_owned)
            .collect();
        if terminal_ids.is_empty() {
            continue;
        }
        assigned.extend(terminal_ids.iter().cloned());
        let base = if group.id.js_trim().is_empty() {
            format!("group-{}", terminal_ids[0])
        } else {
            group.id.js_trim().into()
        };
        next.push(ResolvedGroup {
            id: unique(base),
            terminal_ids,
            vertical: group.split_direction.as_deref() == Some("vertical"),
        });
    }
    for id in ids {
        if assigned.contains(id.as_str()) {
            continue;
        }
        next.push(ResolvedGroup {
            id: unique(format!("group-{id}")),
            terminal_ids: vec![id.clone()],
            vertical: false,
        });
    }
    let order: BTreeMap<&str, usize> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index))
        .collect();
    let rank = |group: &ResolvedGroup| {
        group
            .terminal_ids
            .iter()
            .map(|id| order.get(id.as_str()).copied().unwrap_or(usize::MAX))
            .min()
            .unwrap_or(usize::MAX)
    };
    next.sort_by_key(rank);
    next
}

/// What the drawer shows for its ids, active terminal and groups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrawerPresentation {
    pub terminal_ids: Vec<String>,
    pub groups: Vec<ResolvedGroup>,
    pub active_terminal_id: String,
    pub active_group_index: usize,
    pub visible_terminal_ids: Vec<String>,
    pub vertical: bool,
    pub has_sidebar: bool,
    pub show_group_headers: bool,
    pub split_limit_reached: bool,
}

pub fn present_terminal_drawer(
    terminal_ids: &[String],
    active_terminal_id: &str,
    groups: &[Group],
    active_group_id: &str,
) -> DrawerPresentation {
    let ids = normalize_terminal_ids(terminal_ids);
    let groups = resolve_terminal_groups(&ids, groups);
    let active = if ids.iter().any(|id| id == active_terminal_id) {
        active_terminal_id.to_owned()
    } else {
        ids.first().cloned().unwrap_or_default()
    };
    let active_group_index = groups
        .iter()
        .position(|group| group.id == active_group_id)
        .or_else(|| {
            groups
                .iter()
                .position(|group| group.terminal_ids.contains(&active))
        })
        .unwrap_or(0);
    let visible = groups.get(active_group_index).map_or_else(
        || {
            if ids.is_empty() {
                Vec::new()
            } else {
                vec![active.clone()]
            }
        },
        |group| group.terminal_ids.clone(),
    );
    DrawerPresentation {
        has_sidebar: ids.len() > 1,
        show_group_headers: groups.len() > 1
            || groups.iter().any(|group| group.terminal_ids.len() > 1),
        split_limit_reached: visible.len() >= MAX_PER_GROUP,
        vertical: groups
            .get(active_group_index)
            .is_some_and(|group| group.vertical),
        visible_terminal_ids: visible,
        active_group_index,
        active_terminal_id: active,
        terminal_ids: ids,
        groups,
    }
}

/// A drawer action's label: plain, with its shortcut, or the split limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionLabel {
    Plain,
    Shortcut(String),
    Limit(usize),
}

pub fn split_action_label(limit_reached: bool, shortcut: Option<&str>) -> ActionLabel {
    if limit_reached {
        ActionLabel::Limit(MAX_PER_GROUP)
    } else {
        action_label(shortcut)
    }
}

pub fn action_label(shortcut: Option<&str>) -> ActionLabel {
    match shortcut {
        Some(shortcut) if !shortcut.is_empty() => ActionLabel::Shortcut(shortcut.into()),
        _ => ActionLabel::Plain,
    }
}

/// The question asked before terminals close: their running processes stop and their
/// history is deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseConfirmation {
    One(String),
    Many(Vec<String>),
}

pub fn close_confirmation(labels: Vec<String>) -> Option<CloseConfirmation> {
    match labels.len() {
        0 => None,
        1 => labels.into_iter().next().map(CloseConfirmation::One),
        _ => Some(CloseConfirmation::Many(labels)),
    }
}

/// Whether the client's ids and the server's are the same set.
pub fn terminal_id_lists_equal(left: &[String], right: &[String]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut left = left.to_vec();
    let mut right = right.to_vec();
    left.sort();
    right.sort();
    left == right
}

/// Server sessions lag `terminal.open`; reconciling strict subsets would discard new splits.
pub fn server_terminal_ids_strict_subset_of_client(server: &[String], client: &[String]) -> bool {
    if server.len() >= client.len() || client.is_empty() {
        return false;
    }
    server.iter().all(|id| client.contains(id))
}

pub fn should_reconcile_terminal_ids(server: &[String], client: &[String]) -> bool {
    !terminal_id_lists_equal(server, client)
        && !server_terminal_ids_strict_subset_of_client(server, client)
}

pub fn project_script_cwd(project_cwd: &str, worktree_path: Option<&str>) -> String {
    worktree_path.unwrap_or(project_cwd).to_owned()
}

pub fn project_script_runtime_env(
    project_cwd: &str,
    worktree_path: Option<&str>,
    extra: Option<&BTreeMap<String, String>>,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::from([("T3CODE_PROJECT_ROOT".to_owned(), project_cwd.to_owned())]);
    if let Some(path) = worktree_path.filter(|path| !path.is_empty()) {
        env.insert("T3CODE_WORKTREE_PATH".into(), path.into());
    }
    if let Some(extra) = extra {
        env.extend(extra.clone());
    }
    env
}

/// `Intl.Collator(undefined, { numeric: true })` over terminal ids: digit runs by value,
/// letters without case first.
pub fn compare_terminal_ids(left: &str, right: &str) -> Ordering {
    fn runs(value: &str) -> Vec<(bool, &str)> {
        let mut runs = Vec::new();
        let mut start = 0;
        let mut digit = None;
        for (index, c) in value.char_indices() {
            let is_digit = c.is_ascii_digit();
            if digit.is_some_and(|d| d != is_digit) {
                runs.push((digit == Some(true), &value[start..index]));
                start = index;
            }
            digit = Some(is_digit);
        }
        if start < value.len() {
            runs.push((digit == Some(true), &value[start..]));
        }
        runs
    }
    let (a, b) = (runs(left), runs(right));
    for ((left_digit, left), (right_digit, right)) in a.iter().zip(&b) {
        let order = if *left_digit && *right_digit {
            let (l, r) = (left.trim_start_matches('0'), right.trim_start_matches('0'));
            l.len().cmp(&r.len()).then_with(|| l.cmp(r))
        } else {
            left.to_lowercase().cmp(&right.to_lowercase())
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    a.len()
        .cmp(&b.len())
        .then_with(|| left.to_lowercase().cmp(&right.to_lowercase()))
        .then_with(|| left.cmp(right))
}

pub fn should_handle_terminal_exit(
    current: TerminalStatus,
    synchronized: TerminalStatus,
    already_handled: bool,
    version: u64,
) -> bool {
    version > 0
        && matches!(current, TerminalStatus::Closed | TerminalStatus::Exited)
        && current != synchronized
        && !already_handled
}

/// A writable session's output generation. A new one is a fresh PTY (or a reconnect) that has
/// not seen the surface's grid yet.
pub fn writable_session_generation(
    version: u64,
    status: TerminalStatus,
    generation: u64,
    can_operate: bool,
) -> Option<u64> {
    (can_operate && version > 0 && status == TerminalStatus::Running).then_some(generation)
}

/// The line the drawer writes into a terminal for its own news.
pub fn system_message(message: &str) -> String {
    format!("\r\n[terminal] {message}\r\n")
}

/// What an emulator surface offers the drawer's output synchronization.
pub trait TerminalSurfaceSink {
    fn reset_and_write(&mut self, data: &str);
    fn write(&mut self, data: &str);
    fn selection_cell_count(&self) -> usize;
    fn selection(&self) -> String;
    fn clear_selection(&mut self);
}

pub fn write_terminal_output_update(
    sink: &mut impl TerminalSurfaceSink,
    update: &TerminalOutputUpdate,
) {
    match update {
        TerminalOutputUpdate::Reset { data, .. } => sink.reset_and_write(data),
        TerminalOutputUpdate::Append { data, .. } => sink.write(data),
        TerminalOutputUpdate::None(_) => {}
    }
}

/// Writes the output the surface has not shown since `cursor`, and clears a selection the
/// output moved. Reads only what is new unless the output was reset.
pub fn synchronize_terminal_output(
    sink: &mut impl TerminalSurfaceSink,
    output: &TerminalOutputState,
    version: u64,
    cursor: TerminalOutputCursor,
) -> TerminalOutputCursor {
    if version == 0 {
        return cursor;
    }
    let update = output.read(cursor);
    if matches!(update, TerminalOutputUpdate::None(_)) {
        return update.cursor();
    }
    let cells = sink.selection_cell_count();
    let compared = cells <= MAX_COMPARED_SELECTION_CELLS;
    let selection = if cells > 0 && compared {
        sink.selection()
    } else {
        String::new()
    };
    write_terminal_output_update(sink, &update);
    if !compared || (cells > 0 && sink.selection() != selection) {
        sink.clear_selection();
    }
    update.cursor()
}

/// What changed between two session notifications, by identity rather than by content.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionMark {
    pub version: u64,
    pub output: (u64, u64, u64),
    pub error: Option<String>,
}

impl SessionMark {
    pub fn of(version: u64, output: &TerminalOutputState, error: Option<&str>) -> Self {
        Self {
            version,
            output: (output.generation, output.reset_version, output.next_offset),
            error: error.map(str::to_owned),
        }
    }
}

/// Writes what changed between two session notifications: new output, then a new error.
pub fn synchronize_terminal_session(
    sink: &mut impl TerminalSurfaceSink,
    previous: &SessionMark,
    current: &SessionMark,
    output: &TerminalOutputState,
    cursor: TerminalOutputCursor,
) -> TerminalOutputCursor {
    let next = if current.version == previous.version && current.output == previous.output {
        cursor
    } else {
        synchronize_terminal_output(sink, output, current.version, cursor)
    };
    // An attach failure can change only the error, keeping the last buffer.
    if let Some(error) = &current.error
        && current.error != previous.error
    {
        sink.write(&system_message(error));
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::terminal::{AttachGenerations, TerminalBufferState};
    use t3_contracts::terminal::*;

    #[derive(Default)]
    struct Sink {
        resets: Vec<String>,
        writes: Vec<String>,
        selection: String,
        cells: usize,
        cleared: usize,
        reads: std::cell::Cell<usize>,
    }
    impl TerminalSurfaceSink for Sink {
        fn reset_and_write(&mut self, data: &str) {
            self.resets.push(data.into());
        }
        fn write(&mut self, data: &str) {
            self.writes.push(data.into());
        }
        fn selection_cell_count(&self) -> usize {
            self.reads.set(self.reads.get() + 1);
            self.cells
        }
        fn selection(&self) -> String {
            self.reads.set(self.reads.get() + 1);
            self.selection.clone()
        }
        fn clear_selection(&mut self) {
            self.cleared += 1;
        }
    }
    impl Sink {
        fn selected() -> Self {
            Self {
                selection: "selected".into(),
                cells: 8,
                ..Self::default()
            }
        }
    }

    fn snapshot(history: &str) -> TerminalSessionSnapshot {
        serde_json::from_value(serde_json::json!({
            "threadId":"thread","terminalId":"term-1","cwd":"/repo","worktreePath":null,
            "status":"running","pid":123,"history":history,"exitCode":null,"exitSignal":null,
            "label":"Terminal 1","updatedAt":"2026-09-05T00:00:00.000Z"
        }))
        .unwrap_or_else(|error| panic!("{error}"))
    }
    fn snapshot_event(history: &str) -> TerminalAttachStreamEvent {
        serde_json::from_value(serde_json::json!({"type":"snapshot","snapshot":snapshot(history)}))
            .unwrap_or_else(|error| panic!("{error}"))
    }
    fn event(value: serde_json::Value) -> TerminalAttachStreamEvent {
        serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"))
    }
    fn attached(generations: &mut AttachGenerations, history: &str) -> TerminalBufferState {
        let mut buffer = generations.seed();
        buffer.apply(&snapshot_event(history), 1 << 20, generations);
        buffer
    }

    #[test]
    fn handles_an_exit_that_lands_while_the_surface_is_still_loading() {
        use TerminalStatus::*;
        assert!(should_handle_terminal_exit(Exited, Running, false, 1));
        assert!(!should_handle_terminal_exit(Exited, Exited, false, 1));
        assert!(!should_handle_terminal_exit(Closed, Running, true, 1));
        assert!(!should_handle_terminal_exit(Closed, Running, false, 0));
    }

    #[test]
    fn retains_visible_output_and_selection_until_a_replacement_subscription_receives_its_snapshot()
    {
        let mut generations = AttachGenerations::default();
        let mut sink = Sink::selected();
        let first = attached(&mut generations, "host output");
        let cursor = synchronize_terminal_output(
            &mut sink,
            &first.output,
            first.version,
            TerminalOutputCursor::default(),
        );
        assert_eq!(sink.resets, ["host output"]);
        sink.resets.clear();
        sink.cleared = 0;

        // A replacement subscription before its snapshot: nothing is written or cleared.
        let pending = generations.seed();
        assert_eq!(
            synchronize_terminal_output(&mut sink, &pending.output, pending.version, cursor),
            cursor
        );
        assert!(sink.resets.is_empty() && sink.writes.is_empty() && sink.cleared == 0);

        let mut observed = attached(&mut generations, "host output\nobserved output");
        let observed_cursor =
            synchronize_terminal_output(&mut sink, &observed.output, observed.version, cursor);
        assert_eq!(sink.resets, ["host output\nobserved output"]);

        observed.apply(
            &event(serde_json::json!({"type":"cleared","threadId":"thread","terminalId":"term-1"})),
            1 << 20,
            &mut generations,
        );
        sink.resets.clear();
        synchronize_terminal_output(
            &mut sink,
            &observed.output,
            observed.version,
            observed_cursor,
        );
        assert!(sink.resets.is_empty());
        assert_eq!(
            sink.writes.last().map(String::as_str),
            Some("\u{1b}[H\u{1b}[2J\u{1b}[3J")
        );
    }

    #[test]
    fn reports_an_attach_failure_that_leaves_the_output_unchanged() {
        let mut sink = Sink::selected();
        let output = TerminalOutputState::default();
        let previous = SessionMark::of(0, &output, None);
        let failed = SessionMark::of(0, &output, Some("Terminal attach failed"));
        let cursor = TerminalOutputCursor::default();
        assert_eq!(
            synchronize_terminal_session(&mut sink, &previous, &failed, &output, cursor),
            cursor
        );
        assert_eq!(sink.writes, ["\r\n[terminal] Terminal attach failed\r\n"]);
        assert!(sink.resets.is_empty() && sink.cleared == 0 && sink.reads.get() == 0);
    }

    #[test]
    fn appends_read_only_the_new_output_and_clear_a_moved_selection() {
        let mut generations = AttachGenerations::default();
        let mut buffer = attached(&mut generations, "start");
        let mut sink = Sink::selected();
        let mut cursor = synchronize_terminal_output(
            &mut sink,
            &buffer.output,
            buffer.version,
            TerminalOutputCursor::default(),
        );
        for chunk in ["a", "b", "c"] {
            buffer.apply(
                &event(serde_json::json!({"type":"output","threadId":"thread","terminalId":"term-1","data":chunk})),
                1 << 20,
                &mut generations,
            );
        }
        sink.selection = "moved".into();
        let before = sink.cleared;
        cursor = synchronize_terminal_output(&mut sink, &buffer.output, buffer.version, cursor);
        assert_eq!(sink.writes, ["abc"]);
        assert_eq!(sink.resets, ["start"]);
        // The fake's selection did not change across the write: nothing is cleared.
        assert_eq!(sink.cleared, before);
        assert!(matches!(
            buffer.output.read(cursor),
            TerminalOutputUpdate::None(_)
        ));
        // A selection over the comparison limit is cleared without being formatted.
        let mut huge = Sink {
            cells: MAX_COMPARED_SELECTION_CELLS + 1,
            ..Sink::default()
        };
        buffer.apply(
            &event(serde_json::json!({"type":"output","threadId":"thread","terminalId":"term-1","data":"d"})),
            1 << 20,
            &mut generations,
        );
        synchronize_terminal_output(&mut huge, &buffer.output, buffer.version, cursor);
        assert_eq!(huge.cleared, 1);
    }

    #[test]
    fn labels_and_ids_follow_the_shared_rules() {
        assert_eq!(
            terminal_label("term-3"),
            TerminalLabel::Numbered("3".into())
        );
        assert_eq!(
            terminal_label("Terminal-12"),
            TerminalLabel::Numbered("12".into())
        );
        assert_eq!(
            terminal_label("term-2-0a1b2c3d-0000-4000-8000-0123456789ab"),
            TerminalLabel::Numbered("2".into())
        );
        assert_eq!(
            terminal_label("term-2-x"),
            TerminalLabel::Text("term-2-x".into())
        );
        assert_eq!(
            terminal_label("script-1"),
            TerminalLabel::Text("script-1".into())
        );
        assert_eq!(
            resolve_terminal_session_label("term-1", Some("  dev server ")),
            TerminalLabel::Text("dev server".into())
        );
        assert_eq!(
            resolve_terminal_session_label("term-1", Some("  ")),
            TerminalLabel::Numbered("1".into())
        );
        let ids = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            next_terminal_id(&ids(&["term-1", "term-3"]), None),
            "term-2"
        );
        assert_eq!(next_terminal_id(&ids(&[]), Some("u")), "term-1-u");
        assert_eq!(
            allocate_terminal_id(
                Some(&ids(&["term-1"])),
                &ids(&["term-2"]),
                &[],
                true,
                || "u".into()
            ),
            "term-3"
        );
        // Without readable metadata, a suffix keeps unseen host sessions apart.
        assert_eq!(
            allocate_terminal_id(None, &ids(&["term-1"]), &[], true, || "u".into()),
            "term-2-u"
        );
        assert_eq!(
            allocate_terminal_id(Some(&[]), &[], &[], false, || "u".into()),
            "term-1-u"
        );
    }

    #[test]
    fn groups_are_repaired_and_ordered_by_their_first_terminal() {
        let ids: Vec<String> = ["a", "b", "c", "d"].map(String::from).to_vec();
        let groups = vec![
            Group {
                id: "g".into(),
                terminal_ids: vec!["d".into(), "c".into(), "missing".into()],
                split_direction: Some("vertical".into()),
            },
            Group {
                id: "g".into(),
                terminal_ids: vec!["c".into(), "b".into()],
                split_direction: None,
            },
        ];
        let resolved = resolve_terminal_groups(&ids, &groups);
        assert_eq!(
            resolved,
            vec![
                ResolvedGroup {
                    id: "group-a".into(),
                    terminal_ids: vec!["a".into()],
                    vertical: false
                },
                ResolvedGroup {
                    id: "g-2".into(),
                    terminal_ids: vec!["b".into()],
                    vertical: false
                },
                ResolvedGroup {
                    id: "g".into(),
                    terminal_ids: vec!["d".into(), "c".into()],
                    vertical: true
                },
            ]
        );
        assert_eq!(resolved[2].kind(), GroupKind::Stacked);
        assert_eq!(resolved[2].active_terminal("a"), "d");
        let view = present_terminal_drawer(&ids, "c", &groups, "nope");
        assert_eq!(view.visible_terminal_ids, ["d", "c"]);
        assert!(view.vertical && view.has_sidebar && view.show_group_headers);
        assert!(!view.split_limit_reached);
        let single = present_terminal_drawer(&["x".into()], "", &[], "");
        assert_eq!(single.active_terminal_id, "x");
        assert!(!single.has_sidebar && !single.show_group_headers);
        let empty = present_terminal_drawer(&[], "", &[], "");
        assert!(empty.visible_terminal_ids.is_empty());
    }

    #[test]
    fn heights_clamp_to_the_window_and_the_chat_column() {
        let window = DrawerLayout {
            viewport_height: 800.0,
            ..Default::default()
        };
        assert_eq!(resolve_terminal_drawer_max_height(&window), 600.0);
        let column = DrawerLayout {
            viewport_height: 800.0,
            chat_column_height: Some(500.0),
            header_height: Some(50.0),
            composer_height: Some(120.0),
        };
        assert_eq!(resolve_terminal_drawer_max_height(&column), 330.0);
        assert_eq!(
            resolve_terminal_drawer_max_height(&DrawerLayout {
                viewport_height: 100.0,
                ..Default::default()
            }),
            MIN_DRAWER_HEIGHT
        );
        assert_eq!(clamp_drawer_height(f64::NAN, 600.0), DEFAULT_HEIGHT);
        assert_eq!(clamp_drawer_height(100.0, 600.0), MIN_DRAWER_HEIGHT);
        assert_eq!(clamp_drawer_height(900.4, 600.0), 600.0);
        assert_eq!(clamp_drawer_height(250.5, 600.0), 251.0);

        let mut drag = DrawerDrag::start(500.0, 280.0);
        assert_eq!(drag.move_to(500.0, 600.0), None);
        assert_eq!(drag.move_to(400.0, 600.0), Some(380.0));
        assert_eq!(drag.relayout(300.0), Some(300.0));
        assert_eq!(drag.end(280.0), Some(300.0));
        // A drag back to its start keeps the stored preference.
        let mut back = DrawerDrag::start(500.0, 280.0);
        back.move_to(450.0, 600.0);
        back.move_to(500.0, 600.0);
        assert_eq!(back.end(400.0), None);
        // A stored height taller than the cap is not rewritten by a drag ending at it.
        let mut same = DrawerDrag::start(500.0, 300.0);
        same.move_to(480.0, 600.0);
        assert_eq!(same.end(320.0), None);
    }

    #[test]
    fn reconciliation_skips_equal_sets_and_lagging_server_subsets() {
        let v = |s: &[&str]| s.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(!should_reconcile_terminal_ids(
            &v(&["b", "a"]),
            &v(&["a", "b"])
        ));
        assert!(!should_reconcile_terminal_ids(&v(&["a"]), &v(&["a", "b"])));
        assert!(!should_reconcile_terminal_ids(&v(&[]), &v(&[])));
        assert!(should_reconcile_terminal_ids(
            &v(&["a", "c"]),
            &v(&["a", "b"])
        ));
        assert!(should_reconcile_terminal_ids(&v(&["a", "b"]), &v(&["a"])));
        assert!(should_reconcile_terminal_ids(&v(&["a"]), &v(&[])));
    }

    #[test]
    fn project_scripts_run_in_the_worktree_with_its_environment() {
        assert_eq!(project_script_cwd("/p", Some("/w")), "/w");
        assert_eq!(project_script_cwd("/p", None), "/p");
        let env = project_script_runtime_env("/p", Some(""), None);
        assert_eq!(env.len(), 1);
        let extra = BTreeMap::from([("T3CODE_PROJECT_ROOT".to_owned(), "/x".to_owned())]);
        let env = project_script_runtime_env("/p", Some("/w"), Some(&extra));
        assert_eq!(env["T3CODE_PROJECT_ROOT"], "/x");
        assert_eq!(env["T3CODE_WORKTREE_PATH"], "/w");
    }

    #[test]
    fn terminal_ids_sort_numerically() {
        let mut ids = vec!["term-10", "term-2", "Term-1", "alpha"];
        ids.sort_by(|a, b| compare_terminal_ids(a, b));
        assert_eq!(ids, ["alpha", "Term-1", "term-2", "term-10"]);
    }
}
