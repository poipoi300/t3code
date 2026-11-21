//! Package A: the composed orchestration runtime behind the server, end to end over a real
//! socket. A client pairs, creates a thread, dispatches a message and watches the turn on
//! `subscribeThread` and `subscribeShell`; the orchestration HTTP reads serve the same state.
//!
//! Two providers drive the same client path: the scripted stand-in from
//! `t3-orchestration/tests/runtime.rs` behind the real adapter registry, and Replay (dev),
//! registered as TypeScript's dev server does. `projects.mutate` needs the unported
//! `ProjectService`, so the project row is seeded in the live database.
#![cfg(all(feature = "local-services", feature = "orchestration"))]
#![allow(clippy::unwrap_used, clippy::expect_used)]
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use t3_contracts::{
    orchestration_v2_id_table::IdOrIndex,
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
};
use t3_orchestration::{api::*, provider::*};
use t3_rpc::{CallEvent, CallKind, Client, Exit, Frame};
use t3_server::{
    adapters::config::{ConfigInputs, ServerFlags},
    runtime::StartOptions,
};
use tokio::sync::mpsc;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};
use tokio_util::sync::CancellationToken;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const NOW: &str = "2026-11-13T12:00:00.000Z";

fn sample(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/samples")
        .join(format!("{name}.jsonl"));
    let text = std::fs::read_to_string(path).expect("sample fixture");
    serde_json::from_str(text.lines().next().expect("sample line")).expect("sample json")
}

fn expanded<T: Clone>(id: &IdOrIndex<T>) -> ProviderResult<T> {
    match id {
        IdOrIndex::Id(id) => Ok(id.clone()),
        _ => Err(ProviderError::Unsupported("compact id in scripted adapter")),
    }
}

fn provider_error(error: serde_json::Error) -> ProviderError {
    ProviderError::Operation {
        operation: ProviderOperation::Open,
        detail: error.to_string(),
    }
}

#[derive(Default)]
struct RestorePause {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    gate: Arc<t3_server::writers::FolderGate>,
}

struct ScriptedAdapter {
    pause: Option<Arc<RestorePause>>,
    instance: ProviderInstanceId,
    capabilities: OrchestrationV2ProviderCapabilities,
}

impl ScriptedAdapter {
    fn new() -> Self {
        Self {
            pause: None,
            instance: ProviderInstanceId("replay".into()),
            capabilities: {
                let mut capabilities =
                    sample("OrchestrationV2ProviderSession")["capabilities"].clone();
                // The conversation rewinds with the files (`rollback` below).
                capabilities["threads"]["canRollbackThread"] = true.into();
                serde_json::from_value(capabilities).expect("capabilities")
            },
        }
    }
}

#[async_trait]
impl ProviderAdapter for ScriptedAdapter {
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.instance
    }
    fn driver(&self) -> ProviderDriverKind {
        ProviderDriverKind("replay".into())
    }
    /// A scripted provider that runs every mode it is given.
    fn supported_runtime_modes(&self) -> Option<Vec<t3_contracts::provider_policy::RuntimeMode>> {
        None
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
        let mut session = sample("OrchestrationV2ProviderSession");
        session["id"] = json!(input.provider_session_id);
        session["driver"] = "replay".into();
        session["providerInstanceId"] = "replay".into();
        session["status"] = "ready".into();
        session["model"] = json!(input.model_selection.model);
        session["capabilities"] =
            serde_json::to_value(&self.capabilities).map_err(provider_error)?;
        Ok(Arc::new(ScriptedSession {
            pause: self.pause.clone(),
            session: serde_json::from_value(session).map_err(provider_error)?,
            subscribers: Mutex::new(Vec::new()),
        }))
    }
}

struct ScriptedSession {
    pause: Option<Arc<RestorePause>>,
    session: OrchestrationV2ProviderSession,
    subscribers: Mutex<Vec<mpsc::Sender<ProviderResult<ProviderEvent>>>>,
}

impl ScriptedSession {
    fn event(kind: ProviderEventKind) -> ProviderEvent {
        ProviderEvent {
            driver: ProviderDriverKind("replay".into()),
            kind,
        }
    }

    /// One completed turn: started, an assistant reply, completed, terminal.
    fn turn_events(input: &TurnInput) -> ProviderResult<Vec<ProviderEvent>> {
        let provider_thread_id = expanded(&input.provider_thread.id)?;
        let provider_turn_id = ProviderTurnId(format!("provider-turn:{}", input.run_id.0));
        let turn = |status: &str, completed: Option<&str>| -> ProviderResult<ProviderEvent> {
            let turn = json!({"id":provider_turn_id,"providerThreadId":provider_thread_id,"nodeId":input.root_node_id,"runAttemptId":input.attempt_id,"nativeTurnRef":null,"ordinal":input.provider_turn_ordinal,"status":status,"startedAt":NOW,"completedAt":completed});
            Ok(Self::event(ProviderEventKind::Turn {
                provider_turn: Box::new(serde_json::from_value(turn).map_err(provider_error)?),
                thread_id: None,
            }))
        };
        let reply = json!({"createdBy":"agent","creationSource":"server","id":format!("message:reply:{}", input.run_id.0),"threadId":input.thread_id,"runId":input.run_id,"nodeId":input.root_node_id,"role":"assistant","text":"Replayed reply","attachments":[],"streaming":false,"createdAt":NOW,"updatedAt":NOW});
        Ok(vec![
            turn("running", None)?,
            Self::event(ProviderEventKind::Message(Box::new(
                serde_json::from_value(reply).map_err(provider_error)?,
            ))),
            turn("completed", Some(NOW))?,
            Self::event(ProviderEventKind::Terminal {
                provider_thread_id,
                provider_turn_id: provider_turn_id.clone(),
                run_ordinal: input.run_ordinal,
                disposition: ThreadDisposition::Reusable,
                outcome: TerminalOutcome::Completed { reason: None },
            }),
        ])
    }
}

