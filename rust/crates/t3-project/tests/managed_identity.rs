#![allow(clippy::unwrap_used)]
mod support;
use std::sync::Arc;
use support::*;
use t3_contracts::base_schemas::{ProjectId, ThreadId};
use t3_git::{ExecuteOptions, Git};
use t3_project::{identity::*, managed::*};

#[test]
fn folder_names_match_unicode_reserved_names_and_limits() {
    for (input, expected) in [
        ("Crème Brûlée", "creme-brulee"),
        ("CON", "con-project"),
        ("COM1", "com1-project"),
        (".. / 🙂", "project"),
        ("one two three four five six", "one-two-three-four-five-six"),
    ] {
        assert_eq!(folder_name(input), expected);
    }
    assert_eq!(folder_name(&"x".repeat(100)).len(), 64);
    assert_eq!(
        folder_words("Hello, WORLD! use the local files"),
        "hello-world-use-the-local"
    );
}
#[tokio::test]
async fn named_projects_are_real_committed_repositories_with_unique_folders() {
    let f = Fixture::new();
    let base = f.root("base");
    let folders = ManagedProjectFolders::new(
        base,
        f.service.clone(),
        Arc::new(FixtureIo),
        git_environment(f.temp.path()),
    );
    let input = || serde_json::from_value(serde_json::json!({"name":"Crème App"})).unwrap();
    let (a, b) = tokio::join!(folders.create_new(input()), folders.create_new(input()));
    let a = a.unwrap();
    let b = b.unwrap();
    assert_ne!(a.workspace_root, b.workspace_root);
    assert!(a.commit_error.is_none());
    assert!(b.commit_error.is_none());
    for p in [&a, &b] {
        let git = Git::new(&p.workspace_root.0).with_environment(git_environment(f.temp.path()));
        assert_eq!(
            git.execute(&["log", "-1", "--format=%s"], ExecuteOptions::default())
                .await
                .unwrap()
                .stdout_text()
                .trim(),
            "Initial commit"
        );
        assert_eq!(
            git.execute(&["ls-files"], ExecuteOptions::default())
                .await
                .unwrap()
                .stdout_text(),
            "README.md\nassets/icon.svg\n"
        );
        assert!(
            std::fs::read_to_string(std::path::Path::new(&p.workspace_root.0).join("README.md"))
                .unwrap()
                .contains("# Crème App")
        );
    }
    f.shutdown().await;
}
#[tokio::test]
async fn scratch_is_unique_recreated_and_never_inherits_a_checkout() {
    let f = Fixture::new();
    let base = f.root("base");
    let folders = ManagedProjectFolders::new(
        base.clone(),
        f.service.clone(),
        Arc::new(FixtureIo),
        git_environment(f.temp.path()),
    );
    let (a, b) = tokio::join!(folders.ensure_scratch(), folders.ensure_scratch());
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.project_id, b.project_id);
    let p = folders
        .folder_for_thread(
            &a.project_id,
            &ThreadId("ABC-123456789".into()),
            "Build a tiny tool",
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        p.file_name().unwrap(),
        "2026-09-04-build-a-tiny-tool-23456789"
    );
    let p2 = folders
        .folder_for_thread(
            &a.project_id,
            &ThreadId("ABC-123456789".into()),
            "Build a tiny tool",
        )
        .await
        .unwrap()
        .unwrap();
    assert_ne!(p, p2);
    let scratch = folders.scratch_root().await.unwrap();
    assert!(scratch.starts_with(f.temp.path()));
    std::fs::remove_dir_all(&scratch).unwrap();
    assert_eq!(
        folders.ensure_scratch().await.unwrap().project_id,
        a.project_id
    );
    assert!(scratch.is_dir());
    assert!(
        folders
            .folder_for_thread(&ProjectId("unknown".into()), &ThreadId("x".into()), "x",)
            .await
            .unwrap()
            .is_none()
    );
    let checkout = f.root("checkout");
    Git::new(&checkout)
        .with_environment(git_environment(f.temp.path()))
        .execute(&["init"], ExecuteOptions::default())
        .await
        .unwrap();
    let unavailable = ManagedProjectFolders::new(
        checkout,
        f.service.clone(),
        Arc::new(FixtureIo),
        git_environment(f.temp.path()),
    );
    assert!(unavailable.scratch_root().await.is_none());
    assert!(unavailable.ensure_scratch().await.is_err());
    f.shutdown().await;
}
#[tokio::test]
async fn cleanup_preserves_owned_directories_and_removes_only_unowned_claims() {
    let f = Fixture::new();
    let base = f.root("base");
    let folders = ManagedProjectFolders::new(
        base,
        f.service.clone(),
        Arc::new(FixtureIo),
        git_environment(f.temp.path()),
    );
    std::fs::create_dir_all(&folders.named_projects_root).unwrap();
    let owned = folders.named_projects_root.join("owned");
    std::fs::create_dir(&owned).unwrap();
    std::fs::write(owned.join("keep"), "owned").unwrap();
    f.service
        .create(f.create("owner", "owner", &owned))
        .await
        .unwrap();
    folders.cleanup_unowned(&owned).await;
    assert_eq!(
        std::fs::read_to_string(owned.join("keep")).unwrap(),
        "owned"
    );
    let unowned = folders.named_projects_root.join("unowned");
    std::fs::create_dir(&unowned).unwrap();
    std::fs::write(unowned.join("remove"), "temporary").unwrap();
    folders.cleanup_unowned(&unowned).await;
    assert!(!unowned.exists());
    folders.cleanup_unowned(&folders.named_projects_root).await;
    assert!(folders.named_projects_root.exists());
    let uncertain = folders.named_projects_root.join("ownership-unavailable");
    std::fs::create_dir(&uncertain).unwrap();
    std::fs::write(uncertain.join("keep"), "cannot establish ownership").unwrap();
    f.database
        .transaction(|tx| {
            tx.execute_batch("ALTER TABLE projection_projects RENAME TO unavailable_projects")?;
            Ok(())
        })
        .await
        .unwrap();
    folders.cleanup_unowned(&uncertain).await;
    assert_eq!(
        std::fs::read_to_string(uncertain.join("keep")).unwrap(),
        "cannot establish ownership"
    );
    f.database
        .transaction(|tx| {
            tx.execute_batch("ALTER TABLE unavailable_projects RENAME TO projection_projects")?;
            Ok(())
        })
        .await
        .unwrap();
    f.shutdown().await;
}
#[cfg(windows)]
#[tokio::test]
async fn cleanup_refuses_a_real_junction_pointing_outside() {
    let f = Fixture::new();
    let base = f.root("base");
    let outside = f.root("outside");
    std::fs::write(outside.join("keep"), "outside data").unwrap();
    let folders = ManagedProjectFolders::new(
        base,
        f.service.clone(),
        Arc::new(FixtureIo),
        git_environment(f.temp.path()),
    );
    std::fs::create_dir_all(&folders.named_projects_root).unwrap();
    let link = folders.named_projects_root.join("claimed");
    let options = t3_process::ProcessOptions::new("cmd.exe").args(vec![
        std::ffi::OsString::from("/d"),
        "/c".into(),
        "mklink".into(),
        "/J".into(),
        link.clone().into_os_string(),
        outside.clone().into_os_string(),
    ]);
    t3_process::run(options).await.unwrap();
    folders.cleanup_unowned(&link).await;
    assert!(std::fs::symlink_metadata(&link).is_ok());
    assert_eq!(
        std::fs::read_to_string(outside.join("keep")).unwrap(),
        "outside data"
    );
    std::fs::remove_dir(link).unwrap();
    f.shutdown().await;
}

