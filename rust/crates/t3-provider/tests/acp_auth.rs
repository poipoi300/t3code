#![allow(clippy::unwrap_used)]
mod common;
use async_trait::async_trait;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use t3_contracts::{provider_instance::ProviderInstanceId, provider_setup::*};
use t3_provider::{
    acp_auth::AcpAuth,
    acp_auth_state::AcpAuthenticationState,
    acp_coordinator::AcpRuntimeCoordinator,
    auth::*,
    environment::{Environment, Platform},
    registry::{RegistryCatalog, RegistryFetch},
    registry_installation::{ProcessRegistryCommands, ReqwestRegistryDownloads},
    registry_service::{RegistryAgentCatalog, RegistryCatalogOptions},
    sensitive::*,
    transport::SystemClock,
};
struct NoFetch;
#[async_trait]
impl RegistryFetch for NoFetch {
    async fn bytes(&self, _: &str) -> ServiceResult<Vec<u8>> {
        panic!("local sign-in must not fetch a registry")
    }
}
struct Stopper;
#[async_trait]
impl SessionStopper for Stopper {
    async fn stop(&self, _: &ProviderInstanceId, _: &CredentialBinding) -> ServiceResult<()> {
        Ok(())
    }
}
async fn phase(subscription: &mut AuthSubscription, expected: ProviderAuthPhase) -> AuthState {
    loop {
        let state = subscription.next().await.unwrap();
        if state.expose().phase == ProviderAuthPhase::Failed {
            assert_eq!(state.expose().phase, expected);
        }
        if state.expose().phase == expected {
            return state;
        }
    }
}
#[tokio::test]
async fn acp_confirmation_binding_matches_javascript_ignores_cosmetics_and_invalidates_profile_or_command_changes()
 {
    let root = tempfile::tempdir().unwrap();
    let settings = json!({"source":"local","commandArgs":["--acp"],"commandPath":"fixture"});
    let instance: ProviderInstanceId = "acp".into();
    let state = AcpAuthenticationState::load(
        root.path(),
        &instance,
        &settings,
        &vec![],
        &Environment::new(),
    )
    .await;
    assert!(!state.get());
    state.set(true).await.unwrap();
    let file = std::fs::read_dir(root.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let persisted: serde_json::Value =
        serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    assert_eq!(
        persisted["binding"],
        "5fec8bba5d8a8e6d75072d1a58950be0f4bd2bffa53fc1a895a5a7b610517bad"
    );
    let mut cosmetic = settings.clone();
    cosmetic["customModels"] = json!(["other"]);
    assert!(
        AcpAuthenticationState::load(
            root.path(),
            &instance,
            &cosmetic,
            &vec![],
            &Environment::new()
        )
        .await
        .get()
    );
    assert!(
        !AcpAuthenticationState::load(
            root.path(),
            &instance,
            &settings,
            &vec![],
            &Environment::from([("HOME".into(), "different".into())])
        )
        .await
        .get()
    );
    assert!(!format!("{state:?}").contains("fixture"));
}

#[test]
fn cancelled_confirmation_writes_hold_the_persistence_lock_until_the_write_finishes() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let root = tempfile::tempdir().unwrap();
        let settings = json!({"source":"local","commandPath":"fixture"});
        let state = AcpAuthenticationState::load(
            root.path(),
            &"acp".into(),
            &settings,
            &vec![],
            &Environment::new(),
        )
        .await;
        let (ready, started) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let blocking = tokio::task::spawn_blocking(move || {
            ready.send(()).unwrap();
            blocked.recv().unwrap();
        });
        started.await.unwrap();
        let mut updates = state.subscribe();
        let first = tokio::spawn({
            let state = state.clone();
            async move { state.set(true).await }
        });
        updates.changed().await.unwrap();
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        let second = state.set(false);
        tokio::pin!(second);
        std::future::poll_fn(|cx| {
            assert!(second.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert!(
            state.get(),
            "the next write must wait for the cancelled write's persistence owner"
        );
        release.send(()).unwrap();
        blocking.await.unwrap();
        second.await.unwrap();
        assert!(
            !AcpAuthenticationState::load(
                root.path(),
                &"acp".into(),
                &settings,
                &vec![],
                &Environment::new()
            )
            .await
            .get()
        );
    });
}
#[tokio::test]
async fn acp_agent_auth_requires_private_browser_consent_uses_the_configured_method_and_confirms_only_after_verification()
 {
    let fixture = common::Fixture::new(
        json!({"protocol":"acp","expectedAuthMethod":"second","loginUrl":"https://auth.invalid/?never-format-code","initialize":{"protocolVersion":1,"agentCapabilities":{"auth":{"logout":{}}},"authMethods":[{"id":"first","name":"First"},{"id":"second","name":"Second"},{"id":"terminal","name":"Terminal","type":"terminal"}]}}),
    );
    let settings = json!({"source":"local","commandPath":fixture.binary(),"authMethodId":"second"});
    let confirmed = AcpAuthenticationState::load(
        fixture.directory.path(),
        &"acp".into(),
        &settings,
        &vec![],
        &fixture.environment,
    )
    .await;
    let backend = Arc::new(AcpAuth::new(
        &native(&fixture, settings),
        true,
        fixture.directory.path().into(),
        confirmed.clone(),
    ));
    let methods = backend.methods().await.unwrap();
    assert_eq!(methods.len(), 2);
    let controller = AuthController::new(
        "acp".into(),
        backend.binding(),
        backend,
        Arc::new(Stopper),
        Duration::from_secs(60),
    )
    .await;
    let mut subscription = controller.subscribe("owner".into());
    controller
        .start("owner", None, None, None, 0)
        .await
        .unwrap();
    let waiting = phase(&mut subscription, ProviderAuthPhase::Waiting).await;
    assert!(!format!("{waiting:?}").contains("never-format-code"));
    assert!(!confirmed.get());
    let flow = waiting.expose().flow_id.as_ref().unwrap();
    // The server mints the interaction id; the agent's own (`auth-url`) is never shown.
    let Some(Some(t3_contracts::provider_setup::ProviderAuthInteraction::Browser { id, .. })) =
        &waiting.expose().interaction
    else {
        panic!("no browser interaction")
    };
    assert_ne!(id.0, "auth-url");
    controller
        .respond(
            "owner",
            ProviderAuthRespondInput {
                instance_id: "acp".into(),
                flow_id: flow.clone(),
                interaction_id: id.clone(),
                response: ProviderAuthResponse::Browser {
                    action: ProviderBrowserAuthAction::Accept,
                },
            },
        )
        .await
        .unwrap();
    phase(&mut subscription, ProviderAuthPhase::Succeeded).await;
    assert!(confirmed.get());
    controller.logout().await.unwrap();
    assert!(!confirmed.get());
    controller.close();
}

fn native(
    fixture: &common::Fixture,
    settings: serde_json::Value,
) -> t3_provider::acp_probe::AcpNativeServices {
    let clock = Arc::new(SystemClock);
    let catalog = RegistryAgentCatalog::new(
        RegistryCatalogOptions {
            cache_dir: fixture.directory.path().join("cache"),
            tools_dir: fixture.directory.path().join("tools"),
            platform: Platform::current(),
            architecture: std::env::consts::ARCH.into(),
            environment: fixture.environment.clone(),
        },
        Arc::new(RegistryCatalog::with_fetcher(Arc::new(NoFetch), None)),
        t3_config::settings::SettingsService::new(fixture.directory.path().join("settings.json")),
        Arc::new(ProcessRegistryCommands {
            cwd: fixture.directory.path().into(),
        }),
        Arc::new(ReqwestRegistryDownloads::new().unwrap()),
        clock.clone(),
    );
    t3_provider::acp_probe::AcpNativeServices {
        instance_id: "acp".into(),
        settings,
        environment: Sensitive::new(fixture.environment.clone()),
        catalog: Arc::new(catalog),
        coordinator: Arc::new(AcpRuntimeCoordinator::new(clock)),
    }
}

struct Terminal {
    config: std::path::PathBuf,
    success: bool,
}
#[async_trait]
impl t3_provider::acp_auth::AcpTerminalLogin for Terminal {
    async fn login(
        &self,
        _: &t3_provider::registry_service::ResolvedRegistryAgent,
        method: Sensitive<serde_json::Value>,
        _: &mut AuthContext,
    ) -> ServiceResult<()> {
        assert_eq!(method.expose()["id"], "terminal");
        assert!(!format!("{method:?}").contains("never-terminal-secret"));
        if !self.success {
            return Err(ServiceError::Response);
        }
        let mut config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&self.config).unwrap()).unwrap();
        config["authRequired"] = json!(false);
        std::fs::write(&self.config, serde_json::to_vec(&config).unwrap()).unwrap();
        Ok(())
    }
}
#[tokio::test]
async fn terminal_auth_is_optional_and_reconnects_to_verify_before_confirming() {
    for success in [false, true] {
        let fixture = common::Fixture::new(
            json!({"protocol":"acp","authRequired":true,"initialize":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[{"id":"terminal","name":"Login","type":"terminal","env":{"KEY":"never-terminal-secret"},"args":["login"]}]}}),
        );
        let settings = json!({"source":"local","commandPath":fixture.binary()});
        let native = native(&fixture, settings.clone());
        let confirmed = AcpAuthenticationState::load(
            fixture.directory.path(),
            &"acp".into(),
            &settings,
            &vec![],
            &fixture.environment,
        )
        .await;
        let mut backend = AcpAuth::new(
            &native,
            true,
            fixture.directory.path().into(),
            confirmed.clone(),
        );
        assert!(backend.methods().await.unwrap().is_empty());
        backend.terminal = Some(Arc::new(Terminal {
            config: fixture.config.clone(),
            success,
        }));
        assert_eq!(backend.methods().await.unwrap().len(), 1);
        let binding = backend.binding();
        let controller = AuthController::new(
            "acp".into(),
            binding,
            Arc::new(backend),
            Arc::new(Stopper),
            Duration::from_secs(60),
        )
        .await;
        let mut subscription = controller.subscribe("owner".into());
        controller
            .start("owner", None, None, None, 0)
            .await
            .unwrap();
        phase(
            &mut subscription,
            if success {
                ProviderAuthPhase::Succeeded
            } else {
                ProviderAuthPhase::Failed
            },
        )
        .await;
        assert_eq!(confirmed.get(), success);
        let calls = fixture.calls();
        assert_eq!(
            calls
                .iter()
                .flatten()
                .filter(|method| *method == "session/new")
                .count(),
            usize::from(success)
        );
        assert!(
            !calls
                .iter()
                .flatten()
                .any(|method| method == "authenticate" || method == "session/prompt")
        );
        controller.close();
    }
}
