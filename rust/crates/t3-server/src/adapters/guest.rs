//! Bind guest capabilities to the orchestration runtime without giving them an owner session.

use std::sync::Arc;

use async_trait::async_trait;
use t3_cloud::{
    Error, Result,
    guest::{GuestEvents, GuestFailure, GuestSharing, GuestThread, GuestThreads},
    managed::ManagedEndpoint,
};
use t3_contracts::{
    base_schemas::{CommandId, GuestShareId, ThreadId, TrimmedNonEmptyString},
    guest_share::{GuestSubmitRequest, ThreadGuestShare},
    guest_stream::GuestStreamEvent,
    orchestration_v2::*,
};
use t3_host::guest::{guest_share_view, guest_thread_model};
use t3_orchestration::{
    api::{ProjectionReader, SnapshotWindow},
    composition::OrchestrationRuntime,
    ports::transport::thread_stream::{SnapshotOptions, build_bounded_thread_stream_snapshot},
};

use super::orchestration::OrchestrationAdapter;

pub(crate) struct GuestAdapter {
    pub sharing: GuestSharing,
    runtime: Arc<OrchestrationRuntime>,
}

struct Threads {
    runtime: Arc<OrchestrationRuntime>,
    settings: Arc<t3_config::settings::SettingsService>,
    connector: Arc<dyn ManagedEndpoint>,
    allowed: bool,
    #[cfg(feature = "providers")]
    providers: Option<Arc<t3_provider::service::ProviderService>>,
}

impl GuestAdapter {
    pub fn from_domains(domains: &crate::services::Domains) -> Option<Arc<Self>> {
        if let Some(guest) = domains.get::<Self>() {
            return Some(guest);
        }
        let cloud = domains.get::<super::cloud::CloudAdapter>()?;
        let orchestration = domains.get::<OrchestrationAdapter>()?;
        let runtime = orchestration.composed()?.clone();
        let threads = Arc::new(Threads {
            runtime: runtime.clone(),
            settings: cloud.guest_settings.clone(),
            connector: cloud.guest_connector.clone(),
            allowed: cloud.guest_connect_allowed,
            #[cfg(feature = "providers")]
            providers: domains
                .get::<super::providers::ProvidersAdapter>()
                .map(|adapter| adapter.domain.providers.clone()),
        });
        Some(Arc::new(Self {
            sharing: GuestSharing::new(threads, cloud.secrets.clone(), cloud.clock.clone()),
            runtime,
        }))
    }

    pub async fn item(
        &self,
        thread_id: ThreadId,
        item_id: t3_contracts::base_schemas::TurnItemId,
    ) -> Result<OrchestrationV2GetTurnItemResult> {
        use t3_orchestration::ports::transport::wire_projection::*;
        let mut item = self
            .runtime
            .projections
            .turn_item(thread_id, item_id)
            .await
            .map_err(unavailable)?
            .ok_or(Error::Guest(GuestFailure::Unavailable))?;
        if matches!(item, OrchestrationV2TurnItem::FileChange(_)) {
            project_turn_item_for_wire(&mut item)
        } else {
            project_turn_item_for_detail(&mut item)
        }
        .map_err(unavailable)?;
        Ok(OrchestrationV2GetTurnItemResult { item: Some(item) })
    }

    pub async fn command_output(
        &self,
        thread_id: ThreadId,
        item_id: t3_contracts::base_schemas::TurnItemId,
    ) -> Result<t3_orchestration::ports::transport::command_output_hub::CommandOutputSubscription>
    {
        let item = self
            .runtime
            .projections
            .turn_item(thread_id.clone(), item_id.clone())
            .await
            .map_err(unavailable)?;
        if !matches!(item, Some(OrchestrationV2TurnItem::CommandExecution(_))) {
            return Err(Error::Guest(GuestFailure::Unavailable));
        }
        self.runtime
            .command_output
            .as_ref()
            .ok_or(Error::Guest(GuestFailure::Unavailable))?
            .subscribe(OrchestrationV2SubscribeCommandOutputInput { thread_id, item_id })
            .await
            .map_err(unavailable)
    }

