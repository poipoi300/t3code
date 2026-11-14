//! ACP Registry agent management over a real socket: a registry instance enabled as a provider,
//! its native sessions listed and deleted, its configurable model providers listed, set and
//! disabled, a sign-in completed through the URL consent flow and signed out, and every served
//! method's typed failure and missing-scope refusal.
//!
//! No real registry agent runs. The instance is a local agent whose command is this test binary:
//! started with `T3_FAKE_ACP` in its environment (one of the instance's own variables), it
//! answers ACP over stdio and records what it was asked in a state directory. The agent's
//! arguments are not libtest's, so the file has its own `main` (`harness = false`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    clippy::print_stderr
)]

use serde_json::{Value, json};
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    process::ExitCode,
};

const INSTANCE: &str = "acpFake";
const PROJECT: &str = "project:acp";
const HEADER_SECRET: &str = "acp-header-secret-91c4";
const LOGIN_CODE: &str = "acp-login-code-7d2e";

const SERVED: &[&str] = &[
    "server.acceptAcpRegistryUrlAuth",
    "server.listAcpRegistrySessions",
    "server.deleteAcpRegistrySession",
    "server.listAcpRegistryProviders",
    "server.setAcpRegistryProvider",
    "server.disableAcpRegistryProvider",
    "server.logoutAcpRegistry",
    "server.importAcpRegistrySession",
];

fn main() -> ExitCode {
    if std::env::var_os("T3_FAKE_ACP").is_some() {
        return fake::run();
    }
    type Case = fn() -> Pin<Box<dyn Future<Output = ()>>>;
    let cases: &[(&str, Case)] = &[
        ("registry_agent_sessions_are_listed_and_deleted", || {
            Box::pin(sessions())
        }),
        (
            "registry_agent_model_providers_are_listed_set_and_disabled",
            || Box::pin(providers()),
        ),
        (
            "registry_agent_url_sign_in_is_accepted_then_signed_out",
            || Box::pin(url_sign_in_and_logout()),
        ),
        (
            "registry_turn_streams_signs_in_asks_approval_and_settles_with_a_checkpoint",
            || Box::pin(registry_turn()),
        ),
        (
            "registry_turns_are_refused_above_the_ceiling_disabled_or_blocked",
            || Box::pin(registry_turn_refusals()),
        ),
        (
            "imported_session_brings_its_history_into_the_thread",
            || Box::pin(imported_session_history()),
        ),
        ("every_method_refuses_a_session_missing_its_scope", || {
            Box::pin(scope_refusals())
        }),
    ];
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.iter().any(|argument| argument == "--list") {
        for (name, _) in cases {
            println!("{name}: test");
        }
        return ExitCode::SUCCESS;
    }
    let exact = arguments.iter().any(|argument| argument == "--exact");
    let filter = arguments.iter().find(|argument| !argument.starts_with('-'));
    let selected: Vec<_> = cases
        .iter()
        .filter(|(name, _)| {
            filter.is_none_or(|filter| {
                if exact {
                    name == filter
                } else {
                    name.contains(filter.as_str())
                }
            })
        })
        .collect();
    println!("\nrunning {} tests", selected.len());
    let mut failed = Vec::new();
    for (name, case) in &selected {
        // The case thread polls the server's own start (about 0.8 MB of stack in a debug build,
        // measured) and the test client, which together outgrow Windows' 1 MB main thread.
        // Request handling runs on the runtime's workers at tokio's default 2 MiB, as in `t3`.
        let case = *case;
        let result = std::thread::Builder::new()
            .name("acp-case".into())
            .stack_size(4 << 20)
            .spawn(move || {
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(case());
            })
            .unwrap()
            .join();
        println!(
            "test {name} ... {}",
            if result.is_ok() { "ok" } else { "FAILED" }
        );
        if result.is_err() {
            failed.push(*name);
        }
    }
    println!(
        "\ntest result: {}. {} passed; {} failed\n",
        if failed.is_empty() { "ok" } else { "FAILED" },
        selected.len() - failed.len(),
        failed.len()
    );
    if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        for name in failed {
            println!("failed: {name}");
        }
        ExitCode::FAILURE
    }
}

/// The registry instance: a local agent whose command is this binary.
fn settings(state: &Path, login_url: Option<&str>) -> Value {
    settings_enabled(state, login_url, true)
}

fn settings_enabled(state: &Path, login_url: Option<&str>, enabled: bool) -> Value {
    let mut environment = vec![
        json!({"name":"T3_FAKE_ACP","value":"1","sensitive":false}),
        json!({"name":"T3_FAKE_ACP_STATE","value":state,"sensitive":false}),
    ];
    if let Some(url) = login_url {
        environment.push(json!({"name":"T3_FAKE_ACP_LOGIN_URL","value":url,"sensitive":false}));
    }
    json!({"providerInstances": {INSTANCE: {
        "driver": "acpRegistry",
        "enabled": enabled,
        "environment": environment,
        "config": {"source": "local", "commandPath": std::env::current_exe().unwrap()},
    }}})
}

