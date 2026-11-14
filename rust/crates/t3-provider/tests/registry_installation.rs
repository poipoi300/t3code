#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use t3_provider::{
    acp_service::AcpResult,
    environment::{Environment, Platform},
    registry::*,
    registry_installation::*,
    sensitive::Sensitive,
    transport::SystemClock,
};
struct Commands;
#[async_trait]
impl RegistryCommands for Commands {
    fn resolve(&self, _: &str, _: &Environment) -> Option<PathBuf> {
        None
    }
    async fn run(
        &self,
        _: &Path,
        _: &[String],
        _: &Environment,
        _: Duration,
        _: usize,
    ) -> AcpResult<Sensitive<String>> {
        panic!("raw binaries need no command or provider probe")
    }
}
struct Download {
    bytes: Vec<u8>,
    calls: AtomicUsize,
}
#[async_trait]
impl RegistryDownloads for Download {
    async fn download(&self, _: &str, path: &Path) -> AcpResult<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::fs::write(path, &self.bytes).unwrap();
        Ok(())
    }
}
fn agent() -> Agent {
    serde_json::from_value(json!({"id":"assistant","name":"Assistant","version":"1.0.0","description":"Fixture","distribution":{"binary":{"windows-x86_64":{"archive":"https://archive.invalid/agent.bin","cmd":"bin/agent.exe","sha256":format!("{:x}",Sha256::digest(b"fixture bytes")),"env":{"TOKEN":"never-format"},"args":["never-format"]}}}})).unwrap()
}
fn make_installer(root: &Path, download: Arc<Download>) -> RegistryBinaryInstaller {
    RegistryBinaryInstaller {
        tools: root.join("tools"),
        platform: Platform::current(),
        target: "windows-x86_64".into(),
        environment: Environment::new(),
        commands: Arc::new(Commands),
        downloads: download,
        clock: Arc::new(SystemClock),
    }
}
#[tokio::test]
async fn registry_raw_install_checks_checksum_publishes_only_complete_commands_and_reuses_cache() {
    let root = tempfile::tempdir().unwrap();
    let download = Arc::new(Download {
        bytes: b"fixture bytes".to_vec(),
        calls: AtomicUsize::new(0),
    });
    let installer = make_installer(root.path(), download.clone());
    let agent = agent();
    let target = &agent.distribution.binary.as_ref().unwrap()["windows-x86_64"];
    assert!(!format!("{agent:?}").contains("never-format"));
    let executable = installer.install(&agent, target).await.unwrap();
    assert_eq!(std::fs::read(&executable).unwrap(), b"fixture bytes");
    assert_eq!(installer.install(&agent, target).await.unwrap(), executable);
    assert_eq!(download.calls.load(Ordering::SeqCst), 1);
    std::fs::remove_file(executable).unwrap();
    let corrupt = Arc::new(Download {
        bytes: b"corrupt".to_vec(),
        calls: AtomicUsize::new(0),
    });
    let installer = make_installer(root.path(), corrupt);
    let error = installer.install(&agent, target).await.unwrap_err();
    assert_eq!(
        error.reason,
        t3_contracts::acp_registry::AcpRegistryOperationErrorReason::ChecksumMismatch
    );
    assert!(!format!("{error:?}").contains("never-format"));
    assert!(
        !root
            .path()
            .join("tools/assistant/1.0.0/windows-x86_64/bin/agent.exe")
            .exists()
    );
    assert!(
        !std::fs::read_dir(root.path().join("tools/assistant/1.0.0"))
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("lock"))
    );
}
#[tokio::test]
async fn cached_registry_roots_cannot_redirect_to_an_outside_directory() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(outside.path().join("bin")).unwrap();
    std::fs::write(outside.path().join("bin/agent.exe"), b"outside").unwrap();
    let parent = root.path().join("tools/assistant/1.0.0");
    std::fs::create_dir_all(&parent).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(outside.path(), parent.join("windows-x86_64")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), parent.join("windows-x86_64")).unwrap();
    let download = Arc::new(Download {
        bytes: vec![],
        calls: AtomicUsize::new(0),
    });
    let installer = make_installer(root.path(), download.clone());
    let agent = agent();
    assert!(
        installer
            .install(
                &agent,
                &agent.distribution.binary.as_ref().unwrap()["windows-x86_64"]
            )
            .await
            .is_err()
    );
    assert_eq!(download.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::fs::read(outside.path().join("bin/agent.exe")).unwrap(),
        b"outside"
    );
}

