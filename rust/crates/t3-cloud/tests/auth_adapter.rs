#![cfg(feature = "auth")]
#![allow(clippy::unwrap_used)]
mod support;
use serde_json::json;
use support::*;
use t3_cloud::{
    Secret,
    cloud::CredentialIssuer,
    tunnel::{ConnectionAuthenticator, UpgradeCredentials},
};

async fn mint(
    cloud: &t3_cloud::cloud::CloudLink,
    signer: &t3_cloud::crypto::EnvironmentKey,
    issuer: &dyn CredentialIssuer,
    key: &str,
    now: i64,
) -> t3_cloud::http::Sensitive<t3_contracts::relay::RelayEnvironmentMintResponse> {
    let unique = uuid::Uuid::new_v4().to_string();
    let proof = signer
        .sign(
            t3_cloud::crypto::MINT_REQUEST,
            &json!({
                "iss":"https://issuer.test","aud":"t3-env:env-one","sub":"user-one",
                "jti":unique,"nonce":unique,"iat":now/1000,"exp":now/1000+120,
                "environmentId":"env-one","scope":["environment:connect"],
                "clientProofKeyThumbprint":key,"cnf":{"jkt":key}
            }),
        )
        .unwrap();
    let response = cloud.mint(proof, issuer).await.unwrap();
    let verified: t3_contracts::relay::RelayEnvironmentMintResponseProofPayload =
        t3_cloud::crypto::verify(
            &cloud.signing_key.public_key,
            &Secret::new(&response.expose().proof.0),
            t3_cloud::crypto::MINT_RESPONSE,
            "t3-env:env-one",
            "https://issuer.test",
            now / 1000,
        )
        .unwrap();
    assert_eq!(verified.client_proof_key_thumbprint.0, key);
    assert_eq!(verified.credential, response.expose().credential);
    assert!(!format!("{response:?}").contains(&response.expose().credential.0));
    response
}

