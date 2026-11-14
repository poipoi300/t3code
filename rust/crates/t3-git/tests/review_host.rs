mod common;
use async_trait::async_trait;
use common::{Repository, TestResult};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use t3_contracts::source_control::*;
use t3_git::{Git, GitError, GitWorkflowService, ReviewHost};

struct Host {
    git: Git,
    requests: Vec<ChangeRequest>,
    probes: Mutex<Vec<String>>,
    gate: Option<Arc<Gate>>,
}
struct Gate {
    started: tokio::sync::mpsc::UnboundedSender<()>,
    release: tokio::sync::Semaphore,
}
struct Factory {
    first: std::path::PathBuf,
    workflows: [GitWorkflowService; 2],
}
#[async_trait]
impl t3_git::GitWorkflowFactory for Factory {
    async fn for_checkout(&self, cwd: &Path) -> Result<GitWorkflowService, GitError> {
        let index = usize::from(std::fs::canonicalize(cwd)? != std::fs::canonicalize(&self.first)?);
        Ok(self.workflows[index].at(cwd))
    }
}
#[async_trait]
impl ReviewHost for Host {
    async fn list(
        &self,
        _: &Path,
        head: &str,
        _: Option<ChangeRequestState>,
        _: usize,
    ) -> Result<Vec<ChangeRequest>, GitError> {
        self.probes.lock().expect("probes").push(head.into());
        if let Some(gate) = &self.gate {
            let _ = gate.started.send(());
            gate.release.acquire().await.expect("release").forget();
        }
        Ok(self.requests.clone())
    }
    async fn get(&self, _: &Path, _: &str) -> Result<ChangeRequest, GitError> {
        Ok(self.requests[0].clone())
    }
    async fn checkout(&self, cwd: &Path, _: &str) -> Result<(), GitError> {
        self.git
            .at(cwd)
            .execute(
                &["switch", &self.requests[0].head_ref_name.0],
                Default::default(),
            )
            .await
            .map(|_| ())
    }
    async fn create(&self, _: &Path, _: &str, _: &str, _: &str, _: &Path) -> Result<(), GitError> {
        Ok(())
    }
    async fn clone_urls(
        &self,
        _: &Path,
        _: &str,
        _: &str,
    ) -> Result<SourceControlRepositoryCloneUrls, GitError> {
        Err(GitError::Unsupported("unused clone".into()))
    }
    async fn default_branch(&self, _: &Path) -> Result<Option<String>, GitError> {
        Ok(Some("main".into()))
    }
    fn head_ref(&self, number: u64) -> Option<String> {
        Some(format!("refs/pull/{number}/head"))
    }
    async fn targets_remote(&self, _: &Path, _: &str, _: Option<&str>) -> Result<bool, GitError> {
        Ok(false)
    }
}
#[tokio::test]
async fn application_factory_keeps_forge_bindings_and_branch_changes_scoped_to_each_checkout()
-> TestResult {
    let first = Repository::new().await?;
    let second = Repository::new().await?;
    first.commit("first").await?;
    second.commit("second").await?;
    let workflows = [(&first, 11), (&second, 22)].map(|(repo, number)| {
        GitWorkflowService::new(repo.git.clone()).with_review_host(Arc::new(Host {
            git: repo.git.clone(),
            requests: vec![request("main", number)],
            probes: Mutex::new(vec![]),
            gate: None,
        }))
    });
    let template = workflows[0].clone();
    let factory = Arc::new(Factory {
        first: first.root().to_owned(),
        workflows,
    });
    let broadcaster =
        t3_git::VcsStatusBroadcaster::new(template.clone()).with_workflow_factory(factory.clone());
    let service = t3_git::GitApplicationService::new(
        template,
        broadcaster.clone(),
        t3_git::WorktreeSetupTracker::new(),
    )
    .with_workflow_factory(factory);
    let mut changes = broadcaster.stream_branch_changes();
    for (repo, number) in [(&first, 11), (&second, 22)] {
        let input =
            serde_json::from_value(serde_json::json!({"cwd":repo.root(),"reference":"#1"}))?;
        assert_eq!(
            service
                .resolve_pull_request(&input)
                .await?
                .pull_request
                .number,
            number
        );
        let input = serde_json::from_value(serde_json::json!({"cwd":repo.root()}))?;
        let mut branch = service.subscribe_vcs_branch(&input).await?;
        assert_eq!(
            branch.recv().await.expect("initial").ref_name.as_deref(),
            Some("main")
        );
        let change = changes.recv().await.expect("branch change");
        assert_eq!(
            std::fs::canonicalize(change.cwd)?,
            std::fs::canonicalize(repo.root())?
        );
        branch.close().await;
    }
    broadcaster.shutdown().await;
    assert!(changes.recv().await.is_none());
    Ok(())
}

