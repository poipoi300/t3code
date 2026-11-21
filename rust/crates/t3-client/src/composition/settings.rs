use super::*;

impl<P: Platform + Clone + 'static> ClientRuntime<P> {
    /// The settings pages' service (`services::settings::service`).
    pub fn settings(&self) -> crate::services::settings::service::SettingsService<P> {
        crate::services::settings::service::SettingsService::new(
            self.clone(),
            Rc::new(self.inner.platform.clone()),
            self.inner.spawn.clone(),
        )
    }
}

impl<P: Platform + Clone + 'static> ClientRuntime<P> {
    pub fn access_manager(
        &self,
        id: &EnvironmentId,
    ) -> Option<Rc<crate::services::settings::access::Manager>> {
        crate::services::settings::access::Manager::new(
            self.client(id)?,
            Rc::new(self.inner.platform.clone()),
        )
        .map(Rc::new)
    }
    /// Sanitize the same failure snapshot used to compose the visible remedies and clipboard report.
    pub fn settings_diagnosis(
        &self,
        id: &EnvironmentId,
        client_version: Option<&str>,
    ) -> Option<ConnectionDiagnosis> {
        let mut entry = self
            .environments()
            .into_iter()
            .find(|entry| &entry.id == id)?;
        if let Some(failure) = &mut entry.state.last_failure
            && failure
                .verification_code
                .as_deref()
                .is_some_and(|code| crate::services::settings::access::display_code(code).is_none())
        {
            failure.verification_code = Some("<code>".into());
        }
        let mut diagnosis = self.diagnosis_for_entry(entry, client_version)?;
        diagnosis
            .facts
            .retain(|(label, _)| !matches!(label.as_str(), "Error" | "Trace ID"));
        Some(diagnosis)
    }
    /// Local device preference, saved under the same registry lease as pairing.
    pub fn set_saved_environment_enabled(
        &self,
        id: &EnvironmentId,
        enabled: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.owned(self.set_composer_environment_enabled(id, enabled), || {
            Err(ClientError::Cancelled)
        })
    }

    /// Only the catalogue order observed by the caller may be edited. A route
    /// learned or removed while a confirmation was open requires a fresh decision.
    pub fn edit_saved_routes(
        &self,
        id: EnvironmentId,
        observed: Vec<String>,
        edit: crate::services::settings::SavedRouteEdit,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        use crate::services::settings::SavedRouteEdit;
        let this = self.clone();
        self.owned(
            async move {
                let _lease = this.inner.registry.lock().await;
                let platform = &this.inner.platform;
                let mut registry = this.load_registry().await?;
                let routes = &registry
                    .get(&id)
                    .ok_or(ClientError::EnvironmentMismatch)?
                    .registration()
                    .routes;
                if !routes.iter().map(|route| &route.id).eq(&observed) {
                    return Err(ClientError::InvalidPayload(
                        "The saved routes have changed.".into(),
                    ));
                }
                let time = Time::from_platform(platform);
                let mut ssh_target = None;
                match edit {
                    SavedRouteEdit::Order(next) => {
                        registry.reorder_routes(&id, &next, time)?;
                    }
                    SavedRouteEdit::Remove(route) => {
                        if routes.len() < 2
                            || route.starts_with("learned:")
                            || !routes.iter().any(|r| r.id == route)
                            || crate::connection::routes_after_removing(routes, &route).is_empty()
                        {
                            return Err(ClientError::InvalidPayload(
                                "This route cannot be removed.".into(),
                            ));
                        }
                        if routes
                            .iter()
                            .any(|r| r.id == route && r.mode == ConnectionMode::Ssh)
                        {
                            let stores = ConnectionStores::new(Rc::new(platform.clone()));
                            let profile = stores.profile(&id, &route).await.map_err(|_| {
                                ClientError::InvalidPayload(
                                    "Could not read the saved SSH profile.".into(),
                                )
                            })?;
                            if let Some(ConnectionProfile::Ssh { target, .. }) = profile {
                                ssh_target = Some(target);
                            }
                        }
                        registry.remove_route(&id, &route, time)?;
                    }
                }
                registry.save(platform).await?;
                if let Some(target) = ssh_target {
                    let _ = platform.ssh_disconnect(target).await;
                }
                this.start_saved(&id).await
            }
            .boxed_local(),
            || Err(ClientError::Cancelled),
        )
    }
}

impl<P: Platform + Clone + 'static> ClientRuntime<P> {
    /// Only the connected, opted-in peer-link hosts visible to this session.
    pub fn settings_peer_session(&self, id: &EnvironmentId) -> Option<crate::rpc::SessionEndpoint> {
        use t3_contracts::auth::AuthEnvironmentScope::AccessRead;
        let state = self.inner.store.snapshot();
        let env = state.environments.get(id)?;
        let config = env.config.as_ref()?;
        if config.environment.capabilities.peer_links != Some(true)
            || !config.settings.linked_environments_enabled
        {
            return None;
        }
        let client = self.client(id)?;
        if client.state().phase != Phase::Connected {
            return None;
        }
        let session = client.session()?;
        (session.grants_scope(AccessRead) == Some(true)).then_some(session)
    }
}

