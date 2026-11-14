#![allow(clippy::unwrap_used)]
mod common;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use t3_provider::{
    acp_coordinator::AcpRuntimeCoordinator,
    acp_probe::*,
    acp_service::AcpSessionManager,
    environment::Platform,
    registry::{RegistryCatalog, RegistryFetch},
    registry_installation::{ProcessRegistryCommands, ReqwestRegistryDownloads},
    registry_service::{RegistryAgentCatalog, RegistryCatalogOptions},
    sensitive::{Sensitive, ServiceResult},
    transport::SystemClock,
};
struct NoFetch;
#[async_trait]
impl RegistryFetch for NoFetch {
    async fn bytes(&self, _: &str) -> ServiceResult<Vec<u8>> {
        panic!("local ACP management must not fetch a registry")
    }
}
fn service(fixture: &common::Fixture) -> AcpNativeServices {
    let clock = Arc::new(SystemClock);
    let catalog = RegistryAgentCatalog::new(
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
    );
    AcpNativeServices {
        instance_id: "acp".into(),
        settings: json!({"source":"local","commandPath":fixture.binary()}),
        environment: Sensitive::new(fixture.environment.clone()),
        catalog: Arc::new(catalog),
        coordinator: Arc::new(AcpRuntimeCoordinator::new(clock)),
    }
}
#[tokio::test]
async fn native_acp_probes_delete_empty_sessions_and_management_preserves_advertised_capabilities_without_turns()
 {
    let fixture = common::Fixture::new(json!({"protocol":"acp"}));
    let service = service(&fixture);
    let probe = service.probe(fixture.directory.path()).await.unwrap();
    assert_eq!(probe.probe.expose().models[0].id.0, "model");
    assert!(probe.probe.expose().session_management.can_delete);
    assert_eq!(probe.commands.slash_commands.len(), 1);
    assert!(
        fixture
            .calls()
            .iter()
            .flatten()
            .any(|method| method == "session/delete")
    );
    let sessions = service
        .list_sessions(fixture.directory.path(), None)
        .await
        .unwrap();
    assert_eq!(sessions.sessions[0].session_id.0, "one");
    assert!(sessions.can_delete);
    service
        .delete_session(fixture.directory.path(), "one")
        .await
        .unwrap();
    assert_eq!(
        service
            .list_providers(fixture.directory.path())
            .await
            .unwrap_err()
            .reason,
        t3_contracts::acp_registry::AcpRegistryOperationErrorReason::ProvidersUnsupported
    );
    service.logout(fixture.directory.path()).await.unwrap();
    fixture.write(json!({"protocol":"acp","initialize":{"protocolVersion":2,"info":{"name":"fixture","version":"1"},"capabilities":{"session":{"delete":{}},"providers":{}},"authMethods":[{"methodId":"login","name":"Sign in","type":"agent"}]}}));
    let listed = service
        .list_providers(fixture.directory.path())
        .await
        .unwrap();
    assert_eq!(listed.providers.len(), 1);
    assert_eq!(listed.providers[0].provider_id.0, "openai");
    let input=Sensitive::new(serde_json::from_value(json!({"instanceId":"acp","projectId":"project","providerId":"openai","apiType":"openai","baseUrl":"https://api.invalid","headers":{"Authorization":"never-logged"}})).unwrap());
    service
        .set_provider(fixture.directory.path(), input)
        .await
        .unwrap();
    service
        .disable_provider(fixture.directory.path(), "openai")
        .await
        .unwrap();
    assert!(
        !fixture
            .calls()
            .iter()
            .flatten()
            .any(|method| method == "session/prompt")
    );
    fixture.write(json!({"protocol":"acp","authRequired":true,"initialize":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[{"id":"terminal","name":"Login","type":"terminal","env":{"API_KEY":"never-error-secret"}}]}}));
    let failure = service.probe(fixture.directory.path()).await.unwrap_err();
    assert_eq!(
        failure.error.reason,
        t3_contracts::acp_registry::AcpRegistryOperationErrorReason::AuthenticationFailed
    );
    assert_eq!(failure.auth_methods.expose().len(), 1);
    assert!(!format!("{failure:?} {failure}").contains("never-error-secret"));
}
