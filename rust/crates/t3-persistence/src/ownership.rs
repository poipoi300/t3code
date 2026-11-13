use std::path::Path;

use rusqlite::Connection;

use crate::{Error, Result};

/// Compatible with `serverOwnership.ts`; never unlink `server-owner.sqlite`.
/// Runtime-state publication and legacy service detection belong to the server crate.
pub struct OwnershipLock {
    _connection: Connection,
}

impl OwnershipLock {
    pub fn acquire(state_dir: impl AsRef<Path>) -> Result<Self> {
        let state_dir = state_dir.as_ref();
        std::fs::create_dir_all(state_dir)?;
        let connection = Connection::open(state_dir.join("server-owner.sqlite"))?;
        connection.execute_batch("PRAGMA busy_timeout = 250;")?;
        if let Err(cause) = connection.execute_batch("BEGIN EXCLUSIVE;") {
            if cause.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy) {
                return Err(Error::ServerOwnsDatabase {
                    state_dir: state_dir.to_owned(),
                });
            }
            return Err(cause.into());
        }
        Ok(Self {
            _connection: connection,
        })
    }
}
