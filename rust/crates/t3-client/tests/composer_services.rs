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
    let target = ThreadKey { thread: ThreadId("first".into()), ..key() };
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
