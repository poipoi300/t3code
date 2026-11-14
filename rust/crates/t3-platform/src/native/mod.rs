//! Native host: Windows, macOS, Linux, Android and iOS on tokio.
//!
//! Open it inside a tokio runtime and poll the client's futures on that runtime (a
//! current-thread runtime driving them, or any executor while a multi-thread runtime runs).
//! Network work runs as runtime tasks and storage on its blocking pool; dropping a returned
//! future cancels the request, and a dropped storage call still commits or rolls back whole.
mod files;
mod history;
pub use files::FileDialog;
#[cfg(any(target_os = "windows", target_os = "macos", test))]
mod proxy;
mod host;
mod http;
mod secret;
mod socket;
mod store;

#[cfg(target_os = "ios")]
pub use secret::KeychainStore;
#[cfg(target_os = "android")]
pub use secret::{AndroidKeystoreStore, set_android_vm};
pub use secret::{EphemeralSecretStore, SecretKeyStore, SecretStoreError};
pub use store::{OpenError, OpenReport};

use crate::common::{EventSender, HostExtension};
use futures::{FutureExt, channel::oneshot, future::LocalBoxFuture, stream::LocalBoxStream};
use std::{
    future::Future,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use t3_client::platform::{
    HostAction, HostFuture, HostReply, HttpRequest, HttpResponse, LegacyStorage, Platform,
    PlatformError, PlatformEvent, SocketRequest, Storage, StorageArea, StorageKey,
    StorageTransaction, StorageWrite, WebSocket,
};
use t3_contracts::auth::AuthClientPresentationMetadata;
use tokio::runtime::Handle;

#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    /// TCP, TLS and WebSocket handshake.
    pub connect: Duration,
    /// Whole HTTP exchange, body included; the client also races its own shorter timers.
    pub http: Duration,
    /// Waiting for the peer's close frame before the socket is dropped.
    pub close: Duration,
}
impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(15),
            http: Duration::from_secs(120),
            close: Duration::from_secs(5),
        }
    }
}

pub struct NativeConfig {
    /// App-private data directory chosen by the shell (app data on desktop, the sandbox's
    /// files directory on Android and iOS).
    pub data_dir: PathBuf,
    pub secrets: Box<dyn SecretKeyStore>,
    /// Surface, app version and device details only the shell knows.
    pub presentation: Option<AuthClientPresentationMetadata>,
    /// Read-only access to the previous app's stores, when the shell can reach them.
    pub legacy: Option<Rc<dyn LegacyStorage>>,
    pub extension: Option<Rc<dyn HostExtension>>,
    pub timeouts: Timeouts,
    /// The link the app was launched with (a mobile deep link, a claimed URL), if any; the
    /// app's location starts there.
    pub launch_url: Option<String>,
    /// DER certificates trusted in addition to the OS trust store on desktop (or the Mozilla
    /// roots on mobile): a deployment's own CA, or a test's.
    pub extra_roots: Vec<Vec<u8>>,
    /// The file picker for attachments: the OS dialog on desktop; on mobile the shell's
    /// extension answers instead, until a native picker lands.
    pub file_dialog: Option<Box<dyn FileDialog>>,
}
impl NativeConfig {
    pub fn new(data_dir: impl Into<PathBuf>, secrets: Box<dyn SecretKeyStore>) -> Self {
        Self {
            data_dir: data_dir.into(),
            secrets,
            presentation: None,
            legacy: None,
            extension: None,
            launch_url: None,
            extra_roots: Vec::new(),
            timeouts: Timeouts::default(),
            file_dialog: None,
        }
    }
    /// Desktop default: the storage key lives in the OS credential store.
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    pub fn desktop(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            file_dialog: Some(Box::new(files::SystemFileDialog)),
            ..Self::new(data_dir, <dyn SecretKeyStore>::os_default())
        }
    }
    /// Mobile default: on Android the storage key is wrapped by an Android Keystore key (call
    /// `set_android_vm` first); on iOS it is a keychain item.
    #[cfg(target_os = "android")]
    pub fn mobile(data_dir: impl Into<PathBuf>) -> Result<Self, SecretStoreError> {
        let data_dir = data_dir.into();
        let secrets = AndroidKeystoreStore::new(data_dir.join("keys"))?;
        Ok(Self::new(data_dir, Box::new(secrets)))
    }
    /// Mobile default: on Android the storage key is wrapped by an Android Keystore key (call
    /// `set_android_vm` first); on iOS it is a keychain item.
    #[cfg(target_os = "ios")]
    pub fn mobile(data_dir: impl Into<PathBuf>) -> Result<Self, SecretStoreError> {
        Ok(Self::new(data_dir, Box::new(KeychainStore::new())))
    }
}

