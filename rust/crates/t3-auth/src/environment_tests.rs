#![allow(clippy::unwrap_used)]
use super::*;
#[path = "../tests/common/mod.rs"]
mod common;
use common::*;

#[tokio::test]
async fn revocation_cannot_be_undone_by_a_racing_exchange() {
    let mut h = Harness::new().await;
    let key = bootstrap_thumbprint();
    let credential = h.auth.issue_connect_credential(&key, false).await.unwrap();
    let hook = Arc::new(ConnectTestHook::default());
    h.auth.connect_test_hook = Some(hook.clone());
    let auth = h.auth.clone();
    let request = bootstrap_request(Some(key.clone()));
    let exchange = tokio::spawn(async move {
        auth.exchange_bootstrap(&request, &credential.credential.0, None, metadata())
            .await
    });
    hook.spent.notified().await;
    assert!(
        !h.auth
            .pairing
            .is_connect_device_trusted(&key)
            .await
            .unwrap()
    );
    let auth = h.auth.clone();
    let revoke_key = key.clone();
    let mut revoke = tokio::spawn(async move { auth.revoke_connect_device(&revoke_key).await });
    tokio::select! {
        result = &mut revoke => {
            result.unwrap().unwrap();
            hook.resume.notify_one();
            let issued = exchange.await.unwrap().unwrap();
            assert!(h.auth.sessions.verify(&issued.access_token.0).await.is_err(), "revocation completed in the spend-to-trust window and exchange restored access");
        }
        () = hook.revoke_waiting.notified() => {
            hook.resume.notify_one();
            let issued = exchange.await.unwrap().unwrap();
            assert!(revoke.await.unwrap().unwrap());
            assert!(h.auth.sessions.verify(&issued.access_token.0).await.is_err());
        }
    }
    assert!(
        !h.auth
            .pairing
            .is_connect_device_trusted(&key)
            .await
            .unwrap()
    );
}
