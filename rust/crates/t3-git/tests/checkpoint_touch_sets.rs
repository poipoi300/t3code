mod common;
use common::{Repository, TestResult};
use t3_git::{ExecuteOptions, GitError, checkpoint_ref};

async fn index_is_head(repo: &Repository) -> TestResult {
    repo.command(&["diff", "--cached", "--quiet", "HEAD", "--", "."])
        .await?;
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn filesystem_case_variants_and_planted_git_do_not_hide_write_ancestors() -> TestResult {
    for (tracked, linked, ignorecase, planted) in [
        ("Ä", "ä", "true", false),
        ("UPPER", "upper", "false", false),
        ("packages/a/dist", "packages/a/dist", "true", true),
    ] {
        let repo = Repository::new().await?;
        repo.write(&format!("{tracked}/x"), "checkpoint\n")?;
        repo.write(".gitignore", "")?;
        let commit = repo.commit("base").await?;
        repo.command(&["config", "core.ignorecase", ignorecase])
            .await?;
        std::fs::remove_dir_all(repo.root().join(tracked))?;
        let target = repo.temp.path().join("outside");
        std::fs::create_dir(&target)?;
        std::fs::write(target.join("x"), "outside\n")?;
        junction::create(&target, repo.root().join(linked))?;
        let git = if planted {
            repo.write("dist", "ignored root file\n")?;
            repo.write(".gitignore", "/dist\n")?;
            std::fs::create_dir(repo.root().join("packages/a/.git"))?;
            repo.git.at(repo.root().join("packages/a"))
        } else {
            repo.write(".gitignore", format!("{linked}/\n"))?;
            repo.git.clone()
        };
        let result = git.restore_checkpoint(&commit, false).await;
        assert!(
            matches!(result, Err(GitError::UnsafeRestore(_))),
            "{tracked}: {result:?}"
        );
        assert_eq!(std::fs::read_to_string(target.join("x"))?, "outside\n");
        index_is_head(&repo).await?;
        junction::delete(repo.root().join(linked))?;
    }
    Ok(())
}

#[tokio::test]
async fn tracked_directory_symlinks_are_captured_and_restored_as_links() -> TestResult {
    let repo = Repository::new().await?;
    repo.command(&["config", "core.symlinks", "true"]).await?;
    repo.write("target/x", "inside\n")?;
    let link = repo.root().join("current");
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir("target", &link)?;
    #[cfg(unix)]
    std::os::unix::fs::symlink("target", &link)?;
    repo.commit("base").await?;
    let reference = checkpoint_ref("directory-symlink", 0);
    repo.git.capture_checkpoint(&reference).await?;
    assert!(
        repo.command(&["ls-tree", &reference, "--", "current"])
            .await?
            .starts_with("120000 blob ")
    );
    #[cfg(windows)]
    std::fs::remove_dir(&link)?;
    #[cfg(unix)]
    std::fs::remove_file(&link)?;
    std::fs::write(&link, "replace me\n")?;
    assert!(repo.git.restore_checkpoint(&reference, false).await?);
    assert_eq!(
        std::fs::read_link(&link)?,
        std::path::PathBuf::from("target")
    );
    assert_eq!(
        std::fs::read_to_string(repo.root().join("target/x"))?,
        "inside\n"
    );
    index_is_head(&repo).await?;
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn nested_repositories_and_submodules_with_links_are_leaves() -> TestResult {
    for tracked in [false, true] {
        let repo = Repository::new().await?;
        repo.write("safe", "base\n")?;
        let without_gitlink = repo.commit("base").await?;
        repo.command(&["init", "parent/nested"]).await?;
        let nested = repo.git.at(repo.root().join("parent/nested"));
        nested
            .execute(
                &["commit", "--allow-empty", "-m", "nested"],
                ExecuteOptions::default(),
            )
            .await?;
        if tracked {
            repo.command(&["add", "parent/nested"]).await?;
            repo.command(&["commit", "-m", "gitlink"]).await?;
        }
        let reference = repo.command(&["rev-parse", "HEAD"]).await?;
        let outside = repo.temp.path().join("outside");
        std::fs::create_dir(&outside)?;
        std::fs::write(outside.join("marker"), "untouched\n")?;
        junction::create(&outside, nested.cwd().join("node_modules"))?;
        repo.write("safe", "dirty\n")?;
        repo.write("parent/junk", "clean me\n")?;
        assert!(repo.git.restore_checkpoint(reference.trim(), false).await?);
        assert!(junction::exists(nested.cwd().join("node_modules"))?);
        assert_eq!(
            std::fs::read_to_string(outside.join("marker"))?,
            "untouched\n"
        );
        assert!(!repo.root().join("parent/junk").exists());
        if tracked {
            assert!(repo.git.restore_checkpoint(&without_gitlink, false).await?);
            assert!(junction::exists(nested.cwd().join("node_modules"))?);
            index_is_head(&repo).await?;
        }
        junction::delete(nested.cwd().join("node_modules"))?;
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn unhidden_junction_is_cleaned_without_following_its_target() -> TestResult {
    let repo = Repository::new().await?;
    repo.write(".gitignore", "")?;
    repo.write("safe", "checkpoint\n")?;
    let checkpoint = repo.commit("checkpoint").await?;
    repo.write("safe", "HEAD\n")?;
    repo.commit("HEAD").await?;
    repo.write(".gitignore", "build/\n")?;
    let outside = repo.temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("marker"), "untouched\n")?;
    std::fs::create_dir(repo.root().join("build"))?;
    junction::create(&outside, repo.root().join("build/link"))?;
    let result = repo.git.restore_checkpoint(&checkpoint, false).await;
    assert!(matches!(result, Ok(true)), "{result:?}");
    assert!(!repo.root().join("build").exists());
    assert_eq!(
        std::fs::read_to_string(repo.root().join("safe"))?,
        "checkpoint\n"
    );
    index_is_head(&repo).await?;
    assert_eq!(
        std::fs::read_to_string(outside.join("marker"))?,
        "untouched\n"
    );
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn clean_checks_untracked_parents_but_never_visits_ignored_roots() -> TestResult {
    for parent in [false, true] {
        let repo = Repository::new().await?;
        repo.write(".gitignore", "node_modules\n")?;
        let checkpoint = repo.commit("base").await?;
        let outside = repo.temp.path().join("outside");
        std::fs::create_dir(&outside)?;
        std::fs::write(outside.join("marker"), "untouched\n")?;
        let path = repo.root().join(if parent {
            "parent/node_modules"
        } else {
            "node_modules"
        });
        std::fs::create_dir_all(path.parent().unwrap())?;
        junction::create(&outside, &path)?;
        if parent {
            repo.write("parent/junk", "untracked\n")?;
        }
        let result = repo.git.restore_checkpoint(&checkpoint, false).await;
        assert!(matches!(result, Ok(true)), "{result:?}");
        if parent {
            assert!(!repo.root().join("parent/junk").exists());
        }
        assert!(junction::exists(&path)?);
        assert_eq!(
            std::fs::read_to_string(outside.join("marker"))?,
            "untouched\n"
        );
        junction::delete(path)?;
    }
    Ok(())
}

#[tokio::test]
async fn three_hundred_ignored_directories_do_not_expand_clean_argv() -> TestResult {
    let repo = Repository::new().await?;
    repo.write(".gitignore", "ignored-*/\n")?;
    let checkpoint = repo.commit("base").await?;
    for n in 0..300 {
        repo.write(
            &format!("ignored-{n:03}-{}/marker", "x".repeat(110)),
            "keep\n",
        )?;
    }
    assert!(repo.git.restore_checkpoint(&checkpoint, false).await?);
    assert_eq!(
        std::fs::read_to_string(
            repo.root()
                .join(format!("ignored-000-{}/marker", "x".repeat(110)))
        )?,
        "keep\n"
    );
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn a_link_swapped_after_the_first_write_check_is_refused() -> TestResult {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    let repo = Repository::new().await?;
    repo.write("dir/x", "checkpoint\n")?;
    let checkpoint = repo.commit("base").await?;
    repo.write("dir/x", "dirty\n")?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let git = repo
        .git
        .clone()
        .with_program(env!("CARGO_BIN_EXE_git-gate-fixture"))
        .with_environment(
            [
                (
                    "T3_GIT_GATE_ADDRESS".into(),
                    Some(listener.local_addr()?.to_string().into()),
                ),
                ("T3_GIT_GATE_COMMAND".into(), Some("diff".into())),
            ]
            .into(),
        );
    let task = tokio::spawn(async move { git.restore_checkpoint(&checkpoint, false).await });
    for _ in 0..2 {
        let (socket, _) = listener.accept().await?;
        let mut socket = BufReader::new(socket);
        let mut ready = [0; 5];
        socket.read_exact(&mut ready).await?;
        let mut args = String::new();
        socket.read_line(&mut args).await?;
        socket.get_mut().write_all(b"x").await?;
    }
    let mut sockets = Vec::new();
    for _ in 0..2 {
        let (socket, _) = listener.accept().await?;
        let mut socket = BufReader::new(socket);
        let mut ready = [0; 5];
        socket.read_exact(&mut ready).await?;
        let mut args = String::new();
        socket.read_line(&mut args).await?;
        sockets.push(socket);
    }
    let outside = repo.temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("x"), "outside\n")?;
    std::fs::remove_dir_all(repo.root().join("dir"))?;
    junction::create(&outside, repo.root().join("dir"))?;
    for socket in &mut sockets {
        socket.get_mut().write_all(b"x").await?;
    }
    let result = task.await?;
    assert!(
        matches!(result, Err(GitError::UnsafeRestore(_))),
        "{result:?}"
    );
    assert_eq!(std::fs::read_to_string(outside.join("x"))?, "outside\n");
    index_is_head(&repo).await?;
    junction::delete(repo.root().join("dir"))?;
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn forced_index_flags_and_above_root_links_obey_the_write_boundary() -> TestResult {
    for flag in ["--skip-worktree", "--assume-unchanged"] {
        let repo = Repository::new().await?;
        repo.write("dir/x", "checkpoint\n")?;
        let checkpoint = repo.commit("base").await?;
        repo.command(&["update-index", flag, "dir/x"]).await?;
        let outside = repo.temp.path().join("outside");
        std::fs::create_dir(&outside)?;
        std::fs::write(outside.join("x"), "outside\n")?;
        std::fs::remove_dir_all(repo.root().join("dir"))?;
        junction::create(&outside, repo.root().join("dir"))?;
        let result = repo.git.restore_checkpoint(&checkpoint, false).await;
        assert!(
            matches!(result, Err(GitError::UnsafeRestore(_))),
            "{flag}: {result:?}"
        );
        assert_eq!(std::fs::read_to_string(outside.join("x"))?, "outside\n");
        junction::delete(repo.root().join("dir"))?;
    }
    let repo = Repository::new().await?;
    repo.write("x", "checkpoint\n")?;
    let checkpoint = repo.commit("base").await?;
    let alias = repo.temp.path().join("root-alias");
    junction::create(repo.root(), &alias)?;
    repo.write("x", "dirty\n")?;
    let git = repo.git.at(&alias);
    git.capture_checkpoint(&checkpoint_ref("alias", 0)).await?;
    assert!(git.restore_checkpoint(&checkpoint, false).await?);
    assert_eq!(
        std::fs::read_to_string(repo.root().join("x"))?,
        "checkpoint\n"
    );
    junction::delete(alias)?;
    Ok(())
}

#[tokio::test]
async fn fifty_thousand_tracked_files_capture_and_restore_without_explicit_pathspecs() -> TestResult
{
    let repo = Repository::new().await?;
    repo.write(".gitignore", "ignored-file\n")?;
    repo.write("ignored-file", "ignored\n")?;
    for n in 0..50_000 {
        repo.write(&format!("files/{:03}/{n:05}.txt", n / 500), "base\n")?;
    }
    for command in [vec!["add", "."], vec!["commit", "-m", "base"]] {
        repo.git
            .execute(
                &command,
                ExecuteOptions {
                    timeout: None,
                    ..Default::default()
                },
            )
            .await?;
    }
    repo.write("files/000/00000.txt", "checkpoint\n")?;
    let reference = checkpoint_ref("large-visible", 0);
    repo.git.capture_checkpoint(&reference).await?;
    repo.write("files/000/00000.txt", "dirty\n")?;
    assert!(repo.git.restore_checkpoint(&reference, false).await?);
    assert_eq!(
        std::fs::read_to_string(repo.root().join("files/000/00000.txt"))?,
        "checkpoint\n"
    );
    index_is_head(&repo).await?;
    Ok(())
}

#[tokio::test]
async fn cancellation_after_restore_still_resets_the_index_before_returning() -> TestResult {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    let repo = Repository::new().await?;
    repo.write("safe", "checkpoint\n")?;
    let checkpoint = repo.commit("checkpoint").await?;
    repo.write("safe", "HEAD\n")?;
    repo.commit("HEAD").await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let cancellation = t3_git::CancellationToken::new();
    let git = repo
        .git
        .clone()
        .with_cancellation(cancellation.clone())
        .with_program(env!("CARGO_BIN_EXE_git-gate-fixture"))
        .with_environment(
            [
                (
                    "T3_GIT_GATE_ADDRESS".into(),
                    Some(listener.local_addr()?.to_string().into()),
                ),
                ("T3_GIT_GATE_COMMAND".into(), Some("clean".into())),
            ]
            .into(),
        );
    let task = tokio::spawn(async move { git.restore_checkpoint(&checkpoint, false).await });
    let (socket, _) = listener.accept().await?;
    let mut socket = BufReader::new(socket);
    let mut ready = [0; 5];
    socket.read_exact(&mut ready).await?;
    let mut args = String::new();
    socket.read_line(&mut args).await?;
    assert_eq!(
        std::fs::read_to_string(repo.root().join("safe"))?,
        "checkpoint\n"
    );
    cancellation.cancel();
    let result = task.await?;
    assert!(
        matches!(
            result,
            Err(GitError::Process(t3_process::ProcessError::Cancelled))
        ),
        "{result:?}"
    );
    index_is_head(&repo).await?;
    repo.git.drain_operations().await;
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn capture_matches_git_add_for_a_junction_without_changing_the_real_index() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("dir/x", "base\n")?;
    repo.commit("base").await?;
    let index = repo.index()?;
    let outside = repo.temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(
        outside.join("x"),
        "outside capture is inherited Git behavior\n",
    )?;
    std::fs::remove_dir_all(repo.root().join("dir"))?;
    junction::create(&outside, repo.root().join("dir"))?;
    let reference = checkpoint_ref("capture-add-parity", 0);
    repo.git.capture_checkpoint(&reference).await?;
    let env = [(
        "GIT_INDEX_FILE".into(),
        Some(repo.temp.path().join("parity-index").into_os_string()),
    )]
    .into();
    let options = ExecuteOptions {
        capture: true,
        env,
        ..Default::default()
    };
    repo.git
        .execute(&["read-tree", "HEAD"], options.clone())
        .await?;
    repo.git
        .execute(&["add", "-A", "--", "."], options.clone())
        .await?;
    let expected = repo
        .git
        .execute(&["write-tree"], options)
        .await?
        .stdout_text();
    assert_eq!(
        repo.command(&["rev-parse", &format!("{reference}^{{tree}}")])
            .await?,
        expected
    );
    assert_eq!(repo.index()?, index);
    assert_eq!(
        std::fs::read_to_string(outside.join("x"))?,
        "outside capture is inherited Git behavior\n"
    );
    junction::delete(repo.root().join("dir"))?;
    Ok(())
}

#[tokio::test]
async fn restore_removes_index_only_paths_and_clean_decodes_real_quoted_names() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("old/x", "checkpoint\n")?;
    let checkpoint = repo.commit("checkpoint").await?;
    repo.command(&["config", "diff.renames", "true"]).await?;
    repo.command(&["mv", "old", "new"]).await?;
    for name in ["é with spaces/x", "-leading", "[literal]"] {
        repo.write(name, "clean me\n")?;
    }
    #[cfg(unix)]
    repo.write("back\\slash\nname", "clean me\n")?;
    assert!(repo.git.restore_checkpoint(&checkpoint, false).await?);
    assert_eq!(
        std::fs::read_to_string(repo.root().join("old/x"))?,
        "checkpoint\n"
    );
    for name in ["new/x", "é with spaces", "-leading", "[literal]"] {
        assert!(!repo.root().join(name).exists(), "{name}");
    }
    #[cfg(unix)]
    assert!(!repo.root().join("back\\slash\nname").exists());
    index_is_head(&repo).await?;
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn restore_never_recursively_clears_a_destination_junction_or_its_parent() -> TestResult {
    for direct in [false, true] {
        let repo = Repository::new().await?;
        repo.write("leaf", "checkpoint\n")?;
        let checkpoint = repo.commit("base").await?;
        std::fs::remove_file(repo.root().join("leaf"))?;
        let outside = repo.temp.path().join("outside");
        std::fs::create_dir(&outside)?;
        std::fs::write(outside.join("marker"), "must survive\n")?;
        let linked = if direct {
            repo.root().join("leaf")
        } else {
            std::fs::create_dir(repo.root().join("leaf"))?;
            repo.root().join("leaf/junction")
        };
        junction::create(&outside, &linked)?;
        let result = repo.git.restore_checkpoint(&checkpoint, false).await;
        assert!(
            matches!(result, Err(GitError::UnsafeRestore(_))),
            "direct={direct}: {result:?}"
        );
        assert_eq!(
            std::fs::read_to_string(outside.join("marker"))?,
            "must survive\n"
        );
        junction::delete(linked)?;
        index_is_head(&repo).await?;
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn registered_submodules_stay_leaves_even_when_config_enables_recursion() -> TestResult {
    let source = Repository::new().await?;
    source.write("x", "one\n")?;
    let one = source.commit("one").await?;
    source.write("x", "two\n")?;
    source.commit("two").await?;
    let repo = Repository::new().await?;
    repo.command(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        &source.root().to_string_lossy(),
        "sub",
    ])
    .await?;
    let checkpoint = repo.commit("submodule two").await?;
    let sub = repo.git.at(repo.root().join("sub"));
    sub.execute(&["checkout", &one], ExecuteOptions::default())
        .await?;
    repo.command(&["add", "sub"]).await?;
    repo.command(&["config", "submodule.recurse", "true"])
        .await?;
    let outside = repo.temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("marker"), "untouched\n")?;
    junction::create(&outside, sub.cwd().join("node_modules"))?;
    assert!(repo.git.restore_checkpoint(&checkpoint, false).await?);
    assert_eq!(std::fs::read_to_string(sub.cwd().join("x"))?, "one\n");
    assert_eq!(
        sub.execute(&["rev-parse", "HEAD"], ExecuteOptions::default())
            .await?
            .stdout_text()
            .trim(),
        one
    );
    assert!(junction::exists(sub.cwd().join("node_modules"))?);
    assert_eq!(
        std::fs::read_to_string(outside.join("marker"))?,
        "untouched\n"
    );
    junction::delete(sub.cwd().join("node_modules"))?;
    index_is_head(&repo).await?;
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn sparse_restore_checks_changed_and_source_only_paths_outside_the_cone() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("a/x", "inside\n")?;
    repo.write("b/old", "base\n")?;
    repo.commit("base").await?;
    repo.command(&["sparse-checkout", "init", "--cone", "--sparse-index"])
        .await?;
    repo.command(&["sparse-checkout", "set", "a"]).await?;
    repo.write("b/new", "captured outside cone\n")?;
    repo.write("b/old", "changed outside cone\n")?;
    let checkpoint = checkpoint_ref("outside-cone", 0);
    repo.git.capture_checkpoint(&checkpoint).await?;
    assert_eq!(
        repo.show(&checkpoint, "b/new").await?,
        "captured outside cone\n"
    );
    std::fs::remove_dir_all(repo.root().join("b"))?;
    let outside = repo.temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    junction::create(&outside, repo.root().join("b"))?;
    let index = repo.index()?;
    let result = repo.git.restore_checkpoint(&checkpoint, false).await;
    junction::delete(repo.root().join("b"))?;
    assert!(
        matches!(result, Err(GitError::UnsafeRestore(_))),
        "{result:?}"
    );
    assert!(std::fs::read_dir(&outside)?.next().is_none());
    assert_eq!(repo.index()?, index);
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn unchanged_gitlinks_still_check_their_ancestors() -> TestResult {
    let repo = Repository::new().await?;
    repo.command(&["init", "x/sub"]).await?;
    repo.git
        .at(repo.root().join("x/sub"))
        .execute(
            &["commit", "--allow-empty", "-m", "sub"],
            Default::default(),
        )
        .await?;
    let checkpoint = repo.commit("gitlink").await?;
    std::fs::remove_dir_all(repo.root().join("x"))?;
    let outside = repo.temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("sub"), "victim file\n")?;
    junction::create(&outside, repo.root().join("x"))?;
    assert!(
        repo.command(&["diff", "--cached", "--name-only", &checkpoint])
            .await?
            .is_empty()
    );
    assert!(
        repo.command(&[
            "diff",
            "--name-only",
            "--ignore-submodules=all",
            &checkpoint
        ])
        .await?
        .is_empty()
    );
    let index = repo.index()?;
    let result = repo.git.restore_checkpoint(&checkpoint, false).await;
    junction::delete(repo.root().join("x"))?;
    assert!(
        matches!(result, Err(GitError::UnsafeRestore(_))),
        "{result:?}"
    );
    assert_eq!(
        std::fs::read_to_string(outside.join("sub"))?,
        "victim file\n"
    );
    assert_eq!(repo.index()?, index);
    Ok(())
}

#[tokio::test]
async fn restore_replaces_true_symlink_and_file_ancestors_without_following_them() -> TestResult {
    for symlinks in ["false", "true"] {
        for staged in [false, true] {
            let repo = Repository::new().await?;
            repo.write("link/deep/x", "checkpoint\n")?;
            repo.write("file/b", "checkpoint\n")?;
            let checkpoint = repo.commit("base").await?;
            repo.command(&["config", "core.symlinks", symlinks]).await?;
            std::fs::remove_dir_all(repo.root().join("link"))?;
            std::fs::remove_dir_all(repo.root().join("file"))?;
            let outside = repo.temp.path().join("outside");
            let victim = repo.temp.path().join("victim");
            std::fs::create_dir(&outside)?;
            std::fs::create_dir(&victim)?;
            std::fs::write(victim.join("x"), "outside\n")?;
            #[cfg(windows)]
            {
                junction::create(&victim, outside.join("deep"))?;
                std::os::windows::fs::symlink_dir(&outside, repo.root().join("link"))?;
            }
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&victim, outside.join("deep"))?;
                std::os::unix::fs::symlink(&outside, repo.root().join("link"))?;
            }
            repo.write("file", "regular file ancestor\n")?;
            if staged {
                repo.command(&["add", "-A"]).await?;
            }
            let result = repo.git.restore_checkpoint(&checkpoint, false).await;
            #[cfg(windows)]
            junction::delete(outside.join("deep"))?;
            #[cfg(unix)]
            std::fs::remove_file(outside.join("deep"))?;
            assert!(
                matches!(result, Ok(true)),
                "symlinks={symlinks}, staged={staged}: {result:?}"
            );
            assert_eq!(std::fs::read_to_string(victim.join("x"))?, "outside\n");
            assert!(
                !std::fs::symlink_metadata(repo.root().join("link"))?
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(
                std::fs::read_to_string(repo.root().join("link/deep/x"))?,
                "checkpoint\n"
            );
            assert_eq!(
                std::fs::read_to_string(repo.root().join("file/b"))?,
                "checkpoint\n"
            );
            index_is_head(&repo).await?;
        }
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn clean_unlinks_top_level_and_nested_junctions_without_following_targets() -> TestResult {
    for suffix in ["junk", "junk/link", "junk/a/link"] {
        let repo = Repository::new().await?;
        repo.write("safe", "checkpoint\n")?;
        let checkpoint = repo.commit("base").await?;
        repo.write("safe", "dirty\n")?;
        let outside = repo.temp.path().join("outside");
        std::fs::create_dir(&outside)?;
        std::fs::write(outside.join("marker"), "untouched\n")?;
        let path = repo.root().join(suffix);
        std::fs::create_dir_all(path.parent().unwrap())?;
        junction::create(&outside, &path)?;
        let result = repo.git.restore_checkpoint(&checkpoint, false).await;
        if junction::exists(&path)? {
            junction::delete(&path)?;
        }
        assert!(matches!(result, Ok(true)), "{suffix}: {result:?}");
        assert!(!repo.root().join("junk").exists());
        assert_eq!(
            std::fs::read_to_string(outside.join("marker"))?,
            "untouched\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.root().join("safe"))?,
            "checkpoint\n"
        );
        index_is_head(&repo).await?;
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn unknown_or_old_git_refuses_clean_before_restoring_or_resetting_staging() -> TestResult {
    for (version, native_guard) in [
        ("git version 2.22.0.windows.1", false),
        ("git version 2.23.0.windows.1", true),
        ("git version 2.52.0", false),
        ("unknown", false),
    ] {
        let repo = Repository::new().await?;
        repo.write("safe", "checkpoint\n")?;
        let checkpoint = repo.commit("base").await?;
        repo.write("safe", "staged\n")?;
        repo.command(&["add", "safe"]).await?;
        repo.write("safe", "working\n")?;
        let outside = repo.temp.path().join("outside");
        std::fs::create_dir(&outside)?;
        std::fs::write(outside.join("marker"), "untouched\n")?;
        std::fs::create_dir(repo.root().join("junk"))?;
        junction::create(&outside, repo.root().join("junk/link"))?;
        let index = repo.index()?;
        let git = repo
            .git
            .clone()
            .with_program(env!("CARGO_BIN_EXE_git-gate-fixture"))
            .with_environment([("T3_GIT_VERSION_OVERRIDE".into(), Some(version.into()))].into());
        let result = git.restore_checkpoint(&checkpoint, false).await;
        if junction::exists(repo.root().join("junk/link"))? {
            junction::delete(repo.root().join("junk/link"))?;
        }
        if native_guard {
            assert!(matches!(result, Ok(true)), "{version}: {result:?}");
            assert!(!repo.root().join("junk").exists());
            assert_eq!(
                std::fs::read_to_string(repo.root().join("safe"))?,
                "checkpoint\n"
            );
            index_is_head(&repo).await?;
        } else {
            assert!(
                matches!(result, Err(GitError::UnsafeRestore(_))),
                "{result:?}"
            );
            assert_eq!(
                std::fs::read_to_string(repo.root().join("safe"))?,
                "working\n",
                "{version}"
            );
            assert_eq!(repo.index()?, index);
            assert!(result.unwrap_err().to_string().contains("junk"));
        }
        assert_eq!(
            std::fs::read_to_string(outside.join("marker"))?,
            "untouched\n"
        );
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn a_later_clean_refusal_names_the_path_and_reports_restored_tracked_files() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("safe", "checkpoint\n")?;
    repo.write("d/k", "equal\n")?;
    repo.write(".gitignore", "")?;
    let checkpoint = repo.commit("checkpoint").await?;
    repo.write("safe", "HEAD\n")?;
    repo.write(".gitignore", "/d/extra\n")?;
    repo.commit("HEAD").await?;
    std::fs::remove_dir_all(repo.root().join("d"))?;
    let outside = repo.temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("k"), "equal\n")?;
    std::fs::write(outside.join("extra"), "untouched\n")?;
    junction::create(&outside, repo.root().join("d"))?;
    let result = repo.git.restore_checkpoint(&checkpoint, false).await;
    junction::delete(repo.root().join("d"))?;
    let error = result
        .expect_err("new ignore rules expose unsafe clean ancestor")
        .to_string();
    assert!(error.contains("Tracked files were restored"), "{error}");
    assert!(error.contains("d"), "{error}");
    assert_eq!(
        std::fs::read_to_string(repo.root().join("safe"))?,
        "checkpoint\n"
    );
    assert_eq!(
        std::fs::read_to_string(outside.join("extra"))?,
        "untouched\n"
    );
    index_is_head(&repo).await?;
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn clean_ancestor_preflight_refuses_before_restore_even_when_both_diffs_are_empty()
-> TestResult {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let repo = Repository::new().await?;
    repo.write("d/k", "equal\n")?;
    let checkpoint = repo.commit("base").await?;
    std::fs::remove_dir_all(repo.root().join("d"))?;
    let outside = repo.temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("k"), "equal\n")?;
    std::fs::write(outside.join("extra"), "untouched\n")?;
    junction::create(&outside, repo.root().join("d"))?;
    assert!(
        repo.command(&["diff", "--cached", "--name-only", &checkpoint])
            .await?
            .is_empty()
    );
    assert!(
        repo.command(&["diff", "--name-only", &checkpoint])
            .await?
            .is_empty()
    );
    let index = repo.index()?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let git = repo
        .git
        .clone()
        .with_program(env!("CARGO_BIN_EXE_git-gate-fixture"))
        .with_environment(
            [
                ("T3_GIT_GATE_COMMAND".into(), Some("restore".into())),
                (
                    "T3_GIT_GATE_ADDRESS".into(),
                    Some(listener.local_addr()?.to_string().into()),
                ),
            ]
            .into(),
        );
    let mut task = tokio::spawn(async move { git.restore_checkpoint(&checkpoint, false).await });
    let (result, started_restore) = tokio::select! {
        result = &mut task => (result?, false),
        accepted = listener.accept() => {
            let (mut socket, _) = accepted?;
            let mut ready = [0;5]; socket.read_exact(&mut ready).await?;
            socket.write_all(b"x").await?;
            (task.await?, true)
        }
    };
    junction::delete(repo.root().join("d"))?;
    assert!(
        matches!(result, Err(GitError::UnsafeRestore(_))),
        "{result:?}"
    );
    assert!(
        !started_restore,
        "clean ancestor refusal came after restore started"
    );
    assert_eq!(repo.index()?, index);
    assert_eq!(
        std::fs::read_to_string(outside.join("extra"))?,
        "untouched\n"
    );
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn unmerged_index_stages_are_checked_and_restore_resolves_them() -> TestResult {
    for linked in [false, true] {
        let repo = Repository::new().await?;
        repo.write("dir/k", "base\n")?;
        let checkpoint = repo.commit("base").await?;
        let blob = repo.command(&["rev-parse", "HEAD:dir/k"]).await?;
        repo.command(&["update-index", "--force-remove", "dir/k"])
            .await?;
        let stages: String = (1..=3)
            .map(|stage| format!("100644 {} {stage}\tdir/k\n", blob.trim()))
            .collect();
        repo.git
            .execute(
                &["update-index", "--index-info"],
                ExecuteOptions {
                    stdin: Some(stages.into_bytes()),
                    ..Default::default()
                },
            )
            .await?;
        repo.write("dir/k", "conflict\n")?;
        let outside = repo.temp.path().join("outside");
        std::fs::create_dir(&outside)?;
        std::fs::write(outside.join("k"), "outside\n")?;
        if linked {
            std::fs::remove_dir_all(repo.root().join("dir"))?;
            junction::create(&outside, repo.root().join("dir"))?;
        }
        let index = repo.index()?;
        let result = repo.git.restore_checkpoint(&checkpoint, false).await;
        if linked {
            junction::delete(repo.root().join("dir"))?;
            assert!(
                matches!(result, Err(GitError::UnsafeRestore(_))),
                "{result:?}"
            );
            assert_eq!(repo.index()?, index);
        } else {
            assert!(matches!(result, Ok(true)), "{result:?}");
            assert!(repo.command(&["ls-files", "--unmerged"]).await?.is_empty());
            assert_eq!(
                std::fs::read_to_string(repo.root().join("dir/k"))?,
                "base\n"
            );
            index_is_head(&repo).await?;
        }
        assert_eq!(std::fs::read_to_string(outside.join("k"))?, "outside\n");
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn intent_to_add_entries_are_checked_before_restore_removes_them() -> TestResult {
    for linked in [false, true] {
        let repo = Repository::new().await?;
        let checkpoint = repo.commit("empty").await?;
        repo.write("new/path", "intent to add\n")?;
        repo.command(&["add", "--intent-to-add", "new/path"])
            .await?;
        let outside = repo.temp.path().join("outside");
        std::fs::create_dir(&outside)?;
        std::fs::write(outside.join("path"), "outside\n")?;
        if linked {
            std::fs::remove_dir_all(repo.root().join("new"))?;
            junction::create(&outside, repo.root().join("new"))?;
        }
        let index = repo.index()?;
        let result = repo.git.restore_checkpoint(&checkpoint, false).await;
        if linked {
            junction::delete(repo.root().join("new"))?;
            assert!(
                matches!(result, Err(GitError::UnsafeRestore(_))),
                "{result:?}"
            );
            assert_eq!(repo.index()?, index);
        } else {
            assert!(matches!(result, Ok(true)), "{result:?}");
            assert!(!repo.root().join("new/path").exists());
            index_is_head(&repo).await?;
        }
        assert_eq!(std::fs::read_to_string(outside.join("path"))?, "outside\n");
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn case_renamed_index_paths_are_checked_against_the_checkpoint() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("Dir/k", "checkpoint\n")?;
    let checkpoint = repo.commit("base").await?;
    repo.command(&["config", "core.ignorecase", "false"])
        .await?;
    repo.command(&["mv", "Dir", "renaming"]).await?;
    repo.command(&["mv", "renaming", "dir"]).await?;
    let changed = repo
        .command(&[
            "diff",
            "--cached",
            "--no-renames",
            "--name-only",
            &checkpoint,
        ])
        .await?;
    assert!(changed.lines().any(|path| path == "Dir/k"));
    assert!(changed.lines().any(|path| path == "dir/k"));
    std::fs::remove_dir_all(repo.root().join("dir"))?;
    let outside = repo.temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("k"), "outside\n")?;
    junction::create(&outside, repo.root().join("dir"))?;
    let index = repo.index()?;
    let result = repo.git.restore_checkpoint(&checkpoint, false).await;
    junction::delete(repo.root().join("dir"))?;
    assert!(
        matches!(result, Err(GitError::UnsafeRestore(_))),
        "{result:?}"
    );
    assert_eq!(repo.index()?, index);
    assert_eq!(std::fs::read_to_string(outside.join("k"))?, "outside\n");
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn a_checkpoint_file_over_an_index_gitlink_checks_the_checkout_contents() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("leaf", "checkpoint file\n")?;
    let checkpoint = repo.commit("file").await?;
    std::fs::remove_file(repo.root().join("leaf"))?;
    repo.command(&["init", "leaf"]).await?;
    repo.git
        .at(repo.root().join("leaf"))
        .execute(
            &["commit", "--allow-empty", "-m", "sub"],
            Default::default(),
        )
        .await?;
    repo.commit("gitlink").await?;
    let outside = repo.temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("marker"), "untouched\n")?;
    junction::create(&outside, repo.root().join("leaf/link"))?;
    let index = repo.index()?;
    let result = repo.git.restore_checkpoint(&checkpoint, false).await;
    junction::delete(repo.root().join("leaf/link"))?;
    assert!(
        matches!(result, Err(GitError::UnsafeRestore(_))),
        "{result:?}"
    );
    assert_eq!(repo.index()?, index);
    assert_eq!(
        std::fs::read_to_string(outside.join("marker"))?,
        "untouched\n"
    );
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn content_equal_eol_paths_are_not_written_through_junctions_or_resmudged() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("d/k", "equal\n")?;
    repo.write("inside", "equal\n")?;
    let checkpoint = repo.commit("LF base").await?;
    repo.command(&["config", "core.autocrlf", "true"]).await?;
    std::fs::remove_dir_all(repo.root().join("d"))?;
    let outside = repo.temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("k"), "equal\n")?;
    let stamp = filetime::FileTime::from_unix_time(1_234_567_890, 0);
    filetime::set_file_mtime(outside.join("k"), stamp)?;
    filetime::set_file_mtime(repo.root().join("inside"), stamp)?;
    let before_outside = std::fs::metadata(outside.join("k"))?.modified()?;
    let before_inside = std::fs::metadata(repo.root().join("inside"))?.modified()?;
    junction::create(&outside, repo.root().join("d"))?;
    assert!(
        repo.command(&["diff", "--cached", "--name-only", &checkpoint])
            .await?
            .is_empty()
    );
    assert!(
        repo.command(&["diff", "--name-only", &checkpoint])
            .await?
            .is_empty()
    );
    let result = repo.git.restore_checkpoint(&checkpoint, false).await;
    junction::delete(repo.root().join("d"))?;
    assert!(matches!(result, Ok(true)), "{result:?}");
    assert_eq!(std::fs::read(outside.join("k"))?, b"equal\n");
    assert_eq!(std::fs::read(repo.root().join("inside"))?, b"equal\n");
    assert_eq!(
        std::fs::metadata(outside.join("k"))?.modified()?,
        before_outside
    );
    assert_eq!(
        std::fs::metadata(repo.root().join("inside"))?.modified()?,
        before_inside
    );
    index_is_head(&repo).await?;
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
#[allow(unsafe_code)]
async fn clean_refuses_a_real_non_symlink_name_surrogate() -> TestResult {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::System::{IO::DeviceIoControl, Ioctl::FSCTL_SET_REPARSE_POINT};
    let repo = Repository::new().await?;
    repo.write("safe", "base\n")?;
    let checkpoint = repo.commit("base").await?;
    let point = repo.root().join("opaque");
    std::fs::write(&point, "")?;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .share_mode(7)
        .custom_flags(0x00200000)
        .open(&point)?;
    let mut data = [0u8; 24];
    data[..4].copy_from_slice(&0x20000042u32.to_le_bytes());
    data[8..].fill(1);
    let mut returned = 0;
    // SAFETY: File owns the live handle; the input GUID reparse buffer and output count remain valid.
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_REPARSE_POINT,
            data.as_ptr().cast(),
            data.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    drop(file);
    let index = repo.index()?;
    let result = repo.git.restore_checkpoint(&checkpoint, false).await;
    assert!(
        matches!(result, Err(GitError::UnsafeRestore(_))),
        "{result:?}"
    );
    assert!(std::fs::symlink_metadata(&point).is_ok());
    assert_eq!(repo.index()?, index);
    Ok(())
}
