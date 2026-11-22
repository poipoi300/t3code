//! The worktree-setup card and signed asset URL services the timeline rows bind to
//! (useWorktreeSetup.ts, assetUrls.ts), against the fake server.
#![allow(clippy::unwrap_used)]

mod support {
    pub mod fake_server;
}
use futures::{FutureExt, StreamExt, channel::mpsc};
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc};
use support::fake_server::*;
use t3_client::{
    ClientError, ClientStore, composition::ClientRuntime, connection::PairingInput,
    permissions::SessionGrants, services::assets::AssetUrlState, types::ThreadKey,
};
use t3_contracts::{
    assets::AssetResource,
    orchestration_v2::OrchestrationV2CreationSource,
    rpc::{RpcMethod, WsOrchestrationV2SubscribeShellRpc},
    worktree_setup::WorktreeSetupSnapshot,
};
use t3_rpc::{Exit, HandlerOutput};

type Frames = Rc<RefCell<Vec<mpsc::UnboundedSender<Value>>>>;

struct Server {
    calls: Rc<RefCell<Vec<(String, Value)>>>,
    setup_streams: Frames,
    cancel: Rc<RefCell<Result<Value, Value>>>,
}

fn key(thread: &str) -> ThreadKey {
    ThreadKey {
        environment: environment("env"),
        thread: support::fake_server::thread(thread),
    }
}

/// `t1` has a worktree and a preparing run; `t2` has neither.
fn shell() -> Value {
    let mut shell = shell_snapshot(&["t1", "t2"]);
    let t1 = &mut shell["threads"][0];
    t1["worktreePath"] = json!("/repo/.t3/worktrees/feature");
    t1["latestRunId"] = json!("run-1");
    t1["status"] = json!("preparing");
    shell
}

fn setup(h: &mut Harness) -> (ClientRuntime<TestPlatform>, Server) {
    let server = Server {
        calls: Rc::default(),
        setup_streams: Rc::default(),
        cancel: Rc::new(RefCell::new(Ok(json!({"cancelled":true})))),
    };
    h.server().state.borrow_mut().extra_methods = vec![
        ("subscribeWorktreeSetup", t3_rpc::CallKind::Stream),
        ("worktreeSetup.cancel", t3_rpc::CallKind::Unary),
        ("assets.createUrl", t3_rpc::CallKind::Unary),
    ];
    let (calls, streams, cancel) = (
        server.calls.clone(),
        server.setup_streams.clone(),
        server.cancel.clone(),
    );
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
        let tag = request.tag.as_str();
        if tag == WsOrchestrationV2SubscribeShellRpc::TAG {
            return Some(HandlerOutput::Stream(
                futures::stream::once(async {
                    Ok(vec![
                        json!({"kind":"snapshot","snapshot":shell()}),
                        json!({"kind":"synchronized"}),
                    ])
                })
                .chain(futures::stream::pending())
                .boxed_local(),
            ));
        }
        calls
            .borrow_mut()
            .push((tag.to_owned(), request.payload.clone()));
        match tag {
            "subscribeWorktreeSetup" => {
                let (sender, receiver) = mpsc::unbounded();
                streams.borrow_mut().push(sender);
                Some(HandlerOutput::Stream(
                    receiver.map(|value| Ok(vec![value])).boxed_local(),
                ))
            }
            "worktreeSetup.cancel" => {
                let answer = cancel.borrow().clone();
                Some(HandlerOutput::Unary(
                    async move {
                        match answer {
                            Ok(value) => Exit::success(value),
                            Err(error) => Exit::fail(error),
                        }
                    }
                    .boxed_local(),
                ))
            }
            "assets.createUrl" => Some(HandlerOutput::Unary(
                async {
                    Exit::success(json!({"relativeUrl":"/api/assets/signed?token=abc","expiresAt":4102444800000_i64,
                        "imageDimensions":{"width":64,"height":32}}))
                }
                .boxed_local(),
            )),
            _ => None,
        }
    }));
    let runtime = ClientRuntime::new(
        h.platform.clone(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        h.spawner(),
    );
    h.run(runtime.pair(PairingInput {
        pairing_url: Some(format!("{ORIGIN}/pair#token=pairing-token")),
        ..Default::default()
    }))
    .unwrap();
    h.settle();
    (runtime, server)
}

