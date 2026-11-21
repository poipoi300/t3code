mod common;
use common::{Repository, TestResult};
use t3_git::{ExecuteOptions, checkpoint_ref};

#[cfg(unix)]
#[tokio::test]
async fn a_repository_binding_preserves_newlines_in_checkout_paths() -> TestResult {
    let repo = Repository::new().await?;
    let relocated = repo.temp.path().join("checkout\nwith-newline");
    std::fs::rename(repo.root(), &relocated)?;
    let binding = repo
        .git
        .at(&relocated)
        .bind_repository()
        .await?
        .expect("binding");
    assert_eq!(binding.top_level, relocated.canonicalize()?);
    assert_eq!(binding.git_dir, relocated.join(".git").canonicalize()?);
    Ok(())
}

#[tokio::test]
async fn refusing_a_cloned_destination_keeps_the_previous_repository_binding() -> TestResult {
    let repo = Repository::new().await?;
    let previous = repo.git.bind_repository().await?.expect("binding");
    let git_directory = repo.root().join(".git");
    let saved = repo.temp.path().join("original.git");
    std::fs::rename(&git_directory, &saved)?;
    repo.git
        .execute(
            &["init", "--initial-branch=other"],
            ExecuteOptions::default(),
        )
        .await?;
    let mut wrong_destination = t3_git::FileId::directory(repo.root())?;
    wrong_destination.inode = wrong_destination.inode.wrapping_add(1);
    let replacement = t3_git::FileId::directory(&git_directory)?;
    assert!(matches!(
        repo.git
            .bind_cloned_repository(wrong_destination, replacement)
            .await,
        Err(t3_git::GitError::RepositoryChanged { .. })
    ));
    assert!(matches!(
        repo.git.bind_repository().await,
        Err(t3_git::GitError::RepositoryChanged { .. })
    ));
    std::fs::rename(&git_directory, repo.temp.path().join("replacement.git"))?;
    std::fs::rename(saved, git_directory)?;
    assert_eq!(repo.git.bind_repository().await?, Some(previous));
    Ok(())
}

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

/// A clean-filter helper that records each run in `marker` and passes content through.
fn marker_filter(repo: &Repository, name: &str) -> std::io::Result<(std::path::PathBuf, String)> {
    let marker = repo.temp.path().join(format!("{name}-ran"));
    let script = repo.temp.path().join(format!("{name}.sh"));
    let marker_path = marker.to_string_lossy().replace('\\', "/");
    std::fs::write(
        &script,
        format!("printf '%s\\n' \"$1\" >> '{marker_path}'\ncat\n"),
    )?;
    let command = format!("sh '{}' clean", script.to_string_lossy().replace('\\', "/"));
    Ok((marker, command))
}

/// Re-review 4 F4: the real `.git` moved aside and a repository created at the same path, with a
/// clean filter on `f` that records each run. Returns the filter's marker.
async fn replace_git_directory_at_the_same_path(
    repo: &Repository,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    std::fs::rename(repo.root().join(".git"), repo.temp.path().join("real.git"))?;
    repo.command(&["init", "--initial-branch=planted"]).await?;
    let (marker, command) = marker_filter(repo, "swapped")?;
    repo.command(&["config", "filter.plant.clean", &command])
        .await?;
    repo.write(".gitattributes", "f filter=plant\n")?;
    repo.write("f", "changed after the swap\n")?;
    Ok(marker)
}

fn refused_as_changed<T>(result: &Result<T, t3_git::GitError>) -> bool {
    matches!(result, Err(t3_git::GitError::RepositoryChanged { .. }))
}

#[tokio::test]
async fn a_git_directory_replaced_at_the_same_path_is_refused_in_process() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("f", "real\n")?;
    repo.commit("real").await?;
    let identity = repo.git.bind_repository().await?.expect("repository");
    let marker = replace_git_directory_at_the_same_path(&repo).await?;
    let capture = repo
        .git
        .capture_checkpoint(&checkpoint_ref("swapped", 0))
        .await;
    assert!(refused_as_changed(&capture), "{capture:?}");
    let rebound = repo.git.clone().with_repository_binding(identity).await;
    assert!(refused_as_changed(&rebound), "{:?}", rebound.map(|_| ()));
    assert!(!marker.exists(), "the replaced repository's filter ran");
    Ok(())
}

const CHILD_SCOPE: &str = "T3_GIT_TEST_REPLACED_SCOPE";
const CHILD_BINDING: &str = "T3_GIT_TEST_REPLACED_BINDING";

/// A server restart: a fresh process (empty binding cache) with the saved binding.
#[tokio::test]
async fn a_git_directory_replaced_at_the_same_path_is_refused_after_a_restart() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("f", "real\n")?;
    repo.commit("real").await?;
    let saved = serde_json::to_string(&repo.git.bind_repository().await?.expect("repository"))?;
    let marker = replace_git_directory_at_the_same_path(&repo).await?;
    let home = repo.temp.path().join("home");
    let output = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "fresh_process_capture_with_a_saved_binding",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_SCOPE, repo.root())
        .env(CHILD_BINDING, &saved)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("GIT_CONFIG_GLOBAL", home.join("no-global-config"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("child: repository changed"), "{stdout}");
    assert!(!marker.exists(), "the replaced repository's filter ran");
    Ok(())
}

