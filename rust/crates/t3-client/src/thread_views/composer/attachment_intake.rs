//! Port of web composerAttachmentFiles.ts, composerImagePersistence.ts and composerDraftUploads.ts.

use super::{send::Changes, web_content, web_drafts};
use crate::{
    ClientError,
    operations::{SendAttachment, persist_attachments},
    platform::{HostResource, Platform},
    rpc::{RpcEndpoint, SessionEndpoint},
    services::assets::{self, AttachmentUploadResult},
    types::{EnvironmentId, ThreadId},
};
use base64::Engine;
use futures::stream::LocalBoxStream;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};
use t3_contracts::{base_schemas::MessageId, chat_attachment::*};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Image,
    File,
    UnsupportedImage,
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map_or_else(String::new, |(_, ext)| ext.to_ascii_lowercase())
}

fn dng(name: &str, mime: &str) -> bool {
    extension(name) == "dng"
        || matches!(
            mime.split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "image/dng" | "image/x-dng" | "image/x-adobe-dng"
        )
}

fn svg(name: &str, mime: &str) -> bool {
    extension(name) == "svg"
        || mime
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .eq_ignore_ascii_case("image/svg+xml")
}

pub fn image_mime(name: &str, mime: &str) -> String {
    if !mime.is_empty() && !mime.eq_ignore_ascii_case("application/octet-stream") {
        return mime.to_owned();
    }
    match extension(name).as_str() {
        "gif" => "image/gif",
        "jpeg" | "jpg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        _ => return mime.to_owned(),
    }
    .into()
}

pub fn classify(name: &str, mime: &str) -> FileKind {
    if svg(name, mime) || dng(name, mime) {
        return FileKind::File;
    }
    let normalized = image_mime(name, mime).to_ascii_lowercase();
    if matches!(normalized.as_str(), "image/heic" | "image/heif")
        || ((mime.is_empty() || mime.eq_ignore_ascii_case("application/octet-stream"))
            && matches!(extension(name).as_str(), "heic" | "heif"))
        || matches!(
            normalized.as_str(),
            "image/gif" | "image/jpeg" | "image/png" | "image/webp"
        )
    {
        return FileKind::Image;
    }
    if !normalized.starts_with("image/")
        || matches!(
            extension(name).as_str(),
            "avif"
                | "bmp"
                | "ico"
                | "tif"
                | "tiff"
                | "svg"
                | "heic"
                | "heif"
                | "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
        )
    {
        FileKind::File
    } else {
        FileKind::UnsupportedImage
    }
}

pub fn file_mime(name: &str, mime: &str) -> String {
    if dng(name, mime) {
        return "image/x-adobe-dng".into();
    }
    if svg(name, mime) {
        return "image/svg+xml".into();
    }
    let normalized = mime
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if normalized.starts_with("video/")
        && !matches!(
            normalized.as_str(),
            "video/mp2t" | "video/vnd.dlna.mpeg-tts"
        )
    {
        return normalized;
    }
    if matches!(
        normalized.as_str(),
        "" | "application/octet-stream"
            | "binary/octet-stream"
            | "application/unknown"
            | "video/mp2t"
            | "video/vnd.dlna.mpeg-tts"
    ) {
        let inferred = match extension(name).as_str() {
            "avi" => Some("video/x-msvideo"),
            "m4v" | "mp4" => Some("video/mp4"),
            "mkv" => Some("video/x-matroska"),
            "mov" => Some("video/quicktime"),
            "ogv" => Some("video/ogg"),
            "webm" => Some("video/webm"),
            _ => None,
        };
        if let Some(mime) = inferred {
            return mime.into();
        }
    }
    if mime.is_empty() {
        "application/octet-stream".into()
    } else {
        mime.into()
    }
}

pub fn should_handle_paste(files: &[HostResource], plain_text: &str) -> bool {
    files.iter().any(|file| {
        dng(&file.name, &file.mime)
            || svg(&file.name, &file.mime)
            || matches!(
                classify(&file.name, &file.mime),
                FileKind::Image | FileKind::UnsupportedImage
            )
    }) || (plain_text.is_empty()
        && files
            .iter()
            .any(|file| classify(&file.name, &file.mime) == FileKind::File))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub known: bool,
    pub uploads: bool,
    pub max_file_bytes: Option<u64>,
}
impl Capabilities {
    pub fn staging_limit(&self) -> Option<u64> {
        if !self.known {
            Some(PROVIDER_SEND_TURN_MAX_FILE_BYTES)
        } else if self.uploads {
            self.max_file_bytes
                .map(assets::clamp_file_attachment_upload_bytes)
        } else {
            None
        }
    }
    pub fn block_reason(&self, files: &[Value]) -> Option<String> {
        if files.is_empty() {
            return None;
        }
        if !self.known {
            return Some("Waiting for the server before file attachments can send".into());
        }
        let Some(limit) = self.staging_limit() else {
            return Some(
                "This server does not accept file attachments right now. Remove the files to send."
                    .into(),
            );
        };
        files
            .iter()
            .find(|file| file["sizeBytes"].as_u64().is_some_and(|size| size > limit))
            .map(|file| {
                assets::file_attachment_too_large_message(
                    file["name"].as_str().unwrap_or_default(),
                    limit,
                )
            })
    }
}

pub fn uploads_to_release(attachments: &[Value]) -> Vec<Value> {
    attachments
        .iter()
        .filter(|a| !(a["type"] == "file" && a.get("uploadedAttachmentId").is_some()))
        .cloned()
        .collect()
}
pub fn uploaded_attachment(value: &Value) -> Result<ChatAttachment, ClientError> {
    Ok(serde_json::from_value(
        json!({"type":value["type"],"id":value["uploadedAttachmentId"],"name":value["name"],"mimeType":value["mimeType"],"sizeBytes":value["sizeBytes"]}),
    )?)
}

pub fn release_composer_draft_uploads(
    state: &Value,
    key: &str,
    cleanup: &super::cleanup::AttachmentCleanup,
) {
    let draft = &state["draftsByThreadKey"][key];
    for field in ["images", "files"] {
        if let Some(attachments) = draft[field].as_array() {
            cleanup.schedule(attachments);
        }
    }
}

pub fn release_project_draft_uploads(
    state: &Value,
    environment: &EnvironmentId,
    project_id: &str,
    thread_keys: &[String],
    cleanup: &super::cleanup::AttachmentCleanup,
) {
    if let Some(sessions) = state["draftThreadsByThreadKey"].as_object() {
        for (key, session) in sessions {
            if session["environmentId"] == environment.0 && session["projectId"] == project_id {
                release_composer_draft_uploads(state, key, cleanup);
            }
        }
    }
    for key in thread_keys {
        release_composer_draft_uploads(state, key, cleanup);
    }
}

