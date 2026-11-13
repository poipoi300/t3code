use async_trait::async_trait;
use std::{
    collections::BTreeSet,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, Ordering},
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
}
#[async_trait]
impl CleanupHost for Host {
    async fn snapshot(&self) -> Result<CleanupSnapshot> {
        Ok(self.state.lock().await.clone())
    }
    async fn acquire_workspace_lease(&self, path: &Path) -> Result<OwnedMutexGuard<()>> {
        if self.open_terminal_on_lease.swap(false, Ordering::SeqCst) {
            self.state.lock().await.terminal_paths.push(path.into());
        }
        Ok(self.lease.clone().lock_owned().await)
    }
    async fn branch_pull_request(&self, _: &Path, _: &str) -> Result<Option<MergedPullRequest>> {
        Ok(None)
    }
    async fn invalidate_git_status(&self, _: &Path) {}
}
fn host() -> Arc<Host> {
    Arc::new(Host {
        state: Mutex::new(CleanupSnapshot {
            settings: ServerSettings::default(),
            threads: vec![],
            project_roots: vec![],
            terminal_paths: vec![],
            live_provider_paths: vec![],
            pending_deletion_threads: BTreeSet::new(),
            managed_roots: vec![],
        }),
        lease: Arc::new(Mutex::new(())),
        open_terminal_on_lease: AtomicBool::new(false),
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
    let service = StorageCleanup::new(temp.path().into(), h, Arc::new(Time(AtomicI64::new(NOW))));
    let mut stream = service.reports().await;
    assert!(stream.recv().await.unwrap().is_none());
    for (name, at) in [
        ("logs/nested/a.log.1", NOW - 86_400_001),
        ("logs/live.log", 0),
        ("logs/a.log.2", NOW - 86_400_000),
        ("logs/x.txt.1", 0),
        ("browser-artifacts/old.png", 0),
        ("browser-artifacts/nested/kept.png", 0),
    ] {
        write_old(&temp.path().join(name), b"abc", at);
    }
    let preview = service.preview().await.unwrap();
    assert_eq!(preview.bytes_freed, WireNumber::Finite(0.into()));
    assert!(temp.path().join("browser-artifacts/old.png").exists());
    assert!(service.latest_report().await.is_none());
    assert!(stream.try_recv().is_err());
    let report = service.run_now().await.unwrap();
    assert_eq!(report.bytes_freed, WireNumber::Finite(6.into()));
    assert!(!temp.path().join("logs/nested/a.log.1").exists());
    assert!(!temp.path().join("browser-artifacts/old.png").exists());
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
    assert_eq!(report.counts.failed, WireNumber::Finite(1.into()));
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
    let mut options = t3_process::ProcessOptions::new("git").args(args.iter().copied());
    options.cwd = Some(cwd.into());
    for (k, v) in [("GIT_CONFIG_GLOBAL", "NUL"), ("GIT_CONFIG_NOSYSTEM", "1")] {
        options.env.insert(k.into(), Some(v.into()));
    }
    t3_process::run(options).await.unwrap();
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
            project_path: repo.clone(),
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
