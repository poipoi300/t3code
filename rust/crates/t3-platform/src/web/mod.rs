//! Browser host (`wasm32-unknown-unknown`, window context).
//!
//! Storage is the IndexedDB database `t3code:client`; the TypeScript client's databases are
//! read-only legacy sources (see the README for the mapping).
mod files;
mod history;
mod host;
mod http;
mod idb;
mod js;
mod socket;
mod timer;

use crate::common::{EventSender, HostExtension};
use futures::{FutureExt, future::LocalBoxFuture, stream::LocalBoxStream};
use std::rc::Rc;
use t3_client::platform::{
    HostAction, HostFuture, HostReply, HttpRequest, HttpResponse, LegacyStorage, NetworkStatus,
    Platform, PlatformError, PlatformEvent, SocketRequest, Storage, StorageArea, StorageKey,
    StorageTransaction, StorageWrite, WebSocket,
};
use t3_contracts::auth::AuthClientPresentationMetadata;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{BroadcastChannel, IdbDatabase, IdbFactory, MessageEvent, VisibilityState, Window};

pub const DEFAULT_DATABASE: &str = "t3code:client";
/// The legacy source name the web host answers for.
pub const LEGACY_SOURCE: &str = "web";

/// What this browser says about itself (user agent, platform, touch points), for
/// `t3_client::connection::presentation::web`. Empty fields where the page has no window.
pub fn browser_identity() -> t3_client::connection::presentation::BrowserIdentity {
    let navigator = web_sys::window().map(|window| window.navigator());
    t3_client::connection::presentation::BrowserIdentity {
        user_agent: navigator
            .as_ref()
            .and_then(|navigator| navigator.user_agent().ok())
            .unwrap_or_default(),
        platform: navigator
            .as_ref()
            .and_then(|navigator| navigator.platform().ok())
            .unwrap_or_default(),
        max_touch_points: navigator.map_or(0, |navigator| {
            u32::try_from(navigator.max_touch_points()).unwrap_or(0)
        }),
    }
}

pub struct WebConfig {
    pub database: String,
    pub presentation: Option<AuthClientPresentationMetadata>,
    pub extension: Option<Rc<dyn HostExtension>>,
}
impl Default for WebConfig {
    fn default() -> Self {
        Self {
            database: DEFAULT_DATABASE.into(),
            presentation: None,
            extension: None,
        }
    }
}

struct Inner {
    window: Window,
    factory: IdbFactory,
    database: IdbDatabase,
    channel: Option<BroadcastChannel>,
    events: EventSender,
    listeners: js::Listeners,
    presentation: Option<AuthClientPresentationMetadata>,
    extension: Option<Rc<dyn HostExtension>>,
    dpop: futures::lock::Mutex<Option<host::BrowserDpopKey>>,
    history: history::BrowserHistory,
    picked: files::PickedFiles,
}
impl Drop for Inner {
    fn drop(&mut self) {
        js::remove_listeners(&mut self.listeners);
        if let Some(channel) = &self.channel {
            channel.close();
        }
        self.database.close();
    }
}

#[derive(Clone)]
pub struct WebPlatform {
    inner: Rc<Inner>,
}
impl std::fmt::Debug for WebPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WebPlatform")
    }
}

fn unavailable(what: &str) -> PlatformError {
    PlatformError::Unsupported(format!("{what} is unavailable in this browser context"))
}

impl WebPlatform {
    pub async fn open(config: WebConfig) -> Result<Self, PlatformError> {
        let window = web_sys::window().ok_or_else(|| unavailable("window"))?;
        let factory = window
            .indexed_db()
            .ok()
            .flatten()
            .ok_or_else(|| unavailable("IndexedDB"))?;
        let database = idb::open(&factory, &config.database).await?;
        let events = EventSender::default();
        let mut listeners = Vec::new();
        for (name, status) in [
            ("online", NetworkStatus::Online),
            ("offline", NetworkStatus::Offline),
        ] {
            let events = events.clone();
            listeners.push(js::listen(&window, name, move |_| {
                events.emit(PlatformEvent::Network(status));
            })?);
        }
        if let Some(document) = window.document() {
            let events = events.clone();
            let observed = document.clone();
            listeners.push(js::listen(&document, "visibilitychange", move |_| {
                events.emit(match observed.visibility_state() {
                    VisibilityState::Hidden => PlatformEvent::Suspended,
                    _ => PlatformEvent::Foreground,
                });
            })?);
        }
        // Other tabs of this profile announce committed keys; this tab announces its own.
        let channel = BroadcastChannel::new(&format!("{}:changes", config.database)).ok();
        if let Some(channel) = &channel {
            let events = events.clone();
            listeners.push(js::listen(channel, "message", move |event| {
                let key = event
                    .dyn_ref::<MessageEvent>()
                    .and_then(|event| event.data().as_string())
                    .and_then(|raw| StorageKey::decode_tuple(&raw).ok());
                if let Some(key) = key {
                    events.emit(PlatformEvent::StorageChanged(key));
                }
            })?);
        }
        let (history, popstate) = history::BrowserHistory::new(window.clone())?;
        listeners.push(popstate);
        Ok(Self {
            inner: Rc::new(Inner {
                history,
                window,
                factory,
                database,
                channel,
                events,
                listeners,
                presentation: config.presentation,
                extension: config.extension,
                dpop: futures::lock::Mutex::new(None),
                picked: files::PickedFiles::default(),
            }),
        })
    }

