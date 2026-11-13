use super::provider_event_ingestor_test::{Harness, decode};
use async_trait::async_trait;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use t3_contracts::provider_instance::ProviderInstanceId;
use t3_orchestration::{
    api::*,
    error::{Error, Result},
    ports::runs_ingestion::runtime_request_service::*,
    provider::*,
    testing::{FakeCall, TEST_TIME},
};

struct Sessions {
    runtime: Option<Arc<dyn SessionRuntime>>,
    gets: AtomicUsize,
}
#[async_trait]
impl SessionDirectory for Sessions {
    async fn get(&self, _session_id: ProviderSessionId) -> Result<Option<Arc<dyn SessionRuntime>>> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        Ok(self.runtime.clone())
    }
    async fn attach(
        &self,
        _instance_id: ProviderInstanceId,
        _input: OpenSession,
    ) -> Result<Arc<dyn SessionRuntime>> {
        Err(Error::Unsupported("unused test attach"))
    }
    async fn detach(&self, _session_id: ProviderSessionId, _revoke_credential: bool) -> Result<()> {
        Err(Error::Unsupported("unused test detach"))
    }
    async fn shutdown(&self) -> Result<()> {
        Err(Error::Unsupported("unused test shutdown"))
    }
}

#[tokio::test]
async fn only_committed_responses_execute_and_feedback_is_preserved() -> Result<()> {
    for status in ["expired", "pending", "resolved"] {
        let h = Harness::new().await?;
        let (adapter, mut calls) = h.fake();
        let sessions = Arc::new(Sessions {
            runtime: Some(adapter.runtime.clone()),
            gets: AtomicUsize::new(0),
        });
        let request = json!({"id":"request:test","nodeId":"node:test","providerTurnId":null,"nativeRequestRef":null,"kind":"command","status":status,"responseCapability":{"type":"live","providerSessionId":"session:test"},"createdAt":TEST_TIME,"resolvedAt":TEST_TIME});
        h.write("runtime-request.updated", request).await?;
        let service = RuntimeRequestService {
            projections: h.reader.clone(),
            sessions: sessions.clone(),
        };
        let response = RuntimeResponse {
            request_id: RuntimeRequestId("request:test".into()),
            decision: Some(decode(json!("decline"))),
            feedback: Some("Use an explicit path.".into()),
            answers: None,
        };
        let result = service
            .respond(
                ThreadId("thread:test".into()),
                ProviderSessionId("session:test".into()),
                response,
            )
            .await;
        if status == "resolved" {
            result.unwrap();
            let Some(FakeCall::Respond(response)) = calls.recv().await else {
                panic!("response")
            };
            assert_eq!(response.request_id.0, "request:test");
            assert_eq!(response.feedback.as_deref(), Some("Use an explicit path."));
            assert_eq!(serde_json::to_value(response.decision)?, json!("decline"));
            assert_eq!(sessions.gets.load(Ordering::SeqCst), 1);
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.reason, ResponseFailureReason::RequestNotReady);
            assert_eq!(
                error.to_string(),
                "Runtime request request:test on thread thread:test is not ready for response execution."
            );
            assert!(error.cause.is_none());
            assert_eq!(sessions.gets.load(Ordering::SeqCst), 0);
            assert!(calls.try_recv().is_err());
        }
    }
    Ok(())
}

#[tokio::test]
async fn validates_request_existence_capability_session_binding_and_session_liveness() -> Result<()>
{
    for (capability, exists, reason) in [
        (
            json!({"type":"message"}),
            true,
            ResponseFailureReason::RequestNotResumable,
        ),
        (
            json!({"type":"live","providerSessionId":"other"}),
            true,
            ResponseFailureReason::RequestNotResumable,
        ),
        (
            json!({"type":"live","providerSessionId":"session:test"}),
            false,
            ResponseFailureReason::ProviderSessionNotActive,
        ),
    ] {
        let h = Harness::new().await?;
        let sessions = Arc::new(Sessions {
            runtime: None,
            gets: AtomicUsize::new(0),
        });
        let service = RuntimeRequestService {
            projections: h.reader.clone(),
            sessions: sessions.clone(),
        };
        let response = RuntimeResponse {
            request_id: RuntimeRequestId("request:test".into()),
            decision: None,
            feedback: None,
            answers: None,
        };
        let missing = service
            .respond(
                ThreadId("thread:test".into()),
                ProviderSessionId("session:test".into()),
                response.clone(),
            )
            .await
            .unwrap_err();
        assert_eq!(missing.reason, ResponseFailureReason::RequestMissing);
        h.write("runtime-request.updated",json!({"id":"request:test","nodeId":"node:test","providerTurnId":null,"nativeRequestRef":null,"kind":"command","status":"resolved","responseCapability":capability,"createdAt":TEST_TIME,"resolvedAt":TEST_TIME})).await?;
        let error = service
            .respond(
                ThreadId("thread:test".into()),
                ProviderSessionId("session:test".into()),
                response,
            )
            .await
            .unwrap_err();
        assert_eq!(error.reason, reason);
        assert_eq!(sessions.gets.load(Ordering::SeqCst), usize::from(!exists));
    }
    Ok(())
}
