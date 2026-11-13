//! Native-only SQLite persistence. This crate does not target wasm32-unknown-unknown.
//! Run `open` and synchronous closures on a blocking thread, or use the async closure helpers.

mod checkpoint;
mod citations;
mod error;
mod import;
mod manifest;
pub mod migrations;
mod ownership;

use std::{
    cell::RefCell,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    path::Path,
    sync::{Arc, Condvar, Mutex},
};

pub use error::{Error, Result};
pub use import::initialize_v2_database;
pub use ownership::OwnershipLock;
pub use rusqlite;
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};

#[derive(Clone, Debug)]
pub struct OpenOptions {
    pub readers: usize,
    /// CLI callers supply the directory containing `server-owner.sqlite`.
    /// The server itself holds `OwnershipLock` for its entire lifetime.
    pub migration_owner_directory: Option<std::path::PathBuf>,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            readers: 4,
            migration_owner_directory: None,
        }
    }
}

struct ReaderPool {
    available: Mutex<Vec<Connection>>,
    ready: Condvar,
}

struct ReaderLease<'a> {
    connection: Option<Connection>,
    pool: &'a ReaderPool,
}

impl Drop for ReaderLease<'_> {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            let mut available = self
                .pool
                .available
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            available.push(connection);
            self.pool.ready.notify_one();
        }
    }
}

struct Inner {
    // SQLite checkpoints on the last writable close: stop the worker, then readers, then writer.
    checkpoint: Option<checkpoint::CheckpointWorker>,
    readers: Option<ReaderPool>,
    writer: Mutex<Connection>,
    reader_warning: Option<String>,
    checkpoint_warning: Option<String>,
}

#[derive(Clone)]
pub struct Database(Arc<Inner>);

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_options(path, OpenOptions::default())
    }

    /// The configuration path: snapshot V1 if necessary, then open and migrate V2.
    pub fn open_v2(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
        import::initialize_supported_v2_database(path.as_ref())?;
        Self::open_with_options(path, options)
    }

    pub fn open_with_options(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
        let path = path.as_ref();
        Self::open_inner(path, options).map_err(|error| error.with_database_path(path))
    }

    fn open_inner(path: &Path, options: OpenOptions) -> Result<Self> {
        let memory = path == Path::new(":memory:") || path.as_os_str().is_empty();
        if !memory
            && let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let mut writer = Connection::open(path)?;
        let (checkpoint, checkpoint_warning) = if !memory && rusqlite::version_number() >= 3_051_000
        {
            match checkpoint::CheckpointWorker::start(path) {
                Ok(worker) => {
                    // Match nodeSqliteClient's worker backstop before Sqlite.ts's ordered setup.
                    writer.execute_batch("PRAGMA wal_autocheckpoint = 10000;")?;
                    (Some(worker), None)
                }
                Err(error) => (
                    None,
                    Some(format!(
                        "SQLite checkpoint worker unavailable; the writer checkpoints its own WAL: {error}"
                    )),
                ),
            }
        } else {
            (None, None)
        };
        setup(&writer)?;
        {
            let _ownership = if migrations::has_pending_migrations(&writer)? {
                options
                    .migration_owner_directory
                    .as_ref()
                    .map(OwnershipLock::acquire)
                    .transpose()?
            } else {
                None
            };
            migrations::run_migrations(&mut writer, None)?;
        }
        clear_planner_statistics(&writer)?;
        if let Some(checkpoint) = &checkpoint {
            checkpoint.notify();
        }
        let readers = || -> Result<Vec<Connection>> {
            let mut readers = Vec::with_capacity(options.readers);
            for _ in 0..options.readers {
                let reader = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
                reader.execute_batch("PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;")?;
                let journal: String =
                    reader.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
                if !journal.eq_ignore_ascii_case("wal") {
                    return Err(Error::ReaderWithoutWal(journal));
                }
                readers.push(reader);
            }
            Ok(readers)
        };
        let (readers, reader_warning) = if memory || options.readers == 0 {
            (None, None)
        } else {
            match readers() {
                Ok(readers) => (
                    Some(ReaderPool {
                        available: Mutex::new(readers),
                        ready: Condvar::new(),
                    }),
                    None,
                ),
                Err(error) => (
                    None,
                    Some(format!(
                        "SQLite reader worker unavailable; reads use the writer: {error}"
                    )),
                ),
            }
        };
        // Closures keep transactions on one connection; spawn_blocking isolates SQLite and lock waits.
        // There is no async mutex held over blocking disk I/O or a transaction crossing an await.
        Ok(Self(Arc::new(Inner {
            checkpoint,
            writer: Mutex::new(writer),
            readers,
            reader_warning,
            checkpoint_warning,
        })))
    }

    pub fn reader_warning(&self) -> Option<&str> {
        self.0.reader_warning.as_deref()
    }

    pub fn checkpoint_warning(&self) -> Option<String> {
        self.0.checkpoint_warning.clone().or_else(|| {
            self.0
                .checkpoint
                .as_ref()
                .and_then(checkpoint::CheckpointWorker::warning)
        })
    }

    pub fn with_writer<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T>,
    ) -> Result<T> {
        let _access = Access::enter(self)?;
        let mut writer = self
            .0
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        rollback_open_transaction(&writer)?;
        self.0.writer.clear_poison();
        let result = catch_unwind(AssertUnwindSafe(|| operation(&mut writer)));
        let cleanup = rollback_open_transaction(&writer);
        drop(writer);
        if let Some(checkpoint) = &self.0.checkpoint {
            checkpoint.notify();
        }
        finish_operation(result, cleanup)
    }

    pub fn with_reader<T>(&self, operation: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let Some(pool) = &self.0.readers else {
            return self.with_writer(|connection| operation(connection));
        };
        let _access = Access::enter(self)?;
        let mut available = pool.available.lock().map_err(|_| Error::Poisoned)?;
        while available.is_empty() {
            available = pool.ready.wait(available).map_err(|_| Error::Poisoned)?;
        }
        let lease = ReaderLease {
            connection: available.pop(),
            pool,
        };
        drop(available);
        let connection = lease.connection.as_ref().ok_or(Error::Poisoned)?;
        rollback_open_transaction(connection)?;
        let result = catch_unwind(AssertUnwindSafe(|| operation(connection)));
        let cleanup = rollback_open_transaction(connection);
        drop(lease);
        finish_operation(result, cleanup)
    }

    pub fn transaction<T>(
        &self,
        behavior: TransactionBehavior,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        self.with_writer(|connection| {
            let transaction = connection.transaction_with_behavior(behavior)?;
            let value = operation(&transaction)?;
            transaction.commit()?;
            Ok(value)
        })
    }

    pub fn read_transaction<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        self.with_reader(|connection| {
            let transaction = connection.unchecked_transaction()?;
            let value = operation(&transaction)?;
            transaction.commit()?;
            Ok(value)
        })
    }

    pub async fn writer<T, F>(&self, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        Access::check(self)?;
        let database = self.clone();
        tokio::task::spawn_blocking(move || database.with_writer(operation)).await?
    }

    pub async fn reader<T, F>(&self, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
    {
        Access::check(self)?;
        let database = self.clone();
        tokio::task::spawn_blocking(move || database.with_reader(operation)).await?
    }

    /// Drop this handle off the runtime; dropping the last handle joins the checkpoint worker.
    pub async fn close(self) -> Result<()> {
        tokio::task::spawn_blocking(move || drop(self)).await?;
        Ok(())
    }
}

