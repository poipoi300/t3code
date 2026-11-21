use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_contracts::{
    assets::*,
    base_schemas::{ThreadId, TrimmedNonEmptyString as Text, TurnItemId},
};
use t3_workspace::{assets::*, attachments, paths::InternalPaths, project::FaviconResolver};
struct External;
impl ExternalAssets for External {
    fn tool_image<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        index: u64,
    ) -> AssetFuture<'a, Option<(String, Vec<u8>)>> {
        Box::pin(async move { Ok((index == 0).then(|| ("image/png".into(), vec![1, 2, 3]))) })
    }
    fn native_icon<'a>(
        &'a self,
        _: &'a NativeAppIconReference,
    ) -> AssetFuture<'a, Option<PathBuf>> {
        Box::pin(async { Ok(None) })
    }
    fn validate_source_control<'a>(
        &'a self,
        _: &'a Path,
        url: &'a str,
        _: Option<&'a str>,
    ) -> AssetFuture<'a, Option<SourceControlMedia>> {
        Box::pin(async move {
            Ok(url
                .starts_with("https://allowed.invalid/")
                .then(|| SourceControlMedia {
                    fetch_url: url.split('#').next().expect("url").into(),
                    file_name: "file.png".into(),
                }))
        })
    }
}
fn service(root: &Path) -> AssetAccess {
    AssetAccess {
        signer: AssetSigner::new(vec![9; 32]).expect("signer"),
        attachments_dir: root.join("attachments"),
        home: root.join("home"),
        internal: InternalPaths {
            state_dir: root.join("state"),
            secrets_dir: root.join("secrets"),
        },
        favicons: Arc::new(FaviconResolver::new(root.into())),
        external: Arc::new(External),
    }
}
fn put(root: &Path, path: &str, bytes: impl AsRef<[u8]>) {
    let p = root.join(path);
    fs::create_dir_all(p.parent().expect("test fixture")).expect("test fixture");
    fs::write(p, bytes).expect("test fixture");
}
fn url_parts(url: &str) -> (&str, &str) {
    let rest = url.strip_prefix("/api/assets/").expect("test fixture");
    rest.split_once('/').expect("test fixture")
}
fn workspace(path: &str) -> AssetResource {
    AssetResource::WorkspaceFile {
        thread_id: ThreadId("thread".into()),
        path: Text(path.into()),
    }
}
fn context(root: &Path) -> AssetContext<'_> {
    AssetContext {
        workspace_root: Some(root),
        ..Default::default()
    }
}

#[test]
fn signing_verifies_payloads_and_rejects_bad_tokens() {
    let signer = AssetSigner::new(vec![1; 32]).expect("signer");
    let payload = "UTF-8 é🌎";
    let signature = signer.sign(payload).expect("test fixture");
    assert!(signer.verify(payload, &signature));
    assert!(!signer.verify("other", &signature));
    assert!(!signer.verify(payload, "!!!"));
    let token = signer
        .sign_json(&serde_json::json!({"a":1}))
        .expect("test fixture");
    assert_eq!(
        signer
            .verify_json::<serde_json::Value>(&token)
            .expect("test fixture"),
        serde_json::json!({"a":1})
    );
    assert!(
        signer
            .verify_json::<serde_json::Value>(&format!("{token}.extra"))
            .is_none()
    );
}