fn grant(runtime: &ClientRuntime<TestPlatform>, scopes: &[&str]) {
    runtime
        .client(&environment("env"))
        .unwrap()
        .session()
        .unwrap()
        .update_grants(SessionGrants {
            valid: true,
            scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
        });
}

fn snapshot(phase: &str, sequence: u64) -> Value {
    json!({"threadId":"t1","phase":phase,"startedAt":"2026-10-26T12:00:00.000Z","endedAt":null,
        "branch":"t3/feature","baseRef":"main","worktreePath":"/repo/.t3/worktrees/feature","setupScript":null,
        "stages":[{"id":"fetch","status":if phase == "running" {"running"} else {"done"},"startedAt":"2026-10-26T12:00:00.000Z",
            "endedAt":null,"percent":null,"detail":null,"tail":[]}],
        "error":null,"sequence":sequence})
}

fn subscribes(server: &Server, tag: &str) -> usize {
    server
        .calls
        .borrow()
        .iter()
        .filter(|(t, _)| t == tag)
        .count()
}

#[test]
fn setup_follows_only_threads_that_need_it_and_holds_the_newest_snapshot() {
    let mut h = Harness::new("env");
    let (runtime, server) = setup(&mut h);
    let mut idle = runtime.worktree_setup().watch(key("t2"));
    h.settle();
    let first = h.run(async move { (idle.next().await.unwrap(), idle) });
    let (view, idle) = first;
    assert_eq!(view.snapshot, None);
    assert!(!view.preparing);
    assert_eq!(subscribes(&server, "subscribeWorktreeSetup"), 0);
    drop(idle);

    let mut watch = runtime.worktree_setup().watch(key("t1"));
    h.settle();
    let (view, mut watch) = h.run(async move { (watch.next().await.unwrap(), watch) });
    // The run is preparing, so the card is pending before any snapshot arrives.
    assert!(view.preparing);
    assert_eq!(view.snapshot, None);
    assert_eq!(subscribes(&server, "subscribeWorktreeSetup"), 1);
    assert_eq!(
        server
            .calls
            .borrow()
            .iter()
            .find(|(tag, _)| tag == "subscribeWorktreeSetup")
            .unwrap()
            .1,
        json!({"threadId":"t1"}),
        "the subscription names the thread"
    );

    let stream = server.setup_streams.borrow()[0].clone();
    stream.unbounded_send(snapshot("running", 2)).unwrap();
    h.settle();
    let (view, mut watch) = h.run(async move { (watch.next().await.unwrap(), watch) });
    let shown = view.snapshot.clone().unwrap();
    assert_eq!(shown.sequence, 2);
    assert_eq!(view.row_input.as_ref().unwrap()["phase"], "running");

    // An older snapshot and a dropped one keep the newest on screen.
    stream.unbounded_send(snapshot("running", 1)).unwrap();
    stream.unbounded_send(Value::Null).unwrap();
    h.settle();
    assert!(watch.next().now_or_never().is_none(), "nothing changed");

    stream.unbounded_send(snapshot("failed", 3)).unwrap();
    h.settle();
    let (view, watch) = h.run(async move { (watch.next().await.unwrap(), watch) });
    assert_eq!(view.snapshot.unwrap().sequence, 3);
    drop(watch);
    h.settle();
    assert!(
        stream.is_closed(),
        "dropping the watch ends the server subscription"
    );
}

