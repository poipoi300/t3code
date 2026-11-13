#![cfg(feature = "auth")]
#![allow(clippy::unwrap_used)]
mod support;
use support::*;
use t3_cloud::{
    Secret,
    cloud::CredentialIssuer,
    tunnel::{ConnectionAuthenticator, UpgradeCredentials},
};

#[tokio::test(start_paused = true)]
async fn real_auth_issues_bound_one_use_cloud_grants_and_authenticates_rpc_tickets() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.sqlite");
    let db = tokio::task::spawn_blocking(move || t3_persistence::Database::open(path))
        .await
        .unwrap()
        .unwrap();
    let secrets = t3_auth::ServerSecretStore::open(temp.path().join("secrets"))
        .await
        .unwrap();
    let auth = t3_auth::EnvironmentAuth::new(
        db,
        secrets,
        t3_auth::AuthConfig {
            port: 3773,
            host: Some("127.0.0.1".into()),
            environment_id: "env-one".into(),
            state_dir: temp.path().to_string_lossy().into_owned(),
            ..Default::default()
        },
        None,
        t3_auth::clock::TokioClock::at(NOW),
    )
    .await
    .unwrap();
    let issued = auth.issue_cloud_pairing("key-one").await.unwrap();
    assert_eq!(issued.expires_at_ms, NOW + 120_000);
    assert!(
        auth.pairing
            .consume(
                issued.credential.expose(),
                t3_auth::pairing::ConsumePairing {
                    proof_key_thumbprint: Some("wrong-key".into()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    let grant = auth
        .pairing
        .consume(
            issued.credential.expose(),
            t3_auth::pairing::ConsumePairing {
                proof_key_thumbprint: Some("key-one".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(grant.subject, "cloud-connect");
    assert_eq!(grant.scopes, t3_auth::authorization::STANDARD_SCOPES);
    assert_eq!(grant.proof_key_thumbprint.as_deref(), Some("key-one"));
    assert!(
        auth.pairing
            .consume(issued.credential.expose(), Default::default())
            .await
            .is_err()
    );
    let session = auth
        .sessions
        .issue(t3_auth::session::IssueSession {
            subject: Some("cloud-test".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let verified = auth.sessions.verify(&session.token).await.unwrap();
    let ticket = auth
        .issue_websocket_ticket(
            &verified,
            Some(t3_contracts::auth::AuthWebSocketTicketUse::Rpc),
        )
        .await
        .unwrap();
    let credentials = UpgradeCredentials {
        bearer: None,
        dpop: None,
        web_socket_ticket: Some(Secret::new(ticket.ticket.0)),
        method: "GET".into(),
        absolute_url: Secret::new("https://env.test/ws"),
        cookies: Default::default(),
        origin: None,
    };
    let principal = auth.authenticate(&credentials).await.unwrap();
    assert_eq!(principal.session_id, verified.session_id.0);
    auth.sessions.revoke(&verified.session_id).await.unwrap();
    assert!(auth.authenticate(&credentials).await.is_err());
}
