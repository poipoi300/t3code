#![allow(clippy::unwrap_used)]
use futures::{
    FutureExt, Stream, StreamExt, channel::mpsc, executor::block_on, future::LocalBoxFuture,
    stream::LocalBoxStream,
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};
use t3_client::{
    ClientError,
    authorization::{AuthorizedEnvironment, HttpAuthorization, Sensitive},
    environment::SessionIdentity,
    permissions::EnvironmentHttpClient,
    platform::{HttpResponse, Platform},
    rpc::{RpcEndpoint, RpcMethod},
    services::{
        assets::*, filesystem::*, http::*, resources::*, runtime::Services, sessions::*,
        updates::*, usage::*, views::*,
    },
    testing::{FakePlatform, RpcScript, ScriptedRpc},
    types::{EnvironmentId, Generation},
};
use t3_contracts::{
    auth::*, environment::ServerSelfUpdateMethod, peer_link::PeerLinkSummary, server::*,
};

fn env() -> EnvironmentId {
    EnvironmentId("env".into())
}
fn identity(generation: u64) -> SessionIdentity {
    SessionIdentity {
        environment: env(),
        generation: Generation(generation),
        account: None,
        dpop_thumbprint: None,
    }
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}
fn response(value: Value) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: Default::default(),
        body: serde_json::to_vec(&value).unwrap(),
    }
}
fn prepared(authorization: HttpAuthorization) -> AuthorizedEnvironment {
    AuthorizedEnvironment {
        session: identity(1),
        http_base_url: "https://remote.test/".into(),
        socket_url: Sensitive("wss://remote.test/ws".into()),
        authorization,
        descriptor: None,
    }
}
fn ready(version: &str, at: &str, outcome: Value) -> ServerLifecycleStreamReadyEvent {
    let mut config: ServerConfig = serde_json::from_str(
        include_str!("../../../fixtures/samples/ServerConfig.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    config.environment.server_version.0 = version.into();
    let mut value = json!({"version":1,"sequence":1,"type":"ready","payload":{"environment":config.environment,"at":at}});
    if !outcome.is_null() {
        value["payload"]["updateOutcome"] = outcome;
    }
    decode(value)
}

struct Number;
impl RpcMethod for Number {
    type Input = u64;
    type Output = u64;
    const NAME: &'static str = "test.number";
}
struct DropEvents {
    receiver: mpsc::UnboundedReceiver<Value>,
    dropped: Rc<Cell<usize>>,
}
impl Stream for DropEvents {
    type Item = Value;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Value>> {
        Pin::new(&mut self.receiver).poll_next(cx)
    }
}
impl Drop for DropEvents {
    fn drop(&mut self) {
        self.dropped.set(self.dropped.get() + 1);
    }
}
struct EventRpc {
    events: RefCell<Option<mpsc::UnboundedReceiver<Value>>>,
    drops: Rc<Cell<usize>>,
    subscriptions: Cell<usize>,
    inputs: RefCell<Vec<(String, Value)>>,
}
impl EventRpc {
    fn new() -> (Rc<Self>, mpsc::UnboundedSender<Value>) {
        let (tx, rx) = mpsc::unbounded();
        (
            Rc::new(Self {
                events: RefCell::new(Some(rx)),
                drops: Rc::new(Cell::new(0)),
                subscriptions: Cell::new(0),
                inputs: Default::default(),
            }),
            tx,
        )
    }
}
impl RpcEndpoint for EventRpc {
    fn destination(&self) -> Option<EnvironmentId> {
        Some(env())
    }
    fn call<M: RpcMethod>(
        &self,
        _: M::Input,
    ) -> LocalBoxFuture<'_, Result<M::Output, ClientError>> {
        async {
            futures::future::pending::<()>().await;
            Err(ClientError::Disconnected)
        }
        .boxed_local()
    }
    fn subscribe<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxStream<'static, Result<M::Output, ClientError>> {
        self.inputs
            .borrow_mut()
            .push((M::NAME.into(), serde_json::to_value(input).unwrap()));
        self.subscriptions.set(self.subscriptions.get() + 1);
        DropEvents {
            receiver: self.events.borrow_mut().take().unwrap(),
            dropped: self.drops.clone(),
        }
        .map(|value| serde_json::from_value(value).map_err(Into::into))
        .boxed_local()
    }
}

