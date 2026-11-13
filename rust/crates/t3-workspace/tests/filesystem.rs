use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_contracts::{base_schemas::TrimmedNonEmptyString as Text, filesystem::*, project::*};
use t3_workspace::{
    Error,
    filesystem::{READ_MAX_BYTES, WorkspaceFileSystem},
    paths::{self, InternalPaths, KnownWorkspaces},
};

fn service(root: &Path) -> WorkspaceFileSystem {
    WorkspaceFileSystem {
        home: root.join("home"),
        internal: InternalPaths {
            state_dir: root.join("state"),
            secrets_dir: root.join("secrets"),
        },
        known: {
            let root = root.to_owned();
            Arc::new(move || {
                let root = root.clone();
                async move { KnownWorkspaces::new([root]) }
            })
        },
        indexes: Arc::new(t3_workspace::search::SearchIndexes::new(
            t3_workspace::paths::InternalPaths {
                state_dir: root.join("state"),
                secrets_dir: root.join("secrets"),
            },
        )),
    }
}
fn put(root: &Path, path: &str, bytes: impl AsRef<[u8]>) {
    let target = root.join(path);
    fs::create_dir_all(target.parent().expect("test fixture")).expect("test fixture");
    fs::write(target, bytes).expect("test fixture");
}
fn read(root: &Path, path: &str, missing: bool) -> ProjectReadFileInput {
    ProjectReadFileInput {
        cwd: Text(root.to_string_lossy().into()),
        relative_path: path.into(),
        allow_missing: missing.then_some(Some(true)),
    }
}
fn write(root: &Path, path: &str) -> ProjectWriteFileInput {
    ProjectWriteFileInput {
        cwd: Text(root.to_string_lossy().into()),
        relative_path: path.into(),
        contents: "new\n".into(),
    }
}
fn link_file(target: &Path, link: &Path) {
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(target, link).expect("test fixture");
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).expect("test fixture");
}
fn link_dir(target: &Path, link: &Path) {
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(target, link).expect("test fixture");
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).expect("test fixture");
}

#[tokio::test]
async fn snapshot_grants_require_registered_projects_or_git_verified_thread_worktrees() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let repo = root.join("repo");
    fs::create_dir(&repo).expect("repo");
    let project:t3_contracts::project::Project=serde_json::from_value(serde_json::json!({"id":"p","title":"Project","workspaceRoot":repo.to_string_lossy(),"defaultModelSelection":null,"scripts":[],"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","deletedAt":null})).expect("project");
    async fn git(cwd: &Path, args: &[&str]) {
        let mut options = t3_process::ProcessOptions::new("git").args(args.iter().copied());
        options.cwd = Some(cwd.into());
        options.env.insert(
            "GIT_CONFIG_GLOBAL".into(),
            Some(cwd.join("global-config").into_os_string()),
        );
        options
            .env
            .insert("GIT_CONFIG_NOSYSTEM".into(), Some("1".into()));
        t3_process::run(options).await.expect("git");
    }
    git(&repo, &["init"]).await;
    put(&repo, "file", "initial");
    git(&repo, &["add", "file"]).await;
    git(
        &repo,
        &[
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "core.hooksPath=",
            "commit",
            "-m",
            "initial",
        ],
    )
    .await;
    let worktree = root.join("thread-worktree");
    git(
        &repo,
        &["worktree", "add", "--detach", &worktree.to_string_lossy()],
    )
    .await;
    let unlisted = root.join("unlisted");
    fs::create_dir(&unlisted).expect("unlisted");
    let managed = root.join("managed");
    fs::create_dir(&managed).expect("managed");
    let known = KnownWorkspaces::from_project_snapshot(
        std::slice::from_ref(&project),
        vec![managed.clone()],
        &[
            ("p".into(), worktree.clone()),
            ("p".into(), unlisted.clone()),
        ],
    )
    .await
    .expect("snapshot");
    assert!(known.assert_contains(&repo).is_ok());
    assert!(known.assert_contains(&worktree).is_ok());
    assert!(known.assert_contains(&managed).is_ok());
    assert!(known.assert_contains(&unlisted).is_err());
    let mut deleted = project;
    deleted.deleted_at =
        Some(serde_json::from_value(serde_json::json!("2026-01-02T00:00:00.000Z")).expect("date"));
    let known =
        KnownWorkspaces::from_project_snapshot(&[deleted], vec![], &[("p".into(), worktree)])
            .await
            .expect("deleted");
    assert!(known.assert_contains(&repo).is_err());
}

