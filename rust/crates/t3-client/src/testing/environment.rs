//! A connectable fake environment for tests outside this crate (cargo feature `testing`, off by
//! default; dependents enable it as a dev-dependency only).
//!
//! The smallest part of `tests/support/fake_server.rs` a client needs to really connect: HTTP
//! pairing and session, then one WebSocket served by `t3_rpc::AsyncServer` with the shell,
//! thread and config streams. Its sockets run on the spawner the test supplies, so they share
//! the test's executor (a headless UI's, or a `LocalPool`). Time is `FakePlatform`'s manual clock.
//!
//! ```ignore
//! let environment = FakeEnvironment::new("env", spawner.clone());
//! environment.set_shell(environment::shell(&[("thread-a", "Composer thread")]));
//! environment.reply("orchestration.dispatchCommand", Ok(json!({"sequence": 2})));
//! let client = ClientRuntime::new(environment.platform(), store, source, spawner.clone());
//! let pairing = client.pair(environment.pairing());
//! spawner(async move { pairing.await.expect("paired"); }.boxed_local());
//! client.open_thread(&key);
//! // … drive the UI, then:
//! assert_eq!(environment.calls("orchestration.dispatchCommand").len(), 1);
//! ```
//!
//! T3 Connect: `environment.sign_in()`, then build the client with
//! `ClientRuntime::with_relay(environment.platform(), store, source, spawner, FakeEnvironment::relay_config())`.
//! After `restore`, the relay lists this environment as linked to [`ACCOUNT`] and
//! `add_relay_environment` connects to it through the relay.
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::{Rc, Weak};

use futures::{
    FutureExt, SinkExt, StreamExt,
    channel::{mpsc, oneshot},
    future::LocalBoxFuture,
};
use serde_json::{Value, json};
use t3_rpc::{CallKind, Exit, Frame, Handler, HandlerOutput, Request};

use super::FakePlatform;
use crate::composition::Spawner;
use crate::connection::PairingInput;
use crate::platform::*;

pub const ORIGIN: &str = "https://server.test";
pub const NOW: &str = "2026-06-20T00:00:00.000Z";
/// The fake T3 Connect relay; its connect answer points at `ORIGIN`.
pub const RELAY: &str = "https://relay.test";
/// The account the host is signed in to after [`FakeEnvironment::sign_in`].
pub const ACCOUNT: &str = "account";

/// A terminal subscription whose next connection can be driven by the test.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TerminalStream {
    Attach,
    Observe,
    Metadata,
}

impl TerminalStream {
    fn from_tag(tag: &str) -> Option<Self> {
        use t3_contracts::rpc::*;
        match tag {
            WsTerminalAttachRpc::TAG => Some(Self::Attach),
            WsTerminalObserveRpc::TAG => Some(Self::Observe),
            WsSubscribeTerminalMetadataRpc::TAG => Some(Self::Metadata),
            _ => None,
        }
    }
}

enum ScriptedReply {
    Stream(Vec<Value>),
    Updates(mpsc::UnboundedReceiver<Value>),
    Ready(Result<Value, Value>),
    Deferred(oneshot::Receiver<Result<Value, Value>>),
}
type DeferredRead = oneshot::Receiver<Option<Vec<u8>>>;

#[derive(Default)]
struct State {
    shell: Option<Value>,
    threads: BTreeMap<String, Value>,
    config: Option<Value>,
    permissions: Option<Vec<String>>,
    methods: Vec<(&'static str, CallKind)>,
    script: BTreeMap<String, VecDeque<ScriptedReply>>,
    calls: Vec<(String, Value)>,
    deferred_reads: BTreeMap<StorageKey, DeferredRead>,
    terminal_streams: BTreeMap<TerminalStream, VecDeque<mpsc::UnboundedReceiver<Value>>>,
    sequence: u64,
    signed_in: bool,
    relay_refuses: bool,
    relay: Vec<String>,
}

struct Server {
    environment: String,
    origin: String,
    state: RefCell<State>,
    spawn: Spawner,
}

/// One environment server. Clone the platform into a `ClientRuntime`; keep this to script
/// replies and read what the client sent.
#[derive(Clone)]
pub struct FakeEnvironment {
    server: Rc<Server>,
    fake: FakePlatform,
}

impl FakeEnvironment {
    pub fn new(environment: &str, spawn: Spawner) -> Self {
        Self::at_origin(environment, ORIGIN, spawn)
    }

