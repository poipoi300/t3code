//! Connection-local reverse lookup for attachment previews. Projection writes use the same
//! SQLite writer. TEMP triggers invalidate changed threads within their projection transaction.
//! An external commit or a new writer connection rebuilds the disposable index.
use t3_orchestration::{api::ThreadId, error::Result};
use t3_persistence::rusqlite::{Connection, OptionalExtension};

const RECORDS_FOR_THREAD: &str = "
    SELECT json_extract(a.value, '$.id') AS id
    FROM orchestration_v2_projection_messages m, json_each(m.payload_json, '$.attachments') a
    WHERE m.thread_id = ?1
    UNION ALL
    SELECT json_extract(a.value, '$.id')
    FROM orchestration_v2_projection_threads t,
         json_each(t.payload_json, '$.guestShare.messages') m,
         json_each(m.value, '$.attachments') a
    WHERE t.thread_id = ?1 AND json_extract(m.value, '$.status') = 'pending'
    UNION ALL
    SELECT json_extract(a.value, '$.id')
    FROM orchestration_v2_projection_turn_items i,
         json_each(i.payload_json, '$.questionAnswer.attachmentsByQuestionId') q,
         json_each(q.value) a
    WHERE i.thread_id = ?1 AND i.type = 'user_input_request'";

const TABLES: &str = "
    CREATE TEMP TABLE preview_index_version(version INTEGER NOT NULL);
    CREATE TEMP TABLE preview_dirty(thread_id TEXT PRIMARY KEY) WITHOUT ROWID;
    CREATE TEMP TABLE preview_threads(thread_id TEXT PRIMARY KEY, segment TEXT) WITHOUT ROWID;
    CREATE INDEX preview_by_segment ON preview_threads(segment);
    CREATE TEMP TABLE preview_members(
        thread_id TEXT, id TEXT NOT NULL, PRIMARY KEY(thread_id, id)
    ) WITHOUT ROWID;
    CREATE INDEX preview_by_attachment ON preview_members(id, thread_id);";

fn initialize(db: &Connection) -> Result<()> {
    let initialized = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_temp_master WHERE name='preview_index_version')",
        [],
        |r| r.get::<_, bool>(0),
    )?;
    if !initialized {
        db.execute_batch(TABLES)?;
        for (table, field) in [
            ("messages", "$.attachments"),
            ("threads", "$.guestShare.messages"),
            ("turn_items", "$.questionAnswer.attachmentsByQuestionId"),
        ] {
            let projection = format!("main.orchestration_v2_projection_{table}");
            for (event, record) in [("INSERT", "NEW"), ("DELETE", "OLD")] {
                let when = if table == "threads" {
                    String::new()
                } else {
                    format!(
                        "WHEN json_extract({record}.payload_json, '{field}') NOT IN ('[]', '{{}}')"
                    )
                };
                db.execute_batch(&format!(
                    "CREATE TEMP TRIGGER preview_{table}_{event} AFTER {event} ON {projection}
                     {when} BEGIN
                         INSERT INTO preview_dirty VALUES({record}.thread_id) ON CONFLICT DO NOTHING;
                     END;"
                ))?;
            }
            db.execute_batch(&format!(
                "CREATE TEMP TRIGGER preview_{table}_update AFTER UPDATE ON {projection}
                 WHEN OLD.thread_id IS NOT NEW.thread_id
                   OR json_extract(OLD.payload_json, '{field}') IS NOT json_extract(NEW.payload_json, '{field}')
                 BEGIN
                     INSERT INTO preview_dirty VALUES(OLD.thread_id) ON CONFLICT DO NOTHING;
                     INSERT INTO preview_dirty VALUES(NEW.thread_id) ON CONFLICT DO NOTHING;
                 END;"
            ))?;
        }
    }
    let version: i64 = db.query_row("PRAGMA main.data_version", [], |r| r.get(0))?;
    let known = db
        .query_row("SELECT version FROM preview_index_version", [], |r| {
            r.get::<_, i64>(0)
        })
        .optional()?;
    if known != Some(version) {
        db.execute_batch("DELETE FROM preview_members; DELETE FROM preview_threads;
            DELETE FROM preview_dirty; INSERT INTO preview_dirty SELECT thread_id FROM orchestration_v2_projection_threads;
            DELETE FROM preview_index_version;")?;
        db.execute("INSERT INTO preview_index_version VALUES(?1)", [version])?;
    }
    Ok(())
}

