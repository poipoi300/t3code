//! Managed relay HTTP client, proof construction and account-scoped discovery.
use crate::{
    authorization::{
        AuthorizationError, Sensitive, form, http_url, parse_endpoint, request, scalar_string,
        send_http,
    },
    platform::{
        Credentials, HostAction, HostReply, Platform, StorageArea, StorageKey, StorageWrite,
    },
    types::EnvironmentId,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::{FutureExt, future::LocalBoxFuture};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    fmt,
    rc::Rc,
};
use t3_contracts::{
    base_schemas::{DpopFailureReason, TrimmedNonEmptyString},
    relay::*,
};

#[derive(Clone, PartialEq, Eq)]
pub enum RelayStatus {
    SignedOut,
    Discovering,
    Ready,
    Unavailable(String),
}
impl fmt::Debug for RelayStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::SignedOut => "SignedOut",
            Self::Discovering => "Discovering",
            Self::Ready => "Ready",
            Self::Unavailable(_) => "Unavailable([REDACTED])",
        })
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct RelayEndpoint {
    pub environment: EnvironmentId,
    pub url: String,
    pub direct_candidates: Vec<String>,
}
impl fmt::Debug for RelayEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RelayEndpoint([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RelayError {
    #[error("relay URL must be a secure absolute HTTPS origin")]
    InvalidUrl,
    #[error("could not load relay proof key")]
    Key,
    #[error("could not create relay authorization proof")]
    Proof,
    #[error("relay request timed out")]
    Timeout,
    #[error("relay transport failed")]
    Network,
    #[error("relay token storage failed")]
    Storage,
    #[error("relay returned invalid data")]
    InvalidResponse,
    #[error("relay granted unexpected access token scopes")]
    UnexpectedScopes,
    #[error("cloud session changed")]
    SessionChanged,
    #[error("cloud session token is unavailable")]
    SignedOut,
    #[error("relay returned a different environment or endpoint")]
    EnvironmentMismatch,
    #[error("{}", relay_failure_detail(*status, error.as_ref().map(Sensitive::expose)))]
    Request {
        status: u16,
        error: Option<Sensitive<RelayProtectedError>>,
        trace_id: Option<Sensitive<String>>,
    },
}
impl From<AuthorizationError> for RelayError {
    fn from(value: AuthorizationError) -> Self {
        match value {
            AuthorizationError::InvalidUrl => Self::InvalidUrl,
            AuthorizationError::Timeout => Self::Timeout,
            AuthorizationError::Network => Self::Network,
            _ => Self::InvalidResponse,
        }
    }
}
pub trait DpopSigner {
    fn thumbprint(&self) -> LocalBoxFuture<'_, Result<String, RelayError>>;
    fn create_proof<'a>(
        &'a self,
        method: &'a str,
        url: &'a str,
        access_token: Option<&'a str>,
    ) -> LocalBoxFuture<'a, Result<Sensitive<String>, RelayError>>;
}
#[derive(Clone)]
pub struct PlatformDpopSigner {
    platform: Rc<dyn Platform>,
    jwk: Value,
    thumbprint: String,
}
impl fmt::Debug for PlatformDpopSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PlatformDpopSigner([REDACTED])")
    }
}
impl PlatformDpopSigner {
    pub fn new(platform: Rc<dyn Platform>, jwk: Value) -> Result<Self, RelayError> {
        if jwk.get("kty").and_then(Value::as_str) != Some("EC")
            || jwk.get("crv").and_then(Value::as_str) != Some("P-256")
        {
            return Err(RelayError::Key);
        }
        let x = jwk
            .get("x")
            .and_then(Value::as_str)
            .ok_or(RelayError::Key)?;
        let y = jwk
            .get("y")
            .and_then(Value::as_str)
            .ok_or(RelayError::Key)?;
        for value in [x, y] {
            if URL_SAFE_NO_PAD
                .decode(value)
                .map_err(|_| RelayError::Key)?
                .len()
                != 32
            {
                return Err(RelayError::Key);
            }
        }
        // Only public members can reach a JWT, even if the host accidentally includes a private JWK.
        let jwk = json!({"crv":"P-256","kty":"EC","x":x,"y":y});
        let bytes = serde_json::to_vec(&jwk).map_err(|_| RelayError::Key)?;
        let thumbprint = URL_SAFE_NO_PAD.encode(Sha256::digest(bytes));
        Ok(Self {
            platform,
            jwk,
            thumbprint,
        })
    }
    pub async fn from_host(platform: Rc<dyn Platform>) -> Result<Self, RelayError> {
        let HostReply::Document(jwk) = platform
            .host(HostAction::DpopPublicJwk)
            .await
            .map_err(|_| RelayError::Key)?
        else {
            return Err(RelayError::Key);
        };
        Self::new(platform, jwk)
    }
    async fn assert_key(&self) -> Result<(), RelayError> {
        if self
            .platform
            .dpop_thumbprint()
            .await
            .map_err(|_| RelayError::Key)?
            .as_deref()
            != Some(self.thumbprint.as_str())
        {
            return Err(RelayError::Key);
        }
        Ok(())
    }
}
impl DpopSigner for PlatformDpopSigner {
    fn thumbprint(&self) -> LocalBoxFuture<'_, Result<String, RelayError>> {
        async move {
            self.assert_key().await?;
            Ok(self.thumbprint.clone())
        }
        .boxed_local()
    }
    fn create_proof<'a>(
        &'a self,
        method: &'a str,
        url: &'a str,
        access_token: Option<&'a str>,
    ) -> LocalBoxFuture<'a, Result<Sensitive<String>, RelayError>> {
        async move {
            let mut htu = parse_endpoint(url,&["http","https"]).map_err(|_| RelayError::Proof)?;
            htu.set_query(None); htu.set_fragment(None);
            self.assert_key().await?;
            let mut random = self.platform.random_bytes(16).map_err(|_| RelayError::Proof)?;
            if random.len() != 16 { return Err(RelayError::Proof); }
            random[6] = (random[6]&15)|64; random[8] = (random[8]&63)|128;
            let hex = random.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
            let jti = format!("{}-{}-{}-{}-{}",&hex[..8],&hex[8..12],&hex[12..16],&hex[16..20],&hex[20..]);
            let mut payload = json!({"htm":method.to_uppercase(),"htu":htu.as_str(),"iat":self.platform.now_epoch_ms()/1000,"jti":jti});
            if let Some(token) = access_token.filter(|token| !token.is_empty()) { payload["ath"] = Value::String(URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))); }
            let encode = |value: &Value| serde_json::to_vec(value).map(|bytes| URL_SAFE_NO_PAD.encode(bytes)).map_err(|_| RelayError::Proof);
            let input = format!("{}.{}",encode(&json!({"typ":"dpop+jwt","alg":"ES256","jwk":self.jwk}))?,encode(&payload)?);
            let signature = self.platform.dpop_sign(input.as_bytes().to_vec()).await.map_err(|_| RelayError::Proof)?;
            if signature.len() != 64 { return Err(RelayError::Proof); }
            Ok(Sensitive(format!("{input}.{}",URL_SAFE_NO_PAD.encode(signature))))
        }.boxed_local()
    }
}
pub fn normalize_secure_relay_url(value: &str) -> Result<String, RelayError> {
    let url = parse_endpoint(value.trim(), &["https"]).map_err(|_| RelayError::InvalidUrl)?;
    if url.query().is_some() || url.fragment().is_some() || !url.path().chars().all(|c| c == '/') {
        return Err(RelayError::InvalidUrl);
    }
    Ok(url.origin().ascii_serialization())
}
pub fn decode_relay_jwt(token: &str) -> Option<Sensitive<Value>> {
    let parts = token.split('.').collect::<Vec<_>>();
    if parts.len() != 3 || parts[1].len() > 64 * 1024 {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(parts[1]).ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    claims.is_object().then_some(Sensitive(claims))
}
pub fn relay_account_id(token: &str) -> Option<String> {
    decode_relay_jwt(token)?
        .expose()
        .get("sub")?
        .as_str()
        .filter(|sub| !sub.is_empty())
        .map(str::to_owned)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayAccessTokenCacheEntry {
    pub account_id: String,
    pub client_id: RelayPublicClientId,
    pub relay_url: String,
    pub thumbprint: String,
    pub scopes: Vec<RelayDpopAccessTokenScope>,
    pub access_token: Sensitive<String>,
    pub expires_at_millis: u64,
}
impl fmt::Debug for RelayAccessTokenCacheEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RelayAccessTokenCacheEntry([REDACTED])")
    }
}
impl RelayAccessTokenCacheEntry {
    fn matches(
        &self,
        account: &str,
        client: RelayPublicClientId,
        relay: &str,
        thumbprint: &str,
        scopes: &[RelayDpopAccessTokenScope],
        now: u64,
    ) -> bool {
        self.account_id == account
            && self.client_id == client
            && self.relay_url == relay
            && self.thumbprint == thumbprint
            && self.expires_at_millis.saturating_sub(now) > 5_000
            && scopes.iter().all(|scope| self.scopes.contains(scope))
            && !self.access_token.expose().is_empty()
    }
}
#[derive(Default)]
struct RelayCache {
    loaded: bool,
    tokens: Vec<RelayAccessTokenCacheEntry>,
}
#[derive(Clone)]
pub struct ManagedRelayClient {
    platform: Rc<dyn Platform>,
    signer: Rc<dyn DpopSigner>,
    relay_url: Result<String, RelayError>,
    client_id: RelayPublicClientId,
    cache: Rc<RefCell<RelayCache>>,
    lock: Rc<futures::lock::Mutex<()>>,
    epoch: Rc<Cell<u64>>,
}
impl fmt::Debug for ManagedRelayClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ManagedRelayClient([REDACTED])")
    }
}
impl ManagedRelayClient {
    pub fn new(
        platform: Rc<dyn Platform>,
        signer: Rc<dyn DpopSigner>,
        relay_url: &str,
        client_id: RelayPublicClientId,
    ) -> Self {
        Self {
            platform,
            signer,
            relay_url: normalize_secure_relay_url(relay_url),
            client_id,
            cache: Rc::default(),
            lock: Rc::new(futures::lock::Mutex::new(())),
            epoch: Rc::new(Cell::new(0)),
        }
    }
    fn url(&self, path: &str) -> Result<String, RelayError> {
        http_url(self.relay_url.as_ref().map_err(Clone::clone)?, path).map_err(Into::into)
    }
    fn storage_key(&self) -> Result<StorageKey, RelayError> {
        let origin = self.relay_url.as_ref().map_err(Clone::clone)?;
        crate::persistence::relay_dpop_cache_key(origin, &scalar_string(&self.client_id)?)
            .map_err(|_| RelayError::InvalidResponse)
    }
    async fn stored_tokens(
        &self,
    ) -> Result<(Option<Vec<u8>>, Vec<RelayAccessTokenCacheEntry>), RelayError> {
        let key = self.storage_key()?;
        let bytes = self
            .platform
            .get(StorageArea::Secure, key.clone())
            .await
            .map_err(|_| RelayError::Storage)?;
        let tokens = match &bytes {
            Some(bytes) => {
                crate::persistence::decode_record::<Vec<RelayAccessTokenCacheEntry>>(bytes, &key, 1)
                    .map_err(|_| RelayError::InvalidResponse)?
                    .payload
            }
            None => vec![],
        };
        Ok((bytes, tokens))
    }
    async fn load_cache(&self) -> Result<(), RelayError> {
        if self.cache.borrow().loaded {
            return Ok(());
        }
        let (_, tokens) = self.stored_tokens().await?;
        let origin = self.relay_url.as_ref().map_err(Clone::clone)?;
        *self.cache.borrow_mut() = RelayCache {
            loaded: true,
            tokens: tokens
                .into_iter()
                .filter(|token| token.client_id == self.client_id && token.relay_url == *origin)
                .collect(),
        };
        Ok(())
    }
    async fn save_cache(&self, tokens: &[RelayAccessTokenCacheEntry]) -> Result<(), RelayError> {
        let key = self.storage_key()?;
        let origin = self.relay_url.as_ref().map_err(Clone::clone)?;
        for _ in 0..3 {
            let (expected, mut existing) = self.stored_tokens().await?;
            existing
                .retain(|entry| entry.client_id != self.client_id || entry.relay_url != *origin);
            existing.extend_from_slice(tokens);
            let bytes =
                crate::persistence::Record::new(&key.namespace, None, &key.record, 1, existing)
                    .encode()
                    .map_err(|_| RelayError::InvalidResponse)?;
            if self
                .platform
                .transaction(crate::platform::StorageTransaction {
                    conditions: vec![crate::platform::StorageCondition {
                        area: StorageArea::Secure,
                        key: key.clone(),
                        expected,
                    }],
                    ordinary: vec![],
                    secure: vec![StorageWrite {
                        key: key.clone(),
                        value: Some(bytes),
                    }],
                })
                .await
                .map_err(|_| RelayError::Storage)?
            {
                return Ok(());
            }
        }
        Err(RelayError::Storage)
    }
    pub fn invalidate_token_cache(&self) {
        self.epoch.set(self.epoch.get().wrapping_add(1));
        self.cache.borrow_mut().tokens.clear();
    }
    pub async fn reset_token_cache(&self) -> Result<(), RelayError> {
        self.invalidate_token_cache();
        let _lock = self.lock.lock().await;
        self.save_cache(&[]).await?;
        *self.cache.borrow_mut() = RelayCache {
            loaded: true,
            tokens: vec![],
        };
        Ok(())
    }
    async fn http<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    ) -> Result<T, RelayError> {
        let response = send_http(
            self.platform.as_ref(),
            request(method, self.url(path)?, headers, body, Credentials::Omit),
            10_000,
        )
        .await?;
        if !(200..300).contains(&response.status) {
            let error: Option<RelayProtectedError> = serde_json::from_slice(&response.body).ok();
            let trace_id = error
                .as_ref()
                .and_then(|error| serde_json::to_value(error).ok())
                .and_then(|value| {
                    value
                        .get("traceId")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .or_else(|| {
                    response
                        .headers
                        .iter()
                        .find(|(key, _)| key.eq_ignore_ascii_case("traceparent"))
                        .and_then(|(_, value)| traceparent_id(value))
                });
            return Err(RelayError::Request {
                status: response.status,
                error: error.map(Sensitive),
                trace_id: trace_id.map(Sensitive),
            });
        }
        serde_json::from_slice(&response.body).map_err(|_| RelayError::InvalidResponse)
    }
    async fn bearer<T: DeserializeOwned, P: Serialize>(
        &self,
        clerk: &str,
        method: &str,
        path: &str,
        payload: Option<&P>,
    ) -> Result<T, RelayError> {
        self.relay_url.as_ref().map_err(Clone::clone)?;
        if clerk.is_empty() {
            return Err(RelayError::SignedOut);
        }
        let mut headers = BTreeMap::from([("authorization".into(), format!("Bearer {clerk}"))]);
        let body = if let Some(payload) = payload {
            headers.insert("content-type".into(), "application/json".into());
            serde_json::to_vec(payload).map_err(|_| RelayError::InvalidResponse)?
        } else {
            vec![]
        };
        self.http(method, path, headers, body).await
    }
    async fn exchange(
        &self,
        clerk: &str,
        scopes: &[RelayDpopAccessTokenScope],
    ) -> Result<RelayDpopAccessTokenResponse, RelayError> {
        let url = self.url("/v1/client/dpop-token")?;
        let proof = self.signer.create_proof("POST", &url, None).await?;
        let names = scopes
            .iter()
            .map(scalar_string)
            .collect::<Result<Vec<_>, _>>()?;
        let payload = RelayDpopAccessTokenRequest {
            grant_type: RelayDpopTokenExchangeGrantType::UrnIetfParamsOauthGrantTypeTokenExchange,
            subject_token: TrimmedNonEmptyString(clerk.to_owned()),
            subject_token_type: RelayJwtSubjectTokenType::UrnIetfParamsOauthTokenTypeJwt,
            requested_token_type: RelayAccessTokenType::UrnIetfParamsOauthTokenTypeAccessToken,
            resource: TrimmedNonEmptyString(self.relay_url.as_ref().map_err(Clone::clone)?.clone()),
            scope: TrimmedNonEmptyString(names.join(" ")),
            client_id: self.client_id,
        };
        let response: RelayDpopAccessTokenResponse = self
            .http(
                "POST",
                "/v1/client/dpop-token",
                BTreeMap::from([
                    ("dpop".into(), proof.into_inner()),
                    (
                        "content-type".into(),
                        "application/x-www-form-urlencoded".into(),
                    ),
                ]),
                form(&payload)?,
            )
            .await?;
        let granted = response
            .scope
            .0
            .as_str()
            .split_whitespace()
            .collect::<BTreeSet<_>>();
        if granted != names.iter().map(String::as_str).collect() {
            return Err(RelayError::UnexpectedScopes);
        }
        Ok(response)
    }
    async fn obtain_token(
        &self,
        clerk: &str,
        scopes: &[RelayDpopAccessTokenScope],
        thumbprint: &str,
    ) -> Result<RelayAccessTokenCacheEntry, RelayError> {
        let relay = self.relay_url.as_ref().map_err(Clone::clone)?;
        let account = relay_account_id(clerk);
        let epoch = self.epoch.get();
        if let Some(account) = &account
            && let Some(token) = self
                .cache
                .borrow()
                .tokens
                .iter()
                .find(|token| {
                    token.matches(
                        account,
                        self.client_id,
                        relay,
                        thumbprint,
                        scopes,
                        self.platform.now_epoch_ms(),
                    )
                })
                .cloned()
        {
            return Ok(token);
        }
        let _lock = self.lock.lock().await;
        if epoch != self.epoch.get() {
            return Err(RelayError::SessionChanged);
        }
        self.load_cache().await?;
        if let Some(account) = &account
            && let Some(token) = self
                .cache
                .borrow()
                .tokens
                .iter()
                .find(|token| {
                    token.matches(
                        account,
                        self.client_id,
                        relay,
                        thumbprint,
                        scopes,
                        self.platform.now_epoch_ms(),
                    )
                })
                .cloned()
        {
            return Ok(token);
        }
        let now = self.platform.now_epoch_ms();
        let response = self.exchange(clerk, scopes).await?;
        if epoch != self.epoch.get() {
            return Err(RelayError::SessionChanged);
        }
        let token = RelayAccessTokenCacheEntry {
            account_id: account.clone().unwrap_or_default(),
            client_id: self.client_id,
            relay_url: relay.clone(),
            thumbprint: thumbprint.into(),
            scopes: scopes.to_vec(),
            access_token: Sensitive(response.access_token.0),
            expires_at_millis: now.saturating_add(response.expires_in.saturating_mul(1000)),
        };
        if account.is_some() {
            let mut tokens: Vec<_> = self
                .cache
                .borrow()
                .tokens
                .iter()
                .filter(|token| token.expires_at_millis.saturating_sub(now) > 5_000)
                .cloned()
                .collect();
            tokens.push(token.clone());
            self.save_cache(&tokens).await?;
            if epoch != self.epoch.get() {
                self.save_cache(&[]).await?;
                return Err(RelayError::SessionChanged);
            }
            self.cache.borrow_mut().tokens = tokens;
        }
        Ok(token)
    }
    async fn invalidate(&self, token: &str) -> Result<(), RelayError> {
        let _lock = self.lock.lock().await;
        let tokens: Vec<_> = self
            .cache
            .borrow()
            .tokens
            .iter()
            .filter(|entry| entry.access_token.expose() != token)
            .cloned()
            .collect();
        if tokens.len() != self.cache.borrow().tokens.len() {
            self.save_cache(&tokens).await?;
            self.cache.borrow_mut().tokens = tokens;
        }
        Ok(())
    }
    async fn dpop<T: DeserializeOwned, P: Serialize>(
        &self,
        clerk: &str,
        scopes: &[RelayDpopAccessTokenScope],
        method: &str,
        path: &str,
        payload: Option<&P>,
    ) -> Result<T, RelayError> {
        let url = self.url(path)?;
        let epoch = self.epoch.get();
        for attempt in 0..2 {
            let thumbprint = self.signer.thumbprint().await?;
            let token = self.obtain_token(clerk, scopes, &thumbprint).await?;
            let proof = self
                .signer
                .create_proof(method, &url, Some(token.access_token.expose()))
                .await?;
            if epoch != self.epoch.get() {
                return Err(RelayError::SessionChanged);
            }
            let mut headers = BTreeMap::from([
                (
                    "authorization".into(),
                    format!("DPoP {}", token.access_token.expose()),
                ),
                ("dpop".into(), proof.into_inner()),
            ]);
            let body = if let Some(payload) = payload {
                headers.insert("content-type".into(), "application/json".into());
                serde_json::to_vec(payload).map_err(|_| RelayError::InvalidResponse)?
            } else {
                vec![]
            };
            let result = self.http(method, path, headers, body).await;
            if epoch != self.epoch.get() {
                return Err(RelayError::SessionChanged);
            }
            if let Err(RelayError::Request {
                error: Some(error), ..
            }) = &result
                && matches!(error.expose(),RelayProtectedError::RelayAuthInvalidError(error) if error.reason==RelayAuthInvalidReason::InvalidBearer)
            {
                self.invalidate(token.access_token.expose()).await?;
                if attempt == 0 {
                    continue;
                }
            }
            return result;
        }
        Err(RelayError::InvalidResponse)
    }
    pub async fn list_environments(
        &self,
        clerk: &str,
    ) -> Result<Vec<RelayClientEnvironmentRecord>, RelayError> {
        Ok(self
            .bearer::<RelayListEnvironmentsResponse, ()>(clerk, "GET", "/v1/environments", None)
            .await?
            .environments)
    }
    pub async fn list_devices(
        &self,
        clerk: &str,
    ) -> Result<Vec<RelayClientDeviceRecord>, RelayError> {
        Ok(self
            .bearer::<RelayListDevicesResponse, ()>(clerk, "GET", "/v2/client/devices", None)
            .await?
            .devices)
    }
    pub async fn create_environment_link_challenge(
        &self,
        clerk: &str,
        payload: &RelayEnvironmentLinkChallengeRequest,
    ) -> Result<Sensitive<RelayEnvironmentLinkChallengeResponse>, RelayError> {
        self.bearer(
            clerk,
            "POST",
            "/v1/client/environment-link-challenges",
            Some(payload),
        )
        .await
        .map(Sensitive)
    }
    pub async fn link_environment(
        &self,
        clerk: &str,
        payload: &RelayEnvironmentLinkRequest,
    ) -> Result<Sensitive<RelayEnvironmentLinkResponse>, RelayError> {
        self.bearer(clerk, "POST", "/v1/client/environment-links", Some(payload))
            .await
            .map(Sensitive)
    }
    pub async fn unlink_environment(
        &self,
        clerk: &str,
        environment: &EnvironmentId,
    ) -> Result<RelayOkResponse, RelayError> {
        self.bearer::<_, ()>(
            clerk,
            "DELETE",
            &format!(
                "/v1/client/environment-links/{}",
                path_segment(&environment.0)
            ),
            None,
        )
        .await
    }
    pub async fn get_t3_chat_link(
        &self,
        clerk: &str,
    ) -> Result<Sensitive<RelayT3ChatLinkResponse>, RelayError> {
        self.bearer::<_, ()>(clerk, "GET", "/v1/client/t3chat-link", None)
            .await
            .map(Sensitive)
    }
    pub async fn create_t3_chat_link_request(
        &self,
        clerk: &str,
    ) -> Result<Sensitive<RelayT3ChatLinkRequestResponse>, RelayError> {
        self.bearer::<_, ()>(clerk, "POST", "/v1/client/t3chat-link-requests", None)
            .await
            .map(Sensitive)
    }
    pub async fn unlink_t3_chat(&self, clerk: &str) -> Result<RelayOkResponse, RelayError> {
        self.bearer::<_, ()>(clerk, "DELETE", "/v1/client/t3chat-link", None)
            .await
    }
    pub async fn get_environment_status(
        &self,
        clerk: &str,
        environment: &EnvironmentId,
    ) -> Result<Sensitive<RelayEnvironmentStatusResponse>, RelayError> {
        let response: RelayEnvironmentStatusResponse = self
            .dpop::<_, ()>(
                clerk,
                &[
                    RelayDpopAccessTokenScope::EnvironmentStatus,
                    RelayDpopAccessTokenScope::EnvironmentConnect,
                ],
                "POST",
                &format!("/v1/environments/{}/status", path_segment(&environment.0)),
                None,
            )
            .await?;
        if response.environment_id != *environment {
            return Err(RelayError::EnvironmentMismatch);
        }
        Ok(Sensitive(response))
    }
    pub async fn connect_environment(
        &self,
        clerk: &str,
        environment: &EnvironmentId,
        device: Option<&str>,
    ) -> Result<Sensitive<RelayEnvironmentConnectResponse>, RelayError> {
        let thumbprint = self.signer.thumbprint().await?;
        let payload = RelayEnvironmentConnectRequest {
            device_id: device.map(|value| Some(TrimmedNonEmptyString(value.to_owned()))),
            client_key_thumbprint: Some(Some(TrimmedNonEmptyString(thumbprint))),
            client_proof_key_thumbprint: None,
        };
        let response: RelayEnvironmentConnectResponse = self
            .dpop(
                clerk,
                &[RelayDpopAccessTokenScope::EnvironmentConnect],
                "POST",
                &format!("/v1/environments/{}/connect", path_segment(&environment.0)),
                Some(&payload),
            )
            .await?;
        if response.environment_id != *environment {
            return Err(RelayError::EnvironmentMismatch);
        }
        Ok(Sensitive(response))
    }
    pub async fn register_device(
        &self,
        clerk: &str,
        payload: &RelayDeviceRegistrationRequest,
    ) -> Result<RelayOkResponse, RelayError> {
        self.dpop(
            clerk,
            &[RelayDpopAccessTokenScope::MobileRegistration],
            "POST",
            "/v1/mobile/devices",
            Some(payload),
        )
        .await
    }
    pub async fn unregister_device(
        &self,
        clerk: &str,
        device: &str,
    ) -> Result<RelayOkResponse, RelayError> {
        self.dpop::<_, ()>(
            clerk,
            &[RelayDpopAccessTokenScope::MobileRegistration],
            "DELETE",
            &format!("/v1/mobile/devices/{}", path_segment(device)),
            None,
        )
        .await
    }
    pub async fn register_live_activity(
        &self,
        clerk: &str,
        payload: &RelayLiveActivityRegistrationRequest,
    ) -> Result<RelayOkResponse, RelayError> {
        self.dpop(
            clerk,
            &[RelayDpopAccessTokenScope::MobileRegistration],
            "POST",
            "/v1/mobile/live-activities",
            Some(payload),
        )
        .await
    }
    pub async fn get_agent_activity_snapshot(
        &self,
        clerk: &str,
    ) -> Result<Sensitive<RelayAgentActivitySnapshotResponse>, RelayError> {
        self.dpop::<_, ()>(
            clerk,
            &[RelayDpopAccessTokenScope::MobileRegistration],
            "GET",
            "/v1/mobile/agent-activity",
            None,
        )
        .await
        .map(Sensitive)
    }
}
fn traceparent_id(value: &str) -> Option<String> {
    let parts = value.split('-').collect::<Vec<_>>();
    if parts.len() != 4
        || parts[0].len() != 2
        || parts[1].len() != 32
        || parts[2].len() != 16
        || parts[3].len() != 2
        || !parts
            .iter()
            .all(|part| part.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        return None;
    }
    Some(parts[1].into())
}
fn path_segment(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

pub const DPOP_CLOCK_HINT: &str =
    "Hint: Check that automatic date and time is enabled on both devices, then try again.";
pub const DPOP_UNKNOWN_HINT: &str = "Hint: Try again. If it still fails, clock skew may be the cause; check that automatic date and time is enabled on both devices.";
pub const DPOP_RETRY_HINT: &str = "Hint: Try again. If the problem continues, copy the trace ID.";
pub const RELAY_GATEWAY_TIMEOUT_MESSAGE: &str = "Relay timed out before it finished the request (HTTP 504). Try again in a few minutes. If it keeps failing, copy the trace ID when you report it.";
pub fn dpop_failure_message(reason: Option<DpopFailureReason>) -> String {
    format!(
        "Relay rejected the DPoP proof. {}",
        match reason {
            Some(DpopFailureReason::TimeWindow) => DPOP_CLOCK_HINT,
            None => DPOP_UNKNOWN_HINT,
            _ => DPOP_RETRY_HINT,
        }
    )
}
pub fn relay_failure_detail(status: u16, error: Option<&RelayProtectedError>) -> String {
    if let Some(error) = error {
        return relay_protected_error_message(error);
    }
    if status == 504 {
        return RELAY_GATEWAY_TIMEOUT_MESSAGE.into();
    }
    format!("Relay returned an unexpected HTTP {status}.")
}
pub fn relay_protected_error_message(error: &RelayProtectedError) -> String {
    use RelayProtectedError::*;
    let reason = |value: &dyn fmt::Debug| format!("{value:?}");
    match error {
        RelayAuthInvalidError(error)=>match error.reason {
            RelayAuthInvalidReason::MissingBearer|RelayAuthInvalidReason::InvalidBearer=>"Relay rejected the cloud session token.".into(),
            RelayAuthInvalidReason::InvalidDpop=>dpop_failure_message(error.dpop_failure_reason),
            RelayAuthInvalidReason::NotAuthorized=>"Relay rejected the authenticated request.".into(),
        },
        RelayEnvironmentLinkProofExpiredError(_)=>"Relay rejected an expired environment link proof.".into(),
        RelayEnvironmentLinkProofInvalidError(error)=>format!("Relay rejected the environment link proof ({}).",scalar_string(&error.reason).unwrap_or_else(|_|reason(&error.reason))),
        RelayEnvironmentConnectNotAuthorizedError(error)=>match error.reason.flatten() {
            Some(RelayEnvironmentConnectNotAuthorizedReason::EnvironmentLinkNotFound)=>"Relay has no active link for this environment. If it was deregistered from T3 Connect, link it again on that machine.".into(),
            Some(RelayEnvironmentConnectNotAuthorizedReason::ManagedEndpointAllocationNotReady)=>"The environment's T3 Connect tunnel is still being set up.".into(),
            Some(RelayEnvironmentConnectNotAuthorizedReason::EndpointProviderNotManaged)=>"This environment is linked only to publish agent activity, so it cannot accept connections. On that computer, open Settings > Sharing & access and turn T3 Connect on.".into(),
            Some(value)=>format!("Relay rejected the environment connection request ({}).",scalar_string(&value).unwrap_or_default()),
            None=>"Relay rejected the environment connection request.".into(),
        },
        RelayEnvironmentEndpointUnavailableError(error)=>format!("Relay could not reach the environment endpoint ({}).",scalar_string(&error.reason).unwrap_or_default()),
        RelayEnvironmentEndpointTimedOutError(_)=>"Relay timed out while contacting the environment endpoint.".into(),
        RelayEnvironmentLinkFailedError(error)=>format!("Relay could not link the environment ({}).",scalar_string(&error.reason).unwrap_or_default()),
        RelayEnvironmentLinkUnavailableError(error)=>format!("Relay cannot provision the managed endpoint ({}).",scalar_string(&error.reason).unwrap_or_default()),
        RelayEnvironmentLinkLimitExceededError(error)=>{
            let (limit,upgrade)=match &error.max_tunnels {t3_contracts::auth::EncodedNumber::Finite(value)=>(value.to_string(),value.as_f64().is_some_and(|value|value<100.0)),t3_contracts::auth::EncodedNumber::NonFinite(value)=>(scalar_string(value).unwrap_or_default(),false)};
            if upgrade {format!("Relay refused the link: this account already has its maximum of {limit} managed tunnels. Unlink an environment, or link a paid T3 Chat account in your T3 Connect account settings on web or desktop. Pro raises the default limit to 10 and Premier to 100.")}
            else {format!("Relay refused the link: this account already has its maximum of {limit} managed tunnels. Unlink an environment to free one up.")}
        },
        RelayAgentActivityPublishProofExpiredError(_)=>"Relay rejected an expired agent activity publish proof.".into(),
        RelayAgentActivityPublishProofInvalidError(error)=>format!("Relay rejected the agent activity publish proof ({}).",scalar_string(&error.reason).unwrap_or_default()),
        RelayInternalError(error)=>format!("Relay encountered an internal error ({}).",scalar_string(&error.reason).unwrap_or_default()),
    }
}
pub fn relay_offline_reason_message(
    status: &RelayEnvironmentStatusResponse,
) -> Option<&'static str> {
    status.offline_reason.flatten().map(|_|"Offline for a while, so its T3 Connect tunnel was removed. Start T3 Code on that computer and update it to the latest version to reconnect.")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayEnvironmentAvailability {
    Checking,
    Online,
    Offline,
    Error,
}
#[derive(Clone)]
pub struct DiscoveredEnvironment {
    pub environment: RelayClientEnvironmentRecord,
    pub availability: RelayEnvironmentAvailability,
    pub status: Option<RelayEnvironmentStatusResponse>,
    pub error: Option<RelayError>,
}
impl fmt::Debug for DiscoveredEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiscoveredEnvironment")
            .field("availability", &self.availability)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Default)]
