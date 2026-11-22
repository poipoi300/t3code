//! Shared settings rules, plus the settings pages' scope, search and composed service.
pub mod devices;
pub mod inheritance;
pub mod maintenance;
pub mod models;
pub mod panels;
pub mod scope;
pub mod scoped;
pub mod search;
mod search_items;
pub mod service;
pub mod sessions;

use crate::ClientError;
use serde_json::{Value, json};
use t3_contracts::{
    provider_instance::ProviderInstanceId,
    settings::{ServerSettings, ServerSettingsPatch},
};

const SHARED_KEYS: &[&str] = &[
    "continueThreadsAfterServerUpdate",
    "sidebarAutoSettleAfterDays",
    "sidebarAutoSettleOnMerge",
    "autoResumeLimitedThreads",
    "snoozeLimitedThreads",
    "snoozePresets",
    "newWorktreesStartFromOrigin",
    "sourceControlWritingStyle",
    "textGenerationModelSelection",
];
pub fn split_shared_server_patch(
    patch: ServerSettingsPatch,
) -> Result<(ServerSettingsPatch, ServerSettingsPatch), ClientError> {
    let value = serde_json::to_value(patch)?;
    let mut shared = json!({});
    let mut local = json!({});
    if let Some(fields) = value.as_object() {
        for (key, value) in fields {
            if SHARED_KEYS.contains(&key.as_str()) {
                shared[key] = value.clone();
            } else {
                local[key] = value.clone();
            }
        }
    }
    Ok((
        serde_json::from_value(shared)?,
        serde_json::from_value(local)?,
    ))
}
fn selection_enabled(settings: &ServerSettings, instance: &ProviderInstanceId) -> bool {
    if let Some(generator) = settings.text_generators.get(instance) {
        return generator.enabled;
    }
    if let Some(config) = settings.provider_instances.get(instance) {
        let config_enabled = config
            .config
            .as_ref()
            .and_then(|config| config.get("enabled"))
            .and_then(Value::as_bool);
        if config.enabled == Some(false) || config_enabled == Some(false) {
            return false;
        }
        return config.enabled.or(config_enabled).unwrap_or(!matches!(
            config.driver.0.as_str(),
            "cursor" | "grok" | "fx" | "kiro" | "muse" | "pi" | "opencode" | "antigravity"
        ));
    }
    matches!(instance.0.as_str(), "codex" | "claudeAgent")
}
pub fn filter_shared_server_patch(
    patch: ServerSettingsPatch,
    settings: Option<&ServerSettings>,
    source: Option<&ServerSettings>,
    target_is_source: bool,
) -> Result<ServerSettingsPatch, ClientError> {
    let source = source.or(settings);
    let mut value = serde_json::to_value(patch)?;
    if !target_is_source && let Some(selection) = value.get("textGenerationModelSelection") {
        let id = selection
            .get("instanceId")
            .and_then(Value::as_str)
            .map(|id| ProviderInstanceId(id.into()))
            .or_else(|| {
                source.map(|source| source.text_generation_model_selection.instance_id.clone())
            });
        let keep = settings.is_some_and(|settings| {
            id.as_ref().is_some_and(|id| {
                let source_driver = source
                    .and_then(|source| source.provider_instances.get(id))
                    .map_or(&id.0, |config| &config.driver.0);
                let driver = settings
                    .provider_instances
                    .get(id)
                    .map_or(&id.0, |config| &config.driver.0);
                source_driver == driver && selection_enabled(settings, id)
            })
        });
        if !keep && let Some(value) = value.as_object_mut() {
            value.remove("textGenerationModelSelection");
        }
    }
    if settings.is_none()
        && let Some(value) = value.as_object_mut()
    {
        value.remove("continueThreadsAfterServerUpdate");
    }
    Ok(serde_json::from_value(value)?)
}
pub fn supports_shared_settings_sync(phase: &str, config_loaded: bool) -> bool {
    phase == "connected" && config_loaded
}
pub fn hidden_thread_tags(settings: &ServerSettings) -> Option<Vec<String>> {
    if !settings.hide_tagged_threads {
        return None;
    }
    let tags: Vec<_> = settings
        .hidden_thread_tags
        .iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect();
    (!tags.is_empty()).then_some(tags)
}
pub fn usage_scan_settings_key(settings: &ServerSettings) -> Result<String, ClientError> {
    let prices: Vec<_> = settings
        .usage_price_overrides
        .iter()
        .map(|(model, price)| {
            json!([
                model,
                price.input_cost_per_million_tokens,
                price.output_cost_per_million_tokens,
                price.cache_read_cost_per_million_tokens,
                price.cache_write_cost_per_million_tokens
            ])
        })
        .collect();
    let aliases: Vec<_> = settings.usage_model_aliases.iter().collect();
    Ok(serde_json::to_string(&(
        serde_json::to_string(&prices)?,
        aliases,
    ))?)
}

