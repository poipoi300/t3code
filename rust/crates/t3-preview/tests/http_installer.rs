#![allow(clippy::unwrap_used)]
use std::{
    io::{Cursor, Write},
    sync::{Arc, Mutex},
    time::Duration,
};
use t3_preview::{
    discovery::PortDiscovery,
    installer::{BrowserRelease, PreviewBrowser, extract_archive, pinned_release},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

struct HttpFixture {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl HttpFixture {
    async fn new(
        handler: impl Fn(&str) -> (u16, Vec<(String, String)>, Vec<u8>) + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let handler = handler.clone();
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut bytes = vec![];
                    let mut chunk = [0u8; 1024];
                    while !bytes.windows(4).any(|b| b == b"\r\n\r\n") {
                        let count = socket.read(&mut chunk).await.unwrap();
                        if count == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&chunk[..count]);
                        if bytes.len() > 16384 {
                            return;
                        }
                    }
                    let text = String::from_utf8_lossy(&bytes);
                    let target = text.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    seen.lock().unwrap().push(target.clone());
                    let (status, headers, body) = handler(&target);
                    let mut response = format!(
                        "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n",
                        body.len()
                    );
                    for (key, value) in headers {
                        response.push_str(&format!("{key}: {value}\r\n"));
                    }
                    response.push_str("\r\n");
                    socket.write_all(response.as_bytes()).await.unwrap();
                    let _ = socket.write_all(&body).await;
                });
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }
}

#[tokio::test]
async fn discovery_uses_real_http_paths_and_does_not_follow_redirects() {
    let http = HttpFixture::new(|target| {
        if target == "/docs" {
            (
                302,
                vec![("Location".into(), "/must-not-follow".into())],
                vec![],
            )
        } else {
            (500, vec![], vec![])
        }
    })
    .await;
    let scanner = PortDiscovery::with_options(false, std::path::PathBuf::from("not-used")).unwrap();
    let configured = format!("{}/docs#first", http.url);
    let listed = scanner
        .scan(std::slice::from_ref(&configured))
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].url.as_str(), configured);
    assert_eq!(listed[0].pid, None);
    let next = format!("{}/docs#second", http.url);
    assert_eq!(
        scanner.scan(std::slice::from_ref(&next)).await.unwrap()[0]
            .url
            .as_str(),
        next
    );
    assert_eq!(*http.requests.lock().unwrap(), vec!["/docs"]);
}

#[tokio::test]
async fn discovery_filters_real_non_document_responses_and_keeps_subscribers_separate() {
    let http = HttpFixture::new(|target| match target {
        "/docs" | "/admin" => (
            200,
            vec![("Content-Type".into(), "text/html; charset=utf-8".into())],
            b"<p>app</p>".to_vec(),
        ),
        "/json" => (
            200,
            vec![("Content-Type".into(), "application/json".into())],
            b"{}".to_vec(),
        ),
        _ => (
            404,
            vec![("Content-Type".into(), "text/html".into())],
            b"missing".to_vec(),
        ),
    })
    .await;
    let scanner = PortDiscovery::with_options(false, std::path::PathBuf::new()).unwrap();
    assert!(
        scanner
            .scan(&[
                format!("{}/json", http.url),
                format!("{}/missing", http.url)
            ])
            .await
            .unwrap()
            .is_empty()
    );
    let mut docs = scanner
        .subscribe(vec![format!("{}/docs", http.url)])
        .await
        .unwrap();
    let mut admin = scanner
        .subscribe(vec![format!("{}/admin", http.url)])
        .await
        .unwrap();
    assert!(
        docs.events.recv().await.unwrap().servers[0]
            .url
            .as_str()
            .ends_with("/docs")
    );
    assert!(
        admin.events.recv().await.unwrap().servers[0]
            .url
            .as_str()
            .ends_with("/admin")
    );
    scanner.poll().await.unwrap();
    assert!(docs.events.try_recv().is_err());
    assert!(admin.events.try_recv().is_err());
    scanner.shutdown().await;
    assert!(docs.events.recv().await.is_none());
    assert!(admin.events.recv().await.is_none());
    assert!(scanner.subscribe(vec![]).await.is_err());
    assert!(scanner.retain().await.is_err());
}

#[cfg(windows)]
#[tokio::test]
async fn windows_stop_is_unsupported_even_for_the_current_process() {
    let scanner = PortDiscovery::new().unwrap();
    let error = scanner
        .stop_server(t3_contracts::preview::PreviewStopLocalServerInput {
            pid: std::process::id() as u64,
            port: 3000,
        })
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        t3_preview::Error::Stop(t3_contracts::preview::PreviewStopLocalServerReason::Unsupported)
    ));
}