#[tokio::test]
async fn cancelled_scaffolding_removes_the_unowned_claim() {
    use async_trait::async_trait;
    struct BlockWrite {
        ready: tokio::sync::watch::Sender<bool>,
    }
    #[async_trait]
    impl ManagedFolderIo for BlockWrite {
        async fn create_directory(
            &self,
            path: &std::path::Path,
            recursive: bool,
        ) -> t3_project::Result<bool> {
            FixtureIo.create_directory(path, recursive).await
        }
        async fn write_file(&self, _: &std::path::Path, _: &str) -> t3_project::Result<()> {
            self.ready.send_replace(true);
            std::future::pending().await
        }
        async fn remove_directory(&self, path: &std::path::Path) -> t3_project::Result<()> {
            FixtureIo.remove_directory(path).await
        }
    }
    let f = Fixture::new();
    let (ready, mut entered) = tokio::sync::watch::channel(false);
    let folders = ManagedProjectFolders::new(
        f.root("base"),
        f.service.clone(),
        Arc::new(BlockWrite { ready }),
        git_environment(f.temp.path()),
    );
    let worker = folders.clone();
    let task = tokio::spawn(async move {
        worker
            .create_new(serde_json::from_value(serde_json::json!({"name":"Cancelled"})).unwrap())
            .await
    });
    entered.changed().await.unwrap();
    assert!(folders.named_projects_root.join("cancelled").exists());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    folders.shutdown().await;
    assert!(!folders.named_projects_root.join("cancelled").exists());
    assert!(f.service.snapshot().await.unwrap().projects.is_empty());
    f.shutdown().await;
}
#[tokio::test]
async fn identity_handles_upstream_marks_local_remotes_and_bare_repositories() {
    let f = Fixture::new();
    let root = f.root("repo");
    let git = Git::new(&root).with_environment(git_environment(f.temp.path()));
    for args in [
        vec!["init"],
        vec!["remote", "add", "origin", "git@github.com:Fork/Repo.git"],
        vec![
            "remote",
            "add",
            "upstream",
            "https://github.com/Team/Repo.git",
        ],
        vec!["remote", "add", "gb-local", "."],
    ] {
        git.execute(&args, ExecuteOptions::default()).await.unwrap();
    }
    let resolver =
        RepositoryIdentityResolver::default().with_environment(git_environment(f.temp.path()));
    let id = resolver
        .resolve(&root.to_string_lossy(), false)
        .await
        .unwrap();
    assert_eq!(id.canonical_key.0, "github.com/team/repo");
    assert_eq!(id.origin.unwrap().canonical_key.0, "github.com/fork/repo");
    git.execute(
        &["config", "remote.origin.gh-resolved", "base"],
        ExecuteOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        resolver
            .resolve(&root.to_string_lossy(), false)
            .await
            .unwrap()
            .canonical_key
            .0,
        "github.com/team/repo"
    );
    assert_eq!(
        resolver
            .resolve(&root.to_string_lossy(), true)
            .await
            .unwrap()
            .canonical_key
            .0,
        "github.com/fork/repo"
    );
    let bare = f.root("bare");
    Git::new(&bare)
        .with_environment(git_environment(f.temp.path()))
        .execute(&["init", "--bare"], ExecuteOptions::default())
        .await
        .unwrap();
    Git::new(&bare)
        .with_environment(git_environment(f.temp.path()))
        .execute(
            &[
                "--git-dir=.",
                "remote",
                "add",
                "origin",
                "https://example.test/group/repo.git",
            ],
            ExecuteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        resolver
            .resolve(&bare.to_string_lossy(), false)
            .await
            .unwrap()
            .canonical_key
            .0,
        "example.test/group/repo"
    );
    f.shutdown().await;
}
#[test]
fn remote_parsing_preserves_spaces_and_ignores_push_local_and_malformed_urls() {
    let remotes = parse_fetch_urls(
        "origin\tssh://git@example.test/team repos/app.git (fetch) [blob:none]\norigin\thttps://push.invalid/a/b (push)\nlocal\tC:\\repo (fetch)\nfile\tfile:///tmp/a (fetch)\n",
    );
    assert_eq!(remotes.len(), 1);
    assert_eq!(
        remotes["origin"],
        "ssh://git@example.test/team repos/app.git"
    );
    for local in [
        ".",
        "../repo",
        "C:\\repo",
        "file:///tmp/repo",
        "/tmp/foo:bar",
    ] {
        assert!(is_local_remote(local));
    }
    for remote in [
        "host:team/repo",
        "git@host:team/repo",
        "https://host/team/repo",
    ] {
        assert!(!is_local_remote(remote));
    }
}
