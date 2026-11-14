//! Destination-session checks shared by UI availability and execution.
pub mod connect_device;
use crate::{
    ClientError,
    types::{EnvironmentId, Generation},
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::{
    auth::{AuthAccessSnapshot, AuthAccessStreamEvent, AuthEnvironmentScope, AuthSessionState},
    client_rpc_permissions::client_rpc_required_scopes,
    rpc::WS_METHODS,
};

#[derive(Clone, PartialEq, Eq)]
pub struct SessionGrants {
    pub scopes: BTreeSet<String>,
    pub valid: bool,
}
impl std::fmt::Debug for SessionGrants {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionGrants")
            .field("valid", &self.valid)
            .field("scope_count", &self.scopes.len())
            .finish()
    }
}
pub trait PermissionPolicy {
    fn required_scopes(&self, method: &str, input: &Value) -> Option<Vec<String>>;
}
#[derive(Debug, Clone, Copy, Default)]
pub struct ContractPermissionPolicy;
impl PermissionPolicy for ContractPermissionPolicy {
    fn required_scopes(&self, method: &str, input: &Value) -> Option<Vec<String>> {
        let descriptor = WS_METHODS.iter().find(|entry| entry.tag == method)?;
        let mut scopes = client_rpc_required_scopes(method, Some(input)).ok()?;
        if method == "orchestration.launchThread" {
            scopes.push(AuthEnvironmentScope::OrchestrationOperate);
        }
        match method {
            "orchestration.dispatchCommand" => scopes.push(match input.get("type")?.as_str()? {
                "thread.visit" | "thread.mark-unread" => AuthEnvironmentScope::OrchestrationRead,
                _ => AuthEnvironmentScope::OrchestrationOperate,
            }),
            "projects.mutate" => scopes.push(AuthEnvironmentScope::OrchestrationOperate),
            "projects.writeFile" => scopes.push(AuthEnvironmentScope::FilesystemWrite),
            _ => {},
        }
        if let Some(scope) = descriptor.required_scope
            && !scopes.contains(&scope)
        {
            scopes.push(scope);
        }
        scopes.into_iter().map(scope_name).collect()
    }
}
fn scope_name(scope: AuthEnvironmentScope) -> Option<String> {
    serde_json::to_value(scope)
        .ok()?
        .as_str()
        .map(str::to_owned)
}
pub fn authorize(
    policy: &impl PermissionPolicy,
    environment: Option<&EnvironmentId>,
    session: Option<&SessionGrants>,
    method: &str,
    input: &Value,
) -> Result<(), ClientError> {
    let known = WS_METHODS.iter().find(|entry| entry.tag == method);
    let denied = || ClientError::PermissionDenied {
        method: known.map_or("<unknown>", |entry| entry.tag).into(),
    };
    let Some(grants) = session.filter(|grants| grants.valid) else {
        return Err(denied());
    };
    if environment.is_none() || known.is_none() {
        return Err(denied());
    }
    let required = policy.required_scopes(method, input).ok_or_else(denied)?;
    // Extensions can add requirements, but cannot bypass the contract guards.
    let contract = ContractPermissionPolicy
        .required_scopes(method, input)
        .ok_or_else(denied)?;
    if required
        .iter()
        .chain(&contract)
        .all(|scope| grants.scopes.contains(scope))
    {
        Ok(())
    } else {
        Err(denied())
    }
}
pub fn available(
    environment: Option<&EnvironmentId>,
    session: Option<&SessionGrants>,
    method: &str,
    input: &Value,
) -> bool {
    authorize(
        &ContractPermissionPolicy,
        environment,
        session,
        method,
        input,
    )
    .is_ok()
}
pub fn session_grants_scope(session: &AuthSessionState, scope: AuthEnvironmentScope) -> bool {
    if !session.authenticated {
        return false;
    }
    if let Some(permissions) = &session.permissions {
        return permissions.contains(&scope);
    }
    let scopes = session.scopes.as_deref().unwrap_or_default();
    if scopes.contains(&scope) {
        return true;
    }
    if session.auth.server_update_scope.is_some() {
        return false;
    }
    use AuthEnvironmentScope::*;
    let parent = match scope {
        FilesystemRead | DiagnosticsRead => OrchestrationRead,
        SettingsWrite | ProvidersManage | EnvironmentMaintain | PreviewOperate
        | SourceControlWrite | FilesystemWrite => OrchestrationOperate,
        TerminalRead => TerminalOperate,
        _ => return false,
    };
    scopes.contains(&parent)
}
impl From<&AuthSessionState> for SessionGrants {
    fn from(session: &AuthSessionState) -> Self {
        let mut candidates = Vec::new();
        candidates.extend([AuthEnvironmentScope::FilesystemWrite, AuthEnvironmentScope::OrchestrationOperate, AuthEnvironmentScope::OrchestrationRead]);
        candidates.extend(WS_METHODS.iter().filter_map(|method| method.required_scope));
        candidates.extend(
            t3_contracts::client_rpc_permissions::CLIENT_GUARDED_RPC_SCOPES
                .iter()
                .map(|entry| entry.scope()),
        );
        candidates.extend(session.scopes.iter().flatten().copied());
        candidates.extend(session.permissions.iter().flatten().copied());
        Self {
            valid: session.authenticated,
            scopes: candidates
                .into_iter()
                .filter(|scope| session_grants_scope(session, *scope))
                .filter_map(scope_name)
                .collect(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct DestinationSessions {
    sessions: BTreeMap<EnvironmentId, (Generation, Option<SessionGrants>)>,
}
impl DestinationSessions {
    pub fn begin(&mut self, environment: EnvironmentId, generation: Generation) {
        self.sessions.insert(environment, (generation, None));
    }
    pub fn update(
        &mut self,
        environment: &EnvironmentId,
        generation: Generation,
        session: &AuthSessionState,
    ) -> bool {
        let Some((current, grants)) = self.sessions.get_mut(environment) else {
            return false;
        };
        if *current != generation {
            return false;
        }
        *grants = Some(session.into());
        true
    }
    pub fn disconnect(&mut self, environment: &EnvironmentId, generation: Generation) {
        if self
            .sessions
            .get(environment)
            .is_some_and(|(current, _)| *current == generation)
        {
            self.sessions.remove(environment);
        }
    }
    pub fn grants(&self, environment: &EnvironmentId) -> Option<&SessionGrants> {
        self.sessions.get(environment)?.1.as_ref()
    }
    pub fn authorize(
        &self,
        environment: Option<&EnvironmentId>,
        method: &str,
        input: &Value,
    ) -> Result<(), ClientError> {
        authorize(
            &ContractPermissionPolicy,
            environment,
            environment.and_then(|id| self.grants(id)),
            method,
            input,
        )
    }
}
pub fn apply_auth_access_stream_event(
    current: &mut AuthAccessSnapshot,
    event: AuthAccessStreamEvent,
) {
    match event {
        AuthAccessStreamEvent::AuthAccessStreamSnapshotEvent(event) => *current = event.payload,
        AuthAccessStreamEvent::AuthAccessStreamPairingLinkUpsertedEvent(event) => {
            current
                .pairing_links
                .retain(|item| item.id != event.payload.id);
            current.pairing_links.push(event.payload);
        }
        AuthAccessStreamEvent::AuthAccessStreamPairingLinkRemovedEvent(event) => current
            .pairing_links
            .retain(|item| item.id != event.payload.id),
        AuthAccessStreamEvent::AuthAccessStreamClientUpsertedEvent(event) => {
            current
                .client_sessions
                .retain(|item| item.session_id != event.payload.session_id);
            current.client_sessions.push(event.payload);
        }
        AuthAccessStreamEvent::AuthAccessStreamClientRemovedEvent(event) => current
            .client_sessions
            .retain(|item| item.session_id != event.payload.session_id),
    }
}
#[derive(Debug, Clone, Default)]
pub struct SessionRetry {
    failures: u32,
    generation: Generation,
}
impl SessionRetry {
    pub fn begin(&mut self, generation: Generation) {
        if self.generation != generation {
            self.generation = generation;
            self.failures = 0;
        }
    }
    pub fn succeeded(&mut self) {
        self.failures = 0;
    }
    pub fn failed(&mut self) -> u64 {
        let delay = 2_000u64
            .saturating_mul(1u64 << self.failures.min(5))
            .min(60_000);
        self.failures = self.failures.saturating_add(1);
        delay
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn session() -> AuthSessionState {
        serde_json::from_value(json!({"authenticated":true,"auth":{"policy":"remote-reachable","bootstrapMethods":[],"sessionMethods":[],"sessionCookieName":"session"},"scopes":["orchestration:operate","orchestration:read"]})).unwrap()
    }
    #[test]
    fn denies_unknown_destination_session_and_failed_session_even_for_unguarded_methods() {
        let id = EnvironmentId("env".into());
        let grants = SessionGrants {
            scopes: BTreeSet::new(),
            valid: true,
        };
        assert!(available(
            Some(&id),
            Some(&grants),
            "server.probe",
            &json!({})
        ));
        assert!(!available(None, Some(&grants), "server.probe", &json!({})));
        assert!(!available(Some(&id), None, "server.probe", &json!({})));
        assert!(!available(
            Some(&id),
            Some(&SessionGrants {
                valid: false,
                ..grants.clone()
            }),
            "server.probe",
            &json!({})
        ));
        let error = authorize(
            &ContractPermissionPolicy,
            Some(&id),
            Some(&grants),
            "SECRET_UNKNOWN_METHOD",
            &json!({}),
        )
        .unwrap_err();
        assert!(!format!("{error:?} {error}").contains("SECRET_UNKNOWN_METHOD"));
    }
    #[test]
    fn every_guarded_contract_method_requires_its_own_scope() {
        let id = EnvironmentId("env".into());
        for entry in t3_contracts::client_rpc_permissions::CLIENT_GUARDED_RPC_SCOPES {
            let input = match entry.tag() {
                "git.runStackedAction" => {
                    json!({"cwd":"/repo","action":"push","actionId":"action"})
                }
                "git.preparePullRequestThread" => {
                    json!({"cwd":"/repo","mode":"local","reference":"123"})
                }
                _ => json!({}),
            };
            let required = ContractPermissionPolicy
                .required_scopes(entry.tag(), &input)
                .unwrap();
            assert!(required.contains(&scope_name(entry.scope()).unwrap()));
            let mut grants = SessionGrants {
                valid: true,
                scopes: required.into_iter().collect(),
            };
            assert!(
                available(Some(&id), Some(&grants), entry.tag(), &input),
                "{}",
                entry.tag()
            );
            grants.scopes.clear();
            assert!(!available(Some(&id), Some(&grants), entry.tag(), &input));
        }
    }
    #[test]
    fn input_dependent_permissions_and_invalid_input_fail_closed() {
        let policy = ContractPermissionPolicy;
        assert_eq!(
            policy
                .required_scopes(
                    "git.runStackedAction",
                    &json!({"cwd":"/repo", "action":"create_pr", "threadId":"thread", "actionId":"action"})
                )
                .unwrap()
                .len(),
            2
        );
        assert_eq!(policy.required_scopes("git.preparePullRequestThread", &json!({"cwd":"/repo", "mode":"worktree", "threadId":"thread", "reference":"123"})).unwrap().len(),2);
        assert!(
            policy
                .required_scopes("git.runStackedAction", &json!({"action":"SECRET"}))
                .is_none()
        );
    }
    #[test]
    fn exact_permissions_override_legacy_parents_and_new_servers_disable_expansion() {
        let mut state = session();
        assert!(session_grants_scope(
            &state,
            AuthEnvironmentScope::SettingsWrite
        ));
        state.permissions = Some(vec![]);
        assert!(!session_grants_scope(
            &state,
            AuthEnvironmentScope::SettingsWrite
        ));
        assert!(!session_grants_scope(
            &state,
            AuthEnvironmentScope::OrchestrationRead
        ));
        state.permissions = None;
        state.auth.server_update_scope =
            Some(t3_contracts::auth::ServerUpdateScope::EnvironmentMaintain);
        assert!(!session_grants_scope(
            &state,
            AuthEnvironmentScope::SettingsWrite
        ));
        assert!(session_grants_scope(
            &state,
            AuthEnvironmentScope::OrchestrationRead
        ));
        state.authenticated = false;
        assert!(!session_grants_scope(
            &state,
            AuthEnvironmentScope::OrchestrationRead
        ));
    }
    #[test]
    fn destination_sessions_recheck_current_grants_and_ignore_stale_completions() {
        let a = EnvironmentId("a".into());
        let b = EnvironmentId("b".into());
        let mut sessions = DestinationSessions::default();
        sessions.begin(a.clone(), Generation(1));
        sessions.begin(b.clone(), Generation(1));
        assert!(sessions.update(&a, Generation(1), &session()));
        assert!(
            sessions
                .authorize(Some(&a), "preview.close", &json!({}))
                .is_ok()
        );
        assert!(
            sessions
                .authorize(Some(&b), "preview.close", &json!({}))
                .is_err()
        );
        sessions.begin(a.clone(), Generation(2));
        assert!(!sessions.update(&a, Generation(1), &session()));
        assert!(
            sessions
                .authorize(Some(&a), "preview.close", &json!({}))
                .is_err()
        );
        sessions.update(&a, Generation(2), &session());
        let mut revoked = session();
        revoked.permissions = Some(vec![]);
        sessions.update(&a, Generation(2), &revoked);
        assert!(
            sessions
                .authorize(Some(&a), "preview.close", &json!({}))
                .is_err()
        );
        sessions.disconnect(&a, Generation(1));
        assert!(sessions.grants(&a).is_some());
        sessions.disconnect(&a, Generation(2));
        assert!(sessions.grants(&a).is_none());
    }
    #[test]
    fn retries_back_off_reset_on_session_and_success() {
        let mut retry = SessionRetry::default();
        assert_eq!(
            (0..7).map(|_| retry.failed()).collect::<Vec<_>>(),
            vec![2000, 4000, 8000, 16000, 32000, 60000, 60000]
        );
        retry.begin(Generation(1));
        assert_eq!(retry.failed(), 2000);
        retry.succeeded();
        assert_eq!(retry.failed(), 2000);
    }
    #[test]
    fn public_permission_types_do_not_render_untrusted_scopes() {
        let value = SessionGrants {
            valid: true,
            scopes: BTreeSet::from(["MARKER_SECRET".into()]),
        };
        assert!(!format!("{value:?}").contains("MARKER_SECRET"));
    }
    #[test]
    fn access_stream_accumulates_upserts_removals_and_replacements() {
        let mut snapshot: AuthAccessSnapshot =
            serde_json::from_value(json!({"pairingLinks":[],"clientSessions":[]})).unwrap();
        let event = |revision, label| {
            serde_json::from_value(json!({"version":1,"revision":revision,"type":"pairingLinkUpserted","payload":{"id":"link","scopes":["orchestration:read"],"subject":"subject","label":label,"createdAt":"2036-04-07T00:00:00.000Z","expiresAt":"2036-04-07T00:05:00.000Z"}})).unwrap()
        };
        apply_auth_access_stream_event(&mut snapshot, event(1, "Phone"));
        apply_auth_access_stream_event(&mut snapshot, event(2, "Renamed"));
        assert_eq!(snapshot.pairing_links.len(), 1);
        apply_auth_access_stream_event(&mut snapshot,serde_json::from_value(json!({"version":1,"revision":3,"type":"pairingLinkRemoved","payload":{"id":"link"}})).unwrap());
        assert!(snapshot.pairing_links.is_empty());
    }
}

#[derive(Clone)]
pub struct EnvironmentHttpClient {
    platform: std::rc::Rc<dyn crate::platform::Platform>,
    prepared: crate::authorization::AuthorizedEnvironment,
    signer: Option<std::rc::Rc<dyn crate::relay::DpopSigner>>,
    remote: Option<crate::authorization::RemoteAuthorization>,
    relay_route: bool,
    authorization_revision: Option<u64>,
}
impl std::fmt::Debug for EnvironmentHttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EnvironmentHttpClient([REDACTED])")
    }
}
impl EnvironmentHttpClient {
    pub fn new(
        platform: std::rc::Rc<dyn crate::platform::Platform>,
        prepared: crate::authorization::AuthorizedEnvironment,
        signer: Option<std::rc::Rc<dyn crate::relay::DpopSigner>>,
        remote: Option<crate::authorization::RemoteAuthorization>,
        relay_route: bool,
    ) -> Self {
        let authorization_revision = remote
            .as_ref()
            .and_then(|remote| remote.session_revision(&prepared.session));
        Self {
            platform,
            prepared,
            signer,
            remote,
            relay_route,
            authorization_revision,
        }
    }
    pub async fn execute<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        timeout_ms: u64,
    ) -> Result<crate::authorization::Sensitive<T>, crate::authorization::AuthorizationError> {
        self.execute_validated(method, path, body, timeout_ms, |_| false)
            .await
    }
    async fn execute_validated<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        timeout_ms: u64,
        unauthorized: impl Fn(&T) -> bool,
    ) -> Result<crate::authorization::Sensitive<T>, crate::authorization::AuthorizationError> {
        use crate::authorization::{
            AuthorizationError as Error, Sensitive, http_url, response_json,
        };
        self.exchange(
            method,
            |base| http_url(base, path),
            body,
            timeout_ms,
            crate::authorization::MAX_AUTH_RESPONSE_BYTES,
            |response| {
                let result = response_json::<T>(response);
                let renew = match &result {
                    Err(Error::CredentialRejected) => true,
                    Ok(response) => unauthorized(response),
                    _ => false,
                };
                (result.map(Sensitive), renew)
            },
        )
        .await
    }
    /// One request whose response comes back whatever its status, for routes that answer
    /// with more than JSON success (snapshots: 404 means missing). `url` builds the request URL
    /// from the current HTTP base, which a relay route renews with its token. A rejected DPoP
    /// credential is renewed once, exactly as for [`Self::execute`].
    pub async fn send(
        &self,
        method: &str,
        url: impl Fn(&str) -> Result<String, crate::authorization::AuthorizationError>,
        timeout_ms: u64,
        max_response_bytes: usize,
    ) -> Result<crate::platform::HttpResponse, crate::authorization::AuthorizationError> {
        self.exchange(
            method,
            url,
            None,
            timeout_ms,
            max_response_bytes,
            |response| {
                let renew = crate::authorization::rejects_credential(&response);
                (Ok(response), renew)
            },
        )
        .await
    }
    /// Authorizes and sends until `read` accepts the answer. `read` returns the result and
    /// whether the credential was refused; a refused DPoP credential is renewed once, and a
    /// second refusal is `CredentialRejected`. Other credentials return `read`'s result as is.
    async fn exchange<R>(
        &self,
        method: &str,
        url: impl Fn(&str) -> Result<String, crate::authorization::AuthorizationError>,
        body: Option<&Value>,
        timeout_ms: u64,
        max_response_bytes: usize,
        read: impl Fn(
            crate::platform::HttpResponse,
        ) -> (Result<R, crate::authorization::AuthorizationError>, bool),
    ) -> Result<R, crate::authorization::AuthorizationError> {
        use crate::authorization::{
            AuthorizationError as Error, HttpAuthorization, auth_headers, request, send_http,
        };
        use futures::{
            FutureExt,
            future::{Either, select},
        };
        let operation = async {
            let mut rejected = None::<String>;
            loop {
                let mut base = self.prepared.http_base_url.clone();
                let mut authorization = self.prepared.authorization.clone();
                if matches!(authorization, HttpAuthorization::Dpop { .. }) {
                    let remote = self.remote.as_ref().ok_or(Error::SessionChanged)?;
                    remote
                        .validate_session_at(&self.prepared.session, self.authorization_revision)
                        .await?;
                    let (current, current_auth) = remote
                        .authorize_dpop_http(&self.prepared.session, rejected.as_deref())
                        .await?;
                    if self.relay_route {
                        base = current;
                    }
                    authorization = current_auth;
                }
                let url = url(&base)?;
                let mut headers =
                    auth_headers(&authorization, method, &url, self.signer.as_deref())
                        .await?
                        .into_inner();
                headers.insert(
                    t3_contracts::environment::ORCHESTRATION_PROTOCOL_HEADER.into(),
                    t3_contracts::environment::ORCHESTRATION_PROTOCOL_VERSION_TEXT.into(),
                );
                let body = match body {
                    Some(body) => {
                        headers.insert("content-type".into(), "application/json".into());
                        serde_json::to_vec(body).map_err(|_| Error::InvalidResponse)?
                    }
                    None => vec![],
                };
                let credentials = if matches!(authorization, HttpAuthorization::Cookie) {
                    crate::platform::Credentials::Include
                } else {
                    crate::platform::Credentials::Omit
                };
                if let Some(remote) = &self.remote {
                    remote
                        .validate_session_at(&self.prepared.session, self.authorization_revision)
                        .await?;
                }
                let mut request = request(method, url, headers, body, credentials);
                request.max_response_bytes = max_response_bytes;
                let response = send_http(self.platform.as_ref(), request, timeout_ms).await?;
                if let Some(remote) = &self.remote {
                    remote
                        .validate_session_at(&self.prepared.session, self.authorization_revision)
                        .await?;
                }
                let (result, refused) = read(response);
                if refused && let HttpAuthorization::Dpop { access_token, .. } = &authorization {
                    if rejected.is_none() {
                        rejected = Some(access_token.expose().clone());
                        continue;
                    }
                    return Err(Error::CredentialRejected);
                }
                return result;
            }
        }
        .boxed_local();
        match select(operation, self.platform.sleep(timeout_ms)).await {
            Either::Left((result, _)) => result,
            Either::Right(_) => Err(Error::Timeout),
        }
    }
    pub async fn fetch_session_state(
        &self,
        timeout_ms: Option<u64>,
    ) -> Result<AuthSessionState, crate::authorization::AuthorizationError> {
        let mut timeout = timeout_ms.unwrap_or(6_000).min(60_000);
        loop {
            match self
                .execute_validated::<AuthSessionState>(
                    "GET",
                    "/api/auth/session",
                    None,
                    timeout,
                    |state| !state.authenticated,
                )
                .await
            {
                Ok(state) => return Ok(state.into_inner()),
                Err(crate::authorization::AuthorizationError::Timeout) if timeout < 60_000 => {
                    timeout = timeout.saturating_mul(2).min(60_000)
                }
                Err(error) => return Err(error),
            }
        }
    }
    pub async fn create_pairing_credential(
        &self,
        input: &t3_contracts::auth::AuthCreatePairingCredentialInput,
    ) -> Result<
        crate::authorization::Sensitive<t3_contracts::auth::AuthPairingCredentialResult>,
        crate::authorization::AuthorizationError,
    > {
        let body = serde_json::to_value(input)
            .map_err(|_| crate::authorization::AuthorizationError::InvalidResponse)?;
        self.execute("POST", "/api/auth/pairing-token", Some(&body), 8_000)
            .await
    }
    pub async fn revoke_pairing_link(
        &self,
        input: &t3_contracts::auth::AuthRevokePairingLinkInput,
    ) -> Result<
        t3_contracts::environment_http::AuthPairingLinkRevokeResult,
        crate::authorization::AuthorizationError,
    > {
        let body = serde_json::to_value(input)
            .map_err(|_| crate::authorization::AuthorizationError::InvalidResponse)?;
        self.execute("POST", "/api/auth/pairing-links/revoke", Some(&body), 8_000)
            .await
            .map(crate::authorization::Sensitive::into_inner)
    }
    pub async fn revoke_client(
        &self,
        input: &t3_contracts::auth::AuthRevokeClientSessionInput,
    ) -> Result<
        t3_contracts::environment_http::AuthClientSessionRevokeResult,
        crate::authorization::AuthorizationError,
    > {
        let body = serde_json::to_value(input)
            .map_err(|_| crate::authorization::AuthorizationError::InvalidResponse)?;
        self.execute("POST", "/api/auth/clients/revoke", Some(&body), 8_000)
            .await
            .map(crate::authorization::Sensitive::into_inner)
    }
    pub async fn revoke_other_clients(
        &self,
    ) -> Result<
        t3_contracts::environment_http::AuthOtherClientSessionsRevokeResult,
        crate::authorization::AuthorizationError,
    > {
        self.execute("POST", "/api/auth/clients/revoke-others", None, 8_000)
            .await
            .map(crate::authorization::Sensitive::into_inner)
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use crate::{
        authorization::{
            AuthorizationError, AuthorizedEnvironment, HttpAuthorization, RemoteTokenStore,
            Sensitive,
            tests::{TestPlatform, TestSigner, identity, remote_token, script_mint, service},
        },
        platform::Credentials,
    };
    use futures::{FutureExt, executor::block_on};
    use serde_json::json;
    use std::rc::Rc;
    fn prepared(authorization: HttpAuthorization) -> AuthorizedEnvironment {
        AuthorizedEnvironment {
            session: identity(),
            http_base_url: "https://direct.test".into(),
            socket_url: Sensitive("wss://direct.test/ws".into()),
            authorization,
            descriptor: None,
        }
    }
    fn dpop() -> HttpAuthorization {
        HttpAuthorization::Dpop {
            access_token: Sensitive("expired-prepared".into()),
            expires_at_epoch_ms: 1,
        }
    }
    fn session() -> Value {
        json!({"authenticated":true,"auth":{"policy":"remote-reachable","bootstrapMethods":[],"sessionMethods":[],"sessionCookieName":"session"},"permissions":["orchestration:read"]})
    }
    #[test]
    fn cookies_include_credentials_bearers_use_headers_and_all_paths_send_protocol_header() {
        block_on(async {
            for authorization in [
                HttpAuthorization::Cookie,
                HttpAuthorization::Bearer(Sensitive("MARKER_SECRET".into())),
            ] {
                let platform = Rc::new(TestPlatform::default());
                let client = EnvironmentHttpClient::new(
                    platform.clone(),
                    prepared(authorization.clone()),
                    None,
                    None,
                    false,
                );
                platform.respond(200, json!({"value":1}));
                client
                    .execute::<Value>("POST", "/api/data", Some(&json!({"input":"value"})), 10000)
                    .await
                    .unwrap();
                let requests = platform.requests.borrow();
                let request = &requests[0];
                assert_eq!(
                    request.headers[t3_contracts::environment::ORCHESTRATION_PROTOCOL_HEADER],
                    "2"
                );
                if matches!(authorization, HttpAuthorization::Cookie) {
                    assert_eq!(request.credentials, Credentials::Include);
                    assert!(!request.headers.contains_key("authorization"));
                } else {
                    assert_eq!(request.credentials, Credentials::Omit);
                    assert_eq!(request.headers["authorization"], "Bearer MARKER_SECRET");
                }
                assert!(!format!("{client:?} {request:?}").contains("MARKER_SECRET"));
            }
        });
    }
    #[test]
    fn raw_send_renews_a_refused_dpop_credential_once_and_returns_other_statuses() {
        block_on(async {
            let refused = json!({"_tag":"EnvironmentAuthInvalidError","code":"auth_invalid","reason":"invalid_credential","traceId":"t"});
            // The renewed token answers: its response comes back whatever the status. Refused
            // again: the request ends instead of looping.
            for (status, answer) in [
                (404, json!({"_tag":"EnvironmentResourceNotFoundError"})),
                (401, refused.clone()),
            ] {
                let platform = Rc::new(TestPlatform::default());
                let signer = Rc::new(TestSigner::default());
                let remote = service(platform.clone(), signer.clone());
                RemoteTokenStore::new(platform.clone())
                    .put(&remote_token("cached", 3600000))
                    .await
                    .unwrap();
                let client = EnvironmentHttpClient::new(
                    platform.clone(),
                    prepared(dpop()),
                    Some(signer),
                    Some(remote),
                    true,
                );
                platform.respond(401, refused.clone());
                script_mint(&platform, "fresh");
                platform.respond(status, answer);
                let url = |base: &str| {
                    Ok(format!(
                        "{}/api/orchestration/shell?resumeFrom=4",
                        base.trim_end_matches('/')
                    ))
                };
                let result = client.send("GET", url, 10_000, 1 << 20).await;
                if status == 404 {
                    assert_eq!(result.unwrap().status, 404);
                } else {
                    assert_eq!(result.unwrap_err(), AuthorizationError::CredentialRejected);
                }
                let requests = platform.requests.borrow();
                assert_eq!(requests.len(), 6);
                assert_eq!(requests[0].headers["authorization"], "DPoP cached");
                assert_eq!(requests[5].headers["authorization"], "DPoP fresh");
                assert_eq!(
                    requests[5].url,
                    "https://managed.test/api/orchestration/shell?resumeFrom=4"
                );
                assert_eq!(requests[5].max_response_bytes, 1 << 20);
            }
        });
    }
    #[test]
    fn dpop_renews_before_send_retries_rejected_credential_once_and_preserves_direct_origin() {
        block_on(async {
            for relay_route in [false, true] {
                let platform = Rc::new(TestPlatform::default());
                let signer = Rc::new(TestSigner::default());
                let remote = service(platform.clone(), signer.clone());
                RemoteTokenStore::new(platform.clone())
                    .put(&remote_token("cached", 3600000))
                    .await
                    .unwrap();
                let client = EnvironmentHttpClient::new(
                    platform.clone(),
                    prepared(dpop()),
                    Some(signer),
                    Some(remote.clone()),
                    relay_route,
                );
                platform.respond(401,json!({"_tag":"EnvironmentAuthInvalidError","code":"auth_invalid","reason":"invalid_credential","traceId":"MARKER_SECRET"}));
                script_mint(&platform, "fresh");
                platform.respond(200, json!({"ok":true}));
                client
                    .execute::<Value>("GET", "/api/data", None, 10000)
                    .await
                    .unwrap();
                {
                    let requests = platform.requests.borrow();
                    assert_eq!(requests.len(), 6);
                    assert_eq!(requests[0].headers["authorization"], "DPoP cached");
                    assert_eq!(requests[5].headers["authorization"], "DPoP fresh");
                    assert_eq!(
                        requests[5].url,
                        if relay_route {
                            "https://managed.test/api/data"
                        } else {
                            "https://direct.test/api/data"
                        }
                    );
                }
                remote.invalidate_sessions();
                remote.begin_session(&identity()).await.unwrap();
                RemoteTokenStore::new(platform.clone())
                    .put(&remote_token("replacement", 3_600_000))
                    .await
                    .unwrap();
                platform.respond(200, json!({"ok":true}));
                assert_eq!(
                    client
                        .execute::<Value>("GET", "/api/data", None, 10_000)
                        .await
                        .unwrap_err(),
                    AuthorizationError::SessionChanged
                );
                assert_eq!(platform.requests.borrow().len(), 6);
            }
        });
    }
    #[test]
    fn session_unauthenticated_200_renews_before_permissions_and_persistent_rejection_is_an_error()
    {
        block_on(async {
            for recover in [true, false] {
                let platform = Rc::new(TestPlatform::default());
                let signer = Rc::new(TestSigner::default());
                let remote = service(platform.clone(), signer.clone());
                RemoteTokenStore::new(platform.clone())
                    .put(&remote_token("cached", 3600000))
                    .await
                    .unwrap();
                let client = EnvironmentHttpClient::new(
                    platform.clone(),
                    prepared(dpop()),
                    Some(signer),
                    Some(remote),
                    true,
                );
                let mut unauthenticated = session();
                unauthenticated["authenticated"] = json!(false);
                platform.respond(200, unauthenticated.clone());
                script_mint(&platform, "fresh");
                platform.respond(200, if recover { session() } else { unauthenticated });
                let result = client.fetch_session_state(None).await;
                if recover {
                    assert!(result.unwrap().authenticated);
                } else {
                    assert_eq!(result.unwrap_err(), AuthorizationError::CredentialRejected);
                }
                assert_eq!(platform.requests.borrow().len(), 6);
            }
        });
    }
    #[test]
    fn absent_remote_or_signer_never_sends_the_prepared_dpop_token() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let client =
                EnvironmentHttpClient::new(platform.clone(), prepared(dpop()), None, None, true);
            assert_eq!(
                client.fetch_session_state(None).await.unwrap_err(),
                AuthorizationError::SessionChanged
            );
            assert!(platform.requests.borrow().is_empty());
            let remote = service(platform.clone(), Rc::default());
            RemoteTokenStore::new(platform.clone())
                .put(&remote_token("cached", 3600000))
                .await
                .unwrap();
            let client = EnvironmentHttpClient::new(
                platform.clone(),
                prepared(dpop()),
                None,
                Some(remote),
                true,
            );
            assert_eq!(
                client.fetch_session_state(None).await.unwrap_err(),
                AuthorizationError::Proof
            );
            assert!(platform.requests.borrow().is_empty());
        });
    }
    #[test]
    fn session_timeouts_double_to_sixty_seconds_without_turning_busy_hosts_into_denials() {
        let platform = Rc::new(TestPlatform::default());
        let _responses = (0..5).map(|_| platform.pending()).collect::<Vec<_>>();
        let client = EnvironmentHttpClient::new(
            platform.clone(),
            prepared(HttpAuthorization::Cookie),
            None,
            None,
            false,
        );
        let mut request = client.fetch_session_state(None).boxed_local();
        assert!(request.as_mut().now_or_never().is_none());
        assert_eq!(platform.requests.borrow().len(), 1);
        for (index, timeout) in [6000, 12000, 24000, 48000].into_iter().enumerate() {
            platform.fake.advance(timeout);
            assert!(request.as_mut().now_or_never().is_none());
            assert_eq!(platform.requests.borrow().len(), index + 2);
        }
        platform.fake.advance(60000);
        assert_eq!(block_on(request).unwrap_err(), AuthorizationError::Timeout);
    }
    #[test]
    fn invalid_payloads_and_undeclared_http_status_are_sanitized() {
        block_on(async {
            let platform = Rc::new(TestPlatform::default());
            let client = EnvironmentHttpClient::new(
                platform.clone(),
                prepared(HttpAuthorization::Bearer(Sensitive("MARKER_SECRET".into()))),
                None,
                None,
                false,
            );
            platform.respond(200, json!({"MARKER_SECRET":"wrong-shape"}));
            assert_eq!(
                client.fetch_session_state(None).await.unwrap_err(),
                AuthorizationError::InvalidResponse
            );
            platform.respond(502, json!({"error":"MARKER_SECRET"}));
            let error = client.fetch_session_state(None).await.unwrap_err();
            assert_eq!(error, AuthorizationError::HttpStatus(502));
            assert!(!format!("{error:?} {error}").contains("MARKER_SECRET"));
        });
    }
}
