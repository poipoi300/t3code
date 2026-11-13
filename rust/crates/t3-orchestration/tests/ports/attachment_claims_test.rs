use async_trait::async_trait;
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use t3_contracts::chat_attachment::ChatAttachment;
use t3_orchestration::{
    api::{AttachmentBackend, ThreadId},
    error::Result,
    ports::runs_ingestion::attachment_claims::*,
};
use tokio::sync::{Notify, mpsc};

fn pending(
    directory: &Path,
    name: &str,
    mime: &str,
    bytes: &[u8],
    extension: Option<&str>,
) -> ChatAttachment {
    let id = format!(
        "pending-{}{}",
        uuid::Uuid::new_v4(),
        extension
            .map(|extension| format!("-{extension}"))
            .unwrap_or_default()
    );
    let attachment:ChatAttachment=serde_json::from_value(json!({"type":if mime=="image/png" {"image"}else{"file"},"id":id,"name":name,"mimeType":mime,"sizeBytes":bytes.len()})).unwrap();
    std::fs::write(attachment_path(directory, &attachment).unwrap(), bytes).unwrap();
    attachment
}
struct Documents(AtomicUsize);

#[test]
fn image_paths_match_typescript_mime_parameters_and_filename_fallbacks() {
    for (mime, expected) in [
        ("image/x-ms-bmp", "jpeg"),
        ("image/x-icon", "jpeg"),
        ("image/vnd.microsoft.icon", "jpeg"),
        ("image/pjpeg", "jpeg"),
        ("image/x-png", "jpeg"),
        ("IMAGE/PNG", "png"),
        ("image/png; charset=utf8", "png"),
        ("image/jpeg; charset=utf8", "jpg"),
        ("image/tiff; charset=utf8", "jpeg"),
        ("image/svg+xml;charset=utf8", "svg"),
        ("image/webp; charset=utf8", "webp"),
        (" image/png ", "png"),
    ] {
        let attachment:ChatAttachment=serde_json::from_value(json!({"type":"image","id":"image-oracle","name":"picture.jpeg","mimeType":mime,"sizeBytes":1})).unwrap();
        assert_eq!(
            attachment_path(Path::new("attachments"), &attachment).unwrap(),
            Path::new("attachments").join(format!("image-oracle.{expected}")),
            "{mime}"
        );
    }
}
#[async_trait]
impl DocumentText for Documents {
    async fn ensure(&self, _path: PathBuf) -> Result<Option<PathBuf>> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
}

