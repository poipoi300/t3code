//! Real hosts for `t3_client::platform::Platform`: one constructor per host family.
//!
//! Compiles to native (`native`, tokio) and to `wasm32-unknown-unknown` (`web`, browser APIs).
//! Shells pass the data directory, secret-key store and host extensions; feature code stays in
//! `t3-client`.

mod common;

pub use common::{EventSender, HostExtension};

#[cfg(not(target_arch = "wasm32"))]
pub mod native;
#[cfg(target_os = "ios")]
pub use native::KeychainStore;
#[cfg(target_os = "android")]
pub use native::{AndroidKeystoreStore, set_android_vm};
#[cfg(not(target_arch = "wasm32"))]
pub use native::{
    EphemeralSecretStore, NativeConfig, NativePlatform, OpenError, SecretKeyStore,
    SecretStoreError, Timeouts,
};

#[cfg(target_arch = "wasm32")]
pub mod web;
#[cfg(target_arch = "wasm32")]
pub use web::{WebConfig, WebPlatform};
