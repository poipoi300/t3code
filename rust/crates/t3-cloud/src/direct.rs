use crate::{Error, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    time::Duration,
};
use t3_contracts::relay::RelayManagedEndpointOrigin;
use url::Url;

pub fn normalized_host(host: &str) -> String {
    host.trim_matches(['[', ']'])
        .trim_end_matches('.')
        .to_ascii_lowercase()
}
pub fn loopback(host: &str) -> bool {
    let host = normalized_host(host);
    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}
pub fn tailnet(host: &str) -> bool {
    let host = normalized_host(host);
    host.ends_with(".ts.net")
        || host
            .parse::<Ipv6Addr>()
            .is_ok_and(|ip| ip.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0])
}
pub fn tailnet_ipv4(host: &str) -> bool {
    host.parse::<Ipv4Addr>()
        .is_ok_and(|ip| ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
}
pub fn peer_origin(value: &str) -> Result<Url> {
    let mut url = Url::parse(value).map_err(|_| Error::Configuration)?;
    let host = url.host_str().ok_or(Error::Configuration)?;
    if !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.scheme(), "http" | "https")
        || (url.scheme() == "http" && !loopback(host) && !tailnet(host))
    {
        return Err(Error::Configuration);
    }
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}
pub fn validate_tunnel_origin(origin: &RelayManagedEndpointOrigin) -> Result<()> {
    if !matches!(
        normalized_host(origin.local_http_host.0.as_str()).as_str(),
        "127.0.0.1" | "::1" | "localhost"
    ) || origin.local_http_port == 0
    {
        return Err(Error::Configuration);
    }
    Ok(())
}
pub fn allowed_link_origin(
    origin: &RelayManagedEndpointOrigin,
    request_url: &str,
    allow_non_loopback: bool,
) -> Result<()> {
    validate_tunnel_origin(origin)?;
    let request = Url::parse(request_url).map_err(|_| Error::Configuration)?;
    if !matches!(request.scheme(), "http" | "https")
        || (!allow_non_loopback && !loopback(request.host_str().unwrap_or("")))
        || request.port_or_known_default() != Some(origin.local_http_port)
    {
        return Err(Error::Configuration);
    }
    Ok(())
}
pub fn parse_local_origin(value: &str) -> Result<RelayManagedEndpointOrigin> {
    let url = Url::parse(value).map_err(|_| Error::Configuration)?;
    if value != value.trim()
        || !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || value.contains(['?', '#'])
    {
        return Err(Error::Configuration);
    }
    Ok(RelayManagedEndpointOrigin {
        local_http_host: crate::text(url.host_str().ok_or(Error::Configuration)?),
        local_http_port: url.port_or_known_default().ok_or(Error::Configuration)?,
    })
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailscaleStatus {
    pub running: Option<bool>,
    pub magic_dns_name: Option<String>,
    pub tailnet_ipv4_addresses: Vec<String>,
    pub https_serve_enabled: Option<bool>,
}
pub fn parse_status(raw: &str) -> Result<TailscaleStatus> {
    let v: Value = serde_json::from_str(raw).map_err(|_| Error::Protocol)?;
    if !v.is_object() || v.get("Self").is_some_and(|v| !v.is_object()) {
        return Err(Error::Protocol);
    }
    Ok(TailscaleStatus {
        running: v.get("BackendState").map(|v| v == "Running"),
        magic_dns_name: v["Self"]["DNSName"]
            .as_str()
            .map(|s| s.trim().trim_end_matches('.').to_owned())
            .filter(|s| !s.is_empty()),
        tailnet_ipv4_addresses: v["Self"]["TailscaleIPs"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .filter(|s| tailnet_ipv4(s))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        https_serve_enabled: v["Self"]["CapMap"]
            .as_object()
            .map(|v| v.contains_key("https")),
    })
}
pub fn parse_ssh_enabled(raw: &str) -> Result<bool> {
    let v: Value = serde_json::from_str(raw).map_err(|_| Error::Protocol)?;
    if !v.is_object() {
        return Err(Error::Protocol);
    }
    Ok(v["RunSSH"] == true && v["WantRunning"] != false)
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitNode {
    pub name: Option<String>,
    pub online: bool,
}
pub fn parse_exit_node(raw: &str) -> Result<Option<ExitNode>> {
    let v: Value = serde_json::from_str(raw).map_err(|_| Error::Protocol)?;
    let Some(status) = v.get("ExitNodeStatus") else {
        return Ok(None);
    };
    let peer = v["Peer"]
        .as_object()
        .and_then(|v| v.values().find(|p| p["ExitNode"] == true));
    let name = peer
        .and_then(|p| {
            p["DNSName"]
                .as_str()
                .and_then(|s| s.split('.').next())
                .filter(|s| !s.is_empty())
                .or_else(|| p["HostName"].as_str().map(str::trim))
        })
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    Ok(Some(ExitNode {
        name,
        online: status["Online"] == true,
    }))
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeRoute {
    pub port: u16,
    pub mount: Option<String>,
    pub host: Option<String>,
    pub ambiguous: Option<String>,
    pub owner: Option<String>,
}
pub fn serve_off_args(route: &ServeRoute) -> Option<Vec<String>> {
    if route.owner.as_deref() == Some("foreground") {
        return None;
    }
    let mut args = vec!["serve".to_owned()];
    if let Some(owner) = &route.owner {
        args.push(format!("--service={owner}"));
    }
    match &route.mount {
        Some(mount) => args.extend([
            format!("--https={}", route.port),
            format!("--set-path={mount}"),
        ]),
        None => args.push(format!("--tcp={}", route.port)),
    }
    args.push("off".to_owned());
    Some(args)
}
fn dialed(proxy: &str) -> Option<(String, u16)> {
    let expanded = if proxy.bytes().all(|b| b.is_ascii_digit()) {
        format!("http://127.0.0.1:{proxy}")
    } else if let Some(p) = proxy.strip_prefix("https+insecure://") {
        format!("https://{p}")
    } else if proxy.contains("://") || proxy.starts_with("unix:") {
        proxy.to_owned()
    } else {
        format!("http://{proxy}")
    };
    let (scheme, rest) = expanded.split_once("://")?;
    if !matches!(scheme, "http" | "https" | "tcp") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?;
    let (host, port) = if authority.starts_with('[') {
        let (h, p) = authority.split_once(']')?;
        (
            format!("{h}]"),
            p.strip_prefix(':')
                .unwrap_or(if scheme == "https" { "443" } else { "80" }),
        )
    } else if let Some((h, p)) = authority.rsplit_once(':') {
        (h.to_owned(), p)
    } else {
        (
            authority.to_owned(),
            if scheme == "https" { "443" } else { "80" },
        )
    };
    Some((normalized_host(&host), port.parse().ok()?))
}
fn reaches(host: &str, bind: &str) -> Option<bool> {
    let host = normalized_host(host);
    let bind = normalized_host(bind);
    let literal = host.parse::<IpAddr>().ok();
    let Some(address) = literal else {
        return Some(true);
    };
    if address == IpAddr::V6(Ipv6Addr::UNSPECIFIED) {
        return matches!(bind.as_str(), "127.0.0.1" | "::1").then_some(true);
    }
    let address = match address {
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| ip.to_string()),
        IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_owned(),
        IpAddr::V4(ip) => ip.to_string(),
    };
    (address == bind).then_some(false)
}
pub fn parse_serve_routes(raw: &str, bind: &str, port: u16) -> Result<Vec<ServeRoute>> {
    let v: Value = serde_json::from_str(raw).map_err(|_| Error::Protocol)?;
    if !v.is_null() && !v.is_object() {
        return Err(Error::Protocol);
    }
    let mut routes = Vec::new();
    let mut visit = |config: &Value, owner: Option<&str>| {
        if let Some(tcp) = config["TCP"].as_object() {
            for (serve, handler) in tcp {
                let (Ok(serve), Some(target)) =
                    (serve.parse::<u16>(), handler["TCPForward"].as_str())
                else {
                    continue;
                };
                let Some((host, p)) = dialed(&format!("tcp://{target}")) else {
                    continue;
                };
                if p != port {
                    continue;
                }
                let Some(ambiguous) = reaches(&host, bind) else {
                    continue;
                };
                routes.push(ServeRoute {
                    port: serve,
                    mount: None,
                    host: None,
                    ambiguous: ambiguous.then_some(host),
                    owner: owner.map(str::to_owned),
                });
            }
        }
        if let Some(web) = config["Web"].as_object() {
            for (host_port, site) in web {
                let Some((host, serve)) = host_port.rsplit_once(':') else {
                    continue;
                };
                let Ok(serve) = serve.parse::<u16>() else {
                    continue;
                };
                let Some(handlers) = site["Handlers"].as_object() else {
                    continue;
                };
                for (mount, handler) in handlers {
                    let Some((target, p)) = handler["Proxy"].as_str().and_then(dialed) else {
                        continue;
                    };
                    if p != port {
                        continue;
                    }
                    let Some(ambiguous) = reaches(&target, bind) else {
                        continue;
                    };
                    routes.push(ServeRoute {
                        port: serve,
                        mount: Some(mount.clone()),
                        host: Some(host.to_owned()),
                        ambiguous: ambiguous.then_some(target),
                        owner: owner.map(str::to_owned),
                    });
                }
            }
        }
    };
    visit(&v, None);
    if let Some(services) = v["Services"].as_object() {
        for (owner, service) in services {
            visit(service, Some(owner));
        }
    }
    if let Some(foreground) = v["Foreground"].as_object() {
        for session in foreground.values() {
            visit(session, Some("foreground"));
        }
    }
    Ok(routes)
}

#[async_trait]
pub trait Commands: Send + Sync {
    async fn run(&self, args: &[String], timeout: Duration) -> Result<String>;
}
pub struct NativeTailscale;
#[async_trait]
impl Commands for NativeTailscale {
    async fn run(&self, args: &[String], timeout: Duration) -> Result<String> {
        let mut options = t3_process::ProcessOptions::new(if cfg!(windows) {
            "tailscale.exe"
        } else {
            "tailscale"
        })
        .args(args);
        options.timeout = Some(timeout);
        options.termination_grace = Duration::from_millis(500);
        options.max_output_bytes = 1024 * 1024;
        let output = t3_process::run(options).await.map_err(|_| Error::Process)?;
        Ok(output.stdout_text())
    }
}
pub struct Tailscale<C = NativeTailscale>(pub C);
impl<C: Commands> Tailscale<C> {
    pub async fn status(&self) -> Result<TailscaleStatus> {
        parse_status(
            &self
                .0
                .run(
                    &["status".into(), "--json".into()],
                    Duration::from_millis(3500),
                )
                .await?,
        )
    }
    pub async fn routes(&self, bind: &str, port: u16) -> Result<Vec<ServeRoute>> {
        parse_serve_routes(
            &self
                .0
                .run(
                    &["serve".into(), "status".into(), "--json".into()],
                    Duration::from_millis(3500),
                )
                .await?,
            bind,
            port,
        )
    }
    pub async fn ensure_serve(
        &self,
        local_port: u16,
        serve_port: u16,
        local_host: &str,
    ) -> Result<()> {
        if local_port == 0 || serve_port == 0 || !loopback(local_host) {
            return Err(Error::Configuration);
        }
        if self
            .status()
            .await
            .is_ok_and(|s| s.https_serve_enabled == Some(false))
        {
            return Err(Error::Disabled);
        }
        let host = if local_host.contains(':') && !local_host.starts_with('[') {
            format!("[{local_host}]")
        } else {
            local_host.to_owned()
        };
        self.0
            .run(
                &[
                    "serve".into(),
                    "--bg".into(),
                    format!("--https={serve_port}"),
                    format!("http://{host}:{local_port}"),
                ],
                Duration::from_secs(10),
            )
            .await?;
        Ok(())
    }
    pub async fn disable(&self, route: &ServeRoute) -> Result<()> {
        let args = serve_off_args(route).ok_or(Error::Configuration)?;
        self.0.run(&args, Duration::from_secs(10)).await?;
        Ok(())
    }
}
pub fn lan_endpoints(addresses: impl IntoIterator<Item = IpAddr>, port: u16) -> Result<Vec<Url>> {
    if port == 0 {
        return Err(Error::Configuration);
    }
    let mut endpoints = Vec::new();
    for ip in addresses {
        if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
            continue;
        }
        let host = match ip {
            IpAddr::V4(ip) => ip.to_string(),
            IpAddr::V6(ip) => format!("[{ip}]"),
        };
        let url = Url::parse(&format!("http://{host}:{port}")).map_err(|_| Error::Configuration)?;
        if !endpoints.contains(&url) {
            endpoints.push(url);
        }
    }
    Ok(endpoints)
}
