#![allow(clippy::unwrap_used)]
mod common;
use common::Fixture;
use serde_json::{Value, json};
use std::sync::Arc;
use t3_contracts::{provider_instance::*, server::ServerProviderState};
use t3_provider::{
    catalog::Manifest, environment::Platform, instances::InstanceSpec, native_probe::ProcessProbes,
    native_provider::*, service::ProviderInstanceService, transport::ProviderClock,
};
struct Clock;
impl ProviderClock for Clock {
    fn now_ms(&self) -> i64 {
        1_800_000_000_000
    }
}
fn provider(fixture: &Fixture, kind: &str) -> NativeProvider {
    with_manifest(fixture, kind, Arc::new(Manifest::bundled().unwrap()))
}
fn with_manifest(
    fixture: &Fixture,
    kind: &str,
    manifest: impl Into<t3_provider::catalog::ManifestSource>,
) -> NativeProvider {
    let spec = Arc::new(InstanceSpec {
        instance_id: ProviderInstanceId("work".into()),
        driver: ProviderDriverKind(kind.into()),
        display_name: Some("Work".into()),
        accent_color: None,
        enabled: true,
        config: json!({"binaryPath":fixture.binary(),"homePath":fixture.directory.path().join("provider-home"),"customModels":[]}),
        variables: vec![],
    });
    NativeProvider::new(
        spec,
        NativeProviderContext {
            environment: fixture.environment.clone(),
            home: fixture.directory.path().to_path_buf(),
            cwd: fixture.directory.path().to_path_buf(),
            platform: Platform::current(),
            managed_mcp: false,
            server_version: "0.1.0".into(),
        },
        Arc::new(ProcessProbes {
            server_version: "0.1.0".into(),
        }),
        Arc::new(ProcessExecutableProbe),
        Arc::new(Clock),
        manifest,
    )
}