pub struct RelayDiscoveryState {
    pub environments: BTreeMap<EnvironmentId, DiscoveredEnvironment>,
    pub listed: bool,
    pub refreshing: bool,
    pub offline: bool,
    pub error: Option<RelayError>,
}
impl fmt::Debug for RelayDiscoveryState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RelayDiscoveryState")
            .field("listed", &self.listed)
            .field("refreshing", &self.refreshing)
            .field("offline", &self.offline)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayEnvironmentRegistration {
    Registered,
    Unregistered,
    Unknown,
}
impl RelayDiscoveryState {
    pub fn registration(&self, environment: &EnvironmentId) -> RelayEnvironmentRegistration {
        if self.environments.contains_key(environment) {
            RelayEnvironmentRegistration::Registered
        } else if self.listed {
            RelayEnvironmentRegistration::Unregistered
        } else {
            RelayEnvironmentRegistration::Unknown
        }
    }
}
/// Discovery state whose every write tells the `changes` streams to read it again.
#[derive(Default)]
struct DiscoveryCell {
    state: RefCell<RelayDiscoveryState>,
    watchers: RefCell<Vec<futures::channel::mpsc::Sender<()>>>,
}
impl DiscoveryCell {
    fn borrow(&self) -> std::cell::Ref<'_, RelayDiscoveryState> {
        self.state.borrow()
    }
    fn borrow_mut(&self) -> DiscoveryWrite<'_> {
        DiscoveryWrite {
            state: self.state.borrow_mut(),
            _notify: Notify(&self.watchers),
        }
    }
}
/// Fields drop in order: the state is released before the readers are told.
struct DiscoveryWrite<'a> {
    state: std::cell::RefMut<'a, RelayDiscoveryState>,
    _notify: Notify<'a>,
}
impl std::ops::Deref for DiscoveryWrite<'_> {
    type Target = RelayDiscoveryState;
    fn deref(&self) -> &RelayDiscoveryState {
        &self.state
    }
}
impl std::ops::DerefMut for DiscoveryWrite<'_> {
    fn deref_mut(&mut self) -> &mut RelayDiscoveryState {
        &mut self.state
    }
}
struct Notify<'a>(&'a RefCell<Vec<futures::channel::mpsc::Sender<()>>>);
impl Drop for Notify<'_> {
    fn drop(&mut self) {
        // A full channel already holds a notification; its reader reads the latest state.
        self.0
            .borrow_mut()
            .retain_mut(|watcher| match watcher.try_send(()) {
                Ok(()) => true,
                Err(error) => !error.is_disconnected(),
            });
    }
}
#[derive(Clone)]
pub struct RelayDiscovery {
    client: ManagedRelayClient,
    state: Rc<DiscoveryCell>,
    generation: Rc<Cell<u64>>,
    account: Rc<RefCell<Option<String>>>,
    refresh_lock: Rc<futures::lock::Mutex<()>>,
    has_refreshed: Rc<Cell<bool>>,
}
impl fmt::Debug for RelayDiscovery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RelayDiscovery([REDACTED])")
    }
}
impl RelayDiscovery {
    pub fn new(client: ManagedRelayClient) -> Self {
        Self {
            client,
            state: Rc::default(),
            generation: Rc::default(),
            account: Rc::default(),
            refresh_lock: Rc::new(futures::lock::Mutex::new(())),
            has_refreshed: Rc::default(),
        }
    }
    pub fn snapshot(&self) -> RelayDiscoveryState {
        self.state.borrow().clone()
    }
    /// The current state, then the state after each change. A slow reader skips intermediate
    /// states and reads the latest; dropping the stream unregisters it.
    pub fn changes(&self) -> futures::stream::LocalBoxStream<'static, RelayDiscoveryState> {
        use futures::StreamExt;
        let (sender, receiver) = futures::channel::mpsc::channel(0);
        self.state.watchers.borrow_mut().push(sender);
        let state = self.state.clone();
        futures::stream::once(futures::future::ready(()))
            .chain(receiver)
            .map(move |()| state.borrow().clone())
            .boxed_local()
    }
    pub fn credentials_changed(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        *self.account.borrow_mut() = None;
        *self.state.borrow_mut() = RelayDiscoveryState::default();
    }
    pub fn set_offline(&self, offline: bool) {
        let mut state = self.state.borrow_mut();
        state.offline = offline;
        if offline {
            state.refreshing = false;
        }
    }
    pub fn has_refreshed(&self) -> bool {
        self.has_refreshed.get()
    }
    pub async fn run(&self, platform: Rc<dyn Platform>) {
        use futures::{
            StreamExt,
            future::{Either, select},
            stream::FuturesUnordered,
        };
        let mut events = platform.events();
        let mut active = FuturesUnordered::<LocalBoxFuture<'static, ()>>::new();
        loop {
            let event = if active.is_empty() {
                events.next().await
            } else {
                match select(events.next().boxed_local(), active.next().boxed_local()).await {
                    Either::Left((event, _)) => event,
                    Either::Right(_) => continue,
                }
            };
            let Some(event) = event else {
                return;
            };
            let refresh = match event {
                crate::platform::PlatformEvent::CredentialsChanged => {
                    self.credentials_changed();
                    true
                }
                crate::platform::PlatformEvent::Network(
                    crate::platform::NetworkStatus::Offline,
                ) => {
                    self.set_offline(true);
                    false
                }
                crate::platform::PlatformEvent::Network(_) => {
                    self.set_offline(false);
                    self.has_refreshed()
                }
                _ => false,
            };
            if refresh {
                let generation = self.generation.get();
                let service = self.clone();
                let platform = platform.clone();
                active.push(
                    async move {
                        let token = platform.cloud_token().await;
                        if generation != service.generation.get() {
                            return;
                        }
                        match token {
                            Ok(token) => service.refresh(token.map(Sensitive)).await,
                            Err(_) => {
                                let mut state = service.state.borrow_mut();
                                state.refreshing = false;
                                state.error = Some(RelayError::SignedOut);
                            }
                        }
                    }
                    .boxed_local(),
                );
            }
        }
    }
    pub async fn refresh(&self, clerk: Option<Sensitive<String>>) {
        let generation = self.generation.get();
        let _lock = self.refresh_lock.lock().await;
        if generation != self.generation.get() {
            return;
        }
        self.has_refreshed.set(true);
        if self.state.borrow().offline {
            self.state.borrow_mut().refreshing = false;
            return;
        }
        *self.state.borrow_mut() = RelayDiscoveryState {
            refreshing: true,
            ..Default::default()
        };
        let Some(clerk) = clerk else {
            self.state.borrow_mut().refreshing = false;
            return;
        };
        let account = relay_account_id(clerk.expose());
        if self
            .account
            .borrow()
            .as_ref()
            .is_some_and(|previous| Some(previous) != account.as_ref())
        {
            self.state.borrow_mut().environments.clear();
        }
        *self.account.borrow_mut() = account;
        let result = self.client.list_environments(clerk.expose()).await;
        if generation != self.generation.get() {
            return;
        }
        let environments = match result {
            Ok(values) => values,
            Err(error) => {
                let mut state = self.state.borrow_mut();
                state.error = Some(error);
                state.refreshing = false;
                return;
            }
        };
        {
            let mut state = self.state.borrow_mut();
            state.listed = true;
            for environment in &environments {
                state.environments.insert(
                    environment.environment_id.clone(),
                    DiscoveredEnvironment {
                        environment: environment.clone(),
                        availability: RelayEnvironmentAvailability::Checking,
                        status: None,
                        error: None,
                    },
                );
            }
        }
        use futures::{StreamExt, stream::FuturesUnordered};
        let mut statuses = FuturesUnordered::new();
        for environment in environments {
            let client = self.client.clone();
            let clerk = clerk.clone();
            statuses.push(async move {
                let result = client
                    .get_environment_status(clerk.expose(), &environment.environment_id)
                    .await
                    .and_then(|status| validate_status(&environment, status.into_inner()));
                (environment.environment_id, result)
            });
        }
        while let Some((environment, result)) = statuses.next().await {
            if generation != self.generation.get() {
                return;
            }
            let mut state = self.state.borrow_mut();
            if let Some(entry) = state.environments.get_mut(&environment) {
                match result {
                    Ok(status) => {
                        entry.availability = match status.status {
                            RelayEnvironmentStatusValue::Online => {
                                RelayEnvironmentAvailability::Online
                            }
                            RelayEnvironmentStatusValue::Offline => {
                                RelayEnvironmentAvailability::Offline
                            }
                        };
                        entry.status = Some(status);
                        entry.error = None;
                    }
                    Err(error) => {
                        entry.availability = RelayEnvironmentAvailability::Error;
                        entry.error = Some(error);
                    }
                }
            }
        }
        if generation == self.generation.get() {
            self.state.borrow_mut().refreshing = false;
        }
    }
}
fn validate_status(
    environment: &RelayClientEnvironmentRecord,
    status: RelayEnvironmentStatusResponse,
) -> Result<RelayEnvironmentStatusResponse, RelayError> {
    if status.environment_id != environment.environment_id
        || status.endpoint != environment.endpoint
        || status
            .descriptor
            .as_ref()
            .and_then(Option::as_ref)
            .is_some_and(|descriptor| descriptor.environment_id != environment.environment_id)
    {
        return Err(RelayError::EnvironmentMismatch);
    }
    Ok(status)
}