fn archive(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes, mode) in entries {
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(*mode);
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
fn browser_name() -> &'static str {
    if cfg!(windows) {
        "chrome-headless-shell.exe"
    } else {
        "chrome-headless-shell"
    }
}

#[tokio::test]
async fn browser_installer_verifies_real_download_and_reuses_installed_files() {
    let binary = format!("chrome-headless-shell-fixture/{}", browser_name());
    let bytes = archive(&[
        (&binary, b"browser fixture", 0o755),
        (
            "chrome-headless-shell-fixture/locales/en-US.pak",
            b"strings",
            0o644,
        ),
    ]);
    let body = bytes.clone();
    let http = HttpFixture::new(move |_| (200, vec![], body.clone())).await;
    let root = tempfile::tempdir().unwrap();
    let hash = {
        use sha2::Digest;
        format!("{:x}", sha2::Sha256::digest(&bytes))
    };
    let install_root = root.path().join("tools/chrome-headless-shell/fixture");
    for name in ["1.0.0", "1.1.0", ".install-fresh", ".install-old"] {
        std::fs::create_dir_all(install_root.join(name)).unwrap();
    }
    let old = std::fs::File::open(install_root.join(".install-old")).ok();
    #[cfg(unix)]
    old.unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::UNIX_EPOCH))
        .unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .access_mode(0x100)
            .custom_flags(0x02000000)
            .open(install_root.join(".install-old"))
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::UNIX_EPOCH))
            .unwrap();
    }
    #[cfg(not(unix))]
    drop(old);
    let release = BrowserRelease {
        version: "1.2.3".into(),
        platform: "fixture".into(),
        url: http.url.clone(),
        bytes: bytes.len() as u64,
        sha256: hash,
    };
    let installer = PreviewBrowser::with_release(
        root.path().to_owned(),
        Some(release),
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(installer.installed().is_none());
    let path = installer.executable().await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"browser fixture");
    assert_eq!(
        std::fs::read(path.parent().unwrap().join("locales/en-US.pak")).unwrap(),
        b"strings"
    );
    assert_eq!(installer.executable().await.unwrap(), path);
    assert_eq!(installer.installed().unwrap(), path);
    assert_eq!(http.requests.lock().unwrap().len(), 1);
    assert!(!install_root.join("1.0.0").exists());
    assert!(install_root.join("1.1.0").is_dir());
    assert!(install_root.join(".install-fresh").is_dir());
    assert!(!install_root.join(".install-old").exists());
}

