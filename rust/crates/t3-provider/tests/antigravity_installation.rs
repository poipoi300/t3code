#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use sha2::{Digest, Sha512};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use t3_contracts::provider_setup::ProviderInstallPhase;
use t3_provider::{
    antigravity_installation::*,
    antigravity_release::{Format, Member, Release},
    catalog::Manifest,
    environment::{Environment, Platform},
    sensitive::*,
    transport::SystemClock,
};
use tokio::sync::watch;
struct Releases {
    asset: Release,
    bytes: Vec<u8>,
    downloads: AtomicUsize,
    block: Option<Arc<tokio::sync::Notify>>,
}
#[async_trait]
impl AntigravityReleases for Releases {
    async fn latest(&self, _: &str) -> ServiceResult<Option<Release>> {
        Ok(Some(self.asset.clone()))
    }
    async fn download(
        &self,
        _: &Release,
        path: &Path,
        progress: watch::Sender<u64>,
    ) -> ServiceResult<()> {
        self.downloads.fetch_add(1, Ordering::SeqCst);
        if let Some(block) = &self.block {
            block.notified().await;
        }
        tokio::fs::write(path, &self.bytes)
            .await
            .map_err(|_| ServiceError::Storage)?;
        progress.send_replace(self.bytes.len() as u64);
        Ok(())
    }
}
struct Validator;
#[async_trait]
impl AntigravityValidator for Validator {
    async fn validate(&self, executable: &AntigravityExecutable, _: &str) -> ServiceResult<()> {
        if std::fs::read(&executable.executable).unwrap() == b"native runtime" {
            Ok(())
        } else {
            Err(ServiceError::Maintenance)
        }
    }
}
fn release(bytes: &[u8]) -> Release {
    Release {
        version: "1.5.3".into(),
        url: "https://example.invalid/runtime".into(),
        sha256: None,
        sha512: Some(format!("{:x}", Sha512::digest(bytes))),
        archive_bytes: Some(bytes.len() as u64),
        executable: Member {
            name: if cfg!(windows) {
                "agy.exe"
            } else {
                "antigravity"
            }
            .into(),
            bytes: None,
        },
        harness: None,
        format: Format::Executable,
    }
}
fn installer(base: &Path, releases: Arc<Releases>) -> Arc<AntigravityInstallation> {
    AntigravityInstallation::new(
        InstallationOptions {
            base: base.into(),
            platform: Platform::current(),
            architecture: "x64".into(),
            musl: false,
            environment: Environment::new(),
            manifest: Arc::new(Manifest::bundled().unwrap()).into(),
            server_version: "1".into(),
            pin: None,
            update_checks: true,
        },
        releases,
        Arc::new(Validator),
        Arc::new(SystemClock),
    )
}
async fn settled(installer: &AntigravityInstallation) -> ProviderInstallPhase {
    let mut changes = installer.subscribe();
    changes
        .wait_for(|state| {
            matches!(
                state.phase,
                ProviderInstallPhase::Succeeded
                    | ProviderInstallPhase::Failed
                    | ProviderInstallPhase::Cancelled
            )
        })
        .await
        .unwrap()
        .phase
        .clone()
}
#[tokio::test]
async fn publication_verification_reuse_and_runtime_leases_preserve_the_active_installation() {
    let temp = tempfile::tempdir().unwrap();
    let bytes = b"native runtime".to_vec();
    let releases = Arc::new(Releases {
        asset: release(&bytes),
        bytes,
        downloads: AtomicUsize::new(0),
        block: None,
    });
    let installation = installer(temp.path(), releases.clone());
    let initial = installation.start().await.unwrap();
    assert_eq!(initial.phase, ProviderInstallPhase::Downloading);
    assert_eq!(
        settled(&installation).await,
        ProviderInstallPhase::Succeeded
    );
    assert_eq!(
        installer(temp.path(), releases.clone())
            .state()
            .installed_version
            .as_ref()
            .unwrap()
            .0,
        "1.5.3"
    );
    let lease = installation
        .acquire(None, &Environment::new())
        .await
        .unwrap();
    assert_eq!(lease.executable.version.as_deref(), Some("1.5.3"));
    assert_eq!(lease.executable.source, ExecutableSource::Managed);
    assert!(installation.remove(&[]).await.is_err());
    drop(lease);
    installation.start().await.unwrap();
    assert_eq!(
        settled(&installation).await,
        ProviderInstallPhase::Succeeded
    );
    assert_eq!(releases.downloads.load(Ordering::SeqCst), 1);
    let selected = installation
        .resolve(None, &Environment::new())
        .await
        .unwrap();
    assert!(
        installation
            .remove(std::slice::from_ref(&selected.executable))
            .await
            .is_err()
    );
    let missing = std::path::PathBuf::from(
        selected
            .executable
            .to_string_lossy()
            .trim_start_matches(r"\\?\"),
    )
    .parent()
    .unwrap()
    .join("selected-but-not-materialized");
    assert!(installation.remove(&[missing]).await.is_err());
    installation.remove(&[]).await.unwrap();
    assert!(!installation.directory().exists());
}
#[tokio::test]
async fn cancelled_or_hash_mismatched_downloads_never_publish_an_active_pointer() {
    for cancel in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let releases = Arc::new(Releases {
            asset: release(b"native runtime"),
            bytes: b"corrupt runtime".to_vec(),
            downloads: AtomicUsize::new(0),
            block: cancel.then(|| Arc::new(tokio::sync::Notify::new())),
        });
        let installation = installer(temp.path(), releases);
        let initial = installation.start().await.unwrap();
        if cancel {
            installation
                .cancel(&initial.operation_id.unwrap().0)
                .await
                .unwrap();
            assert_eq!(
                settled(&installation).await,
                ProviderInstallPhase::Cancelled
            );
        } else {
            assert_eq!(settled(&installation).await, ProviderInstallPhase::Failed);
        }
        assert!(!installation.directory().join("active.json").exists());
    }
}
#[tokio::test]
async fn native_path_discovery_skips_incomplete_pairs_and_keeps_cli_launcher_names() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    let name = if cfg!(windows) {
        "agy_acp_server.exe"
    } else {
        "agy_acp_server.par"
    };
    for path in [
        first.join(name),
        second.join(name),
        second.join(if cfg!(windows) {
            "localharness_external.exe"
        } else {
            "localharness_external"
        }),
    ] {
        std::fs::write(&path, b"runtime").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let releases = Arc::new(Releases {
        asset: release(b"native runtime"),
        bytes: vec![],
        downloads: AtomicUsize::new(0),
        block: None,
    });
    let installation = installer(temp.path(), releases);
    let environment = Environment::from([(
        "PATH".into(),
        format!(
            "{}{}{}",
            first.display(),
            if cfg!(windows) { ';' } else { ':' },
            second.display()
        ),
    )]);
    let resolved = installation.resolve(None, &environment).await.unwrap();
    assert_eq!(
        resolved.executable,
        std::fs::canonicalize(second.join(name)).unwrap()
    );
    assert_eq!(resolved.source, ExecutableSource::Path);
}

