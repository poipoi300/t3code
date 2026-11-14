//! `ceiling-rereview` R1: a managed policy from a platform source (the Group Policy registry key)
//! reaches the server as the policy file does. Windows only: the key is below HKCU, named through
//! the development-build override, never the real Policies key.
#![cfg(all(windows, feature = "local-services", feature = "orchestration"))]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use t3_rpc::{CallEvent, CallKind, Client, Frame};
use t3_server::adapters::config::{ConfigInputs, ServerFlags};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

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

/// An admin socket on a started server: browser session, RPC ticket, upgrade.
async fn admin_socket(server: &t3_server::runtime::RunningServer) -> (Socket, Client) {
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
    let (socket, _) = tokio_tungstenite::connect_async(format!(
        "ws://{}/ws?orchestrationProtocol=2&wsTicket={}",
        server.address,
        ticket["ticket"].as_str().unwrap()
    ))
    .await
    .unwrap();
    (socket, Client::new(0, Default::default()))
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

/// `ceiling-rereview` R1: a policy Group Policy writes (here a key below HKCU named through the
/// dev-build override, never the real Policies key) caps thread modes as the file does, and a
/// policy file beside it refuses start.
#[tokio::test]
async fn a_group_policy_registry_policy_caps_thread_runtime_modes() {
    /// A key below `HKCU\Software` written with reg.exe and deleted when dropped.
    struct TemporaryKey(String);
    impl TemporaryKey {
        fn reg(args: &[&str]) {
            let status = std::process::Command::new("reg.exe")
                .args(args)
                .stdout(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "reg.exe {args:?}");
        }
        fn text(&self, name: &str, data: &str) {
            let key = format!(r"HKCU\{}", self.0);
            Self::reg(&["add", &key, "/v", name, "/t", "REG_SZ", "/d", data, "/f"]);
        }
    }
    impl Drop for TemporaryKey {
        fn drop(&mut self) {
            let _ = std::process::Command::new("reg.exe")
                .args(["delete", &format!(r"HKCU\{}", self.0), "/f"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let key = TemporaryKey(format!(
        r"Software\T3CodeServerPolicyTest-{}",
        uuid::Uuid::new_v4()
    ));
    key.text("Organization", "Acme");
    key.text("MaxRuntimeMode", "auto-accept-edits");
    let policy_file = temp.path().join("policy.json");
    std::fs::write(&policy_file, r#"{"schemaVersion":1,"organization":"Acme"}"#).unwrap();
    let mut inputs = dev_inputs(&home);
    inputs.env.insert(
        t3_config::policy_sources::DEV_REGISTRY_KEY_ENV.into(),
        key.0.clone(),
    );
    inputs.env.insert(
        "T3CODE_DEV_MANAGED_POLICY_PATH".into(),
        policy_file.to_string_lossy().into_owned(),
    );
    let refused = match t3_server::runtime::start(inputs.clone()).await {
        Ok(_) => panic!("a registry policy beside a policy file must refuse start"),
        Err(error) => error.to_string(),
    };
    assert!(
        refused.contains(&format!(
            r"T3 Code cannot use the managed policy at HKCU\{}: a policy file at {} also exists; deploy the policy one way.",
            key.0,
            policy_file.display()
        )),
        "{refused}"
    );

    std::fs::remove_file(&policy_file).unwrap();
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
    let full = call_exit(
        &mut socket,
        &mut client,
        "orchestration.dispatchCommand",
        create("thread:full", "full-access"),
    )
    .await;
    assert_eq!(full["_tag"], "Failure", "{full}");
    assert!(
        full.to_string()
            .contains("Acme limits threads to Auto-accept edits."),
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
    server.shutdown().await.unwrap();
}