#[async_trait]
impl SessionRuntime for ScriptedSession {
    fn session(&self) -> &OrchestrationV2ProviderSession {
        &self.session
    }
    fn can_resume_after_restart(&self) -> bool {
        true
    }
    fn background_tasks_end_with_process(&self) -> bool {
        true
    }
    /// Its turns run nothing after they end.
    fn leaves_no_background_work(&self) -> bool {
        true
    }
    async fn subscribe(&self) -> ProviderResult<EventSubscription> {
        let (sender, events) = mpsc::channel(64);
        self.subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(sender);
        Ok(EventSubscription {
            events,
            cancellation: CancellationToken::new(),
        })
    }
    async fn close(&self) -> ProviderResult<()> {
        self.subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
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
    async fn start_turn(&self, input: TurnInput) -> ProviderResult<()> {
        let events = Self::turn_events(&input)?;
        let subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        tokio::spawn(async move {
            for event in events {
                for subscriber in &subscribers {
                    let _ = subscriber.send(Ok(event.clone())).await;
                }
            }
        });
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
    async fn rollback(&self, input: RollbackInput) -> ProviderResult<ThreadSnapshot> {
        if let Some(pause) = &self.pause {
            pause.entered.notify_one();
            pause.release.notified().await;
        }
        Ok(ThreadSnapshot {
            provider_thread: input.provider_thread,
            provider_turns: Vec::new(),
            messages: Vec::new(),
            reasoning_message_ids: None,
            runtime_requests: Vec::new(),
            provider_payload: None,
        })
    }
    async fn fork(&self, _input: ForkInput) -> ProviderResult<OrchestrationV2ProviderThread> {
        Err(ProviderError::Unsupported("fork"))
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

fn ok(exit: Exit) -> Value {
    match exit {
        Exit::Success { value } => value.unwrap_or(Value::Null),
        other => panic!(
            "expected success, got {}",
            serde_json::to_value(other).unwrap()
        ),
    }
}

const THREAD: &str = "thread:server-e2e";

#[tokio::test]
async fn a_dispatched_message_runs_a_turn_seen_on_subscribe_thread_and_http_reads() {
    turn_over_the_socket(
        r#"{"providerInstances":{"replay":{"driver":"replay","enabled":true}}}"#,
        StartOptions {
            providers: vec![Arc::new(ScriptedAdapter::new())],
            dev_replay: false,
            ..Default::default()
        },
        json!({"instanceId":"replay","model":"replay-model"}),
    )
    .await;
}

#[tokio::test]
async fn a_dispatched_message_runs_a_replay_turn_over_the_socket() {
    turn_over_the_socket(
        r#"{"providerInstances":{"devReplay":{"driver":"acpRegistry","displayName":"Replay (dev)","enabled":true}}}"#,
        StartOptions {
            providers: Vec::new(),
            dev_replay: true,
            ..Default::default()
        },
        json!({"instanceId":"devReplay","model":"multi-turn"}),
    )
    .await;
}

async fn turn_over_the_socket(settings: &str, options: StartOptions, model_selection: Value) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    std::fs::create_dir_all(home.join("userdata")).unwrap();
    std::fs::write(home.join("userdata").join("settings.json"), settings).unwrap();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let runtime = t3_server::runtime::start_with(
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
        },
        options,
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
            "INSERT INTO projection_projects
             (project_id, title, workspace_root, scripts_json, created_at, updated_at)
             VALUES ('project:e2e', 'E2E', ?1, '[]', '2026-11-13T12:00:00.000Z', '2026-11-13T12:00:00.000Z')",
            [workspace.to_string_lossy()],
        )
        .unwrap();

    // The shell snapshot over HTTP: gated by protocol, then by authentication.
    let shell_url = format!("{origin}/api/orchestration/shell");
    let gate = http
        .get(&shell_url)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    // Effect's HttpApi rejects the undecodable header with an empty 400, before authentication.
    assert_eq!(gate.status(), 400);
    assert!(gate.bytes().await.unwrap().is_empty());
    let anonymous = http
        .get(&shell_url)
        .header("x-t3-orchestration-protocol", "2")
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);
    let shell: Value = http
        .get(&shell_url)
        .header("cookie", &cookie)
        .header("x-t3-orchestration-protocol", "2")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        shell["projects"].to_string().contains("project:e2e"),
        "{shell}"
    );

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

    client
        .start(
            "orchestration.dispatchCommand",
            json!({"type":"thread.create","commandId":"command:thread","threadId":THREAD,"projectId":"project:e2e","title":"E2E thread","modelSelection":model_selection,"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"}),
            CallKind::Unary,
        )
        .unwrap();
    match event(&mut socket, &mut client).await {
        CallEvent::Exit { exit, .. } => assert!(ok(exit)["sequence"].is_number()),
        other => panic!("unexpected {other:?}"),
    }

