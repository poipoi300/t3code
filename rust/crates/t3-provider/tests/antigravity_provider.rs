#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use t3_contracts::{provider_instance::ProviderInstanceId, provider_setup::*, wire::WireDateTime};
use t3_provider::{
    antigravity_auth::*,
    antigravity_profile::AuthConfig,
    antigravity_provider::AntigravityProvider,
    antigravity_runtime::*,
    auth::*,
    catalog::Manifest,
    environment::{Environment, Platform},
    instances::InstanceSpec,
    managed_provider::{BackgroundPolicy, ManagedOptions, RuntimeUsageUpdate},
    native_provider::NativeProviderContext,
    sensitive::*,
    service::ProviderInstanceService,
    transport::SystemClock,
};
use tokio::sync::{Notify, mpsc};
#[derive(Default)]
struct Control {
    finish: Notify,
    closed: AtomicUsize,
    created: AtomicUsize,
    stops: AtomicUsize,
    block_health: AtomicBool,
    health_started: Notify,
    health_finish: Notify,
}
struct Factory(Arc<Control>);
struct Runtime {
    control: Arc<Control>,
    authorization: Option<mpsc::Sender<Sensitive<String>>>,
    closed: AtomicBool,
}
impl Drop for Runtime {
    fn drop(&mut self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            self.control.closed.fetch_add(1, Ordering::SeqCst);
        }
    }
}
fn initialized() -> Sensitive<Value> {
    Sensitive::new(
        json!({"agentInfo":{"version":"1.5.3"},"agentCapabilities":{"auth":{"logout":{}}}}),
    )
}
fn started() -> Started {
    Started {
        initialize: initialized(),
        setup: Sensitive::new(
            json!({"sessionId":"temporary-session","models":{"currentModelId":"model","availableModels":[{"modelId":"model","name":"Model"}]}}),
        ),
    }
}
#[async_trait]
impl AntigravityRuntime for Runtime {
    async fn initialize(&self) -> ServiceResult<Sensitive<Value>> {
        if self.authorization.is_none() && self.control.block_health.load(Ordering::SeqCst) {
            self.control.health_started.notify_one();
            self.control.health_finish.notified().await;
        }
        Ok(initialized())
    }
    async fn start(&self) -> ServiceResult<Started> {
        if let Some(urls) = &self.authorization {
            assert!(self.control.stops.load(Ordering::SeqCst) > 0);
            urls.send(Sensitive::new("https://accounts.google.com/o/oauth2/v2/auth?response_type=code&state=secret-state&redirect_uri=http%3A%2F%2F127.0.0.1%3A32123%2F".into())).await.unwrap();
            self.control.finish.notified().await;
        }
        Ok(started())
    }
    async fn logout(&self) -> ServiceResult<()> {
        Ok(())
    }
    async fn close(&self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            self.control.closed.fetch_add(1, Ordering::SeqCst);
        }
    }
}
#[async_trait]
impl AntigravityRuntimeFactory for Factory {
    async fn supports_text_generation(&self) -> bool {
        true
    }
    async fn create(
        &self,
        authorization: Option<mpsc::Sender<Sensitive<String>>>,
    ) -> ServiceResult<Arc<dyn AntigravityRuntime>> {
        self.0.created.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(Runtime {
            control: self.0.clone(),
            authorization,
            closed: AtomicBool::new(false),
        }))
    }
}
struct Forwarder(Arc<Control>);
#[async_trait]
impl CallbackForwarder for Forwarder {
    async fn forward(&self, callback: &Sensitive<url::Url>) -> ServiceResult<()> {
        assert!(!format!("{callback:?}").contains("secret"));
        self.0.finish.notify_one();
        Ok(())
    }
}
struct Stopper(Arc<Control>);
#[async_trait]
impl SessionStopper for Stopper {
    async fn stop(&self, _: &ProviderInstanceId, _: &CredentialBinding) -> ServiceResult<()> {
        self.0.stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
fn provider(root: &std::path::Path, control: Arc<Control>) -> Arc<AntigravityProvider> {
    AntigravityProvider::new(
        Arc::new(InstanceSpec {
            instance_id: "antigravity-work".into(),
            driver: "antigravity".into(),
            enabled: true,
            display_name: None,
            accent_color: None,
            config: json!({"authMethod":"oauth-personal"}),
            variables: vec![],
        }),
        NativeProviderContext {
            environment: Environment::new(),
            home: root.into(),
            cwd: root.into(),
            platform: Platform::current(),
            managed_mcp: false,
            server_version: "1".into(),
        },
        Arc::new(Factory(control)),
        Arc::new(SystemClock),
        Arc::new(Manifest::bundled().unwrap()).into(),
        None,
    )
    .unwrap()
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
async fn client_owned_google_flow_validates_callbacks_stops_sessions_and_clears_managed_usage_on_logout()
 {
    let temp = tempfile::tempdir().unwrap();
    let control = Arc::new(Control::default());
    let provider = provider(temp.path(), control.clone());
    let auth = Arc::new(AntigravityAuth {
        factory: Arc::new(Factory(control.clone())),
        provider: provider.clone(),
        config: AuthConfig::from_settings(&json!({})),
        forwarder: Arc::new(Forwarder(control.clone())),
    });
    let controller = AuthController::new(
        "antigravity-work".into(),
        CredentialBinding {
            key: "private-profile".into(),
            owner: ProviderCredentialOwner::Provider,
        },
        auth,
        Arc::new(Stopper(control.clone())),
        Duration::from_secs(300),
    )
    .await;
    provider.bind_auth(&controller).await;
    let (_policy, background) = tokio::sync::watch::channel(BackgroundPolicy::default());
    let managed = provider
        .clone()
        .into_managed(ManagedOptions::new(background))
        .await
        .unwrap();
    let mut snapshots = managed.subscribe_snapshots().unwrap();
    let mut owner = controller.subscribe("owner".into());
    let flow = controller
        .start("owner", None, None, None, 1000)
        .await
        .unwrap()
        .expose()
        .flow_id
        .clone()
        .unwrap()
        .0;
    let waiting = phase(&mut owner, ProviderAuthPhase::Waiting).await;
    assert!(!format!("{waiting:?}").contains("secret-state"));
    assert!(
        controller
            .snapshot("other")
            .await
            .expose()
            .authorization_url
            .is_none()
    );
    let invalid = controller
        .complete(
            "owner",
            ProviderAuthCompleteInput {
                instance_id: "antigravity-work".into(),
                flow_id: flow.clone().into(),
                callback_url: "http://127.0.0.1:32123/?state=other&code=secret-code".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(invalid, ServiceError::Callback);
    controller
        .complete(
            "owner",
            ProviderAuthCompleteInput {
                instance_id: "antigravity-work".into(),
                flow_id: flow.clone().into(),
                callback_url: "http://127.0.0.1:32123/?state=secret-state&code=secret-code".into(),
            },
        )
        .await
        .unwrap();
    phase(&mut owner, ProviderAuthPhase::Succeeded).await;
    snapshots
        .wait_for(|value| {
            value.as_ref().is_some_and(|snapshot| {
                snapshot.auth.status
                    == t3_contracts::server::ServerProviderAuthStatus::Authenticated
            })
        })
        .await
        .unwrap();
    assert_eq!(provider.current().await.models[0].slug.0, "model");
    managed
        .apply_usage(RuntimeUsageUpdate {
            windows: serde_json::from_value(
                json!([{"id":"weekly","kind":"weekly","label":"Weekly","usedPercent":50}]),
            )
            .unwrap(),
            checked_at: WireDateTime("2026-11-13T10:00:00.000Z".into()),
            complete: true,
            auth_label: None,
        })
        .await;
    assert!(managed.current().await.usage_limits.is_some());
    controller.logout().await.unwrap();
    snapshots
        .wait_for(|value| {
            value.as_ref().is_some_and(|snapshot| {
                snapshot.auth.status
                    == t3_contracts::server::ServerProviderAuthStatus::Unauthenticated
            })
        })
        .await
        .unwrap();
    assert!(managed.current().await.usage_limits.is_none());
    assert!(provider.current().await.models.is_empty());
    assert_eq!(control.stops.load(Ordering::SeqCst), 2);
    assert_eq!(
        control.closed.load(Ordering::SeqCst),
        control.created.load(Ordering::SeqCst)
    );
}
#[tokio::test]
async fn late_health_result_cannot_restore_metadata_after_sign_out() {
    let temp = tempfile::tempdir().unwrap();
    let control = Arc::new(Control::default());
    let provider = provider(temp.path(), control.clone());
    provider.session_started(started(), None).await.unwrap();
    control.block_health.store(true, Ordering::SeqCst);
    let refresh = tokio::spawn({
        let provider = provider.clone();
        async move { provider.snapshot(None, false).await.unwrap() }
    });
    control.health_started.notified().await;
    provider.signed_out().await.unwrap();
    control.health_finish.notify_one();
    let snapshot = refresh.await.unwrap();
    assert_eq!(
        snapshot.auth.status,
        t3_contracts::server::ServerProviderAuthStatus::Unauthenticated
    );
    assert!(snapshot.models.is_empty());
    assert_eq!(snapshot.supports_text_generation, Some(Some(false)));
}

#[tokio::test]
async fn explicit_model_refresh_authenticates_an_empty_session_and_closes_it() {
    let temp = tempfile::tempdir().unwrap();
    let control = Arc::new(Control::default());
    let provider = provider(temp.path(), control.clone());
    assert!(provider.current().await.models.is_empty());
    provider.refresh_models().await.unwrap();
    let snapshot = provider.current().await;
    assert_eq!(snapshot.models[0].slug.0, "model");
    assert_eq!(control.created.load(Ordering::SeqCst), 1);
    assert_eq!(control.closed.load(Ordering::SeqCst), 1);
}
