//! The guest client over a scripted transport: redemption, returning sessions, one isolated
//! thread, reconnects, revocation and the composer's limits.
#![allow(clippy::unwrap_used)]

use futures::{FutureExt, executor::block_on, future::LocalBoxFuture};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
use t3_client::guest::{
    session::{GuestClient, GuestCredential, GuestEnd, GuestPhase, SubmitRefusal},
    transport::{GuestError, GuestFailure, GuestFuture, GuestStreamHandle, GuestTransport},
};

const NOW: &str = "2026-06-20T00:00:00.000Z";

enum Step {
    Frame(Value),
    Fail(GuestError),
    Close,
    /// The stream stays open and silent.
    Hang,
}

type Script = VecDeque<Step>;

#[derive(Default)]
struct Wire {
    redeem: RefCell<VecDeque<Result<(), GuestError>>>,
    session: RefCell<VecDeque<Result<(), GuestError>>>,
    streams: RefCell<VecDeque<Result<Script, GuestError>>>,
    submit: RefCell<VecDeque<Result<(), GuestError>>>,
    calls: RefCell<Vec<String>>,
    sleeps: RefCell<Vec<u64>>,
}

struct Fake(Rc<Wire>);

struct Stream(Script);

impl GuestStreamHandle for Stream {
    fn next(&mut self) -> GuestFuture<'_, Option<Value>> {
        let step = self.0.pop_front();
        Box::pin(async move {
            match step {
                Some(Step::Frame(frame)) => Ok(Some(frame)),
                Some(Step::Fail(error)) => Err(error),
                Some(Step::Hang) => std::future::pending().await,
                Some(Step::Close) | None => Ok(None),
            }
        })
    }
}

impl GuestTransport for Fake {
    fn redeem(&self, credential: &str, name: &str) -> GuestFuture<'_, ()> {
        self.0.calls.borrow_mut().push(format!("redeem {name}"));
        assert_eq!(credential, "share_1.signature");
        let answer = self.0.redeem.borrow_mut().pop_front().unwrap_or(Ok(()));
        Box::pin(async move { answer })
    }
    fn session(&self, share: &str) -> GuestFuture<'_, ()> {
        self.0.calls.borrow_mut().push(format!("session {share}"));
        let answer = self.0.session.borrow_mut().pop_front().unwrap_or(Ok(()));
        Box::pin(async move { answer })
    }
    fn open_stream(&self, share: &str) -> GuestFuture<'_, Box<dyn GuestStreamHandle>> {
        self.0.calls.borrow_mut().push(format!("stream {share}"));
        let answer = self.0.streams.borrow_mut().pop_front();
        Box::pin(async move {
            match answer {
                Some(Ok(script)) => Ok(Box::new(Stream(script)) as Box<dyn GuestStreamHandle>),
                Some(Err(error)) => Err(error),
                None => Err(GuestError::Rejected {
                    status: 403,
                    reason: GuestFailure::Inactive,
                    message: "script exhausted".into(),
                }),
            }
        })
    }
    fn submit(&self, share: &str, text: &str) -> GuestFuture<'_, ()> {
        self.0
            .calls
            .borrow_mut()
            .push(format!("submit {share} {text}"));
        let answer = self.0.submit.borrow_mut().pop_front().unwrap_or(Ok(()));
        Box::pin(async move { answer })
    }
    fn sleep(&self, millis: u64) -> LocalBoxFuture<'_, ()> {
        self.0.sleeps.borrow_mut().push(millis);
        Box::pin(async {})
    }
}

fn rejected(status: u16, reason: GuestFailure) -> GuestError {
    GuestError::Rejected {
        status,
        reason,
        message: "refused".into(),
    }
}

