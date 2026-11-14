//! Scheduled-task service boundary and raw webhook HTTP transport.
use crate::{registry::Failure, services::DomainRuntime};
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use futures::{StreamExt, future::BoxFuture, stream::BoxStream};
use serde_json::json;
use std::sync::{Arc, Mutex};
use t3_contracts::{
    base_schemas::ScheduledTaskId, rpc::WsScheduledTasksSubscribeError, scheduled_task::*,
};
pub use t3_scheduler::{
    Clock, DispatchReceipt, DispatchRequest, Orchestration, RelayDeliveryProof,
    Result as SchedulerResult, ScheduledTaskService, SchedulerOptions, SystemClock, TokioClock,
};
use t3_scheduler::{
    Registration, Scheduler, TriggerResult, WEBHOOK_MAX_BODY_BYTES, WebhookTriggerRequest,
};

pub struct SchedulerAdapter {
    service: Arc<ScheduledTaskService>,
    clock: Arc<dyn Clock>,
    relay: Option<RelayDeliveryProof>,
    scheduler: Scheduler,
    registrations: Mutex<Vec<Registration>>,
    commands_available: bool,
}

impl SchedulerAdapter {
    /// The supplied service has completed recovery; sources remain parked until `activate`.
    pub fn new(
        service: Arc<ScheduledTaskService>,
        clock: Arc<dyn Clock>,
        relay: Option<RelayDeliveryProof>,
    ) -> Self {
        let scheduler = Scheduler::new(clock.clone());
        let registrations = service
            .sources()
            .into_iter()
            .map(|source| scheduler.register(source))
            .collect();
        Self {
            service,
            clock,
            relay,
            scheduler,
            registrations: Mutex::new(registrations),
            commands_available: true,
        }
    }
    /// Call after the composition root's recovery barriers, before publishing ready.
    pub fn activate(&self) {
        if self.commands_available {
            self.scheduler.activate();
        }
    }
    /// The one scheduled-task service; project promotion moves a thread's tasks through it.
    pub fn service(&self) -> Arc<ScheduledTaskService> {
        self.service.clone()
    }
    pub fn commands_available(&self) -> bool {
        self.commands_available
    }
    pub async fn stop(&self) {
        self.registrations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.service.shutdown().await;
    }
    pub async fn subscribe(
        &self,
    ) -> Result<
        BoxStream<
            'static,
            Result<ScheduledTaskListResult, Failure<WsScheduledTasksSubscribeError>>,
        >,
        Failure<WsScheduledTasksSubscribeError>,
    > {
        let mut subscription = self
            .service
            .subscribe_list()
            .await
            .map_err(|e| failure(e, None))?;
        Ok(async_stream::try_stream! {
            while let Some(list) = subscription.next().await.map_err(|e| failure(e, None))? {
                yield list;
            }
        }
        .boxed())
    }
    pub async fn upsert(
        &self,
        input: ScheduledTaskUpsertInput,
    ) -> Result<ScheduledTaskMutationResult, Failure<WsScheduledTasksSubscribeError>> {
        let id = input.id.clone().flatten();
        self.service
            .upsert(input)
            .await
            .map(|task| ScheduledTaskMutationResult { task })
            .map_err(|e| failure(e, id))
    }
    pub async fn set_enabled(
        &self,
        input: ScheduledTaskSetEnabledInput,
    ) -> Result<ScheduledTaskMutationResult, Failure<WsScheduledTasksSubscribeError>> {
        let id = input.id.clone();
        self.service
            .set_enabled(input)
            .await
            .map(|task| ScheduledTaskMutationResult { task })
            .map_err(|e| failure(e, Some(id)))
    }
    pub async fn delete(
        &self,
        input: ScheduledTaskDeleteInput,
    ) -> Result<ScheduledTaskDeleteResult, Failure<WsScheduledTasksSubscribeError>> {
        self.service
            .delete(&input.id)
            .await
            .map_err(|e| failure(e, Some(input.id.clone())))?;
        Ok(input)
    }
    pub async fn run_now(
        &self,
        input: ScheduledTaskRunNowInput,
    ) -> Result<ScheduledTaskRunNowResult, Failure<WsScheduledTasksSubscribeError>> {
        self.service
            .run_now(&input.id)
            .await
            .map(|result| ScheduledTaskRunNowResult { task: result.task })
            .map_err(|e| failure(e, Some(input.id)))
    }
    pub async fn rotate_webhook_token(
        &self,
        input: ScheduledTaskRotateWebhookTokenInput,
    ) -> Result<ScheduledTaskMutationResult, Failure<WsScheduledTasksSubscribeError>> {
        self.service
            .rotate_webhook_token(&input.id)
            .await
            .map(|task| ScheduledTaskMutationResult { task })
            .map_err(|e| failure(e, Some(input.id)))
    }
    pub async fn list_webhook_deliveries(
        &self,
        input: ScheduledTaskListWebhookDeliveriesInput,
    ) -> Result<ScheduledTaskListWebhookDeliveriesResult, Failure<WsScheduledTasksSubscribeError>>
    {
        let id = input.id.clone();
        self.service
            .list_webhook_deliveries(input)
            .await
            .map_err(|e| failure(e, Some(id)))
    }
    pub async fn get_webhook_delivery(
        &self,
        input: ScheduledTaskGetWebhookDeliveryInput,
    ) -> Result<ScheduledTaskGetWebhookDeliveryResult, Failure<WsScheduledTasksSubscribeError>>
    {
        let id = input.id.clone();
        self.service
            .get_webhook_delivery(input)
            .await
            .map_err(|e| failure(e, Some(id)))
    }
}
impl DomainRuntime for SchedulerAdapter {
    fn name(&self) -> &'static str {
        "scheduler"
    }
    fn activate(&self) -> BoxFuture<'_, crate::Result<()>> {
        Box::pin(async {
            self.activate();
            Ok(())
        })
    }
    fn shutdown(&self) -> BoxFuture<'_, crate::Result<()>> {
        Box::pin(async {
            self.stop().await;
            Ok(())
        })
    }
}
fn failure(
    error: t3_scheduler::Error,
    task_id: Option<ScheduledTaskId>,
) -> Failure<WsScheduledTasksSubscribeError> {
    use t3_scheduler::Error;
    // TypeScript's texts; storage causes stay out of the client's message.
    let message = match error {
        Error::NotFound(_) => "Schedule task not found.".into(),
        Error::Persistence(_) | Error::Sqlite(_) | Error::Json(_) => {
            "Could not update schedule task.".into()
        }
        Error::CompletionAfterDispatch(_) => {
            "The run started, but completion could not be recorded.".into()
        }
        error => error.to_string(),
    };
    Failure::Typed(WsScheduledTasksSubscribeError::ScheduledTaskError(
        Box::new(ScheduledTaskError {
            _tag: ScheduledTaskErrorTag::ScheduledTaskError,
            message,
            task_id: task_id.map(Some),
            cause: None,
        }),
    ))
}

