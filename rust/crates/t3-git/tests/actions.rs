mod common;
use common::{Repository, TestResult};
use t3_contracts::git::{
    GitCommandFailureReason as Reason, GitPushStepStatus, VcsPullResultStatus,
};
use t3_git::{CommitOptions, GitError, LocalStatusOptions};

#[tokio::test]
async fn selected_commit_resets_staging_and_uses_root_literal_paths_from_nested_cwd() -> TestResult
{
    let repo = Repository::new().await?;
    repo.write("a", "before\n")?;
    repo.write("nested/keep", "before\n")?;
    repo.commit("initial").await?;
    repo.write("a", "after\n")?;
    repo.command(&["add", "a"]).await?;
    repo.write("nested/keep", "working\n")?;
    repo.write("[literal]", "literal\n")?;
    repo.write("-leading", "option-looking path\n")?;
    repo.write("l", "unrelated\n")?;
    let before = repo.index()?;
    let paths = vec!["[literal]".into(), "nested/keep".into(), "-leading".into()];
    let git = repo.git.at(repo.root().join("nested"));
    let prepared = git
        .prepare_commit_context(Some(&paths), false)
        .await?
        .expect("changes");
    assert!(prepared.staged_patch.is_empty());
    assert!(prepared.staged_summary.contains("[literal]"));
    assert_eq!(repo.index()?, before);
    let sha = git
        .commit_message("  Subject...\r\n\r\n  Body line\r\nSecond  ", Some(&paths))
        .await?
        .expect("commit");
    assert_eq!(
        repo.command(&["log", "-1", "--format=%B"]).await?.trim(),
        "Subject...\n\nBody line\nSecond"
    );
    assert_eq!(repo.show(&sha, "a").await?, "before\n");
    assert_eq!(repo.show(&sha, "[literal]").await?, "literal\n");
    assert_eq!(repo.show(&sha, "-leading").await?, "option-looking path\n");
    assert_eq!(repo.show(&sha, "nested/keep").await?, "working\n");
    assert!(
        repo.command(&["diff", "--cached", "--name-only"])
            .await?
            .is_empty()
    );
    assert!(
        repo.command(&["status", "--porcelain"])
            .await?
            .contains("?? l")
    );
    Ok(())
}

#[tokio::test]
async fn preparing_unborn_and_split_indexes_leaves_no_index_or_objects_behind() -> TestResult {
    for split in [false, true] {
        let repo = Repository::new().await?;
        if split {
            repo.write("base", "base\n")?;
            repo.commit("initial").await?;
            repo.command(&["update-index", "--split-index"]).await?;
        }
        repo.write("new", "new content unique to this draft\n")?;
        let index_before = std::fs::read(repo.root().join(".git/index")).ok();
        let objects_before = repo.command(&["count-objects", "-v"]).await?;
        let context = repo
            .git
            .prepare_commit_context(None, true)
            .await?
            .expect("changes");
        assert!(context.staged_patch.contains("new content unique"));
        assert_eq!(
            std::fs::read(repo.root().join(".git/index")).ok(),
            index_before
        );
        assert_eq!(
            repo.command(&["count-objects", "-v"]).await?,
            objects_before
        );
        assert!(
            repo.git
                .commit_message("Initial", Some(&["new".into()]))
                .await?
                .is_some()
        );
    }
    Ok(())
}

