use crate::{Error, Result, blocking, paths};
use fff_search::{
    FilePicker, FilePickerOptions, FuzzySearchOptions, MixedItemRef, PaginationArgs, QueryParser,
    SharedFilePicker, SharedFrecency,
};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};
use t3_contracts::{base_schemas::TrimmedNonEmptyString as Text, project::*};
use tokio::{sync::Mutex, time::Instant};

pub const MAX_ENTRIES: usize = 25_000;
const PAGE_CAP: usize = MAX_ENTRIES + 2;
const SCAN_TIMEOUT: Duration = Duration::from_secs(15);
const IDLE_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum IndexVariant {
    Paths,
    Content,
}

pub(crate) async fn git(
    cwd: &Path,
    args: &[&str],
    stdin: Option<Vec<u8>>,
    limit: usize,
) -> Result<t3_process::ProcessOutput> {
    t3_git::Git::new(cwd)
        .execute(
            args,
            t3_git::ExecuteOptions {
                stdin,
                timeout: Some(Duration::from_secs(10)),
                max_output_bytes: limit,
                output_mode: t3_process::OutputMode::Truncate,
                allow_nonzero: true,
                ..Default::default()
            },
        )
        .await
        .map_err(|error| Error::Search(error.to_string()))
}

async fn ignored_paths(cwd: &Path) -> HashSet<String> {
    match git(
        cwd,
        &[
            "-c",
            "core.fsmonitor=false",
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "-z",
        ],
        None,
        16 * 1024 * 1024,
    )
    .await
    {
        Ok(out) if out.code == Some(0) && !out.stdout_truncated => out
            .stdout_text()
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(|s| normalize_path(s).trim_end_matches('/').into())
            .collect(),
        _ => HashSet::new(),
    }
}
fn normalize_path(s: &str) -> String {
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s.into()
    }
}
fn ignored(set: &HashSet<String>, path: &str) -> bool {
    let mut candidate = path.trim_end_matches('/');
    loop {
        if set.contains(candidate) {
            return true;
        }
        match candidate.rsplit_once('/') {
            Some((parent, _)) => candidate = parent,
            None => return false,
        }
    }
}
pub(crate) fn entry(path: String, kind: ProjectEntryKind) -> ProjectEntry {
    ProjectEntry {
        path: Text(path),
        kind,
        ignored: None,
    }
}
pub fn is_image_path(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|ext| {
        matches!(
            ext.to_string_lossy().to_lowercase().as_str(),
            "svg"
                | "ico"
                | "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "avif"
                | "bmp"
                | "heic"
                | "heif"
                | "tif"
                | "tiff"
                | "icns"
        )
    })
}

