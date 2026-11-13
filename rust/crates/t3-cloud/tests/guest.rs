#![allow(clippy::unwrap_used)]
mod support;
use async_trait::async_trait;
use futures_util::StreamExt;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use support::*;
use t3_cloud::{Clock, Error, Result, Secret, guest::*, http::Sensitive};
use t3_contracts::{
    base_schemas::{GuestShareId, ThreadId},
    guest_share::*,
    guest_stream::GuestStreamEvent,
};
use tokio::sync::Mutex;

struct Threads {
    rows: Mutex<BTreeMap<ThreadId, GuestThread>>,
    enabled: AtomicBool,
    host: Mutex<Option<String>>,
    clock: Arc<Time>,
}
impl Threads {
    fn new(clock: Arc<Time>) -> Self {
        Self {
            rows: Mutex::new(BTreeMap::from([(
                ThreadId("thread-one".into()),
                GuestThread {
                    thread_id: ThreadId("thread-one".into()),
                    share: None,
                    archived: false,
                    deleted: false,
                },
            )])),
            enabled: AtomicBool::new(true),
            host: Mutex::new(Some("env.test".into())),
            clock,
        }
    }
}
#[async_trait]
impl GuestThreads for Threads {
    async fn enabled(&self) -> Result<bool> {
        Ok(self.enabled.load(Ordering::Relaxed))
    }
    async fn connect_hostname(&self) -> Result<Option<String>> {
        Ok(self.host.lock().await.clone())
    }
    async fn find_share(&self, id: &GuestShareId) -> Result<Option<ThreadId>> {
        Ok(self
            .rows
            .lock()
            .await
            .values()
            .find(|t| t.share.as_ref().is_some_and(|s| &s.id == id))
            .map(|t| t.thread_id.clone()))
    }
    async fn read(&self, id: &ThreadId) -> Result<GuestThread> {
        self.rows
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or(Error::Guest(GuestFailure::Inactive))
    }
    async fn open(&self, id: &ThreadId, share: ThreadGuestShare) -> Result<()> {
        let mut rows = self.rows.lock().await;
        let thread = rows.get_mut(id).unwrap();
        if thread.archived || thread.deleted {
            return Err(Error::Guest(GuestFailure::Inactive));
        }
        thread.share = Some(share);
        Ok(())
    }
    async fn redeem(&self, id: &ThreadId, share: &GuestShareId, name: &str) -> Result<()> {
        let mut rows = self.rows.lock().await;
        let current = rows.get_mut(id).unwrap().share.as_mut().unwrap();
        if &current.id != share
            || chrono::DateTime::parse_from_rfc3339(&current.expires_at.0)
                .unwrap()
                .timestamp_millis()
                <= self.clock.now_ms()
        {
            return Err(Error::Guest(GuestFailure::Inactive));
        }
        if current.guest_name.is_some() {
            return Err(Error::Guest(GuestFailure::Used));
        }
        current.guest_name = Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
            name.into(),
        ));
        Ok(())
    }
    async fn submit(&self, id: &ThreadId, request: GuestSubmitRequest) -> Result<()> {
        let mut rows = self.rows.lock().await;
        let share = rows.get_mut(id).unwrap().share.as_mut().unwrap();
        if share.id != request.share_id {
            return Err(Error::Guest(GuestFailure::Inactive));
        }
        if share.messages.len() >= GUEST_PENDING_MESSAGE_LIMIT {
            return Err(Error::Guest(GuestFailure::Busy));
        }
        share.messages.push(ThreadGuestMessage {
            id: t3_contracts::base_schemas::GuestMessageId(uuid::Uuid::new_v4().to_string()),
            text: request.text.0,
            status: ThreadGuestMessageStatus::Pending,
            submitted_at: t3_contracts::wire::WireDateTime("2027-01-15T08:00:00.000Z".into()),
            attachments: None,
        });
        Ok(())
    }
    async fn stream(&self, _: &ThreadId, _: &GuestShareId) -> Result<GuestEvents> {
        Ok(Box::pin(futures_util::stream::pending()))
    }
}
async fn sharing() -> (Arc<GuestSharing>, Arc<Threads>, Arc<Time>) {
    let time = Arc::new(Time::default());
    let threads = Arc::new(Threads::new(time.clone()));
    let sharing = Arc::new(GuestSharing::new(
        threads.clone(),
        Arc::new(MemorySecrets::default()),
        time.clone(),
    ));
    (sharing, threads, time)
}
async fn create(sharing: &GuestSharing) -> Secret {
    let created = sharing
        .create(GuestShareCreateInput {
            thread_id: ThreadId("thread-one".into()),
        })
        .await
        .unwrap();
    assert!(!format!("{created:?}").contains("https://env.test/guest#"));
    Secret::new(created.expose().url.0.split_once('#').unwrap().1)
}
fn redeem_request(invite: &Secret) -> Sensitive<GuestRedeemRequest> {
    Sensitive::new(GuestRedeemRequest {
        credential: t3_contracts::base_schemas::TrimmedNonEmptyString(invite.expose().into()),
        name: t3_contracts::base_schemas::TrimmedNonEmptyString("Guest".into()),
    })
}

