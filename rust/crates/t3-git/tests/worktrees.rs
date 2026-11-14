mod common;
use common::{Repository, TestResult};
use t3_git::{
    RestoreParticipant, RestoreSafety, RestoreSession, WorktreeOptions, checkpoint_ref,
    is_restore_isolated,
};

#[tokio::test]
async fn repository_status_branch_and_ref_queries_handle_renames_and_detached_head() -> TestResult {
    let repo = Repository::new().await?;
    assert_eq!(repo.git.current_branch().await?.as_deref(), Some("main"));
    assert!(repo.git.status().await?.head.is_none());
    repo.write("old file", "base\n")?;
    repo.commit("initial").await?;
    repo.command(&["mv", "old file", "new file"]).await?;
    repo.write("untracked with spaces", "new\n")?;
    let status = repo.git.status().await?;
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert!(
        status.files.iter().any(
            |file| file.path == "new file" && file.previous_path.as_deref() == Some("old file")
        )
    );
    assert!(
        status
            .files
            .iter()
            .any(|file| file.path == "untracked with spaces")
    );
    repo.command(&["reset", "--hard"]).await?;
    repo.command(&["checkout", "--detach"]).await?;
    assert!(repo.git.current_branch().await?.is_none());
    assert!(repo.git.status().await?.branch.is_none());
    let refs = repo.git.list_refs(&["refs/heads/"]).await?;
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].name, "refs/heads/main");
    let outside = repo.temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    assert!(!repo.git.at(&outside).is_repository().await?);
    assert!(
        !repo
            .git
            .at(repo.temp.path().join("missing"))
            .is_repository()
            .await?
    );
    let bare = repo.temp.path().join("bare");
    repo.command(&["init", "--bare", &bare.to_string_lossy()])
        .await?;
    assert!(!repo.git.at(bare).is_repository().await?);
    std::fs::write(repo.root().join(".git/config"), "invalid config [")?;
    assert!(repo.git.is_repository().await.is_err());
    Ok(())
}

#[tokio::test]
async fn linked_worktree_capture_add_list_remove_and_dirty_lock_checks() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("a", "base\n")?;
    repo.commit("initial").await?;
    let target = repo.temp.path().join("linked tree");
    let mut options = WorktreeOptions::new(&target, "main");
    options.new_branch = Some("topic".into());
    assert_eq!(repo.git.add_worktree(options).await?, target);
    let worktrees = repo.git.list_worktrees().await?;
    assert_eq!(worktrees.len(), 2);
    assert!(
        worktrees
            .iter()
            .any(|tree| tree.branch.as_deref() == Some("refs/heads/topic"))
    );
    let linked = repo.git.at(&target);
    std::fs::write(target.join("a"), "working\n")?;
    let reference = checkpoint_ref("linked", 0);
    linked.capture_checkpoint(&reference).await?;
    assert_eq!(repo.show(&reference, "a").await?, "working\n");
    assert_eq!(repo.command(&["show", "HEAD:a"]).await?, "base\n");
    let error = repo
        .git
        .remove_worktree(&target, false)
        .await
        .expect_err("dirty worktree");
    #[cfg(windows)]
    assert!(
        matches!(error, t3_git::GitError::Unavailable(_)),
        "{error:?}"
    );
    #[cfg(not(windows))]
    assert!(
        matches!(error, t3_git::GitError::Vcs(ref error) if matches!(error.error, t3_contracts::vcs::VcsError::Exit(_))),
        "{error:?}"
    );
    assert_eq!(std::fs::read(target.join("a"))?, b"working\n");
    repo.command(&["worktree", "lock", &target.to_string_lossy()])
        .await?;
    let error = repo
        .git
        .remove_worktree(&target, true)
        .await
        .expect_err("locked worktree");
    #[cfg(windows)]
    assert!(
        matches!(error, t3_git::GitError::Unavailable(_)),
        "{error:?}"
    );
    #[cfg(not(windows))]
    assert!(
        matches!(error, t3_git::GitError::Vcs(ref error) if matches!(error.error, t3_contracts::vcs::VcsError::Exit(_))),
        "{error:?}"
    );
    assert_eq!(std::fs::read(target.join("a"))?, b"working\n");
    repo.command(&["worktree", "unlock", &target.to_string_lossy()])
        .await?;
    repo.git.remove_worktree(&target, true).await?;
    assert!(!target.exists());
    assert_eq!(repo.git.list_worktrees().await?.len(), 1);
    repo.git.remove_worktree(&target, true).await?;
    assert!(repo.git.has_checkpoint(&reference).await?);
    Ok(())
}

#[tokio::test]
async fn worktree_removal_ignores_status_show_untracked_files_and_refuses_option_names()
-> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    let target = repo.temp.path().join("linked");
    let mut options = WorktreeOptions::new(&target, "main");
    options.new_branch = Some("topic".into());
    repo.git.add_worktree(options).await?;
    repo.command(&["config", "status.showUntrackedFiles", "no"])
        .await?;
    std::fs::write(target.join("untracked"), "keep")?;
    assert!(repo.git.remove_worktree(&target, false).await.is_err());
    assert!(target.join("untracked").exists());
    repo.git.remove_worktree(&target, true).await?;
    assert!(
        repo.git
            .add_worktree(WorktreeOptions::new(&target, "--orphan"))
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn automatic_worktree_paths_are_stable_bounded_and_collision_safe() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    let options = WorktreeOptions {
        reference: "main".into(),
        new_branch: Some("feature/a-long-topic-with-details".into()),
        path: None,
        directory: Some(repo.temp.path().join("trees")),
        submodules: Default::default(),
    };
    std::fs::create_dir(repo.root().join("child"))?;
    let target = repo
        .git
        .at(repo.root().join("child/.."))
        .add_worktree(options.clone())
        .await?;
    let name = target.file_name().expect("basename").to_string_lossy();
    assert!(name.len() <= 40);
    assert!(name.starts_with("feature-a-long-topic-wi-"));
    repo.git.remove_worktree(&target, false).await?;
    repo.command(&["branch", "-D", "feature/a-long-topic-with-details"])
        .await?;
    std::fs::create_dir(&target)?;
    let next = repo.git.add_worktree(options).await?;
    assert_eq!(
        next.file_name().expect("basename").to_string_lossy(),
        format!("{name}-2")
    );
    repo.git.remove_worktree(&next, false).await?;
    Ok(())
}

