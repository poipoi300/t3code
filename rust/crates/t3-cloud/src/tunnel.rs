use crate::{Error, Result, Secret, SharedClock, http::Sensitive};
use async_trait::async_trait;
use std::{collections::VecDeque, ffi::OsString, path::Path, sync::Arc, time::Duration};
use t3_contracts::relay::{RelayManagedEndpointProviderKind, RelayManagedEndpointRuntimeConfig};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader},
    sync::{mpsc, watch},
};
use tokio_stream::wrappers::ReceiverStream;

pub fn validate_runtime(config: &RelayManagedEndpointRuntimeConfig) -> Result<()> {
    if config.provider_kind != RelayManagedEndpointProviderKind::CloudflareTunnel
        || config.connector_token.0.as_str().trim().is_empty()
    {
        return Err(Error::Configuration);
    }
    Ok(())
}
pub fn connector_options(
    executable: &Path,
    config: &Sensitive<RelayManagedEndpointRuntimeConfig>,
    http2: bool,
) -> Result<t3_process::ProcessOptions> {
    let config = config.expose();
    validate_runtime(config)?;
    let mut args = vec![
        "tunnel",
        "--no-autoupdate",
        "--loglevel",
        "info",
        "--output",
        "default",
        "--metrics",
        "127.0.0.1:0",
    ];
    if http2 {
        args.extend(["--protocol", "http2"]);
    }
    args.push("run");
    let mut options = t3_process::ProcessOptions::new(executable).args(args);
    options.env.insert(
        OsString::from("TUNNEL_TOKEN"),
        Some(OsString::from(config.connector_token.0.as_str())),
    );
    options.timeout = None;
    Ok(options)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorEvent {
    Connected,
    Unregistered,
    Warning,
    RecoveryRequired,
    SwitchHttp2,
    Restarted,
}
#[derive(Debug, Default)]
pub struct ConnectorState {
    pub metrics_port: Option<u16>,
    pub http2: bool,
    rejected: u8,
    stalls: VecDeque<i64>,
    restart_delay: u64,
    transport_chosen: bool,
}
impl ConnectorState {
    pub fn with_transport_choice(chosen: bool) -> Self {
        Self {
            transport_chosen: chosen,
            ..Default::default()
        }
    }
    /// Never returns raw output: cloudflared can echo a token in a failure.
    pub fn observe(&mut self, line: &str, now: i64) -> Option<ConnectorEvent> {
        let lower = line.to_lowercase();
        if lower.contains("registered tunnel connection")
            && !lower.contains("unregistered tunnel connection")
        {
            self.rejected = 0;
            if line.contains("protocol=http2") {
                self.http2 = true;
            }
            return Some(ConnectorEvent::Connected);
        }
        if let Some(rest) = line.split("Starting metrics server on 127.0.0.1:").nth(1) {
            if let Some(port) = rest
                .split("/metrics")
                .next()
                .and_then(|p| p.parse::<u16>().ok())
                .filter(|p| *p > 0)
            {
                self.metrics_port = Some(port);
            }
            return None;
        }
        let warning = line
            .split_whitespace()
            .any(|s| matches!(s, "ERR" | "WRN" | "FTL" | "PNC"));
        if warning {
            if line.contains("timeout: no recent network activity")
                && !self.http2
                && !self.transport_chosen
            {
                self.stalls.retain(|at| now.saturating_sub(*at) < 300_000);
                self.stalls.push_back(now);
                if self.stalls.len() >= 30 {
                    self.http2 = true;
                    return Some(ConnectorEvent::SwitchHttp2);
                }
            }
            if lower.contains("register tunnel error from server side")
                && [
                    "unauthorized:",
                    "failed to get tunnel",
                    "tunnel not found",
                    "record for tunnel not found",
                    "invalid tunnel secret",
                ]
                .iter()
                .any(|s| lower.contains(s))
            {
                self.rejected += 1;
                if self.rejected >= 4 {
                    self.rejected = 0;
                    return Some(ConnectorEvent::RecoveryRequired);
                }
            }
            return Some(ConnectorEvent::Warning);
        }
        lower
            .contains("unregistered tunnel connection")
            .then_some(ConnectorEvent::Unregistered)
    }
    pub fn exited(&mut self, uptime_ms: i64) -> Duration {
        if uptime_ms >= 30_000 {
            self.restart_delay = 0;
            return Duration::ZERO;
        }
        let delay = self.restart_delay;
        self.restart_delay = if delay == 0 {
            1000
        } else {
            (delay * 2).min(60_000)
        };
        Duration::from_millis(delay)
    }
}
pub struct ConnectorHandle {
    pub events: watch::Receiver<Option<ConnectorEvent>>,
    pub facts: watch::Receiver<ConnectorFacts>,
    cancel: t3_process::CancellationToken,
    task: tokio::task::JoinHandle<Result<()>>,
}
#[derive(Debug, Clone, Default)]
pub struct ConnectorFacts {
    pub pid: Option<u32>,
    pub metrics_port: Option<u16>,
    pub ready_connections: Option<u64>,
    pub hostname: Option<String>,
    pub http2: bool,
}
pub struct ConnectorServices {
    pub http: crate::http::Http,
    pub secrets: crate::store::SecretStore,
}
pub async fn inspect_metrics(
    http: &dyn crate::http::Transport,
    port: u16,
) -> Result<(u64, Option<String>)> {
    if port == 0 {
        return Err(Error::Configuration);
    }
    let get = async |path: &str| -> Result<serde_json::Value> {
        let request = crate::http::Request {
            method: reqwest::Method::GET,
            url: url::Url::parse(&format!("http://127.0.0.1:{port}/{path}"))
                .map_err(|_| Error::Configuration)?,
            headers: Default::default(),
            body: Vec::new(),
            timeout: Duration::from_secs(5),
        };
        let response = tokio::time::timeout(Duration::from_secs(5), http.send(request))
            .await
            .map_err(|_| Error::Io)??;
        if !matches!(response.status, 200 | 503) || response.body.len() > 1024 * 1024 {
            return Err(Error::Protocol);
        }
        serde_json::from_slice(&response.body).map_err(|_| Error::Protocol)
    };
    let ready = get("ready").await?["readyConnections"]
        .as_u64()
        .ok_or(Error::Protocol)?;
    let config = get("config").await?;
    let hostname = config["config"]["ingress"]
        .as_array()
        .and_then(|rules| rules.iter().find_map(|r| r["hostname"].as_str()))
        .map(str::to_owned);
    Ok((ready, hostname))
}
async fn read_line<R: AsyncBufRead + Unpin>(reader: &mut R, output: &mut Vec<u8>) -> Result<usize> {
    loop {
        let buffer = reader.fill_buf().await.map_err(|_| Error::Process)?;
        if buffer.is_empty() {
            return Ok(output.len());
        }
        let end = buffer.iter().position(|b| *b == b'\n').map(|i| i + 1);
        let used = end.unwrap_or(buffer.len());
        if output.len().saturating_add(used) > 64 * 1024 {
            return Err(Error::Process);
        }
        output.extend_from_slice(&buffer[..used]);
        reader.consume(used);
        if end.is_some() {
            return Ok(output.len());
        }
    }
}
impl ConnectorHandle {
    pub async fn cancel_and_wait(&mut self) -> Result<()> {
        self.cancel.cancel();
        (&mut self.task).await.map_err(|_| Error::Process)?
    }
    pub async fn stop(mut self) -> Result<()> {
        self.cancel.cancel();
        (&mut self.task).await.map_err(|_| Error::Process)?
    }
}
impl Drop for ConnectorHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}
/// Owns only its connector child; dropping the handle aborts and kills that child's tree.
pub fn start_connector(
    executable: std::path::PathBuf,
    config: Sensitive<RelayManagedEndpointRuntimeConfig>,
    clock: SharedClock,
    services: ConnectorServices,
) -> Result<ConnectorHandle> {
    validate_runtime(config.expose())?;
    let cancel = t3_process::CancellationToken::new();
    let cancellation = cancel.clone();
    let (events, receiver) = watch::channel(None);
    let (facts, facts_receiver) = watch::channel(ConnectorFacts::default());
    let explicit = std::env::var_os("TUNNEL_TRANSPORT_PROTOCOL").is_some();
    let task = tokio::spawn(async move {
        let mut state = ConnectorState::with_transport_choice(explicit);
        if !explicit
            && let Ok(Some(raw)) =
                crate::store::read_text(services.secrets.as_ref(), "cloud-endpoint-http2-fallback")
                    .await
        {
            let previous = serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .and_then(|v| v["fellBackAt"].as_i64());
            state.http2 = previous.is_some_and(|at| {
                clock.now_ms() >= at && clock.now_ms().saturating_sub(at) < 86_400_000
            });
        }
        loop {
            if cancellation.is_cancelled() {
                return Ok(());
            }
            let started = clock.now_ms();
            let options = connector_options(&executable, &config, state.http2 && !explicit)?;
            let mut child = t3_process::spawn(&options).map_err(|_| Error::Process)?;
            facts.send_replace(ConnectorFacts {
                pid: child.id(),
                http2: state.http2,
                ..Default::default()
            });
            state.metrics_port = None;
            let stdout = child.stdout.take().ok_or(Error::Process)?;
            let stderr = child.stderr.take().ok_or(Error::Process)?;
            let mut out = BufReader::new(stdout);
            let mut err = BufReader::new(stderr);
            let mut out_buf = Vec::new();
            let mut err_buf = Vec::new();
            let mut out_closed = false;
            let mut err_closed = false;
            let registration = tokio::time::sleep(Duration::from_secs(180));
            tokio::pin!(registration);
            let mut registered = false;
            let mut restart = false;
            let mut readiness = tokio::time::interval_at(
                tokio::time::Instant::now() + Duration::from_secs(30),
                Duration::from_secs(30),
            );
            readiness.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut not_ready = 0_u32;
            loop {
                let data = tokio::select! {
                    _=cancellation.cancelled()=>{child.kill().await.map_err(|_|Error::Process)?;return Ok(());},
                    _=&mut registration, if !registered=>{registered=true;events.send_replace(Some(ConnectorEvent::RecoveryRequired));continue;},
                    result=child.wait()=>{result.map_err(|_|Error::Process)?;break;},
                    _=readiness.tick(), if state.metrics_port.is_some()=>{
                        if let Some(port)=state.metrics_port {
                            match inspect_metrics(services.http.as_ref(),port).await {
                                Ok((connections,hostname))=>{facts.send_modify(|f|{f.ready_connections=Some(connections);f.hostname=hostname;});if connections>0 {not_ready=0;}else{not_ready+=1;}},
                                Err(_)=>not_ready+=1,
                            }
                            if not_ready>=6 {restart=true;break;}
                        }
                        continue;
                    },
                    result=read_line(&mut out,&mut out_buf), if !out_closed=>{let n=result?;if n==0 {out_closed=true;}std::mem::take(&mut out_buf)},
                    result=read_line(&mut err,&mut err_buf), if !err_closed=>{let n=result?;if n==0 {err_closed=true;}std::mem::take(&mut err_buf)},
                };
                if data.len() > 64 * 1024 {
                    return Err(Error::Process);
                }
                let was_http2 = state.http2;
                let event = state.observe(&String::from_utf8_lossy(&data), clock.now_ms());
                facts.send_modify(|f| {
                    f.metrics_port = state.metrics_port;
                    f.http2 = state.http2;
                });
                if !was_http2 && state.http2 && !explicit {
                    let encoded =
                        serde_json::to_vec(&serde_json::json!({"fellBackAt":clock.now_ms()}))
                            .map_err(|_| Error::Protocol)?;
                    services
                        .secrets
                        .set("cloud-endpoint-http2-fallback", &encoded)
                        .await?;
                }
                if let Some(event) = event {
                    if event == ConnectorEvent::Connected {
                        registered = true;
                    }
                    events.send_replace(Some(event));
                    if event == ConnectorEvent::SwitchHttp2 {
                        restart = true;
                        break;
                    }
                }
            }
            if restart {
                child.kill().await.map_err(|_| Error::Process)?;
            }
            let delay = state.exited(clock.now_ms().saturating_sub(started));
            tokio::select! {_=cancellation.cancelled()=>return Ok(()),_=tokio::time::sleep(delay)=>{}}
            events.send_replace(Some(ConnectorEvent::Restarted));
        }
    });
    Ok(ConnectorHandle {
        events: receiver,
        facts: facts_receiver,
        cancel,
        task,
    })
}

