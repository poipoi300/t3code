//! `orchestration.launchThread` over a real socket: Replay (dev) runs the first turn, worktree
//! launches publish into the VCS group's setup tracker (`subscribeWorktreeSetup`), and the
//! client's project, refs, worktree path and runtime mode are refused before anything is created.
#![cfg(all(
    feature = "local-services",
    feature = "orchestration",
    feature = "git",
    feature = "workspace",
    feature = "git-actions"
))]
#![allow(clippy::unwrap_used, clippy::expect_used)]
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use t3_rpc::{CallEvent, CallKind, Client, Exit, Frame, RequestId};
use t3_server::adapters::config::{ConfigInputs, ServerFlags};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const MODEL: &str = r#"{"instanceId":"devReplay","model":"multi-turn"}"#;

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(["-c", "user.name=t3", "-c", "user.email=t3@example.invalid"])
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", cwd.join(".no-global-config"))
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn repository(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    git(path, &["init", "-q", "-b", "main"]);
    std::fs::write(path.join("hello.txt"), "hello\n").unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "-q", "-m", "init"]);
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

/// The first failure (`Fail` error or `Die` defect).
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

/// TypeScript's typed launch refusal; `text` is found in its message or its cause.
fn launch_refused(exit: Exit, text: &str) -> Value {
    let error = failure(exit);
    assert_eq!(error["_tag"], "OrchestrationV2ThreadLaunchError", "{error}");
    assert!(error.to_string().contains(text), "{error}");
    error
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap()
}