#[tokio::test]
async fn copies_uploads_preserving_retry_sources_normalizes_metadata_and_tracks_exact_paths()
-> Result<()> {
    let directory = tempfile::tempdir()?;
    let upload = pending(
        directory.path(),
        "screenshot.png",
        "image/png",
        &[1, 2, 3, 4],
        None,
    );
    let source = attachment_path(directory.path(), &upload).unwrap();
    let service = AttachmentClaimService::new(directory.path().into(), Arc::new(NoDocumentText));
    let claimed = service
        .claim_pending(ThreadId("thread:test".into()), vec![upload.clone()])
        .await?;
    assert_ne!(
        attachment_id(&claimed.attachments[0]),
        attachment_id(&upload)
    );
    assert!(attachment_id(&claimed.attachments[0]).starts_with("thread-test-"));
    assert_eq!(std::fs::read(&claimed.claimed_paths[0])?, vec![1, 2, 3, 4]);
    assert_eq!(std::fs::read(source)?, vec![1, 2, 3, 4]);
    service
        .release_claimed_paths(claimed.claimed_paths.clone())
        .await?;
    assert!(!claimed.claimed_paths[0].exists());
    service.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn pending_document_extraction_is_copied_without_reparsing_and_sidecars_are_released()
-> Result<()> {
    let directory = tempfile::tempdir()?;
    let upload = pending(
        directory.path(),
        "document.docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        b"invalid document",
        Some("docx"),
    );
    let source = attachment_path(directory.path(), &upload).unwrap();
    std::fs::write(format!("{}.txt", source.display()), "already extracted")?;
    let documents = Arc::new(Documents(AtomicUsize::new(0)));
    let service = AttachmentClaimService::new(directory.path().into(), documents.clone());
    let claimed = service
        .claim_pending(ThreadId("thread".into()), vec![upload])
        .await?;
    assert_eq!(documents.0.load(Ordering::Relaxed), 0);
    assert_eq!(claimed.claimed_paths.len(), 2);
    assert_eq!(
        std::fs::read_to_string(&claimed.claimed_paths[1])?,
        "already extracted"
    );
    assert!(source.exists());
    service
        .release_claimed_paths(claimed.claimed_paths.clone())
        .await?;
    assert!(claimed.claimed_paths.iter().all(|path| !path.exists()));
    service.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn duplicate_ids_and_image_budgets_fail_before_any_copy_and_size_failure_rolls_back_prior_copies()
-> Result<()> {
    for failure in ["duplicate", "image-budget", "size"] {
        let directory = tempfile::tempdir()?;
        let first = pending(
            directory.path(),
            "one.png",
            "image/png",
            &[1, 2, 3, 4],
            None,
        );
        let second = pending(directory.path(), "two.png", "image/png", &[5], None);
        let mut attachments = vec![first.clone(), second];
        match failure {
            "duplicate" => attachments = vec![first.clone(), first],
            "image-budget" => {
                let ChatAttachment::Image(second) = &mut attachments[1] else {
                    unreachable!()
                };
                second.size_bytes = 80 * 1024 * 1024;
            }
            _ => {
                let ChatAttachment::Image(second) = &mut attachments[1] else {
                    unreachable!()
                };
                second.size_bytes = 2;
            }
        }
        let service =
            AttachmentClaimService::new(directory.path().into(), Arc::new(NoDocumentText));
        let error = service
            .claim_pending(ThreadId("thread".into()), attachments)
            .await
            .unwrap_err();
        assert!(error.to_string().contains(match failure {
            "duplicate" => "Duplicate",
            "image-budget" => "80 MiB",
            _ => "stored size does not match",
        }));
        assert!(std::fs::read_dir(directory.path())?.all(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("pending-")
        }));
        service.shutdown().await;
    }
    Ok(())
}

struct GatedCopy {
    started: mpsc::UnboundedSender<(PathBuf, PathBuf)>,
    release: Arc<Notify>,
}
#[async_trait]
impl CopyFile for GatedCopy {
    async fn copy(&self, from: PathBuf, to: PathBuf) -> Result<()> {
        self.started.send((from.clone(), to.clone())).unwrap();
        self.release.notified().await;
        std::fs::copy(from, to)?;
        Ok(())
    }
}
struct GatedDocuments {
    started: mpsc::UnboundedSender<PathBuf>,
    release: Arc<Notify>,
}
#[async_trait]
impl DocumentText for GatedDocuments {
    async fn ensure(&self, path: PathBuf) -> Result<Option<PathBuf>> {
        self.started.send(path.clone()).unwrap();
        self.release.notified().await;
        let sidecar = PathBuf::from(format!("{}.txt", path.display()));
        std::fs::write(&sidecar, "extracted")?;
        Ok(Some(sidecar))
    }
}

#[tokio::test]
async fn dropped_claims_await_late_copy_and_sidecar_publication_before_rollback() -> Result<()> {
    for during_copy in [true, false] {
        let directory = tempfile::tempdir()?;
        let upload = pending(directory.path(), "one.png", "image/png", &[1], None);
        let release = Arc::new(Notify::new());
        let (copy_tx, mut copy_rx) = mpsc::unbounded_channel();
        let (docs_tx, mut docs_rx) = mpsc::unbounded_channel();
        let service = if during_copy {
            AttachmentClaimService::with_copy_file(
                directory.path().into(),
                Arc::new(NoDocumentText),
                Arc::new(GatedCopy {
                    started: copy_tx,
                    release: release.clone(),
                }),
            )
        } else {
            AttachmentClaimService::new(
                directory.path().into(),
                Arc::new(GatedDocuments {
                    started: docs_tx,
                    release: release.clone(),
                }),
            )
        };
        let mut claim =
            Box::pin(service.claim_pending(ThreadId("thread".into()), vec![upload.clone()]));
        let path = if during_copy {
            tokio::select! {result=&mut claim=>panic!("early claim result: {result:?}"),args=copy_rx.recv()=>args.unwrap().1}
        } else {
            tokio::select! {result=&mut claim=>panic!("early claim result: {result:?}"),path=docs_rx.recv()=>path.unwrap()}
        };
        drop(claim);
        release.notify_one();
        service.shutdown().await;
        assert!(!path.exists());
        assert!(!PathBuf::from(format!("{}.txt", path.display())).exists());
        assert!(attachment_path(directory.path(), &upload).unwrap().exists());
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
    }
    Ok(())
}
