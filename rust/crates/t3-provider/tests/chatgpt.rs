#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
};
use t3_provider::{auth::AuthBackend, chatgpt::*, sensitive::*, transport::*};
use tokio::sync::Mutex;

#[derive(Default)]
struct Store(Mutex<SavedProfiles>);
#[async_trait]
impl ProfileStore for Store {
    async fn load(&self) -> ServiceResult<SavedProfiles> {
        Ok(self.0.lock().await.clone())
    }
    async fn save(&self, saved: &SavedProfiles) -> ServiceResult<()> {
        *self.0.lock().await = saved.clone();
        Ok(())
    }
}
struct Http {
    replies: Mutex<VecDeque<HttpResponse>>,
    calls: Mutex<Vec<HttpRequest>>,
}
#[async_trait]
impl HttpTransport for Http {
    async fn send(&self, request: HttpRequest) -> ServiceResult<HttpResponse> {
        self.calls.lock().await.push(request);
        self.replies
            .lock()
            .await
            .pop_front()
            .ok_or(ServiceError::Network)
    }
}
struct Clock(AtomicI64);
impl ProviderClock for Clock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
struct EntropySource;
impl Entropy for EntropySource {
    fn bytes(&self, size: usize) -> ServiceResult<Vec<u8>> {
        Ok(vec![42; size])
    }
}
struct Verifier(Mutex<VerifiedIdentity>);
#[async_trait]
impl IdentityVerifier for Verifier {
    async fn verify(
        &self,
        _: &SecretString,
        _: &Discovery,
        _: &str,
        _: i64,
    ) -> ServiceResult<VerifiedIdentity> {
        Ok(self.0.lock().await.clone())
    }
}
struct Callbacks;
#[async_trait]
impl CallbackFactory for Callbacks {
    async fn listen(&self) -> ServiceResult<Box<dyn CallbackListener>> {
        Err(ServiceError::Unsupported)
    }
}
fn response(status: u16, body: Value) -> HttpResponse {
    HttpResponse {
        status,
        body: Sensitive::new(body),
    }
}
fn discovery() -> HttpResponse {
    response(
        200,
        json!({"issuer":"https://auth.openai.com","authorization_endpoint":"https://auth.openai.com/api/accounts/authorize","token_endpoint":"https://auth.openai.com/oauth/token","jwks_uri":"https://auth.openai.com/.well-known/jwks.json","revocation_endpoint":"https://auth.openai.com/oauth/revoke"}),
    )
}
fn tokens(scopes: &str) -> HttpResponse {
    response(
        200,
        json!({"access_token":"secret-access","refresh_token":"secret-refresh","id_token":"secret-id","token_type":"Bearer","expires_in":3600,"scope":scopes}),
    )
}
struct Harness {
    auth: Arc<ChatGptAuth>,
    store: Arc<Store>,
    http: Arc<Http>,
    clock: Arc<Clock>,
    verifier: Arc<Verifier>,
}
fn harness(replies: Vec<HttpResponse>) -> Harness {
    let store = Arc::new(Store::default());
    let http = Arc::new(Http {
        replies: Mutex::new(replies.into()),
        calls: Mutex::new(vec![]),
    });
    let clock = Arc::new(Clock(AtomicI64::new(100_000)));
    let verifier = Arc::new(Verifier(Mutex::new(VerifiedIdentity {
        subject: "subject".into(),
        email: Some("ada@example.com".into()),
        nonce: None,
    })));
    let auth = ChatGptAuth::new(
        store.clone(),
        http.clone(),
        verifier.clone(),
        Arc::new(EntropySource),
        clock.clone(),
        Arc::new(Callbacks),
        "env-id".into(),
    );
    Harness {
        auth,
        store,
        http,
        clock,
        verifier,
    }
}

struct HandoffCallbacks(Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<SecretString>>>);
struct HandoffListener(tokio::sync::mpsc::UnboundedReceiver<SecretString>);
#[async_trait]
impl CallbackFactory for HandoffCallbacks {
    async fn listen(&self) -> ServiceResult<Box<dyn CallbackListener>> {
        Ok(Box::new(HandoffListener(
            self.0
                .lock()
                .await
                .take()
                .ok_or(ServiceError::Unavailable)?,
        )))
    }
}
#[async_trait]
impl CallbackListener for HandoffListener {
    fn redirect_uri(&self) -> &str {
        "http://127.0.0.1:52000/auth/callback"
    }
    async fn receive(&mut self) -> ServiceResult<SecretString> {
        self.0.recv().await.ok_or(ServiceError::Cancelled)
    }
    async fn respond(&mut self, _: bool) {}
}

