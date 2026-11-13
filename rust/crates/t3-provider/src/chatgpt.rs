use crate::{
    auth::{AuthBackend, AuthContext, AuthReply},
    sensitive::{SecretString, Sensitive, ServiceError, ServiceResult},
    transport::{HttpRequest, HttpTransport, ProviderClock},
};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use t3_contracts::provider_setup::{
    ChatGptReconnectProfile, ChatGptTransferredProfile, ProviderAuthCallbackMode,
    ProviderAuthInteraction, ProviderAuthMethod,
};
use tokio::sync::Mutex;

pub const RESOURCE: &str = "https://api.openai.com/v1";
pub const REQUIRED_SCOPE: &str = "chatgpt.tokens.use.direct";
pub const DISCOVERY: &str = "https://auth.openai.com/.well-known/openid-configuration";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    pub client_id: String,
    pub access_token: SecretString,
    #[serde(with = "t3_contracts::wire::nullable")]
    pub refresh_token: Option<SecretString>,
    pub expires_at: f64,
    #[serde(with = "t3_contracts::wire::nullable")]
    pub earliest_refresh_at: Option<f64>,
    pub scopes: Vec<String>,
    pub subject: String,
    #[serde(with = "t3_contracts::wire::nullable")]
    pub email: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub id_token: Option<SecretString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub issuer: Option<String>,
}
impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credentials([REDACTED])")
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sessions {
    #[serde(with = "t3_contracts::wire::nullable")]
    pub active_client_id: Option<String>,
    pub sessions: Vec<Credentials>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Registrations {
    pub profiles: Vec<ChatGptReconnectProfile>,
    #[serde(with = "t3_contracts::wire::nullable")]
    pub last_client_id: Option<String>,
}
#[derive(Clone, Default)]
pub struct SavedProfiles {
    pub sessions: Sessions,
    pub registrations: Registrations,
}
impl std::fmt::Debug for SavedProfiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SavedProfiles([REDACTED])")
    }
}

#[async_trait]
pub trait ProfileStore: Send + Sync {
    async fn load(&self) -> ServiceResult<SavedProfiles>;
    async fn save(&self, profiles: &SavedProfiles) -> ServiceResult<()>;
    async fn lease(&self) -> ServiceResult<crate::credential_lease::CredentialLease> {
        Ok(crate::credential_lease::CredentialLease::memory())
    }
}
pub struct ConfigProfileStore {
    directory: std::path::PathBuf,
    sessions: crate::secrets::CredentialStore,
    registrations: crate::secrets::CredentialStore,
}
impl ConfigProfileStore {
    pub fn new(directory: impl Into<std::path::PathBuf>, instance: &str) -> Self {
        let directory = directory.into();
        let store = t3_config::secrets::SecretStore::new(&directory);
        Self {
            directory,
            sessions: crate::secrets::CredentialStore::new(
                store.clone(),
                "codex-chatgpt",
                instance,
            ),
            registrations: crate::secrets::CredentialStore::new(
                store,
                "codex-chatgpt-registration",
                instance,
            ),
        }
    }
}
#[async_trait]
impl ProfileStore for ConfigProfileStore {
    async fn lease(&self) -> ServiceResult<crate::credential_lease::CredentialLease> {
        crate::credential_lease::CredentialLease::acquire(&self.directory, self.sessions.key())
            .await
    }
    async fn load(&self) -> ServiceResult<SavedProfiles> {
        let sessions = match self.sessions.get().map_err(|_| ServiceError::Storage)? {
            None => Sessions::default(),
            Some(bytes) => {
                let value: Value =
                    serde_json::from_slice(&bytes).map_err(|_| ServiceError::Storage)?;
                if value.get("sessions").is_some() {
                    serde_json::from_value(value).map_err(|_| ServiceError::Storage)?
                } else {
                    let record: Credentials =
                        serde_json::from_value(value).map_err(|_| ServiceError::Storage)?;
                    Sessions {
                        active_client_id: Some(record.client_id.clone()),
                        sessions: vec![record],
                    }
                }
            }
        };
        let registrations = match self
            .registrations
            .get()
            .map_err(|_| ServiceError::Storage)?
        {
            None => Registrations::default(),
            Some(bytes) => {
                let value: Value =
                    serde_json::from_slice(&bytes).map_err(|_| ServiceError::Storage)?;
                if value.get("profiles").is_some() {
                    serde_json::from_value(value).map_err(|_| ServiceError::Storage)?
                } else {
                    let record: ChatGptReconnectProfile =
                        serde_json::from_value(value).map_err(|_| ServiceError::Storage)?;
                    Registrations {
                        last_client_id: Some(record.client_id.clone()),
                        profiles: vec![record],
                    }
                }
            }
        };
        Ok(SavedProfiles {
            sessions,
            registrations,
        })
    }
    async fn save(&self, profiles: &SavedProfiles) -> ServiceResult<()> {
        let registrations =
            serde_json::to_vec(&profiles.registrations).map_err(|_| ServiceError::Storage)?;
        self.registrations
            .set(&registrations)
            .await
            .map_err(|_| ServiceError::Storage)?;
        if profiles.sessions.sessions.is_empty() {
            self.sessions.remove().map_err(|_| ServiceError::Storage)?;
        } else {
            self.sessions
                .set(&serde_json::to_vec(&profiles.sessions).map_err(|_| ServiceError::Storage)?)
                .await
                .map_err(|_| ServiceError::Storage)?;
        }
        Ok(())
    }
}

