#![cfg(feature = "sqlite")]
mod support;
use serde_json::json;
use support::decode;
use t3_persistence::Database;
use t3_pull_request::{
    linked::{Records, ViewedMark, ViewedScope},
    sqlite::SqliteRecords,
};

#[tokio::test]
async fn sqlite_viewed_marks_are_scoped_transactional_and_survive_reopening() {
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("state.sqlite");
    let database = Database::open(&path).expect("database");
    let records = SqliteRecords::new(database.clone());
    let scope = ViewedScope {
        provider: "gitlab".into(),
        host: "gitlab.com".into(),
        repository: "gitlab.com/owner/repo".into(),
        number: 42,
        viewer: "reader".into(),
    };
    let path_text = "x'); DROP TABLE pull_request_files_viewed; --";
    records
        .set_viewed(
            &scope,
            vec![(
                ViewedMark {
                    path: path_text.into(),
                    revision: Some("shown".into()),
                },
                true,
            )],
            "2026-01-01T00:00:00.000Z".into(),
        )
        .await
        .expect("mark");
    let mut other = scope.clone();
    other.viewer = "other".into();
    assert!(records.viewed(&other).await.expect("other").0.is_empty());
    drop(records);
    database.close().await.expect("close");
    let database = Database::open(&path).expect("reopen");
    let records = SqliteRecords::new(database.clone());
    let marks = records.viewed(&scope).await.expect("marks");
    assert_eq!(marks.0[0].path, path_text);
    assert_eq!(marks.0[0].revision.as_deref(), Some("shown"));
    records
        .set_viewed(
            &scope,
            vec![(
                ViewedMark {
                    path: path_text.into(),
                    revision: None,
                },
                false,
            )],
            "2026-01-02T00:00:00.000Z".into(),
        )
        .await
        .expect("unmark");
    assert!(records.viewed(&scope).await.expect("empty").0.is_empty());
    drop(records);
    database.close().await.expect("close");
}

#[tokio::test]
async fn sqlite_guides_roundtrip_wire_contract_and_replace_atomically() {
    let dir = tempfile::tempdir().expect("temp");
    let database = Database::open(dir.path().join("state.sqlite")).expect("database");
    let records = SqliteRecords::new(database.clone());
    let key = decode(json!({"host":"github.com","repository":"owner/repo","number":42}));
    assert!(records.guide(&key).await.expect("none").is_none());
    let guide = decode(
        json!({"headSha":"abc1234","generatedAt":"2026-01-01T00:00:00.000Z","modelSelection":{"instanceId":"codex","model":"test"},"stops":[]}),
    );
    records.set_guide(&key, guide).await.expect("write");
    assert_eq!(
        records
            .guide(&key)
            .await
            .expect("read")
            .expect("guide")
            .head_sha
            .as_str(),
        "abc1234"
    );
    drop(records);
    database.close().await.expect("close");
}

#[tokio::test]
async fn sqlite_rejects_out_of_range_native_pr_numbers() {
    let database = Database::open(":memory:").expect("database");
    let records = SqliteRecords::new(database.clone());
    let scope = ViewedScope {
        provider: "github".into(),
        host: "github.com".into(),
        repository: "owner/repo".into(),
        number: u64::MAX,
        viewer: "viewer".into(),
    };
    assert!(records.viewed(&scope).await.is_err());
    drop(records);
    database.close().await.expect("close");
}

#[tokio::test]
async fn linked_thread_query_keeps_archived_threads_orders_activity_and_excludes_hidden_links() {
    let database = Database::open(":memory:").expect("database");
    database.writer(|c|{
        for (id,host,repository,source,side_chat,archived,deleted,settled,updated) in [
            ("a","GITHUB.COM","Owner/Repo","manual",false,false,false,false,"2026-01-01T00:00:00.000Z"),
            ("b","github.com","owner/repo","agent",false,true,false,true,"2026-01-02T00:00:00.000Z"),
            ("side","github.com","owner/repo","agent",true,false,false,false,"2026-01-03T00:00:00.000Z"),
            ("gone","github.com","owner/repo","agent",false,false,true,false,"2026-01-03T00:00:00.000Z"),
            ("dismissed","github.com","owner/repo","stack-dismissed",false,false,false,false,"2026-01-03T00:00:00.000Z"),
            ("other","gitlab.com","owner/repo","agent",false,false,false,false,"2026-01-03T00:00:00.000Z"),
            ("other-repo","github.com","owner/other","agent",false,false,false,false,"2026-01-03T00:00:00.000Z"),
        ]{
            let payload=json!({"settledAt":null,"settledOverride":if settled{"settled"}else{"active"},"lineage":{"sideChat":side_chat},"pullRequests":[{"host":host,"repository":repository,"number":42,"url":"https://github.com/owner/repo/pull/42","source":source}]});
            c.execute("INSERT INTO orchestration_v2_projection_threads(thread_id,project_id,title,default_provider,runtime_mode,interaction_mode,created_at,updated_at,payload_json,archived_at,deleted_at) VALUES (?1,'project',?1,'codex','full-access','default',?2,?2,?3,?4,?5)",t3_persistence::rusqlite::params![id,updated,payload.to_string(),archived.then_some(updated),deleted.then_some(updated)])?;
        }Ok(())
    }).await.expect("rows");
    let records = SqliteRecords::new(database.clone());
    let result = records
        .linked_threads(&decode(
            json!({"host":"github.com","repository":"owner/repo","number":42}),
        ))
        .await
        .expect("linked");
    assert_eq!(
        result
            .threads
            .iter()
            .map(|t| t.id.0.as_str())
            .collect::<Vec<_>>(),
        ["b", "a"]
    );
    assert_eq!(result.threads[0].settled, Some(true));
    assert!(result.threads[0].archived_at.is_some());
    drop(records);
    database.close().await.expect("close");
}