    let shell_stream = client
        .start("orchestration.subscribeShell", json!({}), CallKind::Stream)
        .unwrap();
    let subscription = client
        .start(
            "orchestration.subscribeThread",
            json!({"threadId": THREAD}),
            CallKind::Stream,
        )
        .unwrap();
    let dispatch = client
        .start(
            "orchestration.dispatchCommand",
            json!({"type":"message.dispatch","commandId":"command:message","threadId":THREAD,"messageId":"message:e2e","text":"Hello","attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"}),
            CallKind::Unary,
        )
        .unwrap();
    let (mut dispatched, mut reply, mut completed, mut shell_seen) = (false, false, false, false);
    while !(dispatched && reply && completed && shell_seen) {
        let next = event(&mut socket, &mut client).await;
        match next {
            CallEvent::Exit { id, exit } if id == dispatch => {
                ok(exit);
                dispatched = true;
            }
            CallEvent::Chunk { id, values } if id == subscription => {
                client.acknowledge(id);
                let text = Value::Array(values).to_string();
                reply |= text.contains("\"message.updated\"") && text.contains("\"assistant\"");
                completed |= text.contains("\"run.updated\"") && text.contains("\"completed\"");
            }
            CallEvent::Chunk { id, values } if id == shell_stream => {
                client.acknowledge(id);
                shell_seen |= Value::Array(values).to_string().contains(THREAD);
            }
            CallEvent::Exit { id, exit } if id == subscription || id == shell_stream => {
                panic!(
                    "subscription ended: {}",
                    serde_json::to_value(exit).unwrap()
                )
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    // The thread snapshot over HTTP shows the persisted reply; an unknown thread is 404.
    let snapshot: Value = http
        .get(format!("{origin}/api/orchestration/threads/{THREAD}"))
        .header("cookie", &cookie)
        .header("x-t3-orchestration-protocol", "2")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(snapshot.to_string().contains("\"assistant\""), "{snapshot}");
    let missing = http
        .get(format!("{origin}/api/orchestration/threads/thread:absent"))
        .header("cookie", &cookie)
        .header("x-t3-orchestration-protocol", "2")
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
    runtime.shutdown().await.unwrap();
}

fn dev_inputs(home: &std::path::Path) -> ConfigInputs {
    ConfigInputs {
        flags: ServerFlags {
            base_dir: Some(home.to_string_lossy().into_owned()),
            host: Some("127.0.0.1".into()),
            port: Some(0),
            dev_url: Some("http://127.0.0.1:5733".into()),
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

/// `--dev-url` is TypeScript's dev mode: it seeds the free `devReplay` instance ("Replay (dev)",
/// driver `acpRegistry`) and keeps a user's later edit to it.
#[tokio::test]
async fn dev_mode_seeds_replay_dev_and_keeps_user_edits() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let settings_path = home.join("userdata").join("settings.json");
    let read =
        || -> Value { serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap() };
    let server = t3_server::runtime::start(dev_inputs(home)).await.unwrap();
    let seeded = read();
    let instance = &seeded["providerInstances"]["devReplay"];
    assert_eq!(instance["driver"], "acpRegistry", "{seeded}");
    assert_eq!(instance["displayName"], "Replay (dev)", "{seeded}");
    server.shutdown().await.unwrap();

    // The user turns it off; a restart in dev mode keeps that.
    let mut edited = read();
    edited["providerInstances"]["devReplay"]["enabled"] = json!(false);
    std::fs::write(&settings_path, edited.to_string()).unwrap();
    let server = t3_server::runtime::start(dev_inputs(home)).await.unwrap();
    assert_eq!(read()["providerInstances"]["devReplay"]["enabled"], false);
    server.shutdown().await.unwrap();
}

/// The round's milestone: the server in dev mode (`--dev-url`), as `vp run dev` starts it, runs
/// a turn a client can watch, using only what a client does. Pair, get the scratch project,
/// create a Replay (dev) thread in it, send a message and see the turn complete live. An
/// unserved method answers exactly as Effect's RpcServer does for an unknown request tag.
#[tokio::test]
async fn dev_mode_scratch_project_thread_and_replay_turn_over_the_socket() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let server = t3_server::runtime::start(dev_inputs(home)).await.unwrap();
    let origin = format!("http://{}", server.address);
    let http = reqwest::Client::new();
    let cookie = http
        .post(format!("{origin}/api/auth/browser-session"))
        .json(&json!({"credential": server.pairing_credential}))
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
        server.address,
        ticket["ticket"].as_str().unwrap()
    ))
    .await
    .unwrap();
    let mut client = Client::new(0, Default::default());
    let unary = async |client: &mut Client, socket: &mut Socket, tag: &str, payload: Value| {
        let id = client.start(tag, payload, CallKind::Unary).unwrap();
        loop {
            match event(socket, client).await {
                CallEvent::Exit { id: done, exit } if done == id => return exit,
                CallEvent::Chunk { id, .. } => client.acknowledge(id),
                _ => {}
            }
        }
    };

    // The scratch project is created once and returned again on the next call.
    let scratch = ok(unary(
        &mut client,
        &mut socket,
        "projects.ensureScratch",
        json!({}),
    )
    .await);
    let project = scratch["projectId"].as_str().unwrap().to_owned();
    let again = ok(unary(
        &mut client,
        &mut socket,
        "projects.ensureScratch",
        json!({}),
    )
    .await);
    assert_eq!(again["projectId"], project.as_str());
    // A named project gets its own folder under the managed projects root.
    let named = ok(unary(
        &mut client,
        &mut socket,
        "projects.createNew",
        json!({"name": "Named project"}),
    )
    .await);
    let root = named["workspaceRoot"].as_str().unwrap();
    let projects_root = std::fs::canonicalize(home.join("projects")).unwrap();
    assert!(
        std::fs::canonicalize(root)
            .unwrap()
            .starts_with(&projects_root),
        "{named}"
    );

    // A No project thread with its own folder under the scratch root becomes a project there;
    // one without its own folder is refused with the promotion's message.
    let own_folder = home.join("scratch").join("promote-me");
    std::fs::create_dir_all(&own_folder).unwrap();
    for (id, worktree) in [
        ("thread:promote", json!(own_folder.to_string_lossy())),
        ("thread:no-folder", Value::Null),
    ] {
        ok(unary(
            &mut client,
            &mut socket,
            "orchestration.dispatchCommand",
            json!({"type":"thread.create","commandId":format!("command:{id}"),"threadId":id,"projectId":project,"title":"Scratch work","modelSelection":{"instanceId":"devReplay","model":"multi-turn"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":worktree,"createdBy":"user","creationSource":"web"}),
        )
        .await);
    }
    let promoted = ok(unary(
        &mut client,
        &mut socket,
        "projects.createFromThread",
        json!({"threadId": "thread:promote"}),
    )
    .await);
    let promoted_project = promoted["projectId"].as_str().unwrap();
    assert_ne!(promoted_project, project, "{promoted}");
    let refused = serde_json::to_value(
        unary(
            &mut client,
            &mut socket,
            "projects.createFromThread",
            json!({"threadId": "thread:no-folder"}),
        )
        .await,
    )
    .unwrap();
    let error = &refused["cause"][0]["error"];
    assert_eq!(
        error["_tag"], "OrchestrationDispatchCommandError",
        "{refused}"
    );
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("its own folder"),
        "{refused}"
    );

    // An unserved method: Effect's answer for an unknown request tag, never a structured defect.
    let unserved = unary(
        &mut client,
        &mut socket,
        "server.runStorageCleanup",
        json!({}),
    )
    .await;
    let unserved = serde_json::to_value(unserved).unwrap();
    assert_eq!(
        unserved["cause"][0]["defect"], "Unknown request tag: server.runStorageCleanup",
        "{unserved}"
    );

    let thread = "thread:dev-milestone";
    ok(unary(
        &mut client,
        &mut socket,
        "orchestration.dispatchCommand",
        json!({"type":"thread.create","commandId":"command:dev-thread","threadId":thread,"projectId":project,"title":"Dev milestone","modelSelection":{"instanceId":"devReplay","model":"multi-turn"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"}),
    )
    .await);
    // No suggestion before the first turn: the stream opens with `null`.
    let suggestion = client
        .start(
            "orchestration.subscribePromptSuggestion",
            json!({"threadId": thread}),
            CallKind::Stream,
        )
        .unwrap();
    loop {
        match event(&mut socket, &mut client).await {
            CallEvent::Chunk { id, values } if id == suggestion => {
                client.acknowledge(id);
                assert_eq!(values, vec![Value::Null]);
                break;
            }
            CallEvent::Exit { id, exit } if id == suggestion => {
                panic!("suggestions ended: {}", serde_json::to_value(exit).unwrap())
            }
            CallEvent::Chunk { id, .. } => client.acknowledge(id),
            _ => {}
        }
    }
    let subscription = client
        .start(
            "orchestration.subscribeThread",
            json!({"threadId": thread}),
            CallKind::Stream,
        )
        .unwrap();
    let dispatch = client
        .start(
            "orchestration.dispatchCommand",
            json!({"type":"message.dispatch","commandId":"command:dev-message","threadId":thread,"messageId":"message:dev","text":"Hello","attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"}),
            CallKind::Unary,
        )
        .unwrap();
    // The turn's checkpoint is captured after the run completes; the diff read needs it.
    let (mut dispatched, mut reply, mut completed, mut checkpointed) = (false, false, false, false);
    while !(dispatched && reply && completed && checkpointed) {
        match event(&mut socket, &mut client).await {
            CallEvent::Exit { id, exit } if id == dispatch => {
                ok(exit);
                dispatched = true;
            }
            CallEvent::Chunk { id, values } if id == subscription => {
                client.acknowledge(id);
                let text = Value::Array(values).to_string();
                assert!(
                    !text.contains("\"status\":\"failed\""),
                    "the turn failed: {text}"
                );
                reply |= text.contains("\"message.updated\"") && text.contains("\"assistant\"");
                completed |= text.contains("\"run.updated\"") && text.contains("\"completed\"");
                checkpointed |= text.contains("\"checkpoint.captured\"");
            }
            CallEvent::Exit { id, exit } if id == subscription => {
                panic!(
                    "subscription ended: {}",
                    serde_json::to_value(exit).unwrap()
                )
            }
            CallEvent::Chunk { id, .. } => client.acknowledge(id),
            _ => {}
        }
    }

    // Reads bound on the composed runtime: titles, link metadata, the turn's diff.
    let titles = ok(unary(
        &mut client,
        &mut socket,
        "orchestration.getThreadTitles",
        json!({"threadIds": [thread, "thread:absent"]}),
    )
    .await);
    assert_eq!(titles[thread], "Dev milestone");
    assert_eq!(titles.as_object().unwrap().len(), 1, "{titles}");
    let links = ok(unary(
        &mut client,
        &mut socket,
        "orchestration.getThreadLinkMetadata",
        json!({"threadIds": [thread]}),
    )
    .await);
    assert!(links.get(thread).is_some(), "{links}");
    // An empty range answers without reading checkpoints.
    let diff = ok(unary(
        &mut client,
        &mut socket,
        "orchestration.getTurnDiff",
        json!({"threadId": thread, "fromTurnCount": 1, "toTurnCount": 1}),
    )
    .await);
    assert_eq!(diff["threadId"], thread);
    assert_eq!(diff["diff"], "");
    // The scratch folder is not a git repository, so its checkpoints are captured as missing and
    // no turn is diffable: the typed diff failure, carrying the query's own cause.
    let beyond = unary(
        &mut client,
        &mut socket,
        "orchestration.getFullThreadDiff",
        json!({"threadId": thread, "toTurnCount": 1}),
    )
    .await;
    let beyond = serde_json::to_value(beyond).unwrap();
    let failure = &beyond["cause"][0]["error"];
    assert_eq!(
        failure["_tag"], "OrchestrationGetFullThreadDiffError",
        "{beyond}"
    );
    assert_eq!(failure["message"], "Failed to load full thread diff");
    assert_eq!(
        failure["cause"]["_tag"], "CheckpointTurnRangeUnavailableError",
        "{beyond}"
    );
    assert_eq!(failure["cause"]["availableTurnCount"], 0, "{beyond}");

    // The transcript export: orchestration:read, then the thread's items.
    let transcript_url = format!("{origin}/api/orchestration/threads/{thread}/transcript");
    let transcript = http
        .get(&transcript_url)
        .header("cookie", &cookie)
        .header("x-t3-orchestration-protocol", "2")
        .send()
        .await
        .unwrap();
    assert_eq!(transcript.status(), 200);
    let transcript: Value = transcript.json().await.unwrap();
    assert_eq!(transcript["threadId"], thread);
    assert_eq!(transcript["title"], "Dev milestone");
    assert!(!transcript["items"].as_array().unwrap().is_empty());
    let anonymous = http
        .get(&transcript_url)
        .header("x-t3-orchestration-protocol", "2")
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);
    let grant: Value = http
        .post(format!("{origin}/api/auth/pairing-token"))
        .header("cookie", &cookie)
        .json(&json!({"scopes": ["settings:write"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let unscoped = http
        .post(format!("{origin}/api/auth/browser-session"))
        .json(&json!({"credential": grant["credential"]}))
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
    let refused = http
        .get(&transcript_url)
        .header("cookie", &unscoped)
        .header("x-t3-orchestration-protocol", "2")
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);

    // Search over t3-shared's policy, in-thread find, and command output.
    let found = ok(unary(
        &mut client,
        &mut socket,
        "orchestration.searchThreads",
        // Message text; the client matches titles itself.
        json!({"query": "Hello"}),
    )
    .await);
    assert!(found.to_string().contains(thread), "{found}");
    let find = client
        .start(
            "orchestration.searchThreadStream",
            json!({"threadId": thread, "query": "Hello"}),
            CallKind::Stream,
        )
        .unwrap();
    let first = loop {
        match event(&mut socket, &mut client).await {
            CallEvent::Chunk { id, values } if id == find => {
                client.acknowledge(id);
                break values;
            }
            CallEvent::Exit { id, exit } if id == find => {
                panic!("find ended: {}", serde_json::to_value(exit).unwrap())
            }
            CallEvent::Chunk { id, .. } => client.acknowledge(id),
            _ => {}
        }
    };
    assert!(!first.is_empty());
    let output = client
        .start(
            "orchestration.subscribeCommandOutput",
            json!({"threadId": thread, "itemId": "item:absent"}),
            CallKind::Stream,
        )
        .unwrap();
    let missing = loop {
        match event(&mut socket, &mut client).await {
            CallEvent::Exit { id, exit } if id == output => {
                break serde_json::to_value(exit).unwrap();
            }
            CallEvent::Chunk { id, .. } => client.acknowledge(id),
            _ => {}
        }
    };
    let error = &missing["cause"][0]["error"];
    assert_eq!(
        error["_tag"], "OrchestrationV2CommandOutputError",
        "{missing}"
    );
    assert_eq!(error["message"], "No such command.");

    // Feedback goes through the thread's live provider session, with TypeScript's refusals.
    for (target, cause) in [
        (
            "thread:no-folder",
            "No provider session has run in this thread yet.".to_owned(),
        ),
        (
            thread,
            "Provider 'acpRegistry' does not support feedback uploads.".to_owned(),
        ),
    ] {
        let exit = serde_json::to_value(
            unary(
                &mut client,
                &mut socket,
                "provider.uploadFeedback",
                json!({"threadId": target}),
            )
            .await,
        )
        .unwrap();
        let error = &exit["cause"][0]["error"];
        assert_eq!(error["_tag"], "ProviderUploadFeedbackError", "{exit}");
        assert_eq!(error["cause"], cause.as_str(), "{exit}");
    }
    server.shutdown().await.unwrap();
}

/// Item 5: without `serve` (not headless) the server bootstraps the project for its working
/// directory and names it in `welcome`; a restart finds the same project. No thread is
/// launched: thread launch is not available yet, and start-up continues, as TypeScript's
/// `ThreadLaunchError` path does.
#[tokio::test]
async fn a_start_from_a_folder_bootstraps_its_project_once() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let app = temp.path().join("my-app");
    std::fs::create_dir_all(&app).unwrap();
    let start = || {
        let mut inputs = dev_inputs(&home);
        inputs.flags.dev_url = None;
        inputs.process_cwd = app.clone();
        inputs.headless = false;
        inputs.force_auto_bootstrap = None;
        t3_server::runtime::start(inputs)
    };
    let welcome = async |server: &t3_server::runtime::RunningServer| -> Value {
        let origin = format!("http://{}", server.address);
        let http = reqwest::Client::new();
        let cookie = http
            .post(format!("{origin}/api/auth/browser-session"))
            .json(&json!({"credential": server.pairing_credential}))
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
            server.address,
            ticket["ticket"].as_str().unwrap()
        ))
        .await
        .unwrap();
        let mut client = Client::new(0, Default::default());
        let id = client
            .start("subscribeServerLifecycle", json!({}), CallKind::Stream)
            .unwrap();
        loop {
            match event(&mut socket, &mut client).await {
                CallEvent::Chunk { id: chunk, values } if chunk == id => {
                    client.acknowledge(chunk);
                    if let Some(welcome) = values.into_iter().find(|v| v["type"] == "welcome") {
                        return welcome["payload"].clone();
                    }
                }
                CallEvent::Chunk { id, .. } => client.acknowledge(id),
                other => panic!("unexpected {other:?}"),
            }
        }
    };
    let first = start().await.unwrap();
    let payload = welcome(&first).await;
    let project = payload["bootstrapProjectId"]
        .as_str()
        .expect("a bootstrap project")
        .to_owned();
    assert!(payload.get("bootstrapThreadId").is_none(), "{payload}");
    first.shutdown().await.unwrap();

    let second = start().await.unwrap();
    assert_eq!(
        welcome(&second).await["bootstrapProjectId"],
        project.as_str()
    );
    second.shutdown().await.unwrap();
}

/// An admin socket on a started server: browser session, RPC ticket, upgrade.
async fn admin_socket(server: &t3_server::runtime::RunningServer) -> (Socket, Client) {
    let (_, socket, client) = admin_session(server).await;
    (socket, client)
}

/// Pairs once (the credential is single-use): the session cookie and a socket on it.
async fn admin_session(server: &t3_server::runtime::RunningServer) -> (String, Socket, Client) {
    let origin = format!("http://{}", server.address);
    let http = reqwest::Client::new();
    let cookie = http
        .post(format!("{origin}/api/auth/browser-session"))
        .json(&json!({"credential": server.pairing_credential}))
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
    let (socket, client) = session_socket(server, &cookie).await;
    (cookie, socket, client)
}

async fn session_socket(
    server: &t3_server::runtime::RunningServer,
    cookie: &str,
) -> (Socket, Client) {
    let origin = format!("http://{}", server.address);
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
        "ws://{}/ws?orchestrationProtocol=2&wsTicket={}",
        server.address,
        ticket["ticket"].as_str().unwrap()
    ))
    .await
    .unwrap();
    (socket, Client::new(0, Default::default()))
}

async fn call_exit(socket: &mut Socket, client: &mut Client, tag: &str, payload: Value) -> Value {
    let id = client.start(tag, payload, CallKind::Unary).unwrap();
    loop {
        match event(socket, client).await {
            CallEvent::Exit { id: done, exit } if done == id => {
                return serde_json::to_value(exit).unwrap();
            }
            CallEvent::Chunk { id, .. } => client.acknowledge(id),
            _ => {}
        }
    }
}

/// `approvals-review` H1/M2: the organisation's `maxRuntimeMode` caps thread creation and mode
/// changes over the socket, with TypeScript's message; modes within the cap still work.
#[tokio::test]
async fn the_managed_policy_caps_thread_runtime_modes() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let policy = temp.path().join("policy.json");
    std::fs::write(
        &policy,
        r#"{"schemaVersion":1,"organization":"Acme","limits":{"maxRuntimeMode":"auto-accept-edits"}}"#,
    )
    .unwrap();
    let mut inputs = dev_inputs(&home);
    inputs.env.insert(
        "T3CODE_DEV_MANAGED_POLICY_PATH".into(),
        policy.to_string_lossy().into_owned(),
    );
    let server = t3_server::runtime::start(inputs).await.unwrap();
    let (mut socket, mut client) = admin_socket(&server).await;
    let scratch = call_exit(
        &mut socket,
        &mut client,
        "projects.ensureScratch",
        json!({}),
    )
    .await;
    let project = scratch["value"]["projectId"].as_str().unwrap().to_owned();
    let create = |id: &str, mode: &str| json!({"type":"thread.create","commandId":format!("command:{id}"),"threadId":id,"projectId":project,"title":"Capped","modelSelection":{"instanceId":"devReplay","model":"multi-turn"},"runtimeMode":mode,"interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"});
    let refused_text = |exit: &Value| exit.to_string();

    let full = call_exit(
        &mut socket,
        &mut client,
        "orchestration.dispatchCommand",
        create("thread:full", "full-access"),
    )
    .await;
    assert_eq!(full["_tag"], "Failure", "{full}");
    assert!(
        refused_text(&full).contains("Acme limits threads to Auto-accept edits."),
        "{full}"
    );

    let allowed = call_exit(
        &mut socket,
        &mut client,
        "orchestration.dispatchCommand",
        create("thread:capped", "auto-accept-edits"),
    )
    .await;
    assert_eq!(allowed["_tag"], "Success", "{allowed}");
    let switched = call_exit(
        &mut socket,
        &mut client,
        "orchestration.dispatchCommand",
        json!({"type":"thread.runtime-mode.set","commandId":"command:switch","threadId":"thread:capped","runtimeMode":"full-access","createdBy":"user","creationSource":"web"}),
    )
    .await;
    assert_eq!(switched["_tag"], "Failure", "{switched}");
    assert!(
        refused_text(&switched).contains("Acme limits threads to Auto-accept edits."),
        "{switched}"
    );
    server.shutdown().await.unwrap();
}

/// Waits on `subscribeThread` for the first chunk whose text contains every part of one of
/// `outcomes`, and returns that outcome's first part with the chunk. List every way the awaited
/// step can end, failures included, or a failure waits forever.
async fn thread_outcome<'a>(
    socket: &mut Socket,
    client: &mut Client,
    subscription: &t3_rpc::RequestId,
    outcomes: &[&[&'a str]],
) -> (&'a str, String) {
    loop {
        match event(socket, client).await {
            CallEvent::Chunk { id, values } if id == *subscription => {
                client.acknowledge(id);
                let text = Value::Array(values).to_string();
                if let Some(outcome) = outcomes
                    .iter()
                    .find(|parts| parts.iter().all(|part| text.contains(part)))
                {
                    return (outcome[0], text);
                }
            }
            CallEvent::Chunk { id, .. } => client.acknowledge(id),
            CallEvent::Exit { id, exit } if id == *subscription => {
                panic!(
                    "subscription ended: {}",
                    serde_json::to_value(exit).unwrap()
                )
            }
            _ => {}
        }
    }
}

const RESTORE_THREAD: &str = "thread:restore";

/// Git in `dir` with nothing of the developer's configuration.
fn git_in(dir: &Path, gitconfig: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(["-c", "user.name=T3", "-c", "user.email=t3@example.invalid"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", gitconfig)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// The scripted provider's server on `home`, with a shell for terminals and nothing of the
/// developer's profile.
/// The MCP sessions a restore server issues agents' thread credentials from.
#[cfg(feature = "mcp")]
type AgentCredentials = Arc<t3_server::adapters::mcp::McpSessions>;
#[cfg(not(feature = "mcp"))]
type AgentCredentials = ();

fn agent_credentials() -> AgentCredentials {
    AgentCredentials::default()
}

async fn restore_server(
    home: &Path,
    credentials: &AgentCredentials,
    pause: Option<Arc<RestorePause>>,
) -> t3_server::runtime::RunningServer {
    let mut env = std::collections::BTreeMap::new();
    for key in ["PATH", "SystemRoot", "ComSpec", "WINDIR", "SHELL"] {
        if let Ok(value) = std::env::var(key) {
            env.insert(key.to_owned(), value);
        }
    }
    for key in ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA"] {
        env.insert(key.to_owned(), home.to_string_lossy().into_owned());
    }
    env.insert(
        "GIT_CONFIG_GLOBAL".into(),
        home.join("gitconfig").to_string_lossy().into_owned(),
    );
    t3_server::runtime::start_with(
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
            writer_gate: pause.as_ref().map(|p| p.gate.clone()),
            providers: vec![Arc::new(ScriptedAdapter {
                pause,
                ..ScriptedAdapter::new()
            })],
            dev_replay: false,
            #[cfg(feature = "mcp")]
            mcp_sessions: Some(credentials.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

/// A server whose project folder is `<home>/repository/<scope>` in a committed Git checkout,
/// and one thread in that folder whose first turn has run.
struct RestoreFixture {
    _temp: tempfile::TempDir,
    home: std::path::PathBuf,
    repository: std::path::PathBuf,
    folder: std::path::PathBuf,
    server: t3_server::runtime::RunningServer,
    /// The admin client's session cookie.
    cookie: String,
    credentials: AgentCredentials,
    socket: Socket,
    client: Client,
    subscription: t3_rpc::RequestId,
    /// The thread-start checkpoint, as the client's rewind picks it.
    checkpoint: Value,
}

impl RestoreFixture {
    async fn start(scope: Option<&str>) -> Self {
        Self::start_with_pause(scope, None).await
    }

    async fn start_with_pause(scope: Option<&str>, pause: Option<Arc<RestorePause>>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().to_path_buf();
        std::fs::create_dir_all(home.join("userdata")).unwrap();
        std::fs::write(
            home.join("userdata").join("settings.json"),
            r#"{"providerInstances":{"replay":{"driver":"replay","enabled":true}}}"#,
        )
        .unwrap();
        let repository = home.join("repository");
        let folder = scope.map_or_else(|| repository.clone(), |scope| repository.join(scope));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("tracked.txt"), "first\n").unwrap();
        let gitconfig = home.join("gitconfig");
        git_in(&repository, &gitconfig, &["init", "-q", "-b", "main"]);
        git_in(&repository, &gitconfig, &["add", "."]);
        git_in(&repository, &gitconfig, &["commit", "-q", "-m", "first"]);
        let credentials = agent_credentials();
        let server = restore_server(&home, &credentials, pause).await;
        let layout = t3_config::home::HomeLayout {
            base_dir: home.clone(),
            state_dir: home.join("userdata"),
        };
        t3_persistence::rusqlite::Connection::open(layout.database())
            .unwrap()
            .execute(
                "INSERT INTO projection_projects
                 (project_id, title, workspace_root, scripts_json, created_at, updated_at)
                 VALUES ('project:restore', 'Restore', ?1, '[]', '2026-11-13T12:00:00.000Z', '2026-11-13T12:00:00.000Z')",
                [folder.to_string_lossy()],
            )
            .unwrap();
        let (cookie, mut socket, mut client) = admin_session(&server).await;
        let created = call_exit(
            &mut socket,
            &mut client,
            "orchestration.dispatchCommand",
            json!({"type":"thread.create","commandId":"command:thread","threadId":RESTORE_THREAD,"projectId":"project:restore","title":"Restore","modelSelection":{"instanceId":"replay","model":"replay-model"},"runtimeMode":"full-access","interactionMode":"default","branch":"main","worktreePath":folder,"createdBy":"user","creationSource":"web"}),
        )
        .await;
        assert_eq!(created["_tag"], "Success", "{created}");
        let subscription = client
            .start(
                "orchestration.subscribeThread",
                json!({"threadId": RESTORE_THREAD}),
                CallKind::Stream,
            )
            .unwrap();
        let sent = call_exit(
            &mut socket,
            &mut client,
            "orchestration.dispatchCommand",
            json!({"type":"message.dispatch","commandId":"command:message","threadId":RESTORE_THREAD,"messageId":"message:restore","text":"Hello","attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"}),
        )
        .await;
        assert_eq!(sent["_tag"], "Success", "{sent}");
        let (outcome, text) = thread_outcome(
            &mut socket,
            &mut client,
            &subscription,
            &[
                &["\"run.updated\"", "\"completed\""],
                &["\"run.updated\"", "\"failed\""],
                &["\"run.updated\"", "\"interrupted\""],
            ],
        )
        .await;
        assert!(
            outcome == "\"run.updated\"" && text.contains("\"completed\""),
            "the turn did not complete: {text}"
        );
        let snapshot: Value = reqwest::Client::new()
            .get(format!(
                "http://{}/api/orchestration/threads/{RESTORE_THREAD}",
                server.address
            ))
            .header("cookie", &cookie)
            .header("x-t3-orchestration-protocol", "2")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let checkpoint = snapshot["projection"]["checkpoints"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|checkpoint| {
                checkpoint["ordinalWithinScope"] == 0 && checkpoint["status"] == "ready"
            })
            .unwrap_or_else(|| panic!("no thread-start checkpoint: {snapshot}"))
            .clone();
        Self {
            _temp: temp,
            home,
            repository,
            folder,
            server,
            cookie,
            credentials,
            socket,
            client,
            subscription,
            checkpoint,
        }
    }

    fn folder(&self) -> String {
        self.folder.to_string_lossy().into_owned()
    }

    async fn call(&mut self, tag: &str, payload: Value) -> Value {
        call_exit(&mut self.socket, &mut self.client, tag, payload).await
    }

    /// Requests a file restore to the thread-start checkpoint and returns how it ended: the
    /// failure's message, or `None` once its runs are rolled back.
    async fn restore(&mut self, command: &str) -> Option<String> {
        let requested = self
            .call(
                "orchestration.dispatchCommand",
                json!({"type":"checkpoint.rollback","commandId":command,"threadId":RESTORE_THREAD,"checkpointId":self.checkpoint["id"],"scopeId":self.checkpoint["scopeId"],"restoreFiles":true}),
            )
            .await;
        assert_eq!(requested["_tag"], "Success", "{requested}");
        let failure = format!("\"rollbackFailure\":{{\"requestId\":\"{command}\"");
        let (outcome, text) = thread_outcome(
            &mut self.socket,
            &mut self.client,
            &self.subscription,
            &[&["\"rolled_back\""], &[failure.as_str()]],
        )
        .await;
        (outcome == failure).then_some(text)
    }

    /// Stops the server, runs `between` on its home and the project folder, starts it again on
    /// the same home and resubscribes to the thread.
    async fn restart(self, between: impl FnOnce(&Path, &Path)) -> Self {
        let Self {
            _temp,
            home,
            repository,
            folder,
            server,
            socket,
            client,
            checkpoint,
            ..
        } = self;
        drop((socket, client));
        server.shutdown().await.unwrap();
        between(&home, &folder);
        let credentials = agent_credentials();
        let server = restore_server(&home, &credentials, None).await;
        let (cookie, socket, mut client) = admin_session(&server).await;
        let subscription = client
            .start(
                "orchestration.subscribeThread",
                json!({"threadId": RESTORE_THREAD}),
                CallKind::Stream,
            )
            .unwrap();
        Self {
            _temp,
            home,
            repository,
            folder,
            server,
            cookie,
            credentials,
            socket,
            client,
            subscription,
            checkpoint,
        }
    }
}

#[cfg(all(
    feature = "git",
    feature = "workspace",
    any(feature = "providers", feature = "forge")
))]
async fn remove_recorded_checkout(f: &mut RestoreFixture) -> std::path::PathBuf {
    let path = f.home.join("worktrees/repository/feature");
    let gitconfig = f.home.join("gitconfig");
    git_in(
        &f.repository,
        &gitconfig,
        &["worktree", "add", "-b", "feature", path.to_str().unwrap()],
    );
    std::fs::create_dir(path.join("sub")).unwrap();
    std::fs::write(path.join("sub/file"), "fixture").unwrap();
    git_in(&path, &gitconfig, &["add", "sub/file"]);
    git_in(&path, &gitconfig, &["commit", "-m", "nested terminal cwd"]);
    let update = f.call("orchestration.dispatchCommand",
        json!({"type":"thread.metadata.update","commandId":"command:move","threadId":RESTORE_THREAD,"branch":"feature","worktreePath":path})).await;
    assert_eq!(update["_tag"], "Success", "{update}");
    // Record the existing checkout's identity, as at launch.
    let trusted = f
        .call("vcs.trustRepository", json!({"threadId":RESTORE_THREAD}))
        .await;
    assert_eq!(trusted["_tag"], "Success", "{trusted}");
    git_in(
        &f.repository,
        &gitconfig,
        &["worktree", "remove", path.to_str().unwrap()],
    );
    path
}

#[cfg(all(
    feature = "terminal",
    all(
        feature = "git",
        feature = "workspace",
        any(feature = "providers", feature = "forge")
    )
))]
#[tokio::test]
async fn opening_a_terminal_recovers_only_its_own_recorded_checkout() {
    let mut f = RestoreFixture::start(None).await;
    let path = remove_recorded_checkout(&mut f).await;
    let unknown = f.call("terminal.open", json!({"threadId":"another-thread","terminalId":"foreign","cwd":path,"worktreePath":path})).await;
    assert_eq!(
        unknown["cause"][0]["error"]["_tag"], "TerminalCwdNotFoundError",
        "{unknown}"
    );
    assert!(!path.exists());
    // Reattaching to a running terminal ignores its supplied cwd.
    let live = f
        .call(
            "terminal.open",
            json!({"threadId":RESTORE_THREAD,"terminalId":"live","cwd":f.repository}),
        )
        .await;
    assert_eq!(live["_tag"], "Success", "{live}");
    let (mut socket, mut client) = session_socket(&f.server, &f.cookie).await;
    for restart in [false, true] {
        let attach = client.start("terminal.attach", json!({"threadId":RESTORE_THREAD,"terminalId":"live","cwd":path,"restartIfNotRunning":restart}), CallKind::Stream).unwrap();
        loop {
            match event(&mut socket, &mut client).await {
                CallEvent::Chunk { id, .. } if id == attach => {
                    client.acknowledge(id);
                    break;
                }
                CallEvent::Chunk { id, .. } => client.acknowledge(id),
                CallEvent::Exit { id, .. } if id == attach => {
                    panic!("running terminal did not attach")
                }
                _ => {}
            }
        }
    }
    drop(socket);
    assert!(
        !path.exists(),
        "attach to a running terminal recreated the ignored cwd"
    );
    // The requested cwd is missing, although the checkout root itself still has build leftovers.
    std::fs::create_dir_all(path.join("node_modules")).unwrap();
    let nested = path.join("sub");
    let opened = f.call("terminal.open", json!({"threadId":RESTORE_THREAD,"terminalId":"owned","cwd":nested,"worktreePath":f.repository})).await;
    assert_eq!(opened["_tag"], "Success", "{opened}");
    assert!(path.join(".git").is_file());
    assert_eq!(opened["value"]["cwd"], nested.to_string_lossy().as_ref());
    assert!(!path.join("node_modules").exists());
    f.server.shutdown().await.unwrap();
}

/// A thread keeps its recorded branch when a managed checkout is removed between turns.
#[cfg(all(
    feature = "git",
    feature = "workspace",
    any(feature = "providers", feature = "forge")
))]
#[tokio::test]
async fn a_turn_recreates_its_missing_managed_checkout() {
    let mut f = RestoreFixture::start(None).await;
    let path = remove_recorded_checkout(&mut f).await;
    let sent = f.call("orchestration.dispatchCommand",
        json!({"type":"message.dispatch","commandId":"command:next","threadId":RESTORE_THREAD,"messageId":"message:next","text":"Again","attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"})).await;
    assert_eq!(sent["_tag"], "Success", "{sent}");
    let (_, result) = thread_outcome(
        &mut f.socket,
        &mut f.client,
        &f.subscription,
        &[
            &["\"run.updated\"", "\"completed\""],
            &["\"run.updated\"", "\"failed\""],
        ],
    )
    .await;
    assert!(
        path.join(".git").is_file(),
        "the missing checkout was not recreated: {result}"
    );
    assert!(result.contains("\"completed\""), "{result}");
    assert_eq!(
        std::fs::read_to_string(path.join("tracked.txt")).unwrap(),
        "first\n"
    );
    let refs = f.call("vcs.listRefs", json!({"cwd":path})).await;
    assert_eq!(refs["_tag"], "Success", "{refs}");
    f.server.shutdown().await.unwrap();
}

