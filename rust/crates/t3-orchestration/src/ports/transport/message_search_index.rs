use crate::error::Result;
use t3_persistence::rusqlite::Connection;

const BOOTSTRAP: &str = "[User attached one or more files without additional text. Respond using the conversation context and the attached files.]";
const WHITESPACE: &str = "\t\n\u{000b}\u{000c}\r \u{a0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}";
pub fn non_placeholder_message_condition(row: &str) -> String {
    format!(
        "CASE WHEN substr(json_extract({row}.payload_json,'$.text'),1,11)='Ultrathink:' COLLATE NOCASE THEN ltrim(substr(json_extract({row}.payload_json,'$.text'),12),'{WHITESPACE}') ELSE json_extract({row}.payload_json,'$.text') END COLLATE BINARY <> '{BOOTSTRAP}'"
    )
}
pub fn indexed_message_condition(row: &str) -> String {
    format!(
        "{row}.streaming=0 AND ({row}.role='assistant' OR ({row}.role='user' AND COALESCE(json_extract({row}.payload_json,'$.createdBy'),'user')='user')) AND NOT EXISTS(SELECT 1 FROM orchestration_v2_projection_threads AS subagent_threads WHERE subagent_threads.thread_id={row}.thread_id AND json_extract(subagent_threads.payload_json,'$.lineage.relationshipToParent')='subagent') AND {}",
        non_placeholder_message_condition(row)
    )
}
pub fn message_search_text(row: &str) -> String {
    format!("COALESCE({row}.search_text,json_extract({row}.payload_json,'$.text'))")
}
pub fn indexed_message_text(payload: &str) -> String {
    fold_apostrophes(&format!("json_extract({payload},'$.text')"))
}
fn fold_apostrophes(text: &str) -> String {
    format!("replace(replace({text},'’',''''),'‘','''')")
}
pub fn readable_search_text(text: &str, plain: impl FnOnce(&str) -> String) -> Option<String> {
    let readable = plain(text);
    (readable != text).then_some(readable)
}
fn has_index(db: &Connection) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='orchestration_v2_message_search')",
        [],
        |row| row.get(0),
    )?)
}
fn indexed_text(db: &Connection, row: &str) -> Result<String> {
    let has_column:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('orchestration_v2_projection_messages') WHERE name='search_text')",[],|row|row.get(0))?;
    Ok(if has_column {
        fold_apostrophes(&message_search_text(row))
    } else {
        indexed_message_text(&format!("{row}.payload_json"))
    })
}
pub fn create_message_search_triggers(db: &Connection) -> Result<()> {
    let new = indexed_text(db, "new")?;
    let old = indexed_text(db, "old")?;
    let condition = indexed_message_condition("new");
    db.execute_batch(&format!("CREATE TRIGGER orchestration_v2_message_search_insert AFTER INSERT ON orchestration_v2_projection_messages WHEN {condition} BEGIN INSERT INTO orchestration_v2_message_search(rowid,text) SELECT new.rowid,{new} WHERE json_extract(new.payload_json,'$.text') IS NOT NULL; END;
        CREATE TRIGGER orchestration_v2_message_search_update AFTER UPDATE ON orchestration_v2_projection_messages WHEN (old.streaming=0 OR new.streaming=0) AND (old.streaming IS NOT new.streaming OR old.role IS NOT new.role OR {old} IS NOT {new} OR json_extract(old.payload_json,'$.createdBy') IS NOT json_extract(new.payload_json,'$.createdBy')) BEGIN DELETE FROM orchestration_v2_message_search WHERE rowid=old.rowid AND old.streaming=0; INSERT INTO orchestration_v2_message_search(rowid,text) SELECT new.rowid,{new} WHERE {condition} AND json_extract(new.payload_json,'$.text') IS NOT NULL; END;
        CREATE TRIGGER orchestration_v2_message_search_delete AFTER DELETE ON orchestration_v2_projection_messages WHEN old.streaming=0 BEGIN DELETE FROM orchestration_v2_message_search WHERE rowid=old.rowid; END;"))?;
    Ok(())
}
pub fn recreate_message_search_triggers(db: &Connection) -> Result<()> {
    if !has_index(db)? {
        return Ok(());
    }
    db.execute_batch("DROP TRIGGER orchestration_v2_message_search_insert; DROP TRIGGER orchestration_v2_message_search_update; DROP TRIGGER orchestration_v2_message_search_delete;")?;
    create_message_search_triggers(db)
}
/// Call inside the caller's writer transaction; clearing and refilling are atomic.
pub fn rebuild_message_search(tx: &t3_persistence::rusqlite::Transaction<'_>) -> Result<()> {
    if !has_index(tx)? {
        return Ok(());
    }
    let text = indexed_text(tx, "messages")?;
    tx.execute("INSERT INTO orchestration_v2_message_search(orchestration_v2_message_search) VALUES('delete-all')",[])?;
    tx.execute(&format!("INSERT INTO orchestration_v2_message_search(rowid,text) SELECT messages.rowid,{text} FROM orchestration_v2_projection_messages AS messages WHERE {} AND json_extract(messages.payload_json,'$.text') IS NOT NULL",indexed_message_condition("messages")),[])?;
    Ok(())
}
