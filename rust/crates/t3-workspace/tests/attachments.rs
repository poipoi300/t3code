use std::{
    fs,
    future::Future,
    path::Path,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, UNIX_EPOCH},
};
use t3_contracts::assets::AttachmentCreateUploadUrlInput;
use t3_workspace::{Error, assets::AssetSigner, attachments::*};

struct Documents(AtomicUsize);
impl DocumentText for Documents {
    fn ensure<'a>(&'a self, path: &'a Path) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            tokio::fs::write(format!("{}.txt", path.display()), "extracted")
                .await
                .expect("test fixture");
        })
    }
}
fn input(file: bool, name: &str, mime: &str, size: u64) -> AttachmentCreateUploadUrlInput {
    serde_json::from_value(serde_json::json!({"type":if file {"file"}else{"image"},"name":name,"mimeType":mime,"sizeBytes":size})).expect("test fixture")
}
fn token(url: &str) -> &str {
    url.rsplit('/').next().expect("test fixture")
}

#[test]
fn attachment_ids_layout_stability_and_segment_collisions() {
    assert_eq!(
        safe_thread_segment("  A..B---__x__ "),
        Some("a-b-__x".into())
    );
    assert_eq!(safe_thread_segment("pending"), Some("_pending".into()));
    assert!(safe_thread_segment("!@#$").is_none());
    assert_eq!(
        safe_thread_segment(&"a".repeat(200))
            .expect("test fixture")
            .len(),
        80
    );
    let id = deterministic_id("thread-1", "message-1:0").expect("test fixture");
    assert_eq!(
        deterministic_id("thread-1", "message-1:0").expect("test fixture"),
        id
    );
    assert_eq!(
        parse_id(&id).expect("test fixture").thread_segment,
        "thread-1"
    );
    assert_ne!(
        deterministic_id("thread.a", "key"),
        deterministic_id("thread-a", "key")
    );
    assert_ne!(stable_key("m", 0, b"a"), stable_key("m", 0, b"b"));
    for thread in ["foo", "foo-bar", "Pending", "x_y"] {
        let id = create_id(thread, Some(".PDF")).expect("test fixture");
        let parsed = parse_id(&id).expect("test fixture");
        assert_eq!(
            parsed.thread_segment,
            safe_thread_segment(thread).expect("test fixture")
        );
        assert_eq!(parsed.extension, Some("pdf".into()));
    }
    for invalid in [
        "../../id",
        "a/id",
        "id.png",
        "bad",
        "pending-00000000-0000-0000-0000-00000000000g",
    ] {
        assert!(parse_id(invalid).is_none(), "{invalid}");
    }
    assert_eq!(
        parse_id(&pending_id(None))
            .expect("test fixture")
            .thread_segment,
        "pending"
    );
    for (name, expected) in [
        ("x.PDF", ".pdf"),
        ("x.part", ".bin"),
        ("x.abc12345678", ".bin"),
        ("x", ".bin"),
        ("x.a-b", ".bin"),
    ] {
        assert_eq!(file_extension(name), expected);
    }
}

#[test]
fn paths_claims_and_legacy_image_lookup() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let id = pending_id(Some("txt"));
    let path = root.join(format!("{id}.txt"));
    fs::write(&path, "body").expect("test fixture");
    assert_eq!(resolve_by_id(root, &id), Some(path.clone()));
    assert!(resolve_by_id(root, &format!("discarded/../{id}")).is_none());
    let plan = plan_claim(root, "thread-a", &id).expect("test fixture");
    assert_eq!(plan.current_path, path);
    assert_eq!(
        parse_id(&plan.final_id)
            .expect("test fixture")
            .thread_segment,
        "thread-a"
    );
    assert!(plan.final_path.ends_with(format!("{}.txt", plan.final_id)));
    assert!(
        plan_claim(
            root,
            "thread-a",
            &create_id("thread-a", None).expect("test fixture")
        )
        .is_err()
    );
    assert!(plan_claim(root, "thread-a", &pending_id(None)).is_err());
    assert!(plan_claim(root, "!@#", &id).is_err());
    let legacy = pending_id(None);
    fs::write(root.join(format!("{legacy}.png")), b"image").expect("test fixture");
    assert!(
        resolve_by_id(root, &legacy)
            .expect("test fixture")
            .ends_with(format!("{legacy}.png"))
    );
    for escape in ["../secret", "..\\secret", "a/../../secret", "\0", ""] {
        assert!(resolve_relative(root, escape).is_none(), "{escape}");
    }
}