#[tokio::test]
async fn registry_archives_are_listed_and_extracted_with_real_tar_before_atomic_publication() {
    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("package");
    std::fs::create_dir_all(package.join("bin")).unwrap();
    std::fs::write(package.join("bin/agent.exe"), b"fixture executable").unwrap();
    let archive = root.path().join("recorded.tar.gz");
    let mut environment = Environment::new();
    for name in ["PATH", "SystemRoot"] {
        if let Ok(value) = std::env::var(name) {
            environment.insert(name.into(), value);
        }
    }
    let tar = if cfg!(windows) {
        PathBuf::from(environment.get("SystemRoot").unwrap()).join("System32/tar.exe")
    } else {
        PathBuf::from("tar")
    };
    let mut options = t3_provider::environment::process_options(
        tar,
        &[
            "-czf".into(),
            archive.to_string_lossy().into_owned(),
            "-C".into(),
            package.to_string_lossy().into_owned(),
            ".".into(),
        ],
        &environment,
    );
    options.cwd = Some(root.path().into());
    t3_process::run(options).await.unwrap();
    let bytes = std::fs::read(archive).unwrap();
    let mut agent = agent();
    let target = agent
        .distribution
        .binary
        .as_mut()
        .unwrap()
        .get_mut("windows-x86_64")
        .unwrap();
    target.archive = "https://archive.invalid/recorded.tar.gz".into();
    target.sha256 = Some(format!("{:x}", Sha256::digest(&bytes)));
    let installer = RegistryBinaryInstaller {
        tools: root.path().join("tools"),
        platform: Platform::current(),
        target: "windows-x86_64".into(),
        environment,
        commands: Arc::new(ProcessRegistryCommands {
            cwd: root.path().into(),
        }),
        downloads: Arc::new(Download {
            bytes,
            calls: AtomicUsize::new(0),
        }),
        clock: Arc::new(SystemClock),
    };
    let executable = installer
        .install(
            &agent,
            &agent.distribution.binary.as_ref().unwrap()["windows-x86_64"],
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(executable).unwrap(), b"fixture executable");
}

struct WindowsZip;
#[async_trait]
impl RegistryCommands for WindowsZip {
    fn resolve(&self, _: &str, _: &Environment) -> Option<PathBuf> {
        None
    }
    async fn run(
        &self,
        command: &Path,
        args: &[String],
        _: &Environment,
        _: Duration,
        _: usize,
    ) -> AcpResult<Sensitive<String>> {
        if command != Path::new("D:/Windows").join("System32").join("tar.exe") {
            return Err(t3_provider::acp_service::AcpError::new(
                t3_contracts::acp_registry::AcpRegistryOperationErrorReason::InstallFailed,
                "The PATH tar does not support zip archives.",
            ));
        }
        if args[0] == "-tf" {
            return Ok(Sensitive::new("bin/agent.exe\n".into()));
        }
        let root = Path::new(&args[3]);
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin/agent.exe"), b"fixture executable").unwrap();
        Ok(Sensitive::new(String::new()))
    }
}
#[tokio::test]
async fn windows_arm_zip_fallback_uses_system_bsdtar_even_when_path_tar_cannot_read_zip() {
    let root = tempfile::tempdir().unwrap();
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("bin/agent.exe", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"fixture executable").unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    let mut agent = agent();
    let target = agent
        .distribution
        .binary
        .as_mut()
        .unwrap()
        .get_mut("windows-x86_64")
        .unwrap();
    target.archive = "https://archive.invalid/recorded.zip".into();
    target.sha256 = Some(format!("{:x}", Sha256::digest(&bytes)));
    let target = target.clone();
    let installer = RegistryBinaryInstaller {
        tools: root.path().join("tools"),
        platform: Platform::Windows,
        target: "windows-aarch64".into(),
        environment: Environment::from([("SYSTEMROOT".into(), "D:/Windows".into())]),
        commands: Arc::new(WindowsZip),
        downloads: Arc::new(Download {
            bytes,
            calls: AtomicUsize::new(0),
        }),
        clock: Arc::new(SystemClock),
    };
    assert_eq!(
        std::fs::read(installer.install(&agent, &target).await.unwrap()).unwrap(),
        b"fixture executable"
    );
}