fn thread_json(id: &str) -> Value {
    json!({"id":id,"projectId":"project","title":"Shared","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":id,"parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":NOW,"updatedAt":NOW,"archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null})
}

fn snapshot(thread: &str, sequence: u64) -> Value {
    json!({"type":"thread","item":{"kind":"snapshot","snapshotSequence":sequence,"projection":{
        "thread":thread_json(thread),"runs":[],"attempts":[],"nodes":[],"subagents":[],
        "providerSessions":[],"providerThreads":[],"providerTurns":[],"runtimeRequests":[],
        "messages":[],"plans":[],"turnItems":[],"checkpointScopes":[],"checkpoints":[],
        "contextHandoffs":[],"contextTransfers":[],"visibleTurnItems":[],"updatedAt":NOW}}})
}

fn view(pending: usize) -> Value {
    let messages: Vec<Value> = (0..pending)
        .map(|n| json!({"id":format!("m{n}"),"text":"hello","status":"pending","submittedAt":NOW}))
        .collect();
    json!({"type":"view","view":{"shareId":"share_1","title":"Shared","guestName":"Sam",
        "expiresAt":"2026-06-21T00:00:00.000Z","messages":messages,
        "model":{"driver":"codex","name":"GPT"}}})
}

fn open_stream(pending: usize) -> Script {
    let mut script = live_stream(pending);
    script.push_back(Step::Hang);
    script
}

fn live_stream(pending: usize) -> Script {
    VecDeque::from([
        Step::Frame(view(pending)),
        Step::Frame(snapshot("thread:shared", 1)),
    ])
}

struct Page {
    wire: Rc<Wire>,
    client: GuestClient,
    phases: Rc<RefCell<Vec<GuestPhase>>>,
}

fn page(credential: bool) -> Page {
    let wire = Rc::new(Wire::default());
    let phases = Rc::new(RefCell::new(Vec::new()));
    if credential {
        // The cookie check that precedes the name form: no cookie yet.
        wire.session
            .borrow_mut()
            .push_back(Err(rejected(403, GuestFailure::Invalid)));
    }
    let seen = phases.clone();
    let client = GuestClient::new(
        Rc::new(Fake(wire.clone())),
        "share_1",
        credential.then(|| GuestCredential::new("share_1.signature")),
        move |state| {
            let mut seen = seen.borrow_mut();
            if seen.last() != Some(&state.phase) {
                seen.push(state.phase.clone());
            }
        },
    );
    Page {
        wire,
        client,
        phases,
    }
}

fn phase(page: &Page) -> GuestPhase {
    page.client.snapshot(|state| state.phase.clone())
}

#[test]
fn an_invite_asks_for_a_name_then_redeems_once_and_goes_live() {
    let page = page(true);
    page.wire.streams.borrow_mut().push_back(Ok(open_stream(0)));
    block_on(async {
        page.client.start().await;
        assert_eq!(phase(&page), GuestPhase::NeedsName { error: None });
        page.client.join("  Sam  ").await;
        assert_eq!(phase(&page), GuestPhase::Connecting);
        assert!(
            page.client.run().now_or_never().is_none(),
            "the stream stays open"
        );
    });
    assert_eq!(phase(&page), GuestPhase::Live);
    assert_eq!(
        page.wire.calls.borrow().as_slice(),
        ["session share_1", "redeem Sam", "stream share_1"]
    );
    assert_eq!(
        page.client.snapshot(|s| s.thread_id().cloned()).unwrap().0,
        "thread:shared"
    );
    assert_eq!(
        page.client.snapshot(|s| s.view.clone()).unwrap().title,
        "Shared"
    );
}

#[test]
fn a_bad_name_is_refused_before_the_invite_is_spent() {
    let page = page(true);
    block_on(async {
        page.client.start().await;
        for name in ["", "   ", "Line\nbreak", "Alex\u{202e}", &"x".repeat(41)] {
            page.client.join(name).await;
            assert!(
                matches!(phase(&page), GuestPhase::NeedsName { error: Some(_) }),
                "{name:?}"
            );
        }
    });
    assert!(
        page.wire
            .calls
            .borrow()
            .iter()
            .all(|call| !call.starts_with("redeem")),
        "{:?}",
        page.wire.calls.borrow()
    );
}

#[test]
fn a_network_failure_keeps_the_invite_and_a_used_invite_ends_the_page() {
    let page = page(true);
    page.wire.redeem.borrow_mut().extend([
        Err(GuestError::Network("offline".into())),
        Err(rejected(409, GuestFailure::Used)),
    ]);
    block_on(async {
        page.client.start().await;
        page.client.join("Sam").await;
        assert!(matches!(
            phase(&page),
            GuestPhase::NeedsName { error: Some(_) }
        ));
        page.client.join("Sam").await;
        assert_eq!(phase(&page), GuestPhase::Ended(GuestEnd::Used));
        // The credential is spent for good: another attempt sends nothing.
        page.client.join("Sam").await;
    });
    let redeems = page
        .wire
        .calls
        .borrow()
        .iter()
        .filter(|call| call.starts_with("redeem"))
        .count();
    assert_eq!(redeems, 2);
}

#[test]
fn a_spent_link_in_a_browser_that_already_joined_goes_straight_back_in() {
    let page = page(true);
    page.wire.session.borrow_mut().clear();
    page.wire.streams.borrow_mut().push_back(Ok(open_stream(0)));
    block_on(async {
        page.client.start().await;
        assert_eq!(phase(&page), GuestPhase::Connecting);
        // The cookie is good, so the spent invite is dropped and never sent.
        page.client.join("Sam").await;
        assert!(page.client.run().now_or_never().is_none());
    });
    assert_eq!(phase(&page), GuestPhase::Live);
    assert_eq!(
        page.wire.calls.borrow().as_slice(),
        ["session share_1", "stream share_1"]
    );
}

#[test]
fn an_invite_with_no_usable_cookie_shows_the_name_form_even_when_the_probe_fails() {
    for answer in [
        rejected(403, GuestFailure::Invalid),
        GuestError::Network("offline".into()),
    ] {
        let page = page(true);
        page.wire.session.borrow_mut().clear();
        page.wire.session.borrow_mut().push_back(Err(answer));
        block_on(page.client.start());
        assert_eq!(phase(&page), GuestPhase::NeedsName { error: None });
    }
}

#[test]
fn a_proxy_refusal_without_the_server_reason_keeps_the_invite_and_the_form() {
    for status in [408, 413, 429] {
        let page = page(true);
        page.wire
            .redeem
            .borrow_mut()
            .push_back(Err(rejected(status, GuestFailure::Unknown)));
        page.wire.streams.borrow_mut().push_back(Ok(open_stream(0)));
        block_on(async {
            page.client.start().await;
            page.client.join("Sam").await;
            assert!(
                matches!(phase(&page), GuestPhase::NeedsName { error: Some(_) }),
                "{status}"
            );
            page.client.join("Sam").await;
            assert_eq!(phase(&page), GuestPhase::Connecting, "{status}");
        });
    }
}

#[test]
fn an_offline_start_retries_until_the_network_returns() {
    let page = page(false);
    page.wire
        .session
        .borrow_mut()
        .push_back(Err(GuestError::Network("offline".into())));
    page.wire.streams.borrow_mut().extend([
        Err(GuestError::Network("offline".into())),
        Ok(open_stream(0)),
    ]);
    block_on(async {
        assert!(page.client.resume().now_or_never().is_none());
    });
    assert_eq!(phase(&page), GuestPhase::Live);
    assert_eq!(page.wire.sleeps.borrow().as_slice(), [1_000]);
}

#[test]
fn a_server_refusal_after_an_offline_start_still_ends_the_page() {
    let page = page(false);
    page.wire
        .session
        .borrow_mut()
        .push_back(Err(GuestError::Network("offline".into())));
    page.wire
        .streams
        .borrow_mut()
        .push_back(Err(rejected(403, GuestFailure::Inactive)));
    block_on(page.client.resume());
    assert_eq!(phase(&page), GuestPhase::Ended(GuestEnd::Inactive));
}

#[test]
fn a_returning_guest_checks_its_cookie_and_a_bad_one_shows_an_invalid_link() {
    let page = page(false);
    page.wire.streams.borrow_mut().push_back(Ok(open_stream(0)));
    block_on(async {
        page.client.start().await;
        assert_eq!(phase(&page), GuestPhase::Connecting);
        assert!(page.client.run().now_or_never().is_none());
    });
    assert_eq!(phase(&page), GuestPhase::Live);
    assert_eq!(page.wire.calls.borrow()[0], "session share_1");

    let page = self::page(false);
    page.wire
        .session
        .borrow_mut()
        .push_back(Err(rejected(403, GuestFailure::Invalid)));
    block_on(page.client.start());
    assert_eq!(phase(&page), GuestPhase::Ended(GuestEnd::Invalid));
}

#[test]
fn a_dropped_stream_keeps_the_transcript_and_reconnects_with_backoff() {
    let page = page(false);
    let mut first = live_stream(0);
    first.push_back(Step::Fail(GuestError::Network("reset".into())));
    let mut second = live_stream(1);
    second.push_back(Step::Close);
    page.wire.streams.borrow_mut().extend([
        Ok(first),
        Err(GuestError::Network("still down".into())),
        Ok(second),
        Err(rejected(403, GuestFailure::Inactive)),
    ]);
    block_on(async {
        page.client.start().await;
        page.client.run().await;
    });
    assert_eq!(phase(&page), GuestPhase::Ended(GuestEnd::Inactive));
    assert_eq!(page.wire.sleeps.borrow().as_slice(), [1_000, 2_000, 1_000]);
    let phases = page.phases.borrow();
    assert!(phases.contains(&GuestPhase::Reconnecting));
    assert!(phases.contains(&GuestPhase::Live));
    // Revocation drops what the guest had read.
    assert!(
        page.client
            .snapshot(|s| s.thread().is_none() && s.view.is_none())
    );
}

#[test]
fn an_ended_frame_stops_reconnection_and_clears_the_projection() {
    let page = page(false);
    let mut stream = live_stream(0);
    stream.push_back(Step::Frame(json!({"type":"ended"})));
    page.wire.streams.borrow_mut().push_back(Ok(stream));
    block_on(async {
        page.client.start().await;
        page.client.run().await;
    });
    assert_eq!(phase(&page), GuestPhase::Ended(GuestEnd::Inactive));
    assert!(page.client.snapshot(|s| s.thread().is_none()));
    assert_eq!(
        page.wire
            .calls
            .borrow()
            .iter()
            .filter(|c| c.starts_with("stream"))
            .count(),
        1
    );
}

#[test]
fn a_snapshot_of_another_thread_is_never_shown() {
    let page = page(false);
    let mut stream = live_stream(0);
    stream.push_back(Step::Frame(snapshot("thread:private", 1)));
    page.wire.streams.borrow_mut().push_back(Ok(stream));
    block_on(async {
        page.client.start().await;
        page.client.run().await;
    });
    assert_eq!(phase(&page), GuestPhase::Ended(GuestEnd::Invalid));
    assert!(page.client.snapshot(|s| s.thread().is_none()));
}

#[test]
fn the_composer_enforces_the_share_limits_and_surfaces_server_refusals() {
    let page = page(false);
    page.wire.streams.borrow_mut().push_back(Ok(open_stream(3)));
    page.wire
        .submit
        .borrow_mut()
        .extend([Err(rejected(409, GuestFailure::Busy)), Ok(())]);
    block_on(async {
        assert_eq!(
            page.client.submit("early").await,
            Err(SubmitRefusal::NotLive)
        );
        page.client.start().await;
        assert!(page.client.run().now_or_never().is_none());
        // Three messages already await the owner.
        assert_eq!(
            page.client.submit("one more").await,
            Err(SubmitRefusal::TooManyPending)
        );
    });

    let page = self::page(false);
    page.wire.streams.borrow_mut().push_back(Ok(open_stream(0)));
    page.wire
        .submit
        .borrow_mut()
        .extend([Err(rejected(409, GuestFailure::Busy)), Ok(())]);
    block_on(async {
        page.client.start().await;
        assert!(page.client.run().now_or_never().is_none());
        assert_eq!(page.client.submit("  \n ").await, Err(SubmitRefusal::Empty));
        assert_eq!(
            page.client.submit(&"x".repeat(4_001)).await,
            Err(SubmitRefusal::TooLong)
        );
        assert!(matches!(
            page.client.submit(" hi ").await,
            Err(SubmitRefusal::Server(GuestFailure::Busy, _))
        ));
        assert_eq!(page.client.submit("hi").await, Ok(()));
    });
    let calls = page.wire.calls.borrow();
    assert_eq!(calls.iter().filter(|c| c.starts_with("submit")).count(), 2);
    assert_eq!(calls[calls.len() - 1], "submit share_1 hi");
}

fn message_event(sequence: u64, id: &str, kind: &str, ordinal: u64, text: &str) -> Value {
    json!({"type":"thread","item":{"kind":"event","sequence":sequence,"event":{
        "type":"turn-item.updated","id":format!("event-{sequence}"),"threadId":"thread:shared",
        "occurredAt":NOW,"payload":{
            "id":id,"type":kind,"threadId":"thread:shared","runId":null,"nodeId":null,
            "providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,
            "ordinal":ordinal,"status":"completed","title":null,"startedAt":null,"completedAt":null,
            "updatedAt":NOW,"messageId":format!("msg-{id}"),"createdBy":"user","creationSource":"web","inputIntent":"turn_start","phase":"final_answer","streaming":false,"text":text,"attachments":[]}}}})
}

#[test]
fn the_transcript_follows_ordered_deltas_and_ignores_replays() {
    let page = page(false);
    let mut stream = live_stream(0);
    stream.extend([
        Step::Frame(message_event(2, "b", "assistant_message", 2, "Hi Sam")),
        Step::Frame(message_event(3, "a", "user_message", 1, "Hello")),
        Step::Frame(message_event(3, "a", "user_message", 1, "replayed")),
        Step::Hang,
    ]);
    page.wire.streams.borrow_mut().push_back(Ok(stream));
    block_on(async {
        page.client.start().await;
        let still = page.client.run().now_or_never().is_none();
        assert!(still, "ended: {:?}", phase(&page));
    });
    let rows = page.client.snapshot(|s| s.transcript());
    let rows: Vec<_> = rows.iter().map(|r| (r.role, r.text.as_str())).collect();
    use t3_client::guest::session::TranscriptRole::{Assistant, Person};
    assert_eq!(rows, [(Person, "Hello"), (Assistant, "Hi Sam")]);
}

#[test]
fn the_invite_never_appears_in_debug_output() {
    let credential = GuestCredential::new("share_1.signature");
    assert!(!format!("{credential:?}").contains("signature"));
}
