//! The VCS status, branch and worktree-setup group (`subscribeVcsBranch` …
//! `vcs.createWorktree`) served by a started server over a real socket, against real
//! temporary repositories.
#![cfg(all(
    feature = "local-services",
    feature = "workspace",
    feature = "git",
    feature = "orchestration"
))]
#![allow(clippy::unwrap_used, clippy::expect_used)]
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use t3_rpc::{CallEvent, CallKind, Client, Exit, Frame, RequestId};
use t3_server::adapters::config::{ConfigInputs, ServerFlags};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const METHODS: [&str; 7] = [
    "subscribeVcsBranch",
    "vcs.refreshBranch",
    "subscribeVcsStatus",
    "vcs.refreshStatus",
    "subscribeWorktreeSetup",
    "worktreeSetup.cancel",
    "vcs.createWorktree",
];

struct Server {
    temp: tempfile::TempDir,
    runtime: Option<t3_server::runtime::RunningServer>,
    origin: String,
    cookie: String,
    socket: Socket,
    client: Client,
    /// Chunks of streams other than the one being read, in arrival order.
    chunks: Vec<(RequestId, Value)>,
    exits: Vec<(RequestId, Exit)>,
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

/// The workspace authority's refusal of a directory outside every granted root.
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

/// The RPC guard's refusal of a session that lacks the method's scope, which it names exactly as
/// the required permission (`requiredScope` is its legacy parent).
fn missing_scope(exit: Exit, scope: &str) {
    let error = failure(exit);
    assert_eq!(error["_tag"], "EnvironmentAuthorizationError", "{error}");
    assert_eq!(error["requiredPermission"], scope, "{error}");
}

/// The lexical refusal of a cwd that is not a path on a local disk, before any filesystem call.
fn not_local_disk(exit: Exit) {
    let error = failure(exit);
    assert_eq!(error["_tag"], "EnvironmentAuthorizationError", "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("must be a folder on this server's local disks"),
        "{error}"
    );
}

/// `t3-auth`'s standard scopes: every one a pairing token can carry.
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

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap()
}

