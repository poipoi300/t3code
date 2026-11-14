#![allow(clippy::unwrap_used)]
mod common;
use common::*;
use t3_auth::environment::RevokeConnectByCode;

#[test]
fn pending_error_uses_the_canonical_wire_shape() {
    let trace = t3_contracts::base_schemas::TrimmedNonEmptyString("trace-connect".into());
    assert!(
        Error::ScopeNotGranted
            .connect_approval_pending_response(trace.clone())
            .is_none()
    );
    let response = Error::ConnectApprovalPending
        .connect_approval_pending_response(trace)
        .unwrap();
    assert_eq!(
        serde_json::to_value(response).unwrap(),
        serde_json::json!({"_tag":"EnvironmentConnectApprovalPendingError","code":"connect_approval_pending","traceId":"trace-connect"})
    );
}
use t3_auth::{Error, authorization::STANDARD_SCOPES, pairing::*, session::IssueSession};
use t3_contracts::auth::*;

#[tokio::test]
async fn changed_keys_and_client_identity_claims_cannot_inherit_trust() {
    let h = Harness::new().await;
    let key = bootstrap_thumbprint();
    let old = h.auth.issue_connect_credential(&key, false).await.unwrap();
    h.auth
        .exchange_bootstrap(
            &bootstrap_request(Some(key.clone())),
            &old.credential.0,
            None,
            metadata(),
        )
        .await
        .unwrap();
    let another_key = "different-key-with-same-client-label";
    let changed = h
        .auth
        .issue_connect_credential(another_key, true)
        .await
        .unwrap();
    assert!(matches!(
        h.auth
            .exchange_bootstrap(
                &bootstrap_request(Some(key.clone())),
                &changed.credential.0,
                None,
                metadata()
            )
            .await,
        Err(Error::InvalidCredential { .. })
    ));
    assert!(
        !h.auth
            .pairing
            .is_connect_device_trusted(another_key)
            .await
            .unwrap()
    );
    assert_eq!(h.auth.list_connect_devices().await.unwrap().len(), 1);
    assert!(
        h.auth
            .pairing
            .consume(
                &changed.credential.0,
                ConsumePairing {
                    proof_key_thumbprint: Some(another_key.into()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    let first = h
        .auth
        .sessions
        .issue(IssueSession {
            proof_key_thumbprint: Some(key.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    let sibling = h
        .auth
        .sessions
        .issue(IssueSession {
            proof_key_thumbprint: Some(key.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(matches!(
        h.auth
            .revoke_client(&first.session_id, &first.session_id)
            .await,
        Err(Error::SelfRevocation)
    ));
    assert!(
        h.auth
            .revoke_client(&first.session_id, &sibling.session_id)
            .await
            .unwrap()
    );
    assert!(
        h.auth
            .pairing
            .is_connect_device_trusted(&key)
            .await
            .unwrap()
    );
    assert!(h.auth.sessions.verify(&first.token).await.is_ok());
    let mut claims: serde_json::Value = serde_json::from_slice(
        &t3_auth::utils::unb64(first.token.split('.').next().unwrap()).unwrap(),
    )
    .unwrap();
    claims["sid"] = serde_json::json!(sibling.session_id.0);
    let forged = format!(
        "{}.{}",
        t3_auth::utils::b64(serde_json::to_vec(&claims).unwrap()),
        first.token.split('.').nth(1).unwrap()
    );
    assert!(
        h.auth
            .authenticate_http(&token_request(&forged))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn code_ambiguity_never_approves_or_revokes_a_device() {
    let h = Harness::new().await;
    let keys = ["AAAAAAAA-one", "AAAAAAAA-two"];
    let one = h
        .auth
        .issue_connect_credential(keys[0], true)
        .await
        .unwrap();
    let two = h
        .auth
        .issue_connect_credential(keys[1], true)
        .await
        .unwrap();
    let code = connect_device_verification_code(keys[0]);
    assert!(
        !h.auth
            .approve_connect_device_by_code("unknown")
            .await
            .unwrap()
    );
    assert!(!h.auth.approve_connect_device_by_code(&code).await.unwrap());
    assert!(h.auth.list_connect_devices().await.unwrap().is_empty());
    assert_eq!(
        h.auth.revoke_connect_device_by_code(&code).await.unwrap(),
        RevokeConnectByCode::NotFound
    );
    assert!(h.auth.approve_connect_device(&one.id.0).await.unwrap());
    assert!(h.auth.approve_connect_device(&two.id.0).await.unwrap());
    assert_eq!(
        h.auth.revoke_connect_device_by_code(&code).await.unwrap(),
        RevokeConnectByCode::Ambiguous
    );
    assert_eq!(h.auth.list_connect_devices().await.unwrap().len(), 2);
    h.auth.revoke_connect_device(keys[1]).await.unwrap();
    assert_eq!(
        h.auth
            .revoke_connect_device_by_code(&code.replace('-', " ").to_lowercase())
            .await
            .unwrap(),
        RevokeConnectByCode::Revoked
    );
    assert!(h.auth.list_connect_devices().await.unwrap().is_empty());
}

#[tokio::test]
async fn trust_metadata_preserves_first_approval_time_and_optional_client() {
    let h = Harness::new().await;
    let issued = h
        .auth
        .issue_connect_credential("trusted-key", true)
        .await
        .unwrap();
    assert!(h.auth.approve_connect_device(&issued.id.0).await.unwrap());
    let original = h.auth.list_connect_devices().await.unwrap().remove(0);
    assert!(original.client.is_none());
    assert!(
        serde_json::to_value(&original)
            .unwrap()
            .get("client")
            .is_none()
    );
    h.clock.advance(1000);
    h.auth
        .pairing
        .trust_connect_device("trusted-key", metadata())
        .await
        .unwrap();
    let updated = h.auth.list_connect_devices().await.unwrap().remove(0);
    assert_eq!(updated.approved_at, original.approved_at);
    assert_eq!(updated.client, Some(metadata()));
    assert!(h.auth.approve_connect_device(&issued.id.0).await.unwrap());
    assert_eq!(
        h.auth.list_connect_devices().await.unwrap()[0].client,
        Some(metadata())
    );
}

#[tokio::test]
async fn revoking_other_clients_keeps_only_current_device_and_its_unspent_links() {
    let h = Harness::new().await;
    for key in ["current-key", "other-key"] {
        h.auth
            .pairing
            .trust_connect_device(key, metadata())
            .await
            .unwrap();
    }
    let current = h
        .auth
        .sessions
        .issue(IssueSession {
            proof_key_thumbprint: Some("current-key".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let other = h
        .auth
        .sessions
        .issue(IssueSession {
            proof_key_thumbprint: Some("other-key".into()),
            ttl_ms: Some(-1),
            ..Default::default()
        })
        .await
        .unwrap();
    let keep = h
        .auth
        .issue_connect_credential("current-key", true)
        .await
        .unwrap();
    let die = h
        .auth
        .issue_connect_credential("other-key", true)
        .await
        .unwrap();
    let unbound = h
        .auth
        .pairing
        .issue_one_time_token(
            STANDARD_SCOPES,
            IssuePairing {
                scopes: STANDARD_SCOPES.to_vec(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        h.auth
            .revoke_other_client_sessions(&current.session_id)
            .await
            .unwrap(),
        1
    );
    assert!(
        h.auth
            .sessions
            .get(&other.session_id)
            .await
            .unwrap()
            .unwrap()
            .revoked_at
            .is_some()
    );
    assert!(
        h.auth
            .pairing
            .consume(
                &die.credential.0,
                ConsumePairing {
                    proof_key_thumbprint: Some("other-key".into()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    assert!(
        h.auth
            .pairing
            .consume(
                &keep.credential.0,
                ConsumePairing {
                    proof_key_thumbprint: Some("current-key".into()),
                    ..Default::default()
                }
            )
            .await
            .is_ok()
    );
    assert!(
        h.auth
            .pairing
            .consume(&unbound.credential.0, ConsumePairing::default())
            .await
            .is_ok()
    );
    let devices = h.auth.list_connect_devices().await.unwrap();
    assert_eq!(devices.len(), 1);
    assert!(devices[0].proof_key_thumbprint.0 == "current-key");
    let owner = h.issue("owner").await;
    h.auth
        .revoke_other_client_sessions(&owner.session_id)
        .await
        .unwrap();
    assert!(h.auth.list_connect_devices().await.unwrap().is_empty());
}

#[tokio::test]
async fn key_revocation_includes_expired_sessions_and_survives_reopen() {
    let h = Harness::new().await;
    let session = h
        .auth
        .sessions
        .issue(IssueSession {
            proof_key_thumbprint: Some("persisted-key".into()),
            ttl_ms: Some(-1),
            ..Default::default()
        })
        .await
        .unwrap();
    let reopened = h.reopen(h.auth.config().clone()).await;
    assert!(
        reopened
            .sessions
            .proof_key_thumbprint_of(&session.session_id)
            .await
            .unwrap()
            .as_deref()
            == Some("persisted-key")
    );
    assert_eq!(
        reopened
            .sessions
            .revoke_by_proof_key("missing")
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        reopened
            .sessions
            .revoke_by_proof_key("persisted-key")
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        reopened
            .sessions
            .revoke_by_proof_key("persisted-key")
            .await
            .unwrap(),
        0
    );
    assert!(
        h.auth
            .sessions
            .get(&session.session_id)
            .await
            .unwrap()
            .unwrap()
            .revoked_at
            .is_some()
    );
}

#[tokio::test]
async fn connect_lifecycle_refusals_precede_approval_and_reconnection() {
    let h = Harness::new().await;
    let mut events = h.auth.pairing.subscribe().await;
    let key = bootstrap_thumbprint();
    let (one, two) = tokio::join!(
        h.auth.issue_connect_credential(&key, true),
        h.auth.issue_connect_credential(&key, true)
    );
    let one = one.unwrap();
    assert!(
        matches!(events.recv().await, Some(PairingChange::PairingLinkUpserted(link)) if link.connect_device_request.as_ref().is_some_and(|r| r.client.is_none()))
    );
    assert!(
        one.credential == two.unwrap().credential,
        "concurrent mints did not reuse request"
    );
    assert_eq!(
        t3_auth::clock::epoch(&one.expires_at).unwrap(),
        NOW + 600_000
    );
    let mut wrong_proof = bootstrap_request(Some(key.clone()));
    wrong_proof
        .headers
        .insert("dpop".into(), "malformed".into());
    assert!(
        h.auth
            .exchange_bootstrap(&wrong_proof, &one.credential.0, None, metadata())
            .await
            .is_err()
    );
    assert!(
        h.auth
            .exchange_bootstrap(
                &bootstrap_request(None),
                &one.credential.0,
                None,
                metadata()
            )
            .await
            .is_err()
    );
    let request = bootstrap_request(Some(key.clone()));
    assert!(matches!(
        h.auth
            .exchange_bootstrap(&request, &one.credential.0, None, metadata())
            .await,
        Err(Error::ConnectApprovalPending)
    ));
    assert!(
        matches!(events.recv().await, Some(PairingChange::PairingLinkUpserted(link)) if link.connect_device_request.as_ref().is_some_and(|r| r.client == Some(metadata())))
    );
    let links = h.auth.list_pairing_links(None).await.unwrap();
    let link = &links[0];
    let pending = link.connect_device_request.as_ref().unwrap();
    assert!(pending.verification_code.0 == connect_device_verification_code(&key));
    assert_eq!(pending.client, Some(metadata()));
    let mut changed = metadata();
    changed.label = Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
        "Changed claim".into(),
    ));
    assert!(matches!(
        h.auth
            .exchange_bootstrap(&request, &one.credential.0, None, changed)
            .await,
        Err(Error::ConnectApprovalPending)
    ));
    assert_eq!(
        h.auth.list_pairing_links(None).await.unwrap()[0]
            .connect_device_request
            .as_ref()
            .unwrap()
            .client,
        Some(metadata())
    );
    assert!(matches!(
        events.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    assert!(!h.auth.approve_connect_device("unknown").await.unwrap());
    assert!(
        h.auth
            .approve_connect_device_by_code(&pending.verification_code.0.to_lowercase())
            .await
            .unwrap()
    );
    assert!(
        matches!(events.recv().await, Some(PairingChange::PairingLinkRemoved(id)) if id == one.id.0)
    );
    assert!(matches!(
        events.recv().await,
        Some(PairingChange::ConnectDevicesChanged)
    ));
    assert!(h.auth.list_pairing_links(None).await.unwrap().is_empty());
    let first = h
        .auth
        .exchange_bootstrap(&request, &one.credential.0, None, metadata())
        .await
        .unwrap();
    assert!(!h.auth.approve_connect_device(&one.id.0).await.unwrap());
    assert!(
        h.auth
            .exchange_bootstrap(&request, &one.credential.0, None, metadata())
            .await
            .is_err()
    );
    let devices = h.auth.list_connect_devices().await.unwrap();
    assert_eq!(devices.len(), 1);
    assert!(devices[0].proof_key_thumbprint.0 == key);
    let second = h.auth.issue_connect_credential(&key, true).await.unwrap();
    assert_eq!(
        t3_auth::clock::epoch(&second.expires_at).unwrap(),
        NOW + 120_000
    );
    let second = h
        .auth
        .exchange_bootstrap(&request, &second.credential.0, None, metadata())
        .await
        .unwrap();
    let first = h.auth.sessions.verify(&first.access_token.0).await.unwrap();
    let second = h
        .auth
        .sessions
        .verify(&second.access_token.0)
        .await
        .unwrap();
    assert_eq!(second.client.label.unwrap().0, "T3 Connect");
    let mut live = h
        .auth
        .sessions
        .watch_invalidation(&first.session_id, Some(&key))
        .await
        .unwrap();
    h.auth
        .sessions
        .mark_connected(&first.session_id)
        .await
        .unwrap();
    let unspent = h.auth.issue_connect_credential(&key, true).await.unwrap();
    let owner = h.issue("owner").await;
    assert!(
        h.auth
            .revoke_client(&owner.session_id, &second.session_id)
            .await
            .unwrap()
    );
    assert_eq!(live.next_deadline().await.unwrap(), None);
    assert!(h.auth.sessions.verify(&first.token).await.is_err());
    assert!(h.auth.sessions.verify(&second.token).await.is_err());
    assert!(
        h.auth
            .exchange_bootstrap(&request, &unspent.credential.0, None, metadata())
            .await
            .is_err()
    );
    assert!(h.auth.list_connect_devices().await.unwrap().is_empty());
    let denied = h.auth.issue_connect_credential(&key, true).await.unwrap();
    assert!(h.auth.pairing.revoke(&denied.id.0).await.unwrap());
    assert!(!h.auth.approve_connect_device(&denied.id.0).await.unwrap());
    let retried = h.auth.issue_connect_credential(&key, true).await.unwrap();
    assert!(
        denied.credential == retried.credential,
        "denied retry opened a prompt"
    );
    assert!(
        h.auth
            .exchange_bootstrap(&request, &retried.credential.0, None, metadata())
            .await
            .is_err()
    );
    assert!(h.auth.list_pairing_links(None).await.unwrap().is_empty());
    h.clock.advance(600_000);
    assert!(!h.auth.approve_connect_device(&denied.id.0).await.unwrap());
    let fresh = h.auth.issue_connect_credential(&key, true).await.unwrap();
    assert!(
        fresh.credential != denied.credential,
        "expired denial was reused"
    );
    h.clock.advance(600_000);
    assert!(!h.auth.approve_connect_device(&fresh.id.0).await.unwrap());
}

#[tokio::test]
async fn pending_credentials_refuse_wrong_keys_and_unapproved_consumption_without_spending() {
    let h = Harness::new().await;
    let issued = h
        .auth
        .pairing
        .issue_one_time_token(
            STANDARD_SCOPES,
            IssuePairing {
                scopes: STANDARD_SCOPES.to_vec(),
                proof_key_thumbprint: Some(bootstrap_thumbprint()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let id = issued.id.0.clone();
    h.db.writer(move |db| {
        db.execute(
            "UPDATE auth_pairing_links SET requires_connect_approval=1 WHERE id=?1",
            [id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    credential_failure(
        h.auth
            .pairing
            .consume(
                &issued.credential.0,
                ConsumePairing {
                    proof_key_thumbprint: Some("wrong-key".into()),
                    ..Default::default()
                },
            )
            .await,
        t3_auth::error::CredentialFailure::BootstrapCredentialProofKeyMismatchError,
    );
    credential_failure(
        h.auth
            .pairing
            .consume(
                &issued.credential.0,
                ConsumePairing {
                    proof_key_thumbprint: Some(bootstrap_thumbprint()),
                    requested_scopes: Some(vec![]),
                    ..Default::default()
                },
            )
            .await,
        t3_auth::error::CredentialFailure::BootstrapCredentialScopeNotGrantedError,
    );
    assert!(
        h.auth
            .pairing
            .consume(
                &issued.credential.0,
                ConsumePairing {
                    proof_key_thumbprint: Some(bootstrap_thumbprint()),
                    ..Default::default()
                }
            )
            .await
            .is_err(),
        "unapproved credential was consumed"
    );
    assert_eq!(h.auth.pairing.list_active().await.unwrap().len(), 1);
}

#[tokio::test]
async fn missing_approval_columns_fail_closed() {
    let h = Harness::new().await;
    h.db.writer(|db| {
        db.execute(
            "ALTER TABLE auth_pairing_links DROP COLUMN requires_connect_approval",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    assert!(matches!(
        h.auth
            .pairing
            .issue_one_time_token(
                STANDARD_SCOPES,
                IssuePairing {
                    scopes: STANDARD_SCOPES.to_vec(),
                    ..Default::default()
                }
            )
            .await,
        Err(Error::Persistence(_))
    ));
    for statement in [
        "ALTER TABLE auth_pairing_links DROP COLUMN connect_denied",
        "DROP TABLE auth_connect_devices",
        "ALTER TABLE auth_pairing_links DROP COLUMN requested_client_json",
    ] {
        let h = Harness::new().await;
        h.db.writer(move |db| {
            db.execute(statement, [])?;
            Ok(())
        })
        .await
        .unwrap();
        assert!(matches!(
            h.auth.issue_connect_credential("device", true).await,
            Err(Error::Persistence(_))
        ));
    }
    let h = Harness::new().await;
    h.db.writer(|db| {
        db.execute(
            "ALTER TABLE auth_sessions DROP COLUMN proof_key_thumbprint",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    assert!(matches!(
        h.auth.sessions.issue(IssueSession::default()).await,
        Err(Error::Persistence(_))
    ));
}

#[tokio::test]
async fn pending_credentials_cannot_grant_browser_or_mcp_sessions() {
    let h = Harness::new().await;
    let key = bootstrap_thumbprint();
    fn send_future<F: Future + Send>(future: F) -> F {
        future
    }
    let pending = send_future(h.auth.issue_connect_credential(&key, true))
        .await
        .unwrap();
    let request = bootstrap_request(Some(key.clone()));
    assert!(
        h.auth
            .create_browser_session(&request, &pending.credential.0, metadata(), None)
            .await
            .is_err()
    );
    assert!(matches!(
        h.auth
            .consume_mcp_approval_code(&pending.credential.0, AuthMcpClientAccess::Auto)
            .await,
        Err(Error::McpApprovalCode {
            reason: t3_auth::error::McpApprovalReason::UnknownOrUsed
        })
    ));
    assert_eq!(h.auth.pairing.list_active().await.unwrap().len(), 1);
    assert!(h.auth.sessions.list_active().await.unwrap().is_empty());
    assert!(h.auth.list_connect_devices().await.unwrap().is_empty());
    assert!(matches!(
        h.auth
            .exchange_bootstrap(&request, &pending.credential.0, None, metadata())
            .await,
        Err(Error::ConnectApprovalPending)
    ));
    let unbound = h
        .auth
        .pairing
        .issue_one_time_token(
            STANDARD_SCOPES,
            IssuePairing {
                scopes: STANDARD_SCOPES.to_vec(),
                requires_connect_approval: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        h.auth
            .create_browser_session(&request, &unbound.credential.0, metadata(), None)
            .await,
        Err(Error::BootstrapConnectApprovalPending { .. })
    ));
    assert!(matches!(
        h.auth
            .consume_mcp_approval_code(&unbound.credential.0, AuthMcpClientAccess::Auto)
            .await,
        Err(Error::McpApprovalCode {
            reason: t3_auth::error::McpApprovalReason::UnknownOrUsed
        })
    ));
    assert_eq!(h.auth.pairing.list_active().await.unwrap().len(), 2);
    assert!(h.auth.sessions.list_active().await.unwrap().is_empty());
}

#[tokio::test]
async fn dpop_exchange_of_an_ordinary_pairing_link_does_not_trust_the_key() {
    let h = Harness::new().await;
    let key = bootstrap_thumbprint();
    let ordinary = h
        .auth
        .create_pairing_link(
            STANDARD_SCOPES,
            IssuePairing {
                scopes: STANDARD_SCOPES.to_vec(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let session = h
        .auth
        .exchange_bootstrap(
            &bootstrap_request(Some(key.clone())),
            &ordinary.credential.0,
            None,
            metadata(),
        )
        .await
        .unwrap();
    assert_eq!(session.token_type, AuthTokenType::DPoP);
    assert!(
        !h.auth
            .pairing
            .is_connect_device_trusted(&key)
            .await
            .unwrap()
    );
    assert!(h.auth.list_connect_devices().await.unwrap().is_empty());
    let connect = h.auth.issue_connect_credential(&key, true).await.unwrap();
    assert!(matches!(
        h.auth
            .exchange_bootstrap(
                &bootstrap_request(Some(key)),
                &connect.credential.0,
                None,
                metadata()
            )
            .await,
        Err(Error::ConnectApprovalPending)
    ));
}

#[tokio::test]
async fn proof_keys_are_persisted_on_sessions() {
    let h = Harness::new().await;
    let session = h
        .auth
        .sessions
        .issue(IssueSession {
            proof_key_thumbprint: Some(bootstrap_thumbprint()),
            method: Some(ServerAuthSessionMethod::DpopAccessToken),
            ..Default::default()
        })
        .await
        .unwrap();
    let id = session.session_id.0;
    let key: Option<String> =
        h.db.reader(move |db| {
            Ok(db.query_row(
                "SELECT proof_key_thumbprint FROM auth_sessions WHERE session_id=?1",
                [id],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert!(
        key.as_deref() == Some(bootstrap_thumbprint().as_str()),
        "session key was not persisted"
    );
}