#[tokio::test]
async fn bad_hash_and_wrong_size_install_nothing_and_each_call_can_retry() {
    for wrong_size in [false, true] {
        let binary = format!("chrome-headless-shell-fixture/{}", browser_name());
        let bytes = archive(&[(&binary, b"fixture", 0o755)]);
        let body = bytes.clone();
        let http = HttpFixture::new(move |_| (200, vec![], body.clone())).await;
        let root = tempfile::tempdir().unwrap();
        let installer = PreviewBrowser::with_release(
            root.path().to_owned(),
            Some(BrowserRelease {
                version: "1.2.3".into(),
                platform: "fixture".into(),
                url: http.url.clone(),
                bytes: bytes.len() as u64 + u64::from(wrong_size),
                sha256: "0".repeat(64),
            }),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(
            installer.executable().await.unwrap_err().tag(),
            "PreviewBrowserInstallError"
        );
        assert!(installer.installed().is_none());
        assert_eq!(
            std::fs::read_dir(root.path().join("tools/chrome-headless-shell/fixture"))
                .unwrap()
                .count(),
            0
        );
        assert!(installer.executable().await.is_err());
        assert_eq!(http.requests.lock().unwrap().len(), 2);
    }
}

#[test]
fn archive_rejects_traversal_absolute_paths_symlinks_and_missing_executable() {
    let root = tempfile::tempdir().unwrap();
    let binary = format!("chrome-headless-shell-fixture/{}", browser_name());
    for (index, evil) in [
        "chrome-headless-shell-fixture/../escape",
        "/escape",
        "outside",
        "chrome-headless-shell-fixture/..\\escape",
        "chrome-headless-shell-fixture/C:/escape",
    ]
    .iter()
    .enumerate()
    {
        let bytes = archive(&[(&binary, b"fixture", 0o755), (evil, b"escaped", 0o644)]);
        let file = root.path().join(format!("{index}.zip"));
        std::fs::write(&file, bytes).unwrap();
        assert!(
            extract_archive(
                &file,
                &root.path().join(format!("out-{index}")),
                "fixture",
                browser_name()
            )
            .is_err()
        );
        assert!(!root.path().join("escape").exists());
    }
    let bytes = archive(&[("chrome-headless-shell-fixture/only-library.so", b"x", 0o755)]);
    let file = root.path().join("missing.zip");
    std::fs::write(&file, bytes).unwrap();
    assert!(
        extract_archive(
            &file,
            &root.path().join("missing"),
            "fixture",
            browser_name()
        )
        .is_err()
    );
}

#[test]
fn pinned_platform_selection_is_complete_and_release_paths_are_validated() {
    for (os, arch, bytes, hash) in [
        (
            "linux",
            "x64",
            127965683,
            "f90329720fb61e5a76da9b6b509b6ad12a484c9c5df0359403e935177c6d153a",
        ),
        (
            "linux",
            "arm64",
            128047138,
            "912fbf877e53de34d7b3cca5104f3b95607d41bd35412ccc6cd2a599ff0c0dbf",
        ),
        (
            "darwin",
            "arm64",
            104822985,
            "17bf66d53c1b1104dcb641c5b331cd4ac5d200c778f5feed0934a0b92deaf8d6",
        ),
        (
            "darwin",
            "x64",
            110132547,
            "f1a6a9561eaf1f409849538de663695d36d72fbf1ebca4d42e47c7b5f2a157f3",
        ),
        (
            "win32",
            "ia32",
            120403126,
            "d9829c59dcc79d9e6e920f8ab841f2a9a055dcf54229d756ac0b42dfec303910",
        ),
        (
            "win32",
            "x64",
            126047001,
            "2138c1583d25d6f3abec5269943b2f430207d87a539895aa9e90cc4697ec0e1f",
        ),
    ] {
        let release = pinned_release(os, arch).unwrap();
        assert_eq!(release.version, "157.0.8103.78");
        assert_eq!(release.bytes, bytes);
        assert_eq!(release.sha256, hash);
    }
    for (platform, arch, expected) in [
        ("linux", "x64", "linux64"),
        ("linux", "arm64", "linux-arm64"),
        ("darwin", "x64", "mac-x64"),
        ("darwin", "arm64", "mac-arm64"),
        ("win32", "ia32", "win32"),
        ("win32", "arm64", "win64"),
    ] {
        let release = pinned_release(platform, arch).unwrap();
        assert_eq!(release.platform, expected);
        assert_eq!(release.sha256.len(), 64);
        assert!(release.url.contains(&release.version));
    }
    assert!(pinned_release("freebsd", "x64").is_none());
    let root = tempfile::tempdir().unwrap();
    assert!(
        PreviewBrowser::with_release(
            root.path().into(),
            Some(BrowserRelease {
                version: "../escape".into(),
                platform: "linux64".into(),
                url: "http://localhost/".into(),
                bytes: 1,
                sha256: "0".repeat(64)
            }),
            Duration::from_secs(1)
        )
        .is_err()
    );
}

#[tokio::test]
async fn cancelled_install_waiter_does_not_cancel_the_shared_download() {
    let binary = format!("chrome-headless-shell-fixture/{}", browser_name());
    let bytes = archive(&[(&binary, b"browser", 0o755)]);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let hash = {
        use sha2::Digest;
        format!("{:x}", sha2::Sha256::digest(&bytes))
    };
    let size = bytes.len();
    let (requested, request) = tokio::sync::oneshot::channel();
    let (respond, response) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0u8; 1024];
        assert!(socket.read(&mut buffer).await.unwrap() > 0);
        requested.send(()).unwrap();
        response.await.unwrap();
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        socket.write_all(&bytes).await.unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let installer = PreviewBrowser::with_release(
        root.path().into(),
        Some(BrowserRelease {
            version: "1.0.0".into(),
            platform: "fixture".into(),
            url,
            bytes: size as u64,
            sha256: hash,
        }),
        Duration::from_secs(5),
    )
    .unwrap();
    let first = {
        let installer = installer.clone();
        tokio::spawn(async move { installer.executable().await })
    };
    request.await.unwrap();
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let mut remaining = Vec::new();
    for _ in 0..8 {
        let installer = installer.clone();
        remaining.push(tokio::spawn(async move { installer.executable().await }));
    }
    respond.send(()).unwrap();
    for waiter in remaining {
        let path = waiter.await.unwrap().unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"browser");
    }
    server.await.unwrap();
    assert!(installer.installed().is_some());
}

