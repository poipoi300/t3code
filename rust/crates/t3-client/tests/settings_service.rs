#![allow(clippy::unwrap_used)]
//! The settings pages' service against the fake server: scope views from the store, device
//! writes, `server.updateSettings` writes, permission refusals, rejections and older servers.
#[path = "support/fake_server.rs"]
mod fake_server;

use futures::FutureExt;
use serde_json::{Value, json};
use std::rc::Rc;
use t3_client::navigation::SettingsPage;
use t3_client::services::settings::scope::{ScopeKindName, SettingsScopeSearch};
use t3_client::services::settings::service::SettingsWriteAccess;

fn settings_runtime(
    permissions: &[&str],
    settings: Value,
) -> (
    fake_server::Harness,
    t3_client::composition::ClientRuntime<fake_server::TestPlatform>,
) {
    let mut h = fake_server::Harness::new("env");
    h.server().state.borrow_mut().permissions =
        Some(permissions.iter().map(|p| (*p).to_owned()).collect());
    // The settings pages also list text generator models.
    h.server().state.borrow_mut().extra_methods =
        vec![("server.listTextGeneratorModels", t3_rpc::CallKind::Unary)];
    let mut config = fake_server::config("env");
    let mut base = serde_json::to_value(t3_contracts::settings::ServerSettings::default()).unwrap();
    base.as_object_mut()
        .unwrap()
        .extend(settings.as_object().unwrap().clone());
    config["settings"] = base;
    let source = config.clone();
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
        let config = source.clone();
        match request.tag.as_str() {
            "server.getConfig" => Some(t3_rpc::HandlerOutput::Unary(
                async move { t3_rpc::Exit::success(config) }.boxed_local(),
            )),
            "subscribeServerConfig" => Some(t3_rpc::HandlerOutput::Stream(
                futures::stream::once(async move {
                    Ok(vec![json!({"version":1,"type":"snapshot","config":config})])
                })
                .chain(futures::stream::pending())
                .boxed_local(),
            )),
            _ => None,
        }
    }));
    let runtime = t3_client::composition::ClientRuntime::new(
        h.platform.clone(),
        t3_client::ClientStore::default(),
        t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web,
        h.spawner(),
    );
    h.run(runtime.pair(t3_client::connection::PairingInput {
        pairing_url: Some(format!("{}/pair#token=pairing-token", fake_server::ORIGIN)),
        ..Default::default()
    }))
    .unwrap();
    h.settle();
    // Session grants are read when a connection authenticates, as composer_services.rs does.
    h.run(runtime.reconnect(&t3_client::types::EnvironmentId("env".into())))
        .unwrap();
    h.settle();
    (h, runtime)
}

use futures::StreamExt;

fn patch(value: Value) -> serde_json::Map<String, Value> {
    value.as_object().unwrap().clone()
}

fn update_calls(h: &fake_server::Harness) -> Vec<Value> {
    h.server()
        .state
        .borrow()
        .rpc_calls
        .iter()
        .filter(|(tag, _)| tag == "server.updateSettings")
        .map(|(_, input)| input["patch"].clone())
        .collect()
}

#[test]
fn review_device_preference_types_round_trip_and_reject_bad_storage_values() {
    use t3_client::platform::{Storage, StorageArea, StorageKey, StorageWrite};

    const ADVANCED: &str = "t3code:typography-advanced";
    const THEME: &str = "t3code:theme";
    const MODE: &str = "t3code:theme-appearance-mode";
    let (mut h, runtime) = settings_runtime(&["orchestration:read"], json!({}));
    let settings = runtime.settings();
    for (key, value) in [
        (ADVANCED, json!(true)),
        (THEME, json!("system")),
        (MODE, json!("dark")),
        ("t3code:themes:v1", json!([{"future": 42}])),
        ("t3code:theme-halves:v1", json!({"dark": "ocean"})),
        ("t3code:theme-follow-system", json!(false)),
    ] {
        h.run(settings.write_device_preference(key, value.clone()))
            .unwrap();
        assert_eq!(h.run(settings.read_device_preference(key)), Some(value));
    }
    assert!(matches!(
        h.run(settings.write_device_preference(THEME, json!(false))),
        Err(t3_client::platform::PlatformError::InvalidData(_))
    ));
    assert_eq!(
        h.run(settings.read_device_preference(THEME)),
        Some(json!("system"))
    );
    let platform = h.platform.clone();
    h.run(async move {
        platform
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: StorageKey::new("ui-preference", None, ADVANCED),
                    value: Some(br#""true""#.to_vec()),
                }],
            )
            .await
    })
    .unwrap();
    assert_eq!(h.run(settings.read_device_preference(ADVANCED)), None);
}

