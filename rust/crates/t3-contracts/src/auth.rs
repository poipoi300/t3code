use crate::{
    base_schemas::{AuthSessionId, ClientSurface, ClientWebDeployment, TrimmedNonEmptyString},
    provider_policy::RuntimeMode,
    wire::{JsNumber, WireDateTime},
};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerAuthPolicy {
    #[serde(rename = "desktop-managed-local")]
    DesktopManagedLocal,
    #[serde(rename = "loopback-browser")]
    LoopbackBrowser,
    #[serde(rename = "remote-reachable")]
    RemoteReachable,
    #[serde(rename = "unsafe-no-auth")]
    UnsafeNoAuth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerAuthBootstrapMethod {
    #[serde(rename = "desktop-bootstrap")]
    DesktopBootstrap,
    #[serde(rename = "one-time-token")]
    OneTimeToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerAuthSessionMethod {
    #[serde(rename = "browser-session-cookie")]
    BrowserSessionCookie,
    #[serde(rename = "bearer-access-token")]
    BearerAccessToken,
    #[serde(rename = "dpop-access-token")]
    DpopAccessToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthEnvironmentScope {
    #[serde(rename = "orchestration:read")]
    OrchestrationRead,
    #[serde(rename = "orchestration:operate")]
    OrchestrationOperate,
    #[serde(rename = "settings:write")]
    SettingsWrite,
    #[serde(rename = "providers:manage")]
    ProvidersManage,
    #[serde(rename = "environment:maintain")]
    EnvironmentMaintain,
    #[serde(rename = "preview:operate")]
    PreviewOperate,
    #[serde(rename = "diagnostics:read")]
    DiagnosticsRead,
    #[serde(rename = "terminal:read")]
    TerminalRead,
    #[serde(rename = "terminal:operate")]
    TerminalOperate,
    #[serde(rename = "filesystem:read")]
    FilesystemRead,
    #[serde(rename = "filesystem:write")]
    FilesystemWrite,
    #[serde(rename = "review:write")]
    ReviewWrite,
    #[serde(rename = "source-control:write")]
    SourceControlWrite,
    #[serde(rename = "access:read")]
    AccessRead,
    #[serde(rename = "access:write")]
    AccessWrite,
    #[serde(rename = "relay:read")]
    RelayRead,
    #[serde(rename = "relay:write")]
    RelayWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthGrantScope {
    #[serde(rename = "orchestration:read")]
    OrchestrationRead,
    #[serde(rename = "orchestration:operate")]
    OrchestrationOperate,
    #[serde(rename = "settings:write")]
    SettingsWrite,
    #[serde(rename = "providers:manage")]
    ProvidersManage,
    #[serde(rename = "environment:maintain")]
    EnvironmentMaintain,
    #[serde(rename = "preview:operate")]
    PreviewOperate,
    #[serde(rename = "diagnostics:read")]
    DiagnosticsRead,
    #[serde(rename = "terminal:read")]
    TerminalRead,
    #[serde(rename = "terminal:operate")]
    TerminalOperate,
    #[serde(rename = "filesystem:read")]
    FilesystemRead,
    #[serde(rename = "filesystem:write")]
    FilesystemWrite,
    #[serde(rename = "source-control:write")]
    SourceControlWrite,
    #[serde(rename = "access:read")]
    AccessRead,
    #[serde(rename = "access:write")]
    AccessWrite,
    #[serde(rename = "relay:read")]
    RelayRead,
    #[serde(rename = "relay:write")]
    RelayWrite,
}

pub type AuthEnvironmentScopes = Vec<AuthEnvironmentScope>;

pub type AuthGrantScopes = Vec<AuthGrantScope>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerUpdateScope {
    #[serde(rename = "environment:maintain")]
    EnvironmentMaintain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerAuthDescriptor {
    #[serde(rename = "policy")]
    pub policy: ServerAuthPolicy,
    #[serde(rename = "bootstrapMethods")]
    pub bootstrap_methods: Vec<ServerAuthBootstrapMethod>,
    #[serde(rename = "sessionMethods")]
    pub session_methods: Vec<ServerAuthSessionMethod>,
    #[serde(rename = "sessionCookieName")]
    pub session_cookie_name: TrimmedNonEmptyString,
    #[serde(
        rename = "serverUpdateScope",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub server_update_scope: Option<ServerUpdateScope>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthBrowserSessionRequest {
    #[serde(rename = "credential")]
    pub credential: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Authenticated;
impl Serialize for Authenticated {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        true.serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for Authenticated {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if bool::deserialize(deserializer)? {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom("invalid literal"))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthBrowserSessionResult {
    #[serde(rename = "authenticated")]
    pub authenticated: Authenticated,
    #[serde(rename = "scopes")]
    pub scopes: AuthEnvironmentScopes,
    #[serde(
        rename = "permissions",
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_scopes"
    )]
    pub permissions: Option<AuthEnvironmentScopes>,
    #[serde(rename = "sessionMethod")]
    pub session_method: ServerAuthSessionMethod,
    #[serde(rename = "expiresAt")]
    pub expires_at: WireDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthClientMetadataDeviceType {
    #[serde(rename = "desktop")]
    Desktop,
    #[serde(rename = "mobile")]
    Mobile,
    #[serde(rename = "tablet")]
    Tablet,
    #[serde(rename = "bot")]
    Bot,
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthClientPresentationMetadata {
    #[serde(
        rename = "label",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub label: Option<TrimmedNonEmptyString>,
    #[serde(
        rename = "deviceType",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub device_type: Option<AuthClientMetadataDeviceType>,
    #[serde(
        rename = "os",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub os: Option<TrimmedNonEmptyString>,
    #[serde(
        rename = "osMajorVersion",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub os_major_version: Option<i64>,
    #[serde(
        rename = "deviceModel",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub device_model: Option<TrimmedNonEmptyString>,
    #[serde(
        rename = "surface",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub surface: Option<ClientSurface>,
    #[serde(
        rename = "webDeployment",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub web_deployment: Option<ClientWebDeployment>,
    #[serde(
        rename = "browser",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub browser: Option<TrimmedNonEmptyString>,
    #[serde(
        rename = "appVersion",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub app_version: Option<TrimmedNonEmptyString>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthTokenExchangeGrantType {
    #[serde(rename = "urn:ietf:params:oauth:grant-type:token-exchange")]
    UrnIetfParamsOauthGrantTypeTokenExchange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthAccessTokenType {
    #[serde(rename = "urn:ietf:params:oauth:token-type:access_token")]
    UrnIetfParamsOauthTokenTypeAccessToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthEnvironmentBootstrapTokenType {
    #[serde(rename = "urn:t3:params:oauth:token-type:environment-bootstrap")]
    UrnT3ParamsOauthTokenTypeEnvironmentBootstrap,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthTokenExchangeRequest {
    #[serde(rename = "grant_type")]
    pub grant_type: AuthTokenExchangeGrantType,
    #[serde(rename = "subject_token")]
    pub subject_token: TrimmedNonEmptyString,
    #[serde(rename = "subject_token_type")]
    pub subject_token_type: AuthEnvironmentBootstrapTokenType,
    #[serde(rename = "requested_token_type")]
    pub requested_token_type: AuthAccessTokenType,
    #[serde(
        rename = "scope",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub scope: Option<TrimmedNonEmptyString>,
    #[serde(
        rename = "client_label",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub client_label: Option<TrimmedNonEmptyString>,
    #[serde(
        rename = "client_device_type",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub client_device_type: Option<AuthClientMetadataDeviceType>,
    #[serde(
        rename = "client_os",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub client_os: Option<TrimmedNonEmptyString>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthTokenType {
    #[serde(rename = "Bearer")]
    Bearer,
    #[serde(rename = "DPoP")]
    DPoP,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthAccessTokenResult {
    #[serde(rename = "access_token")]
    pub access_token: TrimmedNonEmptyString,
    #[serde(rename = "issued_token_type")]
    pub issued_token_type: AuthAccessTokenType,
    #[serde(rename = "token_type")]
    pub token_type: AuthTokenType,
    #[serde(rename = "expires_in")]
    pub expires_in: EncodedNumber,
    #[serde(rename = "scope")]
    pub scope: TrimmedNonEmptyString,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthWebSocketTicketResult {
    #[serde(rename = "ticket")]
    pub ticket: TrimmedNonEmptyString,
    #[serde(rename = "expiresAt")]
    pub expires_at: WireDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthWebSocketTicketUse {
    #[serde(rename = "rpc")]
    Rpc,
    #[serde(rename = "media")]
    Media,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthWebSocketTicketParams {
    #[serde(rename = "use")]
    pub r#use: AuthWebSocketTicketUse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthMcpClientAccess {
    #[serde(rename = "read-only")]
    ReadOnly,
    #[serde(rename = "approval-required")]
    ApprovalRequired,
    #[serde(rename = "auto-accept-edits")]
    AutoAcceptEdits,
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "full-access")]
    FullAccess,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthPairingCredentialResult {
    #[serde(rename = "id")]
    pub id: TrimmedNonEmptyString,
    #[serde(rename = "credential")]
    pub credential: TrimmedNonEmptyString,
    #[serde(
        rename = "label",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub label: Option<TrimmedNonEmptyString>,
    #[serde(rename = "expiresAt")]
    pub expires_at: WireDateTime,
    #[serde(
        rename = "access",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub access: Option<AuthMcpClientAccess>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthPairingLink {
    #[serde(rename = "id")]
    pub id: TrimmedNonEmptyString,
    #[serde(rename = "scopes")]
    pub scopes: AuthEnvironmentScopes,
    #[serde(
        rename = "permissions",
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_scopes"
    )]
    pub permissions: Option<AuthEnvironmentScopes>,
    #[serde(rename = "subject")]
    pub subject: TrimmedNonEmptyString,
    #[serde(
        rename = "label",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub label: Option<TrimmedNonEmptyString>,
    #[serde(rename = "createdAt")]
    pub created_at: WireDateTime,
    #[serde(rename = "expiresAt")]
    pub expires_at: WireDateTime,
    #[serde(
        rename = "connectDeviceRequest",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub connect_device_request: Option<AuthConnectDeviceRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthClientMetadata {
    #[serde(
        rename = "label",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub label: Option<TrimmedNonEmptyString>,
    #[serde(
        rename = "ipAddress",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub ip_address: Option<TrimmedNonEmptyString>,
    #[serde(
        rename = "userAgent",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub user_agent: Option<TrimmedNonEmptyString>,
    #[serde(rename = "deviceType")]
    pub device_type: AuthClientMetadataDeviceType,
    #[serde(
        rename = "os",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub os: Option<TrimmedNonEmptyString>,
    #[serde(
        rename = "browser",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub browser: Option<TrimmedNonEmptyString>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthClientSession {
    #[serde(rename = "sessionId")]
    pub session_id: AuthSessionId,
    #[serde(rename = "subject")]
    pub subject: TrimmedNonEmptyString,
    #[serde(rename = "scopes")]
    pub scopes: AuthEnvironmentScopes,
    #[serde(
        rename = "permissions",
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_scopes"
    )]
    pub permissions: Option<AuthEnvironmentScopes>,
    #[serde(rename = "method")]
    pub method: ServerAuthSessionMethod,
    #[serde(rename = "client")]
    pub client: AuthClientMetadata,
    #[serde(rename = "issuedAt")]
    pub issued_at: WireDateTime,
    #[serde(rename = "expiresAt")]
    pub expires_at: WireDateTime,
    #[serde(rename = "lastConnectedAt", with = "crate::wire::nullable")]
    pub last_connected_at: Option<WireDateTime>,
    #[serde(rename = "connected")]
    pub connected: bool,
    #[serde(rename = "current")]
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthAccessSnapshot {
    #[serde(rename = "pairingLinks")]
    pub pairing_links: Vec<AuthPairingLink>,
    #[serde(rename = "clientSessions")]
    pub client_sessions: Vec<AuthClientSession>,
    #[serde(
        rename = "connectDevices",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub connect_devices: Option<Vec<AuthConnectDevice>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthStreamVersion;
impl Serialize for AuthStreamVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        1.serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for AuthStreamVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if u64::deserialize(deserializer)? == 1 {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom("invalid literal"))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthPairingLinkRemoved {
    #[serde(rename = "id")]
    pub id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthClientRemoved {
    #[serde(rename = "sessionId")]
    pub session_id: AuthSessionId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthAccessStreamSnapshotEventType {
    #[serde(rename = "snapshot")]
    Snapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthAccessStreamSnapshotEvent {
    #[serde(rename = "version")]
    pub version: AuthStreamVersion,
    #[serde(rename = "revision")]
    pub revision: EncodedNumber,
    #[serde(rename = "type")]
    pub r#type: AuthAccessStreamSnapshotEventType,
    #[serde(rename = "payload")]
    pub payload: AuthAccessSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthAccessStreamPairingLinkUpsertedEventType {
    #[serde(rename = "pairingLinkUpserted")]
    PairingLinkUpserted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthAccessStreamPairingLinkUpsertedEvent {
    #[serde(rename = "version")]
    pub version: AuthStreamVersion,
    #[serde(rename = "revision")]
    pub revision: EncodedNumber,
    #[serde(rename = "type")]
    pub r#type: AuthAccessStreamPairingLinkUpsertedEventType,
    #[serde(rename = "payload")]
    pub payload: AuthPairingLink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthAccessStreamPairingLinkRemovedEventType {
    #[serde(rename = "pairingLinkRemoved")]
    PairingLinkRemoved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthAccessStreamPairingLinkRemovedEvent {
    #[serde(rename = "version")]
    pub version: AuthStreamVersion,
    #[serde(rename = "revision")]
    pub revision: EncodedNumber,
    #[serde(rename = "type")]
    pub r#type: AuthAccessStreamPairingLinkRemovedEventType,
    #[serde(rename = "payload")]
    pub payload: AuthPairingLinkRemoved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthAccessStreamClientUpsertedEventType {
    #[serde(rename = "clientUpserted")]
    ClientUpserted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthAccessStreamClientUpsertedEvent {
    #[serde(rename = "version")]
    pub version: AuthStreamVersion,
    #[serde(rename = "revision")]
    pub revision: EncodedNumber,
    #[serde(rename = "type")]
    pub r#type: AuthAccessStreamClientUpsertedEventType,
    #[serde(rename = "payload")]
    pub payload: AuthClientSession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthAccessStreamClientRemovedEventType {
    #[serde(rename = "clientRemoved")]
    ClientRemoved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthAccessStreamClientRemovedEvent {
    #[serde(rename = "version")]
    pub version: AuthStreamVersion,
    #[serde(rename = "revision")]
    pub revision: EncodedNumber,
    #[serde(rename = "type")]
    pub r#type: AuthAccessStreamClientRemovedEventType,
    #[serde(rename = "payload")]
    pub payload: AuthClientRemoved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AuthAccessStreamEvent {
    AuthAccessStreamSnapshotEvent(AuthAccessStreamSnapshotEvent),
    AuthAccessStreamPairingLinkUpsertedEvent(AuthAccessStreamPairingLinkUpsertedEvent),
    AuthAccessStreamPairingLinkRemovedEvent(AuthAccessStreamPairingLinkRemovedEvent),
    AuthAccessStreamClientUpsertedEvent(AuthAccessStreamClientUpsertedEvent),
    AuthAccessStreamClientRemovedEvent(AuthAccessStreamClientRemovedEvent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthAccessStreamErrorTag {
    #[serde(rename = "AuthAccessStreamError")]
    AuthAccessStreamError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthAccessStreamError {
    #[serde(rename = "_tag")]
    pub _tag: AuthAccessStreamErrorTag,
    #[serde(rename = "message")]
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentAuthorizationErrorTag {
    #[serde(rename = "EnvironmentAuthorizationError")]
    EnvironmentAuthorizationError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentAuthorizationError {
    #[serde(rename = "_tag")]
    pub _tag: EnvironmentAuthorizationErrorTag,
    #[serde(rename = "message")]
    pub message: String,
    #[serde(rename = "requiredScope")]
    pub required_scope: AuthEnvironmentScope,
    #[serde(
        rename = "requiredPermission",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub required_permission: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthRevokePairingLinkInput {
    #[serde(rename = "id")]
    pub id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthRevokeClientSessionInput {
    #[serde(rename = "sessionId")]
    pub session_id: AuthSessionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthCreatePairingCredentialInput {
    #[serde(
        rename = "label",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub label: Option<TrimmedNonEmptyString>,
    #[serde(
        rename = "scopes",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub scopes: Option<AuthGrantScopes>,
    #[serde(
        rename = "access",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub access: Option<AuthMcpClientAccess>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthSessionState {
    #[serde(rename = "authenticated")]
    pub authenticated: bool,
    #[serde(rename = "auth")]
    pub auth: ServerAuthDescriptor,
    #[serde(
        rename = "scopes",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub scopes: Option<AuthEnvironmentScopes>,
    #[serde(
        rename = "permissions",
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_scopes"
    )]
    pub permissions: Option<AuthEnvironmentScopes>,
    #[serde(
        rename = "sessionMethod",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub session_method: Option<ServerAuthSessionMethod>,
    #[serde(
        rename = "expiresAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub expires_at: Option<WireDateTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthHeaderMethod {
    #[serde(rename = "header")]
    Header,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthCodeResponse {
    #[serde(rename = "code")]
    Code,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthAuthorizationCodeGrant {
    #[serde(rename = "authorization_code")]
    AuthorizationCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthS256Challenge {
    #[serde(rename = "S256")]
    S256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthNoneMethod {
    #[serde(rename = "none")]
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthBearerToken {
    #[serde(rename = "Bearer")]
    Bearer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMcpProtectedResourceMetadata {
    #[serde(rename = "resource")]
    pub resource: String,
    #[serde(rename = "authorization_servers")]
    pub authorization_servers: Vec<String>,
    #[serde(rename = "scopes_supported")]
    pub scopes_supported: Vec<AuthEnvironmentScope>,
    #[serde(rename = "bearer_methods_supported")]
    pub bearer_methods_supported: Vec<AuthHeaderMethod>,
    #[serde(rename = "resource_name")]
    pub resource_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMcpAuthorizationServerMetadata {
    #[serde(rename = "issuer")]
    pub issuer: String,
    #[serde(rename = "authorization_endpoint")]
    pub authorization_endpoint: String,
    #[serde(rename = "token_endpoint")]
    pub token_endpoint: String,
    #[serde(rename = "registration_endpoint")]
    pub registration_endpoint: String,
    #[serde(rename = "response_types_supported")]
    pub response_types_supported: Vec<AuthCodeResponse>,
    #[serde(rename = "grant_types_supported")]
    pub grant_types_supported: Vec<AuthAuthorizationCodeGrant>,
    #[serde(rename = "code_challenge_methods_supported")]
    pub code_challenge_methods_supported: Vec<AuthS256Challenge>,
    #[serde(rename = "token_endpoint_auth_methods_supported")]
    pub token_endpoint_auth_methods_supported: Vec<AuthNoneMethod>,
    #[serde(rename = "scopes_supported")]
    pub scopes_supported: Vec<AuthEnvironmentScope>,
    #[serde(rename = "authorization_response_iss_parameter_supported")]
    pub authorization_response_iss_parameter_supported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMcpClientRegistration {
    #[serde(
        rename = "client_name",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub client_name: Option<String>,
    #[serde(
        rename = "software_id",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub software_id: Option<String>,
    #[serde(
        rename = "redirect_uris",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub redirect_uris: Option<Vec<String>>,
    #[serde(
        rename = "token_endpoint_auth_method",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub token_endpoint_auth_method: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMcpRegisteredClient {
    #[serde(rename = "client_id")]
    pub client_id: String,
    #[serde(rename = "client_name")]
    pub client_name: String,
    #[serde(rename = "redirect_uris")]
    pub redirect_uris: Vec<String>,
    #[serde(rename = "grant_types")]
    pub grant_types: Vec<AuthAuthorizationCodeGrant>,
    #[serde(rename = "response_types")]
    pub response_types: Vec<AuthCodeResponse>,
    #[serde(rename = "token_endpoint_auth_method")]
    pub token_endpoint_auth_method: AuthNoneMethod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthMcpRegistrationErrorCode {
    #[serde(rename = "invalid_client_metadata")]
    InvalidClientMetadata,
    #[serde(rename = "invalid_redirect_uri")]
    InvalidRedirectUri,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMcpRegistrationError {
    #[serde(rename = "error")]
    pub error: AuthMcpRegistrationErrorCode,
    #[serde(rename = "error_description")]
    pub error_description: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMcpAuthorizationRequest {
    #[serde(
        rename = "response_type",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub response_type: Option<String>,
    #[serde(
        rename = "client_id",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub client_id: Option<String>,
    #[serde(
        rename = "redirect_uri",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub redirect_uri: Option<String>,
    #[serde(
        rename = "code_challenge",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub code_challenge: Option<String>,
    #[serde(
        rename = "code_challenge_method",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub code_challenge_method: Option<String>,
    #[serde(
        rename = "state",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub state: Option<String>,
    #[serde(
        rename = "resource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub resource: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthRuntimeModeCeiling {
    #[serde(rename = "organization")]
    pub organization: String,
    #[serde(rename = "maxRuntimeMode")]
    pub max_runtime_mode: RuntimeMode,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMcpApprovalDetails {
    #[serde(rename = "clientName")]
    pub client_name: String,
    #[serde(rename = "redirectHost")]
    pub redirect_host: String,
    #[serde(rename = "environmentHost")]
    pub environment_host: String,
    #[serde(
        rename = "csrfToken",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub csrf_token: Option<String>,
    #[serde(
        rename = "oneClickAccess",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub one_click_access: Option<Vec<AuthMcpClientAccess>>,
    #[serde(
        rename = "runtimeModeCeiling",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub runtime_mode_ceiling: Option<AuthRuntimeModeCeiling>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMcpApprovalRedirect {
    #[serde(rename = "redirectTo")]
    pub redirect_to: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthMcpApprovalDenyTag {
    #[serde(rename = "deny")]
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMcpApprovalDeny {
    #[serde(rename = "_tag")]
    pub _tag: AuthMcpApprovalDenyTag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthMcpApprovalPairingCodeTag {
    #[serde(rename = "pairing-code")]
    PairingCode,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMcpApprovalPairingCode {
    #[serde(rename = "_tag")]
    pub _tag: AuthMcpApprovalPairingCodeTag,
    #[serde(rename = "access")]
    pub access: AuthMcpClientAccess,
    #[serde(rename = "code")]
    pub code: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthMcpApprovalBrowserSessionTag {
    #[serde(rename = "browser-session")]
    BrowserSession,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMcpApprovalBrowserSession {
    #[serde(rename = "_tag")]
    pub _tag: AuthMcpApprovalBrowserSessionTag,
    #[serde(rename = "access")]
    pub access: AuthMcpClientAccess,
    #[serde(rename = "csrfToken")]
    pub csrf_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AuthMcpApprovalDecision {
    AuthMcpApprovalDeny(AuthMcpApprovalDeny),
    AuthMcpApprovalPairingCode(AuthMcpApprovalPairingCode),
    AuthMcpApprovalBrowserSession(AuthMcpApprovalBrowserSession),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMcpApprovalDecisionRequest {
    #[serde(rename = "authorization")]
    pub authorization: AuthMcpAuthorizationRequest,
    #[serde(rename = "decision")]
    pub decision: AuthMcpApprovalDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthMcpApprovalErrorTag {
    #[serde(rename = "AuthMcpApprovalError")]
    AuthMcpApprovalError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMcpApprovalError {
    #[serde(rename = "_tag")]
    pub _tag: AuthMcpApprovalErrorTag,
    #[serde(rename = "message")]
    pub message: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMcpTokenRequest {
    #[serde(
        rename = "grant_type",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub grant_type: Option<String>,
    #[serde(
        rename = "code",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub code: Option<String>,
    #[serde(
        rename = "redirect_uri",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub redirect_uri: Option<String>,
    #[serde(
        rename = "client_id",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub client_id: Option<String>,
    #[serde(
        rename = "code_verifier",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub code_verifier: Option<String>,
    #[serde(
        rename = "resource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub resource: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMcpTokenResult {
    #[serde(rename = "access_token")]
    pub access_token: String,
    #[serde(rename = "token_type")]
    pub token_type: AuthBearerToken,
    #[serde(rename = "expires_in")]
    pub expires_in: EncodedNumber,
    #[serde(rename = "scope")]
    pub scope: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthMcpTokenErrorCode {
    #[serde(rename = "invalid_request")]
    InvalidRequest,
    #[serde(rename = "invalid_client")]
    InvalidClient,
    #[serde(rename = "invalid_grant")]
    InvalidGrant,
    #[serde(rename = "unsupported_grant_type")]
    UnsupportedGrantType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMcpTokenError {
    #[serde(rename = "error")]
    pub error: AuthMcpTokenErrorCode,
    #[serde(rename = "error_description")]
    pub error_description: String,
}

mod optional_scopes {
    use super::*;
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<AuthEnvironmentScopes>, D::Error> {
        crate::wire::deserialize_lenient_array(deserializer).map(Some)
    }
    pub use crate::wire::optional::serialize;
}

// Golden samples encode non-finite Schema.Number values as strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NonFiniteNumber {
    #[serde(rename = "NaN")]
    NaN,
    #[serde(rename = "Infinity")]
    Infinity,
    #[serde(rename = "-Infinity")]
    NegativeInfinity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EncodedNumber {
    Finite(JsNumber),
    NonFinite(NonFiniteNumber),
}

crate::redacted_debug!(AuthBrowserSessionRequest {  } redact { credential });
crate::redacted_debug!(AuthTokenExchangeRequest { grant_type, subject_token_type, requested_token_type, scope, client_label, client_device_type, client_os } redact { subject_token });
crate::redacted_debug!(AuthAccessTokenResult { issued_token_type, token_type, expires_in, scope } redact { access_token });
crate::redacted_debug!(AuthWebSocketTicketResult { expires_at } redact { ticket });
crate::redacted_debug!(AuthPairingCredentialResult { id, label, expires_at, access } redact { credential });
crate::redacted_debug!(AuthMcpRegisteredClient { client_name, redirect_uris, grant_types, response_types, token_endpoint_auth_method } redact { client_id });
crate::redacted_debug!(AuthMcpAuthorizationRequest { response_type, redirect_uri, code_challenge, code_challenge_method, resource } redact { state, client_id });
crate::redacted_debug!(AuthMcpApprovalDetails { client_name, redirect_host, environment_host, one_click_access, runtime_mode_ceiling } redact { csrf_token });
crate::redacted_debug!(AuthMcpApprovalRedirect {  } redact { redirect_to });
crate::redacted_debug!(AuthMcpApprovalPairingCode { _tag, access } redact { code });
crate::redacted_debug!(AuthMcpApprovalBrowserSession { _tag, access } redact { csrf_token });
crate::redacted_debug!(AuthMcpTokenRequest { grant_type, redirect_uri, resource } redact { code, code_verifier, client_id });
crate::redacted_debug!(AuthMcpTokenResult { token_type, expires_in, scope } redact { access_token });

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthConnectDeviceRequest {
    pub verification_code: TrimmedNonEmptyString,
    #[serde(
        rename = "client",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub client: Option<AuthClientMetadata>,
}
crate::redacted_debug!(AuthConnectDeviceRequest { client } redact { verification_code });

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthConnectDevice {
    pub proof_key_thumbprint: TrimmedNonEmptyString,
    pub verification_code: TrimmedNonEmptyString,
    #[serde(
        rename = "client",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub client: Option<AuthClientMetadata>,
    #[serde(deserialize_with = "crate::wire::utc_datetime::deserialize")]
    pub approved_at: WireDateTime,
}
crate::redacted_debug!(AuthConnectDevice { proof_key_thumbprint, client, approved_at } redact { verification_code });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthApproveConnectDeviceInput {
    pub pairing_link_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthApproveConnectDeviceResult {
    pub approved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthRevokeConnectDeviceInput {
    pub proof_key_thumbprint: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthRevokeConnectDeviceResult {
    pub revoked: bool,
}