#[derive(Clone, Deserialize)]
pub struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    #[serde(default, with = "t3_contracts::wire::optional")]
    pub revocation_endpoint: Option<String>,
}
#[derive(Clone)]
pub struct VerifiedIdentity {
    pub subject: String,
    pub email: Option<String>,
    pub nonce: Option<String>,
}
impl std::fmt::Debug for VerifiedIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VerifiedIdentity([REDACTED])")
    }
}
#[async_trait]
pub trait IdentityVerifier: Send + Sync {
    /// Verify RS256/ES256 signature, issuer, audience and required exp (5s tolerance).
    async fn verify(
        &self,
        token: &SecretString,
        metadata: &Discovery,
        audience: &str,
        now_ms: i64,
    ) -> ServiceResult<VerifiedIdentity>;
}
pub trait Entropy: Send + Sync {
    fn bytes(&self, size: usize) -> ServiceResult<Vec<u8>>;
}
pub struct RandomEntropy;
impl Entropy for RandomEntropy {
    fn bytes(&self, size: usize) -> ServiceResult<Vec<u8>> {
        let mut bytes = vec![0; size];
        getrandom::fill(&mut bytes).map_err(|_| ServiceError::Response)?;
        Ok(bytes)
    }
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: SecretString,
    #[serde(default, with = "t3_contracts::wire::optional")]
    refresh_token: Option<SecretString>,
    #[serde(default, with = "t3_contracts::wire::optional")]
    id_token: Option<SecretString>,
    token_type: String,
    expires_in: u64,
    scope: String,
    #[serde(default, with = "t3_contracts::wire::optional")]
    earliest_refresh_at: Option<Value>,
}
pub struct PendingLogin {
    pub authorization_url: Sensitive<String>,
    pub redirect_uri: String,
    state: SecretString,
    nonce: SecretString,
    verifier: SecretString,
    client_id: Option<String>,
    expected_subject: Option<String>,
}
impl std::fmt::Debug for PendingLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PendingLogin([REDACTED])")
    }
}
#[async_trait]
pub trait CallbackListener: Send {
    fn redirect_uri(&self) -> &str;
    async fn receive(&mut self) -> ServiceResult<Sensitive<String>>;
    async fn respond(&mut self, accepted: bool);
    async fn finish(&mut self, accepted: bool) {
        self.respond(accepted).await;
    }
    fn return_url(&mut self, _return_url: Option<&str>) {}
}
#[async_trait]
pub trait CallbackFactory: Send + Sync {
    async fn listen(&self) -> ServiceResult<Box<dyn CallbackListener>>;
}