#[test]
fn replacement_cancels_buffered_old_frames_and_preserves_each_new_event() {
    block_on(async {
        let (sessions, changes) = mpsc::unbounded();
        let (old, old_events) = EventRpc::new();
        let (new, new_events) = EventRpc::new();
        sessions
            .unbounded_send(Some(ServiceSession {
                identity: identity(1),
                endpoint: old.clone(),
            }))
            .unwrap();
        let mut stream = follow_rpc::<Number, _>(env(), changes.boxed_local(), 0);
        assert!(stream.next().now_or_never().is_none());
        old_events.unbounded_send(json!(1)).unwrap();
        sessions
            .unbounded_send(Some(ServiceSession {
                identity: identity(2),
                endpoint: new.clone(),
            }))
            .unwrap();
        for n in 2..=5 {
            new_events.unbounded_send(json!(n)).unwrap();
        }
        for n in 2..=5 {
            assert_eq!(stream.next().await.unwrap().unwrap(), n);
        }
        assert_eq!(old.drops.get(), 1);
        sessions
            .unbounded_send(Some(ServiceSession {
                identity: identity(2),
                endpoint: new.clone(),
            }))
            .unwrap();
        assert!(stream.next().now_or_never().is_none());
        assert_eq!(new.subscriptions.get(), 1);
        sessions.unbounded_send(None).unwrap();
        assert!(stream.next().now_or_never().is_none());
        assert_eq!(new.drops.get(), 1);
    });
}

#[test]
fn session_follow_keeps_readable_state_and_drops_work_on_disconnect_and_disposal() {
    let platform = FakePlatform::default();
    let resource = Resource::new(ResourcePolicy::default(), Generation(0));
    let (sessions, changes) = mpsc::unbounded();
    let (rpc, events) = EventRpc::new();
    let mut driver = Box::pin(follow_resource::<Number, _, _>(
        &resource,
        &platform,
        env(),
        changes.boxed_local(),
        0,
        |_| false,
    ));
    sessions
        .unbounded_send(Some(ServiceSession {
            identity: identity(1),
            endpoint: rpc.clone(),
        }))
        .unwrap();
    assert!(driver.as_mut().now_or_never().is_none());
    events.unbounded_send(json!(7)).unwrap();
    assert!(driver.as_mut().now_or_never().is_none());
    assert_eq!(resource.snapshot().value.as_deref(), Some(&7));
    assert!(resource.snapshot().loading);
    sessions.unbounded_send(None).unwrap();
    assert!(driver.as_mut().now_or_never().is_none());
    assert_eq!(rpc.drops.get(), 1);
    assert_eq!(resource.snapshot().value.as_deref(), Some(&7));
    assert!(!resource.snapshot().loading);
    let (new, _) = EventRpc::new();
    sessions
        .unbounded_send(Some(ServiceSession {
            identity: identity(2),
            endpoint: new.clone(),
        }))
        .unwrap();
    assert!(driver.as_mut().now_or_never().is_none());
    drop(driver);
    assert_eq!(new.drops.get(), 1);
    assert!(!resource.snapshot().loading);
}

#[test]
fn query_connection_policy_retains_cached_answers_during_retry_and_reports_terminal_failures() {
    for phase in [
        QueryConnectionPhase::Connecting,
        QueryConnectionPhase::Reconnecting,
        QueryConnectionPhase::Backoff,
    ] {
        assert_eq!(
            query_availability(phase, false, false, Some("denied")),
            QueryAvailability::Fail("denied".into())
        );
        assert_eq!(
            query_availability(phase, false, true, Some("denied")),
            QueryAvailability::Wait
        );
        assert_eq!(
            query_availability(phase, false, false, None),
            QueryAvailability::Wait
        );
    }
    for phase in [
        QueryConnectionPhase::Offline,
        QueryConnectionPhase::Blocked,
        QueryConnectionPhase::Available,
    ] {
        assert!(matches!(
            query_availability(phase, false, true, None),
            QueryAvailability::Fail(_)
        ));
    }
    assert_eq!(
        query_availability(QueryConnectionPhase::Connected, false, false, None),
        QueryAvailability::Wait
    );
    assert_eq!(
        query_availability(QueryConnectionPhase::Connected, true, false, None),
        QueryAvailability::Execute
    );
}