#[derive(Clone, PartialEq, Eq)]
pub enum CloudAccountStep {
    None,
    SignOut {
        previous_account_id: Option<String>,
        is_account_transition: bool,
    },
    SignIn {
        account_id: String,
        switch_from_account_id: Option<String>,
        is_account_transition: bool,
    },
}
impl fmt::Debug for CloudAccountStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::None => "None",
            Self::SignIn { .. } => "SignIn([REDACTED])",
            Self::SignOut { .. } => "SignOut([REDACTED])",
        })
    }
}
pub fn plan_cloud_account_step(
    previous: Option<Option<&str>>,
    next: Option<&str>,
) -> CloudAccountStep {
    if previous == Some(next) {
        return CloudAccountStep::None;
    }
    let is_account_transition = previous.is_some();
    match next {
        None => CloudAccountStep::SignOut {
            previous_account_id: previous.flatten().map(str::to_owned),
            is_account_transition,
        },
        Some(account) => CloudAccountStep::SignIn {
            account_id: account.into(),
            switch_from_account_id: previous.flatten().map(str::to_owned),
            is_account_transition,
        },
    }
}
#[derive(Clone, PartialEq, Default)]
pub enum CloudAccountState {
    #[default]
    Unobserved,
    SignedOut,
    Activating(String),
    Active(String),
    Failed {
        account_id: String,
        step: CloudAccountStep,
        was_active: bool,
    },
    SignOutFailed {
        previous_account_id: Option<String>,
        was_active: bool,
        error: RelayError,
    },
}
impl fmt::Debug for CloudAccountState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unobserved => "Unobserved",
            Self::SignedOut => "SignedOut",
            Self::Activating(_) => "Activating([REDACTED])",
            Self::Active(_) => "Active([REDACTED])",
            Self::Failed { .. } => "Failed([REDACTED])",
            Self::SignOutFailed { .. } => "SignOutFailed([REDACTED])",
        })
    }
}
pub trait CloudAccountHandlers {
    fn deactivate(&self);
    fn clean_up(
        &self,
        account: Option<String>,
        was_active: bool,
    ) -> LocalBoxFuture<'_, Result<(), RelayError>>;
    fn prepare(
        &self,
        account: String,
        is_current: Rc<dyn Fn() -> bool>,
    ) -> LocalBoxFuture<'_, Result<(), RelayError>>;
    fn activate(&self, account: &str, is_account_transition: bool);
    fn sign_out_transition(&self) {}
    fn detach(&self);
}
struct AccountJob {
    step: CloudAccountStep,
    was_active: bool,
    generation: u64,
    retry_cleanup: bool,
}
#[derive(Clone)]
pub struct CloudAccountService {
    state: Rc<RefCell<CloudAccountState>>,
    generation: Rc<Cell<u64>>,
    jobs: futures::channel::mpsc::UnboundedSender<AccountJob>,
    handlers: Rc<dyn CloudAccountHandlers>,
    attachment: Rc<Cell<u64>>,
    attached: Rc<Cell<bool>>,
    authorization: crate::authorization::RemoteAuthorization,
}
impl fmt::Debug for CloudAccountService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CloudAccountService")
    }
}
impl CloudAccountService {
    pub fn new(
        handlers: Rc<dyn CloudAccountHandlers>,
        authorization: crate::authorization::RemoteAuthorization,
    ) -> (Self, LocalBoxFuture<'static, ()>) {
        let (jobs, mut receiver) = futures::channel::mpsc::unbounded::<AccountJob>();
        let state = Rc::new(RefCell::new(CloudAccountState::Unobserved));
        let generation = Rc::new(Cell::new(0));
        let service = Self {
            state: state.clone(),
            generation: generation.clone(),
            jobs,
            handlers: handlers.clone(),
            attachment: Rc::new(Cell::new(0)),
            attached: Rc::new(Cell::new(false)),
            authorization: authorization.clone(),
        };
        let driver = async move {
            use futures::StreamExt;
            while let Some(job) = receiver.next().await {
                match &job.step {
                    CloudAccountStep::None => {}
                    CloudAccountStep::SignOut {
                        previous_account_id,
                        ..
                    } => {
                        let result = async {
                            authorization
                                .flush_cleanup()
                                .await
                                .map_err(RelayError::from)?;
                            handlers
                                .clean_up(previous_account_id.clone(), job.was_active)
                                .await
                        }
                        .await;
                        if let Err(error) = result
                            && generation.get() == job.generation
                        {
                            *state.borrow_mut() = CloudAccountState::SignOutFailed {
                                previous_account_id: previous_account_id.clone(),
                                was_active: job.was_active,
                                error,
                            };
                        }
                    }
                    CloudAccountStep::SignIn {
                        account_id,
                        switch_from_account_id,
                        is_account_transition,
                    } => {
                        let result = async {
                            authorization
                                .flush_cleanup()
                                .await
                                .map_err(RelayError::from)?;
                            if switch_from_account_id.is_some() || job.retry_cleanup {
                                handlers
                                    .clean_up(switch_from_account_id.clone(), job.was_active)
                                    .await?;
                            }
                            if generation.get() != job.generation {
                                return Ok(());
                            }
                            let current_generation = generation.clone();
                            let target = job.generation;
                            let is_current: Rc<dyn Fn() -> bool> =
                                Rc::new(move || current_generation.get() == target);
                            handlers.prepare(account_id.clone(), is_current).await?;
                            if generation.get() == job.generation {
                                *state.borrow_mut() = CloudAccountState::Active(account_id.clone());
                                handlers.activate(account_id, *is_account_transition);
                            }
                            Ok::<(), RelayError>(())
                        }
                        .await;
                        if result.is_err() && generation.get() == job.generation {
                            *state.borrow_mut() = CloudAccountState::Failed {
                                account_id: account_id.clone(),
                                step: job.step.clone(),
                                was_active: job.was_active,
                            };
                        }
                    }
                }
            }
        }
        .boxed_local();
        (service, driver)
    }
    pub fn attach(&self) -> CloudAccountObserver {
        if self.attached.get() {
            self.detach();
        }
        self.attached.set(true);
        self.attachment.set(self.attachment.get().wrapping_add(1));
        CloudAccountObserver {
            service: self.clone(),
            attachment: self.attachment.get(),
        }
    }
    pub fn state(&self) -> CloudAccountState {
        self.state.borrow().clone()
    }
    pub fn observe(&self, next: Option<Option<String>>) {
        let Some(next) = next else {
            return;
        };
        let previous = self.state();
        let retry_cleanup = matches!(&previous, CloudAccountState::SignOutFailed { .. });
        let step = match &previous {
            CloudAccountState::SignOutFailed {
                previous_account_id,
                ..
            } => match &next {
                None => CloudAccountStep::SignOut {
                    previous_account_id: previous_account_id.clone(),
                    is_account_transition: false,
                },
                Some(account_id) => CloudAccountStep::SignIn {
                    account_id: account_id.clone(),
                    switch_from_account_id: previous_account_id.clone(),
                    is_account_transition: true,
                },
            },
            CloudAccountState::Failed {
                account_id, step, ..
            } if next.as_ref() == Some(account_id) => step.clone(),
            _ => {
                let observed = match &previous {
                    CloudAccountState::Unobserved => None,
                    CloudAccountState::SignedOut => Some(None),
                    CloudAccountState::SignOutFailed {
                        previous_account_id,
                        ..
                    } => Some(previous_account_id.as_deref()),
                    CloudAccountState::Activating(account)
                    | CloudAccountState::Active(account)
                    | CloudAccountState::Failed {
                        account_id: account,
                        ..
                    } => Some(Some(account.as_str())),
                };
                plan_cloud_account_step(observed, next.as_deref())
            }
        };
        if step == CloudAccountStep::None {
            return;
        }
        self.generation.set(self.generation.get().wrapping_add(1));
        let was_active = match previous {
            CloudAccountState::Active(_) => true,
            CloudAccountState::Failed { was_active, .. }
            | CloudAccountState::SignOutFailed { was_active, .. } => was_active,
            _ => false,
        };
        match &step {
            CloudAccountStep::SignOut {
                is_account_transition,
                ..
            } => {
                self.authorization.invalidate_sessions();
                *self.state.borrow_mut() = CloudAccountState::SignedOut;
                if *is_account_transition {
                    self.handlers.sign_out_transition();
                }
                self.handlers.deactivate();
            }
            CloudAccountStep::SignIn {
                account_id,
                switch_from_account_id,
                ..
            } => {
                *self.state.borrow_mut() = CloudAccountState::Activating(account_id.clone());
                if switch_from_account_id.is_some() || retry_cleanup {
                    self.authorization.invalidate_sessions();
                    self.handlers.deactivate();
                }
            }
            CloudAccountStep::None => {}
        }
        let _ = self.jobs.unbounded_send(AccountJob {
            step,
            was_active,
            generation: self.generation.get(),
            retry_cleanup,
        });
    }
    pub fn detach(&self) {
        self.authorization.invalidate_sessions();
        self.attached.set(false);
        self.attachment.set(self.attachment.get().wrapping_add(1));
        self.generation.set(self.generation.get().wrapping_add(1));
        *self.state.borrow_mut() = CloudAccountState::Unobserved;
        self.handlers.detach();
    }
}