pub struct SearchIndex {
    cwd: PathBuf,
    picker: SharedFilePicker,
    frecency: SharedFrecency,
    ignored: RwLock<HashSet<String>>,
    incomplete: bool,
    scope: crate::scoped::ScopedDir,
    internal: paths::InternalPaths,
}
fn fff_error(e: impl std::fmt::Display) -> Error {
    Error::Search(e.to_string())
}
impl SearchIndex {
    pub async fn new(
        cwd: PathBuf,
        variant: IndexVariant,
        internal: paths::InternalPaths,
    ) -> Result<Arc<Self>> {
        Self::new_checked(cwd, variant, internal, None).await
    }
    async fn new_checked(
        cwd: PathBuf,
        variant: IndexVariant,
        internal: paths::InternalPaths,
        known: Option<&paths::KnownWorkspaces>,
    ) -> Result<Arc<Self>> {
        let scope = match known {
            Some(known) => crate::scoped::ScopedDir::open_authorized(&cwd, known)?,
            None => crate::scoped::ScopedDir::open(&cwd)?,
        };
        let cwd = scope.root.clone();
        let ignores = ignored_paths(&cwd).await;
        blocking(move || {
            let picker = SharedFilePicker::default();
            let frecency = SharedFrecency::default();
            FilePicker::new_with_shared_state(
                picker.clone(),
                frecency.clone(),
                FilePickerOptions {
                    base_path: cwd.to_string_lossy().into_owned(),
                    enable_mmap_cache: false,
                    enable_content_indexing: false,
                    mode: fff_search::FFFMode::Neovim,
                    ..Default::default()
                },
            )
            .map_err(fff_error)?;
            let complete = picker.wait_for_scan(SCAN_TIMEOUT);
            let index = Arc::new(Self {
                cwd,
                picker,
                frecency,
                ignored: RwLock::new(ignores),
                incomplete: !complete,
                scope,
                internal,
            });
            if !complete && variant == IndexVariant::Content {
                return Err(Error::ScanTimeout);
            }
            Ok(index)
        })
        .await
    }
    pub async fn refresh(self: &Arc<Self>) -> Result<()> {
        let this = self.clone();
        blocking(move || {
            this.picker
                .trigger_full_rescan_async(&this.frecency)
                .map_err(fff_error)?;
            this.picker.wait_for_scan(SCAN_TIMEOUT);
            Ok(())
        })
        .await?;
        let ignored = ignored_paths(&self.cwd).await;
        *self.ignored.write().map_err(fff_error)? = ignored;
        Ok(())
    }
    fn search_sync(
        &self,
        query: &str,
        limit: usize,
        kind: Option<ProjectEntryKind>,
        images: bool,
    ) -> Result<ProjectSearchEntriesResult> {
        let guard = self.picker.read().map_err(fff_error)?;
        let picker = guard
            .as_ref()
            .ok_or_else(|| Error::Search("index closed".into()))?;
        let ignored_set = self.ignored.read().map_err(fff_error)?;
        let requested = if images {
            PAGE_CAP
        } else {
            limit.saturating_add(1).max(1)
        };
        let mut size = requested;
        let (mut entries, total, fetched) = loop {
            let options = FuzzySearchOptions {
                project_path: Some(picker.base_path()),
                pagination: PaginationArgs {
                    offset: 0,
                    limit: size,
                },
                ..Default::default()
            };
            let (items, total) = if kind == Some(ProjectEntryKind::File) || images {
                let parser = QueryParser::default();
                let parsed = parser.parse(query);
                let result = picker.fuzzy_search(&parsed, None, options);
                (
                    result
                        .items
                        .iter()
                        .map(|i| {
                            entry(
                                normalize_path(&i.relative_path(picker)),
                                ProjectEntryKind::File,
                            )
                        })
                        .collect::<Vec<_>>(),
                    result.total_matched,
                )
            } else if kind == Some(ProjectEntryKind::Directory) {
                let parser = QueryParser::new(fff_search::DirSearchConfig);
                let parsed = parser.parse(query);
                let result = picker.fuzzy_search_directories(&parsed, options);
                (
                    result
                        .items
                        .iter()
                        .map(|i| {
                            entry(
                                normalize_path(&i.relative_path(picker))
                                    .trim_end_matches('/')
                                    .into(),
                                ProjectEntryKind::Directory,
                            )
                        })
                        .collect(),
                    result.total_matched,
                )
            } else {
                let parser = QueryParser::new(fff_search::MixedSearchConfig);
                let parsed = parser.parse(query);
                let result = picker.fuzzy_search_mixed(&parsed, None, options);
                (
                    result
                        .items
                        .iter()
                        .map(|i| match i {
                            MixedItemRef::File(f) => entry(
                                normalize_path(&f.relative_path(picker)),
                                ProjectEntryKind::File,
                            ),
                            MixedItemRef::Dir(d) => entry(
                                normalize_path(&d.relative_path(picker))
                                    .trim_end_matches('/')
                                    .into(),
                                ProjectEntryKind::Directory,
                            ),
                        })
                        .collect(),
                    result.total_matched,
                )
            };
            let fetched = items.len();
            let root_count = items.iter().filter(|e| e.path.0.is_empty()).count();
            let visible: Vec<_> = items
                .into_iter()
                .filter(|e| {
                    !ignored(&ignored_set, &e.path.0)
                        && self.safe_entry(&e.path.0, e.kind == ProjectEntryKind::File)
                })
                .collect();
            let total_visible = total
                .saturating_sub(fetched - visible.len())
                .saturating_sub(root_count);
            if visible.len() >= requested
                || fetched >= total
                || size >= PAGE_CAP
                || ignored_set.is_empty()
            {
                break (
                    visible
                        .into_iter()
                        .filter(|e| !e.path.0.is_empty())
                        .collect::<Vec<_>>(),
                    total_visible,
                    fetched,
                );
            }
            size = size.saturating_mul(4).min(PAGE_CAP);
        };
        if images {
            entries.retain(|e| is_image_path(&e.path.0));
        }
        let truncated = (self.incomplete && picker.is_scan_active())
            || entries.len() > limit
            || if images {
                total > fetched
            } else {
                total > limit
            };
        entries.truncate(limit);
        Ok(ProjectSearchEntriesResult { entries, truncated })
    }
    pub async fn search(
        self: &Arc<Self>,
        raw_query: String,
        limit: usize,
        kind: Option<ProjectEntryKind>,
        images: bool,
    ) -> Result<ProjectSearchEntriesResult> {
        paths::bound_text(&raw_query, 256, true, "entry query")?;
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid("entry search limit must be 1..=200".into()));
        }
        self.search_inner(raw_query, limit, kind, images).await
    }
    async fn search_inner(
        self: &Arc<Self>,
        raw_query: String,
        limit: usize,
        kind: Option<ProjectEntryKind>,
        images: bool,
    ) -> Result<ProjectSearchEntriesResult> {
        let this = self.clone();
        blocking(move || {
            let query = raw_query.replace('[', "");
            let mut result = this.search_sync(&query, limit, kind.clone(), images)?;
            if result.entries.is_empty() && query.contains('{') {
                result = this.search_sync(&query.replace('{', ""), limit, kind, images)?;
            }
            Ok(result)
        })
        .await
    }
    pub async fn list(self: &Arc<Self>) -> Result<ProjectListEntriesResult> {
        let result = self
            .search_inner(String::new(), MAX_ENTRIES, None, false)
            .await?;
        let mut entries: HashMap<String, ProjectEntry> = result
            .entries
            .into_iter()
            .map(|e| (e.path.0.clone(), e))
            .collect();
        for path in entries.keys().cloned().collect::<Vec<_>>() {
            let mut p = path.as_str();
            while let Some((parent, _)) = p.rsplit_once('/') {
                entries
                    .entry(parent.into())
                    .or_insert_with(|| entry(parent.into(), ProjectEntryKind::Directory));
                p = parent;
            }
        }
        let mut entries: Vec<_> = entries.into_values().collect();
        entries.sort_by(|a, b| crate::locale_cmp(&a.path.0, &b.path.0));
        let truncated = result.truncated || entries.len() > MAX_ENTRIES;
        entries.truncate(MAX_ENTRIES);
        Ok(ProjectListEntriesResult { entries, truncated })
    }
    fn safe_entry(&self, path: &str, file: bool) -> bool {
        let Ok(relative) = self.scope.resolve(path, false) else {
            return false;
        };
        if self.internal.contains(&self.scope.root.join(&relative)) {
            return false;
        }
        if file {
            self.scope.open_file(&relative).is_ok()
        } else {
            self.scope.read_dir(&relative).is_ok()
        }
    }
    pub async fn search_contents(
        self: &Arc<Self>,
        input: ProjectSearchContentsInput,
    ) -> Result<ProjectSearchContentsResult> {
        paths::bound_text(&input.query, 256, false, "content query")?;
        if !(1..=500).contains(&input.limit) {
            return Err(Error::Invalid(
                "content search limit must be 1..=500".into(),
            ));
        }
        let this = self.clone();
        blocking(move || {
            let guard = this.picker.read().map_err(fff_error)?;
            let picker = guard
                .as_ref()
                .ok_or_else(|| Error::Search("index closed".into()))?;
            let ignored_set = this.ignored.read().map_err(fff_error)?;
            let mut parsed = fff_search::grep::parse_grep_query(&input.query);
            let text = if parsed.constraints.is_empty() {
                input.query.clone()
            } else {
                parsed.grep_text()
            };
            parsed.fuzzy_query = fff_search::FuzzyQuery::Empty;
            let candidates = picker.fuzzy_search(
                &parsed,
                None,
                FuzzySearchOptions {
                    project_path: Some(picker.base_path()),
                    pagination: PaginationArgs {
                        offset: 0,
                        limit: MAX_ENTRIES,
                    },
                    ..Default::default()
                },
            );
            let pattern = if input.use_regex {
                text.clone()
            } else {
                regex::escape(&text)
            };
            let compiled = regex::RegexBuilder::new(&pattern)
                .case_insensitive(!input.case_sensitive)
                .build();
            let (regex, fallback) = match compiled {
                Ok(regex) => (regex, None),
                Err(error) => (
                    regex::RegexBuilder::new(&regex::escape(&text))
                        .case_insensitive(!input.case_sensitive)
                        .build()
                        .map_err(fff_error)?,
                    Some(Some(error.to_string())),
                ),
            };
            let deadline = std::time::Instant::now() + Duration::from_millis(250);
            let mut matches = Vec::new();
            let mut truncated = candidates.total_matched > MAX_ENTRIES;
            'files: for item in candidates.items {
                if std::time::Instant::now() >= deadline {
                    truncated = true;
                    break;
                }
                let path = normalize_path(&item.relative_path(picker));
                if ignored(&ignored_set, &path) {
                    continue;
                }
                let Ok(relative) = this.scope.resolve(&path, false) else {
                    continue;
                };
                if this.internal.contains(&this.scope.root.join(&relative)) {
                    continue;
                }
                // fff's grep reopens paths and mmap caches; only the validated descriptor may read content here.
                let Ok(file) = this.scope.open_file(&relative) else {
                    continue;
                };
                let mut bytes = Vec::new();
                if file
                    .take(crate::filesystem::READ_MAX_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .is_err()
                    || bytes.contains(&0)
                {
                    continue;
                }
                if bytes.len() > crate::filesystem::READ_MAX_BYTES as usize {
                    truncated = true;
                    continue;
                }
                let content = String::from_utf8_lossy(&bytes);
                let mut count = 0;
                for (line_index, line) in content.lines().enumerate() {
                    if std::time::Instant::now() >= deadline {
                        truncated = true;
                        break 'files;
                    }
                    let ranges: Vec<_> = regex
                        .find_iter(line)
                        .filter(|m| !input.whole_word || whole_word(line, m.start(), m.end()))
                        .map(|m| ProjectContentMatchRange {
                            start: line[..m.start()].encode_utf16().count() as u64,
                            end: line[..m.end()].encode_utf16().count() as u64,
                        })
                        .collect();
                    if ranges.is_empty() {
                        continue;
                    }
                    if matches.len() >= input.limit as usize {
                        truncated = true;
                        break 'files;
                    }
                    matches.push(ProjectContentMatch {
                        path: Text(path.clone()),
                        line_number: line_index as u64 + 1,
                        line_content: line.into(),
                        match_ranges: ranges,
                    });
                    count += 1;
                    if count >= 100 {
                        break;
                    }
                }
            }
            Ok(ProjectSearchContentsResult {
                matches,
                truncated,
                regex_fallback_error: fallback,
            })
        })
        .await
    }
    /// Wait for watcher readiness, then subscribe; callers receive real filesystem events.
    pub async fn watch(self: &Arc<Self>, pattern: String) -> Result<WatchSubscription> {
        paths::bound_text(&pattern, 512, false, "watch pattern")?;
        paths::resolve_relative(&self.cwd, &pattern)?;
        let relative = self.scope.resolve(&pattern, true)?;
        if paths::in_git_folder(&self.scope.root.join(relative))? {
            return Err(Error::Invalid("watch targets protected metadata".into()));
        }
        let this = self.clone();
        blocking(move || {
            if !this.picker.wait_for_watcher(SCAN_TIMEOUT) {
                return Err(Error::ScanTimeout);
            }
            let (sender, receiver) = watch_event_channel();
            let id = this
                .picker
                .watch(&pattern, Default::default(), move |_, events| {
                    let _ = sender.send(events.to_vec());
                })
                .map_err(fff_error)?;
            Ok(WatchSubscription {
                index: this,
                id,
                receiver,
            })
        })
        .await
    }
}
impl Drop for SearchIndex {
    fn drop(&mut self) {
        self.picker.cancel();
        let picker = self.picker.clone();
        let close = move || {
            picker.shutdown_watches_and_wait();
            if let Ok(mut guard) = picker.write()
                && let Some(mut picker) = guard.take()
            {
                picker.stop_background_monitor();
            }
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn_blocking(close);
        } else {
            close();
        }
    }
}
type WatchBatch = Vec<fff_search::WatchEvent>;
fn watch_event_channel() -> (
    tokio::sync::broadcast::Sender<WatchBatch>,
    tokio::sync::broadcast::Receiver<WatchBatch>,
) {
    tokio::sync::broadcast::channel(32)
}
pub struct WatchSubscription {
    index: Arc<SearchIndex>,
    id: fff_search::WatchId,
    pub receiver: tokio::sync::broadcast::Receiver<Vec<fff_search::WatchEvent>>,
}
impl Drop for WatchSubscription {
    fn drop(&mut self) {
        self.index.picker.unwatch(self.id);
    }
}

