//! Environment catalog and generation-checked session installation.
//!
//! `register`, route changes and `broadcast` return `RegistryChange`s that the
//! connection runtime must execute. A registry used for connections shares its
//! `RemoteAuthorization` through `with_authorization` or `load_authorized`.
//! Removal revokes authorization immediately; `save` atomically removes the
//! catalog routes, credentials, profiles, DPoP tokens and routing grants.
//! Cleanup entries still describe cache and host-owned data cleanup.
//! `save` persists catalog edits and `set_enabled_persisted` saves before applying
//! the enabled intent. Other in-memory mutations require explicit persistence.
//! [`crate::connection::connect`] owns the explicit-origin connection runtime.

use crate::{
    ClientError,
    authorization::AuthorizedEnvironment,
    connection::{
        AttemptId, ConnectionAction, ConnectionEvent, ConnectionMode, Route, Supervisor, Time,
    },
    platform::{
        Credentials, HttpRequest, Platform, StorageArea, StorageCondition, StorageKey,
        StorageTransaction, StorageWrite,
    },
    types::{EnvironmentId, Generation, ProjectId, ThreadId},
};
use futures::{
    future::{Either, select},
    pin_mut,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_contracts::{
    environment::{ExecutionEnvironmentDescriptor, ScopedProjectRef, ScopedThreadRef},
    server::ServerConfig,
};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentRegistration {
    pub id: EnvironmentId,
    pub label: String,
    pub routes: Vec<Route>,
    pub enabled: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionIdentity {
    pub environment: EnvironmentId,
    pub generation: Generation,
    pub account: Option<String>,
    pub dpop_thumbprint: Option<String>,
}

pub fn scope_project_ref(environment_id: EnvironmentId, project_id: ProjectId) -> ScopedProjectRef {
    ScopedProjectRef {
        environment_id,
        project_id,
    }
}
pub fn scope_thread_ref(environment_id: EnvironmentId, thread_id: ThreadId) -> ScopedThreadRef {
    ScopedThreadRef {
        environment_id,
        thread_id,
    }
}
pub fn scoped_project_key(reference: &ScopedProjectRef) -> String {
    format!("{}:{}", reference.environment_id.0, reference.project_id.0)
}
pub fn scoped_thread_key(reference: &ScopedThreadRef) -> String {
    format!("{}:{}", reference.environment_id.0, reference.thread_id.0)
}
fn parse_scoped_key(key: &str) -> Option<(EnvironmentId, &str)> {
    let (environment, local) = key.split_once(':')?;
    if environment.is_empty() || local.is_empty() {
        return None;
    }
    Some((EnvironmentId(environment.into()), local))
}
pub fn parse_scoped_project_key(key: &str) -> Option<ScopedProjectRef> {
    let (environment, local) = parse_scoped_key(key)?;
    Some(scope_project_ref(environment, ProjectId(local.into())))
}
pub fn parse_scoped_thread_key(key: &str) -> Option<ScopedThreadRef> {
    let (environment, local) = parse_scoped_key(key)?;
    Some(scope_thread_ref(environment, ThreadId(local.into())))
}

pub fn normalize_http_base_url(raw: &str) -> Result<String, ClientError> {
    let mut url = Url::parse(raw).map_err(|_| invalid_endpoint())?;
    match url.scheme() {
        "ws" => {
            url.set_scheme("http").map_err(|()| invalid_endpoint())?;
        }
        "wss" => {
            url.set_scheme("https").map_err(|()| invalid_endpoint())?;
        }
        "http" | "https" => {}
        _ => return Err(invalid_endpoint()),
    }
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.into())
}
pub fn derive_ws_base_url(raw: &str) -> Result<String, ClientError> {
    let mut url = Url::parse(&normalize_http_base_url(raw)?).map_err(|_| invalid_endpoint())?;
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme).map_err(|()| invalid_endpoint())?;
    Ok(url.into())
}
pub fn environment_endpoint_url(base: &str, pathname: &str) -> Result<String, ClientError> {
    let mut url = Url::parse(base).map_err(|_| invalid_endpoint())?;
    url.set_path(pathname);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.into())
}
fn invalid_endpoint() -> ClientError {
    ClientError::InvalidPayload("invalid environment endpoint".into())
}

pub async fn fetch_remote_environment_descriptor(
    platform: &dyn Platform,
    base: &str,
    timeout_ms: Option<u64>,
) -> Result<ExecutionEnvironmentDescriptor, ClientError> {
    let url = environment_endpoint_url(base, "/.well-known/t3/environment")?;
    let parsed = Url::parse(&url).map_err(|_| invalid_endpoint())?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(invalid_endpoint());
    }
    let request = platform.http(HttpRequest {
        method: "GET".into(),
        url,
        // No `cache-control`: the server's CORS allow-list (TypeScript `httpCors.ts`) does not
        // include it, so a cross-origin browser preflight would refuse the descriptor read. The
        // TypeScript client sends none, and the response carries no validators to cache on.
        headers: BTreeMap::new(),
        body: Vec::new(),
        credentials: Credentials::Omit,
        max_response_bytes: 1024 * 1024,
    });
    let timeout = platform.sleep(timeout_ms.unwrap_or(10_000));
    pin_mut!(request, timeout);
    let response = match select(request, timeout).await {
        Either::Left((response, _)) => response?,
        Either::Right(_) => {
            return Err(ClientError::Platform(
                crate::platform::PlatformError::Network(
                    "environment descriptor request timed out".into(),
                ),
            ));
        }
    };
    if !(200..300).contains(&response.status) {
        return Err(ClientError::InvalidPayload(format!(
            "environment descriptor returned HTTP {}",
            response.status
        )));
    }
    serde_json::from_slice(&response.body)
        .map_err(|_| ClientError::InvalidPayload("invalid environment descriptor".into()))
}

