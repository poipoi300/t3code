//! Generated text and project setup scripts over real sockets: thread titles from the
//! configured text generator, `git.runStackedAction` with a generated commit message, and
//! `git.preparePullRequestThread` with the project's setup script. Edges are faked only: an
//! OpenAI-compatible text generator on a loopback listener, a fake `glab` on the server's PATH,
//! temporary repositories and a bare local remote. Nothing reaches a real model, forge or
//! provider.
#![cfg(all(
    feature = "local-services",
    feature = "textgen",
    feature = "git-actions",
    feature = "orchestration"
))]
#![allow(clippy::unwrap_used, clippy::expect_used)]
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use t3_rpc::{CallEvent, CallKind, Client, Exit, Frame, RequestId};
use t3_server::adapters::config::{ConfigInputs, ServerFlags};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// One reply serves every operation: each decoder reads only its own fields.
const REPLY: &str = r#"{"title":"Fix Pairing Expiry","needsRefinement":false,"subject":"Add greeting.","body":"Generated body"}"#;

/// The text generator stand-in: records each request's authorization and prompt; answers 401
/// while `reject` is set.
#[derive(Clone, Default)]
struct Generator {
    requests: Arc<Mutex<Vec<(String, String)>>>,
    reject: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}
impl Generator {
    async fn start() -> (String, Self) {
        use axum::{
            http::{HeaderMap, StatusCode},
            response::IntoResponse,
        };
        let generator = Self::default();
        let state = generator.clone();
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(move |headers: HeaderMap, body: String| {
                let state = state.clone();
                async move {
                    let authorization = headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_owned();
                    let request: Value = serde_json::from_str(&body).unwrap_or_default();
                    let prompt = request["messages"][0]["content"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    state.requests.lock().unwrap().push((authorization, prompt));
                    state.notify.notify_waiters();
                    if state.reject.load(Ordering::SeqCst) {
                        return StatusCode::UNAUTHORIZED.into_response();
                    }
                    (
                        StatusCode::OK,
                        [("content-type", "application/json")],
                        json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":REPLY}}]})
                            .to_string(),
                    )
                        .into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}/v1"), generator)
    }
    fn requests(&self) -> Vec<(String, String)> {
        self.requests.lock().unwrap().clone()
    }
}

fn git(cwd: &Path, args: &[&str]) -> String {
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

fn repository(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    git(path, &["init", "-q", "-b", "main"]);
    git(path, &["config", "user.name", "t3"]);
    git(path, &["config", "user.email", "t3@example.invalid"]);
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

/// A fake `glab` that answers `mr view` with merge request 7 from branch `feature`.
fn fake_glab(bin: &Path) {
    std::fs::create_dir_all(bin).unwrap();
    let request = json!({
        "iid": 7, "title": "Fixture change", "state": "opened",
        "web_url": "https://gitlab.com/acme/web/-/merge_requests/7",
        "source_branch": "feature", "target_branch": "main",
    });
    std::fs::write(bin.join("mr.json"), request.to_string()).unwrap();
    std::fs::write(bin.join("project.json"), json!({"id": 42, "path_with_namespace": "acme/web"}).to_string()).unwrap();
    if cfg!(windows) {
        std::fs::write(
            bin.join("glab.cmd"),
            "@echo off\r\n\
             if \"%~1\"==\"--version\" (\r\n echo glab version 1.0.0\r\n exit /b 0\r\n)\r\n\
             if \"%~1\"==\"auth\" exit /b 1\r\n\
             if \"%~1\"==\"mr\" (\r\n type \"%~dp0mr.json\"\r\n exit /b 0\r\n)\r\n\
             type \"%~dp0project.json\"\r\n",
        )
        .unwrap();
    } else {
        let script = bin.join("glab");
        std::fs::write(
            &script,
            "#!/bin/sh\nd=$(dirname \"$0\")\n\
             case \"$1\" in --version) echo 'glab version 1.0.0';; auth) exit 1;; \
             mr) cat \"$d/mr.json\";; *) cat \"$d/project.json\";; esac\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
}

fn ok(exit: Exit) -> Value {
    match exit {
        Exit::Success { value } => value.unwrap_or(Value::Null),
        other => panic!("expected success, got {}", serde_json::to_value(other).unwrap()),
    }
}

fn failure(exit: Exit) -> Value {
    let encoded = serde_json::to_value(&exit).unwrap();
    let cause = &encoded["cause"][0];
    let value = if cause["error"].is_null() {
        &cause["defect"]
    } else {
        &cause["error"]
    };
    assert!(!value.is_null(), "expected a failure, got {encoded}");
    value.clone()
}

fn missing_scope(exit: Exit, scope: &str) {
    let error = failure(exit);
    assert_eq!(error["_tag"], "EnvironmentAuthorizationError", "{error}");
    assert_eq!(error["requiredPermission"], scope, "{error}");
}

fn outside_workspace(exit: Exit) {
    let error = failure(exit);
    assert_eq!(error["_tag"], "EnvironmentAuthorizationError", "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("outside this session's authorized project workspaces"),
        "{error}"
    );
}

const STANDARD_SCOPES: [&str; 16] = [
    "orchestration:read",
    "orchestration:operate",
    "settings:write",
    "providers:manage",
    "environment:maintain",
    "preview:operate",
    "diagnostics:read",
    "terminal:read",
    "terminal:operate",
    "source-control:write",
    "filesystem:read",
    "filesystem:write",
    "relay:read",
    "access:read",
    "access:write",
    "relay:write",
];

struct Server {
    temp: tempfile::TempDir,
    runtime: Option<t3_server::runtime::RunningServer>,
    origin: String,
    cookie: String,
    socket: Socket,
    client: Client,
    chunks: Vec<(RequestId, Value)>,
    exits: Vec<(RequestId, Exit)>,
}

impl Server {
    /// The configured text generator `fake` writes titles and source-control text; Replay (dev)
    /// runs turns; the PATH holds the fake glab and Git only.
    async fn start(generator: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        fake_glab(&home.join("bin"));
        let base = home.join(".t3");
        std::fs::create_dir_all(base.join("userdata")).unwrap();
        std::fs::write(
            base.join("userdata").join("settings.json"),
            json!({
                "textGenerators": {"fake": {"kind": "openaiCompatible", "baseUrl": generator, "apiKey": "test-key", "enabled": true}},
                "textGenerationModelSelection": {"instanceId": "fake", "model": "fake-model"},
            })
            .to_string(),
        )
        .unwrap();
        let path = std::env::join_paths(std::iter::once(home.join("bin")).chain(git_directories()))
            .unwrap();
        let env = ["SystemRoot", "ComSpec", "WINDIR"]
            .into_iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_owned(), v)))
            .chain([
                ("PATH".to_owned(), path.to_string_lossy().into_owned()),
                ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
                (
                    "GIT_CONFIG_GLOBAL".into(),
                    home.join(".no-global-config").to_string_lossy().into_owned(),
                ),
                ("GIT_TERMINAL_PROMPT".into(), "0".into()),
                ("GIT_AUTHOR_NAME".into(), "t3".into()),
                ("GIT_AUTHOR_EMAIL".into(), "t3@example.invalid".into()),
                ("GIT_COMMITTER_NAME".into(), "t3".into()),
                ("GIT_COMMITTER_EMAIL".into(), "t3@example.invalid".into()),
            ])
            .collect();
        let runtime = t3_server::runtime::start_with(
            ConfigInputs {
                flags: ServerFlags {
                    base_dir: Some(base.to_string_lossy().into_owned()),
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
                dev_replay: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let origin = format!("http://{}", runtime.address);
        let paired = reqwest::Client::new()
            .post(format!("{origin}/api/auth/browser-session"))
            .json(&json!({"credential": runtime.pairing_credential}))
            .send()
            .await
            .unwrap();
        let cookie = session_cookie(&paired);
        let socket = connect(&origin, runtime.address, &cookie).await;
        Self {
            temp,
            runtime: Some(runtime),
            origin,
            cookie,
            socket,
            client: Client::new(0, Default::default()),
            chunks: Vec::new(),
            exits: Vec::new(),
        }
    }
    fn home(&self) -> &Path {
        self.temp.path()
    }
    fn base(&self) -> PathBuf {
        self.home().join(".t3")
    }
    fn runtime(&self) -> &t3_server::runtime::RunningServer {
        self.runtime.as_ref().unwrap()
    }
    fn database(&self) -> t3_persistence::rusqlite::Connection {
        let layout = t3_config::home::HomeLayout {
            base_dir: self.base(),
            state_dir: self.base().join("userdata"),
        };
        t3_persistence::rusqlite::Connection::open(layout.database()).unwrap()
    }
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
        let cookie = session_cookie(&paired);
        self.socket = connect(&self.origin, self.runtime().address, &cookie).await;
        self.client = Client::new(0, Default::default());
        self.chunks.clear();
        self.exits.clear();
    }
    async fn flush(&mut self) {
        while let Some(message) = self.client.pop_outgoing() {
            let Frame::Text(text) = t3_rpc::codec::encode(&message).unwrap() else {
                panic!("text frame expected");
            };
            self.socket.send(Message::Text(text.into())).await.unwrap();
        }
    }
    async fn event(&mut self) -> Option<CallEvent> {
        loop {
            self.flush().await;
            if let Some(event) = self.client.pop_event() {
                return Some(event);
            }
            let frame = match self.socket.next().await {
                Some(Ok(Message::Text(text))) => Frame::Text(text.to_string()),
                Some(Ok(Message::Binary(bytes))) => Frame::Binary(bytes.to_vec()),
                Some(Ok(Message::Ping(payload))) => {
                    self.socket.send(Message::Pong(payload)).await.unwrap();
                    continue;
                }
                _ => return None,
            };
            for message in t3_rpc::codec::decode(&frame).unwrap() {
                self.client.receive(message, 0);
            }
        }
    }
    async fn pump(&mut self) {
        match self.event().await.expect("connection closed") {
            CallEvent::Chunk { id, values } => {
                self.client.acknowledge(id.clone());
                self.chunks.extend(values.into_iter().map(|value| (id.clone(), value)));
            }
            CallEvent::Exit { id, exit } => self.exits.push((id, exit)),
            other => panic!("unexpected {other:?}"),
        }
    }
    fn subscribe(&mut self, tag: &str, payload: Value) -> RequestId {
        self.client.start(tag, payload, CallKind::Stream).unwrap()
    }
    async fn next(&mut self, id: &RequestId) -> Result<Value, Exit> {
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
    async fn call(&mut self, tag: &str, payload: Value) -> Exit {
        let id = self.client.start(tag, payload, CallKind::Unary).unwrap();
        loop {
            if let Some(index) = self.exits.iter().position(|(exit, _)| exit == &id) {
                return self.exits.remove(index).1;
            }
            self.pump().await;
        }
    }
    /// Every value of stream `id` until it ends, and its exit.
    async fn drain(&mut self, id: &RequestId) -> (Vec<Value>, Exit) {
        let mut values = Vec::new();
        loop {
            match self.next(id).await {
                Ok(value) => values.push(value),
                Err(exit) => return (values, exit),
            }
        }
    }
    async fn create_project(&mut self, id: &str, root: &Path, scripts: Value) {
        ok(self
            .call(
                "projects.mutate",
                json!({"type": "project.create", "commandId": format!("command:{id}"), "projectId": id, "title": id, "workspaceRoot": root.to_string_lossy(), "scripts": scripts}),
            )
            .await);
    }
}

fn session_cookie(response: &reqwest::Response) -> String {
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
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

#[tokio::test]
async fn a_new_threads_title_is_generated_once_by_the_configured_text_generator() {
    let (url, generator) = Generator::start().await;
    let mut s = Server::start(&url).await;
    let root = s.home().join("work").join("titles");
    std::fs::create_dir_all(&root).unwrap();
    s.create_project("project:titles", &root, json!([])).await;
    let thread = "thread:titles";
    ok(s.call(
        "orchestration.dispatchCommand",
        json!({"type":"thread.create","commandId":"command:thread","threadId":thread,"projectId":"project:titles","title":"New thread","modelSelection":{"instanceId":"devReplay","model":"multi-turn"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"}),
    )
    .await);
    let shell = s.subscribe("orchestration.subscribeShell", json!({}));
    ok(s.call(
        "orchestration.dispatchCommand",
        json!({"type":"message.dispatch","commandId":"command:message","threadId":thread,"messageId":"message:first","text":"Fix pairing expiry and preserve remote access.","attachments":[],"titleSeed":"Fix pairing expiry","dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"}),
    )
    .await);
    // The milestone: the generated title reaches the shell stream.
    loop {
        let value = s.next(&shell).await.expect("shell stream ended");
        if value.to_string().contains("Fix Pairing Expiry") {
            break;
        }
    }
    let requests = generator.requests();
    assert_eq!(requests.len(), 1, "one generation request");
    assert_eq!(requests[0].0, "Bearer test-key");
    assert!(
        requests[0].1.contains("Fix pairing expiry and preserve remote access."),
        "{}",
        requests[0].1
    );
    let titles = ok(s
        .call("orchestration.getThreadTitles", json!({"threadIds": [thread]}))
        .await);
    assert_eq!(titles[thread], "Fix Pairing Expiry");

    // The effect settled on its first attempt (the worker is joined at shutdown).
    s.runtime.take().unwrap().shutdown().await.unwrap();
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

#[tokio::test]
async fn stacked_actions_commit_and_push_with_a_generated_message() {
    let (url, generator) = Generator::start().await;
    let mut s = Server::start(&url).await;
    for method in ["git.runStackedAction", "git.preparePullRequestThread"] {
        assert!(
            !s.runtime().unserved_methods.contains(&method),
            "{method} is unserved"
        );
    }
    let repo = s.home().join("work").join("app");
    repository(&repo);
    let remote = s.home().join("remote.git");
    git(s.home(), &["init", "-q", "--bare", "-b", "main", &remote.to_string_lossy()]);
    git(&repo, &["remote", "add", "origin", &remote.to_string_lossy()]);
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    s.create_project("project:app", &repo, json!([])).await;

    // Success: the generated message is committed and pushed.
    std::fs::write(repo.join("greeting.txt"), "hi\n").unwrap();
    let action = s.subscribe(
        "git.runStackedAction",
        json!({"actionId": "action:1", "cwd": repo.to_string_lossy(), "action": "commit_push"}),
    );
    let (events, exit) = s.drain(&action).await;
    ok(exit);
    let kinds: Vec<_> = events.iter().map(|event| event["kind"].clone()).collect();
    assert_eq!(kinds.first(), Some(&json!("action_started")), "{events:?}");
    let finished = events
        .iter()
        .find(|event| event["kind"] == "action_finished")
        .unwrap_or_else(|| panic!("no action_finished in {events:?}"));
    assert_eq!(finished["result"]["commit"]["subject"], "Add greeting", "{finished}");
    assert_eq!(finished["result"]["push"]["status"], "pushed", "{finished}");
    assert_eq!(git(&repo, &["log", "-1", "--format=%B"]).trim(), "Add greeting\n\nGenerated body");
    assert_eq!(
        git(&repo, &["rev-parse", "HEAD"]),
        git(&remote, &["rev-parse", "refs/heads/main"])
    );
    let requests = generator.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "Bearer test-key");
    assert!(requests[0].1.contains("greeting.txt"), "{}", requests[0].1);

    // Typed failure: the model API rejects the key; nothing is committed.
    generator.reject.store(true, Ordering::SeqCst);
    std::fs::write(repo.join("second.txt"), "two\n").unwrap();
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let action = s.subscribe(
        "git.runStackedAction",
        json!({"actionId": "action:2", "cwd": repo.to_string_lossy(), "action": "commit"}),
    );
    let (events, exit) = s.drain(&action).await;
    let error = failure(exit);
    assert_eq!(error["_tag"], "TextGenerationError", "{error}");
    assert_eq!(error["operation"], "generateCommitMessage");
    assert_eq!(error["detail"], "The model API rejected the API key.");
    assert!(events.iter().any(|event| event["kind"] == "action_failed"), "{events:?}");
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), head);

    // Refusals: a checkout outside every workspace, and a session without source-control:write.
    let elsewhere = s.home().join("elsewhere");
    repository(&elsewhere);
    let action = s.subscribe(
        "git.runStackedAction",
        json!({"actionId": "action:3", "cwd": elsewhere.to_string_lossy(), "action": "commit", "commitMessage": "x"}),
    );
    outside_workspace(s.drain(&action).await.1);
    let without: Vec<_> = STANDARD_SCOPES
        .iter()
        .copied()
        .filter(|scope| *scope != "source-control:write")
        .collect();
    s.reconnect_with(&without).await;
    let action = s.subscribe(
        "git.runStackedAction",
        json!({"actionId": "action:4", "cwd": repo.to_string_lossy(), "action": "commit", "commitMessage": "x"}),
    );
    missing_scope(s.drain(&action).await.1, "source-control:write");
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), head);
    s.runtime.take().unwrap().shutdown().await.unwrap();
}

#[tokio::test]
async fn pull_request_threads_get_a_worktree_and_the_projects_setup_script() {
    let (url, _generator) = Generator::start().await;
    let mut s = Server::start(&url).await;
    let repo = s.home().join("work").join("review");
    repository(&repo);
    git(&repo, &["branch", "feature"]);
    git(&repo, &["remote", "add", "origin", "https://gitlab.com/acme/web.git"]);
    // Fetches fail at once instead of reaching the network.
    git(&repo, &["config", "http.proxy", "http://127.0.0.1:9"]);
    s.create_project(
        "project:review",
        &repo,
        json!([{"id": "setup", "name": "Setup", "command": "echo t3-setup-marker", "icon": "configure", "runOnWorktreeCreate": true}]),
    )
    .await;
    let thread = "thread:review";

    let prepared = ok(s
        .call(
            "git.preparePullRequestThread",
            json!({"cwd": repo.to_string_lossy(), "reference": "7", "mode": "worktree", "threadId": thread}),
        )
        .await);
    assert_eq!(prepared["branch"], "feature", "{prepared}");
    assert_eq!(prepared["pullRequest"]["number"], 7, "{prepared}");
    let worktree = PathBuf::from(prepared["worktreePath"].as_str().unwrap());
    assert!(
        std::fs::canonicalize(&worktree)
            .unwrap()
            .starts_with(std::fs::canonicalize(s.base().join("worktrees")).unwrap()),
        "{}",
        worktree.display()
    );
    assert!(worktree.join("hello.txt").is_file());

    // The project's setup script runs in the thread's setup terminal, in the worktree.
    let attach = s.subscribe(
        "terminal.attach",
        json!({"threadId": thread, "terminalId": "setup-setup"}),
    );
    let mut output = String::new();
    while output.matches("t3-setup-marker").count() < 2 {
        let value = s.next(&attach).await.expect("terminal stream ended");
        output.push_str(&value.to_string());
    }

    // Typed failure: local mode refuses a checkout with uncommitted tracked changes.
    std::fs::write(repo.join("hello.txt"), "changed\n").unwrap();
    let error = failure(
        s.call(
            "git.preparePullRequestThread",
            json!({"cwd": repo.to_string_lossy(), "reference": "7", "mode": "local"}),
        )
        .await,
    );
    assert_eq!(error["_tag"], "GitCommandError", "{error}");
    assert!(
        error["detail"].as_str().unwrap().contains("uncommitted changes"),
        "{error}"
    );

    // Refusals: outside every workspace, and without source-control:write.
    let elsewhere = s.home().join("elsewhere");
    repository(&elsewhere);
    outside_workspace(
        s.call(
            "git.preparePullRequestThread",
            json!({"cwd": elsewhere.to_string_lossy(), "reference": "7", "mode": "local"}),
        )
        .await,
    );
    let without: Vec<_> = STANDARD_SCOPES
        .iter()
        .copied()
        .filter(|scope| *scope != "source-control:write")
        .collect();
    s.reconnect_with(&without).await;
    missing_scope(
        s.call(
            "git.preparePullRequestThread",
            json!({"cwd": repo.to_string_lossy(), "reference": "7", "mode": "local"}),
        )
        .await,
        "source-control:write",
    );
    s.runtime.take().unwrap().shutdown().await.unwrap();
}