#[test]
fn sweeps_pending_sidecars_and_partials_but_preserves_thread_files() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let pending = pending_id(Some("txt"));
    let owned = create_id("thread", None).expect("test fixture");
    let now = 1_000_000_000u64;
    for (name, age) in [
        (format!("{pending}.txt"), 24 * 60 * 60_000 + 1),
        (format!("{pending}.txt.owner"), 5),
        (
            format!("{}.png.owner", pending_id(None)),
            24 * 60 * 60_000 + 1,
        ),
        (format!("{pending}.txt.txt"), 24 * 60 * 60_000 + 1),
        ("upload.part".into(), 60 * 60_000 + 1),
        (format!("{owned}.png"), 24 * 60 * 60_000 + 1),
        (format!("{}.png", pending_id(None)), 100),
    ] {
        let p = root.join(name);
        fs::write(&p, b"x").expect("test fixture");
        let file = fs::OpenOptions::new()
            .write(true)
            .open(&p)
            .expect("test fixture");
        file.set_times(
            fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_millis(now - age)),
        )
        .expect("test fixture");
    }
    assert_eq!(sweep_stale(root, now), 4);
    assert!(root.join(format!("{owned}.png")).is_file());
    assert!(root.join(format!("{pending}.txt.owner")).is_file());
    assert_eq!(fs::read_dir(root).expect("test fixture").count(), 3);
}

#[tokio::test]
async fn signed_uploads_replay_immutably_and_cleanup_on_size_errors() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let docs = Arc::new(Documents(AtomicUsize::new(0)));
    let store = AttachmentStore::new(
        root.into(),
        AssetSigner::new(vec![7; 32]).expect("signer"),
        docs.clone(),
    );
    let upload = store
        .issue_upload(input(false, "image.png", "image/png", 4), 100)
        .await
        .expect("test fixture");
    let tok = token(&upload.relative_url.0);
    assert!(store.validate_upload(tok, 100).is_some());
    assert!(store.validate_upload(tok, 100 + UPLOAD_TTL_MS).is_none());
    let path = store
        .upload(tok, 101, &b"test"[..])
        .await
        .expect("test fixture");
    assert_eq!(fs::read(&path).expect("test fixture"), b"test");
    store
        .upload(tok, 101, &b"test"[..])
        .await
        .expect("test fixture");
    assert!(matches!(
        store.upload(tok, 101, &b"evil"[..]).await,
        Err(Error::Upload { status: 409, .. })
    ));
    assert_eq!(fs::read(&path).expect("test fixture"), b"test");
    for bytes in [b"x".as_slice(), b"too long".as_slice()] {
        assert!(matches!(
            store.upload(tok, 101, bytes).await,
            Err(Error::Upload { status: 400, .. })
        ));
    }
    assert!(!fs::read_dir(root).expect("test fixture").any(|e| {
        e.expect("test fixture")
            .file_name()
            .to_string_lossy()
            .ends_with(".part")
    }));
    assert_eq!(docs.0.load(Ordering::SeqCst), 2);
    let tampered = format!("{}x", tok);
    assert!(store.validate_upload(&tampered, 101).is_none());
    assert!(
        store
            .validate_upload(&format!("{tok}.extra"), 101)
            .is_none()
    );
    let owned = create_id("thread", None).expect("test fixture");
    fs::write(root.join(format!("{owned}.png")), b"image").expect("test fixture");
    store.delete_pending(&owned).await.expect("test fixture");
    assert!(root.join(format!("{owned}.png")).exists());
    store
        .delete_pending(&upload.attachment_id.0)
        .await
        .expect("test fixture");
    assert!(!path.exists());
    assert!(!Path::new(&format!("{}.txt", path.display())).exists());
}

