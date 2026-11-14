//! Native provider turns end to end: the server builds the Codex and Claude adapters from the
//! provider domain's configured instances and runs them over a real socket.
//!
//! No real provider runs. The instance's `binaryPath` is this test binary: started with
//! `T3_FAKE_PROVIDER` in its environment (set through the instance's own environment
//! variables), it plays the Codex app-server or the Claude CLI from TypeScript's recorded
//! transcripts (`apps/server/src/orchestration-v2/testkit/fixtures`). The provider launch
//! arguments (`app-server -c ...`, `--output-format stream-json ...`) are not libtest's, so the
//! file has its own `main` (`harness = false`).
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

const SECRET: &str = "native-provider-secret-5f1d2c";
const THREAD: &str = "thread:native";

fn main() -> ExitCode {
    if let Ok(kind) = std::env::var("T3_FAKE_PROVIDER") {
        return fake::run(&kind);
    }
    type Case = fn() -> Pin<Box<dyn Future<Output = ()>>>;
    let cases: &[(&str, Case)] = &[
        (
            "codex_turn_streams_asks_approval_and_settles_with_a_checkpoint",
            || Box::pin(provider_turn(Provider::Codex)),
        ),
        (
            "claude_turn_streams_asks_approval_and_settles_with_a_checkpoint",
            || Box::pin(provider_turn(Provider::Claude)),
        ),
        ("codex_process_ending_mid_turn_fails_the_run", || {
            Box::pin(process_ends_mid_turn(Provider::Codex))
        }),
        ("claude_process_ending_mid_turn_fails_the_run", || {
            Box::pin(process_ends_mid_turn(Provider::Claude))
        }),
        (
            "a_turn_above_the_managed_ceiling_never_reaches_the_provider",
            || Box::pin(turn_above_the_ceiling()),
        ),
        (
            "shutdown_with_a_live_session_stops_the_provider_and_fails_its_approval_closed",
            || Box::pin(pending_approval_after_restart()),
        ),
        ("a_disabled_instance_refuses_turns_until_enabled", || {
            Box::pin(disabled_instance())
        }),
        ("a_driver_the_policy_disallows_has_no_turn_adapter", || {
            Box::pin(disallowed_driver())
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
        let result = std::panic::catch_unwind(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(case());
        });
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

fn fixture(scenario: &str, file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../apps/server/src/orchestration-v2/testkit/fixtures")
        .join(scenario)
        .join(file)
}

#[derive(Clone, Copy, PartialEq)]
enum Provider {
    Codex,
    Claude,
}

impl Provider {
    fn instance(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claudeAgent",
        }
    }
    fn model(self) -> &'static str {
        match self {
            Self::Codex => "gpt-6-sol",
            Self::Claude => "claude-sonnet-4-6",
        }
    }
    fn transcript(self) -> PathBuf {
        match self {
            Self::Codex => fixture("tool_call_read_only_on_request", "codex_transcript.ndjson"),
            Self::Claude => fixture("tool_call_read_only_on_request", "claude_transcript.ndjson"),
        }
    }
    /// The first streamed assistant frame: the fake exits there in the mid-turn case.
    fn first_reply_label(self) -> &'static str {
        match self {
            Self::Codex => "item/agentMessage/delta",
            Self::Claude => "assistant",
        }
    }
    /// Text the recorded turn streams before it asks for approval.
    fn streamed_text(self) -> &'static str {
        match self {
            Self::Codex => "write the exact text to the file",
            Self::Claude => "Done. I used `printf`",
        }
    }
}

/// An instance whose binary is this test file playing `provider`, with one sensitive variable.
fn instance(provider: Provider, home: &Path, exit_after: Option<&str>, enabled: bool) -> Value {
    let mut environment = vec![
        json!({"name":"T3_FAKE_PROVIDER","value":provider.instance(),"sensitive":false}),
        json!({"name":"T3_FAKE_TRANSCRIPT","value":provider.transcript(),"sensitive":false}),
        json!({"name":"T3_FAKE_REPORT","value":home.join("reports"),"sensitive":false}),
        json!({"name":"T3_FAKE_SECRET","value":SECRET,"sensitive":true}),
    ];
    if let Some(label) = exit_after {
        environment.push(json!({"name":"T3_FAKE_EXIT_AFTER","value":label,"sensitive":false}));
    }
    json!({
        "driver": provider.instance(),
        "enabled": enabled,
        "environment": environment,
        "config": {
            "binaryPath": std::env::current_exe().unwrap(),
            "homePath": home.join(format!("{}-home", provider.instance())),
        },
    })
}