pub struct LiveSession<S> {
    pub identity: SessionIdentity,
    pub route_id: String,
    pub endpoint: S,
}

pub struct EnvironmentHandle<S> {
    registration: EnvironmentRegistration,
    platform_owned: bool,
    supervisor: Supervisor,
    session: Option<LiveSession<S>>,
    prepared: Option<AuthorizedEnvironment>,
    config: Option<Rc<ServerConfig>>,
    account: Option<String>,
    dpop_thumbprint: Option<String>,
}

impl<S> EnvironmentHandle<S> {
    pub fn platform_event(
        &mut self,
        event: crate::platform::PlatformEvent,
        time: Time,
        random: f64,
    ) -> Vec<ConnectionAction> {
        let actions = self.supervisor.platform_event(event, time, random);
        if self
            .session
            .as_ref()
            .is_some_and(|session| !self.supervisor.accepts_session(session.identity.generation))
        {
            self.session = None;
            self.prepared = None;
        }
        actions
    }
    pub fn registration(&self) -> &EnvironmentRegistration {
        &self.registration
    }
    pub fn supervisor(&self) -> &Supervisor {
        &self.supervisor
    }
    pub fn session(&self) -> Option<&LiveSession<S>> {
        self.session.as_ref()
    }
    pub fn prepared(&self) -> Option<&AuthorizedEnvironment> {
        self.prepared.as_ref()
    }
    pub fn config(&self) -> Option<&Rc<ServerConfig>> {
        self.config.as_ref()
    }
    pub fn pending_identity(&self) -> Option<SessionIdentity> {
        self.supervisor.active_attempt()?;
        Some(SessionIdentity {
            environment: self.registration.id.clone(),
            generation: self.supervisor.state().generation,
            account: self.account.clone(),
            dpop_thumbprint: self.dpop_thumbprint.clone(),
        })
    }
    pub fn update_identity(
        &mut self,
        account: Option<String>,
        dpop_thumbprint: Option<String>,
        time: Time,
    ) -> Vec<ConnectionAction> {
        if self.account == account && self.dpop_thumbprint == dpop_thumbprint {
            return Vec::new();
        }
        self.account = account;
        self.dpop_thumbprint = dpop_thumbprint;
        self.handle(
            ConnectionEvent::Wake(crate::connection::Wakeup::CredentialsChanged),
            time,
            0.5,
        )
    }
    pub fn handle(
        &mut self,
        event: ConnectionEvent,
        time: Time,
        random: f64,
    ) -> Vec<ConnectionAction> {
        if let ConnectionEvent::SetEnabled(enabled) = &event {
            self.registration.enabled = *enabled;
        }
        let actions = self.supervisor.handle(event, time, random);
        if self
            .session
            .as_ref()
            .is_some_and(|s| !self.supervisor.accepts_session(s.identity.generation))
        {
            self.session = None;
            self.prepared = None;
        }
        actions
    }
    pub fn install(
        &mut self,
        attempt: AttemptId,
        route_id: &str,
        identity: SessionIdentity,
        endpoint: S,
        time: Time,
    ) -> Result<Vec<ConnectionAction>, ClientError> {
        if self.supervisor.active_attempt() != Some(attempt)
            || self.pending_identity().as_ref() != Some(&identity)
            || !self
                .supervisor
                .attempt_routes()
                .iter()
                .any(|r| r.id == route_id)
        {
            return Err(ClientError::Cancelled);
        }
        let actions = self.handle(
            ConnectionEvent::Ready {
                attempt,
                route_id: route_id.into(),
            },
            time,
            0.5,
        );
        if !self.supervisor.accepts_session(identity.generation) {
            return Err(ClientError::Cancelled);
        }
        self.session = Some(LiveSession {
            identity,
            route_id: route_id.into(),
            endpoint,
        });
        Ok(actions)
    }
    pub fn install_authorized(
        &mut self,
        attempt: AttemptId,
        route_id: &str,
        prepared: AuthorizedEnvironment,
        endpoint: S,
        time: Time,
    ) -> Result<Vec<ConnectionAction>, ClientError> {
        if prepared
            .descriptor
            .as_ref()
            .is_some_and(|d| d.environment_id != self.registration.id)
        {
            return Err(ClientError::EnvironmentMismatch);
        }
        let actions = self.install(attempt, route_id, prepared.session.clone(), endpoint, time)?;
        self.prepared = Some(prepared);
        Ok(actions)
    }
    pub fn set_config(&mut self, generation: Generation, config: ServerConfig) -> bool {
        if !self.supervisor.accepts_session(generation)
            || config.environment.environment_id != self.registration.id
        {
            return false;
        }
        self.config = Some(Rc::new(config));
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvironmentCleanup {
    Route {
        environment: EnvironmentId,
        route_id: String,
    },
    Environment(EnvironmentId),
    RoutingTrust(EnvironmentId),
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistryChange {
    pub actions: Vec<(EnvironmentId, ConnectionAction)>,
    pub cleanup: Vec<EnvironmentCleanup>,
}
impl RegistryChange {
    fn actions(environment: &EnvironmentId, actions: Vec<ConnectionAction>) -> Self {
        Self {
            actions: actions
                .into_iter()
                .map(|a| (environment.clone(), a))
                .collect(),
            cleanup: Vec::new(),
        }
    }
}

pub struct EnvironmentRegistry<S> {
    entries: BTreeMap<EnvironmentId, EnvironmentHandle<S>>,
    order: Vec<EnvironmentId>,
    retired: BTreeMap<EnvironmentId, (Generation, u64)>,
    catalog_bytes: Option<Vec<u8>>,
    authorization: Option<crate::authorization::RemoteAuthorization>,
    /// Environments with a saved record that could not be read at load (an undecryptable or
    /// unknown-shape profile, credential or token); they stay listed so they can be removed.
    damaged: BTreeSet<EnvironmentId>,
}
impl<S> Default for EnvironmentRegistry<S> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            order: Vec::new(),
            retired: BTreeMap::new(),
            damaged: BTreeSet::new(),
            catalog_bytes: None,
            authorization: None,
        }
    }
}
impl<S> EnvironmentRegistry<S> {
    pub fn bind_authorization(&mut self, authorization: crate::authorization::RemoteAuthorization) {
        self.authorization = Some(authorization);
    }
    pub fn with_authorization(authorization: crate::authorization::RemoteAuthorization) -> Self {
        Self {
            authorization: Some(authorization),
            ..Self::default()
        }
    }
    pub async fn authorize_attempt(
        &mut self,
        environment: &EnvironmentId,
        authorization: crate::authorization::RemoteAuthorization,
    ) -> Result<SessionIdentity, ClientError> {
        let handle = self
            .entries
            .get(environment)
            .ok_or(ClientError::EnvironmentMismatch)?;
        let attempt = handle
            .supervisor
            .active_attempt()
            .ok_or(ClientError::Cancelled)?;
        let generation = handle.supervisor.state().generation;
        self.authorization = Some(authorization.clone());
        authorization.allow_environment(environment);
        let identity = authorization
            .capture_session(environment.clone(), generation)
            .await
            .map_err(|error| ClientError::InvalidPayload(error.to_string()))?;
        let handle = self
            .entries
            .get_mut(environment)
            .ok_or(ClientError::EnvironmentMismatch)?;
        if handle.supervisor.active_attempt() != Some(attempt) {
            authorization.clear_session(environment);
            return Err(ClientError::Cancelled);
        }
        handle.account = identity.account.clone();
        handle.dpop_thumbprint = identity.dpop_thumbprint.clone();
        Ok(identity)
    }
    pub async fn load_authorized(
        platform: &dyn Platform,
        authorization: crate::authorization::RemoteAuthorization,
        network: crate::platform::NetworkStatus,
        time: Time,
    ) -> Result<(Self, RegistryChange), ClientError> {
        let mut registry = Self::load(platform, network, time).await?;
        registry.authorization = Some(authorization.clone());
        let change = registry.broadcast(ConnectionEvent::Connect, time, 0.5);
        let environments: Vec<_> = registry
            .entries
            .iter()
            .filter(|(_, handle)| handle.supervisor.active_attempt().is_some())
            .map(|(id, _)| id.clone())
            .collect();
        for environment in environments {
            registry
                .authorize_attempt(&environment, authorization.clone())
                .await?;
        }
        Ok((registry, change))
    }
    /// Whether a saved record of `environment` could not be read when the catalog loaded.
    pub fn is_damaged(&self, environment: &EnvironmentId) -> bool {
        self.damaged.contains(environment)
    }
    pub fn entries(&self) -> impl Iterator<Item = &EnvironmentRegistration> {
        self.order
            .iter()
            .filter_map(|id| self.entries.get(id).map(|h| &h.registration))
    }
    pub fn get(&self, id: &EnvironmentId) -> Option<&EnvironmentHandle<S>> {
        self.entries.get(id)
    }
    pub fn get_mut(&mut self, id: &EnvironmentId) -> Option<&mut EnvironmentHandle<S>> {
        self.entries.get_mut(id)
    }
    pub fn register(
        &mut self,
        registration: EnvironmentRegistration,
        platform_owned: bool,
        network: crate::platform::NetworkStatus,
        time: Time,
    ) -> Result<RegistryChange, ClientError> {
        self.register_with_intent(registration, platform_owned, network, time, true)
    }
    fn register_with_intent(
        &mut self,
        mut registration: EnvironmentRegistration,
        platform_owned: bool,
        network: crate::platform::NetworkStatus,
        time: Time,
        connect: bool,
    ) -> Result<RegistryChange, ClientError> {
        validate_registration(&registration)?;
        let id = registration.id.clone();
        if let Some(authorization) = &self.authorization {
            authorization.allow_environment(&id);
        }
        if let Some(existing) = self.entries.get_mut(&id) {
            if existing.platform_owned && !platform_owned {
                return Err(ClientError::PermissionDenied {
                    method: "register platform environment".into(),
                });
            }
            if !platform_owned {
                let mut routes = existing.registration.routes.clone();
                for route in registration.routes {
                    routes.retain(|old| {
                        old.id == route.id
                            || old.http_base.trim_end_matches('/')
                                != route.http_base.trim_end_matches('/')
                            || old.mode != route.mode
                    });
                    crate::connection::upsert_route(&mut routes, route);
                }
                registration.routes = routes;
                registration.enabled = existing.registration.enabled;
            }
            if existing.registration == registration && existing.platform_owned == platform_owned {
                return Ok(RegistryChange::default());
            }
            let old_key = crate::connection::routes_connection_key(&existing.registration.routes);
            let routes_changed = existing.registration.routes != registration.routes;
            existing.platform_owned = platform_owned;
            let mut actions = Vec::new();
            if routes_changed {
                actions.extend(existing.handle(
                    ConnectionEvent::SetRoutes(registration.routes.clone()),
                    time,
                    0.5,
                ));
            }
            if existing.registration.enabled != registration.enabled {
                actions.extend(existing.handle(
                    ConnectionEvent::SetEnabled(registration.enabled),
                    time,
                    0.5,
                ));
            }
            let mut change = RegistryChange::actions(&id, actions);
            change.cleanup.extend(
                existing
                    .registration
                    .routes
                    .iter()
                    .filter(|r| !registration.routes.iter().any(|kept| kept.id == r.id))
                    .map(|r| EnvironmentCleanup::Route {
                        environment: id.clone(),
                        route_id: r.id.clone(),
                    }),
            );
            if old_key != crate::connection::routes_connection_key(&registration.routes) {
                change
                    .cleanup
                    .push(EnvironmentCleanup::RoutingTrust(id.clone()));
            }
            existing.registration = registration;
            return Ok(change);
        }
        let (generation, serial) = self.retired.get(&id).copied().unwrap_or((Generation(0), 0));
        let primary = platform_owned
            && registration
                .routes
                .iter()
                .any(|r| r.mode == ConnectionMode::Local);
        let supervisor = Supervisor::new(
            registration.routes.clone(),
            primary,
            registration.enabled,
            network,
        )
        .with_label(registration.label.clone())
        .with_lineage(generation, serial);
        let mut handle = EnvironmentHandle {
            registration,
            platform_owned,
            supervisor,
            session: None,
            prepared: None,
            config: None,
            account: None,
            dpop_thumbprint: None,
        };
        let actions = if connect && handle.registration.enabled {
            handle.handle(ConnectionEvent::Connect, time, 0.5)
        } else {
            Vec::new()
        };
        self.order.push(id.clone());
        self.entries.insert(id.clone(), handle);
        Ok(RegistryChange::actions(&id, actions))
    }
    pub fn assert_known_address(
        &self,
        route: &Route,
    ) -> Result<(), crate::connection::ConnectionFailure> {
        if let Some(existing) = self.entries.get(&route.environment)
            && !existing.platform_owned
            && !existing.registration.routes.iter().any(|r| {
                r.mode == route.mode
                    && r.http_base.trim_end_matches('/') == route.http_base.trim_end_matches('/')
            })
        {
            return Err(crate::connection::ConnectionFailure::new(
                crate::connection::FailureKind::Configuration,
                format!(
                    "That address claims to be {}, which is already saved. Add it as a route from that environment's settings instead.",
                    existing.registration.label
                ),
            ));
        }
        Ok(())
    }
    pub async fn register_prepared(
        &mut self,
        platform: &dyn Platform,
        prepared: crate::connection::PreparedRegistration,
        known_address_only: bool,
        enable: bool,
        network: crate::platform::NetworkStatus,
        time: Time,
    ) -> Result<RegistryChange, ClientError> {
        let crate::connection::PreparedRegistration {
            mut registration,
            profile,
            credential,
        } = prepared;
        validate_registration(&registration)?;
        if profile.environment_id() != &registration.id
            || !registration
                .routes
                .iter()
                .any(|r| r.id == profile.connection_id())
        {
            return Err(ClientError::EnvironmentMismatch);
        }
        let had_runtime = self
            .entries
            .get(&registration.id)
            .is_some_and(|h| h.registration.enabled);
        if let Some(existing) = self.entries.get(&registration.id) {
            if existing.platform_owned {
                return Ok(RegistryChange::default());
            }
            if known_address_only {
                for route in &registration.routes {
                    self.assert_known_address(route)
                        .map_err(|f| ClientError::InvalidPayload(f.detail))?;
                }
            }
            let mut routes = existing.registration.routes.clone();
            for route in &registration.routes {
                routes.retain(|old| {
                    old.id == route.id
                        || old.http_base.trim_end_matches('/')
                            != route.http_base.trim_end_matches('/')
                        || old.mode != route.mode
                });
                crate::connection::upsert_route(&mut routes, route.clone());
            }
            registration.routes = routes;
            registration.enabled = enable || existing.registration.enabled;
        }
        let mut entries: Vec<_> = self
            .entries()
            .filter(|r| self.entries.get(&r.id).is_some_and(|h| !h.platform_owned))
            .cloned()
            .collect();
        if let Some(existing) = entries.iter_mut().find(|r| r.id == registration.id) {
            *existing = registration.clone();
        } else {
            entries.push(registration.clone());
        }
        let profile_key = StorageKey {
            namespace: "connection-profile".into(),
            environment: None,
            record: profile.connection_id().into(),
        };
        let credential_key = StorageKey {
            namespace: "connection-credential".into(),
            environment: None,
            record: profile.connection_id().into(),
        };
        let profile_value = crate::persistence::Record::new(
            "connection-profile",
            None,
            profile.connection_id(),
            1,
            &profile,
        )
        .encode()
        .map_err(|_| ClientError::InvalidPayload("could not encode connection profile".into()))?;
        let credential_value = crate::persistence::Record::new(
            "connection-credential",
            None,
            profile.connection_id(),
            1,
            serde_json::json!({"connectionId":profile.connection_id(),"credential":credential}),
        )
        .encode()
        .map_err(|_| {
            ClientError::InvalidPayload("could not encode connection credential".into())
        })?;
        let overrides = vec![
            StorageWrite {
                key: profile_key,
                value: Some(profile_value),
            },
            StorageWrite {
                key: credential_key,
                value: Some(credential_value),
            },
        ];
        self.catalog_bytes = Some(
            save_catalog_with(platform, entries, self.catalog_bytes.clone(), overrides).await?,
        );
        let id = registration.id.clone();
        let mut change = self.register(registration, false, network, time)?;
        if enable {
            let handle = self
                .entries
                .get_mut(&id)
                .ok_or(ClientError::EnvironmentMismatch)?;
            change.actions.extend(
                handle
                    .handle(ConnectionEvent::SetEnabled(true), time, 0.5)
                    .into_iter()
                    .map(|a| (id.clone(), a)),
            );
        }
        if had_runtime {
            let handle = self
                .entries
                .get_mut(&id)
                .ok_or(ClientError::EnvironmentMismatch)?;
            change.actions.extend(
                handle
                    .handle(ConnectionEvent::Restart, time, 0.5)
                    .into_iter()
                    .map(|a| (id.clone(), a)),
            );
        }
        Ok(change)
    }
    pub fn reconcile_platform(
        &mut self,
        registrations: Vec<EnvironmentRegistration>,
        network: crate::platform::NetworkStatus,
        time: Time,
    ) -> Result<RegistryChange, ClientError> {
        for registration in &registrations {
            validate_registration(registration)?;
        }
        let desired: BTreeSet<_> = registrations.iter().map(|r| r.id.clone()).collect();
        if desired.len() != registrations.len() {
            return Err(ClientError::InvalidPayload(
                "duplicate platform environment".into(),
            ));
        }
        let removed: Vec<_> = self
            .entries
            .iter()
            .filter(|(id, h)| h.platform_owned && !desired.contains(id))
            .map(|(id, _)| id.clone())
            .collect();
        let mut change = RegistryChange::default();
        for id in removed {
            if let Some(handle) = self.entries.get_mut(&id) {
                handle.platform_owned = false;
            }
            let removed = self.remove(&id, time)?;
            change.actions.extend(removed.actions);
            change.cleanup.extend(removed.cleanup);
        }
        for registration in registrations {
            let installed = self.register(registration, true, network, time)?;
            change.actions.extend(installed.actions);
            change.cleanup.extend(installed.cleanup);
        }
        Ok(change)
    }
    pub fn remove(
        &mut self,
        id: &EnvironmentId,
        time: Time,
    ) -> Result<RegistryChange, ClientError> {
        let handle = self
            .entries
            .get_mut(id)
            .ok_or(ClientError::EnvironmentMismatch)?;
        if handle.platform_owned {
            return Err(ClientError::PermissionDenied {
                method: "remove platform environment".into(),
            });
        }
        if let Some(authorization) = &self.authorization {
            authorization.forget_environment(id);
        }
        let mut change =
            RegistryChange::actions(id, handle.handle(ConnectionEvent::Disconnect, time, 0.5));
        let (generation, serial) = handle.supervisor.lineage();
        self.retired
            .insert(id.clone(), (Generation(generation.0 + 1), serial));
        change
            .cleanup
            .extend(
                handle
                    .registration
                    .routes
                    .iter()
                    .map(|r| EnvironmentCleanup::Route {
                        environment: id.clone(),
                        route_id: r.id.clone(),
                    }),
            );
        change
            .cleanup
            .push(EnvironmentCleanup::Environment(id.clone()));
        self.entries.remove(id);
        self.order.retain(|key| key != id);
        Ok(change)
    }
    pub fn reorder_routes(
        &mut self,
        id: &EnvironmentId,
        route_ids: &[String],
        time: Time,
    ) -> Result<RegistryChange, ClientError> {
        let handle = self
            .entries
            .get_mut(id)
            .ok_or(ClientError::EnvironmentMismatch)?;
        if handle.platform_owned {
            return Err(ClientError::PermissionDenied {
                method: "reorder platform routes".into(),
            });
        }
        let routes = &handle.registration.routes;
        let unique: BTreeSet<_> = route_ids.iter().collect();
        if unique.len() != routes.len()
            || route_ids.len() != routes.len()
            || !routes.iter().all(|r| unique.contains(&r.id))
        {
            return Err(ClientError::InvalidPayload(
                "The route order must list every saved route once.".into(),
            ));
        }
        let next: Vec<_> = route_ids
            .iter()
            .filter_map(|id| routes.iter().find(|r| &r.id == id).cloned())
            .collect();
        if next == *routes {
            return Ok(RegistryChange::default());
        }
        handle.registration.routes = next.clone();
        Ok(RegistryChange::actions(
            id,
            handle.handle(ConnectionEvent::SetRoutes(next), time, 0.5),
        ))
    }
    pub fn remove_route(
        &mut self,
        id: &EnvironmentId,
        route_id: &str,
        time: Time,
    ) -> Result<RegistryChange, ClientError> {
        let handle = self
            .entries
            .get(id)
            .ok_or(ClientError::EnvironmentMismatch)?;
        if handle.platform_owned {
            return Err(ClientError::PermissionDenied {
                method: "remove platform route".into(),
            });
        }
        if !handle.registration.routes.iter().any(|r| r.id == route_id) {
            return Ok(RegistryChange::default());
        }
        let remaining =
            crate::connection::routes_after_removing(&handle.registration.routes, route_id);
        if remaining.is_empty() {
            return self.remove(id, time);
        }
        let removed: Vec<_> = handle
            .registration
            .routes
            .iter()
            .filter(|r| !remaining.iter().any(|kept| kept.id == r.id))
            .map(|r| r.id.clone())
            .collect();
        let handle = self
            .entries
            .get_mut(id)
            .ok_or(ClientError::EnvironmentMismatch)?;
        handle.registration.routes = remaining.clone();
        let mut change = RegistryChange::actions(
            id,
            handle.handle(ConnectionEvent::SetRoutes(remaining), time, 0.5),
        );
        change.cleanup.extend(
            removed
                .into_iter()
                .map(|route_id| EnvironmentCleanup::Route {
                    environment: id.clone(),
                    route_id,
                }),
        );
        change
            .cleanup
            .push(EnvironmentCleanup::RoutingTrust(id.clone()));
        Ok(change)
    }
    pub fn broadcast(&mut self, event: ConnectionEvent, time: Time, random: f64) -> RegistryChange {
        RegistryChange {
            actions: self
                .order
                .iter()
                .filter_map(|id| {
                    self.entries
                        .get_mut(id)
                        .map(|handle| (id.clone(), handle.handle(event.clone(), time, random)))
                })
                .flat_map(|(id, actions)| actions.into_iter().map(move |a| (id.clone(), a)))
                .collect(),
            cleanup: Vec::new(),
        }
    }
    pub async fn save(&mut self, platform: &dyn Platform) -> Result<(), ClientError> {
        let entries: Vec<_> = self
            .entries()
            .filter(|r| self.entries.get(&r.id).is_some_and(|h| !h.platform_owned))
            .cloned()
            .collect();
        self.catalog_bytes =
            Some(save_catalog(platform, entries, self.catalog_bytes.clone()).await?);
        if let Some(authorization) = &self.authorization {
            authorization.flush_cleanup().await.map_err(|_| {
                ClientError::InvalidPayload(
                    "could not finish environment authorization cleanup".into(),
                )
            })?;
        }
        Ok(())
    }
    pub async fn set_enabled_persisted(
        &mut self,
        platform: &dyn Platform,
        id: &EnvironmentId,
        enabled: bool,
        time: Time,
    ) -> Result<RegistryChange, ClientError> {
        let handle = self
            .entries
            .get(id)
            .ok_or(ClientError::EnvironmentMismatch)?;
        if handle.registration.enabled == enabled {
            return Ok(RegistryChange::default());
        }
        if !handle.platform_owned {
            let next: Vec<_> = self
                .entries()
                .filter(|r| self.entries.get(&r.id).is_some_and(|h| !h.platform_owned))
                .map(|r| {
                    let mut r = r.clone();
                    if &r.id == id {
                        r.enabled = enabled;
                    }
                    r
                })
                .collect();
            self.catalog_bytes =
                Some(save_catalog(platform, next, self.catalog_bytes.clone()).await?);
        }
        let handle = self
            .entries
            .get_mut(id)
            .ok_or(ClientError::EnvironmentMismatch)?;
        Ok(RegistryChange::actions(
            id,
            handle.handle(ConnectionEvent::SetEnabled(enabled), time, 0.5),
        ))
    }
    /// Restores catalog data without starting attempts; broadcast `Connect` or use `load_authorized`.
    pub async fn load(
        platform: &dyn Platform,
        network: crate::platform::NetworkStatus,
        time: Time,
    ) -> Result<Self, ClientError> {
        let key = catalog_key();
        let Some(bytes) = platform.get(StorageArea::Ordinary, key.clone()).await? else {
            return Ok(Self::default());
        };
        let loaded = match crate::persistence::load_connection_catalog(platform).await {
            Ok(Some(loaded)) => loaded,
            Ok(None) => {
                return Err(ClientError::InvalidPayload(
                    "connection catalog disappeared during load".into(),
                ));
            }
            Err(crate::persistence::PersistenceError::Platform(error)) => {
                return Err(ClientError::Platform(error));
            }
            // Unreadable: kept aside and replaced with an empty catalog, so pairing works again.
            Err(_) => {
                return Ok(Self {
                    catalog_bytes: Some(quarantine_catalog(platform, bytes, time).await?),
                    ..Self::default()
                });
            }
        };
        if loaded.dropped_targets > 0 {
            // Keep the original beside the catalog: its unknown targets are gone from the next save.
            quarantine_copy(platform, &bytes, time).await?;
        }
        let doc = loaded.document;
        let mut damaged = BTreeSet::new();
        for label in &loaded.damaged {
            let Some((kind, id)) = label.split_once(' ') else {
                continue;
            };
            match kind {
                "connection-profile" | "connection-credential" => damaged.extend(
                    doc.targets
                        .iter()
                        .filter(|target| target["connectionId"] == id)
                        .filter_map(|target| target["environmentId"].as_str())
                        .map(|environment| EnvironmentId(environment.into())),
                ),
                _ => {
                    damaged.insert(EnvironmentId(id.into()));
                }
            }
        }
        let mut registry = Self::default();
        let mut registrations: Vec<EnvironmentRegistration> = Vec::new();
        for target in doc.targets {
            let environment = EnvironmentId(
                target["environmentId"]
                    .as_str()
                    .ok_or_else(invalid_endpoint)?
                    .into(),
            );
            let label = target["label"]
                .as_str()
                .ok_or_else(invalid_endpoint)?
                .to_owned();
            let tag = target["_tag"].as_str().ok_or_else(invalid_endpoint)?;
            let route_id = if tag == "RelayConnectionTarget" {
                "relay"
            } else {
                target["connectionId"]
                    .as_str()
                    .ok_or_else(invalid_endpoint)?
            };
            let profile = doc
                .profiles
                .iter()
                .find(|p| p["connectionId"] == route_id && p["environmentId"] == environment.0);
            let mode = match tag {
                "RelayConnectionTarget" => ConnectionMode::Relay,
                "SshConnectionTarget" => ConnectionMode::Ssh,
                "BearerConnectionTarget" => ConnectionMode::Remote,
                _ => return Err(invalid_endpoint()),
            };
            let route = Route {
                id: route_id.into(),
                environment: environment.clone(),
                mode,
                http_base: profile
                    .and_then(|p| p["httpBaseUrl"].as_str())
                    .unwrap_or("")
                    .into(),
                ws_base: profile
                    .and_then(|p| p["wsBaseUrl"].as_str())
                    .unwrap_or("")
                    .into(),
            };
            if let Some(existing) = registrations.iter_mut().find(|r| r.id == environment) {
                if !existing.routes.iter().any(|r| r.id == route.id) {
                    existing.routes.push(route);
                }
            } else {
                let enabled = !doc.disabled_environment_ids.contains(&environment);
                registrations.push(EnvironmentRegistration {
                    id: environment,
                    label,
                    routes: vec![route],
                    enabled,
                });
            }
        }
        for registration in registrations {
            registry.register_with_intent(registration, false, network, time, false)?;
        }
        registry.catalog_bytes = Some(bytes);
        registry.damaged = damaged;
        Ok(registry)
    }
}