#[tokio::test]
async fn legacy_tokens_generic_files_svg_names_and_input_limits() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let signer = AssetSigner::new(vec![8; 32]).expect("signer");
    let store = AttachmentStore::new(
        root.into(),
        signer.clone(),
        Arc::new(Documents(AtomicUsize::new(0))),
    );
    let legacy = serde_json::json!({"version":1,"kind":"attachment-upload","attachmentId":pending_id(None),"name":"x.png","mimeType":"image/png","sizeBytes":3,"expiresAt":900});
    let tok = signer.sign_json(&legacy).expect("test fixture");
    assert_eq!(
        store
            .validate_upload(&tok, 100)
            .expect("test fixture")
            .attachment_type,
        "image"
    );
    store
        .upload(&tok, 100, &b"png"[..])
        .await
        .expect("test fixture");
    for (name, mime, expected) in [
        ("Notes.TXT", "text/plain", "txt"),
        ("vector", "image/svg+xml", "svg"),
        ("upload.part", "application/octet-stream", "bin"),
    ] {
        let upload = store
            .issue_upload(input(true, name, mime, 3), 100)
            .await
            .expect("test fixture");
        let tok = token(&upload.relative_url.0);
        let claims = store.validate_upload(tok, 100).expect("test fixture");
        assert_eq!(
            parse_id(&claims.attachment_id)
                .expect("test fixture")
                .extension
                .as_deref(),
            Some(expected)
        );
        let path = store
            .upload(tok, 100, &b"abc"[..])
            .await
            .expect("test fixture");
        assert_eq!(path.extension().expect("test fixture"), expected);
    }
    assert!(
        store
            .issue_upload(input(true, "x", "text/plain", FILE_MAX_BYTES + 1), 100)
            .await
            .is_err()
    );
    assert!(
        store
            .issue_upload(input(false, "x", "image/png", IMAGE_MAX_BYTES + 1), 100)
            .await
            .is_err()
    );
    assert!(
        store
            .issue_upload(input(true, "x", "text/plain", 0), 100)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn incomplete_stream_removes_its_partial_file() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let store = AttachmentStore::new(
        root.into(),
        AssetSigner::new(vec![2; 32]).expect("signer"),
        Arc::new(Documents(AtomicUsize::new(0))),
    );
    let issued = store
        .issue_upload(input(true, "file.txt", "text/plain", 10), 100)
        .await
        .expect("test fixture");
    let (reader, mut writer) = tokio::io::duplex(16);
    let token = token(&issued.relative_url.0).to_owned();
    let owned = store.clone();
    let upload = tokio::spawn(async move { owned.upload(&token, 100, reader).await });
    use tokio::io::AsyncWriteExt;
    writer.write_all(b"short").await.expect("test fixture");
    // Closing the writer produces a deterministic incomplete stream and exercises the same part guard.
    drop(writer);
    assert!(matches!(
        upload.await.expect("test fixture"),
        Err(Error::Upload { status: 400, .. })
    ));
    assert_eq!(fs::read_dir(root).expect("test fixture").count(), 0);
}

#[tokio::test]
async fn cancelling_an_upload_removes_its_partial_file() {
    struct WaitingReader(Arc<tokio::sync::Notify>);
    impl tokio::io::AsyncRead for WaitingReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            self.0.notify_one();
            std::task::Poll::Pending
        }
    }
    let temp = tempfile::tempdir().expect("temp");
    let store = AttachmentStore::new(
        temp.path().into(),
        AssetSigner::new(vec![1; 32]).expect("signer"),
        Arc::new(Documents(AtomicUsize::new(0))),
    );
    let issued = store
        .issue_upload(input(true, "file.txt", "text/plain", 10), 100)
        .await
        .expect("issue");
    let tok = token(&issued.relative_url.0).to_owned();
    let ready = Arc::new(tokio::sync::Notify::new());
    let reader = WaitingReader(ready.clone());
    let upload = tokio::spawn(async move { store.upload(&tok, 100, reader).await });
    ready.notified().await;
    assert!(fs::read_dir(temp.path()).expect("parts").any(|e| {
        e.expect("part")
            .file_name()
            .to_string_lossy()
            .ends_with(".part")
    }));
    upload.abort();
    assert!(upload.await.expect_err("cancelled").is_cancelled());
    assert_eq!(fs::read_dir(temp.path()).expect("cleaned").count(), 0);
}

