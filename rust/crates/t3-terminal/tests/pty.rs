#![cfg(windows)]
#![allow(clippy::unwrap_used)]
use t3_contracts::base_schemas::TrimmedNonEmptyString;
use t3_terminal::*;

fn input(root: &std::path::Path, thread: &str, id: &str) -> TerminalOpenInput {
    TerminalOpenInput {
        thread_id: TrimmedNonEmptyString(thread.into()),
        terminal_id: TrimmedNonEmptyString(id.into()),
        cwd: TrimmedNonEmptyString(root.to_string_lossy().into_owned()),
        worktree_path: None,
        cols: None,
        rows: None,
        env: None,
        provider_instance_id: None,
        stop_on_settle: None,
    }
}
fn key(thread: &str, id: &str) -> TerminalObserveInput {
    TerminalObserveInput {
        thread_id: TrimmedNonEmptyString(thread.into()),
        terminal_id: TrimmedNonEmptyString(id.into()),
    }
}
fn attach_input(root: &std::path::Path, thread: &str, id: &str) -> TerminalAttachInput {
    TerminalAttachInput {
        thread_id: TrimmedNonEmptyString(thread.into()),
        terminal_id: TrimmedNonEmptyString(id.into()),
        cwd: Some(Some(TrimmedNonEmptyString(
            root.to_string_lossy().into_owned(),
        ))),
        worktree_path: None,
        cols: None,
        rows: None,
        env: None,
        provider_instance_id: None,
        stop_on_settle: None,
        restart_if_not_running: None,
        resume: None,
        max_history_bytes: None,
    }
}
fn manager(root: &std::path::Path) -> TerminalManager {
    let mut options = ManagerOptions::new(root.join("logs"));
    options.preferred_shell = Some("C:\\Windows\\System32\\cmd.exe".into());
    TerminalManager::new(options).unwrap()
}
async fn write(manager: &TerminalManager, thread: &str, id: &str, data: &str) {
    manager
        .write(TerminalWriteInput {
            thread_id: TrimmedNonEmptyString(thread.into()),
            terminal_id: TrimmedNonEmptyString(id.into()),
            data: data.into(),
        })
        .await
        .unwrap();
}
async fn until_output(
    manager: &TerminalManager,
    stream: &mut AttachStream,
    thread: &str,
    id: &str,
    needle: &str,
) -> String {
    let mut output = String::new();
    loop {
        match stream.recv().await.unwrap().expect("stream ended") {
            TerminalAttachStreamEvent::TerminalOutputEvent(TerminalOutputEvent {
                data, ..
            }) => {
                output.push_str(&data);
                if output.contains("\x1b[6n") {
                    write(manager, thread, id, "\x1b[1;1R").await;
                    output = output.replace("\x1b[6n", "");
                }
                if output.contains(needle) {
                    return output;
                }
            }
            TerminalAttachStreamEvent::TerminalErrorEvent(TerminalErrorEvent {
                message, ..
            }) => panic!("PTY error: {message}"),
            TerminalAttachStreamEvent::TerminalExitedEvent(TerminalExitedEvent {
                exit_code,
                ..
            }) => {
                panic!("early exit {exit_code:?}: {output:?}")
            }
            _ => {}
        }
    }
}
async fn snapshot(stream: &mut AttachStream) -> TerminalSessionSnapshot {
    match stream.recv().await.unwrap().unwrap() {
        TerminalAttachStreamEvent::TerminalAttachSnapshotEvent(TerminalAttachSnapshotEvent {
            snapshot,
            ..
        }) => snapshot,
        e => panic!("expected snapshot: {e:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_pty_reuse_scope_output_exit_replay_clear_close_and_resume() {
    let temp = tempfile::tempdir().unwrap();
    let manager = manager(temp.path());
    let mut metadata = manager.subscribe_metadata();
    assert!(
        matches!(metadata.recv().await,Some(TerminalMetadataStreamEvent::TerminalMetadataSnapshotEvent(TerminalMetadataSnapshotEvent {terminals, ..})) if terminals.is_empty())
    );
    let first = manager
        .open(input(temp.path(), "thread-a", "term-1"))
        .await
        .unwrap();
    assert_eq!(first.status, TerminalSessionStatus::Running, "{first:?}");
    let again = manager
        .open(input(temp.path(), "thread-a", "term-1"))
        .await
        .unwrap();
    assert_eq!(again.pid, first.pid);
    let second = manager
        .open(input(temp.path(), "thread-b", "term-1"))
        .await
        .unwrap();
    assert_ne!(first.pid, second.pid);
    let third = manager
        .open(input(temp.path(), "thread-a", "term-2"))
        .await
        .unwrap();
    assert_ne!(third.pid, first.pid);
    let mut stream = manager.observe(key("thread-a", "term-1")).await.unwrap();
    let initial = snapshot(&mut stream).await;
    assert_eq!(initial.pid, first.pid);
    write(
        &manager,
        "thread-a",
        "term-1",
        "@echo off\r\necho __HELLO_%TERM_PROGRAM%__\r\n",
    )
    .await;
    let output = until_output(
        &manager,
        &mut stream,
        "thread-a",
        "term-1",
        "__HELLO_t3code__",
    )
    .await;
    assert!(output.contains("__HELLO_t3code__"));
    let mut reconnect = manager
        .attach(
            TerminalAttachInput {
                resume: Some(Some(true)),
                max_history_bytes: Some(Some(64)),
                ..attach_input(temp.path(), "thread-a", "term-1")
            },
            Some("client".into()),
        )
        .await
        .unwrap();
    let replay = snapshot(&mut reconnect).await;
    assert_eq!(replay.pid, first.pid);
    assert!(replay.history.len() <= 64);
    // A 64-byte cut through ConPTY's title control may legitimately discard the entire tail.
    write(
        &manager,
        "thread-a",
        "term-1",
        "echo __FINAL__ & exit 7\r\n",
    )
    .await;
    let mut last_sequence = replay.sequence.unwrap();
    let mut reconnect_sequence = replay.sequence.flatten().unwrap();
    let mut reconnect_final = false;
    loop {
        match reconnect.recv().await.unwrap().unwrap() {
            TerminalAttachStreamEvent::TerminalOutputEvent(e) => {
                assert_eq!(e.sequence.flatten(), Some(reconnect_sequence + 1));
                reconnect_sequence += 1;
                reconnect_final |= e.data.contains("__FINAL__");
            }
            TerminalAttachStreamEvent::TerminalExitedEvent(e) => {
                assert_eq!(e.sequence.flatten(), Some(reconnect_sequence + 1));
                assert!(reconnect_final);
                assert_eq!(e.exit_code, Some(7));
                break;
            }
            other => panic!("unexpected reconnect event {other:?}"),
        }
    }
    let mut final_seen = false;
    loop {
        match stream.recv().await.unwrap().unwrap() {
            TerminalAttachStreamEvent::TerminalOutputEvent(TerminalOutputEvent {
                sequence,
                data,
                ..
            }) => {
                if sequence.flatten().unwrap() > last_sequence.unwrap() {
                    assert_eq!(sequence.flatten(), last_sequence.map(|n| n + 1));
                    last_sequence = sequence.unwrap();
                }
                final_seen |= data.contains("__FINAL__");
            }
            TerminalAttachStreamEvent::TerminalExitedEvent(TerminalExitedEvent {
                sequence,
                exit_code,
                exit_signal,
                ..
            }) => {
                assert_eq!(sequence.flatten(), last_sequence.map(|n| n + 1));
                assert_eq!(exit_code, Some(7));
                assert_eq!(exit_signal, None);
                assert!(final_seen);
                break;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    write(&manager, "thread-a", "term-1", "ignored after exit").await;
    let observed = manager.snapshot(key("thread-a", "term-1")).await.unwrap();
    assert_eq!(observed.status, TerminalSessionStatus::Exited);
    assert!(observed.history.contains("__FINAL__"));
    manager.flush().await.unwrap();
    let store = HistoryStore::new(temp.path().join("logs"), 5000, 8 * 1024 * 1024);
    assert!(
        store
            .read("thread-a", "term-1")
            .unwrap()
            .contains("__FINAL__")
    );
    manager.clear(key("thread-a", "term-1")).await.unwrap();
    assert!(
        manager
            .snapshot(key("thread-a", "term-1"))
            .await
            .unwrap()
            .history
            .is_empty()
    );
    manager
        .close(TerminalCloseInput {
            thread_id: TrimmedNonEmptyString("thread-a".into()),
            terminal_id: None,
            delete_history: None,
        })
        .await
        .unwrap();
    let mut resumed = manager
        .attach(
            TerminalAttachInput {
                resume: Some(Some(true)),
                ..attach_input(temp.path(), "thread-a", "term-1")
            },
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        resumed.recv().await.unwrap(),
        Some(TerminalAttachStreamEvent::TerminalClosedEvent(
            TerminalClosedEvent { .. }
        ))
    ));
    let reopened = manager
        .open(input(temp.path(), "thread-a", "term-1"))
        .await
        .unwrap();
    assert_eq!(reopened.status, TerminalSessionStatus::Running);
    loop {
        if let TerminalAttachStreamEvent::TerminalAttachSnapshotEvent(
            TerminalAttachSnapshotEvent { snapshot, .. },
        ) = resumed.recv().await.unwrap().unwrap()
        {
            assert_eq!(snapshot.pid, reopened.pid);
            break;
        }
    }
    manager.shutdown().await.unwrap();
    assert!(
        manager
            .open(input(temp.path(), "new", "term-1"))
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_grid_uses_smallest_viewer_and_retains_resize_across_retry() {
    let temp = tempfile::tempdir().unwrap();
    let manager = manager(temp.path());
    let mut wide = manager
        .attach(
            TerminalAttachInput {
                cols: Some(Some(160)),
                rows: Some(Some(50)),
                ..attach_input(temp.path(), "t", "term-1")
            },
            Some("wide".into()),
        )
        .await
        .unwrap();
    assert_eq!(snapshot(&mut wide).await.cols.flatten(), Some(160));
    let mut narrow = manager
        .attach(
            TerminalAttachInput {
                cols: Some(Some(80)),
                rows: Some(Some(24)),
                ..attach_input(temp.path(), "t", "term-1")
            },
            Some("narrow".into()),
        )
        .await
        .unwrap();
    assert_eq!(snapshot(&mut narrow).await.cols.flatten(), Some(80));
    loop {
        if let TerminalAttachStreamEvent::TerminalActivityEvent(TerminalActivityEvent {
            cols,
            rows,
            ..
        }) = wide.recv().await.unwrap().unwrap()
        {
            assert_eq!((cols.flatten(), rows.flatten()), (Some(80), Some(24)));
            break;
        }
    }
    manager
        .resize(
            TerminalResizeInput {
                thread_id: TrimmedNonEmptyString("t".into()),
                terminal_id: TrimmedNonEmptyString("term-1".into()),
                cols: 60,
                rows: 20,
            },
            Some("narrow".into()),
        )
        .await
        .unwrap();
    let mut retry = manager
        .attach(
            TerminalAttachInput {
                cols: Some(Some(80)),
                rows: Some(Some(24)),
                ..attach_input(temp.path(), "t", "term-1")
            },
            Some("narrow".into()),
        )
        .await
        .unwrap();
    assert_eq!(snapshot(&mut retry).await.cols.flatten(), Some(60));
    drop(narrow);
    assert_eq!(
        manager
            .snapshot(key("t", "term-1"))
            .await
            .unwrap()
            .cols
            .flatten(),
        Some(60)
    );
    drop(retry);
    loop {
        if let TerminalAttachStreamEvent::TerminalActivityEvent(TerminalActivityEvent {
            cols: Some(Some(160)),
            rows: Some(Some(50)),
            ..
        }) = wide.recv().await.unwrap().unwrap()
        {
            break;
        }
    }
    manager.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_preserves_disk_history_and_context_changes_clear_it() {
    let temp = tempfile::tempdir().unwrap();
    let manager = manager(temp.path());
    let mut stream = manager
        .attach(attach_input(temp.path(), "t", "term-1"), None)
        .await
        .unwrap();
    snapshot(&mut stream).await;
    write(
        &manager,
        "t",
        "term-1",
        "@echo off\r\necho __PERSISTED__\r\n",
    )
    .await;
    until_output(&manager, &mut stream, "t", "term-1", "__PERSISTED__").await;
    manager.shutdown().await.unwrap();
    drop(manager);
    let manager = self::manager(temp.path());
    let mut recovered = manager
        .attach(
            TerminalAttachInput {
                resume: Some(Some(true)),
                ..attach_input(temp.path(), "t", "term-1")
            },
            None,
        )
        .await
        .unwrap();
    let replay = snapshot(&mut recovered).await;
    assert!(replay.history.contains("__PERSISTED__"));
    assert_eq!(replay.status, TerminalSessionStatus::Running);
    let mut changed = input(temp.path(), "t", "term-1");
    changed.env = Some(Some(Environment::from([(
        "CUSTOM".into(),
        "changed".into(),
    )])));
    let restarted = manager.open(changed).await.unwrap();
    assert_ne!(restarted.pid, replay.pid);
    assert!(restarted.history.is_empty());
    manager
        .close(TerminalCloseInput {
            thread_id: TrimmedNonEmptyString("t".into()),
            delete_history: Some(Some(true)),
            terminal_id: None,
        })
        .await
        .unwrap();
    assert!(
        std::fs::read_dir(temp.path().join("logs"))
            .unwrap()
            .next()
            .is_none()
    );
    manager.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_cwd_and_provider_fail_without_starting_children() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("t3-terminal-validation-cmd.exe");
    std::fs::copy("C:\\Windows\\System32\\cmd.exe", &executable).unwrap();
    let mut options = ManagerOptions::new(temp.path().join("logs"));
    options.preferred_shell = Some(executable.to_string_lossy().into_owned());
    let manager = TerminalManager::new(options).unwrap();
    let mut missing = input(temp.path(), "t", "term-1");
    missing.cwd = TrimmedNonEmptyString(temp.path().join("missing").to_string_lossy().into_owned());
    assert!(matches!(
        manager.open(missing).await,
        Err(TerminalError::CwdNotFound { .. })
    ));
    let mut provider = input(temp.path(), "t", "term-1");
    provider.provider_instance_id = Some(Some(
        t3_contracts::provider_instance::ProviderInstanceId("missing".into()),
    ));
    assert!(matches!(
        manager.open(provider).await,
        Err(TerminalError::ProviderInstanceNotFound(_))
    ));
    assert!(manager.observe(key("t", "term-1")).await.is_err());
    let mut table = sysinfo::System::new().unwrap();
    table.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::Always),
    );
    let expected = executable.canonicalize().unwrap();
    assert!(
        !table.processes().values().any(|process| process
            .exe()
            .and_then(|path| path.canonicalize().ok())
            .is_some_and(|path| path == expected)),
        "invalid inputs spawned a PTY child"
    );
    manager.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_windows_shell_falls_back_and_reports_real_powershell_exit() {
    let temp = tempfile::tempdir().unwrap();
    let mut options = ManagerOptions::new(temp.path().join("logs"));
    options.preferred_shell = Some("t3-nonexistent-shell.exe".into());
    let manager = TerminalManager::new(options).unwrap();
    let mut stream = manager
        .attach(attach_input(temp.path(), "t", "term-1"), None)
        .await
        .unwrap();
    let initial = snapshot(&mut stream).await;
    assert_eq!(
        initial.status,
        TerminalSessionStatus::Running,
        "{initial:?}"
    );
    let shell = manager.get_shell(key("t", "term-1")).await.unwrap();
    assert!(
        shell.to_lowercase().ends_with("powershell.exe") || shell == "pwsh.exe",
        "{shell}"
    );
    write(
        &manager,
        "t",
        "term-1",
        "Write-Output ('__DEFAULT_' + $env:TERM_PROGRAM + '__')\r",
    )
    .await;
    until_output(&manager, &mut stream, "t", "term-1", "__DEFAULT_t3code__").await;
    write(&manager, "t", "term-1", "exit 9\r").await;
    loop {
        if let TerminalAttachStreamEvent::TerminalExitedEvent(TerminalExitedEvent {
            exit_code,
            ..
        }) = stream.recv().await.unwrap().unwrap()
        {
            assert_eq!(exit_code, Some(9));
            break;
        }
    }
    manager.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_environment_resolves_on_open_but_running_attach_keeps_existing_process() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let temp = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let mut options = ManagerOptions::new(temp.path().join("logs"));
    options.preferred_shell = Some("C:\\Windows\\System32\\cmd.exe".into());
    options.resolve_provider_environment = Some(Arc::new(move |id, env| {
        assert_eq!(id, "test-provider");
        let mut env = env.cloned().unwrap_or_default();
        env.insert(
            "PROVIDER_TEST".into(),
            counter.fetch_add(1, Ordering::SeqCst).to_string(),
        );
        Ok(env)
    }));
    let manager = TerminalManager::new(options).unwrap();
    let mut open = input(temp.path(), "t", "term-1");
    open.provider_instance_id = Some(Some(t3_contracts::provider_instance::ProviderInstanceId(
        "test-provider".into(),
    )));
    let initial = manager.open(open.clone()).await.unwrap();
    let mut stream = manager
        .attach(
            TerminalAttachInput {
                provider_instance_id: Some(Some(
                    t3_contracts::provider_instance::ProviderInstanceId("test-provider".into()),
                )),
                ..attach_input(temp.path(), "t", "term-1")
            },
            None,
        )
        .await
        .unwrap();
    let attached = snapshot(&mut stream).await;
    assert_eq!(attached.pid, initial.pid);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    write(
        &manager,
        "t",
        "term-1",
        "@echo off\r\necho __PROVIDER_%PROVIDER_TEST%__\r\n",
    )
    .await;
    until_output(&manager, &mut stream, "t", "term-1", "__PROVIDER_0__").await;
    let restarted = manager.open(open).await.unwrap();
    assert_ne!(restarted.pid, initial.pid);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(restarted.history.is_empty());
    manager.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_settle_and_drop_terminate_descendants_while_idle_close_preserves_busy_shell() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for stop in ["close", "settle", "drop"] {
        let temp = tempfile::tempdir().unwrap();
        let manager = manager(temp.path());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        manager
            .open(TerminalOpenInput {
                stop_on_settle: Some(Some(stop == "settle")),
                ..input(temp.path(), "t", "term-1")
            })
            .await
            .unwrap();
        let command = format!(
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe -NoLogo -NoProfile -Command \"$c=New-Object Net.Sockets.TcpClient('127.0.0.1',{port});$s=$c.GetStream();$s.WriteByte(42);$null=$s.ReadByte()\"\r\n"
        );
        write(&manager, "t", "term-1", &command).await;
        let (mut socket, _) = listener.accept().await.unwrap();
        assert_eq!(socket.read_u8().await.unwrap(), 42);
        manager.refresh_activity().await.unwrap();
        let mut metadata = manager.subscribe_metadata();
        match metadata.recv().await.unwrap() {
            TerminalMetadataStreamEvent::TerminalMetadataSnapshotEvent(
                TerminalMetadataSnapshotEvent { terminals, .. },
            ) => {
                assert_eq!(terminals.len(), 1);
                assert!(terminals[0].has_running_subprocess);
                assert_eq!(terminals[0].label, "powershell");
            }
            e => panic!("{e:?}"),
        }
        manager
            .close_idle(TerminalCloseInput {
                thread_id: TrimmedNonEmptyString("t".into()),
                terminal_id: None,
                delete_history: None,
            })
            .await
            .unwrap();
        assert_eq!(
            manager.snapshot(key("t", "term-1")).await.unwrap().status,
            TerminalSessionStatus::Running
        );
        if stop == "drop" {
            drop(manager);
        } else {
            if stop == "settle" {
                manager
                    .close_on_settle(TerminalThreadInput {
                        thread_id: TrimmedNonEmptyString("t".into()),
                    })
                    .await
                    .unwrap();
            } else {
                manager
                    .close(TerminalCloseInput {
                        thread_id: TrimmedNonEmptyString("t".into()),
                        terminal_id: None,
                        delete_history: None,
                    })
                    .await
                    .unwrap();
            }
            manager.shutdown().await.unwrap();
        }
        let mut byte = [0];
        match socket.read(&mut byte).await {
            Ok(0) => {}
            Err(e)
                if [
                    std::io::ErrorKind::ConnectionReset,
                    std::io::ErrorKind::ConnectionAborted,
                ]
                .contains(&e.kind()) => {}
            other => panic!("descendant still has socket: {other:?}"),
        }
        let _ = socket.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overflow_resyncs_bounded_snapshot_without_restarting_and_follows_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let mut options = ManagerOptions::new(temp.path().join("logs"));
    options.preferred_shell = Some("C:\\Windows\\System32\\cmd.exe".into());
    options.max_queued_output = 1024;
    options.history_line_limit = 5;
    options.history_byte_limit = 512;
    let manager = TerminalManager::new(options).unwrap();
    let mut slow = manager
        .attach(
            TerminalAttachInput {
                max_history_bytes: Some(Some(64)),
                cols: Some(Some(70)),
                rows: Some(Some(20)),
                ..attach_input(temp.path(), "t", "term-1")
            },
            Some("slow".into()),
        )
        .await
        .unwrap();
    let initial = snapshot(&mut slow).await;
    let mut fast = manager.observe(key("t", "term-1")).await.unwrap();
    snapshot(&mut fast).await;
    write(&manager,"t","term-1","@echo off\r\nfor /L %i in (1,1,4000) do @echo line-%i-abcdefghijklmnopqrstuvwxyz\r\necho __DONE_%TERM_PROGRAM%__\r\n").await;
    let mut done = false;
    while !done {
        match fast.recv().await.unwrap().unwrap() {
            TerminalAttachStreamEvent::TerminalOutputEvent(TerminalOutputEvent {
                data, ..
            }) => done = data.contains("__DONE_t3code__"),
            TerminalAttachStreamEvent::TerminalAttachSnapshotEvent(
                TerminalAttachSnapshotEvent { snapshot, .. },
            ) => done = snapshot.history.contains("__DONE_t3code__"),
            TerminalAttachStreamEvent::TerminalErrorEvent(TerminalErrorEvent {
                message, ..
            }) => panic!("{message}"),
            _ => {}
        }
    }
    let replay = snapshot(&mut slow).await;
    assert_eq!(replay.pid, initial.pid);
    assert!(replay.history.len() <= 64);
    manager.flush().await.unwrap();
    let disk =
        std::fs::read(HistoryStore::new(temp.path().join("logs"), 5, 512).path("t", "term-1"))
            .unwrap();
    assert!(disk.len() <= 1024);
    manager
        .close(TerminalCloseInput {
            thread_id: TrimmedNonEmptyString("t".into()),
            terminal_id: None,
            delete_history: None,
        })
        .await
        .unwrap();
    let reopened = manager
        .open(TerminalOpenInput {
            cols: Some(Some(160)),
            rows: Some(Some(50)),
            ..input(temp.path(), "t", "term-1")
        })
        .await
        .unwrap();
    assert_eq!(
        (reopened.cols.flatten(), reopened.rows.flatten()),
        (Some(70), Some(20))
    );
    loop {
        if let TerminalAttachStreamEvent::TerminalAttachSnapshotEvent(
            TerminalAttachSnapshotEvent { snapshot, .. },
        ) = slow.recv().await.unwrap().unwrap()
        {
            assert_eq!(snapshot.pid, reopened.pid);
            break;
        }
    }
    manager.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writes_preserve_invocation_order() {
    let temp = tempfile::tempdir().unwrap();
    let manager = manager(temp.path());
    let mut stream = manager
        .attach(attach_input(temp.path(), "fifo", "term-1"), None)
        .await
        .unwrap();
    snapshot(&mut stream).await;
    write(
        &manager,
        "fifo",
        "term-1",
        "@echo off\r\necho __READY_%TERM_PROGRAM%__\r\n",
    )
    .await;
    until_output(&manager, &mut stream, "fifo", "term-1", "__READY_t3code__").await;
    let expected = (0..400)
        .map(|i| char::from(b'a' + (i * 17 % 26) as u8))
        .collect::<String>();
    manager
        .resize(
            TerminalResizeInput {
                thread_id: TrimmedNonEmptyString("fifo".into()),
                terminal_id: TrimmedNonEmptyString("term-1".into()),
                cols: 1000,
                rows: 30,
            },
            None,
        )
        .await
        .unwrap();
    write(&manager, "fifo", "term-1", "echo ").await;
    let mut pending = Vec::new();
    for ch in expected.chars() {
        pending.push(Some(Box::pin(manager.write(TerminalWriteInput {
            thread_id: TrimmedNonEmptyString("fifo".into()),
            terminal_id: TrimmedNonEmptyString("term-1".into()),
            data: ch.to_string(),
        }))));
    }
    std::future::poll_fn(|cx| {
        let mut complete = true;
        for future in &mut pending {
            if let Some(active) = future {
                if let std::task::Poll::Ready(result) = active.as_mut().poll(cx) {
                    result.unwrap();
                    *future = None;
                } else {
                    complete = false;
                }
            }
        }
        if complete {
            std::task::Poll::Ready(())
        } else {
            std::task::Poll::Pending
        }
    })
    .await;
    write(
        &manager,
        "fifo",
        "term-1",
        "\r\necho __DONE_%TERM_PROGRAM%__\r\n",
    )
    .await;
    let output = until_output(&manager, &mut stream, "fifo", "term-1", "__DONE_t3code__").await;
    assert!(
        output.contains(&expected),
        "input was reordered: {output:?}"
    );
    manager.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_open_does_not_block_another_session_snapshot() {
    use std::sync::{Arc, mpsc};
    let temp = tempfile::tempdir().unwrap();
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let entered = std::sync::Mutex::new(Some(entered));
    let (release, release_rx) = mpsc::channel();
    let release_rx = std::sync::Mutex::new(release_rx);
    let mut options = ManagerOptions::new(temp.path().join("logs"));
    options.preferred_shell = Some("C:\\Windows\\System32\\cmd.exe".into());
    options.resolve_provider_environment = Some(Arc::new(move |_, _| {
        entered.lock().unwrap().take().unwrap().send(()).unwrap();
        release_rx.lock().unwrap().recv().unwrap();
        Ok(Environment::new())
    }));
    let manager = TerminalManager::new(options).unwrap();
    let initial = manager
        .open(input(temp.path(), "fast", "term-1"))
        .await
        .unwrap();
    let copy = manager.clone();
    let mut slow = input(temp.path(), "slow", "term-1");
    slow.provider_instance_id = Some(Some(t3_contracts::provider_instance::ProviderInstanceId(
        "slow".into(),
    )));
    let opening = tokio::spawn(async move { copy.open(slow).await });
    entered_rx.await.unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        manager.snapshot(key("fast", "term-1")),
    )
    .await;
    release.send(()).unwrap();
    opening.await.unwrap().unwrap();
    assert_eq!(
        result
            .expect("snapshot blocked behind another open")
            .unwrap()
            .pid,
        initial.pid
    );
    manager.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_ends_streams_without_closing_client_terminals() {
    let temp = tempfile::tempdir().unwrap();
    let manager = manager(temp.path());
    let mut attached = manager
        .attach(attach_input(temp.path(), "t", "term-1"), None)
        .await
        .unwrap();
    snapshot(&mut attached).await;
    let mut metadata = manager.subscribe_metadata();
    metadata.recv().await.unwrap();
    manager.shutdown().await.unwrap();
    while let Some(event) = attached.recv().await.unwrap() {
        assert!(!matches!(
            event,
            TerminalAttachStreamEvent::TerminalClosedEvent(TerminalClosedEvent { .. })
        ));
    }
    while let Some(event) = metadata.recv().await {
        assert!(!matches!(
            event,
            TerminalMetadataStreamEvent::TerminalMetadataRemoveEvent(
                TerminalMetadataRemoveEvent { .. }
            )
        ));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_executable_does_not_fall_back_to_another_shell() {
    let temp = tempfile::tempdir().unwrap();
    let invalid = temp.path().join("invalid.exe");
    std::fs::write(&invalid, "not an executable").unwrap();
    let mut options = ManagerOptions::new(temp.path().join("logs"));
    options.preferred_shell = Some(invalid.to_string_lossy().into_owned());
    let manager = TerminalManager::new(options).unwrap();
    let result = manager
        .open(input(temp.path(), "t", "term-1"))
        .await
        .unwrap();
    assert_eq!(result.status, TerminalSessionStatus::Error);
    assert_eq!(result.pid, None);
    manager.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn detached_programs_survive_by_default_and_job_termination_is_opt_in() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    fn encoded(script: &str) -> String {
        STANDARD.encode(
            script
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        )
    }
    for kill_descendants in [false, true] {
        for action in ["close", "exit"] {
            let temp = tempfile::tempdir().unwrap();
            let mut options = ManagerOptions::new(temp.path().join("logs"));
            options.preferred_shell = Some("C:\\Windows\\System32\\cmd.exe".into());
            if kill_descendants {
                options.kill_descendants = true;
            }
            let manager = TerminalManager::new(options).unwrap();
            manager
                .open(input(temp.path(), "detached", "term-1"))
                .await
                .unwrap();
            let mut stream = manager.observe(key("detached", "term-1")).await.unwrap();
            snapshot(&mut stream).await;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let child = encoded(&format!(
                "$c=[Net.Sockets.TcpClient]::new('127.0.0.1',{port});$s=$c.GetStream();$s.WriteByte(42);while(($b=$s.ReadByte()) -gt 0){{$s.WriteByte($b)}};$c.Dispose()"
            ));
            let launcher = encoded(&format!(
                "Start-Process -WindowStyle Hidden -FilePath 'C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe' -ArgumentList '-NoProfile','-EncodedCommand','{child}'"
            ));
            write(
                &manager,
                "detached",
                "term-1",
                &format!("powershell.exe -NoProfile -EncodedCommand {launcher}\r\n"),
            )
            .await;
            let (mut socket, _) = listener.accept().await.unwrap();
            assert_eq!(socket.read_u8().await.unwrap(), 42);
            if action == "exit" {
                write(&manager, "detached", "term-1", "exit 0\r\n").await;
                loop {
                    if matches!(
                        stream.recv().await.unwrap(),
                        Some(TerminalAttachStreamEvent::TerminalExitedEvent(..))
                    ) {
                        break;
                    }
                }
            } else {
                manager
                    .close(TerminalCloseInput {
                        thread_id: TrimmedNonEmptyString("detached".into()),
                        terminal_id: None,
                        delete_history: None,
                    })
                    .await
                    .unwrap();
            }
            manager.shutdown().await.unwrap();
            if kill_descendants {
                let mut byte = [0];
                assert!(matches!(socket.read(&mut byte).await, Ok(0) | Err(_)));
            } else {
                socket
                    .write_u8(91)
                    .await
                    .expect("detached child must survive terminal teardown");
                assert_eq!(socket.read_u8().await.unwrap(), 91);
                socket.write_u8(0).await.unwrap();
                let mut byte = [0];
                assert_eq!(socket.read(&mut byte).await.unwrap(), 0);
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_logs_history_deletion_failures_and_still_succeeds() {
    let temp = tempfile::tempdir().unwrap();
    let manager = manager(temp.path());
    manager
        .open(input(temp.path(), "delete", "term-1"))
        .await
        .unwrap();
    manager
        .close(TerminalCloseInput {
            thread_id: TrimmedNonEmptyString("delete".into()),
            terminal_id: None,
            delete_history: None,
        })
        .await
        .unwrap();
    let store = HistoryStore::new(temp.path().join("logs"), 5000, 8 * 1024 * 1024);
    let path = store.path("delete", "term-1");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    manager
        .close(TerminalCloseInput {
            thread_id: TrimmedNonEmptyString("delete".into()),
            terminal_id: Some(Some(TrimmedNonEmptyString("term-1".into()))),
            delete_history: Some(Some(true)),
        })
        .await
        .unwrap();
    assert!(manager.snapshot(key("delete", "term-1")).await.is_err());
    manager.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restarting_attach_uses_the_new_viewer_grid_at_spawn() {
    let temp = tempfile::tempdir().unwrap();
    let manager = manager(temp.path());
    let mut wide = manager
        .attach(
            TerminalAttachInput {
                cols: Some(Some(160)),
                rows: Some(Some(50)),
                ..attach_input(temp.path(), "grid", "term-1")
            },
            Some("wide".into()),
        )
        .await
        .unwrap();
    snapshot(&mut wide).await;
    write(&manager, "grid", "term-1", "exit 0\r\n").await;
    loop {
        if matches!(
            wide.recv().await.unwrap(),
            Some(TerminalAttachStreamEvent::TerminalExitedEvent(..))
        ) {
            break;
        }
    }
    let mut events = manager.subscribe();
    let mut narrow = manager
        .attach(
            TerminalAttachInput {
                cols: Some(Some(60)),
                rows: Some(Some(20)),
                restart_if_not_running: Some(Some(true)),
                ..attach_input(temp.path(), "grid", "term-1")
            },
            Some("narrow".into()),
        )
        .await
        .unwrap();
    assert_eq!(snapshot(&mut narrow).await.cols.flatten(), Some(60));
    match events.recv().await.unwrap() {
        TerminalEvent::TerminalStartedEvent(e) => assert_eq!(
            (e.snapshot.cols.flatten(), e.snapshot.rows.flatten()),
            (Some(60), Some(20))
        ),
        other => panic!("expected start {other:?}"),
    }
    manager.shutdown().await.unwrap();
}
