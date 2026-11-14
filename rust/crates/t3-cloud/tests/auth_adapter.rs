#![cfg(feature = "auth")]
#![allow(clippy::unwrap_used)]
mod support;
use serde_json::json;
use support::*;
use t3_cloud::{
    Secret,
    cloud::AuthCredentialIssuer,
    tunnel::{ConnectionAuthenticator, UpgradeCredentials},
};

fn bootstrap_request(key_byte: u8) -> (t3_auth::Request, String) {
    use p256::ecdsa::signature::Signer;
    let key = p256::ecdsa::SigningKey::from_slice(&[key_byte; 32]).unwrap();
    let point = key.verifying_key().to_encoded_point(false);
    let jwk = t3_auth::dpop::PublicJwk {
        kty: "EC".into(),
        crv: "P-256".into(),
        x: t3_auth::utils::b64(point.x().unwrap()),
        y: t3_auth::utils::b64(point.y().unwrap()),
    };
    let thumbprint = t3_auth::dpop::thumbprint(&jwk);
    let url = "https://env.test/oauth/token";
    let header = t3_auth::utils::b64(
        serde_json::to_vec(&json!({"typ":"dpop+jwt","alg":"ES256","jwk":jwk})).unwrap(),
    );
    let claims = t3_auth::utils::b64(
        serde_json::to_vec(
            &json!({"htm":"POST","htu":url,"iat":NOW/1000,"jti":uuid::Uuid::new_v4().to_string()}),
        )
        .unwrap(),
    );
    let input = format!("{header}.{claims}");
    let signature: p256::ecdsa::Signature = key.sign(input.as_bytes());
    let signature = signature.normalize_s().unwrap_or(signature);
    (
        t3_auth::Request {
            method: "POST".into(),
            url: url.into(),
            headers: [(
                "DPoP".into(),
                format!("{input}.{}", t3_auth::utils::b64(signature.to_bytes())),
            )]
            .into(),
            ..Default::default()
        },
        thumbprint,
    )
}

#[tokio::test(start_paused = true)]
async fn minted_credential_requires_approval_and_its_real_dpop_key_to_bootstrap_a_session() {
    let temp = tempfile::tempdir().unwrap();
    let auth = auth(&temp).await;
    let settings = t3_config::settings::SettingsService::new(temp.path().join("settings.json"));
    settings
        .update(&serde_json::from_value(json!({"requireConnectDeviceApproval":true})).unwrap())
        .await
        .unwrap();
    let issuer = AuthCredentialIssuer::new(auth.clone(), settings);
    let (cloud, _, _, _, signer) = cloud().await;
    let (request, key) = bootstrap_request(7);
    let pending = mint(&cloud, &signer, &issuer, &key, NOW).await;
    let metadata =
        || serde_json::from_value(json!({"deviceType":"desktop","label":"Test laptop"})).unwrap();
    let mut no_proof = request.clone();
    no_proof.headers = Default::default();
    assert!(
        auth.exchange_bootstrap(&no_proof, &pending.expose().credential.0, None, metadata())
            .await
            .is_err()
    );
    assert!(
        auth.exchange_bootstrap(
            &bootstrap_request(8).0,
            &pending.expose().credential.0,
            None,
            metadata()
        )
        .await
        .is_err()
    );
    assert!(matches!(
        auth.exchange_bootstrap(&request, &pending.expose().credential.0, None, metadata())
            .await,
        Err(t3_auth::Error::ConnectApprovalPending)
    ));
    assert!(auth.list_connect_devices().await.unwrap().is_empty());
    let requests = auth.list_pairing_links(None).await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(
        auth.approve_connect_device(&requests[0].id.0)
            .await
            .unwrap()
    );
    assert!(
        auth.exchange_bootstrap(&no_proof, &pending.expose().credential.0, None, metadata())
            .await
            .is_err()
    );
    let session = auth
        .exchange_bootstrap(&request, &pending.expose().credential.0, None, metadata())
        .await
        .unwrap();
    let verified = auth.sessions.verify(&session.access_token.0).await.unwrap();
    assert_eq!(verified.proof_key_thumbprint.as_deref(), Some(key.as_str()));
    assert_eq!(verified.subject, "cloud-connect");
    assert_eq!(verified.scopes, t3_auth::authorization::STANDARD_SCOPES);
    assert_eq!(verified.client.label.unwrap().0, "T3 Connect");
    assert_eq!(auth.list_connect_devices().await.unwrap().len(), 1);
    assert!(
        auth.exchange_bootstrap(&request, &pending.expose().credential.0, None, metadata())
            .await
            .is_err()
    );
    let trusted = mint(&cloud, &signer, &issuer, &key, NOW).await;
    assert_eq!(
        trusted.expose().expires_at.0,
        chrono::DateTime::from_timestamp_millis(NOW + 120_000)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    );
    assert!(
        auth.exchange_bootstrap(&request, &trusted.expose().credential.0, None, metadata())
            .await
            .is_ok()
    );
}

