//! Review: every served method refuses a session that lacks exactly one of the scopes it
//! requires, with the typed `EnvironmentAuthorizationError`, before its handler runs. Payloads
//! are the contract's own samples (`fixtures/samples`), chosen so the guard accepts them.
#![cfg(feature = "local-services")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use t3_auth::authorization::required_scopes_for_rpc_call;
use t3_contracts::{auth::AuthEnvironmentScope, rpc::WS_METHODS};
use t3_rpc::{CallEvent, CallKind, Client, Exit, Frame};
use t3_server::adapters::config::{ConfigInputs, ServerFlags};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// A payload the guard accepts, from the method's schema samples.
fn sample(tag: &str, schema: &str) -> Option<(Value, Vec<AuthEnvironmentScope>)> {
    let path = format!(
        "{}/../../fixtures/samples/{schema}.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|payload| {
            let scopes = required_scopes_for_rpc_call(tag, &payload).ok()?;
            Some((payload, scopes))
        })
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
    tokio_tungstenite::connect_async(format!(
        "ws://{address}/ws?orchestrationProtocol=2&wsTicket={}",
        ticket["ticket"].as_str().unwrap()
    ))
    .await
    .unwrap()
    .0
}

async fn session(origin: &str, credential: &Value) -> (String, Value) {
    let response = reqwest::Client::new()
        .post(format!("{origin}/api/auth/browser-session"))
        .json(&json!({ "credential": credential }))
        .send()
        .await
        .unwrap();
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    (cookie, response.json().await.unwrap())
}

/// Methods no group serves yet, in the default test environment (no Node runtime). A group that
/// silently drops out, or a method that starts being served, fails here: move it in this list
/// together with `briefs/server-unserved.md`.
const UNSERVED_WITHOUT_NODE: &[&str] = &[
    "device.action",
    "device.close",
    "device.configure",
    "device.detail",
    "device.easAccount",
    "device.list",
    "device.open",
    "device.shutdown",
    "device.simlockIdentity",
    "device.testHost",
    "guestShare.create",
    "orchestration.getNativeChildWork",
    "preview.adjust",
    "preview.clearProfile",
    "preview.close",
    "preview.list",
    "preview.open",
    "preview.reportProfiles",
    "preview.reportStatus",
    "preview.resize",
    "preview.stopLocalServer",
    "provider.chatgpt.handoff.subscribe",
    "provider.consumeResetCredit",
    "pullRequests.generateReviewGuide",
    "pullRequests.subscribeLinkedThreadRefreshes",
    "server.commitDesktopUpdate",
    "server.getStorageCleanupReport",
    "server.importAcpRegistrySession",
    "server.refreshProviders",
    "server.runStorageCleanup",
    "server.updateServer",
    "server.updateServerWithProgress",
    "subscribeDeviceState",
    "subscribeDiscoveredLocalServers",
    "subscribePreviewEvents",
];
/// With Node and the preview edge present, the devices and preview groups are served too.
const UNSERVED_WITH_NODE: &[&str] = &[
    "guestShare.create",
    "orchestration.getNativeChildWork",
    "provider.chatgpt.handoff.subscribe",
    "provider.consumeResetCredit",
    "pullRequests.generateReviewGuide",
    "pullRequests.subscribeLinkedThreadRefreshes",
    "server.commitDesktopUpdate",
    "server.getStorageCleanupReport",
    "server.importAcpRegistrySession",
    "server.refreshProviders",
    "server.runStorageCleanup",
    "server.updateServer",
    "server.updateServerWithProgress",
];

#[tokio::test]
async fn every_served_method_refuses_a_session_missing_one_required_scope() {
    sweep(Default::default(), UNSERVED_WITHOUT_NODE).await;
}

/// The same sweep with the optional groups served: the runner's Node on PATH (devices and
/// preview) and preview's fake Playwright module, as `composition_preview` uses.
#[tokio::test]
async fn every_optional_group_is_swept_when_its_runtime_is_present() {
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    let Some(node) = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join(name))
        .find(|path| path.is_absolute() && path.is_file())
    else {
        panic!("this sweep needs Node on the test runner's PATH, as composition_preview does");
    };
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../t3-preview/driver/tests/worker-fixture.cjs");
    let module = std::env::temp_dir().join(format!("t3-sweep-playwright-{}.cjs", std::process::id()));
    // `composition_preview`'s fake Playwright edge: launches resolve, a CDP screencast answers.
    let edge = r#"
