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
    /// Removes the key stored under `id`; an id with no key is success. A store that cannot
    /// delete leaves one orphaned entry, which opens nothing once its records are gone.
    fn delete(&self, _id: &str) -> Result<(), SecretStoreError> {
        Ok(())
    }
    /// Called before a new key is stored in place of one that failed launch after launch. A
    /// store that wraps every key with one device key (Android's Keystore alias) replaces it,
    /// since a broken one fails every new key too; one with an entry per key has nothing to do.
    fn replace_device_key(&self) -> Result<(), SecretStoreError> {
        Ok(())
    }
}

/// What a failed Android Keystore call means. `secret/android.rs` gathers the facts from the
/// exception and the call that raised it; the decision is here, tested on the cases AOSP
/// delivers. Only a key the Keystore says is gone, or a blob that does not match the key, is
/// certain; everything else, keystore2's system errors included, fails this launch only.
#[cfg(any(target_os = "android", test))]
pub(crate) mod keystore_failure {
    /// The call that raised the exception.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Site {
        /// `KeyStore.getKey`.
        GetKey,
        /// `Cipher.init`.
        Init,
        /// `Cipher.doFinal`.
        DoFinal,
        Other,
    }

    /// The exception's class, as far as the decision needs it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Thrown {
        AeadBadTag,
        KeyPermanentlyInvalidated,
        UnrecoverableKey,
        Other,
    }

    /// The exception's cause.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Cause {
        None,
        /// An `android.security.KeyStoreException`, with its `isTransientFailure()` and
        /// `getNumericErrorCode()` (both API 33+).
        KeyStore {
            transient: Option<bool>,
            code: Option<i32>,
        },
        Other,
    }

    #[derive(Debug, Clone, Copy)]
    pub(crate) struct Facts {
        pub site: Site,
        pub thrown: Thrown,
        pub cause: Cause,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Failure {
        /// The key will never open this blob; `replace_key` when the Keystore entry itself is
        /// gone for good and must be deleted before a new key can wrap anything.
        Permanent { replace_key: bool },
        /// Possibly temporary: fail this launch and touch nothing.
        Transient,
    }

    /// `KeyStoreException.ERROR_KEY_DOES_NOT_EXIST` and `ERROR_KEY_CORRUPTED` (android.jar,
    /// API 34).
    const KEY_DOES_NOT_EXIST: i32 = 6;
    const KEY_CORRUPTED: i32 = 7;

    pub(crate) fn classify(facts: &Facts) -> Failure {
        match (facts.site, facts.thrown, facts.cause) {
            (_, Thrown::KeyPermanentlyInvalidated, _)
            // keystore2's getKey rethrows an invalidated key, or one of an unknown algorithm,
            // as UnrecoverableKeyException with no cause.
            | (Site::GetKey, Thrown::UnrecoverableKey, Cause::None)
            // API 33+ names a corrupted or vanished key; below 33 that looks like any other
            // keystore error, which the store gives up on after three launches running.
            | (
                Site::GetKey,
                _,
                Cause::KeyStore {
                    code: Some(KEY_CORRUPTED | KEY_DOES_NOT_EXIST),
                    ..
                },
            ) => Failure::Permanent { replace_key: true },
            (Site::DoFinal, Thrown::AeadBadTag, _) => Failure::Permanent { replace_key: false },
            _ => Failure::Transient,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The exceptions AOSP raises, from `sources/android-31` (second re-review A1, A2).
        #[test]
        fn only_certain_keystore_failures_discard_saved_credentials() {
            let facts = |site, thrown, cause| Facts { site, thrown, cause };
            let replace = Failure::Permanent { replace_key: true };
            for (facts, expected) in [
                // keystore2 `AndroidKeyStoreSpi.engineGetKey` (:108-114): a permanently
                // invalidated key, rethrown with no cause. Also "Key algorithm unknown"
                // (`AndroidKeyStoreProvider.java:387, 401`).
                (facts(Site::GetKey, Thrown::UnrecoverableKey, Cause::None), replace),
                // A binder failure after one retry: `KeyStore2.handleRemoteExceptionWithRetry`
                // (:103-127) raises SYSTEM_ERROR, wrapped by `loadAndroidKeyStoreKeyFromKeystore`
                // (:350-365). API 31-32 have no `isTransientFailure`; API 33+ answers false.
                (
                    facts(
                        Site::GetKey,
                        Thrown::UnrecoverableKey,
                        Cause::KeyStore { transient: None, code: None },
                    ),
                    Failure::Transient,
                ),
                (
                    facts(
                        Site::GetKey,
                        Thrown::UnrecoverableKey,
                        Cause::KeyStore { transient: Some(false), code: Some(4) },
                    ),
                    Failure::Transient,
                ),
                (
                    facts(
                        Site::GetKey,
                        Thrown::UnrecoverableKey,
                        Cause::KeyStore { transient: Some(true), code: Some(4) },
                    ),
                    Failure::Transient,
                ),
                // API 33+: the code says the key is corrupted, or gone.
                (
                    facts(
                        Site::GetKey,
                        Thrown::UnrecoverableKey,
                        Cause::KeyStore { transient: Some(false), code: Some(7) },
                    ),
                    replace,
                ),
                (
                    facts(
                        Site::GetKey,
                        Thrown::UnrecoverableKey,
                        Cause::KeyStore { transient: Some(false), code: Some(6) },
                    ),
                    replace,
                ),
                // A code at another call says nothing about the key itself.
                (
                    facts(
                        Site::DoFinal,
                        Thrown::Other,
                        Cause::KeyStore { transient: Some(false), code: Some(7) },
                    ),
                    Failure::Transient,
                ),
                (
                    facts(Site::GetKey, Thrown::UnrecoverableKey, Cause::Other),
                    Failure::Transient,
                ),
                // Older keystores raise the invalidation itself, at `Cipher.init`.
                (facts(Site::Init, Thrown::KeyPermanentlyInvalidated, Cause::None), replace),
                // The blob was not wrapped by this key.
                (
                    facts(Site::DoFinal, Thrown::AeadBadTag, Cause::None),
                    Failure::Permanent { replace_key: false },
                ),
                // Bad padding, an invalid key at init, anything else: not certain.
                (facts(Site::DoFinal, Thrown::Other, Cause::None), Failure::Transient),
                (facts(Site::Init, Thrown::Other, Cause::None), Failure::Transient),
                (facts(Site::GetKey, Thrown::AeadBadTag, Cause::None), Failure::Transient),
                (facts(Site::Other, Thrown::UnrecoverableKey, Cause::None), Failure::Transient),
            ] {
                assert_eq!(classify(&facts), expected, "{facts:?}");
            }
        }
    }
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
    fn delete(&self, id: &str) -> Result<(), SecretStoreError> {
        self.0.delete(id)
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
    fn delete(&self, id: &str) -> Result<(), SecretStoreError> {
        match keyring::Entry::new(&self.service, id).and_then(|entry| entry.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(SecretStoreError::Unavailable(e.to_string())),
        }
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
    fn delete(&self, id: &str) -> Result<(), SecretStoreError> {
        self.keys
            .lock()
            .map_err(|_| SecretStoreError::Unavailable("poisoned".into()))?
            .retain(|(key, _)| key != id);
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
