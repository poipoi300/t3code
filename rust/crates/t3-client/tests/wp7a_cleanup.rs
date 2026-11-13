use futures::{FutureExt, executor::block_on, future::LocalBoxFuture};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use t3_client::{
    ClientError,
    platform::PlatformError,
    thread_views::composer::{
        attachment_files::AttachmentRetention, cleanup::*, mobile_drafts::AttachmentReferences,
    },
    types::EnvironmentId,
};

#[derive(Default)]
struct Owners {
    ready: Cell<bool>,
    shares_readable: Cell<bool>,
    references: Rc<RefCell<AttachmentReferences>>,
    shares: RefCell<Vec<Value>>,
    events: RefCell<Vec<&'static str>>,
}
impl CleanupSources for Owners {
    fn hydrate_and_flush(&self) -> LocalBoxFuture<'_, Result<bool, ClientError>> {
        async {
            self.events.borrow_mut().push("flush");
            Ok(self.ready.get())
        }
        .boxed_local()
    }
    fn references(&self) -> Option<AttachmentReferences> {
        self.ready.get().then(|| self.references.borrow().clone())
    }
    fn incoming_shares(&self) -> LocalBoxFuture<'_, Result<Vec<Value>, ClientError>> {
        async {
            self.events.borrow_mut().push("shares");
            if self.shares_readable.get() {
                Ok(self.shares.borrow().clone())
            } else {
                Err(ClientError::InvalidPayload(
                    "unreadable incoming shares".into(),
                ))
            }
        }
        .boxed_local()
    }
}
#[derive(Default)]
struct Removal {
    files: RefCell<Vec<String>>,
    uploads: RefCell<Vec<(EnvironmentId, String)>>,
    fail: Cell<bool>,
    acquire_on_first: RefCell<Option<(Rc<RefCell<AttachmentReferences>>, String)>>,
}
impl CleanupRemoval for Removal {
    fn file(&self, uri: String) -> LocalBoxFuture<'_, Result<(), ClientError>> {
        async move {
            if self.fail.get() {
                return Err(PlatformError::Closed.into());
            }
            self.files.borrow_mut().push(uri);
            if let Some((references, key)) = self.acquire_on_first.borrow_mut().take() {
                references.borrow_mut().files.insert(key);
            }
            Ok(())
        }
        .boxed_local()
    }
    fn upload(
        &self,
        environment: EnvironmentId,
        id: String,
    ) -> LocalBoxFuture<'_, Result<(), ClientError>> {
        async move {
            self.uploads.borrow_mut().push((environment, id));
            Ok(())
        }
        .boxed_local()
    }
}
const ROOT: &str = "file:///app/Documents/";
fn attachment(name: &str) -> Value {
    json!({"fileUri":format!("{ROOT}t3-composer-attachments/{name}"),"uploadEnvironmentId":"env","uploadedAttachmentId":name})
}

#[test]
fn cleanup_waits_for_every_owner_and_strict_share_read_before_deleting() {
    block_on(async {
        let cleanup = AttachmentCleanup::default();
        cleanup.schedule(&[attachment("a")]);
        let owners = Owners::default();
        let removal = Removal::default();
        let retention = AttachmentRetention::default();
        assert!(
            cleanup
                .run_once(&owners, &removal, &retention, ROOT)
                .await
                .expect("load")
                .deferred
        );
        assert_eq!(*owners.events.borrow(), vec!["flush"]);
        owners.ready.set(true);
        assert!(
            cleanup
                .run_once(&owners, &removal, &retention, ROOT)
                .await
                .expect("shares")
                .deferred
        );
        assert!(removal.files.borrow().is_empty());
        assert!(removal.uploads.borrow().is_empty());
        assert_eq!(cleanup.pending(), 2);
        owners.shares_readable.set(true);
        owners
            .shares
            .borrow_mut()
            .push(json!({"attachments":[attachment("a")]}));
        let report = cleanup
            .run_once(&owners, &removal, &retention, ROOT)
            .await
            .expect("share owns");
        assert!(report.files_removed.is_empty());
        assert!(report.uploads_removed.is_empty());
        owners.shares.borrow_mut().clear();
        let report = cleanup
            .run_once(&owners, &removal, &retention, ROOT)
            .await
            .expect("release");
        assert_eq!(report.files_removed.len(), 1);
        assert_eq!(report.uploads_removed.len(), 1);
        assert_eq!(cleanup.pending(), 0);
    });
}
#[test]
fn cleanup_rechecks_live_ownership_after_each_asynchronous_deletion_and_retains_failures() {
    block_on(async {
        let cleanup = AttachmentCleanup::default();
        cleanup.schedule(&[
            attachment("a"),
            attachment("b"),
            json!({"fileUri":"file:///foreign/file"}),
        ]);
        let owners = Owners::default();
        owners.ready.set(true);
        owners.shares_readable.set(true);
        let removal = Removal::default();
        let retention = AttachmentRetention::default();
        let second = attachment("b")["fileUri"]
            .as_str()
            .expect("uri")
            .to_string();
        *removal.acquire_on_first.borrow_mut() = Some((owners.references.clone(), second.clone()));
        let report = cleanup
            .run_once(&owners, &removal, &retention, ROOT)
            .await
            .expect("cleanup");
        assert_eq!(report.files_removed.len(), 1);
        assert_eq!(removal.files.borrow().len(), 1);
        assert_eq!(cleanup.pending(), 1);
        owners.references.borrow_mut().files.clear();
        removal.fail.set(true);
        assert_eq!(
            cleanup
                .run_once(&owners, &removal, &retention, ROOT)
                .await
                .expect("failure")
                .failures
                .len(),
            1
        );
        assert_eq!(cleanup.pending(), 1);
        removal.fail.set(false);
        let preview = retention.retain(&second, || {});
        assert!(
            cleanup
                .run_once(&owners, &removal, &retention, ROOT)
                .await
                .expect("retained")
                .files_removed
                .is_empty()
        );
        drop(preview);
        assert_eq!(
            cleanup
                .run_once(&owners, &removal, &retention, ROOT)
                .await
                .expect("released")
                .files_removed,
            vec![second]
        );
        assert_eq!(cleanup.pending(), 0);
    });
}
#[test]
fn marked_host_shim_retains_the_resource_until_real_host_removal_is_available() {
    assert!(matches!(
        block_on(PendingResourceRemoval.remove_resource("owned".into())),
        Err(PlatformError::Unsupported(_))
    ));
}