#[tokio::test]
async fn workspace_html_grants_dependencies_and_rejects_hidden_traversal_and_symlinks() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let service = service(root);
    put(root, "public/index.html", "<html/>");
    put(root, "public/app.js", "js");
    put(root, "public/.secret.js", "secret");
    put(root, "public/source.rs", "code");
    put(root, "private.js", "private");
    let url = service
        .issue_url(workspace("public/index.html"), context(root), 100)
        .await
        .expect("test fixture");
    let (token, name) = url_parts(&url.relative_url.0);
    assert!(matches!(
        service
            .resolve(token, name, 101)
            .await
            .expect("test fixture"),
        Some(ResolvedAsset::File { .. })
    ));
    assert!(matches!(
        service
            .resolve(token, "app.js", 101)
            .await
            .expect("test fixture"),
        Some(ResolvedAsset::File { .. })
    ));
    for forbidden in [
        "../private.js",
        "%2e%2e/private.js",
        ".secret.js",
        "source.rs",
        "%00.js",
        "/private.js",
        "..\\private.js",
    ] {
        assert!(
            service
                .resolve(token, forbidden, 101)
                .await
                .expect("test fixture")
                .is_none(),
            "{forbidden}"
        );
    }
    let outside = tempfile::tempdir().expect("test fixture");
    put(outside.path(), "outside.js", "outside");
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(
        outside.path().join("outside.js"),
        root.join("public/link.js"),
    )
    .expect("test fixture");
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        outside.path().join("outside.js"),
        root.join("public/link.js"),
    )
    .expect("test fixture");
    assert!(
        service
            .resolve(token, "link.js", 101)
            .await
            .expect("test fixture")
            .is_none()
    );
    assert!(
        service
            .resolve(token, name, 100 + 60 * 60_000)
            .await
            .expect("test fixture")
            .is_none()
    );
}

#[tokio::test]
async fn every_non_html_workspace_preview_is_an_exact_grant() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let service = service(root);
    for ext in [
        "pdf", "odt", "ods", "odp", "rtf", "epub", "ipynb", "zip", "jar", "tar", "gz", "tgz",
        "sqlite", "sqlite3", "db", "docx", "xlsx", "pptx", "bmp", "heic", "tif", "tiff", "png",
        "ttf", "woff2",
    ] {
        let path = format!("preview.{ext}");
        put(root, &path, "data");
        let issued = service
            .issue_url(workspace(&path), context(root), 100)
            .await
            .expect("test fixture");
        let (tok, name) = url_parts(&issued.relative_url.0);
        assert!(
            service
                .resolve(tok, name, 101)
                .await
                .expect("test fixture")
                .is_some(),
            "{ext}"
        );
        assert!(
            service
                .resolve(tok, "private.js", 101)
                .await
                .expect("test fixture")
                .is_none(),
            "{ext}"
        );
    }
    for path in ["private.rs", "clip.mp4", "sound.mp3"] {
        put(root, path, "bytes");
        assert!(
            service
                .issue_url(workspace(path), context(root), 100)
                .await
                .is_err(),
            "{path}"
        );
    }
    let granted_root = root.join("granted");
    let outside = root.join("outside");
    put(&granted_root, "preview.png", "inside");
    put(&outside, "preview.png", "outside");
    let issued = service
        .issue_url(workspace("preview.png"), context(&granted_root), 100)
        .await
        .expect("grant before root swap");
    fs::rename(&granted_root, root.join("old-granted")).expect("move granted root");
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&outside, &granted_root).expect("replace root");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &granted_root).expect("replace root");
    let (token, name) = url_parts(&issued.relative_url.0);
    assert!(
        service
            .resolve(token, name, 101)
            .await
            .expect("resolve swapped root")
            .is_none(),
        "a signed root must not expand into an outside directory after issuance"
    );
}

#[tokio::test]
async fn exact_media_identity_rejects_replacement_and_keeps_descriptor_bound() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let service = service(root);
    put(root, "video.mp4", "original");
    let resource = || AssetResource::MediaFile {
        thread_id: ThreadId("thread".into()),
        path: Text(root.join("video.mp4").to_string_lossy().into()),
        disposition: None,
        host_only: None,
    };
    let issued = service
        .issue_url(resource(), context(root), 100)
        .await
        .expect("test fixture");
    let (tok, name) = url_parts(&issued.relative_url.0);
    let Some(ResolvedAsset::File {
        file: mut opened, ..
    }) = service.resolve(tok, name, 101).await.expect("test fixture")
    else {
        panic!("media descriptor missing")
    };
    fs::rename(root.join("video.mp4"), root.join("old.mp4")).expect("test fixture");
    put(root, "video.mp4", "replacement");
    assert_eq!(opened.read_range(0, 8).expect("test fixture"), b"original");
    assert!(
        service
            .resolve(tok, name, 102)
            .await
            .expect("test fixture")
            .is_none()
    );
    let fresh = service
        .issue_url(resource(), context(root), 103)
        .await
        .expect("test fixture");
    assert_ne!(issued.file_revision, fresh.file_revision);
    let (tok, name) = url_parts(&fresh.relative_url.0);
    fs::write(root.join("video.mp4"), "edit").expect("test fixture");
    assert!(
        service
            .resolve(tok, name, 104)
            .await
            .expect("test fixture")
            .is_some()
    );
}

