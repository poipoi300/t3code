use crate::{
    CommandError, ExecuteOptions, Git, GitError, Repository, diagnostics::is_non_repository,
    parse_numstat,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_contracts::base_schemas::TrimmedNonEmptyString as Text;
use t3_contracts::git::{
    VcsListRefsInput, VcsListRefsInputRefKind, VcsListRefsResult, VcsRef,
    VcsStatusLocalResultBranchChanges, VcsStatusLocalResultWorkingTree,
    VcsStatusLocalResultWorkingTreeFiles, VcsStatusRemoteResult, VcsStatusResult,
};
use tokio::{
    sync::{Mutex, Semaphore, watch},
    time::Instant,
};

pub(crate) const METADATA_LIMIT: usize = 16 * 1024 * 1024;
pub(crate) const NONINTERACTIVE: &[(&str, &str)] = &[
    ("GCM_INTERACTIVE", "never"),
    ("GIT_ASKPASS", ""),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("SSH_ASKPASS", ""),
    ("SSH_ASKPASS_REQUIRE", "never"),
];

type CacheValue<V> = Option<(Instant, Result<V, CommandError>)>;
type CacheEntry<V> = Arc<Mutex<CacheValue<V>>>;
pub(crate) struct Cache<V> {
    entries: Mutex<BTreeMap<String, (u64, CacheEntry<V>)>>,
    capacity: usize,
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test(start_paused = true)]
    async fn caches_expire_at_their_deadline_and_failures_can_have_a_separate_ttl() {
        let cache = Cache::new(2);
        let reads = AtomicUsize::new(0);
        let read = || async { Ok(reads.fetch_add(1, Ordering::SeqCst)) };
        assert_eq!(
            cache
                .get("repo", Duration::from_secs(5), Duration::ZERO, read)
                .await
                .expect("first"),
            0
        );
        assert_eq!(
            cache
                .get("repo", Duration::from_secs(5), Duration::ZERO, read)
                .await
                .expect("cached"),
            0
        );
        tokio::time::advance(Duration::from_secs(5)).await;
        assert_eq!(
            cache
                .get("repo", Duration::from_secs(5), Duration::ZERO, read)
                .await
                .expect("expired"),
            1
        );
        let failed = || async {
            reads.fetch_add(1, Ordering::SeqCst);
            Err::<usize, _>(CommandError::new("test", Path::new("."), 0, "fixed failure").into())
        };
        assert!(
            cache
                .get("failure", Duration::ZERO, Duration::from_secs(30), failed)
                .await
                .is_err()
        );
        assert!(
            cache
                .get("failure", Duration::ZERO, Duration::from_secs(30), failed)
                .await
                .is_err()
        );
        assert_eq!(reads.load(Ordering::SeqCst), 3);
        tokio::time::advance(Duration::from_secs(30)).await;
        assert!(
            cache
                .get("failure", Duration::ZERO, Duration::from_secs(30), failed)
                .await
                .is_err()
        );
        assert_eq!(reads.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn invalidation_during_a_read_does_not_publish_the_old_generation() {
        let cache = Arc::new(Cache::new(2));
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let old = {
            let cache = cache.clone();
            let started = started.clone();
            let release = release.clone();
            tokio::spawn(async move {
                cache
                    .get("repo", Duration::from_secs(120), Duration::ZERO, || async {
                        started.notify_one();
                        release.notified().await;
                        Ok(1)
                    })
                    .await
            })
        };
        started.notified().await;
        cache.invalidate("repo").await;
        assert_eq!(
            cache
                .get("repo", Duration::from_secs(120), Duration::ZERO, || async {
                    Ok(2)
                })
                .await
                .expect("new generation"),
            2
        );
        release.notify_one();
        assert_eq!(old.await.expect("task").expect("rechecked"), 2);
    }
}
impl<V> Cache<V> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: Mutex::new(BTreeMap::new()),
            capacity,
        }
    }
    pub(crate) async fn invalidate(&self, key: &str) {
        self.entries.lock().await.remove(key);
    }
    pub(crate) async fn clear(&self) {
        self.entries.lock().await.clear();
    }
    async fn contains_key(&self, key: &str) -> bool {
        self.entries.lock().await.contains_key(key)
    }
}
impl<V: Clone> Cache<V> {
    pub(crate) async fn get<F, Fut>(
        &self,
        key: &str,
        ttl: Duration,
        failure_ttl: Duration,
        read: F,
    ) -> Result<V, GitError>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<V, GitError>>,
    {
        self.get_dynamic(key, |_| ttl, failure_ttl, read).await
    }
    async fn get_dynamic<F, Fut, Ttl>(
        &self,
        key: &str,
        ttl: Ttl,
        failure_ttl: Duration,
        read: F,
    ) -> Result<V, GitError>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<V, GitError>>,
        Ttl: Fn(&V) -> Duration,
    {
        loop {
            let entry = {
                let mut entries = self.entries.lock().await;
                let sequence = entries
                    .values()
                    .map(|(sequence, _)| *sequence)
                    .max()
                    .unwrap_or(0)
                    + 1;
                if !entries.contains_key(key)
                    && entries.len() >= self.capacity
                    && let Some(oldest) = entries
                        .iter()
                        .min_by_key(|(_, (sequence, _))| sequence)
                        .map(|(key, _)| key.clone())
                {
                    entries.remove(&oldest);
                }
                let (last, entry) = entries
                    .entry(key.into())
                    .or_insert_with(|| (sequence, Arc::new(Mutex::new(None))));
                *last = sequence;
                entry.clone()
            };
            let mut cached = entry.lock().await;
            if let Some((expires, value)) = &*cached
                && Instant::now() < *expires
            {
                return value.clone().map_err(Into::into);
            }
            let result = read().await;
            // Invalidations can race an in-flight scan; every waiter must see the new generation.
            let current = self
                .entries
                .lock()
                .await
                .get(key)
                .is_some_and(|(_, current)| Arc::ptr_eq(current, &entry));
            if !current {
                continue;
            }
            match result {
                Ok(value) => {
                    *cached = Some((Instant::now() + ttl(&value), Ok(value.clone())));
                    return Ok(value);
                }
                Err(GitError::Command(error)) if !failure_ttl.is_zero() => {
                    *cached = Some((Instant::now() + failure_ttl, Err((*error).clone())));
                    return Err(error.into());
                }
                Err(error) => return Err(error),
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct RepositoryPaths {
    pub repository: Repository,
    pub current_branch: Option<String>,
    pub at_root: Option<bool>,
}
#[derive(Clone)]
struct RefsSnapshot {
    local: Vec<VcsRef>,
    remote: Vec<VcsRef>,
    has_remote: bool,
}
#[derive(Default)]
struct RemoteFetch {
    retry_at: Option<Instant>,
    failures: u32,
    pending: Option<watch::Receiver<Option<bool>>>,
}
pub(crate) struct CoreState {
    #[cfg(windows)]
    pub disabled_hooks: std::sync::Mutex<BTreeMap<Option<PathBuf>, tempfile::TempDir>>,
    pub paths: Cache<Option<RepositoryPaths>>,
    paths_refresh: Cache<Option<RepositoryPaths>>,
    defaults: Cache<Option<String>>,
    remotes: Cache<bool>,
    refs: Cache<RefsSnapshot>,
    refs_refresh: Cache<RefsSnapshot>,
    pub config_locks: Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>>,
    pub checkout_locks: Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>>,
    refreshes: Mutex<BTreeMap<String, Arc<Mutex<RemoteFetch>>>>,
    fetch_permits: Arc<Semaphore>,
    pub local_status: Cache<GitStatusDetails>,
    pub untracked_failures: Mutex<BTreeMap<PathBuf, (String, u32, Instant, String)>>,
    pub operations: std::sync::atomic::AtomicUsize,
    pub operations_finished: tokio::sync::Notify,
    background_shutdown: crate::CancellationToken,
}
impl Default for CoreState {
    fn default() -> Self {
        Self {
            #[cfg(windows)]
            disabled_hooks: std::sync::Mutex::new(BTreeMap::new()),
            paths: Cache::new(2048),
            paths_refresh: Cache::new(2048),
            defaults: Cache::new(2048),
            remotes: Cache::new(2048),
            refs: Cache::new(64),
            refs_refresh: Cache::new(64),
            config_locks: Mutex::new(BTreeMap::new()),
            checkout_locks: Mutex::new(BTreeMap::new()),
            refreshes: Mutex::new(BTreeMap::new()),
            fetch_permits: Arc::new(Semaphore::new(4)),
            local_status: Cache::new(2048),
            untracked_failures: Mutex::new(BTreeMap::new()),
            operations: std::sync::atomic::AtomicUsize::new(0),
            operations_finished: tokio::sync::Notify::new(),
            background_shutdown: crate::CancellationToken::new(),
        }
    }
}
impl Drop for CoreState {
    fn drop(&mut self) {
        self.background_shutdown.cancel();
    }
}

#[derive(Clone, Debug)]
pub struct GitStatusDetails {
    pub is_repo: bool,
    pub has_origin_remote: bool,
    pub is_default_branch: bool,
    pub branch: Option<String>,
    pub upstream_ref: Option<String>,
    pub has_working_tree_changes: bool,
    pub working_tree: WorkingTree,
    pub branch_changes: Option<VcsStatusLocalResultBranchChanges>,
    pub has_upstream: bool,
    pub ahead_count: u64,
    pub behind_count: u64,
    pub ahead_of_default_count: u64,
}
pub type WorkingTree = VcsStatusLocalResultWorkingTree;
impl Default for GitStatusDetails {
    fn default() -> Self {
        Self {
            is_repo: false,
            has_origin_remote: false,
            is_default_branch: false,
            branch: None,
            upstream_ref: None,
            has_working_tree_changes: false,
            working_tree: WorkingTree {
                root_path: None,
                files: vec![],
                insertions: 0,
                deletions: 0,
            },
            branch_changes: None,
            has_upstream: false,
            ahead_count: 0,
            behind_count: 0,
            ahead_of_default_count: 0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LocalStatusOptions {
    pub include_divergence: bool,
    pub include_branch_changes: bool,
}
impl Default for LocalStatusOptions {
    fn default() -> Self {
        Self {
            include_divergence: true,
            include_branch_changes: false,
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct RemoteStatusDetails {
    pub is_repo: bool,
    pub default_branch: Option<String>,
    pub is_default_branch: bool,
    pub branch: Option<String>,
    pub upstream_ref: Option<String>,
    pub has_upstream: bool,
    pub ahead_count: u64,
    pub behind_count: u64,
    pub ahead_of_default_count: u64,
}
impl RemoteStatusDetails {
    pub fn to_contract(&self) -> VcsStatusRemoteResult {
        VcsStatusRemoteResult {
            has_upstream: self.has_upstream,
            ahead_count: self.ahead_count,
            behind_count: self.behind_count,
            ahead_of_default_count: Some(Some(self.ahead_of_default_count)),
            pr: None,
        }
    }
}

pub(crate) fn path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
pub(crate) fn physical_path(path: &Path) -> PathBuf {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    git_path(&path)
}
pub(crate) fn git_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path.to_owned()
}
pub(crate) fn branch_from_ref(value: &str) -> Option<String> {
    value
        .trim()
        .strip_prefix("refs/heads/")
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}
pub(crate) fn unborn(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("bad revision 'head'")
        || lower.contains("unknown revision") && lower.contains("path not in the working tree")
}
pub(crate) fn locale_compare(left: &str, right: &str) -> std::cmp::Ordering {
    icu_collator::Collator::try_new(Default::default(), Default::default())
        .map(|collator| collator.compare(left, right))
        .unwrap_or_else(|_| left.cmp(right))
}
pub fn parse_remote_ref(reference: &str, remote_names: &[String]) -> Option<(String, String)> {
    let reference = reference.trim();
    let mut names: Vec<_> = remote_names.iter().collect();
    names.sort_by_key(|name| std::cmp::Reverse(name.len()));
    names.into_iter().find_map(|name| {
        reference
            .strip_prefix(&format!("{name}/"))
            .map(str::trim)
            .filter(|branch| !branch.is_empty())
            .map(|branch| (name.clone(), branch.into()))
    })
}

impl GitStatusDetails {
    pub fn to_contract(&self) -> VcsStatusResult {
        VcsStatusResult {
            is_repo: self.is_repo,
            source_control_provider: None,
            has_primary_remote: self.has_origin_remote,
            is_default_ref: self.is_default_branch,
            ref_name: self.branch.clone().map(Text),
            has_working_tree_changes: self.has_working_tree_changes,
            working_tree: t3_contracts::git::VcsStatusResultWorkingTree {
                root_path: self.working_tree.root_path.clone(),
                files: self
                    .working_tree
                    .files
                    .iter()
                    .map(|file| t3_contracts::git::VcsStatusResultWorkingTreeFiles {
                        path: file.path.clone(),
                        insertions: file.insertions,
                        deletions: file.deletions,
                    })
                    .collect(),
                insertions: self.working_tree.insertions,
                deletions: self.working_tree.deletions,
            },
            branch_changes: self.branch_changes.as_ref().map(|value| {
                Some(t3_contracts::git::VcsStatusResultBranchChanges {
                    base_ref: value.base_ref.clone(),
                    insertions: value.insertions,
                    deletions: value.deletions,
                })
            }),
            has_upstream: self.has_upstream,
            ahead_count: self.ahead_count,
            behind_count: self.behind_count,
            ahead_of_default_count: Some(Some(self.ahead_of_default_count)),
            pr: None,
        }
    }
}

impl Git {
    pub(crate) async fn repository_paths_uncached(
        &self,
    ) -> Result<Option<RepositoryPaths>, GitError> {
        let reader = self.repository_reader().await?;
        let output = reader
            .execute_command(
                "GitVcsDriver.resolveRepositoryPaths.commonDir",
                &["rev-parse", "--git-common-dir"],
                ExecuteOptions {
                    allow_nonzero: true,
                    timeout: Some(Duration::from_secs(5)),
                    ..Default::default()
                },
            )
            .await?;
        if output.code != Some(0) {
            if is_non_repository(&output.stderr_text()) || !reader.cwd().is_dir() {
                return Ok(None);
            }
            return Err(CommandError::exit(
                "GitVcsDriver.resolveRepositoryPaths.commonDir",
                reader.cwd(),
                &["rev-parse", "--git-common-dir"],
                &output,
                Some("Failed to resolve the Git common directory."),
            )
            .into());
        }
        let common = reader.absolute_git_path(output.stdout_text().trim())?;
        let common_dir = physical_path(&common);
        let root = reader
            .core_text(
                "GitVcsDriver.resolveRepositoryPaths.worktreeRoot",
                &["rev-parse", "--show-toplevel"],
                true,
            )
            .await?;
        let root = root.strip_suffix('\n').unwrap_or(&root);
        if root.is_empty() {
            return Ok(None);
        }
        let branch = reader
            .core_text(
                "GitVcsDriver.resolveRepositoryPaths.currentBranch",
                &["symbolic-ref", "--quiet", "HEAD"],
                true,
            )
            .await?;
        let root = PathBuf::from(root);
        Ok(Some(RepositoryPaths {
            at_root: std::fs::canonicalize(reader.cwd())
                .ok()
                .map(|_| physical_path(reader.cwd()) == root),
            repository: Repository { root, common_dir },
            current_branch: branch_from_ref(&branch),
        }))
    }
    pub(crate) async fn repository_paths(
        &self,
        refresh: bool,
    ) -> Result<Option<RepositoryPaths>, GitError> {
        let key = path_key(self.cwd());
        if refresh {
            self.core
                .paths_refresh
                .get_dynamic(
                    &key,
                    |paths| Duration::from_secs(if paths.is_none() { 1 } else { 5 }),
                    Duration::ZERO,
                    || async {
                        self.core.paths.invalidate(&key).await;
                        self.core
                            .paths
                            .get_dynamic(
                                &key,
                                |paths| Duration::from_secs(if paths.is_none() { 1 } else { 600 }),
                                Duration::ZERO,
                                || self.repository_paths_uncached(),
                            )
                            .await
                    },
                )
                .await
        } else {
            self.core
                .paths
                .get_dynamic(
                    &key,
                    |paths| Duration::from_secs(if paths.is_none() { 1 } else { 600 }),
                    Duration::ZERO,
                    || self.repository_paths_uncached(),
                )
                .await
        }
    }
    pub async fn invalidate_caches(&self) {
        let key = path_key(self.cwd());
        let paths = self.repository_paths(false).await.ok().flatten();
        if let Some(paths) = paths {
            let common = path_key(&paths.repository.common_dir);
            self.core.refs.invalidate(&common).await;
            self.core.refs_refresh.invalidate(&common).await;
            self.core.defaults.invalidate(&common).await;
            self.core.remotes.invalidate(&common).await;
        }
        self.core.paths.invalidate(&key).await;
        self.core.paths_refresh.invalidate(&key).await;
        self.core
            .local_status
            .invalidate(&path_key(&physical_path(self.cwd())))
            .await;
    }
    pub async fn read_config_value(&self, key: &str) -> Result<Option<String>, GitError> {
        crate::checkpoint::validate_config_key(key)?;
        let output = self
            .core_text(
                "GitVcsDriver.readConfigValue",
                &["config", "--get", "--", key],
                true,
            )
            .await?;
        Ok((!output.trim().is_empty()).then(|| output.trim().into()))
    }
    pub async fn read_branch_base(&self, branch: &str) -> Result<Option<String>, GitError> {
        match self
            .read_config_value(&format!("branch.{branch}.t3-merge-base"))
            .await?
        {
            Some(value) => Ok(Some(value)),
            None => {
                self.read_config_value(&format!("branch.{branch}.gh-merge-base"))
                    .await
            }
        }
    }
    pub async fn list_remote_names(&self) -> Result<Vec<String>, GitError> {
        Ok(self
            .core_text("GitVcsDriver.listRemoteNames", &["remote"], false)
            .await?
            .lines()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect())
    }
    pub async fn remote_exists(&self, name: &str) -> Result<bool, GitError> {
        Ok(self
            .execute_command(
                "GitVcsDriver.remoteExists",
                &["remote", "get-url", "--", name],
                ExecuteOptions {
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await?
            .code
            == Some(0))
    }
    pub async fn remote_branch_exists(&self, remote: &str, branch: &str) -> Result<bool, GitError> {
        self.ref_exists(&format!("refs/remotes/{remote}/{branch}"))
            .await
    }
    pub(crate) async fn ref_exists(&self, reference: &str) -> Result<bool, GitError> {
        Ok(self
            .execute_command(
                "GitVcsDriver.branchExists",
                &["show-ref", "--verify", "--quiet", "--", reference],
                ExecuteOptions {
                    allow_nonzero: true,
                    timeout: Some(Duration::from_secs(5)),
                    ..Default::default()
                },
            )
            .await?
            .code
            == Some(0))
    }
    pub async fn primary_remote_name(&self) -> Result<Option<String>, GitError> {
        if self.remote_exists("origin").await? {
            return Ok(Some("origin".into()));
        }
        Ok(self.list_remote_names().await?.into_iter().next())
    }
    pub async fn default_branch_name(&self, remote: &str) -> Result<Option<String>, GitError> {
        let output = self
            .core_text(
                "GitVcsDriver.resolveDefaultBranchName",
                &["symbolic-ref", &format!("refs/remotes/{remote}/HEAD")],
                true,
            )
            .await?;
        let reference = output.trim();
        let Some(name) = reference
            .strip_prefix(&format!("refs/remotes/{remote}/"))
            .filter(|name| !name.is_empty() && *name != "HEAD")
        else {
            return Ok(None);
        };
        Ok(self.has_commit(reference).await?.then(|| name.to_owned()))
    }
    async fn primary_default_branch(&self) -> Result<Option<String>, GitError> {
        if let Some(branch) = self.default_branch_name("origin").await? {
            return Ok(Some(branch));
        }
        let names = self.list_remote_names().await?;
        if names.iter().any(|name| name == "origin") {
            return Ok(None);
        }
        match names.first() {
            Some(name) => self.default_branch_name(name).await,
            None => Ok(None),
        }
    }
    pub(crate) async fn current_upstream(&self) -> Result<Option<String>, GitError> {
        let output = self
            .core_text(
                "GitVcsDriver.resolveCurrentUpstream",
                &["rev-parse", "--symbolic-full-name", "@{upstream}"],
                true,
            )
            .await?;
        let value = output.trim();
        let value = value
            .strip_prefix("refs/remotes/")
            .or_else(|| value.strip_prefix("refs/heads/"))
            .unwrap_or(value);
        Ok((!value.is_empty() && value != "@{upstream}").then(|| value.into()))
    }
    pub(crate) async fn comparison_base(
        &self,
        branch: &str,
        allow_current: bool,
        known_default: Option<&str>,
    ) -> Result<Option<(String, String)>, GitError> {
        if let Some(reference) = self
            .read_config_value(&format!("branch.{branch}.t3-merge-base-ref"))
            .await?
            && (reference.starts_with("refs/heads/") || reference.starts_with("refs/remotes/"))
            && self.ref_exists(&reference).await?
        {
            let short = reference
                .strip_prefix("refs/remotes/")
                .unwrap_or(&reference);
            let display = if self.ref_exists(&format!("refs/heads/{short}")).await? {
                reference.clone()
            } else {
                short.into()
            };
            return Ok(Some((display, reference)));
        }
        let configured = self.read_branch_base(branch).await?.unwrap_or_default();
        let comparison =
            if configured.starts_with("refs/heads/") || configured.starts_with("refs/remotes/") {
                Some(configured.clone())
            } else if configured.contains('/') && !configured.starts_with("refs/") {
                Some(format!("refs/remotes/{configured}"))
            } else {
                None
            };
        if let Some(reference) = comparison
            && self.ref_exists(&reference).await?
        {
            if configured.contains('/')
                && !configured.starts_with("refs/")
                && self.ref_exists(&format!("refs/heads/{configured}")).await?
            {
                let reference = format!("refs/heads/{configured}");
                return Ok(Some((reference.clone(), reference)));
            }
            return Ok(Some((configured.clone(), reference)));
        }
        let remote = self.primary_remote_name().await.ok().flatten();
        let default = match remote.as_deref() {
            Some("origin") if known_default.is_some() => known_default.map(str::to_owned),
            Some(name) => self.default_branch_name(name).await?,
            None => None,
        };
        for candidate in [
            Some(configured),
            default,
            Some("main".into()),
            Some("master".into()),
        ]
        .into_iter()
        .flatten()
        {
            if candidate.is_empty() {
                continue;
            }
            let candidate = candidate
                .strip_prefix("origin/")
                .or_else(|| {
                    remote
                        .as_ref()
                        .and_then(|remote| candidate.strip_prefix(&format!("{remote}/")))
                })
                .unwrap_or(&candidate);
            if candidate == branch {
                if allow_current {
                    let upstream = self
                        .core_text(
                            "GitVcsDriver.resolveTrackedRemoteBaseRef",
                            &[
                                "rev-parse",
                                "--symbolic-full-name",
                                &format!("{branch}@{{upstream}}"),
                            ],
                            true,
                        )
                        .await?;
                    if upstream.trim().starts_with("refs/remotes/") {
                        let value = upstream.trim();
                        return Ok(Some((value[13..].into(), value.into())));
                    }
                    if let Some(remote) = &remote
                        && self.remote_branch_exists(remote, candidate).await?
                    {
                        return Ok(Some((
                            format!("{remote}/{candidate}"),
                            format!("refs/remotes/{remote}/{candidate}"),
                        )));
                    }
                }
                continue;
            }
            if let Some(remote) = &remote
                && self.remote_branch_exists(remote, candidate).await?
            {
                return Ok(Some((
                    format!("{remote}/{candidate}"),
                    format!("refs/remotes/{remote}/{candidate}"),
                )));
            }
            if self.ref_exists(&format!("refs/heads/{candidate}")).await? {
                return Ok(Some((candidate.into(), format!("refs/heads/{candidate}"))));
            }
        }
        Ok(None)
    }
    async fn ahead_of_base(&self, branch: &str, default: Option<&str>) -> Result<u64, GitError> {
        let Some((_, reference)) = self.comparison_base(branch, false, default).await? else {
            return Ok(0);
        };
        let output = self
            .core_text(
                "GitVcsDriver.computeAheadCountAgainstBase",
                &[
                    "rev-list",
                    "--count",
                    "--end-of-options",
                    &format!("{reference}..HEAD"),
                ],
                true,
            )
            .await?;
        Ok(output.trim().parse().unwrap_or(0))
    }
    pub async fn status_details_local(
        &self,
        options: LocalStatusOptions,
    ) -> Result<GitStatusDetails, GitError> {
        let reader = self.repository_operation().await?;
        let paths = reader.repository_paths(false).await?;
        let Some(paths) = paths else {
            return Ok(GitStatusDetails::default());
        };
        let mut args = vec!["status", "--porcelain=2", "--branch", "-z"];
        if !options.include_divergence {
            args.push("--no-ahead-behind");
        }
        let index = reader
            .core_text(
                "GitVcsDriver.statusDetails.indexPath",
                &["rev-parse", "--git-path", "index"],
                true,
            )
            .await?;
        if !index.trim().is_empty()
            && reader
                .cwd()
                .join(format!("{}.lock", index.trim()))
                .try_exists()?
        {
            return Err(reader.command_error(
                "GitVcsDriver.statusDetails.indexPath",
                "Git index is locked. Status will resume when the index lock is removed.",
            ));
        }
        let status = reader
            .execute_command(
                "GitVcsDriver.statusDetails.status",
                &args,
                ExecuteOptions {
                    allow_nonzero: true,
                    max_output_bytes: METADATA_LIMIT,
                    ..Default::default()
                },
            )
            .await?;
        if status.code != Some(0) {
            if is_non_repository(&status.stderr_text()) || !reader.cwd().is_dir() {
                return Ok(GitStatusDetails::default());
            }
            return Err(CommandError::exit(
                "GitVcsDriver.statusDetails.status",
                reader.cwd(),
                &args,
                &status,
                Some("Git status failed."),
            )
            .into());
        }
        let parsed = crate::parse_status(&status.stdout_text())?;
        let numstat = reader
            .execute_command(
                "GitVcsDriver.statusDetails.numstat",
                &[
                    "diff-index",
                    "--no-ext-diff",
                    "--no-textconv",
                    "-M",
                    "--numstat",
                    "-z",
                    "HEAD",
                    "--",
                ],
                ExecuteOptions {
                    allow_nonzero: true,
                    max_output_bytes: METADATA_LIMIT,
                    ..Default::default()
                },
            )
            .await?;
        let entries = if numstat.code == Some(0) {
            parse_numstat(&numstat.stdout_text())
        } else if unborn(&numstat.stderr_text()) {
            let mut entries = parse_numstat(
                &reader
                    .core_text(
                        "GitVcsDriver.statusDetails.numstat.unborn.staged",
                        &[
                            "diff",
                            "--no-ext-diff",
                            "--no-textconv",
                            "--cached",
                            "--numstat",
                            "-z",
                        ],
                        false,
                    )
                    .await?,
            );
            entries.extend(parse_numstat(
                &reader
                    .core_text(
                        "GitVcsDriver.statusDetails.numstat.unborn",
                        &[
                            "diff-files",
                            "--no-ext-diff",
                            "--no-textconv",
                            "--numstat",
                            "-z",
                        ],
                        false,
                    )
                    .await?,
            ));
            entries
        } else {
            return Err(CommandError::exit(
                "GitVcsDriver.statusDetails.numstat",
                reader.cwd(),
                &[
                    "diff-index",
                    "--no-ext-diff",
                    "--no-textconv",
                    "-M",
                    "--numstat",
                    "-z",
                    "HEAD",
                    "--",
                ],
                &numstat,
                Some("git diff-index HEAD --numstat failed."),
            )
            .into());
        };
        let common = path_key(&paths.repository.common_dir);
        let common_git = reader
            .clone()
            .with_git_dir(paths.repository.common_dir.clone());
        let default = reader
            .core
            .defaults
            .get(&common, Duration::from_secs(300), Duration::ZERO, || {
                common_git.primary_default_branch()
            })
            .await
            .ok()
            .flatten();
        let has_remote = reader
            .core
            .remotes
            .get(
                &common,
                Duration::from_secs(300),
                Duration::ZERO,
                || async { Ok(!common_git.list_remote_names().await?.is_empty()) },
            )
            .await
            .unwrap_or(false);
        let is_default = parsed.branch.as_deref().is_some_and(|branch| {
            Some(branch) == default.as_deref()
                || default.is_none() && ["main", "master"].contains(&branch)
        });
        let ahead_base = if options.include_divergence
            && let Some(branch) = parsed.branch.as_deref()
            && (parsed.upstream.is_none() || !is_default)
        {
            reader
                .ahead_of_base(branch, default.as_deref())
                .await
                .unwrap_or(0)
        } else {
            0
        };
        let mut file_map: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        for file in entries {
            let counts = file_map.entry(file.path).or_default();
            counts.0 += file.additions;
            counts.1 += file.deletions;
        }
        for file in &parsed.files {
            file_map.entry(file.path.clone()).or_default();
            if let Some(old) = &file.previous_path {
                file_map.entry(old.clone()).or_default();
            }
        }
        let mut files: Vec<_> = file_map
            .into_iter()
            .map(
                |(path, (insertions, deletions))| VcsStatusLocalResultWorkingTreeFiles {
                    path,
                    insertions,
                    deletions,
                },
            )
            .collect();
        files.sort_by(|left, right| locale_compare(&left.path, &right.path));
        let working_tree = WorkingTree {
            root_path: Some(Some(path_key(&paths.repository.root))),
            insertions: files.iter().map(|file| file.insertions).sum(),
            deletions: files.iter().map(|file| file.deletions).sum(),
            files,
        };
        let branch_changes = if options.include_branch_changes {
            reader
                .at(&paths.repository.root)
                .branch_change_totals(
                    parsed.branch.as_deref(),
                    default.as_deref(),
                    &parsed
                        .files
                        .iter()
                        .filter(|file| file.index_status == '?')
                        .map(|file| file.path.clone())
                        .collect::<Vec<_>>(),
                )
                .await
                .ok()
        } else {
            None
        };
        Ok(GitStatusDetails {
            is_repo: true,
            has_origin_remote: has_remote,
            is_default_branch: is_default,
            branch: parsed.branch.clone(),
            upstream_ref: parsed.upstream.clone(),
            has_working_tree_changes: !parsed.files.is_empty(),
            working_tree,
            branch_changes,
            has_upstream: parsed.upstream.is_some(),
            ahead_count: if parsed.upstream.is_none() {
                ahead_base
            } else {
                parsed.ahead
            },
            behind_count: parsed.behind,
            ahead_of_default_count: if is_default { 0 } else { ahead_base },
        })
    }
    pub async fn status_details(&self) -> Result<GitStatusDetails, GitError> {
        let _ = self.refresh_status_upstream().await;
        self.status_details_local(LocalStatusOptions::default())
            .await
    }
    pub async fn cached_local_status(&self) -> Result<GitStatusDetails, GitError> {
        self.repository_operation().await?;
        let key = path_key(&physical_path(self.cwd()));
        self.core
            .local_status
            .get(&key, Duration::from_secs(1), Duration::ZERO, || {
                self.status_details_local(LocalStatusOptions {
                    include_divergence: false,
                    include_branch_changes: true,
                })
            })
            .await
    }
    pub async fn status_details_remote(
        &self,
        refresh_upstream: bool,
    ) -> Result<RemoteStatusDetails, GitError> {
        let reader = self.repository_operation().await?;
        if refresh_upstream {
            let _ = reader.refresh_status_upstream().await;
        }
        if reader.repository_paths_uncached().await?.is_none() {
            return Ok(RemoteStatusDetails::default());
        }
        let branch = branch_from_ref(
            &reader
                .core_text(
                    "GitVcsDriver.statusDetailsRemote.branch",
                    &["symbolic-ref", "--quiet", "HEAD"],
                    true,
                )
                .await?,
        );
        let upstream = reader.current_upstream().await?;
        let (ahead, behind) = if upstream.is_some() {
            let divergence = reader
                .core_text(
                    "GitVcsDriver.statusDetailsRemote.divergence",
                    &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
                    true,
                )
                .await?;
            let mut fields = divergence
                .split_whitespace()
                .map(|count| count.parse::<u64>().unwrap_or(0));
            (fields.next().unwrap_or(0), fields.next().unwrap_or(0))
        } else {
            (
                match &branch {
                    Some(branch) => reader.ahead_of_base(branch, None).await.unwrap_or(0),
                    None => 0,
                },
                0,
            )
        };
        let default = reader.primary_default_branch().await?;
        let is_default = branch.as_deref().is_some_and(|branch| {
            Some(branch) == default.as_deref()
                || default.is_none() && ["main", "master"].contains(&branch)
        });
        let ahead_of_default_count = if is_default {
            0
        } else if let Some(branch) = branch.as_deref() {
            if upstream.is_none() {
                ahead
            } else {
                reader
                    .ahead_of_base(branch, default.as_deref())
                    .await
                    .unwrap_or(0)
            }
        } else {
            0
        };
        Ok(RemoteStatusDetails {
            is_repo: true,
            default_branch: default,
            is_default_branch: is_default,
            branch,
            upstream_ref: upstream.clone(),
            has_upstream: upstream.is_some(),
            ahead_count: ahead,
            behind_count: behind,
            ahead_of_default_count,
        })
    }
    pub async fn refresh_status_upstream(&self) -> Result<(), GitError> {
        let reader = self.repository_operation().await?;
        let head = reader
            .core_text(
                "GitVcsDriver.refreshStatusUpstreamIfStale.headRef",
                &["symbolic-ref", "--quiet", "HEAD"],
                true,
            )
            .await?;
        if head.trim().is_empty() {
            return Ok(());
        }
        let remote = reader
            .core_text(
                "GitVcsDriver.refreshStatusUpstreamIfStale.upstreamRemote",
                &[
                    "for-each-ref",
                    "--format=%(upstream:remotename)",
                    head.trim(),
                ],
                true,
            )
            .await?;
        let remote = remote.trim();
        if remote.is_empty() || remote == "." {
            return Ok(());
        }
        let Some(paths) = reader.repository_paths(false).await? else {
            return Ok(());
        };
        let common = path_key(&paths.repository.common_dir);
        let key = format!("{common}\0{remote}");
        let state = {
            let mut entries = reader.core.refreshes.lock().await;
            if entries.len() >= 2048 && !entries.contains_key(&key) {
                entries.retain(|_, state| Arc::strong_count(state) > 1);
                if entries.len() >= 2048 {
                    entries.pop_first();
                }
            }
            entries.entry(key).or_default().clone()
        };
        let mut locked = state.lock().await;
        let mut receiver = if let Some(pending) = &locked.pending {
            if locked.retry_at.is_some_and(|retry| Instant::now() < retry) {
                return Ok(());
            }
            pending.clone()
        } else {
            if locked.retry_at.is_some_and(|retry| Instant::now() < retry) {
                return Ok(());
            }
            let (sender, receiver) = watch::channel(None);
            locked.pending = Some(receiver.clone());
            let state = state.clone();
            let fetch_cwd = if paths
                .repository
                .common_dir
                .file_name()
                .is_some_and(|name| name == ".git")
            {
                paths.repository.common_dir.parent().unwrap_or(reader.cwd())
            } else {
                &paths.repository.common_dir
            };
            let mut git = reader.at(fetch_cwd).with_cancellation(
                reader
                    .status_fetch_scope
                    .as_ref()
                    .unwrap_or(&reader.core.background_shutdown)
                    .child_token(),
            );
            // The fetch must not keep its owning service alive after the last handle is dropped.
            git.core = Arc::new(CoreState::default());
            let remote = remote.to_owned();
            let permits = reader.core.fetch_permits.clone();
            let operation = reader.status_fetch_scope.as_ref().map(|_| {
                reader
                    .core
                    .operations
                    .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                crate::actions::OperationGuard(reader.core.clone())
            });
            let scope = reader.operation_scope.as_ref().map(|scope| scope.start());
            tokio::spawn(async move {
                let _operation = operation;
                let _scope = scope;
                let _permit = tokio::select! {
                    _ = git.cancellation.cancelled() => None,
                    permit = permits.acquire() => permit.ok(),
                };
                let result = git
                    .execute_command(
                        "GitVcsDriver.fetchRemoteForStatus",
                        &[
                            "--git-dir",
                            &common,
                            "fetch",
                            "--quiet",
                            "--no-tags",
                            "--no-auto-gc",
                            "--no-write-fetch-head",
                            "--",
                            &remote,
                        ],
                        ExecuteOptions {
                            timeout: Some(Duration::from_secs(300)),
                            env: crate::env(NONINTERACTIVE),
                            ..Default::default()
                        },
                    )
                    .await;
                let mut locked = state.lock().await;
                locked.failures = if result.is_ok() {
                    0
                } else {
                    locked.failures.saturating_add(1)
                };
                let cooldown = if locked.failures == 0 {
                    15
                } else {
                    30u64
                        .saturating_mul(1u64.checked_shl(locked.failures - 1).unwrap_or(u64::MAX))
                        .min(900)
                };
                locked.retry_at = Some(Instant::now() + Duration::from_secs(cooldown));
                locked.pending = None;
                let _ = sender.send(Some(result.is_ok()));
            });
            receiver
        };
        drop(locked);
        tokio::select! {
            _ = reader.cancellation.cancelled() => {},
            _ = tokio::time::timeout(Duration::from_secs(5), receiver.wait_for(|value| value.is_some())) => {},
        }
        let mut locked = state.lock().await;
        if locked.pending.is_some() {
            locked.retry_at = Some(Instant::now() + Duration::from_secs(15));
        }
        Ok(())
    }
    pub(crate) async fn await_status_fetch(&self, remote: &str) {
        if let Ok(Some(paths)) = self.repository_paths(false).await {
            let key = format!("{}\0{remote}", path_key(&paths.repository.common_dir));
            let state = self.core.refreshes.lock().await.get(&key).cloned();
            if let Some(state) = state
                && let Some(mut receiver) = state.lock().await.pending.clone()
            {
                let _ = receiver.wait_for(|value| value.is_some()).await;
            }
        }
    }
    async fn read_refs_snapshot(&self, paths: &RepositoryPaths) -> Result<RefsSnapshot, GitError> {
        let common = path_key(&paths.repository.common_dir);
        let git = self.at(&paths.repository.root);
        let args = [
            "--git-dir",
            &common,
            "for-each-ref",
            "--format=%(refname)%09%(committerdate:unix)%09%(symref)%09%(upstream)",
            "refs/heads",
            "refs/remotes",
        ];
        let refs = git
            .execute_command(
                "GitVcsDriver.listRefs.snapshotRefs",
                &args,
                ExecuteOptions {
                    max_output_bytes: METADATA_LIMIT,
                    ..Default::default()
                },
            )
            .await?;
        let default_name = git.default_branch_name("origin").await?;
        let default = default_name.as_deref();
        let names = git.list_remote_names().await.unwrap_or_default();
        let worktrees = git.list_worktrees().await.unwrap_or_default();
        let mut local = Vec::new();
        let mut remote = Vec::new();
        for line in refs.stdout_text().lines().filter(|line| !line.is_empty()) {
            let fields: Vec<_> = line.split('\t').collect();
            if fields.len() < 4 || !fields[2].is_empty() {
                continue;
            }
            let time = fields[1].parse::<i64>().unwrap_or(0);
            if let Some(name) = fields[0].strip_prefix("refs/heads/") {
                let worktree_path = worktrees
                    .iter()
                    .find(|tree| {
                        tree.branch.as_deref() == Some(&format!("refs/heads/{name}"))
                            && tree.prunable.is_none()
                            && tree.path.exists()
                            && tree.path != paths.repository.common_dir
                    })
                    .map(|tree| Text(path_key(&tree.path)));
                local.push((
                    time,
                    VcsRef {
                        name: Text(name.into()),
                        is_remote: Some(Some(false)),
                        remote_name: None,
                        upstream_ref: (!fields[3].is_empty()).then(|| Some(Text(fields[3].into()))),
                        current: false,
                        is_default: Some(name) == default,
                        worktree_path,
                    },
                ));
            } else if let Some(name) = fields[0].strip_prefix("refs/remotes/") {
                let parsed = parse_remote_ref(name, &names);
                if parsed.as_ref().is_some_and(|(_, branch)| branch == "HEAD") {
                    continue;
                }
                remote.push((
                    time,
                    VcsRef {
                        name: Text(name.into()),
                        is_remote: Some(Some(true)),
                        remote_name: parsed
                            .as_ref()
                            .map(|(remote, _)| Some(Text(remote.clone()))),
                        upstream_ref: None,
                        current: false,
                        is_default: parsed.as_ref().is_some_and(|(remote, branch)| {
                            remote == "origin" && Some(branch.as_str()) == default
                        }),
                        worktree_path: None,
                    },
                ));
            }
        }
        for refs in [&mut local, &mut remote] {
            refs.sort_by(|left, right| {
                right
                    .0
                    .cmp(&left.0)
                    .then_with(|| locale_compare(&left.1.name.0, &right.1.name.0))
            });
        }
        Ok(RefsSnapshot {
            local: local.into_iter().map(|(_, reference)| reference).collect(),
            remote: remote.into_iter().map(|(_, reference)| reference).collect(),
            has_remote: !names.is_empty(),
        })
    }
    pub async fn list_branches(
        &self,
        input: &VcsListRefsInput,
    ) -> Result<VcsListRefsResult, GitError> {
        let reader = self.repository_operation().await?;
        let git = reader.at(&input.cwd.0).repository_operation().await?;
        let refresh = input.refresh.flatten() == Some(true);
        let Some(paths) = git.repository_paths(refresh).await? else {
            return Ok(VcsListRefsResult {
                is_at_repository_root: None,
                refs: vec![],
                is_repo: false,
                has_primary_remote: false,
                next_cursor: None,
                total_count: 0,
            });
        };
        let common = path_key(&paths.repository.common_dir);
        let snapshot = if refresh || !reader.core.refs.contains_key(&common).await {
            reader
                .core
                .refs_refresh
                .get(
                    &common,
                    Duration::from_secs(5),
                    Duration::from_secs(30),
                    || async {
                        reader.core.refs.invalidate(&common).await;
                        reader
                            .core
                            .refs
                            .get(&common, Duration::from_secs(120), Duration::ZERO, || {
                                git.read_refs_snapshot(&paths)
                            })
                            .await
                    },
                )
                .await?
        } else {
            reader
                .core
                .refs
                .get(&common, Duration::from_secs(120), Duration::ZERO, || {
                    git.read_refs_snapshot(&paths)
                })
                .await?
        };
        let mut local = snapshot.local;
        let root = path_key(&paths.repository.root);
        let branch = git
            .core_text(
                "GitVcsDriver.listRefs.currentBranch",
                &["symbolic-ref", "--quiet", "HEAD"],
                true,
            )
            .await?;
        let current_branch = branch_from_ref(&branch);
        for reference in &mut local {
            reference.current = Some(&reference.name.0) == current_branch.as_ref();
            if reference
                .worktree_path
                .as_ref()
                .is_some_and(|path| path.0 == root)
            {
                reference.worktree_path = None;
            }
            if reference.current {
                reference.worktree_path = Some(Text(root.clone()));
            }
        }
        let local_names: BTreeSet<_> = local
            .iter()
            .map(|reference| reference.name.0.clone())
            .collect();
        let include_matching = input.include_matching_remote_refs.flatten() == Some(true);
        local.extend(snapshot.remote.into_iter().filter(|reference| {
            if include_matching {
                return true;
            }
            let remote = reference
                .remote_name
                .as_ref()
                .and_then(Option::as_ref)
                .map(|name| name.0.as_str());
            let branch = reference
                .name
                .0
                .split_once('/')
                .map(|(_, branch)| branch)
                .unwrap_or("");
            remote != Some("origin") || !local_names.contains(branch)
        }));
        local.sort_by_key(|reference| {
            if reference.current {
                0
            } else if reference.is_default {
                1
            } else {
                2
            }
        });
        let query = input
            .query
            .as_ref()
            .and_then(Option::as_ref)
            .map(|value| value.0.to_lowercase());
        let kind = input.ref_kind.as_ref().and_then(Option::as_ref);
        local.retain(|reference| {
            query
                .as_ref()
                .is_none_or(|query| reference.name.0.to_lowercase().contains(query))
                && match kind {
                    Some(VcsListRefsInputRefKind::Local) => reference.is_remote != Some(Some(true)),
                    Some(VcsListRefsInputRefKind::Remote) => {
                        reference.is_remote == Some(Some(true))
                    }
                    _ => true,
                }
        });
        let total_count = local.len() as u64;
        let cursor = input.cursor.flatten().unwrap_or(0) as usize;
        let limit = input.limit.flatten().unwrap_or(100) as usize;
        let refs: Vec<_> = local.into_iter().skip(cursor).take(limit).collect();
        let next = cursor.saturating_add(refs.len()) as u64;
        Ok(VcsListRefsResult {
            is_at_repository_root: paths.at_root.map(Some),
            refs,
            is_repo: true,
            has_primary_remote: snapshot.has_remote,
            next_cursor: (next < total_count).then_some(next),
            total_count,
        })
    }
}