mod harness {
    use super::*;
    use futures::{SinkExt, StreamExt};
    use t3_rpc::{CallEvent, CallKind, Client, Exit, Frame, RequestId};
    use t3_server::{
        adapters::config::{ConfigInputs, ServerFlags},
        runtime::{RunningServer, StartOptions},
    };
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

    type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

    pub struct Server {
        pub runtime: RunningServer,
        pub home: PathBuf,
        pub workspace: PathBuf,
        pub origin: String,
        pub cookie: String,
        socket: Socket,
        client: Client,
        thread_stream: Option<RequestId>,
        /// Every frame the client received, for the "no secret reaches a client" check.
        pub received: String,
        /// The thread stream's values, in order.
        pub thread: Vec<Value>,
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

    /// The server's environment: enough for a Windows child to start, no PATH (so nothing can
    /// resolve a real provider CLI) and no real home.
    fn environment() -> std::collections::BTreeMap<String, String> {
        ["SystemRoot", "SYSTEMROOT", "windir", "TEMP", "TMP"]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_owned(), value)))
            .collect()
    }

    impl Server {
        /// `settings` is written as `settings.json`; `policy` becomes the managed policy file.
        pub async fn start(home: &Path, settings: &Value, policy: Option<&str>) -> Self {
            std::fs::create_dir_all(home.join("userdata")).unwrap();
            std::fs::create_dir_all(home.join("reports")).unwrap();
            if std::env::var_os("NATIVE_PROVIDERS_TRACE").is_some() {
                eprintln!("home: {}", home.display());
            }
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
            let http = reqwest::Client::new();
            let cookie = http
                .post(format!("{origin}/api/auth/browser-session"))
                .json(&json!({"credential": runtime.pairing_credential}))
                .send()
                .await
                .unwrap()
                .headers()["set-cookie"]
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_owned();
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
                     VALUES ('project:native', 'Native', ?1, '[]', '2026-11-14T12:00:00.000Z', '2026-11-14T12:00:00.000Z')",
                    [workspace.to_string_lossy()],
                )
                .unwrap();
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
                runtime,
                home: home.into(),
                workspace,
                origin,
                cookie,
                socket,
                client: Client::new(0, Default::default()),
                thread_stream: None,
                received: String::new(),
                thread: Vec::new(),
            }
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

        /// Handles one event: thread stream values are recorded; returns a finished call.
        async fn pump(&mut self) -> Option<(RequestId, Exit)> {
            match self.event().await {
                CallEvent::Chunk { id, values } if self.thread_stream.as_ref() == Some(&id) => {
                    self.client.acknowledge(id);
                    if std::env::var_os("NATIVE_PROVIDERS_TRACE").is_some() {
                        for value in &values {
                            eprintln!("stream: {value}");
                        }
                    }
                    self.thread.extend(values);
                    None
                }
                CallEvent::Exit { id, exit } if self.thread_stream.as_ref() == Some(&id) => panic!(
                    "thread stream ended: {}",
                    serde_json::to_value(exit).unwrap()
                ),
                CallEvent::Exit { id, exit } => Some((id, exit)),
                other => panic!("unexpected {other:?}"),
            }
        }

        /// A unary call: `Ok(value)` or `Err(failure json)`.
        pub async fn call(&mut self, method: &str, payload: Value) -> Result<Value, Value> {
            let id = self.client.start(method, payload, CallKind::Unary).unwrap();
            loop {
                if let Some((done, exit)) = self.pump().await
                    && done == id
                {
                    return match exit {
                        Exit::Success { value } => Ok(value.unwrap_or(Value::Null)),
                        other => Err(serde_json::to_value(other).unwrap()),
                    };
                }
            }
        }

        pub async fn dispatch(&mut self, command: Value) -> Result<Value, Value> {
            self.call("orchestration.dispatchCommand", command).await
        }

        pub async fn create_thread(
            &mut self,
            provider: Provider,
            mode: &str,
        ) -> Result<Value, Value> {
            self.dispatch(json!({"type":"thread.create","commandId":"command:thread","threadId":THREAD,"projectId":"project:native","title":"Native thread","modelSelection":{"instanceId":provider.instance(),"model":provider.model()},"runtimeMode":mode,"interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"})).await
        }

        pub fn subscribe(&mut self) {
            self.thread_stream = Some(
                self.client
                    .start(
                        "orchestration.subscribeThread",
                        json!({"threadId": THREAD}),
                        CallKind::Stream,
                    )
                    .unwrap(),
            );
        }

        pub async fn send(&mut self, id: &str, text: &str) -> Result<Value, Value> {
            self.dispatch(json!({"type":"message.dispatch","commandId":format!("command:{id}"),"threadId":THREAD,"messageId":format!("message:{id}"),"text":text,"attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"})).await
        }

        /// Pumps the socket until `found` returns a value from the thread stream so far.
        pub async fn until<T>(&mut self, found: impl Fn(&[Value]) -> Option<T>) -> T {
            loop {
                if let Some(value) = found(&self.thread) {
                    return value;
                }
                if let Some((id, exit)) = self.pump().await {
                    panic!(
                        "unexpected exit {id:?}: {}",
                        serde_json::to_value(exit).unwrap()
                    );
                }
            }
        }

        pub async fn snapshot(&self) -> Value {
            reqwest::Client::new()
                .get(format!(
                    "{}/api/orchestration/threads/{THREAD}",
                    self.origin
                ))
                .header("cookie", &self.cookie)
                .header("x-t3-orchestration-protocol", "2")
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap()
        }

        pub async fn stop(mut self) -> (PathBuf, String) {
            let _ = self.socket.close(None).await;
            self.runtime.shutdown().await.unwrap();
            (self.home, self.received)
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

    /// The latest record of the thread's run, by its status.
    pub fn run_status(values: &[Value]) -> Option<String> {
        objects(values, &|value| {
            value["userMessageId"].is_string()
                && value["rootNodeId"].is_string()
                && value["status"].is_string()
        })
        .last()
        .map(|run| run["status"].as_str().unwrap().to_owned())
    }

    /// Provider launches the fake recorded (one report per process).
    pub fn launches(home: &Path) -> Vec<Value> {
        std::fs::read_dir(home.join("reports"))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.path().extension().is_some_and(|e| e == "json"))
                    .map(|entry| {
                        serde_json::from_str(&std::fs::read_to_string(entry.path()).unwrap())
                            .unwrap()
                    })
                    .collect()
            })
            .unwrap_or_default()
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

