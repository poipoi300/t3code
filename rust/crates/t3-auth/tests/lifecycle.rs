#![allow(clippy::unwrap_used)]
use t3_auth::Request;
mod common;
use common::*;
use t3_auth::error::CredentialFailure as F;
use t3_auth::{
    AuthConfig, Error, authorization::STANDARD_SCOPES, pairing::*, session::*, utils::*,
};
use t3_contracts::{auth::*, base_schemas::AuthSessionId, provider_policy::RuntimeMode};

#[tokio::test]
async fn sessions_derive_authority_from_rows_accept_v2_and_preserve_claim_flags() {
    let h = Harness::new().await;
    let session = h
        .auth
        .sessions
        .issue(IssueSession {
            scopes: Some(vec![
                AuthEnvironmentScope::OrchestrationRead,
                AuthEnvironmentScope::TerminalOperate,
                AuthEnvironmentScope::ReviewWrite,
            ]),
            runtime_mode_ceiling: Some(RuntimeMode::Auto),
            peer_link: true,
            ..Default::default()
        })
        .await
        .unwrap();
    let body = session.token.split('.').next().unwrap();
    let original: serde_json::Value = serde_json::from_slice(&unb64(body).unwrap()).unwrap();
    assert_eq!(original["v"], 1);
    let signer = PayloadSigner::from_store(&h.auth.secrets, "server-signing-key")
        .await
        .unwrap();
    for version in [1, 2] {
        let mut claims = original.clone();
        claims["v"] = version.into();
        claims["scopes"] = serde_json::json!(["access:write"]);
        let body = b64(serde_json::to_vec(&claims).unwrap());
        let token = format!("{body}.{}", signer.sign(&body));
        let verified = h.auth.sessions.verify(&token).await.unwrap();
        assert_eq!(verified.scopes, session.scopes);
        assert_eq!(verified.runtime_mode_ceiling, Some(RuntimeMode::Auto));
        assert!(verified.peer_link);
    }
    assert!(
        h.auth
            .sessions
            .verify(&h.issue("plain").await.token)
            .await
            .unwrap()
            .runtime_mode_ceiling
            .is_none()
    );
    let token = format!("{}.ignored", session.token);
    assert!(h.auth.sessions.verify(&token).await.is_err());
    h.db.writer({
        let id = session.session_id.0.clone();
        move |db| {
            db.execute(
                "UPDATE auth_sessions SET scopes='[\"access:read\"]' WHERE session_id=?1",
                [id],
            )?;
            Ok(())
        }
    })
    .await
    .unwrap();
    assert_eq!(
        h.auth.sessions.verify(&session.token).await.unwrap().scopes,
        vec![AuthEnvironmentScope::AccessRead]
    );
}

#[tokio::test]
async fn expiry_revocation_and_malformed_errors_are_distinct() {
    let h = Harness::new().await;
    let s = h
        .auth
        .sessions
        .issue(IssueSession {
            ttl_ms: Some(1000),
            ..Default::default()
        })
        .await
        .unwrap();
    let ws = h
        .auth
        .sessions
        .issue_websocket_token(&s.session_id, None, None, None)
        .await
        .unwrap();
    credential_failure(
        h.auth.sessions.verify("no-dot").await,
        F::MalformedSessionTokenError,
    );
    credential_failure(
        h.auth
            .sessions
            .verify(&format!("{}.bad", s.token.split('.').next().unwrap()))
            .await,
        F::InvalidSessionTokenSignatureError,
    );
    let signer = PayloadSigner::from_store(&h.auth.secrets, "server-signing-key")
        .await
        .unwrap();
    let body = b64(b"not json");
    credential_failure(
        h.auth
            .sessions
            .verify(&format!("{body}.{}", signer.sign(&body)))
            .await,
        F::InvalidSessionTokenPayloadError,
    );
    h.clock.advance(1000);
    credential_failure(
        h.auth.sessions.verify(&s.token).await,
        F::SessionTokenExpiredError,
    );
    credential_failure(
        h.auth.sessions.verify_websocket_token(&ws.ticket.0).await,
        F::WebSocketSessionExpiredError,
    );
    h.clock.advance(300000);
    credential_failure(
        h.auth.sessions.verify_websocket_token(&ws.ticket.0).await,
        F::WebSocketTokenExpiredError,
    );
    let s = h.issue("revoked").await;
    let ws = h
        .auth
        .sessions
        .issue_websocket_token(&s.session_id, None, None, None)
        .await
        .unwrap();
    assert!(h.auth.sessions.revoke(&s.session_id).await.unwrap());
    assert!(!h.auth.sessions.revoke(&s.session_id).await.unwrap());
    credential_failure(
        h.auth.sessions.verify(&s.token).await,
        F::SessionTokenRevokedError,
    );
    credential_failure(
        h.auth.sessions.verify_websocket_token(&ws.ticket.0).await,
        F::WebSocketSessionRevokedError,
    );
}

