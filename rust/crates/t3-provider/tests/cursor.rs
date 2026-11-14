#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
};
use t3_provider::{cursor_catalog::*, sensitive::*, transport::*};
use tokio::sync::Mutex;
struct Clock(AtomicI64);
impl ProviderClock for Clock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
struct Http {
    replies: Mutex<BTreeMap<String, VecDeque<HttpResponse>>>,
    calls: Mutex<Vec<HttpRequest>>,
}
#[async_trait]
impl HttpTransport for Http {
    async fn send(&self, request: HttpRequest) -> ServiceResult<HttpResponse> {
        assert!(!format!("{request:?}").contains("secret-key"));
        self.calls.lock().await.push(request.clone());
        self.replies
            .lock()
            .await
            .get_mut(&request.url)
            .and_then(VecDeque::pop_front)
            .ok_or(ServiceError::Network)
    }
}
fn response(status: u16, body: Value) -> HttpResponse {
    HttpResponse {
        status,
        body: Sensitive::new(body),
    }
}
fn user() -> HttpResponse {
    response(
        200,
        json!({"apiKeyName":"T3", "createdAt":"2026-01-01", "userEmail":"ada@example.com"}),
    )
}
fn models() -> HttpResponse {
    response(
        200,
        json!({"items":[{"id":"model","displayName":"Model","parameters":[],"variants":[]}]}),
    )
}
fn http(users: Vec<HttpResponse>, models: Vec<HttpResponse>) -> Arc<Http> {
    Arc::new(Http {
        replies: Mutex::new(BTreeMap::from([
            ("https://cursor.invalid/v1/me".into(), users.into()),
            ("https://cursor.invalid/v1/models".into(), models.into()),
        ])),
        calls: Mutex::new(vec![]),
    })
}
#[tokio::test]
async fn account_labels_are_always_fresh_while_nonempty_models_are_cached_per_key_for_thirty_minutes()
 {
    let http = http(
        vec![user(), user(), user(), user()],
        vec![models(), models(), models()],
    );
    let clock = Arc::new(Clock(AtomicI64::new(0)));
    let catalog = CursorCatalog::new(
        http.clone(),
        clock.clone(),
        "https://cursor.invalid/".into(),
    );
    let key = Sensitive::new("secret-key".into());
    for _ in 0..2 {
        assert_eq!(catalog.read(&key).await.unwrap().models.len(), 1);
    }
    clock.0.store(30 * 60 * 1000, Ordering::SeqCst);
    catalog.read(&key).await.unwrap();
    catalog
        .read(&Sensitive::new("second-secret-key".into()))
        .await
        .unwrap();
    let calls = http.calls.lock().await;
    assert_eq!(
        calls
            .iter()
            .filter(|request| request.url.ends_with("/me"))
            .count(),
        4
    );
    assert_eq!(
        calls
            .iter()
            .filter(|request| request.url.ends_with("/models"))
            .count(),
        3
    );
    assert_eq!(
        calls[0].headers["authorization"].expose(),
        "Bearer secret-key"
    );
}
#[tokio::test]
async fn blocked_account_labels_allow_models_but_plan_and_authentication_failures_remain_distinct()
{
    for (status, body, allowed, authentication, plan) in [
        (
            403,
            json!({"error":{"code":"enterprise_blocked","message":"secret-key"}}),
            true,
            false,
            false,
        ),
        (
            403,
            json!({"error":{"code":"plan_required","message":"secret-key"}}),
            false,
            false,
            true,
        ),
        (
            401,
            json!({"code":"rejected","message":"secret-key"}),
            false,
            true,
            false,
        ),
    ] {
        let catalog = CursorCatalog::new(
            http(vec![response(status, body)], vec![models()]),
            Arc::new(Clock(AtomicI64::new(0))),
            "https://cursor.invalid".into(),
        );
        match catalog.read(&Sensitive::new("secret-key".into())).await {
            Ok(snapshot) => {
                assert!(allowed);
                assert!(snapshot.user.is_none());
                assert_eq!(snapshot.models.len(), 1);
            }
            Err(error) => {
                assert!(!allowed);
                assert_eq!(error.authentication_failure, authentication);
                assert_eq!(error.plan_required, plan);
                assert!(!format!("{error:?} {error}").contains("secret-key"));
            }
        }
    }
}
#[tokio::test]
async fn empty_or_malformed_model_responses_are_not_cached_and_explicit_optional_nulls_are_rejected()
 {
    let invalid = response(
        200,
        json!({"items":[{"id":"model","displayName":"Model","parameters":null}]}),
    );
    let http = http(
        vec![user(), user(), user(), user()],
        vec![response(200, json!({"items":[]})), invalid, models()],
    );
    let catalog = CursorCatalog::new(
        http.clone(),
        Arc::new(Clock(AtomicI64::new(0))),
        "https://cursor.invalid".into(),
    );
    let key = Sensitive::new("secret-key".into());
    assert!(catalog.read(&key).await.unwrap().models.is_empty());
    assert!(catalog.read(&key).await.is_err());
    assert_eq!(catalog.read(&key).await.unwrap().models.len(), 1);
    catalog.read(&key).await.unwrap();
    assert_eq!(
        http.calls
            .lock()
            .await
            .iter()
            .filter(|request| request.url.ends_with("/models"))
            .count(),
        3
    );
}

