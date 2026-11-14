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
    Failure::Typed(WsScheduledTasksSubscribeError::ScheduledTaskError(
        Box::new(ScheduledTaskError {
            _tag: ScheduledTaskErrorTag::ScheduledTaskError,
            message: error.to_string(),
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
        )
        .await?;
        adapter.commands_available = false;
        Ok(Arc::new(adapter))
    }
    /// Supplies real destination validation/intake and the linked environment's relay verifier.
    pub async fn start_with(
        context: &crate::services::DomainContext<'_>,
        backend: Arc<dyn Orchestration>,
        options: SchedulerOptions,
        relay: Option<RelayDeliveryProof>,
    ) -> crate::Result<Arc<Self>> {
        Self::build(context, backend, options, relay)
            .await
            .map(Arc::new)
    }
    async fn build(
        context: &crate::services::DomainContext<'_>,
        backend: Arc<dyn Orchestration>,
        options: SchedulerOptions,
        relay: Option<RelayDeliveryProof>,
    ) -> crate::Result<Self> {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let service =
            ScheduledTaskService::new(context.database.clone(), clock.clone(), backend, options)
                .await
                .map_err(|error| crate::Error::Service(error.to_string()))?;
        Ok(Self::new(Arc::new(service), clock, relay))
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