#[tokio::test]
async fn native_models_follow_a_refreshed_shared_manifest_without_rebuilding_the_provider() {
    use t3_provider::catalog::ModelManifest;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let manifest = Arc::new(
        ModelManifest::new(reqwest::Client::new(), None)
            .unwrap()
            .with_url(format!("http://{address}")),
    );
    let source = manifest.source();
    let fixture = Fixture::new(json!({"protocol":"claude","version":"2.1.200 (Claude Code)"}));
    let provider = with_manifest(&fixture, "claudeAgent", manifest.clone());
    assert!(
        !provider
            .snapshot(None, true)
            .await
            .unwrap()
            .models
            .iter()
            .any(|model| model.slug.0 == "future-model")
    );
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        assert_ne!(stream.read(&mut request).await.unwrap(), 0);
        let body = json!({"version":1,"updatedAt":"2099-01-01T00:00:00Z","currentModels":{},"providers":{"claudeAgent":{"profiles":{},"models":[{"slug":"future-model","name":"Future Model","status":"current"}]}}}).to_string();
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    manifest
        .refresh(1_800_000_000_000, true, true, true)
        .await
        .unwrap()
        .unwrap();
    server.await.unwrap();
    assert!(Arc::ptr_eq(&source.current(), &manifest.current().await));
    let snapshot = provider.snapshot(None, true).await.unwrap();
    assert_eq!(snapshot.models[0].slug.0, "future-model");
    assert_eq!(snapshot.models[0].name.0, "Future Model");
    assert_eq!(
        fixture
            .calls()
            .iter()
            .filter(|call| call == &&vec!["get_usage"])
            .count(),
        1
    );
}
#[tokio::test]
async fn native_codex_discovery_reads_account_models_config_and_usage_without_starting_a_turn() {
    let fixture = Fixture::new(
        json!({"protocol":"codex","version":"codex-cli 0.166.0", "nativeModels":[], "refreshedModels":[{"id":"gpt-6-luna","model":"gpt-6-luna","displayName":"GPT-6 Luna","description":"","hidden":false,"isDefault":true,"supportedReasoningEfforts":[{"reasoningEffort":"low","description":""}],"defaultReasoningEffort":"low"}]}),
    );
    let snapshot = provider(&fixture, "codex")
        .snapshot(None, true)
        .await
        .unwrap();
    assert_eq!(snapshot.status, ServerProviderState::Ready);
    assert_eq!(snapshot.models[0].slug.0, "gpt-6-luna");
    let value = serde_json::to_value(snapshot).unwrap();
    assert_eq!(value["auth"]["label"], "ChatGPT Pro 200 Subscription");
    assert_eq!(value["auth"]["workspaceId"], "workspace");
    assert_eq!(
        value["usageLimits"]["windows"][0]["usedPercent"].as_f64(),
        Some(42.)
    );
    let methods: Vec<_> = fixture
        .calls()
        .into_iter()
        .filter(|c| c.len() == 1)
        .flatten()
        .collect();
    for method in [
        "initialize",
        "account/read",
        "model/list",
        "config/read",
        "account/rateLimits/read",
    ] {
        assert!(methods.iter().any(|m| m == method));
    }
    assert_eq!(
        methods
            .iter()
            .filter(|method| *method == "model/list")
            .count(),
        2
    );
    assert!(
        !methods
            .iter()
            .any(|m| m.starts_with("turn/") || m.starts_with("thread/"))
    );
}
#[tokio::test]
async fn native_claude_discovery_uses_hook_free_transient_session_and_reads_account_and_usage() {
    let fixture = Fixture::new(json!({"protocol":"claude","version":"2.1.200 (Claude Code)"}));
    let (policy, background) =
        tokio::sync::watch::channel(t3_provider::managed_provider::BackgroundPolicy::default());
    let provider = provider(&fixture, "claudeAgent")
        .into_managed(t3_provider::managed_provider::ManagedOptions::new(
            background,
        ))
        .unwrap();
    assert!(fixture.calls().is_empty());
    assert_eq!(
        provider.current().await.status,
        ServerProviderState::Warning
    );
    let mut changes = provider.subscribe_snapshots().unwrap();
    policy.send_modify(|policy| policy.server_ready = true);
    let snapshot = loop {
        let current = changes.borrow_and_update().clone().unwrap();
        if current.status == ServerProviderState::Ready {
            break current;
        }
        changes.changed().await.unwrap();
    };
    assert_eq!(snapshot.status, ServerProviderState::Ready);
    let value = serde_json::to_value(snapshot).unwrap();
    assert_eq!(value["auth"]["label"], "Claude Max Subscription");
    assert_eq!(value["usageLimits"]["windows"][0]["id"], "five_hour");
    assert_eq!(value["setup"]["canAuthenticate"], false);
    let calls = fixture.calls();
    let launch = calls
        .iter()
        .find(|c| c.iter().any(|s| s == "--input-format"))
        .unwrap();
    assert!(launch.iter().any(|s| s == "--no-session-persistence"));
    assert!(launch.iter().any(|s| s.contains("disableAllHooks")));
    for method in ["initialize", "get_usage", "get_settings"] {
        assert!(calls.iter().any(|c| c == &[method]));
    }
    assert!(
        !calls
            .iter()
            .flatten()
            .any(|s| s == "--resume" || s == "--continue")
    );
    provider.refresh_usage().await.unwrap();
    assert_eq!(
        fixture
            .calls()
            .iter()
            .filter(|call| call == &&vec!["get_usage"])
            .count(),
        2
    );
}
#[tokio::test]
async fn signed_out_accounts_do_not_look_ready_and_external_codex_routes_never_read_native_quota() {
    for (kind, account) in [
        ("codex", json!({"account":null,"requiresOpenaiAuth":true})),
        (
            "claudeAgent",
            json!({"tokenSource":"none","apiKeySource":"none"}),
        ),
    ] {
        let fixture = Fixture::new(
            json!({"protocol":if kind=="codex"{"codex"}else{"claude"},"version":"0.166.0","nativeAccount":account}),
        );
        assert_eq!(
            provider(&fixture, kind)
                .snapshot(None, true)
                .await
                .unwrap()
                .status,
            ServerProviderState::Error
        );
    }
    let fixture = Fixture::new(
        json!({"protocol":"codex","version":"0.166.0","routing":{"model_provider":"proxy","model_providers":{"proxy":{"base_url":"https://proxy.example/v1"}}}}),
    );
    let snapshot = provider(&fixture, "codex")
        .snapshot(None, true)
        .await
        .unwrap();
    let value: Value = serde_json::to_value(snapshot).unwrap();
    assert_eq!(value["usageLimits"]["unavailable"]["reason"], "unsupported");
    assert!(
        !fixture
            .calls()
            .iter()
            .any(|c| c == &["account/rateLimits/read"])
    );
}