use harness::{Server, launches, objects, run_status};

fn settings_with(instances: Value) -> Value {
    json!({ "providerInstances": instances })
}

/// A message runs a native turn: the provider process starts with the instance's binary,
/// home and environment; the turn streams; the provider's approval request reaches the client,
/// which answers it; the run settles with a Git checkpoint. The sensitive instance variable
/// reaches the provider process and nothing else.
async fn provider_turn(provider: Provider) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().to_path_buf();
    if std::env::var_os("NATIVE_PROVIDERS_TRACE").is_some() {
        std::mem::forget(temp);
    }
    let settings =
        settings_with(json!({ provider.instance(): instance(provider, &home, None, true) }));
    let mut server = Server::start(&home, &settings, None).await;
    server
        .create_thread(provider, "approval-required")
        .await
        .unwrap();
    server.subscribe();
    server
        .send("first", "Write the fixture file.")
        .await
        .unwrap();

    // The approval request, after streamed text (Codex streams before asking; Claude asks first).
    let request = server
        .until(|values| {
            objects(values, &|value| {
                value["responseCapability"].is_object() && value["status"] == "pending"
            })
            .first()
            .map(|record| record["id"].as_str().unwrap().to_owned())
        })
        .await;
    let snapshot = server.snapshot().await;
    assert_eq!(
        run_status(std::slice::from_ref(&snapshot)).as_deref(),
        Some("running"),
        "{snapshot}"
    );
    server
        .dispatch(json!({"type":"runtime-request.respond","commandId":"command:approve","threadId":THREAD,"requestId":request,"decision":"accept"}))
        .await
        .unwrap();
    server
        .until(|values| (run_status(values).as_deref() == Some("completed")).then_some(()))
        .await;
    let streamed = Value::Array(server.thread.clone()).to_string();
    assert!(streamed.contains(provider.streamed_text()), "{streamed}");
    let answered = objects(&server.thread, &|value| {
        value["responseCapability"].is_object() && value["id"] == request.as_str()
    });
    assert!(
        answered.iter().any(|item| item["status"] != "pending"),
        "{answered:?}"
    );

    // The settled run's checkpoint is a real commit in the workspace repository.
    let snapshot = server.snapshot().await;
    let checkpoints = objects(std::slice::from_ref(&snapshot), &|value| {
        value["status"] == "ready" && value["ref"].is_string()
    });
    assert!(!checkpoints.is_empty(), "{snapshot}");
    for checkpoint in &checkpoints {
        harness::git(
            &server.workspace,
            &[
                "rev-parse",
                "--verify",
                &format!("{}^{{commit}}", checkpoint["ref"].as_str().unwrap()),
            ],
        );
    }

    // The provider process got the instance's home and its secret; no client frame did.
    let launches = launches(&home);
    assert_eq!(launches.len(), 1, "{launches:?}");
    assert_eq!(launches[0]["secret"], true, "{launches:?}");
    assert_eq!(
        launches[0]["home"],
        json!(home.join(format!("{}-home", provider.instance()))),
        "{launches:?}"
    );
    let (home, received) = server.stop().await;
    assert!(!received.contains(SECRET));
    assert!(!snapshot.to_string().contains(SECRET));
    for entry in std::fs::read_dir(home.join("userdata")).unwrap() {
        let path = entry.unwrap().path();
        if path.to_string_lossy().contains("statev2")
            || path.extension().is_some_and(|e| e == "sqlite")
        {
            let bytes = std::fs::read(&path).unwrap();
            assert!(
                !bytes
                    .windows(SECRET.len())
                    .any(|window| window == SECRET.as_bytes()),
                "{path:?}"
            );
        }
    }
}

