#![allow(clippy::unwrap_used)]

use super::{
    command_policy_test::{change, projection},
    orchestrator_receipt_race_test::Harness,
};
use serde_json::{Value, json};
use t3_contracts::{base_schemas::*, guest_share::*, orchestration_v2::*};
use t3_orchestration::{api::*, ports::decisions::guest_share_decider::*, testing::TEST_TIME};

fn now() -> IsoDateTime {
    t3_contracts::wire::WireDateTime(TEST_TIME.into())
}
fn opened() -> OrchestrationV2AppThread {
    let thread = projection().thread;
    decide_guest_share_command(&thread,GuestShareCommand::Open(&serde_json::from_value(json!({"commandId":"open","threadId":"thread","shareId":"share","host":"env.test","expiresAt":"2026-09-04T00:00:00.000Z"})).unwrap()),&now()).unwrap()
}
fn redeemed() -> OrchestrationV2AppThread {
    decide_guest_share_command(&opened(), GuestShareCommand::Redeem(&redeem()), &now()).unwrap()
}
fn redeem() -> InternalCommandThreadGuestShareRedeem {
    serde_json::from_value(
        json!({"commandId":"redeem","threadId":"thread","shareId":"share","guestName":"Sam"}),
    )
    .unwrap()
}
fn submit(id: &str) -> InternalCommandThreadGuestMessageSubmit {
    serde_json::from_value(json!({"commandId":format!("submit:{id}"),"threadId":"thread","shareId":"share","guestMessageId":id,"text":"Why?"})).unwrap()
}
fn pending() -> OrchestrationV2AppThread {
    decide_guest_share_command(
        &redeemed(),
        GuestShareCommand::Submit(&submit("guest")),
        &now(),
    )
    .unwrap()
}
fn share(thread: &OrchestrationV2AppThread) -> &ThreadGuestShare {
    thread
        .guest_share
        .as_ref()
        .and_then(Option::as_ref)
        .unwrap()
}
fn attachment(id: &str, bytes: u64) -> Value {
    json!({"type":"file","id":id,"name":"file.txt","mimeType":"text/plain","sizeBytes":bytes})
}

#[test]
fn guest_links_redeem_once_and_reject_unredeemed_expired_or_replaced_links() {
    let thread = opened();
    assert_eq!(
        decide_guest_share_command(&thread, GuestShareCommand::Submit(&submit("guest")), &now())
            .unwrap_err(),
        INACTIVE
    );
    assert_eq!(
        decide_guest_share_command(&redeemed(), GuestShareCommand::Redeem(&redeem()), &now())
            .unwrap_err(),
        USED
    );
    let expires = t3_contracts::wire::WireDateTime("2026-09-04T00:00:00.000Z".into());
    assert_eq!(
        decide_guest_share_command(&thread, GuestShareCommand::Redeem(&redeem()), &expires)
            .unwrap_err(),
        INACTIVE
    );
    let wrong = change(&redeem(), json!({"shareId":"replaced"}));
    assert_eq!(
        decide_guest_share_command(&thread, GuestShareCommand::Redeem(&wrong), &now()).unwrap_err(),
        INACTIVE
    );
}

#[test]
fn owner_send_uses_guest_words_and_cannot_send_twice_or_edit_attribution() {
    let result =
        decide_guest_message_send(&pending(), &GuestMessageId("guest".into()), &now()).unwrap();
    assert_eq!(
        result.text,
        "Message from guest Sam, sent by the owner:\n\nWhy?"
    );
    assert_eq!(
        serde_json::to_value(&result.guest).unwrap(),
        json!({"guestMessageId":"guest","name":"Sam","text":"Why?","shareId":"share"})
    );
    assert_eq!(
        share(&result.thread).messages[0].status,
        ThreadGuestMessageStatus::Sent
    );
    assert_eq!(
        decide_guest_message_send(&result.thread, &GuestMessageId("guest".into()), &now())
            .err()
            .unwrap(),
        "Guest message guest is not pending."
    );
    let owner:CommandMessageDispatch=serde_json::from_value(json!({"commandId":"owner","threadId":"thread","messageId":"owner","text":"Mine","context":{"version":1,"records":[],"guest":{"guestMessageId":"guest","name":"Sam","text":"Mine"}},"attachments":[],"dispatchMode":{"type":"defer_start"},"createdBy":"user","creationSource":"web"})).unwrap();
    assert!(
        serde_json::to_value(without_guest_attribution(owner)).unwrap()["context"]
            .get("guest")
            .is_none()
    );
}

#[test]
fn cancelling_a_queued_send_dismisses_only_the_same_shares_sent_message() {
    let sent =
        decide_guest_message_send(&pending(), &GuestMessageId("guest".into()), &now()).unwrap();
    let cancelled = guest_share_after_cancelled_send(&sent.thread, &sent.guest, &now());
    assert_eq!(
        share(&cancelled).messages[0].status,
        ThreadGuestMessageStatus::Dismissed
    );
    let other = change(&sent.guest, json!({"shareId":"other"}));
    assert_eq!(
        guest_share_after_cancelled_send(&sent.thread, &other, &now()),
        sent.thread
    );
    assert_eq!(
        guest_share_after_cancelled_send(&pending(), &sent.guest, &now()),
        pending()
    );
}