#[tokio::test]
async fn guest_invite_redeems_once_session_cannot_be_used_as_invite_and_replacement_revokes_it() {
    let (sharing, _, _) = sharing().await;
    let invite = create(&sharing).await;
    assert!(sharing.check(&invite).await.is_err());
    let (a, b) = tokio::join!(
        sharing.redeem(redeem_request(&invite)),
        sharing.redeem(redeem_request(&invite))
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let (session, share) = a.or(b).unwrap();
    sharing.check(&session).await.unwrap();
    assert!(matches!(
        sharing.redeem(redeem_request(&invite)).await,
        Err(Error::Guest(GuestFailure::Used))
    ));
    assert!(sharing.redeem(redeem_request(&session)).await.is_err());
    let stale = GuestSubmitRequest {
        share_id: GuestShareId("other-share".into()),
        text: t3_contracts::base_schemas::TrimmedString("hello".into()),
    };
    assert!(matches!(
        sharing.submit(&session, stale).await,
        Err(Error::Guest(GuestFailure::StalePage))
    ));
    for n in 0..GUEST_PENDING_MESSAGE_LIMIT {
        sharing
            .submit(
                &session,
                GuestSubmitRequest {
                    share_id: share.clone(),
                    text: t3_contracts::base_schemas::TrimmedString(format!("hello {n}")),
                },
            )
            .await
            .unwrap();
    }
    assert!(matches!(
        sharing
            .submit(
                &session,
                GuestSubmitRequest {
                    share_id: share,
                    text: t3_contracts::base_schemas::TrimmedString("hello".into())
                }
            )
            .await,
        Err(Error::Guest(GuestFailure::Busy))
    ));
    create(&sharing).await;
    assert!(matches!(
        sharing.check(&session).await,
        Err(Error::Guest(GuestFailure::Inactive))
    ));
}

#[tokio::test]
async fn guest_access_rechecks_labs_flag_connect_hostname_archive_deletion_and_expiry() {
    for reason in ["disabled", "host", "archive", "delete", "expired", "closed"] {
        let (sharing, threads, time) = sharing().await;
        let invite = create(&sharing).await;
        let (session, _) = sharing.redeem(redeem_request(&invite)).await.unwrap();
        match reason {
            "disabled" => threads.enabled.store(false, Ordering::Relaxed),
            "host" => *threads.host.lock().await = Some("other.test".into()),
            "archive" => {
                threads
                    .rows
                    .lock()
                    .await
                    .get_mut(&ThreadId("thread-one".into()))
                    .unwrap()
                    .archived = true
            }
            "delete" => {
                threads
                    .rows
                    .lock()
                    .await
                    .get_mut(&ThreadId("thread-one".into()))
                    .unwrap()
                    .deleted = true
            }
            "expired" => time.advance(GUEST_SHARE_TTL_MS as i64),
            _ => {
                threads
                    .rows
                    .lock()
                    .await
                    .get_mut(&ThreadId("thread-one".into()))
                    .unwrap()
                    .share = None
            }
        }
        assert!(
            matches!(
                sharing.check(&session).await,
                Err(Error::Guest(GuestFailure::Inactive))
            ),
            "{reason}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn open_guest_stream_rechecks_authority_and_emits_ended_without_a_backend_event() {
    let (sharing, threads, time) = sharing().await;
    let invite = create(&sharing).await;
    let (session, _) = sharing.redeem(redeem_request(&invite)).await.unwrap();
    let mut stream = sharing.stream(&session).await.unwrap();
    threads.enabled.store(false, Ordering::Relaxed);
    time.advance(30_000);
    tokio::time::advance(Duration::from_secs(30)).await;
    assert!(matches!(stream.next().await, Some(GuestStreamEvent::Ended)));
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn guest_tokens_are_exact_two_part_mac_values_and_tampering_never_leaks_secrets() {
    let secrets = Arc::new(MemorySecrets::default());
    use t3_cloud::store::Secrets;
    secrets.set("guest-share-signing-key", b"").await.unwrap();
    let time = Arc::new(Time::default());
    let invalid = GuestSharing::new(Arc::new(Threads::new(time.clone())), secrets.clone(), time);
    assert!(invalid.invite(&GuestShareId("share".into())).await.is_err());
    secrets
        .set("guest-share-signing-key", b"legacy-short-key")
        .await
        .unwrap();
    assert!(invalid.invite(&GuestShareId("share".into())).await.is_ok());
    let (sharing, _, _) = sharing().await;
    let invite = create(&sharing).await;
    for token in [
        "marker-secret".into(),
        format!("{}.extra", invite.expose()),
        format!(
            "{}.marker-secret",
            invite.expose().split('.').next().unwrap()
        ),
        format!(
            "marker-secret.{}",
            invite.expose().split('.').nth(1).unwrap()
        ),
    ] {
        let error = sharing
            .redeem(redeem_request(&Secret::new(token)))
            .await
            .unwrap_err();
        assert!(!format!("{error:?} {error}").contains("marker-secret"));
    }
}

#[test]
fn guest_uploads_validate_signatures_size_count_and_sanitize_names() {
    let valid = Upload {
        name: "invoice\u{202e}/foo.png",
        content_type: " IMAGE/PNG; charset=utf-8 ",
        size: 20,
        header: b"\x89PNG\r\n\x1a\n",
    };
    let drafts = classify_uploads(&[valid]).unwrap();
    assert_eq!(drafts[0].name, "invoice__foo.png");
    assert!(drafts[0].image);
    for (mime, header) in [
        ("image/svg+xml", b"<svg".as_slice()),
        ("image/png", b"<html>".as_slice()),
    ] {
        assert!(matches!(
            classify_uploads(&[Upload {
                name: "file",
                content_type: mime,
                size: 10,
                header
            }]),
            Err(Error::Guest(GuestFailure::UnsupportedFile))
        ));
    }
    assert!(matches!(
        classify_uploads(&[Upload {
            name: "file",
            content_type: "text/plain",
            size: 10 * 1024 * 1024 + 1,
            header: b""
        }]),
        Err(Error::Guest(GuestFailure::TooLarge))
    ));
    let uploads = (0..5)
        .map(|_| Upload {
            name: "file",
            content_type: "text/plain",
            size: 1,
            header: b"",
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        classify_uploads(&uploads),
        Err(Error::Guest(GuestFailure::TooManyFiles))
    ));
    assert!(matches!(
        classify_uploads(&[Upload {
            name: "file",
            content_type: "text/plain",
            size: 0,
            header: b""
        }]),
        Err(Error::Guest(GuestFailure::UnsupportedFile))
    ));
}
