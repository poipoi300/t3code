use std::{fs, path::Path, sync::Arc};
use t3_contracts::{
    base_schemas::{TrimmedNonEmptyString as Text, TrimmedString},
    filesystem::*,
    project::*,
};
use t3_workspace::{
    entries::WorkspaceEntries,
    search::{IndexVariant, SearchIndexes},
};
fn put(root: &Path, path: &str, contents: &str) {
    let p = root.join(path);
    fs::create_dir_all(p.parent().expect("test fixture")).expect("test fixture");
    fs::write(p, contents).expect("test fixture");
}
fn entries(home: &Path) -> WorkspaceEntries {
    WorkspaceEntries {
        home: home.into(),
        known: {
            let root = home.to_owned();
            Arc::new(move || {
                let root = root.clone();
                async move { t3_workspace::paths::KnownWorkspaces::new([root]) }
            })
        },
        indexes: Arc::new(t3_workspace::search::SearchIndexes::new(
            t3_workspace::paths::InternalPaths {
                state_dir: home.join("state"),
                secrets_dir: home.join("secrets"),
            },
        )),
    }
}
fn text(p: &Path) -> Text {
    Text(p.to_string_lossy().into())
}
fn search(
    root: &Path,
    query: &str,
    limit: u64,
    kind: Option<ProjectEntryKind>,
    images: bool,
) -> ProjectSearchEntriesInput {
    ProjectSearchEntriesInput {
        cwd: text(root),
        query: TrimmedString(query.into()),
        limit,
        kind: kind.map(Some),
        image_only: Some(Some(images)),
    }
}
fn content(
    root: &Path,
    query: &str,
    whole: bool,
    case: bool,
    regex: bool,
    limit: u64,
) -> ProjectSearchContentsInput {
    ProjectSearchContentsInput {
        cwd: text(root),
        query: query.into(),
        limit,
        case_sensitive: case,
        whole_word: whole,
        use_regex: regex,
    }
}
async fn git(root: &Path, args: &[&str]) {
    let mut options = t3_process::ProcessOptions::new("git").args(args.iter().copied());
    options.cwd = Some(root.into());
    options.env.insert(
        "GIT_CONFIG_GLOBAL".into(),
        Some(root.join("global-config").into_os_string()),
    );
    options
        .env
        .insert("GIT_CONFIG_NOSYSTEM".into(), Some("1".into()));
    t3_process::run(options).await.expect("test fixture");
}

#[tokio::test]
async fn immediate_listing_ignored_empty_directories_and_metadata_hidden() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let service = entries(root);
    git(root, &["init"]).await;
    put(root, ".gitignore", "ignored.txt\nnode_modules/\n");
    put(root, "ignored.txt", "ignored");
    put(root, "visible.txt", "visible");
    fs::create_dir(root.join("empty")).expect("test fixture");
    put(root, "node_modules/lib.js", "module");
    let list = service
        .list(ProjectListEntriesInput {
            cwd: text(root),
            directory_path: Some(Some(TrimmedString("".into()))),
        })
        .await
        .expect("test fixture");
    assert!(!list.truncated);
    assert!(
        list.entries
            .iter()
            .any(|e| e.path.0 == "empty" && e.kind == ProjectEntryKind::Directory)
    );
    assert!(!list.entries.iter().any(|e| e.path.0 == ".git"));
    assert_eq!(
        list.entries
            .iter()
            .find(|e| e.path.0 == "ignored.txt")
            .expect("test fixture")
            .ignored,
        Some(Some(true))
    );
    for dir in ["../", ".git", "missing"] {
        assert!(
            service
                .list(ProjectListEntriesInput {
                    cwd: text(root),
                    directory_path: Some(Some(TrimmedString(dir.into())))
                })
                .await
                .is_err()
        );
    }
    let ignored = service
        .list_ignored_files(ProjectListIgnoredFilesInput {
            cwd: text(root),
            query: None,
        })
        .await
        .expect("test fixture");
    assert_eq!(ignored.files.len(), 1);
    assert_eq!(ignored.files[0].path.0, "ignored.txt");
    assert_eq!(ignored.files[0].byte_length, 7);

    put(root, ".gitignore", "alias\n");
    fs::create_dir(root.join("large")).expect("large directory");
    for index in 0..1001 {
        fs::write(root.join(format!("large/{index}.txt")), "visible").expect("child");
    }
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(root.join("large"), root.join("alias")).expect("alias");
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("large"), root.join("alias")).expect("alias");
    let aliased = service
        .list(ProjectListEntriesInput {
            cwd: text(root),
            directory_path: Some(Some(TrimmedString("alias".into()))),
        })
        .await
        .expect("aliased listing");
    assert_eq!(aliased.entries.len(), 1001);
    assert!(
        aliased.entries.iter().all(|entry| entry.ignored.is_none()),
        "ignored symlink name must not mark its visible target's children ignored"
    );
}

