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
    history: RefCell<
        VecDeque<
            Result<t3_contracts::orchestration_v2::OrchestrationV2ThreadHistoryPage, GuestError>,
        >,
    >,
    command_outputs: RefCell<VecDeque<Result<Script, GuestError>>>,
    dropped_command_outputs: RefCell<Vec<String>>,
    submit: RefCell<VecDeque<Result<(), GuestError>>>,
    calls: RefCell<Vec<String>>,
    sleeps: RefCell<Vec<u64>>,
    random: RefCell<f64>,
}

struct Fake(Rc<Wire>);

struct Stream {
    script: Script,
    drop_record: Option<(Rc<Wire>, String)>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        if let Some((wire, tag)) = self.drop_record.take() {
            wire.dropped_command_outputs.borrow_mut().push(tag);
        }
    }
}

impl GuestStreamHandle for Stream {
    fn next(&mut self) -> GuestFuture<'_, Option<Value>> {
        let step = self.script.pop_front();
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
                Some(Ok(script)) => Ok(Box::new(Stream {
                    script,
                    drop_record: None,
                }) as Box<dyn GuestStreamHandle>),
                Some(Err(error)) => Err(error),
                None => Err(GuestError::Rejected {
                    status: 403,
                    reason: GuestFailure::Inactive,
                    message: "script exhausted".into(),
                }),
            }
        })
    }
    fn history(
        &self,
        share: &str,
        cursor: &str,
    ) -> GuestFuture<'_, t3_contracts::orchestration_v2::OrchestrationV2ThreadHistoryPage> {
        self.0
            .calls
            .borrow_mut()
            .push(format!("history {share} {cursor}"));
        let answer = self.0.history.borrow_mut().pop_front();
        Box::pin(async move {
            answer.unwrap_or_else(|| Err(GuestError::Network("no scripted history".into())))
        })
    }
    fn command_output(
        &self,
        share: &str,
        item: &str,
    ) -> GuestFuture<'_, Box<dyn GuestStreamHandle>> {
        self.0
            .calls
            .borrow_mut()
            .push(format!("command-output {share} {item}"));
        let answer = self.0.command_outputs.borrow_mut().pop_front();
        let wire = self.0.clone();
        let item_tag = item.to_owned();
        Box::pin(async move {
            match answer {
                Some(Ok(script)) => Ok(Box::new(Stream {
                    script,
                    drop_record: Some((wire, item_tag)),
                }) as Box<dyn GuestStreamHandle>),
                Some(Err(error)) => Err(error),
                None => Err(GuestError::Rejected {
                    status: 403,
                    reason: GuestFailure::Unavailable,
                    message: "no scripted output".into(),
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
    fn random(&self) -> f64 {
        *self.0.random.borrow()
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
    *wire.random.borrow_mut() = 0.5;
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
    assert_eq!(page.wire.sleeps.borrow().as_slice(), [750]);
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
    assert_eq!(page.wire.sleeps.borrow().as_slice(), [750, 1_500, 750]);
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

#[test]
fn reconnect_backoff_adds_jitter_and_honours_retry_after_capped_at_15s() {
    use t3_client::guest::session::reconnect_delay_ms;
    assert_eq!(reconnect_delay_ms(0, 0.0, None), 500);
    assert_eq!(reconnect_delay_ms(0, 1.0, None), 1_000);
    assert_eq!(reconnect_delay_ms(4, 0.0, None), 7_500);
    assert_eq!(reconnect_delay_ms(4, 1.0, None), 15_000);
    // Retry-After is honoured (with jitter on top) and capped at 15 s.
    assert_eq!(reconnect_delay_ms(0, 0.0, Some(5_000)), 5_000);
    assert_eq!(reconnect_delay_ms(0, 1.0, Some(5_000)), 5_500);
    assert_eq!(reconnect_delay_ms(0, 1.0, Some(60_000)), 15_000);
    assert_eq!(reconnect_delay_ms(2, 1.0, Some(1_000)), 4_000);

    let page = page(false);
    page.wire.streams.borrow_mut().extend([
        Err(GuestError::RetryAfter {
            millis: 5_000,
            message: "HTTP 429".into(),
        }),
        Err(GuestError::RetryAfter {
            millis: 60_000,
            message: "HTTP 429".into(),
        }),
        Err(rejected(403, GuestFailure::Inactive)),
    ]);
    block_on(async {
        page.client.start().await;
        page.client.run().await;
    });
    assert_eq!(page.wire.sleeps.borrow().as_slice(), [5_250, 15_000]);

    // Different jitter samples produce distinct backoff sleeps within [base/2, base].
    let p_low = page_with_random(0.0);
    let p_high = page_with_random(1.0);
    for p in [&p_low, &p_high] {
        p.wire.streams.borrow_mut().extend([
            Err(GuestError::Network("drop".into())),
            Err(rejected(403, GuestFailure::Inactive)),
        ]);
    }
    block_on(async {
        p_low.client.start().await;
        p_low.client.run().await;
        p_high.client.start().await;
        p_high.client.run().await;
    });
    assert_eq!(p_low.wire.sleeps.borrow().as_slice(), [500]);
    assert_eq!(p_high.wire.sleeps.borrow().as_slice(), [1_000]);
}

fn page_with_random(random: f64) -> Page {
    let p = page(false);
    *p.wire.random.borrow_mut() = random;
    p
}

fn assistant_row(id: &str, ordinal: u64, text: &str) -> Value {
    json!({
        "position": 0,
        "visibility": "local",
        "sourceThreadId": "thread:shared",
        "sourceItemId": id,
        "item": {
            "id": id,
            "type": "assistant_message",
            "threadId": "thread:shared",
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

fn command_row(id: &str, ordinal: u64, input: &str, exit_code: Option<i64>) -> Value {
    json!({
        "position": 0,
        "visibility": "local",
        "sourceThreadId": "thread:shared",
        "sourceItemId": id,
        "item": {
            "id": id,
            "type": "command_execution",
            "threadId": "thread:shared",
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

#[test]
fn history_paging_merges_older_items_without_duplicates_or_reordering_and_ignores_stale_cursors() {
    let page = page(false);
    let mut stream = VecDeque::from([
        Step::Frame(view(0)),
        Step::Frame(json!({
            "type": "thread",
            "item": {
                "kind": "snapshot",
                "snapshotSequence": 1,
                "historyCursor": "c1",
                "hasMoreHistory": true,
                "projection": {
                    "thread": thread_json("thread:shared"),
                    "runs": [], "attempts": [], "nodes": [], "subagents": [],
                    "providerSessions": [], "providerThreads": [], "providerTurns": [],
                    "runtimeRequests": [], "messages": [], "plans": [],
                    "turnItems": [assistant_row("a2", 2, "Latest")["item"]],
                    "checkpointScopes": [], "checkpoints": [], "contextHandoffs": [],
                    "contextTransfers": [],
                    "visibleTurnItems": [assistant_row("a2", 2, "Latest")],
                    "updatedAt": NOW
                }
            }
        })),
        Step::Hang,
    ]);
    let _ = &mut stream;
    page.wire.streams.borrow_mut().push_back(Ok(stream));
    page.wire.history.borrow_mut().extend([
        Err(GuestError::Network("offline".into())),
        Ok(serde_json::from_value(json!({
            "snapshotSequence": 1,
            "items": [
                assistant_row("a0", 0, "Oldest"),
                assistant_row("a1", 1, "Middle"),
                assistant_row("a2", 2, "Latest duplicate")
            ],
            "nextCursor": "c2",
            "hasMoreHistory": true
        }))
        .unwrap()),
        Ok(serde_json::from_value(json!({
            "snapshotSequence": 1,
            "items": [],
            "nextCursor": null,
            "hasMoreHistory": false
        }))
        .unwrap()),
    ]);
    block_on(async {
        page.client.start().await;
        assert!(page.client.run().now_or_never().is_none());
        assert_eq!(
            page.client
                .snapshot(|s| s.history().cursor.as_deref().map(str::to_owned)),
            Some("c1".into())
        );

        page.client.load_earlier().await;
        assert_eq!(
            page.client.snapshot(|s| s.history().error),
            Some("Could not reach the server.".into())
        );
        assert!(page.client.snapshot(|s| s.history().has_more));

        page.client.load_earlier().await;
        assert_eq!(page.client.snapshot(|s| s.history().error), None);
        assert_eq!(
            page.client
                .snapshot(|s| s.history().cursor.as_deref().map(str::to_owned)),
            Some("c2".into())
        );

        let texts: Vec<_> = page
            .client
            .snapshot(|s| s.transcript().into_iter().map(|r| (r.id, r.text)).collect());
        assert_eq!(
            texts,
            vec![
                ("a0".into(), "Oldest".into()),
                ("a1".into(), "Middle".into()),
                ("a2".into(), "Latest".into()),
            ]
        );

        page.client.load_earlier().await;
        assert!(!page.client.snapshot(|s| s.history().has_more));
        assert_eq!(
            page.client
                .snapshot(|s| s.history().cursor.as_deref().map(str::to_owned)),
            None
        );
    });
}

#[test]
fn a_reconnect_snapshot_discards_an_in_flight_history_page_for_an_older_cursor() {
    use futures::channel::oneshot;
    struct GatedHistory {
        fake: Fake,
        gate: RefCell<Option<oneshot::Sender<()>>>,
        reply: RefCell<Option<t3_contracts::orchestration_v2::OrchestrationV2ThreadHistoryPage>>,
    }
    impl GuestTransport for GatedHistory {
        fn redeem(&self, _: &str, _: &str) -> GuestFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
        fn session(&self, share: &str) -> GuestFuture<'_, ()> {
            self.fake.session(share)
        }
        fn open_stream(&self, share: &str) -> GuestFuture<'_, Box<dyn GuestStreamHandle>> {
            self.fake.open_stream(share)
        }
        fn history(
            &self,
            _: &str,
            _: &str,
        ) -> GuestFuture<'_, t3_contracts::orchestration_v2::OrchestrationV2ThreadHistoryPage>
        {
            let (tx, rx) = oneshot::channel();
            *self.gate.borrow_mut() = Some(tx);
            let reply = self.reply.borrow_mut().take().unwrap();
            Box::pin(async move {
                let _ = rx.await;
                Ok(reply)
            })
        }
        fn command_output(
            &self,
            share: &str,
            item: &str,
        ) -> GuestFuture<'_, Box<dyn GuestStreamHandle>> {
            self.fake.command_output(share, item)
        }
        fn submit(&self, share: &str, text: &str) -> GuestFuture<'_, ()> {
            self.fake.submit(share, text)
        }
        fn sleep(&self, millis: u64) -> LocalBoxFuture<'_, ()> {
            self.fake.sleep(millis)
        }
    }

    let wire = Rc::new(Wire::default());
    let snap = |seq: u64, cursor: &str, id: &str| {
        json!({
            "type": "thread",
            "item": {
                "kind": "snapshot",
                "snapshotSequence": seq,
                "historyCursor": cursor,
                "hasMoreHistory": true,
                "projection": {
                    "thread": thread_json("thread:shared"),
                    "runs": [], "attempts": [], "nodes": [], "subagents": [],
                    "providerSessions": [], "providerThreads": [], "providerTurns": [],
                    "runtimeRequests": [], "messages": [], "plans": [],
                    "turnItems": [assistant_row(id, seq, id)["item"]],
                    "checkpointScopes": [], "checkpoints": [], "contextHandoffs": [],
                    "contextTransfers": [],
                    "visibleTurnItems": [assistant_row(id, seq, id)],
                    "updatedAt": NOW
                }
            }
        })
    };
    let (tx, rx) = futures::channel::mpsc::unbounded();
    tx.unbounded_send(view(0)).unwrap();
    tx.unbounded_send(snap(1, "c1", "before")).unwrap();
    struct LiveChan(futures::channel::mpsc::UnboundedReceiver<Value>);
    impl GuestStreamHandle for LiveChan {
        fn next(&mut self) -> GuestFuture<'_, Option<Value>> {
            use futures::StreamExt;
            Box::pin(async move { Ok(self.0.next().await) })
        }
    }
    struct ChanTransport(Rc<GatedHistory>, RefCell<Option<LiveChan>>);
    impl GuestTransport for ChanTransport {
        fn redeem(&self, c: &str, n: &str) -> GuestFuture<'_, ()> {
            self.0.redeem(c, n)
        }
        fn session(&self, s: &str) -> GuestFuture<'_, ()> {
            self.0.session(s)
        }
        fn open_stream(&self, _: &str) -> GuestFuture<'_, Box<dyn GuestStreamHandle>> {
            let s = self.1.borrow_mut().take().unwrap();
            Box::pin(async move { Ok(Box::new(s) as Box<dyn GuestStreamHandle>) })
        }
        fn history(
            &self,
            s: &str,
            c: &str,
        ) -> GuestFuture<'_, t3_contracts::orchestration_v2::OrchestrationV2ThreadHistoryPage>
        {
            self.0.history(s, c)
        }
        fn command_output(&self, s: &str, i: &str) -> GuestFuture<'_, Box<dyn GuestStreamHandle>> {
            self.0.command_output(s, i)
        }
        fn submit(&self, s: &str, t: &str) -> GuestFuture<'_, ()> {
            self.0.submit(s, t)
        }
        fn sleep(&self, m: u64) -> LocalBoxFuture<'_, ()> {
            self.0.sleep(m)
        }
    }

    let stale_page: t3_contracts::orchestration_v2::OrchestrationV2ThreadHistoryPage =
        serde_json::from_value(json!({
            "snapshotSequence": 1,
            "items": [assistant_row("stale-old", 0, "Stale")],
            "nextCursor": null,
            "hasMoreHistory": false
        }))
        .unwrap();
    let gated = Rc::new(GatedHistory {
        fake: Fake(wire),
        gate: RefCell::new(None),
        reply: RefCell::new(Some(stale_page)),
    });
    let transport = Rc::new(ChanTransport(
        gated.clone(),
        RefCell::new(Some(LiveChan(rx))),
    ));
    let client = GuestClient::new(transport, "share_1", None, |_| {});

    use futures::task::LocalSpawnExt;
    let mut pool = futures::executor::LocalPool::new();
    let spawner = pool.spawner();
    pool.run_until(client.start());
    let run_client = client.clone();
    spawner
        .spawn_local(async move { run_client.run().await })
        .unwrap();
    pool.run_until_stalled();
    assert_eq!(
        client.snapshot(|s| s.history().cursor.as_deref().map(str::to_owned)),
        Some("c1".into())
    );

    let load_client = client.clone();
    let (done_tx, done_rx) = oneshot::channel();
    spawner
        .spawn_local(async move {
            load_client.load_earlier().await;
            let _ = done_tx.send(());
        })
        .unwrap();
    pool.run_until_stalled();
    assert!(gated.gate.borrow().is_some());

    // Reconnect snapshot moves cursor to c2 before the c1 reply resolves.
    tx.unbounded_send(snap(2, "c2", "after")).unwrap();
    pool.run_until_stalled();
    assert_eq!(
        client.snapshot(|s| s.history().cursor.as_deref().map(str::to_owned)),
        Some("c2".into())
    );

    gated.gate.borrow_mut().take().unwrap().send(()).unwrap();
    pool.run_until(async {
        let _ = done_rx.await;
    });

    assert_eq!(
        client.snapshot(|s| s.history().cursor.as_deref().map(str::to_owned)),
        Some("c2".into())
    );
    assert!(client.snapshot(|s| s.history().has_more));
    let ids: Vec<_> = client.snapshot(|s| s.transcript().into_iter().map(|r| r.id).collect());
    assert_eq!(ids, vec!["after".to_owned()]);
}

#[test]
fn command_rows_appear_in_transcript_and_stream_output_until_settled() {
    use futures::task::LocalSpawnExt;
    let page = page(false);
    page.wire.streams.borrow_mut().push_back(Ok(VecDeque::from([
        Step::Frame(view(0)),
        Step::Frame(json!({
            "type": "thread",
            "item": {
                "kind": "snapshot",
                "snapshotSequence": 1,
                "projection": {
                    "thread": thread_json("thread:shared"),
                    "runs": [], "attempts": [], "nodes": [], "subagents": [],
                    "providerSessions": [], "providerThreads": [], "providerTurns": [],
                    "runtimeRequests": [], "messages": [], "plans": [],
                    "turnItems": [command_row("cmd-1", 1, "vp run check:changed", Some(0))["item"]],
                    "checkpointScopes": [], "checkpoints": [], "contextHandoffs": [],
                    "contextTransfers": [],
                    "visibleTurnItems": [command_row("cmd-1", 1, "vp run check:changed", Some(0))],
                    "updatedAt": NOW
                }
            }
        })),
        Step::Hang,
    ])));
    page.wire.command_outputs.borrow_mut().push_back(Ok(VecDeque::from([
        Step::Frame(json!({"kind":"replace","text":"checking...\n","truncated":false,"running":true})),
        Step::Frame(json!({"kind":"append","text":"4 files checked\n","truncated":false,"running":false})),
    ])));

    let mut pool = futures::executor::LocalPool::new();
    let spawner_handle = pool.spawner();
    let spawner: t3_client::composition::Spawner = Rc::new(move |fut| {
        let _ = spawner_handle.spawn_local(fut);
    });
    pool.run_until(async {
        page.client.start().await;
        assert!(page.client.run().now_or_never().is_none());
        let rows = page.client.snapshot(|s| s.transcript());
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].role,
            t3_client::guest::session::TranscriptRole::Command
        );
        assert_eq!(rows[0].text, "vp run check:changed");
        assert_eq!(rows[0].exit_code, Some(0));

        let lease = page.client.command_output("cmd-1", &spawner);
        let mut changes = lease.changes();
        use futures::StreamExt;
        while lease.read(None).is_none_or(|delta| delta.running) {
            changes.next().await;
        }
        assert_eq!(
            lease.text().as_deref(),
            Some("checking...\n4 files checked\n")
        );
        assert!(!lease.read(None).unwrap().running);
        assert_eq!(
            page.wire.dropped_command_outputs.borrow().as_slice(),
            ["cmd-1"]
        );
    });
}