#[cfg(feature = "workspace")]
#[tokio::test]
async fn overlapping_socket_writers_wait_through_provider_rollback() {
    let pause = Arc::new(RestorePause::default());
    let mut f = RestoreFixture::start_with_pause(None, Some(pause.clone())).await;
    let folder = f.folder();
    let mut calls = Vec::new();
    let mut waiting = pause.gate.waiting();
    let writers = [
        (
            "projects.writeFile",
            json!({"cwd":folder,"relativePath":"after.txt","contents":"released"}),
        ),
        #[cfg(feature = "git")]
        (
            "vcs.createRef",
            json!({"cwd":folder,"refName":"after-restore"}),
        ),
        #[cfg(feature = "terminal")]
        (
            "terminal.open",
            json!({"threadId":"another-thread","terminalId":"writer","cwd":folder}),
        ),
    ];
    let count = writers.len();
    for (tag, payload) in writers {
        let (socket, client) = session_socket(&f.server, &f.cookie).await;
        calls.push((socket, client, tag, payload));
    }
    let file = f.folder.join("after.txt");
    {
        let rollback = f.restore("command:held");
        tokio::pin!(rollback);
        tokio::select! {
            () = pause.entered.notified() => {}
            result = &mut rollback => panic!("provider rollback did not start: {result:?}"),
        }
        let mut writers: futures::stream::FuturesUnordered<_> = calls
            .into_iter()
            .map(|(mut socket, mut client, tag, payload)| async move {
                let exit = call_exit(&mut socket, &mut client, tag, payload).await;
                assert_eq!(exit["_tag"], "Success", "{tag}: {exit}");
            })
            .collect();
        tokio::select! {
            reached = waiting.wait_for(|n| *n == count) => { reached.unwrap(); }
            _ = writers.next() => panic!("a writer ran inside the restore"),
        }
        assert!(!file.exists());
        pause.release.notify_one();
        let (result, _) = tokio::join!(rollback, writers.collect::<Vec<_>>());
        assert_eq!(result, None);
    }
    assert_eq!(std::fs::read_to_string(file).unwrap(), "released");
    f.server.shutdown().await.unwrap();
}