#[test]
fn pairing_payloads_preserve_every_access_level_and_mints_are_independent() {
    block_on(async {
        for (access, count) in [
            (AuthMcpClientAccess::ReadOnly, 1),
            (AuthMcpClientAccess::ApprovalRequired, 2),
            (AuthMcpClientAccess::AutoAcceptEdits, 2),
            (AuthMcpClientAccess::Auto, 2),
            (AuthMcpClientAccess::FullAccess, 4),
        ] {
            let payload = peer_pairing_payload(access);
            assert_eq!(payload.scopes.unwrap().len(), count);
            assert!(payload.access.is_none());
        }
        let platform = Rc::new(FakePlatform::default());
        let client = EnvironmentHttpClient::new(
            platform.clone(),
            prepared(HttpAuthorization::Cookie),
            None,
            None,
            false,
        );
        for code in ["first", "second"] {
            platform.push_http(Ok(response(
                json!({"id":code,"credential":code,"expiresAt":"2026-11-13T00:00:00Z"}),
            )));
            assert_eq!(
                mint_peer_pairing_code(&client, AuthMcpClientAccess::ReadOnly)
                    .await
                    .unwrap()
                    .expose(),
                code
            );
        }
        platform.push_http(Ok(response(
            json!({"id":"approval","credential":"AUTO-code","expiresAt":"2026-11-13T00:00:00Z"}),
        )));
        mint_mcp_approval_code(&client, AuthMcpClientAccess::Auto)
            .await
            .unwrap();
        let requests = platform.http_requests();
        assert_eq!(requests.len(), 3);
        let body: Value = serde_json::from_slice(&requests[2].body).unwrap();
        assert_eq!(body, json!({"label":"App sign-in","access":"auto"}));
    });
}

#[test]
fn hub_access_keeps_cookie_mode_and_mints_a_fresh_media_ticket_on_every_reconnect() {
    block_on(async {
        let platform = Rc::new(FakePlatform::default());
        let cookie = prepared(HttpAuthorization::Cookie);
        let client =
            EnvironmentHttpClient::new(platform.clone(), cookie.clone(), None, None, false);
        let access = resolve_device_hub_access(&client, &cookie, "/api/device-hub")
            .await
            .unwrap()
            .into_inner();
        assert!(access.credentials);
        assert!(access.query.is_empty());
        assert!(platform.http_requests().is_empty());
        let bearer = prepared(HttpAuthorization::Bearer(Sensitive("secret".into())));
        let client =
            EnvironmentHttpClient::new(platform.clone(), bearer.clone(), None, None, false);
        for ticket in ["one", "two"] {
            platform.push_http(Ok(response(
                json!({"ticket":ticket,"expiresAt":"2026-11-13T00:00:00Z"}),
            )));
            let access = resolve_device_hub_access(&client, &bearer, "/api/device-hub")
                .await
                .unwrap()
                .into_inner();
            assert!(!access.credentials);
            assert_eq!(access.query["wsTicket"], ticket);
            assert_eq!(access.ws_base, "wss://remote.test/api/device-hub");
        }
        assert!(
            platform
                .http_requests()
                .iter()
                .all(|r| r.url.ends_with("/api/auth/websocket-ticket/media"))
        );
    });
}

#[test]
fn attachment_size_decimal_ties_follow_javascript_to_fixed() {
    for (bytes, expected) in [
        (1_047_552, "1023 KB"),
        (1_047_553, "1.0 MB"),
        (1_310_720, "1.3 MB"),
        (2_621_440, "2.5 MB"),
    ] {
        assert_eq!(format_attachment_size(bytes), expected);
    }
}

#[test]
fn peer_health_day_rounding_is_ceil_at_integer_millisecond_boundaries() {
    let mut link: PeerLinkSummary = decode(
        json!({"environmentId":"peer","label":"Peer","urls":["https://peer.test"],"access":"read-only","linkedAt":"2026-11-01T00:00:00Z","expiresAt":"2026-11-20T00:00:00.000Z","lastReachedAt":null,"lastError":null,"status":"reachable"}),
    );
    let expires = t3_client::services::config::timestamp_ms(&link.expires_at.0).unwrap();
    for (remaining, days) in [
        (1, Some(1)),
        (86_400_000, Some(1)),
        (86_400_001, Some(2)),
        (432_000_000, Some(5)),
        (432_000_001, None),
    ] {
        assert_eq!(
            peer_link_health(&link, expires - remaining),
            PeerLinkHealth::Reachable {
                expires_in_days: days
            }
        );
    }
    assert_eq!(peer_link_health(&link, expires), PeerLinkHealth::Expired);
    link.status = t3_contracts::peer_link::PeerLinkStatus::Unreachable;
    link.last_error = Some("refused".into());
    assert_eq!(
        peer_link_health(&link, expires - 1),
        PeerLinkHealth::Unreachable {
            detail: Some("refused".into())
        }
    );
    link.status = t3_contracts::peer_link::PeerLinkStatus::Reachable;
    assert_eq!(
        peer_link_health(&link, expires - 1),
        PeerLinkHealth::Failing {
            detail: "refused".into()
        }
    );
}