pub struct CloudAccountObserver {
    service: CloudAccountService,
    attachment: u64,
}
impl fmt::Debug for CloudAccountObserver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CloudAccountObserver")
    }
}
impl CloudAccountObserver {
    pub fn observe(&self, account: Option<Option<String>>) {
        if self.service.attached.get() && self.service.attachment.get() == self.attachment {
            self.service.observe(account);
        }
    }
}
impl Drop for CloudAccountObserver {
    fn drop(&mut self) {
        if self.service.attached.get() && self.service.attachment.get() == self.attachment {
            self.service.detach();
        }
    }
}

type CloudRead =
    Rc<dyn Fn() -> LocalBoxFuture<'static, Result<Option<Sensitive<String>>, RelayError>>>;
type CloudReadFuture =
    futures::future::Shared<LocalBoxFuture<'static, Result<Option<Sensitive<String>>, RelayError>>>;
struct PendingCloudRead {
    id: u64,
    at: u64,
    future: CloudReadFuture,
}
#[derive(Default)]
struct ManagedSessionState {
    cached: Option<(Sensitive<String>, u64)>,
    pending: Option<PendingCloudRead>,
    generation: u64,
    next_id: u64,
}
#[derive(Clone)]
pub struct ManagedRelaySession {
    account_id: String,
    platform: Rc<dyn Platform>,
    reader: Rc<RefCell<CloudRead>>,
    state: Rc<RefCell<ManagedSessionState>>,
}
impl fmt::Debug for ManagedRelaySession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ManagedRelaySession([REDACTED])")
    }
}
impl ManagedRelaySession {
    pub fn new(account_id: String, platform: Rc<dyn Platform>, reader: CloudRead) -> Self {
        Self {
            account_id,
            platform,
            reader: Rc::new(RefCell::new(reader)),
            state: Rc::default(),
        }
    }
    pub fn account_id(&self) -> &str {
        &self.account_id
    }
    pub fn update_reader(&self, reader: CloudRead) {
        let mut state = self.state.borrow_mut();
        state.generation = state.generation.wrapping_add(1);
        state.pending = None;
        *self.reader.borrow_mut() = reader;
    }
    fn token_read(&self) -> CloudReadFuture {
        let mut state = self.state.borrow_mut();
        if let Some((token, expires)) = &state.cached
            && expires.saturating_sub(self.platform.now_epoch_ms()) > 5_000
        {
            return futures::future::ready(Ok(Some(token.clone())))
                .boxed_local()
                .shared();
        }
        if let Some(pending) = &state.pending
            && self.platform.monotonic_ms().saturating_sub(pending.at) < 4_000
        {
            return pending.future.clone();
        }
        let reader = self.reader.borrow().clone();
        let future = reader();
        let state_ref = Rc::downgrade(&self.state);
        let generation = state.generation;
        state.next_id = state.next_id.wrapping_add(1);
        let id = state.next_id;
        let read = async move {
            let result = future.await;
            let Some(state_ref) = state_ref.upgrade() else {
                return result;
            };
            let mut state = state_ref.borrow_mut();
            if generation == state.generation
                && let Ok(token) = &result
            {
                state.cached = token.as_ref().and_then(|token| {
                    decode_relay_jwt(token.expose())
                        .and_then(|claims| claims.expose().get("exp").and_then(Value::as_u64))
                        .map(|seconds| (token.clone(), seconds.saturating_mul(1000)))
                });
            }
            if state
                .pending
                .as_ref()
                .is_some_and(|pending| pending.id == id)
            {
                state.pending = None;
            }
            result
        }
        .boxed_local()
        .shared();
        state.pending = Some(PendingCloudRead {
            id,
            at: self.platform.monotonic_ms(),
            future: read.clone(),
        });
        read
    }
    pub async fn read_clerk_token(&self) -> Result<Option<Sensitive<String>>, RelayError> {
        use futures::future::{Either, select};
        for _ in 0..3 {
            match select(self.token_read().boxed_local(), self.platform.sleep(4_000)).await {
                Either::Left((result, _)) => return result,
                Either::Right(_) => {}
            }
        }
        Err(RelayError::Timeout)
    }
    pub async fn require_token(&self, account: &str) -> Result<Sensitive<String>, RelayError> {
        if account != self.account_id {
            return Err(RelayError::SessionChanged);
        }
        self.read_clerk_token()
            .await?
            .filter(|value| !value.expose().is_empty())
            .ok_or(RelayError::SignedOut)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RelayQueryKind {
    Environments,
    Devices,
    T3ChatLink,
}
#[derive(Clone)]
pub enum RelayQueryData {
    Environments(Vec<RelayClientEnvironmentRecord>),
    Devices(Vec<RelayClientDeviceRecord>),
    T3ChatLink(RelayT3ChatLinkResponse),
}
impl fmt::Debug for RelayQueryData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RelayQueryData([REDACTED])")
    }
}
#[derive(Debug, Clone, Default)]
pub struct RelayQuerySnapshot {
    pub data: Option<RelayQueryData>,
    pub error: Option<RelayError>,
    pub is_pending: bool,
}
type WeakQueryFuture =
    futures::future::WeakShared<LocalBoxFuture<'static, Result<RelayQueryData, RelayError>>>;