impl Server {
    async fn start() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let env = ["SystemRoot", "ComSpec", "WINDIR"]
            .into_iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_owned(), v)))
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
    fn runtime(&self) -> &t3_server::runtime::RunningServer {
        self.runtime.as_ref().unwrap()
    }
    /// A fresh connection for the same full session.
    async fn reconnect(&mut self) {
        self.socket = connect(&self.origin, self.runtime().address, &self.cookie).await;
        self.client = Client::new(0, Default::default());
        self.chunks.clear();
        self.exits.clear();
    }
    /// Opens a second connection for the same session and returns the current one, still open.
    async fn another_connection(&mut self) -> Socket {
        let next = connect(&self.origin, self.runtime().address, &self.cookie).await;
        self.client = Client::new(0, Default::default());
        self.chunks.clear();
        self.exits.clear();
        std::mem::replace(&mut self.socket, next)
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
    /// The next frame's events, or `None` once the server closed the connection.
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
    /// Records one event: chunks are acknowledged as a real client does.
    async fn pump(&mut self) {
        match self.event().await.expect("connection closed") {
            CallEvent::Chunk { id, values } => {
                self.client.acknowledge(id.clone());
                self.chunks
                    .extend(values.into_iter().map(|value| (id.clone(), value)));
            }
            CallEvent::Exit { id, exit } => self.exits.push((id, exit)),
            // The client's own record of a call this test interrupted.
            CallEvent::Failed {
                error: t3_rpc::ClientError::Interrupted,
                ..
            } => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    fn subscribe(&mut self, tag: &str, payload: Value) -> RequestId {
        self.client.start(tag, payload, CallKind::Stream).unwrap()
    }
    /// The next value of stream `id`; the stream's exit if it ended instead.
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

#[tokio::test]
async fn branch_and_status_streams_follow_a_real_repository() {
    let mut s = Server::start().await;
    for method in METHODS {
        assert!(
            !s.runtime().unserved_methods.contains(&method),
            "{method} is unserved"
        );
    }
    // A managed worktree folder is a workspace from the start.
    let repo = s.home().join("worktrees").join("repo");
    repository(&repo);
    let cwd = json!({"cwd": repo.to_string_lossy()});

    let branch = s.subscribe("subscribeVcsBranch", cwd.clone());
    let first = s.next(&branch).await.unwrap();
    assert_eq!(first["refName"], "main", "{first}");
    let status = s.subscribe(
        "subscribeVcsStatus",
        json!({"cwd": repo.to_string_lossy(), "includeRemote": false}),
    );
    let snapshot = s.next(&status).await.unwrap();
    assert_eq!(snapshot["_tag"], "snapshot", "{snapshot}");
    assert_eq!(snapshot["local"]["isRepo"], true);
    assert_eq!(snapshot["local"]["hasWorkingTreeChanges"], false);

    // A new untracked file: the refresh answers with it and the stream publishes it.
    std::fs::write(repo.join("new.txt"), "new\n").unwrap();
    let refreshed = ok(s
        .call(
            "vcs.refreshStatus",
            json!({"cwd": repo.to_string_lossy(), "localOnly": true}),
        )
        .await);
    assert_eq!(refreshed["hasWorkingTreeChanges"], true, "{refreshed}");
    let update = s.next(&status).await.unwrap();
    assert_eq!(update["_tag"], "localUpdated", "{update}");
    assert_eq!(update["local"]["hasWorkingTreeChanges"], true);

    // A commit cleans the tree again.
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "add new"]);
    ok(s.call(
        "vcs.refreshStatus",
        json!({"cwd": repo.to_string_lossy(), "localOnly": true}),
    )
    .await);
    let committed = s.next(&status).await.unwrap();
    assert_eq!(committed["_tag"], "localUpdated", "{committed}");
    assert_eq!(committed["local"]["hasWorkingTreeChanges"], false);

    // A branch switch reaches the branch stream (HEAD watcher or the explicit refresh).
    git(&repo, &["switch", "-q", "-c", "feature"]);
    assert_eq!(
        ok(s.call("vcs.refreshBranch", cwd.clone()).await),
        Value::Null
    );
    let switched = s.next(&branch).await.unwrap();
    assert_eq!(switched["refName"], "feature", "{switched}");

    // Closing the socket drops both subscriptions; a new connection subscribes afresh.
    s.socket.close(None).await.unwrap();
    s.reconnect().await;
    let branch = s.subscribe("subscribeVcsBranch", cwd);
    assert_eq!(s.next(&branch).await.unwrap()["refName"], "feature");

    // Shutdown ends the open stream and completes with nothing left running.
    let runtime = s.runtime.take().unwrap();
    let shutdown = tokio::spawn(runtime.shutdown());
    loop {
        if let Some(index) = s.exits.iter().position(|(exit, _)| exit == &branch) {
            s.exits.remove(index);
            break;
        }
        match s.event().await {
            None => break,
            Some(CallEvent::Chunk { id, .. }) => s.client.acknowledge(id),
            Some(CallEvent::Exit { id, exit }) => s.exits.push((id, exit)),
            Some(other) => panic!("unexpected {other:?}"),
        }
    }
    shutdown.await.unwrap().unwrap();
}

#[tokio::test]
async fn status_methods_refuse_unauthorized_directories_and_missing_scopes() {
    let mut s = Server::start().await;
    let outside = s.home().join("outside");
    repository(&outside);
    let cwd = json!({"cwd": outside.to_string_lossy()});

    let branch = s.subscribe("subscribeVcsBranch", cwd.clone());
    outside_workspace(s.next(&branch).await.unwrap_err());
    let status = s.subscribe("subscribeVcsStatus", cwd.clone());
    outside_workspace(s.next(&status).await.unwrap_err());
    outside_workspace(s.call("vcs.refreshBranch", cwd.clone()).await);
    outside_workspace(s.call("vcs.refreshStatus", cwd.clone()).await);
    // A project's own root is granted once the project exists.
    s.insert_project("project-1", &outside);
    ok(s.call("vcs.refreshStatus", cwd.clone()).await);

    s.reconnect_with(&["filesystem:read"]).await;
    let branch = s.subscribe("subscribeVcsBranch", cwd.clone());
    missing_scope(s.next(&branch).await.unwrap_err(), "orchestration:read");
    let status = s.subscribe("subscribeVcsStatus", cwd.clone());
    missing_scope(s.next(&status).await.unwrap_err(), "orchestration:read");
    missing_scope(
        s.call("vcs.refreshBranch", cwd.clone()).await,
        "orchestration:read",
    );
    missing_scope(s.call("vcs.refreshStatus", cwd).await, "orchestration:read");
    s.socket.close(None).await.unwrap();
    s.runtime.take().unwrap().shutdown().await.unwrap();
}

#[tokio::test]
async fn worktrees_are_created_where_the_server_decides() {
    let mut s = Server::start().await;
    let project = s.home().join("project");
    repository(&project);
    s.insert_project("project-1", &project);
    let create = |extra: Value| {
        let mut payload =
            json!({"cwd": project.to_string_lossy(), "refName": "main", "path": null});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        payload
    };

    // The default managed folder, keyed by repository name; Git registers the checkout.
    let created = ok(s
        .call(
            "vcs.createWorktree",
            create(json!({"newRefName": "feature/one"})),
        )
        .await);
    assert_eq!(created["worktree"]["refName"], "feature/one");
    let path = PathBuf::from(created["worktree"]["path"].as_str().unwrap());
    assert!(
        canonical(&path).starts_with(canonical(&s.home().join("worktrees")).join("project")),
        "{}",
        path.display()
    );
    assert!(path.join("hello.txt").exists());
    let listed = std::process::Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&listed.stdout).contains("refs/heads/feature/one"));

    // The configured folder wins, and a client-chosen destination does not decode.
    let custom = s.home().join("custom-trees");
    ok(s.call(
        "server.updateSettings",
        json!({"patch": {"worktreesDirectory": custom.to_string_lossy()}}),
    )
    .await);
    let created = ok(s
        .call(
            "vcs.createWorktree",
            create(json!({"newRefName": "feature/two"})),
        )
        .await);
    let path = PathBuf::from(created["worktree"]["path"].as_str().unwrap());
    assert!(
        canonical(&path).starts_with(canonical(&custom).join("project")),
        "{}",
        path.display()
    );
    let elsewhere = s.home().join("elsewhere");
    let chosen = s
        .call(
            "vcs.createWorktree",
            create(json!({"newRefName": "feature/three", "path": elsewhere.to_string_lossy()})),
        )
        .await;
    failure(chosen);
    assert!(!elsewhere.exists());

    // Typed failures: an unknown base ref, and a worktree location that is not absolute.
    let missing = failure(
        s.call(
            "vcs.createWorktree",
            create(json!({"refName": "no-such-ref", "newRefName": "feature/four"})),
        )
        .await,
    );
    assert_eq!(missing["_tag"], "GitCommandError", "{missing}");
    ok(s.call(
        "server.updateSettings",
        json!({"patch": {"worktreesDirectory": "relative/trees"}}),
    )
    .await);
    let relative = failure(
        s.call(
            "vcs.createWorktree",
            create(json!({"newRefName": "feature/five"})),
        )
        .await,
    );
    assert_eq!(relative["_tag"], "GitCommandError", "{relative}");
    assert!(
        relative["detail"]
            .as_str()
            .unwrap()
            .contains("must be an absolute folder on this machine"),
        "{relative}"
    );

    // Outside every workspace, and without the source-control scope.
    let outside = s.home().join("outside");
    repository(&outside);
    outside_workspace(
        s.call(
            "vcs.createWorktree",
            json!({"cwd": outside.to_string_lossy(), "refName": "main", "path": null}),
        )
        .await,
    );
    s.reconnect_with(&["orchestration:read", "orchestration:operate"])
        .await;
    missing_scope(
        s.call(
            "vcs.createWorktree",
            create(json!({"newRefName": "feature/six"})),
        )
        .await,
        "source-control:write",
    );
    s.socket.close(None).await.unwrap();
    s.runtime.take().unwrap().shutdown().await.unwrap();
}

