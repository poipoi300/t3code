//! Real hosts for `t3_client::platform::Platform`: one constructor per host family.
//!
//! Compiles to native (`native`, tokio) and to `wasm32-unknown-unknown` (`web`, browser APIs).
//! Shells pass the data directory, secret-key store and host extensions; feature code stays in
//! `t3-client`.

mod common;
pub mod guest;
mod socket_ingress;

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
pub use web::{WebConfig, WebPlatform, browser_identity};

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub use desktop_environment::desktop_environment_id;

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod desktop_environment {
    use std::{
        io::Read,
        path::{Path, PathBuf},
    };

    use t3_client::types::EnvironmentId;

    /// Reads the environment identity written by a server sharing the desktop's T3 home.
    pub fn desktop_environment_id() -> Option<EnvironmentId> {
        let configured = std::env::var("T3CODE_HOME").ok();
        let development = std::env::var("VITE_DEV_SERVER_URL")
            .ok()
            .is_some_and(|url| url::Url::parse(url.trim()).is_ok());
        let state = state_dir(
            configured.as_deref(),
            development,
            std::env::home_dir().as_deref(),
        )?;
        read_environment_id(&state)
    }

    // Mirrors DesktopStatePaths.ts. An explicit home always uses userdata.
    fn state_dir(
        configured: Option<&str>,
        development: bool,
        home: Option<&Path>,
    ) -> Option<PathBuf> {
        let configured = configured
            .map(t3_contracts::wire::trim_js)
            .filter(|value| !value.is_empty());
        if let Some(configured) = configured {
            return Some(Path::new(configured).join("userdata"));
        }
        let subdir = if development { "dev" } else { "userdata" };
        home.map(|home| home.join(".t3").join(subdir))
    }

    fn read_environment_id(state_dir: &Path) -> Option<EnvironmentId> {
        const MAX_ID_FILE_BYTES: u64 = 4096;
        let file = std::fs::File::open(state_dir.join("environment-id")).ok()?;
        let mut value = String::new();
        file.take(MAX_ID_FILE_BYTES + 1)
            .read_to_string(&mut value)
            .ok()?;
        if value.len() as u64 > MAX_ID_FILE_BYTES {
            return None;
        }
        let id = t3_contracts::wire::trim_js(&value);
        (!id.is_empty() && !id.chars().any(char::is_control)).then(|| EnvironmentId(id.to_owned()))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn desktop_identity_rejects_oversized_files_and_embedded_controls() {
            let state = tempfile::tempdir().expect("temporary state directory");
            let file = state.path().join("environment-id");
            for contents in [
                "a".repeat(4097),
                "local\nother".into(),
                "local\0other".into(),
            ] {
                std::fs::write(&file, contents).expect("write invalid ID");
                assert!(read_environment_id(state.path()).is_none());
            }
        }

        #[test]
        fn desktop_identity_comes_from_the_resolved_state_directory() {
            let home = tempfile::tempdir().expect("temporary home");
            let configured = home.path().join("configured");
            for (home_setting, development, expected) in [
                (
                    Some(format!("  {}  ", configured.display())),
                    true,
                    configured.join("userdata"),
                ),
                (Some("  ".to_owned()), true, home.path().join(".t3/dev")),
                (None, false, home.path().join(".t3/userdata")),
            ] {
                let state = state_dir(home_setting.as_deref(), development, Some(home.path()))
                    .expect("state directory resolves");
                assert_eq!(state, expected);
                std::fs::create_dir_all(&state).expect("create fixture directory");
                let file = state.join("environment-id");
                assert_eq!(read_environment_id(&state), None);
                std::fs::write(&file, "  local-environment\r\n").expect("write ID");
                assert_eq!(
                    read_environment_id(&state),
                    Some(EnvironmentId("local-environment".into()))
                );
                std::fs::write(&file, " \t\r\n").expect("write blank ID");
                assert_eq!(read_environment_id(&state), None);
                std::fs::write(&file, [0xff]).expect("write invalid UTF-8");
                assert_eq!(read_environment_id(&state), None);
            }
        }
    }
}