#[test]
fn access_matrices_preserve_offline_grants_and_distinguish_denied_and_pending() {
    let session: AuthSessionState = decode(
        json!({"authenticated":true,"auth":{"policy":"remote-reachable","bootstrapMethods":[],"sessionMethods":[],"sessionCookieName":"session","serverUpdateScope":"environment:maintain"},"scopes":["filesystem:read","diagnostics:read"]}),
    );
    for ready in [false, true] {
        assert!(filesystem_read_access(ready, Some("offline"), None, Some(&session), None).allowed);
    }
    for phase in ["connecting", "reconnecting", "connected"] {
        assert!(usage_access(phase, None, false).pending);
    }
    for phase in ["available", "offline", "error"] {
        assert!(!usage_access(phase, None, false).pending);
    }
    assert!(usage_access("offline", Some(&session), false).allowed);
    assert!(!usage_access("connected", Some(&session), true).allowed);
    assert!(filesystem_read_access(false, None, None, None, None).pending);
    let mut denied = session.clone();
    denied.scopes = Some(vec![AuthEnvironmentScope::OrchestrationRead]);
    assert!(!filesystem_read_access(true, Some("connected"), None, Some(&denied), None).allowed);
    assert!(!usage_access("connected", Some(&denied), false).allowed);
    denied.auth.server_update_scope = None;
    assert!(filesystem_read_access(true, Some("connected"), None, Some(&denied), None).allowed);
}

type Lifecycle =
    LocalBoxStream<'static, Result<(Generation, ServerLifecycleStreamReadyEvent), ClientError>>;
type Connections = LocalBoxStream<'static, Result<UpdateConnectionState, ClientError>>;
struct UpdateScript {
    session: UpdateSession,
    lifecycle: RefCell<VecDeque<Lifecycle>>,
    connections: RefCell<VecDeque<Connections>>,
    update: RefCell<Option<Result<ServerSelfUpdateResult, ClientError>>>,
    progress: RefCell<Vec<Result<ServerSelfUpdateProgressEvent, ClientError>>>,
    commits: RefCell<VecDeque<Result<(), ClientError>>>,
    commit_count: Cell<usize>,
    retries: Cell<usize>,
    calls: RefCell<Vec<Generation>>,
    on_commit: RefCell<Option<Box<dyn FnOnce()>>>,
}
impl UpdateScript {
    fn new(method: ServerSelfUpdateMethod, progress: bool) -> Self {
        Self {
            session: UpdateSession {
                generation: Generation(1),
                version: "1".into(),
                method,
                progress,
            },
            lifecycle: Default::default(),
            connections: Default::default(),
            update: Default::default(),
            progress: Default::default(),
            commits: Default::default(),
            commit_count: Cell::new(0),
            retries: Cell::new(0),
            calls: Default::default(),
            on_commit: Default::default(),
        }
    }
    fn push_ready(&self, version: &str, at: &str, outcome: Value) {
        self.lifecycle.borrow_mut().push_back(
            futures::stream::once(futures::future::ready(Ok((
                Generation(1),
                ready(version, at, outcome),
            ))))
            .chain(futures::stream::pending())
            .boxed_local(),
        );
    }
}
impl UpdateHost for UpdateScript {
    fn session(&self) -> Result<UpdateSession, ClientError> {
        Ok(self.session.clone())
    }
    fn lifecycle(&self) -> Lifecycle {
        self.lifecycle
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| futures::stream::pending().boxed_local())
    }
    fn states(&self) -> Connections {
        self.connections
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| {
                futures::stream::once(futures::future::ready(Ok(UpdateConnectionState {
                    phase: QueryConnectionPhase::Connected,
                    authentication_failure: false,
                })))
                .chain(futures::stream::pending())
                .boxed_local()
            })
    }
    fn update(
        &self,
        generation: Generation,
        _: ServerSelfUpdateInput,
    ) -> LocalBoxFuture<'_, Result<ServerSelfUpdateResult, ClientError>> {
        self.calls.borrow_mut().push(generation);
        async { self.update.borrow_mut().take().unwrap() }.boxed_local()
    }
    fn progress(
        &self,
        generation: Generation,
        _: ServerSelfUpdateInput,
    ) -> LocalBoxStream<'static, Result<ServerSelfUpdateProgressEvent, ClientError>> {
        self.calls.borrow_mut().push(generation);
        futures::stream::iter(std::mem::take(&mut *self.progress.borrow_mut())).boxed_local()
    }
    fn commit(&self, _: String) -> LocalBoxFuture<'_, Result<(), ClientError>> {
        async {
            self.commit_count.set(self.commit_count.get() + 1);
            if let Some(f) = self.on_commit.borrow_mut().take() {
                f();
            }
            self.commits.borrow_mut().pop_front().unwrap_or(Ok(()))
        }
        .boxed_local()
    }
    fn retry_now(&self) -> LocalBoxFuture<'_, Result<(), ClientError>> {
        async {
            self.retries.set(self.retries.get() + 1);
            Ok(())
        }
        .boxed_local()
    }
}
fn input() -> ServerSelfUpdateInput {
    decode(json!({"targetVersion":"2"}))
}
fn connected(phase: QueryConnectionPhase) -> Result<UpdateConnectionState, ClientError> {
    Ok(UpdateConnectionState {
        phase,
        authentication_failure: false,
    })
}