#[tokio::test]
async fn listing_links_hides_escapes_dangling_links_git_targets_and_cycles() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let outside = tempfile::tempdir().expect("test fixture");
    let service = entries(root);
    put(root, "docs/file", "ok");
    put(root, ".git/config", "private");
    #[cfg(windows)]
    let link = std::os::windows::fs::symlink_dir;
    #[cfg(unix)]
    let link = std::os::unix::fs::symlink;
    for (target, name) in [
        (root.join("docs"), "alias"),
        (outside.path().to_path_buf(), "escape"),
        (root.to_path_buf(), "cycle"),
        (root.join(".git"), "git-alias"),
        (root.join("missing"), "dangling"),
    ] {
        link(target, root.join(name)).expect("test fixture");
    }
    let list = service
        .list(ProjectListEntriesInput {
            cwd: text(root),
            directory_path: Some(Some(TrimmedString("".into()))),
        })
        .await
        .expect("test fixture");
    let names: Vec<_> = list.entries.iter().map(|e| e.path.0.as_str()).collect();
    assert!(names.contains(&"alias"));
    for hidden in ["escape", "cycle", "git-alias", "dangling"] {
        assert!(!names.contains(&hidden), "{hidden}");
    }
    assert!(
        service
            .list(ProjectListEntriesInput {
                cwd: text(root),
                directory_path: Some(Some(TrimmedString("escape".into())))
            })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn fuzzy_search_keeps_fff_ranking_kind_filters_images_and_query_workarounds() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let service = entries(root);
    for path in [
        "src/main.ts",
        "src/app/game/[id]/index.tsx",
        "src/foo{bar}.ts",
        "assets/logo.svg",
        "assets/logo.ts",
        "images/blob.png",
    ] {
        put(root, path, "text");
    }
    let result = service
        .search(search(
            root,
            "@./src/main",
            10,
            Some(ProjectEntryKind::File),
            false,
        ))
        .await
        .expect("test fixture");
    assert_eq!(result.entries[0].path.0, "src/main.ts");
    for query in ["game/[id]/index", "foo{bar}"] {
        assert!(
            !service
                .search(search(root, query, 10, Some(ProjectEntryKind::File), false))
                .await
                .expect("test fixture")
                .entries
                .is_empty(),
            "{query}"
        );
    }
    let result = service
        .search(search(root, "logo", 1, None, true))
        .await
        .expect("test fixture");
    assert_eq!(result.entries.len(), 1);
    assert_eq!(result.entries[0].path.0, "assets/logo.svg");
    let dirs = service
        .search(search(
            root,
            "src",
            10,
            Some(ProjectEntryKind::Directory),
            false,
        ))
        .await
        .expect("test fixture");
    assert!(
        dirs.entries
            .iter()
            .all(|e| e.kind == ProjectEntryKind::Directory)
    );
    let list = service
        .list(ProjectListEntriesInput {
            cwd: text(root),
            directory_path: None,
        })
        .await
        .expect("test fixture");
    assert!(list.entries.iter().any(|e| e.path.0 == "images/blob.png"));
    assert!(list.entries.iter().any(|e| e.path.0 == "src/app/game"));
    let actual = service
        .search(search(root, "main", 5, Some(ProjectEntryKind::File), false))
        .await
        .expect("test fixture");
    let shared = fff_search::SharedFilePicker::default();
    fff_search::FilePicker::new_with_shared_state(
        shared.clone(),
        fff_search::SharedFrecency::default(),
        fff_search::FilePickerOptions {
            base_path: root.to_string_lossy().into(),
            ..Default::default()
        },
    )
    .expect("test fixture");
    assert!(shared.wait_for_scan(std::time::Duration::from_secs(15)));
    {
        let guard = shared.read().expect("test fixture");
        let picker = guard.as_ref().expect("test fixture");
        let parser = fff_search::QueryParser::default();
        let q = parser.parse("main");
        let expected = picker.fuzzy_search(
            &q,
            None,
            fff_search::FuzzySearchOptions {
                project_path: Some(picker.base_path()),
                pagination: fff_search::PaginationArgs {
                    offset: 0,
                    limit: 6,
                },
                ..Default::default()
            },
        );
        assert_eq!(
            actual.entries[0].path.0,
            expected.items[0].relative_path(picker).replace('\\', "/")
        );
    }
    shared.cancel();
    if let Some(mut picker) = shared.write().expect("test fixture").take() {
        picker.stop_background_monitor();
    }
}

#[tokio::test]
async fn search_applies_gitignore_info_exclude_and_tracked_ignore_rules() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    git(root, &["init"]).await;
    put(root, ".gitignore", "ignored/\ntracked.txt\n");
    put(root, ".git/info/exclude", "excluded.txt\n");
    put(root, "ignored/file.ts", "hidden");
    put(root, "tracked.txt", "tracked");
    put(root, "excluded.txt", "hidden");
    put(root, "visible.ts", "visible");
    git(root, &["add", "-f", "tracked.txt"]).await;
    let service = entries(root);
    let list = service
        .list(ProjectListEntriesInput {
            cwd: text(root),
            directory_path: None,
        })
        .await
        .expect("test fixture");
    for hidden in ["ignored/file.ts", "excluded.txt", "tracked.txt"] {
        assert!(!list.entries.iter().any(|e| e.path.0 == hidden), "{hidden}");
    }
    assert!(list.entries.iter().any(|e| e.path.0 == "visible.ts"));
}