fn request(head: &str, number: u64) -> ChangeRequest {
    serde_json::from_value(serde_json::json!({"provider":"github","number":number,"title":format!("PR {number}"),"url":format!("https://github.com/upstream/repo/pull/{number}"),"baseRefName":"main","headRefName":head,"state":"open","updatedAt":{"_tag":"None"},"isCrossRepository":true,"headRepositoryNameWithOwner":"fork/repo","headRepositoryOwnerLogin":"fork"})).expect("request")
}
#[tokio::test]
async fn fork_lookup_follows_own_published_ref_and_preserves_explicit_pr_worktrees() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    repo.command(&[
        "remote",
        "add",
        "origin",
        "https://github.com/fork/repo.git",
    ])
    .await?;
    repo.command(&["update-ref", "refs/remotes/origin/main", "HEAD"])
        .await?;
    repo.command(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ])
    .await?;
    repo.command(&["switch", "-c", "feature", "--track", "origin/main"])
        .await?;
    let host = Arc::new(Host {
        git: repo.git.clone(),
        requests: vec![request("main", 1), request("feature", 2)],
        probes: Mutex::new(vec![]),
        gate: None,
    });
    let service = GitWorkflowService::new(repo.git.clone()).with_review_host(host.clone());
    assert!(
        service
            .remote_status(false)
            .await?
            .expect("remote")
            .pr
            .is_none()
    );
    assert!(host.probes.lock().expect("probes").is_empty());
    repo.command(&["update-ref", "refs/remotes/origin/feature", "HEAD"])
        .await?;
    service.invalidate_status().await;
    assert_eq!(
        service
            .remote_status(false)
            .await?
            .expect("remote")
            .pr
            .expect("own PR")
            .number,
        2
    );
    repo.command(&["switch", "-c", "t3code/pr-1/main", "--track", "origin/main"])
        .await?;
    service.invalidate_status().await;
    assert_eq!(
        service
            .remote_status(false)
            .await?
            .expect("remote")
            .pr
            .expect("explicit PR")
            .number,
        1
    );
    Ok(())
}

