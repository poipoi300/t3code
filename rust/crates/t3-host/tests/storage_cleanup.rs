use async_trait::async_trait;
use std::{
    collections::BTreeSet,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
    },
};
use t3_contracts::{orchestration_v2::OrchestrationV2ThreadShell, settings::*};
use t3_host::{Result, background::Clock, storage_cleanup::*};
use tokio::sync::{Mutex, OwnedMutexGuard};
struct Time(AtomicI64);
impl Clock for Time {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
struct Host {
    state: Mutex<CleanupSnapshot>,
    lease: Arc<Mutex<()>>,
    open_terminal_on_lease: AtomicBool,
    snapshot_reads: AtomicUsize,
    lease_acquisitions: AtomicUsize,
    lease_requested: tokio::sync::Notify,
    pr_lookups: AtomicUsize,
    pr_path: Mutex<Option<std::path::PathBuf>>,
    disable_after_snapshot_under_lease: AtomicBool,
    lease_change: Mutex<Option<&'static str>>,
    pull_request: Mutex<Option<MergedPullRequest>>,
    unavailable: Mutex<Option<&'static str>>,
    scheduled: Mutex<BTreeSet<t3_contracts::base_schemas::ThreadId>>,
}
#[async_trait]
impl CleanupHost for Host {
    async fn snapshot(&self) -> Result<CleanupSnapshot> {
        self.snapshot_reads.fetch_add(1, Ordering::SeqCst);
        if let Some(source) = *self.unavailable.lock().await {
            return Err(t3_host::Error::Refused(source));
        }
        let mut state = self.state.lock().await;
        let snapshot = state.clone();
        if self.lease_acquisitions.load(Ordering::SeqCst) > 0
            && self
                .disable_after_snapshot_under_lease
                .swap(false, Ordering::SeqCst)
        {
            state.settings.storage_cleanup.worktree_after_days = None;
        }
        Ok(snapshot)
    }
    async fn enabled_scheduled_threads(
        &self,
    ) -> Result<BTreeSet<t3_contracts::base_schemas::ThreadId>> {
        Ok(self.scheduled.lock().await.clone())
    }
    async fn storage_cleanup_settings(&self) -> Result<StorageCleanupSettings> {
        Ok(self.state.lock().await.settings.storage_cleanup.clone())
    }
    async fn acquire_workspace_lease(&self, path: &Path) -> Result<OwnedMutexGuard<()>> {
        self.lease_acquisitions.fetch_add(1, Ordering::SeqCst);
        self.lease_requested.notify_one();
        if let Some(change) = self.lease_change.lock().await.take() {
            let mut state = self.state.lock().await;
            match change {
                "unavailable" => {
                    *self.unavailable.lock().await = Some("provider sessions unavailable");
                }
                "ancestor" => {
                    state.threads[1].shell.status =
                        t3_contracts::orchestration_v2::OrchestrationV2ShellThreadStatus::Running
                }
                "settings" => state.settings.storage_cleanup.worktree_after_days = None,
                "thread" => {
                    state.threads[0].shell.active_run_id = Some(t3_contracts::base_schemas::RunId(
                        "started-during-check".into(),
                    ))
                }
                "deletion" => {
                    let id = state.threads[0].shell.id.clone();
                    state.pending_deletion_threads.insert(id);
                }
                _ => unreachable!(),
            }
        }
        if self.open_terminal_on_lease.swap(false, Ordering::SeqCst) {
            self.state.lock().await.terminal_paths.push(path.into());
        }
        Ok(self.lease.clone().lock_owned().await)
    }
    async fn branch_pull_request(&self, cwd: &Path, _: &str) -> Result<Option<MergedPullRequest>> {
        *self.pr_path.lock().await = Some(cwd.into());
        self.pr_lookups.fetch_add(1, Ordering::SeqCst);
        Ok(self.pull_request.lock().await.clone())
    }
    async fn invalidate_git_status(&self, _: &Path) {}
}
fn host() -> Arc<Host> {
    Arc::new(Host {
        state: Mutex::new(CleanupSnapshot {
            settings: ServerSettings::default(),
            threads: vec![],
            project_roots: vec![],
            deleted_project_roots: vec![],
            terminal_paths: vec![],
            live_provider_paths: vec![],
            pending_deletion_threads: BTreeSet::new(),
            managed_roots: vec![],
        }),
        lease: Arc::new(Mutex::new(())),
        open_terminal_on_lease: AtomicBool::new(false),
        snapshot_reads: AtomicUsize::new(0),
        lease_acquisitions: AtomicUsize::new(0),
        lease_requested: Default::default(),
        pr_lookups: AtomicUsize::new(0),
        pr_path: Mutex::default(),
        disable_after_snapshot_under_lease: AtomicBool::default(),
        lease_change: Mutex::new(None),
        pull_request: Mutex::new(None),
        unavailable: Mutex::new(None),
        scheduled: Mutex::new(BTreeSet::new()),
    })
}
fn write_old(path: &Path, bytes: &[u8], at: i64) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
    filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(at / 1000, 0)).unwrap();
}
const NOW: i64 = 1_800_000_000_000;
fn shell(path: &Path) -> OrchestrationV2ThreadShell {
    let mut value: serde_json::Value = serde_json::from_str(
        include_str!("../../../fixtures/samples/OrchestrationV2ThreadShell.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    for(k,v)in serde_json::json!({"id":"thread","projectId":"project","title":"Thread","branch":"feature","worktreePath":path.to_string_lossy(),"activeRunId":null,"latestRunId":null,"status":"idle","pendingBackgroundTasks":[],"pendingRuntimeRequest":null,"latestUserMessageAt":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"createdAt":"2020-01-01T00:00:00.000Z","updatedAt":"2020-01-01T00:00:00.000Z","archivedAt":null,"settledAt":null,"settledOverride":null,"usageLimitResetAt":null,"limitRecovery":null,"lineage":{"rootThreadId":"thread","parentThreadId":null,"relationshipToParent":null}}).as_object().unwrap(){value[k]=v.clone();}
    serde_json::from_value(value).unwrap()
}
#[tokio::test]
async fn file_retention_is_strict_recursive_only_for_logs_and_dry_run_does_not_publish() {
    let temp = tempfile::tempdir().unwrap();
    let h = host();
    {
        let mut s = h.state.lock().await;
        s.settings.storage_cleanup.logs_after_days = Some(1);
        s.settings.storage_cleanup.browser_artifacts_after_days = Some(1);
    }
    let service = StorageCleanup::new(
        temp.path().into(),
        h.clone(),
        Arc::new(Time(AtomicI64::new(NOW))),
    );
    let mut stream = service.reports().await;
    assert!(stream.recv().await.unwrap().is_none());
    for (name, at) in [
        ("logs/nested/a.log.1", NOW - 86_400_001),
        ("logs/live.log", 0),
        ("logs/a.log.2", NOW - 86_400_000),
        ("logs/x.txt.1", 0),
        ("browser-artifacts/old.png", 0),
        ("browser-artifacts/before-epoch.png", -86_400_000),
        ("browser-artifacts/nested/kept.png", 0),
    ] {
        write_old(&temp.path().join(name), b"abc", at);
    }
    let preview = service.preview().await.unwrap();
    assert_eq!(preview.bytes_freed, WireNumber::Finite(0.into()));
    assert!(temp.path().join("browser-artifacts/old.png").exists());
    assert!(service.latest_report().await.is_none());
    assert!(stream.try_recv().is_err());
    h.snapshot_reads.store(0, Ordering::SeqCst);
    let report = service.run_now().await.unwrap();
    assert_eq!(
        h.snapshot_reads.load(Ordering::SeqCst),
        1,
        "expired files only need fresh settings, not fresh projections"
    );
    assert_eq!(report.bytes_freed, WireNumber::Finite(9.into()));
    assert!(!temp.path().join("logs/nested/a.log.1").exists());
    assert!(!temp.path().join("browser-artifacts/old.png").exists());
    assert!(
        !temp
            .path()
            .join("browser-artifacts/before-epoch.png")
            .exists()
    );
    for name in [
        "logs/live.log",
        "logs/a.log.2",
        "logs/x.txt.1",
        "browser-artifacts/nested/kept.png",
    ] {
        assert!(temp.path().join(name).exists(), "{name}");
    }
    assert_eq!(stream.recv().await.unwrap(), Some(report));
}
#[tokio::test]
async fn junction_or_symlink_cleanup_roots_refuse_outside_deletion() {
    let data = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write_old(&outside.path().join("valuable.log.1"), b"keep", 0);
    #[cfg(windows)]
    junction::create(outside.path(), data.path().join("logs")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), data.path().join("logs")).unwrap();
    let h = host();
    h.state
        .lock()
        .await
        .settings
        .storage_cleanup
        .logs_after_days = Some(1);
    let service = StorageCleanup::new(data.path().into(), h, Arc::new(Time(AtomicI64::new(NOW))));
    let report = service.run_now().await.unwrap();
    assert_eq!(report.counts.failed, WireNumber::Finite(0.into()));
    assert_eq!(report.counts.kept, WireNumber::Finite(1.into()));
    assert!(
        report
            .entries
            .iter()
            .any(|e| e.reason.contains("symbolic link"))
    );
    assert_eq!(
        std::fs::read(outside.path().join("valuable.log.1")).unwrap(),
        b"keep"
    );
}
#[tokio::test]
async fn nested_junctions_and_symlink_files_are_not_followed() {
    let data = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write_old(&outside.path().join("valuable.log.1"), b"keep", 0);
    std::fs::create_dir(data.path().join("logs")).unwrap();
    #[cfg(windows)]
    junction::create(outside.path(), data.path().join("logs/nested")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), data.path().join("logs/nested")).unwrap();
    let h = host();
    h.state
        .lock()
        .await
        .settings
        .storage_cleanup
        .logs_after_days = Some(1);
    let service = StorageCleanup::new(data.path().into(), h, Arc::new(Time(AtomicI64::new(NOW))));
    service.run_now().await.unwrap();
    assert!(outside.path().join("valuable.log.1").exists());
}
#[test]
fn active_pending_queued_and_limit_recovery_threads_are_kept() {
    let base = shell(Path::new("C:/worktrees/project/feature"));
    let settings = ServerSettings::default();
    assert!(thread_idle(&base, NOW, &settings).unwrap());
    for (status, active) in [
        ("running", false),
        ("starting", false),
        ("preparing", false),
        ("waiting", false),
        ("queued", false),
        ("idle", true),
    ] {
        let mut value = t3_contracts::stored::to_value(&base).unwrap();
        value["status"] = status.into();
        if active {
            value["activeRunId"] = "run".into();
        }
        let thread = serde_json::from_value(value).unwrap();
        assert!(!thread_idle(&thread, NOW, &settings).unwrap());
    }
    let mut value = t3_contracts::stored::to_value(&base).unwrap();
    value["latestUserMessageAt"] = chrono::DateTime::from_timestamp_millis(NOW - 1000)
        .unwrap()
        .to_rfc3339()
        .into();
    let thread = serde_json::from_value(value).unwrap();
    assert!(!thread_idle(&thread, NOW, &settings).unwrap());
}
#[test]
fn activity_ignores_metadata_refreshes_and_merge_requires_exact_head() {
    let mut thread = shell(Path::new("C:/worktrees/project/feature"));
    let activity = activity_at(&thread).unwrap();
    thread.updated_at.0 = "2099-01-01T00:00:00.000Z".into();
    assert_eq!(activity_at(&thread).unwrap(), activity);
    let mut pr = MergedPullRequest {
        state: "merged".into(),
        head_ref: "feature".into(),
        base_ref: "main".into(),
        head_sha: "head".into(),
    };
    assert!(pull_request_merged(
        Some(&pr),
        "feature",
        "main",
        "head",
        false
    ));
    pr.head_sha = "stale".into();
    assert!(!pull_request_merged(
        Some(&pr),
        "feature",
        "main",
        "head",
        false
    ));
    assert!(pull_request_merged(
        Some(&pr),
        "feature",
        "main",
        "head",
        true
    ));
}
async fn git(cwd: &Path, args: &[&str]) {
    git_text(cwd, args).await;
}
async fn git_text(cwd: &Path, args: &[&str]) -> String {
    let mut options = t3_process::ProcessOptions::new("git").args(args.iter().copied());
    options.cwd = Some(cwd.into());
    for (k, v) in [("GIT_CONFIG_GLOBAL", "NUL"), ("GIT_CONFIG_NOSYSTEM", "1")] {
        options.env.insert(k.into(), Some(v.into()));
    }
    t3_process::run(options).await.unwrap().stdout_text()
}
#[tokio::test]
async fn real_linked_worktree_is_removed_only_when_clean_and_inside_managed_roots() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]).await;
    std::fs::write(repo.join("tracked"), "original\n").unwrap();
    git(&repo, &["add", "tracked"]).await;
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "root",
        ],
    )
    .await;
    let root = temp.path().join("worktrees");
    let path = root.join("repo/feature");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    git(
        &repo,
        &["worktree", "add", "-b", "feature", path.to_str().unwrap()],
    )
    .await;
    let h = host();
    {
        let mut state = h.state.lock().await;
        state.settings.storage_cleanup.worktree_after_days = Some(1);
        state.managed_roots = vec![root];
        state.project_roots = vec![repo.clone()];
        state.threads = vec![CleanupThread {
            shell: shell(&path),
            deleted: false,
            project_path: Some(repo.clone()),
        }];
    }
    let service = StorageCleanup::new(
        temp.path().join("state"),
        h.clone(),
        Arc::new(Time(AtomicI64::new(NOW))),
    );
    let alias = path.parent().unwrap().join("alias");
    #[cfg(windows)]
    junction::create(&path, &alias).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    h.state.lock().await.threads[0].shell.worktree_path = Some(
        t3_contracts::base_schemas::TrimmedNonEmptyString(alias.to_string_lossy().into_owned()),
    );
    let refused = service.run_now().await.unwrap();
    assert!(path.exists());
    assert!(
        refused
            .entries
            .iter()
            .any(|entry| entry.reason.contains("symbolic link"))
    );
    h.state.lock().await.threads[0].shell.worktree_path = Some(
        t3_contracts::base_schemas::TrimmedNonEmptyString(path.to_string_lossy().into_owned()),
    );
    std::fs::write(path.join("untracked"), "keep").unwrap();
    let report = service.run_now().await.unwrap();
    assert!(path.exists());
    assert!(
        report
            .entries
            .iter()
            .any(|e| e.reason.contains("uncommitted"))
    );
    std::fs::remove_file(path.join("untracked")).unwrap();
    for flag in ["--assume-unchanged", "--skip-worktree"] {
        git(&path, &["update-index", flag, "tracked"]).await;
        std::fs::write(path.join("tracked"), "changed behind the index flag\n").unwrap();
        let report = service.run_now().await.unwrap();
        assert!(path.exists());
        assert!(
            report
                .entries
                .iter()
                .any(|entry| entry.reason.contains("hidden from git status")),
            "{flag}"
        );
        std::fs::write(path.join("tracked"), "original\n").unwrap();
        let clear = if flag == "--assume-unchanged" {
            "--no-assume-unchanged"
        } else {
            "--no-skip-worktree"
        };
        git(&path, &["update-index", clear, "tracked"]).await;
    }
    h.state.lock().await.managed_roots = vec![temp.path().join("elsewhere")];
    assert!(
        service
            .run_now()
            .await
            .unwrap()
            .entries
            .iter()
            .any(|e| e.reason.contains("Outside"))
    );
    assert!(path.exists());
    h.state.lock().await.managed_roots = vec![temp.path().join("worktrees")];
    h.open_terminal_on_lease.store(true, Ordering::SeqCst);
    let blocked = service.run_now().await.unwrap();
    assert!(path.exists());
    assert!(
        blocked
            .entries
            .iter()
            .any(|e| e.reason.contains("Open terminal"))
    );
    h.state.lock().await.terminal_paths.clear();
    let report = service.run_now().await.unwrap();
    assert_eq!(report.counts.removed, WireNumber::Finite(1.into()));
    assert!(!path.exists());
    assert_eq!(
        h.state.lock().await.threads[0]
            .shell
            .worktree_path
            .as_ref()
            .unwrap()
            .0,
        path.to_string_lossy()
    );
}