    pub async fn history(
        &self,
        thread_id: ThreadId,
        cursor: String,
    ) -> Result<OrchestrationV2ThreadHistoryPage> {
        self.runtime
            .transport
            .history_page(
                t3_orchestration::api::HistoryQuery {
                    thread_id,
                    cursor,
                    through_entry_id: None,
                    conversation_only: false,
                    work_mode: None,
                },
                false,
            )
            .await
            .map_err(unavailable)
    }
}

fn unavailable(_: t3_orchestration::error::Error) -> Error {
    Error::Guest(GuestFailure::Unavailable)
}

fn rejected(error: t3_orchestration::error::Error) -> Error {
    use t3_orchestration::ports::decisions::guest_share_decider::{
        BUSY, FILES_BUSY, INACTIVE, USED,
    };
    let reason = match error {
        t3_orchestration::error::Error::Rejected { reason, .. } => match reason.as_str() {
            INACTIVE => GuestFailure::Inactive,
            USED => GuestFailure::Used,
            BUSY => GuestFailure::Busy,
            FILES_BUSY => GuestFailure::FilesBusy,
            _ => GuestFailure::Rejected,
        },
        _ => GuestFailure::Rejected,
    };
    Error::Guest(reason)
}

impl Threads {
    async fn dispatch(&self, command: OrchestrationV2InternalCommand) -> Result<()> {
        self.runtime
            .dispatch_command(OrchestrationV2ServerCommand::Internal(Box::new(command)))
            .await
            .map(|_| ())
            .map_err(rejected)
    }
}

fn command_id(label: &str) -> CommandId {
    CommandId(format!(
        "server:guest-share:{label}:{}",
        uuid::Uuid::new_v4()
    ))
}

#[async_trait]
impl GuestThreads for Threads {
    async fn enabled(&self) -> Result<bool> {
        Ok(self
            .settings
            .snapshot()
            .await
            .is_ok_and(|settings| settings.guest_sharing_enabled))
    }

    async fn connect_hostname(&self) -> Result<Option<String>> {
        if !self.allowed {
            return Ok(None);
        }
        Ok(self
            .connector
            .inspect()
            .await
            .and_then(|facts| facts.hostname))
    }

    async fn find_share(&self, id: &GuestShareId) -> Result<Option<ThreadId>> {
        self.runtime
            .projections
            .find_guest_share_thread_id(id.clone())
            .await
            .map_err(unavailable)
    }

    async fn read(&self, id: &ThreadId) -> Result<GuestThread> {
        let thread = self
            .runtime
            .projections
            .thread(id.clone())
            .await
            .map_err(unavailable)?
            .ok_or(Error::Guest(GuestFailure::Unavailable))?;
        Ok(GuestThread {
            thread_id: id.clone(),
            archived: thread.archived_at.is_some(),
            deleted: thread.deleted_at.is_some(),
            share: thread.guest_share.flatten(),
        })
    }

    async fn open(&self, thread: &ThreadId, share: ThreadGuestShare) -> Result<()> {
        self.dispatch(OrchestrationV2InternalCommand::ThreadGuestShareOpen(
            Box::new(InternalCommandThreadGuestShareOpen {
                command_id: command_id("open"),
                thread_id: thread.clone(),
                share_id: share.id,
                host: share.host,
                expires_at: share.expires_at,
            }),
        ))
        .await
    }

    async fn redeem(&self, thread: &ThreadId, share: &GuestShareId, name: &str) -> Result<()> {
        self.dispatch(OrchestrationV2InternalCommand::ThreadGuestShareRedeem(
            Box::new(InternalCommandThreadGuestShareRedeem {
                command_id: command_id("redeem"),
                thread_id: thread.clone(),
                share_id: share.clone(),
                guest_name: TrimmedNonEmptyString(name.into()),
            }),
        ))
        .await
    }

