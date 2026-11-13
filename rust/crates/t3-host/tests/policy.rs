use serde_json::json;
use t3_contracts::{auth::ServerAuthPolicy, settings::ServerSettings};
use t3_host::policy::ManagedPolicy;
fn policy(value: serde_json::Value) -> ManagedPolicy {
    ManagedPolicy::from_document(Some(serde_json::from_value(value).unwrap())).unwrap()
}
#[test]
fn malformed_unreadable_and_unknown_policy_files_stop_loading() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("policy.json");
    assert!(ManagedPolicy::load(&path).unwrap().allows_remote_clients());
    for raw in [
        "{",
        "{}",
        r#"{"schemaVersion":1,"organization":"Org","mystery":1}"#,
    ] {
        std::fs::write(&path, raw).unwrap();
        assert!(ManagedPolicy::load(&path).is_err());
    }
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(ManagedPolicy::load(&path).is_err());
}
#[test]
fn local_only_policy_checks_auth_before_bind_and_connect() {
    let p = policy(
        json!({"schemaVersion":1,"organization":"Org","limits":{"clients":["local-browser"]}}),
    );
    assert!(!p.allows_connect());
    assert!(!p.allows_auth_policy(&ServerAuthPolicy::UnsafeNoAuth));
    assert!(!p.allows_auth_policy(&ServerAuthPolicy::RemoteReachable));
    for host in [
        "0.0.0.0",
        "",
        "192.168.1.1",
        "example.com",
        "localhost.example.com",
    ] {
        assert!(
            p.check_server_reachability(ServerAuthPolicy::LoopbackBrowser, Some(host), false)
                .is_err(),
            "{host}"
        );
    }
    for (host, expected) in [
        (None, "127.0.0.1"),
        (Some("localhost"), "127.0.0.1"),
        (Some("127.7.0.1"), "127.7.0.1"),
        (Some("::1"), "::1"),
    ] {
        assert_eq!(
            p.check_server_reachability(ServerAuthPolicy::LoopbackBrowser, host, false)
                .unwrap(),
            expected
        );
    }
    assert!(
        p.check_server_reachability(ServerAuthPolicy::LoopbackBrowser, None, true)
            .is_err()
    );
}
#[test]
fn locked_settings_are_overlaid_and_client_echo_does_not_persist_them() {
    let p = policy(
        json!({"schemaVersion":1,"organization":"Org","settings":{"keepAwakeWhileAgentsWork":{"locked":true,"value":false},"defaultRuntimeMode":{"locked":true,"value":"approval-required"}}}),
    );
    let current = ServerSettings::default();
    let locked = p.apply_locks(&current).unwrap();
    assert!(!locked.keep_awake_while_agents_work);
    assert!(current.keep_awake_while_agents_work);
    let kept = p
        .vet_write(
            &current,
            &locked,
            &json!({"keepAwakeWhileAgentsWork":false,"defaultRuntimeMode":"approval-required"}),
        )
        .unwrap();
    assert_eq!(
        kept.keep_awake_while_agents_work,
        current.keep_awake_while_agents_work
    );
    assert_eq!(kept.default_runtime_mode, current.default_runtime_mode);
    assert!(
        p.vet_write(
            &current,
            &current,
            &json!({"keepAwakeWhileAgentsWork":true})
        )
        .is_err()
    );
}
#[test]
fn invalid_settings_and_override_bypasses_fail_at_load() {
    for setting in [
        json!({"madeUp":{"locked":true,"value":false}}),
        json!({"observability":{"locked":true,"value":{}}}),
        json!({"keepAwakeWhileAgentsWork":{"locked":true,"value":"wrong"}}),
        json!({"storageCleanup.worktreeAfterDays":{"locked":true,"value":1}}),
        json!({"providerInstances.codex.config.binaryPath":{"locked":true,"value":"evil"}}),
    ] {
        assert!(
            ManagedPolicy::from_document(Some(
                serde_json::from_value(
                    json!({"schemaVersion":1,"organization":"Org","settings":setting})
                )
                .unwrap()
            ))
            .is_err()
        );
    }
}
#[test]
fn runtime_ceiling_and_update_gates_use_policy_not_environment() {
    let p = policy(
        json!({"schemaVersion":1,"organization":"Org","limits":{"maxRuntimeMode":"auto-accept-edits","updates":{"mode":"auto","feedUrl":"https://mirror.example/releases","allowNightly":false},"telemetry":false}}),
    );
    assert_eq!(
        p.apply_locks(&ServerSettings::default())
            .unwrap()
            .default_runtime_mode,
        t3_contracts::provider_policy::RuntimeMode::AutoAcceptEdits
    );
    assert_eq!(
        p.update_mirror(),
        Some(("Org", "https://mirror.example/releases"))
    );
    assert!(p.check_update(Some("stable")).is_ok());
    assert!(p.check_update(Some("nightly")).is_err());
    assert_eq!(p.denied_by("telemetry"), Some("Org"));
}