#[cfg(all(feature = "git-actions", feature = "terminal", feature = "preview"))]
fn setup_preview_launch(f: &RestoreFixture, ready: &tokio::net::TcpListener) -> Value {
    // Keep the fixture outside the checkout the rollback is asked to restore.
    let script = f.home.join("serve.cjs");
    std::fs::write(
        &script,
        format!(
            r#"
const http = require("node:http"), net = require("node:net");
const server = http.createServer((req, res) => res.end("setup preview"));
server.listen(0, "127.0.0.1", () => {{
  const ready = net.connect({}, "127.0.0.1", () => ready.end(String(server.address().port)));
}});
"#,
            ready.local_addr().unwrap().port()
        ),
    )
    .unwrap();
    let scripts = json!([{"id":"preview","name":"Setup preview","command":format!("node \"{}\"", script.display()),"icon":"configure","runOnWorktreeCreate":true,"async":false}]);
    let layout = t3_config::home::HomeLayout {
        base_dir: f.home.clone(),
        state_dir: f.home.join("userdata"),
    };
    t3_persistence::rusqlite::Connection::open(layout.database())
        .unwrap()
        .execute(
            "UPDATE projection_projects SET scripts_json=?1 WHERE project_id='project:restore'",
            [scripts.to_string()],
        )
        .unwrap();
    json!({
        "commandId":"command:setup","threadId":"thread:preview","projectId":"project:restore","title":"Preview",
        "modelSelection":{"instanceId":"replay","model":"replay-model"},"runtimeMode":"full-access","interactionMode":"default",
        "workspaceStrategy":{"type":"existing_worktree","worktreePath":f.folder(),"branch":"main"}
    })
}

