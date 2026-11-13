#![allow(clippy::unwrap_used)]
use serde_json::json;
use t3_contracts::{
    managed_policy::ManagedProviderLimits, provider_policy::RuntimeMode, settings::ServerSettings,
};
use t3_provider::{
    environment::{Environment, Platform},
    policy::*,
};

fn settings(value: serde_json::Value) -> ServerSettings {
    serde_json::from_value(value).unwrap()
}
fn limits(value: serde_json::Value) -> ManagedProviderLimits {
    serde_json::from_value(value).unwrap()
}

#[test]
fn windows_policy_locks_replace_all_spellings_and_restore_stored_values_on_echo() {
    let policy=ManagedProviders::new(&limits(json!({"instances":{"codex":{"locked":true,"binaryPath":"company-codex","launchArgs":"--locked","environment":[{"name":"HTTPS_PROXY","value":"gateway"},{"name":"CODEX_HOME","value":"corporate"}]}}})),None,Platform::Windows).unwrap();
    let stored = settings(
        json!({"providerInstances":{"work":{"driver":"codex","config":{"binaryPath":"own-codex","homePath":"own-home","launchArgs":"own-args"},"environment":[{"name":"https_proxy","value":"mine","sensitive":false},{"name":"Https_Proxy","value":"last","sensitive":false},{"name":"T3CODE_CODEX_LAUNCH_ARGS","value":"override","sensitive":false}]}}}),
    );
    let shown = policy.apply_limits(&stored);
    let work = &shown.provider_instances[&"work".to_owned().into()];
    assert_eq!(
        work.config.as_ref().unwrap()["binaryPath"],
        json!("company-codex")
    );
    assert!(work.config.as_ref().unwrap().get("homePath").is_none());
    let env = work.environment.as_ref().unwrap();
    assert_eq!(
        env.iter()
            .filter(|v| v.name.eq_ignore_ascii_case("HTTPS_PROXY"))
            .count(),
        1
    );
    assert!(!env.iter().any(|v| v.name == "T3CODE_CODEX_LAUNCH_ARGS"));
    let saved = policy.vet_write(&stored, &shown).unwrap();
    assert_eq!(saved.provider_instances, stored.provider_instances);
    let mut tampered = shown.clone();
    tampered
        .provider_instances
        .get_mut(&"work".to_owned().into())
        .unwrap()
        .config
        .as_mut()
        .unwrap()["binaryPath"] = json!("escape");
    assert!(matches!(
        policy.vet_write(&stored, &tampered),
        Err(WriteRefusal::Locked(_))
    ));
}

#[test]
fn ceiling_takes_launch_fields_and_whole_environment_and_allowlist_clears_text_generators() {
    let policy=ManagedProviders::new(&limits(json!({"allowedDrivers":["codex"],"instances":{"codex":{"locked":false,"binaryPath":"company-codex","environment":[{"name":"PATH","value":"trusted"}]}}})),Some(RuntimeMode::ApprovalRequired),Platform::Windows).unwrap();
    let stored = settings(
        json!({"providerInstances":{"work":{"driver":"codex","config":{"setupMode":"managed","shadowHomePath":"escape"},"environment":[{"name":"NODE_OPTIONS","value":"escape","sensitive":false}]}},"reviewGuideModelSelection":{"instanceId":"grok","model":"grok-build"}}),
    );
    let shown = policy.apply_limits(&stored);
    let work = &shown.provider_instances[&"work".to_owned().into()];
    assert!(work.config.as_ref().unwrap().get("setupMode").is_none());
    assert!(
        work.config
            .as_ref()
            .unwrap()
            .get("shadowHomePath")
            .is_none()
    );
    assert_eq!(work.environment.as_ref().unwrap().len(), 1);
    assert!(shown.review_guide_model_selection.is_none());
    assert!(
        policy.locked_instance_fields()[&"codex".to_owned().into()].contains(&"environment".into())
    );
    let mut tampered = shown.clone();
    tampered
        .provider_instances
        .get_mut(&"work".to_owned().into())
        .unwrap()
        .environment
        .as_mut()
        .unwrap()
        .push(
            serde_json::from_value(
                json!({"name":"NODE_OPTIONS","value":"other","sensitive":false}),
            )
            .unwrap(),
        );
    assert!(policy.vet_write(&stored, &tampered).is_err());
}

