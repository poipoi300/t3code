use crate::{api::*, error::Error, provider::RuntimeResponse};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseFailureReason {
    RequestMissing,
    RequestNotReady,
    RequestNotResumable,
    ProviderSessionNotActive,
    UnexpectedFailure,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}", message = self.message())]
pub struct RuntimeRequestResponseExecutionError {
    pub reason: ResponseFailureReason,
    pub thread_id: ThreadId,
    pub session_id: ProviderSessionId,
    pub request_id: RuntimeRequestId,
    #[source]
    pub cause: Option<Box<Error>>,
}

impl RuntimeRequestResponseExecutionError {
    pub fn message(&self) -> String {
        let request = &self.request_id.0;
        let thread = &self.thread_id.0;
        let session = &self.session_id.0;
        match self.reason {
            ResponseFailureReason::RequestMissing => {
                format!("Runtime request {request} no longer exists on thread {thread}.")
            }
            ResponseFailureReason::RequestNotReady => format!(
                "Runtime request {request} on thread {thread} is not ready for response execution."
            ),
            ResponseFailureReason::RequestNotResumable => format!(
                "Runtime request {request} on thread {thread} is not resumable on provider session {session}."
            ),
            ResponseFailureReason::ProviderSessionNotActive => format!(
                "Provider session {session} is not active for runtime request {request} on thread {thread}."
            ),
            ResponseFailureReason::UnexpectedFailure => format!(
                "Failed to respond to runtime request {request} on thread {thread} via provider session {session}."
            ),
        }
    }
}

pub struct RuntimeRequestService {
    pub projections: Arc<dyn ProjectionReader>,
    pub sessions: Arc<dyn SessionDirectory>,
}

impl RuntimeRequestService {
    pub async fn respond(
        &self,
        thread_id: ThreadId,
        session_id: ProviderSessionId,
        response: RuntimeResponse,
    ) -> std::result::Result<(), RuntimeRequestResponseExecutionError> {
        let request_id = response.request_id.clone();
        let failure = |reason, cause: Option<Error>| RuntimeRequestResponseExecutionError {
            reason,
            cause: cause.map(Box::new),
            thread_id: thread_id.clone(),
            session_id: session_id.clone(),
            request_id: request_id.clone(),
        };
        let request = self
            .projections
            .runtime_request(thread_id.clone(), response.request_id.clone())
            .await
            .map_err(|error| failure(ResponseFailureReason::UnexpectedFailure, Some(error)))?
            .ok_or_else(|| failure(ResponseFailureReason::RequestMissing, None))?;
        if request.status != RuntimeRequestStatus::Resolved {
            return Err(failure(ResponseFailureReason::RequestNotReady, None));
        }
        if !matches!(request.response_capability, RuntimeRequestResponseCapability::Live(ref capability) if capability.provider_session_id == session_id)
        {
            return Err(failure(ResponseFailureReason::RequestNotResumable, None));
        }
        let session = self
            .sessions
            .get(session_id.clone())
            .await
            .map_err(|error| failure(ResponseFailureReason::UnexpectedFailure, Some(error)))?
            .ok_or_else(|| failure(ResponseFailureReason::ProviderSessionNotActive, None))?;
        session.respond(response).await.map_err(|error| {
            failure(ResponseFailureReason::UnexpectedFailure, Some(error.into()))
        })?;
        Ok(())
    }
}
