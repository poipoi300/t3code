//! Remote pairing, bearer and DPoP authorization. All transport and key I/O is host supplied.
use crate::{
    environment::SessionIdentity,
    platform::{
        Credentials, HttpRequest, HttpResponse, Platform, PlatformEvent, StorageArea,
        StorageCondition, StorageKey, StorageTransaction, StorageWrite,
    },
    types::EnvironmentId,
};
use futures::{
    FutureExt, StreamExt,
    future::{Either, LocalBoxFuture, select},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fmt,
    rc::Rc,
};
use t3_contracts::base_schemas::TrimmedNonEmptyString;
use t3_contracts::{
    auth::*, environment::ExecutionEnvironmentDescriptor, relay::RelayManagedEndpoint,
};
use url::Url;

pub const DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS: u64 = 60_000;
pub const DEFAULT_REMOTE_REQUEST_TIMEOUT_MS: u64 = 10_000;
pub(crate) const MAX_AUTH_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Sensitive<T>(pub T);
impl<T> fmt::Debug for Sensitive<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl<T> Sensitive<T> {
    pub fn expose(&self) -> &T {
        &self.0
    }
    pub fn into_inner(self) -> T {
        self.0
    }
}
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AuthorizationError {
    #[error("invalid authorization endpoint")]
    InvalidUrl,
    #[error("unexpected environment identity")]
    EnvironmentMismatch,
    #[error("waiting for cloud sign-in")]
    SessionPending,
    #[error("authorization session changed")]
    SessionChanged,
    #[error("authorization storage failed")]
    Storage,
    #[error("authorization network request failed")]
    Network,
    #[error("invalid authorization response")]
    InvalidResponse,
    #[error("authorization endpoint returned HTTP {0}")]
    HttpStatus(u16),
    #[error("environment rejected the request (HTTP {status})")]
    Remote {
        status: u16,
        details: Sensitive<Box<t3_contracts::environment_http::EnvironmentHttpCommonError>>,
    },
    #[error("environment rejected the credential")]
    CredentialRejected,
    #[error("environment requires a credential")]
    CredentialMissing,
    #[error("authorization request timed out")]
    Timeout,
    #[error("could not create authorization proof")]
    Proof,
    #[error("unexpected authorization token type")]
    TokenType,
    #[error("relay authorization failed")]
    Relay,
    #[error("authorization operation cancelled")]
    Cancelled,
}

#[derive(Clone)]
pub struct AccessToken {
    pub session: SessionIdentity,
    pub token: String,
    pub expires_at_epoch_ms: u64,
}
impl fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccessToken([REDACTED])")
    }
}
impl AccessToken {
    pub fn usable(&self, identity: &SessionIdentity, now: u64, rejected: Option<&str>) -> bool {
        !self.token.is_empty()
            && self.session == *identity
            && self.expires_at_epoch_ms.saturating_sub(now) > DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS
            && rejected != Some(self.token.as_str())
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpAuthorization {
    Cookie,
    Bearer(Sensitive<String>),
    Dpop {
        access_token: Sensitive<String>,
        expires_at_epoch_ms: u64,
    },
}
impl HttpAuthorization {
    pub fn access_token(&self) -> Option<&str> {
        match self {
            Self::Cookie => None,
            Self::Bearer(token)
            | Self::Dpop {
                access_token: token,
                ..
            } => Some(token.expose()),
        }
    }
}
#[derive(Clone)]
pub struct AuthorizedEnvironment {
    pub session: SessionIdentity,
    pub http_base_url: String,
    pub socket_url: Sensitive<String>,
    pub authorization: HttpAuthorization,
    pub descriptor: Option<ExecutionEnvironmentDescriptor>,
}
impl fmt::Debug for AuthorizedEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthorizedEnvironment([REDACTED])")
    }
}

pub(crate) fn http_url(base: &str, path: &str) -> Result<String, AuthorizationError> {
    let mut url = parse_endpoint(base, &["https", "http"])?;
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.into())
}
pub(crate) fn parse_endpoint(value: &str, schemes: &[&str]) -> Result<Url, AuthorizationError> {
    let url = Url::parse(value).map_err(|_| AuthorizationError::InvalidUrl)?;
    if !schemes.contains(&url.scheme())
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(AuthorizationError::InvalidUrl);
    }
    Ok(url)
}
pub(crate) fn scalar_string<T: Serialize>(value: &T) -> Result<String, AuthorizationError> {
    serde_json::to_value(value)
        .map_err(|_| AuthorizationError::InvalidResponse)?
        .as_str()
        .map(str::to_owned)
        .ok_or(AuthorizationError::InvalidResponse)
}
pub(crate) fn form<T: Serialize>(value: &T) -> Result<Vec<u8>, AuthorizationError> {
    let value = serde_json::to_value(value).map_err(|_| AuthorizationError::InvalidResponse)?;
    let values = value
        .as_object()
        .ok_or(AuthorizationError::InvalidResponse)?;
    let mut encoder = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in values {
        if let Some(value) = value.as_str() {
            encoder.append_pair(key, value);
        } else {
            return Err(AuthorizationError::InvalidResponse);
        }
    }
    Ok(encoder.finish().into_bytes())
}
pub(crate) async fn send_http(
    platform: &dyn Platform,
    request: HttpRequest,
    timeout_ms: u64,
) -> Result<HttpResponse, AuthorizationError> {
    let limit = request.max_response_bytes;
    let response = match select(platform.http(request), platform.sleep(timeout_ms)).await {
        Either::Left((result, _)) => result.map_err(|_| AuthorizationError::Network)?,
        Either::Right(_) => return Err(AuthorizationError::Timeout),
    };
    if response.body.len() > limit {
        return Err(AuthorizationError::InvalidResponse);
    }
    Ok(response)
}
/// Whether the environment refused the request's credential as invalid (not missing).
pub(crate) fn rejects_credential(response: &HttpResponse) -> bool {
    !(200..300).contains(&response.status)
        && serde_json::from_slice::<serde_json::Value>(&response.body).is_ok_and(|body| {
            body.get("_tag").and_then(|value| value.as_str()) == Some("EnvironmentAuthInvalidError")
                && body.get("reason").and_then(|value| value.as_str()) == Some("invalid_credential")
        })
}
pub(crate) fn response_json<T: DeserializeOwned>(
    response: HttpResponse,
) -> Result<T, AuthorizationError> {
    if !(200..300).contains(&response.status) {
        let body: serde_json::Value = serde_json::from_slice(&response.body).unwrap_or_default();
        if body.get("_tag").and_then(|value| value.as_str()) == Some("EnvironmentAuthInvalidError")
        {
            match body.get("reason").and_then(|value| value.as_str()) {
                Some("invalid_credential") => return Err(AuthorizationError::CredentialRejected),
                Some("missing_credential") => return Err(AuthorizationError::CredentialMissing),
                _ => {}
            }
        }
        if let Ok(error) = serde_json::from_slice::<
            t3_contracts::environment_http::EnvironmentHttpCommonError,
        >(&response.body)
        {
            return Err(AuthorizationError::Remote {
                status: response.status,
                details: Sensitive(Box::new(error)),
            });
        }
        return Err(AuthorizationError::HttpStatus(response.status));
    }
    serde_json::from_slice(&response.body).map_err(|_| AuthorizationError::InvalidResponse)
}
pub(crate) fn request(
    method: &str,
    url: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
    credentials: Credentials,
) -> HttpRequest {
    HttpRequest {
        method: method.into(),
        url,
        headers,
        body,
        credentials,
        max_response_bytes: MAX_AUTH_RESPONSE_BYTES,
    }
}
pub async fn fetch_descriptor(
    platform: &dyn Platform,
    base: &str,
    expected: &EnvironmentId,
    timeout_ms: u64,
) -> Result<ExecutionEnvironmentDescriptor, AuthorizationError> {
    let descriptor: ExecutionEnvironmentDescriptor = response_json(
        send_http(
            platform,
            request(
                "GET",
                http_url(base, "/.well-known/t3/environment")?,
                BTreeMap::new(),
                vec![],
                Credentials::Omit,
            ),
            timeout_ms,
        )
        .await?,
    )?;
    if descriptor.environment_id != *expected {
        return Err(AuthorizationError::EnvironmentMismatch);
    }
    Ok(descriptor)
}