/// An edit of the saved per-device routes, in preference order.
pub enum SavedRouteEdit {
    Order(Vec<String>),
    Remove(String),
}

/// Display only the HTTP origin. A catalog imported from another client may
/// include userinfo, a pairing fragment or a query bearing a token.
pub fn saved_route_address(route: &crate::connection::Route) -> Option<String> {
    if matches!(
        route.mode,
        crate::connection::ConnectionMode::Ssh | crate::connection::ConnectionMode::Relay
    ) {
        return None;
    }
    crate::connection::agent_consent::page_origin(&route.http_base)
}

pub fn saved_route_label(route: &crate::connection::Route) -> String {
    use crate::connection::RouteKind;
    match crate::connection::route_kind(route, false) {
        RouteKind::Relay => "T3 Connect",
        RouteKind::Loopback => "This device",
        RouteKind::Lan => "LAN",
        RouteKind::Tailnet => "Tailscale",
        RouteKind::Vpn => "VPN",
        RouteKind::Ssh => "SSH",
        RouteKind::Public => {
            return url::Url::parse(&route.http_base)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned))
                .unwrap_or_else(|| "Remote link".to_owned());
        }
    }
    .to_owned()
}

/// A manual address never receives a pairing credential minted by this client.
/// A catalog selection is resolved again by id when the user submits the form.
pub enum PeerDestination {
    Address(String),
    Machine(EnvironmentId),
}
use crate::types::EnvironmentId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerLinkFailure {
    NotAllowed,
    Disconnected,
    Refused(t3_contracts::peer_link::PeerLinkErrorReason),
    Mint(crate::services::http::PeerPairingCodeError),
    Unavailable,
}

impl From<crate::rpc::RpcCallError> for PeerLinkFailure {
    fn from(error: crate::rpc::RpcCallError) -> Self {
        use crate::rpc::RpcCallError;
        use t3_contracts::rpc::WsPeerLinksListError;
        match error {
            RpcCallError::Rejected(errors) => errors
                .into_iter()
                .find_map(
                    |e| match serde_json::from_value::<WsPeerLinksListError>(e).ok()? {
                        WsPeerLinksListError::PeerLinkError(error) => {
                            Some(Self::Refused(error.reason))
                        }
                        WsPeerLinksListError::EnvironmentAuthorizationError(_) => {
                            Some(Self::NotAllowed)
                        }
                    },
                )
                .unwrap_or(Self::Unavailable),
            RpcCallError::NotSent(crate::ClientError::PermissionDenied { .. }) => Self::NotAllowed,
            RpcCallError::Unknown
            | RpcCallError::Interrupted
            | RpcCallError::NotSent(
                crate::ClientError::Disconnected | crate::ClientError::Cancelled,
            ) => Self::Disconnected,
            _ => Self::Unavailable,
        }
    }
}

/// One mounted Sharing & access page. The socket, its grants and the stream
/// snapshot belong to one session; a reconnect mounts a new manager.
pub mod access {
    use crate::{
        connection::ConnectedClient,
        permissions::{
            EnvironmentHttpClient, apply_auth_access_stream_event,
            connect_device::{connect_device_name, describe_connect_device_client},
        },
        platform::Platform,
        rpc::{RpcEndpoint, SessionEndpoint, subscription::SubscriptionOptions},
    };
    use futures::StreamExt;
    use std::{cell::RefCell, rc::Rc};
    use t3_contracts::{
        auth::*,
        base_schemas::{AuthSessionId, TrimmedNonEmptyString},
        rpc::{
            Empty, WsServerUpdateSettingsPayload, WsServerUpdateSettingsRpc,
            WsSubscribeAuthAccessRpc,
        },
        settings::ServerSettingsPatch,
    };