/// No producer in this server begins a setup (thread launch into a new worktree is not ported),
/// so a thread has none: the stream starts with null and cancelling finds nothing.
#[tokio::test]
async fn worktree_setup_subscriptions_and_cancel() {
    let mut s = Server::start().await;
    let thread = json!({"threadId": "thread-1"});
    let setups = s.subscribe("subscribeWorktreeSetup", thread.clone());
    assert_eq!(s.next(&setups).await.unwrap(), Value::Null);
    assert_eq!(
        ok(s.call("worktreeSetup.cancel", thread.clone()).await),
        json!({"cancelled": false})
    );
    // Malformed input is refused before any handler runs.
    failure(s.call("worktreeSetup.cancel", json!({})).await);

    s.reconnect_with(&["filesystem:read"]).await;
    let setups = s.subscribe("subscribeWorktreeSetup", thread.clone());
    missing_scope(s.next(&setups).await.unwrap_err(), "orchestration:read");
    s.reconnect_with(&["orchestration:read"]).await;
    missing_scope(
        s.call("worktreeSetup.cancel", thread).await,
        "orchestration:operate",
    );
    s.socket.close(None).await.unwrap();
    s.runtime.take().unwrap().shutdown().await.unwrap();
}

/// Review M2: a cwd that is not a path on a local disk is refused by its shape alone, so the
/// server never opens it (a UNC or WebDAV path would authenticate to its host).
#[tokio::test]
async fn non_local_cwds_are_refused_before_the_filesystem() {
    let mut s = Server::start().await;
    let mut shapes = vec!["relative\\repo".to_owned(), "relative/repo".to_owned()];
    if cfg!(windows) {
        shapes.extend(
            [
                r"\\attacker.invalid\share",
                r"\\attacker.invalid@80\dav",
                r"\\?\UNC\attacker.invalid\share",
                r"\\?\C:\Windows",
                r"\\.\C:\Windows",
                r"\\.\pipe\t3",
                r"C:Windows",
                r"\Windows",
            ]
            .map(str::to_owned),
        );
    }
    for shape in &shapes {
        let cwd = json!({"cwd": shape});
        let status = s.subscribe("subscribeVcsStatus", cwd.clone());
        not_local_disk(s.next(&status).await.unwrap_err());
        let branch = s.subscribe("subscribeVcsBranch", cwd.clone());
        not_local_disk(s.next(&branch).await.unwrap_err());
        not_local_disk(s.call("vcs.refreshBranch", cwd.clone()).await);
        not_local_disk(s.call("vcs.refreshStatus", cwd.clone()).await);
        not_local_disk(
            s.call(
                "vcs.createWorktree",
                json!({"cwd": shape, "refName": "main", "path": null}),
            )
            .await,
        );
        // The older source-control methods share the same check.
        not_local_disk(s.call("vcs.listRefs", cwd).await);
    }
    // An ordinary authorised cwd still works.
    let repo = s.home().join("worktrees").join("repo");
    repository(&repo);
    let refreshed = ok(s
        .call("vcs.refreshStatus", json!({"cwd": repo.to_string_lossy()}))
        .await);
    assert_eq!(refreshed["isRepo"], true, "{refreshed}");
    s.socket.close(None).await.unwrap();
    s.runtime.take().unwrap().shutdown().await.unwrap();
}

