#![allow(clippy::unwrap_used)]

use std::{
    fs,
    path::Path,
    sync::{Arc, Barrier},
};

use rusqlite::{Connection, TransactionBehavior};
use serde::Deserialize;
use serde_json::json;
use tempfile::TempDir;

use crate::{
    Database, Error, OpenOptions, OwnershipLock, initialize_v2_database,
    migrations::{self, MANIFEST},
};

const SCHEMA: &str = include_str!("../../../fixtures/database/schema.sql");

fn database_at(head: u32) -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    let sql = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("sql/head-{head}.sql")),
    )
    .unwrap();
    seed_ts_head(&connection, head, &sql);
    connection
}

fn seed_ts_head(connection: &Connection, head: u32, schema: &str) {
    connection.execute_batch(schema).unwrap();
    for &(id, name) in MANIFEST.iter().filter(|&&(id, _)| id <= head) {
        connection
            .execute(
                "INSERT INTO effect_sql_migrations(migration_id,name) VALUES (?1,?2)",
                (id, name),
            )
            .unwrap();
    }
}

fn schema(connection: &Connection) -> Vec<(String, String, String, Vec<u8>)> {
    let mut statement = connection.prepare("SELECT type, name, tbl_name, sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' AND name NOT IN (SELECT name FROM pragma_table_list WHERE type = 'shadow') ORDER BY type, name").unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get::<_, String>(3)?.into_bytes(),
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn assert_head_schema(connection: &Connection) {
    let expected = Connection::open_in_memory().unwrap();
    expected.execute_batch(SCHEMA).unwrap();
    let actual = schema(connection);
    let expected = schema(&expected);
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(&expected) {
        assert!(
            actual == expected,
            "schema object {} differs byte for byte",
            actual.1
        );
    }
}

#[test]
fn fresh_database_matches_typescript_schema_ledger_and_pragmas() {
    let temporary = TempDir::new().unwrap();
    let database = Database::open(temporary.path().join("statev2.sqlite")).unwrap();
    assert!(database.reader_warning().is_none());
    #[derive(Deserialize)]
    struct Entry {
        id: u32,
        name: String,
    }
    let ledger: Vec<Entry> =
        serde_json::from_str(include_str!("../../../fixtures/database/migrations.json")).unwrap();
    let pragmas: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/database/pragmas.json")).unwrap();
    database.with_writer(|connection| {
        assert_head_schema(connection);
        let rows = migrations::history(connection)?;
        assert_eq!(rows.iter().map(|row| (row.id, row.name.as_str())).collect::<Vec<_>>(), ledger.iter().map(|row| (row.id, row.name.as_str())).collect::<Vec<_>>());
        assert_eq!(connection.query_row("SELECT count(*) FROM effect_sql_migrations WHERE created_at NOT GLOB '????-??-?? ??:??:??'", [], |row| row.get::<_,i32>(0))?, 0);
        for (name, row) in pragmas.as_object().unwrap() {
            let value = row.as_object().unwrap().values().next().unwrap();
            let actual = connection.query_row(&format!("PRAGMA {name}"), [], |row| {
                Ok(match row.get_ref(0)? {
                    rusqlite::types::ValueRef::Integer(value) => json!(value),
                    rusqlite::types::ValueRef::Text(value) => json!(std::str::from_utf8(value).unwrap()),
                    other => panic!("unexpected pragma {other:?}"),
                })
            })?;
            assert_eq!(&actual, value, "PRAGMA {name}");
        }
        assert_eq!(connection.query_row("SELECT schema_version, last_sequence FROM orchestration_v2_projection_metadata WHERE projection_name = 'thread-projections'", [], |row| Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?)))?, (1,0));
        Ok(())
    }).unwrap();
}

#[test]
fn every_supported_head_resumes_to_identical_schema() {
    for head in 69..=89 {
        let mut connection = database_at(head);
        let executed = migrations::run_migrations(&mut connection, None).unwrap();
        assert_eq!(executed.len(), (89 - head) as usize);
        assert_head_schema(&connection);
        assert!(!migrations::has_pending_migrations(&connection).unwrap());
    }
}

fn insert_message(connection: &Connection, id: &str, text: serde_json::Value) {
    connection.execute("INSERT INTO orchestration_v2_projection_messages(message_id,thread_id,role,streaming,created_at,updated_at,payload_json) VALUES (?1,'thread:quote','user',0,'2026-10-01','2026-10-01',?2)", (id,json!({"text":text}).to_string())).unwrap();
}

