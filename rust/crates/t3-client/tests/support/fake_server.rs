//! An in-process environment server for connection-runtime tests.
//!
//! HTTP is routed by path; each WebSocket is a pair of channels served by `t3_rpc::AsyncServer`
//! on the test's own executor. Time is `FakePlatform`'s manual clock. The server records what a
//! client subscribed to and which subscriptions are still open, so tests assert on what the
//! client leaves running, not on its internals.
#![allow(dead_code, clippy::unwrap_used)]

use futures::{
    FutureExt, SinkExt, StreamExt,
    channel::mpsc,
    executor::{LocalPool, LocalSpawner},
    future::LocalBoxFuture,
    stream::LocalBoxStream,
    task::LocalSpawnExt,
};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::{Rc, Weak},
};
use t3_client::{
    platform::*,
    testing::FakePlatform,
    types::{EnvironmentId, ThreadId},
};
use t3_contracts::rpc::RpcMethod;
use t3_rpc::{Exit, Frame, Handler, HandlerOutput, Request};

pub const ORIGIN: &str = "https://server.test";
/// The fake T3 Connect relay; its connect answer points at `ORIGIN`.
pub const RELAY: &str = "https://relay.test";
pub const ACCOUNT: &str = "account";

/// A cloud session token for `account`, as the host's account store hands it out.
pub fn cloud_token(account: &str) -> String {
    use base64::Engine;
    format!(
        "e30.{}.sig",
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&json!({"sub": account, "exp": 4_102_444_800u64})).unwrap())
    )
}

/// The host's DPoP public key and its RFC 7638 thumbprint.
pub fn dpop_key() -> (Value, String) {
    use base64::Engine;
    use sha2::Digest;
    let encode = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let jwk = json!({"crv":"P-256","kty":"EC","x":encode(&[1; 32]),"y":encode(&[2; 32])});
    let thumbprint = encode(&sha2::Sha256::digest(serde_json::to_vec(&jwk).unwrap()));
    (jwk, thumbprint)
}

pub fn ssh_target(alias: &str) -> Value {
    json!({"alias": alias, "hostname": format!("{alias}.example"), "username": "dev", "port": null})
}

fn ssh_bootstrap(alias: &str) -> Value {
    json!({"target": ssh_target(alias), "httpBaseUrl": ORIGIN, "wsBaseUrl": "wss://server.test", "pairingToken": null, "remoteServerKind": "managed"})
}

fn relay_endpoint() -> Value {
    json!({"httpBaseUrl": ORIGIN, "wsBaseUrl": "wss://server.test", "providerKind": "cloudflare_tunnel"})
}

