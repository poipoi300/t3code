#![cfg(feature = "providers")]
#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::path::Path;
use t3_rpc::{CallEvent, CallKind, Client, Exit, Frame};
use t3_server::adapters::config::{ConfigInputs, ServerFlags};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

struct Server {
    temp: tempfile::TempDir,
    effects: Arc<Effects>,
    runtime: t3_server::runtime::RunningServer,
    origin: String,
    cookie: String,
    socket: Socket,
    client: Client,
    chunks: std::collections::VecDeque<Value>,
}

impl Server {
    async fn start() -> Self {
        Self::start_with(|_| Vec::new()).await
    }
    /// `extra` adds environment entries, given the home before the server starts.
    async fn start_with(extra: impl FnOnce(&Path) -> Vec<(String, String)>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        std::fs::create_dir_all(home.join("userdata")).unwrap();
        std::fs::write(home.join("userdata/settings.json"),json!({
            "enableProviderUpdateChecks":false,
            "providerInstances":{"codex":{"driver":"codex","environment":[{"name":"PRIVATE_ENV","value":"private-environment-value","sensitive":true}],"config":{"setupMode":"managed","homePath":home.join("private-codex")}}},
            "textGenerators":{"fixture":{"kind":"openaiCompatible","baseUrl":"https://trusted.invalid/v1/","apiKey":"private-generator-key"}}
        }).to_string()).unwrap();
        // The terminal's shell: without PATH it resolves ComSpec (cmd.exe), so no user shell
        // profile from the real account can load. Nothing else from this process is passed.
        let env = ["SystemRoot", "ComSpec", "WINDIR"]
            .into_iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_owned(), v)))
            .chain(std::iter::once((
                "T3CODE_DEV_MANAGED_POLICY_PATH".into(),
                home.join("no-policy.json").to_string_lossy().into_owned(),
            )))
            .chain(extra(home))
            .collect();
        let effects = Arc::new(Effects::default());
        let runtime = t3_server::runtime::start_with(
            ConfigInputs {
                flags: ServerFlags {
                    base_dir: Some(home.to_string_lossy().into_owned()),
                    host: Some("127.0.0.1".into()),
                    port: Some(0),
                    ..Default::default()
                },
                env,
                home: home.into(),
                process_cwd: home.into(),
                bootstrap: None,
                headless: true,
                force_auto_bootstrap: Some(false),
                service_managed: false,
                cli_log_level: None,
                automatic_port: None,
                persisted_observability: Default::default(),
                static_dir: None,
            },
            t3_server::runtime::StartOptions {
                provider_effects: Some(effects_for(effects.clone())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let origin = format!("http://{}", runtime.address);
        let http = reqwest::Client::new();
        let paired = http
            .post(format!("{origin}/api/auth/browser-session"))
            .json(&json!({"credential": runtime.pairing_credential}))
            .send()
            .await
            .unwrap();
        let cookie = paired.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let ticket: Value = http
            .post(format!("{origin}/api/auth/websocket-ticket/rpc"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let (socket, _) = tokio_tungstenite::connect_async(format!(
            "ws://{}/ws?orchestrationProtocol=2&wsTicket={}",
            runtime.address,
            ticket["ticket"].as_str().unwrap()
        ))
        .await
        .unwrap();
        Self {
            temp,
            effects,
            runtime,
            origin,
            cookie,
            socket,
            client: Client::new(0, Default::default()),
            chunks: Default::default(),
        }
    }
    fn home(&self) -> &Path {
        self.temp.path()
    }
    /// Replaces this connection with one whose session holds only `scopes`.
    async fn reconnect_with(&mut self, scopes: &[&str]) {
        let http = reqwest::Client::new();
        let grant: Value = http
            .post(format!("{}/api/auth/pairing-token", self.origin))
            .header("cookie", &self.cookie)
            .json(&json!({ "scopes": scopes }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let paired = http
            .post(format!("{}/api/auth/browser-session", self.origin))
            .json(&json!({"credential": grant["credential"]}))
            .send()
            .await
            .unwrap();
        self.cookie = paired.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        self.reopen(self.cookie.clone()).await;
    }
    async fn reopen(&mut self, cookie: String) {
        self.cookie = cookie;
        let http = reqwest::Client::new();
        let ticket: Value = http
            .post(format!("{}/api/auth/websocket-ticket/rpc", self.origin))
            .header("cookie", &self.cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let (socket, _) = tokio_tungstenite::connect_async(format!(
            "ws://{}/ws?orchestrationProtocol=2&wsTicket={}",
            self.runtime.address,
            ticket["ticket"].as_str().unwrap()
        ))
        .await
        .unwrap();
        self.socket = socket;
        self.client = Client::new(0, Default::default());
        self.chunks.clear();
    }
    /// A unary call's exit, acknowledging chunks of other open streams meanwhile.
    async fn call(&mut self, tag: &str, payload: Value) -> Exit {
        let id = self.client.start(tag, payload, CallKind::Unary).unwrap();
        loop {
            match self.event().await {
                CallEvent::Exit { id: done, exit } if done == id => return exit,
                CallEvent::Chunk { id, values } => {
                    self.client.acknowledge(id);
                    self.chunks.extend(values);
                }
                _ => {}
            }
        }
    }
    async fn flush(&mut self) {
        while let Some(message) = self.client.pop_outgoing() {
            let Frame::Text(text) = t3_rpc::codec::encode(&message).unwrap() else {
                panic!("text frame expected");
            };
            self.socket.send(Message::Text(text.into())).await.unwrap();
        }
    }
    async fn event(&mut self) -> CallEvent {
        loop {
            self.flush().await;
            if let Some(event) = self.client.pop_event() {
                return event;
            }
            let frame = match self.socket.next().await.unwrap().unwrap() {
                Message::Text(text) => Frame::Text(text.to_string()),
                Message::Binary(bytes) => Frame::Binary(bytes.to_vec()),
                Message::Ping(payload) => {
                    self.socket.send(Message::Pong(payload)).await.unwrap();
                    continue;
                }
                other => panic!("connection ended: {other:?}"),
            };
            for message in t3_rpc::codec::decode(&frame).unwrap() {
                self.client.receive(message, 0);
            }
        }
    }
    async fn unary(&mut self, tag: &str, payload: Value) -> Exit {
        self.client.start(tag, payload, CallKind::Unary).unwrap();
        match self.event().await {
            CallEvent::Exit { exit, .. } => exit,
            other => panic!("unexpected {other:?}"),
        }
    }
    /// The next chunk of any stream, acknowledged as a real client does.
    async fn chunk(&mut self) -> Value {
        if let Some(value) = self.chunks.pop_front() {
            return value;
        }
        match self.event().await {
            CallEvent::Chunk { id, values } => {
                self.client.acknowledge(id);
                values.into_iter().next().unwrap()
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

fn ok(exit: Exit) -> Value {
    match exit {
        Exit::Success { value } => value.unwrap_or(Value::Null),
        other => panic!(
            "expected success, got {}",
            serde_json::to_value(other).unwrap()
        ),
    }
}
/// The first failure's tag (`Fail` error or `Die` defect).
fn failure(exit: Exit) -> String {
    let encoded = serde_json::to_value(&exit).unwrap();
    let cause = &encoded["cause"][0];
    let value = if cause["error"].is_null() {
        &cause["defect"]
    } else {
        &cause["error"]
    };
    assert!(!value.is_null(), "expected a failure");
    value["_tag"].as_str().unwrap_or("").to_owned()
}

use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use t3_provider::{
    codex_installation::*,
    discovery::{Driver, VersionProbe},
    native_probe::*,
    native_provider::ExecutableProbe,
    sensitive::*,
    transport::*,
};
struct Effects {
    registry_failure: AtomicBool,
    updated: AtomicBool,
    now: AtomicI64,
}
impl Default for Effects {
    fn default() -> Self {
        Self {
            registry_failure: AtomicBool::new(false),
            updated: AtomicBool::new(false),
            now: AtomicI64::new(1_800_000_000_000),
        }
    }
}
impl ProviderClock for Effects {
    fn now_ms(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }
}
impl Effects {
    fn version(&self) -> &'static str {
        if self.updated.load(Ordering::SeqCst) {
            "0.166.0"
        } else {
            "0.165.0"
        }
    }
}
#[async_trait]
impl HttpTransport for Effects {
    async fn send(&self, request: HttpRequest) -> ServiceResult<HttpResponse> {
        assert!(!format!("{request:?}").contains("private-generator-key"));
        let body = match request.url.as_str() {
            "https://trusted.invalid/v1/models" => json!({"data":[{"id":"fixture-model"}]}),
            "https://auth.openai.com/.well-known/openid-configuration" => {
                json!({"issuer":"https://auth.openai.com","authorization_endpoint":"https://auth.openai.com/api/accounts/authorize","token_endpoint":"https://auth.openai.com/oauth/token","jwks_uri":"https://auth.openai.com/.well-known/jwks.json","revocation_endpoint":"https://auth.openai.com/oauth/revoke"})
            }
            "https://auth.openai.com/.well-known/jwks.json" => json!({"keys":[fixture_jwk()]}),
            "https://releases.openai.com/codex/channels/latest" => {
                json!({"tag_name":"rust-v0.166.0"})
            }
            _ => return Err(ServiceError::Network),
        };
        Ok(HttpResponse {
            status: 200,
            body: Sensitive::new(body),
        })
    }
}
#[async_trait]
impl NativeProbe for Effects {
    async fn codex(&self, _: ProbeLaunch) -> ServiceResult<Sensitive<Value>> {
        Ok(Sensitive::new(
            json!({"initialize":{"userAgent":format!("codex/{}",self.version())},"account":{"account":{"type":"apiKey"},"requiresOpenaiAuth":false},"models":[],"config":{"config":{}},"nativeUsage":false}),
        ))
    }
    async fn claude(&self, _: ProbeLaunch) -> ServiceResult<Sensitive<Value>> {
        Err(ServiceError::Unavailable)
    }
}
#[async_trait]
impl ExecutableProbe for Effects {
    async fn version(
        &self,
        _: Driver,
        binary: &str,
        _: &t3_provider::environment::Environment,
        _: &Path,
    ) -> ServiceResult<VersionProbe> {
        if !binary.ends_with("fixture-codex.exe") {
            return Err(ServiceError::Unavailable);
        }
        Ok(VersionProbe {
            installed: true,
            version: Some(self.version().into()),
            status: t3_contracts::server::ServerProviderState::Ready,
            message: None,
            resolved_executable: Some(binary.into()),
        })
    }
}
#[async_trait]
impl ReleaseTransport for Effects {
    async fn latest(&self, _: &str) -> ServiceResult<Option<ReleaseAsset>> {
        Ok(None)
    }
    async fn download(
        &self,
        _: &ReleaseAsset,
        _: &Path,
        _: tokio::sync::watch::Sender<u64>,
    ) -> ServiceResult<()> {
        std::future::pending().await
    }
}
#[async_trait]
impl CodexExecutableValidator for Effects {
    async fn validate(
        &self,
        _: &Path,
        _: &str,
        _: &t3_provider::environment::Environment,
    ) -> ServiceResult<()> {
        Err(ServiceError::Response)
    }
}
fn effects_for(effects: Arc<Effects>) -> Arc<t3_server::adapters::providers::ProviderEffects> {
    Arc::new(t3_server::adapters::providers::ProviderEffects {
        http: effects.clone(),
        clock: effects.clone(),
        protocols: effects.clone(),
        executables: effects.clone(),
        codex: t3_provider::domain::CodexInstallEffects {
            transport: effects.clone(),
            validator: effects.clone(),
            local: Arc::new(NoLocalCodex),
        },
        maintenance_probe: Some(effects.clone()),
        maintenance_executor: Some(effects.clone()),
        registry_fetch: Some(effects.clone()),
        registry_commands: Some(effects.clone()),
        registry_downloads: Some(effects),
    })
}

#[tokio::test]
async fn configuration_contains_real_provider_rows_and_tracks_settings_updates() {
    let mut server = Server::start().await;
    let config = ok(server.call("server.getConfig", json!({})).await);
    assert!(!config["providers"].as_array().unwrap().is_empty());
    assert!(!config.to_string().contains("private-environment-value"));
    assert!(!config.to_string().contains("private-generator-key"));
    server
        .client
        .start("subscribeServerConfig", json!({}), CallKind::Stream)
        .unwrap();
    assert_eq!(server.chunk().await["type"], "snapshot");
    ok(server.call("server.updateSettings", json!({"patch":{"providerInstances":{"codex":{"driver":"codex","displayName":"Renamed","config":{"setupMode":"managed","homePath":server.home().join("private-codex")}}}}})).await);
    loop {
        let event = server.chunk().await;
        if event["type"] == "providerStatuses"
            && event["payload"]["providers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|provider| provider["displayName"] == "Renamed")
        {
            break;
        }
    }
    server.socket.close(None).await.unwrap();
    server.runtime.shutdown().await.unwrap();
}

#[async_trait]
impl t3_provider::registry::RegistryFetch for Effects {
    async fn bytes(&self, _: &str) -> ServiceResult<Vec<u8>> {
        if self.registry_failure.load(Ordering::SeqCst) {
            return Err(ServiceError::Network);
        }
        Ok(json!({"version":"1","agents":[{"id":"fixture","name":"Fixture","version":"1.0.0","description":"Fixture agent","distribution":{"binary":{
            "windows-x86_64":{"archive":"https://fixture.invalid/agent.bin","cmd":"agent.exe","env":{"TOKEN":"private-registry-value"}},
            "linux-x86_64":{"archive":"https://fixture.invalid/agent.bin","cmd":"agent","env":{"TOKEN":"private-registry-value"}},
            "darwin-aarch64":{"archive":"https://fixture.invalid/agent.bin","cmd":"agent","env":{"TOKEN":"private-registry-value"}}
        }}}]}).to_string().into_bytes())
    }
}
#[async_trait]
impl t3_provider::registry_installation::RegistryCommands for Effects {
    fn resolve(
        &self,
        _: &str,
        _: &t3_provider::environment::Environment,
    ) -> Option<std::path::PathBuf> {
        None
    }
    async fn run(
        &self,
        _: &Path,
        _: &[String],
        _: &t3_provider::environment::Environment,
        _: std::time::Duration,
        _: usize,
    ) -> t3_provider::acp_service::AcpResult<Sensitive<String>> {
        panic!("fixture registry binary needs no process")
    }
}
#[async_trait]
impl t3_provider::registry_installation::RegistryDownloads for Effects {
    async fn download(&self, url: &str, path: &Path) -> t3_provider::acp_service::AcpResult<()> {
        assert_eq!(url, "https://fixture.invalid/agent.bin");
        std::fs::write(path, b"fixture binary").unwrap();
        Ok(())
    }
}
#[async_trait]
impl t3_provider::maintenance_resolution::MaintenanceProbe for Effects {
    fn resolve(&self, command: &str, _: &t3_provider::environment::Environment) -> Option<String> {
        command
            .ends_with("fixture-codex.exe")
            .then(|| command.into())
    }
    fn canonical(&self, path: &str) -> Option<String> {
        Some(path.into())
    }
    async fn run(
        &self,
        _: &str,
        _: &[String],
        _: &t3_provider::environment::Environment,
        _: &[i32],
    ) -> Option<String> {
        None
    }
}
#[async_trait]
impl t3_provider::service::MaintenanceExecutor for Effects {
    async fn execute(
        &self,
        action: t3_provider::maintenance::MaintenanceAction,
    ) -> ServiceResult<()> {
        assert!(action.executable.ends_with("fixture-codex.exe"));
        assert_eq!(action.args, ["update"]);
        assert!(!format!("{action:?}").contains("private"));
        self.updated.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn provider_status_and_model_families_have_success_typed_failures_and_scope_refusals() {
    let mut server = Server::start().await;
    for tag in [
        "provider.auth.subscribe",
        "provider.install.subscribe",
        "server.searchAcpRegistry",
        "server.listTextGeneratorModels",
    ] {
        assert!(!server.runtime.unserved_methods.contains(&tag), "{tag}");
    }
    let models = ok(server
        .call("server.listTextGeneratorModels", json!({"id":"fixture"}))
        .await);
    assert_eq!(models["models"][0]["id"], "fixture-model");
    assert!(!models.to_string().contains("private-generator-key"));
    assert_eq!(
        failure(
            server
                .call("server.listTextGeneratorModels", json!({"id":"missing"}))
                .await
        ),
        "TextGeneratorError"
    );
    assert_eq!(failure(server.call("server.listTextGeneratorModels",json!({"id":"fixture","config":{"kind":"openaiCompatible","baseUrl":"http://169.254.169.254/private","apiKey":"private-generator-key"}})).await),"TextGeneratorError");
    server
        .effects
        .registry_failure
        .store(true, Ordering::SeqCst);
    assert_eq!(
        failure(
            server
                .call("server.searchAcpRegistry", json!({"query":""}))
                .await
        ),
        "AcpRegistryOperationError"
    );
    server
        .effects
        .registry_failure
        .store(false, Ordering::SeqCst);
    let registry = ok(server
        .call("server.searchAcpRegistry", json!({"query":""}))
        .await);
    assert_eq!(registry["agents"][0]["id"], "fixture");
    assert!(!registry.to_string().contains("private-registry-value"));
    server
        .client
        .start(
            "provider.auth.subscribe",
            json!({"instanceId":"codex"}),
            CallKind::Stream,
        )
        .unwrap();
    assert_eq!(server.chunk().await["phase"], "idle");
    server
        .client
        .start(
            "provider.install.subscribe",
            json!({"instanceId":"codex"}),
            CallKind::Stream,
        )
        .unwrap();
    assert_eq!(server.chunk().await["phase"], "idle");
    for tag in ["provider.auth.subscribe", "provider.install.subscribe"] {
        server
            .client
            .start(tag, json!({"instanceId":"missing"}), CallKind::Stream)
            .unwrap();
        loop {
            match server.event().await {
                CallEvent::Exit { exit, .. } => {
                    assert_eq!(failure(exit), "ProviderSetupError");
                    break;
                }
                CallEvent::Chunk { id, .. } => server.client.acknowledge(id),
                _ => {}
            }
        }
    }
    server.reconnect_with(&["terminal:read"]).await;
    for (tag, payload, kind) in [
        (
            "server.searchAcpRegistry",
            json!({"query":""}),
            CallKind::Unary,
        ),
        (
            "server.listTextGeneratorModels",
            json!({"id":"fixture"}),
            CallKind::Unary,
        ),
        (
            "provider.auth.subscribe",
            json!({"instanceId":"codex"}),
            CallKind::Stream,
        ),
        (
            "provider.install.subscribe",
            json!({"instanceId":"codex"}),
            CallKind::Stream,
        ),
    ] {
        server.client.start(tag, payload, kind).unwrap();
        match server.event().await {
            CallEvent::Exit { exit, .. } => {
                assert_eq!(failure(exit), "EnvironmentAuthorizationError", "{tag}")
            }
            event => panic!("unexpected {event:?}"),
        }
    }
    server.socket.close(None).await.unwrap();
    server.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn managed_auth_has_private_session_owned_updates_and_rejects_unbound_callbacks() {
    let mut server = Server::start().await;
    server
        .client
        .start(
            "provider.auth.subscribe",
            json!({"instanceId":"codex"}),
            CallKind::Stream,
        )
        .unwrap();
    assert_eq!(server.chunk().await["phase"], "idle");
    assert_eq!(
        failure(
            server
                .call(
                    "provider.auth.start",
                    json!({"instanceId":"codex","returnUrl":"https://untrusted.invalid/settings"})
                )
                .await
        ),
        "ProviderSetupError"
    );
    let started = ok(server
        .call(
            "provider.auth.start",
            json!({"instanceId":"codex","callbackMode":"client"}),
        )
        .await);
    let flow = started["flowId"].as_str().unwrap().to_owned();
    let waiting = loop {
        let event = server.chunk().await;
        if event["phase"] == "waiting" {
            break event;
        }
        assert_ne!(event["phase"], "failed", "{event}");
    };
    assert!(
        waiting["authorizationUrl"]
            .as_str()
            .unwrap()
            .starts_with("https://auth.openai.com/")
    );
    assert!(!waiting.to_string().contains("private-generator-key"));
    assert_eq!(failure(server.call("provider.auth.complete",json!({"instanceId":"codex","flowId":flow,"callbackUrl":"https://untrusted.invalid/auth/callback?code=private-code&state=bad"})).await),"ProviderSetupError");
    assert_eq!(failure(server.call("provider.auth.respond",json!({"instanceId":"codex","flowId":flow,"interactionId":"wrong","response":{"type":"browser","action":"accept"}})).await),"ProviderSetupError");
    let original_cookie = server.cookie.clone();
    server
        .reconnect_with(&["providers:manage", "orchestration:read"])
        .await;
    server
        .client
        .start(
            "provider.auth.subscribe",
            json!({"instanceId":"codex"}),
            CallKind::Stream,
        )
        .unwrap();
    let other = server.chunk().await;
    assert!(other["authorizationUrl"].is_null());
    assert!(other["flowId"].is_null());
    assert_eq!(
        failure(
            server
                .call(
                    "provider.auth.cancel",
                    json!({"instanceId":"codex","flowId":flow})
                )
                .await
        ),
        "ProviderSetupError"
    );
    server.reopen(original_cookie).await;
    server
        .client
        .start(
            "provider.auth.subscribe",
            json!({"instanceId":"codex"}),
            CallKind::Stream,
        )
        .unwrap();
    assert_eq!(server.chunk().await["phase"], "waiting");
    let canceled = ok(server
        .call(
            "provider.auth.cancel",
            json!({"instanceId":"codex","flowId":flow}),
        )
        .await);
    assert_eq!(canceled["phase"], "cancelled");
    loop {
        let event = server.chunk().await;
        if event["phase"] == "cancelled" {
            break;
        }
    }
    assert_eq!(
        ok(server
            .call("provider.auth.logout", json!({"instanceId":"codex"}))
            .await)["phase"],
        "idle"
    );
    assert_eq!(
        failure(
            server
                .call(
                    "provider.auth.removeMethod",
                    json!({"instanceId":"codex","methodId":"not-an-advertised-method"})
                )
                .await
        ),
        "ProviderSetupError"
    );
    let failed_import = server
        .call(
            "provider.chatgpt.import-profile",
            json!({"instanceId":"codex","profile":transferred_profile()}),
        )
        .await;
    assert!(
        !serde_json::to_string(&failed_import)
            .unwrap()
            .contains("private-transfer")
    );
    assert_eq!(failure(failed_import), "ProviderSetupError");
    server.socket.close(None).await.unwrap();
    server.runtime.shutdown().await.unwrap();
}
#[tokio::test]
async fn installation_stream_tracks_owned_operation_cancel_and_remove_without_a_real_installer() {
    let mut server = Server::start().await;
    server
        .client
        .start(
            "provider.install.subscribe",
            json!({"instanceId":"codex"}),
            CallKind::Stream,
        )
        .unwrap();
    assert_eq!(server.chunk().await["phase"], "idle");
    let installing = ok(server
        .call("provider.install.start", json!({"instanceId":"codex"}))
        .await);
    let operation = installing["operationId"].as_str().unwrap().to_owned();
    assert_eq!(
        failure(
            server
                .call(
                    "provider.install.cancel",
                    json!({"instanceId":"codex","operationId":"forged"})
                )
                .await
        ),
        "ProviderSetupError"
    );
    assert_eq!(
        ok(server
            .call(
                "provider.install.cancel",
                json!({"instanceId":"codex","operationId":operation})
            )
            .await)["phase"],
        "cancelled"
    );
    loop {
        let event = server.chunk().await;
        if event["phase"] == "cancelled" {
            break;
        }
    }
    let removed = ok(server
        .call("provider.install.remove", json!({"instanceId":"codex"}))
        .await);
    assert!(removed["installedVersion"].is_null());
    assert_eq!(
        failure(
            server
                .call("provider.install.start", json!({"instanceId":"unknown"}))
                .await
        ),
        "ProviderSetupError"
    );
    server.socket.close(None).await.unwrap();
    server.runtime.shutdown().await.unwrap();
}
#[tokio::test]
async fn registry_preparation_uses_known_metadata_and_managed_paths_and_releases_expired_reservations()
 {
    let mut server = Server::start().await;
    let result = ok(server
        .call(
            "server.prepareAcpRegistryAgent",
            json!({"agentId":"fixture"}),
        )
        .await);
    assert!(!result.to_string().contains("private-registry-value"));
    let target = t3_provider::registry::platform_target(
        t3_provider::environment::Platform::current(),
        if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x64"
        },
    )
    .unwrap();
    let installed = server
        .home()
        .join("tools/acp-registry/fixture/1.0.0")
        .join(target);
    assert!(installed.is_dir());
    assert_eq!(
        failure(
            server
                .call(
                    "server.prepareAcpRegistryAgent",
                    json!({"agentId":"../private-registry-value"})
                )
                .await
        ),
        "AcpRegistryOperationError"
    );
    server
        .effects
        .now
        .fetch_add(20 * 60 * 1000, Ordering::SeqCst);
    assert_eq!(
        ok(server
            .call(
                "server.uninstallAcpRegistryManagedBinary",
                json!({"agentId":"fixture"})
            )
            .await)["removed"],
        true
    );
    assert!(!installed.exists());
    server.socket.close(None).await.unwrap();
    server.runtime.shutdown().await.unwrap();
}
#[tokio::test]
async fn provider_update_uses_the_configured_binary_and_verifies_the_fake_process_result() {
    let mut server=Server::start_with(|home|{
        let binary=home.join("native-home/packages/standalone/test/fixture-codex.exe");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();std::fs::write(&binary,b"fixture").unwrap();
        std::fs::write(home.join("userdata/settings.json"),json!({"enableProviderUpdateChecks":false,"providerInstances":{
            "codex":{"driver":"codex","config":{"setupMode":"managed","homePath":home.join("private-codex")}},
            "native":{"driver":"codex","config":{"setupMode":"existing","binaryPath":binary,"homePath":home.join("native-home")}}
        }}).to_string()).unwrap();Vec::new()
    }).await;
    assert_eq!(
        failure(
            server
                .call(
                    "server.updateProvider",
                    json!({"provider":"claudeAgent","instanceId":"native"})
                )
                .await
        ),
        "ServerProviderUpdateError"
    );
    assert_eq!(
        failure(
            server
                .call(
                    "server.updateProvider",
                    json!({"provider":"codex","instanceId":"unknown"})
                )
                .await
        ),
        "ServerProviderUpdateError"
    );
    let refused = ok(server.call("server.updateProvider",json!({"provider":"codex","instanceId":"native","targetVersion":"0.166.0 --untrusted-argument"})).await);
    assert!(
        refused["providers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["instanceId"] == "native" && row["updateState"]["status"] == "failed")
    );
    assert!(!server.effects.updated.load(Ordering::SeqCst));
    let result = ok(server
        .call(
            "server.updateProvider",
            json!({"provider":"codex","instanceId":"native"}),
        )
        .await);
    assert!(server.effects.updated.load(Ordering::SeqCst));
    assert!(
        result["providers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|provider| provider["instanceId"] == "native" && provider["version"] == "0.166.0")
    );
    server.socket.close(None).await.unwrap();
    server.runtime.shutdown().await.unwrap();
}

fn transferred_profile() -> Value {
    json!({"registration":{"clientId":"fixture"},"credentials":{"clientId":"fixture","accessToken":"private-transfer-access","refreshToken":null,"idToken":"private-transfer-id","issuer":"https://auth.openai.com","expiresAt":1800000000000_u64,"earliestRefreshAt":null,"scopes":[],"subject":"fixture","email":null}})
}
#[tokio::test]
async fn every_served_provider_method_refuses_a_session_missing_its_contract_scope() {
    let mut server = Server::start().await;
    server.reconnect_with(&["terminal:read"]).await;
    for (tag, input, kind) in [
        (
            "server.updateProvider",
            json!({"provider":"codex","instanceId":"codex"}),
            CallKind::Unary,
        ),
        (
            "provider.auth.start",
            json!({"instanceId":"codex"}),
            CallKind::Unary,
        ),
        (
            "provider.auth.respond",
            json!({"instanceId":"codex","flowId":"flow","interactionId":"interaction","response":{"type":"browser","action":"accept"}}),
            CallKind::Unary,
        ),
        (
            "provider.auth.complete",
            json!({"instanceId":"codex","flowId":"flow","callbackUrl":"http://127.0.0.1:52000/auth/callback"}),
            CallKind::Unary,
        ),
        (
            "provider.auth.cancel",
            json!({"instanceId":"codex","flowId":"flow"}),
            CallKind::Unary,
        ),
        (
            "provider.auth.removeMethod",
            json!({"instanceId":"codex","methodId":"method"}),
            CallKind::Unary,
        ),
        (
            "provider.auth.logout",
            json!({"instanceId":"codex"}),
            CallKind::Unary,
        ),
        (
            "provider.auth.subscribe",
            json!({"instanceId":"codex"}),
            CallKind::Stream,
        ),
        (
            "provider.chatgpt.reconnect-profile",
            json!({"instanceId":"codex","methodId":"chatgpt-change-account"}),
            CallKind::Unary,
        ),
        (
            "provider.chatgpt.import-profile",
            json!({"instanceId":"codex","profile":transferred_profile()}),
            CallKind::Unary,
        ),
        (
            "provider.install.start",
            json!({"instanceId":"codex"}),
            CallKind::Unary,
        ),
        (
            "provider.install.cancel",
            json!({"instanceId":"codex","operationId":"operation"}),
            CallKind::Unary,
        ),
        (
            "provider.install.remove",
            json!({"instanceId":"codex"}),
            CallKind::Unary,
        ),
        (
            "provider.install.subscribe",
            json!({"instanceId":"codex"}),
            CallKind::Stream,
        ),
        (
            "server.searchAcpRegistry",
            json!({"query":""}),
            CallKind::Unary,
        ),
        (
            "server.prepareAcpRegistryAgent",
            json!({"agentId":"fixture"}),
            CallKind::Unary,
        ),
        (
            "server.uninstallAcpRegistryManagedBinary",
            json!({"agentId":"fixture"}),
            CallKind::Unary,
        ),
        (
            "server.listTextGeneratorModels",
            json!({"id":"fixture"}),
            CallKind::Unary,
        ),
    ] {
        assert!(!server.runtime.unserved_methods.contains(&tag), "{tag}");
        server.client.start(tag, input, kind).unwrap();
        match server.event().await {
            CallEvent::Exit { exit, .. } => {
                assert_eq!(failure(exit), "EnvironmentAuthorizationError", "{tag}")
            }
            event => panic!("unexpected {event:?}"),
        }
    }
    server.socket.close(None).await.unwrap();
    server.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn server_configuration_and_auth_use_the_loaded_managed_provider_policy() {
    let mut server=Server::start_with(|home|{
        let policy=home.join("managed-policy.json");
        std::fs::write(&policy,json!({"schemaVersion":1,"organization":"Fixture organization","limits":{"providers":{"allowedDrivers":["claudeAgent"]}}}).to_string()).unwrap();
        vec![("T3CODE_DEV_MANAGED_POLICY_PATH".into(),policy.to_string_lossy().into_owned())]
    }).await;
    let config = ok(server.call("server.getConfig", json!({})).await);
    let codex = config["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|provider| provider["instanceId"] == "codex")
        .unwrap();
    assert_eq!(codex["enabled"], false);
    assert!(!codex["availability"].is_null());
    assert_eq!(
        failure(
            server
                .call("provider.auth.start", json!({"instanceId":"codex"}))
                .await
        ),
        "ProviderSetupError"
    );
    assert_eq!(
        failure(
            server
                .call("server.listTextGeneratorModels", json!({"id":"fixture"}))
                .await
        ),
        "TextGeneratorError"
    );
    server.socket.close(None).await.unwrap();
    server.runtime.shutdown().await.unwrap();
}

fn fixture_signing_key() -> p256::ecdsa::SigningKey {
    p256::ecdsa::SigningKey::from_slice(&[9u8; 32]).unwrap()
}
fn fixture_jwk() -> Value {
    let point = fixture_signing_key()
        .verifying_key()
        .to_encoded_point(false);
    json!({"kty":"EC","crv":"P-256","kid":"fixture","alg":"ES256","use":"sig",
        "x":t3_auth::utils::b64(point.x().unwrap()),"y":t3_auth::utils::b64(point.y().unwrap())})
}
fn transferred_signed_profile() -> Value {
    use p256::ecdsa::signature::Signer;
    let input = format!("{}.{}",
        t3_auth::utils::b64(json!({"alg":"ES256","kid":"fixture"}).to_string()),
        t3_auth::utils::b64(json!({"iss":"https://auth.openai.com","aud":"fixture","sub":"fixture-subject","exp":1_800_003_600_u64}).to_string()));
    let signature: p256::ecdsa::Signature = fixture_signing_key().sign(input.as_bytes());
    let token = format!("{input}.{}", t3_auth::utils::b64(signature.to_bytes()));
    json!({"registration":{"clientId":"fixture","subject":"fixture-subject","idTokenHint":"private-import-hint"},
        "credentials":{"clientId":"fixture","accessToken":"private-transfer-access","refreshToken":null,"idToken":token,
        "issuer":"https://auth.openai.com","expiresAt":1_800_003_600_000_u64,"earliestRefreshAt":null,
        "scopes":["chatgpt.tokens.use.direct"],"subject":"fixture-subject","email":null}})
}
#[derive(Clone)]
struct LogWriter(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[tokio::test]
async fn reconnect_exports_only_the_authorized_hint_and_import_strips_it_from_registration() {
    let logs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let output = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || LogWriter(output.clone()))
        .finish();
    let _capture = tracing::subscriber::set_default(subscriber);
    tracing::info!("provider fixture log capture");
    let mut server = Server::start().await;
    let profile = transferred_signed_profile();
    let token = profile["credentials"]["idToken"].as_str().unwrap();
    let transferred: t3_contracts::provider_setup::ChatGptTransferredProfile =
        serde_json::from_value(profile.clone()).unwrap();
    for secret in [token, "private-import-hint", "private-transfer-access"] {
        assert!(!format!("{transferred:?}").contains(secret));
    }

    let imported = ok(server
        .call(
            "provider.chatgpt.import-profile",
            json!({"instanceId":"codex","profile":profile}),
        )
        .await);
    assert!(!imported.to_string().contains(token));
    let exported = ok(server
        .call(
            "provider.chatgpt.reconnect-profile",
            json!({"instanceId":"codex","methodId":"chatgpt-profile:fixture"}),
        )
        .await);
    assert_eq!(exported["idTokenHint"], token);
    let typed: t3_contracts::provider_setup::ChatGptReconnectProfile =
        serde_json::from_value(exported.clone()).unwrap();
    assert!(!format!("{typed:?}").contains(token));
    let mut public = exported.clone();
    public.as_object_mut().unwrap().remove("idTokenHint");
    assert!(!public.to_string().contains(token));
    assert!(!exported.to_string().contains("private-transfer-access"));
    let store = t3_config::secrets::SecretStore::new(server.home().join("userdata/secrets"));
    let registration = store
        .get(&t3_provider::secrets::credential_binding_key(
            "codex-chatgpt-registration",
            "codex",
        ))
        .unwrap()
        .unwrap();
    let registration: Value = serde_json::from_slice(&registration).unwrap();
    assert!(registration["profiles"][0].get("idTokenHint").is_none());
    let config = ok(server.call("server.getConfig", json!({})).await);
    assert!(!config.to_string().contains(token));
    server
        .client
        .start(
            "provider.auth.subscribe",
            json!({"instanceId":"codex"}),
            CallKind::Stream,
        )
        .unwrap();
    let status = server.chunk().await;
    assert!(!status.to_string().contains(token));
    let error = server
        .call(
            "provider.chatgpt.reconnect-profile",
            json!({"instanceId":"codex","methodId":"chatgpt-profile:missing"}),
        )
        .await;
    let error_json = serde_json::to_value(&error).unwrap();
    assert_eq!(error_json["cause"][0]["error"]["operation"], "export");
    assert_eq!(
        error_json["cause"][0]["error"]["detail"],
        "This saved connection is no longer available."
    );
    for secret in [token, "private-import-hint", "private-transfer-access"] {
        assert!(!error_json.to_string().contains(secret));
    }
    assert_eq!(failure(error), "ProviderSetupError");
    assert_eq!(
        ok(server
            .call(
                "provider.chatgpt.reconnect-profile",
                json!({"instanceId":"codex","methodId":"chatgpt-change-account"})
            )
            .await),
        Value::Null
    );
    server.socket.close(None).await.unwrap();
    server.runtime.shutdown().await.unwrap();
    let logs = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("provider fixture log capture"));
    for secret in [token, "private-import-hint", "private-transfer-access"] {
        assert!(!logs.contains(secret));
    }
}
