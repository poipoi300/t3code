use crate::{
    Error, Result, blocking,
    ignore_patterns::{IgnoreDecision, IgnoreFile},
    paths,
    search::{self, SearchIndex, entry},
};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    io::Read,
    path::Path,
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use t3_contracts::project::{ProjectEntry, ProjectEntryKind};
use tokio::{
    sync::{Mutex, watch},
    time::Instant,
};

#[derive(Default)]
pub(crate) struct SymlinkScan {
    pub entries: Vec<ProjectEntry>,
    pub paths: HashSet<String>,
    pub complete: bool,
}
type ScanResult = std::result::Result<Arc<SymlinkScan>, String>;
#[derive(Default)]
struct State {
    last: Option<Arc<SymlinkScan>>,
    generation: u64,
    expires: Option<Instant>,
    pending: Option<watch::Receiver<Option<ScanResult>>>,
}
#[derive(Default)]
pub(crate) struct SymlinkCache {
    state: Mutex<State>,
    generation: AtomicU64,
    known: RwLock<HashSet<String>>,
    pub watcher: std::sync::Mutex<Option<fff_search::WatchId>>,
}
impl SymlinkCache {
    pub fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
    pub fn on_events(&self, root: &Path, events: &[fff_search::WatchEvent]) {
        let known = self
            .known
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if events.iter().any(|event| {
            event.kind != fff_search::WatchEventKind::Modified
                || event
                    .path
                    .file_name()
                    .is_some_and(|name| name == ".ignore" || name == ".gitignore")
                || event
                    .path
                    .strip_prefix(root)
                    .is_ok_and(|p| known.contains(&paths::display_path(p)))
        }) {
            self.invalidate();
        }
    }
    pub async fn read<F, Fut>(self: &Arc<Self>, watched: bool, scan: F) -> Result<Arc<SymlinkScan>>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<SymlinkScan>> + Send + 'static,
    {
        let mut state = self.state.lock().await;
        let generation = self.generation.load(Ordering::Relaxed);
        if state.pending.is_none()
            && (state.last.is_none()
                || state.generation != generation
                || state.expires.is_some_and(|at| Instant::now() >= at))
        {
            let (sender, receiver) = watch::channel(None);
            state.pending = Some(receiver);
            let cache = self.clone();
            tokio::spawn(async move {
                let result = scan().await.map(Arc::new).map_err(|e| e.to_string());
                let mut state = cache.state.lock().await;
                if let Ok(scan) = &result {
                    *cache
                        .known
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = scan.paths.clone();
                    state.last = Some(scan.clone());
                    state.generation = generation;
                    state.expires = (!watched || !scan.complete)
                        .then(|| Instant::now() + Duration::from_secs(2));
                }
                state.pending = None;
                sender.send_replace(Some(result));
            });
        }
        let last = state.last.clone();
        let pending = state.pending.clone();
        drop(state);
        let Some(mut pending) = pending else {
            return last.ok_or_else(|| Error::Search("missing symlink scan".into()));
        };
        let wait = async {
            pending
                .wait_for(Option::is_some)
                .await
                .map_err(|_| Error::Search("symlink scan closed".into()))?
                .as_ref()
                .cloned()
                .ok_or_else(|| Error::Search("missing symlink result".into()))?
                .map_err(Error::Search)
        };
        match last {
            None => wait.await,
            Some(last) => match tokio::time::timeout(Duration::from_millis(5), wait).await {
                Ok(result) => result,
                Err(_) => Ok(last),
            },
        }
    }
}

