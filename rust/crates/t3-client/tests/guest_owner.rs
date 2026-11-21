//! The owner's "Share with guest" creation over the fake server: the request carries only the
//! thread, typed refusals read as the TS messages, and the invite never reaches debug output.
#![allow(clippy::unwrap_used)]

mod support {
    pub mod fake_server;
}
use serde_json::json;
use support::fake_server::*;
use t3_client::{
    ClientStore, composition::ClientRuntime, connection::PairingInput, types::ThreadKey,
};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;

fn key() -> ThreadKey {
    ThreadKey {
        environment: environment("env"),
        thread: thread("t1"),
    }
}

fn paired(h: &mut Harness) -> ClientRuntime<TestPlatform> {
    paired_with(
        h,
        &[
            "orchestration:read",
            "orchestration:operate",
            "access:write",
        ],
    )
}

fn paired_with(h: &mut Harness, scopes: &[&str]) -> ClientRuntime<TestPlatform> {
    {
        let mut state = h.server().state.borrow_mut();
        state
            .extra_methods
            .push(("guestShare.create", t3_rpc::CallKind::Unary));
        state.permissions = Some(scopes.iter().map(|scope| (*scope).to_owned()).collect());
    }
    let c = ClientRuntime::new(
        h.platform.clone(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        h.spawner(),
    );
    h.run(c.pair(PairingInput {
        pairing_url: Some(format!("{ORIGIN}/pair#token=pairing-token")),
        ..Default::default()
    }))
    .unwrap();
    c.open_thread(&key());
    h.settle();
    c
}

fn script(h: &Harness, answer: Result<serde_json::Value, serde_json::Value>) {
    h.server()
        .state
        .borrow_mut()
        .rpc_script
        .insert("guestShare.create".into(), [answer].into());
}

#[test]
fn creating_a_link_sends_only_the_thread_and_returns_the_invite_url() {
    let mut h = Harness::new("env");
    let c = paired(&mut h);
    script(
        &h,
        Ok(
            json!({"url":"https://box.example/guest#share1.sig","share":{
            "id":"share1","host":"box.example","createdAt":NOW,
            "expiresAt":"2026-06-21T00:00:00.000Z","guestName":null,"messages":[]}}),
        ),
    );
    let link = h.run(c.thread_actions().create_guest_share(key())).unwrap();
    assert_eq!(link.url(), "https://box.example/guest#share1.sig");
    assert!(!format!("{link:?}").contains("sig"));
    let calls = h.server().state.borrow().rpc_calls.clone();
    let (_, payload) = calls
        .iter()
        .find(|(tag, _)| tag == "guestShare.create")
        .unwrap();
    assert_eq!(payload, &json!({"threadId":"t1"}));
}

#[test]
fn typed_refusals_read_as_the_web_messages() {
    for (reason, message) in [
        (
            "disabled",
            "Guest links are off. Turn them on in Settings > Labs.",
        ),
        (
            "connect_unavailable",
            "Guest links need a running T3 Connect tunnel.",
        ),
        ("thread_unavailable", "This thread cannot be shared."),
        ("read_failed", "Failed to read the shared thread."),
        (
            "changed",
            "The guest link changed while it was being created.",
        ),
    ] {
        let mut h = Harness::new("env");
        let c = paired(&mut h);
        script(
            &h,
            Err(json!({"_tag":"GuestShareCreateError","reason":reason})),
        );
        let error = h
            .run(c.thread_actions().create_guest_share(key()))
            .unwrap_err();
        assert_eq!(error, message);
    }
    let mut h = Harness::new("env");
    let c = paired(&mut h);
    script(
        &h,
        Err(
            json!({"_tag":"EnvironmentAuthorizationError","requiredScope":"access:write","message":"Permission denied."}),
        ),
    );
    let error = h
        .run(c.thread_actions().create_guest_share(key()))
        .unwrap_err();
    assert_eq!(error, "Could not create a guest link.");
}

#[test]
fn a_session_without_access_write_cannot_create_a_link() {
    let mut h = Harness::new("env");
    let c = paired_with(&mut h, &["orchestration:read", "orchestration:operate"]);
    let error = h
        .run(c.thread_actions().create_guest_share(key()))
        .unwrap_err();
    assert_eq!(error, "Could not create a guest link.");
    assert!(
        h.server()
            .state
            .borrow()
            .rpc_calls
            .iter()
            .all(|(tag, _)| tag != "guestShare.create"),
        "the client refused before sending"
    );
}

fn dispatched(h: &Harness) -> Vec<serde_json::Value> {
    h.server()
        .state
        .borrow()
        .rpc_calls
        .iter()
        .filter(|(tag, _)| tag == "orchestration.dispatchCommand")
        .map(|(_, payload)| payload.clone())
        .collect()
}

fn accept_commands(h: &Harness) {
    let mut state = h.server().state.borrow_mut();
    let script = state
        .rpc_script
        .entry("orchestration.dispatchCommand".into())
        .or_default();
    for sequence in 2..6 {
        script.push_back(Ok(json!({ "sequence": sequence })));
    }
}

#[test]
fn dismissing_a_guest_message_sends_the_dismiss_command() {
    use t3_client::thread_views::thread_ops::guest_share::GuestMessageDecision;
    let mut h = Harness::new("env");
    let c = paired(&mut h);
    accept_commands(&h);
    h.run(c.thread_actions().resolve_guest_message(
        key(),
        t3_contracts::base_schemas::GuestMessageId("gm1".into()),
        GuestMessageDecision::Dismiss,
    ))
    .unwrap();
    let sent = dispatched(&h);
    let command = sent.last().expect("a command was dispatched");
    assert_eq!(command["type"], "thread.guest-message.dismiss");
    assert_eq!(command["threadId"], "t1");
    assert_eq!(command["guestMessageId"], "gm1");
}

#[test]
fn sending_a_guest_message_queues_it_without_client_supplied_text() {
    use t3_client::thread_views::thread_ops::guest_share::GuestMessageDecision;
    let mut h = Harness::new("env");
    let c = paired(&mut h);
    accept_commands(&h);
    h.run(c.thread_actions().resolve_guest_message(
        key(),
        t3_contracts::base_schemas::GuestMessageId("gm1".into()),
        GuestMessageDecision::Send,
    ))
    .unwrap();
    let sent = dispatched(&h);
    let command = sent.last().expect("a command was dispatched");
    // As the TS client: the server supplies the guest's words; the run queues after the active one.
    assert_eq!(command["type"], "message.dispatch");
    assert_eq!(command["guestMessageId"], "gm1");
    assert_eq!(command["text"], "");
    assert_eq!(command["attachments"], json!([]));
    assert_eq!(
        command["dispatchMode"],
        json!({"type": "queue_after_active"})
    );
    assert_eq!(command["createdBy"], "user");
    assert_eq!(command["creationSource"], "web");
    assert_eq!(command["messageId"], command["commandId"]);
}

#[test]
fn a_session_without_operate_scope_cannot_resolve_guest_messages() {
    use t3_client::thread_views::thread_ops::guest_share::GuestMessageDecision;
    let mut h = Harness::new("env");
    let c = paired_with(&mut h, &["orchestration:read", "access:write"]);
    accept_commands(&h);
    let result = h.run(c.thread_actions().resolve_guest_message(
        key(),
        t3_contracts::base_schemas::GuestMessageId("gm1".into()),
        GuestMessageDecision::Dismiss,
    ));
    assert!(result.is_err());
    assert!(dispatched(&h).is_empty());
}