#[tokio::test]
async fn content_search_case_regex_unicode_whole_words_and_utf16_ranges() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let service = entries(root);
    put(
        root,
        "src/shapes.ts",
        "export const square = 4;\nexport const Square = 16;\nexport const squareSize = 8;\né🌎 square square\ncaté cat 𐐀cat cat𐐀 cat\n",
    );
    let exact = service
        .search_contents(content(root, "square", true, true, false, 100))
        .await
        .expect("test fixture");
    assert_eq!(exact.matches.len(), 2);
    assert_eq!(exact.matches[1].match_ranges[0].start, 4);
    let case = service
        .search_contents(content(root, "SQUARE", true, false, false, 100))
        .await
        .expect("test fixture");
    assert_eq!(case.matches.len(), 3);
    let word = service
        .search_contents(content(root, "cat", true, true, false, 100))
        .await
        .expect("test fixture");
    assert_eq!(word.matches.len(), 1);
    assert_eq!(word.matches[0].match_ranges.len(), 2);
    put(root, "regex.txt", "[hello\n[HELLO\n123\n");
    service.indexes.refresh(root).await.expect("test fixture");
    let fallback = service
        .search_contents(content(root, "[HELLO", false, false, true, 100))
        .await
        .expect("test fixture");
    assert!(fallback.regex_fallback_error.is_some());
    assert!(fallback.matches.len() >= 2);
    let digits = service
        .search_contents(content(root, r"\d+", false, false, true, 100))
        .await
        .expect("test fixture");
    assert!(digits.matches.iter().any(|m| m.line_content == "123"));
}

#[tokio::test]
async fn whole_word_search_continues_after_filtered_pages_and_limits_dense_files() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let service = entries(root);
    put(root, "a.txt", &format!("{}cat\n", "catalog\n".repeat(90)));
    put(root, "b.txt", "cat\n");
    let results = service
        .search_contents(content(root, "cat", true, true, false, 2))
        .await
        .expect("test fixture");
    assert_eq!(results.matches.len(), 2);
    assert!(results.matches.iter().all(|m| m.line_content == "cat"));
    put(root, "dense.txt", &"needle\n".repeat(150));
    service.indexes.refresh(root).await.expect("test fixture");
    let results = service
        .search_contents(content(root, "needle", false, true, false, 200))
        .await
        .expect("test fixture");
    assert_eq!(results.matches.len(), 100);
}

#[tokio::test]
async fn watcher_delivers_changes_and_refresh_sees_new_files() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    put(root, "file.txt", "before");
    let indexes = SearchIndexes::new(t3_workspace::paths::InternalPaths {
        state_dir: root.join("state"),
        secrets_dir: root.join("secrets"),
    });
    let index = indexes
        .get(root, IndexVariant::Paths)
        .await
        .expect("test fixture");
    let mut watch = index.watch("file.txt".into()).await.expect("test fixture");
    fs::write(root.join("file.txt"), "after").expect("test fixture");
    let events = watch.receiver.recv().await.expect("test fixture");
    assert!(!events.is_empty());
    put(root, "new/file.txt", "new");
    indexes.refresh(root).await.expect("test fixture");
    let list = index.list().await.expect("test fixture");
    assert!(list.entries.iter().any(|e| e.path.0 == "new/file.txt"));
}

#[tokio::test]
async fn browsing_hidden_prefix_and_explicit_relative_paths() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let service = entries(root);
    fs::create_dir(root.join(".hidden")).expect("test fixture");
    fs::create_dir(root.join("Visible")).expect("test fixture");
    let all = service
        .browse(FilesystemBrowseInput {
            partial_path: Text(format!("{}/", root.display())),
            cwd: None,
        })
        .await
        .expect("test fixture");
    assert_eq!(all.entries.len(), 2);
    let hidden = service
        .browse(FilesystemBrowseInput {
            partial_path: Text(format!("{}/.", root.display())),
            cwd: None,
        })
        .await
        .expect("test fixture");
    assert_eq!(hidden.entries.len(), 1);
    assert!(
        service
            .browse(FilesystemBrowseInput {
                partial_path: Text("./".into()),
                cwd: None
            })
            .await
            .is_err()
    );
    let relative = service
        .browse(FilesystemBrowseInput {
            partial_path: Text("./vi".into()),
            cwd: Some(Some(text(root))),
        })
        .await
        .expect("test fixture");
    assert_eq!(relative.entries[0].name.0, "Visible");
}
