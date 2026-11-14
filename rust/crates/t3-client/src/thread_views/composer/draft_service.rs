//! The web document shared by the composer, attachments and delivery recovery.
use super::{send::Changes, web_drafts, web_models};
use crate::{
    ClientError,
    platform::{Platform, PlatformEvent, StorageArea},
    thread_views::drafts::{
        DraftFormat, SavedDraftDocument, WebDraftPersistence, merge_web_states, storage_key,
    },
    types::ThreadKey,
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, lock::Mutex, stream::LocalBoxStream};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};

#[derive(Clone, Debug)]
pub struct ComposerDraftView {
    pub draft_key: String,
    pub revision: u64,
    pub prompt: Rc<str>,
    pub cursor: usize,
    pub loaded: bool,
    pub writable: bool,
    pub error: Option<Rc<ClientError>>,
}

impl PartialEq for ComposerDraftView {
    fn eq(&self, other: &Self) -> bool {
        self.draft_key == other.draft_key
            && self.revision == other.revision
            && self.prompt == other.prompt
            && self.cursor == other.cursor
            && self.loaded == other.loaded
            && self.writable == other.writable
            && match (&self.error, &other.error) {
                (None, None) => true,
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                _ => false,
            }
    }
}

pub fn draft_key(key: &ThreadKey) -> String {
    format!("{}:{}", key.environment.0, key.thread.0)
}

pub struct ComposerDraftService<P: Platform> {
    platform: Rc<P>,
    document: Rc<RefCell<Value>>,
    baseline: RefCell<SavedDraftDocument>,
    baseline_live: RefCell<Value>,
    views: RefCell<BTreeMap<String, Rc<ComposerDraftView>>>,
    revisions: RefCell<BTreeMap<String, u64>>,
    editor_targets: RefCell<BTreeMap<ThreadKey, String>>,
    loaded: Cell<bool>,
    dirty: Cell<bool>,
    deadline: Cell<Option<u64>>,
    error: RefCell<Option<Rc<ClientError>>>,
    writing: Mutex<()>,
    changes: Changes,
}

