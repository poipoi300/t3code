#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use t3_contracts::{provider_instance::ProviderInstanceId, provider_setup::*};
use t3_provider::{auth::*, sensitive::*};
use tokio::sync::{Mutex, mpsc};

struct Backend {
    events: mpsc::UnboundedSender<&'static str>,
    stopped: Mutex<bool>,
}
#[async_trait]
impl AuthBackend for Backend {
    async fn methods(&self) -> ServiceResult<Vec<ProviderAuthMethod>> {
        Ok(serde_json::from_value(
            json!([{"id":"key","name":"API key","type":"credentials","description":null}]),
        )
        .unwrap())
    }
    async fn authenticate(&self, _: &str, mut context: AuthContext) -> ServiceResult<()> {
        self.events.send("started").unwrap();
        context.interaction(serde_json::from_value(json!({"type":"credentials","id":"key-input","fields":[{"name":"key","label":"API key","secret":true}]})).unwrap(), true, false).await?;
        let reply = context.receive().await?;
        assert!(!format!("{:?}", reply.reply).contains("secret-test-key"));
        context.verifying().await;
        reply.finish(Ok(()));
        self.events.send("verified").unwrap();
        Ok(())
    }
    async fn logout(&self) -> ServiceResult<Option<String>> {
        assert!(*self.stopped.lock().await);
        self.events.send("logout").unwrap();
        Ok(None)
    }
}
struct Stopper(Arc<Backend>);
#[async_trait]
impl SessionStopper for Stopper {
    async fn stop(&self, _: &ProviderInstanceId, _: &CredentialBinding) -> ServiceResult<()> {
        *self.0.stopped.lock().await = true;
        self.0.events.send("stopped").unwrap();
        Ok(())
    }
}
async fn setup(timeout: Duration) -> (Arc<AuthController>, mpsc::UnboundedReceiver<&'static str>) {
    let (events, rx) = mpsc::unbounded_channel();
    let backend = Arc::new(Backend {
        events,
        stopped: Mutex::new(false),
    });
    (
        AuthController::new(
            ProviderInstanceId("work".into()),
            CredentialBinding {
                key: "binding".into(),
                owner: ProviderCredentialOwner::T3,
            },
            backend.clone(),
            Arc::new(Stopper(backend)),
            timeout,
        )
        .await,
        rx,
    )
}
async fn phase(subscription: &mut AuthSubscription, phase: ProviderAuthPhase) -> AuthState {
    loop {
        let state = subscription.next().await.unwrap();
        if state.expose().phase == phase {
            return state;
        }
    }
}
#[tokio::test]
async fn auth_flow_stops_sessions_hides_other_clients_and_validates_flow_and_interaction() {
    let (controller, mut events) = setup(Duration::from_secs(300)).await;
    let mut owner = controller.subscribe("owner".into());
    let start = controller
        .start("owner", None, None, None, 0)
        .await
        .unwrap();
    let id = start.expose().flow_id.as_ref().unwrap().0.clone();
    assert_eq!(
        controller
            .start("owner", None, None, None, 0)
            .await
            .unwrap(),
        start
    );
    assert_eq!(
        controller
            .start("other", None, None, None, 0)
            .await
            .unwrap_err(),
        ServiceError::Busy
    );
    assert_eq!(events.recv().await.unwrap(), "stopped");
    assert_eq!(events.recv().await.unwrap(), "started");
    phase(&mut owner, ProviderAuthPhase::Waiting).await;
    let redacted = controller.snapshot("other").await;
    assert!(redacted.expose().flow_id.is_none());
    assert_eq!(redacted.expose().interaction, Some(None));
    assert!(redacted.expose().authorization_url.is_none());
    let respond = |flow: &str, interaction: &str| {
        serde_json::from_value(json!({"instanceId":"work","flowId":flow,"interactionId":interaction,"response":{"type":"credentials","values":{"key":"secret-test-key"}}})).unwrap()
    };
    assert_eq!(
        controller
            .respond("other", respond(&id, "key-input"))
            .await
            .unwrap_err(),
        ServiceError::Inactive
    );
    assert_eq!(
        controller
            .respond("owner", respond(&id, "stale"))
            .await
            .unwrap_err(),
        ServiceError::Interaction
    );
    controller
        .respond("owner", respond(&id, "key-input"))
        .await
        .unwrap();
    let success = phase(&mut owner, ProviderAuthPhase::Succeeded).await;
    assert!(!format!("{success:?}").contains("secret-test-key"));
    assert!(success.expose().expires_at.is_none());
    assert_eq!(events.recv().await.unwrap(), "verified");
    assert_eq!(
        controller.cancel("owner", &id).await.unwrap_err(),
        ServiceError::Inactive
    );
    controller.logout().await.unwrap();
    assert_eq!(events.recv().await.unwrap(), "stopped");
    assert_eq!(events.recv().await.unwrap(), "logout");
}
#[tokio::test(start_paused = true)]
async fn timeout_and_cancel_publish_terminal_states_and_bad_methods_never_stop_sessions() {
    let (controller, mut events) = setup(Duration::from_secs(30)).await;
    let mut subscription = controller.subscribe("owner".into());
    controller
        .start("owner", Some("unknown"), None, None, 0)
        .await
        .unwrap();
    phase(&mut subscription, ProviderAuthPhase::Failed).await;
    assert!(events.try_recv().is_err());
    let start = controller
        .start("owner", None, None, None, 0)
        .await
        .unwrap();
    phase(&mut subscription, ProviderAuthPhase::Waiting).await;
    controller
        .cancel("owner", &start.expose().flow_id.as_ref().unwrap().0)
        .await
        .unwrap();
    assert_eq!(
        controller.snapshot("owner").await.expose().phase,
        ProviderAuthPhase::Cancelled
    );
    controller
        .start("owner", None, None, None, 0)
        .await
        .unwrap();
    phase(&mut subscription, ProviderAuthPhase::Waiting).await;
    tokio::time::advance(Duration::from_secs(30)).await;
    let failed = phase(&mut subscription, ProviderAuthPhase::Failed).await;
    assert_eq!(
        failed.expose().message.as_deref(),
        Some("Sign-in expired. Start again.")
    );
}
#[test]
fn all_secret_carriers_and_errors_format_without_payloads_but_serialize_for_transport() {
    let secret = SecretString::new("secret-test-key".into());
    assert_eq!(
        serde_json::to_value(&secret).unwrap(),
        json!("secret-test-key")
    );
    assert_eq!(format!("{secret:?} {secret}"), "[REDACTED] [REDACTED]");
    let reply = AuthReply::Callback(secret);
    assert!(!format!("{reply:?}").contains("secret-test-key"));
    for error in [
        ServiceError::Storage,
        ServiceError::Identity,
        ServiceError::Network,
        ServiceError::Response,
    ] {
        assert!(!format!("{error:?} {error}").contains("secret-test-key"));
    }
    let io = t3_provider::Error::Io(std::io::Error::other("secret-test-key"));
    let json = t3_provider::Error::Json(
        serde_json::from_value::<ProviderAuthPhase>(json!("secret-test-key")).unwrap_err(),
    );
    for error in [io, json] {
        assert!(!format!("{error:?} {error}").contains("secret-test-key"));
    }
}

