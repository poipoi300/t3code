//! Shared mobile-format draft ownership with hydration, coalesced changes, and atomic rebased writes.
use super::{mobile_drafts, send::Changes};
use crate::{
    ClientError,
    platform::{Platform, StorageArea, StorageCondition, StorageTransaction, StorageWrite},
    thread_views::drafts::{
        DraftFormat, MOBILE_DEBOUNCE_MS, SavedDraftDocument, merge_map, storage_key,
    },
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, lock::Mutex, stream::LocalBoxStream};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    rc::Rc,
};

pub fn merge_documents(
    stored: &Value,
    local: &Value,
    baseline: &Value,
) -> Result<Value, ClientError> {
    if !stored.is_object() || !local.is_object() || !baseline.is_object() {
        return Err(ClientError::InvalidPayload(
            "Unreadable draft document retained for recovery.".into(),
        ));
    }
    let mut merged = merge_map(Some(stored), Some(local), Some(baseline));
    for field in ["drafts", "signedOutDrafts", "recoveredMessages"] {
        if [stored, local, baseline]
            .iter()
            .any(|document| document.get(field).is_some())
            && [stored, local, baseline]
                .iter()
                .all(|document| document.get(field).is_none_or(Value::is_object))
        {
            merged[field] = merge_map(stored.get(field), local.get(field), baseline.get(field));
        }
    }
    let keys: [&str; 2] = ["modelOptionMemory", "modelOptionsByModelByProvider"];
    for field in keys {
        if [stored, local, baseline]
            .iter()
            .any(|document| document.get(field).is_some_and(|value| !value.is_object()))
        {
            continue;
        }
        let instances: BTreeSet<_> = [stored, local, baseline]
            .iter()
            .filter_map(|document| document[field].as_object())
            .flat_map(|instances| instances.keys().cloned())
            .collect();
        if instances.is_empty() {
            continue;
        }
        merged[field] = json!({});
        for instance in instances {
            merged[field][&instance] = merge_map(
                stored[field].get(&instance),
                local[field].get(&instance),
                baseline[field].get(&instance),
            );
        }
    }
    Ok(merged)
}