#[tokio::test]
async fn partial_commit_refuses_merge_state_and_all_commit_refuses_real_markers() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("conflict", "base\n")?;
    repo.commit("initial").await?;
    repo.command(&["checkout", "-b", "feature"]).await?;
    repo.write("conflict", "feature\n")?;
    repo.commit("feature").await?;
    let feature = repo.command(&["rev-parse", "HEAD"]).await?;
    repo.command(&["checkout", "main"]).await?;
    repo.write("conflict", "main\n")?;
    repo.commit("main").await?;
    assert!(repo.command(&["merge", "feature"]).await.is_err());
    repo.write("selected", "selected\n")?;
    let index = repo.index()?;
    let before = repo.command(&["rev-parse", "HEAD"]).await?;
    assert!(
        repo.git
            .prepare_commit_context(Some(&["selected".into()]), false)
            .await?
            .is_some()
    );
    assert_eq!(repo.index()?, index);
    let error = repo
        .git
        .commit(
            "partial",
            "",
            CommitOptions {
                stage: Some(vec!["selected".into()]),
                ..Default::default()
            },
        )
        .await
        .expect_err("partial merge");
    assert!(
        error
            .to_string()
            .contains("Cannot commit selected files during a merge")
    );
    assert!(repo.root().join(".git/MERGE_HEAD").exists());
    let error = repo
        .git
        .commit(
            "all",
            "",
            CommitOptions {
                stage: Some(vec![]),
                ..Default::default()
            },
        )
        .await
        .expect_err("unresolved");
    assert!(error.to_string().contains("unresolved merge conflicts"));
    assert_eq!(repo.command(&["rev-parse", "HEAD"]).await?, before);
    assert_eq!(repo.index()?, index);
    repo.write("conflict", "Heading\n=======\nresolved\n")?;
    repo.git
        .commit(
            "finish",
            "",
            CommitOptions {
                stage: Some(vec![]),
                ..Default::default()
            },
        )
        .await?;
    assert!(
        repo.command(&["show", "-s", "--format=%P", "HEAD"])
            .await?
            .contains(feature.trim())
    );
    Ok(())
}

#[tokio::test]
async fn selected_commit_stops_on_index_lock_without_discarding_staging() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("a", "a\n")?;
    repo.commit("initial").await?;
    repo.write("a", "changed\n")?;
    repo.command(&["add", "a"]).await?;
    let before = repo.index()?;
    std::fs::write(repo.root().join(".git/index.lock"), "held")?;
    assert!(
        repo.git
            .commit(
                "selected",
                "",
                CommitOptions {
                    stage: Some(vec!["a".into()]),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    assert_eq!(repo.index()?, before);
    assert_eq!(
        std::fs::read_to_string(repo.root().join(".git/index.lock"))?,
        "held"
    );
    Ok(())
}

#[tokio::test]
async fn branch_switch_reuses_tracking_branch_and_missing_name_preserves_dirty_file() -> TestResult
{
    let repo = Repository::new().await?;
    repo.write("base", "base\n")?;
    repo.commit("initial").await?;
    let remote = repo.temp.path().join("remote.git");
    repo.command(&["init", "--bare", &remote.to_string_lossy()])
        .await?;
    repo.command(&["remote", "add", "origin", &remote.to_string_lossy()])
        .await?;
    repo.command(&["push", "origin", "main:topic"]).await?;
    repo.git.create_branch("custom", "origin/topic").await?;
    assert_eq!(
        repo.git.switch_ref("origin/topic").await?.as_deref(),
        Some("custom")
    );
    repo.git.switch_ref("main").await?;
    repo.write("missing", "local work\n")?;
    assert!(repo.git.switch_ref("missing").await.is_err());
    assert_eq!(
        std::fs::read_to_string(repo.root().join("missing"))?,
        "local work\n"
    );
    assert_eq!(repo.git.current_branch().await?.as_deref(), Some("main"));
    assert!(repo.git.switch_ref("--detach").await.is_err());
    Ok(())
}

#[tokio::test]
async fn rename_reserves_case_variants_namespaces_and_exact_names() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    repo.git.create_ref("T3", false).await?;
    assert_eq!(
        repo.git.rename_branch("main", "t3/child", false).await?,
        "t3-child"
    );
    repo.git.create_ref("T3-Fix/child", false).await?;
    assert_eq!(
        repo.git.rename_branch("t3-child", "t3-fix", false).await?,
        "t3-fix-1"
    );
    let error = repo
        .git
        .rename_branch("t3-fix-1", "T3", true)
        .await
        .expect_err("collision");
    assert!(matches!(error, GitError::Command(_)));
    assert_eq!(
        repo.git.current_branch().await?.as_deref(),
        Some("t3-fix-1")
    );
    repo.git.delete_local_branch("T3", false).await?;
    assert!(
        !repo
            .git
            .list_local_branch_names()
            .await?
            .iter()
            .any(|name| name == "T3")
    );
    Ok(())
}

#[tokio::test]
async fn push_publishes_own_branch_and_preserves_remote_and_local_comparison_bases() -> TestResult {
    for local_base in [false, true] {
        let repo = Repository::new().await?;
        repo.write("base", "base\n")?;
        repo.commit("initial").await?;
        let remote = repo.temp.path().join("remote.git");
        repo.command(&["init", "--bare", &remote.to_string_lossy()])
            .await?;
        repo.command(&["remote", "add", "origin", &remote.to_string_lossy()])
            .await?;
        repo.command(&["push", "-u", "origin", "main"]).await?;
        let base_sha = repo.command(&["rev-parse", "main"]).await?;
        repo.command(&[
            "checkout",
            "--track",
            "-b",
            "feature/x",
            if local_base {
                "refs/heads/main"
            } else {
                "refs/remotes/origin/main"
            },
        ])
        .await?;
        repo.write("feature", "feature\n")?;
        repo.commit("feature").await?;
        let result = repo.git.push_current_branch(None, None).await?;
        assert_eq!(result.status, GitPushStepStatus::Pushed);
        assert_eq!(
            result.upstream_branch.expect("upstream").expect("value").0,
            "origin/feature/x"
        );
        assert_eq!(
            repo.git.read_branch_base("feature/x").await?.as_deref(),
            Some("main")
        );
        assert_eq!(
            repo.command(&["rev-parse", "refs/remotes/origin/main"])
                .await?,
            base_sha
        );
        assert_eq!(
            repo.git.push_current_branch(None, None).await?.status,
            GitPushStepStatus::SkippedUpToDate
        );
    }
    Ok(())
}

#[tokio::test]
async fn explicit_push_remote_and_auto_setup_rebase_are_recorded() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    let remote = repo.temp.path().join("remote.git");
    repo.command(&["init", "--bare", &remote.to_string_lossy()])
        .await?;
    repo.command(&["remote", "add", "origin-1", &remote.to_string_lossy()])
        .await?;
    repo.git
        .write_config_value("branch.autoSetupRebase", "always")
        .await?;
    let result = repo.git.push_current_branch(None, Some("origin-1")).await?;
    assert_eq!(result.set_upstream, Some(Some(true)));
    assert_eq!(
        repo.git
            .read_config_value("branch.main.rebase")
            .await?
            .as_deref(),
        Some("true")
    );
    assert_eq!(
        repo.git
            .read_config_value("branch.main.remote")
            .await?
            .as_deref(),
        Some("origin-1")
    );
    assert_eq!(
        repo.git.pull_current_branch().await?.status,
        VcsPullResultStatus::SkippedUpToDate
    );
    Ok(())
}

