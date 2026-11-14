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

struct ScriptedAdapter {
    instance: ProviderInstanceId,
    capabilities: OrchestrationV2ProviderCapabilities,
}

impl ScriptedAdapter {
    fn new() -> Self {
        Self {
            instance: ProviderInstanceId("replay".into()),
            capabilities: serde_json::from_value(
                sample("OrchestrationV2ProviderSession")["capabilities"].clone(),
            )
            .expect("capabilities"),
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
            session: serde_json::from_value(session).map_err(provider_error)?,
            subscribers: Mutex::new(Vec::new()),
        }))
    }
}

struct ScriptedSession {
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
    async fn rollback(&self, _input: RollbackInput) -> ProviderResult<ThreadSnapshot> {
        Err(ProviderError::Unsupported("rollback"))
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
        },
        json!({"instanceId":"replay","model":"replay-model"}),
    )
    .await;
}

#[tokio::test]
#[ignore = "t3-orchestration: Replay in the composed runtime fails to start the turn with \
            'ACP provider thread has no native session ID' (orchestration integrator)"]
async fn a_dispatched_message_runs_a_replay_turn_over_the_socket() {
    turn_over_the_socket(
        r#"{"providerInstances":{"devReplay":{"driver":"acpRegistry","displayName":"Replay (dev)","enabled":true}}}"#,
        StartOptions {
            providers: Vec::new(),
            dev_replay: true,
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
