//! GUEST-1: the guest page through the production app at desktop and phone widths, over a
//! scripted transport: join, read the live thread, send for approval, and revocation.
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use futures::{StreamExt, channel::mpsc, future::LocalBoxFuture};
use serde_json::{Value, json};
use t3_client::guest::transport::{
    GuestError, GuestFailure, GuestFuture, GuestStreamHandle, GuestTransport,
};
use t3_ui::screens::guest::install_transport_for;

use crate::support::{DESKTOP, PHONE, start_on_web};

const NOW: &str = "2026-06-20T00:00:00.000Z";

type Frames = mpsc::UnboundedSender<Value>;

#[derive(Clone, Default)]
struct Probe {
    calls: Arc<Mutex<Vec<String>>>,
    frames: Arc<Mutex<Option<Frames>>>,
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
    let probe = install(share, true);
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
    assert_eq!(probe.calls()[0], "redeem Sam");
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
