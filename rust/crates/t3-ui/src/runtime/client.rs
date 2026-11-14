//! The production backend: t3-client's composition root over t3-platform's host for this
//! target, its futures on NSH's executor, saved environments restored at start-up.

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::FutureExt;
use futures::future::LocalBoxFuture;
use nsh_core::prelude::*;
use t3_client::ClientStore;
use t3_client::composition::{ClientRuntime, Spawner};
use t3_client::connection::Time;
use t3_client::environment::EnvironmentRegistry;
use t3_client::platform::NetworkStatus;
use t3_client::rpc::SessionEndpoint;

use super::backend::Unavailable;
use super::host::{Choice, Started};

/// Set once the client's storage opened with a key store that outlives the process.
static SAVED_SERVERS_LAST: AtomicBool = AtomicBool::new(false);

/// Whether servers paired now are still saved, with their credentials, at the next start on this
/// host. False for the demo, an unavailable host, and a mobile host without its OS key store.
pub fn saved_servers_last() -> bool {
    SAVED_SERVERS_LAST.load(Ordering::Relaxed)
}

pub fn start(store: ClientStore) -> LocalBoxFuture<'static, Started> {
    // The client's tasks live as long as the owner that started it (`App`).
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        });
    });
    async move {
        let platform = match host::open().await {
            Ok((platform, lasting)) => {
                SAVED_SERVERS_LAST.store(lasting, Ordering::Relaxed);
                platform
            }
            Err(reason) => {
                log::warn!("t3-ui: the client did not start: {reason}");
                return Started {
                    backend: Rc::new(Unavailable(reason)),
                    choice: Choice::Client,
                    paired: false,
                };
            }
        };
        let paired = EnvironmentRegistry::<SessionEndpoint>::load(
            &platform,
            NetworkStatus::Unknown,
            Time::from_platform(&platform),
        )
        .await
        .is_ok_and(|registry| registry.entries().next().is_some());
        let runtime = ClientRuntime::new(platform, store, host::SOURCE, spawner.clone());
        let restoring = runtime.clone();
        spawner(
            async move {
                // Per-environment failures are listed with their state; this is the catalogue.
                if let Err(error) = restoring.restore().await {
                    log::warn!("t3-ui: restoring saved environments failed: {error}");
                }
            }
            .boxed_local(),
        );
        Started {
            backend: Rc::new(runtime),
            choice: Choice::Client,
            paired,
        }
    }
    .boxed_local()
}

#[cfg(not(target_arch = "wasm32"))]
mod host {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::OnceLock;
    use std::task::{Context, Poll};

    use t3_client::connection::presentation;
    use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
    use t3_platform::{NativeConfig, NativePlatform};

    pub const SOURCE: OrchestrationV2CreationSource =
        if cfg!(any(target_os = "android", target_os = "ios")) {
            OrchestrationV2CreationSource::Mobile
        } else {
            OrchestrationV2CreationSource::Web
        };

