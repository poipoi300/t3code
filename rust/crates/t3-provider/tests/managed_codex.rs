#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_contracts::{provider_setup::*, server::ServerProviderState};
use t3_provider::{
    auth::*,
    catalog::Manifest,
    chatgpt::*,
    codex_installation::*,
    discovery::{Driver, VersionProbe},
    environment::{Environment, Platform, codex_home_layout},
    instances::InstanceSpec,
    managed_codex::*,
    native_probe::{NativeProbe, ProbeLaunch},
    native_provider::*,
    sensitive::*,
    service::ProviderInstanceService,
    token_broker::*,
    transport::*,
};
use tokio::sync::{Mutex, watch};

struct Effects {
    binary: PathBuf,
    store: Mutex<SavedProfiles>,
    launches: Mutex<Vec<ProbeLaunch>>,
}
impl ProviderClock for Effects {
    fn now_ms(&self) -> i64 {
        1_800_000_000_000
    }
}
#[async_trait]
impl ProfileStore for Effects {
    async fn load(&self) -> ServiceResult<SavedProfiles> {
        Ok(self.store.lock().await.clone())
    }
    async fn save(&self, saved: &SavedProfiles) -> ServiceResult<()> {
        *self.store.lock().await = saved.clone();
        Ok(())
    }
}
#[async_trait]
impl HttpTransport for Effects {
    async fn send(&self, request: HttpRequest) -> ServiceResult<HttpResponse> {
        assert_eq!(request.method, "GET");
        assert_eq!(request.url, "https://api.openai.com/v1/models");
        assert_eq!(
            request.headers["authorization"].expose(),
            "Bearer never-log-managed-token"
        );
        assert!(!format!("{request:?}").contains("never-log"));
        Ok(HttpResponse {
            status: 200,
            body: Sensitive::new(json!({"models":[
                {"slug":"gpt-6-luna","display_name":"Account Luna","visibility":"list"},
                {"slug":"internal","display_name":"Hidden","visibility":"hide"}
            ]})),
        })
    }
}
#[async_trait]
impl IdentityVerifier for Effects {
    async fn verify(
        &self,
        _: &SecretString,
        _: &Discovery,
        _: &str,
        _: i64,
    ) -> ServiceResult<VerifiedIdentity> {
        panic!("no login")
    }
}
impl Entropy for Effects {
    fn bytes(&self, _: usize) -> ServiceResult<Vec<u8>> {
        panic!("no login")
    }
}
#[async_trait]
impl CallbackFactory for Effects {
    async fn listen(&self) -> ServiceResult<Box<dyn CallbackListener>> {
        panic!("no browser")
    }
}
#[async_trait]
impl LocalCodex for Effects {
    async fn resolve(&self) -> ServiceResult<Option<CodexExecutable>> {
        Ok(Some(CodexExecutable {
            path: self.binary.clone(),
            version: "0.166.0".into(),
            source: ProviderInstallSource::Local,
            managed_directory: None,
        }))
    }
}
#[async_trait]
impl ReleaseTransport for Effects {
    async fn latest(&self, _: &str) -> ServiceResult<Option<ReleaseAsset>> {
        panic!("no release requests")
    }
    async fn download(
        &self,
        _: &ReleaseAsset,
        _: &Path,
        _: watch::Sender<u64>,
    ) -> ServiceResult<()> {
        panic!("no download")
    }
}
#[async_trait]
impl CodexExecutableValidator for Effects {
    async fn validate(&self, _: &Path, _: &str, _: &Environment) -> ServiceResult<()> {
        panic!("no installation")
    }
}
#[async_trait]
impl SessionStopper for Effects {
    async fn stop(
        &self,
        _: &t3_contracts::provider_instance::ProviderInstanceId,
        _: &CredentialBinding,
    ) -> ServiceResult<()> {
        Ok(())
    }
}
#[async_trait]
impl ExecutableProbe for Effects {
    async fn version(
        &self,
        _: Driver,
        _: &str,
        _: &Environment,
        _: &Path,
    ) -> ServiceResult<VersionProbe> {
        Ok(VersionProbe {
            installed: true,
            version: Some("0.166.0".into()),
            status: ServerProviderState::Ready,
            message: None,
            resolved_executable: None,
        })
    }
}
#[async_trait]
impl NativeProbe for Effects {
    async fn codex(&self, launch: ProbeLaunch) -> ServiceResult<Sensitive<Value>> {
        self.launches.lock().await.push(launch);
        Ok(Sensitive::new(
            json!({"account":{"account":{"type":"apiKey"},"requiresOpenaiAuth":false},"initialize":{"userAgent":"codex/0.166.0"},"nativeUsage":false,"config":{"config":{"model_provider":"openai_token_sharing"}},"models":[{"id":"gpt-6-luna","model":"gpt-6-luna","displayName":"Native Luna","description":"","hidden":false,"isDefault":true,"supportedReasoningEfforts":[{"reasoningEffort":"low","description":"Low"}],"defaultReasoningEffort":"low"}],"skills":{"data":[]}}),
        ))
    }
    async fn claude(&self, _: ProbeLaunch) -> ServiceResult<Sensitive<Value>> {
        panic!("wrong provider")
    }
}

