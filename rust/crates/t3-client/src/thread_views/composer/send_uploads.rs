use super::{
    send::SendService,
    send_transport::{Failure, FailureKind},
};
use crate::{
    ClientError,
    platform::Platform,
    rpc::RpcEndpoint,
    thread_views::outbox::{DraftAttachment, QueuedMessage},
    types::Generation,
};
use serde_json::json;
use t3_contracts::chat_attachment::{ChatAttachment, UploadChatAttachment};

impl<P: Platform + 'static, E: RpcEndpoint + 'static> SendService<P, E> {
    pub(super) async fn legacy_upload(
        &self,
        attachment: &DraftAttachment,
        generation: Generation,
    ) -> Result<UploadChatAttachment, ClientError> {
        use base64::Engine;
        let value = attachment.value();
        let data = if let Some(data) = value["dataUrl"].as_str() {
            data.to_owned()
        } else {
            let bytes = self.attachment_bytes(attachment, generation).await?;
            format!(
                "data:{};base64,{}",
                value["mimeType"]
                    .as_str()
                    .unwrap_or("application/octet-stream"),
                base64::engine::general_purpose::STANDARD.encode(bytes)
            )
        };
        Ok(serde_json::from_value(
            json!({"type":value["type"],"id":attachment.id(),"name":attachment.name(),"mimeType":value["mimeType"],"sizeBytes":value["sizeBytes"],"dataUrl":data}),
        )?)
    }
    async fn attachment_bytes(
        &self,
        attachment: &DraftAttachment,
        generation: Generation,
    ) -> Result<Vec<u8>, ClientError> {
        let value = attachment.value();
        let limit = if attachment.is_file() {
            t3_contracts::chat_attachment::PROVIDER_SEND_TURN_MAX_FILE_BYTES
        } else {
            t3_contracts::chat_attachment::PROVIDER_SEND_TURN_MAX_IMAGE_BYTES
        };
        let bytes = if value["dataUrl"].is_string() {
            super::web_drafts::hydrate_image(value)
                .map(|image| image.bytes)
                .ok_or_else(|| {
                    ClientError::InvalidPayload(format!(
                        "Reattach {} before sending.",
                        attachment.name()
                    ))
                })?
        } else {
            let uri = value["fileUri"]
                .as_str()
                .filter(|uri| !uri.is_empty())
                .ok_or_else(|| {
                    ClientError::InvalidPayload(format!(
                        "Reattach {} before sending.",
                        attachment.name()
                    ))
                })?;
            self.inner
                .platform
                .read_resource(uri.into(), limit as usize)
                .await?
        };
        self.check_current(generation)?;
        if bytes.len() as u64 > limit {
            return Err(ClientError::InvalidPayload(format!(
                "{} exceeds the attachment limit.",
                attachment.name()
            )));
        }
        Ok(bytes)
    }
    pub(super) async fn upload_attachment(
        &self,
        message: &QueuedMessage,
        attachment: &DraftAttachment,
        generation: Generation,
        endpoint: &E,
        base: &str,
    ) -> Result<ChatAttachment, ClientError> {
        use crate::services::assets::{AttachmentUploadResult, attachment_upload_cycle};
        let value = attachment.value();
        let body = self.attachment_bytes(attachment, generation).await?;
        let input = serde_json::from_value(
            json!({"type":value["type"],"name":attachment.name(),"mimeType":value["mimeType"],"sizeBytes":body.len()}),
        )?;
        let result = attachment_upload_cycle(
            endpoint,
            self.inner.platform.as_ref(),
            base,
            input,
            body,
            value["mimeType"]
                .as_str()
                .unwrap_or("application/octet-stream"),
            |_| {
                self.current(generation)
                    && !self.inner.editing.borrow().contains(&message.message_id)
            },
        )
        .await;
        let id = match result {
            AttachmentUploadResult::Uploaded { attachment_id } => attachment_id,
            AttachmentUploadResult::Cancelled { .. } => {
                return Err(self.fail(Failure {
                    kind: FailureKind::Stale,
                    reason: "The attachment upload was abandoned.".into(),
                    command: None,
                }));
            }
            AttachmentUploadResult::Failed {
                attachment_id,
                error,
                ..
            } => {
                if let Some(id) = attachment_id {
                    let _ = endpoint
                        .call::<t3_contracts::rpc::WsAttachmentsDeleteRpc>(serde_json::from_value(
                            json!({"attachmentId":id}),
                        )?)
                        .await;
                }
                return Err(error);
            }
        };
        self.check_current(generation)?;
        Ok(serde_json::from_value(
            json!({"type":value["type"],"id":id,"name":attachment.name(),"mimeType":value["mimeType"],"sizeBytes":value["sizeBytes"]}),
        )?)
    }
    pub(super) async fn persist_upload_references(
        &self,
        message: &QueuedMessage,
        attachments: &[ChatAttachment],
    ) -> Result<bool, ClientError> {
        let mut next = message.clone();
        for (original, uploaded) in next.attachments.iter_mut().zip(attachments) {
            let mut value = original.value().clone();
            value["uploadEnvironmentId"] = json!(message.environment_id);
            value["uploadedAttachmentId"] = json!(crate::operations::attachment_id(uploaded));
            *original = DraftAttachment::decode(value)?;
        }
        next.compact_uploaded_attachments();
        if &next == message {
            return Ok(false);
        }
        let matches = self
            .inner
            .outbox
            .messages()
            .iter()
            .any(|current| current.as_ref() == message);
        if !matches {
            return Err(self.fail(Failure {
                kind: FailureKind::Stale,
                reason: "The queued message changed during upload.".into(),
                command: None,
            }));
        }
        let revision = self.inner.outbox.revision(&message.message_id);
        if !self
            .inner
            .outbox
            .update(next.clone(), Some(revision))
            .await?
        {
            return Err(self.fail(Failure {
                kind: FailureKind::Stale,
                reason: "The queued message changed during upload.".into(),
                command: None,
            }));
        }
        self.inner.prepared.borrow_mut().insert(
            message.message_id.clone(),
            (next.attachments, attachments.to_vec()),
        );
        self.inner.changes.notify();
        Ok(true)
    }
}