#[tokio::test]
async fn pull_fast_forwards_and_fetch_missing_branch_does_not_return_stale_tracking_ref()
-> TestResult {
    let repo = Repository::new().await?;
    repo.write("base", "base\n")?;
    repo.commit("initial").await?;
    let remote = repo.temp.path().join("remote.git");
    let peer = repo.temp.path().join("peer");
    repo.command(&["init", "--bare", &remote.to_string_lossy()])
        .await?;
    repo.command(&["remote", "add", "origin", &remote.to_string_lossy()])
        .await?;
    repo.command(&["push", "-u", "origin", "main"]).await?;
    repo.command(&[
        "clone",
        "--branch",
        "main",
        &remote.to_string_lossy(),
        &peer.to_string_lossy(),
    ])
    .await?;
    let git = repo.git.at(&peer);
    std::fs::write(peer.join("advance"), "advance\n")?;
    git.commit_message("Advance", None).await?;
    git.push_current_branch(None, None).await?;
    assert_eq!(
        repo.git.pull_current_branch().await?.status,
        VcsPullResultStatus::Pulled
    );
    assert_eq!(
        std::fs::read_to_string(repo.root().join("advance"))?,
        "advance\n"
    );
    repo.git.fetch_remote("origin", Some("main")).await?;
    repo.command(&[
        "--git-dir",
        &remote.to_string_lossy(),
        "update-ref",
        "-d",
        "refs/heads/main",
    ])
    .await?;
    repo.git.fetch_remote("origin", Some("main")).await?;
    let state = repo
        .git
        .status_details_local(LocalStatusOptions::default())
        .await?;
    assert!(state.has_upstream);
    Ok(())
}