pub async fn bootstrap_remote_session(
    platform: &dyn Platform,
    base: &str,
    credential: &str,
    scopes: Option<&[AuthEnvironmentScope]>,
    metadata: Option<&AuthClientPresentationMetadata>,
    proof: Option<&str>,
) -> Result<Sensitive<AuthAccessTokenResult>, AuthorizationError> {
    let payload = AuthTokenExchangeRequest {
        grant_type: AuthTokenExchangeGrantType::UrnIetfParamsOauthGrantTypeTokenExchange,
        subject_token: TrimmedNonEmptyString(credential.to_owned()),
        subject_token_type:
            AuthEnvironmentBootstrapTokenType::UrnT3ParamsOauthTokenTypeEnvironmentBootstrap,
        requested_token_type: AuthAccessTokenType::UrnIetfParamsOauthTokenTypeAccessToken,
        scope: scopes
            .map(|scopes| {
                scopes
                    .iter()
                    .map(scalar_string)
                    .collect::<Result<Vec<_>, _>>()
                    .map(|values| TrimmedNonEmptyString(values.join(" ")))
            })
            .transpose()?,
        client_label: metadata.and_then(|value| value.label.clone()),
        client_device_type: metadata.and_then(|value| value.device_type),
        client_os: metadata
            .and_then(|value| value.os.clone())
            .filter(|value| !matches!(value.0.as_str(), "unknown" | "other")),
    };
    let mut headers = BTreeMap::from([(
        "content-type".into(),
        "application/x-www-form-urlencoded".into(),
    )]);
    if let Some(proof) = proof {
        headers.insert("dpop".into(), proof.into());
    }
    let result: AuthAccessTokenResult = response_json(
        send_http(
            platform,
            request(
                "POST",
                http_url(base, "/oauth/token")?,
                headers,
                form(&payload)?,
                Credentials::Omit,
            ),
            DEFAULT_REMOTE_REQUEST_TIMEOUT_MS,
        )
        .await?,
    )?;
    if (proof.is_some() && result.token_type != AuthTokenType::DPoP)
        || (proof.is_none() && result.token_type != AuthTokenType::Bearer)
    {
        return Err(AuthorizationError::TokenType);
    }
    Ok(Sensitive(result))
}
pub async fn issue_websocket_ticket(
    platform: &dyn Platform,
    base: &str,
    authorization: &HttpAuthorization,
    signer: Option<&dyn crate::relay::DpopSigner>,
    audience: Option<AuthWebSocketTicketUse>,
    timeout_ms: u64,
) -> Result<Sensitive<AuthWebSocketTicketResult>, AuthorizationError> {
    let audience = audience.map(|value| scalar_string(&value)).transpose()?;
    let mut path = audience.as_ref().map_or_else(
        || "/api/auth/websocket-ticket".into(),
        |audience| format!("/api/auth/websocket-ticket/{audience}"),
    );
    loop {
        let url = http_url(base, &path)?;
        let headers = auth_headers(authorization, "POST", &url, signer)
            .await?
            .into_inner();
        let credentials = if matches!(authorization, HttpAuthorization::Cookie) {
            Credentials::Include
        } else {
            Credentials::Omit
        };
        let response = send_http(
            platform,
            request("POST", url, headers, vec![], credentials),
            timeout_ms,
        )
        .await?;
        if response.status == 404 && path != "/api/auth/websocket-ticket" {
            path = "/api/auth/websocket-ticket".into();
            continue;
        }
        return response_json(response).map(Sensitive);
    }
}
pub async fn auth_headers(
    authorization: &HttpAuthorization,
    method: &str,
    url: &str,
    signer: Option<&dyn crate::relay::DpopSigner>,
) -> Result<Sensitive<BTreeMap<String, String>>, AuthorizationError> {
    let headers = match authorization {
        HttpAuthorization::Cookie => BTreeMap::new(),
        HttpAuthorization::Bearer(token) => {
            BTreeMap::from([("authorization".into(), format!("Bearer {}", token.expose()))])
        }
        HttpAuthorization::Dpop { access_token, .. } => {
            let proof = signer
                .ok_or(AuthorizationError::Proof)?
                .create_proof(method, url, Some(access_token.expose()))
                .await
                .map_err(|_| AuthorizationError::Proof)?;
            BTreeMap::from([
                (
                    "authorization".into(),
                    format!("DPoP {}", access_token.expose()),
                ),
                ("dpop".into(), proof.into_inner()),
            ])
        }
    };
    Ok(Sensitive(headers))
}
pub fn append_client_connection_params(
    url: &mut Url,
    metadata: Option<&AuthClientPresentationMetadata>,
    connection_method: Option<&str>,
    owner_hash: Option<&str>,
    device_hash: Option<&str>,
) -> Result<(), AuthorizationError> {
    let mut params = url.query_pairs_mut();
    if let Some(metadata) = metadata {
        if let Some(value) = metadata.surface {
            params.append_pair("clientSurface", &scalar_string(&value)?);
        }
        if let Some(value) = &metadata.app_version {
            params.append_pair("clientAppVersion", value.0.as_str());
        }
        if let Some(value) = metadata.device_type {
            params.append_pair(
                "clientDeviceType",
                match value {
                    AuthClientMetadataDeviceType::Mobile => "phone",
                    AuthClientMetadataDeviceType::Desktop => "desktop",
                    AuthClientMetadataDeviceType::Tablet => "tablet",
                    _ => "unknown",
                },
            );
        }
        if let Some(value) = &metadata.os {
            params.append_pair("clientOs", value.0.as_str());
        }
        if metadata.surface == Some(t3_contracts::base_schemas::ClientSurface::Web) {
            if let Some(value) = metadata.web_deployment {
                params.append_pair("clientWebDeployment", &scalar_string(&value)?);
            }
            if let Some(value) = &metadata.browser {
                params.append_pair("clientBrowser", value.0.as_str());
            }
        }
        if metadata.surface == Some(t3_contracts::base_schemas::ClientSurface::Mobile) {
            if let Some(value) = metadata.os_major_version {
                params.append_pair("clientOsMajorVersion", &value.to_string());
            }
            if let Some(value) = &metadata.device_model {
                params.append_pair("clientDeviceModel", value.0.as_str());
            }
        }
    }
    if let Some(value) = connection_method {
        params.append_pair("connectionMethod", value);
    }
    if let Some(value) = owner_hash {
        params.append_pair("clientOwner", value);
    }
    if let Some(value) = device_hash {
        params.append_pair("clientDevice", value);
    }
    Ok(())
}
pub async fn read_client_owner_params(platform: &dyn Platform) -> (Option<String>, Option<String>) {
    use sha2::{Digest, Sha256};
    let hash = |value: &str| {
        Sha256::digest(value.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let account = match platform
        .host(crate::platform::HostAction::CloudIdentity)
        .await
    {
        Ok(crate::platform::HostReply::Document(value)) => value
            .get("accountId")
            .and_then(serde_json::Value::as_str)
            .map(hash),
        _ => None,
    };
    let device = platform
        .analytics_device_id()
        .await
        .ok()
        .flatten()
        .map(|value| hash(&value));
    (account, device)
}
#[expect(
    clippy::too_many_arguments,
    reason = "authorization, endpoint and presentation inputs are independent protocol fields"
)]
pub async fn websocket_connection_url(
    platform: &dyn Platform,
    http_base: &str,
    ws_base: &str,
    authorization: &HttpAuthorization,
    signer: Option<&dyn crate::relay::DpopSigner>,
    metadata: Option<&AuthClientPresentationMetadata>,
    connection_method: Option<&str>,
    timeout_ms: u64,
) -> Result<Sensitive<String>, AuthorizationError> {
    let mut url = parse_endpoint(ws_base, &["ws", "wss"])?;
    if url.path().is_empty() || url.path() == "/" {
        url.set_path("/ws");
    }
    let ticket = issue_websocket_ticket(
        platform,
        http_base,
        authorization,
        signer,
        Some(AuthWebSocketTicketUse::Rpc),
        timeout_ms,
    )
    .await?;
    let proof = if matches!(authorization, HttpAuthorization::Dpop { .. }) {
        let mut target = url.clone();
        target
            .set_scheme(if url.scheme() == "wss" {
                "https"
            } else {
                "http"
            })
            .map_err(|_| AuthorizationError::InvalidUrl)?;
        target.set_query(None);
        target.set_fragment(None);
        Some(
            signer
                .ok_or(AuthorizationError::Proof)?
                .create_proof(
                    "GET",
                    target.as_str(),
                    Some(ticket.expose().ticket.0.as_str()),
                )
                .await
                .map_err(|_| AuthorizationError::Proof)?,
        )
    } else {
        None
    };
    let preserved: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| key != "wsTicket" && key != "dpop")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.set_query(None);
    url.set_fragment(None);
    url.query_pairs_mut()
        .extend_pairs(preserved)
        .append_pair("wsTicket", ticket.expose().ticket.0.as_str());
    if let Some(proof) = proof {
        url.query_pairs_mut().append_pair("dpop", proof.expose());
    }
    let (owner, device) = read_client_owner_params(platform).await;
    append_client_connection_params(
        &mut url,
        metadata,
        connection_method,
        owner.as_deref(),
        device.as_deref(),
    )?;
    Ok(Sensitive(url.into()))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDpopAccessToken {
    pub environment_id: EnvironmentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    pub label: String,
    pub endpoint: RelayManagedEndpoint,
    pub access_token: Sensitive<String>,
    pub expires_at_epoch_ms: u64,
    pub dpop_thumbprint: String,
}
impl fmt::Debug for RemoteDpopAccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RemoteDpopAccessToken([REDACTED])")
    }
}
#[derive(Clone)]
pub struct RemoteTokenStore {
    platform: Rc<dyn Platform>,
}
impl fmt::Debug for RemoteTokenStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RemoteTokenStore")
    }
}
impl RemoteTokenStore {
    pub fn new(platform: Rc<dyn Platform>) -> Self {
        Self { platform }
    }
    fn key(environment: &EnvironmentId) -> StorageKey {
        crate::persistence::remote_dpop_token_key(environment)
    }
    pub async fn get(
        &self,
        environment: &EnvironmentId,
    ) -> Result<Option<RemoteDpopAccessToken>, AuthorizationError> {
        let bytes = self
            .platform
            .get(StorageArea::Secure, Self::key(environment))
            .await
            .map_err(|_| AuthorizationError::Storage)?;
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        let record: RemoteDpopAccessToken = crate::persistence::decode_record::<
            RemoteDpopAccessToken,
        >(&bytes, &Self::key(environment), 1)
        .map_err(|_| AuthorizationError::InvalidResponse)?
        .payload;
        if record.environment_id != *environment {
            return Err(AuthorizationError::EnvironmentMismatch);
        }
        Ok(Some(record))
    }
    pub async fn put(&self, record: &RemoteDpopAccessToken) -> Result<(), AuthorizationError> {
        let key = Self::key(&record.environment_id);
        let bytes = crate::persistence::Record::new(
            &key.namespace,
            key.environment.clone(),
            &key.record,
            1,
            record,
        )
        .encode()
        .map_err(|_| AuthorizationError::InvalidResponse)?;
        self.platform
            .write(
                StorageArea::Secure,
                vec![StorageWrite {
                    key: Self::key(&record.environment_id),
                    value: Some(bytes),
                }],
            )
            .await
            .map_err(|_| AuthorizationError::Storage)
    }
    pub async fn remove_if_matches(
        &self,
        environment: &EnvironmentId,
        token: &str,
    ) -> Result<bool, AuthorizationError> {
        let key = Self::key(environment);
        let Some(bytes) = self
            .platform
            .get(StorageArea::Secure, key.clone())
            .await
            .map_err(|_| AuthorizationError::Storage)?
        else {
            return Ok(false);
        };
        let current = crate::persistence::decode_record::<RemoteDpopAccessToken>(&bytes, &key, 1)
            .map_err(|_| AuthorizationError::InvalidResponse)?
            .payload;
        if current.environment_id != *environment || current.access_token.expose() != token {
            return Ok(false);
        }
        self.platform
            .transaction(StorageTransaction {
                conditions: vec![StorageCondition {
                    area: StorageArea::Secure,
                    key: key.clone(),
                    expected: Some(bytes),
                }],
                ordinary: vec![],
                secure: vec![StorageWrite { key, value: None }],
            })
            .await
            .map_err(|_| AuthorizationError::Storage)
    }
    pub async fn remove(&self, environment: &EnvironmentId) -> Result<(), AuthorizationError> {
        self.platform
            .write(
                StorageArea::Secure,
                vec![StorageWrite {
                    key: Self::key(environment),
                    value: None,
                }],
            )
            .await
            .map_err(|_| AuthorizationError::Storage)
    }
}

fn map_relay_error(error: crate::relay::RelayError) -> AuthorizationError {
    use crate::relay::RelayError;
    match error {
        RelayError::Network => AuthorizationError::Network,
        RelayError::Timeout => AuthorizationError::Timeout,
        RelayError::InvalidUrl => AuthorizationError::InvalidUrl,
        RelayError::Key | RelayError::Proof => AuthorizationError::Proof,
        RelayError::Storage => AuthorizationError::Storage,
        RelayError::SignedOut | RelayError::SessionChanged => AuthorizationError::SessionChanged,
        RelayError::EnvironmentMismatch => AuthorizationError::EnvironmentMismatch,
        RelayError::UnexpectedScopes | RelayError::InvalidResponse => {
            AuthorizationError::InvalidResponse
        }
        RelayError::Request {
            status: 401 | 403, ..
        } => AuthorizationError::CredentialRejected,
        RelayError::Request {
            status: 500..=599, ..
        } => AuthorizationError::Relay,
        RelayError::Request { status, .. } => AuthorizationError::HttpStatus(status),
    }
}