fn refresh(db: &Connection) -> Result<()> {
    initialize(db)?;
    let mut dirty = db.prepare("SELECT thread_id FROM preview_dirty")?;
    let mut read = db.prepare(RECORDS_FOR_THREAD)?;
    let mut add = db.prepare("INSERT OR IGNORE INTO preview_members VALUES(?1,?2)")?;
    for thread in dirty.query_map([], |r| r.get::<_, String>(0))? {
        let thread = thread?;
        db.execute("DELETE FROM preview_members WHERE thread_id=?1", [&thread])?;
        db.execute("DELETE FROM preview_threads WHERE thread_id=?1", [&thread])?;
        db.execute("INSERT INTO preview_threads SELECT thread_id,?2 FROM orchestration_v2_projection_threads WHERE thread_id=?1",
            (&thread,t3_workspace::attachments::safe_thread_segment(&thread)))?;
        for id in read.query_map([&thread], |r| r.get::<_, Option<String>>(0))? {
            if let Some(id) = id? {
                add.execute([&thread, &id])?;
            }
        }
    }
    db.execute("DELETE FROM preview_dirty", [])?;
    Ok(())
}

pub(super) fn readable(db: &Connection, id: &str, segment: &str) -> Result<bool> {
    refresh(db)?;
    // The index supplies candidates. Recheck current stored membership before granting
    // access, including when SQLite REPLACE removes an older row in a different thread.
    let mut sources = db.prepare(
        "SELECT m.thread_id FROM preview_members m
         JOIN orchestration_v2_projection_threads t USING(thread_id)
         WHERE m.id = ?1 AND t.deleted_at IS NULL",
    )?;
    let sql = format!("SELECT EXISTS(SELECT 1 FROM ({RECORDS_FOR_THREAD}) WHERE id=?2)");
    let mut recorded = db.prepare(&sql)?;
    for source in sources.query_map([id], |r| r.get::<_, String>(0))? {
        if recorded.query_row((source?, id), |r| r.get::<_, bool>(0))? {
            return Ok(true);
        }
    }
    let mut candidates = db.prepare(
        "SELECT s.thread_id FROM preview_threads s
         JOIN orchestration_v2_projection_threads t USING(thread_id)
         WHERE s.segment = ?1 AND t.deleted_at IS NULL AND EXISTS(
             SELECT 1 FROM orchestration_v2_projection_turn_items i
             WHERE i.thread_id = s.thread_id AND i.type = 'dynamic_tool'
         )",
    )?;
    for thread in candidates.query_map([segment], |r| r.get::<_, String>(0))? {
        if t3_orchestration::projections::read::attachment_ids(db, &ThreadId(thread?))?
            .iter()
            .any(|found| found == id)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod attachment_review_tests {
    use super::*;
    #[test]
    fn preview_membership_uses_thread_indexes() {
        let db = t3_persistence::rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE orchestration_v2_projection_threads(thread_id TEXT PRIMARY KEY, deleted_at TEXT, payload_json TEXT);
            CREATE TABLE orchestration_v2_projection_messages(thread_id TEXT, payload_json TEXT);
            CREATE INDEX messages_by_thread ON orchestration_v2_projection_messages(thread_id);
            CREATE TABLE orchestration_v2_projection_turn_items(thread_id TEXT, type TEXT, payload_json TEXT);
            CREATE INDEX items_by_thread ON orchestration_v2_projection_turn_items(thread_id);").unwrap();
        db.execute_batch("INSERT INTO orchestration_v2_projection_threads VALUES('origin',NULL,'{}'),('shared',NULL,'{}');
                INSERT INTO orchestration_v2_projection_messages VALUES('shared','{\"attachments\":[{\"id\":\"origin-image\"}]}');").unwrap();
        assert!(readable(&db, "origin-image", "origin").unwrap());
        db.execute(
            "DELETE FROM orchestration_v2_projection_threads WHERE thread_id='origin'",
            [],
        )
        .unwrap();
        assert!(readable(&db, "origin-image", "origin").unwrap());
        db.execute(
            "UPDATE orchestration_v2_projection_messages SET payload_json='{}'",
            [],
        )
        .unwrap();
        assert!(!readable(&db, "origin-image", "origin").unwrap());
        let mut plan = db
            .prepare(&format!("EXPLAIN QUERY PLAN {RECORDS_FOR_THREAD}"))
            .unwrap();
        let args = vec!["fixture"; plan.parameter_count()];
        let rows: Vec<String> = plan
            .query_map(t3_persistence::rusqlite::params_from_iter(args), |r| {
                r.get(3)
            })
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert!(
            rows.iter()
                .any(|r| r.contains("SEARCH m USING INDEX messages_by_thread"))
                && rows
                    .iter()
                    .any(|r| r.contains("SEARCH i USING INDEX items_by_thread")),
            "{rows:?}"
        );
    }
}