#[tokio::test]
async fn paths_normalize_create_and_reject_escape() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let home = root.join("home");
    fs::create_dir(&home).expect("test fixture");
    assert_eq!(
        paths::normalize_root(" ~/nested ", &home, true)
            .await
            .expect("test fixture"),
        home.join("nested")
    );
    assert!(matches!(
        paths::normalize_root("~/missing", &home, false).await,
        Err(Error::RootMissing(_))
    ));
    put(root, "file", "");
    assert!(matches!(
        paths::normalize_root(&root.join("file").to_string_lossy(), &home, false).await,
        Err(Error::RootNotDirectory(_))
    ));
    for p in ["", ".", "..", "../escape", "a/../../escape"] {
        assert!(paths::resolve_relative(root, p).is_err(), "{p}");
    }
    assert!(paths::resolve_relative(root, &root.join("file").to_string_lossy()).is_err());
    assert_eq!(
        paths::resolve_relative(root, "a/../file")
            .expect("test fixture")
            .1,
        "file"
    );
    #[cfg(windows)]
    for p in [
        "C:\\file",
        "C:file",
        "\\file",
        "\\\\server\\share\\file",
        "\\\\?\\C:\\file",
        "\\\\.\\C:\\file",
        "a\\..\\..\\escape",
    ] {
        assert!(paths::resolve_relative(root, p).is_err(), "{p}");
    }
    #[cfg(unix)]
    assert_eq!(
        paths::resolve_relative(root, "..\\..\\etc")
            .expect("test fixture")
            .1,
        "..\\..\\etc"
    );
}

#[tokio::test]
async fn read_utf8_host_paths_missing_and_binary_limits() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let fsys = service(root);
    put(root, "docs/readme.md", "hello é 🌎\n");
    let result = fsys
        .read_file(read(root, "docs/./readme.md", false))
        .await
        .expect("test fixture");
    assert_eq!(result.contents, "hello é 🌎\n");
    assert_eq!(result.relative_path, "docs/readme.md");
    assert_eq!(result.byte_length, 14);
    assert!(!result.truncated);
    assert_eq!(result.exists, None);
    let host = tempfile::NamedTempFile::new().expect("test fixture");
    fs::write(host.path(), "outside").expect("test fixture");
    assert!(
        fsys.read_file(read(root, &host.path().to_string_lossy(), false))
            .await
            .is_err()
    );
    put(root, "home/report", "report");
    assert_eq!(
        fsys.read_file(read(root, "~/report", false))
            .await
            .expect("test fixture")
            .contents,
        "report"
    );
    let absent = fsys
        .read_file(read(root, "absent/nested.md", true))
        .await
        .expect("test fixture");
    assert_eq!(absent.exists, Some(Some(false)));
    assert_eq!(absent.byte_length, 0);
    assert!(fsys.read_file(read(root, "absent", false)).await.is_err());
    assert!(
        fsys.read_file(read(root, "../missing", true))
            .await
            .is_err()
    );
    put(root, "binary", b"text\0SECRET");
    let error = fsys
        .read_file(read(root, "binary", false))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Binary(_)));
    assert!(!error.to_string().contains("SECRET"));
    assert!(fsys.read_file(read(root, "docs", false)).await.is_err());
    let mut bytes = vec![b'a'; READ_MAX_BYTES as usize - 1];
    bytes.extend_from_slice("🌎".as_bytes());
    put(root, "large", bytes);
    let result = fsys
        .read_file(read(root, "large", false))
        .await
        .expect("test fixture");
    assert!(result.truncated);
    assert_eq!(result.contents.len(), READ_MAX_BYTES as usize - 1);
    assert!(!result.contents.ends_with('\u{fffd}'));
}