fn empty() -> SavedDraftDocument {
    SavedDraftDocument::decode_web(br#"{"version":9,"state":{"draftsByThreadKey":{},"draftThreadsByThreadKey":{},"logicalProjectDraftThreadKeyByLogicalProjectKey":{}}}"#)
        .expect("canonical empty web draft")
}
struct DraftAttachmentOwner<P: Platform> {
    drafts: Rc<ComposerDraftService<P>>,
    key: String,
    active: Rc<Cell<bool>>,
}
impl<P: Platform + 'static> super::attachment_intake::AttachmentDraftOwner
    for DraftAttachmentOwner<P>
{
    fn writable(&self) -> bool {
        self.active.get() && self.drafts.writable()
    }
    fn changed(&self) {
        if self.active.get() && self.drafts.writable() {
            self.drafts.attachment_changed(&self.key);
        }
    }
}

impl<P: Platform + 'static> ComposerDraftService<P> {
    pub fn new(platform: Rc<P>) -> Self {
        let baseline = empty();
        Self {
            platform,
            document: Rc::new(RefCell::new(baseline.state().clone())),
            baseline_live: RefCell::new(baseline.state().clone()),
            baseline: RefCell::new(baseline),
            views: RefCell::default(),
            revisions: RefCell::default(),
            editor_targets: RefCell::default(),
            loaded: Cell::new(false),
            dirty: Cell::new(false),
            deadline: Cell::new(None),
            error: RefCell::new(None),
            writing: Mutex::new(()),
            changes: Changes::default(),
        }
    }

    pub fn document(&self) -> Rc<RefCell<Value>> {
        self.document.clone()
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        self.changes.subscribe()
    }
    pub fn loaded(&self) -> bool {
        self.loaded.get()
    }
    pub fn writable(&self) -> bool {
        self.loaded.get() && self.error.borrow().is_none()
    }
    pub(crate) fn attachment_owner(
        self: &Rc<Self>,
        key: String,
        active: Rc<Cell<bool>>,
    ) -> Rc<dyn super::attachment_intake::AttachmentDraftOwner> {
        Rc::new(DraftAttachmentOwner {
            drafts: self.clone(),
            key,
            active,
        })
    }
    pub(crate) fn attachment_changed(&self, key: &str) {
        *self.revisions.borrow_mut().entry(key.into()).or_default() += 1;
        self.dirty.set(true);
        self.deadline
            .set(Some(self.platform.monotonic_ms().saturating_add(300)));
        self.changes.notify();
    }
    pub(super) fn record_error(&self, error: ClientError) {
        *self.error.borrow_mut() = Some(Rc::new(error));
        self.changes.notify();
    }

    pub fn view(&self, key: &ThreadKey) -> Rc<ComposerDraftView> {
        self.view_key(&self.editor_draft_key(key))
    }
    pub fn editor_draft_key(&self, key: &ThreadKey) -> String {
        self.editor_targets
            .borrow()
            .get(key)
            .cloned()
            .unwrap_or_else(|| self.thread_draft_key(key))
    }
    pub(crate) fn set_editor_target(&self, key: &ThreadKey, target: Option<String>) {
        let mut targets = self.editor_targets.borrow_mut();
        if targets.get(key) == target.as_ref() {
            return;
        }
        match target {
            Some(target) => {
                targets.insert(key.clone(), target);
            }
            None => {
                targets.remove(key);
            }
        }
        drop(targets);
        self.changes.notify();
    }
    pub fn thread_draft_key(&self, key: &ThreadKey) -> String {
        super::web_sessions::resolve(
            &self.document.borrow(),
            super::web_sessions::Target::Thread(&t3_contracts::environment::ScopedThreadRef {
                environment_id: key.environment.clone(),
                thread_id: key.thread.clone(),
            }),
        )
        .unwrap_or_else(|| draft_key(key))
    }
    pub fn view_key(&self, id: &str) -> Rc<ComposerDraftView> {
        let document = self.document.borrow();
        let draft = &document["draftsByThreadKey"][id];
        let next = ComposerDraftView {
            draft_key: id.into(),
            revision: *self.revisions.borrow().get(id).unwrap_or(&0),
            prompt: draft["prompt"].as_str().unwrap_or_default().into(),
            cursor: draft["cursor"].as_u64().unwrap_or(0) as usize,
            loaded: self.loaded.get(),
            writable: self.loaded.get() && self.error.borrow().is_none(),
            error: self.error.borrow().clone(),
        };
        let mut views = self.views.borrow_mut();
        if let Some(prior) = views.get(id).filter(|prior| ***prior == next) {
            return prior.clone();
        }
        let next = Rc::new(next);
        views.insert(id.into(), next.clone());
        next
    }

    pub fn update(
        &self,
        change: impl FnOnce(&mut Value) -> Result<(), ClientError>,
    ) -> Result<bool, ClientError> {
        if !self.writable() {
            return Err(ClientError::Cancelled);
        }
        let before = self.document.borrow().clone();
        let mut next = before.clone();
        change(&mut next)?;
        if next == before {
            return Ok(false);
        }
        self.baseline
            .borrow()
            .replace_current_web_state(next.clone())?;
        self.replace_document(next);
        self.dirty.set(true);
        self.deadline
            .set(Some(self.platform.monotonic_ms().saturating_add(300)));
        self.changes.notify();
        Ok(true)
    }

    fn replace_document(&self, next: Value) {
        let before = self.document.borrow();
        let mut keys = std::collections::BTreeSet::new();
        for field in ["draftsByThreadKey", "draftThreadsByThreadKey"] {
            for document in [&*before, &next] {
                keys.extend(
                    document[field]
                        .as_object()
                        .into_iter()
                        .flat_map(|drafts| drafts.keys())
                        .cloned(),
                );
            }
        }
        for key in keys {
            if ["draftsByThreadKey", "draftThreadsByThreadKey"]
                .iter()
                .any(|field| before[*field].get(&key) != next[*field].get(&key))
            {
                *self.revisions.borrow_mut().entry(key).or_default() += 1;
            }
        }
        drop(before);
        *self.document.borrow_mut() = next;
    }

    pub fn set_prompt(
        &self,
        key: &ThreadKey,
        prompt: String,
        cursor: usize,
    ) -> Result<(), ClientError> {
        self.set_prompt_key(&self.editor_draft_key(key), prompt, cursor)
    }
    pub fn set_prompt_key(
        &self,
        id: &str,
        prompt: String,
        cursor: usize,
    ) -> Result<(), ClientError> {
        if !self.writable() {
            return Err(ClientError::Cancelled);
        }
        let id = super::web_sessions::resolve(
            &self.document.borrow(),
            super::web_sessions::Target::Draft(id),
        )
        .ok_or_else(|| ClientError::InvalidPayload("A composer draft is required.".into()))?;
        let cursor = cursor.min(prompt.encode_utf16().count());
        {
            let mut document = self.document.borrow_mut();
            let prior = &document["draftsByThreadKey"][&id];
            if prior["prompt"].as_str().unwrap_or_default() == prompt
                && prior["cursor"].as_u64().unwrap_or(0) == cursor as u64
            {
                return Ok(());
            }
            web_models::set_prompt(&mut document, &id, &prompt);
            if document["draftsByThreadKey"].get(&id).is_some() {
                document["draftsByThreadKey"][&id]["cursor"] = json!(cursor);
            }
        }
        self.attachment_changed(&id);
        Ok(())
    }

    pub fn clear_revision(&self, key: &ThreadKey, revision: u64) -> Result<bool, ClientError> {
        self.clear_key_revision(&self.thread_draft_key(key), revision)
    }
    pub fn clear_key_revision(&self, id: &str, revision: u64) -> Result<bool, ClientError> {
        if self.view_key(id).revision != revision {
            return Ok(false);
        }
        self.update(|document| {
            web_models::clear_content(document, id);
            Ok(())
        })
    }

    pub async fn load(&self) -> Result<(), ClientError> {
        let _guard = self.writing.lock().await;
        self.reload().await
    }

    async fn reload(&self) -> Result<(), ClientError> {
        let result = async {
            let bytes = self
                .platform
                .get(StorageArea::Ordinary, storage_key(DraftFormat::Web))
                .await?;
            let saved = bytes
                .as_deref()
                .map(SavedDraftDocument::decode_web)
                .transpose()?
                .unwrap_or_else(empty)
                .migrate_web(&crate::time::iso_from_epoch_ms(
                    self.platform.now_epoch_ms(),
                ))?;
            let mut hydrated = saved.state().clone();
            if let Some(drafts) = hydrated["draftsByThreadKey"].as_object_mut() {
                for draft in drafts.values_mut() {
                    *draft = web_drafts::hydrate_draft(draft);
                }
            }
            let merged = merge_web_states(
                &hydrated,
                &self.document.borrow(),
                &self.baseline_live.borrow(),
            )?;
            *self.baseline_live.borrow_mut() = hydrated;
            *self.baseline.borrow_mut() = saved;
            self.replace_document(merged);
            self.loaded.set(true);
            *self.error.borrow_mut() = None;
            Ok::<_, ClientError>(())
        }
        .await;
        if let Err(error) = result {
            *self.error.borrow_mut() = Some(Rc::new(error));
            self.changes.notify();
            return Err(ClientError::InvalidPayload(
                "Unreadable draft document retained for recovery.".into(),
            ));
        }
        self.changes.notify();
        Ok(())
    }

    pub async fn flush(&self) -> Result<bool, ClientError> {
        let _guard = self.writing.lock().await;
        if !self.loaded.get() {
            self.reload().await?;
        }
        if !self.dirty.get() {
            return Ok(false);
        }
        let image_source = self.document.borrow().clone();
        for (key, draft) in image_source["draftsByThreadKey"]
            .as_object()
            .into_iter()
            .flatten()
        {
            let images = draft["images"].as_array().map_or(&[][..], Vec::as_slice);
            if images.is_empty() {
                continue;
            }
            let saved = super::attachment_intake::serialize_composer_images(
                self.platform.as_ref(),
                images,
                draft["persistedAttachments"]
                    .as_array()
                    .map_or(&[][..], Vec::as_slice),
            )
            .await;
            let live = self.document.borrow()["draftsByThreadKey"][key].clone();
            let saved = saved
                .into_iter()
                .filter(|image| {
                    live["images"].as_array().is_some_and(|images| {
                        images.iter().any(|current| current["id"] == image["id"])
                    })
                })
                .collect::<Vec<_>>();
            self.update(|document| {
                super::web_content::sync_persisted_attachments(document, key, &saved);
                Ok(())
            })?;
        }
        let captured = self.document.borrow().clone();
        let baseline = self.baseline.borrow().clone();
        let local = baseline.replace_current_web_state(web_drafts::partialize_state(
            &captured,
            &Default::default(),
        ))?;
        let mut persistence = WebDraftPersistence::new(baseline)?;
        persistence.schedule(local.clone(), self.platform.monotonic_ms())?;
        let written = persistence
            .flush(self.platform.as_ref(), storage_key(DraftFormat::Web))
            .await?;
        let saved = self
            .platform
            .get(StorageArea::Ordinary, storage_key(DraftFormat::Web))
            .await?
            .ok_or_else(|| {
                ClientError::InvalidPayload("The saved composer draft is missing.".into())
            })?;
        let saved = SavedDraftDocument::decode_web(&saved)?;
        let current = self.document.borrow().clone();
        let mut hydrated = saved.state().clone();
        if let Some(drafts) = hydrated["draftsByThreadKey"].as_object_mut() {
            for (key, draft) in drafts {
                *draft = if local.state()["draftsByThreadKey"].get(key) == Some(draft) {
                    captured["draftsByThreadKey"][key.as_str()].clone()
                } else {
                    web_drafts::hydrate_draft(draft)
                };
            }
        }
        let merged = merge_web_states(&hydrated, &current, &captured)?;
        self.dirty.set(current != captured);
        self.deadline.set(
            self.dirty
                .get()
                .then(|| self.platform.monotonic_ms().saturating_add(300)),
        );
        self.replace_document(merged);
        *self.baseline_live.borrow_mut() = hydrated;
        *self.baseline.borrow_mut() = saved;
        *self.error.borrow_mut() = None;
        self.changes.notify();
        Ok(written)
    }

    pub fn run(self: Rc<Self>) -> LocalBoxFuture<'static, ()> {
        async move {
            let _ = self.load().await;
            let mut changes = self.changes();
            let mut events = self.platform.events().chain(futures::stream::pending());
            loop {
                let timer = match self.deadline.get() {
                    Some(deadline) => self.platform.sleep(deadline.saturating_sub(self.platform.monotonic_ms())),
                    None => futures::future::pending().boxed_local(),
                };
                let flush = futures::select_biased! {
                    _ = changes.next().fuse() => false,
                    _ = timer.fuse() => true,
                    event = events.next().fuse() => match event {
                        Some(PlatformEvent::StorageChanged(key)) if key == storage_key(DraftFormat::Web) => {
                            let _ = self.load().await; false
                        },
                        Some(PlatformEvent::Suspended | PlatformEvent::Foreground) => true,
                        _ => false,
                    },
                };
                if flush && let Err(error) = self.flush().await {
                    *self.error.borrow_mut() = Some(Rc::new(error));
                    self.deadline.set(None);
                    self.changes.notify();
                }
            }
        }.boxed_local()
    }
}