pub struct ChatGptAuth {
    store: Arc<dyn ProfileStore>,
    http: Arc<dyn HttpTransport>,
    verifier: Arc<dyn IdentityVerifier>,
    entropy: Arc<dyn Entropy>,
    clock: Arc<dyn ProviderClock>,
    callbacks: Arc<dyn CallbackFactory>,
    environment_id: String,
    discovery: Mutex<Option<Discovery>>,
    gate: Mutex<()>,
}
impl ChatGptAuth {
    pub fn new(
        store: Arc<dyn ProfileStore>,
        http: Arc<dyn HttpTransport>,
        verifier: Arc<dyn IdentityVerifier>,
        entropy: Arc<dyn Entropy>,
        clock: Arc<dyn ProviderClock>,
        callbacks: Arc<dyn CallbackFactory>,
        environment_id: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            http,
            verifier,
            entropy,
            clock,
            callbacks,
            environment_id,
            discovery: Mutex::new(None),
            gate: Mutex::new(()),
        })
    }
    async fn metadata(&self) -> ServiceResult<Discovery> {
        let mut current = self.discovery.lock().await;
        if let Some(current) = current.as_ref() {
            return Ok(current.clone());
        }
        let response = self
            .http
            .send(HttpRequest {
                method: "GET",
                url: DISCOVERY.into(),
                headers: BTreeMap::new(),
                body: None,
            })
            .await?;
        if !(200..300).contains(&response.status) {
            return Err(ServiceError::Network);
        }
        let metadata: Discovery = serde_json::from_value(response.body.into_inner())
            .map_err(|_| ServiceError::Response)?;
        if metadata.issuer != "https://auth.openai.com"
            || [
                &metadata.authorization_endpoint,
                &metadata.token_endpoint,
                &metadata.jwks_uri,
            ]
            .into_iter()
            .chain(metadata.revocation_endpoint.iter())
            .any(|value| {
                url::Url::parse(value).map_or(true, |u| {
                    u.origin().ascii_serialization() != metadata.issuer
                        || !u.username().is_empty()
                        || u.password().is_some()
                })
            })
        {
            return Err(ServiceError::Identity);
        }
        *current = Some(metadata.clone());
        Ok(metadata)
    }
    fn active(saved: &SavedProfiles) -> Option<&Credentials> {
        saved
            .sessions
            .sessions
            .iter()
            .find(|s| Some(&s.client_id) == saved.sessions.active_client_id.as_ref())
    }
    pub async fn read(&self) -> ServiceResult<Option<Credentials>> {
        Ok(Self::active(&self.store.load().await?).cloned())
    }
    fn save_registration(saved: &mut SavedProfiles, mut profile: ChatGptReconnectProfile) {
        profile.id_token_hint = None;
        let existing = saved
            .registrations
            .profiles
            .iter()
            .find(|p| p.client_id == profile.client_id);
        let mut number = saved.registrations.profiles.len() + 1;
        while saved
            .registrations
            .profiles
            .iter()
            .any(|p| p.connection_label.as_deref() == Some(&format!("Connection {number}")))
        {
            number += 1;
        }
        profile.connection_label = existing
            .and_then(|p| p.connection_label.clone())
            .or(profile.connection_label)
            .or_else(|| Some(format!("Connection {number}")));
        saved
            .registrations
            .profiles
            .retain(|p| p.client_id != profile.client_id);
        saved.registrations.last_client_id = Some(profile.client_id.clone());
        saved.registrations.profiles.insert(0, profile);
    }
    fn save_credentials(saved: &mut SavedProfiles, record: Credentials, activate: bool) {
        if activate {
            saved.sessions.active_client_id = Some(record.client_id.clone());
        }
        saved
            .sessions
            .sessions
            .retain(|s| s.client_id != record.client_id);
        saved.sessions.sessions.push(record);
    }
    async fn clear_active(
        &self,
        lease: &crate::credential_lease::CredentialLease,
    ) -> ServiceResult<()> {
        let mut saved = self.store.load().await?;
        if let Some(record) = Self::active(&saved).cloned() {
            if let Some(profile) = saved
                .registrations
                .profiles
                .iter_mut()
                .find(|p| p.client_id == record.client_id)
            {
                profile.subject.get_or_insert(record.subject);
                profile.email.get_or_insert(record.email);
            }
            saved
                .sessions
                .sessions
                .retain(|s| s.client_id != record.client_id);
        }
        saved.sessions.active_client_id = None;
        lease.check()?;
        self.store.save(&saved).await
    }
    async fn exchange(
        &self,
        lease: &crate::credential_lease::CredentialLease,
        pairs: &[(&str, &str)],
    ) -> ServiceResult<TokenResponse> {
        let metadata = self.metadata().await?;
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs.iter().copied())
            .finish();
        let response = self
            .http
            .send(HttpRequest {
                method: "POST",
                url: metadata.token_endpoint,
                headers: BTreeMap::from([
                    ("accept".into(), Sensitive::new("application/json".into())),
                    (
                        "content-type".into(),
                        Sensitive::new("application/x-www-form-urlencoded".into()),
                    ),
                ]),
                body: Some(Sensitive::new(body)),
            })
            .await?;
        let grant = pairs
            .iter()
            .find(|(k, _)| *k == "grant_type")
            .map(|(_, v)| *v);
        if !(200..300).contains(&response.status) {
            let error = response
                .body
                .expose()
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("");
            if grant == Some("refresh_token")
                && [
                    "invalid_grant",
                    "invalid_refresh_token",
                    "token_expired",
                    "refresh_token_expired",
                    "refresh_token_invalidated",
                    "refresh_token_reused",
                ]
                .contains(&error)
            {
                self.clear_active(lease).await?;
                return Err(ServiceError::Disconnected);
            }
            return Err(match error {
                "invalid_client" => ServiceError::Client,
                "invalid_grant" if grant == Some("authorization_code") => ServiceError::CodeExpired,
                _ => ServiceError::Response,
            });
        }
        let tokens: TokenResponse = serde_json::from_value(response.body.into_inner())
            .map_err(|_| ServiceError::Response)?;
        if tokens.access_token.expose().is_empty()
            || tokens.expires_in == 0
            || tokens
                .refresh_token
                .as_ref()
                .is_some_and(|s| s.expose().is_empty())
            || tokens
                .earliest_refresh_at
                .as_ref()
                .is_some_and(|v| !v.is_number() && !v.is_string())
        {
            return Err(ServiceError::Response);
        }
        Ok(tokens)
    }
    fn earliest(value: Option<&Value>) -> Option<f64> {
        let value = value?;
        value
            .as_f64()
            .map(|v| v * 1000.)
            .or_else(|| {
                value
                    .as_str()
                    .and_then(crate::catalog::date_millis)
                    .map(|v| v as f64)
            })
            .filter(|v| v.is_finite())
    }
    async fn validate_import(
        &self,
        profile: &Sensitive<ChatGptTransferredProfile>,
    ) -> ServiceResult<()> {
        let profile = profile.expose();
        let credentials = &profile.credentials;
        let metadata = self.metadata().await?;
        if credentials.client_id != profile.registration.client_id
            || credentials.issuer != metadata.issuer
            || !credentials.scopes.iter().any(|s| s == REQUIRED_SCOPE)
            || credentials.expires_at.value() <= self.clock.now_ms() as f64
        {
            return Err(ServiceError::Identity);
        }
        let identity = self
            .verifier
            .verify(
                &Sensitive::new(credentials.id_token.clone()),
                &metadata,
                &credentials.client_id,
                self.clock.now_ms(),
            )
            .await?;
        if identity.subject != credentials.subject
            || profile.registration.subject.as_ref() != Some(&credentials.subject)
            || identity.email != credentials.email
            || profile
                .registration
                .email
                .as_ref()
                .is_some_and(|e| *e != credentials.email)
        {
            return Err(ServiceError::Identity);
        }
        let saved = self.store.load().await?;
        if saved.registrations.profiles.iter().any(|p| {
            p.client_id == credentials.client_id
                && p.subject
                    .as_ref()
                    .is_some_and(|s| *s != credentials.subject)
        }) {
            return Err(ServiceError::Identity);
        }
        Ok(())
    }
    pub async fn saved_profiles(&self) -> ServiceResult<SavedProfiles> {
        self.store.load().await
    }
    pub async fn export_profile(&self) -> ServiceResult<Sensitive<ChatGptTransferredProfile>> {
        let _gate = self.gate.lock().await;
        let lease = self.store.lease().await?;
        lease
            .run(async {
                        lease.check()?;
                        let saved = self.store.load().await?;
                        let credentials = Self::active(&saved).ok_or(ServiceError::Unavailable)?;
                        let registration = saved
                            .registrations
                            .profiles
                            .iter()
                            .find(|p| p.client_id == credentials.client_id)
                            .ok_or(ServiceError::Unavailable)?;
                        let credentials = serde_json::from_value(
                            serde_json::to_value(credentials)
                                .map_err(|_| ServiceError::Response)?,
                        )
                        .map_err(|_| ServiceError::Response)?;
                        Ok(Sensitive::new(ChatGptTransferredProfile {
                            registration: registration.clone(),
                            credentials,
                        }))
            })
            .await
    }
    pub async fn begin(&self, method: &str, redirect_uri: String) -> ServiceResult<PendingLogin> {
        let redirect = url::Url::parse(&redirect_uri).map_err(|_| ServiceError::Callback)?;
        if redirect.scheme() != "http"
            || redirect.host_str() != Some("127.0.0.1")
            || redirect.port().is_none()
            || redirect.path() != "/auth/callback"
            || !redirect.username().is_empty()
            || redirect.password().is_some()
            || redirect.query().is_some()
            || redirect.fragment().is_some()
        {
            return Err(ServiceError::Callback);
        }
        let metadata = self.metadata().await?;
        let saved = self.store.load().await?;
        let previous = Self::active(&saved);
        let selected = method
            .strip_prefix("chatgpt-profile:")
            .map(str::to_owned)
            .or_else(|| previous.map(|p| p.client_id.clone()))
            .or(saved.registrations.last_client_id.clone());
        let registration = if method == "chatgpt-change-account" {
            None
        } else {
            saved
                .registrations
                .profiles
                .iter()
                .find(|p| Some(&p.client_id) == selected.as_ref())
        };
        let legacy = registration
            .and_then(|p| p.redirect_uri.as_ref())
            .is_some_and(|s| s.contains("//localhost:"));
        let client_id = registration
            .filter(|_| !legacy)
            .map(|p| p.client_id.clone());
        let tokens = saved
            .sessions
            .sessions
            .iter()
            .find(|s| Some(&s.client_id) == selected.as_ref());
        let state = URL_SAFE_NO_PAD.encode(self.entropy.bytes(32)?);
        let nonce = URL_SAFE_NO_PAD.encode(self.entropy.bytes(32)?);
        let verifier = URL_SAFE_NO_PAD.encode(self.entropy.bytes(64)?);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut url = url::Url::parse(&metadata.authorization_endpoint)
            .map_err(|_| ServiceError::Identity)?;
        {
            let mut query = url.query_pairs_mut();
            query.clear();
            query.append_pair(
                "client_id",
                client_id.as_deref().unwrap_or("dynamic_agent_client"),
            );
            if client_id.is_some() {
                if let Some(email) = registration
                    .and_then(|r| r.email.as_ref())
                    .and_then(Option::as_ref)
                {
                    query.append_pair("login_hint", email);
                }
                if let Some(hint) = tokens
                    .and_then(|t| t.id_token.as_ref())
                    .map(|t| t.expose().as_str())
                    .or_else(|| registration.and_then(|r| r.id_token_hint.as_deref()))
                {
                    query.append_pair("id_token_hint", hint);
                }
                if registration.is_some_and(|r| r.sharing_enabled == Some(false))
                    || tokens.is_some_and(|r| !r.scopes.iter().any(|s| s == REQUIRED_SCOPE))
                {
                    query.append_pair("prompt", "consent");
                }
            } else {
                query.append_pair("agent_name_hint", "T3 Code");
            }
            for (k, v) in [
                (
                    "ext_agent_host_id",
                    format!("urn:uuid:{}", self.environment_id),
                ),
                ("response_type", "code".into()),
                ("redirect_uri", redirect_uri.clone()),
                (
                    "scope",
                    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct"
                        .into(),
                ),
                ("resource", RESOURCE.into()),
                ("state", state.clone()),
                ("nonce", nonce.clone()),
                ("code_challenge_method", "S256".into()),
                ("code_challenge", challenge),
            ] {
                query.append_pair(k, &v);
            }
        }
        Ok(PendingLogin {
            authorization_url: Sensitive::new(url.into()),
            redirect_uri,
            state: Sensitive::new(state),
            nonce: Sensitive::new(nonce),
            verifier: Sensitive::new(verifier),
            client_id,
            expected_subject: registration.and_then(|p| p.subject.clone()).or_else(|| {
                previous
                    .filter(|p| Some(&p.client_id) == selected.as_ref())
                    .map(|p| p.subject.clone())
            }),
        })
    }
    pub async fn finish(&self, pending: PendingLogin, callback: &str) -> ServiceResult<()> {
        let _gate = self.gate.lock().await;
        let lease = self.store.lease().await?;
        lease.run(async {
        lease.check()?;
        let returned = validate_callback(callback, &pending.redirect_uri, pending.state.expose())?;
        let query: BTreeMap<_, _> = returned
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        if query.contains_key("error") {
            return Err(ServiceError::Cancelled);
        }
        let client = pending
            .client_id
            .clone()
            .or_else(|| query.get("client_id").cloned())
            .filter(|s| s.starts_with("oaiapp_"))
            .ok_or(ServiceError::Callback)?;
        if pending
            .client_id
            .as_ref()
            .is_some_and(|c| query.get("client_id").is_some_and(|r| r != c))
        {
            return Err(ServiceError::Callback);
        }
        let tokens = match self
            .exchange(
                &lease,
                &[
                    ("grant_type", "authorization_code"),
                    ("client_id", &client),
                    ("code", query.get("code").ok_or(ServiceError::Callback)?),
                    ("code_verifier", pending.verifier.expose()),
                    ("redirect_uri", &pending.redirect_uri),
                    ("resource", RESOURCE),
                ],
            )
            .await
        {
            Ok(tokens) => tokens,
            Err(ServiceError::CodeExpired) if pending.client_id.is_none() => {
                let mut saved = self.store.load().await?;
                Self::save_registration(
                    &mut saved,
                    serde_json::from_value(
                        json!({"clientId":client,"redirectUri":pending.redirect_uri}),
                    )
                    .map_err(|_| ServiceError::Response)?,
                );
                lease.check()?;
                self.store.save(&saved).await?;
                return Err(ServiceError::CodeExpired);
            }
            Err(error) => return Err(error),
        };
        let id_token = tokens.id_token.as_ref().ok_or(ServiceError::Identity)?;
        let metadata = self.metadata().await?;
        let identity = self
            .verifier
            .verify(id_token, &metadata, &client, self.clock.now_ms())
            .await?;
        if identity.nonce.as_deref() != Some(pending.nonce.expose().as_str())
            || pending.client_id.is_some()
                && pending
                    .expected_subject
                    .as_ref()
                    .is_some_and(|s| *s != identity.subject)
            || !tokens.token_type.eq_ignore_ascii_case("bearer")
        {
            return Err(ServiceError::Identity);
        }
        let mut saved = self.store.load().await?;
        if let Some(existing) = saved
            .registrations
            .profiles
            .iter()
            .find(|p| p.client_id == client)
        {
            if existing
                .subject
                .as_ref()
                .is_some_and(|s| *s != identity.subject)
            {
                return Err(ServiceError::Identity);
            }
            if let Some(original) = existing.redirect_uri.as_ref() {
                let original = url::Url::parse(original).map_err(|_| ServiceError::Identity)?;
                let current =
                    url::Url::parse(&pending.redirect_uri).map_err(|_| ServiceError::Identity)?;
                if original.scheme() != current.scheme()
                    || original.host_str() != current.host_str()
                    || original.path() != current.path()
                {
                    return Err(ServiceError::Identity);
                }
            }
        }
        let scopes: Vec<_> = tokens.scope.split_whitespace().map(str::to_owned).collect();
        let sharing = scopes.iter().any(|s| s == REQUIRED_SCOPE);
        Self::save_registration(&mut saved, serde_json::from_value(json!({"clientId":client,"redirectUri":pending.redirect_uri,"sharingEnabled":sharing,"subject":identity.subject,"email":identity.email})).map_err(|_|ServiceError::Response)?);
        let activate = sharing || Self::active(&saved).is_none();
        Self::save_credentials(
            &mut saved,
            Credentials {
                client_id: client,
                access_token: tokens.access_token,
                refresh_token: tokens.refresh_token,
                expires_at: self.clock.now_ms() as f64 + tokens.expires_in as f64 * 1000.,
                earliest_refresh_at: Self::earliest(tokens.earliest_refresh_at.as_ref()),
                scopes,
                subject: identity.subject,
                email: identity.email,
                id_token: tokens.id_token,
                issuer: Some(metadata.issuer),
            },
            activate,
        );
        lease.check()?;
        self.store.save(&saved).await?;
        if sharing {
            Ok(())
        } else {
            Err(ServiceError::Sharing)
        }
        }).await
    }
    pub fn profile_key(record: &Credentials) -> ServiceResult<String> {
        let tuple = serde_json::to_vec(&json!([
            record.client_id,
            record.subject,
            record.id_token.as_ref().map(|s| s.expose())
        ]))
        .map_err(|_| ServiceError::Response)?;
        Ok(format!("{:x}", Sha256::digest(tuple)))
    }
    pub async fn access(
        self: &Arc<Self>,
        expected_profile: Option<String>,
    ) -> ServiceResult<Credentials> {
        let this = self.clone();
        tokio::spawn(async move { this.access_inner(expected_profile.as_deref()).await })
            .await
            .map_err(|_| ServiceError::Refresh)?
    }
    async fn access_inner(&self, expected: Option<&str>) -> ServiceResult<Credentials> {
        let _gate = self.gate.lock().await;
        let lease = self.store.lease().await?;
        lease
            .run(async {
                lease.check()?;
                let mut record = self.read().await?.ok_or(ServiceError::Disconnected)?;
                if expected.is_some_and(|key| {
                    Self::profile_key(&record).map_or(true, |current| current != key)
                }) {
                    return Err(ServiceError::ProfileChanged);
                }
                if !record.scopes.iter().any(|s| s == REQUIRED_SCOPE) {
                    return Err(ServiceError::Sharing);
                }
                let now = self.clock.now_ms() as f64;
                if record.expires_at - now > 60_000. {
                    return Ok(record);
                }
                if record.earliest_refresh_at.is_some_and(|t| t > now) {
                    return if record.expires_at > now {
                        Ok(record)
                    } else {
                        Err(ServiceError::RefreshEarly)
                    };
                }
                let Some(refresh) = record.refresh_token.as_ref() else {
                    self.clear_active(&lease).await?;
                    return Err(ServiceError::Disconnected);
                };
                let tokens = tokio::time::timeout(
                    Duration::from_secs(20),
                    self.exchange(
                        &lease,
                        &[
                            ("grant_type", "refresh_token"),
                            ("client_id", &record.client_id),
                            ("refresh_token", refresh.expose()),
                            ("resource", RESOURCE),
                        ],
                    ),
                )
                .await
                .map_err(|_| ServiceError::Refresh)??;
                if tokens.refresh_token.is_none()
                    || !tokens.token_type.eq_ignore_ascii_case("bearer")
                {
                    return Err(ServiceError::Refresh);
                }
                record.access_token = tokens.access_token;
                record.refresh_token = tokens.refresh_token;
                record.expires_at = self.clock.now_ms() as f64 + tokens.expires_in as f64 * 1000.;
                record.earliest_refresh_at = Self::earliest(tokens.earliest_refresh_at.as_ref());
                record.scopes = tokens.scope.split_whitespace().map(str::to_owned).collect();
                let mut saved = self.store.load().await?;
                Self::save_credentials(&mut saved, record.clone(), true);
                lease.check()?;
                self.store.save(&saved).await?;
                if record.scopes.iter().any(|s| s == REQUIRED_SCOPE) {
                    Ok(record)
                } else {
                    Err(ServiceError::Sharing)
                }
            })
            .await
    }
}

