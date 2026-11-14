use crate::{
    Error, Result, Secret, SharedClock, crypto, direct,
    http::{Client, Http, Request, Response, Sensitive, check},
    iso,
    store::{SecretStore, read_text},
};
use futures_util::{StreamExt, stream::FuturesUnordered};
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use t3_contracts::{
    environment::ExecutionEnvironmentDescriptor,
    peer_link::{PeerLink, PeerLinkCreateInput, PeerLinkStatus, PeerLinkSummary},
};
use t3_persistence::{
    Database,
    rusqlite::{self, OptionalExtension, params},
};
use tokio::sync::Mutex;
use url::Url;

const REDIRECT: &str = "http://127.0.0.1/t3code-peer-link";
#[derive(Clone)]
struct Row {
    link: PeerLink,
    secret_name: String,
}
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    let urls: String = r.get("urls_json")?;
    let access: String = r.get("access")?;
    let conversion = || {
        rusqlite::Error::InvalidColumnType(0, "peer link".to_owned(), rusqlite::types::Type::Text)
    };
    let linked_at: String = r.get("linked_at")?;
    let fenced: i64 = r.get("fenced")?;
    Ok(Row {
        secret_name: r.get("secret_name")?,
        link: PeerLink {
            environment_id: r.get::<_, String>("environment_id")?.into(),
            label: r.get("label")?,
            urls: serde_json::from_str(&urls).map_err(|_| conversion())?,
            access: serde_json::from_value(json!(access)).map_err(|_| conversion())?,
            linked_at: t3_contracts::wire::WireDateTime(linked_at.clone()),
            expires_at: t3_contracts::wire::WireDateTime(if fenced == 1 {
                r.get("expires_at")?
            } else {
                linked_at
            }),
            last_reached_at: r
                .get::<_, Option<String>>("last_reached_at")?
                .map(t3_contracts::wire::WireDateTime),
            last_error: r.get("last_error")?,
        },
    })
}
pub struct PeerLinks {
    db: Database,
    secrets: SecretStore,
    http: Http,
    clock: SharedClock,
    self_descriptor: ExecutionEnvironmentDescriptor,
    pub enabled: AtomicBool,
    mutation: Mutex<()>,
}
pub struct ResolvedPeer {
    pub link: PeerLink,
    pub origin: Url,
    pub token: Secret,
}
impl std::fmt::Debug for ResolvedPeer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedPeer")
            .field("environment_id", &self.link.environment_id)
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}
fn limits_hold(descriptor: &ExecutionEnvironmentDescriptor) -> bool {
    descriptor.capabilities.mcp_mode_limit_header == Some(true)
        && descriptor.capabilities.link_fence == Some(true)
}
fn expired(link: &PeerLink, now: i64) -> Result<bool> {
    Ok(chrono::DateTime::parse_from_rfc3339(&link.expires_at.0)
        .map_err(|_| Error::Storage)?
        .timestamp_millis()
        <= now)
}
impl PeerLinks {
    pub fn new(
        db: Database,
        secrets: SecretStore,
        http: Http,
        clock: SharedClock,
        self_descriptor: ExecutionEnvironmentDescriptor,
    ) -> Self {
        Self {
            db,
            secrets,
            http,
            clock,
            self_descriptor,
            enabled: AtomicBool::new(false),
            mutation: Mutex::new(()),
        }
    }
    async fn read(&self, id: &str) -> Result<Option<Row>> {
        let id = id.to_owned();
        self.db
            .reader(move |c| {
                Ok(c.query_row(
                    "SELECT * FROM peer_environment_links WHERE environment_id=?1",
                    [id],
                    row,
                )
                .optional()?)
            })
            .await
            .map_err(|_| Error::Storage)
    }
    pub async fn get(&self, id: &str) -> Result<Option<PeerLink>> {
        Ok(self.read(id).await?.map(|r| r.link))
    }
    fn client(&self, origin: Url) -> Client {
        Client::at(self.http.clone(), origin, self.clock.clone())
    }
    async fn probe(&self, origin: Url) -> Result<ExecutionEnvironmentDescriptor> {
        let timeout = if direct::loopback(origin.host_str().unwrap_or("")) {
            Duration::from_secs(3)
        } else {
            Duration::from_secs(10)
        };
        self.client(origin)
            .call(
                Method::GET,
                &[".well-known", "t3", "environment"],
                None,
                None::<&Value>,
                timeout,
            )
            .await
    }
    pub async fn link(&self, input: Sensitive<PeerLinkCreateInput>) -> Result<PeerLink> {
        if !self.enabled.load(Ordering::Relaxed) {
            return Err(Error::Disabled);
        }
        let input = input.into_inner();
        let _lock = self.mutation.lock().await;
        let urls =
            std::iter::once(&input.url).chain(input.alternate_urls.as_deref().unwrap_or(&[]));
        let mut origins = Vec::new();
        let mut descriptor = None;
        for url in urls {
            let origin = match direct::peer_origin(url.0.as_str()) {
                Ok(v) => v,
                Err(_) if input.expected_environment_id.is_some() => continue,
                Err(e) => return Err(e),
            };
            if let Some(expected) = &input.expected_environment_id {
                if direct::loopback(origin.host_str().unwrap_or("")) {
                    continue;
                }
                let Ok(found) = self.probe(origin.clone()).await else {
                    continue;
                };
                if &found.environment_id != expected {
                    continue;
                }
                if descriptor.is_none() {
                    descriptor = Some(found);
                }
            } else if descriptor.is_none() {
                descriptor = Some(self.probe(origin.clone()).await?);
            }
            if !origins.contains(&origin) {
                origins.push(origin);
            }
        }
        let peer = descriptor.ok_or(Error::Identity)?;
        let primary = origins.first().ok_or(Error::Identity)?.clone();
        if peer.environment_id == self.self_descriptor.environment_id || !limits_hold(&peer) {
            return Err(Error::Identity);
        }
        let previous = self.read(&peer.environment_id.0).await?;
        if input.expected_environment_id.is_none()
            && previous.as_ref().is_some_and(|r| {
                expired(&r.link, self.clock.now_ms()).is_ok_and(|e| !e)
                    && !r
                        .link
                        .urls
                        .iter()
                        .any(|u| u.0.as_str() == primary.origin().ascii_serialization())
            })
        {
            return Err(Error::Identity);
        }
        let issued = self.sign_in(primary, &input).await?;
        let now = self.clock.now_ms();
        let link = PeerLink {
            environment_id: peer.environment_id,
            label: peer.label.0.as_str().to_owned(),
            urls: origins
                .iter()
                .map(|u| crate::text(u.origin().ascii_serialization()))
                .collect(),
            access: input.access,
            linked_at: t3_contracts::wire::WireDateTime(iso(now)?),
            expires_at: t3_contracts::wire::WireDateTime(iso(now
                .checked_add(issued.expires_in.checked_mul(1000).ok_or(Error::Protocol)?)
                .ok_or(Error::Protocol)?)?),
            last_reached_at: Some(t3_contracts::wire::WireDateTime(iso(now)?)),
            last_error: None,
        };
        let secret_name = format!("peer-link-{}", uuid::Uuid::new_v4().simple());
        self.secrets
            .set(&secret_name, issued.access_token.expose().as_bytes())
            .await?;
        let saved = link.clone();
        let name = secret_name.clone();
        let urls = serde_json::to_string(&saved.urls).map_err(|_| Error::Protocol)?;
        let access = serde_json::to_value(saved.access)
            .map_err(|_| Error::Protocol)?
            .as_str()
            .ok_or(Error::Protocol)?
            .to_owned();
        let result=self.db.writer(move |c|{c.execute("INSERT INTO peer_environment_links (environment_id,secret_name,label,urls_json,access,linked_at,expires_at,last_reached_at,last_error,fenced) VALUES (?1,?2,?3,?4,?5,?6,?7,?6,NULL,1) ON CONFLICT(environment_id) DO UPDATE SET secret_name=excluded.secret_name,label=excluded.label,urls_json=excluded.urls_json,access=excluded.access,linked_at=excluded.linked_at,expires_at=excluded.expires_at,last_reached_at=excluded.last_reached_at,last_error=NULL,fenced=1",params![saved.environment_id.0,name,saved.label,urls,access,saved.linked_at.0,saved.expires_at.0])?;Ok(())}).await;
        if result.is_err() {
            self.secrets.remove(&secret_name).await?;
            return Err(Error::Storage);
        }
        if let Some(previous) = previous {
            self.secrets.remove(&previous.secret_name).await?;
        }
        Ok(link)
    }
    async fn sign_in(&self, origin: Url, input: &PeerLinkCreateInput) -> Result<OAuthToken> {
        use t3_contracts::auth::{
            AuthMcpApprovalDecisionRequest, AuthMcpApprovalRedirect, AuthMcpClientRegistration,
            AuthMcpRegisteredClient, AuthMcpTokenRequest, AuthMcpTokenResult, EncodedNumber,
        };
        let client = self.client(origin.clone());
        let timeout = Duration::from_secs(10);
        let registration_body = AuthMcpClientRegistration {
            client_name: Some(format!("T3 Code · {}", self.self_descriptor.label.0)),
            software_id: Some("t3code-peer-link".into()),
            redirect_uris: Some(vec![REDIRECT.into()]),
            token_endpoint_auth_method: None,
        };
        let registration: AuthMcpRegisteredClient = client
            .call(
                Method::POST,
                &["oauth", "mcp", "register"],
                None,
                Some(&registration_body),
                timeout,
            )
            .await?;
        let client_id = registration.client_id;
        if client_id.is_empty() {
            return Err(Error::Protocol);
        }
        let mut bytes = zeroize::Zeroizing::new([0; 32]);
        getrandom::fill(bytes.as_mut()).map_err(|_| Error::Proof)?;
        let verifier = Secret::new(crypto::b64(bytes.as_ref()));
        let challenge = crypto::b64(Sha256::digest(verifier.expose().as_bytes()));
        let state = uuid::Uuid::new_v4().to_string();
        let resource = format!("{}/mcp", origin.origin().ascii_serialization());
        let decision_body: AuthMcpApprovalDecisionRequest = crate::decode(
            json!({"authorization":{"response_type":"code","client_id":client_id,"redirect_uri":REDIRECT,"code_challenge":challenge,"code_challenge_method":"S256","state":state,"resource":resource},"decision":{"_tag":"pairing-code","access":input.access,"code":input.pairing_code}}),
        )?;
        let decision: AuthMcpApprovalRedirect = client
            .call(
                Method::POST,
                &["oauth", "mcp", "decision"],
                None,
                Some(&decision_body),
                timeout,
            )
            .await?;
        let redirect = Url::parse(&decision.redirect_to).map_err(|_| Error::Protocol)?;
        let fields: HashMap<_, _> = redirect.query_pairs().collect();
        let expected = Url::parse(REDIRECT).map_err(|_| Error::Configuration)?;
        if redirect.origin() != expected.origin()
            || redirect.path() != expected.path()
            || fields.get("state").map(|s| s.as_ref()) != Some(&state)
        {
            return Err(Error::Proof);
        }
        let code = fields.get("code").ok_or(Error::Proof)?;
        let token_body: AuthMcpTokenRequest = crate::decode(
            json!({"grant_type":"authorization_code","code":code,"redirect_uri":REDIRECT,"client_id":client_id,"code_verifier":verifier.expose(),"resource":resource}),
        )?;
        let token: AuthMcpTokenResult = client
            .call(
                Method::POST,
                &["oauth", "mcp", "token"],
                None,
                Some(&token_body),
                timeout,
            )
            .await?;
        let expires_in = match token.expires_in {
            EncodedNumber::Finite(n) => n.as_i64().ok_or(Error::Protocol)?,
            EncodedNumber::NonFinite(_) => return Err(Error::Protocol),
        };
        if expires_in <= 0 || token.access_token.is_empty() {
            return Err(Error::Protocol);
        }
        Ok(OAuthToken {
            access_token: Secret::new(token.access_token),
            expires_in,
        })
    }
    pub async fn resolve(&self, id: &str) -> Result<ResolvedPeer> {
        let row = self.read(id).await?.ok_or(Error::NotLinked)?;
        if expired(&row.link, self.clock.now_ms())? {
            return Err(Error::Expired);
        }
        let token = Secret::new(
            read_text(self.secrets.as_ref(), &row.secret_name)
                .await?
                .ok_or(Error::NotLinked)?,
        );
        for candidate in &row.link.urls {
            let origin = direct::peer_origin(candidate.0.as_str())?;
            if let Ok(found) = self.probe(origin.clone()).await
                && found.environment_id == row.link.environment_id
            {
                if !limits_hold(&found) {
                    return Err(Error::Identity);
                }
                let mut link = row.link;
                link.label = found.label.0.as_str().to_owned();
                return Ok(ResolvedPeer {
                    link,
                    origin,
                    token,
                });
            }
        }
        Err(crate::http::unavailable())
    }
    pub async fn list(&self) -> Result<Vec<PeerLinkSummary>> {
        let rows = self
            .db
            .reader(|c| {
                let mut s = c.prepare(
                    "SELECT * FROM peer_environment_links ORDER BY label,environment_id",
                )?;
                Ok(s.query_map([], row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .await
            .map_err(|_| Error::Storage)?;
        let now = self.clock.now_ms();
        futures_util::future::try_join_all(rows.into_iter().map(|row| async move {
            let mut link = row.link;
            let status = if expired(&link, now)? {
                PeerLinkStatus::Expired
            } else {
                let answered = async {
                    let mut probes: FuturesUnordered<_> = link.urls.iter().map(|url| async move {
                        let origin = direct::peer_origin(url.0.as_str()).ok()?;
                        self.client(origin).call::<ExecutionEnvironmentDescriptor>(
                            Method::GET, &[".well-known", "t3", "environment"], None,
                            None::<&serde_json::Value>, Duration::from_secs(3)
                        ).await.ok()
                    }).collect();
                    while let Some(found) = probes.next().await {
                        if let Some(found) = found && found.environment_id == link.environment_id {
                            return Some(found.label.0);
                        }
                    }
                    None
                };
                let label = tokio::time::timeout(Duration::from_secs(3), answered).await.ok().flatten();
                if let Some(label) = label {
                    if link.label != label {
                        let id = link.environment_id.0.clone();
                        let saved = label.clone();
                        let _ = self.db.writer(move |db| {
                            db.execute("UPDATE peer_environment_links SET label=?1 WHERE environment_id=?2", params![saved,id])?;
                            Ok(())
                        }).await;
                        link.label = label;
                    }
                    PeerLinkStatus::Reachable
                } else {
                    PeerLinkStatus::Unreachable
                }
            };
            Ok(PeerLinkSummary {
                environment_id: link.environment_id,
                label: link.label,
                urls: link.urls,
                access: link.access,
                linked_at: link.linked_at,
                expires_at: link.expires_at,
                last_reached_at: link.last_reached_at,
                last_error: link.last_error,
                status,
            })
        })).await
    }
    pub async fn unlink(&self, id: &str) -> Result<bool> {
        let _lock = self.mutation.lock().await;
        let id = id.to_owned();
        let name=self.db.writer(move |c|Ok(c.query_row("DELETE FROM peer_environment_links WHERE environment_id=?1 RETURNING secret_name",[id],|r|r.get::<_,String>(0)).optional()?)).await.map_err(|_|Error::Storage)?;
        if let Some(name) = name {
            self.secrets.remove(&name).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    pub async fn record_outcome(&self, id: &str, success: bool) -> Result<()> {
        let id = id.to_owned();
        let now = iso(self.clock.now_ms())?;
        self.db.writer(move |c|{if success {c.execute("UPDATE peer_environment_links SET last_reached_at=?1,last_error=NULL WHERE environment_id=?2",params![now,id])?;}else{c.execute("UPDATE peer_environment_links SET last_error='The linked environment stopped answering.' WHERE environment_id=?1",[id])?;}Ok(())}).await.map_err(|_|Error::Storage)
    }
}
struct OAuthToken {
    access_token: Secret,
    expires_in: i64,
}

#[derive(Debug, Clone)]
pub struct CallLimits {
    pub runtime_mode: t3_contracts::provider_policy::RuntimeMode,
    pub interaction_mode: t3_contracts::provider_policy::ProviderInteractionMode,
}
pub struct PeerCall<'a> {
    pub environment_id: &'a str,
    pub tool: &'a str,
    pub arguments: Value,
    pub limits: CallLimits,
    pub namespace: &'a str,
    pub sender: Option<t3_contracts::orchestration_v2::OrchestrationV2LinkedSender>,
    pub linked_origin: bool,
}
pub struct PeerMcpClient {
    links: Arc<PeerLinks>,
    sessions: Mutex<HashMap<String, Secret>>,
}
impl PeerMcpClient {
    pub fn new(links: Arc<PeerLinks>) -> Self {
        Self {
            links,
            sessions: Mutex::new(HashMap::new()),
        }
    }
    async fn post(
        &self,
        peer: &ResolvedPeer,
        body: Value,
        limits: &CallLimits,
        session: Option<&Secret>,
        sender: Option<&t3_contracts::orchestration_v2::OrchestrationV2LinkedSender>,
    ) -> Result<Response> {
        let mut url = peer.origin.clone();
        url.set_path("/mcp");
        let runtime = serde_json::to_value(limits.runtime_mode).map_err(|_| Error::Protocol)?;
        let interaction =
            serde_json::to_value(limits.interaction_mode).map_err(|_| Error::Protocol)?;
        let mut headers = BTreeMap::from([
            ("content-type".into(), Secret::new("application/json")),
            (
                "accept".into(),
                Secret::new("application/json, text/event-stream"),
            ),
            (
                "authorization".into(),
                Secret::new(format!("Bearer {}", peer.token.expose())),
            ),
            (
                "t3-mode-limit".into(),
                Secret::new(format!(
                    "{}/{}",
                    runtime.as_str().ok_or(Error::Protocol)?,
                    interaction.as_str().ok_or(Error::Protocol)?
                )),
            ),
        ]);
        if let Some(session) = session {
            headers.insert("mcp-session-id".into(), session.clone());
            headers.insert("mcp-protocol-version".into(), Secret::new("2025-06-18"));
        }
        if let Some(sender) = sender {
            headers.insert(
                "t3-sender-thread".into(),
                Secret::new(sender_header(sender)),
            );
        }
        let timeout = if body["method"] == "tools/call" {
            Duration::from_secs(65)
        } else if direct::loopback(peer.origin.host_str().unwrap_or("")) {
            Duration::from_secs(3)
        } else {
            Duration::from_secs(10)
        };
        self.links
            .http
            .send(Request {
                method: Method::POST,
                url,
                headers,
                body: serde_json::to_vec(&body).map_err(|_| Error::Protocol)?,
                timeout,
            })
            .await
    }
    async fn open(&self, peer: &ResolvedPeer, limits: &CallLimits) -> Result<Secret> {
        let response=self.post(peer,json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t3code-peer","version":"1"}}}),limits,None,None).await?;
        check(&response, self.links.clock.now_ms())?;
        let session = Secret::new(
            response
                .headers
                .get("mcp-session-id")
                .ok_or(Error::Protocol)?
                .clone(),
        );
        let response = self
            .post(
                peer,
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                limits,
                Some(&session),
                None,
            )
            .await?;
        check(&response, self.links.clock.now_ms())?;
        Ok(session)
    }
    /// Caller enforces local tool permissions and prevents a linked caller using another link.
    pub async fn call<T: DeserializeOwned>(&self, call: PeerCall<'_>) -> Result<Sensitive<T>> {
        if call.linked_origin {
            return Err(Error::Identity);
        }
        if !self.links.enabled.load(Ordering::Relaxed)
            && !matches!(
                call.tool,
                "t3_thread_read" | "t3_thread_wait" | "t3_thread_interrupt"
            )
        {
            return Err(Error::Disabled);
        }
        if !call.arguments.is_object() {
            return Err(Error::Protocol);
        }
        if call.tool != "t3_thread_wait" {
            return self.call_once(call).await;
        }
        let mut remaining = call.arguments["timeoutMs"]
            .as_f64()
            .unwrap_or(600_000.0)
            .clamp(1.0, 3_600_000.0) as u64;
        let mut arguments = call.arguments;
        loop {
            let chunk = forwarded_wait_chunk(remaining);
            arguments["timeoutMs"] = json!(chunk);
            let result = self
                .call_once::<t3_contracts::orchestrator_mcp::OrchestratorMcpThreadWaitResult>(
                    PeerCall {
                        environment_id: call.environment_id,
                        tool: call.tool,
                        arguments: arguments.clone(),
                        limits: call.limits.clone(),
                        namespace: call.namespace,
                        sender: call.sender.clone(),
                        linked_origin: false,
                    },
                )
                .await?
                .into_inner();
            remaining -= chunk;
            if !result.timed_out || remaining == 0 {
                return serde_json::from_value(crate::encode(&result)?)
                    .map(Sensitive::new)
                    .map_err(|_| Error::Protocol);
            }
            match result.run_id {
                Some(run) => arguments["runId"] = json!(run),
                None => {
                    arguments
                        .as_object_mut()
                        .ok_or(Error::Protocol)?
                        .remove("runId");
                }
            }
        }
    }
    async fn call_once<T: DeserializeOwned>(&self, call: PeerCall<'_>) -> Result<Sensitive<T>> {
        let PeerCall {
            environment_id: id,
            tool,
            mut arguments,
            limits,
            namespace,
            sender,
            linked_origin: _,
        } = call;
        if let Some(args) = arguments.as_object_mut() {
            args.remove("environmentId");
            if args
                .get("attachments")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
            {
                return Err(Error::Configuration);
            }
            if let Some(key) = args.get("clientRequestId").and_then(Value::as_str) {
                args.insert(
                    "clientRequestId".into(),
                    json!(scoped_request_id(namespace, key)),
                );
            }
        } else {
            return Err(Error::Protocol);
        }
        let peer = self.links.resolve(id).await?;
        let key = format!("{id} {}", peer.origin);
        let existing = self.sessions.lock().await.get(&key).cloned();
        let mut session = match existing {
            Some(s) => s,
            None => self.open(&peer, &limits).await?,
        };
        self.sessions
            .lock()
            .await
            .insert(key.clone(), session.clone());
        let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":arguments}});
        let mut response = self
            .post(
                &peer,
                body.clone(),
                &limits,
                Some(&session),
                sender.as_ref(),
            )
            .await?;
        if response.status == 404 {
            session = self.open(&peer, &limits).await?;
            self.sessions.lock().await.insert(key, session.clone());
            response = self
                .post(&peer, body, &limits, Some(&session), sender.as_ref())
                .await?;
        }
        if let Err(e) = check(&response, self.links.clock.now_ms()) {
            self.links.record_outcome(id, false).await?;
            return Err(e);
        }
        self.links.record_outcome(id, true).await?;
        let value = parse_mcp_response(&response)?;
        serde_json::from_value(value)
            .map(Sensitive::new)
            .map_err(|_| Error::Protocol)
    }
}
pub fn parse_mcp_response(response: &Response) -> Result<Value> {
    let text = String::from_utf8_lossy(&response.body);
    let json = if response
        .headers
        .get("content-type")
        .is_some_and(|s| s.contains("text/event-stream"))
    {
        text.lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .map(str::trim)
            .collect::<String>()
    } else {
        text.into_owned()
    };
    let body: Value = serde_json::from_str(&json).map_err(|_| Error::Protocol)?;
    if body.get("error").is_some() {
        return Err(Error::PeerFailure {
            code: t3_contracts::orchestrator_mcp::OrchestratorMcpFailureCode::InvalidRequest,
        });
    }
    let content = body["result"]["content"]
        .as_array()
        .ok_or(Error::Protocol)?;
    let text = content
        .iter()
        .filter_map(|v| v["text"].as_str())
        .collect::<String>();
    if body["result"]["isError"] == true {
        let code = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|failure| serde_json::from_value(failure["code"].clone()).ok())
            .unwrap_or(
                t3_contracts::orchestrator_mcp::OrchestratorMcpFailureCode::OrchestrationError,
            );
        return Err(Error::PeerFailure { code });
    }
    serde_json::from_str(&text).map_err(|_| Error::Protocol)
}
pub fn scoped_request_id(namespace: &str, key: &str) -> String {
    let hash = Sha256::digest(format!("{namespace}\n{key}"));
    format!(
        "fwd:{}",
        hash.iter().map(|v| format!("{v:02x}")).collect::<String>()
    )
}
pub fn forwarded_wait_chunk(remaining_ms: u64) -> u64 {
    remaining_ms.min(50_000)
}
pub fn sender_header(
    sender: &t3_contracts::orchestration_v2::OrchestrationV2LinkedSender,
) -> String {
    const SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'!')
        .remove(b'~')
        .remove(b'*')
        .remove(b'\'')
        .remove(b'(')
        .remove(b')');
    let title = sender.title.chars().take(200).collect::<String>();
    [&sender.environment_id.0, &sender.thread_id.0, &title]
        .into_iter()
        .map(|s| percent_encoding::utf8_percent_encode(s, SET).to_string())
        .collect::<Vec<_>>()
        .join("/")
}
