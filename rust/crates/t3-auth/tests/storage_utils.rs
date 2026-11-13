#![allow(clippy::unwrap_used)]
mod common;
use common::*;
use t3_auth::{AuthConfig, ServerSecretStore, clock::Clock, utils::*};
use t3_contracts::auth::*;

#[tokio::test]
async fn secret_create_publish_race_reuses_winner_without_partial_reads() {
    let h = Harness::new().await;
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(12));
    let mut tasks = Vec::new();
    for _ in 0..12 {
        let secrets = h.auth.secrets.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            secrets
                .get_or_create_random("session-signing-key", 32)
                .await
                .unwrap()
        }));
    }
    let mut values = Vec::new();
    for task in tasks {
        values.push(task.await.unwrap());
    }
    let persisted = h
        .auth
        .secrets
        .get("session-signing-key")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.len(), 32);
    assert!(values.iter().all(|v| v == &persisted));
    assert_eq!(
        h.auth
            .secrets
            .get_or_create_random("session-signing-key", 1)
            .await
            .unwrap(),
        persisted
    );
    assert!(
        h.auth
            .secrets
            .create("session-signing-key", b"different")
            .await
            .unwrap_err()
            .is_already_exists()
    );
    h.auth
        .secrets
        .set("session-signing-key", b"replacement")
        .await
        .unwrap();
    assert_eq!(
        h.auth
            .secrets
            .get("session-signing-key")
            .await
            .unwrap()
            .unwrap(),
        b"replacement"
    );
    h.auth.secrets.remove("session-signing-key").await.unwrap();
    h.auth.secrets.remove("session-signing-key").await.unwrap();
    assert!(
        h.auth
            .secrets
            .get("session-signing-key")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        std::fs::read_dir(h.auth.secrets.directory())
            .unwrap()
            .all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp"))
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(h.auth.secrets.directory())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(h.auth.secrets.path("server-signing-key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[tokio::test]
async fn secret_read_and_persist_failures_propagate() {
    let temp = tempfile::tempdir().unwrap();
    let store = ServerSecretStore::open(temp.path().join("secrets"))
        .await
        .unwrap();
    std::fs::create_dir(store.path("read-failure")).unwrap();
    assert!(matches!(
        store.get_or_create_random("read-failure", 32).await,
        Err(t3_auth::Error::Secret {
            operation: "read",
            ..
        })
    ));
    std::fs::create_dir(store.path("write-failure")).unwrap();
    assert!(matches!(
        store.set("write-failure", b"hello").await,
        Err(t3_auth::Error::Secret {
            operation: "persist",
            ..
        })
    ));
    let fake = temp.path().join("file");
    std::fs::write(&fake, b"x").unwrap();
    assert!(ServerSecretStore::open(fake.join("secrets")).await.is_err());
}

#[tokio::test]
async fn replay_pruning_removes_only_strictly_expired_marker_bin_files() {
    let h = Harness::new().await;
    let old = std::time::UNIX_EPOCH + std::time::Duration::from_millis((NOW - 86400001) as u64);
    let boundary = old + std::time::Duration::from_millis(1);
    for name in [
        "dpop-proof-old",
        "cloud-mint-jti-old",
        "cloud-mint-nonce-old",
        "cloud-health-jti-old",
        "cloud-health-nonce-old",
        "dpop-proof-at-max-age",
        "server-signing-key",
        "asset-access-signing-key",
        "cloud-cli-oauth-token",
        "cloud-cli-desired-link",
        "cloud-link-ed25519-key-pair",
        "cloud-link-ed25519-private-key",
        "cloud-link-ed25519-public-key",
        "cloud-mint-ed25519-public-key",
        "cloud-endpoint-runtime-config",
        "cloud-endpoint-confirmed-origin",
        "cloud-linked-user-id",
        "cloud-relay-url",
        "cloud-relay-issuer",
        "cloud-relay-environment-credential",
        "cloud-publish-agent-activity",
        "provider-env-test",
        "provider-auth-test",
        "usage-limit-source-test",
    ] {
        h.auth.secrets.set(name, b"x").await.unwrap();
        let file = std::fs::File::options()
            .write(true)
            .open(h.auth.secrets.path(name))
            .unwrap();
        file.set_modified(if name == "dpop-proof-at-max-age" {
            boundary
        } else {
            old
        })
        .unwrap();
    }
    let temp = h
        .auth
        .secrets
        .directory()
        .join("dpop-proof-pending.bin.0000.tmp");
    std::fs::write(&temp, b"x").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&temp)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let result = h
        .auth
        .secrets
        .prune_replay_markers(h.clock.now_ms())
        .await
        .unwrap();
    assert_eq!(result.removed, 5);
    assert_eq!(result.failed, 0);
    assert_eq!(result.matched, 6);
    assert!(temp.exists());
    assert!(h.auth.secrets.path("server-signing-key").exists());
    assert!(h.auth.secrets.path("dpop-proof-at-max-age").exists());
}

#[test]
fn metadata_infers_browser_and_preserves_transport_identity() {
    for (agent, browser) in [
        (
            "Mozilla/5.0 Macintosh Chrome/136 Electron/36 Safari/537",
            "Electron",
        ),
        ("Android Chrome/129 Mobile EdgA/129", "Edge"),
        ("iPhone Safari/605 EdgiOS/129", "Edge"),
        ("iPhone FxiOS/131 Safari/605", "Firefox"),
        ("iPhone OPiOS/5 Safari/605", "Opera"),
    ] {
        assert_eq!(
            derive_client_metadata(Some(agent), None, None)
                .browser
                .unwrap()
                .0,
            browser
        );
    }
    let presented=serde_json::from_value(serde_json::json!({"label":"T3 Code Mobile","deviceType":"mobile","os":"iOS","browser":"ignored"})).unwrap();
    let m = derive_client_metadata(
        Some("Mozilla/5.0 (Macintosh; Intel Mac OS X) Chrome/136 Electron/36 Safari/537"),
        Some("::ffff:192.168.213.72"),
        Some(&presented),
    );
    assert_eq!(m.browser.unwrap().0, "Electron");
    assert_eq!(m.device_type, AuthClientMetadataDeviceType::Mobile);
    assert_eq!(m.os.unwrap().0, "iOS");
    assert_eq!(m.ip_address.unwrap().0, "192.168.213.72");
    assert!(m.user_agent.unwrap().0.contains("Electron/36"));
    for (agent, device) in [
        ("curl/1", AuthClientMetadataDeviceType::Bot),
        ("iPad Safari/1", AuthClientMetadataDeviceType::Tablet),
        ("iPhone", AuthClientMetadataDeviceType::Mobile),
        ("Windows NT", AuthClientMetadataDeviceType::Desktop),
    ] {
        assert_eq!(
            derive_client_metadata(Some(agent), None, None).device_type,
            device
        );
    }
}

#[test]
fn cookies_and_policy_are_isolated_by_actual_bind_host_mode_and_environment() {
    let mut config = AuthConfig {
        port: 5775,
        host: Some("127.0.0.1".into()),
        state_dir: "/srv/one".into(),
        environment_id: "env-one".into(),
        ..Default::default()
    };
    let first = session_cookie_name(&config);
    config.state_dir = "/srv/two".into();
    assert_ne!(first, session_cookie_name(&config));
    config.host = Some("0.0.0.0".into());
    let first = session_cookie_name(&config);
    config.port = 8080;
    config.state_dir = "/srv/moved".into();
    assert_eq!(first, session_cookie_name(&config));
    config.environment_id = "env-two".into();
    assert_ne!(first, session_cookie_name(&config));
    assert_eq!(legacy_cookie_name(&config).as_deref(), Some("t3_session"));
    config.desktop = true;
    assert_eq!(session_cookie_name(&config), "t3_session_8080");
    assert_eq!(
        descriptor(&config).policy,
        ServerAuthPolicy::RemoteReachable
    );
    assert_eq!(descriptor(&config).bootstrap_methods.len(), 2);
    for host in [
        None,
        Some("localhost"),
        Some("127.0.0.2"),
        Some("[::1]"),
        Some("LOCALHOST."),
    ] {
        config.host = host.map(str::to_owned);
        config.desktop = false;
        assert_eq!(
            descriptor(&config).policy,
            ServerAuthPolicy::LoopbackBrowser
        );
        config.desktop = true;
        assert_eq!(
            descriptor(&config).policy,
            ServerAuthPolicy::DesktopManagedLocal
        );
    }
    for host in [
        "",
        "0.0.0.0",
        "::",
        "remote.test",
        "::ffff:127.0.0.1",
        "127.1",
    ] {
        assert!(loopback_bind_address(Some(host)).is_none(), "{host}");
    }
}

#[tokio::test]
async fn server_internal_paths_cover_secrets_sqlite_and_symlink_targets() {
    let h = Harness::new().await;
    let state = h.temp.path().to_owned();
    let secrets = h.auth.secrets.directory().to_owned();
    for name in ["state.sqlite", "state.sqlite-wal", "statev2.sqlite-shm"] {
        if !state.join(name).exists() {
            std::fs::File::options()
                .write(true)
                .create_new(true)
                .open(state.join(name))
                .unwrap();
        }
        assert!(
            is_server_internal_path(state.join(name), state.clone(), secrets.clone())
                .await
                .unwrap(),
            "{name}: state={state:?}, canonical={:?}, file={:?}",
            dunce::canonicalize(&state),
            dunce::canonicalize(state.join(name))
        );
    }
    assert!(
        is_server_internal_path(
            secrets.join("server-signing-key.bin"),
            state.clone(),
            secrets.clone()
        )
        .await
        .unwrap()
    );
    assert!(
        !is_server_internal_path(state.join("notes.txt"), state.clone(), secrets.clone())
            .await
            .unwrap()
    );
    assert!(
        !is_server_internal_path(state.join("nested/state.sqlite"), state, secrets)
            .await
            .unwrap()
    );
}

#[test]
fn mac_and_base64_formats_are_compatible() {
    let id = uuid::Uuid::parse_str(&random_uuid().unwrap()).unwrap();
    assert_eq!(id.get_version_num(), 4);
    assert_eq!(id.get_variant(), uuid::Variant::RFC4122);

    let signer = PayloadSigner::new(vec![0x0b; 20]).unwrap();
    assert_eq!(
        hex(signer.sign_bytes("Hi There")),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    assert!(signer.verify("Hi There", &signer.sign("Hi There")));
    assert!(!signer.verify("Hi There", "!"));
    assert!(timing_safe_b64("Zg", "Zg=="));
    assert!(timing_safe_b64("Zg\r\n", "Zg"));
    assert!(!timing_safe_b64("abc+", "abc-"));
}
