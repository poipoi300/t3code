//! The production backend: t3-client's composition root over t3-platform's host for this
//! target, its futures on NSH's executor, saved environments restored at start-up.

use std::rc::Rc;

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
            Ok(platform) => platform,
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

    pub async fn open() -> Result<NativePlatform, String> {
        let dir = nsh_core::prelude::data_dir()
            .ok_or("This host keeps no app data, so the client cannot save servers.")?
            .join("client");
        let runtime = runtime()?;
        let mut config = config(dir);
        config.presentation = Some(presentation_for_host());
        Entered {
            handle: runtime.handle().clone(),
            future: Box::pin(NativePlatform::open(config)),
        }
        .await
        .map_err(|error| format!("The client's storage did not open: {error}"))
    }

    /// A release build sets it; development builds report `0.0.0`, which the host does not show.
    const APP_VERSION: Option<&str> = option_env!("T3_APP_VERSION");

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    fn config(dir: std::path::PathBuf) -> NativeConfig {
        NativeConfig::desktop(dir)
    }

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    fn presentation_for_host() -> t3_contracts::auth::AuthClientPresentationMetadata {
        presentation::desktop(std::env::consts::OS, APP_VERSION.unwrap_or("0.0.0"))
    }

    /// The OS key store (Android Keystore, iOS Keychain); without it saved credentials last one run.
    #[cfg(any(target_os = "android", target_os = "ios"))]
    fn config(dir: std::path::PathBuf) -> NativeConfig {
        #[cfg(target_os = "android")]
        if let Some(vm) = android::java_vm() {
            t3_platform::set_android_vm(vm);
        }
        NativeConfig::mobile(dir.clone()).unwrap_or_else(|error| {
            log::warn!("t3-ui: no OS key store ({error}); saved servers last this run only");
            NativeConfig::new(dir, Box::new(t3_platform::EphemeralSecretStore::default()))
        })
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
    fn config(dir: std::path::PathBuf) -> NativeConfig {
        NativeConfig::new(dir, Box::new(t3_platform::EphemeralSecretStore::default()))
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

    #[cfg(target_os = "android")]
    mod android {
        use jni::sys::{JNI_OK, JavaVM as RawVm, jint, jsize};

        type GetCreatedJavaVms = unsafe extern "system" fn(*mut *mut RawVm, jsize, *mut jsize) -> jint;

        /// The process's Java VM through the NDK's public `JNI_GetCreatedJavaVMs` (libnativehelper,
        /// API 31+), looked up at run time so older devices fall back to a one-run store. NSH keeps
        /// its own handle private (`nsh-platform-android/src/java.rs`).
        pub fn java_vm() -> Option<jni::JavaVM> {
            // SAFETY: a public NDK library and symbol with the JNI signature above; the VM pointer
            // it returns lives as long as the process.
            unsafe {
                let library = libc::dlopen(c"libnativehelper.so".as_ptr(), libc::RTLD_NOW);
                if library.is_null() {
                    return None;
                }
                let symbol = libc::dlsym(library, c"JNI_GetCreatedJavaVMs".as_ptr());
                if symbol.is_null() {
                    return None;
                }
                let get: GetCreatedJavaVms = std::mem::transmute(symbol);
                let mut vm = std::ptr::null_mut();
                let mut count: jsize = 0;
                if get(&mut vm, 1, &mut count) != JNI_OK || count < 1 || vm.is_null() {
                    return None;
                }
                jni::JavaVM::from_raw(vm).ok()
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod host {
    use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
    use t3_platform::{WebConfig, WebPlatform};

    pub const SOURCE: OrchestrationV2CreationSource = OrchestrationV2CreationSource::Web;

    pub async fn open() -> Result<WebPlatform, String> {
        WebPlatform::open(WebConfig::default())
            .await
            .map_err(|error| format!("The client's storage did not open: {error}"))
    }
}
