#![allow(dead_code)]
use futures::{SinkExt, StreamExt, future::BoxFuture};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use t3_contracts::{
    assets::{AssetResource, NativeAppIconReference},
    terminal::{TerminalAttachInput, TerminalOpenInput},
};
use t3_rpc::{CallEvent, CallKind, Client, Exit, Frame};
use t3_server::{
    adapters::{
        config::{ConfigInputs, LocalServices, ServerFlags},
        terminal::*,
        workspace::*,
    },
    handlers::workspace_terminal::{self, TerminalRequest, TerminalTransport},
    registry::HandlerRegistry,
    services::{Domains, ServiceRegistry},
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

pub type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
pub struct Authority {
    pub root: PathBuf,
    pub allowed: Arc<AtomicBool>,
}
impl AssetAuthority for Authority {
    fn resolve<'a>(
        &'a self,
        _resource: &'a AssetResource,
    ) -> BoxFuture<'a, Result<Option<AssetWorkspaceContext>>> {
        Box::pin(async move {
            Ok(self
                .allowed
                .load(Ordering::SeqCst)
                .then(|| AssetWorkspaceContext {
                    workspace_root: Some(self.root.clone()),
                    ..Default::default()
                }))
        })
    }
}
struct NoExternal;
impl ExternalAssets for NoExternal {
    fn tool_image<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: u64,
    ) -> BoxFuture<'a, Result<Option<(String, Vec<u8>)>>> {
        Box::pin(async { Ok(None) })
    }
    fn native_icon<'a>(
        &'a self,
        _: &'a NativeAppIconReference,
    ) -> BoxFuture<'a, Result<Option<PathBuf>>> {
        Box::pin(async { Ok(None) })
    }
    fn validate_source_control<'a>(
        &'a self,
        _: &'a Path,
        _: &'a str,
        _: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Option<t3_server::adapters::workspace::SourceControlMedia>>> {
        Box::pin(async { Ok(None) })
    }
}
struct NoDocuments;
impl DocumentText for NoDocuments {
    fn ensure<'a>(&'a self, _: &'a Path) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}
struct TestLaunch;
impl TerminalLaunchPreparation for TestLaunch {
    fn open(&self, input: TerminalOpenInput) -> BoxFuture<'_, TerminalResult<TerminalOpenInput>> {
        Box::pin(async { Ok(input) })
    }
    fn attach(
        &self,
        input: TerminalAttachInput,
    ) -> BoxFuture<'_, TerminalResult<TerminalAttachInput>> {
        Box::pin(async { Ok(input) })
    }
}
pub fn terminal(base: &Path) -> Arc<TerminalAdapter> {
    let mut options = ManagerOptions::new(base.join("terminal-logs"));
    options.home = base.into();
    options.preferred_shell = Some(if cfg!(windows) { "cmd.exe" } else { "/bin/sh" }.into());
    Arc::new(TerminalAdapter::new(options, Arc::new(TestLaunch)).unwrap())
}
pub fn workspace(base: &Path, root: &Path, allowed: Arc<AtomicBool>) -> Arc<WorkspaceAdapter> {
    let internal = InternalPaths {
        state_dir: base.join("userdata"),
        secrets_dir: base.join("userdata/secrets"),
    };
    let indexes = Arc::new(SearchIndexes::new(internal.clone()));
    let root_copy = root.to_owned();
    let granted = allowed.clone();
    let known: Arc<dyn WorkspaceRoots> = Arc::new(move || {
        let root = root_copy.clone();
        let allowed = granted.clone();
        async move {
            KnownWorkspaces::new(if allowed.load(Ordering::SeqCst) {
                vec![root]
            } else {
                vec![]
            })
        }
    });
    let signer = AssetSigner::new(vec![42; 32]).unwrap();
    let attachments = AttachmentStore::new(
        base.join("attachments"),
        signer.clone(),
        Arc::new(NoDocuments),
    );
    Arc::new(WorkspaceAdapter {
        filesystem: WorkspaceFileSystem {
            home: base.into(),
            internal: internal.clone(),
            known: known.clone(),
            indexes: indexes.clone(),
        },
        entries: WorkspaceEntries {
            home: base.into(),
            indexes,
            known,
        },
        assets: AssetAccess {
            signer,
            attachments_dir: base.join("attachments"),
            home: base.into(),
            internal,
            favicons: Arc::new(FaviconResolver::new(base.into())),
            external: Arc::new(NoExternal),
        },
        attachments,
        authority: Arc::new(Authority {
            root: root.into(),
            allowed,
        }),
    })
}
pub fn inputs(base: &Path) -> ConfigInputs {
    ConfigInputs {
        flags: ServerFlags {
            base_dir: Some(base.to_string_lossy().into()),
            host: Some("127.0.0.1".into()),
            port: Some(0),
            ..Default::default()
        },
        env: Default::default(),
        home: base.into(),
        process_cwd: base.into(),
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
pub struct Fixture {
    pub temp: tempfile::TempDir,
    pub root: PathBuf,
    pub allowed: Arc<AtomicBool>,
    pub workspace: Arc<WorkspaceAdapter>,
    pub terminal: Arc<TerminalAdapter>,
    pub services: Arc<ServiceRegistry>,
    pub base: String,
    pub ws: String,
    pub cookie: String,
    pub http: reqwest::Client,
    task: tokio::task::JoinHandle<()>,
}
impl Fixture {
    pub async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("hello.txt"), "hello workspace\n").unwrap();
        let allowed = Arc::new(AtomicBool::new(true));
        let workspace = workspace(temp.path(), &root, allowed.clone());
        let terminal = terminal(temp.path());
        let mut domains = Domains::default();
        domains.insert(workspace.clone()).unwrap();
        domains.insert_runtime(terminal.clone()).unwrap();
        // Viewer-isolation tests use explicit socket IDs; this fixture only drives RPC policy.
        domains
            .insert(Arc::new(TerminalTransport {
                resolve: Arc::new(|context| {
                    Some(TerminalRequest {
                        viewer_id: context.session_id.0.clone(),
                        output_window: Arc::new(TerminalOutputWindow::default()),
                    })
                }),
            }))
            .unwrap();
        let mut handlers = HandlerRegistry::default();
        workspace_terminal::register_with(&mut handlers, &domains).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut config = t3_server::adapters::config::resolve(&inputs(temp.path())).unwrap();
        config.port = address.port();
        let local = LocalServices::open(config, address.port()).await.unwrap();
        let credential = local.auth.startup_pairing_credential().await.unwrap();
        let services = Arc::new(ServiceRegistry {
            handlers,
            domains,
            local,
            shutdown: Default::default(),
            connections: Default::default(),
            lifecycle: Default::default(),
        });
        let app = t3_server::http::router(services.clone())
            .merge(workspace_terminal::http_routes().with_state(services.clone()));
        let stop = services.shutdown.clone();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(stop.cancelled_owned())
            .await
            .unwrap();
        });
        let base = format!("http://{address}");
        let ws = format!("ws://{address}/ws?orchestrationProtocol=2");
        let http = reqwest::Client::new();
        let cookie = pair(&http, &base, &credential).await;
        Self {
            temp,
            root,
            allowed,
            workspace,
            terminal,
            services,
            base,
            ws,
            cookie,
            http,
            task,
        }
    }
    pub async fn socket(&self, cookie: &str) -> (Socket, Client) {
        let response = self
            .http
            .post(format!("{}/api/auth/websocket-ticket/rpc", self.base))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let ticket = response.json::<Value>().await.unwrap()["ticket"]
            .as_str()
            .unwrap()
            .to_owned();
        let (socket, _) =
            tokio_tungstenite::connect_async(format!("{}&wsTicket={ticket}", self.ws))
                .await
                .unwrap();
        (socket, Client::new(0, Default::default()))
    }
    pub async fn scoped(&self, scopes: &[&str]) -> String {
        let response = self
            .http
            .post(format!("{}/api/auth/pairing-token", self.base))
            .header("cookie", &self.cookie)
            .json(&json!({"scopes":scopes}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let grant = response.json::<Value>().await.unwrap();
        pair(
            &self.http,
            &self.base,
            grant["credential"].as_str().unwrap(),
        )
        .await
    }
    pub async fn close(self) {
        self.services.shutdown.cancel();
        self.task.await.unwrap();
        self.services.connections.close();
        self.services.connections.wait().await;
        self.terminal.manager.shutdown().await.unwrap();
        let services = Arc::try_unwrap(self.services)
            .ok()
            .expect("server handles released");
        drop(services.handlers);
        drop(services.domains);
        services.local.shutdown().await.unwrap();
    }
}
pub async fn pair(http: &reqwest::Client, base: &str, credential: &str) -> String {
    let response = http
        .post(format!("{base}/api/auth/browser-session"))
        .json(&json!({"credential":credential}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .into()
}
pub async fn flush(socket: &mut Socket, client: &mut Client) {
    while let Some(message) = client.pop_outgoing() {
        let Frame::Text(text) = t3_rpc::codec::encode(&message).unwrap() else {
            panic!("text frame")
        };
        socket.send(Message::Text(text.into())).await.unwrap();
    }
}
pub async fn event(socket: &mut Socket, client: &mut Client) -> CallEvent {
    loop {
        flush(socket, client).await;
        if let Some(event) = client.pop_event() {
            return event;
        }
        let frame = match socket.next().await.unwrap().unwrap() {
            Message::Text(text) => Frame::Text(text.to_string()),
            Message::Binary(bytes) => Frame::Binary(bytes.to_vec()),
            Message::Ping(bytes) => {
                socket.send(Message::Pong(bytes)).await.unwrap();
                continue;
            }
            other => panic!("unexpected frame {other:?}"),
        };
        for message in t3_rpc::codec::decode(&frame).unwrap() {
            client.receive(message, 0);
        }
    }
}
pub async fn call(
    socket: &mut Socket,
    client: &mut Client,
    method: &str,
    payload: Value,
    stream: bool,
) -> Exit {
    let id = client
        .start(
            method,
            payload,
            if stream {
                CallKind::Stream
            } else {
                CallKind::Unary
            },
        )
        .unwrap();
    match event(socket, client).await {
        CallEvent::Exit { id: actual, exit } => {
            assert_eq!(id, actual);
            exit
        }
        other => panic!("expected exit for {method}, got {other:?}"),
    }
}
pub fn success(exit: Exit) -> Value {
    match exit {
        Exit::Success { value } => value.unwrap_or(Value::Null),
        other => panic!("expected success, got {other:?}"),
    }
}
pub fn decode<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}