#[test]
fn launcher_update_uses_accepting_baseline_then_rejects_a_foreign_restart_promptly() {
    block_on(async {
        let host = UpdateScript::new(ServerSelfUpdateMethod::BootService, false);
        host.push_ready("1", "2026-11-13T00:00:00Z", Value::Null);
        host.push_ready("1", "2026-11-13T00:01:00Z", Value::Null);
        *host.update.borrow_mut() = Some(Ok(decode(
            json!({"targetVersion":"2","method":"boot-service","updateId":"id"}),
        )));
        let platform = FakePlatform::default();
        let service = ServerUpdateService::default();
        let error = service
            .run(env(), &host, &platform, input())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("without committing"));
        assert_eq!(*host.calls.borrow(), vec![Generation(1)]);
        assert!(matches!(
            service.state(env()).snapshot().value.as_deref(),
            Some(ServerUpdateState::Failed {
                stage: ServerUpdateStage::Resuming,
                ..
            })
        ));
        assert_eq!(platform.pending_timers(), 0);
    });
}

#[test]
fn progress_completion_survives_handoff_but_a_protocol_failure_does_not() {
    block_on(async {
        for transport_loss in [true, false] {
            let host = UpdateScript::new(ServerSelfUpdateMethod::Respawn, true);
            host.progress.borrow_mut().extend([
                Ok(decode(json!({"type":"progress","stage":"installing"}))),
                Ok(decode(
                    json!({"type":"complete","result":{"targetVersion":"2","method":"respawn"}}),
                )),
                Err(if transport_loss {
                    ClientError::Disconnected
                } else {
                    ClientError::InvalidPayload("protocol".into())
                }),
            ]);
            host.push_ready("2", "2026-11-13T00:01:00Z", Value::Null);
            let service = ServerUpdateService::default();
            let result = service
                .run(env(), &host, &FakePlatform::default(), input())
                .await;
            assert_eq!(result.is_ok(), transport_loss);
            if transport_loss {
                assert_eq!(
                    service.state(env()).snapshot().value.as_deref(),
                    Some(&ServerUpdateState::Idle)
                );
            }
        }
    });
}

#[test]
fn desktop_commit_observer_is_armed_before_a_synchronous_disconnect_and_fast_reconnect() {
    block_on(async {
        let host = UpdateScript::new(ServerSelfUpdateMethod::DesktopApp, false);
        let (tx, rx) = mpsc::unbounded();
        tx.unbounded_send(connected(QueryConnectionPhase::Connected))
            .unwrap();
        host.connections.borrow_mut().push_back(rx.boxed_local());
        host.commits
            .borrow_mut()
            .push_back(Err(ClientError::Disconnected));
        *host.on_commit.borrow_mut() = Some(Box::new(move || {
            tx.unbounded_send(connected(QueryConnectionPhase::Backoff))
                .unwrap();
            tx.unbounded_send(connected(QueryConnectionPhase::Connected))
                .unwrap();
        }));
        let platform = FakePlatform::default();
        assert!(matches!(
            run_desktop_commit_with_reconnect_observer(&host, &platform, "token".into()).await,
            Err(ClientError::Disconnected)
        ));
        assert_eq!(host.commit_count.get(), 1);
        assert_eq!(platform.pending_timers(), 0);
    });
}

