//! One SQLite file holds both areas, so a cross-area transaction is a single SQLite transaction.
//! Secure values are sealed with AES-256-GCM under a key held by the OS secret store.
use super::secret::{SecretKeyStore, SecretStoreError};
use ring::{
    aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{path::Path, sync::Mutex};
use t3_client::platform::{
    PlatformError, StorageArea, StorageKey, StorageTransaction, StorageWrite,
};

pub(crate) const DATABASE_FILE: &str = "client-storage.sqlite";
const SCHEMA_VERSION: &str = "1";
const SEALED_VERSION: u8 = 1;
const AAD_PREFIX: &[u8] = b"t3-platform/secure/v1\0";

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("could not open the storage directory: {0}")]
    Directory(std::io::Error),
    #[error("could not open the storage database: {0}")]
    Database(String),
    #[error("the storage database was written by a newer version")]
    NewerSchema,
    #[error(transparent)]
    SecretStore(#[from] SecretStoreError),
    #[error("OS randomness is unavailable")]
    Random,
    #[error("no tokio runtime is running; open the platform inside one")]
    NoRuntime,
    #[error("the storage worker stopped")]
    Worker,
    #[error("could not build the TLS or HTTP client: {0}")]
    Network(String),
}

/// What opening found, for the shell's diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OpenReport {
    /// Secure records discarded because their key is gone from the OS secret store, or could
    /// no longer be read from it.
    pub discarded_secure_records: usize,
    /// The OS store held this database's key but it was unreadable (a damaged wrapped key, a
    /// wrong length), so a new key replaced it.
    pub replaced_unreadable_key: bool,
    /// The database file was not a readable SQLite database; it was moved aside (named
    /// `client-storage.sqlite.corrupt-<seconds>`, deleted by the next open) and a new one
    /// started.
    pub recreated_database: bool,
    /// Secure records sealed under a key the OS store could not give back (missing, or
    /// unreadable); kept unreadable under that key's id, and reopened when it reads again.
    pub retained_secure_records: usize,
    /// Records kept that way on an earlier open, reopened now that their key read again.
    pub recovered_secure_records: usize,
}

/// SQLite reports a file that is damaged rather than busy or unreachable.
fn damaged(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if matches!(failure.code, rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt)
    )
}

/// Deletes databases moved aside by earlier launches, which reported them: their sealed rows
/// (credentials, tokens) would otherwise stay on disk. One that cannot be deleted (a directory
/// of that name, a file another program holds) is logged and left; returns whether none is left.
fn remove_moved_aside(directory: &Path) -> bool {
    let prefix = format!("{DATABASE_FILE}.corrupt-");
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    let mut all_gone = true;
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix)
            && let Err(error) = std::fs::remove_file(entry.path())
        {
            log::warn!("t3-platform: could not delete a moved-aside database: {error}");
            all_gone = false;
        }
    }
    all_gone
}

/// The key id a damaged database recorded, when its meta table can still be read, so the OS
/// entry can go with the database copy.
fn damaged_key_id(path: &Path) -> Option<String> {
    let connection =
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    connection
        .query_row(
            "SELECT value FROM meta WHERE name = 'secure-key-id'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
}

/// Renames the database and its WAL and shared-memory files aside, for diagnosis until the
/// next launch.
fn move_aside(path: &Path) -> Result<(), OpenError> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    for suffix in ["", "-wal", "-shm"] {
        let from = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
        if from.exists() {
            let to = format!("{}.corrupt-{seconds}{suffix}", path.display());
            std::fs::rename(&from, to).map_err(OpenError::Directory)?;
        }
    }
    Ok(())
}

/// The app's data directory, readable only by its user on Unix: it holds conversation history.
fn create_private_directory(directory: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)?;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(directory)
    }
}

