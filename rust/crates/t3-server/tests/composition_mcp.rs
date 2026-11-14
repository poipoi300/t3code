//! The `/mcp` endpoint over real HTTP with real per-thread credentials: the credential a
//! provider session opens with, tools acting as the calling thread, and every refusal the
//! access rules owe (`McpHttpServer.ts`, `McpToolAccess.ts`, `McpSessionRegistry.ts`).
//!
//! The provider is a scripted stand-in that holds its turn open, so the calling thread stays a
//! live caller; it reads its thread's MCP configuration when the session opens, as a native
//! adapter does.
#![cfg(all(feature = "local-services", feature = "mcp"))]
#![allow(clippy::unwrap_used, clippy::expect_used)]
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
};
use t3_contracts::{
    base_schemas::ThreadId,
    orchestration_v2_id_table::IdOrIndex,
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
};
use t3_mcp::{CredentialRequest, ProviderSessionConfig, session::AliveMark};
use t3_orchestration::{api::*, provider::*};
use t3_rpc::{CallEvent, CallKind, Client, Exit, Frame};
use t3_server::{
    adapters::{
        config::{ConfigInputs, ServerFlags},
        mcp::McpSessions,
    },
    runtime::StartOptions,
};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};
use tokio_util::sync::CancellationToken;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
const NOW: &str = "2026-11-14T12:00:00.000Z";
const CALLER: &str = "thread:mcp-caller";
const OTHER: &str = "thread:mcp-full-access";
const IDLE_WINDOW_MS: i64 = 86_400_000;
const APP_ITEM: &str = "item:mcp-app";

fn sample(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/samples")
        .join(format!("{name}.jsonl"));
    let text = std::fs::read_to_string(path).expect("sample fixture");
    serde_json::from_str(text.lines().next().expect("sample line")).expect("sample json")
}

fn provider_error(error: serde_json::Error) -> ProviderError {
    ProviderError::Operation {
        operation: ProviderOperation::Open,
        detail: error.to_string(),
    }
}

/// What the stand-in provider saw: the MCP configuration of each session it opened, and each
/// turn it started.
#[derive(Default)]
struct Seen {
    configs: Mutex<Vec<(ThreadId, Option<ProviderSessionConfig>)>>,
    turns: Mutex<Vec<ThreadId>>,
}

struct HoldingAdapter {
    instance: ProviderInstanceId,
    capabilities: OrchestrationV2ProviderCapabilities,
    sessions: Arc<McpSessions>,
    seen: Arc<Seen>,
    changed: watch::Sender<u64>,
}

#[async_trait]
impl ProviderAdapter for HoldingAdapter {
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.instance
    }
    fn driver(&self) -> ProviderDriverKind {
        ProviderDriverKind("replay".into())
    }
    async fn capabilities(&self) -> ProviderResult<OrchestrationV2ProviderCapabilities> {
        Ok(self.capabilities.clone())
    }
    async fn plan_selection_transition(
        &self,
        _input: SelectionTransitionInput,
    ) -> ProviderResult<SelectionTransitionPlan> {
        Ok(SelectionTransitionPlan::ApplyOnNextTurn)
    }
    async fn open_session(
        &self,
        input: OpenSession,
        _cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        // A native adapter reads its thread's MCP server here (`McpProviderSessions.read`).
        self.seen.configs.lock().unwrap().push((
            input.thread_id.clone(),
            self.sessions.read(&input.thread_id),
        ));
        self.changed.send_modify(|n| *n += 1);
        let mut session = sample("OrchestrationV2ProviderSession");
        session["id"] = json!(input.provider_session_id);
        session["driver"] = "replay".into();
        session["providerInstanceId"] = "replay".into();
        session["status"] = "ready".into();
        session["model"] = json!(input.model_selection.model);
        session["capabilities"] =
            serde_json::to_value(&self.capabilities).map_err(provider_error)?;
        Ok(Arc::new(HoldingSession {
            session: serde_json::from_value(session).map_err(provider_error)?,
            subscribers: Mutex::new(Vec::new()),
            seen: self.seen.clone(),
            changed: self.changed.clone(),
        }))
    }
}