#[tokio::test]
async fn server_internal_and_network_paths_are_not_served() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let fsys = service(root);
    for p in [
        "state/statev2.sqlite",
        "state/statev2.sqlite-wal",
        "secrets/key",
    ] {
        put(root, p, "secret");
        assert!(matches!(
            fsys.read_file(read(root, p, false)).await,
            Err(Error::OutsideRoot(_))
        ));
        assert!(matches!(
            fsys.read_file(read(root, &root.join(p).to_string_lossy(), false))
                .await,
            Err(Error::OutsideRoot(_))
        ));
    }
    let metadata = fsys
        .get_metadata(FilesystemGetMetadataInput {
            paths: vec![
                Text(root.join("state/statev2.sqlite").to_string_lossy().into()),
                Text("//unreachable.invalid/share/secret".into()),
                Text("relative".into()),
            ],
        })
        .await
        .expect("test fixture");
    assert_eq!(metadata.entries, vec![None, None, None]);
    for p in [
        "//unreachable.invalid/share/x",
        "\\\\unreachable.invalid\\share\\x",
        "\\\\?\\C:\\x",
    ] {
        assert!(matches!(
            fsys.read_file(read(root, p, true)).await,
            Err(Error::OutsideRoot(_))
        ));
    }
    for p in [
        "~//localhost/C$/Windows/win.ini",
        r"~/\\localhost\C$\Windows\win.ini",
        "~/C:/Windows/win.ini",
    ] {
        assert!(fsys.read_file(read(root, p, false)).await.is_err(), "{p}");
    }
    #[cfg(windows)]
    {
        let alias = root.join("network-link");
        std::os::windows::fs::symlink_dir(r"\\unreachable.invalid\share", &alias)
            .expect("network-target link");
        assert!(t3_workspace::paths::canonicalize(alias.join("secret")).is_err());
        assert!(
            fsys.read_file(read(root, "network-link/secret", true))
                .await
                .is_err()
        );
        assert!(
            fsys.write_file(write(root, "network-link/secret"))
                .await
                .is_err()
        );
        assert_eq!(
            fsys.get_metadata(FilesystemGetMetadataInput {
                paths: vec![Text(alias.join("secret").to_string_lossy().into())],
            })
            .await
            .expect("network metadata")
            .entries,
            vec![None]
        );
    }
    put(root, "state/ordinary.txt", "visible");
    assert!(
        fsys.read_file(read(root, "state/ordinary.txt", false))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn metadata_preserves_order_and_sniffs_extensionless_files() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let fsys = service(root);
    put(root, "script", b"#!/usr/bin/env python3\n");
    put(root, "image", b"\x89PNG\r\n\x1a\n");
    put(root, "empty", b"");
    let path = |p: &str| Text(root.join(p).to_string_lossy().into());
    let result = fsys
        .get_metadata(FilesystemGetMetadataInput {
            paths: vec![
                path("script"),
                path("image"),
                path("script"),
                path("empty"),
                Text(root.to_string_lossy().into()),
                path("absent"),
            ],
        })
        .await
        .expect("test fixture");
    assert_eq!(result.entries[0], result.entries[2]);
    assert_eq!(
        result.entries[0].as_ref().expect("test fixture").mime_type,
        Some(Text("text/x-python".into()))
    );
    assert_eq!(
        result.entries[1].as_ref().expect("test fixture").mime_type,
        Some(Text("image/png".into()))
    );
    assert_eq!(
        result.entries[3].as_ref().expect("test fixture").mime_type,
        None
    );
    assert_eq!(
        result.entries[4].as_ref().expect("test fixture").kind,
        FilesystemEntryKind::Directory
    );
    assert_eq!(result.entries[5], None);
}

#[tokio::test]
async fn known_workspaces_and_lexical_write_guards() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let fsys = service(root);
    let outside = tempfile::tempdir().expect("test fixture");
    assert!(matches!(
        fsys.write_file(write(outside.path(), "file")).await,
        Err(Error::UnknownWorkspace(_))
    ));
    for p in ["../escape", "nested/../../escape"] {
        assert!(matches!(
            fsys.write_file(write(root, p)).await,
            Err(Error::OutsideRoot(_))
        ));
    }
    assert!(
        fsys.write_file(write(root, &root.join("absolute").to_string_lossy()))
            .await
            .is_err()
    );
    fsys.write_file(write(root, "new/nested/file"))
        .await
        .expect("test fixture");
    assert_eq!(
        fs::read_to_string(root.join("new/nested/file")).expect("test fixture"),
        "new\n"
    );
    assert!(
        !KnownWorkspaces::new([PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" })])
            .expect("test fixture")
            .contains(&fs::canonicalize(root).expect("test fixture"))
    );
}

#[tokio::test]
async fn symlinks_missing_tails_and_dangling_links_are_checked() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let fsys = service(root);
    let outside = tempfile::tempdir().expect("test fixture");
    put(root, "docs/file", "old");
    put(outside.path(), "file", "outside");
    link_dir(outside.path(), &root.join("external"));
    link_file(&outside.path().join("file"), &root.join("external-file"));
    link_file(&outside.path().join("absent"), &root.join("dangling"));
    for p in [
        "external/file",
        "external/new",
        "external/nested/new",
        "external-file",
    ] {
        assert!(
            matches!(
                fsys.write_file(write(root, p)).await,
                Err(Error::PathEscape(_) | Error::OutsideRoot(_))
            ),
            "{p}"
        );
        assert!(
            matches!(
                fsys.read_file(read(root, p, true)).await,
                Err(Error::PathEscape(_) | Error::OutsideRoot(_))
            ),
            "{p}"
        );
    }
    assert!(matches!(
        fsys.write_file(write(root, "dangling")).await,
        Err(Error::Io {
            operation: "realpath-target",
            ..
        })
    ));
    assert!(fsys.read_file(read(root, "dangling", true)).await.is_err());
    link_dir(&root.join("docs"), &root.join("inside"));
    link_file(&root.join("docs/file"), &root.join("inside-file"));
    fsys.write_file(write(root, "inside/new/nested"))
        .await
        .expect("test fixture");
    fsys.write_file(write(root, "inside-file"))
        .await
        .expect("test fixture");
    assert_eq!(
        fs::read_to_string(root.join("docs/file")).expect("test fixture"),
        "new\n"
    );
    assert!(
        fs::symlink_metadata(root.join("inside-file"))
            .expect("test fixture")
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fsys.read_file(read(root, "inside-file", false))
            .await
            .expect("test fixture")
            .contents,
        "new\n"
    );
    assert_eq!(
        fs::read_to_string(outside.path().join("file")).expect("test fixture"),
        "outside"
    );
    link_dir(root, &outside.path().join("workspace-alias"));
    fsys.write_file(write(&outside.path().join("workspace-alias"), "alias-new"))
        .await
        .expect("test fixture");
    assert!(root.join("alias-new").is_file());
}