/// Review L7: a subscription re-checks the workspace authority before each value, so removing
/// its project ends it with the typed refusal.
#[tokio::test]
async fn removing_a_project_ends_its_streams_with_the_refusal() {
    let mut s = Server::start().await;
    let project = s.home().join("project");
    repository(&project);
    s.insert_project("project-1", &project);
    let branch = s.subscribe(
        "subscribeVcsBranch",
        json!({"cwd": project.to_string_lossy()}),
    );
    assert_eq!(s.next(&branch).await.unwrap()["refName"], "main");
    let status = s.subscribe(
        "subscribeVcsStatus",
        json!({"cwd": project.to_string_lossy(), "includeRemote": false}),
    );
    assert_eq!(s.next(&status).await.unwrap()["_tag"], "snapshot");

    s.database()
        .execute(
            "UPDATE projection_projects SET deleted_at = '2026-01-02T00:00:00.000Z'
             WHERE project_id = 'project-1'",
            (),
        )
        .unwrap();
    // The next change (a branch switch, seen by the HEAD watcher) is not delivered.
    git(&project, &["switch", "-q", "-c", "moved"]);
    outside_workspace(s.next(&branch).await.unwrap_err());
    outside_workspace(s.next(&status).await.unwrap_err());
    s.socket.close(None).await.unwrap();
    s.runtime.take().unwrap().shutdown().await.unwrap();
}