pub fn validate_callback(value: &str, redirect: &str, state: &str) -> ServiceResult<url::Url> {
    let expected = url::Url::parse(redirect).map_err(|_| ServiceError::Callback)?;
    let pasted = value.trim_start();
    let callback = url::Url::parse(if pasted.contains("://") { pasted } else { "" })
        .or_else(|_| url::Url::parse(&format!("{}://{pasted}", expected.scheme())))
        .map_err(|_| ServiceError::Callback)?;
    let single = |key: &str| -> ServiceResult<Option<String>> {
        let values: Vec<_> = callback
            .query_pairs()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
            .collect();
        if values.len() > 1 {
            Err(ServiceError::Callback)
        } else {
            Ok(values.into_iter().next())
        }
    };
    let code = single("code")?;
    let error = single("error")?;
    let client = single("client_id")?;
    if value.encode_utf16().count() > 16384
        || callback.origin() != expected.origin()
        || callback.path() != expected.path()
        || !callback.username().is_empty()
        || callback.password().is_some()
        || callback.fragment().is_some()
        || single("state")?.as_deref() != Some(state)
        || client.as_ref().is_some_and(|s| {
            !s.strip_prefix("oaiapp_").is_some_and(|s| {
                !s.is_empty()
                    && s.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            })
        })
        || !(code.as_ref().is_some_and(|s| !s.is_empty()) && error.is_none()
            || error.as_ref().is_some_and(|s| !s.is_empty()) && code.is_none())
    {
        return Err(ServiceError::Callback);
    }
    Ok(callback)
}

