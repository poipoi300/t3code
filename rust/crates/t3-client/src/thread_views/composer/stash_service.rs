//! The prompt stash over the authoritative draft and transactional host storage.
use super::{context_records, send::Changes, target::ComposerTarget, web_content, web_models};
use crate::{
    ClientError,
    composition::ClientRuntime,
    platform::{
        Platform, PlatformEvent, StorageArea, StorageCondition, StorageKey, StorageTransaction,
        StorageWrite,
    },
    view_logic::{
        JsTrim,
        shell::stash::{self, Entry, Stash, WriteOutcome},
    },
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, lock::Mutex, stream::LocalBoxStream};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[derive(Default)]
pub struct PromptStashState {
    entries: RefCell<Vec<Rc<Entry>>>,
    revision: Cell<u64>,
    loaded: Cell<bool>,
    changes: Changes,
    gate: Mutex<()>,
}
#[derive(Clone, Default)]
pub struct PromptStashSnapshot {
    pub entries: Vec<Rc<Entry>>,
    pub can_stash: bool,
    pub stash_revision: u64,
    pub can_restore: bool,
    pub can_delete: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StashMenu {
    Keep,
    Open,
    Close,
    Toggle,
}
#[derive(Clone, Debug)]
pub struct StashNotice {
    pub warning: bool,
    pub title: String,
    pub description: Option<String>,
}
#[derive(Clone, Debug)]
pub struct StashCompletion {
    pub notices: Vec<StashNotice>,
    pub menu: StashMenu,
}
#[derive(Clone)]
pub struct PromptStashSession<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
    platform: P,
    target: ComposerTarget,
    state: Rc<PromptStashState>,
}
struct Stored {
    entries: Vec<Entry>,
    expected: Option<Vec<u8>>,
    raw: Value,
}
pub fn storage_key() -> StorageKey {
    StorageKey::new("prompt-stash", None, stash::STORAGE_KEY)
}
async fn read(platform: &dyn Platform) -> Result<Stored, ClientError> {
    let key = storage_key();
    let expected = platform.get(StorageArea::Ordinary, key.clone()).await?;
    let (bytes, version, record) = if let Some(bytes) = expected.clone() {
        (Some(bytes), 2, key)
    } else {
        let key = StorageKey::new("prompt-stash", None, stash::LEGACY_STORAGE_KEY);
        (
            platform.get(StorageArea::Ordinary, key.clone()).await?,
            1,
            key,
        )
    };
    let raw = if let Some(bytes) = bytes {
        let raw: Value = serde_json::from_slice(&bytes)?;
        if raw.get("formatVersion").is_some() {
            crate::persistence::decode_record::<Value>(&bytes, &record, version)
                .map_err(|error| ClientError::InvalidPayload(error.to_string()))?
                .payload
        } else {
            raw
        }
    } else {
        json!({"version":2,"state":{"entries":[]}})
    };
    let mut raw = raw;
    if raw["state"]["entries"].is_null() && raw["entries"].is_array() {
        raw["state"] = json!({"entries":raw["entries"]});
    }
    let entries = stash::decode(&serde_json::to_string(&raw)?)?;
    Ok(Stored {
        entries,
        expected,
        raw,
    })
}
async fn persist(
    platform: &dyn Platform,
    stored: &Stored,
    entries: &[Entry],
) -> Result<(), ClientError> {
    let mut raw = stored.raw.clone();
    let encoded: Value = serde_json::from_str(&stash::encode(entries)?)?;
    raw["version"] = json!(2);
    raw["state"]["entries"] = encoded["state"]["entries"].clone();
    let bytes = serde_json::to_vec(&crate::persistence::Record::new(
        "prompt-stash",
        None,
        stash::STORAGE_KEY,
        2,
        raw,
    ))?;
    if !platform
        .transaction(StorageTransaction {
            conditions: vec![StorageCondition {
                area: StorageArea::Ordinary,
                key: storage_key(),
                expected: stored.expected.clone(),
            }],
            ordinary: vec![StorageWrite {
                key: storage_key(),
                value: Some(bytes),
            }],
            secure: vec![],
        })
        .await?
    {
        return Err(ClientError::Cancelled);
    }
    Ok(())
}
impl PromptStashState {
    fn publish(&self, entries: Vec<Entry>) {
        let prior = self.entries.borrow().clone();
        *self.entries.borrow_mut() = entries
            .into_iter()
            .map(|entry| {
                prior
                    .iter()
                    .find(|prior| ***prior == entry)
                    .cloned()
                    .unwrap_or_else(|| Rc::new(entry))
            })
            .collect();
        self.loaded.set(true);
        self.changes.notify();
    }
    pub async fn load(&self, platform: &dyn Platform) -> Result<(), ClientError> {
        let _guard = self.gate.lock().await;
        self.publish(read(platform).await?.entries);
        Ok(())
    }
}
impl<P: Platform + Clone + 'static> PromptStashSession<P> {
    pub fn new(
        runtime: ClientRuntime<P>,
        platform: P,
        target: ComposerTarget,
        state: Rc<PromptStashState>,
    ) -> Self {
        Self {
            runtime,
            platform,
            target,
            state,
        }
    }
    pub fn snapshot(&self) -> PromptStashSnapshot {
        let ready = self.runtime.composer_drafts().writable() && self.state.loaded.get();
        let preparing = self
            .runtime
            .composer_attachments(&self.target)
            .ok()
            .is_some_and(|attachments| {
                attachments
                    .items()
                    .iter()
                    .any(|item| item.upload == super::attachment_intake::UploadState::Preparing)
            });
        let pending = match &self.target {
            ComposerTarget::Thread(key) => self
                .runtime
                .store()
                .snapshot()
                .environments
                .get(&key.environment)
                .and_then(|environment| environment.threads.get(&key.thread))
                .and_then(|thread| thread.projection.as_deref())
                .is_some_and(|projection| {
                    !crate::thread_views::requests::derive_pending(
                        &projection.runtime_requests,
                        &projection.turn_items,
                    )
                    .user_inputs
                    .is_empty()
                }),
            _ => false,
        };
        PromptStashSnapshot {
            entries: self.state.entries.borrow().clone(),
            can_stash: ready && !pending && !preparing,
            stash_revision: self.state.revision.get(),
            can_restore: ready && !pending,
            can_delete: self.state.loaded.get(),
        }
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        let this = self.clone();
        let events=self.platform.events().filter(|event|futures::future::ready(matches!(event,PlatformEvent::StorageChanged(key) if key.namespace=="prompt-stash"))).then(move |_|{let this=this.clone();async move{let _=this.state.load(&this.platform).await;}}).boxed_local();
        futures::stream::select_all(vec![
            self.state.changes.subscribe(),
            self.runtime.composer_drafts().changes(),
            self.runtime.store().subscribe().map(|_| ()).boxed_local(),
            events,
        ])
        .boxed_local()
    }
    fn key(&self) -> Result<String, ClientError> {
        self.target
            .draft_key(&self.runtime.composer_drafts().document().borrow())
            .ok_or_else(|| ClientError::InvalidPayload("A composer draft is required.".into()))
    }
    pub fn stash_current(&self) -> LocalBoxFuture<'static, Result<StashCompletion, ClientError>> {
        let this = self.clone();
        async move {
            if !this.snapshot().can_stash {
                return Err(ClientError::Cancelled);
            }
            let key = this.key()?;
            let drafts = this.runtime.composer_drafts();
            let captured = drafts.view_key(&key);
            let raw = drafts.document().borrow()["draftsByThreadKey"][&key].clone();
            let images = raw["images"].as_array().cloned().unwrap_or_default();
            let files = raw["files"].as_array().cloned().unwrap_or_default();
            if !images.is_empty() {
                return Err(ClientError::Unsupported(
                    "stash image compression (Platform U9)",
                ));
            }
            let mut prompt = captured.prompt.js_trim().to_owned();
            if crate::view_logic::context_references::collect_references(&captured.prompt)
                .last()
                .is_some_and(|reference| reference.end == captured.prompt.js_trim_end().len())
            {
                prompt.push(' ');
            }
            if prompt.is_empty() && files.is_empty() {
                let entries = this.state.entries.borrow().clone();
                if entries.len() == 1
                    && entries[0]
                        .pending_image_count
                        .is_none_or(|count| count == 0.0)
                {
                    return this.restore(entries[0].id.clone())?.await;
                }
                return Ok(StashCompletion {
                    notices: vec![],
                    menu: StashMenu::Toggle,
                });
            }
            let environment = this
                .target
                .environment()
                .ok_or(ClientError::EnvironmentMismatch)?;
            let mut saved_files = vec![];
            for file in &files {
                if web_content::needs_reattach(file) {
                    return Err(ClientError::Rejected {
                        message: "Attach dropped files again or remove them before stashing".into(),
                        detail: None,
                    });
                }
                if file["uploadEnvironmentId"] != environment.0
                    || !file["uploadedAttachmentId"].is_string()
                {
                    return Err(ClientError::Rejected {
                        message: "Wait for file uploads before stashing this prompt".into(),
                        detail: None,
                    });
                }
                let mut saved = file.clone();
                saved["environmentId"] = json!(environment);
                saved["attachmentId"] = file["uploadedAttachmentId"].clone();
                saved_files.push(serde_json::from_value(saved)?);
            }
            let records = context_records::draft_chip_records(&raw)?
                .into_values()
                .collect::<Vec<_>>();
            let mut metadata = std::collections::BTreeMap::new();
            if !records.is_empty() {
                metadata.insert("records".into(), json!(records));
            }
            let entry = Entry {
                id: crate::operations::allocate_command_id(&this.platform, None)?.0,
                created_at: crate::time::iso_from_epoch_ms(this.platform.now_epoch_ms()),
                prompt,
                attachments: vec![],
                dropped_image_names: vec![],
                files: (!saved_files.is_empty()).then_some(saved_files),
                unreadable_image_names: Some(vec![]),
                pending_image_count: None,
                metadata,
            };
            let _guard = this.state.gate.lock().await;
            let stored = read(&this.platform).await?;
            let mut stash = Stash {
                entries: stored.entries.clone(),
            };
            let mut failure = None;
            let outcome = stash
                .stash(entry, Some(&stored.entries), async |entries| match persist(
                    &this.platform,
                    &stored,
                    entries,
                )
                .await
                {
                    Ok(()) => WriteOutcome {
                        written: true,
                        durable: true,
                    },
                    Err(error) => {
                        failure = Some(error);
                        WriteOutcome::default()
                    }
                })
                .await;
            if let Some(error) = failure {
                return Err(error);
            }
            this.state.publish(stash.entries);
            this.state.revision.set(this.state.revision.get() + 1);
            drafts.clear_key_revision(&key, captured.revision)?;
            drafts.flush().await?;
            let notices = outcome
                .evicted
                .map(|_| StashNotice {
                    warning: true,
                    title: "Oldest stashed prompt discarded".into(),
                    description: Some(format!(
                        "The stash holds {} prompts; the oldest was removed to make room.",
                        stash::MAX_ENTRIES
                    )),
                })
                .into_iter()
                .collect();
            Ok(StashCompletion {
                notices,
                menu: StashMenu::Keep,
            })
        }
        .boxed_local()
    }
    pub fn restore(
        &self,
        id: String,
    ) -> Result<LocalBoxFuture<'static, Result<StashCompletion, ClientError>>, ClientError> {
        if !self.snapshot().can_restore {
            return Err(ClientError::Cancelled);
        }
        let entry = self
            .state
            .entries
            .borrow()
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
            .ok_or(ClientError::Cancelled)?;
        if entry.files.as_ref().is_some_and(|files| {
            files
                .iter()
                .any(|file| Some(&file.environment_id) != self.target.environment())
        }) {
            return Err(ClientError::Rejected {
                message: "Stashed files belong to another environment".into(),
                detail: Some(
                    "Restore this prompt in the environment that received its files.".into(),
                ),
            });
        }
        let this = self.clone();
        Ok(async move {
            let key = this.key()?;
            let mut expired = std::collections::BTreeSet::new();
            for file in entry.files.as_deref().unwrap_or_default() {
                let endpoint = this
                    .runtime
                    .client(&file.environment_id)
                    .and_then(|client| client.session())
                    .ok_or(ClientError::Disconnected)?;
                match crate::services::assets::verify_persisted_attachment(
                    &endpoint,
                    file.attachment_id.clone(),
                )
                .await
                {
                    crate::services::assets::AttachmentVerification::Verified => {}
                    crate::services::assets::AttachmentVerification::Missing => {
                        expired.insert(file.attachment_id.clone());
                    }
                    crate::services::assets::AttachmentVerification::Failed(error) => {
                        return Err(error);
                    }
                }
            }
            if !this.snapshot().can_restore || this.key()? != key {
                return Err(ClientError::Cancelled);
            }
            let _guard = this.state.gate.lock().await;
            let stored = read(&this.platform).await?;
            let current = stored
                .entries
                .iter()
                .find(|entry| entry.id == id)
                .ok_or(ClientError::Cancelled)?;
            if current != entry.as_ref() {
                return Err(ClientError::Cancelled);
            }
            let drafts = this.runtime.composer_drafts();
            let baseline = drafts.document().borrow().clone();
            let mut next = baseline.clone();
            let records: Vec<t3_contracts::composer_context::ComposerContextRecord> = entry
                .metadata
                .get("records")
                .map(|records| serde_json::from_value(records.clone()))
                .transpose()?
                .unwrap_or_default();
            let thread = match &this.target {
                ComposerTarget::Thread(key) => Some(key),
                _ => None,
            };
            let rewritten = context_records::import_records(
                &this.platform,
                &mut next,
                &key,
                thread,
                &records,
                &[],
            )?;
            let prompt = context_records::rewrite_references(&entry.prompt, &rewritten);
            let prior = next["draftsByThreadKey"][&key]["prompt"]
                .as_str()
                .unwrap_or_default();
            let prompt = if prompt.is_empty() {
                prior.into()
            } else if prior.js_trim().is_empty() {
                prompt
            } else {
                format!("{}\n\n{}", prior.js_trim_end(), prompt)
            };
            web_models::set_prompt(&mut next, &key, &prompt);
            let mut files = vec![];
            for file in entry.files.as_deref().unwrap_or_default() {
                let mut value = serde_json::to_value(file)?;
                value["type"] = json!("file");
                value["file"] = Value::Null;
                if !expired.contains(&file.attachment_id) {
                    value["uploadedAttachmentId"] = json!(file.attachment_id);
                    value["uploadEnvironmentId"] = json!(file.environment_id);
                }
                files.push(value);
            }
            web_content::add_files(&mut next, &key, &files, false, true);
            web_content::add_images(
                &mut next,
                &key,
                &entry
                    .attachments
                    .iter()
                    .map(|image| {
                        let mut image = serde_json::to_value(image).expect("stash image");
                        image["type"] = json!("image");
                        image["file"] = json!("memory");
                        image
                    })
                    .collect::<Vec<_>>(),
                false,
            );
            drafts.update(|document| {
                *document = crate::thread_views::drafts::merge_web_states(&next, document, &baseline)?;
                Ok(())
            })?;
            drafts.flush().await?;
            let mut stash = Stash {
                entries: stored.entries.clone(),
            };
            let mut failure = None;
            let (_, durable) = stash
                .take(&id, Some(&stored.entries), async |entries| {
                    match persist(&this.platform, &stored, entries).await {
                        Ok(()) => WriteOutcome {
                            written: true,
                            durable: true,
                        },
                        Err(error) => {
                            failure = Some(error);
                            WriteOutcome::default()
                        }
                    }
                })
                .await;
            let mut notices=vec![];
            if failure.is_some()||!durable {
                notices.push(StashNotice{warning:true,title:"Restored prompt may reappear in the stash".into(),description:Some("Browser storage rejected the update, so this entry could still be there after a reload.".into())});
            }
            this.state.publish(stash.entries);
            let expired_names=entry.files.as_deref().unwrap_or_default().iter().filter(|file|expired.contains(&file.attachment_id)).map(|file|file.name.clone()).collect::<Vec<_>>();
            let mut missing=vec![];
            if !expired_names.is_empty(){missing.push(format!("{}: stashed files are kept for 24 hours and this upload expired. Attach the file again.",expired_names.join(", ")));}
            if !entry.dropped_image_names.is_empty(){missing.push(format!("{} were too large to save.",entry.dropped_image_names.join(", ")));}
            if let Some(names)=entry.unreadable_image_names.as_ref().filter(|names|!names.is_empty()){missing.push(format!("{} could not be read when this prompt was saved.",names.join(", ")));}
            if !missing.is_empty(){notices.push(StashNotice{warning:true,title:"Some attachments were not restored".into(),description:Some(missing.join(" "))});}
            Ok(StashCompletion {
                notices,
                menu: StashMenu::Close,
            })
        }
        .boxed_local())
    }
    pub fn delete(
        &self,
        id: String,
    ) -> LocalBoxFuture<'static, Result<StashCompletion, ClientError>> {
        let this = self.clone();
        async move {
            let _guard = this.state.gate.lock().await;
            let stored = read(&this.platform).await?;
            let mut stash = Stash {
                entries: stored.entries.clone(),
            };
            let mut failure = None;
            stash
                .take(&id, Some(&stored.entries), async |entries| {
                    match persist(&this.platform, &stored, entries).await {
                        Ok(()) => WriteOutcome {
                            written: true,
                            durable: true,
                        },
                        Err(error) => {
                            failure = Some(error);
                            WriteOutcome::default()
                        }
                    }
                })
                .await;
            if let Some(error) = failure {
                return Err(error);
            }
            let empty = stash.entries.is_empty();
            this.state.publish(stash.entries);
            Ok(StashCompletion {
                notices: vec![],
                menu: if empty {
                    StashMenu::Close
                } else {
                    StashMenu::Keep
                },
            })
        }
        .boxed_local()
    }
}
