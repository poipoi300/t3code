use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(
        "The T3 Code database at {path} is damaged or is not a SQLite database, so T3 Code cannot start. To recover, quit T3 Code, then move that file and any -wal and -shm files next to it out of the folder, or replace them with a backup. T3 Code creates a new database on the next start."
    )]
    DamagedDatabase {
        path: PathBuf,
        #[source]
        cause: rusqlite::Error,
    },
    #[error(
        "Database migration head {head} is older than the minimum supported head {minimum}. Open the previous app version once to upgrade this database, then retry. The database schema was not changed."
    )]
    UnsupportedHead { head: u32, minimum: u32 },
    #[error(
        "Cannot migrate a nonempty database without a migration ledger. Open the previous app version once, then retry."
    )]
    MissingLedger,
    #[error(
        "Cannot upgrade this database's migration history: {details}. The migrator skips every id up to the highest recorded id, regardless of name. Start the build that wrote it, or report this migration history so an upgrade path can be added."
    )]
    MigrationHistory { details: String },
    #[error("Migration {id}_{name} failed: {cause}")]
    Migration {
        id: u32,
        name: &'static str,
        #[source]
        cause: Box<Error>,
    },
    #[error(
        "Another T3 Code process, usually a running server, owns {state_dir}, and this CLI would change its database schema. Use the `t3` that matches the running server, or stop that server, then retry. The database was not changed."
    )]
    ServerOwnsDatabase { state_dir: PathBuf },
    #[error(
        "Could not copy the V1 database at {source_path} to {destination_path}. The V1 database has not been migrated: {cause}"
    )]
    Import {
        source_path: PathBuf,
        destination_path: PathBuf,
        #[source]
        cause: Box<Error>,
    },
    #[error("SQLite backup did not finish copying the database; no snapshot was published")]
    IncompleteBackup,
    #[error(
        "Nested connection access to the same Database is not allowed; use the connection supplied to the closure"
    )]
    NestedAccess,
    #[error("Database synchronization lock was poisoned by a panicking caller")]
    Poisoned,
    #[error("Database blocking task failed: {0}")]
    BlockingTask(#[from] tokio::task::JoinError),
    #[error("Expected WAL mode for a reader connection, found {0}")]
    ReaderWithoutWal(String),
}

impl Error {
    pub(crate) fn with_database_path(self, path: &std::path::Path) -> Self {
        match self {
            Self::Sqlite(cause)
                if cause
                    .sqlite_error()
                    .is_some_and(|error| matches!(error.extended_code & 0xff, 11 | 26)) =>
            {
                Self::DamagedDatabase {
                    path: path.to_owned(),
                    cause,
                }
            }
            Self::Migration { id, name, cause } => Self::Migration {
                id,
                name,
                cause: Box::new(cause.with_database_path(path)),
            },
            other => other,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