const launch=chromium.launch;
chromium.launch=async(...args)=>{
  const browser=await launch(...args),create=browser.newContext;
  browser.newContext=async(...args)=>{
    const context=await create(...args),connect=context.newCDPSession;
    context.newCDPSession=async(...args)=>{
      const session=await connect(...args),send=session.send;
      session.send=async(method,params)=>{
        if(method==='Page.startScreencast')queueMicrotask(()=>session.emit('Page.screencastFrame',{data:'/9j/2Q==',sessionId:1,metadata:{deviceWidth:1280,deviceHeight:800}}));
        return send(method,params);
      };return session;
    };return context;
  };return browser;
};
chromium.launchPersistentContext=async(_directory,options)=>(await chromium.launch(options)).newContext();
chromium.connectOverCDP=async()=>{throw Error('unexpected desktop launch')};
exports.chromium=chromium;
"#;
    std::fs::write(
        &module,
        format!(
            "const {{chromium}}=require({});{edge}",
            serde_json::to_string(&fixture).unwrap()
        ),
    )
    .unwrap();
    let mut env: std::collections::BTreeMap<String, String> =
        ["SystemRoot", "ComSpec", "WINDIR"]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
            .collect();
    let homes = tempfile::tempdir().unwrap();
    for name in ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA"] {
        env.insert(name.into(), homes.path().to_string_lossy().into_owned());
    }
    env.insert(
        "PATH".into(),
        node.parent().unwrap().to_string_lossy().into_owned(),
    );
    env.insert(
        "T3_PREVIEW_PLAYWRIGHT_MODULE".into(),
        module.to_string_lossy().into_owned(),
    );
    sweep(env, UNSERVED_WITH_NODE).await;
    let _ = std::fs::remove_file(module);
}

