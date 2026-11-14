#![allow(clippy::unwrap_used)]
mod common;
use async_trait::async_trait;
use serde_json::json;
use std::{path::Path, sync::Arc};
use t3_provider::{
    antigravity_installation::*, antigravity_profile::*, antigravity_release::Release,
    antigravity_runtime::*, catalog::Manifest, environment::Platform, sensitive::*,
    transport::SystemClock,
};
struct NoDownload;
#[async_trait]
impl AntigravityReleases for NoDownload {
    async fn latest(&self, _: &str) -> ServiceResult<Option<Release>> {
        panic!("read-only discovery must not download")
    }
    async fn download(
        &self,
        _: &Release,
        _: &Path,
        _: tokio::sync::watch::Sender<u64>,
    ) -> ServiceResult<()> {
        panic!("read-only discovery must not download")
    }
}
struct NoValidate;
#[async_trait]
impl AntigravityValidator for NoValidate {
    async fn validate(&self, _: &AntigravityExecutable, _: &str) -> ServiceResult<()> {
        panic!("external discovery must not install")
    }
}
#[tokio::test]
async fn native_setup_probes_initialize_auth_and_an_empty_session_without_any_provider_turn() {
    let one = json!([
        {"id":"model","type":"select","currentValue":"one","options":[{"value":"one","name":"One"},{"value":"two","name":"Two"}]},
        {"id":"thinking","category":"thought_level","type":"select","currentValue":"high","options":[{"value":"high","name":"High"}]}
    ]);
    let mut two = one.clone();
    two[0]["currentValue"] = json!("two");
    two.as_array_mut().unwrap().pop();
    let fixture = common::Fixture::new(
        json!({"protocol":"acp","initialize":{"protocolVersion":1,"agentInfo":{"name":"fixture","version":"1.5.3"},"agentCapabilities":{"auth":{"logout":{}}},"authMethods":[{"id":"oauth-personal","name":"Google"}]},"expectedAuthMethod":"oauth-personal", "setup":{"sessionId":"fixture-session","configOptions":one}, "modelConfigs":{"one":one,"two":two}}),
    );
    let home = fixture.directory.path();
    let executable = home.join(if cfg!(windows) { "agy.exe" } else { "agy" });
    std::fs::copy(fixture.binary(), &executable).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let auth = AuthConfig::from_settings(&json!({}));
    let profile = Arc::new(
        prepare(PrepareProfile {
            directory: &home.join("profile"),
            temp_directory: None,
            user_home: home,
            browser_helper: Path::new(env!("CARGO_BIN_EXE_antigravity-browser-helper")),
            environment: &fixture.environment,
            platform: Platform::current(),
            auth: &auth,
        })
        .await
        .unwrap(),
    );
    let installation = AntigravityInstallation::new(
        InstallationOptions {
            base: home.into(),
            platform: Platform::current(),
            architecture: "x64".into(),
            musl: false,
            environment: fixture.environment.clone(),
            manifest: Arc::new(Manifest::bundled().unwrap()).into(),
            server_version: "1".into(),
            pin: None,
            update_checks: false,
        },
        Arc::new(NoDownload),
        Arc::new(NoValidate),
        Arc::new(SystemClock),
    );
    let factory = ProcessAntigravityFactory {
        installation,
        binary: Some(executable.to_string_lossy().into_owned()),
        profile: profile.clone(),
        auth,
        environment: fixture.environment.clone(),
    };
    let before = fixture.calls().len();
    factory.health().await.unwrap();
    assert_eq!(
        fixture.calls().len(),
        before,
        "periodic health checks must not unpack the runtime"
    );
    let runtime = factory.create(None).await.unwrap();
    assert_eq!(
        runtime.initialize().await.unwrap().expose()["agentInfo"]["version"],
        "1.5.3"
    );
    let started = runtime.start().await.unwrap();
    assert_eq!(started.setup.expose()["sessionId"], "fixture-session");
    let catalog = runtime.catalog(&started).await.unwrap();
    assert_eq!(catalog.expose().config_updates.len(), 3);
    assert_eq!(
        catalog.expose().config_updates[1].expose()[0]["currentValue"],
        "two"
    );
    assert_eq!(catalog.expose().config_updates[1].expose().len(), 1);
    assert_eq!(
        catalog.expose().config_updates[2].expose()[0]["currentValue"],
        "one"
    );
    assert_eq!(catalog.expose().commands[0]["name"], "review");
    runtime.logout().await.unwrap();
    runtime.close().await;
    let calls = fixture.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|args| args == &&vec!["initialize".to_owned()])
            .count(),
        1
    );
    assert!(calls.iter().any(|args| args == &["authenticate"]));
    assert!(calls.iter().any(|args| args == &["session/new"]));
    assert!(calls.iter().any(|args| args == &["logout"]));
    assert!(calls.iter().all(|args| {
        !args
            .iter()
            .any(|arg| matches!(arg.as_str(), "session/prompt" | "prompt"))
    }));
    assert!(factory.supports_text_generation().await);
    std::fs::create_dir_all(profile.gemini_home.join("config")).unwrap();
    std::fs::write(
        profile.gemini_home.join("config/hooks.json"),
        "{\"hooks\":{\"before\":[\"blocked\"]}}",
    )
    .unwrap();
    assert!(!factory.supports_text_generation().await);
}