#[test]
fn stored_sources_flush_real_drafts_and_outbox_and_include_persisted_and_active_shares() {
    block_on(async {
        use t3_client::{
            ClientStore,
            platform::{Storage, StorageArea, StorageKey, StorageWrite},
            rpc::SessionEndpoint,
            testing::FakePlatform,
            thread_views::composer::{mobile_store::MobileDraftStore, send::SendService},
        };
        let platform = Rc::new(FakePlatform::default());
        let drafts = Rc::new(MobileDraftStore::new(platform.clone()));
        drafts.load().await.expect("drafts");
        drafts
            .update(|current| {
                let mut next = current.clone();
                next["drafts"]["env:thread"] =
                    json!({"text":"draft","attachments":[attachment("a")]});
                Ok(next)
            })
            .expect("edit");
        let sends = SendService::<_, SessionEndpoint>::new(
            EnvironmentId("env".into()),
            platform.clone(),
            ClientStore::default(),
            t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web,
        );
        sends.attach_drafts(drafts.clone());
        let sources = StoredCleanupSources::new(drafts.clone(), sends).expect("sources");
        let share_key = StorageKey {
            namespace: "incoming-share".into(),
            environment: None,
            record: "incoming-shares/share.json".into(),
        };
        let share = t3_client::persistence::Record::new(
            "incoming-share",
            None,
            &share_key.record,
            1,
            json!({"schemaVersion":1,"id":"share","text":"shared","createdAt":"then","warnings":[],"attachments":[{"type":"file","id":"b","name":"b.txt","mimeType":"text/plain","sizeBytes":1,"fileUri":attachment("b")["fileUri"]}]}),
        );
        platform
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: share_key.clone(),
                    value: Some(serde_json::to_vec(&share).expect("json")),
                }],
            )
            .await
            .expect("share");
        let active = sources.retain_import(json!({"attachments":[attachment("c")]}));
        let cleanup = AttachmentCleanup::default();
        cleanup.schedule(&[attachment("a"), attachment("b"), attachment("c")]);
        let removal = Removal::default();
        let retention = AttachmentRetention::default();
        let unreadable = StorageKey {
            namespace: "outbox".into(),
            environment: Some(EnvironmentId("env".into())),
            record: "unreadable".into(),
        };
        platform
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: unreadable.clone(),
                    value: Some(b"unreadable queued message".to_vec()),
                }],
            )
            .await
            .expect("unreadable outbox");
        assert!(
            cleanup
                .run_once(&sources, &removal, &retention, ROOT)
                .await
                .expect("defer unreadable owner")
                .deferred
        );
        assert!(removal.files.borrow().is_empty());
        platform
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: unreadable,
                    value: None,
                }],
            )
            .await
            .expect("repair outbox");
        let report = cleanup
            .run_once(&sources, &removal, &retention, ROOT)
            .await
            .expect("protected");
        assert!(!report.deferred);
        assert!(report.files_removed.is_empty());
        assert!(drafts.deadline().is_none());
        drafts
            .update(|current| {
                let mut next = current.clone();
                next["drafts"] = json!({});
                Ok(next)
            })
            .expect("clear");
        platform
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: share_key,
                    value: None,
                }],
            )
            .await
            .expect("ack share");
        drop(active);
        let report = cleanup
            .run_once(&sources, &removal, &retention, ROOT)
            .await
            .expect("release");
        assert_eq!(report.files_removed.len(), 3);
        assert_eq!(cleanup.pending(), 0);
    });
}
