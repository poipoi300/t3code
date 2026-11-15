#![cfg(feature = "local-services")]
#![allow(clippy::unwrap_used)]
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use t3_rpc::{CallEvent, CallKind, Client, Exit, Frame};
use t3_server::adapters::config::{ConfigInputs, ServerFlags};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest},
};
type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
fn inputs(temp: &std::path::Path) -> ConfigInputs {
    ConfigInputs {
        flags: ServerFlags {
            base_dir: Some(temp.to_string_lossy().into_owned()),
            host: Some("127.0.0.1".into()),
            port: Some(0),
            ..Default::default()
        },
        env: Default::default(),
        home: temp.into(),
        process_cwd: temp.into(),
        bootstrap: None,
        headless: true,
        force_auto_bootstrap: Some(false),
        service_managed: false,
        cli_log_level: None,
        automatic_port: None,
        persisted_observability: Default::default(),
        static_dir: Some(temp.join("web")),
    }
}
async fn flush(socket: &mut Socket, client: &mut Client) {
    while let Some(m) = client.pop_outgoing() {
        let Frame::Text(t) = t3_rpc::codec::encode(&m).unwrap() else {
            panic!("text expected")
        };
        socket.send(Message::Text(t.into())).await.unwrap();
    }
}
async fn event(socket: &mut Socket, client: &mut Client) -> CallEvent {
    loop {
        flush(socket, client).await;
        if let Some(event) = client.pop_event() {
            return event;
        }
        let message = socket.next().await.unwrap().unwrap();
        let frame = match message {
            Message::Text(t) => Frame::Text(t.to_string()),
            Message::Binary(b) => Frame::Binary(b.to_vec()),
            Message::Ping(p) => {
                socket.send(Message::Pong(p)).await.unwrap();
                continue;
            }
            _ => panic!("connection closed before result"),
        };
        for message in t3_rpc::codec::decode(&frame).unwrap() {
            client.receive(message, 0);
        }
    }
}
async fn unary(socket: &mut Socket, client: &mut Client, tag: &str, payload: Value) -> Exit {
    let id = client.start(tag, payload, CallKind::Unary).unwrap();
    match event(socket, client).await {
        CallEvent::Exit { id: actual, exit } => {
            assert_eq!(actual, id);
            exit
        }
        other => panic!("unexpected {other:?}"),
    }
}
/// The next stream chunk, skipping the config stream's background provider-status updates
/// (probes settle after the snapshot, as in TypeScript); no test here is about them.
async fn chunk(socket: &mut Socket, client: &mut Client) -> Value {
    loop {
        match event(socket, client).await {
            CallEvent::Chunk { id, values } => {
                client.acknowledge(id);
                let value = values.into_iter().next().unwrap();
                if value["type"] != "providerStatuses" {
                    return value;
                }
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
async fn pair(http: &reqwest::Client, base: &str, credential: &str) -> String {
    let response = http
        .post(format!("{base}/api/auth/browser-session"))
        .json(&json!({"credential":credential}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Lax"));
    assert_eq!(
        response.json::<Value>().await.unwrap()["authenticated"],
        true
    );
    cookie.split(';').next().unwrap().into()
}
async fn ticket(http: &reqwest::Client, base: &str, cookie: &str, audience: &str) -> String {
    let response = http
        .post(format!("{base}/api/auth/websocket-ticket/{audience}"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.json::<Value>().await.unwrap()["ticket"]
        .as_str()
        .unwrap()
        .into()
}
#[tokio::test]
async fn real_http_pairing_effect_rpc_scopes_revocation_static_and_shutdown() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("web")).unwrap();
    std::fs::write(temp.path().join("web/index.html"), "<html>t3 client</html>").unwrap();
    std::fs::write(temp.path().join("web/client.js"), "export const x=1").unwrap();
    std::fs::create_dir_all(temp.path().join("web/.vite")).unwrap();
    std::fs::create_dir_all(temp.path().join("web/assets")).unwrap();
    std::fs::write(
        temp.path().join("web/assets/app-12345678.js"),
        "export const x=2",
    )
    .unwrap();
    std::fs::write(
        temp.path().join("web/assets/custom-12345678.js"),
        "export const y=2",
    )
    .unwrap();
    std::fs::write(
        temp.path().join("web/.vite/manifest.json"),
        r#"{"main":{"file":"assets/app-12345678.js"}}"#,
    )
    .unwrap();
    let runtime = t3_server::runtime::start(inputs(temp.path()))
        .await
        .unwrap();
    let base = format!("http://{}", runtime.address);
    let http = reqwest::Client::new();
    assert_eq!(
        http.get(format!("{base}/healthz"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        http.get(format!("{base}/assets/app-12345678.js"))
            .send()
            .await
            .unwrap()
            .headers()["cache-control"],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(
        http.get(format!("{base}/assets/custom-12345678.js"))
            .send()
            .await
            .unwrap()
            .headers()["cache-control"],
        "no-cache"
    );
    let preflight = http
        .request(
            reqwest::Method::OPTIONS,
            format!("{base}/api/auth/browser-session"),
        )
        .header("origin", "https://app.t3.codes")
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "content-type,dpop")
        .send()
        .await
        .unwrap();
    assert_eq!(preflight.status(), 204);
    assert_eq!(preflight.headers()["access-control-allow-origin"], "*");
    let shell = http.get(format!("{base}/threads/1")).send().await.unwrap();
    assert!(!shell.headers().contains_key("etag"));
    assert_eq!(shell.text().await.unwrap(), "<html>t3 client</html>");
    let js = http.get(format!("{base}/client.js")).send().await.unwrap();
    let etag = js.headers()["etag"].clone();
    assert_eq!(
        http.get(format!("{base}/client.js"))
            .header("if-none-match", etag)
            .send()
            .await
            .unwrap()
            .status(),
        304
    );
    assert_eq!(
        http.head(format!("{base}/client.js"))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        http.get(format!("{base}/%2e%2e%5csecret"))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        http.get(format!("{base}/api/auth/session"))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()["authenticated"],
        false
    );
    let cookie = pair(&http, &base, &runtime.pairing_credential).await;
    assert_eq!(
        http.post(format!("{base}/api/auth/browser-session"))
            .json(&json!({"credential":runtime.pairing_credential}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        http.get(format!("{base}/api/auth/session"))
            .header("cookie", &cookie)
            .header("origin", "https://foreign.invalid")
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()["authenticated"],
        false
    );
    let mut wrong = format!("ws://{}/ws", runtime.address)
        .into_client_request()
        .unwrap();
    wrong
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    let error = tokio_tungstenite::connect_async(wrong).await.unwrap_err();
    assert!(matches!(error,tokio_tungstenite::tungstenite::Error::Http(r) if r.status()==426));
    let rpc_ticket = ticket(&http, &base, &cookie, "rpc").await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!(
        "ws://{}/ws?orchestrationProtocol=2&wsTicket={rpc_ticket}",
        runtime.address
    ))
    .await
    .unwrap();
    let mut client = Client::new(0, Default::default());
    assert!(matches!(
        unary(&mut socket, &mut client, "server.probe", json!({})).await,
        Exit::Success { .. }
    ));
    let config = unary(&mut socket, &mut client, "server.getConfig", json!({})).await;
    let Exit::Success {
        value: Some(config),
    } = config
    else {
        panic!("config failed {config:?}")
    };
    assert_eq!(config["environment"]["orchestrationProtocolVersion"], 2);
    assert!(config["settings"].is_object());
    assert!(config["keybindings"].is_array());
    for (method, payload, key) in [
        ("server.getTraceDiagnostics", json!({}), "recordCount"),
        ("server.getProcessDiagnostics", json!({}), "processCount"),
        ("server.getHostResources", json!({}), "totalMemoryBytes"),
        (
            "server.getProcessResourceHistory",
            json!({"windowMs":1000,"bucketMs":1000}),
            "buckets",
        ),
        (
            "server.getResourceTelemetryHistory",
            json!({"windowMs":1000,"bucketMs":1000}),
            "health",
        ),
        ("server.retryResourceTelemetry", json!({}), "accepted"),
        (
            "server.signalProcess",
            json!({"pid":std::process::id(),"startTimeMs":0,"signal":"SIGKILL"}),
            "signaled",
        ),
    ] {
        let result = unary(&mut socket, &mut client, method, payload).await;
        let Exit::Success { value: Some(value) } = result else {
            panic!("diagnostics RPC {method} failed: {result:?}");
        };
        assert!(value.get(key).is_some(), "{method}: {value}");
        if method == "server.signalProcess" {
            assert_eq!(value["signaled"], false);
        }
    }
    let checked = unary(
        &mut socket,
        &mut client,
        "server.checkOtlpEndpoint",
        json!({"signal":"traces","url":format!("{base}/healthz")}),
    )
    .await;
    let Exit::Success {
        value: Some(checked),
    } = checked
    else {
        panic!("endpoint check failed: {checked:?}");
    };
    assert_eq!(checked["_tag"], "Rejected");
    let resource_ticket = ticket(&http, &base, &cookie, "rpc").await;
    let (mut resources_socket, _) = tokio_tungstenite::connect_async(format!(
        "ws://{}/ws?orchestrationProtocol=2&wsTicket={resource_ticket}",
        runtime.address
    ))
    .await
    .unwrap();
    let mut resources_client = Client::new(0, Default::default());
    resources_client
        .start("subscribeResourceTelemetry", json!({}), CallKind::Stream)
        .unwrap();
    let snapshot = chunk(&mut resources_socket, &mut resources_client).await;
    assert_eq!(snapshot["health"]["native"]["status"], "healthy");
    resources_socket.close(None).await.unwrap();
    client
        .start("subscribeServerConfig", json!({}), CallKind::Stream)
        .unwrap();
    assert_eq!(chunk(&mut socket, &mut client).await["type"], "snapshot");
    std::fs::write(
        temp.path().join("userdata/settings.json"),
        r#"{"defaultAutoPull":true}"#,
    )
    .unwrap();
    let settings_update = chunk(&mut socket, &mut client).await;
    assert_eq!(settings_update["type"], "settingsUpdated");
    assert_eq!(
        settings_update["payload"]["settings"]["defaultAutoPull"],
        true
    );
    let subscription = client
        .start("subscribeAuthAccess", json!({}), CallKind::Stream)
        .unwrap();
    let access = chunk(&mut socket, &mut client).await;
    assert_eq!(access["type"], "snapshot");
    assert_eq!(access["payload"]["connectDevices"], json!([]));
    assert!(
        access["payload"]["clientSessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["current"] == true && s["connected"] == true)
    );
    client.interrupt(&subscription);
    flush(&mut socket, &mut client).await;
    while client.pop_event().is_some() {}
    let grant = http
        .post(format!("{base}/api/auth/pairing-token"))
        .header("cookie", &cookie)
        .json(&json!({"scopes":["orchestration:read"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(grant.status(), 200);
    let grant = grant.json::<Value>().await.unwrap();
    let scoped = pair(&http, &base, grant["credential"].as_str().unwrap()).await;
    let scoped_ticket = ticket(&http, &base, &scoped, "rpc").await;
    let (mut scoped_socket, _) = tokio_tungstenite::connect_async(format!(
        "ws://{}/ws?orchestrationProtocol=2&wsTicket={scoped_ticket}",
        runtime.address
    ))
    .await
    .unwrap();
    let mut scoped_client = Client::new(0, Default::default());
    let denied = unary(
        &mut scoped_socket,
        &mut scoped_client,
        "server.updateSettings",
        json!({"patch":{}}),
    )
    .await;
    let encoded = serde_json::to_value(denied).unwrap();
    assert_eq!(
        encoded["cause"][0]["error"]["_tag"],
        "EnvironmentAuthorizationError"
    );
    for (tag, payload, kind) in [
        (
            "server.updateSettings",
            json!({"patch":{}}),
            CallKind::Unary,
        ),
        ("subscribeAuthAccess", json!({}), CallKind::Stream),
        ("server.getProcessDiagnostics", json!({}), CallKind::Unary),
        ("subscribeResourceTelemetry", json!({}), CallKind::Stream),
        (
            "server.signalProcess",
            json!({"pid":std::process::id(),"startTimeMs":0,"signal":"SIGKILL"}),
            CallKind::Unary,
        ),
        (
            "server.checkOtlpEndpoint",
            json!({"signal":"traces","url":format!("{base}/healthz")}),
            CallKind::Unary,
        ),
    ] {
        let mut request = t3_rpc::Request::new(100u64.into(), tag, payload);
        request.discard = Some(t3_rpc::True);
        scoped_client.start_request(request, kind).unwrap();
        match event(&mut scoped_socket, &mut scoped_client).await {
            CallEvent::Exit { exit, .. } => assert_eq!(
                serde_json::to_value(&exit).unwrap()["cause"][0]["error"]["_tag"],
                "EnvironmentAuthorizationError",
                "{tag}: {exit:?}"
            ),
            other => panic!("unexpected discard result {other:?}"),
        }
    }
    // TypeScript's RpcAuthorization.ts gives host resources `orchestration:read` (thread load
    // balancing reads host load), so this session is served, not refused.
    let resources = unary(
        &mut scoped_socket,
        &mut scoped_client,
        "server.getHostResources",
        json!({}),
    )
    .await;
    assert!(
        matches!(resources, Exit::Success { .. }),
        "{}",
        serde_json::to_value(&resources).unwrap()
    );
    let unsupported = unary(
        &mut scoped_socket,
        &mut scoped_client,
        "server.getConfig",
        json!({}),
    )
    .await;
    assert!(matches!(unsupported, Exit::Success { .. }));
    let not_implemented = unary(
        &mut scoped_socket,
        &mut scoped_client,
        "server.refreshProviders",
        json!({}),
    )
    .await;
    assert_eq!(
        serde_json::to_value(not_implemented).unwrap()["cause"][0]["defect"],
        "Unknown request tag: server.refreshProviders"
    );
    let mut request = t3_rpc::Request::new(101u64.into(), "server.refreshProviders", json!({}));
    request.discard = Some(t3_rpc::True);
    scoped_client
        .start_request(request, CallKind::Unary)
        .unwrap();
    match event(&mut scoped_socket, &mut scoped_client).await {
        CallEvent::Exit { exit, .. } => assert_eq!(
            serde_json::to_value(exit).unwrap()["cause"][0]["defect"],
            "Unknown request tag: server.refreshProviders"
        ),
        other => panic!("unexpected unimplemented discard {other:?}"),
    }
    let media_ticket = ticket(&http, &base, &cookie, "media").await;
    let error = tokio_tungstenite::connect_async(format!(
        "ws://{}/ws?orchestrationProtocol=2&wsTicket={media_ticket}",
        runtime.address
    ))
    .await
    .unwrap_err();
    assert!(matches!(error,tokio_tungstenite::tungstenite::Error::Http(r) if r.status()==401));
    // Read the scoped client's id through the admin access stream before revoking it.
    client
        .start("subscribeAuthAccess", json!({}), CallKind::Stream)
        .unwrap();
    let access = chunk(&mut socket, &mut client).await;
    let scoped_id = access["payload"]["clientSessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["current"] == false && s["connected"] == true)
        .unwrap()["sessionId"]
        .clone();
    let revoke = http
        .post(format!("{base}/api/auth/clients/revoke"))
        .header("cookie", &cookie)
        .json(&json!({"sessionId":scoped_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(revoke.status(), 200);
    assert!(matches!(
        scoped_socket.next().await,
        None | Some(Ok(Message::Close(_))) | Some(Err(_))
    ));
    runtime.shutdown().await.unwrap();
    drop(socket);
    drop(scoped_socket);
    assert!(!temp.path().join("userdata/server-runtime.json").exists());
    let restarted = t3_server::runtime::start(inputs(temp.path()))
        .await
        .unwrap();
    restarted.shutdown().await.unwrap();
    println!(
        "ephemeral HTTP + single-use pairing + Effect RPC/config streams + scope denial + revocation + shutdown/restart passed"
    );
}
