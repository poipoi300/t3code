use super::persistence::{Database, service_error};
use axum::http::{HeaderMap, Uri};
use serde_json::Value;
use std::sync::Arc;
pub use t3_auth::session::VerifiedSession;
use t3_auth::{
    AuthConfig, EnvironmentAuth, ServerSecretStore, authorization,
    clock::{Clock, SystemClock},
    oauth::{AuthorizationError, AuthorizationRequest, McpOAuth, OAuthUrls},
};
pub use t3_auth::{Error, Request};
use t3_contracts::{auth::*, base_schemas::AuthSessionId};

pub struct AuthService {
    inner: EnvironmentAuth,
    mcp: McpOAuth,
    /// Whether this machine accepts peer links (Settings → Labs). Registering a peer-link client
    /// is refused while it is false; nothing flips it yet because that setting is not assembled
    /// in this server.
    links_enabled: tokio::sync::watch::Sender<bool>,
}
impl AuthService {
    pub async fn new(
        config: &super::config::ResolvedConfig,
        environment_id: String,
        db: Database,
        policy: Option<t3_contracts::managed_policy::ManagedPolicyDocument>,
    ) -> crate::Result<Self> {
        let secrets = ServerSecretStore::open(config.paths.secrets())
            .await
            .map_err(service_error)?;
        let auth_config = AuthConfig {
            trust_forwarded_host: true,
            desktop: config.mode == super::config::RuntimeMode::Desktop,
            port: config.port,
            host: config.host.clone(),
            environment_id,
            state_dir: config.paths.state_dir.to_string_lossy().into_owned(),
            dev_url: config.dev_url.as_ref().map(ToString::to_string),
            dev_auth_token: config.dev_auth_token().map(str::to_owned),
            dev_allowed_origins: config.dev_allowed_origins.clone(),
            ..Default::default()
        };
        let inner = EnvironmentAuth::new(db, secrets, auth_config, policy, SystemClock::new())
            .await
            .map_err(service_error)?;
        let (links_enabled, links) = tokio::sync::watch::channel(false);
        let mcp = McpOAuth::new(inner.clone(), links)
            .await
            .map_err(service_error)?;
        Ok(Self {
            inner,
            mcp,
            links_enabled,
        })
    }
    pub fn set_peer_links_enabled(&self, enabled: bool) {
        self.links_enabled.send_replace(enabled);
    }
    pub fn descriptor(&self) -> ServerAuthDescriptor {
        self.inner.descriptor()
    }
    pub async fn startup_pairing_credential(&self) -> crate::Result<String> {
        Ok(self
            .inner
            .startup_pairing_credential()
            .await
            .map_err(service_error)?
            .credential
            .0)
    }
    pub async fn authenticate(&self, request: &Request) -> t3_auth::Result<VerifiedSession> {
        self.inner.authenticate_http(request).await
    }
    pub async fn websocket(&self, request: &Request) -> t3_auth::Result<VerifiedSession> {
        self.inner
            .authenticate_websocket(request, AuthWebSocketTicketUse::Rpc)
            .await
    }
    pub async fn invalidation(&self, session: &VerifiedSession) {
        let _ = self
            .inner
            .sessions
            .await_invalidation(&session.session_id, session.proof_key_thumbprint.as_deref())
            .await;
    }
    pub async fn connected(&self, id: &AuthSessionId) -> t3_auth::Result<()> {
        self.inner.sessions.mark_connected(id).await
    }
    pub async fn disconnected(&self, id: &AuthSessionId) {
        let _ = self.inner.sessions.mark_disconnected(id).await;
    }
    pub async fn session_state(&self, req: &Request) -> t3_auth::Result<AuthSessionState> {
        self.inner.session_state(req).await
    }
    pub fn cookie_name(&self) -> String {
        self.inner.sessions.cookie_name()
    }
    /// The token of a session cookie presented under its legacy name, which the session route
    /// re-issues under the current name.
    pub fn legacy_cookie_token<'a>(&self, req: &'a Request) -> Option<&'a str> {
        let (token, source) = t3_auth::environment::select_request_credential(
            req,
            &self.inner.sessions.cookie_name(),
            self.inner.sessions.legacy_cookie_name().as_deref(),
        )?;
        (source == t3_auth::environment::CredentialSource::LegacyCookie).then_some(token)
    }
    pub async fn browser_session(
        &self,
        req: &Request,
        input: AuthBrowserSessionRequest,
    ) -> t3_auth::Result<t3_auth::environment::BrowserSession> {
        // The session being replaced: the current cookie, or the legacy one (TypeScript
        // `selectRequestCredential`, kept only for browser-cookie sources).
        use t3_auth::environment::CredentialSource;
        let previous = t3_auth::environment::select_request_credential(
            req,
            &self.inner.sessions.cookie_name(),
            self.inner.sessions.legacy_cookie_name().as_deref(),
        )
        .filter(|(_, source)| {
            matches!(
                source,
                CredentialSource::Cookie | CredentialSource::LegacyCookie
            )
        })
        .map(|(token, _)| token);
        self.inner
            .create_browser_session(req, &input.credential.0, req.metadata(None), previous)
            .await
    }
    /// Authenticates over HTTP and requires `scope`: the order every guarded route follows.
    pub async fn require(
        &self,
        req: &Request,
        scope: AuthEnvironmentScope,
    ) -> t3_auth::Result<VerifiedSession> {
        let session = self.authenticate(req).await?;
        authorization::require_scope(&session.scopes, scope)?;
        Ok(session)
    }
    pub async fn ticket(
        &self,
        session: &VerifiedSession,
        audience: Option<AuthWebSocketTicketUse>,
    ) -> t3_auth::Result<AuthWebSocketTicketResult> {
        self.inner.issue_websocket_ticket(session, audience).await
    }
    pub async fn pairing(
        &self,
        session: &VerifiedSession,
        input: AuthCreatePairingCredentialInput,
    ) -> t3_auth::Result<AuthPairingCredentialResult> {
        self.inner
            .issue_pairing_credential_for(&session.scopes, input)
            .await
    }
    pub async fn revoke_pairing_link(&self, id: &str) -> t3_auth::Result<bool> {
        self.inner.pairing.revoke(id).await
    }
    pub async fn revoke(
        &self,
        session: &VerifiedSession,
        target: &AuthSessionId,
    ) -> t3_auth::Result<bool> {
        self.inner.revoke_client(&session.session_id, target).await
    }
    /// Also removes the other trusted Connect keys and their unspent credentials.
    pub async fn revoke_others(&self, session: &VerifiedSession) -> t3_auth::Result<usize> {
        self.inner
            .revoke_other_client_sessions(&session.session_id)
            .await
    }
    pub async fn approve_connect_device(&self, pairing_link_id: &str) -> t3_auth::Result<bool> {
        self.inner.approve_connect_device(pairing_link_id).await
    }
    pub async fn revoke_connect_device(&self, proof_key: &str) -> t3_auth::Result<bool> {
        self.inner.revoke_connect_device(proof_key).await
    }
    pub async fn exchange(
        &self,
        req: &Request,
        input: AuthTokenExchangeRequest,
    ) -> t3_auth::Result<AuthAccessTokenResult> {
        let scopes = input
            .scope
            .as_ref()
            .map(|s| t3_auth::environment::parse_requested_scopes(&s.0))
            .transpose()?;
        let presented = AuthClientPresentationMetadata {
            label: input.client_label.clone(),
            device_type: input.client_device_type,
            os: input.client_os.clone(),
            os_major_version: None,
            device_model: None,
            surface: None,
            web_deployment: None,
            browser: None,
            app_version: None,
        };
        self.inner
            .exchange_bootstrap(
                req,
                &input.subject_token.0,
                scopes,
                req.metadata(Some(&presented)),
            )
            .await
    }
    pub fn mcp_protected_resource(
        &self,
        req: &Request,
    ) -> t3_auth::Result<AuthMcpProtectedResourceMetadata> {
        t3_auth::oauth::protected_resource_metadata(&OAuthUrls::from_request(req))
    }
    pub fn mcp_authorization_server(
        &self,
        req: &Request,
    ) -> t3_auth::Result<AuthMcpAuthorizationServerMetadata> {
        t3_auth::oauth::authorization_server_metadata(&OAuthUrls::from_request(req))
    }
    pub fn mcp_register(
        &self,
        metadata: AuthMcpClientRegistration,
    ) -> t3_auth::Result<AuthMcpRegisteredClient> {
        self.mcp.register(metadata)?.registration()
    }
    pub fn mcp_validate(
        &self,
        req: &Request,
        authorization: AuthMcpAuthorizationRequest,
    ) -> std::result::Result<AuthorizationRequest, AuthorizationError> {
        self.mcp
            .validate_authorization(&OAuthUrls::from_request(req), authorization)
    }
    pub async fn mcp_approval_details(
        &self,
        req: &Request,
        authorization: &AuthorizationRequest,
    ) -> t3_auth::Result<AuthMcpApprovalDetails> {
        self.mcp.approval_details(req, authorization).await
    }
    pub async fn mcp_decide(
        &self,
        req: &Request,
        authorization: &AuthorizationRequest,
        decision: AuthMcpApprovalDecision,
    ) -> t3_auth::Result<t3_auth::oauth::ApprovalResult> {
        self.mcp.approve(req, authorization, decision).await
    }
    pub fn mcp_deny(&self, authorization: &AuthorizationRequest) -> t3_auth::Result<String> {
        self.mcp.deny(authorization)
    }
    pub async fn mcp_token(
        &self,
        req: &Request,
        token: AuthMcpTokenRequest,
    ) -> t3_auth::Result<AuthMcpTokenResult> {
        self.mcp
            .exchange_code(req, &OAuthUrls::from_request(req), token)
            .await
    }
    pub fn inner(&self) -> &EnvironmentAuth {
        &self.inner
    }
}
pub fn request(
    method: &str,
    uri: &Uri,
    headers: &HeaderMap,
    remote: Option<std::net::SocketAddr>,
) -> Request {
    let mut result = Request {
        method: method.into(),
        url: uri.to_string(),
        protocol: Some("http".into()),
        remote_address: remote.map(|a| a.ip().to_string()),
        ..Default::default()
    };
    // Every header counts, including values that are not UTF-8: dropping one would hide a
    // duplicate that `t3-auth` treats as ambiguous.
    for (key, value) in headers {
        result.headers.append(
            key.as_str().into(),
            String::from_utf8_lossy(value.as_bytes()).into_owned(),
        );
    }
    for cookie in headers.get_all("cookie") {
        for part in String::from_utf8_lossy(cookie.as_bytes()).split(';') {
            if let Some((key, value)) = part.trim().split_once('=') {
                let value = value.trim();
                let decoded = percent_encoding::percent_decode_str(value)
                    .decode_utf8()
                    .map_or_else(|_| value.to_owned(), |v| v.into_owned());
                result.cookies.insert(key.trim().into(), decoded);
            }
        }
    }
    result
}
pub fn authorize(
    scopes: &[AuthEnvironmentScope],
    method: &str,
    payload: &Value,
) -> std::result::Result<(), t3_rpc::Exit> {
    authorization::authorize_rpc(scopes, method, payload).map_err(|e| match e {
        t3_auth::Error::Authorization(error) => {
            t3_rpc::Exit::fail(serde_json::to_value(error).unwrap_or(Value::Null))
        }
        _ => t3_rpc::Exit::die("Authorization failed"),
    })
}
pub fn now_iso() -> crate::Result<String> {
    t3_auth::clock::iso(SystemClock::new().now_ms())
        .map(|s| s.0)
        .map_err(service_error)
}