#[test]
fn classifies_only_trusted_diagnostics_and_discards_matched_secrets() {
    use t3_git::classify_git_failure;
    for (stderr, reason) in [
        (
            "fatal: Authentication failed for 'https://secret:token@example.invalid/repo'",
            Some(Reason::AuthenticationFailed),
        ),
        ("remote: Authentication failed", None),
        ("hook says fatal: Authentication failed", None),
        ("error: open('private'): Permission denied", None),
        (
            "git@example: Permission denied (publickey).\nfatal: Could not read from remote repository.",
            Some(Reason::AuthenticationFailed),
        ),
        (
            "fatal: invalid reference: private",
            Some(Reason::RefNotFound),
        ),
        (
            "error: object is a tree, not a commit\nfatal: invalid reference: private",
            None,
        ),
        (
            " ! [remote rejected] main -> main (secret)",
            Some(Reason::PushRejectedByRemote),
        ),
        (
            " ! [rejected] main -> main (non-fast-forward)\nerror: failed to push some refs",
            Some(Reason::PushRejectedNonFastForward),
        ),
        (" ! [rejected] main -> main (non-fast-forward)", None),
        (
            "error: could not lock config file private: File exists",
            Some(Reason::ConfigLocked),
        ),
        (
            "fatal: gitfile does not point to a valid repository",
            Some(Reason::NotARepository),
        ),
    ] {
        assert_eq!(classify_git_failure(stderr), reason, "{stderr}");
    }
}

#[tokio::test]
async fn commit_progress_reports_real_hook_output_and_exit_codes() -> TestResult {
    use t3_git::CommitProgress;
    for reject in [false, true] {
        let repo = Repository::new().await?;
        repo.write("a", "new\n")?;
        let hook = repo.root().join(".git/hooks/pre-commit");
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\nprintf 'hook first\\r\\nhook last' >&2\nexit {}\n",
                if reject { 1 } else { 0 }
            ),
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
        }
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = events.clone();
        let result = repo
            .git
            .commit(
                "commit",
                "",
                CommitOptions {
                    stage: Some(vec![]),
                    progress: Some(std::sync::Arc::new(move |event| {
                        observed.lock().expect("events").push(event)
                    })),
                    ..Default::default()
                },
            )
            .await;
        assert_eq!(result.is_err(), reject);
        let events = events.lock().expect("events");
        assert!(events.iter().any(|event| matches!(event,CommitProgress::HookStarted{hook_name} if hook_name=="pre-commit")));
        assert!(events.iter().any(|event| matches!(event,CommitProgress::HookFinished{hook_name,exit_code,..} if hook_name=="pre-commit" && *exit_code==Some(if reject {1}else{0}))));
        assert!(events.iter().any(|event| matches!(event,CommitProgress::Output{stream:"stderr",text} if text=="hook first")));
        assert!(events.iter().any(|event| matches!(event,CommitProgress::Output{stream:"stderr",text} if text=="hook last")));
    }
    Ok(())
}

#[tokio::test]
async fn config_keys_cannot_turn_a_write_into_an_unset() -> TestResult {
    let repo = Repository::new().await?;
    repo.command(&["config", "t3.keep", "untouched"]).await?;
    assert!(
        repo.git
            .write_config_value("--unset", "t3.keep")
            .await
            .is_err()
    );
    for key in ["--get", "--unset-all", "-z"] {
        assert!(repo.git.write_config_value(key, "t3.keep").await.is_err());
        assert!(repo.git.read_config_value(key).await.is_err());
    }
    assert_eq!(
        repo.git.read_config_value("t3.keep").await?.as_deref(),
        Some("untouched")
    );
    Ok(())
}

