#![allow(clippy::unwrap_used)]
mod common;
use common::Fixture;
use serde_json::json;
use std::collections::BTreeMap;
use t3_contracts::{
    secret_reference::SecretReference,
    server::{ServerProviderAuthStatus, ServerProviderState},
};
use t3_provider::{
    discovery::*,
    environment::{Environment, Platform},
    fx::*,
    secrets::*,
    versions::*,
};

#[tokio::test]
async fn fx_reads_only_version_status_and_models_and_preserves_default_alias() {
    let fixture =
        Fixture::new(json!({"models":{"ids":["other","default","other","  third  ",""]}}));
    let provider = FxProvider {
        instance_id: "work".to_owned().into(),
        settings: FxSettings {
            enabled: true,
            binary_path: fixture.binary(),
            custom_models: vec![],
        },
        environment: fixture.environment.clone(),
    };
    let snapshot = provider.check("2026-11-13T00:00:00Z").await.unwrap();
    assert_eq!(snapshot.status, ServerProviderState::Ready);
    assert_eq!(
        snapshot.auth.status,
        ServerProviderAuthStatus::Authenticated
    );
    assert_eq!(snapshot.version.unwrap().0, "0.0.13");
    assert_eq!(default_model_id(&snapshot.models), Some("meta/muse"));
    assert_eq!(
        snapshot
            .models
            .iter()
            .map(|m| m.slug.0.as_str())
            .collect::<Vec<_>>(),
        ["default", "other", "third"]
    );
    assert_eq!(
        fixture.calls(),
        vec![
            vec!["--version"],
            vec!["status", "--json"],
            vec!["models", "--json"]
        ]
    );
    assert_eq!(snapshot.supports_text_generation, Some(Some(false)));
}

#[tokio::test]
async fn fx_distinguishes_signed_out_unknown_login_and_failed_model_listing() {
    for (config, state, auth) in [
        (
            json!({"status":{"auth":"missing"}}),
            ServerProviderState::Error,
            ServerProviderAuthStatus::Unauthenticated,
        ),
        (
            json!({"status":{"auth":"  "}}),
            ServerProviderState::Warning,
            ServerProviderAuthStatus::Unknown,
        ),
        (
            json!({"modelsExit":1}),
            ServerProviderState::Warning,
            ServerProviderAuthStatus::Authenticated,
        ),
        (
            json!({"models":{"ids":null}}),
            ServerProviderState::Warning,
            ServerProviderAuthStatus::Authenticated,
        ),
        (
            json!({"status":{"auth":"oauth","model":null}}),
            ServerProviderState::Warning,
            ServerProviderAuthStatus::Unknown,
        ),
        (
            json!({"versionExit":1,"version":"wrapper 9.9.9"}),
            ServerProviderState::Error,
            ServerProviderAuthStatus::Unknown,
        ),
    ] {
        let fixture = Fixture::new(config);
        let provider = FxProvider {
            instance_id: "fx".to_owned().into(),
            settings: FxSettings {
                enabled: true,
                binary_path: fixture.binary(),
                custom_models: vec![],
            },
            environment: fixture.environment.clone(),
        };
        let snapshot = provider.check("2026-11-13T00:00:00Z").await.unwrap();
        assert_eq!(snapshot.status, state);
        assert_eq!(snapshot.auth.status, auth);
        if fixture.calls().len() == 1 {
            assert!(snapshot.version.is_none());
        }
    }
}

#[tokio::test]
async fn disabled_fx_never_spawns_a_process() {
    let fixture = Fixture::new(json!({}));
    let provider = FxProvider {
        instance_id: "fx".to_owned().into(),
        settings: FxSettings {
            enabled: false,
            binary_path: fixture.binary(),
            custom_models: vec![],
        },
        environment: fixture.environment.clone(),
    };
    assert_eq!(
        provider.check("2026-11-13T00:00:00Z").await.unwrap().status,
        ServerProviderState::Disabled
    );
    assert!(fixture.calls().is_empty());
}