/// A preview server started by another thread's setup script is a live writer in
/// the shared checkout, including after the launch call has returned.
#[cfg(all(feature = "git-actions", feature = "terminal", feature = "preview"))]
#[tokio::test]
async fn a_live_setup_script_serving_a_preview_refuses_file_restore() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut f = RestoreFixture::start(None).await;
    let ready = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let launch = setup_preview_launch(&f, &ready);
    let launched = f.call("orchestration.launchThread", launch).await;
    assert_eq!(launched["_tag"], "Success", "{launched}");
    let (mut connected, _) =
        tokio::time::timeout(std::time::Duration::from_secs(30), ready.accept())
            .await
            .expect("setup script did not start its preview listener")
            .unwrap();
    let mut port = String::new();
    connected.read_to_string(&mut port).await.unwrap();
    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap()))
        .await
        .unwrap();
    socket
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).await.unwrap();
    assert!(response.ends_with("setup preview"), "{response}");
    // Move the launch thread's projection away. Its still-running setup terminal
    // independently owns this cwd, even though the thread no longer shares it.
    let moved = f.home.join("other-checkout");
    std::fs::create_dir(&moved).unwrap();
    let changed = f.call("orchestration.dispatchCommand", json!({"type":"thread.metadata.update","commandId":"command:move-preview","threadId":"thread:preview","worktreePath":moved,"branch":null})).await;
    assert_eq!(changed["_tag"], "Success", "{changed}");
    let refused = f.restore("command:refuse-preview").await;
    assert!(
        refused
            .as_deref()
            .is_some_and(|text| text.contains("still writing to its folder: terminal")),
        "{refused:?}"
    );
    f.server.shutdown().await.unwrap();
}