#[derive(Clone)]
struct IssuedToken {
    token: RemoteDpopAccessToken,
    descriptor: Option<ExecutionEnvironmentDescriptor>,
    from_cache: bool,
}

struct PendingToken {
    identity: SessionIdentity,
    id: u64,
    future: futures::future::WeakShared<
        LocalBoxFuture<'static, Result<IssuedToken, AuthorizationError>>,
    >,
}
#[derive(Default)]
struct AuthorizationState {
    sessions: BTreeMap<EnvironmentId, SessionIdentity>,
    owners: BTreeMap<EnvironmentId, (String, SessionIdentity)>,
    pending: BTreeMap<EnvironmentId, PendingToken>,
    descriptors: BTreeMap<String, (u64, ExecutionEnvironmentDescriptor)>,
    next_pending: u64,
    next_session_revision: u64,
    session_revisions: BTreeMap<EnvironmentId, u64>,
    retired: BTreeSet<EnvironmentId>,
    forgotten: BTreeSet<EnvironmentId>,
    cleanup_environments: BTreeSet<EnvironmentId>,
    cleanup_all: bool,
    epoch: u64,
}
#[derive(Clone)]
pub struct RemoteAuthorization {
    platform: Rc<dyn Platform>,
    signer: Option<Rc<dyn crate::relay::DpopSigner>>,
    relay: Option<crate::relay::ManagedRelayClient>,
    store: RemoteTokenStore,
    state: Rc<RefCell<AuthorizationState>>,
    storage_lock: Rc<futures::lock::Mutex<()>>,
    credential_events: Rc<RefCell<futures::stream::LocalBoxStream<'static, PlatformEvent>>>,
}
impl fmt::Debug for RemoteAuthorization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RemoteAuthorization([REDACTED])")
    }
}
impl RemoteAuthorization {
    pub fn new(
        platform: Rc<dyn Platform>,
        signer: Rc<dyn crate::relay::DpopSigner>,
        relay: crate::relay::ManagedRelayClient,
    ) -> Self {
        Self {
            credential_events: Rc::new(RefCell::new(platform.events())),
            store: RemoteTokenStore::new(platform.clone()),
            platform,
            signer: Some(signer),
            relay: Some(relay),
            state: Rc::default(),
            storage_lock: Rc::new(futures::lock::Mutex::new(())),
        }
    }
    pub fn direct(platform: Rc<dyn Platform>) -> Self {
        Self {
            credential_events: Rc::new(RefCell::new(platform.events())),
            store: RemoteTokenStore::new(platform.clone()),
            platform,
            signer: None,
            relay: None,
            state: Rc::default(),
            storage_lock: Rc::new(futures::lock::Mutex::new(())),
        }
    }
    pub(crate) fn dpop_signer(&self) -> Option<Rc<dyn crate::relay::DpopSigner>> {
        self.signer.clone()
    }
    pub fn set_session(&self, identity: SessionIdentity) {
        let mut state = self.state.borrow_mut();
        if state.sessions.get(&identity.environment) != Some(&identity) {
            state.next_session_revision += 1;
            let revision = state.next_session_revision;
            state
                .session_revisions
                .insert(identity.environment.clone(), revision);
        }
        state.retired.remove(&identity.environment);
        state
            .sessions
            .insert(identity.environment.clone(), identity);
    }
    pub fn clear_session(&self, environment: &EnvironmentId) {
        let mut state = self.state.borrow_mut();
        state.sessions.remove(environment);
        state.pending.remove(environment);
        state.owners.remove(environment);
        state.retired.insert(environment.clone());
        state.cleanup_environments.insert(environment.clone());
        state.epoch = state.epoch.wrapping_add(1);
    }
    pub fn invalidate_sessions(&self) {
        let mut state = self.state.borrow_mut();
        let environments: Vec<_> = state.sessions.keys().cloned().collect();
        state.retired.extend(environments);
        state.sessions.clear();
        state.pending.clear();
        state.owners.clear();
        state.descriptors.clear();
        state.cleanup_all = true;
        state.epoch = state.epoch.wrapping_add(1);
        if let Some(relay) = &self.relay {
            relay.invalidate_token_cache();
        }
    }
    fn drain_credential_events(&self) {
        loop {
            let event = self
                .credential_events
                .borrow_mut()
                .next()
                .now_or_never()
                .flatten();
            match event {
                Some(PlatformEvent::CredentialsChanged) if self.relay.is_some() => {
                    self.invalidate_sessions()
                }
                Some(_) => {}
                None => break,
            }
        }
    }
    pub fn assert_session(&self, identity: &SessionIdentity) -> Result<(), AuthorizationError> {
        self.drain_credential_events();
        if self.state.borrow().sessions.get(&identity.environment) != Some(identity) {
            return Err(AuthorizationError::SessionChanged);
        }
        Ok(())
    }
    pub async fn flush_cleanup(&self) -> Result<(), AuthorizationError> {
        let _lock = self.storage_lock.lock().await;
        let (all, environments, epoch) = {
            let state = self.state.borrow();
            (
                state.cleanup_all,
                state.cleanup_environments.clone(),
                state.epoch,
            )
        };
        if !all && environments.is_empty() {
            return Ok(());
        }
        let keys = if all {
            self.platform
                .list(StorageArea::Secure, "authorization.remote-dpop".into())
                .await
                .map_err(|_| AuthorizationError::Storage)?
        } else {
            environments.iter().map(RemoteTokenStore::key).collect()
        };
        if !keys.is_empty() {
            self.platform
                .write(
                    StorageArea::Secure,
                    keys.into_iter()
                        .map(|key| StorageWrite { key, value: None })
                        .collect(),
                )
                .await
                .map_err(|_| AuthorizationError::Storage)?;
        }
        if all && let Some(relay) = &self.relay {
            relay.reset_token_cache().await.map_err(map_relay_error)?;
        }
        let mut state = self.state.borrow_mut();
        if state.epoch == epoch {
            state.cleanup_all = false;
            state.cleanup_environments.clear();
        }
        Ok(())
    }
    async fn validate_cloud_identity(
        &self,
        identity: &SessionIdentity,
    ) -> Result<(), AuthorizationError> {
        let Some(account) = &identity.account else {
            return Ok(());
        };
        let current = self
            .platform
            .cloud_identity()
            .await
            .map_err(|_| AuthorizationError::SessionPending)?;
        if current.as_ref().map(|identity| &identity.account_id) != Some(account) {
            self.invalidate_sessions();
            self.flush_cleanup().await?;
            return Err(AuthorizationError::SessionChanged);
        }
        Ok(())
    }
    pub async fn validate_session(
        &self,
        identity: &SessionIdentity,
    ) -> Result<(), AuthorizationError> {
        if let Err(error) = self.assert_session(identity) {
            self.flush_cleanup().await?;
            return Err(error);
        }
        self.validate_cloud_identity(identity).await?;
        let result = self.assert_session(identity);
        if result.is_err() {
            self.flush_cleanup().await?;
        }
        result
    }
    pub(crate) fn session_revision(&self, identity: &SessionIdentity) -> Option<u64> {
        self.state
            .borrow()
            .session_revisions
            .get(&identity.environment)
            .copied()
    }
    pub(crate) async fn validate_session_at(
        &self,
        identity: &SessionIdentity,
        revision: Option<u64>,
    ) -> Result<(), AuthorizationError> {
        self.validate_session(identity).await?;
        if revision != self.session_revision(identity) {
            return Err(AuthorizationError::SessionChanged);
        }
        Ok(())
    }
    pub async fn begin_session(
        &self,
        identity: &SessionIdentity,
    ) -> Result<(), AuthorizationError> {
        self.drain_credential_events();
        self.flush_cleanup().await?;
        {
            let state = self.state.borrow();
            if state.forgotten.contains(&identity.environment)
                || state
                    .sessions
                    .get(&identity.environment)
                    .is_some_and(|current| current.generation.0 > identity.generation.0)
            {
                return Err(AuthorizationError::SessionChanged);
            }
        }
        let epoch = self.state.borrow().epoch;
        self.validate_cloud_identity(identity).await?;
        if self.state.borrow().epoch != epoch {
            return Err(AuthorizationError::SessionChanged);
        }
        self.set_session(identity.clone());
        self.validate_session(identity).await
    }
    pub async fn ensure_session(
        &self,
        identity: &SessionIdentity,
    ) -> Result<(), AuthorizationError> {
        self.drain_credential_events();
        let uninitialized = {
            let state = self.state.borrow();
            !state.sessions.contains_key(&identity.environment)
                && !state.retired.contains(&identity.environment)
                && !state.forgotten.contains(&identity.environment)
        };
        if uninitialized {
            self.begin_session(identity).await
        } else {
            self.validate_session(identity).await
        }
    }
    pub async fn capture_session(
        &self,
        environment: EnvironmentId,
        generation: crate::types::Generation,
    ) -> Result<SessionIdentity, AuthorizationError> {
        let account = match self.platform.cloud_identity().await {
            Ok(identity) => identity.map(|identity| identity.account_id),
            Err(crate::platform::PlatformError::Unsupported(_)) => None,
            Err(_) => return Err(AuthorizationError::SessionPending),
        };
        let dpop_thumbprint = if account.is_some() {
            Some(
                self.signer
                    .as_ref()
                    .ok_or(AuthorizationError::Proof)?
                    .thumbprint()
                    .await
                    .map_err(|_| AuthorizationError::Proof)?,
            )
        } else {
            None
        };
        let identity = SessionIdentity {
            environment,
            generation,
            account,
            dpop_thumbprint,
        };
        self.begin_session(&identity).await?;
        Ok(identity)
    }
    pub fn forget_environment(&self, environment: &EnvironmentId) {
        self.clear_session(environment);
        self.state
            .borrow_mut()
            .forgotten
            .insert(environment.clone());
    }
    pub fn allow_environment(&self, environment: &EnvironmentId) {
        self.state.borrow_mut().forgotten.remove(environment);
    }
    pub fn remember_checked_descriptor(
        &self,
        base: &str,
        descriptor: Option<ExecutionEnvironmentDescriptor>,
        keeps_its_host: bool,
    ) -> Result<(), AuthorizationError> {
        let url = parse_endpoint(base, &["http", "https"])?;
        let key = http_url(base, "/")?;
        let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        let mut state = self.state.borrow_mut();
        state.descriptors.remove(&key);
        if keeps_its_host
            && (url.scheme() == "https" || loopback)
            && let Some(descriptor) = descriptor
        {
            state
                .descriptors
                .insert(key, (self.platform.monotonic_ms(), descriptor));
        }
        Ok(())
    }
    async fn verify_endpoint(
        &self,
        identity: &SessionIdentity,
        base: &str,
    ) -> Result<ExecutionEnvironmentDescriptor, AuthorizationError> {
        self.verify_endpoint_for(identity, base, "relay").await
    }
    async fn verify_endpoint_for(
        &self,
        identity: &SessionIdentity,
        base: &str,
        connection_method: &str,
    ) -> Result<ExecutionEnvironmentDescriptor, AuthorizationError> {
        let revision = self.session_revision(identity);
        self.validate_session_at(identity, revision).await?;
        let cached = self
            .state
            .borrow_mut()
            .descriptors
            .remove(&http_url(base, "/")?);
        if let Some((at, descriptor)) = cached
            && self.platform.monotonic_ms().saturating_sub(at) <= 5_000
            && descriptor.environment_id == identity.environment
        {
            return Ok(descriptor);
        }
        let descriptor = fetch_descriptor(
            self.platform.as_ref(),
            base,
            &identity.environment,
            if connection_method == "direct" && crate::connection::is_nearby_address(base) {
                3_000
            } else {
                DEFAULT_REMOTE_REQUEST_TIMEOUT_MS
            },
        )
        .await?;
        self.validate_session_at(identity, revision).await?;
        Ok(descriptor)
    }
    pub async fn authorize_bearer(
        &self,
        identity: &SessionIdentity,
        http_base: &str,
        ws_base: &str,
        bearer: Sensitive<String>,
        connection_method: &str,
    ) -> Result<AuthorizedEnvironment, AuthorizationError> {
        let revision = self.session_revision(identity);
        let descriptor = self
            .verify_endpoint_for(identity, http_base, connection_method)
            .await?;
        let authorization = HttpAuthorization::Bearer(bearer);
        let socket_url = websocket_connection_url(
            self.platform.as_ref(),
            http_base,
            ws_base,
            &authorization,
            None,
            None,
            Some(connection_method),
            DEFAULT_REMOTE_REQUEST_TIMEOUT_MS,
        )
        .await?;
        self.validate_session_at(identity, revision).await?;
        Ok(AuthorizedEnvironment {
            session: identity.clone(),
            http_base_url: http_base.into(),
            socket_url,
            authorization,
            descriptor: Some(descriptor),
        })
    }
    pub async fn pair_bearer(
        &self,
        identity: &SessionIdentity,
        http_base: &str,
        ws_base: &str,
        credential: Sensitive<String>,
        scopes: Option<&[AuthEnvironmentScope]>,
        metadata: Option<&AuthClientPresentationMetadata>,
    ) -> Result<AuthorizedEnvironment, AuthorizationError> {
        let revision = self.session_revision(identity);
        let descriptor = self
            .verify_endpoint_for(identity, http_base, "direct")
            .await?;
        self.validate_session_at(identity, revision).await?;
        let token = bootstrap_remote_session(
            self.platform.as_ref(),
            http_base,
            credential.expose(),
            scopes,
            metadata,
            None,
        )
        .await?;
        self.validate_session_at(identity, revision).await?;
        let authorization = HttpAuthorization::Bearer(Sensitive(token.into_inner().access_token.0));
        let socket_url = websocket_connection_url(
            self.platform.as_ref(),
            http_base,
            ws_base,
            &authorization,
            None,
            metadata,
            Some("direct"),
            DEFAULT_REMOTE_REQUEST_TIMEOUT_MS,
        )
        .await?;
        self.validate_session_at(identity, revision).await?;
        Ok(AuthorizedEnvironment {
            session: identity.clone(),
            http_base_url: http_base.into(),
            socket_url,
            authorization,
            descriptor: Some(descriptor),
        })
    }
    async fn reject_token(
        &self,
        environment: &EnvironmentId,
        token: &str,
    ) -> Result<(), AuthorizationError> {
        let _lock = self.storage_lock.lock().await;
        if self.store.remove_if_matches(environment, token).await? {
            self.state.borrow_mut().owners.remove(environment);
        }
        Ok(())
    }
    async fn mint_token(
        &self,
        identity: &SessionIdentity,
    ) -> Result<IssuedToken, AuthorizationError> {
        let revision = self.session_revision(identity);
        self.validate_session_at(identity, revision).await?;
        let clerk = self
            .platform
            .cloud_token()
            .await
            .map_err(|_| AuthorizationError::SessionPending)?
            .filter(|value| !value.is_empty())
            .ok_or(AuthorizationError::SessionPending)?;
        self.validate_session_at(identity, revision).await?;
        if crate::relay::relay_account_id(&clerk).as_ref() != identity.account.as_ref() {
            return Err(AuthorizationError::SessionChanged);
        }
        let device = self
            .platform
            .relay_device_id()
            .await
            .map_err(|_| AuthorizationError::Relay)?;
        self.validate_session_at(identity, revision).await?;
        let bootstrap = self
            .relay
            .as_ref()
            .ok_or(AuthorizationError::Relay)?
            .connect_environment(&clerk, &identity.environment, device.as_deref())
            .await
            .map_err(map_relay_error)?
            .into_inner();
        self.validate_session_at(identity, revision).await?;
        if bootstrap.environment_id != identity.environment {
            return Err(AuthorizationError::EnvironmentMismatch);
        }
        let descriptor = self
            .verify_endpoint(identity, bootstrap.endpoint.http_base_url.0.as_str())
            .await?;
        let proof = self
            .signer
            .as_ref()
            .ok_or(AuthorizationError::Proof)?
            .create_proof(
                "POST",
                &http_url(bootstrap.endpoint.http_base_url.0.as_str(), "/oauth/token")?,
                None,
            )
            .await
            .map_err(|_| AuthorizationError::Proof)?;
        self.validate_session_at(identity, revision).await?;
        let access = bootstrap_remote_session(
            self.platform.as_ref(),
            bootstrap.endpoint.http_base_url.0.as_str(),
            bootstrap.credential.0.as_str(),
            None,
            None,
            Some(proof.expose()),
        )
        .await?
        .into_inner();
        self.validate_session_at(identity, revision).await?;
        let seconds = match access.expires_in {
            EncodedNumber::Finite(value) => value
                .as_f64()
                .filter(|value| value.is_finite() && *value > 0.0)
                .ok_or(AuthorizationError::InvalidResponse)?,
            _ => return Err(AuthorizationError::InvalidResponse),
        };
        let token = RemoteDpopAccessToken {
            environment_id: identity.environment.clone(),
            account_id: identity.account.clone(),
            label: descriptor.label.0.clone(),
            endpoint: bootstrap.endpoint,
            access_token: Sensitive(access.access_token.0),
            expires_at_epoch_ms: self
                .platform
                .now_epoch_ms()
                .saturating_add((seconds * 1000.0) as u64),
            dpop_thumbprint: identity
                .dpop_thumbprint
                .clone()
                .ok_or(AuthorizationError::Proof)?,
        };
        {
            let _lock = self.storage_lock.lock().await;
            self.assert_session(identity)?;
            if self.session_revision(identity) != revision {
                return Err(AuthorizationError::SessionChanged);
            }
            self.store.put(&token).await?;
        }
        if let Err(error) = self.validate_session_at(identity, revision).await {
            self.store
                .remove_if_matches(&identity.environment, token.access_token.expose())
                .await?;
            return Err(error);
        }
        self.state.borrow_mut().owners.insert(
            identity.environment.clone(),
            (token.access_token.expose().clone(), identity.clone()),
        );
        Ok(IssuedToken {
            token,
            descriptor: Some(descriptor),
            from_cache: false,
        })
    }
    async fn get_token(
        &self,
        identity: &SessionIdentity,
        rejected: Option<&str>,
    ) -> Result<IssuedToken, AuthorizationError> {
        let revision = self.session_revision(identity);
        self.validate_session_at(identity, revision).await?;
        if identity.account.as_deref().is_none_or(str::is_empty) {
            return Err(AuthorizationError::SessionPending);
        }
        let thumbprint = self
            .signer
            .as_ref()
            .ok_or(AuthorizationError::Proof)?
            .thumbprint()
            .await
            .map_err(|_| AuthorizationError::Proof)?;
        self.validate_session_at(identity, revision).await?;
        if identity.dpop_thumbprint.as_deref() != Some(thumbprint.as_str()) {
            return Err(AuthorizationError::SessionChanged);
        }
        let cached = self.store.get(&identity.environment).await?;
        self.validate_session_at(identity, revision).await?;
        if let Some(token) = cached {
            let owned = self
                .state
                .borrow()
                .owners
                .get(&identity.environment)
                .is_none_or(|(owned, owner)| {
                    owned != token.access_token.expose() || owner == identity
                });
            if token.environment_id == identity.environment
                && token.account_id == identity.account
                && token.dpop_thumbprint == thumbprint
                && token
                    .expires_at_epoch_ms
                    .saturating_sub(self.platform.now_epoch_ms())
                    > DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS
                && rejected != Some(token.access_token.expose())
                && owned
                && !token.access_token.expose().is_empty()
            {
                self.state.borrow_mut().owners.insert(
                    identity.environment.clone(),
                    (token.access_token.expose().clone(), identity.clone()),
                );
                return Ok(IssuedToken {
                    token,
                    descriptor: None,
                    from_cache: true,
                });
            }
        }
        if let Some(rejected) = rejected {
            self.reject_token(&identity.environment, rejected).await?;
            self.validate_session_at(identity, revision).await?;
        }
        let future = {
            let mut state = self.state.borrow_mut();
            if let Some(future) = state
                .pending
                .get(&identity.environment)
                .filter(|pending| pending.identity == *identity)
                .and_then(|pending| pending.future.upgrade())
            {
                future
            } else {
                state.next_pending = state.next_pending.wrapping_add(1);
                let id = state.next_pending;
                let service = self.clone();
                let identity = identity.clone();
                let key = identity.environment.clone();
                let pending_identity = identity.clone();
                let future = async move {
                    let result = match select(
                        service.mint_token(&identity).boxed_local(),
                        service.platform.sleep(30_000),
                    )
                    .await
                    {
                        Either::Left((result, _)) => result,
                        Either::Right(_) => Err(AuthorizationError::Timeout),
                    };
                    let mut state = service.state.borrow_mut();
                    if state
                        .pending
                        .get(&identity.environment)
                        .is_some_and(|pending| pending.id == id)
                    {
                        state.pending.remove(&identity.environment);
                    }
                    result
                }
                .boxed_local()
                .shared();
                state.pending.insert(
                    key,
                    PendingToken {
                        identity: pending_identity,
                        id,
                        future: future.downgrade().ok_or(AuthorizationError::Cancelled)?,
                    },
                );
                future
            }
        };
        let issued = future.await?;
        self.validate_session_at(identity, revision).await?;
        Ok(issued)
    }
    pub async fn authorize_dpop_http(
        &self,
        identity: &SessionIdentity,
        rejected: Option<&str>,
    ) -> Result<(String, HttpAuthorization), AuthorizationError> {
        let revision = self.session_revision(identity);
        let issued = self.get_token(identity, rejected).await?;
        self.validate_session_at(identity, revision).await?;
        Ok((
            http_url(&issued.token.endpoint.http_base_url.0, "/")?,
            HttpAuthorization::Dpop {
                access_token: issued.token.access_token,
                expires_at_epoch_ms: issued.token.expires_at_epoch_ms,
            },
        ))
    }
    pub async fn authorize_dpop(
        &self,
        identity: &SessionIdentity,
        direct: Option<(&str, &str)>,
    ) -> Result<AuthorizedEnvironment, AuthorizationError> {
        let revision = self.session_revision(identity);
        let checked = if let Some((http, _)) = direct {
            Some(self.verify_endpoint_for(identity, http, "direct").await?)
        } else {
            None
        };
        let mut issued = self.get_token(identity, None).await?;
        for attempt in 0..2 {
            let http = direct.map_or(
                issued.token.endpoint.http_base_url.0.as_str(),
                |(http, _)| http,
            );
            let ws = direct.map_or(issued.token.endpoint.ws_base_url.0.as_str(), |(_, ws)| ws);
            let authorization = HttpAuthorization::Dpop {
                access_token: issued.token.access_token.clone(),
                expires_at_epoch_ms: issued.token.expires_at_epoch_ms,
            };
            let timeout = if issued.from_cache {
                3_000
            } else {
                DEFAULT_REMOTE_REQUEST_TIMEOUT_MS
            };
            let result = websocket_connection_url(
                self.platform.as_ref(),
                http,
                ws,
                &authorization,
                self.signer.as_deref(),
                None,
                Some("relay"),
                timeout,
            )
            .await;
            self.validate_session_at(identity, revision).await?;
            match result {
                Ok(socket_url) => {
                    let descriptor = match checked.clone().or(issued.descriptor) {
                        Some(value) => value,
                        None => self.verify_endpoint(identity, http).await?,
                    };
                    self.validate_session_at(identity, revision).await?;
                    return Ok(AuthorizedEnvironment {
                        session: identity.clone(),
                        http_base_url: http.into(),
                        socket_url,
                        authorization,
                        descriptor: Some(descriptor),
                    });
                }
                Err(error)
                    if attempt == 0
                        && issued.from_cache
                        && (direct.is_none()
                            || error == AuthorizationError::CredentialRejected) =>
                {
                    issued = self
                        .get_token(identity, Some(issued.token.access_token.expose()))
                        .await?;
                }
                Err(error) => {
                    if direct.is_none() || error == AuthorizationError::CredentialRejected {
                        self.reject_token(
                            &identity.environment,
                            issued.token.access_token.expose(),
                        )
                        .await?;
                    }
                    return Err(error);
                }
            }
        }
        Err(AuthorizationError::CredentialRejected)
    }
    pub async fn renew_dpop_before_expiry(
        &self,
        identity: &SessionIdentity,
        mut expires_at_epoch_ms: u64,
    ) -> Result<(), AuthorizationError> {
        let revision = self.session_revision(identity);
        loop {
            self.validate_session_at(identity, revision).await?;
            let now = self.platform.now_epoch_ms();
            if now >= expires_at_epoch_ms {
                return Ok(());
            }
            self.platform
                .sleep(
                    expires_at_epoch_ms
                        .saturating_sub(DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS)
                        .saturating_sub(now)
                        .max(5_000),
                )
                .await;
            self.validate_session_at(identity, revision).await?;
            match self.get_token(identity, None).await {
                Ok(token) => {
                    expires_at_epoch_ms = expires_at_epoch_ms.max(token.token.expires_at_epoch_ms)
                }
                Err(
                    AuthorizationError::Network
                    | AuthorizationError::Timeout
                    | AuthorizationError::Relay,
                ) => {}
                Err(error) => return Err(error),
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        platform::*,
        relay::{DpopSigner, ManagedRelayClient, RelayError},
        testing::FakePlatform,
        types::Generation,
    };
    use futures::executor::block_on;
    use serde_json::{Value, json};
    use std::collections::VecDeque;
    use t3_contracts::relay::RelayPublicClientId;

    type HostHandler = Rc<dyn Fn(HostAction) -> Result<HostReply, PlatformError>>;
    type ResponseFuture = LocalBoxFuture<'static, Result<HttpResponse, PlatformError>>;
    #[derive(Default)]
    pub(crate) struct TestPlatform {
        pub fake: FakePlatform,
        pub cloud_account: RefCell<Option<String>>,
        pub requests: RefCell<Vec<HttpRequest>>,
        responses: RefCell<VecDeque<ResponseFuture>>,
        pub before_write: RefCell<Option<Rc<dyn Fn()>>>,
        pub host_handler: RefCell<Option<HostHandler>>,
        pub before_transaction: RefCell<Option<LocalBoxFuture<'static, ()>>>,
    }
    impl TestPlatform {
        pub fn respond(&self, status: u16, body: Value) {
            self.responses.borrow_mut().push_back(
                futures::future::ready(Ok(HttpResponse {
                    status,
                    headers: BTreeMap::new(),
                    body: serde_json::to_vec(&body).unwrap(),
                }))
                .boxed_local(),
            );
        }
        pub fn pending(&self) -> futures::channel::oneshot::Sender<HttpResponse> {
            let (tx, rx) = futures::channel::oneshot::channel();
            self.responses.borrow_mut().push_back(
                async move { rx.await.map_err(|_| PlatformError::Closed) }.boxed_local(),
            );
            tx
        }
        pub fn respond_error(&self, error: PlatformError) {
            self.responses
                .borrow_mut()
                .push_back(futures::future::ready(Err(error)).boxed_local());
        }
    }
    impl Storage for TestPlatform {
        fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
            self.fake.get(area, key)
        }
        fn list(&self, area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>> {
            self.fake.list(area, namespace)
        }
        fn transaction(&self, transaction: StorageTransaction) -> HostFuture<'_, bool> {
            async move {
                let hook = self.before_transaction.borrow_mut().take();
                if let Some(hook) = hook {
                    hook.await;
                }
                self.fake.transaction(transaction).await
            }
            .boxed_local()
        }
        fn write(&self, area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()> {
            async move {
                self.fake.write(area, writes).await?;
                if let Some(hook) = self.before_write.borrow().as_ref() {
                    hook();
                }
                Ok(())
            }
            .boxed_local()
        }
    }
    impl Platform for TestPlatform {
        fn cloud_identity(&self) -> HostFuture<'_, Option<CloudSessionIdentity>> {
            futures::future::ready(Ok(self
                .cloud_account
                .borrow()
                .clone()
                .map(|account_id| CloudSessionIdentity { account_id })))
            .boxed_local()
        }
        fn now_epoch_ms(&self) -> u64 {
            self.fake.now_epoch_ms()
        }
        fn monotonic_ms(&self) -> u64 {
            self.fake.monotonic_ms()
        }
        fn random_bytes(&self, length: usize) -> Result<Vec<u8>, PlatformError> {
            self.fake.random_bytes(length)
        }
        fn sleep(&self, milliseconds: u64) -> LocalBoxFuture<'_, ()> {
            self.fake.sleep(milliseconds)
        }
        fn yield_to_host(&self) -> LocalBoxFuture<'_, ()> {
            self.fake.yield_to_host()
        }
        fn events(&self) -> futures::stream::LocalBoxStream<'static, PlatformEvent> {
            self.fake.events()
        }
        fn http(&self, request: HttpRequest) -> HostFuture<'_, HttpResponse> {
            self.requests.borrow_mut().push(request);
            self.responses.borrow_mut().pop_front().unwrap_or_else(|| {
                futures::future::ready(Err(PlatformError::Network(
                    "MARKER_SECRET unscripted".into(),
                )))
                .boxed_local()
            })
        }
        fn websocket(&self, request: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>> {
            self.fake.websocket(request)
        }
        fn host(&self, action: HostAction) -> HostFuture<'_, HostReply> {
            if let Some(handler) = self.host_handler.borrow().as_ref() {
                return futures::future::ready(handler(action)).boxed_local();
            }
            self.fake.host(action)
        }
    }
    #[derive(Default)]
    pub(crate) struct TestSigner {
        pub proofs: RefCell<Vec<(String, String, Option<String>)>>,
    }
    impl DpopSigner for TestSigner {
        fn thumbprint(&self) -> LocalBoxFuture<'_, Result<String, RelayError>> {
            futures::future::ready(Ok("key".into())).boxed_local()
        }
        fn create_proof<'a>(
            &'a self,
            method: &'a str,
            url: &'a str,
            access_token: Option<&'a str>,
        ) -> LocalBoxFuture<'a, Result<Sensitive<String>, RelayError>> {
            self.proofs.borrow_mut().push((
                method.into(),
                url.into(),
                access_token.map(str::to_owned),
            ));
            futures::future::ready(Ok(Sensitive(format!(
                "proof-{}",
                self.proofs.borrow().len()
            ))))
            .boxed_local()
        }
    }
    pub(crate) fn jwt(account: &str) -> String {
        use base64::Engine;
        format!(
            "e30.{}.sig",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&json!({"sub":account,"exp":3600})).unwrap())
        )
    }
    pub(crate) fn identity() -> SessionIdentity {
        SessionIdentity {
            environment: EnvironmentId("env".into()),
            generation: Generation(1),
            account: Some("account".into()),
            dpop_thumbprint: Some("key".into()),
        }
    }
    pub(crate) fn endpoint() -> RelayManagedEndpoint {
        serde_json::from_value(json!({"httpBaseUrl":"https://managed.test","wsBaseUrl":"wss://managed.test","providerKind":"cloudflare_tunnel"})).unwrap()
    }
    pub(crate) fn descriptor(id: &str) -> Value {
        json!({"environmentId":id,"label":"Host","platform":{"os":"linux","arch":"x64"},"serverVersion":"1.0.0","capabilities":{"terminal":true,"desktopIntegration":false}})
    }
    pub(crate) fn remote_token(token: &str, expires: u64) -> RemoteDpopAccessToken {
        RemoteDpopAccessToken {
            environment_id: EnvironmentId("env".into()),
            account_id: Some("account".into()),
            label: "Host".into(),
            endpoint: endpoint(),
            access_token: Sensitive(token.into()),
            expires_at_epoch_ms: expires,
            dpop_thumbprint: "key".into(),
        }
    }
    pub(crate) fn service(
        platform: Rc<TestPlatform>,
        signer: Rc<TestSigner>,
    ) -> RemoteAuthorization {
        platform.cloud_account.replace(Some("account".into()));
        let relay = ManagedRelayClient::new(
            platform.clone(),
            signer.clone(),
            "https://relay.test",
            RelayPublicClientId::T3Web,
        );
        let service = RemoteAuthorization::new(platform, signer, relay);
        service.set_session(identity());
        service
    }

    #[test]
    fn live_signout_rejects_and_erases_a_cached_environment_token() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let service = service(platform.clone(), Rc::new(TestSigner::default()));
            service
                .store
                .put(&remote_token("old-access", 1_000_000))
                .await
                .unwrap();
            platform.cloud_account.replace(None);
            assert!(matches!(
                service.authorize_dpop_http(&identity(), None).await,
                Err(AuthorizationError::SessionChanged)
            ));
            assert!(
                service
                    .store
                    .get(&identity().environment)
                    .await
                    .unwrap()
                    .is_none()
            );
        });
    }

    #[test]
    fn live_account_switch_rejects_and_erases_the_previous_accounts_token() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let service = service(platform.clone(), Rc::new(TestSigner::default()));
            service
                .store
                .put(&remote_token("old-access", 1_000_000))
                .await
                .unwrap();
            platform
                .cloud_account
                .replace(Some("different-account".into()));
            assert!(matches!(
                service.authorize_dpop_http(&identity(), None).await,
                Err(AuthorizationError::SessionChanged)
            ));
            assert!(
                service
                    .store
                    .get(&identity().environment)
                    .await
                    .unwrap()
                    .is_none()
            );
        });
    }

    #[test]
    fn credentials_changed_invalidates_the_cached_session_before_authorization() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let service = service(platform.clone(), Rc::new(TestSigner::default()));
            service
                .store
                .put(&remote_token("old-access", 1_000_000))
                .await
                .unwrap();
            platform.fake.emit(PlatformEvent::CredentialsChanged);
            assert!(matches!(
                service.authorize_dpop_http(&identity(), None).await,
                Err(AuthorizationError::SessionChanged)
            ));
            assert!(
                service
                    .store
                    .get(&identity().environment)
                    .await
                    .unwrap()
                    .is_none()
            );
        });
    }

    #[test]
    fn cloud_account_signout_and_switch_revoke_authorization_synchronously() {
        use crate::relay::{CloudAccountHandlers, CloudAccountService};
        struct Handlers;
        impl CloudAccountHandlers for Handlers {
            fn deactivate(&self) {}
            fn clean_up(
                &self,
                _: Option<String>,
                _: bool,
            ) -> LocalBoxFuture<'_, Result<(), RelayError>> {
                futures::future::ready(Ok(())).boxed_local()
            }
            fn prepare(
                &self,
                _: String,
                _: Rc<dyn Fn() -> bool>,
            ) -> LocalBoxFuture<'_, Result<(), RelayError>> {
                futures::future::ready(Ok(())).boxed_local()
            }
            fn activate(&self, _: &str, _: bool) {}
            fn detach(&self) {}
        }
        for next in [None, Some("different-account".to_owned())] {
            let platform = Rc::new(TestPlatform::default());
            let remote = service(platform.clone(), Rc::new(TestSigner::default()));
            block_on(remote.store.put(&remote_token("old-access", 1_000_000))).unwrap();
            let (accounts, mut driver) =
                CloudAccountService::new(Rc::new(Handlers), remote.clone());
            accounts.observe(Some(Some("account".into())));
            assert!(driver.as_mut().now_or_never().is_none());
            accounts.observe(Some(next));
            assert_eq!(
                remote.assert_session(&identity()),
                Err(AuthorizationError::SessionChanged)
            );
            assert!(driver.as_mut().now_or_never().is_none());
            assert!(
                block_on(remote.store.get(&identity().environment))
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn registry_removal_revokes_authorization_before_any_storage_io() {
        use crate::{
            connection::{ConnectionMode, Route, Time},
            environment::{EnvironmentRegistration, EnvironmentRegistry},
        };
        let platform = Rc::new(TestPlatform::default());
        let remote = service(platform, Rc::default());
        let mut registry = EnvironmentRegistry::<()>::with_authorization(remote.clone());
        registry
            .register(
                EnvironmentRegistration {
                    id: identity().environment,
                    label: "Host".into(),
                    enabled: true,
                    routes: vec![Route {
                        id: "relay".into(),
                        environment: identity().environment,
                        mode: ConnectionMode::Relay,
                        http_base: String::new(),
                        ws_base: String::new(),
                    }],
                },
                false,
                NetworkStatus::Online,
                Time::default(),
            )
            .unwrap();
        registry
            .remove(&identity().environment, Time::default())
            .unwrap();
        assert_eq!(
            remote.assert_session(&identity()),
            Err(AuthorizationError::SessionChanged)
        );
        assert_eq!(
            block_on(remote.authorize_dpop_http(&identity(), None)).err(),
            Some(AuthorizationError::SessionChanged)
        );
    }

    #[test]
    fn catalog_restore_captures_the_live_signin_without_manual_session_setup() {
        use crate::{
            connection::{ConnectionMode, Route, Time},
            environment::{EnvironmentRegistration, EnvironmentRegistry},
        };
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            platform.cloud_account.replace(Some("account".into()));
            let signer = Rc::new(TestSigner::default());
            let relay = ManagedRelayClient::new(
                platform.clone(),
                signer.clone(),
                "https://relay.test",
                RelayPublicClientId::T3Web,
            );
            let remote = RemoteAuthorization::new(platform.clone(), signer, relay);
            let mut catalog = EnvironmentRegistry::<()>::default();
            catalog
                .register(
                    EnvironmentRegistration {
                        id: identity().environment,
                        label: "Host".into(),
                        enabled: true,
                        routes: vec![Route {
                            id: "relay".into(),
                            environment: identity().environment,
                            mode: ConnectionMode::Relay,
                            http_base: String::new(),
                            ws_base: String::new(),
                        }],
                    },
                    false,
                    NetworkStatus::Online,
                    Time::default(),
                )
                .unwrap();
            catalog.save(platform.as_ref()).await.unwrap();
            remote
                .store
                .put(&remote_token("restored-access", 1_000_000))
                .await
                .unwrap();
            let (mut registry, startup) = EnvironmentRegistry::<()>::load_authorized(
                platform.as_ref(),
                remote.clone(),
                NetworkStatus::Online,
                Time::default(),
            )
            .await
            .unwrap();
            let captured = registry
                .get(&identity().environment)
                .unwrap()
                .pending_identity()
                .unwrap();
            assert_eq!(captured, identity());
            assert_eq!(startup.actions.len(), 1);
            assert_eq!(
                remote
                    .authorize_dpop_http(&captured, None)
                    .await
                    .unwrap()
                    .1
                    .access_token(),
                Some("restored-access")
            );
            registry
                .remove(&captured.environment, Time::default())
                .unwrap();
            assert_eq!(
                remote.assert_session(&captured),
                Err(AuthorizationError::SessionChanged)
            );
            assert_eq!(
                remote.begin_session(&captured).await,
                Err(AuthorizationError::SessionChanged)
            );
            registry.save(platform.as_ref()).await.unwrap();
            assert!(
                remote
                    .store
                    .get(&captured.environment)
                    .await
                    .unwrap()
                    .is_none()
            );
        });
    }
    pub(crate) fn relay_access() -> Value {
        json!({"access_token":"relay-access","issued_token_type":"urn:ietf:params:oauth:token-type:access_token","token_type":"DPoP","expires_in":3600,"scope":"environment:connect"})
    }
    pub(crate) fn environment_access(token: &str) -> Value {
        json!({"access_token":token,"issued_token_type":"urn:ietf:params:oauth:token-type:access_token","token_type":"DPoP","expires_in":3600,"scope":"orchestration:read"})
    }
    pub(crate) fn script_mint(platform: &TestPlatform, token: &str) {
        platform
            .fake
            .push_host(Ok(HostReply::Text(Some(jwt("account")))));
        platform.fake.push_host(Ok(HostReply::Text(None)));
        platform.respond(200, relay_access());
        platform.respond(200,json!({"environmentId":"env","endpoint":endpoint(),"credential":"MARKER_BOOTSTRAP","expiresAt":"2036-01-01T00:00:00.000Z"}));
        platform.respond(200, descriptor("env"));
        platform.respond(200, environment_access(token));
    }
    #[test]
    fn token_identity_key_generation_account_expiry_skew_and_rejection() {
        let identity = identity();
        let token = AccessToken {
            session: identity.clone(),
            token: "MARKER_SECRET".into(),
            expires_at_epoch_ms: 60_001,
        };
        assert!(token.usable(&identity, 0, None));
        assert!(!token.usable(&identity, 1, None));
        assert!(!token.usable(&identity, 0, Some("MARKER_SECRET")));
        for other in [
            SessionIdentity {
                generation: Generation(2),
                ..identity.clone()
            },
            SessionIdentity {
                account: Some("other".into()),
                ..identity.clone()
            },
            SessionIdentity {
                environment: EnvironmentId("other".into()),
                ..identity.clone()
            },
            SessionIdentity {
                dpop_thumbprint: Some("other".into()),
                ..identity
            },
        ] {
            assert!(!token.usable(&other, 0, None));
        }
        assert!(!format!("{token:?}").contains("MARKER_SECRET"));
    }
    #[test]
    fn descriptor_mismatch_spends_no_pairing_or_bearer_credential() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let service = service(platform.clone(), Rc::default());
            platform.respond(200, descriptor("other"));
            let error = service
                .pair_bearer(
                    &identity(),
                    "https://host.test",
                    "wss://host.test",
                    Sensitive("MARKER_SECRET".into()),
                    None,
                    None,
                )
                .await
                .err()
                .unwrap();
            assert_eq!(error, AuthorizationError::EnvironmentMismatch);
            assert_eq!(platform.requests.borrow().len(), 1);
            assert!(platform.requests.borrow()[0].headers.is_empty());
            assert!(platform.requests.borrow()[0].body.is_empty());
        });
    }
    #[test]
    fn scoped_token_exchange_uses_contract_form_and_metadata() {
        block_on(async {
            let platform = TestPlatform::default();
            let mut result = environment_access("token");
            result["token_type"] = json!("Bearer");
            platform.respond(200, result);
            let metadata: AuthClientPresentationMetadata = serde_json::from_value(
                json!({"label":"T3 Mobile","deviceType":"mobile","os":"unknown"}),
            )
            .unwrap();
            bootstrap_remote_session(
                &platform,
                "https://host.test/path?old=1",
                "MARKER_SECRET",
                Some(&[AuthEnvironmentScope::OrchestrationRead]),
                Some(&metadata),
                None,
            )
            .await
            .unwrap();
            let request = &platform.requests.borrow()[0];
            assert_eq!(request.url, "https://host.test/oauth/token");
            let form: url::form_urlencoded::Parse<'_> = url::form_urlencoded::parse(&request.body);
            let values = form.into_owned().collect::<BTreeMap<_, _>>();
            assert_eq!(values["subject_token"], "MARKER_SECRET");
            assert_eq!(values["scope"], "orchestration:read");
            assert_eq!(values["client_label"], "T3 Mobile");
            assert!(!values.contains_key("client_os"));
        });
    }
    #[test]
    fn websocket_dpop_fallback_reproofs_each_path_and_ticket_upgrade() {
        block_on(async {
            let platform = TestPlatform::default();
            let signer = TestSigner::default();
            platform.respond(404, json!({}));
            platform.respond(
                200,
                json!({"ticket":"MARKER_TICKET","expiresAt":"2036-01-01T00:00:00.000Z"}),
            );
            let auth = HttpAuthorization::Dpop {
                access_token: Sensitive("MARKER_ACCESS".into()),
                expires_at_epoch_ms: 3600000,
            };
            let url = websocket_connection_url(
                &platform,
                "https://host.test",
                "wss://host.test/?wsTicket=old&dpop=old",
                &auth,
                Some(&signer),
                None,
                Some("relay"),
                10000,
            )
            .await
            .unwrap();
            assert_eq!(platform.requests.borrow().len(), 2);
            let proofs = signer.proofs.borrow();
            assert_eq!(proofs.len(), 3);
            assert!(proofs[0].1.ends_with("/websocket-ticket/rpc"));
            assert!(proofs[1].1.ends_with("/websocket-ticket"));
            assert_eq!(
                proofs[2],
                (
                    "GET".into(),
                    "https://host.test/ws".into(),
                    Some("MARKER_TICKET".into())
                )
            );
            let parsed = Url::parse(url.expose()).unwrap();
            assert_eq!(
                parsed
                    .query_pairs()
                    .filter(|(key, _)| key == "wsTicket")
                    .count(),
                1
            );
            assert!(!format!("{url:?}").contains("MARKER"));
        });
    }
    #[test]
    fn websocket_bearer_ticket_and_mobile_metadata() {
        block_on(async {
            let platform = TestPlatform::default();
            platform.respond(
                200,
                json!({"ticket":"ticket","expiresAt":"2036-01-01T00:00:00.000Z"}),
            );
            let metadata=serde_json::from_value(json!({"surface":"mobile","appVersion":"1.2.3","deviceType":"mobile","os":"Android","osMajorVersion":15,"deviceModel":"Pixel 9"})).unwrap();
            let url = websocket_connection_url(
                &platform,
                "https://host.test",
                "wss://host.test",
                &HttpAuthorization::Bearer(Sensitive("MARKER_SECRET".into())),
                None,
                Some(&metadata),
                Some("relay"),
                10000,
            )
            .await
            .unwrap();
            assert_eq!(
                url.expose(),
                "wss://host.test/ws?wsTicket=ticket&clientSurface=mobile&clientAppVersion=1.2.3&clientDeviceType=phone&clientOs=Android&clientOsMajorVersion=15&clientDeviceModel=Pixel+9&connectionMethod=relay"
            );
        });
    }
    #[test]
    fn secure_store_never_writes_ordinary_storage_and_redacts_loaded_records() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let store = RemoteTokenStore::new(platform.clone());
            let token = remote_token("MARKER_SECRET", 3600000);
            store.put(&token).await.unwrap();
            let loaded = store.get(&identity().environment).await.unwrap().unwrap();
            assert_eq!(loaded.access_token.expose(), "MARKER_SECRET");
            assert!(!format!("{loaded:?} {store:?}").contains("MARKER_SECRET"));
            assert!(
                platform
                    .list(
                        StorageArea::Ordinary,
                        crate::persistence::REMOTE_DPOP_TOKEN_KIND.into()
                    )
                    .await
                    .unwrap()
                    .is_empty()
            );
            store.remove(&identity().environment).await.unwrap();
            assert!(store.get(&identity().environment).await.unwrap().is_none());
        });
    }
    #[test]
    fn cached_tokens_reuse_across_services_and_validate_direct_hosts_first() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let service = service(platform.clone(), Rc::default());
            service
                .store
                .put(&remote_token("cached", 3600000))
                .await
                .unwrap();
            let (_, auth) = service
                .authorize_dpop_http(&identity(), None)
                .await
                .unwrap();
            assert_eq!(auth.access_token(), Some("cached"));
            assert!(platform.requests.borrow().is_empty());
            platform.respond(200, descriptor("other"));
            assert_eq!(
                service
                    .authorize_dpop(
                        &identity(),
                        Some(("https://direct.test", "wss://direct.test"))
                    )
                    .await
                    .err()
                    .unwrap(),
                AuthorizationError::EnvironmentMismatch
            );
            assert_eq!(platform.requests.borrow().len(), 1);
        });
    }
    #[test]
    fn expired_tokens_refresh_persist_and_reject_old_account_key_or_unowned_tokens() {
        block_on(async {
            for kind in 0..4 {
                let platform = Rc::new(TestPlatform::default());
                let service = service(platform.clone(), Rc::default());
                let mut old = remote_token("old", 3600000);
                match kind {
                    0 => old.expires_at_epoch_ms = 60000,
                    1 => old.account_id = Some("other".into()),
                    2 => old.account_id = None,
                    _ => old.dpop_thumbprint = "other".into(),
                };
                service.store.put(&old).await.unwrap();
                script_mint(&platform, "fresh");
                let (_, auth) = service
                    .authorize_dpop_http(&identity(), None)
                    .await
                    .unwrap();
                assert_eq!(auth.access_token(), Some("fresh"));
                assert_eq!(
                    service
                        .store
                        .get(&identity().environment)
                        .await
                        .unwrap()
                        .unwrap()
                        .access_token
                        .expose(),
                    "fresh"
                );
                assert_eq!(platform.requests.borrow().len(), 4);
            }
        });
    }
    #[test]
    fn pending_renewals_share_one_exchange_and_survive_one_caller_dropping() {
        let platform = Rc::new(TestPlatform::default());
        let service = service(platform.clone(), Rc::default());
        platform
            .fake
            .push_host(Ok(HostReply::Text(Some(jwt("account")))));
        platform.fake.push_host(Ok(HostReply::Text(None)));
        let release = platform.pending();
        platform.respond(200,json!({"environmentId":"env","endpoint":endpoint(),"credential":"bootstrap","expiresAt":"later"}));
        platform.respond(200, descriptor("env"));
        platform.respond(200, environment_access("fresh"));
        let identity = identity();
        let mut first = service.authorize_dpop_http(&identity, None).boxed_local();
        assert!(first.as_mut().now_or_never().is_none());
        let mut second = service.authorize_dpop_http(&identity, None).boxed_local();
        assert!(second.as_mut().now_or_never().is_none());
        assert_eq!(platform.requests.borrow().len(), 1);
        drop(first);
        release
            .send(HttpResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: serde_json::to_vec(&relay_access()).unwrap(),
            })
            .unwrap();
        assert_eq!(block_on(second).unwrap().1.access_token(), Some("fresh"));
        assert_eq!(platform.requests.borrow().len(), 4);
    }
    #[test]
    fn logout_during_exchange_or_storage_does_not_publish_or_retain_tokens() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let service = service(platform.clone(), Rc::default());
            script_mint(&platform, "MARKER_SECRET");
            let service_copy = service.clone();
            platform.before_write.replace(Some(Rc::new(move || {
                service_copy.clear_session(&identity().environment)
            })));
            assert_eq!(
                service
                    .authorize_dpop_http(&identity(), None)
                    .await
                    .err()
                    .unwrap(),
                AuthorizationError::SessionChanged
            );
            assert!(
                service
                    .store
                    .get(&identity().environment)
                    .await
                    .unwrap()
                    .is_none()
            );
        });
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let service = service(platform.clone(), Rc::default());
            platform
                .fake
                .push_host(Ok(HostReply::Text(Some(jwt("account")))));
            platform.fake.push_host(Ok(HostReply::Text(None)));
            platform.respond(200, relay_access());
            platform.respond(200, json!({"environmentId":"env","endpoint":endpoint(),"credential":"bootstrap","expiresAt":"later"}));
            platform.respond(200, descriptor("env"));
            let release = platform.pending();
            let identity = identity();
            let mut old_exchange = service.authorize_dpop_http(&identity, None).boxed_local();
            assert!(old_exchange.as_mut().now_or_never().is_none());
            service.invalidate_sessions();
            service.begin_session(&identity).await.unwrap();
            release
                .send(HttpResponse {
                    status: 200,
                    headers: BTreeMap::new(),
                    body: serde_json::to_vec(&environment_access("stale-access")).unwrap(),
                })
                .unwrap();
            assert_eq!(
                old_exchange.await.err(),
                Some(AuthorizationError::SessionChanged)
            );
            assert!(
                service
                    .store
                    .get(&identity.environment)
                    .await
                    .unwrap()
                    .is_none()
            );
        });
    }
    #[test]
    fn late_rejection_cannot_remove_replacement_token() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let service = service(platform.clone(), Rc::default());
            service
                .store
                .put(&remote_token("replacement", 3600000))
                .await
                .unwrap();
            service
                .reject_token(&identity().environment, "old")
                .await
                .unwrap();
            assert_eq!(
                service
                    .store
                    .get(&identity().environment)
                    .await
                    .unwrap()
                    .unwrap()
                    .access_token
                    .expose(),
                "replacement"
            );
            service
                .store
                .put(&remote_token("old", 3600000))
                .await
                .unwrap();
            let writer = service.store.clone();
            platform.before_transaction.replace(Some(
                async move {
                    writer.put(&remote_token("newer", 3600000)).await.unwrap();
                }
                .boxed_local(),
            ));
            service
                .reject_token(&identity().environment, "old")
                .await
                .unwrap();
            assert_eq!(
                service
                    .store
                    .get(&identity().environment)
                    .await
                    .unwrap()
                    .unwrap()
                    .access_token
                    .expose(),
                "newer"
            );
        });
    }
    #[test]
    fn descriptor_cache_is_one_shot_fresh_and_rejects_unstable_plain_http_hosts() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let service = service(platform.clone(), Rc::default());
            let descriptor: ExecutionEnvironmentDescriptor =
                serde_json::from_value(descriptor("env")).unwrap();
            service
                .remember_checked_descriptor("https://host.test", Some(descriptor.clone()), true)
                .unwrap();
            service
                .verify_endpoint(&identity(), "https://host.test")
                .await
                .unwrap();
            assert!(platform.requests.borrow().is_empty());
            platform.respond(200, json!(descriptor));
            service
                .verify_endpoint(&identity(), "https://host.test")
                .await
                .unwrap();
            assert_eq!(platform.requests.borrow().len(), 1);
            service
                .remember_checked_descriptor("http://192.168.1.3", Some(descriptor.clone()), true)
                .unwrap();
            platform.respond(200, json!(descriptor));
            service
                .verify_endpoint(&identity(), "http://192.168.1.3")
                .await
                .unwrap();
            assert_eq!(platform.requests.borrow().len(), 2);
            service
                .remember_checked_descriptor("https://host.test", Some(descriptor.clone()), true)
                .unwrap();
            platform.fake.advance(5001);
            platform.respond(200, json!(descriptor));
            service
                .verify_endpoint(&identity(), "https://host.test")
                .await
                .unwrap();
            assert_eq!(platform.requests.borrow().len(), 3);
        });
    }
    #[test]
    fn public_auth_types_and_untrusted_transport_response_errors_never_render_secrets() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let service = service(platform.clone(), Rc::default());
            platform.respond_error(PlatformError::Network("MARKER_SECRET".into()));
            let error = bootstrap_remote_session(
                platform.as_ref(),
                "https://host.test",
                "MARKER_SECRET",
                None,
                None,
                None,
            )
            .await
            .err()
            .unwrap();
            assert!(!format!("{error:?} {error}").contains("MARKER_SECRET"));
            platform.respond(
                400,
                json!({"message":"MARKER_SECRET","access_token":"MARKER_SECRET"}),
            );
            let error = bootstrap_remote_session(
                platform.as_ref(),
                "https://host.test",
                "MARKER_SECRET",
                None,
                None,
                None,
            )
            .await
            .err()
            .unwrap();
            assert!(!format!("{error:?} {error}").contains("MARKER_SECRET"));
            let prepared = AuthorizedEnvironment {
                session: identity(),
                http_base_url: "https://MARKER_SECRET@host.test".into(),
                socket_url: Sensitive("MARKER_SECRET".into()),
                authorization: HttpAuthorization::Bearer(Sensitive("MARKER_SECRET".into())),
                descriptor: None,
            };
            assert!(
                !format!("{prepared:?} {:?} {service:?}", prepared.authorization)
                    .contains("MARKER_SECRET")
            );
            assert_eq!(
                service
                    .authorize_bearer(
                        &identity(),
                        "https://user:MARKER_SECRET@host.test",
                        "wss://host.test",
                        Sensitive("MARKER_SECRET".into()),
                        "direct"
                    )
                    .await
                    .err()
                    .unwrap(),
                AuthorizationError::InvalidUrl
            );
        });
    }
}