#[derive(Default)]
struct QueryEntry {
    snapshot: RelayQuerySnapshot,
    updated: Option<u64>,
    used: u64,
    pending: Option<(u64, WeakQueryFuture)>,
}
#[derive(Default)]
struct QueryManagerState {
    session: Option<ManagedRelaySession>,
    generation: u64,
    queries: BTreeMap<(String, RelayQueryKind), QueryEntry>,
    next_id: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayQueryStage {
    CloudToken,
    RelayRequest,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayQueryPhase {
    Start,
    Success,
    Failure,
}
#[derive(Debug, Clone)]
pub struct RelayQueryEvent {
    pub operation: RelayQueryKind,
    pub stage: RelayQueryStage,
    pub phase: RelayQueryPhase,
    pub error: Option<RelayError>,
}
type QueryObserver = Rc<dyn Fn(RelayQueryEvent)>;
#[derive(Clone)]
pub struct ManagedRelayQueryManager {
    client: ManagedRelayClient,
    platform: Rc<dyn Platform>,
    state: Rc<RefCell<QueryManagerState>>,
    stale_time: u64,
    idle_ttl: u64,
    observer: Option<QueryObserver>,
}
impl fmt::Debug for ManagedRelayQueryManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ManagedRelayQueryManager([REDACTED])")
    }
}
impl ManagedRelayQueryManager {
    pub fn new(client: ManagedRelayClient, platform: Rc<dyn Platform>) -> Self {
        Self {
            client,
            platform,
            state: Rc::default(),
            stale_time: 15_000,
            idle_ttl: 300_000,
            observer: None,
        }
    }
    pub fn with_observer(mut self, observer: QueryObserver) -> Self {
        self.observer = Some(observer);
        self
    }
    fn emit(
        &self,
        operation: RelayQueryKind,
        stage: RelayQueryStage,
        phase: RelayQueryPhase,
        error: Option<RelayError>,
    ) {
        if let Some(observer) = &self.observer {
            observer(RelayQueryEvent {
                operation,
                stage,
                phase,
                error,
            });
        }
    }
    pub fn set_session(&self, session: Option<ManagedRelaySession>) {
        let mut state = self.state.borrow_mut();
        let old = state.session.as_ref().map(ManagedRelaySession::account_id);
        let new = session.as_ref().map(ManagedRelaySession::account_id);
        if old != new {
            state.generation = state.generation.wrapping_add(1);
            state.queries.clear();
        }
        state.session = session;
    }
    fn current(&self, account: &str, generation: u64) -> bool {
        let state = self.state.borrow();
        state.generation == generation
            && state
                .session
                .as_ref()
                .is_some_and(|session| session.account_id() == account)
    }
    pub fn snapshot(&self, account: &str, kind: RelayQueryKind) -> RelayQuerySnapshot {
        let mut state = self.state.borrow_mut();
        let now = self.platform.monotonic_ms();
        state.queries.retain(|_, entry| {
            if entry
                .pending
                .as_ref()
                .is_some_and(|(_, future)| future.upgrade().is_none())
            {
                entry.pending = None;
                entry.snapshot.is_pending = false;
            }
            entry.pending.is_some() || now.saturating_sub(entry.used) < self.idle_ttl
        });
        if state
            .session
            .as_ref()
            .is_none_or(|session| session.account_id() != account)
        {
            return RelayQuerySnapshot {
                error: Some(RelayError::SessionChanged),
                ..Default::default()
            };
        }
        let entry = state.queries.entry((account.into(), kind)).or_default();
        entry.used = now;
        entry.snapshot.clone()
    }
    pub fn invalidate(&self, account: &str, kind: RelayQueryKind) {
        if let Some(entry) = self
            .state
            .borrow_mut()
            .queries
            .get_mut(&(account.into(), kind))
        {
            entry.updated = None;
        }
    }
    pub async fn load(
        &self,
        account: &str,
        kind: RelayQueryKind,
    ) -> Result<RelayQueryData, RelayError> {
        let future = {
            let mut state = self.state.borrow_mut();
            let generation = state.generation;
            let session = state
                .session
                .as_ref()
                .filter(|session| session.account_id() == account)
                .cloned()
                .ok_or(RelayError::SessionChanged)?;
            let key = (account.to_owned(), kind);
            let now = self.platform.monotonic_ms();
            state.queries.retain(|_, entry| {
                entry.pending.is_some() || now.saturating_sub(entry.used) < self.idle_ttl
            });
            let entry = state.queries.entry(key.clone()).or_default();
            entry.used = now;
            if let Some(data) = &entry.snapshot.data
                && entry
                    .updated
                    .is_some_and(|at| now.saturating_sub(at) < self.stale_time)
            {
                return Ok(data.clone());
            }
            if let Some(future) = entry
                .pending
                .as_ref()
                .and_then(|(_, future)| future.upgrade())
            {
                future
            } else {
                entry.snapshot.is_pending = true;
                entry.snapshot.error = None;
                state.next_id = state.next_id.wrapping_add(1);
                let id = state.next_id;
                let service = self.clone();
                let account = account.to_owned();
                let future = async move {
                    let result = async {
                        service.emit(
                            kind,
                            RelayQueryStage::CloudToken,
                            RelayQueryPhase::Start,
                            None,
                        );
                        let clerk = match session.require_token(&account).await {
                            Ok(token) => {
                                service.emit(
                                    kind,
                                    RelayQueryStage::CloudToken,
                                    RelayQueryPhase::Success,
                                    None,
                                );
                                token
                            }
                            Err(error) => {
                                service.emit(
                                    kind,
                                    RelayQueryStage::CloudToken,
                                    RelayQueryPhase::Failure,
                                    Some(error.clone()),
                                );
                                return Err(error);
                            }
                        };
                        if !service.current(&account, generation) {
                            return Err(RelayError::SessionChanged);
                        }
                        service.emit(
                            kind,
                            RelayQueryStage::RelayRequest,
                            RelayQueryPhase::Start,
                            None,
                        );
                        let listing = async {
                            let data = match kind {
                                RelayQueryKind::Environments => RelayQueryData::Environments(
                                    service.client.list_environments(clerk.expose()).await?,
                                ),
                                RelayQueryKind::Devices => RelayQueryData::Devices(
                                    service.client.list_devices(clerk.expose()).await?,
                                ),
                                RelayQueryKind::T3ChatLink => RelayQueryData::T3ChatLink(
                                    service
                                        .client
                                        .get_t3_chat_link(clerk.expose())
                                        .await?
                                        .into_inner(),
                                ),
                            };
                            Ok::<_, RelayError>(data)
                        }
                        .await;
                        service.emit(
                            kind,
                            RelayQueryStage::RelayRequest,
                            if listing.is_ok() {
                                RelayQueryPhase::Success
                            } else {
                                RelayQueryPhase::Failure
                            },
                            listing.as_ref().err().cloned(),
                        );
                        if !service.current(&account, generation) {
                            return Err(RelayError::SessionChanged);
                        }
                        listing
                    }
                    .await;
                    if service.current(&account, generation)
                        && let Some(entry) =
                            service.state.borrow_mut().queries.get_mut(&(account, kind))
                        && entry
                            .pending
                            .as_ref()
                            .is_some_and(|(pending, _)| *pending == id)
                    {
                        entry.pending = None;
                        entry.snapshot.is_pending = false;
                        match &result {
                            Ok(data) => {
                                entry.snapshot.data = Some(data.clone());
                                entry.snapshot.error = None;
                                entry.updated = Some(service.platform.monotonic_ms());
                            }
                            Err(error) => {
                                entry.snapshot.error = Some(error.clone());
                                entry.updated = None;
                            }
                        }
                    }
                    result
                }
                .boxed_local()
                .shared();
                state.queries.entry(key).or_default().pending =
                    Some((id, future.downgrade().ok_or(RelayError::SessionChanged)?));
                future
            }
        };
        future.await
    }
    pub async fn create_t3_chat_link_request(
        &self,
        account: &str,
    ) -> Result<Sensitive<RelayT3ChatLinkRequestResponse>, RelayError> {
        let (session, generation) = {
            let state = self.state.borrow();
            (
                state
                    .session
                    .as_ref()
                    .filter(|session| session.account_id() == account)
                    .cloned()
                    .ok_or(RelayError::SessionChanged)?,
                state.generation,
            )
        };
        let token = session.require_token(account).await?;
        if !self.current(account, generation) {
            return Err(RelayError::SessionChanged);
        }
        let result = self
            .client
            .create_t3_chat_link_request(token.expose())
            .await?;
        if !self.current(account, generation) {
            return Err(RelayError::SessionChanged);
        }
        Ok(result)
    }
    pub async fn unlink_t3_chat(&self, account: &str) -> Result<RelayOkResponse, RelayError> {
        let (session, generation) = {
            let state = self.state.borrow();
            (
                state
                    .session
                    .as_ref()
                    .filter(|session| session.account_id() == account)
                    .cloned()
                    .ok_or(RelayError::SessionChanged)?,
                state.generation,
            )
        };
        let token = session.require_token(account).await?;
        if !self.current(account, generation) {
            return Err(RelayError::SessionChanged);
        }
        let result = self.client.unlink_t3_chat(token.expose()).await?;
        if !self.current(account, generation) {
            return Err(RelayError::SessionChanged);
        }
        self.invalidate(account, RelayQueryKind::T3ChatLink);
        Ok(result)
    }
    pub async fn deregister_environment(
        &self,
        account: &str,
        environment: &EnvironmentId,
    ) -> Result<RelayOkResponse, RelayError> {
        let (session, generation) = {
            let state = self.state.borrow();
            (
                state
                    .session
                    .as_ref()
                    .filter(|session| session.account_id() == account)
                    .cloned()
                    .ok_or(RelayError::SessionChanged)?,
                state.generation,
            )
        };
        let token = session.require_token(account).await?;
        if !self.current(account, generation) {
            return Err(RelayError::SessionChanged);
        }
        let result = self
            .client
            .unlink_environment(token.expose(), environment)
            .await?;
        if !self.current(account, generation) {
            return Err(RelayError::SessionChanged);
        }
        self.invalidate(account, RelayQueryKind::Environments);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        authorization::tests::{TestPlatform, TestSigner, descriptor, endpoint, jwt, relay_access},
        platform::{HostReply, HttpResponse, PlatformError, Storage},
        testing::FakePlatform,
    };
    use futures::{FutureExt, executor::block_on};
    fn client(platform: Rc<TestPlatform>) -> ManagedRelayClient {
        ManagedRelayClient::new(
            platform,
            Rc::new(TestSigner::default()),
            "https://relay.test",
            RelayPublicClientId::T3Web,
        )
    }
    fn relay_error(reason: &str) -> Value {
        json!({"_tag":"RelayAuthInvalidError","code":"auth_invalid","reason":reason,"traceId":"MARKER_SECRET_TRACE"})
    }
    fn environment() -> RelayClientEnvironmentRecord {
        serde_json::from_value(json!({"environmentId":"env","label":"Host","endpoint":endpoint(),"linkedAt":"2036-01-01T00:00:00.000Z"})).unwrap()
    }
    fn status() -> Value {
        json!({"environmentId":"env","endpoint":endpoint(),"status":"online","checkedAt":"now"})
    }
    #[test]
    fn relay_origins_are_https_canonical_origins_before_credentials_are_used() {
        for url in [
            "http://relay.test",
            "https://user:MARKER_SECRET@relay.test",
            "https://relay.test/path",
            "https://relay.test?secret=MARKER_SECRET",
            "https://relay.test/#MARKER_SECRET",
            "garbage",
            "file:///local",
        ] {
            assert_eq!(normalize_secure_relay_url(url), Err(RelayError::InvalidUrl));
            let platform = Rc::new(TestPlatform::default());
            let client = ManagedRelayClient::new(
                platform.clone(),
                Rc::new(TestSigner::default()),
                url,
                RelayPublicClientId::T3Web,
            );
            assert_eq!(
                block_on(client.list_environments("MARKER_SECRET")).unwrap_err(),
                RelayError::InvalidUrl
            );
            assert!(platform.requests.borrow().is_empty());
        }
        assert_eq!(
            normalize_secure_relay_url(" https://RELAY.test:443/// ").unwrap(),
            "https://relay.test"
        );
    }
    #[test]
    fn proof_header_claims_thumbprint_ath_and_signature_are_rfc9449_compatible() {
        block_on(async {
            let platform = Rc::new(FakePlatform::default());
            platform.advance(123456);
            let jwk = json!({"kty":"EC","crv":"P-256","x":URL_SAFE_NO_PAD.encode([1;32]),"y":URL_SAFE_NO_PAD.encode([2;32]),"d":"MARKER_PRIVATE_KEY"});
            let signer = PlatformDpopSigner::new(platform.clone(), jwk).unwrap();
            platform.push_host(Ok(HostReply::Text(Some(signer.thumbprint.clone()))));
            platform.push_host(Ok(HostReply::Bytes(vec![3; 64])));
            let proof = signer
                .create_proof(
                    "post",
                    "https://EXAMPLE.test:443/path?q=MARKER_QUERY#fragment",
                    Some("MARKER_TOKEN"),
                )
                .await
                .unwrap();
            let parts = proof.expose().split('.').collect::<Vec<_>>();
            assert_eq!(parts.len(), 3);
            let header: Value =
                serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
            let payload: Value =
                serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
            assert_eq!(header["typ"], "dpop+jwt");
            assert_eq!(header["alg"], "ES256");
            assert!(header["jwk"].get("d").is_none());
            assert_eq!(payload["htu"], "https://example.test/path");
            assert_eq!(payload["htm"], "POST");
            assert_eq!(payload["iat"], 123);
            assert_eq!(
                payload["ath"],
                URL_SAFE_NO_PAD.encode(Sha256::digest(b"MARKER_TOKEN"))
            );
            assert_eq!(payload["jti"].as_str().unwrap().len(), 36);
            assert_eq!(URL_SAFE_NO_PAD.decode(parts[2]).unwrap(), vec![3; 64]);
            assert!(!proof.expose().contains("MARKER"));
            assert!(!format!("{proof:?} {signer:?}").contains("MARKER"));
        });
    }
    #[test]
    fn proof_invalid_url_key_mismatch_and_non_raw_signatures_fail_without_leaking() {
        block_on(async {
            let platform = Rc::new(FakePlatform::default());
            let signer=PlatformDpopSigner::new(platform.clone(),json!({"kty":"EC","crv":"P-256","x":URL_SAFE_NO_PAD.encode([1;32]),"y":URL_SAFE_NO_PAD.encode([2;32])})).unwrap();
            assert_eq!(
                signer
                    .create_proof("GET", "MARKER_SECRET", None)
                    .await
                    .unwrap_err(),
                RelayError::Proof
            );
            platform.push_host(Ok(HostReply::Text(Some("different-key".into()))));
            assert_eq!(signer.thumbprint().await.unwrap_err(), RelayError::Key);
            platform.push_host(Ok(HostReply::Text(Some(signer.thumbprint.clone()))));
            platform.push_host(Ok(HostReply::Bytes(vec![1; 70])));
            assert_eq!(
                signer
                    .create_proof("GET", "https://relay.test/", None)
                    .await
                    .unwrap_err(),
                RelayError::Proof
            );
            platform.push_host(Err(PlatformError::Network("MARKER_SECRET".into())));
            let error = signer.thumbprint().await.unwrap_err();
            assert!(!format!("{error:?} {error}").contains("MARKER_SECRET"));
        });
    }
    #[test]
    fn cache_reuses_persisted_token_across_clerk_rotation_and_client_instances() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let first = client(platform.clone());
            let mut access = relay_access();
            access["scope"] = json!("environment:status environment:connect");
            platform.respond(200, access);
            platform.respond(200, status());
            first
                .get_environment_status(&jwt("account"), &EnvironmentId("env".into()))
                .await
                .unwrap();
            let second = client(platform.clone());
            platform.respond(200, status());
            second
                .get_environment_status(
                    &format!("{}.other", jwt("account").rsplit_once('.').unwrap().0),
                    &EnvironmentId("env".into()),
                )
                .await
                .unwrap();
            assert_eq!(platform.requests.borrow().len(), 3);
            assert!(platform.requests.borrow()[0].url.ends_with("/dpop-token"));
        });
    }
    #[test]
    fn requested_scope_set_must_match_the_exchange_response() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let client = client(platform.clone());
            platform.respond(200, relay_access());
            let result = client
                .get_environment_status(&jwt("account"), &EnvironmentId("env".into()))
                .await;
            assert_eq!(result.unwrap_err(), RelayError::UnexpectedScopes);
            assert_eq!(platform.requests.borrow().len(), 1);
        });
    }
    #[test]
    fn rejected_access_token_refreshes_once_and_final_rejection_evicts_it() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let client = client(platform.clone());
            for _ in 0..2 {
                platform.respond(200, relay_access());
                platform.respond(401, relay_error("invalid_bearer"));
            }
            let error = client
                .connect_environment(&jwt("account"), &EnvironmentId("env".into()), None)
                .await
                .unwrap_err();
            assert!(matches!(error, RelayError::Request { status: 401, .. }));
            assert_eq!(platform.requests.borrow().len(), 4);
            assert!(client.cache.borrow().tokens.is_empty());
            assert!(!format!("{error:?} {error}").contains("MARKER_SECRET"));
        });
    }
    #[test]
    fn invalid_subject_bypasses_persistent_cache_and_scope_supersets_are_reused() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let client = client(platform.clone());
            for _ in 0..2 {
                platform.respond(200, relay_access());
                platform.respond(200,json!({"environmentId":"env","endpoint":endpoint(),"credential":"bootstrap","expiresAt":"later"}));
                client
                    .connect_environment("bad-jwt", &EnvironmentId("env".into()), None)
                    .await
                    .unwrap();
            }
            assert!(client.cache.borrow().tokens.is_empty());
            assert!(
                platform
                    .list(
                        StorageArea::Secure,
                        crate::persistence::RELAY_DPOP_TOKEN_KIND.into()
                    )
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(platform.requests.borrow().len(), 4);
            let mut access = relay_access();
            access["scope"] = json!("environment:status environment:connect");
            platform.respond(200, access);
            platform.respond(200, status());
            client
                .get_environment_status(&jwt("account"), &EnvironmentId("env".into()))
                .await
                .unwrap();
            platform.respond(200,json!({"environmentId":"env","endpoint":endpoint(),"credential":"bootstrap","expiresAt":"later"}));
            client
                .connect_environment(&jwt("account"), &EnvironmentId("env".into()), None)
                .await
                .unwrap();
            assert_eq!(platform.requests.borrow().len(), 7);
        });
    }
    #[test]
    fn cache_hit_is_not_blocked_behind_another_scope_exchange() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let client = client(platform.clone());
            let mut access = relay_access();
            access["scope"] = json!("environment:status environment:connect");
            platform.respond(200, access);
            platform.respond(200, status());
            client
                .get_environment_status(&jwt("account"), &EnvironmentId("env".into()))
                .await
                .unwrap();
            let release = platform.pending();
            let clerk = jwt("account");
            let mut exchange = client
                .obtain_token(
                    &clerk,
                    &[RelayDpopAccessTokenScope::MobileRegistration],
                    "key",
                )
                .boxed_local();
            assert!(exchange.as_mut().now_or_never().is_none());
            platform.respond(200, status());
            client
                .get_environment_status(&clerk, &EnvironmentId("env".into()))
                .await
                .unwrap();
            let mut access = relay_access();
            access["scope"] = json!("mobile:registration");
            release
                .send(HttpResponse {
                    status: 200,
                    headers: BTreeMap::new(),
                    body: serde_json::to_vec(&access).unwrap(),
                })
                .unwrap();
            exchange.await.unwrap();
        });
    }
    #[test]
    fn relay_errors_keep_safe_hints_and_trace_without_rendering_untrusted_fields() {
        for (reason, hint) in [
            (None, DPOP_UNKNOWN_HINT),
            (Some("time_window"), DPOP_CLOCK_HINT),
            (Some("key_mismatch"), DPOP_RETRY_HINT),
        ] {
            let mut value = relay_error("invalid_dpop");
            if let Some(reason) = reason {
                value["dpopFailureReason"] = json!(reason);
            }
            let error: RelayProtectedError = serde_json::from_value(value).unwrap();
            assert!(relay_protected_error_message(&error).contains(hint));
        }
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let client = client(platform.clone());
            platform.respond(504, json!({"token":"MARKER_SECRET"}));
            let payload:RelayEnvironmentLinkChallengeRequest=serde_json::from_value(json!({"notificationsEnabled":true,"liveActivitiesEnabled":true,"managedTunnelsEnabled":true})).unwrap();
            let error = client
                .create_environment_link_challenge("MARKER_SECRET", &payload)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("HTTP 504"));
            assert!(!format!("{error:?}").contains("MARKER_SECRET"));
        });
    }
    #[test]
    fn discovery_registration_and_status_identity_checks_fail_closed() {
        let mut state = RelayDiscoveryState::default();
        let env = EnvironmentId("env".into());
        assert_eq!(
            state.registration(&env),
            RelayEnvironmentRegistration::Unknown
        );
        state.listed = true;
        assert_eq!(
            state.registration(&env),
            RelayEnvironmentRegistration::Unregistered
        );
        let environment = environment();
        let status: RelayEnvironmentStatusResponse = serde_json::from_value(status()).unwrap();
        validate_status(&environment, status.clone()).unwrap();
        let mut changed = status.clone();
        changed.environment_id = EnvironmentId("other".into());
        assert_eq!(
            validate_status(&environment, changed).unwrap_err(),
            RelayError::EnvironmentMismatch
        );
        let mut changed = status.clone();
        changed.endpoint.http_base_url = TrimmedNonEmptyString("https://other.test".into());
        assert_eq!(
            validate_status(&environment, changed).unwrap_err(),
            RelayError::EnvironmentMismatch
        );
        let mut changed = status;
        changed.descriptor = Some(Some(serde_json::from_value(descriptor("other")).unwrap()));
        assert_eq!(
            validate_status(&environment, changed).unwrap_err(),
            RelayError::EnvironmentMismatch
        );
    }
    #[test]
    fn discovery_changes_replay_the_current_state_then_each_transition() {
        use futures::StreamExt;
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let discovery = RelayDiscovery::new(client(platform.clone()));
            discovery.set_offline(true);
            let mut changes = discovery.changes();
            assert!(changes.next().await.unwrap().offline);
            assert!(changes.next().now_or_never().is_none());
            discovery.set_offline(false);
            discovery.credentials_changed();
            let latest = changes.next().await.unwrap();
            assert!(!latest.offline && !latest.listed);
            assert!(changes.next().now_or_never().is_none(), "writes coalesce");
            drop(changes);
            discovery.set_offline(true);
            assert!(discovery.state.watchers.borrow().is_empty());
        });
    }
    #[test]
    fn discovery_publishes_status_then_clears_rows_on_failed_refresh_or_signout() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let discovery = RelayDiscovery::new(client(platform.clone()));
            platform.respond(200, json!({"environments":[environment()]}));
            let mut access = relay_access();
            access["scope"] = json!("environment:status environment:connect");
            platform.respond(200, access);
            platform.respond(200, status());
            discovery.refresh(Some(Sensitive(jwt("account")))).await;
            let state = discovery.snapshot();
            assert!(state.listed);
            assert!(!state.refreshing);
            assert_eq!(
                state.environments[&EnvironmentId("env".into())].availability,
                RelayEnvironmentAvailability::Online
            );
            platform.respond(500, json!({"message":"MARKER_SECRET"}));
            discovery.refresh(Some(Sensitive(jwt("account")))).await;
            assert!(discovery.snapshot().environments.is_empty());
            assert!(discovery.snapshot().error.is_some());
            discovery.credentials_changed();
            discovery.refresh(None).await;
            let state = discovery.snapshot();
            assert!(state.environments.is_empty());
            assert!(!state.listed);
            assert!(state.error.is_none());
        });
    }
    #[test]
    fn discovery_ignores_a_list_that_finishes_after_credentials_change() {
        let platform = Rc::new(TestPlatform::default());
        let discovery = RelayDiscovery::new(client(platform.clone()));
        let release = platform.pending();
        let mut refresh = discovery
            .refresh(Some(Sensitive(jwt("account"))))
            .boxed_local();
        assert!(refresh.as_mut().now_or_never().is_none());
        discovery.credentials_changed();
        release
            .send(HttpResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: serde_json::to_vec(&json!({"environments":[environment()]})).unwrap(),
            })
            .unwrap();
        block_on(refresh);
        assert!(discovery.snapshot().environments.is_empty());
        assert!(!discovery.snapshot().listed);
    }
    #[test]
    fn account_steps_distinguish_first_emission_repeats_switch_and_signout() {
        assert_eq!(
            plan_cloud_account_step(None, None),
            CloudAccountStep::SignOut {
                previous_account_id: None,
                is_account_transition: false
            }
        );
        assert_eq!(
            plan_cloud_account_step(Some(Some("a")), Some("a")),
            CloudAccountStep::None
        );
        assert_eq!(
            plan_cloud_account_step(Some(Some("a")), Some("b")),
            CloudAccountStep::SignIn {
                account_id: "b".into(),
                switch_from_account_id: Some("a".into()),
                is_account_transition: true
            }
        );
    }
    #[derive(Default)]
    struct AccountHandlers {
        events: RefCell<Vec<String>>,
        fail: Cell<bool>,
        cleanup_fail: Cell<bool>,
        gate: RefCell<Option<futures::channel::oneshot::Receiver<()>>>,
    }
    impl CloudAccountHandlers for AccountHandlers {
        fn deactivate(&self) {
            self.events.borrow_mut().push("deactivate".into());
        }
        fn clean_up(
            &self,
            account: Option<String>,
            was_active: bool,
        ) -> LocalBoxFuture<'_, Result<(), RelayError>> {
            self.events
                .borrow_mut()
                .push(format!("cleanup:{account:?}:{was_active}"));
            async move {
                if self.cleanup_fail.get() {
                    Err(RelayError::Storage)
                } else {
                    Ok(())
                }
            }
            .boxed_local()
        }
        fn prepare(
            &self,
            account: String,
            _is_current: Rc<dyn Fn() -> bool>,
        ) -> LocalBoxFuture<'_, Result<(), RelayError>> {
            async move {
                self.events.borrow_mut().push(format!("prepare:{account}"));
                let gate = self.gate.borrow_mut().take();
                if let Some(gate) = gate {
                    let _ = gate.await;
                }
                if self.fail.replace(false) {
                    Err(RelayError::Network)
                } else {
                    Ok(())
                }
            }
            .boxed_local()
        }
        fn activate(&self, account: &str, _transition: bool) {
            self.events.borrow_mut().push(format!("activate:{account}"));
        }
        fn detach(&self) {
            self.events.borrow_mut().push("detach".into());
        }
    }
    #[test]
    fn account_switch_deactivates_synchronously_serializes_cleanup_and_retries_failure() {
        let handlers = Rc::new(AccountHandlers::default());
        let (service, mut driver) = CloudAccountService::new(
            handlers.clone(),
            crate::authorization::tests::service(Rc::default(), Rc::default()),
        );
        service.observe(None);
        assert_eq!(service.state(), CloudAccountState::Unobserved);
        service.observe(Some(Some("a".into())));
        assert!(driver.as_mut().now_or_never().is_none());
        assert_eq!(service.state(), CloudAccountState::Active("a".into()));
        service.observe(Some(Some("a".into())));
        assert_eq!(handlers.events.borrow().len(), 2);
        service.observe(Some(Some("b".into())));
        assert_eq!(handlers.events.borrow().last().unwrap(), "deactivate");
        handlers.fail.set(true);
        assert!(driver.as_mut().now_or_never().is_none());
        assert!(matches!(service.state(), CloudAccountState::Failed { .. }));
        service.observe(Some(Some("b".into())));
        assert!(driver.as_mut().now_or_never().is_none());
        assert_eq!(service.state(), CloudAccountState::Active("b".into()));
        let events = handlers.events.borrow();
        let cleanup = events
            .iter()
            .position(|event| event == "cleanup:Some(\"a\"):true")
            .unwrap();
        let activation = events
            .iter()
            .position(|event| event == "activate:b")
            .unwrap();
        assert!(cleanup < activation);
    }
    #[test]
    fn signout_cleanup_failure_is_visible_and_retryable() {
        for next in [None, Some("account".to_owned())] {
            let handlers = Rc::new(AccountHandlers::default());
            let (service, mut driver) = CloudAccountService::new(
                handlers.clone(),
                crate::authorization::tests::service(Rc::default(), Rc::default()),
            );
            service.observe(Some(Some("account".into())));
            assert!(driver.as_mut().now_or_never().is_none());
            handlers.cleanup_fail.set(true);
            service.observe(Some(None));
            assert!(driver.as_mut().now_or_never().is_none());
            assert_ne!(service.state(), CloudAccountState::SignedOut);
            handlers.cleanup_fail.set(false);
            service.observe(Some(next.clone()));
            assert!(driver.as_mut().now_or_never().is_none());
            assert_eq!(
                service.state(),
                next.map_or(CloudAccountState::SignedOut, CloudAccountState::Active)
            );
        }
    }
    #[test]
    fn account_signout_or_detach_prevents_a_pending_activation() {
        for detach in [false, true] {
            let handlers = Rc::new(AccountHandlers::default());
            let (tx, rx) = futures::channel::oneshot::channel();
            handlers.gate.replace(Some(rx));
            let (service, mut driver) = CloudAccountService::new(
                handlers.clone(),
                crate::authorization::tests::service(Rc::default(), Rc::default()),
            );
            service.observe(Some(Some("a".into())));
            assert!(driver.as_mut().now_or_never().is_none());
            if detach {
                service.detach();
            } else {
                service.observe(Some(None));
            }
            tx.send(()).unwrap();
            assert!(driver.as_mut().now_or_never().is_none());
            assert!(
                !handlers
                    .events
                    .borrow()
                    .iter()
                    .any(|event| event == "activate:a")
            );
        }
    }
    #[test]
    fn managed_session_deduplicates_reads_caches_until_skew_and_updates_same_account_reader() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let count = Rc::new(Cell::new(0));
            let token = jwt("account");
            let count_read = count.clone();
            let token_read = token.clone();
            let reader: CloudRead = Rc::new(move || {
                count_read.set(count_read.get() + 1);
                futures::future::ready(Ok(Some(Sensitive(token_read.clone())))).boxed_local()
            });
            let session = ManagedRelaySession::new("account".into(), platform.clone(), reader);
            session.require_token("account").await.unwrap();
            session.require_token("account").await.unwrap();
            assert_eq!(count.get(), 1);
            platform.fake.advance(3_596_000);
            session.require_token("account").await.unwrap();
            assert_eq!(count.get(), 2);
            let count_read = count.clone();
            session.update_reader(Rc::new(move || {
                count_read.set(count_read.get() + 1);
                futures::future::ready(Ok(None)).boxed_local()
            }));
            assert_eq!(
                session.require_token("other").await.unwrap_err(),
                RelayError::SessionChanged
            );
            assert_eq!(
                session.require_token("account").await.unwrap_err(),
                RelayError::SignedOut
            );
            assert_eq!(count.get(), 3);
        });
    }
    #[test]
    fn managed_session_slow_read_replacement_and_three_timeouts_use_injected_time() {
        let platform = Rc::new(TestPlatform::default());
        let count = Rc::new(Cell::new(0));
        let count_read = count.clone();
        let session = ManagedRelaySession::new(
            "account".into(),
            platform.clone(),
            Rc::new(move || {
                count_read.set(count_read.get() + 1);
                futures::future::pending().boxed_local()
            }),
        );
        let mut read = session.read_clerk_token().boxed_local();
        assert!(read.as_mut().now_or_never().is_none());
        assert_eq!(count.get(), 1);
        for expected in [2, 3] {
            platform.fake.advance(4000);
            assert!(read.as_mut().now_or_never().is_none());
            assert_eq!(count.get(), expected);
        }
        platform.fake.advance(4000);
        assert_eq!(block_on(read).unwrap_err(), RelayError::Timeout);
    }
    #[test]
    fn managed_query_deduplicates_requests_and_denies_actions_after_account_switch() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let manager = ManagedRelayQueryManager::new(client(platform.clone()), platform.clone());
            let clerk = jwt("account");
            manager.set_session(Some(ManagedRelaySession::new(
                "account".into(),
                platform.clone(),
                Rc::new(move || {
                    futures::future::ready(Ok(Some(Sensitive(clerk.clone())))).boxed_local()
                }),
            )));
            let release = platform.pending();
            let mut first = manager
                .load("account", RelayQueryKind::Devices)
                .boxed_local();
            assert!(first.as_mut().now_or_never().is_none());
            let mut second = manager
                .load("account", RelayQueryKind::Devices)
                .boxed_local();
            assert!(second.as_mut().now_or_never().is_none());
            assert_eq!(platform.requests.borrow().len(), 1);
            release
                .send(HttpResponse {
                    status: 200,
                    headers: BTreeMap::new(),
                    body: br#"{"devices":[]}"#.to_vec(),
                })
                .unwrap();
            first.await.unwrap();
            second.await.unwrap();
            manager
                .load("account", RelayQueryKind::Devices)
                .await
                .unwrap();
            assert_eq!(platform.requests.borrow().len(), 1);
            manager.set_session(None);
            assert_eq!(
                manager
                    .deregister_environment("account", &EnvironmentId("env".into()))
                    .await
                    .unwrap_err(),
                RelayError::SessionChanged
            );
            assert!(
                manager
                    .snapshot("account", RelayQueryKind::Devices)
                    .data
                    .is_none()
            );
        });
    }
    #[test]
    fn public_relay_types_after_loading_marker_secrets_have_redacted_debug() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let client = client(platform.clone());
            let entry = RelayAccessTokenCacheEntry {
                account_id: "MARKER_SECRET".into(),
                client_id: RelayPublicClientId::T3Web,
                relay_url: "https://MARKER_SECRET.test".into(),
                thumbprint: "MARKER_SECRET".into(),
                scopes: vec![],
                access_token: Sensitive("MARKER_SECRET".into()),
                expires_at_millis: 1,
            };
            let loaded: RelayAccessTokenCacheEntry =
                serde_json::from_slice(&serde_json::to_vec(&entry).unwrap()).unwrap();
            assert!(!format!("{loaded:?} {client:?}").contains("MARKER_SECRET"));
            let endpoint = RelayEndpoint {
                environment: EnvironmentId("env".into()),
                url: "https://host/?token=MARKER_SECRET".into(),
                direct_candidates: vec!["MARKER_SECRET".into()],
            };
            assert!(
                !format!(
                    "{endpoint:?} {:?}",
                    RelayStatus::Unavailable("MARKER_SECRET".into())
                )
                .contains("MARKER_SECRET")
            );
            let error = RelayError::Request {
                status: 401,
                error: Some(Sensitive(
                    serde_json::from_value(relay_error("invalid_dpop")).unwrap(),
                )),
                trace_id: Some(Sensitive("MARKER_SECRET".into())),
            };
            assert!(!format!("{error:?} {error}").contains("MARKER_SECRET"));
        });
    }
}