#[async_trait]
impl AuthBackend for ChatGptAuth {
    async fn methods(&self) -> ServiceResult<Vec<ProviderAuthMethod>> {
        let saved = self.store.load().await?;
        let current = Self::active(&saved)
            .map(|s| s.client_id.as_str())
            .or(saved.registrations.last_client_id.as_deref());
        let mut profiles = saved.registrations.profiles.clone();
        profiles.sort_by_key(|p| Some(p.client_id.as_str()) != current);
        let mut methods = vec![
            json!({"id":"chatgpt","name":"Sign in with ChatGPT","description":profiles.first().and_then(|p|p.email.as_ref()).and_then(Option::as_ref).map(|s|format!("Reconnect {s}.")),"type":"agent"}),
            json!({"id":"chatgpt-change-account","name":"Use a different ChatGPT account","description":"Register a connection for another ChatGPT account.","type":"agent"}),
        ];
        for (index, profile) in profiles.into_iter().take(30).enumerate() {
            let label = profile
                .connection_label
                .unwrap_or_else(|| format!("Connection {}", index + 1));
            methods.push(json!({"id":format!("chatgpt-profile:{}",profile.client_id),"canRemove":profile.subject.is_none() && !saved.sessions.sessions.iter().any(|s|s.client_id == profile.client_id),"name":format!("{} · {label}",profile.email.as_ref().and_then(Option::as_ref).map_or("ChatGPT account",String::as_str)),"description":"Reuse this account's original sign-in registration.","type":"agent"}));
        }
        serde_json::from_value(json!(methods)).map_err(|_| ServiceError::Response)
    }
    async fn authenticate(&self, method: &str, mut context: AuthContext) -> ServiceResult<()> {
        let mut listener = if context.callback_mode == Some(ProviderAuthCallbackMode::Client) {
            None
        } else {
            Some(self.callbacks.listen().await?)
        };
        if let Some(listener) = listener.as_mut() {
            listener.return_url(context.return_url.as_deref());
        }
        let redirect = if let Some(listener) = listener.as_ref() {
            listener.redirect_uri().into()
        } else {
            let bytes = self.entropy.bytes(2)?;
            let port = 49152 + u16::from_be_bytes([bytes[0], bytes[1]]) % 16384;
            format!("http://127.0.0.1:{port}/auth/callback")
        };
        let pending = self.begin(method, redirect).await?;
        context
            .interaction(
                ProviderAuthInteraction::Browser {
                    id: context.flow_id.clone().into(),
                    url: pending.authorization_url.expose().clone().into(),
                    requires_consent: false,
                    accepts_callback: Some(true),
                },
                false,
                true,
            )
            .await;
        loop {
            let next = async {
                if let Some(listener) = listener.as_mut() {
                    listener.receive().await
                } else {
                    std::future::pending().await
                }
            };
            tokio::select! {
                reply = context.receive() => {
                    let reply = reply?;
                    let callback = match &reply.reply { AuthReply::Callback(callback) => callback.expose().clone(), _ => { reply.finish(Err(ServiceError::Interaction)); continue; } };
                    if let Err(error) = validate_callback(&callback,&pending.redirect_uri,pending.state.expose()) { reply.finish(Err(error)); continue; }
                    reply.finish(Ok(()));
                    context.verifying().await;
                    let result = self.finish(pending,&callback).await;
                    if let Some(listener) = listener.as_mut() { listener.finish(result.is_ok()).await; }
                    return result;
                }
                callback = next => {
                    let callback = callback?;
                    if validate_callback(callback.expose(),&pending.redirect_uri,pending.state.expose()).is_err() { if let Some(listener) = listener.as_mut() { listener.respond(false).await; } continue; }
                    context.verifying().await;
                    let result = self.finish(pending,callback.expose()).await;
                    if let Some(listener) = listener.as_mut() { listener.finish(result.is_ok()).await; }
                    return result;
                }
            }
        }
    }
    async fn logout(&self) -> ServiceResult<Option<String>> {
        let _gate = self.gate.lock().await;
        let lease = self.store.lease().await?;
        lease.run(async {
        lease.check()?;
        let record = self.read().await?;
        let mut confirmed = record.as_ref().is_none_or(|r| r.refresh_token.is_none());
        if let Some(record) = record
            && let Some(refresh) = record.refresh_token
        {
            for attempt in 0..3 {
                let result = async {
                    let metadata = self.metadata().await?;
                    let Some(endpoint) = metadata.revocation_endpoint else {
                        return Ok((false, false));
                    };
                    let body = url::form_urlencoded::Serializer::new(String::new())
                        .extend_pairs([
                            ("token", refresh.expose().as_str()),
                            ("token_type_hint", "refresh_token"),
                            ("client_id", &record.client_id),
                        ])
                        .finish();
                    let response = self
                        .http
                        .send(HttpRequest {
                            method: "POST",
                            url: endpoint,
                            headers: BTreeMap::from([(
                                "content-type".into(),
                                Sensitive::new("application/x-www-form-urlencoded".into()),
                            )]),
                            body: Some(Sensitive::new(body)),
                        })
                        .await?;
                    Ok::<_, ServiceError>((response.status == 200, response.status >= 500))
                };
                let (success, retry) = tokio::time::timeout(Duration::from_secs(10), result)
                    .await
                    .unwrap_or(Err(ServiceError::Network))
                    .unwrap_or((false, true));
                confirmed = success;
                if success || !retry || attempt == 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(if attempt == 0 { 250 } else { 1000 }))
                    .await;
            }
        }
        self.clear_active(&lease).await?;
        Ok((!confirmed).then(||"Signed out locally. Remote revocation could not be confirmed. Disconnect the app in ChatGPT Settings.".into()))
        }).await
    }
    async fn remove_method(&self, method: &str) -> ServiceResult<()> {
        let _gate = self.gate.lock().await;
        let lease = self.store.lease().await?;
        lease
            .run(async {
                lease.check()?;
                let mut saved = self.store.load().await?;
                let client = method
                    .strip_prefix("chatgpt-profile:")
                    .ok_or(ServiceError::RemoveMethod)?;
                if !saved
                    .registrations
                    .profiles
                    .iter()
                    .any(|p| p.client_id == client && p.subject.is_none())
                    || saved
                        .sessions
                        .sessions
                        .iter()
                        .any(|s| s.client_id == client)
                {
                    return Err(ServiceError::RemoveMethod);
                }
                saved
                    .registrations
                    .profiles
                    .retain(|p| p.client_id != client);
                if saved.registrations.last_client_id.as_deref() == Some(client) {
                    saved.registrations.last_client_id = saved
                        .registrations
                        .profiles
                        .first()
                        .map(|p| p.client_id.clone());
                }
                lease.check()?;
                self.store.save(&saved).await
            })
            .await
    }
    async fn reconnect_profile(
        &self,
        method: &str,
    ) -> ServiceResult<Option<Sensitive<ChatGptReconnectProfile>>> {
        if method == "chatgpt-change-account" {
            return Ok(None);
        }
        let saved = self.store.load().await?;
        let client = method
            .strip_prefix("chatgpt-profile:")
            .or_else(|| Self::active(&saved).map(|r| r.client_id.as_str()))
            .or(saved.registrations.last_client_id.as_deref());
        let profile = saved
            .registrations
            .profiles
            .iter()
            .find(|p| Some(p.client_id.as_str()) == client)
            .cloned();
        if profile.is_none() && method.starts_with("chatgpt-profile:") {
            return Err(ServiceError::Unavailable);
        }
        Ok(profile.map(|mut p| {
            p.id_token_hint = saved
                .sessions
                .sessions
                .iter()
                .find(|s| s.client_id == p.client_id)
                .and_then(|s| s.id_token.as_ref())
                .map(|t| t.expose().clone());
            Sensitive::new(p)
        }))
    }
    async fn validate_profile(
        &self,
        profile: &Sensitive<ChatGptTransferredProfile>,
    ) -> ServiceResult<()> {
        self.validate_import(profile).await
    }
    async fn import_profile(
        &self,
        profile: Sensitive<ChatGptTransferredProfile>,
    ) -> ServiceResult<()> {
        let _gate = self.gate.lock().await;
        let lease = self.store.lease().await?;
        lease
            .run(async {
                lease.check()?;
                self.validate_import(&profile).await?;
                let profile = profile.into_inner();
                let credentials = profile.credentials;
                let mut saved = self.store.load().await?;
                Self::save_registration(&mut saved, profile.registration);
                let record: Credentials = serde_json::from_value(
                    serde_json::to_value(credentials).map_err(|_| ServiceError::Response)?,
                )
                .map_err(|_| ServiceError::Response)?;
                Self::save_credentials(&mut saved, record, true);
                lease.check()?;
                self.store.save(&saved).await
            })
            .await
    }
}