#[cfg(test)]
mod additional_authorization_tests {
    use super::tests::*;
    use super::*;
    use crate::platform::{HostReply, HttpResponse};
    use futures::{FutureExt, executor::block_on};
    use serde_json::json;
    #[test]
    fn owner_telemetry_is_hashed_before_becoming_a_query_parameter() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            platform.host_handler.replace(Some(Rc::new(|action| {
                Ok(match action {
                    crate::platform::HostAction::CloudIdentity => {
                        HostReply::Document(json!({"accountId":"MARKER_ACCOUNT"}))
                    }
                    crate::platform::HostAction::AnalyticsDeviceId => {
                        HostReply::Text(Some("MARKER_DEVICE".into()))
                    }
                    _ => HostReply::Unit,
                })
            })));
            platform.respond(
                200,
                json!({"ticket":"ticket","expiresAt":"2036-01-01T00:00:00.000Z"}),
            );
            let url = websocket_connection_url(
                platform.as_ref(),
                "https://host.test",
                "wss://host.test",
                &HttpAuthorization::Bearer(Sensitive("token".into())),
                None,
                None,
                None,
                10000,
            )
            .await
            .unwrap();
            assert!(!url.expose().contains("MARKER"));
            let parsed = Url::parse(url.expose()).unwrap();
            let fields = parsed
                .query_pairs()
                .into_owned()
                .collect::<BTreeMap<_, _>>();
            assert_eq!(fields["clientOwner"].len(), 64);
            assert_eq!(fields["clientDevice"].len(), 64);
        });
    }
    #[test]
    fn cached_managed_socket_failure_refreshes_and_new_ticket_failure_evicts_the_token() {
        block_on(async {
            for succeeds in [true, false] {
                let platform = Rc::new(TestPlatform::default());
                let service = service(platform.clone(), Rc::default());
                service
                    .store
                    .put(&remote_token("cached", 3600000))
                    .await
                    .unwrap();
                platform.respond(502, json!({}));
                script_mint(&platform, "fresh");
                platform.respond(if succeeds {200}else{401},if succeeds {json!({"ticket":"ticket","expiresAt":"2036-01-01T00:00:00.000Z"})}else{json!({"_tag":"EnvironmentAuthInvalidError","code":"auth_invalid","reason":"invalid_credential","traceId":"trace"})});
                let result = service.authorize_dpop(&identity(), None).await;
                if succeeds {
                    assert_eq!(result.unwrap().authorization.access_token(), Some("fresh"));
                } else {
                    assert_eq!(
                        result.err().unwrap(),
                        AuthorizationError::CredentialRejected
                    );
                    assert!(
                        service
                            .store
                            .get(&identity().environment)
                            .await
                            .unwrap()
                            .is_none()
                    );
                }
            }
        });
    }
    #[test]
    fn direct_transient_failure_preserves_a_token_for_the_managed_route() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let service = service(platform.clone(), Rc::default());
            service
                .store
                .put(&remote_token("cached", 3600000))
                .await
                .unwrap();
            platform.respond(200, descriptor("env"));
            platform.respond(502, json!({}));
            assert_eq!(
                service
                    .authorize_dpop(
                        &identity(),
                        Some(("https://direct.test", "wss://direct.test"))
                    )
                    .await
                    .err()
                    .unwrap(),
                AuthorizationError::HttpStatus(502)
            );
            assert_eq!(
                service
                    .store
                    .get(&identity().environment)
                    .await
                    .unwrap()
                    .unwrap()
                    .access_token
                    .expose(),
                "cached"
            );
            assert_eq!(platform.requests.borrow().len(), 2);
        });
    }
    #[test]
    fn pending_session_retries_and_same_account_reauthentication_requires_a_fresh_token() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let service = service(platform.clone(), Rc::default());
            service
                .store
                .put(&remote_token("cached", 3600000))
                .await
                .unwrap();
            service
                .authorize_dpop_http(&identity(), None)
                .await
                .unwrap();
            let mut next = identity();
            next.generation.0 += 1;
            service.set_session(next.clone());
            script_mint(&platform, "fresh");
            assert_eq!(
                service
                    .authorize_dpop_http(&next, None)
                    .await
                    .unwrap()
                    .1
                    .access_token(),
                Some("fresh")
            );
            next.account = None;
            service.set_session(next.clone());
            assert_eq!(
                service
                    .authorize_dpop_http(&next, None)
                    .await
                    .err()
                    .unwrap(),
                AuthorizationError::SessionPending
            );
        });
    }
    #[test]
    fn mismatched_bootstrap_identity_is_rejected_before_descriptor_or_pairing_exchange() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let service = service(platform.clone(), Rc::default());
            platform
                .fake
                .push_host(Ok(HostReply::Text(Some(jwt("account")))));
            platform.fake.push_host(Ok(HostReply::Text(None)));
            platform.respond(200, relay_access());
            platform.respond(200,json!({"environmentId":"other","endpoint":endpoint(),"credential":"MARKER_SECRET","expiresAt":"later"}));
            assert_eq!(
                service
                    .authorize_dpop_http(&identity(), None)
                    .await
                    .err()
                    .unwrap(),
                AuthorizationError::EnvironmentMismatch
            );
            assert_eq!(platform.requests.borrow().len(), 2);
        });
    }
    #[test]
    fn failed_shared_renewal_is_removed_so_a_later_call_can_retry() {
        let platform = Rc::new(TestPlatform::default());
        let service = service(platform.clone(), Rc::default());
        platform
            .fake
            .push_host(Ok(HostReply::Text(Some(jwt("account")))));
        platform.fake.push_host(Ok(HostReply::Text(None)));
        let release = platform.pending();
        let identity = identity();
        let mut a = service.authorize_dpop_http(&identity, None).boxed_local();
        let mut b = service.authorize_dpop_http(&identity, None).boxed_local();
        assert!(a.as_mut().now_or_never().is_none());
        assert!(b.as_mut().now_or_never().is_none());
        release
            .send(HttpResponse {
                status: 503,
                headers: BTreeMap::new(),
                body: vec![],
            })
            .unwrap();
        assert_eq!(block_on(a).unwrap_err(), AuthorizationError::Relay);
        assert_eq!(block_on(b).unwrap_err(), AuthorizationError::Relay);
        assert!(service.state.borrow().pending.is_empty());
        script_mint(&platform, "fresh");
        assert_eq!(
            block_on(service.authorize_dpop_http(&identity, None))
                .unwrap()
                .1
                .access_token(),
            Some("fresh")
        );
    }
    #[test]
    fn renewal_before_expiry_uses_manual_clock_and_stops_on_signout() {
        let platform = Rc::new(TestPlatform::default());
        let service = service(platform.clone(), Rc::default());
        let identity = identity();
        block_on(service.store.put(&remote_token("cached", 120000))).unwrap();
        let mut renew = service
            .renew_dpop_before_expiry(&identity, 120000)
            .boxed_local();
        assert!(renew.as_mut().now_or_never().is_none());
        script_mint(&platform, "fresh");
        platform.fake.advance(60000);
        assert!(renew.as_mut().now_or_never().is_none());
        assert_eq!(
            block_on(service.store.get(&identity.environment))
                .unwrap()
                .unwrap()
                .access_token
                .expose(),
            "fresh"
        );
        service.clear_session(&identity.environment);
        platform.fake.advance(3600000);
        assert_eq!(
            block_on(renew).unwrap_err(),
            AuthorizationError::SessionChanged
        );
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::tests::*;
    use super::*;
    use crate::platform::HostReply;
    #[test]
    fn dropping_the_last_waiter_and_service_releases_pending_credentials() {
        let platform = Rc::new(TestPlatform::default());
        let service = service(platform.clone(), Rc::default());
        let state = Rc::downgrade(&service.state);
        platform
            .fake
            .push_host(Ok(HostReply::Text(Some(jwt("account")))));
        platform.fake.push_host(Ok(HostReply::Text(None)));
        let _release = platform.pending();
        let identity = identity();
        let mut pending = service.authorize_dpop_http(&identity, None).boxed_local();
        assert!(pending.as_mut().now_or_never().is_none());
        drop(pending);
        drop(service);
        assert!(state.upgrade().is_none());
    }
}