#[tokio::test]
async fn absolute_media_downloads_internal_paths_network_and_draft_types() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let service = service(root);
    put(root, "report.csv", "report");
    put(root, "state/statev2.sqlite", "secret");
    put(root, "secrets/key", "key");
    put(root, "sound.mp3", "audio");
    let download = AssetResource::MediaFile {
        thread_id: ThreadId("thread".into()),
        path: Text(root.join("report.csv").to_string_lossy().into()),
        disposition: Some(DownloadDisposition::Attachment),
        host_only: None,
    };
    let issued = service
        .issue_url(download, context(root), 100)
        .await
        .expect("test fixture");
    let (tok, name) = url_parts(&issued.relative_url.0);
    assert!(matches!(
        service.resolve(tok, name, 101).await.expect("test fixture"),
        Some(ResolvedAsset::File { download: true, .. })
    ));
    for path in [
        root.join("state/statev2.sqlite"),
        root.join("secrets/key"),
        PathBuf::from("//unreachable.invalid/share/file.mp4"),
    ] {
        let r = AssetResource::MediaFile {
            thread_id: ThreadId("t".into()),
            path: Text(path.to_string_lossy().into()),
            disposition: Some(DownloadDisposition::Attachment),
            host_only: None,
        };
        assert!(service.issue_url(r, context(root), 100).await.is_err());
    }
    let draft = AssetResource::DraftWorkspaceFile {
        cwd: Text(root.to_string_lossy().into()),
        path: Text("sound.mp3".into()),
    };
    assert!(service.issue_url(draft, context(root), 100).await.is_ok());
    let draft = AssetResource::DraftWorkspaceFile {
        cwd: Text(root.to_string_lossy().into()),
        path: Text("../outside.mp4".into()),
    };
    assert!(service.issue_url(draft, context(root), 100).await.is_err());
}

#[tokio::test]
async fn generic_attachment_download_and_inline_types_use_server_extension() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let service = service(root);
    fs::create_dir(&service.attachments_dir).expect("test fixture");
    for (ext, mime, inline, download, expected_mime) in [
        ("svg", "text/plain", true, false, "image/svg+xml"),
        ("txt", "text/html", true, true, "text/html"),
        ("pdf", "text/html", true, false, "application/pdf"),
        ("html", "text/html", true, false, "text/html"),
    ] {
        let id = attachments::create_id("thread", Some(ext)).expect("test fixture");
        fs::write(service.attachments_dir.join(format!("{id}.{ext}")), "data")
            .expect("test fixture");
        let resource = AssetResource::Attachment {
            attachment_id: Text(id),
            file_name: Some(Text("file".into())),
            mime_type: Some(Text(mime.into())),
            disposition: inline.then_some(AssetDisposition::Inline),
            render_intent: Some(AssetRenderIntent::McpApp),
        };
        let issued = service
            .issue_url(resource, AssetContext::default(), 100)
            .await
            .expect("test fixture");
        let (tok, name) = url_parts(&issued.relative_url.0);
        let Some(ResolvedAsset::File {
            download: d,
            mime_type,
            render_intent,
            ..
        }) = service.resolve(tok, name, 101).await.expect("test fixture")
        else {
            panic!("missing")
        };
        assert_eq!(d, download);
        assert_eq!(mime_type.as_deref(), Some(expected_mime));
        assert_eq!(render_intent.is_some(), ext == "html");
    }
}

