use futures::{
    FutureExt, channel::oneshot, executor::block_on, future::LocalBoxFuture, stream::LocalBoxStream,
};
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc};
use t3_client::{
    platform::*,
    testing::FakePlatform,
    thread_views::{
        composer::mobile_store::MobileDraftStore,
        drafts::{DraftFormat, SavedDraftDocument, storage_key},
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Boundary {
    Read,
    Commit,
}
struct Gate {
    boundary: Boundary,
    started: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
}
#[derive(Default)]
struct PausedHost {
    host: FakePlatform,
    gate: RefCell<Option<Gate>>,
}
impl PausedHost {
    fn pause_next(&self, boundary: Boundary) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (started, observed) = oneshot::channel();
        let (resume, paused) = oneshot::channel();
        *self.gate.borrow_mut() = Some(Gate {
            boundary,
            started,
            resume: paused,
        });
        (observed, resume)
    }
    async fn boundary(&self, boundary: Boundary) {
        let gate = {
            let mut gate = self.gate.borrow_mut();
            if gate.as_ref().is_some_and(|gate| gate.boundary == boundary) {
                gate.take()
            } else {
                None
            }
        };
        if let Some(gate) = gate {
            let _ = gate.started.send(());
            gate.resume.await.expect("resume host I/O");
        }
    }
    async fn saved(&self) -> Value {
        let bytes = self
            .host
            .get(StorageArea::Ordinary, storage_key(DraftFormat::Mobile))
            .await
            .expect("read")
            .expect("saved");
        SavedDraftDocument::decode_mobile(&bytes)
            .expect("document")
            .raw()
            .clone()
    }
}
impl Storage for PausedHost {
    fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
        async move {
            let draft = key == storage_key(DraftFormat::Mobile);
            let value = self.host.get(area, key).await?;
            if draft {
                self.boundary(Boundary::Read).await;
            }
            Ok(value)
        }
        .boxed_local()
    }
    fn write(&self, area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()> {
        self.host.write(area, writes)
    }
    fn list(&self, area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>> {
        self.host.list(area, namespace)
    }
    fn transaction(&self, transaction: StorageTransaction) -> HostFuture<'_, bool> {
        async move {
            if transaction
                .ordinary
                .iter()
                .any(|write| write.key == storage_key(DraftFormat::Mobile))
            {
                self.boundary(Boundary::Commit).await;
            }
            self.host.transaction(transaction).await
        }
        .boxed_local()
    }
}
impl Platform for PausedHost {
    fn now_epoch_ms(&self) -> u64 {
        self.host.now_epoch_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.host.monotonic_ms()
    }
    fn random_bytes(&self, length: usize) -> Result<Vec<u8>, PlatformError> {
        self.host.random_bytes(length)
    }
    fn sleep(&self, milliseconds: u64) -> LocalBoxFuture<'_, ()> {
        self.host.sleep(milliseconds)
    }
    fn yield_to_host(&self) -> LocalBoxFuture<'_, ()> {
        self.host.yield_to_host()
    }
    fn events(&self) -> LocalBoxStream<'static, PlatformEvent> {
        self.host.events()
    }
    fn http(&self, request: HttpRequest) -> HostFuture<'_, HttpResponse> {
        self.host.http(request)
    }
    fn websocket(&self, request: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>> {
        self.host.websocket(request)
    }
    fn host(&self, action: HostAction) -> HostFuture<'_, HostReply> {
        self.host.host(action)
    }
}
fn replace(store: &MobileDraftStore<PausedHost>, key: &str, text: &str) {
    store
        .update(|current| {
            let mut next = current.clone();
            next["drafts"][key] = json!({"text":text,"attachments":[]});
            Ok(next)
        })
        .expect("edit");
}

