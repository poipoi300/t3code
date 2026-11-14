//! Message attachments: who may claim a pending upload, and the runtime's claim port over the
//! server's attachments directory.
//!
//! TypeScript binds a pending upload to nobody: any caller that knows `pending-<uuid>` can claim
//! it, repeatedly. Here the issuer is recorded at `attachments.createUploadUrl` (the auth session,
//! or an MCP caller), a claim must come from that issuer, and an upload is claimed once (a claim
//! the runtime rolls back makes it claimable again). The claimer is the task-local [`CLAIMER`],
//! set where client calls are dispatched (`registry.rs`); a claim with no claimer is refused.
//! The ledger is in memory: an upload issued before a restart cannot be claimed after it.
use std::sync::Arc;

/// Who issued, and may claim, a pending upload.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Principal {
    /// A client's auth session.
    Session(String),
    /// An MCP caller: the token's thread, or its request namespace (t3-mcp's upload owner).
    Mcp(String),
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
    /// The upload and sweep clock; `None` is the system clock.
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
    use t3_workspace::attachments::AttachmentStore;

    /// Outstanding (issued, unclaimed) uploads one issuer may hold: one message's worth.
    const MAX_PENDING_PER_ISSUER: usize = PROVIDER_SEND_TURN_MAX_ATTACHMENTS;
    /// The sweep removes a pending file 24 h after its upload, which may come one URL lifetime after
    /// issue (TypeScript's MCP `UPLOAD_OWNER_TTL_MS`).
    const ISSUE_RECORD_TTL_MS: u64 =
        24 * 60 * 60 * 1000 + t3_contracts::assets::ATTACHMENT_UPLOAD_URL_TTL_MS;

    struct Issued {
        issuer: Principal,
        issued_ms: u64,
        claimed: bool,
    }

    #[derive(Default)]
    struct Ledger {
        issued: HashMap<String, Issued>,
        /// A claimed copy's path to the pending id it came from, so a rollback can reopen the upload.
        copies: HashMap<PathBuf, String>,
    }

    impl Ledger {
        fn prune(&mut self, now: u64) {
            self.issued
                .retain(|_, entry| now.saturating_sub(entry.issued_ms) <= ISSUE_RECORD_TTL_MS);
            let issued = &self.issued;
            self.copies.retain(|_, id| issued.contains_key(id));
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
                Arc::new(NoFollowCopy),
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

        /// `attachments.createUploadUrl` for `issuer`, within its outstanding-upload allowance.
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
            let result = self.store.issue_upload(input, now).await?;
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

        /// Records an upload another surface issued (MCP's `t3_attachment_prepare_upload`).
        pub async fn record_issued(&self, attachment_id: String, issuer: Principal) {
            let now = self.now();
            let mut ledger = self.ledger.lock().await;
            ledger.prune(now);
            ledger.issued.insert(
                attachment_id,
                Issued {
                    issuer,
                    issued_ms: now,
                    claimed: false,
                },
            );
        }

        /// `attachments.delete`: only the issuer removes its pending upload; anything else is a no-op
        /// that does not reveal whether the id exists.
        pub async fn delete_pending(
            &self,
            issuer: &Principal,
            attachment_id: &str,
        ) -> Result<(), t3_workspace::Error> {
            let mut ledger = self.ledger.lock().await;
            if ledger
                .issued
                .get(attachment_id)
                .is_none_or(|entry| entry.issuer != *issuer)
            {
                return Ok(());
            }
            self.store.delete_pending(attachment_id).await?;
            ledger.issued.remove(attachment_id);
            Ok(())
        }

        /// The upload route's clock-aware token check and storage (`POST /api/attachments/upload`).
        pub fn store(&self) -> &AttachmentStore {
            &self.store
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
            {
                let mut ledger = self.ledger.lock().await;
                ledger.prune(self.now());
                // Missing, expired, another caller's and already claimed read the same.
                if let Some(refused) = attachments.iter().find(|a| {
                    attachment_is_pending_upload(a)
                        && ledger
                            .issued
                            .get(attachment_id(a))
                            .is_none_or(|entry| entry.issuer != claimer || entry.claimed)
                }) {
                    return Err(claim_error(format!(
                        "Attachment '{}' cannot be sent: attachment not found (removed or expired).",
                        attachment_name(refused)
                    )));
                }
                for id in &pending {
                    if let Some(entry) = ledger.issued.get_mut(id) {
                        entry.claimed = true;
                    }
                }
            }
            let reopen = |ledger: &mut Ledger| {
                for id in &pending {
                    if let Some(entry) = ledger.issued.get_mut(id) {
                        entry.claimed = false;
                    }
                }
            };
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
                    reopen(&mut *self.ledger.lock().await);
                    Err(error)
                }
            }
        }

        async fn release_claimed_paths(&self, paths: Vec<PathBuf>) -> RuntimeResult<()> {
            {
                let mut ledger = self.ledger.lock().await;
                for path in &paths {
                    if let Some(id) = ledger.copies.remove(path)
                        && let Some(entry) = ledger.issued.get_mut(&id)
                    {
                        entry.claimed = false;
                    }
                }
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

    /// Copies a pending upload without following links: the source must be a regular file when
    /// opened (checked on the handle), and the destination is created exclusively.
    struct NoFollowCopy;

    #[async_trait]
    impl CopyFile for NoFollowCopy {
        async fn copy(&self, from: PathBuf, to: PathBuf) -> RuntimeResult<()> {
            tokio::task::spawn_blocking(move || copy_no_follow(&from, &to))
                .await
                .map_err(|error| claim_error(error.to_string()))?
                .map_err(|error| claim_error(format!("Failed to claim attachment: {error}")))
        }
    }

    fn copy_no_follow(from: &Path, to: &Path) -> std::io::Result<()> {
        use std::io::{Read, Write};
        let mut source = open_regular_no_follow(from)?;
        let size = source.metadata()?.len();
        let mut target = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)?;
        let copied = std::io::copy(&mut (&mut source).take(size), &mut target)?;
        if copied != size || source.read(&mut [0])? != 0 {
            drop(target);
            let _ = std::fs::remove_file(to);
            return Err(std::io::Error::other(
                "attachment changed while it was claimed",
            ));
        }
        target.flush()?;
        target.sync_all()
    }

    #[cfg(windows)]
    fn open_regular_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let metadata = file.metadata()?;
        if metadata.is_symlink() || !metadata.is_file() {
            return Err(std::io::Error::other("attachment is not a regular file"));
        }
        Ok(file)
    }

    /// No `O_NOFOLLOW` without a new dependency: the entry is checked with `lstat` and the opened
    /// handle must be that same file, so a link swapped in between is refused.
    #[cfg(unix)]
    fn open_regular_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
        use std::os::unix::fs::MetadataExt;
        let before = std::fs::symlink_metadata(path)?;
        if !before.is_file() {
            return Err(std::io::Error::other("attachment is not a regular file"));
        }
        let file = std::fs::File::open(path)?;
        let opened = file.metadata()?;
        if !opened.is_file() || opened.dev() != before.dev() || opened.ino() != before.ino() {
            return Err(std::io::Error::other(
                "attachment changed while it was opened",
            ));
        }
        Ok(file)
    }

    fn system_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as u64)
    }
}