#[test]
fn themes_read_imported_web_records_and_clearing_a_mix_survives_relaunch() {
    use t3_client::{
        persistence::{read_web_storage, save_record},
        platform::StorageArea,
    };
    let (mut h, runtime) = settings_runtime(&["orchestration:read"], json!({}));
    let settings = runtime.settings();
    for (key, raw, value) in [
        (
            "t3code:theme-halves:v1",
            r#"{"dark":"ocean"}"#,
            json!({"dark":"ocean"}),
        ),
        (
            "t3code:themes:v1",
            r#"[{"future":42}]"#,
            json!([{"future":42}]),
        ),
        ("t3code:theme-follow-system", "false", json!(false)),
        ("t3code:theme", "ocean", json!("ocean")),
    ] {
        let imported = read_web_storage(key, raw.as_bytes(), false)
            .unwrap()
            .pop()
            .unwrap();
        let platform = h.platform.clone();
        h.run(async move { save_record(&platform, StorageArea::Ordinary, &imported.record).await })
            .unwrap();
        assert_eq!(h.run(settings.read_device_preference(key)), Some(value));
    }
    const HALVES: &str = "t3code:theme-halves:v1";
    const LIBRARY: &str = "t3code:themes:v1";
    assert!(
        h.run(settings.write_device_preference(HALVES, json!({"light":false})))
            .is_err()
    );
    let read_theme = |key| {
        let settings = settings.clone();
        async move { settings.read_theme_preference(key).await }
    };
    let (_, mix_read) = h.run(read_theme(HALVES)).unwrap();
    assert!(
        h.run(settings.write_theme_preferences(
            vec![(HALVES, None), (LIBRARY, Some(json!("invalid")))],
            mix_read.clone()
        ))
        .is_err()
    );
    assert_eq!(
        h.run(settings.read_device_preference(HALVES)),
        Some(json!({"dark":"ocean"}))
    );
    assert!(
        h.run(settings.write_theme_preferences(
            vec![(HALVES, None), ("t3code:theme", Some(json!("system")))],
            mix_read.clone()
        ))
        .unwrap()
    );
    assert!(
        !h.run(settings.write_theme_preferences(
            vec![(HALVES, Some(json!({"dark":"ocean","light":"grove"})))],
            mix_read
        ))
        .unwrap(),
        "a stale half edit must be rebased before it can overwrite the other half"
    );
    let (_, library_read) = h.run(read_theme(LIBRARY)).unwrap();
    h.run(settings.write_device_preference(LIBRARY, json!([{"future":43}])))
        .unwrap();
    assert!(
        !h.run(settings.write_theme_preferences(vec![(LIBRARY, Some(json!([])))], library_read))
            .unwrap(),
        "a stale writer must not overwrite another window's library"
    );
    let reopened = runtime.settings();
    assert_eq!(h.run(reopened.read_device_preference(HALVES)), None);
    assert_eq!(
        h.run(reopened.read_device_preference("t3code:theme")),
        Some(json!("system"))
    );
    assert_eq!(
        h.run(reopened.read_device_preference(LIBRARY)),
        Some(json!([{"future":43}]))
    );
}

#[test]
fn review_device_preference_writes_surface_storage_errors() {
    let (mut h, runtime) = settings_runtime(&["orchestration:read"], json!({}));
    h.server().state.borrow_mut().fail_device_preference_write = true;
    let write = h.run(
        runtime
            .settings()
            .write_device_preference("t3code:theme", json!("system")),
    );
    assert!(matches!(
        write,
        Err(t3_client::platform::PlatformError::Storage(_))
    ));
}

