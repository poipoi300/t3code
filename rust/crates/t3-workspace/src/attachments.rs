use crate::{Error, Result, io_error, paths};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};
use t3_contracts::{
    assets::{AttachmentCreateUploadUrlInput, AttachmentCreateUploadUrlResult},
    base_schemas::TrimmedNonEmptyString as Text,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

pub const SAFE_IMAGE_EXTENSIONS: &[&str] = &[
    ".avif", ".bmp", ".gif", ".heic", ".heif", ".ico", ".jpeg", ".jpg", ".png", ".svg", ".tiff",
    ".webp",
];
pub const IMAGE_MAX_BYTES: u64 = 10 * 1024 * 1024;
pub const FILE_MAX_BYTES: u64 = 50 * 1024 * 1024;
pub const UPLOAD_TTL_MS: u64 = 10 * 60_000;

fn id_pattern() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| match regex::Regex::new(r"(?i)^([a-z0-9_]+(?:-[a-z0-9_]+)*)-([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})(?:-([a-z0-9]{1,10}))?$") {
        Ok(re) => re,
        Err(_) => unreachable!("constant attachment pattern"),
    })
}
pub fn safe_thread_segment(thread: &str) -> Option<String> {
    let mut s = String::new();
    for ch in thread.trim().to_lowercase().chars() {
        let ch = if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            ch
        } else {
            '-'
        };
        if ch != '-' || !s.ends_with('-') {
            s.push(ch);
        }
    }
    let s = s.trim_matches(['-', '_']);
    let s = s[..s.len().min(80)].trim_end_matches(['-', '_']);
    if s.is_empty() {
        None
    } else if s == "pending" {
        Some("_pending".into())
    } else {
        Some(s.into())
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAttachmentId {
    pub thread_segment: String,
    pub uuid: String,
    pub extension: Option<String>,
}
pub fn parse_id(id: &str) -> Option<ParsedAttachmentId> {
    if id.len() > 256 || id_pattern().captures(id).is_none() {
        return None;
    }
    let normalized = normalize_relative_path(id)?;
    if normalized.contains(['/', '.']) {
        return None;
    }
    let captures = id_pattern().captures(&normalized)?;
    Some(ParsedAttachmentId {
        thread_segment: captures[1].to_ascii_lowercase(),
        uuid: captures[2].to_ascii_lowercase(),
        extension: captures.get(3).map(|s| s.as_str().to_ascii_lowercase()),
    })
}
fn extension_suffix(extension: Option<&str>) -> String {
    extension
        .filter(|s| !s.is_empty())
        .map(|s| {
            let s = s.strip_prefix('.').unwrap_or(s).to_ascii_lowercase();
            if !s.is_empty() && s.len() <= 10 && s.bytes().all(|c| c.is_ascii_alphanumeric()) {
                format!("-{s}")
            } else {
                "-bin".into()
            }
        })
        .unwrap_or_default()
}
pub fn create_id(thread: &str, extension: Option<&str>) -> Option<String> {
    Some(format!(
        "{}-{}{}",
        safe_thread_segment(thread)?,
        uuid::Uuid::new_v4(),
        extension_suffix(extension)
    ))
}
pub fn pending_id(extension: Option<&str>) -> String {
    format!(
        "pending-{}{}",
        uuid::Uuid::new_v4(),
        extension_suffix(extension)
    )
}
pub fn deterministic_id(thread: &str, stable_key: &str) -> Option<String> {
    let json = serde_json::to_vec(&[thread, stable_key]).ok()?;
    let hash = hex(&Sha256::digest(json));
    Some(format!(
        "{}-{}-{}-{}-{}-{}",
        safe_thread_segment(thread)?,
        &hash[..8],
        &hash[8..12],
        &hash[12..16],
        &hash[16..20],
        &hash[20..32]
    ))
}
pub fn stable_key(message_id: &str, index: usize, bytes: &[u8]) -> String {
    format!("{message_id}:{index}:{}", hex(&Sha256::digest(bytes)))
}
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn file_extension(name: &str) -> String {
    let ext = Path::new(name)
        .extension()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if ext == "part"
        || ext.is_empty()
        || ext.len() > 10
        || !ext.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        ".bin".into()
    } else {
        format!(".{ext}")
    }
}
pub fn image_extension(mime: &str, name: &str) -> String {
    match mime.to_ascii_lowercase().as_str() {
        "image/avif" => ".avif",
        "image/bmp" => ".bmp",
        "image/gif" => ".gif",
        "image/heic" => ".heic",
        "image/heif" => ".heif",
        "image/jpeg" | "image/jpg" => ".jpg",
        "image/png" => ".png",
        "image/svg+xml" => ".svg",
        "image/tiff" => ".tiff",
        "image/webp" => ".webp",
        "image/x-icon" | "image/vnd.microsoft.icon" => ".ico",
        _ => {
            let ext = file_extension(name.trim());
            return if SAFE_IMAGE_EXTENSIONS.contains(&ext.as_str()) {
                ext
            } else {
                ".bin".into()
            };
        }
    }
    .into()
}
pub fn normalize_relative_path(raw: &str) -> Option<String> {
    if raw.contains('\0') {
        return None;
    }
    // Keep parent traversal visible instead of clamping it against an artificial root.
    let separators = if cfg!(windows) {
        raw.replace('\\', "/")
    } else {
        raw.into()
    };
    let mut parts = Vec::new();
    for part in separators.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    let normalized = parts.join("/").replace('\\', "/");
    if normalized.is_empty() || normalized.starts_with("..") {
        None
    } else {
        Some(normalized)
    }
}
pub fn resolve_relative(base: &Path, relative: &str) -> Option<PathBuf> {
    let relative = normalize_relative_path(relative)?;
    let base = paths::absolute(base).ok()?;
    let file = paths::absolute(&base.join(relative)).ok()?;
    (file != base && paths::within(&file, &base)).then_some(file)
}
pub fn resolve_by_id(base: &Path, id: &str) -> Option<PathBuf> {
    let normalized = normalize_relative_path(id)?;
    if normalized.contains(['/', '.']) {
        return None;
    }
    let parsed = parse_id(&normalized)?;
    if let Some(extension) = parsed.extension {
        let p = resolve_relative(base, &format!("{normalized}.{extension}"))?;
        return p.exists().then_some(p);
    }
    for extension in SAFE_IMAGE_EXTENSIONS.iter().copied().chain([".bin"]) {
        let p = resolve_relative(base, &format!("{normalized}{extension}"))?;
        if p.exists() {
            return Some(p);
        }
    }
    None
}
#[derive(Debug, Clone)]
pub struct ClaimPlan {
    pub final_id: String,
    pub current_path: PathBuf,
    pub final_path: PathBuf,
}
pub fn plan_claim(base: &Path, thread: &str, id: &str) -> Result<ClaimPlan> {
    let parsed = parse_id(id).ok_or_else(|| Error::Invalid("invalid attachment id".into()))?;
    if safe_thread_segment(thread).is_none() {
        return Err(Error::Invalid("invalid thread id".into()));
    }
    if parsed.thread_segment != "pending" {
        return Err(Error::Invalid("attachment must be a pending upload".into()));
    }
    let current_path = resolve_by_id(base, id)
        .ok_or_else(|| Error::Invalid("attachment not found (removed or expired)".into()))?;
    let final_id = create_id(thread, parsed.extension.as_deref())
        .ok_or_else(|| Error::Invalid("invalid thread id".into()))?;
    let ext = current_path
        .extension()
        .unwrap_or_default()
        .to_string_lossy();
    let final_path =
        resolve_relative(base, &format!("{final_id}.{ext}")).ok_or(Error::AssetDenied)?;
    Ok(ClaimPlan {
        final_id,
        current_path,
        final_path,
    })
}
pub fn sweep_stale(base: &Path, now_ms: u64) -> usize {
    let Ok(scope) = crate::scoped::ScopedDir::open(base) else {
        return 0;
    };
    let Ok((_held, entries)) = scope.read_dir(Path::new("")) else {
        return 0;
    };
    let mut deleted = 0;
    for child in entries.flatten() {
        let file_name = child.file_name();
        let Some(name) = file_name.to_str().map(str::to_owned) else {
            continue;
        };
        let partial = name.ends_with(".part");
        if !partial {
            let name = if name.ends_with(".txt") && name[..name.len() - 4].contains('.') {
                &name[..name.len() - 4]
            } else {
                &name
            };
            let Some((id, _)) = name.rsplit_once('.') else {
                continue;
            };
            if !parse_id(id).is_some_and(|p| p.thread_segment == "pending") {
                continue;
            }
        }
        let Some(path) = resolve_relative(base, &name) else {
            continue;
        };
        let Some(file_name) = path.file_name() else {
            continue;
        };
        let Ok(file) = scope.open_file(Path::new(file_name)) else {
            continue;
        };
        let Ok(modified) = file.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        let mtime = modified
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let ttl = if partial {
            60 * 60_000
        } else {
            24 * 60 * 60_000
        };
        #[cfg(test)]
        SWEEP_BEFORE_REMOVE.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook();
            }
        });
        drop(file);
        if now_ms.saturating_sub(mtime) > ttl && scope.remove_file(Path::new(file_name)).is_ok() {
            deleted += 1;
        }
    }
    deleted
}