fn meta(connection: &Connection, name: &str) -> Result<Option<String>, OpenError> {
    connection
        .query_row("SELECT value FROM meta WHERE name = ?1", [name], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(database_error)
}

/// A one-way fingerprint of the storage key, to tell which key sealed the secure records.
fn key_check(secret: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, secret)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn prepare(path: &Path) -> rusqlite::Result<Connection> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(std::time::Duration::from_secs(10))?;
    // Switching to WAL returns busy at once, without the busy timeout, while another instance
    // opens the same file.
    let mut attempts = 0;
    while let Err(error) = connection.execute_batch("PRAGMA journal_mode = WAL;") {
        let busy = matches!(
            &error,
            rusqlite::Error::SqliteFailure(failure, _)
                if matches!(failure.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
        );
        attempts += 1;
        if !busy || attempts == 100 {
            return Err(error);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    connection.execute_batch(
        "PRAGMA synchronous = FULL;
         PRAGMA secure_delete = ON;
         CREATE TABLE IF NOT EXISTS meta (name TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
         CREATE TABLE IF NOT EXISTS records (
           area INTEGER NOT NULL,
           key TEXT NOT NULL,
           namespace TEXT NOT NULL,
           value BLOB NOT NULL,
           PRIMARY KEY (area, key)
         ) STRICT, WITHOUT ROWID;
         CREATE INDEX IF NOT EXISTS records_namespace ON records (area, namespace);",
    )?;
    // Damage past the header otherwise fails some later read on every launch instead.
    let verdict: String = connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    if verdict != "ok" {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
            Some(verdict),
        ));
    }
    Ok(connection)
}

/// Secure records whose key the OS store could not give back, kept for when it reads again.
const RETAINED_AREA: i64 = 2;
/// Consecutive launches whose key read failed as temporary before the key is given up on.
const UNAVAILABLE_LAUNCHES: u32 = 3;

fn aad(encoded_key: &str) -> Vec<u8> {
    [AAD_PREFIX, encoded_key.as_bytes()].concat()
}

fn seal_with(
    key: &LessSafeKey,
    random: &SystemRandom,
    encoded_key: &str,
    plaintext: &[u8],
) -> Result<Vec<u8>, PlatformError> {
    let mut nonce = [0_u8; NONCE_LEN];
    random
        .fill(&mut nonce)
        .map_err(|_| storage_error("OS randomness is unavailable"))?;
    let mut sealed = plaintext.to_vec();
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::from(aad(encoded_key)),
        &mut sealed,
    )
    .map_err(|_| storage_error("could not seal a secure record"))?;
    Ok([&[SEALED_VERSION][..], &nonce, &sealed].concat())
}

fn open_with(
    key: &LessSafeKey,
    encoded_key: &str,
    stored: Vec<u8>,
) -> Result<Vec<u8>, PlatformError> {
    let corrupt = || storage_error("a secure record failed authentication");
    let (&version, rest) = stored.split_first().ok_or_else(corrupt)?;
    if version != SEALED_VERSION || rest.len() < NONCE_LEN {
        return Err(corrupt());
    }
    let (nonce, sealed) = rest.split_at(NONCE_LEN);
    let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| corrupt())?;
    let mut sealed = sealed.to_vec();
    let plaintext = key
        .open_in_place(nonce, Aad::from(aad(encoded_key)), &mut sealed)
        .map_err(|_| corrupt())?;
    Ok(plaintext.to_vec())
}

fn unbound(secret: &[u8]) -> Result<LessSafeKey, OpenError> {
    UnboundKey::new(&AES_256_GCM, secret)
        .map(LessSafeKey::new)
        .map_err(|_| SecretStoreError::Corrupt.into())
}

enum KeyRead {
    Present(zeroize::Zeroizing<Vec<u8>>),
    Missing,
    Unreadable,
}

