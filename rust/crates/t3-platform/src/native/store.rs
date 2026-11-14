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
    /// `client-storage.sqlite.corrupt-<seconds>`) and a new one started.
    pub recreated_database: bool,
}

/// SQLite reports a file that is damaged rather than busy or unreachable.
fn damaged(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if matches!(failure.code, rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt)
    )
}

/// Renames the database and its WAL and shared-memory files aside, for diagnosis.
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

fn prepare(path: &Path) -> rusqlite::Result<Connection> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(std::time::Duration::from_secs(10))?;
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = FULL;
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
    Ok(connection)
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
        let mut report = OpenReport::default();
        // A damaged file would otherwise fail every launch; keep it aside and start again.
        let mut connection = match prepare(&path) {
            Ok(connection) => connection,
            Err(error) if damaged(&error) => {
                move_aside(&path)?;
                report.recreated_database = true;
                prepare(&path).map_err(database_error)?
            }
            Err(error) => return Err(database_error(error)),
        };
        let random = SystemRandom::new();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let read = |name: &str| {
            transaction
                .query_row("SELECT value FROM meta WHERE name = ?1", [name], |row| {
                    row.get::<_, String>(0)
                })
                .optional()
                .map_err(database_error)
        };
        match read("schema")?.as_deref() {
            None => {
                transaction
                    .execute(
                        "INSERT INTO meta (name, value) VALUES ('schema', ?1)",
                        [SCHEMA_VERSION],
                    )
                    .map_err(database_error)?;
            }
            Some(SCHEMA_VERSION) => {}
            Some(_) => return Err(OpenError::NewerSchema),
        }
        let key_id = match read("secure-key-id")? {
            Some(id) => id,
            None => {
                let mut bytes = [0_u8; 16];
                random.fill(&mut bytes).map_err(|_| OpenError::Random)?;
                let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                let id = format!("storage-{id}");
                transaction
                    .execute(
                        "INSERT INTO meta (name, value) VALUES ('secure-key-id', ?1)",
                        [&id],
                    )
                    .map_err(database_error)?;
                id
            }
        };
        let loaded = match secrets.load(&key_id) {
            Ok(Some(secret)) if secret.len() == AES_256_GCM.key_len() => Some(secret),
            // Unreadable for good: its records can never be opened, as with a missing key.
            Ok(Some(_)) | Err(SecretStoreError::Corrupt) => {
                report.replaced_unreadable_key = true;
                None
            }
            Ok(None) => None,
            Err(error) => return Err(error.into()),
        };
        let secret = match loaded {
            Some(secret) => secret,
            None => {
                // Records sealed under a key the OS no longer holds can never be read again.
                report.discarded_secure_records = transaction
                    .execute("DELETE FROM records WHERE area = 1", [])
                    .map_err(database_error)?;
                let mut fresh = zeroize::Zeroizing::new([0_u8; 32]);
                random.fill(fresh.as_mut()).map_err(|_| OpenError::Random)?;
                secrets.store(&key_id, fresh.as_ref())?;
                let stored = secrets.load(&key_id)?.ok_or(SecretStoreError::Corrupt)?;
                if stored.as_slice() != fresh.as_ref() {
                    return Err(SecretStoreError::Corrupt.into());
                }
                stored
            }
        };
        let key = UnboundKey::new(&AES_256_GCM, &secret)
            .map(LessSafeKey::new)
            .map_err(|_| SecretStoreError::Corrupt)?;
        transaction.commit().map_err(database_error)?;
        Ok((
            Self {
                connection: Mutex::new(connection),
                key,
                random,
            },
            report,
        ))
    }

    fn aad(encoded_key: &str) -> Vec<u8> {
        [AAD_PREFIX, encoded_key.as_bytes()].concat()
    }

    fn seal(&self, encoded_key: &str, plaintext: &[u8]) -> Result<Vec<u8>, PlatformError> {
        let mut nonce = [0_u8; NONCE_LEN];
        self.random
            .fill(&mut nonce)
            .map_err(|_| storage_error("OS randomness is unavailable"))?;
        let mut sealed = plaintext.to_vec();
        self.key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(Self::aad(encoded_key)),
                &mut sealed,
            )
            .map_err(|_| storage_error("could not seal a secure record"))?;
        Ok([&[SEALED_VERSION][..], &nonce, &sealed].concat())
    }

    fn open_sealed(&self, encoded_key: &str, stored: Vec<u8>) -> Result<Vec<u8>, PlatformError> {
        let corrupt = || storage_error("a secure record failed authentication");
        let (&version, rest) = stored.split_first().ok_or_else(corrupt)?;
        if version != SEALED_VERSION || rest.len() < NONCE_LEN {
            return Err(corrupt());
        }
        let (nonce, sealed) = rest.split_at(NONCE_LEN);
        let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| corrupt())?;
        let mut sealed = sealed.to_vec();
        let plaintext = self
            .key
            .open_in_place(nonce, Aad::from(Self::aad(encoded_key)), &mut sealed)
            .map_err(|_| corrupt())?;
        Ok(plaintext.to_vec())
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
        let _ = connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
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
    fn a_lost_os_key_discards_secure_records_but_keeps_ordinary_ones() {
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
        assert_eq!(report.discarded_secure_records, 1);
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