fn form_value(body: &[u8], name: &str) -> Option<String> {
    url::form_urlencoded::parse(body)
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

/// The relay's protocol surface the client uses: token exchange, list, status, connect.
fn relay_response(server: &FakeServer, request: &HttpRequest, path: &str) -> HttpResponse {
    let environment = server.environment.clone();
    server
        .state
        .borrow_mut()
        .relay
        .push(format!("{} {path}", request.method));
    let connect = format!("/v1/environments/{environment}/connect");
    let status = format!("/v1/environments/{environment}/status");
    match (request.method.as_str(), path) {
        ("POST", "/v1/client/dpop-token") => response(
            200,
            json!({"access_token":"relay-access","issued_token_type":"urn:ietf:params:oauth:token-type:access_token","token_type":"DPoP","expires_in":3600,"scope": form_value(&request.body, "scope").unwrap_or_default()}),
        ),
        ("GET", "/v1/environments") => response(
            200,
            json!({"environments":[{"environmentId": environment, "label": "Fake host", "endpoint": relay_endpoint(), "linkedAt": "2026-06-20T00:00:00.000Z"}]}),
        ),
        ("POST", path) if path == status => response(
            200,
            json!({"environmentId": environment, "endpoint": relay_endpoint(), "status": "online", "checkedAt": NOW}),
        ),
        ("POST", path) if path == connect => {
            if server.state.borrow().relay_refuses {
                return response(
                    503,
                    json!({"_tag":"RelayEnvironmentEndpointUnavailableError","code":"environment_endpoint_unavailable","reason":"endpoint_request_failed","traceId":"relay-trace"}),
                );
            }
            response(
                200,
                json!({"environmentId": environment, "endpoint": relay_endpoint(), "credential": "relay-credential", "expiresAt": "2036-01-01T00:00:00.000Z"}),
            )
        }
        _ => response(404, json!({})),
    }
}
pub const NOW: &str = "2026-06-20T00:00:00.000Z";

#[derive(Default)]
pub struct ServerState {
    pub extra_rpc: Option<Rc<dyn Fn(&Request) -> Option<HandlerOutput>>>,
    pub sockets_opened: usize,
    pub sockets_open: usize,
    /// Every `subscribeThread` request, in order.
    pub thread_subscribes: Vec<String>,
    /// `subscribeThread` streams the server is still serving, by thread.
    pub open_threads: BTreeMap<String, usize>,
    pub shell_subscribes: usize,
    /// `getTurnDiff` requests answered.
    pub turn_diffs: usize,
    pub http: Vec<String>,
    /// Failure injection: HTTP and sockets fail as from an unreachable host.
    pub unreachable: bool,
    /// Failure injection: HTTP works but every WebSocket upgrade fails (a proxy without upgrade).
    pub refuse_sockets: bool,
    /// Failure injection: dispatches are received and never answered.
    pub hold_dispatches: bool,
    /// Failure injection: the host cannot produce random bytes.
    pub fail_random: bool,
    /// The server speaks an older orchestration protocol and can update itself.
    pub outdated: bool,
    /// The client's host has no presentation to give when pairing.
    pub anonymous: bool,
    /// The host is signed in to the cloud account `account` (identity, token, device id).
    pub signed_in: bool,
    /// Every request the fake relay at `RELAY` answered, as "METHOD /path".
    pub relay: Vec<String>,
    /// Failure injection: the relay refuses to connect this device to an environment.
    pub relay_refuses: bool,
    /// Every SSH host action, as "provision alias", "prepare alias", "disconnect alias".
    pub ssh: Vec<String>,
    /// `subscribeCommandOutput` requests answered, and how many are still being served.
    pub command_output_subscribes: usize,
    pub command_outputs_open: usize,
    /// The command keeps running: only the first frame is sent.
    pub command_running: bool,
    /// Failure injection: secure records in this namespace fail to decrypt (disk damage, or a
    /// row from another install).
    pub undecryptable: Option<String>,
    pub rpc_script: BTreeMap<String, std::collections::VecDeque<Result<Value, Value>>>,
    pub rpc_calls: Vec<(String, Value)>,
    pub thread_snapshots: BTreeMap<String, Value>,
    /// Ends every socket the server holds, as a dropped connection does.
    hangups: Vec<mpsc::UnboundedSender<()>>,
}

pub struct FakeServer {
    pub environment: String,
    pub state: RefCell<ServerState>,
    spawner: LocalSpawner,
}

impl FakeServer {
    pub fn open_thread_streams(&self) -> usize {
        self.state.borrow().open_threads.values().sum()
    }
    pub fn thread_subscribes(&self, thread: &str) -> usize {
        self.state
            .borrow()
            .thread_subscribes
            .iter()
            .filter(|id| *id == thread)
            .count()
    }
    pub fn open_threads_of(&self, thread: &str) -> usize {
        self.state
            .borrow()
            .open_threads
            .get(thread)
            .copied()
            .unwrap_or(0)
    }
    /// Closes every connection from the server side.
    pub fn hang_up(&self) {
        for hangup in self.state.borrow_mut().hangups.drain(..) {
            let _ = hangup.unbounded_send(());
        }
    }
}

#[derive(Clone)]
pub struct TestPlatform {
    pub fake: FakePlatform,
    pub server: Rc<FakeServer>,
}

pub struct Harness {
    pub pool: LocalPool,
    pub platform: TestPlatform,
}

impl Harness {
    pub fn new(environment: &str) -> Self {
        let pool = LocalPool::new();
        let server = Rc::new(FakeServer {
            environment: environment.into(),
            state: RefCell::default(),
            spawner: pool.spawner(),
        });
        Self {
            pool,
            platform: TestPlatform {
                fake: FakePlatform::default(),
                server,
            },
        }
    }
    pub fn server(&self) -> &FakeServer {
        &self.platform.server
    }
    /// A spawner for code that keeps futures running on the harness executor.
    pub fn spawner(&self) -> Rc<dyn Fn(LocalBoxFuture<'static, ()>)> {
        let spawner = self.pool.spawner();
        Rc::new(move |future| spawner.spawn_local(future).unwrap())
    }
    pub fn spawn(&self, future: impl std::future::Future<Output = ()> + 'static) {
        self.pool.spawner().spawn_local(future).unwrap();
    }
    /// Runs every task until all of them wait on something.
    pub fn settle(&mut self) {
        self.pool.run_until_stalled();
    }
    /// Runs `future` to completion, settling other tasks around it.
    pub fn run<T: 'static>(&mut self, future: impl std::future::Future<Output = T> + 'static) -> T {
        let slot = Rc::new(RefCell::new(None));
        let output = slot.clone();
        self.spawn(async move {
            *output.borrow_mut() = Some(future.await);
        });
        self.settle();
        slot.borrow_mut().take().expect("future did not finish")
    }
    /// Advances the manual clock in steps, settling after each.
    pub fn advance(&mut self, milliseconds: u64) {
        for _ in 0..milliseconds / 250 {
            self.platform.fake.advance(250);
            self.settle();
        }
    }
}