use t3_contracts::{provider_instance::ProviderInstanceId, provider_setup::*};
use t3_provider::{auth::*, cursor_auth::*};
#[derive(Default)]
struct Store {
    value: Mutex<Option<CursorCredentials>>,
    save_gate: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
}
#[async_trait]
impl CursorCredentialStore for Store {
    async fn load(&self) -> ServiceResult<Option<CursorCredentials>> {
        Ok(self.value.lock().await.clone())
    }
    async fn save(&self, credentials: &CursorCredentials) -> ServiceResult<()> {
        if let Some((started, wait)) = self.save_gate.lock().await.take() {
            started.send(()).unwrap();
            wait.await.unwrap();
        }
        *self.value.lock().await = Some(credentials.clone());
        Ok(())
    }
    async fn clear(&self) -> ServiceResult<()> {
        *self.value.lock().await = None;
        Ok(())
    }
}
struct Login(Mutex<Option<tokio::sync::oneshot::Receiver<CursorCredentials>>>);
#[async_trait]
impl CursorLogin for Login {
    async fn login(
        &self,
        name: String,
        urls: tokio::sync::mpsc::UnboundedSender<SecretString>,
        _: t3_process::CancellationToken,
    ) -> ServiceResult<CursorCredentials> {
        assert_eq!(name, "T3 Code - Work");
        urls.send(Sensitive::new(
            "https://cursor.invalid/login?nonce=secret-key".into(),
        ))
        .unwrap();
        self.0
            .lock()
            .await
            .take()
            .unwrap()
            .await
            .map_err(|_| ServiceError::Cancelled)
    }
}
struct Stopper;
#[async_trait]
impl SessionStopper for Stopper {
    async fn stop(&self, _: &ProviderInstanceId, _: &CredentialBinding) -> ServiceResult<()> {
        Ok(())
    }
}
fn credentials() -> CursorCredentials {
    serde_json::from_value(json!({"version":1,"backendUrl":"https://api.cursor.com","apiKey":"secret-key","createdAtMs":0,"apiKeyExpiresAtMs":10000,"email":"ada@example.com"})).unwrap()
}
async fn authentication(
    configured: bool,
) -> (
    Arc<CursorAuth>,
    Arc<AuthController>,
    Arc<Store>,
    tokio::sync::oneshot::Sender<CursorCredentials>,
) {
    let (send, receive) = tokio::sync::oneshot::channel();
    let store = Arc::new(Store::default());
    let clock = Arc::new(Clock(AtomicI64::new(0)));
    let auth = Arc::new(CursorAuth {
        enabled: true,
        display_name: "Work".into(),
        configured_key: configured.then(|| Sensitive::new(" configured-key ".into())),
        store: store.clone(),
        login: Arc::new(Login(Mutex::new(Some(receive)))),
        catalog: Arc::new(CursorCatalog::new(
            http(vec![user()], vec![models()]),
            clock.clone(),
            "https://cursor.invalid".into(),
        )),
        clock,
    });
    let controller = AuthController::new(
        "work".into(),
        CredentialBinding {
            key: "cursor-work".into(),
            owner: ProviderCredentialOwner::T3,
        },
        auth.clone(),
        Arc::new(Stopper),
        std::time::Duration::from_secs(300),
    )
    .await;
    (auth, controller, store, send)
}
async fn phase(states: &mut AuthSubscription, expected: ProviderAuthPhase) -> AuthState {
    loop {
        let state = states.next().await.unwrap();
        if state.expose().phase == expected {
            return state;
        }
    }
}
#[tokio::test]
async fn cursor_browser_credentials_are_private_verified_and_cancelled_login_cannot_save_late() {
    let (auth, controller, store, mut send) = authentication(false).await;
    let mut states = controller.subscribe("owner".into());
    let start = controller
        .start("owner", None, None, None, 0)
        .await
        .unwrap();
    phase(&mut states, ProviderAuthPhase::Waiting).await;
    assert!(
        controller
            .snapshot("other")
            .await
            .expose()
            .authorization_url
            .is_none()
    );
    assert!(!format!("{:?}", controller.snapshot("owner").await).contains("secret-key"));
    controller
        .cancel("owner", &start.expose().flow_id.as_ref().unwrap().0)
        .await
        .unwrap();
    send.closed().await;
    assert!(send.send(credentials()).is_err());
    assert!(store.load().await.unwrap().is_none());
    assert!(auth.read_api_key().await.unwrap().is_none());

    let (auth, controller, store, send) = authentication(false).await;
    let service = AuthService::default();
    service.register("work".into(), controller.clone()).await;
    let mut states = controller.subscribe("owner".into());
    controller
        .start("owner", None, None, None, 0)
        .await
        .unwrap();
    phase(&mut states, ProviderAuthPhase::Waiting).await;
    send.send(credentials()).unwrap();
    phase(&mut states, ProviderAuthPhase::Succeeded).await;
    assert_eq!(
        auth.read_api_key().await.unwrap().unwrap().expose(),
        "secret-key"
    );
    assert!(!format!("{:?}", store.load().await.unwrap()).contains("secret-key"));
    let admitted = controller.admit_access().await.unwrap();
    assert!(
        !service
            .try_handle_prompt_command(&"work".into(), "/logout", true)
            .await
            .unwrap()
    );
    assert!(
        service
            .try_handle_prompt_command(&"work".into(), " /logout ", false)
            .await
            .unwrap()
    );
    assert!(admitted.cancellation.is_cancelled());
    assert!(auth.read_api_key().await.unwrap().is_none());
}
#[tokio::test]
async fn configured_cursor_keys_block_browser_mutations_and_commit_cannot_be_interrupted_mid_save()
{
    let (auth, controller, _store, _send) = authentication(true).await;
    assert_eq!(
        auth.read_api_key().await.unwrap().unwrap().expose(),
        "configured-key"
    );
    assert_eq!(
        controller
            .start("owner", None, None, None, 0)
            .await
            .unwrap_err(),
        ServiceError::Unsupported
    );
    assert_eq!(
        controller.logout().await.unwrap_err(),
        ServiceError::Unsupported
    );
    let (auth, controller, store, send) = authentication(false).await;
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    *store.save_gate.lock().await = Some((entered, released));
    let start = controller
        .start("owner", None, None, None, 0)
        .await
        .unwrap();
    send.send(credentials()).unwrap();
    waiting.await.unwrap();
    let cancel = tokio::spawn({
        let controller = controller.clone();
        let flow = start.expose().flow_id.as_ref().unwrap().0.clone();
        async move { controller.cancel("owner", &flow).await }
    });
    release.send(()).unwrap();
    assert_eq!(cancel.await.unwrap().unwrap_err(), ServiceError::Inactive);
    assert!(auth.read_api_key().await.unwrap().is_some());
    assert_eq!(
        controller.snapshot("owner").await.expose().phase,
        ProviderAuthPhase::Succeeded
    );
}