struct PendingBackend(mpsc::UnboundedSender<AuthDelivery>);
#[async_trait]
impl AuthBackend for PendingBackend {
    async fn methods(&self) -> ServiceResult<Vec<ProviderAuthMethod>> {
        Ok(serde_json::from_value(
            json!([{"id":"key","name":"Key","type":"credentials","description":null}]),
        )
        .unwrap())
    }
    async fn authenticate(&self, _: &str, mut context: AuthContext) -> ServiceResult<()> {
        context.interaction(serde_json::from_value(json!({"type":"credentials","id":"input","fields":[{"name":"key","label":"Key","secret":true}]})).unwrap(),true,false).await?;
        loop {
            let delivery = context.receive().await?;
            self.0.send(delivery).map_err(|_| ServiceError::Cancelled)?;
        }
    }
    async fn logout(&self) -> ServiceResult<Option<String>> {
        Ok(None)
    }
}
struct NoSessions;
#[async_trait]
impl SessionStopper for NoSessions {
    async fn stop(&self, _: &ProviderInstanceId, _: &CredentialBinding) -> ServiceResult<()> {
        Ok(())
    }
}
#[tokio::test]
async fn an_admitted_response_finishes_after_the_requesting_client_disconnects() {
    let (deliveries, mut received) = mpsc::unbounded_channel();
    let controller = AuthController::new(
        "work".into(),
        CredentialBinding {
            key: "binding".into(),
            owner: ProviderCredentialOwner::T3,
        },
        Arc::new(PendingBackend(deliveries)),
        Arc::new(NoSessions),
        Duration::from_secs(300),
    )
    .await;
    let mut subscription = controller.subscribe("owner".into());
    let state = controller
        .start("owner", None, None, None, 0)
        .await
        .unwrap();
    let flow = state.expose().flow_id.as_ref().unwrap().0.clone();
    phase(&mut subscription, ProviderAuthPhase::Waiting).await;
    let input:ProviderAuthRespondInput=serde_json::from_value(json!({"instanceId":"work","flowId":flow,"interactionId":"input","response":{"type":"credentials","values":{"key":"secret-test-key"}}})).unwrap();
    let first = tokio::spawn({
        let controller = controller.clone();
        let input = input.clone();
        async move { controller.respond("owner", input).await }
    });
    let delivery = received.recv().await.unwrap();
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    delivery.finish(Err(ServiceError::Interaction));
    subscription.next().await.unwrap();
    let second = tokio::spawn({
        let controller = controller.clone();
        async move { controller.respond("owner", input).await }
    });
    received.recv().await.unwrap().finish(Ok(()));
    second.await.unwrap().unwrap();
    controller.cancel("owner", &flow).await.unwrap();
}

