use futures::executor::block_on;
use serde_json::json;
use std::rc::Rc;
use t3_client::{
    platform::{Storage, StorageArea},
    testing::FakePlatform,
    thread_views::{
        composer::{mobile_content, mobile_store::MobileDraftStore, web_sessions},
        drafts::{DraftFormat, storage_key},
    },
};
use t3_contracts::environment::ScopedProjectRef;
#[test]
fn new_task_lifecycle_loads_saved_drafts_before_retarget_and_flushes_environment_removal() {
    block_on(async {
        let platform = Rc::new(FakePlatform::default());
        let store = MobileDraftStore::new(platform.clone());
        let project: ScopedProjectRef =
            serde_json::from_value(json!({"environmentId":"env","projectId":"p"}))
                .expect("project");
        assert!(
            !store
                .retarget_saved("new-task:missing", &project)
                .await
                .expect("load")
        );
        assert!(
            store.snapshot()["drafts"]
                .as_object()
                .expect("drafts")
                .is_empty()
        );
        let key = store.create_new_task(&project).expect("create");
        let old_operation = store.operations.start(&key);
        let empty_operation = store.operations.start("new:empty");
        let created = store.snapshot()["drafts"][&key]["project"]["createdAt"].clone();
        store.update(|current|{let mut next=current.clone();next["drafts"][&key]["attachments"]=json!([{"type":"file","id":"a","name":"a.txt","mimeType":"text/plain","sizeBytes":1,"fileUri":"file:///owned","uploadEnvironmentId":"env","uploadedAttachmentId":"uploaded"}]);next["drafts"]["other:t"]=json!({"text":"other","attachments":[]});Ok(next)}).expect("edit");
        let moved: ScopedProjectRef =
            serde_json::from_value(json!({"environmentId":"new","projectId":"new-p"}))
                .expect("project");
        assert!(store.retarget_saved(&key, &moved).await.expect("move"));
        assert_eq!(
            store.snapshot()["drafts"][&key]["project"]["createdAt"],
            created
        );
        assert!(
            store.snapshot()["drafts"][&key]["attachments"][0]
                .get("uploadedAttachmentId")
                .is_none()
        );
        assert_eq!(
            mobile_content::new_task_keys(&store.snapshot()["drafts"], "new", "new-p"),
            vec![key.clone()]
        );
        assert!(
            store
                .clear_environment("env")
                .await
                .expect("old environment")
                .is_empty()
        );
        assert!(store.snapshot()["drafts"].get(&key).is_some());
        let cleanup = store
            .clear_environment("new")
            .await
            .expect("new environment");
        assert!(old_operation.is_aborted());
        assert!(empty_operation.is_aborted());
        assert_eq!(cleanup.len(), 1);
        let bytes = platform
            .get(StorageArea::Ordinary, storage_key(DraftFormat::Mobile))
            .await
            .expect("read")
            .expect("saved");
        let saved: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert!(saved["drafts"].get(&key).is_none());
        assert_eq!(saved["drafts"]["other:t"]["text"], "other");
    });
}
#[test]
fn discarded_draft_cancels_suspended_work_and_old_completion_cannot_unregister_new_operations() {
    use futures::{Future, future::pending, task::noop_waker_ref};
    use std::{
        pin::pin,
        task::{Context, Poll},
    };
    let store = MobileDraftStore::new(Rc::new(FakePlatform::default()));
    let operation = store.operations.start("env:t");
    let mut running = pin!(operation.run(pending::<()>()));
    let mut context = Context::from_waker(noop_waker_ref());
    assert!(running.as_mut().poll(&mut context).is_pending());
    store.clear_draft("env:t").expect("clear");
    let newer = store.operations.start("env:t");
    assert!(matches!(
        running.as_mut().poll(&mut context),
        Poll::Ready(Err(_))
    ));
    assert!(!newer.is_aborted());
    store.clear_draft("env:t").expect("clear newer");
    assert!(newer.is_aborted());
}
#[test]
fn web_environment_teardown_clears_reserved_sessions_mappings_transitions_and_blob_owners() {
    let mut state = json!({"draftsByThreadKey":{"reserved":{"images":[{"previewUrl":"blob:reserved"}]},"env:t":{"prompt":"remove"},"other:t":{"prompt":"keep"},"mapped":{"images":[{"previewUrl":"blob:mapped"}]}},"draftThreadsByThreadKey":{"reserved":{"environmentId":"env"},"kept":{"environmentId":"other"}},"logicalProjectDraftThreadKeyByLogicalProjectKey":{"env:p":"mapped","other:p":"reserved","other:p2":"kept"},"backgroundSubmissionThreadKeys":{"env:t":true,"other:t":true},"rewindingThreadKeys":["env:t","other:t"],"promotedDraftRedirects":{"a":{"environmentId":"env"},"b":{"environmentId":"other"}}});
    let mut released = web_sessions::clear_environment(&mut state, "env");
    released.sort();
    assert_eq!(released, vec!["blob:mapped", "blob:reserved"]);
    assert_eq!(
        state["draftsByThreadKey"],
        json!({"other:t":{"prompt":"keep"}})
    );
    assert_eq!(
        state["logicalProjectDraftThreadKeyByLogicalProjectKey"],
        json!({"other:p2":"kept"})
    );
    assert_eq!(
        state["backgroundSubmissionThreadKeys"],
        json!({"other:t":true})
    );
    assert_eq!(state["rewindingThreadKeys"], json!(["other:t"]));
    assert!(state["promotedDraftRedirects"].get("a").is_none());
    let mut draft =
        json!({"branch":"old","checkoutPullRequest":{"branch":"new","url":"https://example/pr/1"}});
    assert!(web_sessions::checkout_pull_request_url(&draft).is_none());
    draft["branch"] = json!("new");
    assert_eq!(
        web_sessions::checkout_pull_request_url(&draft),
        Some("https://example/pr/1")
    );
    draft["branch"] = json!("old");
    assert!(web_sessions::checkout_pull_request_url(&draft).is_none());
}