    /// The shell can add lifecycle events the browser does not raise itself.
    pub fn event_sender(&self) -> EventSender {
        self.inner.events.clone()
    }

    /// `navigator.onLine` as the supervisor's starting network status.
    pub fn network_status(&self) -> NetworkStatus {
        if self.inner.window.navigator().on_line() {
            NetworkStatus::Online
        } else {
            NetworkStatus::Offline
        }
    }

    fn subtle(&self) -> Result<web_sys::SubtleCrypto, PlatformError> {
        Ok(self
            .inner
            .window
            .crypto()
            .map_err(|_| unavailable("WebCrypto"))?
            .subtle())
    }

    fn announce(&self, keys: Vec<StorageKey>) {
        if let Some(channel) = &self.inner.channel {
            for key in &keys {
                if let Ok(raw) = key.encode_tuple() {
                    let _ = channel.post_message(&JsValue::from_str(&raw));
                }
            }
        }
        self.inner.events.storage_changed(keys);
    }
}

impl Storage for WebPlatform {
    fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
        async move { idb::get(&self.inner.database, area, &key).await }.boxed_local()
    }

    fn write(&self, area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()> {
        async move {
            idb::write(&self.inner.database, area, &writes).await?;
            self.announce(writes.into_iter().map(|write| write.key).collect());
            Ok(())
        }
        .boxed_local()
    }

    fn list(&self, area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>> {
        async move { idb::list(&self.inner.database, area, &namespace).await }.boxed_local()
    }

    fn transaction(&self, transaction: StorageTransaction) -> HostFuture<'_, bool> {
        async move {
            let keys: Vec<StorageKey> = transaction
                .ordinary
                .iter()
                .chain(&transaction.secure)
                .map(|write| write.key.clone())
                .collect();
            let committed = idb::transact(&self.inner.database, transaction).await?;
            if committed {
                self.announce(keys);
            }
            Ok(committed)
        }
        .boxed_local()
    }
}

enum LegacyNamespace {
    IndexedDb { database: String, store: String },
    Local,
    Session,
}
fn legacy_namespace(source: &str, namespace: &str) -> Result<LegacyNamespace, PlatformError> {
    if source != LEGACY_SOURCE {
        return Err(PlatformError::Unsupported(format!(
            "legacy source {source} on the web host"
        )));
    }
    match namespace {
        "localStorage" => Ok(LegacyNamespace::Local),
        "sessionStorage" => Ok(LegacyNamespace::Session),
        other => match other
            .strip_prefix("indexeddb/")
            .and_then(|rest| rest.rsplit_once('/'))
        {
            Some((database, store)) if !database.is_empty() && !store.is_empty() => {
                Ok(LegacyNamespace::IndexedDb {
                    database: database.into(),
                    store: store.into(),
                })
            }
            _ => Err(PlatformError::InvalidData("legacy namespace".into())),
        },
    }
}

impl WebPlatform {
    fn web_storage(&self, session: bool) -> Result<Option<web_sys::Storage>, PlatformError> {
        let storage = if session {
            self.inner.window.session_storage()
        } else {
            self.inner.window.local_storage()
        };
        storage.map_err(|_| PlatformError::Denied("site storage is blocked".into()))
    }
}