#[tokio::test]
async fn invalid_profile_import_leaves_existing_sessions_running() {
    let (controller, mut events) = setup(Duration::from_secs(300)).await;
    let access = controller.admit_access().await.unwrap();
    let profile=serde_json::from_value(json!({"registration":{"clientId":"client"},"credentials":{"clientId":"client","issuer":"https://auth.openai.com","accessToken":"secret-test-key","refreshToken":null,"idToken":"secret-id","expiresAt":1234,"earliestRefreshAt":null,"scopes":[],"subject":"subject","email":null}})).unwrap();
    assert_eq!(
        controller
            .adopt_credentials(Sensitive::new(profile))
            .await
            .unwrap_err(),
        ServiceError::Unsupported
    );
    assert!(events.try_recv().is_err());
    assert!(!access.cancellation.is_cancelled());
}

#[tokio::test]
async fn instances_sharing_credentials_revoke_existing_access_and_block_new_admission() {
    let (first, _events) = setup(Duration::from_secs(300)).await;
    let (deliveries, _received) = mpsc::unbounded_channel();
    let second = AuthController::new(
        "other".into(),
        first.binding.clone(),
        Arc::new(PendingBackend(deliveries)),
        Arc::new(NoSessions),
        Duration::from_secs(300),
    )
    .await;
    let service = AuthService::default();
    service.register("work".into(), first.clone()).await;
    service.register("other".into(), second.clone()).await;
    let existing = second.admit_access().await.unwrap();
    let mut subscription = first.subscribe("owner".into());
    let started = service
        .start(
            serde_json::from_value(json!({"instanceId":"work"})).unwrap(),
            "owner",
            0,
        )
        .await
        .unwrap();
    phase(&mut subscription, ProviderAuthPhase::Waiting).await;
    assert!(existing.cancellation.is_cancelled());
    assert!(matches!(
        second.admit_access().await,
        Err(ServiceError::SharedBinding)
    ));
    first
        .cancel("owner", &started.expose().flow_id.as_ref().unwrap().0)
        .await
        .unwrap();
    second.admit_access().await.unwrap();
}

