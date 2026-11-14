mod common;
use common::{Repository, TestResult};
use std::sync::Arc;
use t3_contracts::git::*;
use t3_git::{GitWorkflowService, VcsStatusBroadcaster};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

async fn accept_gate(listener: &tokio::net::TcpListener) -> std::io::Result<tokio::net::TcpStream> {
    let (gate, _) = listener.accept().await?;
    let mut reader = BufReader::new(gate);
    let mut message = String::new();
    reader.read_line(&mut message).await?;
    assert!(message.starts_with("ready{"));
    Ok(reader.into_inner())
}

async fn poll_once<F: std::future::Future>(
    future: std::pin::Pin<&mut F>,
) -> std::task::Poll<F::Output> {
    let mut future = future;
    std::future::poll_fn(|cx| std::task::Poll::Ready(future.as_mut().poll(cx))).await
}

fn gated_workflow(
    repo: &Repository,
    address: std::net::SocketAddr,
    command: &str,
) -> GitWorkflowService {
    let environment = [
        (
            "T3_GIT_GATE_ADDRESS".into(),
            Some(address.to_string().into()),
        ),
        ("T3_GIT_GATE_COMMAND".into(), Some(command.into())),
    ]
    .into();
    GitWorkflowService::new(
        repo.git
            .clone()
            .with_program(env!("CARGO_BIN_EXE_git-gate-fixture"))
            .with_environment(environment),
    )
}

#[tokio::test]
async fn refreshes_queued_during_a_read_share_one_followup_read() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let service =
        VcsStatusBroadcaster::new(gated_workflow(&repo, listener.local_addr()?, "status"));
    let running = service.clone();
    let root = repo.root().to_owned();
    let first = tokio::spawn(async move { running.refresh_status(&root, true).await });
    let mut gate = accept_gate(&listener).await?;
    let second = service.refresh_status(repo.root(), true);
    let third = service.refresh_status(repo.root(), true);
    tokio::pin!(second, third);
    assert!(poll_once(second.as_mut()).await.is_pending());
    assert!(poll_once(third.as_mut()).await.is_pending());
    gate.write_all(b"x").await?;
    let release = async {
        let mut gate = accept_gate(&listener).await?;
        gate.write_all(b"x").await
    };
    let (a, b, c, released) = tokio::join!(first, second, third, release);
    a??;
    b?;
    c?;
    released?;
    let accept = listener.accept();
    tokio::pin!(accept);
    assert!(poll_once(accept.as_mut()).await.is_pending());
    service.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn dropping_last_remote_subscription_cancels_its_running_fetch_and_reaps_children()
-> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    let remote = repo.temp.path().join("remote.git");
    repo.command(&["init", "--bare", &remote.to_string_lossy()])
        .await?;
    repo.command(&["remote", "add", "origin", &remote.to_string_lossy()])
        .await?;
    repo.command(&["push", "-u", "origin", "main"]).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let service = VcsStatusBroadcaster::new(gated_workflow(&repo, listener.local_addr()?, "fetch"));
    let mut stream = service.stream_status(repo.root(), true).await?;
    stream.recv().await.expect("snapshot");
    let mut gate = accept_gate(&listener).await?;
    stream.close().await;
    let mut remaining = Vec::new();
    assert!(match gate.read_to_end(&mut remaining).await {
        Ok(_) => true,
        Err(error) => error.kind() == std::io::ErrorKind::ConnectionReset,
    });
    assert_eq!(service.active_workers(), (0, 0));
    service.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn branch_stream_starts_current_follows_external_switch_and_releases_shared_watch()
-> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    repo.command(&["branch", "other"]).await?;
    let service = VcsStatusBroadcaster::new(GitWorkflowService::new(repo.git.clone()));
    let mut first = service.stream_branch_status(repo.root()).await?;
    let mut second = service.stream_branch_status(repo.root()).await?;
    assert_eq!(
        first.recv().await.expect("initial").ref_name.as_deref(),
        Some("main")
    );
    second.recv().await.expect("initial");
    assert_eq!(service.active_workers(), (1, 0));
    repo.command(&["switch", "other"]).await?;
    assert_eq!(
        first.recv().await.expect("switch").ref_name.as_deref(),
        Some("other")
    );
    assert_eq!(
        second.recv().await.expect("switch").ref_name.as_deref(),
        Some("other")
    );
    first.close().await;
    assert_eq!(service.active_workers(), (1, 0));
    second.close().await;
    assert_eq!(service.active_workers(), (0, 0));
    service.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn detailed_stream_publishes_local_refresh_and_initial_remote_with_disabled_fetch()
-> TestResult {
    let repo = Repository::new().await?;
    repo.write("file", "before\n")?;
    repo.commit("initial").await?;
    let service = VcsStatusBroadcaster::new(GitWorkflowService::new(repo.git.clone()))
        .with_policy(Arc::new(t3_git::DisabledVcsPolling));
    let mut stream = service.stream_status(repo.root(), true).await?;
    assert!(matches!(
        stream.recv().await,
        Some(VcsStatusStreamEvent::Snapshot(_))
    ));
    assert!(matches!(
        stream.recv().await,
        Some(VcsStatusStreamEvent::RemoteUpdated(_))
    ));
    repo.write("file", "after\n")?;
    let refreshed = service.refresh_status(repo.root(), true).await?;
    assert!(refreshed.has_working_tree_changes);
    assert!(matches!(
        stream.recv().await,
        Some(VcsStatusStreamEvent::LocalUpdated(_))
    ));
    let mut passive = service.stream_status(repo.root(), false).await?;
    passive.recv().await.expect("snapshot");
    stream.close().await;
    assert_eq!(service.active_workers(), (1, 0));
    passive.close().await;
    assert_eq!(service.active_workers(), (0, 0));
    service.shutdown().await;
    Ok(())
}