#[test]
fn guest_requests_unsettle_only_explicitly_settled_threads() {
    let thread = change(
        &redeemed(),
        json!({"settledOverride":"settled","settledAt":TEST_TIME}),
    );
    let next =
        decide_guest_share_command(&thread, GuestShareCommand::Submit(&submit("guest")), &now())
            .unwrap();
    assert!(next.settled_override.is_none());
    assert!(next.settled_at.is_none());
    assert_eq!(next.unsettled_at, Some(Some(now())));
    let active = change(
        &redeemed(),
        json!({"settledOverride":"active","settledAt":null}),
    );
    assert_eq!(
        decide_guest_share_command(&active, GuestShareCommand::Submit(&submit("guest")), &now())
            .unwrap()
            .settled_override,
        Some(AppThreadSettledOverride::Active)
    );
}

#[test]
fn pending_limits_count_messages_and_attachment_bytes_and_keep_newest_resolved_messages() {
    let mut thread = redeemed();
    for id in 0..GUEST_PENDING_MESSAGE_LIMIT {
        thread = decide_guest_share_command(
            &thread,
            GuestShareCommand::Submit(&submit(&format!("guest:{id}"))),
            &now(),
        )
        .unwrap();
    }
    assert_eq!(
        decide_guest_share_command(
            &thread,
            GuestShareCommand::Submit(&submit("overflow")),
            &now()
        )
        .unwrap_err(),
        BUSY
    );
    let first = change(
        &submit("one"),
        json!({"attachments":[attachment("one",GUEST_PENDING_ATTACHMENT_BYTES_LIMIT)]}),
    );
    let thread =
        decide_guest_share_command(&redeemed(), GuestShareCommand::Submit(&first), &now()).unwrap();
    let second = change(&submit("two"), json!({"attachments":[attachment("two",1)]}));
    assert_eq!(
        decide_guest_share_command(&thread, GuestShareCommand::Submit(&second), &now())
            .unwrap_err(),
        FILES_BUSY
    );
    assert!(
        decide_guest_share_command(
            &thread,
            GuestShareCommand::Submit(&submit("no-files")),
            &now()
        )
        .is_ok()
    );
    let mut thread = pending();
    for number in 0..GUEST_RESOLVED_MESSAGE_LIMIT + 5 {
        let id = GuestMessageId(format!("resolved:{number}"));
        thread =
            decide_guest_share_command(&thread, GuestShareCommand::Submit(&submit(&id.0)), &now())
                .unwrap();
        thread =
            decide_guest_share_command(&thread, GuestShareCommand::Dismiss(&id), &now()).unwrap();
    }
    assert_eq!(
        share(&thread).messages.len(),
        GUEST_RESOLVED_MESSAGE_LIMIT + 1
    );
    assert_eq!(
        share(&thread).messages[0].id,
        GuestMessageId("guest".into())
    );
    assert_eq!(
        share(&thread).messages[1].id,
        GuestMessageId("resolved:5".into())
    );
}

#[test]
fn guest_cleanup_releases_dropped_pending_files_and_preserves_sent_files() {
    let submit = change(
        &submit("guest"),
        json!({"attachments":[attachment("file",7)]}),
    );
    let thread =
        decide_guest_share_command(&redeemed(), GuestShareCommand::Submit(&submit), &now())
            .unwrap();
    let dismissed = decide_guest_share_command(
        &thread,
        GuestShareCommand::Dismiss(&GuestMessageId("guest".into())),
        &now(),
    )
    .unwrap();
    assert_eq!(
        released_guest_attachment_ids(Some(share(&thread)), Some(share(&dismissed))),
        vec!["file"]
    );
    let sent = decide_guest_message_send(&thread, &GuestMessageId("guest".into()), &now()).unwrap();
    assert!(
        released_guest_attachment_ids(Some(share(&thread)), Some(share(&sent.thread))).is_empty()
    );
    assert!(released_guest_attachment_ids(Some(share(&sent.thread)), None).is_empty());
    assert_eq!(
        released_guest_attachment_ids(Some(share(&thread)), None),
        vec!["file"]
    );
}

#[tokio::test]
async fn share_changes_commit_through_migrated_sqlite_and_cleanup_is_durable() {
    let harness = Harness::new(None, false);
    harness.create("thread").await.unwrap();
    let mut committed = harness.sink.subscribe();
    harness.dispatch(json!({"type":"thread.guest-share.open","commandId":"open","threadId":"thread","shareId":"share","host":"env.test","expiresAt":"2026-09-04T00:00:00.000Z"})).await.unwrap();
    assert!(committed.recv().await.is_ok());
    harness.dispatch(json!({"type":"thread.guest-share.redeem","commandId":"redeem","threadId":"thread","shareId":"share","guestName":"Sam"})).await.unwrap();
    harness.dispatch(json!({"type":"thread.guest-message.submit","commandId":"submit","threadId":"thread","shareId":"share","guestMessageId":"guest","text":"Why?","attachments":[attachment("file",7)]})).await.unwrap();
    let close = json!({"type":"thread.guest-share.close","commandId":"close","threadId":"thread"});
    harness.dispatch(close.clone()).await.unwrap();
    harness.dispatch(close).await.unwrap();
    assert!(
        harness
            .reader
            .thread(ThreadId("thread".into()))
            .await
            .unwrap()
            .unwrap()
            .guest_share
            .flatten()
            .is_none()
    );
    let effects=harness.database.stores.read(|db|Ok(db.query_row("SELECT count(*) FROM orchestration_v2_effect_outbox WHERE effect_type='attachment.cleanup'",[],|row|row.get::<_,i64>(0))?)).await.unwrap();
    assert_eq!(effects, 1);
    harness
        .dispatch(json!({"type":"thread.archive","commandId":"archive","threadId":"thread"}))
        .await
        .unwrap();
    assert!(harness.dispatch(json!({"type":"thread.guest-share.open","commandId":"open-again","threadId":"thread","shareId":"share2","host":"env.test","expiresAt":"2026-09-04T00:00:00.000Z"})).await.is_err());
}