#[test]
fn unlocked_pins_fill_only_unset_fields_and_user_values_return_after_policy_removal() {
    let policy=ManagedProviders::new(&limits(json!({"instances":{"pi":{"locked":false,"binaryPath":"company-pi","launchArgs":"--corporate","environment":[{"name":"GATEWAY","value":"corporate"}]}}})),None,Platform::Linux).unwrap();
    let stored = settings(
        json!({"providerInstances":{"work":{"driver":"pi","config":{"binaryPath":"own-pi","launchArgs":""},"environment":[{"name":"GATEWAY","value":"mine","sensitive":false}]}}}),
    );
    let shown = policy.apply_limits(&stored);
    let config = shown.provider_instances[&"work".to_owned().into()]
        .config
        .as_ref()
        .unwrap();
    assert_eq!(config["binaryPath"], json!("own-pi"));
    assert_eq!(config["launchArgs"], json!("--corporate"));
    let saved = policy.vet_write(&stored, &shown).unwrap();
    assert!(
        saved.provider_instances[&"work".to_owned().into()]
            .config
            .as_ref()
            .unwrap()
            .get("launchArgs")
            .is_none()
    );
    assert_eq!(
        saved.provider_instances[&"work".to_owned().into()].environment,
        stored.provider_instances[&"work".to_owned().into()].environment
    );
}

#[test]
fn policy_rejects_unknown_drivers_unsupported_pins_duplicate_windows_names_and_new_disallowed_enables()
 {
    for bad in [
        json!({"allowedDrivers":["unknown"]}),
        json!({"instances":{"kiro":{"locked":true,"launchArgs":"unsupported"}}}),
        json!({"instances":{"codex":{"locked":true,"environment":[{"name":"PATH","value":"a"},{"name":"path","value":"b"}]}}}),
    ] {
        assert!(ManagedProviders::new(&limits(bad), None, Platform::Windows).is_err());
    }
    let policy = ManagedProviders::new(
        &limits(json!({"allowedDrivers":["codex"]})),
        None,
        Platform::Linux,
    )
    .unwrap();
    let stored = settings(json!({"providerInstances":{"work":{"driver":"grok","enabled":false}}}));
    let mut next = stored.clone();
    next.provider_instances
        .get_mut(&"work".to_owned().into())
        .unwrap()
        .enabled = Some(true);
    assert!(matches!(
        policy.vet_write(&stored, &next),
        Err(WriteRefusal::Disallowed(_))
    ));
    let next = settings(
        json!({"providerInstances":{"work":{"driver":"grok","enabled":false}},"projectSettingsOverrides":{"project":{"providerInstances":{"work":{"enabled":true}}}}}),
    );
    assert!(matches!(
        policy.vet_write(&stored, &next),
        Err(WriteRefusal::Disallowed(_))
    ));
}

#[test]
fn a_new_disallowed_instance_with_an_implicit_disabled_default_may_stay_configured() {
    let policy = ManagedProviders::new(
        &limits(json!({"allowedDrivers":["codex"]})),
        None,
        Platform::Linux,
    )
    .unwrap();
    assert!(
        policy
            .vet_write(
                &ServerSettings::default(),
                &settings(json!({"providerInstances":{"work":{"driver":"pi"}}}))
            )
            .is_ok()
    );
}

#[test]
fn managed_host_ignores_launch_overrides_and_modes_respect_explicit_policy() {
    let env = Environment::from([
        ("t3code_codex_launch_args".into(), "escape".into()),
        ("PATH".into(), "keep".into()),
    ]);
    assert_eq!(provider_host_environment(true, &env).len(), 1);
    assert_eq!(provider_host_environment(false, &env), env);
    assert_eq!(kiro_autopilot(RuntimeMode::FullAccess, false, false), "on");
    assert_eq!(kiro_autopilot(RuntimeMode::FullAccess, true, false), "off");
    assert_eq!(antigravity_mode(RuntimeMode::AutoAcceptEdits), "auto_edit");
    assert_eq!(antigravity_cli_mode("yolo"), "default");
    assert_eq!(
        t3_provider::fx::session_mode(RuntimeMode::Auto, false, false),
        "auto"
    );
    assert_eq!(
        t3_provider::fx::session_mode(RuntimeMode::Auto, false, true),
        "ask"
    );
    assert_eq!(
        t3_provider::fx::advertised_mode("auto", &["code".into()]),
        Some("code")
    );
}