    /// A second independently pairable environment for a multi-machine fixture.
    pub fn at_origin(environment: &str, origin: &str, spawn: Spawner) -> Self {
        Self {
            server: Rc::new(Server {
                environment: environment.into(),
                origin: origin.into(),
                state: RefCell::default(),
                spawn,
            }),
            fake: FakePlatform::default(),
        }
    }

    /// The host the client runs on: storage, clock and host actions of a `FakePlatform`,
    /// network to this server.
    pub fn platform(&self) -> EnvironmentPlatform {
        EnvironmentPlatform {
            fake: self.fake.clone(),
            server: self.server.clone(),
            additional: BTreeMap::new(),
            ssh_provisioning: false,
        }
    }

    /// What the pairing screen hands `ClientRuntime::pair` for this server.
    pub fn pairing(&self) -> PairingInput {
        PairingInput {
            pairing_url: Some(format!("{}/pair#token=pairing-token", self.server.origin)),
            ..Default::default()
        }
    }

    /// The shell snapshot `subscribeShell` sends (default: `shell(&[("thread", "Thread")])`).
    pub fn set_shell(&self, snapshot: Value) {
        self.server.state.borrow_mut().shell = Some(snapshot);
    }

    /// The projection `subscribeThread` sends for `thread` (default: `projection(thread, "Thread", [], [])`).
    pub fn set_thread(&self, thread: &str, projection: Value) {
        self.server
            .state
            .borrow_mut()
            .threads
            .insert(thread.into(), projection);
    }

    /// Edits the `ServerConfig` the server answers (skills, providers, capabilities).
    pub fn edit_config(&self, edit: impl FnOnce(&mut Value)) {
        let mut state = self.server.state.borrow_mut();
        let config = state
            .config
            .get_or_insert_with(|| config(&self.server.environment));
        edit(config);
    }

    /// The session's scopes (default: `orchestration:read`, `orchestration:operate`).
    pub fn set_permissions(&self, scopes: &[&str]) {
        self.server.state.borrow_mut().permissions =
            Some(scopes.iter().map(|scope| (*scope).to_owned()).collect());
    }

    /// Registers another RPC method on sockets opened after this call.
    pub fn register(&self, tag: &'static str, kind: CallKind) {
        self.server.state.borrow_mut().methods.push((tag, kind));
    }

    /// The next reply to `method` (its wire tag, e.g. `orchestration.dispatchCommand`): Ok is a
    /// success value, Err a typed failure. Unscripted dispatches succeed; other unscripted
    /// unary methods fail.
    pub fn reply(&self, method: &str, answer: Result<Value, Value>) {
        self.server
            .state
            .borrow_mut()
            .script
            .entry(method.into())
            .or_default()
            .push_back(ScriptedReply::Ready(answer));
    }

    /// Emit these wire values on the next subscription, then keep the stream open.
    pub fn stream_snapshot(&self, method: &str, items: Vec<Value>) {
        self.server
            .state
            .borrow_mut()
            .script
            .entry(method.into())
            .or_default()
            .push_back(ScriptedReply::Stream(items));
    }

    /// Send wire values to the next subscription while a mounted test is running.
    pub fn stream_updates(&self, method: &str) -> mpsc::UnboundedSender<Value> {
        let (send, receive) = mpsc::unbounded();
        self.server
            .state
            .borrow_mut()
            .script
            .entry(method.into())
            .or_default()
            .push_back(ScriptedReply::Updates(receive));
        send
    }

    /// Holds the next ordinary storage read of this key until the test supplies a result.
    pub fn defer_read(&self, key: StorageKey) -> oneshot::Sender<Option<Vec<u8>>> {
        let (send, receive) = oneshot::channel();
        self.server
            .state
            .borrow_mut()
            .deferred_reads
            .insert(key, receive);
        send
    }

    /// Holds the next unary reply for this method until the test supplies an answer.
    pub fn defer_reply(&self, method: &str) -> oneshot::Sender<Result<Value, Value>> {
        let (send, receive) = oneshot::channel();
        self.server
            .state
            .borrow_mut()
            .script
            .entry(method.into())
            .or_default()
            .push_back(ScriptedReply::Deferred(receive));
        send
    }