#[tokio::test]
async fn clearing_an_unopened_profile_and_shutdown_need_neither_node_nor_chrome() {
    use t3_preview::{
        BrowserDriver,
        driver::{BrowserCommand, PlaywrightOptions},
        installer::ManagedBrowserDriver,
    };
    let root = tempfile::tempdir().unwrap();
    let profiles = root.path().join("profiles");
    std::fs::create_dir_all(profiles.join("default")).unwrap();
    std::fs::write(profiles.join("default/state"), b"cookies").unwrap();
    let driver = ManagedBrowserDriver::new(
        PreviewBrowser::with_release(root.path().into(), None, Duration::from_secs(1)).unwrap(),
        PlaywrightOptions {
            node: root.path().join("node-must-not-start"),
            cwd: root.path().into(),
            executable: root.path().join("chrome-must-not-start"),
            profiles_dir: profiles.clone(),
            downloads_dir: root.path().join("downloads"),
            attachments_dir: root.path().join("attachments"),
            playwright_module: None,
            sandbox: true,
        },
    );
    driver
        .call(
            BrowserCommand::ClearProfile {
                profile_id: "default".into(),
            },
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    assert!(!profiles.join("default").exists());
    driver
        .call(BrowserCommand::Shutdown, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(!root.path().join("tools").exists());
}

#[tokio::test]
async fn browser_installation_has_its_own_wait_budget_before_the_render_deadline() {
    use t3_preview::{
        BrowserDriver,
        driver::{BrowserCommand, PlaywrightOptions},
        installer::ManagedBrowserDriver,
    };
    let binary = format!("chrome-headless-shell-fixture/{}", browser_name());
    let bytes = archive(&[(&binary, b"not executable", 0o755)]);
    let hash = {
        use sha2::Digest;
        format!("{:x}", sha2::Sha256::digest(&bytes))
    };
    let size = bytes.len();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (requested, request) = tokio::sync::oneshot::channel();
    let (respond, response) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0u8; 1024];
        assert!(socket.read(&mut buffer).await.unwrap() > 0);
        requested.send(()).unwrap();
        response.await.unwrap();
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        socket.write_all(&bytes).await.unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let installer = PreviewBrowser::with_release(
        root.path().into(),
        Some(BrowserRelease {
            version: "1.0.0".into(),
            platform: "fixture".into(),
            url,
            bytes: size as u64,
            sha256: hash,
        }),
        Duration::from_secs(45),
    )
    .unwrap();
    let installing = {
        let installer = installer.clone();
        tokio::spawn(async move { installer.executable().await })
    };
    request.await.unwrap();
    let driver = ManagedBrowserDriver::new(
        installer,
        PlaywrightOptions {
            node: "node".into(),
            cwd: root.path().into(),
            executable: root.path().join("never-run"),
            profiles_dir: root.path().join("profiles"),
            downloads_dir: root.path().join("downloads"),
            attachments_dir: root.path().join("attachments"),
            playwright_module: None,
            sandbox: true,
        },
    );
    driver
        .call(
            BrowserCommand::Executable {
                path: root.path().join("never-run"),
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    tokio::time::pause();
    let mut capture = Box::pin(driver.call(
        BrowserCommand::HtmlCapture {
            html: "<p>test</p>".into(),
            width: 728,
            appearance: "dark".into(),
            fragment: String::new(),
        },
        Duration::from_secs(1),
    ));
    assert!(futures_util::poll!(capture.as_mut()).is_pending());
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(
        futures_util::poll!(capture.as_mut()).is_pending(),
        "installation consumed the render deadline"
    );
    drop(capture);
    tokio::time::resume();
    respond.send(()).unwrap();
    installing.await.unwrap().unwrap();
    server.await.unwrap();
    driver
        .call(BrowserCommand::Shutdown, Duration::from_secs(5))
        .await
        .unwrap();
}

#[cfg(windows)]
#[tokio::test]
async fn windows_native_listener_discovery_attributes_only_registered_terminal_pids() {
    let http = HttpFixture::new(|_| {
        (
            200,
            vec![("Content-Type".into(), "text/html".into())],
            b"ok".to_vec(),
        )
    })
    .await;
    let port = url::Url::parse(&http.url).unwrap().port().unwrap();
    let discovery = PortDiscovery::with_options(true, std::path::PathBuf::new()).unwrap();
    discovery.register_terminal_processes("thread", "terminal", &[std::process::id() as u64]);
    let servers = discovery
        .scan(std::slice::from_ref(&http.url))
        .await
        .unwrap();
    let server = servers.iter().find(|server| server.port == port).unwrap();
    assert_eq!(server.pid, Some(std::process::id() as u64));
    let owner = server.terminal.as_ref().unwrap();
    assert_eq!(owner.thread_id.0.as_str(), "thread");
    assert_eq!(owner.terminal_id.as_str(), "terminal");
    assert!(
        server
            .process_name
            .as_ref()
            .is_some_and(|name| !name.as_str().is_empty())
    );
    discovery.unregister_terminal("thread", "terminal");
    assert!(
        discovery
            .scan(&[])
            .await
            .unwrap()
            .iter()
            .all(|server| server.port != port)
    );
}
