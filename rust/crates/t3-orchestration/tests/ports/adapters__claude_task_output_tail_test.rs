use std::{collections::BTreeMap, path::Path};
use t3_orchestration::ports::claude::adapters::claude_task_output_tail::*;
use tokio_util::sync::CancellationToken;

#[test]
fn file_discovery_split_utf8_truncation_and_final_read() {
    let temp = tempfile::tempdir().unwrap();
    let tasks = temp.path().join("project/session/tasks");
    let file = tasks.join("task.output");
    assert!(locate(temp.path(), "session", "task").is_none());
    std::fs::create_dir_all(&tasks).unwrap();
    std::fs::write(&file, &"café 1\n".as_bytes()[..4]).unwrap();
    assert_eq!(locate(temp.path(), "session", "task").unwrap(), file);
    let mut cursor = OutputCursor::default();
    assert_eq!(cursor.read(&file, false).unwrap(), "caf");
    std::fs::write(&file, "café 1\n").unwrap();
    assert_eq!(cursor.read(&file, false).unwrap(), "é 1\n");
    std::fs::write(&file, &"café".as_bytes()[..4]).unwrap();
    assert_eq!(cursor.read(&file, false).unwrap(), "caf");
    std::fs::write(&file, "ok\n").unwrap();
    assert_eq!(cursor.read(&file, false).unwrap(), "ok\n");
    std::fs::write(&file, "ok\nlast\n").unwrap();
    assert_eq!(cursor.read(&file, true).unwrap(), "last\n");
    assert!(locate(temp.path(), "../session", "task").is_none());
    assert!(locate(temp.path(), "session", "../task").is_none());
}
#[test]
fn flood_skips_to_tail_without_splicing_partial_characters() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("task.output");
    std::fs::write(&file, "start\n").unwrap();
    let mut cursor = OutputCursor::default();
    assert_eq!(cursor.read(&file, false).unwrap(), "start\n");
    std::fs::write(&file, format!("start\n{}end.\n", "é\n".repeat(1_000_000))).unwrap();
    let flooded = cursor.read(&file, false).unwrap();
    assert!(flooded.len() <= MAX_READ_BYTES + 1);
    assert!(flooded.starts_with("\n\né"));
    assert!(flooded.ends_with("é\nend.\n"));
    assert!(!flooded.contains('\u{fffd}'));
}
#[tokio::test]
async fn stopping_tailer_reads_last_write_and_awaits_delivery() {
    let temp = tempfile::tempdir().unwrap();
    let tasks = temp.path().join("project/session/tasks");
    std::fs::create_dir_all(&tasks).unwrap();
    let file = tasks.join("task.output");
    std::fs::write(&file, "one\n").unwrap();
    let (sender, mut receiver) = tokio::sync::mpsc::channel(2);
    let stop = CancellationToken::new();
    let token = stop.clone();
    let root = temp.path().to_owned();
    let task = tokio::spawn(async move {
        tail_claude_task_output(root, "session".into(), "task".into(), token, |text| {
            let sender = sender.clone();
            async move {
                sender.send(text).await.unwrap();
            }
        })
        .await;
    });
    assert_eq!(receiver.recv().await.unwrap(), "one\n");
    std::fs::write(file, "one\ntwo\n").unwrap();
    stop.cancel();
    assert_eq!(receiver.recv().await.unwrap(), "two\n");
    task.await.unwrap();
    assert!(receiver.recv().await.is_none());
}
#[test]
fn root_uses_claudes_platform_environment_and_explicit_uid() {
    let root = |env: &[(&str, &str)], platform, cwd| {
        claude_task_output_root(
            &env.iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect::<BTreeMap<_, _>>(),
            Path::new(cwd),
            platform,
            Some(1000),
        )
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
    };
    for (env, platform, cwd, expected) in [
        (
            vec![("TMPDIR", "/var/folders/x/T/")],
            "linux",
            "/",
            "/var/folders/x/T/claude-1000",
        ),
        (
            vec![("TMP", "/tmp/user/1000")],
            "linux",
            "/",
            "/tmp/user/1000/claude-1000",
        ),
        (vec![], "linux", "/", "/tmp/claude-1000"),
        (
            vec![("TMPDIR", "/var/folders/x/T/")],
            "darwin",
            "/",
            "/tmp/claude-1000",
        ),
        (
            vec![("CLAUDE_CODE_TMPDIR", "/srv/claude"), ("TMPDIR", "/other")],
            "linux",
            "/",
            "/srv/claude/claude-1000",
        ),
        (
            vec![("CLAUDE_CODE_TMPDIR", "/srv/claude")],
            "darwin",
            "/",
            "/srv/claude/claude-1000",
        ),
        (
            vec![("TMPDIR", "tmp")],
            "linux",
            "/work/project",
            "/work/project/tmp/claude-1000",
        ),
        (
            vec![("CLAUDE_CODE_TMPDIR", "../claude")],
            "darwin",
            "/work/project",
            "/work/claude/claude-1000",
        ),
    ] {
        assert_eq!(root(&env, platform, cwd), expected);
    }
    assert!(claude_task_output_root(&BTreeMap::new(), Path::new("."), "win32", None).is_none());
}