fn read_key(secrets: &dyn SecretKeyStore, id: &str) -> Result<KeyRead, SecretStoreError> {
    match secrets.load(id) {
        Ok(Some(secret)) if secret.len() == AES_256_GCM.key_len() => Ok(KeyRead::Present(secret)),
        Ok(Some(_)) | Err(SecretStoreError::Corrupt) => Ok(KeyRead::Unreadable),
        Ok(None) => Ok(KeyRead::Missing),
        // Unavailable: fail this launch and touch nothing.
        Err(error) => Err(error),
    }
}

fn new_key_id(random: &SystemRandom) -> Result<String, OpenError> {
    let mut bytes = [0_u8; 16];
    random.fill(&mut bytes).map_err(|_| OpenError::Random)?;
    let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("storage-{id}"))
}

fn set_meta(connection: &Connection, name: &str, value: Option<&str>) -> Result<(), OpenError> {
    match value {
        Some(value) => connection.execute(
            "INSERT INTO meta (name, value) VALUES (?1, ?2)
             ON CONFLICT (name) DO UPDATE SET value = excluded.value",
            params![name, value],
        ),
        None => connection.execute("DELETE FROM meta WHERE name = ?1", [name]),
    }
    .map(drop)
    .map_err(database_error)
}

fn checkpoint(connection: &Connection) {
    let _ = connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
}

pub(crate) struct Store {
    connection: Mutex<Connection>,
    key: LessSafeKey,
    random: SystemRandom,
}

fn area_id(area: StorageArea) -> i64 {
    match area {
        StorageArea::Ordinary => 0,
        StorageArea::Secure => 1,
    }
}
fn storage_error(error: impl std::fmt::Display) -> PlatformError {
    PlatformError::Storage(error.to_string())
}
fn database_error(error: rusqlite::Error) -> OpenError {
    OpenError::Database(error.to_string())
}
fn encode_key(key: &StorageKey) -> Result<String, PlatformError> {
    key.encode_tuple()
        .map_err(|_| PlatformError::InvalidData("storage key".into()))
}

