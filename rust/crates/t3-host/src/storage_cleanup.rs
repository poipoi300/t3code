use crate::{Error, Result, background::Clock, worktrees::validate_cleanup_worktree};
use async_trait::async_trait;
use cap_std::fs::Dir;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};
use t3_contracts::{
    base_schemas::{ProjectId, ThreadId},
    orchestration_v2::OrchestrationV2ThreadShell,
    settings::*,
};
use tokio::sync::{Mutex, OwnedMutexGuard, mpsc};

const DAY_MS: i64 = 86_400_000;
#[derive(Clone, PartialEq)]
pub struct CleanupThread {
    pub shell: OrchestrationV2ThreadShell,
    pub deleted: bool,
    pub project_path: PathBuf,
}
#[derive(Clone, PartialEq)]
pub struct CleanupSnapshot {
    pub settings: ServerSettings,
    pub threads: Vec<CleanupThread>,
    pub project_roots: Vec<PathBuf>,
    pub terminal_paths: Vec<PathBuf>,
    pub live_provider_paths: Vec<PathBuf>,
    pub pending_deletion_threads: BTreeSet<ThreadId>,
    pub managed_roots: Vec<PathBuf>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedPullRequest {
    pub state: String,
    pub head_ref: String,
    pub base_ref: String,
    pub head_sha: String,
}
#[async_trait]
pub trait CleanupHost: Send + Sync {
    /// Complete authoritative settings, live/archived/deleted ownership and project roots,
    /// starting/running terminal cwds, and every live provider cwd (including ready/shared
    /// sessions). Pending deletions include failed effects, excluding only succeeded/cancelled.
    /// Any unavailable source, decode or liveness failure MUST return Err, never empty data.
    /// Read again under the SAME lease used by provider startup and workspace restoration.
    async fn snapshot(&self) -> Result<CleanupSnapshot>;
    /// Complete enabled scheduled-task thread ids; unavailable reads must fail.
    async fn enabled_scheduled_threads(&self) -> Result<BTreeSet<ThreadId>>;
    async fn storage_cleanup_settings(&self) -> Result<StorageCleanupSettings>;
    /// Must be the same lease provider turn startup and checkpoint restore use.
    async fn acquire_workspace_lease(&self, path: &Path) -> Result<OwnedMutexGuard<()>>;
    async fn branch_pull_request(
        &self,
        project: &Path,
        branch: &str,
    ) -> Result<Option<MergedPullRequest>>;
    async fn invalidate_git_status(&self, project: &Path);
}
pub struct StorageCleanup {
    state_dir: PathBuf,
    host: Arc<dyn CleanupHost>,
    clock: Arc<dyn Clock>,
    gate: Mutex<()>,
    report: Mutex<ReportState>,
    automatic: tokio::sync::Notify,
    dev_worktree_root: Option<PathBuf>,
    orphan_sizes: Mutex<BTreeMap<PathBuf, Option<u64>>>,
}
struct ReportState {
    latest: Option<StorageCleanupReport>,
    subscribers: Vec<mpsc::UnboundedSender<Option<StorageCleanupReport>>>,
}
pub struct CleanupWorker {
    task: tokio::task::JoinHandle<()>,
}
impl Drop for CleanupWorker {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn number(n: u64) -> WireNumber {
    WireNumber::Finite(n.into())
}
fn epoch(date: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(date.as_str()?)
        .ok()
        .map(|d| d.timestamp_millis())
}
fn shell_value(shell: &OrchestrationV2ThreadShell) -> Result<Value> {
    t3_contracts::stored::to_value(shell).map_err(|_| Error::Invalid)
}
pub fn activity_at(shell: &OrchestrationV2ThreadShell) -> Result<i64> {
    let value = shell_value(shell)?;
    [
        "createdAt",
        "latestUserMessageAt",
        "latestRunRequestedAt",
        "latestRunStartedAt",
        "latestRunCompletedAt",
    ]
    .iter()
    .filter_map(|k| epoch(&value[k]))
    .max()
    .ok_or(Error::Timestamp)
}
pub fn thread_idle(
    shell: &OrchestrationV2ThreadShell,
    now: i64,
    settings: &ServerSettings,
) -> Result<bool> {
    let value = shell_value(shell)?;
    let status = value["status"].as_str().unwrap_or("");
    let message = epoch(&value["latestUserMessageAt"]);
    let queued = status != "failed"
        && message.is_some_and(|message| {
            (now - message).abs() <= 120_000
                && (value["latestRunId"].is_null()
                    || [
                        "latestRunRequestedAt",
                        "latestRunStartedAt",
                        "latestRunCompletedAt",
                    ]
                    .iter()
                    .all(|k| epoch(&value[k]).is_none_or(|at| at < message)))
        });
    let mut recovery = false;
    if status == "failed"
        && value["lastErrorClass"] == "usage_limit"
        && value["archivedAt"].is_null()
        && value["settledOverride"] != "settled"
        && let Some(reset) = epoch(&value["usageLimitResetAt"])
    {
        let recorded = &value["limitRecovery"];
        recovery = if recorded["runId"] == value["latestRunId"]
            && recorded["resetAt"] == value["usageLimitResetAt"]
        {
            recorded["autoResume"] == true || recorded["snooze"] == true && reset > now
        } else {
            reset
                > epoch(&value["latestRunCompletedAt"])
                    .or_else(|| epoch(&value["updatedAt"]))
                    .unwrap_or(i64::MAX)
                && now - reset <= DAY_MS
                && (settings.auto_resume_limited_threads
                    || settings.snooze_limited_threads && reset > now)
        };
    }
    Ok(value["activeRunId"].is_null()
        && matches!(
            status,
            "idle" | "completed" | "interrupted" | "failed" | "cancelled" | "rolled_back"
        )
        && value["pendingBackgroundTasks"]
            .as_array()
            .is_none_or(Vec::is_empty)
        && value["pendingRuntimeRequest"].is_null()
        && !queued
        && !recovery)
}
pub fn storage_cleanup_thread_idle(
    shell: &OrchestrationV2ThreadShell,
    now: i64,
    settings: &ServerSettings,
) -> Result<bool> {
    Ok(shell.worktree_path.is_some() && thread_idle(shell, now, settings)?)
}
pub fn pull_request_merged(
    pr: Option<&MergedPullRequest>,
    branch: &str,
    default: &str,
    head: &str,
    integrated: bool,
) -> bool {
    pr.is_some_and(|p| {
        p.state == "merged"
            && (integrated || p.head_ref == branch && p.base_ref == default && p.head_sha == head)
    })
}
pub fn resolve_rules(
    settings: &ServerSettings,
    project: &ProjectId,
) -> Result<WorktreeCleanupRules> {
    let policy = settings
        .project_settings_overrides
        .get(project)
        .and_then(|p| p.worktree_cleanup.as_ref())
        .unwrap_or(&settings.worktree_cleanup);
    if let Some(WorktreeCleanupValueValue::Custom2(custom)) = policy {
        return Ok(custom.rules.clone());
    }
    let off = matches!(policy, Some(WorktreeCleanupValueValue::Off1(_)));
    let c = &settings.storage_cleanup;
    Ok(WorktreeCleanupRules {
        worktree_keep_when: c.worktree_keep_when.clone(),
        worktree_after_days: if off { None } else { c.worktree_after_days },
        worktree_settled_after_days: if off {
            None
        } else {
            c.worktree_settled_after_days
        },
        worktree_on_merge: !off && c.worktree_on_merge,
        worktree_on_delete: !off && c.worktree_on_delete,
        worktree_unchanged: !off && c.worktree_unchanged,
    })
}
impl StorageCleanup {
    pub fn new(state_dir: PathBuf, host: Arc<dyn CleanupHost>, clock: Arc<dyn Clock>) -> Self {
        let dev_worktree_root = state_dir.parent().and_then(|base| {
            t3_config::environment::resolve_worktree_home(base)
                .filter(|home| std::path::absolute(base).ok().as_ref() == Some(home))
                .map(|_| base.join("worktrees"))
        });
        Self {
            state_dir,
            host,
            clock,
            gate: Mutex::new(()),
            report: Mutex::new(ReportState {
                latest: None,
                subscribers: Vec::new(),
            }),
            automatic: tokio::sync::Notify::new(),
            dev_worktree_root,
            orphan_sizes: Mutex::new(BTreeMap::new()),
        }
    }
    async fn snapshot(&self) -> Result<CleanupSnapshot> {
        let mut snapshot = tokio::time::timeout(Duration::from_secs(30), self.host.snapshot())
            .await
            .map_err(|_| Error::Refused("cleanup safety snapshot is unavailable"))??;
        snapshot.managed_roots.retain(|path| !network_path(path));
        if let Some(only) = &self.dev_worktree_root {
            let real = dunce::canonicalize(only).unwrap_or_else(|_| only.clone());
            snapshot
                .managed_roots
                .retain(|path| dunce::canonicalize(path).unwrap_or_else(|_| path.clone()) == real);
        }
        Ok(snapshot)
    }
    pub fn request_automatic_sweep(&self) {
        self.automatic.notify_one();
    }
    pub fn start(self: &Arc<Self>) -> CleanupWorker {
        let service = self.clone();
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(3600));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {_=interval.tick()=>{},_=service.automatic.notified()=>{}}
                let _ = service.run_automatic().await;
            }
        });
        CleanupWorker { task }
    }
    pub async fn latest_report(&self) -> Option<StorageCleanupReport> {
        self.report.lock().await.latest.clone()
    }
    pub async fn reports(&self) -> mpsc::UnboundedReceiver<Option<StorageCleanupReport>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut report = self.report.lock().await;
        let _ = tx.send(report.latest.clone());
        report.subscribers.push(tx);
        rx
    }
    pub async fn drain(&self) {
        let _guard = self.gate.lock().await;
    }
    pub async fn run_now(&self) -> Result<StorageCleanupReport> {
        self.sweep(StorageCleanupReportTrigger::Manual, false).await
    }
    pub async fn run_automatic(&self) -> Result<StorageCleanupReport> {
        self.sweep(StorageCleanupReportTrigger::Automatic, false)
            .await
    }
    pub async fn preview(&self) -> Result<StorageCleanupReport> {
        self.sweep(StorageCleanupReportTrigger::Manual, true).await
    }
    async fn sweep(
        &self,
        trigger: StorageCleanupReportTrigger,
        dry_run: bool,
    ) -> Result<StorageCleanupReport> {
        let _guard = self.gate.lock().await;
        let now = self.clock.now_ms();
        let initial = self.snapshot().await?;
        let mut entries = Vec::new();
        self.clean_worktrees(&initial, now, dry_run, &mut entries)
            .await;
        for (kind, root, days) in [
            (
                StorageCleanupReportEntryKind::BrowserArtifacts,
                self.state_dir.join("browser-artifacts"),
                initial
                    .settings
                    .storage_cleanup
                    .browser_artifacts_after_days,
            ),
            (
                StorageCleanupReportEntryKind::Logs,
                self.state_dir.join("logs"),
                initial.settings.storage_cleanup.logs_after_days,
            ),
        ] {
            let Some(days) = days else {
                continue;
            };
            let mut entry = blank(kind.clone(), None);
            let mut removed = (0_u64, 0_u64);
            let result = self
                .clean_files(
                    &root,
                    days,
                    now,
                    kind == StorageCleanupReportEntryKind::Logs,
                    dry_run,
                    &mut removed,
                )
                .await;
            entry.outcome = if matches!(
                &result,
                Err(Error::Refused("cleanup folder is a symbolic link"))
            ) {
                StorageCleanupReportEntryOutcome::Kept
            } else if result.is_err() {
                StorageCleanupReportEntryOutcome::Failed
            } else if removed.0 > 0 && !dry_run {
                StorageCleanupReportEntryOutcome::Removed
            } else {
                StorageCleanupReportEntryOutcome::Kept
            };
            entry.reason = match result {
                Err(Error::Refused(reason)) => reason.into(),
                Err(_) => "Storage file cleanup failed".into(),
                Ok(()) if dry_run && removed.0 > 0 => {
                    format!("Would remove {} expired files", removed.0)
                }
                Ok(()) if removed.0 > 0 => format!(
                    "Removed {} {}",
                    removed.0,
                    if kind == StorageCleanupReportEntryKind::Logs {
                        "rotated logs"
                    } else {
                        "browser artifacts"
                    }
                ),
                Ok(()) => "No expired files".into(),
            };
            if removed.0 > 0 {
                entry.files = Some(number(removed.0));
                entry.bytes = Some(number(removed.1));
            }
            entries.push(entry);
        }
        let mut counts = [0_u64; 3];
        let mut freed = 0_u64;
        for e in &entries {
            let i = match e.outcome {
                StorageCleanupReportEntryOutcome::Removed => 0,
                StorageCleanupReportEntryOutcome::Kept => 1,
                StorageCleanupReportEntryOutcome::Failed => 2,
            };
            counts[i] += 1;
            if i != 1
                && let Some(WireNumber::Finite(bytes)) = &e.bytes
            {
                freed = freed.saturating_add(bytes.as_u64().unwrap_or(0));
            }
        }
        entries.sort_by_key(|e| match e.outcome {
            StorageCleanupReportEntryOutcome::Failed => 0,
            StorageCleanupReportEntryOutcome::Removed => 1,
            _ if e.kind == StorageCleanupReportEntryKind::Worktree && e.thread_id.is_none() => 2,
            _ => 3,
        });
        let omitted = entries.len().saturating_sub(200);
        entries.truncate(200);
        let iso = |ms| {
            chrono::DateTime::from_timestamp_millis(ms)
                .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
                .ok_or(Error::Timestamp)
        };
        let report = StorageCleanupReport {
            trigger,
            started_at: iso(now)?,
            finished_at: iso(self.clock.now_ms())?,
            entries,
            counts: StorageCleanupReportCounts {
                removed: number(counts[0]),
                kept: number(counts[1]),
                failed: number(counts[2]),
            },
            omitted_count: number(omitted as u64),
            bytes_freed: number(freed),
        };
        if !dry_run {
            let mut published = self.report.lock().await;
            published.latest = Some(report.clone());
            published
                .subscribers
                .retain(|tx| tx.send(Some(report.clone())).is_ok());
        }
        Ok(report)
    }
    async fn clean_files(
        &self,
        root: &Path,
        days: i64,
        now: i64,
        logs: bool,
        dry_run: bool,
        removed: &mut (u64, u64),
    ) -> Result<()> {
        if days < 0 {
            return Err(Error::Invalid);
        }
        if !root.try_exists().map_err(|_| Error::Io)? {
            return Ok(());
        }
        let real = dunce::canonicalize(root).map_err(|_| Error::Io)?;
        let parent =
            dunce::canonicalize(root.parent().ok_or(Error::Invalid)?).map_err(|_| Error::Io)?;
        if real != parent.join(root.file_name().ok_or(Error::Invalid)?) {
            return Err(Error::Refused("cleanup folder is a symbolic link"));
        }
        let state = dunce::canonicalize(&self.state_dir).map_err(|_| Error::Io)?;
        if real == state || !real.starts_with(&state) {
            return Err(Error::Refused(
                "cleanup folder is outside the data directory",
            ));
        }
        let cap = Arc::new(
            Dir::open_ambient_dir(&real, cap_std::ambient_authority()).map_err(|_| Error::Io)?,
        );
        let mut pending = vec![PathBuf::new()];
        let threshold = now.saturating_sub(days.saturating_mul(DAY_MS));
        while let Some(relative) = pending.pop() {
            let directory = real.join(&relative);
            let entries = tokio::task::spawn_blocking(move || {
                fs::read_dir(&directory)
                    .map_err(|_| Error::Io)?
                    .map(|entry| {
                        let entry = entry.map_err(|_| Error::Io)?;
                        let path = entry.path();
                        let metadata = fs::symlink_metadata(&path).map_err(|_| Error::Io)?;
                        if metadata.file_type().is_symlink()
                            || dunce::canonicalize(&path).map_err(|_| Error::Io)? != path
                        {
                            return Ok(None);
                        }
                        Ok(Some((relative.join(entry.file_name()), metadata)))
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .await
            .map_err(|_| Error::Service)??;
            for (child, metadata) in entries.into_iter().flatten() {
                if metadata.is_dir() {
                    if logs {
                        pending.push(child);
                    }
                    continue;
                }
                if !metadata.is_file()
                    || logs
                        && !rotated_log(&child.file_name().ok_or(Error::Invalid)?.to_string_lossy())
                {
                    continue;
                }
                let modified =
                    metadata
                        .modified()
                        .ok()
                        .map(|t| match t.duration_since(UNIX_EPOCH) {
                            Ok(duration) => duration.as_millis() as i128,
                            Err(error) => -(error.duration().as_millis() as i128),
                        });
                if modified.is_none_or(|ms| ms >= i128::from(threshold)) {
                    continue;
                }
                let settings = self.host.storage_cleanup_settings().await?;
                if (if logs {
                    settings.logs_after_days
                } else {
                    settings.browser_artifacts_after_days
                }) != Some(days)
                {
                    return Ok(());
                }
                if !dry_run {
                    let cap = cap.clone();
                    tokio::task::spawn_blocking(move || cap.remove_file(&child))
                        .await
                        .map_err(|_| Error::Service)?
                        .map_err(|_| Error::Io)?;
                }
                removed.0 += 1;
                removed.1 = removed.1.saturating_add(metadata.len());
            }
        }
        Ok(())
    }
    async fn clean_worktrees(
        &self,
        snapshot: &CleanupSnapshot,
        now: i64,
        dry_run: bool,
        entries: &mut Vec<StorageCleanupReportEntry>,
    ) {
        if !std::iter::once(ProjectId(String::new()))
            .chain(snapshot.settings.project_settings_overrides.keys().cloned())
            .any(|project| {
                resolve_rules(&snapshot.settings, &project).is_ok_and(|rules| enabled(&rules))
            })
        {
            return;
        }
        let mut groups: Vec<(PathBuf, Vec<&CleanupThread>)> = Vec::new();
        let mut group_indices = BTreeMap::new();
        for thread in snapshot
            .threads
            .iter()
            .filter(|t| !t.deleted)
            .chain(snapshot.threads.iter().filter(|t| t.deleted))
        {
            if let Some(path) = &thread.shell.worktree_path {
                let path = PathBuf::from(&path.0);
                let key = dunce::canonicalize(&path).unwrap_or(path);
                let index = *group_indices.entry(key.clone()).or_insert_with(|| {
                    groups.push((key, Vec::new()));
                    groups.len() - 1
                });
                groups[index].1.push(thread);
            }
        }
        for (path, group) in groups {
            let live: Vec<_> = group.iter().copied().filter(|t| !t.deleted).collect();
            let deleted_owner = group.iter().copied().filter(|t| t.deleted).find(|deleted| {
                resolve_rules(&snapshot.settings, &deleted.shell.project_id)
                    .is_ok_and(|r| r.worktree_on_delete)
                    && owner(&[vec![*deleted], live.clone()].concat())
                        .is_some_and(|t| t.shell.id == deleted.shell.id)
            });
            let candidate = deleted_owner.or_else(|| owner(&live)).unwrap_or(group[0]);
            let members = if candidate.deleted {
                [vec![candidate], live].concat()
            } else {
                live
            };
            let owner = candidate;
            let rules = match resolve_rules(&snapshot.settings, &owner.shell.project_id) {
                Ok(rules) => rules,
                Err(_) => continue,
            };
            if !enabled(&rules) || !path.exists() {
                continue;
            }
            let mut entry = blank(StorageCleanupReportEntryKind::Worktree, Some(&path));
            entry.thread_id = Some(owner.shell.id.clone());
            entry.thread_title = Some(owner.shell.title.clone());
            entry.project_path = Some(owner.project_path.to_string_lossy().into_owned());
            let result = self
                .clean_worktree(snapshot, &members, owner, &path, &rules, now, dry_run)
                .await;
            match result {
                Ok((reason, bytes)) => {
                    entry.reason = reason;
                    entry.bytes = bytes.map(number);
                    if !dry_run && entry.bytes.is_some() {
                        entry.outcome = StorageCleanupReportEntryOutcome::Removed;
                    }
                }
                Err(Error::Refused(reason)) => entry.reason = sentence(reason),
                Err(_) => {
                    entry.outcome = StorageCleanupReportEntryOutcome::Failed;
                    entry.reason = "Worktree inspection or removal failed".into();
                }
            }
            entries.push(entry);
        }
        self.report_orphans(snapshot, now, entries).await;
    }
    #[allow(clippy::too_many_arguments)]
    async fn clean_worktree(
        &self,
        snapshot: &CleanupSnapshot,
        group: &[&CleanupThread],
        owner: &CleanupThread,
        path: &Path,
        rules: &WorktreeCleanupRules,
        now: i64,
        dry_run: bool,
    ) -> Result<(String, Option<u64>)> {
        if self::owner(group).is_none() {
            return Err(Error::Refused("worktree is shared by unrelated threads"));
        }
        for thread in group {
            if !thread.deleted && !thread_idle(&thread.shell, now, &snapshot.settings)? {
                return Err(Error::Refused("thread is running or has pending work"));
            }
            if thread.shell.branch != owner.shell.branch {
                return Err(Error::Refused(
                    "subagent branch differs from the worktree branch",
                ));
            }
        }
        let activity = group
            .iter()
            .map(|t| activity_at(&t.shell))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .max()
            .ok_or(Error::Timestamp)?;
        let value = shell_value(&owner.shell)?;
        let mut settled = epoch(&value["settledAt"]);
        let mut current = owner;
        let mut seen = BTreeSet::new();
        while settled.is_none() && !owner.deleted {
            let value = shell_value(&current.shell)?;
            if value["lineage"]["relationshipToParent"] != "subagent"
                || value["settledOverride"] == "active"
                || !value["pinnedAt"].is_null()
                || !seen.insert(current.shell.id.clone())
            {
                break;
            }
            let Some(parent) = snapshot
                .threads
                .iter()
                .find(|t| !t.deleted && value["lineage"]["parentThreadId"] == t.shell.id.0)
            else {
                break;
            };
            if parent.shell.project_id != owner.shell.project_id
                || !thread_idle(&parent.shell, now, &snapshot.settings)?
                || shell_value(&parent.shell)?["lineage"]["rootThreadId"]
                    != value["lineage"]["rootThreadId"]
            {
                break;
            }
            current = parent;
            settled = epoch(&shell_value(&parent.shell)?["settledAt"]);
        }
        if settled.is_some()
            && current.shell.id != owner.shell.id
            && now
                - epoch(&shell_value(&current.shell)?["updatedAt"])
                    .unwrap_or(now)
                    .max(activity)
                < 60_000
        {
            return Err(Error::Refused("settled moments ago"));
        }
        if group.iter().any(|t| {
            shell_value(&t.shell).ok().is_some_and(|v| {
                !v["settledAt"].is_null()
                    && now - epoch(&v["updatedAt"]).unwrap_or(now).max(activity) < 60_000
            })
        }) {
            return Err(Error::Refused("settled moments ago"));
        }
        if owner.deleted && !rules.worktree_on_delete {
            return Err(Error::Refused("no rules apply"));
        }
        let old = rules
            .worktree_after_days
            .is_some_and(|days| activity < now - days.saturating_mul(DAY_MS));
        let settled_due = settled
            .zip(rules.worktree_settled_after_days)
            .is_some_and(|(at, days)| now - at.max(activity) >= days.saturating_mul(DAY_MS));
        let unchanged =
            rules.worktree_unchanged && (settled.is_some() || !value["archivedAt"].is_null());
        let mut eligible = owner.deleted || old || settled_due;
        if !eligible && (owner.shell.branch.is_none() || !unchanged && !rules.worktree_on_merge) {
            return Err(Error::Refused("worktree is not due for cleanup"));
        }
        let requested = owner.shell.worktree_path.as_ref().ok_or(Error::Invalid)?;
        if owner.shell.branch.is_none() && !owner.deleted && self.is_scratch(path) {
            let reason = if old {
                format!("Inactive for {} days", (now - activity) / DAY_MS)
            } else {
                format!(
                    "Settled for {} days",
                    (now - settled.unwrap_or(now).max(activity)) / DAY_MS
                )
            };
            return self
                .clean_scratch(
                    snapshot,
                    group,
                    path,
                    Path::new(&requested.0),
                    rules,
                    dry_run,
                    reason,
                )
                .await;
        }
        let Some(branch) = owner.shell.branch.as_ref() else {
            return Err(Error::Refused("repository or branch changed"));
        };
        validate_cleanup_worktree(Path::new(&requested.0), &snapshot.managed_roots)?;
        validate_cleanup_worktree(path, &snapshot.managed_roots)?;
        protect(snapshot, path)?;
        protect(snapshot, Path::new(&requested.0))?;
        let git = t3_git::Git::new(path);
        let head = read_git(&git, &["rev-parse", "HEAD"]).await?;
        check_branch(&git, &branch.0).await?;
        local_changes(&git, rules).await?;
        let mut reason = if owner.deleted {
            "Thread was deleted".into()
        } else if old {
            format!("Inactive for {} days", (now - activity) / DAY_MS)
        } else {
            format!(
                "Settled for {} days",
                (now - settled.unwrap_or(now).max(activity)) / DAY_MS
            )
        };
        if !eligible {
            let project_git = t3_git::Git::new(&owner.project_path);
            let remotes = read_git(&project_git, &["remote"]).await?;
            let names: Vec<_> = remotes.lines().collect();
            if dry_run && !names.is_empty() {
                return Err(Error::Refused(
                    "preview requires remote eligibility checks; keeping worktree",
                ));
            }
            let default = if names.is_empty() {
                read_git(&project_git, &["symbolic-ref", "--quiet", "HEAD"])
                    .await?
                    .trim()
                    .to_owned()
            } else {
                let remote = if names.contains(&"origin") {
                    "origin"
                } else {
                    names[0]
                };
                let symbolic = execute_git(
                    &project_git,
                    &[
                        "symbolic-ref",
                        "--quiet",
                        &format!("refs/remotes/{remote}/HEAD"),
                    ],
                    true,
                )
                .await?;
                let default = if symbolic.code == Some(0) {
                    symbolic
                        .stdout_text()
                        .trim()
                        .strip_prefix(&format!("refs/remotes/{remote}/"))
                        .map(str::to_owned)
                } else {
                    let output = read_git(
                        &project_git,
                        &["ls-remote", "--symref", "--", remote, "HEAD"],
                    )
                    .await?;
                    output.lines().find_map(|l| {
                        l.strip_prefix("ref: refs/heads/")
                            .and_then(|s| s.strip_suffix("\tHEAD"))
                            .map(str::to_owned)
                    })
                }
                .ok_or(Error::Refused("default branch is unavailable"))?;
                read_git(
                    &project_git,
                    &[
                        "fetch",
                        "--no-tags",
                        "--",
                        remote,
                        &format!("+refs/heads/{default}:refs/remotes/{remote}/{default}"),
                    ],
                )
                .await?;
                format!("refs/remotes/{remote}/{default}")
            };
            let base = read_git(&git, &["rev-parse", &default]).await?;
            let integrated = execute_git(
                &git,
                &["merge-base", "--is-ancestor", head.trim(), base.trim()],
                true,
            )
            .await?
            .code
                == Some(0);
            eligible = integrated && unchanged;
            if eligible {
                reason = if names.is_empty() {
                    "No commits beyond the project branch".into()
                } else {
                    "No commits beyond the default branch".into()
                };
            }
            if !eligible && !names.is_empty() && rules.worktree_on_merge {
                let pr = self
                    .host
                    .branch_pull_request(&owner.project_path, &branch.0)
                    .await?;
                let default_branch = default.splitn(4, '/').nth(3).unwrap_or(&default);
                eligible = pull_request_merged(
                    pr.as_ref(),
                    &branch.0,
                    default_branch,
                    head.trim(),
                    integrated,
                );
                if eligible {
                    reason = "Pull request was merged".into();
                }
            }
        }
        if !eligible {
            return Err(Error::Refused(
                "worktree is not merged or has commits beyond the default branch",
            ));
        }
        let allowed = if path.join(".gitmodules").exists() {
            read_git(
                &git,
                &[
                    "submodule",
                    "foreach",
                    "--recursive",
                    "--quiet",
                    "printf \"%s\\0\" \"$toplevel/$sm_path\"",
                ],
            )
            .await?
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect()
        } else {
            Vec::new()
        };
        let measured_path = path.to_owned();
        let bytes = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::task::spawn_blocking(move || {
                measure_with_repositories(&measured_path, true, &allowed)
            }),
        )
        .await
        .map_err(|_| Error::Refused("worktree inspection timed out"))?
        .map_err(|_| Error::Service)??;
        if dry_run {
            return Ok((format!("Would remove: {reason}"), Some(bytes)));
        }
        let _lease = tokio::time::timeout(
            Duration::from_secs(30),
            self.host.acquire_workspace_lease(path),
        )
        .await
        .map_err(|_| Error::Refused("workspace is busy"))??;
        let latest = self.snapshot().await?;
        validate_cleanup_worktree(Path::new(&requested.0), &latest.managed_roots)?;
        validate_cleanup_worktree(path, &latest.managed_roots)?;
        protect(&latest, path)?;
        protect(&latest, Path::new(&requested.0))?;
        if latest.settings != snapshot.settings
            || group.iter().any(|old| !latest.threads.contains(old))
            || latest
                .threads
                .iter()
                .filter(|t| {
                    !t.deleted
                        && t.shell.worktree_path.as_ref().is_some_and(|p| {
                            dunce::canonicalize(&p.0).ok().as_deref() == Some(path)
                        })
                })
                .count()
                != group.iter().filter(|t| !t.deleted).count()
            || !latest.threads.contains(current)
        {
            return Err(Error::Refused(
                "thread activity or settings changed since check",
            ));
        }
        if latest.pending_deletion_threads.contains(&owner.shell.id) {
            return Err(Error::Refused("thread deletion is still pending"));
        }
        check_branch(&git, &branch.0).await?;
        if read_git(&git, &["rev-parse", "HEAD"]).await? != head {
            return Err(Error::Refused("commit changed since check"));
        }
        local_changes(&git, rules).await?;
        let admin = read_git(&git, &["rev-parse", "--absolute-git-dir"]).await?;
        let force = submodule_removal(&git, rules, Path::new(admin.trim())).await?;
        if !dry_run {
            if rules.worktree_keep_when == WorktreeKeepWhen::TrackedChanges {
                read_git(&git, &["clean", "-ffdx"]).await?;
            }
            if force {
                local_changes(&git, rules).await?;
            }
            t3_git::Git::new(&owner.project_path)
                .remove_worktree(path, force)
                .await
                .map_err(|_| Error::Service)?;
            self.host.invalidate_git_status(&owner.project_path).await;
        }
        Ok((
            if dry_run {
                format!("Would remove: {reason}")
            } else {
                reason
            },
            Some(bytes),
        ))
    }
    fn scratch_root(&self) -> Option<PathBuf> {
        dunce::canonicalize(self.state_dir.parent()?.join("scratch")).ok()
    }
    fn is_scratch(&self, path: &Path) -> bool {
        self.scratch_root()
            .is_some_and(|root| path.parent() == Some(root.as_path()))
    }
    async fn scratch_keep(
        &self,
        path: &Path,
        requested: &Path,
        rules: &WorktreeCleanupRules,
        group: &[&CleanupThread],
    ) -> Result<()> {
        let real_parent = dunce::canonicalize(requested.parent().ok_or(Error::Invalid)?)
            .map_err(|_| Error::Io)?;
        if dunce::canonicalize(requested).map_err(|_| Error::Io)?
            != real_parent.join(requested.file_name().ok_or(Error::Invalid)?)
        {
            return Err(Error::Refused("folder is a symbolic link"));
        }
        if !self.is_scratch(path) {
            return Err(Error::Refused("outside the scratch folder"));
        }
        if rules.worktree_keep_when != WorktreeKeepWhen::TrackedChanges {
            for entry in fs::read_dir(path).map_err(|_| Error::Io)? {
                if entry.map_err(|_| Error::Io)?.file_name() != "node_modules" {
                    return Err(Error::Refused("has local files"));
                }
            }
        }
        let scheduled = tokio::time::timeout(
            Duration::from_secs(30),
            self.host.enabled_scheduled_threads(),
        )
        .await
        .map_err(|_| Error::Refused("scheduled tasks are unavailable"))??;
        if group.iter().any(|t| scheduled.contains(&t.shell.id)) {
            return Err(Error::Refused("thread has a scheduled task"));
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    async fn clean_scratch(
        &self,
        snapshot: &CleanupSnapshot,
        group: &[&CleanupThread],
        path: &Path,
        requested: &Path,
        rules: &WorktreeCleanupRules,
        dry_run: bool,
        reason: String,
    ) -> Result<(String, Option<u64>)> {
        self.scratch_keep(path, requested, rules, group).await?;
        protect(snapshot, path)?;
        protect(snapshot, requested)?;
        let inspect = || {
            let path = path.to_owned();
            async move {
                tokio::time::timeout(
                    Duration::from_secs(30),
                    tokio::task::spawn_blocking(move || {
                        if path.join(".git").exists() {
                            return Err(Error::Refused("contains a nested Git repository"));
                        }
                        measure_worktree(&path, true)
                    }),
                )
                .await
                .map_err(|_| Error::Refused("worktree inspection timed out"))?
                .map_err(|_| Error::Service)?
            }
        };
        let bytes = inspect().await?;
        if dry_run {
            return Ok((format!("Would remove: {reason}"), Some(bytes)));
        }
        let _lease = tokio::time::timeout(
            Duration::from_secs(30),
            self.host.acquire_workspace_lease(path),
        )
        .await
        .map_err(|_| Error::Refused("workspace is busy"))??;
        inspect().await?;
        self.scratch_keep(path, requested, rules, group).await?;
        let latest = self.snapshot().await?;
        protect(&latest, path)?;
        protect(&latest, requested)?;
        let members = latest
            .threads
            .iter()
            .filter(|t| {
                !t.deleted
                    && t.shell
                        .worktree_path
                        .as_ref()
                        .is_some_and(|p| dunce::canonicalize(&p.0).ok().as_deref() == Some(path))
            })
            .count();
        if latest.settings != snapshot.settings
            || group.iter().any(|t| !latest.threads.contains(t))
            || members != group.len()
        {
            return Err(Error::Refused(
                "thread activity or settings changed since check",
            ));
        }
        if group
            .iter()
            .any(|t| latest.pending_deletion_threads.contains(&t.shell.id))
        {
            return Err(Error::Refused("thread deletion is still pending"));
        }
        let removal_path = path.to_owned();
        tokio::task::spawn_blocking(move || fs::remove_dir_all(removal_path))
            .await
            .map_err(|_| Error::Service)?
            .map_err(|_| Error::Io)?;
        Ok((reason, Some(bytes)))
    }
    async fn report_orphans(
        &self,
        snapshot: &CleanupSnapshot,
        now: i64,
        entries: &mut Vec<StorageCleanupReportEntry>,
    ) {
        let referenced: Vec<_> = snapshot
            .threads
            .iter()
            .filter_map(|t| t.shell.worktree_path.as_ref())
            .map(|p| dunce::canonicalize(&p.0).unwrap_or_else(|_| PathBuf::from(&p.0)))
            .collect();
        let mut registered: Option<Vec<PathBuf>> = None;
        let mut candidates = BTreeSet::new();
        let scratch_root = self.scratch_root();
        if let Some(root) = &scratch_root
            && let Ok(children) = fs::read_dir(root)
        {
            for child in children.flatten() {
                let path = child.path();
                if child
                    .file_type()
                    .is_ok_and(|t| t.is_dir() && !t.is_symlink())
                    && dunce::canonicalize(&path).ok().as_ref() == Some(&path)
                    && !referenced.iter().any(|p| occupies(&path, p))
                    && !snapshot.project_roots.iter().any(|p| occupies(&path, p))
                    && child
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .is_some_and(|d| d.as_millis() as i128 <= i128::from(now - 3_600_000))
                {
                    candidates.insert(path);
                }
            }
        }
        for root in &snapshot.managed_roots {
            let Ok(root) = dunce::canonicalize(root) else {
                continue;
            };
            if root.parent().is_none() {
                continue;
            }
            let mut pending = vec![(root.clone(), 0)];
            while let Some((folder, depth)) = pending.pop() {
                let Ok(children) = fs::read_dir(&folder) else {
                    continue;
                };
                for child in children.flatten() {
                    let path = child.path();
                    let Ok(metadata) = fs::symlink_metadata(&path) else {
                        continue;
                    };
                    if !metadata.is_dir()
                        || metadata.file_type().is_symlink()
                        || dunce::canonicalize(&path).ok().as_ref() != Some(&path)
                        || snapshot
                            .project_roots
                            .iter()
                            .any(|p| occupies(p, &path) && !root.starts_with(p))
                    {
                        continue;
                    }
                    if referenced.iter().any(|p| p == &path) {
                        continue;
                    }
                    if depth == 0 && path.join(".git").is_dir() {
                        continue;
                    }
                    if depth == 0 && !path.join(".git").exists() {
                        pending.push((path, depth + 1));
                        continue;
                    }
                    let mut targets = Vec::new();
                    if !path.join(".git").exists() {
                        if registered.is_none() {
                            let mut paths = Vec::new();
                            for project in &snapshot.project_roots {
                                if let Ok(worktrees) =
                                    t3_git::Git::new(project).list_worktrees().await
                                {
                                    paths.extend(worktrees.into_iter().map(|w| w.path));
                                }
                            }
                            registered = Some(paths);
                        }
                        for nested in registered
                            .as_ref()
                            .into_iter()
                            .flatten()
                            .filter(|p| p.starts_with(&path) && *p != &path)
                        {
                            if nested.join(".git").is_file()
                                && dunce::canonicalize(nested).ok().as_ref() == Some(nested)
                            {
                                targets.push(nested.clone());
                            }
                        }
                    }
                    if targets.is_empty() {
                        targets.push(path);
                    }
                    for path in targets {
                        if referenced
                            .iter()
                            .any(|p| p == &path || p.starts_with(&path))
                            || snapshot.project_roots.iter().any(|p| occupies(&path, p))
                        {
                            continue;
                        }
                        let Ok(metadata) = fs::metadata(&path) else {
                            continue;
                        };
                        if metadata
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                            .is_some_and(|d| d.as_millis() as i128 > i128::from(now - 3_600_000))
                        {
                            continue;
                        }
                        candidates.insert(path);
                    }
                }
            }
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut first = true;
        let mut sizes = self.orphan_sizes.lock().await;
        for path in &candidates {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if !sizes.contains_key(path) && (first || !remaining.is_zero()) {
                let measured_path = path.clone();
                let timeout = if first {
                    Duration::from_secs(30)
                } else {
                    remaining
                };
                first = false;
                let measured = tokio::time::timeout(
                    timeout,
                    tokio::task::spawn_blocking(move || measure_worktree(&measured_path, false)),
                )
                .await;
                if let Ok(result) = measured {
                    sizes.insert(path.clone(), result.ok().and_then(Result::ok));
                } else if timeout == Duration::from_secs(30) {
                    sizes.insert(path.clone(), None);
                }
            }
            let mut entry = blank(StorageCleanupReportEntryKind::Worktree, Some(path));
            entry.reason = if path.parent() == scratch_root.as_deref() {
                "No thread uses this folder".into()
            } else if path.join(".git").is_file() {
                "No thread uses this worktree".into()
            } else {
                "No thread uses this folder, and it isn't a git worktree".into()
            };
            entry.bytes = sizes.get(path).copied().flatten().map(number);
            entries.push(entry);
        }
        sizes.retain(|path, _| candidates.contains(path));
    }
}
fn enabled(r: &WorktreeCleanupRules) -> bool {
    r.worktree_after_days.is_some()
        || r.worktree_settled_after_days.is_some()
        || r.worktree_on_merge
        || r.worktree_on_delete
        || r.worktree_unchanged
}
fn blank(kind: StorageCleanupReportEntryKind, path: Option<&Path>) -> StorageCleanupReportEntry {
    StorageCleanupReportEntry {
        kind,
        outcome: StorageCleanupReportEntryOutcome::Kept,
        reason: String::new(),
        path: path.map(|p| p.to_string_lossy().into_owned()),
        project_path: None,
        thread_id: None,
        thread_title: None,
        bytes: None,
        files: None,
    }
}
fn sentence(reason: &str) -> String {
    let mut chars = reason.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}
fn rotated_log(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, number)| {
        !number.is_empty()
            && number.bytes().all(|b| b.is_ascii_digit())
            && (stem.ends_with(".log") || stem.ends_with(".ndjson"))
    })
}
fn occupies(root: &Path, path: &Path) -> bool {
    let roots = [
        path_key(root),
        dunce::canonicalize(root)
            .map(|p| path_key(&p))
            .unwrap_or_else(|_| path_key(root)),
    ];
    let targets = [
        path_key(path),
        dunce::canonicalize(path)
            .map(|p| path_key(&p))
            .unwrap_or_else(|_| path_key(path)),
    ];
    roots.iter().any(|root| {
        targets
            .iter()
            .any(|target| target == root || target.starts_with(root))
    })
}
fn path_key(path: &Path) -> PathBuf {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    #[cfg(windows)]
    {
        PathBuf::from(path.to_string_lossy().to_lowercase())
    }
    #[cfg(not(windows))]
    {
        path
    }
}
fn protect(snapshot: &CleanupSnapshot, path: &Path) -> Result<()> {
    if snapshot.project_roots.iter().any(|p| occupies(path, p)) {
        return Err(Error::Refused("contains a project checkout"));
    }
    if snapshot.terminal_paths.iter().any(|p| occupies(path, p)) {
        return Err(Error::Refused("open terminal"));
    }
    if snapshot
        .live_provider_paths
        .iter()
        .any(|p| occupies(path, p))
    {
        return Err(Error::Refused("provider session is still open"));
    }
    Ok(())
}
fn owner<'a>(group: &[&'a CleanupThread]) -> Option<&'a CleanupThread> {
    if group.len() == 1 {
        return Some(group[0]);
    }
    let values: Vec<_> = group
        .iter()
        .map(|t| shell_value(&t.shell).ok())
        .collect::<Option<_>>()?;
    let map: BTreeMap<_, _> = group
        .iter()
        .enumerate()
        .map(|(i, t)| (t.shell.id.0.as_str(), i))
        .collect();
    let tops: Vec<_> = values
        .iter()
        .enumerate()
        .filter(|(_, v)| {
            !v["lineage"]["parentThreadId"]
                .as_str()
                .is_some_and(|id| map.contains_key(id))
        })
        .map(|(i, _)| i)
        .collect();
    if tops.len() != 1 {
        return None;
    }
    let top = tops[0];
    for mut i in 0..group.len() {
        let mut seen = BTreeSet::new();
        while i != top {
            let lineage = &values[i]["lineage"];
            if !seen.insert(i)
                || lineage["relationshipToParent"] != "subagent" && lineage["sideChat"] != true
                || lineage["rootThreadId"] != values[top]["lineage"]["rootThreadId"]
            {
                return None;
            }
            i = *map.get(lineage["parentThreadId"].as_str()?)?;
        }
    }
    Some(group[top])
}
async fn execute_git(
    git: &t3_git::Git,
    args: &[&str],
    allow_nonzero: bool,
) -> Result<t3_process::ProcessOutput> {
    let mut options = t3_git::ExecuteOptions {
        allow_nonzero,
        max_output_bytes: 1024 * 1024,
        output_mode: t3_git::OutputMode::Error,
        timeout: Some(Duration::from_secs(30)),
        ..Default::default()
    };
    options.env.insert("LC_ALL".into(), Some("C".into()));
    options
        .env
        .insert("GIT_TERMINAL_PROMPT".into(), Some("0".into()));
    options.env.insert("GIT_ASKPASS".into(), Some("".into()));
    git.execute(args, options).await.map_err(|_| Error::Service)
}
async fn read_git(git: &t3_git::Git, args: &[&str]) -> Result<String> {
    execute_git(git, args, false).await.map(|o| o.stdout_text())
}
async fn check_branch(git: &t3_git::Git, branch: &str) -> Result<()> {
    let index = read_git(git, &["rev-parse", "--git-path", "index"]).await?;
    if git.cwd().join(format!("{}.lock", index.trim())).exists() {
        return Err(Error::Refused("Git index is locked"));
    }
    let symbolic = execute_git(git, &["symbolic-ref", "--quiet", "HEAD"], true).await?;
    if symbolic.code != Some(0) || symbolic.stdout_text().trim() != format!("refs/heads/{branch}") {
        return Err(Error::Refused("repository or branch changed"));
    }
    Ok(())
}
#[derive(Default)]
struct HiddenIndex {
    pending: Vec<u8>,
    flagged: Vec<(String, String, String)>,
    overflow: bool,
}
impl HiddenIndex {
    fn push(&mut self, chunk: &[u8], sparse: bool) {
        for byte in chunk {
            if *byte == b'\n' {
                let line = String::from_utf8_lossy(&self.pending);
                let metadata = line.split_once('\t');
                if let Some((header, name)) = metadata {
                    let fields: Vec<_> = header.split_whitespace().collect();
                    if fields.len() == 4
                        && fields[0].len() == 1
                        && (fields[0].as_bytes()[0].is_ascii_lowercase()
                            || fields[0] == "S" && !sparse)
                        && fields[1] != "160000"
                    {
                        if self.flagged.len() < 10_000 {
                            self.flagged
                                .push((fields[1].into(), fields[2].into(), name.into()));
                        } else {
                            self.overflow = true;
                        }
                    }
                }
                self.pending.clear();
            } else if self.pending.len() < 1024 * 1024 {
                self.pending.push(*byte);
            } else {
                self.overflow = true;
            }
        }
    }
}
async fn hidden_tracked_changes(git: &t3_git::Git, sparse: bool) -> Result<()> {
    let scan = Arc::new(std::sync::Mutex::new(HiddenIndex::default()));
    let callback = scan.clone();
    git.execute(
        &["-c", "core.quotePath=false", "ls-files", "-s", "-v"],
        t3_git::ExecuteOptions {
            max_output_bytes: 64 * 1024,
            output_mode: t3_git::OutputMode::Truncate,
            on_stdout_chunk: Some(Arc::new(move |chunk| {
                callback
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(chunk, sparse)
            })),
            ..Default::default()
        },
    )
    .await
    .map_err(|_| Error::Service)?;
    let entries = {
        let mut scan = scan
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !scan.pending.is_empty() {
            scan.push(b"\n", sparse);
        }
        if scan.overflow {
            return Err(Error::Refused(
                "hidden tracked files exceed the inspection limit",
            ));
        }
        std::mem::take(&mut scan.flagged)
    };
    let mut files = Vec::new();
    let mut link_files = Vec::new();
    for (mode, expected, name) in entries {
        if name.starts_with('"') || name.contains('\u{fffd}') {
            return Err(Error::Refused("hidden tracked file could not be inspected"));
        }
        let metadata = match fs::symlink_metadata(git.cwd().join(&name)) {
            Ok(info) => info,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                continue;
            }
            Err(_) => return Err(Error::Io),
        };
        if metadata.file_type().is_symlink() {
            if mode != "120000" {
                return Err(Error::Refused("hidden tracked file changed type"));
            }
            let target = fs::read_link(git.cwd().join(&name)).map_err(|_| Error::Io)?;
            let output = git
                .execute(
                    &["hash-object", "--stdin"],
                    t3_git::ExecuteOptions {
                        stdin: Some(target.to_string_lossy().as_bytes().to_vec()),
                        max_output_bytes: 1024 * 1024,
                        output_mode: t3_git::OutputMode::Error,
                        ..Default::default()
                    },
                )
                .await
                .map_err(|_| Error::Service)?;
            if output.stdout_text().trim() != expected {
                return Err(Error::Refused(
                    "has uncommitted changes hidden from git status",
                ));
            }
        } else if metadata.is_file() {
            if mode == "120000" {
                link_files.push((expected, name));
            } else {
                files.push((expected, name));
            }
        } else {
            return Err(Error::Refused("hidden tracked file changed type"));
        }
    }
    for (entries, no_filters) in [(files, false), (link_files, true)] {
        if entries.is_empty() {
            continue;
        }
        let args = if no_filters {
            vec!["hash-object", "--no-filters", "--stdin-paths"]
        } else {
            vec!["hash-object", "--stdin-paths"]
        };
        let stdin = entries
            .iter()
            .map(|(_, name)| format!("{name}\n"))
            .collect::<String>()
            .into_bytes();
        let output = git
            .execute(
                &args,
                t3_git::ExecuteOptions {
                    stdin: Some(stdin),
                    max_output_bytes: 1024 * 1024,
                    output_mode: t3_git::OutputMode::Error,
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| Error::Service)?;
        let hashes = output.stdout_text();
        if entries
            .iter()
            .zip(hashes.lines())
            .any(|((expected, _), actual)| expected != actual)
            || hashes.lines().count() != entries.len()
        {
            return Err(Error::Refused(
                "has uncommitted changes hidden from git status",
            ));
        }
    }
    Ok(())
}
async fn local_changes(git: &t3_git::Git, rules: &WorktreeCleanupRules) -> Result<()> {
    let untracked = if rules.worktree_keep_when == WorktreeKeepWhen::TrackedChanges {
        "--untracked-files=no"
    } else {
        "--untracked-files=all"
    };
    if !read_git(
        git,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--ignore-submodules=none",
            untracked,
        ],
    )
    .await?
    .is_empty()
    {
        return Err(Error::Refused("has uncommitted changes"));
    }
    let sparse = read_git(
        git,
        &[
            "config",
            "--type=bool",
            "--default=false",
            "core.sparseCheckout",
        ],
    )
    .await?;
    hidden_tracked_changes(git, sparse.trim() == "true").await?;
    if rules.worktree_keep_when == WorktreeKeepWhen::AnyLocalFiles {
        let files = read_git(
            git,
            &[
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
                "-z",
            ],
        )
        .await?;
        if files.split('\0').any(|name| {
            !name.is_empty() && name != "node_modules/" && !name.ends_with("/node_modules/")
        }) {
            return Err(Error::Refused("has ignored files"));
        }
    }
    Ok(())
}
async fn submodule_removal(
    git: &t3_git::Git,
    rules: &WorktreeCleanupRules,
    admin: &Path,
) -> Result<bool> {
    let modules = admin.join("modules");
    let stored = modules.try_exists().map_err(|_| Error::Io)?;
    if !stored && !git.cwd().join(".gitmodules").exists() {
        return Ok(false);
    }
    let populated = read_git(
        git,
        &[
            "submodule",
            "foreach",
            "--recursive",
            "--quiet",
            "printf \"%s\\0\" \"$toplevel/$sm_path\"",
        ],
    )
    .await?;
    let paths: Vec<_> = populated
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect();
    let mut pending = vec![modules];
    for path in &paths {
        pending.push(PathBuf::from(
            read_git(
                &t3_git::Git::new(path),
                &["rev-parse", "--absolute-git-dir"],
            )
            .await?
            .trim(),
        ));
    }
    let mut visited = BTreeSet::new();
    while let Some(directory) = pending.pop() {
        if !directory.exists() {
            continue;
        }
        let canonical = dunce::canonicalize(&directory).map_err(|_| Error::Io)?;
        if !visited.insert(canonical) {
            continue;
        }
        if visited.len() > 10_000 {
            return Err(Error::Refused("submodule inspection exceeded the limit"));
        }
        if !directory.is_dir() {
            return Err(Error::Refused(
                "submodule repository could not be inspected",
            ));
        }
        if directory.join("HEAD").exists() {
            let unpushed = read_git(
                git,
                &[
                    "--git-dir",
                    directory.to_str().ok_or(Error::Invalid)?,
                    "--work-tree",
                    git.cwd().to_str().ok_or(Error::Invalid)?,
                    "rev-list",
                    "--max-count=1",
                    "HEAD",
                    "--all",
                    "--reflog",
                    "--not",
                    "--remotes",
                ],
            )
            .await?;
            if !unpushed.trim().is_empty() {
                return Err(Error::Refused(
                    "submodule has commits not on a remote-tracking ref",
                ));
            }
            pending.push(directory.join("modules"));
        } else {
            for name in fs::read_dir(&directory).map_err(|_| Error::Io)? {
                pending.push(name.map_err(|_| Error::Io)?.path());
            }
        }
    }
    for path in &paths {
        local_changes(&t3_git::Git::new(path), rules).await?;
    }
    let force = stored || !paths.is_empty();
    if force && rules.worktree_keep_when != WorktreeKeepWhen::TrackedChanges {
        let root = dunce::canonicalize(git.cwd()).map_err(|_| Error::Io)?;
        let mut unchecked = Vec::new();
        for repository in std::iter::once(&root).chain(paths.iter()) {
            let index = read_git(
                &t3_git::Git::new(repository),
                &["ls-files", "--stage", "-z"],
            )
            .await?;
            for record in index.split('\0').filter(|r| r.starts_with("160000 ")) {
                let name = record.split_once('\t').ok_or(Error::Invalid)?.1;
                let target = repository.join(name);
                if !target.starts_with(&root) {
                    return Err(Error::Refused("submodules could not be inspected"));
                }
                if !paths.contains(&target) && target.exists() {
                    unchecked.push(target);
                }
            }
        }
        let mut inspected = unchecked.len();
        while let Some(target) = unchecked.pop() {
            if inspected > 10_000 {
                return Err(Error::Refused("submodule inspection exceeded the limit"));
            }
            if dunce::canonicalize(&target).map_err(|_| Error::Io)? != target || !target.is_dir() {
                return Err(Error::Refused("submodule has local files"));
            }
            if target.file_name().is_some_and(|n| n == "node_modules") {
                continue;
            }
            for name in fs::read_dir(&target).map_err(|_| Error::Io)? {
                inspected += 1;
                unchecked.push(name.map_err(|_| Error::Io)?.path());
            }
        }
    }
    Ok(force)
}
pub fn measure_worktree(root: &Path, inspect_repositories: bool) -> Result<u64> {
    measure_with_repositories(root, inspect_repositories, &[])
}
fn measure_with_repositories(
    root: &Path,
    inspect_repositories: bool,
    allowed: &[PathBuf],
) -> Result<u64> {
    if network_path(root) {
        return Err(Error::Refused("network paths cannot be inspected"));
    }
    let root = dunce::canonicalize(root).map_err(|_| Error::Io)?;
    #[cfg(windows)]
    let root_directory =
        Dir::open_ambient_dir(&root, cap_std::ambient_authority()).map_err(|_| Error::Io)?;
    let mut pending = vec![root.clone()];
    let mut entries = 0;
    let mut bytes = 0_u64;
    let skipped = shared_pnpm_virtual_store(&root);
    let start = std::time::Instant::now();
    while let Some(path) = pending.pop() {
        entries += 1;
        if entries > 2_000_000 {
            return Err(Error::Refused("worktree could not be fully inspected"));
        }
        #[cfg(not(windows))]
        let metadata = fs::symlink_metadata(&path).map_err(|_| Error::Io)?;
        #[cfg(windows)]
        let metadata = if path == root {
            root_directory.dir_metadata()
        } else {
            root_directory.symlink_metadata(path.strip_prefix(&root).map_err(|_| Error::Invalid)?)
        }
        .map_err(|_| Error::Io)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if dunce::canonicalize(&path).map_err(|_| Error::Io)? != path || !path.starts_with(&root) {
            return Err(Error::Refused("worktree could not be fully inspected"));
        }
        if start.elapsed() > Duration::from_secs(30) {
            return Err(Error::Refused("worktree could not be fully inspected"));
        }
        let in_store = skipped.as_ref().is_some_and(|s| path.starts_with(s));
        if in_store && !inspect_repositories {
            continue;
        }
        if metadata.is_dir() {
            let names = fs::read_dir(&path)
                .map_err(|_| Error::Io)?
                .collect::<std::io::Result<Vec<_>>>()
                .map_err(|_| Error::Io)?;
            if inspect_repositories
                && path != root
                && !allowed.contains(&path)
                && (path.join(".git").exists()
                    || path.join("HEAD").exists()
                        && path.join("objects").exists()
                        && path.join("refs").exists())
            {
                return Err(Error::Refused("contains a nested Git repository"));
            }
            pending.extend(
                names
                    .into_iter()
                    .filter(|e| !in_store || e.file_type().is_ok_and(|t| t.is_dir()))
                    .map(|e| e.path()),
            );
        } else if metadata.is_file() {
            if in_store {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.nlink() == 1 {
                    bytes = bytes.saturating_add(metadata.blocks().saturating_mul(512));
                }
            }
            #[cfg(not(unix))]
            {
                use cap_fs_ext::MetadataExt;
                if metadata.nlink() == 1 {
                    bytes = bytes.saturating_add(metadata.len());
                }
            }
        }
    }
    Ok(bytes)
}
fn network_path(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text.starts_with("//") || text.starts_with("\\\\")
}
fn shared_pnpm_virtual_store(root: &Path) -> Option<PathBuf> {
    use cap_fs_ext::MetadataExt;
    let raw = fs::read_to_string(root.join("node_modules/.modules.yaml")).ok()?;
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Manifest {
        store_dir: String,
        virtual_store_dir: Option<String>,
    }
    let manifest: Manifest = serde_yaml_ng::from_str(&raw).ok()?;
    let store = PathBuf::from(manifest.store_dir);
    let virtual_store = PathBuf::from(manifest.virtual_store_dir.unwrap_or_else(|| ".pnpm".into()));
    if network_path(&store) || network_path(&virtual_store) {
        return None;
    }
    let virtual_store = root.join("node_modules").join(virtual_store);
    let store = Dir::open_ambient_dir(store, cap_std::ambient_authority())
        .ok()?
        .dir_metadata()
        .ok()?;
    let virtual_info = Dir::open_ambient_dir(&virtual_store, cap_std::ambient_authority())
        .ok()?
        .dir_metadata()
        .ok()?;
    (store.dev() == virtual_info.dev()).then_some(virtual_store)
}
