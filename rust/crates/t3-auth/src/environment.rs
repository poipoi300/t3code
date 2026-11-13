use crate::{
    Error, PairingGrantStore, Result, ServerSecretStore, SessionStore,
    authorization::{STANDARD_SCOPES, legacy_scopes, mcp_scopes},
    clock::{Clock, epoch, expires_after, iso},
    error::McpApprovalReason,
    pairing::{APPROVAL_SUBJECT_PREFIX, ConsumePairing, IssuePairing, oauth_scope},
    session::{IssueSession, IssuedSession, SESSION_TTL_MS, VerifiedSession},
    utils::{default_metadata, derive_client_metadata, descriptor, is_loopback_host, pairing_url},
};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};
use t3_contracts::{
    auth::{
        AuthAccessTokenResult, AuthAccessTokenType, AuthBrowserSessionResult, AuthClientMetadata,
        AuthClientMetadataDeviceType, AuthClientPresentationMetadata, AuthClientSession,
        AuthCreatePairingCredentialInput, AuthEnvironmentScope, AuthGrantScope,
        AuthMcpClientAccess, AuthPairingCredentialResult, AuthPairingLink, AuthRuntimeModeCeiling,
        AuthSessionState, AuthTokenType, AuthWebSocketTicketResult, AuthWebSocketTicketUse,
        Authenticated, EncodedNumber, ServerAuthBootstrapMethod, ServerAuthDescriptor,
        ServerAuthPolicy, ServerAuthSessionMethod,
    },
    base_schemas::{AuthSessionId, DpopFailureReason, TrimmedNonEmptyString},
    managed_policy::{ManagedPolicyClient, ManagedPolicyDocument},
};
use t3_persistence::Database;

#[derive(Clone, Default)]
pub struct AuthConfig {
    pub desktop: bool,
    pub trust_forwarded_host: bool,
    pub port: u16,
    pub host: Option<String>,
    pub state_dir: String,
    pub environment_id: String,
    pub dev_url: Option<String>,
    pub dev_auth_token: Option<String>,
    pub dev_allowed_origins: Vec<String>,
    pub desktop_bootstrap_token: Option<String>,
    pub desktop_bootstrap_secret: Option<String>,
}
impl AuthConfig {
    pub fn validate(&self) -> Result<()> {
        if [
            &self.desktop_bootstrap_secret,
            &self.desktop_bootstrap_token,
            &self.dev_auth_token,
        ]
        .into_iter()
        .any(|secret| secret.as_ref().is_some_and(String::is_empty))
        {
            return Err(Error::EmptySecret);
        }
        Ok(())
    }
    pub fn development(&self) -> bool {
        self.dev_url.is_some()
    }
}

pub fn allows_auth_policy(
    policy: Option<&ManagedPolicyDocument>,
    auth_policy: ServerAuthPolicy,
) -> bool {
    let Some(policy) = policy else {
        return true;
    };
    if auth_policy == ServerAuthPolicy::UnsafeNoAuth {
        return false;
    }
    policy
        .limits
        .as_ref()
        .and_then(|l| l.clients.as_ref())
        .is_none_or(|clients| {
            clients.iter().any(|client| {
                matches!(
                    (client, auth_policy),
                    (
                        ManagedPolicyClient::Desktop,
                        ServerAuthPolicy::DesktopManagedLocal
                    ) | (
                        ManagedPolicyClient::LocalBrowser,
                        ServerAuthPolicy::LoopbackBrowser
                    ) | (
                        ManagedPolicyClient::Remote,
                        ServerAuthPolicy::RemoteReachable
                    )
                )
            })
        })
}

