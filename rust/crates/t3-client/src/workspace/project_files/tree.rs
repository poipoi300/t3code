//! Port of useDirectoryEntries.ts, fileTreeExpansion.ts, loadFileAncestors.ts and the tree state of
//! FileBrowserPanel.tsx, with the parts of Pierre's tree it configures (natural sort, compact
//! folders, `hide-non-matches` search). No I/O: the service feeds listings and search replies in.
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    rc::Rc,
};
use t3_contracts::project::{ProjectEntry, ProjectEntryKind};

/// useDirectoryEntries.ts: at most four folder reads at once, shared by every tree load.
pub const MAX_CONCURRENT_LISTS: usize = 4;
pub const EXPANDED_DIRECTORIES_STORAGE_KEY: &str = "t3code.fileTreeExpandedDirectories";
/// Roots are kept most recent first and capped so worktrees that come and go do not grow storage.
pub const MAX_STORED_ROOTS: usize = 20;
/// FileBrowserPanel.tsx passes 200 to useProjectPathSearch; queries.ts debounces by 120 ms.
pub const SEARCH_LIMIT: u32 = 200;
pub const SEARCH_DEBOUNCE_MS: u64 = 120;
pub const SEARCH_QUERY_MAX_UTF16: usize = 256;
pub const UNABLE_TO_LOAD_FOLDER: &str = "Unable to load folder.";

