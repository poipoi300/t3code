use super::*;
use crate::thread_views::composer::{
    attachment_intake::{AttachmentConnection, AttachmentSession, Capabilities},
    target::ComposerTarget,
};
use serde_json::json;

pub(super) struct AttachmentDriver {
    pub session: AttachmentSession,
    task: AbortHandle,
    active: Rc<std::cell::Cell<bool>>,
}
impl Drop for AttachmentDriver {
    fn drop(&mut self) {
        self.active.set(false);
        self.task.abort();
    }
}
impl<P: Platform + Clone + 'static> ClientRuntime<P> {
    pub(crate) fn retire_composer_attachments(&self, keys: &[String]) {
        self.inner
            .composer_attachments
            .borrow_mut()
            .retain(|(_, key), _| !keys.contains(key));
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
                task: handle,
                active,
            },
        );
        Ok(session)
    }
}