#[cfg(test)]
mod endpoint_timeout_tests {
    use super::tests::*;
    use super::*;
    use futures::executor::block_on;
    #[test]
    fn direct_lan_and_loopback_descriptors_time_out_earlier_than_public_or_ssh_routes() {
        for (base, method, timeout) in [
            ("http://127.0.0.1", "direct", 3000),
            ("http://192.168.1.4", "direct", 3000),
            ("https://public.test", "direct", 10000),
            ("http://127.0.0.1", "ssh", 10000),
        ] {
            let platform = Rc::new(TestPlatform::default());
            let service = service(platform.clone(), Rc::default());
            let _release = platform.pending();
            let identity = identity();
            let mut pending = service
                .authorize_bearer(
                    &identity,
                    base,
                    "wss://host.test",
                    Sensitive("MARKER_SECRET".into()),
                    method,
                )
                .boxed_local();
            assert!(pending.as_mut().now_or_never().is_none());
            platform.fake.advance(timeout - 1);
            assert!(pending.as_mut().now_or_never().is_none());
            platform.fake.advance(1);
            assert_eq!(
                block_on(pending).err().unwrap(),
                AuthorizationError::Timeout
            );
        }
    }
    #[test]
    fn typed_remote_error_details_are_retained_but_never_render_credentials() {
        let error=response_json::<serde_json::Value>(HttpResponse {status:403,headers:BTreeMap::new(),body:serde_json::to_vec(&serde_json::json!({"_tag":"EnvironmentScopeRequiredError","code":"insufficient_scope","requiredScope":"orchestration:read","requiredPermission":"MARKER_SECRET","traceId":"MARKER_SECRET"})).unwrap()}).unwrap_err();
        assert!(matches!(
            error,
            AuthorizationError::Remote { status: 403, .. }
        ));
        assert!(!format!("{error:?} {error}").contains("MARKER_SECRET"));
    }
}