struct HoldingSession {
    session: OrchestrationV2ProviderSession,
    subscribers: Mutex<Vec<mpsc::Sender<ProviderResult<ProviderEvent>>>>,
    seen: Arc<Seen>,
    changed: watch::Sender<u64>,
}

fn expanded<T: Clone>(id: &IdOrIndex<T>) -> ProviderResult<T> {
    match id {
        IdOrIndex::Id(id) => Ok(id.clone()),
        _ => Err(ProviderError::Unsupported("compact id in scripted adapter")),
    }
}

#[async_trait]
impl SessionRuntime for HoldingSession {
    fn session(&self) -> &OrchestrationV2ProviderSession {
        &self.session
    }
    fn can_resume_after_restart(&self) -> bool {
        true
    }
    fn background_tasks_end_with_process(&self) -> bool {
        true
    }
    async fn subscribe(&self) -> ProviderResult<EventSubscription> {
        let (sender, events) = mpsc::channel(64);
        self.subscribers.lock().unwrap().push(sender);
        Ok(EventSubscription {
            events,
            cancellation: CancellationToken::new(),
        })
    }
    async fn close(&self) -> ProviderResult<()> {
        self.subscribers.lock().unwrap().clear();
        Ok(())
    }
    async fn ensure_thread(
        &self,
        input: EnsureThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        let mut thread = match input.existing_provider_thread {
            Some(thread) => serde_json::to_value(thread).map_err(provider_error)?,
            None => {
                let mut thread = sample("OrchestrationV2ProviderThread");
                thread["id"] = format!("provider-thread:{}", input.thread_id.0).into();
                thread["ownerNodeId"] = Value::Null;
                thread["lastRunOrdinal"] = Value::Null;
                thread["createdAt"] = NOW.into();
                thread["updatedAt"] = NOW.into();
                thread
            }
        };
        thread["driver"] = "replay".into();
        thread["providerInstanceId"] = "replay".into();
        thread["providerSessionId"] = json!(self.session.id);
        thread["appThreadId"] = json!(input.thread_id);
        thread["nativeThreadRef"] = json!({"driver":"replay","nativeId":format!("native:{}", input.thread_id.0),"strength":"strong"});
        serde_json::from_value(thread).map_err(provider_error)
    }
    async fn resume_thread(
        &self,
        input: ResumeThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        Ok(input.provider_thread)
    }
    /// The turn starts and stays running: the thread keeps a live run for the whole test.
    async fn start_turn(&self, input: TurnInput) -> ProviderResult<()> {
        let provider_thread_id = expanded(&input.provider_thread.id)?;
        let turn = json!({"id":format!("provider-turn:{}", input.run_id.0),"providerThreadId":provider_thread_id,"nodeId":input.root_node_id,"runAttemptId":input.attempt_id,"nativeTurnRef":null,"ordinal":input.provider_turn_ordinal,"status":"running","startedAt":NOW,"completedAt":null});
        let event = ProviderEvent {
            driver: ProviderDriverKind("replay".into()),
            kind: ProviderEventKind::Turn {
                provider_turn: Box::new(serde_json::from_value(turn).map_err(provider_error)?),
                thread_id: None,
            },
        };
        let item = json!({"id":APP_ITEM,"threadId":input.thread_id,"runId":input.run_id,"nodeId":input.root_node_id,"providerThreadId":provider_thread_id,"providerTurnId":format!("provider-turn:{}", input.run_id.0),"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":"Board","startedAt":NOW,"completedAt":NOW,"updatedAt":NOW,"type":"dynamic_tool","toolName":"board.show","input":{},"output":{"t3McpApp":{"attachmentId":"board-view","server":"board","tool":"show","resourceUri":"ui://board/view"}}});
        let app = ProviderEvent {
            driver: ProviderDriverKind("replay".into()),
            kind: ProviderEventKind::TurnItem {
                item: Box::new(serde_json::from_value(item).map_err(provider_error)?),
                session_scoped: None,
            },
        };
        let subscribers = self.subscribers.lock().unwrap().clone();
        for subscriber in &subscribers {
            let _ = subscriber.send(Ok(event.clone())).await;
            let _ = subscriber.send(Ok(app.clone())).await;
        }
        self.seen.turns.lock().unwrap().push(input.thread_id.clone());
        self.changed.send_modify(|n| *n += 1);
        Ok(())
    }
    async fn steer_turn(&self, _input: SteerInput) -> ProviderResult<()> {
        Err(ProviderError::Unsupported("steer_turn"))
    }
    async fn interrupt_turn(&self, _input: InterruptInput) -> ProviderResult<()> {
        Err(ProviderError::Unsupported("interrupt_turn"))
    }
    async fn respond(&self, _input: RuntimeResponse) -> ProviderResult<()> {
        Err(ProviderError::Unsupported("respond"))
    }
    async fn snapshot(
        &self,
        _thread: OrchestrationV2ProviderThread,
    ) -> ProviderResult<ThreadSnapshot> {
        Err(ProviderError::Unsupported("snapshot"))
    }
    async fn rollback(&self, _input: RollbackInput) -> ProviderResult<ThreadSnapshot> {
        Err(ProviderError::Unsupported("rollback"))
    }
    async fn fork(&self, _input: ForkInput) -> ProviderResult<OrchestrationV2ProviderThread> {
        Err(ProviderError::Unsupported("fork"))
    }
    fn extensions(&self) -> Option<&dyn SessionExtensions> {
        Some(self)
    }
}

