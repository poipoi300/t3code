//! Message attachments: who may claim a pending upload, and the runtime's claim port over the
//! server's attachments directory.
//!
//! TypeScript binds a pending upload to nobody: any caller that knows `pending-<uuid>` can claim
//! it, repeatedly. Here `attachments.createUploadUrl` signs the issuer into the upload token; the
//! upload records it beside the stored file (t3-workspace `UploadOwner`, `<file>.owner`), so it
//! survives a restart and goes with the file. A claim must come from that issuer, once: the
//! record is marked claimed, and a claim the runtime rolls back unmarks it. A missing, unreadable
//! or malformed record refuses the claim.
//!
//! Each intake gets a backend bound to the authenticated caller. The shared runtime backend
//! has no pending-upload authority.
use std::sync::Arc;

/// Who issued, and may claim, a pending upload: an identity that survives a restart.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Principal {
    /// A client's auth session (its stored session id).
    Session(String),
    /// The source thread bound to the verified MCP token.
    McpThread(String),
    /// The verified request namespace for an unthreaded MCP caller.
    McpNamespace(String),
}

impl Principal {
    /// The form recorded beside an upload.
    pub fn record(&self) -> String {
        match self {
            Self::Session(id) => format!("session:{id}"),
            Self::McpThread(id) => format!("mcp:thread:{id}"),
            Self::McpNamespace(id) => format!("mcp:namespace:{id}"),
        }
    }
}

#[cfg(feature = "orchestration")]
pub(crate) fn session_intake(
    ctx: &crate::registry::Context,
    base: &t3_orchestration::ports::runs_ingestion::thread_message_intake::ThreadMessageIntake,
) -> t3_orchestration::ports::runs_ingestion::thread_message_intake::ThreadMessageIntake {
    use t3_orchestration::ports::runs_ingestion::thread_message_intake::ThreadMessageIntake;
    #[cfg(feature = "workspace")]
    let attachments = ctx.services.domain::<AttachmentsAdapter>().map_or_else(
        || base.attachments.clone(),
        |adapter| -> Arc<dyn t3_orchestration::api::AttachmentBackend> {
            adapter.for_caller(Principal::Session(ctx.session_id.0.clone()))
        },
    );
    #[cfg(not(feature = "workspace"))]
    let attachments = {
        let _ = ctx;
        base.attachments.clone()
    };
    ThreadMessageIntake {
        attachments,
        threads: base.threads.clone(),
        launches: base.launches.clone(),
        projections: base.projections.clone(),
    }
}

/// Milliseconds since the epoch; tests drive it.
pub type Now = Arc<dyn Fn() -> u64 + Send + Sync>;

#[derive(Clone, Default)]
pub struct AttachmentOptions {
    /// The upload, sweep and asset clock; `None` is the system clock.
    pub clock: Option<Now>,
}

#[cfg(all(feature = "workspace", feature = "orchestration"))]
pub use claims::*;