#[cfg(test)]
thread_local! {
    static SWEEP_BEFORE_REMOVE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = Default::default();
}

#[cfg(test)]
mod sweep_tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_swapped_attachment_alias_cannot_redirect_stale_deletion() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("real");
        let outside = temp.path().join("outside");
        let alias = temp.path().join("alias");
        fs::create_dir(&root).expect("root");
        fs::create_dir(&outside).expect("outside");
        fn link(target: &Path, alias: &Path) -> io::Result<()> {
            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_dir(target, alias)
            }
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(target, alias)
            }
        }
        link(&root, &alias).expect("alias");
        fs::write(root.join("upload.part"), "old partial").expect("partial");
        fs::write(outside.join("upload.part"), "outside").expect("outside file");
        let swapped_alias = alias.clone();
        let swapped_outside = outside.clone();
        SWEEP_BEFORE_REMOVE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                #[cfg(windows)]
                fs::remove_dir(&swapped_alias).expect("remove alias");
                #[cfg(unix)]
                fs::remove_file(&swapped_alias).expect("remove alias");
                link(&swapped_outside, &swapped_alias).expect("swap alias");
            }));
        });
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis() as u64
            + 60 * 60_000
            + 1;
        assert_eq!(sweep_stale(&alias, now), 1);
        assert_eq!(
            fs::read_to_string(outside.join("upload.part")).expect("outside preserved"),
            "outside"
        );
        assert!(!root.join("upload.part").exists());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadClaims {
    pub version: u8,
    pub kind: String,
    #[serde(rename = "type", default = "default_image")]
    pub attachment_type: String,
    pub attachment_id: String,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub expires_at: u64,
}
fn default_image() -> String {
    "image".into()
}