fn indexed(connection: &Connection, phrase: &str) -> Vec<String> {
    connection.prepare("SELECT messages.message_id FROM orchestration_v2_message_search AS search INNER JOIN orchestration_v2_projection_messages AS messages ON messages.rowid = search.rowid WHERE orchestration_v2_message_search MATCH ?1 ORDER BY messages.message_id").unwrap().query_map([phrase], |row| row.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap()
}

fn citation(text: &str, comment: Option<&str>) -> String {
    let mut url =
        url::Url::parse("t3-citation://v1/environment%3Alocal/thread%3Aquoted/quoted-answer")
            .unwrap();
    url.query_pairs_mut().extend_pairs([
        ("text", text),
        ("start", "0"),
        ("end", "54"),
        ("prefix", "beacon01"),
        ("suffix", "harbor"),
    ]);
    if let Some(comment) = comment {
        url.query_pairs_mut().append_pair("comment", comment);
    }
    format!("[Assistant quote]({url})")
}

#[test]
fn upgrades_actual_typescript_head80_backfilling_citations_across_pages() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("statev2.sqlite");
    let source = Connection::open(&path).unwrap();
    seed_ts_head(&source, 80, include_str!("../sql/head-80.sql"));
    let quote = citation(
        "A lighthouse guides ships safely through the darkness.",
        Some("what are you talking about"),
    );
    insert_message(
        &source,
        "quoted",
        json!(format!("{quote} quiet silver lantern")),
    );
    insert_message(&source, "plain", json!("quiet silver lantern"));
    insert_message(
        &source,
        "invalid",
        json!("[Assistant quote](t3-citation://v1/not-a-citation)"),
    );
    insert_message(&source, "unreadable", json!(20261001));
    insert_message(
        &source,
        "accented",
        json!(citation("The kittiwake’s résumé", None)),
    );
    let non_scalar = format!(
        r#"{{"text":"\ud800 {}"}}"#,
        citation("Unicode citation", None)
    );
    source.execute("INSERT INTO orchestration_v2_projection_messages(message_id,thread_id,role,streaming,created_at,updated_at,payload_json) VALUES ('non-scalar','thread:quote','user',0,'2026-10-01','2026-10-01',?1)",[non_scalar]).unwrap();
    for index in 0..501 {
        insert_message(&source, &format!("page-{index:03}"), json!(quote));
    }
    drop(source);
    let database = Database::open(&path).unwrap();
    database.with_reader(|connection| {
        assert_head_schema(connection);
        let text: Option<String> = connection.query_row("SELECT search_text FROM orchestration_v2_projection_messages WHERE message_id='quoted'",[],|row| row.get(0))?;
        assert_eq!(text.as_deref(),Some("A lighthouse guides ships safely through the darkness.\nComment: what are you talking about quiet silver lantern"));
        assert_eq!(connection.query_row("SELECT count(*) FROM orchestration_v2_projection_messages WHERE search_text IS NULL",[],|row|row.get::<_,i32>(0))?,3);
        assert_eq!(indexed(connection,"\"résumé\""),["accented"]);
        assert_eq!(indexed(connection,"\"kittiwake's\""),["accented"]);
        assert!(indexed(connection,"\"beacon01\"").is_empty());
        assert_eq!(indexed(connection,"\"what are you talking about\"").len(),502);
        assert_eq!(connection.query_row("SELECT search_text FROM orchestration_v2_projection_messages WHERE message_id='non-scalar'",[],|row|row.get::<_,String>(0))?,"��� Unicode citation");
        Ok(())
    }).unwrap();
}

#[test]
fn citation_parser_preserves_invalid_links_and_utf16_limits() {
    use crate::citations::readable_search_text;
    let valid = citation("words", None);
    assert_eq!(
        readable_search_text(&format!("before {valid} after")),
        Some("before words after".into())
    );
    assert_eq!(
        readable_search_text(&valid.replace("text=words", "text=words\u{85}")),
        Some("words\u{85}".into())
    );
    assert_eq!(
        readable_search_text(&valid.replace("harbor)", "harbor#)")),
        Some("words".into())
    );
    for invalid in [
        valid.replace("start=0", "start=-1"),
        valid.replace("end=54", "end=0"),
        valid.replace("end=54", "end=9007199254740992"),
        valid.replace("end=54", "end=54&end=55"),
        valid.replace("end=54", "end=54&unknown=x"),
        valid.replace("quoted-answer", "%ZZ"),
        valid.replace("quoted-answer", "%FF"),
        valid.replace("quoted-answer", "%20"),
        valid.replace("harbor", "harbor#fragment"),
        citation(&"😀".repeat(4_001), None),
        citation(" \u{feff}", None),
        citation("words", Some(&"x".repeat(8_001))),
    ] {
        assert_eq!(readable_search_text(&invalid), None, "{invalid}");
    }
    assert_eq!(
        readable_search_text(&citation(&"😀".repeat(4_000), None)),
        Some("😀".repeat(4_000))
    );
}

#[test]
fn migrations_keep_completed_steps_and_roll_back_failing_step_and_ledger() {
    let mut connection = database_at(81);
    connection
        .execute_batch("CREATE TABLE orchestration_v2_projection_turn_item_counts(thread_id TEXT);")
        .unwrap();
    assert!(matches!(
        migrations::run_migrations(&mut connection, None),
        Err(Error::Migration { id: 85, .. })
    ));
    assert_eq!(
        migrations::history(&connection).unwrap().last().unwrap().id,
        84
    );
    assert!(
        !migrations::table_exists(&connection, "orchestration_v2_turn_item_counts_insert").unwrap()
    );
    connection.execute_batch("DROP TABLE orchestration_v2_projection_turn_item_counts; CREATE TRIGGER fail_ledger BEFORE INSERT ON effect_sql_migrations WHEN new.migration_id=85 BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
    assert!(migrations::run_migrations(&mut connection, None).is_err());
    assert!(
        !migrations::table_exists(&connection, "orchestration_v2_projection_turn_item_counts")
            .unwrap()
    );
    connection
        .execute_batch("DROP TRIGGER fail_ledger;")
        .unwrap();
    assert_eq!(
        migrations::run_migrations(&mut connection, None)
            .unwrap()
            .first()
            .unwrap()
            .id,
        85
    );
    assert_head_schema(&connection);
}

#[test]
fn ledger_validates_v2_names_gaps_limits_and_accepts_later_builds() {
    for (sql, expected) in [
        (
            "UPDATE effect_sql_migrations SET name='FutureMainMigration' WHERE migration_id=70",
            "70_PeerLinks (recorded: FutureMainMigration)",
        ),
        (
            "DELETE FROM effect_sql_migrations WHERE migration_id=70",
            "70_PeerLinks (recorded: nothing)",
        ),
    ] {
        let mut connection = database_at(72);
        connection.execute_batch(sql).unwrap();
        let before = migrations::history(&connection).unwrap();
        let error = migrations::run_migrations(&mut connection, None).unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
        assert_eq!(migrations::history(&connection).unwrap(), before);
        assert!(
            migrations::run_migrations(&mut connection, Some(69))
                .unwrap()
                .is_empty()
        );
    }
    let mut connection = database_at(89);
    connection.execute_batch("UPDATE effect_sql_migrations SET name='SiteLocalMigration' WHERE migration_id IN (30,54); INSERT INTO effect_sql_migrations(migration_id,name) VALUES (90,'NewerV2Migration');").unwrap();
    assert!(
        migrations::run_migrations(&mut connection, None)
            .unwrap()
            .is_empty()
    );
    connection
        .execute_batch("DELETE FROM effect_sql_migrations WHERE migration_id=89")
        .unwrap();
    assert!(
        migrations::run_migrations(&mut connection, None)
            .unwrap_err()
            .to_string()
            .contains("89_ShellThreadReadIndexes (recorded: nothing)")
    );
    let mut connection = database_at(69);
    connection
        .execute_batch(
            "UPDATE effect_sql_migrations SET name='OrchestrationV2' WHERE migration_id=53",
        )
        .unwrap();
    let before = schema(&connection);
    assert!(
        migrations::run_migrations(&mut connection, None)
            .unwrap_err()
            .to_string()
            .contains("V2 preview has unexpected later migrations")
    );
    assert_eq!(schema(&connection), before);
}

#[test]
fn empty_ledger_left_by_an_interrupted_bootstrap_can_start_fresh() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE effect_sql_migrations(migration_id INTEGER PRIMARY KEY, created_at datetime NOT NULL DEFAULT current_timestamp, name VARCHAR(255) NOT NULL)").unwrap();
    assert_eq!(
        migrations::run_migrations(&mut connection, None)
            .unwrap()
            .len(),
        89
    );
    assert_head_schema(&connection);
}

