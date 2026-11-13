use std::{fs, sync::Arc};
use t3_workspace::{leases::*, project::*, scripts::*};

#[tokio::test]
async fn reservations_allow_parallel_starts_and_serialize_overlapping_restore() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    fs::create_dir(root.join("child")).expect("test fixture");
    let reservations = WorkspaceReservations::default();
    let first = reservations
        .acquire(root, ReservationKind::TurnStart)
        .await
        .expect("test fixture");
    let second = reservations
        .acquire(&root.join("child"), ReservationKind::TurnStart)
        .await
        .expect("test fixture");
    let mut restore = Box::pin(reservations.acquire(root, ReservationKind::Restore));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(restore.as_mut().poll(cx).is_pending()))
            .await
    );
    drop(first);
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(restore.as_mut().poll(cx).is_pending()))
            .await
    );
    drop(second);
    let restore = restore.await.expect("test fixture");
    let missing = root.join("child/missing");
    let mut blocked = Box::pin(reservations.acquire(&missing, ReservationKind::TurnStart));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(blocked.as_mut().poll(cx).is_pending()))
            .await
    );
    drop(restore);
    let _released = blocked.await.expect("test fixture");
}

#[tokio::test]
async fn reservation_cancellation_does_not_hold_a_lock_and_aliases_conflict() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    fs::create_dir(root.join("real")).expect("test fixture");
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(root.join("real"), root.join("alias")).expect("test fixture");
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("real"), root.join("alias")).expect("test fixture");
    let reservations = WorkspaceReservations::default();
    let held = reservations
        .acquire(&root.join("real/missing"), ReservationKind::Restore)
        .await
        .expect("test fixture");
    let alias = root.join("alias");
    let mut pending = Box::pin(reservations.acquire(&alias, ReservationKind::TurnStart));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(pending.as_mut().poll(cx).is_pending()))
            .await
    );
    drop(pending);
    drop(held);
    let _next = reservations
        .acquire(&root.join("real"), ReservationKind::Restore)
        .await
        .expect("test fixture");
    let leases = WorkspaceLeases::default();
    let held = leases.acquire(root).await.expect("test fixture");
    let mut pending = Box::pin(leases.acquire(root));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(pending.as_mut().poll(cx).is_pending()))
            .await
    );
    drop(held);
    let _next = pending.await.expect("test fixture");
}

#[tokio::test]
async fn scripts_skip_fenced_links_before_terminal_access_and_observe_a_real_shell() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let mut options = t3_terminal::ManagerOptions::from_state_dir(root.join("state"));
    options.home = root.into();
    #[cfg(windows)]
    {
        options.preferred_shell = Some("powershell.exe".into());
        options
            .env
            .insert("USERPROFILE".into(), root.to_string_lossy().into());
    }
    #[cfg(unix)]
    {
        options.preferred_shell = Some("bash".into());
        options
            .env
            .insert("HOME".into(), root.to_string_lossy().into());
    }
    let terminal = t3_terminal::TerminalManager::new(options).expect("terminal manager");
    let runner = ScriptRunner::new(terminal.clone());
    let source = if cfg!(windows) {
        r#"{"scripts":[{"name":"Setup","command":"Write-Output 'script output'; & $env:ComSpec /d /c exit 7","runOnWorktreeCreate":true}]}"#
    } else {
        r#"{"scripts":[{"name":"Setup","command":"printf 'script output\\n'; exit 7","runOnWorktreeCreate":true}]}"#
    };
    let file = parse_project_file(source).expect("test fixture");
    let scripts = scripts_from_file(&file, ["setup".into()]);
    let cwd = root.to_string_lossy();
    let fenced = runner
        .run_for_thread(ScriptInput {
            thread_id: "thread",
            project_root: &cwd,
            worktree_path: "missing",
            scripts: &scripts,
            trigger: ScriptTrigger::Setup,
            fenced_link: true,
            observe_completion: true,
            on_output_line: None,
        })
        .await
        .expect("test fixture");
    assert!(matches!(fenced, RunScriptResult::SkippedForLink));
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let callback = Arc::new(move |line: String| {
        let _ = sender.send(line);
    });
    let result = runner
        .run_for_thread(ScriptInput {
            thread_id: "thread",
            project_root: &cwd,
            worktree_path: &cwd,
            scripts: &scripts,
            trigger: ScriptTrigger::Setup,
            fenced_link: false,
            observe_completion: true,
            on_output_line: Some(callback),
        })
        .await
        .expect("test fixture");
    let RunScriptResult::Started(started) = result else {
        panic!("script did not start")
    };
    assert_eq!(started.terminal_id, "setup-setup");
    let completion = started
        .completion
        .expect("test fixture")
        .await
        .expect("test fixture");
    assert_eq!(completion.exit_code, Some(7));
    let mut saw_output = false;
    while let Ok(line) = receiver.try_recv() {
        saw_output |= line.contains("script output");
    }
    assert!(saw_output);
    drop(runner);
    terminal.shutdown().await.expect("test fixture");
}

#[test]
fn dimensions_headers_include_jpeg_exif_gif_and_webp() {
    use t3_workspace::image::dimensions;
    let gif = b"GIF89a\x20\0\x10\0";
    let result = dimensions(gif).expect("test fixture");
    assert_eq!((result.width, result.height), (32, 16));
    let mut webp = vec![0; 30];
    webp[..4].copy_from_slice(b"RIFF");
    webp[8..12].copy_from_slice(b"WEBP");
    webp[12..16].copy_from_slice(b"VP8X");
    webp[24] = 31;
    webp[27] = 15;
    let result = dimensions(&webp).expect("test fixture");
    assert_eq!((result.width, result.height), (32, 16));
    let jpeg = [0xff, 0xd8, 0xff, 0xc0, 0, 17, 8, 0, 16, 0, 32, 0, 0];
    let result = dimensions(&jpeg).expect("test fixture");
    assert_eq!((result.width, result.height), (32, 16));
    assert!(dimensions(b"malformed").is_none());
}