struct Inner {
    store: Arc<store::Store>,
    http: http::HttpClient,
    tls: Arc<rustls::ClientConfig>,
    runtime: Handle,
    events: EventSender,
    epoch: tokio::time::Instant,
    timeouts: Timeouts,
    presentation: Option<AuthClientPresentationMetadata>,
    legacy: Option<Rc<dyn LegacyStorage>>,
    extension: Option<Rc<dyn HostExtension>>,
    host: host::HostState,
    report: OpenReport,
    launch_url: Option<String>,
    history: std::cell::RefCell<history::MemoryHistory>,
    file_dialog: Option<Box<dyn FileDialog>>,
    picked: files::PickedFiles,
}

#[derive(Clone)]
pub struct NativePlatform {
    inner: Rc<Inner>,
}
impl std::fmt::Debug for NativePlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativePlatform")
            .field("report", &self.inner.report)
            .finish_non_exhaustive()
    }
}

/// Desktop trusts the OS store (Windows CryptoAPI, macOS Security framework, Linux's system
/// bundle), so a server behind a private or enterprise CA connects as it does with the
/// TypeScript desktop app. Mobile keeps the Mozilla roots until the Android verifier lands.
fn tls_config(extra_roots: Vec<Vec<u8>>) -> Result<rustls::ClientConfig, OpenError> {
    let network = |error: rustls::Error| OpenError::Network(error.to_string());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(network)?;
    let extra = extra_roots
        .into_iter()
        .map(rustls::pki_types::CertificateDer::from);
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    {
        let verifier = rustls_platform_verifier::Verifier::new_with_extra_roots(extra, provider)
            .map_err(network)?;
        Ok(builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = provider;
        let mut roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        for root in extra {
            roots.add(root).map_err(network)?;
        }
        Ok(builder.with_root_certificates(roots).with_no_client_auth())
    }
}

/// Runs on the runtime even when the caller polls from another executor; drop aborts it.
pub(crate) fn spawn_abortable<T: Send + 'static>(
    runtime: &Handle,
    work: impl Future<Output = Result<T, PlatformError>> + Send + 'static,
) -> impl Future<Output = Result<T, PlatformError>> + 'static {
    struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);
    impl<T> Drop for AbortOnDrop<T> {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let mut task = AbortOnDrop(runtime.spawn(work));
    async move { (&mut task.0).await.map_err(|_| PlatformError::Cancelled)? }
}

impl NativePlatform {
    pub async fn open(config: NativeConfig) -> Result<Self, OpenError> {
        let runtime = Handle::try_current().map_err(|_| OpenError::NoRuntime)?;
        let NativeConfig {
            data_dir,
            secrets,
            presentation,
            legacy,
            extension,
            timeouts,
            launch_url,
            extra_roots,
            file_dialog,
        } = config;
        // A plain thread: OS key stores may block, and some start their own async runtime.
        let (tx, rx) = oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(store::Store::open(&data_dir, secrets.as_ref()));
        });
        let (store, report) = rx.await.map_err(|_| OpenError::Worker)??;
        let tls = Arc::new(tls_config(extra_roots)?);
        let http = http::HttpClient::new(tls.clone(), &timeouts)
            .map_err(|e| OpenError::Network(e.to_string()))?;
        Ok(Self {
            inner: Rc::new(Inner {
                store: Arc::new(store),
                http,
                tls,
                runtime,
                events: EventSender::default(),
                epoch: tokio::time::Instant::now(),
                timeouts,
                presentation,
                legacy,
                extension,
                host: host::HostState::default(),
                report,
                history: std::cell::RefCell::new(history::MemoryHistory::new(
                    launch_url
                        .as_deref()
                        .and_then(t3_client::navigation::app_path),
                )),
                launch_url,
                file_dialog,
                picked: files::PickedFiles::default(),
            }),
        })
    }

    /// The shell feeds network reachability, foreground/background and credential changes here.
    pub fn event_sender(&self) -> EventSender {
        self.inner.events.clone()
    }

    pub fn open_report(&self) -> OpenReport {
        self.inner.report
    }

    /// The shell hands over a link the OS delivered while the app runs (a mobile deep link or
    /// a claimed URL): an app address becomes the next location, and every link is announced
    /// as `PlatformEvent::OpenedLink` (pairing links are read from it).
    pub fn open_link(&self, link: String) {
        if let Some(path) = t3_client::navigation::app_path(&link) {
            self.inner.history.borrow_mut().push(path);
        }
        self.inner.events.emit(PlatformEvent::OpenedLink(link));
    }

    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(&store::Store, &EventSender) -> Result<T, PlatformError> + Send + 'static,
    ) -> Result<T, PlatformError> {
        let store = self.inner.store.clone();
        let events = self.inner.events.clone();
        self.inner
            .runtime
            .spawn_blocking(move || work(&store, &events))
            .await
            .map_err(|_| PlatformError::Storage("storage worker stopped".into()))?
    }
}

