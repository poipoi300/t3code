use serde_json::{Value, json};
use t3_config::{home::HomeLayout, secrets::*, settings::*};
use t3_contracts::{provider_instance::ProviderInstanceMutation, settings::ServerSettingsPatch};
fn patch(value: Value) -> ServerSettingsPatch {
    serde_json::from_value(value).unwrap()
}
fn write(path: &std::path::Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
fn value(settings: impl serde::Serialize) -> Value {
    serde_json::to_value(settings).unwrap()
}

#[tokio::test]
async fn persisted_defaults_and_legacy_token_and_idle_modes() {
    let temp = tempfile::tempdir().unwrap();
    let path = HomeLayout::derive(temp.path(), false, true).settings();
    let service = SettingsService::new(&path);
    assert_eq!(
        value(service.snapshot().await.unwrap())["idleCompaction"],
        "onReturn"
    );
    write(
        &path,
        r#"{ // config comment
      "responseStreamingMode":"token", "compactIdleThreads":"off",
      "projectSettingsOverrides":{"p":{"responseStreamingMode":"token","futureSetting":{"a":true},"futureNull":null}},
      "threadTitleInstructions":"keep me", "futureTopLevel":true,
    }"#,
    );
    let settings = value(service.reload().await.unwrap().settings);
    assert_eq!(settings["responseStreamingMode"], "paragraph");
    assert_eq!(
        settings["projectSettingsOverrides"]["p"]["responseStreamingMode"],
        "paragraph"
    );
    assert_eq!(settings["idleCompaction"], "off");
    assert_eq!(settings["compactIdleThreads"], false);
    service
        .update(&patch(
            json!({"projectSettingsOverrides":{"p":{"defaultAutoPull":true}}}),
        ))
        .await
        .unwrap();
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        saved["projectSettingsOverrides"]["p"]["futureSetting"],
        json!({"a":true})
    );
    assert!(saved.get("futureTopLevel").is_none());
    assert_eq!(saved["threadTitleInstructions"], "keep me");
    assert!(
        saved["projectSettingsOverrides"]["p"]
            .get("futureNull")
            .is_some_and(Value::is_null)
    );
    assert_eq!(
        value(
            service
                .update(&patch(json!({"compactIdleThreads":false})))
                .await
                .unwrap()
        )["idleCompaction"],
        "off"
    );
}
#[tokio::test]
async fn sparse_writes_external_edit_and_invalid_file_protection() {
    let temp = tempfile::tempdir().unwrap();
    let path = HomeLayout::derive(temp.path(), false, true).settings();
    let service = SettingsService::new(&path);
    service
        .update(&patch(json!({"threadTitleInstructions":" title "})))
        .await
        .unwrap();
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved["threadTitleInstructions"], "title");
    assert!(saved.get("storageCleanup").is_none());
    write(
        &path,
        r#"{"threadTitleInstructions":"external","branchNamePrefix":"custom"}"#,
    );
    let updated = value(
        service
            .update(&patch(json!({"continueThreadsAfterServerUpdate":false})))
            .await
            .unwrap(),
    );
    assert_eq!(updated["threadTitleInstructions"], "external");
    assert_eq!(updated["branchNamePrefix"], "custom");
    let defaults = json!({
        "defaultTheme": "aurora",
        "defaultThemeSetAt": "2026-11-14T00:00:00.000Z",
        "defaultAppearance": "dark",
        "defaultAppearanceSetAt": "2026-11-14T00:01:00.000Z",
        "requireConnectDeviceApproval": true
    });
    service.update(&patch(defaults.clone())).await.unwrap();
    let client: t3_contracts::settings::ClientServerSettingsPatch = serde_json::from_value(json!({
        "defaultTheme": "blocked",
        "defaultThemeSetAt": "2026-11-14T01:00:00.000Z",
        "defaultAppearance": "light",
        "defaultAppearanceSetAt": "2026-11-14T01:01:00.000Z",
        "threadTitleInstructions": "client title"
    }))
    .unwrap();
    service.update(&client.0).await.unwrap();
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for (key, expected) in defaults.as_object().unwrap() {
        assert_eq!(&saved[key], expected, "{key}");
    }
    assert_eq!(saved["threadTitleInstructions"], "client title");
    assert_eq!(
        value(service.reload().await.unwrap().settings)["defaultAppearance"],
        "dark"
    );
    write(&path, "{ truncated");
    let loaded = service.reload().await.unwrap();
    assert!(loaded.parse_failure.is_some());
    assert_eq!(
        value(loaded.settings)["continueThreadsAfterServerUpdate"],
        false
    );
    assert!(
        service
            .update(&patch(json!({"threadTitleInstructions":"lost"})))
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ truncated");
}
#[tokio::test]
async fn serializes_model_preferences_and_buffers_subscriptions() {
    let temp = tempfile::tempdir().unwrap();
    let path = HomeLayout::derive(temp.path(), false, true).settings();
    let service = SettingsService::new(&path);
    let mut events = service.subscribe_changes();
    let p1 = patch(
        json!({"setModelFavorites":[{"provider":"codex_work","model":"sol","favorite":true}]}),
    );
    let p2 = patch(
        json!({"setModelFavorites":[{"provider":"codex_work","model":"luna","favorite":true}],"setModelsHidden":[{"provider":"codex_work","model":"astra","hidden":true}]}),
    );
    let p3 = patch(
        json!({"setProviderModelOrder":{"provider":"codex_work","modelOrder":["luna","sol"]}}),
    );
    let (a, b, c) = tokio::join!(
        service.update(&p1),
        service.update(&p2),
        service.update(&p3)
    );
    a.unwrap();
    b.unwrap();
    c.unwrap();
    let current = value(service.snapshot().await.unwrap());
    assert_eq!(current["favorites"].as_array().unwrap().len(), 2);
    assert_eq!(
        current["providerModelPreferences"]["codex_work"]["hiddenModels"],
        json!(["astra"])
    );
    assert_eq!(
        current["providerModelPreferences"]["codex_work"]["modelOrder"],
        json!(["luna", "sol"])
    );
    assert!(events.recv().await.is_some());
    assert!(events.recv().await.is_some());
    assert!(events.recv().await.is_some());
    service.update(&p1).await.unwrap();
    assert_eq!(
        value(service.snapshot().await.unwrap())["favorites"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
#[tokio::test]
async fn provider_create_is_atomic_and_secrets_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let home = HomeLayout::derive(temp.path(), false, true);
    let service = SettingsService::new(home.settings());
    let mutation:ProviderInstanceMutation = serde_json::from_value(json!({"operation":"create","instanceId":"codex_work","instance":{"driver":"codex","config":{},"environment":[{"name":"TOKEN","value":"private","sensitive":true}]}})).unwrap();
    let p = patch(json!({}));
    let (a, b) = tokio::join!(
        service.update_provider_instance(&mutation, &p),
        service.update_provider_instance(&mutation, &p)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let raw = std::fs::read_to_string(home.settings()).unwrap();
    assert!(!raw.contains("private"));
    assert!(raw.contains("valueRedacted"));
    assert_eq!(
        value(service.snapshot().await.unwrap())["providerInstances"]["codex_work"]["environment"]
            [0]["value"],
        "private"
    );
    let mut redacted = value(service.snapshot().await.unwrap());
    redact_for_client(&mut redacted, true);
    assert_eq!(
        redacted["providerInstances"]["codex_work"]["environment"][0]["value"],
        ""
    );
    let store = SecretStore::new(home.secrets());
    assert_eq!(
        store
            .get("provider-env-Y29kZXhfd29yaw-VE9LRU4")
            .unwrap()
            .unwrap(),
        b"private"
    );
    let remove: ProviderInstanceMutation =
        serde_json::from_value(json!({"operation":"remove","instanceId":"codex_work"})).unwrap();
    service.update_provider_instance(&remove, &p).await.unwrap();
    assert!(
        store
            .get("provider-env-Y29kZXhfd29yaw-VE9LRU4")
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
async fn model_selection_and_nested_patch_semantics() {
    let temp = tempfile::tempdir().unwrap();
    let path = HomeLayout::derive(temp.path(), false, true).settings();
    let service = SettingsService::new(path);
    service.update(&patch(json!({"textGenerationModelSelection":{"instanceId":"claudeAgent","model":"custom","options":[{"id":"x","value":"a"}]},"usagePriceOverrides":{"model":{"inputCostPerMillionTokens":1,"outputCostPerMillionTokens":2}}}))).await.unwrap();
    let updated = value(service.update(&patch(json!({"textGenerationModelSelection":{"instanceId":"codex"},"usagePriceOverrides":{"model":null},"storageCleanup":{"logsAfterDays":8}}))).await.unwrap());
    assert_eq!(updated["textGenerationModelSelection"]["model"], "custom");
    assert!(
        updated["textGenerationModelSelection"]
            .get("options")
            .is_none()
    );
    assert!(updated["usagePriceOverrides"].get("model").is_none());
    assert_eq!(updated["storageCleanup"]["logsAfterDays"], 8);
    service
        .update(&patch(
            json!({"worktreesDirectory":temp.path().join("custom")}),
        ))
        .await
        .unwrap();
    let updated = value(
        service
            .update(&patch(
                json!({"worktreesDirectory":temp.path().join("next")}),
            ))
            .await
            .unwrap(),
    );
    assert_eq!(
        updated["previousWorktreesDirectories"],
        json!([temp.path().join("custom")])
    );
}
#[tokio::test]
async fn inline_credentials_and_legacy_providers_migrate() {
    let temp = tempfile::tempdir().unwrap();
    let home = HomeLayout::derive(temp.path(), false, true);
    write(
        &home.settings(),
        r#"{"providers":{"opencode":{"enabled":false,"serverPassword":"password"}},"providerInstances":{"codex":{"driver":"codex","enabled":true,"config":{"enabled":false}}},"bitbucket":{"accessToken":"access","email":"me"},"projectAutoPullOverrides":{"p":true}}"#,
    );
    let service = SettingsService::new(home.settings());
    let state = service.reload().await.unwrap();
    assert!(
        !format!("{state:?}").contains("password"),
        "materialized secret in Debug output"
    );
    let loaded = value(state.settings);
    assert_eq!(loaded["providerInstances"]["opencode"]["enabled"], false);
    assert_eq!(
        loaded["providerInstances"]["opencode"]["config"]["serverPassword"],
        "password"
    );
    assert_eq!(loaded["providerInstances"]["codex"]["enabled"], false);
    assert_eq!(
        loaded["sourceControlHosts"]["bitbucket"]["accessToken"],
        "access"
    );
    assert_eq!(
        loaded["projectSettingsOverrides"]["p"]["defaultAutoPull"],
        true
    );
    let raw = std::fs::read_to_string(home.settings()).unwrap();
    let saved: Value = serde_json::from_str(&raw).unwrap();
    assert!(saved.get("providers").is_none());
    assert!(saved.get("bitbucket").is_none());
    assert!(!raw.contains("password"));
    assert!(!raw.contains("\"access\""));
    assert_eq!(saved["projectSettingsFolded"], true);
}
#[tokio::test]
async fn sqlite_history_and_project_settings_fold_once() {
    let temp = tempfile::tempdir().unwrap();
    let home = HomeLayout::derive(temp.path(), false, true);
    home.ensure_directories().unwrap();
    let db_path = home.database();
    let db = tokio::task::spawn_blocking(move || t3_persistence::Database::open(db_path))
        .await
        .unwrap()
        .unwrap();
    db.writer(|connection| {
        connection.execute("INSERT INTO projection_thread_sessions (thread_id,status,provider_name,provider_instance_id,updated_at) VALUES ('t','ready','cursor','cursor','now')",[])?;
        connection.execute("INSERT INTO projection_projects (project_id,title,workspace_root,scripts_json,created_at,updated_at,default_thread_env_mode,auto_pull) VALUES ('p','Project','/temporary','[]','now','now','worktree',1)",[])?;
        Ok(())
    }).await.unwrap();
    write(
        &home.settings(),
        r#"{"providers":{},"projectAutoPullOverrides":{"p":false}}"#,
    );
    let service = SettingsService::with_database(home.settings(), Some(db.clone()));
    let loaded = value(service.snapshot().await.unwrap());
    assert_eq!(loaded["providerInstances"]["cursor"]["enabled"], true);
    assert_eq!(
        loaded["projectSettingsOverrides"]["p"]["defaultAutoPull"],
        false
    );
    assert_eq!(
        loaded["projectSettingsOverrides"]["p"]["defaultThreadEnvMode"],
        "worktree"
    );
    service
        .update(&patch(json!({"projectSettingsOverrides":{"p":null}})))
        .await
        .unwrap();
    assert!(
        value(service.reload().await.unwrap().settings)["projectSettingsOverrides"]
            .get("p")
            .is_none()
    );
    drop(service);
    db.close().await.unwrap();
}
#[tokio::test]
async fn snapshot_excludes_provider_mutations() {
    let temp = tempfile::tempdir().unwrap();
    let home = HomeLayout::derive(temp.path(), false, true);
    let service = SettingsService::new(home.settings());
    let p = patch(json!({}));
    let mutation: ProviderInstanceMutation = serde_json::from_value(
        json!({"operation":"create","instanceId":"work","instance":{"driver":"codex","config":{}}}),
    )
    .unwrap();
    let mut update = std::pin::pin!(service.update_provider_instance(&mutation, &p));
    service.with_snapshot(async |_| {
        tokio::select! {
            biased;
            result = service.snapshot() => { result.unwrap(); },
            _ = async {} => panic!("cached reader blocked inside writer callback"),
        }
        tokio::select! { biased; _ = &mut update => panic!("mutation completed while snapshot was held"), _ = async {} => () }
        Ok(())
    }).await.unwrap();
    update.await.unwrap();
    assert!(
        value(service.snapshot().await.unwrap())["providerInstances"]
            .get("work")
            .is_some()
    );
}
#[tokio::test]
async fn duplicate_secret_rows_keep_explicit_replacements() {
    let temp = tempfile::tempdir().unwrap();
    let home = HomeLayout::derive(temp.path(), false, true);
    write(
        &home.settings(),
        r#"{"providerInstances":{"work":{"driver":"codex","config":{},"environment":[{"name":"TOKEN","value":"inline","sensitive":true}]}}}"#,
    );
    let service = SettingsService::new(home.settings());
    let next = service.update(&patch(json!({"providerInstances":{"work":{"driver":"codex","config":{},"environment":[{"name":"TOKEN","value":"replacement","sensitive":true},{"name":"TOKEN","value":"","sensitive":true,"valueRedacted":true}]}}}))).await.unwrap();
    assert_eq!(
        value(next)["providerInstances"]["work"]["environment"][0]["value"],
        "replacement"
    );
}
#[tokio::test]
async fn echoed_secret_references_survive_old_client_updates() {
    let temp = tempfile::tempdir().unwrap();
    let home = HomeLayout::derive(temp.path(), false, true);
    let service = SettingsService::new(home.settings());
    let reference = json!({"provider":"vault","reference":"op://a/b/c"});
    let generator = json!({"kind":"openaiCompatible","baseUrl":"https://example.com","apiKey":reference,"enabled":true});
    service.update(&patch(json!({"secretProviders":{"vault":{"kind":"one_password","label":"Vault","config":{"account":"work"}}},"textGenerators":{"local":generator}}))).await.unwrap();
    let mut old_client = value(service.snapshot().await.unwrap());
    redact_for_client(&mut old_client, false);
    assert_eq!(old_client["textGenerators"]["local"]["apiKey"], REDACTED);
    let next = value(
        service
            .update(&patch(
                json!({"textGenerators":{"local":old_client["textGenerators"]["local"]}}),
            ))
            .await
            .unwrap(),
    );
    assert_eq!(next["textGenerators"]["local"]["apiKey"], reference);
    let next = value(
        service
            .update(&patch(json!({"secretProviders":{"vault":null}})))
            .await
            .unwrap(),
    );
    assert_eq!(next["textGenerators"]["local"]["apiKey"], "");
}
#[test]
fn redaction_keeps_reference_envelopes_in_provider_environment() {
    let reference = json!({"provider":"vault","reference":"op://a/b/c"});
    let mut settings = json!({"providerInstances":{"work":{"driver":"codex","config":{"unknown":reference},"environment":[{"name":"TOKEN","value":"","sensitive":true,"secret":reference}]}},"secretProviders":{}});
    redact_for_client(&mut settings, false);
    assert_eq!(
        settings["providerInstances"]["work"]["environment"][0]["secret"],
        reference
    );
    assert_eq!(
        settings["providerInstances"]["work"]["config"]["unknown"],
        reference
    );
}
#[tokio::test(start_paused = true)]
async fn external_watch_invalid_then_valid() {
    let temp = tempfile::tempdir().unwrap();
    let path = HomeLayout::derive(temp.path(), false, true).settings();
    let service = SettingsService::new(&path);
    let _watch = service.start().await.unwrap();
    let mut events = service.subscribe_changes();
    write(&path, "{broken");
    let invalid = value(events.recv().await.unwrap());
    assert_eq!(invalid["continueThreadsAfterServerUpdate"], false);
    write(&path, r#"{"threadTitleInstructions":"from disk"}"#);
    loop {
        let event = value(events.recv().await.unwrap());
        if event["threadTitleInstructions"] == "from disk" {
            break;
        }
    }
}
#[tokio::test(start_paused = true)]
async fn symlinked_settings_keep_link_and_watch_target() {
    let temp = tempfile::tempdir().unwrap();
    let path = HomeLayout::derive(temp.path(), false, true).settings();
    let target = temp.path().join("external/settings.json");
    write(&target, "{}");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&target, &path).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let service = SettingsService::new(&path);
    service
        .update(&patch(json!({"threadTitleInstructions":"through link"})))
        .await
        .unwrap();
    assert!(
        std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        std::fs::read_to_string(&target)
            .unwrap()
            .contains("through link")
    );
    let _watch = service.start().await.unwrap();
    let mut events = service.subscribe_changes();
    write(&target, r#"{"threadTitleInstructions":"target edit"}"#);
    loop {
        if value(events.recv().await.unwrap())["threadTitleInstructions"] == "target edit" {
            break;
        }
    }
}
#[cfg(windows)]
#[tokio::test(start_paused = true)]
async fn failed_settings_commit_rolls_back_secret() {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::tempdir().unwrap();
    let home = HomeLayout::derive(temp.path(), false, true);
    let service = SettingsService::new(home.settings());
    let initial = patch(
        json!({"textGenerators":{"local":{"kind":"openaiCompatible","baseUrl":"https://example.com","apiKey":"before","enabled":true}}}),
    );
    service.update(&initial).await.unwrap();
    let _locked = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(home.settings())
        .unwrap();
    let p = patch(
        json!({"textGenerators":{"local":{"kind":"openaiCompatible","baseUrl":"https://example.com","apiKey":"after","enabled":true}}}),
    );
    assert!(service.update(&p).await.is_err());
    assert_eq!(
        SecretStore::new(home.secrets())
            .get("text-generator-bG9jYWw-YXBpS2V5")
            .unwrap()
            .unwrap(),
        b"before"
    );
}