#[tokio::test]
async fn real_process_probes_parse_provider_specific_versions_and_minimum_gates() {
    for (driver, output, expected, status) in [
        (
            Driver::Codex,
            "codex-cli 1.2.3-beta.1",
            "1.2.3-beta.1",
            ServerProviderState::Warning,
        ),
        (
            Driver::Claude,
            "Claude Code v2.0.22",
            "2.0.22",
            ServerProviderState::Warning,
        ),
        (Driver::Pi, "0.80.4", "0.80.4", ServerProviderState::Error),
        (
            Driver::OpenCode,
            "2.2.5",
            "2.2.5",
            ServerProviderState::Warning,
        ),
        (
            Driver::Muse,
            "muse 1.4.6-R11.2",
            "1.4.6-R11.2",
            ServerProviderState::Warning,
        ),
    ] {
        let fixture = Fixture::new(json!({"version":output}));
        let result = probe_version(driver, Some(&fixture.binary()), &fixture.environment, None)
            .await
            .unwrap();
        assert!(result.installed);
        assert_eq!(result.version.as_deref(), Some(expected));
        assert_eq!(result.status, status);
        assert_eq!(fixture.calls(), vec![vec!["--version"]]);
    }
}

#[test]
fn sdk_and_managed_providers_are_never_misrepresented_as_cli_probes() {
    assert_eq!(Driver::Cursor.discovery_route(), DiscoveryRoute::CursorSdk);
    assert_eq!(
        Driver::Antigravity.discovery_route(),
        DiscoveryRoute::ManagedAcp
    );
    assert_eq!(
        Driver::AcpRegistry.discovery_route(),
        DiscoveryRoute::RegistryDistribution
    );
    for driver in Driver::ALL {
        assert_eq!(Driver::from_kind(driver.kind()), Some(driver));
    }
    assert!(is_windows_command_not_found(
        Platform::Windows,
        Some(9009),
        ""
    ));
    assert!(!is_windows_command_not_found(
        Platform::Linux,
        Some(9009),
        ""
    ));
    for message in [
        "n'est pas reconnu en tant que commande interne",
        "não é reconhecido como um comando interno",
        "non è riconosciuto come comando interno o esterno",
    ] {
        assert!(is_windows_command_not_found(
            Platform::Windows,
            Some(1),
            message
        ));
    }
}

#[test]
fn probe_timeout_overrides_only_extend_integer_node_timer_delays() {
    for (value, expected) in [
        ("500", 1000),
        ("1000", 1000),
        ("2000", 2000),
        ("2000.5", 1000),
        ("2147483648", 1000),
        ("NaN", 1000),
        ("2e3", 2000),
    ] {
        let env = Environment::from([("T3CODE_FX_PROBE_TIMEOUT_MS".into(), value.into())]);
        assert_eq!(probe_timeout_ms("fx", 1000, &env), expected);
    }
}

#[tokio::test]
async fn one_password_batches_deduplicated_references_and_retains_verbatim_whitespace() {
    let fixture = Fixture::new(
        json!({"secrets":{"op://vault/item/one":" first \n","op://vault/item/two":"second"}}),
    );
    let resolver = SecretResolver::new(OnePassword::with_executable(
        fixture.binary(),
        fixture.environment.clone(),
    ));
    let providers=serde_json::from_value(json!({"work":{"kind":"one_password","label":"Work","config":{"account":"company.1password.com"}}})).unwrap();
    let references:Vec<SecretReference>=serde_json::from_value(json!([{"provider":"work","reference":"op://vault/item/one"},{"provider":"work","reference":"op://vault/item/two"},{"provider":"work","reference":"op://vault/item/one"}])).unwrap();
    let values = resolver.read(&providers, &references, false).await;
    assert_eq!(
        values,
        [
            Some(" first \n".into()),
            Some("second".into()),
            Some(" first \n".into())
        ]
    );
    assert_eq!(resolver.read(&providers, &references, false).await, values);
    let calls = fixture.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        &calls[0][..3],
        ["--cache=false", "inject", "--account=company.1password.com"]
    );
    assert!(!std::path::Path::new(&calls[0][4]).exists());
}