pub trait ConnectionIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> ConnectionIo for T {}
pub type BoxIo = Box<dyn ConnectionIo>;

#[derive(Debug, Default)]
pub struct Principal {
    pub session_id: String,
    pub subject: String,
    pub scopes: Vec<t3_contracts::auth::AuthEnvironmentScope>,
    pub proof_key_thumbprint: Option<String>,
    pub runtime_mode_ceiling: Option<t3_contracts::provider_policy::RuntimeMode>,
}
#[derive(Debug)]
pub struct UpgradeCredentials {
    pub bearer: Option<Secret>,
    pub dpop: Option<Secret>,
    pub web_socket_ticket: Option<Secret>,
    pub method: String,
    pub absolute_url: Secret,
    pub cookies: std::collections::BTreeMap<String, Secret>,
    pub origin: Option<Secret>,
}
#[async_trait]
pub trait ConnectionAuthenticator: Send + Sync {
    /// Must run ordinary environment WS auth, including ticket use, DPoP and revocation.
    /// The connector's loopback source address confers no identity or permission.
    async fn authenticate(&self, credentials: &UpgradeCredentials) -> Result<Principal>;
}
#[cfg(feature = "auth")]
#[async_trait]
impl ConnectionAuthenticator for t3_auth::EnvironmentAuth {
    async fn authenticate(&self, credentials: &UpgradeCredentials) -> Result<Principal> {
        if credentials.method != "GET" {
            return Err(Error::Proof);
        }
        let mut url =
            url::Url::parse(credentials.absolute_url.expose()).map_err(|_| Error::Proof)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(Error::Proof);
        }
        if let Some(ticket) = &credentials.web_socket_ticket {
            let existing = url
                .query_pairs()
                .filter(|(k, _)| k == "wsTicket")
                .collect::<Vec<_>>();
            if existing.len() > 1 || existing.first().is_some_and(|(_, v)| v != ticket.expose()) {
                return Err(Error::Proof);
            }
            if existing.is_empty() {
                url.query_pairs_mut()
                    .append_pair("wsTicket", ticket.expose());
            }
        }
        let mut request = t3_auth::Request {
            method: credentials.method.clone(),
            url: url.into(),
            ..Default::default()
        };
        if let Some(bearer) = &credentials.bearer {
            request.headers.insert(
                "authorization".into(),
                format!(
                    "{} {}",
                    if credentials.dpop.is_some() {
                        "DPoP"
                    } else {
                        "Bearer"
                    },
                    bearer.expose()
                ),
            );
        }
        if let Some(dpop) = &credentials.dpop {
            request
                .headers
                .insert("dpop".into(), dpop.expose().to_owned());
        }
        if let Some(origin) = &credentials.origin {
            request
                .headers
                .insert("origin".into(), origin.expose().to_owned());
        }
        request.cookies = credentials
            .cookies
            .iter()
            .map(|(k, v)| (k.clone(), v.expose().to_owned()))
            .collect();
        let verified = self
            .authenticate_websocket(&request, t3_contracts::auth::AuthWebSocketTicketUse::Rpc)
            .await
            .map_err(|_| Error::Proof)?;
        Ok(Principal {
            session_id: verified.session_id.0,
            subject: verified.subject,
            scopes: verified.scopes,
            proof_key_thumbprint: verified.proof_key_thumbprint,
            runtime_mode_ceiling: verified.runtime_mode_ceiling,
        })
    }
}
pub struct AcceptedConnection {
    pub io: BoxIo,
    pub principal: Principal,
}
pub struct Ingress {
    sender: mpsc::Sender<AcceptedConnection>,
    auth: Arc<dyn ConnectionAuthenticator>,
}
impl Ingress {
    pub fn channel(
        capacity: usize,
        auth: Arc<dyn ConnectionAuthenticator>,
    ) -> Result<(Self, ReceiverStream<AcceptedConnection>)> {
        if capacity == 0 {
            return Err(Error::Configuration);
        }
        let (sender, receiver) = mpsc::channel(capacity);
        Ok((Self { sender, auth }, ReceiverStream::new(receiver)))
    }
    /// Give the upgraded application byte stream to the same RPC handler as local upgrades.
    pub async fn accept(&self, io: BoxIo, credentials: UpgradeCredentials) -> Result<()> {
        let principal = self.auth.authenticate(&credentials).await?;
        self.sender
            .send(AcceptedConnection { io, principal })
            .await
            .map_err(|_| Error::Io)
    }
}
