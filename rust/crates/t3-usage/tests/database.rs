#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
use t3_usage::{
    UsageSpeed,
    database::{AntigravityCache, read_antigravity, read_opencode},
    sources::{TierRequest, resolve_codex_history},
};
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}
fn copy_tree(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for e in fs::read_dir(source).unwrap() {
        let e = e.unwrap();
        let p = target.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &p);
        } else {
            fs::copy(e.path(), p).unwrap();
        }
    }
}
fn records(read: t3_usage::database::DatabaseRead) -> Vec<t3_usage::UsageRecord> {
    assert!(read.errors.is_empty(), "{:?}", read.errors);
    read.files.into_iter().flat_map(|(_, r)| r).collect()
}
#[test]
fn opencode_sqlite_and_legacy_messages_match_typescript_and_are_read_only() {
    let temp = tempfile::tempdir().unwrap();
    copy_tree(&fixtures().join("databases/opencode"), temp.path());
    let path = temp.path().join("opencode.db");
    let before = fs::read(&path).unwrap();
    let expected: Value =
        serde_json::from_slice(&fs::read(fixtures().join("databases.json")).unwrap()).unwrap();
    let actual = records(read_opencode(temp.path(), 0.0));
    assert_eq!(
        serde_json::from_value::<Vec<t3_usage::UsageRecord>>(expected["opencode"].clone()).unwrap(),
        actual
    );
    assert_eq!(before, fs::read(path).unwrap());
    assert_eq!(actual.len(), 132);
    assert!(!temp.path().join("opencode.db-wal").exists());
    assert!(!temp.path().join("opencode.db-shm").exists());
}

