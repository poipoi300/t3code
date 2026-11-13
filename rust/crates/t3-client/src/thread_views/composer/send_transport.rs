use super::{
    delivery_policy::{ThreadSettings, resolve_settings, should_retry},
    send::{SendService, SendState},
    send_commands,
};
use crate::thread_views::{
    drafts::{DraftFormat, SavedDraftDocument, merge_recovery, storage_key},
    outbox::{DeliveryResult, DeliveryTransport, QueuedMessage},
};
use crate::{
    ClientError,
    operations::{CommandOutcome, CommandStatus, OperationError},
    optimistic::ProjectionKey,
    platform::{
        Platform, PlatformError, StorageArea, StorageCondition, StorageTransaction, StorageWrite,
    },
    rpc::{RpcCallError, RpcEndpoint, RpcMethod},
    types::{CommandId, EnvironmentId, Generation},
};
use futures::{FutureExt, future::LocalBoxFuture, stream::LocalBoxStream};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::BTreeSet, rc::Rc};
use t3_contracts::{
    chat_attachment::{ChatAttachment, PersistChatAttachmentsInput},
    orchestration_v2::*,
    rpc::{WsOrchestrationV2DispatchCommandRpc, WsOrchestrationV2LaunchThreadRpc},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FailureKind {
    Retry,
    Rejected,
    Unknown,
    Stale,
}
#[derive(Debug, Clone)]
pub(super) struct Failure {
    pub kind: FailureKind,
    pub reason: String,
    pub command: Option<CommandId>,
}
impl Failure {
    fn error(error: &ClientError) -> Self {
        Self {
            kind: if matches!(
                error,
                ClientError::Disconnected
                    | ClientError::Cancelled
                    | ClientError::Platform(
                        PlatformError::Network(_)
                            | PlatformError::Closed
                            | PlatformError::Storage(_)
                    )
            ) {
                FailureKind::Retry
            } else {
                FailureKind::Rejected
            },
            reason: error.to_string(),
            command: None,
        }
    }
}
pub(super) fn operation_error(error: OperationError) -> ClientError {
    match error {
        OperationError::Client(error) => error,
        error => ClientError::InvalidPayload(error.to_string()),
    }
}
struct ObservedEndpoint<'a, E> {
    endpoint: &'a E,
    failure: Rc<RefCell<Option<Failure>>>,
    command: CommandId,
}
impl<E: RpcEndpoint> RpcEndpoint for ObservedEndpoint<'_, E> {
    fn destination(&self) -> Option<EnvironmentId> {
        self.endpoint.destination()
    }
    fn call<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxFuture<'_, Result<M::Output, ClientError>> {
        self.endpoint.call::<M>(input)
    }
    fn subscribe<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxStream<'static, Result<M::Output, ClientError>> {
        self.endpoint.subscribe::<M>(input)
    }
    fn command<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxFuture<'_, Result<M::Output, RpcCallError>>
    where
        M::Output: 'static,
    {
        let call = self.endpoint.command::<M>(input);
        async move {
            let result = call.await;
            let failure = match &result {
                Err(RpcCallError::NotSent(error)) => Some(Failure::error(error)),
                Err(RpcCallError::Rejected(errors)) => Some(Failure {
                    kind: if errors.iter().any(should_retry) {
                        FailureKind::Retry
                    } else {
                        FailureKind::Rejected
                    },
                    reason: errors
                        .iter()
                        .find_map(|error| error.get("message").and_then(Value::as_str))
                        .unwrap_or("The server rejected this message.")
                        .into(),
                    command: None,
                }),
                Err(_) => Some(Failure {
                    kind: FailureKind::Unknown,
                    reason:
                        "Waiting for delivery confirmation after the connection was interrupted."
                            .into(),
                    command: None,
                }),
                Ok(_) => None,
            };
            *self.failure.borrow_mut() = failure.map(|mut failure| {
                failure.command = Some(self.command.clone());
                failure
            });
            result
        }
        .boxed_local()
    }
}
pub(super) struct ProductionDelivery<P: Platform, E: RpcEndpoint>(pub SendService<P, E>);
impl<P: Platform + 'static, E: RpcEndpoint + 'static> DeliveryTransport
    for ProductionDelivery<P, E>
{
    fn generation(&self, _: &EnvironmentId) -> Generation {
        self.0
            .inner
            .session
            .borrow()
            .as_ref()
            .map_or(Generation(u64::MAX), |session| session.generation)
    }
    fn send<'a>(
        &'a self,
        message: &'a QueuedMessage,
    ) -> LocalBoxFuture<'a, Result<Option<u64>, ClientError>> {
        self.0.prepare_and_send(message).boxed_local()
    }
    fn restore<'a>(
        &'a self,
        message: &'a QueuedMessage,
        reason: &'a str,
    ) -> LocalBoxFuture<'a, Result<bool, ClientError>> {
        self.0.restore_message(message, reason).boxed_local()
    }
}
impl<P: Platform + 'static, E: RpcEndpoint + 'static> SendService<P, E> {
    pub(super) fn schedule_retry(
        &self,
        id: &t3_contracts::base_schemas::MessageId,
        reason: String,
    ) {
        let attempt = {
            let mut attempts = self.inner.attempts.borrow_mut();
            let attempt = attempts.entry(id.clone()).or_default();
            *attempt = attempt.saturating_add(1);
            *attempt
        };
        self.set_state(
            id,
            SendState::Retry {
                not_before: self
                    .inner
                    .platform
                    .monotonic_ms()
                    .saturating_add(super::super::outbox::retry_delay_ms(attempt)),
                reason,
            },
        );
    }
    pub(super) fn fail(&self, failure: Failure) -> ClientError {
        let reason = failure.reason.clone();
        *self.inner.failure.borrow_mut() = Some(failure);
        ClientError::InvalidPayload(reason)
    }
    pub(super) fn check_current(&self, generation: Generation) -> Result<(), ClientError> {
        if self.current(generation) {
            Ok(())
        } else {
            Err(self.fail(Failure {
                kind: FailureKind::Stale,
                reason: "The destination session changed.".into(),
                command: None,
            }))
        }
    }
    async fn command(
        &self,
        message: &QueuedMessage,
        command: OrchestrationV2Command,
        visible: bool,
    ) -> Result<Option<u64>, ClientError> {
        self.execute::<WsOrchestrationV2DispatchCommandRpc>(message, &command, visible, |reply| {
            Some(reply.sequence)
        })
        .await
    }
    async fn execute<M: t3_contracts::rpc::RpcMethod>(
        &self,
        message: &QueuedMessage,
        command: &M::Payload,
        visible: bool,
        sequence: fn(&M::Success) -> Option<u64>,
    ) -> Result<Option<u64>, ClientError>
    where
        M::Success: 'static,
    {
        let (generation, endpoint) = {
            let session = self.inner.session.borrow();
            let session = session.as_ref().ok_or(ClientError::Disconnected)?;
            (session.generation, session.endpoint.clone())
        };
        let input = serde_json::to_value(command)?;
        let id: CommandId = serde_json::from_value(input["commandId"].clone())?;
        match self.inner.commands.status(&id) {
            Some(CommandStatus::Accepted { sequence }) => return Ok(Some(sequence)),
            Some(CommandStatus::AcceptedAwaitingEntity) => return Ok(None),
            Some(CommandStatus::UnknownDelivery) => {
                return Err(self.fail(Failure {
                    kind: FailureKind::Unknown,
                    reason: "Waiting for delivery confirmation.".into(),
                    command: Some(id),
                }));
            }
            Some(CommandStatus::Rejected(reason)) if reason == "RPC request was not sent" => {
                self.inner
                    .commands
                    .remove_unsent(self.inner.platform.as_ref(), &id)
                    .await
                    .map_err(operation_error)?;
            }
            Some(CommandStatus::Rejected(reason)) => {
                return Err(self.fail(Failure {
                    kind: FailureKind::Rejected,
                    reason,
                    command: Some(id),
                }));
            }
            _ => {}
        }
        self.check_current(generation)?;
        if self.inner.commands.status(&id).is_none() {
            let projection =
                ProjectionKey::thread(message.environment_id.clone(), message.thread_id.clone());
            let epoch = self.projection_epoch(&message.thread_key());
            self.inner
                .commands
                .enqueue::<M>(
                    self.inner.platform.as_ref(),
                    command,
                    projection.clone(),
                    epoch,
                    send_commands::overlay(message, visible),
                )
                .await
                .map_err(operation_error)?;
            self.inner
                .epochs
                .borrow_mut()
                .entry(projection)
                .or_insert((generation, epoch));
        }
        self.check_current(generation)?;
        if visible {
            self.set_state(&message.message_id, SendState::Dispatching);
        }
        let failure = Rc::new(RefCell::new(None));
        let observed = ObservedEndpoint {
            endpoint: endpoint.as_ref(),
            failure: failure.clone(),
            command: id,
        };
        let report = self
            .inner
            .commands
            .deliver::<M>(
                self.inner.platform.as_ref(),
                &observed,
                &serde_json::from_value(input["commandId"].clone())?,
                sequence,
            )
            .await
            .map_err(operation_error)?;
        match report.outcome {
            CommandOutcome::Accepted { receipt, .. } => {
                Ok(receipt.map(|receipt| receipt.committed_sequence))
            }
            _ => {
                let failure = failure.borrow_mut().take().unwrap_or(Failure {
                    kind: FailureKind::Unknown,
                    reason: "Waiting for delivery confirmation.".into(),
                    command: None,
                });
                Err(self.fail(failure))
            }
        }
    }
    pub(super) async fn prepare_and_send(
        &self,
        message: &QueuedMessage,
    ) -> Result<Option<u64>, ClientError> {
        self.inner.failure.borrow_mut().take();
        let (generation, endpoint, config) = {
            let session = self.inner.session.borrow();
            let session = session.as_ref().ok_or(ClientError::Disconnected)?;
            (
                session.generation,
                session.endpoint.clone(),
                session.config.clone(),
            )
        };
        let existing = self
            .inner
            .commands
            .records()
            .into_iter()
            .find(|record| record.command == message.command_id);
        if let Some(record) = existing {
            if record.method
                == <WsOrchestrationV2LaunchThreadRpc as t3_contracts::rpc::RpcMethod>::TAG
            {
                return self
                    .execute::<WsOrchestrationV2LaunchThreadRpc>(
                        message,
                        &serde_json::from_value(record.input)?,
                        true,
                        |reply| reply.sequence,
                    )
                    .await;
            }
            return self
                .command(message, serde_json::from_value(record.input)?, true)
                .await;
        }
        self.set_state(&message.message_id, SendState::Preparing);
        let fallback = self
            .thread_settings(&message.thread_key())
            .or_else(|| {
                message
                    .model_selection
                    .clone()
                    .map(|model_selection| ThreadSettings {
                        model_selection,
                        runtime_mode: message
                            .runtime_mode
                            .unwrap_or(t3_contracts::provider_policy::RuntimeMode::FullAccess),
                        interaction_mode: message.interaction_mode.unwrap_or(
                            t3_contracts::provider_policy::ProviderInteractionMode::Default,
                        ),
                    })
            })
            .ok_or_else(|| {
                ClientError::InvalidPayload("Thread settings are unavailable.".into())
            })?;
        let settings = resolve_settings(message, &fallback, &config.providers.0);
        let snapshot = self.inner.store.snapshot();
        let project = message
            .creation
            .as_ref()
            .map(|creation| &creation.project_id)
            .or_else(|| {
                snapshot
                    .environments
                    .get(&message.environment_id)?
                    .shell
                    .snapshot
                    .as_ref()?
                    .threads
                    .iter()
                    .find(|thread| thread.id == message.thread_id)
                    .map(|thread| &thread.project_id)
            });
        if let Some(reason) = super::send_policy::block_reason(&config, project, message, &settings)
        {
            return Err(ClientError::InvalidPayload(reason));
        }
        let creation = if self.thread_settings(&message.thread_key()).is_none() {
            let creation = message.creation.as_ref().ok_or_else(|| {
                ClientError::InvalidPayload("The destination thread was removed.".into())
            })?;
            Some(creation.clone())
        } else {
            for (setting, changed, value) in [
                (
                    "runtime-mode",
                    settings.runtime_mode != fallback.runtime_mode,
                    json!(settings.runtime_mode),
                ),
                (
                    "interaction-mode",
                    settings.interaction_mode != fallback.interaction_mode,
                    json!(settings.interaction_mode),
                ),
            ] {
                if changed && !message.extra.contains_key("queuedRunId") {
                    let command: OrchestrationV2Command = serde_json::from_value(
                        json!({"type":format!("thread.{setting}.set"),"commandId":format!("{}:{setting}",message.command_id.0),"threadId":message.thread_id,if setting=="runtime-mode"{"runtimeMode"}else{"interactionMode"}:value}),
                    )?;
                    if let Err(error) = self.command(message, command, false).await {
                        if let Some(failure) = self.inner.failure.borrow_mut().as_mut()
                            && failure.kind == FailureKind::Rejected
                        {
                            failure.kind = FailureKind::Retry;
                        }
                        return Err(error);
                    }
                    self.check_current(generation)?;
                }
            }
            None
        };
        let cached = self
            .inner
            .prepared
            .borrow()
            .get(&message.message_id)
            .filter(|(original, _)| original == &message.attachments)
            .map(|(_, attachments)| attachments.clone())
            .or_else(|| {
                let group = message.extra.get("fanoutGroup")?.as_str()?;
                self.inner
                    .fanout_prepared
                    .borrow()
                    .get(group)
                    .filter(|(original, _)| original == &message.attachments)
                    .map(|(_, attachments)| attachments.clone())
            });
        let attachments = if let Some(cached) = cached {
            cached
        } else {
            let mut uploads = vec![];
            let mut attachments = vec![];
            for attachment in &message.attachments {
                let value = attachment.value();
                if value["uploadEnvironmentId"] == message.environment_id.0
                    && value["uploadedAttachmentId"].is_string()
                {
                    match crate::services::assets::verify_persisted_attachment(
                        endpoint.as_ref(),
                        value["uploadedAttachmentId"]
                            .as_str()
                            .unwrap_or_default()
                            .into(),
                    )
                    .await
                    {
                        crate::services::assets::AttachmentVerification::Verified => {}
                        crate::services::assets::AttachmentVerification::Missing => {
                            let base = self
                                .inner
                                .session
                                .borrow()
                                .as_ref()
                                .and_then(|session| session.upload_base.clone());
                            if let Some(base) = base {
                                attachments.push(Some(
                                    self.upload_attachment(
                                        message,
                                        attachment,
                                        generation,
                                        endpoint.as_ref(),
                                        &base,
                                    )
                                    .await?,
                                ));
                            } else {
                                uploads.push(self.legacy_upload(attachment, generation).await?);
                                attachments.push(None);
                            }
                            continue;
                        }
                        crate::services::assets::AttachmentVerification::Failed(error) => {
                            return Err(error);
                        }
                    }
                    self.check_current(generation)?;
                    attachments.push(Some(serde_json::from_value::<ChatAttachment>(json!({"type":value["type"],"id":value["uploadedAttachmentId"],"name":value["name"],"mimeType":value["mimeType"],"sizeBytes":value["sizeBytes"]}))?));
                    continue;
                }
                let base = self
                    .inner
                    .session
                    .borrow()
                    .as_ref()
                    .and_then(|session| session.upload_base.clone());
                if let Some(base) = base {
                    attachments.push(Some(
                        self.upload_attachment(
                            message,
                            attachment,
                            generation,
                            endpoint.as_ref(),
                            &base,
                        )
                        .await?,
                    ));
                    continue;
                }
                uploads.push(self.legacy_upload(attachment, generation).await?);
                attachments.push(None);
            }
            if !uploads.is_empty() {
                let persisted = endpoint
                    .call::<t3_contracts::rpc::WsAssetsPersistChatAttachmentsRpc>(
                        PersistChatAttachmentsInput {
                            thread_id: message.thread_id.clone(),
                            message_id: message.message_id.clone(),
                            attachments: uploads,
                        },
                    )
                    .await?;
                self.check_current(generation)?;
                let mut persisted = persisted.attachments.into_iter();
                for attachment in &mut attachments {
                    if attachment.is_none() {
                        *attachment = persisted.next();
                    }
                }
                if persisted.next().is_some() || attachments.iter().any(Option::is_none) {
                    return Err(ClientError::InvalidPayload(
                        "The upload reply did not match the attachment count.".into(),
                    ));
                }
            }
            let attachments = attachments.into_iter().flatten().collect::<Vec<_>>();
            self.inner.prepared.borrow_mut().insert(
                message.message_id.clone(),
                (message.attachments.clone(), attachments.clone()),
            );
            attachments
        };
        if let Some(group) = message.extra.get("fanoutGroup").and_then(Value::as_str) {
            self.inner
                .fanout_prepared
                .borrow_mut()
                .entry(group.to_owned())
                .or_insert_with(|| (message.attachments.clone(), attachments.clone()));
        }
        if self.inner.editing.borrow().contains(&message.message_id) {
            return Err(self.fail(Failure {
                kind: FailureKind::Stale,
                reason: "The queued message is being edited.".into(),
                command: None,
            }));
        }
        let current_config = self
            .inner
            .session
            .borrow()
            .as_ref()
            .map(|session| session.config.clone())
            .ok_or(ClientError::Disconnected)?;
        let settings = resolve_settings(message, &settings, &current_config.providers.0);
        if let Some(reason) =
            super::send_policy::block_reason(&current_config, project, message, &settings)
        {
            return Err(ClientError::InvalidPayload(reason));
        }
        let modern = self
            .inner
            .session
            .borrow()
            .as_ref()
            .is_some_and(|session| session.upload_base.is_some());
        if modern
            && self
                .persist_upload_references(message, &attachments)
                .await?
        {
            return Err(self.fail(Failure {
                kind: FailureKind::Stale,
                reason: "Uploaded references were persisted; continue with the durable message."
                    .into(),
                command: None,
            }));
        }
        if let Some(mut creation) = creation {
            if let Some(reference) = &creation.pull_request_reference {
                let cwd = creation
                    .project_cwd
                    .as_ref()
                    .or_else(|| {
                        snapshot
                            .environments
                            .get(&message.environment_id)?
                            .shell
                            .snapshot
                            .as_ref()?
                            .projects
                            .iter()
                            .find(|project| project.id == creation.project_id)
                            .map(|project| &project.workspace_root.0)
                    })
                    .ok_or_else(|| {
                        ClientError::InvalidPayload("The project workspace is unavailable.".into())
                    })?;
                let prepared=endpoint.call::<t3_contracts::rpc::WsGitPreparePullRequestThreadRpc>(serde_json::from_value(json!({"cwd":cwd,"reference":reference,"mode":"worktree","threadId":message.thread_id}))?).await?;
                self.check_current(generation)?;
                if !prepared.is_on_pull_request_head || prepared.worktree_path.is_none() {
                    return Err(ClientError::InvalidPayload("The pull request worktree has local changes and could not update to its head. Resolve them before trying again.".into()));
                }
                creation.workspace_mode = crate::thread_views::outbox::WorkspaceMode::Local;
                creation.branch = Some(prepared.branch.0);
                creation.worktree_path = prepared.worktree_path.map(|path| path.0);
                creation.start_from_origin = Some(false);
            }
            let title =
                crate::operations::derive_thread_title_seed(&message.text, &attachments, &[]);
            let branch = creation
                .branch
                .map(t3_contracts::base_schemas::TrimmedNonEmptyString);
            let prepare = if creation.workspace_mode
                == crate::thread_views::outbox::WorkspaceMode::Worktree
            {
                let base_ref = branch.clone().ok_or_else(|| {
                    ClientError::InvalidPayload("Choose a base branch before sending.".into())
                })?;
                Some(ThreadLaunchWorkspaceStrategyWorktree {
                    base_ref,
                    branch: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                        format!("t3/{}", message.command_id.0),
                    ))),
                    start_from_origin: creation.start_from_origin.map(Some),
                })
            } else {
                None
            };
            let strategy = crate::operations::launch_workspace_strategy(
                branch,
                creation
                    .worktree_path
                    .map(t3_contracts::base_schemas::TrimmedNonEmptyString),
                prepare,
            );
            let context = send_commands::remapped_context(message, &attachments);
            let mut initial = json!({"messageId":message.message_id,"text":message.text.trim(),"attachments":attachments});
            if let Some(context) = context {
                initial["context"] = json!(context);
            }
            let input: OrchestrationV2ThreadLaunchInput = serde_json::from_value(
                json!({"commandId":message.command_id,"threadId":message.thread_id,"projectId":creation.project_id,"title":title,"generateTitle":true,"modelSelection":settings.model_selection,"runtimeMode":settings.runtime_mode,"interactionMode":settings.interaction_mode,"workspaceStrategy":strategy,"creationSource":self.inner.source,"initialMessage":initial}),
            )?;
            return self
                .execute::<WsOrchestrationV2LaunchThreadRpc>(message, &input, true, |reply| {
                    reply.sequence
                })
                .await;
        }
        if message.compact_before_send == Some(true) {
            self.command(
                message,
                send_commands::message(
                    message,
                    &settings,
                    vec![],
                    self.inner.source.clone(),
                    true,
                )?,
                false,
            )
            .await?;
            self.check_current(generation)?;
        }
        self.command(
            message,
            send_commands::message(
                message,
                &settings,
                attachments,
                self.inner.source.clone(),
                false,
            )?,
            true,
        )
        .await
    }
    pub(super) async fn handle_failure(
        &self,
        message: &QueuedMessage,
        error: ClientError,
        generation: Generation,
    ) -> Result<DeliveryResult, ClientError> {
        let failure = self
            .inner
            .failure
            .borrow_mut()
            .take()
            .unwrap_or_else(|| Failure::error(&error));
        match failure.kind {
            FailureKind::Unknown => {
                self.set_state(&message.message_id, SendState::AwaitingConfirmation);
                Ok(DeliveryResult::Wait)
            }
            FailureKind::Stale => Ok(DeliveryResult::StaleSession),
            FailureKind::Retry => {
                if let Some(command) = failure.command
                    && matches!(
                        self.inner.commands.status(&command),
                        Some(CommandStatus::Rejected(_))
                    )
                {
                    self.inner
                        .commands
                        .remove_unsent(self.inner.platform.as_ref(), &command)
                        .await
                        .map_err(operation_error)?;
                }
                self.schedule_retry(&message.message_id, failure.reason);
                Ok(DeliveryResult::Retry)
            }
            FailureKind::Rejected => {
                if !self.current(generation) {
                    return Ok(DeliveryResult::StaleSession);
                }
                if self.inner.editing.borrow().contains(&message.message_id) {
                    return Ok(DeliveryResult::Wait);
                }
                let revision = self.inner.outbox.revision(&message.message_id);
                if self.restore_message(message, &failure.reason).await? {
                    let removed = self
                        .inner
                        .outbox
                        .remove(message, Some(revision), || {
                            self.current(generation)
                                && !self.inner.editing.borrow().contains(&message.message_id)
                        })
                        .await?;
                    Ok(if removed.is_some() {
                        DeliveryResult::Restored
                    } else {
                        DeliveryResult::Wait
                    })
                } else {
                    Ok(DeliveryResult::Wait)
                }
            }
        }
    }
    async fn restore_message(
        &self,
        message: &QueuedMessage,
        reason: &str,
    ) -> Result<bool, ClientError> {
        let drafts = self.inner.drafts.borrow().clone();
        let draft_key = if let Some(drafts) = drafts {
            drafts.load().await?;
            let mut key = String::new();
            drafts.update(|current| {
                let mut document = current.clone();
                let (draft_key, _) = recover_document(&mut document, message)?;
                key = draft_key;
                Ok(document)
            })?;
            drafts.flush_all().await?;
            key
        } else {
            let key = storage_key(DraftFormat::Mobile);
            let expected = self
                .inner
                .platform
                .get(StorageArea::Ordinary, key.clone())
                .await?;
            let mut document = expected
                .as_deref()
                .map(SavedDraftDocument::decode_mobile)
                .transpose()?
                .map_or_else(
                    || json!({"schemaVersion":1,"drafts":{}}),
                    |document| document.raw().clone(),
                );
            let (draft_key, changed) = recover_document(&mut document, message)?;
            if changed {
                let record = crate::persistence::Record::new(
                    "composer-drafts.mobile",
                    None,
                    "current",
                    1,
                    document,
                );
                let committed = self
                    .inner
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
                    .await?;
                if !committed {
                    return Err(ClientError::InvalidPayload(
                        "The draft changed during message recovery; retry recovery.".into(),
                    ));
                }
            }
            draft_key
        };
        self.set_state(
            &message.message_id,
            SendState::Restored {
                draft_key,
                reason: reason.into(),
            },
        );
        Ok(true)
    }
    pub(super) async fn reconcile(&self) -> Result<(), ClientError> {
        let Some(generation) = self
            .inner
            .session
            .borrow()
            .as_ref()
            .map(|session| session.generation)
        else {
            return Ok(());
        };
        let snapshot = self.inner.store.snapshot();
        let Some(environment) = snapshot.environments.get(&self.inner.environment) else {
            return Ok(());
        };
        let mut reset = BTreeSet::new();
        let records = self.inner.commands.records();
        for record in &records {
            let epoch = record
                .projection
                .thread
                .as_ref()
                .and_then(|id| environment.threads.get(id))
                .map_or(0, |thread| thread.generation);
            if reset.insert(record.projection.clone())
                && self.inner.epochs.borrow().get(&record.projection) != Some(&(generation, epoch))
            {
                self.inner.commands.reset(&record.projection, epoch, 0);
                self.inner
                    .epochs
                    .borrow_mut()
                    .insert(record.projection.clone(), (generation, epoch));
                for same in records
                    .iter()
                    .filter(|same| same.projection == record.projection)
                {
                    self.inner
                        .resolved_entities
                        .borrow_mut()
                        .remove(&same.command);
                }
            }
        }
        for record in records {
            let thread_id = record.projection.thread.as_ref();
            let projection = thread_id
                .and_then(|id| environment.threads.get(id))
                .filter(|thread| {
                    thread.session == generation && thread.status == crate::types::SyncStatus::Live
                })
                .and_then(|thread| thread.projection.as_ref());
            let shell = thread_id
                .filter(|_| {
                    environment.shell.session == generation
                        && environment.shell.status == crate::types::SyncStatus::Live
                })
                .and_then(|id| {
                    environment
                        .shell
                        .snapshot
                        .as_ref()?
                        .threads
                        .iter()
                        .find(|thread| &thread.id == id)
                });
            let evidence = if record.method
                == <WsOrchestrationV2LaunchThreadRpc as t3_contracts::rpc::RpcMethod>::TAG
            {
                record.change.apply.0.as_ref().is_some_and(|message|projection.is_some_and(|projection|projection.messages.iter().any(|row|matches!(&row.id,t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(id) if id==&message.message_id))))
            } else {
                let command: OrchestrationV2Command = serde_json::from_value(record.input)?;
                match &command {
                OrchestrationV2Command::MessageDispatch(command)=>projection.is_some_and(|projection|projection.messages.iter().any(|message|matches!(&message.id,t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(id) if id==&command.message_id))),
                OrchestrationV2Command::QueuedRunEdit(command)=>projection.is_some_and(|projection|projection.messages.iter().any(|message|matches!(&message.run_id,Some(t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(id)) if id==&command.run_id) && message.text==command.text && command.attachments.as_ref().is_none_or(|attachments|message.attachments==attachments.clone().unwrap_or_default()) && command.context.as_ref().is_none_or(|context|message.context.as_ref().and_then(Option::as_ref)==context.as_ref()))),
                OrchestrationV2Command::ThreadCreate(_)=>shell.is_some(),
                OrchestrationV2Command::ThreadRuntimeModeSet(command)=>shell.is_some_and(|shell|shell.runtime_mode==command.runtime_mode),
                OrchestrationV2Command::ThreadInteractionModeSet(command)=>shell.is_some_and(|shell|shell.interaction_mode==command.interaction_mode),
                _=>false,
            }
            };
            if evidence
                && matches!(
                    self.inner.commands.status(&record.command),
                    Some(CommandStatus::UnknownDelivery | CommandStatus::AcceptedAwaitingEntity)
                )
                && !self
                    .inner
                    .resolved_entities
                    .borrow()
                    .contains(&record.command)
            {
                let epoch = record
                    .projection
                    .thread
                    .as_ref()
                    .and_then(|id| environment.threads.get(id))
                    .map_or(0, |thread| thread.generation);
                self.inner
                    .commands
                    .resolve_entity(
                        self.inner.platform.as_ref(),
                        &record.command,
                        &record.projection,
                        epoch,
                    )
                    .await
                    .map_err(operation_error)?;
                self.inner
                    .resolved_entities
                    .borrow_mut()
                    .insert(record.command.clone());
                if let Some(message) = record.change.apply.0 {
                    if self
                        .inner
                        .outbox
                        .messages()
                        .iter()
                        .any(|queued| queued.message_id == message.message_id)
                    {
                        self.set_state(&message.message_id, SendState::Queued);
                    }
                } else {
                    for message in self.inner.outbox.messages() {
                        if self.state(&message.message_id) == Some(SendState::AwaitingConfirmation)
                        {
                            self.set_state(&message.message_id, SendState::Queued);
                        }
                    }
                }
            }
        }
        for (id, thread) in &environment.threads {
            if thread.session == generation
                && thread.status == crate::types::SyncStatus::Live
                && thread.projection.is_some()
                && !thread.resync_required
            {
                self.inner.commands.observe(
                    &ProjectionKey::thread(self.inner.environment.clone(), id.clone()),
                    thread.generation,
                    thread.sequence,
                );
            }
        }
        if !self
            .inner
            .outbox
            .messages()
            .iter()
            .any(|message| message.environment_id == self.inner.environment)
        {
            let records = self.inner.commands.records();
            if records.iter().any(|record| match record.status {
                CommandStatus::Accepted { sequence } => record
                    .projection
                    .thread
                    .as_ref()
                    .and_then(|id| environment.threads.get(id))
                    .is_some_and(|thread| {
                        thread.session == generation
                            && thread.generation == record.generation
                            && thread.sequence >= sequence
                    }),
                CommandStatus::AcceptedAwaitingEntity => self
                    .inner
                    .resolved_entities
                    .borrow()
                    .contains(&record.command),
                _ => false,
            }) {
                self.inner
                    .commands
                    .prune_confirmed(self.inner.platform.as_ref())
                    .await
                    .map_err(operation_error)?;
                let remaining: BTreeSet<_> = self
                    .inner
                    .commands
                    .records()
                    .into_iter()
                    .map(|record| record.command)
                    .collect();
                self.inner
                    .resolved_entities
                    .borrow_mut()
                    .retain(|command| remaining.contains(command));
            }
        }
        Ok(())
    }
}