impl LegacyStorage for WebPlatform {
    /// Namespaces: `indexeddb/<database>/<store>`, `localStorage`, `sessionStorage`.
    fn list_legacy(&self, source: String, namespace: String) -> HostFuture<'_, Vec<String>> {
        async move {
            match legacy_namespace(&source, &namespace)? {
                LegacyNamespace::IndexedDb { database, store } => {
                    let Some(database) = idb::open_existing(&self.inner.factory, &database).await?
                    else {
                        return Ok(Vec::new());
                    };
                    let keys = idb::legacy_keys(&database, &store).await;
                    database.close();
                    keys
                }
                namespace => {
                    let session = matches!(namespace, LegacyNamespace::Session);
                    let Some(storage) = self.web_storage(session)? else {
                        return Ok(Vec::new());
                    };
                    let length = storage.length().unwrap_or(0);
                    Ok((0..length)
                        .filter_map(|index| storage.key(index).ok().flatten())
                        .collect())
                }
            }
        }
        .boxed_local()
    }

    fn read_legacy(
        &self,
        source: String,
        namespace: String,
        key: String,
    ) -> HostFuture<'_, Option<Vec<u8>>> {
        async move {
            match legacy_namespace(&source, &namespace)? {
                LegacyNamespace::IndexedDb { database, store } => {
                    let Some(database) = idb::open_existing(&self.inner.factory, &database).await?
                    else {
                        return Ok(None);
                    };
                    let value = idb::legacy_value(&database, &store, &key).await;
                    database.close();
                    match value? {
                        Some(value) => idb::legacy_bytes(&value),
                        None => Ok(None),
                    }
                }
                namespace => {
                    let session = matches!(namespace, LegacyNamespace::Session);
                    let Some(storage) = self.web_storage(session)? else {
                        return Ok(None);
                    };
                    Ok(storage
                        .get_item(&key)
                        .ok()
                        .flatten()
                        .map(String::into_bytes))
                }
            }
        }
        .boxed_local()
    }

    /// Browser profiles hold no attachment files; references resolve to nothing.
    fn read_legacy_resource(
        &self,
        source: String,
        _reference: String,
    ) -> HostFuture<'_, Option<Vec<u8>>> {
        async move {
            legacy_namespace(&source, "localStorage")?;
            Ok(None)
        }
        .boxed_local()
    }
}

impl Platform for WebPlatform {
    fn now_epoch_ms(&self) -> u64 {
        js_sys::Date::now() as u64
    }

    fn monotonic_ms(&self) -> u64 {
        self.inner
            .window
            .performance()
            .map_or_else(|| js_sys::Date::now() as u64, |p| p.now() as u64)
    }

    fn external_url_surface(&self) -> t3_client::persistence::external_links::ExternalUrlSurface {
        t3_client::persistence::external_links::ExternalUrlSurface::Web
    }
    fn random_bytes(&self, length: usize) -> Result<Vec<u8>, PlatformError> {
        let crypto = self
            .inner
            .window
            .crypto()
            .map_err(|_| unavailable("crypto.getRandomValues"))?;
        let mut bytes = vec![0; length];
        // getRandomValues fills at most 65536 bytes per call.
        for chunk in bytes.chunks_mut(65_536) {
            crypto
                .get_random_values_with_u8_array(chunk)
                .map_err(|_| unavailable("crypto.getRandomValues"))?;
        }
        Ok(bytes)
    }

    fn sleep(&self, milliseconds: u64) -> LocalBoxFuture<'_, ()> {
        timer::sleep(self.inner.window.clone(), milliseconds).boxed_local()
    }

    fn yield_to_host(&self) -> LocalBoxFuture<'_, ()> {
        timer::sleep(self.inner.window.clone(), 0).boxed_local()
    }

    fn events(&self) -> LocalBoxStream<'static, PlatformEvent> {
        self.inner.events.subscribe()
    }
    fn can_pick_resources(&self) -> bool {
        true
    }
    fn set_route_policy(&self, policy: t3_client::navigation::RoutePolicy) {
        self.inner.history.set_policy(policy);
    }
    fn app_url(&self) -> HostFuture<'_, Option<String>> {
        let launch = self.inner.history.launch_url();
        async move { Ok(launch) }.boxed_local()
    }
    fn app_url_changes(
        &self,
    ) -> LocalBoxStream<'static, Result<t3_client::navigation::AppUrlSnapshot, PlatformError>> {
        self.inner.history.changes()
    }
    fn navigate_app_url(
        &self,
        request: t3_client::navigation::AppUrlRequest,
    ) -> HostFuture<'_, ()> {
        async move { self.inner.history.apply(request).await }.boxed_local()
    }

    fn http(&self, request: HttpRequest) -> HostFuture<'_, HttpResponse> {
        http::fetch(&self.inner.window, request).boxed_local()
    }

    fn websocket(&self, request: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>> {
        let window = self.inner.window.clone();
        async move { Ok(Box::new(socket::connect(window, request).await?) as Box<dyn WebSocket>) }
            .boxed_local()
    }

    fn host(&self, action: HostAction) -> HostFuture<'_, HostReply> {
        host::dispatch(self, action).boxed_local()
    }
}
