mod common;
use common::{Repository, TestResult};
use serde_json::json;
use t3_contracts::vcs::VcsDriverKind;
use t3_git::{GitWorkflowService, VcsDriverRegistry, VcsSelection, resolve_vcs_kind};

#[tokio::test]
async fn workflow_finds_named_refs_across_namespaces_without_pattern_expansion() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    repo.command(&["update-ref", "refs/remotes/upstream/feature/topic", "HEAD"])
        .await?;
    repo.command(&[
        "symbolic-ref",
        "refs/remotes/origin/alias",
        "refs/heads/main",
    ])
    .await?;
    repo.command(&["update-ref", "refs/mirror/origin/mirrored", "HEAD"])
        .await?;
    for (name, expected) in [
        ("main", true),
        ("feature/topic", true),
        ("alias", true),
        ("mirrored", true),
        ("t3/renamed", false),
        ("feature/*", false),
    ] {
        assert_eq!(repo.git.has_ref_named(name).await?, expected);
    }
    let workflow = GitWorkflowService::new(repo.git.clone());
    assert_eq!(workflow.current_branch().await?.as_deref(), Some("main"));
    assert_eq!(
        std::fs::canonicalize(workflow.repository_root().await?.expect("root"))?,
        std::fs::canonicalize(repo.root())?
    );
    assert!(
        repo.git
            .head_path()
            .await
            .is_some_and(|path| path.ends_with(".git/HEAD"))
    );
    Ok(())
}

#[tokio::test]
async fn stacked_commit_uses_supplied_message_and_returns_wire_results_and_progress() -> TestResult
{
    let repo = Repository::new().await?;
    repo.write("new", "new\n")?;
    let workflow = GitWorkflowService::new(repo.git.clone());
    let input = serde_json::from_value(
        json!({"actionId":"action-1","cwd":repo.root(),"action":"commit","commitMessage":"Add new\n\nBody"}),
    )?;
    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = events.clone();
    let result = workflow
        .run_stacked_action(
            &input,
            None,
            Some(std::sync::Arc::new(move |event| {
                observed.lock().expect("events").push(event)
            })),
        )
        .await?;
    assert_eq!(
        result.commit.status,
        t3_contracts::git::GitCommitStepStatus::Created
    );
    assert_eq!(
        repo.command(&["log", "-1", "--format=%B"]).await?.trim(),
        "Add new\n\nBody"
    );
    let events = serde_json::to_value(&*events.lock().expect("events"))?;
    assert_eq!(events[0]["kind"], "action_started");
    assert_eq!(
        events.as_array().expect("events").last().expect("last")["kind"],
        "action_finished"
    );
    let second = workflow.run_stacked_action(&input, None, None).await?;
    assert_eq!(
        second.commit.status,
        t3_contracts::git::GitCommitStepStatus::SkippedNoChanges
    );
    Ok(())
}

#[tokio::test]
async fn stacked_feature_branch_names_are_safe_and_no_changes_refuses_branch_creation() -> TestResult
{
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    let workflow = GitWorkflowService::new(repo.git.clone());
    let input = serde_json::from_value(
        json!({"actionId":"feature-action","cwd":repo.root(),"action":"commit","featureBranch":true,"commitMessage":"Fix café login"}),
    )?;
    assert!(
        workflow
            .run_stacked_action(&input, None, None)
            .await
            .is_err()
    );
    assert_eq!(repo.git.list_local_branch_names().await?, vec!["main"]);
    repo.write("new", "new\n")?;
    let result = workflow.run_stacked_action(&input, None, None).await?;
    assert_eq!(
        result.branch.name.expect("name").expect("value").0,
        "feature/fix-cafe-login"
    );
    Ok(())
}

#[tokio::test]
async fn project_config_searches_ancestors_honors_precedence_and_tolerates_comments() -> TestResult
{
    let temp = tempfile::tempdir()?;
    let nested = temp.path().join("packages/app");
    std::fs::create_dir_all(&nested)?;
    std::fs::create_dir(temp.path().join(".t3code"))?;
    let file = temp.path().join(".t3code/vcs.json");
    std::fs::write(
        &file,
        "{\n// comment\n\"vcs\": {\"kind\":\"jj\",},\n\"vcsKind\":\"git\",\n}",
    )?;
    assert_eq!(
        resolve_vcs_kind(&nested, VcsSelection::Auto).await.0,
        VcsSelection::Kind(VcsDriverKind::Jj)
    );
    assert_eq!(
        resolve_vcs_kind(&nested, VcsSelection::Kind(VcsDriverKind::Git))
            .await
            .0,
        VcsSelection::Kind(VcsDriverKind::Git)
    );
    std::fs::write(&file, "{invalid json")?;
    let (kind, warnings) = resolve_vcs_kind(&nested, VcsSelection::Auto).await;
    assert_eq!(kind, VcsSelection::Auto);
    assert_eq!(warnings[0].operation, "decode");
    Ok(())
}

#[tokio::test]
async fn registry_invalidates_removed_and_nested_repositories_and_rejects_jj() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    let registry = VcsDriverRegistry::new(repo.git.clone());
    assert!(
        registry
            .detect(repo.root(), VcsSelection::Auto)
            .await?
            .is_some()
    );
    let nested = repo.root().join("nested");
    std::fs::create_dir(&nested)?;
    let parent = registry
        .detect(&nested, VcsSelection::Auto)
        .await?
        .expect("parent");
    assert!(parent.root_path.0.ends_with("repo"));
    repo.git
        .at(&nested)
        .execute(&["init"], Default::default())
        .await?;
    let child = registry
        .detect(&nested, VcsSelection::Auto)
        .await?
        .expect("nested");
    assert!(child.root_path.0.ends_with("nested"));
    assert!(registry.get(VcsDriverKind::Jj).is_err());
    std::fs::remove_dir_all(nested.join(".git"))?;
    assert!(
        registry
            .detect(&nested, VcsSelection::Auto)
            .await?
            .expect("parent again")
            .root_path
            .0
            .ends_with("repo")
    );
    Ok(())
}