    /// Queues a stream for the next subscription of this kind. Send snapshot, output
    /// and exit frames in wire format. Dropping the sender ends the subscription.
    /// Use `reply` to script the terminal's unary commands.
    pub fn terminal_stream(&self, stream: TerminalStream) -> mpsc::UnboundedSender<Value> {
        let (send, receive) = mpsc::unbounded();
        self.server
            .state
            .borrow_mut()
            .terminal_streams
            .entry(stream)
            .or_default()
            .push_back(receive);
        send
    }

    /// Signs the host in to T3 Connect as [`ACCOUNT`] (or out): the cloud identity and token it
    /// answers. The relay lists this environment as linked to that account.
    pub fn sign_in(&self, signed_in: bool) {
        self.server.state.borrow_mut().signed_in = signed_in;
    }

    /// The relay refuses to connect this device (503 `environment_endpoint_unavailable`).
    pub fn refuse_relay(&self, refuses: bool) {
        self.server.state.borrow_mut().relay_refuses = refuses;
    }

    /// Every request the relay answered, as "METHOD /path".
    pub fn relay_requests(&self) -> Vec<String> {
        self.server.state.borrow().relay.clone()
    }

    /// What `ClientRuntime::with_relay` takes to reach this relay.
    pub fn relay_config() -> crate::composition::RelayConfig {
        crate::composition::RelayConfig {
            relay_url: RELAY.into(),
            client_id: t3_contracts::relay::RelayPublicClientId::T3Web,
        }
    }

