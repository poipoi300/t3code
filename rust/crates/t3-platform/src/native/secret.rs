//! The key that encrypts the secure storage area lives in an OS key store, never on disk.
use std::sync::Mutex;
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
pub enum SecretStoreError {
    #[error("the OS secret store is unavailable: {0}")]
    Unavailable(String),
    #[error("the OS secret store holds an unusable key")]
    Corrupt,
}

/// Holds one 32-byte storage key per data directory.
///
/// Desktop builds get [`SecretKeyStore::os_default`] (Windows Credential Manager, macOS Keychain,
/// Linux Secret Service). Android and iOS shells implement this with the Android Keystore and
/// the iOS Keychain; there is no plaintext fallback.
pub trait SecretKeyStore: Send + Sync + 'static {
    fn load(&self, id: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretStoreError>;
    fn store(&self, id: &str, secret: &[u8]) -> Result<(), SecretStoreError>;
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
impl dyn SecretKeyStore {
    pub fn os_default() -> Box<dyn SecretKeyStore> {
        Box::new(OsSecretStore {
            service: "T3 Code client storage".into(),
        })
    }
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
struct OsSecretStore {
    service: String,
}
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
impl SecretKeyStore for OsSecretStore {
    fn load(&self, id: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretStoreError> {
        let entry = keyring::Entry::new(&self.service, id)
            .map_err(|e| SecretStoreError::Unavailable(e.to_string()))?;
        match entry.get_secret() {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SecretStoreError::Unavailable(e.to_string())),
        }
    }
    fn store(&self, id: &str, secret: &[u8]) -> Result<(), SecretStoreError> {
        keyring::Entry::new(&self.service, id)
            .and_then(|entry| entry.set_secret(secret))
            .map_err(|e| SecretStoreError::Unavailable(e.to_string()))
    }
}

/// Keeps keys in process memory only: secure records become unreadable when the process exits
/// and are discarded on the next open. For tests and throwaway tools, never for an app.
#[derive(Default)]
pub struct EphemeralSecretStore {
    keys: Mutex<Vec<(String, Zeroizing<Vec<u8>>)>>,
}
impl SecretKeyStore for EphemeralSecretStore {
    fn load(&self, id: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretStoreError> {
        let keys = self
            .keys
            .lock()
            .map_err(|_| SecretStoreError::Unavailable("poisoned".into()))?;
        Ok(keys
            .iter()
            .find(|(key, _)| key == id)
            .map(|(_, secret)| secret.clone()))
    }
    fn store(&self, id: &str, secret: &[u8]) -> Result<(), SecretStoreError> {
        let mut keys = self
            .keys
            .lock()
            .map_err(|_| SecretStoreError::Unavailable("poisoned".into()))?;
        keys.retain(|(key, _)| key != id);
        keys.push((id.to_owned(), Zeroizing::new(secret.to_vec())));
        Ok(())
    }
}
