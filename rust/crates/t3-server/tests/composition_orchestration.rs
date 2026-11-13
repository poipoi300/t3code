//! Package A: the composed orchestration runtime behind the server, end to end over a real
//! socket. A client pairs, creates a thread, dispatches a message and watches the turn on
//! `subscribeThread` and `subscribeShell`; the orchestration HTTP reads serve the same state.
//!
//! The provider is Replay (dev), registered as TypeScript's dev server does. `projects.mutate`
//! needs the unported `ProjectService`, so the project row is seeded in the live database.
#![cfg(all(feature = "local-services", feature = "orchestration"))]
#![allow(clippy::unwrap_used, clippy::expect_used)]
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use t3_rpc::{CallEvent, CallKind, Client, Exit, Frame};
use t3_server::{
    adapters::config::{ConfigInputs, ServerFlags},
    runtime::StartOptions,
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;


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
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    std::fs::create_dir_all(home.join("userdata")).unwrap();
    std::fs::write(
        home.join("userdata").join("settings.json"),
        r#"{"providerInstances":{"devReplay":{"driver":"acpRegistry","displayName":"Replay (dev)","enabled":true}}}"#,
    )
    .unwrap();
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
        StartOptions {
            providers: Vec::new(),
            dev_replay: true,
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
    assert_eq!(gate.status(), 426);
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
            json!({"type":"thread.create","commandId":"command:thread","threadId":THREAD,"projectId":"project:e2e","title":"E2E thread","modelSelection":{"instanceId":"devReplay","model":"multi-turn"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"}),
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
        match event(&mut socket, &mut client).await {
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