/// The methods the agent was asked, with their parameters, in order.
fn calls(state: &Path) -> Vec<Value> {
    std::fs::read_to_string(state.join("calls.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn reason(failure: &Value) -> Option<String> {
    harness::objects(std::slice::from_ref(failure), &|value| {
        value["_tag"] == "AcpRegistryOperationError"
    })
    .first()
    .map(|error| error["reason"].as_str().unwrap().to_owned())
}

fn refused_scope(failure: &Value) -> Option<String> {
    harness::objects(std::slice::from_ref(failure), &|value| {
        value["_tag"] == "EnvironmentAuthorizationError"
    })
    .first()
    // `requiredScope` carries the legacy name; `requiredPermission` the scope itself.
    .map(|error| error["requiredPermission"].as_str().unwrap().to_owned())
}

mod harness {
    use super::*;
    use futures::{SinkExt, StreamExt};
    use std::collections::BTreeMap;
    use t3_rpc::{CallEvent, CallKind, Client, Exit, Frame, RequestId};
    use t3_server::{
        adapters::config::{ConfigInputs, ServerFlags},
        runtime::{RunningServer, StartOptions},
    };
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

    type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

    pub struct Server {
        pub runtime: RunningServer,
        pub workspace: PathBuf,
        origin: String,
        admin: String,
        /// The scopes the admin session holds.
        pub held: Vec<Value>,
        socket: Socket,
        client: Client,
        config_stream: Option<RequestId>,
        thread_stream: Option<RequestId>,
        finished: BTreeMap<String, Exit>,
        /// Every frame the client received.
        pub received: String,
        /// The configuration stream's values, in order.
        pub config: Vec<Value>,
        /// The thread stream's values, in order.
        pub thread: Vec<Value>,
        origin_cookie: String,
    }

    pub fn git(path: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .env("GIT_CONFIG_GLOBAL", path.join(".no-global-config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Enough for a Windows child to start; no PATH and no real home.
    fn environment() -> BTreeMap<String, String> {
        ["SystemRoot", "SYSTEMROOT", "windir", "TEMP", "TMP"]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_owned(), value)))
            .collect()
    }

    async fn session_cookie(origin: &str, credential: &Value) -> (String, Value) {
        let response = reqwest::Client::new()
            .post(format!("{origin}/api/auth/browser-session"))
            .json(&json!({ "credential": credential }))
            .send()
            .await
            .unwrap();
        let cookie = response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        (cookie, response.json().await.unwrap())
    }

    async fn connect(origin: &str, address: std::net::SocketAddr, cookie: &str) -> Socket {
        let ticket: Value = reqwest::Client::new()
            .post(format!("{origin}/api/auth/websocket-ticket/rpc"))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        tokio_tungstenite::connect_async(format!(
            "ws://{address}/ws?orchestrationProtocol=2&wsTicket={}",
            ticket["ticket"].as_str().unwrap()
        ))
        .await
        .unwrap()
        .0
    }

    impl Server {
        /// `settings` is written as `settings.json` on the first start; `policy` becomes the
        /// managed policy file.
        pub async fn start(home: &Path, settings: &Value, policy: Option<&str>) -> Self {
            std::fs::create_dir_all(home.join("userdata")).unwrap();
            if !home.join("userdata/settings.json").exists() {
                std::fs::write(home.join("userdata/settings.json"), settings.to_string()).unwrap();
            }
            let workspace = home.join("workspace");
            if !workspace.exists() {
                std::fs::create_dir_all(&workspace).unwrap();
                git(&workspace, &["init", "-q", "-b", "main"]);
                git(&workspace, &["config", "core.autocrlf", "false"]);
                std::fs::write(workspace.join("README.md"), "workspace\n").unwrap();
                git(&workspace, &["add", "."]);
                git(&workspace, &["commit", "-q", "-m", "init"]);
            }
            let mut env = environment();
            if let Some(policy) = policy {
                let path = home.join("policy.json");
                std::fs::write(&path, policy).unwrap();
                env.insert(
                    "T3CODE_DEV_MANAGED_POLICY_PATH".into(),
                    path.to_string_lossy().into_owned(),
                );
            }
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
                StartOptions {
                    provider_effects: Some(effects::provider_effects()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            let origin = format!("http://{}", runtime.address);
            let (admin, body) = session_cookie(&origin, &json!(runtime.pairing_credential)).await;
            let held = body["permissions"].as_array().unwrap().clone();
            // The project row a ProjectService commit would have written.
            let layout = t3_config::home::HomeLayout {
                base_dir: home.into(),
                state_dir: home.join("userdata"),
            };
            t3_persistence::rusqlite::Connection::open(layout.database())
                .unwrap()
                .execute(
                    "INSERT OR IGNORE INTO projection_projects
                     (project_id, title, workspace_root, scripts_json, created_at, updated_at)
                     VALUES (?1, 'ACP', ?2, '[]', '2026-11-14T12:00:00.000Z', '2026-11-14T12:00:00.000Z')",
                    [PROJECT, &workspace.to_string_lossy()],
                )
                .unwrap();
            let socket = connect(&origin, runtime.address, &admin).await;
            let admin_cookie = admin.clone();
            Self {
                runtime,
                workspace,
                origin,
                admin,
                held,
                socket,
                client: Client::new(0, Default::default()),
                config_stream: None,
                thread_stream: None,
                finished: BTreeMap::new(),
                received: String::new(),
                config: Vec::new(),
                thread: Vec::new(),
                origin_cookie: admin_cookie,
            }
        }

        /// Replaces the connection with one whose session holds every admin scope but `withheld`.
        pub async fn reconnect_without(&mut self, withheld: &str) {
            let scopes: Vec<&Value> = self.held.iter().filter(|s| *s != withheld).collect();
            let grant: Value = reqwest::Client::new()
                .post(format!("{}/api/auth/pairing-token", self.origin))
                .header("cookie", &self.admin)
                .json(&json!({ "scopes": scopes }))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let (cookie, _) = session_cookie(&self.origin, &grant["credential"]).await;
            let _ = self.socket.close(None).await;
            self.socket = connect(&self.origin, self.runtime.address, &cookie).await;
            self.client = Client::new(0, Default::default());
            self.config_stream = None;
            self.finished.clear();
        }

        async fn event(&mut self) -> CallEvent {
            loop {
                while let Some(message) = self.client.pop_outgoing() {
                    let Frame::Text(text) = t3_rpc::codec::encode(&message).unwrap() else {
                        panic!("text frame expected");
                    };
                    self.socket.send(Message::Text(text.into())).await.unwrap();
                }
                if let Some(event) = self.client.pop_event() {
                    return event;
                }
                let frame = match self.socket.next().await.unwrap().unwrap() {
                    Message::Text(text) => {
                        self.received.push_str(&text);
                        Frame::Text(text.to_string())
                    }
                    Message::Binary(bytes) => {
                        self.received.push_str(&String::from_utf8_lossy(&bytes));
                        Frame::Binary(bytes.to_vec())
                    }
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

        /// Handles one event: configuration values are recorded, finished calls kept.
        async fn pump(&mut self) {
            match self.event().await {
                CallEvent::Chunk { id, values } if self.thread_stream.as_ref() == Some(&id) => {
                    self.client.acknowledge(id);
                    self.thread.extend(values);
                }
                CallEvent::Exit { id, exit } if self.thread_stream.as_ref() == Some(&id) => panic!(
                    "thread stream ended: {}",
                    serde_json::to_value(exit).unwrap()
                ),
                CallEvent::Chunk { id, values } if self.config_stream.as_ref() == Some(&id) => {
                    self.client.acknowledge(id);
                    self.config.extend(values);
                }
                CallEvent::Exit { id, exit } if self.config_stream.as_ref() == Some(&id) => panic!(
                    "configuration stream ended: {}",
                    serde_json::to_value(exit).unwrap()
                ),
                CallEvent::Exit { id, exit } => {
                    self.finished.insert(format!("{id:?}"), exit);
                }
                other => panic!("unexpected {other:?}"),
            }
        }

        pub fn start_call(&mut self, method: &str, payload: Value) -> RequestId {
            self.client.start(method, payload, CallKind::Unary).unwrap()
        }

        /// A started call's result: `Ok(value)` or `Err(failure json)`.
        pub async fn finish(&mut self, id: RequestId) -> Result<Value, Value> {
            let key = format!("{id:?}");
            loop {
                if let Some(exit) = self.finished.remove(&key) {
                    return match exit {
                        Exit::Success { value } => Ok(value.unwrap_or(Value::Null)),
                        other => Err(serde_json::to_value(other).unwrap()),
                    };
                }
                self.pump().await;
            }
        }

        pub async fn call(&mut self, method: &str, payload: Value) -> Result<Value, Value> {
            let id = self.start_call(method, payload);
            self.finish(id).await
        }

        pub fn subscribe_config(&mut self) {
            self.config_stream = Some(
                self.client
                    .start("subscribeServerConfig", json!({}), CallKind::Stream)
                    .unwrap(),
            );
        }

        /// Pumps the socket until `found` returns a value from the registry instance's
        /// published statuses so far.
        pub async fn until_provider<T>(&mut self, found: impl Fn(&Value) -> Option<T>) -> T {
            loop {
                let statuses = objects(&self.config, &|value| {
                    value["instanceId"] == INSTANCE && value["driver"] == "acpRegistry"
                });
                if let Some(value) = statuses.into_iter().rev().find_map(&found) {
                    return value;
                }
                self.pump().await;
            }
        }

        fn statuses(&self) -> Vec<&Value> {
            objects(&self.config, &|value| {
                value["instanceId"] == INSTANCE && value["driver"] == "acpRegistry"
            })
        }

        pub fn status_count(&self) -> usize {
            self.statuses().len()
        }

        /// A configuration change re-probes the agent (as TypeScript's `refreshInstance` after
        /// `invalidateEnrichmentCache`): the readiness status, then the probed one. Capability
        /// checks read the probed status, so a client waits for it as this does.
        pub async fn reprobed(&mut self, since: usize) {
            loop {
                let statuses = self.statuses();
                let later = statuses.get(since..).unwrap_or_default();
                if let Some(base) = later.iter().position(|s| !s["nativeSessions"].is_object())
                    && later[base..]
                        .iter()
                        .any(|s| s["configurableProviders"] == true)
                {
                    return;
                }
                self.pump().await;
            }
        }

        /// Pumps the socket until the instance's latest published status satisfies `holds`.
        pub async fn until_latest_provider(&mut self, holds: impl Fn(&Value) -> bool) {
            loop {
                let statuses = objects(&self.config, &|value| {
                    value["instanceId"] == INSTANCE && value["driver"] == "acpRegistry"
                });
                if statuses.last().is_some_and(|status| holds(status)) {
                    return;
                }
                self.pump().await;
            }
        }

        /// The registry agent's probe finished: the status advertises its capabilities.
        pub async fn probed(&mut self) {
            self.until_provider(|provider| provider["nativeSessions"].is_object().then_some(()))
                .await;
        }

        pub async fn dispatch(&mut self, command: Value) -> Result<Value, Value> {
            self.call("orchestration.dispatchCommand", command).await
        }

        pub async fn create_thread(&mut self, mode: &str) -> Result<Value, Value> {
            self.dispatch(json!({"type":"thread.create","commandId":"command:thread","threadId":THREAD,"projectId":PROJECT,"title":"Registry thread","modelSelection":{"instanceId":INSTANCE,"model":"model"},"runtimeMode":mode,"interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"})).await
        }

        pub fn subscribe_thread(&mut self, thread: &str) {
            self.thread.clear();
            self.thread_stream = Some(
                self.client
                    .start(
                        "orchestration.subscribeThread",
                        json!({ "threadId": thread }),
                        CallKind::Stream,
                    )
                    .unwrap(),
            );
        }

        pub async fn send(&mut self, id: &str, text: &str) -> Result<Value, Value> {
            self.dispatch(json!({"type":"message.dispatch","commandId":format!("command:{id}"),"threadId":THREAD,"messageId":format!("message:{id}"),"text":text,"attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"})).await
        }

        /// Pumps the socket until `found` returns a value from the thread stream so far.
        pub async fn until_thread<T>(&mut self, found: impl Fn(&[Value]) -> Option<T>) -> T {
            loop {
                if let Some(value) = found(&self.thread) {
                    return value;
                }
                self.pump().await;
            }
        }

        pub async fn snapshot(&self, thread: &str) -> Value {
            reqwest::Client::new()
                .get(format!(
                    "{}/api/orchestration/threads/{thread}",
                    self.origin
                ))
                .header("cookie", &self.origin_cookie)
                .header("x-t3-orchestration-protocol", "2")
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap()
        }

        pub async fn stop(mut self) -> String {
            let _ = self.socket.close(None).await;
            self.runtime.shutdown().await.unwrap();
            self.received
        }
    }

    /// Every object in `values` (recursively) that `matches`.
    pub fn objects<'a>(values: &'a [Value], matches: &dyn Fn(&Value) -> bool) -> Vec<&'a Value> {
        fn walk<'a>(value: &'a Value, matches: &dyn Fn(&Value) -> bool, out: &mut Vec<&'a Value>) {
            match value {
                Value::Object(map) => {
                    if matches(value) {
                        out.push(value);
                    }
                    map.values().for_each(|child| walk(child, matches, out));
                }
                Value::Array(items) => items.iter().for_each(|child| walk(child, matches, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        values
            .iter()
            .for_each(|value| walk(value, matches, &mut out));
        out
    }

    mod effects {
        //! Provider status and installation effects: nothing probes or downloads a real CLI.
        use async_trait::async_trait;
        use std::{path::Path, sync::Arc};
        use t3_provider::{
            codex_installation::*,
            discovery::{Driver, VersionProbe},
            native_probe::*,
            native_provider::ExecutableProbe,
            sensitive::*,
            transport::*,
        };

        pub struct Effects;
        impl ProviderClock for Effects {
            fn now_ms(&self) -> i64 {
                1_800_000_000_000
            }
        }
        #[async_trait]
        impl HttpTransport for Effects {
            async fn send(&self, _: HttpRequest) -> ServiceResult<HttpResponse> {
                Err(ServiceError::Network)
            }
        }
        #[async_trait]
        impl NativeProbe for Effects {
            async fn codex(&self, _: ProbeLaunch) -> ServiceResult<Sensitive<serde_json::Value>> {
                Err(ServiceError::Unavailable)
            }
            async fn claude(&self, _: ProbeLaunch) -> ServiceResult<Sensitive<serde_json::Value>> {
                Err(ServiceError::Unavailable)
            }
        }
        #[async_trait]
        impl ExecutableProbe for Effects {
            async fn version(
                &self,
                _: Driver,
                _: &str,
                _: &t3_provider::environment::Environment,
                _: &Path,
            ) -> ServiceResult<VersionProbe> {
                Err(ServiceError::Unavailable)
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
                Err(ServiceError::Network)
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
        pub fn provider_effects() -> Arc<t3_server::adapters::providers::ProviderEffects> {
            let effects = Arc::new(Effects);
            Arc::new(t3_server::adapters::providers::ProviderEffects {
                http: effects.clone(),
                clock: effects.clone(),
                protocols: effects.clone(),
                executables: effects.clone(),
                codex: t3_provider::domain::CodexInstallEffects {
                    transport: effects.clone(),
                    validator: effects,
                    local: Arc::new(NoLocalCodex),
                },
                maintenance_probe: None,
                maintenance_executor: None,
                registry_fetch: None,
                registry_commands: None,
                registry_downloads: None,
            })
        }
    }
}

use harness::Server;

async fn started(login_url: Option<&str>) -> (tempfile::TempDir, PathBuf, Server) {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("agent-state");
    std::fs::create_dir_all(&state).unwrap();
    let mut server = Server::start(temp.path(), &settings(&state, login_url), None).await;
    server.subscribe_config();
    server.probed().await;
    (temp, state, server)
}

fn scope(input: Value) -> Value {
    let mut input = input;
    input["instanceId"] = json!(INSTANCE);
    input["projectId"] = json!(PROJECT);
    input
}

/// The agent's native sessions are listed for the project's workspace and one is deleted; the
/// project, the instance and the instance's driver are each checked first.
async fn sessions() {
    let (_temp, state, mut server) = started(None).await;
    let workspace = server.workspace.to_string_lossy().into_owned();

    let listed = server
        .call("server.listAcpRegistrySessions", scope(json!({})))
        .await
        .unwrap();
    let ids: Vec<&str> = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|session| session["sessionId"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["one", "two"]);
    assert_eq!(listed["sessions"][0]["importedThreadId"], Value::Null);
    assert_eq!(listed["canDelete"], true);
    let list_call = calls(&state)
        .into_iter()
        .find(|call| call["method"] == "session/list")
        .unwrap();
    assert_eq!(
        list_call["params"]["cwd"], workspace,
        "the project's root, not a client path"
    );

    let deleted = server
        .call(
            "server.deleteAcpRegistrySession",
            scope(json!({"sessionId": "two"})),
        )
        .await
        .unwrap();
    assert_eq!(deleted, json!({"deleted": true}));
    assert!(
        calls(&state)
            .iter()
            .any(|call| call["method"] == "session/delete" && call["params"]["sessionId"] == "two")
    );
    let listed = server
        .call("server.listAcpRegistrySessions", scope(json!({})))
        .await
        .unwrap();
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 1);

    // Typed failures, before any agent runs.
    let before = calls(&state).len();
    let missing_project = server
        .call(
            "server.listAcpRegistrySessions",
            json!({"instanceId": INSTANCE, "projectId": "project:missing"}),
        )
        .await
        .unwrap_err();
    assert_eq!(
        reason(&missing_project).as_deref(),
        Some("project_not_found")
    );
    let missing_instance = server
        .call(
            "server.deleteAcpRegistrySession",
            json!({"instanceId": "missing", "projectId": PROJECT, "sessionId": "one"}),
        )
        .await
        .unwrap_err();
    assert_eq!(
        reason(&missing_instance).as_deref(),
        Some("instance_not_found")
    );
    let not_registry = server
        .call(
            "server.listAcpRegistrySessions",
            json!({"instanceId": "codex", "projectId": PROJECT}),
        )
        .await
        .unwrap_err();
    assert_eq!(
        reason(&not_registry).as_deref(),
        Some("session_list_unsupported")
    );
    assert_eq!(
        calls(&state).len(),
        before,
        "no agent ran for a refused call"
    );
    server.stop().await;
}

/// The agent's configurable model providers are listed, one is set (stopping live sessions
/// first) and disabled; a required provider, an unadvertised API type and too many headers are
/// refused. The header value reaches the agent and no client frame.
async fn providers() {
    let (_temp, state, mut server) = started(None).await;
    let listed = server
        .call("server.listAcpRegistryProviders", scope(json!({})))
        .await
        .unwrap();
    assert_eq!(listed["providers"].as_array().unwrap().len(), 2);
    assert_eq!(listed["providers"][0]["current"], Value::Null);

    let since = server.status_count();
    let configured = server
        .call(
            "server.setAcpRegistryProvider",
            scope(json!({"providerId":"openai","apiType":"openai","baseUrl":"https://api.example.invalid/v1","headers":{"Authorization":format!("Bearer {HEADER_SECRET}")}})),
        )
        .await
        .unwrap();
    assert_eq!(configured, json!({"configured": true}));
    let set = calls(&state)
        .into_iter()
        .find(|call| call["method"] == "providers/set")
        .unwrap();
    assert_eq!(
        set["params"]["headers"]["Authorization"],
        format!("Bearer {HEADER_SECRET}")
    );
    server.reprobed(since).await;
    let listed = server
        .call("server.listAcpRegistryProviders", scope(json!({})))
        .await
        .unwrap();
    assert_eq!(
        listed["providers"][0]["current"],
        json!({"apiType":"openai","baseUrl":"https://api.example.invalid/v1"})
    );

    let unsupported = server
        .call(
            "server.setAcpRegistryProvider",
            scope(json!({"providerId":"openai","apiType":"anthropic","baseUrl":"https://api.example.invalid"})),
        )
        .await
        .unwrap_err();
    assert_eq!(
        reason(&unsupported).as_deref(),
        Some("provider_configuration_failed")
    );
    let headers: serde_json::Map<String, Value> = (0..33)
        .map(|index| (format!("x-header-{index}"), json!("value")))
        .collect();
    let too_many = server
        .call(
            "server.setAcpRegistryProvider",
            scope(json!({"providerId":"openai","apiType":"openai","baseUrl":"https://api.example.invalid","headers":headers})),
        )
        .await
        .unwrap_err();
    assert_eq!(
        reason(&too_many).as_deref(),
        Some("provider_configuration_failed")
    );
    let required = server
        .call(
            "server.disableAcpRegistryProvider",
            scope(json!({"providerId":"builtin"})),
        )
        .await
        .unwrap_err();
    assert_eq!(
        reason(&required).as_deref(),
        Some("provider_configuration_failed")
    );
    let since = server.status_count();
    let disabled = server
        .call(
            "server.disableAcpRegistryProvider",
            scope(json!({"providerId":"openai"})),
        )
        .await
        .unwrap();
    assert_eq!(disabled, json!({"disabled": true}));
    server.reprobed(since).await;
    let listed = server
        .call("server.listAcpRegistryProviders", scope(json!({})))
        .await
        .unwrap();
    assert_eq!(listed["providers"][0]["current"], Value::Null);
    let missing = server
        .call(
            "server.listAcpRegistryProviders",
            json!({"instanceId": INSTANCE, "projectId": "project:missing"}),
        )
        .await
        .unwrap_err();
    assert_eq!(reason(&missing).as_deref(), Some("project_not_found"));
    let received = server.stop().await;
    assert!(!received.contains(HEADER_SECRET));
}

/// A management call that needs sign-in publishes the agent's URL consent request in the
/// instance's status; a wrong elicitation id is not accepted, the right one is, and the call
/// completes. The server never contacts the URL. Signing out reaches the agent.
async fn url_sign_in_and_logout() {
    let stand_in = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    stand_in.set_nonblocking(true).unwrap();
    let login = format!(
        "http://{}/login?code={LOGIN_CODE}",
        stand_in.local_addr().unwrap()
    );
    let (_temp, state, mut server) = started(Some(&login)).await;

    let pending = server.start_call("server.listAcpRegistrySessions", scope(json!({})));
    let elicitation = server
        .until_provider(|provider| {
            provider["auth"]["action"]["elicitationId"]
                .as_str()
                .map(str::to_owned)
        })
        .await;
    let action = server
        .until_provider(|provider| provider["auth"]["action"].as_object().cloned())
        .await;
    assert_eq!(action["url"], login);

    let wrong = server
        .call(
            "server.acceptAcpRegistryUrlAuth",
            json!({"instanceId": INSTANCE, "elicitationId": "not-the-pending-one"}),
        )
        .await
        .unwrap();
    assert_eq!(wrong, json!({"accepted": false}));
    let other_instance = server
        .call(
            "server.acceptAcpRegistryUrlAuth",
            json!({"instanceId": "codex", "elicitationId": elicitation}),
        )
        .await
        .unwrap();
    assert_eq!(other_instance, json!({"accepted": false}));
    let accepted = server
        .call(
            "server.acceptAcpRegistryUrlAuth",
            json!({"instanceId": INSTANCE, "elicitationId": elicitation}),
        )
        .await
        .unwrap();
    assert_eq!(accepted, json!({"accepted": true}));
    let listed = server.finish(pending).await.unwrap();
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 2);
    assert!(state.join("signed-in").exists());
    let again = server
        .call(
            "server.acceptAcpRegistryUrlAuth",
            json!({"instanceId": INSTANCE, "elicitationId": elicitation}),
        )
        .await
        .unwrap();
    assert_eq!(again, json!({"accepted": false}), "a consent is used once");

    let logged_out = server
        .call("server.logoutAcpRegistry", json!({"instanceId": INSTANCE}))
        .await
        .unwrap();
    assert_eq!(logged_out, json!({"loggedOut": true}));
    assert!(!state.join("signed-in").exists());
    assert!(
        calls(&state)
            .iter()
            .any(|call| call["method"] == "logout" || call["method"] == "auth/logout")
    );
    let missing = server
        .call("server.logoutAcpRegistry", json!({"instanceId": "missing"}))
        .await
        .unwrap_err();
    assert_eq!(reason(&missing).as_deref(), Some("instance_not_found"));
    // The action left the published status once it was answered.
    server
        .until_latest_provider(|provider| provider["auth"].get("action").is_none())
        .await;
    server.stop().await;
    assert!(
        matches!(stand_in.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "the server contacted the sign-in URL"
    );
}

/// Each served method refuses a session missing its scope with the typed authorization error,
/// before any agent runs; the served methods are off the unserved list.
async fn scope_refusals() {
    let (_temp, state, mut server) = started(None).await;
    for method in SERVED {
        assert!(
            !server.runtime.unserved_methods.contains(method),
            "{method} is served"
        );
    }

    let payloads = [
        (
            "server.acceptAcpRegistryUrlAuth",
            json!({"instanceId": INSTANCE, "elicitationId": "x"}),
        ),
        ("server.listAcpRegistrySessions", scope(json!({}))),
        (
            "server.deleteAcpRegistrySession",
            scope(json!({"sessionId": "one"})),
        ),
        ("server.listAcpRegistryProviders", scope(json!({}))),
        (
            "server.setAcpRegistryProvider",
            scope(
                json!({"providerId":"openai","apiType":"openai","baseUrl":"https://api.example.invalid"}),
            ),
        ),
        (
            "server.disableAcpRegistryProvider",
            scope(json!({"providerId":"openai"})),
        ),
        ("server.logoutAcpRegistry", json!({"instanceId": INSTANCE})),
        (
            "server.importAcpRegistrySession",
            scope(json!({"sessionId": "one"})),
        ),
    ];
    let before = calls(&state).len();
    for (method, payload) in payloads {
        let required = t3_auth::authorization::required_scopes_for_rpc_call(method, &payload)
            .unwrap()
            .into_iter()
            .map(|scope| serde_json::to_value(scope).unwrap())
            .find(|scope| server.held.contains(scope))
            .unwrap();
        server.reconnect_without(required.as_str().unwrap()).await;
        let refused = server.call(method, payload).await.unwrap_err();
        assert_eq!(
            refused_scope(&refused).as_deref(),
            required.as_str(),
            "{method}"
        );
    }
    assert_eq!(
        calls(&state).len(),
        before,
        "no agent ran for a refused call"
    );
    server.stop().await;
}

const THREAD: &str = "thread:registry";

/// The latest record of a run, by its status.
fn run_status(values: &[Value]) -> Option<String> {
    harness::objects(values, &|value| {
        value["userMessageId"].is_string()
            && value["rootNodeId"].is_string()
            && value["status"].is_string()
    })
    .last()
    .map(|run| run["status"].as_str().unwrap().to_owned())
}

fn launches(state: &Path) -> Vec<Value> {
    std::fs::read_to_string(state.join("launches.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// One turn on a registry agent: the agent starts from the instance's settings in the
/// project's workspace with only its resolved environment; its text streams; it asks for sign-in
/// during the turn, which is published on the instance and accepted by the client (the server
/// never contacts the URL); it asks to edit a workspace file, which the client approves; the run
/// settles with a Git checkpoint holding the agent's file.
async fn registry_turn() {
    let stand_in = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    stand_in.set_nonblocking(true).unwrap();
    let login = format!(
        "http://{}/login?code={LOGIN_CODE}",
        stand_in.local_addr().unwrap()
    );
    let (_temp, state, mut server) = started(Some(&login)).await;
    server.create_thread("approval-required").await.unwrap();
    server.subscribe_thread(THREAD);
    server.send("first", "Write the notes file.").await.unwrap();

    let elicitation = server
        .until_provider(|provider| {
            provider["auth"]["action"]["elicitationId"]
                .as_str()
                .filter(|id| *id == "turn-sign-in-1")
                .map(str::to_owned)
        })
        .await;
    assert_eq!(
        run_status(&server.thread).as_deref(),
        Some("running"),
        "the turn waits for the sign-in"
    );
    let wrong = server
        .call(
            "server.acceptAcpRegistryUrlAuth",
            json!({"instanceId": INSTANCE, "elicitationId": "sign-in-1"}),
        )
        .await
        .unwrap();
    assert_eq!(wrong, json!({"accepted": false}));
    let accepted = server
        .call(
            "server.acceptAcpRegistryUrlAuth",
            json!({"instanceId": INSTANCE, "elicitationId": elicitation}),
        )
        .await
        .unwrap();
    assert_eq!(accepted, json!({"accepted": true}));

    let request = server
        .until_thread(|values| {
            harness::objects(values, &|value| {
                value["responseCapability"].is_object() && value["status"] == "pending"
            })
            .first()
            .map(|record| record["id"].as_str().unwrap().to_owned())
        })
        .await;
    assert!(!server.workspace.join("agent-notes.txt").exists());
    server
        .dispatch(json!({"type":"runtime-request.respond","commandId":"command:approve","threadId":THREAD,"requestId":request,"decision":"accept"}))
        .await
        .unwrap();
    server
        .until_thread(|values| (run_status(values).as_deref() == Some("completed")).then_some(()))
        .await;
    let streamed = Value::Array(server.thread.clone()).to_string();
    assert!(streamed.contains("Working on it."), "{streamed}");
    assert!(streamed.contains("Done."), "{streamed}");
    assert_eq!(
        std::fs::read_to_string(server.workspace.join("agent-notes.txt")).unwrap(),
        "written by the registry agent\n"
    );
    assert!(state.join("signed-in").exists());

    let snapshot = server.snapshot(THREAD).await;
    let checkpoints = harness::objects(std::slice::from_ref(&snapshot), &|value| {
        value["status"] == "ready" && value["ref"].is_string()
    });
    assert!(!checkpoints.is_empty(), "{snapshot}");
    for checkpoint in &checkpoints {
        let reference = checkpoint["ref"].as_str().unwrap();
        harness::git(
            &server.workspace,
            &["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
        );
    }
    // The run's checkpoint (not the baseline) holds the file the agent wrote.
    assert!(
        checkpoints.iter().any(|checkpoint| {
            std::process::Command::new("git")
                .args([
                    "show",
                    &format!("{}:agent-notes.txt", checkpoint["ref"].as_str().unwrap()),
                ])
                .current_dir(&server.workspace)
                .output()
                .is_ok_and(|output| output.stdout == b"written by the registry agent\n")
        }),
        "{checkpoints:?}"
    );

    // The turn's agent ran in the workspace with no arguments, no PATH (the server has none) and
    // no T3 tool credential; it was asked with the registry client capabilities.
    let workspace = server.workspace.canonicalize().unwrap();
    let turn = launches(&state)
        .into_iter()
        .filter(|launch| {
            PathBuf::from(launch["cwd"].as_str().unwrap())
                .canonicalize()
                .is_ok_and(|cwd| cwd == workspace)
        })
        .collect::<Vec<_>>();
    assert!(!turn.is_empty());
    for launch in &turn {
        assert_eq!(launch["args"], json!([]), "{launch}");
        assert_eq!(launch["mcpEndpoint"], false, "{launch}");
        assert_eq!(launch["mcpAuthorization"], false, "{launch}");
        assert_eq!(launch["path"], false, "{launch}");
    }
    let initialize = calls(&state)
        .into_iter()
        .filter(|call| call["method"] == "initialize")
        .find(|call| call["params"]["clientInfo"]["name"] == "t3-code")
        .unwrap();
    let capabilities = &initialize["params"]["clientCapabilities"];
    assert_eq!(
        capabilities["fs"],
        json!({"readTextFile":false,"writeTextFile":false})
    );
    assert_eq!(capabilities["terminal"], false);
    assert_eq!(capabilities["elicitation"], json!({"form":{},"url":{}}));
    // The commands the live session advertised reached the instance's status.
    server
        .until_provider(|provider| {
            provider["slashCommands"]
                .as_array()
                .is_some_and(|commands| commands.iter().any(|c| c["name"] == "review"))
                .then_some(())
        })
        .await;
    let received = server.stop().await;
    assert!(
        matches!(stand_in.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "the server contacted the sign-in URL"
    );
    assert!(!received.contains("T3_ACP_MCP"));
}

/// A message on a registry thread is refused, and no agent prompt is sent, when the thread runs
/// above the managed ceiling, when the instance is disabled, and when the policy leaves the
/// ACP Registry driver out.
async fn registry_turn_refusals() {
    refused_above_the_ceiling().await;
    refused_when_disabled().await;
    refused_when_the_driver_is_blocked().await;
}

fn prompted(state: &Path) -> bool {
    calls(state)
        .iter()
        .any(|call| call["method"] == "session/prompt")
}

/// Created at full access before the policy existed, then above its ceiling.
async fn refused_above_the_ceiling() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("agent-state");
    std::fs::create_dir_all(&state).unwrap();
    let mut server = Server::start(temp.path(), &settings(&state, None), None).await;
    server.create_thread("full-access").await.unwrap();
    server.stop().await;
    let mut server = Server::start(
        temp.path(),
        &settings(&state, None),
        Some(r#"{"schemaVersion":1,"organization":"Acme","limits":{"maxRuntimeMode":"auto-accept-edits"}}"#),
    )
    .await;
    let refused = server
        .send("first", "Write the notes file.")
        .await
        .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("Acme limits threads to Auto-accept edits."),
        "{refused}"
    );
    assert!(!prompted(&state));
    server.stop().await;
}

async fn refused_when_disabled() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("agent-state");
    std::fs::create_dir_all(&state).unwrap();
    let mut server = Server::start(temp.path(), &settings_enabled(&state, None, false), None).await;
    server.create_thread("approval-required").await.unwrap();
    let refused = server
        .send("first", "Write the notes file.")
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains(&format!(
            r#"Provider instance \"{INSTANCE}\" is disabled for project {PROJECT}."#
        )),
        "{refused}"
    );
    assert!(!prompted(&state));
    server.stop().await;
}

/// A policy that leaves the driver out: the instance has no turn adapter.
async fn refused_when_the_driver_is_blocked() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("agent-state");
    std::fs::create_dir_all(&state).unwrap();
    let mut server = Server::start(
        temp.path(),
        &settings(&state, None),
        Some(r#"{"schemaVersion":1,"organization":"Acme","limits":{"providers":{"allowedDrivers":["codex"]}}}"#),
    )
    .await;
    server.create_thread("approval-required").await.unwrap();
    let refused = server
        .send("first", "Write the notes file.")
        .await
        .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("No orchestration provider adapter is registered"),
        "{refused}"
    );
    assert!(launches(&state).is_empty(), "no agent process starts");
    server.stop().await;
}

/// A saved session is imported: a thread is created for it and the history the agent replays
/// on `session/load` (user text, reasoning, answer) becomes the thread's run-less history. The
/// list shows the mapping, the mapped session cannot be deleted, and a second import changes
/// nothing.
async fn imported_session_history() {
    let (_temp, state, mut server) = started(None).await;
    let imported = server
        .call(
            "server.importAcpRegistrySession",
            scope(json!({"sessionId": "one", "title": "Imported one"})),
        )
        .await
        .unwrap();
    assert_eq!(imported["imported"], true, "{imported}");
    let thread = imported["threadId"].as_str().unwrap().to_owned();
    let load = calls(&state)
        .into_iter()
        .find(|call| call["method"] == "session/load" && call["params"]["sessionId"] == "one")
        .unwrap();
    assert_eq!(
        PathBuf::from(load["params"]["cwd"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        server.workspace.canonicalize().unwrap()
    );

    let snapshot = server.snapshot(&thread).await;
    let snapshot = std::slice::from_ref(&snapshot);
    let message = |role: &str, text: &str| {
        !harness::objects(snapshot, &|value| {
            value["role"] == role && value["text"] == text && value["runId"].is_null()
        })
        .is_empty()
    };
    assert!(message("user", "Hello from history"), "{snapshot:?}");
    assert!(message("assistant", "History answer"), "{snapshot:?}");
    assert!(
        !harness::objects(snapshot, &|value| {
            value["type"] == "reasoning" && value["text"] == "Thinking it over"
        })
        .is_empty(),
        "{snapshot:?}"
    );
    assert!(!message("assistant", "Thinking it over"), "{snapshot:?}");
    assert!(
        !harness::objects(snapshot, &|value| value["historyOrigin"] == "v1_import").is_empty(),
        "{snapshot:?}"
    );

    let listed = server
        .call("server.listAcpRegistrySessions", scope(json!({})))
        .await
        .unwrap();
    let one = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["sessionId"] == "one")
        .unwrap();
    assert_eq!(one["importedThreadId"], thread);
    assert!(one.get("nativeHistoryPending").is_none(), "{one}");
    let refused = server
        .call(
            "server.deleteAcpRegistrySession",
            scope(json!({"sessionId": "one"})),
        )
        .await
        .unwrap_err();
    assert_eq!(reason(&refused).as_deref(), Some("session_delete_failed"));
    let again = server
        .call(
            "server.importAcpRegistrySession",
            scope(json!({"sessionId": "one"})),
        )
        .await
        .unwrap();
    assert_eq!(again, json!({"threadId": thread, "imported": false}));
    let missing = server
        .call(
            "server.importAcpRegistrySession",
            json!({"instanceId": INSTANCE, "projectId": "project:missing", "sessionId": "one"}),
        )
        .await
        .unwrap_err();
    assert_eq!(reason(&missing).as_deref(), Some("project_not_found"));
    server.stop().await;
}

mod fake {
    //! The registry agent: ACP over stdio, recording each request in the state directory.
    use super::*;
    use std::io::{BufRead, Write};

    fn append(state: &Path, file: &str, value: &Value) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(state.join(file))
            .unwrap();
        writeln!(file, "{value}").unwrap();
    }

    fn deleted(state: &Path) -> Vec<String> {
        std::fs::read_to_string(state.join("deleted"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn send(output: &mut impl Write, value: Value) {
        writeln!(output, "{value}").unwrap();
        output.flush().unwrap();
    }

    fn update(output: &mut impl Write, session: &str, update: Value) {
        send(
            output,
            json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":update}}),
        );
    }

    fn text(kind: &str, text: &str) -> Value {
        json!({"sessionUpdate":kind,"content":{"type":"text","text":text}})
    }

    /// Sends a request to the client and waits for its answer.
    fn request(
        output: &mut impl Write,
        input: &mut impl Iterator<Item = std::io::Result<String>>,
        id: &str,
        method: &str,
        params: Value,
    ) -> Value {
        send(
            output,
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
        );
        while let Some(Ok(line)) = input.next() {
            let reply: Value = serde_json::from_str(&line).unwrap_or_default();
            if reply["id"] == id && reply.get("method").is_none() {
                return reply;
            }
        }
        Value::Null
    }

    fn accepted(reply: &Value) -> bool {
        let action = &reply["result"]["action"];
        action == "accept" || action["action"] == "accept"
    }

    pub fn run() -> ExitCode {
        let state = PathBuf::from(std::env::var("T3_FAKE_ACP_STATE").unwrap());
        let login = std::env::var("T3_FAKE_ACP_LOGIN_URL").ok();
        // What this process was started with: no T3 tool credential, its own working directory.
        append(
            &state,
            "launches.jsonl",
            &json!({
                "pid": std::process::id(),
                "cwd": std::env::current_dir().unwrap(),
                "args": std::env::args().skip(1).collect::<Vec<_>>(),
                "mcpEndpoint": std::env::var_os("T3_ACP_MCP_ENDPOINT").is_some(),
                "mcpAuthorization": std::env::var_os("T3_ACP_MCP_AUTHORIZATION").is_some(),
                "path": std::env::var_os("PATH").is_some(),
            }),
        );
        let mut output = std::io::stdout().lock();
        let stdin = std::io::stdin();
        let mut input = stdin.lock().lines();
        let mut cwd: Option<PathBuf> = None;
        while let Some(Ok(line)) = input.next() {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let (Some(method), Some(id)) = (message["method"].as_str(), message.get("id")) else {
                continue;
            };
            let params = &message["params"];
            append(
                &state,
                "calls.jsonl",
                &json!({"method": method, "params": params}),
            );
            let models = json!({"currentModelId":"model","availableModels":[{"modelId":"model","name":"Model"}]});
            let result = match method {
                "initialize" => Ok(json!({
                    "protocolVersion": 1,
                    "agentCapabilities": {
                        "loadSession": true,
                        "sessionCapabilities": {"list": {}, "delete": {}, "resume": {}},
                        "auth": {"logout": {}},
                        "providers": {},
                    },
                    "authMethods": [{"id": "login", "name": "Sign in"}],
                })),
                "session/new" => {
                    cwd = params["cwd"].as_str().map(PathBuf::from);
                    let session = format!("live-{}", std::process::id());
                    update(
                        &mut output,
                        &session,
                        json!({"sessionUpdate":"available_commands_update","availableCommands":[{"name":"review","description":"Review the change"}]}),
                    );
                    Ok(json!({"sessionId":session,"models":models,"configOptions":[]}))
                }
                "session/load" => {
                    cwd = params["cwd"].as_str().map(PathBuf::from);
                    let session = params["sessionId"].as_str().unwrap_or_default();
                    if session == "one" {
                        update(
                            &mut output,
                            session,
                            text("user_message_chunk", "Hello from "),
                        );
                        update(&mut output, session, text("user_message_chunk", "history"));
                        update(
                            &mut output,
                            session,
                            text("agent_thought_chunk", "Thinking it over"),
                        );
                        update(
                            &mut output,
                            session,
                            text("agent_message_chunk", "History answer"),
                        );
                    }
                    Ok(json!({"models": models}))
                }
                "session/prompt" => {
                    let session = params["sessionId"].as_str().unwrap_or_default().to_owned();
                    update(
                        &mut output,
                        &session,
                        text("agent_message_chunk", "Working on it. "),
                    );
                    let mut signed_in = true;
                    if let Some(url) = &login
                        && !state.join("signed-in").exists()
                    {
                        let reply = request(
                            &mut output,
                            &mut input,
                            "turn-sign-in",
                            "session/elicitation",
                            json!({"sessionId":session,"mode":"url","elicitationId":"turn-sign-in-1","url":url,"message":"Sign in to continue"}),
                        );
                        signed_in = accepted(&reply);
                        if signed_in {
                            std::fs::write(state.join("signed-in"), "").unwrap();
                        }
                    }
                    if signed_in {
                        let file = cwd.clone().unwrap_or_default().join("agent-notes.txt");
                        let call = json!({"toolCallId":"call-1","title":"Write agent-notes.txt","kind":"edit","status":"pending","locations":[{"path":file}]});
                        let mut tool = call.clone();
                        tool["sessionUpdate"] = json!("tool_call");
                        update(&mut output, &session, tool);
                        let reply = request(
                            &mut output,
                            &mut input,
                            "permission-1",
                            "session/request_permission",
                            json!({"sessionId":session,"toolCall":call,"options":[{"optionId":"allow","name":"Allow","kind":"allow_once"},{"optionId":"reject","name":"Reject","kind":"reject_once"}]}),
                        );
                        let allowed = reply["result"]["outcome"]["optionId"] == "allow";
                        if allowed {
                            std::fs::write(&file, "written by the registry agent\n").unwrap();
                        }
                        update(
                            &mut output,
                            &session,
                            json!({"sessionUpdate":"tool_call_update","toolCallId":"call-1","status":if allowed {"completed"} else {"failed"}}),
                        );
                        update(&mut output, &session, text("agent_message_chunk", "Done."));
                        Ok(json!({"stopReason":"end_turn"}))
                    } else {
                        Err(json!({"code":-32000,"message":"authentication required"}))
                    }
                }
                "session/set_model"
                | "session/set_config_option"
                | "session/set_mode"
                | "session/close" => Ok(json!({})),
                "session/delete" => {
                    let id = params["sessionId"].as_str().unwrap_or_default();
                    if id == "one" || id == "two" {
                        append(&state, "deleted", &json!(id));
                    }
                    Ok(json!({}))
                }
                "session/list" => {
                    let mut allowed = true;
                    if let Some(url) = &login
                        && !state.join("signed-in").exists()
                    {
                        let reply = request(
                            &mut output,
                            &mut input,
                            "sign-in",
                            "session/elicitation",
                            json!({"sessionId":"probe","mode":"url","elicitationId":"sign-in-1","url":url,"message":"Sign in"}),
                        );
                        allowed = accepted(&reply);
                        if allowed {
                            std::fs::write(state.join("signed-in"), "").unwrap();
                        }
                    }
                    if allowed {
                        let gone = deleted(&state);
                        let sessions: Vec<Value> = ["one", "two"]
                            .into_iter()
                            .filter(|id| !gone.iter().any(|gone| gone.trim_matches('"') == *id))
                            .map(|id| json!({"sessionId":id,"cwd":params["cwd"],"title":format!("Session {id}"),"updatedAt":"2026-11-14T10:00:00.000Z"}))
                            .collect();
                        Ok(json!({"sessions": sessions, "nextCursor": null}))
                    } else {
                        Err(json!({"code":-32000,"message":"authentication required"}))
                    }
                }
                "providers/list" => {
                    let current: Value = std::fs::read_to_string(state.join("provider"))
                        .ok()
                        .and_then(|text| serde_json::from_str(&text).ok())
                        .unwrap_or(Value::Null);
                    Ok(json!({"providers":[
                        {"providerId":"openai","supported":["openai"],"required":false,"current":current},
                        {"providerId":"builtin","supported":["anthropic"],"required":true,"current":null},
                    ]}))
                }
                "providers/set" => {
                    std::fs::write(
                        state.join("provider"),
                        json!({"apiType":params["apiType"],"baseUrl":params["baseUrl"]})
                            .to_string(),
                    )
                    .unwrap();
                    Ok(json!({}))
                }
                "providers/disable" => {
                    let _ = std::fs::remove_file(state.join("provider"));
                    Ok(json!({}))
                }
                "logout" | "auth/logout" => {
                    let _ = std::fs::remove_file(state.join("signed-in"));
                    Ok(json!({}))
                }
                "authenticate" => Ok(json!({})),
                _ => Err(json!({"code":-32601,"message":"unsupported"})),
            };
            let response = match result {
                Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                Err(error) => json!({"jsonrpc":"2.0","id":id,"error":error}),
            };
            send(&mut output, response);
        }
        ExitCode::SUCCESS
    }
}