fn response(status: u16, body: Value) -> HttpResponse {
    HttpResponse {
        status,
        headers: Default::default(),
        body: serde_json::to_vec(&body).unwrap(),
    }
}

pub fn descriptor(environment: &str) -> Value {
    json!({"environmentId":environment,"label":"Fake host","platform":{"os":"linux","arch":"x64"},"serverVersion":"1.0.0","capabilities":{"terminal":true,"desktopIntegration":false},"orchestrationProtocolVersion":t3_contracts::environment::ORCHESTRATION_PROTOCOL_VERSION})
}

pub fn config(environment: &str) -> Value {
    let mut config: Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/ServerConfig.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    config["environment"]["environmentId"] = json!(environment);
    config["environment"]["label"] = json!("Fake host");
    config["environment"]["capabilities"]["commandOutputStreaming"] = json!(true);
    config
}

struct OpenCommandOutput {
    server: Weak<FakeServer>,
}

impl Drop for OpenCommandOutput {
    fn drop(&mut self) {
        if let Some(server) = self.server.upgrade() {
            server.state.borrow_mut().command_outputs_open -= 1;
        }
    }
}

pub fn thread_json(id: &str) -> Value {
    json!({"id":id,"projectId":"project","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":id,"parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":NOW,"updatedAt":NOW,"archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null})
}

pub fn shell_snapshot(threads: &[&str]) -> Value {
    let threads: Vec<Value> = threads
        .iter()
        .map(|id| {
            let mut thread = thread_json(id);
            thread.as_object_mut().unwrap().extend(json!({"latestRunId":null,"activeRunId":null,"status":"idle","pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":null,"hasActionableProposedPlan":false,"itemCount":1,"visibleItemCount":1}).as_object().unwrap().clone());
            thread
        })
        .collect();
    json!({"schemaVersion":1,"snapshotSequence":1,"threads":threads,"archivedThreads":[],"projects":[{"id":"project","title":"Project","workspaceRoot":"/workspace/project","repositoryIdentity":null,"defaultModelSelection":null,"scripts":[],"createdAt":NOW,"updatedAt":NOW}]})
}

pub fn thread_snapshot(id: &str) -> Value {
    let item = json!({"id":format!("{id}-item"),"type":"command_execution","threadId":id,"runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":"Command","input":"pwd","output":"done","exitCode":0,"startedAt":NOW,"completedAt":NOW,"updatedAt":NOW});
    json!({"kind":"snapshot","snapshotSequence":1,"projection":{"thread":thread_json(id),"runs":[],"attempts":[],"nodes":[],"subagents":[],"providerSessions":[],"providerThreads":[],"providerTurns":[],"runtimeRequests":[],"messages":[],"plans":[],"turnItems":[item.clone()],"checkpointScopes":[],"checkpoints":[],"contextHandoffs":[],"contextTransfers":[],"visibleTurnItems":[{"position":0,"visibility":"local","sourceThreadId":id,"sourceItemId":item["id"],"item":item}],"updatedAt":NOW}})
}

impl Storage for TestPlatform {
    fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
        if area == StorageArea::Secure
            && self.server.state.borrow().undecryptable.as_deref() == Some(key.namespace.as_str())
        {
            return async { Err(PlatformError::Storage("authentication tag mismatch".into())) }
                .boxed_local();
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

impl Platform for TestPlatform {
    fn now_epoch_ms(&self) -> u64 {
        self.fake.now_epoch_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.fake.monotonic_ms()
    }
    fn random_bytes(&self, length: usize) -> Result<Vec<u8>, PlatformError> {
        if self.server.state.borrow().fail_random {
            return Err(PlatformError::Unsupported("random source".into()));
        }
        self.fake.random_bytes(length)
    }
    fn sleep(&self, milliseconds: u64) -> LocalBoxFuture<'_, ()> {
        self.fake.sleep(milliseconds)
    }
    fn yield_to_host(&self) -> LocalBoxFuture<'_, ()> {
        self.fake.yield_to_host()
    }
    fn events(&self) -> LocalBoxStream<'static, PlatformEvent> {
        self.fake.events()
    }
    fn http(&self, request: HttpRequest) -> HostFuture<'_, HttpResponse> {
        let server = self.server.clone();
        async move {
            let url = url::Url::parse(&request.url).unwrap();
            let path = url.path().to_owned();
            if url.host_str() == Some("relay.test") {
                return Ok(relay_response(&server, &request, &path));
            }
            server.state.borrow_mut().http.push(path.clone());
            if server.state.borrow().unreachable {
                return Err(PlatformError::Network("host unreachable".into()));
            }
            let environment = server.environment.clone();
            Ok(match (request.method.as_str(), path.as_str()) {
                ("GET", "/.well-known/t3/environment") => {
                    let mut descriptor = descriptor(&environment);
                    if server.state.borrow().outdated {
                        descriptor["orchestrationProtocolVersion"] =
                            json!(t3_contracts::environment::ORCHESTRATION_PROTOCOL_VERSION - 1);
                        descriptor["capabilities"]["serverSelfUpdate"] = json!("respawn");
                    }
                    response(200, descriptor)
                }
                ("GET", "/api/auth/session") => response(
                    200,
                    json!({"authenticated":true,"auth":{"policy":"remote-reachable","bootstrapMethods":[],"sessionMethods":[],"sessionCookieName":"session"},"permissions":["orchestration:read","orchestration:operate"]}),
                ),
                ("POST", "/api/auth/websocket-ticket" | "/api/auth/websocket-ticket/rpc") => {
                    response(200, json!({"ticket":"ticket","expiresAt":"2036-01-01T00:00:00.000Z"}))
                }
                ("POST", "/oauth/token") => response(
                    200,
                    json!({"access_token":"paired-bearer","issued_token_type":"urn:ietf:params:oauth:token-type:access_token","token_type": if request.headers.keys().any(|name| name.eq_ignore_ascii_case("dpop")) { "DPoP" } else { "Bearer" },"expires_in":3600,"scope":"orchestration:read orchestration:operate"}),
                ),
                // Snapshots come over the socket; HTTP answers that it cannot serve them.
                _ => response(503, json!({})),
            })
        }
        .boxed_local()
    }
    fn websocket(&self, _request: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>> {
        let server = self.server.clone();
        async move {
            let refused = {
                let state = server.state.borrow();
                state.unreachable || state.refuse_sockets
            };
            if refused {
                return Err(PlatformError::Network("websocket upgrade refused".into()));
            }
            let (to_server, server_incoming) = mpsc::unbounded::<Frame>();
            let (server_outgoing, from_server) = mpsc::unbounded::<Frame>();
            let (hangup, hung_up) = mpsc::unbounded();
            {
                let mut state = server.state.borrow_mut();
                state.sockets_opened += 1;
                state.sockets_open += 1;
                state.hangups.push(hangup);
            }
            let handler = FakeHandler {
                server: Rc::downgrade(&server),
            };
            let weak = Rc::downgrade(&server);
            let serve = async move {
                let mut rpc = t3_rpc::AsyncServer::new(
                    server_outgoing.sink_map_err(|error| error.to_string()),
                    server_incoming.map(Ok::<Frame, String>),
                    handler,
                );
                {
                    use t3_contracts::rpc::*;
                    use t3_rpc::CallKind::{Stream, Unary};
                    for (tag, kind) in [
                        (tag::<WsServerGetConfigRpc>(), Unary),
                        (tag::<WsServerProbeRpc>(), Unary),
                        (tag::<WsSubscribeServerConfigRpc>(), Stream),
                        (tag::<WsOrchestrationV2SubscribeShellRpc>(), Stream),
                        (tag::<WsOrchestrationV2SubscribeThreadRpc>(), Stream),
                        (tag::<WsOrchestrationV2GetTurnDiffRpc>(), Unary),
                        (tag::<WsOrchestrationV2DispatchCommandRpc>(), Unary),
                        (tag::<WsOrchestrationV2SubscribeCommandOutputRpc>(), Stream),
                    ] {
                        rpc.state.register(tag, kind);
                    }
                }
                let run = rpc.run().fuse();
                let mut hung_up = hung_up.into_future().fuse();
                futures::pin_mut!(run);
                futures::select! {
                    _ = run => {},
                    _ = hung_up => {},
                }
                if let Some(server) = weak.upgrade() {
                    server.state.borrow_mut().sockets_open -= 1;
                }
            };
            server.spawner.spawn_local(serve).unwrap();
            Ok(Box::new(Socket {
                to_server: Some(to_server),
                from_server,
            }) as Box<dyn WebSocket>)
        }
        .boxed_local()
    }
    fn host(&self, action: HostAction) -> HostFuture<'_, HostReply> {
        let reply = |reply: HostReply| async move { Ok(reply) }.boxed_local();
        let signed_in = self.server.state.borrow().signed_in;
        let (jwk, thumbprint) = dpop_key();
        let ssh = |kind: &str, alias: &str| {
            self.server
                .state
                .borrow_mut()
                .ssh
                .push(format!("{kind} {alias}"));
        };
        match action {
            // Every client presents itself when pairing, as the TypeScript clients do.
            HostAction::ClientPresentation if !self.server.state.borrow().anonymous => {
                reply(HostReply::Document(json!({"label":"Test client"})))
            }
            HostAction::CloudIdentity => {
                reply(HostReply::Text(signed_in.then(|| ACCOUNT.to_owned())))
            }
            HostAction::CloudToken => reply(HostReply::Text(signed_in.then(|| cloud_token(ACCOUNT)))),
            HostAction::RelayDeviceId => reply(HostReply::Text(Some("device-1".into()))),
            HostAction::DpopPublicJwk => reply(HostReply::Document(jwk)),
            HostAction::DpopThumbprint => reply(HostReply::Text(Some(thumbprint))),
            HostAction::DpopSign { .. } => reply(HostReply::Bytes(vec![3; 64])),
            HostAction::SshProvision { target, .. } => {
                ssh("provision", &target.alias);
                reply(HostReply::Document(json!({"environmentId": self.server.environment, "label": "", "bootstrap": ssh_bootstrap(&target.alias), "bearerToken": "ssh-bearer"})))
            }
            HostAction::SshPrepare { target, .. } => {
                ssh("prepare", &target.alias);
                reply(HostReply::Document(json!({"bootstrap": ssh_bootstrap(&target.alias), "bearerToken": "ssh-bearer"})))
            }
            HostAction::SshDisconnect(target) => {
                ssh("disconnect", &target.alias);
                reply(HostReply::Unit)
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

/// Counts a `subscribeThread` stream as open until the server drops it.
struct OpenThread {
    server: Weak<FakeServer>,
    thread: String,
}

impl Drop for OpenThread {
    fn drop(&mut self) {
        if let Some(server) = self.server.upgrade() {
            let mut state = server.state.borrow_mut();
            if let Some(count) = state.open_threads.get_mut(&self.thread) {
                *count -= 1;
                if *count == 0 {
                    state.open_threads.remove(&self.thread);
                }
            }
        }
    }
}

struct FakeHandler {
    server: Weak<FakeServer>,
}

fn tag<M: RpcMethod>() -> &'static str {
    M::TAG
}

/// Answers `items`, then stays open; dropping the stream drops `guard`.
fn live<G: 'static>(items: Vec<Value>, guard: G) -> HandlerOutput {
    HandlerOutput::Stream(
        futures::stream::once(async move { Ok(items) })
            .chain(futures::stream::pending())
            .map(move |item| {
                let _ = &guard;
                item
            })
            .boxed_local(),
    )
}

impl Handler for FakeHandler {
    fn handle(&mut self, request: Request) -> HandlerOutput {
        use t3_contracts::rpc::*;
        let Some(server) = self.server.upgrade() else {
            return HandlerOutput::Unary(async { Exit::die("server gone") }.boxed_local());
        };
        let environment = server.environment.clone();
        let tag = request.tag.as_str();
        let extra = server.state.borrow().extra_rpc.clone();
        if let Some(output) = extra.and_then(|extra| extra(&request)) {
            return output;
        }
        let scripted = {
            let mut state = server.state.borrow_mut();
            state.rpc_calls.push((tag.to_owned(), request.payload.clone()));
            state.rpc_script.get_mut(tag).and_then(|script| script.pop_front())
        };
        if let Some(answer) = scripted {
            return HandlerOutput::Unary(async move { match answer {
                Ok(value) => Exit::success(value), Err(error) => Exit::fail(error),
            }}.boxed_local());
        }
        if tag == self::tag::<WsServerGetConfigRpc>() {
            let config = config(&environment);
            HandlerOutput::Unary(async move { Exit::success(config) }.boxed_local())
        } else if tag == self::tag::<WsServerProbeRpc>() {
            HandlerOutput::Unary(async { Exit::success(json!({})) }.boxed_local())
        } else if tag == self::tag::<WsSubscribeServerConfigRpc>() {
            live(
                vec![json!({"version":1,"type":"snapshot","config":config(&environment)})],
                (),
            )
        } else if tag == self::tag::<WsOrchestrationV2SubscribeShellRpc>() {
            server.state.borrow_mut().shell_subscribes += 1;
            live(
                vec![
                    json!({"kind":"snapshot","snapshot":shell_snapshot(&["t1", "t2"])}),
                    json!({"kind":"synchronized"}),
                ],
                (),
            )
        } else if tag == self::tag::<WsOrchestrationV2DispatchCommandRpc>()
            && self
                .server
                .upgrade()
                .is_some_and(|server| server.state.borrow().hold_dispatches)
        {
            HandlerOutput::Unary(futures::future::pending().boxed_local())
        } else if tag == self::tag::<WsOrchestrationV2DispatchCommandRpc>() {
            // The Rust server's refusal seen 2026-11-14 when no provider adapter is registered.
            let refusal = json!({"_tag":"OrchestrationV2DispatchCommandError","commandId":request.payload["commandId"],"commandType":request.payload["type"],"message":"Failed to dispatch orchestration V2 command","detail":"service ProviderAdapterRegistry.get: No orchestration provider adapter is registered for codex."});
            HandlerOutput::Unary(async move { Exit::fail(refusal) }.boxed_local())
        } else if tag == self::tag::<WsOrchestrationV2GetTurnDiffRpc>() {
            server.state.borrow_mut().turn_diffs += 1;
            let mut answer = request.payload.clone();
            answer["diff"] = json!("diff --git a/x b/x");
            if let Some(object) = answer.as_object_mut() {
                object.remove("ignoreWhitespace");
            }
            HandlerOutput::Unary(async move { Exit::success(answer) }.boxed_local())
        } else if tag == self::tag::<WsOrchestrationV2SubscribeCommandOutputRpc>() {
            let running = {
                let mut state = server.state.borrow_mut();
                state.command_output_subscribes += 1;
                state.command_outputs_open += 1;
                state.command_running
            };
            let mut frames =
                vec![json!({"kind":"replace","text":"install\n12%","truncated":false,"running":true})];
            if !running {
                frames.push(
                    json!({"kind":"append","text":"\r100%\n","truncated":false,"running":false}),
                );
            }
            live(
                frames,
                OpenCommandOutput {
                    server: self.server.clone(),
                },
            )
        } else if tag == self::tag::<WsOrchestrationV2SubscribeThreadRpc>() {
            let thread = request.payload["threadId"].as_str().unwrap().to_owned();
            {
                let mut state = server.state.borrow_mut();
                state.thread_subscribes.push(thread.clone());
                *state.open_threads.entry(thread.clone()).or_default() += 1;
            }
            let guard = OpenThread {
                server: self.server.clone(),
                thread: thread.clone(),
            };
            live(
                vec![server.state.borrow().thread_snapshots.get(&thread).cloned().unwrap_or_else(|| thread_snapshot(&thread)), json!({"kind":"synchronized"})],
                guard,
            )
        } else {
            let tag = tag.to_owned();
            eprintln!("fake server: unscripted {tag} {}", request.payload);
            HandlerOutput::Unary(
                async move { Exit::die(format!("unscripted {tag}")) }.boxed_local(),
            )
        }
    }
}

pub fn environment(id: &str) -> EnvironmentId {
    EnvironmentId(id.into())
}

pub fn thread(id: &str) -> ThreadId {
    ThreadId(id.into())
}