/// The provider process exits during the turn: the run settles as failed with an error item,
/// and nothing waits forever.
async fn process_ends_mid_turn(provider: Provider) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().to_path_buf();
    if std::env::var_os("NATIVE_PROVIDERS_TRACE").is_some() {
        std::mem::forget(temp);
    }
    let settings = settings_with(json!({
        provider.instance(): instance(provider, &home, Some(provider.first_reply_label()), true)
    }));
    let mut server = Server::start(&home, &settings, None).await;
    server
        .create_thread(provider, "approval-required")
        .await
        .unwrap();
    server.subscribe();
    server
        .send("first", "Write the fixture file.")
        .await
        .unwrap();
    let status = server
        .until(|values| {
            run_status(values)
                .filter(|status| !matches!(status.as_str(), "queued" | "starting" | "running"))
        })
        .await;
    assert_eq!(status, "failed", "{:?}", server.thread);
    let errors = objects(&server.thread, &|value| {
        value["type"] == "error" && value["runId"].is_string()
    });
    assert!(!errors.is_empty(), "{:?}", server.thread);
    assert_eq!(launches(&home).len(), 1);
    server.stop().await;
}

/// Whether a process with this id still runs.
fn alive(pid: u64) -> bool {
    if cfg!(windows) {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
    } else {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .is_ok_and(|status| status.success())
    }
}