async fn setup() -> (tempfile::TempDir, Arc<Effects>, ManagedCodexProvider) {
    let directory = tempfile::tempdir().unwrap();
    let shared = directory.path().join("shared");
    std::fs::create_dir(&shared).unwrap();
    std::fs::write(shared.join("auth.json"), "native login").unwrap();
    let effects = Arc::new(Effects {
        binary: directory.path().join("fixture-codex.exe"),
        store: Mutex::new(SavedProfiles {
            sessions: Sessions { active_client_id: Some("client".into()), sessions: vec![serde_json::from_value(json!({"clientId":"client","accessToken":"never-log-managed-token","refreshToken":null,"idToken":"never-log-id","issuer":"https://auth.openai.com","expiresAt":1_800_003_600_000i64,"earliestRefreshAt":null,"scopes":[REQUIRED_SCOPE],"subject":"subject","email":"ada@example.com"})).unwrap()] },
            registrations: Default::default(),
        }), launches: Mutex::new(vec![]),
    });
    let manifest = Arc::new(Manifest::bundled().unwrap());
    let auth = ChatGptAuth::new(
        effects.clone(),
        effects.clone(),
        effects.clone(),
        effects.clone(),
        effects.clone(),
        effects.clone(),
        "environment".into(),
    );
    let controller = AuthController::new(
        "work".into(),
        CredentialBinding {
            key: "binding".into(),
            owner: ProviderCredentialOwner::T3,
        },
        auth.clone(),
        effects.clone(),
        Duration::from_secs(300),
    )
    .await;
    let installation = CodexInstallation::new(
        InstallationOptions {
            base_directory: directory.path().join("state"),
            platform: Platform::current(),
            environment: Environment::new(),
            pin: None,
            manifest: manifest.clone().into(),
            server_version: "0.1.0".into(),
        },
        effects.clone(),
        effects.clone(),
        effects.clone(),
        effects.clone(),
    );
    let broker = ManagedTokenBroker::new("environment".into());
    broker.register("work".into(), auth.clone()).await;
    #[cfg(unix)]
    let uid = {
        use std::os::unix::fs::MetadataExt;
        Some(std::fs::metadata(directory.path()).unwrap().uid())
    };
    #[cfg(not(unix))]
    let uid = None;
    let listener = Arc::new(broker.listen(directory.path(), uid).await.unwrap());
    let runtime = Arc::new(ManagedCodexRuntime {
        auth,
        controller,
        installation,
        layout: codex_home_layout(
            &shared.to_string_lossy(),
            &directory.path().join("private").to_string_lossy(),
            directory.path(),
            directory.path(),
        ),
        environment: Environment::from([
            ("ACCESS_TOKEN".into(), "inherited-token".into()),
            ("OPENAI_API_KEY".into(), "inherited-key".into()),
            ("OPENAI_BASE_URL".into(), "https://other.example".into()),
            ("T3CODE_CODEX_LAUNCH_ARGS".into(), "inherited-args".into()),
        ]),
        environment_id: "environment".into(),
        instance: "work".into(),
        helper_command: "server-helper".into(),
        helper_args: vec!["--fixture".into()],
        listener,
    });
    let provider = ManagedCodexProvider {
        runtime: runtime.clone(),
        spec: Arc::new(InstanceSpec {
            instance_id: "work".into(),
            driver: "codex".into(),
            display_name: Some("Work".into()),
            accent_color: None,
            enabled: true,
            config: json!({"setupMode":"managed"}),
            variables: vec![],
        }),
        context: NativeProviderContext {
            environment: Environment::new(),
            home: directory.path().into(),
            cwd: directory.path().into(),
            platform: Platform::current(),
            managed_mcp: false,
            server_version: "0.1.0".into(),
        },
        protocols: effects.clone(),
        executables: effects.clone(),
        clock: effects.clone(),
        manifest: manifest.into(),
        http: effects.clone(),
    };
    (directory, effects, provider)
}