/// Only MCP apps: one app-callable tool on the `board` server.
#[async_trait]
impl SessionExtensions for HoldingSession {
    fn capabilities(&self) -> ExtensionCapabilities {
        ExtensionCapabilities {
            mcp_apps: true,
            ..Default::default()
        }
    }
    async fn pending_background_work(
        &self,
        _thread: Option<OrchestrationV2ProviderThread>,
        _include_subagents: bool,
    ) -> ProviderResult<bool> {
        Ok(false)
    }
    async fn model_context_window(
        &self,
        _selection: t3_contracts::model_selection::ModelSelection,
        _cwd: Option<std::path::PathBuf>,
    ) -> ProviderResult<Option<u64>> {
        Ok(None)
    }
    fn can_reuse_context_usage(
        &self,
        _previous: &t3_contracts::model_selection::ModelSelection,
        _next: &t3_contracts::model_selection::ModelSelection,
    ) -> bool {
        false
    }
    fn normalize_thread_load_selection(
        &self,
        selection: t3_contracts::model_selection::ModelSelection,
    ) -> t3_contracts::model_selection::ModelSelection {
        selection
    }
    async fn inject_history(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _messages: Vec<OrchestrationV2HistoricalMessage>,
        _context: String,
    ) -> ProviderResult<bool> {
        Err(ProviderError::Unsupported("inject_history"))
    }
    async fn compact(&self, _input: TurnInput) -> ProviderResult<()> {
        Err(ProviderError::Unsupported("compact"))
    }
    async fn apply_selection(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _selection: t3_contracts::model_selection::ModelSelection,
    ) -> ProviderResult<()> {
        Err(ProviderError::Unsupported("apply_selection"))
    }
    async fn stop_task(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _task_id: String,
    ) -> ProviderResult<()> {
        Err(ProviderError::Unsupported("stop_task"))
    }
    async fn await_tasks(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _task_ids: Vec<String>,
        _awaited: bool,
    ) -> ProviderResult<Vec<String>> {
        Err(ProviderError::Unsupported("await_tasks"))
    }
    async fn stop_subagent(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _native_task_id: String,
        _requested_at: Option<t3_contracts::base_schemas::IsoDateTime>,
    ) -> ProviderResult<Option<SubagentOutcome>> {
        Err(ProviderError::Unsupported("stop_subagent"))
    }
    async fn unload(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _archive: bool,
    ) -> ProviderResult<()> {
        Err(ProviderError::Unsupported("unload"))
    }
    async fn upload_feedback(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _reason: Option<String>,
    ) -> ProviderResult<String> {
        Err(ProviderError::Unsupported("upload_feedback"))
    }
    async fn list_tools(
        &self,
        _thread: OrchestrationV2ProviderThread,
        server: String,
    ) -> ProviderResult<Vec<McpTool>> {
        assert_eq!(server, "board");
        Ok(vec![
            McpTool {
                name: "show".into(),
                input_schema: json!({"type":"object"}),
                meta: Some(json!({"ui":{"visibility":["model","app"]}})),
                annotations: Some(json!({"title":"Show board","readOnlyHint":true})),
            },
            McpTool {
                name: "secret".into(),
                input_schema: json!({"type":"object"}),
                meta: Some(json!({"ui":{"visibility":["model"]}})),
                annotations: None,
            },
        ])
    }
    async fn call_tool(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _server: String,
        tool: String,
        arguments: Value,
    ) -> ProviderResult<McpResult> {
        Ok(McpResult {
            content: vec![json!({"type":"text","text":format!("{tool} {arguments}")})],
            structured_content: None,
            is_error: None,
            meta: None,
        })
    }
    async fn read_resource(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _server: String,
        uri: String,
    ) -> ProviderResult<Vec<Value>> {
        Ok(vec![json!({"uri":uri,"mimeType":"text/html","text":"<p>board</p>"})])
    }
}