#[test]
fn concurrent_migration_runners_resume_after_the_winner() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("racing.sqlite");
    let seed = Connection::open(&path).unwrap();
    seed_ts_head(&seed, 80, include_str!("../sql/head-80.sql"));
    drop(seed);
    let barrier = Arc::new(Barrier::new(4));
    let applied = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let path = &path;
                let barrier = barrier.clone();
                scope.spawn(move || {
                    let mut connection = Connection::open(path).unwrap();
                    barrier.wait();
                    migrations::run_migrations(&mut connection, None)
                        .unwrap()
                        .len()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .sum::<usize>()
    });
    assert_eq!(applied, 9);
    assert_head_schema(&Connection::open(path).unwrap());
}

#[test]
fn missing_peer_link_ledger_tail_is_reapplied_idempotently() {
    let mut connection = database_at(72);
    connection
        .execute_batch("DELETE FROM effect_sql_migrations WHERE migration_id BETWEEN 70 AND 72")
        .unwrap();
    assert_eq!(
        migrations::run_migrations(&mut connection, None)
            .unwrap()
            .first()
            .unwrap()
            .id,
        70
    );
    assert_head_schema(&connection);
}

#[test]
fn unsupported_old_and_preview_histories_are_refused_without_schema_changes() {
    for (head, name) in [
        (52, "ProjectionThreadTitleState"),
        (53, "OrchestrationV2"),
        (54, "OrchestrationV2"),
        (68, "ShellStopRequestIndex"),
    ] {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE effect_sql_migrations(migration_id INTEGER PRIMARY KEY,created_at DATETIME DEFAULT current_timestamp,name TEXT); CREATE TABLE preserved(text TEXT);").unwrap();
        connection
            .execute(
                "INSERT INTO effect_sql_migrations(migration_id,name) VALUES (?1,?2)",
                (head, name),
            )
            .unwrap();
        let before = schema(&connection);
        let error = migrations::run_migrations(&mut connection, None).unwrap_err();
        assert!(matches!(error, Error::UnsupportedHead { minimum: 69, .. }));
        assert!(
            error
                .to_string()
                .contains("Open the previous app version once")
        );
        assert_eq!(schema(&connection), before);
    }
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE user_data(value TEXT);")
        .unwrap();
    assert!(matches!(
        migrations::run_migrations(&mut connection, None),
        Err(Error::MissingLedger)
    ));
}

fn upsert_item(connection: &Connection, id: &str, thread: &str, run: Option<&str>) {
    connection.execute("INSERT INTO orchestration_v2_projection_turn_items(turn_item_id,thread_id,run_id,ordinal,type,status,updated_at,payload_json) VALUES (?1,?2,?3,1,'assistant_message','completed','2026-10-29','{}') ON CONFLICT(turn_item_id) DO UPDATE SET thread_id=excluded.thread_id,run_id=excluded.run_id,status=excluded.status",(id,thread,run)).unwrap();
}

