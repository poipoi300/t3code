#![allow(clippy::unwrap_used)]
mod common;
use common::Fixture;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use t3_contracts::{provider_instance::*, settings::ServerSettings};
use t3_provider::{instances::*, secrets::*};

fn settings(value: serde_json::Value) -> ServerSettings {
    serde_json::from_value(value).unwrap()
}
fn prepared(entry: &RegistryEntry) -> Arc<InstanceSpec> {
    match &entry.status {
        InstanceStatus::Prepared(spec) => spec.clone(),
        _ => panic!("expected prepared instance"),
    }
}

#[test]
fn explicit_disable_wins_over_both_raw_and_decoded_configuration() {
    let mut entry: ProviderInstanceConfig =
        serde_json::from_value(json!({"driver":"codex","enabled":true,"config":{"enabled":false}}))
            .unwrap();
    assert!(!resolve_enabled(&entry, &json!({"enabled":true})));
    entry.enabled = Some(false);
    entry.config = Some(json!({"enabled":true}));
    assert!(!resolve_enabled(&entry, &json!({"enabled":true})));
    entry.enabled = None;
    entry.config = None;
    assert!(resolve_enabled(&entry, &json!({})));
    assert!(!resolve_enabled(&entry, &json!({"enabled":false})));
}

#[test]
fn hydration_adds_defaults_but_no_registry_and_probes_project_enabled_instances() {
    let settings = settings(
        json!({"providerInstances":{"work":{"driver":"codex","enabled":false,"config":{"enabled":false}}},"projectSettingsOverrides":{"project":{"providerInstances":{"work":{"enabled":true},"missing":{"enabled":true}}}}}),
    );
    let map = derive_config_map(&settings);
    assert_eq!(map.len(), 11);
    assert!(!map.contains_key(&ProviderInstanceId("acpRegistry".into())));
    assert!(!map.contains_key(&ProviderInstanceId("missing".into())));
    let work = &map[&ProviderInstanceId("work".into())];
    assert_eq!(work.enabled, Some(true));
    assert_eq!(work.config.as_ref().unwrap()["enabled"], json!(true));
}

#[test]
fn project_enablement_overrides_a_drivers_implicit_disabled_default() {
    let current = settings(
        json!({"providerInstances":{"work":{"driver":"pi"}},"projectSettingsOverrides":{"project":{"providerInstances":{"work":{"enabled":true}}}}}),
    );
    assert_eq!(
        derive_config_map(&current)[&ProviderInstanceId("work".into())].enabled,
        Some(true)
    );
}

#[tokio::test]
async fn reconciliation_preserves_independent_instances_and_reports_replacements_and_removals() {
    let registry = InstanceRegistry::new(core_driver_definitions());
    let fixture = Fixture::new(json!({}));
    let resolver = SecretResolver::new(OnePassword::with_executable(
        fixture.binary(),
        fixture.environment.clone(),
    ));
    let mut settings = settings(
        json!({"providerInstances":{"work":{"driver":"codex","enabled":true},"personal":{"driver":"codex","enabled":true}}}),
    );
    let mut subscription = registry.subscribe().await;
    let changes = registry
        .reconcile(
            &settings,
            &resolver,
            &BTreeSet::new(),
            false,
            &BTreeMap::new(),
        )
        .await;
    subscription.recv().await.unwrap();
    assert_eq!(changes.prepared.len(), 12);
    let work_id = ProviderInstanceId("work".into());
    let personal_id = ProviderInstanceId("personal".into());
    let work = prepared(&registry.get(&work_id).await.unwrap());
    let personal = prepared(&registry.get(&personal_id).await.unwrap());
    assert_ne!(
        work.default_continuation_key(),
        personal.default_continuation_key()
    );
    let no_changes = registry
        .reconcile(
            &settings,
            &resolver,
            &BTreeSet::new(),
            false,
            &BTreeMap::new(),
        )
        .await;
    assert!(no_changes.prepared.is_empty());
    assert!(Arc::ptr_eq(
        &work,
        &prepared(&registry.get(&work_id).await.unwrap())
    ));
    settings
        .provider_instances
        .get_mut(&work_id)
        .unwrap()
        .display_name = Some(Some("Renamed".into()));
    settings.provider_instances.remove(&personal_id);
    let changes = registry
        .reconcile(
            &settings,
            &resolver,
            &BTreeSet::new(),
            false,
            &BTreeMap::new(),
        )
        .await;
    assert!(changes.retired.contains(&work_id));
    assert!(changes.retired.contains(&personal_id));
    assert!(changes.prepared.contains(&work_id));
    assert!(fixture.calls().is_empty());
}