#[tokio::test]
async fn native_reset_uses_only_the_dedicated_control_with_the_supplied_idempotency_key() {
    use t3_provider::native_probe::{NativeProbe, ProbeLaunch};
    let fixture = Fixture::new(json!({"protocol":"codex","resetKey":"fixture-key"}));
    let outcome = ProcessProbes {
        server_version: "0.1.0".into(),
    }
    .consume_codex_reset(
        ProbeLaunch {
            executable: fixture.binary(),
            environment: fixture.environment.clone(),
            cwd: fixture.directory.path().to_path_buf(),
            launch_args: String::new(),
            managed_mcp: false,
            include_status: true,
            gateway_discovery_grace: std::time::Duration::ZERO,
        },
        "fixture-key".into(),
    )
    .await
    .unwrap();
    assert_eq!(serde_json::to_value(outcome).unwrap(), json!("reset"));
    let controls: Vec<_> = fixture
        .calls()
        .into_iter()
        .filter(|call| call.len() == 1)
        .collect();
    assert_eq!(
        controls,
        vec![
            vec!["initialize"],
            vec!["initialized"],
            vec!["account/rateLimitResetCredit/consume"]
        ]
    );
}

#[tokio::test]
async fn claude_workspace_cache_skips_account_probes_and_deleted_workspaces_keep_user_skills() {
    let fixture = Fixture::new(json!({"protocol":"claude","version":"2.1.200 (Claude Code)"}));
    let home_skill = fixture
        .directory
        .path()
        .join("provider-home/skills/user/SKILL.md");
    std::fs::create_dir_all(home_skill.parent().unwrap()).unwrap();
    std::fs::write(
        &home_skill,
        "---\nname: user\ndescription: User skill\n---\nInstructions",
    )
    .unwrap();
    let provider = provider(&fixture, "claudeAgent");
    provider.snapshot(None, true).await.unwrap();
    let cwd = fixture.directory.path().join("removed-project");
    std::fs::create_dir(&cwd).unwrap();
    let path = cwd.to_str().unwrap();
    provider.snapshot(Some(path), true).await.unwrap();
    let calls = fixture.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call == &&vec!["--version"])
            .count(),
        1
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call == &&vec!["get_usage"])
            .count(),
        1
    );
    provider.snapshot(Some(path), false).await.unwrap();
    assert_eq!(fixture.calls(), calls);
    std::fs::remove_dir(&cwd).unwrap();
    let snapshot = provider.snapshot(Some(path), true).await.unwrap();
    let workspaces = serde_json::to_value(snapshot).unwrap()["workspaceSnapshots"].clone();
    assert_eq!(workspaces[0]["skills"].as_array().unwrap().len(), 1);
    assert_eq!(workspaces[0]["slashCommands"].as_array().unwrap().len(), 1);
    assert_eq!(workspaces[0]["slashCommands"][0]["name"], "compact");
    assert_eq!(workspaces[0]["slashCommandsPending"], false);
    assert_eq!(fixture.calls(), calls);
}
