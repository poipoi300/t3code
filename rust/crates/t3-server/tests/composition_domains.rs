//! Package A: the workspace, terminal, source-control and scheduler domains assembled by the
//! composition root, driven through real HTTP and WebSocket requests, and the workspace
//! authority's allow and deny paths (port of `workspace/KnownWorkspaces.ts`).
#![cfg(all(
    feature = "local-services",
    feature = "workspace",
    feature = "terminal",
    feature = "git",
    feature = "scheduler"
))]
#![allow(clippy::unwrap_used, clippy::expect_used)]
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::path::Path;
use t3_rpc::{CallEvent, CallKind, Client, Exit, Frame};
use t3_server::adapters::config::{ConfigInputs, ServerFlags};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

struct Server {
    temp: tempfile::TempDir,
    runtime: t3_server::runtime::RunningServer,
    origin: String,
    cookie: String,
    socket: Socket,
    client: Client,
}

fn git(cwd: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(["-c", "user.name=t3", "-c", "user.email=t3@example.invalid"])
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", cwd.join(".no-global-config"))
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn repository(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    git(path, &["init", "-q", "-b", "main"]);
    std::fs::write(path.join("hello.txt"), "hello\n").unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "-q", "-m", "init"]);
}

impl Server {
    async fn start() -> Self {
        Self::start_with(|_| Vec::new()).await
    }
    /// `extra` adds environment entries, given the home before the server starts.
    async fn start_with(extra: impl FnOnce(&Path) -> Vec<(String, String)>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        // The terminal's shell: without PATH it resolves ComSpec (cmd.exe), so no user shell
        // profile from the real account can load. Nothing else from this process is passed.
        let env = ["SystemRoot", "ComSpec", "WINDIR"]
            .into_iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_owned(), v)))
            .chain(extra(home))
            .collect();
        let runtime = t3_server::runtime::start(ConfigInputs {
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
        })
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
            runtime,
            origin,
            cookie,
            socket,
            client: Client::new(0, Default::default()),
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
    }
    /// A unary call's exit, acknowledging chunks of other open streams meanwhile.
    async fn call(&mut self, tag: &str, payload: Value) -> Exit {
        let id = self.client.start(tag, payload, CallKind::Unary).unwrap();
        loop {
            match self.event().await {
                CallEvent::Exit { id: done, exit } if done == id => return exit,
                CallEvent::Chunk { id, .. } => self.client.acknowledge(id),
                _ => {}
            }
        }
    }
    fn database(&self) -> t3_persistence::rusqlite::Connection {
        let layout = t3_config::home::HomeLayout {
            base_dir: self.home().into(),
            state_dir: self.home().join("userdata"),
        };
        t3_persistence::rusqlite::Connection::open(layout.database()).unwrap()
    }
    fn insert_project(&self, id: &str, root: &Path) {
        self.database()
            .execute(
                "INSERT INTO projection_projects
                 (project_id, title, workspace_root, scripts_json, created_at, updated_at)
                 VALUES (?1, ?1, ?2, '[]', '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
                (id, root.to_string_lossy()),
            )
            .unwrap();
    }
    fn insert_thread(&self, id: &str, project: &str, worktree: &Path) {
        let payload = json!({"worktreePath": worktree.to_string_lossy()}).to_string();
        self.database()
            .execute(
                "INSERT INTO orchestration_v2_projection_threads
                 (thread_id, project_id, title, default_provider, runtime_mode, interaction_mode,
                  created_at, updated_at, payload_json)
                 VALUES (?1, ?2, ?1, 'codex', 'full-access', 'default',
                  '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z', ?3)",
                (id, project, payload),
            )
            .unwrap();
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
    assert!(!value.is_null(), "expected a failure, got {encoded}");
    value["_tag"].as_str().unwrap_or("").to_owned()
}

fn read(cwd: &Path) -> Value {
    json!({"cwd": cwd.to_string_lossy(), "relativePath": "hello.txt"})
}

#[tokio::test]
async fn workspace_files_follow_the_environment_authority_and_deny_everything_else() {
    let mut s = Server::start().await;
    let managed = s.home().join("worktrees").join("repo");
    repository(&managed);
    let outside = s.home().join("outside");
    repository(&outside);
    let project = s.home().join("project");
    repository(&project);

    // A managed worktree folder is a workspace from the start.
    let contents = ok(s.unary("projects.readFile", read(&managed)).await);
    assert_eq!(contents["contents"], "hello\n");

    // Anything else is refused, for reads and for writes, and the write leaves no file.
    assert_ne!(
        failure(s.unary("projects.readFile", read(&outside)).await),
        ""
    );
    let write =
        json!({"cwd": outside.to_string_lossy(), "relativePath": "planted.txt", "contents": "x"});
    assert_ne!(failure(s.unary("projects.writeFile", write).await), "");
    assert!(!outside.join("planted.txt").exists());
    // `..` cannot climb out of a granted root.
    let climb =
        json!({"cwd": managed.to_string_lossy(), "relativePath": "../../outside/hello.txt"});
    assert_ne!(failure(s.unary("projects.readFile", climb).await), "");

    // A live project grants its root, re-read on every call; deleting it revokes at once.
    s.insert_project("project-1", &project);
    ok(s.unary("projects.readFile", read(&project)).await);
    s.database()
        .execute(
            "UPDATE projection_projects SET deleted_at = '2026-01-02T00:00:00.000Z' WHERE project_id = 'project-1'",
            (),
        )
        .unwrap();
    assert_ne!(
        failure(s.unary("projects.readFile", read(&project)).await),
        ""
    );
    s.database()
        .execute("UPDATE projection_projects SET deleted_at = NULL", ())
        .unwrap();

    // A thread worktree counts only when Git lists it for the thread's project.
    let linked = s.home().join("linked");
    git(
        &project,
        &["worktree", "add", "-q", linked.to_str().unwrap()],
    );
    s.insert_thread("thread-linked", "project-1", &linked);
    ok(s.unary("projects.readFile", read(&linked)).await);
    s.insert_thread("thread-forged", "project-1", &outside);
    assert_ne!(
        failure(s.unary("projects.readFile", read(&outside)).await),
        ""
    );

    // Asset URLs are signed for the same authority and served by the mounted HTTP route.
    std::fs::write(managed.join("page.html"), "<p>granted</p>").unwrap();
    let url = ok(s
        .unary(
            "assets.createUrl",
            json!({"resource": {"_tag": "draft-workspace-file", "cwd": managed.to_string_lossy(), "path": "page.html"}}),
        )
        .await);
    let href = url["relativeUrl"].as_str().unwrap();
    let href = if href.starts_with("http") {
        href.to_owned()
    } else {
        format!("{}{href}", s.origin)
    };
    let served = reqwest::Client::new()
        .get(href)
        .header("cookie", &s.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(served.status(), 200);
    assert_eq!(served.text().await.unwrap(), "<p>granted</p>");
    let refused = s
        .unary(
            "assets.createUrl",
            json!({"resource": {"_tag": "draft-workspace-file", "cwd": outside.to_string_lossy(), "path": "page.html"}}),
        )
        .await;
    assert_ne!(failure(refused), "");
    s.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn terminals_attach_per_socket_and_stream_while_the_client_acknowledges() {
    let mut s = Server::start().await;
    let managed = s.home().join("worktrees").join("term");
    std::fs::create_dir_all(&managed).unwrap();
    let session = json!({"threadId": "thread-t", "terminalId": "term-1"});
    let mut open = session.clone();
    open["cwd"] = json!(managed.to_string_lossy());
    open["cols"] = json!(80);
    open["rows"] = json!(24);
    ok(s.unary("terminal.open", open).await);
    s.client
        .start("terminal.attach", session.clone(), CallKind::Stream)
        .unwrap();
    let mut write = session.clone();
    // Shell-agnostic: the echoed command line and the printed line each carry the marker.
    write["data"] = json!(if cfg!(windows) {
        "echo socket-marker\r\n"
    } else {
        "echo socket-marker\n"
    });
    // The write's exit and the attach stream's chunks arrive interleaved.
    let write_id = s
        .client
        .start("terminal.write", write, CallKind::Unary)
        .unwrap();
    let (mut written, mut output) = (false, String::new());
    while !(written && output.matches("socket-marker").count() >= 2) {
        match s.event().await {
            CallEvent::Exit { id, exit } if id == write_id => {
                ok(exit);
                written = true;
            }
            CallEvent::Chunk { id, values } => {
                s.client.acknowledge(id);
                for value in values {
                    if let Some(data) = value["data"].as_str() {
                        output.push_str(data);
                    }
                }
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    s.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn source_control_serves_granted_repositories_and_refuses_others() {
    let mut s = Server::start().await;
    let managed = s.home().join("worktrees").join("repo");
    repository(&managed);
    let outside = s.home().join("outside");
    repository(&outside);
    let refs = ok(s
        .unary("vcs.listRefs", json!({"cwd": managed.to_string_lossy()}))
        .await);
    assert!(refs.to_string().contains("main"), "{refs}");
    let denied = s
        .unary("vcs.listRefs", json!({"cwd": outside.to_string_lossy()}))
        .await;
    assert_eq!(failure(denied), "EnvironmentAuthorizationError");
    let created = s
        .unary(
            "vcs.createRef",
            json!({"cwd": outside.to_string_lossy(), "refName": "planted"}),
        )
        .await;
    assert_eq!(failure(created), "EnvironmentAuthorizationError");
    s.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn scheduled_tasks_and_their_webhook_route_are_served() {
    let mut s = Server::start().await;
    s.client
        .start("scheduledTasks.subscribe", json!({}), CallKind::Stream)
        .unwrap();
    let list = s.chunk().await;
    assert!(
        list.to_string().contains("tasks") || list.is_array() || list.is_object(),
        "{list}"
    );
    let http = reqwest::Client::new();
    let head = http
        .head(format!("{}/api/hooks/hook-1/token-1", s.origin))
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), 405);
    assert!(head.headers()["allow"].to_str().unwrap().contains("POST"));
    let post = http
        .post(format!("{}/api/hooks/hook-1/token-1", s.origin))
        .body("{}")
        .send()
        .await
        .unwrap();
    // Without command intake the webhook adapter fails closed for every hook (package G).
    assert_eq!(post.status(), 500);
    assert_eq!(post.headers()["x-t3-hook-outcome"], "error");
    s.runtime.shutdown().await.unwrap();
}

/// t3-host services: database storage's status stream and its reclaim refusal (no supervisor
/// can restart this server with the database closed), and the scope checks.
#[cfg(feature = "host")]
#[tokio::test]
async fn host_database_storage_and_scoped_host_services() {
    let mut s = Server::start().await;
    s.client
        .start("server.getDatabaseStorage", json!({}), CallKind::Stream)
        .unwrap();
    let status = s.chunk().await;
    assert!(status["databaseBytes"].as_f64().unwrap() > 0.0, "{status}");
    assert_eq!(status["canReclaim"], false, "{status}");
    let command = status["compactCommand"].as_str().unwrap();
    assert!(command.starts_with("t3 doctor --base-dir '"), "{command}");

    let refused = s.call("server.reclaimDatabaseSpace", json!({})).await;
    let encoded = serde_json::to_value(&refused).unwrap();
    let error = &encoded["cause"][0]["error"];
    assert_eq!(error["_tag"], "DatabaseReclaimError", "{encoded}");
    assert_eq!(
        error["reason"],
        format!("Stop T3 Code on this machine, then run: {command}")
    );

    // A read-only session: no reclaim (environment:maintain) and no triage (orchestration:operate).
    s.reconnect_with(&["orchestration:read"]).await;
    for (tag, payload) in [
        ("server.reclaimDatabaseSpace", json!({})),
        ("server.prepareTriage", json!({"client": {}})),
    ] {
        let exit = s.call(tag, payload).await;
        assert_eq!(failure(exit), "EnvironmentAuthorizationError", "{tag}");
    }
    s.runtime.shutdown().await.unwrap();
}

/// Devices: one runtime when a Node runtime is on PATH (here a stand-in file nothing runs),
/// unsupported without one. Device support stays off, so no host or hub starts.
#[cfg(feature = "device")]
#[tokio::test]
async fn devices_are_served_with_a_node_runtime_and_unsupported_without() {
    let mut absent = Server::start().await;
    let exit = absent
        .call("device.list", json!({"inspectOnly": true}))
        .await;
    let encoded = serde_json::to_value(&exit).unwrap();
    assert_eq!(
        encoded["cause"][0]["defect"], "Unknown request tag: device.list",
        "{encoded}"
    );
    let without_node = absent.runtime.rpc_methods_served;
    absent.runtime.shutdown().await.unwrap();

    let mut s = Server::start_with(|home| {
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let node = if cfg!(windows) { "node.exe" } else { "node" };
        std::fs::write(bin.join(node), "").unwrap();
        vec![("PATH".into(), bin.to_string_lossy().into_owned())]
    })
    .await;
    // The eleven device methods are the only difference a Node runtime makes.
    let with_node = s.runtime.rpc_methods_served;
    println!("RPC methods served: {without_node} without Node, {with_node} with Node (of 220)");
    assert_eq!(with_node, without_node + 11);
    s.client
        .start("subscribeDeviceState", json!({}), CallKind::Stream)
        .unwrap();
    let state = s.chunk().await;
    assert!(state["hosts"].is_array(), "{state}");
    let listed = ok(s.call("device.list", json!({"inspectOnly": true})).await);
    assert!(listed["hosts"].is_array(), "{listed}");
    let configured = ok(s.call("device.configure", json!({"enabled": false})).await);
    assert!(configured["hosts"].is_array(), "{configured}");

    // The hub proxy: its policy, then authentication, then readiness (nothing is running).
    let http = reqwest::Client::new();
    let hub = format!("{}/api/device-hub/api/devices", s.origin);
    assert_eq!(http.get(&hub).send().await.unwrap().status(), 401);
    let authorized = http
        .get(&hub)
        .header("cookie", &s.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(authorized.status(), 503);
    let unknown = format!("{}/api/device-hub/not-a-route", s.origin);
    assert_eq!(http.get(&unknown).send().await.unwrap().status(), 404);

    // A read-only session may list but not configure (settings:write).
    s.reconnect_with(&["orchestration:read"]).await;
    ok(s.call("device.list", json!({"inspectOnly": true})).await);
    let refused = s.call("device.configure", json!({"enabled": false})).await;
    assert_eq!(failure(refused), "EnvironmentAuthorizationError");
    s.runtime.shutdown().await.unwrap();
}