    /// Every request payload the client sent to `method`, in order.
    pub fn calls(&self, method: &str) -> Vec<Value> {
        self.server
            .state
            .borrow()
            .calls
            .iter()
            .filter(|(tag, _)| tag == method)
            .map(|(_, payload)| payload.clone())
            .collect()
    }
}

pub fn thread_row(id: &str, title: &str, project: &str) -> Value {
    json!({"id":id,"projectId":project,"title":title,"providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":id,"parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":NOW,"updatedAt":NOW,"archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null})
}

/// A shell with one project (`project`, rooted at `/workspace/project`) holding `threads`.
pub fn shell(threads: &[(&str, &str)]) -> Value {
    let threads = threads
        .iter()
        .map(|(id, title)| {
            let mut row = thread_row(id, title, "project");
            if let (Some(row), Some(extra)) = (
                row.as_object_mut(),
                json!({"latestRunId":null,"activeRunId":null,"status":"idle","pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0}).as_object(),
            ) {
                row.extend(extra.clone());
            }
            row
        })
        .collect::<Vec<_>>();
    json!({"schemaVersion":1,"snapshotSequence":1,"threads":threads,"archivedThreads":[],"projects":[{"id":"project","title":"Project","workspaceRoot":"/workspace/project","repositoryIdentity":null,"defaultModelSelection":null,"scripts":[],"createdAt":NOW,"updatedAt":NOW}]})
}

/// A thread projection holding `runtime_requests` and `turn_items` (none visible).
pub fn projection(id: &str, title: &str, runtime_requests: Value, turn_items: Value) -> Value {
    json!({"thread":thread_row(id, title, "project"),"runs":[],"attempts":[],"nodes":[],"subagents":[],"providerSessions":[],"providerThreads":[],"providerTurns":[],"runtimeRequests":runtime_requests,"messages":[],"plans":[],"turnItems":turn_items,"checkpointScopes":[],"checkpoints":[],"contextHandoffs":[],"contextTransfers":[],"visibleTurnItems":[],"updatedAt":NOW})
}

fn descriptor(environment: &str) -> Value {
    json!({"environmentId":environment,"label":"Fake host","platform":{"os":"linux","arch":"x64"},"serverVersion":"1.0.0","capabilities":{"terminal":true,"desktopIntegration":false},"orchestrationProtocolVersion":t3_contracts::environment::ORCHESTRATION_PROTOCOL_VERSION})
}

fn config(environment: &str) -> Value {
    let mut config: Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/ServerConfig.jsonl")
            .lines()
            .next()
            .unwrap_or_default(),
    )
    .unwrap_or_default();
    config["environment"]["environmentId"] = json!(environment);
    config["environment"]["label"] = json!("Fake host");
    config
}

fn relay_endpoint() -> Value {
    json!({"httpBaseUrl": ORIGIN, "wsBaseUrl": "wss://server.test", "providerKind": "cloudflare_tunnel"})
}

/// The relay's protocol surface the client uses: token exchange, list, status, connect.
fn relay_response(server: &Server, request: &HttpRequest, path: &str) -> HttpResponse {
    let environment = &server.environment;
    let mut state = server.state.borrow_mut();
    state.relay.push(format!("{} {path}", request.method));
    let connect = format!("/v1/environments/{environment}/connect");
    let status = format!("/v1/environments/{environment}/status");
    match (request.method.as_str(), path) {
        ("POST", "/v1/client/dpop-token") => {
            let scope = url::form_urlencoded::parse(&request.body)
                .find(|(key, _)| key == "scope")
                .map(|(_, value)| value.into_owned())
                .unwrap_or_default();
            response(
                200,
                json!({"access_token":"relay-access","issued_token_type":"urn:ietf:params:oauth:token-type:access_token","token_type":"DPoP","expires_in":3600,"scope": scope}),
            )
        }
        ("GET", "/v1/environments") => response(
            200,
            json!({"environments":[{"environmentId": environment, "label": "Fake host", "endpoint": relay_endpoint(), "linkedAt": NOW}]}),
        ),
        ("POST", path) if path == status => response(
            200,
            json!({"environmentId": environment, "endpoint": relay_endpoint(), "status": "online", "checkedAt": NOW}),
        ),
        ("POST", path) if path == connect && state.relay_refuses => response(
            503,
            json!({"_tag":"RelayEnvironmentEndpointUnavailableError","code":"environment_endpoint_unavailable","reason":"endpoint_request_failed","traceId":"relay-trace"}),
        ),
        ("POST", path) if path == connect => response(
            200,
            json!({"environmentId": environment, "endpoint": relay_endpoint(), "credential": "relay-credential", "expiresAt": "2036-01-01T00:00:00.000Z"}),
        ),
        _ => response(404, json!({})),
    }
}

/// A cloud session token for `account`, as the host's account store hands it out.
fn cloud_token(account: &str) -> String {
    use base64::Engine;
    let claims = json!({"sub": account, "exp": 4_102_444_800u64}).to_string();
    format!(
        "e30.{}.sig",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims)
    )
}

/// The host's DPoP public key and its RFC 7638 thumbprint.
fn dpop_key() -> (Value, String) {
    use base64::Engine;
    use sha2::Digest;
    let encode = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let jwk = json!({"crv":"P-256","kty":"EC","x":encode(&[1; 32]),"y":encode(&[2; 32])});
    let thumbprint = encode(&sha2::Sha256::digest(jwk.to_string()));
    (jwk, thumbprint)
}

fn response(status: u16, body: Value) -> HttpResponse {
    HttpResponse {
        status,
        headers: Default::default(),
        body: serde_json::to_vec(&body).unwrap_or_default(),
    }
}

#[derive(Clone)]
pub struct EnvironmentPlatform {
    /// Tests may script SSH host replies through the fake platform.
    pub ssh_provisioning: bool,
    pub fake: FakePlatform,
    server: Rc<Server>,
    additional: BTreeMap<String, Rc<Server>>,
}

impl EnvironmentPlatform {
    /// Route requests for an additional host to another independently pairable fixture.
    pub fn with_environment(mut self, environment: &FakeEnvironment) -> Self {
        if let Ok(url) = url::Url::parse(&environment.server.origin)
            && let Some(host) = url.host_str()
        {
            self.additional
                .insert(host.into(), environment.server.clone());
        }
        self
    }

    fn server_for(&self, url: &str) -> Rc<Server> {
        url::Url::parse(url)
            .ok()
            .and_then(|url| self.additional.get(url.host_str()?).cloned())
            .unwrap_or_else(|| self.server.clone())
    }
}

impl Storage for EnvironmentPlatform {
    fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
        if area == StorageArea::Ordinary
            && let Some(reply) = self.server.state.borrow_mut().deferred_reads.remove(&key)
        {
            return async move { reply.await.map_err(|_| PlatformError::Cancelled) }.boxed_local();
        }
        self.fake.get(area, key)
    }
    fn write(&self, area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()> {
        self.fake.write(area, writes)
    }
    fn list(&self, area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>> {
        self.fake.list(area, namespace)
    }
    fn transaction(&self, transaction: StorageTransaction) -> HostFuture<'_, bool> {
        self.fake.transaction(transaction)
    }
}

impl Platform for EnvironmentPlatform {
    fn can_provision_ssh(&self) -> bool {
        self.ssh_provisioning
    }
    fn now_epoch_ms(&self) -> u64 {
        self.fake.now_epoch_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.fake.monotonic_ms()
    }
    fn random_bytes(&self, length: usize) -> Result<Vec<u8>, PlatformError> {
        self.fake.random_bytes(length)
    }
    fn sleep(&self, milliseconds: u64) -> LocalBoxFuture<'_, ()> {
        self.fake.sleep(milliseconds)
    }
    fn yield_to_host(&self) -> LocalBoxFuture<'_, ()> {
        self.fake.yield_to_host()
    }
    fn events(&self) -> futures::stream::LocalBoxStream<'static, PlatformEvent> {
        self.fake.events()
    }
    fn set_route_policy(&self, policy: crate::navigation::RoutePolicy) {
        self.fake.set_route_policy(policy);
    }
    fn http(&self, request: HttpRequest) -> HostFuture<'_, HttpResponse> {
        let server = self.server_for(&request.url);
        async move {
            let url = url::Url::parse(&request.url)
                .map_err(|error| PlatformError::Network(error.to_string()))?;
            let path = url.path().to_owned();
            if url.host_str() == Some("relay.test") {
                return Ok(relay_response(&server, &request, &path));
            }
            let permissions = server.state.borrow().permissions.clone().unwrap_or_else(|| {
                vec!["orchestration:read".into(), "orchestration:operate".into()]
            });
            Ok(match (request.method.as_str(), path.as_str()) {
                ("GET", "/.well-known/t3/environment") => {
                    response(200, descriptor(&server.environment))
                }
                ("GET", "/api/auth/session") => response(
                    200,
                    json!({"authenticated":true,"auth":{"policy":"remote-reachable","bootstrapMethods":[],"sessionMethods":[],"sessionCookieName":"session"},"permissions":permissions}),
                ),
                ("POST", "/api/auth/websocket-ticket" | "/api/auth/websocket-ticket/rpc") => {
                    response(200, json!({"ticket":"ticket","expiresAt":"2036-01-01T00:00:00.000Z"}))
                }
                // A DPoP-bound exchange (through the relay) gets a DPoP token, as the server does.
                ("POST", "/oauth/token") => {
                    let bound = request
                        .headers
                        .keys()
                        .any(|name| name.eq_ignore_ascii_case("dpop"));
                    let token_type = if bound { "DPoP" } else { "Bearer" };
                    response(
                        200,
                        json!({"access_token":"paired-bearer","issued_token_type":"urn:ietf:params:oauth:token-type:access_token","token_type":token_type,"expires_in":3600,"scope":permissions.join(" ")}),
                    )
                }
                // Snapshots come over the socket; HTTP answers that it cannot serve them.
                _ => response(503, json!({})),
            })
        }
        .boxed_local()
    }
    fn websocket(&self, request: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>> {
        let server = self.server_for(&request.url);
        async move {
            let (to_server, server_incoming) = mpsc::unbounded::<Frame>();
            let (server_outgoing, from_server) = mpsc::unbounded::<Frame>();
            let handler = FakeHandler {
                server: Rc::downgrade(&server),
            };
            let methods = server.state.borrow().methods.clone();
            let serve = async move {
                let mut rpc = t3_rpc::AsyncServer::new(
                    server_outgoing.sink_map_err(|error| error.to_string()),
                    server_incoming.map(Ok::<Frame, String>),
                    handler,
                );
                use t3_contracts::rpc::*;
                for (tag, kind) in [
                    (WsServerGetConfigRpc::TAG, CallKind::Unary),
                    (WsServerProbeRpc::TAG, CallKind::Unary),
                    (WsServerUpdateSettingsRpc::TAG, CallKind::Unary),
                    (WsSubscribeServerConfigRpc::TAG, CallKind::Stream),
                    (WsOrchestrationV2SubscribeShellRpc::TAG, CallKind::Stream),
                    (WsOrchestrationV2SubscribeThreadRpc::TAG, CallKind::Stream),
                    (WsOrchestrationV2DispatchCommandRpc::TAG, CallKind::Unary),
                    (
                        WsOrchestrationV2GetThreadProjectionRpc::TAG,
                        CallKind::Unary,
                    ),
                    (WsProjectsSearchEntriesRpc::TAG, CallKind::Unary),
                    (WsProjectsSearchContentsRpc::TAG, CallKind::Unary),
                    (WsPullRequestsListRpc::TAG, CallKind::Unary),
                    (WsPullRequestsDetailRpc::TAG, CallKind::Unary),
                    (WsPullRequestsLinkedThreadsRpc::TAG, CallKind::Unary),
                    (WsTerminalOpenRpc::TAG, CallKind::Unary),
                    (WsTerminalWriteRpc::TAG, CallKind::Unary),
                    (WsTerminalResizeRpc::TAG, CallKind::Unary),
                    (WsTerminalClearRpc::TAG, CallKind::Unary),
                    (WsTerminalCloseRpc::TAG, CallKind::Unary),
                    (WsTerminalAttachRpc::TAG, CallKind::Stream),
                    (WsTerminalObserveRpc::TAG, CallKind::Stream),
                    (WsSubscribeTerminalMetadataRpc::TAG, CallKind::Stream),
                ]
                .into_iter()
                .chain(methods)
                {
                    rpc.state.register(tag, kind);
                }
                let _ = rpc.run().await;
            };
            (server.spawn)(serve.boxed_local());
            Ok(Box::new(Socket {
                to_server: Some(to_server),
                from_server,
            }) as Box<dyn WebSocket>)
        }
        .boxed_local()
    }
    fn host(&self, action: HostAction) -> HostFuture<'_, HostReply> {
        match action {
            HostAction::ClientPresentation => {
                async { Ok(HostReply::Document(json!({"label":"Test client"}))) }.boxed_local()
            }
            HostAction::CloudIdentity | HostAction::CloudToken => {
                let token = matches!(action, HostAction::CloudToken);
                let signed_in = self.server.state.borrow().signed_in;
                let answer = signed_in.then(|| {
                    if token {
                        cloud_token(ACCOUNT)
                    } else {
                        ACCOUNT.to_owned()
                    }
                });
                async move { Ok(HostReply::Text(answer)) }.boxed_local()
            }
            HostAction::RelayDeviceId => {
                async { Ok(HostReply::Text(Some("device-1".into()))) }.boxed_local()
            }
            HostAction::DpopPublicJwk => {
                async { Ok(HostReply::Document(dpop_key().0)) }.boxed_local()
            }
            HostAction::DpopThumbprint => {
                async { Ok(HostReply::Text(Some(dpop_key().1))) }.boxed_local()
            }
            HostAction::DpopSign { .. } => {
                async { Ok(HostReply::Bytes(vec![3; 64])) }.boxed_local()
            }
            action => self.fake.host(action),
        }
    }
}