/// Merge before the static fallback; it uses the same domain instance as the RPC handlers.
pub fn routes(
    router: Router<Arc<crate::services::ServiceRegistry>>,
) -> Router<Arc<crate::services::ServiceRegistry>> {
    router.route(
        "/api/hooks/{hook_id}/{token}",
        get(webhook).post(webhook).put(webhook).patch(webhook),
    )
}
async fn webhook(
    State(services): State<Arc<crate::services::ServiceRegistry>>,
    Path((hook_id, token)): Path<(String, String)>,
    request: Request,
) -> Response {
    let Some(adapter) = services.domain::<SchedulerAdapter>() else {
        return response(
            StatusCode::NOT_FOUND,
            json!({"error":"hook_not_found"}),
            "not_found",
        );
    };
    if services.shutdown.is_cancelled() {
        return response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error":"internal_error"}),
            "error",
        );
    }
    adapter.handle_webhook(hook_id, token, request).await
}
impl SchedulerAdapter {
    pub async fn handle_webhook(
        &self,
        hook_id: String,
        token: String,
        request: Request,
    ) -> Response {
        if request.method() == Method::HEAD {
            let mut result = response(
                StatusCode::METHOD_NOT_ALLOWED,
                json!({"error":"method_not_allowed"}),
                "method_not_allowed",
            );
            result
                .headers_mut()
                .insert("allow", HeaderValue::from_static("GET, POST, PUT, PATCH"));
            return result;
        }
        if !matches!(
            *request.method(),
            Method::GET | Method::POST | Method::PUT | Method::PATCH
        ) {
            return response(
                StatusCode::METHOD_NOT_ALLOWED,
                json!({"error":"method_not_allowed"}),
                "method_not_allowed",
            );
        }
        if !self.commands_available {
            return response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({"error":"internal_error"}),
                "error",
            );
        }
        let (parts, body) = request.into_parts();
        if parts.headers.get("content-length").is_some_and(|length| {
            length
                .to_str()
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .is_none_or(|n| n > WEBHOOK_MAX_BODY_BYTES as u64)
        }) {
            return response(
                StatusCode::PAYLOAD_TOO_LARGE,
                json!({"error":"body_too_large"}),
                "body_too_large",
            );
        }
        let Ok(body) = read_body(body).await else {
            return response(
                StatusCode::PAYLOAD_TOO_LARGE,
                json!({"error":"body_too_large_or_unreadable"}),
                "body_too_large",
            );
        };
        let headers = request_headers(&parts.headers);
        let relay = self.relay.as_ref().and_then(|proof| {
            proof.verify(
                &headers.iter().cloned().collect(),
                &hook_id,
                self.clock.now(),
            )
        });
        let path = format!(
            "/api/hooks/{}",
            percent_encoding::utf8_percent_encode(&hook_id, ENCODE_COMPONENT)
        );
        let input = WebhookTriggerRequest {
            hook_id,
            token,
            body,
            request: t3_scheduler::webhook::WebhookRequest {
                method: parts.method.to_string(),
                path,
                query: parts.uri.query().unwrap_or_default().into(),
                headers: headers.into_iter().collect(),
                body_text: String::new(),
            },
            relay,
        };
        match self.service.trigger_webhook(input).await {
            Ok(result) => {
                let body = match &result {
                    TriggerResult::Accepted { delivery_id, .. } => {
                        json!({"deliveryId":delivery_id})
                    }
                    TriggerResult::NotFound => json!({"error":"hook_not_found"}),
                    TriggerResult::RejectedSignature => json!({"error":"invalid_signature"}),
                    TriggerResult::Disabled => json!({"error":"hook_disabled"}),
                    TriggerResult::BlockedByPolicy => json!({"error":"blocked_by_policy"}),
                    TriggerResult::RateLimited { outcome } => json!({"error":outcome}),
                    TriggerResult::Expired => json!({"error":"delivery_too_old"}),
                };
                response(
                    StatusCode::from_u16(result.http_status())
                        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    body,
                    result.outcome(),
                )
            }
            Err(_) => response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({"error":"internal_error"}),
                "error",
            ),
        }
    }
}
fn request_headers(headers: &axum::http::HeaderMap) -> Vec<(String, String)> {
    headers
        .keys()
        .map(|name| {
            let values = headers
                .get_all(name)
                .iter()
                .map(|value| {
                    value
                        .as_bytes()
                        .iter()
                        .map(|byte| char::from(*byte))
                        .collect::<String>()
                })
                .collect::<Vec<_>>();
            let separator = if name == axum::http::header::COOKIE {
                "; "
            } else {
                ", "
            };
            (name.as_str().to_owned(), values.join(separator))
        })
        .collect()
}
async fn read_body(body: Body) -> Result<Vec<u8>, ()> {
    let mut stream = body.into_data_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ())?;
        if chunk.len() > WEBHOOK_MAX_BODY_BYTES - bytes.len() {
            return Err(());
        }
        let needed = bytes.len() + chunk.len();
        if needed > bytes.capacity() {
            bytes.reserve_exact(
                needed.next_power_of_two().min(WEBHOOK_MAX_BODY_BYTES) - bytes.len(),
            );
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
const ENCODE_COMPONENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');
fn response(status: StatusCode, body: serde_json::Value, outcome: &str) -> Response {
    let mut response = (status, Json(body)).into_response();
    if let Ok(value) = HeaderValue::from_str(outcome) {
        response.headers_mut().insert("x-t3-hook-outcome", value);
    }
    response
}

/// Read-only scheduler composition until command intake and destination validation exist.
#[cfg(feature = "local-services")]
struct UnassembledIntake;
#[cfg(feature = "local-services")]
#[async_trait::async_trait]
impl t3_scheduler::Orchestration for UnassembledIntake {
    async fn validate(
        &self,
        _task: &ScheduledTask,
        _destination_changed: bool,
    ) -> t3_scheduler::Result<()> {
        Err(t3_scheduler::Error::Invalid(
            "Orchestration command intake is not assembled in this server.".into(),
        ))
    }
    async fn dispatch(
        &self,
        _request: t3_scheduler::DispatchRequest,
    ) -> t3_scheduler::Result<t3_scheduler::DispatchReceipt> {
        Err(t3_scheduler::Error::Dispatch(
            "Orchestration command intake is not assembled in this server.".into(),
        ))
    }
}
#[cfg(feature = "local-services")]
impl SchedulerAdapter {
    /// Recovers persisted state; dispatch, destination mutations and sources remain unavailable.
    pub async fn start(context: &crate::services::DomainContext<'_>) -> crate::Result<Arc<Self>> {
        let mut adapter = Self::build(
            context,
            Arc::new(UnassembledIntake),
            SchedulerOptions::default(),
            None,
            None,
        )
        .await?;
        adapter.commands_available = false;
        Ok(Arc::new(adapter))
    }
    /// Over the shared orchestration runtime when it started: a task that fires is a queued
    /// `message.dispatch` through the same intake, dispatch ceiling and turn policy as a client
    /// message. Without the runtime, tasks stay readable and commands unserved.
    pub async fn start_for(
        context: &crate::services::DomainContext<'_>,
        domains: &crate::services::Domains,
        clock: Option<Arc<dyn Clock>>,
    ) -> crate::Result<Arc<Self>> {
        #[cfg(feature = "orchestration")]
        if let Some(runtime) = domains
            .get::<crate::adapters::orchestration::OrchestrationAdapter>()
            .and_then(|adapter| adapter.composed().cloned())
        {
            let intake = Arc::new(runtime_intake::RuntimeIntake {
                runtime: runtime.clone(),
                shutdown: context.shutdown.clone(),
            });
            let backend = Arc::new(t3_scheduler::intake::IntakeDispatchBackend::new(intake));
            // Unbound tasks launch their thread (`adapters/scheduled_launch.rs`).
            #[cfg(feature = "local-services")]
            let backend = crate::adapters::scheduled_launch::LaunchingBackend::wrap(
                backend,
                runtime,
                context.local.managed_policy.as_ref(),
            );
            return Self::build(context, backend, SchedulerOptions::default(), None, clock)
                .await
                .map(Arc::new);
        }
        let _ = (domains, clock);
        Self::start(context).await
    }
    /// Supplies real destination validation/intake and the linked environment's relay verifier.
    pub async fn start_with(
        context: &crate::services::DomainContext<'_>,
        backend: Arc<dyn Orchestration>,
        options: SchedulerOptions,
        relay: Option<RelayDeliveryProof>,
    ) -> crate::Result<Arc<Self>> {
        Self::build(context, backend, options, relay, None)
            .await
            .map(Arc::new)
    }
    async fn build(
        context: &crate::services::DomainContext<'_>,
        backend: Arc<dyn Orchestration>,
        options: SchedulerOptions,
        relay: Option<RelayDeliveryProof>,
        clock: Option<Arc<dyn Clock>>,
    ) -> crate::Result<Self> {
        let clock: Arc<dyn Clock> = clock.unwrap_or_else(|| Arc::new(SystemClock));
        let service =
            ScheduledTaskService::new(context.database.clone(), clock.clone(), backend, options)
                .await
                .map_err(|error| crate::Error::Service(error.to_string()))?;
        Ok(Self::new(Arc::new(service), clock, relay))
    }
}

/// `RuntimeTaskIntake` over the one orchestration runtime (recipe in `server-seam-requests.md`,
/// "t3-scheduler runtime-intake server assembly"). TypeScript: `ScheduledTaskService.ts`
/// `sendBound`, `hasQueuedOccurrence` and `recordRunFailure`.
#[cfg(all(feature = "local-services", feature = "orchestration"))]
mod runtime_intake {
    use std::{sync::Arc, time::Duration};
    use t3_contracts::{
        base_schemas::{MessageId, ThreadId},
        orchestration_v2::{OrchestrationV2RunStatus, OrchestrationV2ThreadProjection},
        scheduled_task::ScheduledTask,
    };
    use t3_orchestration::{
        api::{MessageFilter, ProjectionReader, ProjectionRecordQuery, RecordFilter, RecordKind},
        composition::OrchestrationRuntime,
        error::Error as OrchestrationError,
        ports::{
            runs_ingestion::provider_event_ingestor::expanded_id,
            thread_project::thread_management_service::{SendInput, SendMode, WaitInput},
        },
    };
    use t3_scheduler::{
        DispatchReceipt, Error, Result,
        intake::{BoundTaskMessage, BoundTaskThread, IntakeError, RuntimeTaskIntake, TaskRunMode},
    };
    use tokio_util::sync::CancellationToken;

    /// `RUN_OUTCOME_WAIT_MS` in TypeScript.
    const RUN_OUTCOME_WAIT: Duration = Duration::from_secs(24 * 60 * 60);

    pub(super) struct RuntimeIntake {
        pub runtime: Arc<OrchestrationRuntime>,
        pub shutdown: CancellationToken,
    }

    fn read_failure(_: OrchestrationError) -> Error {
        Error::Dispatch("Could not read the destination thread.".into())
    }

    impl RuntimeIntake {
        async fn records(
            &self,
            thread_id: &ThreadId,
            fields: Vec<RecordKind>,
            messages: MessageFilter,
            run_statuses: Option<Vec<OrchestrationV2RunStatus>>,
        ) -> Result<OrchestrationV2ThreadProjection> {
            self.runtime
                .threads
                .query_records(ProjectionRecordQuery {
                    thread_id: thread_id.clone(),
                    fields,
                    filter: RecordFilter::default(),
                    messages,
                    run_statuses,
                    include_latest_run: false,
                })
                .await
                .map_err(read_failure)
        }
    }

    #[async_trait::async_trait]
    impl RuntimeTaskIntake for RuntimeIntake {
        /// TypeScript's `upsert` and `setEnabled` check only a changed bound destination, which
        /// the backend does; an unbound draft for any project saves disabled and cannot be
        /// enabled or launched while no launch environment is assembled (`LAUNCH_UNAVAILABLE`).
        async fn validate(&self, _task: &ScheduledTask, _destination_changed: bool) -> Result<()> {
            Ok(())
        }

        async fn thread(&self, id: &ThreadId) -> Result<Option<BoundTaskThread>> {
            let shell = self
                .runtime
                .projections
                .shell(id.clone())
                .await
                .map_err(read_failure)?;
            Ok(shell.map(|shell| BoundTaskThread {
                project_id: shell.project_id,
                modes: TaskRunMode {
                    runtime_mode: shell.runtime_mode,
                    interaction_mode: shell.interaction_mode,
                },
                archived: shell.archived_at.is_some(),
                deleted: shell.deleted_at.is_some(),
            }))
        }

        async fn enqueue(
            &self,
            message: BoundTaskMessage,
        ) -> std::result::Result<DispatchReceipt, IntakeError> {
            let command = message.command;
            let thread_id = command.thread_id.clone();
            let sent = self
                .runtime
                .intake
                .send_to_thread(SendInput {
                    project_id: message.project_id,
                    command_id: command.command_id,
                    thread_id: command.thread_id,
                    message_id: command.message_id,
                    text: command.text,
                    attachments: command.attachments,
                    mode: SendMode::Queue,
                    fresh_context: command.fresh_context.flatten().unwrap_or(false),
                    // The bound thread's own provider and modes govern the run.
                    model_selection: None,
                    scheduled_task_id: command.scheduled_task_id.flatten(),
                    sender_thread_id: None,
                    linked_sender: None,
                    created_by: command.created_by,
                    creation_source: command.creation_source,
                })
                .await;
            match sent {
                Ok(_) => Ok(DispatchReceipt { thread_id }),
                Err(OrchestrationError::ThreadNotFound(_)) => Err(IntakeError::ThreadNotFound),
                // A refusal (such as the managed runtime-mode ceiling) carries TypeScript's text.
                Err(OrchestrationError::Rejected { reason, .. }) => {
                    Err(IntakeError::Other(Error::Dispatch(reason)))
                }
                Err(error) => {
                    tracing::warn!(%error, "scheduled message was not accepted");
                    Err(IntakeError::Other(Error::Dispatch(
                        "The scheduled message could not be dispatched.".into(),
                    )))
                }
            }
        }

        async fn has_queued_occurrence(&self, task: &ScheduledTask) -> Result<bool> {
            let Some(thread_id) = &task.thread_id else {
                return Ok(false);
            };
            let queued = self
                .records(
                    thread_id,
                    vec![RecordKind::Runs],
                    MessageFilter::default(),
                    Some(vec![OrchestrationV2RunStatus::Queued]),
                )
                .await?;
            let ids: Vec<MessageId> = queued
                .runs
                .iter()
                .filter(|run| run.status == OrchestrationV2RunStatus::Queued)
                .filter_map(|run| expanded_id(&run.user_message_id).ok())
                .collect();
            if ids.is_empty() {
                return Ok(false);
            }
            let messages = self
                .records(
                    thread_id,
                    vec![RecordKind::Messages],
                    MessageFilter {
                        ids: Some(ids.clone()),
                        ..Default::default()
                    },
                    None,
                )
                .await?;
            Ok(messages.messages.iter().any(|message| {
                expanded_id(&message.id).is_ok_and(|id| ids.contains(&id))
                    && message
                        .scheduled_task_id
                        .clone()
                        .flatten()
                        .and_then(|id| expanded_id(&id).ok())
                        .is_some_and(|id| id == task.id)
            }))
        }

        async fn wait_for_failure(
            &self,
            receipt: DispatchReceipt,
            message_id: String,
        ) -> Result<Option<String>> {
            let Some(shell) = self
                .runtime
                .projections
                .shell(receipt.thread_id.clone())
                .await
                .map_err(read_failure)?
            else {
                return Ok(None);
            };
            let message_id = MessageId(message_id);
            let runs = self
                .records(
                    &receipt.thread_id,
                    vec![RecordKind::Runs],
                    MessageFilter::default(),
                    None,
                )
                .await?;
            let Some(dispatched) = runs
                .runs
                .iter()
                .find(|run| expanded_id(&run.user_message_id).is_ok_and(|id| id == message_id))
            else {
                return Ok(None);
            };
            let run_id = expanded_id(&dispatched.id).map_err(read_failure)?;
            let waited = self
                .runtime
                .threads
                .wait_for_thread(
                    WaitInput {
                        project_id: shell.project_id.clone(),
                        thread_id: receipt.thread_id.clone(),
                        run_id: Some(run_id.clone()),
                        timeout: RUN_OUTCOME_WAIT,
                        until_delegated_work_settled: false,
                    },
                    self.shutdown.child_token(),
                )
                .await
                .map_err(read_failure)?;
            let Some(run) = waited.run else {
                return Ok(None);
            };
            let fallback = match run.status {
                OrchestrationV2RunStatus::Failed => "The run failed.",
                OrchestrationV2RunStatus::Interrupted => "The run was stopped.",
                OrchestrationV2RunStatus::Cancelled => "The run was cancelled.",
                OrchestrationV2RunStatus::RolledBack => "The run was rolled back.",
                _ => return Ok(None),
            };
            let records = self
                .records(
                    &receipt.thread_id,
                    vec![RecordKind::TurnItems, RecordKind::Messages],
                    MessageFilter {
                        ids: Some(vec![message_id.clone()]),
                        ..Default::default()
                    },
                    None,
                )
                .await?;
            // A queued message promoted to a steer cancels its run and joins the active one.
            if run.status == OrchestrationV2RunStatus::Cancelled
                && records
                    .messages
                    .iter()
                    .find(|message| expanded_id(&message.id).is_ok_and(|id| id == message_id))
                    .and_then(|message| message.run_id.as_ref())
                    .and_then(|id| expanded_id(id).ok())
                    .is_none_or(|id| id != run_id)
            {
                return Ok(None);
            }
            let failure = (run.status == OrchestrationV2RunStatus::Failed)
                .then(|| latest_root_failure(&run, &records))
                .flatten();
            Ok(Some(failure.unwrap_or_else(|| fallback.into())))
        }
    }

    /// `latestRootProviderFailure` (`packages/shared/src/orchestrationV2ThreadError.ts`): the
    /// failed root error item with the greatest `updatedAt`, then ordinal, then id.
    fn latest_root_failure(
        run: &t3_contracts::orchestration_v2::OrchestrationV2Run,
        records: &OrchestrationV2ThreadProjection,
    ) -> Option<String> {
        let run = serde_json::to_value(run).ok()?;
        let millis = |value: &serde_json::Value| {
            value
                .as_str()
                .and_then(t3_shared::search_policy::epoch_millis)
        };
        records
            .turn_items
            .iter()
            .filter_map(|item| serde_json::to_value(item).ok())
            .filter(|item| {
                item["type"] == "error"
                    && item["status"] == "failed"
                    && item["runId"] == run["id"]
                    && item["nodeId"] == run["rootNodeId"]
            })
            .max_by(|a, b| {
                millis(&a["updatedAt"])
                    .cmp(&millis(&b["updatedAt"]))
                    .then(a["ordinal"].as_u64().cmp(&b["ordinal"].as_u64()))
                    .then_with(|| a["id"].to_string().cmp(&b["id"].to_string()))
            })
            .and_then(|item| item["failure"]["message"].as_str().map(str::to_owned))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_signature_headers_cannot_select_a_valid_credential() {
        let signature: ScheduledTaskWebhookSignature = serde_json::from_value(
            json!({"header":"X-Signature","encoding":"hex","prefix":"sha256="}),
        )
        .unwrap();
        let valid = HeaderValue::from_static(
            "sha256=8fe02057647cfb1a6d2f74959e1a26e50f6eaa9cfeb78d17d8e5684634dfb1e1",
        );
        for invalid in [
            HeaderValue::from_static("sha256=wrong"),
            HeaderValue::from_bytes(&[0xff]).unwrap(),
        ] {
            for values in [[invalid.clone(), valid.clone()], [valid.clone(), invalid]] {
                let mut headers = axum::http::HeaderMap::new();
                for value in values {
                    headers.append("x-signature", value);
                }
                let normalised = request_headers(&headers).into_iter().collect();
                assert!(!t3_scheduler::webhook::verify_webhook_signature(
                    &signature,
                    "fixture-secret",
                    &normalised,
                    "{\"text\":\"café\"}".as_bytes()
                ));
            }
        }
    }
}