#[test]
fn credential_settings_are_refused_and_defaults_preserve_stored_user_choices() {
    assert!(ManagedPolicy::from_document(Some(serde_json::from_value(json!({"schemaVersion":1,"organization":"Org","settings":{"cloudSpeechCredentials.openai":{"locked":true,"value":"sensitive-token"}}})).unwrap())).is_err());
    let p = policy(
        json!({"schemaVersion":1,"organization":"Org","settings":{"keepAwakeWhileAgentsWork":{"locked":false,"value":false}}}),
    );
    assert!(!p.defaults().unwrap().keep_awake_while_agents_work);
    assert_eq!(
        p.with_defaults(&json!({}))["keepAwakeWhileAgentsWork"],
        false
    );
    assert_eq!(
        p.with_defaults(&json!({"keepAwakeWhileAgentsWork":true}))["keepAwakeWhileAgentsWork"],
        true
    );
    let settings = p.defaults().unwrap();
    let restored = p
        .keep_stored(
            json!({}),
            &settings,
            &json!({"keepAwakeWhileAgentsWork":false}),
            |_| false,
        )
        .unwrap();
    assert_eq!(restored["keepAwakeWhileAgentsWork"], false);
    assert!(ManagedPolicy::from_document(Some(serde_json::from_value(json!({"schemaVersion":1,"organization":"Org","limits":{"providers":{"allowedDrivers":["unknown"]}}})).unwrap())).is_err());
}

#[test]
fn deployment_templates_cover_registry_controls_and_profile_matches_typescript() {
    use t3_host::policy_templates::*;
    let admx = windows_admx();
    for value in [
        "Organization",
        "SupportUrl",
        "Settings",
        "MaxRuntimeMode",
        "Connect",
        "WebhookTasks",
        "Telemetry",
        "BrowserCookieImport",
        "ProviderInstances",
        "UpdatesMode",
        "UpdatesFeedUrl",
        "AllowNightly",
        "Clients",
        "AllowedDrivers",
        "Updates",
    ] {
        assert!(admx.contains(&format!("valueName=\"{value}\"")), "{value}");
    }
    for list in ["Clients", "AllowedDrivers"] {
        assert!(admx.contains(&format!(
            "key=\"SOFTWARE\\Policies\\T3Tools\\T3Code\\{list}\""
        )));
    }
    assert!(windows_adml().contains("<presentation id=\"AllowedDrivers\">"));
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("policy-profile.json")).unwrap();
    let document = serde_json::from_value(fixture["document"].clone()).unwrap();
    assert_eq!(
        macos_profile(&document, "profile-uuid", "payload-uuid").unwrap(),
        fixture["profile"].as_str().unwrap()
    );
    let document=serde_json::from_value(json!({"schemaVersion":1,"organization":"Org","settings":{"defaultModelSelection":{"value":null,"locked":true}}})).unwrap();
    assert_eq!(
        macos_profile(&document, "p", "q").unwrap_err().to_string(),
        "settings.defaultModelSelection is null, which a configuration profile cannot hold; deploy this policy as a file."
    );
}

#[tokio::test(start_paused = true)]
async fn serve_routes_fail_closed_on_hangs_and_failed_removal_and_recheck_success() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use t3_host::{
        Result,
        policy::{ServeRoute, ServeRoutes},
    };
    struct Routes {
        hang: u8,
        fail: bool,
        present: AtomicBool,
    }
    #[async_trait::async_trait]
    impl ServeRoutes for Routes {
        async fn can_bind(&self, _: &str, _: u16) -> Result<bool> {
            if self.hang == 1 {
                std::future::pending().await
            } else {
                Ok(true)
            }
        }
        async fn routes(&self, _: &str, _: u16) -> Result<Vec<ServeRoute>> {
            if self.hang == 2 {
                return std::future::pending().await;
            }
            Ok(if self.present.load(Ordering::SeqCst) {
                vec![ServeRoute {
                    serve_port: 443,
                    removable: true,
                }]
            } else {
                vec![]
            })
        }
        async fn disable(&self, _: &ServeRoute) -> Result<()> {
            if self.hang == 3 {
                return std::future::pending().await;
            }
            if !self.fail {
                self.present.store(false, Ordering::SeqCst);
            }
            Ok(())
        }
    }
    let p = policy(
        json!({"schemaVersion":1,"organization":"Org","limits":{"clients":["local-browser"]}}),
    );
    for (hang, fail, success) in [
        (1, false, false),
        (2, false, false),
        (3, false, false),
        (0, true, false),
        (0, false, true),
    ] {
        let routes = Routes {
            hang,
            fail,
            present: AtomicBool::new(true),
        };
        assert_eq!(
            p.close_tailscale_serve_to("127.0.0.1", 3773, &routes)
                .await
                .is_ok(),
            success,
            "hang={hang}, failed removal={fail}"
        );
        assert_eq!(routes.present.load(Ordering::SeqCst), !success);
    }
}
