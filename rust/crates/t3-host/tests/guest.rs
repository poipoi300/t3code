use async_trait::async_trait;
use futures_util::{FutureExt, StreamExt};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use t3_cloud::{
    Error, Result, Secret,
    guest::{AttachmentDraft, GuestEvents, GuestFailure, GuestThread, GuestThreads},
    http::Sensitive,
    store::Secrets,
};
use t3_contracts::{
    base_schemas::*, chat_attachment::ChatAttachment, guest_share::*, orchestration_v2::*,
};
use t3_host::guest::*;

struct Time;
impl t3_cloud::Clock for Time {
    fn now_ms(&self) -> i64 {
        1_700_000_000_000
    }
}
struct Disk(t3_config::secrets::SecretStore);
#[async_trait]
impl Secrets for Disk {
    async fn get(&self, n: &str) -> Result<Option<Vec<u8>>> {
        self.0.get(n).map_err(|_| Error::Storage)
    }
    async fn set(&self, n: &str, b: &[u8]) -> Result<()> {
        self.0.set(n, b).await.map_err(|_| Error::Storage)
    }
    async fn create(&self, n: &str, b: &[u8]) -> Result<bool> {
        match self.0.create(n, b) {
            Ok(()) => Ok(true),
            Err(_) if self.0.get(n).map_err(|_| Error::Storage)?.is_some() => Ok(false),
            Err(_) => Err(Error::Storage),
        }
    }
    async fn remove(&self, n: &str) -> Result<()> {
        self.0.remove(n).map_err(|_| Error::Storage)
    }
}
fn item(kind: &str, text: &str) -> OrchestrationV2TurnItem {
    let mut value = include_str!("../../../fixtures/samples/OrchestrationV2TurnItem.jsonl")
        .lines()
        .map(|s| serde_json::from_str::<Value>(s).unwrap())
        .find(|v| v["type"] == kind)
        .unwrap();
    value["id"] = json!("item");
    value["threadId"] = json!("thread");
    if kind == "assistant_message" {
        value["text"] = json!(text);
    }
    serde_json::from_value(value).unwrap()
}
fn attachment(id: &str) -> ChatAttachment {
    serde_json::from_value(
        json!({"type":"file","id":id,"name":"file.txt","mimeType":"text/plain","sizeBytes":1}),
    )
    .unwrap()
}
fn attachment_id(a: &ChatAttachment) -> &str {
    match a {
        ChatAttachment::Image(a) => &a.id,
        ChatAttachment::File(a) => &a.id,
        ChatAttachment::Unknown(a) => &a.id,
    }
}
struct Host {
    thread: Mutex<OrchestrationV2AppThread>,
    item: Mutex<OrchestrationV2TurnItem>,
    enabled: AtomicBool,
    access_checks: AtomicUsize,
    output: Mutex<Vec<OrchestrationV2CommandOutputFrame>>,
    fail_submit: AtomicBool,
    submitted: Mutex<Vec<InternalCommandThreadGuestMessageSubmit>>,
    reads: AtomicUsize,
    parses: AtomicUsize,
    details: Mutex<Vec<bool>>,
    files: PathBuf,
}
impl Host {
    fn new(files: PathBuf) -> Self {
        let mut value: Value = serde_json::from_str(
            include_str!("../../../fixtures/stored/OrchestrationV2AppThreadJson.jsonl")
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        value["id"] = json!("thread");
        value["archivedAt"] = Value::Null;
        value["deletedAt"] = Value::Null;
        Self {
            thread: Mutex::new(serde_json::from_value(value).unwrap()),
            item: Mutex::new(item("assistant_message", "hello")),
            enabled: AtomicBool::new(true),
            access_checks: AtomicUsize::new(0),
            output: Mutex::new(vec![]),
            fail_submit: AtomicBool::new(false),
            submitted: Mutex::new(vec![]),
            reads: AtomicUsize::new(0),
            parses: AtomicUsize::new(0),
            details: Mutex::new(vec![]),
            files,
        }
    }
    fn share(&self) -> ThreadGuestShare {
        self.thread
            .lock()
            .unwrap()
            .guest_share
            .clone()
            .flatten()
            .unwrap()
    }
}
#[async_trait]
impl GuestThreads for Host {
    async fn enabled(&self) -> Result<bool> {
        self.access_checks.fetch_add(1, Ordering::SeqCst);
        Ok(self.enabled.load(Ordering::SeqCst))
    }
    async fn connect_hostname(&self) -> Result<Option<String>> {
        Ok(Some("env.test".into()))
    }
    async fn find_share(&self, id: &GuestShareId) -> Result<Option<ThreadId>> {
        Ok((self.share().id == *id).then(|| ThreadId("thread".into())))
    }
    async fn read(&self, id: &ThreadId) -> Result<GuestThread> {
        let t = self.thread.lock().unwrap();
        Ok(GuestThread {
            thread_id: id.clone(),
            share: t.guest_share.clone().flatten(),
            archived: t.archived_at.is_some(),
            deleted: t.deleted_at.is_some(),
        })
    }
    async fn open(&self, _: &ThreadId, s: ThreadGuestShare) -> Result<()> {
        self.thread.lock().unwrap().guest_share = Some(Some(s));
        Ok(())
    }
    async fn redeem(&self, _: &ThreadId, id: &GuestShareId, name: &str) -> Result<()> {
        let mut t = self.thread.lock().unwrap();
        let s = t.guest_share.as_mut().unwrap().as_mut().unwrap();
        if &s.id != id || s.guest_name.is_some() {
            return Err(Error::Guest(GuestFailure::Used));
        }
        s.guest_name = Some(TrimmedNonEmptyString(name.into()));
        Ok(())
    }
    async fn submit(&self, _: &ThreadId, _: GuestSubmitRequest) -> Result<()> {
        unreachable!("host submit must carry attachments")
    }
    async fn stream(&self, _: &ThreadId, _: &GuestShareId) -> Result<GuestEvents> {
        Ok(Box::pin(futures_util::stream::pending()))
    }
}
#[async_trait]
impl GuestContent for Host {
    async fn submit(&self, c: InternalCommandThreadGuestMessageSubmit) -> Result<()> {
        if self.fail_submit.load(Ordering::SeqCst) {
            return Err(Error::Guest(GuestFailure::FilesBusy));
        }
        self.submitted.lock().unwrap().push(c);
        Ok(())
    }
    async fn thread(&self, _: &ThreadId) -> Result<OrchestrationV2AppThread> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.thread.lock().unwrap().clone())
    }
    async fn item(&self, _: &ThreadId, _: &TurnItemId) -> Result<Option<OrchestrationV2TurnItem>> {
        Ok(Some(self.item.lock().unwrap().clone()))
    }
    fn project_item(&self, _: &mut OrchestrationV2TurnItem, detail: bool) -> Result<()> {
        self.details.lock().unwrap().push(detail);
        Ok(())
    }
    async fn history(&self, _: &ThreadId, _: &str) -> Result<OrchestrationV2ThreadHistoryPage> {
        let item = t3_contracts::stored::to_value(&*self.item.lock().unwrap()).unwrap();
        Ok(serde_json::from_value(json!({"snapshotSequence":1,"items":[{"position":0,"visibility":"local","sourceThreadId":"thread","sourceItemId":"item","item":item}],"nextCursor":null,"hasMoreHistory":false})).unwrap())
    }
    async fn command_output(&self, _: &ThreadId, _: &TurnItemId) -> Result<CommandOutput> {
        let frames = std::mem::take(&mut *self.output.lock().unwrap());
        Ok(Box::pin(
            futures_util::stream::iter(frames.into_iter().map(Ok))
                .chain(futures_util::stream::pending()),
        ))
    }
    async fn message_attachment(
        &self,
        _: &ThreadId,
        id: &str,
        _: bool,
    ) -> Result<Option<ChatAttachment>> {
        Ok((id == "owner").then(|| attachment(id)))
    }
    fn attachment_path(&self, a: &ChatAttachment) -> Result<PathBuf> {
        Ok(self.files.join(attachment_id(a)))
    }
    fn render_attachment(
        &self,
        _: &ThreadId,
        _: &OrchestrationV2TurnItem,
    ) -> Result<Option<(RenderAttachment, PathBuf)>> {
        Ok(None)
    }
    fn host_image_paths(&self, markdown: &str) -> Vec<String> {
        self.parses.fetch_add(1, Ordering::SeqCst);
        markdown
            .strip_prefix("![image](")
            .and_then(|s| s.strip_suffix(')'))
            .map(|s| vec![s.into()])
            .unwrap_or_default()
    }
}
struct Fixture {
    _dir: tempfile::TempDir,
    host: Arc<Host>,
    sharing: Arc<GuestSharing>,
    service: GuestShareContent,
    token: Secret,
}
async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::new(dir.path().to_owned()));
    let sharing = Arc::new(GuestSharing::new(
        host.clone(),
        Arc::new(Disk(t3_config::secrets::SecretStore::new(
            dir.path().join("secrets"),
        ))),
        Arc::new(Time),
    ));
    let created = sharing
        .create(GuestShareCreateInput {
            thread_id: ThreadId("thread".into()),
        })
        .await
        .unwrap();
    let invite = created.expose().url.0.split_once('#').unwrap().1;
    let (token, _) = sharing
        .redeem(Sensitive::new(GuestRedeemRequest {
            credential: TrimmedNonEmptyString(invite.into()),
            name: TrimmedNonEmptyString("Guest".into()),
        }))
        .await
        .unwrap();
    let service = GuestShareContent::new(sharing.clone(), host.clone());
    Fixture {
        _dir: dir,
        host,
        sharing,
        service,
        token,
    }
}
#[tokio::test]
async fn reads_require_a_signed_live_session_and_recheck_thread_and_item_identity() {
    let f = fixture().await;
    let forged = Secret::new(format!("{}x", f.token.expose()));
    assert!(f.service.item(&forged, "item").await.is_err());
    assert_eq!(f.host.reads.load(Ordering::SeqCst), 0);
    f.service.item(&f.token, "item").await.unwrap();
    assert_eq!(*f.host.details.lock().unwrap(), vec![true]);
    assert!(f.service.item(&f.token, "other-item").await.is_err());
    *f.host.item.lock().unwrap() = item("file_change", "");
    f.service.item(&f.token, "item").await.unwrap();
    assert_eq!(*f.host.details.lock().unwrap(), vec![true, false]);
    let mut wrong = t3_contracts::stored::to_value(&*f.host.item.lock().unwrap()).unwrap();
    wrong["threadId"] = json!("another-thread");
    *f.host.item.lock().unwrap() = serde_json::from_value(wrong).unwrap();
    assert!(f.service.item(&f.token, "item").await.is_err());
    assert!(!format!("{:?}", f.token).contains(f.token.expose()));
    let err = f.service.item(&forged, "item").await.unwrap_err();
    assert!(!format!("{err:?} {err}").contains(f.token.expose()));
    f.sharing
        .create(GuestShareCreateInput {
            thread_id: ThreadId("thread".into()),
        })
        .await
        .unwrap();
    assert!(f.service.session(&f.token).await.is_err());
}
#[tokio::test]
async fn media_requires_a_delivered_reply_exact_path_and_image_signature_and_drops_edited_sources()
{
    let f = fixture().await;
    let path = f._dir.path().join("picture.png");
    fs::write(&path, b"\x89PNG\r\n\x1a\nrest").unwrap();
    *f.host.item.lock().unwrap() = item(
        "assistant_message",
        &format!("![image]({})", path.display()),
    );
    assert!(f.service.media(&f.token, &path).await.is_err());
    f.service.history(&f.token, "cursor").await.unwrap();
    assert_eq!(f.host.parses.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.service.media(&f.token, &path).await.unwrap().1,
        "image/png"
    );
    f.service.media(&f.token, &path).await.unwrap();
    assert_eq!(f.host.parses.load(Ordering::SeqCst), 1);
    fs::write(&path, b"<html>not png</html>").unwrap();
    assert!(f.service.media(&f.token, &path).await.is_err());
    fs::write(&path, b"\x89PNG\r\n\x1a\n").unwrap();
    *f.host.item.lock().unwrap() = item("assistant_message", "image removed");
    f.service.history(&f.token, "cursor").await.unwrap();
    assert!(f.service.media(&f.token, &path).await.is_err());
    for p in [
        "relative.png",
        "\\\\server\\share\\a.png",
        "//server/share/a.jpg",
        "C:\\path\\a.svg",
    ] {
        assert!(guest_host_image_type(Path::new(p)).is_none());
    }
}
#[tokio::test(start_paused = true)]
async fn output_only_opens_command_rows_and_terminates_when_access_is_revoked() {
    let f = fixture().await;
    assert!(f.service.command_output(&f.token, "item").await.is_err());
    *f.host.item.lock().unwrap() = item("command_execution", "");
    let frame: OrchestrationV2CommandOutputFrame = serde_json::from_value(
        json!({"kind":"append","text":"output","truncated":false,"running":true}),
    )
    .unwrap();
    *f.host.output.lock().unwrap() = vec![frame.clone(); 100];
    let mut output = f.service.command_output(&f.token, "item").await.unwrap();
    assert_eq!(output.next().await.unwrap().unwrap(), frame);
    let checks = f.host.access_checks.load(Ordering::SeqCst);
    for _ in 1..100 {
        assert_eq!(output.next().await.unwrap().unwrap(), frame);
    }
    assert_eq!(
        f.host.access_checks.load(Ordering::SeqCst),
        checks,
        "streaming frames must not reread access state"
    );
    let mut stream = f.service.stream(&f.token).await.unwrap();
    assert!(stream.next().now_or_never().is_none());
    assert!(output.next().now_or_never().is_none());
    f.host.enabled.store(false, Ordering::SeqCst);
    tokio::time::advance(std::time::Duration::from_secs(30)).await;
    assert!(output.next().await.is_none());
    assert!(matches!(
        stream.next().await,
        Some(t3_contracts::guest_stream::GuestStreamEvent::Ended)
    ));
    assert!(stream.next().await.is_none());
}
#[tokio::test]
async fn dismissed_share_files_are_not_readable_and_owner_message_files_remain_readable() {
    let f = fixture().await;
    fs::write(f._dir.path().join("guest"), b"g").unwrap();
    fs::write(f._dir.path().join("owner"), b"o").unwrap();
    {
        let mut t = f.host.thread.lock().unwrap();
        t.guest_share
            .as_mut()
            .unwrap()
            .as_mut()
            .unwrap()
            .messages
            .push(ThreadGuestMessage {
                id: GuestMessageId("message".into()),
                text: "".into(),
                status: ThreadGuestMessageStatus::Pending,
                submitted_at: t3_contracts::wire::WireDateTime("2023-11-14T22:13:20.000Z".into()),
                attachments: Some(Some(vec![attachment("guest")])),
            });
    }
    assert_eq!(
        fs::read(f.service.attachment(&f.token, "guest").await.unwrap().path).unwrap(),
        b"g"
    );
    f.host
        .thread
        .lock()
        .unwrap()
        .guest_share
        .as_mut()
        .unwrap()
        .as_mut()
        .unwrap()
        .messages[0]
        .status = ThreadGuestMessageStatus::Dismissed;
    assert!(f.service.attachment(&f.token, "guest").await.is_err());
    assert_eq!(
        fs::read(f.service.attachment(&f.token, "owner").await.unwrap().path).unwrap(),
        b"o"
    );
}
struct Files {
    dir: PathBuf,
    count: AtomicUsize,
    fail_after: usize,
}
#[async_trait]
impl GuestUploadStore for Files {
    async fn store(
        &self,
        _: &ThreadId,
        draft: &AttachmentDraft,
        source: &Path,
    ) -> Result<ChatAttachment> {
        let n = self.count.fetch_add(1, Ordering::SeqCst);
        if n >= self.fail_after {
            return Err(Error::Storage);
        }
        let id = format!("stored-{n}");
        fs::copy(source, self.dir.join(&id)).unwrap();
        Ok(serde_json::from_value(json!({"type":if draft.image{"image"}else{"file"},"id":id,"name":draft.name,"mimeType":draft.mime_type,"sizeBytes":draft.size})).unwrap())
    }
    fn remove(&self, a: &ChatAttachment) {
        fs::remove_file(self.dir.join(attachment_id(a))).unwrap();
    }
}
#[tokio::test]
async fn uploads_enforce_limits_and_roll_back_real_files_on_later_copy_or_dispatch_failure() {
    let f = fixture().await;
    let source = f._dir.path().join("source");
    fs::write(&source, b"hello").unwrap();
    let dest = tempfile::tempdir().unwrap();
    let upload = GuestUpload {
        name: "..\\invoice\u{202e}.txt".into(),
        content_type: "text/plain; charset=utf8".into(),
        path: source.clone(),
    };
    let request = || GuestSubmitRequest {
        share_id: f.host.share().id,
        text: TrimmedString("".into()),
    };
    let files = Files {
        dir: dest.path().into(),
        count: AtomicUsize::new(0),
        fail_after: 1,
    };
    assert!(
        f.service
            .submit(
                &f.token,
                request(),
                &[upload.clone(), upload.clone()],
                &files
            )
            .await
            .is_err()
    );
    assert_eq!(fs::read_dir(dest.path()).unwrap().count(), 0);
    let files = Files {
        dir: dest.path().into(),
        count: AtomicUsize::new(0),
        fail_after: usize::MAX,
    };
    f.host.fail_submit.store(true, Ordering::SeqCst);
    assert!(
        f.service
            .submit(&f.token, request(), std::slice::from_ref(&upload), &files)
            .await
            .is_err()
    );
    assert_eq!(fs::read_dir(dest.path()).unwrap().count(), 0);
    f.host.fail_submit.store(false, Ordering::SeqCst);
    f.service
        .submit(&f.token, request(), std::slice::from_ref(&upload), &files)
        .await
        .unwrap();
    assert_eq!(fs::read_dir(dest.path()).unwrap().count(), 1);
    let c = f.host.submitted.lock().unwrap()[0].clone();
    assert!(c.text.is_empty());
    let a = &c.attachments.unwrap().unwrap()[0];
    let value = t3_contracts::stored::to_value(a).unwrap();
    assert_eq!(value["name"], ".._invoice_.txt");
    assert_eq!(value["mimeType"], "text/plain");
    let unsupported = GuestUpload {
        content_type: "image/png".into(),
        ..upload.clone()
    };
    assert!(matches!(
        f.service
            .submit(&f.token, request(), &[unsupported], &files)
            .await,
        Err(Error::Guest(GuestFailure::UnsupportedFile))
    ));
    assert!(matches!(
        f.service
            .submit(&f.token, request(), &vec![upload.clone(); 5], &files)
            .await,
        Err(Error::Guest(GuestFailure::TooManyFiles))
    ));
    fs::OpenOptions::new()
        .write(true)
        .open(source)
        .unwrap()
        .set_len(GUEST_MESSAGE_MAX_ATTACHMENT_BYTES + 1)
        .unwrap();
    assert!(matches!(
        f.service
            .submit(&f.token, request(), &[upload], &files)
            .await,
        Err(Error::Guest(GuestFailure::TooLarge))
    ));
}