/// Each method is refused for a session holding every standard scope but its own.
#[tokio::test]
async fn each_method_refuses_a_session_missing_exactly_its_scope() {
    let mut s = Server::start().await;
    let repo = s.home().join("worktrees").join("repo");
    repository(&repo);
    let cwd = repo.to_string_lossy().into_owned();
    let thread = json!({"threadId": "thread-1"});
    let cases = [
        (
            "subscribeVcsBranch",
            json!({"cwd": cwd}),
            "orchestration:read",
            true,
        ),
        (
            "vcs.refreshBranch",
            json!({"cwd": cwd}),
            "orchestration:read",
            false,
        ),
        (
            "subscribeVcsStatus",
            json!({"cwd": cwd}),
            "orchestration:read",
            true,
        ),
        (
            "vcs.refreshStatus",
            json!({"cwd": cwd}),
            "orchestration:read",
            false,
        ),
        (
            "subscribeWorktreeSetup",
            thread.clone(),
            "orchestration:read",
            true,
        ),
        (
            "worktreeSetup.cancel",
            thread,
            "orchestration:operate",
            false,
        ),
        (
            "vcs.createWorktree",
            json!({"cwd": cwd, "refName": "main", "newRefName": "feature/x", "path": null}),
            "source-control:write",
            false,
        ),
    ];
    for (method, payload, scope, stream) in cases {
        let others: Vec<&str> = STANDARD_SCOPES
            .into_iter()
            .filter(|other| *other != scope)
            .collect();
        s.reconnect_with(&others).await;
        let exit = if stream {
            let id = s.subscribe(method, payload);
            s.next(&id).await.unwrap_err()
        } else {
            s.call(method, payload).await
        };
        missing_scope(exit, scope);
    }
    // Refused before running: no worktree was created.
    let listed = std::process::Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&listed.stdout).contains("feature/x"));
    s.socket.close(None).await.unwrap();
    s.runtime.take().unwrap().shutdown().await.unwrap();
}

