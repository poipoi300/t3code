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
//! The claimer is the task-local [`CLAIMER`], set for every unary client call in `registry.rs`;
//! a claim with no claimer is refused. The runtime's `claim_pending` does not take the caller
//! yet (orchestration seam request).
use std::sync::Arc;

/// Who issued, and may claim, a pending upload: an identity that survives a restart.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Principal {
    /// A client's auth session (its stored session id).
    Session(String),
    /// An MCP caller: the thread its token is scoped to, else its request namespace.
    Mcp(String),
}

impl Principal {
    /// The form recorded beside an upload.
    pub fn record(&self) -> String {
        match self {
            Self::Session(id) => format!("session:{id}"),
            Self::Mcp(owner) => format!("mcp:{owner}"),
        }
    }
}

tokio::task_local! {
    /// The caller a claim made in this task acts for.
    pub static CLAIMER: Principal;
}

/// Runs `future` with `principal` as the claimer of any pending upload it sends.
pub async fn as_claimer<F: std::future::Future>(principal: Principal, future: F) -> F::Output {
    CLAIMER.scope(principal, future).await
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
    use super::{AttachmentOptions, CLAIMER, Now, Principal};
    use async_trait::async_trait;
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
        sync::{Arc, OnceLock},
    };
    use t3_contracts::{
        assets::{AttachmentCreateUploadUrlInput, AttachmentCreateUploadUrlResult},
        chat_attachment::{ChatAttachment, PROVIDER_SEND_TURN_MAX_ATTACHMENTS},
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

    /// Upload URLs one issuer may hold unclaimed at a time: one message's worth.
    const MAX_PENDING_PER_ISSUER: usize = PROVIDER_SEND_TURN_MAX_ATTACHMENTS;
    /// An issued URL counts against its issuer until the sweep could have removed its upload:
    /// 24 h after an upload made up to one URL lifetime after issue.
    const ISSUE_RECORD_TTL_MS: u64 =
        24 * 60 * 60 * 1000 + t3_contracts::assets::ATTACHMENT_UPLOAD_URL_TTL_MS;

    struct Issued {
        issuer: Principal,
        issued_ms: u64,
        claimed: bool,
    }

    /// In memory: the per-issuer allowance (a limit, so it may restart empty) and each claimed
    /// copy's pending id, which a rollback in this process needs. Its lock also serializes claims.
    #[derive(Default)]
    struct Ledger {
        issued: HashMap<String, Issued>,
        copies: HashMap<PathBuf, String>,
    }

    impl Ledger {
        fn prune(&mut self, now: u64) {
            self.issued
                .retain(|_, entry| now.saturating_sub(entry.issued_ms) <= ISSUE_RECORD_TTL_MS);
        }
        fn mark(&mut self, id: &str, claimed: bool) {
            if let Some(entry) = self.issued.get_mut(id) {
                entry.claimed = claimed;
            }
        }
    }

    #[derive(Debug, thiserror::Error)]
    pub enum UploadRefusal {
        #[error("too many uploads are waiting to be sent; send or remove some first")]
        TooMany,
        #[error(transparent)]
        Store(#[from] t3_workspace::Error),
    }

    pub struct AttachmentsAdapter {
        store: AttachmentStore,
        claims: AttachmentClaimService,
        ledger: tokio::sync::Mutex<Ledger>,
        projections: OnceLock<Arc<dyn ProjectionReader>>,
        now: Now,
    }

    impl AttachmentsAdapter {
        pub fn new(store: AttachmentStore, options: AttachmentOptions) -> Arc<Self> {
            let claims = AttachmentClaimService::with_copy_file(
                store.base.clone(),
                Arc::new(NoDocumentText),
                Arc::new(ContainedCopy(ContainedFiles::new(store.base.clone()))),
            );
            Arc::new(Self {
                store,
                claims,
                ledger: Default::default(),
                projections: OnceLock::new(),
                now: options.clock.unwrap_or_else(|| Arc::new(system_ms)),
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
        /// allowance. `attachments.createUploadUrl`; MCP's prepare tool uses it too.
        pub async fn create_upload_url(
            &self,
            issuer: Principal,
            input: AttachmentCreateUploadUrlInput,
        ) -> Result<AttachmentCreateUploadUrlResult, UploadRefusal> {
            let now = self.now();
            let mut ledger = self.ledger.lock().await;
            ledger.prune(now);
            let outstanding = ledger
                .issued
                .values()
                .filter(|entry| entry.issuer == issuer && !entry.claimed)
                .count();
            if outstanding >= MAX_PENDING_PER_ISSUER {
                return Err(UploadRefusal::TooMany);
            }
            let result = self
                .store
                .issue_owned_upload(input, now, Some(issuer.record()))
                .await?;
            ledger.issued.insert(
                result.attachment_id.0.clone(),
                Issued {
                    issuer,
                    issued_ms: now,
                    claimed: false,
                },
            );
            Ok(result)
        }

        /// `attachments.delete`: only the owner removes its pending upload; anything else is a
        /// no-op that does not reveal whether the id exists.
        pub async fn delete_pending(
            &self,
            issuer: &Principal,
            attachment_id: &str,
        ) -> Result<(), t3_workspace::Error> {
            let mut ledger = self.ledger.lock().await;
            let recorded = self.store.owner(attachment_id).await;
            let owned = match &recorded {
                Some(record) => record.owner == issuer.record(),
                None => ledger
                    .issued
                    .get(attachment_id)
                    .is_some_and(|entry| entry.issuer == *issuer),
            };
            if !owned {
                return Ok(());
            }
            self.store.delete_pending(attachment_id).await?;
            ledger.issued.remove(attachment_id);
            Ok(())
        }

        async fn thread_attachment_ids(&self, thread: &ThreadId) -> RuntimeResult<Vec<String>> {
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
            Ok(records
                .records
                .into_iter()
                .flat_map(|set| match set {
                    ProjectionRecordSet::Messages(messages) => messages
                        .into_iter()
                        .flat_map(|message| message.attachments)
                        .map(|attachment| attachment_id(&attachment).to_owned())
                        .collect(),
                    _ => vec![],
                })
                .collect())
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

    #[async_trait]
    impl AttachmentBackend for AttachmentsAdapter {
        async fn claim_pending(
            &self,
            thread_id: ThreadId,
            attachments: Vec<ChatAttachment>,
        ) -> RuntimeResult<ClaimedAttachments> {
            let claimer = CLAIMER.try_with(Clone::clone).map_err(|_| {
                claim_error(
                    "Uploaded attachments can be sent only by the caller that uploaded them.",
                )
            })?;
            // A reference to an existing attachment must already be on the target thread, so one
            // thread's files never reach another thread's provider (MCP applies the same rule).
            if attachments.iter().any(|a| !attachment_is_pending_upload(a)) {
                let owned = self.thread_attachment_ids(&thread_id).await?;
                if let Some(foreign) = attachments.iter().find(|a| {
                    !attachment_is_pending_upload(a)
                        && !owned.iter().any(|id| id == attachment_id(a))
                }) {
                    return Err(claim_error(format!(
                        "Attachment '{}' cannot be sent: it does not belong to this thread.",
                        attachment_name(foreign)
                    )));
                }
            }
            let pending: Vec<String> = attachments
                .iter()
                .filter(|a| attachment_is_pending_upload(a))
                .map(|a| attachment_id(a).to_owned())
                .collect();
            let mut ledger = self.ledger.lock().await;
            for attachment in attachments
                .iter()
                .filter(|a| attachment_is_pending_upload(a))
            {
                // Missing, unreadable, another caller's and already claimed read the same.
                let record = self.store.owner(attachment_id(attachment)).await;
                if record.is_none_or(|record| record.owner != claimer.record() || record.claimed) {
                    return Err(claim_error(format!(
                        "Attachment '{}' cannot be sent: attachment not found (removed or expired).",
                        attachment_name(attachment)
                    )));
                }
            }
            self.set_claimed(&pending, true).await?;
            pending.iter().for_each(|id| ledger.mark(id, true));
            drop(ledger);
            match self
                .claims
                .claim_pending(thread_id, attachments.clone())
                .await
            {
                Ok(claimed) => {
                    let mut ledger = self.ledger.lock().await;
                    for (before, after) in attachments.iter().zip(&claimed.attachments) {
                        if attachment_is_pending_upload(before)
                            && let Some(path) = attachment_path(&self.store.base, after)
                        {
                            ledger.copies.insert(path, attachment_id(before).to_owned());
                        }
                    }
                    Ok(claimed)
                }
                Err(error) => {
                    let mut ledger = self.ledger.lock().await;
                    let _ = self.set_claimed(&pending, false).await;
                    pending.iter().for_each(|id| ledger.mark(id, false));
                    Err(error)
                }
            }
        }

        /// A rollback reopens the uploads its copies came from.
        async fn release_claimed_paths(&self, paths: Vec<PathBuf>) -> RuntimeResult<()> {
            {
                let mut ledger = self.ledger.lock().await;
                let reopened: Vec<String> = paths
                    .iter()
                    .filter_map(|path| ledger.copies.remove(path))
                    .collect();
                let _ = self.set_claimed(&reopened, false).await;
                reopened.iter().for_each(|id| ledger.mark(id, false));
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

    /// Copies a pending upload through a handle opened under the attachments directory without
    /// following a link (`ContainedFiles`); the opened file must be the entry itself, and the
    /// destination is created exclusively.
    struct ContainedCopy(ContainedFiles);

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

    fn copy_contained(files: &ContainedFiles, from: &Path, to: &Path) -> std::io::Result<()> {
        use std::io::Write;
        const CHUNK: usize = 64 * 1024;
        let source = files.open(&files.realpath(from)?)?;
        let stat = source.stat()?;
        if !stat.regular || files.lstat_identity(from)? != stat.identity {
            return Err(std::io::Error::other(
                "attachment is not a regular file of its own",
            ));
        }
        let mut target = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)?;
        let mut offset = 0;
        let result = loop {
            let chunk = source.read_at(offset, CHUNK)?;
            if chunk.is_empty() {
                break if offset == stat.size {
                    Ok(())
                } else {
                    Err(std::io::Error::other(
                        "attachment changed while it was claimed",
                    ))
                };
            }
            offset += chunk.len() as u64;
            if offset > stat.size {
                break Err(std::io::Error::other(
                    "attachment changed while it was claimed",
                ));
            }
            target.write_all(&chunk)?;
        };
        match result.and_then(|()| target.sync_all()) {
            Ok(()) => Ok(()),
            Err(error) => {
                drop(target);
                let _ = std::fs::remove_file(to);
                Err(error)
            }
        }
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
