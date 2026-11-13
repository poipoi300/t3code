use crate::{
    AuthConfig, Error, Result, ServerSecretStore,
    authorization::STANDARD_SCOPES,
    clock::{Clock, epoch, expires_after, iso},
    error::CredentialFailure as F,
    utils::{
        PayloadSigner, b64, default_metadata, hex, legacy_cookie_name, random_uuid,
        session_cookie_name, sha256, unb64,
    },
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use t3_contracts::{
    auth::{
        AuthClientMetadata, AuthClientSession, AuthEnvironmentScope, AuthWebSocketTicketResult,
        AuthWebSocketTicketUse, ServerAuthSessionMethod,
    },
    base_schemas::{AuthSessionId, TrimmedNonEmptyString},
    provider_policy::RuntimeMode,
    wire::WireDateTime,
};
use t3_persistence::{
    Database,
    rusqlite::{self, OptionalExtension, params},
};
use tokio::sync::{Mutex, mpsc, watch};

pub const SESSION_TTL_MS: i64 = 30 * 86_400_000;
pub const WEBSOCKET_TTL_MS: i64 = 300_000;
pub const DEV_EXPIRY_MS: i64 = 253_402_300_799_999;

#[derive(Clone)]
pub struct ReusableDevAuth {
    pub credential: String,
    pub session_id: AuthSessionId,
    pub cookie_name: String,
    hash: Vec<u8>,
}
impl ReusableDevAuth {
    pub fn resolve(config: &AuthConfig) -> Option<Self> {
        if config.desktop || !config.development() {
            return None;
        }
        let token = config.dev_auth_token.as_ref().filter(|t| !t.is_empty())?;
        let hash = sha256(token);
        let id = hex(&hash);
        Some(Self {
            credential: token.clone(),
            session_id: AuthSessionId(format!("dev-auth-{id}")),
            cookie_name: format!("t3_dev_session_{id}"),
            hash,
        })
    }
    pub fn matches(&self, token: &str) -> bool {
        use subtle::ConstantTimeEq;
        bool::from(self.hash.ct_eq(&sha256(token)))
    }
}

#[derive(Clone)]
pub struct SessionStore {
    inner: Arc<Inner>,
}
struct Inner {
    db: Database,
    clock: Arc<dyn Clock>,
    signer: PayloadSigner,
    cookie_name: String,
    legacy_cookie_name: Option<String>,
    dev: Option<ReusableDevAuth>,
    state: Mutex<State>,
    changed: watch::Sender<u64>,
}
#[derive(Default)]
struct State {
    connected: HashMap<String, usize>,
    latest: HashMap<String, String>,
    subscribers: Vec<mpsc::UnboundedSender<SessionChange>>,
}
#[derive(Debug, Clone)]
pub enum SessionChange {
    ClientUpserted(Arc<AuthClientSession>),
    ClientRemoved(AuthSessionId),
}
#[derive(Clone)]
pub struct SessionRecord {
    pub session_id: AuthSessionId,
    pub subject: String,
    pub scopes: Vec<AuthEnvironmentScope>,
    pub method: ServerAuthSessionMethod,
    pub client: AuthClientMetadata,
    pub issued_at: WireDateTime,
    pub expires_at: WireDateTime,
    pub last_connected_at: Option<WireDateTime>,
    pub revoked_at: Option<WireDateTime>,
}
#[derive(Clone)]
pub struct IssuedSession {
    pub session_id: AuthSessionId,
    pub token: String,
    pub method: ServerAuthSessionMethod,
    pub client: AuthClientMetadata,
    pub expires_at: WireDateTime,
    pub scopes: Vec<AuthEnvironmentScope>,
    pub proof_key_thumbprint: Option<String>,
}
#[derive(Clone)]
pub struct VerifiedSession {
    pub session_id: AuthSessionId,
    pub token: String,
    pub method: ServerAuthSessionMethod,
    pub client: AuthClientMetadata,
    pub expires_at: WireDateTime,
    pub subject: String,
    pub scopes: Vec<AuthEnvironmentScope>,
    pub proof_key_thumbprint: Option<String>,
    pub runtime_mode_ceiling: Option<RuntimeMode>,
    pub peer_link: bool,
    pub audience: Option<AuthWebSocketTicketUse>,
    pub oauth_audience: Option<String>,
    pub resource_indicator: Option<String>,
}
#[derive(Default)]
pub struct IssueSession {
    pub mcp_resource: Option<String>,
    pub ttl_ms: Option<i64>,
    pub subject: Option<String>,
    pub method: Option<ServerAuthSessionMethod>,
    pub scopes: Option<Vec<AuthEnvironmentScope>>,
    pub client: Option<AuthClientMetadata>,
    pub proof_key_thumbprint: Option<String>,
    pub runtime_mode_ceiling: Option<RuntimeMode>,
    pub peer_link: bool,
    pub replace_active_for_subject_and_method: bool,
    pub replace_session_id: Option<AuthSessionId>,
}

#[derive(Serialize, Deserialize)]
struct SessionClaims {
    v: u8,
    kind: String,
    sid: AuthSessionId,
    sub: String,
    scopes: Vec<AuthEnvironmentScope>,
    method: ServerAuthSessionMethod,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    aud: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    resource: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    jkt: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    rtc: Option<RuntimeMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    plk: Option<u8>,
    iat: f64,
    exp: f64,
}
#[derive(Serialize, Deserialize)]
struct WebSocketClaims {
    v: u8,
    kind: String,
    sid: AuthSessionId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    aud: Option<AuthWebSocketTicketUse>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    jkt: Option<String>,
    iat: f64,
    exp: f64,
}
impl SessionStore {
    pub async fn new(
        db: Database,
        secrets: &ServerSecretStore,
        config: AuthConfig,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        config.validate()?;
        let cookie_name = session_cookie_name(&config);
        let legacy_cookie_name = legacy_cookie_name(&config);
        let signer = PayloadSigner::from_store(secrets, "server-signing-key").await?;
        let dev = ReusableDevAuth::resolve(&config);
        let (changed, _) = watch::channel(0);
        let store = Self {
            inner: Arc::new(Inner {
                db,
                signer,
                cookie_name,
                legacy_cookie_name,
                dev,
                clock,
                state: Mutex::new(State::default()),
                changed,
            }),
        };
        if let Some(dev) = &store.inner.dev {
            let mut client = default_metadata();
            client.label = Some(TrimmedNonEmptyString("Reusable dev token".into()));
            let record = SessionRecord {
                session_id: dev.session_id.clone(),
                subject: "reusable-dev-token".into(),
                scopes: STANDARD_SCOPES.to_vec(),
                method: ServerAuthSessionMethod::BrowserSessionCookie,
                client,
                issued_at: iso(store.now_ms())?,
                expires_at: iso(DEV_EXPIRY_MS)?,
                last_connected_at: None,
                revoked_at: None,
            };
            store
                .inner
                .db
                .writer(move |db| {
                    insert(db, &record, true)?;
                    Ok(())
                })
                .await?;
        }
        Ok(store)
    }
    pub fn cookie_name(&self) -> String {
        self.inner.cookie_name.clone()
    }
    pub fn legacy_cookie_name(&self) -> Option<String> {
        self.inner.legacy_cookie_name.clone()
    }
    pub fn dev_auth(&self) -> Option<&ReusableDevAuth> {
        self.inner.dev.as_ref()
    }
    pub fn now_ms(&self) -> i64 {
        self.inner.clock.now_ms()
    }
    pub async fn subscribe(&self) -> mpsc::UnboundedReceiver<SessionChange> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner.state.lock().await.subscribers.push(tx);
        rx
    }
    fn publish(&self, state: &mut State, change: SessionChange) {
        state.subscribers.retain(|s| s.send(change.clone()).is_ok());
        self.inner.changed.send_modify(|v| *v = v.wrapping_add(1));
    }
    pub async fn get(&self, id: &AuthSessionId) -> Result<Option<SessionRecord>> {
        let id = id.0.clone();
        Ok(self
            .inner
            .db
            .reader(move |db| {
                Ok(db
                    .query_row(&format!("{SELECT} WHERE session_id=?1"), [&id], decode_row)
                    .optional()?)
            })
            .await?)
    }
    pub async fn issue(&self, input: IssueSession) -> Result<IssuedSession> {
        let store = self.clone();
        crate::finish_mutation(async move { store.issue_inner(input).await }).await
    }
    async fn issue_inner(&self, input: IssueSession) -> Result<IssuedSession> {
        let audience = input
            .mcp_resource
            .as_deref()
            .map(crate::oauth::canonical_mcp_resource)
            .transpose()?;
        let mut state = self.inner.state.lock().await;
        let now = self.now_ms();
        let expires = now
            .checked_add(input.ttl_ms.unwrap_or(SESSION_TTL_MS))
            .ok_or_else(|| Error::Date("session TTL overflow".into()))?;
        let claims = SessionClaims {
            v: 1,
            kind: "session".into(),
            sid: AuthSessionId(random_uuid()?),
            sub: input.subject.unwrap_or_else(|| "browser".into()),
            scopes: input.scopes.unwrap_or_else(|| STANDARD_SCOPES.to_vec()),
            method: input
                .method
                .unwrap_or(ServerAuthSessionMethod::BrowserSessionCookie),
            aud: audience,
            resource: None,
            jkt: input.proof_key_thumbprint.filter(|s| !s.is_empty()),
            rtc: input.runtime_mode_ceiling,
            plk: input.peer_link.then_some(1),
            iat: now as f64,
            exp: expires as f64,
        };
        let token = self.encode(&claims)?;
        let record = SessionRecord {
            session_id: claims.sid.clone(),
            subject: claims.sub.clone(),
            scopes: claims.scopes.clone(),
            method: claims.method,
            client: input.client.unwrap_or_else(default_metadata),
            issued_at: iso(now)?,
            expires_at: iso(expires)?,
            last_connected_at: None,
            revoked_at: None,
        };
        let stored = record.clone();
        let replace = input.replace_session_id.map(|id| id.0);
        let replacing = replace.is_some() || input.replace_active_for_subject_and_method;
        let removed = self.inner.db.writer(move |db| {
            let tx = db.transaction()?;
            let mut removed = Vec::new();
            if replacing {
                let selector = if replace.is_some() { "session_id" } else { "subject" };
                let method = json_enum(stored.method)?;
                let mut stmt = tx.prepare(&format!("UPDATE auth_sessions SET revoked_at=?1 WHERE {selector}=?2 AND method=?3 AND revoked_at IS NULL AND expires_at>?1 RETURNING session_id"))?;
                removed = stmt.query_map(params![stored.issued_at.0, replace.as_deref().unwrap_or(&stored.subject), method], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?;
            }
            insert(&tx, &stored, false)?; tx.commit()?; Ok(removed)
        }).await?;
        if let Some(jkt) = &claims.jkt {
            state.latest.insert(jkt.clone(), claims.sid.0.clone());
        }
        for id in removed {
            state.connected.remove(&id);
            self.publish(&mut state, SessionChange::ClientRemoved(AuthSessionId(id)));
        }
        self.publish(
            &mut state,
            SessionChange::ClientUpserted(Arc::new(client_session(&record, false))),
        );
        Ok(IssuedSession {
            session_id: record.session_id,
            token,
            method: record.method,
            client: record.client,
            expires_at: record.expires_at,
            scopes: record.scopes,
            proof_key_thumbprint: claims.jkt,
        })
    }
    fn encode(&self, claims: &impl Serialize) -> Result<String> {
        let body = b64(serde_json::to_vec(claims)?);
        Ok(format!("{body}.{}", self.inner.signer.sign(&body)))
    }
    fn decode<T: serde::de::DeserializeOwned>(&self, token: &str, websocket: bool) -> Result<T> {
        let mut parts = token.split('.');
        let body = parts.next().unwrap_or("");
        let signature = parts.next().unwrap_or("");
        if body.is_empty() || signature.is_empty() || parts.next().is_some() {
            return Err(Error::credential(
                if websocket {
                    F::MalformedWebSocketTokenError
                } else {
                    F::MalformedSessionTokenError
                },
                None,
            ));
        }
        if !self.inner.signer.verify(body, signature) {
            return Err(Error::credential(
                if websocket {
                    F::InvalidWebSocketTokenSignatureError
                } else {
                    F::InvalidSessionTokenSignatureError
                },
                None,
            ));
        }
        unb64(body)
            .and_then(|b| serde_json::from_slice(&b).ok())
            .ok_or_else(|| {
                Error::credential(
                    if websocket {
                        F::InvalidWebSocketTokenPayloadError
                    } else {
                        F::InvalidSessionTokenPayloadError
                    },
                    None,
                )
            })
    }
    pub async fn verify(&self, token: &str) -> Result<VerifiedSession> {
        if let Some(dev) = &self.inner.dev
            && dev.matches(token)
        {
            let row = self.get(&dev.session_id).await?.ok_or_else(|| {
                Error::credential(F::UnknownSessionTokenError, Some(&dev.session_id.0))
            })?;
            if let Some(revoked) = &row.revoked_at {
                return Err(revoked_error(
                    F::SessionTokenRevokedError,
                    &row.session_id,
                    revoked,
                ));
            }
            self.check_expiry(
                &row.session_id,
                epoch(&row.expires_at)? as f64,
                F::SessionTokenExpiredError,
            )?;
            return Ok(verified(row, token, None, None, false, None));
        }
        let mut claims: SessionClaims = self.decode(token, false)?;
        claims.sid.0 = claims.sid.0.trim().to_owned();
        if ![1, 2].contains(&claims.v)
            || claims.kind != "session"
            || claims.plk.is_some_and(|p| p != 1)
            || claims.sid.0.trim().is_empty()
        {
            return Err(Error::credential(F::InvalidSessionTokenPayloadError, None));
        }
        self.check_expiry(&claims.sid, claims.exp, F::SessionTokenExpiredError)?;
        let row = self
            .get(&claims.sid)
            .await?
            .ok_or_else(|| Error::credential(F::UnknownSessionTokenError, Some(&claims.sid.0)))?;
        if let Some(revoked) = &row.revoked_at {
            return Err(revoked_error(
                F::SessionTokenRevokedError,
                &row.session_id,
                revoked,
            ));
        }
        let mut result = verified(
            row,
            token,
            claims.jkt.filter(|s| !s.is_empty()),
            claims.rtc,
            claims.plk == Some(1),
            None,
        );
        result.method = claims.method;
        result.subject = claims.sub;
        result.expires_at = iso(claims.exp as i64)?;
        result.oauth_audience = claims.aud;
        result.resource_indicator = claims.resource;
        Ok(result)
    }
    /// Verify an OAuth MCP bearer for one configured public resource and its required permissions.
    pub async fn verify_mcp(
        &self,
        token: &str,
        resource: &str,
        required_scopes: &[AuthEnvironmentScope],
    ) -> Result<VerifiedSession> {
        let resource = crate::oauth::canonical_mcp_resource(resource)?;
        let session = self.verify(token).await?;
        if session.oauth_audience.as_deref() != Some(resource.as_str()) {
            return Err(Error::invalid(
                "MCP token audience does not match the resource.",
            ));
        }
        if session
            .resource_indicator
            .as_ref()
            .is_some_and(|value| value != &resource)
        {
            return Err(Error::invalid(
                "MCP token resource indicator does not match.",
            ));
        }
        if session.subject != "mcp-client"
            || session.method != ServerAuthSessionMethod::BearerAccessToken
            || session.proof_key_thumbprint.is_some()
        {
            return Err(Error::invalid(
                "Only unbound MCP bearer sessions are accepted here.",
            ));
        }
        let row = self.get(&session.session_id).await?.ok_or_else(|| {
            Error::credential(F::UnknownSessionTokenError, Some(&session.session_id.0))
        })?;
        if let Some(revoked) = &row.revoked_at {
            return Err(revoked_error(
                F::SessionTokenRevokedError,
                &row.session_id,
                revoked,
            ));
        }
        self.check_expiry(
            &row.session_id,
            epoch(&row.expires_at)? as f64,
            F::SessionTokenExpiredError,
        )?;
        if row.subject != "mcp-client" || row.method != ServerAuthSessionMethod::BearerAccessToken {
            return Err(Error::invalid(
                "Only persisted MCP bearer sessions are accepted here.",
            ));
        }
        if !row
            .scopes
            .contains(&AuthEnvironmentScope::OrchestrationRead)
            || required_scopes
                .iter()
                .any(|scope| !row.scopes.contains(scope))
        {
            return Err(Error::ScopeNotGranted);
        }
        Ok(VerifiedSession {
            scopes: row.scopes,
            ..session
        })
    }
    fn check_expiry(&self, id: &AuthSessionId, expires: f64, failure: F) -> Result<()> {
        if !expires.is_finite() || expires.abs() > 8.64e15 {
            return Err(Error::credential(
                F::InvalidSessionExpirationClaimError,
                Some(&id.0),
            ));
        }
        if expires <= self.now_ms() as f64 {
            return Err(Error::Credential {
                kind: failure,
                session_id: Some(id.0.clone()),
                expires_at: Some(iso(expires as i64)?.0),
                observed_at: Some(iso(self.now_ms())?.0),
                revoked_at: None,
            });
        }
        Ok(())
    }
    pub async fn issue_websocket_token(
        &self,
        id: &AuthSessionId,
        ttl_ms: Option<i64>,
        audience: Option<AuthWebSocketTicketUse>,
        proof_key_thumbprint: Option<String>,
    ) -> Result<AuthWebSocketTicketResult> {
        let now = self.now_ms();
        let expires = expires_after(now, ttl_ms.unwrap_or(WEBSOCKET_TTL_MS))?;
        let claims = WebSocketClaims {
            v: 1,
            kind: "websocket".into(),
            sid: id.clone(),
            aud: audience,
            jkt: proof_key_thumbprint.filter(|s| !s.is_empty()),
            iat: now as f64,
            exp: expires as f64,
        };
        Ok(AuthWebSocketTicketResult {
            ticket: TrimmedNonEmptyString(self.encode(&claims)?),
            expires_at: iso(expires)?,
        })
    }
    pub async fn verify_websocket_token(&self, token: &str) -> Result<VerifiedSession> {
        let mut claims: WebSocketClaims = self.decode(token, true)?;
        claims.sid.0 = claims.sid.0.trim().to_owned();
        if claims.v != 1 || claims.kind != "websocket" || claims.sid.0.trim().is_empty() {
            return Err(Error::credential(
                F::InvalidWebSocketTokenPayloadError,
                None,
            ));
        }
        if claims.sid.0.starts_with("dev-auth-")
            && self
                .inner
                .dev
                .as_ref()
                .is_none_or(|d| d.session_id != claims.sid)
        {
            return Err(Error::credential(
                F::UnknownWebSocketSessionError,
                Some(&claims.sid.0),
            ));
        }
        self.check_expiry(&claims.sid, claims.exp, F::WebSocketTokenExpiredError)?;
        let row = self.get(&claims.sid).await?.ok_or_else(|| {
            Error::credential(F::UnknownWebSocketSessionError, Some(&claims.sid.0))
        })?;
        self.check_expiry(
            &claims.sid,
            epoch(&row.expires_at)? as f64,
            F::WebSocketSessionExpiredError,
        )?;
        if let Some(revoked) = &row.revoked_at {
            return Err(revoked_error(
                F::WebSocketSessionRevokedError,
                &row.session_id,
                revoked,
            ));
        }
        Ok(verified(
            row,
            token,
            claims.jkt.filter(|s| !s.is_empty()),
            None,
            false,
            claims.aud,
        ))
    }
    pub async fn list_active(&self) -> Result<Vec<AuthClientSession>> {
        let connected = self.inner.state.lock().await.connected.clone();
        let now = iso(self.now_ms())?.0;
        let ids = serde_json::to_string(&connected.keys().collect::<Vec<_>>())?;
        let rows = self.inner.db.reader(move |db| {
            let mut statement = db.prepare(&format!("{SELECT} WHERE revoked_at IS NULL AND (expires_at>?1 OR session_id IN (SELECT value FROM json_each(?2))) ORDER BY issued_at DESC,session_id DESC"))?;
            Ok(statement.query_map(params![now, ids], decode_row)?.collect::<rusqlite::Result<Vec<_>>>()?)
        }).await?;
        Ok(rows
            .into_iter()
            .map(|row| client_session(&row, connected.contains_key(&row.session_id.0)))
            .collect())
    }
    pub async fn revoke(&self, id: &AuthSessionId) -> Result<bool> {
        let store = self.clone();
        let id = id.clone();
        crate::finish_mutation(async move { store.revoke_inner(&id).await }).await
    }
    async fn revoke_inner(&self, id: &AuthSessionId) -> Result<bool> {
        let mut state = self.inner.state.lock().await;
        let id_string = id.0.clone();
        let now = iso(self.now_ms())?.0;
        let changed = self
            .inner
            .db
            .writer(move |db| {
                Ok(db.execute(
                    "UPDATE auth_sessions SET revoked_at=?1
                 WHERE session_id=?2 AND revoked_at IS NULL",
                    params![now, id_string],
                )? > 0)
            })
            .await?;
        if changed {
            state.connected.remove(&id.0);
            self.publish(&mut state, SessionChange::ClientRemoved(id.clone()));
        }
        Ok(changed)
    }
    pub async fn revoke_all_except(&self, id: &AuthSessionId) -> Result<usize> {
        let store = self.clone();
        let id = id.clone();
        crate::finish_mutation(async move { store.revoke_all_except_inner(&id).await }).await
    }
    async fn revoke_all_except_inner(&self, id: &AuthSessionId) -> Result<usize> {
        let mut state = self.inner.state.lock().await;
        let id_string = id.0.clone();
        let now = iso(self.now_ms())?.0;
        let removed = self
            .inner
            .db
            .writer(move |db| {
                let mut stmt = db.prepare(
                    "UPDATE auth_sessions SET revoked_at=?1
                 WHERE session_id<>?2 AND revoked_at IS NULL
                 RETURNING session_id",
                )?;
                Ok(stmt
                    .query_map(params![now, id_string], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .await?;
        let count = removed.len();
        for id in removed {
            state.connected.remove(&id);
            self.publish(&mut state, SessionChange::ClientRemoved(AuthSessionId(id)));
        }
        Ok(count)
    }
    pub async fn mark_connected(&self, id: &AuthSessionId) -> Result<()> {
        let store = self.clone();
        let id = id.clone();
        crate::finish_mutation(async move { store.mark_connected_inner(&id).await }).await
    }
    async fn mark_connected_inner(&self, id: &AuthSessionId) -> Result<()> {
        let mut state = self.inner.state.lock().await;
        let count = state.connected.entry(id.0.clone()).or_default();
        let first = *count == 0;
        *count += 1;
        if first {
            let now = iso(self.now_ms())?.0;
            let id = id.0.clone();
            self.inner
                .db
                .writer(move |db| {
                    db.execute(
                        "UPDATE auth_sessions SET last_connected_at=?1
                     WHERE session_id=?2 AND revoked_at IS NULL",
                        params![now, id],
                    )?;
                    Ok(())
                })
                .await?;
        }
        if let Some(row) = self.get(id).await?
            && row.revoked_at.is_none()
        {
            self.publish(
                &mut state,
                SessionChange::ClientUpserted(Arc::new(client_session(&row, true))),
            );
        }
        Ok(())
    }
    pub async fn mark_disconnected(&self, id: &AuthSessionId) -> Result<()> {
        let store = self.clone();
        let id = id.clone();
        crate::finish_mutation(async move { store.mark_disconnected_inner(&id).await }).await
    }
    async fn mark_disconnected_inner(&self, id: &AuthSessionId) -> Result<()> {
        let mut state = self.inner.state.lock().await;
        if let Some(count) = state.connected.get_mut(&id.0) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.connected.remove(&id.0);
            }
        }
        let connected = state.connected.contains_key(&id.0);
        if let Some(row) = self.get(id).await?
            && row.revoked_at.is_none()
            && (connected || epoch(&row.expires_at)? > self.now_ms())
        {
            self.publish(
                &mut state,
                SessionChange::ClientUpserted(Arc::new(client_session(&row, connected))),
            );
        } else {
            self.publish(&mut state, SessionChange::ClientRemoved(id.clone()));
        }
        Ok(())
    }
    pub async fn watch_invalidation(
        &self,
        id: &AuthSessionId,
        proof_key: Option<&str>,
    ) -> Result<SessionInvalidation> {
        let changes = self.inner.changed.subscribe();
        let original = self.get(id).await?;
        Ok(SessionInvalidation {
            store: self.clone(),
            changes,
            chain: vec![id.clone()],
            current: original.clone(),
            original,
            proof_key: proof_key.map(str::to_owned),
        })
    }
    pub async fn await_invalidation(
        &self,
        id: &AuthSessionId,
        proof_key: Option<&str>,
    ) -> Result<()> {
        self.watch_invalidation(id, proof_key).await?.wait().await
    }
}
/// A live socket's renewal chain. Subscribe before the initial row read to close the upgrade race.
pub struct SessionInvalidation {
    store: SessionStore,
    changes: watch::Receiver<u64>,
    original: Option<SessionRecord>,
    current: Option<SessionRecord>,
    chain: Vec<AuthSessionId>,
    proof_key: Option<String>,
}
impl SessionInvalidation {
    /// None means invalidated; otherwise returns the next credential deadline in epoch milliseconds.
    pub async fn next_deadline(&mut self) -> Result<Option<i64>> {
        let Some(original) = &self.original else {
            return Ok(None);
        };
        loop {
            for id in &self.chain {
                if self
                    .store
                    .get(id)
                    .await?
                    .is_none_or(|r| r.revoked_at.is_some())
                {
                    return Ok(None);
                }
            }
            let Some(current) = &self.current else {
                return Ok(None);
            };
            let deadline = epoch(&current.expires_at)?;
            if deadline > self.store.now_ms() {
                return Ok(Some(deadline));
            }
            let successor = {
                let state = self.store.inner.state.lock().await;
                self.proof_key
                    .as_ref()
                    .and_then(|k| state.latest.get(k).cloned())
            };
            let Some(successor) = successor else {
                return Ok(None);
            };
            if successor == current.session_id.0 {
                return Ok(None);
            }
            let Some(row) = self.store.get(&AuthSessionId(successor)).await? else {
                return Ok(None);
            };
            if row.subject != original.subject
                || !original.scopes.iter().all(|s| row.scopes.contains(s))
            {
                return Ok(None);
            }
            self.chain.push(row.session_id.clone());
            self.current = Some(row);
        }
    }
    pub async fn wait(mut self) -> Result<()> {
        while let Some(deadline) = self.next_deadline().await? {
            tokio::select! {
                result = self.changes.changed() => { result.map_err(|_| Error::EventStreamClosed)?; },
                () = self.store.inner.clock.wait_until(deadline.min(self.store.now_ms().saturating_add(1000))) => {}
            }
        }
        Ok(())
    }
}

fn verified(
    row: SessionRecord,
    token: &str,
    proof_key_thumbprint: Option<String>,
    runtime_mode_ceiling: Option<RuntimeMode>,
    peer_link: bool,
    audience: Option<AuthWebSocketTicketUse>,
) -> VerifiedSession {
    VerifiedSession {
        session_id: row.session_id,
        token: token.into(),
        method: row.method,
        client: row.client,
        expires_at: row.expires_at,
        subject: row.subject,
        scopes: row.scopes,
        proof_key_thumbprint,
        runtime_mode_ceiling,
        peer_link,
        audience,
        oauth_audience: None,
        resource_indicator: None,
    }
}
fn revoked_error(kind: F, id: &AuthSessionId, revoked: &WireDateTime) -> Error {
    Error::Credential {
        kind,
        session_id: Some(id.0.clone()),
        expires_at: None,
        observed_at: None,
        revoked_at: Some(revoked.0.clone()),
    }
}
fn client_session(row: &SessionRecord, connected: bool) -> AuthClientSession {
    AuthClientSession {
        session_id: row.session_id.clone(),
        subject: TrimmedNonEmptyString(row.subject.clone()),
        scopes: row.scopes.clone(),
        permissions: None,
        method: row.method,
        client: row.client.clone(),
        issued_at: row.issued_at.clone(),
        expires_at: row.expires_at.clone(),
        last_connected_at: row.last_connected_at.clone(),
        connected,
        current: false,
    }
}
pub(crate) fn decode_json<T: serde::de::DeserializeOwned>(value: &str) -> rusqlite::Result<T> {
    serde_json::from_str(value).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}
pub(crate) fn db_date(value: String) -> rusqlite::Result<WireDateTime> {
    let date = WireDateTime(value);
    epoch(&date).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e.to_string(),
            )),
        )
    })?;
    Ok(date)
}
fn json_enum(value: impl Serialize) -> rusqlite::Result<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or(rusqlite::Error::InvalidQuery)
}
fn insert(db: &rusqlite::Connection, record: &SessionRecord, ignore: bool) -> rusqlite::Result<()> {
    let c = &record.client;
    let scopes =
        serde_json::to_string(&record.scopes).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let conflict = if ignore {
        "ON CONFLICT(session_id) DO NOTHING"
    } else {
        ""
    };
    db.execute(
        &format!(
            "INSERT INTO auth_sessions(
            session_id, subject, scopes, method, client_label, client_ip_address,
            client_user_agent, client_device_type, client_os, client_browser,
            issued_at, expires_at, revoked_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,NULL) {conflict}"
        ),
        params![
            record.session_id.0,
            record.subject,
            scopes,
            json_enum(record.method)?,
            c.label.as_ref().map(|s| &s.0),
            c.ip_address.as_ref().map(|s| &s.0),
            c.user_agent.as_ref().map(|s| &s.0),
            json_enum(c.device_type)?,
            c.os.as_ref().map(|s| &s.0),
            c.browser.as_ref().map(|s| &s.0),
            record.issued_at.0,
            record.expires_at.0
        ],
    )?;
    Ok(())
}
const SELECT: &str = "SELECT session_id, subject, scopes, method, client_label, client_ip_address, client_user_agent,
             client_device_type, client_os, client_browser, issued_at, expires_at, last_connected_at, revoked_at
             FROM auth_sessions";
fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    let opt = |i| -> rusqlite::Result<Option<TrimmedNonEmptyString>> {
        Ok(row
            .get::<_, Option<String>>(i)?
            .filter(|s| !s.is_empty())
            .map(TrimmedNonEmptyString))
    };
    Ok(SessionRecord {
        session_id: AuthSessionId(row.get(0)?),
        subject: row.get(1)?,
        scopes: decode_json(&row.get::<_, String>(2)?)?,
        method: decode_enum(&row.get::<_, String>(3)?)?,
        client: AuthClientMetadata {
            label: opt(4)?,
            ip_address: opt(5)?,
            user_agent: opt(6)?,
            device_type: decode_enum(&row.get::<_, String>(7)?)?,
            os: opt(8)?,
            browser: opt(9)?,
        },
        issued_at: db_date(row.get(10)?)?,
        expires_at: db_date(row.get(11)?)?,
        last_connected_at: row.get::<_, Option<String>>(12)?.map(db_date).transpose()?,
        revoked_at: row.get::<_, Option<String>>(13)?.map(db_date).transpose()?,
    })
}

pub(crate) fn decode_enum<T: serde::de::DeserializeOwned>(value: &str) -> rusqlite::Result<T> {
    T::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(value))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })
}
