use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_contracts::{
    base_schemas::{TrimmedNonEmptyString as Text, TrimmedString},
    filesystem::*,
    project::*,
};
use t3_workspace::{
    entries::WorkspaceEntries,
    filesystem::WorkspaceFileSystem,
    paths::{self, InternalPaths, KnownWorkspaces},
};
fn text(p: &Path) -> Text {
    Text(p.to_string_lossy().into())
}
fn fsys(root: &Path) -> WorkspaceFileSystem {
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
fn entries(root: &Path) -> WorkspaceEntries {
    WorkspaceEntries {
        home: root.into(),
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
fn read(root: &Path, p: &str) -> ProjectReadFileInput {
    ProjectReadFileInput {
        cwd: text(root),
        relative_path: p.into(),
        allow_missing: Some(Some(true)),
    }
}
fn write(root: &Path, p: &str) -> ProjectWriteFileInput {
    ProjectWriteFileInput {
        cwd: text(root),
        relative_path: p.into(),
        contents: "changed".into(),
    }
}
fn search(root: &Path, q: &str, limit: u64) -> ProjectSearchEntriesInput {
    ProjectSearchEntriesInput {
        cwd: text(root),
        query: TrimmedString(q.into()),
        limit,
        kind: None,
        image_only: None,
    }
}
#[test]
fn home_expansion_never_replaces_home_with_a_suffix_prefix() {
    let home = if cfg!(windows) {
        Path::new("C:/home/user")
    } else {
        Path::new("/home/user")
    };
    for suffix in [
        "//localhost/share/file",
        r"\\localhost\share\file",
        "C:/Windows/win.ini",
        r"\Windows\win.ini",
        "/etc/passwd",
    ] {
        let expanded = paths::expand_home(&format!("~/{suffix}"), home);
        assert!(
            paths::within(&expanded, home),
            "{suffix}: {}",
            expanded.display()
        );
    }
}
#[tokio::test]
async fn hardlinks_cannot_mutate_outside_or_read_internal_files() {
    let tmp = tempfile::tempdir().expect("temp");
    let root = tmp.path().join("project");
    fs::create_dir(&root).expect("root");
    let service = fsys(&root);
    let outside = tmp.path().join("outside.txt");
    fs::write(&outside, "original").expect("write");
    fs::hard_link(&outside, root.join("alias.txt")).expect("hardlink");
    let result = service.write_file(write(&root, "alias.txt")).await;
    assert!(result.is_err(), "hardlink write accepted");
    assert_eq!(fs::read_to_string(&outside).expect("outside"), "original");
    fs::create_dir(root.join("state")).expect("state");
    fs::write(root.join("state/statev2.sqlite"), "private").expect("state");
    fs::hard_link(root.join("state/statev2.sqlite"), root.join("innocent.txt")).expect("hardlink");
    assert!(
        service
            .read_file(read(&root, "innocent.txt"))
            .await
            .is_err()
    );
    assert_eq!(
        service
            .get_metadata(FilesystemGetMetadataInput {
                paths: vec![text(&root.join("innocent.txt"))]
            })
            .await
            .expect("metadata")
            .entries,
        vec![None]
    );
}
#[cfg(windows)]
async fn short_path(path: &Path) -> PathBuf {
    let mut options=t3_process::ProcessOptions::new("powershell.exe").args(["-NoProfile","-NonInteractive","-Command", "$f = New-Object -ComObject Scripting.FileSystemObject; if (Test-Path -LiteralPath $env:T3_SHORT_PATH -PathType Container) { $f.GetFolder($env:T3_SHORT_PATH).ShortPath } else { $f.GetFile($env:T3_SHORT_PATH).ShortPath }"]);
    options
        .env
        .insert("T3_SHORT_PATH".into(), Some(path.as_os_str().to_owned()));
    let output = t3_process::run(options).await.expect("short path");
    let short = PathBuf::from(output.stdout_text().trim());
    assert!(
        short.exists(),
        "short alias does not exist: {}",
        short.display()
    );
    assert!(
        short
            .file_name()
            .expect("name")
            .to_string_lossy()
            .contains('~'),
        "real 8.3 alias required: {}",
        short.display()
    );
    short
}
#[cfg(windows)]
#[tokio::test]
async fn upload_resolution_rejects_a_real_short_alias() {
    use t3_workspace::attachments;

    let tmp = tempfile::tempdir().expect("temp");
    let id = attachments::pending_id(None);
    let path = tmp.path().join(format!("{id}.png"));
    fs::write(&path, "private").expect("write");
    let short = short_path(&path).await;
    let alias = short.file_stem().expect("stem").to_str().expect("utf8");
    assert!(
        tmp.path().join(format!("{alias}.png")).exists(),
        "alias {alias} does not resolve"
    );
    assert!(
        attachments::resolve_by_id(tmp.path(), alias).is_none(),
        "resolved another upload via {alias}"
    );
}
#[tokio::test]
async fn boundaries_enforce_wire_limits_before_indexing() {
    let tmp = tempfile::tempdir().expect("temp");
    let root = tmp.path();
    let fs = fsys(root);
    let entries = entries(root);
    for q in ["x".repeat(257), "🌎".repeat(129)] {
        assert!(
            entries.search(search(root, &q, 1)).await.is_err(),
            "query length"
        );
    }
    for limit in [0, 201, u64::MAX] {
        assert!(
            entries.search(search(root, "", limit)).await.is_err(),
            "limit {limit}"
        );
    }
    for q in ["".into(), "x".repeat(257)] {
        assert!(
            entries
                .search_contents(ProjectSearchContentsInput {
                    cwd: text(root),
                    query: q,
                    limit: 1,
                    case_sensitive: true,
                    whole_word: false,
                    use_regex: false
                })
                .await
                .is_err()
        );
    }
    for limit in [0, 501, u64::MAX] {
        assert!(
            entries
                .search_contents(ProjectSearchContentsInput {
                    cwd: text(root),
                    query: "x".into(),
                    limit,
                    case_sensitive: true,
                    whole_word: false,
                    use_regex: false
                })
                .await
                .is_err()
        );
    }
    for p in ["".into(), "x".repeat(513)] {
        assert!(fs.read_file(read(root, &p)).await.is_err(), "read bounds");
        assert!(
            fs.write_file(write(root, &p)).await.is_err(),
            "write bounds"
        );
    }
}
#[cfg(windows)]
#[tokio::test]
async fn windows_streams_devices_and_trailing_aliases_are_refused() {
    let tmp = tempfile::tempdir().expect("temp");
    let root = tmp.path();
    fs::write(root.join("file.txt"), "original").expect("file");
    let service = fsys(root);
    for p in [
        "file.txt:stream",
        "file.txt::$DATA",
        "file.txt.",
        "file.txt ",
        "NUL",
        "COM1.txt",
        "dir./x",
    ] {
        assert!(
            service.write_file(write(root, p)).await.is_err(),
            "write accepted {p}"
        );
        assert!(
            service.read_file(read(root, p)).await.is_err(),
            "read accepted {p}"
        );
    }
}
#[cfg(windows)]
#[tokio::test]
async fn browse_refuses_network_even_when_the_share_exists() {
    let tmp = tempfile::tempdir().expect("temp");
    let service = entries(tmp.path());
    assert!(
        service
            .browse(FilesystemBrowseInput {
                partial_path: Text("//localhost/C$/Windows/".into()),
                cwd: None
            })
            .await
            .is_err()
    );
}
#[tokio::test]
async fn read_strips_only_one_bom() {
    let tmp = tempfile::tempdir().expect("temp");
    fs::write(tmp.path().join("bom.txt"), "\u{feff}\u{feff}text").expect("file");
    assert_eq!(
        fsys(tmp.path())
            .read_file(read(tmp.path(), "bom.txt"))
            .await
            .expect("read")
            .contents,
        "\u{feff}text"
    );
}
#[tokio::test]
async fn nested_worktree_rooted_queries_never_emit_empty_or_outside_entries() {
    let tmp = tempfile::tempdir().expect("temp");
    let root = tmp.path();
    async fn git(root: &Path, args: &[&str]) {
        let mut opts = t3_process::ProcessOptions::new("git").args(args.iter().copied());
        opts.cwd = Some(root.into());
        opts.env
            .insert("GIT_CONFIG_NOSYSTEM".into(), Some("1".into()));
        opts.env.insert(
            "GIT_CONFIG_GLOBAL".into(),
            Some(root.join("no-global").into_os_string()),
        );
        t3_process::run(opts).await.expect("git");
    }
    git(root, &["init"]).await;
    git(
        root,
        &[
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "core.hooksPath=",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    )
    .await;
    let nested = root.join("nested");
    git(
        root,
        &[
            "worktree",
            "add",
            "--detach",
            nested.to_str().expect("path"),
        ],
    )
    .await;
    fs::write(nested.join("file.txt"), "nested").expect("write");
    let service = entries(root);
    let valid = service
        .search(search(root, "nested/file.txt", 10))
        .await
        .expect("valid");
    assert!(valid.entries.iter().any(|e| e.path.0 == "nested/file.txt"));
    for q in [
        "/Windows/win.ini",
        r"\Windows\win.ini",
        "C:/Windows/win.ini",
        "C:Windows/win.ini",
        "//localhost/C$/Windows/win.ini",
        "../outside/file",
    ] {
        let result = service.search(search(root, q, 10)).await;
        if let Ok(result) = result {
            assert!(
                result
                    .entries
                    .iter()
                    .all(|e| !e.path.0.is_empty()
                        && paths::resolve_relative(root, &e.path.0).is_ok()),
                "{q}"
            );
        }
    }
}

#[tokio::test]
async fn metadata_and_writes_have_resource_bounds() {
    let tmp = tempfile::tempdir().expect("temp");
    let root = tmp.path();
    let service = fsys(root);
    assert!(
        service
            .get_metadata(FilesystemGetMetadataInput {
                paths: vec![text(root); 1025]
            })
            .await
            .is_err()
    );
    let mut input = write(root, "large.txt");
    input.contents = "x".repeat(10 * 1024 * 1024 + 1);
    assert!(service.write_file(input).await.is_err());
    assert!(!root.join("large.txt").exists());
}

#[tokio::test]
async fn hardlinked_state_files_are_hidden_even_under_an_innocent_name() {
    let tmp = tempfile::tempdir().expect("temp");
    let root = tmp.path();
    let service = fsys(root);
    fs::create_dir(root.join("state")).expect("state");
    fs::write(root.join("state/statev2.sqlite"), "private").expect("state");
    fs::hard_link(root.join("state/statev2.sqlite"), root.join("innocent.txt")).expect("hardlink");
    assert!(service.read_file(read(root, "innocent.txt")).await.is_err());
}

#[tokio::test]
async fn revoked_workspace_authorization_is_rechecked_at_each_save() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let source = Arc::new(std::sync::RwLock::new(
        KnownWorkspaces::new([root.into()]).expect("roots"),
    ));
    let mut service = fsys(root);
    service.known = {
        let source = source.clone();
        Arc::new(move || {
            let snapshot = source.read().expect("snapshot").clone();
            async move { Ok(snapshot) }
        })
    };
    service
        .write_file(write(root, "before.txt"))
        .await
        .expect("initially authorized");
    *source.write().expect("revoke") = KnownWorkspaces::default();
    assert!(
        service.write_file(write(root, "after.txt")).await.is_err(),
        "revoked workspace remains writable"
    );
}

#[tokio::test]
async fn containment_matrix_covers_every_workspace_and_attachment_entry_point() {
    use t3_workspace::{
        assets::AssetSigner,
        attachments::{AttachmentStore, DocumentText},
        search::IndexVariant,
    };
    struct NoDocuments;
    impl DocumentText for NoDocuments {
        fn ensure<'a>(
            &'a self,
            _: &'a Path,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
            Box::pin(async {})
        }
    }
    let tmp = tempfile::tempdir().expect("temp");
    let root = tmp.path().join("workspace");
    let outside = tmp.path().join("outside");
    fs::create_dir(&root).expect("root");
    fs::create_dir(&outside).expect("outside");
    fs::create_dir(root.join("home")).expect("home");
    fs::write(outside.join("secret.txt"), "OUTSIDE_SECRET").expect("secret");
    fs::write(root.join("inside.txt"), "inside").expect("inside");
    #[cfg(windows)]
    let link = std::os::windows::fs::symlink_dir;
    #[cfg(unix)]
    let link = std::os::unix::fs::symlink;
    link(&outside, root.join("escape")).expect("symlink");
    let mut cases: Vec<String> = [
        "../outside/secret.txt",
        "safe/../../outside/secret.txt",
        "escape/secret.txt",
        "escape/new.txt",
        "\0",
        "~/../..",
        "~",
        "~/",
        "~\\",
        "~//localhost/C$/Windows/win.ini",
        r"~/\\localhost\C$\Windows\win.ini",
        "~/C:/Windows/win.ini",
        r"~\\\\localhost\C$\Windows\win.ini",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    cases.push(outside.join("secret.txt").to_string_lossy().into());
    #[cfg(windows)]
    {
        for path in [
            "C:secret.txt",
            r"\Windows\win.ini",
            r"\\localhost\C$\Windows\win.ini",
            r"\\?\C:\Windows\win.ini",
            r"\\.\C:\Windows\win.ini",
            "inside.txt:stream",
            "inside.txt::$DATA",
            "inside.txt.",
            "inside.txt ",
            "NUL",
            "COM1.txt",
            "LPT¹.txt",
            r"safe\..\..\outside/secret.txt",
        ] {
            cases.push(path.into());
        }
        let opts = t3_process::ProcessOptions::new("cmd.exe").args([
            "/d",
            "/c",
            "mklink",
            "/J",
            root.join("junction").to_str().expect("path"),
            outside.to_str().expect("path"),
        ]);
        t3_process::run(opts).await.expect("junction");
        cases.push("junction/secret.txt".into());
        fs::create_dir(root.join("g i t")).expect("occupy short name");
        fs::create_dir(root.join(".git")).expect("git directory");
        fs::write(root.join(".git/config"), "[core]\n").expect("config");
        let alias = short_path(&root.join(".git")).await;
        let name = alias.file_name().expect("name").to_str().expect("unicode");
        assert_ne!(name.to_uppercase(), "GIT~1");
        cases.push(format!("{name}/config"));
        fs::write(root.join("a long workspace name.txt"), "inside alias").expect("file");
        let alias = short_path(&root.join("a long workspace name.txt")).await;
        cases.push(alias.file_name().expect("name").to_string_lossy().into());
        cases.push("INSIDE.TXT".into());
    }
    let service = fsys(&root);
    let mut folders = entries(&root);
    folders.home = service.home.clone();
    let index = service
        .indexes
        .get(&root, IndexVariant::Paths)
        .await
        .expect("index");
    let store = AttachmentStore::new(
        root.join("attachments"),
        AssetSigner::new(vec![7; 32]).expect("signer"),
        Arc::new(NoDocuments),
    );
    for candidate in cases {
        if let Ok(result) = service.read_file(read(&root, &candidate)).await {
            assert_ne!(result.contents, "OUTSIDE_SECRET", "read {candidate}");
        }
        if let Ok(result) = service.write_file(write(&root, &candidate)).await {
            let (path, _) =
                paths::resolve_relative(&root, &result.relative_path).expect("write relative");
            let real = paths::canonicalize(path).expect("write path");
            assert!(
                paths::within(&real, &paths::canonicalize(&root).expect("root")),
                "write {candidate}"
            );
        }
        if let Ok(result) = folders
            .list(ProjectListEntriesInput {
                cwd: text(&root),
                directory_path: Some(Some(TrimmedString(candidate.clone()))),
            })
            .await
        {
            for entry in result.entries {
                let path = paths::canonicalize(root.join(&entry.path.0)).expect("entry");
                assert!(
                    paths::within(&path, &paths::canonicalize(&root).expect("root")),
                    "list {candidate}"
                );
            }
        }
        if let Ok(watch) = index.watch(candidate.clone()).await {
            drop(watch);
        }
        if let Ok(result) = folders.search(search(&root, &candidate, 10)).await {
            for entry in result.entries {
                assert!(!entry.path.0.is_empty(), "search {candidate}");
                let (target, _) =
                    paths::resolve_relative(&root, &entry.path.0).expect("search relative");
                let path = paths::canonicalize(target).expect("search path");
                assert!(
                    paths::within(&path, &paths::canonicalize(&root).expect("root")),
                    "search {candidate}"
                );
            }
        }
        let hostile_cwd = if Path::new(&candidate).is_absolute() {
            PathBuf::from(&candidate)
        } else {
            root.join(&candidate)
        };
        if let Ok(result) = folders
            .search_contents(ProjectSearchContentsInput {
                cwd: text(&hostile_cwd),
                query: "OUTSIDE_SECRET".into(),
                limit: 10,
                case_sensitive: true,
                whole_word: false,
                use_regex: false,
            })
            .await
        {
            assert!(result.matches.is_empty(), "content {candidate}");
        }
        if let Ok(result) = folders
            .list_ignored_files(ProjectListIgnoredFilesInput {
                cwd: text(&hostile_cwd),
                query: None,
            })
            .await
        {
            assert!(result.files.is_empty(), "ignored {candidate}");
        }
        if let Ok(result) = folders
            .browse(FilesystemBrowseInput {
                partial_path: Text(candidate.clone()),
                cwd: Some(Some(text(&root))),
            })
            .await
        {
            for entry in result.entries {
                assert!(
                    paths::within(
                        &paths::canonicalize(&entry.full_path.0).expect("browse"),
                        &paths::canonicalize(&root).expect("root")
                    ),
                    "browse {candidate}"
                );
            }
        }
        let _ = service
            .get_metadata(FilesystemGetMetadataInput {
                paths: vec![Text(candidate.clone())],
            })
            .await;
        store
            .delete_pending(&candidate)
            .await
            .expect("delete invalid id");
        assert_eq!(
            fs::read_to_string(outside.join("secret.txt")).expect("outside"),
            "OUTSIDE_SECRET",
            "changed outside via {candidate}"
        );
        assert_eq!(
            fs::read_dir(&outside).expect("outside").count(),
            1,
            "created outside via {candidate}"
        );
    }
}
#[cfg(windows)]
#[tokio::test]
async fn real_short_names_and_case_aliases_are_checked_after_canonicalization() {
    let tmp = tempfile::tempdir().expect("temp");
    let root = tmp.path();
    fs::create_dir(root.join("g i t")).expect("occupy GIT~1");
    fs::create_dir(root.join(".git")).expect("git");
    fs::write(root.join(".git/config"), "git metadata").expect("config");
    let alias = short_path(&root.join(".git")).await;
    let name = alias.file_name().expect("name").to_str().expect("unicode");
    assert_ne!(
        name.to_uppercase(),
        "GIT~1",
        "fixture must resolve a different real alias"
    );
    let service = fsys(root);
    let folders = entries(root);
    assert!(
        service
            .write_file(write(root, &format!("{name}/config")))
            .await
            .is_err()
    );
    assert!(
        folders
            .list(ProjectListEntriesInput {
                cwd: text(root),
                directory_path: Some(Some(TrimmedString(name.into())))
            })
            .await
            .is_err()
    );
    fs::write(root.join("a sufficiently long file name.txt"), "inside").expect("file");
    let alias = short_path(&root.join("a sufficiently long file name.txt")).await;
    let name = alias.file_name().expect("name").to_str().expect("unicode");
    assert_eq!(
        service
            .read_file(read(root, name))
            .await
            .expect("short read")
            .contents,
        "inside"
    );
    service
        .write_file(write(root, name))
        .await
        .expect("short write");
    assert_eq!(
        fs::read_to_string(root.join("a sufficiently long file name.txt")).expect("file"),
        "changed"
    );
    let mixed = root.join("MiXeD");
    fs::create_dir(&mixed).expect("mixed");
    fs::write(mixed.join("file.txt"), "case").expect("file");
    assert_eq!(
        service
            .read_file(read(root, "mixed/FILE.TXT"))
            .await
            .expect("case")
            .contents,
        "case"
    );
}
#[test]
fn invalid_unicode_cannot_enter_wire_strings_or_path_helpers() {
    assert!(serde_json::from_str::<String>(r#""\ud800""#).is_err());
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        let path = PathBuf::from(std::ffi::OsString::from_wide(&[0xd800]));
        assert!(paths::validate_host_path(&path).is_err());
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let path = PathBuf::from(std::ffi::OsString::from_vec(vec![0xff]));
        assert!(paths::validate_host_path(&path).is_err());
    }
}