#[test]
fn command_output_retries_network_and_retry_after_until_settled_and_stops_on_rejected() {
    use futures::task::LocalSpawnExt;
    let page = page(false);
    page.wire.command_outputs.borrow_mut().extend([
        Err(GuestError::Network("blip".into())),
        Ok(VecDeque::from([
            Step::Frame(
                json!({"kind":"replace","text":"part 1\n","truncated":false,"running":true}),
            ),
            Step::Fail(GuestError::RetryAfter {
                millis: 5_000,
                message: "HTTP 429".into(),
            }),
        ])),
        Ok(VecDeque::from([Step::Close])),
        Ok(VecDeque::from([Step::Frame(
            json!({"kind":"replace","text":"part 1\npart 2\n","truncated":false,"running":false}),
        )])),
        Err(rejected(403, GuestFailure::Unavailable)),
        Ok(VecDeque::from([
            Step::Frame(json!({"kind":"replace","text":"live\n","truncated":false,"running":true})),
            Step::Hang,
        ])),
    ]);

    let mut pool = futures::executor::LocalPool::new();
    let spawner_handle = pool.spawner();
    let spawner: t3_client::composition::Spawner = Rc::new(move |fut| {
        let _ = spawner_handle.spawn_local(fut);
    });
    pool.run_until(async {
        let lease = page.client.command_output("cmd-retry", &spawner);
        let mut changes = lease.changes();
        use futures::StreamExt;
        while lease.read(None).is_none_or(|delta| delta.running) {
            assert!(lease.error().is_none(), "{:?}", lease.error());
            changes.next().await;
        }
        assert_eq!(lease.text().as_deref(), Some("part 1\npart 2\n"));
        assert!(lease.error().is_none());
        assert_eq!(
            page.wire.sleeps.borrow().as_slice(),
            [750, 5_250, 1_500],
            "initial network (attempt 0), mid-stream Retry-After (reset to 0), clean EOF (attempt 1 with random=0.5)"
        );
        assert_eq!(
            page.wire.dropped_command_outputs.borrow().as_slice(),
            ["cmd-retry", "cmd-retry", "cmd-retry"]
        );

        let rejected_lease = page.client.command_output("cmd-gone", &spawner);
        let mut rejected_changes = rejected_lease.changes();
        while rejected_lease.error().is_none() {
            rejected_changes.next().await;
        }
        assert_eq!(
            rejected_lease.error().as_deref(),
            Some("Output is no longer available.")
        );
        assert_eq!(page.wire.sleeps.borrow().len(), 3);

        let hanging = page.client.command_output("cmd-hang", &spawner);
        let mut hang_changes = hanging.changes();
        hang_changes.next().await;
        assert_eq!(hanging.text().as_deref(), Some("live\n"));
        assert!(
            !page
                .wire
                .dropped_command_outputs
                .borrow()
                .contains(&"cmd-hang".to_owned()),
            "stream handle stays open while lease is held"
        );
        drop(hanging);
    });
    pool.run_until_stalled();
    assert!(
        page.wire
            .dropped_command_outputs
            .borrow()
            .contains(&"cmd-hang".to_owned()),
        "dropping the lease releases the stream handle"
    );
}

