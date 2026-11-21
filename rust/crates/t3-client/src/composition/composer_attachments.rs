use super::*;
use crate::thread_views::composer::{
    attachment_intake::{AttachmentConnection, AttachmentSession, Capabilities},
    target::ComposerTarget,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(super) struct RewindIntake<P: Platform + Clone + 'static>(pub(super) std::rc::Weak<Inner<P>>);

impl<P: Platform + Clone + 'static> crate::thread_views::thread_ops::rewind::RewindAttachments
    for RewindIntake<P>
{
    fn ready(&self, _: &ThreadKey) -> bool {
        self.0
            .upgrade()
            .is_some_and(|inner| inner.composer_drafts.writable())
    }

    fn prepare(
        &self,
        key: ThreadKey,
        attachments: Vec<t3_contracts::chat_attachment::ChatAttachment>,
    ) -> LocalBoxFuture<
        'static,
        Result<
            Box<dyn crate::thread_views::thread_ops::rewind::PreparedRewindAttachments>,
            ClientError,
        >,
    > {
        use crate::view_logic::chat_view::{AttachmentView, prepare_reverted_message_attachments};
        use t3_contracts::rpc::WsAssetsCreateUrlRpc;

        let inner = self.0.upgrade();
        async move {
            let runtime = ClientRuntime {
                inner: inner.ok_or(ClientError::Cancelled)?,
            };
            let session = runtime.composer_attachments(&ComposerTarget::Thread(key.clone()))?;
            let mut reservation = session.reserve_rewind(attachments.len())?;
            if !attachments.is_empty() {
                let client = runtime
                    .client(&key.environment)
                    .ok_or(ClientError::Disconnected)?;
                let endpoint = client.session().ok_or(ClientError::Disconnected)?;
                let base = client.http_base_url().ok_or(ClientError::Disconnected)?;
                let attachments = attachments
                    .into_iter()
                    .map(|attachment| AttachmentView {
                        attachment,
                        preview_url: None,
                        downloadable: true,
                        resource: None,
                    })
                    .collect::<Vec<_>>();
                let files = prepare_reverted_message_attachments(
                    &runtime.inner.platform,
                    &attachments,
                    &base,
                    |input| endpoint.call::<WsAssetsCreateUrlRpc>(input),
                )
                .await?;
                for (attachment, file) in attachments.iter().zip(files) {
                    reservation.prepare(&attachment.attachment, file)?;
                }
            }
            Ok(Box::new(reservation)
                as Box<
                    dyn crate::thread_views::thread_ops::rewind::PreparedRewindAttachments,
                >)
        }
        .boxed_local()
    }
}

pub(super) struct AttachmentDriver {
    pub session: AttachmentSession,
    task: Option<AbortHandle>,
    active: Rc<std::cell::Cell<bool>>,
}
impl Drop for AttachmentDriver {
    fn drop(&mut self) {
        self.active.set(false);
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
impl<P: Platform + Clone + 'static> ClientRuntime<P> {
    pub(crate) fn retire_composer_attachments(&self, keys: &[String]) {
        self.inner
            .composer_attachments
            .borrow_mut()
            .retain(|(_, key), _| !keys.contains(key));
    }
    pub(crate) async fn discard_deleted_thread_drafts(
        &self,
        threads: &[ThreadKey],
    ) -> Result<(), ClientError> {
        use crate::thread_views::composer::{mobile_drafts::AttachmentReferences, web_sessions};
        use t3_contracts::{
            assets::AttachmentDeleteInput, base_schemas::TrimmedNonEmptyString,
            rpc::WsAttachmentsDeleteRpc,
        };
        let drafts = &self.inner.composer_drafts;
        if !drafts.loaded() {
            drafts.load().await?;
        }
        let keys = threads
            .iter()
            .map(|thread| drafts.thread_draft_key(thread))
            .collect::<BTreeSet<_>>();
        let mut candidates = AttachmentReferences::default();
        drafts.update(|document| {
            for key in &keys {
                let attachments = draft_attachment_values(&document["draftsByThreadKey"][key])
                    .collect::<Vec<_>>();
                candidates.retain_owner(&json!({"attachments":attachments}));
                web_sessions::clear(document, key);
            }
            Ok(())
        })?;
        {
            let mut drivers = self.inner.composer_attachments.borrow_mut();
            let retired = drivers
                .keys()
                .filter(|(_, key)| keys.contains(key))
                .cloned()
                .collect::<Vec<_>>();
            for identity in retired {
                if let Some(mut driver) = drivers.remove(&identity) {
                    candidates
                        .uploads
                        .extend(driver.session.uploaded_references());
                    // Let in-flight calls observe that the draft is gone and release their own upload.
                    driver.task.take();
                    driver.session.refresh();
                }
            }
        }
        drafts.flush().await?;
        let mut failure = None;
        for (environment, id) in candidates.uploads {
            let environment = EnvironmentId(environment);
            let Some(messages) = self.messages(&environment) else {
                failure.get_or_insert(ClientError::Disconnected);
                continue;
            };
            match messages.flush_outbox().await {
                Ok(true) => {}
                Ok(false) => {
                    failure.get_or_insert_with(|| {
                        ClientError::InvalidPayload(
                            "Could not check pending messages for attachment references.".into(),
                        )
                    });
                    continue;
                }
                Err(error) => {
                    failure.get_or_insert(error);
                    continue;
                }
            }
            let mut retained = AttachmentReferences::default();
            for owner in messages.attachment_owners() {
                retained.retain_owner(&owner);
            }
            let document = drafts.document();
            for draft in document.borrow()["draftsByThreadKey"]
                .as_object()
                .into_iter()
                .flat_map(|drafts| drafts.values())
            {
                let attachments = draft_attachment_values(draft).collect::<Vec<_>>();
                retained.retain_owner(&json!({"attachments":attachments}));
            }
            if retained
                .uploads
                .contains(&(environment.0.clone(), id.clone()))
            {
                continue;
            }
            let Some(endpoint) = self
                .client(&environment)
                .and_then(|client| client.session())
            else {
                failure.get_or_insert(ClientError::Disconnected);
                continue;
            };
            if let Err(error) = endpoint
                .call::<WsAttachmentsDeleteRpc>(AttachmentDeleteInput {
                    attachment_id: TrimmedNonEmptyString(id),
                })
                .await
            {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    pub fn composer_attachments(
        &self,
        target: &ComposerTarget,
    ) -> Result<AttachmentSession, ClientError> {
        if !self.inner.composer_drafts.writable() {
            return Err(ClientError::Cancelled);
        }
        let environment = target
            .environment()
            .ok_or(ClientError::EnvironmentMismatch)?
            .clone();
        let document = self.inner.composer_drafts.document();
        let key = match target {
            ComposerTarget::Thread(key) => crate::thread_views::composer::web_sessions::resolve(
                &document.borrow(),
                crate::thread_views::composer::web_sessions::Target::Thread(
                    &t3_contracts::environment::ScopedThreadRef {
                        environment_id: key.environment.clone(),
                        thread_id: key.thread.clone(),
                    },
                ),
            ),
            ComposerTarget::Draft { draft_id, .. } => {
                crate::thread_views::composer::web_sessions::resolve(
                    &document.borrow(),
                    crate::thread_views::composer::web_sessions::Target::Draft(draft_id),
                )
            }
        }
        .ok_or_else(|| ClientError::InvalidPayload("A composer draft is required.".into()))?;
        let identity = (environment.clone(), key.clone());
        if let Some(driver) = self.inner.composer_attachments.borrow().get(&identity) {
            return Ok(driver.session.clone());
        }
        self.inner
            .composer_attachments
            .borrow_mut()
            .retain(|(_, draft_id), _| draft_id != &key);
        self.inner.composer_drafts.update(|document| {
            for field in ["images", "files"] {
                if let Some(attachments) = document["draftsByThreadKey"][&key][field].as_array_mut()
                {
                    for attachment in attachments {
                        if attachment["uploadEnvironmentId"]
                            .as_str()
                            .is_some_and(|id| id != environment.0)
                            && let Some(fields) = attachment.as_object_mut()
                        {
                            fields.remove("uploadedAttachmentId");
                            fields.remove("uploadEnvironmentId");
                        }
                    }
                }
            }
            Ok(())
        })?;
        let session = AttachmentSession::new(
            Rc::new(self.inner.platform.clone()),
            document.clone(),
            key.clone(),
            environment.clone(),
        );
        let (handle, registration) = AbortHandle::new_pair();
        let active = Rc::new(std::cell::Cell::new(true));
        session.bind_draft_owner(
            self.inner
                .composer_drafts
                .attachment_owner(key.clone(), active.clone()),
        );
        let driver_active = active.clone();
        let weak = Rc::downgrade(&self.inner);
        let source = session.clone();
        let drafts = self.inner.composer_drafts.clone();
        let store = self.inner.store.clone();
        let mut changes = futures::stream::select_all(vec![
            store
                .subscribe_environment(&environment)
                .map(|_| ())
                .boxed_local(),
            drafts.changes(),
            session.changes(),
        ]);
        let mut last_connection = None;
        let mut last_draft = json!([
            document.borrow()["draftsByThreadKey"][&key]["images"],
            document.borrow()["draftsByThreadKey"][&key]["files"]
        ]);
        (self.inner.spawn)(
            Abortable::new(
                async move {
                    let mut uploads =
                        futures::stream::FuturesUnordered::<LocalBoxFuture<'static, ()>>::new();
                    loop {
                        if uploads.is_empty() {
                            if changes.next().await.is_none() {
                                break;
                            }
                        } else if matches!(
                            futures::future::select(changes.next(), uploads.next()).await,
                            futures::future::Either::Left((None, _))
                        ) {
                            break;
                        }
                        if !driver_active.get() {
                            while uploads.next().await.is_some() {}
                            break;
                        }
                        let Some(inner) = weak.upgrade() else {
                            break;
                        };
                        let snapshot = store.snapshot();
                        let config = snapshot
                            .environments
                            .get(&environment)
                            .and_then(|entry| entry.config.as_ref());
                        let capabilities = Capabilities {
                            known: config.is_some(),
                            uploads: config.is_some(),
                            max_file_bytes: config
                                .and_then(|config| {
                                    config.environment.capabilities.file_attachments.as_ref()
                                })
                                .and_then(|capabilities| {
                                    u64::try_from(capabilities.max_upload_bytes).ok()
                                }),
                        };
                        let connection = inner
                            .environments
                            .borrow()
                            .iter()
                            .find(|entry| entry.id == environment)
                            .and_then(|entry| {
                                entry.client.as_ref().and_then(|client| {
                                    client.session().map(|endpoint| {
                                        (
                                            client.state().generation,
                                            AttachmentConnection {
                                                endpoint,
                                                // The session's own HTTP address, as message
                                                // delivery uses (`bind_sessions`): a relay or SSH
                                                // route saves no address of its own.
                                                base_url: client
                                                    .http_base_url()
                                                    .unwrap_or_default(),
                                            },
                                        )
                                    })
                                })
                            });
                        let connection_key = (
                            capabilities.clone(),
                            connection.as_ref().map(|(generation, connection)| {
                                (*generation, connection.base_url.clone())
                            }),
                        );
                        let connection_changed = last_connection.as_ref() != Some(&connection_key);
                        if connection_changed {
                            source.set_connection(
                                capabilities,
                                connection.map(|(_, connection)| connection),
                            );
                            last_connection = Some(connection_key);
                        }
                        drop(inner);
                        let current = json!([
                            document.borrow()["draftsByThreadKey"][&key]["images"],
                            document.borrow()["draftsByThreadKey"][&key]["files"]
                        ]);
                        let content_changed = current != last_draft;
                        if content_changed {
                            last_draft = current;
                            source.refresh();
                        }
                        if (connection_changed || content_changed)
                            && uploads.is_empty()
                            && source.capabilities().uploads
                        {
                            let source = source.clone();
                            uploads.push(
                                async move {
                                    let _ = source.upload_pending().await;
                                }
                                .boxed_local(),
                            );
                        }
                    }
                },
                registration,
            )
            .map(|_| ())
            .boxed_local(),
        );
        self.inner.composer_attachments.borrow_mut().insert(
            identity,
            AttachmentDriver {
                session: session.clone(),
                task: Some(handle),
                active,
            },
        );
        Ok(session)
    }
}

fn draft_attachment_values(draft: &Value) -> impl Iterator<Item = &Value> {
    ["images", "files"]
        .into_iter()
        .flat_map(|field| draft[field].as_array().into_iter().flatten())
}