    #[derive(Clone, PartialEq, Eq)]
    pub enum Action {
        Client(AuthSessionId),
        Others,
        Approve(TrimmedNonEmptyString),
        Deny(TrimmedNonEmptyString),
        Device(TrimmedNonEmptyString),
    }
    #[derive(Clone, PartialEq, Eq)]
    pub struct ClientRow {
        pub id: AuthSessionId,
        pub name: String,
        pub details: String,
        pub scopes: Vec<String>,
        pub current: bool,
        pub live: bool,
        pub last_connected: Option<String>,
    }
    #[derive(Clone, PartialEq, Eq)]
    pub struct Pending {
        pub id: TrimmedNonEmptyString,
        pub code: Option<String>,
        pub details: String,
    }
    #[derive(Clone, PartialEq, Eq)]
    pub struct Device {
        pub key: TrimmedNonEmptyString,
        pub name: String,
        pub code: Option<String>,
        pub since: String,
    }
    #[derive(Clone, PartialEq, Eq, Default)]
    pub struct Records {
        pub clients: Vec<ClientRow>,
        pub pending: Vec<Pending>,
        pub trusted: Vec<Device>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum AccessFailure {
        Changed,
        Save,
        Revoke,
    }
    #[derive(Clone, PartialEq, Eq)]
    pub enum PermissionState {
        Checking,
        Failed,
        Rejected,
        Verified(Vec<bool>),
    }
    pub enum RevokeTitle {
        OtherClients,
        Device(String),
    }
    pub enum Outcome {
        Done,
        Confirm(RevokeTitle),
    }
    pub struct Manager {
        client: ConnectedClient,
        endpoint: SessionEndpoint,
        platform: Rc<dyn Platform>,
        snapshot: RefCell<Option<AuthAccessSnapshot>>,
    }
    pub fn display_code(code: &str) -> Option<String> {
        let (first, last) = code.split_once('-')?;
        (first.len() == 4
            && last.len() == 4
            && first
                .bytes()
                .chain(last.bytes())
                .all(|b| b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ".contains(&b)))
        .then(|| code.to_owned())
    }
    fn records(snapshot: &AuthAccessSnapshot) -> Records {
        let mut result = Records::default();
        let mut sessions = snapshot.client_sessions.iter().collect::<Vec<_>>();
        sessions.sort_by_key(|s| {
            std::cmp::Reverse((
                s.current,
                s.connected,
                crate::time::epoch_ms(&s.issued_at.0),
            ))
        });
        for s in sessions {
            let m = &s.client;
            let os_browser = [m.os.as_ref(), m.browser.as_ref()]
                .into_iter()
                .flatten()
                .map(|v| v.0.as_str())
                .collect::<Vec<_>>()
                .join(" · ");
            let name = m.label.as_ref().map(|l| l.0.clone()).unwrap_or_else(|| {
                if os_browser.is_empty() {
                    s.subject.0.clone()
                } else {
                    os_browser
                }
            });
            let mut bits = Vec::new();
            if let Some(kind) = serde_json::to_value(m.device_type)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                && kind != "unknown"
            {
                let mut kind = kind;
                if let Some(first) = kind.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                bits.push(kind);
            }
            bits.extend(
                [m.os.as_ref(), m.browser.as_ref(), m.ip_address.as_ref()]
                    .into_iter()
                    .flatten()
                    .map(|s| s.0.clone()),
            );
            result.clients.push(ClientRow {
                id: s.session_id.clone(),
                name,
                details: bits.join(" · "),
                current: s.current,
                live: s.current || s.connected,
                last_connected: s.last_connected_at.as_ref().map(|date| date.0.clone()),
                scopes: s
                    .permissions
                    .as_ref()
                    .unwrap_or(&s.scopes)
                    .iter()
                    .filter_map(|scope| {
                        serde_json::to_value(scope)
                            .ok()?
                            .as_str()
                            .map(str::to_owned)
                    })
                    .collect(),
            });
        }
        if let Some(devices) = &snapshot.connect_devices {
            let mut links = snapshot.pairing_links.iter().collect::<Vec<_>>();
            links.sort_by_key(|l| std::cmp::Reverse(crate::time::epoch_ms(&l.created_at.0)));
            result.pending = links
                .into_iter()
                .filter_map(|l| {
                    let r = l.connect_device_request.as_ref()?;
                    Some(Pending {
                        id: l.id.clone(),
                        code: display_code(&r.verification_code.0),
                        details: describe_connect_device_client(r.client.as_ref()),
                    })
                })
                .collect();
            result.trusted = devices
                .iter()
                .map(|d| Device {
                    key: d.proof_key_thumbprint.clone(),
                    name: describe_connect_device_client(d.client.as_ref()),
                    code: display_code(&d.verification_code.0),
                    since: d.approved_at.0.clone(),
                })
                .collect();
        }
        result
    }
    impl Manager {
        pub(crate) fn new(client: ConnectedClient, platform: Rc<dyn Platform>) -> Option<Self> {
            let endpoint = client.session()?;
            Some(Self {
                client,
                endpoint,
                platform,
                snapshot: Default::default(),
            })
        }
        pub async fn permissions(
            &self,
            scopes: &[AuthEnvironmentScope],
            mut publish: impl FnMut(PermissionState),
        ) {
            let mut retry = crate::permissions::SessionRetry::default();
            loop {
                if self
                    .endpoint
                    .grants_scope(AuthEnvironmentScope::AccessRead)
                    .is_none()
                {
                    return;
                }
                let result = self.client.verify_session().await;
                if self
                    .endpoint
                    .grants_scope(AuthEnvironmentScope::AccessRead)
                    .is_none()
                {
                    return;
                }
                match result {
                    Ok(state) if state.authenticated => {
                        publish(PermissionState::Verified(
                            scopes
                                .iter()
                                .map(|scope| {
                                    crate::permissions::session_grants_scope(&state, *scope)
                                })
                                .collect(),
                        ));
                        return;
                    }
                    Ok(_) => {
                        publish(PermissionState::Rejected);
                        return;
                    }
                    Err(_) => {
                        publish(PermissionState::Failed);
                        self.platform.sleep(retry.failed()).await;
                    }
                }
            }
        }
        pub async fn follow(&self, mut publish: impl FnMut(Result<Records, ()>)) {
            if self.endpoint.grants_scope(AuthEnvironmentScope::AccessRead) != Some(true) {
                return;
            }
            let mut events = self
                .endpoint
                .subscribe_recovering::<WsSubscribeAuthAccessRpc, _>(
                    self.platform.as_ref(),
                    Empty {},
                    SubscriptionOptions::default(),
                );
            while let Some(event) = events.next().await {
                if self.endpoint.grants_scope(AuthEnvironmentScope::AccessRead) != Some(true) {
                    break;
                }
                let result = match event {
                    Ok(AuthAccessStreamEvent::AuthAccessStreamSnapshotEvent(e)) => {
                        self.snapshot.replace(Some(e.payload));
                        Ok(())
                    }
                    Ok(event) => self
                        .snapshot
                        .borrow_mut()
                        .as_mut()
                        .map(|s| apply_auth_access_stream_event(s, event))
                        .ok_or(()),
                    Err(_) => Err(()),
                };
                if result.is_err() {
                    self.snapshot.take();
                }
                publish(self.snapshot.borrow().as_ref().map(records).ok_or(()));
            }
            self.snapshot.take();
            publish(Err(()));
        }
        fn writable(&self) -> Result<EnvironmentHttpClient, ()> {
            if self
                .endpoint
                .grants_scope(AuthEnvironmentScope::AccessWrite)
                != Some(true)
            {
                return Err(());
            }
            self.client.access_http().ok_or(())
        }
        /// Revalidate membership on click and after any interleaved setting save.
        fn connect_revoke(&self, action: &Action) -> Result<Option<RevokeTitle>, ()> {
            if !matches!(action, Action::Others)
                && self.endpoint.grants_scope(AuthEnvironmentScope::AccessRead) != Some(true)
            {
                return Err(());
            }
            let state = self.snapshot.borrow();
            let snapshot = state.as_ref();
            match action {
                Action::Others => {
                    if let Some(s) = snapshot
                        && !s.client_sessions.iter().any(|v| !v.current)
                    {
                        return Err(());
                    }
                    Ok(snapshot
                        .filter(|s| {
                            s.connect_devices.as_ref().is_some_and(|d| !d.is_empty())
                                || s.client_sessions
                                    .iter()
                                    .any(|v| !v.current && v.subject.0 == "cloud-connect")
                        })
                        .map(|_| RevokeTitle::OtherClients))
                }
                Action::Client(id) => {
                    let record = snapshot
                        .ok_or(())?
                        .client_sessions
                        .iter()
                        .find(|v| &v.session_id == id && !v.current)
                        .ok_or(())?;
                    Ok((record.subject.0 == "cloud-connect").then(|| {
                        RevokeTitle::Device(connect_device_name(Some(&record.client)).into())
                    }))
                }
                Action::Device(key) => {
                    let device = snapshot
                        .and_then(|s| s.connect_devices.as_ref())
                        .ok_or(())?
                        .iter()
                        .find(|d| &d.proof_key_thumbprint == key)
                        .ok_or(())?;
                    Ok(Some(RevokeTitle::Device(
                        connect_device_name(device.client.as_ref()).into(),
                    )))
                }
                Action::Approve(id) | Action::Deny(id) => {
                    let s = snapshot.filter(|s| s.connect_devices.is_some()).ok_or(())?;
                    let request = s
                        .pairing_links
                        .iter()
                        .find(|l| &l.id == id)
                        .and_then(|l| l.connect_device_request.as_ref())
                        .ok_or(())?;
                    if matches!(action, Action::Approve(_))
                        && display_code(&request.verification_code.0).is_none()
                    {
                        return Err(());
                    }
                    Ok(None)
                }
            }
        }
        pub async fn apply(
            &self,
            action: &Action,
            confirmed: Option<bool>,
        ) -> Result<Outcome, AccessFailure> {
            self.writable().map_err(|_| AccessFailure::Changed)?;
            if let Some(title) = self
                .connect_revoke(action)
                .map_err(|_| AccessFailure::Changed)?
            {
                match confirmed {
                    None => {
                        if self.endpoint.connect_approval() == Some(false) {
                            return Ok(Outcome::Confirm(title));
                        }
                    }
                    Some(true) => {
                        let patch = ServerSettingsPatch {
                            require_connect_device_approval: Some(true),
                            ..Default::default()
                        };
                        let input = WsServerUpdateSettingsPayload {
                            patch: patch.into(),
                            provider_instance_mutation: None,
                        };
                        if !self.endpoint.available::<WsServerUpdateSettingsRpc>(&input) {
                            return Err(AccessFailure::Save);
                        }
                        let saved = self
                            .endpoint
                            .command::<WsServerUpdateSettingsRpc>(input)
                            .await
                            .map_err(|_| AccessFailure::Save)?;
                        if !saved.require_connect_device_approval {
                            return Err(AccessFailure::Save);
                        }
                    }
                    Some(false) => {}
                }
            }
            let http = self.writable().map_err(|_| AccessFailure::Changed)?;
            self.connect_revoke(action)
                .map_err(|_| AccessFailure::Changed)?;
            match action {
                Action::Client(id) => http
                    .revoke_client(&AuthRevokeClientSessionInput {
                        session_id: id.clone(),
                    })
                    .await
                    .map(|_| ()),
                Action::Others => http.revoke_other_clients().await.map(|_| ()),
                Action::Approve(id) => http
                    .approve_connect_device(&AuthApproveConnectDeviceInput {
                        pairing_link_id: id.clone(),
                    })
                    .await
                    .map(|_| ()),
                Action::Deny(id) => http
                    .revoke_pairing_link(&AuthRevokePairingLinkInput { id: id.clone() })
                    .await
                    .map(|_| ()),
                Action::Device(key) => http
                    .revoke_connect_device(&AuthRevokeConnectDeviceInput {
                        proof_key_thumbprint: key.clone(),
                    })
                    .await
                    .map(|_| ()),
            }
            .map_err(|_| AccessFailure::Revoke)?;
            Ok(Outcome::Done)
        }
    }
}