fn counts(connection: &Connection) -> Vec<(String, String, i32)> {
    connection.prepare("SELECT thread_id,run_key,item_count FROM orchestration_v2_projection_turn_item_counts ORDER BY thread_id,run_key").unwrap().query_map([],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap()
}

#[test]
fn item_counts_backfill_then_follow_upserts_moves_and_deletes() {
    let mut connection = database_at(84);
    upsert_item(&connection, "runless-1", "thread-1", None);
    upsert_item(&connection, "runless-2", "thread-1", None);
    upsert_item(&connection, "run-1", "thread-1", Some("run-1"));
    migrations::run_migrations(&mut connection, None).unwrap();
    assert_eq!(
        counts(&connection),
        vec![
            ("thread-1".into(), "".into(), 2),
            ("thread-1".into(), "run-1".into(), 1)
        ]
    );
    upsert_item(&connection, "run-1", "thread-1", Some("run-1"));
    upsert_item(&connection, "run-2", "thread-2", Some("run-2"));
    upsert_item(&connection, "runless-2", "thread-1", Some("run-1"));
    assert_eq!(
        counts(&connection),
        vec![
            ("thread-1".into(), "".into(), 1),
            ("thread-1".into(), "run-1".into(), 2),
            ("thread-2".into(), "run-2".into(), 1)
        ]
    );
    upsert_item(&connection, "runless-2", "thread-3", None);
    assert_eq!(
        counts(&connection).last().unwrap(),
        &("thread-3".into(), "".into(), 1)
    );
    connection
        .execute_batch("DELETE FROM orchestration_v2_projection_turn_items")
        .unwrap();
    assert!(counts(&connection).is_empty());
}

#[test]
fn custom_project_titles_backfill_even_when_preview_added_column() {
    for preview in [false, true] {
        let mut connection = database_at(86);
        for (id, payload) in [
            ("renamed", json!({"title":"repo"})),
            ("untouched", json!({"autoPull":true})),
            ("null", json!({"title":null})),
        ] {
            connection.execute("INSERT INTO projection_projects(project_id,title,workspace_root,scripts_json,created_at,updated_at) VALUES (?1,'repo',?1,'[]','2026-10-01','2026-10-01')",[id]).unwrap();
            connection.execute("INSERT INTO orchestration_events(event_id,aggregate_kind,stream_id,stream_version,event_type,occurred_at,actor_kind,payload_json,metadata_json,application_event_version) VALUES (?1,'project',?1,1,'project.meta-updated','2026-10-01','client',?2,'{}',2)",(id,payload.to_string())).unwrap();
        }
        if preview {
            connection.execute_batch("ALTER TABLE projection_projects ADD COLUMN has_custom_title INTEGER NOT NULL DEFAULT 0").unwrap();
        }
        migrations::run_migrations(&mut connection, None).unwrap();
        let renamed: Vec<String> = connection
            .prepare("SELECT project_id FROM projection_projects WHERE has_custom_title=1")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(renamed, ["renamed"]);
        assert_head_schema(&connection);
    }
}

#[test]
fn boolean_settlement_repair_only_changes_boolean_overrides() {
    let mut connection = database_at(76);
    for (id, override_value) in [
        ("true", Some(json!(true))),
        ("false", Some(json!(false))),
        ("settled", Some(json!("settled"))),
        ("active", Some(json!("active"))),
        ("null", Some(json!(null))),
        ("absent", None),
    ] {
        let mut payload = json!({"settledAt":if id=="true" {Some("2026-09-01")}else{None}});
        if let Some(value) = override_value {
            payload["settledOverride"] = value;
        }
        connection.execute("INSERT INTO orchestration_v2_projection_threads(thread_id,project_id,title,default_provider,runtime_mode,interaction_mode,created_at,updated_at,payload_json) VALUES (?1,'project',?1,'codex','full-access','default','2026-09-01','2026-09-01',?2)",(id,payload.to_string())).unwrap();
    }
    let get = |connection: &Connection, id: &str| -> String {
        connection
            .query_row(
                "SELECT payload_json FROM orchestration_v2_projection_threads WHERE thread_id=?1",
                [id],
                |row| row.get(0),
            )
            .unwrap()
    };
    let before: Vec<_> = ["settled", "active", "null", "absent"]
        .map(|id| (id, get(&connection, id)))
        .into();
    migrations::run_migrations(&mut connection, None).unwrap();
    for (id, text) in before {
        assert_eq!(get(&connection, id), text);
    }
    let repaired: serde_json::Value = serde_json::from_str(&get(&connection, "true")).unwrap();
    assert_eq!(repaired["settledOverride"], "settled");
    assert_eq!(repaired["settledAt"], "2026-09-01");
    let repaired: serde_json::Value = serde_json::from_str(&get(&connection, "false")).unwrap();
    assert!(repaired["settledOverride"].is_null());
}

#[test]
fn connection_preserves_existing_auto_vacuum_and_clears_stale_planner_statistics() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("database.sqlite");
    {
        let connection = Connection::open(&path).unwrap();
        seed_ts_head(&connection, 80, include_str!("../sql/head-80.sql"));
        connection.execute_batch("ANALYZE; INSERT INTO sqlite_stat1(tbl,idx,stat) VALUES ('projection_projects',NULL,'1');").unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA auto_vacuum", [], |row| row.get::<_, i32>(0))
                .unwrap(),
            0
        );
    }
    let database = Database::open(&path).unwrap();
    database
        .with_writer(|connection| {
            assert_eq!(
                connection.query_row("PRAGMA auto_vacuum", [], |row| row.get::<_, i32>(0))?,
                0
            );
            assert_eq!(
                connection.query_row("SELECT count(*) FROM sqlite_stat1", [], |row| row
                    .get::<_, i32>(0))?,
                0
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn damaged_database_has_actionable_error_and_is_never_replaced() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("broken.sqlite");
    fs::write(&path, "not a SQLite database").unwrap();
    let error = match Database::open(&path) {
        Ok(_) => panic!("opened damaged database"),
        Err(error) => error,
    };
    assert!(matches!(error, Error::DamagedDatabase { .. }));
    assert!(error.to_string().contains("-wal and -shm"));
    assert_eq!(fs::read_to_string(path).unwrap(), "not a SQLite database");
}

#[test]
fn ownership_lock_blocks_cli_schema_changes_but_allows_current_schema() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("statev2.sqlite");
    {
        let connection = Connection::open(&path).unwrap();
        seed_ts_head(&connection, 80, include_str!("../sql/head-80.sql"));
    }
    let owner = OwnershipLock::acquire(temporary.path()).unwrap();
    let options = OpenOptions {
        migration_owner_directory: Some(temporary.path().to_owned()),
        ..Default::default()
    };
    assert!(matches!(
        Database::open_with_options(&path, options.clone()),
        Err(Error::ServerOwnsDatabase { .. })
    ));
    assert_eq!(
        migrations::history(&Connection::open(&path).unwrap())
            .unwrap()
            .last()
            .unwrap()
            .id,
        80
    );
    drop(owner);
    drop(Database::open_with_options(&path, options.clone()).unwrap());
    let _owner = OwnershipLock::acquire(temporary.path()).unwrap();
    drop(Database::open_with_options(&path, options).unwrap());
}

#[test]
fn v1_snapshot_includes_committed_wal_and_never_mutates_source() {
    let temporary = TempDir::new().unwrap();
    let source_path = temporary.path().join("state.sqlite");
    let destination = temporary.path().join("statev2.sqlite");
    let source = Connection::open(&source_path).unwrap();
    source.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE messages(text TEXT); INSERT INTO messages VALUES ('committed'); BEGIN; INSERT INTO messages VALUES ('uncommitted');").unwrap();
    let original = fs::read(&source_path).unwrap();
    initialize_v2_database(&destination).unwrap();
    let copy = Connection::open(&destination).unwrap();
    let messages: Vec<String> = copy
        .prepare("SELECT text FROM messages")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(messages, ["committed"]);
    assert_eq!(fs::read(&source_path).unwrap(), original);
    copy.execute_batch("CREATE TABLE v2_work(text TEXT); INSERT INTO v2_work VALUES ('keep');")
        .unwrap();
    source
        .execute_batch("ROLLBACK; INSERT INTO messages VALUES ('continued in V1');")
        .unwrap();
    initialize_v2_database(&destination).unwrap();
    assert_eq!(
        copy.query_row("SELECT count(*) FROM messages", [], |row| row
            .get::<_, i32>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        copy.query_row("SELECT text FROM v2_work", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "keep"
    );
    assert_eq!(
        fs::read_dir(temporary.path())
            .unwrap()
            .filter(|entry| entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".v2-import-"))
            .count(),
        0
    );
}

#[test]
fn v1_import_failure_is_typed_and_does_not_publish_partial_snapshot() {
    let temporary = TempDir::new().unwrap();
    let source = temporary.path().join("state.sqlite");
    let destination = temporary.path().join("statev2.sqlite");
    fs::write(&source, "invalid SQLite").unwrap();
    assert!(matches!(
        initialize_v2_database(&destination),
        Err(Error::Import { .. })
    ));
    assert!(!destination.exists());
    assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 1);
    fs::write(&destination, "existing V2 state").unwrap();
    initialize_v2_database(&destination).unwrap();
    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "existing V2 state"
    );
}