/// A clock the test moves past the credential idle window.
struct TestClock(AtomicI64);
impl t3_mcp::session::Clock for TestClock {
    fn now(&self) -> AliveMark {
        let ms = self.0.load(Ordering::SeqCst);
        AliveMark {
            wall_ms: ms,
            monotonic_ms: ms,
        }
    }
}

fn inputs(home: &Path) -> ConfigInputs {
    ConfigInputs {
        flags: ServerFlags {
            base_dir: Some(home.to_string_lossy().into_owned()),
            host: Some("127.0.0.1".into()),
            port: Some(0),
            ..Default::default()
        },
        env: Default::default(),
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
    }
}

async fn event(socket: &mut Socket, client: &mut Client) -> CallEvent {
    loop {
        while let Some(message) = client.pop_outgoing() {
            let Frame::Text(text) = t3_rpc::codec::encode(&message).unwrap() else {
                panic!("text frame expected");
            };
            socket.send(Message::Text(text.into())).await.unwrap();
        }
        if let Some(event) = client.pop_event() {
            return event;
        }
        let frame = match socket.next().await.unwrap().unwrap() {
            Message::Text(text) => Frame::Text(text.to_string()),
            Message::Binary(bytes) => Frame::Binary(bytes.to_vec()),
            Message::Ping(payload) => {
                socket.send(Message::Pong(payload)).await.unwrap();
                continue;
            }
            other => panic!("connection ended: {other:?}"),
        };
        for message in t3_rpc::codec::decode(&frame).unwrap() {
            client.receive(message, 0);
        }
    }
}

async fn command(socket: &mut Socket, client: &mut Client, command: Value) {
    let id = client
        .start("orchestration.dispatchCommand", command, CallKind::Unary)
        .unwrap();
    loop {
        match event(socket, client).await {
            CallEvent::Exit { id: done, exit } if done == id => match exit {
                Exit::Success { .. } => return,
                other => panic!("command failed: {}", serde_json::to_value(other).unwrap()),
            },
            _ => {}
        }
    }
}

async fn rpc(socket: &mut Socket, client: &mut Client, tag: &str, payload: Value) -> Exit {
    let id = client.start(tag, payload, CallKind::Unary).unwrap();
    loop {
        match event(socket, client).await {
            CallEvent::Exit { id: done, exit } if done == id => return exit,
            CallEvent::Chunk { id, .. } => client.acknowledge(id),
            _ => {}
        }
    }
}

fn success(exit: Exit) -> Value {
    match exit {
        Exit::Success { value } => value.unwrap_or(Value::Null),
        other => panic!("expected success: {}", serde_json::to_value(other).unwrap()),
    }
}