#[tokio::test]
async fn git_metadata_aliases_bare_repositories_and_linked_worktrees() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let fsys = service(root);
    put(root, ".git/config", "[core]\n");
    put(root, ".bare/HEAD", "ref: refs/heads/main\n");
    put(root, ".bare/objects/info/packs", "");
    put(root, ".bare/refs/heads/main", "");
    put(root, ".bare/hooks/.git/keep", "");
    put(root, ".bare/worktrees/main/HEAD", "");
    put(root, ".bare/worktrees/main/commondir", "../..");
    for p in [
        ".git/config",
        ".git/hooks/pre-commit",
        "sub/.git",
        ".GIT/config",
        ".gıt/config",
        ".git. /config",
        ".git::$INDEX_ALLOCATION/config",
        "GIT~1/config",
        ".bare/config",
        ".bare/hooks/post-checkout",
        ".bare/worktrees/main/commondir",
    ] {
        assert!(
            matches!(
                fsys.write_file(write(root, p)).await,
                Err(Error::PathEscape(_) | Error::OutsideRoot(_))
            ),
            "{p}"
        );
    }
    put(root, "shared/config", "[core]\n");
    fs::remove_file(root.join(".git/config")).expect("test fixture");
    link_file(&root.join("shared/config"), &root.join(".git/config"));
    assert!(matches!(
        fsys.write_file(write(root, ".git/config")).await,
        Err(Error::PathEscape(_))
    ));
    assert_eq!(
        fs::read_to_string(root.join("shared/config")).expect("test fixture"),
        "[core]\n"
    );
    put(root, ".bare/main/.git", "gitdir: ../worktrees/main");
    for p in [
        ".gitignore",
        ".gitattributes",
        ".github/workflows/ci.yml",
        "vendor/lib.git/README.md",
        "plan.md",
    ] {
        fsys.write_file(write(&root.join(".bare/main"), p))
            .await
            .expect("test fixture");
    }
}

#[cfg(windows)]
#[tokio::test]
async fn junctions_case_aliases_and_short_names_cannot_escape() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let fsys = service(root);
    let outside = tempfile::tempdir().expect("test fixture");
    put(outside.path(), "file", "outside");
    let mut cmd = t3_process::ProcessOptions::new("cmd.exe").args([
        "/d",
        "/c",
        "mklink",
        "/J",
        &root.join("junction").to_string_lossy(),
        &outside.path().to_string_lossy(),
    ]);
    cmd.cwd = Some(root.into());
    t3_process::run(cmd).await.expect("test fixture");
    assert!(matches!(
        fsys.write_file(write(root, "junction/new")).await,
        Err(Error::PathEscape(_))
    ));
    assert!(matches!(
        fsys.read_file(read(root, "junction/file", false)).await,
        Err(Error::PathEscape(_))
    ));
    put(root, "MixedCase/file", "ok");
    assert_eq!(
        fsys.read_file(read(root, "mixedcase/FILE", false))
            .await
            .expect("test fixture")
            .contents,
        "ok"
    );
    put(root, ".git/config", "[core]");
    assert!(fsys.write_file(write(root, "GIT~1/config")).await.is_err());
    assert!(!paths::within(
        &PathBuf::from(format!("{}-sibling", root.display())),
        root
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn fifo_is_rejected_without_waiting_for_a_writer() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let fsys = service(root);
    let mut cmd = t3_process::ProcessOptions::new("mkfifo").args([root.join("fifo").as_os_str()]);
    cmd.cwd = Some(root.into());
    t3_process::run(cmd).await.expect("test fixture");
    assert!(matches!(
        fsys.read_file(read(root, "fifo", false)).await,
        Err(Error::NotFile(_))
    ));
}