fn whole_word(line: &str, start: usize, end: usize) -> bool {
    if end <= start || !line.is_char_boundary(start) || !line.is_char_boundary(end) {
        return false;
    }
    let word =
        |c: Option<char>| c.is_some_and(|c| c == '_' || c.is_alphanumeric() || regex_word_mark(c));
    let before = line[..start].chars().next_back();
    let first = line[start..].chars().next();
    let last = line[..end].chars().next_back();
    let after = line[end..].chars().next();
    (!word(before) || !word(first)) && (!word(last) || !word(after))
}
fn regex_word_mark(c: char) -> bool {
    static MARK: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    MARK.get_or_init(|| regex::Regex::new(r"\p{M}").ok())
        .as_ref()
        .is_some_and(|r| r.is_match(c.encode_utf8(&mut [0; 4])))
}

type IndexCell = Arc<tokio::sync::OnceCell<Arc<SearchIndex>>>;
type CachedIndexes = HashMap<(PathBuf, IndexVariant), (IndexCell, Instant)>;
pub const MAX_INDEXES: usize = 16;
pub struct SearchIndexes {
    indexes: Mutex<CachedIndexes>,
    internal: paths::InternalPaths,
}
impl SearchIndexes {
    pub fn new(internal: paths::InternalPaths) -> Self {
        Self {
            indexes: Mutex::default(),
            internal,
        }
    }
    pub async fn get(&self, cwd: &Path, variant: IndexVariant) -> Result<Arc<SearchIndex>> {
        self.get_checked(cwd, variant, None).await
    }
    pub(crate) async fn get_authorized(
        &self,
        cwd: &Path,
        variant: IndexVariant,
        known: &paths::KnownWorkspaces,
    ) -> Result<Arc<SearchIndex>> {
        self.get_checked(cwd, variant, Some(known)).await
    }
    async fn get_checked(
        &self,
        cwd: &Path,
        variant: IndexVariant,
        known: Option<&paths::KnownWorkspaces>,
    ) -> Result<Arc<SearchIndex>> {
        let cwd = paths::canonicalize(cwd)
            .map_err(|e| crate::io_error("realpath-workspace-root", cwd, e))?;
        if known.is_some_and(|known| !known.contains(&cwd)) {
            return Err(Error::UnknownWorkspace(cwd));
        }
        self.get_using(&cwd, variant, |path, kind| {
            SearchIndex::new_checked(path, kind, self.internal.clone(), known)
        })
        .await
    }
    async fn get_using<F, Fut>(
        &self,
        cwd: &Path,
        variant: IndexVariant,
        build: F,
    ) -> Result<Arc<SearchIndex>>
    where
        F: FnOnce(PathBuf, IndexVariant) -> Fut,
        Fut: std::future::Future<Output = Result<Arc<SearchIndex>>>,
    {
        let key = (paths::absolute(cwd)?, variant);
        let (cell, evicted) = {
            let mut map = self.indexes.lock().await;
            let keys: Vec<_> = map
                .iter()
                .filter(|(_, (cell, last))| {
                    last.elapsed() >= IDLE_TTL
                        && Arc::strong_count(cell) == 1
                        && cell.get().is_none_or(|index| Arc::strong_count(index) == 1)
                })
                .map(|(key, _)| key.clone())
                .collect();
            let mut evicted = Vec::new();
            for key in keys {
                if let Some(value) = map.remove(&key) {
                    evicted.push(value);
                }
            }
            if !map.contains_key(&key) && map.len() >= MAX_INDEXES {
                // Active subscriptions and in-flight initializations cannot be reclaimed.
                let oldest = map
                    .iter()
                    .filter(|(_, (cell, _))| {
                        Arc::strong_count(cell) == 1
                            && cell.get().is_none_or(|i| Arc::strong_count(i) == 1)
                    })
                    .min_by_key(|(_, (_, last))| *last)
                    .map(|(key, _)| key.clone());
                if let Some(oldest) = oldest {
                    if let Some(value) = map.remove(&oldest) {
                        evicted.push(value);
                    }
                } else {
                    return Err(Error::Search("workspace index capacity reached".into()));
                }
            }
            let (cell, last) = map
                .entry(key.clone())
                .or_insert_with(|| (Arc::new(tokio::sync::OnceCell::new()), Instant::now()));
            *last = Instant::now();
            (cell.clone(), evicted)
        };
        tokio::task::spawn_blocking(move || drop(evicted));
        let result = cell.get_or_try_init(|| build(key.0.clone(), variant)).await;
        match result {
            Ok(index) => Ok(index.clone()),
            Err(error) => {
                let mut map = self.indexes.lock().await;
                if map
                    .get(&key)
                    .is_some_and(|(current, _)| Arc::ptr_eq(current, &cell))
                {
                    map.remove(&key);
                }
                Err(error)
            }
        }
    }
    pub async fn refresh(&self, cwd: &Path) -> Result<()> {
        let cwd = paths::canonicalize(cwd).unwrap_or(paths::absolute(cwd)?);
        let indexes: Vec<_> = self
            .indexes
            .lock()
            .await
            .iter()
            .filter(|((path, _), _)| *path == cwd)
            .filter_map(|(_, (cell, _))| cell.get().cloned())
            .collect();
        for index in indexes {
            if index.refresh().await.is_err() {
                let removed = {
                    let mut map = self.indexes.lock().await;
                    let key = (cwd.clone(), IndexVariant::Paths);
                    if map
                        .get(&key)
                        .is_some_and(|(cell, _)| cell.get().is_some_and(|i| Arc::ptr_eq(i, &index)))
                    {
                        map.remove(&key)
                    } else {
                        let key = (cwd.clone(), IndexVariant::Content);
                        map.remove(&key)
                    }
                };
                tokio::task::spawn_blocking(move || drop(removed));
            }
        }
        Ok(())
    }
    pub async fn evict_idle(&self) {
        let removed = {
            let mut map = self.indexes.lock().await;
            let keys: Vec<_> = map
                .iter()
                .filter(|(_, (cell, last))| {
                    last.elapsed() >= IDLE_TTL
                        && Arc::strong_count(cell) == 1
                        && cell.get().is_none_or(|index| Arc::strong_count(index) == 1)
                })
                .map(|(key, _)| key.clone())
                .collect();
            keys.into_iter()
                .filter_map(|key| map.remove(&key))
                .collect::<Vec<_>>()
        };
        tokio::task::spawn_blocking(move || drop(removed));
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    #[test]
    fn a_stalled_watcher_retains_at_most_32_batches() {
        let (sender, mut receiver) = watch_event_channel();
        for _ in 0..128 {
            sender
                .send(vec![fff_search::WatchEvent {
                    path: PathBuf::from("file"),
                    kind: fff_search::WatchEventKind::Modified,
                    from: None,
                }])
                .expect("subscriber");
        }
        assert!(
            sender.len() <= 32,
            "stalled subscriber retained {} batches",
            sender.len()
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(96))
        ));
        assert_eq!(receiver.len(), 32);
    }

    #[tokio::test]
    async fn index_capacity_includes_in_flight_builds_and_recovers_after_cancellation() {
        let temp = tempfile::tempdir().expect("temp");
        let indexes = SearchIndexes::new(paths::InternalPaths {
            state_dir: temp.path().join("state"),
            secrets_dir: temp.path().join("secrets"),
        });
        let roots: Vec<_> = (0..MAX_INDEXES)
            .map(|i| temp.path().join(i.to_string()))
            .collect();
        let mut pending = Vec::new();
        for root in &roots {
            let mut build = Box::pin(
                indexes.get_using(root, IndexVariant::Paths, |_, _| std::future::pending()),
            );
            assert!(
                std::future::poll_fn(|cx| std::task::Poll::Ready(
                    build.as_mut().poll(cx).is_pending()
                ))
                .await
            );
            pending.push(build);
        }
        let extra = temp.path().join("extra");
        let result = indexes
            .get_using(&extra, IndexVariant::Paths, |_, _| std::future::pending())
            .await;
        assert!(matches!(result,Err(Error::Search(message)) if message.contains("capacity")));
        drop(pending);
        let result = indexes
            .get_using(&extra, IndexVariant::Paths, |_, _| async {
                Err(Error::Search("build ran".into()))
            })
            .await;
        assert!(matches!(result,Err(Error::Search(message)) if message=="build ran"));
    }
    #[tokio::test]
    async fn blocked_index_build_does_not_block_another_workspace_save() {
        let temp = tempfile::tempdir().expect("temp");
        let first = temp.path().join("first");
        let other = temp.path().join("other");
        let indexes = SearchIndexes::new(paths::InternalPaths {
            state_dir: temp.path().join("state"),
            secrets_dir: temp.path().join("secrets"),
        });
        let (started, ready) = tokio::sync::oneshot::channel();
        let (_release, gate) = tokio::sync::oneshot::channel::<()>();
        let build = |_, _| async {
            let _ = started.send(());
            let _ = gate.await;
            Err(Error::Search("gated".into()))
        };
        let mut pending = Box::pin(indexes.get_using(&first, IndexVariant::Paths, build));
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(
                pending.as_mut().poll(cx).is_pending()
            ))
            .await
        );
        ready.await.expect("build started");
        let mut save = Box::pin(indexes.refresh(&other));
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(save.as_mut().poll(cx).is_ready()))
                .await,
            "save blocked behind another workspace build"
        );
    }

    #[tokio::test]
    async fn git_rejects_implicit_bare_repository_config() {
        let tmp = tempfile::tempdir().expect("temp");
        let root = tmp.path();
        let mut opts = t3_process::ProcessOptions::new("git").args([
            "init",
            "--bare",
            root.to_str().expect("path"),
        ]);
        opts.env
            .insert("GIT_CONFIG_NOSYSTEM".into(), Some("1".into()));
        opts.env.insert(
            "GIT_CONFIG_GLOBAL".into(),
            Some(root.join("no-global").into_os_string()),
        );
        t3_process::run(opts).await.expect("git init");
        let output = git(root, &["worktree", "list", "--porcelain"], None, 1024)
            .await
            .expect("git");
        assert_ne!(output.code, Some(0), "implicit bare repository accepted");
    }
}
