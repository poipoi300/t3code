#![allow(clippy::unwrap_used)]
mod common;
use common::*;
use t3_auth::{Error, authorization::STANDARD_SCOPES, pairing::*, session::IssueSession};
use t3_contracts::auth::*;

#[tokio::test]
#[cfg(any())]
async fn connect_lifecycle_refusals_precede_approval_and_reconnection() {
    let h = Harness::new().await;
    let key = bootstrap_thumbprint();
    let (one, two) = tokio::join!(h.auth.issue_connect_credential(&key, true), h.auth.issue_connect_credential(&key, true));
    let one = one.unwrap();
    assert!(one.credential == two.unwrap().credential, "concurrent mints did not reuse request");
    assert_eq!(t3_auth::clock::epoch(&one.expires_at).unwrap(), NOW + 600_000);
    let mut wrong_proof = bootstrap_request(Some(key.clone()));
    wrong_proof.headers.insert("dpop".into(), "malformed".into());
    assert!(h.auth.exchange_bootstrap(&wrong_proof, &one.credential.0, None, metadata()).await.is_err());
    assert!(h.auth.exchange_bootstrap(&bootstrap_request(None), &one.credential.0, None, metadata()).await.is_err());
    let request = bootstrap_request(Some(key.clone()));
    assert!(matches!(h.auth.exchange_bootstrap(&request, &one.credential.0, None, metadata()).await, Err(Error::ConnectApprovalPending)));
    let links = h.auth.list_pairing_links(None).await.unwrap();
    let link = &links[0];
    let pending = link.connect_device_request.as_ref().unwrap();
    assert!(pending.verification_code.0 == connect_device_verification_code(&key));
    assert_eq!(pending.client, Some(metadata()));
    let mut changed = metadata();
    changed.label = Some(t3_contracts::base_schemas::TrimmedNonEmptyString("Changed claim".into()));
    assert!(matches!(h.auth.exchange_bootstrap(&request, &one.credential.0, None, changed).await, Err(Error::ConnectApprovalPending)));
    assert_eq!(h.auth.list_pairing_links(None).await.unwrap()[0].connect_device_request.as_ref().unwrap().client, Some(metadata()));
    assert!(!h.auth.approve_connect_device("unknown").await.unwrap());
    assert!(h.auth.approve_connect_device_by_code(&pending.verification_code.0.to_lowercase()).await.unwrap());
    assert!(h.auth.list_pairing_links(None).await.unwrap().is_empty());
    let first = h.auth.exchange_bootstrap(&request, &one.credential.0, None, metadata()).await.unwrap();
    assert!(h.auth.exchange_bootstrap(&request, &one.credential.0, None, metadata()).await.is_err());
    let devices = h.auth.list_connect_devices().await.unwrap();
    assert_eq!(devices.len(), 1);
    assert!(devices[0].proof_key_thumbprint.0 == key);
    let second = h.auth.issue_connect_credential(&key, true).await.unwrap();
    assert_eq!(t3_auth::clock::epoch(&second.expires_at).unwrap(), NOW + 120_000);
    let second = h.auth.exchange_bootstrap(&request, &second.credential.0, None, metadata()).await.unwrap();
    let first = h.auth.sessions.verify(&first.access_token.0).await.unwrap();
    let second = h.auth.sessions.verify(&second.access_token.0).await.unwrap();
    assert_eq!(second.client.label.unwrap().0, "T3 Connect");
    let mut live = h.auth.sessions.watch_invalidation(&first.session_id, Some(&key)).await.unwrap();
    h.auth.sessions.mark_connected(&first.session_id).await.unwrap();
    let unspent = h.auth.issue_connect_credential(&key, true).await.unwrap();
    let owner = h.issue("owner").await;
    assert!(h.auth.revoke_client(&owner.session_id, &second.session_id).await.unwrap());
    assert_eq!(live.next_deadline().await.unwrap(), None);
    assert!(h.auth.sessions.verify(&first.token).await.is_err());
    assert!(h.auth.sessions.verify(&second.token).await.is_err());
    assert!(h.auth.exchange_bootstrap(&request, &unspent.credential.0, None, metadata()).await.is_err());
    assert!(h.auth.list_connect_devices().await.unwrap().is_empty());
    let denied = h.auth.issue_connect_credential(&key, true).await.unwrap();
    assert!(h.auth.pairing.revoke(&denied.id.0).await.unwrap());
    assert!(!h.auth.approve_connect_device(&denied.id.0).await.unwrap());
    let retried = h.auth.issue_connect_credential(&key, true).await.unwrap();
    assert!(denied.credential == retried.credential, "denied retry opened a prompt");
    assert!(h.auth.exchange_bootstrap(&request, &retried.credential.0, None, metadata()).await.is_err());
    assert!(h.auth.list_pairing_links(None).await.unwrap().is_empty());
    h.clock.advance(600_000);
    assert!(!h.auth.approve_connect_device(&denied.id.0).await.unwrap());
    let fresh = h.auth.issue_connect_credential(&key, true).await.unwrap();
    assert!(fresh.credential != denied.credential, "expired denial was reused");
    h.clock.advance(600_000);
    assert!(!h.auth.approve_connect_device(&fresh.id.0).await.unwrap());
}