#[test]
fn edits_while_hydration_reads_preserve_live_text_and_other_saved_drafts() {
    block_on(async {
        let host = Rc::new(PausedHost::default());
        host.host.write(StorageArea::Ordinary,vec![StorageWrite {key:storage_key(DraftFormat::Mobile),value:Some(serde_json::to_vec(&json!({"schemaVersion":1,"drafts":{"key":{"text":"saved","attachments":[]},"other":{"text":"other","attachments":[]}}})).expect("json"))}]).await.expect("seed");
        let store = MobileDraftStore::new(host.clone());
        let (started, resume) = host.pause_next(Boundary::Read);
        let typing = async {
            started.await.expect("read suspended");
            replace(&store, "key", "typed during hydration");
            resume.send(()).expect("resume");
        };
        let (result, ()) = futures::join!(store.load(), typing);
        result.expect("hydrate");
        assert_eq!(
            store.snapshot()["drafts"]["key"]["text"],
            "typed during hydration"
        );
        assert_eq!(store.snapshot()["drafts"]["other"]["text"], "other");
        store.flush_all().await.expect("save");
        assert_eq!(
            host.saved().await["drafts"]["key"]["text"],
            "typed during hydration"
        );
    });
}
#[test]
fn edits_while_commit_is_suspended_remain_dirty_until_the_next_serialized_write() {
    block_on(async {
        let host = Rc::new(PausedHost::default());
        let store = MobileDraftStore::new(host.clone());
        store.load().await.expect("load");
        replace(&store, "key", "first snapshot");
        let (started, resume) = host.pause_next(Boundary::Commit);
        let typing = async {
            started.await.expect("commit suspended");
            replace(&store, "key", "newer typing");
            resume.send(()).expect("resume");
        };
        let (result, ()) = futures::join!(store.flush(), typing);
        assert!(result.expect("commit"));
        assert_eq!(
            host.saved().await["drafts"]["key"]["text"],
            "first snapshot"
        );
        assert_eq!(store.snapshot()["drafts"]["key"]["text"], "newer typing");
        assert!(store.deadline().is_some());
        store.flush_all().await.expect("drain");
        assert_eq!(host.saved().await["drafts"]["key"]["text"], "newer typing");
        assert!(store.deadline().is_none());
    });
}
#[test]
fn failed_compare_and_swap_retains_local_changes_and_retry_keeps_the_other_writer() {
    block_on(async {
        let host = Rc::new(PausedHost::default());
        let store = MobileDraftStore::new(host.clone());
        store.load().await.expect("load");
        replace(&store, "key", "local");
        let (started, resume) = host.pause_next(Boundary::Commit);
        let other = async {
            started.await.expect("commit suspended");
            host.host.write(StorageArea::Ordinary,vec![StorageWrite {key:storage_key(DraftFormat::Mobile),value:Some(serde_json::to_vec(&json!({"schemaVersion":1,"drafts":{"other":{"text":"other writer","attachments":[]}},"future":true})).expect("json"))}]).await.expect("other write");
            resume.send(()).expect("resume");
        };
        let (result, ()) = futures::join!(store.flush(), other);
        assert!(result.is_err());
        assert!(store.deadline().is_some());
        assert_eq!(store.snapshot()["drafts"]["key"]["text"], "local");
        store.flush_all().await.expect("retry");
        let saved = host.saved().await;
        assert_eq!(saved["drafts"]["key"]["text"], "local");
        assert_eq!(saved["drafts"]["other"]["text"], "other writer");
        assert_eq!(saved["future"], true);
        let mut corrupt = saved.clone();
        corrupt["modelOptionMemory"] = json!({"codex":{"model":"unreadable"}});
        host.host
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: storage_key(DraftFormat::Mobile),
                    value: Some(serde_json::to_vec(&corrupt).expect("json")),
                }],
            )
            .await
            .expect("other writer corrupt metadata");
        replace(&store, "key", "new local edit");
        assert!(store.flush_all().await.is_err());
        assert_eq!(host.saved().await, corrupt);
        assert!(store.deadline().is_some());
        corrupt["modelOptionMemory"] = json!({"codex":{"model":[{"id":"fastMode","value":false}]}});
        host.host
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: storage_key(DraftFormat::Mobile),
                    value: Some(serde_json::to_vec(&corrupt).expect("json")),
                }],
            )
            .await
            .expect("repair metadata");
        store.flush_all().await.expect("retry after repair");
        let saved = host.saved().await;
        assert_eq!(saved["drafts"]["key"]["text"], "new local edit");
        assert_eq!(saved["modelOptionMemory"], corrupt["modelOptionMemory"]);
    });
}
