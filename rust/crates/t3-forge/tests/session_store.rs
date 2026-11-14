use std::sync::Arc;
use t3_config::{secrets::SecretStore, settings::SettingsService};
use t3_forge::github::{
    app::{AppSession, AppSessionStore},
    session_store::SettingsAppSessions,
};
fn session(token: &str) -> AppSession {
    AppSession {
        access_token: token.into(),
        access_token_expires_at: None,
        refresh_token: Some("private-refresh".into()),
        refresh_token_expires_at: None,
    }
}
#[tokio::test]
async fn refuses_host_mismatch_and_stale_refresh_before_persisting_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let settings = SettingsService::new(&path);
    let secrets = SecretStore::new(dir.path().join("secrets"));
    let store = Arc::new(SettingsAppSessions::new(settings.clone(), secrets.clone()));
    assert!(
        store
            .write("evil.example", session("private-rejected"))
            .await
            .is_err()
    );
    assert!(!path.exists());
    assert!(
        !store
            .replace("github.com", "old", Some(session("stale")))
            .await
            .unwrap()
    );
    store
        .write(" GITHUB.COM ", session("private-access"))
        .await
        .unwrap();
    let persisted = std::fs::read_to_string(&path).unwrap();
    assert!(!persisted.contains("private-access"));
    assert!(!persisted.contains("private-refresh"));
    assert!(persisted.contains("signIns"));
    assert!(!format!("{:?}", session("private-access")).contains("private-"));
    assert_eq!(
        store
            .read("github.com")
            .await
            .unwrap()
            .unwrap()
            .access_token,
        "private-access"
    );
    assert!(
        !store
            .replace("github.com", "wrong", Some(session("stale")))
            .await
            .unwrap()
    );
    assert!(
        store
            .replace("github.com", "private-access", Some(session("newer")))
            .await
            .unwrap()
    );
    assert!(
        !store
            .replace("github.com", "private-access", Some(session("stale")))
            .await
            .unwrap()
    );
    let reopened = SettingsAppSessions::new(SettingsService::new(&path), secrets.clone());
    assert_eq!(
        reopened
            .read("github.com")
            .await
            .unwrap()
            .unwrap()
            .access_token,
        "newer"
    );
    settings
        .update(
            &serde_json::from_value(
                serde_json::json!({"sourceControlHosts":{"github":{"signIns":{}}}}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        !store
            .replace("github.com", "newer", Some(session("resurrected")))
            .await
            .unwrap()
    );
    assert!(store.read("github.com").await.unwrap().is_none());
    store.write("github.com", session("last")).await.unwrap();
    assert!(store.replace("github.com", "last", None).await.unwrap());
    assert!(store.read("github.com").await.unwrap().is_none());
    assert!(
        !store
            .replace("github.com", "last", Some(session("resurrected")))
            .await
            .unwrap()
    );
    let output = t3_forge::github::cli::CliOutput {
        code: 1,
        stdout: "private-access".into(),
        stderr: "private-refresh".into(),
    };
    assert!(!format!("{output:?}").contains("private-"));
    store.write("github.com", session("race")).await.unwrap();
    let patch =
        serde_json::from_value(serde_json::json!({"sourceControlHosts":{"github":{"signIns":{}}}}))
            .unwrap();
    let (refresh, sign_out) = tokio::join!(
        store.replace("github.com", "race", Some(session("late-refresh"))),
        settings.update(&patch)
    );
    refresh.unwrap();
    sign_out.unwrap();
    assert!(store.read("github.com").await.unwrap().is_none());
}