/// Case-insensitive names; duplicate values remain ambiguous and cannot authenticate.
#[derive(Default, Clone)]
pub struct Headers(BTreeMap<String, Vec<String>>);
impl Headers {
    pub fn insert(&mut self, name: String, value: String) {
        self.0.insert(name.to_ascii_lowercase(), vec![value]);
    }
    pub fn append(&mut self, name: String, value: String) {
        self.0
            .entry(name.to_ascii_lowercase())
            .or_default()
            .push(value);
    }
    pub fn remove(&mut self, name: &str) {
        self.0.remove(&name.to_ascii_lowercase());
    }
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .get(&name.to_ascii_lowercase())
            .map(|values| match values.as_slice() {
                [value] => value.as_str(),
                _ => "",
            })
    }
}
impl FromIterator<(String, String)> for Headers {
    fn from_iter<T: IntoIterator<Item = (String, String)>>(values: T) -> Self {
        let mut headers = Self::default();
        for (name, value) in values {
            headers.append(name, value);
        }
        headers
    }
}
impl<const N: usize> From<[(String, String); N]> for Headers {
    fn from(values: [(String, String); N]) -> Self {
        values.into_iter().collect()
    }
}
#[derive(Default, Clone)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub protocol: Option<String>,
    pub headers: Headers,
    pub cookies: BTreeMap<String, String>,
    pub remote_address: Option<String>,
}
impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name)
    }
    pub fn absolute_url(&self) -> Option<url::Url> {
        let host = self.header("host").unwrap_or("localhost");
        let proto = if self.header("x-forwarded-proto") == Some("https") {
            "https"
        } else {
            self.protocol
                .as_deref()
                .unwrap_or(if self.url.to_lowercase().starts_with("https:") {
                    "https"
                } else {
                    "http"
                })
        };
        let base = url::Url::parse(&format!("{proto}://{host}")).ok()?;
        base.join(&if self.url.starts_with('/') {
            format!(".{}", self.url)
        } else {
            self.url.clone()
        })
        .ok()
    }
    pub fn metadata(
        &self,
        presented: Option<&AuthClientPresentationMetadata>,
    ) -> AuthClientMetadata {
        derive_client_metadata(
            self.header("user-agent"),
            self.remote_address.as_deref(),
            presented,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSource {
    Cookie,
    Bearer,
    Dpop,
    LegacyCookie,
    DevCookie,
}
pub fn parse_authorization<'a>(request: &'a Request, scheme: &str) -> Option<&'a str> {
    let header = request.header("authorization")?;
    let (prefix, token) = header.split_once(' ')?;
    (prefix.eq_ignore_ascii_case(scheme) && !token.trim().is_empty()).then(|| token.trim())
}
pub fn select_request_credential<'a>(
    request: &'a Request,
    cookie_name: &str,
    legacy_cookie: Option<&str>,
) -> Option<(&'a str, CredentialSource)> {
    if let Some(token) = request.cookies.get(cookie_name).filter(|t| !t.is_empty()) {
        return Some((token, CredentialSource::Cookie));
    }
    if let Some(token) = parse_authorization(request, "Bearer") {
        return Some((token, CredentialSource::Bearer));
    }
    if let Some(token) = parse_authorization(request, "DPoP") {
        return Some((token, CredentialSource::Dpop));
    }
    legacy_cookie
        .and_then(|name| request.cookies.get(name))
        .map(|t| (t.as_str(), CredentialSource::LegacyCookie))
}
pub fn browser_origin_allowed(
    request: &Request,
    trusted: &HashSet<String>,
    trust_forwarded_host: bool,
) -> bool {
    let Some(origin) = request.header("origin") else {
        return true;
    };
    if request.header("sec-fetch-site") == Some("same-origin") || trusted.contains(origin) {
        return true;
    }
    let Ok(origin) = url::Url::parse(origin) else {
        return false;
    };
    let Some(host) = origin.host_str() else {
        return false;
    };
    let origin_host = match origin.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.into(),
    }
    .to_lowercase();
    ["host", "x-forwarded-host"]
        .iter()
        .filter(|h| **h == "host" || trust_forwarded_host)
        .any(|h| {
            request
                .header(h)
                .and_then(|v| v.split(',').next())
                .is_some_and(|h| h.trim().to_lowercase() == origin_host)
        })
}

#[derive(Clone, PartialEq, Eq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub expires_at: t3_contracts::wire::WireDateTime,
    pub http_only: bool,
    pub path: &'static str,
    pub same_site: &'static str,
    pub secure: bool,
    pub max_age_seconds: Option<i64>,
}
impl Cookie {
    pub fn session(
        name: String,
        token: String,
        expires_at: t3_contracts::wire::WireDateTime,
        secure: bool,
    ) -> Self {
        Self {
            name,
            value: token,
            expires_at,
            http_only: true,
            path: "/",
            same_site: "lax",
            secure,
            max_age_seconds: None,
        }
    }
    pub fn expire(name: String, secure: bool) -> Result<Self> {
        let mut cookie = Self::session(name, String::new(), iso(0)?, secure);
        cookie.max_age_seconds = Some(0);
        Ok(cookie)
    }
}
impl std::fmt::Debug for Cookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cookie")
            .field("name", &self.name)
            .field("value", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .field("http_only", &self.http_only)
            .field("path", &self.path)
            .field("same_site", &self.same_site)
            .field("secure", &self.secure)
            .field("max_age_seconds", &self.max_age_seconds)
            .finish()
    }
}
pub const CREDENTIAL_RESPONSE_HEADERS: &[(&str, &str)] =
    &[("cache-control", "no-store"), ("pragma", "no-cache")];