async fn linked_repository(
    temp: &Path,
) -> (
    std::path::PathBuf,
    std::path::PathBuf,
    Arc<Host>,
    StorageCleanup,
) {
    let repo = temp.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]).await;
    std::fs::write(repo.join("tracked"), "base\n").unwrap();
    git(&repo, &["add", "tracked"]).await;
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "root",
        ],
    )
    .await;
    let path = temp.join("worktrees/repo/feature");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    git(
        &repo,
        &["worktree", "add", "-b", "feature", path.to_str().unwrap()],
    )
    .await;
    let h = host();
    {
        let mut state = h.state.lock().await;
        state.managed_roots = vec![temp.join("worktrees")];
        state.project_roots = vec![repo.clone()];
        state.threads = vec![CleanupThread {
            shell: shell(&path),
            deleted: false,
            project_path: Some(repo.clone()),
        }];
        state.settings.storage_cleanup.worktree_after_days = Some(1);
    }
    let service = StorageCleanup::new(
        temp.join("state"),
        h.clone(),
        Arc::new(Time(AtomicI64::new(NOW))),
    );
    (repo, path, h, service)
}

#[tokio::test]
async fn preview_never_fetches_resolves_remote_eligibility_or_takes_a_workspace_lease() {
    let temp = tempfile::tempdir().unwrap();
    let (repo, path, h, service) = linked_repository(temp.path()).await;
    let remote = temp.path().join("origin.git");
    std::fs::create_dir(&remote).unwrap();
    git(&remote, &["init", "--bare", "-b", "main"]).await;
    git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    )
    .await;
    git(&repo, &["push", "origin", "main"]).await;
    git(&repo, &["update-ref", "-d", "refs/remotes/origin/main"]).await;
    {
        let mut state = h.state.lock().await;
        state.settings.storage_cleanup.worktree_after_days = None;
        state.settings.storage_cleanup.worktree_on_merge = true;
        state.settings.storage_cleanup.worktree_unchanged = false;
    }
    service.preview().await.unwrap();
    let refs = t3_git::Git::new(&repo)
        .execute(
            &[
                "show-ref",
                "--verify",
                "--quiet",
                "refs/remotes/origin/main",
            ],
            t3_git::ExecuteOptions {
                allow_nonzero: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_ne!(refs.code, Some(0), "preview must not fetch remote refs");
    assert_eq!(h.pr_lookups.load(Ordering::SeqCst), 0);
    h.state
        .lock()
        .await
        .settings
        .storage_cleanup
        .worktree_after_days = Some(1);
    let report = service.preview().await.unwrap();
    assert!(
        report
            .entries
            .iter()
            .any(|e| e.reason.starts_with("Would remove:"))
    );
    assert_eq!(h.lease_acquisitions.load(Ordering::SeqCst), 0);
    assert!(path.exists());
}

#[cfg(windows)]
#[tokio::test]
async fn missing_case_variant_terminal_provider_and_project_paths_prevent_removal() {
    let temp = tempfile::tempdir().unwrap();
    let (_, path, h, service) = linked_repository(temp.path()).await;
    let spelling =
        std::path::PathBuf::from(path.to_string_lossy().to_uppercase()).join("missing-child");
    for kind in ["terminal", "provider", "project"] {
        {
            let mut state = h.state.lock().await;
            match kind {
                "terminal" => state.terminal_paths.push(spelling.clone()),
                "provider" => {
                    state.terminal_paths.clear();
                    state.live_provider_paths.push(spelling.clone());
                }
                _ => {
                    state.live_provider_paths.clear();
                    state.project_roots.push(spelling.clone());
                }
            }
        }
        let report = service.run_now().await.unwrap();
        assert!(
            path.exists(),
            "{kind} cwd must protect missing paths: raw={}, canonical root={}, cwd={}",
            path.display(),
            dunce::canonicalize(path.parent().unwrap())
                .unwrap()
                .display(),
            spelling.display()
        );
        assert_eq!(report.counts.removed, WireNumber::Finite(0.into()));
    }
}

#[tokio::test]
async fn orphan_report_includes_second_level_full_clones_but_skips_first_level_checkouts() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("worktrees/first");
    let second = temp.path().join("worktrees/repo/branch");
    for path in [&first, &second] {
        std::fs::create_dir_all(path).unwrap();
        git(path, &["init", "-b", "main"]).await;
        filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(0, 0)).unwrap();
    }
    let h = host();
    {
        let mut state = h.state.lock().await;
        state.managed_roots = vec![temp.path().join("worktrees")];
        state.settings.storage_cleanup.worktree_after_days = Some(1);
    }
    let service = StorageCleanup::new(
        temp.path().join("state"),
        h,
        Arc::new(Time(AtomicI64::new(NOW))),
    );
    let report = service.run_now().await.unwrap();
    assert!(report.entries.iter().any(
        |e| e.path.as_deref() == Some(dunce::canonicalize(&second).unwrap().to_str().unwrap())
    ));
    assert!(
        !report
            .entries
            .iter()
            .any(|e| e.path.as_deref()
                == Some(dunce::canonicalize(&first).unwrap().to_str().unwrap()))
    );
    assert!(first.exists() && second.exists());
}

