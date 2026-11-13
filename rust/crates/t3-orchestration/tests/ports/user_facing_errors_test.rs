use serde_json::json;
use t3_orchestration::{ports::decisions::user_facing_errors::*, testing::TestDatabase};

#[tokio::test]
async fn real_database_open_failure_is_explained_without_exposing_the_path() {
    let directory = tempfile::tempdir().unwrap();
    let error = t3_persistence::rusqlite::Connection::open(
        directory.path().join("missing").join("state.sqlite"),
    )
    .err()
    .unwrap();
    let message = user_facing_database_error_message(&error).unwrap();
    assert!(message.contains("server could not open a database file (SQLITE_CANTOPEN)"));
    assert!(message.contains("machine running the server"));
    assert!(message.contains("free disk space"));
    assert!(message.contains("data and temporary directories exist and are writable"));
    assert!(message.contains("then retry"));
    assert!(!message.contains(directory.path().to_str().unwrap()));
}

#[tokio::test]
async fn full_database_explains_capacity_and_can_retry_after_capacity_is_restored() {
    let database = TestDatabase::memory().unwrap();
    database
        .stores
        .transaction(|tx| {
            tx.execute_batch("CREATE TABLE error_test_entries(payload BLOB)")?;
            let count: i64 = tx.query_row("PRAGMA page_count", [], |row| row.get(0))?;
            tx.pragma_update(None, "max_page_count", count + 1)?;
            Ok(())
        })
        .await
        .unwrap();
    let error = database
        .stores
        .transaction(|tx| {
            tx.execute(
                "INSERT INTO error_test_entries VALUES (zeroblob(16777216))",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap_err();
    let message = user_facing_database_error_message(&error).unwrap();
    assert!(message.contains("database or disk is full (SQLITE_FULL)"));
    assert!(message.contains("Free space on the machine running the server, then retry"));
    assert!(message.contains("database size limit"));
    database.stores.transaction(|tx|{tx.execute_batch("PRAGMA max_page_count=100000; INSERT INTO error_test_entries VALUES (zeroblob(16384))")?;Ok(())}).await.unwrap();
    let rows = database
        .stores
        .read(|db| {
            Ok(
                db.query_row("SELECT count(*) FROM error_test_entries", [], |row| {
                    row.get::<_, i64>(0)
                })?,
            )
        })
        .await
        .unwrap();
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn read_only_database_uses_permission_guidance_and_hides_private_query_data() {
    let database = TestDatabase::memory().unwrap();
    database
        .stores
        .transaction(|tx| {
            tx.execute_batch("CREATE TABLE error_test_entries(payload TEXT)")?;
            Ok(())
        })
        .await
        .unwrap();
    let error = database
        .stores
        .transaction(|tx| {
            tx.execute_batch(
                "PRAGMA query_only=ON; INSERT INTO error_test_entries VALUES ('private-message')",
            )?;
            Ok(())
        })
        .await
        .unwrap_err();
    let message = user_facing_database_error_message(&error).unwrap();
    assert!(message.contains("server database is read-only (SQLITE_READONLY)"));
    assert!(message.contains("data directory is writable"));
    assert!(!message.contains("private-message"));
}

#[test]
fn unknown_sql_errors_are_contextual_and_nested_dispatch_messages_are_deduplicated() {
    let cause = json!({"message":"Failed to dispatch orchestration command message.dispatch (command-1).","cause":{"_tag":"SqlError","cause":{"message":"private-query-data","errcode":1}}});
    let message = user_facing_dispatch_error_message(&cause).unwrap();
    assert!(message.contains("server could not complete this database request"));
    assert!(message.contains("server logs"));
    assert!(!message.contains("private-query-data"));
    assert_eq!(
        user_facing_dispatch_error_message(&json!({"message":"wrapper","errors":["a","b","a"]})),
        Some("b".into())
    );
    assert_eq!(
        sqlite_recovery_message(Some(14 | (1 << 8))),
        sqlite_recovery_message(Some(14))
    );
    assert_eq!(
        capability_rejection_message("customAgent", "interrupt"),
        Some("Custom cannot stop a run once it has started.".into())
    );
    assert_eq!(
        user_facing_dispatch_error_message(
            &json!({"_tag":"SqlError","cause":{"errcode":4294967304_u64}})
        ),
        Some(sqlite_recovery_message(Some(8)).into())
    );
    assert_eq!(
        user_facing_dispatch_error_message(&json!("\u{feff} database unavailable \u{feff}")),
        Some("database unavailable".into())
    );
    use t3_orchestration::ports::decisions::unknown_field::field;
    assert_eq!(
        field(&json!(["first", "second"]), "1"),
        Some(json!("second"))
    );
    assert_eq!(field(&json!(["first", "second"]), "length"), Some(json!(2)));
    assert_eq!(field(&json!(["first", "second"]), "01"), None);
}