#[tokio::test]
async fn favicon_cache_bucket_ignores_untrusted_resource_path_and_rechecks_canonical_file() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let service = service(root);
    put(root, "favicon.svg", "svg");
    let resource = || AssetResource::ProjectFavicon {
        cwd: Text(root.to_string_lossy().into()),
        path: Some(Some(Text("untrusted.svg".into()))),
    };
    let issued = service
        .issue_url(resource(), context(root), 100)
        .await
        .expect("test fixture");
    assert_eq!(issued.source_path, Some(Some(Text("favicon.svg".into()))));
    let same = service
        .issue_url(resource(), context(root), 101)
        .await
        .expect("test fixture");
    assert_eq!(issued.relative_url, same.relative_url);
    let (tok, name) = url_parts(&issued.relative_url.0);
    assert!(
        service
            .resolve(tok, name, 102)
            .await
            .expect("test fixture")
            .is_some()
    );
    fs::remove_file(root.join("favicon.svg")).expect("test fixture");
    assert!(
        service
            .resolve(tok, name, 102)
            .await
            .expect("test fixture")
            .is_none()
    );
    let pending = service
        .issue_url(
            resource(),
            AssetContext {
                workspace_root: Some(root),
                checkout: Checkout::Pending,
                ..Default::default()
            },
            100,
        )
        .await
        .expect("test fixture");
    assert!(pending.relative_url.0.ends_with("project-favicon-missing"));
}

#[tokio::test]
async fn mcp_html_limits_and_icns_png_selection() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let limit = 5 * 1024 * 1024 + 128 * 1024;
    put(root, "app.html", vec![b'a'; limit]);
    assert_eq!(
        read_mcp_app_html(root.join("app.html"))
            .await
            .expect("test fixture")
            .expect("test fixture")
            .len(),
        limit
    );
    put(root, "app.html", vec![b'a'; limit + 1]);
    assert!(
        read_mcp_app_html(root.join("app.html"))
            .await
            .expect("test fixture")
            .is_none()
    );
    put(root, "app.html", "");
    assert_eq!(
        read_mcp_app_html(root.join("app.html"))
            .await
            .expect("test fixture")
            .expect("test fixture"),
        Vec::<u8>::new()
    );
    assert!(read_mcp_app_html(root.join("absent")).await.is_err());
    let png = |width: u32| {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes
    };
    let mut body = Vec::new();
    for width in [32, 256, 128, 64] {
        let p = png(width);
        body.extend_from_slice(b"ic07");
        body.extend_from_slice(&((p.len() + 8) as u32).to_be_bytes());
        body.extend_from_slice(&p);
    }
    let mut icns = b"icns".to_vec();
    icns.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    icns.extend_from_slice(&body);
    assert_eq!(extract_icns_png(&icns, 96), Some(png(128)));
    assert_eq!(extract_icns_png(&icns, 512), Some(png(256)));
    assert!(extract_icns_png(b"bad", 96).is_none());
}

#[test]
fn existing_typescript_asset_payloads_decode_without_reformatting() {
    let signer = AssetSigner::new(vec![9; 32]).expect("signer");
    let payload = r#"{"version":1,"kind":"attachment","attachmentId":"pending-00000000-0000-4000-8000-000000000001","expiresAt":1000}"#;
    let encoded = URL_SAFE_NO_PAD.encode(payload);
    let token = format!("{encoded}.{}", signer.sign(&encoded).expect("test fixture"));
    assert_eq!(
        signer
            .verify_json::<serde_json::Value>(&token)
            .expect("test fixture")["kind"],
        "attachment"
    );
}