#[test]
fn desktop_retries_transport_lost_commits_but_success_on_the_wrong_version_is_failure() {
    block_on(async {
        for successful_retry in [true, false] {
            let host = UpdateScript::new(ServerSelfUpdateMethod::DesktopApp, false);
            let result = decode(
                json!({"targetVersion":"2","method":"desktop-app","desktopUpdateToken":"token"}),
            );
            host.push_ready("1", "2026-11-13T00:00:00Z", Value::Null);
            if successful_retry {
                host.commits.borrow_mut().push_back(Ok(()));
            } else {
                host.push_ready("2", "2026-11-13T00:01:00Z", Value::Null);
                host.commits
                    .borrow_mut()
                    .push_back(Err(ClientError::Disconnected));
                host.connections.borrow_mut().push_back(
                    futures::stream::iter([
                        connected(QueryConnectionPhase::Connected),
                        connected(QueryConnectionPhase::Backoff),
                        connected(QueryConnectionPhase::Connected),
                    ])
                    .chain(futures::stream::pending())
                    .boxed_local(),
                );
            }
            let ready =
                wait_for_desktop_update_target(&host, &FakePlatform::default(), &result, None)
                    .await;
            assert_eq!(ready.is_err(), successful_retry);
            assert_eq!(host.commit_count.get(), 1);
        }
    });
}

#[test]
fn update_nudges_each_retry_entry_and_only_authentication_blocks_with_injected_pacing() {
    let host = UpdateScript::new(ServerSelfUpdateMethod::Respawn, false);
    let (tx, rx) = mpsc::unbounded();
    host.connections.borrow_mut().push_back(rx.boxed_local());
    let platform = FakePlatform::default();
    let mut nudge = Box::pin(nudge_reconnect_during_update(&host, &platform, 1_000));
    tx.unbounded_send(connected(QueryConnectionPhase::Backoff))
        .unwrap();
    assert!(nudge.as_mut().now_or_never().is_none());
    assert_eq!(host.retries.get(), 0);
    platform.advance(1_000);
    assert_eq!(platform.monotonic_ms(), 1_000);
    assert!(nudge.as_mut().now_or_never().is_none());
    assert_eq!(host.retries.get(), 1);
    tx.unbounded_send(connected(QueryConnectionPhase::Blocked))
        .unwrap();
    tx.unbounded_send(Ok(UpdateConnectionState {
        phase: QueryConnectionPhase::Blocked,
        authentication_failure: true,
    }))
    .unwrap();
    assert!(nudge.as_mut().now_or_never().is_none());
    platform.advance(1_000);
    assert!(nudge.as_mut().now_or_never().is_none());
    assert_eq!(host.retries.get(), 2);
    drop(nudge);
    assert_eq!(platform.pending_timers(), 0);
}

#[test]
fn update_timeout_and_disposal_settle_state_and_release_timers() {
    let host = UpdateScript::new(ServerSelfUpdateMethod::Respawn, false);
    *host.update.borrow_mut() = Some(Ok(decode(json!({"targetVersion":"2","method":"respawn"}))));
    let platform = FakePlatform::default();
    let service = ServerUpdateService::default();
    let mut update = Box::pin(service.run(env(), &host, &platform, input()));
    assert!(update.as_mut().now_or_never().is_none());
    platform.advance(UPDATE_RESUME_TIMEOUT_MS);
    assert!(update.as_mut().now_or_never().unwrap().is_err());
    drop(update);
    assert!(matches!(
        service.state(env()).snapshot().value.as_deref(),
        Some(ServerUpdateState::Failed { .. })
    ));
    *host.update.borrow_mut() = Some(Ok(decode(json!({"targetVersion":"2","method":"respawn"}))));
    let mut update = Box::pin(service.run(env(), &host, &platform, input()));
    assert!(update.as_mut().now_or_never().is_none());
    drop(update);
    assert_eq!(
        service.state(env()).snapshot().value.as_deref(),
        Some(&ServerUpdateState::Idle)
    );
    assert_eq!(platform.pending_timers(), 0);
}

