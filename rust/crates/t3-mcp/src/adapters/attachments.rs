use crate::{services::AttachmentService, session::Clock, *};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
use t3_contracts::assets::{AttachmentCreateUploadUrlResult, AttachmentDeleteInput};
use t3_workspace::attachments::AttachmentStore;

pub struct AttachmentAdapter {
    pub store: Arc<AttachmentStore>,
    pub clock: Arc<dyn Clock>,
    owners: tokio::sync::Mutex<HashMap<String, (String, i64)>>,
}
impl AttachmentAdapter {
    pub fn new(store: Arc<AttachmentStore>, clock: Arc<dyn Clock>) -> Self {
        Self {
            store,
            clock,
            owners: Default::default(),
        }
    }
}
fn owner(context: &CallContext) -> &str {
    context
        .scope
        .thread
        .as_ref()
        .map_or(context.scope.request_namespace.as_str(), |t| {
            t.thread_id.0.as_str()
        })
}
#[async_trait]
impl AttachmentService for AttachmentAdapter {
    async fn t3_attachment_prepare_upload(
        &self,
        context: CallContext,
        input: DecodedInput,
    ) -> Result<AttachmentCreateUploadUrlResult> {
        let now = self.clock.now().wall_ms;
        let result = self
            .store
            .issue_upload(
                super::decoded(input.value()["upload"].clone())?,
                now.max(0) as u64,
            )
            .await
            .map_err(|_| McpError::unavailable())?;
        let mut owners = self.owners.lock().await;
        owners.retain(|_, (_, issued)| {
            now.saturating_sub(*issued)
                <= 86_400_000 + t3_contracts::assets::ATTACHMENT_UPLOAD_URL_TTL_MS as i64
        });
        owners.insert(
            result.attachment_id.0.clone(),
            (owner(&context).into(), now),
        );
        Ok(result)
    }
    async fn t3_attachment_discard(
        &self,
        context: CallContext,
        input: AttachmentDeleteInput,
    ) -> Result<Value> {
        let mut owners = self.owners.lock().await;
        let id = super::encoded(input)?["attachmentId"]
            .as_str()
            .ok_or(McpError::Internal)?
            .to_owned();
        if owners
            .get(&id)
            .is_none_or(|(caller, _)| caller != owner(&context))
        {
            return Err(McpError::failure(
                "invalid_request",
                "Only the caller that prepared a pending upload can discard it.",
            ));
        }
        self.store
            .delete_pending(&id)
            .await
            .map_err(|_| McpError::unavailable())?;
        owners.remove(&id);
        Ok(json!({}))
    }
}
