use crate::{Capability, InvocationScope, McpError, Result, ThreadCaller};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap},
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use t3_contracts::{
    base_schemas::{EnvironmentId, ThreadId},
    provider_instance::ProviderInstanceId,
};

#[derive(Clone, Copy, Debug)]
pub struct AliveMark {
    pub wall_ms: i64,
    pub monotonic_ms: i64,
}

pub trait Clock: Send + Sync {
    fn now(&self) -> AliveMark;
}
pub struct SystemClock {
    origin: Instant,
}
impl Default for SystemClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}
impl Clock for SystemClock {
    fn now(&self) -> AliveMark {
        AliveMark {
            wall_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64),
            monotonic_ms: self.origin.elapsed().as_millis() as i64,
        }
    }
}

pub struct CredentialRequest {
    pub thread_id: ThreadId,
    pub provider_instance_id: ProviderInstanceId,
    pub browser_tools_available: bool,
    pub capabilities: Option<BTreeSet<Capability>>,
    pub subagent: bool,
}

impl CredentialRequest {
    pub fn new(thread_id: ThreadId, provider_instance_id: ProviderInstanceId) -> Self {
        Self {
            thread_id,
            provider_instance_id,
            browser_tools_available: true,
            capabilities: None,
            subagent: false,
        }
    }
}

#[derive(Clone)]
pub struct ProviderSessionConfig {
    pub scope: InvocationScope,
    pub endpoint: String,
    pub authorization_header: String,
    pub browser_tools_available: bool,
}

impl ProviderSessionConfig {
    pub fn acp_environment(&self) -> std::collections::BTreeMap<String, String> {
        std::collections::BTreeMap::from([
            ("T3_ACP_MCP_ENDPOINT".into(), self.endpoint.clone()),
            (
                "T3_ACP_MCP_AUTHORIZATION".into(),
                self.authorization_header.clone(),
            ),
        ])
    }
    pub fn http_server(&self, deferred: bool) -> serde_json::Value {
        let mut headers = serde_json::json!({"Authorization":self.authorization_header});
        if deferred {
            headers["t3-tool-exposure"] = "deferred".into();
        }
        serde_json::json!({"type":"http","url":self.endpoint,"headers":headers})
    }
}

impl std::fmt::Debug for ProviderSessionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderSessionConfig")
            .field("scope", &self.scope)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

struct CredentialRecord {
    scope: InvocationScope,
    last_alive: AliveMark,
    expired: bool,
}

pub struct SessionRegistry {
    environment_id: EnvironmentId,
    endpoint: String,
    liveness_window_ms: i64,
    clock: Arc<dyn Clock>,
    records: Mutex<HashMap<[u8; 32], CredentialRecord>>,
}

pub fn endpoint(address: Option<SocketAddr>) -> String {
    match address {
        Some(address) => {
            let host = if address.ip().is_unspecified() {
                IpAddr::from([127, 0, 0, 1])
            } else {
                address.ip()
            };
            format!("http://{}/mcp", SocketAddr::new(host, address.port()))
        }
        None => "http://127.0.0.1/mcp".into(),
    }
}

impl SessionRegistry {
    pub fn new(environment_id: EnvironmentId, address: Option<SocketAddr>) -> Self {
        Self::with_clock(
            environment_id,
            address,
            86_400_000,
            Arc::new(SystemClock::default()),
        )
    }
    pub fn with_clock(
        environment_id: EnvironmentId,
        address: Option<SocketAddr>,
        liveness_window_ms: i64,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            environment_id,
            endpoint: endpoint(address),
            liveness_window_ms,
            clock,
            records: Mutex::new(HashMap::new()),
        }
    }
    pub fn issue(&self, request: CredentialRequest) -> Result<ProviderSessionConfig> {
        let now = self.clock.now();
        let mut secret = [0_u8; 32];
        getrandom::fill(&mut secret).map_err(|_| McpError::Internal)?;
        let raw = URL_SAFE_NO_PAD.encode(secret);
        let namespace = uuid::Uuid::new_v4().to_string();
        let mut capabilities = BTreeSet::from([
            Capability::Orchestration,
            Capability::Worktree,
            Capability::PullRequests,
        ]);
        capabilities.extend(request.capabilities.unwrap_or_else(|| {
            if request.browser_tools_available {
                BTreeSet::from([Capability::Preview])
            } else {
                BTreeSet::new()
            }
        }));
        let scope = InvocationScope {
            environment_id: self.environment_id.clone(),
            request_namespace: namespace.clone(),
            issued_at: now.wall_ms,
            thread: Some(ThreadCaller {
                thread_id: request.thread_id,
                provider_instance_id: request.provider_instance_id,
                provider_session_id: namespace,
                subagent: request.subagent,
            }),
            client: None,
            capabilities,
            linked_environments: None,
        };
        let browser_tools_available = scope.capabilities.contains(&Capability::Preview);
        self.records.lock().map_err(|_| McpError::Internal)?.insert(
            Sha256::digest(raw.as_bytes()).into(),
            CredentialRecord {
                scope: scope.clone(),
                last_alive: now,
                expired: false,
            },
        );
        Ok(ProviderSessionConfig {
            scope,
            endpoint: self.endpoint.clone(),
            authorization_header: format!("Bearer {raw}"),
            browser_tools_available,
        })
    }
    pub fn resolve(&self, raw: &str) -> Option<InvocationScope> {
        if raw.is_empty() {
            return None;
        }
        let now = self.clock.now();
        let hash: [u8; 32] = Sha256::digest(raw.as_bytes()).into();
        let mut records = self.records.lock().ok()?;
        let record = records.get_mut(&hash)?;
        if record.expired {
            return None;
        }
        if (now.wall_ms - record.last_alive.wall_ms)
            .max(now.monotonic_ms - record.last_alive.monotonic_ms)
            > self.liveness_window_ms
        {
            record.expired = true;
            return None;
        }
        record.last_alive = now;
        Some(record.scope.clone())
    }
    pub fn touch(&self, thread_id: &ThreadId) -> Result<()> {
        let now = self.clock.now();
        for record in self
            .records
            .lock()
            .map_err(|_| McpError::Internal)?
            .values_mut()
        {
            if record
                .scope
                .thread
                .as_ref()
                .is_some_and(|caller| &caller.thread_id == thread_id)
            {
                record.last_alive = now;
                record.expired = false;
            }
        }
        Ok(())
    }
    pub fn revoke_provider_session(&self, id: &str) -> Result<()> {
        self.revoke(|scope| {
            scope
                .thread
                .as_ref()
                .is_some_and(|t| t.provider_session_id == id)
        })
    }
    pub fn revoke_thread(&self, id: &ThreadId) -> Result<()> {
        self.revoke(|scope| scope.thread.as_ref().is_some_and(|t| &t.thread_id == id))
    }
    pub fn revoke_all(&self) -> Result<()> {
        self.revoke(|_| true)
    }
    fn revoke(&self, predicate: impl Fn(&InvocationScope) -> bool) -> Result<()> {
        self.records
            .lock()
            .map_err(|_| McpError::Internal)?
            .retain(|_, record| !predicate(&record.scope));
        Ok(())
    }
}