#[tokio::test]
async fn orphan_report_finds_nested_registered_worktrees_without_deleting_the_container() {
    let temp = tempfile::tempdir().unwrap();
    let (repo, _, h, service) = linked_repository(temp.path()).await;
    let container = temp.path().join("worktrees/other/container");
    let nested = container.join("nested");
    std::fs::create_dir_all(&container).unwrap();
    git(
        &repo,
        &["worktree", "add", "-b", "orphan", nested.to_str().unwrap()],
    )
    .await;
    filetime::set_file_mtime(&nested, filetime::FileTime::from_unix_time(0, 0)).unwrap();
    h.state.lock().await.threads.clear();
    let report = service.run_now().await.unwrap();
    let canonical = dunce::canonicalize(&nested).unwrap();
    let container = dunce::canonicalize(&container).unwrap();
    assert!(report.entries.iter().any(|e| {
        e.path
            .as_deref()
            .is_some_and(|path| Path::new(path) == canonical)
            && e.reason.contains("No thread uses this worktree")
    }));
    assert!(!report.entries.iter().any(|e| {
        e.path
            .as_deref()
            .is_some_and(|path| Path::new(path) == container)
    }));
    assert!(nested.join(".git").is_file());
    assert_eq!(report.counts.removed, WireNumber::Finite(0.into()));
}