#[tokio::test]
async fn replacement_preserves_admitted_access_and_subscription_until_binding_changes() {
    let (original, _events) = setup(Duration::from_secs(300)).await;
    let service = AuthService::default();
    service.register("work".into(), original.clone()).await;
    let access = original.admit_access().await.unwrap();
    let mut subscription = service
        .subscribe(
            serde_json::from_value(json!({"instanceId":"work"})).unwrap(),
            "owner".into(),
        )
        .await
        .unwrap();
    assert_eq!(
        subscription.next().await.unwrap().expose().phase,
        ProviderAuthPhase::Idle
    );
    let (replacement, _events) = setup(Duration::from_secs(300)).await;
    service.register("work".into(), replacement.clone()).await;
    assert!(!access.cancellation.is_cancelled());
    assert_eq!(
        subscription.next().await.unwrap().expose().phase,
        ProviderAuthPhase::Idle
    );
    assert!(matches!(
        original.admit_access().await,
        Err(ServiceError::Unavailable)
    ));
    replacement
        .start("owner", Some("unknown"), None, None, 0)
        .await
        .unwrap();
    phase(&mut subscription, ProviderAuthPhase::Failed).await;
    assert!(!access.cancellation.is_cancelled());
    let started = replacement
        .start("owner", None, None, None, 0)
        .await
        .unwrap();
    phase(&mut subscription, ProviderAuthPhase::Waiting).await;
    assert!(access.cancellation.is_cancelled());
    replacement
        .cancel("owner", &started.expose().flow_id.as_ref().unwrap().0)
        .await
        .unwrap();
    service.unregister(&"work".into()).await;
    assert!(subscription.next().await.is_none());
}

struct HeldStopper {
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}
#[async_trait]
impl SessionStopper for HeldStopper {
    async fn stop(&self, _: &ProviderInstanceId, _: &CredentialBinding) -> ServiceResult<()> {
        self.started.lock().await.take().unwrap().send(()).unwrap();
        self.release.lock().await.take().unwrap().await.unwrap();
        Ok(())
    }
}
#[tokio::test]
async fn registering_an_auth_flow_waiting_for_session_stop_joins_the_current_binding_domain() {
    let (first, _events) = setup(Duration::from_secs(300)).await;
    let service = AuthService::default();
    service.register("work".into(), first.clone()).await;
    let access = first.admit_access().await.unwrap();
    let (started, stopped) = tokio::sync::oneshot::channel();
    let (release, ready) = tokio::sync::oneshot::channel();
    let (deliveries, _received) = mpsc::unbounded_channel();
    let second = AuthController::new(
        "other".into(),
        first.binding.clone(),
        Arc::new(PendingBackend(deliveries)),
        Arc::new(HeldStopper {
            started: Mutex::new(Some(started)),
            release: Mutex::new(Some(ready)),
        }),
        Duration::from_secs(300),
    )
    .await;
    let mut states = second.subscribe("owner".into());
    let flow = second.start("owner", None, None, None, 0).await.unwrap();
    stopped.await.unwrap();
    service.register("other".into(), second.clone()).await;
    assert!(matches!(
        first.admit_access().await,
        Err(ServiceError::SharedBinding)
    ));
    assert!(!access.cancellation.is_cancelled());
    release.send(()).unwrap();
    phase(&mut states, ProviderAuthPhase::Waiting).await;
    assert!(access.cancellation.is_cancelled());
    second
        .cancel("owner", &flow.expose().flow_id.as_ref().unwrap().0)
        .await
        .unwrap();
}