#[tokio::test(start_paused = true)]
async fn settings_read_failure_cannot_mint_or_fall_back_to_disabled_approval() {
    let temp = tempfile::tempdir().unwrap();
    let auth = auth(&temp).await;
    let settings_path = temp.path().join("marker-settings-path");
    std::fs::create_dir(&settings_path).unwrap();
    let settings = t3_config::settings::SettingsService::new(&settings_path);
    let issuer = AuthCredentialIssuer::new(auth.clone(), settings);
    let (cloud, _, _, _, signer) = cloud().await;
    let proof = signer.sign(t3_cloud::crypto::MINT_REQUEST,&json!({
        "iss":"https://issuer.test","aud":"t3-env:env-one","sub":"user-one",
        "iat":NOW/1000,"exp":NOW/1000+120,"jti":"settings-failure-jti","nonce":"settings-failure-nonce",
        "environmentId":"env-one","scope":["environment:connect"],"clientProofKeyThumbprint":"new-key","cnf":{"jkt":"new-key"}
    })).unwrap();
    let error = cloud.mint(proof, &issuer).await.unwrap_err();
    assert!(matches!(error, t3_cloud::Error::Storage));
    assert!(!format!("{error:?} {error}").contains("marker-settings-path"));
    let response = t3_cloud::api::HttpFailure::from(error);
    assert_eq!(response.status, 500);
    assert!(!format!("{response:?}").contains("marker-settings-path"));
    assert!(auth.list_pairing_links(None).await.unwrap().is_empty());
    assert!(auth.list_connect_devices().await.unwrap().is_empty());
    std::fs::remove_dir(&settings_path).unwrap();
    std::fs::write(&settings_path, r#"{"requireConnectDeviceApproval":true}"#).unwrap();
    let pending = mint(&cloud, &signer, &issuer, "new-key", NOW).await;
    assert!(matches!(
        auth.pairing
            .consume(
                &pending.expose().credential.0,
                t3_auth::pairing::ConsumePairing {
                    proof_key_thumbprint: Some("new-key".into()),
                    ..Default::default()
                }
            )
            .await,
        Err(t3_auth::Error::BootstrapConnectApprovalPending { .. })
    ));
}

async fn mint(
    cloud: &t3_cloud::cloud::CloudLink,
    signer: &t3_cloud::crypto::EnvironmentKey,
    issuer: &AuthCredentialIssuer,
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
    auth_at(temp, NOW).await
}

#[tokio::test(start_paused = true)]
async fn mint_uses_auth_expiry_when_the_auth_and_relay_proof_clocks_differ() {
    for offset in [1, 5_000, -5_000] {
        let temp = tempfile::tempdir().unwrap();
        let auth = auth_at(&temp, NOW + offset).await;
        let settings = t3_config::settings::SettingsService::new(temp.path().join("settings.json"));
        settings
            .update(&serde_json::from_value(json!({"requireConnectDeviceApproval":true})).unwrap())
            .await
            .unwrap();
        let issuer = t3_cloud::cloud::AuthCredentialIssuer::new(auth.clone(), settings);
        let (cloud, _, _, _, signer) = cloud().await;
        let answer = mint(&cloud, &signer, &issuer, "different-clock-key", NOW).await;
        assert!(matches!(
            auth.pairing
                .consume(
                    &answer.expose().credential.0,
                    t3_auth::pairing::ConsumePairing {
                        proof_key_thumbprint: Some("different-clock-key".into()),
                        ..Default::default()
                    }
                )
                .await,
            Err(t3_auth::Error::BootstrapConnectApprovalPending { .. })
        ));
        assert_eq!(
            answer.expose().expires_at.0,
            chrono::DateTime::from_timestamp_millis(NOW + offset + 600_000)
                .unwrap()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        );
    }
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