#[tokio::test]
async fn selected_paths_cannot_inject_additional_nul_delimited_paths() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("chosen", "base\n")?;
    repo.write("secret", "base\n")?;
    repo.commit("base").await?;
    repo.write("chosen", "changed\n")?;
    repo.write("secret", "changed\n")?;
    repo.command(&["add", "secret"]).await?;
    let index = repo.index()?;
    assert!(
        repo.git
            .commit(
                "injected",
                "",
                CommitOptions {
                    stage: Some(vec!["chosen\0secret".into()]),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    assert_eq!(repo.index()?, index);
    assert_eq!(
        repo.command(&["log", "-1", "--format=%s"]).await?.trim(),
        "base"
    );
    Ok(())
}

#[tokio::test]
async fn fetch_ref_commit_cannot_accept_a_destination_refspec() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("a", "base\n")?;
    let head = repo.commit("base").await?;
    repo.command(&["remote", "add", "origin", "."]).await?;
    assert!(
        repo.git
            .fetch_ref_commit("refs/heads/main:refs/heads/injected")
            .await
            .is_err()
    );
    assert_eq!(repo.command(&["rev-parse", "HEAD"]).await?.trim(), head);
    assert!(
        repo.command(&["for-each-ref", "--format=%(refname)", "refs/heads/injected"])
            .await?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn remote_names_with_credentials_are_not_echoed_in_fetch_errors() -> TestResult {
    let repo = Repository::new().await?;
    let error = repo
        .git
        .fetch_remote("invalid://user:secret-token@host/repo", None)
        .await
        .expect_err("invalid remote");
    assert!(!format!("{error:?} {error}").contains("secret-token"));
    Ok(())
}

#[tokio::test]
async fn internal_indexes_do_not_run_post_index_change_hooks() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("a", "base\n")?;
    repo.commit("base").await?;
    repo.write("a", "working\n")?;
    let hook = repo.root().join(".git/hooks/post-index-change");
    std::fs::write(&hook, "#!/bin/sh\nprintf hook > hook-ran\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(windows)]
    {
        let path = repo
            .git
            .execute(
                &["config", "--get", "core.hooksPath"],
                t3_git::ExecuteOptions {
                    capture: true,
                    ..Default::default()
                },
            )
            .await?
            .stdout_text();
        let path = std::path::Path::new(path.trim());
        assert!(
            path.is_dir(),
            "disabled hooks path must be a server-owned directory: {path:?}"
        );
        assert!(std::fs::read_dir(path)?.next().is_none());
    }
    repo.git.prepare_commit_context(None, true).await?;
    assert!(
        !repo.root().join("hook-ran").exists(),
        "disposable commit index ran a hook"
    );
    repo.git
        .capture_checkpoint(&t3_git::checkpoint_ref("hooks", 0))
        .await?;
    assert!(
        !repo.root().join("hook-ran").exists(),
        "checkpoint index ran a hook"
    );
    Ok(())
}

#[tokio::test]
async fn user_push_honors_configured_ssh_transport() -> TestResult {
    let repo = Repository::new().await?;
    let head = repo.commit("base").await?;
    let bare = repo.temp.path().join("remote.git");
    repo.command(&["init", "--bare", &bare.to_string_lossy()])
        .await?;
    let script = repo.temp.path().join("ssh.sh");
    let marker = repo.temp.path().join("ssh-used");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf configured > '{}'\nexec git-receive-pack '{}'\n",
            marker.to_string_lossy().replace('\\', "/"),
            bare.to_string_lossy().replace('\\', "/")
        ),
    )?;
    repo.command(&[
        "config",
        "core.sshCommand",
        &format!("sh '{}'", script.to_string_lossy().replace('\\', "/")),
    ])
    .await?;
    repo.command(&["config", "ssh.variant", "ssh"]).await?;
    repo.command(&[
        "remote",
        "add",
        "origin",
        "ssh://test@127.0.0.1:1/repository",
    ])
    .await?;
    let pushed = repo.git.push_current_branch(Some("main"), None).await?;
    assert_eq!(pushed.status, t3_contracts::git::GitPushStepStatus::Pushed);
    assert_eq!(std::fs::read_to_string(marker)?, "configured");
    assert_eq!(
        repo.command(&[
            "--git-dir",
            &bare.to_string_lossy(),
            "rev-parse",
            "refs/heads/main"
        ])
        .await?
        .trim(),
        head
    );
    Ok(())
}

#[tokio::test]
async fn background_fetch_is_noninteractive_and_preserves_configured_ssh_transport() -> TestResult {
    let repo = Repository::new().await?;
    let head = repo.commit("base").await?;
    let bare = repo.temp.path().join("remote.git");
    repo.command(&[
        "init",
        "--bare",
        "--initial-branch=main",
        &bare.to_string_lossy(),
    ])
    .await?;
    repo.command(&["push", &bare.to_string_lossy(), "main"])
        .await?;
    let script = repo.temp.path().join("fetch-ssh.sh");
    let marker = repo.temp.path().join("fetch-env");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s|%s|%s|%s|%s' \"$GIT_TERMINAL_PROMPT\" \"$GCM_INTERACTIVE\" \"$GIT_ASKPASS\" \"$SSH_ASKPASS\" \"$SSH_ASKPASS_REQUIRE\" > '{}'\nexec git-upload-pack '{}'\n",
            marker.to_string_lossy().replace('\\', "/"),
            bare.to_string_lossy().replace('\\', "/")
        ),
    )?;
    repo.command(&[
        "config",
        "core.sshCommand",
        &format!("sh '{}'", script.to_string_lossy().replace('\\', "/")),
    ])
    .await?;
    repo.command(&["config", "ssh.variant", "ssh"]).await?;
    repo.command(&[
        "remote",
        "add",
        "origin",
        "ssh://test@127.0.0.1:1/repository",
    ])
    .await?;
    repo.command(&["update-ref", "refs/remotes/origin/main", &head])
        .await?;
    repo.command(&["branch", "--set-upstream-to=origin/main", "main"])
        .await?;
    repo.git.status_details_remote(true).await?;
    assert_eq!(std::fs::read_to_string(marker)?, "0|never|||never");
    Ok(())
}