#[tokio::test]
async fn cursor_credential_migration_preserves_the_secret_binding_and_never_overwrites_a_valid_login()
 {
    let directory = tempfile::tempdir().unwrap();
    let legacy = directory.path().join("legacy.json");
    let store = ConfigCursorCredentials::new(directory.path().join("secrets"), "work");
    assert_eq!(
        store.binding_key(),
        t3_provider::secrets::credential_binding_key("cursor", "work")
    );
    std::fs::write(&legacy, serde_json::to_vec(&credentials()).unwrap()).unwrap();
    store.migrate_legacy(&legacy).await.unwrap();
    assert!(!legacy.exists());
    assert_eq!(
        store.load().await.unwrap().unwrap().api_key.expose(),
        "secret-key"
    );
    let mut current = credentials();
    current.api_key = Sensitive::new("current-secret".into());
    store.save(&current).await.unwrap();
    std::fs::write(&legacy, serde_json::to_vec(&credentials()).unwrap()).unwrap();
    store.migrate_legacy(&legacy).await.unwrap();
    assert_eq!(
        store.load().await.unwrap().unwrap().api_key.expose(),
        "current-secret"
    );
    assert!(!format!("{:?}", store.load().await.unwrap()).contains("current-secret"));
    store.clear().await.unwrap();
    assert!(store.load().await.unwrap().is_none());
    let invalid = json!({"version":1,"backendUrl":"https://cursor.invalid","apiKey":"secret-key","createdAtMs":0,"apiKeyExpiresAtMs":null});
    assert!(serde_json::from_value::<CursorCredentials>(invalid).is_err());
}
#[tokio::test]
async fn cursor_usage_exchanges_keys_once_and_preserves_dashboard_percentages_and_custom_endpoint_rules()
 {
    use t3_provider::{cursor_usage, environment::Environment};
    let reply = json!({"billingCycleStart":1800000000000i64,"billingCycleEnd":"1802592000000","planUsage":{"totalPercentUsed":120,"autoPercentUsed":32.5,"apiPercentUsed":-4}});
    let http = Http {
        replies: Mutex::new(BTreeMap::from([
            (
                "https://api2.cursor.sh/auth/exchange_user_api_key".into(),
                vec![response(200, json!({"accessToken":"secret-access"}))].into(),
            ),
            (
                "https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage".into(),
                vec![response(200, reply)].into(),
            ),
        ])),
        calls: Mutex::new(vec![]),
    };
    let environment = Environment::from([("CURSOR_API_KEY".into(), "secret-key".into())]);
    let limits = serde_json::to_value(
        cursor_usage::read(&http, None, &environment, "2027-01-15T08:00:00.000Z")
            .await
            .unwrap(),
    )
    .unwrap();
    let windows = limits["windows"].as_array().unwrap();
    assert_eq!(
        windows
            .iter()
            .find(|window| window["id"] == "totalPercentUsed")
            .unwrap()["usedPercent"],
        100.
    );
    assert_eq!(
        windows
            .iter()
            .find(|window| window["id"] == "autoPercentUsed")
            .unwrap()["usedPercent"],
        32.5
    );
    assert_eq!(
        windows
            .iter()
            .find(|window| window["id"] == "apiPercentUsed")
            .unwrap()["usedPercent"],
        0.
    );
    assert_eq!(windows[0]["windowDurationMins"], 43200);
    let calls = http.calls.lock().await;
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[1].headers["authorization"].expose(),
        "Bearer secret-access"
    );
    assert_eq!(calls[1].headers["connect-protocol-version"].expose(), "1");
    drop(calls);
    let unsupported = serde_json::to_value(
        cursor_usage::read(
            &http,
            Some("https://custom.invalid"),
            &environment,
            "2027-01-15T08:00:00.000Z",
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(unsupported["unavailable"]["reason"], "unsupported");
    assert_eq!(http.calls.lock().await.len(), 2);
    assert!(cursor_usage::limits(&json!({"planUsage":null}), "2027-01-15T08:00:00.000Z").is_err());
}
#[tokio::test]
async fn cursor_status_uses_the_shipped_sdk_http_catalog_and_disabled_instances_make_no_requests() {
    use t3_provider::{
        catalog::Manifest,
        cursor_provider::CursorProvider,
        environment::{Environment, Platform},
        instances::InstanceSpec,
        native_provider::NativeProviderContext,
        service::ProviderInstanceService,
    };
    let directory = tempfile::tempdir().unwrap();
    for enabled in [true, false] {
        let (auth, _controller, _store, _send) = authentication(true).await;
        let usage = http(vec![], vec![]);
        let provider = CursorProvider::new(
            Arc::new(InstanceSpec {
                instance_id: "work".into(),
                driver: "cursor".into(),
                display_name: Some("Cursor Work".into()),
                accent_color: None,
                enabled,
                config: json!({"customModels":["custom-model"]}),
                variables: vec![],
            }),
            NativeProviderContext {
                environment: Environment::new(),
                home: directory.path().into(),
                cwd: directory.path().into(),
                platform: Platform::current(),
                managed_mcp: false,
                server_version: "0.1.0".into(),
            },
            auth,
            usage.clone(),
            Arc::new(Manifest::bundled().unwrap()).into(),
            Arc::new(Clock(AtomicI64::new(0))),
        );
        let (_policy, background) =
            tokio::sync::watch::channel(t3_provider::managed_provider::BackgroundPolicy::default());
        let provider = provider
            .into_managed(
                t3_provider::managed_provider::ManagedOptions::new(background),
                None,
            )
            .await
            .unwrap();
        assert!(usage.calls.lock().await.is_empty());
        let snapshot = serde_json::to_value(provider.snapshot(None, true).await.unwrap()).unwrap();
        assert_eq!(snapshot["installed"], enabled);
        assert_eq!(snapshot["setup"]["canAuthenticate"], false);
        assert_eq!(snapshot["auth"]["canLogout"], false);
        assert_eq!(
            snapshot["status"],
            if enabled { "ready" } else { "disabled" }
        );
        assert!(snapshot["version"].is_null());
        if enabled {
            assert_eq!(snapshot["models"].as_array().unwrap().len(), 2);
            assert_eq!(snapshot["auth"]["label"], "Cursor API key (T3)");
            assert_eq!(snapshot["auth"]["email"], "ada@example.com");
            let project = directory.path().join("workspace");
            std::fs::create_dir_all(project.join(".cursor/skills/project")).unwrap();
            std::fs::write(
                project.join(".cursor/skills/project/SKILL.md"),
                "# Instructions",
            )
            .unwrap();
            let workspace = serde_json::to_value(
                provider
                    .snapshot(Some(project.to_str().unwrap()), true)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(workspace["auth"]["status"], "authenticated");
            assert_eq!(
                workspace["workspaceSnapshots"][0]["skills"][0]["name"],
                "project"
            );
            assert!(workspace["skills"].as_array().unwrap().is_empty());
        } else {
            assert!(usage.calls.lock().await.is_empty());
            assert_eq!(snapshot["models"].as_array().unwrap().len(), 1);
        }
    }
}
#[test]
fn cursor_windows_defaults_require_auto_review_for_the_unsandboxed_path_but_explicit_policy_wins() {
    use t3_contracts::provider_policy::{ProviderInteractionMode, RuntimeMode};
    use t3_provider::{environment::Platform, modes};
    for (mode, review, sandbox_windows, sandbox_linux) in [
        (RuntimeMode::ApprovalRequired, true, false, true),
        (RuntimeMode::AutoAcceptEdits, false, true, true),
        (RuntimeMode::Auto, true, false, true),
        (RuntimeMode::FullAccess, false, false, false),
    ] {
        let windows = modes::cursor(
            mode,
            ProviderInteractionMode::Default,
            None,
            None,
            Platform::Windows,
        );
        let linux = modes::cursor(
            mode,
            ProviderInteractionMode::Default,
            None,
            None,
            Platform::Linux,
        );
        assert_eq!(
            (windows.auto_review, windows.sandbox_enabled),
            (review, sandbox_windows)
        );
        assert_eq!(linux.sandbox_enabled, sandbox_linux);
        let explicit = modes::cursor(
            mode,
            ProviderInteractionMode::Plan,
            Some("never"),
            Some("readOnly"),
            Platform::Windows,
        );
        assert_eq!(
            (
                explicit.auto_review,
                explicit.sandbox_enabled,
                explicit.mode
            ),
            (false, true, "plan")
        );
    }
}

#[tokio::test]
async fn empty_http_authentication_failures_preserve_status_and_never_expose_response_bytes() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut replies = tokio::task::JoinSet::new();
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            replies.spawn(async move {
                let mut request=[0;4096]; let count=socket.read(&mut request).await.unwrap();
                let request=String::from_utf8_lossy(&request[..count]);
                let reply=if request.starts_with("GET /v1/me ") {
                    "HTTP/1.1 401 Unauthorized\r\nContent-Length: 10\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nsecret-key"
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Length: 12\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"items\":[]}"
                };
                let _=socket.write_all(reply.as_bytes()).await;
            });
        }
        while let Some(result) = replies.join_next().await {
            result.unwrap();
        }
    });
    let catalog = CursorCatalog::new(
        Arc::new(ReqwestTransport::new().unwrap()),
        Arc::new(Clock(AtomicI64::new(0))),
        format!("http://{address}"),
    );
    let error = match catalog.read(&Sensitive::new("secret-key".into())).await {
        Err(error) => error,
        Ok(_) => panic!("unauthorized account"),
    };
    assert!(!format!("{error:?} {error}").contains("secret-key"));
    assert!(error.authentication_failure);
    server.await.unwrap();
}