#[tokio::test]
async fn worktree_report_entries_preserve_projection_order() {
    let temp = tempfile::tempdir().unwrap();
    let h = host();
    {
        let mut state = h.state.lock().await;
        state.managed_roots = vec![temp.path().join("worktrees")];
        state.settings.storage_cleanup.worktree_after_days = Some(1);
        for id in ["z-first", "a-second"] {
            let path = temp.path().join("worktrees/repo").join(id);
            std::fs::create_dir_all(&path).unwrap();
            let mut row = shell(&path);
            row.id = t3_contracts::base_schemas::ThreadId(id.into());
            row.status = t3_contracts::orchestration_v2::OrchestrationV2ShellThreadStatus::Running;
            state.threads.push(CleanupThread {
                shell: row,
                deleted: false,
                project_path: Some(temp.path().join("project")),
            });
        }
    }
    let service = StorageCleanup::new(
        temp.path().join("state"),
        h,
        Arc::new(Time(AtomicI64::new(NOW))),
    );
    let report = service.run_now().await.unwrap();
    let ids: Vec<_> = report
        .entries
        .iter()
        .filter_map(|e| e.thread_id.as_ref().map(|id| id.0.as_str()))
        .collect();
    assert_eq!(ids, vec!["z-first", "a-second"]);
}

