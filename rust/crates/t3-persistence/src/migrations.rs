use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

pub use crate::manifest::MANIFEST;
use crate::{Error, Result, citations::readable_search_text};

pub const MINIMUM_SUPPORTED_HEAD: u32 = 69;
pub const LATEST_HEAD: u32 = 91;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Migration {
    pub id: u32,
    pub name: String,
}

pub fn history(connection: &Connection) -> Result<Vec<Migration>> {
    if !table_exists(connection, "effect_sql_migrations")? {
        return Ok(Vec::new());
    }
    let mut statement = connection
        .prepare("SELECT migration_id, name FROM effect_sql_migrations ORDER BY migration_id")?;
    Ok(statement
        .query_map([], |row| {
            Ok(Migration {
                id: row.get(0)?,
                name: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn has_pending_migrations(connection: &Connection) -> Result<bool> {
    let rows = history(connection)?;
    Ok(rows.last().map_or(0, |row| row.id) < LATEST_HEAD
        || rows
            .iter()
            .any(|row| matches!(row.id, 53 | 54) && row.name == "OrchestrationV2"))
}

type LogSink = fn(&str, &[(&str, String)]);
static LOG_SINK: std::sync::OnceLock<LogSink> = std::sync::OnceLock::new();

/// Where the three migration lines below go. The server installs one before it opens the
/// database, because its tracing subscriber exists only after that, and the desktop reads these
/// lines from the server's output. Without a sink nothing is logged (repair tooling, tests).
pub fn set_log_sink(sink: LogSink) {
    let _ = LOG_SINK.set(sink);
}

fn log(message: &str, fields: &[(&str, String)]) {
    if let Some(sink) = LOG_SINK.get() {
        sink(message, fields);
    }
}

/// The lines the desktop reads from the server's output to tell a database that is still
/// migrating from a server that never started (`databaseMigrationLog.ts`). Keep the text.
pub const MIGRATIONS_STARTED_MESSAGE: &str = "Running database migrations";
pub const MIGRATION_PROGRESS_MESSAGE: &str = "Upgrading database";
pub const MIGRATIONS_FINISHED_MESSAGE: &str = "Migrations ran successfully";

/// Runs only genuine incremental steps on existing databases. `through` supports repair tooling.
pub fn run_migrations(connection: &mut Connection, through: Option<u32>) -> Result<Vec<Migration>> {
    let through = through.unwrap_or(LATEST_HEAD);
    // The desktop waits past its readiness budget until the finished line follows this one.
    let announced = has_pending_migrations(connection)?;
    if announced {
        log(MIGRATIONS_STARTED_MESSAGE, &[]);
    }
    let initial = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let rows = history(&initial)?;
    let mut head = rows.last().map_or(0, |row| row.id);
    let empty = rows.is_empty() && initial.query_row(
        "SELECT count(*) = 0 FROM sqlite_schema WHERE type = 'table' AND name <> 'effect_sql_migrations'",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if head == 0 && !empty {
        return Err(Error::MissingLedger);
    }
    if (!empty && head < MINIMUM_SUPPORTED_HEAD) || (empty && through < MINIMUM_SUPPORTED_HEAD) {
        return Err(Error::UnsupportedHead {
            head,
            minimum: MINIMUM_SUPPORTED_HEAD,
        });
    }
    if through >= 55
        && rows
            .iter()
            .any(|row| matches!(row.id, 53 | 54) && row.name == "OrchestrationV2")
    {
        return Err(Error::MigrationHistory {
            details: "V2 preview has unexpected later migrations".into(),
        });
    }
    let divergent: Vec<_> = MANIFEST
        .iter()
        .filter_map(|&(id, name)| {
            if id < 55 || id > head || id > through {
                return None;
            }
            let recorded = rows.iter().find(|row| row.id == id);
            if recorded.is_some_and(|row| row.name == name) {
                return None;
            }
            Some(format!(
                "{id}_{name} (recorded: {})",
                recorded.map_or("nothing", |row| row.name.as_str())
            ))
        })
        .collect();
    if !divergent.is_empty() {
        return Err(Error::MigrationHistory {
            details: divergent.join(", "),
        });
    }
    let mut executed = Vec::new();
    if empty {
        // Only an empty database uses this TS-generated baseline; upgrades never jump schemas.
        initial.execute_batch("DROP TABLE IF EXISTS effect_sql_migrations;")?;
        initial.execute_batch(include_str!("../sql/head-69.sql"))?;
        for &(id, name) in MANIFEST
            .iter()
            .take_while(|&&(id, _)| id <= MINIMUM_SUPPORTED_HEAD)
        {
            initial.execute(
                "INSERT INTO effect_sql_migrations(migration_id, name) VALUES (?1, ?2)",
                (id, name),
            )?;
            executed.push(Migration {
                id,
                name: name.to_owned(),
            });
        }
        head = MINIMUM_SUPPORTED_HEAD;
    }
    initial.commit()?;
    // A database that already had a ledger reports each step; a new one runs its baseline above.
    let report_progress = !empty;
    let pending: Vec<_> = MANIFEST
        .iter()
        .filter(|&&(id, _)| id > head && id <= through)
        .collect();
    for (index, &&(id, name)) in pending.iter().enumerate() {
        if report_progress {
            log(
                MIGRATION_PROGRESS_MESSAGE,
                &[
                    ("migration", format!("{id}_{name}")),
                    ("progress", format!("{} of {}", index + 1, pending.len())),
                ],
            );
        }
        let result = (|| -> Result<bool> {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // Another process can finish this step while we wait for the write lock.
            let latest = transaction.query_row(
                "SELECT COALESCE(MAX(migration_id),0) FROM effect_sql_migrations",
                [],
                |row| row.get::<_, u32>(0),
            )?;
            if latest >= id {
                transaction.commit()?;
                return Ok(false);
            }
            transaction.execute(
                "INSERT INTO effect_sql_migrations(migration_id, name) VALUES (?1, ?2)",
                (id, name),
            )?;
            apply(&transaction, id)?;
            transaction.commit()?;
            Ok(true)
        })();
        let applied = result.map_err(|cause| Error::Migration {
            id,
            name,
            cause: Box::new(cause),
        })?;
        if applied {
            executed.push(Migration {
                id,
                name: name.to_owned(),
            });
        }
    }
    // A started line is always closed, also when another process applied the steps first.
    if announced || !executed.is_empty() {
        let names: Vec<String> = executed
            .iter()
            .map(|migration| format!("{}_{}", migration.id, migration.name))
            .collect();
        log(
            MIGRATIONS_FINISHED_MESSAGE,
            &[("migrations", names.join(","))],
        );
    }
    Ok(executed)
}

pub(crate) fn table_exists(connection: &Connection, name: &str) -> Result<bool> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE name = ?1",
            [name],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn has_column(connection: &Connection, table: &str, column: &str) -> Result<bool> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2",
            (table, column),
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn replace_search_triggers(connection: &Connection, readable: bool) -> Result<()> {
    if !table_exists(connection, "orchestration_v2_message_search")? {
        return Ok(());
    }
    connection.execute_batch("DROP TRIGGER orchestration_v2_message_search_insert; DROP TRIGGER orchestration_v2_message_search_update; DROP TRIGGER orchestration_v2_message_search_delete;")?;
    connection.execute_batch(if readable {
        include_str!("../sql/search-triggers.sql")
    } else {
        include_str!("../sql/search-triggers-before-81.sql")
    })?;
    Ok(())
}

fn apply(connection: &Connection, id: u32) -> Result<()> {
    let sql = match id {
        70 => include_str!("../sql/070.sql"),
        71 if !has_column(connection, "peer_environment_links", "fenced")? => {
            include_str!("../sql/071.sql")
        }
        71 => "",
        72 => include_str!("../sql/072.sql"),
        73 => include_str!("../sql/073.sql"),
        74 => return replace_search_triggers(connection, false),
        75 => include_str!("../sql/075.sql"),
        76 => include_str!("../sql/076.sql"),
        77 => include_str!("../sql/077.sql"),
        78 => include_str!("../sql/078.sql"),
        79 => include_str!("../sql/079.sql"),
        80 => include_str!("../sql/080.sql"),
        81 => return migrate_search_text(connection),
        82 => include_str!("../sql/082.sql"),
        83 => include_str!("../sql/083.sql"),
        84 => include_str!("../sql/084.sql"),
        85 => include_str!("../sql/085.sql"),
        86 => include_str!("../sql/086.sql"),
        87 => {
            if !has_column(connection, "projection_projects", "has_custom_title")? {
                connection.execute_batch("ALTER TABLE projection_projects ADD COLUMN has_custom_title INTEGER NOT NULL DEFAULT 0;")?;
            }
            include_str!("../sql/087.sql")
        }
        88 => include_str!("../sql/088.sql"),
        89 => include_str!("../sql/089.sql"),
        90 => {
            if !has_column(
                connection,
                "auth_pairing_links",
                "requires_connect_approval",
            )? {
                connection.execute_batch("ALTER TABLE auth_pairing_links ADD COLUMN requires_connect_approval INTEGER NOT NULL DEFAULT 0;")?;
            }
            if !has_column(connection, "auth_pairing_links", "requested_client_json")? {
                connection.execute_batch(
                    "ALTER TABLE auth_pairing_links ADD COLUMN requested_client_json TEXT;",
                )?;
            }
            if !has_column(connection, "auth_sessions", "proof_key_thumbprint")? {
                connection.execute_batch(
                    "ALTER TABLE auth_sessions ADD COLUMN proof_key_thumbprint TEXT;",
                )?;
            }
            include_str!("../sql/090.sql")
        }
        91 if !has_column(connection, "auth_pairing_links", "connect_denied")? => {
            "ALTER TABLE auth_pairing_links ADD COLUMN connect_denied INTEGER NOT NULL DEFAULT 0;"
        }
        91 => "",
        _ => unreachable!("manifest migration must have an implementation"),
    };
    connection.execute_batch(sql)?;
    Ok(())
}

fn migrate_search_text(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "ALTER TABLE orchestration_v2_projection_messages ADD COLUMN search_text TEXT;",
    )?;
    replace_search_triggers(connection, true)?;
    let mut after = String::new();
    loop {
        let mut statement = connection.prepare("SELECT message_id, json_extract(payload_json, '$.text') FROM orchestration_v2_projection_messages WHERE message_id > ?1 AND payload_json LIKE '%t3-citation://%' AND json_type(payload_json, '$.text') = 'text' ORDER BY message_id LIMIT 500")?;
        let rows: Vec<(String, String)> = statement
            .query_map([&after], |row| {
                // node:sqlite replaces invalid UTF-8 returned by JSON extraction of lone surrogates.
                let text = match row.get_ref(1)? {
                    rusqlite::types::ValueRef::Text(bytes) => {
                        String::from_utf8_lossy(bytes).into_owned()
                    }
                    _ => row.get(1)?,
                };
                Ok((row.get(0)?, text))
            })?
            .collect::<rusqlite::Result<_>>()?;
        for (id, text) in &rows {
            if let Some(readable) = readable_search_text(text) {
                connection.execute("UPDATE orchestration_v2_projection_messages SET search_text = ?1 WHERE message_id = ?2", (&readable, id))?;
            }
        }
        if rows.len() < 500 {
            break;
        }
        if let Some((last, _)) = rows.last() {
            after.clone_from(last);
        }
    }
    Ok(())
}
