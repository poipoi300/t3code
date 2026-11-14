mod common;
use common::{Repository, TestResult};
use t3_git::{ExecuteOptions, checkpoint_ref};

async fn planted_repository_is_refused(operation: &str) -> TestResult {
    let mut failures = Vec::new();
    for git_file in [false, true] {
        let repo = Repository::new().await?;
        repo.write("pkg/f", "real checkpoint\n")?;
        repo.commit("real").await?;
        let scope = repo.git.at(repo.root().join("pkg"));
        let reference = checkpoint_ref("bound-scope", 0);
        scope.capture_checkpoint(&reference).await?;
        scope.status().await?;
        let mut init = vec!["init", "--initial-branch=planted"];
        let outside = repo.temp.path().join("planted.git");
        let outside_text = outside.to_string_lossy();
        if git_file {
            init.extend(["--separate-git-dir", &outside_text]);
        }
        scope.execute(&init, ExecuteOptions::default()).await?;
        let user_edit = "user's real uncommitted edit\n";
        std::fs::write(scope.cwd().join("f"), "x".repeat(user_edit.len()))?;
        std::fs::write(scope.cwd().join(".gitattributes"), "f filter=plant\n")?;
        scope
            .execute(&["add", "f", ".gitattributes"], ExecuteOptions::default())
            .await?;
        scope
            .execute(&["commit", "-m", "planted"], ExecuteOptions::default())
            .await?;
        scope
            .execute(
                &["update-ref", &reference, "HEAD"],
                ExecuteOptions::default(),
            )
            .await?;
        let marker = repo.temp.path().join("helper-ran");
        let script = repo.temp.path().join("helper.sh");
        let marker_path = marker.to_string_lossy().replace('\\', "/");
        std::fs::write(
            &script,
            format!(
                "printf '%s\\n' \"$1\" >> '{marker_path}'\nif [ \"$1\" = clean ] || [ \"$1\" = smudge ]; then cat; fi\n"
            ),
        )?;
        let script_path = script.to_string_lossy().replace('\\', "/");
        for (key, command) in [
            ("core.fsmonitor", format!("sh '{script_path}' fsmonitor")),
            ("filter.plant.clean", format!("sh '{script_path}' clean")),
            ("filter.plant.smudge", format!("sh '{script_path}' smudge")),
        ] {
            scope
                .execute(&["config", key, &command], ExecuteOptions::default())
                .await?;
        }
        std::fs::write(scope.cwd().join("f"), user_edit)?;
        let timestamp = filetime::FileTime::from_unix_time(2_000_000_000, 0);
        filetime::set_file_times(scope.cwd().join("f"), timestamp, timestamp)?;
        std::fs::write(scope.cwd().join("notes.txt"), "user notes\n")?;
        let result = match operation {
            "capture" => {
                scope
                    .capture_checkpoint(&checkpoint_ref("bound-scope", 1))
                    .await
            }
            "restore" => scope
                .restore_checkpoint(&reference, false)
                .await
                .map(|_| ()),
            "status" => scope.status().await.map(|_| ()),
            _ => unreachable!(),
        };
        let unchanged = std::fs::read_to_string(scope.cwd().join("f"))
            .ok()
            .as_deref()
            == Some("user's real uncommitted edit\n")
            && std::fs::read_to_string(scope.cwd().join("notes.txt"))
                .ok()
                .as_deref()
                == Some("user notes\n");
        if !result
            .as_ref()
            .is_err_and(|error| error.to_string().contains("repository identity changed"))
            || marker.exists()
            || !unchanged
        {
            failures.push(format!(
                "git_file={git_file}: {result:?}, marker={}, unchanged={unchanged}",
                marker.exists()
            ));
        }
        if operation == "status" {
            scope
                .execute(
                    &["-c", "core.fsmonitor=false", "status", "--porcelain"],
                    ExecuteOptions::default(),
                )
                .await?;
            assert!(
                marker.exists(),
                "unbound status must exercise the planted filter"
            );
        }
    }
    assert!(failures.is_empty(), "{operation}: {failures:#?}");
    Ok(())
}

#[tokio::test]
async fn capture_refuses_a_valid_planted_repository_and_git_file() -> TestResult {
    planted_repository_is_refused("capture").await
}

#[tokio::test]
async fn restore_refuses_a_valid_planted_repository_and_git_file() -> TestResult {
    planted_repository_is_refused("restore").await
}