impl Store {
    pub(crate) fn open(
        directory: &Path,
        secrets: &dyn SecretKeyStore,
    ) -> Result<(Self, OpenReport), OpenError> {
        create_private_directory(directory).map_err(OpenError::Directory)?;
        let path = directory.join(DATABASE_FILE);
        let copies_gone = remove_moved_aside(directory);
        let mut report = OpenReport::default();
        // A damaged file would otherwise fail every launch; keep it aside and start again.
        let mut aside_key_id = None;
        let mut connection = match prepare(&path) {
            Ok(connection) => connection,
            Err(error) if damaged(&error) => {
                aside_key_id = damaged_key_id(&path);
                move_aside(&path)?;
                report.recreated_database = true;
                prepare(&path).map_err(database_error)?
            }
            Err(error) => return Err(database_error(error)),
        };
        let random = SystemRandom::new();
        // 1. The key id is committed on its own before any secret is stored, so an open that
        //    fails later reuses it instead of leaving a new OS entry behind each time.
        let (key_id, finished_aside) = {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            match meta(&transaction, "schema")?.as_deref() {
                None => set_meta(&transaction, "schema", Some(SCHEMA_VERSION))?,
                Some(SCHEMA_VERSION) => {}
                Some(_) => return Err(OpenError::NewerSchema),
            }
            let key_id = match meta(&transaction, "secure-key-id")? {
                Some(id) => id,
                None => {
                    let id = new_key_id(&random)?;
                    set_meta(&transaction, "secure-key-id", Some(&id))?;
                    id
                }
            };
            // The moved-aside copy's key goes once the copy itself is gone.
            let finished_aside = match aside_key_id.as_deref() {
                Some(id) => {
                    set_meta(&transaction, "aside-key-id", Some(id))?;
                    None
                }
                None if copies_gone => meta(&transaction, "aside-key-id")?,
                None => None,
            };
            transaction.commit().map_err(database_error)?;
            (key_id, finished_aside)
        };
        if let Some(id) = finished_aside
            && secrets.delete(&id).is_ok()
        {
            set_meta(&connection, "aside-key-id", None)?;
        }
        // 2. Every read of the OS store runs with no database lock held: a keychain or Secret
        //    Service prompt must not keep another process waiting on the database.
        let recorded = meta(&connection, "secure-key-check")?;
        let failures = meta(&connection, "unavailable-key-launches")?;
        let read = match read_key(secrets, &key_id) {
            // A key that fails as temporary on launch after launch is given up on at the third,
            // as an unreadable one is: a corrupted Android Keystore entry below API 33 looks
            // exactly like a keystore error. Its rows are kept, so a wrong call costs nothing.
            Err(error @ SecretStoreError::Unavailable(_)) => {
                let count = failures
                    .as_deref()
                    .and_then(|count| count.parse::<u32>().ok())
                    .unwrap_or(0)
                    + 1;
                set_meta(&connection, "unavailable-key-launches", Some(&count.to_string()))?;
                if count < UNAVAILABLE_LAUNCHES {
                    return Err(error.into());
                }
                None
            }
            other => Some(other?),
        };
        let secret = match read {
            Some(KeyRead::Present(secret))
                if recorded.as_deref().is_none_or(|check| check == key_check(&secret)) =>
            {
                if recorded.is_none() {
                    set_meta(&connection, "secure-key-check", Some(&key_check(&secret)))?;
                }
                if failures.is_some() {
                    set_meta(&connection, "unavailable-key-launches", None)?;
                }
                secret
            }
            Some(_) => Self::create_key(&mut connection, secrets, &random, &mut report, false)?,
            None => Self::create_key(&mut connection, secrets, &random, &mut report, true)?,
        };
        let key = unbound(&secret)?;
        // 3. Rows kept under a key that could not be read come back once it reads again, on a
        //    later launch than the one that just failed to read it twice.
        if report.retained_secure_records == 0 {
            Self::recover(&mut connection, secrets, &key, &random, &mut report)?;
        }
        Ok((
            Self {
                connection: Mutex::new(connection),
                key,
                random,
            },
            report,
        ))
    }

