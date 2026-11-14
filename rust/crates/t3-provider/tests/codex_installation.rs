#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use t3_contracts::provider_setup::ProviderInstallPhase;
use t3_provider::{
    catalog::Manifest,
    codex_installation::*,
    environment::{Environment, Platform, process_options},
    sensitive::*,
    transport::ProviderClock,
};
use tokio::sync::watch;
struct Clock;
impl ProviderClock for Clock {
    fn now_ms(&self) -> i64 {
        1_800_000_000_000
    }
}
struct Transport {
    archive: PathBuf,
    corrupt: AtomicBool,
}
#[async_trait]
impl ReleaseTransport for Transport {
    async fn latest(&self, _: &str) -> ServiceResult<Option<ReleaseAsset>> {
        Err(ServiceError::Network)
    }
    async fn download(
        &self,
        _: &ReleaseAsset,
        destination: &Path,
        progress: watch::Sender<u64>,
    ) -> ServiceResult<()> {
        let bytes = if self.corrupt.load(Ordering::SeqCst) {
            b"corrupt".to_vec()
        } else {
            std::fs::read(&self.archive).unwrap()
        };
        std::fs::write(destination, &bytes).unwrap();
        progress.send_replace(bytes.len() as u64);
        Ok(())
    }
}
struct Validator;
#[async_trait]
impl CodexExecutableValidator for Validator {
    async fn validate(&self, path: &Path, version: &str, _: &Environment) -> ServiceResult<()> {
        assert!(path.is_file());
        assert_eq!(version, "0.166.0");
        Ok(())
    }
}
async fn setup() -> (tempfile::TempDir, Arc<CodexInstallation>, Arc<Transport>) {
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("package");
    std::fs::create_dir_all(package.join("bin")).unwrap();
    std::fs::create_dir_all(package.join("codex-path")).unwrap();
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    for file in [
        format!("bin/codex{suffix}"),
        format!("bin/codex-code-mode-host{suffix}"),
        format!("codex-path/rg{suffix}"),
    ] {
        let path = package.join(file);
        std::fs::write(&path, b"fixture executable").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    std::fs::write(package.join("codex-package.json"),json!({"layoutVersion":1,"version":"0.166.0","target":"fixture","entrypoint":format!("bin/codex{suffix}")}).to_string()).unwrap();
    let archive = directory.path().join("recorded.tar.gz");
    let environment: Environment = std::env::vars().collect();
    let tar = if cfg!(windows) {
        PathBuf::from(std::env::var("SystemRoot").unwrap()).join("System32/tar.exe")
    } else {
        PathBuf::from("tar")
    };
    let options = process_options(
        tar.as_os_str(),
        &[
            "-czf".into(),
            archive.to_string_lossy().into_owned(),
            "-C".into(),
            package.to_string_lossy().into_owned(),
            "bin".into(),
            "codex-path".into(),
            "codex-package.json".into(),
        ],
        &environment,
    );
    t3_process::run(options).await.unwrap();
    let bytes = std::fs::read(&archive).unwrap();
    let pin = ReleaseAsset {
        version: "0.166.0".into(),
        target: "fixture".into(),
        url: "https://example.invalid/recorded.tar.gz".into(),
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        archive_bytes: Some(bytes.len() as u64),
    };
    let transport = Arc::new(Transport {
        archive,
        corrupt: AtomicBool::new(false),
    });
    let installation = CodexInstallation::new(
        InstallationOptions {
            base_directory: directory.path().join("state"),
            platform: Platform::current(),
            environment,
            pin: Some(pin),
            manifest: Arc::new(Manifest::bundled().unwrap()).into(),
            server_version: "0.1.0".into(),
        },
        transport.clone(),
        Arc::new(Validator),
        Arc::new(NoLocalCodex),
        Arc::new(Clock),
    );
    (directory, installation, transport)
}
#[tokio::test]
async fn verified_recorded_package_publishes_compatible_disk_records_and_active_leases_block_removal()
 {
    let (directory, installation, _) = setup().await;
    installation.start().await.unwrap();
    installation.wait().await;
    assert_eq!(installation.state().phase, ProviderInstallPhase::Succeeded);
    let active = directory.path().join("state/tools/codex/active.json");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(active).unwrap()).unwrap(),
        json!({"version":"0.166.0"})
    );
    let lease = installation.acquire().await.unwrap();
    assert_eq!(
        installation.remove(&[]).await.unwrap_err(),
        ServiceError::Busy
    );
    let path = lease.executable.path.clone();
    drop(lease);
    assert_eq!(
        installation
            .remove(std::slice::from_ref(&path))
            .await
            .unwrap_err(),
        ServiceError::Busy
    );
    let missing = PathBuf::from(path.to_string_lossy().trim_start_matches(r"\\?\"))
        .parent()
        .unwrap()
        .join("selected-but-not-materialized");
    assert_eq!(
        installation.remove(&[missing]).await.unwrap_err(),
        ServiceError::Busy
    );

    installation.remove(&[]).await.unwrap();
    assert_eq!(installation.state().phase, ProviderInstallPhase::Idle);
}
#[tokio::test]
async fn a_bad_download_never_publishes_an_active_runtime_and_reinstall_can_recover() {
    let (directory, installation, transport) = setup().await;
    transport.corrupt.store(true, Ordering::SeqCst);
    installation.start().await.unwrap();
    installation.wait().await;
    assert_eq!(installation.state().phase, ProviderInstallPhase::Failed);
    assert!(
        !directory
            .path()
            .join("state/tools/codex/active.json")
            .exists()
    );
    transport.corrupt.store(false, Ordering::SeqCst);
    installation.start().await.unwrap();
    installation.wait().await;
    assert_eq!(installation.state().phase, ProviderInstallPhase::Succeeded);
    installation.start().await.unwrap();
    installation.wait().await;
    assert_eq!(installation.state().phase, ProviderInstallPhase::Succeeded);
}
#[test]
fn unsafe_archive_entries_and_links_are_rejected_before_extraction_and_all_pins_are_present() {
    for path in [
        "../escape",
        "/absolute",
        "a/../b",
        "a\\b",
        "C:/absolute",
        "./a",
        "a//b",
        "a:b",
    ] {
        assert!(validate_archive(path, "- file").is_err());
    }
    assert!(validate_archive("safe", "l link").is_err());
    assert!(validate_archive("a\nb", "- file").is_err());
    validate_archive("bin/\nbin/codex", "d directory\n- file").unwrap();
    for platform in ["win32", "darwin", "linux"] {
        for architecture in ["x64", "arm64"] {
            assert_eq!(
                pinned_release(platform, architecture).unwrap().version,
                "0.166.0"
            );
        }
    }
}

#[tokio::test]
async fn removal_rejects_a_managed_directory_reached_through_a_linked_tools_parent() {
    let (directory, installation, _) = setup().await;
    let base = directory.path().join("state");
    let outside = directory.path().join("unrelated-tools");
    std::fs::create_dir_all(outside.join("codex")).unwrap();
    std::fs::create_dir(&base).unwrap();
    let sentinel = outside.join("codex/keep");
    std::fs::write(&sentinel, "unrelated data").unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&outside, base.join("tools")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, base.join("tools")).unwrap();
    assert_eq!(
        installation.remove(&[]).await.unwrap_err(),
        ServiceError::Storage
    );
    assert!(sentinel.is_file());
}