#[tokio::test]
async fn status_refuses_a_valid_planted_repository_and_git_file() -> TestResult {
    planted_repository_is_refused("status").await
}

#[tokio::test]
async fn checkpoint_commands_disable_the_real_repository_fsmonitor() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("pkg/f", "checkpoint\n")?;
    repo.commit("real").await?;
    let trace = repo.temp.path().join("git-trace.jsonl");
    let trace_env = std::collections::BTreeMap::from([(
        "GIT_TRACE2_EVENT".into(),
        Some(trace.as_os_str().to_owned()),
    )]);
    let scope = repo
        .git
        .at(repo.root().join("pkg"))
        .with_environment(trace_env);
    let marker = repo.temp.path().join("fsmonitor-ran");
    let marker_path = marker.to_string_lossy().replace('\\', "/");
    let command = format!("sh -c 'echo ran >> \"{marker_path}\"'");
    repo.command(&["config", "core.fsmonitor", &command])
        .await?;
    let common_marker = repo.temp.path().join("filter-common-dir");
    let common_marker_path = common_marker.to_string_lossy().replace('\\', "/");
    let filter =
        format!("sh -c 'printf \"%s\" \"$GIT_COMMON_DIR\" > \"{common_marker_path}\"; cat'");
    repo.command(&["config", "filter.bound.clean", &filter])
        .await?;
    repo.write("pkg/.gitattributes", "f filter=bound\n")?;
    repo.write("pkg/f", "checkpoint with filter\n")?;
    let reference = checkpoint_ref("real-config", 0);
    scope.capture_checkpoint(&reference).await?;
    assert_eq!(
        std::fs::canonicalize(std::fs::read_to_string(common_marker)?)?,
        std::fs::canonicalize(repo.root().join(".git"))?,
    );
    repo.write("pkg/f", "dirty\n")?;
    repo.write("pkg/junk", "junk\n")?;
    assert!(scope.restore_checkpoint(&reference, false).await?);
    scope.status().await?;
    assert!(!marker.exists(), "checkpoint/status invoked core.fsmonitor");
    let mut pinned = 0;
    for line in std::fs::read_to_string(trace)?.lines() {
        let event: serde_json::Value = serde_json::from_str(line)?;
        if event["event"] != "start" {
            continue;
        }
        let args: Vec<_> = event["argv"]
            .as_array()
            .expect("argv")
            .iter()
            .filter_map(|arg| arg.as_str())
            .collect();
        assert!(args.contains(&"core.fsmonitor=false"), "{args:?}");
        if !args.contains(&"--absolute-git-dir") {
            assert!(
                args.contains(&"--git-dir") && args.contains(&"--work-tree"),
                "{args:?}"
            );
            pinned += 1;
        }
    }
    assert!(pinned > 0);
    repo.command(&["status", "--porcelain"]).await?;
    assert!(
        marker.exists(),
        "the fsmonitor fixture must run without the override"
    );
    Ok(())
}

#[tokio::test]
async fn legitimate_submodules_and_nested_worktrees_keep_their_repository_identity() -> TestResult {
    for worktree in [false, true] {
        let repo = Repository::new().await?;
        repo.write("pkg/f", "real checkpoint\n")?;
        let head = repo.commit("real").await?;
        let scope = repo.git.at(repo.root().join("pkg"));
        let reference = checkpoint_ref("legitimate", 0);
        scope.capture_checkpoint(&reference).await?;
        if worktree {
            repo.command(&["worktree", "add", "-b", "nested", "pkg/nested", "HEAD"])
                .await?;
        } else {
            let source = Repository::new().await?;
            source.write("f", "submodule\n")?;
            source.commit("source").await?;
            repo.command(&[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                &source.root().to_string_lossy(),
                "pkg/nested",
            ])
            .await?;
            repo.command(&["commit", "-m", "submodule"]).await?;
        }
        let nested = repo.git.at(scope.cwd().join("nested"));
        let nested_reference = checkpoint_ref("nested", 0);
        nested.capture_checkpoint(&nested_reference).await?;
        std::fs::write(nested.cwd().join("junk"), "nested junk\n")?;
        repo.write("pkg/f", "dirty\n")?;
        scope
            .capture_checkpoint(&checkpoint_ref("legitimate", 1))
            .await?;
        assert!(scope.restore_checkpoint(&reference, false).await?);
        assert_eq!(
            std::fs::read_to_string(nested.cwd().join("junk"))?,
            "nested junk\n"
        );
        assert!(scope.status().await?.is_repo);
        assert!(nested.status().await?.is_repo);
        assert!(nested.restore_checkpoint(&nested_reference, false).await?);
        assert!(!nested.cwd().join("junk").exists());
        if worktree {
            assert_eq!(repo.command(&["rev-parse", "HEAD"]).await?.trim(), head);
        }
    }
    Ok(())
}