#[tokio::test]
async fn commit_progress_redacts_urls_from_hook_output() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("base").await?;
    let hook = repo.root().join(".git/hooks/pre-commit");
    std::fs::write(
        &hook,
        "#!/bin/sh\nprintf '%s\\n' 'https://user:secret-token@example.invalid/repo?token=private'\n",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
    }
    repo.write("a", "new\n")?;
    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = events.clone();
    repo.git
        .commit(
            "new",
            "",
            CommitOptions {
                stage: Some(vec![]),
                progress: Some(std::sync::Arc::new(move |event| {
                    observed.lock().expect("events").push(event)
                })),
                ..Default::default()
            },
        )
        .await?;
    let events = events.lock().expect("events");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, t3_git::CommitProgress::Output { .. }))
    );
    assert!(!format!("{events:?}").contains("secret-token"));
    assert!(!format!("{events:?}").contains("private"));
    Ok(())
}

#[tokio::test]
async fn refreshing_a_checkout_never_interprets_a_revision_as_a_reset_option() -> TestResult {
    let repo = Repository::new().await?;
    let head = repo.commit("base").await?;
    assert!(
        repo.git
            .refresh_checked_out_branch("--hard", Some(&head))
            .await
            .is_err()
    );
    assert_eq!(repo.command(&["rev-parse", "HEAD"]).await?.trim(), head);
    Ok(())
}

#[tokio::test]
async fn option_like_branch_names_are_rejected_without_mutation_and_remote_names_are_literal()
-> TestResult {
    let repo = Repository::new().await?;
    repo.write("a", "base\n")?;
    let head = repo.commit("base").await?;
    repo.write("a", "dirty\n")?;
    let index = repo.index()?;
    for name in ["--help", "--delete", "-D"] {
        assert!(repo.git.create_branch(name, "HEAD").await.is_err());
        assert!(repo.git.create_branch("safe", name).await.is_err());
        assert!(repo.git.create_ref(name, false).await.is_err());
        assert!(repo.git.rename_branch("main", name, true).await.is_err());
        assert!(repo.git.rename_branch(name, "safe", true).await.is_err());
        assert!(repo.git.delete_local_branch(name, true).await.is_err());
        assert!(repo.git.switch_ref(name).await.is_err());
    }
    assert_eq!(repo.git.list_local_branch_names().await?, vec!["main"]);
    assert_eq!(repo.command(&["rev-parse", "HEAD"]).await?.trim(), head);
    assert_eq!(repo.index()?, index);
    assert_eq!(std::fs::read_to_string(repo.root().join("a"))?, "dirty\n");
    let bare = repo.temp.path().join("remote.git");
    repo.command(&["init", "--bare", &bare.to_string_lossy()])
        .await?;
    for remote in ["-mirror", "--receive-pack"] {
        repo.command(&["remote", "add", "--", remote, &bare.to_string_lossy()])
            .await?;
        repo.git
            .push_current_branch(Some("main"), Some(remote))
            .await?;
        repo.git.fetch_remote(remote, Some("main")).await?;
        assert_eq!(
            repo.command(&[
                "rev-parse",
                "--verify",
                &format!("refs/remotes/{remote}/main")
            ])
            .await?
            .trim(),
            head
        );
    }
    Ok(())
}