#[test]
fn signing_rejects_empty_and_short_keys() {
    for length in [0, 1, 31] {
        assert!(
            AssetSigner::new(vec![0; length]).is_err(),
            "weak signing key accepted"
        );
    }
}
#[tokio::test]
async fn html_grants_check_canonical_dot_segments_and_extensions() {
    let tmp = tempfile::tempdir().expect("temp");
    let root = tmp.path();
    let service = service(root);
    put(root, "public/index.html", "html");
    put(root, "public/.hidden/secret.js", "secret");
    put(root, "public/.secret.js", "secret");
    put(root, "public/source.rs", "private");
    #[cfg(windows)]
    let link = std::os::windows::fs::symlink_file;
    #[cfg(unix)]
    let link = std::os::unix::fs::symlink;
    link(
        root.join("public/.hidden/secret.js"),
        root.join("public/hidden.js"),
    )
    .expect("link");
    link(root.join("public/source.rs"), root.join("public/source.js")).expect("link");
    let url = service
        .issue_url(workspace("public/index.html"), context(root), 100)
        .await
        .expect("url");
    let (token, _) = url_parts(&url.relative_url.0);
    for name in ["hidden.js", "source.js"] {
        assert!(
            service
                .resolve(token, name, 101)
                .await
                .expect("resolve")
                .is_none(),
            "{name}"
        );
    }
    #[cfg(windows)]
    {
        let mut opts=t3_process::ProcessOptions::new("powershell.exe").args(["-NoProfile","-NonInteractive","-Command","$f=New-Object -ComObject Scripting.FileSystemObject; $f.GetFile($env:T3_SHORT_PATH).ShortName"]);
        opts.env.insert(
            "T3_SHORT_PATH".into(),
            Some(root.join("public/.secret.js").into_os_string()),
        );
        let output = t3_process::run(opts).await.expect("short alias");
        let alias = output.stdout_text().trim().to_owned();
        assert!(
            alias.contains('~') && root.join("public").join(&alias).exists(),
            "real short alias {alias}"
        );
        assert!(
            service
                .resolve(token, &alias, 101)
                .await
                .expect("resolve")
                .is_none(),
            "{alias}"
        );
    }
}
#[cfg(windows)]
#[tokio::test]
async fn absolute_workspace_assets_accept_case_differences() {
    let tmp = tempfile::tempdir().expect("temp");
    let root = tmp.path();
    put(root, "Preview.PNG", b"png");
    let service = service(root);
    assert!(
        service
            .issue_url(
                workspace(&root.join("Preview.PNG").to_string_lossy().to_uppercase()),
                context(root),
                100
            )
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn source_media_signs_the_narrowed_fetch_url_and_name() {
    let tmp = tempfile::tempdir().expect("temp");
    let root = tmp.path();
    let service = service(root);
    let issued = service
        .issue_url(
            AssetResource::SourceControlMedia {
                cwd: Text(root.to_string_lossy().into()),
                url: Text("https://allowed.invalid/file.png#fragment".into()),
                repository: None,
            },
            context(root),
            100,
        )
        .await
        .expect("url");
    let (token, name) = url_parts(&issued.relative_url.0);
    let claims: serde_json::Value = service.signer.verify_json(token).expect("claims");
    assert_eq!(claims["url"], "https://allowed.invalid/file.png");
    assert_eq!(name, "file.png");
}
#[tokio::test]
async fn attachment_dimensions_use_the_canonical_attachment_path() {
    let tmp = tempfile::tempdir().expect("temp");
    let root = tmp.path();
    let mut service = service(root);
    fs::create_dir(root.join("real")).expect("directory");
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(root.join("real"), root.join("alias")).expect("symlink");
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("real"), root.join("alias")).expect("symlink");
    service.attachments_dir = root.join("alias");
    let id = attachments::pending_id(None);
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend_from_slice(&32u32.to_be_bytes());
    png.extend_from_slice(&16u32.to_be_bytes());
    put(root, &format!("real/{id}.png"), png);
    let issued = service
        .issue_url(
            AssetResource::Attachment {
                attachment_id: Text(id),
                file_name: None,
                mime_type: None,
                disposition: None,
                render_intent: None,
            },
            context(root),
            100,
        )
        .await
        .expect("url");
    let dimensions = issued
        .image_dimensions
        .flatten()
        .expect("canonical image dimensions");
    assert_eq!((dimensions.width, dimensions.height), (32, 16));
}

#[tokio::test]
async fn a_real_node_media_token_uses_the_same_device_and_inode() {
    let tmp = tempfile::tempdir().expect("temp");
    let root = tmp.path();
    put(root, "video.mp4", "node identity");
    let service = service(root);
    let script = r#"const fs=require('node:fs'),crypto=require('node:crypto'); const p=fs.realpathSync(process.env.T3_MEDIA_PATH),s=fs.statSync(p,{bigint:true}); const claims={version:1,kind:'media-file-exact',filePath:p,device:s.dev.toString(),inode:s.ino.toString(),expiresAt:1000}; const payload=Buffer.from(JSON.stringify(claims)).toString('base64url'); process.stdout.write(JSON.stringify({token:payload+'.'+crypto.createHmac('sha256',Buffer.alloc(32,9)).update(payload).digest('base64url'),name:require('node:path').basename(p)}));"#;
    let mut options = t3_process::ProcessOptions::new("node").args(["-e", script]);
    options.env.insert(
        "T3_MEDIA_PATH".into(),
        Some(root.join("video.mp4").into_os_string()),
    );
    let output = t3_process::run(options)
        .await
        .expect("Node identity fixture");
    let generated: serde_json::Value = serde_json::from_slice(&output.stdout).expect("Node token");
    let rust = service
        .issue_url(
            AssetResource::MediaFile {
                thread_id: ThreadId("t".into()),
                path: Text(root.join("video.mp4").to_string_lossy().into()),
                disposition: None,
                host_only: None,
            },
            context(root),
            100,
        )
        .await
        .expect("Rust token");
    let (rt, _) = url_parts(&rust.relative_url.0);
    let rust_claims: serde_json::Value = service.signer.verify_json(rt).expect("Rust claims");
    let node_claims: serde_json::Value = service
        .signer
        .verify_json(generated["token"].as_str().expect("token"))
        .expect("Node claims");
    assert_eq!(rust_claims["device"], node_claims["device"]);
    assert_eq!(rust_claims["inode"], node_claims["inode"]);
    let asset = service
        .resolve(
            generated["token"].as_str().expect("token"),
            generated["name"].as_str().expect("name"),
            101,
        )
        .await
        .expect("resolve");
    let Some(ResolvedAsset::File { mut file, .. }) = asset else {
        panic!("real TypeScript-compatible Node identity rejected")
    };
    assert_eq!(file.read_range(0, 64).expect("read"), b"node identity");
}

#[tokio::test]
async fn tool_images_have_dimensions_and_native_icons_have_the_contract_name() {
    struct RichExternal {
        icon: PathBuf,
        image: Vec<u8>,
    }
    impl ExternalAssets for RichExternal {
        fn tool_image<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
            _: u64,
        ) -> AssetFuture<'a, Option<(String, Vec<u8>)>> {
            Box::pin(async { Ok(Some(("image/png".into(), self.image.clone()))) })
        }
        fn native_icon<'a>(
            &'a self,
            _: &'a NativeAppIconReference,
        ) -> AssetFuture<'a, Option<PathBuf>> {
            Box::pin(async { Ok(Some(self.icon.clone())) })
        }
        fn validate_source_control<'a>(
            &'a self,
            _: &'a Path,
            _: &'a str,
            _: Option<&'a str>,
        ) -> AssetFuture<'a, Option<SourceControlMedia>> {
            Box::pin(async { Ok(None) })
        }
    }
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let mut service = service(root);
    put(root, "actual-cache-name.png", "png");
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend_from_slice(&32u32.to_be_bytes());
    png.extend_from_slice(&16u32.to_be_bytes());
    service.external = Arc::new(RichExternal {
        icon: root.join("actual-cache-name.png"),
        image: png,
    });
    let image = service
        .issue_url(
            AssetResource::ToolOutputImage {
                thread_id: ThreadId("t".into()),
                item_id: TurnItemId("item".into()),
                index: 0,
            },
            context(root),
            100,
        )
        .await
        .expect("image");
    assert!(image.relative_url.0.ends_with("/image-1.png"));
    let dimensions = image.image_dimensions.flatten().expect("dimensions");
    assert_eq!((dimensions.width, dimensions.height), (32, 16));
    let app = serde_json::from_value(serde_json::json!({"_tag":"app-path","path":"/Example.app"}))
        .expect("app");
    let native = service
        .issue_url(AssetResource::NativeAppIcon { app }, context(root), 100)
        .await
        .expect("icon");
    assert!(native.relative_url.0.ends_with("/native-app-icon.png"));
    let (token, name) = url_parts(&native.relative_url.0);
    let Some(ResolvedAsset::File { mut file, .. }) =
        service.resolve(token, name, 101).await.expect("resolve")
    else {
        panic!("native icon lacks a descriptor")
    };
    assert_eq!(file.read_range(0, 3).expect("read"), b"png");
}