#[tokio::test]
#[ignore = "run as the child process of a_git_directory_replaced_at_the_same_path_is_refused_after_a_restart"]
async fn fresh_process_capture_with_a_saved_binding() -> TestResult {
    let (Ok(scope), Ok(saved)) = (std::env::var(CHILD_SCOPE), std::env::var(CHILD_BINDING)) else {
        return Ok(());
    };
    let identity: t3_git::RepositoryBinding = serde_json::from_str(&saved)?;
    match t3_git::Git::new(scope)
        .with_repository_binding(identity)
        .await
    {
        Err(t3_git::GitError::RepositoryChanged { .. }) => println!("child: repository changed"),
        Err(error) => println!("child: other error {error}"),
        Ok(git) => {
            let capture = git
                .capture_checkpoint(&checkpoint_ref("restarted", 0))
                .await;
            println!("child: bound, capture {capture:?}");
        }
    }
    Ok(())
}

/// A binding saved before identities carried file ids cannot prove the repository is the same
/// one: it is refused with the typed error, never silently rebound.
#[tokio::test]
async fn a_binding_saved_in_the_earlier_format_is_refused_not_rebound() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("f", "real\n")?;
    repo.commit("real").await?;
    let identity = repo.git.bind_repository().await?.expect("repository");
    let earlier = serde_json::json!({
        "top_level": identity.top_level,
        "git_dir": identity.git_dir,
        "common_dir": identity.common_dir,
    });
    let result = repo
        .git
        .clone()
        .with_repository_binding(serde_json::from_value(earlier)?)
        .await;
    let Err(t3_git::GitError::RepositoryChanged { detail, .. }) = result else {
        panic!("expected a refusal, got {:?}", result.map(|_| ()));
    };
    assert!(detail.contains("earlier version"), "{detail}");
    Ok(())
}

/// Re-review 4 F5: a repository planted at a submodule path is not run by the automatic status
/// reads, which still report the submodule's changed commit.
#[tokio::test]
async fn status_reads_do_not_run_a_repository_planted_at_a_submodule_path() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("f", "real\n")?;
    repo.commit("real").await?;
    let source = Repository::new().await?;
    source.write("f", "submodule\n")?;
    source.commit("source").await?;
    repo.command(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        &source.root().to_string_lossy(),
        "sub",
    ])
    .await?;
    repo.command(&["commit", "-m", "submodule"]).await?;
    // The submodule's `.git` pointer replaced by a repository with a recording clean filter.
    let sub = repo.root().join("sub");
    std::fs::remove_file(sub.join(".git"))?;
    let planted = repo.git.at(&sub);
    planted
        .execute(
            &["init", "--initial-branch=planted"],
            ExecuteOptions::default(),
        )
        .await?;
    let (marker, command) = marker_filter(&repo, "submodule")?;
    planted
        .execute(
            &["config", "filter.plant.clean", &command],
            ExecuteOptions::default(),
        )
        .await?;
    std::fs::write(sub.join(".gitattributes"), "f filter=plant\n")?;
    planted
        .execute(&["add", "f", ".gitattributes"], ExecuteOptions::default())
        .await?;
    planted
        .execute(&["commit", "-m", "planted"], ExecuteOptions::default())
        .await?;
    // Stat-dirty, so a status inside the submodule must run the filter to compare contents.
    let timestamp = filetime::FileTime::from_unix_time(2_000_000_000, 0);
    filetime::set_file_times(sub.join("f"), timestamp, timestamp)?;
    std::fs::remove_file(&marker)?;

    let status = repo.git.status().await?;
    let details = repo
        .git
        .status_details_local(t3_git::LocalStatusOptions::default())
        .await?;
    assert!(!marker.exists(), "a status read ran the planted repository");
    assert!(
        status.files.iter().any(|file| file.path == "sub"),
        "the changed submodule commit must still be reported: {:?}",
        status.files
    );
    assert!(details.has_working_tree_changes);
    // The fixture is live: the same status without the setting runs the planted filter.
    repo.command(&[
        "-c",
        "core.fsmonitor=false",
        "status",
        "--porcelain=2",
        "--branch",
        "-z",
    ])
    .await?;
    assert!(
        marker.exists(),
        "the planted filter must run without the setting"
    );
    Ok(())
}

#[tokio::test]
async fn only_the_recorded_project_can_bind_a_recreated_linked_checkout() -> TestResult {
    let project = Repository::new().await?;
    project.commit("base").await?;
    let other = Repository::new().await?;
    let source = project.git.bind_repository().await?.expect("project");
    let wrong = other.git.bind_repository().await?.expect("other project");
    let folder = project.temp.path().join("linked");
    let moved = project.temp.path().join("old");
    let name = folder.to_str().expect("fixture path");
    project
        .command(&["worktree", "add", "-b", "initial", name])
        .await?;
    let checkout = project.git.at(&folder);
    let first = checkout
        .bind_repository()
        .await?
        .expect("original worktree");
    project
        .command(&["worktree", "move", name, moved.to_str().expect("old path")])
        .await?;
    project
        .command(&["worktree", "add", "-b", "recreated", name])
        .await?;
    assert!(checkout.bind_recreated_worktree(&wrong).await.is_err());
    assert!(
        checkout.bind_repository().await.is_err(),
        "original identity must stay pinned"
    );
    let second = checkout.bind_recreated_worktree(&source).await?;
    assert_ne!(first, second);
    assert_eq!(checkout.bind_repository().await?, Some(second));
    Ok(())
}