async fn sweep(env: std::collections::BTreeMap<String, String>, expected_unserved: &[&str]) {
    let temp = tempfile::tempdir().unwrap();
    let server = t3_server::runtime::start(ConfigInputs {
        flags: ServerFlags {
            base_dir: Some(temp.path().to_string_lossy().into_owned()),
            host: Some("127.0.0.1".into()),
            port: Some(0),
            ..Default::default()
        },
        env,
        home: temp.path().into(),
        process_cwd: temp.path().into(),
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
    let mut unserved: Vec<&str> = server.unserved_methods.clone();
    unserved.sort_unstable();
    let mut expected: Vec<&str> = expected_unserved.to_vec();
    expected.sort_unstable();
    assert_eq!(unserved, expected, "the unserved list changed");
    let origin = format!("http://{}", server.address);
    let (admin, body) = session(&origin, &json!(server.pairing_credential)).await;
    let held: Vec<AuthEnvironmentScope> =
        serde_json::from_value(body["permissions"].clone()).unwrap();

    let fixture: Vec<Value> =
        serde_json::from_str(include_str!("../../../fixtures/rpc.json")).unwrap();
    let served: std::collections::BTreeSet<&str> = WS_METHODS
        .iter()
        .map(|m| m.tag)
        .filter(|tag| !server.unserved_methods.contains(tag))
        .collect();
    // The scope to withhold, and the calls refused for it.
    let mut by_scope: BTreeMap<String, Vec<(&str, Value, bool)>> = BTreeMap::new();
    let mut unchecked = Vec::new();
    for row in fixture.iter().filter(|r| r["group"] == "WsRpcGroup") {
        let tag = row["tag"].as_str().unwrap();
        if !served.contains(tag) {
            continue;
        }
        let schema = row["payload"]["$ref"]
            .as_str()
            .and_then(|r| r.rsplit('/').next())
            .unwrap();
        let Some((payload, required)) = sample(tag, schema) else {
            unchecked.push(tag);
            continue;
        };
        let Some(scope) = required.iter().find(|s| held.contains(s)) else {
            unchecked.push(tag);
            continue;
        };
        let streams = row["streams"].as_bool().unwrap();
        by_scope
            .entry(
                serde_json::to_value(scope)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned(),
            )
            .or_default()
            .push((tag, payload, streams));
    }
    assert!(
        unchecked.is_empty(),
        "no checkable sample for {unchecked:?}"
    );

    let mut checked = 0;
    for (withheld, calls) in by_scope {
        let scopes: Vec<Value> = held
            .iter()
            .map(|s| serde_json::to_value(s).unwrap())
            .filter(|s| s.as_str() != Some(withheld.as_str()))
            .collect();
        let grant: Value = reqwest::Client::new()
            .post(format!("{origin}/api/auth/pairing-token"))
            .header("cookie", &admin)
            .json(&json!({ "scopes": scopes }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let (cookie, _) = session(&origin, &grant["credential"]).await;
        let mut socket = connect(&origin, server.address, &cookie).await;
        let mut client = Client::new(0, Default::default());
        for (tag, payload, streams) in calls {
            let kind = if streams {
                CallKind::Stream
            } else {
                CallKind::Unary
            };
            let id = client.start(tag, payload, kind).unwrap();
            let exit = loop {
                match event(&mut socket, &mut client).await {
                    CallEvent::Exit { id: done, exit } if done == id => break exit,
                    CallEvent::Chunk { id: chunk, .. } if chunk == id => {
                        panic!("{tag} streamed to a session without {withheld}")
                    }
                    _ => {}
                }
            };
            let encoded = serde_json::to_value(&exit).unwrap();
            assert!(matches!(exit, Exit::Failure { .. }), "{tag}: {encoded}");
            let error = &encoded["cause"][0]["error"];
            assert_eq!(
                error["_tag"], "EnvironmentAuthorizationError",
                "{tag} without {withheld}: {encoded}"
            );
            // `requiredScope` carries the legacy name; `requiredPermission` the scope itself.
            assert_eq!(
                error["requiredPermission"],
                withheld.as_str(),
                "{tag}: {encoded}"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, served.len(), "every served method was checked");
    println!("scope sweep: {checked} served methods refused");
    server.shutdown().await.unwrap();
}

/// Review: a browser cookie cannot upgrade from a page on another origin (cross-site WebSocket
/// hijacking). A ticket is not single-use: TypeScript's `verifyWebSocketToken` records no use,
/// so a ticket opens connections until it expires, as here.
#[tokio::test]
async fn a_foreign_origin_cannot_upgrade_with_the_cookie() {
    use tokio_tungstenite::tungstenite::{Error, client::IntoClientRequest, http::HeaderValue};
    let temp = tempfile::tempdir().unwrap();
    let server = t3_server::runtime::start(ConfigInputs {
        flags: ServerFlags {
            base_dir: Some(temp.path().to_string_lossy().into_owned()),
            host: Some("127.0.0.1".into()),
            port: Some(0),
            ..Default::default()
        },
        env: Default::default(),
        home: temp.path().into(),
        process_cwd: temp.path().into(),
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
    let origin = format!("http://{}", server.address);
    let (cookie, _) = session(&origin, &json!(server.pairing_credential)).await;
    let status = |result: Result<_, Error>| match result {
        Err(Error::Http(response)) => response.status().as_u16(),
        Ok(_) => 101,
        Err(other) => panic!("unexpected {other}"),
    };

    let ticket: Value = reqwest::Client::new()
        .post(format!("{origin}/api/auth/websocket-ticket/rpc"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let url = format!(
        "ws://{}/ws?orchestrationProtocol=2&wsTicket={}",
        server.address,
        ticket["ticket"].as_str().unwrap()
    );
    let first = tokio_tungstenite::connect_async(url.as_str()).await;
    assert_eq!(status(first.map(|(socket, _)| socket)), 101);

    let plain = format!("ws://{}/ws?orchestrationProtocol=2", server.address);
    let upgrade = |page: Option<&str>| {
        let mut request = plain.as_str().into_client_request().unwrap();
        request
            .headers_mut()
            .insert("cookie", HeaderValue::from_str(&cookie).unwrap());
        if let Some(page) = page {
            request
                .headers_mut()
                .insert("origin", HeaderValue::from_str(page).unwrap());
        }
        request
    };
    let same = tokio_tungstenite::connect_async(upgrade(Some(&origin))).await;
    assert_eq!(status(same.map(|(socket, _)| socket)), 101);
    let foreign = tokio_tungstenite::connect_async(upgrade(Some("https://attacker.example"))).await;
    assert_eq!(status(foreign.map(|(socket, _)| socket)), 401);
    server.shutdown().await.unwrap();
}