#[test]
fn cancel_answers_the_server_result_keeps_failures_typed_and_skips_finished_setups() {
    let mut h = Harness::new("env");
    let (runtime, server) = setup(&mut h);
    let service = runtime.worktree_setup();
    let running: WorktreeSetupSnapshot = serde_json::from_value(snapshot("running", 1)).unwrap();
    let done: WorktreeSetupSnapshot = serde_json::from_value(snapshot("done", 2)).unwrap();

    assert!(h.run(service.cancel(&key("t1"), &running)).unwrap());
    assert_eq!(
        server
            .calls
            .borrow()
            .iter()
            .find(|(tag, _)| tag == "worktreeSetup.cancel")
            .unwrap()
            .1,
        json!({"threadId":"t1"})
    );
    assert!(!h.run(service.cancel(&key("t1"), &done)).unwrap());
    assert_eq!(subscribes(&server, "worktreeSetup.cancel"), 1);

    *server.cancel.borrow_mut() = Ok(json!({"cancelled":false}));
    assert!(!h.run(service.cancel(&key("t1"), &running)).unwrap());

    // The method has no client-side scope guard; the server's refusal stays typed.
    *server.cancel.borrow_mut() = Err(json!({"_tag":"EnvironmentAuthorizationError",
        "message":"This session cannot change source control.","requiredScope":"source-control:write"}));
    let error = h.run(service.cancel(&key("t1"), &running)).unwrap_err();
    assert!(matches!(error, ClientError::Rejected { .. }), "{error:?}");
    assert!(
        error.to_string().contains("cannot change source control"),
        "{error}"
    );
    assert!(matches!(
        h.run(service.cancel(&key_in("other"), &running)),
        Err(ClientError::Disconnected)
    ));
}

fn key_in(environment_id: &str) -> ThreadKey {
    ThreadKey {
        environment: environment(environment_id),
        thread: support::fake_server::thread("t1"),
    }
}

#[test]
fn setup_terminal_opens_only_for_the_shown_thread() {
    let mut h = Harness::new("env");
    let (runtime, _server) = setup(&mut h);
    let service = runtime.worktree_setup();
    let setup: WorktreeSetupSnapshot = serde_json::from_value(snapshot("running", 1)).unwrap();
    assert!(!service.open_terminal(&key("t2"), &setup, "setup-1"));
    assert!(!runtime.terminals().ui_state(&key("t2")).terminal_open);
    assert!(service.open_terminal(&key("t1"), &setup, "setup-1"));
    let state = runtime.terminals().ui_state(&key("t1"));
    assert!(state.terminal_open);
    assert_eq!(state.active_terminal_id, "setup-1");
}

fn workspace_image() -> AssetResource {
    serde_json::from_value(
        json!({"_tag":"workspace-file","threadId":"t1","path":"/repo/first.png"}),
    )
    .unwrap()
}

#[test]
fn asset_urls_mint_once_per_resource_and_resolve_against_the_connection() {
    let mut h = Harness::new("env");
    let (runtime, server) = setup(&mut h);
    grant(&runtime, &["orchestration:read", "filesystem:read"]);
    let mut first = runtime
        .asset_urls()
        .watch(environment("env"), workspace_image());
    let mut second = runtime
        .asset_urls()
        .watch(environment("env"), workspace_image());
    h.settle();
    let (states, first, second) = h.run(async move {
        let mut states = vec![first.next().await.unwrap()];
        while !matches!(states.last(), Some(AssetUrlState::Success { .. })) {
            states.push(first.next().await.unwrap());
        }
        let mut other = second.next().await.unwrap();
        while !matches!(other, AssetUrlState::Success { .. }) {
            other = second.next().await.unwrap();
        }
        states.push(other);
        (states, first, second)
    });
    let AssetUrlState::Success { url, asset } = states.last().unwrap() else {
        unreachable!()
    };
    assert_eq!(url, &format!("{ORIGIN}/api/assets/signed?token=abc"));
    assert_eq!(
        asset.image_dimensions.clone().flatten().map(|d| d.width),
        Some(64)
    );
    assert_eq!(
        subscribes(&server, "assets.createUrl"),
        1,
        "readers share one mint"
    );
    drop((first, second));
}

#[test]
fn file_assets_without_file_access_never_mint() {
    let mut h = Harness::new("env");
    let (runtime, server) = setup(&mut h);
    grant(&runtime, &["orchestration:read"]);
    let mut watch = runtime
        .asset_urls()
        .watch(environment("env"), workspace_image());
    h.settle();
    let state = h.run(async move { watch.next().await.unwrap() });
    assert_eq!(
        state,
        AssetUrlState::Failure {
            missing_file_access: true
        }
    );
    assert_eq!(subscribes(&server, "assets.createUrl"), 0);
}