#[tokio::test]
async fn persisted_identity_refuses_a_repository_planted_before_first_scope_read() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("pkg/f", "real\n")?;
    repo.commit("real").await?;
    let identity = repo.git.bind_repository().await?.expect("bound repository");
    let saved = serde_json::to_vec(&identity)?;
    let scope = repo.git.at(repo.root().join("pkg"));
    scope.execute(&["init"], ExecuteOptions::default()).await?;
    let result = scope
        .with_repository_binding(serde_json::from_slice(&saved)?)
        .await;
    assert!(matches!(
        result,
        Err(t3_git::GitError::RepositoryChanged { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn linked_worktree_binding_refuses_gitdir_and_commondir_redirection() -> TestResult {
    for common in [false, true] {
        let repo = Repository::new().await?;
        repo.write("f", "real\n")?;
        repo.commit("real").await?;
        let checkout = repo.temp.path().join("checkout");
        repo.command(&[
            "worktree",
            "add",
            "-b",
            "linked",
            &checkout.to_string_lossy(),
        ])
        .await?;
        let scope = repo.git.at(&checkout);
        let identity = scope.bind_repository().await?.expect("worktree identity");
        let other = Repository::new().await?;
        let path = if common {
            identity.git_dir.join("commondir")
        } else {
            checkout.join(".git")
        };
        std::fs::write(
            &path,
            format!(
                "{}{}\n",
                if common { "" } else { "gitdir: " },
                other.root().join(".git").display()
            ),
        )?;
        let error = scope
            .status()
            .await
            .expect_err("changed repository must be refused");
        assert!(
            matches!(error, t3_git::GitError::RepositoryChanged { .. }),
            "{error:?}"
        );
        let t3_git::GitError::RepositoryChanged { path: changed, .. } = error else {
            unreachable!()
        };
        assert_eq!(
            std::fs::canonicalize(changed)?,
            std::fs::canonicalize(path)?
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_recreated_git_handle_cannot_forget_the_scope_identity() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("pkg/f", "real\n")?;
    repo.commit("real").await?;
    let scope = repo.git.at(repo.root().join("pkg"));
    scope.bind_repository().await?.expect("repository");
    scope.execute(&["init"], ExecuteOptions::default()).await?;
    let environment = std::collections::BTreeMap::from([
        (
            "GIT_CONFIG_GLOBAL".into(),
            Some(repo.temp.path().join("no-global").into_os_string()),
        ),
        (
            "GIT_CONFIG_SYSTEM".into(),
            Some(repo.temp.path().join("no-system").into_os_string()),
        ),
        ("GIT_CONFIG_NOSYSTEM".into(), Some("1".into())),
        ("GIT_CONFIG_COUNT".into(), Some("0".into())),
    ]);
    let fresh = t3_git::Git::new(scope.cwd()).with_environment(environment);
    let result = fresh.status().await;
    assert!(matches!(
        result,
        Err(t3_git::GitError::RepositoryChanged { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn persisted_handles_revalidate_discovery_before_each_operation() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("pkg/f", "real\n")?;
    repo.commit("real").await?;
    let identity = repo.git.bind_repository().await?.expect("repository");
    let scope = repo
        .git
        .at(repo.root().join("pkg"))
        .with_repository_binding(identity)
        .await?;
    let other = repo.temp.path().join("other-worktree");
    std::fs::create_dir(&other)?;
    repo.command(&["config", "core.worktree", &other.to_string_lossy()])
        .await?;
    assert!(matches!(
        scope.status().await,
        Err(t3_git::GitError::RepositoryChanged { .. })
    ));
    assert!(matches!(
        scope.resolve_checkpoint("HEAD").await,
        Err(t3_git::GitError::RepositoryChanged { .. })
    ));
    assert!(matches!(
        scope
            .capture_checkpoint(&checkpoint_ref("changed-top-level", 0))
            .await,
        Err(t3_git::GitError::RepositoryChanged { .. })
    ));
    assert!(matches!(
        scope.restore_checkpoint("HEAD", false).await,
        Err(t3_git::GitError::RepositoryChanged { .. })
    ));
    Ok(())
}
