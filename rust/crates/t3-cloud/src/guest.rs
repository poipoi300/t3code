use crate::{Error, Result, Secret, SharedClock, crypto, http::Sensitive, iso, store::SecretStore};
use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{pin::Pin, sync::Arc};
use subtle::ConstantTimeEq;
use t3_contracts::{
    base_schemas::{GuestShareId, ThreadId},
    guest_share::*,
    guest_stream::GuestStreamEvent,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestFailure {
    Invalid,
    Inactive,
    Used,
    Busy,
    StalePage,
    Unavailable,
    Rejected,
    Empty,
    TooManyFiles,
    TooLarge,
    UnsupportedFile,
    FilesBusy,
}
fn fail(reason: GuestFailure) -> Error {
    Error::Guest(reason)
}
#[derive(Debug, Clone)]
pub struct GuestThread {
    pub thread_id: ThreadId,
    pub share: Option<ThreadGuestShare>,
    pub archived: bool,
    pub deleted: bool,
}
pub type GuestEvents = Pin<Box<dyn Stream<Item = GuestStreamEvent> + Send>>;
/// Adapter to the event-sourced thread service; mutations must recheck share identity,
/// expiry and one-use redemption atomically when the command commits.
#[async_trait]
pub trait GuestThreads: Send + Sync {
    async fn enabled(&self) -> Result<bool>;
    async fn connect_hostname(&self) -> Result<Option<String>>;
    async fn find_share(&self, id: &GuestShareId) -> Result<Option<ThreadId>>;
    async fn read(&self, id: &ThreadId) -> Result<GuestThread>;
    async fn open(&self, thread: &ThreadId, share: ThreadGuestShare) -> Result<()>;
    async fn redeem(&self, thread: &ThreadId, share: &GuestShareId, name: &str) -> Result<()>;
    async fn submit(&self, thread: &ThreadId, request: GuestSubmitRequest) -> Result<()>;
    /// Must project the guest view and terminate on share replacement/closure.
    async fn stream(&self, thread: &ThreadId, share: &GuestShareId) -> Result<GuestEvents>;
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionClaims {
    kind: String,
    thread_id: ThreadId,
    share_id: GuestShareId,
}
pub struct GuestSharing {
    threads: Arc<dyn GuestThreads>,
    secrets: SecretStore,
    clock: SharedClock,
}
impl GuestSharing {
    pub fn new(threads: Arc<dyn GuestThreads>, secrets: SecretStore, clock: SharedClock) -> Self {
        Self {
            threads,
            secrets,
            clock,
        }
    }
    async fn key(&self) -> Result<zeroize::Zeroizing<Vec<u8>>> {
        const NAME: &str = "guest-share-signing-key";
        if let Some(key) = self.secrets.get(NAME).await? {
            return Ok(zeroize::Zeroizing::new(key));
        }
        let mut key = zeroize::Zeroizing::new(vec![0; 32]);
        getrandom::fill(&mut key).map_err(|_| Error::Proof)?;
        if self.secrets.create(NAME, &key).await? {
            return Ok(key);
        }
        Ok(zeroize::Zeroizing::new(
            self.secrets.get(NAME).await?.ok_or(Error::Storage)?,
        ))
    }
    async fn signature(&self, payload: &str, truncate: bool) -> Result<String> {
        let key = self.key().await?;
        if key.is_empty() {
            return Err(Error::Storage);
        }
        let mut mac = Hmac::<Sha256>::new_from_slice(&key).map_err(|_| Error::Proof)?;
        mac.update(payload.as_bytes());
        let bytes = mac.finalize().into_bytes();
        Ok(crypto::b64(if truncate {
            &bytes[..16]
        } else {
            &bytes[..]
        }))
    }
    pub async fn invite(&self, share: &GuestShareId) -> Result<Secret> {
        Ok(Secret::new(format!(
            "{}.{}",
            share.0,
            self.signature(&format!("guest-invite:{}", share.0), true)
                .await?
        )))
    }
    async fn session(&self, claims: &SessionClaims) -> Result<Secret> {
        let payload = crypto::b64(serde_json::to_vec(claims).map_err(|_| Error::Protocol)?);
        Ok(Secret::new(format!(
            "{payload}.{}",
            self.signature(&payload, false).await?
        )))
    }
    async fn authorize(&self, token: &Secret, invite: bool) -> Result<GuestThread> {
        if token.expose().len() > 16 * 1024 {
            return Err(fail(GuestFailure::Invalid));
        }
        let mut parts = token.expose().split('.');
        let payload = parts
            .next()
            .filter(|s| !s.is_empty())
            .ok_or(fail(GuestFailure::Invalid))?;
        let signature = parts
            .next()
            .filter(|s| !s.is_empty())
            .ok_or(fail(GuestFailure::Invalid))?;
        if parts.next().is_some() {
            return Err(fail(GuestFailure::Invalid));
        }
        let expected = self
            .signature(
                &if invite {
                    format!("guest-invite:{payload}")
                } else {
                    payload.to_owned()
                },
                invite,
            )
            .await?;
        let actual = crypto::unb64(signature).map_err(|_| fail(GuestFailure::Invalid))?;
        let expected = crypto::unb64(&expected).map_err(|_| fail(GuestFailure::Invalid))?;
        if !bool::from(actual.ct_eq(&expected)) {
            return Err(fail(GuestFailure::Invalid));
        }
        let claims = if invite {
            let share_id = GuestShareId(payload.to_owned());
            let thread_id = self
                .threads
                .find_share(&share_id)
                .await?
                .ok_or(fail(GuestFailure::Invalid))?;
            SessionClaims {
                kind: "session".into(),
                thread_id,
                share_id,
            }
        } else {
            serde_json::from_slice::<SessionClaims>(
                &crypto::unb64(payload).map_err(|_| fail(GuestFailure::Invalid))?,
            )
            .map_err(|_| fail(GuestFailure::Invalid))?
        };
        if claims.kind != "session" {
            return Err(fail(GuestFailure::Invalid));
        }
        let thread = self.threads.read(&claims.thread_id).await?;
        let share = thread.share.as_ref().ok_or(fail(GuestFailure::Inactive))?;
        let expires = chrono::DateTime::parse_from_rfc3339(&share.expires_at.0)
            .map_err(|_| fail(GuestFailure::Inactive))?
            .timestamp_millis();
        if !self.threads.enabled().await?
            || thread.archived
            || thread.deleted
            || share.id != claims.share_id
            || self.clock.now_ms() >= expires
            || (!invite && share.guest_name.is_none())
            || self.threads.connect_hostname().await?.as_deref() != Some(share.host.0.as_str())
        {
            return Err(fail(GuestFailure::Inactive));
        }
        Ok(thread)
    }
    pub async fn create(
        &self,
        input: GuestShareCreateInput,
    ) -> Result<Sensitive<GuestShareCreateResult>> {
        if !self.threads.enabled().await? {
            return Err(Error::Disabled);
        }
        let host = self
            .threads
            .connect_hostname()
            .await?
            .ok_or(Error::NotLinked)?;
        let parsed =
            url::Url::parse(&format!("https://{host}")).map_err(|_| Error::Configuration)?;
        if parsed.host_str() != Some(&host)
            || parsed.port().is_some()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.path() != "/"
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(Error::Configuration);
        }
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).map_err(|_| Error::Proof)?;
        let id = GuestShareId(crypto::b64(bytes));
        let now = self.clock.now_ms();
        let share = ThreadGuestShare {
            id: id.clone(),
            host: crate::text(host.clone()),
            created_at: t3_contracts::wire::WireDateTime(iso(now)?),
            expires_at: t3_contracts::wire::WireDateTime(iso(now + GUEST_SHARE_TTL_MS as i64)?),
            guest_name: None,
            messages: Vec::new(),
        };
        self.threads.open(&input.thread_id, share).await?;
        let thread = self.threads.read(&input.thread_id).await?;
        let share = thread
            .share
            .filter(|s| s.id == id)
            .ok_or(Error::Superseded)?;
        let invite = self.invite(&id).await?;
        Ok(Sensitive::new(GuestShareCreateResult {
            url: crate::text(format!("https://{host}/guest#{}", invite.expose())),
            share,
        }))
    }
    pub async fn redeem(
        &self,
        request: Sensitive<GuestRedeemRequest>,
    ) -> Result<(Secret, GuestShareId)> {
        let request = request.expose();
        let name = t3_contracts::wire::trim_js(&request.name.0);
        if !valid_guest_name(name) {
            return Err(fail(GuestFailure::Invalid));
        }
        let thread = self
            .authorize(&Secret::new(request.credential.0.as_str()), true)
            .await?;
        let share = thread.share.as_ref().ok_or(fail(GuestFailure::Inactive))?;
        self.threads
            .redeem(&thread.thread_id, &share.id, name)
            .await?;
        Ok((
            self.session(&SessionClaims {
                kind: "session".into(),
                thread_id: thread.thread_id,
                share_id: share.id.clone(),
            })
            .await?,
            share.id.clone(),
        ))
    }
    pub async fn check(&self, session: &Secret) -> Result<()> {
        self.authorize(session, false).await.map(|_| ())
    }
    /// Resolve a named share from its own verified cookie. The caller never supplies a thread id.
    pub async fn thread_for_session(
        &self,
        session: &Secret,
        share: &GuestShareId,
    ) -> Result<ThreadId> {
        let thread = self.authorize(session, false).await?;
        if thread
            .share
            .as_ref()
            .is_none_or(|current| &current.id != share)
        {
            return Err(fail(GuestFailure::Invalid));
        }
        Ok(thread.thread_id)
    }

    pub async fn submit(&self, session: &Secret, request: GuestSubmitRequest) -> Result<()> {
        let thread = self.authorize(session, false).await?;
        if thread
            .share
            .as_ref()
            .is_none_or(|s| s.id != request.share_id)
        {
            return Err(fail(GuestFailure::StalePage));
        }
        let text = request.text.0.as_str();
        if t3_contracts::wire::trim_js(text).is_empty() {
            return Err(fail(GuestFailure::Empty));
        }
        if text.encode_utf16().count() > GUEST_MESSAGE_MAX_LENGTH {
            return Err(fail(GuestFailure::Rejected));
        }
        self.threads.submit(&thread.thread_id, request).await
    }
    pub async fn stream(&self, session: &Secret) -> Result<GuestEvents> {
        let thread = self.authorize(session, false).await?;
        let share = thread.share.as_ref().ok_or(fail(GuestFailure::Inactive))?;
        let events = self.threads.stream(&thread.thread_id, &share.id).await?;
        let expires = chrono::DateTime::parse_from_rfc3339(&share.expires_at.0)
            .map_err(|_| fail(GuestFailure::Inactive))?
            .timestamp_millis();
        let remaining = expires.saturating_sub(self.clock.now_ms()).max(0) as u64;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(remaining);
        let host = share.host.0.clone();
        let auth = Self::new(
            self.threads.clone(),
            self.secrets.clone(),
            self.clock.clone(),
        );
        let token = session.clone();
        let spacing = std::time::Duration::from_secs(30);
        let interval = tokio::time::interval_at(tokio::time::Instant::now() + spacing, spacing);
        Ok(Box::pin(futures_util::stream::unfold(
            (events, auth, token, interval, host, false),
            move |(mut events, auth, token, mut interval, host, done)| async move {
                if done {
                    return None;
                }
                loop {
                    let mut event = tokio::select! {
                        biased;
                        () = tokio::time::sleep_until(deadline) => {
                            return Some((GuestStreamEvent::Ended, (events, auth, token, interval, host, true)));
                        }
                        _ = interval.tick() => None,
                        event = events.next() => Some(event?),
                    };
                    if auth.check(&token).await.is_err() {
                        // The browser reconnects after a temporary settings or tunnel interruption.
                        if !auth.threads.enabled().await.unwrap_or(false)
                            || auth
                                .threads
                                .connect_hostname()
                                .await
                                .ok()
                                .flatten()
                                .as_deref()
                                != Some(&host)
                        {
                            return None;
                        }
                        event = Some(GuestStreamEvent::Ended);
                    }
                    if let Some(event) = event {
                        let ended = matches!(event, GuestStreamEvent::Ended);
                        return Some((event, (events, auth, token, interval, host, ended)));
                    }
                }
            },
        )))
    }
}

