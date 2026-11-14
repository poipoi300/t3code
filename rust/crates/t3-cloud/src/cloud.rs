use crate::{
    Error, Rejection, Result, Secret, SharedClock,
    crypto::{self, EnvironmentKey},
    decode, encode,
    http::{Client, Sensitive},
    iso,
    store::{SecretStore, consume_replay, read_text},
};
use async_trait::async_trait;
use reqwest::Method;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use t3_contracts::{environment::ExecutionEnvironmentDescriptor, relay::*};
use tokio::sync::Mutex;

pub const RELAY_URL: &str = "cloud-relay-url";
pub const RELAY_ISSUER: &str = "cloud-relay-issuer";
pub const USER: &str = "cloud-linked-user-id";
pub const CREDENTIAL: &str = "cloud-relay-environment-credential";
pub const MINT_KEY: &str = "cloud-mint-ed25519-public-key";
pub const RUNTIME: &str = "cloud-endpoint-runtime-config";
pub const REVOKED: &str = "cloud-link-revoked";
pub const PUBLISH: &str = "cloud-publish-agent-activity";
pub const HOLD: &str = "cloud-hold-webhooks-while-offline";
const CONFIG_NAMES: [&str; 7] = [
    RELAY_URL,
    RELAY_ISSUER,
    USER,
    CREDENTIAL,
    MINT_KEY,
    RUNTIME,
    REVOKED,
];

/// Issues a proof-bound Connect credential through the host's shared auth service.
#[async_trait]
pub trait CredentialIssuer: Send + Sync {
    async fn issue_cloud_pairing(&self, proof_key_thumbprint: &str) -> Result<IssuedCredential>;
}
#[derive(Debug)]
pub struct IssuedCredential {
    pub credential: Secret,
    pub expires_at_ms: i64,
}

