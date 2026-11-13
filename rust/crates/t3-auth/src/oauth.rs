use crate::{
    EnvironmentAuth, Error, Request, Result,
    authorization::{mcp_scopes, require_scope},
    clock::{epoch, expires_after},
    error::ApprovalPage,
    pairing::{oauth_scope, parse_approval_code},
    utils::{PayloadSigner, b64, sha256, timing_safe_b64, unb64},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashMap, sync::Arc};
use t3_contracts::auth::{
    AuthBearerToken, AuthEnvironmentScope, AuthMcpApprovalDecision, AuthMcpApprovalDetails,
    AuthMcpAuthorizationRequest, AuthMcpAuthorizationServerMetadata, AuthMcpClientAccess,
    AuthMcpClientRegistration, AuthMcpProtectedResourceMetadata, AuthMcpRegisteredClient,
    AuthMcpTokenRequest, AuthMcpTokenResult, EncodedNumber,
};
use tokio::sync::{Mutex, watch};

pub const PEER_LINK_SOFTWARE_ID: &str = "t3code-peer-link";
#[derive(Debug, Clone)]
pub struct OAuthUrls {
    pub issuer: String,
    pub resource: String,
}
impl OAuthUrls {
    pub fn from_request(request: &Request) -> Self {
        let issuer = request
            .absolute_url()
            .map(|u| u.origin().ascii_serialization())
            .unwrap_or_else(|| "http://localhost".into());
        Self {
            resource: format!("{issuer}/mcp"),
            issuer,
        }
    }
}
pub fn protected_resource_metadata(urls: &OAuthUrls) -> Result<AuthMcpProtectedResourceMetadata> {
    Ok(serde_json::from_value(
        json!({"resource":urls.resource,"authorization_servers":[urls.issuer],"scopes_supported":["orchestration:read","orchestration:operate"],"bearer_methods_supported":["header"],"resource_name":"T3 Code"}),
    )?)
}
pub fn authorization_server_metadata(
    urls: &OAuthUrls,
) -> Result<AuthMcpAuthorizationServerMetadata> {
    let issuer = &urls.issuer;
    Ok(serde_json::from_value(
        json!({"issuer":issuer,"authorization_endpoint":format!("{issuer}/oauth/mcp/authorize"),"token_endpoint":format!("{issuer}/oauth/mcp/token"),"registration_endpoint":format!("{issuer}/oauth/mcp/register"),"response_types_supported":["code"],"grant_types_supported":["authorization_code"],"code_challenge_methods_supported":["S256"],"token_endpoint_auth_methods_supported":["none"],"scopes_supported":["orchestration:read","orchestration:operate"],"authorization_response_iss_parameter_supported":true}),
    )?)
}

fn parse_redirect(value: &str) -> Option<url::Url> {
    if value.encode_utf16().count() > 512 || value.contains('#') {
        return None;
    }
    let url = url::Url::parse(value).ok()?;
    let allowed = url.scheme() == "https"
        || (url.scheme() == "http"
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")));
    (allowed && url.username().is_empty() && url.password().is_none_or(str::is_empty))
        .then_some(url)
}
pub fn redirect_matches(registered: &str, presented: &str) -> bool {
    let (Some(left), Some(right)) = (parse_redirect(registered), parse_redirect(presented)) else {
        return false;
    };
    if left.scheme() == "https" || right.scheme() == "https" {
        return left == right;
    }
    left.host_str() == right.host_str()
        && left.path() == right.path()
        && left.query() == right.query()
}
pub fn same_resource(urls: &OAuthUrls, value: Option<&str>) -> bool {
    let Some(value) = value else {
        return true;
    };
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    url.fragment().is_none_or(str::is_empty)
        && format!(
            "{}{}",
            url.origin().ascii_serialization(),
            url.path().trim_end_matches('/')
        ) == urls.resource
}

/// The fixed MCP endpoint's canonical resource identifier, distinct from OAuth endpoint URLs.
pub fn canonical_mcp_resource(value: &str) -> Result<String> {
    let url = url::Url::parse(value).map_err(|_| Error::invalid("Invalid MCP resource URL."))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.path() != "/mcp"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(Error::invalid(
            "MCP resource must be an http(s) /mcp URL without credentials, query or fragment.",
        ));
    }
    Ok(url.into())
}