struct RouterSettings(t3_contracts::settings::ServerSettings);
#[async_trait]
impl t3_provider::text_generators::SettingsSource for RouterSettings {
    async fn settings(&self) -> ServiceResult<t3_contracts::settings::ServerSettings> {
        Ok(self.0.clone())
    }
}
struct NoMaintenance;
#[async_trait]
impl t3_provider::service::CatalogRefresh for NoMaintenance {
    async fn refresh(&self) -> ServiceResult<()> {
        Ok(())
    }
}
#[async_trait]
impl t3_provider::service::MaintenanceExecutor for NoMaintenance {
    async fn execute(&self, _: t3_provider::maintenance::MaintenanceAction) -> ServiceResult<()> {
        panic!("installation RPC never runs provider maintenance")
    }
}

#[tokio::test]
async fn setup_router_enforces_managed_instances_and_protects_configured_executables() {
    use std::collections::{BTreeMap, BTreeSet};
    use t3_provider::{
        auth::AuthService, installations::*, instances::*, secrets::*, service::ProviderService,
    };
    let (directory, installation, _) = setup().await;
    let registry = Arc::new(InstanceRegistry::new(core_driver_definitions()));
    let resolver = SecretResolver::new(OnePassword::new(Environment::new()));
    let mut settings: t3_contracts::settings::ServerSettings = serde_json::from_value(json!({
        "enableProviderUpdateChecks":false,
        "providerInstances":{
            "work":{"driver":"codex","config":{"setupMode":"managed","binaryPath":"codex"}},
            "custom":{"driver":"codex","config":{"setupMode":"existing","binaryPath":"codex"}},
            "override":{"driver":"codex","config":{"setupMode":"managed","binaryPath":"custom-codex"}},
            "claude":{"driver":"claudeAgent"}
        }
    })).unwrap();
    registry
        .reconcile(
            &settings,
            &resolver,
            &BTreeSet::new(),
            false,
            &BTreeMap::new(),
        )
        .await;
    let providers = ProviderService::new(
        Arc::new(AuthService::default()),
        Arc::new(NoMaintenance),
        Arc::new(NoMaintenance),
        Arc::new(Clock),
        Arc::new(Manifest::bundled().unwrap()),
        "0.1.0".into(),
    );
    let router = ProviderInstallations::new(
        registry.clone(),
        providers,
        Arc::new(RouterSettings(settings.clone())),
        InstallationContext {
            environment: Environment::new(),
            home: directory.path().into(),
            cwd: directory.path().into(),
            platform: Platform::current(),
        },
    );
    router.register("codex", installation.clone()).await;
    let input = |id: &str| serde_json::from_value(json!({"instanceId":id})).unwrap();
    for id in ["custom", "override", "claude"] {
        assert_eq!(
            router.start(input(id)).await.unwrap_err(),
            ServiceError::Unsupported
        );
    }
    let mut states = router.subscribe(input("work")).await.unwrap();
    let started = router.start(input("work")).await.unwrap();
    assert!(started.operation_id.is_some());
    installation.wait().await;
    assert_eq!(
        states.borrow_and_update().phase,
        ProviderInstallPhase::Succeeded
    );
    let executable = installation.resolve().await.unwrap();
    settings.provider_instances.insert("protected".into(), serde_json::from_value(json!({"driver":"codex","enabled":false,"config":{"setupMode":"existing","binaryPath":executable.path}})).unwrap());
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
        router.remove(input("work")).await.unwrap_err(),
        ServiceError::Busy
    );
    assert!(executable.path.is_file());
    settings.provider_instances.remove(&"protected".into());
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
        router.remove(input("work")).await.unwrap().phase,
        ProviderInstallPhase::Idle
    );
}
