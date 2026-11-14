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
async fn unhidden_junction_refuses_clean_and_failure_always_resets_index() -> TestResult {
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
    assert!(
        matches!(result, Err(GitError::UnsafeRestore(_))),
        "{result:?}"
    );
    assert_eq!(
        std::fs::read_to_string(repo.root().join("safe"))?,
        "checkpoint\n"
    );
    index_is_head(&repo).await?;
    assert_eq!(
        std::fs::read_to_string(outside.join("marker"))?,
        "untouched\n"
    );
    junction::delete(repo.root().join("build/link"))?;
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
        if parent {
            assert!(
                matches!(result, Ok(true) | Err(GitError::UnsafeRestore(_))),
                "{result:?}"
            );
        } else {
            assert!(matches!(result, Ok(true)), "{result:?}");
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
