//! Provider-native text generation over a real socket: thread titles and commit messages through
//! the Codex and Claude instances (no HTTP generator configured), and review guides through an
//! isolated checkout.
//!
//! No real provider runs. Each instance's `binaryPath` is this test binary: with
//! `T3_FAKE_TEXTGEN` in its environment it plays `codex exec` / `codex mcp list` or `claude -p`,
//! reports its arguments, working directory, prompt and selected environment over a loopback
//! socket the test listens on, and answers as `T3_FAKE_MODE_FILE` says (`reply`, `fail`, `hang`).
//! The repositories are temporary; the "fork" is served by `git daemon` on loopback.
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

const REPLY: &str = r#"{"title":"Fix Pairing Expiry","needsRefinement":false,"subject":"Add greeting.","body":"Generated body","stops":[{"title":"Overview","summary":"What changed.","attention":null,"excerpts":[],"files":[],"notes":[],"questions":[]}]}"#;

fn main() -> ExitCode {
    if let Ok(kind) = std::env::var("T3_FAKE_TEXTGEN") {
        return fake::run(&kind);
    }
    type Case = fn() -> Pin<Box<dyn Future<Output = ()>>>;
    let cases: &[(&str, Case)] = &[
        (
            "titles_and_commit_messages_run_through_codex_by_default",
            || Box::pin(codex_by_default()),
        ),
        (
            "claude_writes_titles_without_tools_in_an_empty_directory",
            || Box::pin(claude_titles_and_commits()),
        ),
        (
            "review_guides_check_out_in_isolation_and_leave_nothing_behind",
            || Box::pin(review_guides()),
        ),
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
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut failed = 0;
    for (name, case) in &selected {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            runtime.block_on(case());
        }));
        match result {
            Ok(()) => println!("test {name} ... ok"),
            Err(_) => {
                failed += 1;
                println!("test {name} ... FAILED");
            }
        }
    }
    // A failed case can leave its server's tasks running; do not wait for them.
    runtime.shutdown_timeout(std::time::Duration::from_secs(5));
    println!(
        "test result: {}. {} passed; {failed} failed",
        if failed == 0 { "ok" } else { "FAILED" },
        selected.len() - failed
    );
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

mod fake {
    //! The fake provider CLI.
    use super::*;
    use std::io::{Read, Write};

    fn report(value: &Value) {
        if let Ok(address) = std::env::var("T3_FAKE_NOTIFY")
            && let Ok(mut stream) = std::net::TcpStream::connect(address)
        {
            let _ = writeln!(stream, "{value}");
        }
    }

    fn mode() -> String {
        std::env::var("T3_FAKE_MODE_FILE")
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|mode| mode.trim().to_owned())
            .unwrap_or_else(|| "reply".into())
    }

    pub fn run(kind: &str) -> ExitCode {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let cwd = std::env::current_dir().unwrap_or_default();
        if kind == "codex"
            && args.iter().any(|arg| arg == "mcp")
            && args.iter().any(|arg| arg == "list")
        {
            print!(r#"[{{"name":"docs","enabled":true}},{{"name":"off","enabled":false}}]"#);
            return ExitCode::SUCCESS;
        }
        let one_shot = match kind {
            "codex" => args.first().is_some_and(|arg| arg == "exec"),
            _ => args.iter().any(|arg| arg == "-p"),
        };
        if !one_shot {
            return ExitCode::FAILURE;
        }
        let mut prompt = String::new();
        let _ = std::io::stdin().read_to_string(&mut prompt);
        let mode = mode();
        report(&json!({
            "kind": kind,
            "args": args,
            "cwd": cwd,
            "prompt": prompt,
            "codexHome": std::env::var("CODEX_HOME").ok(),
            "secret": std::env::var("T3_FAKE_SECRET").ok(),
            "mode": mode,
        }));
        match mode.as_str() {
            "fail" => return ExitCode::FAILURE,
            "hang" => std::thread::sleep(std::time::Duration::from_secs(600)),
            _ => {}
        }
        if kind == "codex" {
            let output = args
                .iter()
                .position(|arg| arg == "--output-last-message")
                .and_then(|index| args.get(index + 1))
                .expect("output path");
            std::fs::write(output, REPLY).unwrap();
        } else {
            let reply: Value = serde_json::from_str(REPLY).unwrap();
            print!("{}", json!({"type":"result","structured_output": reply}));
        }
        ExitCode::SUCCESS
    }
}