#[tokio::test]
async fn managed_snapshot_isolates_auth_uses_account_catalog_and_excludes_unsupported_commands() {
    let (directory, effects, provider) = setup().await;
    let runtime = &provider.runtime;
    let shared = directory.path().join("shared");
    let snapshot = serde_json::to_value(provider.snapshot(None, true).await.unwrap()).unwrap();
    assert_eq!(snapshot["status"], "ready");
    assert_eq!(snapshot["auth"]["subscriptionSharing"], true);
    assert_eq!(snapshot["models"][0]["name"], "Account Luna");
    assert_eq!(snapshot["models"].as_array().unwrap().len(), 1);
    assert!(
        snapshot["slashCommands"]
            .as_array()
            .unwrap()
            .iter()
            .all(|command| command["name"] != "feedback")
    );
    assert_eq!(
        snapshot["usageLimits"]["unavailable"]["reason"],
        "unsupported"
    );
    let launches = effects.launches.lock().await;
    let launch = launches.last().unwrap();
    for key in [
        "ACCESS_TOKEN",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "T3CODE_CODEX_LAUNCH_ARGS",
    ] {
        assert!(!launch.environment.contains_key(key));
    }
    assert!(launch.launch_args.contains("openai_token_sharing"));
    assert!(!launch.launch_args.contains("never-log"));
    assert!(!format!("{launch:?}").contains("never-log"));
    assert_eq!(
        std::fs::read_to_string(shared.join("auth.json")).unwrap(),
        "native login"
    );
    let private = runtime.layout.effective_home.as_ref().unwrap();
    assert!(!private.join("auth.json").exists());
}

struct BlockedCatalog(Mutex<Option<tokio::sync::oneshot::Sender<()>>>);
#[async_trait]
impl HttpTransport for BlockedCatalog {
    async fn send(&self, _: HttpRequest) -> ServiceResult<HttpResponse> {
        self.0.lock().await.take().unwrap().send(()).unwrap();
        std::future::pending().await
    }
}

#[tokio::test]
async fn logout_interrupts_catalog_discovery_and_publishes_current_authentication() {
    let (_directory, _effects, mut provider) = setup().await;
    let (started, waiting) = tokio::sync::oneshot::channel();
    provider.http = Arc::new(BlockedCatalog(Mutex::new(Some(started))));
    let provider = Arc::new(provider);
    let task = tokio::spawn({
        let provider = provider.clone();
        async move { provider.snapshot(None, true).await }
    });
    waiting.await.unwrap();
    provider.runtime.controller.logout().await.unwrap();
    let snapshot = serde_json::to_value(task.await.unwrap().unwrap()).unwrap();
    assert_eq!(snapshot["status"], "warning");
    assert_eq!(snapshot["auth"]["status"], "unauthenticated");
    assert!(snapshot["auth"].get("subscriptionSharing").is_none());
    assert!(snapshot.get("usageLimits").is_none());
    assert!(snapshot["models"].as_array().unwrap().is_empty());
}
