//! The production backend: t3-client's composition root over t3-platform's host for this
//! target, its futures on NSH's executor, saved environments restored at start-up.

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::FutureExt;
use futures::future::LocalBoxFuture;
use nsh_core::prelude::*;
use t3_client::ClientStore;
use t3_client::composition::{ClientRuntime, Spawner};
use t3_client::platform::Platform;
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;

use super::backend::Unavailable;
use super::host::{Choice, Started};

use crate::runtime::tasks::watch;

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
            watch(future);
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
        let (runtime, paired) = start_on(platform, store, host::SOURCE, spawner).await;
        Started {
            backend: Rc::new(runtime),
            choice: Choice::Client,
            paired,
        }
    }
    .boxed_local()
}

/// The client over an opened platform. The saved environments are restored first: restoring
/// settles the catalog (a damaged one is set aside, and the runtime keeps that for its one-time
/// storage notice), and starts each environment's connection without waiting for its server.
/// Whether an environment is saved is then read from the runtime, never from a second load.
pub async fn start_on<P: Platform + Clone + 'static>(
    platform: P,
    store: ClientStore,
    source: OrchestrationV2CreationSource,
    spawner: Spawner,
) -> (ClientRuntime<P>, bool) {
    let runtime = ClientRuntime::new(platform, store, source, spawner);
    // Per-environment failures are listed with their state; this is the catalogue.
    if let Err(error) = runtime.restore().await {
        log::warn!("t3-ui: restoring saved environments failed: {error}");
    }
    let paired = !runtime.environments().is_empty();
    (runtime, paired)
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

    /// The Android Keystore or the iOS Keychain.
    #[cfg(any(target_os = "android", target_os = "ios"))]
    fn config(dir: std::path::PathBuf) -> (NativeConfig, bool) {
        // Supply NSH's process VM before opening the Keystore. The NDK lookup needs API 31.
        #[cfg(target_os = "android")]
        if let Some(vm) = nsh::java_vm() {
            t3_platform::set_android_vm(vm);
        }
        mobile_config(
            || NativeConfig::mobile(dir.clone()).map_err(|error| error.to_string()),
            || in_memory_config(&dir),
        )
    }

    /// Keys in process memory, over a directory of its own emptied first: what a run without a
    /// key store saves is unreadable after it, so it must never share (and then discard) the rows
    /// a run with the key store saved, nor list environments it cannot authenticate.
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    fn in_memory_config(dir: &std::path::Path) -> NativeConfig {
        NativeConfig::new(
            fresh_in_memory_dir(dir),
            Box::new(t3_platform::EphemeralSecretStore::default()),
        )
    }

    /// `client-memory` beside the persistent `client` directory, emptied.
    #[cfg(any(
        test,
        not(any(target_os = "windows", target_os = "macos", target_os = "linux"))
    ))]
    fn fresh_in_memory_dir(dir: &std::path::Path) -> std::path::PathBuf {
        let memory = dir.with_file_name("client-memory");
        if let Err(error) = std::fs::remove_dir_all(&memory)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("t3-ui: the last run's in-memory storage was not cleared: {error}");
        }
        memory
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
        (in_memory_config(&dir), false)
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
        use super::{fresh_in_memory_dir, mobile_config};

        // Platform re-review 2, B2: the fallback shared the persistent directory and discarded the
        // rows a run with the Keystore had saved.
        #[test]
        fn the_in_memory_fallback_never_touches_the_persistent_directory() {
            let root =
                std::env::temp_dir().join(format!("t3-ui-memory-dir-{}", std::process::id()));
            let persistent = root.join("client");
            std::fs::create_dir_all(&persistent).expect("test directory");
            std::fs::write(persistent.join("client-storage.sqlite"), b"saved")
                .expect("test directory");
            std::fs::create_dir_all(root.join("client-memory")).expect("test directory");
            std::fs::write(
                root.join("client-memory").join("client-storage.sqlite"),
                b"last run",
            )
            .expect("test directory");
            let memory = fresh_in_memory_dir(&persistent);
            assert_eq!(memory, root.join("client-memory"));
            assert!(
                !memory.exists(),
                "the last run's in-memory rows are gone before it opens"
            );
            assert_eq!(
                std::fs::read(persistent.join("client-storage.sqlite")).expect("test directory"),
                b"saved"
            );
            std::fs::remove_dir_all(&root).expect("test directory");
        }

        // Integration 3 (U13): Android was given in-memory keys even with a working Keystore, so
        // nothing paired survived a restart.
        #[test]
        fn a_mobile_host_keeps_its_servers_when_its_key_store_opens() {
            assert_eq!(
                mobile_config(|| Ok("key store"), || "memory"),
                ("key store", true)
            );
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

#[cfg(test)]
mod tests {
    use super::start_on;
    use futures::executor::block_on;
    use std::rc::Rc;
    use t3_client::ClientStore;
    use t3_client::platform::{Storage, StorageArea, StorageKey, StorageWrite};
    use t3_client::testing::FakePlatform;
    use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;

    // Platform re-review 2, B1: start loaded the catalog once to decide `paired`, which set a
    // damaged catalog aside before `restore` could report it, so the notice never said so.
    #[test]
    fn starting_over_a_damaged_catalog_reports_the_reset_and_opens_unpaired() {
        let platform = FakePlatform::default();
        block_on(platform.write(
            StorageArea::Ordinary,
            vec![StorageWrite {
                key: StorageKey::new("connection-catalog", None, "current"),
                value: Some(b"{not a catalog".to_vec()),
            }],
        ))
        .expect("written");
        // Restore runs on a runtime-owned task, so the spawner must run what it is given.
        use futures::task::LocalSpawnExt;
        let mut pool = futures::executor::LocalPool::new();
        let spawner = pool.spawner();
        let (runtime, paired) = pool.run_until(start_on(
            platform,
            ClientStore::default(),
            OrchestrationV2CreationSource::Web,
            Rc::new(move |future| {
                let _ = spawner.spawn_local(future);
            }),
        ));
        assert!(!paired, "nothing usable is saved");
        let notice = runtime.take_storage_notice().expect("the user is told");
        assert!(notice.reset_catalog, "the notice says the list was reset");
    }
}
