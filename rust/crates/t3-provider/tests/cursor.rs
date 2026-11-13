#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{collections::{BTreeMap, VecDeque}, sync::{Arc, atomic::{AtomicI64, Ordering}}};
use t3_provider::{cursor_catalog::*, sensitive::*, transport::*};
use tokio::sync::Mutex;
struct Clock(AtomicI64);
impl ProviderClock for Clock { fn now_ms(&self) -> i64 { self.0.load(Ordering::SeqCst) } }
struct Http {
    replies: Mutex<BTreeMap<String, VecDeque<HttpResponse>>>,
    calls: Mutex<Vec<HttpRequest>>,
}
#[async_trait]
impl HttpTransport for Http {
    async fn send(&self, request: HttpRequest) -> ServiceResult<HttpResponse> {
        assert!(!format!("{request:?}").contains("secret-key"));
        self.calls.lock().await.push(request.clone());
        self.replies.lock().await.get_mut(&request.url).and_then(VecDeque::pop_front).ok_or(ServiceError::Network)
    }
}
fn response(status: u16, body: Value) -> HttpResponse { HttpResponse { status, body: Sensitive::new(body) } }
fn user() -> HttpResponse { response(200, json!({"apiKeyName":"T3", "createdAt":"2026-01-01", "userEmail":"ada@example.com"})) }
fn models() -> HttpResponse { response(200, json!({"items":[{"id":"model","displayName":"Model","parameters":[],"variants":[]}]})) }
fn http(users: Vec<HttpResponse>, models: Vec<HttpResponse>) -> Arc<Http> {
    Arc::new(Http { replies: Mutex::new(BTreeMap::from([("https://cursor.invalid/v1/me".into(), users.into()), ("https://cursor.invalid/v1/models".into(), models.into())])), calls: Mutex::new(vec![]) })
}
#[tokio::test]
async fn account_labels_are_always_fresh_while_nonempty_models_are_cached_per_key_for_thirty_minutes() {
    let http = http(vec![user(),user(),user(),user()], vec![models(),models(),models()]);
    let clock = Arc::new(Clock(AtomicI64::new(0)));
    let catalog = CursorCatalog::new(http.clone(), clock.clone(), "https://cursor.invalid/".into());
    let key = Sensitive::new("secret-key".into());
    for _ in 0..2 { assert_eq!(catalog.read(&key).await.unwrap().models.len(), 1); }
    clock.0.store(30 * 60 * 1000, Ordering::SeqCst);
    catalog.read(&key).await.unwrap();
    catalog.read(&Sensitive::new("second-secret-key".into())).await.unwrap();
    let calls = http.calls.lock().await;
    assert_eq!(calls.iter().filter(|request| request.url.ends_with("/me")).count(), 4);
    assert_eq!(calls.iter().filter(|request| request.url.ends_with("/models")).count(), 3);
    assert_eq!(calls[0].headers["authorization"].expose(), "Bearer secret-key");
}
#[tokio::test]
async fn blocked_account_labels_allow_models_but_plan_and_authentication_failures_remain_distinct() {
    for (status, body, allowed, authentication, plan) in [
        (403, json!({"error":{"code":"enterprise_blocked","message":"secret-key"}}), true, false, false),
        (403, json!({"error":{"code":"plan_required","message":"secret-key"}}), false, false, true),
        (401, json!({"code":"rejected","message":"secret-key"}), false, true, false),
    ] {
        let catalog = CursorCatalog::new(http(vec![response(status, body)], vec![models()]), Arc::new(Clock(AtomicI64::new(0))), "https://cursor.invalid".into());
        match catalog.read(&Sensitive::new("secret-key".into())).await {
            Ok(snapshot) => { assert!(allowed); assert!(snapshot.user.is_none()); assert_eq!(snapshot.models.len(), 1); }
            Err(error) => { assert!(!allowed); assert_eq!(error.authentication_failure, authentication); assert_eq!(error.plan_required, plan); assert!(!format!("{error:?} {error}").contains("secret-key")); }
        }
    }
}
#[tokio::test]
async fn empty_or_malformed_model_responses_are_not_cached_and_explicit_optional_nulls_are_rejected() {
    let invalid = response(200, json!({"items":[{"id":"model","displayName":"Model","parameters":null}]}));
    let http = http(vec![user(),user(),user(),user()], vec![response(200, json!({"items":[]})), invalid, models()]);
    let catalog = CursorCatalog::new(http.clone(), Arc::new(Clock(AtomicI64::new(0))), "https://cursor.invalid".into());
    let key = Sensitive::new("secret-key".into());
    assert!(catalog.read(&key).await.unwrap().models.is_empty());
    assert!(catalog.read(&key).await.is_err());
    assert_eq!(catalog.read(&key).await.unwrap().models.len(), 1);
    catalog.read(&key).await.unwrap();
    assert_eq!(http.calls.lock().await.iter().filter(|request| request.url.ends_with("/models")).count(), 3);
}