struct Directory {
    path: String,
    entries: Vec<ProjectEntry>,
    links: HashSet<String>,
    ignore: Option<IgnoreFile>,
    repository: bool,
}
fn scan_directory(index: &SearchIndex, path: String) -> Result<Directory> {
    let mut result = Directory {
        path: path.clone(),
        entries: vec![],
        links: HashSet::new(),
        ignore: None,
        repository: false,
    };
    // Opening the named directory without following ancestors never walks a linked tree.
    let Ok((_held, children)) = index.scope.read_dir(Path::new(&path)) else {
        return Ok(result);
    };
    let mut children: Vec<_> = children
        .take(search::MAX_ENTRIES)
        .filter_map(std::result::Result::ok)
        .collect();
    children.sort_by_key(|child| child.file_name());
    for child in children {
        let Some(name) = child.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let relative = if path.is_empty() {
            name.clone()
        } else {
            format!("{path}/{name}")
        };
        if paths::is_git_name(&name) {
            result.repository = true;
            continue;
        }
        let Ok(kind) = child.file_type() else {
            continue;
        };
        if name == ".ignore"
            && kind.is_file()
            && let Ok(file) = index.scope.open_file(Path::new(&relative))
        {
            let mut text = String::new();
            if file.take(1024 * 1024).read_to_string(&mut text).is_ok() {
                result.ignore = Some(IgnoreFile::parse(&text));
            }
        }
        let absolute = index.cwd.join(&relative);
        if !kind.is_symlink() && !is_reparse(&absolute) {
            continue;
        }
        result.links.insert(relative.clone());
        if paths::validate_host_path(&absolute).is_err() {
            continue;
        }
        let Ok(resolved) = index.scope.resolve(&relative, false) else {
            continue;
        };
        if index.internal.contains(&index.cwd.join(&resolved))
            || paths::in_git_folder(&index.cwd.join(&resolved)).unwrap_or(true)
        {
            continue;
        }
        let kind = if index.scope.open_file(&resolved).is_ok() {
            ProjectEntryKind::File
        } else if index.scope.read_dir(&resolved).is_ok() {
            ProjectEntryKind::Directory
        } else {
            continue;
        };
        result.entries.push(entry(relative, kind));
    }
    Ok(result)
}
fn is_reparse(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        std::fs::symlink_metadata(path).is_ok_and(|m| m.file_attributes() & 0x400 != 0)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}
fn within(path: &str, directory: &str) -> bool {
    directory.is_empty() || path.starts_with(&format!("{directory}/"))
}