#[tokio::test]
async fn tracked_changes_cleanup_removes_ignored_and_untracked_files_only_after_the_lease_recheck()
{
    for change in [Some("settings"), Some("thread"), Some("deletion"), None] {
        let temp = tempfile::tempdir().unwrap();
        let (_, path, h, service) = linked_repository(temp.path()).await;
        std::fs::write(path.join(".gitignore"), "ignored\n").unwrap();
        std::fs::write(path.join("ignored"), "valuable ignored file").unwrap();
        std::fs::write(path.join("untracked"), "valuable untracked file").unwrap();
        h.state
            .lock()
            .await
            .settings
            .storage_cleanup
            .worktree_keep_when = WorktreeKeepWhen::TrackedChanges;
        *h.lease_change.lock().await = change;
        let report = service.run_now().await.unwrap();
        if change.is_some() {
            assert!(path.exists());
            assert_eq!(
                std::fs::read_to_string(path.join("ignored")).unwrap(),
                "valuable ignored file"
            );
            assert!(path.join("untracked").exists());
            assert_eq!(report.counts.removed, WireNumber::Finite(0.into()));
        } else {
            assert!(!path.exists());
            assert_eq!(report.counts.removed, WireNumber::Finite(1.into()));
        }
    }
}

#[tokio::test]
async fn deleted_owner_keeps_idle_subagents_until_pending_deletion_finishes() {
    let temp = tempfile::tempdir().unwrap();
    let (_, path, h, service) = linked_repository(temp.path()).await;
    {
        let mut state = h.state.lock().await;
        state.settings.storage_cleanup.worktree_after_days = None;
        state.settings.storage_cleanup.worktree_on_delete = true;
        state.settings.storage_cleanup.worktree_on_merge = false;
        state.settings.storage_cleanup.worktree_unchanged = false;
        state.threads[0].deleted = true;
        let mut child = t3_contracts::stored::to_value(&state.threads[0].shell).unwrap();
        child["id"] = serde_json::json!("child");
        child["lineage"] = serde_json::json!({"rootThreadId":"thread","parentThreadId":"thread","relationshipToParent":"subagent"});
        let project_path = state.threads[0].project_path.clone();
        state.threads.push(CleanupThread {
            shell: serde_json::from_value(child).unwrap(),
            deleted: false,
            project_path,
        });
        state
            .pending_deletion_threads
            .insert(t3_contracts::base_schemas::ThreadId("thread".into()));
    }
    let report = service.run_now().await.unwrap();
    assert!(path.exists());
    assert!(
        report
            .entries
            .iter()
            .any(|e| e.reason.contains("deletion is still pending"))
    );
    h.state.lock().await.pending_deletion_threads.clear();
    let report = service.run_now().await.unwrap();
    assert!(!path.exists());
    assert_eq!(report.counts.removed, WireNumber::Finite(1.into()));
    assert_eq!(report.entries[0].thread_id.as_ref().unwrap().0, "thread");
    assert!(
        h.state
            .lock()
            .await
            .threads
            .iter()
            .all(|t| t.shell.worktree_path.is_some())
    );
}

#[tokio::test]
async fn integrated_branch_cleanup_requires_integration_and_archived_or_settled_ownership() {
    let temp = tempfile::tempdir().unwrap();
    let (repo, path, h, service) = linked_repository(temp.path()).await;
    std::fs::write(path.join("feature"), "feature").unwrap();
    git(&path, &["add", "feature"]).await;
    git(
        &path,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "feature",
        ],
    )
    .await;
    {
        let mut state = h.state.lock().await;
        state.settings.storage_cleanup.worktree_after_days = None;
        state.settings.storage_cleanup.worktree_on_merge = false;
        state.settings.storage_cleanup.worktree_unchanged = true;
        state.threads[0].shell.archived_at = Some(t3_contracts::wire::WireDateTime(
            "2020-01-01T00:00:00.000Z".into(),
        ));
    }
    service.run_now().await.unwrap();
    assert!(path.exists(), "unintegrated feature commits must survive");
    git(&repo, &["merge", "--ff-only", "feature"]).await;
    let report = service.run_now().await.unwrap();
    assert!(!path.exists());
    assert_eq!(report.counts.removed, WireNumber::Finite(1.into()));
}

#[tokio::test]
async fn settled_subagent_only_inherits_an_idle_ancestor_after_the_grace_period() {
    let temp = tempfile::tempdir().unwrap();
    let (_, path, h, service) = linked_repository(temp.path()).await;
    {
        let mut state = h.state.lock().await;
        state.settings.storage_cleanup.worktree_after_days = None;
        state.settings.storage_cleanup.worktree_on_merge = false;
        state.settings.storage_cleanup.worktree_unchanged = false;
        state.settings.storage_cleanup.worktree_settled_after_days = Some(1);
        let mut parent = state.threads[0].clone();
        parent.shell.id = t3_contracts::base_schemas::ThreadId("parent".into());
        parent.shell.worktree_path = None;
        parent.shell.lineage.root_thread_id = t3_contracts::base_schemas::ThreadId("parent".into());
        parent.shell.settled_at = Some(t3_contracts::wire::WireDateTime(
            "2020-01-01T00:00:00.000Z".into(),
        ));
        parent.shell.status =
            t3_contracts::orchestration_v2::OrchestrationV2ShellThreadStatus::Running;
        let mut child = t3_contracts::stored::to_value(&state.threads[0].shell).unwrap();
        child["lineage"] = serde_json::json!({"rootThreadId":"parent","parentThreadId":"parent","relationshipToParent":"subagent"});
        state.threads[0].shell = serde_json::from_value(child).unwrap();
        state.threads.push(parent);
    }
    service.run_now().await.unwrap();
    assert!(path.exists());
    {
        let mut state = h.state.lock().await;
        state.threads[1].shell.status =
            t3_contracts::orchestration_v2::OrchestrationV2ShellThreadStatus::Idle;
        state.threads[1].shell.updated_at = t3_contracts::wire::WireDateTime(
            chrono::DateTime::from_timestamp_millis(NOW)
                .unwrap()
                .to_rfc3339(),
        );
    }
    service.run_now().await.unwrap();
    assert!(
        path.exists(),
        "recent ancestor settlement must keep the worktree"
    );
    h.state.lock().await.threads[1].shell.updated_at =
        t3_contracts::wire::WireDateTime("2020-01-01T00:00:00.000Z".into());
    let report = service.run_now().await.unwrap();
    assert!(!path.exists());
    assert_eq!(report.counts.removed, WireNumber::Finite(1.into()));
}