#[cfg(test)]
mod es256_test {
    use super::*;
    use crate::{authorization::tests::TestPlatform, platform::PlatformError};
    use p256::ecdsa::{
        Signature, SigningKey,
        signature::{Signer, Verifier},
    };
    #[test]
    fn generated_proof_passes_a_real_p256_verifier() {
        futures::executor::block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let key = SigningKey::from_slice(&[7; 32]).unwrap();
            let verifier = *key.verifying_key();
            let point = verifier.to_encoded_point(false);
            let jwk = json!({"crv":"P-256","kty":"EC","x":URL_SAFE_NO_PAD.encode(point.x().unwrap()),"y":URL_SAFE_NO_PAD.encode(point.y().unwrap())});
            let thumbprint =
                URL_SAFE_NO_PAD.encode(Sha256::digest(serde_json::to_vec(&jwk).unwrap()));
            let host_jwk = jwk.clone();
            platform
                .host_handler
                .replace(Some(Rc::new(move |action| match action {
                    HostAction::DpopPublicJwk => Ok(HostReply::Document(host_jwk.clone())),
                    HostAction::DpopThumbprint => Ok(HostReply::Text(Some(thumbprint.clone()))),
                    HostAction::DpopSign { signing_input } => {
                        let signature: Signature = key.sign(&signing_input);
                        Ok(HostReply::Bytes(signature.to_bytes().to_vec()))
                    }
                    _ => Err(PlatformError::Unsupported("test action".into())),
                })));
            let signer = PlatformDpopSigner::from_host(platform).await.unwrap();
            let proof = signer
                .create_proof(
                    "post",
                    "https://relay.test/path?ignored=true#ignored",
                    Some("MARKER_ACCESS_TOKEN"),
                )
                .await
                .unwrap();
            let (input, signature) = proof.expose().rsplit_once('.').unwrap();
            let signature =
                Signature::from_slice(&URL_SAFE_NO_PAD.decode(signature).unwrap()).unwrap();
            verifier.verify(input.as_bytes(), &signature).unwrap();
            let claims: Value = serde_json::from_slice(
                &URL_SAFE_NO_PAD
                    .decode(input.split('.').nth(1).unwrap())
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                claims["ath"],
                URL_SAFE_NO_PAD.encode(Sha256::digest(b"MARKER_ACCESS_TOKEN"))
            );
            assert_eq!(claims["htu"], "https://relay.test/path");
        });
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use crate::authorization::tests::{TestPlatform, TestSigner, jwt};
    #[test]
    fn dropping_pending_session_reads_and_queries_releases_their_state() {
        let platform = Rc::new(TestPlatform::default());
        let session = ManagedRelaySession::new(
            "account".into(),
            platform.clone(),
            Rc::new(|| futures::future::pending().boxed_local()),
        );
        let weak = Rc::downgrade(&session.state);
        let mut read = session.read_clerk_token().boxed_local();
        assert!(read.as_mut().now_or_never().is_none());
        drop(read);
        drop(session);
        assert!(weak.upgrade().is_none());
        let client = ManagedRelayClient::new(
            platform.clone(),
            Rc::new(TestSigner::default()),
            "https://relay.test",
            RelayPublicClientId::T3Web,
        );
        let manager = ManagedRelayQueryManager::new(client, platform.clone());
        let clerk = jwt("account");
        manager.set_session(Some(ManagedRelaySession::new(
            "account".into(),
            platform.clone(),
            Rc::new(move || {
                futures::future::ready(Ok(Some(Sensitive(clerk.clone())))).boxed_local()
            }),
        )));
        let weak = Rc::downgrade(&manager.state);
        let _release = platform.pending();
        let mut query = manager
            .load("account", RelayQueryKind::Devices)
            .boxed_local();
        assert!(query.as_mut().now_or_never().is_none());
        drop(query);
        drop(manager);
        assert!(weak.upgrade().is_none());
    }
    #[test]
    fn discovery_refreshes_on_credential_events_before_a_manual_refresh() {
        let platform = Rc::new(TestPlatform::default());
        let client = ManagedRelayClient::new(
            platform.clone(),
            Rc::new(TestSigner::default()),
            "https://relay.test",
            RelayPublicClientId::T3Web,
        );
        let discovery = RelayDiscovery::new(client);
        let mut driver = discovery.run(platform.clone()).boxed_local();
        assert!(driver.as_mut().now_or_never().is_none());
        platform
            .fake
            .push_host(Ok(HostReply::Text(Some(jwt("account")))));
        platform.respond(200, json!({"environments":[]}));
        platform
            .fake
            .emit(crate::platform::PlatformEvent::CredentialsChanged);
        assert!(driver.as_mut().now_or_never().is_none());
        assert!(discovery.snapshot().listed);
        assert_eq!(platform.requests.borrow().len(), 1);
    }
    #[test]
    fn a_failed_listing_emits_one_failure_for_all_concurrent_readers() {
        let platform = Rc::new(TestPlatform::default());
        let client = ManagedRelayClient::new(
            platform.clone(),
            Rc::new(TestSigner::default()),
            "https://relay.test",
            RelayPublicClientId::T3Web,
        );
        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = events.clone();
        let manager = ManagedRelayQueryManager::new(client, platform.clone())
            .with_observer(Rc::new(move |event| observed.borrow_mut().push(event)));
        let clerk = jwt("account");
        manager.set_session(Some(ManagedRelaySession::new(
            "account".into(),
            platform.clone(),
            Rc::new(move || {
                futures::future::ready(Ok(Some(Sensitive(clerk.clone())))).boxed_local()
            }),
        )));
        let release = platform.pending();
        let mut a = manager
            .load("account", RelayQueryKind::Devices)
            .boxed_local();
        let mut b = manager
            .load("account", RelayQueryKind::Devices)
            .boxed_local();
        assert!(a.as_mut().now_or_never().is_none());
        assert!(b.as_mut().now_or_never().is_none());
        release
            .send(crate::platform::HttpResponse {
                status: 500,
                headers: BTreeMap::new(),
                body: vec![],
            })
            .unwrap();
        assert!(futures::executor::block_on(a).is_err());
        assert!(futures::executor::block_on(b).is_err());
        assert_eq!(
            events
                .borrow()
                .iter()
                .filter(|event| event.phase == RelayQueryPhase::Failure)
                .count(),
            1
        );
    }
}

