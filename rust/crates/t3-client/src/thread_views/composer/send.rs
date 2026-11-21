//! Durable composer submission and the shell-driven delivery loop shared by every client.
use super::super::outbox::{
    Delivery, DeliveryResult, DispatchMode, DraftAttachment, Outbox, QueuedCreation, QueuedMessage,
};
use super::delivery_policy::ThreadSettings;
use crate::{
    ClientError, ClientStore,
    operations::{CommandStatus, OperationQueue},
    platform::{
        HostFuture, NetworkStatus, Platform, PlatformEvent, Storage, StorageArea, StorageKey,
        StorageTransaction, StorageWrite,
    },
    rpc::RpcEndpoint,
    types::{CommandId, EnvironmentId, Generation, ThreadKey},
};
use futures::{
    FutureExt, Stream, StreamExt, future::LocalBoxFuture, lock::Mutex, stream::LocalBoxStream,
};
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    pin::Pin,
    rc::{Rc, Weak},
    task::{Context, Poll, Waker},
};
use t3_contracts::{
    base_schemas::MessageId, composer_context::OrchestrationMessageContext,
    orchestration_v2::OrchestrationV2CreationSource, server::ServerConfig,
};

#[derive(Debug, Clone, Default)]
pub struct SendOptions {
    pub attachments: Vec<DraftAttachment>,
    pub context: Option<OrchestrationMessageContext>,
    pub settings: Option<ThreadSettings>,
    pub creation: Option<QueuedCreation>,
    pub dispatch_mode: Option<DispatchMode>,
    pub compact_before_send: bool,
    pub fresh_context: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendState {
    Queued,
    Preparing,
    Dispatching,
    Retry { not_before: u64, reason: String },
    AwaitingConfirmation,
    Accepted { sequence: Option<u64> },
    Restored { draft_key: String, reason: String },
}
#[derive(Debug, Clone, PartialEq)]
pub struct PendingMessage {
    pub message: Rc<QueuedMessage>,
    pub state: SendState,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Overlay(pub Option<QueuedMessage>);
type PreparedAttachments = (
    Vec<DraftAttachment>,
    Vec<t3_contracts::chat_attachment::ChatAttachment>,
);

#[derive(Debug, Clone)]
pub struct FanoutTarget {
    pub model: t3_contracts::model_selection::ModelSelection,
    pub text: String,
}
#[derive(Debug)]
pub struct FanoutSubmission {
    pub thread: ThreadKey,
    pub model: t3_contracts::model_selection::ModelSelection,
    pub message: Result<MessageId, ClientError>,
}

pub(super) struct SharedStorage<P>(pub Rc<P>);
impl<P> Clone for SharedStorage<P> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<P: Storage> Storage for SharedStorage<P> {
    fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
        self.0.get(area, key)
    }
    fn write(&self, area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()> {
        self.0.write(area, writes)
    }
    fn list(&self, area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>> {
        self.0.list(area, namespace)
    }
    fn transaction(&self, transaction: StorageTransaction) -> HostFuture<'_, bool> {
        self.0.transaction(transaction)
    }
}
pub(super) struct Session<E> {
    pub generation: Generation,
    pub endpoint: Rc<E>,
    pub config: Rc<ServerConfig>,
    pub upload_base: Option<String>,
}
#[derive(Default)]
struct SignalSlot {
    dirty: bool,
    closed: bool,
    waker: Option<Waker>,
}
#[derive(Default)]
pub(crate) struct Changes {
    slots: RefCell<Vec<Weak<RefCell<SignalSlot>>>>,
}
struct ChangesStream(Rc<RefCell<SignalSlot>>);
impl Stream for ChangesStream {
    type Item = ();
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<()>> {
        let mut slot = self.0.borrow_mut();
        if slot.dirty {
            slot.dirty = false;
            Poll::Ready(Some(()))
        } else if slot.closed {
            Poll::Ready(None)
        } else {
            slot.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}
impl Changes {
    pub fn subscribe(&self) -> LocalBoxStream<'static, ()> {
        let slot = Rc::new(RefCell::new(SignalSlot {
            dirty: true,
            closed: false,
            waker: None,
        }));
        self.slots.borrow_mut().push(Rc::downgrade(&slot));
        Box::pin(ChangesStream(slot))
    }
    pub fn notify(&self) {
        let mut wakes = vec![];
        self.slots.borrow_mut().retain(|weak| {
            let Some(slot) = weak.upgrade() else {
                return false;
            };
            let mut slot = slot.borrow_mut();
            slot.dirty = true;
            if let Some(waker) = slot.waker.take() {
                wakes.push(waker);
            }
            true
        });
        for waker in wakes {
            waker.wake();
        }
    }
}
impl Drop for Changes {
    fn drop(&mut self) {
        for slot in self.slots.get_mut().iter().filter_map(Weak::upgrade) {
            let mut slot = slot.borrow_mut();
            slot.closed = true;
            if let Some(waker) = slot.waker.take() {
                waker.wake();
            }
        }
    }
}
pub(super) struct SendInner<P: Platform, E: RpcEndpoint> {
    pub environment: EnvironmentId,
    pub platform: Rc<P>,
    pub store: ClientStore,
    pub source: OrchestrationV2CreationSource,
    pub session: RefCell<Option<Session<E>>>,
    pub outbox: Outbox<SharedStorage<P>>,
    // Keep echoes until durable removal succeeds, including across history window changes.
    pub projected_echoes: RefCell<BTreeMap<MessageId, u64>>,
    pub commands: OperationQueue<Overlay>,
    pub states: RefCell<BTreeMap<MessageId, SendState>>,
    pub attempts: RefCell<BTreeMap<MessageId, u32>>,
    pub epochs: RefCell<BTreeMap<crate::optimistic::ProjectionKey, (Generation, u64)>>,
    pub resolved_entities: RefCell<BTreeSet<CommandId>>,
    pub editing: RefCell<BTreeSet<MessageId>>,
    pub delivery: Mutex<Delivery>,
    pub step_lock: Mutex<()>,
    pub loaded: Cell<bool>,
    pub load_lock: Mutex<()>,
    pub disposed: Cell<bool>,
    pub running: Cell<bool>,
    /// The delivery loop could not step (storage, unreadable records): how many times in a row,
    /// until when it waits before trying again, and why.
    pub stall: RefCell<Option<(u32, u64, String)>>,
    pub changes: Changes,
    pub interrupted: Changes,
    pub prepared: RefCell<BTreeMap<MessageId, PreparedAttachments>>,
    pub launches: RefCell<
        BTreeMap<
            MessageId,
            (
                u64,
                t3_contracts::orchestration_v2::OrchestrationV2ThreadLaunchInput,
            ),
        >,
    >,
    pub background_spawn: RefCell<Option<crate::composition::Spawner>>,
    pub checkout_links: RefCell<BTreeMap<MessageId, Option<Rc<ClientError>>>>,
    pub fanout_prepared: RefCell<BTreeMap<String, PreparedAttachments>>,
    pub failure: RefCell<Option<super::send_transport::Failure>>,
    pub drafts: RefCell<Option<Rc<super::mobile_store::MobileDraftStore<P>>>>,
    pub web_drafts: RefCell<Option<Rc<super::draft_service::ComposerDraftService<P>>>>,
    /// Messages the server refused, kept visible until dismissed. Their text was restored into
    /// the thread's draft and their outbox row removed; this list lives in memory only.
    pub refused: RefCell<Vec<Refused>>,
}
pub(super) struct Refused {
    pub message: Rc<QueuedMessage>,
    pub draft_key: String,
    /// The draft text as recovery left it, for hosts without an attached draft store.
    pub text: Option<String>,
}
pub struct SendService<P: Platform, E: RpcEndpoint> {
    pub(super) inner: Rc<SendInner<P, E>>,
}
impl<P: Platform, E: RpcEndpoint> Clone for SendService<P, E> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}
impl<P: Platform + 'static, E: RpcEndpoint + 'static> SendService<P, E> {
    pub fn new(
        environment: EnvironmentId,
        platform: Rc<P>,
        store: ClientStore,
        source: OrchestrationV2CreationSource,
    ) -> Self {
        Self {
            inner: Rc::new(SendInner {
                commands: OperationQueue::new(environment.clone()),
                environment,
                platform: platform.clone(),
                store,
                source,
                session: RefCell::new(None),
                outbox: Outbox::new(SharedStorage(platform)),
                projected_echoes: RefCell::default(),
                states: RefCell::new(BTreeMap::new()),
                attempts: RefCell::new(BTreeMap::new()),
                epochs: RefCell::new(BTreeMap::new()),
                resolved_entities: RefCell::new(BTreeSet::new()),
                editing: RefCell::new(BTreeSet::new()),
                delivery: Mutex::new(Delivery::default()),
                step_lock: Mutex::new(()),
                loaded: Cell::new(false),
                load_lock: Mutex::new(()),
                disposed: Cell::new(false),
                running: Cell::new(false),
                stall: RefCell::new(None),
                changes: Changes::default(),
                interrupted: Changes::default(),
                prepared: RefCell::new(BTreeMap::new()),
                launches: RefCell::new(BTreeMap::new()),
                background_spawn: RefCell::new(None),
                checkout_links: RefCell::new(BTreeMap::new()),
                fanout_prepared: RefCell::new(BTreeMap::new()),
                failure: RefCell::new(None),
                drafts: RefCell::new(None),
                web_drafts: RefCell::new(None),
                refused: RefCell::new(Vec::new()),
            }),
        }
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        self.inner.changes.subscribe()
    }
    pub fn attach_drafts(&self, drafts: Rc<super::mobile_store::MobileDraftStore<P>>) {
        *self.inner.drafts.borrow_mut() = Some(drafts);
    }
    pub(crate) fn attach_background_spawner(&self, spawn: crate::composition::Spawner) {
        *self.inner.background_spawn.borrow_mut() = Some(spawn);
    }
    pub fn checkout_pull_request_link_error(&self, id: &MessageId) -> Option<Rc<ClientError>> {
        self.inner
            .checkout_links
            .borrow()
            .get(id)
            .cloned()
            .flatten()
    }
    pub fn state(&self, id: &MessageId) -> Option<SendState> {
        self.inner.states.borrow().get(id).cloned()
    }
    pub(super) fn set_state(&self, id: &MessageId, state: SendState) {
        self.inner.states.borrow_mut().insert(id.clone(), state);
        self.inner.changes.notify();
    }
    pub fn connect(
        &self,
        generation: Generation,
        endpoint: Rc<E>,
        config: Rc<ServerConfig>,
    ) -> Result<(), ClientError> {
        if endpoint.destination().as_ref() != Some(&self.inner.environment)
            || config.environment.environment_id != self.inner.environment
        {
            return Err(ClientError::EnvironmentMismatch);
        }
        let changed = self.inner.session.borrow().as_ref().is_none_or(|session| {
            session.generation != generation || !Rc::ptr_eq(&session.endpoint, &endpoint)
        });
        if changed {
            self.inner.epochs.borrow_mut().clear();
            self.inner.resolved_entities.borrow_mut().clear();
        }
        let upload_base = (!changed)
            .then(|| {
                self.inner
                    .session
                    .borrow()
                    .as_ref()
                    .and_then(|session| session.upload_base.clone())
            })
            .flatten();
        *self.inner.session.borrow_mut() = Some(Session {
            generation,
            endpoint,
            config,
            upload_base,
        });
        self.wake();
        Ok(())
    }
    pub fn connect_with_upload_base(
        &self,
        generation: Generation,
        endpoint: Rc<E>,
        config: Rc<ServerConfig>,
        base: &str,
    ) -> Result<(), ClientError> {
        let base = crate::environment::normalize_http_base_url(base)
            .map_err(|_| ClientError::InvalidPayload("The upload origin is invalid.".into()))?;
        self.connect(generation, endpoint, config)?;
        if let Some(session) = self.inner.session.borrow_mut().as_mut() {
            session.upload_base = Some(base);
        }
        Ok(())
    }
    pub fn disconnect(&self) {
        self.inner.session.borrow_mut().take();
        self.inner.commands.disconnect();
        self.inner.interrupted.notify();
        self.inner.changes.notify();
    }
    pub fn wake(&self) {
        self.inner
            .states
            .borrow_mut()
            .retain(|_, state| !matches!(state, SendState::Retry { .. }));
        self.inner.attempts.borrow_mut().clear();
        self.inner.changes.notify();
    }
    pub fn dispose(&self) {
        self.inner.disposed.set(true);
        self.inner.launches.borrow_mut().clear();
        self.inner.interrupted.notify();
        self.inner.changes.notify();
    }
    pub fn hold_edit(&self, id: MessageId) {
        self.inner.editing.borrow_mut().insert(id);
        self.inner.changes.notify();
    }
    pub fn release_edit(&self, id: &MessageId) {
        self.inner.editing.borrow_mut().remove(id);
        self.inner.changes.notify();
    }
    pub fn retry_message(&self, id: &MessageId) -> Result<(), ClientError> {
        if matches!(self.state(id), Some(SendState::Restored { .. })) {
            return Err(ClientError::InvalidPayload(
                "This message was refused and its text is back in the composer; send it again from there."
                    .into(),
            ));
        }
        if matches!(self.state(id), Some(SendState::AwaitingConfirmation)) {
            return Err(ClientError::InvalidPayload(
                "Wait for delivery confirmation before retrying this message.".into(),
            ));
        }
        self.inner.attempts.borrow_mut().remove(id);
        self.inner.states.borrow_mut().remove(id);
        self.inner.changes.notify();
        Ok(())
    }
    pub async fn update_pending(
        &self,
        id: &MessageId,
        text: String,
        options: SendOptions,
    ) -> Result<bool, ClientError> {
        self.load().await?;
        let creation = self
            .inner
            .outbox
            .messages()
            .iter()
            .any(|message| &message.message_id == id && message.creation.is_some());
        let _guard = if creation {
            None
        } else {
            Some(self.inner.step_lock.lock().await)
        };
        let Some(current) = self.inner.outbox.messages().into_iter().find(|message| {
            message.environment_id == self.inner.environment && &message.message_id == id
        }) else {
            return Ok(false);
        };
        if self.inner.commands.status(&current.command_id).is_some()
            && !(current.creation.is_some() && self.inner.launches.borrow().contains_key(id))
        {
            return Err(ClientError::InvalidPayload(
                "This message has already entered delivery. Wait for its result before editing it."
                    .into(),
            ));
        }
        if text.trim().is_empty() && options.attachments.is_empty() {
            return Err(ClientError::InvalidPayload(
                "Write a message or attach a file.".into(),
            ));
        }
        let revision = self.inner.outbox.revision(id);
        let mut next = current.as_ref().clone();
        next.text = text;
        next.attachments = options.attachments;
        next.context = options.context;
        if let Some(settings) = options.settings {
            next.model_selection = Some(settings.model_selection);
            next.runtime_mode = Some(settings.runtime_mode);
            next.interaction_mode = Some(settings.interaction_mode);
        }
        next.dispatch_mode = options.dispatch_mode;
        next.compact_before_send = options.compact_before_send.then_some(true);
        next.fresh_context = options.fresh_context.then_some(true);
        let changed = self.inner.outbox.update(next, Some(revision)).await?;
        if changed {
            self.inner.prepared.borrow_mut().remove(id);
            self.set_state(id, SendState::Queued);
            self.inner.attempts.borrow_mut().remove(id);
        }
        Ok(changed)
    }
    pub async fn discard_pending(
        &self,
        id: &MessageId,
    ) -> Result<Option<Rc<QueuedMessage>>, ClientError> {
        self.load().await?;
        let _guard = self.inner.step_lock.lock().await;
        let Some(current) = self.inner.outbox.messages().into_iter().find(|message| {
            message.environment_id == self.inner.environment && &message.message_id == id
        }) else {
            return Ok(None);
        };
        if self.inner.commands.status(&current.command_id).is_some() {
            return Err(ClientError::InvalidPayload("This message has already entered delivery. Wait for its result before discarding it.".into()));
        }
        let revision = self.inner.outbox.revision(id);
        let removed = self
            .inner
            .outbox
            .remove(&current, Some(revision), || true)
            .await?;
        if removed.is_some() {
            self.inner.prepared.borrow_mut().remove(id);
            self.inner.launches.borrow_mut().remove(id);
            self.inner.states.borrow_mut().remove(id);
            self.inner.attempts.borrow_mut().remove(id);
            self.inner.changes.notify();
        }
        Ok(removed)
    }
    pub async fn flush_outbox(&self) -> Result<bool, ClientError> {
        let previously_loaded = self.inner.loaded.get();
        self.load().await?;
        if previously_loaded && !self.inner.outbox.completely_loaded() {
            let _guard = self.inner.load_lock.lock().await;
            self.inner.outbox.load().await?;
        }
        for message in self.inner.outbox.messages() {
            if !self.inner.outbox.confirm_queued(&message).await {
                return Ok(false);
            }
        }
        Ok(self.inner.outbox.completely_loaded())
    }
    pub fn attachment_owners(&self) -> Vec<serde_json::Value> {
        let mut owners = self
            .inner
            .outbox
            .messages()
            .into_iter()
            .map(|message| (message.message_id.clone(), message))
            .collect::<BTreeMap<_, _>>();
        for record in self.inner.commands.records() {
            if let Some(message) = record.change.apply.0 {
                owners
                    .entry(message.message_id.clone())
                    .or_insert(Rc::new(message));
            }
        }
        owners
            .values()
            .map(|message| serde_json::json!({"attachments":message.attachments}))
            .collect()
    }
    pub async fn load(&self) -> Result<(), ClientError> {
        let _guard = self.inner.load_lock.lock().await;
        if self.inner.loaded.get() {
            return Ok(());
        }
        self.inner.outbox.load().await?;
        let report = self
            .inner
            .commands
            .restore_available(self.inner.platform.as_ref())
            .await
            .map_err(super::send_transport::operation_error)?;
        if !report.errors.is_empty() {
            return Err(ClientError::InvalidPayload(
                "Unreadable delivery records were retained for recovery.".into(),
            ));
        }
        self.inner.loaded.set(true);
        self.inner.changes.notify();
        Ok(())
    }
    pub async fn send_message(
        &self,
        key: &ThreadKey,
        text: String,
    ) -> Result<MessageId, ClientError> {
        self.submit(
            key,
            text,
            SendOptions {
                dispatch_mode: Some(DispatchMode::Auto),
                ..Default::default()
            },
        )
        .await
    }
    pub async fn submit(
        &self,
        key: &ThreadKey,
        text: String,
        options: SendOptions,
    ) -> Result<MessageId, ClientError> {
        self.enqueue_submission(key, text, options, None, None)
            .await
    }
    pub async fn edit_queued_run(
        &self,
        key: &ThreadKey,
        run: t3_contracts::base_schemas::RunId,
        message: MessageId,
        text: String,
        options: SendOptions,
    ) -> Result<MessageId, ClientError> {
        if options.creation.is_some() || options.compact_before_send || options.fresh_context {
            return Err(ClientError::InvalidPayload(
                "Queued edits cannot create or restart a thread.".into(),
            ));
        }
        self.enqueue_submission(key, text, options, Some((run, message)), None)
            .await
    }
    pub async fn submit_multiple(
        &self,
        targets: Vec<FanoutTarget>,
        options: SendOptions,
    ) -> Result<Vec<FanoutSubmission>, ClientError> {
        let valid = targets.len() > 1
            && options.creation.as_ref().is_some_and(|creation| {
                creation.workspace_mode == super::super::outbox::WorkspaceMode::Worktree
                    && creation
                        .branch
                        .as_ref()
                        .is_some_and(|branch| !branch.trim().is_empty())
            })
            && options.settings.is_some();
        if !valid {
            return Err(ClientError::InvalidPayload(
                "Multiple models require a new thread, a Git base branch, and captured settings."
                    .into(),
            ));
        }
        let mut seen = BTreeSet::new();
        let mut submissions = vec![];
        for target in targets {
            if !seen.insert(serde_json::to_string(&target.model)?) {
                return Err(ClientError::InvalidPayload(
                    "Choose each model once.".into(),
                ));
            }
            let thread = ThreadKey {
                environment: self.inner.environment.clone(),
                thread: t3_contracts::base_schemas::ThreadId(
                    crate::operations::allocate_command_id(self.inner.platform.as_ref(), None)?.0,
                ),
            };
            let mut options = options.clone();
            if let Some(settings) = options.settings.as_mut() {
                settings.model_selection = target.model.clone();
                settings.interaction_mode =
                    t3_contracts::provider_policy::ProviderInteractionMode::Default;
            }
            submissions.push((thread, target, options));
        }
        let group = crate::operations::allocate_command_id(self.inner.platform.as_ref(), None)?.0;
        Ok(
            futures::future::join_all(submissions.into_iter().map(|(thread, target, options)| {
                let group = group.clone();
                async move {
                    let message = self
                        .enqueue_submission(&thread, target.text, options, None, Some(group))
                        .await;
                    FanoutSubmission {
                        thread,
                        model: target.model,
                        message,
                    }
                }
            }))
            .await,
        )
    }
    async fn enqueue_submission(
        &self,
        key: &ThreadKey,
        text: String,
        options: SendOptions,
        edit: Option<(t3_contracts::base_schemas::RunId, MessageId)>,
        fanout_group: Option<String>,
    ) -> Result<MessageId, ClientError> {
        if key.environment != self.inner.environment {
            return Err(ClientError::EnvironmentMismatch);
        }
        if self.inner.disposed.get() {
            return Err(ClientError::Cancelled);
        }
        if text.trim().is_empty() && options.attachments.is_empty() {
            return Err(ClientError::InvalidPayload(
                "Write a message or attach a file.".into(),
            ));
        }
        let captured_draft = self.capture_web_draft(key, &text);
        if let Some(validation) =
            super::submission::get_composer_prompt_length_validation_message(&text)?
        {
            return Err(ClientError::Rejected {
                message: format!(
                    "Prompt is {} {} over the 120,000-character limit. Shorten or split it before sending.",
                    validation.excess,
                    if validation.excess == 1 {
                        "character"
                    } else {
                        "characters"
                    }
                ),
                detail: None,
            });
        }
        self.load().await?;
        let settings = options
            .settings
            .or_else(|| self.thread_settings(key))
            .ok_or_else(|| {
                ClientError::InvalidPayload("Thread settings are not available yet.".into())
            })?;
        let command_id =
            crate::operations::allocate_command_id(self.inner.platform.as_ref(), None)?;
        let mut extra = serde_json::Map::new();
        if let Some((_, draft)) = &captured_draft {
            extra.insert("composerDraft".into(), draft.clone());
        }
        if let Some(group) = fanout_group {
            extra.insert("fanoutGroup".into(), serde_json::Value::String(group));
        }
        let message_id = if let Some((run, message)) = edit {
            if self
                .inner
                .outbox
                .messages()
                .iter()
                .any(|queued| queued.message_id == message)
            {
                return Err(ClientError::InvalidPayload(
                    "This queued message already has a pending edit.".into(),
                ));
            }
            extra.insert("queuedRunId".into(), serde_json::to_value(run)?);
            message
        } else {
            MessageId(crate::operations::allocate_command_id(self.inner.platform.as_ref(), None)?.0)
        };
        let message = QueuedMessage {
            environment_id: key.environment.clone(),
            thread_id: key.thread.clone(),
            message_id: message_id.clone(),
            command_id,
            text,
            context: options.context,
            attachments: options.attachments,
            model_selection: Some(settings.model_selection),
            runtime_mode: Some(settings.runtime_mode),
            interaction_mode: Some(settings.interaction_mode),
            creation: options.creation,
            dispatch_mode: options.dispatch_mode,
            compact_before_send: options.compact_before_send.then_some(true),
            fresh_context: options.fresh_context.then_some(true),
            created_at: crate::time::iso_from_epoch_ms(self.inner.platform.now_epoch_ms()),
            extra,
        };
        let pending = self.inner.outbox.enqueue(message);
        self.set_state(&message_id, SendState::Queued);
        if let Err(error) = pending.await {
            self.inner.states.borrow_mut().remove(&message_id);
            self.inner.changes.notify();
            return Err(error);
        }
        self.accept_web_draft(key, captured_draft).await?;
        self.inner.changes.notify();
        Ok(message_id)
    }
    pub(super) fn thread_settings(&self, key: &ThreadKey) -> Option<ThreadSettings> {
        let snapshot = self.inner.store.snapshot();
        let environment = snapshot.environments.get(&key.environment)?;
        let thread = environment
            .shell
            .snapshot
            .as_ref()?
            .threads
            .iter()
            .find(|thread| thread.id == key.thread)?;
        Some(ThreadSettings {
            model_selection: thread.model_selection.clone(),
            runtime_mode: thread.runtime_mode,
            interaction_mode: thread.interaction_mode,
        })
    }
    pub(super) fn projection_epoch(&self, key: &ThreadKey) -> u64 {
        self.inner
            .store
            .snapshot()
            .environments
            .get(&key.environment)
            .and_then(|environment| environment.threads.get(&key.thread))
            .map_or(0, |thread| thread.generation)
    }
    /// Stops showing a refused message (`SendState::Restored`); its text stays in the draft.
    pub fn dismiss_message(&self, id: &MessageId) {
        let removed = {
            let mut refused = self.inner.refused.borrow_mut();
            let before = refused.len();
            refused.retain(|entry| &entry.message.message_id != id);
            before != refused.len()
        };
        if removed {
            self.inner.states.borrow_mut().remove(id);
            self.inner.changes.notify();
        }
    }
    /// The draft the latest refused message of `key` was restored into: the attached draft
    /// store's current text, else the text recovery wrote.
    pub fn draft_text(&self, key: &ThreadKey) -> Option<String> {
        let refused = self.inner.refused.borrow();
        let entry = refused
            .iter()
            .rev()
            .find(|entry| entry.message.thread_key() == *key)?;
        if let Some(drafts) = self.inner.web_drafts.borrow().as_ref() {
            return drafts.document().borrow()["draftsByThreadKey"][&entry.draft_key]["prompt"]
                .as_str()
                .map(str::to_owned);
        }
        match self.inner.drafts.borrow().as_ref() {
            Some(drafts) => drafts.snapshot()["drafts"][&entry.draft_key]["text"]
                .as_str()
                .map(str::to_owned),
            None => entry.text.clone(),
        }
    }
    pub fn pending_messages(&self, key: &ThreadKey) -> Vec<PendingMessage> {
        self.pending(key, false)
    }
    pub fn pending_edits(&self, key: &ThreadKey) -> Vec<PendingMessage> {
        self.pending(key, true)
    }
    fn pending(&self, key: &ThreadKey, edits: bool) -> Vec<PendingMessage> {
        use crate::optimistic::ProjectionKey;
        let base = Rc::new(Vec::<PendingMessage>::new());
        let queued = self.inner.commands.view(
            &base,
            &ProjectionKey::thread(key.environment.clone(), key.thread.clone()),
            |messages, change, status| {
                if let Some(message) = &change.apply.0 {
                    messages.push(PendingMessage {
                        message: Rc::new(message.clone()),
                        state: match status {
                            CommandStatus::Accepted { sequence } => SendState::Accepted {
                                sequence: Some(*sequence),
                            },
                            CommandStatus::UnknownDelivery => SendState::AwaitingConfirmation,
                            _ => SendState::Dispatching,
                        },
                    });
                }
            },
        );
        let mut result = queued.as_ref().clone();
        for message in self
            .inner
            .outbox
            .messages()
            .into_iter()
            .filter(|message| message.thread_key() == *key)
        {
            if !result
                .iter()
                .any(|item| item.message.message_id == message.message_id)
            {
                let state = self.state(&message.message_id).unwrap_or(SendState::Queued);
                result.push(PendingMessage { message, state });
            }
        }
        if !edits {
            for entry in self.inner.refused.borrow().iter() {
                if entry.message.thread_key() == *key
                    && !result
                        .iter()
                        .any(|item| item.message.message_id == entry.message.message_id)
                    && let Some(state) = self.state(&entry.message.message_id)
                {
                    result.push(PendingMessage {
                        message: entry.message.clone(),
                        state,
                    });
                }
            }
        }
        if result.is_empty() {
            return result;
        }
        let snapshot = self.inner.store.snapshot();
        let seen = snapshot
            .environments
            .get(&key.environment)
            .and_then(|environment| environment.threads.get(&key.thread))
            .and_then(|thread| thread.projection.as_ref())
            .map_or_else(BTreeSet::new, |projection| {
                super::send_transport::projected_user_message_ids(projection).collect()
            });
        result.retain(|item| {
            let id = &item.message.message_id;
            item.message.extra.contains_key("queuedRunId") == edits
                && (edits || (!seen.contains(id) && !self.has_projected_echo(id)))
        });
        result.sort_by(|a, b| {
            (&a.message.created_at, &a.message.message_id)
                .cmp(&(&b.message.created_at, &b.message.message_id))
        });
        result
    }
    pub(super) fn has_projected_echo(&self, id: &MessageId) -> bool {
        self.inner.projected_echoes.borrow().get(id) == Some(&self.inner.outbox.revision(id))
    }
    pub(super) fn current(&self, generation: Generation) -> bool {
        !self.inner.disposed.get()
            && self
                .inner
                .session
                .borrow()
                .as_ref()
                .is_some_and(|session| session.generation == generation)
    }
    pub async fn step(&self) -> Result<Option<DeliveryResult>, ClientError> {
        self.load().await?;
        let _guard = self.inner.step_lock.lock().await;
        self.reconcile().await?;
        let Some(generation) = self
            .inner
            .session
            .borrow()
            .as_ref()
            .map(|session| session.generation)
        else {
            return Ok(None);
        };
        let messages = self.inner.outbox.messages();
        let echo = messages
            .iter()
            .find(|message| {
                message.environment_id == self.inner.environment
                    && self.has_projected_echo(&message.message_id)
            })
            .cloned();
        let acknowledged = echo.is_some();
        let candidate = echo.or_else(|| {
            if !self
                .inner
                .store
                .snapshot()
                .environments
                .get(&self.inner.environment)
                .is_some_and(|environment| {
                    environment.shell.session == generation
                        && environment.shell.status == crate::types::SyncStatus::Live
                })
            {
                return None;
            }
            let now = self.inner.platform.monotonic_ms();
            let mut seen = BTreeSet::new();
            messages
                .into_iter()
                .filter(|message| {
                    message.environment_id == self.inner.environment
                        && seen.insert(message.thread_id.clone())
                })
                .find(|message| {
                    !self.inner.editing.borrow().contains(&message.message_id)
                        && !matches!(
                            self.state(&message.message_id),
                            Some(SendState::Retry { not_before, .. }) if not_before > now
                        )
                        && !matches!(
                            self.state(&message.message_id),
                            Some(SendState::AwaitingConfirmation)
                        )
                })
        });
        let Some(message) = candidate else {
            return Ok(None);
        };
        let result = if acknowledged {
            let revision = self.inner.outbox.revision(&message.message_id);
            if self
                .inner
                .outbox
                .remove(&message, Some(revision), || self.current(generation))
                .await?
                .is_none()
            {
                return Ok(Some(DeliveryResult::StaleMessage));
            }
            self.inner
                .projected_echoes
                .borrow_mut()
                .remove(&message.message_id);
            self.inner
                .delivery
                .lock()
                .await
                .clear_acknowledgement(&message.message_id);
            Ok(DeliveryResult::Accepted(None))
        } else {
            if !self.inner.outbox.confirm_queued(&message).await {
                self.inner.outbox.enqueue(message.as_ref().clone()).await?;
                self.inner.changes.notify();
                return Ok(Some(DeliveryResult::Retry));
            }
            let transport = super::send_transport::ProductionDelivery(self.clone());
            let gate = || {
                self.current(generation)
                    && !self.inner.editing.borrow().contains(&message.message_id)
            };
            let mut delivery = self.inner.delivery.lock().await;
            delivery
                .deliver(
                    &self.inner.outbox,
                    &transport,
                    message.clone(),
                    super::super::outbox::DispatchStep::Send,
                    gate,
                )
                .await
        };
        let outcome = match result {
            Ok(DeliveryResult::Accepted(sequence)) => {
                self.link_checkout_pull_request(&message).await;
                self.set_state(&message.message_id, SendState::Accepted { sequence });
                self.inner.prepared.borrow_mut().remove(&message.message_id);
                self.inner.attempts.borrow_mut().remove(&message.message_id);
                self.inner.launches.borrow_mut().remove(&message.message_id);
                DeliveryResult::Accepted(sequence)
            }
            Ok(result) => result,
            Err(error) => match self.handle_failure(&message, error, generation).await {
                Ok(result) => result,
                Err(error) => {
                    self.schedule_retry(&message.message_id, error.to_string());
                    DeliveryResult::Retry
                }
            },
        };
        if let Some(group) = message
            .extra
            .get("fanoutGroup")
            .and_then(serde_json::Value::as_str)
            && !self.inner.outbox.messages().iter().any(|pending| {
                pending
                    .extra
                    .get("fanoutGroup")
                    .and_then(serde_json::Value::as_str)
                    == Some(group)
            })
        {
            self.inner.fanout_prepared.borrow_mut().remove(group);
        }
        self.inner.changes.notify();
        Ok(Some(outcome))
    }
    pub fn retry_deadline(&self) -> Option<u64> {
        let generation = self
            .inner
            .session
            .borrow()
            .as_ref()
            .map(|session| session.generation)?;
        if self.inner.disposed.get()
            || !self
                .inner
                .store
                .snapshot()
                .environments
                .get(&self.inner.environment)
                .is_some_and(|environment| {
                    environment.shell.session == generation
                        && environment.shell.status == crate::types::SyncStatus::Live
                })
        {
            return None;
        }
        let mut seen = BTreeSet::new();
        self.inner
            .outbox
            .messages()
            .iter()
            .filter(|message| {
                message.environment_id == self.inner.environment
                    && seen.insert(message.thread_id.clone())
                    && !self.inner.editing.borrow().contains(&message.message_id)
            })
            .filter_map(|message| match self.state(&message.message_id) {
                Some(SendState::Retry { not_before, .. }) => Some(not_before),
                _ => None,
            })
            .min()
    }
    /// Why delivery is stalled, while it waits to try again; `None` while it works.
    pub fn delivery_problem(&self) -> Option<String> {
        self.inner
            .stall
            .borrow()
            .as_ref()
            .map(|(_, _, reason)| reason.clone())
    }
    fn stalled(&self, error: &ClientError) {
        let now = self.inner.platform.monotonic_ms();
        let mut stall = self.inner.stall.borrow_mut();
        let attempts = stall.as_ref().map_or(0, |(attempts, ..)| *attempts) + 1;
        *stall = Some((
            attempts,
            now.saturating_add(super::super::outbox::retry_delay_ms(attempts)),
            error.to_string(),
        ));
        drop(stall);
        self.inner.changes.notify();
    }
    fn stepped(&self) {
        if self.inner.stall.borrow_mut().take().is_some() {
            self.inner.changes.notify();
        }
    }
    /// Deletes `environment`'s saved messages and delivery records, for a removed environment.
    pub async fn forget_saved(
        platform: Rc<P>,
        environment: &EnvironmentId,
    ) -> Result<(), ClientError> {
        Outbox::new(SharedStorage(platform.clone()))
            .clear_environment(environment)
            .await?;
        crate::operations::forget_environment(platform.as_ref(), environment).await
    }
    pub fn run(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.clone();
        async move {
            if service.inner.running.replace(true) {
                return Err(ClientError::InvalidPayload("The composer delivery loop is already running.".into()));
            }
            struct Running<P:Platform,E:RpcEndpoint>(Rc<SendInner<P,E>>);
            impl<P:Platform,E:RpcEndpoint> Drop for Running<P,E> {
                fn drop(&mut self) {self.0.running.set(false);}
            }
            let _running=Running(service.inner.clone());
            let mut changes=service.changes();
            let mut store=service.inner.store.subscribe_environment(&service.inner.environment).chain(futures::stream::pending());
            let mut events=service.inner.platform.events().chain(futures::stream::pending());
            let mut interrupted=service.inner.interrupted.subscribe();
            let _=interrupted.next().await;
            while !service.inner.disposed.get() {
                // A step that failed outright waits out its backoff, so it cannot spin on the
                // change it announced; the loop itself never ends on an error.
                let stalled_until = service.inner.stall.borrow().as_ref().map(|(_, until, _)| *until);
                if let Some(until) = stalled_until {
                    let wait = service.inner.platform.sleep(until.saturating_sub(service.inner.platform.monotonic_ms()));
                    futures::select_biased! {
                        _=interrupted.next().fuse()=>{
                            if service.inner.disposed.get(){return Ok(());}
                        },
                        _=wait.fuse()=>{},
                    }
                }
                loop {
                    let outcome=futures::select_biased! {
                        _=interrupted.next().fuse()=>{
                            if service.inner.disposed.get(){return Ok(());}
                            continue;
                        },
                        outcome=service.step().fuse()=>match outcome {
                            Ok(outcome)=>{service.stepped();outcome},
                            Err(error)=>{service.stalled(&error);None},
                        },
                    };
                    let Some(outcome)=outcome else {break;};
                    if matches!(outcome,DeliveryResult::Wait|DeliveryResult::Retry|DeliveryResult::StaleSession|DeliveryResult::StaleMessage) {
                        break;
                    }
                }
                if service.inner.stall.borrow().is_some() {
                    continue;
                }
                let timer=match service.retry_deadline() {
                    Some(deadline)=>service.inner.platform.sleep(deadline.saturating_sub(service.inner.platform.monotonic_ms())),
                    None=>futures::future::pending().boxed_local(),
                };
                futures::select_biased! {
                    _=changes.next().fuse()=>{},
                    _=store.next().fuse()=>{
                        if let Err(error)=service.reconcile().await {service.stalled(&error);}
                    },
                    event=events.next().fuse()=>{
                        if matches!(event,Some(PlatformEvent::Foreground|PlatformEvent::Network(NetworkStatus::Online))) {service.wake();}
                    },
                    _=timer.fuse()=>{},
                }
            }
            Ok(())
        }.boxed_local()
    }
}
