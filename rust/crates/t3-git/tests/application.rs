mod common;
use async_trait::async_trait;
use common::{Repository, TestResult};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
struct Generator(AtomicUsize);
#[async_trait]
impl t3_git::WorkflowTextGenerator for Generator {
    async fn commit_message(
        &self,
        _: &t3_git::Git,
        context: &t3_git::PreparedCommit,
    ) -> Result<String, t3_git::GitError> {
        assert!(context.staged_patch.contains("generated"));
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok("Generated subject\n\nGenerated body".into())
    }
    async fn pull_request(
        &self,
        _: &t3_git::Git,
        _: &str,
        _: &str,
    ) -> Result<t3_git::PullRequestContent, t3_git::GitError> {
        Ok(t3_git::PullRequestContent {
            title: "Title".into(),
            body: "Body".into(),
        })
    }
}
use t3_git::{
    GitApplicationService, GitWorkflowService, VcsStatusBroadcaster, WorktreeSetupTracker,
};

#[tokio::test]
async fn rpc_service_creates_worktree_records_base_and_streams_stacked_actions() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    let generator = Arc::new(Generator(AtomicUsize::new(0)));
    let workflow = GitWorkflowService::new(repo.git.clone())
        .with_text_generator(generator.clone())
        .with_worktrees_directory(repo.temp.path().join("worktrees"));
    let broadcaster = VcsStatusBroadcaster::new(workflow.clone());
    let service =
        GitApplicationService::new(workflow, broadcaster.clone(), WorktreeSetupTracker::new());
    let input = serde_json::from_value(
        json!({"cwd":repo.root(),"refName":"main","newRefName":"feature","baseRefName":"main","path":null}),
    )?;
    let tree = service.create_worktree(&input).await?.worktree;
    assert!(std::path::Path::new(&tree.path.0).join(".git").is_file());
    assert_eq!(tree.ref_name.0, "feature");
    assert_eq!(
        repo.git.read_branch_base("feature").await?.as_deref(),
        Some("main")
    );
    repo.write("file", "new\n")?;
    let input = serde_json::from_value(
        json!({"actionId":"action","cwd":repo.root(),"action":"commit","commitMessage":"Add file"}),
    )?;
    let mut action = service.run_stacked_action(input);
    let mut events = vec![];
    while let Some(event) = action.recv().await {
        events.push(serde_json::to_value(event?)?);
    }
    assert_eq!(events.first().expect("started")["kind"], "action_started");
    assert_eq!(events.last().expect("finished")["kind"], "action_finished");
    action.close().await;
    assert_eq!(
        repo.command(&["log", "-1", "--format=%s"]).await?.trim(),
        "Add file"
    );
    for changes in [false, true] {
        if changes {
            repo.write("generated", "generated content\n")?;
        }
        let input = serde_json::from_value(
            json!({"actionId":"generated-action","cwd":repo.root(),"action":"commit"}),
        )?;
        let mut stream = service.run_stacked_action(input);
        let mut finished = None;
        while let Some(event) = stream.recv().await {
            if let t3_contracts::git::GitActionProgressEvent::Finished(event) = event? {
                finished = Some(event.result.commit.status);
            }
        }
        stream.close().await;
        assert_eq!(
            finished,
            Some(if changes {
                t3_contracts::git::GitCommitStepStatus::Created
            } else {
                t3_contracts::git::GitCommitStepStatus::SkippedNoChanges
            })
        );
    }
    assert_eq!(generator.0.load(Ordering::SeqCst), 1);
    repo.git
        .remove_worktree(std::path::Path::new(&tree.path.0), true)
        .await?;
    broadcaster.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn closing_a_stacked_action_cancels_and_reaps_its_commit_process() -> TestResult {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    let repo = Repository::new().await?;
    let before = repo.commit("initial").await?;
    repo.write("file", "new\n")?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let environment = [
        (
            "T3_GIT_GATE_ADDRESS".into(),
            Some(listener.local_addr()?.to_string().into()),
        ),
        ("T3_GIT_GATE_COMMAND".into(), Some("commit".into())),
    ]
    .into();
    let workflow = GitWorkflowService::new(
        repo.git
            .clone()
            .with_program(env!("CARGO_BIN_EXE_git-gate-fixture"))
            .with_environment(environment),
    );
    let broadcaster = VcsStatusBroadcaster::new(workflow.clone());
    let service =
        GitApplicationService::new(workflow, broadcaster.clone(), WorktreeSetupTracker::new());
    let input = serde_json::from_value(
        json!({"actionId":"cancel","cwd":repo.root(),"action":"commit","commitMessage":"Will cancel"}),
    )?;
    let action = service.run_stacked_action(input);
    let (gate, _) = listener.accept().await?;
    let mut reader = BufReader::new(gate);
    let mut message = String::new();
    reader.read_line(&mut message).await?;
    assert!(message.starts_with("ready{"));
    action.close().await;
    let mut remaining = Vec::new();
    assert!(match reader.read_to_end(&mut remaining).await {
        Ok(_) => true,
        Err(error) => error.kind() == std::io::ErrorKind::ConnectionReset,
    });
    assert_eq!(repo.git.resolve_commit("HEAD").await?, before);
    assert!(repo.root().join("file").exists());
    broadcaster.shutdown().await;
    Ok(())
}