#[tokio::test]
async fn pull_request_cleanup_requires_the_merged_record_for_the_exact_unintegrated_head() {
    let temp = tempfile::tempdir().unwrap();
    let (repo, path, h, service) = linked_repository(temp.path()).await;
    let remote = temp.path().join("origin.git");
    std::fs::create_dir(&remote).unwrap();
    git(&remote, &["init", "--bare", "-b", "main"]).await;
    git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    )
    .await;
    git(&repo, &["push", "origin", "main"]).await;
    git(
        &repo,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    )
    .await;
    std::fs::write(path.join("feature"), "new commit").unwrap();
    git(&path, &["add", "feature"]).await;
    git(
        &path,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "feature",
        ],
    )
    .await;
    let head = git_text(&path, &["rev-parse", "HEAD"])
        .await
        .trim()
        .to_owned();
    {
        let mut state = h.state.lock().await;
        state.settings.storage_cleanup.worktree_after_days = None;
        state.settings.storage_cleanup.worktree_on_merge = true;
        state.settings.storage_cleanup.worktree_unchanged = false;
    }
    for (state, sha) in [("open", head.clone()), ("merged", "0".repeat(40))] {
        *h.pull_request.lock().await = Some(MergedPullRequest {
            state: state.into(),
            head_ref: "feature".into(),
            base_ref: "main".into(),
            head_sha: sha,
        });
        service.run_now().await.unwrap();
        assert!(path.exists());
    }
    *h.pull_request.lock().await = Some(MergedPullRequest {
        state: "merged".into(),
        head_ref: "feature".into(),
        base_ref: "main".into(),
        head_sha: head,
    });
    let report = service.run_now().await.unwrap();
    assert!(!path.exists());
    assert_eq!(report.counts.removed, WireNumber::Finite(1.into()));
    assert!(h.pr_lookups.load(Ordering::SeqCst) >= 3);
    assert_eq!(h.pr_path.lock().await.as_deref(), Some(path.as_path()));
}

#[tokio::test]
async fn forced_submodule_removal_keeps_dirty_files_and_unpushed_stored_history() {
    let temp = tempfile::tempdir().unwrap();
    let (repo, path, _, service) = linked_repository(temp.path()).await;
    let source = temp.path().join("submodule-origin");
    std::fs::create_dir(&source).unwrap();
    git(&source, &["init", "-b", "main"]).await;
    std::fs::write(source.join("subfile"), "original\n").unwrap();
    git(&source, &["add", "subfile"]).await;
    git(
        &source,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "submodule",
        ],
    )
    .await;
    git(
        &repo,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            source.to_str().unwrap(),
            "nested",
        ],
    )
    .await;
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-am",
            "add submodule",
        ],
    )
    .await;
    git(&path, &["reset", "--hard", "main"]).await;
    git(
        &path,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "update",
            "--init",
        ],
    )
    .await;
    let nested = path.join("nested");
    std::fs::write(nested.join("subfile"), "uncommitted value").unwrap();
    service.run_now().await.unwrap();
    assert_eq!(
        std::fs::read_to_string(nested.join("subfile")).unwrap(),
        "uncommitted value"
    );
    git(&nested, &["add", "subfile"]).await;
    git(
        &nested,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "unpushed",
        ],
    )
    .await;
    let admin = git_text(&nested, &["rev-parse", "--absolute-git-dir"])
        .await
        .trim()
        .to_owned();
    git(&path, &["submodule", "deinit", "--force", "nested"]).await;
    let report = service.run_now().await.unwrap();
    assert!(path.exists());
    assert!(
        report
            .entries
            .iter()
            .any(|e| e.reason.contains("commits not on a remote-tracking ref"))
    );
    git(
        &repo,
        &[
            "--git-dir",
            &admin,
            "reset",
            "--soft",
            "refs/remotes/origin/main",
        ],
    )
    .await;
    git(
        &repo,
        &[
            "--git-dir",
            &admin,
            "reflog",
            "expire",
            "--expire=all",
            "--all",
        ],
    )
    .await;
    let report = service.run_now().await.unwrap();
    assert!(
        !path.exists(),
        "a safe deinitialized submodule requires forced worktree removal"
    );
    assert_eq!(report.counts.removed, WireNumber::Finite(1.into()));
    assert_eq!(
        std::fs::read_to_string(source.join("subfile")).unwrap(),
        "original\n"
    );
}