impl Storage for NativePlatform {
    fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
        self.blocking(move |store, _| store.get(area, &key))
            .boxed_local()
    }

    fn write(&self, area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()> {
        self.blocking(move |store, events| {
            events.storage_changed(store.write(area, writes)?);
            Ok(())
        })
        .boxed_local()
    }

    fn list(&self, area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>> {
        self.blocking(move |store, _| store.list(area, &namespace))
            .boxed_local()
    }

    fn transaction(&self, transaction: StorageTransaction) -> HostFuture<'_, bool> {
        self.blocking(move |store, events| match store.transaction(transaction)? {
            Some(changed) => {
                events.storage_changed(changed);
                Ok(true)
            }
            None => Ok(false),
        })
        .boxed_local()
    }
}

impl LegacyStorage for NativePlatform {
    fn list_legacy(&self, source: String, namespace: String) -> HostFuture<'_, Vec<String>> {
        match &self.inner.legacy {
            Some(legacy) => legacy.list_legacy(source, namespace),
            None => futures::future::ready(Err(no_legacy())).boxed_local(),
        }
    }
    fn read_legacy(
        &self,
        source: String,
        namespace: String,
        key: String,
    ) -> HostFuture<'_, Option<Vec<u8>>> {
        match &self.inner.legacy {
            Some(legacy) => legacy.read_legacy(source, namespace, key),
            None => futures::future::ready(Err(no_legacy())).boxed_local(),
        }
    }
    fn read_legacy_resource(
        &self,
        source: String,
        reference: String,
    ) -> HostFuture<'_, Option<Vec<u8>>> {
        match &self.inner.legacy {
            Some(legacy) => legacy.read_legacy_resource(source, reference),
            None => futures::future::ready(Err(no_legacy())).boxed_local(),
        }
    }
}
fn no_legacy() -> PlatformError {
    PlatformError::Unsupported(
        "legacy stores are not reachable on this host; the shell must provide them".into(),
    )
}

impl Platform for NativePlatform {
    fn now_epoch_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as u64)
    }

    fn monotonic_ms(&self) -> u64 {
        self.inner.epoch.elapsed().as_millis() as u64
    }

    fn external_url_surface(&self) -> t3_client::persistence::external_links::ExternalUrlSurface {
        if cfg!(any(target_os = "android", target_os = "ios")) {
            t3_client::persistence::external_links::ExternalUrlSurface::Mobile
        } else {
            t3_client::persistence::external_links::ExternalUrlSurface::Desktop
        }
    }
    fn random_bytes(&self, length: usize) -> Result<Vec<u8>, PlatformError> {
        use ring::rand::SecureRandom;
        let mut bytes = vec![0; length];
        ring::rand::SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| PlatformError::RandomUnavailable)?;
        Ok(bytes)
    }

    fn sleep(&self, milliseconds: u64) -> LocalBoxFuture<'_, ()> {
        let _runtime = self.inner.runtime.enter();
        tokio::time::sleep(Duration::from_millis(milliseconds)).boxed_local()
    }

    fn yield_to_host(&self) -> LocalBoxFuture<'_, ()> {
        tokio::task::yield_now().boxed_local()
    }

    fn events(&self) -> LocalBoxStream<'static, PlatformEvent> {
        self.inner.events.subscribe()
    }
    fn can_pick_resources(&self) -> bool {
        self.inner.file_dialog.is_some()
    }
    fn set_route_policy(&self, policy: t3_client::navigation::RoutePolicy) {
        self.inner.history.borrow_mut().set_policy(policy);
    }
    fn storage_report(&self) -> t3_client::platform::StorageReport {
        let report = self.inner.report;
        t3_client::platform::StorageReport {
            discarded_secure_records: report.discarded_secure_records,
            replaced_unreadable_key: report.replaced_unreadable_key,
            recreated_database: report.recreated_database,
            retained_secure_records: report.retained_secure_records,
            recovered_secure_records: report.recovered_secure_records,
        }
    }
    fn app_url(&self) -> HostFuture<'_, Option<String>> {
        let launch = self.inner.launch_url.clone();
        async move { Ok(launch) }.boxed_local()
    }
    fn app_url_changes(
        &self,
    ) -> LocalBoxStream<'static, Result<t3_client::navigation::AppUrlSnapshot, PlatformError>> {
        self.inner.history.borrow_mut().changes()
    }
    fn navigate_app_url(
        &self,
        request: t3_client::navigation::AppUrlRequest,
    ) -> HostFuture<'_, ()> {
        self.inner.history.borrow_mut().apply(request);
        async { Ok(()) }.boxed_local()
    }

    fn http(&self, request: HttpRequest) -> HostFuture<'_, HttpResponse> {
        let client = self.inner.http.clone();
        spawn_abortable(
            &self.inner.runtime,
            async move { client.send(request).await },
        )
        .boxed_local()
    }

    fn websocket(&self, request: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>> {
        let connect = spawn_abortable(
            &self.inner.runtime,
            socket::connect(request, self.inner.tls.clone(), self.inner.timeouts),
        );
        async move { Ok(Box::new(connect.await?) as Box<dyn WebSocket>) }.boxed_local()
    }

    fn host(&self, action: HostAction) -> HostFuture<'_, HostReply> {
        host::dispatch(self, action).boxed_local()
    }
}
