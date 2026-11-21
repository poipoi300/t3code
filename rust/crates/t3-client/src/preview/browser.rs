//! Browser panel commands and subscriptions for a scoped thread.

use crate::{
    ClientError,
    composition::ClientRuntime,
    platform::Platform,
    rpc::{RpcCallError, RpcEndpoint, SessionEndpoint},
    types::{Generation, ThreadKey},
    view_logic::shell::preview_state::bound_configured_urls,
};
use futures::{StreamExt, stream::LocalBoxStream};
use t3_contracts::{
    auth::AuthEnvironmentScope,
    base_schemas::TrimmedNonEmptyString,
    preview::{
        DiscoveredLocalServer, DiscoveredLocalServerList, PreviewCloseInput, PreviewEvent,
        PreviewListInput, PreviewListResult, PreviewOpenInput, PreviewRuntime,
        PreviewSessionSnapshot, PreviewTabId,
    },
    rpc::{
        Empty, WsPreviewCloseRpc, WsPreviewListRpc, WsPreviewOpenRpc,
        WsSubscribeDiscoveredLocalServersPayload, WsSubscribeDiscoveredLocalServersRpc,
        WsSubscribePreviewEventsRpc,
    },
};

use super::open::{BrowserDefaults, EntryPoint, apply_defaults, resolve_address};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserAccess {
    Disconnected,
    Restricted,
    Connected { serial: u64, generation: Generation },
}

#[derive(Clone)]
pub struct BrowserPreview<P: Platform + Clone + 'static> {
    pub(super) runtime: ClientRuntime<P>,
    pub(super) thread: ThreadKey,
}

impl<P: Platform + Clone + 'static> ClientRuntime<P> {
    pub fn browser_preview(&self, thread: ThreadKey) -> BrowserPreview<P> {
        BrowserPreview {
            runtime: self.clone(),
            thread,
        }
    }
}

impl<P: Platform + Clone + 'static> BrowserPreview<P> {
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        futures::stream::select(
            self.runtime
                .store()
                .subscribe_environment(&self.thread.environment)
                .map(|_| ()),
            self.runtime.changes(),
        )
        .boxed_local()
    }

    pub fn access(&self) -> BrowserAccess {
        let Some(connection) = self.runtime.client(&self.thread.environment) else {
            return BrowserAccess::Disconnected;
        };
        let Some(endpoint) = connection.session() else {
            return BrowserAccess::Disconnected;
        };
        match endpoint.grants_scope(AuthEnvironmentScope::PreviewOperate) {
            None => BrowserAccess::Disconnected,
            Some(false) => BrowserAccess::Restricted,
            Some(true) => BrowserAccess::Connected {
                serial: connection.serial(),
                generation: connection.state().generation,
            },
        }
    }

    pub(super) fn endpoint(&self) -> Result<SessionEndpoint, ClientError> {
        let endpoint = self
            .runtime
            .client(&self.thread.environment)
            .and_then(|connection| connection.session())
            .ok_or(ClientError::Disconnected)?;
        match endpoint.grants_scope(AuthEnvironmentScope::PreviewOperate) {
            Some(true) => Ok(endpoint),
            Some(false) => Err(ClientError::PermissionDenied {
                method: "browser preview".into(),
            }),
            None => Err(ClientError::Disconnected),
        }
    }

    pub fn configured_urls(&self) -> Vec<TrimmedNonEmptyString> {
        let Some(shell) = self.runtime.store().shared_shell(&self.thread.environment) else {
            return Vec::new();
        };
        let project = shell
            .threads
            .iter()
            .chain(shell.archived_threads.iter())
            .find(|thread| thread.id == self.thread.thread)
            .and_then(|thread| shell.projects.iter().find(|p| p.id == thread.project_id));
        let urls = project
            .into_iter()
            .flat_map(|project| &project.scripts)
            .filter_map(|script| script.preview_url.as_ref().and_then(Option::as_ref))
            .map(|url| url.0.clone())
            .collect::<Vec<_>>();
        bound_configured_urls(&urls)
            .into_iter()
            .map(TrimmedNonEmptyString)
            .collect()
    }

    pub fn address(&self, raw: &str) -> Option<String> {
        resolve_address(raw, &self.runtime.settings().client_settings())
    }

    pub async fn list(&self) -> Result<PreviewListResult, ClientError> {
        self.endpoint()?
            .call::<WsPreviewListRpc>(PreviewListInput {
                thread_id: self.thread.thread.clone(),
            })
            .await
    }

    pub fn events(&self) -> LocalBoxStream<'static, Result<PreviewEvent, ClientError>> {
        let endpoint = match self.endpoint() {
            Ok(endpoint) => endpoint,
            Err(error) => return futures::stream::once(async { Err(error) }).boxed_local(),
        };
        let thread = self.thread.thread.clone();
        endpoint
            .subscribe::<WsSubscribePreviewEventsRpc>(Empty {})
            .filter(move |result| {
                let belongs = match result {
                    Ok(
                        PreviewEvent::Opened { thread_id, .. }
                        | PreviewEvent::Navigated { thread_id, .. }
                        | PreviewEvent::Resized { thread_id, .. }
                        | PreviewEvent::Failed { thread_id, .. }
                        | PreviewEvent::Closed { thread_id, .. },
                    ) => thread_id.0 == thread.0,
                    Err(_) => true,
                };
                futures::future::ready(belongs)
            })
            .boxed_local()
    }

    pub fn discovery(
        &self,
        configured: Vec<TrimmedNonEmptyString>,
    ) -> LocalBoxStream<'static, Result<Vec<DiscoveredLocalServer>, ClientError>> {
        let endpoint = match self.endpoint() {
            Ok(endpoint) => endpoint,
            Err(error) => return futures::stream::once(async { Err(error) }).boxed_local(),
        };
        endpoint
            .subscribe::<WsSubscribeDiscoveredLocalServersRpc>(
                WsSubscribeDiscoveredLocalServersPayload {
                    configured_urls: (!configured.is_empty()).then(|| Some(configured.clone())),
                },
            )
            .map(move |result| result.map(|list| discovered_servers(list, &configured)))
            .boxed_local()
    }

    pub async fn open(&self, url: String) -> Result<PreviewSessionSnapshot, BrowserCommandError> {
        let endpoint = self.endpoint().map_err(BrowserCommandError::Client)?;
        let settings = self.runtime.settings().client_settings();
        let input = apply_defaults(
            PreviewOpenInput {
                thread_id: self.thread.thread.clone(),
                url: Some(Some(TrimmedNonEmptyString(url))),
                viewport: None,
                profile_id: None,
                runtime: Some(Some(PreviewRuntime::Server)),
                reveal: None,
                color_scheme: None,
                zoom_factor: None,
            },
            &BrowserDefaults {
                viewport: settings.browser_default_viewport.clone(),
                profile_id: settings.browser_default_profile_id.clone(),
                zoom_factor: settings.browser_default_zoom_factor.clone(),
                appearance: settings.browser_default_appearance,
            },
            EntryPoint::Session,
        );
        endpoint
            .command::<WsPreviewOpenRpc>(input)
            .await
            .map_err(BrowserCommandError::from)
    }

    pub async fn close(&self, tab: PreviewTabId) -> Result<(), BrowserCommandError> {
        let endpoint = self.endpoint().map_err(BrowserCommandError::Client)?;
        endpoint
            .command::<WsPreviewCloseRpc>(PreviewCloseInput {
                thread_id: self.thread.thread.clone(),
                tab_id: Some(Some(tab)),
            })
            .await
            .map_err(BrowserCommandError::from)
    }
}

