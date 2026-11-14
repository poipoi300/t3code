//! Package A: the `t3` binary and the composition root's startup and shutdown barriers.
#![cfg(feature = "local-services")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio};
use t3_rpc::{CallEvent, CallKind, Client, Exit, Frame};
use t3_server::{
    adapters::config::{ConfigInputs, ServerFlags},
    services::Phase,
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

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

/// Review H1: without `--host` the server listens on loopback only, as TypeScript's
/// `checkServerReachability` (`input.host ?? "127.0.0.1"`) does; remote starts pass a host.
#[tokio::test]
async fn a_start_without_a_host_listens_on_loopback_only() {
    let temp = tempfile::tempdir().unwrap();
    let mut no_host = inputs(temp.path());
    no_host.flags.host = None;
    let server = t3_server::runtime::start(no_host).await.unwrap();
    assert_eq!(
        server.address.ip(),
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        "{}",
        server.address
    );
    server.shutdown().await.unwrap();
}

/// Review M1: the organisation's policy decides where the server may listen, before binding
/// (`checkServerReachability`), and a policy that cannot be read refuses start.
#[tokio::test]
async fn the_managed_policy_decides_where_the_server_may_listen() {
    let temp = tempfile::tempdir().unwrap();
    let policy = temp.path().join("policy.json");
    let with = |host: Option<&str>| {
        let mut start = inputs(temp.path());
        start.flags.host = host.map(str::to_owned);
        start.env.insert(
            "T3CODE_DEV_MANAGED_POLICY_PATH".into(),
            policy.to_string_lossy().into_owned(),
        );
        start
    };
    std::fs::write(
        &policy,
        r#"{"schemaVersion":1,"organization":"Acme","limits":{"clients":["local-browser"]}}"#,
    )
    .unwrap();
    let refused = t3_server::runtime::start(with(Some("0.0.0.0")))
        .await
        .err()
        .expect("a remote host is refused");
    assert!(refused.to_string().contains("Acme"), "{refused}");
    let local = t3_server::runtime::start(with(None)).await.unwrap();
    assert!(local.address.ip().is_loopback());
    local.shutdown().await.unwrap();

    // A policy part this server cannot enforce yet refuses start rather than being ignored.
    std::fs::write(
        &policy,
        r#"{"schemaVersion":1,"organization":"Acme","limits":{"telemetry":false}}"#,
    )
    .unwrap();
    let unenforced = t3_server::runtime::start(with(None))
        .await
        .err()
        .expect("an unenforced policy part refuses start");
    assert!(unenforced.to_string().contains("telemetry"), "{unenforced}");

    std::fs::write(&policy, "{").unwrap();
    let unreadable = t3_server::runtime::start(with(None))
        .await
        .err()
        .expect("an unreadable policy refuses start");
    assert!(
        unreadable.to_string().contains("managed policy"),
        "{unreadable}"
    );
    // The binary exits 77 for it, which the desktop shell reports at once instead of retrying.
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let refused = binary(&home)
        .env("T3CODE_DEV_MANAGED_POLICY_PATH", &policy)
        .output()
        .await
        .unwrap();
    assert_eq!(refused.status.code(), Some(77), "{refused:?}");
}

/// Review M2: a client that stops reading a large download cannot hold shutdown, the database
/// or the home lock; HTTP connections end with no grace, as in TypeScript.
#[tokio::test]
async fn shutdown_does_not_wait_for_a_stalled_download() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let temp = tempfile::tempdir().unwrap();
    let site = temp.path().join("site");
    std::fs::create_dir_all(&site).unwrap();
    std::fs::write(site.join("index.html"), "<html></html>").unwrap();
    std::fs::write(site.join("large.bin"), vec![7u8; 64 * 1024 * 1024]).unwrap();
    let home = temp.path().join("home");
    let mut start = inputs(&home);
    start.static_dir = Some(site);
    let server = t3_server::runtime::start(start).await.unwrap();
    let mut stalled = tokio::net::TcpStream::connect(server.address)
        .await
        .unwrap();
    stalled
        .write_all(b"GET /large.bin HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut first = [0u8; 16];
    stalled.read_exact(&mut first).await.unwrap();
    assert!(first.starts_with(b"HTTP/1.1 200"));
    // The client now reads nothing more while the server has megabytes left to send.
    server.shutdown().await.unwrap();
    let again = t3_server::runtime::start(inputs(&home)).await.unwrap();
    again.shutdown().await.unwrap();
    drop(stalled);
}

async fn call(socket: &mut Socket, client: &mut Client, tag: &str, kind: CallKind) -> CallEvent {
    client.start(tag, json!({}), kind).unwrap();
    next_event(socket, client).await
}

async fn next_event(socket: &mut Socket, client: &mut Client) -> CallEvent {
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
            other => panic!("connection ended before a result: {other:?}"),
        };
        for message in t3_rpc::codec::decode(&frame).unwrap() {
            client.receive(message, 0);
        }
    }
}