struct Mcp {
    http: reqwest::Client,
    url: String,
}
impl Mcp {
    /// One stateless 2026-07-28 request.
    async fn request(&self, token: Option<&str>, method: &str, params: Value) -> (u16, Value) {
        let mut params = params;
        params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{},"io.modelcontextprotocol/clientInfo":{"name":"composition","version":"1"}});
        let mut request = self
            .http
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", method);
        if let Some(name) = params["name"].as_str() {
            request = request.header("mcp-name", name);
        }
        if let Some(token) = token {
            request = request.header("authorization", token);
        }
        let response = request
            .body(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string())
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap();
        let body = text
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap_or(&text)
            .to_owned();
        (status, serde_json::from_str(&body).unwrap_or(Value::String(text)))
    }
    async fn call(&self, token: &str, tool: &str, arguments: Value) -> Value {
        let (status, body) = self
            .request(
                Some(token),
                "tools/call",
                json!({"name":tool,"arguments":arguments}),
            )
            .await;
        assert_eq!(status, 200, "{tool}: {body}");
        body
    }
}

fn is_error(body: &Value) -> bool {
    body["result"]["isError"] == true
}

#[tokio::test]
async fn provider_sessions_get_a_thread_credential_and_tools_act_as_that_thread_only() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    std::fs::create_dir_all(home.join("userdata")).unwrap();
    std::fs::write(
        home.join("userdata").join("settings.json"),
        r#"{"providerInstances":{"replay":{"driver":"replay","enabled":true}}}"#,
    )
    .unwrap();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let clock = Arc::new(TestClock(AtomicI64::new(1_000)));
    let sessions = Arc::new(McpSessions::with_clock(IDLE_WINDOW_MS, clock.clone()));
    let seen = Arc::new(Seen::default());
    let (changed, mut changes) = watch::channel(0_u64);
    let adapter = HoldingAdapter {
        instance: ProviderInstanceId("replay".into()),
        capabilities: serde_json::from_value(
            sample("OrchestrationV2ProviderSession")["capabilities"].clone(),
        )
        .unwrap(),
        sessions: sessions.clone(),
        seen: seen.clone(),
        changed,
    };
    let runtime = t3_server::runtime::start_with(
        inputs(home),
        StartOptions {
            providers: vec![Arc::new(adapter)],
            mcp_sessions: Some(sessions.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let origin = format!("http://{}", runtime.address);
    let http = reqwest::Client::new();
    let mcp = Mcp {
        http: http.clone(),
        url: format!("{origin}/mcp"),
    };

    // The project row a ProjectService commit would have written.
    let layout = t3_config::home::HomeLayout {
        base_dir: home.into(),
        state_dir: home.join("userdata"),
    };
    t3_persistence::rusqlite::Connection::open(layout.database())
        .unwrap()
        .execute(
            "INSERT INTO projection_projects
             (project_id, title, workspace_root, scripts_json, created_at, updated_at)
             VALUES ('project:mcp', 'MCP', ?1, '[]', '2026-11-14T12:00:00.000Z', '2026-11-14T12:00:00.000Z')",
            [workspace.to_string_lossy()],
        )
        .unwrap();

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
    let (mut socket, _) = tokio_tungstenite::connect_async(format!(
        "ws://{}/ws?orchestrationProtocol=2&wsTicket={}",
        runtime.address,
        ticket["ticket"].as_str().unwrap()
    ))
    .await
    .unwrap();
    let mut client = Client::new(0, Default::default());
    let model = json!({"instanceId":"replay","model":"replay-model"});
    for (thread, mode) in [(CALLER, "approval-required"), (OTHER, "full-access")] {
        command(&mut socket, &mut client, json!({"type":"thread.create","commandId":format!("create:{thread}"),"threadId":thread,"projectId":"project:mcp","title":thread,"modelSelection":model,"runtimeMode":mode,"interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"})).await;
    }
    command(&mut socket, &mut client, json!({"type":"message.dispatch","commandId":"command:message","threadId":CALLER,"messageId":"message:mcp","text":"Hello","attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"})).await;
    // Subscribed after dispatch: the first value is the snapshot, so nothing is missed.
    let watched = client
        .start(
            "orchestration.subscribeThread",
            json!({"threadId": CALLER}),
            CallKind::Stream,
        )
        .unwrap();
    // The provider's MCP app item is projected.
    loop {
        match event(&mut socket, &mut client).await {
            CallEvent::Chunk { id, values } if id == watched => {
                client.acknowledge(id);
                if Value::Array(values).to_string().contains(APP_ITEM) {
                    break;
                }
            }
            CallEvent::Exit { id, .. } if id == watched => panic!("thread stream ended"),
            _ => {}
        }
    }

    // mcpApps: tool info and a call reach the live session that rendered the app.
    let app = json!({"threadId":CALLER,"itemId":APP_ITEM});
    let mut input = app.clone();
    input["name"] = "show".into();
    let info = success(rpc(&mut socket, &mut client, "mcpApps.toolInfo", input.clone()).await);
    assert_eq!(info["callable"], true, "{info}");
    assert_eq!(info["readOnly"], true, "{info}");
    assert_eq!(info["title"], "Show board", "{info}");
    input["arguments"] = json!({"page":2});
    let called = success(rpc(&mut socket, &mut client, "mcpApps.callTool", input).await);
    assert_eq!(called["content"][0]["text"], "show {\"page\":2}", "{called}");
    let mut input = app.clone();
    input["uri"] = "ui://board/view".into();
    let resource = success(rpc(&mut socket, &mut client, "mcpApps.readResource", input).await);
    assert_eq!(resource["contents"][0]["text"], "<p>board</p>", "{resource}");
    // A tool hidden from apps is refused with the typed reason.
    let mut input = app.clone();
    input["name"] = "secret".into();
    input["arguments"] = json!({});
    let refused = rpc(&mut socket, &mut client, "mcpApps.callTool", input).await;
    let refused = serde_json::to_value(refused).unwrap().to_string();
    assert!(refused.contains("McpAppRequestError") && refused.contains("tool-not-callable"), "{refused}");
    // An item that is not an app is refused as such.
    let refused = rpc(&mut socket, &mut client, "mcpApps.toolInfo", json!({"threadId":CALLER,"itemId":"item:absent","name":"show"})).await;
    let refused = serde_json::to_value(refused).unwrap().to_string();
    assert!(refused.contains("not-an-app"), "{refused}");
    // Model context is stored for the app's thread.
    let mut input = app.clone();
    input["conversationThreadId"] = CALLER.into();
    input["content"] = json!([{"type":"text","text":"Board shows page 2."}]);
    success(rpc(&mut socket, &mut client, "mcpApps.updateModelContext", input).await);
    let stored: String = t3_persistence::rusqlite::Connection::open(layout.database())
        .unwrap()
        .query_row(
            "SELECT text FROM mcp_app_model_context WHERE thread_id = ?1 AND item_id = ?2",
            [CALLER, APP_ITEM],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, "Board shows page 2.");

    // The session opened with its thread's credential, before the turn started.
    changes
        .wait_for(|_| seen.turns.lock().unwrap().iter().any(|t| t.0 == CALLER))
        .await
        .unwrap();
    let config = {
        let configs = seen.configs.lock().unwrap();
        let (thread, config) = configs.first().expect("a session opened");
        assert_eq!(thread.0, CALLER);
        config.clone().expect("the session opened with an MCP credential")
    };
    assert_eq!(config.endpoint, format!("{origin}/mcp"));
    let caller = config.scope.thread.clone().unwrap();
    assert_eq!(caller.thread_id.0, CALLER);
    assert_eq!(caller.provider_instance_id.0, "replay");
    assert!(!caller.subagent);
    let token = config.authorization_header.clone();
    assert!(token.starts_with("Bearer "));
    let http_server = config.http_server(false);
    assert_eq!(http_server["url"], format!("{origin}/mcp"));
    assert_eq!(http_server["headers"]["Authorization"], token);

    // No credential: 401 with the OAuth challenge for this origin.
    let anonymous = http.post(&mcp.url).send().await.unwrap();
    assert_eq!(anonymous.status(), 401);
    assert_eq!(
        anonymous.headers()["www-authenticate"].to_str().unwrap(),
        format!("Bearer resource_metadata=\"{origin}/.well-known/oauth-protected-resource/mcp\"")
    );
    let (status, body) = mcp
        .request(Some("Bearer not-a-real-token"), "tools/list", json!({}))
        .await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["error"], "invalid_mcp_credential");

    // The tool text served is byte-identical to the TypeScript export for this caller's mode.
    let (status, listed) = mcp.request(Some(&token), "tools/list", json!({})).await;
    assert_eq!(status, 200, "{listed}");
    let expected = &t3_mcp::catalog::catalog().modes["thread/no-links/full"];
    assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 74);
    assert_eq!(
        listed["result"]["tools"].to_string(),
        expected["tools"].to_string()
    );

    // A tool succeeds as the calling thread.
    let sent = mcp
        .call(&token, "send_to_user", json!({"message":"Build finished."}))
        .await;
    assert!(!is_error(&sent), "{sent}");
    assert_eq!(
        sent["result"]["structuredContent"]["sentMessage"],
        "Build finished."
    );
    let tasks = mcp.call(&token, "list_scheduled_tasks", json!({})).await;
    assert!(!is_error(&tasks), "{tasks}");

    // Thread A's credential cannot change thread B, whose mode is above A's.
    let raised = mcp
        .call(
            &token,
            "t3_thread_update",
            json!({"threadId":OTHER,"action":"rename","title":"Renamed by A"}),
        )
        .await;
    assert!(is_error(&raised), "{raised}");
    assert!(
        raised.to_string().contains("runtime_mode_escalation_denied")
            && raised
                .to_string()
                .contains("Child runtime mode full-access is broader than parent mode approval-required."),
        "{raised}"
    );
    // A credential for B, whose provider holds no run, cannot write as B.
    let registry = sessions.registry().unwrap();
    let other = registry
        .issue(CredentialRequest::new(
            ThreadId(OTHER.into()),
            ProviderInstanceId("replay".into()),
        ))
        .unwrap();
    let idle = mcp
        .call(
            &other.authorization_header,
            "send_to_user",
            json!({"message":"Not mine to send."}),
        )
        .await;
    assert!(is_error(&idle), "{idle}");
    assert!(idle.to_string().contains("parent_not_active"), "{idle}");

    // A sub-agent's credential neither lists nor calls the tools hidden from sub-agents.
    let subagent = registry
        .issue(CredentialRequest {
            subagent: true,
            ..CredentialRequest::new(ThreadId(CALLER.into()), ProviderInstanceId("replay".into()))
        })
        .unwrap();
    let (_, listed) = mcp
        .request(Some(&subagent.authorization_header), "tools/list", json!({}))
        .await;
    let names = listed["result"]["tools"].to_string();
    assert!(!names.contains("\"watch_pull_request\""), "{names}");
    let (status, hidden) = mcp
        .request(
            Some(&subagent.authorization_header),
            "tools/call",
            json!({"name":"watch_pull_request","arguments":{"url":"https://github.com/o/r/pull/1"}}),
        )
        .await;
    assert_eq!(status, 200, "{hidden}");
    assert_eq!(
        hidden["error"]["message"],
        "Tool 'watch_pull_request' not found",
        "{hidden}"
    );

    // A tool whose host service is not bound says so, with the missing method.
    let unbound = mcp.call(&token, "t3_thread_list", json!({})).await;
    assert!(is_error(&unbound), "{unbound}");
    assert_eq!(unbound["result"]["structuredContent"]["available"], false);

    // Revoked by thread: the next request is refused.
    sessions
        .revoke_thread(&ThreadId(OTHER.into()))
        .await
        .unwrap();
    let (status, _) = mcp
        .request(Some(&other.authorization_header), "tools/list", json!({}))
        .await;
    assert_eq!(status, 401);

    // Idle past the window: expired, and stays refused.
    clock.0.fetch_add(IDLE_WINDOW_MS + 1, Ordering::SeqCst);
    let (status, body) = mcp.request(Some(&token), "tools/list", json!({})).await;
    assert_eq!(status, 401, "{body}");

    runtime.shutdown().await.unwrap();
}