mod harness {
    use super::*;
    use futures::{SinkExt, StreamExt};
    use t3_rpc::{CallEvent, CallKind, Client, Exit, Frame, RequestId};
    use t3_server::adapters::config::{ConfigInputs, ServerFlags};
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

    type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

    pub fn git(cwd: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(["-c", "user.name=t3", "-c", "user.email=t3@example.invalid"])
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", cwd.join(".no-global-config"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    pub fn repository(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        git(path, &["init", "-q", "-b", "main"]);
        git(path, &["config", "user.name", "t3"]);
        git(path, &["config", "user.email", "t3@example.invalid"]);
        git(path, &["config", "core.autocrlf", "false"]);
        std::fs::write(path.join("hello.txt"), "hello\n").unwrap();
        git(path, &["add", "."]);
        git(path, &["commit", "-q", "-m", "init"]);
    }

    /// The directories of the real `git` (Git for Windows' `mingw64\bin` also needs `usr\bin`).
    fn git_directories() -> Vec<PathBuf> {
        let name = if cfg!(windows) { "git.exe" } else { "git" };
        let directory = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .find(|directory| directory.join(name).is_file())
            .expect("git on PATH");
        let mut directories = vec![directory.clone()];
        if let Some(root) = directory
            .parent()
            .filter(|parent| {
                parent
                    .file_name()
                    .is_some_and(|n| n.eq_ignore_ascii_case("mingw64"))
            })
            .and_then(Path::parent)
        {
            directories.push(root.join("usr").join("bin"));
        }
        directories
    }