#[cfg(all(feature = "workspace", feature = "orchestration"))]
mod claims {
    use super::{AttachmentOptions, Now, Principal};
    use async_trait::async_trait;
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
        sync::{Arc, OnceLock},
    };
    use t3_contracts::{
        assets::{AttachmentCreateUploadUrlInput, AttachmentCreateUploadUrlResult},
        chat_attachment::ChatAttachment,
    };
    use t3_orchestration::ports::runs_ingestion::thread_message_intake::{
        AttachmentReference, AttachmentReferenceResolver,
    };
    use t3_orchestration::{
        api::{
            AttachmentBackend, ClaimedAttachments, MessageFilter, ProjectionReader,
            ProjectionRecordQuery, ProjectionRecordSet, RecordFilter, RecordKind, ThreadId,
        },
        error::{Error as RuntimeError, Result as RuntimeResult},
        ports::runs_ingestion::attachment_claims::{
            AttachmentClaimService, CopyFile, NoDocumentText, attachment_id,
            attachment_is_pending_upload, attachment_path,
        },
    };
    use t3_workspace::{attachments::AttachmentStore, contained_files::ContainedFiles};

    #[derive(Debug, thiserror::Error)]
    pub enum UploadRefusal {
        #[error("too many uploads are waiting to be sent; send or remove some first")]
        TooMany,
        #[error(transparent)]
        Store(#[from] t3_workspace::Error),
    }

    #[derive(Clone)]
    pub struct AttachmentsAdapter {
        store: AttachmentStore,
        claims: AttachmentClaimService,
        ledger: Arc<tokio::sync::Mutex<()>>,
        projections: Arc<OnceLock<Arc<dyn ProjectionReader>>>,
        principal: Option<Principal>,
        // Only this intake can roll back its copies. Accepted entries die with the request.
        copies: Arc<tokio::sync::Mutex<HashMap<PathBuf, String>>>,
        now: Now,
    }

    impl AttachmentsAdapter {
        pub fn new(store: AttachmentStore, options: AttachmentOptions) -> Arc<Self> {
            let claims = AttachmentClaimService::with_delivery(
                store.base.clone(),
                Arc::new(NoDocumentText),
                Arc::new(ContainedCopy(store.clone())),
                store.delivery.clone(),
            );
            Arc::new(Self {
                store,
                claims,
                ledger: Default::default(),
                projections: Default::default(),
                principal: None,
                copies: Default::default(),
                now: options.clock.unwrap_or_else(|| Arc::new(system_ms)),
            })
        }

        pub fn for_caller(&self, principal: Principal) -> Arc<Self> {
            Arc::new(Self {
                principal: Some(principal),
                copies: Default::default(),
                ..self.clone()
            })
        }

        pub fn now(&self) -> u64 {
            (self.now)()
        }

        /// The thread records that decide whether a non-pending attachment belongs to its target.
        pub fn bind_projections(&self, projections: Arc<dyn ProjectionReader>) {
            let _ = self.projections.set(projections);
        }

        /// An upload URL whose upload records `issuer` as its owner, within the issuer's
        /// allowance for outstanding uploads.
        pub async fn create_upload_url(
            &self,
            issuer: Principal,
            input: AttachmentCreateUploadUrlInput,
        ) -> Result<AttachmentCreateUploadUrlResult, UploadRefusal> {
            self.store
                .issue_owned_upload(input, self.now(), Some(issuer.record()))
                .await
                .map_err(|error| match error {
                    t3_workspace::Error::Upload { status: 429, .. } => UploadRefusal::TooMany,
                    error => error.into(),
                })
        }

        /// `attachments.delete`: only the owner removes its pending upload; anything else is a
        /// no-op that does not reveal whether the id exists.
        pub async fn delete_pending(
            &self,
            issuer: &Principal,
            attachment_id: &str,
        ) -> Result<(), t3_workspace::Error> {
            self.discard_pending(issuer, attachment_id)
                .await
                .map(|_| ())
        }

        /// Reports whether the caller owned an unclaimed upload and discarded it under the claim lock.
        pub(crate) async fn discard_pending(
            &self,
            issuer: &Principal,
            attachment_id: &str,
        ) -> Result<bool, t3_workspace::Error> {
            self.store
                .discard_owned(attachment_id, &issuer.record())
                .await
        }

        async fn thread_attachments(
            &self,
            thread: &ThreadId,
        ) -> RuntimeResult<HashMap<String, ChatAttachment>> {
            let projections = self.projections.get().ok_or(RuntimeError::Unsupported(
                "AttachmentBackend.thread_records",
            ))?;
            let records = projections
                .query_records(ProjectionRecordQuery {
                    thread_id: thread.clone(),
                    fields: vec![RecordKind::Messages],
                    filter: RecordFilter::default(),
                    messages: MessageFilter::default(),
                    run_statuses: None,
                    include_latest_run: false,
                })
                .await?;
            let mut owned = HashMap::new();
            if let Some(share) = records.thread.guest_share.flatten() {
                for message in share.messages {
                    if message.status
                        == t3_contracts::guest_share::ThreadGuestMessageStatus::Pending
                    {
                        for attachment in message.attachments.flatten().unwrap_or_default() {
                            owned.insert(attachment_id(&attachment).to_owned(), attachment);
                        }
                    }
                }
            }
            for set in records.records {
                if let ProjectionRecordSet::Messages(messages) = set {
                    for attachment in messages.into_iter().flat_map(|message| message.attachments) {
                        owned.insert(attachment_id(&attachment).to_owned(), attachment);
                    }
                }
            }
            Ok(owned)
        }

        /// Marks `ids` claimed (or not) in their owner records; on a failed write, the ones
        /// already written are put back and the claim is refused.
        async fn set_claimed(&self, ids: &[String], claimed: bool) -> RuntimeResult<()> {
            for (done, id) in ids.iter().enumerate() {
                if self.store.set_claimed(id, claimed).await.is_err() {
                    for id in &ids[..done] {
                        let _ = self.store.set_claimed(id, !claimed).await;
                    }
                    return Err(claim_error("Failed to record the attachment claim."));
                }
            }
            Ok(())
        }
    }

    fn claim_error(detail: impl Into<String>) -> RuntimeError {
        RuntimeError::Service {
            operation: "AttachmentClaims.claimPendingAttachments",
            detail: detail.into(),
        }
    }

    fn attachment_name(attachment: &ChatAttachment) -> &str {
        match attachment {
            ChatAttachment::Image(value) => &value.name,
            ChatAttachment::File(value) => &value.name,
            ChatAttachment::Unknown(value) => &value.name,
        }
    }

    struct ClaimDelivery {
        adapter: AttachmentsAdapter,
        claimed: Option<ClaimedAttachments>,
    }
    impl ClaimDelivery {
        fn take(mut self) -> RuntimeResult<ClaimedAttachments> {
            self.claimed.take().ok_or(RuntimeError::Cancelled)
        }
    }
    impl Drop for ClaimDelivery {
        fn drop(&mut self) {
            if let Some(claimed) = self.claimed.take() {
                let adapter = self.adapter.clone();
                tokio::spawn(async move {
                    let _ = adapter.release_claimed_paths(claimed.claimed_paths).await;
                });
            }
        }
    }

    impl AttachmentsAdapter {
        async fn claim_uploads(
            &self,
            thread_id: ThreadId,
            attachments: Vec<ChatAttachment>,
        ) -> RuntimeResult<ClaimedAttachments> {
            let claimer = self.principal.as_ref();
            let pending: Vec<String> = attachments
                .iter()
                .filter(|a| attachment_is_pending_upload(a))
                .map(|a| attachment_id(a).to_owned())
                .collect();
            let ledger = self.ledger.lock().await;
            for attachment in attachments
                .iter()
                .filter(|a| attachment_is_pending_upload(a))
            {
                // Missing, unreadable, another caller's and already claimed read the same.
                let claimer = claimer.ok_or_else(|| {
                    claim_error(
                        "Uploaded attachments can be sent only by the caller that uploaded them.",
                    )
                })?;
                let id = attachment_id(attachment);
                let record = if id.bytes().any(|byte| byte.is_ascii_uppercase()) {
                    None
                } else {
                    self.store.owner(id).await
                };
                if record.is_none_or(|record| {
                    record.owner != claimer.record()
                        || record.claimed
                        || !record.matches_attachment(attachment)
                }) {
                    return Err(claim_error(format!(
                        "Attachment '{}' cannot be sent: attachment not found (removed or expired).",
                        attachment_name(attachment)
                    )));
                }
            }
            self.set_claimed(&pending, true).await?;
            drop(ledger);
            match self
                .claims
                .claim_pending(thread_id, attachments.clone())
                .await
            {
                Ok(claimed) => {
                    let mut copies = self.copies.lock().await;
                    for (before, after) in attachments.iter().zip(&claimed.attachments) {
                        if attachment_is_pending_upload(before)
                            && let Some(path) = attachment_path(&self.store.base, after)
                        {
                            copies.insert(path, attachment_id(before).to_owned());
                        }
                    }
                    Ok(claimed)
                }
                Err(error) => {
                    let _ledger = self.ledger.lock().await;
                    let _ = self.set_claimed(&pending, false).await;
                    Err(error)
                }
            }
        }
    }

    #[async_trait]
    impl AttachmentBackend for AttachmentsAdapter {
        async fn claim_pending(
            &self,
            thread_id: ThreadId,
            mut attachments: Vec<ChatAttachment>,
        ) -> RuntimeResult<ClaimedAttachments> {
            // A reference to an existing attachment must already be on the target thread, so one
            // thread's files never reach another thread's provider (MCP applies the same rule).
            if attachments.iter().any(|a| !attachment_is_pending_upload(a)) {
                let owned = self.thread_attachments(&thread_id).await?;
                for attachment in attachments
                    .iter_mut()
                    .filter(|a| !attachment_is_pending_upload(a))
                {
                    *attachment = owned.get(attachment_id(attachment)).ok_or_else(|| {
                        claim_error(format!("Attachment '{}' cannot be sent: it does not belong to this thread.", attachment_name(attachment)))
                    })?.clone();
                }
                t3_orchestration::ports::runs_ingestion::attachment_claims::validate_attachment_limits(&attachments)?;
            }
            if !attachments.iter().any(attachment_is_pending_upload) {
                return Ok(ClaimedAttachments {
                    attachments,
                    claimed_paths: vec![],
                });
            }
            let adapter = self.clone();
            // Finish the disk transaction even if the RPC goes away. A result that never
            // reaches intake owns its rollback until the receiver takes it.
            let delivery = tokio::spawn(async move {
                let claimed = adapter.claim_uploads(thread_id, attachments).await?;
                Ok::<_, RuntimeError>(ClaimDelivery {
                    adapter,
                    claimed: Some(claimed),
                })
            })
            .await
            .map_err(|e| claim_error(e.to_string()))??;
            delivery.take()
        }

        /// A rollback reopens the uploads its copies came from.
        async fn release_claimed_paths(&self, paths: Vec<PathBuf>) -> RuntimeResult<()> {
            {
                let _ledger = self.ledger.lock().await;
                let mut copies = self.copies.lock().await;
                let reopened: Vec<String> = paths
                    .iter()
                    .filter_map(|path| copies.remove(path))
                    .collect();
                let _ = self.set_claimed(&reopened, false).await;
            }
            self.claims.release_claimed_paths(paths).await
        }

        async fn resolve(
            &self,
            thread_id: ThreadId,
            attachments: Vec<ChatAttachment>,
        ) -> RuntimeResult<Vec<ChatAttachment>> {
            self.claims.resolve(thread_id, attachments).await
        }
        async fn claim(
            &self,
            thread_id: ThreadId,
            attachment_ids: Vec<String>,
        ) -> RuntimeResult<()> {
            self.claims.claim(thread_id, attachment_ids).await
        }
        async fn release(
            &self,
            thread_id: ThreadId,
            attachment_ids: Vec<String>,
        ) -> RuntimeResult<()> {
            self.claims.release(thread_id, attachment_ids).await
        }
    }

    #[async_trait]
    impl AttachmentReferenceResolver for AttachmentsAdapter {
        async fn reference(
            &self,
            attachment: &ChatAttachment,
        ) -> RuntimeResult<AttachmentReference> {
            let base = self.store.base.clone();
            let path = attachment_path(&base, attachment)
                .ok_or_else(|| claim_error("Invalid attachment reference."))?;
            let document = matches!(attachment, ChatAttachment::File(_));
            let delivery = self.store.delivery.clone();
            tokio::task::spawn_blocking(move || {
                let saved = delivery.reference(&path).map_err(|_| {
                    claim_error("Attachment is no longer available. Attach it again.")
                })?;
                let extracted_path = if document {
                    let mut text = path.into_os_string();
                    text.push(".txt");
                    delivery.reference(Path::new(&text)).ok()
                } else {
                    None
                };
                Ok(AttachmentReference {
                    path: saved,
                    extracted_path,
                })
            })
            .await
            .map_err(|e| claim_error(e.to_string()))?
        }
    }

    fn checked_stat(
        files: &ContainedFiles,
        path: &Path,
        source: &t3_workspace::contained_files::ContainedFile,
    ) -> std::io::Result<t3_workspace::contained_files::FileStat> {
        let stat = source.stat()?;
        if !stat.regular || stat.links != 1 || files.lstat_identity(path)? != stat.identity {
            return Err(std::io::Error::other(
                "attachment is not a regular file of its own",
            ));
        }
        Ok(stat)
    }

    /// Copies a pending upload through a handle opened under the attachments directory without
    /// following a link (`ContainedFiles`); the opened file must be the entry itself, and the
    /// destination is created exclusively.
    struct ContainedCopy(AttachmentStore);

    #[async_trait]
    impl CopyFile for ContainedCopy {
        async fn copy(&self, from: PathBuf, to: PathBuf) -> RuntimeResult<()> {
            let files = self.0.clone();
            tokio::task::spawn_blocking(move || copy_contained(&files, &from, &to))
                .await
                .map_err(|error| claim_error(error.to_string()))?
                .map_err(|error| claim_error(format!("Failed to claim attachment: {error}")))
        }
    }

    fn copy_contained(store: &AttachmentStore, from: &Path, to: &Path) -> std::io::Result<()> {
        use std::io::Write;
        const CHUNK: usize = 64 * 1024;
        let files = ContainedFiles::new(store.base.clone());
        let source = files.open(&files.realpath(from)?)?;
        let stat = checked_stat(&files, from, &source)?;
        if let Some(id) = from.file_stem().and_then(|s| s.to_str())
            && t3_workspace::attachments::parse_id(id)
                .is_some_and(|p| p.thread_segment == "pending")
        {
            store
                .verify_claimed_source(from, id, &stat)
                .map_err(std::io::Error::other)?;
        }
        store
            .delivery
            .write_claim(to, |target| {
                let mut offset = 0;
                loop {
                    let chunk = source.read_at(offset, CHUNK)?;
                    if chunk.is_empty() && offset == stat.size {
                        return Ok(());
                    }
                    offset += chunk.len() as u64;
                    if chunk.is_empty() || offset > stat.size {
                        return Err(std::io::Error::other(
                            "attachment changed while it was claimed",
                        ));
                    }
                    target.write_all(&chunk)?;
                }
            })
            .map_err(std::io::Error::other)
    }

    fn system_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as u64)
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    mod tests {
        use super::*;

        #[test]
        fn claim_rechecks_link_count_on_the_opened_handle() {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("source.png");
            std::fs::write(&path, b"image").unwrap();
            let files = ContainedFiles::new(temp.path());
            let handle = files.open(&files.realpath(&path).unwrap()).unwrap();
            std::fs::hard_link(&path, temp.path().join("alias.png")).unwrap();
            assert!(checked_stat(&files, &path, &handle).is_err());
        }
        /// A claim reached without a caller in scope (a spawned task, a host path that never set
        /// one) is refused, never treated as anyone's.
        #[tokio::test]
        async fn a_claim_without_a_caller_is_refused() {
            let temp = tempfile::tempdir().unwrap();
            let store = AttachmentStore::new(
                temp.path().into(),
                t3_workspace::assets::AssetSigner::new(vec![3; 32]).unwrap(),
                Arc::new(NoDocuments),
            );
            let adapter = AttachmentsAdapter::new(store, AttachmentOptions::default());
            let upload: ChatAttachment = serde_json::from_value(serde_json::json!({
                "type": "image",
                "id": "pending-00000000-0000-4000-8000-000000000000",
                "name": "pixel.png",
                "mimeType": "image/png",
                "sizeBytes": 1
            }))
            .unwrap();
            let refused = adapter
                .claim_pending(ThreadId("thread".into()), vec![upload])
                .await
                .unwrap_err();
            assert!(
                refused.to_string().contains("only by the caller"),
                "{refused}"
            );
        }

        #[tokio::test]
        async fn stalled_upload_discard_does_not_block_other_issuers() {
            let temp = tempfile::tempdir().unwrap();
            let store = AttachmentStore::new(
                temp.path().into(),
                t3_workspace::assets::AssetSigner::new(vec![3; 32]).unwrap(),
                Arc::new(NoDocuments),
            );
            let adapter = AttachmentsAdapter::new(
                store.clone(),
                AttachmentOptions {
                    clock: Some(Arc::new(|| 100)),
                },
            );
            let image = || {
                serde_json::from_value(serde_json::json!({"type":"image","name":"a.png","mimeType":"image/png","sizeBytes":1})).unwrap()
            };
            let owner = Principal::Session("a".into());
            let grant = adapter
                .create_upload_url(owner.clone(), image())
                .await
                .unwrap();
            let url = grant.relative_url.0.rsplit('/').next().unwrap().to_owned();
            let (writer, reader) = tokio::io::duplex(8);
            let upload = tokio::spawn(async move { store.upload(&url, 101, reader).await });
            let timeout = std::time::Duration::from_secs(5);
            tokio::time::timeout(timeout, async {
                while !std::fs::read_dir(temp.path())
                    .unwrap()
                    .flatten()
                    .any(|e| e.file_name().to_string_lossy().ends_with(".part"))
                {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let discard = adapter.discard_pending(&owner, &grant.attachment_id.0);
            tokio::pin!(discard);
            assert!(futures::poll!(&mut discard).is_pending());
            tokio::time::timeout(
                timeout,
                adapter.create_upload_url(Principal::Session("b".into()), image()),
            )
            .await
            .unwrap()
            .unwrap();
            drop(writer);
            assert!(upload.await.unwrap().is_err());
            assert!(discard.await.unwrap());
        }

        struct NoDocuments;
        impl t3_workspace::attachments::DocumentText for NoDocuments {
            fn ensure<'a>(
                &'a self,
                _: &'a Path,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
                Box::pin(async {})
            }
        }
    }
}
