use super::ThreadOperations;
use crate::{
    ClientError,
    platform::Platform,
    rpc::RpcEndpoint,
    types::{SyncStatus, ThreadKey},
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_contracts::{
    base_schemas::{CheckpointId, CheckpointScopeId, MessageId, RunId},
    orchestration_v2::*,
    orchestration_v2_id_table::IdOrIndex,
    rpc::WsOrchestrationV2DispatchCommandRpc,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RewindTarget {
    pub message_id: MessageId,
    pub turn_count: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RewindState {
    pub reverting: bool,
    pub can_fork: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RewindAvailability {
    pub can_rewind: bool,
    pub can_restore_files: bool,
    pub reason: Option<String>,
}

/// Prepared bytes and a reservation from the canonical composer attachment session.
/// Commit appends every attachment, including duplicates, while retaining current content.
pub trait PreparedRewindAttachments {
    fn commit(&self, document: &mut serde_json::Value, draft_key: &str) -> Result<(), ClientError>;
}
pub trait RewindAttachments {
    fn ready(&self, key: &ThreadKey) -> bool;
    fn prepare(
        &self,
        key: ThreadKey,
        attachments: Vec<t3_contracts::chat_attachment::ChatAttachment>,
    ) -> LocalBoxFuture<'static, Result<Box<dyn PreparedRewindAttachments>, ClientError>>;
}
#[derive(Clone, Debug, PartialEq)]
pub struct RewindHead {
    run: Option<(IdOrIndex<RunId>, u64, OrchestrationV2RunStatus)>,
    message: Option<(IdOrIndex<MessageId>, t3_contracts::wire::WireDateTime)>,
    updated_at: t3_contracts::wire::WireDateTime,
    provider: Option<IdOrIndex<t3_contracts::base_schemas::ProviderThreadId>>,
}
impl RewindHead {
    pub fn capture(p: &OrchestrationV2ThreadProjection) -> Self {
        Self {
            run: p
                .runs
                .iter()
                .max_by_key(|r| r.ordinal)
                .map(|r| (r.id.clone(), r.ordinal, r.status.clone())),
            message: p
                .messages
                .last()
                .map(|m| (m.id.clone(), m.updated_at.clone())),
            updated_at: p.thread.updated_at.clone(),
            provider: p.thread.active_provider_thread_id.clone(),
        }
    }
    pub fn ensure_current(
        &self,
        projection: &OrchestrationV2ThreadProjection,
    ) -> Result<(), ClientError> {
        if self != &Self::capture(projection) {
            return Err(ClientError::InvalidPayload(
                "The thread has changed. Select a message to rewind again.".into(),
            ));
        }
        Ok(())
    }
}
#[derive(Clone)]
struct Confirmation {
    head: RewindHead,
    checkpoint: CheckpointId,
    scope: CheckpointScopeId,
    message: OrchestrationV2ConversationMessage,
    ready: bool,
}
impl Confirmation {
    fn check(&self, p: &OrchestrationV2ThreadProjection, files: bool) -> Result<(), ClientError> {
        self.head.ensure_current(p)?;
        if files && (p.thread.worktree_path.is_none() || !self.ready) {
            return Err(ClientError::Unsupported(
                "file restoration requires a ready checkpoint in a worktree",
            ));
        }
        if p.checkpoints
            .iter()
            .find(|c| c.id == IdOrIndex::Id(self.checkpoint.clone()))
            .is_some_and(|c| {
                c.scope_id != IdOrIndex::Id(self.scope.clone())
                    || (files && c.status != CheckpointStatus::Ready)
            })
        {
            return Err(ClientError::Unsupported(
                "the confirmed checkpoint is no longer available",
            ));
        }
        Ok(())
    }
}
#[derive(Default)]
pub struct RewindStateStore {
    confirmations: RefCell<BTreeMap<(ThreadKey, String, u64), Confirmation>>,
    reverting: RefCell<BTreeSet<ThreadKey>>,
    attachments: RefCell<Option<Rc<dyn RewindAttachments>>>,
    failures: RefCell<BTreeMap<ThreadKey, String>>,
}
impl RewindStateStore {
    pub(super) fn forget_environment(&self, environment: &crate::types::EnvironmentId) {
        self.confirmations
            .borrow_mut()
            .retain(|(key, _, _), _| &key.environment != environment);
        self.failures
            .borrow_mut()
            .retain(|key, _| &key.environment != environment);
    }
}
struct Reverting<P: Platform + Clone + 'static>(ThreadOperations<P>, ThreadKey);
impl<P: Platform + Clone + 'static> Drop for Reverting<P> {
    fn drop(&mut self) {
        self.0.state.rewind.reverting.borrow_mut().remove(&self.1);
        self.0.state.actions.notify();
    }
}

impl<P: Platform + Clone + 'static> ThreadOperations<P> {
    pub fn bind_rewind_attachments(&self, session: Rc<dyn RewindAttachments>) {
        *self.state.rewind.attachments.borrow_mut() = Some(session);
        self.state.actions.notify();
    }
    pub fn rewind_failure(&self, key: &ThreadKey) -> Option<String> {
        self.state.rewind.failures.borrow().get(key).cloned()
    }
    fn projection(
        &self,
        key: &ThreadKey,
    ) -> Result<Rc<OrchestrationV2ThreadProjection>, ClientError> {
        self.runtime
            .store()
            .snapshot()
            .environments
            .get(&key.environment)
            .and_then(|e| e.threads.get(&key.thread))
            .filter(|t| t.status == SyncStatus::Live)
            .and_then(|t| t.projection.clone())
            .ok_or(ClientError::Disconnected)
    }
    pub fn rewind_state(&self, key: &ThreadKey) -> RewindState {
        RewindState {
            reverting: self.state.rewind.reverting.borrow().contains(key),
            can_fork: self.can_operate(key)
                && !self.state.rewind.reverting.borrow().contains(key)
                && self.projection(key).is_ok_and(|p| {
                    crate::thread_views::threads::workflows::resolve_thread_provider_session(&p)
                        .is_some_and(|s| {
                            (s.capabilities.threads.can_fork_thread
                                && s.capabilities.threads.can_fork_from_turn
                                && s.capabilities.identity.native_thread_ids
                                    == OrchestrationV2NativeRefStrength::Strong)
                                || s.capabilities.context.supports_full_thread_handoff
                        })
                }),
        }
    }
    fn check_rewind_context(
        &self,
        key: &ThreadKey,
    ) -> Result<Rc<OrchestrationV2ThreadProjection>, ClientError> {
        self.endpoint(&key.environment)?;
        if !self.can_operate(key) {
            return Err(ClientError::PermissionDenied {
                method: "orchestration.dispatchCommand".into(),
            });
        }
        if self.rewind_state(key).reverting {
            return Err(Self::refused("The thread is already rewinding."));
        }
        let p = self.projection(key)?;
        let provider = crate::thread_views::threads::workflows::resolve_thread_provider_session(&p)
            .ok_or(ClientError::Unsupported("conversation rollback"))?;
        if !provider
            .capabilities
            .checkpointing
            .provider_can_rollback_conversation
        {
            return Err(Self::refused(
                "This provider does not support reverting conversation history. Start a new thread instead.",
            ));
        }
        let actions = self.runtime.composer_primary_actions(key).snapshot();
        if actions.is_running || actions.is_send_busy {
            return Err(Self::refused(
                "Interrupt the current turn before reverting checkpoints.",
            ));
        }
        Ok(p)
    }
    fn check_rewind(
        &self,
        key: &ThreadKey,
        target: &RewindTarget,
    ) -> Result<
        (
            Rc<OrchestrationV2ThreadProjection>,
            OrchestrationV2Checkpoint,
        ),
        ClientError,
    > {
        let p = self.check_rewind_context(key)?;
        if !p.messages.iter().any(|m| {
            m.id == IdOrIndex::Id(target.message_id.clone())
                && m.role == ConversationMessageRole::User
        }) {
            return Err(Self::refused(
                "The thread has changed. Select a message to rewind again.",
            ));
        }
        let entries = crate::view_logic::session::derive_timeline_entries_from_visible_turn_items(
            &crate::view_logic::session::TimelineEntriesInput {
                visible_turn_items: p.visible_turn_items.iter().cloned().map(Rc::new).collect(),
                ..Default::default()
            },
        );
        let summaries = p
            .checkpoints
            .iter()
            .filter_map(|c| {
                Some(crate::view_logic::session::CheckpointSummary {
                    run_id: c.run_id.clone()?,
                    status: c.status.clone(),
                    checkpoint_turn_count: c.app_run_ordinal?,
                })
            })
            .collect::<Vec<_>>();
        let available = crate::view_logic::chat_view::derive_available_revert_turn_counts(Some(&p));
        let counts = crate::view_logic::session::derive_revert_turn_count_by_user_message_id(
            &entries,
            &summaries,
            available.contains(&0),
            Some(&available),
            None,
        );
        if counts.get(&crate::view_logic::session::id_key(&IdOrIndex::Id(
            target.message_id.clone(),
        ))) != Some(&target.turn_count)
        {
            return Err(Self::refused(
                "The thread has changed. Select a message to rewind again.",
            ));
        }
        let checkpoint =
            crate::view_logic::session::find_revert_checkpoint(&p.checkpoints, target.turn_count)
                .cloned()
                .ok_or_else(|| {
                    Self::refused("The thread has changed. Select a message to rewind again.")
                })?;
        Ok((p, checkpoint))
    }
    pub fn rewind_availability(
        &self,
        key: &ThreadKey,
        target: &RewindTarget,
    ) -> RewindAvailability {
        let identity = (key.clone(), target.message_id.0.clone(), target.turn_count);
        if let Some(confirmation) = self.state.rewind.confirmations.borrow().get(&identity) {
            return match self.check_rewind_context(key).and_then(|p| {
                confirmation.check(&p, false)?;
                Ok(confirmation.check(&p, true).is_ok())
            }) {
                Ok(files) => RewindAvailability {
                    can_rewind: true,
                    can_restore_files: files,
                    reason: None,
                },
                Err(error) => RewindAvailability {
                    reason: Some(rewind_reason(error)),
                    ..Default::default()
                },
            };
        }
        let result = self.check_rewind(key, target).and_then(|(p, checkpoint)| {
            let (IdOrIndex::Id(id), IdOrIndex::Id(scope)) = (&checkpoint.id, &checkpoint.scope_id)
            else {
                return Err(ClientError::Unsupported("unresolved checkpoint identity"));
            };
            if !self
                .state
                .rewind
                .attachments
                .borrow()
                .as_ref()
                .is_some_and(|a| a.ready(key))
            {
                return Err(ClientError::Unsupported("composer attachment preflight"));
            }
            let mut confirmations = self.state.rewind.confirmations.borrow_mut();
            confirmations.retain(|(thread, _, _), _| thread != key);
            confirmations.insert(
                identity,
                Confirmation {
                    head: RewindHead::capture(&p),
                    checkpoint: id.clone(),
                    scope: scope.clone(),
                    message: p
                        .messages
                        .iter()
                        .find(|m| m.id == IdOrIndex::Id(target.message_id.clone()))
                        .expect("validated rewind message")
                        .clone(),
                    ready: checkpoint.status == CheckpointStatus::Ready,
                },
            );
            Ok(p.thread.worktree_path.is_some() && checkpoint.status == CheckpointStatus::Ready)
        });
        match result {
            Ok(files) => RewindAvailability {
                can_rewind: true,
                can_restore_files: files,
                reason: None,
            },
            Err(error) => RewindAvailability {
                reason: Some(rewind_reason(error)),
                ..Default::default()
            },
        }
    }
    pub fn cancel_rewind(&self, key: &ThreadKey) {
        self.state
            .rewind
            .confirmations
            .borrow_mut()
            .retain(|(thread, _, _), _| thread != key);
    }
    pub fn rewind(
        &self,
        key: ThreadKey,
        target: RewindTarget,
        restore_files: bool,
    ) -> LocalBoxFuture<'static, Result<String, ClientError>> {
        let service = self.clone();
        async move {
            let confirmation = service
                .state
                .rewind
                .confirmations
                .borrow_mut()
                .remove(&(key.clone(), target.message_id.0.clone(), target.turn_count))
                .ok_or_else(|| {
                    Self::refused("The thread has changed. Select a message to rewind again.")
                })?;
            let p = service.check_rewind_context(&key)?;
            confirmation.check(&p, restore_files)?;
            let message = confirmation.message.clone();
            let attachments = service
                .state
                .rewind
                .attachments
                .borrow()
                .clone()
                .ok_or(ClientError::Unsupported("composer attachment preflight"))?;
            let prepared = attachments
                .prepare(key.clone(), message.attachments.clone())
                .await?;
            let current = service.check_rewind_context(&key)?;
            confirmation.check(&current, restore_files)?;
            let id = crate::operations::allocate_command_id(&service.platform, None)?;
            let command = crate::operations::checkpoint_rollback_command(
                id.clone(),
                &current,
                Some(&confirmation.checkpoint),
                Some(&confirmation.scope),
                Some(target.turn_count),
                Some(restore_files),
            )?;
            let endpoint = service.endpoint(&key.environment)?;
            let generation = service.session_generation(&key.environment);
            let drafts = service.runtime.composer_drafts();
            let draft_key = drafts.thread_draft_key(&key);
            if !drafts.view_key(&draft_key).writable {
                return Err(ClientError::Unsupported("composer draft restoration"));
            }
            let demand = service
                .runtime
                .client(&key.environment)
                .ok_or(ClientError::Disconnected)?
                .open_thread(key.thread.clone());
            service
                .state
                .rewind
                .reverting
                .borrow_mut()
                .insert(key.clone());
            service.state.rewind.failures.borrow_mut().remove(&key);
            service.state.actions.notify();
            let reverting = Reverting(service.clone(), key.clone());
            let runtime = service.runtime.clone();
            let (send, receive) = futures::channel::oneshot::channel();
            runtime.spawn(
                async move {
                    let result = async {
                        crate::view_logic::chat_view::wait_for_reverted_message(
                            service.runtime.store(),
                            &service.platform,
                            &key,
                            target.turn_count,
                            id,
                            async {
                                endpoint
                                    .command::<WsOrchestrationV2DispatchCommandRpc>(command)
                                    .await
                                    .map_err(|error| {
                                        service.command_failure(
                                            &key.environment,
                                            "orchestration.dispatchCommand",
                                            generation,
                                            error,
                                        )
                                    })?;
                                Ok(())
                            },
                            30_000,
                        )
                        .await?;
                        let mut prompt = String::new();
                        drafts.update(|document| {
                            let current = document["draftsByThreadKey"][&draft_key]["prompt"]
                                .as_str()
                                .unwrap_or_default();
                            let restored = super::recall::recallable_prompt(&message.text);
                            prompt = if restored.is_empty() {
                                current.into()
                            } else if current.is_empty() {
                                restored
                            } else {
                                format!("{current}\n\n{restored}")
                            };
                            crate::thread_views::composer::web_models::set_prompt(
                                document, &draft_key, &prompt,
                            );
                            prepared.commit(document, &draft_key)?;
                            Ok(())
                        })?;
                        drafts.flush().await?;
                        Ok::<_, ClientError>(drafts.view_key(&draft_key).prompt.to_string())
                    }
                    .await;
                    if let Err(error) = &result {
                        service
                            .state
                            .rewind
                            .failures
                            .borrow_mut()
                            .insert(key, error.to_string());
                    }
                    drop(demand);
                    drop(reverting);
                    let _ = send.send(result);
                }
                .boxed_local(),
            );
            receive.await.map_err(|_| ClientError::Cancelled)?
        }
        .boxed_local()
    }
    pub fn fork(
        &self,
        key: ThreadKey,
        run: RunId,
    ) -> LocalBoxFuture<'static, Result<ThreadKey, ClientError>> {
        let service = self.clone();
        async move {
            if !service.rewind_state(&key).can_fork { return Err(ClientError::Unsupported("forking this response")); }
            let projection=service.projection(&key)?;
            let capabilities=crate::thread_views::threads::workflows::resolve_thread_provider_session(&projection).map(|s|&s.capabilities);
            if !projection.visible_turn_items.iter().any(|item|matches!(&item.item, OrchestrationV2TurnItem::AssistantMessage(message) if message.run_id==Some(IdOrIndex::Id(run.clone()))) && crate::thread_views::threads::workflows::can_fork_projected_assistant_item(item,capabilities)) {
                return Err(Self::refused("This response is no longer available to fork."));
            }
            let source = service.thread_shell(&key).ok_or(ClientError::Disconnected)?;
            let target = ThreadKey { environment:key.environment.clone(),thread:crate::types::ThreadId(crate::operations::allocate_command_id(&service.platform,None)?.0) };
            let input: OrchestrationV2Command = serde_json::from_value(serde_json::json!({"type":"thread.fork","commandId":crate::operations::allocate_command_id(&service.platform,None)?,"createdBy":"user","creationSource":"web","sourceThreadId":key.thread,"targetThreadId":target.thread,"sourcePoint":{"type":"run","runId":run},"title":format!("{} fork",source.title)}))?;
            let mut changes = service.changes(&target);
            let generation=service.session_generation(&key.environment);
            service.endpoint(&key.environment)?.command::<WsOrchestrationV2DispatchCommandRpc>(input).await.map_err(|error|service.command_failure(&key.environment,"orchestration.dispatchCommand",generation,error))?;
            let timeout = service.platform.sleep(10_000).fuse(); futures::pin_mut!(timeout);
            loop {
                if service.thread_shell(&target).is_some() { return Ok(target); }
                futures::select! { _ = timeout => return Err(Self::refused("The fork was created, but its thread data did not reach this client. Reconnect and try opening it from the sidebar.")), change = changes.next().fuse() => if change.is_none() { return Err(ClientError::Cancelled); } }
            }
        }.boxed_local()
    }
}

fn rewind_reason(error: ClientError) -> String {
    match error {
        ClientError::InvalidPayload(message) => message,
        error => error.to_string(),
    }
}