#[test]
fn import_starts_fresh_and_cleans_only_abandoned_import_names() {
    let temporary = TempDir::new().unwrap();
    let destination = temporary.path().join("missing").join("statev2.sqlite");
    initialize_v2_database(&destination).unwrap();
    assert!(!destination.exists());
    let destination = temporary.path().join("statev2.sqlite");
    let live = format!(".v2-import-{}-Kq81Zx", std::process::id());
    let mut exited = std::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" });
    if cfg!(windows) {
        exited.args(["/C", "exit 0"]);
    } else {
        exited.args(["-c", "exit 0"]);
    }
    let mut child = exited.spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    let dead = format!(".v2-import-{pid}-Wm3nPa");
    for name in [&live, &dead, ".v2-import-Q3xZ9a", ".v2-import-not-a-copy"] {
        fs::create_dir(temporary.path().join(name)).unwrap();
        fs::write(
            temporary.path().join(name).join("snapshot.sqlite"),
            "partial copy",
        )
        .unwrap();
    }
    initialize_v2_database(&destination).unwrap();
    assert!(temporary.path().join(&live).exists());
    assert!(temporary.path().join(".v2-import-not-a-copy").exists());
    assert!(!temporary.path().join(&dead).exists());
    assert!(!temporary.path().join(".v2-import-Q3xZ9a").exists());
}

#[test]
fn atomic_publish_preserves_racing_winner() {
    let temporary = TempDir::new().unwrap();
    let destination = temporary.path().join("statev2.sqlite");
    let first = temporary.path().join("first.sqlite");
    let second = temporary.path().join("second.sqlite");
    fs::write(&first, "first complete snapshot").unwrap();
    fs::write(&second, "second complete snapshot").unwrap();
    let barrier = Arc::new(Barrier::new(2));
    std::thread::scope(|scope| {
        for snapshot in [&first, &second] {
            let barrier = barrier.clone();
            let destination = &destination;
            scope.spawn(move || {
                barrier.wait();
                crate::import::publish_snapshot(snapshot, destination).unwrap();
            });
        }
    });
    let winner = fs::read_to_string(&destination).unwrap();
    assert!(winner == "first complete snapshot" || winner == "second complete snapshot");
    crate::import::publish_snapshot(&first, &destination).unwrap();
    assert_eq!(fs::read_to_string(&destination).unwrap(), winner);
}

