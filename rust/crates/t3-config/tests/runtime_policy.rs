use serde_json::json;
use t3_config::{home::HomeLayout, policy::*, runtime::*};
fn state() -> RuntimeState {
    serde_json::from_value(json!({"version":1,"pid":std::process::id(),"port":3773,"origin":"http://127.0.0.1:3773","startedAt":"9999-01-01T00:00:00Z"})).unwrap()
}
#[tokio::test]
async fn runtime_file_and_ownership() {
    let temp = tempfile::tempdir().unwrap();
    let home = HomeLayout::derive(temp.path(), false, true);
    home.ensure_directories().unwrap();
    assert!(read_runtime_state(&home.runtime_state()).is_none());
    for raw in [
        "",
        "not json",
        "{}",
        r#"{"version":2,"pid":1,"port":1,"origin":"x","startedAt":"x"}"#,
    ] {
        std::fs::write(home.runtime_state(), raw).unwrap();
        assert!(read_runtime_state(&home.runtime_state()).is_none());
    }
    persist_runtime_state(&home.runtime_state(), &state())
        .await
        .unwrap();
    assert!(read_live_runtime_state(&home.runtime_state()).is_some());
    assert!(ServerOwner::acquire(&home).await.is_err());
    let mut stale = state();
    stale.pid = 0;
    persist_runtime_state(&home.runtime_state(), &stale)
        .await
        .unwrap();
    let (a, b) = tokio::join!(ServerOwner::acquire(&home), ServerOwner::acquire(&home));
    assert_ne!(a.is_ok(), b.is_ok());
    let owner = a.ok().or(b.ok()).unwrap();
    let database = t3_persistence::Database::open_with_options(
        home.database(),
        t3_persistence::OpenOptions {
            migration_owner_directory: None,
            ..Default::default()
        },
    )
    .unwrap();
    drop(database);
    owner.publish(state()).await.unwrap();
    assert!(
        std::fs::read_to_string(home.runtime_state())
            .unwrap()
            .ends_with(&format!("\"ownerId\":\"{}\"}}\n", owner.owner_id))
    );
    assert_eq!(
        read_runtime_state(&home.runtime_state()).unwrap().owner_id,
        Some(owner.owner_id.clone())
    );
    assert!(ServerOwner::acquire(&home).await.is_err());
    drop(owner);
    assert!(!home.runtime_state().exists());
    assert!(home.ownership_database().exists());
    let owner = ServerOwner::acquire(&home).await.unwrap();
    let mut foreign = state();
    foreign.owner_id = Some("different-owner".into());
    persist_runtime_state(&home.runtime_state(), &foreign)
        .await
        .unwrap();
    drop(owner);
    assert_eq!(read_runtime_state(&home.runtime_state()).unwrap(), foreign);
}
#[test]
fn origins_and_policy_loading() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("policy.json");
    assert_eq!(runtime_origin(None, 123), "http://127.0.0.1:123");
    assert_eq!(runtime_origin(Some("::"), 123), "http://127.0.0.1:123");
    assert_eq!(runtime_origin(Some("::1"), 123), "http://[::1]:123");
    assert_eq!(runtime_origin(Some("*"), 123), "http://*:123");
    assert_eq!(runtime_origin(Some(""), 123), "http://127.0.0.1:123");
    assert!(ManagedPolicy::load_file(&path).unwrap().document.is_none());
    for value in [
        json!({"schemaVersion":1,"organization":"Org","futureLimit":false}),
        json!({"schemaVersion":1,"organization":"Org","limits":{"unknown":true}}),
        json!({"schemaVersion":1,"organization":"Org","limits":{"clients":[]}}),
    ] {
        assert!(decode(&path, &value.to_string()).is_err());
    }
    std::fs::write(&path,json!({"schemaVersion":1,"organization":"Org","settings":{"threadTitleInstructions":{"locked":true,"value":"company"}}}).to_string()).unwrap();
    let policy = ManagedPolicy::load_file(&path).unwrap();
    let loaded = policy.loaded_policy().unwrap().unwrap();
    assert_eq!(loaded.organization, "Org");
    assert_eq!(loaded.fingerprint.len(), 12);
}
#[test]
fn policy_fingerprint_keeps_record_insertion_order() {
    use sha2::{Digest, Sha256};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("policy.json");
    let canonical = r#"{"schemaVersion":1,"organization":"Org","settings":{"threadTitleInstructions":{"value":"first","locked":false},"branchNamePrefix":{"value":"second","locked":true}}}"#;
    std::fs::write(&path, canonical).unwrap();
    let policy = ManagedPolicy::load_file(path).unwrap();
    let hash = format!("{:x}", Sha256::digest(canonical.as_bytes()));
    assert_eq!(
        policy.loaded_policy().unwrap().unwrap().fingerprint,
        hash[..12]
    );
}
#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn replaced_lock_publishes_lost_ownership() {
    let temp = tempfile::tempdir().unwrap();
    let home = HomeLayout::derive(temp.path(), false, true);
    let owner = ServerOwner::acquire(&home).await.unwrap();
    let mut health = owner.ownership_health();
    std::fs::rename(
        home.ownership_database(),
        temp.path().join("moved-owner.sqlite"),
    )
    .unwrap();
    tokio::time::advance(std::time::Duration::from_secs(5)).await;
    health.changed().await.unwrap();
    assert_eq!(*health.borrow(), OwnershipHealth::Lost);
}