impl<P: Platform + Clone + 'static> ClientRuntime<P> {
    pub fn settings_linkable_machines(
        &self,
        source: &EnvironmentId,
        links: &[t3_contracts::peer_link::PeerLinkSummary],
    ) -> Vec<crate::thread_views::link_request::LinkableMachine> {
        use crate::thread_views::link_request::{LinkableMachine, linkable_machines};
        let snapshot = self.store().snapshot();
        let mut machines = linkable_machines(
            &self.environments(),
            |id| {
                snapshot
                    .environments
                    .get(id)
                    .and_then(|e| e.config.as_ref()?.direct_endpoints.as_ref())
                    .map(|d| d.0.iter().map(|v| v.http_base_url.0.clone()).collect())
                    .unwrap_or_default()
            },
            source,
            links,
        );
        if let Some(discovery) = self.relay_environments() {
            for (id, entry) in discovery.environments {
                if &id == source {
                    continue;
                }
                let url = entry.environment.endpoint.http_base_url.0;
                let usable = !crate::thread_views::link_request::is_loopback_url(&url);
                if let Some(machine) = machines.iter_mut().find(|m| m.environment_id == id) {
                    if usable && !machine.urls.contains(&url) {
                        machine.urls.push(url);
                    }
                } else {
                    let linked = links.iter().any(|l| {
                        l.environment_id == id
                            && l.status != t3_contracts::peer_link::PeerLinkStatus::Expired
                    });
                    machines.push(LinkableMachine {
                        environment_id: id,
                        label: entry.environment.label.0,
                        urls: usable.then_some(url).into_iter().collect(),
                        connected: false,
                        connect_only: true,
                        linked,
                    });
                }
            }
        }
        machines
    }

    pub fn forget_settings_peer(
        &self,
        source: EnvironmentId,
        target: EnvironmentId,
    ) -> LocalBoxFuture<'static, Result<(), crate::services::settings::PeerLinkFailure>> {
        use crate::rpc::RpcEndpoint;
        use crate::services::settings::PeerLinkFailure;
        use t3_contracts::{peer_link::PeerLinkRemoveInput, rpc::WsPeerLinksUnlinkRpc};
        let this = self.clone();
        async move {
            let session = this
                .settings_peer_session(&source)
                .ok_or(PeerLinkFailure::Disconnected)?;
            let input = PeerLinkRemoveInput {
                environment_id: target,
            };
            if !session.available::<WsPeerLinksUnlinkRpc>(&input) {
                return Err(PeerLinkFailure::NotAllowed);
            }
            session
                .command::<WsPeerLinksUnlinkRpc>(input)
                .await
                .map(|_| ())
                .map_err(Into::into)
        }
        .boxed_local()
    }

    pub fn link_settings_peer(
        &self,
        source: EnvironmentId,
        destination: crate::services::settings::PeerDestination,
        code: Option<crate::authorization::Sensitive<String>>,
        access: t3_contracts::auth::AuthMcpClientAccess,
    ) -> LocalBoxFuture<'static, Result<(), crate::services::settings::PeerLinkFailure>> {
        use crate::rpc::RpcEndpoint;
        use crate::services::{
            http::{PeerPairingCodeError, mint_peer_pairing_code},
            settings::{PeerDestination, PeerLinkFailure as Failure},
        };
        use t3_contracts::{
            auth::AuthEnvironmentScope, base_schemas::TrimmedNonEmptyString as NonEmpty,
            peer_link::PeerLinkCreateInput, rpc::WsPeerLinksLinkRpc,
        };
        let this = self.clone();
        async move {
            let source_session = this
                .settings_peer_session(&source)
                .ok_or(Failure::Disconnected)?;
            if source_session.grants_scope(AuthEnvironmentScope::AccessWrite) != Some(true) {
                return Err(Failure::NotAllowed);
            }
            let (expected, label, urls, code) = match destination {
                PeerDestination::Address(url) => {
                    (None, None, vec![url], code.ok_or(Failure::Unavailable)?)
                }
                PeerDestination::Machine(id) => {
                    let machine = this
                        .settings_linkable_machines(&source, &[])
                        .into_iter()
                        .find(|m| m.environment_id == id && !m.urls.is_empty())
                        .ok_or(Failure::Unavailable)?;
                    let code = match code {
                        Some(code) => code,
                        None => {
                            let http = this
                                .client(&id)
                                .and_then(|c| c.access_http())
                                .ok_or(Failure::Mint(PeerPairingCodeError::NotAllowed))?;
                            mint_peer_pairing_code(&http, access)
                                .await
                                .map_err(Failure::Mint)?
                        }
                    };
                    (Some(id), Some(NonEmpty(machine.label)), machine.urls, code)
                }
            };
            let mut urls = urls.into_iter().map(NonEmpty);
            let url = urls.next().ok_or(Failure::Unavailable)?;
            let alternates = urls.collect::<Vec<_>>();
            let input = PeerLinkCreateInput {
                url,
                alternate_urls: (!alternates.is_empty()).then_some(alternates),
                expected_environment_id: expected,
                expected_label: label,
                pairing_code: NonEmpty(code.into_inner()),
                access,
            };
            if !source_session.available::<WsPeerLinksLinkRpc>(&input) {
                return Err(Failure::NotAllowed);
            }
            source_session
                .command::<WsPeerLinksLinkRpc>(input)
                .await
                .map(|_| ())
                .map_err(Into::into)
        }
        .boxed_local()
    }
}
