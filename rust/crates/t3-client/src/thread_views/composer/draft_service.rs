//! The web document shared by the composer, attachments and delivery recovery.
use super::{send::Changes, web_drafts, web_models};
use crate::{
    ClientError,
    platform::{Platform, PlatformEvent, StorageArea},
    thread_views::drafts::{DraftFormat, SavedDraftDocument, WebDraftPersistence, merge_web_states, storage_key},
    types::ThreadKey,
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, lock::Mutex, stream::LocalBoxStream};
use serde_json::{Value, json};
use std::{cell::{Cell, RefCell}, collections::BTreeMap, rc::Rc};

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
        self.draft_key == other.draft_key && self.revision == other.revision
            && self.prompt == other.prompt && self.cursor == other.cursor
            && self.loaded == other.loaded && self.writable == other.writable
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
    views: RefCell<BTreeMap<ThreadKey, Rc<ComposerDraftView>>>,
    revisions: RefCell<BTreeMap<String, u64>>,
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

impl<P: Platform + 'static> ComposerDraftService<P> {
    pub fn new(platform: Rc<P>) -> Self {
        let baseline = empty();
        Self {
            platform, document: Rc::new(RefCell::new(baseline.state().clone())),
            baseline_live: RefCell::new(baseline.state().clone()),
            baseline: RefCell::new(baseline), views: RefCell::default(),
            revisions: RefCell::default(), loaded: Cell::new(false), dirty: Cell::new(false),
            deadline: Cell::new(None), error: RefCell::new(None), writing: Mutex::new(()),
            changes: Changes::default(),
        }
    }

    pub fn document(&self) -> Rc<RefCell<Value>> { self.document.clone() }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> { self.changes.subscribe() }
    pub fn loaded(&self) -> bool { self.loaded.get() }

    pub fn view(&self, key: &ThreadKey) -> Rc<ComposerDraftView> {
        let id = draft_key(key);
        let document = self.document.borrow();
        let draft = &document["draftsByThreadKey"][&id];
        let next = ComposerDraftView {
            draft_key: id.clone(), revision: *self.revisions.borrow().get(&id).unwrap_or(&0),
            prompt: draft["prompt"].as_str().unwrap_or_default().into(),
            cursor: draft["cursor"].as_u64().unwrap_or(0) as usize,
            loaded: self.loaded.get(), writable: self.loaded.get() && self.error.borrow().is_none(),
            error: self.error.borrow().clone(),
        };
        let mut views = self.views.borrow_mut();
        if let Some(prior) = views.get(key).filter(|prior| ***prior == next) { return prior.clone(); }
        let next = Rc::new(next);
        views.insert(key.clone(), next.clone());
        next
    }

    pub fn update(&self, change: impl FnOnce(&mut Value) -> Result<(), ClientError>) -> Result<bool, ClientError> {
        let before = self.document.borrow().clone();
        let mut next = before.clone();
        change(&mut next)?;
        if next == before { return Ok(false); }
        self.baseline.borrow().replace_current_web_state(next.clone())?;
        let keys = before["draftsByThreadKey"].as_object().into_iter().flat_map(|v| v.keys())
            .chain(next["draftsByThreadKey"].as_object().into_iter().flat_map(|v| v.keys()))
            .collect::<std::collections::BTreeSet<_>>();
        for key in keys {
            if before["draftsByThreadKey"].get(key) != next["draftsByThreadKey"].get(key) {
                *self.revisions.borrow_mut().entry(key.clone()).or_default() += 1;
            }
        }
        *self.document.borrow_mut() = next;
        self.dirty.set(true);
        self.deadline.set(Some(self.platform.monotonic_ms().saturating_add(300)));
        self.changes.notify();
        Ok(true)
    }

    pub fn set_prompt(&self, key: &ThreadKey, prompt: String, cursor: usize) -> Result<(), ClientError> {
        let id = draft_key(key);
        let cursor = cursor.min(prompt.encode_utf16().count());
        self.update(|document| {
            web_models::set_prompt(document, &id, &prompt);
            if document["draftsByThreadKey"].get(&id).is_some() {
                document["draftsByThreadKey"][&id]["cursor"] = json!(cursor);
            }
            Ok(())
        })?;
        Ok(())
    }

    pub fn clear_revision(&self, key: &ThreadKey, revision: u64) -> Result<bool, ClientError> {
        if self.view(key).revision != revision { return Ok(false); }
        self.update(|document| { web_models::clear_content(document, &draft_key(key)); Ok(()) })
    }

    pub async fn load(&self) -> Result<(), ClientError> {
        let _guard = self.writing.lock().await;
        self.reload().await
    }

    async fn reload(&self) -> Result<(), ClientError> {
        let result = async {
            let bytes = self.platform.get(StorageArea::Ordinary, storage_key(DraftFormat::Web)).await?;
            let saved = bytes.as_deref().map(SavedDraftDocument::decode_web).transpose()?.unwrap_or_else(empty)
                .migrate_web(&crate::time::iso_from_epoch_ms(self.platform.now_epoch_ms()))?;
            let mut hydrated = saved.state().clone();
            if let Some(drafts) = hydrated["draftsByThreadKey"].as_object_mut() {
                for draft in drafts.values_mut() { *draft = web_drafts::hydrate_draft(draft); }
            }
            let merged = merge_web_states(&hydrated, &self.document.borrow(), &self.baseline_live.borrow())?;
            *self.baseline_live.borrow_mut() = hydrated;
            *self.baseline.borrow_mut() = saved;
            *self.document.borrow_mut() = merged;
            self.loaded.set(true);
            *self.error.borrow_mut() = None;
            Ok::<_, ClientError>(())
        }.await;
        if let Err(error) = result {
            *self.error.borrow_mut() = Some(Rc::new(error));
            self.changes.notify();
            return Err(ClientError::InvalidPayload("Unreadable draft document retained for recovery.".into()));
        }
        self.changes.notify();
        Ok(())
    }

    pub async fn flush(&self) -> Result<bool, ClientError> {
        let _guard = self.writing.lock().await;
        if !self.loaded.get() { self.reload().await?; }
        if !self.dirty.get() { return Ok(false); }
        let captured = self.document.borrow().clone();
        let baseline = self.baseline.borrow().clone();
        let local = baseline.replace_current_web_state(web_drafts::partialize_state(&captured, &Default::default()))?;
        let mut persistence = WebDraftPersistence::new(baseline)?;
        persistence.schedule(local.clone(), self.platform.monotonic_ms())?;
        let written = persistence.flush(self.platform.as_ref(), storage_key(DraftFormat::Web)).await?;
        let saved = self.platform.get(StorageArea::Ordinary, storage_key(DraftFormat::Web)).await?
            .ok_or_else(|| ClientError::InvalidPayload("The saved composer draft is missing.".into()))?;
        let saved = SavedDraftDocument::decode_web(&saved)?;
        let current = self.document.borrow().clone();
        let mut hydrated = saved.state().clone();
        if let Some(drafts) = hydrated["draftsByThreadKey"].as_object_mut() {
            for (key, draft) in drafts {
                *draft = if local.state()["draftsByThreadKey"].get(key) == Some(draft) {
                    captured["draftsByThreadKey"][key.as_str()].clone()
                } else { web_drafts::hydrate_draft(draft) };
            }
        }
        let merged = merge_web_states(&hydrated, &current, &captured)?;
        self.dirty.set(current != captured);
        self.deadline.set(self.dirty.get().then(|| self.platform.monotonic_ms().saturating_add(300)));
        *self.document.borrow_mut() = merged;
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