/// Re-review finding 4: a link inside an authorised workspace whose target is a share is refused
/// before it is followed, so neither the network nor Git is reached. The link names a host
/// under `.invalid`, so no real share can answer even without the fix.
#[cfg(windows)]
#[tokio::test]
async fn a_link_to_a_share_inside_a_workspace_is_refused_before_it_is_followed() {
    let mut s = Server::start().await;
    let repo = s.home().join("worktrees").join("repo");
    repository(&repo);
    let link = repo.join("share");
    std::os::windows::fs::symlink_dir(r"\\attacker.invalid\x", &link).unwrap();
    for cwd in [link.clone(), link.join("nested")] {
        let cwd = cwd.to_string_lossy().into_owned();
        let status = s.subscribe("subscribeVcsStatus", json!({"cwd": cwd}));
        not_local_disk(s.next(&status).await.unwrap_err());
        not_local_disk(s.call("vcs.refreshStatus", json!({"cwd": cwd})).await);
        not_local_disk(s.call("vcs.listRefs", json!({"cwd": cwd})).await);
        not_local_disk(
            s.call(
                "vcs.createWorktree",
                json!({"cwd": cwd, "refName": "main", "newRefName": "feature/link", "path": null}),
            )
            .await,
        );
    }
    // The workspace itself still resolves (Git does not follow the link).
    let refreshed = ok(s
        .call("vcs.refreshStatus", json!({"cwd": repo.to_string_lossy()}))
        .await);
    assert_eq!(refreshed["isRepo"], true, "{refreshed}");
    s.socket.close(None).await.unwrap();
    s.runtime.take().unwrap().shutdown().await.unwrap();
}

/// Each live status or branch subscription keeps a status worker per distinct checkout, so
/// their number is bounded per connection and per server (TypeScript has no bound).
const SUBSCRIPTIONS_PER_CONNECTION: usize = 128;
const SUBSCRIPTIONS_PER_SERVER: usize = 512;

fn over_the_limit(exit: Exit, scope: &str) {
    let error = failure(exit);
    assert_eq!(error["_tag"], "GitCommandError", "{error}");
    let detail = error["detail"].as_str().unwrap();
    assert!(
        detail.contains("Too many VCS status subscriptions") && detail.contains(scope),
        "{error}"
    );
}

#[tokio::test]
async fn status_subscriptions_are_bounded_per_connection_and_per_server() {
    let mut s = Server::start().await;
    let repo = s.home().join("worktrees").join("repo");
    repository(&repo);
    let cwd = json!({"cwd": repo.to_string_lossy()});

    let mut open = Vec::new();
    for index in 0..SUBSCRIPTIONS_PER_CONNECTION {
        let method = if index % 2 == 0 {
            "subscribeVcsBranch"
        } else {
            "subscribeVcsStatus"
        };
        let id = s.subscribe(method, cwd.clone());
        s.next(&id).await.unwrap();
        open.push(id);
    }
    let over = s.subscribe("subscribeVcsStatus", cwd.clone());
    over_the_limit(s.next(&over).await.unwrap_err(), "connection");
    let over = s.subscribe("subscribeVcsBranch", cwd.clone());
    over_the_limit(s.next(&over).await.unwrap_err(), "connection");
    // Unary methods are not subscriptions.
    ok(s.call("vcs.refreshBranch", cwd.clone()).await);
    // Ending one makes room on this connection.
    let ended = open.pop().unwrap();
    s.client.interrupt(&ended);
    let again = s.subscribe("subscribeVcsBranch", cwd.clone());
    s.next(&again).await.unwrap();

    // Further connections fill the server; each earlier one stays open meanwhile.
    let mut kept = Vec::new();
    for _ in 1..SUBSCRIPTIONS_PER_SERVER / SUBSCRIPTIONS_PER_CONNECTION {
        kept.push(s.another_connection().await);
        for _ in 0..SUBSCRIPTIONS_PER_CONNECTION {
            let id = s.subscribe("subscribeVcsBranch", cwd.clone());
            s.next(&id).await.unwrap();
        }
    }
    kept.push(s.another_connection().await);
    let over = s.subscribe("subscribeVcsBranch", cwd);
    over_the_limit(s.next(&over).await.unwrap_err(), "server");
    for mut socket in kept {
        socket.close(None).await.unwrap();
    }
    s.socket.close(None).await.unwrap();
    s.runtime.take().unwrap().shutdown().await.unwrap();
}
