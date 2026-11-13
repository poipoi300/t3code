pub use crate::ports::runs_ingestion::{
    provider_event_ingestor::{IngestInput, ProviderEventIngestor},
    run_execution_service::{ProviderEventRouteIdentity, ProviderEventRoutingState},
    runtime_request_service::RuntimeRequestService,
};

use crate::{
    api::{
        CommitOptions, CommitPreparation, CommitRequest, CommitResult, EventCommitter,
        OrchestrationV2StoredEvent,
    },
    error::Result,
    runtime::commit::EventSink,
    stores::receipts::CommandReceipt,
};
use async_trait::async_trait;
use t3_contracts::{
    application_event::{ApplicationProjectEvent, ApplicationStoredEvent},
    base_schemas::{CommandId, ThreadId},
};
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct ProviderCommitter(pub EventSink);

#[async_trait]
impl EventCommitter for ProviderCommitter {
    async fn commit(&self, input: CommitRequest) -> Result<CommitResult> {
        self.commit_with_options(input, CommitOptions::default())
            .await
    }

    async fn commit_with_options(
        &self,
        input: CommitRequest,
        mut options: CommitOptions,
    ) -> Result<CommitResult> {
        options.guard_pending_user_input_cancellations = true;
        self.0.commit_with_options(input, options).await
    }

    async fn commit_prepared(
        &self,
        input: CommitRequest,
        mut options: CommitOptions,
        prepare: CommitPreparation,
    ) -> Result<CommitResult> {
        options.guard_pending_user_input_cancellations = true;
        self.0.commit_prepared(input, options, prepare).await
    }

    async fn commit_project(
        &self,
        event: ApplicationProjectEvent,
        receipt: CommandReceipt,
    ) -> Result<CommandReceipt> {
        self.0.commit_project(event, receipt).await
    }

    async fn events_by_command(
        &self,
        command_id: CommandId,
    ) -> Result<Vec<OrchestrationV2StoredEvent>> {
        self.0.events_by_command(command_id).await
    }

    fn subscribe(&self) -> broadcast::Receiver<ApplicationStoredEvent> {
        self.0.subscribe()
    }

    async fn latest_sequence(&self, thread_id: Option<ThreadId>) -> Result<u64> {
        self.0.latest_sequence(thread_id).await
    }
}
