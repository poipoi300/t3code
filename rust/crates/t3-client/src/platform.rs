use futures::{FutureExt, StreamExt};
use futures::{future::LocalBoxFuture, stream::LocalBoxStream};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use t3_contracts::base_schemas::EnvironmentId;
pub type HostFuture<'a, T> = LocalBoxFuture<'a, Result<T, PlatformError>>;
#[derive(Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlatformError {
    #[error("unsupported host capability")]
    Unsupported(String),
    #[error("host permission denied")]
    Denied(String),
    #[error("host storage failed")]
    Storage(String),
    #[error("host network failed")]
    Network(String),
    #[error("resource closed")]
    Closed,
    #[error("operation cancelled")]
    Cancelled,
    #[error("invalid host data")]
    InvalidData(String),
    #[error("response exceeds byte limit")]
    TooLarge,
    /// The host has no secure random source; never substitute a weak one.
    #[error("secure randomness unavailable")]
    RandomUnavailable,
}
impl std::fmt::Debug for PlatformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unsupported(_) => "Unsupported([redacted])",
            Self::Denied(_) => "Denied([redacted])",
            Self::Storage(_) => "Storage([redacted])",
            Self::Network(_) => "Network([redacted])",
            Self::InvalidData(_) => "InvalidData([redacted])",
            Self::Closed => "Closed",
            Self::Cancelled => "Cancelled",
            Self::TooLarge => "TooLarge",
            Self::RandomUnavailable => "RandomUnavailable",
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StorageKey {
    pub namespace: String,
    pub environment: Option<EnvironmentId>,
    pub record: String,
}
impl StorageKey {
    pub fn new(
        namespace: impl Into<String>,
        environment: Option<EnvironmentId>,
        record: impl Into<String>,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            environment,
            record: record.into(),
        }
    }

    pub fn encode_tuple(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&(&self.namespace, &self.environment, &self.record))
    }

    pub fn decode_tuple(raw: &str) -> Result<Self, serde_json::Error> {
        let (namespace, environment, record) = serde_json::from_str(raw)?;
        Ok(Self {
            namespace,
            environment,
            record,
        })
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct StorageWrite {
    pub key: StorageKey,
    pub value: Option<Vec<u8>>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageArea {
    Ordinary,
    Secure,
}
#[derive(Clone, PartialEq, Eq)]
pub struct StorageCondition {
    pub area: StorageArea,
    pub key: StorageKey,
    pub expected: Option<Vec<u8>>,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct StorageTransaction {
    pub conditions: Vec<StorageCondition>,
    pub ordinary: Vec<StorageWrite>,
    pub secure: Vec<StorageWrite>,
}

pub trait Storage {
    fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>>;
    /// Writes apply in supplied order and commit atomically or leave the area unchanged.
    fn write(&self, area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()>;
    fn list(&self, area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>>;
    /// All conditions precede ordered writes; both areas commit atomically, or false means no writes.
    /// Hosts without a cross-area transaction must return Unsupported, never emulate a partial commit.
    fn transaction(&self, _transaction: StorageTransaction) -> HostFuture<'_, bool> {
        async {
            Err(PlatformError::Unsupported(
                "atomic storage transaction".into(),
            ))
        }
        .boxed_local()
    }
}

/// A read-only export boundary for the old host databases, key stores and document files.
pub trait LegacyStorage {
    fn list_legacy(&self, source: String, namespace: String) -> HostFuture<'_, Vec<String>>;
    fn read_legacy(
        &self,
        source: String,
        namespace: String,
        key: String,
    ) -> HostFuture<'_, Option<Vec<u8>>>;
    fn read_legacy_resource(
        &self,
        source: String,
        reference: String,
    ) -> HostFuture<'_, Option<Vec<u8>>>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Credentials {
    Omit,
    SameOrigin,
    Include,
}
/// Saved data the host's storage could not keep when it opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StorageReport {
    /// Secure records (credentials, tokens) discarded for good: their key is gone.
    pub discarded_secure_records: usize,
    /// The storage key was unreadable and was replaced.
    pub replaced_unreadable_key: bool,
    /// The database was damaged; it was moved aside and a new one started.
    pub recreated_database: bool,
    /// Secure records set aside because their key could not be read; they come back on a later
    /// launch if it reads again.
    pub retained_secure_records: usize,
    /// Records set aside earlier, back now that their key read again.
    pub recovered_secure_records: usize,
}

impl StorageReport {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
    pub credentials: Credentials,
    pub max_response_bytes: usize,
}
#[derive(Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}
#[derive(Clone)]
pub struct SocketRequest {
    pub url: String,
    pub protocols: Vec<String>,
}
pub trait WebSocket {
    fn send(&mut self, frame: t3_rpc::Frame) -> HostFuture<'_, ()>;
    /// The next frame, or `None` once the socket closed. Must be cancel-safe: the session
    /// races it against outgoing frames and timers, and a dropped `receive` future must not
    /// lose a frame it had already taken from the socket.
    fn receive(&mut self) -> HostFuture<'_, Option<t3_rpc::Frame>>;
    fn close(&mut self) -> HostFuture<'_, ()>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkStatus {
    Unknown,
    Offline,
    Online,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformEvent {
    Network(NetworkStatus),
    Foreground,
    Suspended,
    CredentialsChanged,
    RegistrationsChanged,
    StorageChanged(StorageKey),
    /// The OS handed the running app a link (a mobile deep link such as `t3code://…?pairingUrl=`,
    /// or a claimed web link). Navigation reads it from the host's app URL history; pairing
    /// links are picked out with `connection::pairing_link`.
    OpenedLink(String),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MimeData {
    pub mime: String,
    pub bytes: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub id: String,
    pub title: String,
    pub body: String,
    pub thread: Option<crate::types::ThreadKey>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Haptic {
    Selection,
    Impact,
    Success,
    Error,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostResource {
    pub handle: String,
    pub name: String,
    pub mime: String,
}
#[derive(Clone)]
pub enum HostAction {
    NotificationPermission,
    Notify(Notification),
    CancelNotification(String),
    ClipboardRead,
    ClipboardWrite(Vec<MimeData>),
    Haptic(Haptic),
    OpenUrl(String),
    /// A remote editor link (desktop: or a local editor file link), already checked by
    /// `external_links::safe_editor_link`; the web host navigates to it as TypeScript does.
    OpenEditorLink(String),
    PickResources {
        mime: Vec<String>,
        multiple: bool,
    },
    ReadResource {
        handle: String,
        max_bytes: usize,
    },
    WriteResource {
        resource: HostResource,
        bytes: Vec<u8>,
    },
    Share {
        title: String,
        resources: Vec<HostResource>,
    },
    MicrophonePermission,
    StartRecording {
        mime: String,
    },
    StopRecording,
    PlayAudio(MimeData),
    StopAudio,
    CloudIdentity,
    CloudToken,
    RelayDeviceId,
    AnalyticsDeviceId,
    ClientPresentation,
    PrimaryBearer,
    DpopThumbprint,
    DpopPublicJwk,
    DpopSign {
        signing_input: Vec<u8>,
    },
    SshProvision {
        target: t3_contracts::ipc::DesktopSshEnvironmentTargetSchema,
        expected: Option<EnvironmentId>,
    },
    SshPrepare {
        connection_id: String,
        target: t3_contracts::ipc::DesktopSshEnvironmentTargetSchema,
        expected: EnvironmentId,
        bearer: Option<String>,
    },
    SshDisconnect(t3_contracts::ipc::DesktopSshEnvironmentTargetSchema),
    Registrations,
    ClearEnvironment(EnvironmentId),
    /// Deletes an app-owned resource (a picked or recorded file the app copied); an already
    /// missing one is success.
    RemoveResource {
        handle: String,
    },
}
#[derive(Clone, PartialEq)]
pub enum HostReply {
    Unit,
    Granted(bool),
    Text(Option<String>),
    Bytes(Vec<u8>),
    Mime(Vec<MimeData>),
    Resources(Vec<HostResource>),
    Document(serde_json::Value),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSessionIdentity {
    pub account_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedSshEnvironment {
    pub bootstrap: t3_contracts::ipc::DesktopSshEnvironmentBootstrapSchema,
    pub bearer_token: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionedSshEnvironment {
    pub environment_id: EnvironmentId,
    pub label: String,
    pub bootstrap: t3_contracts::ipc::DesktopSshEnvironmentBootstrapSchema,
    pub bearer_token: String,
}
impl std::fmt::Debug for PreparedSshEnvironment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PreparedSshEnvironment([redacted])")
    }
}
impl std::fmt::Debug for ProvisionedSshEnvironment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProvisionedSshEnvironment([redacted])")
    }
}
impl std::fmt::Debug for StorageWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageWrite")
            .field("key", &self.key)
            .field("value_bytes", &self.value.as_ref().map(Vec::len))
            .finish()
    }
}
impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("body_bytes", &self.body.len())
            .field("credentials", &self.credentials)
            .field("max_response_bytes", &self.max_response_bytes)
            .finish_non_exhaustive()
    }
}
impl std::fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("body_bytes", &self.body.len())
            .finish_non_exhaustive()
    }
}
impl std::fmt::Debug for HostAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::NotificationPermission => "NotificationPermission",
            Self::Notify(_) => "Notify",
            Self::CancelNotification(_) => "CancelNotification",
            Self::ClipboardRead => "ClipboardRead",
            Self::ClipboardWrite(_) => "ClipboardWrite",
            Self::Haptic(_) => "Haptic",
            Self::OpenUrl(_) => "OpenUrl",
            Self::OpenEditorLink(_) => "OpenEditorLink",
            Self::PickResources { .. } => "PickResources",
            Self::ReadResource { .. } => "ReadResource",
            Self::WriteResource { .. } => "WriteResource",
            Self::Share { .. } => "Share",
            Self::MicrophonePermission => "MicrophonePermission",
            Self::StartRecording { .. } => "StartRecording",
            Self::StopRecording => "StopRecording",
            Self::PlayAudio(_) => "PlayAudio",
            Self::StopAudio => "StopAudio",
            Self::CloudIdentity => "CloudIdentity",
            Self::CloudToken => "CloudToken",
            Self::RelayDeviceId => "RelayDeviceId",
            Self::AnalyticsDeviceId => "AnalyticsDeviceId",
            Self::ClientPresentation => "ClientPresentation",
            Self::PrimaryBearer => "PrimaryBearer",
            Self::DpopThumbprint => "DpopThumbprint",
            Self::DpopPublicJwk => "DpopPublicJwk",
            Self::DpopSign { .. } => "DpopSign",
            Self::SshProvision { .. } => "SshProvision",
            Self::SshPrepare { .. } => "SshPrepare",
            Self::SshDisconnect(_) => "SshDisconnect",
            Self::Registrations => "Registrations",
            Self::ClearEnvironment(_) => "ClearEnvironment",
            Self::RemoveResource { .. } => "RemoveResource",
        };
        f.write_str(name)
    }
}
impl std::fmt::Debug for HostReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unit => f.write_str("Unit"),
            Self::Granted(v) => f.debug_tuple("Granted").field(v).finish(),
            Self::Text(_) => f.write_str("Text([redacted])"),
            Self::Document(_) => f.write_str("Document([redacted])"),
            Self::Bytes(v) => f.debug_tuple("Bytes").field(&v.len()).finish(),
            Self::Mime(v) => f.debug_tuple("Mime").field(&v.len()).finish(),
            Self::Resources(v) => f.debug_tuple("Resources").field(&v.len()).finish(),
        }
    }
}
pub trait Platform: Storage {
    fn now_epoch_ms(&self) -> u64;
    fn monotonic_ms(&self) -> u64;
    fn random_bytes(&self, length: usize) -> Result<Vec<u8>, PlatformError>;
    fn sleep(&self, milliseconds: u64) -> LocalBoxFuture<'_, ()>;
    fn yield_to_host(&self) -> LocalBoxFuture<'_, ()>;
    fn events(&self) -> LocalBoxStream<'static, PlatformEvent>;
    fn http(&self, request: HttpRequest) -> HostFuture<'_, HttpResponse>;
    fn websocket(&self, request: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>>;
    fn host(&self, action: HostAction) -> HostFuture<'_, HostReply>;
    /// The raw launch address; claimed native schemes and HTTP origins are validated by the host.
    fn app_url(&self) -> HostFuture<'_, Option<String>> {
        async { Err(PlatformError::Unsupported("app URL".into())) }.boxed_local()
    }
    /// Current first, then resolved history/claimed-link changes. Drop releases host listeners.
    fn app_url_changes(
        &self,
    ) -> LocalBoxStream<'static, Result<crate::navigation::AppUrlSnapshot, PlatformError>> {
        futures::stream::once(async { Err(PlatformError::Unsupported("app URL history".into())) })
            .boxed_local()
    }
    /// A GoThenPush reply settles only after traversal resolves and the push prunes forward history.
    fn navigate_app_url(&self, _request: crate::navigation::AppUrlRequest) -> HostFuture<'_, ()> {
        async { Err(PlatformError::Unsupported("app URL navigation".into())) }.boxed_local()
    }
    fn text_download_supported(&self) -> bool {
        false
    }
    fn download_text(
        &self,
        _filename: String,
        _contents: String,
        _media_type: String,
    ) -> HostFuture<'_, ()> {
        async { Err(PlatformError::Unsupported("text download".into())) }.boxed_local()
    }
    fn notification_permission(&self) -> HostFuture<'_, bool> {
        async move {
            match self.host(HostAction::NotificationPermission).await? {
                HostReply::Granted(value) => Ok(value),
                _ => Err(PlatformError::InvalidData(
                    "notification_permission reply".into(),
                )),
            }
        }
        .boxed_local()
    }
    fn notify(&self, notification: Notification) -> HostFuture<'_, ()> {
        async move {
            match self.host(HostAction::Notify(notification)).await? {
                HostReply::Unit => Ok(()),
                _ => Err(PlatformError::InvalidData("notify reply".into())),
            }
        }
        .boxed_local()
    }
    fn cancel_notification(&self, id: String) -> HostFuture<'_, ()> {
        async move {
            match self.host(HostAction::CancelNotification(id)).await? {
                HostReply::Unit => Ok(()),
                _ => Err(PlatformError::InvalidData(
                    "cancel_notification reply".into(),
                )),
            }
        }
        .boxed_local()
    }
    fn clipboard_read(&self) -> HostFuture<'_, Vec<MimeData>> {
        async move {
            match self.host(HostAction::ClipboardRead).await? {
                HostReply::Mime(value) => Ok(value),
                _ => Err(PlatformError::InvalidData("clipboard_read reply".into())),
            }
        }
        .boxed_local()
    }
    fn clipboard_write(&self, data: Vec<MimeData>) -> HostFuture<'_, ()> {
        async move {
            match self.host(HostAction::ClipboardWrite(data)).await? {
                HostReply::Unit => Ok(()),
                _ => Err(PlatformError::InvalidData("clipboard_write reply".into())),
            }
        }
        .boxed_local()
    }
    fn haptic(&self, style: Haptic) -> HostFuture<'_, ()> {
        async move {
            match self.host(HostAction::Haptic(style)).await? {
                HostReply::Unit => Ok(()),
                _ => Err(PlatformError::InvalidData("haptic reply".into())),
            }
        }
        .boxed_local()
    }
    /// What opening this host's storage had to discard, once per launch; empty where storage
    /// never discards (the browser).
    fn storage_report(&self) -> StorageReport {
        StorageReport::default()
    }
    /// Which links this host hands to the OS (`external_links::safe_external_url`).
    fn external_url_surface(&self) -> crate::persistence::external_links::ExternalUrlSurface {
        crate::persistence::external_links::ExternalUrlSurface::Web
    }
    /// Opens an editor link (TypeScript's `openRemoteEditorUrl`), through its own policy so that
    /// [`Self::open_url`] stays limited to what an ordinary link may open.
    fn open_editor_link(&self, url: String) -> HostFuture<'_, ()> {
        async move {
            let url = crate::persistence::external_links::safe_editor_link(
                &url,
                self.external_url_surface(),
            )
            .ok_or_else(|| PlatformError::InvalidData("Unable to open link.".into()))?;
            match self.host(HostAction::OpenEditorLink(url)).await? {
                HostReply::Unit => Ok(()),
                _ => Err(PlatformError::InvalidData("open_editor_link reply".into())),
            }
        }
        .boxed_local()
    }
    fn open_url(&self, url: String) -> HostFuture<'_, ()> {
        async move {
            if crate::persistence::external_links::safe_external_url(
                &url,
                self.external_url_surface(),
            )
            .is_none()
            {
                return Err(PlatformError::InvalidData("Unable to open link.".into()));
            }
            match self.host(HostAction::OpenUrl(url)).await? {
                HostReply::Unit => Ok(()),
                _ => Err(PlatformError::InvalidData("open_url reply".into())),
            }
        }
        .boxed_local()
    }
    fn pick_resources(
        &self,
        mime: Vec<String>,
        multiple: bool,
    ) -> HostFuture<'_, Vec<HostResource>> {
        async move {
            match self
                .host(HostAction::PickResources { mime, multiple })
                .await?
            {
                HostReply::Resources(value) => Ok(value),
                _ => Err(PlatformError::InvalidData("pick_resources reply".into())),
            }
        }
        .boxed_local()
    }
    fn read_resource(&self, handle: String, max_bytes: usize) -> HostFuture<'_, Vec<u8>> {
        async move {
            match self
                .host(HostAction::ReadResource { handle, max_bytes })
                .await?
            {
                HostReply::Bytes(value) => Ok(value),
                _ => Err(PlatformError::InvalidData("read_resource reply".into())),
            }
        }
        .boxed_local()
    }
    fn write_resource(&self, resource: HostResource, bytes: Vec<u8>) -> HostFuture<'_, ()> {
        async move {
            match self
                .host(HostAction::WriteResource { resource, bytes })
                .await?
            {
                HostReply::Unit => Ok(()),
                _ => Err(PlatformError::InvalidData("write_resource reply".into())),
            }
        }
        .boxed_local()
    }
    fn share(&self, title: String, resources: Vec<HostResource>) -> HostFuture<'_, ()> {
        async move {
            match self.host(HostAction::Share { title, resources }).await? {
                HostReply::Unit => Ok(()),
                _ => Err(PlatformError::InvalidData("share reply".into())),
            }
        }
        .boxed_local()
    }
    fn microphone_permission(&self) -> HostFuture<'_, bool> {
        async move {
            match self.host(HostAction::MicrophonePermission).await? {
                HostReply::Granted(value) => Ok(value),
                _ => Err(PlatformError::InvalidData(
                    "microphone_permission reply".into(),
                )),
            }
        }
        .boxed_local()
    }
    fn start_recording(&self, mime: String) -> HostFuture<'_, ()> {
        async move {
            match self.host(HostAction::StartRecording { mime }).await? {
                HostReply::Unit => Ok(()),
                _ => Err(PlatformError::InvalidData("start_recording reply".into())),
            }
        }
        .boxed_local()
    }
    fn remove_resource(&self, handle: String) -> HostFuture<'_, ()> {
        async move {
            match self.host(HostAction::RemoveResource { handle }).await? {
                HostReply::Unit => Ok(()),
                _ => Err(PlatformError::InvalidData("remove_resource reply".into())),
            }
        }
        .boxed_local()
    }
    fn stop_recording(&self) -> HostFuture<'_, Vec<MimeData>> {
        async move {
            match self.host(HostAction::StopRecording).await? {
                HostReply::Mime(value) => Ok(value),
                _ => Err(PlatformError::InvalidData("stop_recording reply".into())),
            }
        }
        .boxed_local()
    }
    fn play_audio(&self, audio: MimeData) -> HostFuture<'_, ()> {
        async move {
            match self.host(HostAction::PlayAudio(audio)).await? {
                HostReply::Unit => Ok(()),
                _ => Err(PlatformError::InvalidData("play_audio reply".into())),
            }
        }
        .boxed_local()
    }
    fn stop_audio(&self) -> HostFuture<'_, ()> {
        async move {
            match self.host(HostAction::StopAudio).await? {
                HostReply::Unit => Ok(()),
                _ => Err(PlatformError::InvalidData("stop_audio reply".into())),
            }
        }
        .boxed_local()
    }
    fn cloud_token(&self) -> HostFuture<'_, Option<String>> {
        async move {
            match self.host(HostAction::CloudToken).await? {
                HostReply::Text(value) => Ok(value),
                _ => Err(PlatformError::InvalidData("cloud_token reply".into())),
            }
        }
        .boxed_local()
    }
    fn cloud_identity(&self) -> HostFuture<'_, Option<CloudSessionIdentity>> {
        async move {
            match self.host(HostAction::CloudIdentity).await? {
                HostReply::Text(value) => {
                    Ok(value.map(|account_id| CloudSessionIdentity { account_id }))
                }
                HostReply::Document(value) => serde_json::from_value(value)
                    .map_err(|_| PlatformError::InvalidData("cloud_identity reply".into())),
                _ => Err(PlatformError::InvalidData("cloud_identity reply".into())),
            }
        }
        .boxed_local()
    }
    fn client_presentation(
        &self,
    ) -> HostFuture<'_, t3_contracts::auth::AuthClientPresentationMetadata> {
        async move {
            match self.host(HostAction::ClientPresentation).await? {
                HostReply::Document(value) => serde_json::from_value(value)
                    .map_err(|_| PlatformError::InvalidData("client_presentation reply".into())),
                _ => Err(PlatformError::InvalidData(
                    "client_presentation reply".into(),
                )),
            }
        }
        .boxed_local()
    }
    fn registrations(&self) -> HostFuture<'_, Vec<serde_json::Value>> {
        async move {
            match self.host(HostAction::Registrations).await? {
                HostReply::Document(serde_json::Value::Array(values)) => Ok(values),
                _ => Err(PlatformError::InvalidData("registrations reply".into())),
            }
        }
        .boxed_local()
    }
    fn ssh_provision(
        &self,
        target: t3_contracts::ipc::DesktopSshEnvironmentTargetSchema,
        expected: Option<EnvironmentId>,
    ) -> HostFuture<'_, ProvisionedSshEnvironment> {
        async move {
            match self
                .host(HostAction::SshProvision {
                    target,
                    expected: expected.clone(),
                })
                .await?
            {
                HostReply::Document(value) => {
                    let reply: ProvisionedSshEnvironment = serde_json::from_value(value)
                        .map_err(|_| PlatformError::InvalidData("ssh_provision reply".into()))?;
                    if expected
                        .as_ref()
                        .is_some_and(|id| id != &reply.environment_id)
                    {
                        return Err(PlatformError::InvalidData(
                            "SSH environment identity".into(),
                        ));
                    }
                    Ok(reply)
                }
                _ => Err(PlatformError::InvalidData("ssh_provision reply".into())),
            }
        }
        .boxed_local()
    }
    fn ssh_prepare(
        &self,
        connection_id: String,
        target: t3_contracts::ipc::DesktopSshEnvironmentTargetSchema,
        expected: EnvironmentId,
        bearer: Option<String>,
    ) -> HostFuture<'_, PreparedSshEnvironment> {
        async move {
            match self
                .host(HostAction::SshPrepare {
                    connection_id,
                    target,
                    expected,
                    bearer,
                })
                .await?
            {
                HostReply::Document(value) => serde_json::from_value(value)
                    .map_err(|_| PlatformError::InvalidData("ssh_prepare reply".into())),
                _ => Err(PlatformError::InvalidData("ssh_prepare reply".into())),
            }
        }
        .boxed_local()
    }
    fn ssh_disconnect(
        &self,
        target: t3_contracts::ipc::DesktopSshEnvironmentTargetSchema,
    ) -> HostFuture<'_, ()> {
        async move {
            match self.host(HostAction::SshDisconnect(target)).await? {
                HostReply::Unit => Ok(()),
                _ => Err(PlatformError::InvalidData("ssh_disconnect reply".into())),
            }
        }
        .boxed_local()
    }
    fn relay_device_id(&self) -> HostFuture<'_, Option<String>> {
        async move {
            match self.host(HostAction::RelayDeviceId).await? {
                HostReply::Text(value) => Ok(value),
                _ => Err(PlatformError::InvalidData("relay_device_id reply".into())),
            }
        }
        .boxed_local()
    }
    fn analytics_device_id(&self) -> HostFuture<'_, Option<String>> {
        async move {
            match self.host(HostAction::AnalyticsDeviceId).await? {
                HostReply::Text(value) => Ok(value),
                _ => Err(PlatformError::InvalidData(
                    "analytics_device_id reply".into(),
                )),
            }
        }
        .boxed_local()
    }
    fn primary_bearer(&self) -> HostFuture<'_, Option<String>> {
        async move {
            match self.host(HostAction::PrimaryBearer).await? {
                HostReply::Text(value) => Ok(value),
                _ => Err(PlatformError::InvalidData("primary_bearer reply".into())),
            }
        }
        .boxed_local()
    }
    fn dpop_thumbprint(&self) -> HostFuture<'_, Option<String>> {
        async move {
            match self.host(HostAction::DpopThumbprint).await? {
                HostReply::Text(value) => Ok(value),
                _ => Err(PlatformError::InvalidData("dpop_thumbprint reply".into())),
            }
        }
        .boxed_local()
    }
    fn dpop_public_jwk(&self) -> HostFuture<'_, serde_json::Value> {
        async move {
            match self.host(HostAction::DpopPublicJwk).await? {
                HostReply::Document(value)
                    if value["kty"] == "EC"
                        && value["crv"] == "P-256"
                        && value["x"].as_str().is_some_and(|x| !x.is_empty())
                        && value["y"].as_str().is_some_and(|y| !y.is_empty())
                        && value.get("d").is_none() =>
                {
                    Ok(serde_json::json!({
                        "kty": "EC", "crv": "P-256", "x": value["x"], "y": value["y"],
                    }))
                }
                _ => Err(PlatformError::InvalidData("dpop_public_jwk reply".into())),
            }
        }
        .boxed_local()
    }
    fn dpop_sign(&self, signing_input: Vec<u8>) -> HostFuture<'_, Vec<u8>> {
        async move {
            match self.host(HostAction::DpopSign { signing_input }).await? {
                HostReply::Bytes(value) => Ok(value),
                _ => Err(PlatformError::InvalidData("dpop_sign reply".into())),
            }
        }
        .boxed_local()
    }
    fn clear_environment(&self, environment: EnvironmentId) -> HostFuture<'_, ()> {
        async move {
            match self.host(HostAction::ClearEnvironment(environment)).await? {
                HostReply::Unit => Ok(()),
                _ => Err(PlatformError::InvalidData("clear_environment reply".into())),
            }
        }
        .boxed_local()
    }
}
