#![allow(clippy::unwrap_used)]
//! The terminal service against the fake server: tabs and their persistence, launching,
//! attaching with replay on reconnect, observing, closing and resizing.
mod support {
    pub mod fake_server;
}
use futures::{FutureExt, StreamExt, channel::mpsc};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
use support::fake_server::*;
use t3_client::{
    ClientError, ClientStore,
    composition::ClientRuntime,
    connection::PairingInput,
    services::{
        terminal::{TerminalGrid, TerminalOutputCursor, TerminalOutputUpdate, TerminalStatus},
        terminal_sessions::{Placement, SessionMode, TerminalLaunch, TerminalSessions},
    },
    types::ThreadKey,
    view_logic::session::terminal_drawer::TerminalLabel,
};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_rpc::{Exit, HandlerOutput};

const OPERATE: &[&str] = &[
    "orchestration:read",
    "orchestration:operate",
    "terminal:operate",
];
const READ: &[&str] = &["orchestration:read", "terminal:read"];

/// The terminal methods the Rust server serves, and a scripted answer for each request.
#[derive(Default)]
struct Terminal {
    requests: RefCell<Vec<(String, Value)>>,
    /// Frames the next `terminal.attach` / `terminal.observe` stream sends.
    attach: RefCell<VecDeque<mpsc::UnboundedReceiver<Value>>>,
    metadata: RefCell<Option<mpsc::UnboundedReceiver<Value>>>,
    failing: RefCell<Vec<&'static str>>,
    held_resizes: RefCell<Vec<futures::channel::oneshot::Sender<()>>>,
    hold_resizes: std::cell::Cell<bool>,
}

