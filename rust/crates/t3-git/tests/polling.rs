mod common;
use common::{Repository, TestResult};
use t3_contracts::git::VcsStatusStreamEvent;
use t3_git::{GitWorkflowService, VcsStatusBroadcaster};

#[tokio::test]
async fn remote_polling_fetches_at_the_configured_interval_using_virtual_time() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
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
    repo.command(&["push", "-u", "origin", "main"]).await?;
    let producer = repo.temp.path().join("producer");
    repo.command(&[
        "clone",
        &remote.to_string_lossy(),
        &producer.to_string_lossy(),
    ])
    .await?;
    let service = VcsStatusBroadcaster::new(GitWorkflowService::new(repo.git.clone()));
    let mut stream = service.stream_status(repo.root(), true).await?;
    stream.recv().await.expect("snapshot");
    assert!(matches!(
        stream.recv().await,
        Some(VcsStatusStreamEvent::RemoteUpdated(_))
    ));
    let git = repo.git.at(&producer);
    git.execute(
        &["commit", "--allow-empty", "-m", "upstream"],
        Default::default(),
    )
    .await?;
    git.execute(&["push"], Default::default()).await?;
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(30)).await;
    tokio::time::resume();
    let event = stream.recv().await.expect("poll change");
    let VcsStatusStreamEvent::RemoteUpdated(event) = event else {
        panic!("expected remote update");
    };
    assert_eq!(event.remote.expect("remote").behind_count, 1);
    stream.close().await;
    service.shutdown().await;
    Ok(())
}