pub fn dpop_challenge(request: &Request) -> bool {
    (request.url.starts_with("/oauth/token") && request.header("dpop").is_some())
        || request
            .header("authorization")
            .is_some_and(|h| h.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("DPoP ")))
}
pub struct BrowserSession {
    pub response: AuthBrowserSessionResult,
    pub session_token: String,
    pub cookies: Vec<Cookie>,
}
pub struct McpClientSession {
    pub session_id: AuthSessionId,
    pub label: String,
    pub access: AuthMcpClientAccess,
    pub peer_link: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PolicyRevocations {
    pub sessions: usize,
    pub pairing_links: usize,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum GrantSource {
    Pairing,
    Desktop,
    Dev,
}
struct ResolvedBootstrap {
    scopes: Vec<AuthEnvironmentScope>,
    subject: String,
    label: Option<String>,
    source: GrantSource,
}
#[derive(Clone)]
pub struct EnvironmentAuth {
    pub sessions: SessionStore,
    pub pairing: PairingGrantStore,
    pub secrets: ServerSecretStore,
    config: AuthConfig,
    trusted_origins: HashSet<String>,
    policy: Option<ManagedPolicyDocument>,
    initial_policy_revocations: PolicyRevocations,
    clock: Arc<dyn Clock>,
}
impl EnvironmentAuth {
    pub async fn new(
        db: Database,
        secrets: ServerSecretStore,
        config: AuthConfig,
        policy: Option<ManagedPolicyDocument>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        let sessions =
            SessionStore::new(db.clone(), &secrets, config.clone(), clock.clone()).await?;
        let pairing = PairingGrantStore::new(db, config.clone(), clock.clone())?;
        let mut trusted_origins: HashSet<_> = config.dev_allowed_origins.iter().cloned().collect();
        trusted_origins.extend(["t3code://app".into(), "t3code-dev://app".into()]);
        if let Some(dev) = &config.dev_url
            && let Ok(url) = url::Url::parse(dev)
        {
            trusted_origins.insert(url.origin().ascii_serialization());
        }
        let mut auth = Self {
            sessions,
            pairing,
            secrets,
            config,
            trusted_origins,
            initial_policy_revocations: PolicyRevocations::default(),
            policy,
            clock,
        };
        auth.initial_policy_revocations = auth.enforce_policy().await?;
        Ok(auth)
    }
    pub fn config(&self) -> &AuthConfig {
        &self.config
    }
    pub fn descriptor(&self) -> ServerAuthDescriptor {
        descriptor(&self.config)
    }
    pub fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }
    pub fn runtime_mode_ceiling(&self) -> Option<AuthRuntimeModeCeiling> {
        let policy = self.policy.as_ref()?;
        Some(AuthRuntimeModeCeiling {
            organization: policy.organization.0.clone(),
            max_runtime_mode: policy.limits.as_ref()?.max_runtime_mode?,
        })
    }
    pub fn remote_allowed(&self) -> bool {
        allows_auth_policy(self.policy.as_ref(), ServerAuthPolicy::RemoteReachable)
    }
    pub fn connect_allowed(&self) -> bool {
        self.remote_allowed()
            && self
                .policy
                .as_ref()
                .and_then(|p| p.limits.as_ref())
                .and_then(|l| l.connect)
                != Some(false)
    }
    pub fn initial_policy_revocations(&self) -> PolicyRevocations {
        self.initial_policy_revocations
    }
    pub async fn enforce_policy(&self) -> Result<PolicyRevocations> {
        let mut counts = PolicyRevocations::default();
        let remote = self.remote_allowed();
        let connect = self.connect_allowed();
        if remote && connect {
            return Ok(counts);
        }
        for session in self.sessions.list_active().await? {
            if (!remote && session.subject.0 == "one-time-token")
                || (!connect && session.subject.0 == "cloud-connect")
                || (!remote
                    && session.subject.0 == "administrative-bootstrap"
                    && session
                        .client
                        .ip_address
                        .as_ref()
                        .is_some_and(|ip| !is_loopback_host(&ip.0)))
            {
                counts.sessions += usize::from(self.sessions.revoke(&session.session_id).await?);
            }
        }
        for link in self.pairing.list_active().await? {
            if (!remote
                && (link.subject.0 == "one-time-token"
                    || link.subject.0.starts_with(APPROVAL_SUBJECT_PREFIX)))
                || (!connect && link.subject.0 == "cloud-connect")
            {
                counts.pairing_links += usize::from(self.pairing.revoke(&link.id.0).await?);
            }
        }
        Ok(counts)
    }
    pub async fn create_pairing_link(
        &self,
        caller_scopes: &[AuthEnvironmentScope],
        input: IssuePairing,
    ) -> Result<AuthPairingCredentialResult> {
        if !input.startup && !self.remote_allowed() {
            return Err(Error::PolicyRefused(format!(
                "{}'s policy does not allow remote clients, so this server cannot create pairing links.",
                self.policy
                    .as_ref()
                    .map(|p| p.organization.0.as_str())
                    .unwrap_or("Your organization")
            )));
        }
        self.pairing
            .issue_one_time_token(caller_scopes, input)
            .await
    }
    pub async fn issue_pairing_credential_for(
        &self,
        caller_scopes: &[AuthEnvironmentScope],
        input: AuthCreatePairingCredentialInput,
    ) -> Result<AuthPairingCredentialResult> {
        let scopes = if let Some(access) = input.access {
            mcp_scopes(access)
        } else {
            input
                .scopes
                .ok_or(Error::InvalidScope)?
                .into_iter()
                .map(|scope| match scope {
                    AuthGrantScope::OrchestrationRead => AuthEnvironmentScope::OrchestrationRead,
                    AuthGrantScope::OrchestrationOperate => {
                        AuthEnvironmentScope::OrchestrationOperate
                    }
                    AuthGrantScope::SettingsWrite => AuthEnvironmentScope::SettingsWrite,
                    AuthGrantScope::ProvidersManage => AuthEnvironmentScope::ProvidersManage,
                    AuthGrantScope::EnvironmentMaintain => {
                        AuthEnvironmentScope::EnvironmentMaintain
                    }
                    AuthGrantScope::PreviewOperate => AuthEnvironmentScope::PreviewOperate,
                    AuthGrantScope::DiagnosticsRead => AuthEnvironmentScope::DiagnosticsRead,
                    AuthGrantScope::TerminalRead => AuthEnvironmentScope::TerminalRead,
                    AuthGrantScope::TerminalOperate => AuthEnvironmentScope::TerminalOperate,
                    AuthGrantScope::SourceControlWrite => AuthEnvironmentScope::SourceControlWrite,
                    AuthGrantScope::FilesystemRead => AuthEnvironmentScope::FilesystemRead,
                    AuthGrantScope::FilesystemWrite => AuthEnvironmentScope::FilesystemWrite,
                    AuthGrantScope::RelayRead => AuthEnvironmentScope::RelayRead,
                    AuthGrantScope::RelayWrite => AuthEnvironmentScope::RelayWrite,
                    AuthGrantScope::AccessRead => AuthEnvironmentScope::AccessRead,
                    AuthGrantScope::AccessWrite => AuthEnvironmentScope::AccessWrite,
                })
                .collect()
        };
        self.create_pairing_link(
            caller_scopes,
            IssuePairing {
                label: input.label.map(|s| s.0),
                scopes,
                mcp_approval: input.access,
                ..Default::default()
            },
        )
        .await
    }
    pub async fn startup_pairing_credential(&self) -> Result<AuthPairingCredentialResult> {
        if let Some(dev) = self.sessions.dev_auth() {
            match self.sessions.verify(&dev.credential).await {
                Ok(session) => {
                    return Ok(AuthPairingCredentialResult {
                        id: TrimmedNonEmptyString(session.session_id.0),
                        credential: TrimmedNonEmptyString(dev.credential.clone()),
                        label: Some(TrimmedNonEmptyString("Reusable dev token".into())),
                        expires_at: session.expires_at,
                        access: None,
                    });
                }
                Err(e) if e.is_credential() => {}
                Err(e) => return Err(e),
            }
        }
        self.pairing
            .issue_one_time_token(
                STANDARD_SCOPES,
                IssuePairing {
                    scopes: STANDARD_SCOPES.to_vec(),
                    startup: true,
                    subject: Some("administrative-bootstrap".into()),
                    ..Default::default()
                },
            )
            .await
    }
    pub async fn startup_pairing_url(&self, base: &str) -> Result<String> {
        pairing_url(base, &self.startup_pairing_credential().await?.credential.0)
    }
    pub async fn list_pairing_links(
        &self,
        exclude_subjects: Option<&[&str]>,
    ) -> Result<Vec<AuthPairingLink>> {
        let mut links = self.pairing.list_active().await?;
        let exclude = exclude_subjects.unwrap_or(&["administrative-bootstrap"]);
        links.retain(|l| {
            !exclude.contains(&l.subject.0.as_str())
                && !l.subject.0.starts_with(APPROVAL_SUBJECT_PREFIX)
        });
        for link in &mut links {
            link.permissions = Some(link.scopes.clone());
            link.scopes = legacy_scopes(&link.scopes);
        }
        Ok(links)
    }
    pub async fn create_browser_session(
        &self,
        request: &Request,
        credential: &str,
        metadata: AuthClientMetadata,
        previous: Option<&str>,
    ) -> Result<BrowserSession> {
        let secure = request
            .absolute_url()
            .is_some_and(|url| url.scheme() == "https");
        if let Some(dev) = self.sessions.dev_auth()
            && dev.matches(credential)
        {
            let session = self
                .sessions
                .verify(credential)
                .await
                .map_err(Error::invalid_session)?;
            let expires = iso(expires_after(self.now_ms(), SESSION_TTL_MS)?)?;
            let response = browser_response(&session.scopes, session.method, expires.clone());
            return Ok(BrowserSession {
                response,
                session_token: credential.into(),
                cookies: vec![
                    Cookie::session(dev.cookie_name.clone(), credential.into(), expires, secure),
                    Cookie::expire(self.sessions.cookie_name(), secure)?,
                ],
            });
        }
        let previous = if let Some(token) = previous {
            match self.sessions.verify(token).await {
                Ok(s) => Some(s),
                Err(e) if e.is_credential() => None,
                Err(e) => return Err(e),
            }
        } else {
            None
        };
        let grant = self
            .pairing
            .consume(credential, ConsumePairing::default())
            .await
            .map_err(Error::invalid_session)?;
        let mut client = metadata;
        if let Some(label) = grant.label.filter(|s| !s.is_empty()) {
            client.label = Some(TrimmedNonEmptyString(label));
        }
        let session = self
            .sessions
            .issue(IssueSession {
                subject: Some(grant.subject),
                scopes: Some(grant.scopes),
                client: Some(client),
                replace_session_id: previous
                    .filter(|s| s.method == ServerAuthSessionMethod::BrowserSessionCookie)
                    .map(|s| s.session_id),
                ..Default::default()
            })
            .await?;
        Ok(BrowserSession {
            response: browser_response(&session.scopes, session.method, session.expires_at.clone()),
            cookies: vec![Cookie::session(
                self.sessions.cookie_name(),
                session.token.clone(),
                session.expires_at.clone(),
                secure,
            )],
            session_token: session.token,
        })
    }
    async fn resolve_bootstrap(
        &self,
        credential: &str,
        input: ConsumePairing,
    ) -> Result<ResolvedBootstrap> {
        if let Some(dev) = self.sessions.dev_auth()
            && dev.matches(credential)
        {
            let session = self
                .sessions
                .verify(credential)
                .await
                .map_err(Error::invalid_session)?;
            if input
                .required_scopes
                .as_ref()
                .is_some_and(|r| !r.iter().all(|s| session.scopes.contains(s)))
            {
                return Err(Error::ScopeNotGranted);
            }
            return Ok(ResolvedBootstrap {
                scopes: session.scopes,
                subject: "reusable-dev-token-child".into(),
                label: None,
                source: GrantSource::Dev,
            });
        }
        let grant = self
            .pairing
            .consume(credential, input)
            .await
            .map_err(|e| match e {
                Error::Credential {
                    kind: crate::error::CredentialFailure::BootstrapCredentialScopeNotGrantedError,
                    ..
                } => Error::ScopeNotGranted,
                e => e.invalid_session(),
            })?;
        Ok(ResolvedBootstrap {
            scopes: grant.scopes,
            subject: grant.subject,
            label: grant.label,
            source: if grant.method == ServerAuthBootstrapMethod::DesktopBootstrap {
                GrantSource::Desktop
            } else {
                GrantSource::Pairing
            },
        })
    }
    pub async fn exchange_bootstrap(
        &self,
        request: &Request,
        credential: &str,
        requested: Option<Vec<AuthEnvironmentScope>>,
        metadata: AuthClientMetadata,
    ) -> Result<AuthAccessTokenResult> {
        let proof = request
            .header("dpop")
            .map(|_| crate::dpop::verify_request_proof(request, None, None, None, self.now_ms()))
            .transpose()?;
        let proof_key = proof.as_ref().map(|proof| proof.thumbprint.clone());
        let grant = self
            .resolve_bootstrap(
                credential,
                ConsumePairing {
                    proof_key_thumbprint: proof_key.clone(),
                    requested_scopes: requested.clone(),
                    ..Default::default()
                },
            )
            .await?;
        let ResolvedBootstrap {
            scopes,
            subject,
            label,
            source,
        } = grant;
        let granted = if let Some(requested) = requested {
            let mut narrowed = Vec::new();
            for s in requested {
                if scopes.contains(&s) && !narrowed.contains(&s) {
                    narrowed.push(s);
                }
            }
            narrowed
        } else {
            scopes
        };
        if granted.is_empty() {
            return Err(Error::ScopeNotGranted);
        }
        if let Some(proof) = proof {
            crate::dpop::record_request_proof(&self.secrets, &proof, self.now_ms()).await?;
        }
        let mut client = metadata;
        if let Some(label) = label.filter(|s| !s.is_empty()) {
            client.label = Some(TrimmedNonEmptyString(label));
        }
        let bound = proof_key.as_ref().is_some_and(|s| !s.is_empty());
        let session = self
            .sessions
            .issue(IssueSession {
                subject: Some(subject),
                scopes: Some(granted.clone()),
                method: Some(if bound {
                    ServerAuthSessionMethod::DpopAccessToken
                } else {
                    ServerAuthSessionMethod::BearerAccessToken
                }),
                proof_key_thumbprint: proof_key,
                ttl_ms: bound.then_some(3_600_000),
                client: Some(client),
                replace_active_for_subject_and_method: source == GrantSource::Desktop,
                ..Default::default()
            })
            .await?;
        Ok(AuthAccessTokenResult {
            access_token: TrimmedNonEmptyString(session.token),
            issued_token_type: AuthAccessTokenType::UrnIetfParamsOauthTokenTypeAccessToken,
            token_type: if bound {
                AuthTokenType::DPoP
            } else {
                AuthTokenType::Bearer
            },
            expires_in: EncodedNumber::Finite(
                ((epoch(&session.expires_at)? - self.now_ms()).max(0) / 1000).into(),
            ),
            scope: TrimmedNonEmptyString(oauth_scope(&granted)?),
        })
    }
    pub async fn issue_session(
        &self,
        subject: Option<String>,
        scopes: Option<Vec<AuthEnvironmentScope>>,
        label: Option<String>,
        ttl_ms: Option<i64>,
    ) -> Result<IssuedSession> {
        let mut client = default_metadata();
        client.device_type = AuthClientMetadataDeviceType::Bot;
        client.label = label.map(TrimmedNonEmptyString);
        self.sessions
            .issue(IssueSession {
                subject: Some(subject.unwrap_or_else(|| "cli-issued-session".into())),
                method: Some(ServerAuthSessionMethod::BearerAccessToken),
                scopes,
                client: Some(client),
                ttl_ms,
                ..Default::default()
            })
            .await
    }
    pub async fn list_client_sessions(
        &self,
        current: Option<&AuthSessionId>,
    ) -> Result<Vec<AuthClientSession>> {
        let mut clients = self.sessions.list_active().await?;
        clients.sort_by(|a, b| {
            b.scopes
                .contains(&AuthEnvironmentScope::AccessWrite)
                .cmp(&a.scopes.contains(&AuthEnvironmentScope::AccessWrite))
                .then_with(|| b.connected.cmp(&a.connected))
                .then_with(|| b.issued_at.cmp(&a.issued_at))
        });
        if let Some(current) = current {
            for session in &mut clients {
                session.current = session.session_id == *current;
                session.permissions = Some(session.scopes.clone());
                session.scopes = legacy_scopes(&session.scopes);
            }
        }
        Ok(clients)
    }
    pub async fn revoke_client(
        &self,
        current: &AuthSessionId,
        target: &AuthSessionId,
    ) -> Result<bool> {
        if current == target {
            return Err(Error::SelfRevocation);
        }
        self.sessions.revoke(target).await
    }
    pub async fn authenticate_http(&self, request: &Request) -> Result<VerifiedSession> {
        let selected = select_request_credential(
            request,
            &self.sessions.cookie_name(),
            self.sessions.legacy_cookie_name().as_deref(),
        );
        let selected = selected.or_else(|| {
            if request.header("authorization").is_some() {
                None
            } else {
                self.sessions
                    .dev_auth()
                    .and_then(|d| request.cookies.get(&d.cookie_name))
                    .map(|t| (t.as_str(), CredentialSource::DevCookie))
            }
        });
        let (token, source) = selected
            .filter(|(t, _)| !t.is_empty())
            .ok_or(Error::MissingCredential)?;
        if matches!(
            source,
            CredentialSource::Cookie | CredentialSource::LegacyCookie | CredentialSource::DevCookie
        ) && !browser_origin_allowed(
            request,
            &self.trusted_origins,
            self.config.trust_forwarded_host,
        ) {
            return Err(Error::invalid(
                "Browser sessions are only accepted from the app's own origin.",
            ));
        }
        let session = self
            .sessions
            .verify(token)
            .await
            .map_err(Error::invalid_session)?;
        reject_mcp(&session)?;
        let dpop = parse_authorization(request, "DPoP");
        if let Some(key) = &session.proof_key_thumbprint {
            if dpop != Some(token) {
                return Err(dpop_invalid(
                    "DPoP-bound access token requires DPoP authorization.",
                    DpopFailureReason::InvalidProof,
                ));
            }
            crate::dpop::verify_request(
                &self.secrets,
                request,
                None,
                Some(key),
                Some(token),
                self.now_ms(),
            )
            .await?;
        } else if dpop.is_some() {
            return Err(dpop_invalid(
                "DPoP authorization requires a proof-bound access token.",
                DpopFailureReason::InvalidProof,
            ));
        }
        Ok(session)
    }
    pub async fn session_state(&self, request: &Request) -> Result<AuthSessionState> {
        match self.authenticate_http(request).await {
            Ok(session) => Ok(AuthSessionState {
                authenticated: true,
                auth: self.descriptor(),
                scopes: Some(legacy_scopes(&session.scopes)),
                permissions: Some(session.scopes),
                session_method: Some(session.method),
                expires_at: Some(session.expires_at),
            }),
            Err(e) if e.is_credential() => Ok(AuthSessionState {
                authenticated: false,
                auth: self.descriptor(),
                scopes: None,
                permissions: None,
                session_method: None,
                expires_at: None,
            }),
            Err(e) => Err(e),
        }
    }
    pub async fn issue_websocket_ticket(
        &self,
        session: &VerifiedSession,
        audience: Option<AuthWebSocketTicketUse>,
    ) -> Result<AuthWebSocketTicketResult> {
        self.sessions
            .issue_websocket_token(
                &session.session_id,
                None,
                audience,
                if audience == Some(AuthWebSocketTicketUse::Rpc) {
                    session.proof_key_thumbprint.clone()
                } else {
                    None
                },
            )
            .await
    }
    pub async fn authenticate_websocket(
        &self,
        request: &Request,
        audience: AuthWebSocketTicketUse,
    ) -> Result<VerifiedSession> {
        if let Some(url) = request.absolute_url() {
            let query: BTreeMap<_, _> = url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            if let Some(token) = query.get("wsTicket").filter(|t| !t.trim().is_empty()) {
                let session = self
                    .sessions
                    .verify_websocket_token(token)
                    .await
                    .map_err(Error::invalid_session)?;
                reject_mcp(&session)?;
                if session.audience.is_some_and(|a| a != audience) {
                    return Err(Error::invalid(format!(
                        "WebSocket ticket was issued for the {} route.",
                        if session.audience == Some(AuthWebSocketTicketUse::Rpc) {
                            "rpc"
                        } else {
                            "media"
                        }
                    )));
                }
                if let Some(key) = &session.proof_key_thumbprint {
                    let proof = request
                        .header("dpop")
                        .or_else(|| query.get("dpop").map(String::as_str))
                        .ok_or_else(|| {
                            dpop_invalid(
                                "DPoP-bound WebSocket ticket requires a DPoP proof.",
                                DpopFailureReason::InvalidProof,
                            )
                        })?;
                    crate::dpop::verify_request(
                        &self.secrets,
                        request,
                        Some(proof),
                        Some(key),
                        Some(token),
                        self.now_ms(),
                    )
                    .await?;
                }
                return Ok(session);
            }
        }
        self.authenticate_http(request).await
    }
    pub async fn issue_mcp_session(
        &self,
        resource: &str,
        label: String,
        access: AuthMcpClientAccess,
        peer_link: bool,
        mut client: AuthClientMetadata,
    ) -> Result<IssuedSession> {
        let runtime = runtime_for_access(access);
        client.label = Some(TrimmedNonEmptyString(label));
        client.device_type = AuthClientMetadataDeviceType::Bot;
        self.sessions
            .issue(IssueSession {
                mcp_resource: Some(resource.into()),
                subject: Some("mcp-client".into()),
                method: Some(ServerAuthSessionMethod::BearerAccessToken),
                scopes: Some(mcp_scopes(access)),
                client: Some(client),
                runtime_mode_ceiling: runtime,
                peer_link,
                ..Default::default()
            })
            .await
    }
    pub async fn authenticate_mcp(&self, request: &Request) -> Result<McpClientSession> {
        let token = parse_authorization(request, "Bearer").ok_or(Error::MissingCredential)?;
        let mut url = request
            .absolute_url()
            .ok_or_else(|| Error::invalid("Invalid MCP request URL."))?;
        url.set_query(None);
        url.set_fragment(None);
        let resource = crate::oauth::canonical_mcp_resource(url.as_str())?;
        let session = self
            .sessions
            .verify_mcp(token, &resource, &[])
            .await
            .map_err(Error::invalid_session)?;
        let access = if session
            .scopes
            .contains(&AuthEnvironmentScope::OrchestrationOperate)
        {
            session
                .runtime_mode_ceiling
                .map(access_for_runtime)
                .unwrap_or(AuthMcpClientAccess::ApprovalRequired)
        } else {
            AuthMcpClientAccess::ReadOnly
        };
        Ok(McpClientSession {
            session_id: session.session_id,
            label: session
                .client
                .label
                .map(|l| l.0)
                .unwrap_or_else(|| "MCP client".into()),
            access,
            peer_link: session.peer_link,
        })
    }
    pub async fn consume_mcp_approval_code(
        &self,
        code: &str,
        access: AuthMcpClientAccess,
    ) -> Result<AuthMcpClientAccess> {
        let grant = self
            .resolve_bootstrap(
                code.trim(),
                ConsumePairing {
                    required_scopes: Some(mcp_scopes(access)),
                    mcp_approval: Some(access),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| match e {
                Error::ScopeNotGranted => Error::McpApprovalCode {
                    reason: McpApprovalReason::InsufficientScope,
                },
                e if e.is_credential() => Error::McpApprovalCode {
                    reason: McpApprovalReason::UnknownOrUsed,
                },
                e => e,
            })?;
        if grant.source == GrantSource::Desktop {
            return Err(Error::McpApprovalCode {
                reason: McpApprovalReason::NotAPairingCode,
            });
        }
        Ok(grant
            .subject
            .strip_prefix(APPROVAL_SUBJECT_PREFIX)
            .and_then(parse_access)
            .unwrap_or(access))
    }
}
fn browser_response(
    scopes: &[AuthEnvironmentScope],
    method: ServerAuthSessionMethod,
    expires_at: t3_contracts::wire::WireDateTime,
) -> AuthBrowserSessionResult {
    AuthBrowserSessionResult {
        authenticated: Authenticated,
        scopes: legacy_scopes(scopes),
        permissions: Some(scopes.to_vec()),
        session_method: method,
        expires_at,
    }
}
pub fn dpop_invalid(message: &str, reason: DpopFailureReason) -> Error {
    Error::InvalidCredential {
        diagnostic: message.into(),
        dpop_failure_reason: Some(reason),
    }
}
fn reject_mcp(session: &VerifiedSession) -> Result<()> {
    if session.subject == "mcp-client" {
        Err(Error::invalid(
            "MCP client sessions are only accepted by the MCP endpoint.",
        ))
    } else {
        Ok(())
    }
}
pub fn parse_requested_scopes(scope: &str) -> Result<Vec<AuthEnvironmentScope>> {
    let mut scopes = Vec::new();
    for token in scope.split(' ') {
        if token.is_empty()
            || !token
                .bytes()
                .all(|b| b == 0x21 || (0x23..=0x5b).contains(&b) || (0x5d..=0x7e).contains(&b))
        {
            return Err(Error::InvalidScope);
        }
        if let Ok(scope) =
            serde_json::from_value::<AuthEnvironmentScope>(serde_json::Value::String(token.into()))
            && scope != AuthEnvironmentScope::ReviewWrite
            && !scopes.contains(&scope)
        {
            scopes.push(scope);
        }
    }
    if scopes.is_empty() {
        Err(Error::InvalidScope)
    } else {
        Ok(scopes)
    }
}

fn runtime_for_access(
    access: AuthMcpClientAccess,
) -> Option<t3_contracts::provider_policy::RuntimeMode> {
    use t3_contracts::provider_policy::RuntimeMode as R;
    match access {
        AuthMcpClientAccess::ReadOnly => None,
        AuthMcpClientAccess::ApprovalRequired => Some(R::ApprovalRequired),
        AuthMcpClientAccess::AutoAcceptEdits => Some(R::AutoAcceptEdits),
        AuthMcpClientAccess::Auto => Some(R::Auto),
        AuthMcpClientAccess::FullAccess => Some(R::FullAccess),
    }
}
fn access_for_runtime(mode: t3_contracts::provider_policy::RuntimeMode) -> AuthMcpClientAccess {
    use t3_contracts::provider_policy::RuntimeMode as R;
    match mode {
        R::ApprovalRequired => AuthMcpClientAccess::ApprovalRequired,
        R::AutoAcceptEdits => AuthMcpClientAccess::AutoAcceptEdits,
        R::Auto => AuthMcpClientAccess::Auto,
        R::FullAccess => AuthMcpClientAccess::FullAccess,
    }
}
fn parse_access(value: &str) -> Option<AuthMcpClientAccess> {
    serde::Deserialize::deserialize(
        serde::de::value::StrDeserializer::<serde::de::value::Error>::new(value),
    )
    .ok()
}
