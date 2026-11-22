//! GUEST-1: the guest page through the production app at desktop and phone widths, over a
//! scripted transport: join, read the live thread, send for approval, and revocation.
#![allow(clippy::unwrap_used)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use futures::{StreamExt, channel::mpsc, channel::oneshot, future::LocalBoxFuture};
use serde_json::{Value, json};
use t3_client::guest::transport::{
    GuestError, GuestFailure, GuestFuture, GuestStreamHandle, GuestTransport,
};
use t3_contracts::orchestration_v2::OrchestrationV2ThreadHistoryPage;
use t3_ui::screens::guest::install_transport_for;

use crate::support::{DESKTOP, PHONE, start_on_web};

const NOW: &str = "2026-06-20T00:00:00.000Z";

type Frames = mpsc::UnboundedSender<Value>;
type HistoryGate = oneshot::Sender<Result<OrchestrationV2ThreadHistoryPage, GuestError>>;
type CommandReceiverQueue =
    Arc<Mutex<VecDeque<Result<mpsc::UnboundedReceiver<Value>, GuestError>>>>;

#[derive(Clone, Default)]
struct Probe {
    calls: Arc<Mutex<Vec<String>>>,
    frames: Arc<Mutex<Option<Frames>>>,
    history_gates: Arc<Mutex<VecDeque<HistoryGate>>>,
    command_pending: CommandReceiverQueue,
}

impl Probe {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn push(&self, frame: Value) {
        let frames = self.frames.lock().unwrap();
        frames
            .as_ref()
            .expect("the stream is open")
            .unbounded_send(frame)
            .unwrap();
    }
    fn queue_command_output(&self) -> Frames {
        let (tx, rx) = mpsc::unbounded();
        self.command_pending.lock().unwrap().push_back(Ok(rx));
        tx
    }
    fn queue_command_output_error(&self, error: GuestError) {
        self.command_pending.lock().unwrap().push_back(Err(error));
    }
    fn resolve_history(&self, reply: Result<OrchestrationV2ThreadHistoryPage, GuestError>) {
        let gate = self
            .history_gates
            .lock()
            .unwrap()
            .pop_front()
            .expect("pending history request");
        gate.send(reply).unwrap();
    }
}

struct Wire {
    probe: Probe,
    stream: std::cell::RefCell<Option<mpsc::UnboundedReceiver<Value>>>,
    session_ok: bool,
}

struct Frames_(mpsc::UnboundedReceiver<Value>);

impl GuestStreamHandle for Frames_ {
    fn next(&mut self) -> GuestFuture<'_, Option<Value>> {
        Box::pin(async move { Ok(self.0.next().await) })
    }
}

impl GuestTransport for Wire {
    fn redeem(&self, _credential: &str, name: &str) -> GuestFuture<'_, ()> {
        self.probe
            .calls
            .lock()
            .unwrap()
            .push(format!("redeem {name}"));
        Box::pin(async { Ok(()) })
    }
    fn session(&self, share: &str) -> GuestFuture<'_, ()> {
        self.probe
            .calls
            .lock()
            .unwrap()
            .push(format!("session {share}"));
        let ok = self.session_ok;
        Box::pin(async move {
            if ok {
                Ok(())
            } else {
                Err(GuestError::Rejected {
                    status: 403,
                    reason: GuestFailure::Invalid,
                    message: String::new(),
                })
            }
        })
    }
    fn open_stream(&self, share: &str) -> GuestFuture<'_, Box<dyn GuestStreamHandle>> {
        self.probe
            .calls
            .lock()
            .unwrap()
            .push(format!("stream {share}"));
        let receiver = self.stream.borrow_mut().take();
        Box::pin(async move {
            match receiver {
                Some(receiver) => Ok(Box::new(Frames_(receiver)) as Box<dyn GuestStreamHandle>),
                None => Err(GuestError::Network("one stream per page".into())),
            }
        })
    }
    fn history(
        &self,
        share: &str,
        cursor: &str,
    ) -> GuestFuture<'_, OrchestrationV2ThreadHistoryPage> {
        self.probe
            .calls
            .lock()
            .unwrap()
            .push(format!("history {share} {cursor}"));
        let (tx, rx) = oneshot::channel();
        self.probe.history_gates.lock().unwrap().push_back(tx);
        Box::pin(async move {
            rx.await
                .unwrap_or_else(|_| Err(GuestError::Network("history gate dropped".into())))
        })
    }
    fn command_output(
        &self,
        share: &str,
        item: &str,
    ) -> GuestFuture<'_, Box<dyn GuestStreamHandle>> {
        self.probe
            .calls
            .lock()
            .unwrap()
            .push(format!("command-output {share} {item}"));
        let next = self.probe.command_pending.lock().unwrap().pop_front();
        Box::pin(async move {
            match next {
                Some(Ok(rx)) => Ok(Box::new(Frames_(rx)) as Box<dyn GuestStreamHandle>),
                Some(Err(err)) => Err(err),
                None => Err(GuestError::Rejected {
                    status: 403,
                    reason: GuestFailure::Unavailable,
                    message: "This item is not available.".into(),
                }),
            }
        })
    }
    fn submit(&self, share: &str, text: &str) -> GuestFuture<'_, ()> {
        self.probe
            .calls
            .lock()
            .unwrap()
            .push(format!("submit {share} {text}"));
        Box::pin(async { Ok(()) })
    }
    fn sleep(&self, _millis: u64) -> LocalBoxFuture<'_, ()> {
        Box::pin(std::future::pending())
    }
}

