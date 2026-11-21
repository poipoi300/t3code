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
                        registry.remove_route(&id, &route, time)?;
                    }
                }
                registry.save(platform).await?;
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
                .filter(|s| s.grants_scope(AuthEnvironmentScope::AccessWrite) == Some(true))
                .ok_or(Failure::Unavailable)?;
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
                return Err(Failure::Unavailable);
            }
            source_session
                .call::<WsPeerLinksLinkRpc>(input)
                .await
                .map(|_| ())
                .map_err(|_| Failure::Unavailable)
        }
        .boxed_local()
    }
}