#[test]
fn worktree_measurement_excludes_shared_hardlinks_and_pnpm_store_blocks() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("worktree");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("one"), vec![0_u8; 4096]).unwrap();
    std::fs::hard_link(root.join("one"), root.join("two")).unwrap();
    assert_eq!(measure_worktree(&root, false).unwrap(), 0);
    std::fs::remove_file(root.join("two")).unwrap();
    assert!(measure_worktree(&root, false).unwrap() >= 4096);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let _locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(root.join("one"))
            .unwrap();
        assert!(
            measure_worktree(&root, false)
                .expect("metadata inspection must not request file read access")
                >= 4096
        );
    }
    let store = temp.path().join("store");
    std::fs::create_dir(&store).unwrap();
    let virtual_store = root.join("node_modules/.pnpm");
    std::fs::create_dir_all(&virtual_store).unwrap();
    std::fs::write(virtual_store.join("package"), vec![0_u8; 8192]).unwrap();
    let manifest = root.join("node_modules/.modules.yaml");
    std::fs::write(
        &manifest,
        serde_json::json!({"storeDir":store.to_string_lossy()}).to_string(),
    )
    .unwrap();
    let shared = measure_worktree(&root, false).unwrap();
    std::fs::remove_dir(&store).unwrap();
    let copied = measure_worktree(&root, false).unwrap();
    assert!(copied >= shared + 8192);
}
#[test]
fn ignored_nested_repositories_are_kept_even_inside_shared_pnpm_store() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("worktree");
    let nested = root.join("node_modules/.pnpm/nested");
    std::fs::create_dir_all(nested.join(".git")).unwrap();
    let store = temp.path().join("store");
    std::fs::create_dir(&store).unwrap();
    std::fs::write(
        root.join("node_modules/.modules.yaml"),
        format!("storeDir: {}\n", store.to_string_lossy()),
    )
    .unwrap();
    assert!(measure_worktree(&root, true).is_err());
    assert!(nested.exists());
}
#[tokio::test]
async fn disabled_worktree_policy_does_not_report_orphans() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("worktrees");
    let orphan = root.join("repo/orphan");
    std::fs::create_dir_all(&orphan).unwrap();
    filetime::set_file_mtime(&orphan, filetime::FileTime::from_unix_time(0, 0)).unwrap();
    let h = host();
    h.state.lock().await.managed_roots = vec![root];
    let service = StorageCleanup::new(
        temp.path().join("state"),
        h.clone(),
        Arc::new(Time(AtomicI64::new(NOW))),
    );
    assert!(service.run_now().await.unwrap().entries.is_empty());
    h.state
        .lock()
        .await
        .settings
        .storage_cleanup
        .worktree_after_days = Some(1);
    let report = service.run_now().await.unwrap();
    assert_eq!(report.entries.len(), 1);
    assert!(report.entries[0].reason.contains("No thread"));
    assert!(orphan.exists());
}

#[tokio::test]
async fn scratch_retention_preserves_files_repositories_and_scheduled_work() {
    for case in [
        "remove",
        "local",
        "repository",
        "bare-root",
        "mixed-case",
        "nested",
        "scheduled",
        "not-due",
        "terminal",
        "late-terminal",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let scratch = temp.path().join("scratch");
        let path = scratch.join("quiet-otter");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("notes.md"), "scratch\n").unwrap();
        if case == "bare-root" {
            git(&path, &["init", "--bare"]).await;
        }
        if case == "mixed-case" {
            std::fs::create_dir_all(path.join("nested/.GIT")).unwrap();
        }
        if case == "repository" {
            std::fs::create_dir(path.join(".git")).unwrap();
        }
        if case == "nested" {
            std::fs::create_dir_all(path.join("clone/.git")).unwrap();
        }
        let leftover = scratch.join("leftover");
        std::fs::create_dir(&leftover).unwrap();
        filetime::set_file_mtime(&leftover, filetime::FileTime::from_unix_time(1, 0)).unwrap();
        let h = host();
        {
            let mut state = h.state.lock().await;
            let mut thread = shell(&path);
            thread.branch = None;
            state.project_roots.push(scratch.clone());
            state.threads.push(CleanupThread {
                shell: thread,
                deleted: false,
                project_path: Some(scratch.clone()),
            });
            state.settings.storage_cleanup.worktree_after_days =
                Some(if case == "not-due" { 99999 } else { 1 });
            if case != "local" {
                state.settings.storage_cleanup.worktree_keep_when =
                    WorktreeKeepWhen::TrackedChanges;
            }
            if case == "terminal" {
                state.terminal_paths.push(path.join("missing"));
            }
        }
        if case == "scheduled" {
            h.scheduled.lock().await.insert("thread".into());
        }
        if case == "late-terminal" {
            h.open_terminal_on_lease.store(true, Ordering::SeqCst);
        }
        let service = StorageCleanup::new(
            temp.path().join("userdata"),
            h.clone(),
            Arc::new(Time(AtomicI64::new(NOW))),
        );
        let report = service.run_now().await.unwrap();
        let row = report
            .entries
            .iter()
            .find(|e| e.thread_id.is_some())
            .unwrap();
        assert_eq!(path.exists(), case != "remove", "{case}: {}", row.reason);
        assert!(leftover.exists());
        assert!(
            report.entries.iter().any(|e| e
                .path
                .as_deref()
                .and_then(|p| dunce::canonicalize(p).ok())
                == dunce::canonicalize(&leftover).ok()
                && e.reason == "No thread uses this folder"),
            "{case}"
        );
        assert_eq!(
            h.state.lock().await.threads[0]
                .shell
                .worktree_path
                .as_ref()
                .unwrap()
                .0,
            path.to_string_lossy()
        );
    }
}

#[tokio::test]
async fn cleanup_never_deletes_when_an_authoritative_source_is_unavailable() {
    let temp = tempfile::tempdir().unwrap();
    let (_, path, h, service) = linked_repository(temp.path()).await;
    for source in [
        "provider sessions unavailable",
        "terminals unavailable",
        "pending deletions unavailable",
    ] {
        *h.unavailable.lock().await = Some(source);
        let log = temp.path().join("state/logs/worker.log.1");
        write_old(&log, b"expired", 0);
        h.state
            .lock()
            .await
            .settings
            .storage_cleanup
            .logs_after_days = Some(1);
        let report = service.run_now().await.unwrap();
        assert!(path.join("tracked").exists());
        assert!(!log.exists());
        assert_eq!(report.counts.failed, WireNumber::Finite(1.into()));
        assert_eq!(
            report.entries[0].kind,
            StorageCleanupReportEntryKind::Worktree
        );
        assert_eq!(service.latest_report().await, Some(report));
    }
    *h.unavailable.lock().await = None;
    *h.lease_change.lock().await = Some("unavailable");
    let report = service.run_now().await.unwrap();
    assert!(path.join("tracked").exists());
    assert_eq!(
        report.entries[0].outcome,
        StorageCleanupReportEntryOutcome::Kept
    );
}