/// The guest name is used in the owner's UI and the agent's attribution line.
pub fn valid_guest_name(name: &str) -> bool {
    !name.is_empty()
        && name.encode_utf16().count() <= GUEST_NAME_MAX_LENGTH
        && !name.chars().any(|ch| {
            ch.is_control()
                || matches!(ch,
                    '\u{061c}' | '\u{200e}' | '\u{200f}'
                    | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
                )
        })
}

#[derive(Debug)]
pub struct Upload<'a> {
    pub name: &'a str,
    pub content_type: &'a str,
    pub size: u64,
    pub header: &'a [u8],
}
#[derive(Debug, PartialEq, Eq)]
pub struct AttachmentDraft {
    pub image: bool,
    pub name: String,
    pub mime_type: String,
    pub size: u64,
}
pub fn image_signature(mime: &str, header: &[u8]) -> bool {
    match mime {
        "image/png" => header.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => header.starts_with(b"\xff\xd8\xff"),
        "image/gif" => header.starts_with(b"GIF8"),
        "image/webp" => header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WEBP"),
        _ => false,
    }
}
pub fn classify_uploads(uploads: &[Upload<'_>]) -> Result<Vec<AttachmentDraft>> {
    if uploads.len() > GUEST_MESSAGE_MAX_ATTACHMENTS {
        return Err(fail(GuestFailure::TooManyFiles));
    }
    let total = uploads
        .iter()
        .try_fold(0_u64, |sum, u| sum.checked_add(u.size))
        .ok_or(fail(GuestFailure::TooLarge))?;
    if total > GUEST_MESSAGE_MAX_ATTACHMENT_BYTES {
        return Err(fail(GuestFailure::TooLarge));
    }
    uploads.iter().map(|u|{
        let mime=u.content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
        let image=mime.starts_with("image/");
        if u.size==0 || image && !image_signature(&mime,u.header){return Err(fail(GuestFailure::UnsupportedFile));}
        let name=u.name.chars().map(|c|if c.is_control() || matches!(c,'\\'|'/'|'\u{061c}'|'\u{200e}'|'\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}'){'_'}else{c}).collect::<String>();
        let name=name.trim().chars().take(255).collect::<String>();
        Ok(AttachmentDraft {image,name:if name.is_empty(){if image{"image"}else{"file"}.to_owned()}else{name},mime_type:if mime.is_empty() || mime.len()>100 {"application/octet-stream".into()}else{mime},size:u.size})
    }).collect()
}
