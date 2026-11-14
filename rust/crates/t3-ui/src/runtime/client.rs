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
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        let config = NativeConfig::desktop(dir);
        // No OS key store binding for mobile yet (P7): saved credentials last one run.
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        let config = NativeConfig::new(
            dir,
            Box::new(t3_platform::EphemeralSecretStore::default()),
        );
        Entered {
            handle: runtime.handle().clone(),
            future: Box::pin(NativePlatform::open(config)),
        }
        .await
        .map_err(|error| format!("The client's storage did not open: {error}"))
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
