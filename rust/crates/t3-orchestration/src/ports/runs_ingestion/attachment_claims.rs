use crate::{
    api::{AttachmentBackend, ClaimedAttachments, ThreadId},
    error::{Error, Result},
};
use async_trait::async_trait;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use t3_contracts::chat_attachment::{
    ChatAttachment, PROVIDER_SEND_TURN_MAX_ATTACHMENTS,
    PROVIDER_SEND_TURN_SUPPORTED_IMAGE_MIME_TYPES,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[async_trait]
pub trait DocumentText: Send + Sync {
    async fn ensure(&self, path: PathBuf) -> Result<Option<PathBuf>>;
}

pub struct NoDocumentText;
#[async_trait]
pub trait CopyFile: Send + Sync {
    async fn copy(&self, from: PathBuf, to: PathBuf) -> Result<()>;
}
struct NativeCopyFile(Arc<t3_workspace::attachment_delivery::AttachmentDelivery>);
#[async_trait]
impl CopyFile for NativeCopyFile {
    async fn copy(&self, from: PathBuf, to: PathBuf) -> Result<()> {
        let delivery = self.0.clone();
        tokio::task::spawn_blocking(move || delivery.copy_claim(&from, &to))
            .await
            .map_err(|e| claim_error(e.to_string()))?
            .map_err(|e| claim_error(e.to_string()))
    }
}
#[async_trait]
impl DocumentText for NoDocumentText {
    async fn ensure(&self, _path: PathBuf) -> Result<Option<PathBuf>> {
        Ok(None)
    }
}

pub fn attachment_id(attachment: &ChatAttachment) -> &str {
    match attachment {
        ChatAttachment::Image(value) => &value.id,
        ChatAttachment::File(value) => &value.id,
        ChatAttachment::Unknown(value) => &value.id,
    }
}
fn properties(attachment: &ChatAttachment) -> (&str, &str, u64) {
    match attachment {
        ChatAttachment::Image(value) => (&value.name, &value.mime_type, value.size_bytes),
        ChatAttachment::File(value) => (&value.name, &value.mime_type, value.size_bytes),
        ChatAttachment::Unknown(value) => (&value.name, &value.mime_type, value.size_bytes),
    }
}
fn claim_error(detail: impl Into<String>) -> Error {
    Error::Service {
        operation: "AttachmentClaims.claimPendingAttachments",
        detail: detail.into(),
    }
}

pub fn validate_attachment_limits(attachments: &[ChatAttachment]) -> Result<()> {
    if attachments.len() > PROVIDER_SEND_TURN_MAX_ATTACHMENTS {
        return Err(claim_error(format!(
            "You can attach up to {PROVIDER_SEND_TURN_MAX_ATTACHMENTS} files per message or question response."
        )));
    }
    let image_bytes = attachments
        .iter()
        .filter(|attachment| {
            matches!(attachment, ChatAttachment::Image(_))
                || PROVIDER_SEND_TURN_SUPPORTED_IMAGE_MIME_TYPES
                    .contains(&properties(attachment).1.to_ascii_lowercase().as_str())
        })
        .fold(0_u64, |total, attachment| {
            total.saturating_add(properties(attachment).2)
        });
    if image_bytes > 80 * 1024 * 1024 {
        return Err(claim_error(
            "Images can total up to 80 MiB per message or question response. Use smaller images or send fewer at once.",
        ));
    }
    Ok(())
}

fn pending_suffix(attachment: &ChatAttachment) -> Option<Option<&str>> {
    let id = attachment_id(attachment);
    let (prefix, rest) = id.split_once('-')?;
    if !prefix.eq_ignore_ascii_case("pending") || rest.len() < 36 {
        return None;
    }
    let uuid = rest.as_bytes().get(..36)?;
    if !uuid.iter().enumerate().all(|(i, c)| {
        if [8, 13, 18, 23].contains(&i) {
            *c == b'-'
        } else {
            c.is_ascii_hexdigit()
        }
    }) {
        return None;
    }
    match rest.get(36..)? {
        "" => Some(None),
        suffix => {
            let extension = suffix.strip_prefix('-')?;
            (extension.len() <= 10
                && !extension.is_empty()
                && extension.bytes().all(|c| c.is_ascii_alphanumeric()))
            .then_some(Some(extension))
        }
    }
}

pub fn attachment_is_pending_upload(attachment: &ChatAttachment) -> bool {
    pending_suffix(attachment).is_some()
}

fn thread_segment(thread: &str) -> Option<String> {
    let mut result = String::new();
    for c in t3_contracts::wire::trim_js(thread).chars() {
        let c = if c.is_ascii_alphanumeric() || c == '_' {
            c.to_ascii_lowercase()
        } else {
            '-'
        };
        if c != '-' || !result.ends_with('-') {
            result.push(c);
        }
    }
    let mut result = result
        .trim_matches(['-', '_'])
        .chars()
        .take(80)
        .collect::<String>();
    result.truncate(result.trim_end_matches(['-', '_']).len());
    if result.is_empty() {
        None
    } else if result == "pending" {
        Some("_pending".into())
    } else {
        Some(result)
    }
}

fn image_extension(name: &str, mime: &str) -> String {
    let key = mime.to_ascii_lowercase();
    let direct = match key.as_str() {
        "image/avif" => Some("avif"),
        "image/bmp" => Some("bmp"),
        "image/gif" => Some("gif"),
        "image/heic" => Some("heic"),
        "image/heif" => Some("heif"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/svg+xml" => Some("svg"),
        "image/tiff" => Some("tiff"),
        "image/webp" => Some("webp"),
        _ => None,
    };
    // Effect's standard MIME table maps TIFF to unsafe .tif and has no icon/BMP aliases.
    let standard = match t3_contracts::wire::trim_js(key.split(';').next().unwrap_or_default()) {
        "image/avif" => Some("avif"),
        "image/bmp" => Some("bmp"),
        "image/gif" => Some("gif"),
        "image/heic" => Some("heic"),
        "image/heif" => Some("heif"),
        "image/jpeg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/svg+xml" => Some("svg"),
        "image/webp" => Some("webp"),
        _ => None,
    };
    if let Some(extension) = direct.or(standard) {
        return extension.into();
    }
    let extension = t3_contracts::wire::trim_js(name)
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    if [
        "avif", "bmp", "gif", "heic", "heif", "ico", "jpeg", "jpg", "png", "svg", "tiff", "webp",
    ]
    .contains(&extension.as_str())
    {
        extension
    } else {
        "bin".into()
    }
}

pub fn attachment_path(directory: &Path, attachment: &ChatAttachment) -> Option<PathBuf> {
    let id = attachment_id(attachment);
    if id.is_empty()
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return None;
    }
    let (name, mime, _) = properties(attachment);
    let extension = match attachment {
        ChatAttachment::Image(_) => image_extension(name, mime),
        ChatAttachment::File(_) => {
            let extension = Path::new(name)
                .extension()
                .and_then(|part| part.to_str())
                .unwrap_or_default();
            if extension.is_empty()
                || extension.len() > 10
                || extension.eq_ignore_ascii_case("part")
                || !extension.bytes().all(|c| c.is_ascii_alphanumeric())
            {
                "bin".into()
            } else {
                extension.to_ascii_lowercase()
            }
        }
        ChatAttachment::Unknown(_) => return None,
    };
    Some(directory.join(format!("{id}.{extension}")))
}

fn sidecar(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(".txt");
    value.into()
}
#[derive(Clone)]
pub struct AttachmentClaimService(Arc<ClaimState>);
struct ClaimState {
    delivery: Arc<t3_workspace::attachment_delivery::AttachmentDelivery>,
    directory: PathBuf,
    documents: Arc<dyn DocumentText>,
    copy_file: Arc<dyn CopyFile>,
    tasks: TaskTracker,
    closed: Mutex<bool>,
}
impl ClaimState {
    fn release_paths(&self, paths: &[PathBuf]) {
        for path in paths {
            self.delivery.remove_claim(path);
            self.delivery.remove_claim(&sidecar(path));
        }
    }
}
struct CancelClaim(CancellationToken);
impl Drop for CancelClaim {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl AttachmentClaimService {
    pub async fn attachment_reference(
        &self,
        attachment: &ChatAttachment,
    ) -> Result<super::thread_message_intake::AttachmentReference> {
        let path =
            attachment_path(&self.0.directory, attachment).ok_or_else(|| Error::Service {
                operation: "AttachmentClaims.attachment_reference",
                detail: "Attachment is no longer available. Attach it again.".into(),
            })?;
        let delivery = self.0.delivery.clone();
        let path = tokio::task::spawn_blocking(move || delivery.reference(&path))
            .await
            .map_err(|e| claim_error(e.to_string()))?
            .map_err(|e| claim_error(e.to_string()))?;
        let extracted_path = if matches!(attachment, ChatAttachment::File(_)) {
            self.0.documents.ensure(path.clone()).await?
        } else {
            None
        };
        Ok(super::thread_message_intake::AttachmentReference {
            path,
            extracted_path,
        })
    }
    pub fn new(directory: PathBuf, documents: Arc<dyn DocumentText>) -> Self {
        let delivery = Arc::new(t3_workspace::attachment_delivery::AttachmentDelivery::new(
            directory.clone(),
        ));
        Self::with_delivery(
            directory,
            documents,
            Arc::new(NativeCopyFile(delivery.clone())),
            delivery,
        )
    }
    pub fn with_copy_file(
        directory: PathBuf,
        documents: Arc<dyn DocumentText>,
        copy_file: Arc<dyn CopyFile>,
    ) -> Self {
        let delivery = Arc::new(t3_workspace::attachment_delivery::AttachmentDelivery::new(
            directory.clone(),
        ));
        Self::with_delivery(directory, documents, copy_file, delivery)
    }
    pub fn with_delivery(
        directory: PathBuf,
        documents: Arc<dyn DocumentText>,
        copy_file: Arc<dyn CopyFile>,
        delivery: Arc<t3_workspace::attachment_delivery::AttachmentDelivery>,
    ) -> Self {
        Self(Arc::new(ClaimState {
            delivery,
            directory,
            documents,
            copy_file,
            tasks: TaskTracker::new(),
            closed: Mutex::new(false),
        }))
    }
    pub async fn shutdown(&self) {
        {
            let mut closed = self.0.closed.lock().unwrap_or_else(|e| e.into_inner());
            *closed = true;
            self.0.tasks.close();
        }
        self.0.tasks.wait().await;
    }
    async fn claim_batch(
        &self,
        thread: ThreadId,
        attachments: Vec<ChatAttachment>,
        cancellation: &CancellationToken,
    ) -> Result<ClaimedAttachments> {
        let mut paths = Vec::new();
        let result = async {
            let segment = thread_segment(&thread.0).ok_or_else(||claim_error("invalid thread id"))?;
            let mut claimed = Vec::new();
            for mut attachment in attachments {
                if cancellation.is_cancelled() { return Err(Error::Cancelled); }
                let Some(suffix) = pending_suffix(&attachment) else { claimed.push(attachment); continue; };
                let (name, _, expected_size) = properties(&attachment);
                let current = if let Some(extension) = suffix {
                    self.0.directory.join(format!("{}.{}", attachment_id(&attachment), extension.to_ascii_lowercase()))
                } else {
                    ["avif","bmp","gif","heic","heif","ico","jpeg","jpg","png","svg","tiff","webp","bin"].iter()
                        .map(|extension|self.0.directory.join(format!("{}.{extension}",attachment_id(&attachment)))).find(|path|path.exists())
                        .ok_or_else(||claim_error(format!("Attachment '{name}' cannot be sent: attachment not found (removed or expired).")))?
                };
                let metadata = std::fs::metadata(&current).map_err(|_|claim_error(format!("Attachment '{name}' cannot be sent: attachment not found.")))?;
                if metadata.len() != expected_size { return Err(claim_error(format!("Attachment '{name}' cannot be sent: stored size does not match."))); }
                let id = format!("{segment}-{}{}",uuid::Uuid::new_v4(),suffix.map(|suffix|format!("-{suffix}")).unwrap_or_default());
                match &mut attachment {
                    ChatAttachment::Image(value) => { value.id=id; value.mime_type=value.mime_type.to_ascii_lowercase(); }
                    ChatAttachment::File(value) => { value.id=id; value.mime_type=value.mime_type.to_ascii_lowercase(); if value.mime_type == "image/svg+xml" && !value.name.to_ascii_lowercase().ends_with(".svg") {value.name.push_str(".svg");} }
                    ChatAttachment::Unknown(value) => { value.id=id; value.mime_type=value.mime_type.to_ascii_lowercase(); }
                }
                let destination = self.0.directory.join(format!("{}.{}",attachment_id(&attachment),current.extension().and_then(|s|s.to_str()).unwrap_or_default()));
                if attachment_path(&self.0.directory,&attachment).as_ref() != Some(&destination) { return Err(claim_error(format!("Attachment '{}' cannot be sent: attachment type does not match the upload.",properties(&attachment).0))); }
                let from=current.clone(); let to=destination.clone();
                self.0.copy_file.copy(from,to).await?;
                paths.push(destination.clone());
                if cancellation.is_cancelled() { return Err(Error::Cancelled); }
                if sidecar(&current).exists() {
                    let from=sidecar(&current); let to=sidecar(&destination);
                    if self.0.copy_file.copy(from,to.clone()).await.is_ok() { paths.push(to); }
                } else if let Some(extracted) = self.0.documents.ensure(destination).await? { paths.push(extracted); }
                if cancellation.is_cancelled() { return Err(Error::Cancelled); }
                claimed.push(attachment);
            }
            Ok(claimed)
        }.await;
        match result {
            Ok(attachments) => Ok(ClaimedAttachments {
                attachments,
                claimed_paths: paths,
            }),
            Err(error) => {
                self.0.release_paths(&paths);
                Err(error)
            }
        }
    }
}

#[async_trait]
impl super::thread_message_intake::AttachmentReferenceResolver for AttachmentClaimService {
    async fn reference(
        &self,
        attachment: &ChatAttachment,
    ) -> Result<super::thread_message_intake::AttachmentReference> {
        self.attachment_reference(attachment).await
    }
}

#[async_trait]
impl AttachmentBackend for AttachmentClaimService {
    async fn claim_pending(
        &self,
        thread_id: ThreadId,
        attachments: Vec<ChatAttachment>,
    ) -> Result<ClaimedAttachments> {
        validate_attachment_limits(&attachments)?;
        let mut ids = HashSet::new();
        if attachments
            .iter()
            .any(|attachment| !ids.insert(attachment_id(attachment)))
        {
            return Err(claim_error("Duplicate attachment ids are not allowed."));
        }
        if !attachments.iter().any(attachment_is_pending_upload) {
            return Ok(ClaimedAttachments {
                attachments,
                claimed_paths: vec![],
            });
        }
        let guard = CancelClaim(CancellationToken::new());
        let cancellation = guard.0.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        {
            let closed = self.0.closed.lock().unwrap_or_else(|e| e.into_inner());
            if *closed {
                return Err(Error::Cancelled);
            }
            let service = self.clone();
            self.0.tasks.spawn(async move {
                let result = service
                    .claim_batch(thread_id, attachments, &cancellation)
                    .await;
                if let Err(Ok(claimed)) = send.send(result) {
                    service.0.release_paths(&claimed.claimed_paths);
                }
            });
        }
        receive.await.map_err(|e| claim_error(e.to_string()))?
    }
    async fn release_claimed_paths(&self, paths: Vec<PathBuf>) -> Result<()> {
        self.0.release_paths(&paths);
        Ok(())
    }
    async fn resolve(
        &self,
        _thread: ThreadId,
        attachments: Vec<ChatAttachment>,
    ) -> Result<Vec<ChatAttachment>> {
        Ok(attachments)
    }
    async fn claim(&self, _thread: ThreadId, _ids: Vec<String>) -> Result<()> {
        Err(Error::Unsupported(
            "AttachmentClaims.claim requires attachment metadata",
        ))
    }
    async fn release(&self, _thread: ThreadId, ids: Vec<String>) -> Result<()> {
        let mut paths = Vec::new();
        for id in ids {
            if !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            {
                return Err(claim_error("invalid attachment id"));
            }
            for extension in [
                "avif", "bmp", "gif", "heic", "heif", "ico", "jpeg", "jpg", "png", "svg", "tiff",
                "webp", "bin",
            ] {
                paths.push(self.0.directory.join(format!("{id}.{extension}")));
            }
        }
        self.0.release_paths(&paths);
        Ok(())
    }
}