/// The launch RPC returns after enqueueing setup. The script itself must wait
/// until rollback releases the hold, even though it belongs to another thread.
#[cfg(all(feature = "git-actions", feature = "terminal", feature = "preview"))]
#[tokio::test]
async fn a_setup_preview_cannot_start_while_rollback_holds_its_folder() {
    let pause = Arc::new(RestorePause::default());
    let mut f = RestoreFixture::start_with_pause(None, Some(pause.clone())).await;
    let ready = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let launch = setup_preview_launch(&f, &ready);
    let (mut socket, mut client) = session_socket(&f.server, &f.cookie).await;
    let mut waiting = pause.gate.waiting();
    {
        let rollback = f.restore("command:hold-setup");
        tokio::pin!(rollback);
        tokio::select! {
            () = pause.entered.notified() => {}
            result = &mut rollback => panic!("provider rollback did not start: {result:?}"),
        }
        let call = tokio::spawn(async move {
            call_exit(
                &mut socket,
                &mut client,
                "orchestration.launchThread",
                launch,
            )
            .await
        });
        tokio::select! {
            result = waiting.wait_for(|n| *n > 0) => { result.unwrap(); }
            connected = ready.accept() => panic!("preview started before restore released its folder: {connected:?}"),
            _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => panic!("setup did not reach the writer gate"),
        }
        pause.release.notify_one();
        let (restored, launched) = tokio::join!(rollback, call);
        assert!(
            restored
                .as_deref()
                .is_none_or(|reason| reason.contains("isolated worktree")),
            "{restored:?}"
        );
        let launched = launched.unwrap();
        assert_eq!(launched["_tag"], "Success", "{launched}");
    }
    tokio::time::timeout(std::time::Duration::from_secs(30), ready.accept())
        .await
        .expect("setup must resume")
        .unwrap();
    f.server.shutdown().await.unwrap();
}