/// Shutdown with a live provider session (review L6): the server stops within a bound, the
/// provider process is gone, and the approval still pending at shutdown is never approved
/// later: after the restart it is no longer pending, answering it is refused, and no provider
/// is relaunched.
async fn pending_approval_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().to_path_buf();
    if std::env::var_os("NATIVE_PROVIDERS_TRACE").is_some() {
        std::mem::forget(temp);
    }
    let settings = settings_with(json!({ "codex": instance(Provider::Codex, &home, None, true) }));
    let mut server = Server::start(&home, &settings, None).await;
    server
        .create_thread(Provider::Codex, "approval-required")
        .await
        .unwrap();
    server.subscribe();
    server
        .send("first", "Write the fixture file.")
        .await
        .unwrap();
    let request = server
        .until(|values| {
            objects(values, &|value| {
                value["responseCapability"].is_object() && value["status"] == "pending"
            })
            .first()
            .map(|record| record["id"].as_str().unwrap().to_owned())
        })
        .await;
    let pid = launches(&home)[0]["pid"].as_u64().unwrap();
    assert!(alive(pid));
    // The bound: a shutdown that waits on a closed provider domain or a lost child hangs.
    tokio::time::timeout(std::time::Duration::from_secs(60), server.stop())
        .await
        .expect("shutdown with a live provider session finishes");
    assert!(!alive(pid), "provider process {pid} outlived the server");

    let mut server = Server::start(&home, &settings, None).await;
    let snapshot = server.snapshot().await;
    let requests = objects(std::slice::from_ref(&snapshot), &|value| {
        value["responseCapability"].is_object() && value["id"] == request.as_str()
    });
    assert!(!requests.is_empty(), "{snapshot}");
    assert!(
        requests.iter().all(|item| item["status"] != "pending"),
        "{requests:?}"
    );
    let refused = server
        .dispatch(json!({"type":"runtime-request.respond","commandId":"command:late","threadId":THREAD,"requestId":request,"decision":"accept"}))
        .await;
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(launches(&home).len(), 1);
    server.stop().await;
}

/// A thread created at full access before the organisation's policy existed: after a restart
/// under a lower ceiling, its turn is refused with TypeScript's text and no provider starts.
async fn turn_above_the_ceiling() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().to_path_buf();
    if std::env::var_os("NATIVE_PROVIDERS_TRACE").is_some() {
        std::mem::forget(temp);
    }
    let settings = settings_with(json!({ "codex": instance(Provider::Codex, &home, None, true) }));
    let mut server = Server::start(&home, &settings, None).await;
    server
        .create_thread(Provider::Codex, "full-access")
        .await
        .unwrap();
    server.stop().await;

    let mut server = Server::start(
        &home,
        &settings,
        Some(r#"{"schemaVersion":1,"organization":"Acme","limits":{"maxRuntimeMode":"auto-accept-edits"}}"#),
    )
    .await;
    let refused = server
        .send("first", "Write the fixture file.")
        .await
        .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("Acme limits threads to Auto-accept edits."),
        "{refused}"
    );
    assert!(launches(&home).is_empty());
    server.stop().await;
}

/// A disabled instance: a message to its thread is refused with TypeScript's error
/// (`ThreadManagementService.assertProviderInstanceEnabled` on `message.dispatch`) and the
/// provider never starts; after the instance is enabled, the same thread runs a turn.
async fn disabled_instance() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().to_path_buf();
    let settings = settings_with(json!({ "codex": instance(Provider::Codex, &home, None, false) }));
    let mut server = Server::start(&home, &settings, None).await;
    server
        .create_thread(Provider::Codex, "approval-required")
        .await
        .unwrap();
    let refused = server
        .send("first", "Write the fixture file.")
        .await
        .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains(r#"Provider instance \"codex\" is disabled for project project:native."#),
        "{refused}"
    );
    assert!(launches(&home).is_empty());

    server
        .call(
            "server.updateSettings",
            json!({"patch":{"providerInstances":{"codex":instance(Provider::Codex, &home, None, true)}}}),
        )
        .await
        .unwrap();
    server.subscribe();
    server
        .send("second", "Write the fixture file.")
        .await
        .unwrap();
    server
        .until(|values| {
            objects(values, &|value| {
                value["responseCapability"].is_object() && value["status"] == "pending"
            })
            .first()
            .map(|_| ())
        })
        .await;
    assert_eq!(launches(&home).len(), 1);
    server.stop().await;
}

/// The policy's `allowedDrivers` leaves Codex out: its instance is unavailable, so a message on
/// a Codex thread is refused with TypeScript's registry lookup error and no Codex process starts.
async fn disallowed_driver() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().to_path_buf();
    let settings = settings_with(json!({ "codex": instance(Provider::Codex, &home, None, true) }));
    let mut server = Server::start(
        &home,
        &settings,
        Some(r#"{"schemaVersion":1,"organization":"Acme","limits":{"providers":{"allowedDrivers":["claudeAgent"]}}}"#),
    )
    .await;
    server
        .create_thread(Provider::Codex, "approval-required")
        .await
        .unwrap();
    let refused = server
        .send("first", "Write the fixture file.")
        .await
        .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("No orchestration provider adapter is registered for codex."),
        "{refused}"
    );
    assert!(launches(&home).is_empty());
    server.stop().await;
}