fn install(share: &str, session_ok: bool) -> Probe {
    let probe = Probe::default();
    let seen = probe.clone();
    install_transport_for(
        share,
        Arc::new(move || {
            let (sender, receiver) = mpsc::unbounded();
            *seen.frames.lock().unwrap() = Some(sender);
            Rc_::new(Wire {
                probe: seen.clone(),
                stream: std::cell::RefCell::new(Some(receiver)),
                session_ok,
            })
        }),
    );
    probe
}

type Rc_ = std::rc::Rc<Wire>;

fn view_frame(pending: &[&str]) -> Value {
    let messages: Vec<Value> = pending
        .iter()
        .enumerate()
        .map(|(n, text)| json!({"id":format!("m{n}"),"text":text,"status":"pending","submittedAt":NOW}))
        .collect();
    json!({"type":"view","view":{"shareId":"x","title":"Shared plan","guestName":"Sam",
        "expiresAt":"2026-06-21T00:00:00.000Z","messages":messages,
        "model":{"driver":"codex","name":"GPT"}}})
}

fn thread(id: &str) -> Value {
    json!({"id":id,"projectId":"project","title":"Shared plan","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":id,"parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":NOW,"updatedAt":NOW,"archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null})
}

fn snapshot(id: &str, text: &str) -> Value {
    let item = json!({"id":"a1","type":"assistant_message","threadId":id,"runId":null,"nodeId":null,
        "providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,
        "ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,
        "updatedAt":NOW,"messageId":"msg-a1","phase":"final_answer","streaming":false,
        "text":text,"attachments":[]});
    json!({"type":"thread","item":{"kind":"snapshot","snapshotSequence":1,"projection":{
        "thread":thread(id),"runs":[],"attempts":[],"nodes":[],"subagents":[],
        "providerSessions":[],"providerThreads":[],"providerTurns":[],"runtimeRequests":[],
        "messages":[],"plans":[],"turnItems":[item],"checkpointScopes":[],"checkpoints":[],
        "contextHandoffs":[],"contextTransfers":[],"visibleTurnItems":[],"updatedAt":NOW}}})
}

fn stream_open(app: &mut nsh_testing::Headless, probe: &Probe) {
    let probe = probe.clone();
    app.wait_until(
        "the stream opening",
        std::time::Duration::from_secs(5),
        move |_| probe.calls().iter().any(|call| call.starts_with("stream")),
    )
    .expect("the page opens its stream");
}

fn join_and_read(window: (f32, f32), share: &str) {
    // No cookie yet: the check that precedes the form is refused.
    let probe = install(share, false);
    let mut app = start_on_web(
        window,
        &format!("https://guest.example/guest#{share}.signature"),
    );
    app.wait_for("Join a shared thread").expect("the name form");
    assert!(!app.screen().shows("Connect to T3 Code"));
    assert_eq!(
        app.browser().url(),
        Some(format!("https://guest.example/guest?share={share}").as_str()),
        "the invite left the address"
    );
    app.type_text("label:Your name", "Sam").expect("name");
    app.press("role:button[Join]").expect("join");
    stream_open(&mut app, &probe);
    assert_eq!(
        probe.calls()[..2],
        ["session ".to_owned() + share, "redeem Sam".into()]
    );
    probe.push(view_frame(&[]));
    probe.push(snapshot("thread:shared", "The plan has three steps."));
    app.wait_for("The plan has three steps.")
        .expect("the live thread");
    assert!(app.screen().shows("Shared plan"));
    assert!(!app.screen().shows("Connect to T3 Code"));

    app.type_text("label:Message", "Looks good")
        .expect("compose");
    app.press("role:button[Send]").expect("send");
    probe.push(view_frame(&["Looks good"]));
    app.wait_for("Waiting for approval").expect("pending row");
    assert!(
        probe
            .calls()
            .iter()
            .any(|call| call == &format!("submit {share} Looks good"))
    );

    probe.push(json!({"type":"ended"}));
    app.wait_for("This link has ended").expect("revoked");
    assert!(!app.screen().shows("The plan has three steps."));
}

/// A browser that already joined opens its spent link and goes straight back in.
fn spent_link(window: (f32, f32), share: &str) {
    let probe = install(share, true);
    let mut app = start_on_web(
        window,
        &format!("https://guest.example/guest#{share}.signature"),
    );
    stream_open(&mut app, &probe);
    probe.push(view_frame(&[]));
    probe.push(snapshot("thread:shared", "Still here."));
    app.wait_for("Still here.").expect("back in the thread");
    assert!(!app.screen().shows("Join a shared thread"));
    assert!(probe.calls().iter().all(|call| !call.starts_with("redeem")));
}

#[test]
fn a_spent_link_goes_straight_back_in_at_both_widths() {
    spent_link(DESKTOP, "guest_spent_desktop");
    spent_link(PHONE, "guest_spent_phone");
}

#[test]
fn join_read_send_and_revocation_on_desktop() {
    join_and_read(DESKTOP, "guest_desktop");
}

#[test]
fn join_read_send_and_revocation_on_phone() {
    join_and_read(PHONE, "guest_phone");
}

fn returning(window: (f32, f32), share: &str, ok: bool) {
    let probe = install(share, ok);
    let mut app = start_on_web(
        window,
        &format!("https://guest.example/guest?share={share}"),
    );
    if ok {
        stream_open(&mut app, &probe);
        probe.push(view_frame(&[]));
        probe.push(snapshot("thread:shared", "Welcome back."));
        app.wait_for("Welcome back.")
            .expect("returning guest reads");
        assert!(!app.screen().shows("Join a shared thread"));
    } else {
        app.wait_for("This link has ended or is not valid")
            .expect("invalid link");
    }
}

#[test]
fn returning_guest_on_desktop_and_phone() {
    returning(DESKTOP, "guest_return_d", true);
    returning(PHONE, "guest_return_p", true);
}

#[test]
fn a_refused_cookie_shows_the_invalid_link_notice_at_both_widths() {
    returning(DESKTOP, "guest_refused_d", false);
    returning(PHONE, "guest_refused_p", false);
}

fn assistant_row(thread_id: &str, id: &str, ordinal: u64, text: &str) -> Value {
    json!({
        "position": 0,
        "visibility": "local",
        "sourceThreadId": thread_id,
        "sourceItemId": id,
        "item": {
            "id": id,
            "type": "assistant_message",
            "threadId": thread_id,
            "runId": null,
            "nodeId": null,
            "providerThreadId": null,
            "providerTurnId": null,
            "nativeItemRef": null,
            "parentItemId": null,
            "ordinal": ordinal,
            "status": "completed",
            "title": null,
            "startedAt": null,
            "completedAt": null,
            "updatedAt": NOW,
            "messageId": format!("msg-{id}"),
            "phase": "final_answer",
            "streaming": false,
            "text": text,
            "attachments": []
        }
    })
}

fn command_row(
    thread_id: &str,
    id: &str,
    ordinal: u64,
    input: &str,
    exit_code: Option<i64>,
) -> Value {
    json!({
        "position": 0,
        "visibility": "local",
        "sourceThreadId": thread_id,
        "sourceItemId": id,
        "item": {
            "id": id,
            "type": "command_execution",
            "threadId": thread_id,
            "runId": null,
            "nodeId": null,
            "providerThreadId": null,
            "providerTurnId": null,
            "nativeItemRef": null,
            "parentItemId": null,
            "ordinal": ordinal,
            "status": "completed",
            "title": null,
            "startedAt": null,
            "completedAt": null,
            "updatedAt": NOW,
            "input": input,
            "outputOmitted": true,
            "exitCode": exit_code
        }
    })
}

fn history_paging(window: (f32, f32), share: &str) {
    let probe = install(share, true);
    let mut app = start_on_web(
        window,
        &format!("https://guest.example/guest?share={share}"),
    );
    stream_open(&mut app, &probe);
    probe.push(view_frame(&[]));
    probe.push(json!({
        "type": "thread",
        "item": {
            "kind": "snapshot",
            "snapshotSequence": 1,
            "historyCursor": "c1",
            "hasMoreHistory": true,
            "projection": {
                "thread": thread("thread:shared"),
                "runs": [], "attempts": [], "nodes": [], "subagents": [],
                "providerSessions": [], "providerThreads": [], "providerTurns": [],
                "runtimeRequests": [], "messages": [], "plans": [],
                "turnItems": [assistant_row("thread:shared", "a2", 2, "Latest turn")["item"]],
                "checkpointScopes": [], "checkpoints": [], "contextHandoffs": [],
                "contextTransfers": [],
                "visibleTurnItems": [assistant_row("thread:shared", "a2", 2, "Latest turn")],
                "updatedAt": NOW
            }
        }
    }));
    app.wait_for("Latest turn").expect("latest turn");
    app.wait_for("role:button[Load earlier turns]")
        .expect("load earlier button");

    app.press("role:button[Load earlier turns]")
        .expect("press load earlier");
    app.wait_for("Loading earlier turns…")
        .expect("loading indicator");

    probe.resolve_history(Err(GuestError::Network("offline".into())));
    app.wait_for("Could not reach the server.")
        .expect("failed history error");
    app.wait_for("Load earlier turns")
        .expect("button re-enabled after failure");

    app.press("role:button[Load earlier turns]")
        .expect("retry load earlier");
    app.wait_for("Loading earlier turns…")
        .expect("loading indicator on retry");
    let page: OrchestrationV2ThreadHistoryPage = serde_json::from_value(json!({
        "snapshotSequence": 1,
        "items": [assistant_row("thread:shared", "a1", 1, "Earlier turn")],
        "nextCursor": null,
        "hasMoreHistory": false
    }))
    .unwrap();
    probe.resolve_history(Ok(page));

    app.wait_for("Earlier turn").expect("earlier turn merged");
    assert!(app.screen().shows("Latest turn"));
    assert!(!app.screen().shows("Could not reach the server."));
    assert!(!app.screen().shows("Load earlier turns"));
}

#[test]
fn guest_history_paging_loading_failed_and_end_of_history_at_both_widths() {
    history_paging(DESKTOP, "guest_history_d");
    history_paging(PHONE, "guest_history_p");
}

fn command_rows(window: (f32, f32), share: &str) {
    let probe = install(share, true);
    let ok_tx = probe.queue_command_output();
    probe.queue_command_output_error(GuestError::Rejected {
        status: 403,
        reason: GuestFailure::Unavailable,
        message: "This item is not available.".into(),
    });
    let mut app = start_on_web(
        window,
        &format!("https://guest.example/guest?share={share}"),
    );
    stream_open(&mut app, &probe);
    probe.push(view_frame(&[]));
    probe.push(json!({
        "type": "thread",
        "item": {
            "kind": "snapshot",
            "snapshotSequence": 1,
            "projection": {
                "thread": thread("thread:shared"),
                "runs": [], "attempts": [], "nodes": [], "subagents": [],
                "providerSessions": [], "providerThreads": [], "providerTurns": [],
                "runtimeRequests": [], "messages": [], "plans": [],
                "turnItems": [
                    command_row("thread:shared", "cmd-1", 1, "vp run check:changed", Some(0))["item"],
                    command_row("thread:shared", "cmd-err", 2, "cargo test", Some(1))["item"],
                    assistant_row("thread:shared", "a1", 3, "All checks pass.")["item"]
                ],
                "checkpointScopes": [], "checkpoints": [], "contextHandoffs": [],
                "contextTransfers": [],
                "visibleTurnItems": [
                    command_row("thread:shared", "cmd-1", 1, "vp run check:changed", Some(0)),
                    command_row("thread:shared", "cmd-err", 2, "cargo test", Some(1)),
                    assistant_row("thread:shared", "a1", 3, "All checks pass.")
                ],
                "updatedAt": NOW
            }
        }
    }));
    app.wait_for("All checks pass.").expect("reply");
    app.wait_for("vp run check:changed").expect("command row");
    assert!(
        probe
            .calls()
            .iter()
            .all(|call| !call.starts_with("command-output")),
        "command output must not stream while collapsed"
    );

    app.press("role:button[vp run check:changed]")
        .expect("expand command");
    app.wait_for("Loading output…").expect("loading output");
    ok_tx
        .unbounded_send(
            json!({"kind":"replace","text":"checking...\n","truncated":false,"running":true}),
        )
        .unwrap();
    app.wait_for("checking...").expect("streaming output");
    ok_tx
        .unbounded_send(
            json!({"kind":"append","text":"4 files checked\n","truncated":false,"running":false}),
        )
        .unwrap();
    app.wait_for("text:/4 files checked/")
        .expect("settled output");

    // Collapse hides the output again.
    app.press("role:button[vp run check:changed]")
        .expect("collapse command");
    app.wait_until(
        "collapsed output hides",
        std::time::Duration::from_secs(5),
        |screen| !screen.shows("text:/4 files checked/"),
    )
    .expect("collapsed");

    // Unavailable output and non-zero exit code surface on expand.
    app.press("role:button[cargo test]")
        .expect("expand failing command");
    app.wait_for("Output is no longer available.")
        .expect("error state");
    app.wait_for("exit 1").expect("non-zero exit code");
}

#[test]
fn guest_command_output_row_streams_only_when_expanded_at_both_widths() {
    command_rows(DESKTOP, "guest_cmd_d");
    command_rows(PHONE, "guest_cmd_p");
}