#[tokio::test]
async fn unreadable_fresh_secret_does_not_replace_a_good_cached_value_and_config_changes_re_read() {
    let fixture = Fixture::new(json!({"secrets":{"op://vault/item/one":"good"}}));
    let resolver = SecretResolver::new(OnePassword::with_executable(
        fixture.binary(),
        fixture.environment.clone(),
    ));
    let mut providers: BTreeMap<_, t3_contracts::settings::SecretProviderConfig> =
        serde_json::from_value(
            json!({"work":{"kind":"one_password","label":"Work","config":{"account":"work"}}}),
        )
        .unwrap();
    let reference: SecretReference =
        serde_json::from_value(json!({"provider":"work","reference":"op://vault/item/one"}))
            .unwrap();
    let references = [reference];
    assert_eq!(
        resolver.read(&providers, &references, false).await,
        [Some("good".into())]
    );
    fixture.write(json!({"secrets":{}}));
    assert_eq!(resolver.read(&providers, &references, true).await, [None]);
    assert_eq!(
        resolver.read(&providers, &references, false).await,
        [Some("good".into())]
    );
    providers.values_mut().next().unwrap().config = Some(json!({"account":"other"}));
    assert_eq!(resolver.read(&providers, &references, false).await, [None]);
    assert_eq!(fixture.calls().len(), 3);
    resolver.invalidate(None).await;
    assert_eq!(resolver.read(&providers, &references, false).await, [None]);
    assert_eq!(fixture.calls().len(), 4);
}

#[tokio::test]
async fn failed_injection_falls_back_to_single_reads() {
    let fixture =
        Fixture::new(json!({"injectExit":1,"secrets":{"op://v/i/a":"one","op://v/i/b":"two"}}));
    let driver = OnePassword::with_executable(fixture.binary(), fixture.environment.clone());
    assert_eq!(
        driver
            .read("work", &["op://v/i/a".into(), "op://v/i/b".into()])
            .await
            .unwrap(),
        [Some("one".into()), Some("two".into())]
    );
    assert_eq!(fixture.calls().len(), 3);
}

#[test]
fn one_password_validation_rejects_template_injection_and_preserves_batch_boundaries() {
    for value in ["--evil", "bad account", "a/b", ""] {
        assert!(!valid_one_password_account(value));
    }
    for value in [
        "op://v/i/a}}",
        "op://v/i/a\n{{op://v/i/b}}",
        "op://v/i",
        "op://v/i/a?bad=space value",
    ] {
        assert!(!valid_one_password_reference(value));
    }
    assert!(valid_one_password_reference(
        "op://Private/OpenAI/api key?attribute=password"
    ));
    assert_eq!(
        parse_injected_secrets(" one __sep__two__sep__\n", "__sep__", 2),
        Some(vec![Some(" one ".into()), Some("two".into())])
    );
    assert!(parse_injected_secrets("one__sep__two", "__sep__", 1).is_none());
}

#[tokio::test]
async fn credentials_are_isolated_by_length_prefixed_driver_binding_tuple() {
    let temp = tempfile::tempdir().unwrap();
    let store = t3_config::secrets::SecretStore::new(temp.path());
    let a = CredentialStore::new(store.clone(), "ab", "c");
    let b = CredentialStore::new(store.clone(), "a", "bc");
    assert_ne!(a.key(), b.key());
    assert!(!credential_binding_key("codex", "../../outside").contains('/'));
    a.set(b"secret").await.unwrap();
    assert_eq!(a.get().unwrap(), Some(b"secret".to_vec()));
    assert_eq!(b.get().unwrap(), None);
    a.remove().unwrap();
    assert_eq!(a.get().unwrap(), None);
}

#[tokio::test]
#[ignore = "opt-in installed binary version probe; never starts a turn"]
async fn installed_codex_version_probe() {
    let environment = std::env::vars().collect();
    let result = probe_version(Driver::Codex, None, &environment, None)
        .await
        .unwrap();
    assert!(result.installed);
    assert!(result.version.is_some());
}