/// A setup script that runs until it is stopped (`async: false`: the launch waits for it).
fn blocking_script() -> String {
    let command = if cfg!(windows) {
        "ping -n 600 127.0.0.1"
    } else {
        "sleep 600"
    };
    json!([{"id":"setup","name":"Setup","command":command,"icon":"configure","runOnWorktreeCreate":true,"async":false}]).to_string()
}

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
    /// Dev mode (Replay (dev) registered and enabled) under a managed ceiling of
    /// `auto-accept-edits`.
    async fn start() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let policy = temp.path().join("policy.json");
        std::fs::write(
            &policy,
            r#"{"schemaVersion":1,"organization":"Acme","limits":{"maxRuntimeMode":"auto-accept-edits"}}"#,
        )
        .unwrap();
        // A real server's terminals inherit its full environment; setup scripts need at least
        // `PATH` and, on Windows, `PATHEXT` (without it PowerShell loses native exit codes).
        let mut env: std::collections::BTreeMap<String, String> =
            ["SystemRoot", "ComSpec", "WINDIR", "PATH", "PATHEXT"]
                .into_iter()
                .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_owned(), v)))
                .collect();
        env.insert(
            "T3CODE_DEV_MANAGED_POLICY_PATH".into(),
            policy.to_string_lossy().into_owned(),
        );
        let runtime = t3_server::runtime::start(ConfigInputs {
            flags: ServerFlags {
                base_dir: Some(home.to_string_lossy().into_owned()),
                host: Some("127.0.0.1".into()),
                port: Some(0),
                dev_url: Some("http://127.0.0.1:5733".into()),
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
        })
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
    fn home(&self) -> PathBuf {
        self.temp.path().join("home")
    }
    fn runtime(&self) -> &t3_server::runtime::RunningServer {
        self.runtime.as_ref().unwrap()
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
                self.chunks
                    .extend(values.into_iter().map(|value| (id.clone(), value)));
            }
            CallEvent::Exit { id, exit } => self.exits.push((id, exit)),
            // A stream this client interrupted.
            CallEvent::Failed { .. } => {}
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
    /// Values of stream `id` until one satisfies `done`; that one is returned.
    async fn until(&mut self, id: &RequestId, done: impl Fn(&Value) -> bool) -> Value {
        loop {
            match self.next(id).await {
                Ok(value) if done(&value) => return value,
                Ok(_) => {}
                Err(exit) => panic!("stream ended: {}", serde_json::to_value(exit).unwrap()),
            }
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
    fn database(&self) -> t3_persistence::rusqlite::Connection {
        let layout = t3_config::home::HomeLayout {
            base_dir: self.home(),
            state_dir: self.home().join("userdata"),
        };
        t3_persistence::rusqlite::Connection::open(layout.database()).unwrap()
    }
    fn insert_project(&self, id: &str, root: &Path, scripts: &str) {
        self.database()
            .execute(
                "INSERT INTO projection_projects
                 (project_id, title, workspace_root, scripts_json, created_at, updated_at)
                 VALUES (?1, ?1, ?2, ?3, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
                (id, root.to_string_lossy(), scripts),
            )
            .unwrap();
    }
    fn thread_count(&self) -> i64 {
        self.database()
            .query_row(
                "SELECT COUNT(*) FROM orchestration_v2_projection_threads",
                [],
                |row| row.get(0),
            )
            .unwrap()
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
    let (socket, _) = tokio_tungstenite::connect_async(format!(
        "ws://{address}/ws?orchestrationProtocol=2&wsTicket={}",
        ticket["ticket"].as_str().unwrap()
    ))
    .await
    .unwrap();
    socket
}

fn launch(thread: &str, project: &str, strategy: Value, mode: &str) -> Value {
    json!({
        "commandId": format!("command:{thread}"),
        "threadId": thread,
        "projectId": project,
        "title": "Launched",
        "modelSelection": serde_json::from_str::<Value>(MODEL).unwrap(),
        "runtimeMode": mode,
        "interactionMode": "default",
        "workspaceStrategy": strategy,
        "initialMessage": {"text": "Hello", "attachments": []},
    })
}

/// The thread's first run streams the agent's reply and settles, seen on `subscribeThread`
/// (as a snapshot when it already finished, or as live deltas).
async fn first_turn_completes(s: &mut Server, thread: &str) {
    let stream = s.subscribe("orchestration.subscribeThread", json!({"threadId": thread}));
    let (mut reply, mut settled) = (false, false);
    while !(reply && settled) {
        let value = s.next(&stream).await.unwrap_or_else(|exit| {
            panic!(
                "thread stream ended: {}",
                serde_json::to_value(exit).unwrap()
            )
        });
        let text = value.to_string();
        reply |= text.contains("\"assistant\"");
        settled |= text.contains("\"run.updated\"") && text.contains("\"completed\"")
            || text.contains("\"runs\"") && text.contains("\"status\":\"completed\"");
    }
    s.client.interrupt(&stream);
}

fn stage<'a>(snapshot: &'a Value, id: &str) -> &'a Value {
    snapshot["stages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stage| stage["id"] == id)
        .unwrap_or_else(|| panic!("no stage {id}: {snapshot}"))
}

#[tokio::test]
async fn launches_into_the_project_root_and_a_new_worktree_run_their_first_turn() {
    let mut s = Server::start().await;
    assert!(
        !s.runtime()
            .unserved_methods
            .contains(&"orchestration.launchThread")
    );
    let project = s.home().join("project");
    repository(&project);
    s.insert_project("project-1", &project, "[]");

    // Root: the thread runs in the project folder on its current branch.
    let launched = ok(s
        .call(
            "orchestration.launchThread",
            launch(
                "thread:root",
                "project-1",
                json!({"type": "root"}),
                "auto-accept-edits",
            ),
        )
        .await);
    assert_eq!(launched["threadId"], "thread:root");
    assert_eq!(launched["resumed"], false);
    let thread = &launched["projection"]["thread"];
    assert_eq!(thread["branch"], "main", "{thread}");
    assert!(thread["worktreePath"].is_null(), "{thread}");
    first_turn_completes(&mut s, "thread:root").await;

    // A new worktree: the setup stream shows fetch, checkout, script and agent, then done.
    let setup = s.subscribe("subscribeWorktreeSetup", json!({"threadId": "thread:tree"}));
    assert_eq!(s.next(&setup).await.unwrap(), Value::Null);
    ok(s.call(
        "orchestration.launchThread",
        launch(
            "thread:tree",
            "project-1",
            json!({"type": "worktree", "baseRef": "main"}),
            "auto-accept-edits",
        ),
    )
    .await);
    let done = s
        .until(&setup, |snapshot| snapshot["phase"] != "running")
        .await;
    assert_eq!(done["phase"], "done", "{done}");
    assert_eq!(stage(&done, "fetch")["status"], "skipped", "{done}");
    assert_eq!(stage(&done, "checkout")["status"], "done", "{done}");
    assert_eq!(stage(&done, "setup-script")["status"], "skipped", "{done}");
    assert_eq!(stage(&done, "agent")["status"], "done", "{done}");
    let path = PathBuf::from(done["worktreePath"].as_str().unwrap());
    assert!(
        canonical(&path).starts_with(canonical(&s.home().join("worktrees")).join("project")),
        "{}",
        path.display()
    );
    let branch = done["branch"].as_str().unwrap().to_owned();
    assert!(branch.starts_with("t3/"), "{branch}");
    assert!(git(&project, &["worktree", "list", "--porcelain"]).contains(&branch));
    first_turn_completes(&mut s, "thread:tree").await;
    s.runtime.take().unwrap().shutdown().await.unwrap();
}

#[tokio::test]
async fn launches_refuse_unknown_projects_bad_refs_client_paths_ceilings_and_missing_scopes() {
    let mut s = Server::start().await;
    let project = s.home().join("project");
    repository(&project);
    s.insert_project("project-1", &project, "[]");
    let elsewhere = s.home().join("elsewhere");
    repository(&elsewhere);
    let root = json!({"type": "root"});

    launch_refused(
        s.call(
            "orchestration.launchThread",
            launch(
                "thread:a",
                "project-missing",
                root.clone(),
                "auto-accept-edits",
            ),
        )
        .await,
        "Failed to launch thread",
    );
    // A base ref that resolves to nothing: TypeScript's own message.
    launch_refused(
        s.call(
            "orchestration.launchThread",
            launch(
                "thread:b",
                "project-1",
                json!({"type": "worktree", "baseRef": "does-not-exist"}),
                "auto-accept-edits",
            ),
        )
        .await,
        "Base ref \\\"does-not-exist\\\" does not resolve to a commit",
    );
    // Refs and branches that Git would read as options never reach it.
    launch_refused(
        s.call(
            "orchestration.launchThread",
            launch(
                "thread:c",
                "project-1",
                json!({"type": "worktree", "baseRef": "--upload-pack=evil"}),
                "auto-accept-edits",
            ),
        )
        .await,
        "is not a valid Git revision",
    );
    launch_refused(
        s.call(
            "orchestration.launchThread",
            launch(
                "thread:d",
                "project-1",
                json!({"type": "worktree", "baseRef": "main", "branch": "-D"}),
                "auto-accept-edits",
            ),
        )
        .await,
        "is not a valid Git branch name",
    );
    // A client-named worktree must be one of the project's registered worktrees; a UNC path is
    // refused from its text.
    for path in [
        elsewhere.to_string_lossy().into_owned(),
        r"\\attacker.example\share".to_owned(),
    ] {
        launch_refused(
            s.call(
                "orchestration.launchThread",
                launch(
                    "thread:e",
                    "project-1",
                    json!({"type": "existing_worktree", "worktreePath": path}),
                    "auto-accept-edits",
                ),
            )
            .await,
            "not a registered worktree of this project",
        );
    }
    // A link inside the authorised project folder whose target is a network name (never a real
    // share: the host does not exist and is never contacted) is refused by the workspace path
    // rule, which runs before the only Git command of the check.
    let link = project.join("link-to-share");
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(r"\\t3-unreachable.invalid\share", &link).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("//t3-unreachable.invalid/share", &link).unwrap();
    launch_refused(
        s.call(
            "orchestration.launchThread",
            launch(
                "thread:e",
                "project-1",
                json!({"type": "existing_worktree", "worktreePath": link.to_string_lossy()}),
                "auto-accept-edits",
            ),
        )
        .await,
        "not a registered worktree of this project",
    );
    // Above the organisation's ceiling: refused at thread creation, as a normal create is.
    launch_refused(
        s.call(
            "orchestration.launchThread",
            launch("thread:f", "project-1", root.clone(), "full-access"),
        )
        .await,
        "Acme limits threads to Auto-accept edits.",
    );
    assert_eq!(s.thread_count(), 0);
    assert!(!git(&project, &["branch", "--list"]).contains("evil"));
    assert_eq!(
        git(&project, &["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        1
    );

    // The method's scope.
    s.reconnect_with(&["orchestration:read"]).await;
    let error = failure(
        s.call(
            "orchestration.launchThread",
            launch("thread:g", "project-1", root, "auto-accept-edits"),
        )
        .await,
    );
    assert_eq!(error["_tag"], "EnvironmentAuthorizationError", "{error}");
    assert_eq!(
        error["requiredPermission"], "orchestration:operate",
        "{error}"
    );
    assert_eq!(s.thread_count(), 0);
    s.runtime.take().unwrap().shutdown().await.unwrap();
}

/// `worktreeSetup.cancel` stops a launch waiting on its setup script: the tracker ends
/// cancelled and the worktree and its branch are removed before the cancel answers. A launch
/// still in its setup when the server shuts down is joined and cleaned up the same way.
/// Removal is asserted where a stopped process cannot hold a folder open (not Windows).
#[tokio::test]
async fn cancelled_and_shutdown_launches_remove_their_worktrees() {
    let mut s = Server::start().await;
    let project = s.home().join("blocking");
    repository(&project);
    s.insert_project("project-b", &project, &blocking_script());

    let running = |snapshot: &Value| {
        snapshot["setupScript"].is_object()
            && snapshot["stages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|stage| stage["id"] == "setup-script" && stage["status"] == "running")
    };
    let setup = s.subscribe(
        "subscribeWorktreeSetup",
        json!({"threadId": "thread:cancel"}),
    );
    ok(s.call(
        "orchestration.launchThread",
        launch(
            "thread:cancel",
            "project-b",
            json!({"type": "worktree", "baseRef": "main", "branch": "feature/cancel"}),
            "auto-accept-edits",
        ),
    )
    .await);
    let snapshot = s.until(&setup, running).await;
    let path = PathBuf::from(snapshot["worktreePath"].as_str().unwrap());
    assert!(path.join("hello.txt").exists());
    let cancelled = ok(s
        .call("worktreeSetup.cancel", json!({"threadId": "thread:cancel"}))
        .await);
    assert_eq!(cancelled["cancelled"], true);
    // On Windows the stopped script can still hold the folder for a moment (the terminal does
    // not await its exit), and removal then fails and is logged, as in TypeScript.
    if !cfg!(windows) {
        assert!(!path.exists(), "{}", path.display());
        assert!(!git(&project, &["branch", "--list"]).contains("feature/cancel"));
    }
    let ended = s
        .until(&setup, |snapshot| snapshot["phase"] != "running")
        .await;
    assert_eq!(ended["phase"], "cancelled", "{ended}");

    // Shut down while the next launch waits on its script.
    let setup = s.subscribe(
        "subscribeWorktreeSetup",
        json!({"threadId": "thread:shutdown"}),
    );
    ok(s.call(
        "orchestration.launchThread",
        launch(
            "thread:shutdown",
            "project-b",
            json!({"type": "worktree", "baseRef": "main", "branch": "feature/shutdown"}),
            "auto-accept-edits",
        ),
    )
    .await);
    let snapshot = s.until(&setup, running).await;
    let path = PathBuf::from(snapshot["worktreePath"].as_str().unwrap());
    s.runtime.take().unwrap().shutdown().await.unwrap();
    if !cfg!(windows) {
        assert!(!path.exists(), "{}", path.display());
    }
    // The stream ended with the server.
    loop {
        match s.event().await {
            None => break,
            Some(CallEvent::Exit { id, .. }) if id == setup => break,
            Some(_) => {}
        }
    }
}

/// `orchestration.getWorkflowScript`: TypeScript's `readWorkflowScript` over the server's
/// contained reader, rooted at `<home>/.claude/projects` (here a temporary home).
#[tokio::test]
async fn workflow_scripts_are_read_only_inside_the_claude_projects_folder() {
    let mut s = Server::start().await;
    assert!(
        !s.runtime()
            .unserved_methods
            .contains(&"orchestration.getWorkflowScript")
    );
    let root = s.home().join(".claude").join("projects");
    let session = root.join("session");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(session.join("flow.js"), "export default 1;\n").unwrap();
    std::fs::write(session.join("big.js"), "x".repeat(300 * 1024)).unwrap();
    std::fs::create_dir(session.join("dir.js")).unwrap();
    std::fs::write(session.join("notes.txt"), "text").unwrap();
    let elsewhere = s.home().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("secret.js"), "outside").unwrap();
    std::fs::create_dir(elsewhere.join("dir.js")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(elsewhere.join("secret.js"), session.join("link.js"))
        .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(elsewhere.join("secret.js"), session.join("link.js")).unwrap();
    let read =
        |path: &Path| json!({"threadId": "thread:any", "scriptPath": path.to_string_lossy()});

    let script = ok(s
        .call(
            "orchestration.getWorkflowScript",
            read(&session.join("flow.js")),
        )
        .await);
    assert_eq!(script["contents"], "export default 1;\n");
    assert_eq!(script["truncated"], false);
    assert_eq!(
        canonical(Path::new(script["scriptPath"].as_str().unwrap())),
        canonical(&session.join("flow.js"))
    );
    // TypeScript's 256 KiB cap.
    let big = ok(s
        .call(
            "orchestration.getWorkflowScript",
            read(&session.join("big.js")),
        )
        .await);
    assert_eq!(big["truncated"], true);
    assert_eq!(big["contents"].as_str().unwrap().len(), 256 * 1024);

    for (path, reason) in [
        (session.join("notes.txt"), "invalid-path"),
        (session.join("dir.js"), "not-regular-file"),
        (session.join("absent.js"), "not-found"),
        // Outside the folder, existing, missing or not a file: one refusal, decided from the
        // path's text before anything is resolved.
        (elsewhere.join("secret.js"), "not-found"),
        (elsewhere.join("absent.js"), "not-found"),
        (elsewhere.join("dir.js"), "not-found"),
        // A link inside the folder resolves to its target, which is outside.
        (session.join("link.js"), "outside-root"),
    ] {
        let error = failure(s.call("orchestration.getWorkflowScript", read(&path)).await);
        assert_eq!(
            error["_tag"], "OrchestrationGetWorkflowScriptError",
            "{error}"
        );
        assert_eq!(error["reason"], reason, "{}: {error}", path.display());
        assert!(!error.to_string().contains("outside\""), "{error}");
    }

    s.reconnect_with(&["filesystem:read"]).await;
    let error = failure(
        s.call(
            "orchestration.getWorkflowScript",
            read(&session.join("flow.js")),
        )
        .await,
    );
    assert_eq!(error["_tag"], "EnvironmentAuthorizationError", "{error}");
    assert_eq!(error["requiredPermission"], "orchestration:read", "{error}");
    s.runtime.take().unwrap().shutdown().await.unwrap();
}

/// TypeScript's launch publishes the setup script into the tracker: started (name, command,
/// terminal), its output tail, then `done` on exit 0 or `failed` with "Setup script exited with
/// N." otherwise (the run fails with it; the worktree stays, as the thread recorded it).
#[tokio::test]
async fn setup_scripts_publish_output_and_their_end_on_the_setup_stream() {
    let mut s = Server::start().await;
    let script = |command: &str| {
        json!([{"id":"setup","name":"Setup","command":command,"icon":"configure","runOnWorktreeCreate":true,"async":false}]).to_string()
    };
    let passing = s.home().join("passing");
    repository(&passing);
    s.insert_project("project-pass", &passing, &script("echo t3-setup-output"));
    let failing = s.home().join("failing");
    repository(&failing);
    // A native program that exits non-zero, as a failing `npm install` would.
    let exit_three = if cfg!(windows) {
        "cmd.exe /c exit 3".to_owned()
    } else {
        "sh -c 'exit 3'".to_owned()
    };
    s.insert_project("project-fail", &failing, &script(&exit_three));

    let setup = s.subscribe("subscribeWorktreeSetup", json!({"threadId": "thread:pass"}));
    assert_eq!(s.next(&setup).await.unwrap(), Value::Null);
    ok(s.call(
        "orchestration.launchThread",
        launch(
            "thread:pass",
            "project-pass",
            json!({"type": "worktree", "baseRef": "main", "branch": "feature/pass"}),
            "auto-accept-edits",
        ),
    )
    .await);
    let started = s
        .until(&setup, |snapshot| snapshot["setupScript"].is_object())
        .await;
    assert_eq!(started["setupScript"]["name"], "Setup", "{started}");
    assert_eq!(started["setupScript"]["command"], "echo t3-setup-output");
    let done = s
        .until(&setup, |snapshot| snapshot["phase"] != "running")
        .await;
    assert_eq!(done["phase"], "done", "{done}");
    let script_stage = stage(&done, "setup-script");
    assert_eq!(script_stage["status"], "done", "{done}");
    assert_eq!(script_stage["detail"], "exited with 0", "{done}");
    assert!(
        script_stage["tail"].to_string().contains("t3-setup-output"),
        "{done}"
    );
    assert_eq!(stage(&done, "agent")["status"], "done", "{done}");
    first_turn_completes(&mut s, "thread:pass").await;

    let setup = s.subscribe("subscribeWorktreeSetup", json!({"threadId": "thread:fail"}));
    assert_eq!(s.next(&setup).await.unwrap(), Value::Null);
    ok(s.call(
        "orchestration.launchThread",
        launch(
            "thread:fail",
            "project-fail",
            json!({"type": "worktree", "baseRef": "main", "branch": "feature/fail"}),
            "auto-accept-edits",
        ),
    )
    .await);
    let failed = s
        .until(&setup, |snapshot| snapshot["phase"] != "running")
        .await;
    assert_eq!(failed["phase"], "failed", "{failed}");
    assert_eq!(failed["error"], "Setup script exited with 3.", "{failed}");
    let script_stage = stage(&failed, "setup-script");
    assert_eq!(script_stage["status"], "failed", "{failed}");
    assert_eq!(script_stage["detail"], "exited with 3", "{failed}");
    s.runtime.take().unwrap().shutdown().await.unwrap();
}

/// Cancelling a launch ends its setup script's whole process tree, not only the shell: a child
/// the script started (as `npm install` starts node) holds a loopback connection to the test and
/// waits on a gate file. After the cancel and the server's shutdown (which awaits the stopped
/// terminals), the gate opens: a surviving child would answer, a dead one closed the socket.
#[cfg(windows)]
#[tokio::test]
async fn cancelling_a_launch_ends_its_setup_scripts_child_processes() {
    use tokio::io::AsyncReadExt;
    let mut s = Server::start().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let gate = s.home().join("child-gate");
    let child = s.home().join("child.ps1");
    std::fs::write(
        &child,
        format!(
            "$c = New-Object Net.Sockets.TcpClient('127.0.0.1', {port}); $s = $c.GetStream(); \
             $s.WriteByte(42); $s.Flush(); \
             while (-not (Test-Path -LiteralPath '{}')) {{ Start-Sleep -Milliseconds 50 }}; \
             $s.WriteByte(77); $s.Flush()",
            gate.display()
        ),
    )
    .unwrap();
    let project = s.home().join("child-project");
    repository(&project);
    let command = format!(
        "powershell.exe -NoProfile -ExecutionPolicy Bypass -File '{}'",
        child.display()
    );
    let scripts = json!([{"id":"setup","name":"Setup","command":command,"icon":"configure","runOnWorktreeCreate":true,"async":false}]).to_string();
    s.insert_project("project-child", &project, &scripts);
    ok(s.call(
        "orchestration.launchThread",
        launch(
            "thread:child",
            "project-child",
            json!({"type": "worktree", "baseRef": "main", "branch": "feature/child"}),
            "auto-accept-edits",
        ),
    )
    .await);
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut byte = [0u8; 1];
    socket.read_exact(&mut byte).await.unwrap();
    assert_eq!(byte[0], 42, "the child started");
    let cancelled = ok(s
        .call("worktreeSetup.cancel", json!({"threadId": "thread:child"}))
        .await);
    assert_eq!(cancelled["cancelled"], true);
    s.runtime.take().unwrap().shutdown().await.unwrap();
    std::fs::write(&gate, "open").unwrap();
    let read = socket.read(&mut byte).await;
    assert!(
        matches!(read, Ok(0) | Err(_)),
        "the setup script's child survived the cancel: {read:?} {byte:?}"
    );
}

/// The server's saved repository bindings (`<state>/repository-bindings.json`).
fn bindings_file(s: &Server) -> PathBuf {
    s.home().join("userdata").join("repository-bindings.json")
}

/// t3-git re-review 4, H-1 item 5: a new worktree is created through the project root's saved
/// binding. Here the saved record (written as a previous server would have left it) names one
/// repository, and the folder now holds another, with a hook and a smudge filter that write a
/// marker. A worktree launch refuses with `checkpoint.git.repository-changed` before any Git
/// command runs against it: no marker, no worktree, no branch in the planted repository.
///
/// The root is never bound in this process: t3-git also pins identities in a process-wide cache,
/// which a test inside one process cannot clear, so only a root this process never bound shows
/// what a restart leaves, where the saved record is the only protection.
#[tokio::test]
async fn a_repository_planted_at_the_bound_project_root_refuses_a_worktree_launch() {
    use t3_orchestration::api::RepositoryBindingStore;
    let mut s = Server::start().await;
    // The repository the record names (bound through t3-git, as the server binds).
    let recorded = s.home().join("recorded");
    repository(&recorded);
    let binding = t3_git::Git::new(&recorded)
        .bind_repository()
        .await
        .unwrap()
        .expect("a repository");

    // The project folder holds a different, hostile repository.
    let project = s.home().join("planted");
    repository(&project);
    let marker = s.home().join("planted-marker");
    let marker_text = marker.to_string_lossy().replace('\\', "/");
    std::fs::write(project.join(".gitattributes"), "*.txt filter=t3\n").unwrap();
    git(&project, &["add", ".gitattributes"]);
    git(&project, &["commit", "-q", "-m", "attributes"]);
    let hook = format!("#!/bin/sh\necho hook >> '{marker_text}'\n");
    for name in ["post-checkout", "reference-transaction"] {
        std::fs::write(project.join(".git").join("hooks").join(name), &hook).unwrap();
    }
    git(
        &project,
        &[
            "config",
            "filter.t3.smudge",
            &format!("sh -c 'echo filter >> \"{marker_text}\"; cat'"),
        ],
    );
    t3_orchestration::runtime::repository_bindings::FileRepositoryBindings::new(bindings_file(&s))
        .put(&project, json!({"repository": binding}))
        .await
        .unwrap();
    s.insert_project("project-planted", &project, "[]");

    let launched = s
        .call(
            "orchestration.launchThread",
            launch(
                "thread:planted",
                "project-planted",
                json!({"type": "worktree", "baseRef": "main", "branch": "feature/planted"}),
                "auto-accept-edits",
            ),
        )
        .await;
    if matches!(launched, Exit::Success { .. }) {
        // Accepted (before the fix): let the preparation finish so the planted hooks have had
        // their chance to run.
        let setup = s.subscribe(
            "subscribeWorktreeSetup",
            json!({"threadId": "thread:planted"}),
        );
        s.until(&setup, |snapshot| {
            !snapshot.is_null() && snapshot["phase"] != "running"
        })
        .await;
    }
    assert!(!marker.exists(), "a planted hook or filter ran");
    assert!(
        !git(&project, &["branch", "--list"]).contains("feature/planted"),
        "a branch was created in the planted repository"
    );
    let trees = s.home().join("worktrees").join("planted");
    assert!(
        std::fs::read_dir(&trees).map_or(true, |mut entries| entries.next().is_none()),
        "a worktree was created"
    );
    let error = launch_refused(launched, "no longer belongs to the repository");
    assert!(
        error
            .to_string()
            .contains("checkpoint.git.repository-changed"),
        "{error}"
    );
    s.runtime.take().unwrap().shutdown().await.unwrap();
}

/// A normal worktree launch records the new checkout's binding before its setup script runs
/// (and so before the first turn): the script itself looks for its folder in the bindings file.
#[tokio::test]
async fn a_launched_worktree_is_bound_before_its_setup_script_runs() {
    let mut s = Server::start().await;
    let project = s.home().join("bound");
    repository(&project);
    let file = bindings_file(&s).to_string_lossy().into_owned();
    let command = if cfg!(windows) {
        format!(
            "$leaf = Split-Path -Leaf (Get-Location).Path; if (Select-String -Quiet -SimpleMatch -Pattern $leaf -Path '{file}') {{ Write-Output t3-bound-before-setup }} else {{ Write-Output t3-not-bound }}"
        )
    } else {
        format!(
            "if grep -qF \"$(basename \"$PWD\")\" '{file}'; then echo t3-bound-before-setup; else echo t3-not-bound; fi"
        )
    };
    let scripts = json!([{"id":"setup","name":"Setup","command":command,"icon":"configure","runOnWorktreeCreate":true,"async":false}]).to_string();
    s.insert_project("project-bound", &project, &scripts);
    let setup = s.subscribe(
        "subscribeWorktreeSetup",
        json!({"threadId": "thread:bound"}),
    );
    ok(s.call(
        "orchestration.launchThread",
        launch(
            "thread:bound",
            "project-bound",
            json!({"type": "worktree", "baseRef": "main", "branch": "feature/bound"}),
            "auto-accept-edits",
        ),
    )
    .await);
    let done = s
        .until(&setup, |snapshot| {
            !snapshot.is_null() && snapshot["phase"] != "running"
        })
        .await;
    assert_eq!(done["phase"], "done", "{done}");
    let tail = stage(&done, "setup-script")["tail"].to_string();
    assert!(tail.contains("t3-bound-before-setup"), "{done}");
    first_turn_completes(&mut s, "thread:bound").await;
    let leaf = PathBuf::from(done["worktreePath"].as_str().unwrap())
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        std::fs::read_to_string(bindings_file(&s))
            .unwrap()
            .contains(&leaf)
    );
    s.runtime.take().unwrap().shutdown().await.unwrap();
}