    /// Runs the client's network and storage work; its futures are polled on NSH's executor.
    fn runtime() -> Result<&'static tokio::runtime::Runtime, String> {
        static RUNTIME: OnceLock<Result<tokio::runtime::Runtime, String>> = OnceLock::new();
        RUNTIME
            .get_or_init(|| {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .thread_name("t3-client-io")
                    .enable_all()
                    .build()
                    .map_err(|error| format!("The client's network runtime did not start: {error}"))
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// `future`, polled inside the runtime's context (`NativePlatform::open` reads it).
    struct Entered<F> {
        handle: tokio::runtime::Handle,
        future: Pin<Box<F>>,
    }

    impl<F: Future> Future for Entered<F> {
        type Output = F::Output;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
            let handle = self.handle.clone();
            let _entered = handle.enter();
            self.future.as_mut().poll(cx)
        }
    }

    /// The platform, and whether what it saves outlives this run.
    pub async fn open() -> Result<(NativePlatform, bool), String> {
        let dir = nsh_core::prelude::data_dir()
            .ok_or("This host keeps no app data, so the client cannot save servers.")?
            .join("client");
        let runtime = runtime()?;
        let (mut config, lasting) = config(dir);
        config.presentation = Some(presentation_for_host());
        Entered {
            handle: runtime.handle().clone(),
            future: Box::pin(NativePlatform::open(config)),
        }
        .await
        .map(|platform| (platform, lasting))
        .map_err(|error| format!("The client's storage did not open: {error}"))
    }

    /// A release build sets it; development builds report `0.0.0`, which the host does not show.
    const APP_VERSION: Option<&str> = option_env!("T3_APP_VERSION");

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    fn config(dir: std::path::PathBuf) -> (NativeConfig, bool) {
        (NativeConfig::desktop(dir), true)
    }

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    fn presentation_for_host() -> t3_contracts::auth::AuthClientPresentationMetadata {
        presentation::desktop(std::env::consts::OS, APP_VERSION.unwrap_or("0.0.0"))
    }

    /// The Android Keystore (t3-platform finds the Java VM itself, U13) or the iOS Keychain.
    #[cfg(any(target_os = "android", target_os = "ios"))]
    fn config(dir: std::path::PathBuf) -> (NativeConfig, bool) {
        mobile_config(
            || NativeConfig::mobile(dir.clone()).map_err(|error| error.to_string()),
            || NativeConfig::new(dir.clone(), Box::new(t3_platform::EphemeralSecretStore::default())),
        )
    }

    /// The OS key store when it opens; otherwise keys in memory, and saved servers last this run.
    #[cfg(any(target_os = "android", target_os = "ios", test))]
    fn mobile_config<C>(
        key_store: impl FnOnce() -> Result<C, String>,
        in_memory: impl FnOnce() -> C,
    ) -> (C, bool) {
        match key_store() {
            Ok(config) => (config, true),
            Err(error) => {
                log::warn!("t3-ui: no OS key store ({error}); saved servers last this run only");
                (in_memory(), false)
            }
        }
    }

    /// NSH reports the Android API level, not the release `apps/mobile` sends, so Android sends
    /// no version; NSH has no device model.
    #[cfg(any(target_os = "android", target_os = "ios"))]
    fn presentation_for_host() -> t3_contracts::auth::AuthClientPresentationMetadata {
        use nsh_core::platform::Idiom;
        use t3_contracts::auth::AuthClientMetadataDeviceType as DeviceType;
        let device = match nsh_core::prelude::idiom() {
            Idiom::Phone => DeviceType::Mobile,
            Idiom::Pad => DeviceType::Tablet,
            _ => DeviceType::Unknown,
        };
        let (major, minor) = nsh_core::prelude::os_version();
        let version = format!("{major}.{minor}");
        let (os, version) = if cfg!(target_os = "ios") {
            ("ios", Some(version.as_str()))
        } else {
            ("android", None)
        };
        presentation::mobile(os, version, None, device, APP_VERSION)
    }

    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux",
        target_os = "android",
        target_os = "ios"
    )))]
    fn config(dir: std::path::PathBuf) -> (NativeConfig, bool) {
        (
            NativeConfig::new(dir, Box::new(t3_platform::EphemeralSecretStore::default())),
            false,
        )
    }

    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux",
        target_os = "android",
        target_os = "ios"
    )))]
    fn presentation_for_host() -> t3_contracts::auth::AuthClientPresentationMetadata {
        presentation::desktop(std::env::consts::OS, APP_VERSION.unwrap_or("0.0.0"))
    }
    #[cfg(test)]
    mod tests {
        use super::mobile_config;

        // Integration 3 (U13): Android was given in-memory keys even with a working Keystore, so
        // nothing paired survived a restart.
        #[test]
        fn a_mobile_host_keeps_its_servers_when_its_key_store_opens() {
            assert_eq!(mobile_config(|| Ok("key store"), || "memory"), ("key store", true));
            assert_eq!(
                mobile_config(|| Err("no Java VM".to_owned()), || "memory"),
                ("memory", false),
                "without a key store the host says its servers last this run only"
            );
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod host {
    use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
    use t3_platform::{WebConfig, WebPlatform};

    pub const SOURCE: OrchestrationV2CreationSource = OrchestrationV2CreationSource::Web;

    /// The platform, and whether what it saves outlives this run (the browser's storage does).
    pub async fn open() -> Result<(WebPlatform, bool), String> {
        WebPlatform::open(WebConfig::default())
            .await
            .map(|platform| (platform, true))
            .map_err(|error| format!("The client's storage did not open: {error}"))
    }
}