pub trait DocumentText: Send + Sync {
    fn ensure<'a>(&'a self, path: &'a Path) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}
#[derive(Clone)]
pub struct AttachmentStore {
    pub base: PathBuf,
    pub signer: crate::assets::AssetSigner,
    pub documents: Arc<dyn DocumentText>,
    last_sweep: Arc<tokio::sync::Mutex<Option<u64>>>,
}
impl AttachmentStore {
    pub fn new(
        base: PathBuf,
        signer: crate::assets::AssetSigner,
        documents: Arc<dyn DocumentText>,
    ) -> Self {
        Self {
            base,
            signer,
            documents,
            last_sweep: Default::default(),
        }
    }
    pub async fn persist_chat_attachments(
        &self,
        input: t3_contracts::chat_attachment::PersistChatAttachmentsInput,
    ) -> Result<t3_contracts::chat_attachment::PersistChatAttachmentsResult> {
        use t3_contracts::chat_attachment::{
            ChatAttachment, ChatImageAttachment, ChatImageAttachmentType,
            PersistChatAttachmentsResult,
        };
        let mut attachments = Vec::new();
        tokio::fs::create_dir_all(&self.base)
            .await
            .map_err(|e| io_error("make-directory", &self.base, e))?;
        for (index, attachment) in input.attachments.into_iter().enumerate() {
            let (mime, bytes) = parse_base64_data_url(&attachment.data_url).ok_or_else(|| {
                Error::Invalid(format!(
                    "Attachment {} has an invalid image payload.",
                    attachment.name
                ))
            })?;
            if mime != attachment.mime_type.to_lowercase()
                || bytes.len() as u64 != attachment.size_bytes
                || attachment.size_bytes > IMAGE_MAX_BYTES
            {
                return Err(Error::Invalid(format!(
                    "Attachment {} size or MIME does not match its payload.",
                    attachment.name
                )));
            }
            let id = deterministic_id(
                &input.thread_id.0,
                &stable_key(&input.message_id.0, index, &bytes),
            )
            .ok_or_else(|| Error::Invalid("Could not allocate an attachment identifier.".into()))?;
            let extension = image_extension(&attachment.mime_type, &attachment.name);
            let path = resolve_relative(&self.base, &format!("{id}{extension}"))
                .ok_or(Error::AssetDenied)?;
            let base = self.base.clone();
            crate::blocking(move || {
                let scope = crate::scoped::ScopedDir::open(&base)?;
                let relative = Path::new(path.file_name().ok_or(Error::AssetDenied)?);
                scope.write_atomic(relative, &bytes)
            })
            .await?;
            let id = serde_json::from_value(serde_json::Value::String(id))
                .map_err(|e| Error::Invalid(e.to_string()))?;
            attachments.push(ChatAttachment::Image(Box::new(ChatImageAttachment {
                r#type: ChatImageAttachmentType::Image,
                id,
                name: attachment.name,
                mime_type: attachment.mime_type,
                size_bytes: attachment.size_bytes,
                source: None,
            })));
        }
        Ok(PersistChatAttachmentsResult { attachments })
    }
    pub async fn issue_upload(
        &self,
        input: AttachmentCreateUploadUrlInput,
        now_ms: u64,
    ) -> Result<AttachmentCreateUploadUrlResult> {
        let (kind, name, mime, size) = match input {
            AttachmentCreateUploadUrlInput::Image(i) => (
                "image",
                i.name.0,
                match i.mime_type {
                    t3_contracts::assets::UploadImageMimeType::Gif => "image/gif",
                    t3_contracts::assets::UploadImageMimeType::Jpeg => "image/jpeg",
                    t3_contracts::assets::UploadImageMimeType::Png => "image/png",
                    t3_contracts::assets::UploadImageMimeType::Webp => "image/webp",
                }
                .to_owned(),
                i.size_bytes,
            ),
            AttachmentCreateUploadUrlInput::File(i) => {
                ("file", i.name.0, i.mime_type.0, i.size_bytes)
            }
        };
        let name = if kind == "file"
            && (mime.eq_ignore_ascii_case("image/svg+xml") || name.to_lowercase().ends_with(".svg"))
            && !name.to_lowercase().ends_with(".svg")
        {
            format!("{name}.svg")
        } else {
            name
        };
        if name.trim().is_empty()
            || name.encode_utf16().count() > 255
            || mime.trim().is_empty()
            || mime.encode_utf16().count() > 100
            || size == 0
            || size
                > if kind == "image" {
                    IMAGE_MAX_BYTES
                } else {
                    FILE_MAX_BYTES
                }
            || (kind == "image"
                && !["image/gif", "image/jpeg", "image/png", "image/webp"].contains(&mime.as_str()))
        {
            return Err(Error::Invalid("invalid attachment upload metadata".into()));
        }
        let mut last = self.last_sweep.lock().await;
        if last.is_none_or(|last| now_ms.saturating_sub(last) >= 15 * 60_000) {
            *last = Some(now_ms);
            let base = self.base.clone();
            crate::blocking(move || Ok(sweep_stale(&base, now_ms))).await?;
        }
        drop(last);
        let extension = (kind == "file").then(|| file_extension(&name));
        let id = pending_id(extension.as_deref());
        let expires_at = now_ms + UPLOAD_TTL_MS;
        let claims = UploadClaims {
            version: 1,
            kind: "attachment-upload".into(),
            attachment_type: kind.into(),
            attachment_id: id.clone(),
            name,
            mime_type: mime,
            size_bytes: size,
            expires_at,
        };
        let token = self.signer.sign_json(&claims)?;
        Ok(AttachmentCreateUploadUrlResult {
            attachment_id: Text(id),
            relative_url: Text(format!("/api/attachments/upload/{token}")),
            expires_at: serde_json::from_value(expires_at.into())
                .map_err(|e| Error::Invalid(e.to_string()))?,
        })
    }
    pub fn validate_upload(&self, token: &str, now_ms: u64) -> Option<UploadClaims> {
        let claims: UploadClaims = self.signer.verify_json(token)?;
        (claims.version == 1
            && claims.kind == "attachment-upload"
            && ["image", "file"].contains(&claims.attachment_type.as_str())
            && claims.expires_at > now_ms)
            .then_some(claims)
    }
    /// The token, rather than caller-supplied claims, authorizes the immutable publication.
    pub async fn upload(
        &self,
        token: &str,
        now_ms: u64,
        mut body: impl AsyncRead + Unpin,
    ) -> Result<PathBuf> {
        let claims = self
            .validate_upload(token, now_ms)
            .ok_or(Error::AssetDenied)?;
        let extension = if claims.attachment_type == "file" {
            file_extension(&claims.name)
        } else {
            image_extension(&claims.mime_type, &claims.name)
        };
        let relative = format!("{}{extension}", claims.attachment_id);
        let final_path = resolve_relative(&self.base, &relative).ok_or(Error::AssetDenied)?;
        let part_path = resolve_relative(
            &self.base,
            &format!("{relative}.{}.part", uuid::Uuid::new_v4()),
        )
        .ok_or(Error::AssetDenied)?;
        tokio::fs::create_dir_all(&self.base)
            .await
            .map_err(|e| io_error("make-directory", &self.base, e))?;
        let scope = Arc::new(crate::scoped::ScopedDir::open(&self.base)?);
        let part_name = PathBuf::from(part_path.file_name().ok_or(Error::AssetDenied)?);
        let final_name = PathBuf::from(final_path.file_name().ok_or(Error::AssetDenied)?);
        let _cleanup = ScopedPartFile {
            scope: scope.clone(),
            name: part_name.clone(),
        };
        let mut options = cap_std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        let file = scope
            .dir
            .open_with(&part_name, &options)
            .map_err(|e| io_error("open", &part_path, e))?
            .into_std();
        scope.check_file(&file, &part_name)?;
        let mut part = tokio::fs::File::from_std(file);
        let mut received = 0u64;
        let mut hash = Sha256::new();
        let mut buf = [0; 64 * 1024];
        loop {
            let count = body
                .read(&mut buf)
                .await
                .map_err(|e| io_error("read-upload", &part_path, e))?;
            if count == 0 {
                break;
            }
            received += count as u64;
            if received > claims.size_bytes {
                break;
            }
            part.write_all(&buf[..count])
                .await
                .map_err(|e| io_error("write-upload", &part_path, e))?;
            hash.update(&buf[..count]);
        }
        if received != claims.size_bytes {
            return Err(Error::Upload {
                status: 400,
                detail: format!("Body was {received} bytes, expected {}.", claims.size_bytes),
            });
        }
        part.sync_all()
            .await
            .map_err(|e| io_error("sync-upload", &part_path, e))?;
        drop(part);
        let pinned = scope.clone();
        let staging = part_name.clone();
        let name = final_name.clone();
        let published =
            crate::blocking(
                move || match pinned.dir.hard_link(&staging, &pinned.dir, &name) {
                    Ok(()) => Ok(true),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
                    Err(error) => Err(io_error("publish-upload", &name, error)),
                },
            )
            .await?;
        if !published {
            let pinned = scope.clone();
            let name = final_name.clone();
            let digest = crate::blocking(move || {
                let mut existing = pinned.open_file(&name)?;
                let mut digest = Sha256::new();
                let mut buf = [0; 64 * 1024];
                let mut total = 0;
                loop {
                    let n = std::io::Read::read(&mut existing, &mut buf)
                        .map_err(|e| io_error("read-upload", &name, e))?;
                    if n == 0 {
                        break;
                    }
                    total += n as u64;
                    if total > FILE_MAX_BYTES {
                        return Err(Error::AssetDenied);
                    }
                    digest.update(&buf[..n]);
                }
                Ok(digest.finalize())
            })
            .await?;
            if digest != hash.finalize() {
                return Err(Error::Upload {
                    status: 409,
                    detail: "Attachment already uploaded.".into(),
                });
            }
        }
        scope.remove_file(&part_name)?;
        if paths::canonicalize(&self.base)
            .map_err(|e| io_error("realpath-attachments", &self.base, e))?
            != scope.root
        {
            return Err(Error::AssetDenied);
        }
        self.documents.ensure(&final_path).await;
        Ok(final_path)
    }
    pub async fn delete_pending(&self, id: &str) -> Result<()> {
        if !parse_id(id).is_some_and(|p| p.thread_segment == "pending") {
            return Ok(());
        }
        let base = self.base.clone();
        let id = id.to_owned();
        crate::blocking(move || {
            let Some(path) = resolve_by_id(&base, &id) else {
                return Ok(());
            };
            let scope = crate::scoped::ScopedDir::open(&base)?;
            let relative = Path::new(path.file_name().ok_or(Error::AssetDenied)?);
            scope.remove_file(relative)?;
            let _ = scope.remove_file(Path::new(&format!("{}.txt", relative.to_string_lossy())));
            Ok(())
        })
        .await?;
        Ok(())
    }
}
struct ScopedPartFile {
    scope: Arc<crate::scoped::ScopedDir>,
    name: PathBuf,
}
impl Drop for ScopedPartFile {
    fn drop(&mut self) {
        let _ = self.scope.remove_file(&self.name);
    }
}

pub fn parse_base64_data_url(value: &str) -> Option<(String, Vec<u8>)> {
    let (header, payload) = value.trim().split_once(',')?;
    if !header.get(..5)?.eq_ignore_ascii_case("data:") {
        return None;
    }
    let parts: Vec<_> = header[5..]
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() < 2 || !parts.last()?.eq_ignore_ascii_case("base64") {
        return None;
    }
    let compact: String = payload
        .chars()
        .filter(|c| !['\r', '\n', ' '].contains(c))
        .collect();
    if compact.is_empty() || !compact.len().is_multiple_of(4) {
        return None;
    }
    Some((
        parts[0].to_ascii_lowercase(),
        STANDARD.decode(compact).ok()?,
    ))
}

pub fn encoded_payload<T: Serialize>(value: &T) -> Result<String> {
    Ok(URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(value).map_err(|e| Error::Invalid(e.to_string()))?))
}