#[test]
fn transactions_rollback_and_readers_observe_committed_snapshots_and_are_read_only() {
    let temporary = TempDir::new().unwrap();
    let database = Database::open(temporary.path().join("db.sqlite")).unwrap();
    database
        .with_writer(|connection| {
            connection.execute_batch("CREATE TABLE work(value TEXT);")?;
            Ok(())
        })
        .unwrap();
    let failure: crate::Result<()> =
        database.transaction(TransactionBehavior::Immediate, |transaction| {
            transaction.execute("INSERT INTO work VALUES ('rolled back')", [])?;
            Err(Error::Sqlite(rusqlite::Error::InvalidQuery))
        });
    assert!(failure.is_err());
    database
        .read_transaction(|snapshot| {
            assert_eq!(
                snapshot.query_row("SELECT count(*) FROM work", [], |row| row.get::<_, i32>(0))?,
                0
            );
            std::thread::scope(|scope| {
                scope
                    .spawn(|| {
                        database.transaction(TransactionBehavior::Immediate, |transaction| {
                            transaction.execute("INSERT INTO work VALUES ('committed')", [])?;
                            Ok(())
                        })
                    })
                    .join()
                    .unwrap()
            })?;
            assert_eq!(
                snapshot.query_row("SELECT count(*) FROM work", [], |row| row.get::<_, i32>(0))?,
                0
            );
            Ok(())
        })
        .unwrap();
    database
        .with_reader(|connection| {
            assert_eq!(
                connection
                    .query_row("SELECT count(*) FROM work", [], |row| row.get::<_, i32>(0))?,
                1
            );
            assert_eq!(
                connection
                    .execute("INSERT INTO work VALUES ('forbidden')", [])
                    .unwrap_err()
                    .sqlite_error_code(),
                Some(rusqlite::ErrorCode::ReadOnly)
            );
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn async_store_closures_use_blocking_threads_and_multiple_readers() {
    let temporary = TempDir::new().unwrap();
    let database = Database::open(temporary.path().join("db.sqlite")).unwrap();
    database
        .writer(|connection| {
            connection.execute_batch("CREATE TABLE work(value TEXT);")?;
            Ok(())
        })
        .await
        .unwrap();
    let (reader_entered, entered) = tokio::sync::oneshot::channel();
    let release = Arc::new(Barrier::new(2));
    let hold = release.clone();
    let reader_db = database.clone();
    let reader = tokio::spawn(async move {
        reader_db
            .reader(move |connection| {
                reader_entered.send(()).unwrap();
                hold.wait();
                Ok(connection
                    .query_row("SELECT count(*) FROM work", [], |row| row.get::<_, i32>(0))?)
            })
            .await
            .unwrap()
    });
    entered.await.unwrap();
    database
        .writer(|connection| {
            connection.execute("INSERT INTO work VALUES ('saved')", [])?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        database
            .reader(|connection| Ok(connection.query_row(
                "SELECT count(*) FROM work",
                [],
                |row| row.get::<_, i32>(0)
            )?))
            .await
            .unwrap(),
        1
    );
    tokio::task::spawn_blocking(move || release.wait())
        .await
        .unwrap();
    assert_eq!(reader.await.unwrap(), 1);
}

#[test]
fn memory_database_routes_reads_to_writer() {
    let database = Database::open(Path::new(":memory:")).unwrap();
    database
        .with_reader(|connection| {
            assert_head_schema(connection);
            Ok(())
        })
        .unwrap();
}

#[test]
fn upgrades_head69_preserving_subagent_delivery_scheduled_targets_and_correlations() {
    let mut connection = Connection::open_in_memory().unwrap();
    seed_ts_head(&connection, 69, include_str!("../sql/head-69.sql"));
    for (id, delivery) in [
        ("pending", Some("pending")),
        ("delivered", Some("delivered")),
        ("absent", None),
    ] {
        let payload = delivery.map_or_else(
            || json!({}),
            |state| json!({"completionDelivery":{"state":state}}),
        );
        connection.execute("INSERT INTO orchestration_v2_projection_subagents(subagent_id,thread_id,parent_node_id,provider,origin,status,updated_at,payload_json) VALUES (?1,'thread','node','codex','delegated','completed','2026-09-01',?2)", (id, payload.to_string())).unwrap();
    }
    for (id, thread) in [("continue", Some("thread")), ("new", None)] {
        connection.execute("INSERT INTO scheduled_tasks(task_id,title,prompt,enabled,schedule_json,project_id,thread_id,workspace_strategy_json,model_selection_json,runtime_mode,interaction_mode,created_by,creation_source,created_at,updated_at,last_run_status,run_count) VALUES (?1,?1,'prompt',1,'{}','project',?2,'{}','{}','full-access','default','user','web','2026-09-01','2026-09-01','never',0)", (id, thread)).unwrap();
    }
    connection.execute_batch("INSERT INTO orchestration_events(event_id,aggregate_kind,stream_id,stream_version,event_type,occurred_at,correlation_id,actor_kind,payload_json,metadata_json) VALUES ('event','thread','thread',0,'thread.created','2026-09-01','correlation','client','{}','{}');").unwrap();
    migrations::run_migrations(&mut connection, None).unwrap();
    let deliveries: Vec<(String, Option<String>)> = connection.prepare("SELECT subagent_id,completion_delivery_state FROM orchestration_v2_projection_subagents ORDER BY subagent_id").unwrap().query_map([], |row| Ok((row.get(0)?,row.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    assert_eq!(
        deliveries,
        [
            ("absent".into(), None),
            ("delivered".into(), Some("delivered".into())),
            ("pending".into(), Some("pending".into()))
        ]
    );
    let modes: Vec<(String, String)> = connection
        .prepare("SELECT task_id,thread_mode FROM scheduled_tasks ORDER BY task_id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        modes,
        [
            ("continue".into(), "continue".into()),
            ("new".into(), "new_thread".into())
        ]
    );
    assert!(!migrations::table_exists(&connection, "idx_orch_events_correlation_id").unwrap());
    assert_eq!(
        connection
            .query_row(
                "SELECT correlation_id FROM orchestration_events WHERE event_id='event'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "correlation"
    );
    assert_head_schema(&connection);
}

#[test]
fn upgrades_database_without_optional_fts_index() {
    let mut connection = database_at(80);
    connection.execute_batch("DROP TRIGGER orchestration_v2_message_search_insert; DROP TRIGGER orchestration_v2_message_search_update; DROP TRIGGER orchestration_v2_message_search_delete; DROP TABLE orchestration_v2_message_search;").unwrap();
    insert_message(
        &connection,
        "quoted",
        json!(citation("Readable without FTS", None)),
    );
    migrations::run_migrations(&mut connection, None).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT search_text FROM orchestration_v2_projection_messages",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "Readable without FTS"
    );
    assert!(!migrations::table_exists(&connection, "orchestration_v2_message_search").unwrap());
}

#[test]
fn checkpoint_waits_for_idle_below_threshold_and_stops_retrying_pinned_readers() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("checkpoint.sqlite");
    let writer = Connection::open(&path).unwrap();
    writer
        .execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE work(value BLOB);",
        )
        .unwrap();
    writer
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    let reader = Connection::open(&path).unwrap();
    reader.execute_batch("BEGIN").unwrap();
    assert_eq!(
        reader
            .query_row("SELECT count(*) FROM work", [], |row| row.get::<_, i32>(0))
            .unwrap(),
        0
    );
    writer
        .execute_batch("INSERT INTO work VALUES (zeroblob(32));")
        .unwrap();
    assert!(crate::checkpoint::check(&writer, false).unwrap());
    assert!(!crate::checkpoint::check(&writer, true).unwrap());
    reader.execute_batch("COMMIT").unwrap();
    assert!(!crate::checkpoint::check(&writer, true).unwrap());
    writer
        .execute_batch("INSERT INTO work VALUES (zeroblob(5000000));")
        .unwrap();
    assert!(!crate::checkpoint::check(&writer, false).unwrap());
    assert_eq!(
        writer
            .query_row("PRAGMA wal_checkpoint(NOOP)", [], |row| Ok(row
                .get::<_, i64>(1)?
                - row.get::<_, i64>(2)?))
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn close_checkpoints_wal_after_readers_and_worker_stop() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("close.sqlite");
    let database = Database::open(&path).unwrap();
    database
        .with_writer(|connection| {
            connection.execute_batch(
                "CREATE TABLE close_work(value); INSERT INTO close_work VALUES ('saved');",
            )?;
            Ok(())
        })
        .unwrap();
    database.close().await.unwrap();
    let wal = path.with_extension("sqlite-wal");
    match fs::metadata(&wal) {
        Ok(metadata) => assert_eq!(metadata.len(), 0, "nonempty WAL after close"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("could not inspect WAL after close: {error}"),
    }
    let connection =
        Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT value FROM close_work", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "saved"
    );
}

#[test]
fn open_v2_refuses_unsupported_source_before_publication() {
    for head in [0, 1, 52, 53, 54, 68] {
        let temporary = TempDir::new().unwrap();
        let source_path = temporary.path().join("state.sqlite");
        let destination = temporary.path().join("statev2.sqlite");
        let source = Connection::open(&source_path).unwrap();
        source.execute_batch("CREATE TABLE effect_sql_migrations(migration_id INTEGER PRIMARY KEY, name TEXT); CREATE TABLE preserved(value); INSERT INTO preserved VALUES ('keep');").unwrap();
        if head == 1 {
            source
                .execute_batch("DROP TABLE effect_sql_migrations;")
                .unwrap();
        } else if head > 0 {
            source
                .execute(
                    "INSERT INTO effect_sql_migrations VALUES (?1, 'Legacy')",
                    [head],
                )
                .unwrap();
        }
        drop(source);
        let original = fs::read(&source_path).unwrap();
        let error = match Database::open_v2(&destination, OpenOptions::default()) {
            Ok(_) => panic!("opened unsupported head {head}"),
            Err(error) => error,
        };
        assert!(
            !destination.exists(),
            "published unsupported source at head {head}"
        );
        assert!(
            error
                .to_string()
                .contains("Open the previous app version once"),
            "{error}"
        );
        assert_eq!(fs::read(&source_path).unwrap(), original);
        assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 1);
    }
}

#[test]
fn writer_panic_does_not_disable_later_access() {
    let database = Database::open(":memory:").unwrap();
    database
        .with_writer(|connection| {
            connection.execute_batch("CREATE TABLE panic_work(value);")?;
            Ok(())
        })
        .unwrap();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: crate::Result<()> = database.with_writer(|connection| {
            connection.execute_batch("BEGIN; INSERT INTO panic_work VALUES ('discard');")?;
            panic!("caller panic");
        });
    }));
    assert!(panic.is_err());
    database
        .with_writer(|connection| {
            assert!(connection.is_autocommit());
            assert_eq!(
                connection.query_row("SELECT count(*) FROM panic_work", [], |row| row
                    .get::<_, i32>(0))?,
                0
            );
            connection.execute_batch("BEGIN; INSERT INTO panic_work VALUES ('keep'); COMMIT;")?;
            Ok(())
        })
        .unwrap();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let connection = database.0.writer.lock().unwrap();
        connection
            .execute_batch("BEGIN; INSERT INTO panic_work VALUES ('discard poisoned');")
            .unwrap();
        panic!("poisoned lock");
    }));
    assert!(panic.is_err());
    assert!(database.0.writer.is_poisoned());
    database
        .with_writer(|connection| {
            assert!(connection.is_autocommit());
            assert_eq!(
                connection.query_row("SELECT count(*) FROM panic_work", [], |row| row
                    .get::<_, i32>(0))?,
                1
            );
            Ok(())
        })
        .unwrap();
    assert!(!database.0.writer.is_poisoned());
}