#[tokio::test]
async fn connection_counts_last_connected_and_revoking_other_clients() {
    let h = Harness::new().await;
    let owner = h.issue("owner").await;
    let client = h
        .auth
        .sessions
        .issue(IssueSession {
            client: Some(metadata()),
            ttl_ms: Some(500),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut events = h.auth.sessions.subscribe().await;
    h.auth
        .sessions
        .mark_connected(&client.session_id)
        .await
        .unwrap();
    assert!(matches!(events.recv().await.unwrap(),SessionChange::ClientUpserted(c) if c.connected));
    let first = h
        .auth
        .sessions
        .get(&client.session_id)
        .await
        .unwrap()
        .unwrap()
        .last_connected_at
        .unwrap();
    h.clock.advance(100);
    h.auth
        .sessions
        .mark_connected(&client.session_id)
        .await
        .unwrap();
    assert_eq!(
        h.auth
            .sessions
            .get(&client.session_id)
            .await
            .unwrap()
            .unwrap()
            .last_connected_at,
        Some(first.clone())
    );
    h.clock.advance(500);
    assert_eq!(h.auth.sessions.list_active().await.unwrap().len(), 2);
    h.auth
        .sessions
        .mark_disconnected(&client.session_id)
        .await
        .unwrap();
    assert_eq!(h.auth.sessions.list_active().await.unwrap().len(), 2);
    h.auth
        .sessions
        .mark_disconnected(&client.session_id)
        .await
        .unwrap();
    assert_eq!(h.auth.sessions.list_active().await.unwrap().len(), 1);
    let other = h.issue("other").await;
    h.auth
        .sessions
        .mark_connected(&other.session_id)
        .await
        .unwrap();
    assert_eq!(
        h.auth
            .sessions
            .revoke_all_except(&owner.session_id)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        h.auth.sessions.list_active().await.unwrap()[0].session_id,
        owner.session_id
    );
    assert!(
        h.auth
            .revoke_client(&owner.session_id, &owner.session_id)
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replacement_race_and_transaction_rollback() {
    let h = Harness::new().await;
    let browser = h.issue("desktop-bootstrap").await;
    let input = || IssueSession {
        subject: Some("desktop-bootstrap".into()),
        method: Some(ServerAuthSessionMethod::BearerAccessToken),
        replace_active_for_subject_and_method: true,
        ..Default::default()
    };
    let path = h.temp.path().join("state.sqlite");
    let other_db = tokio::task::spawn_blocking(move || t3_persistence::Database::open(path))
        .await
        .unwrap()
        .unwrap();
    let other = SessionStore::new(
        other_db,
        &h.auth.secrets,
        h.auth.config().clone(),
        h.clock.clone(),
    )
    .await
    .unwrap();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let mut tasks = Vec::new();
    for store in [h.auth.sessions.clone(), other] {
        let barrier = barrier.clone();
        let input = input();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            store.issue(input).await
        }));
    }
    let b = tasks.pop().unwrap().await.unwrap();
    let a = tasks.pop().unwrap().await.unwrap();
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(h.auth.sessions.list_active().await.unwrap().len(), 2);
    assert_ne!(
        h.auth.sessions.verify(&a.token).await.is_ok(),
        h.auth.sessions.verify(&b.token).await.is_ok()
    );
    assert!(h.auth.sessions.verify(&browser.token).await.is_ok());
    for operation in ["insert", "revoke"] {
        let h = Harness::new().await;
        let previous = h.issue("previous").await;
        let unrelated = h.issue("previous").await;
        h.db.writer(move |db| { db.execute_batch(if operation=="insert" {"CREATE TRIGGER reject_auth_session_insert BEFORE INSERT ON auth_sessions BEGIN SELECT RAISE(ABORT,'insert failure'); END"} else {"CREATE TRIGGER reject_auth_session_revoke BEFORE UPDATE OF revoked_at ON auth_sessions BEGIN SELECT RAISE(ABORT,'revoke failure'); END"})?; Ok(()) }).await.unwrap();
        assert!(
            h.auth
                .sessions
                .issue(IssueSession {
                    replace_session_id: Some(previous.session_id.clone()),
                    ..Default::default()
                })
                .await
                .is_err()
        );
        assert!(h.auth.sessions.verify(&previous.token).await.is_ok());
        assert!(h.auth.sessions.verify(&unrelated.token).await.is_ok());
        assert_eq!(h.auth.sessions.list_active().await.unwrap().len(), 2);
    }
    let previous = if h.auth.sessions.verify(&a.token).await.is_ok() {
        a
    } else {
        b
    };
    h.db.writer(|db| {db.execute_batch("CREATE TRIGGER reject_insert BEFORE INSERT ON auth_sessions BEGIN SELECT RAISE(ABORT,'insert failure'); END")?; Ok(())}).await.unwrap();
    assert!(h.auth.sessions.issue(input()).await.is_err());
    assert!(h.auth.sessions.verify(&previous.token).await.is_ok());
}

#[tokio::test]
async fn invalidation_observes_revocation_even_before_subscribing_and_all_connections() {
    for operation in ["revoke", "revokeAllExcept", "replace"] {
        let h = Harness::new().await;
        let owner = h.issue("owner").await;
        let session = h
            .auth
            .sessions
            .issue(IssueSession {
                subject: Some("relay".into()),
                proof_key_thumbprint: Some("key".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        let (a, b) = (h.auth.sessions.clone(), h.auth.sessions.clone());
        let (ida, idb) = (session.session_id.clone(), session.session_id.clone());
        let first = tokio::spawn(async move { a.await_invalidation(&ida, Some("key")).await });
        let second = tokio::spawn(async move { b.await_invalidation(&idb, Some("key")).await });
        match operation {
            "revoke" => {
                h.auth.sessions.revoke(&session.session_id).await.unwrap();
            }
            "revokeAllExcept" => {
                h.auth
                    .sessions
                    .revoke_all_except(&owner.session_id)
                    .await
                    .unwrap();
            }
            _ => {
                h.auth
                    .sessions
                    .issue(IssueSession {
                        replace_session_id: Some(session.session_id.clone()),
                        ..Default::default()
                    })
                    .await
                    .unwrap();
            }
        }
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        h.auth
            .sessions
            .await_invalidation(&session.session_id, None)
            .await
            .unwrap();
        assert!(h.auth.sessions.verify(&owner.token).await.is_ok());
    }
}

#[tokio::test]
async fn connected_sessions_expire_and_dpop_renewal_requires_key_subject_and_full_scope_set() {
    for renewal in [
        "same",
        "other-key",
        "fewer-scopes",
        "other-subject",
        "none",
        "revoke-original",
        "revoke-renewal",
        "old-chain",
        "disconnect",
    ] {
        let h = Harness::new().await;
        let input = || IssueSession {
            subject: Some("relay".into()),
            proof_key_thumbprint: Some("key".into()),
            ttl_ms: Some(3600000),
            ..Default::default()
        };
        let original = h.auth.sessions.issue(input()).await.unwrap();
        h.auth
            .sessions
            .mark_connected(&original.session_id)
            .await
            .unwrap();
        let mut watcher = h
            .auth
            .sessions
            .watch_invalidation(&original.session_id, Some("key"))
            .await
            .unwrap();
        assert!(watcher.next_deadline().await.unwrap().is_some());
        h.clock.advance(3540000);
        let mut next = input();
        match renewal {
            "other-key" => next.proof_key_thumbprint = Some("other".into()),
            "fewer-scopes" => next.scopes = Some(vec![AuthEnvironmentScope::OrchestrationRead]),
            "other-subject" => next.subject = Some("other".into()),
            _ => {}
        }
        let issued = if renewal != "none" {
            Some(h.auth.sessions.issue(next).await.unwrap())
        } else {
            None
        };
        h.clock.advance(60000);
        let deadline = watcher.next_deadline().await.unwrap();
        assert_eq!(
            deadline.is_some(),
            matches!(
                renewal,
                "same" | "revoke-original" | "revoke-renewal" | "old-chain" | "disconnect"
            )
        );
        if matches!(
            renewal,
            "same" | "revoke-original" | "revoke-renewal" | "old-chain" | "disconnect"
        ) {
            // A second renewal is committed before the first expires.
            if renewal == "old-chain" {
                h.clock.advance(3480000);
                h.auth.sessions.issue(input()).await.unwrap();
                h.clock.advance(120000);
                assert!(watcher.next_deadline().await.unwrap().is_some());
                h.auth
                    .sessions
                    .revoke(&issued.as_ref().unwrap().session_id)
                    .await
                    .unwrap();
            } else if renewal == "revoke-original" {
                h.auth.sessions.revoke(&original.session_id).await.unwrap();
            } else if renewal == "revoke-renewal" {
                h.auth
                    .sessions
                    .revoke(&issued.as_ref().unwrap().session_id)
                    .await
                    .unwrap();
            } else {
                if renewal == "disconnect" {
                    h.auth
                        .sessions
                        .mark_disconnected(&original.session_id)
                        .await
                        .unwrap();
                }
                h.clock.advance(3540000);
            }
        }
        assert!(watcher.next_deadline().await.unwrap().is_none());
        watcher.wait().await.unwrap();
    }
}

#[tokio::test]
async fn reusable_dev_credential_is_local_persistent_revocable_and_rotatable() {
    let config = AuthConfig {
        dev_url: Some("http://localhost:5173".into()),
        dev_auth_token: Some("token-a".into()),
        ..Default::default()
    };
    let a = Harness::config(config.clone()).await;
    let b = Harness::config(config).await;
    let dev = a.auth.sessions.verify("token-a").await.unwrap();
    let bdev = b.auth.sessions.verify("token-a").await.unwrap();
    assert_eq!(dev.session_id, bdev.session_id);
    let local = a.issue("local-a").await;
    let ticket = a
        .auth
        .sessions
        .issue_websocket_token(&local.session_id, None, None, None)
        .await
        .unwrap();
    credential_failure(
        b.auth.sessions.verify(&local.token).await,
        F::InvalidSessionTokenSignatureError,
    );
    credential_failure(
        b.auth
            .sessions
            .verify_websocket_token(&ticket.ticket.0)
            .await,
        F::InvalidWebSocketTokenSignatureError,
    );
    a.auth.sessions.revoke(&dev.session_id).await.unwrap();
    let restarted = a.reopen(a.auth.config().clone()).await;
    credential_failure(
        restarted.sessions.verify("token-a").await,
        F::SessionTokenRevokedError,
    );
    let old_ticket = b
        .auth
        .sessions
        .issue_websocket_token(&bdev.session_id, None, None, None)
        .await
        .unwrap();
    let mut config = b.auth.config().clone();
    config.dev_auth_token = Some("token-b".into());
    let rotated = b.reopen(config.clone()).await;
    credential_failure(
        rotated.sessions.verify("token-a").await,
        F::MalformedSessionTokenError,
    );
    credential_failure(
        rotated
            .sessions
            .verify_websocket_token(&old_ticket.ticket.0)
            .await,
        F::UnknownWebSocketSessionError,
    );
    assert!(
        rotated
            .sessions
            .list_active()
            .await
            .unwrap()
            .iter()
            .any(|s| s.session_id == bdev.session_id)
    );
    let new = rotated.sessions.verify("token-b").await.unwrap();
    let new_ticket = rotated
        .sessions
        .issue_websocket_token(&new.session_id, None, None, None)
        .await
        .unwrap();
    config.dev_auth_token = None;
    let removed = b.reopen(config).await;
    credential_failure(
        removed.sessions.verify("token-b").await,
        F::MalformedSessionTokenError,
    );
    credential_failure(
        removed
            .sessions
            .verify_websocket_token(&new_ticket.ticket.0)
            .await,
        F::UnknownWebSocketSessionError,
    );
    let inactive = Harness::config(AuthConfig {
        desktop: true,
        dev_url: Some("http://localhost:1".into()),
        dev_auth_token: Some("token-a".into()),
        ..Default::default()
    })
    .await;
    assert!(inactive.auth.sessions.dev_auth().is_none());
}

#[tokio::test]
async fn pairing_is_case_insensitive_single_use_and_race_safe_across_independent_stores() {
    let h = Harness::new().await;
    let mut events = h.auth.pairing.subscribe().await;
    let issued = h
        .auth
        .pairing
        .issue_one_time_token(
            t3_auth::authorization::STANDARD_SCOPES,
            IssuePairing {
                scopes: t3_auth::authorization::STANDARD_SCOPES.to_vec(),
                label: Some("phone".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(issued.credential.0.len(), 12);
    assert!(
        issued
            .credential
            .0
            .bytes()
            .all(|b| PAIRING_ALPHABET.contains(&b))
    );
    let event = events.recv().await.unwrap();
    match event {
        PairingChange::PairingLinkUpserted(link) => {
            assert_eq!(link.id, issued.id);
            assert!(
                serde_json::to_value(link)
                    .unwrap()
                    .get("credential")
                    .is_none()
            );
        }
        _ => panic!(),
    }
    let grant = h
        .auth
        .pairing
        .consume(
            &issued.credential.0.to_lowercase(),
            ConsumePairing::default(),
        )
        .await
        .unwrap();
    assert_eq!(grant.scopes, STANDARD_SCOPES);
    assert_eq!(grant.label.as_deref(), Some("phone"));
    credential_failure(
        h.auth
            .pairing
            .consume(&issued.credential.0, ConsumePairing::default())
            .await,
        F::UnknownBootstrapCredentialError,
    );
    assert!(
        matches!(events.recv().await,Some(PairingChange::PairingLinkRemoved(id)) if id==issued.id.0)
    );
    let issued = h
        .auth
        .pairing
        .issue_one_time_token(
            t3_auth::authorization::STANDARD_SCOPES,
            IssuePairing {
                scopes: t3_auth::authorization::STANDARD_SCOPES.to_vec(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let other_db = {
        let path = h.temp.path().join("state.sqlite");
        tokio::task::spawn_blocking(move || t3_persistence::Database::open(path))
            .await
            .unwrap()
            .unwrap()
    };
    let other = t3_auth::PairingGrantStore::new(other_db, h.auth.config().clone(), h.clock.clone())
        .unwrap();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(8));
    let mut tasks = Vec::new();
    for i in 0..8 {
        let store = if i % 2 == 0 {
            h.auth.pairing.clone()
        } else {
            other.clone()
        };
        let code = issued.credential.0.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            store
                .consume(
                    &code,
                    ConsumePairing {
                        requested_scopes: Some(vec![AuthEnvironmentScope::OrchestrationRead]),
                        ..Default::default()
                    },
                )
                .await
        }));
    }
    let mut successes = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(_) => successes += 1,
            result => credential_failure(result, F::UnknownBootstrapCredentialError),
        }
    }
    assert_eq!(successes, 1);
}

#[tokio::test]
async fn proof_and_scope_rejections_do_not_spend_codes_and_approvals_are_hidden() {
    let h = Harness::new().await;
    let issued = h
        .auth
        .pairing
        .issue_one_time_token(
            t3_auth::authorization::STANDARD_SCOPES,
            IssuePairing {
                proof_key_thumbprint: Some("key".into()),
                scopes: vec![AuthEnvironmentScope::OrchestrationRead],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    for key in [None, Some("other")] {
        credential_failure(
            h.auth
                .pairing
                .consume(
                    &issued.credential.0,
                    ConsumePairing {
                        proof_key_thumbprint: key.map(str::to_owned),
                        requested_scopes: Some(vec![AuthEnvironmentScope::AccessWrite]),
                        ..Default::default()
                    },
                )
                .await,
            F::BootstrapCredentialProofKeyMismatchError,
        );
    }
    credential_failure(
        h.auth
            .pairing
            .consume(
                &issued.credential.0,
                ConsumePairing {
                    proof_key_thumbprint: Some("key".into()),
                    requested_scopes: Some(vec![AuthEnvironmentScope::AccessWrite]),
                    ..Default::default()
                },
            )
            .await,
        F::BootstrapCredentialScopeNotGrantedError,
    );
    assert!(
        h.auth
            .pairing
            .consume(
                &issued.credential.0,
                ConsumePairing {
                    proof_key_thumbprint: Some("key".into()),
                    ..Default::default()
                }
            )
            .await
            .is_ok()
    );
    let mut events = h.auth.pairing.subscribe().await;
    let approval = h
        .auth
        .pairing
        .issue_one_time_token(
            t3_auth::authorization::STANDARD_SCOPES,
            IssuePairing {
                scopes: t3_auth::authorization::STANDARD_SCOPES.to_vec(),
                mcp_approval: Some(AuthMcpClientAccess::Auto),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        h.auth
            .create_browser_session(
                &Request::default(),
                &approval.credential.0,
                metadata(),
                None
            )
            .await
            .is_err()
    );
    assert!(
        h.auth
            .exchange_bootstrap(
                &bootstrap_request(None),
                &approval.credential.0,
                None,
                metadata()
            )
            .await
            .is_err()
    );
    assert!(
        h.auth
            .consume_mcp_approval_code(
                &approval.credential.0,
                AuthMcpClientAccess::ApprovalRequired
            )
            .await
            .is_err()
    );
    assert!(h.auth.list_pairing_links(None).await.unwrap().is_empty());
    assert!(events.try_recv().is_err());
    assert_eq!(
        h.auth
            .consume_mcp_approval_code(
                &approval.credential.0.to_lowercase(),
                AuthMcpClientAccess::Auto
            )
            .await
            .unwrap(),
        AuthMcpClientAccess::Auto
    );
    assert!(events.try_recv().is_err());
    let expired = h
        .auth
        .pairing
        .issue_one_time_token(
            t3_auth::authorization::STANDARD_SCOPES,
            IssuePairing {
                scopes: t3_auth::authorization::STANDARD_SCOPES.to_vec(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    h.clock.advance(300000);
    credential_failure(
        h.auth
            .pairing
            .consume(&expired.credential.0, ConsumePairing::default())
            .await,
        F::ExpiredBootstrapCredentialError,
    );
}

#[tokio::test]
async fn desktop_seeds_are_exact_reusable_and_rotating_secrets_replace_old_seeds() {
    let h = Harness::config(AuthConfig {
        desktop_bootstrap_token: Some("abcdefghjkmn".into()),
        ..Default::default()
    })
    .await;
    for _ in 0..2 {
        assert_eq!(
            h.auth
                .pairing
                .consume("abcdefghjkmn", ConsumePairing::default())
                .await
                .unwrap()
                .method,
            ServerAuthBootstrapMethod::DesktopBootstrap
        );
    }
    credential_failure(
        h.auth
            .pairing
            .consume("ABCDEFGHJKMN", ConsumePairing::default())
            .await,
        F::UnknownBootstrapCredentialError,
    );
    h.clock.advance(12 * 3600000);
    assert!(
        h.auth
            .pairing
            .consume("abcdefghjkmn", ConsumePairing::default())
            .await
            .is_ok()
    );
    h.clock.advance(13 * 3600000);
    credential_failure(
        h.auth
            .pairing
            .consume("abcdefghjkmn", ConsumePairing::default())
            .await,
        F::ExpiredBootstrapCredentialError,
    );
    credential_failure(
        h.auth
            .pairing
            .consume("abcdefghjkmn", ConsumePairing::default())
            .await,
        F::UnknownBootstrapCredentialError,
    );
    let h = Harness::config(AuthConfig {
        desktop_bootstrap_token: Some("old".into()),
        desktop_bootstrap_secret: Some("secret".into()),
        ..Default::default()
    })
    .await;
    for window in [-1, 0, 1] {
        let token = current_desktop_token("secret", NOW + window * DESKTOP_WINDOW_MS).unwrap();
        assert!(
            h.auth
                .pairing
                .consume(&token, ConsumePairing::default())
                .await
                .is_ok()
        );
    }
    credential_failure(
        h.auth
            .pairing
            .consume(
                &current_desktop_token("secret", NOW - 2 * DESKTOP_WINDOW_MS).unwrap(),
                ConsumePairing::default(),
            )
            .await,
        F::UnknownBootstrapCredentialError,
    );
    credential_failure(
        h.auth
            .pairing
            .consume("old", ConsumePairing::default())
            .await,
        F::UnknownBootstrapCredentialError,
    );
}

#[tokio::test]
async fn unknown_session_and_ticket_ids_remain_typed() {
    let h = Harness::new().await;
    let ticket = h
        .auth
        .sessions
        .issue_websocket_token(&AuthSessionId("unknown".into()), None, None, None)
        .await
        .unwrap();
    credential_failure(
        h.auth
            .sessions
            .verify_websocket_token(&ticket.ticket.0)
            .await,
        F::UnknownWebSocketSessionError,
    );
    assert!(matches!(
        h.auth.sessions.verify("bad").await,
        Err(Error::Credential { .. })
    ));
}

#[tokio::test]
async fn bearer_verification_trusts_signed_expiry_but_database_decode_failures_stay_internal() {
    let h = Harness::new().await;
    let session = h.issue("expiry").await;
    h.db.writer({let id=session.session_id.0.clone(); move |db| {db.execute("UPDATE auth_sessions SET expires_at='2000-01-01T00:00:00.000Z' WHERE session_id=?1",[id])?; Ok(())}}).await.unwrap();
    assert!(h.auth.sessions.verify(&session.token).await.is_ok());
    assert!(h.auth.sessions.list_active().await.unwrap().is_empty());
    h.db.writer({
        let id = session.session_id.0.clone();
        move |db| {
            db.execute(
                "UPDATE auth_sessions SET issued_at='invalid-date' WHERE session_id=?1",
                [id],
            )?;
            Ok(())
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        h.auth.sessions.verify(&session.token).await,
        Err(Error::Persistence(_))
    ));
}

#[tokio::test]
async fn cancellation_after_sqlite_mutation_started_still_publishes_revocation() {
    let h = Harness::new().await;
    let s = h.issue("cancelled-revoke").await;
    let mut events = h.auth.sessions.subscribe().await;
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    h.db.writer(move |db| {
        let entered=std::cell::RefCell::new(Some(entered_tx));
        db.create_scalar_function("revocation_barrier",0,rusqlite::functions::FunctionFlags::SQLITE_UTF8,move |_| {
            if let Some(tx)=entered.borrow_mut().take() {let _=tx.send(()); release_rx.recv().map_err(|e|rusqlite::Error::UserFunctionError(Box::new(e)))?;} Ok(0)
        })?;
        db.execute_batch("CREATE TRIGGER auth_revocation_barrier BEFORE UPDATE OF revoked_at ON auth_sessions BEGIN SELECT revocation_barrier(); END")?; Ok(())
    }).await.unwrap();
    let store = h.auth.sessions.clone();
    let id = s.session_id.clone();
    let task = tokio::spawn(async move { store.revoke(&id).await });
    entered_rx.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    release_tx.send(()).unwrap();
    let milestone =
        h.db.writer(|db| {
            Ok(db.query_row(
                "SELECT COUNT(*) FROM auth_sessions WHERE revoked_at IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(milestone, 1);
    // The completion event is required even when the transport drops its waiting future.
    assert!(matches!(
        events.recv().await,
        Some(SessionChange::ClientRemoved(_))
    ));
}