fn discovered_servers(
    list: DiscoveredLocalServerList,
    configured: &[TrimmedNonEmptyString],
) -> Vec<DiscoveredLocalServer> {
    use std::collections::BTreeMap;
    let mut configured_by_host = BTreeMap::new();
    for raw in configured {
        if let Ok(url) = url::Url::parse(&raw.0)
            && let Some(host) = url.host_str()
            && let Some(port) = url.port_or_known_default()
        {
            configured_by_host
                .entry(server_key(host, port))
                .or_insert(raw);
        }
    }
    let mut servers = list.servers;
    let key = |server: &DiscoveredLocalServer| server_key(&server.host.0, server.port);
    servers.sort_by_key(|server| (!configured_by_host.contains_key(&key(server)), server.port));
    if list.configured_url_probing.flatten().is_none() {
        for server in &mut servers {
            if let Some(url) = configured_by_host.get(&key(server)) {
                server.url = (*url).clone();
            }
        }
    }
    servers
}

fn server_key(host: &str, port: u16) -> (String, u16) {
    let host = host.to_ascii_lowercase();
    let host = match host.as_str() {
        "127.0.0.1" | "0.0.0.0" | "::1" | "[::1]" => "localhost".into(),
        _ => host,
    };
    (host, port)
}

#[derive(Debug, thiserror::Error)]
pub enum BrowserCommandError {
    #[error("Enter an http or https URL.")]
    InvalidUrl,
    #[error("This browser tab is closed.")]
    TabClosed,
    #[error("Take control of this browser tab first.")]
    ControlRequired,
    #[error("Browser unavailable")]
    Unavailable,
    #[error(transparent)]
    Client(ClientError),
}

impl From<RpcCallError> for BrowserCommandError {
    fn from(error: RpcCallError) -> Self {
        use t3_contracts::{preview::PreviewError, rpc::WsPreviewOpenError};
        match error {
            RpcCallError::Rejected(errors) => {
                for error in &errors {
                    if let Ok(WsPreviewOpenError::PreviewError(error)) =
                        serde_json::from_value(error.clone())
                    {
                        return match *error {
                            PreviewError::InvalidUrl(_) => Self::InvalidUrl,
                            PreviewError::SessionLookup(_) => Self::TabClosed,
                            PreviewError::ControlRequired(_) => Self::ControlRequired,
                        };
                    }
                }
                Self::Client(RpcCallError::Rejected(errors).into_client_error())
            }
            RpcCallError::Defect | RpcCallError::NotSent(ClientError::Unsupported(_)) => {
                Self::Unavailable
            }
            error => Self::Client(error.into_client_error()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_matches_hosts_and_ports_and_normalizes_loopback() {
        let row = |host: &str, url: &str| DiscoveredLocalServer {
            host: TrimmedNonEmptyString(host.into()),
            port: 5173,
            url: TrimmedNonEmptyString(url.into()),
            process_name: None,
            pid: None,
            terminal: None,
            agent: None,
        };
        let list = DiscoveredLocalServerList {
            servers: vec![
                row("other.example", "http://other.example:5173/"),
                row("127.0.0.1", "http://127.0.0.1:5173/"),
            ],
            scanned_at: String::new(),
            configured_url_probing: None,
        };
        let servers = discovered_servers(
            list,
            &[TrimmedNonEmptyString("http://localhost:5173/app".into())],
        );
        assert_eq!(servers[0].host.0, "127.0.0.1");
        assert_eq!(servers[0].url.0, "http://localhost:5173/app");
        assert_eq!(servers[1].url.0, "http://other.example:5173/");
    }
}