/// An error line: `owned` when it is client English (a translation key), else server detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub text: String,
    pub owned: bool,
}
impl Problem {
    pub fn client(text: &str) -> Self {
        Self {
            text: text.into(),
            owned: true,
        }
    }
    pub fn server(text: String) -> Self {
        Self { text, owned: false }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Child {
    path: Rc<str>,
    name_start: usize,
    directory: bool,
    ignored: bool,
}
impl Child {
    fn name(&self) -> &str {
        &self.path[self.name_start..]
    }
}

/// A row of the tree as Pierre draws it. A compact folder chain (`a/b/c` with one folder each)
/// is one node: `id` is the head folder, whose expansion it follows, and `path` the folder whose
/// children it shows.
#[derive(Debug, Clone, PartialEq)]
pub struct FileTreeNode {
    pub id: Rc<str>,
    pub path: Rc<str>,
    pub label: Rc<str>,
    pub kind: ProjectEntryKind,
    pub ignored: bool,
    /// An expanded folder whose first read has not answered (Pierre's loading decoration).
    pub loading: bool,
    /// `None` for a file, a collapsed folder, or a folder not listed yet.
    pub children: Option<Vec<FileTreeNode>>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchView {
    pub active: bool,
    pub pending: bool,
    pub truncated: bool,
    pub error: Option<Problem>,
    /// Pierre restores the unfiltered tree when a search has no matches; the panel says so.
    pub no_matches: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileTreeView {
    pub nodes: Rc<Vec<FileTreeNode>>,
    /// Ids of the expanded nodes.
    pub expanded: Rc<Vec<Rc<str>>>,
    pub ready: bool,
    pub pending: bool,
    pub error: Option<Problem>,
    pub search: SearchView,
    pub has_entries: bool,
    pub has_directories: bool,
    pub all_expanded: bool,
    pub expand_all: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct SearchState {
    query: String,
    pending: bool,
    entries: Rc<Vec<ProjectEntry>>,
    truncated: bool,
    error: Option<Problem>,
}

/// What the panel must do for an external selection (a file link, a breadcrumb).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reveal {
    pub id: Rc<str>,
}

#[derive(Debug, Default)]
pub struct TreeModel {
    directories: BTreeMap<Rc<str>, Rc<[Child]>>,
    errors: Vec<(String, Problem)>,
    loading: BTreeSet<String>,
    in_flight: BTreeMap<String, bool>,
    follow_up: BTreeSet<String>,
    queued: VecDeque<(String, bool)>,
    loaded: BTreeSet<String>,
    requested: BTreeSet<String>,
    directory_count: usize,
    expanded: BTreeSet<String>,
    restore: BTreeSet<String>,
    expand_all: bool,
    query: String,
    search: SearchState,
    search_generation: u64,
    selected_by_tree: Option<String>,
    handled_reveal: Option<(String, u64)>,
    handled_mutation: Option<String>,
    /// Bumped whenever the expanded set changes, so the service persists it.
    pub expansion_revision: u64,
    /// Lists started, for tests that count work.
    pub lists_started: u64,
}

/// Pierre's sort: folders first, then natural order of the lower-cased name, then raw.
pub fn compare_names(left: &str, right: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    #[derive(PartialEq)]
    enum Token<'a> {
        Text(&'a str),
        Number(u128),
    }
    fn tokens(value: &str) -> Vec<Token<'_>> {
        let bytes = value.as_bytes();
        let mut out = Vec::new();
        let (mut start, mut index) = (0, 0);
        while index < bytes.len() {
            while index < bytes.len() && !bytes[index].is_ascii_digit() {
                index += 1;
            }
            if index >= bytes.len() {
                break;
            }
            if index > start {
                out.push(Token::Text(&value[start..index]));
            }
            let mut number: u128 = 0;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                number = number
                    .saturating_mul(10)
                    .saturating_add(u128::from(bytes[index] - b'0'));
                index += 1;
            }
            out.push(Token::Number(number));
            start = index;
        }
        if start < value.len() || out.is_empty() {
            out.push(Token::Text(&value[start..]));
        }
        out
    }
    let (lower_left, lower_right) = (left.to_lowercase(), right.to_lowercase());
    let (a, b) = (tokens(&lower_left), tokens(&lower_right));
    let simple = a.len() == 1
        && b.len() == 1
        && matches!(a[0], Token::Text(_))
        && matches!(b[0], Token::Text(_));
    let natural = if simple {
        lower_left.cmp(&lower_right)
    } else {
        let mut order = Ordering::Equal;
        for (x, y) in a.iter().zip(&b) {
            if x == y {
                continue;
            }
            order = match (x, y) {
                (Token::Number(x), Token::Number(y)) => x.cmp(y),
                (x, y) => token_text(x).cmp(&token_text(y)),
            };
            break;
        }
        order
            .then(a.len().cmp(&b.len()))
            .then_with(|| lower_left.cmp(&lower_right))
    };
    fn token_text(token: &Token<'_>) -> String {
        match token {
            Token::Text(text) => (*text).to_owned(),
            Token::Number(number) => number.to_string(),
        }
    }
    natural.then_with(|| left.cmp(right))
}

fn compare_children(left: &Child, right: &Child) -> std::cmp::Ordering {
    right
        .directory
        .cmp(&left.directory)
        .then_with(|| compare_names(left.name(), right.name()))
}

fn parent_of(path: &str) -> &str {
    path.rfind('/').map_or("", |index| &path[..index])
}

/// Pierre's search value: trimmed, backslashes as slashes, lower case.
pub fn normalize_search_query(value: &str) -> String {
    value.trim().replace('\\', "/").to_lowercase()
}

/// queries.ts: the first 256 UTF-16 units, never ending inside a surrogate pair.
pub fn bounded_search_query(value: &str) -> String {
    let mut units = 0;
    let mut end = 0;
    for (index, character) in value.char_indices() {
        units += character.len_utf16();
        if units > SEARCH_QUERY_MAX_UTF16 {
            break;
        }
        end = index + character.len_utf8();
    }
    value[..end].to_owned()
}

/// fileTreeExpansion.ts storage: `[{ root, paths }]`, most recent root first, at most twenty.
pub fn read_expanded_directories(stored: Option<&[u8]>, root: &str) -> Vec<String> {
    stored_roots(stored)
        .into_iter()
        .find(|entry| entry.root == root)
        .map(|entry| entry.paths)
        .unwrap_or_default()
}
pub fn write_expanded_directories(
    stored: Option<&[u8]>,
    root: &str,
    paths: Vec<String>,
) -> Vec<u8> {
    let mut next: Vec<StoredRoot> = stored_roots(stored)
        .into_iter()
        .filter(|entry| entry.root != root)
        .collect();
    if !paths.is_empty() {
        next.insert(
            0,
            StoredRoot {
                root: root.into(),
                paths,
            },
        );
    }
    next.truncate(MAX_STORED_ROOTS);
    serde_json::to_vec(&next).unwrap_or_else(|_| b"[]".to_vec())
}
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredRoot {
    root: String,
    paths: Vec<String>,
}
fn stored_roots(stored: Option<&[u8]>) -> Vec<StoredRoot> {
    stored
        .and_then(|bytes| serde_json::from_slice(bytes).ok())
        .unwrap_or_default()
}

impl TreeModel {
    /// A fresh tree starts its root read at once, as the hook's mount effect does.
    pub fn new(restored: Vec<String>) -> Self {
        let mut model = Self {
            restore: restored.into_iter().collect(),
            ..Self::default()
        };
        model.request("", false);
        model
    }

    /// Folders expanded when the panel last closed; each reopens once lazy loading lists it.
    pub fn restore(&mut self, paths: Vec<String>) {
        self.restore.extend(paths);
        self.sync_expansion();
    }

    /// `load(directoryPath, refresh)`: one read per folder; a refresh during a read follows it.
    pub fn request(&mut self, directory: &str, refresh: bool) {
        let busy = self.in_flight.contains_key(directory)
            || self.queued.iter().any(|(path, _)| path == directory);
        if busy {
            if refresh {
                self.follow_up.insert(directory.into());
            }
            return;
        }
        if !refresh && self.loaded.contains(directory) {
            return;
        }
        if !refresh {
            self.loading.insert(directory.into());
        }
        self.loaded.insert(directory.into());
        self.requested.insert(directory.into());
        self.queued.push_back((directory.into(), refresh));
    }

    /// The next folder to read under the shared cap.
    pub fn next_start(&mut self) -> Option<String> {
        if self.in_flight.len() >= MAX_CONCURRENT_LISTS {
            return None;
        }
        let (path, refresh) = self.queued.pop_front()?;
        self.in_flight.insert(path.clone(), refresh);
        self.lists_started += 1;
        Some(path)
    }

    pub fn complete(&mut self, directory: &str, result: Result<Vec<ProjectEntry>, Problem>) {
        let refresh = self.in_flight.remove(directory).unwrap_or(false);
        match result {
            Ok(entries) => {
                let mut children: Vec<Child> = entries
                    .into_iter()
                    .filter(|entry| parent_of(&entry.path.0) == directory)
                    .map(|entry| Child {
                        name_start: entry.path.0.rfind('/').map_or(0, |index| index + 1),
                        directory: entry.kind == ProjectEntryKind::Directory,
                        ignored: entry.ignored.flatten() == Some(true),
                        path: Rc::from(entry.path.0),
                    })
                    .collect();
                children.sort_by(compare_children);
                let count = children.iter().filter(|child| child.directory).count();
                let previous = self.directories.insert(directory.into(), children.into());
                let before = previous.map_or(0, |old| old.iter().filter(|c| c.directory).count());
                self.directory_count = self.directory_count + count - before;
                self.errors.retain(|(path, _)| path != directory);
            }
            Err(problem) => {
                self.loaded.remove(directory);
                self.errors.retain(|(path, _)| path != directory);
                self.errors.push((directory.into(), problem));
            }
        }
        if !refresh {
            self.loading.remove(directory);
        }
        if self.follow_up.remove(directory) {
            self.request(directory, true);
        }
        self.sync_expansion();
    }

    fn known_directory(&self, path: &str) -> Option<&Child> {
        self.directories
            .get(parent_of(path))?
            .iter()
            .find(|child| child.directory && &*child.path == path)
    }

    fn reachable(&self, directory: &str) -> bool {
        directory.is_empty()
            || self
                .known_directory(directory)
                .is_some_and(|_| self.reachable(parent_of(directory)))
    }

    /// The compact chain from `head`: each folder whose listing is exactly one folder.
    fn chain<'a>(&'a self, head: &'a Child) -> (Vec<&'a Child>, &'a Child) {
        let mut members = vec![head];
        let mut tail = head;
        while let Some(listing) = self.directories.get(&*tail.path)
            && listing.len() == 1
            && listing[0].directory
        {
            tail = &listing[0];
            members.push(tail);
        }
        (members, tail)
    }

    /// The hook's subscription to Pierre's model: restored folders reopen once listed, expanded
    /// folders (and their compact chains) are read, and expand-all opens every known folder that
    /// is not ignored while no search is open.
    fn sync_expansion(&mut self) {
        let searching = !normalize_search_query(&self.query).is_empty();
        let mut loads = Vec::new();
        let mut changed = false;
        if !searching {
            let known: Vec<(String, bool)> = self
                .directories
                .values()
                .flat_map(|listing| listing.iter())
                .filter(|child| child.directory)
                .map(|child| (child.path.to_string(), child.ignored))
                .collect();
            for (path, ignored) in known {
                let restored = self.restore.remove(&path);
                if (restored || (self.expand_all && !ignored)) && self.expanded.insert(path) {
                    changed = true;
                }
            }
        }
        for path in &self.expanded {
            let Some(child) = self.known_directory(path) else {
                continue;
            };
            let (members, _) = self.chain(child);
            loads.extend(members.into_iter().map(|member| member.path.to_string()));
        }
        for path in loads {
            self.request(&path, false);
        }
        if changed {
            self.expansion_revision += 1;
        }
    }

    /// The paths to persist: expanded folders plus restored ones not listed yet.
    pub fn persisted_expansion(&self) -> Vec<String> {
        self.expanded
            .iter()
            .chain(self.restore.iter())
            .cloned()
            .collect()
    }

    /// The tree reported its expanded node ids (heads). Collapsing a folder by hand ends
    /// expand-all, as FileBrowserPanel's `setExpandAll(false)` does.
    pub fn set_expanded(&mut self, ids: &[impl AsRef<str>]) {
        let next: BTreeSet<String> = ids.iter().map(|id| id.as_ref().to_owned()).collect();
        if !normalize_search_query(&self.query).is_empty() {
            return;
        }
        let shown: BTreeSet<String> = self.shown_expanded_ids();
        let collapsed: Vec<String> = shown.difference(&next).cloned().collect();
        let opened: Vec<String> = next.difference(&shown).cloned().collect();
        if collapsed.is_empty() && opened.is_empty() {
            return;
        }
        if !collapsed.is_empty() {
            self.expand_all = false;
        }
        for id in collapsed {
            self.expanded.remove(&id);
        }
        for id in opened {
            self.expanded.insert(id);
        }
        self.expansion_revision += 1;
        self.sync_expansion();
    }

    fn shown_expanded_ids(&self) -> BTreeSet<String> {
        self.view()
            .expanded
            .iter()
            .map(|id| id.to_string())
            .collect()
    }

    /// The header button: expand every known folder that is not ignored, or collapse them all.
    pub fn toggle_all(&mut self) {
        let view = self.view();
        let expanded = !(self.expand_all || view.all_expanded);
        self.expand_all = expanded;
        if expanded {
            let all: Vec<String> = self
                .directories
                .values()
                .flat_map(|listing| listing.iter())
                .filter(|child| child.directory && !child.ignored)
                .map(|child| child.path.to_string())
                .collect();
            self.expanded.extend(all);
        } else {
            self.expanded.clear();
        }
        self.expansion_revision += 1;
        self.sync_expansion();
    }

    /// `refresh()`: re-read every requested folder still reachable from the root.
    pub fn refresh(&mut self) {
        let paths: Vec<String> = self
            .requested
            .iter()
            .filter(|path| self.reachable(path))
            .cloned()
            .collect();
        for path in paths {
            self.request(&path, true);
        }
    }

    /// A new session: read failed folders again rather than leaving the error until a retry.
    pub fn retry_errors(&mut self) {
        let paths: Vec<String> = self.errors.iter().map(|(path, _)| path.clone()).collect();
        for path in paths {
            self.request(&path, false);
        }
    }

    /// useWorkspaceMutationRefresh: refresh once per new mutation token. True when the search
    /// must be refreshed too.
    pub fn observe_mutation(&mut self, mutation: Option<&str>) -> bool {
        let Some(mutation) = mutation else {
            return false;
        };
        if self.handled_mutation.as_deref() == Some(mutation) {
            return false;
        }
        self.handled_mutation = Some(mutation.into());
        self.refresh();
        !normalize_search_query(&self.query).is_empty()
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// The search field changed. Returns the generation the debounced search must carry, or
    /// `None` when no server search is needed (blank query, host folders).
    pub fn set_query(&mut self, query: String, server_search: bool) -> Option<u64> {
        if self.query == query {
            return None;
        }
        let blank = query.trim().is_empty();
        self.query = query;
        self.search_generation += 1;
        if blank || !server_search {
            self.search = SearchState::default();
            self.sync_expansion();
            return None;
        }
        self.search.pending = true;
        Some(self.search_generation)
    }

    /// The input of the debounced search for `generation`, if it is still current.
    pub fn search_input(&self, generation: u64) -> Option<String> {
        (generation == self.search_generation && !self.query.trim().is_empty())
            .then(|| bounded_search_query(self.query.trim()))
    }

    pub fn search_generation(&self) -> u64 {
        self.search_generation
    }

    pub fn complete_search(
        &mut self,
        generation: u64,
        result: Result<(Vec<ProjectEntry>, bool), Problem>,
    ) {
        if generation != self.search_generation {
            return;
        }
        self.search = match result {
            Ok((entries, truncated)) => SearchState {
                query: self.query.clone(),
                pending: false,
                entries: Rc::new(entries),
                truncated,
                error: None,
            },
            Err(problem) => SearchState {
                query: self.query.clone(),
                pending: false,
                entries: Rc::default(),
                truncated: false,
                error: Some(problem),
            },
        };
    }

    /// The search could not run (no file access): the hook reports access, not a stale reply.
    pub fn fail_search_access(&mut self, problem: Problem) {
        self.search = SearchState {
            query: self.query.clone(),
            pending: false,
            entries: Rc::default(),
            truncated: false,
            error: Some(problem),
        };
    }

    /// A press in the tree selected `path`; the next reveal of the same path is not replayed.
    pub fn select_from_tree(&mut self, path: &str) {
        self.selected_by_tree = Some(path.into());
    }

    /// loadFileAncestors: read each folder above `path`, through the shared cap.
    pub fn load_ancestors(&mut self, path: &str) {
        let segments: Vec<&str> = path.split('/').collect();
        for count in 0..segments.len() {
            self.request(&segments[..count].join("/"), false);
        }
    }

    /// The reveal effect: once `path` is known, close the search, open its ancestors and select
    /// it, once per `(path, reveal_id)`. Tree selections are already visible and are not
    /// revealed again (that would close the search).
    pub fn reveal(&mut self, path: Option<&str>, reveal_id: u64) -> Option<Reveal> {
        let Some(path) = path else {
            self.handled_reveal = None;
            return None;
        };
        let known_file = self
            .directories
            .get(parent_of(path))
            .is_some_and(|listing| listing.iter().any(|child| &*child.path == path));
        let known = known_file || self.search_entry_kind(path).is_some();
        if !known {
            self.handled_reveal = None;
            return None;
        }
        let request = (path.to_owned(), reveal_id);
        if self.handled_reveal.as_ref() == Some(&request) {
            return None;
        }
        self.handled_reveal = Some(request);
        if self.selected_by_tree.take().as_deref() == Some(path) {
            return None;
        }
        self.query.clear();
        self.search = SearchState::default();
        self.search_generation += 1;
        let segments: Vec<&str> = path.split('/').collect();
        let mut changed = false;
        for count in 1..segments.len() {
            changed |= self.expanded.insert(segments[..count].join("/"));
        }
        if self.known_directory(path).is_some() {
            changed |= self.expanded.insert(path.into());
        }
        if changed {
            self.expansion_revision += 1;
        }
        self.sync_expansion();
        Some(Reveal {
            id: self.node_id_for(path),
        })
    }

    fn search_entry_kind(&self, path: &str) -> Option<ProjectEntryKind> {
        self.search
            .entries
            .iter()
            .find(|entry| entry.path.0 == path)
            .map(|entry| entry.kind.clone())
    }

    /// The node showing `path`: a folder inside a compact chain shows as the chain's head.
    fn node_id_for(&self, path: &str) -> Rc<str> {
        let mut current = String::new();
        for segment in path.split('/') {
            let candidate = if current.is_empty() {
                segment.to_owned()
            } else {
                format!("{current}/{segment}")
            };
            if let Some(child) = self.known_directory(&candidate)
                && candidate != path
            {
                let (members, _) = self.chain(child);
                if let Some(head) = members.first()
                    && members.iter().any(|member| &*member.path == path)
                {
                    return head.path.clone();
                }
            }
            current = candidate;
        }
        Rc::from(path)
    }

    pub fn view(&self) -> FileTreeView {
        let query = normalize_search_query(&self.query);
        let ready = self.directories.contains_key("");
        let pending = !self.in_flight.is_empty() || !self.queued.is_empty();
        let error = self
            .errors
            .iter()
            .find(|(path, _)| self.reachable(path))
            .map(|(_, problem)| problem.clone());
        let mut view = FileTreeView {
            ready,
            pending,
            error,
            has_directories: self.directory_count > 0,
            expand_all: self.expand_all,
            has_entries: self
                .directories
                .get("")
                .is_some_and(|root| !root.is_empty()),
            ..FileTreeView::default()
        };
        view.search = SearchView {
            active: !self.query.trim().is_empty(),
            pending: self.search.pending,
            truncated: self.search.truncated,
            error: self.search.error.clone(),
            no_matches: false,
        };
        if query.is_empty() {
            let mut expanded = Vec::new();
            let mut collapsed_expandable = false;
            let nodes = self.build("", &mut expanded, &mut collapsed_expandable);
            view.all_expanded = self.directory_count > 0 && !collapsed_expandable;
            view.nodes = Rc::new(nodes);
            view.expanded = Rc::new(expanded);
        } else {
            let (nodes, expanded, matched) = self.build_search(&query);
            view.search.no_matches = !matched;
            if matched {
                view.nodes = Rc::new(nodes);
                view.expanded = Rc::new(expanded);
            } else {
                let mut expanded = Vec::new();
                let nodes = self.build("", &mut expanded, &mut false);
                view.nodes = Rc::new(nodes);
                view.expanded = Rc::new(expanded);
            }
        }
        view
    }

    /// Only expanded folders are walked: the work is the rows the tree can show, never the
    /// whole repository.
    fn build(
        &self,
        directory: &str,
        expanded: &mut Vec<Rc<str>>,
        collapsed_expandable: &mut bool,
    ) -> Vec<FileTreeNode> {
        let Some(listing) = self.directories.get(directory) else {
            return Vec::new();
        };
        listing
            .iter()
            .map(|child| {
                if !child.directory {
                    return FileTreeNode {
                        id: child.path.clone(),
                        path: child.path.clone(),
                        label: Rc::from(child.name()),
                        kind: ProjectEntryKind::File,
                        ignored: child.ignored,
                        loading: false,
                        children: None,
                    };
                }
                let (members, tail) = self.chain(child);
                let label = members
                    .iter()
                    .map(|member| member.name())
                    .collect::<Vec<_>>()
                    .join("/");
                let open = self.expanded.contains(&*child.path);
                if !open && !child.ignored {
                    *collapsed_expandable = true;
                }
                let listed = self.directories.contains_key(&*tail.path);
                let children = (open && listed).then(|| {
                    expanded.push(child.path.clone());
                    self.build(&tail.path, expanded, collapsed_expandable)
                });
                let loading = open
                    && members
                        .iter()
                        .any(|member| self.loading.contains(&*member.path));
                if open && !listed {
                    expanded.push(child.path.clone());
                }
                FileTreeNode {
                    id: child.path.clone(),
                    path: tail.path.clone(),
                    label: Rc::from(label),
                    kind: ProjectEntryKind::Directory,
                    ignored: child.ignored,
                    loading,
                    children,
                }
            })
            .collect()
    }

    /// `hide-non-matches`: listed and searched paths containing the query, with their
    /// ancestors, every folder open.
    fn build_search(&self, query: &str) -> (Vec<FileTreeNode>, Vec<Rc<str>>, bool) {
        let mut kinds: BTreeMap<Rc<str>, (bool, bool)> = BTreeMap::new();
        for listing in self.directories.values() {
            for child in listing.iter() {
                kinds.insert(child.path.clone(), (child.directory, child.ignored));
            }
        }
        if !self.search.pending {
            for entry in self.search.entries.iter() {
                let directory = entry.kind == ProjectEntryKind::Directory;
                let ignored = entry.ignored.flatten() == Some(true);
                kinds
                    .entry(Rc::from(entry.path.0.as_str()))
                    .or_insert((directory, ignored));
                let segments: Vec<&str> = entry.path.0.split('/').collect();
                for count in 1..segments.len() {
                    kinds
                        .entry(Rc::from(segments[..count].join("/")))
                        .or_insert((true, false));
                }
            }
        }
        let mut visible: BTreeSet<Rc<str>> = BTreeSet::new();
        let mut matched = false;
        for (path, (directory, _)) in &kinds {
            let tree_path = if *directory {
                format!("{path}/")
            } else {
                path.to_string()
            };
            if !tree_path.to_lowercase().contains(query) {
                continue;
            }
            matched = true;
            visible.insert(path.clone());
            let segments: Vec<&str> = path.split('/').collect();
            for count in 1..segments.len() {
                visible.insert(Rc::from(segments[..count].join("/")));
            }
        }
        let mut by_parent: BTreeMap<&str, Vec<Child>> = BTreeMap::new();
        for path in &visible {
            let (directory, ignored) = kinds.get(path).copied().unwrap_or((true, false));
            by_parent.entry(parent_of(path)).or_default().push(Child {
                name_start: path.rfind('/').map_or(0, |index| index + 1),
                path: path.clone(),
                directory,
                ignored,
            });
        }
        for children in by_parent.values_mut() {
            children.sort_by(compare_children);
        }
        fn nest(
            by_parent: &BTreeMap<&str, Vec<Child>>,
            directory: &str,
            expanded: &mut Vec<Rc<str>>,
        ) -> Vec<FileTreeNode> {
            by_parent
                .get(directory)
                .map(|children| {
                    children
                        .iter()
                        .map(|child| {
                            let children = child.directory.then(|| {
                                expanded.push(child.path.clone());
                                nest(by_parent, &child.path, expanded)
                            });
                            FileTreeNode {
                                id: child.path.clone(),
                                path: child.path.clone(),
                                label: Rc::from(child.name()),
                                kind: if child.directory {
                                    ProjectEntryKind::Directory
                                } else {
                                    ProjectEntryKind::File
                                },
                                ignored: child.ignored,
                                loading: false,
                                children,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default()
        }
        let mut expanded = Vec::new();
        let nodes = nest(&by_parent, "", &mut expanded);
        (nodes, expanded, matched)
    }

    /// A file's kind as the tree knows it (only files open on selection).
    pub fn is_file(&self, path: &str) -> bool {
        self.directories
            .get(parent_of(path))
            .and_then(|listing| listing.iter().find(|child| &*child.path == path))
            .map(|child| !child.directory)
            .or_else(|| {
                self.search_entry_kind(path)
                    .map(|kind| kind == ProjectEntryKind::File)
            })
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use t3_contracts::base_schemas::TrimmedNonEmptyString;

    fn entry(path: &str, directory: bool) -> ProjectEntry {
        ProjectEntry {
            path: TrimmedNonEmptyString(path.into()),
            kind: if directory {
                ProjectEntryKind::Directory
            } else {
                ProjectEntryKind::File
            },
            ignored: None,
        }
    }
    fn ignored(path: &str) -> ProjectEntry {
        ProjectEntry {
            ignored: Some(Some(true)),
            ..entry(path, true)
        }
    }
    fn labels(nodes: &[FileTreeNode]) -> Vec<String> {
        nodes.iter().map(|n| n.label.to_string()).collect()
    }
    fn drain(model: &mut TreeModel) -> Vec<String> {
        std::iter::from_fn(|| model.next_start()).collect()
    }

    #[test]
    fn root_read_starts_at_once_and_children_are_filtered_and_sorted() {
        let mut model = TreeModel::new(vec![]);
        assert_eq!(drain(&mut model), vec![""]);
        assert!(!model.view().ready);
        model.complete(
            "",
            Ok(vec![
                entry("b.txt", false),
                entry("src", true),
                entry("src/nested.rs", false),
                entry("file10", false),
                entry("File2", false),
                entry("Docs", true),
            ]),
        );
        let view = model.view();
        assert!(view.ready && !view.pending);
        assert_eq!(
            labels(&view.nodes),
            ["Docs", "src", "b.txt", "File2", "file10"]
        );
    }

    #[test]
    fn at_most_four_reads_run_and_a_refresh_during_a_read_follows_it() {
        let mut model = TreeModel::new(vec![]);
        drain(&mut model);
        model.complete(
            "",
            Ok((0..6).map(|i| entry(&format!("d{i}"), true)).collect()),
        );
        model.set_expanded(&(0..6).map(|i| format!("d{i}")).collect::<Vec<_>>());
        assert_eq!(drain(&mut model).len(), MAX_CONCURRENT_LISTS);
        model.request("d0", true);
        model.request("d0", true);
        model.complete("d0", Ok(vec![]));
        let next = drain(&mut model);
        assert_eq!(
            next,
            vec!["d4"],
            "one slot freed; the follow-up queues behind the others"
        );
        model.complete("d1", Ok(vec![]));
        assert_eq!(drain(&mut model), vec!["d5"]);
        model.complete("d2", Ok(vec![]));
        assert_eq!(
            drain(&mut model),
            vec!["d0"],
            "the refresh runs once after its read"
        );
        assert_eq!(model.lists_started, 8);
    }

    #[test]
    fn a_loaded_folder_is_not_read_again_unless_refreshed() {
        let mut model = TreeModel::new(vec![]);
        drain(&mut model);
        model.complete("", Ok(vec![entry("a", true)]));
        model.request("", false);
        assert!(drain(&mut model).is_empty());
        model.refresh();
        assert_eq!(drain(&mut model), vec![""]);
        assert!(!model.view().ready || model.view().pending);
    }

    #[test]
    fn collapsed_folders_are_not_walked_and_expanding_reads_them() {
        let mut model = TreeModel::new(vec![]);
        drain(&mut model);
        model.complete("", Ok(vec![entry("a", true), entry("b", true)]));
        assert!(model.view().nodes[0].children.is_none());
        model.set_expanded(&["a"]);
        assert_eq!(drain(&mut model), vec!["a"]);
        let view = model.view();
        assert!(
            view.nodes[0].loading,
            "an expanded folder shows Pierre's loading row state"
        );
        model.complete("a", Ok(vec![entry("a/x.rs", false), entry("a/y", true)]));
        let view = model.view();
        assert_eq!(
            labels(view.nodes[0].children.as_ref().unwrap()),
            ["y", "x.rs"]
        );
        assert_eq!(&*view.expanded[0], "a");
    }

    #[test]
    fn compact_chains_show_as_one_row_and_read_every_member() {
        let mut model = TreeModel::new(vec![]);
        drain(&mut model);
        model.complete("", Ok(vec![entry("src", true)]));
        model.set_expanded(&["src"]);
        assert_eq!(drain(&mut model), vec!["src"]);
        model.complete("src", Ok(vec![entry("src/main", true)]));
        assert_eq!(
            drain(&mut model),
            vec!["src/main"],
            "a chain member is read with its head"
        );
        model.complete("src/main", Ok(vec![entry("src/main/lib.rs", false)]));
        let view = model.view();
        assert_eq!(labels(&view.nodes), ["src/main"]);
        assert_eq!(&*view.nodes[0].id, "src");
        assert_eq!(&*view.nodes[0].path, "src/main");
        assert_eq!(labels(view.nodes[0].children.as_ref().unwrap()), ["lib.rs"]);
    }

    #[test]
    fn errors_are_kept_per_folder_and_retried_on_a_new_session() {
        let mut model = TreeModel::new(vec![]);
        drain(&mut model);
        model.complete("", Err(Problem::server("boom".into())));
        assert_eq!(model.view().error, Some(Problem::server("boom".into())));
        model.retry_errors();
        assert_eq!(drain(&mut model), vec![""]);
        model.complete("", Ok(vec![]));
        assert_eq!(model.view().error, None);
    }

    #[test]
    fn unreachable_folder_errors_are_not_shown() {
        let mut model = TreeModel::new(vec![]);
        drain(&mut model);
        model.complete("", Ok(vec![entry("a", true)]));
        model.set_expanded(&["a"]);
        drain(&mut model);
        model.complete("a", Err(Problem::server("denied".into())));
        assert!(model.view().error.is_some());
        model.refresh();
        drain(&mut model);
        model.complete("", Ok(vec![]));
        assert_eq!(
            model.view().error,
            None,
            "a folder removed from the tree no longer reports"
        );
    }

    #[test]
    fn expand_all_skips_ignored_folders_and_follows_new_listings() {
        let mut model = TreeModel::new(vec![]);
        drain(&mut model);
        model.complete(
            "",
            Ok(vec![
                entry("a", true),
                ignored("node_modules"),
                entry("f", false),
            ]),
        );
        assert!(!model.view().all_expanded);
        model.toggle_all();
        assert_eq!(drain(&mut model), vec!["a"]);
        model.complete("a", Ok(vec![entry("a/b", true), entry("a/c.rs", false)]));
        assert_eq!(
            drain(&mut model),
            vec!["a/b"],
            "expand-all opens folders as they are listed"
        );
        model.complete("a/b", Ok(vec![]));
        let view = model.view();
        assert!(view.all_expanded && view.expand_all);
        model.set_expanded(&[] as &[&str]);
        assert!(
            !model.view().expand_all,
            "collapsing by hand ends expand-all"
        );
    }

    #[test]
    fn collapse_all_closes_every_folder() {
        let mut model = TreeModel::new(vec![]);
        drain(&mut model);
        model.complete("", Ok(vec![entry("a", true)]));
        model.toggle_all();
        drain(&mut model);
        model.complete("a", Ok(vec![]));
        model.toggle_all();
        assert!(model.view().expanded.is_empty());
    }

    #[test]
    fn restored_folders_reopen_once_listed_and_are_persisted_meanwhile() {
        let mut model = TreeModel::new(vec!["a".into(), "a/b".into()]);
        assert_eq!(model.persisted_expansion(), ["a", "a/b"]);
        drain(&mut model);
        model.complete("", Ok(vec![entry("a", true), entry("z", true)]));
        assert_eq!(drain(&mut model), vec!["a"]);
        model.complete("a", Ok(vec![entry("a/b", true), entry("a/c", false)]));
        assert_eq!(drain(&mut model), vec!["a/b"]);
        assert_eq!(model.persisted_expansion(), ["a", "a/b"]);
    }

    #[test]
    fn expansion_storage_keeps_twenty_roots_most_recent_first() {
        let mut stored: Option<Vec<u8>> = None;
        for index in 0..25 {
            stored = Some(write_expanded_directories(
                stored.as_deref(),
                &format!("root{index}"),
                vec!["a".into()],
            ));
        }
        let roots = stored_roots(stored.as_deref());
        assert_eq!(roots.len(), MAX_STORED_ROOTS);
        assert_eq!(roots[0].root, "root24");
        assert_eq!(
            read_expanded_directories(stored.as_deref(), "root24"),
            ["a"]
        );
        let cleared = write_expanded_directories(stored.as_deref(), "root24", vec![]);
        assert!(read_expanded_directories(Some(&cleared), "root24").is_empty());
        assert!(read_expanded_directories(Some(b"not json"), "x").is_empty());
    }

    #[test]
    fn search_hides_non_matches_and_merges_server_results_with_ancestors() {
        let mut model = TreeModel::new(vec![]);
        drain(&mut model);
        model.complete("", Ok(vec![entry("src", true), entry("readme.md", false)]));
        let generation = model.set_query("Lib".into(), true).unwrap();
        assert_eq!(model.search_input(generation).as_deref(), Some("Lib"));
        assert!(model.view().search.pending);
        model.complete_search(
            generation,
            Ok((vec![entry("src/deep/lib.rs", false)], true)),
        );
        let view = model.view();
        assert!(!view.search.no_matches && view.search.truncated);
        assert_eq!(labels(&view.nodes), ["src"]);
        let src = &view.nodes[0];
        assert_eq!(labels(src.children.as_ref().unwrap()), ["deep"]);
        assert!(view.expanded.iter().any(|id| &**id == "src/deep"));
        assert!(model.is_file("src/deep/lib.rs"));
    }

    #[test]
    fn a_search_with_no_matches_keeps_the_tree_and_says_so() {
        let mut model = TreeModel::new(vec![]);
        drain(&mut model);
        model.complete("", Ok(vec![entry("a.rs", false)]));
        let generation = model.set_query("zzz".into(), true).unwrap();
        model.complete_search(generation, Ok((vec![], false)));
        let view = model.view();
        assert!(view.search.no_matches);
        assert_eq!(labels(&view.nodes), ["a.rs"]);
    }

    #[test]
    fn stale_search_replies_are_ignored() {
        let mut model = TreeModel::new(vec![]);
        let first = model.set_query("a".into(), true).unwrap();
        let second = model.set_query("ab".into(), true).unwrap();
        assert_eq!(model.search_input(first), None);
        model.complete_search(first, Ok((vec![entry("x", false)], false)));
        assert!(model.view().search.pending);
        model.complete_search(second, Ok((vec![], false)));
        assert!(!model.view().search.pending);
        assert_eq!(model.set_query("   ".into(), true), None);
    }

    #[test]
    fn query_is_bounded_to_256_utf16_units_without_splitting_pairs() {
        let long = format!("{}😀", "a".repeat(255));
        assert_eq!(bounded_search_query(&long), "a".repeat(255));
        assert_eq!(normalize_search_query("  Src\\Lib "), "src/lib");
    }

    #[test]
    fn reveal_opens_ancestors_once_and_skips_tree_selections() {
        let mut model = TreeModel::new(vec![]);
        drain(&mut model);
        model.load_ancestors("a/b/c.rs");
        model.complete("", Ok(vec![entry("a", true), entry("a2", true)]));
        assert_eq!(
            model.reveal(Some("a/b/c.rs"), 1),
            None,
            "unknown until its folder is listed"
        );
        let reads = drain(&mut model);
        assert_eq!(reads, vec!["a", "a/b"]);
        model.complete("a", Ok(vec![entry("a/b", true), entry("a/z", false)]));
        model.complete("a/b", Ok(vec![entry("a/b/c.rs", false)]));
        model.set_query("q".into(), true);
        let reveal = model.reveal(Some("a/b/c.rs"), 1).unwrap();
        assert_eq!(&*reveal.id, "a/b/c.rs");
        assert_eq!(model.query(), "", "an external reveal closes the search");
        assert!(model.view().expanded.iter().any(|id| &**id == "a"));
        assert_eq!(
            model.reveal(Some("a/b/c.rs"), 1),
            None,
            "handled reveals do not replay"
        );
        model.select_from_tree("a/z");
        assert_eq!(
            model.reveal(Some("a/z"), 2),
            None,
            "a tree selection is already visible"
        );
        assert!(model.reveal(Some("a/b/c.rs"), 3).is_some());
    }

    #[test]
    fn mutation_tokens_refresh_once_each() {
        let mut model = TreeModel::new(vec![]);
        drain(&mut model);
        model.complete("", Ok(vec![]));
        assert!(!model.observe_mutation(None));
        model.observe_mutation(Some("m1"));
        assert_eq!(drain(&mut model), vec![""]);
        model.complete("", Ok(vec![]));
        model.observe_mutation(Some("m1"));
        assert!(drain(&mut model).is_empty());
    }

    #[test]
    fn view_work_follows_shown_rows_not_the_repository() {
        let mut model = TreeModel::new(vec![]);
        drain(&mut model);
        let big: Vec<ProjectEntry> = (0..5_000)
            .map(|i| entry(&format!("big/f{i}"), false))
            .collect();
        model.complete("", Ok(vec![entry("big", true), entry("small", true)]));
        model.set_expanded(&["big"]);
        drain(&mut model);
        model.complete("big", Ok(big));
        model.set_expanded(&[] as &[&str]);
        let view = model.view();
        assert!(view.nodes.iter().all(|node| node.children.is_none()));
        assert_eq!(
            view.nodes.len(),
            2,
            "a collapsed folder's 5,000 rows are not built"
        );
    }
}