#[tokio::test]
async fn restore_safety_checks_archived_threads_sessions_and_idle_side_chats() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("scope/a", "base")?;
    let cwd = repo.root().join("scope");
    let mut safety = RestoreSafety {
        thread_id: "mine".into(),
        worktree_path: Some(cwd.clone()),
        scope_cwd: cwd.clone(),
        participants: Vec::new(),
    };
    assert!(is_restore_isolated(&safety)?);
    let other = RestoreParticipant {
        thread_id: "other".into(),
        deleted: false,
        worktree_path: Some(cwd.clone()),
        project_root: None,
        scope_paths: vec![],
        provider_sessions: vec![],
        side_chat: true,
        active_run: false,
        has_mutating_turn_items: false,
    };
    safety.participants.push(other);
    assert!(is_restore_isolated(&safety)?);
    safety.participants[0].has_mutating_turn_items = true;
    assert!(!is_restore_isolated(&safety)?);
    safety.participants[0].deleted = true;
    assert!(is_restore_isolated(&safety)?);
    safety.participants[0].deleted = false;
    safety.participants[0].side_chat = false;
    safety.participants[0].worktree_path = Some(repo.temp.path().join("nonexistent"));
    safety.participants[0]
        .provider_sessions
        .push(RestoreSession {
            cwd: repo.root().into(),
            stopped: false,
            supports_multiple_provider_threads: false,
        });
    assert!(!is_restore_isolated(&safety)?);
    safety.participants[0].provider_sessions[0].stopped = true;
    assert!(is_restore_isolated(&safety)?);
    safety.participants[0].worktree_path = None;
    assert!(!is_restore_isolated(&safety)?);
    safety.participants.clear();
    safety.worktree_path = None;
    assert!(!is_restore_isolated(&safety)?);
    assert!(
        repo.git
            .at(cwd)
            .restore_checkpoint_isolated("refs/t3/missing", false, &safety)
            .await
            .is_err()
    );
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn removing_a_junction_does_not_follow_it_into_another_directory() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    let shared = repo.temp.path().join("shared");
    std::fs::create_dir(&shared)?;
    std::fs::write(shared.join("keep"), "outside worktree")?;
    let target = repo.temp.path().join("linked");
    let mut options = WorktreeOptions::new(&target, "main");
    options.new_branch = Some("topic".into());
    repo.git.add_worktree(options).await?;
    junction::create(&shared, target.join("junction"))?;
    repo.git.remove_worktree(&target, true).await?;
    assert!(!target.exists());
    assert_eq!(
        std::fs::read_to_string(shared.join("keep"))?,
        "outside worktree"
    );
    Ok(())
}

#[tokio::test]
async fn missing_restore_scope_still_overlaps_existing_and_missing_owners() -> TestResult {
    let repo = Repository::new().await?;
    let missing = repo.root().join("missing/deeper");
    let mut safety = RestoreSafety {
        thread_id: "mine".into(),
        worktree_path: Some(missing.clone()),
        scope_cwd: missing,
        participants: vec![RestoreParticipant {
            thread_id: "other".into(),
            deleted: false,
            worktree_path: Some(repo.root().into()),
            project_root: None,
            scope_paths: vec![],
            provider_sessions: vec![],
            side_chat: false,
            active_run: false,
            has_mutating_turn_items: false,
        }],
    };
    assert!(!is_restore_isolated(&safety)?);
    safety.participants[0].worktree_path = Some(repo.root().join("missing"));
    assert!(!is_restore_isolated(&safety)?);
    #[cfg(windows)]
    {
        safety.participants[0].worktree_path = Some(repo.root().join("MISSING"));
        assert!(!is_restore_isolated(&safety)?);
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn worktree_commands_override_disabled_long_paths_including_removal_checks() -> TestResult {
    let repo = Repository::new().await?;
    let path = format!(
        "{}/{}/{}/file",
        "a".repeat(90),
        "b".repeat(90),
        "c".repeat(90)
    );
    repo.write(&path, "keep\n")?;
    repo.command(&["-c", "core.longpaths=true", "add", "."])
        .await?;
    repo.command(&["commit", "-m", "long file"]).await?;
    repo.command(&["config", "core.longpaths", "false"]).await?;
    let target = repo.temp.path().join("long-worktree");
    repo.git
        .add_worktree(WorktreeOptions::new(&target, "HEAD"))
        .await?;
    assert_eq!(std::fs::read(target.join(&path))?, b"keep\n");
    repo.git.remove_worktree(&target, false).await?;
    assert!(!target.exists());
    Ok(())
}

#[tokio::test]
async fn ref_listing_treats_prefixes_as_data() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("base").await?;
    repo.command(&["branch", "second"]).await?;
    assert!(repo.git.list_refs(&["--count=1"]).await?.is_empty());
    Ok(())
}