use t3_provider::{auth::*, cursor_auth::*};
use t3_contracts::{provider_setup::*, provider_instance::ProviderInstanceId};
#[derive(Default)]
struct Store {
    value: Mutex<Option<CursorCredentials>>,
    save_gate: Mutex<Option<(tokio::sync::oneshot::Sender<()>, tokio::sync::oneshot::Receiver<()>)>>,
}
#[async_trait]
impl CursorCredentialStore for Store {
    async fn load(&self) -> ServiceResult<Option<CursorCredentials>> { Ok(self.value.lock().await.clone()) }
    async fn save(&self, credentials: &CursorCredentials) -> ServiceResult<()> {
        if let Some((started, wait)) = self.save_gate.lock().await.take() { started.send(()).unwrap(); wait.await.unwrap(); }
        *self.value.lock().await = Some(credentials.clone()); Ok(())
    }
    async fn clear(&self) -> ServiceResult<()> { *self.value.lock().await = None; Ok(()) }
}
struct Login(Mutex<Option<tokio::sync::oneshot::Receiver<CursorCredentials>>>);
#[async_trait]
impl CursorLogin for Login {
    async fn login(&self, name: String, urls: tokio::sync::mpsc::UnboundedSender<SecretString>, _: t3_process::CancellationToken) -> ServiceResult<CursorCredentials> {
        assert_eq!(name, "T3 Code - Work");
        urls.send(Sensitive::new("https://cursor.invalid/login?nonce=secret-key".into())).unwrap();
        self.0.lock().await.take().unwrap().await.map_err(|_| ServiceError::Cancelled)
    }
}
struct Stopper;
#[async_trait]
impl SessionStopper for Stopper { async fn stop(&self, _: &ProviderInstanceId, _: &CredentialBinding) -> ServiceResult<()> { Ok(()) } }
fn credentials() -> CursorCredentials {
    serde_json::from_value(json!({"version":1,"backendUrl":"https://api.cursor.com","apiKey":"secret-key","createdAtMs":0,"apiKeyExpiresAtMs":10000,"email":"ada@example.com"})).unwrap()
}
async fn authentication(configured: bool) -> (Arc<CursorAuth>, Arc<AuthController>, Arc<Store>, tokio::sync::oneshot::Sender<CursorCredentials>) {
    let (send, receive) = tokio::sync::oneshot::channel();
    let store = Arc::new(Store::default());
    let clock = Arc::new(Clock(AtomicI64::new(0)));
    let auth = Arc::new(CursorAuth { enabled: true, display_name: "Work".into(), configured_key: configured.then(|| Sensitive::new(" configured-key ".into())), store: store.clone(), login: Arc::new(Login(Mutex::new(Some(receive)))), catalog: Arc::new(CursorCatalog::new(http(vec![user()], vec![models()]), clock.clone(), "https://cursor.invalid".into())), clock });
    let controller = AuthController::new("work".into(), CredentialBinding { key: "cursor-work".into(), owner: ProviderCredentialOwner::T3 }, auth.clone(), Arc::new(Stopper), std::time::Duration::from_secs(300)).await;
    (auth, controller, store, send)
}
async fn phase(states: &mut AuthSubscription, expected: ProviderAuthPhase) -> AuthState {
    loop { let state = states.next().await.unwrap(); if state.expose().phase == expected { return state; } }
}
#[tokio::test]
async fn cursor_browser_credentials_are_private_verified_and_cancelled_login_cannot_save_late() {
    let (auth, controller, store, mut send) = authentication(false).await;
    let mut states = controller.subscribe("owner".into());
    let start = controller.start("owner", None, None, None, 0).await.unwrap();
    phase(&mut states, ProviderAuthPhase::Waiting).await;
    assert!(controller.snapshot("other").await.expose().authorization_url.is_none());
    assert!(!format!("{:?}", controller.snapshot("owner").await).contains("secret-key"));
    controller.cancel("owner", &start.expose().flow_id.as_ref().unwrap().0).await.unwrap();
    send.closed().await;
    assert!(send.send(credentials()).is_err());
    assert!(store.load().await.unwrap().is_none());
    assert!(auth.read_api_key().await.unwrap().is_none());

    let (auth, controller, store, send) = authentication(false).await;
    let service = AuthService::default();
    service.register("work".into(), controller.clone()).await;
    let mut states = controller.subscribe("owner".into());
    controller.start("owner", None, None, None, 0).await.unwrap();
    phase(&mut states, ProviderAuthPhase::Waiting).await;
    send.send(credentials()).unwrap();
    phase(&mut states, ProviderAuthPhase::Succeeded).await;
    assert_eq!(auth.read_api_key().await.unwrap().unwrap().expose(), "secret-key");
    assert!(!format!("{:?}", store.load().await.unwrap()).contains("secret-key"));
    let admitted = controller.admit_access().await.unwrap();
    assert!(!service.try_handle_prompt_command(&"work".into(), "/logout", true).await.unwrap());
    assert!(service.try_handle_prompt_command(&"work".into(), " /logout ", false).await.unwrap());
    assert!(admitted.cancellation.is_cancelled());
    assert!(auth.read_api_key().await.unwrap().is_none());
}
#[tokio::test]
async fn configured_cursor_keys_block_browser_mutations_and_commit_cannot_be_interrupted_mid_save() {
    let (auth, controller, _store, _send) = authentication(true).await;
    assert_eq!(auth.read_api_key().await.unwrap().unwrap().expose(), "configured-key");
    assert_eq!(controller.start("owner", None, None, None, 0).await.unwrap_err(), ServiceError::Unsupported);
    assert_eq!(controller.logout().await.unwrap_err(), ServiceError::Unsupported);
    let (auth, controller, store, send) = authentication(false).await;
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    *store.save_gate.lock().await = Some((entered, released));
    let start = controller.start("owner", None, None, None, 0).await.unwrap();
    send.send(credentials()).unwrap();
    waiting.await.unwrap();
    let cancel = tokio::spawn({ let controller = controller.clone(); let flow = start.expose().flow_id.as_ref().unwrap().0.clone(); async move { controller.cancel("owner", &flow).await } });
    release.send(()).unwrap();
    assert_eq!(cancel.await.unwrap().unwrap_err(), ServiceError::Inactive);
    assert!(auth.read_api_key().await.unwrap().is_some());
    assert_eq!(controller.snapshot("owner").await.expose().phase, ProviderAuthPhase::Succeeded);
}