struct State {
    current: Rc<Value>,
    baseline: Rc<Value>,
    deadline: Option<u64>,
}
pub struct MobileDraftStore<P: Platform> {
    pub(super) platform: Rc<P>,
    pub operations: super::mobile_operations::DraftOperations,
    pub selections: RefCell<super::mobile_selection::SelectionRegistry>,
    state: RefCell<State>,
    loaded: Cell<bool>,
    loading: Mutex<()>,
    writing: Mutex<()>,
    changes: Changes,
    disposed: Cell<bool>,
    running: Cell<bool>,
}
impl<P: Platform> MobileDraftStore<P> {
    pub fn new(platform: Rc<P>) -> Self {
        let empty = Rc::new(json!({"schemaVersion":1,"drafts":{}}));
        Self {
            platform,
            operations: Default::default(),
            selections: Default::default(),
            state: RefCell::new(State {
                current: empty.clone(),
                baseline: empty,
                deadline: None,
            }),
            loaded: Cell::new(false),
            loading: Mutex::new(()),
            writing: Mutex::new(()),
            changes: Changes::default(),
            disposed: Cell::new(false),
            running: Cell::new(false),
        }
    }
    pub fn snapshot(&self) -> Rc<Value> {
        self.state.borrow().current.clone()
    }
    pub fn create_new_task(
        &self,
        project: &t3_contracts::environment::ScopedProjectRef,
    ) -> Result<String, ClientError> {
        let id = crate::operations::allocate_command_id(self.platform.as_ref(), None)?;
        let now = crate::time::iso_from_epoch_ms(self.platform.now_epoch_ms());
        let mut key = String::new();
        self.update(|current| {
            let mut document = current.clone();
            let (drafts, created) = super::mobile_content::new_task(
                &current["drafts"],
                &id.0,
                &project.environment_id.0,
                &project.project_id.0,
                &now,
            );
            document["drafts"] = drafts;
            key = created;
            Ok(document)
        })?;
        Ok(key)
    }
    pub async fn retarget_saved(
        &self,
        key: &str,
        project: &t3_contracts::environment::ScopedProjectRef,
    ) -> Result<bool, ClientError> {
        self.retarget_saved_with_options(key, project, false).await
    }
    pub async fn retarget_saved_with_options(
        &self,
        key: &str,
        project: &t3_contracts::environment::ScopedProjectRef,
        keep_pull_request_workspace: bool,
    ) -> Result<bool, ClientError> {
        self.load().await?;
        let now = crate::time::iso_from_epoch_ms(self.platform.now_epoch_ms());
        let mut found = false;
        self.update(|current| {
            let Some(draft) = current["drafts"].get(key) else {
                return Ok(current.clone());
            };
            found = true;
            let mut document = current.clone();
            document["drafts"][key] = super::mobile_content::retarget_with_options(
                draft,
                &project.environment_id.0,
                &project.project_id.0,
                &now,
                keep_pull_request_workspace,
            );
            Ok(document)
        })?;
        Ok(found)
    }
    pub async fn clear_environment(&self, environment: &str) -> Result<Vec<Value>, ClientError> {
        self.load().await?;
        let mut cleanup = vec![];
        let snapshot = self.snapshot();
        let retained = super::mobile_content::remove_environment(&snapshot["drafts"], environment);
        self.operations.abort_where(|key| {
            key.starts_with(&format!("{environment}:"))
                || snapshot["drafts"].get(key).is_some() && retained.get(key).is_none()
        });
        self.update(|current| {
            let mut next = current.clone();
            next["drafts"] =
                super::mobile_content::remove_environment(&current["drafts"], environment);
            if let Some(drafts) = current["drafts"].as_object() {
                for (_, draft) in drafts
                    .iter()
                    .filter(|(key, _)| next["drafts"].get(*key).is_none())
                {
                    cleanup.extend(
                        draft["attachments"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .cloned(),
                    );
                }
            }
            Ok(next)
        })?;
        self.flush_all().await?;
        Ok(cleanup)
    }
    pub fn clear_draft(&self, key: &str) -> Result<Vec<Value>, ClientError> {
        self.operations.abort(key);
        self.selections.borrow_mut().reset(key)?;
        let mut cleanup = vec![];
        self.update(|current| {
            let mut next = current.clone();
            if let Some(draft) = next["drafts"]
                .as_object_mut()
                .and_then(|drafts| drafts.remove(key))
            {
                cleanup.extend(
                    draft["attachments"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .cloned(),
                );
            }
            Ok(next)
        })?;
        Ok(cleanup)
    }
    pub fn clear_content(
        &self,
        key: &str,
        clear_model: bool,
        clear_workspace: bool,
    ) -> Result<Vec<Value>, ClientError> {
        self.selections.borrow_mut().reset(key)?;
        let mut cleanup = vec![];
        self.update(|current| {
            cleanup.extend(
                current["drafts"][key]["attachments"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
            let mut next = current.clone();
            next["drafts"] = super::mobile_content::clear_content(
                &current["drafts"],
                key,
                clear_model,
                clear_workspace,
            );
            Ok(next)
        })?;
        Ok(cleanup)
    }
    pub fn loaded(&self) -> bool {
        self.loaded.get()
    }
    pub fn deadline(&self) -> Option<u64> {
        self.state.borrow().deadline
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        self.changes.subscribe()
    }
    pub fn update(
        &self,
        change: impl FnOnce(&Value) -> Result<Value, ClientError>,
    ) -> Result<bool, ClientError> {
        if self.disposed.get() {
            return Err(ClientError::Cancelled);
        }
        let before = self.snapshot();
        let next = change(&before)?;
        if next == *before {
            return Ok(false);
        }
        mobile_drafts::validate_document(&next)?;
        let mut state = self.state.borrow_mut();
        state.current = Rc::new(next);
        state.deadline = Some(
            self.platform
                .monotonic_ms()
                .saturating_add(MOBILE_DEBOUNCE_MS),
        );
        drop(state);
        self.changes.notify();
        Ok(true)
    }
    pub async fn load(&self) -> Result<(), ClientError> {
        let _guard = self.loading.lock().await;
        if self.loaded.get() {
            return Ok(());
        }
        let saved = self
            .platform
            .get(StorageArea::Ordinary, storage_key(DraftFormat::Mobile))
            .await?;
        let raw = saved
            .as_deref()
            .map(SavedDraftDocument::decode_mobile)
            .transpose()?
            .map_or_else(
                || json!({"schemaVersion":1,"drafts":{}}),
                |document| document.raw().clone(),
            );
        let now = crate::time::iso_from_epoch_ms(self.platform.now_epoch_ms());
        let mut id_error = None;
        let migrated = mobile_drafts::migrate_document(&raw, &now, || {
            match crate::operations::allocate_command_id(self.platform.as_ref(), None) {
                Ok(id) => id.0,
                Err(error) => {
                    id_error = Some(error);
                    String::new()
                }
            }
        })?;
        if let Some(error) = id_error {
            return Err(error);
        }
        let mut state = self.state.borrow_mut();
        state.current = Rc::new(merge_documents(&migrated, &state.current, &state.baseline)?);
        state.baseline = Rc::new(raw);
        if *state.current != *state.baseline {
            state.deadline = Some(
                self.platform
                    .monotonic_ms()
                    .saturating_add(MOBILE_DEBOUNCE_MS),
            );
        }
        self.loaded.set(true);
        drop(state);
        self.changes.notify();
        Ok(())
    }
    pub async fn flush(&self) -> Result<bool, ClientError> {
        self.load().await?;
        let _guard = self.writing.lock().await;
        let (local, baseline) = {
            let state = self.state.borrow();
            if state.deadline.is_none() {
                return Ok(false);
            }
            (state.current.clone(), state.baseline.clone())
        };
        let key = storage_key(DraftFormat::Mobile);
        let expected = self
            .platform
            .get(StorageArea::Ordinary, key.clone())
            .await?;
        let stored = expected
            .as_deref()
            .map(SavedDraftDocument::decode_mobile)
            .transpose()?
            .map_or_else(
                || json!({"schemaVersion":1,"drafts":{}}),
                |document| document.raw().clone(),
            );
        mobile_drafts::validate_document(&stored)?;
        let merged = merge_documents(&stored, &local, &baseline)?;
        mobile_drafts::validate_document(&merged)?;
        let record = crate::persistence::Record::new(
            "composer-drafts.mobile",
            None,
            "current",
            1,
            merged.clone(),
        );
        if !self
            .platform
            .transaction(StorageTransaction {
                conditions: vec![StorageCondition {
                    area: StorageArea::Ordinary,
                    key: key.clone(),
                    expected,
                }],
                ordinary: vec![StorageWrite {
                    key,
                    value: Some(serde_json::to_vec(&record)?),
                }],
                secure: vec![],
            })
            .await?
        {
            return Err(ClientError::InvalidPayload(
                "Drafts changed during persistence; retry the pending flush.".into(),
            ));
        }
        let mut state = self.state.borrow_mut();
        let current = merge_documents(&merged, &state.current, &local)?;
        state.deadline = if current == merged {
            None
        } else {
            state.deadline
        };
        state.current = Rc::new(current);
        state.baseline = Rc::new(merged);
        drop(state);
        self.changes.notify();
        Ok(true)
    }
    pub async fn flush_all(&self) -> Result<(), ClientError> {
        while self.flush().await? {}
        Ok(())
    }
    pub fn dispose(&self) {
        self.operations.abort_where(|_| true);
        self.disposed.set(true);
        self.changes.notify();
    }
    pub fn run(self: Rc<Self>) -> LocalBoxFuture<'static, Result<(), ClientError>>
    where
        P: 'static,
    {
        async move {
            if self.running.replace(true){return Err(ClientError::InvalidPayload("The draft persistence loop is already running.".into()));}
            struct Running<P:Platform>(Rc<MobileDraftStore<P>>);
            impl<P:Platform> Drop for Running<P>{fn drop(&mut self){self.0.running.set(false);}}
            let _running=Running(self.clone());self.load().await?;
            let mut changes=self.changes();let mut events=self.platform.events().chain(futures::stream::pending());
            while !self.disposed.get() {
                if self.deadline().is_some_and(|deadline|deadline<=self.platform.monotonic_ms()){self.flush_all().await?;}
                let timer=match self.deadline(){Some(deadline)=>self.platform.sleep(deadline.saturating_sub(self.platform.monotonic_ms())),None=>futures::future::pending().boxed_local()};
                futures::select_biased! {
                    _=changes.next().fuse()=>{},
                    _=timer.fuse()=>{},
                    event=events.next().fuse()=>{if matches!(event,Some(crate::platform::PlatformEvent::Suspended|crate::platform::PlatformEvent::Foreground)){self.flush_all().await?;}},
                }
            }
            self.flush_all().await?;Ok(())
        }.boxed_local()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        platform::{Storage, StorageWrite},
        testing::FakePlatform,
    };
    use futures::executor::block_on;
    #[test]
    fn persistence_driver_debounces_on_injected_time_and_leaves_no_idle_timer() {
        block_on(async {
            let platform = Rc::new(FakePlatform::default());
            let store = Rc::new(MobileDraftStore::new(platform.clone()));
            store
                .update(|current| {
                    let mut next = current.clone();
                    next["drafts"]["key"] = json!({"text":"typed","attachments":[]});
                    Ok(next)
                })
                .expect("edit");
            let changes = store.changes();
            let scenario = async {
                platform.advance(MOBILE_DEBOUNCE_MS);
                let mut saved = changes.filter_map(|()| {
                    futures::future::ready(store.deadline().is_none().then_some(()))
                });
                saved.next().await.expect("persisted event");
                assert_eq!(platform.pending_timers(), 0);
                store.dispose();
            };
            let (result, ()) = futures::join!(store.clone().run(), scenario);
            result.expect("driver");
            assert_eq!(platform.pending_timers(), 0);
            let bytes = platform
                .get(StorageArea::Ordinary, storage_key(DraftFormat::Mobile))
                .await
                .expect("read")
                .expect("saved");
            assert_eq!(
                SavedDraftDocument::decode_mobile(&bytes)
                    .expect("decode")
                    .raw()["drafts"]["key"]["text"],
                "typed"
            );
        });
    }
    #[test]
    fn hydration_keeps_live_edits_and_flush_merges_other_writers_by_draft_key() {
        block_on(async {
            let platform = Rc::new(FakePlatform::default());
            let key = storage_key(DraftFormat::Mobile);
            platform.write(StorageArea::Ordinary,vec![StorageWrite {key:key.clone(),value:Some(serde_json::to_vec(&json!({"schemaVersion":1,"drafts":{"saved":{"text":"saved","attachments":[]}},"future":true})).expect("json"))}]).await.expect("seed");
            let first = MobileDraftStore::new(platform.clone());
            first
                .update(|current| {
                    let mut next = current.clone();
                    next["drafts"]["live"] = json!({"text":"live","attachments":[]});
                    Ok(next)
                })
                .expect("type before hydration");
            first.load().await.expect("hydrate");
            assert_eq!(first.snapshot()["drafts"]["saved"]["text"], "saved");
            assert_eq!(first.snapshot()["drafts"]["live"]["text"], "live");
            let second = MobileDraftStore::new(platform.clone());
            second.load().await.expect("second hydrate");
            second
                .update(|current| {
                    let mut next = current.clone();
                    next["drafts"]["other"] = json!({"text":"other writer","attachments":[]});
                    Ok(next)
                })
                .expect("other edit");
            second.flush_all().await.expect("second flush");
            first.flush_all().await.expect("first flush");
            let saved = SavedDraftDocument::decode_mobile(
                &platform
                    .get(StorageArea::Ordinary, key)
                    .await
                    .expect("read")
                    .expect("bytes"),
            )
            .expect("decode");
            assert_eq!(saved.raw()["drafts"].as_object().expect("drafts").len(), 3);
            assert_eq!(saved.raw()["future"], true);
            assert!(first.deadline().is_none());
            assert_eq!(first.snapshot()["drafts"]["other"]["text"], "other writer");
        });
    }
    #[test]
    fn unreadable_storage_is_preserved_and_does_not_publish_a_loaded_empty_owner() {
        block_on(async {
            let platform = Rc::new(FakePlatform::default());
            let key = storage_key(DraftFormat::Mobile);
            let bytes = b"unreadable user data".to_vec();
            platform
                .write(
                    StorageArea::Ordinary,
                    vec![StorageWrite {
                        key: key.clone(),
                        value: Some(bytes.clone()),
                    }],
                )
                .await
                .expect("seed");
            let store = MobileDraftStore::new(platform.clone());
            assert!(store.load().await.is_err());
            assert!(!store.loaded());
            assert!(store.flush().await.is_err());
            assert_eq!(
                platform
                    .get(StorageArea::Ordinary, key)
                    .await
                    .expect("read"),
                Some(bytes)
            );
        });
    }
}