fn success(event: CallEvent) -> Value {
    match event {
        CallEvent::Exit {
            exit: Exit::Success { value },
            ..
        } => value.unwrap_or(Value::Null),
        other => panic!("expected success, got {other:?}"),
    }
}

/// Spawns the binary with nothing from this process's environment but what Windows needs to
/// start a process, so neither a developer `T3CODE_*` variable nor the real home can leak in.
fn spawn_binary(home: &Path) -> tokio::process::Child {
    binary(home).spawn().unwrap()
}

fn binary(home: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_t3"));
    command
        .args(["serve", "--base-dir"])
        .arg(home)
        .args(["--host", "127.0.0.1", "--port", "0"])
        .current_dir(home)
        .env_clear()
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("TEMP", home)
        .env("TMP", home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    for name in ["SystemRoot", "SystemDrive", "windir"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
}

/// Stops the binary the way its operator would. POSIX sends SIGTERM and expects the full
/// clean shutdown. Windows has no safe API to raise Ctrl+Break in another process (this crate
/// takes no `unsafe` without sign-off), so there the binary is force-terminated and only
/// recovery is asserted; clean shutdown with live connections is covered in-process.
async fn stop(child: &mut tokio::process::Child, runtime_file: &Path) {
    #[cfg(unix)]
    {
        let pid = child.id().expect("child still running").to_string();
        let sent = std::process::Command::new("kill")
            .args(["-TERM", &pid])
            .status()
            .unwrap();
        assert!(sent.success());
        let status = child.wait().await.unwrap();
        assert!(status.success(), "t3 exited with {status:?}");
        assert!(
            !runtime_file.exists(),
            "the owner clears its runtime state on clean exit"
        );
    }
    #[cfg(not(unix))]
    {
        child.kill().await.unwrap();
        let _ = runtime_file;
    }
}

#[tokio::test]
async fn binary_serves_health_bootstrap_and_core_rpc_then_stops_and_releases_its_home() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let mut child = spawn_binary(home);
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let (mut origin, mut pairing_url) = (None, None);
    while pairing_url.is_none() {
        let line = stdout
            .next_line()
            .await
            .unwrap()
            .expect("t3 exited before announcing its pairing URL");
        // The canonical headless output of the TypeScript CLI (`formatHeadlessServeOutput`).
        if let Some(value) = line.strip_prefix("Connection string: ") {
            origin = Some(value.to_owned());
        } else if let Some(value) = line.strip_prefix("Pairing URL: ") {
            pairing_url = Some(value.to_owned());
        }
    }
    let (origin, pairing_url) = (origin.unwrap(), pairing_url.unwrap());
    let port: u16 = origin.rsplit(':').next().unwrap().parse().unwrap();
    assert_ne!(port, 0, "the announced origin carries the bound port");
    let credential = pairing_url
        .strip_prefix(&format!("{origin}/pair#token="))
        .expect("pairing URL is a fragment on the announced origin");

    // The published runtime state names the port actually bound, not the requested zero.
    let runtime_file = home.join("userdata/server-runtime.json");
    let runtime_state: Value =
        serde_json::from_slice(&std::fs::read(&runtime_file).unwrap()).unwrap();
    assert_eq!(runtime_state["port"], port);
    assert_eq!(runtime_state["pid"], child.id().unwrap());

    let http = reqwest::Client::new();
    let health = http.get(format!("{origin}/healthz")).send().await.unwrap();
    assert_eq!(health.status(), 200);
    assert_eq!(health.json::<Value>().await.unwrap()["status"], "ok");
    let descriptor: Value = http
        .get(format!("{origin}/.well-known/t3/environment"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(descriptor["orchestrationProtocolVersion"], 2);
    let unauthenticated: Value = http
        .get(format!("{origin}/api/auth/session"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(unauthenticated["authenticated"], false);

    let paired = http
        .post(format!("{origin}/api/auth/browser-session"))
        .json(&json!({ "credential": credential }))
        .send()
        .await
        .unwrap();
    assert_eq!(paired.status(), 200);
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
    let ws_origin = origin.replacen("http://", "ws://", 1);
    let (mut socket, _) = tokio_tungstenite::connect_async(format!(
        "{ws_origin}/ws?orchestrationProtocol=2&wsTicket={}",
        ticket["ticket"].as_str().unwrap()
    ))
    .await
    .unwrap();
    let mut client = Client::new(0, Default::default());
    success(call(&mut socket, &mut client, "server.probe", CallKind::Unary).await);
    let config = success(
        call(
            &mut socket,
            &mut client,
            "server.getConfig",
            CallKind::Unary,
        )
        .await,
    );
    assert_eq!(
        config["environment"]["environmentId"],
        descriptor["environmentId"]
    );
    assert_eq!(config["observability"]["localTracingEnabled"], true);
    let traces = success(
        call(
            &mut socket,
            &mut client,
            "server.getTraceDiagnostics",
            CallKind::Unary,
        )
        .await,
    );
    assert_eq!(traces["parseErrorCount"], 0);
    assert!(
        traces["recordCount"]
            .as_u64()
            .is_some_and(|count| count > 0),
        "{traces}"
    );
    let trace_path = home.join("userdata/logs/server.trace.ndjson");
    let trace_text = std::fs::read_to_string(&trace_path).unwrap();
    assert!(trace_text.contains("server.ready"));
    assert!(!trace_text.contains(credential));
    assert_eq!(
        config["keybindingsConfigPath"],
        home.join("userdata")
            .join("keybindings.json")
            .to_string_lossy()
            .as_ref()
    );
    // The host clock (upstream 2d80cb6bc7), as an ISO timestamp.
    assert!(
        config["serverTime"]
            .as_str()
            .is_some_and(|t| t.ends_with('Z') && t.contains('T')),
        "{config}"
    );
    match call(
        &mut socket,
        &mut client,
        "subscribeServerConfig",
        CallKind::Stream,
    )
    .await
    {
        CallEvent::Chunk { values, .. } => assert_eq!(values[0]["type"], "snapshot"),
        other => panic!("expected the config snapshot, got {other:?}"),
    }

    // A second server on the owned home refuses with the supervisor's do-not-restart code.
    let overflow_backup = home.join("userdata/logs/server.trace.ndjson.42");
    std::fs::write(&overflow_backup, "owned server backup").unwrap();
    let second = spawn_binary(home).wait_with_output().await.unwrap();
    assert_eq!(second.status.code(), Some(78), "{second:?}");
    assert!(
        overflow_backup.exists(),
        "a refused server must not prune the owner's logs"
    );

    // Stop with a live WebSocket and an open subscription.
    stop(&mut child, &runtime_file).await;
    assert!(matches!(
        socket.next().await,
        None | Some(Ok(Message::Close(_))) | Some(Err(_))
    ));
    // Ownership and the database were released: the same home starts again.
    let restarted = t3_server::runtime::start(inputs(home)).await.unwrap();
    restarted.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_second_server_on_the_same_home_is_refused_until_the_owner_releases_it() {
    let temp = tempfile::tempdir().unwrap();
    let first = t3_server::runtime::start(inputs(temp.path()))
        .await
        .unwrap();
    assert!(first.wait_ready().await);
    assert_eq!(first.phase(), Phase::Ready);
    let runtime_file = temp.path().join("userdata/server-runtime.json");
    let published = std::fs::read(&runtime_file).unwrap();

    let Err(refused) = t3_server::runtime::start(inputs(temp.path())).await else {
        panic!("a second owner must not start");
    };
    assert!(
        matches!(refused, t3_server::Error::StateDirOwned(_)),
        "{refused:?}"
    );
    assert_eq!(
        std::fs::read(&runtime_file).unwrap(),
        published,
        "the refused server must not overwrite the owner's runtime state"
    );

    first.shutdown().await.unwrap();
    assert!(!runtime_file.exists());
    let second = t3_server::runtime::start(inputs(temp.path()))
        .await
        .unwrap();
    let port = second.address.port();
    let state: Value = serde_json::from_slice(&std::fs::read(&runtime_file).unwrap()).unwrap();
    assert_eq!(state["port"], port);
    second.shutdown().await.unwrap();
}

async fn register_peer_link(origin: std::net::SocketAddr) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("http://{origin}/oauth/mcp/register"))
        .json(&json!({
            "client_name": "Peer",
            "redirect_uris": ["http://127.0.0.1:8765/callback"],
            "token_endpoint_auth_method": "none",
            "software_id": "t3code-peer-link",
        }))
        .send()
        .await
        .unwrap();
    (response.status().as_u16(), response.json().await.unwrap())
}

#[tokio::test]
async fn peer_link_registration_follows_the_linked_environments_setting_at_startup() {
    let temp = tempfile::tempdir().unwrap();
    let server = t3_server::runtime::start(inputs(temp.path()))
        .await
        .unwrap();
    let (status, body) = register_peer_link(server.address).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (400, Some("invalid_client_metadata"))
    );
    server.shutdown().await.unwrap();

    std::fs::write(
        temp.path().join("userdata").join("settings.json"),
        r#"{"linkedEnvironmentsEnabled":true}"#,
    )
    .unwrap();
    let server = t3_server::runtime::start(inputs(temp.path()))
        .await
        .unwrap();
    let (status, body) = register_peer_link(server.address).await;
    assert_eq!(status, 200, "{body}");
    server.shutdown().await.unwrap();
}

/// Every request header the Rust clients send (t3-client and t3-platform's web and native HTTP)
/// must pass a cross-origin preflight on every route group, with TypeScript's allow-list
/// (`httpCors.ts`): GET/POST/OPTIONS, max-age 600, wildcard origin without credentials.
#[tokio::test]
async fn the_real_clients_preflight_passes_on_every_route_group() {
    let temp = tempfile::tempdir().unwrap();
    let server = t3_server::runtime::start(inputs(temp.path()))
        .await
        .unwrap();
    let http = reqwest::Client::new();
    let client_headers = [
        "authorization",
        "dpop",
        "content-type",
        "traceparent",
        "x-t3-orchestration-protocol",
    ];
    for path in [
        "/.well-known/t3/environment",
        "/api/auth/session",
        "/api/auth/browser-session",
        "/api/auth/websocket-ticket/rpc",
        "/api/orchestration/shell",
        "/ws",
    ] {
        let preflight = http
            .request(
                reqwest::Method::OPTIONS,
                format!("http://{}{path}", server.address),
            )
            .header("origin", "https://app.t3.codes")
            .header("access-control-request-method", "GET")
            .header("access-control-request-headers", client_headers.join(","))
            .send()
            .await
            .unwrap();
        assert_eq!(preflight.status(), 204, "{path}");
        let headers = preflight.headers();
        assert_eq!(headers["access-control-allow-origin"], "*", "{path}");
        assert_eq!(headers["access-control-max-age"], "600", "{path}");
        assert!(
            !headers.contains_key("access-control-allow-credentials"),
            "{path}"
        );
        let allowed = headers["access-control-allow-headers"].to_str().unwrap();
        for header in client_headers {
            assert!(
                allowed.split(',').any(|h| h.trim() == header),
                "{path}: {header}"
            );
        }
        // TypeScript does not allow it either, so no client may send it cross-origin.
        assert!(!allowed.contains("cache-control"), "{path}");
        let methods = headers["access-control-allow-methods"].to_str().unwrap();
        assert!(
            ["GET", "POST", "OPTIONS"]
                .iter()
                .all(|m| methods.contains(m)),
            "{path}"
        );
    }
    // The simple cross-origin read the descriptor needs carries the wildcard origin too.
    let descriptor = http
        .get(format!(
            "http://{}/.well-known/t3/environment",
            server.address
        ))
        .header("origin", "https://app.t3.codes")
        .send()
        .await
        .unwrap();
    assert_eq!(descriptor.status(), 200);
    assert_eq!(descriptor.headers()["access-control-allow-origin"], "*");
    server.shutdown().await.unwrap();
}