mod fake {
    //! The fake provider process: replays one recorded transcript over stdio.
    use serde_json::{Value, json};
    use std::{
        collections::{BTreeMap, VecDeque},
        io::{BufRead, Write},
        process::ExitCode,
    };

    struct Wire {
        input: std::io::Lines<std::io::StdinLock<'static>>,
        output: std::io::Stdout,
        log: std::fs::File,
    }
    impl Wire {
        fn read(&mut self) -> Option<Value> {
            loop {
                let line = self.input.next()?.ok()?;
                if line.trim().is_empty() {
                    continue;
                }
                let _ = writeln!(self.log, "<- {line}");
                return serde_json::from_str(&line).ok();
            }
        }
        fn write(&mut self, value: &Value) {
            let _ = writeln!(self.log, "-> {value}");
            let mut output = self.output.lock();
            writeln!(output, "{value}").unwrap();
            output.flush().unwrap();
        }
    }

    pub fn run(kind: &str) -> ExitCode {
        let transcript = std::env::var("T3_FAKE_TRANSCRIPT").unwrap();
        let steps: Vec<Value> = std::fs::read_to_string(transcript)
            .unwrap()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let exit_after = std::env::var("T3_FAKE_EXIT_AFTER").ok();
        let report = std::path::Path::new(&std::env::var("T3_FAKE_REPORT").unwrap())
            .join(format!("{}.json", std::process::id()));
        let home = match kind {
            "codex" => std::env::var("CODEX_HOME").ok(),
            _ => std::env::var("CLAUDE_CONFIG_DIR").ok(),
        };
        std::fs::write(
            report,
            json!({
                "secret": std::env::var("T3_FAKE_SECRET").ok().as_deref() == Some(super::SECRET),
                "home": home,
                "pid": std::process::id(),
            })
            .to_string(),
        )
        .unwrap();
        let log = std::path::Path::new(&std::env::var("T3_FAKE_REPORT").unwrap())
            .join(format!("{}.wire.log", std::process::id()));
        let mut wire = Wire {
            input: std::io::stdin().lock().lines(),
            output: std::io::stdout(),
            log: std::fs::File::create(log).unwrap(),
        };
        match kind {
            "codex" => codex(&mut wire, &steps, exit_after.as_deref()),
            _ => claude(&mut wire, &steps, exit_after.as_deref()),
        }
        ExitCode::SUCCESS
    }

    /// Codex app-server JSON-RPC: requests are answered from the transcript's responses for the
    /// same method, in order; notifications and server requests are emitted where recorded.
    fn codex(wire: &mut Wire, steps: &[Value], exit_after: Option<&str>) {
        let mut responses: BTreeMap<String, VecDeque<Value>> = BTreeMap::new();
        for step in steps {
            let frame = &step["frame"];
            if step["type"] == "emit_inbound"
                && frame.get("method").is_none()
                && frame.get("id").is_some()
            {
                responses
                    .entry(step["label"].as_str().unwrap().to_owned())
                    .or_default()
                    .push_back(frame.clone());
            }
        }
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        let mut needed: BTreeMap<String, usize> = BTreeMap::new();
        let mut answer = |wire: &mut Wire, message: &Value, seen: &mut BTreeMap<String, usize>| {
            let Some(method) = message["method"].as_str() else {
                return;
            };
            *seen.entry(method.to_owned()).or_default() += 1;
            if message.get("id").is_some() {
                let mut reply = responses
                    .get_mut(method)
                    .and_then(VecDeque::pop_front)
                    .unwrap_or_else(|| json!({"result": {}}));
                reply["id"] = message["id"].clone();
                wire.write(&reply);
            }
        };
        for step in steps {
            let frame = &step["frame"];
            match step["type"].as_str() {
                Some("expect_outbound") => {
                    if let Some(method) = frame["method"].as_str() {
                        let count = needed.entry(method.to_owned()).or_default();
                        *count += 1;
                        let count = *count;
                        while seen.get(method).copied().unwrap_or(0) < count {
                            let Some(message) = wire.read() else { return };
                            answer(wire, &message, &mut seen);
                        }
                    } else {
                        // The client's answer to a server request.
                        loop {
                            let Some(message) = wire.read() else { return };
                            if message.get("method").is_none() && message["id"] == frame["id"] {
                                break;
                            }
                            answer(wire, &message, &mut seen);
                        }
                    }
                }
                Some("emit_inbound") if frame.get("method").is_some() => {
                    let mut frame = frame.clone();
                    if let Some(object) = frame.as_object_mut() {
                        object.remove("emittedAtMs");
                    }
                    wire.write(&frame);
                    if exit_after == step["label"].as_str() {
                        std::process::exit(7);
                    }
                }
                _ => {}
            }
        }
        while let Some(message) = wire.read() {
            answer(wire, &message, &mut seen);
        }
    }