#[test]
fn data_urls_validate_headers_padding_and_whitespace() {
    assert_eq!(
        parse_base64_data_url(" DATA:image/png; charset=x; base64, Y W\r\nJj ")
            .expect("test fixture"),
        ("image/png".into(), b"abc".to_vec())
    );
    for invalid in [
        "data:image/png,AAAA",
        "data:;base64,AAAA",
        "data:image/png;base64,AA=A",
        "data:image/png;base64,A===",
        "data:image/png;base64,AA\tAA",
        "data:image/png;base64,A",
        "data:image/png;base64,",
    ] {
        assert!(parse_base64_data_url(invalid).is_none(), "{invalid}");
    }
}

#[tokio::test]
async fn legacy_data_urls_use_stable_message_and_content_ids() {
    let temp = tempfile::tempdir().expect("temp");
    let store = AttachmentStore::new(
        temp.path().into(),
        AssetSigner::new(vec![1; 32]).expect("signer"),
        Arc::new(Documents(AtomicUsize::new(0))),
    );
    let input = |data: &str| {
        serde_json::from_value(serde_json::json!({"threadId":"thread","messageId":"message","attachments":[{"type":"image","name":"picture.png","mimeType":"image/png","sizeBytes":3,"dataUrl":data}]})).expect("input")
    };
    let first = store
        .persist_chat_attachments(input("data:image/png;base64,YWJj"))
        .await
        .expect("persist");
    assert_eq!(
        first,
        store
            .persist_chat_attachments(input("data:image/png;base64,YWJj"))
            .await
            .expect("retry")
    );
    let changed = store
        .persist_chat_attachments(input("data:image/png;base64,ZGVm"))
        .await
        .expect("changed");
    assert_ne!(first, changed);
    assert!(
        store
            .persist_chat_attachments(input("data:image/jpeg;base64,YWJj"))
            .await
            .is_err()
    );
    assert!(
        store
            .persist_chat_attachments(input("data:image/png;base64,YQ=="))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn legacy_persistence_cannot_follow_a_preplanted_file_link() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("attachments");
    fs::create_dir(&root).expect("directory");
    let outside = temp.path().join("outside");
    fs::write(&outside, "outside").expect("file");
    let bytes = b"image";
    let id = deterministic_id("thread", &stable_key("message", 0, bytes)).expect("id");
    let link = root.join(format!("{id}.png"));
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&outside, &link).expect("symlink");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).expect("symlink");
    let store = AttachmentStore::new(
        root,
        AssetSigner::new(vec![1; 32]).expect("signer"),
        Arc::new(Documents(AtomicUsize::new(0))),
    );
    let input=serde_json::from_value(serde_json::json!({"threadId":"thread","messageId":"message","attachments":[{"type":"image","name":"image.png","mimeType":"image/png","sizeBytes":5,"dataUrl":"data:image/png;base64,aW1hZ2U="}]})).expect("input");
    let _ = store.persist_chat_attachments(input).await;
    assert_eq!(fs::read_to_string(outside).expect("outside"), "outside");
}
#[tokio::test]
async fn swapping_an_upload_directory_cannot_publish_outside_it() {
    struct SwappingReader {
        root: std::path::PathBuf,
        outside: std::path::PathBuf,
        sent: bool,
    }
    impl tokio::io::AsyncRead for SwappingReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if !self.sent {
                let old = self.root.with_extension("old");
                match fs::rename(&self.root, &old) {
                    Ok(()) => {
                        #[cfg(windows)]
                        std::os::windows::fs::symlink_dir(&self.outside, &self.root).expect("swap");
                        #[cfg(unix)]
                        std::os::unix::fs::symlink(&self.outside, &self.root).expect("swap");
                    }
                    Err(error) => assert!(
                        error.kind() == std::io::ErrorKind::PermissionDenied
                            || matches!(error.raw_os_error(), Some(5 | 32)),
                        "unexpected swap error: {error}"
                    ),
                }
                self.sent = true;
                buf.put_slice(b"body");
            }
            std::task::Poll::Ready(Ok(()))
        }
    }
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("attachments");
    let outside = temp.path().join("outside");
    fs::create_dir(&root).expect("root");
    fs::create_dir(&outside).expect("outside");
    let store = AttachmentStore::new(
        root.clone(),
        AssetSigner::new(vec![1; 32]).expect("signer"),
        Arc::new(Documents(AtomicUsize::new(0))),
    );
    let issued = store
        .issue_upload(input(true, "file.txt", "text/plain", 4), 100)
        .await
        .expect("issue");
    let _ = store
        .upload(
            token(&issued.relative_url.0),
            101,
            SwappingReader {
                root,
                outside: outside.clone(),
                sent: false,
            },
        )
        .await;
    assert_eq!(
        fs::read_dir(&outside).expect("outside").count(),
        0,
        "upload touched a replacement directory outside its pinned root"
    );
}