#[derive(Clone)]
pub struct McpOAuthClient {
    pub client_id: String,
    pub name: String,
    pub redirect_uris: Vec<String>,
    pub peer_link: bool,
}
impl McpOAuthClient {
    pub fn registration(&self) -> Result<AuthMcpRegisteredClient> {
        Ok(serde_json::from_value(
            json!({"client_id":self.client_id,"client_name":self.name,"redirect_uris":self.redirect_uris,"grant_types":["authorization_code"],"response_types":["code"],"token_endpoint_auth_method":"none"}),
        )?)
    }
}
#[derive(Clone)]
pub struct AuthorizationRequest {
    pub client: McpOAuthClient,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub state: Option<String>,
    pub resource: String,
    pub issuer: String,
}
#[derive(thiserror::Error)]
pub enum AuthorizationError {
    #[error("{0}")]
    Page(String),
    #[error("{description}")]
    Redirect {
        error: &'static str,
        description: String,
        redirect_uri: String,
        state: Option<String>,
    },
}
impl std::fmt::Debug for AuthorizationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Page(_) => f.write_str("AuthorizationError::Page"),
            Self::Redirect { error, .. } => f
                .debug_struct("AuthorizationError::Redirect")
                .field("error", error)
                .field("redirect_uri", &"[REDACTED]")
                .field("state", &"[REDACTED]")
                .finish(),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMethod {
    PairingCode,
    BrowserSession,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApprovalGrant {
    pub access: AuthMcpClientAccess,
    pub method: ApprovalMethod,
}
pub struct ApprovalResult {
    pub redirect: String,
    pub granted: Option<ApprovalGrant>,
}
impl std::fmt::Debug for ApprovalResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalResult")
            .field("redirect", &"[REDACTED]")
            .field("granted", &self.granted)
            .finish()
    }
}
impl AuthorizationError {
    pub fn redirect(&self, issuer: &str) -> Option<String> {
        if let Self::Redirect {
            error,
            description,
            redirect_uri,
            state,
        } = self
        {
            redirect_with(
                redirect_uri,
                &[
                    ("error", Some(error)),
                    ("error_description", Some(description)),
                    ("state", state.as_deref()),
                    ("iss", Some(issuer)),
                ],
            )
            .ok()
        } else {
            None
        }
    }
}
#[derive(Serialize, Deserialize)]
struct ClientId {
    v: u8,
    n: String,
    r: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    p: Option<u8>,
}
struct PendingCode {
    client_id: String,
    client_name: String,
    peer_link: bool,
    redirect_uri: String,
    code_challenge: String,
    resource: String,
    access: AuthMcpClientAccess,
    expires_ms: i64,
}
pub struct ApprovingSession {
    pub csrf_token: String,
    pub scopes: Vec<AuthEnvironmentScope>,
}
#[derive(Clone)]
pub struct McpOAuth {
    auth: EnvironmentAuth,
    signer: PayloadSigner,
    links_enabled: watch::Receiver<bool>,
    codes: Arc<Mutex<HashMap<String, PendingCode>>>,
}
impl McpOAuth {
    pub async fn new(auth: EnvironmentAuth, links_enabled: watch::Receiver<bool>) -> Result<Self> {
        let signer = PayloadSigner::from_store(&auth.secrets, "mcp-oauth-signing-key").await?;
        Ok(Self {
            auth,
            signer,
            links_enabled,
            codes: Arc::new(Mutex::new(HashMap::new())),
        })
    }
    fn sign(&self, domain: &str, body: &str) -> String {
        self.signer.sign(&format!("{domain}.{body}"))
    }
    pub fn parse_client_id(&self, client_id: &str) -> Option<McpOAuthClient> {
        let parts: Vec<_> = client_id.split('.').collect();
        if parts.len() != 2
            || parts.iter().any(|p| p.is_empty())
            || !self
                .signer
                .verify(&format!("mcp-client-id.{}", parts[0]), parts[1])
        {
            return None;
        }
        let payload: ClientId = serde_json::from_slice(&unb64(parts[0])?).ok()?;
        if payload.v != 1 || payload.p.is_some_and(|p| p != 1) {
            return None;
        }
        Some(McpOAuthClient {
            client_id: client_id.into(),
            name: payload.n,
            redirect_uris: payload.r,
            peer_link: payload.p == Some(1),
        })
    }
    pub fn register(&self, metadata: AuthMcpClientRegistration) -> Result<McpOAuthClient> {
        let name = metadata.client_name.unwrap_or_default().trim().to_owned();
        let name = if name.is_empty() {
            "MCP client".into()
        } else {
            String::from_utf16_lossy(&name.encode_utf16().take(100).collect::<Vec<_>>())
        };
        let redirects = metadata.redirect_uris.unwrap_or_default();
        if redirects.is_empty() || redirects.len() > 5 {
            return Err(oauth_error(
                "invalid_redirect_uri",
                "Register between 1 and 5 redirect URIs.",
            ));
        }
        if redirects.iter().any(|u| parse_redirect(u).is_none()) {
            return Err(oauth_error(
                "invalid_redirect_uri",
                "Redirect URIs must be https, or http on localhost, 127.0.0.1 or [::1].",
            ));
        }
        let peer_link = metadata.software_id.as_deref() == Some(PEER_LINK_SOFTWARE_ID);
        if peer_link && !*self.links_enabled.borrow() {
            return Err(oauth_error(
                "invalid_client_metadata",
                "Linked environments is off on that machine. Turn it on in its Settings → Labs.",
            ));
        }
        let body = b64(serde_json::to_vec(&ClientId {
            v: 1,
            n: name.clone(),
            r: redirects.clone(),
            p: peer_link.then_some(1),
        })?);
        Ok(McpOAuthClient {
            client_id: format!("{body}.{}", self.sign("mcp-client-id", &body)),
            name,
            redirect_uris: redirects,
            peer_link,
        })
    }
    pub fn validate_authorization(
        &self,
        urls: &OAuthUrls,
        request: AuthMcpAuthorizationRequest,
    ) -> std::result::Result<AuthorizationRequest, AuthorizationError> {
        let client = self
            .parse_client_id(request.client_id.as_deref().unwrap_or(""))
            .ok_or_else(|| {
                AuthorizationError::Page(
                    "This sign-in link names an unknown app. Start the sign-in again from it."
                        .into(),
                )
            })?;
        let redirect_uri = request
            .redirect_uri
            .filter(|u| client.redirect_uris.iter().any(|r| redirect_matches(r, u)))
            .ok_or_else(|| {
                AuthorizationError::Page(
                    "This sign-in link sends you to an address the app did not register.".into(),
                )
            })?;
        let fail = |error, description: &str| AuthorizationError::Redirect {
            error,
            description: description.into(),
            redirect_uri: redirect_uri.clone(),
            state: request.state.clone(),
        };
        if request.response_type.as_deref() != Some("code") {
            return Err(fail(
                "unsupported_response_type",
                "Only response_type=code is supported.",
            ));
        }
        let code_challenge = request.code_challenge.unwrap_or_default();
        if request.code_challenge_method.as_deref() != Some("S256")
            || code_challenge.len() != 43
            || !code_challenge
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(fail(
                "invalid_request",
                "PKCE with code_challenge_method=S256 is required.",
            ));
        }
        if !same_resource(urls, request.resource.as_deref()) {
            return Err(fail(
                "invalid_target",
                &format!("This server only issues tokens for {}.", urls.resource),
            ));
        }
        Ok(AuthorizationRequest {
            client,
            redirect_uri,
            code_challenge,
            state: request.state,
            resource: urls.resource.clone(),
            issuer: urls.issuer.clone(),
        })
    }
    pub async fn approving_session(
        &self,
        request: &Request,
        authorization: &AuthorizationRequest,
    ) -> Option<ApprovingSession> {
        let session = self.auth.authenticate_http(request).await.ok()?;
        require_scope(&session.scopes, AuthEnvironmentScope::AccessWrite).ok()?;
        require_scope(&session.scopes, AuthEnvironmentScope::OrchestrationRead).ok()?;
        let holder = session
            .proof_key_thumbprint
            .map(|k| format!("dpop:{k}"))
            .unwrap_or(session.session_id.0);
        let body = serde_json::to_string(&[
            holder.as_str(),
            authorization.client.client_id.as_str(),
            authorization.redirect_uri.as_str(),
            authorization.code_challenge.as_str(),
        ])
        .ok()?;
        Some(ApprovingSession {
            csrf_token: self.sign("mcp-csrf", &body),
            scopes: session.scopes,
        })
    }
    pub async fn approval_details(
        &self,
        request: &Request,
        authorization: &AuthorizationRequest,
    ) -> Result<AuthMcpApprovalDetails> {
        let session = self.approving_session(request, authorization).await;
        let host = |value: &str| -> Result<String> {
            let url = url::Url::parse(value).map_err(|e| Error::InvalidPayload(e.to_string()))?;
            let hostname = url.host_str().unwrap_or("");
            Ok(url
                .port()
                .map(|p| format!("{hostname}:{p}"))
                .unwrap_or_else(|| hostname.to_owned()))
        };
        let one_click_access = session.as_ref().map(|s| {
            [
                AuthMcpClientAccess::ReadOnly,
                AuthMcpClientAccess::ApprovalRequired,
                AuthMcpClientAccess::AutoAcceptEdits,
                AuthMcpClientAccess::Auto,
                AuthMcpClientAccess::FullAccess,
            ]
            .into_iter()
            .filter(|a| mcp_scopes(*a).iter().all(|scope| s.scopes.contains(scope)))
            .collect()
        });
        Ok(AuthMcpApprovalDetails {
            client_name: authorization.client.name.clone(),
            redirect_host: host(&authorization.redirect_uri)?,
            environment_host: host(&authorization.issuer)?,
            csrf_token: session.map(|s| s.csrf_token),
            one_click_access,
            runtime_mode_ceiling: self.auth.runtime_mode_ceiling(),
        })
    }
    pub async fn approve(
        &self,
        request: &Request,
        authorization: &AuthorizationRequest,
        decision: AuthMcpApprovalDecision,
    ) -> Result<ApprovalResult> {
        let (access, method) = match decision {
            AuthMcpApprovalDecision::AuthMcpApprovalDeny(_) => {
                return Ok(ApprovalResult {
                    redirect: self.deny(authorization)?,
                    granted: None,
                });
            }
            AuthMcpApprovalDecision::AuthMcpApprovalPairingCode(decision) => {
                if authorization.client.peer_link && parse_approval_code(&decision.code.0).is_some()
                {
                    return Err(Error::ApprovalPage(ApprovalPage::PeerLinkNeedsPairingCode));
                }
                (
                    self.auth
                        .consume_mcp_approval_code(&decision.code.0, decision.access)
                        .await?,
                    ApprovalMethod::PairingCode,
                )
            }
            AuthMcpApprovalDecision::AuthMcpApprovalBrowserSession(decision) => {
                let session = self.approving_session(request, authorization).await;
                if session.as_ref().is_none_or(|s| {
                    !timing_safe_b64(&decision.csrf_token, &s.csrf_token)
                        || !mcp_scopes(decision.access)
                            .iter()
                            .all(|scope| s.scopes.contains(scope))
                }) {
                    return Err(Error::ApprovalPage(ApprovalPage::SessionCannotApprove));
                }
                (decision.access, ApprovalMethod::BrowserSession)
            }
        };
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| Error::Random(e.to_string()))?;
        let code = b64(bytes);
        let now = self.auth.now_ms();
        let mut codes = self.codes.lock().await;
        codes.retain(|_, p| p.expires_ms > now);
        codes.insert(
            code.clone(),
            PendingCode {
                client_id: authorization.client.client_id.clone(),
                client_name: authorization.client.name.clone(),
                peer_link: authorization.client.peer_link,
                redirect_uri: authorization.redirect_uri.clone(),
                code_challenge: authorization.code_challenge.clone(),
                resource: authorization.resource.clone(),
                access,
                expires_ms: expires_after(now, 60_000)?,
            },
        );
        Ok(ApprovalResult {
            redirect: redirect_with(
                &authorization.redirect_uri,
                &[
                    ("code", Some(&code)),
                    ("state", authorization.state.as_deref()),
                    ("iss", Some(&authorization.issuer)),
                ],
            )?,
            granted: Some(ApprovalGrant { access, method }),
        })
    }
    pub fn deny(&self, authorization: &AuthorizationRequest) -> Result<String> {
        redirect_with(
            &authorization.redirect_uri,
            &[
                ("error", Some("access_denied")),
                ("state", authorization.state.as_deref()),
                ("iss", Some(&authorization.issuer)),
            ],
        )
    }
    pub async fn exchange_code(
        &self,
        request: &Request,
        urls: &OAuthUrls,
        token: AuthMcpTokenRequest,
    ) -> Result<AuthMcpTokenResult> {
        if token.grant_type.as_deref() != Some("authorization_code") {
            return Err(oauth_error(
                "unsupported_grant_type",
                "Only authorization_code is supported.",
            ));
        }
        let required = || {
            oauth_error(
                "invalid_request",
                "code, redirect_uri, client_id and code_verifier are required.",
            )
        };
        let code = token.code.filter(|s| !s.is_empty()).ok_or_else(required)?;
        let redirect = token
            .redirect_uri
            .filter(|s| !s.is_empty())
            .ok_or_else(required)?;
        let client = token
            .client_id
            .filter(|s| !s.is_empty())
            .ok_or_else(required)?;
        let verifier = token
            .code_verifier
            .filter(|s| !s.is_empty())
            .ok_or_else(required)?;
        if self.parse_client_id(&client).is_none() {
            return Err(oauth_error("invalid_client", "The client is unknown."));
        }
        let pending = self.codes.lock().await.remove(&code);
        let now = self.auth.now_ms();
        let invalid = || {
            oauth_error(
                "invalid_grant",
                "The authorization code is invalid or expired.",
            )
        };
        let pending = pending.ok_or_else(invalid)?;
        if pending.expires_ms <= now
            || pending.client_id != client
            || pending.redirect_uri != redirect
            || pending.resource != urls.resource
            || !same_resource(urls, token.resource.as_deref())
            || !pkce_verifies(&verifier, &pending.code_challenge)
        {
            return Err(invalid());
        }
        let issued = self
            .auth
            .issue_mcp_session(
                &pending.resource,
                pending.client_name,
                pending.access,
                pending.peer_link,
                request.metadata(None),
            )
            .await
            .map_err(|source| Error::OAuthSessionIssue {
                source: Box::new(source),
            })?;
        Ok(AuthMcpTokenResult {
            access_token: issued.token,
            token_type: AuthBearerToken::Bearer,
            expires_in: EncodedNumber::Finite(
                ((epoch(&issued.expires_at)? - now).max(0) / 1000).into(),
            ),
            scope: oauth_scope(&mcp_scopes(pending.access))?,
        })
    }
}
pub fn pkce_verifies(verifier: &str, challenge: &str) -> bool {
    (43..=128).contains(&verifier.len())
        && verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
        && timing_safe_b64(&b64(sha256(verifier)), challenge)
}
fn redirect_with(uri: &str, params: &[(&str, Option<&str>)]) -> Result<String> {
    let mut url = url::Url::parse(uri).map_err(|e| Error::InvalidPayload(e.to_string()))?;
    let keys: Vec<_> = params
        .iter()
        .filter(|(_, v)| v.is_some())
        .map(|(k, _)| *k)
        .collect();
    let existing: Vec<_> = url
        .query_pairs()
        .filter(|(k, _)| !keys.contains(&k.as_ref()))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.set_query(None);
    {
        let mut query = url.query_pairs_mut();
        query.extend_pairs(existing);
        for (key, value) in params {
            if let Some(value) = value {
                query.append_pair(key, value);
            }
        }
    }
    Ok(url.into())
}
fn oauth_error(error: &'static str, description: &str) -> Error {
    Error::OAuth {
        error,
        description: description.into(),
    }
}