    /// Claude CLI stream-json: the transcript records the SDK's view (`query.open`,
    /// `prompt.offer`, `permission.request`); the CLI's side of each is its control protocol.
    fn claude(wire: &mut Wire, steps: &[Value], exit_after: Option<&str>) {
        let session = session_argument();
        let mut tool_use = String::from("toolu_fixture");
        let mut permissions = 0;
        let mut pending_permission = None::<String>;
        let respond = |wire: &mut Wire, message: &Value| {
            if message["type"] == "control_request" {
                wire.write(&json!({"type":"control_response","response":{"subtype":"success","request_id":message["request_id"],"response":{}}}));
            }
        };
        for step in steps {
            let frame = &step["frame"];
            match (step["type"].as_str(), frame["type"].as_str()) {
                (Some("expect_outbound"), Some("query.open")) => loop {
                    let Some(message) = wire.read() else { return };
                    let initialize = message["request"]["subtype"] == "initialize";
                    respond(wire, &message);
                    if initialize {
                        break;
                    }
                },
                (Some("expect_outbound"), Some("prompt.offer")) => loop {
                    let Some(message) = wire.read() else { return };
                    if message["type"] == "user" {
                        break;
                    }
                    respond(wire, &message);
                },
                (Some("expect_outbound"), Some("permission.response")) => loop {
                    let Some(message) = wire.read() else { return };
                    if message["type"] == "control_response"
                        && message["response"]["request_id"].as_str()
                            == pending_permission.as_deref()
                    {
                        break;
                    }
                    respond(wire, &message);
                },
                (Some("emit_inbound"), Some("permission.request")) => {
                    permissions += 1;
                    let id = format!("permission-{permissions}");
                    wire.write(&json!({"type":"control_request","request_id":id,"request":{
                        "subtype":"can_use_tool",
                        "tool_name":frame["toolName"],
                        "input":frame["input"],
                        "tool_use_id":tool_use,
                        "permission_suggestions":frame["options"]["suggestions"],
                    }}));
                    pending_permission = Some(id);
                }
                (Some("emit_inbound"), _) => {
                    let mut frame = frame.clone();
                    if let Some(session) = &session {
                        replace_session(&mut frame, session);
                    }
                    if let Some(id) = frame["message"]["content"]
                        .as_array()
                        .and_then(|content| {
                            content.iter().find(|block| block["type"] == "tool_use")
                        })
                        .and_then(|block| block["id"].as_str())
                    {
                        tool_use = id.to_owned();
                    }
                    wire.write(&frame);
                    if exit_after == step["label"].as_str() {
                        std::process::exit(7);
                    }
                }
                _ => {}
            }
        }
        while let Some(message) = wire.read() {
            respond(wire, &message);
        }
    }

    fn session_argument() -> Option<String> {
        let arguments: Vec<String> = std::env::args().collect();
        for (index, argument) in arguments.iter().enumerate() {
            for flag in ["--session-id", "--resume"] {
                if let Some(value) = argument.strip_prefix(&format!("{flag}=")) {
                    return Some(value.to_owned());
                }
                if argument == flag {
                    return arguments.get(index + 1).cloned();
                }
            }
        }
        None
    }

    fn replace_session(value: &mut Value, session: &str) {
        match value {
            Value::Object(map) => {
                for (key, child) in map.iter_mut() {
                    if key == "session_id" && child.is_string() {
                        *child = Value::String(session.to_owned());
                    } else {
                        replace_session(child, session);
                    }
                }
            }
            Value::Array(items) => items
                .iter_mut()
                .for_each(|child| replace_session(child, session)),
            _ => {}
        }
    }
}