#[test]
fn reading_a_closed_wal_database_never_creates_side_files() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("opencode.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE message(id TEXT,session_id TEXT,data TEXT,time_created INTEGER);").unwrap();
    drop(db);
    assert!(!temp.path().join("opencode.db-shm").exists());
    records(read_opencode(temp.path(), 0.0));
    assert!(!temp.path().join("opencode.db-wal").exists());
    assert!(!temp.path().join("opencode.db-shm").exists());
}
#[test]
fn opencode_reads_wal_updates_without_loading_message_bodies() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("opencode.db");
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL;PRAGMA wal_autocheckpoint=0;CREATE TABLE message(id TEXT,session_id TEXT,data TEXT,time_created INTEGER)").unwrap();
    let message=json!({"modelID":"gpt-x","role":"assistant","tokens":{"input":100,"output":20,"reasoning":5},"body":"x".repeat(4*1024*1024)}).to_string();
    db.execute(
        "INSERT INTO message VALUES ('one','session',?1,1780000000000)",
        [message],
    )
    .unwrap();
    let first = records(read_opencode(temp.path(), 0.0));
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].totals.output_tokens, 25);
    db.execute(
        "INSERT INTO message VALUES ('two','session',?1,1780000000001)",
        [json!({"modelID":"gpt-x","tokens":{"input":2}}).to_string()],
    )
    .unwrap();
    assert_eq!(records(read_opencode(temp.path(), 0.0)).len(), 2);
    assert_eq!(
        records(read_opencode(temp.path(), 1780000000001.0)).len(),
        1
    );
}
#[test]
fn corrupt_databases_report_partial_instead_of_hiding_other_sources() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("opencode.db"), "not a sqlite file").unwrap();
    let result = read_opencode(temp.path(), 0.0);
    assert!(!result.missing);
    assert_eq!(result.errors.len(), 1);
    let ag = read_antigravity(&[temp.path().into()], 0.0, &mut AntigravityCache::default());
    assert_eq!(ag.errors.len(), 1);
}
#[test]
fn antigravity_protobuf_models_retries_and_generation_step_deduplication_match_typescript() {
    let temp = tempfile::tempdir().unwrap();
    copy_tree(&fixtures().join("databases/antigravity"), temp.path());
    let expected: Value =
        serde_json::from_slice(&fs::read(fixtures().join("databases.json")).unwrap()).unwrap();
    let mut cache = AntigravityCache::default();
    let roots = vec![temp.path().into()];
    let actual = records(read_antigravity(&roots, 0.0, &mut cache));
    assert_eq!(
        actual,
        serde_json::from_value::<Vec<t3_usage::UsageRecord>>(expected["antigravity"].clone())
            .unwrap()
    );
    assert_eq!(actual.len(), 2);
    assert_eq!(records(read_antigravity(&roots, 0.0, &mut cache)), actual);
    fs::remove_file(temp.path().join("z-evidence.db")).unwrap();
    let without = records(read_antigravity(&roots, 0.0, &mut cache));
    assert_eq!(without[0].model, "antigravity-model-9001");
    assert_eq!(without[1].model, "claude-opus-4-6");
}
#[test]
fn antigravity_cache_observes_wal_updates() {
    let temp = tempfile::tempdir().unwrap();
    copy_tree(&fixtures().join("databases/antigravity"), temp.path());
    let db = rusqlite::Connection::open(temp.path().join("a-usage.db")).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL;PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    let roots = vec![temp.path().into()];
    let mut cache = AntigravityCache::default();
    assert_eq!(records(read_antigravity(&roots, 0.0, &mut cache)).len(), 2);
    db.execute_batch("DELETE FROM gen_metadata;DELETE FROM steps")
        .unwrap();
    assert!(records(read_antigravity(&roots, 0.0, &mut cache)).is_empty());
}
#[test]
fn codex_history_uses_first_running_selection_and_rejects_ambiguous_turns() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("history.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE orchestration_events (aggregate_kind TEXT,application_event_version INTEGER,stream_id TEXT,event_type TEXT,sequence INTEGER,payload_json TEXT);\
        CREATE TABLE orchestration_v2_projection_provider_turns (thread_id TEXT,provider_thread_id TEXT,run_attempt_id TEXT,payload_json TEXT);\
        CREATE TABLE orchestration_v2_projection_run_attempts (attempt_id TEXT,thread_id TEXT,provider_thread_id TEXT,run_id TEXT,payload_json TEXT);").unwrap();
    db.execute("INSERT INTO orchestration_v2_projection_run_attempts VALUES ('attempt','thread','provider-thread','run',?1)",[json!({"nativeThreadId":"session"}).to_string()]).unwrap();
    db.execute("INSERT INTO orchestration_v2_projection_provider_turns VALUES ('thread','provider-thread','attempt',?1)",[json!({"nativeTurnRef":{"driver":"codex","nativeId":"turn"}}).to_string()]).unwrap();
    for (seq, tier) in [(1, "priority"), (2, "ultrafast")] {
        db.execute("INSERT INTO orchestration_events VALUES ('thread',2,'thread','run.updated',?1,?2)",rusqlite::params![seq,json!({"id":"run","activeAttemptId":"attempt","status":"running","modelSelection":{"instanceId":"codex","model":"gpt-x","options":[{"id":"serviceTier","value":tier}]}}).to_string()]).unwrap();
    }
    let request = TierRequest {
        session_id: "session".into(),
        native_turn_id: "turn".into(),
        model: "gpt-x".into(),
    };
    assert_eq!(
        resolve_codex_history(&path, std::slice::from_ref(&request))
            .unwrap()
            .get(&request),
        Some(&UsageSpeed::Fast)
    );
    let wrong = TierRequest {
        session_id: "other".into(),
        ..request.clone()
    };
    assert!(resolve_codex_history(&path, &[wrong]).unwrap().is_empty());
    db.execute("INSERT INTO orchestration_v2_projection_provider_turns SELECT * FROM orchestration_v2_projection_provider_turns",[]).unwrap();
    assert!(resolve_codex_history(&path, &[request]).unwrap().is_empty());
}