/// A stored attachment is the file its entry names. A link planted in the attachments directory,
/// to a file outside it or to another attachment, is neither signed nor served, including through
/// a grant signed before the swap. A client's type never decides how an image renders.
#[tokio::test]
async fn attachment_links_are_never_followed_and_client_types_never_render() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let service = service(root);
    fs::create_dir(&service.attachments_dir).expect("test fixture");
    let link = |target: &Path, link: &Path| {
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(target, link).expect("symlink");
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).expect("symlink");
    };
    let grant = |id: &str, mime: Option<&str>| AssetResource::Attachment {
        attachment_id: Text(id.into()),
        file_name: None,
        mime_type: mime.map(|m| Text(m.into())),
        disposition: Some(AssetDisposition::Inline),
        render_intent: None,
    };
    put(root, "outside/secret.png", b"secret");
    let real = attachments::create_id("thread", None).expect("id");
    put(root, &format!("attachments/{real}.png"), b"real");
    for target in [
        root.join("outside/secret.png"),
        service.attachments_dir.join(format!("{real}.png")),
    ] {
        let planted = attachments::create_id("thread", None).expect("id");
        link(
            &target,
            &service.attachments_dir.join(format!("{planted}.png")),
        );
        assert!(
            service
                .issue_url(grant(&planted, None), AssetContext::default(), 100)
                .await
                .is_err(),
            "{} was signed through a link",
            target.display()
        );
    }
    assert!(
        service
            .issue_url(
                grant(&format!("staging/../{real}"), None),
                AssetContext::default(),
                100
            )
            .await
            .is_err(),
        "signed a path alias instead of an attachment ID"
    );
    // Signed while it was a file, swapped for a link afterwards: nothing is served.
    let swapped = attachments::create_id("thread", None).expect("id");
    let swapped_path = service.attachments_dir.join(format!("{swapped}.png"));
    fs::write(&swapped_path, b"before").expect("test fixture");
    let issued = service
        .issue_url(grant(&swapped, None), AssetContext::default(), 100)
        .await
        .expect("url");
    fs::remove_file(&swapped_path).expect("test fixture");
    link(&root.join("outside/secret.png"), &swapped_path);
    let (token, name) = url_parts(&issued.relative_url.0);
    assert!(
        service
            .resolve(token, name, 101)
            .await
            .expect("resolve")
            .is_none()
    );
    // An image asked for as HTML is served by its stored extension, not as HTML.
    let issued = service
        .issue_url(
            grant(&real, Some("text/html")),
            AssetContext::default(),
            100,
        )
        .await
        .expect("url");
    let (token, name) = url_parts(&issued.relative_url.0);
    let Some(ResolvedAsset::File {
        mime_type, path, ..
    }) = service.resolve(token, name, 101).await.expect("resolve")
    else {
        panic!("missing")
    };
    assert_eq!(mime_type, None);
    assert!(path.ends_with(format!("{real}.png")));
}

#[tokio::test]
async fn an_attachment_preview_grants_the_original_file() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    let service = service(root);
    let id = attachments::create_id("thread", None).expect("id");
    let relative = format!("attachments/{id}.png");
    put(root, &relative, b"first");
    let resource = AssetResource::Attachment {
        attachment_id: Text(id),
        file_name: None,
        mime_type: None,
        disposition: None,
        render_intent: None,
    };
    let grant = service
        .issue_url(resource, AssetContext::default(), 100)
        .await
        .expect("grant");
    let (token, name) = url_parts(&grant.relative_url.0);
    let mut old_claims: serde_json::Value = service.signer.verify_json(token).expect("claims");
    old_claims
        .as_object_mut()
        .expect("object")
        .remove("identity");
    let legacy = service.signer.sign_json(&old_claims).expect("legacy token");
    assert!(
        service
            .resolve(&legacy, name, 101)
            .await
            .expect("legacy grant")
            .is_some()
    );
    fs::rename(root.join(&relative), root.join("original.png")).expect("move original");
    put(root, &relative, b"second");
    assert!(
        service
            .resolve(token, name, 101)
            .await
            .expect("resolve")
            .is_none(),
        "preview accepted a replacement file"
    );
}
