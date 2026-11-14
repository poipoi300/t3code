//! Durable queued messages; storage commits precede delivery and acknowledgements precede removal.
use crate::{
    ClientError,
    platform::{Storage, StorageArea, StorageKey, StorageWrite},
    types::{EnvironmentId, Generation, ThreadId, ThreadKey},
};
use futures::{FutureExt, future::LocalBoxFuture, lock::Mutex};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use t3_contracts::{
    base_schemas::{CommandId, MessageId, ProjectId},
    composer_context::OrchestrationMessageContext,
    model_selection::ModelSelection,
    provider_policy::{ProviderInteractionMode, RuntimeMode},
};
pub const WRITE_VERSION: u32 = 3;
const NAMESPACE: &str = "outbox";
fn invalid(message: &str) -> ClientError {
    ClientError::InvalidPayload(message.into())
}
/// Local attachment metadata includes durable host resource references, never host File handles.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct DraftAttachment(Value);
impl<'de> Deserialize<'de> for DraftAttachment {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        Self::decode(Value::deserialize(decoder)?).map_err(serde::de::Error::custom)
    }
}
impl DraftAttachment {
    pub fn decode(value: Value) -> Result<Self, ClientError> {
        let object = value
            .as_object()
            .ok_or_else(|| invalid("attachment must be an object"))?;
        for field in ["id", "name", "mimeType"] {
            if !object.get(field).is_some_and(Value::is_string) {
                return Err(invalid("invalid attachment metadata"));
            }
        }
        if !object.get("sizeBytes").is_some_and(Value::is_number) {
            return Err(invalid("attachment size must be a number"));
        }
        let string = |key: &str| object.get(key).and_then(Value::as_str);
        match string("type") {
            Some("image") => {
                if string("previewUri").is_none()
                    || !(["fileUri", "dataUrl"]
                        .iter()
                        .any(|key| string(key).is_some_and(|v| !v.is_empty())))
                {
                    return Err(invalid("image attachment has no durable bytes"));
                }
                for key in ["fileUri", "dataUrl"] {
                    if object.contains_key(key) && !string(key).is_some_and(|v| !v.is_empty()) {
                        return Err(invalid("invalid image resource"));
                    }
                }
            }
            Some("file") => {
                if string("fileUri").is_none() {
                    return Err(invalid("file attachment has no resource"));
                }
            }
            _ => return Err(invalid("unknown draft attachment type")),
        }
        for field in ["uploadedAttachmentId", "uploadEnvironmentId"] {
            if object.contains_key(field) && string(field).is_none() {
                return Err(invalid("invalid uploaded attachment metadata"));
            }
        }
        Ok(Self(value))
    }
    pub fn value(&self) -> &Value {
        &self.0
    }
    pub fn id(&self) -> &str {
        self.0["id"].as_str().unwrap_or_default()
    }
    pub fn name(&self) -> &str {
        self.0["name"].as_str().unwrap_or_default()
    }
    pub fn is_file(&self) -> bool {
        self.0["type"] == "file"
    }
    pub fn size_bytes(&self) -> f64 {
        self.0["sizeBytes"].as_f64().unwrap_or_default()
    }
    pub fn retarget(&self, environment: &EnvironmentId) -> Self {
        let mut value = self.0.clone();
        if value.get("uploadEnvironmentId").and_then(Value::as_str) != Some(environment.0.as_str())
            && let Some(object) = value.as_object_mut()
        {
            object.remove("uploadedAttachmentId");
            object.remove("uploadEnvironmentId");
        }
        Self(value)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DispatchMode {
    Auto,
    Queue,
    Steer,
    Restart,
}
pub use t3_contracts::environment::ThreadEnvMode as WorkspaceMode;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedCreation {
    pub project_id: ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub project_title: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub project_cwd: Option<String>,
    pub workspace_mode: WorkspaceMode,
    #[serde(with = "t3_contracts::wire::nullable")]
    pub branch: Option<String>,
    #[serde(with = "t3_contracts::wire::nullable")]
    pub worktree_path: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub start_from_origin: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub pull_request_reference: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub checkout_pull_request_url: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedMessage {
    pub environment_id: EnvironmentId,
    pub thread_id: ThreadId,
    pub message_id: MessageId,
    pub command_id: CommandId,
    pub text: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub context: Option<OrchestrationMessageContext>,
    pub attachments: Vec<DraftAttachment>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub model_selection: Option<ModelSelection>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub dispatch_mode: Option<DispatchMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub compact_before_send: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub fresh_context: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub runtime_mode: Option<RuntimeMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub interaction_mode: Option<ProviderInteractionMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub creation: Option<QueuedCreation>,
    pub created_at: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
impl QueuedMessage {
    pub fn thread_key(&self) -> ThreadKey {
        ThreadKey {
            environment: self.environment_id.clone(),
            thread: self.thread_id.clone(),
        }
    }
    pub fn creation_sendable(&self) -> bool {
        self.creation.as_ref().is_some_and(|c| {
            (!self.text.trim().is_empty() || !self.attachments.is_empty())
                && self.model_selection.is_some()
                && (c.workspace_mode != WorkspaceMode::Worktree
                    || c.branch.as_ref().is_some_and(|b| !b.is_empty()))
        })
    }
}
pub fn decode_message(mut value: Value) -> Result<QueuedMessage, ClientError> {
    let _: t3_contracts::base_schemas::IsoDateTime =
        serde_json::from_value(value["createdAt"].clone())?;
    let version = value
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("outbox schema version is missing"))?;
    if !(1..=4).contains(&version) {
        return Err(invalid("unsupported outbox version"));
    }
    if let Some(object) = value.as_object_mut() {
        object.remove("schemaVersion");
    }
    Ok(serde_json::from_value(value)?)
}
pub fn encode_message(message: &QueuedMessage) -> Result<Value, ClientError> {
    let mut value = serde_json::to_value(message)?;
    value["schemaVersion"] = WRITE_VERSION.into();
    Ok(value)
}
pub fn group_messages(
    messages: impl IntoIterator<Item = QueuedMessage>,
) -> BTreeMap<ThreadKey, Vec<QueuedMessage>> {
    let mut order = vec![];
    let mut dedup = BTreeMap::new();
    for message in messages {
        if !dedup.contains_key(&message.message_id) {
            order.push(message.message_id.clone());
        }
        dedup.insert(message.message_id.clone(), message);
    }
    let mut grouped: BTreeMap<ThreadKey, Vec<QueuedMessage>> = BTreeMap::new();
    for id in order {
        if let Some(message) = dedup.remove(&id) {
            grouped
                .entry(message.thread_key())
                .or_default()
                .push(message);
        }
    }
    for messages in grouped.values_mut() {
        messages.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    }
    grouped
}
pub fn retry_delay_ms(attempt: u32) -> u64 {
    1_000u64
        .saturating_mul(
            1u64.checked_shl(attempt.saturating_sub(1))
                .unwrap_or(u64::MAX),
        )
        .min(16_000)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FamilyRead {
    Pending,
    Failed,
    Settled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryAction {
    Wait,
    Remove,
    Send,
}
pub fn delivery_action(
    creation: bool,
    thread_exists: bool,
    shell_live: bool,
    connected: bool,
    family: Option<FamilyRead>,
) -> DeliveryAction {
    if creation {
        if thread_exists {
            return DeliveryAction::Remove;
        }
        return if connected && shell_live {
            DeliveryAction::Send
        } else {
            DeliveryAction::Wait
        };
    }
    if !thread_exists {
        return if shell_live && family.is_none_or(|f| f == FamilyRead::Settled) {
            DeliveryAction::Remove
        } else {
            DeliveryAction::Wait
        };
    }
    if connected {
        DeliveryAction::Send
    } else {
        DeliveryAction::Wait
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchStep {
    Wait,
    Remove,
    Retry,
    Restore(String),
    Send,
}
pub fn dispatch_step(
    action: DeliveryAction,
    files: &[DraftAttachment],
    config: Option<Option<u64>>,
) -> DispatchStep {
    match action {
        DeliveryAction::Wait => return DispatchStep::Wait,
        DeliveryAction::Remove => return DispatchStep::Remove,
        DeliveryAction::Send => {}
    }
    let Some(max) = config else {
        return DispatchStep::Retry;
    };
    if !files.iter().any(DraftAttachment::is_file) {
        return DispatchStep::Send;
    }
    let Some(max) = max else {
        return DispatchStep::Restore("This server does not support file attachments.".into());
    };
    let max = max.min(t3_contracts::chat_attachment::PROVIDER_SEND_TURN_MAX_FILE_BYTES);
    if let Some(file) = files
        .iter()
        .find(|f| f.is_file() && f.size_bytes() > max as f64)
    {
        let limit = if max >= 1_048_576 && max % 1_048_576 == 0 {
            format!("{} MB", max / 1_048_576)
        } else if max >= 1024 && max % 1024 == 0 {
            format!("{} KB", max / 1024)
        } else {
            format!("{max} {}", if max == 1 { "byte" } else { "bytes" })
        };
        return DispatchStep::Restore(format!(
            "'{}' exceeds the {limit} attachment limit.",
            file.name()
        ));
    }
    DispatchStep::Send
}

fn key(message: &QueuedMessage) -> StorageKey {
    StorageKey {
        namespace: NAMESPACE.into(),
        environment: Some(message.environment_id.clone()),
        record: message.message_id.0.clone(),
    }
}
pub struct LoadResult {
    pub messages: Vec<QueuedMessage>,
    pub errors: Vec<(StorageKey, ClientError)>,
}
pub async fn load(storage: &impl Storage) -> Result<LoadResult, ClientError> {
    let mut messages = vec![];
    let mut errors = vec![];
    for key in storage
        .list(StorageArea::Ordinary, NAMESPACE.into())
        .await?
    {
        let result = async {
            let bytes = storage
                .get(StorageArea::Ordinary, key.clone())
                .await?
                .ok_or_else(|| invalid("outbox record disappeared"))?;
            let value: Value = serde_json::from_slice(&bytes)?;
            let payload = if value.get("formatVersion").is_some() {
                let record: crate::persistence::Record<Value> = serde_json::from_value(value)?;
                if record.format_version != crate::persistence::FORMAT_VERSION
                    || record.kind != key.namespace
                    || record.environment_id != key.environment
                    || record.record_id != key.record
                    || record.payload_version != 1
                {
                    return Err(invalid("outbox record identity/version mismatch"));
                }
                record.payload
            } else {
                value
            };
            let message = decode_message(payload)?;
            if key
                .environment
                .as_ref()
                .is_some_and(|environment| environment != &message.environment_id)
                || key.record != message.message_id.0
            {
                return Err(invalid("outbox message identity mismatch"));
            }
            Ok(message)
        }
        .await;
        match result {
            Ok(message) => messages.push(message),
            Err(error) => errors.push((key, error)),
        }
    }
    Ok(LoadResult { messages, errors })
}
async fn write(storage: &impl Storage, message: &QueuedMessage) -> Result<(), ClientError> {
    storage
        .write(StorageArea::Ordinary, vec![message_storage_write(message)?])
        .await?;
    Ok(())
}
pub fn message_storage_write(message: &QueuedMessage) -> Result<StorageWrite, ClientError> {
    Ok(StorageWrite {
        key: key(message),
        value: Some(serde_json::to_vec(&crate::persistence::Record::new(
            NAMESPACE,
            Some(message.environment_id.clone()),
            message.message_id.0.clone(),
            1,
            encode_message(message)?,
        ))?),
    })
}
async fn remove(storage: &impl Storage, message: &QueuedMessage) -> Result<(), ClientError> {
    storage
        .write(
            StorageArea::Ordinary,
            vec![StorageWrite {
                key: key(message),
                value: None,
            }],
        )
        .await?;
    Ok(())
}
#[derive(Default)]
struct State {
    messages: Vec<Rc<QueuedMessage>>,
    revisions: BTreeMap<MessageId, u64>,
    durable: BTreeMap<MessageId, u64>,
    loaded: bool,
}
impl State {
    fn bump(&mut self, id: &MessageId) {
        *self.revisions.entry(id.clone()).or_default() += 1;
    }
    fn revision(&self, id: &MessageId) -> u64 {
        self.revisions.get(id).copied().unwrap_or_default()
    }
    fn winner(&self, id: &MessageId) -> Option<Rc<QueuedMessage>> {
        self.messages.iter().find(|m| &m.message_id == id).cloned()
    }
}
pub struct Outbox<S> {
    storage: S,
    state: RefCell<State>,
    mutations: Mutex<()>,
}
impl<S: Storage> Outbox<S> {
    pub fn new(storage: S) -> Self {
        Self {
            storage,
            state: RefCell::new(State::default()),
            mutations: Mutex::new(()),
        }
    }
    pub fn messages(&self) -> Vec<Rc<QueuedMessage>> {
        self.state.borrow().messages.clone()
    }
    pub fn revision(&self, id: &MessageId) -> u64 {
        self.state.borrow().revision(id)
    }
    pub async fn load(&self) -> Result<LoadResult, ClientError> {
        let _guard = self.mutations.lock().await;
        let loaded = load(&self.storage).await?;
        {
            let mut state = self.state.borrow_mut();
            for message in &loaded.messages {
                if state.winner(&message.message_id).is_none()
                    && !state.revisions.contains_key(&message.message_id)
                {
                    state.durable.insert(message.message_id.clone(), 0);
                    state.messages.push(Rc::new(message.clone()));
                }
            }
            state.loaded = loaded.errors.is_empty();
        }
        Ok(loaded)
    }
    pub fn completely_loaded(&self) -> bool {
        self.state.borrow().loaded
    }
    pub fn enqueue(
        &self,
        message: QueuedMessage,
    ) -> LocalBoxFuture<'_, Result<Rc<QueuedMessage>, ClientError>> {
        let message = Rc::new(message);
        {
            let mut state = self.state.borrow_mut();
            state.bump(&message.message_id);
            state.durable.remove(&message.message_id);
            state
                .messages
                .retain(|m| m.message_id != message.message_id);
            state.messages.push(message.clone());
        }
        async move {
            let _guard = self.mutations.lock().await;
            if let Err(error) = write(&self.storage, &message).await {
                let remains = {
                    let mut state = self.state.borrow_mut();
                    state.messages.retain(|m| !Rc::ptr_eq(m, &message));
                    state.winner(&message.message_id).is_some()
                };
                if !remains {
                    let _ = remove(&self.storage, &message).await;
                }
                return Err(error);
            }
            let mut state = self.state.borrow_mut();
            if state
                .winner(&message.message_id)
                .is_some_and(|current| Rc::ptr_eq(&current, &message))
            {
                let revision = state.revision(&message.message_id);
                state.durable.insert(message.message_id.clone(), revision);
            }
            Ok(message)
        }
        .boxed_local()
    }
    pub async fn confirm_queued(&self, message: &Rc<QueuedMessage>) -> bool {
        let _guard = self.mutations.lock().await;
        let state = self.state.borrow();
        state.durable.get(&message.message_id) == Some(&state.revision(&message.message_id))
            && state.messages.iter().any(|m| Rc::ptr_eq(m, message))
    }
    pub async fn update(
        &self,
        message: QueuedMessage,
        expected: Option<u64>,
    ) -> Result<bool, ClientError> {
        let _guard = self.mutations.lock().await;
        let stale = || {
            let state = self.state.borrow();
            state.winner(&message.message_id).is_none()
                || expected.is_some_and(|r| r != state.revision(&message.message_id))
        };
        if stale() {
            return Ok(false);
        }
        write(&self.storage, &message).await?;
        if stale() {
            let winner = self.state.borrow().winner(&message.message_id);
            if let Some(winner) = winner {
                write(&self.storage, &winner).await?;
            }
            return Ok(false);
        }
        let mut state = self.state.borrow_mut();
        state.bump(&message.message_id);
        let revision = state.revision(&message.message_id);
        state.durable.insert(message.message_id.clone(), revision);
        state
            .messages
            .retain(|m| m.message_id != message.message_id);
        state.messages.push(Rc::new(message));
        Ok(true)
    }
    pub async fn remove(
        &self,
        message: &QueuedMessage,
        expected: Option<u64>,
        can_remove: impl Fn() -> bool,
    ) -> Result<Option<Rc<QueuedMessage>>, ClientError> {
        let _guard = self.mutations.lock().await;
        let canceled =
            || expected.is_some_and(|r| r != self.revision(&message.message_id)) || !can_remove();
        if canceled() {
            return Ok(None);
        }
        let removed = self
            .state
            .borrow()
            .winner(&message.message_id)
            .unwrap_or_else(|| Rc::new(message.clone()));
        remove(&self.storage, message).await?;
        if canceled() {
            let winner = self.state.borrow().winner(&message.message_id);
            if let Some(winner) = winner {
                write(&self.storage, &winner).await?;
            }
            return Ok(None);
        }
        let mut state = self.state.borrow_mut();
        state
            .messages
            .retain(|m| m.message_id != message.message_id);
        state.bump(&message.message_id);
        state.durable.remove(&message.message_id);
        Ok(Some(removed))
    }
    pub async fn clear_environment(
        &self,
        environment: &EnvironmentId,
    ) -> Result<Vec<Rc<QueuedMessage>>, ClientError> {
        let requested = self.state.borrow().revisions.clone();
        let _guard = self.mutations.lock().await;
        let loaded = load(&self.storage).await?;
        if !loaded.errors.is_empty() {
            return Err(invalid("cannot clear a partially readable outbox"));
        }
        let current = self.messages();
        let all = group_messages(
            loaded
                .messages
                .into_iter()
                .chain(current.iter().map(|m| m.as_ref().clone())),
        );
        let mut removed = vec![];
        for message in all.into_values().flatten() {
            let revision = requested
                .get(&message.message_id)
                .copied()
                .unwrap_or_default();
            if &message.environment_id != environment
                || self.revision(&message.message_id) != revision
            {
                continue;
            }
            if remove(&self.storage, &message).await.is_err() {
                continue;
            }
            if self.revision(&message.message_id) != revision {
                let winner = self.state.borrow().winner(&message.message_id);
                if let Some(winner) = winner {
                    write(&self.storage, &winner).await?;
                }
                continue;
            }
            let mut state = self.state.borrow_mut();
            state
                .messages
                .retain(|m| m.message_id != message.message_id);
            state.bump(&message.message_id);
            state.durable.remove(&message.message_id);
            removed.push(Rc::new(message));
        }
        Ok(removed)
    }
}

/// Destination-bound operations implement delivery using authoritative RPC contracts.
pub trait DeliveryTransport {
    fn generation(&self, environment: &EnvironmentId) -> Generation;
    fn send<'a>(
        &'a self,
        message: &'a QueuedMessage,
    ) -> LocalBoxFuture<'a, Result<Option<u64>, ClientError>>;
    fn restore<'a>(
        &'a self,
        message: &'a QueuedMessage,
        reason: &'a str,
    ) -> LocalBoxFuture<'a, Result<bool, ClientError>>;
}
#[derive(Default)]
pub struct Delivery {
    acknowledged: BTreeMap<MessageId, (u64, Option<u64>)>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryResult {
    Wait,
    Removed,
    Accepted(Option<u64>),
    Restored,
    Retry,
    StaleSession,
    StaleMessage,
}
impl Delivery {
    pub async fn deliver<S: Storage>(
        &mut self,
        outbox: &Outbox<S>,
        transport: &impl DeliveryTransport,
        message: Rc<QueuedMessage>,
        step: DispatchStep,
        can_deliver: impl Fn() -> bool,
    ) -> Result<DeliveryResult, ClientError> {
        if !can_deliver() {
            return Ok(DeliveryResult::Wait);
        }
        let revision = outbox.revision(&message.message_id);
        if !outbox.confirm_queued(&message).await {
            return Ok(DeliveryResult::Wait);
        }
        if !can_deliver() {
            return Ok(DeliveryResult::Wait);
        }
        if let Some((accepted_revision, sequence)) =
            self.acknowledged.get(&message.message_id).copied()
        {
            if accepted_revision != revision {
                return Ok(DeliveryResult::StaleMessage);
            }
            return if outbox
                .remove(&message, Some(revision), &can_deliver)
                .await?
                .is_some()
            {
                self.acknowledged.remove(&message.message_id);
                Ok(DeliveryResult::Accepted(sequence))
            } else {
                Ok(DeliveryResult::Wait)
            };
        }
        match step {
            DispatchStep::Wait => Ok(DeliveryResult::Wait),
            DispatchStep::Retry => Ok(DeliveryResult::Retry),
            DispatchStep::Remove => Ok(
                if outbox
                    .remove(&message, Some(revision), can_deliver)
                    .await?
                    .is_some()
                {
                    DeliveryResult::Removed
                } else {
                    DeliveryResult::Wait
                },
            ),
            DispatchStep::Restore(reason) => {
                if !transport.restore(&message, &reason).await? {
                    return Ok(DeliveryResult::Wait);
                }
                Ok(
                    if outbox
                        .remove(&message, Some(revision), can_deliver)
                        .await?
                        .is_some()
                    {
                        DeliveryResult::Restored
                    } else {
                        DeliveryResult::Wait
                    },
                )
            }
            DispatchStep::Send => {
                let generation = transport.generation(&message.environment_id);
                let response = transport.send(&message).await;
                let sequence = response?;
                self.acknowledged
                    .insert(message.message_id.clone(), (revision, sequence));
                if transport.generation(&message.environment_id) != generation {
                    return Ok(DeliveryResult::StaleSession);
                }
                if outbox
                    .remove(&message, Some(revision), can_deliver)
                    .await?
                    .is_some()
                {
                    self.acknowledged.remove(&message.message_id);
                }
                Ok(DeliveryResult::Accepted(sequence))
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{platform::PlatformError, testing::FakePlatform};
    use futures::executor::block_on;
    use serde_json::json;
    fn value(version: u32) -> Value {
        json!({"schemaVersion":version,"environmentId":"e","threadId":"t","messageId":"m","commandId":"c","text":"hello","attachments":[],"createdAt":"2026-01-01T00:00:00.000Z"})
    }
    fn message() -> QueuedMessage {
        decode_message(value(3)).unwrap()
    }
    #[test]
    fn all_persisted_versions_and_optional_flags() {
        for version in 1..=4 {
            let mut raw = value(version);
            raw["freshContext"] = true.into();
            raw["compactBeforeSend"] = true.into();
            raw["dispatchMode"] = "restart".into();
            let decoded = decode_message(raw).unwrap();
            let encoded = encode_message(&decoded).unwrap();
            assert_eq!(encoded["schemaVersion"], 3);
            assert_eq!(encoded["freshContext"], true);
            assert_eq!(encoded["compactBeforeSend"], true);
            assert_eq!(decoded.dispatch_mode, Some(DispatchMode::Restart));
        }
        assert!(decode_message(value(5)).is_err());
        let mut v = value(3);
        v["runtimeMode"] = Value::Null;
        assert!(decode_message(v).is_err());
    }
    #[test]
    fn attachment_resources_and_metadata() {
        for (kind, extra) in [
            (
                "image",
                json!({"previewUri":"blob:x","dataUrl":"data:image/png,x"}),
            ),
            ("file", json!({"fileUri":"resource"})),
        ] {
            let mut a = json!({"id":"a","name":"file","mimeType":"x","sizeBytes":1,"type":kind,"uploadedAttachmentId":"upload","uploadEnvironmentId":"old"});
            a.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let a = DraftAttachment::decode(a).unwrap();
            let next = a.retarget(&EnvironmentId("e".into()));
            assert!(next.value().get("uploadedAttachmentId").is_none());
            assert_eq!(next.id(), "a");
        }
        assert!(DraftAttachment::decode(json!({"id":"a","type":"image","name":"a","mimeType":"x","sizeBytes":1,"previewUri":"x"})).is_err());
    }
    #[test]
    fn group_dedup_last_and_stable_time_order() {
        let a = message();
        let mut b = a.clone();
        b.text = "new".into();
        let mut c = a.clone();
        c.message_id = MessageId("c".into());
        let grouped = group_messages([a, b, c]);
        let rows = grouped.values().next().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].text, "new");
        assert_eq!(rows[1].message_id.0, "c");
    }
    #[test]
    fn backoff() {
        for (attempt, delay) in [
            (0, 1000),
            (1, 1000),
            (2, 2000),
            (3, 4000),
            (4, 8000),
            (5, 16000),
            (30, 16000),
            (u32::MAX, 16000),
        ] {
            assert_eq!(retry_delay_ms(attempt), delay);
        }
    }
    #[test]
    fn delivery_policy_matrix() {
        for creation in [false, true] {
            for exists in [false, true] {
                for live in [false, true] {
                    for connected in [false, true] {
                        for family in [
                            None,
                            Some(FamilyRead::Pending),
                            Some(FamilyRead::Failed),
                            Some(FamilyRead::Settled),
                        ] {
                            let actual = delivery_action(creation, exists, live, connected, family);
                            let expected = if creation {
                                if exists {
                                    DeliveryAction::Remove
                                } else if live && connected {
                                    DeliveryAction::Send
                                } else {
                                    DeliveryAction::Wait
                                }
                            } else if !exists {
                                if live && family.is_none_or(|f| f == FamilyRead::Settled) {
                                    DeliveryAction::Remove
                                } else {
                                    DeliveryAction::Wait
                                }
                            } else if connected {
                                DeliveryAction::Send
                            } else {
                                DeliveryAction::Wait
                            };
                            assert_eq!(actual, expected);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn dispatch_file_limits() {
        let a=DraftAttachment::decode(json!({"type":"file","id":"a","name":"large","mimeType":"x","sizeBytes":2049,"fileUri":"resource"})).unwrap();
        assert_eq!(
            dispatch_step(DeliveryAction::Wait, &[], None),
            DispatchStep::Wait
        );
        assert_eq!(
            dispatch_step(DeliveryAction::Send, &[], None),
            DispatchStep::Retry
        );
        assert_eq!(
            dispatch_step(DeliveryAction::Send, &[], Some(None)),
            DispatchStep::Send
        );
        assert_eq!(
            dispatch_step(DeliveryAction::Send, std::slice::from_ref(&a), Some(None)),
            DispatchStep::Restore("This server does not support file attachments.".into())
        );
        assert_eq!(
            dispatch_step(DeliveryAction::Send, &[a], Some(Some(2048))),
            DispatchStep::Restore("'large' exceeds the 2 KB attachment limit.".into())
        );
    }
    #[test]
    fn durable_before_confirmation_and_tombstone_revision() {
        block_on(async {
            let p = FakePlatform::default();
            let q = Outbox::new(p.clone());
            let m = q.enqueue(message()).await.unwrap();
            assert!(q.confirm_queued(&m).await);
            assert_eq!(load(&p).await.unwrap().messages.len(), 1);
            let revision = q.revision(&m.message_id);
            assert!(
                q.remove(&m, Some(revision), || true)
                    .await
                    .unwrap()
                    .is_some()
            );
            assert!(load(&p).await.unwrap().messages.is_empty());
            q.enqueue(message()).await.unwrap();
            assert!(!q.update(message(), Some(revision)).await.unwrap());
            assert!(q.revision(&m.message_id) > revision);
        });
    }
    #[test]
    fn locks_and_recovery_partial_loads() {
        block_on(async {
            let p = FakePlatform::default();
            let q = Outbox::new(p.clone());
            let m = q.enqueue(message()).await.unwrap();
            assert!(q.remove(&m, None, || false).await.unwrap().is_none());
            p.write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: StorageKey {
                        namespace: NAMESPACE.into(),
                        environment: None,
                        record: "bad".into(),
                    },
                    value: Some(b"bad".to_vec()),
                }],
            )
            .await
            .unwrap();
            let recovered = Outbox::new(p.clone());
            let result = recovered.load().await.unwrap();
            assert_eq!(result.messages.len(), 1);
            assert_eq!(result.errors.len(), 1);
            assert!(!recovered.completely_loaded());
            assert!(
                recovered
                    .clear_environment(&m.environment_id)
                    .await
                    .is_err()
            );
            assert_eq!(load(&p).await.unwrap().messages.len(), 1);
        });
    }
    struct FailStorage;
    impl Storage for FailStorage {
        fn get(
            &self,
            _: StorageArea,
            _: StorageKey,
        ) -> crate::platform::HostFuture<'_, Option<Vec<u8>>> {
            async { Ok(None) }.boxed_local()
        }
        fn list(
            &self,
            _: StorageArea,
            _: String,
        ) -> crate::platform::HostFuture<'_, Vec<StorageKey>> {
            async { Ok(vec![]) }.boxed_local()
        }
        fn write(
            &self,
            _: StorageArea,
            _: Vec<StorageWrite>,
        ) -> crate::platform::HostFuture<'_, ()> {
            async { Err(PlatformError::Storage("full".into())) }.boxed_local()
        }
    }
    #[test]
    fn failed_write_rolls_back_only_its_entry() {
        block_on(async {
            let q = Outbox::new(FailStorage);
            let first = q.enqueue(message());
            let mut newer = message();
            newer.text = "new".into();
            let second = q.enqueue(newer);
            assert!(first.await.is_err());
            assert_eq!(q.messages()[0].text, "new");
            assert!(second.await.is_err());
            assert!(q.messages().is_empty());
        });
    }
    #[test]
    fn unpolled_or_canceled_enqueue_cannot_be_confirmed() {
        block_on(async {
            let q = Outbox::new(FakePlatform::default());
            let write = q.enqueue(message());
            let optimistic = q.messages()[0].clone();
            assert!(!q.confirm_queued(&optimistic).await);
            drop(write);
            assert!(!q.confirm_queued(&optimistic).await);
            assert!(load(&q.storage).await.unwrap().messages.is_empty());
            let queued = q.enqueue(message()).await.unwrap();
            assert!(q.confirm_queued(&queued).await);
        });
    }
    #[derive(Default)]
    struct ControlledStorage {
        inner: FakePlatform,
        fail_delete: std::cell::Cell<bool>,
        gate: RefCell<
            Option<(
                futures::channel::oneshot::Sender<()>,
                futures::channel::oneshot::Receiver<()>,
            )>,
        >,
    }
    impl ControlledStorage {
        fn hold_next_write(
            &self,
        ) -> (
            futures::channel::oneshot::Receiver<()>,
            futures::channel::oneshot::Sender<()>,
        ) {
            let (entered, wait) = futures::channel::oneshot::channel();
            let (release, gate) = futures::channel::oneshot::channel();
            *self.gate.borrow_mut() = Some((entered, gate));
            (wait, release)
        }
    }
    impl Storage for ControlledStorage {
        fn get(
            &self,
            area: StorageArea,
            key: StorageKey,
        ) -> crate::platform::HostFuture<'_, Option<Vec<u8>>> {
            self.inner.get(area, key)
        }
        fn list(
            &self,
            area: StorageArea,
            namespace: String,
        ) -> crate::platform::HostFuture<'_, Vec<StorageKey>> {
            self.inner.list(area, namespace)
        }
        fn write(
            &self,
            area: StorageArea,
            writes: Vec<StorageWrite>,
        ) -> crate::platform::HostFuture<'_, ()> {
            let gate = self.gate.borrow_mut().take();
            async move {
                if let Some((entered, gate)) = gate {
                    let _ = entered.send(());
                    gate.await.map_err(|_| PlatformError::Cancelled)?;
                }
                if writes.iter().any(|w| w.value.is_none()) && self.fail_delete.replace(false) {
                    return Err(PlatformError::Storage("remove failed".into()));
                }
                self.inner.write(area, writes).await
            }
            .boxed_local()
        }
    }
    struct Transport<'a> {
        storage: &'a ControlledStorage,
        sent: std::cell::Cell<usize>,
        generation: std::cell::Cell<u64>,
        change_generation: bool,
        fail_removal: bool,
        restore_ready: bool,
    }
    impl DeliveryTransport for Transport<'_> {
        fn generation(&self, _: &EnvironmentId) -> Generation {
            Generation(self.generation.get())
        }
        fn send<'a>(
            &'a self,
            _: &'a QueuedMessage,
        ) -> LocalBoxFuture<'a, Result<Option<u64>, ClientError>> {
            async move {
                self.sent.set(self.sent.get() + 1);
                if self.fail_removal && self.sent.get() == 1 {
                    self.storage.fail_delete.set(true);
                }
                if self.change_generation {
                    self.generation.set(self.generation.get() + 1);
                }
                Ok(Some(42))
            }
            .boxed_local()
        }
        fn restore<'a>(
            &'a self,
            _: &'a QueuedMessage,
            _: &'a str,
        ) -> LocalBoxFuture<'a, Result<bool, ClientError>> {
            async move { Ok(self.restore_ready) }.boxed_local()
        }
    }
    #[test]
    fn acknowledged_removal_failure_never_sends_twice() {
        block_on(async {
            let storage = ControlledStorage::default();
            let q = Outbox::new(storage);
            let m = q.enqueue(message()).await.unwrap();
            let transport = Transport {
                storage: &q.storage,
                sent: std::cell::Cell::new(0),
                generation: std::cell::Cell::new(1),
                change_generation: false,
                fail_removal: true,
                restore_ready: false,
            };
            let mut delivery = Delivery::default();
            assert!(
                delivery
                    .deliver(&q, &transport, m.clone(), DispatchStep::Send, || true)
                    .await
                    .is_err()
            );
            assert_eq!(q.messages().len(), 1);
            assert_eq!(
                delivery
                    .deliver(&q, &transport, m, DispatchStep::Send, || true)
                    .await
                    .unwrap(),
                DeliveryResult::Accepted(Some(42))
            );
            assert_eq!(transport.sent.get(), 1);
            assert!(q.messages().is_empty());
        });
    }
    #[test]
    fn acknowledgement_cannot_delete_a_concurrent_edit() {
        block_on(async {
            let q = Outbox::new(ControlledStorage::default());
            let m = q.enqueue(message()).await.unwrap();
            let transport = Transport {
                storage: &q.storage,
                sent: std::cell::Cell::new(0),
                generation: std::cell::Cell::new(1),
                change_generation: false,
                fail_removal: true,
                restore_ready: false,
            };
            let mut delivery = Delivery::default();
            assert!(
                delivery
                    .deliver(&q, &transport, m, DispatchStep::Send, || true)
                    .await
                    .is_err()
            );
            let mut newer = message();
            newer.text = "edited".into();
            assert!(q.update(newer, None).await.unwrap());
            let winner = q.messages()[0].clone();
            assert_eq!(
                delivery
                    .deliver(&q, &transport, winner, DispatchStep::Send, || true)
                    .await
                    .unwrap(),
                DeliveryResult::StaleMessage
            );
            assert_eq!(transport.sent.get(), 1);
            assert_eq!(q.messages()[0].text, "edited");
        });
    }
    #[test]
    fn stale_session_and_unpersisted_restore_keep_durable_message() {
        block_on(async {
            let q = Outbox::new(ControlledStorage::default());
            let m = q.enqueue(message()).await.unwrap();
            let transport = Transport {
                storage: &q.storage,
                sent: std::cell::Cell::new(0),
                generation: std::cell::Cell::new(1),
                change_generation: true,
                fail_removal: false,
                restore_ready: false,
            };
            let mut delivery = Delivery::default();
            assert_eq!(
                delivery
                    .deliver(
                        &q,
                        &transport,
                        m.clone(),
                        DispatchStep::Restore("rejected".into()),
                        || true
                    )
                    .await
                    .unwrap(),
                DeliveryResult::Wait
            );
            assert_eq!(load(&q.storage).await.unwrap().messages.len(), 1);
            assert_eq!(
                delivery
                    .deliver(&q, &transport, m.clone(), DispatchStep::Send, || true)
                    .await
                    .unwrap(),
                DeliveryResult::StaleSession
            );
            assert_eq!(load(&q.storage).await.unwrap().messages.len(), 1);
            assert_eq!(
                delivery
                    .deliver(&q, &transport, m, DispatchStep::Send, || true)
                    .await
                    .unwrap(),
                DeliveryResult::Accepted(Some(42))
            );
            assert_eq!(transport.sent.get(), 1);
        });
    }
    #[test]
    fn concurrent_enqueue_wins_over_in_flight_update_on_disk() {
        block_on(async {
            let q = Outbox::new(ControlledStorage::default());
            q.enqueue(message()).await.unwrap();
            let mut edited = message();
            edited.text = "stale edit".into();
            let revision = q.revision(&edited.message_id);
            let (entered, release) = q.storage.hold_next_write();
            let update = q.update(edited, Some(revision));
            let enqueue = async {
                entered.await.unwrap();
                let mut newer = message();
                newer.text = "winner".into();
                let write = q.enqueue(newer);
                release.send(()).unwrap();
                write.await.unwrap();
            };
            let (result, ()) = futures::join!(update, enqueue);
            assert!(!result.unwrap());
            assert_eq!(q.messages()[0].text, "winner");
            assert_eq!(load(&q.storage).await.unwrap().messages[0].text, "winner");
        });
    }
    #[test]
    fn lock_acquired_during_removal_restores_record() {
        block_on(async {
            let q = Outbox::new(ControlledStorage::default());
            let m = q.enqueue(message()).await.unwrap();
            let (entered, release) = q.storage.hold_next_write();
            let removable = std::cell::Cell::new(true);
            let remove = q.remove(&m, None, || removable.get());
            let lock = async {
                entered.await.unwrap();
                removable.set(false);
                release.send(()).unwrap();
            };
            let (result, ()) = futures::join!(remove, lock);
            assert!(result.unwrap().is_none());
            assert_eq!(q.messages().len(), 1);
            assert_eq!(load(&q.storage).await.unwrap().messages.len(), 1);
        });
    }
}
