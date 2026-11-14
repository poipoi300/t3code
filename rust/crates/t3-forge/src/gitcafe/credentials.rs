use crate::{
    FailureReason, ForgeError, Result,
    cli::*,
    http::{ApiCredential, ApiCredentials},
};
use async_trait::async_trait;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::Mutex;
#[derive(Clone, Default)]
pub struct GitCafeSettings {
    pub token: Option<String>,
    pub environment: t3_process::Environment,
}
type Cache = HashMap<String, (tokio::time::Instant, Result<ApiCredential>)>;
pub struct GitCafeCredentials {
    cwd: PathBuf,
    settings: RwLock<GitCafeSettings>,
    cli: Arc<dyn ForgeCli>,
    cache: Mutex<Cache>,
    revision: AtomicU64,
    stop: t3_process::CancellationToken,
}
impl GitCafeCredentials {
    pub fn new(cwd: PathBuf, settings: GitCafeSettings) -> Self {
        Self {
            cwd,
            settings: RwLock::new(settings),
            cli: Arc::new(ProcessForgeCli),
            cache: Default::default(),
            revision: AtomicU64::new(0),
            stop: t3_process::CancellationToken::new(),
        }
    }
    pub fn with_cli(mut self, cli: Arc<dyn ForgeCli>) -> Self {
        self.cli = cli;
        self
    }
    pub async fn update_settings(&self, settings: GitCafeSettings) {
        let mut cache = self.cache.lock().await;
        *self.settings.write().unwrap_or_else(|e| e.into_inner()) = settings;
        cache.clear();
        self.revision.fetch_add(1, Ordering::Release);
    }
    pub fn shutdown(&self) {
        self.stop.cancel();
    }
    pub fn cli_environment(&self, host: &str) -> t3_process::Environment {
        let settings = self.settings.read().unwrap_or_else(|e| e.into_inner());
        let mut env = settings.environment.clone();
        if host == "git.cafe"
            && let Some(token) = settings.token.as_ref().filter(|s| !s.is_empty())
        {
            env.insert("CAFE_TOKEN".into(), Some(token.into()));
        }
        env.insert("CAFE_NO_UPDATE_CHECK".into(), Some("1".into()));
        env
    }
}
fn error(reason: FailureReason) -> ForgeError {
    ForgeError::provider(
        "gitcafe",
        "credential",
        reason,
        "No GitCafe credential is available. Save a token, set CAFE_TOKEN, or run `cafe auth login`.",
    )
}
pub fn environment_token(host: &str, environment: &t3_process::Environment) -> Option<String> {
    let read = |key: &str| {
        environment
            .get(std::ffi::OsStr::new(key))
            .and_then(|v| v.as_ref())
            .map(|v| v.to_string_lossy().trim().to_string())
    };
    let configured = read("CAFE_HOST").filter(|s| !s.is_empty());
    let base = configured
        .map(|s| {
            if s.contains("://") {
                s
            } else {
                format!("https://{s}")
            }
        })
        .unwrap_or("https://git.cafe".into());
    let url = url::Url::parse(&base).ok()?;
    if !url.authority().eq_ignore_ascii_case(host.trim()) {
        return None;
    }
    read("CAFE_TOKEN").filter(|s| !s.is_empty())
}
#[async_trait]
impl ApiCredentials for GitCafeCredentials {
    fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }
    async fn invalidate(&self, host: &str) {
        self.cache.lock().await.remove(&host.trim().to_lowercase());
    }
    async fn get(&self, host: &str) -> Result<ApiCredential> {
        let host = host.trim().to_lowercase();
        if !super::valid_host(&host) {
            return Err(error(FailureReason::Failed));
        }
        let mut cache = self.cache.lock().await;
        let settings = self
            .settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if host == "git.cafe"
            && let Some(saved) = settings.token.as_ref().filter(|s| !s.is_empty())
        {
            return Ok(ApiCredential::Bearer(saved.clone()));
        }
        if let Some((at, result)) = cache.get(&host)
            && at.elapsed() < Duration::from_secs(if result.is_ok() { 300 } else { 10 })
        {
            return result.clone();
        }
        let result = if let Some(token) = environment_token(&host, &settings.environment) {
            Ok(ApiCredential::Bearer(token))
        } else {
            let mut environment = settings.environment;
            environment.insert("CAFE_NO_UPDATE_CHECK".into(), Some("1".into()));
            let output = self
                .cli
                .run(CliRequest {
                    program: "cafe".into(),
                    provider: "gitcafe",
                    operation: "credential".into(),
                    cwd: self.cwd.clone(),
                    args: vec![
                        "auth".into(),
                        "http".into(),
                        "credential".into(),
                        format!("--host=https://{host}/api"),
                        "get".into(),
                    ],
                    stdin: Some(format!("protocol=https\nhost={host}\n\n").into_bytes()),
                    environment,
                    max_bytes: 16_000,
                    timeout: Duration::from_secs(10),
                    cancellation: self.stop.clone(),
                })
                .await;
            match output {
                Ok(output) if output.code == 0 && !output.truncated => {
                    String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .find_map(|s| {
                            s.strip_prefix("password=")
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                        })
                        .map(|s| ApiCredential::Bearer(s.into()))
                        .ok_or_else(|| error(FailureReason::Unauthenticated))
                }
                Ok(_) => Err(error(FailureReason::Unauthenticated)),
                Err(e) => Err(error(if e.reason() == Some(FailureReason::MissingTool) {
                    FailureReason::MissingTool
                } else {
                    FailureReason::Failed
                })),
            }
        };
        if !matches!(&result,Err(e)if e.reason()==Some(FailureReason::Failed)) {
            if cache.len() >= 4 {
                cache.clear();
            }
            cache.insert(host, (tokio::time::Instant::now(), result.clone()));
        }
        result
    }
}
