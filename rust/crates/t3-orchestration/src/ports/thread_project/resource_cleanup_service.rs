use crate::{
    api::{ResourceCleanup, ResourceKind},
    effects::EffectRequest,
    error::{Error, Result},
};
use std::sync::Arc;
use t3_contracts::base_schemas::ThreadId;

pub struct ResourceCleanupService {
    backend: Arc<dyn ResourceCleanup>,
}
impl ResourceCleanupService {
    pub fn new(backend: Arc<dyn ResourceCleanup>) -> Self {
        Self { backend }
    }

    pub async fn execute(&self, thread_id: ThreadId, request: &EffectRequest) -> Result<()> {
        let (kind, operation) = match request {
            EffectRequest::CleanupScheduledTasks => {
                (ResourceKind::ScheduledTasks, "scheduled-tasks")
            }
            EffectRequest::CleanupTerminals => (ResourceKind::Terminals, "terminal"),
            EffectRequest::CleanupPreviews => (ResourceKind::Previews, "preview"),
            EffectRequest::CleanupAttachments { attachment_ids } => (
                ResourceKind::Attachments(attachment_ids.clone()),
                "attachment",
            ),
            _ => return Err(Error::Unsupported("resource cleanup effect")),
        };
        self.cleanup(thread_id, kind, operation).await
    }

    pub async fn cleanup_devices(&self, thread_id: ThreadId) -> Result<()> {
        self.cleanup(thread_id, ResourceKind::Devices, "device")
            .await
    }

    async fn cleanup(
        &self,
        thread_id: ThreadId,
        kind: ResourceKind,
        operation: &'static str,
    ) -> Result<()> {
        self.backend
            .cleanup(thread_id, kind)
            .await
            .map_err(|error| Error::Service {
                operation,
                detail: error.to_string(),
            })
    }
}