fn quarantine_key(time: Time) -> StorageKey {
    StorageKey::new(
        "connection-catalog-quarantine",
        None,
        time.epoch_ms.to_string(),
    )
}
async fn quarantine_copy(
    platform: &dyn Platform,
    bytes: &[u8],
    time: Time,
) -> Result<(), ClientError> {
    platform
        .write(
            StorageArea::Ordinary,
            vec![StorageWrite {
                key: quarantine_key(time),
                value: Some(bytes.to_vec()),
            }],
        )
        .await?;
    Ok(())
}
/// Moves an unreadable catalog aside and starts an empty one, unless it changed meanwhile.
async fn quarantine_catalog(
    platform: &dyn Platform,
    bytes: Vec<u8>,
    time: Time,
) -> Result<Vec<u8>, ClientError> {
    let empty = crate::persistence::Record::new(
        "connection-catalog",
        None,
        "current",
        1,
        serde_json::json!({"schemaVersion": 1, "targets": [], "disabledEnvironmentIds": []}),
    )
    .encode()
    .map_err(|_| ClientError::InvalidPayload("could not encode connection catalog".into()))?;
    let committed = platform
        .transaction(StorageTransaction {
            conditions: vec![StorageCondition {
                area: StorageArea::Ordinary,
                key: catalog_key(),
                expected: Some(bytes.clone()),
            }],
            ordinary: vec![
                StorageWrite {
                    key: quarantine_key(time),
                    value: Some(bytes),
                },
                StorageWrite {
                    key: catalog_key(),
                    value: Some(empty.clone()),
                },
            ],
            secure: vec![],
        })
        .await?;
    if !committed {
        return Err(ClientError::InvalidPayload(
            "connection catalog changed concurrently".into(),
        ));
    }
    Ok(empty)
}
fn catalog_key() -> StorageKey {
    StorageKey {
        namespace: "connection-catalog".into(),
        environment: None,
        record: "current".into(),
    }
}
async fn save_catalog(
    platform: &dyn Platform,
    entries: Vec<EnvironmentRegistration>,
    expected: Option<Vec<u8>>,
) -> Result<Vec<u8>, ClientError> {
    save_catalog_with(platform, entries, expected, Vec::new()).await
}
async fn save_catalog_with(
    platform: &dyn Platform,
    entries: Vec<EnvironmentRegistration>,
    expected: Option<Vec<u8>>,
    overrides: Vec<StorageWrite>,
) -> Result<Vec<u8>, ClientError> {
    let previous = expected
        .as_ref()
        .map(|bytes| {
            crate::persistence::decode_record::<serde_json::Value>(bytes, &catalog_key(), 1)
                .map(|record| record.payload)
                .map_err(|_| {
                    ClientError::InvalidPayload(
                        "invalid previous connection catalog; original record retained".into(),
                    )
                })
        })
        .transpose()?;
    let previous_targets = previous
        .as_ref()
        .and_then(|value| value["targets"].as_array());
    let mut targets = Vec::new();
    let mut disabled = Vec::new();
    let mut secure = Vec::new();
    let mut conditions = vec![StorageCondition {
        area: StorageArea::Ordinary,
        key: catalog_key(),
        expected,
    }];
    for entry in entries {
        if !entry.enabled {
            disabled.push(entry.id.clone());
        }
        for route in entry.routes {
            let tag = match route.mode {
                ConnectionMode::Relay => "RelayConnectionTarget",
                ConnectionMode::Ssh => "SshConnectionTarget",
                ConnectionMode::Remote | ConnectionMode::Tunnel => "BearerConnectionTarget",
                ConnectionMode::Local => {
                    return Err(ClientError::InvalidPayload(
                        "primary environments are supplied by the platform".into(),
                    ));
                }
            };
            let mut target =
                serde_json::json!({"_tag":tag, "environmentId":entry.id, "label":entry.label});
            if route.mode != ConnectionMode::Relay {
                target["connectionId"] = route.id.clone().into();
                if route.mode != ConnectionMode::Ssh {
                    let key = StorageKey {
                        namespace: "connection-profile".into(),
                        environment: None,
                        record: route.id.clone(),
                    };
                    // A profile that cannot be read (it failed authentication, or has a shape this
                    // build does not know) keeps its route and its row as they are, so every
                    // other environment can still be saved.
                    platform.get(StorageArea::Secure, key.clone()).await?; // N2-PROBE
                    let Ok(existing) = platform.get(StorageArea::Secure, key.clone()).await else {
                        targets.push(target);
                        continue;
                    };
                    let decoded = existing
                        .as_ref()
                        .map(|bytes| {
                            crate::persistence::decode_record::<serde_json::Value>(bytes, &key, 1)
                        })
                        .transpose();
                    let Ok(decoded) = decoded else {
                        targets.push(target);
                        continue;
                    };
                    conditions.push(StorageCondition {
                        area: StorageArea::Secure,
                        key: key.clone(),
                        expected: existing.clone(),
                    });
                    let mut profile = if let Some(record) = decoded {
                        record.payload
                    } else {
                        serde_json::json!({"_tag":"BearerConnectionProfile", "connectionId":route.id, "environmentId":entry.id})
                    };
                    if profile["environmentId"] != serde_json::to_value(&entry.id)?
                        || profile["connectionId"] != route.id
                    {
                        return Err(ClientError::EnvironmentMismatch);
                    }
                    profile["httpBaseUrl"] = route.http_base.into();
                    profile["wsBaseUrl"] = route.ws_base.into();
                    profile["label"] = entry.label.clone().into();
                    let record = crate::persistence::Record::new(
                        "connection-profile",
                        None,
                        &key.record,
                        1,
                        profile,
                    );
                    secure.push(StorageWrite {
                        key,
                        value: Some(record.encode().map_err(|_| {
                            ClientError::InvalidPayload(
                                "could not encode connection profile".into(),
                            )
                        })?),
                    });
                }
            }
            targets.push(target);
        }
    }
    let mut ordinary = Vec::new();
    if let Some(previous_targets) = previous_targets {
        for target in previous_targets {
            let environment = EnvironmentId(
                target["environmentId"]
                    .as_str()
                    .ok_or_else(invalid_endpoint)?
                    .into(),
            );
            if let Some(connection) = target["connectionId"].as_str()
                && !targets
                    .iter()
                    .any(|next| next["connectionId"] == connection)
            {
                for namespace in ["connection-profile", "connection-credential"] {
                    secure.push(StorageWrite {
                        key: StorageKey::new(namespace, None, connection),
                        value: None,
                    });
                }
            }
            if !targets
                .iter()
                .any(|next| next["environmentId"] == environment.0)
                || (target["_tag"] == "RelayConnectionTarget"
                    && !targets.iter().any(|next| {
                        next["environmentId"] == environment.0
                            && next["_tag"] == "RelayConnectionTarget"
                    }))
            {
                secure.push(StorageWrite {
                    key: crate::persistence::remote_dpop_token_key(&environment),
                    value: None,
                });
            }
            let route_ids = |values: &[serde_json::Value]| -> BTreeSet<(String, String)> {
                values
                    .iter()
                    .filter(|value| value["environmentId"] == environment.0)
                    .map(|value| {
                        (
                            value["_tag"].as_str().unwrap_or("").to_owned(),
                            value["connectionId"].as_str().unwrap_or("").to_owned(),
                        )
                    })
                    .collect()
            };
            if route_ids(previous_targets) != route_ids(&targets) {
                ordinary.push(StorageWrite {
                    key: StorageKey::new("routing-grant", Some(environment), "current"),
                    value: None,
                });
            }
        }
    }
    let payload =
        serde_json::json!({"schemaVersion":1,"targets":targets,"disabledEnvironmentIds":disabled});
    let record = crate::persistence::Record::new("connection-catalog", None, "current", 1, payload);
    let value = record
        .encode()
        .map_err(|_| ClientError::InvalidPayload("could not encode connection catalog".into()))?;
    for replacement in overrides {
        secure.retain(|write| write.key != replacement.key);
        secure.push(replacement);
    }
    ordinary.push(StorageWrite {
        key: catalog_key(),
        value: Some(value.clone()),
    });
    let committed = platform
        .transaction(StorageTransaction {
            conditions,
            ordinary,
            secure,
        })
        .await?;
    if !committed {
        return Err(ClientError::InvalidPayload(
            "connection catalog changed concurrently".into(),
        ));
    }
    Ok(value)
}
fn validate_registration(registration: &EnvironmentRegistration) -> Result<(), ClientError> {
    let mut routes = BTreeSet::new();
    if registration.routes.is_empty()
        || registration
            .routes
            .iter()
            .any(|r| r.environment != registration.id || !routes.insert(&r.id))
    {
        return Err(ClientError::InvalidPayload(
            "environment routes must be nonempty, unique and share its identity".into(),
        ));
    }
    Ok(())
}
