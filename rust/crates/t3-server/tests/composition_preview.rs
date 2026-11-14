#![cfg(feature = "preview")]
#![allow(clippy::unwrap_used)]
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use t3_rpc::{CallEvent, CallKind, Client, Exit, Frame};
use t3_server::adapters::config::{ConfigInputs, ServerFlags};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

const METHODS: &[&str] = &[
    "preview.open",
    "preview.resize",
    "preview.adjust",
    "preview.close",
    "preview.list",
    "preview.clearProfile",
    "preview.reportProfiles",
    "preview.reportStatus",
    "preview.stopLocalServer",
    "subscribePreviewEvents",
    "subscribeDiscoveredLocalServers",
];
type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
struct Server {
    root: tempfile::TempDir,
    runtime: t3_server::runtime::RunningServer,
    origin: String,
    cookie: String,
    socket: Socket,
    client: Client,
    chunks: std::collections::VecDeque<Value>,
}
impl Server {
    async fn start(available: bool) -> Self {
        Self::start_with(available, |_, _| {}).await
    }
    async fn start_with(
        available: bool,
        extra: impl FnOnce(&Path, &mut std::collections::BTreeMap<String, String>),
    ) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut env: std::collections::BTreeMap<_, _> = ["SystemRoot", "ComSpec", "WINDIR"]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
            .collect();
        if available {
            let node = node();
            env.insert(
                "PATH".into(),
                node.parent().unwrap().to_string_lossy().into_owned(),
            );
            let module = root.path().join("playwright.cjs");
            let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../t3-preview/driver/tests/worker-fixture.cjs");
            let prefix = format!(
                "const {{chromium}}=require({});",
                serde_json::to_string(&fixture).unwrap()
            );
            let fixture = r#"
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
            std::fs::write(&module, prefix + fixture).unwrap();
            env.insert(
                "T3_PREVIEW_PLAYWRIGHT_MODULE".into(),
                module.to_string_lossy().into_owned(),
            );
        }
        extra(root.path(), &mut env);
        let runtime = t3_server::runtime::start(ConfigInputs {
            flags: ServerFlags {
                base_dir: Some(root.path().to_string_lossy().into_owned()),
                host: Some("127.0.0.1".into()),
                port: Some(0),
                ..Default::default()
            },
            env,
            home: root.path().into(),
            process_cwd: root.path().into(),
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
            .json(&json!({"credential":runtime.pairing_credential}))
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
        let socket = connect(&runtime, &cookie).await;
        Self {
            root,
            runtime,
            origin,
            cookie,
            socket,
            client: Client::new(0, Default::default()),
            chunks: Default::default(),
        }
    }
    fn thread(&self, id: &str) {
        let db = t3_persistence::rusqlite::Connection::open(
            self.root.path().join("userdata/statev2.sqlite"),
        )
        .unwrap();
        db.execute("INSERT INTO orchestration_v2_projection_threads (thread_id,project_id,title,default_provider,runtime_mode,interaction_mode,created_at,updated_at,payload_json) VALUES (?1,'project',?1,'codex','full-access','default','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z','{}')",[id]).unwrap();
    }
    async fn event(&mut self) -> CallEvent {
        loop {
            while let Some(message) = self.client.pop_outgoing() {
                let Frame::Text(text) = t3_rpc::codec::encode(&message).unwrap() else {
                    panic!("text")
                };
                self.socket.send(Message::Text(text.into())).await.unwrap();
            }
            if let Some(event) = self.client.pop_event() {
                return event;
            }
            let frame = match self.socket.next().await.unwrap().unwrap() {
                Message::Text(text) => Frame::Text(text.to_string()),
                Message::Binary(bytes) => Frame::Binary(bytes.to_vec()),
                Message::Ping(bytes) => {
                    self.socket.send(Message::Pong(bytes)).await.unwrap();
                    continue;
                }
                other => panic!("unexpected {other:?}"),
            };
            for message in t3_rpc::codec::decode(&frame).unwrap() {
                self.client.receive(message, 0);
            }
        }
    }
    async fn call(&mut self, tag: &str, input: Value) -> Exit {
        let id = self.client.start(tag, input, CallKind::Unary).unwrap();
        loop {
            match self.event().await {
                CallEvent::Exit { id: done, exit } if id == done => return exit,
                CallEvent::Chunk { id, values } => {
                    self.client.acknowledge(id);
                    self.chunks.extend(values);
                }
                _ => {}
            }
        }
    }
    async fn chunk(&mut self) -> Value {
        if let Some(value) = self.chunks.pop_front() {
            return value;
        }
        match self.event().await {
            CallEvent::Chunk { id, values } => {
                self.client.acknowledge(id);
                self.chunks.extend(values);
                self.chunks.pop_front().unwrap()
            }
            event => panic!("expected chunk: {event:?}"),
        }
    }
    async fn scopes(&mut self, scopes: &[&str]) {
        let http = reqwest::Client::new();
        let grant: Value = http
            .post(format!("{}/api/auth/pairing-token", self.origin))
            .header("cookie", &self.cookie)
            .json(&json!({"scopes":scopes}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let paired = http
            .post(format!("{}/api/auth/browser-session", self.origin))
            .json(&json!({"credential":grant["credential"]}))
            .send()
            .await
            .unwrap();
        self.cookie = paired.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        self.socket.close(None).await.unwrap();
        self.socket = connect(&self.runtime, &self.cookie).await;
        self.client = Client::new(0, Default::default());
    }
    async fn viewer(&self, thread: &str, tab: &str) -> Socket {
        self.viewer_cookie(thread, tab, &self.cookie).await
    }
    async fn viewer_cookie(&self, thread: &str, tab: &str, cookie: &str) -> Socket {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = format!(
            "ws://{}/api/preview-stream/ws?threadId={thread}&tabId={tab}",
            self.runtime.address
        )
        .into_client_request()
        .unwrap();
        request
            .headers_mut()
            .insert("cookie", cookie.parse().unwrap());
        tokio_tungstenite::connect_async(request).await.unwrap().0
    }
}
fn node() -> PathBuf {
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|directory| directory.join(name))
        .find(|path| path.is_absolute() && path.is_file())
        .expect(
            "composition preview's fake Playwright edge requires Node on the test runner's PATH",
        )
}
async fn connect(runtime: &t3_server::runtime::RunningServer, cookie: &str) -> Socket {
    let ticket: Value = reqwest::Client::new()
        .post(format!(
            "http://{}/api/auth/websocket-ticket/rpc",
            runtime.address
        ))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    tokio_tungstenite::connect_async(format!(
        "ws://{}/ws?orchestrationProtocol=2&wsTicket={}",
        runtime.address,
        ticket["ticket"].as_str().unwrap()
    ))
    .await
    .unwrap()
    .0
}
fn success(exit: Exit) -> Value {
    match exit {
        Exit::Success { value } => value.unwrap_or(Value::Null),
        other => panic!("expected success: {other:?}"),
    }
}
fn failure(exit: Exit, tag: &str) -> Value {
    let wire = serde_json::to_value(exit).unwrap();
    let error = wire["cause"][0]["error"].clone();
    assert_eq!(error["_tag"], tag, "{wire}");
    error
}

#[tokio::test]
async fn preview_is_optional_without_node_and_all_eleven_methods_are_served_with_it() {
    let mut absent = Server::start(false).await;
    for method in METHODS {
        assert!(absent.runtime.unserved_methods.contains(method), "{method}");
    }
    let exit = absent
        .call("preview.list", json!({"threadId":"thread"}))
        .await;
    assert_eq!(
        serde_json::to_value(exit).unwrap()["cause"][0]["defect"],
        "Unknown request tag: preview.list"
    );
    absent.runtime.shutdown().await.unwrap();
    let server = Server::start(true).await;
    for method in METHODS {
        assert!(
            !server.runtime.unserved_methods.contains(method),
            "{method}"
        );
    }
    server.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn preview_mutations_emit_events_validate_urls_and_refuse_tabs_from_another_thread() {
    let mut s = Server::start(true).await;
    s.thread("one");
    s.thread("two");
    s.client
        .start("subscribePreviewEvents", json!({}), CallKind::Stream)
        .unwrap();
    let opened = success(
        s.call(
            "preview.open",
            json!({"threadId":"one","url":"localhost:3000","colorScheme":"dark","zoomFactor":1.25}),
        )
        .await,
    );
    let tab = opened["tabId"].as_str().unwrap();
    let first = s.chunk().await;
    assert_eq!(first["type"], "opened");
    assert_eq!(first["snapshot"]["tabId"], tab);
    let resized=success(s.call("preview.resize",json!({"threadId":"one","tabId":tab,"viewport":{"_tag":"freeform","width":390,"height":844}})).await);
    assert_eq!(resized["viewport"]["width"], 390);
    let update = s.chunk().await;
    assert_eq!(update["type"], "resized");
    assert_eq!(update["snapshot"]["viewport"]["width"], 390);
    let adjusted = success(
        s.call(
            "preview.adjust",
            json!({"threadId":"one","tabId":tab,"colorScheme":"light","zoomFactor":0.8}),
        )
        .await,
    );
    assert_eq!(adjusted["zoomFactor"], 0.8);
    success(s.call("preview.reportStatus",json!({"threadId":"one","tabId":tab,"navStatus":{"_tag":"Success","url":"http://localhost:3000/","title":"Ready"},"canGoBack":false,"canGoForward":false})).await);
    let listed = success(s.call("preview.list", json!({"threadId":"one"})).await);
    assert_eq!(listed["sessions"][0]["navStatus"]["title"], "Ready");
    for method in [
        "preview.resize",
        "preview.adjust",
        "preview.reportStatus",
        "preview.close",
    ] {
        let mut input = match method {
            "preview.resize" => json!({"viewport":{"_tag":"fill"}}),
            "preview.adjust" => json!({"colorScheme":"dark"}),
            "preview.reportStatus" => {
                json!({"navStatus":{"_tag":"Idle"},"canGoBack":false,"canGoForward":false})
            }
            _ => json!({}),
        };
        input["threadId"] = json!("two");
        input["tabId"] = json!(tab);
        failure(s.call(method, input).await, "PreviewSessionLookupError");
    }
    for url in [
        "file:///private.html",
        "javascript:alert(1)",
        "data:text/html,secret",
    ] {
        failure(
            s.call("preview.open", json!({"threadId":"one","url":url}))
                .await,
            "PreviewInvalidUrlError",
        );
    }
    failure(
        s.call(
            "preview.open",
            json!({"threadId":"foreign-environment","url":"localhost:3000"}),
        )
        .await,
        "EnvironmentAuthorizationError",
    );
    success(
        s.call("preview.close", json!({"threadId":"one","tabId":tab}))
            .await,
    );
    success(
        s.call("preview.close", json!({"threadId":"one","tabId":tab}))
            .await,
    );
    assert_eq!(
        success(s.call("preview.list", json!({"threadId":"one"})).await)["sessions"],
        json!([])
    );
    s.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn preview_profiles_clear_real_storage_and_map_failures_without_raw_paths() {
    let mut s = Server::start(true).await;
    success(s.call("preview.reportProfiles",json!({"profiles":[{"id":"work","name":"Work","kind":"persistent"}],"defaultProfileId":"work","recordingFrameRate":30})).await);
    let saved: Value = serde_json::from_slice(
        &std::fs::read(
            s.root
                .path()
                .join("userdata/server-browser/reported-profiles.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(saved["defaultProfileId"], "work");
    let profiles = s.root.path().join("userdata/server-browser/profiles");
    let work = profiles.join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("saved"), "cookie").unwrap();
    success(
        s.call("preview.clearProfile", json!({"profileId":"work"}))
            .await,
    );
    assert!(!work.exists());
    std::fs::write(profiles.join("broken"), "private diagnostic").unwrap();
    let error = failure(
        s.call("preview.clearProfile", json!({"profileId":"broken"}))
            .await,
        "PreviewClearProfileError",
    );
    assert!(!error.to_string().contains(s.root.path().to_str().unwrap()));
    failure(
        s.call(
            "preview.stopLocalServer",
            json!({"pid":std::process::id(),"port":1}),
        )
        .await,
        "PreviewStopLocalServerError",
    );
    s.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn every_preview_method_refuses_a_session_missing_its_scope_and_frames_require_read() {
    let mut s = Server::start(true).await;
    s.thread("one");
    s.scopes(&["access:read"]).await;
    let cases = [
        ("preview.open", json!({"threadId":"one"})),
        (
            "preview.resize",
            json!({"threadId":"one","tabId":"tab","viewport":{"_tag":"fill"}}),
        ),
        (
            "preview.adjust",
            json!({"threadId":"one","tabId":"tab","colorScheme":"dark"}),
        ),
        ("preview.close", json!({"threadId":"one","tabId":"tab"})),
        ("preview.list", json!({"threadId":"one"})),
        ("preview.clearProfile", json!({"profileId":"work"})),
        (
            "preview.reportProfiles",
            json!({"profiles":[],"defaultProfileId":"default"}),
        ),
        (
            "preview.reportStatus",
            json!({"threadId":"one","tabId":"tab","navStatus":{"_tag":"Idle"},"canGoBack":false,"canGoForward":false}),
        ),
        ("preview.stopLocalServer", json!({"pid":1,"port":1})),
        ("subscribePreviewEvents", json!({})),
        ("subscribeDiscoveredLocalServers", json!({})),
    ];
    for (method, input) in cases {
        s.client
            .start(
                method,
                input,
                if method.starts_with("subscribe") {
                    CallKind::Stream
                } else {
                    CallKind::Unary
                },
            )
            .unwrap();
        match s.event().await {
            CallEvent::Exit { exit, .. } => {
                failure(exit, "EnvironmentAuthorizationError");
            }
            other => panic!("unauthorized stream yielded {other:?}"),
        }
    }
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = format!(
        "ws://{}/api/preview-stream/ws?threadId=one&tabId=tab",
        s.runtime.address
    )
    .into_client_request()
    .unwrap();
    request
        .headers_mut()
        .insert("cookie", s.cookie.parse().unwrap());
    let tokio_tungstenite::tungstenite::Error::Http(response) =
        tokio_tungstenite::connect_async(request).await.unwrap_err()
    else {
        panic!("expected HTTP scope refusal")
    };
    assert_eq!(response.status(), 403);
    s.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn discovered_servers_stream_starts_and_updates_from_a_real_local_http_listener() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let (received, mut probes) = tokio::sync::mpsc::unbounded_channel();
    let web = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 8192];
            let _ = socket.read(&mut request).await.unwrap();
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await.unwrap();
            let _ = received.send(());
        }
    });
    let mut s = Server::start(true).await;
    s.client
        .start(
            "subscribeDiscoveredLocalServers",
            json!({"configuredUrls":[url]}),
            CallKind::Stream,
        )
        .unwrap();
    let initial = s.chunk().await;
    assert_eq!(initial["servers"][0]["url"], url);
    probes.recv().await.unwrap();
    s.socket.close(None).await.unwrap();
    s.runtime.shutdown().await.unwrap();
    web.abort();
    assert!(web.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn viewer_refuses_a_tab_from_another_thread_and_stops_with_its_server() {
    let mut s = Server::start(true).await;
    s.thread("one");
    s.thread("two");
    let release =
        t3_preview::installer::pinned_release(std::env::consts::OS, std::env::consts::ARCH)
            .unwrap();
    let executable = s
        .root
        .path()
        .join("tools/chrome-headless-shell")
        .join(&release.platform)
        .join(&release.version)
        .join(if cfg!(windows) {
            "chrome-headless-shell.exe"
        } else {
            "chrome-headless-shell"
        });
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    std::fs::write(&executable, "must never execute").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let opened = success(
        s.call("preview.open", json!({"threadId":"one","runtime":"server"}))
            .await,
    );
    let tab = opened["tabId"].as_str().unwrap();
    let mut foreign = s.viewer("two", tab).await;
    assert!(
        matches!(foreign.next().await.unwrap().unwrap(),Message::Close(Some(frame)) if u16::from(frame.code)==4404)
    );
    failure(s.call("preview.reportStatus",json!({"threadId":"one","tabId":tab,"navStatus":{"_tag":"Idle"},"canGoBack":false,"canGoForward":false})).await,"PreviewControlRequiredError");
    let admin_cookie = s.cookie.clone();
    s.scopes(&["orchestration:read"]).await;
    let mut viewer = s.viewer("one", tab).await;
    match viewer.next().await.unwrap().unwrap() {
        Message::Text(text) => {
            let state: Value = serde_json::from_str(&text).unwrap();
            assert!(matches!(
                state["type"].as_str(),
                Some("control" | "viewport")
            ));
            assert_eq!(state["canOperate"], false);
        }
        other => panic!("expected viewer state: {other:?}"),
    }
    loop {
        match viewer.next().await.unwrap().unwrap() {
            Message::Binary(bytes) => {
                assert_eq!(bytes.as_ref(), [255, 216, 255, 217]);
                viewer
                    .send(Message::Binary(br#"{"type":"ack"}"#.to_vec().into()))
                    .await
                    .unwrap();
                break;
            }
            Message::Text(_) => {}
            other => panic!("expected browser frame: {other:?}"),
        }
    }
    viewer
        .send(Message::Text(
            json!({"type":"takeControl"}).to_string().into(),
        ))
        .await
        .unwrap();
    viewer
        .send(Message::Text(
            json!({"type":"viewport","setting":{"_tag":"freeform","width":390,"height":844}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    barrier(&mut viewer).await;
    let listed = success(s.call("preview.list", json!({"threadId":"one"})).await);
    assert_eq!(listed["sessions"][0]["viewport"], json!({"_tag":"fill"}));
    let mut controller = s.viewer_cookie("one", tab, &admin_cookie).await;
    controller
        .send(Message::Text(
            json!({"type":"viewport","setting":{"_tag":"freeform","width":390,"height":844}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    barrier(&mut controller).await;
    assert_eq!(
        success(s.call("preview.list", json!({"threadId":"one"})).await)["sessions"][0]["viewport"]
            ["width"],
        390
    );
    controller
        .send(Message::Text(
            json!({"type":"viewport","setting":{"_tag":"freeform","width":100000,"height":844}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    barrier(&mut controller).await;
    assert_eq!(
        success(s.call("preview.list", json!({"threadId":"one"})).await)["sessions"][0]["viewport"]
            ["width"],
        390
    );
    controller.close(None).await.unwrap();
    s.runtime.cancel();
    while let Some(frame) = viewer.next().await {
        match frame.unwrap() {
            Message::Close(_) => break,
            Message::Ping(bytes) => viewer.send(Message::Pong(bytes)).await.unwrap(),
            _ => {}
        }
    }
    s.runtime.wait().await.unwrap();
}

async fn barrier(socket: &mut Socket) {
    socket
        .send(Message::Ping(b"preview-barrier".to_vec().into()))
        .await
        .unwrap();
    loop {
        match socket.next().await.unwrap().unwrap() {
            Message::Pong(data) if data.as_ref() == b"preview-barrier" => return,
            Message::Binary(bytes) => {
                assert_eq!(bytes.as_ref(), [255, 216, 255, 217]);
                socket
                    .send(Message::Binary(br#"{"type":"ack"}"#.to_vec().into()))
                    .await
                    .unwrap();
            }
            Message::Text(_) => {}
            other => panic!("unexpected viewer reply {other:?}"),
        }
    }
}

#[cfg(feature = "terminal")]
#[tokio::test]
async fn discovery_updates_when_a_real_terminal_child_opens_a_listener() {
    let mut s = Server::start_with(true, |root, environment| {
        let node = node();
        environment.insert(
            "T3_PREVIEW_NODE".into(),
            node.to_string_lossy().into_owned(),
        );
        environment.insert(
            "PATH".into(),
            root.join("empty-bin").to_string_lossy().into_owned(),
        );
        environment.insert("SHELL".into(), node.to_string_lossy().into_owned());
        environment.insert("ComSpec".into(), node.to_string_lossy().into_owned());
        environment.insert(
            "SystemRoot".into(),
            root.join("no-system-shell-profiles")
                .to_string_lossy()
                .into_owned(),
        );
    })
    .await;
    s.thread("owned");
    s.client
        .start(
            "subscribeDiscoveredLocalServers",
            json!({}),
            CallKind::Stream,
        )
        .unwrap();
    assert_eq!(s.chunk().await["servers"], json!([]));
    let cwd = s.root.path().join("worktrees/preview");
    std::fs::create_dir_all(&cwd).unwrap();
    let script = cwd.join("preview-http.cjs");
    std::fs::write(&script,"const http=require('node:http');const server=http.createServer((req,res)=>{res.setHeader('Content-Type','text/html');res.end('ok')});server.on('error',error=>{console.log('PREVIEW_ERROR');process.exit(1)});server.listen(0,'127.0.0.1',()=>console.log('PREVIEW_READY '+server.address().port));").unwrap();
    success(
        s.call(
            "terminal.open",
            json!({"threadId":"owned","terminalId":"preview-http","cwd":cwd,"cols":80,"rows":24}),
        )
        .await,
    );
    s.client
        .start(
            "terminal.attach",
            json!({"threadId":"owned","terminalId":"preview-http"}),
            CallKind::Stream,
        )
        .unwrap();
    let command = format!(
        "require('./preview-http.cjs'){}",
        if cfg!(windows) { "\r\n" } else { "\n" }
    );
    success(
        s.call(
            "terminal.write",
            json!({"threadId":"owned","terminalId":"preview-http","data":command}),
        )
        .await,
    );
    let mut output = String::new();
    let mut port = None;
    let mut discovered = None;
    while port.is_none() || discovered.is_none() {
        let value = s.chunk().await;
        if let Some(data) = value["data"].as_str() {
            output.push_str(data);
            assert!(!output.contains("PREVIEW_ERROR"), "{output}");
            assert!(!output.contains("Uncaught"), "{output}");
            port = output.split("PREVIEW_READY ").skip(1).find_map(|tail| {
                tail.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse::<u16>()
                    .ok()
            });
        }
        if let Some(servers) = value["servers"].as_array() {
            discovered = servers
                .iter()
                .find(|server| server["terminal"]["threadId"] == "owned")
                .cloned();
        }
    }
    let server = discovered.unwrap();
    assert_eq!(server["port"], port.unwrap());
    assert_eq!(server["terminal"]["terminalId"], "preview-http");
    assert!(server["pid"].as_u64().unwrap() > 0);
    s.socket.close(None).await.unwrap();
    s.runtime.shutdown().await.unwrap();
}