pub fn access_stream(
    services: Arc<crate::services::ServiceRegistry>,
    current: AuthSessionId,
) -> futures::stream::BoxStream<'static, crate::Result<AuthAccessStreamEvent>> {
    use futures::{StreamExt, future::Either};
    use t3_auth::{pairing::PairingChange, session::SessionChange};
    async_stream::try_stream! {
        let auth=services.local.auth.inner();
        let mut links=auth.pairing.subscribe().await;
        let mut sessions=auth.sessions.subscribe().await;
        let snapshot=async |revision:u64| -> crate::Result<AuthAccessStreamEvent> {
            let pairing_links=auth.list_pairing_links(None).await.map_err(service_error)?;
            let client_sessions=auth.list_client_sessions(Some(&current)).await.map_err(service_error)?;
            let connect_devices=auth.list_connect_devices().await.map_err(service_error)?;
            serde_json::from_value(serde_json::json!({"version":1,"revision":revision,"type":"snapshot","payload":{"pairingLinks":pairing_links,"clientSessions":client_sessions,"connectDevices":connect_devices}})).map_err(service_error)
        };
        yield snapshot(1).await?;
        let mut revision=1u64;
        loop {
            let change=tokio::select! {c=links.recv()=>Either::Left(c),c=sessions.recv()=>Either::Right(c)};
            // Trust changes have no event of their own: the client gets a fresh snapshot.
            if let Either::Left(Some(PairingChange::ConnectDevicesChanged))=change {
                revision+=1;
                yield snapshot(revision).await?;
                continue;
            }
            let (kind,mut payload)=match change {
                Either::Left(Some(PairingChange::PairingLinkUpserted(link)))=>("pairingLinkUpserted",serde_json::to_value(link).map_err(service_error)?),
                Either::Left(Some(PairingChange::PairingLinkRemoved(id)))=>("pairingLinkRemoved",serde_json::json!({"id":id})),
                Either::Right(Some(SessionChange::ClientUpserted(session)))=>{
                    let mut value=serde_json::to_value(&*session).map_err(service_error)?;
                    value["current"]=serde_json::json!(session.session_id==current);
                    ("clientUpserted",value)
                },
                Either::Right(Some(SessionChange::ClientRemoved(id)))=>("clientRemoved",serde_json::json!({"sessionId":id})),
                _=>break,
            };
            if let Some(scopes)=payload.get("scopes").cloned() {
                let scopes:Vec<AuthEnvironmentScope>=serde_json::from_value(scopes).map_err(service_error)?;
                payload["permissions"]=serde_json::json!(scopes);
                payload["scopes"]=serde_json::json!(authorization::legacy_scopes(&scopes));
            }
            revision+=1;
            yield serde_json::from_value(serde_json::json!({"version":1,"revision":revision,"type":kind,"payload":payload})).map_err(service_error)?;
        }
    }.boxed()
}

const COOKIE_VALUE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');
pub fn cookie_header(
    cookie: t3_auth::environment::Cookie,
) -> crate::Result<axum::http::HeaderValue> {
    let millis = t3_auth::clock::epoch(&cookie.expires_at).map_err(service_error)?;
    let date = std::time::UNIX_EPOCH + std::time::Duration::from_millis(millis.max(0) as u64);
    // Percent-encoded like Effect's cookie writer; `request` decodes it again. Session tokens are
    // already URL-safe, but the reusable dev credential is operator-chosen text.
    let encoded = percent_encoding::utf8_percent_encode(&cookie.value, COOKIE_VALUE).to_string();
    let mut value = format!(
        "{}={}; Path={}; Expires={}; SameSite=Lax",
        cookie.name,
        encoded,
        cookie.path,
        httpdate::fmt_http_date(date)
    );
    if cookie.http_only {
        value.push_str("; HttpOnly");
    }
    if cookie.secure {
        value.push_str("; Secure");
    }
    if let Some(age) = cookie.max_age_seconds {
        value.push_str(&format!("; Max-Age={age}"));
    }
    value.parse().map_err(service_error)
}