#[test]
fn live_event_during_load_earlier_merges_without_duplicates_or_reordering() {
    use futures::channel::oneshot;
    use futures::task::LocalSpawnExt;

    struct GatedHistory {
        fake: Fake,
        gate: RefCell<Option<oneshot::Sender<()>>>,
        reply: RefCell<Option<t3_contracts::orchestration_v2::OrchestrationV2ThreadHistoryPage>>,
    }
    impl GuestTransport for GatedHistory {
        fn redeem(&self, c: &str, n: &str) -> GuestFuture<'_, ()> {
            self.fake.redeem(c, n)
        }
        fn session(&self, s: &str) -> GuestFuture<'_, ()> {
            self.fake.session(s)
        }
        fn open_stream(&self, s: &str) -> GuestFuture<'_, Box<dyn GuestStreamHandle>> {
            self.fake.open_stream(s)
        }
        fn history(
            &self,
            _: &str,
            _: &str,
        ) -> GuestFuture<'_, t3_contracts::orchestration_v2::OrchestrationV2ThreadHistoryPage>
        {
            let (tx, rx) = oneshot::channel();
            *self.gate.borrow_mut() = Some(tx);
            let reply = self.reply.borrow_mut().take().unwrap();
            Box::pin(async move {
                let _ = rx.await;
                Ok(reply)
            })
        }
        fn command_output(&self, s: &str, i: &str) -> GuestFuture<'_, Box<dyn GuestStreamHandle>> {
            self.fake.command_output(s, i)
        }
        fn submit(&self, s: &str, t: &str) -> GuestFuture<'_, ()> {
            self.fake.submit(s, t)
        }
        fn sleep(&self, m: u64) -> LocalBoxFuture<'_, ()> {
            self.fake.sleep(m)
        }
    }

    let wire = Rc::new(Wire::default());
    let (tx, rx) = futures::channel::mpsc::unbounded();
    tx.unbounded_send(view(0)).unwrap();
    tx.unbounded_send(json!({
        "type": "thread",
        "item": {
            "kind": "snapshot",
            "snapshotSequence": 1,
            "historyCursor": "c1",
            "hasMoreHistory": true,
            "projection": {
                "thread": thread_json("thread:shared"),
                "runs": [], "attempts": [], "nodes": [], "subagents": [],
                "providerSessions": [], "providerThreads": [], "providerTurns": [],
                "runtimeRequests": [], "messages": [], "plans": [],
                "turnItems": [assistant_row("a2", 2, "Current")["item"]],
                "checkpointScopes": [], "checkpoints": [], "contextHandoffs": [],
                "contextTransfers": [],
                "visibleTurnItems": [assistant_row("a2", 2, "Current")],
                "updatedAt": NOW
            }
        }
    }))
    .unwrap();
    struct LiveChan(futures::channel::mpsc::UnboundedReceiver<Value>);
    impl GuestStreamHandle for LiveChan {
        fn next(&mut self) -> GuestFuture<'_, Option<Value>> {
            use futures::StreamExt;
            Box::pin(async move { Ok(self.0.next().await) })
        }
    }
    struct ChanTransport(Rc<GatedHistory>, RefCell<Option<LiveChan>>);
    impl GuestTransport for ChanTransport {
        fn redeem(&self, c: &str, n: &str) -> GuestFuture<'_, ()> {
            self.0.redeem(c, n)
        }
        fn session(&self, s: &str) -> GuestFuture<'_, ()> {
            self.0.session(s)
        }
        fn open_stream(&self, _: &str) -> GuestFuture<'_, Box<dyn GuestStreamHandle>> {
            let s = self.1.borrow_mut().take().unwrap();
            Box::pin(async move { Ok(Box::new(s) as Box<dyn GuestStreamHandle>) })
        }
        fn history(
            &self,
            s: &str,
            c: &str,
        ) -> GuestFuture<'_, t3_contracts::orchestration_v2::OrchestrationV2ThreadHistoryPage>
        {
            self.0.history(s, c)
        }
        fn command_output(&self, s: &str, i: &str) -> GuestFuture<'_, Box<dyn GuestStreamHandle>> {
            self.0.command_output(s, i)
        }
        fn submit(&self, s: &str, t: &str) -> GuestFuture<'_, ()> {
            self.0.submit(s, t)
        }
        fn sleep(&self, m: u64) -> LocalBoxFuture<'_, ()> {
            self.0.sleep(m)
        }
    }

    let older_page: t3_contracts::orchestration_v2::OrchestrationV2ThreadHistoryPage =
        serde_json::from_value(json!({
            "snapshotSequence": 1,
            "items": [
                assistant_row("a1", 1, "Older"),
                assistant_row("a2", 2, "Current")
            ],
            "nextCursor": null,
            "hasMoreHistory": false
        }))
        .unwrap();
    let gated = Rc::new(GatedHistory {
        fake: Fake(wire),
        gate: RefCell::new(None),
        reply: RefCell::new(Some(older_page)),
    });
    let transport = Rc::new(ChanTransport(
        gated.clone(),
        RefCell::new(Some(LiveChan(rx))),
    ));
    let client = GuestClient::new(transport, "share_1", None, |_| {});

    let mut pool = futures::executor::LocalPool::new();
    let spawner = pool.spawner();
    pool.run_until(client.start());
    let run_client = client.clone();
    spawner
        .spawn_local(async move { run_client.run().await })
        .unwrap();
    pool.run_until_stalled();

    let load_client = client.clone();
    let (done_tx, done_rx) = oneshot::channel();
    spawner
        .spawn_local(async move {
            load_client.load_earlier().await;
            let _ = done_tx.send(());
        })
        .unwrap();
    pool.run_until_stalled();
    assert!(gated.gate.borrow().is_some());

    // A live turn event arrives while the older history page is in flight.
    tx.unbounded_send(message_event(
        2,
        "a3",
        "assistant_message",
        3,
        "Live update",
    ))
    .unwrap();
    pool.run_until_stalled();

    gated.gate.borrow_mut().take().unwrap().send(()).unwrap();
    pool.run_until(async {
        let _ = done_rx.await;
    });

    let rows: Vec<_> =
        client.snapshot(|s| s.transcript().into_iter().map(|r| (r.id, r.text)).collect());
    assert_eq!(
        rows,
        vec![
            ("a1".into(), "Older".into()),
            ("a2".into(), "Current".into()),
            ("a3".into(), "Live update".into()),
        ]
    );
}
