#![allow(clippy::unwrap_used)]
mod common;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use t3_provider::{
    catalog::Manifest,
    environment::Platform,
    instances::InstanceSpec,
    muse::*,
    native_provider::{ExecutableProbe, NativeProviderContext, ProcessExecutableProbe},
    sensitive::*,
    service::ProviderInstanceService,
    transport::SystemClock,
};
struct Catalog {
    response: Value,
    calls: AtomicUsize,
}
#[async_trait]
impl MuseCatalog for Catalog {
    async fn list(&self, request: CatalogRequest) -> ServiceResult<Sensitive<Value>> {
        assert_eq!(
            request.args,
            [
                "serve",
                "--disable-shell",
                "--disable-write",
                "--no-session-log"
            ]
        );
        assert_eq!(request.initialize.expose(), &initialize_params(true));
        assert!(
            !request
                .environment
                .expose()
                .keys()
                .any(|key| key.eq_ignore_ascii_case("META_API_KEY"))
        );
        assert_eq!(request.environment.expose()["MUSE_NO_AUTO_UPDATE"], "1");
        assert_eq!(request.environment.expose()["MISE_OFFLINE"], "1");
        assert!(!format!("{request:?}").contains("secret-never-format"));
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Sensitive::new(self.response.clone()))
    }
}
fn provider(
    fixture: &common::Fixture,
    catalog: Arc<dyn MuseCatalog>,
    enabled: bool,
) -> MuseProvider {
    provider_with_probe(fixture, catalog, enabled, Arc::new(ProcessExecutableProbe))
}
fn provider_with_probe(
    fixture: &common::Fixture,
    catalog: Arc<dyn MuseCatalog>,
    enabled: bool,
    probe: Arc<dyn ExecutableProbe>,
) -> MuseProvider {
    let mut environment = fixture.environment.clone();
    environment.insert("meta_api_key".into(), "secret-never-format".into());
    MuseProvider::new(
        Arc::new(InstanceSpec {
            instance_id: "muse-work".into(),
            driver: "muse".into(),
            enabled,
            display_name: Some("Work".into()),
            accent_color: None,
            config: json!({"binaryPath":fixture.binary(),"customModels":[{"slug":"custom","name":"Custom"}]}),
            variables: vec![],
        }),
        NativeProviderContext {
            environment,
            home: fixture.directory.path().into(),
            cwd: fixture.directory.path().into(),
            platform: Platform::current(),
            managed_mcp: false,
            server_version: "1".into(),
        },
        probe,
        catalog,
        Arc::new(SystemClock),
        Arc::new(Manifest::bundled().unwrap()).into(),
    )
}
#[tokio::test]
async fn catalog_readiness_does_not_claim_login_and_invalid_or_empty_catalogs_remain_distinct() {
    for (response, state, models) in [
        (
            json!({"providerId":"meta","models":[{"modelId":"muse-2-pro","displayLabel":"","providerId":"meta","isDefault":true,"variants":[]}]}),
            "ready",
            2,
        ),
        (json!({"providerId":"meta","models":[]}), "warning", 1),
        (json!({"providerId":"foreign","models":[]}), "error", 1),
        (
            json!({"providerId":"meta","models":[{"modelId":"m","displayLabel":"M","providerId":"meta","isDefault":true,"variants":null}]}),
            "error",
            1,
        ),
    ] {
        let fixture = common::Fixture::new(
            json!({"version":"Muse 1.2.3-R42.1","expectedEnvironment":{"MUSE_NO_AUTO_UPDATE":"1","MISE_OFFLINE":"1"}}),
        );
        let catalog = Arc::new(Catalog {
            response,
            calls: AtomicUsize::new(0),
        });
        let source = provider(&fixture, catalog.clone(), true);
        assert_eq!(source.initial_snapshot().unwrap().models.len(), 1);
        assert!(fixture.calls().is_empty());
        let snapshot = serde_json::to_value(source.snapshot(None, false).await.unwrap()).unwrap();
        assert_eq!(snapshot["status"], state);
        assert_eq!(snapshot["auth"]["status"], "unknown");
        assert_eq!(snapshot["models"].as_array().unwrap().len(), models);
        assert_eq!(
            snapshot["supportedRuntimeModes"],
            json!(["approval-required", "full-access"])
        );
        assert_eq!(catalog.calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.calls(), [vec!["--version"]]);
    }
}
#[tokio::test]
async fn disabled_or_unfinished_launcher_does_not_start_an_sdk_host() {
    let fixture = common::Fixture::new(
        json!({"version":"","versionStderr":"installed binary is missing: secret-never-format","versionExit":1}),
    );
    let catalog = Arc::new(Catalog {
        response: json!({}),
        calls: AtomicUsize::new(0),
    });
    let source = provider(&fixture, catalog.clone(), false);
    assert!(
        source
            .snapshot(None, false)
            .await
            .unwrap()
            .slash_commands
            .is_empty()
    );
    assert!(fixture.calls().is_empty());
    let source = provider(&fixture, catalog.clone(), true);
    let snapshot = serde_json::to_value(source.snapshot(None, false).await.unwrap()).unwrap();
    assert_eq!(snapshot["status"], "error");
    assert!(
        snapshot["message"]
            .as_str()
            .unwrap()
            .contains("not finished installing")
    );
    assert!(!snapshot.to_string().contains("secret-never-format"));
    assert_eq!(catalog.calls.load(Ordering::SeqCst), 0);
}
struct Version;
#[async_trait]
impl ExecutableProbe for Version {
    async fn version(
        &self,
        _: t3_provider::discovery::Driver,
        _: &str,
        _: &t3_provider::environment::Environment,
        _: &std::path::Path,
    ) -> ServiceResult<t3_provider::discovery::VersionProbe> {
        Ok(t3_provider::discovery::VersionProbe {
            installed: true,
            version: Some("1.2.3-R1".into()),
            status: t3_contracts::server::ServerProviderState::Warning,
            message: None,
            resolved_executable: None,
        })
    }
}
struct Pending(Arc<AtomicUsize>);
struct DropCount(Arc<AtomicUsize>);
impl Drop for DropCount {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
#[async_trait]
impl MuseCatalog for Pending {
    async fn list(&self, _: CatalogRequest) -> ServiceResult<Sensitive<Value>> {
        let _guard = DropCount(self.0.clone());
        std::future::pending().await
    }
}
#[tokio::test(start_paused = true)]
async fn catalog_deadline_cancels_owned_startup_and_permission_defaults_are_explicit() {
    let fixture = common::Fixture::new(json!({"version":"1.2.3-R1"}));
    let dropped = Arc::new(AtomicUsize::new(0));
    let snapshot = provider_with_probe(
        &fixture,
        Arc::new(Pending(dropped.clone())),
        true,
        Arc::new(Version),
    )
    .snapshot(None, false)
    .await
    .unwrap();
    assert_eq!(serde_json::to_value(snapshot).unwrap()["status"], "error");
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(approval_mode("auto-accept-edits"), "promptUnmatched");
    assert_eq!(permission_profile("full-access"), ":unrestricted");
    assert_eq!(
        serve_args(false, false, "full-access"),
        ["serve", "--trust-workspace", "--disable-sandbox"]
    );
}

#[tokio::test]
async fn usage_ingestion_rejects_older_reports_clamps_percentages_and_retains_latest_exhausted_reset()
 {
    use t3_provider::{
        managed_provider::{BackgroundPolicy, ManagedOptions},
        muse_usage::*,
    };
    let fixture = common::Fixture::new(json!({}));
    let catalog = Arc::new(Catalog {
        response: json!({}),
        calls: AtomicUsize::new(0),
    });
    let (_policy, background) = tokio::sync::watch::channel(BackgroundPolicy::default());
    let provider = provider(&fixture, catalog, true)
        .into_managed(ManagedOptions::new(background))
        .unwrap();
    let ingestion = MuseUsageIngestion::new(&provider);
    let usage = json!({"observedAtMs":2000,"tier":" Pro ","window":{"usedPercent":110,"windowDurationMins":300,"resetsAtMs":7000},"weekly":{"usedPercent":100,"resetsAtMs":9000}});
    assert!(ingestion.apply(&usage).await.unwrap());
    let value = serde_json::to_value(provider.current().await).unwrap();
    assert_eq!(value["auth"]["label"], "Pro");
    assert_eq!(value["usageLimits"]["windows"][0]["usedPercent"], 100.0);
    assert_eq!(value["usageLimits"]["windows"].as_array().unwrap().len(), 2);
    let mut older = usage.clone();
    older["observedAtMs"] = json!(1000);
    older["tier"] = json!("Stale");
    assert!(!ingestion.apply(&older).await.unwrap());
    assert_eq!(
        limit_reset_at(
            Some(&usage),
            1000.0,
            Some(&json!({"quota":{"resetsAtMs":8000}}))
        )
        .as_deref(),
        Some("1970-01-01T00:00:09.000Z")
    );
    assert!(is_usage_limit_failure(Some(
        "HTTP 429: usage limit reached"
    )));
    assert!(!is_usage_limit_failure(Some("4290 other error")));
    older["window"]["windowDurationMins"] = json!(0);
    assert!(report(&older).is_err());
    assert!(fixture.calls().is_empty());
}