#[tokio::test]
async fn lost_git_link_is_kept_for_turn_startup_restoration() {
    let temp = tempfile::tempdir().unwrap();
    let (_, path, _, service) = linked_repository(temp.path()).await;
    std::fs::remove_file(path.join(".git")).unwrap();
    std::fs::create_dir(path.join("node_modules")).unwrap();
    let report = service.run_now().await.unwrap();
    assert!(path.join("node_modules").exists());
    assert_eq!(
        report.entries[0].outcome,
        StorageCleanupReportEntryOutcome::Kept
    );
}

#[tokio::test(start_paused = true)]
async fn an_unresponsive_safety_port_refuses_instead_of_hanging_the_sweep() {
    struct Never;
    #[async_trait]
    impl CleanupHost for Never {
        async fn snapshot(&self) -> Result<CleanupSnapshot> {
            std::future::pending().await
        }
        async fn enabled_scheduled_threads(
            &self,
        ) -> Result<BTreeSet<t3_contracts::base_schemas::ThreadId>> {
            unreachable!()
        }
        async fn storage_cleanup_settings(&self) -> Result<StorageCleanupSettings> {
            std::future::pending().await
        }
        async fn acquire_workspace_lease(&self, _: &Path) -> Result<OwnedMutexGuard<()>> {
            unreachable!()
        }
        async fn branch_pull_request(
            &self,
            _: &Path,
            _: &str,
        ) -> Result<Option<MergedPullRequest>> {
            unreachable!()
        }
        async fn invalidate_git_status(&self, _: &Path) {
            unreachable!()
        }
    }
    let temp = tempfile::tempdir().unwrap();
    write_old(&temp.path().join("logs/keep.log.1"), b"keep", 0);
    let service = StorageCleanup::new(
        temp.path().into(),
        Arc::new(Never),
        Arc::new(Time(AtomicI64::new(NOW))),
    );
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(31), service.run_now()).await;
    assert!(outcome.is_ok(), "cleanup must bound its own safety reads");
    assert!(outcome.unwrap().is_err());
    assert!(temp.path().join("logs/keep.log.1").exists());
}

#[tokio::test]
async fn policy_change_during_final_git_checks_keeps_the_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let (_, path, host, cleanup) = linked_repository(temp.path()).await;
    host.disable_after_snapshot_under_lease
        .store(true, Ordering::SeqCst);
    cleanup.run_now().await.unwrap();
    assert!(path.exists());
}

#[tokio::test]
async fn intermediate_settlement_ancestor_is_rechecked_for_scratch_and_git() {
    for scratch in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let (_, checkout, host, cleanup) = linked_repository(temp.path()).await;
        let path = if scratch {
            let path = temp.path().join("scratch/child");
            std::fs::create_dir_all(&path).unwrap();
            path
        } else {
            checkout
        };
        {
            let mut state = host.state.lock().await;
            state.settings.storage_cleanup.worktree_after_days = None;
            state.settings.storage_cleanup.worktree_on_merge = false;
            state.settings.storage_cleanup.worktree_unchanged = false;
            state.settings.storage_cleanup.worktree_settled_after_days = Some(1);
            let original = t3_contracts::stored::to_value(&state.threads[0].shell).unwrap();
            let mut outside = state.threads[0].clone();
            let mut child = original.clone();
            child["lineage"] = serde_json::json!({"rootThreadId":"root","parentThreadId":"parent","relationshipToParent":"subagent"});
            child["worktreePath"] = serde_json::json!(path);
            if scratch {
                child["branch"] = serde_json::Value::Null;
            }
            state.threads[0].shell = serde_json::from_value(child).unwrap();
            let mut parent = original.clone();
            parent["id"] = "parent".into();
            parent["worktreePath"] = serde_json::Value::Null;
            parent["lineage"] = serde_json::json!({"rootThreadId":"root","parentThreadId":"root","relationshipToParent":"subagent"});
            outside.shell = serde_json::from_value(parent).unwrap();
            state.threads.push(outside.clone());
            let mut root = original;
            root["id"] = "root".into();
            root["lineage"]["rootThreadId"] = "root".into();
            root["worktreePath"] = serde_json::Value::Null;
            root["settledAt"] = "2020-01-01T00:00:00.000Z".into();
            outside.shell = serde_json::from_value(root).unwrap();
            state.threads.push(outside);
        }
        *host.lease_change.lock().await = Some("ancestor");
        cleanup.run_now().await.unwrap();
        assert!(
            host.lease_acquisitions.load(Ordering::SeqCst) > 0,
            "scratch={scratch}"
        );
        assert!(path.exists(), "scratch={scratch}");
    }
}

#[tokio::test]
async fn automatic_requests_during_removal_publish_one_followup() {
    let temp = tempfile::tempdir().unwrap();
    let (_, path, host, service) = linked_repository(temp.path()).await;
    let service = Arc::new(service);
    let held = host.lease.clone().lock_owned().await;
    let mut reports = service.reports().await;
    assert!(reports.recv().await.unwrap().is_none());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let worker = {
        let service = service.clone();
        tokio::spawn(async move {
            service
                .run_automatic_until(async {
                    let _ = stopped.await;
                })
                .await;
        })
    };
    host.lease_requested.notified().await;
    for _ in 0..32 {
        service.request_automatic_sweep();
    }
    drop(held);
    let first = reports.recv().await.unwrap().unwrap();
    assert_eq!(first.counts.removed, WireNumber::Finite(1.into()));
    assert!(!path.exists());
    assert_eq!(
        reports.recv().await.unwrap().unwrap().counts.removed,
        WireNumber::Finite(0.into())
    );
    stop.send(()).unwrap();
    worker.await.unwrap();
    assert!(reports.try_recv().is_err());
}