#[tokio::test]
async fn disabled_instances_skip_vaults_and_enabled_instances_park_on_blank_credentials() {
    let fixture = Fixture::new(json!({"secrets":{"op://vault/item/field":"   "}}));
    let resolver = SecretResolver::new(OnePassword::with_executable(
        fixture.binary(),
        fixture.environment.clone(),
    ));
    let registry = InstanceRegistry::new(core_driver_definitions());
    let mut settings = settings(
        json!({"providerInstances":{"work":{"driver":"codex","enabled":false,"environment":[{"name":"OPENAI_API_KEY","value":"","sensitive":true,"secret":{"provider":"vault","reference":"op://vault/item/field"}}]}},"secretProviders":{"vault":{"kind":"one_password","label":"Vault","config":{"account":"work"}}}}),
    );
    registry
        .reconcile(
            &settings,
            &resolver,
            &BTreeSet::new(),
            false,
            &BTreeMap::new(),
        )
        .await;
    let id = ProviderInstanceId("work".into());
    let spec = prepared(&registry.get(&id).await.unwrap());
    assert!(!spec.enabled);
    assert!(spec.variables.is_empty());
    assert!(fixture.calls().is_empty());
    settings.provider_instances.get_mut(&id).unwrap().enabled = Some(true);
    registry
        .reconcile(
            &settings,
            &resolver,
            &BTreeSet::new(),
            false,
            &BTreeMap::new(),
        )
        .await;
    assert!(matches!(
        registry.get(&id).await.unwrap().status,
        InstanceStatus::Unavailable(UnavailableReason::UnreadableSecrets(_))
    ));
    assert_eq!(fixture.calls().len(), 1);
    registry
        .reconcile(
            &settings,
            &resolver,
            &BTreeSet::new(),
            false,
            &BTreeMap::new(),
        )
        .await;
    assert_eq!(fixture.calls().len(), 1);
    fixture.write(json!({"secrets":{"op://vault/item/field":" key "}}));
    resolver.invalidate(None).await;
    registry
        .reconcile(
            &settings,
            &resolver,
            &BTreeSet::from([id.clone()]),
            false,
            &BTreeMap::new(),
        )
        .await;
    let spec = prepared(&registry.get(&id).await.unwrap());
    assert_eq!(spec.variables[0].value, " key ");
    assert!(spec.variables[0].sensitive);
}

#[tokio::test]
async fn config_field_secrets_are_resolved_and_deferred_or_invalid_instances_never_probe() {
    let fixture = Fixture::new(json!({"secrets":{"op://vault/item/password":"secret-value"}}));
    let resolver = SecretResolver::new(OnePassword::with_executable(
        fixture.binary(),
        fixture.environment.clone(),
    ));
    let registry = InstanceRegistry::new(core_driver_definitions());
    let settings = settings(
        json!({"providerInstances":{"server":{"driver":"opencode","enabled":true,"config":{"serverPassword":{"provider":"vault","reference":"op://vault/item/password"}}},"unknown":{"driver":"future"},"invalid":{"driver":"pi","config":{"binaryPath":3}}},"secretProviders":{"vault":{"kind":"one_password","label":"Vault","config":{"account":"work"}}}}),
    );
    registry
        .reconcile(
            &settings,
            &resolver,
            &BTreeSet::new(),
            true,
            &BTreeMap::new(),
        )
        .await;
    let id = ProviderInstanceId("server".into());
    assert!(matches!(
        registry.get(&id).await.unwrap().status,
        InstanceStatus::Unavailable(UnavailableReason::ReadingSecrets)
    ));
    assert!(fixture.calls().is_empty());
    registry
        .reconcile(
            &settings,
            &resolver,
            &BTreeSet::new(),
            false,
            &BTreeMap::new(),
        )
        .await;
    assert_eq!(
        prepared(&registry.get(&id).await.unwrap()).config["serverPassword"],
        json!("secret-value")
    );
    assert!(matches!(
        registry
            .get(&ProviderInstanceId("unknown".into()))
            .await
            .unwrap()
            .status,
        InstanceStatus::Unavailable(UnavailableReason::UnknownDriver)
    ));
    assert!(matches!(
        registry
            .get(&ProviderInstanceId("invalid".into()))
            .await
            .unwrap()
            .status,
        InstanceStatus::Unavailable(UnavailableReason::InvalidConfig(_))
    ));
}

#[tokio::test]
async fn policy_changes_retire_live_instances_and_unblocking_rebuilds_unchanged_configuration(){
    let registry=InstanceRegistry::new(core_driver_definitions());let fixture=Fixture::new(json!({}));let resolver=SecretResolver::new(OnePassword::with_executable(fixture.binary(),fixture.environment.clone()));let settings=settings(json!({}));let id=ProviderInstanceId("codex".into());let blocked=BTreeMap::from([("codex".into(),"Managed policy does not allow Codex.".into())]);
    registry.reconcile(&settings,&resolver,&BTreeSet::new(),false,&BTreeMap::new()).await;
    let changes=registry.reconcile(&settings,&resolver,&BTreeSet::new(),false,&blocked).await;assert_eq!(changes.retired,[id.clone()]);assert!(matches!(registry.get(&id).await.unwrap().status,InstanceStatus::Unavailable(UnavailableReason::Blocked(_))));
    let changes=registry.reconcile(&settings,&resolver,&BTreeSet::new(),false,&BTreeMap::new()).await;assert_eq!(changes.prepared,[id.clone()]);prepared(&registry.get(&id).await.unwrap());assert!(fixture.calls().is_empty());
}
#[test]
fn unavailable_reason_debug_does_not_echo_configuration_or_vault_details(){
    for reason in [UnavailableReason::InvalidConfig("never-format-secret".into()),UnavailableReason::Blocked("never-format-secret".into()),UnavailableReason::UnreadableSecrets(vec!["never-format-secret".into()])]{assert!(!format!("{reason:?}").contains("never-format-secret"));}
}
