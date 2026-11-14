use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Write},
    sync::Arc,
};
use t3_host::{pinned_runtime::*, self_update::*};
use tokio::sync::Mutex;
struct Host {
    archive: Vec<u8>,
    checksum: bool,
    preflight: serde_json::Value,
    urls: Mutex<Vec<String>>,
}
#[async_trait]
impl RuntimeInstallHost for Host {
    async fn fetch(&self, url: &str, _: usize) -> UpdateResult<ReleaseResponse> {
        self.urls.lock().await.push(url.into());
        let body = if url.contains("SHA256SUMS") {
            format!(
                "{}  t3-1.1.0-win32-x64.zip\n",
                if self.checksum {
                    hex::encode(Sha256::digest(&self.archive))
                } else {
                    "0".repeat(64)
                }
            )
            .into_bytes()
        } else {
            self.archive.clone()
        };
        Ok(ReleaseResponse { status: 200, body })
    }
    async fn preflight(&self, options: t3_process::ProcessOptions) -> UpdateResult<Vec<u8>> {
        let args: Vec<_> = options.args.iter().map(|a| a.to_string_lossy()).collect();
        assert_eq!(args[0], "__service-preflight");
        assert_eq!(args[4], "3");
        assert_eq!(options.timeout, Some(std::time::Duration::from_secs(180)));
        Ok(serde_json::to_vec(&self.preflight).unwrap())
    }
}
fn archive(name: &str) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(name, zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"executable fixture").unwrap();
    zip.finish().unwrap().into_inner()
}
fn request() -> RuntimeRequest {
    RuntimeRequest {
        target_version: "1.1.0".into(),
        database_path: "state.sqlite".into(),
        launcher_protocol: 3,
        release_base_url: Some("https://user:password@mirror.example/feed?token=secret".into()),
        require_published_release: false,
        mirror_organization: None,
    }
}
#[tokio::test]
async fn checksum_preflight_and_safe_paths_gate_atomic_publication_and_cached_revalidation() {
    for case in [
        "valid",
        "checksum",
        "version",
        "protocol",
        "blocked",
        "traversal",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let preflight = match case {
            "version" => {
                serde_json::json!({"status":"ready","version":"9.9.9","launcherProtocol":3})
            }
            "protocol" => {
                serde_json::json!({"status":"ready","version":"1.1.0","launcherProtocol":4})
            }
            "blocked" => {
                serde_json::json!({"status":"blocked","version":"1.1.0","reason":"local update required"})
            }
            _ => serde_json::json!({"status":"ready","version":"1.1.0","launcherProtocol":3}),
        };
        let h = Arc::new(Host {
            archive: archive(if case == "traversal" {
                "stem/../../escape"
            } else {
                "stem/t3.exe"
            }),
            checksum: case != "checksum",
            preflight,
            urls: Mutex::new(vec![]),
        });
        let installer =
            PinnedRuntimeInstaller::new(temp.path().into(), "win32-x64".into(), h.clone());
        let result = installer.ensure(request()).await;
        let published = temp.path().join("runtime/versions/1.1.0/.install-complete");
        if case == "valid" {
            assert!(result.unwrap().is_file());
            assert_eq!(std::fs::read_to_string(&published).unwrap(), "1.1.0\n");
            installer.ensure(request()).await.unwrap();
            assert_eq!(h.urls.lock().await.len(), 2);
            let mut r = request();
            r.require_published_release = true;
            installer.ensure(r).await.unwrap();
            assert_eq!(h.urls.lock().await.len(), 3);
            for url in h.urls.lock().await.iter() {
                assert!(url.contains("/feed/v1.1.0/"));
                assert!(url.ends_with("?token=secret"));
            }
        } else {
            let error = result.unwrap_err();
            assert!(!format!("{error:?}").contains("password"));
            assert!(!format!("{error:?}").contains("token=secret"));
            assert!(!published.exists());
            assert!(!temp.path().join("escape").exists());
            assert_eq!(
                std::fs::read_dir(temp.path().join("runtime/versions"))
                    .unwrap()
                    .count(),
                0,
                "{case}"
            );
        }
    }
}