#[tokio::test]
async fn handoff_stream_exports_an_isolated_verified_profile_and_drop_cancels_pending_login() {
    use t3_contracts::provider_setup::{ChatGptHandoffInput, ChatGptHandoffState};
    use t3_provider::handoff::HandoffService;
    let h = harness(vec![discovery(), tokens(REQUIRED_SCOPE), discovery()]);
    let (send, received) = tokio::sync::mpsc::unbounded_channel();
    let service = HandoffService {
        http: h.http.clone(),
        verifier: h.verifier.clone(),
        entropy: Arc::new(EntropySource),
        clock: h.clock.clone(),
        callbacks: Arc::new(HandoffCallbacks(Mutex::new(Some(received)))),
    };
    let input: ChatGptHandoffInput = serde_json::from_value(json!({"instanceId":"work","environmentId":"environment","attemptId":"attempt","returnUrl":"https://app.t3.codes/settings","profile":null})).unwrap();
    let mut stream = service.subscribe(input, "owner").await.unwrap();
    let authorization = loop {
        let state = stream.next().await.unwrap().unwrap().into_inner();
        if let ChatGptHandoffState::Auth { state } = state
            && let Some(url) = state.authorization_url
        {
            break url;
        }
    };
    let url = url::Url::parse(&authorization).unwrap();
    let params: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    h.verifier.0.lock().await.nonce = Some(params["nonce"].clone());
    send.send(Sensitive::new(format!(
        "http://127.0.0.1:52000/auth/callback?state={}&code=secret-code&client_id=oaiapp_test",
        params["state"]
    )))
    .unwrap();
    let profile = loop {
        let state = stream.next().await.unwrap().unwrap();
        assert!(!format!("{state:?}").contains("secret-access"));
        if let ChatGptHandoffState::Finished { profile } = state.into_inner() {
            break profile;
        }
    };
    assert_eq!(profile.credentials.access_token, "secret-access");
    assert_eq!(profile.registration.client_id, "oaiapp_test");
    assert!(stream.next().await.unwrap().is_none());
    assert!(h.store.load().await.unwrap().sessions.sessions.is_empty());
    drop(stream);

    let (send, received) = tokio::sync::mpsc::unbounded_channel();
    let mut service = service;
    service.callbacks = Arc::new(HandoffCallbacks(Mutex::new(Some(received))));
    let input = serde_json::from_value(json!({"instanceId":"work","environmentId":"environment","attemptId":"cancel","returnUrl":"https://app.t3.codes/settings","profile":null})).unwrap();
    let mut pending = service.subscribe(input, "owner").await.unwrap();
    loop {
        if let ChatGptHandoffState::Auth { state } =
            pending.next().await.unwrap().unwrap().into_inner()
            && state.authorization_url.is_some()
        {
            break;
        }
    }
    drop(pending);
    send.closed().await;
    assert_eq!(h.http.calls.lock().await.len(), 3);
}
async fn login(h: &Harness, method: &str) -> (PendingLogin, String) {
    let pending = h
        .auth
        .begin(method, "http://127.0.0.1:52000/auth/callback".into())
        .await
        .unwrap();
    let url = url::Url::parse(pending.authorization_url.expose()).unwrap();
    let param = |name: &str| {
        url.query_pairs()
            .find(|(k, _)| k == name)
            .unwrap()
            .1
            .into_owned()
    };
    h.verifier.0.lock().await.nonce = Some(param("nonce"));
    let callback = format!(
        "http://127.0.0.1:52000/auth/callback?state={}&code=secret-code&client_id=oaiapp_test",
        param("state")
    );
    (pending, callback)
}
#[tokio::test]
async fn dynamic_pkce_login_verifies_identity_saves_original_registration_and_reuses_profile() {
    let h = harness(vec![discovery(), tokens(REQUIRED_SCOPE)]);
    let (pending, callback) = login(&h, "chatgpt").await;
    let url = url::Url::parse(pending.authorization_url.expose()).unwrap();
    assert_eq!(
        url.query_pairs().find(|(k, _)| k == "client_id").unwrap().1,
        "dynamic_agent_client"
    );
    assert_eq!(
        url.query_pairs()
            .find(|(k, _)| k == "code_challenge")
            .unwrap()
            .1
            .len(),
        43
    );
    assert_eq!(format!("{pending:?}"), "PendingLogin([REDACTED])");
    h.auth.finish(pending, &callback).await.unwrap();
    let record = h.auth.access(None).await.unwrap();
    assert_eq!(record.access_token.expose(), "secret-access");
    assert!(!format!("{record:?}").contains("secret-access"));
    let method = h.auth.methods().await.unwrap().pop().unwrap();
    assert_eq!(method.id.0, "chatgpt-profile:oaiapp_test");
    assert_eq!(method.can_remove, Some(false));
    let next = h
        .auth
        .begin("chatgpt", "http://127.0.0.1:52001/auth/callback".into())
        .await
        .unwrap();
    let next = url::Url::parse(next.authorization_url.expose()).unwrap();
    assert_eq!(
        next.query_pairs()
            .find(|(k, _)| k == "client_id")
            .unwrap()
            .1,
        "oaiapp_test"
    );
    assert_eq!(
        next.query_pairs()
            .find(|(k, _)| k == "id_token_hint")
            .unwrap()
            .1,
        "secret-id"
    );
    let calls = h.http.calls.lock().await;
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].method, "POST");
    let pairs: std::collections::BTreeMap<_, _> =
        url::form_urlencoded::parse(calls[1].body.as_ref().unwrap().expose().as_bytes())
            .into_owned()
            .collect();
    assert_eq!(pairs["code"], "secret-code");
    assert_eq!(pairs["resource"], RESOURCE);
    assert_eq!(
        pairs["redirect_uri"],
        "http://127.0.0.1:52000/auth/callback"
    );
    assert!(!format!("{:?}", calls[1]).contains("secret-code"));
}
#[tokio::test]
async fn callback_duplicates_wrong_origins_and_nonce_mismatch_never_publish_credentials() {
    let h = harness(vec![discovery(), tokens(REQUIRED_SCOPE)]);
    let (pending, callback) = login(&h, "chatgpt").await;
    let state = url::Url::parse(&callback)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    for bad in [
        format!("{callback}&state={state}"),
        format!("{callback}&code=duplicate"),
        callback.replace("127.0.0.1", "evil.example"),
        format!("{callback}#secret"),
        callback.replace("oaiapp_test", "invalid"),
    ] {
        assert_eq!(
            validate_callback(&bad, &pending.redirect_uri, &state).unwrap_err(),
            ServiceError::Callback
        );
    }
    assert!(
        validate_callback(
            callback.trim_start_matches("http://"),
            &pending.redirect_uri,
            &state
        )
        .is_ok()
    );
    h.verifier.0.lock().await.nonce = Some("wrong".into());
    assert_eq!(
        h.auth.finish(pending, &callback).await.unwrap_err(),
        ServiceError::Identity
    );
    assert!(h.store.load().await.unwrap().sessions.sessions.is_empty());
}
#[tokio::test]
async fn declined_sharing_keeps_identity_and_profile_but_denies_token_access() {
    let h = harness(vec![discovery(), tokens("openid email")]);
    let (pending, callback) = login(&h, "chatgpt").await;
    assert_eq!(
        h.auth.finish(pending, &callback).await.unwrap_err(),
        ServiceError::Sharing
    );
    assert!(h.auth.read().await.unwrap().is_some());
    assert_eq!(
        h.auth.access(None).await.unwrap_err(),
        ServiceError::Sharing
    );
    let retry = h
        .auth
        .begin("chatgpt", "http://127.0.0.1:52001/auth/callback".into())
        .await
        .unwrap();
    assert!(
        url::Url::parse(retry.authorization_url.expose())
            .unwrap()
            .query_pairs()
            .any(|(k, v)| k == "prompt" && v == "consent")
    );
}
#[tokio::test]
async fn expired_initial_code_retains_removable_registration_and_native_errors_are_safe() {
    let h = harness(vec![
        discovery(),
        response(
            400,
            json!({"error":"invalid_grant","detail":"secret-native-token"}),
        ),
    ]);
    let (pending, callback) = login(&h, "chatgpt").await;
    let error = h.auth.finish(pending, &callback).await.unwrap_err();
    assert_eq!(error, ServiceError::CodeExpired);
    assert!(!format!("{error:?} {error}").contains("secret-native-token"));
    assert_eq!(
        h.auth.methods().await.unwrap().last().unwrap().can_remove,
        Some(true)
    );
    h.auth
        .remove_method("chatgpt-profile:oaiapp_test")
        .await
        .unwrap();
    assert_eq!(h.auth.methods().await.unwrap().len(), 2);
}
#[tokio::test]
async fn renewal_rotates_only_near_expiry_preserves_identity_and_refuses_profile_replacement() {
    let h = harness(vec![
        discovery(),
        tokens(REQUIRED_SCOPE),
        tokens(REQUIRED_SCOPE),
    ]);
    let (pending, callback) = login(&h, "chatgpt").await;
    h.auth.finish(pending, &callback).await.unwrap();
    let original = h.auth.read().await.unwrap().unwrap();
    let key = ChatGptAuth::profile_key(&original).unwrap();
    assert_eq!(
        h.auth.access(Some("0".repeat(64))).await.unwrap_err(),
        ServiceError::ProfileChanged
    );
    h.clock.0.store(3_650_000, Ordering::SeqCst);
    let renewed = h.auth.access(Some(key.clone())).await.unwrap();
    assert_eq!(renewed.expires_at, 7_250_000.);
    assert_eq!(ChatGptAuth::profile_key(&renewed).unwrap(), key);
    assert_eq!(renewed.id_token.as_ref().unwrap().expose(), "secret-id");
    assert_eq!(h.http.calls.lock().await.len(), 3);
    assert_eq!(
        h.store.load().await.unwrap().sessions.sessions[0].expires_at,
        renewed.expires_at
    );
}
#[tokio::test]
async fn invalid_refresh_clears_only_the_active_connection_and_early_refresh_never_replays_a_token()
{
    let h = harness(vec![
        discovery(),
        tokens(REQUIRED_SCOPE),
        response(400, json!({"error":"refresh_token_reused"})),
    ]);
    let (pending, callback) = login(&h, "chatgpt").await;
    h.auth.finish(pending, &callback).await.unwrap();
    {
        let mut saved = h.store.0.lock().await;
        let mut other = saved.sessions.sessions[0].clone();
        other.client_id = "oaiapp_other".into();
        saved.sessions.sessions.push(other);
        saved.sessions.sessions[0].earliest_refresh_at = Some(4_000_000.);
    }
    h.clock.0.store(3_650_000, Ordering::SeqCst);
    assert!(h.auth.access(None).await.is_ok());
    h.clock.0.store(3_750_000, Ordering::SeqCst);
    assert_eq!(
        h.auth.access(None).await.unwrap_err(),
        ServiceError::RefreshEarly
    );
    h.clock.0.store(4_000_000, Ordering::SeqCst);
    assert_eq!(
        h.auth.access(None).await.unwrap_err(),
        ServiceError::Disconnected
    );
    assert_eq!(
        h.auth.access(None).await.unwrap_err(),
        ServiceError::Disconnected
    );
    let saved = h.store.load().await.unwrap();
    assert_eq!(saved.sessions.sessions.len(), 1);
    assert_eq!(saved.sessions.sessions[0].client_id, "oaiapp_other");
    assert!(saved.sessions.active_client_id.is_none());
    assert_eq!(h.http.calls.lock().await.len(), 3);
}
#[tokio::test]
async fn config_store_reads_legacy_single_records_and_writes_existing_secret_layout() {
    let directory = tempfile::tempdir().unwrap();
    let store = t3_config::secrets::SecretStore::new(directory.path());
    let credentials = json!({"clientId":"oaiapp_test","accessToken":"secret-access","refreshToken":"secret-refresh","expiresAt":1234,"earliestRefreshAt":null,"scopes":[REQUIRED_SCOPE],"subject":"subject","email":null});
    store
        .set(
            &t3_provider::secrets::credential_binding_key("codex-chatgpt", "work"),
            &serde_json::to_vec(&credentials).unwrap(),
        )
        .await
        .unwrap();
    store
        .set(
            &t3_provider::secrets::credential_binding_key("codex-chatgpt-registration", "work"),
            br#"{"clientId":"oaiapp_test"}"#,
        )
        .await
        .unwrap();
    let repository = ConfigProfileStore::new(directory.path(), "work");
    let saved = repository.load().await.unwrap();
    assert_eq!(
        saved.sessions.active_client_id.as_deref(),
        Some("oaiapp_test")
    );
    assert_eq!(
        saved.registrations.last_client_id.as_deref(),
        Some("oaiapp_test")
    );
    assert!(!format!("{saved:?}").contains("secret-access"));
    repository.save(&saved).await.unwrap();
    assert_eq!(repository.load().await.unwrap().sessions.sessions.len(), 1);
}
