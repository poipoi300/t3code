use crate::{
    Result, SharedClock,
    direct::{Commands, ExitNode, NativeTailscale},
    http::{Http, Request},
    iso,
    tunnel::ConnectorFacts,
};
use async_trait::async_trait;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use t3_contracts::environment_http::{
    EnvironmentConnectReachability as Reachability, EnvironmentConnectReachabilityCause as Cause,
};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub enum EndpointProbe {
    Status {
        status: u16,
        cloudflare_error: Option<String>,
    },
    DnsFailed {
        exists_publicly: Option<bool>,
    },
    Timeout,
    NetworkError,
}
#[derive(Debug, Clone)]
pub struct ReachabilityFacts {
    pub configured: bool,
    pub connector: Option<ConnectorFacts>,
    pub endpoint: Option<EndpointProbe>,
    pub edge_reachable: Option<bool>,
    pub exit_node: Option<ExitNode>,
}
#[async_trait]
pub trait ReachabilityProbe: Send + Sync {
    async fn endpoint(&self, hostname: &str) -> EndpointProbe;
    async fn edge_reachable(&self) -> bool;
    async fn exit_node(&self) -> Option<ExitNode>;
}
pub struct NativeReachabilityProbe {
    pub http: Http,
}
impl NativeReachabilityProbe {
    async fn get(&self, value: &str, timeout: Duration) -> Result<crate::http::Response> {
        self.http
            .send(Request {
                method: reqwest::Method::GET,
                url: url::Url::parse(value).map_err(|_| crate::Error::Configuration)?,
                headers: BTreeMap::from([(
                    "accept".into(),
                    crate::Secret::new("application/json"),
                )]),
                body: Vec::new(),
                timeout,
            })
            .await
    }
}
#[async_trait]
impl ReachabilityProbe for NativeReachabilityProbe {
    async fn endpoint(&self, hostname: &str) -> EndpointProbe {
        let valid = url::Url::parse(&format!("https://{hostname}"));
        if !valid.is_ok_and(|u| {
            u.host_str() == Some(hostname)
                && u.port().is_none()
                && u.path() == "/"
                && u.query().is_none()
                && u.fragment().is_none()
                && u.username().is_empty()
                && u.password().is_none()
        }) {
            return EndpointProbe::NetworkError;
        }
        let dns = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::lookup_host((hostname, 443)),
        )
        .await;
        if matches!(&dns, Ok(Err(_))) {
            let mut dns_url = match url::Url::parse("https://1.1.1.1/dns-query") {
                Ok(url) => url,
                Err(_) => return EndpointProbe::NetworkError,
            };
            dns_url
                .query_pairs_mut()
                .append_pair("name", hostname)
                .append_pair("type", "A");
            let exists_publicly = self
                .get(dns_url.as_str(), Duration::from_secs(5))
                .await
                .ok()
                .and_then(|r| serde_json::from_slice::<serde_json::Value>(&r.body).ok())
                .and_then(|v| match v["Status"].as_i64() {
                    Some(0) => Some(true),
                    Some(3) => Some(false),
                    _ => None,
                });
            return EndpointProbe::DnsFailed { exists_publicly };
        }
        if dns.is_err() {
            return EndpointProbe::DnsFailed {
                exists_publicly: None,
            };
        }
        match tokio::time::timeout(
            Duration::from_secs(10),
            self.get(
                &format!("https://{hostname}/.well-known/t3/environment"),
                Duration::from_secs(10),
            ),
        )
        .await
        {
            Err(_) => EndpointProbe::Timeout,
            Ok(Err(_)) => EndpointProbe::NetworkError,
            Ok(Ok(response)) => {
                let body = String::from_utf8_lossy(&response.body);
                let code = ["error_code\":", "error code:"].iter().find_map(|needle| {
                    body.find(needle)
                        .and_then(|at| body[at + needle.len()..].trim_start().get(..4))
                        .filter(|code| code.bytes().all(|c| c.is_ascii_digit()))
                        .map(str::to_owned)
                });
                EndpointProbe::Status {
                    status: response.status,
                    cloudflare_error: code,
                }
            }
        }
    }
    async fn edge_reachable(&self) -> bool {
        let connect = async |host| {
            tokio::time::timeout(
                Duration::from_secs(5),
                tokio::net::TcpStream::connect((host, 7844)),
            )
            .await
            .is_ok_and(|r| r.is_ok())
        };
        let (a, b) = tokio::join!(
            connect("region1.v2.argotunnel.com"),
            connect("region2.v2.argotunnel.com")
        );
        a || b
    }
    async fn exit_node(&self) -> Option<ExitNode> {
        NativeTailscale
            .run(
                &["status".into(), "--json".into()],
                Duration::from_millis(3500),
            )
            .await
            .ok()
            .and_then(|raw| crate::direct::parse_exit_node(&raw).ok())
            .flatten()
    }
}
pub struct ConnectReachability {
    probe: Arc<dyn ReachabilityProbe>,
    clock: SharedClock,
    order: AtomicU64,
    latest: Mutex<Option<(u64, Reachability)>>,
}
impl ConnectReachability {
    pub fn new(probe: Arc<dyn ReachabilityProbe>, clock: SharedClock) -> Self {
        Self {
            probe,
            clock,
            order: AtomicU64::new(0),
            latest: Mutex::new(None),
        }
    }
    pub async fn latest(&self) -> Option<Reachability> {
        self.latest
            .lock()
            .await
            .as_ref()
            .map(|(_, result)| result.clone())
    }
    pub async fn check(
        &self,
        configured: bool,
        connector: Option<ConnectorFacts>,
    ) -> Result<Reachability> {
        let order = self.order.fetch_add(1, Ordering::Relaxed);
        let endpoint = if let Some(c) = &connector
            && c.ready_connections != Some(0)
            && let Some(host) = &c.hostname
        {
            Some(self.probe.endpoint(host).await)
        } else {
            None
        };
        let mut facts = ReachabilityFacts {
            configured,
            connector,
            endpoint,
            edge_reachable: None,
            exit_node: None,
        };
        if facts.connector.is_some()
            && !matches!(
                facts.endpoint,
                Some(EndpointProbe::Status { status: 200, .. })
            )
        {
            let (edge, exit) = tokio::join!(self.probe.edge_reachable(), self.probe.exit_node());
            facts.edge_reachable = Some(edge);
            facts.exit_node = exit;
        }
        let result = classify(&facts, iso(self.clock.now_ms())?);
        let mut latest = self.latest.lock().await;
        if latest
            .as_ref()
            .is_none_or(|(previous, _)| *previous <= order)
        {
            *latest = Some((order, result.clone()));
        }
        Ok(result)
    }
}
pub fn classify(facts: &ReachabilityFacts, checked_at: String) -> Reachability {
    let fail = |cause, reason: &str, fix: &str, hint: Option<String>| Reachability::Unreachable {
        checked_at: checked_at.clone(),
        cause,
        reason: reason.into(),
        fix: fix.into(),
        hint,
    };
    let restart = "Restart T3 Code. If it stays, run `t3 connect` again.";
    let exit_hint = facts.exit_node.as_ref().map(|exit| {
        format!(
            "Tailscale exit node{} is on. Traffic to Cloudflare goes through it.",
            exit.name
                .as_ref()
                .map(|name| format!(" \"{name}\""))
                .unwrap_or_default()
        )
    });
    if !facts.configured {
        return fail(
            Cause::NotLinked,
            "The server has not set up a T3 Connect tunnel.",
            "Run `t3 connect`, then restart T3 Code so it applies the link.",
            None,
        );
    }
    let Some(connector) = &facts.connector else {
        return fail(
            Cause::ConnectorNotRunning,
            "The tunnel client (cloudflared) is not running.",
            "Restart T3 Code. If it stays down, check the server log for relay client errors.",
            None,
        );
    };
    let status = match &facts.endpoint {
        Some(EndpointProbe::Status { status, .. }) => Some(*status),
        _ => None,
    };
    if status == Some(200) {
        return Reachability::Reachable {
            checked_at,
            ready_connections: connector
                .ready_connections
                .and_then(|n| i64::try_from(n).ok()),
            protocol: connector.http2.then(|| "http2".into()),
        };
    }
    if facts.edge_reachable == Some(false)
        && status != Some(502)
        && connector.ready_connections.unwrap_or(0) == 0
        && !matches!(facts.endpoint, Some(EndpointProbe::DnsFailed { .. }))
    {
        return fail(
            Cause::EdgeUnreachable,
            "Cloudflare's tunnel port (7844) times out from this machine.",
            if facts.exit_node.is_some() {
                "tailscale set --exit-node=   (or allow outbound 7844 on that network)"
            } else {
                "Allow outbound TCP and UDP port 7844, or turn off the VPN or firewall that blocks it."
            },
            exit_hint.or(Some("A VPN, proxy, or firewall may block it.".into())),
        );
    }
    if status == Some(502) {
        return fail(
            Cause::OriginUnreachable,
            "The tunnel is up, but it cannot reach the T3 server on this machine.",
            "Restart T3 Code so the tunnel uses the port the server listens on.",
            None,
        );
    }
    if facts.endpoint.is_none() || connector.ready_connections == Some(0) {
        return fail(
            Cause::ConnectorNotReady,
            "cloudflared has not connected to Cloudflare yet.",
            "Wait a minute. If it stays, check the network and the server log.",
            None,
        );
    }
    let code = match &facts.endpoint {
        Some(EndpointProbe::Status {
            cloudflare_error, ..
        }) => cloudflare_error.as_deref(),
        _ => None,
    };
    if status == Some(530) && code.is_none_or(|c| c == "1033") {
        return fail(
            Cause::NoConnector,
            "Cloudflare has no live connection from this machine for the tunnel.",
            restart,
            None,
        );
    }
    if let Some(status) = status {
        return fail(
            Cause::UnexpectedResponse,
            &format!(
                "The T3 Connect address answered with HTTP {status}{}.",
                code.map(|code| format!(" (Cloudflare error {code})"))
                    .unwrap_or_default()
            ),
            restart,
            None,
        );
    }
    if let Some(EndpointProbe::DnsFailed { exists_publicly }) = &facts.endpoint {
        return match exists_publicly {
            Some(true) => fail(
                Cause::DnsFailed,
                "The T3 Connect address resolves in public DNS, but this network's DNS still has an old cached answer that it does not exist.",
                "Wait. The cached answer usually clears within about 30 minutes.",
                exit_hint,
            ),
            Some(false) => fail(
                Cause::DnsFailed,
                "The T3 Connect address is missing from public DNS. The relay did not publish it.",
                restart,
                None,
            ),
            None => fail(
                Cause::DnsFailed,
                "The T3 Connect address does not resolve from this machine.",
                "Check the DNS settings on this machine or its VPN.",
                exit_hint,
            ),
        };
    }
    fail(
        Cause::Timeout,
        "The T3 Connect address did not answer from this machine.",
        "Check the network, VPN, or proxy on this machine.",
        exit_hint,
    )
}