pub(crate) async fn scan(index: Arc<SearchIndex>) -> Result<SymlinkScan> {
    let this = index.clone();
    let (directories, complete) = blocking(move || this.symlink_directories()).await?;
    let mut scanned = Vec::new();
    for batch in directories.chunks(16) {
        let mut workers = tokio::task::JoinSet::new();
        for directory in batch {
            let (this, path) = (index.clone(), directory.clone());
            workers.spawn_blocking(move || scan_directory(&this, path));
        }
        while let Some(result) = workers.join_next().await {
            scanned.push(result??);
        }
    }
    scanned.sort_by(|a, b| {
        b.path
            .len()
            .cmp(&a.path.len())
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut scan = SymlinkScan {
        complete,
        ..Default::default()
    };
    let mut groups: HashMap<String, Vec<ProjectEntry>> = HashMap::new();
    for directory in &scanned {
        scan.paths.extend(directory.links.iter().cloned());
        for entry in &directory.entries {
            let decision = scanned
                .iter()
                .filter(|d| within(&entry.path.0, &d.path))
                .find_map(|d| {
                    d.ignore.as_ref()?.decide(
                        if d.path.is_empty() {
                            &entry.path.0
                        } else {
                            entry.path.0.strip_prefix(&format!("{}/", d.path))?
                        },
                        false,
                    )
                });
            match decision {
                Some(IgnoreDecision::Ignored) => {}
                Some(IgnoreDecision::Included) => scan.entries.push(entry.clone()),
                None => {
                    let repository = scanned
                        .iter()
                        .find(|d| d.repository && within(&entry.path.0, &d.path))
                        .map(|d| d.path.clone())
                        .unwrap_or_default();
                    groups.entry(repository).or_default().push(entry.clone());
                }
            }
        }
    }
    for (repository, entries) in groups {
        let cwd = index.cwd.join(&repository);
        let mut stdin = Vec::new();
        for entry in &entries {
            let relative = if repository.is_empty() {
                &entry.path.0
            } else {
                entry
                    .path
                    .0
                    .strip_prefix(&format!("{repository}/"))
                    .unwrap_or(&entry.path.0)
            };
            stdin.extend_from_slice(relative.as_bytes());
            stdin.push(0);
        }
        let result = search::git(
            &cwd,
            &[
                "-c",
                "core.fsmonitor=false",
                "check-ignore",
                "--no-index",
                "-z",
                "--stdin",
            ],
            Some(stdin),
            16 * 1024 * 1024,
        )
        .await;
        let ignored = match result {
            Ok(out) if out.code == Some(1) => HashSet::new(),
            Ok(out) if out.code == Some(0) && !out.stdout_truncated => out
                .stdout_text()
                .split('\0')
                .filter(|s| !s.is_empty())
                .map(|s| {
                    if repository.is_empty() {
                        s.to_owned()
                    } else {
                        format!("{repository}/{s}")
                    }
                })
                .collect(),
            _ => {
                if search::git(&cwd, &["rev-parse", "--git-dir"], None, 4096)
                    .await
                    .is_ok_and(|out| out.code == Some(0))
                {
                    continue;
                } else {
                    HashSet::new()
                }
            }
        };
        scan.entries
            .extend(entries.into_iter().filter(|e| !ignored.contains(&e.path.0)));
    }
    scan.entries.sort_by(|a, b| a.path.0.cmp(&b.path.0));
    Ok(scan)
}

pub(crate) fn score(path: &str, query: &str) -> Option<usize> {
    let value: Vec<_> = path.to_lowercase().encode_utf16().collect();
    let query: Vec<_> = query.to_lowercase().encode_utf16().collect();
    if query.is_empty() {
        return Some(0);
    }
    let (mut first, mut previous, mut gaps, mut matched) = (None, None, 0, 0);
    for (i, ch) in value.iter().enumerate() {
        if ch != &query[matched] {
            continue;
        }
        first.get_or_insert(i);
        if let Some(previous) = previous {
            gaps += i - previous - 1;
        }
        previous = Some(i);
        matched += 1;
        if matched == query.len() {
            let first = first?;
            return Some(
                first * 2 + gaps * 3 + i - first + 1 - query.len()
                    + (value.len() - query.len()).min(64),
            );
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scan(path: &str) -> SymlinkScan {
        SymlinkScan {
            entries: vec![entry(path.into(), ProjectEntryKind::File)],
            paths: [path.into()].into_iter().collect(),
            complete: true,
        }
    }
    #[tokio::test(start_paused = true)]
    async fn coalesces_reads_and_keeps_stale_results_during_a_slow_rescan() {
        let cache = Arc::new(SymlinkCache::default());
        let (started_tx, started) = tokio::sync::oneshot::channel();
        let (gate_tx, gate) = tokio::sync::oneshot::channel();
        let first = tokio::spawn({
            let cache = cache.clone();
            async move {
                cache
                    .read(true, move || async move {
                        let _ = started_tx.send(());
                        gate.await.expect("gate");
                        Ok(scan("old-link"))
                    })
                    .await
                    .expect("first")
            }
        });
        started.await.expect("started");
        let concurrent = tokio::spawn({
            let cache = cache.clone();
            async move {
                cache
                    .read(true, || async {
                        panic!("concurrent read started another scan")
                    })
                    .await
                    .expect("concurrent")
            }
        });
        gate_tx.send(()).expect("release");
        let first = first.await.expect("join");
        let second = concurrent.await.expect("join");
        assert!(Arc::ptr_eq(&first, &second));
        cache.on_events(
            Path::new("root"),
            &[fff_search::WatchEvent {
                kind: fff_search::WatchEventKind::Modified,
                path: Path::new("root/ordinary.ts").into(),
                from: None,
            }],
        );
        assert!(Arc::ptr_eq(
            &first,
            &cache
                .read(true, || async { panic!("ordinary edit rescanned") })
                .await
                .expect("cached")
        ));
        cache.invalidate();
        let (started_tx, started) = tokio::sync::oneshot::channel();
        let (gate_tx, gate) = tokio::sync::oneshot::channel();
        let slow = tokio::spawn({
            let cache = cache.clone();
            async move {
                cache
                    .read(true, move || async move {
                        let _ = started_tx.send(());
                        gate.await.expect("gate");
                        Ok(scan("new-link"))
                    })
                    .await
                    .expect("stale")
            }
        });
        started.await.expect("started");
        let mut finished = cache.state.lock().await.pending.clone().expect("pending");
        assert!(Arc::ptr_eq(&first, &slow.await.expect("join")));
        gate_tx.send(()).expect("release");
        finished.wait_for(Option::is_some).await.expect("finish");
        let fresh = cache
            .read(true, || async { panic!("fresh scan repeated") })
            .await
            .expect("fresh");
        assert_eq!(fresh.entries[0].path.0, "new-link");
        cache.on_events(
            Path::new("root"),
            &[fff_search::WatchEvent {
                kind: fff_search::WatchEventKind::Modified,
                path: Path::new("root/new-link").into(),
                from: None,
            }],
        );
        assert_eq!(
            cache
                .read(true, || async { Ok(scan("replaced")) })
                .await
                .expect("replaced")
                .entries[0]
                .path
                .0,
            "replaced"
        );
    }
    #[tokio::test(start_paused = true)]
    async fn unwatched_scans_expire_and_a_late_watcher_makes_the_next_scan_durable() {
        let cache = Arc::new(SymlinkCache::default());
        cache
            .read(false, || async { Ok(scan("one")) })
            .await
            .expect("one");
        cache
            .read(false, || async { panic!("unexpired") })
            .await
            .expect("cached");
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(
            cache
                .read(true, || async { Ok(scan("two")) })
                .await
                .expect("two")
                .entries[0]
                .path
                .0,
            "two"
        );
        tokio::time::advance(Duration::from_secs(60)).await;
        cache
            .read(true, || async { panic!("watched scan expired") })
            .await
            .expect("durable");
    }
}