struct BrowserBackend(String);
#[async_trait]
impl AuthBackend for BrowserBackend {
    async fn logout(&self) -> ServiceResult<Option<String>> {
        Ok(None)
    }
    async fn methods(&self) -> ServiceResult<Vec<ProviderAuthMethod>> {
        Ok(serde_json::from_value(
            json!([{"id":"browser","name":"Browser","type":"agent","description":null}]),
        )
        .unwrap())
    }
    async fn authenticate(&self, _: &str, mut context: AuthContext) -> ServiceResult<()> {
        context
            .interaction(
                ProviderAuthInteraction::Browser {
                    id: "browser".into(),
                    url: self.0.clone().into(),
                    requires_consent: false,
                    accepts_callback: None,
                },
                false,
                true,
            )
            .await?;
        context.receive().await?;
        Ok(())
    }
}
struct BrowserStopper;
#[async_trait]
impl SessionStopper for BrowserStopper {
    async fn stop(&self, _: &ProviderInstanceId, _: &CredentialBinding) -> ServiceResult<()> {
        Ok(())
    }
}
#[tokio::test]
async fn browser_interactions_validate_and_trim_provider_urls_before_publication() {
    for (url, valid) in [
        (
            "  https://login.example.test/authorize?state=private-state  ".into(),
            true,
        ),
        ("javascript:alert('private-state')".into(), false),
        ("file:///private-state".into(), false),
        (
            format!("https://login.example.test/{}", "x".repeat(16_384)),
            false,
        ),
    ] {
        let controller = AuthController::new(
            "browser".into(),
            CredentialBinding {
                key: "browser".into(),
                owner: ProviderCredentialOwner::T3,
            },
            Arc::new(BrowserBackend(url)),
            Arc::new(BrowserStopper),
            Duration::from_secs(300),
        )
        .await;
        let mut events = controller.subscribe("owner".into());
        controller
            .start("owner", None, None, None, 0)
            .await
            .unwrap();
        let state = loop {
            let state = events.next().await.unwrap();
            if matches!(
                state.expose().phase,
                ProviderAuthPhase::Waiting | ProviderAuthPhase::Failed
            ) {
                break state;
            }
        };
        if valid {
            assert_eq!(
                state.expose().authorization_url.as_deref(),
                Some("https://login.example.test/authorize?state=private-state")
            );
        } else {
            assert_eq!(state.expose().phase, ProviderAuthPhase::Failed);
            assert!(state.expose().authorization_url.is_none());
            assert_eq!(
                state.expose().message.as_deref(),
                Some(ServiceError::Response.to_string().as_str())
            );
        }
        assert!(!format!("{state:?}").contains("private-state"));
        controller.close();
    }
}

struct PendingCommit {
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}
#[async_trait]
impl AuthBackend for PendingCommit {
    async fn methods(&self) -> ServiceResult<Vec<ProviderAuthMethod>> {
        Ok(serde_json::from_value(
            json!([{"id":"key","name":"Key","type":"credentials","description":null}]),
        )
        .unwrap())
    }
    async fn authenticate(&self, _: &str, context: AuthContext) -> ServiceResult<()> {
        let entered = self.entered.lock().await.take().unwrap();
        let release = self.release.lock().await.take().unwrap();
        context
            .commit(async move {
                entered.send(()).unwrap();
                release.await.map_err(|_| ServiceError::Cancelled)
            })
            .await
    }
    async fn logout(&self) -> ServiceResult<Option<String>> {
        Ok(None)
    }
}
#[tokio::test]
async fn another_subscriber_observes_verifying_while_credential_commit_is_pending() {
    let (entered, pending) = tokio::sync::oneshot::channel();
    let (release, blocked) = tokio::sync::oneshot::channel();
    let controller = AuthController::new(
        "work".into(),
        CredentialBinding {
            key: "binding".into(),
            owner: ProviderCredentialOwner::T3,
        },
        Arc::new(PendingCommit {
            entered: Mutex::new(Some(entered)),
            release: Mutex::new(Some(blocked)),
        }),
        Arc::new(NoSessions),
        Duration::from_secs(300),
    )
    .await;
    let mut other = controller.subscribe("other".into());
    other.next().await.unwrap();
    controller
        .start("owner", None, None, None, 0)
        .await
        .unwrap();
    pending.await.unwrap();
    let next = other.next();
    tokio::pin!(next);
    let std::task::Poll::Ready(Some(state)) = futures::poll!(&mut next) else {
        panic!("commit blocked the other subscriber");
    };
    assert_eq!(state.expose().phase, ProviderAuthPhase::Verifying);
    assert!(state.expose().flow_id.is_none());
    release.send(()).unwrap();
    let mut completed = controller.subscribe("owner".into());
    phase(&mut completed, ProviderAuthPhase::Succeeded).await;
}