#[cfg(feature = "auth")]
pub struct AuthCredentialIssuer {
    auth: t3_auth::EnvironmentAuth,
    settings: Arc<t3_config::settings::SettingsService>,
}
#[cfg(feature = "auth")]
impl AuthCredentialIssuer {
    pub fn new(
        auth: t3_auth::EnvironmentAuth,
        settings: Arc<t3_config::settings::SettingsService>,
    ) -> Self {
        Self { auth, settings }
    }
}
#[cfg(feature = "auth")]
#[async_trait]
impl CredentialIssuer for AuthCredentialIssuer {
    async fn issue_cloud_pairing(&self, proof_key_thumbprint: &str) -> Result<IssuedCredential> {
        let require_approval = self
            .settings
            .snapshot()
            .await
            .map_err(|_| Error::Storage)?
            .require_connect_device_approval;
        // Auth owns the shared mint/exchange/revoke lock and cancellation-safe mutation.
        let issued = self
            .auth
            .issue_connect_credential(proof_key_thumbprint, require_approval)
            .await
            .map_err(|_| Error::Storage)?;
        let expires = chrono::DateTime::parse_from_rfc3339(&issued.expires_at.0)
            .map_err(|_| Error::Protocol)?
            .timestamp_millis();
        Ok(IssuedCredential {
            credential: Secret::new(issued.credential.0),
            expires_at_ms: expires,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Registration {
    NotLinked,
    Ready,
    RecoveryRequired,
    Revoked,
    Superseded,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesiredLinkMode {
    Managed,
    PublishOnly,
}
#[derive(Debug)]
pub struct LinkRequestContext<'a> {
    pub request_url: &'a str,
    pub desktop_bootstrap: bool,
    pub forwarded_authority: bool,
}

struct Installed {
    url: String,
    issuer: String,
    user: String,
    credential: Secret,
    mint_key: String,
    runtime: Option<RelayManagedEndpointRuntimeConfig>,
}
#[derive(Clone)]
pub struct CloudLink {
    pub descriptor: ExecutionEnvironmentDescriptor,
    pub signing_key: Arc<EnvironmentKey>,
    secrets: SecretStore,
    http: crate::http::Http,
    clock: SharedClock,
    state_lock: Arc<Mutex<()>>,
    recovery_lock: Arc<Mutex<crate::retry::RecoveryCooldown>>,
}
impl CloudLink {
    pub fn link_proof_for_client(
        &self,
        request: &Sensitive<RelayLinkProofRequest>,
        context: LinkRequestContext<'_>,
    ) -> Result<Secret> {
        if context.forwarded_authority {
            return Err(Error::Configuration);
        }
        self.link_proof(request, context.request_url, context.desktop_bootstrap)
    }
    pub async fn new(
        descriptor: ExecutionEnvironmentDescriptor,
        secrets: SecretStore,
        http: crate::http::Http,
        clock: SharedClock,
    ) -> Result<Self> {
        let signing_key = Arc::new(EnvironmentKey::load(secrets.as_ref()).await?);
        Ok(Self {
            descriptor,
            signing_key,
            secrets,
            http,
            clock,
            state_lock: Arc::new(Mutex::new(())),
            recovery_lock: Arc::new(Mutex::new(Default::default())),
        })
    }
    fn id(&self) -> &str {
        &self.descriptor.environment_id.0
    }
    fn claims(&self, issuer: &str, lifetime: i64) -> Value {
        let now = self.clock.now_ms().div_euclid(1000);
        json!({"iss":format!("t3-env:{}",self.id()),"aud":crypto::normalize_issuer(issuer),"sub":self.id(),"jti":uuid::Uuid::new_v4().to_string(),"iat":now,"exp":now+lifetime,"environmentId":self.id()})
    }
    async fn installed(&self) -> Result<Installed> {
        let required = async |name| {
            read_text(self.secrets.as_ref(), name)
                .await?
                .filter(|s| !s.is_empty())
                .ok_or(Error::NotLinked)
        };
        let url = required(RELAY_URL).await?;
        let issuer = read_text(self.secrets.as_ref(), RELAY_ISSUER)
            .await?
            .unwrap_or_else(|| url.clone());
        crate::http::secure_origin(&url)?;
        crate::http::secure_origin(&issuer)?;
        let runtime = read_text(self.secrets.as_ref(), RUNTIME)
            .await?
            .map(|v| serde_json::from_str(&v).map_err(|_| Error::Storage))
            .transpose()?;
        Ok(Installed {
            url,
            issuer,
            user: required(USER).await?,
            credential: Secret::new(required(CREDENTIAL).await?),
            mint_key: required(MINT_KEY).await?,
            runtime,
        })
    }
    fn client(&self, config: &Installed) -> Result<Client> {
        Client::relay(self.http.clone(), &config.url, self.clock.clone())
    }
    async fn snapshot(&self) -> Result<Installed> {
        let _lock = self.state_lock.lock().await;
        self.installed().await
    }
    pub fn link_proof(
        &self,
        request: &Sensitive<RelayLinkProofRequest>,
        request_url: &str,
        allow_non_loopback: bool,
    ) -> Result<Secret> {
        let request = request.expose();
        crate::direct::allowed_link_origin(&request.origin, request_url, allow_non_loopback)?;
        if !matches!(
            request.endpoint.provider_kind,
            RelayManagedEndpointProviderKind::Manual
                | RelayManagedEndpointProviderKind::CloudflareTunnel
        ) {
            return Err(Error::Configuration);
        }
        crate::http::secure_origin(&request.relay_issuer)?;
        let now = self.clock.now_ms().div_euclid(1000);
        // Only borrow the challenge clock. The relay still authenticates and spends it.
        let iat = crypto::unverified(&request.challenge)
            .ok()
            .and_then(|v| v["iat"].as_i64())
            .filter(|v| *v > 0 && *v <= 9_007_199_254_740_991)
            .unwrap_or(now);
        let mut payload = self.claims(&request.relay_issuer, 300);
        payload["iat"] = json!(iat);
        payload["exp"] = json!(iat + 300);
        payload["challenge"] = json!(request.challenge);
        payload["descriptor"] = encode(&self.descriptor)?;
        payload["environmentPublicKey"] = json!(self.signing_key.public_key.trim());
        payload["endpoint"] = encode(&request.endpoint)?;
        payload["origin"] = encode(&request.origin)?;
        payload["scopes"] = if request.endpoint.provider_kind
            == RelayManagedEndpointProviderKind::CloudflareTunnel
        {
            json!(["agent_activity_notifications", "managed_tunnels"])
        } else {
            json!(["agent_activity_notifications"])
        };
        let typed: RelayEnvironmentLinkProofPayload = decode(payload)?;
        self.signing_key.sign(crypto::LINK, &typed)
    }
    /// Caller authenticates the local request and authorizes installing account trust.
    pub async fn install(&self, request: Sensitive<RelayEnvironmentConfigRequest>) -> Result<()> {
        let this = self.clone();
        tokio::spawn(async move { this.install_inner(request).await })
            .await
            .map_err(|_| Error::Storage)?
    }
    async fn install_inner(&self, request: Sensitive<RelayEnvironmentConfigRequest>) -> Result<()> {
        let request = request.into_inner();
        crate::http::secure_origin(&request.relay_url)?;
        let issuer = request
            .relay_issuer
            .flatten()
            .unwrap_or_else(|| request.relay_url.clone());
        crate::http::secure_origin(&issuer)?;
        crypto::validate_public_key(&request.cloud_mint_public_key)?;
        if request.cloud_user_id.trim().is_empty()
            || request.environment_credential.trim().is_empty()
        {
            return Err(Error::Configuration);
        }
        if let Some(runtime) = &request.endpoint_runtime {
            crate::tunnel::validate_runtime(runtime)?;
        }
        let _lock = self.state_lock.lock().await;
        if read_text(self.secrets.as_ref(), USER)
            .await?
            .is_some_and(|user| user != request.cloud_user_id)
        {
            return Err(Error::Identity);
        }
        let mut previous = Vec::new();
        for name in CONFIG_NAMES
            .into_iter()
            .chain(["cloud-endpoint-confirmed-origin"])
        {
            previous.push((name, self.secrets.get(name).await?));
        }
        let mut writes = vec![
            (RELAY_URL, request.relay_url.into_bytes()),
            (RELAY_ISSUER, issuer.into_bytes()),
            (USER, request.cloud_user_id.into_bytes()),
            (CREDENTIAL, request.environment_credential.into_bytes()),
            (MINT_KEY, request.cloud_mint_public_key.into_bytes()),
        ];
        if let Some(runtime) = request.endpoint_runtime {
            writes.push((
                RUNTIME,
                serde_json::to_vec(&runtime).map_err(|_| Error::Protocol)?,
            ));
        }
        let result = async {
            self.secrets
                .remove("cloud-endpoint-confirmed-origin")
                .await?;
            self.secrets.remove(RUNTIME).await?;
            for (name, value) in &writes {
                self.secrets.set(name, value).await?;
            }
            if !writes.iter().any(|(name, _)| *name == RUNTIME) {
                self.secrets.remove(RUNTIME).await?;
            }
            self.secrets.remove(REVOKED).await
        }
        .await;
        if result.is_err() {
            for (name, value) in previous {
                match value {
                    Some(v) => self.secrets.set(name, &v).await?,
                    None => self.secrets.remove(name).await?,
                }
            }
        }
        result
    }
    pub async fn register(&self, origin: RelayManagedEndpointOrigin) -> Result<Registration> {
        crate::direct::validate_tunnel_origin(&origin)?;
        let config = match self.snapshot().await {
            Ok(v) => v,
            Err(Error::NotLinked) => return Ok(Registration::NotLinked),
            Err(e) => return Err(e),
        };
        let Some(runtime) = &config.runtime else {
            return Ok(Registration::NotLinked);
        };
        if runtime.provider_kind != RelayManagedEndpointProviderKind::CloudflareTunnel {
            return Ok(Registration::NotLinked);
        }
        let Some(Some(tunnel)) = &runtime.tunnel_id else {
            return Ok(Registration::RecoveryRequired);
        };
        let mut payload = self.claims(&config.issuer, 60);
        payload["action"] = json!("register");
        payload["cloudUserId"] = json!(config.user);
        payload["tunnelId"] = encode(tunnel)?;
        payload["origin"] = encode(&origin)?;
        let proof = self.signing_key.sign(
            crypto::RECOVERY,
            &decode::<RelayManagedEndpointRegisterProofPayload>(payload)?,
        )?;
        let request = RelayManagedEndpointRecoveryRegistrationRequest {
            cloud_user_id: crate::text(config.user.clone()),
            tunnel_id: tunnel.clone(),
            origin,
            proof: crate::text(proof.expose()),
        };
        let result = self
            .client(&config)?
            .call::<RelayManagedEndpointRecoveryRegistrationResponse>(
                Method::POST,
                &["v1", "environments", self.id(), "tunnel", "recovery"],
                Some(&config.credential),
                Some(&request),
                Duration::from_secs(30),
            )
            .await;
        let response = match result {
            Ok(v) => v,
            Err(Error::Remote {
                rejection: Rejection::Unauthorized,
                ..
            }) => {
                return Ok(if self.mark_revoked(&config.credential).await? {
                    Registration::Revoked
                } else {
                    Registration::Superseded
                });
            }
            Err(e) => return Err(e),
        };
        let _lock = self.state_lock.lock().await;
        if !self.is_current(&config).await? {
            return Ok(Registration::Superseded);
        }
        self.secrets.remove(REVOKED).await?;
        Ok(
            if response.status == RelayManagedEndpointRecoveryStatus::Ready {
                Registration::Ready
            } else {
                Registration::RecoveryRequired
            },
        )
    }
    async fn is_current(&self, config: &Installed) -> Result<bool> {
        let current = match self.installed().await {
            Ok(c) => c,
            Err(Error::NotLinked) => return Ok(false),
            Err(e) => return Err(e),
        };
        Ok(current.credential.expose() == config.credential.expose()
            && current.runtime == config.runtime
            && current.user == config.user
            && current.url == config.url)
    }
    async fn mark_revoked(&self, credential: &Secret) -> Result<bool> {
        let _lock = self.state_lock.lock().await;
        if read_text(self.secrets.as_ref(), CREDENTIAL)
            .await?
            .as_deref()
            != Some(credential.expose())
        {
            return Ok(false);
        }
        self.secrets.set(REVOKED, b"true").await?;
        Ok(true)
    }
    pub async fn recover(
        &self,
        origin: RelayManagedEndpointOrigin,
        expected: Option<&Sensitive<RelayManagedEndpointRuntimeConfig>>,
    ) -> Result<Sensitive<RelayManagedEndpointRecoveryResponse>> {
        crate::direct::validate_tunnel_origin(&origin)?;
        let config = self.snapshot().await?;
        if expected.is_some_and(|v| config.runtime.as_ref() != Some(v.expose())) {
            return Err(Error::Superseded);
        }
        let mut payload = self.claims(&config.issuer, 60);
        payload["action"] = json!("recover");
        payload["cloudUserId"] = json!(config.user);
        payload["origin"] = encode(&origin)?;
        let proof = self.signing_key.sign(
            crypto::RECOVERY,
            &decode::<RelayManagedEndpointRecoverProofPayload>(payload)?,
        )?;
        let request = RelayManagedEndpointRecoveryRequest {
            cloud_user_id: crate::text(config.user.clone()),
            origin,
            proof: crate::text(proof.expose()),
        };
        let result = self
            .client(&config)?
            .call::<RelayManagedEndpointRecoveryResponse>(
                Method::POST,
                &["v1", "environments", self.id(), "tunnel"],
                Some(&config.credential),
                Some(&request),
                Duration::from_secs(120),
            )
            .await;
        let response = match result {
            Ok(v) => v,
            Err(e) => {
                if matches!(
                    e,
                    Error::Remote {
                        rejection: Rejection::Unauthorized,
                        ..
                    }
                ) {
                    self.mark_revoked(&config.credential).await?;
                }
                return Err(e);
            }
        };
        crate::tunnel::validate_runtime(&response.endpoint_runtime)?;
        if response.endpoint_runtime.provider_kind
            != RelayManagedEndpointProviderKind::CloudflareTunnel
        {
            return Err(Error::Protocol);
        }
        let _lock = self.state_lock.lock().await;
        if !self.is_current(&config).await? {
            return Err(Error::Superseded);
        }
        self.secrets
            .set(
                RUNTIME,
                &serde_json::to_vec(&response.endpoint_runtime).map_err(|_| Error::Protocol)?,
            )
            .await?;
        self.secrets
            .remove("cloud-endpoint-confirmed-origin")
            .await?;
        self.secrets.remove(REVOKED).await?;
        Ok(Sensitive::new(response))
    }
    pub async fn recover_serialized(
        &self,
        origin: RelayManagedEndpointOrigin,
        expected: Sensitive<RelayManagedEndpointRuntimeConfig>,
    ) -> Result<Sensitive<RelayManagedEndpointRecoveryResponse>> {
        let mut cooldown = self.recovery_lock.lock().await;
        tokio::time::sleep(Duration::from_millis(cooldown.wait_ms(self.clock.now_ms()))).await;
        let policy =
            crate::retry::RetryPolicy::new(crate::retry::RetryMode::Recovery, self.clock.now_ms());
        let result = crate::retry::retry(self.clock.clone(), policy, || {
            self.recover(origin.clone(), Some(&expected))
        })
        .await;
        cooldown.finished(self.clock.now_ms());
        result
    }
    pub async fn unlink(&self) -> Result<()> {
        let this = self.clone();
        tokio::spawn(async move { this.unlink_inner().await })
            .await
            .map_err(|_| Error::Storage)?
    }
    pub async fn set_desired_link(&self, mode: Option<DesiredLinkMode>) -> Result<()> {
        let _lock = self.state_lock.lock().await;
        match mode {
            Some(mode) => {
                self.secrets
                    .set(
                        "cloud-cli-desired-link",
                        if mode == DesiredLinkMode::Managed {
                            b"managed"
                        } else {
                            b"publish_only"
                        },
                    )
                    .await
            }
            None => self.secrets.remove("cloud-cli-desired-link").await,
        }
    }
    pub async fn desired_link(&self) -> Result<Option<DesiredLinkMode>> {
        Ok(read_text(self.secrets.as_ref(), "cloud-cli-desired-link")
            .await?
            .map(|v| {
                if v == "publish_only" {
                    DesiredLinkMode::PublishOnly
                } else {
                    DesiredLinkMode::Managed
                }
            }))
    }
    /// Persist only after this exact runtime has been started for the registered origin.
    pub async fn confirm_origin(
        &self,
        config: &Sensitive<RelayManagedEndpointRuntimeConfig>,
        origin: RelayManagedEndpointOrigin,
    ) -> Result<()> {
        crate::direct::validate_tunnel_origin(&origin)?;
        let _lock = self.state_lock.lock().await;
        let installed = self.installed().await?;
        if installed.runtime.as_ref() != Some(config.expose()) {
            return Err(Error::Superseded);
        }
        let value = serde_json::to_vec(&json!({"config":config.expose(),"origin":origin}))
            .map_err(|_| Error::Protocol)?;
        self.secrets
            .set("cloud-endpoint-confirmed-origin", &value)
            .await
    }
    /// Unreachable-relay fallback can reuse only the same runtime and confirmed local port.
    pub async fn confirmed_runtime(
        &self,
        origin: &RelayManagedEndpointOrigin,
    ) -> Result<Option<Sensitive<RelayManagedEndpointRuntimeConfig>>> {
        let _lock = self.state_lock.lock().await;
        let installed = match self.installed().await {
            Ok(c) => c,
            Err(Error::NotLinked) => return Ok(None),
            Err(e) => return Err(e),
        };
        let Some(runtime) = installed.runtime else {
            return Ok(None);
        };
        let Some(marker) =
            read_text(self.secrets.as_ref(), "cloud-endpoint-confirmed-origin").await?
        else {
            return Ok(None);
        };
        let marker: Value = serde_json::from_str(&marker).map_err(|_| Error::Storage)?;
        if marker["config"] != encode(&runtime)? || marker["origin"] != encode(origin)? {
            return Ok(None);
        }
        Ok(Some(Sensitive::new(runtime)))
    }
    pub async fn release_managed_tunnel(&self, account_token: Option<&Secret>) -> Result<bool> {
        if self.desired_link().await? != Some(DesiredLinkMode::Managed) {
            return Ok(false);
        }
        let Some(token) = account_token else {
            return Ok(false);
        };
        let config = self.snapshot().await?;
        if config
            .runtime
            .as_ref()
            .is_none_or(|c| c.provider_kind != RelayManagedEndpointProviderKind::CloudflareTunnel)
        {
            return Ok(false);
        }
        let response: RelayOkResponse = self
            .client(&config)?
            .call(
                Method::DELETE,
                &["v1", "client", "environment-links", self.id(), "tunnel"],
                Some(token),
                None::<&Value>,
                Duration::from_secs(10),
            )
            .await?;
        if !response.ok {
            return Ok(false);
        }
        let _lock = self.state_lock.lock().await;
        if self.is_current(&config).await? {
            self.secrets.remove(RUNTIME).await?;
            self.secrets
                .remove("cloud-endpoint-confirmed-origin")
                .await?;
        }
        Ok(true)
    }
    async fn unlink_inner(&self) -> Result<()> {
        let _lock = self.state_lock.lock().await;
        for name in CONFIG_NAMES.into_iter().chain([
            "cloud-endpoint-confirmed-origin",
            "cloud-cli-desired-link",
            PUBLISH,
        ]) {
            self.secrets.remove(name).await?;
        }
        Ok(())
    }
    pub async fn health(&self, proof: Secret) -> Result<Sensitive<RelayEnvironmentHealthResponse>> {
        let _lock = self.state_lock.lock().await;
        let config = self.installed().await?;
        let now = self.clock.now_ms();
        let payload: RelayCloudEnvironmentHealthProofPayload = crypto::verify(
            &config.mint_key,
            &proof,
            crypto::HEALTH_REQUEST,
            crypto::normalize_issuer(&config.issuer),
            &format!("t3-env:{}", self.id()),
            now.div_euclid(1000),
        )?;
        if payload.environment_id.0 != self.id()
            || payload.sub.0.as_str() != config.user
            || !crypto::bounded(payload.iat, payload.exp, now.div_euclid(1000))
            || payload.scope != [RelayStatusScope::EnvironmentStatus]
        {
            return Err(Error::Proof);
        }
        consume_replay(
            self.secrets.as_ref(),
            "health",
            payload.jti.0.as_str(),
            payload.nonce.0.as_str(),
            &iso(now)?,
        )
        .await?;
        let mut response = self.claims(&config.issuer, 300);
        response["requestNonce"] = encode(&payload.nonce)?;
        response["status"] = json!("online");
        response["descriptor"] = encode(&self.descriptor)?;
        response["checkedAt"] = json!(iso(now)?);
        let response: RelayEnvironmentHealthResponseProofPayload = decode(response)?;
        let signed = self.signing_key.sign(crypto::HEALTH_RESPONSE, &response)?;
        Ok(Sensitive::new(RelayEnvironmentHealthResponse {
            environment_id: self.descriptor.environment_id.clone(),
            status: RelayOnlineStatus::Online,
            descriptor: self.descriptor.clone(),
            checked_at: crate::text(iso(now)?),
            proof: crate::text(signed.expose()),
        }))
    }
    pub async fn mint(
        &self,
        proof: Secret,
        issuer: &dyn CredentialIssuer,
    ) -> Result<Sensitive<RelayEnvironmentMintResponse>> {
        let _lock = self.state_lock.lock().await;
        let config = self.installed().await?;
        let now = self.clock.now_ms();
        let payload: RelayCloudMintCredentialProofPayload = crypto::verify(
            &config.mint_key,
            &proof,
            crypto::MINT_REQUEST,
            crypto::normalize_issuer(&config.issuer),
            &format!("t3-env:{}", self.id()),
            now.div_euclid(1000),
        )?;
        if payload.environment_id.0 != self.id()
            || payload.sub.0.as_str() != config.user
            || payload.cnf.jkt != payload.client_proof_key_thumbprint
            || !crypto::bounded(payload.iat, payload.exp, now.div_euclid(1000))
            || payload.scope != [RelayConnectScope::EnvironmentConnect]
        {
            return Err(Error::Proof);
        }
        consume_replay(
            self.secrets.as_ref(),
            "mint",
            payload.jti.0.as_str(),
            payload.nonce.0.as_str(),
            &iso(now)?,
        )
        .await?;
        let issued = issuer
            .issue_cloud_pairing(payload.client_proof_key_thumbprint.0.as_str())
            .await?;
        if issued.expires_at_ms <= now || issued.expires_at_ms > now.saturating_add(600_000) {
            return Err(Error::Protocol);
        }
        let mut response = self.claims(&config.issuer, 120);
        response["exp"] = json!(issued.expires_at_ms.div_euclid(1000));
        response["clientProofKeyThumbprint"] = encode(&payload.client_proof_key_thumbprint)?;
        response["requestNonce"] = encode(&payload.nonce)?;
        response["credential"] = json!(issued.credential.expose());
        let response: RelayEnvironmentMintResponseProofPayload = decode(response)?;
        let signed = self.signing_key.sign(crypto::MINT_RESPONSE, &response)?;
        Ok(Sensitive::new(RelayEnvironmentMintResponse {
            credential: crate::text(issued.credential.expose()),
            expires_at: crate::text(iso(issued.expires_at_ms)?),
            proof: crate::text(signed.expose()),
        }))
    }
    pub async fn publish_activity(
        &self,
        thread: &str,
        mut state: Option<RelayAgentActivityState>,
    ) -> Result<Option<RelayPublishResponse>> {
        if read_text(self.secrets.as_ref(), PUBLISH).await?.as_deref() != Some("true") {
            return Ok(None);
        }
        let config = self.snapshot().await?;
        if state
            .as_ref()
            .is_some_and(|s| s.environment_id.0 != self.id() || s.thread_id.0 != thread)
        {
            return Err(Error::Identity);
        }
        if let Some(state) = &mut state {
            let detail = if state.phase == RelayAgentAwarenessPhase::Failed {
                Some("The agent run failed.".to_owned())
            } else {
                state.detail.take().flatten().map(|detail| detail.0)
            };
            state.detail = detail.and_then(|detail| {
                let bounded = detail
                    .trim()
                    .chars()
                    .scan(0, |units, c| {
                        *units += c.len_utf16();
                        (*units <= 160).then_some(c)
                    })
                    .collect::<String>();
                let bounded = bounded.trim();
                (!bounded.is_empty()).then(|| Some(crate::text(bounded)))
            });
        }
        let mut payload = self.claims(&config.issuer, 300);
        payload["threadId"] = json!(thread);
        payload["state"] = encode(&state)?;
        let proof = self.signing_key.sign(
            crypto::ACTIVITY,
            &decode::<RelayAgentActivityPublishProofPayload>(payload)?,
        )?;
        let request = RelayAgentActivityPublishRequest {
            state,
            proof: crate::text(proof.expose()),
        };
        self.client(&config)?
            .call(
                Method::POST,
                &[
                    "v1",
                    "environments",
                    self.id(),
                    "threads",
                    thread,
                    "agent-activity",
                ],
                Some(&config.credential),
                Some(&request),
                Duration::from_secs(10),
            )
            .await
            .map(Some)
    }
    pub async fn preferences(&self, publish: bool, hold: bool) -> Result<()> {
        self.update_preferences(
            t3_contracts::environment_http::EnvironmentCloudPreferencesRequest {
                publish_agent_activity: publish,
                hold_webhooks_while_offline: Some(Some(hold)),
            },
        )
        .await
    }
    pub async fn update_preferences(
        &self,
        input: t3_contracts::environment_http::EnvironmentCloudPreferencesRequest,
    ) -> Result<()> {
        let this = self.clone();
        tokio::spawn(async move {
            this.preferences_inner(
                input.publish_agent_activity,
                input.hold_webhooks_while_offline.flatten(),
            )
            .await
        })
        .await
        .map_err(|_| Error::Storage)?
    }
    async fn preferences_inner(&self, publish: bool, hold: Option<bool>) -> Result<()> {
        let _lock = self.state_lock.lock().await;
        let old_hold = self.secrets.get(HOLD).await?;
        let old_publish = self.secrets.get(PUBLISH).await?;
        let result = async {
            self.secrets
                .set(PUBLISH, if publish { b"true" } else { b"false" })
                .await?;
            let Some(hold) = hold else {
                return Ok(());
            };
            let config = self.installed().await?;
            let request = RelayEnvironmentLinkPreferencesRequest {
                hold_webhooks_while_offline: hold,
            };
            let _: RelayEnvironmentLinkPreferencesRequest = self
                .client(&config)?
                .call(
                    Method::POST,
                    &["v1", "environments", self.id(), "link-preferences"],
                    Some(&config.credential),
                    Some(&request),
                    Duration::from_secs(10),
                )
                .await?;
            if let Err(error) = self
                .secrets
                .set(HOLD, if hold { b"true" } else { b"false" })
                .await
            {
                let rollback = RelayEnvironmentLinkPreferencesRequest {
                    hold_webhooks_while_offline: old_hold.as_deref() == Some(b"true"),
                };
                let _ = self
                    .client(&config)?
                    .call::<RelayEnvironmentLinkPreferencesRequest>(
                        Method::POST,
                        &["v1", "environments", self.id(), "link-preferences"],
                        Some(&config.credential),
                        Some(&rollback),
                        Duration::from_secs(10),
                    )
                    .await;
                return Err(error);
            }
            Ok(())
        }
        .await;
        if result.is_err() {
            for (name, value) in [(HOLD, old_hold), (PUBLISH, old_publish)] {
                match value {
                    Some(v) => self.secrets.set(name, &v).await?,
                    None => self.secrets.remove(name).await?,
                }
            }
        }
        result
    }
    pub async fn wake_hooks(&self) -> Result<bool> {
        if read_text(self.secrets.as_ref(), HOLD).await?.as_deref() != Some("true") {
            return Ok(false);
        }
        let config = self.snapshot().await?;
        let response: RelayWakeHeldHooksResponse = self
            .client(&config)?
            .call(
                Method::POST,
                &["v1", "environments", self.id(), "hooks", "wake"],
                Some(&config.credential),
                Some(&json!({})),
                Duration::from_secs(10),
            )
            .await?;
        Ok(response.pending)
    }
    /// Authenticity metadata only: the ordinary hook path token/provider signature still applies.
    pub async fn verify_hook(
        &self,
        proof: Secret,
        hook_id: &str,
        delivery_id: &str,
        received_at: &str,
    ) -> Result<RelayHookDeliveryProofPayload> {
        let config = self.snapshot().await?;
        let payload: RelayHookDeliveryProofPayload = crypto::verify_with_max_age(
            &config.mint_key,
            &proof,
            crypto::HOOK,
            crypto::normalize_issuer(&config.issuer),
            &format!("t3-env:{}", self.id()),
            self.clock.now_ms().div_euclid(1000),
            25 * 60 * 60,
        )?;
        if payload.environment_id.0 != self.id()
            || payload.hook_id.0.as_str() != hook_id
            || payload.delivery_id.0 != delivery_id
            || payload.received_at.0 != received_at
        {
            return Err(Error::Proof);
        }
        Ok(payload)
    }
}

/// Registration/refresh obtains the opaque environment bearer only from the existing relay.
pub async fn register_account(
    client: &Client,
    account_token: &Secret,
    link: &CloudLink,
    mut request: Sensitive<RelayLinkProofRequest>,
    flags: RelayEnvironmentLinkChallengeRequest,
    local_request_url: &str,
) -> Result<Sensitive<RelayEnvironmentLinkResponse>> {
    let challenge: RelayEnvironmentLinkChallengeResponse = client
        .call(
            Method::POST,
            &["v1", "client", "environment-link-challenges"],
            Some(account_token),
            Some(&flags),
            Duration::from_secs(30),
        )
        .await?;
    request.expose_mut().challenge = challenge.challenge.0.as_str().to_owned();
    let proof = link.link_proof(&request, local_request_url, true)?;
    let body = RelayEnvironmentLinkRequest {
        device_id: None,
        proof: crate::text(proof.expose()),
        notifications_enabled: flags.notifications_enabled,
        live_activities_enabled: flags.live_activities_enabled,
        managed_tunnels_enabled: flags.managed_tunnels_enabled,
    };
    let response: RelayEnvironmentLinkResponse = client
        .call(
            Method::POST,
            &["v1", "client", "environment-links"],
            Some(account_token),
            Some(&body),
            Duration::from_secs(120),
        )
        .await?;
    if !response.ok || response.environment_id != link.descriptor.environment_id {
        return Err(Error::Identity);
    }
    link.install(Sensitive::new(RelayEnvironmentConfigRequest {
        relay_url: client.base.origin().ascii_serialization(),
        relay_issuer: Some(Some(response.relay_issuer.0.as_str().to_owned())),
        cloud_user_id: response.cloud_user_id.0.as_str().to_owned(),
        environment_credential: response.environment_credential.0.as_str().to_owned(),
        cloud_mint_public_key: response.cloud_mint_public_key.0.as_str().to_owned(),
        endpoint_runtime: response.endpoint_runtime.clone(),
    }))
    .await?;
    Ok(Sensitive::new(response))
}
