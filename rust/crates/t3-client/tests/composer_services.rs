#![allow(clippy::unwrap_used)]
#[path = "support/fake_server.rs"]
mod fake_server;

use fake_server::Harness;
use futures::{FutureExt, StreamExt};
use serde_json::json;
use std::rc::Rc;
use t3_client::{platform::{Storage, StorageArea, StorageWrite}, thread_views::{composer::draft_service::ComposerDraftService, drafts::{DraftFormat, SavedDraftDocument, storage_key}}, types::{EnvironmentId, ThreadId, ThreadKey}};

fn key() -> ThreadKey { ThreadKey { environment: EnvironmentId("env".into()), thread: ThreadId("thread".into()) } }

#[test]
fn web_draft_hydration_cursor_and_captured_revision_preserve_new_edits() {
    let mut h = Harness::new("env");
    let drafts = Rc::new(ComposerDraftService::new(Rc::new(h.platform.clone())));
    let load = drafts.clone();
    h.run(async move { load.load().await.unwrap(); });
    drafts.set_prompt(&key(), "first 🦀".into(), 8).unwrap();
    let captured = drafts.view(&key());
    assert_eq!(captured.cursor, 8);
    assert!(Rc::ptr_eq(&captured, &drafts.view(&key())));
    drafts.set_prompt(&key(), "typed meanwhile".into(), 99).unwrap();
    assert!(!drafts.clear_revision(&key(), captured.revision).unwrap());
    assert_eq!(&*drafts.view(&key()).prompt, "typed meanwhile");
    let save = drafts.clone(); h.run(async move { save.flush().await.unwrap(); });
    let fresh = Rc::new(ComposerDraftService::new(Rc::new(h.platform.clone())));
    let load = fresh.clone(); h.run(async move { load.load().await.unwrap(); });
    assert_eq!(&*fresh.view(&key()).prompt, "typed meanwhile");
    assert_eq!(fresh.view(&key()).cursor, 15);
    assert!(fresh.clear_revision(&key(), fresh.view(&key()).revision).unwrap());
}

#[test]
fn unreadable_web_document_is_retained_and_other_writers_are_merged() {
    let mut h = Harness::new("env");
    let platform = h.platform.clone();
    h.run(async move { platform.write(StorageArea::Ordinary, vec![StorageWrite { key: storage_key(DraftFormat::Web), value: Some(b"broken".to_vec()) }]).await.unwrap(); });
    let drafts = Rc::new(ComposerDraftService::new(Rc::new(h.platform.clone())));
    let load = drafts.clone(); assert!(h.run(async move { load.load().await }).is_err());
    assert!(!drafts.view(&key()).writable);
    let platform = h.platform.clone();
    assert_eq!(h.run(async move { platform.get(StorageArea::Ordinary, storage_key(DraftFormat::Web)).await.unwrap() }), Some(b"broken".to_vec()));
    let platform = h.platform.clone();
    h.run(async move { platform.write(StorageArea::Ordinary, vec![StorageWrite { key: storage_key(DraftFormat::Web), value: Some(serde_json::to_vec(&json!({"version":9,"state":{"draftsByThreadKey":{},"draftThreadsByThreadKey":{},"logicalProjectDraftThreadKeyByLogicalProjectKey":{},"future":{"keep":true}}})).unwrap()) }]).await.unwrap(); });
    let load = drafts.clone(); h.run(async move { load.load().await.unwrap(); });
    let other = Rc::new(ComposerDraftService::new(Rc::new(h.platform.clone())));
    let load = other.clone(); h.run(async move { load.load().await.unwrap(); });
    let mut changes = drafts.changes(); assert!(changes.next().now_or_never().unwrap().is_some());
    drafts.set_prompt(&key(), "local".into(), 5).unwrap();
    assert!(changes.next().now_or_never().unwrap().is_some());
    let second = ThreadKey { thread: ThreadId("other".into()), ..key() };
    other.set_prompt(&second, "remote".into(), 6).unwrap();
    h.run(async move { other.flush().await.unwrap(); });
    let save = drafts.clone(); h.run(async move { save.flush().await.unwrap(); });
    let platform = h.platform.clone();
    let bytes = h.run(async move { platform.get(StorageArea::Ordinary, storage_key(DraftFormat::Web)).await.unwrap().unwrap() });
    let saved = SavedDraftDocument::decode_web(&bytes).unwrap();
    assert_eq!(saved.state()["draftsByThreadKey"]["env:thread"]["prompt"], "local");
    assert_eq!(saved.state()["draftsByThreadKey"]["env:other"]["prompt"], "remote");
    assert_eq!(saved.state()["future"]["keep"], true);
}