#[test]
fn the_scope_view_reads_environments_and_project_groups_from_the_store() {
    let (_h, runtime) = settings_runtime(&["orchestration:read", "settings:write"], json!({}));
    let settings = runtime.settings();
    let view = settings.scope_view(SettingsPage::Threads, &SettingsScopeSearch::default());
    assert_eq!(view.scope.kind_name(), ScopeKindName::All);
    assert_eq!(view.selected.connected.len(), 1);
    assert_eq!(view.groups.len(), 1);
    assert_eq!(view.groups[0].display_name, "Project");
    assert_eq!(view.targets.len(), 1);
    assert!(!view.device_only && view.page == SettingsPage::Threads);
    assert_eq!(settings.write_access(&view), SettingsWriteAccess::Writable);
    let project = SettingsScopeSearch {
        project: Some(view.groups[0].project_key.clone()),
        ..Default::default()
    };
    let scoped = settings.scope_view(SettingsPage::Threads, &project);
    assert_eq!(scoped.scope.kind_name(), ScopeKindName::Project);
    assert_eq!(scoped.targets[0].project_id.as_ref().unwrap().0, "project");
    let stale = SettingsScopeSearch {
        machine: Some("gone".into()),
        ..Default::default()
    };
    let unavailable = settings.scope_view(SettingsPage::Threads, &stale);
    assert_eq!(unavailable.scope.kind_name(), ScopeKindName::Unavailable);
    assert!(unavailable.targets.is_empty());
}

#[test]
fn device_settings_save_to_the_client_record_without_a_server_call() {
    let (mut h, runtime) = settings_runtime(&["orchestration:read", "settings:write"], json!({}));
    let settings = runtime.settings();
    let mut changes = settings.changes();
    let view = settings.scope_view(SettingsPage::General, &SettingsScopeSearch::default());
    let before = settings.client_settings().confirm_thread_delete;
    let outcome = h.run(settings.update(&view, patch(json!({"confirmThreadDelete": !before}))));
    assert!(!outcome.failed && outcome.toast.is_none());
    assert_eq!(settings.client_settings().confirm_thread_delete, !before);
    assert!(changes.next().now_or_never().is_some());
    assert!(update_calls(&h).is_empty());
    // A second service reads the same record: there is one authority.
    assert_eq!(
        runtime.settings().client_settings().confirm_thread_delete,
        !before
    );
}

#[test]
fn server_settings_save_through_update_settings_and_report_refusals() {
    let (mut h, runtime) = settings_runtime(&["orchestration:read", "settings:write"], json!({}));
    let settings = runtime.settings();
    let view = settings.scope_view(SettingsPage::Threads, &SettingsScopeSearch::default());
    let saved = view.server_settings();
    let mut saved_json = serde_json::to_value(saved.as_ref()).unwrap();
    saved_json["autoResumeLimitedThreads"] = json!(true);
    h.server()
        .state
        .borrow_mut()
        .rpc_script
        .entry("server.updateSettings".into())
        .or_default()
        .push_back(Ok(saved_json));
    let outcome = h.run(settings.update(&view, patch(json!({"autoResumeLimitedThreads": true}))));
    assert!(!outcome.failed, "{outcome:?}");
    assert_eq!(outcome.saved_environments, vec!["Fake host".to_string()]);
    assert_eq!(
        update_calls(&h),
        vec![json!({"autoResumeLimitedThreads": true})]
    );

    // A server that answers without the key is an older server that dropped it.
    let unchanged = serde_json::to_value(saved.as_ref()).unwrap();
    h.server()
        .state
        .borrow_mut()
        .rpc_script
        .entry("server.updateSettings".into())
        .or_default()
        .push_back(Ok(unchanged));
    let dropped = h.run(settings.update(&view, patch(json!({"autoResumeLimitedThreads": true}))));
    let toast = dropped.toast.unwrap();
    assert!(dropped.failed && toast.error);
    assert_eq!(toast.title.key, "Setting not saved");
    assert_eq!(
        toast.description[0].english(),
        "Could not save on Fake host: This environment does not support this setting. Update T3 Code on it."
    );

    h.server()
        .state
        .borrow_mut()
        .rpc_script
        .entry("server.updateSettings".into())
        .or_default()
        .push_back(Err(json!({"_tag":"ServerSettingsError","settingsPath":"settings.json","operation":"normalize","message":"Settings refused","detail":"read-only mount"})));
    let rejected = h.run(settings.update(&view, patch(json!({"autoResumeLimitedThreads": false}))));
    let description = rejected.toast.unwrap().description[0].english();
    assert!(
        description.starts_with("Could not save on Fake host: "),
        "{description}"
    );
    assert!(description.contains("read-only mount"), "{description}");
}

