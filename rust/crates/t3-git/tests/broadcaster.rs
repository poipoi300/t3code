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
    for n in 0..128 {
        let path = repo.root().join(format!(".git/watch-noise-{n}.lock"));
        std::fs::write(&path, "noise")?;
        std::fs::remove_file(path)?;
    }
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

#[tokio::test]
async fn watcher_that_ended_before_repository_creation_restarts_for_existing_subscribers()
-> TestResult {
    let repo = Repository::new().await?;
    std::fs::remove_dir_all(repo.root().join(".git"))?;
    let service = VcsStatusBroadcaster::new(GitWorkflowService::new(repo.git.clone()));
    let mut first = service.stream_branch_status(repo.root()).await?;
    assert_eq!(
        first.recv().await.expect("not repo").is_repo,
        Some(Some(false))
    );
    repo.command(&["init", "--initial-branch=main"]).await?;
    let mut second = service.stream_branch_status(repo.root()).await?;
    assert_eq!(
        second
            .recv()
            .await
            .expect("new snapshot")
            .ref_name
            .as_deref(),
        Some("main")
    );
    assert_eq!(
        first
            .recv()
            .await
            .expect("created repo")
            .ref_name
            .as_deref(),
        Some("main")
    );
    assert_eq!(service.active_workers(), (1, 0));
    second.close().await;
    assert_eq!(service.active_workers(), (1, 0));
    first.close().await;
    assert_eq!(service.active_workers(), (0, 0));
    service.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn status_and_branch_reads_never_run_configured_read_helpers() -> TestResult {
    for unborn in [false, true] {
        for global in [false, true] {
            let repo = Repository::new().await?;
            repo.write("tracked.txt", "before\n")?;
            repo.write(".gitattributes", "*.txt diff=read-marker\n")?;
            repo.command(&["add", "."]).await?;
            if !unborn {
                repo.command(&["commit", "-m", "fixture"]).await?;
            }
            repo.write("tracked.txt", "after\n")?;
            repo.write("untracked.txt", "new\n")?;
            let config = repo.temp.path().join("private-global-config");
            let mut markers = Vec::new();
            for (key, name) in [
                ("core.fsmonitor", "fsmonitor"),
                ("core.pager", "pager"),
                ("diff.external", "external"),
                ("diff.read-marker.command", "driver"),
                ("diff.read-marker.textconv", "textconv"),
            ] {
                let marker = repo.temp.path().join(name);
                let quoted = marker
                    .to_string_lossy()
                    .replace('\\', "/")
                    .replace('\'', "'\"'\"'");
                let command = format!("printf invoked >> '{quoted}'; printf 'token\\0'");
                let mut args = vec!["config"];
                let config_text = config.to_string_lossy();
                if global {
                    args.extend(["--file", &config_text]);
                }
                args.extend([key, &command]);
                repo.command(&args).await?;
                markers.push(marker);
            }
            let git = if global {
                repo.git.clone().with_environment(
                    [("GIT_CONFIG_GLOBAL".into(), Some(config.into_os_string()))].into(),
                )
            } else {
                repo.git.clone()
            };
            git.execute(&["status", "--porcelain=2"], Default::default())
                .await?;
            assert!(
                markers[0].exists(),
                "positive control: configured fsmonitor executes"
            );
            for marker in &markers {
                if marker.exists() {
                    std::fs::remove_file(marker)?;
                }
            }
            let service = VcsStatusBroadcaster::new(GitWorkflowService::new(git.clone()))
                .with_policy(Arc::new(t3_git::DisabledVcsPolling));
            let branch = service.refresh_branch_status(repo.root()).await?;
            assert_eq!(branch.ref_name.as_deref(), Some("main"));
            assert!(
                service
                    .refresh_status(repo.root(), false)
                    .await?
                    .has_working_tree_changes
            );
            let mut branch = service.stream_branch_status(repo.root()).await?;
            branch.recv().await.expect("initial branch");
            branch.close().await;
            let mut status = service.stream_status(repo.root(), false).await?;
            status.recv().await.expect("initial status");
            status.close().await;
            git.status().await?;
            git.status_details_local(Default::default()).await?;
            git.status_details_remote(false).await?;
            git.current_branch().await?;
            service.shutdown().await;
            let invoked: Vec<_> = markers.iter().filter(|path| path.exists()).collect();
            assert!(
                invoked.is_empty(),
                "unborn={unborn}, global={global}: {invoked:?}"
            );
        }
    }
    Ok(())
}