    /// The rare path that writes a key, under the write lock: it re-reads what another
    /// instance may have just created, so two instances starting together share one key.
    /// A key that is missing or unreadable may come back (a roaming credential, a collection
    /// still unlocking, a Keystore error), so its rows are kept and the new key takes a new id;
    /// the old entry is never overwritten.
    /// `give_up`: the key has failed as temporary for `UNAVAILABLE_LAUNCHES` launches running;
    /// treat it as unreadable, and replace the device key that wraps it.
    fn create_key(
        connection: &mut Connection,
        secrets: &dyn SecretKeyStore,
        random: &SystemRandom,
        report: &mut OpenReport,
        give_up: bool,
    ) -> Result<zeroize::Zeroizing<Vec<u8>>, OpenError> {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let key_id = meta(&transaction, "secure-key-id")?
            .ok_or_else(|| OpenError::Database("the storage key id disappeared".into()))?;
        let recorded = meta(&transaction, "secure-key-check")?;
        let current = match read_key(secrets, &key_id) {
            Err(SecretStoreError::Unavailable(_)) if give_up => KeyRead::Unreadable,
            other => other?,
        };
        let unreadable = match current {
            KeyRead::Present(secret) => {
                // Matching: another instance created it meanwhile. Not matching: the rows were
                // sealed with a key that is gone for good (an older build overwrote this id);
                // this key is kept and those rows go.
                let check = key_check(&secret);
                if recorded.as_deref().is_some_and(|recorded| recorded != check) {
                    report.discarded_secure_records += transaction
                        .execute("DELETE FROM records WHERE area = 1", [])
                        .map_err(database_error)?;
                }
                set_meta(&transaction, "secure-key-check", Some(&check))?;
                set_meta(&transaction, "unavailable-key-launches", None)?;
                transaction.commit().map_err(database_error)?;
                if report.discarded_secure_records > 0 {
                    checkpoint(connection);
                }
                return Ok(secret);
            }
            KeyRead::Missing => false,
            KeyRead::Unreadable => true,
        };
        report.replaced_unreadable_key = unreadable;
        let sealed: usize = transaction
            .query_row("SELECT COUNT(*) FROM records WHERE area = 1", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(database_error)? as usize;
        let mut discarded = 0;
        let id = match sealed {
            0 => key_id,
            _ => {
                // One generation is kept; an older one is now beyond recovery.
                discarded += transaction
                    .execute("DELETE FROM records WHERE area = ?1", [RETAINED_AREA])
                    .map_err(database_error)?;
                report.retained_secure_records = transaction
                    .execute(
                        "UPDATE records SET area = ?1 WHERE area = 1",
                        [RETAINED_AREA],
                    )
                    .map_err(database_error)?;
                set_meta(&transaction, "retained-key-id", Some(&key_id))?;
                set_meta(&transaction, "retained-key-check", recorded.as_deref())?;
                new_key_id(random)?
            }
        };
        let mut fresh = zeroize::Zeroizing::new([0_u8; 32]);
        random.fill(fresh.as_mut()).map_err(|_| OpenError::Random)?;
        if give_up {
            secrets.replace_device_key()?;
        }
        secrets.store(&id, fresh.as_ref())?;
        let stored = secrets.load(&id)?.ok_or(SecretStoreError::Corrupt)?;
        if stored.as_slice() != fresh.as_ref() {
            return Err(SecretStoreError::Corrupt.into());
        }
        set_meta(&transaction, "secure-key-id", Some(&id))?;
        set_meta(&transaction, "secure-key-check", Some(&key_check(&stored)))?;
        set_meta(&transaction, "unavailable-key-launches", None)?;
        transaction.commit().map_err(database_error)?;
        report.discarded_secure_records += discarded;
        if discarded > 0 {
            checkpoint(connection);
        }
        Ok(stored)
    }

    /// Reopens rows kept under an earlier key once that key reads again, re-sealing them under
    /// the current one; a newer row of the same key wins. The old entry then goes.
    fn recover(
        connection: &mut Connection,
        secrets: &dyn SecretKeyStore,
        key: &LessSafeKey,
        random: &SystemRandom,
        report: &mut OpenReport,
    ) -> Result<(), OpenError> {
        let Some(retained_id) = meta(connection, "retained-key-id")? else {
            return Ok(());
        };
        let recorded = meta(connection, "retained-key-check")?;
        let old = match read_key(secrets, &retained_id) {
            Ok(KeyRead::Present(secret))
                if recorded.as_deref().is_none_or(|check| check == key_check(&secret)) =>
            {
                unbound(&secret)?
            }
            // Still missing or unreadable: keep waiting.
            _ => return Ok(()),
        };
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let rows: Vec<(String, String, Vec<u8>)> = {
            let mut statement = transaction
                .prepare("SELECT key, namespace, value FROM records WHERE area = ?1")
                .map_err(database_error)?;
            statement
                .query_map([RETAINED_AREA], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .map_err(database_error)?
                .collect::<rusqlite::Result<_>>()
                .map_err(database_error)?
        };
        for (encoded, namespace, value) in rows {
            let Ok(plaintext) = open_with(&old, &encoded, value) else {
                report.discarded_secure_records += 1;
                continue;
            };
            let sealed = seal_with(key, random, &encoded, &plaintext)
                .map_err(|error| OpenError::Database(error.to_string()))?;
            report.recovered_secure_records += transaction
                .execute(
                    "INSERT INTO records (area, key, namespace, value) VALUES (1, ?1, ?2, ?3)
                     ON CONFLICT (area, key) DO NOTHING",
                    params![encoded, namespace, sealed],
                )
                .map_err(database_error)?;
        }
        transaction
            .execute("DELETE FROM records WHERE area = ?1", [RETAINED_AREA])
            .map_err(database_error)?;
        set_meta(&transaction, "retained-key-id", None)?;
        set_meta(&transaction, "retained-key-check", None)?;
        transaction.commit().map_err(database_error)?;
        checkpoint(connection);
        // Nothing reads it any more; a failure leaves one orphaned entry and is not fatal.
        if let Err(error) = secrets.delete(&retained_id) {
            log::warn!("t3-platform: could not delete a recovered storage key: {error}");
        }
        Ok(())
    }

    fn seal(&self, encoded_key: &str, plaintext: &[u8]) -> Result<Vec<u8>, PlatformError> {
        seal_with(&self.key, &self.random, encoded_key, plaintext)
    }

    fn open_sealed(&self, encoded_key: &str, stored: Vec<u8>) -> Result<Vec<u8>, PlatformError> {
        open_with(&self.key, encoded_key, stored)
    }

    fn read(
        &self,
        connection: &Connection,
        area: StorageArea,
        encoded_key: &str,
    ) -> Result<Option<Vec<u8>>, PlatformError> {
        let stored = connection
            .query_row(
                "SELECT value FROM records WHERE area = ?1 AND key = ?2",
                params![area_id(area), encoded_key],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()
            .map_err(storage_error)?;
        match (area, stored) {
            (StorageArea::Secure, Some(stored)) => self.open_sealed(encoded_key, stored).map(Some),
            (_, stored) => Ok(stored),
        }
    }

    fn apply(
        &self,
        connection: &Connection,
        area: StorageArea,
        writes: Vec<StorageWrite>,
        changed: &mut Vec<StorageKey>,
    ) -> Result<bool, PlatformError> {
        let mut secure_deleted = false;
        for write in writes {
            let encoded = encode_key(&write.key)?;
            match write.value {
                Some(value) => {
                    let value = match area {
                        StorageArea::Secure => self.seal(&encoded, &value)?,
                        StorageArea::Ordinary => value,
                    };
                    connection
                        .execute(
                            "INSERT INTO records (area, key, namespace, value) VALUES (?1, ?2, ?3, ?4)
                             ON CONFLICT (area, key) DO UPDATE SET value = excluded.value",
                            params![area_id(area), encoded, write.key.namespace, value],
                        )
                        .map_err(storage_error)?;
                }
                None => {
                    secure_deleted |= connection
                        .execute(
                            "DELETE FROM records WHERE area = ?1 AND key = ?2",
                            params![area_id(area), encoded],
                        )
                        .map_err(storage_error)?
                        > 0
                        && area == StorageArea::Secure;
                }
            }
            changed.push(write.key);
        }
        Ok(secure_deleted)
    }

    /// `secure_delete` zeroes freed pages in the database; the WAL still holds the deleted
    /// ciphertext until a checkpoint copies it back and truncates the log.
    fn scrub_wal(connection: &Connection) {
        checkpoint(connection);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn get(
        &self,
        area: StorageArea,
        key: &StorageKey,
    ) -> Result<Option<Vec<u8>>, PlatformError> {
        let encoded = encode_key(key)?;
        self.read(&self.lock(), area, &encoded)
    }

    pub(crate) fn write(
        &self,
        area: StorageArea,
        writes: Vec<StorageWrite>,
    ) -> Result<Vec<StorageKey>, PlatformError> {
        let mut connection = self.lock();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let mut changed = Vec::new();
        let secure_deleted = self.apply(&transaction, area, writes, &mut changed)?;
        transaction.commit().map_err(storage_error)?;
        if secure_deleted {
            Self::scrub_wal(&connection);
        }
        Ok(changed)
    }

    pub(crate) fn list(
        &self,
        area: StorageArea,
        namespace: &str,
    ) -> Result<Vec<StorageKey>, PlatformError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare_cached("SELECT key FROM records WHERE area = ?1 AND namespace = ?2")
            .map_err(storage_error)?;
        let mut keys = statement
            .query_map(params![area_id(area), namespace], |row| {
                row.get::<_, String>(0)
            })
            .map_err(storage_error)?
            .map(|raw| {
                let raw = raw.map_err(storage_error)?;
                StorageKey::decode_tuple(&raw)
                    .map_err(|_| PlatformError::InvalidData("stored key".into()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        keys.sort();
        Ok(keys)
    }

    /// `None` means a condition failed and nothing was written.
    pub(crate) fn transaction(
        &self,
        request: StorageTransaction,
    ) -> Result<Option<Vec<StorageKey>>, PlatformError> {
        let mut connection = self.lock();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        for condition in &request.conditions {
            let encoded = encode_key(&condition.key)?;
            if self.read(&transaction, condition.area, &encoded)? != condition.expected {
                return Ok(None);
            }
        }
        let mut changed = Vec::new();
        let mut secure_deleted = self.apply(
            &transaction,
            StorageArea::Ordinary,
            request.ordinary,
            &mut changed,
        )?;
        secure_deleted |= self.apply(
            &transaction,
            StorageArea::Secure,
            request.secure,
            &mut changed,
        )?;
        transaction.commit().map_err(storage_error)?;
        if secure_deleted {
            Self::scrub_wal(&connection);
        }
        Ok(Some(changed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::secret::EphemeralSecretStore;
    use t3_client::platform::StorageCondition;

    fn key(record: &str) -> StorageKey {
        StorageKey::new("ns", None, record)
    }
    fn put(record: &str, value: &[u8]) -> StorageWrite {
        StorageWrite {
            key: key(record),
            value: Some(value.to_vec()),
        }
    }

    #[test]
    fn secure_values_are_not_stored_in_plaintext_and_cannot_be_moved_between_keys() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = EphemeralSecretStore::default();
        let (store, _) = Store::open(dir.path(), &secrets).unwrap();
        let secret = b"bearer-token-that-must-not-leak";
        store
            .write(
                StorageArea::Secure,
                vec![put("a", secret), put("b", b"other")],
            )
            .unwrap();
        store
            .write(StorageArea::Ordinary, vec![put("plain", b"visible")])
            .unwrap();
        drop(store);
        let mut raw = Vec::new();
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            raw.extend(std::fs::read(entry.unwrap().path()).unwrap());
        }
        assert!(raw.windows(secret.len()).all(|w| w != secret));
        assert!(raw.windows(7).any(|w| w == b"visible"));

        let connection = Connection::open(dir.path().join(DATABASE_FILE)).unwrap();
        let a = encode_key(&key("a")).unwrap();
        let b = encode_key(&key("b")).unwrap();
        connection
            .execute(
                "UPDATE records SET value = (SELECT value FROM records WHERE area = 1 AND key = ?1)
                 WHERE area = 1 AND key = ?2",
                [&a, &b],
            )
            .unwrap();
        drop(connection);
        let (store, report) = Store::open(dir.path(), &secrets).unwrap();
        assert_eq!(report.discarded_secure_records, 0);
        assert_eq!(
            store.get(StorageArea::Secure, &key("a")).unwrap().unwrap(),
            secret
        );
        assert!(store.get(StorageArea::Secure, &key("b")).is_err());
    }

    #[test]
    fn a_lost_os_key_sets_secure_records_aside_but_keeps_ordinary_ones() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = Store::open(dir.path(), &EphemeralSecretStore::default()).unwrap();
        store
            .write(StorageArea::Secure, vec![put("token", b"t")])
            .unwrap();
        store
            .write(StorageArea::Ordinary, vec![put("cache", b"c")])
            .unwrap();
        drop(store);
        let (store, report) = Store::open(dir.path(), &EphemeralSecretStore::default()).unwrap();
        assert_eq!(report.retained_secure_records, 1);
        assert_eq!(report.discarded_secure_records, 0);
        assert_eq!(store.get(StorageArea::Secure, &key("token")).unwrap(), None);
        assert_eq!(
            store.get(StorageArea::Ordinary, &key("cache")).unwrap(),
            Some(b"c".to_vec())
        );
    }

    #[test]
    fn an_unavailable_secret_store_fails_open_instead_of_writing_plaintext() {
        struct Broken;
        impl SecretKeyStore for Broken {
            fn load(
                &self,
                _: &str,
            ) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, SecretStoreError> {
                Err(SecretStoreError::Unavailable("locked".into()))
            }
            fn store(&self, _: &str, _: &[u8]) -> Result<(), SecretStoreError> {
                Err(SecretStoreError::Unavailable("locked".into()))
            }
        }
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            Store::open(dir.path(), &Broken),
            Err(OpenError::SecretStore(_))
        ));
    }

    #[test]
    fn transaction_with_a_failed_condition_writes_nothing_in_either_area() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = Store::open(dir.path(), &EphemeralSecretStore::default()).unwrap();
        store
            .write(StorageArea::Secure, vec![put("guard", b"v1")])
            .unwrap();
        let attempt = |expected: &[u8]| StorageTransaction {
            conditions: vec![StorageCondition {
                area: StorageArea::Secure,
                key: key("guard"),
                expected: Some(expected.to_vec()),
            }],
            ordinary: vec![put("o", b"1"), put("o2", b"2")],
            secure: vec![put("guard", b"v2")],
        };
        assert_eq!(store.transaction(attempt(b"stale")).unwrap(), None);
        assert_eq!(store.get(StorageArea::Ordinary, &key("o")).unwrap(), None);
        assert_eq!(
            store.get(StorageArea::Secure, &key("guard")).unwrap(),
            Some(b"v1".to_vec())
        );
        let changed = store.transaction(attempt(b"v1")).unwrap().unwrap();
        assert_eq!(changed, vec![key("o"), key("o2"), key("guard")]);
        assert_eq!(
            store.get(StorageArea::Secure, &key("guard")).unwrap(),
            Some(b"v2".to_vec())
        );
        // An absent-key condition holds only while the key is absent.
        let create = StorageTransaction {
            conditions: vec![StorageCondition {
                area: StorageArea::Ordinary,
                key: key("o"),
                expected: None,
            }],
            ordinary: vec![put("o", b"again")],
            secure: vec![],
        };
        assert_eq!(store.transaction(create).unwrap(), None);
    }

    #[test]
    fn writes_apply_in_order_and_list_is_scoped_by_area_and_namespace() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = Store::open(dir.path(), &EphemeralSecretStore::default()).unwrap();
        store
            .write(
                StorageArea::Ordinary,
                vec![
                    put("b", b"1"),
                    StorageWrite {
                        key: key("b"),
                        value: None,
                    },
                    put("a", b"2"),
                    put("a", b"3"),
                    StorageWrite {
                        key: StorageKey::new("other", None, "x"),
                        value: Some(vec![]),
                    },
                ],
            )
            .unwrap();
        store
            .write(StorageArea::Secure, vec![put("s", b"secret")])
            .unwrap();
        assert_eq!(
            store.list(StorageArea::Ordinary, "ns").unwrap(),
            vec![key("a")]
        );
        assert_eq!(
            store.get(StorageArea::Ordinary, &key("a")).unwrap(),
            Some(b"3".to_vec())
        );
        assert_eq!(
            store.list(StorageArea::Secure, "ns").unwrap(),
            vec![key("s")]
        );
        assert_eq!(
            store
                .get(StorageArea::Ordinary, &StorageKey::new("other", None, "x"))
                .unwrap(),
            Some(vec![])
        );
    }
}