#[test]
fn composed_send_restores_a_refusal_beside_a_newer_draft_and_preserves_the_reason() {
    use t3_client::{ClientStore, composition::ClientRuntime, connection::PairingInput};
    use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
    let mut h = Harness::new("env");
    let runtime = ClientRuntime::new(h.platform.clone(), ClientStore::default(), OrchestrationV2CreationSource::Web, h.spawner());
    h.run(runtime.pair(PairingInput { pairing_url: Some(format!("{}/pair#token=pairing-token", fake_server::ORIGIN)), ..Default::default() })).unwrap();
    h.settle();
    let target = ThreadKey { thread: ThreadId("t1".into()), ..key() };
    runtime.open_thread(&target);
    h.settle();
    let drafts = runtime.composer_drafts();
    drafts.set_prompt(&target, "refused prompt".into(), 14).unwrap();
    h.server().state.borrow_mut().hold_dispatches = true;
    h.run(runtime.send_message(&target, "refused prompt".into())).unwrap();
    assert!(drafts.view(&target).prompt.is_empty());
    drafts.set_prompt(&target, "typed meanwhile".into(), 15).unwrap();
    h.server().hang_up();
    h.settle();
    assert_eq!(&*drafts.view(&target).prompt, "typed meanwhile");
    drop(runtime);
    h.settle();
}

#[test]
fn draft_subscription_delivers_an_edit_then_ends_with_its_authority() {
    let h = Harness::new("env");
    let drafts = ComposerDraftService::new(Rc::new(h.platform.clone()));
    let mut changes = drafts.changes();
    assert_eq!(changes.next().now_or_never(), Some(Some(())));
    drafts.set_prompt(&key(), "edit".into(), 4).unwrap();
    assert_eq!(changes.next().now_or_never(), Some(Some(())));
    drop(drafts);
    assert_eq!(changes.next().now_or_never(), Some(None));
}

#[test]
fn primary_actions_and_interrupt_recheck_destination_permission_and_preserve_rejection_detail() {
    use t3_client::{ClientStore, ClientError, composition::ClientRuntime, connection::PairingInput, permissions::SessionGrants};
    use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
    let mut h = Harness::new("env");
    let runtime = ClientRuntime::new(h.platform.clone(), ClientStore::default(), OrchestrationV2CreationSource::Web, h.spawner());
    let target = ThreadKey { thread: ThreadId("t1".into()), ..key() };
    let actions = runtime.composer_primary_actions(&target);
    assert!(actions.snapshot().is_environment_unavailable);
    assert!(!actions.snapshot().can_operate_thread);
    h.run(runtime.pair(PairingInput { pairing_url: Some(format!("{}/pair#token=pairing-token", fake_server::ORIGIN)), ..Default::default() })).unwrap();
    runtime.open_thread(&target); h.settle();
    let drafts = runtime.composer_drafts();
    drafts.set_prompt(&target, "hello".into(), 5).unwrap();
    assert!(actions.snapshot().has_sendable_content);
    assert!(actions.snapshot().can_operate_thread);
    let endpoint = runtime.client(&target.environment).unwrap().session().unwrap();
    let platform = h.platform.clone(); let store = runtime.store().clone(); let key = target.clone(); let session = endpoint.clone();
    let error = h.run(async move { t3_client::thread_views::composer::interrupt::interrupt_thread(&platform, &store, session, &key, true).await }).unwrap_err();
    assert!(matches!(error, ClientError::Rejected { .. }));
    assert!(error.to_string().contains("No orchestration provider adapter is registered for codex."));
    endpoint.update_grants(SessionGrants { valid: true, scopes: Default::default() });
    assert!(!actions.snapshot().can_operate_thread);
    assert_eq!(actions.snapshot().send_disabled_reason.as_deref(), Some("This connection cannot change threads."));
    assert!(matches!(h.run(runtime.interrupt_thread(&target)), Err(ClientError::PermissionDenied { .. })));
}