#[tokio::test]
async fn legacy_zip_contains_only_the_two_pinned_regular_members_and_never_extracts_an_extra_path()
{
    use sha2::Sha256;
    use std::io::Write;
    let (executable, harness) = if cfg!(windows) {
        ("agy_acp_server.exe", "localharness_external.exe")
    } else {
        ("agy_acp_server.par", "localharness_external")
    };
    for unsafe_entry in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(0o755);
        zip.start_file(executable, options).unwrap();
        zip.write_all(b"native runtime").unwrap();
        zip.start_file(if unsafe_entry { "../escaped" } else { harness }, options)
            .unwrap();
        zip.write_all(b"native harness").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let release = Release {
            version: "1.4.5".into(),
            url: "https://example.invalid/legacy.zip".into(),
            sha256: Some(format!("{:x}", Sha256::digest(&bytes))),
            sha512: None,
            archive_bytes: Some(bytes.len() as u64),
            executable: Member {
                name: executable.into(),
                bytes: Some(14),
            },
            harness: Some(Member {
                name: harness.into(),
                bytes: Some(14),
            }),
            format: Format::Zip,
        };
        let releases = Arc::new(Releases {
            asset: release,
            bytes,
            downloads: AtomicUsize::new(0),
            block: None,
        });
        let installation = installer(temp.path(), releases);
        installation.start().await.unwrap();
        assert_eq!(
            settled(&installation).await,
            if unsafe_entry {
                ProviderInstallPhase::Failed
            } else {
                ProviderInstallPhase::Succeeded
            }
        );
        assert!(!temp.path().join("escaped").exists());
        if !unsafe_entry {
            assert!(
                installation
                    .resolve(None, &Environment::new())
                    .await
                    .unwrap()
                    .harness
                    .is_some()
            );
        }
    }
}
