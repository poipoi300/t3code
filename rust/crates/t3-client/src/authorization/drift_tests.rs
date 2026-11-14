use super::{tests::*, *};
use crate::connection::{ConnectionHop, ConnectionVia, FailureCause, FailureKind};
use crate::platform::HostReply;
use futures::executor::block_on;
use serde_json::json;

#[test]
fn pending_connect_approval_carries_host_code_and_trace_without_becoming_a_denial() {
    block_on(async {
        let platform = Rc::new(TestPlatform::default());
        let service = service(platform.clone(), Rc::default());
        platform
            .fake
            .push_host(Ok(HostReply::Text(Some(jwt("account")))));
        platform.fake.push_host(Ok(HostReply::Text(None)));
        platform.respond(200, relay_access());
        platform.respond(200, json!({"environmentId":"env","endpoint":endpoint(),"credential":"MARKER_BOOTSTRAP","expiresAt":"2036-01-01T00:00:00.000Z"}));
        platform.respond(200, descriptor("env"));
        platform.respond(
            403,
            json!({
                "_tag":"EnvironmentConnectApprovalPendingError",
                "code":"connect_approval_pending", "traceId":"approval-trace"
            }),
        );
        let error = service
            .authorize_dpop_http(&identity(), None)
            .await
            .unwrap_err();
        let failure = crate::connection::map_remote_dpop_environment_error(&error);
        assert_eq!(failure.kind, FailureKind::ApprovalPending);
        assert_eq!(
            failure.detail,
            "Approve this device on Host. It shows code 6YL-."
        );
        assert_eq!(failure.verification_code.as_deref(), Some("6YL-"));
        assert_eq!(failure.trace_id.as_deref(), Some("approval-trace"));
        assert_eq!(failure.hop, Some(ConnectionHop::Server));
        assert_eq!(failure.via, Some(ConnectionVia::Relay));
        assert_eq!(failure.cause, Some(FailureCause::ApprovalPending));
        assert!(!format!("{error:?} {error}").contains("approval-trace"));
        assert!(
            service
                .store
                .get(&identity().environment)
                .await
                .unwrap()
                .is_none()
        );
        let denied = response_json::<serde_json::Value>(crate::platform::HttpResponse {
            status: 401,
            headers: Default::default(),
            body: serde_json::to_vec(&json!({
                "_tag":"EnvironmentAuthInvalidError", "code":"auth_invalid",
                "reason":"invalid_credential", "traceId":"denied-trace"
            }))
            .unwrap(),
        })
        .unwrap_err();
        assert_eq!(
            connect_approval::map_token_exchange_error(denied, "Host", "key"),
            AuthorizationError::CredentialRejected
        );
    });
}