    /// Reports from fake provider processes, one JSON object per launch.
    pub struct Reports {
        pub address: String,
        receiver: tokio::sync::mpsc::UnboundedReceiver<Value>,
    }
    impl Reports {
        pub async fn start() -> Self {
            use tokio::io::AsyncBufReadExt;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap().to_string();
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let sender = sender.clone();
                    tokio::spawn(async move {
                        let mut lines = tokio::io::BufReader::new(stream).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            if let Ok(value) = serde_json::from_str(&line) {
                                let _ = sender.send(value);
                            }
                        }
                    });
                }
            });
            Self { address, receiver }
        }
        pub async fn next(&mut self) -> Value {
            self.receiver.recv().await.expect("report")
        }
    }

    pub fn instance(driver: &str, home: &Path, reports: &str, mode: &Path) -> Value {
        json!({
            "driver": driver,
            "enabled": true,
            "environment": [
                {"name":"T3_FAKE_TEXTGEN","value": if driver == "codex" { "codex" } else { "claude" },"sensitive":false},
                {"name":"T3_FAKE_NOTIFY","value":reports,"sensitive":false},
                {"name":"T3_FAKE_MODE_FILE","value":mode,"sensitive":false},
                {"name":"T3_FAKE_SECRET","value":"textgen-secret-1b2c","sensitive":true},
            ],
            "config": {
                "binaryPath": std::env::current_exe().unwrap(),
                "homePath": home.join(format!("{driver}-home")),
            },
        })
    }

    pub struct Server {
        pub temp: tempfile::TempDir,
        pub runtime: Option<t3_server::runtime::RunningServer>,
        pub cleanups: tokio::sync::mpsc::UnboundedReceiver<String>,
        socket: Socket,
        client: Client,
        chunks: Vec<(RequestId, Value)>,
        exits: Vec<(RequestId, Exit)>,
    }

    impl Server {
        pub async fn start(
            temp: tempfile::TempDir,
            settings: Value,
            binder: Option<std::sync::Arc<dyn t3_server::adapters::pull_requests::ProjectBinder>>,
        ) -> Self {
            let home = temp.path().to_path_buf();
            std::fs::create_dir_all(home.join("userdata")).unwrap();
            std::fs::write(home.join("userdata/settings.json"), settings.to_string()).unwrap();
            let path = std::env::join_paths(git_directories()).unwrap();
            let env = [
                "SystemRoot",
                "SYSTEMROOT",
                "windir",
                "TEMP",
                "TMP",
                "ComSpec",
            ]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_owned(), value)))
            .chain([
                ("PATH".to_owned(), path.to_string_lossy().into_owned()),
                ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
                (
                    "GIT_CONFIG_GLOBAL".into(),
                    home.join(".no-global-config")
                        .to_string_lossy()
                        .into_owned(),
                ),
                ("GIT_TERMINAL_PROMPT".into(), "0".into()),
                ("GIT_AUTHOR_NAME".into(), "t3".into()),
                ("GIT_AUTHOR_EMAIL".into(), "t3@example.invalid".into()),
                ("GIT_COMMITTER_NAME".into(), "t3".into()),
                ("GIT_COMMITTER_EMAIL".into(), "t3@example.invalid".into()),
            ])
            .collect();
            let (cleanups_sender, cleanups) = tokio::sync::mpsc::unbounded_channel();
            let runtime = t3_server::runtime::start_with(
                ConfigInputs {
                    flags: ServerFlags {
                        base_dir: Some(home.to_string_lossy().into_owned()),
                        host: Some("127.0.0.1".into()),
                        port: Some(0),
                        ..Default::default()
                    },
                    env,
                    home: home.clone(),
                    process_cwd: home.clone(),
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
                    provider_effects: Some(effects::provider_effects()),
                    dev_replay: true,
                    pull_request_binder: binder,
                    review_guide_cleanups: Some(cleanups_sender),
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
                runtime: Some(runtime),
                cleanups,
                socket,
                client: Client::new(0, Default::default()),
                chunks: Vec::new(),
                exits: Vec::new(),
            }
        }
        pub fn home(&self) -> &Path {
            self.temp.path()
        }
        pub fn database(&self) -> t3_persistence::rusqlite::Connection {
            let layout = t3_config::home::HomeLayout {
                base_dir: self.home().into(),
                state_dir: self.home().join("userdata"),
            };
            t3_persistence::rusqlite::Connection::open(layout.database()).unwrap()
        }
        async fn flush(&mut self) {
            while let Some(message) = self.client.pop_outgoing() {
                let Frame::Text(text) = t3_rpc::codec::encode(&message).unwrap() else {
                    panic!("text frame expected");
                };
                self.socket.send(Message::Text(text.into())).await.unwrap();
            }
        }
        async fn pump(&mut self) {
            loop {
                self.flush().await;
                if let Some(event) = self.client.pop_event() {
                    match event {
                        CallEvent::Chunk { id, values } => {
                            self.client.acknowledge(id.clone());
                            self.chunks
                                .extend(values.into_iter().map(|value| (id.clone(), value)));
                        }
                        CallEvent::Exit { id, exit } => self.exits.push((id, exit)),
                        other => panic!("unexpected {other:?}"),
                    }
                    return;
                }
                let frame = match self.socket.next().await {
                    Some(Ok(Message::Text(text))) => Frame::Text(text.to_string()),
                    Some(Ok(Message::Binary(bytes))) => Frame::Binary(bytes.to_vec()),
                    Some(Ok(Message::Ping(payload))) => {
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
        pub fn subscribe(&mut self, tag: &str, payload: Value) -> RequestId {
            self.client.start(tag, payload, CallKind::Stream).unwrap()
        }
        pub async fn next(&mut self, id: &RequestId) -> Result<Value, Exit> {
            loop {
                if let Some(index) = self.chunks.iter().position(|(chunk, _)| chunk == id) {
                    return Ok(self.chunks.remove(index).1);
                }
                if let Some(index) = self.exits.iter().position(|(exit, _)| exit == id) {
                    return Err(self.exits.remove(index).1);
                }
                self.pump().await;
            }
        }
        pub async fn call(&mut self, tag: &str, payload: Value) -> Exit {
            let id = self.client.start(tag, payload, CallKind::Unary).unwrap();
            loop {
                if let Some(index) = self.exits.iter().position(|(exit, _)| exit == &id) {
                    return self.exits.remove(index).1;
                }
                self.pump().await;
            }
        }
        pub async fn drain(&mut self, id: &RequestId) -> (Vec<Value>, Exit) {
            let mut values = Vec::new();
            loop {
                match self.next(id).await {
                    Ok(value) => values.push(value),
                    Err(exit) => return (values, exit),
                }
            }
        }
        pub async fn create_project(&mut self, id: &str, root: &Path) {
            ok(self
                .call(
                    "projects.mutate",
                    json!({"type":"project.create","commandId":format!("command:{id}"),"projectId":id,"title":id,"workspaceRoot":root.to_string_lossy()}),
                )
                .await);
        }
        /// A Replay (dev) thread whose first message seeds the title; resolves once the
        /// generated title is on the shell stream.
        pub async fn title_thread(&mut self, project: &str, thread: &str, text: &str, title: &str) {
            ok(self.call(
                "orchestration.dispatchCommand",
                json!({"type":"thread.create","commandId":format!("command:{thread}"),"threadId":thread,"projectId":project,"title":"New thread","modelSelection":{"instanceId":"devReplay","model":"multi-turn"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"}),
            )
            .await);
            let shell = self.subscribe("orchestration.subscribeShell", json!({}));
            ok(self.call(
                "orchestration.dispatchCommand",
                json!({"type":"message.dispatch","commandId":format!("command:{thread}:message"),"threadId":thread,"messageId":format!("message:{thread}"),"text":text,"attachments":[],"titleSeed":"Seed title","dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"}),
            )
            .await);
            loop {
                let value = self.next(&shell).await.expect("shell stream ended");
                if value.to_string().contains(title) {
                    break;
                }
            }
        }
        pub async fn shutdown(&mut self) {
            self.runtime.take().unwrap().shutdown().await.unwrap();
        }
    }

    pub fn ok(exit: Exit) -> Value {
        match exit {
            Exit::Success { value } => value.unwrap_or(Value::Null),
            other => panic!(
                "expected success, got {}",
                serde_json::to_value(other).unwrap()
            ),
        }
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

use harness::{Reports, Server, git, instance, ok, repository};

fn args(report: &Value) -> Vec<String> {
    report["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap().to_owned())
        .collect()
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap()
}

/// No HTTP generator: the default title model (Codex) and the commit writer run `codex exec`
/// read-only, with plugins, apps and every enabled MCP server off, the prompt on stdin.
async fn codex_by_default() {
    let temp = tempfile::tempdir().unwrap();
    let mut reports = Reports::start().await;
    let mode = temp.path().join("mode.txt");
    std::fs::write(&mode, "reply").unwrap();
    let settings = json!({"providerInstances": {"codex": instance("codex", temp.path(), &reports.address, &mode)}});
    let mut s = Server::start(temp, settings, None).await;
    let repo = s.home().join("work").join("app");
    repository(&repo);
    s.create_project("project:app", &repo).await;

    s.title_thread(
        "project:app",
        "thread:codex",
        "Fix pairing expiry and preserve remote access.",
        "Fix Pairing Expiry",
    )
    .await;
    let report = reports.next().await;
    assert_eq!(report["kind"], "codex");
    let argv = args(&report);
    assert_eq!(argv[0], "exec");
    for expected in [
        ["-s", "read-only"],
        ["--config", "features.plugins=false"],
        ["--config", "features.apps=false"],
        ["--config", "mcp_servers.docs.enabled=false"],
        ["--config", "model_reasoning_effort=\"low\""],
    ] {
        assert!(
            argv.windows(2).any(|pair| pair == expected),
            "{expected:?} missing from {argv:?}"
        );
    }
    assert!(argv.iter().any(|arg| arg == "--ephemeral"));
    assert!(argv.iter().any(|arg| arg == "--skip-git-repo-check"));
    assert!(
        argv.iter()
            .any(|arg| arg.contains("mcp_servers={\"docs\"={enabled=false}}"))
    );
    assert!(!argv.iter().any(|arg| arg.contains("\"off\"")));
    assert_eq!(argv.last().map(String::as_str), Some("-"));
    assert!(
        !argv
            .iter()
            .any(|arg| arg.contains("dangerously") || arg == "--full-auto")
    );
    assert!(
        report["prompt"]
            .as_str()
            .unwrap()
            .contains("Fix pairing expiry and preserve remote access.")
    );
    // The instance's home, as configured (the fake never creates it).
    let codex_home = PathBuf::from(report["codexHome"].as_str().unwrap());
    assert_eq!(codex_home.file_name().unwrap(), "codex-home");
    assert_eq!(canonical(codex_home.parent().unwrap()), canonical(s.home()));
    assert_eq!(report["secret"], "textgen-secret-1b2c");

    // The commit writer (no writer model set) is the same Codex instance, in the checkout.
    std::fs::write(repo.join("greeting.txt"), "hi\n").unwrap();
    let action = s.subscribe(
        "git.runStackedAction",
        json!({"actionId":"action:codex","cwd":repo.to_string_lossy(),"action":"commit"}),
    );
    let (_, exit) = s.drain(&action).await;
    ok(exit);
    let report = reports.next().await;
    assert_eq!(report["kind"], "codex");
    assert_eq!(
        canonical(Path::new(report["cwd"].as_str().unwrap())),
        canonical(&repo)
    );
    assert!(report["prompt"].as_str().unwrap().contains("greeting.txt"));
    assert_eq!(
        git(&repo, &["log", "-1", "--format=%B"]).trim(),
        "Add greeting\n\nGenerated body"
    );

    s.shutdown().await;
    let rows: Vec<(String, i64)> = s
        .database()
        .prepare(
            "SELECT status, attempt_count FROM orchestration_v2_effect_outbox
             WHERE effect_type = 'thread-title.generate'",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(rows, vec![("succeeded".to_owned(), 1)]);
}

/// A Claude title runs with no tools, hooks or slash commands, `dontAsk`, strict MCP config, in
/// an empty temporary directory; a Claude commit message runs in the checkout.
async fn claude_titles_and_commits() {
    let temp = tempfile::tempdir().unwrap();
    let mut reports = Reports::start().await;
    let mode = temp.path().join("mode.txt");
    std::fs::write(&mode, "reply").unwrap();
    let settings = json!({
        "providerInstances": {"claudeAgent": instance("claudeAgent", temp.path(), &reports.address, &mode)},
        "textGenerationModelSelection": {"instanceId": "claudeAgent", "model": "claude-haiku-5-5"},
    });
    let mut s = Server::start(temp, settings, None).await;
    let repo = s.home().join("work").join("notes");
    repository(&repo);
    s.create_project("project:notes", &repo).await;

    s.title_thread(
        "project:notes",
        "thread:claude",
        "Fix pairing expiry.",
        "Fix Pairing Expiry",
    )
    .await;
    let report = reports.next().await;
    assert_eq!(report["kind"], "claude");
    let argv = args(&report);
    let tools = argv.iter().position(|arg| arg == "--tools").unwrap();
    assert_eq!(argv[tools + 1], "", "{argv:?}");
    assert!(!argv.iter().any(|arg| arg == "--allowedTools"));
    for flag in [
        "--disable-slash-commands",
        "--strict-mcp-config",
        "--no-session-persistence",
    ] {
        assert!(
            argv.iter().any(|arg| arg == flag),
            "{flag} missing from {argv:?}"
        );
    }
    assert!(
        argv.windows(2)
            .any(|pair| pair == ["--permission-mode", "dontAsk"])
    );
    let settings_arg = &argv[argv.iter().position(|arg| arg == "--settings").unwrap() + 1];
    assert_eq!(
        serde_json::from_str::<Value>(settings_arg).unwrap()["disableAllHooks"],
        true
    );
    let cwd = PathBuf::from(report["cwd"].as_str().unwrap());
    assert!(
        cwd.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("t3code-claude-title-"),
        "{}",
        cwd.display()
    );
    assert!(!cwd.exists(), "the title directory is removed");

    std::fs::write(repo.join("greeting.txt"), "hi\n").unwrap();
    let action = s.subscribe(
        "git.runStackedAction",
        json!({"actionId":"action:claude","cwd":repo.to_string_lossy(),"action":"commit"}),
    );
    ok(s.drain(&action).await.1);
    let report = reports.next().await;
    assert_eq!(report["kind"], "claude");
    assert_eq!(
        canonical(Path::new(report["cwd"].as_str().unwrap())),
        canonical(&repo)
    );
    assert_eq!(
        git(&repo, &["log", "-1", "--format=%s"]).trim(),
        "Add greeting"
    );
    s.shutdown().await;
}

mod review {
    //! A pull request from a fork branch on a `git daemon` remote, bound by a fake host.
    use super::*;
    use std::sync::Arc;
    use t3_contracts::pull_request::*;
    use t3_pull_request::{
        Result,
        context::{Binding, Context, Project},
        host::Host,
    };
    use t3_server::adapters::pull_requests::{ProjectBinder, ProjectRecord};

    fn decode<T: serde::de::DeserializeOwned>(value: Value) -> T {
        serde_json::from_value(value).unwrap()
    }

    pub struct FakeHost {
        pub head: std::sync::Mutex<String>,
        caps: PullRequestCapabilities,
    }
    impl FakeHost {
        pub fn new(head: String) -> Arc<Self> {
            Arc::new(Self {
                head: std::sync::Mutex::new(head),
                caps: decode(
                    json!({"diff": true, "comment": false, "actions": [], "mergeMethods": [], "search": false, "review": {"inlineComment": false, "reply": false, "resolve": false, "verdicts": []}, "reviewers": {"request": false, "listCandidates": false}}),
                ),
            })
        }
        fn summary(ctx: &Context) -> Value {
            json!({
                "provider": "github", "projectId": ctx.project.id, "repository": ctx.project.repository,
                "number": ctx.reference.number, "title": "Rename the greeting",
                "url": format!("https://github.com/{}/pull/{}", ctx.project.repository, ctx.reference.number),
                "state": "open", "headBranch": "topic", "baseBranch": "main",
                "updatedAt": "2026-01-01T00:00:00.000Z"
            })
        }
    }
    #[async_trait::async_trait]
    impl Host for FakeHost {
        fn kind(&self) -> &str {
            "github"
        }
        fn capabilities(&self) -> &PullRequestCapabilities {
            &self.caps
        }
        async fn viewer(&self, _: &Project) -> Result<String> {
            Ok("alice".into())
        }
        async fn summary(&self, ctx: &Context) -> Result<PullRequestSummary> {
            Ok(decode(Self::summary(ctx)))
        }
        async fn detail(&self, ctx: &Context) -> Result<PullRequestDetail> {
            let mut value = Self::summary(ctx);
            value.as_object_mut().unwrap().extend(
                json!({
                    "capabilities": self.caps,
                    "viewerPermissions": {"actions": [], "comment": false, "resolve": false, "verdicts": [], "requestReviewers": false},
                    "projectTitle": ctx.project.title, "workspaceRoot": ctx.project.workspace_root,
                    "body": "Renames it.", "author": null, "isDraft": false, "mergeability": "mergeable",
                    "additions": 1, "deletions": 1, "changedFiles": 1,
                    "createdAt": "2026-01-01T00:00:00.000Z", "reviewers": [], "labels": [],
                    "checks": [], "mergeCapabilities": {"merge": false, "squash": false, "rebase": false},
                    "headSha": *self.head.lock().unwrap(), "mergedAt": null, "closedAt": null
                })
                .as_object()
                .unwrap()
                .clone(),
            );
            Ok(decode(value))
        }
        async fn activity(&self, _: &Context) -> Result<PullRequestActivity> {
            Ok(decode(
                json!({"comments":[],"commentCount":0,"commentsTruncated":false,"reviewThreads":[],"commits":[]}),
            ))
        }
        async fn stack(&self, _: &Context, _: bool) -> Result<Option<PullRequestStack>> {
            Ok(None)
        }
        async fn permissions(&self, _: &Context, _: bool) -> Result<PullRequestViewerPermissions> {
            Ok(decode(
                json!({"actions": [], "comment": false, "resolve": false, "verdicts": [], "requestReviewers": false}),
            ))
        }
    }

    pub struct Binder(pub Arc<FakeHost>);
    #[async_trait::async_trait]
    impl ProjectBinder for Binder {
        async fn bind(&self, project: ProjectRecord) -> Result<Option<Binding>> {
            Ok(Some(Binding {
                project: Project {
                    id: project.id,
                    title: project.title,
                    workspace_root: project.workspace_root.to_string_lossy().into(),
                    host: "github.com".into(),
                    repository: "owner/repo".into(),
                    canonical_repository_key: Some("github.com/owner/repo".into()),
                    remove_agent_credits_on_merge: false,
                },
                host: self.0.clone(),
            }))
        }
        fn canonical_repository_key(&self, key: &str) -> String {
            key.to_lowercase()
        }
        fn public_host(&self) -> Option<Arc<dyn Host>> {
            None
        }
    }

    /// `git daemon` serving `base` on a loopback port; killed on drop.
    pub struct Daemon {
        child: tokio::process::Child,
        pub port: u16,
    }
    impl Daemon {
        pub async fn start(base: &Path) -> Self {
            use tokio::io::AsyncBufReadExt;
            let port = std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port();
            let mut child = tokio::process::Command::new("git")
                .args([
                    "daemon",
                    "--verbose",
                    "--reuseaddr",
                    "--export-all",
                    "--listen=127.0.0.1",
                    &format!("--port={port}"),
                    &format!("--base-path={}", base.display()),
                    &base.to_string_lossy(),
                ])
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
            loop {
                let line = lines.next_line().await.unwrap().expect("git daemon exited");
                if line.contains("Ready to rumble") {
                    break;
                }
            }
            tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
            Self { child, port }
        }
    }
    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.child.start_kill();
        }
    }
}

/// Nothing of a run survives: no review checkout folder, no registered worktree, no per-run ref,
/// and the repository's post-checkout hook never ran.
fn nothing_left(repo: &Path, worktrees: &Path, marker: &Path) {
    let leftovers: Vec<_> = std::fs::read_dir(worktrees)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with("review-guide-"))
                .collect()
        })
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    assert_eq!(
        git(repo, &["for-each-ref", "refs/t3/review-guide"]).trim(),
        ""
    );
    let worktree_list = git(repo, &["worktree", "list", "--porcelain"]);
    assert_eq!(
        worktree_list.matches("worktree ").count(),
        1,
        "{worktree_list}"
    );
    assert!(!marker.exists(), "the post-checkout hook ran");
}

async fn review_guides() {
    use review::{Binder, Daemon, FakeHost};
    use std::sync::Arc;
    let temp = tempfile::tempdir().unwrap();
    let mut reports = Reports::start().await;
    let mode = temp.path().join("mode.txt");
    std::fs::write(&mode, "reply").unwrap();
    // The fork: `main` and a `topic` branch the project's checkout has never fetched.
    let served = temp.path().join("served");
    std::fs::create_dir_all(&served).unwrap();
    let seed = temp.path().join("seed");
    repository(&seed);
    git(
        &served,
        &["clone", "-q", "--bare", &seed.to_string_lossy(), "repo.git"],
    );
    let daemon = Daemon::start(&served).await;
    let remote = format!("git://127.0.0.1:{}/repo.git", daemon.port);
    let repo = temp.path().join("work").join("review");
    std::fs::create_dir_all(repo.parent().unwrap()).unwrap();
    git(
        temp.path(),
        &["clone", "-q", &remote, &repo.to_string_lossy()],
    );
    git(&seed, &["switch", "-q", "-c", "topic"]);
    std::fs::write(seed.join("hello.txt"), "hello there\n").unwrap();
    git(&seed, &["commit", "-q", "-am", "Rename the greeting"]);
    git(
        &seed,
        &[
            "push",
            "-q",
            &served.join("repo.git").to_string_lossy(),
            "topic",
        ],
    );
    let head = git(&seed, &["rev-parse", "HEAD"]).trim().to_owned();
    // The checkout's post-checkout hook; proven live before the server runs.
    let marker = temp.path().join("hook-ran");
    let hook = repo.join(".git").join("hooks").join("post-checkout");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\necho ran > '{}'\n",
            marker.to_string_lossy().replace('\\', "/")
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let control = temp.path().join("control");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &control.to_string_lossy(),
            "HEAD",
        ],
    );
    assert!(marker.exists(), "the control checkout ran the hook");
    git(
        &repo,
        &["worktree", "remove", "--force", &control.to_string_lossy()],
    );
    std::fs::remove_file(&marker).unwrap();

    let host = FakeHost::new(head.clone());
    let settings = json!({
        "providerInstances": {"codex": instance("codex", temp.path(), &reports.address, &mode)},
        "reviewGuidesEnabled": true,
    });
    let mut s = Server::start(temp, settings, Some(Arc::new(Binder(host.clone())))).await;
    let worktrees = s.home().join("worktrees");
    s.create_project("project:review", &repo).await;
    let reference = json!({"projectId": "project:review", "repository": "owner/repo", "number": 7});

    // Success: the guide is written from a detached checkout of the fork's head.
    let started = ok(s
        .call("pullRequests.generateReviewGuide", reference.clone())
        .await);
    assert_eq!(started["run"]["_tag"], "Running", "{started}");
    let report = reports.next().await;
    let cwd = PathBuf::from(report["cwd"].as_str().unwrap());
    assert!(
        canonical(&cwd).starts_with(canonical(&worktrees)),
        "{}",
        cwd.display()
    );
    let argv = args(&report);
    assert!(
        argv.windows(2).any(|pair| pair == ["-s", "read-only"]),
        "{argv:?}"
    );
    s.cleanups.recv().await.expect("cleanup");
    let guide = ok(s.call("pullRequests.reviewGuide", reference.clone()).await);
    assert_eq!(guide["guide"]["headSha"], head.as_str(), "{guide}");
    assert_eq!(guide["guide"]["stops"][0]["title"], "Overview", "{guide}");
    assert!(guide["run"].is_null(), "{guide}");
    nothing_left(&repo, &worktrees, &marker);

    // Failure: the agent exits unsuccessfully; the run fails and nothing is left.
    std::fs::write(&mode, "fail").unwrap();
    git(
        &seed,
        &["commit", "-q", "--allow-empty", "-m", "Second head"],
    );
    git(
        &seed,
        &[
            "push",
            "-q",
            &served.join("repo.git").to_string_lossy(),
            "topic",
        ],
    );
    *host.head.lock().unwrap() = git(&seed, &["rev-parse", "HEAD"]).trim().to_owned();
    ok(
        s.call("pullRequests.generateReviewGuide", reference.clone())
            .await,
    );
    assert_eq!(reports.next().await["mode"], "fail");
    s.cleanups.recv().await.expect("cleanup");
    let failed = ok(s.call("pullRequests.reviewGuide", reference.clone()).await);
    assert_eq!(failed["run"]["_tag"], "Failed", "{failed}");
    nothing_left(&repo, &worktrees, &marker);

    // Cancellation: shutdown while the agent runs stops it and removes the checkout.
    std::fs::write(&mode, "hang").unwrap();
    git(
        &seed,
        &["commit", "-q", "--allow-empty", "-m", "Third head"],
    );
    git(
        &seed,
        &[
            "push",
            "-q",
            &served.join("repo.git").to_string_lossy(),
            "topic",
        ],
    );
    *host.head.lock().unwrap() = git(&seed, &["rev-parse", "HEAD"]).trim().to_owned();
    ok(
        s.call("pullRequests.generateReviewGuide", reference.clone())
            .await,
    );
    assert_eq!(reports.next().await["mode"], "hang");
    s.shutdown().await;
    nothing_left(&repo, &worktrees, &marker);
    drop(daemon);
}