/// Cached data is used even when the original host resource no longer exists.
pub async fn serialize_composer_images(
    platform: &dyn Platform,
    images: &[Value],
    persisted: &[Value],
) -> Vec<Value> {
    futures::future::join_all(images.iter().map(|image| async {
        let cached = persisted.iter().find(|old| old["id"] == image["id"]);
        let data = if let Some(data) = cached.and_then(|old| old["dataUrl"].as_str()).or_else(|| image["dataUrl"].as_str()) {
            data.to_owned()
        } else {
            let handle = image["fileUri"].as_str()?;
            let resource = HostResource { handle: handle.into(), name: image["name"].as_str()?.into(), mime: image["mimeType"].as_str()?.into() };
            match crate::view_logic::chat_view::read_file_as_data_url(platform, &resource, PROVIDER_SEND_TURN_MAX_IMAGE_BYTES as usize).await {
                Ok(data) => data,
                Err(_) => return cached.cloned(),
            }
        };
        let mut result = json!({"id":image["id"],"name":image["name"],"mimeType":image["mimeType"],"sizeBytes":image["sizeBytes"],"dataUrl":data});
        if let Some(source) = image.get("source") { result["source"] = source.clone(); }
        Some(result)
    })).await.into_iter().flatten().collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UploadState {
    #[default]
    Idle,
    Preparing,
    Uploading,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct AttachmentItem {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub image: bool,
    pub video: bool,
    pub preview: Option<String>,
    pub needs_reattach: bool,
    pub non_persisted: bool,
    pub upload: UploadState,
}

#[derive(Clone)]
pub struct AttachmentConnection {
    pub endpoint: SessionEndpoint,
    pub base_url: String,
}

#[derive(Clone, Debug)]
pub struct IncomingFile {
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
    pub source: Option<Value>,
    pub insertion: Option<(String, usize, usize)>,
}

enum Source {
    Resource(HostResource),
    Bytes(IncomingFile),
}

struct PreparingBatch {
    session: Rc<Session>,
    ids: Vec<String>,
}
impl Drop for PreparingBatch {
    fn drop(&mut self) {
        self.session
            .preparing
            .borrow_mut()
            .retain(|item| !self.ids.contains(&item.id));
        self.session.changed();
    }
}
impl Source {
    fn metadata(&self) -> HostResource {
        match self {
            Self::Resource(file) => file.clone(),
            Self::Bytes(file) => HostResource {
                handle: String::new(),
                name: file.name.clone(),
                mime: file.mime.clone(),
            },
        }
    }
}

pub fn resource_from_uri(uri: &str) -> Option<HostResource> {
    let url = url::Url::parse(uri).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    let name = url.path_segments()?.next_back()?;
    let encoded = format!("name={}", name.replace('+', "%2B"));
    let name = url::form_urlencoded::parse(encoded.as_bytes())
        .next()?
        .1
        .into_owned();
    Some(HostResource {
        handle: uri.into(),
        name,
        mime: String::new(),
    })
}

pub(crate) trait AttachmentDraftOwner {
    fn writable(&self) -> bool;
    fn changed(&self);
}
struct Session {
    platform: Rc<dyn Platform>,
    drafts: Rc<RefCell<Value>>,
    key: String,
    environment: EnvironmentId,
    capabilities: RefCell<Capabilities>,
    connection: RefCell<Option<AttachmentConnection>>,
    uploads: RefCell<BTreeMap<String, UploadState>>,
    preparing: RefCell<Vec<AttachmentItem>>,
    rewind_slots: Cell<usize>,
    uploaded: RefCell<BTreeMap<String, String>>,
    generation: Cell<u64>,
    cleanup: Rc<super::cleanup::AttachmentCleanup>,
    pasted_names: RefCell<std::collections::BTreeSet<String>>,
    changes: Changes,
    owner: RefCell<Option<Rc<dyn AttachmentDraftOwner>>>,
}
impl Session {
    fn writable(&self) -> bool {
        self.owner
            .borrow()
            .as_ref()
            .is_none_or(|owner| owner.writable())
    }
    fn changed(&self) {
        if let Some(owner) = self.owner.borrow().as_ref() {
            owner.changed();
        }
        self.changes.notify();
    }
}

/// Holds room for the restored files until the rollback is projected and the draft commits.
pub(crate) struct RewindReservation {
    session: Rc<Session>,
    count: usize,
    images: Vec<Value>,
    files: Vec<Value>,
}

impl Drop for RewindReservation {
    fn drop(&mut self) {
        self.session
            .rewind_slots
            .set(self.session.rewind_slots.get() - self.count);
    }
}

impl RewindReservation {
    pub(crate) fn prepare(
        &mut self,
        original: &ChatAttachment,
        file: crate::view_logic::chat_view::RestoredAttachment,
    ) -> Result<(), ClientError> {
        let image = matches!(original, ChatAttachment::Image(_));
        let id = crate::operations::allocate_command_id(self.session.platform.as_ref(), None)?.0;
        let size = file.bytes.len();
        let mime = file.mime_type;
        let data_url = format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(file.bytes)
        );
        let mut draft = json!({
            "id": id, "name": file.name, "mimeType": mime, "sizeBytes": size, "dataUrl": data_url,
        });
        if image {
            draft["type"] = json!("image");
            draft["previewUrl"] = draft["dataUrl"].clone();
            self.images.push(draft);
        } else {
            draft["type"] = json!("file");
            draft["fileUri"] = json!("");
            draft["restoredFromRewind"] = json!(true);
            self.files.push(draft);
        }
        Ok(())
    }
}

impl crate::thread_views::thread_ops::rewind::PreparedRewindAttachments for RewindReservation {
    fn commit(&self, document: &mut Value, draft_key: &str) -> Result<(), ClientError> {
        let images = web_content::add_images(document, draft_key, &self.images, true);
        // The live and reloaded composer must see the same restored images.
        if !self.images.is_empty() {
            let draft = &document["draftsByThreadKey"][draft_key];
            let mut saved = draft["persistedAttachments"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            saved.extend(
                self.images
                    .iter()
                    .filter(|image| images.accepted_ids.iter().any(|id| image["id"] == *id))
                    .cloned(),
            );
            web_content::sync_persisted_attachments(document, draft_key, &saved);
        }
        web_content::add_files(document, draft_key, &self.files, true, false);
        Ok(())
    }
}

/// One draft's host intake. The shared draft document is owned by the client's draft binding.
#[derive(Clone)]
pub struct AttachmentSession(Rc<Session>);

impl AttachmentSession {
    pub fn new(
        platform: Rc<dyn Platform>,
        drafts: Rc<RefCell<Value>>,
        key: String,
        environment: EnvironmentId,
    ) -> Self {
        Self(Rc::new(Session {
            platform,
            drafts,
            key,
            environment,
            capabilities: RefCell::new(Capabilities::default()),
            connection: RefCell::new(None),
            uploads: RefCell::new(BTreeMap::new()),
            preparing: RefCell::new(Vec::new()),
            rewind_slots: Cell::default(),
            uploaded: RefCell::new(BTreeMap::new()),
            generation: Cell::new(0),
            cleanup: Rc::new(super::cleanup::AttachmentCleanup::default()),
            pasted_names: RefCell::new(std::collections::BTreeSet::new()),
            changes: Changes::default(),
            owner: RefCell::new(None),
        }))
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        self.0.changes.subscribe()
    }
    pub fn refresh(&self) {
        self.0.changes.notify();
    }
    pub(crate) fn bind_draft_owner(&self, owner: Rc<dyn AttachmentDraftOwner>) {
        *self.0.owner.borrow_mut() = Some(owner);
    }
    fn changed(&self) {
        self.0.changed();
    }
    pub fn cleanup(&self) -> Rc<super::cleanup::AttachmentCleanup> {
        self.0.cleanup.clone()
    }
    pub fn set_connection(
        &self,
        capabilities: Capabilities,
        connection: Option<AttachmentConnection>,
    ) {
        self.0
            .generation
            .set(self.0.generation.get().wrapping_add(1));
        if capabilities.known && !capabilities.uploads && self.0.writable() {
            let mut drafts = self.0.drafts.borrow_mut();
            if let Some(images) = drafts
                .get_mut("draftsByThreadKey")
                .and_then(|drafts| drafts.get_mut(&self.0.key))
                .and_then(|draft| draft.get_mut("images"))
                .and_then(Value::as_array_mut)
            {
                for image in images {
                    let id = image["id"].as_str().unwrap_or_default().to_owned();
                    if let Some(uploaded) = self.0.uploaded.borrow_mut().remove(&id) {
                        self.0.cleanup.schedule(&[json!({"uploadedAttachmentId":uploaded,"uploadEnvironmentId":self.0.environment})]);
                    }
                    self.0.uploads.borrow_mut().remove(&id);
                    if let Some(fields) = image.as_object_mut() {
                        fields.remove("uploadedAttachmentId");
                        fields.remove("uploadEnvironmentId");
                    }
                }
            }
        }
        *self.0.capabilities.borrow_mut() = capabilities;
        *self.0.connection.borrow_mut() = connection;
        self.changed();
    }
    pub fn capabilities(&self) -> Capabilities {
        self.0.capabilities.borrow().clone()
    }
    pub fn draft(&self) -> Value {
        super::web_models::draft(&self.0.drafts.borrow(), &self.0.key)
    }
    fn count(&self) -> usize {
        let drafts = self.0.drafts.borrow();
        let draft = &drafts["draftsByThreadKey"][&self.0.key];
        ["images", "files"]
            .into_iter()
            .map(|field| draft[field].as_array().map_or(0, Vec::len))
            .sum::<usize>()
            + self.0.preparing.borrow().len()
            + self.0.rewind_slots.get()
    }
    pub fn remaining_slots(&self) -> usize {
        PROVIDER_SEND_TURN_MAX_ATTACHMENTS.saturating_sub(self.count())
    }

    pub(crate) fn reserve_rewind(&self, count: usize) -> Result<RewindReservation, ClientError> {
        if !self.0.writable() {
            return Err(ClientError::Cancelled);
        }
        if !self.0.preparing.borrow().is_empty() {
            return Err(ClientError::InvalidPayload(
                "Wait for attachments to finish preparing before rewinding.".into(),
            ));
        }
        if count > self.remaining_slots() {
            return Err(ClientError::InvalidPayload(
                "Make room for this message's attachments in the composer before rewinding.".into(),
            ));
        }
        self.0.rewind_slots.set(self.0.rewind_slots.get() + count);
        Ok(RewindReservation {
            session: self.0.clone(),
            count,
            images: Vec::new(),
            files: Vec::new(),
        })
    }

    fn contains(&self, id: &str) -> bool {
        let drafts = self.0.drafts.borrow();
        let draft = &drafts["draftsByThreadKey"][&self.0.key];
        ["images", "files"]
            .into_iter()
            .flat_map(|field| draft[field].as_array().into_iter().flatten())
            .any(|attachment| attachment["id"] == id)
    }
    pub fn block_reason(&self) -> Option<String> {
        let drafts = self.0.drafts.borrow();
        let draft = &drafts["draftsByThreadKey"][&self.0.key];
        self.capabilities()
            .block_reason(draft["files"].as_array().map_or(&[], Vec::as_slice))
    }
    pub fn items(&self) -> Vec<AttachmentItem> {
        let drafts = self.0.drafts.borrow();
        let draft = &drafts["draftsByThreadKey"][&self.0.key];
        let mut items: Vec<_> = ["images", "files"]
            .into_iter()
            .flat_map(|field| draft[field].as_array().into_iter().flatten())
            .map(|item| {
                let id = item["id"].as_str().unwrap_or_default().to_owned();
                let mime = item["mimeType"].as_str().unwrap_or_default().to_owned();
                AttachmentItem {
                    name: item["name"].as_str().unwrap_or_default().into(),
                    mime: mime.clone(),
                    size: item["sizeBytes"].as_u64().unwrap_or_default(),
                    image: item["type"] == "image",
                    video: mime.starts_with("video/"),
                    preview: item["previewUrl"]
                        .as_str()
                        .filter(|src| !src.is_empty())
                        .map(str::to_owned),
                    needs_reattach: web_content::needs_reattach(item),
                    non_persisted: draft["nonPersistedImageIds"]
                        .as_array()
                        .is_some_and(|ids| ids.iter().any(|value| value == &id)),
                    upload: self
                        .0
                        .uploads
                        .borrow()
                        .get(&id)
                        .cloned()
                        .unwrap_or_default(),
                    id,
                }
            })
            .collect();
        items.extend(self.0.preparing.borrow().iter().cloned());
        items
    }
    pub fn image_referenced(&self, id: &str) -> bool {
        let context_id = web_drafts::kind_context_id("image", id);
        crate::view_logic::context_references::collect_references(
            self.draft()["prompt"].as_str().unwrap_or_default(),
        )
        .iter()
        .any(|reference| reference.context_id == context_id)
    }
    pub fn remove(&self, id: &str) {
        if !self.0.writable() {
            return;
        }
        let draft = self.draft();
        let persisted_upload = ["images", "files"]
            .into_iter()
            .flat_map(|field| draft[field].as_array().into_iter().flatten())
            .find(|attachment| attachment["id"] == id)
            .and_then(|attachment| attachment["uploadedAttachmentId"].as_str())
            .map(str::to_owned);
        let uploaded = self.0.uploaded.borrow_mut().remove(id).or(persisted_upload);
        if let Some(uploaded) = uploaded {
            // Global draft/outbox/share references are checked by the client's cleanup worker.
            self.0.cleanup.schedule(&[
                json!({"uploadedAttachmentId":uploaded,"uploadEnvironmentId":self.0.environment}),
            ]);
        }
        self.0.preparing.borrow_mut().retain(|item| item.id != id);
        let mut drafts = self.0.drafts.borrow_mut();
        web_content::remove_image(&mut drafts, &self.0.key, id);
        web_content::remove_file(&mut drafts, &self.0.key, id);
        self.0.uploads.borrow_mut().remove(id);
        drop(drafts);
        self.changed();
    }
    pub async fn intake(&self, files: Vec<HostResource>) -> Result<(), ClientError> {
        self.intake_sources(files.into_iter().map(Source::Resource).collect())
            .await
    }
    pub async fn intake_bytes(&self, files: Vec<IncomingFile>) -> Result<(), ClientError> {
        self.intake_sources(files.into_iter().map(Source::Bytes).collect())
            .await
    }
    /// Port of textPaste.ts and useComposerAttachmentIntake.foldPastedText.
    pub fn prepare_text_paste(
        &self,
        text: &str,
        bypass: bool,
        selection: Option<(usize, usize)>,
    ) -> Result<Option<IncomingFile>, ClientError> {
        if bypass || text.is_empty() {
            return Ok(None);
        }
        let draft = self.draft();
        let length = draft["prompt"]
            .as_str()
            .unwrap_or_default()
            .encode_utf16()
            .count();
        let (start, end) = selection.unwrap_or((0, 0));
        let start = start.min(length);
        let end = end.min(length).max(start);
        let exceeds = length - (end - start) + text.encode_utf16().count()
            > PROVIDER_SEND_TURN_MAX_INPUT_CHARS;
        if text.len() < 32 * 1024 && !exceeds {
            return Ok(None);
        }
        let limit = self.capabilities().staging_limit();
        if self.count() >= PROVIDER_SEND_TURN_MAX_ATTACHMENTS || limit.is_none() {
            return if exceeds {
                Err(ClientError::InvalidPayload(
                    "Pasted text is too large for this message".into(),
                ))
            } else {
                Ok(None)
            };
        }
        if text.len() as u64 > limit.unwrap_or_default() {
            return if exceeds {
                Err(ClientError::InvalidPayload(
                    "Pasted text is too large to attach".into(),
                ))
            } else {
                Ok(None)
            };
        }
        let mut names = self.0.pasted_names.borrow_mut();
        for file in draft["files"].as_array().into_iter().flatten() {
            if let Some(name) = file["name"].as_str() {
                names.insert(name.to_lowercase());
            }
        }
        let mut name = "pasted-text.txt".to_owned();
        let mut sequence = 2usize;
        while names.contains(&name) {
            name = format!("pasted-text-{sequence}.txt");
            sequence += 1;
        }
        names.insert(name.clone());
        Ok(Some(IncomingFile {
            name,
            mime: "text/plain;charset=utf-8".into(),
            bytes: text.as_bytes().to_vec(),
            source: Some(json!({"_tag":"pasted-text"})),
            insertion: selection.map(|_| {
                (
                    draft["prompt"].as_str().unwrap_or_default().to_owned(),
                    start,
                    end,
                )
            }),
        }))
    }
    async fn intake_sources(&self, files: Vec<Source>) -> Result<(), ClientError> {
        if !self.0.writable() {
            return Err(ClientError::Cancelled);
        }
        let mut reservations = Vec::new();
        let mut batch = PreparingBatch {
            session: self.0.clone(),
            ids: Vec::new(),
        };
        let mut error = None;
        let mut claimed_markers = std::collections::BTreeSet::new();
        let draft = self.draft();
        // Slots are reserved before the first read so overlapping picker/paste completions share the limit.
        for source in files {
            let file = source.metadata();
            let kind = classify(&file.name, &file.mime);
            let marker = (kind == FileKind::File)
                .then(|| {
                    draft["files"].as_array().and_then(|files| {
                        files.iter().find(|candidate| {
                            web_content::needs_reattach(candidate)
                                && candidate["name"] == file.name
                                && candidate["mimeType"] == file_mime(&file.name, &file.mime)
                                && candidate["id"]
                                    .as_str()
                                    .is_some_and(|id| !claimed_markers.contains(id))
                        })
                    })
                })
                .flatten();
            if marker.is_none()
                && self.count().saturating_sub(claimed_markers.len())
                    >= PROVIDER_SEND_TURN_MAX_ATTACHMENTS
            {
                error = Some(ClientError::InvalidPayload(format!(
                    "You can attach up to {PROVIDER_SEND_TURN_MAX_ATTACHMENTS} files per message."
                )));
                continue;
            }
            if kind == FileKind::UnsupportedImage {
                error = Some(ClientError::InvalidPayload(format!(
                    "'{}' is not a supported image type. Attach GIF, HEIC, HEIF, JPEG, PNG, or WebP images.",
                    file.name
                )));
                continue;
            }
            if kind == FileKind::File && self.capabilities().staging_limit().is_none() {
                error = Some(ClientError::InvalidPayload(
                    "This server does not support file attachments.".into(),
                ));
                continue;
            }
            let id = match self.0.platform.random_bytes(16) {
                Ok(bytes) if bytes.len() == 16 => bytes
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>(),
                Ok(_) => {
                    error = Some(ClientError::InvalidPayload(
                        "random attachment identity".into(),
                    ));
                    continue;
                }
                Err(failure) => {
                    error = Some(failure.into());
                    continue;
                }
            };
            if let Some(id) = marker.and_then(|marker| marker["id"].as_str()) {
                claimed_markers.insert(id.to_owned());
            }
            batch.ids.push(id.clone());
            self.0.preparing.borrow_mut().push(AttachmentItem {
                id: id.clone(),
                name: file.name.clone(),
                mime: file.mime.clone(),
                size: 0,
                image: kind == FileKind::Image,
                video: false,
                preview: None,
                needs_reattach: false,
                non_persisted: false,
                upload: UploadState::Preparing,
            });
            reservations.push((id, source, kind));
        }
        self.changed();
        for (id, source, kind) in reservations {
            let file = source.metadata();
            let attachment_source = match &source {
                Source::Bytes(file) => file.source.clone(),
                Source::Resource(_) => None,
            };
            let insertion = match &source {
                Source::Bytes(file) => file.insertion.clone(),
                Source::Resource(_) => None,
            };
            if !self.0.preparing.borrow().iter().any(|item| item.id == id) {
                continue;
            }
            let limit = if kind == FileKind::Image {
                PROVIDER_SEND_TURN_MAX_IMAGE_BYTES
            } else {
                self.capabilities().staging_limit().unwrap_or_default()
            };
            let inline = matches!(&source, Source::Bytes(_));
            let result = match source {
                Source::Resource(_) => {
                    self.0
                        .platform
                        .read_resource(file.handle.clone(), limit as usize)
                        .await
                }
                Source::Bytes(file) => Ok(file.bytes),
            };
            if !self.0.writable() {
                return Err(ClientError::Cancelled);
            }
            let mut preparing = self.0.preparing.borrow_mut();
            let count = preparing.len();
            preparing.retain(|item| item.id != id);
            let removed = count != preparing.len();
            drop(preparing);
            if !removed {
                continue;
            }
            match result {
                Ok(bytes) if !bytes.is_empty() && bytes.len() as u64 <= limit => {
                    let mime = if kind == FileKind::Image {
                        image_mime(&file.name, &file.mime)
                    } else {
                        file_mime(&file.name, &file.mime)
                    };
                    if kind == FileKind::Image
                        && !matches!(
                            mime.to_ascii_lowercase().as_str(),
                            "image/gif" | "image/jpeg" | "image/png" | "image/webp"
                        )
                    {
                        error = Some(ClientError::Unsupported("image preparation")); // Host image preparation is a requested platform seam.
                        continue;
                    }
                    let mut attachment = json!({"type":if kind == FileKind::Image {"image"} else {"file"},"id":id,"name":if file.name.is_empty() { if kind == FileKind::Image {"image"} else {"file"} } else {&file.name},"mimeType":mime,"sizeBytes":bytes.len(),"fileUri":file.handle,"file":file.handle});
                    if let Some(source) = attachment_source {
                        attachment["source"] = source;
                    }
                    if inline {
                        attachment["dataUrl"] = json!(format!(
                            "data:{mime};base64,{}",
                            base64::engine::general_purpose::STANDARD.encode(&bytes)
                        ));
                    }
                    if kind == FileKind::Image {
                        let data = format!(
                            "data:{mime};base64,{}",
                            base64::engine::general_purpose::STANDARD.encode(&bytes)
                        );
                        attachment["previewUrl"] = json!(data);
                        attachment["dataUrl"] = attachment["previewUrl"].clone();
                        let mut drafts = self.0.drafts.borrow_mut();
                        let added =
                            web_content::add_images(&mut drafts, &self.0.key, &[attachment], false);
                        if added.accepted_ids.contains(&id) {
                            let draft = &mut drafts["draftsByThreadKey"][&self.0.key];
                            let mut prompt =
                                draft["prompt"].as_str().unwrap_or_default().to_owned();
                            if !crate::view_logic::context_references::strip_references(&prompt)
                                .trim()
                                .is_empty()
                            {
                                web_drafts::ensure_reference(
                                    &mut prompt,
                                    "image",
                                    &web_drafts::kind_context_id("image", &id),
                                    &file.name,
                                );
                                draft["prompt"] = json!(prompt);
                            }
                        }
                    } else {
                        let current = self.draft();
                        let files = current["files"].as_array().map_or(&[][..], Vec::as_slice);
                        let count = files.len()
                            + current["images"].as_array().map_or(0, Vec::len)
                            + self.0.rewind_slots.get();
                        if count >= PROVIDER_SEND_TURN_MAX_ATTACHMENTS
                            && !files.iter().any(|marker| {
                                web_content::needs_reattach(marker)
                                    && web_content::reattach_marker_matches(marker, &attachment)
                            })
                        {
                            error = Some(ClientError::InvalidPayload(format!(
                                "You can attach up to {PROVIDER_SEND_TURN_MAX_ATTACHMENTS} files per message."
                            )));
                            continue;
                        }
                        let ids = web_content::add_files(
                            &mut self.0.drafts.borrow_mut(),
                            &self.0.key,
                            &[attachment],
                            false,
                            insertion.is_none(),
                        );
                        if ids.contains(&id)
                            && let Some((expected, start, end)) = insertion
                        {
                            let mut drafts = self.0.drafts.borrow_mut();
                            let draft = &mut drafts["draftsByThreadKey"][&self.0.key];
                            if draft["prompt"].as_str() == Some(&expected) {
                                let replacement = format!(
                                    "{} ",
                                    web_drafts::context_link(
                                        "file",
                                        &web_drafts::kind_context_id("file", &id),
                                        &file.name
                                    )
                                );
                                let edit = crate::thread_views::document::Edit {
                                    start,
                                    end,
                                    cursor_after: start + replacement.encode_utf16().count(),
                                    replacement,
                                };
                                if let Some(prompt) = edit.apply(&expected) {
                                    draft["prompt"] = json!(prompt);
                                }
                            }
                        }
                    }
                }
                Ok(bytes) => {
                    error = Some(ClientError::InvalidPayload(if bytes.is_empty() {
                        format!("'{}' is empty or could not be read.", file.name)
                    } else {
                        assets::file_attachment_too_large_message(&file.name, limit)
                    }));
                }
                Err(failure) => {
                    error = Some(failure.into());
                }
            }
            self.changed();
        }
        self.changed();
        if let Some(error) = error {
            Err(error)
        } else {
            Ok(())
        }
    }
    pub async fn save_images(&self) {
        if !self.0.writable() {
            return;
        }
        let draft = self.draft();
        let serialized = serialize_composer_images(
            self.0.platform.as_ref(),
            draft["images"].as_array().map_or(&[], Vec::as_slice),
            draft["persistedAttachments"]
                .as_array()
                .map_or(&[], Vec::as_slice),
        )
        .await;
        let live = self.draft();
        if !self.0.writable() {
            return;
        }
        let serialized: Vec<_> = serialized
            .into_iter()
            .filter(|image| {
                live["images"]
                    .as_array()
                    .is_some_and(|images| images.iter().any(|current| current["id"] == image["id"]))
            })
            .collect();
        web_content::sync_persisted_attachments(
            &mut self.0.drafts.borrow_mut(),
            &self.0.key,
            &serialized,
        );
        self.changed();
    }
    pub async fn upload(&self, id: &str) -> Result<(), ClientError> {
        if !self.0.writable() {
            return Err(ClientError::Cancelled);
        }
        let connection = self
            .0
            .connection
            .borrow()
            .clone()
            .ok_or(ClientError::Disconnected)?;
        if !self.capabilities().uploads {
            return Err(ClientError::Unsupported("attachment uploads"));
        }
        if matches!(
            self.0.uploads.borrow().get(id),
            Some(UploadState::Uploading | UploadState::Ready)
        ) {
            return Ok(());
        }
        let draft = self.draft();
        let attachment = ["images", "files"]
            .into_iter()
            .flat_map(|field| draft[field].as_array().into_iter().flatten())
            .find(|attachment| attachment["id"] == id)
            .ok_or(ClientError::Cancelled)?
            .clone();
        self.0
            .uploads
            .borrow_mut()
            .insert(id.into(), UploadState::Uploading);
        self.changed();
        let result = self.upload_inner(&connection, &attachment).await;
        if self.contains(id) {
            let missing = self.draft()["files"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|file| file["id"] == id && web_content::needs_reattach(file));
            self.0.uploads.borrow_mut().insert(
                id.into(),
                match &result {
                    Ok(()) => UploadState::Ready,
                    Err(_) if missing => UploadState::Idle,
                    Err(ClientError::InvalidPayload(reason)) => UploadState::Failed(reason.clone()),
                    Err(error) => UploadState::Failed(error.to_string()),
                },
            );
        }
        self.changed();
        result
    }
    async fn upload_inner(
        &self,
        connection: &AttachmentConnection,
        attachment: &Value,
    ) -> Result<(), ClientError> {
        let generation = self.0.generation.get();
        let id = attachment["id"].as_str().ok_or(ClientError::Cancelled)?;
        let image = attachment["type"] == "image";
        if !image
            && attachment["uploadEnvironmentId"] == self.0.environment.0
            && let Some(persisted) = attachment["uploadedAttachmentId"].as_str()
        {
            let verification =
                assets::verify_persisted_attachment(&connection.endpoint, persisted.into()).await;
            if self.0.generation.get() != generation || !self.0.writable() || !self.contains(id) {
                return Err(ClientError::Cancelled);
            }
            let has_bytes = attachment["fileUri"]
                .as_str()
                .is_some_and(|uri| !uri.is_empty())
                || attachment["dataUrl"].is_string();
            match verification {
                assets::AttachmentVerification::Verified => {
                    self.0
                        .uploaded
                        .borrow_mut()
                        .insert(id.into(), persisted.into());
                    return Ok(());
                }
                assets::AttachmentVerification::Missing if !has_bytes => {
                    web_content::mark_file_upload_missing(
                        &mut self.0.drafts.borrow_mut(),
                        &self.0.key,
                        id,
                        &self.0.environment.0,
                        persisted,
                    );
                    return Err(ClientError::InvalidPayload(
                        "Uploaded file expired. Remove it and attach it again.".into(),
                    ));
                }
                assets::AttachmentVerification::Failed(_) => {
                    return Err(ClientError::InvalidPayload(
                        "Uploaded file could not be verified. Retry when the server reconnects."
                            .into(),
                    ));
                }
                assets::AttachmentVerification::Missing => {}
            }
        }
        let limit = if image {
            PROVIDER_SEND_TURN_MAX_IMAGE_BYTES
        } else {
            self.capabilities()
                .staging_limit()
                .ok_or(ClientError::Unsupported("file attachments"))?
        };
        let bytes = if let Some(data) = attachment["dataUrl"].as_str() {
            base64::engine::general_purpose::STANDARD
                .decode(
                    data.split_once(',')
                        .ok_or_else(|| ClientError::InvalidPayload("attachment data URL".into()))?
                        .1,
                )
                .map_err(|_| ClientError::InvalidPayload("attachment data URL".into()))?
        } else {
            self.0
                .platform
                .read_resource(
                    attachment["fileUri"]
                        .as_str()
                        .ok_or(ClientError::Cancelled)?
                        .into(),
                    limit as usize,
                )
                .await?
        };
        if bytes.len() as u64 > limit {
            return Err(ClientError::InvalidPayload(
                assets::file_attachment_too_large_message(
                    attachment["name"].as_str().unwrap_or_default(),
                    limit,
                ),
            ));
        }
        let upload = serde_json::from_value(
            json!({"type":attachment["type"],"name":attachment["name"],"mimeType":attachment["mimeType"],"sizeBytes":bytes.len()}),
        )?;
        let result = assets::attachment_upload_cycle(
            &connection.endpoint,
            self.0.platform.as_ref(),
            &connection.base_url,
            upload,
            bytes,
            attachment["mimeType"].as_str().unwrap_or_default(),
            |_| {
                self.0.generation.get() == generation
                    && self.0.writable()
                    && self.contains(id)
                    && self.capabilities().uploads
            },
        )
        .await;
        let uploaded_id = match result {
            AttachmentUploadResult::Uploaded { attachment_id } => attachment_id,
            AttachmentUploadResult::Cancelled { .. } => return Err(ClientError::Cancelled),
            AttachmentUploadResult::Failed {
                attachment_id,
                error,
                ..
            } => {
                if let Some(id) = attachment_id {
                    delete_upload(&connection.endpoint, &id).await?;
                }
                return Err(error);
            }
        };
        if self.0.generation.get() != generation
            || !self.0.writable()
            || !self.contains(id)
            || !self.capabilities().uploads
        {
            delete_upload(&connection.endpoint, &uploaded_id).await?;
            return Err(ClientError::Cancelled);
        }
        if !image {
            web_content::set_file_upload(
                &mut self.0.drafts.borrow_mut(),
                &self.0.key,
                id,
                &self.0.environment.0,
                &uploaded_id,
            );
        } else {
            let mut drafts = self.0.drafts.borrow_mut();
            if let Some(image) = drafts["draftsByThreadKey"][&self.0.key]["images"]
                .as_array_mut()
                .and_then(|images| images.iter_mut().find(|image| image["id"] == id))
            {
                image["uploadedAttachmentId"] = json!(uploaded_id);
                image["uploadEnvironmentId"] = json!(self.0.environment);
            }
        }
        self.0.uploaded.borrow_mut().insert(id.into(), uploaded_id);
        Ok(())
    }
    pub async fn upload_pending(&self) -> Result<(), ClientError> {
        if !self.capabilities().uploads {
            return Ok(());
        }
        let mut failure = None;
        for item in self.items() {
            if item.needs_reattach || item.upload != UploadState::Idle {
                continue;
            }
            if !item.image
                && self
                    .capabilities()
                    .staging_limit()
                    .is_none_or(|limit| item.size > limit)
            {
                continue;
            }
            if let Err(error) = self.upload(&item.id).await {
                failure = Some(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
    pub async fn persist(
        &self,
        thread: ThreadId,
        message: MessageId,
        attachments: &[SendAttachment],
    ) -> Result<Vec<ChatAttachment>, ClientError> {
        let connection = self
            .0
            .connection
            .borrow()
            .clone()
            .ok_or(ClientError::Disconnected)?;
        persist_attachments(&connection.endpoint, thread, message, attachments).await
    }

    pub fn draft_attachments(
        &self,
    ) -> Result<Vec<crate::thread_views::outbox::DraftAttachment>, ClientError> {
        if !self.0.preparing.borrow().is_empty() {
            return Err(ClientError::InvalidPayload(
                "Wait for attachments to finish preparing.".into(),
            ));
        }
        if let Some(reason) = self.block_reason() {
            return Err(ClientError::InvalidPayload(reason));
        }
        let draft = self.draft();
        ["images", "files"]
            .into_iter()
            .flat_map(|field| draft[field].as_array().into_iter().flatten())
            .map(|attachment| {
                let mut attachment = attachment.clone();
                if attachment["type"] == "image" {
                    attachment["previewUri"] = attachment["previewUrl"].clone();
                    if attachment["fileUri"] == ""
                        && let Some(fields) = attachment.as_object_mut()
                    {
                        let _removed = fields.remove("fileUri");
                    }
                } else if attachment.get("fileUri").is_none()
                    && attachment["uploadedAttachmentId"].is_string()
                {
                    attachment["fileUri"] = json!("");
                }
                if let Some(uploaded) = self
                    .0
                    .uploaded
                    .borrow()
                    .get(attachment["id"].as_str().unwrap_or_default())
                {
                    attachment["uploadedAttachmentId"] = json!(uploaded);
                    attachment["uploadEnvironmentId"] = json!(self.0.environment);
                }
                crate::thread_views::outbox::DraftAttachment::decode(attachment)
            })
            .collect()
    }
}

async fn delete_upload(endpoint: &SessionEndpoint, id: &str) -> Result<(), ClientError> {
    endpoint
        .call::<t3_contracts::rpc::WsAttachmentsDeleteRpc>(serde_json::from_value(
            json!({"attachmentId":id}),
        )?)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::*;
    use crate::{
        platform::{HostReply, PlatformError},
        testing::FakePlatform,
    };
    use futures::executor::block_on;
    use futures::{FutureExt, future::LocalBoxFuture};

    struct ReadGate {
        platform: FakePlatform,
        receiver: RefCell<Option<futures::channel::oneshot::Receiver<Vec<u8>>>>,
    }
    impl Storage for ReadGate {
        fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
            self.platform.get(area, key)
        }
        fn write(&self, area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()> {
            self.platform.write(area, writes)
        }
        fn list(&self, area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>> {
            self.platform.list(area, namespace)
        }
    }
    impl Platform for ReadGate {
        fn now_epoch_ms(&self) -> u64 {
            self.platform.now_epoch_ms()
        }
        fn monotonic_ms(&self) -> u64 {
            self.platform.monotonic_ms()
        }
        fn random_bytes(&self, length: usize) -> Result<Vec<u8>, PlatformError> {
            self.platform.random_bytes(length)
        }
        fn sleep(&self, ms: u64) -> LocalBoxFuture<'_, ()> {
            self.platform.sleep(ms)
        }
        fn yield_to_host(&self) -> LocalBoxFuture<'_, ()> {
            self.platform.yield_to_host()
        }
        fn events(&self) -> LocalBoxStream<'static, PlatformEvent> {
            self.platform.events()
        }
        fn http(&self, request: HttpRequest) -> HostFuture<'_, HttpResponse> {
            self.platform.http(request)
        }
        fn websocket(&self, request: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>> {
            self.platform.websocket(request)
        }
        fn host(&self, action: HostAction) -> HostFuture<'_, HostReply> {
            self.platform.host(action)
        }
        fn read_resource(&self, _handle: String, _max_bytes: usize) -> HostFuture<'_, Vec<u8>> {
            let receiver = self.receiver.borrow_mut().take().expect("one gated read");
            async move { receiver.await.map_err(|_| PlatformError::Cancelled) }.boxed_local()
        }
    }

    fn resource(name: &str, mime: &str) -> HostResource {
        HostResource {
            handle: name.into(),
            name: name.into(),
            mime: mime.into(),
        }
    }
    fn session() -> (FakePlatform, AttachmentSession) {
        let platform = FakePlatform::default();
        let client = AttachmentSession::new(
            Rc::new(platform.clone()),
            Rc::new(RefCell::new(json!({"draftsByThreadKey":{}}))),
            "env:thread".into(),
            EnvironmentId("env".into()),
        );
        (platform, client)
    }
    fn file(name: &str, mime: &str, bytes: &[u8]) -> IncomingFile {
        IncomingFile {
            name: name.into(),
            mime: mime.into(),
            bytes: bytes.into(),
            source: None,
            insertion: None,
        }
    }

    #[test]
    fn picked_file_drafts_persist_metadata_without_picked_bytes() {
        block_on(async {
            let (_, client) = session();
            client
                .intake_bytes(vec![file("note.txt", "text/plain", b"note")])
                .await
                .unwrap();
            let saved =
                web_drafts::partialize_state(&client.0.drafts.borrow(), &Default::default());
            let metadata = &saved["draftsByThreadKey"]["env:thread"]["files"][0];
            assert_eq!(metadata["name"], "note.txt");
            assert_eq!(metadata["sizeBytes"], 4);
            assert!(
                metadata.get("dataUrl").is_none(),
                "only the live draft may hold picked bytes"
            );
            let id = client.draft()["files"][0]["id"]
                .as_str()
                .unwrap()
                .to_owned();
            web_content::set_file_upload(
                &mut client.0.drafts.borrow_mut(),
                "env:thread",
                &id,
                "env",
                "upload",
            );
            let saved =
                web_drafts::partialize_state(&client.0.drafts.borrow(), &Default::default());
            let metadata = &saved["draftsByThreadKey"]["env:thread"]["files"][0];
            assert_eq!(metadata["attachmentId"], "upload");
            assert!(
                metadata.get("dataUrl").is_none(),
                "uploaded drafts keep the upload identity"
            );
        });
    }

    #[test]
    fn a_saved_pick_keeps_bytes_in_the_live_session_and_requires_reattach_after_restart() {
        use crate::thread_views::composer::draft_service::ComposerDraftService;

        block_on(async {
            let platform = Rc::new(FakePlatform::default());
            let drafts = ComposerDraftService::new(platform.clone());
            drafts.load().await.unwrap();
            let client = AttachmentSession::new(
                platform.clone(),
                drafts.document(),
                "env:thread".into(),
                EnvironmentId("env".into()),
            );
            client
                .intake_bytes(vec![file("note.txt", "text/plain", b"note")])
                .await
                .unwrap();
            drafts
                .update(|document| {
                    document["draftsByThreadKey"]["env:thread"]["prompt"] = json!("edited");
                    Ok(())
                })
                .unwrap();
            drafts.flush().await.unwrap();
            drafts.load().await.unwrap();
            assert!(client.draft()["files"][0]["dataUrl"].is_string());

            let reopened = ComposerDraftService::new(platform);
            reopened.load().await.unwrap();
            let document = reopened.document();
            let document = document.borrow();
            let file = &document["draftsByThreadKey"]["env:thread"]["files"][0];
            assert_eq!(file["name"], "note.txt");
            assert!(web_content::needs_reattach(file));
        });
    }

    #[test]
    fn rewind_keeps_the_prompt_and_attachments_that_fit_after_the_draft_changes() {
        use crate::thread_views::thread_ops::rewind::PreparedRewindAttachments;

        let (_, client) = session();
        let mut rewind = client.reserve_rewind(2).unwrap();
        for name in ["one.txt", "two.txt"] {
            let original = serde_json::from_value(json!({
                "type":"file", "id":name, "name":name, "mimeType":"text/plain", "sizeBytes":1
            }))
            .unwrap();
            rewind
                .prepare(
                    &original,
                    crate::view_logic::chat_view::RestoredAttachment {
                        name: name.into(),
                        mime_type: "text/plain".into(),
                        bytes: b"x".to_vec(),
                    },
                )
                .unwrap();
        }
        let old: Vec<_> = (0..PROVIDER_SEND_TURN_MAX_ATTACHMENTS - 1)
            .map(|i| json!({"id":i.to_string(), "name":"old.txt", "type":"file", "sizeBytes":1}))
            .collect();
        let mut document =
            json!({"draftsByThreadKey":{"env:thread":{"prompt":"Restored prompt","files":old}}});
        rewind.commit(&mut document, "env:thread").unwrap();
        let saved = web_drafts::partialize_state(&document, &Default::default());
        let draft = &saved["draftsByThreadKey"]["env:thread"];
        assert_eq!(draft["prompt"], "Restored prompt");
        let files = draft["files"].as_array().unwrap();
        assert_eq!(files.len(), PROVIDER_SEND_TURN_MAX_ATTACHMENTS);
        assert_eq!(files.last().unwrap()["name"], "one.txt");
        assert!(
            files.last().unwrap()["dataUrl"].is_string(),
            "rewind bytes survive reload"
        );
        let legacy = document["draftsByThreadKey"]["env:thread"]["files"]
            .as_array_mut()
            .unwrap()
            .last_mut()
            .unwrap();
        legacy.as_object_mut().unwrap().remove("restoredFromRewind");
        let saved = web_drafts::partialize_state(&document, &Default::default());
        let legacy = saved["draftsByThreadKey"]["env:thread"]["files"]
            .as_array()
            .unwrap()
            .last()
            .unwrap();
        assert!(
            legacy["dataUrl"].is_string(),
            "older rewind bytes survive reload"
        );
        assert_eq!(legacy["restoredFromRewind"], true);
    }

    #[test]
    fn classification_ports_the_web_image_file_and_raw_table() {
        for (name, mime, expected) in [
            ("diagram.svg", "image/svg+xml", FileKind::File),
            ("diagram.SVG", "", FileKind::File),
            ("photo.tiff", "image/tiff", FileKind::File),
            ("picture.bmp", "image/bmp", FileKind::File),
            (
                "layers.psd",
                "image/vnd.adobe.photoshop",
                FileKind::UnsupportedImage,
            ),
            ("photo.DNG", "image/png", FileKind::File),
            ("clipboard", "image/x-dng", FileKind::File),
            ("photo.jpg", "image/jpeg", FileKind::Image),
            ("photo.heic", "", FileKind::Image),
            ("photo.cr2", "image/x-canon-cr2", FileKind::UnsupportedImage),
            ("photo.PNG", "application/octet-stream", FileKind::Image),
        ] {
            assert_eq!(classify(name, mime), expected, "{name}");
        }
        assert_eq!(file_mime("photo.DNG", "image/jpeg"), "image/x-adobe-dng");
        assert_eq!(
            file_mime("movie.MOV", "application/octet-stream"),
            "video/quicktime"
        );
        assert_eq!(image_mime("x.PNG", "application/octet-stream"), "image/png");
    }
    #[test]
    fn clipboard_text_is_preserved_with_synthetic_files_but_images_svg_and_dng_claim_it() {
        for (name, mime, expected) in [
            ("clipboard.rtf", "application/rtf", false),
            ("photo.tiff", "image/tiff", false),
            ("diagram.svg", "image/svg+xml", true),
            ("photo.dng", "application/octet-stream", true),
            ("photo.png", "image/png", true),
        ] {
            assert_eq!(
                should_handle_paste(&[resource(name, mime)], "caption"),
                expected
            );
            assert!(should_handle_paste(&[resource(name, mime)], ""));
        }
        assert!(!should_handle_paste(&[], "text"));
    }
    #[test]
    fn unknown_capabilities_stage_files_but_cannot_send_and_known_limits_are_clamped() {
        let files = [json!({"name":"pending.zip","sizeBytes":PROVIDER_SEND_TURN_MAX_FILE_BYTES})];
        let mut caps = Capabilities::default();
        assert_eq!(
            caps.staging_limit(),
            Some(PROVIDER_SEND_TURN_MAX_FILE_BYTES)
        );
        assert_eq!(
            caps.block_reason(&files).as_deref(),
            Some("Waiting for the server before file attachments can send")
        );
        caps.known = true;
        assert!(caps.staging_limit().is_none());
        assert!(caps.block_reason(&files).is_some());
        caps.uploads = true;
        caps.max_file_bytes = Some(u64::MAX);
        assert_eq!(
            caps.staging_limit(),
            Some(PROVIDER_SEND_TURN_MAX_FILE_BYTES)
        );
        assert!(caps.block_reason(&files).is_none());
        caps.max_file_bytes = Some(1);
        assert_eq!(
            caps.block_reason(&files),
            Some(assets::file_attachment_too_large_message("pending.zip", 1))
        );
        assert!(caps.block_reason(&[]).is_none());
    }
    #[test]
    fn capability_loss_preserves_draft_persisted_files() {
        let attachments = [
            json!({"id":"image","type":"image"}),
            json!({"id":"pending","type":"file"}),
            json!({"id":"stored","type":"file","uploadedAttachmentId":"server"}),
        ];
        assert_eq!(uploads_to_release(&attachments), attachments[..2]);
    }

    #[test]
    fn project_upload_cleanup_stays_in_the_requested_environment_and_project() {
        let state = json!({"draftThreadsByThreadKey":{"a":{"environmentId":"one","projectId":"project"},"b":{"environmentId":"two","projectId":"project"},"c":{"environmentId":"one","projectId":"other"}},
            "draftsByThreadKey":{"a":{"images":[{"uploadedAttachmentId":"upload-a","uploadEnvironmentId":"one"}]},"b":{"files":[{"uploadedAttachmentId":"upload-b","uploadEnvironmentId":"two"}]},"c":{"images":[{"uploadedAttachmentId":"upload-c","uploadEnvironmentId":"one"}]},"one:thread":{"files":[{"uploadedAttachmentId":"upload-thread","uploadEnvironmentId":"one"}]}}});
        let cleanup = super::super::cleanup::AttachmentCleanup::default();
        release_project_draft_uploads(
            &state,
            &EnvironmentId("one".into()),
            "project",
            &["one:thread".into()],
            &cleanup,
        );
        assert_eq!(cleanup.pending(), 2);
        release_composer_draft_uploads(&state, "b", &cleanup);
        assert_eq!(cleanup.pending(), 3);
    }

    #[test]
    fn large_unicode_pastes_get_unique_names_and_keep_their_bytes_and_selected_range() {
        block_on(async {
            let (_, client) = session();
            client.0.drafts.borrow_mut()["draftsByThreadKey"]["env:thread"] =
                json!({"prompt":"a😀b","images":[],"files":[]});
            let large = "界".repeat(11_000);
            assert!(
                client
                    .prepare_text_paste(&large, true, None)
                    .unwrap()
                    .is_none()
            );
            assert!(
                client
                    .prepare_text_paste("small", false, None)
                    .unwrap()
                    .is_none()
            );
            let first = client
                .prepare_text_paste(&large, false, Some((1, 3)))
                .unwrap()
                .unwrap();
            assert_eq!(first.name, "pasted-text.txt");
            assert_eq!(first.bytes, large.as_bytes());
            let second = client
                .prepare_text_paste(&large, false, None)
                .unwrap()
                .unwrap();
            assert_eq!(second.name, "pasted-text-2.txt");
            client.intake_bytes(vec![first]).await.unwrap();
            let draft = client.draft();
            let prompt = draft["prompt"].as_str().unwrap();
            assert!(prompt.starts_with("a[pasted-text.txt]"));
            assert!(prompt.ends_with(" b"));
            assert!(!prompt.contains('😀'));
            assert_eq!(draft["files"][0]["source"]["_tag"], "pasted-text");
        });
    }
    #[test]
    fn serialization_reads_only_new_images_and_prunes_removed_images() {
        block_on(async {
            let platform = FakePlatform::default();
            platform.push_host(Ok(HostReply::Bytes(b"a".to_vec())));
            let a =
                json!({"id":"a","name":"a.png","mimeType":"image/png","sizeBytes":1,"fileUri":"a"});
            let b =
                json!({"id":"b","name":"b.png","mimeType":"image/png","sizeBytes":1,"fileUri":"b"});
            let first = serialize_composer_images(&platform, std::slice::from_ref(&a), &[]).await;
            assert_eq!(first[0]["dataUrl"], "data:image/png;base64,YQ==");
            platform.push_host(Ok(HostReply::Bytes(b"b".to_vec())));
            let second = serialize_composer_images(&platform, &[a.clone(), b], &first).await;
            assert_eq!(second[0], first[0]);
            assert_eq!(second[1]["dataUrl"], "data:image/png;base64,Yg==");
            assert_eq!(
                serialize_composer_images(&platform, &[a], &second).await,
                first
            );
            assert!(
                serialize_composer_images(&platform, &[], &second)
                    .await
                    .is_empty()
            );
        });
    }
    #[test]
    fn cached_serialization_never_reads_an_unavailable_file() {
        block_on(async {
            let platform = FakePlatform::default();
            platform.push_host(Err(PlatformError::Unsupported("unavailable file".into())));
            let a = json!({"id":"a","name":"a.png","mimeType":"image/png","sizeBytes":1,"fileUri":"gone","source":{"kind":"snap-shot"}});
            let persisted = json!({"id":"a","name":"a.png","mimeType":"image/png","sizeBytes":1,"dataUrl":"data:image/png;base64,YQ==","source":{"kind":"snap-shot"}});
            assert_eq!(
                serialize_composer_images(&platform, &[a], std::slice::from_ref(&persisted)).await,
                vec![persisted]
            );
            assert!(platform.read_resource("gone".into(), 1).await.is_err());
        });
    }
    #[test]
    fn serialization_omits_failed_uncached_reads_and_keeps_the_successes() {
        block_on(async {
            let platform = FakePlatform::default();
            platform.push_host(Err(PlatformError::Unsupported("unreadable".into())));
            platform.push_host(Ok(HostReply::Bytes(b"b".to_vec())));
            let image = |id: &str| json!({"id":id,"name":format!("{id}.png"),"mimeType":"image/png","sizeBytes":1,"fileUri":id});
            let result = serialize_composer_images(&platform, &[image("a"), image("b")], &[]).await;
            assert_eq!(result.len(), 1);
            assert_eq!(result[0]["id"], "b");
        });
    }
    #[test]
    fn intake_keeps_good_files_after_a_rejected_file_and_never_leaks_preparing_slots() {
        let (_, client) = session();
        block_on(async {
            assert!(
                client
                    .intake_bytes(vec![
                        file("empty.txt", "text/plain", b""),
                        file("good.txt", "text/plain", b"hello"),
                        file("bad.psd", "image/vnd.adobe.photoshop", b"bytes")
                    ])
                    .await
                    .is_err()
            );
            assert_eq!(client.items().len(), 1);
            let item = client.items()[0].clone();
            assert_eq!(item.name, "good.txt");
            assert_eq!(item.size, 5);
            assert_eq!(item.upload, UploadState::Idle);
            assert!(
                client.draft()["prompt"]
                    .as_str()
                    .unwrap()
                    .contains("good.txt")
            );
            assert!(client.draft_attachments().is_err());
            client.set_connection(
                Capabilities {
                    known: true,
                    uploads: true,
                    max_file_bytes: Some(100),
                },
                None,
            );
            assert_eq!(client.draft_attachments().unwrap().len(), 1);
            client.remove(&item.id);
            assert!(client.items().is_empty());
            assert_eq!(client.draft()["prompt"], "");
        });
    }
    #[test]
    fn duplicate_images_are_deduplicated_by_the_existing_draft_store() {
        let (_, client) = session();
        block_on(async {
            client
                .intake_bytes(vec![
                    file("a.png", "image/png", b"a"),
                    file("a.png", "image/png", b"a"),
                ])
                .await
                .unwrap();
            assert_eq!(client.items().len(), 1);
            assert_eq!(
                client.items()[0].preview.as_deref(),
                Some("data:image/png;base64,YQ==")
            );
            client.save_images().await;
            assert_eq!(
                client.draft()["persistedAttachments"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
        });
    }
    #[test]
    fn failed_host_read_and_unsupported_conversion_remove_the_preparing_tile() {
        let (platform, client) = session();
        block_on(async {
            platform.push_host(Err(PlatformError::Unsupported("read".into())));
            assert!(
                client
                    .intake(vec![resource("gone.png", "image/png")])
                    .await
                    .is_err()
            );
            assert!(client.items().is_empty());
            assert!(
                client
                    .intake_bytes(vec![file("photo.heic", "image/heic", b"heic")])
                    .await
                    .is_err()
            );
            assert!(client.items().is_empty());
        });
    }
    #[test]
    fn uri_names_decode_percent_escapes_without_turning_plus_into_space() {
        assert_eq!(
            resource_from_uri("file:///tmp/a%20b+c.png").unwrap().name,
            "a b+c.png"
        );
        assert!(resource_from_uri("https://example.com/photo.png").is_none());
    }

    #[test]
    fn pending_reads_reserve_capacity_and_removal_prevents_a_late_result_from_returning() {
        block_on(async {
            let (sender, receiver) = futures::channel::oneshot::channel();
            let platform = Rc::new(ReadGate {
                platform: FakePlatform::default(),
                receiver: RefCell::new(Some(receiver)),
            });
            let existing:Vec<_> = (0..PROVIDER_SEND_TURN_MAX_ATTACHMENTS-1).map(|i|json!({"id":format!("old-{i}"),"name":format!("old-{i}.txt"),"type":"file","mimeType":"text/plain","sizeBytes":1,"file":"old","fileUri":"old"})).collect();
            let client = AttachmentSession::new(
                platform,
                Rc::new(RefCell::new(
                    json!({"draftsByThreadKey":{"env:thread":{"prompt":"","images":[],"files":existing}}}),
                )),
                "env:thread".into(),
                EnvironmentId("env".into()),
            );
            let mut pending = Box::pin(client.intake(vec![resource("late.txt", "text/plain")]));
            assert!(futures::poll!(pending.as_mut()).is_pending());
            assert_eq!(client.items().len(), PROVIDER_SEND_TURN_MAX_ATTACHMENTS);
            assert!(
                client
                    .intake_bytes(vec![file("overflow.txt", "text/plain", b"x")])
                    .await
                    .is_err()
            );
            let id = client
                .items()
                .iter()
                .find(|item| item.upload == UploadState::Preparing)
                .unwrap()
                .id
                .clone();
            client.remove(&id);
            sender.send(b"late".to_vec()).unwrap();
            pending.await.unwrap();
            assert_eq!(client.items().len(), PROVIDER_SEND_TURN_MAX_ATTACHMENTS - 1);
            assert!(client.items().iter().all(|item| item.name != "late.txt"));
        });
    }

    #[test]
    fn dropping_the_intake_future_releases_preparing_slots() {
        block_on(async {
            let (_sender, receiver) = futures::channel::oneshot::channel();
            let platform = Rc::new(ReadGate {
                platform: FakePlatform::default(),
                receiver: RefCell::new(Some(receiver)),
            });
            let client = AttachmentSession::new(
                platform,
                Rc::new(RefCell::new(json!({"draftsByThreadKey":{}}))),
                "env:thread".into(),
                EnvironmentId("env".into()),
            );
            let mut pending = Box::pin(client.intake(vec![resource("late.txt", "text/plain")]));
            assert!(futures::poll!(pending.as_mut()).is_pending());
            assert_eq!(client.items().len(), 1);
            drop(pending);
            assert!(client.items().is_empty());
        });
    }

    #[test]
    fn preparing_tiles_keep_intake_order_instead_of_random_identity_order() {
        block_on(async {
            let (_sender, receiver) = futures::channel::oneshot::channel();
            let platform = Rc::new(ReadGate {
                platform: FakePlatform::default(),
                receiver: RefCell::new(Some(receiver)),
            });
            platform.random_bytes(250).unwrap();
            let client = AttachmentSession::new(
                platform,
                Rc::new(RefCell::new(json!({"draftsByThreadKey":{}}))),
                "env:thread".into(),
                EnvironmentId("env".into()),
            );
            let mut pending = Box::pin(client.intake(vec![
                resource("first.png", "image/png"),
                resource("second.png", "image/png"),
            ]));
            assert!(futures::poll!(pending.as_mut()).is_pending());
            assert_eq!(
                client
                    .items()
                    .iter()
                    .map(|item| item.name.as_str())
                    .collect::<Vec<_>>(),
                vec!["first.png", "second.png"]
            );
            drop(pending);
            assert!(client.items().is_empty());
        });
    }

    #[test]
    fn full_drafts_can_replace_a_matching_reattach_marker_but_reject_different_bytes() {
        block_on(async {
            let (platform, client) = session();
            let markers:Vec<_> = (0..PROVIDER_SEND_TURN_MAX_ATTACHMENTS).map(|i|json!({"id":format!("marker-{i}"),"name":format!("marker-{i}.txt"),"type":"file","mimeType":"text/plain","sizeBytes":1,"file":null})).collect();
            client.0.drafts.borrow_mut()["draftsByThreadKey"]["env:thread"] =
                json!({"prompt":"","images":[],"files":markers});
            client
                .intake_bytes(vec![file("marker-0.txt", "text/plain", b"x")])
                .await
                .unwrap();
            assert_eq!(client.items().len(), PROVIDER_SEND_TURN_MAX_ATTACHMENTS);
            assert!(
                !client
                    .items()
                    .iter()
                    .find(|item| item.name == "marker-0.txt")
                    .unwrap()
                    .needs_reattach
            );
            platform.push_host(Ok(HostReply::Bytes(b"changed".to_vec())));
            assert!(
                client
                    .intake(vec![resource("marker-1.txt", "text/plain")])
                    .await
                    .is_err()
            );
            assert!(
                client
                    .items()
                    .iter()
                    .find(|item| item.name == "marker-1.txt")
                    .unwrap()
                    .needs_reattach
            );
        });
    }
}