fn recover_document(
    document: &mut Value,
    message: &QueuedMessage,
) -> Result<(String, bool), ClientError> {
    let draft_key = if message.creation.is_some() {
        format!("new-task:restored-{}", message.message_id.0)
    } else {
        format!("{}:{}", message.environment_id.0, message.thread_id.0)
    };
    if document
        .get("recoveredMessages")
        .is_some_and(|receipts| !receipts.is_object())
    {
        return Err(ClientError::InvalidPayload(
            "Unreadable draft recovery receipts were retained.".into(),
        ));
    }
    let receipt = serde_json::to_value(&message.command_id)?;
    let changed = document["recoveredMessages"].get(&message.message_id.0) != Some(&receipt);
    if changed {
        let mut incoming = json!({"text":message.text,"attachments":message.attachments});
        let optional = [
            ("context", serde_json::to_value(&message.context)?),
            (
                "modelSelection",
                serde_json::to_value(&message.model_selection)?,
            ),
            ("runtimeMode", serde_json::to_value(message.runtime_mode)?),
            (
                "interactionMode",
                serde_json::to_value(message.interaction_mode)?,
            ),
            ("freshContext", serde_json::to_value(message.fresh_context)?),
        ];
        for (key, value) in optional {
            if !value.is_null() {
                incoming[key] = value;
            }
        }
        if let Some(creation) = &message.creation {
            incoming["project"] = json!({"environmentId":message.environment_id,"projectId":creation.project_id,"createdAt":message.created_at});
            incoming["workspaceSelection"] = json!({"mode":creation.workspace_mode,"branch":creation.branch,"worktreePath":creation.worktree_path,"startFromOrigin":creation.start_from_origin,"pullRequestReference":creation.pull_request_reference});
        }
        document["drafts"][&draft_key] = if let Some(existing) = document["drafts"].get(&draft_key)
        {
            merge_recovery(existing, &incoming)?
        } else {
            incoming
        };
        document["recoveredMessages"][&message.message_id.0] = receipt;
    }
    Ok((draft_key, changed))
}