#[tokio::test]
async fn runtime_pruning_keeps_references_young_incomplete_links_and_unreadable_state() {
    struct References(Option<String>);
    #[async_trait]
    impl RuntimeReferences for References {
        async fn running_executables(&self) -> t3_host::Result<String> {
            self.0.clone().ok_or(t3_host::Error::Service)
        }
    }
    for case in [
        "unused",
        "running",
        "state",
        "unknown-state",
        "unknown-processes",
        "young",
        "launcher",
        "link",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let versions = temp.path().join("runtime/versions");
        let stale = versions.join("1.0.0");
        let current = versions.join("1.1.0");
        for path in [&stale, &current] {
            std::fs::create_dir_all(path).unwrap();
            std::fs::write(path.join("t3.exe"), "runtime").unwrap();
            std::fs::write(
                path.join(".install-complete"),
                format!("{}\n", path.file_name().unwrap().to_string_lossy()),
            )
            .unwrap();
            filetime::set_file_mtime(
                path.join(".install-complete"),
                filetime::FileTime::from_unix_time(if case == "young" { 1800000000 } else { 1 }, 0),
            )
            .unwrap();
        }
        let incomplete = versions.join("0.9.0");
        std::fs::create_dir(&incomplete).unwrap();
        if case == "state" {
            std::fs::write(
                temp.path().join("runtime/service-state.json"),
                r#"{"protocol":4,"activeVersion":"1.0.0"}"#,
            )
            .unwrap();
        }
        if case == "unknown-state" {
            std::fs::write(temp.path().join("runtime/service-state.json"), "malformed").unwrap();
        }
        let linked = versions.join("0.8.0");
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("keep"), "safe").unwrap();
        if case == "link" {
            #[cfg(windows)]
            junction::create(&outside, &linked).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(&outside, &linked).unwrap();
        }
        let references = References(if case == "unknown-processes" {
            None
        } else if case == "running" {
            Some(stale.join("t3.exe").to_string_lossy().to_uppercase())
        } else {
            Some("other executable".into())
        });
        let launcher = temp.path().join("bin/t3.cmd");
        std::fs::create_dir(launcher.parent().unwrap()).unwrap();
        if case == "launcher" {
            std::fs::write(
                &launcher,
                format!("@echo off\r\n\"{}\" %*\r\n", stale.join("t3.exe").display()),
            )
            .unwrap();
            std::fs::write(
                temp.path().join("runtime/launchers"),
                launcher.to_string_lossy().as_bytes(),
            )
            .unwrap();
        }
        let removed = prune_pinned_runtimes(
            temp.path(),
            &["1.1.0".into()],
            &[],
            1800000000000,
            &references,
            &LauncherSearch {
                windows: true,
                home: temp.path().into(),
                environment: Default::default(),
            },
        )
        .await
        .unwrap();
        let should_remove = matches!(case, "unused" | "link");
        assert_eq!(!stale.exists(), should_remove, "{case}: {removed:?}");
        assert!(current.exists());
        assert!(incomplete.exists());
        assert!(outside.join("keep").exists());
    }
}

