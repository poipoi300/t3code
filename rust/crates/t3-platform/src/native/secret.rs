//! The key that encrypts the secure storage area lives in an OS key store, never on disk.
use std::sync::Mutex;
use zeroize::Zeroizing;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::{AndroidKeystoreStore, set_android_vm};

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
/// Linux Secret Service); Android gets `AndroidKeystoreStore` and iOS `KeychainStore`, through
/// `NativeConfig::mobile`. There is no plaintext fallback.
pub trait SecretKeyStore: Send + Sync + 'static {
    fn load(&self, id: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretStoreError>;
    fn store(&self, id: &str, secret: &[u8]) -> Result<(), SecretStoreError>;
}

#[cfg(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "linux",
    target_os = "ios"
))]
const SERVICE: &str = "T3 Code client storage";

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
impl dyn SecretKeyStore {
    pub fn os_default() -> Box<dyn SecretKeyStore> {
        Box::new(OsSecretStore {
            service: SERVICE.into(),
        })
    }
}

/// The storage key as a generic password in the app's keychain access group.
#[cfg(target_os = "ios")]
pub struct KeychainStore(OsSecretStore);
#[cfg(target_os = "ios")]
impl KeychainStore {
    pub fn new() -> Self {
        Self(OsSecretStore {
            service: SERVICE.into(),
        })
    }
}
#[cfg(target_os = "ios")]
impl Default for KeychainStore {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(target_os = "ios")]
impl SecretKeyStore for KeychainStore {
    fn load(&self, id: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretStoreError> {
        self.0.load(id)
    }
    fn store(&self, id: &str, secret: &[u8]) -> Result<(), SecretStoreError> {
        self.0.store(id, secret)
    }
}

#[cfg(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "linux",
    target_os = "ios"
))]
struct OsSecretStore {
    service: String,
}
#[cfg(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "linux",
    target_os = "ios"
))]
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

/// A storage key wrapped by a platform key, as written beside the data: a version byte, the
/// nonce length and nonce, then the ciphertext with its tag.
#[cfg(any(target_os = "android", test))]
mod wrapped {
    use super::SecretStoreError;
    use sha2::{Digest, Sha256};

    const VERSION: u8 = 1;

    pub(super) fn encode(nonce: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, SecretStoreError> {
        let length = u8::try_from(nonce.len()).map_err(|_| SecretStoreError::Corrupt)?;
        if nonce.is_empty() || ciphertext.is_empty() {
            return Err(SecretStoreError::Corrupt);
        }
        let mut blob = Vec::with_capacity(2 + nonce.len() + ciphertext.len());
        blob.push(VERSION);
        blob.push(length);
        blob.extend_from_slice(nonce);
        blob.extend_from_slice(ciphertext);
        Ok(blob)
    }

    pub(super) fn decode(blob: &[u8]) -> Result<(&[u8], &[u8]), SecretStoreError> {
        let [VERSION, length, rest @ ..] = blob else {
            return Err(SecretStoreError::Corrupt);
        };
        let length = usize::from(*length);
        if length == 0 || rest.len() <= length {
            return Err(SecretStoreError::Corrupt);
        }
        Ok(rest.split_at(length))
    }

    /// Key ids may hold characters a file name cannot.
    pub(super) fn file_name(id: &str) -> String {
        let digest = Sha256::digest(id.as_bytes());
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        format!("{hex}.wrapped-key")
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_wrapped_key_reads_back_and_damage_is_refused() {
            let blob = encode(&[7; 12], &[9; 48]).unwrap();
            assert_eq!(decode(&blob).unwrap(), (&[7u8; 12][..], &[9u8; 48][..]));
            for damaged in [
                &blob[..0],
                &blob[..2],
                &blob[..14],
                &[2, 12, 1, 2][..],
                &[1, 0, 1, 2][..],
            ] {
                assert!(
                    matches!(decode(damaged), Err(SecretStoreError::Corrupt)),
                    "{damaged:?}"
                );
            }
            assert!(encode(&[], &[1]).is_err());
            assert!(encode(&[1; 256], &[1]).is_err());
        }

        #[test]
        fn every_key_id_gets_a_plain_file_name_of_its_own() {
            let name = file_name("storage:C:\\data/one");
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
            );
            assert_ne!(name, file_name("storage:C:\\data/two"));
            assert_eq!(name, file_name("storage:C:\\data/one"));
        }
    }
}
