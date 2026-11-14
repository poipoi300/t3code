#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use t3_contracts::{
    base_schemas::{ProjectId, ThreadId},
    provider_instance::ProviderInstanceId,
    settings::ServerSettingsPatch,
};
use t3_provider::{
    acp_service::*,
    auth::{CredentialBinding, SessionStopper},
    codex_installation::*,
    discovery::{Driver, VersionProbe},
    domain::*,
    environment::{Environment, Platform},
    host::ProviderSessions,
    managed_provider::BackgroundPolicy,
    native_probe::{NativeProbe, ProbeLaunch},
    native_provider::{ExecutableProbe, NativeProviderContext},
    sensitive::*,
    transport::*,
};
use tokio::sync::watch;
#[derive(Default)]
struct Effects {
    probes: AtomicUsize,
    retired: AtomicUsize,
}
#[async_trait]
impl ProviderSessions for Effects {
    async fn retire(&self, _: &ProviderInstanceId) -> ServiceResult<()> {
        self.retired.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
#[async_trait]
impl SessionStopper for Effects {
    async fn stop(&self, _: &ProviderInstanceId, _: &CredentialBinding) -> ServiceResult<()> {
        Ok(())
    }
}
#[async_trait]
impl NativeProbe for Effects {
    async fn codex(&self, _: ProbeLaunch) -> ServiceResult<Sensitive<Value>> {
        panic!("constructor must not start a provider")
    }
    async fn claude(&self, _: ProbeLaunch) -> ServiceResult<Sensitive<Value>> {
        panic!("constructor must not start a provider")
    }
}
#[async_trait]
impl ExecutableProbe for Effects {
    async fn version(
        &self,
        _: Driver,
        _: &str,
        _: &Environment,
        _: &Path,
    ) -> ServiceResult<VersionProbe> {
        self.probes.fetch_add(1, Ordering::SeqCst);
        Err(ServiceError::Unavailable)
    }
}
#[async_trait]
impl HttpTransport for Effects {
    async fn send(&self, _: HttpRequest) -> ServiceResult<HttpResponse> {
        panic!("constructor must not call a provider")
    }
}
#[async_trait]
impl ReleaseTransport for Effects {
    async fn latest(&self, _: &str) -> ServiceResult<Option<ReleaseAsset>> {
        panic!("constructor must not check releases")
    }
    async fn download(
        &self,
        _: &ReleaseAsset,
        _: &Path,
        _: watch::Sender<u64>,
    ) -> ServiceResult<()> {
        panic!("constructor must not download")
    }
}
#[async_trait]
impl CodexExecutableValidator for Effects {
    async fn validate(&self, _: &Path, _: &str, _: &Environment) -> ServiceResult<()> {
        panic!("constructor must not install")
    }
}
#[async_trait]
impl AcpSessionHost for Effects {
    async fn project_workspace(&self, _: &ProjectId) -> AcpResult<PathBuf> {
        panic!("no ACP query")
    }
    async fn instance(&self, _: &ProviderInstanceId) -> AcpResult<AcpInstance> {
        panic!("no ACP query")
    }
    async fn thread(&self, _: &ThreadId) -> AcpResult<Option<ImportedThread>> {
        panic!("no ACP query")
    }
    async fn launch_import(&self, _: ImportLaunch) -> AcpResult<()> {
        panic!("no ACP query")
    }
    async fn import_history(&self, _: &ThreadId, _: &Path) -> AcpResult<()> {
        panic!("no ACP query")
    }
    async fn close_sessions(&self, _: &ProviderInstanceId) -> AcpResult<()> {
        panic!("no ACP query")
    }
    async fn refresh(&self, _: &ProviderInstanceId) -> AcpResult<()> {
        panic!("no ACP query")
    }
}
#[tokio::test]
async fn startup_publishes_real_registry_keeps_checks_deferred_and_watches_current_settings() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let settings = t3_config::settings::SettingsService::new(root.join("settings.json"));
    let patch:ServerSettingsPatch=serde_json::from_value(json!({"enableProviderUpdateChecks":false,"providerInstances":{"codex":{"driver":"codex","config":{"setupMode":"managed","homePath":root.join("private-codex")}},"claudeAgent":{"driver":"claudeAgent","config":{"homePath":root.join("private-claude")}}}})).unwrap();
    settings.update(&patch).await.unwrap();
    let (background, _) = watch::channel(BackgroundPolicy::default());
    let effects = Arc::new(Effects::default());
    let context = NativeProviderContext {
        environment: Environment::new(),
        home: root.join("account"),
        cwd: root.into(),
        platform: Platform::current(),
        managed_mcp: false,
        server_version: "0.0.99".into(),
    };
    #[cfg(unix)]
    let owner_uid = {
        use std::os::unix::fs::MetadataExt;
        Some(std::fs::metadata(root).unwrap().uid())
    };
    #[cfg(not(unix))]
    let owner_uid = None;
    let domain = ProviderDomain::open(
        DomainOptions {
            context,
            paths: DomainPaths {
                base: root.into(),
                state: root.join("state"),
                cache: root.join("cache"),
                secrets: root.join("secrets"),
                broker_parent: root.into(),
            },
            environment_id: "fixture-env".into(),
            owner_uid,
            architecture: "x64".into(),
            helper_command: "fixture-helper".into(),
            helper_args: vec![],
            manifest_cache: None,
            manifest: None,
        },
        DomainPorts {
            settings: settings.clone(),
            policy: Arc::new(
                t3_provider::policy::ManagedProviders::new(
                    &serde_json::from_value(json!({"allowedDrivers":["codex"]})).unwrap(),
                    None,
                    Platform::current(),
                )
                .unwrap(),
            ),
            http: effects.clone(),
            clock: Arc::new(SystemClock),
            sessions: effects.clone(),
            stopper: effects.clone(),
            acp_host: effects.clone(),
            background: background.subscribe(),
            codex: CodexInstallEffects {
                transport: effects.clone(),
                validator: effects.clone(),
                local: Arc::new(NoLocalCodex),
            },
            protocols: effects.clone(),
            executables: effects.clone(),
            factories: BTreeMap::new(),
            installations: BTreeMap::new(),
            usage_sources: None,
        },
    )
    .await
    .unwrap();
    let providers = domain.config_providers().await;
    assert!(!providers.0.is_empty());
    assert!(
        providers
            .0
            .iter()
            .filter(|provider| provider.driver.0 != "codex")
            .all(|provider| !provider.enabled && provider.availability.is_some())
    );

    assert!(
        providers
            .0
            .iter()
            .any(|provider| provider.driver.0 == "codex" && provider.availability.is_none())
    );
    assert!(
        domain
            .managed_codex
            .runtime(&"codex".into())
            .await
            .is_some()
    );
    assert_eq!(effects.probes.load(Ordering::SeqCst), 0);
    assert!(!format!("{domain:?}").contains("fixture-env"));
    let mut changed = domain.providers.subscribe();
    let patch: ServerSettingsPatch = serde_json::from_value(
        json!({"providerInstances":{"codex":{"driver":"codex","displayName":"Renamed during startup","config":{"setupMode":"managed","homePath":root.join("private-codex")}}}}),
    )
    .unwrap();
    settings.update(&patch).await.unwrap();
    loop {
        changed.changed().await.unwrap();
        if domain.config_providers().await.0.iter().any(|provider| {
            provider
                .display_name
                .as_ref()
                .and_then(Option::as_ref)
                .is_some_and(|name| name.0 == "Renamed during startup")
        }) {
            break;
        }
    }
    assert_eq!(effects.probes.load(Ordering::SeqCst), 0);
    domain.shutdown().await.unwrap();
    assert!(domain.config_providers().await.0.is_empty());
    assert!(
        domain
            .managed_codex
            .runtime(&"codex".into())
            .await
            .is_none()
    );
    assert!(effects.retired.load(Ordering::SeqCst) > 0);
}