#[tokio::test]
async fn installing_never_deletes_a_runtime_published_by_another_process_during_download() {
    struct Race {
        delegate: Host,
        destination: std::path::PathBuf,
    }
    #[async_trait]
    impl RuntimeInstallHost for Race {
        async fn fetch(&self, url: &str, limit: usize) -> UpdateResult<ReleaseResponse> {
            let result = self.delegate.fetch(url, limit).await?;
            if url.contains("SHA256SUMS") {
                std::fs::create_dir_all(&self.destination).unwrap();
                std::fs::write(self.destination.join("t3.exe"), "concurrent winner").unwrap();
                std::fs::write(self.destination.join(".install-complete"), "1.1.0\n").unwrap();
            }
            Ok(result)
        }
        async fn preflight(&self, options: t3_process::ProcessOptions) -> UpdateResult<Vec<u8>> {
            self.delegate.preflight(options).await
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("runtime/versions/1.1.0");
    let host = Arc::new(Race {
        delegate: Host {
            archive: archive("stem/t3.exe"),
            checksum: true,
            preflight: serde_json::json!({"status":"ready","version":"1.1.0","launcherProtocol":3}),
            urls: Mutex::new(vec![]),
        },
        destination: destination.clone(),
    });
    let installer = PinnedRuntimeInstaller::new(temp.path().into(), "win32-x64".into(), host);
    installer.ensure(request()).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(destination.join("t3.exe")).unwrap(),
        "concurrent winner"
    );
}

#[tokio::test(start_paused = true)]
async fn download_retries_redact_private_urls_and_failed_installs_can_be_retried() {
    use std::sync::atomic::{AtomicU16, Ordering};
    struct Http {
        delegate: Host,
        status: AtomicU16,
    }
    #[async_trait]
    impl RuntimeInstallHost for Http {
        async fn fetch(&self, url: &str, limit: usize) -> UpdateResult<ReleaseResponse> {
            let result = self.delegate.fetch(url, limit).await?;
            let status = self.status.load(Ordering::SeqCst);
            if status == 200 {
                Ok(result)
            } else {
                Ok(ReleaseResponse {
                    status,
                    body: b"private response body".to_vec(),
                })
            }
        }
        async fn preflight(&self, options: t3_process::ProcessOptions) -> UpdateResult<Vec<u8>> {
            self.delegate.preflight(options).await
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let h = Arc::new(Http {
        delegate: Host {
            archive: archive("stem/t3.exe"),
            checksum: true,
            preflight: serde_json::json!({"status":"ready","version":"1.1.0","launcherProtocol":3}),
            urls: Mutex::new(vec![]),
        },
        status: AtomicU16::new(503),
    });
    let installer = PinnedRuntimeInstaller::new(temp.path().into(), "win32-x64".into(), h.clone());
    let error = installer.ensure(request()).await.unwrap_err();
    assert_eq!(h.delegate.urls.lock().await.len(), 4);
    assert_eq!(
        error.reason.0,
        "Could not prepare t3@1.1.0 while downloading the t3 release checksums (HTTP 503 from mirror.example)."
    );
    for secret in ["password", "token=secret", "private response body"] {
        assert!(!format!("{error:?}").contains(secret));
    }
    assert_eq!(
        std::fs::read_dir(temp.path().join("runtime/versions"))
            .unwrap()
            .count(),
        0
    );
    h.status.store(200, Ordering::SeqCst);
    installer.ensure(request()).await.unwrap();
}

#[tokio::test]
async fn startup_prunes_only_owned_runtimes_and_preserves_the_installed_units_launcher() {
    struct References;
    #[async_trait]
    impl RuntimeReferences for References {
        async fn running_executables(&self) -> t3_host::Result<String> {
            Ok("other executable".into())
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let versions = temp.path().join("runtime/versions");
    let stale = versions.join("1.0.0");
    let current = versions.join("1.1.0");
    for (path, version) in [(&stale, "1.0.0"), (&current, "1.1.0")] {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join("t3.exe"), "runtime").unwrap();
        std::fs::write(path.join(".install-complete"), version).unwrap();
        filetime::set_file_mtime(
            path.join(".install-complete"),
            filetime::FileTime::from_unix_time(1, 0),
        )
        .unwrap();
    }
    let installer = PinnedRuntimeInstaller::new(
        temp.path().into(),
        "win32-x64".into(),
        Arc::new(Host {
            archive: vec![],
            checksum: false,
            preflight: serde_json::Value::Null,
            urls: Mutex::new(vec![]),
        }),
    );
    let search = LauncherSearch {
        windows: true,
        home: temp.path().into(),
        environment: Default::default(),
    };
    let unrelated = temp.path().join("other.exe");
    assert!(
        installer
            .prune_on_start(
                RuntimeStartup {
                    current_version: "1.1.0",
                    managed: false,
                    executable: &unrelated,
                    unit_program: None
                },
                1800000000000,
                &References,
                &search
            )
            .await
            .unwrap()
            .is_empty()
    );
    assert!(stale.exists());
    let executable = dunce::canonicalize(current.join("t3.exe")).unwrap();
    let unit = stale.join("t3.exe");
    assert!(
        installer
            .prune_on_start(
                RuntimeStartup {
                    current_version: "1.1.0",
                    managed: false,
                    executable: &executable,
                    unit_program: Some(&unit)
                },
                1800000000000,
                &References,
                &search
            )
            .await
            .unwrap()
            .is_empty()
    );
    assert!(stale.exists());
    assert_eq!(
        installer
            .prune_on_start(
                RuntimeStartup {
                    current_version: "1.1.0",
                    managed: true,
                    executable: &executable,
                    unit_program: None
                },
                1800000000000,
                &References,
                &search
            )
            .await
            .unwrap(),
        vec!["1.0.0"]
    );
    assert!(current.exists());
}