#[test]
fn closures_return_connections_without_open_transactions() {
    for readers in [0, 1] {
        let temporary = TempDir::new().unwrap();
        let database = Database::open_with_options(
            temporary.path().join("clean.sqlite"),
            OpenOptions {
                readers,
                ..Default::default()
            },
        )
        .unwrap();
        for fail in [true, false] {
            let result: crate::Result<()> = database.with_writer(|connection| {
                connection.execute_batch("BEGIN;")?;
                if fail {
                    Err(Error::Sqlite(rusqlite::Error::InvalidQuery))
                } else {
                    Ok(())
                }
            });
            assert_eq!(result.is_err(), fail);
            database
                .with_writer(|connection| {
                    assert!(connection.is_autocommit(), "writer transaction leaked");
                    Ok(())
                })
                .unwrap();
            let result: crate::Result<()> = database.with_reader(|connection| {
                connection.execute_batch("BEGIN; SELECT count(*) FROM sqlite_schema;")?;
                if fail {
                    Err(Error::Sqlite(rusqlite::Error::InvalidQuery))
                } else {
                    Ok(())
                }
            });
            assert_eq!(result.is_err(), fail);
            database
                .with_reader(|connection| {
                    assert!(connection.is_autocommit(), "reader transaction leaked");
                    Ok(())
                })
                .unwrap();
        }
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _: crate::Result<()> = database.with_reader(|connection| {
                connection.execute_batch("BEGIN; SELECT count(*) FROM sqlite_schema;")?;
                panic!("reader panic");
            });
        }));
        assert!(panic.is_err());
        database
            .with_reader(|connection| {
                assert!(connection.is_autocommit());
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn nested_connection_access_is_rejected_before_acquiring_locks() {
    for (path, readers) in [
        (":memory:", 0),
        ("nested.sqlite", 0),
        ("nested.sqlite", 1),
        ("nested.sqlite", 2),
    ] {
        let temporary = TempDir::new().unwrap();
        let path = if path == ":memory:" {
            Path::new(path).to_owned()
        } else {
            temporary.path().join(path)
        };
        let database = Database::open_with_options(
            path,
            OpenOptions {
                readers,
                ..Default::default()
            },
        )
        .unwrap();
        for writer in [false, true] {
            let check = |_: &Connection| {
                assert!(matches!(
                    database.with_reader(|_| Ok(())),
                    Err(Error::NestedAccess)
                ));
                assert!(matches!(
                    database.with_writer(|_| Ok(())),
                    Err(Error::NestedAccess)
                ));
                assert!(matches!(
                    database.transaction(TransactionBehavior::Immediate, |_| Ok(())),
                    Err(Error::NestedAccess)
                ));
                assert!(matches!(
                    database.read_transaction(|_| Ok(())),
                    Err(Error::NestedAccess)
                ));
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap();
                assert!(matches!(
                    runtime.block_on(database.reader(|_| Ok(()))),
                    Err(Error::NestedAccess)
                ));
                assert!(matches!(
                    runtime.block_on(database.writer(|_| Ok(()))),
                    Err(Error::NestedAccess)
                ));
                Ok(())
            };
            if writer {
                database
                    .with_writer(|connection| check(connection))
                    .unwrap();
            } else {
                database.with_reader(check).unwrap();
            }
        }
        database.with_writer(|_| Ok(())).unwrap();
        database.with_reader(|_| Ok(())).unwrap();
    }
}

#[test]
fn bare_destination_resolves_import_cleanup_directory() {
    assert_eq!(
        crate::import::import_directory(Path::new("statev2.sqlite")),
        Path::new(".")
    );
}

#[test]
fn import_cleanup_failures_are_reported_and_do_not_prevent_open() {
    let temporary = TempDir::new().unwrap();
    let blocker = temporary.path().join("file");
    fs::write(&blocker, "not a directory").unwrap();
    let mut warnings = Vec::new();
    crate::import::cleanup_abandoned_copies(&blocker, |error| warnings.push(error.to_string()));
    assert_eq!(warnings.len(), 1);
    // Existing destination skips import even when its parent cannot be enumerated as a directory.
    initialize_v2_database(&blocker).unwrap();
}

#[test]
fn preview_history_error_is_actionable_without_conflicting_advice() {
    let mut connection = database_at(69);
    connection
        .execute_batch(
            "UPDATE effect_sql_migrations SET name='OrchestrationV2' WHERE migration_id=53",
        )
        .unwrap();
    let message = migrations::run_migrations(&mut connection, None)
        .unwrap_err()
        .to_string();
    assert!(
        !message.contains("history would skip required schema migrations: Cannot upgrade"),
        "{message}"
    );
    assert!(
        message.contains("Start the build that wrote it"),
        "{message}"
    );
}

#[test]
fn open_v2_imports_supported_sources_and_preserves_existing_destinations() {
    let temporary = TempDir::new().unwrap();
    let source_path = temporary.path().join("state.sqlite");
    let destination = temporary.path().join("statev2.sqlite");
    let source = Connection::open(&source_path).unwrap();
    seed_ts_head(&source, 80, include_str!("../sql/head-80.sql"));
    source.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE imported(value); INSERT INTO imported VALUES ('from source');").unwrap();
    let database = Database::open_v2(&destination, OpenOptions::default()).unwrap();
    database
        .with_reader(|connection| {
            assert_eq!(
                connection.query_row("SELECT value FROM imported", [], |row| row
                    .get::<_, String>(0))?,
                "from source"
            );
            assert_eq!(migrations::history(connection)?.last().unwrap().id, 89);
            Ok(())
        })
        .unwrap();
    assert_eq!(migrations::history(&source).unwrap().last().unwrap().id, 80);
    drop(database);
    drop(source);
    fs::write(source_path, "subsequently unusable V1").unwrap();
    drop(Database::open_v2(&destination, OpenOptions::default()).unwrap());
    let fresh = temporary.path().join("new").join("statev2.sqlite");
    drop(Database::open_v2(fresh, OpenOptions::default()).unwrap());
}

#[test]
fn fresh_schema_preserves_typescript_creation_order() {
    let expected = Connection::open_in_memory().unwrap();
    expected
        .execute_batch(include_str!("../sql/head-89.sql"))
        .unwrap();
    let names = |connection: &Connection| {
        connection.prepare("SELECT name FROM sqlite_schema WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' AND name NOT IN (SELECT name FROM pragma_table_list WHERE type='shadow') ORDER BY rowid").unwrap()
            .query_map([], |row| row.get::<_, String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
    };
    Database::open(":memory:")
        .unwrap()
        .with_reader(|connection| {
            assert_eq!(names(connection), names(&expected));
            Ok(())
        })
        .unwrap();
}

#[test]
fn checkpoint_worker_handles_notifications_idle_pinned_readers_and_shutdown() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("worker.sqlite");
    let writer = Connection::open(&path).unwrap();
    writer
        .execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE work(value);",
        )
        .unwrap();
    writer
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    let reader =
        Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    reader
        .execute_batch("BEGIN; SELECT count(*) FROM work;")
        .unwrap();
    let (worker, checks) = crate::checkpoint::CheckpointWorker::start_observed(&path).unwrap();
    writer
        .execute_batch("INSERT INTO work VALUES ('pinned');")
        .unwrap();
    worker.notify();
    assert_eq!(checks.recv().unwrap(), (false, true));
    assert_eq!(checks.recv().unwrap(), (true, false));
    reader.execute_batch("COMMIT;").unwrap();
    writer
        .execute_batch("INSERT INTO work VALUES (zeroblob(5000000));")
        .unwrap();
    worker.notify();
    assert_eq!(checks.recv().unwrap(), (false, false));
    writer
        .execute_batch("INSERT INTO work VALUES ('idle');")
        .unwrap();
    worker.notify();
    assert_eq!(checks.recv().unwrap(), (false, true));
    assert_eq!(checks.recv().unwrap(), (true, false));
    assert!(worker.warning().is_none());
    drop(worker);
    assert!(
        checks.recv().is_err(),
        "worker did not close its observation channel"
    );
    // An unnotified worker must also close without waiting for its first request.
    drop(crate::checkpoint::CheckpointWorker::start(&path).unwrap());
}

#[test]
fn incomplete_backup_has_a_specific_error_instead_of_invalid_query() {
    let source = Connection::open_in_memory().unwrap();
    source
        .execute_batch("CREATE TABLE data(value); INSERT INTO data VALUES (zeroblob(100000));")
        .unwrap();
    let mut copy = Connection::open_in_memory().unwrap();
    let backup = rusqlite::backup::Backup::new(&source, &mut copy).unwrap();
    let step = backup.step(1).unwrap();
    assert!(matches!(step, rusqlite::backup::StepResult::More));
    assert!(matches!(
        crate::import::finish_backup(step),
        Err(Error::IncompleteBackup)
    ));
}