#[tokio::test]
async fn pending_credentials_refuse_wrong_keys_and_unapproved_consumption_without_spending() {
    let h = Harness::new().await;
    let issued = h.auth.pairing.issue_one_time_token(STANDARD_SCOPES, IssuePairing {
        scopes: STANDARD_SCOPES.to_vec(),
        proof_key_thumbprint: Some(bootstrap_thumbprint()),
        ..Default::default()
    }).await.unwrap();
    let id = issued.id.0.clone();
    h.db.writer(move |db| {
        db.execute("UPDATE auth_pairing_links SET requires_connect_approval=1 WHERE id=?1", [id])?;
        Ok(())
    }).await.unwrap();
    credential_failure(h.auth.pairing.consume(&issued.credential.0, ConsumePairing {
        proof_key_thumbprint: Some("wrong-key".into()), ..Default::default()
    }).await, t3_auth::error::CredentialFailure::BootstrapCredentialProofKeyMismatchError);
    assert!(h.auth.pairing.consume(&issued.credential.0, ConsumePairing {
        proof_key_thumbprint: Some(bootstrap_thumbprint()), ..Default::default()
    }).await.is_err(), "unapproved credential was consumed");
    assert_eq!(h.auth.pairing.list_active().await.unwrap().len(), 1);
}

#[tokio::test]
async fn missing_approval_columns_fail_closed() {
    let h = Harness::new().await;
    h.db.writer(|db| {
        db.execute("ALTER TABLE auth_pairing_links DROP COLUMN requires_connect_approval", [])?;
        Ok(())
    }).await.unwrap();
    assert!(matches!(h.auth.pairing.issue_one_time_token(STANDARD_SCOPES, IssuePairing {
        scopes: STANDARD_SCOPES.to_vec(), ..Default::default()
    }).await, Err(Error::Persistence(_))));
}

#[tokio::test]
async fn proof_keys_are_persisted_on_sessions() {
    let h = Harness::new().await;
    let session = h.auth.sessions.issue(IssueSession {
        proof_key_thumbprint: Some(bootstrap_thumbprint()),
        method: Some(ServerAuthSessionMethod::DpopAccessToken),
        ..Default::default()
    }).await.unwrap();
    let id = session.session_id.0;
    let key: Option<String> = h.db.reader(move |db| Ok(db.query_row(
        "SELECT proof_key_thumbprint FROM auth_sessions WHERE session_id=?1", [id], |r| r.get(0)
    )?)).await.unwrap();
    assert!(key.as_deref() == Some(bootstrap_thumbprint().as_str()), "session key was not persisted");
}