#[tokio::test]
async fn replay_cannot_recreate_missing_claim_authority() {
    let temp = tempfile::tempdir().expect("temp");
    let store = AttachmentStore::new(
        temp.path().into(),
        AssetSigner::new(vec![7; 32]).expect("key"),
        Arc::new(Documents(AtomicUsize::new(0))),
    );
    let issued = store
        .issue_owned_upload(
            input(false, "a.png", "image/png", 1),
            100,
            Some("session:a".into()),
        )
        .await
        .expect("issue");
    let url = token(&issued.relative_url.0);
    let path = store.upload(url, 101, &b"x"[..]).await.expect("upload");
    store
        .set_claimed(&issued.attachment_id.0, true)
        .await
        .expect("claim");
    store
        .upload(url, 102, &b"x"[..])
        .await
        .expect("identical retry");
    assert!(
        store
            .owner(&issued.attachment_id.0)
            .await
            .expect("owner")
            .claimed
    );
    fs::remove_file(path.with_extension("png.owner")).expect("remove record");
    let _ = store.upload(url, 103, &b"x"[..]).await;
    assert!(
        store.owner(&issued.attachment_id.0).await.is_none(),
        "a retry granted fresh claim authority"
    );
}

#[tokio::test]
async fn owner_authority_is_signed_and_bound_to_its_payload() {
    let temp = tempfile::tempdir().expect("temp");
    let store = AttachmentStore::new(
        temp.path().into(),
        AssetSigner::new(vec![7; 32]).expect("key"),
        Arc::new(Documents(AtomicUsize::new(0))),
    );
    let issue = || {
        store.issue_owned_upload(
            input(false, "a.png", "image/png", 1),
            100,
            Some("session:a".into()),
        )
    };
    let a = issue().await.expect("issue a");
    let b = issue().await.expect("issue b");
    let a_path = store
        .upload(token(&a.relative_url.0), 101, &b"a"[..])
        .await
        .expect("upload a");
    let b_path = store
        .upload(token(&b.relative_url.0), 101, &b"b"[..])
        .await
        .expect("upload b");
    let owner = a_path.with_extension("png.owner");
    let signed = fs::read(&owner).expect("signed record");
    fs::copy(b_path.with_extension("png.owner"), &owner).expect("copy fixture");
    assert!(
        store.owner(&a.attachment_id.0).await.is_none(),
        "record from a different id"
    );
    fs::write(&owner, br#"{"owner":"session:a","claimed":false}"#).expect("unsigned record");
    assert!(
        store.owner(&a.attachment_id.0).await.is_none(),
        "unsigned record"
    );
    fs::write(&owner, signed).expect("restore");
    fs::rename(&a_path, a_path.with_extension("saved")).expect("move original");
    fs::write(&a_path, b"c").expect("new payload");
    assert!(
        store.set_claimed(&a.attachment_id.0, true).await.is_err(),
        "replacement payload"
    );
}

#[tokio::test]
async fn deleting_before_upload_revokes_the_signed_grant_across_restart() {
    let temp = tempfile::tempdir().unwrap();
    let restart = || {
        AttachmentStore::new(
            temp.path().into(),
            AssetSigner::new(vec![7; 32]).unwrap(),
            Arc::new(Documents(AtomicUsize::new(0))),
        )
    };
    let store = restart();
    let grant = store
        .issue_owned_upload(input(false, "a.png", "image/png", 1), 100, Some("a".into()))
        .await
        .unwrap();
    assert!(
        !store
            .discard_owned(&grant.attachment_id.0, "b")
            .await
            .unwrap()
    );
    assert!(
        store
            .discard_owned(&grant.attachment_id.0, "a")
            .await
            .unwrap()
    );
    assert!(
        restart()
            .upload(token(&grant.relative_url.0), 102, &b"x"[..])
            .await
            .is_err()
    );
}

#[tokio::test]
async fn owned_grants_reserve_bytes_per_issuer_across_restart() {
    let temp = tempfile::tempdir().unwrap();
    let restart = || {
        AttachmentStore::new(
            temp.path().into(),
            AssetSigner::new(vec![7; 32]).unwrap(),
            Arc::new(Documents(AtomicUsize::new(0))),
        )
    };
    let store = restart();
    for _ in 0..4 {
        store
            .issue_owned_upload(
                input(true, "a.bin", "application/octet-stream", FILE_MAX_BYTES),
                100,
                Some("a".into()),
            )
            .await
            .unwrap();
    }
    let store = restart();
    assert!(
        store
            .issue_owned_upload(
                input(true, "a.bin", "application/octet-stream", 1),
                101,
                Some("a".into())
            )
            .await
            .is_err()
    );
    assert!(
        store
            .issue_owned_upload(
                input(true, "a.bin", "application/octet-stream", 1),
                101,
                Some("b".into())
            )
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn unspent_count_survives_restart_and_expired_grants_release_it() {
    let root = tempfile::tempdir().unwrap();
    let restart = || {
        AttachmentStore::new(
            root.path().into(),
            AssetSigner::new(vec![7; 32]).unwrap(),
            Arc::new(Documents(AtomicUsize::new(0))),
        )
    };
    let store = restart();
    for _ in 0..t3_contracts::chat_attachment::PROVIDER_SEND_TURN_MAX_ATTACHMENTS {
        store
            .issue_owned_upload(
                input(false, "pixel.png", "image/png", 1),
                100,
                Some("a".into()),
            )
            .await
            .unwrap();
    }
    let store = restart();
    assert!(
        store
            .issue_owned_upload(
                input(false, "more.png", "image/png", 1),
                101,
                Some("a".into())
            )
            .await
            .is_err()
    );
    assert!(
        store
            .issue_owned_upload(
                input(false, "fresh.png", "image/png", 1),
                101 + UPLOAD_TTL_MS,
                Some("a".into())
            )
            .await
            .is_ok()
    );
}

#[test]
fn question_snapshot_survives_restart_and_rollback_removes_it() {
    use t3_workspace::attachment_delivery::AttachmentDelivery;
    let root = tempfile::tempdir().unwrap();
    let sources = root.path().join("attachments");
    fs::create_dir(&sources).unwrap();
    let refs = root.path().join("references");
    let restart = || AttachmentDelivery::with_reference_directory(sources.clone(), refs.clone());
    let path = sources.join("a.png");
    fs::write(&path, b"first").unwrap();
    let snapshot = restart().reference(&path).unwrap();
    assert!(snapshot.starts_with(&refs));
    fs::write(&path, b"later").unwrap();
    assert_eq!(fs::read(&snapshot).unwrap(), b"first");
    let delivery = restart();
    let replacement = delivery.reference(&path).unwrap();
    assert_ne!(snapshot, replacement);
    delivery.remove_claim(&path);
    assert!(!path.exists());
    assert!(!snapshot.exists());
    assert!(!replacement.exists());
}

#[cfg(unix)]
#[test]
fn question_snapshot_rejects_a_public_directory_on_every_attempt() {
    use std::os::unix::fs::PermissionsExt;
    use t3_workspace::attachment_delivery::AttachmentDelivery;
    let root = tempfile::tempdir().unwrap();
    let sources = root.path().join("attachments");
    let refs = root.path().join("references");
    fs::create_dir(&sources).unwrap();
    fs::create_dir(&refs).unwrap();
    fs::set_permissions(&refs, fs::Permissions::from_mode(0o755)).unwrap();
    let path = sources.join("a.png");
    fs::write(&path, b"a").unwrap();
    let delivery = AttachmentDelivery::with_reference_directory(sources, refs);
    assert!(delivery.reference(&path).is_err());
    assert!(delivery.reference(&path).is_err());
}