#[test]
fn attachment_transfer_failure_retains_the_mint_and_each_verification_calls_the_server() {
    block_on(async {
        let rpc = ScriptedRpc::default();
        let upload: t3_contracts::assets::AttachmentCreateUploadUrlInput = decode(
            json!({"type":"file","name":"report.pdf","mimeType":"application/pdf","sizeBytes":3}),
        );
        rpc.scripts.borrow_mut().push_back(RpcScript {
            method: "attachments.createUploadUrl".into(),
            input: json!(upload),
            replies: vec![Ok(
                json!({"attachmentId":"fresh","relativeUrl":"/upload","expiresAt":1}),
            )],
        });
        let platform = FakePlatform::default();
        platform.push_http(Ok(HttpResponse {
            status: 413,
            headers: Default::default(),
            body: vec![],
        }));
        assert!(
            matches!(attachment_upload_cycle(&rpc, &platform, "https://host.test", upload, vec![1,2,3], "application/pdf", |_| true).await, AttachmentUploadResult::Failed { step: AttachmentUploadStep::Transfer, attachment_id: Some(id), .. } if id == "fresh")
        );
        for reply in [
            Err(ClientError::Disconnected),
            Ok(json!({"relativeUrl":"/asset","expiresAt":1})),
        ] {
            rpc.scripts.borrow_mut().push_back(RpcScript {
                method: "assets.createUrl".into(),
                input: json!({"resource":{"_tag":"attachment","attachmentId":"fresh"}}),
                replies: vec![reply],
            });
        }
        assert!(matches!(
            verify_persisted_attachment(&rpc, "fresh".into()).await,
            AttachmentVerification::Failed(_)
        ));
        assert!(matches!(
            verify_persisted_attachment(&rpc, "fresh".into()).await,
            AttachmentVerification::Verified
        ));
        assert!(rpc.scripts.borrow().is_empty());
    });
}