fn setup(h: &mut Harness, scopes: &[&str]) -> (ClientRuntime<TestPlatform>, Rc<Terminal>) {
    let terminal = Rc::new(Terminal::default());
    {
        let mut state = h.server().state.borrow_mut();
        state.permissions = Some(scopes.iter().map(|s| (*s).to_owned()).collect());
        state.extra_methods = t3_contracts::rpc::WS_METHODS
            .iter()
            .filter(|m| m.tag.starts_with("terminal.") || m.tag == "subscribeTerminalMetadata")
            .map(|m| {
                (
                    m.tag,
                    if m.streams {
                        t3_rpc::CallKind::Stream
                    } else {
                        t3_rpc::CallKind::Unary
                    },
                )
            })
            .collect();
        let handler = terminal.clone();
        state.extra_rpc = Some(Rc::new(move |request| {
            let tag = request.tag.as_str();
            if !tag.starts_with("terminal.") && tag != "subscribeTerminalMetadata" {
                return None;
            }
            handler
                .requests
                .borrow_mut()
                .push((tag.to_owned(), request.payload.clone()));
            if handler.failing.borrow().contains(&tag) {
                return Some(HandlerOutput::Unary(
                    async {
                        Exit::fail(json!({"_tag":"TerminalError","message":"no such terminal"}))
                    }
                    .boxed_local(),
                ));
            }
            Some(match tag {
                "terminal.attach" | "terminal.observe" => {
                    let frames = handler.attach.borrow_mut().pop_front();
                    HandlerOutput::Stream(match frames {
                        Some(frames) => frames.map(|frame| Ok(vec![frame])).boxed_local(),
                        None => futures::stream::pending().boxed_local(),
                    })
                }
                "subscribeTerminalMetadata" => {
                    let frames = handler.metadata.borrow_mut().take();
                    HandlerOutput::Stream(match frames {
                        Some(frames) => frames.map(|frame| Ok(vec![frame])).boxed_local(),
                        None => futures::stream::pending().boxed_local(),
                    })
                }
                "terminal.open" => {
                    let snapshot = snapshot(request.payload["terminalId"].as_str().unwrap(), "");
                    HandlerOutput::Unary(async move { Exit::success(snapshot) }.boxed_local())
                }
                "terminal.resize" if handler.hold_resizes.get() => {
                    let (send, receive) = futures::channel::oneshot::channel();
                    handler.held_resizes.borrow_mut().push(send);
                    HandlerOutput::Unary(
                        async move {
                            let _ = receive.await;
                            Exit::success(Value::Null)
                        }
                        .boxed_local(),
                    )
                }
                _ => HandlerOutput::Unary(async { Exit::success(Value::Null) }.boxed_local()),
            })
        }));
    }
    let runtime = ClientRuntime::new(
        h.platform.clone(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        h.spawner(),
    );
    h.run(runtime.pair(PairingInput {
        pairing_url: Some(format!("{ORIGIN}/pair#token=pairing-token")),
        ..PairingInput::default()
    }))
    .unwrap();
    h.settle();
    // Grants as the session states them; operate implies read, as the server's grants do.
    runtime
        .client(&environment("env"))
        .unwrap()
        .session()
        .unwrap()
        .update_grants(t3_client::permissions::SessionGrants {
            valid: true,
            scopes: scopes
                .iter()
                .map(|s| (*s).to_owned())
                .chain(
                    scopes
                        .contains(&"terminal:operate")
                        .then(|| "terminal:read".to_owned()),
                )
                .collect(),
        });
    (runtime, terminal)
}

fn key() -> ThreadKey {
    ThreadKey {
        environment: environment("env"),
        thread: thread("t1"),
    }
}

fn snapshot(terminal: &str, history: &str) -> Value {
    json!({"threadId":"t1","terminalId":terminal,"cwd":"/workspace/project","worktreePath":null,
        "status":"running","pid":1,"history":history,"exitCode":null,"exitSignal":null,
        "label":"","updatedAt":NOW})
}

fn summary(terminal: &str, label: &str) -> Value {
    json!({"threadId":"t1","terminalId":terminal,"cwd":"/workspace/project/sub","worktreePath":null,
        "status":"running","pid":1,"exitCode":null,"exitSignal":null,"label":label,
        "updatedAt":NOW,"hasRunningSubprocess":false})
}

fn stream(terminal: &Terminal) -> mpsc::UnboundedSender<Value> {
    let (send, receive) = mpsc::unbounded();
    terminal.attach.borrow_mut().push_back(receive);
    send
}

fn metadata(terminal: &Terminal) -> mpsc::UnboundedSender<Value> {
    let (send, receive) = mpsc::unbounded();
    *terminal.metadata.borrow_mut() = Some(receive);
    send
}

fn requests(terminal: &Terminal, tag: &str) -> Vec<Value> {
    terminal
        .requests
        .borrow()
        .iter()
        .filter(|(t, _)| t == tag)
        .map(|(_, payload)| payload.clone())
        .collect()
}

fn launch() -> TerminalLaunch {
    TerminalLaunch {
        cwd: "/workspace/project".into(),
        worktree_path: None,
        env: [(
            "T3CODE_PROJECT_ROOT".to_owned(),
            "/workspace/project".to_owned(),
        )]
        .into(),
    }
}

fn read_all(
    lease: &t3_client::services::terminal_sessions::TerminalSessionLease,
    cursor: &mut TerminalOutputCursor,
) -> TerminalOutputUpdate {
    let update = lease.with_output(|output| output.read(*cursor));
    *cursor = update.cursor();
    update
}

#[test]
fn tabs_persist_and_a_new_runtime_restores_them() {
    let mut h = Harness::new("env");
    let (runtime, _) = setup(&mut h, OPERATE);
    let terminals = runtime.terminals();
    h.run(terminals.load()).unwrap();
    terminals.set_open(&key(), true);
    terminals.place(&key(), "term-2", Placement::SplitVertical);
    terminals.set_height(&key(), 333.0);
    h.settle();
    let state = terminals.ui_state(&key());
    assert_eq!(state.terminal_ids, ["term-1", "term-2"]);
    assert_eq!(state.active_terminal_id, "term-2");
    assert_eq!(state.terminal_groups.len(), 1);
    assert_eq!(
        state.terminal_groups[0].split_direction.as_deref(),
        Some("vertical")
    );
    assert_eq!(terminals.open_threads(), [key()]);

    let restored = ClientRuntime::new(
        h.platform.clone(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        h.spawner(),
    )
    .terminals();
    h.run(restored.load()).unwrap();
    assert_eq!(restored.ui_state(&key()), state);
    assert_eq!(restored.ui_state(&key()).terminal_height, 333.0);

    // Closing the last tab resets the thread to the default and stops persisting it.
    terminals.close_tab(&key(), "term-1");
    terminals.close_tab(&key(), "term-2");
    h.settle();
    assert!(terminals.open_threads().is_empty());
    let again = ClientRuntime::new(
        h.platform.clone(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        h.spawner(),
    )
    .terminals();
    h.run(again.load()).unwrap();
    assert!(again.ui_state(&key()).terminal_ids.is_empty());
}

#[test]
fn launching_places_the_tab_then_opens_the_shell_in_the_project() {
    let mut h = Harness::new("env");
    let (runtime, terminal) = setup(&mut h, OPERATE);
    let terminals = runtime.terminals();
    let send = metadata(&terminal);
    let drawer = terminals.drawer(&key(), None);
    h.settle();
    send.unbounded_send(json!({"type":"snapshot","terminals":[summary("term-1","")]}))
        .unwrap();
    h.settle();
    // The server's session becomes the thread's tab.
    assert_eq!(drawer.view().terminal_ids, ["term-1"]);

    let (id, opened) = terminals
        .launch_drawer_terminal(&key(), Placement::Split, None)
        .unwrap();
    assert_eq!(id, "term-2", "allocated against the known sessions");
    // Placed before the open resolves.
    assert_eq!(
        terminals.ui_state(&key()).terminal_ids,
        ["term-1", "term-2"]
    );
    h.run(opened).unwrap();
    let open = requests(&terminal, "terminal.open");
    assert_eq!(open.len(), 1);
    assert_eq!(open[0]["terminalId"], "term-2");
    assert_eq!(open[0]["cwd"], "/workspace/project");
    assert_eq!(
        open[0]["env"],
        json!({"T3CODE_PROJECT_ROOT":"/workspace/project"})
    );
    assert!(open[0].get("worktreePath").is_none());

    // The server still lags the new tab: the subset does not discard it.
    send.unbounded_send(json!({"type":"upsert","terminal":summary("term-1","dev server")}))
        .unwrap();
    h.settle();
    let view = drawer.view();
    assert_eq!(view.terminal_ids, ["term-1", "term-2"]);
    assert_eq!(
        view.label("term-1"),
        TerminalLabel::Text("dev server".into())
    );
    assert_eq!(view.label("term-2"), TerminalLabel::Numbered("2".into()));
    assert_eq!(view.launch("term-1").unwrap().cwd, "/workspace/project/sub");
    assert_eq!(view.launch("term-2").unwrap().cwd, "/workspace/project");
}

#[test]
fn without_operate_nothing_launches_or_closes_and_the_server_order_shows() {
    let mut h = Harness::new("env");
    let (runtime, terminal) = setup(&mut h, READ);
    let terminals = runtime.terminals();
    let send = metadata(&terminal);
    let drawer = terminals.drawer(&key(), None);
    h.settle();
    send.unbounded_send(json!({"type":"snapshot","terminals":[summary("term-2",""),summary("term-10",""),summary("term-1","")]}))
        .unwrap();
    h.settle();
    assert!(!terminals.can_operate(&environment("env")));
    assert!(terminals.can_read(&environment("env")));
    assert!(
        terminals
            .launch_drawer_terminal(&key(), Placement::New, None)
            .is_none()
    );
    assert!(terminals.close_drawer_terminal(&key(), "term-1").is_none());
    let view = drawer.view();
    assert!(!view.can_operate);
    assert_eq!(view.terminal_ids, ["term-1", "term-2", "term-10"]);
    h.settle();
    assert!(requests(&terminal, "terminal.open").is_empty());
    assert!(requests(&terminal, "terminal.close").is_empty());
}

#[test]
fn attach_replays_output_and_a_reconnect_resumes_without_blanking_it() {
    let mut h = Harness::new("env");
    let (runtime, terminal) = setup(&mut h, OPERATE);
    let terminals = runtime.terminals();
    let first = stream(&terminal);
    let lease = terminals.session(
        &environment("env"),
        TerminalSessions::<TestPlatform>::attach_input(&key(), "term-1", &launch()),
    );
    let mut changes = lease.changes();
    h.settle();
    assert_eq!(lease.view().mode, SessionMode::Attached);
    let attach = requests(&terminal, "terminal.attach");
    assert_eq!(attach.len(), 1);
    assert!(
        attach[0].get("resume").is_none(),
        "the first attach opens the shell"
    );
    assert_eq!(
        attach[0]["maxHistoryBytes"],
        t3_client::services::terminal::DEFAULT_MAX_TERMINAL_BUFFER_BYTES as u64
    );
    assert_eq!(attach[0]["cwd"], "/workspace/project");

    first
        .unbounded_send(json!({"type":"snapshot","snapshot":snapshot("term-1","host output")}))
        .unwrap();
    h.settle();
    assert!(futures::executor::block_on(changes.next()).is_some());
    let view = lease.view();
    assert_eq!(view.status, TerminalStatus::Running);
    let mut cursor = TerminalOutputCursor::default();
    assert!(
        matches!(read_all(&lease, &mut cursor), TerminalOutputUpdate::Reset { data, .. } if data == "host output")
    );
    for chunk in ["a", "b"] {
        first
            .unbounded_send(
                json!({"type":"output","threadId":"t1","terminalId":"term-1","data":chunk}),
            )
            .unwrap();
    }
    h.settle();
    assert!(
        matches!(read_all(&lease, &mut cursor), TerminalOutputUpdate::Append { data, .. } if data == "ab")
    );

    // The connection drops: the shown output stays until the new snapshot replays it.
    let second = stream(&terminal);
    h.server().hang_up();
    h.settle();
    h.advance(10_000);
    let attach = requests(&terminal, "terminal.attach");
    assert_eq!(attach.len(), 2);
    assert_eq!(attach[1]["resume"], true);
    assert!(matches!(
        read_all(&lease, &mut cursor),
        TerminalOutputUpdate::None(_)
    ));
    second
        .unbounded_send(
            json!({"type":"snapshot","snapshot":snapshot("term-1","host outputab more")}),
        )
        .unwrap();
    h.settle();
    assert!(lease.view().generation > view.generation);
    assert!(
        matches!(read_all(&lease, &mut cursor), TerminalOutputUpdate::Reset { data, .. } if data == "host outputab more")
    );

    // Exit is reported through the status.
    second
        .unbounded_send(json!({"type":"exited","threadId":"t1","terminalId":"term-1","exitCode":0,"exitSignal":null}))
        .unwrap();
    h.settle();
    assert_eq!(lease.view().status, TerminalStatus::Exited);

    // Readers of one launch share one subscription; the last one leaving ends it.
    let shared = terminals.session(
        &environment("env"),
        TerminalSessions::<TestPlatform>::attach_input(&key(), "term-1", &launch()),
    );
    assert_eq!(shared.view().version, lease.view().version);
    drop((lease, shared, changes));
    h.settle();
    assert_eq!(requests(&terminal, "terminal.attach").len(), 2);
}

#[test]
fn a_read_only_viewer_observes_and_a_failure_shows_as_an_error() {
    let mut h = Harness::new("env");
    let (runtime, terminal) = setup(&mut h, READ);
    let terminals = runtime.terminals();
    let frames = stream(&terminal);
    let lease = terminals.session(
        &environment("env"),
        TerminalSessions::<TestPlatform>::attach_input(&key(), "term-1", &launch()),
    );
    h.settle();
    assert_eq!(lease.view().mode, SessionMode::Observing);
    assert!(requests(&terminal, "terminal.attach").is_empty());
    assert_eq!(
        requests(&terminal, "terminal.observe"),
        [json!({"threadId":"t1","terminalId":"term-1"})]
    );
    frames
        .unbounded_send(json!({"type":"error","threadId":"t1","terminalId":"term-1","message":"Shell failed to start"}))
        .unwrap();
    h.settle();
    let view = lease.view();
    assert_eq!(view.status, TerminalStatus::Error);
    assert_eq!(view.error.as_deref(), Some("Shell failed to start"));
    // Writing is refused before anything is sent.
    let refused = h.run(terminals.write(&key(), "term-1", "ls\r".into()));
    assert!(
        matches!(refused, Err(ClientError::PermissionDenied { .. })),
        "{refused:?}"
    );
    assert!(requests(&terminal, "terminal.write").is_empty());
}

#[test]
fn closing_a_tab_ends_its_session_and_a_failed_close_types_exit() {
    let mut h = Harness::new("env");
    let (runtime, terminal) = setup(&mut h, OPERATE);
    let terminals = runtime.terminals();
    h.run(terminals.load()).unwrap();
    terminals.set_open(&key(), true);
    terminals.place(&key(), "term-2", Placement::New);
    let closing = terminals.close_drawer_terminal(&key(), "term-1").unwrap();
    assert_eq!(terminals.ui_state(&key()).terminal_ids, ["term-2"]);
    h.run(closing);
    assert_eq!(
        requests(&terminal, "terminal.close"),
        [json!({"threadId":"t1","terminalId":"term-1","deleteHistory":true})]
    );
    assert!(requests(&terminal, "terminal.write").is_empty());

    // Stale metadata does not bring the closed tab back.
    terminals.reconcile(&key(), &["term-1".into(), "term-2".into()]);
    assert_eq!(terminals.ui_state(&key()).terminal_ids, ["term-2"]);

    terminal.failing.borrow_mut().push("terminal.close");
    let closing = terminals.close_drawer_terminal(&key(), "term-2").unwrap();
    h.run(closing);
    assert_eq!(
        requests(&terminal, "terminal.write"),
        [json!({"threadId":"t1","terminalId":"term-2","data":"exit\n"})]
    );
    assert!(terminals.ui_state(&key()).terminal_ids.is_empty());
}

#[test]
fn resizes_send_only_the_latest_grid_after_the_one_in_flight() {
    let mut h = Harness::new("env");
    let (runtime, terminal) = setup(&mut h, OPERATE);
    let terminals = runtime.terminals();
    terminal.hold_resizes.set(true);
    let grid = |cols, rows| TerminalGrid { cols, rows };
    let first = terminals.resize(&key(), "term-1", grid(80, 24));
    let done = Rc::new(RefCell::new(None));
    let slot = done.clone();
    h.spawn(async move {
        *slot.borrow_mut() = Some(first.await);
    });
    h.settle();
    h.run(terminals.resize(&key(), "term-1", grid(100, 30)))
        .unwrap();
    h.run(terminals.resize(&key(), "term-1", grid(120, 40)))
        .unwrap();
    // Another terminal is not held behind this one.
    let other = terminals.resize(&key(), "term-2", grid(90, 20));
    h.spawn(async move {
        let _ = other.await;
    });
    h.settle();
    assert_eq!(requests(&terminal, "terminal.resize").len(), 2);
    let held: Vec<_> = terminal.held_resizes.borrow_mut().drain(..).collect();
    for sender in held {
        let _ = sender.send(());
    }
    h.settle();
    let resizes = requests(&terminal, "terminal.resize");
    assert_eq!(resizes.len(), 3, "{resizes:?}");
    assert_eq!(
        resizes[2],
        json!({"threadId":"t1","terminalId":"term-1","cols":120,"rows":40})
    );
    for sender in terminal.held_resizes.borrow_mut().drain(..) {
        let _ = sender.send(());
    }
    h.settle();
    assert!(matches!(*done.borrow(), Some(Ok(()))));
}

#[test]
fn an_unserved_terminal_method_fails_as_unsupported_rather_than_succeeding() {
    let mut h = Harness::new("env");
    let (runtime, _terminal) = setup(&mut h, OPERATE);
    h.server().state.borrow_mut().extra_rpc = None;
    h.server().state.borrow_mut().extra_methods.clear();
    let terminals = runtime.terminals();
    // A fresh socket so the server serves only its built-in methods.
    h.server().hang_up();
    h.settle();
    h.advance(10_000);
    let result = h.run(terminals.clear(&key(), "term-1"));
    assert!(result.is_err(), "{result:?}");
}