struct Socket {
    to_server: Option<mpsc::UnboundedSender<Frame>>,
    from_server: mpsc::UnboundedReceiver<Frame>,
}

impl WebSocket for Socket {
    fn send(&mut self, frame: Frame) -> HostFuture<'_, ()> {
        async move {
            self.to_server
                .as_ref()
                .ok_or(PlatformError::Closed)?
                .unbounded_send(frame)
                .map_err(|_| PlatformError::Closed)
        }
        .boxed_local()
    }
    fn receive(&mut self) -> HostFuture<'_, Option<Frame>> {
        async move { Ok(self.from_server.next().await) }.boxed_local()
    }
    fn close(&mut self) -> HostFuture<'_, ()> {
        async move {
            self.to_server = None;
            Ok(())
        }
        .boxed_local()
    }
}

struct FakeHandler {
    server: Weak<Server>,
}

/// Answers `items`, then stays open.
fn live(items: Vec<Value>) -> HandlerOutput {
    HandlerOutput::Stream(
        futures::stream::once(async move { Ok(items) })
            .chain(futures::stream::pending())
            .boxed_local(),
    )
}

fn unary(answer: Result<Value, Value>) -> HandlerOutput {
    HandlerOutput::Unary(
        async move {
            match answer {
                Ok(value) => Exit::success(value),
                Err(error) => Exit::fail(error),
            }
        }
        .boxed_local(),
    )
}