thread_local! {
    static ACTIVE_DATABASES: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

struct Access(usize);

impl Access {
    fn key(database: &Database) -> usize {
        Arc::as_ptr(&database.0) as usize
    }

    fn check(database: &Database) -> Result<()> {
        if ACTIVE_DATABASES.with(|active| active.borrow().contains(&Self::key(database))) {
            return Err(Error::NestedAccess);
        }
        Ok(())
    }

    fn enter(database: &Database) -> Result<Self> {
        Self::check(database)?;
        let key = Self::key(database);
        ACTIVE_DATABASES.with(|active| active.borrow_mut().push(key));
        Ok(Self(key))
    }
}

impl Drop for Access {
    fn drop(&mut self) {
        ACTIVE_DATABASES.with(|active| {
            let removed = active.borrow_mut().pop();
            debug_assert_eq!(removed, Some(self.0));
        });
    }
}

fn rollback_open_transaction(connection: &Connection) -> Result<()> {
    if !connection.is_autocommit() {
        connection.execute_batch("ROLLBACK")?;
    }
    Ok(())
}

fn finish_operation<T>(result: std::thread::Result<Result<T>>, cleanup: Result<()>) -> Result<T> {
    match result {
        Ok(result) => cleanup.and(result),
        Err(panic) => resume_unwind(panic),
    }
}

fn setup(connection: &Connection) -> Result<()> {
    connection.execute_batch("PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;")?;
    let tables: i64 = connection.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE type = 'table'",
        [],
        |row| row.get(0),
    )?;
    if tables == 0 {
        connection.execute_batch("PRAGMA auto_vacuum = INCREMENTAL;")?;
    }
    connection.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA checkpoint_fullfsync = ON; PRAGMA temp_store = MEMORY; PRAGMA journal_size_limit = 134217728;")?;
    Ok(())
}

fn clear_planner_statistics(connection: &Connection) -> Result<()> {
    if migrations::table_exists(connection, "sqlite_stat1")? {
        connection.execute_batch("DELETE FROM sqlite_stat1; ANALYZE sqlite_schema;")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