struct ListedArchive {
    verbose: String,
    extractions: AtomicUsize,
}
#[async_trait]
impl RegistryCommands for ListedArchive {
    fn resolve(&self, _: &str, _: &Environment) -> Option<PathBuf> {
        None
    }
    async fn run(
        &self,
        _: &Path,
        args: &[String],
        _: &Environment,
        _: Duration,
        _: usize,
    ) -> AcpResult<Sensitive<String>> {
        if args[0].starts_with("-t") {
            return Ok(Sensitive::new(if args[0].contains('v') {
                self.verbose.clone()
            } else {
                "bin/agent.exe\n".repeat(self.verbose.lines().count())
            }));
        }
        self.extractions.fetch_add(1, Ordering::SeqCst);
        let root = Path::new(&args[3]);
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin/agent.exe"), b"fixture executable").unwrap();
        Ok(Sensitive::new(String::new()))
    }
}
async fn refuses_listed_archive(verbose: &str) {
    let root = tempfile::tempdir().unwrap();
    let commands = Arc::new(ListedArchive {
        verbose: verbose.into(),
        extractions: AtomicUsize::new(0),
    });
    let mut agent = agent();
    let target = agent
        .distribution
        .binary
        .as_mut()
        .unwrap()
        .get_mut("windows-x86_64")
        .unwrap();
    target.archive = "https://archive.invalid/recorded.tar.gz".into();
    let target = target.clone();
    let mut installer = make_installer(
        root.path(),
        Arc::new(Download {
            bytes: b"fixture bytes".to_vec(),
            calls: AtomicUsize::new(0),
        }),
    );
    installer.commands = commands.clone();
    assert!(
        installer.install(&agent, &target).await.is_err(),
        "accepted {verbose}"
    );
    assert_eq!(commands.extractions.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn registry_archive_rejects_symlinks_before_extraction() {
    refuses_listed_archive("lrwxrwxrwx 0 user group 0 Nov 14 01:00 bin/agent.exe -> outside\n")
        .await;
}
#[tokio::test]
async fn registry_archive_rejects_hardlinks_before_extraction() {
    refuses_listed_archive(
        "hrw-r--r-- 0 user group 0 Nov 14 01:00 bin/agent.exe link to outside\n",
    )
    .await;
}
#[tokio::test]
async fn registry_archive_rejects_oversize_expansion_before_extraction() {
    refuses_listed_archive("-rw-r--r-- 0 user group 1073741825 Nov 14 01:00 bin/agent.exe\n").await;
}

#[tokio::test]
async fn registry_archive_caps_the_sum_of_regular_file_sizes() {
    refuses_listed_archive("-rw-r--r-- 0/0 536870913 2026-11-14 01:00 bin/agent.exe\n-rw-r--r-- 0/0 536870913 2026-11-14 01:00 another-file\n").await;
}
#[tokio::test]
async fn registry_zip_rejects_symlink_metadata_before_native_extraction() {
    let root = tempfile::tempdir().unwrap();
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.add_symlink(
        "bin/agent.exe",
        "outside",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    let mut agent = agent();
    let target = agent
        .distribution
        .binary
        .as_mut()
        .unwrap()
        .get_mut("windows-x86_64")
        .unwrap();
    target.archive = "https://archive.invalid/recorded.zip".into();
    target.sha256 = Some(format!("{:x}", Sha256::digest(&bytes)));
    let target = target.clone();
    let installer = RegistryBinaryInstaller {
        tools: root.path().join("tools"),
        platform: Platform::Windows,
        target: "windows-x86_64".into(),
        environment: Environment::from([("SYSTEMROOT".into(), "D:/Windows".into())]),
        commands: Arc::new(WindowsZip),
        downloads: Arc::new(Download {
            bytes,
            calls: AtomicUsize::new(0),
        }),
        clock: Arc::new(SystemClock),
    };
    assert_eq!(
        installer.install(&agent, &target).await.unwrap_err().reason,
        t3_contracts::acp_registry::AcpRegistryOperationErrorReason::ArchiveInvalid
    );
    assert!(
        !root
            .path()
            .join("tools/assistant/1.0.0/windows-x86_64")
            .exists()
    );
}