    async fn submit(&self, thread: &ThreadId, request: GuestSubmitRequest) -> Result<()> {
        self.dispatch(OrchestrationV2InternalCommand::ThreadGuestMessageSubmit(
            Box::new(InternalCommandThreadGuestMessageSubmit {
                command_id: command_id("submit"),
                thread_id: thread.clone(),
                share_id: request.share_id,
                guest_message_id: t3_contracts::base_schemas::GuestMessageId(
                    uuid::Uuid::new_v4().to_string(),
                ),
                text: request.text.0,
                attachments: None,
            }),
        ))
        .await
    }

    async fn stream(&self, thread: &ThreadId, share: &GuestShareId) -> Result<GuestEvents> {
        let snapshot = self
            .runtime
            .projections
            .snapshot(
                thread.clone(),
                Some(SnapshotWindow {
                    row_limit: 600,
                    user_turn_limit: Some(20),
                    ..Default::default()
                }),
            )
            .await
            .map_err(unavailable)?;
        let providers = Vec::new();
        #[cfg(feature = "providers")]
        let providers = match &self.providers {
            Some(service) => service.get_providers().await.0,
            None => providers,
        };
        let current = &snapshot.projection.thread;
        let initial = guest_share_view(
            current,
            guest_thread_model(&current.model_selection, &providers),
        )
        .filter(|view| &view.share_id == share)
        .ok_or(Error::Guest(GuestFailure::Inactive))?;
        let live_input = OrchestrationV2SubscribeThreadInput {
            thread_id: thread.clone(),
            after_sequence: Some(snapshot.snapshot_sequence),
            request_completion_marker: None,
            accept_bounded_snapshot: Some(true),
            accept_deferred_work: None,
            accept_compact_turn_items: None,
            accept_deduped_bodies: None,
            accept_compact_ids: None,
        };
        let initial_item = build_bounded_thread_stream_snapshot(
            snapshot.snapshot_sequence,
            snapshot.projection,
            SnapshotOptions::default(),
        )
        .map_err(unavailable)?;
        let mut live = self
            .runtime
            .transport
            .subscribe_thread(
                live_input,
                t3_orchestration::ports::transport::live_stream_budget::LiveStreamLimits::default(),
            )
            .await
            .map_err(unavailable)?;
        let share = share.clone();
        Ok(Box::pin(async_stream::stream! {
            yield GuestStreamEvent::View { view: Box::new(initial) };
            yield GuestStreamEvent::Thread { item: Box::new(initial_item) };
            'stream: while let Ok(Some(batch)) = live.pull().await {
                for entry in batch {
                let item = entry.value.clone();
                let updated = match &item {
                    OrchestrationV2ThreadStreamItem::Snapshot(snapshot) => Some(&snapshot.projection.thread),
                    OrchestrationV2ThreadStreamItem::Event(item) => match &item.event {
                        OrchestrationV2DomainEvent::ThreadArchived(_) | OrchestrationV2DomainEvent::ThreadDeleted(_) => {
                            yield GuestStreamEvent::Ended;
                            break 'stream;
                        }
                        OrchestrationV2DomainEvent::ThreadGuestShareUpdated(event)
                        | OrchestrationV2DomainEvent::ThreadMetadataUpdated(event)
                        | OrchestrationV2DomainEvent::ThreadModelSelectionUpdated(event)
                        | OrchestrationV2DomainEvent::ThreadProviderSwitched(event) => Some(&event.payload),
                        _ => None,
                    },
                    _ => continue,
                };
                let view = if let Some(updated) = updated {
                    let Some(view) = guest_share_view(updated, guest_thread_model(&updated.model_selection, &providers))
                        .filter(|view| view.share_id == share)
                    else {
                        yield GuestStreamEvent::Ended;
                        break 'stream;
                    };
                    Some(view)
                } else {
                    None
                };
                yield GuestStreamEvent::Thread { item: Box::new(item) };
                if let Some(view) = view {
                    yield GuestStreamEvent::View { view: Box::new(view) };
                }
            }
            }
        }))
    }
}

#[cfg(test)]
mod tests;
