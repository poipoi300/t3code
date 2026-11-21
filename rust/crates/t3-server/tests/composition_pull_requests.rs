//! The `pullRequests.*` group over a started server and a real socket. The forge is faked at the
//! registry seam (`ProjectBinder`): projects come from the server's own project store, so
//! routing, authorisation, persistence and error mapping are the production paths.
#![cfg(feature = "pull-requests")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use t3_contracts::pull_request::*;
use t3_pull_request::{
    Result,
    context::{Binding, Context, Project},
    host::Host,
};
use t3_rpc::{CallEvent, CallKind, Client, Exit, Frame};
use t3_server::adapters::{
    config::{ConfigInputs, ServerFlags},
    pull_requests::{ProjectBinder, ProjectRecord},
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}

/// A forge driver that records every host call it receives.
struct FakeForge {
    kind: &'static str,
    public: bool,
    caps: PullRequestCapabilities,
    calls: Mutex<Vec<String>>,
}
impl FakeForge {
    fn new(public: bool) -> Arc<Self> {
        Arc::new(Self {
            kind: "github",
            public,
            caps: decode(json!({
                "diff": true, "comment": true, "actions": ["merge"], "mergeMethods": ["merge"],
                "search": true,
                "review": {"inlineComment": true, "reply": true, "resolve": true, "verdicts": ["comment"]},
                "reviewers": {"request": true, "listCandidates": true}
            })),
            calls: Mutex::new(Vec::new()),
        })
    }
    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn summary(ctx: &Context) -> PullRequestSummary {
        decode(json!({
            "provider": "github", "projectId": ctx.project.id, "repository": ctx.project.repository,
            "number": ctx.reference.number, "title": format!("PR {}", ctx.reference.number),
            "url": format!("https://{}/{}/pull/{}", ctx.project.host, ctx.project.repository, ctx.reference.number),
            "state": "open", "headBranch": "topic", "baseBranch": "main",
            "updatedAt": "2026-01-01T00:00:00.000Z"
        }))
    }
}
#[async_trait::async_trait]
impl Host for FakeForge {
    fn kind(&self) -> &str {
        self.kind
    }
    fn capabilities(&self) -> &PullRequestCapabilities {
        &self.caps
    }
    fn reads_public_host(&self, host: &str) -> bool {
        self.public && host == "github.com"
    }
    async fn viewer(&self, _: &Project) -> Result<String> {
        Ok("alice".into())
    }
    async fn summary(&self, ctx: &Context) -> Result<PullRequestSummary> {
        self.record(format!(
            "summary {}/{}#{} from {}",
            ctx.project.host, ctx.project.repository, ctx.reference.number, ctx.project.id.0
        ));
        Ok(Self::summary(ctx))
    }
    async fn detail(&self, ctx: &Context) -> Result<PullRequestDetail> {
        self.record(format!("detail #{}", ctx.reference.number));
        let mut value = serde_json::to_value(Self::summary(ctx)).unwrap();
        value.as_object_mut().unwrap().extend(
            json!({
                "capabilities": self.caps,
                "viewerPermissions": {"actions": [], "comment": true, "resolve": true, "verdicts": [], "requestReviewers": false},
                "projectTitle": ctx.project.title, "workspaceRoot": ctx.project.workspace_root,
                "body": "", "author": null, "isDraft": false, "mergeability": "mergeable",
                "additions": 1, "deletions": 0, "changedFiles": 1,
                "createdAt": "2026-01-01T00:00:00.000Z", "reviewers": [], "labels": [],
                "checks": [], "mergeCapabilities": {"merge": true, "squash": false, "rebase": false},
                "headSha": "abc1234", "mergedAt": null, "closedAt": null
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        Ok(decode(value))
    }
    async fn permissions(&self, _: &Context, _: bool) -> Result<PullRequestViewerPermissions> {
        Ok(decode(
            json!({"actions": [], "comment": true, "resolve": true, "verdicts": [], "requestReviewers": false}),
        ))
    }
    async fn comment(&self, ctx: &Context, input: &PullRequestCommentInput) -> Result<()> {
        self.record(format!(
            "comment #{} {}",
            ctx.reference.number,
            input.body.as_str()
        ));
        Ok(())
    }
}

/// Binds projects whose folder is named `github-*` to `github.com/owner/repo`; every other
/// project has no recognised remote. The public host is a separate shared driver.
struct FakeRegistry {
    checkout: Arc<FakeForge>,
    public: Arc<FakeForge>,
}
#[async_trait::async_trait]
impl ProjectBinder for FakeRegistry {
    async fn bind(&self, project: ProjectRecord) -> Result<Option<Binding>> {
        let name = project
            .workspace_root
            .file_name()
            .unwrap()
            .to_string_lossy();
        if !name.starts_with("github-") {
            return Ok(None);
        }
        Ok(Some(Binding {
            project: Project {
                id: project.id,
                title: project.title,
                workspace_root: project.workspace_root.to_string_lossy().into(),
                host: "github.com".into(),
                repository: "owner/repo".into(),
                canonical_repository_key: Some("github.com/owner/repo".into()),
                remove_agent_credits_on_merge: project.remove_agent_credits_on_merge,
            },
            host: self.checkout.clone(),
        }))
    }
    fn canonical_repository_key(&self, key: &str) -> String {
        key.to_lowercase()
    }
    fn public_host(&self) -> Option<Arc<dyn Host>> {
        Some(self.public.clone())
    }
}

struct Server {
    temp: tempfile::TempDir,
    runtime: t3_server::runtime::RunningServer,
    origin: String,
    cookie: String,
    socket: Socket,
    client: Client,
    checkout: Arc<FakeForge>,
    public: Arc<FakeForge>,
}

impl Server {
    async fn start() -> Self {
        Self::start_in(tempfile::tempdir().unwrap()).await
    }
    async fn start_in(temp: tempfile::TempDir) -> Self {
        let home = temp.path();
        let env = ["SystemRoot", "ComSpec", "WINDIR"]
            .into_iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_owned(), v)))
            .collect();
        let checkout = FakeForge::new(false);
        let public = FakeForge::new(true);
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
                pull_request_binder: Some(Arc::new(FakeRegistry {
                    checkout: checkout.clone(),
                    public: public.clone(),
                })),
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
        let socket = connect(&origin, &runtime.address.to_string(), &cookie).await;
        Self {
            temp,
            runtime,
            origin,
            cookie,
            socket,
            client: Client::new(0, Default::default()),
            checkout,
            public,
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
        let cookie = paired.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        self.socket = connect(&self.origin, &self.runtime.address.to_string(), &cookie).await;
        self.client = Client::new(0, Default::default());
    }
    /// A new connection for the full-scope session, closing the current one.
    async fn reconnect(&mut self) {
        let _ = self.socket.close(None).await;
        self.socket = connect(
            &self.origin,
            &self.runtime.address.to_string(),
            &self.cookie,
        )
        .await;
        self.client = Client::new(0, Default::default());
    }
    fn database(&self) -> t3_persistence::rusqlite::Connection {
        let layout = t3_config::home::HomeLayout {
            base_dir: self.home().into(),
            state_dir: self.home().join("userdata"),
        };
        t3_persistence::rusqlite::Connection::open(layout.database()).unwrap()
    }
    fn insert_project(&self, id: &str, folder: &str) {
        let root = self.home().join(folder);
        std::fs::create_dir_all(&root).unwrap();
        self.database()
            .execute(
                "INSERT INTO projection_projects
                 (project_id, title, workspace_root, scripts_json, created_at, updated_at)
                 VALUES (?1, ?1, ?2, '[]', '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
                (id, root.to_string_lossy()),
            )
            .unwrap();
    }
    fn insert_thread(&self, id: &str, project: &str, payload: Value) {
        self.database()
            .execute(
                "INSERT INTO orchestration_v2_projection_threads
                 (thread_id, project_id, title, default_provider, runtime_mode, interaction_mode,
                  created_at, updated_at, payload_json)
                 VALUES (?1, ?2, ?1, 'codex', 'full-access', 'default',
                  '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z', ?3)",
                (id, project, payload.to_string()),
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
    /// A unary call's exit. A chunk of any open stream fails the test: none is expected here.
    async fn call(&mut self, tag: &str, payload: Value) -> Exit {
        let id = self.client.start(tag, payload, CallKind::Unary).unwrap();
        loop {
            match self.event().await {
                CallEvent::Exit { id: done, exit } if done == id => return exit,
                CallEvent::Chunk { id, values } => panic!("unexpected chunk on {id:?}: {values:?}"),
                _ => {}
            }
        }
    }
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

async fn connect(origin: &str, address: &str, cookie: &str) -> Socket {
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

fn ok(exit: Exit) -> Value {
    match exit {
        Exit::Success { value } => value.unwrap_or(Value::Null),
        other => panic!(
            "expected success, got {}",
            serde_json::to_value(other).unwrap()
        ),
    }
}
/// The typed failure (`Fail` error), never a defect.
fn typed(exit: Exit) -> Value {
    let encoded = serde_json::to_value(&exit).unwrap();
    let error = encoded["cause"][0]["error"].clone();
    assert!(!error.is_null(), "expected a typed failure, got {encoded}");
    error
}
fn reference(project: &str, number: u64) -> Value {
    json!({"projectId": project, "host": "github.com", "repository": "owner/repo", "number": number})
}

#[tokio::test]
async fn reads_route_through_the_environments_projects_and_never_a_client_chosen_host() {
    let mut s = Server::start().await;
    s.insert_project("bound", "github-app");
    s.insert_project("scratch", "scratch");

    // A bound project reads through its own checkout's driver.
    let summary = ok(s.call("pullRequests.summary", reference("bound", 7)).await);
    assert_eq!(summary["title"], "PR 7");
    assert_eq!(
        s.checkout.calls(),
        ["summary github.com/owner/repo#7 from bound"]
    );
    let detail = ok(s.call("pullRequests.detail", reference("bound", 7)).await);
    assert_eq!(detail["headSha"], "abc1234");
    let guide = ok(s
        .call("pullRequests.reviewGuide", reference("bound", 7))
        .await);
    assert_eq!(guide["currentHeadSha"], "abc1234");
    assert!(guide["guide"].is_null());

    // A project with no remote reads public github.com through the shared public driver.
    let public =
        json!({"projectId": "scratch", "host": "github.com", "repository": "org/lib", "number": 5});
    assert_eq!(
        ok(s.call("pullRequests.summary", public).await)["repository"],
        "org/lib"
    );
    assert_eq!(
        s.public.calls(),
        ["summary github.com/org/lib#5 from scratch"]
    );

    // A host the client names with no project bound to it is refused before any driver runs,
    // and so is a project id this environment does not have.
    let before = (s.checkout.calls().len(), s.public.calls().len());
    let foreign = json!({"projectId": "bound", "host": "attacker.example", "repository": "owner/repo", "number": 7});
    let error = typed(s.call("pullRequests.summary", foreign).await);
    assert_eq!(error["_tag"], "PullRequestUnavailableError");
    assert_eq!(error["reason"], "provider-unsupported");
    let unknown = json!({"projectId": "ghost", "repository": "owner/repo", "number": 7});
    assert_eq!(
        typed(s.call("pullRequests.summary", unknown).await)["_tag"],
        "PullRequestUnavailableError"
    );
    let not_ours = json!({"projectId": "bound", "repository": "someone/else", "number": 7});
    let error = typed(s.call("pullRequests.detail", not_ours).await);
    assert_eq!(error["_tag"], "PullRequestOperationError");
    assert_eq!(
        error["detail"],
        "The change request does not belong to the selected project."
    );
    assert_eq!((s.checkout.calls().len(), s.public.calls().len()), before);

    // Threads linked to the pull request come from the persisted projection.
    s.insert_thread(
        "thread-linked",
        "bound",
        json!({"pullRequests": [{"host": "github.com", "repository": "Owner/Repo", "number": 7,
            "url": "https://github.com/owner/repo/pull/7", "source": "agent"}]}),
    );
    s.insert_thread(
        "thread-dismissed",
        "bound",
        json!({"pullRequests": [{"host": "github.com", "repository": "owner/repo", "number": 7,
            "url": "https://github.com/owner/repo/pull/7", "source": "stack-dismissed"}]}),
    );
    let linked = ok(s
        .call("pullRequests.linkedThreads", reference("bound", 7))
        .await);
    assert_eq!(linked["threads"].as_array().unwrap().len(), 1);
    assert_eq!(linked["threads"][0]["id"], "thread-linked");
    assert_eq!(linked["threads"][0]["settled"], false);
    let none = ok(s
        .call("pullRequests.linkedThreads", reference("bound", 8))
        .await);
    assert_eq!(none["threads"], json!([]));

    // Reads need `orchestration:read`; the guard refuses before the driver runs.
    s.reconnect_with(&["settings:write"]).await;
    let before = s.checkout.calls().len();
    for method in [
        "pullRequests.summary",
        "pullRequests.detail",
        "pullRequests.linkedThreads",
        "pullRequests.reviewGuide",
    ] {
        let error = typed(s.call(method, reference("bound", 7)).await);
        assert_eq!(error["_tag"], "EnvironmentAuthorizationError", "{method}");
    }
    assert_eq!(s.checkout.calls().len(), before);
    s.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn writes_reach_the_driver_once_and_notify_refresh_subscribers() {
    let mut s = Server::start().await;
    s.insert_project("bound", "github-app");
    let comment = |body: &str| {
        let mut value = reference("bound", 7);
        value["body"] = json!(body);
        value
    };

    // The refresh stream starts quiet (revision zero is not sent) and reports each write.
    let subscription = s
        .client
        .start(
            "pullRequests.subscribeRefreshes",
            json!({}),
            CallKind::Stream,
        )
        .unwrap();
    ok(s.call("pullRequests.comment", comment("Looks good")).await);
    assert_eq!(s.checkout.calls(), ["comment #7 Looks good"]);
    assert_eq!(s.chunk().await, json!(1));
    ok(s.call(
        "pullRequests.invalidate",
        json!({"reference": reference("bound", 7)}),
    )
    .await);
    assert_eq!(s.chunk().await, json!(2));

    // An interrupted subscription ends: the next write sends nothing to this socket (a chunk
    // would fail `call`).
    s.client.interrupt(&subscription);
    assert!(matches!(
        s.client.pop_event(),
        Some(CallEvent::Failed { .. })
    ));
    ok(s.call("pullRequests.comment", comment("Second")).await);
    ok(s.call("server.probe", json!({})).await);

    // A subscription on a closed socket is gone; a new socket's starts at the current revision.
    s.client
        .start(
            "pullRequests.subscribeRefreshes",
            json!({}),
            CallKind::Stream,
        )
        .unwrap();
    assert_eq!(s.chunk().await, json!(3));
    s.reconnect().await;
    s.client
        .start(
            "pullRequests.subscribeRefreshes",
            json!({}),
            CallKind::Stream,
        )
        .unwrap();
    assert_eq!(s.chunk().await, json!(3));

    // A typed failure with TypeScript's text, and no driver call.
    let error = typed(s.call("pullRequests.comment", comment("   ")).await);
    assert_eq!(error["_tag"], "PullRequestOperationError");
    assert_eq!(error["detail"], "A comment cannot be empty.");
    assert_eq!(s.checkout.calls().len(), 2);

    // Writes need `source-control:write`: a read-only session is refused before the driver.
    s.reconnect_with(&["orchestration:read"]).await;
    let error = typed(s.call("pullRequests.comment", comment("Refused")).await);
    assert_eq!(error["_tag"], "EnvironmentAuthorizationError");
    assert_eq!(s.checkout.calls().len(), 2);
    // Reads still work for that session.
    ok(s.call("pullRequests.summary", reference("bound", 7)).await);
    s.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn renaming_a_persisted_linked_thread_publishes_a_revision_and_reconnecting_replays_it() {
    let mut s = Server::start().await;
    let method = "pullRequests.subscribeLinkedThreadRefreshes";
    assert!(!s.runtime.unserved_methods.contains(&method));
    s.insert_project("bound", "github-app");
    ok(s.call("orchestration.dispatchCommand", json!({
        "type":"thread.create", "commandId":"create", "threadId":"thread", "projectId":"bound",
        "title":"Linked thread", "modelSelection":{"instanceId":"codex","model":"gpt-5"},
        "runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,
        "createdBy":"user","creationSource":"web"
    })).await);
    // A link can predate this Rust process (e.g. a TypeScript-created thread).
    s.runtime.shutdown().await.unwrap();
    {
        let db = t3_persistence::rusqlite::Connection::open(
            s.temp.path().join("userdata/statev2.sqlite"),
        )
        .unwrap();
        let links = json!([{"host":"github.com","repository":"owner/repo","number":7,
            "url":"https://github.com/owner/repo/pull/7","source":"manual",
            "linkedAt":"2026-01-01T00:00:00Z","snapshot":null,"stack":null}]);
        assert_eq!(db.execute("UPDATE orchestration_v2_projection_threads SET payload_json=json_set(payload_json,'$.pullRequests',json(?1)) WHERE thread_id='thread'",[links.to_string()]).unwrap(),1);
    }
    let mut s = Server::start_in(s.temp).await;
    let config = ok(s.call("server.getConfig", json!({})).await);
    assert_eq!(
        config["environment"]["capabilities"]["linkedThreadRefreshes"],
        true
    );
    s.client.start(method, json!({}), CallKind::Stream).unwrap();
    let command = s
        .client
        .start(
            "orchestration.dispatchCommand",
            json!({
                "type":"thread.metadata.update","commandId":"rename","threadId":"thread","title":"Renamed"
            }),
            CallKind::Unary,
        )
        .unwrap();
    let mut revision = None;
    let mut linked = false;
    while revision.is_none() || !linked {
        match s.event().await {
            CallEvent::Chunk { id, values } => {
                s.client.acknowledge(id);
                revision = Some(values[0].clone());
            }
            CallEvent::Exit { id, exit } if id == command => {
                ok(exit);
                linked = true;
            }
            event => panic!("unexpected event {event:?}"),
        }
    }
    assert_eq!(revision, Some(json!(1)));
    let linked_threads = ok(s
        .call("pullRequests.linkedThreads", reference("bound", 7))
        .await);
    assert_eq!(linked_threads["threads"][0]["title"], "Renamed");
    s.reconnect().await;
    s.client.start(method, json!({}), CallKind::Stream).unwrap();
    assert_eq!(s.chunk().await, json!(1));
    s.runtime.shutdown().await.unwrap();
}