#[tokio::test(start_paused = true)]
async fn signed_mints_obey_live_approval_trust_and_denial_until_expiry() {
    let temp = tempfile::tempdir().unwrap();
    let auth = auth(&temp).await;
    let settings = t3_config::settings::SettingsService::new(temp.path().join("settings.json"));
    settings
        .update(&serde_json::from_value(json!({"requireConnectDeviceApproval":true})).unwrap())
        .await
        .unwrap();
    let issuer = t3_cloud::cloud::AuthCredentialIssuer::new(auth.clone(), settings.clone());
    let (cloud, _, _, time, signer) = cloud().await;
    let pending = mint(&cloud, &signer, &issuer, "new-key", NOW).await;
    assert_eq!(
        pending.expose().expires_at.0,
        chrono::DateTime::from_timestamp_millis(NOW + 600_000)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    );
    let consume = || t3_auth::pairing::ConsumePairing {
        proof_key_thumbprint: Some("new-key".into()),
        ..Default::default()
    };
    assert!(matches!(
        auth.pairing
            .consume(&pending.expose().credential.0, consume())
            .await,
        Err(t3_auth::Error::BootstrapConnectApprovalPending { .. })
    ));
    assert!(auth.list_connect_devices().await.unwrap().is_empty());
    let requests = auth.list_pairing_links(None).await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].connect_device_request.is_some());
    assert!(auth.pairing.revoke(&requests[0].id.0).await.unwrap());
    let denied = mint(&cloud, &signer, &issuer, "new-key", NOW).await;
    assert_eq!(pending.expose().credential, denied.expose().credential);
    assert!(
        auth.pairing
            .consume(&denied.expose().credential.0, consume())
            .await
            .is_err()
    );
    assert!(
        !auth
            .approve_connect_device(&requests[0].id.0)
            .await
            .unwrap()
    );
    tokio::time::advance(std::time::Duration::from_secs(601)).await;
    time.advance(601_000);
    let renewed = mint(&cloud, &signer, &issuer, "new-key", NOW + 601_000).await;
    assert_ne!(pending.expose().credential, renewed.expose().credential);
    assert!(matches!(
        auth.pairing
            .consume(&renewed.expose().credential.0, consume())
            .await,
        Err(t3_auth::Error::BootstrapConnectApprovalPending { .. })
    ));

    auth.pairing
        .trust_connect_device(
            "trusted-key",
            serde_json::from_value(json!({"deviceType":"unknown"})).unwrap(),
        )
        .await
        .unwrap();
    let trusted = mint(&cloud, &signer, &issuer, "trusted-key", NOW + 601_000).await;
    let trusted_grant = auth
        .pairing
        .consume(
            &trusted.expose().credential.0,
            t3_auth::pairing::ConsumePairing {
                proof_key_thumbprint: Some("trusted-key".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(trusted_grant.label.as_deref(), Some("T3 Connect"));
    assert_eq!(
        trusted_grant.expires_at.0,
        chrono::DateTime::from_timestamp_millis(NOW + 721_000)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    );

    settings
        .update(&serde_json::from_value(json!({"requireConnectDeviceApproval":false})).unwrap())
        .await
        .unwrap();
    let allowed = mint(&cloud, &signer, &issuer, "setting-off-key", NOW + 601_000).await;
    assert!(
        auth.pairing
            .consume(
                &allowed.expose().credential.0,
                t3_auth::pairing::ConsumePairing {
                    proof_key_thumbprint: Some("setting-off-key".into()),
                    ..Default::default()
                }
            )
            .await
            .is_ok()
    );
    settings
        .update(&serde_json::from_value(json!({"requireConnectDeviceApproval":true})).unwrap())
        .await
        .unwrap();
    let waiting = mint(
        &cloud,
        &signer,
        &issuer,
        "setting-back-on-key",
        NOW + 601_000,
    )
    .await;
    assert!(matches!(
        auth.pairing
            .consume(
                &waiting.expose().credential.0,
                t3_auth::pairing::ConsumePairing {
                    proof_key_thumbprint: Some("setting-back-on-key".into()),
                    ..Default::default()
                }
            )
            .await,
        Err(t3_auth::Error::BootstrapConnectApprovalPending { .. })
    ));
}

async fn auth(temp: &tempfile::TempDir) -> t3_auth::EnvironmentAuth {
    let path = temp.path().join("state.sqlite");
    let db = tokio::task::spawn_blocking(move || t3_persistence::Database::open(path))
        .await
        .unwrap()
        .unwrap();
    let secrets = t3_auth::ServerSecretStore::open(temp.path().join("secrets"))
        .await
        .unwrap();
    t3_auth::EnvironmentAuth::new(
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
    .unwrap()
}

#[tokio::test(start_paused = true)]
async fn real_auth_issues_bound_one_use_cloud_grants_and_authenticates_rpc_tickets() {
    let temp = tempfile::tempdir().unwrap();
    let auth = auth(&temp).await;
    let settings = t3_config::settings::SettingsService::new(temp.path().join("settings.json"));
    settings
        .update(
            &serde_json::from_value(serde_json::json!({"requireConnectDeviceApproval":true}))
                .unwrap(),
        )
        .await
        .unwrap();
    let issuer = t3_cloud::cloud::AuthCredentialIssuer::new(auth.clone(), settings);
    let issued = issuer.issue_cloud_pairing("key-one").await.unwrap();
    assert!(matches!(
        auth.pairing
            .consume(
                issued.credential.expose(),
                t3_auth::pairing::ConsumePairing {
                    proof_key_thumbprint: Some("key-one".into()),
                    ..Default::default()
                }
            )
            .await,
        Err(t3_auth::Error::BootstrapConnectApprovalPending { .. })
    ));
    let pending = auth.list_pairing_links(None).await.unwrap();
    assert!(auth.approve_connect_device(&pending[0].id.0).await.unwrap());
    assert_eq!(issued.expires_at_ms, NOW + 600_000);
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