#[test]
fn service_disconnect_rejects_old_projection_and_welcome_events() {
    let services = Services::default();
    services.begin_session(env(), Generation(1));
    services.disconnect(&env());
    let config: ServerConfig = serde_json::from_str(
        include_str!("../../../fixtures/samples/ServerConfig.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert!(
        !services
            .apply_server_config(
                env(),
                Generation(1),
                decode(json!({"version":1,"type":"snapshot","config":config}))
            )
            .unwrap()
    );
    assert!(!services.apply_lifecycle(&env(), Generation(1), decode(json!({"version":1,"sequence":1,"type":"ready","payload":{"at":"2026-11-13T00:00:00Z","environment":config.environment}}))));
    assert!(services.welcome(&env()).is_none());
}

#[test]
fn terminal_driver_preserves_unanswered_attach_input_and_resumes_only_after_a_reply() {
    use t3_client::{
        services::{server::drive_terminal_attach, terminal::*},
        types::{ThreadId, ThreadKey},
    };
    let services = Services::default();
    let input: t3_contracts::terminal::TerminalAttachInput = decode(
        json!({"threadId":"thread","terminalId":"terminal","cwd":"/repo","restartIfNotRunning":true}),
    );
    let (tx, rx) = mpsc::unbounded();
    let (first, _events) = EventRpc::new();
    tx.unbounded_send(Some(ServiceSession {
        identity: identity(1),
        endpoint: first.clone(),
    }))
    .unwrap();
    let mut driver = Box::pin(drive_terminal_attach(
        &services,
        env(),
        rx.boxed_local(),
        input.clone(),
    ));
    assert!(driver.as_mut().now_or_never().is_none());
    let mut expected = json!(input);
    expected["maxHistoryBytes"] = json!(DEFAULT_MAX_TERMINAL_BUFFER_BYTES);
    assert_eq!(first.inputs.borrow()[0].1, expected);
    let (second, events) = EventRpc::new();
    tx.unbounded_send(Some(ServiceSession {
        identity: identity(2),
        endpoint: second.clone(),
    }))
    .unwrap();
    assert!(driver.as_mut().now_or_never().is_none());
    assert_eq!(first.drops.get(), 1);
    assert_eq!(second.inputs.borrow()[0].1, expected);
    events
        .unbounded_send(json!({"type":"closed","threadId":"thread","terminalId":"terminal"}))
        .unwrap();
    assert!(driver.as_mut().now_or_never().is_none());
    let key = TerminalKey {
        thread: ThreadKey {
            environment: env(),
            thread: ThreadId("thread".into()),
        },
        terminal: "terminal".into(),
    };
    assert_eq!(
        services.terminal_buffer(&key).unwrap().status,
        TerminalStatus::Closed
    );
    let (third, _events) = EventRpc::new();
    tx.unbounded_send(Some(ServiceSession {
        identity: identity(3),
        endpoint: third.clone(),
    }))
    .unwrap();
    assert!(driver.as_mut().now_or_never().is_none());
    expected["resume"] = json!(true);
    assert_eq!(third.inputs.borrow()[0].1, expected);
    assert_eq!(
        services.terminal_buffer(&key).unwrap().status,
        TerminalStatus::Closed
    );
    drop(driver);
    assert_eq!(third.drops.get(), 1);
}

#[test]
fn terminal_metadata_driver_has_no_authoritative_empty_seed_and_retains_data_on_reconnect() {
    use t3_client::services::server::drive_terminal_metadata;
    let services = Services::default();
    let (tx, rx) = mpsc::unbounded();
    let (first, events) = EventRpc::new();
    tx.unbounded_send(Some(ServiceSession {
        identity: identity(1),
        endpoint: first,
    }))
    .unwrap();
    let mut driver = Box::pin(drive_terminal_metadata(&services, env(), rx.boxed_local()));
    assert!(driver.as_mut().now_or_never().is_none());
    assert!(services.terminal_metadata(&env()).is_none());
    events.unbounded_send(json!({"type":"snapshot","terminals":[{"threadId":"thread","terminalId":"terminal","cwd":"/repo","worktreePath":null,"status":"running","pid":123,"exitCode":null,"exitSignal":null,"hasRunningSubprocess":false,"label":"Terminal","updatedAt":"2026-11-13T00:00:00Z"}]})).unwrap();
    assert!(driver.as_mut().now_or_never().is_none());
    assert_eq!(services.terminal_metadata(&env()).unwrap().len(), 1);
    let (second, events) = EventRpc::new();
    tx.unbounded_send(Some(ServiceSession {
        identity: identity(2),
        endpoint: second,
    }))
    .unwrap();
    assert!(driver.as_mut().now_or_never().is_none());
    assert_eq!(services.terminal_metadata(&env()).unwrap().len(), 1);
    events
        .unbounded_send(json!({"type":"snapshot","terminals":[]}))
        .unwrap();
    assert!(driver.as_mut().now_or_never().is_none());
    assert!(services.terminal_metadata(&env()).unwrap().is_empty());
}

#[test]
fn removing_an_environment_cancels_an_update_and_clears_its_timers() {
    let service = ServerUpdateService::default();
    let platform = FakePlatform::default();
    let host = UpdateScript::new(ServerSelfUpdateMethod::Respawn, false);
    *host.update.borrow_mut() = Some(Ok(decode(json!({"targetVersion":"2","method":"respawn"}))));
    let mut update = Box::pin(service.run(env(), &host, &platform, input()));
    assert!(update.as_mut().now_or_never().is_none());
    let state = service.state(env());
    service.remove_environment(&env());
    assert!(matches!(block_on(update), Err(ClientError::Cancelled)));
    assert!(state.is_closed());
    assert_eq!(platform.pending_timers(), 0);
}

#[test]
fn hosted_approval_answers_preserve_refusals_redirects_and_the_csrf_retry_predicate() {
    let details =
        json!({"clientName":"Editor","redirectHost":"editor.test","environmentHost":"host.test"});
    assert!(approval_without_csrf(&details));
    let mut authorized = details.clone();
    authorized["csrfToken"] = json!("token");
    assert!(!approval_without_csrf(&authorized));
    assert!(matches!(
        approval_answer(authorized).unwrap(),
        ConnectMcpApprovalAnswer::Details(_)
    ));
    assert!(
        matches!(approval_answer(json!({"redirectTo":"https://editor.test/callback"})).unwrap(),ConnectMcpApprovalAnswer::Redirect(url) if url=="https://editor.test/callback")
    );
    assert!(
        matches!(approval_answer(json!({"_tag":"AuthMcpApprovalError","message":"Request expired"})).unwrap(),ConnectMcpApprovalAnswer::Refused(message) if message=="Request expired")
    );
    assert!(approval_answer(json!({"message":"malformed"})).is_err());
}