#[test]
fn a_connection_without_the_settings_grant_is_refused_before_sending() {
    let (mut h, runtime) = settings_runtime(&["orchestration:read"], json!({}));
    let settings = runtime.settings();
    let view = settings.scope_view(SettingsPage::Threads, &SettingsScopeSearch::default());
    assert_eq!(
        settings.write_access(&view),
        SettingsWriteAccess::Denied {
            labels: "Fake host".into()
        }
    );
    let outcome = h.run(settings.update(&view, patch(json!({"autoResumeLimitedThreads": true}))));
    assert!(outcome.failed);
    assert_eq!(
        outcome.toast.unwrap().description[0].english(),
        "Could not save on Fake host: This connection lacks permission to change these settings."
    );
    assert!(update_calls(&h).is_empty());
}

#[test]
fn project_scope_writes_overrides_and_refuses_device_settings() {
    let (mut h, runtime) = settings_runtime(
        &["orchestration:read", "settings:write"],
        json!({"projectSettingsOverrides": {"project": {"responseStreamingMode": "paragraph"}}}),
    );
    let settings = runtime.settings();
    let all = settings.scope_view(SettingsPage::Threads, &SettingsScopeSearch::default());
    let view = settings.scope_view(
        SettingsPage::Threads,
        &SettingsScopeSearch {
            project: Some(all.groups[0].project_key.clone()),
            ..Default::default()
        },
    );
    assert_eq!(view.scope.kind_name(), ScopeKindName::Project);
    let refused = h.run(settings.update(&view, patch(json!({"confirmThreadDelete": false}))));
    assert_eq!(
        refused.toast.unwrap().description[0].key,
        "This setting is environment-wide and cannot be overridden by a project."
    );
    let current = serde_json::to_value(view.server_settings().as_ref()).unwrap();
    let mut saved = serde_json::to_value(all.server_settings().as_ref()).unwrap();
    saved["projectSettingsOverrides"]["project"]["sidebarAutoSettleOnMerge"] = json!(true);
    assert_eq!(current["responseStreamingMode"], "paragraph");
    h.server()
        .state
        .borrow_mut()
        .rpc_script
        .entry("server.updateSettings".into())
        .or_default()
        .push_back(Ok(saved));
    let outcome = h.run(settings.update(&view, patch(json!({"sidebarAutoSettleOnMerge": true}))));
    assert!(!outcome.failed, "{outcome:?}");
    assert_eq!(
        update_calls(&h),
        vec![
            json!({"projectSettingsOverrides": {"project": {"responseStreamingMode": "paragraph", "sidebarAutoSettleOnMerge": true}}})
        ]
    );
}

#[test]
fn the_last_enabled_grouping_mode_is_remembered() {
    let (mut h, runtime) = settings_runtime(&["orchestration:read"], json!({}));
    let settings = runtime.settings();
    assert_eq!(h.run(settings.last_enabled_grouping_mode()), "repository");
    h.run(settings.remember_enabled_grouping_mode("repository_path"));
    h.run(settings.remember_enabled_grouping_mode("separate"));
    assert_eq!(
        h.run(settings.last_enabled_grouping_mode()),
        "repository_path"
    );
}

#[test]
fn text_generator_models_load_into_the_picker_targets() {
    let (mut h, runtime) = settings_runtime(
        &["orchestration:read", "settings:write", "providers:manage"],
        json!({"textGenerators": {"work": {"kind": "anthropic", "baseUrl": "https://api.anthropic.com/v1", "apiKey": "", "enabled": true}}}),
    );
    h.server()
        .state
        .borrow_mut()
        .rpc_script
        .entry("server.listTextGeneratorModels".into())
        .or_default()
        .push_back(Ok(
            json!({"models": [{"id": "claude-small", "name": "Claude Small"}]}),
        ));
    let settings = runtime.settings();
    let mut changes = settings.changes();
    let view = settings.scope_view(SettingsPage::Threads, &SettingsScopeSearch::default());
    let loading = settings.text_generation_targets(&view);
    let generator = loading
        .providers
        .iter()
        .find(|p| p.instance_id.0 == "work")
        .expect("the generator stands in as a provider");
    assert!(generator.models.is_empty(), "its list is still loading");
    h.settle();
    assert!(
        changes.next().now_or_never().is_some(),
        "the list's arrival is a change"
    );
    let loaded = settings.text_generation_targets(&view);
    let generator = loaded
        .providers
        .iter()
        .find(|p| p.instance_id.0 == "work")
        .expect("generator");
    assert_eq!(generator.models[0].slug.0, "claude-small");
    assert_eq!(
        h.server()
            .state
            .borrow()
            .rpc_calls
            .iter()
            .filter(|(tag, _)| tag == "server.listTextGeneratorModels")
            .count(),
        1,
        "the list is fetched once and then read from the cache"
    );
}
