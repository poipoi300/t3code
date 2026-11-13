#![cfg(unix)]
#![allow(clippy::unwrap_used)]
use std::time::Duration;
use t3_contracts::base_schemas::TrimmedNonEmptyString as Text;
use t3_terminal::*;
fn open(root: &std::path::Path, id: &str) -> TerminalOpenInput {
    TerminalOpenInput {
        thread_id: Text("unix".into()),
        terminal_id: Text(id.into()),
        cwd: Text(root.to_string_lossy().into_owned()),
        worktree_path: None,
        cols: None,
        rows: None,
        env: None,
        provider_instance_id: None,
        stop_on_settle: None,
    }
}
fn key(id: &str) -> TerminalSessionInput {
    TerminalSessionInput {
        thread_id: Text("unix".into()),
        terminal_id: Text(id.into()),
    }
}
fn manager(root: &std::path::Path) -> TerminalManager {
    let mut options = ManagerOptions::new(root.join("logs"));
    options.preferred_shell = Some("/bin/sh".into());
    TerminalManager::new(options).unwrap()
}
async fn output(stream: &mut AttachStream, needle: &str) -> String {
    let mut text = String::new();
    while let Some(event) = stream.recv().await.unwrap() {
        match event {
            TerminalAttachStreamEvent::TerminalOutputEvent(event) => {
                text.push_str(&event.data);
                if text.contains(needle) {
                    return text;
                }
            }
            TerminalAttachStreamEvent::TerminalErrorEvent(event) => panic!("{}", event.message),
            _ => {}
        }
    }
    panic!("stream ended: {text:?}")
}
async fn exited(stream: &mut AttachStream) -> TerminalExitedEvent {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(TerminalAttachStreamEvent::TerminalExitedEvent(event)) =
                stream.recv().await.unwrap()
            {
                return event;
            }
        }
    })
    .await
    .expect("exit must not await slave EOF")
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exit_is_published_while_a_background_job_holds_the_slave() {
    let temp = tempfile::tempdir().unwrap();
    let manager = manager(temp.path());
    manager.open(open(temp.path(), "exit")).await.unwrap();
    let mut stream = manager.observe(key("exit")).await.unwrap();
    stream.recv().await.unwrap();
    manager
        .write(TerminalWriteInput {
            thread_id: Text("unix".into()),
            terminal_id: Text("exit".into()),
            data: "stty -echo; printf '__%s__\\n' NOECHO\r".into(),
        })
        .await
        .unwrap();
    output(&mut stream, "__NOECHO__").await;
    manager
        .write(TerminalWriteInput {
            thread_id: Text("unix".into()),
            terminal_id: Text("exit".into()),
            data: "sh -c 'trap \"\" HUP; echo __PID_$$; exec sleep 600' &\r".into(),
        })
        .await
        .unwrap();
    let text = output(&mut stream, "__PID_").await;
    let mut text = text;
    while !text
        .rsplit("__PID_")
        .next()
        .unwrap()
        .chars()
        .any(|c| c == '\r' || c == '\n')
    {
        text.push_str(&output(&mut stream, "\n").await);
    }
    let pid = text
        .rsplit("__PID_")
        .next()
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse::<i32>()
        .unwrap();
    manager
        .write(TerminalWriteInput {
            thread_id: Text("unix".into()),
            terminal_id: Text("exit".into()),
            data: "exit 7\r".into(),
        })
        .await
        .unwrap();
    let event = exited(&mut stream).await;
    assert_eq!((event.exit_code, event.exit_signal), (Some(7), Some(0)));
    assert_eq!(manager.snapshot(key("exit")).await.unwrap().pid, None);
    manager
        .close(TerminalCloseInput {
            thread_id: Text("unix".into()),
            terminal_id: Some(Some(Text("exit".into()))),
            delete_history: None,
        })
        .await
        .unwrap();
    // This PID came from the background process created by this test, never a process-table match.
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), manager.shutdown())
        .await
        .unwrap()
        .unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_pty_input_buffer_does_not_block_close_or_another_terminal() {
    let temp = tempfile::tempdir().unwrap();
    let manager = manager(temp.path());
    manager.open(open(temp.path(), "blocked")).await.unwrap();
    let other = manager.open(open(temp.path(), "other")).await.unwrap();
    let mut stream = manager.observe(key("blocked")).await.unwrap();
    stream.recv().await.unwrap();
    manager
        .write(TerminalWriteInput {
            thread_id: Text("unix".into()),
            terminal_id: Text("blocked".into()),
            data: "stty -echo; printf '__%s__' READY; exec sleep 600\r".into(),
        })
        .await
        .unwrap();
    output(&mut stream, "__READY__").await;
    let pending = (0..400)
        .map(|_| {
            manager.write(TerminalWriteInput {
                thread_id: Text("unix".into()),
                terminal_id: Text("blocked".into()),
                data: "x".repeat(60000),
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(manager.snapshot(key("other")).await.unwrap().pid, other.pid);
    tokio::time::timeout(
        Duration::from_secs(3),
        manager.close(TerminalCloseInput {
            thread_id: Text("unix".into()),
            terminal_id: Some(Some(Text("blocked".into()))),
            delete_history: None,
        }),
    )
    .await
    .unwrap()
    .unwrap();
    drop(pending);
    tokio::time::timeout(Duration::from_secs(3), manager.shutdown())
        .await
        .unwrap()
        .unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signal_exit_uses_node_pty_exit_shape_and_stop_after_wait_is_inert() {
    let temp = tempfile::tempdir().unwrap();
    let manager = manager(temp.path());
    manager.open(open(temp.path(), "signal")).await.unwrap();
    let mut stream = manager.observe(key("signal")).await.unwrap();
    stream.recv().await.unwrap();
    manager
        .write(TerminalWriteInput {
            thread_id: Text("unix".into()),
            terminal_id: Text("signal".into()),
            data: "exec sh -c 'kill -TERM $$'\r".into(),
        })
        .await
        .unwrap();
    let event = exited(&mut stream).await;
    assert_eq!((event.exit_code, event.exit_signal), (Some(0), Some(15)));
    manager
        .close(TerminalCloseInput {
            thread_id: Text("unix".into()),
            terminal_id: Some(Some(Text("signal".into()))),
            delete_history: None,
        })
        .await
        .unwrap();
    manager.shutdown().await.unwrap();
}
