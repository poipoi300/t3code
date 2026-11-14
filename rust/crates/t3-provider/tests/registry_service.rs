#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use t3_contracts::acp_registry::*;
use t3_provider::{
    acp_service::{AcpCatalogOperations, AcpResult},
    environment::{Environment, Platform},
    registry::*,
    registry_installation::*,
    registry_service::*,
    sensitive::{Sensitive, ServiceResult},
    transport::ProviderClock,
};
struct Clock(AtomicI64);
impl ProviderClock for Clock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
struct Fetch;
#[async_trait]
impl RegistryFetch for Fetch {
    async fn bytes(&self, _: &str) -> ServiceResult<Vec<u8>> {
        Ok(json!({"version":"1","agents":[{"id":"assistant","name":"Assistant","version":"1.0.0","description":"Fixture","distribution":{"binary":{"windows-x86_64":{"archive":"https://archive.invalid/agent.bin","cmd":"agent.exe","args":["acp"],"env":{"TOKEN":"never-format"}}}}}]}).to_string().into_bytes())
    }
}
struct Commands(PathBuf);
#[async_trait]
impl RegistryCommands for Commands {
    fn resolve(&self, command: &str, _: &Environment) -> Option<PathBuf> {
        (command == "override").then(|| self.0.clone())
    }
    async fn run(
        &self,
        _: &Path,
        _: &[String],
        _: &Environment,
        _: Duration,
        _: usize,
    ) -> AcpResult<Sensitive<String>> {
        panic!("raw archive makes no process calls")
    }
}
struct Download(AtomicUsize);
#[async_trait]
impl RegistryDownloads for Download {
    async fn download(&self, _: &str, path: &Path) -> AcpResult<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        std::fs::write(path, b"fixture").unwrap();
        Ok(())
    }
}
#[tokio::test]
async fn prepare_reservations_disabled_references_and_package_directories_guard_binary_uninstall_and_overrides_keep_distribution_environment()
 {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    std::fs::write(&path, "{}").unwrap();
    let settings = t3_config::settings::SettingsService::new(&path);
    let command = root.path().join("local.exe");
    std::fs::write(&command, b"local").unwrap();
    let download = Arc::new(Download(AtomicUsize::new(0)));
    let catalog = RegistryAgentCatalog::new(
        RegistryCatalogOptions {
            cache_dir: root.path().join("cache"),
            tools_dir: root.path().join("tools"),
            platform: Platform::Windows,
            architecture: "x64".into(),
            environment: Environment::new(),
        },
        Arc::new(RegistryCatalog::with_fetcher(Arc::new(Fetch), None)),
        settings.clone(),
        Arc::new(Commands(command)),
        download.clone(),
        Arc::new(Clock(AtomicI64::new(0))),
    );
    assert!(
        catalog
            .inspect(&json!({"agentId":"assistant"}), &Environment::new())
            .await
            .is_err()
    );
    let search = catalog
        .search(AcpRegistrySearchInput {
            query: "".into(),
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(search.agents.len(), 1);
    assert_eq!(
        catalog
            .inspect(&json!({"agentId":"assistant"}), &Environment::new())
            .await
            .unwrap()
            .status,
        InspectionStatus::Unprepared
    );
    catalog
        .prepare(AcpRegistryPrepareInput {
            agent_id: "assistant".into(),
        })
        .await
        .unwrap();
    assert_eq!(download.0.load(Ordering::SeqCst), 1);
    let uninstall = || AcpRegistryManagedBinaryUninstallInput {
        agent_id: "assistant".into(),
    };
    assert!(!catalog.uninstall(uninstall()).await.unwrap().removed);
    std::fs::write(&path,json!({"providerInstances":{"disabled":{"driver":"acpRegistry","enabled":false,"config":{"agentId":" assistant "}}}}).to_string()).unwrap();
    settings.reload().await.unwrap();
    assert!(!catalog.uninstall(uninstall()).await.unwrap().removed);
    assert_eq!(
        catalog
            .inspect(&json!({"agentId":"assistant"}), &Environment::new())
            .await
            .unwrap()
            .status,
        InspectionStatus::Ready
    );
    let package = root.path().join("tools/assistant/1.0.0/npm");
    std::fs::create_dir(&package).unwrap();
    std::fs::write(package.join("keep"), b"package").unwrap();
    std::fs::write(&path, "{}").unwrap();
    settings.reload().await.unwrap();
    assert!(catalog.uninstall(uninstall()).await.unwrap().removed);
    assert!(package.join("keep").exists());
    assert_eq!(
        catalog
            .inspect(&json!({"agentId":"assistant"}), &Environment::new())
            .await
            .unwrap()
            .status,
        InspectionStatus::Unprepared
    );
    let inspected = catalog
        .inspect(
            &json!({"agentId":"assistant","commandPath":"override"}),
            &Environment::new(),
        )
        .await
        .unwrap();
    assert_eq!(inspected.status, InspectionStatus::Ready);
    assert_eq!(inspected.version, Some(None));
    let resolved = catalog
        .resolve(
            &json!({"agentId":"assistant","commandPath":"override"}),
            root.path(),
            &Environment::from([("TOKEN".into(), "inherited".into())]),
        )
        .await
        .unwrap();
    assert_eq!(resolved.args, ["acp"]);
    assert_eq!(resolved.environment.expose()["TOKEN"], "never-format");
    assert!(!format!("{resolved:?}").contains("never-format"));
    assert_eq!(download.0.load(Ordering::SeqCst), 1);
}
