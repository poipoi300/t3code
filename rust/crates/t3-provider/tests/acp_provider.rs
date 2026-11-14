#![allow(clippy::unwrap_used)]
mod common;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use t3_contracts::server::{ServerProvider, ServerProviderAuthStatus};
use t3_provider::{
    acp_auth_state::AcpAuthenticationState,
    acp_coordinator::{AcpRuntimeCoordinator, LiveConfiguration},
    acp_probe::AcpNativeServices,
    acp_provider::{AcpProvider, models},
    catalog,
    environment::Platform,
    instances::InstanceSpec,
    registry::{RegistryCatalog, RegistryFetch},
    registry_installation::{ProcessRegistryCommands, ReqwestRegistryDownloads},
    registry_service::{RegistryAgentCatalog, RegistryCatalogOptions},
    sensitive::{Sensitive, ServiceResult},
    service::ProviderInstanceService,
    transport::SystemClock,
};
struct NoFetch;
#[async_trait]
impl RegistryFetch for NoFetch {
    async fn bytes(&self, _: &str) -> ServiceResult<Vec<u8>> {
        panic!("local ACP snapshots must not fetch a registry")
    }
}
async fn observed(
    updates: &mut tokio::sync::watch::Receiver<Option<ServerProvider>>,
    matches: impl Fn(&ServerProvider) -> bool,
) -> ServerProvider {
    loop {
        let current = updates.borrow_and_update().clone();
        if let Some(current) = current
            && matches(&current)
        {
            return current;
        }
        updates.changed().await.unwrap();
    }
}
#[tokio::test]
async fn background_acp_metadata_is_cached_without_proving_auth_and_missing_commands_override_cached_readiness()
 {
    let fixture = common::Fixture::new(json!({"protocol":"acp"}));
    let executable = fixture.directory.path().join(if cfg!(windows) {
        "local-agent.exe"
    } else {
        "local-agent"
    });
    std::fs::copy(fixture.binary(), &executable).unwrap();
    let clock = Arc::new(SystemClock);
    let settings = json!({"source":"local","commandPath":executable});
    let confirmed = AcpAuthenticationState::load(
        fixture.directory.path(),
        &"acp".into(),
        &settings,
        &vec![],
        &fixture.environment,
    )
    .await;
    let catalog = Arc::new(RegistryAgentCatalog::new(
        RegistryCatalogOptions {
            cache_dir: fixture.directory.path().join("cache"),
            tools_dir: fixture.directory.path().join("tools"),
            platform: Platform::current(),
            architecture: std::env::consts::ARCH.into(),
            environment: fixture.environment.clone(),
        },
        Arc::new(RegistryCatalog::with_fetcher(Arc::new(NoFetch), None)),
        t3_config::settings::SettingsService::new(fixture.directory.path().join("settings.json")),
        Arc::new(ProcessRegistryCommands {
            cwd: fixture.directory.path().into(),
        }),
        Arc::new(ReqwestRegistryDownloads::new().unwrap()),
        clock.clone(),
    ));
    let native = Arc::new(AcpNativeServices {
        instance_id: "acp".into(),
        settings: settings.clone(),
        environment: Sensitive::new(fixture.environment.clone()),
        catalog,
        coordinator: Arc::new(AcpRuntimeCoordinator::new(clock.clone())),
    });
    let spec = Arc::new(InstanceSpec {
        instance_id: "acp".into(),
        driver: "acpRegistry".into(),
        display_name: None,
        accent_color: None,
        enabled: true,
        config: settings,
        variables: vec![],
    });
    let provider = AcpProvider::new(
        spec,
        native,
        confirmed.clone(),
        clock,
        fixture.directory.path().into(),
    )
    .await;
    let mut updates = provider.subscribe_snapshots().unwrap();
    let initial = provider.snapshot(None, false).await.unwrap();
    assert!(initial.installed);
    assert!(initial.native_sessions.is_none());
    let enriched = observed(&mut updates, |provider| provider.native_sessions.is_some()).await;
    assert_eq!(enriched.models[0].slug.0, "model");
    assert_eq!(enriched.slash_commands.len(), 1);
    assert_eq!(enriched.auth.status, ServerProviderAuthStatus::Unknown);
    assert!(!confirmed.get());
    let calls = fixture.calls();
    assert!(
        calls
            .iter()
            .flatten()
            .any(|method| method == "session/delete")
    );
    let cached = provider.snapshot(None, false).await.unwrap();
    assert_eq!(cached.models, enriched.models);
    assert_eq!(calls, fixture.calls());
    confirmed.set(true).await.unwrap();
    observed(&mut updates, |provider| {
        provider.auth.status == ServerProviderAuthStatus::Authenticated
    })
    .await;
    // Confirmation invalidates metadata, so wait for its replacement probe to release the executable.
    observed(&mut updates, |provider| provider.native_sessions.is_some()).await;
    std::fs::remove_file(&executable).unwrap();
    let missing = provider.snapshot(None, false).await.unwrap();
    assert!(!missing.installed);
    assert_eq!(missing.auth.status, ServerProviderAuthStatus::Unknown);
    assert!(
        !fixture
            .calls()
            .iter()
            .flatten()
            .any(|method| method == "session/prompt")
    );
}
#[test]
fn model_switches_preserve_learned_options_without_leaking_them_to_other_models() {
    let shared = catalog::boolean_option("fast", "Fast", Some(false));
    let model_option = catalog::select_option(
        "effort",
        "Effort",
        &[("low", "Low"), ("high", "High")],
        Some("high"),
    );
    let mut live=LiveConfiguration{models:serde_json::from_value(json!([{"id":"one","name":"One","description":null},{"id":"two","name":"Two","description":null}])).unwrap(),current_model_id:Some("one".into()),config_options:vec![shared.clone(),model_option.clone()],model_option_ids:Some(vec!["effort".into()])};
    let first = models(Some(&live), &[], &[]);
    assert_eq!(
        catalog::descriptors(first[0].capabilities.as_ref().unwrap()).len(),
        2
    );
    assert_eq!(
        catalog::descriptors(first[1].capabilities.as_ref().unwrap()).len(),
        1
    );
    live.current_model_id = Some("two".into());
    live.config_options = vec![shared];
    let next = models(Some(&live), &[], &first);
    assert_eq!(
        catalog::descriptors(next[0].capabilities.as_ref().unwrap()).len(),
        2
    );
    assert_eq!(
        catalog::descriptors(next[1].capabilities.as_ref().unwrap()).len(),
        1
    );
    assert_eq!(next[1].is_default, Some(Some(true)));
}