#[cfg(test)]
mod account_attachment_tests {
    use super::*;
    #[derive(Default)]
    struct Handlers {
        activated: RefCell<Vec<String>>,
        detached: Cell<u64>,
    }
    impl CloudAccountHandlers for Handlers {
        fn deactivate(&self) {}
        fn clean_up(
            &self,
            _account: Option<String>,
            _was_active: bool,
        ) -> LocalBoxFuture<'_, Result<(), RelayError>> {
            futures::future::ready(Ok(())).boxed_local()
        }
        fn prepare(
            &self,
            _account: String,
            _current: Rc<dyn Fn() -> bool>,
        ) -> LocalBoxFuture<'_, Result<(), RelayError>> {
            futures::future::ready(Ok(())).boxed_local()
        }
        fn activate(&self, account: &str, _transition: bool) {
            self.activated.borrow_mut().push(account.into());
        }
        fn detach(&self) {
            self.detached.set(self.detached.get() + 1);
        }
    }
    #[test]
    fn attaching_a_new_source_releases_the_old_source_and_ignores_its_late_emissions() {
        let handlers = Rc::new(Handlers::default());
        let (service, mut driver) = CloudAccountService::new(
            handlers.clone(),
            crate::authorization::tests::service(Rc::default(), Rc::default()),
        );
        let first = service.attach();
        first.observe(Some(Some("a".into())));
        assert!(driver.as_mut().now_or_never().is_none());
        let second = service.attach();
        assert_eq!(handlers.detached.get(), 1);
        first.observe(Some(Some("stale".into())));
        second.observe(Some(Some("b".into())));
        assert!(driver.as_mut().now_or_never().is_none());
        drop(first);
        assert_eq!(handlers.detached.get(), 1);
        assert_eq!(*handlers.activated.borrow(), vec!["a", "b"]);
        drop(second);
        assert_eq!(handlers.detached.get(), 2);
        assert_eq!(service.state(), CloudAccountState::Unobserved);
    }
}