/// t3-git H-2 at the server: a file restore refuses while the thread has a live terminal,
/// naming it, and goes ahead once the terminal is closed.
#[tokio::test]
async fn a_file_restore_refuses_while_the_thread_has_a_live_terminal() {
    let mut fixture = RestoreFixture::start(None).await;
    let folder = fixture.folder();
    let opened = fixture
        .call(
            "terminal.open",
            json!({"threadId":RESTORE_THREAD,"terminalId":"writer","cwd":folder,"worktreePath":folder}),
        )
        .await;
    assert_eq!(opened["_tag"], "Success", "{opened}");
    let refused = fixture.restore("command:rollback-refused").await;
    assert!(
        refused
            .as_deref()
            .is_some_and(|text| text.contains("still writing to its folder: terminal `writer`")),
        "{refused:?}"
    );
    let closed = fixture
        .call(
            "terminal.close",
            json!({"threadId":RESTORE_THREAD,"terminalId":"writer","deleteHistory":true}),
        )
        .await;
    assert_eq!(closed["_tag"], "Success", "{closed}");
    assert_eq!(fixture.restore("command:rollback").await, None);
    fixture.server.shutdown().await.unwrap();
}

/// t3-git H-1 at the server: the repository a project folder belongs to is saved in the state
/// folder, so after a restart a repository planted in that folder refuses the restore with
/// `checkpoint.git.repository-changed`'s message instead of restoring against the plant.
#[tokio::test]
async fn a_repository_planted_in_a_bound_folder_refuses_the_restore_after_a_restart() {
    // A project in a subdirectory of its repository, the layout a nested `.git` redirects.
    let fixture = RestoreFixture::start(Some("pkg")).await;
    let saved = std::fs::read_to_string(
        fixture
            .home
            .join("userdata")
            .join("repository-bindings.json"),
    )
    .unwrap();
    assert!(saved.contains("pkg"), "{saved}");
    let mut fixture = fixture
        .restart(|home, folder| git_in(folder, &home.join("gitconfig"), &["init", "-q"]))
        .await;
    assert!(fixture.folder.join(".git").exists() && fixture.repository.join(".git").exists());
    let refused = fixture.restore("command:rollback-planted").await;
    assert!(
        refused.as_deref().is_some_and(
            |text| text.contains("no longer belongs to the repository this thread started in")
        ),
        "{refused:?}"
    );
    fixture.server.shutdown().await.unwrap();
}

/// Copies a directory tree: a `.git` replaced by an identical copy, as re-cloning in place
/// leaves it.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A client session paired with exactly `scopes`.
async fn scoped_socket(
    server: &t3_server::runtime::RunningServer,
    admin_cookie: &str,
    scopes: &[&str],
) -> (Socket, Client) {
    let origin = format!("http://{}", server.address);
    let http = reqwest::Client::new();
    let grant: Value = http
        .post(format!("{origin}/api/auth/pairing-token"))
        .header("cookie", admin_cookie)
        .json(&json!({ "scopes": scopes }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let cookie = http
        .post(format!("{origin}/api/auth/browser-session"))
        .json(&json!({"credential": grant["credential"]}))
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
        server.address,
        ticket["ticket"].as_str().unwrap()
    ))
    .await
    .unwrap();
    (socket, Client::new(0, Default::default()))
}

/// The agent's own thread credential (its MCP bearer) cannot trust a repository: it is not an
/// RPC session, so the socket refuses it, and no MCP tool trusts.
#[cfg(feature = "mcp")]
async fn agent_cannot_trust(
    server: &t3_server::runtime::RunningServer,
    credentials: &AgentCredentials,
) {
    use tokio_tungstenite::tungstenite::{Error, client::IntoClientRequest};
    let header = credentials
        .read(&t3_contracts::base_schemas::ThreadId(RESTORE_THREAD.into()))
        .expect("the thread's provider session has an MCP credential")
        .authorization_header;
    assert!(header.starts_with("Bearer "));
    let mut request = format!("ws://{}/ws?orchestrationProtocol=2", server.address)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", header.parse().unwrap());
    match tokio_tungstenite::connect_async(request).await {
        Err(Error::Http(response)) => assert_eq!(response.status(), 401),
        Err(other) => panic!("unexpected upgrade failure: {other}"),
        Ok((mut socket, _)) => {
            let mut client = Client::new(0, Default::default());
            let exit = call_exit(
                &mut socket,
                &mut client,
                "vcs.trustRepository",
                json!({"threadId": RESTORE_THREAD}),
            )
            .await;
            assert_eq!(
                exit["_tag"], "Failure",
                "an agent trusted a repository: {exit}"
            );
        }
    }
}

/// `vcs.trustRepository` (Rust only): a folder whose repository was replaced (here by an
/// identical copy, as re-cloning in place does) refuses the restore; a client with the restore's
/// scope trusts it, and the restore goes ahead. A client without the scope and the agent's own
/// thread credential cannot trust.
#[tokio::test]
async fn a_trusted_repository_restores_after_its_folder_was_recloned_in_place() {
    let fixture = RestoreFixture::start(None).await;
    #[cfg(feature = "mcp")]
    agent_cannot_trust(&fixture.server, &fixture.credentials).await;
    let mut fixture = fixture
        .restart(|_, folder| {
            let (git, aside) = (folder.join(".git"), folder.join(".git-aside"));
            std::fs::rename(&git, &aside).unwrap();
            copy_tree(&aside, &git);
            std::fs::remove_dir_all(&aside).unwrap();
        })
        .await;
    let refused = fixture.restore("command:rollback-changed").await;
    assert!(
        refused.as_deref().is_some_and(
            |text| text.contains("no longer belongs to the repository this thread started in")
        ),
        "{refused:?}"
    );

    let (mut socket, mut client) = scoped_socket(
        &fixture.server,
        &fixture.cookie,
        &[
            "orchestration:read",
            "source-control:write",
            "filesystem:write",
        ],
    )
    .await;
    let denied = call_exit(
        &mut socket,
        &mut client,
        "vcs.trustRepository",
        json!({"threadId": RESTORE_THREAD}),
    )
    .await;
    assert_eq!(
        denied["cause"][0]["error"]["requiredPermission"], "orchestration:operate",
        "{denied}"
    );

    let trusted = fixture
        .call("vcs.trustRepository", json!({"threadId": RESTORE_THREAD}))
        .await;
    assert_eq!(trusted["_tag"], "Success", "{trusted}");
    let top_level = trusted["value"]["topLevel"].as_str().unwrap_or_default();
    assert!(
        top_level
            .trim_end_matches(['\\', '/'])
            .ends_with("repository"),
        "{trusted}"
    );
    assert_eq!(trusted["value"]["worktreeOfProject"], false, "{trusted}");
    assert_eq!(fixture.restore("command:rollback-trusted").await, None);
    fixture.server.shutdown().await.unwrap();
}
