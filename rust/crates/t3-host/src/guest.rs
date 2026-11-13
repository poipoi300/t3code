//! Guest token issuance/authorization belongs to t3-cloud::guest::GuestSharing.
//! These are the unowned, transport-neutral guest view projections.
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::Stream;
use std::{
    collections::VecDeque,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
pub use t3_cloud::Secret as GuestToken;
pub use t3_cloud::guest::GuestSharing;
use t3_cloud::{
    Result as GuestResult,
    guest::{GuestEvents, GuestFailure},
};
use t3_contracts::{
    base_schemas::{GuestShareId, ThreadId, TurnItemId},
    chat_attachment::ChatAttachment,
    guest_share::ThreadGuestMessageStatus,
    guest_stream::GuestStreamEvent,
    orchestration_v2::{
        OrchestrationV2CommandOutputFrame, OrchestrationV2GetTurnItemResult,
        OrchestrationV2ThreadHistoryPage, OrchestrationV2TurnItem,
    },
};
use t3_contracts::{
    guest_stream::{GuestShareView, GuestThreadModel},
    model_selection::ModelSelection,
    orchestration_v2::OrchestrationV2AppThread,
    server::ServerProvider,
};

pub type CommandOutput =
    Pin<Box<dyn Stream<Item = GuestResult<OrchestrationV2CommandOutputFrame>> + Send>>;
#[derive(Debug, Clone)]
pub struct RenderAttachment {
    pub attachment_id: String,
    pub title: String,
}
#[derive(Debug, Clone)]
pub struct GuestFile {
    pub path: PathBuf,
    pub attachment: ChatAttachment,
}
#[derive(Debug, Clone)]
pub struct GuestRender {
    pub path: PathBuf,
    pub title: String,
}
#[derive(Debug, Clone)]
pub struct GuestUpload {
    pub name: String,
    pub content_type: String,
    pub path: PathBuf,
}
#[async_trait]
pub trait GuestUploadStore: Send + Sync {
    /// Copy at most draft.size bytes through the attachment owner. Clean a partial copy on cancellation.
    async fn store(
        &self,
        thread: &ThreadId,
        draft: &t3_cloud::guest::AttachmentDraft,
        source: &Path,
    ) -> GuestResult<ChatAttachment>;
    fn remove(&self, attachment: &ChatAttachment);
}

#[async_trait]
pub trait GuestContent: Send + Sync {
    /// Dispatch atomically: recheck enabled/host/expiry/share identity and pending file/message limits.
    async fn submit(
        &self,
        command: t3_contracts::orchestration_v2::InternalCommandThreadGuestMessageSubmit,
    ) -> GuestResult<()>;
    async fn thread(&self, thread: &ThreadId) -> GuestResult<OrchestrationV2AppThread>;
    async fn item(
        &self,
        thread: &ThreadId,
        item: &TurnItemId,
    ) -> GuestResult<Option<OrchestrationV2TurnItem>>;
    /// Apply the existing orchestration wire/detail projector; file diffs stay omitted.
    fn project_item(&self, item: &mut OrchestrationV2TurnItem, detail: bool) -> GuestResult<()>;
    async fn history(
        &self,
        thread: &ThreadId,
        cursor: &str,
    ) -> GuestResult<OrchestrationV2ThreadHistoryPage>;
    async fn command_output(
        &self,
        thread: &ThreadId,
        item: &TurnItemId,
    ) -> GuestResult<CommandOutput>;
    async fn message_attachment(
        &self,
        thread: &ThreadId,
        id: &str,
        include_fork: bool,
    ) -> GuestResult<Option<ChatAttachment>>;
    /// Resolve through the owning attachment store, including its ID/path validation.
    fn attachment_path(&self, attachment: &ChatAttachment) -> GuestResult<PathBuf>;
    fn render_attachment(
        &self,
        thread: &ThreadId,
        item: &OrchestrationV2TurnItem,
    ) -> GuestResult<Option<(RenderAttachment, PathBuf)>>;
    /// Use the shared Markdown parser and host-destination decoder, including HTML images.
    fn host_image_paths(&self, markdown: &str) -> Vec<String>;
}
struct Session {
    id: ThreadId,
    thread: OrchestrationV2AppThread,
    share: GuestShareId,
}
struct Reply {
    key: String,
    text: String,
    sources: Option<Vec<String>>,
}
struct Sent {
    share: GuestShareId,
    open_streams: usize,
    replies: Vec<Reply>,
}
type SentReplies = Arc<Mutex<VecDeque<Sent>>>;
pub struct GuestShareContent {
    sharing: Arc<GuestSharing>,
    content: Arc<dyn GuestContent>,
    sent: SentReplies,
}
fn denied(reason: GuestFailure) -> t3_cloud::Error {
    t3_cloud::Error::Guest(reason)
}
fn unavailable() -> t3_cloud::Error {
    denied(GuestFailure::Unavailable)
}
impl GuestShareContent {
    pub fn new(sharing: Arc<GuestSharing>, content: Arc<dyn GuestContent>) -> Self {
        Self {
            sharing,
            content,
            sent: Arc::new(Mutex::new(VecDeque::new())),
        }
    }
    async fn authorize(&self, session: &GuestToken) -> GuestResult<Session> {
        self.sharing.check(session).await?;
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Claims {
            kind: String,
            thread_id: ThreadId,
            share_id: GuestShareId,
        }
        let payload = session.expose().split('.').next().ok_or_else(unavailable)?;
        let claims: Claims =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).map_err(|_| unavailable())?)
                .map_err(|_| unavailable())?;
        if claims.kind != "session" {
            return Err(unavailable());
        }
        let thread = self.content.thread(&claims.thread_id).await?;
        if thread.id
            != t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(claims.thread_id.clone())
            || thread.archived_at.is_some()
            || thread.deleted_at.is_some()
            || thread
                .guest_share
                .as_ref()
                .and_then(Option::as_ref)
                .is_none_or(|share| share.id != claims.share_id || share.guest_name.is_none())
        {
            return Err(denied(GuestFailure::Inactive));
        }
        Ok(Session {
            id: claims.thread_id,
            thread,
            share: claims.share_id,
        })
    }
    pub async fn session(&self, session: &GuestToken) -> GuestResult<()> {
        self.authorize(session).await.map(|_| ())
    }
    pub async fn submit(
        &self,
        session: &GuestToken,
        request: t3_contracts::guest_share::GuestSubmitRequest,
        uploads: &[GuestUpload],
        store: &dyn GuestUploadStore,
    ) -> GuestResult<()> {
        let context = self.authorize(session).await?;
        if request.share_id != context.share {
            return Err(denied(GuestFailure::StalePage));
        }
        if request.text.0.is_empty() && uploads.is_empty() {
            return Err(denied(GuestFailure::Empty));
        }
        if request.text.0.encode_utf16().count()
            > t3_contracts::guest_share::GUEST_MESSAGE_MAX_LENGTH
        {
            return Err(denied(GuestFailure::Rejected));
        }
        if uploads.len() > t3_contracts::guest_share::GUEST_MESSAGE_MAX_ATTACHMENTS {
            return Err(denied(GuestFailure::TooManyFiles));
        }
        let mut inspected = Vec::with_capacity(uploads.len());
        let mut headers = Vec::with_capacity(uploads.len());
        let mut sizes = Vec::with_capacity(uploads.len());
        for upload in uploads {
            let metadata = std::fs::symlink_metadata(&upload.path)
                .map_err(|_| denied(GuestFailure::Rejected))?;
            if !metadata.is_file() {
                return Err(denied(GuestFailure::Rejected));
            }
            let mut file = File::open(&upload.path).map_err(|_| denied(GuestFailure::Rejected))?;
            let mut header = vec![0_u8; 12];
            let count = file
                .read(&mut header)
                .map_err(|_| denied(GuestFailure::Rejected))?;
            header.truncate(count);
            sizes.push(metadata.len());
            headers.push(header);
        }
        for (index, upload) in uploads.iter().enumerate() {
            inspected.push(t3_cloud::guest::Upload {
                name: &upload.name,
                content_type: &upload.content_type,
                size: sizes[index],
                header: &headers[index],
            });
        }
        let drafts = t3_cloud::guest::classify_uploads(&inspected)?;
        let mut stored = UploadRollback {
            store,
            attachments: Vec::new(),
            committed: false,
        };
        for (upload, draft) in uploads.iter().zip(&drafts) {
            stored
                .attachments
                .push(store.store(&context.id, draft, &upload.path).await?);
        }
        use t3_contracts::base_schemas::{CommandId, GuestMessageId};
        let command = t3_contracts::orchestration_v2::InternalCommandThreadGuestMessageSubmit {
            command_id: CommandId(format!("guest-submit:{}", uuid::Uuid::new_v4())),
            thread_id: context.id,
            share_id: context.share,
            guest_message_id: GuestMessageId(uuid::Uuid::new_v4().to_string()),
            text: request.text.0,
            attachments: (!stored.attachments.is_empty()).then(|| Some(stored.attachments.clone())),
        };
        self.content.submit(command).await?;
        stored.committed = true;
        Ok(())
    }
    async fn thread_item(
        &self,
        session: &GuestToken,
        item_id: &str,
    ) -> GuestResult<(Session, OrchestrationV2TurnItem)> {
        let context = self.authorize(session).await?;
        let item = self
            .content
            .item(&context.id, &TurnItemId(item_id.into()))
            .await?
            .ok_or_else(unavailable)?;
        let value = t3_contracts::stored::to_value(&item).map_err(|_| unavailable())?;
        if value["threadId"].as_str() != Some(context.id.0.as_str())
            || value["id"].as_str() != Some(item_id)
        {
            return Err(unavailable());
        }
        Ok((context, item))
    }
    pub async fn item(
        &self,
        session: &GuestToken,
        item_id: &str,
    ) -> GuestResult<OrchestrationV2GetTurnItemResult> {
        let (_, mut item) = self.thread_item(session, item_id).await?;
        let detail = !matches!(item, OrchestrationV2TurnItem::FileChange(_));
        self.content.project_item(&mut item, detail)?;
        Ok(OrchestrationV2GetTurnItemResult { item: Some(item) })
    }
    pub async fn render(&self, session: &GuestToken, item_id: &str) -> GuestResult<GuestRender> {
        let (context, item) = self.thread_item(session, item_id).await?;
        if !matches!(item, OrchestrationV2TurnItem::DynamicTool(_)) {
            return Err(unavailable());
        }
        let (reference, path) = self
            .content
            .render_attachment(&context.id, &item)?
            .ok_or_else(unavailable)?;
        if reference.attachment_id.is_empty() {
            return Err(unavailable());
        }
        Ok(GuestRender {
            path,
            title: reference.title,
        })
    }
    pub async fn history(
        &self,
        session: &GuestToken,
        cursor: &str,
    ) -> GuestResult<OrchestrationV2ThreadHistoryPage> {
        let context = self.authorize(session).await?;
        let page = self.content.history(&context.id, cursor).await?;
        for row in &page.items {
            remember(&self.sent, &context.share, &row.item);
        }
        Ok(page)
    }
    pub async fn attachment(&self, session: &GuestToken, id: &str) -> GuestResult<GuestFile> {
        let context = self.authorize(session).await?;
        let share = context
            .thread
            .guest_share
            .as_ref()
            .and_then(Option::as_ref)
            .ok_or_else(unavailable)?;
        let own = share
            .messages
            .iter()
            .filter(|m| m.status != ThreadGuestMessageStatus::Dismissed)
            .flat_map(|m| {
                m.attachments
                    .as_ref()
                    .and_then(Option::as_ref)
                    .into_iter()
                    .flatten()
            })
            .find(|a| attachment_id(a) == Some(id))
            .cloned();
        let attachment = match own {
            Some(found) => found,
            None => self
                .content
                .message_attachment(&context.id, id, context.thread.forked_from.is_some())
                .await?
                .filter(|a| attachment_id(a) == Some(id))
                .ok_or_else(unavailable)?,
        };
        let path = self.content.attachment_path(&attachment)?;
        if !path.try_exists().map_err(|_| unavailable())? {
            return Err(unavailable());
        }
        Ok(GuestFile { path, attachment })
    }
    pub async fn command_output(
        &self,
        session: &GuestToken,
        item_id: &str,
    ) -> GuestResult<CommandOutput> {
        let (context, item) = self.thread_item(session, item_id).await?;
        if !matches!(item, OrchestrationV2TurnItem::CommandExecution(_)) {
            return Err(unavailable());
        }
        let output = self
            .content
            .command_output(&context.id, &TurnItemId(item_id.into()))
            .await?;
        let sharing = self.sharing.clone();
        let session = session.clone();
        Ok(Box::pin(futures_util::stream::unfold(
            (
                output,
                sharing,
                session,
                tokio::time::interval_at(
                    tokio::time::Instant::now() + std::time::Duration::from_secs(30),
                    std::time::Duration::from_secs(30),
                ),
            ),
            |(mut output, sharing, session, mut interval)| async move {
                use futures_util::StreamExt;
                loop {
                    tokio::select! {frame=output.next()=>{let frame=frame?.ok()?;if sharing.check(&session).await.is_err(){return None;}return Some((Ok(frame),(output,sharing,session,interval)));},_=interval.tick()=>{if sharing.check(&session).await.is_err(){return None;}}}
                }
            },
        )))
    }
    pub async fn stream(&self, session: &GuestToken) -> GuestResult<GuestEvents> {
        let context = self.authorize(session).await?;
        let inner = self.sharing.stream(session).await?;
        Ok(Box::pin(RecordedStream {
            inner,
            share: context.share,
            sent: self.sent.clone(),
            started: false,
            finished: false,
        }))
    }
    pub async fn media(&self, session: &GuestToken, path: &Path) -> GuestResult<(PathBuf, String)> {
        let context = self.authorize(session).await?;
        let mime = guest_host_image_type(path).ok_or_else(unavailable)?;
        let named = path.to_str().ok_or_else(unavailable)?;
        let visible = {
            let mut records = self
                .sent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            records
                .iter_mut()
                .find(|r| r.share == context.share)
                .is_some_and(|record| {
                    record.replies.iter_mut().any(|reply| {
                        reply
                            .sources
                            .get_or_insert_with(|| self.content.host_image_paths(&reply.text))
                            .iter()
                            .any(|source| source == named)
                    })
                })
        };
        if !visible {
            return Err(unavailable());
        }
        let metadata = std::fs::symlink_metadata(path).map_err(|_| unavailable())?;
        if !metadata.is_file() {
            return Err(unavailable());
        }
        let mut header = [0_u8; 12];
        let mut file = File::open(path).map_err(|_| unavailable())?;
        let read = file.read(&mut header).map_err(|_| unavailable())?;
        if !t3_cloud::guest::image_signature(mime, &header[..read]) {
            return Err(unavailable());
        }
        Ok((path.to_owned(), mime.into()))
    }
}
struct UploadRollback<'a> {
    store: &'a dyn GuestUploadStore,
    attachments: Vec<ChatAttachment>,
    committed: bool,
}
impl Drop for UploadRollback<'_> {
    fn drop(&mut self) {
        if !self.committed {
            for attachment in &self.attachments {
                self.store.remove(attachment);
            }
        }
    }
}
fn attachment_id(attachment: &ChatAttachment) -> Option<&str> {
    match attachment {
        ChatAttachment::Image(a) => Some(a.id.as_str()),
        ChatAttachment::File(a) => Some(a.id.as_str()),
        ChatAttachment::Unknown(_) => None,
    }
}
pub fn guest_host_image_type(path: &Path) -> Option<&'static str> {
    let text = path.to_str()?;
    if !path.is_absolute() || text.starts_with("\\\\") || text.starts_with("//") {
        return None;
    }
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}
fn sent_record<'a>(records: &'a mut VecDeque<Sent>, share: &GuestShareId) -> &'a mut Sent {
    if let Some(index) = records.iter().position(|r| &r.share == share) {
        return &mut records[index];
    }
    records.push_back(Sent {
        share: share.clone(),
        open_streams: 0,
        replies: Vec::new(),
    });
    while records.len() > 64 {
        let Some(index) = records
            .iter()
            .position(|r| r.open_streams == 0 && &r.share != share)
        else {
            break;
        };
        records.remove(index);
    }
    let index = records.len() - 1;
    &mut records[index]
}
fn remember(sent: &SentReplies, share: &GuestShareId, item: &OrchestrationV2TurnItem) {
    if let OrchestrationV2TurnItem::AssistantMessage(item) = item {
        let (
            t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(thread),
            t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(id),
        ) = (&item.thread_id, &item.id)
        else {
            return;
        };
        let key = format!("{}\0{}", thread.0, id.0);
        let text = &item.text;
        let mut records = sent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if text.contains("![") || text.to_ascii_lowercase().contains("<img") {
            let record = sent_record(&mut records, share);
            record.replies.retain(|r| r.key != key);
            record.replies.push(Reply {
                key,
                text: text.clone(),
                sources: None,
            });
        } else if let Some(record) = records.iter_mut().find(|r| &r.share == share) {
            record.replies.retain(|r| r.key != key);
        }
    }
}
struct RecordedStream {
    inner: GuestEvents,
    share: GuestShareId,
    sent: SentReplies,
    started: bool,
    finished: bool,
}
impl RecordedStream {
    fn release(&mut self) {
        if self.started {
            let mut records = self
                .sent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(record) = records.iter_mut().find(|r| r.share == self.share) {
                record.open_streams = record.open_streams.saturating_sub(1);
            }
            self.started = false;
        }
        self.finished = true;
    }
}
impl Stream for RecordedStream {
    type Item = GuestStreamEvent;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.finished {
            return Poll::Ready(None);
        }
        if !this.started {
            let mut records = this
                .sent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            sent_record(&mut records, &this.share).open_streams += 1;
            this.started = true;
        }
        let result = this.inner.as_mut().poll_next(cx);
        if let Poll::Ready(Some(GuestStreamEvent::Thread { item })) = &result {
            use t3_contracts::orchestration_v2::{
                OrchestrationV2DomainEvent, OrchestrationV2ThreadStreamItem,
            };
            match item.as_ref() {
                OrchestrationV2ThreadStreamItem::Snapshot(snapshot) => {
                    for row in &snapshot.projection.visible_turn_items {
                        remember(&this.sent, &this.share, &row.item);
                    }
                }
                OrchestrationV2ThreadStreamItem::Event(event) => {
                    if let OrchestrationV2DomainEvent::TurnItemUpdated(event) = &event.event {
                        remember(&this.sent, &this.share, &event.payload);
                    }
                }
                _ => {}
            }
        }
        if matches!(result, Poll::Ready(None | Some(GuestStreamEvent::Ended))) {
            this.release();
        }
        result
    }
}
impl Drop for RecordedStream {
    fn drop(&mut self) {
        self.release();
    }
}

pub fn guest_thread_model(
    selection: &ModelSelection,
    providers: &[ServerProvider],
) -> GuestThreadModel {
    let provider = providers
        .iter()
        .find(|p| p.instance_id == selection.instance_id);
    let model = provider.and_then(|p| p.models.iter().find(|m| m.slug.0 == selection.model));
    GuestThreadModel {
        driver: provider.map(|p| p.driver.clone()),
        name: model.map_or_else(
            || t3_contracts::base_schemas::TrimmedNonEmptyString(selection.model.clone()),
            |m| m.name.clone(),
        ),
        short_name: model.and_then(|m| m.short_name.clone()),
        sub_provider: model.and_then(|m| m.sub_provider.clone()),
    }
}
pub fn guest_share_view(
    thread: &OrchestrationV2AppThread,
    model: GuestThreadModel,
) -> Option<GuestShareView> {
    let share = thread.guest_share.as_ref()?.as_ref()?;
    Some(GuestShareView {
        share_id: share.id.clone(),
        title: thread.title.0.clone(),
        guest_name: share.guest_name.clone()?,
        expires_at: share.expires_at.clone(),
        messages: share.messages.clone(),
        model,
    })
}