fn terminal_snapshot(input: &Value) -> Value {
    json!({
        "threadId": input["threadId"],
        "terminalId": input["terminalId"],
        "cwd": input["cwd"].as_str().unwrap_or("/workspace/project"),
        "worktreePath": input["worktreePath"],
        "status": "running",
        "pid": 1,
        "history": "",
        "exitCode": null,
        "exitSignal": null,
        "label": "",
        "updatedAt": NOW
    })
}

impl Handler for FakeHandler {
    fn handle(&mut self, request: Request) -> HandlerOutput {
        use t3_contracts::rpc::*;
        let Some(server) = self.server.upgrade() else {
            return HandlerOutput::Unary(async { Exit::die("server gone") }.boxed_local());
        };
        let tag = request.tag.clone();
        let scripted = {
            let mut state = server.state.borrow_mut();
            state.calls.push((tag.clone(), request.payload.clone()));
            state.script.get_mut(&tag).and_then(VecDeque::pop_front)
        };
        match scripted {
            Some(ScriptedReply::Stream(items)) => return live(items),
            Some(ScriptedReply::Updates(frames)) => {
                return HandlerOutput::Stream(frames.map(|frame| Ok(vec![frame])).boxed_local());
            }
            Some(ScriptedReply::Ready(answer)) => return unary(answer),
            Some(ScriptedReply::Deferred(reply)) => {
                return HandlerOutput::Unary(
                    async move {
                        match reply.await {
                            Ok(Ok(value)) => Exit::success(value),
                            Ok(Err(error)) => Exit::fail(error),
                            Err(_) => Exit::die("fake deferred reply cancelled"),
                        }
                    }
                    .boxed_local(),
                );
            }
            None => {}
        }
        if let Some(stream) = TerminalStream::from_tag(&tag) {
            let scripted = server
                .state
                .borrow_mut()
                .terminal_streams
                .get_mut(&stream)
                .and_then(VecDeque::pop_front);
            return if let Some(frames) = scripted {
                HandlerOutput::Stream(frames.map(|frame| Ok(vec![frame])).boxed_local())
            } else {
                let initial = match stream {
                    TerminalStream::Attach => {
                        json!({"type": "snapshot", "snapshot": terminal_snapshot(&request.payload)})
                    }
                    TerminalStream::Observe => return live(vec![]),
                    TerminalStream::Metadata => json!({"type": "snapshot", "terminals": []}),
                };
                live(vec![initial])
            };
        }
        if tag == WsTerminalOpenRpc::TAG {
            return unary(Ok(terminal_snapshot(&request.payload)));
        }
        if matches!(
            tag.as_str(),
            WsTerminalWriteRpc::TAG
                | WsTerminalResizeRpc::TAG
                | WsTerminalClearRpc::TAG
                | WsTerminalCloseRpc::TAG
        ) {
            return unary(Ok(Value::Null));
        }
        let environment = server.environment.clone();
        let config = || {
            server
                .state
                .borrow()
                .config
                .clone()
                .unwrap_or_else(|| config(&environment))
        };
        if tag == WsServerGetConfigRpc::TAG {
            unary(Ok(config()))
        } else if tag == WsServerProbeRpc::TAG {
            unary(Ok(json!({})))
        } else if tag == WsSubscribeServerConfigRpc::TAG {
            live(vec![
                json!({"version":1,"type":"snapshot","config":config()}),
            ])
        } else if tag == WsOrchestrationV2SubscribeShellRpc::TAG {
            let shell = server
                .state
                .borrow()
                .shell
                .clone()
                .unwrap_or_else(|| shell(&[("thread", "Thread")]));
            live(vec![
                json!({"kind":"snapshot","snapshot":shell}),
                json!({"kind":"synchronized"}),
            ])
        } else if tag == WsOrchestrationV2SubscribeThreadRpc::TAG {
            let thread = request.payload["threadId"].as_str().unwrap_or_default();
            let projection = server
                .state
                .borrow()
                .threads
                .get(thread)
                .cloned()
                .unwrap_or_else(|| projection(thread, "Thread", json!([]), json!([])));
            live(vec![
                json!({"kind":"snapshot","snapshotSequence":1,"projection":projection}),
                json!({"kind":"synchronized"}),
            ])
        } else if tag == WsOrchestrationV2DispatchCommandRpc::TAG {
            let sequence = {
                let mut state = server.state.borrow_mut();
                state.sequence += 1;
                state.sequence + 1
            };
            unary(Ok(json!({ "sequence": sequence })))
        } else {
            HandlerOutput::Unary(
                async move { Exit::die(format!("unscripted {tag}")) }.boxed_local(),
            )
        }
    }
}
