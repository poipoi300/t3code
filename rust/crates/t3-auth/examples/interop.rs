use p256::ecdsa::{Signature, SigningKey, signature::Signer};
use serde_json::json;
use t3_auth::{
    AuthConfig, EnvironmentAuth, Request, ServerSecretStore,
    clock::SystemClock,
    dpop::{PublicJwk, access_token_hash, thumbprint, verify_request},
    oauth::{McpOAuth, OAuthUrls},
    pairing::{ConsumePairing, IssuePairing, current_desktop_token},
    session::IssueSession,
    utils::{b64, sha256},
};
use t3_contracts::{auth::*, provider_policy::RuntimeMode};
use t3_persistence::Database;

fn proof(
    signer: &SigningKey,
    jwk: &PublicJwk,
    token: &str,
    now: i64,
    jti: &str,
) -> t3_auth::Result<String> {
    let header = b64(serde_json::to_vec(
        &json!({"typ":"dpop+jwt","alg":"ES256","jwk":jwk}),
    )?);
    let payload = b64(serde_json::to_vec(
        &json!({"htm":"GET","htu":"https://example.test/ws","jti":jti,"iat":now/1000,"ath":access_token_hash(token)}),
    )?);
    let body = format!("{header}.{payload}");
    let signature: Signature = signer.sign(body.as_bytes());
    let signature = signature.normalize_s().unwrap_or(signature);
    Ok(format!("{body}.{}", b64(signature.to_bytes())))
}
fn request(proof: &str) -> Request {
    Request {
        method: "GET".into(),
        url: "https://example.test/ws".into(),
        headers: [("DPoP".into(), proof.into())].into(),
        ..Default::default()
    }
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("Expected an absolute temporary directory")?,
    );
    if !directory.is_absolute()
        || !directory
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("t3-auth-interop-"))
    {
        return Err("Requires a dedicated temporary interop directory".into());
    }
    let path = directory.join("state.sqlite");
    let db = tokio::task::spawn_blocking(move || Database::open(path)).await??;
    let input: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join("typescript.json"))?)?;
    let read = |key: &str| input[key].as_str().ok_or("Missing TypeScript credential");
    let high_proof = read("highProof")?;
    assert!(
        t3_auth::dpop::verify_proof(t3_auth::dpop::VerifyProof {
            proof: Some(high_proof),
            method: "GET",
            url: "https://example.test/ws",
            expected_thumbprint: None,
            expected_access_token: Some(read("session")?),
            now_epoch_seconds: chrono::Utc::now().timestamp(),
            max_age_seconds: None
        })
        .is_err()
    );
    let auth = EnvironmentAuth::new(
        db.clone(),
        ServerSecretStore::open(directory.join("secrets")).await?,
        AuthConfig {
            port: 3773,
            state_dir: directory.to_string_lossy().into_owned(),
            environment_id: "interop-environment".into(),
            ..Default::default()
        },
        None,
        SystemClock::new(),
    )
    .await?;
    let read_connect = |key: &str| {
        input["connect"][key]
            .as_str()
            .ok_or("Missing TypeScript Connect fixture")
    };
    if std::env::args().nth(2).as_deref() == Some("recheck") {
        let output: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join("rust.json"))?)?;
        let token = output["session"].as_str().ok_or("Missing session")?;
        let proof = output["tsSpends"].as_str().ok_or("Missing proof")?;
        let mcp = output["mcp"].as_str().ok_or("Missing MCP token")?;
        let connect = &output["connect"];
        let trusted_key = connect["trustedKey"]
            .as_str()
            .ok_or("Missing Rust trusted key")?;
        assert!(!auth.pairing.is_connect_device_trusted(trusted_key).await?);
        for field in ["boundId", "expiredId"] {
            let id = t3_contracts::base_schemas::AuthSessionId(
                connect[field]
                    .as_str()
                    .ok_or("Missing bound session")?
                    .into(),
            );
            let row = auth.sessions.get(&id).await?.ok_or("Missing revoked row")?;
            assert!(row.revoked_at.is_some());
            assert!(row.proof_key_thumbprint.as_deref() == Some(trusted_key));
        }
        assert!(
            auth.pairing
                .consume(
                    connect["unspent"]["credential"]
                        .as_str()
                        .ok_or("Missing unspent credential")?,
                    ConsumePairing {
                        proof_key_thumbprint: Some(trusted_key.into()),
                        ..Default::default()
                    }
                )
                .await
                .is_err()
        );
        let pending_key = read_connect("pendingKey")?;
        assert!(auth.pairing.is_connect_device_trusted(pending_key).await?);
        assert!(
            auth.sessions
                .verify_mcp(mcp, "https://example.test/mcp", &[])
                .await
                .is_err()
        );
        assert!(
            verify_request(
                &auth.secrets,
                &request(proof),
                None,
                None,
                Some(token),
                auth.now_ms()
            )
            .await
            .is_err()
        );
        println!(
            "Rust rejected the TypeScript-spent proof and TypeScript-revoked MCP session after reopening."
        );
        return Ok(());
    }
    for pair in input["connect"]["verificationCodes"]
        .as_array()
        .ok_or("Missing code fixtures")?
    {
        let value = pair[0].as_str().ok_or("Missing code input")?;
        assert!(
            Some(t3_auth::pairing::connect_device_verification_code(value).as_str())
                == pair[1].as_str()
        );
    }
    let pending_key = read_connect("pendingKey")?;
    let pending_id = read_connect("pendingId")?;
    let pending_credential = input["connect"]["pending"]["credential"]
        .as_str()
        .ok_or("Missing TS pending credential")?;
    assert!(
        auth.pairing
            .consume(
                pending_credential,
                ConsumePairing {
                    proof_key_thumbprint: Some("wrong-key".into()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    assert!(
        auth.pairing
            .consume(
                pending_credential,
                ConsumePairing {
                    proof_key_thumbprint: Some(pending_key.into()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    let pending_link = auth
        .pairing
        .list_active()
        .await?
        .into_iter()
        .find(|link| link.id.0 == pending_id)
        .ok_or("Missing TS pending row")?;
    let pending_request = pending_link
        .connect_device_request
        .ok_or("Missing TS request data")?;
    assert!(
        pending_request.verification_code.0
            == t3_auth::pairing::connect_device_verification_code(pending_key)
    );
    let metadata: AuthClientMetadata =
        serde_json::from_value(input["connect"]["clientMetadata"].clone())?;
    assert_eq!(pending_request.client, Some(metadata.clone()));
    assert!(auth.approve_connect_device(pending_id).await?);
    auth.pairing
        .consume(
            pending_credential,
            ConsumePairing {
                proof_key_thumbprint: Some(pending_key.into()),
                ..Default::default()
            },
        )
        .await?;
    assert!(
        auth.pairing
            .consume(
                pending_credential,
                ConsumePairing {
                    proof_key_thumbprint: Some(pending_key.into()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    let denied_key = read_connect("deniedKey")?;
    let denied = auth.issue_connect_credential(denied_key, true).await?;
    assert!(
        Some(denied.credential.0.as_str()) == input["connect"]["denied"]["credential"].as_str()
    );
    assert!(
        auth.pairing
            .consume(
                &denied.credential.0,
                ConsumePairing {
                    proof_key_thumbprint: Some(denied_key.into()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    assert!(!auth.approve_connect_device(&denied.id.0).await?);
    let trusted_key = read_connect("trustedKey")?;
    let trusted = auth
        .list_connect_devices()
        .await?
        .into_iter()
        .find(|device| device.proof_key_thumbprint.0 == trusted_key)
        .ok_or("Missing TS trust")?;
    assert_eq!(trusted.approved_at.0, read_connect("trustedAt")?);
    assert_eq!(trusted.client, Some(metadata));
    let id = t3_contracts::base_schemas::AuthSessionId(read_connect("boundId")?.into());
    assert!(auth.sessions.proof_key_thumbprint_of(&id).await?.as_deref() == Some(trusted_key));
    assert_eq!(auth.sessions.revoke_by_proof_key(trusted_key).await?, 2);
    assert!(auth.revoke_connect_device(trusted_key).await?);
    assert!(auth.sessions.verify(read_connect("bound")?).await.is_err());
    assert!(
        auth.sessions
            .get(&t3_contracts::base_schemas::AuthSessionId(
                read_connect("expiredId")?.into()
            ))
            .await?
            .ok_or("Missing TS expired row")?
            .revoked_at
            .is_some()
    );
    println!(
        "Rust read TypeScript Connect rows, refused wrong/unapproved/spent/denied credentials, approved a request and revoked active plus expired key sessions."
    );
    let ts = auth.sessions.verify(read("session")?).await?;
    let ts_mcp = auth.sessions.verify(read("mcp")?).await?;
    assert_eq!(ts_mcp.subject, "mcp-client");
    assert_eq!(
        ts_mcp.scopes,
        t3_auth::authorization::mcp_scopes(AuthMcpClientAccess::Auto)
    );
    assert!(ts_mcp.oauth_audience.is_none());
    assert!(
        auth.sessions
            .verify_mcp(read("mcp")?, "https://example.test/mcp", &[])
            .await
            .is_err()
    );
    let bound = auth
        .sessions
        .verify_mcp(
            read("mcpBound")?,
            "https://example.test/mcp",
            &[AuthEnvironmentScope::OrchestrationOperate],
        )
        .await?;
    assert_eq!(bound.session_id, ts_mcp.session_id);
    assert!(
        auth.sessions
            .verify_mcp(read("mcpBound")?, "https://other.example/mcp", &[])
            .await
            .is_err()
    );
    auth.sessions.revoke(&ts_mcp.session_id).await?;
    assert!(
        auth.sessions
            .verify_mcp(read("mcpBound")?, "https://example.test/mcp", &[])
            .await
            .is_err()
    );
    println!(
        "Rust accepted the real TypeScript MCP token through generic verification, refused its missing audience on strict verification, and accepted the explicitly extended TypeScript-signed fixture only at its resource."
    );
    assert_eq!(ts.subject, "typescript");
    assert_eq!(ts.scopes, vec![AuthEnvironmentScope::OrchestrationRead]);
    let flags = auth.sessions.verify(read("flags")?).await?;
    assert_eq!(
        flags.proof_key_thumbprint.as_deref(),
        Some(read("thumbprint")?)
    );
    assert_eq!(flags.runtime_mode_ceiling, Some(RuntimeMode::Auto));
    assert!(flags.peer_link);
    let ticket = auth
        .sessions
        .verify_websocket_token(read("ticket")?)
        .await?;
    assert_eq!(
        ticket.proof_key_thumbprint.as_deref(),
        Some(read("thumbprint")?)
    );
    for key in ["expired", "revoked"] {
        assert!(auth.sessions.verify(read(key)?).await.is_err());
    }
    assert!(
        auth.sessions
            .verify(&format!("{}x", read("session")?))
            .await
            .is_err()
    );
    for key in ["expiredTicket", "revokedTicket"] {
        assert!(
            auth.sessions
                .verify_websocket_token(read(key)?)
                .await
                .is_err()
        );
    }
    assert!(
        auth.sessions
            .verify_websocket_token(&format!("{}x", read("ticket")?))
            .await
            .is_err()
    );
    auth.pairing
        .consume(read("pairing")?, ConsumePairing::default())
        .await?;
    assert!(
        auth.pairing
            .consume(read("pairing")?, ConsumePairing::default())
            .await
            .is_err()
    );
    for key in ["expiredPairing", "revokedPairing"] {
        assert!(
            auth.pairing
                .consume(read(key)?, ConsumePairing::default())
                .await
                .is_err()
        );
    }
    auth.pairing
        .consume(
            read("approval")?,
            ConsumePairing {
                mcp_approval: Some(AuthMcpClientAccess::Auto),
                ..Default::default()
            },
        )
        .await?;
    assert!(
        verify_request(
            &auth.secrets,
            &request(read("proof")?),
            None,
            None,
            Some(read("session")?),
            auth.now_ms()
        )
        .await
        .is_err()
    );
    verify_request(
        &auth.secrets,
        &request(read("rustSpends")?),
        None,
        None,
        Some(read("session")?),
        auth.now_ms(),
    )
    .await?;
    let connect_pending_key = "Rust-pending-key-for-interop";
    let connect_denied_key = "Rust-denied-key-for-interop";
    let connect_trusted_key = "Rust-trusted-key-for-interop";
    let connect_metadata: AuthClientMetadata = serde_json::from_value(
        json!({"label":"Rust Connect device","deviceType":"desktop","os":"Windows"}),
    )?;
    let connect_pending = auth
        .issue_connect_credential(connect_pending_key, true)
        .await?;
    auth.pairing
        .record_connect_request_client(&connect_pending.id.0, connect_metadata.clone())
        .await?;
    let connect_denied = auth
        .issue_connect_credential(connect_denied_key, true)
        .await?;
    auth.pairing.revoke(&connect_denied.id.0).await?;
    auth.pairing
        .trust_connect_device(connect_trusted_key, connect_metadata.clone())
        .await?;
    let connect_trusted_at = auth
        .list_connect_devices()
        .await?
        .into_iter()
        .find(|device| device.proof_key_thumbprint.0 == connect_trusted_key)
        .ok_or("Missing Rust trust")?
        .approved_at;
    let connect_bound = auth
        .sessions
        .issue(IssueSession {
            proof_key_thumbprint: Some(connect_trusted_key.into()),
            ..Default::default()
        })
        .await?;
    let connect_expired = auth
        .sessions
        .issue(IssueSession {
            proof_key_thumbprint: Some(connect_trusted_key.into()),
            ttl_ms: Some(-1),
            ..Default::default()
        })
        .await?;
    let connect_unspent = auth
        .issue_connect_credential(connect_trusted_key, true)
        .await?;
    let connect = json!({ "pending":connect_pending, "pendingKey":connect_pending_key, "denied":connect_denied, "deniedKey":connect_denied_key, "trustedKey":connect_trusted_key, "trustedAt":connect_trusted_at, "boundId":connect_bound.session_id, "expiredId":connect_expired.session_id, "unspent":connect_unspent, "clientMetadata":connect_metadata });
    let oauth = McpOAuth::new(auth.clone(), tokio::sync::watch::channel(true).1).await?;
    assert!(oauth.parse_client_id(read("clientId")?).is_some());
    assert!(
        oauth
            .parse_client_id(&format!("{}x", read("clientId")?))
            .is_none()
    );
    let now = auth.now_ms();
    assert!(t3_auth::pairing::valid_desktop_token(
        "desktop-secret",
        read("desktop")?,
        now
    )?);
    let session = auth
        .sessions
        .issue(IssueSession {
            subject: Some("rust".into()),
            method: Some(ServerAuthSessionMethod::BearerAccessToken),
            scopes: Some(vec![AuthEnvironmentScope::OrchestrationRead]),
            ..Default::default()
        })
        .await?;
    let signer = SigningKey::from_slice(&[1u8; 32])?;
    let point = signer.verifying_key().to_encoded_point(false);
    let jwk = PublicJwk {
        kty: "EC".into(),
        crv: "P-256".into(),
        x: b64(point.x().ok_or("missing x")?),
        y: b64(point.y().ok_or("missing y")?),
    };
    let thumb = thumbprint(&jwk);
    let flags = auth
        .sessions
        .issue(IssueSession {
            subject: Some("rust-flags".into()),
            method: Some(ServerAuthSessionMethod::DpopAccessToken),
            scopes: Some(vec![AuthEnvironmentScope::OrchestrationRead]),
            proof_key_thumbprint: Some(thumb.clone()),
            runtime_mode_ceiling: Some(RuntimeMode::Auto),
            peer_link: true,
            ..Default::default()
        })
        .await?;
    let ticket = auth
        .sessions
        .issue_websocket_token(
            &session.session_id,
            None,
            Some(AuthWebSocketTicketUse::Rpc),
            Some(thumb.clone()),
        )
        .await?;
    let expired = auth
        .sessions
        .issue(IssueSession {
            ttl_ms: Some(-1),
            ..Default::default()
        })
        .await?;
    let expired_ticket = auth
        .sessions
        .issue_websocket_token(&session.session_id, Some(-1), None, None)
        .await?;
    let revoked = auth.sessions.issue(IssueSession::default()).await?;
    let revoked_ticket = auth
        .sessions
        .issue_websocket_token(&revoked.session_id, None, None, None)
        .await?;
    auth.sessions.revoke(&revoked.session_id).await?;
    let input = || IssuePairing {
        scopes: t3_auth::authorization::STANDARD_SCOPES.to_vec(),
        ..Default::default()
    };
    let pairing = auth
        .pairing
        .issue_one_time_token(t3_auth::authorization::STANDARD_SCOPES, input())
        .await?;
    let expired_pairing = auth
        .pairing
        .issue_one_time_token(
            t3_auth::authorization::STANDARD_SCOPES,
            IssuePairing {
                ttl_ms: Some(-1),
                ..input()
            },
        )
        .await?;
    let revoked_pairing = auth
        .pairing
        .issue_one_time_token(t3_auth::authorization::STANDARD_SCOPES, input())
        .await?;
    auth.pairing.revoke(&revoked_pairing.id.0).await?;
    let approval = auth
        .pairing
        .issue_one_time_token(
            t3_auth::authorization::STANDARD_SCOPES,
            IssuePairing {
                mcp_approval: Some(AuthMcpClientAccess::Auto),
                ..input()
            },
        )
        .await?;
    let client = oauth.register(serde_json::from_value(
        json!({"redirect_uris":["https://example.test/callback"]}),
    )?)?;
    let mcp_client = oauth.register(serde_json::from_value(json!({
        "redirect_uris":["https://example.test/callback"], "client_name":"Rust MCP client"
    }))?)?;
    let urls = OAuthUrls {
        issuer: "https://example.test".into(),
        resource: "https://example.test/mcp".into(),
    };
    let verifier = "v".repeat(64);
    let authorization = oauth.validate_authorization(&urls, serde_json::from_value(json!({
        "client_id":mcp_client.client_id, "redirect_uri":"https://example.test/callback",
        "response_type":"code", "code_challenge_method":"S256", "code_challenge":b64(sha256(&verifier)), "resource":urls.resource
    }))?).map_err(|_| "MCP authorization failed")?;
    let grant = auth
        .pairing
        .issue_one_time_token(t3_auth::authorization::STANDARD_SCOPES, input())
        .await?;
    let approved = oauth
        .approve(
            &request("unused"),
            &authorization,
            serde_json::from_value(json!({
                "_tag":"pairing-code", "code":grant.credential.0, "access":"auto"
            }))?,
        )
        .await?;
    let code = url::Url::parse(&approved.redirect)?
        .query_pairs()
        .find(|(key, _)| key == "code")
        .ok_or("Missing MCP authorization code")?
        .1
        .into_owned();
    let mcp = oauth.exchange_code(&request("unused"), &urls, serde_json::from_value(json!({
        "grant_type":"authorization_code", "client_id":mcp_client.client_id,
        "redirect_uri":"https://example.test/callback", "code":code, "code_verifier":verifier, "resource":urls.resource
    }))?).await?;
    auth.sessions
        .verify_mcp(
            &mcp.access_token,
            &urls.resource,
            &[AuthEnvironmentScope::OrchestrationOperate],
        )
        .await?;
    let mcp_expired = auth
        .sessions
        .issue(IssueSession {
            mcp_resource: Some(urls.resource.clone()),
            subject: Some("mcp-client".into()),
            method: Some(ServerAuthSessionMethod::BearerAccessToken),
            scopes: Some(t3_auth::authorization::mcp_scopes(
                AuthMcpClientAccess::Auto,
            )),
            ttl_ms: Some(-1),
            ..Default::default()
        })
        .await?;
    let mcp_revoked = auth
        .issue_mcp_session(
            &urls.resource,
            "Revoked client".into(),
            AuthMcpClientAccess::Auto,
            false,
            t3_auth::utils::default_metadata(),
        )
        .await?;
    auth.sessions.revoke(&mcp_revoked.session_id).await?;
    let spent_proof = proof(&signer, &jwk, &session.token, now, "rust-spent")?;
    let ts_spends = proof(&signer, &jwk, &session.token, now, "typescript-spends-rust")?;
    verify_request(
        &auth.secrets,
        &request(&spent_proof),
        None,
        None,
        Some(&session.token),
        now,
    )
    .await?;
    std::fs::write(
        directory.join("rust.json"),
        serde_json::to_vec(&json!({
            "connect":connect,
            "mcp":mcp.access_token,"mcpExpired":mcp_expired.token,"mcpRevoked":mcp_revoked.token,
            "session":session.token,"flags":flags.token,"ticket":ticket.ticket.0,"expired":expired.token,"revoked":revoked.token,
            "expiredTicket":expired_ticket.ticket.0,"revokedTicket":revoked_ticket.ticket.0,"pairing":pairing.credential.0,
            "expiredPairing":expired_pairing.credential.0,"revokedPairing":revoked_pairing.credential.0,"approval":approval.credential.0,
            "clientId":client.client_id,"proof":spent_proof,"tsSpends":ts_spends,"thumbprint":thumb,"desktop":current_desktop_token("desktop-secret",now)?,"now":now
        }))?,
    )?;
    println!(
        "Rust accepted TypeScript credentials and rejected tampering, expiry, revocation, single-use reuse and shared DPoP replays; issued reverse samples."
    );
    Ok(())
}