#[tokio::test]
async fn resolves_and_prepares_pr_locally_with_dirty_checkout_protection() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("file", "before\n")?;
    let sha = repo.commit("initial").await?;
    repo.command(&["branch", "feature"]).await?;
    let remote = repo.temp.path().join("remote.git");
    repo.command(&[
        "init",
        "--bare",
        "--initial-branch=main",
        &remote.to_string_lossy(),
    ])
    .await?;
    repo.command(&["remote", "add", "origin", &remote.to_string_lossy()])
        .await?;
    repo.command(&["push", "origin", "main", "feature"]).await?;
    let mut pr = request("feature", 2);
    pr.is_cross_repository = Some(Some(false));
    pr.head_repository_name_with_owner = None;
    pr.head_repository_owner_login = None;
    pr.head_sha = Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(sha)));
    let service = GitWorkflowService::new(repo.git.clone())
        .with_worktrees_directory(repo.temp.path().join("trees"))
        .with_review_host(Arc::new(Host {
            git: repo.git.clone(),
            requests: vec![pr],
            probes: Mutex::new(vec![]),
            gate: None,
        }));
    assert_eq!(
        service
            .resolve_pull_request("#2")
            .await?
            .pull_request
            .number,
        2
    );
    let input = serde_json::from_value(
        serde_json::json!({"cwd":repo.root(),"reference":"#2","mode":"local"}),
    )?;
    repo.write("file", "dirty\n")?;
    assert!(service.prepare_pull_request_thread(&input).await.is_err());
    assert_eq!(service.current_branch().await?.as_deref(), Some("main"));
    repo.command(&["restore", "file"]).await?;
    let result = service.prepare_pull_request_thread(&input).await?;
    assert_eq!(result.branch.0, "feature");
    assert!(result.is_on_pull_request_head);
    assert!(result.worktree_path.is_none());
    repo.command(&["switch", "main"]).await?;
    let input = serde_json::from_value(
        serde_json::json!({"cwd":repo.root(),"reference":"#2","mode":"worktree"}),
    )?;
    let first = service.prepare_pull_request_thread(&input).await?;
    assert!(first.is_on_pull_request_head);
    let path = first.worktree_path.expect("worktree");
    assert_eq!(
        std::fs::read_to_string(Path::new(&path.0).join("file"))?,
        "before\n"
    );
    let reused = service
        .prepare_pull_request_thread(&input)
        .await?
        .worktree_path
        .expect("reused worktree");
    assert_eq!(
        std::fs::canonicalize(&reused.0)?,
        std::fs::canonicalize(&path.0)?
    );
    std::fs::write(Path::new(&path.0).join("file"), "dirty checkout\n")?;
    let reused = service.prepare_pull_request_thread(&input).await?;
    assert_eq!(
        std::fs::canonicalize(&reused.worktree_path.expect("reused dirty worktree").0)?,
        std::fs::canonicalize(&path.0)?
    );
    assert_eq!(
        std::fs::read_to_string(Path::new(&path.0).join("file"))?,
        "dirty checkout\n"
    );
    repo.git.remove_worktree(Path::new(&path.0), true).await?;
    Ok(())
}

#[tokio::test]
async fn late_pr_lookup_cannot_publish_the_previous_branch_and_unsubscribe_cancels_lookup()
-> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    repo.command(&[
        "remote",
        "add",
        "origin",
        "https://github.com/fork/repo.git",
    ])
    .await?;
    repo.command(&["update-ref", "refs/remotes/origin/main", "HEAD"])
        .await?;
    repo.command(&["branch", "--set-upstream-to=origin/main", "main"])
        .await?;
    repo.command(&["branch", "feature"]).await?;
    let (started, mut starts) = tokio::sync::mpsc::unbounded_channel();
    let gate = Arc::new(Gate {
        started,
        release: tokio::sync::Semaphore::new(0),
    });
    let host = Arc::new(Host {
        git: repo.git.clone(),
        requests: vec![request("main", 1)],
        probes: Mutex::new(vec![]),
        gate: Some(gate.clone()),
    });
    let workflow = GitWorkflowService::new(repo.git.clone()).with_review_host(host);
    let service = t3_git::VcsStatusBroadcaster::new(workflow)
        .with_policy(Arc::new(t3_git::DisabledVcsPolling));
    let mut stream = service.stream_status(repo.root(), true).await?;
    stream.recv().await.expect("snapshot");
    starts.recv().await.expect("lookup started");
    repo.command(&["switch", "feature"]).await?;
    service.refresh_status(repo.root(), true).await?;
    gate.release.add_permits(100);
    let status = service.get_status(repo.root()).await?;
    assert_eq!(status.ref_name.expect("branch").0, "feature");
    assert!(status.pr.is_none());
    stream.close().await;
    service.shutdown().await;
    repo.command(&["switch", "main"]).await?;
    let (started, mut starts) = tokio::sync::mpsc::unbounded_channel();
    let gate = Arc::new(Gate {
        started,
        release: tokio::sync::Semaphore::new(0),
    });
    let host = Arc::new(Host {
        git: repo.git.clone(),
        requests: vec![request("main", 1)],
        probes: Mutex::new(vec![]),
        gate: Some(gate),
    });
    let workflow = GitWorkflowService::new(repo.git.clone()).with_review_host(host);
    let service = t3_git::VcsStatusBroadcaster::new(workflow)
        .with_policy(Arc::new(t3_git::DisabledVcsPolling));
    let mut stream = service.stream_status(repo.root(), true).await?;
    stream.recv().await.expect("snapshot");
    starts.recv().await.expect("new lookup started");
    stream.close().await;
    assert_eq!(service.active_workers(), (0, 0));
    service.shutdown().await;
    Ok(())
}
