use crate::{Error, Result, datetime, unix_millis};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::Path,
    sync::{Arc, RwLock},
    time::Duration,
};

pub const DEFAULT_INGEST_HOST: &str = "https://ingest.t3.codes";
pub const DEFAULT_POSTHOG_KEY: &str = "phc_XOWci4oZP4VvLiEyrFqkFjP4CZn55mjYYBMREK5Wd6m";

#[derive(Clone, Copy, Debug)]
pub struct Consent {
    pub enabled: bool,
    pub policy_allows: bool,
    pub shell_discovery_pending: bool,
    pub settings_readable: bool,
}
impl Consent {
    pub fn allows(self) -> bool {
        self.enabled
            && self.policy_allows
            && !self.shell_discovery_pending
            && self.settings_readable
    }
}

#[derive(Clone)]
pub struct AnalyticsConfig {
    pub host: String,
    pub api_key: String,
    pub enabled: bool,
    pub batch_size: usize,
    pub max_buffered_events: usize,
    pub platform: String,
    pub arch: String,
    pub wsl_distro: Option<String>,
    pub version: String,
    pub channel: String,
    pub mode: String,
}
impl AnalyticsConfig {
    pub fn from_environment(
        env: &BTreeMap<String, String>,
        version: String,
        channel: String,
        mode: String,
    ) -> Result<Self> {
        let integer = |key: &str, default: usize| -> Result<usize> {
            env.get(key).map_or(Ok(default), |v| {
                v.parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| Error::Configuration(format!("invalid {key}")))
            })
        };
        let enabled = env
            .get("T3CODE_TELEMETRY_ENABLED")
            .map(|v| {
                t3_config::config::parse_bool(v)
                    .ok_or_else(|| Error::Configuration("invalid T3CODE_TELEMETRY_ENABLED".into()))
            })
            .transpose()?
            .unwrap_or(true);
        Ok(Self {
            host: env
                .get("T3CODE_POSTHOG_HOST")
                .cloned()
                .unwrap_or(DEFAULT_INGEST_HOST.into()),
            api_key: env
                .get("T3CODE_POSTHOG_KEY")
                .cloned()
                .unwrap_or(DEFAULT_POSTHOG_KEY.into()),
            enabled,
            batch_size: integer("T3CODE_TELEMETRY_FLUSH_BATCH_SIZE", 20)?,
            max_buffered_events: integer("T3CODE_TELEMETRY_MAX_BUFFERED_EVENTS", 1000)?,
            platform: match std::env::consts::OS {
                "windows" => "win32",
                "macos" => "darwin",
                other => other,
            }
            .into(),
            arch: match std::env::consts::ARCH {
                "x86_64" => "x64",
                "aarch64" => "arm64",
                other => other,
            }
            .into(),
            wsl_distro: env.get("WSL_DISTRO_NAME").cloned(),
            version,
            channel,
            mode,
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Identity {
    pub distinct_id: Option<String>,
    pub install_id: Option<String>,
}
pub struct IdentityPaths {
    pub anonymous_id: std::path::PathBuf,
    pub codex_auth: std::path::PathBuf,
    pub claude_account: std::path::PathBuf,
}
async fn read_identity_document(path: &Path) -> Option<Value> {
    let bytes = tokio::fs::read(path).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}
pub async fn identity_for_paths(paths: &IdentityPaths) -> Identity {
    let (codex, claude) = tokio::join!(
        read_identity_document(&paths.codex_auth),
        read_identity_document(&paths.claude_account)
    );
    resolve_identity(&paths.anonymous_id, codex.as_ref(), claude.as_ref()).await
}
pub async fn owners_for_paths(
    paths: &IdentityPaths,
    connect_user_id: Option<&str>,
) -> Vec<TelemetryOwner> {
    let (codex, claude) = tokio::join!(
        read_identity_document(&paths.codex_auth),
        read_identity_document(&paths.claude_account)
    );
    server_owners(connect_user_id, codex.as_ref(), claude.as_ref())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryOwner {
    pub source: OwnerSource,
    pub owner_id: String,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum OwnerSource {
    Connect,
    Openai,
    Claude,
    Device,
}
pub fn hash_telemetry_id(raw: &str) -> String {
    format!("{:x}", Sha256::digest(raw.as_bytes()))
}
pub fn is_telemetry_hash(raw: &str) -> bool {
    raw.len() == 64
        && raw
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn openai_user_id(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let claims: Value = serde_json::from_slice(&decoded).ok()?;
    claims["https://api.openai.com/auth"]["user_id"]
        .as_str()
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

pub async fn resolve_identity(
    anonymous_id_path: &Path,
    codex_auth: Option<&Value>,
    claude: Option<&Value>,
) -> Identity {
    let anonymous = match tokio::fs::read_to_string(anonymous_id_path).await {
        Ok(value) => Some(value),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let value = uuid::Uuid::new_v4().to_string();
            let result = async {
                if let Some(parent) = anonymous_id_path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                use tokio::io::AsyncWriteExt;
                let temporary =
                    anonymous_id_path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
                let result = async {
                    let mut file = tokio::fs::OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(&temporary)
                        .await?;
                    file.write_all(value.as_bytes()).await?;
                    file.flush().await?;
                    drop(file);
                    match tokio::fs::hard_link(&temporary, anonymous_id_path).await {
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                        other => other,
                    }
                }
                .await;
                let _ = tokio::fs::remove_file(temporary).await;
                result?;
                tokio::fs::read_to_string(anonymous_id_path).await
            }
            .await;
            result.ok()
        }
        Err(_) => None,
    };
    let install_id = anonymous.as_deref().map(hash_telemetry_id);
    let provider = codex_auth
        .and_then(|a| a.get("tokens"))
        .and_then(|t| t.get("account_id"))
        .and_then(Value::as_str)
        .or_else(|| claude.and_then(|c| c.get("userID")).and_then(Value::as_str));
    Identity {
        distinct_id: provider
            .map(hash_telemetry_id)
            .or_else(|| install_id.clone()),
        install_id,
    }
}

pub fn server_owners(
    connect_user_id: Option<&str>,
    codex: Option<&Value>,
    claude: Option<&Value>,
) -> Vec<TelemetryOwner> {
    let openai = codex
        .and_then(|c| c["tokens"]["id_token"].as_str())
        .and_then(openai_user_id);
    [
        (OwnerSource::Connect, connect_user_id),
        (OwnerSource::Openai, openai.as_deref()),
        (
            OwnerSource::Claude,
            claude.and_then(|c| c["oauthAccount"]["accountUuid"].as_str()),
        ),
    ]
    .into_iter()
    .filter_map(|(source, raw)| {
        raw.filter(|s| !s.trim().is_empty())
            .map(|id| TelemetryOwner {
                source,
                owner_id: hash_telemetry_id(id),
            })
    })
    .collect()
}

#[derive(Clone, Serialize)]
struct Event {
    uuid: String,
    event: String,
    properties: BTreeMap<String, Value>,
    timestamp: String,
}
#[derive(Default)]
struct Delivery {
    buffer: VecDeque<Event>,
    failed: Vec<Event>,
    attempts: u32,
    first_failed_at: u64,
    failures: u32,
    retry_at: u64,
    owners: BTreeMap<String, BTreeSet<String>>,
}
pub struct AnalyticsService {
    config: AnalyticsConfig,
    identity: Identity,
    consent: Arc<RwLock<Option<Consent>>>,
    client: reqwest::Client,
    delivery: tokio::sync::Mutex<Delivery>,
    flush_lock: tokio::sync::Mutex<()>,
}
pub struct AnalyticsWorker {
    cancel: tokio_util::sync::CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for AnalyticsWorker {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl AnalyticsWorker {
    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

pub fn retry_delay_ms(failures: u32, random: f64) -> u64 {
    let ceiling = (2000u64.saturating_mul(
        1u64.checked_shl(failures.saturating_sub(1))
            .unwrap_or(u64::MAX),
    ))
    .min(300_000);
    (ceiling as f64 / 2.0 * (1.0 + random.clamp(0.0, 1.0))).round() as u64
}

impl AnalyticsService {
    pub fn new(
        config: AnalyticsConfig,
        identity: Identity,
        consent: Arc<RwLock<Option<Consent>>>,
    ) -> Result<Arc<Self>> {
        if config.batch_size == 0 || config.max_buffered_events == 0 {
            return Err(Error::Configuration(
                "analytics buffer limits must be positive".into(),
            ));
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| Error::Export)?;
        Ok(Arc::new(Self {
            config,
            identity,
            consent,
            client,
            delivery: tokio::sync::Mutex::new(Delivery::default()),
            flush_lock: tokio::sync::Mutex::new(()),
        }))
    }
    fn enabled(&self) -> bool {
        self.config.enabled
            && self.identity.distinct_id.is_some()
            && self
                .consent
                .read()
                .ok()
                .and_then(|c| *c)
                .is_some_and(Consent::allows)
    }
    pub async fn record(&self, event: &str, properties: BTreeMap<String, Value>) {
        self.record_at(event, properties, unix_millis()).await;
    }
    pub async fn record_at(&self, event: &str, properties: BTreeMap<String, Value>, now_ms: u64) {
        if !self.enabled() {
            return;
        }
        let mut properties = properties;
        properties.retain(|key, value| {
            !crate::privacy::private_field(key) || is_usage_counter(key) && value.is_number()
        });
        for (key, value) in &mut properties {
            if !is_usage_counter(key) {
                crate::privacy::sanitize(value);
            }
        }
        let event = Event {
            uuid: uuid::Uuid::now_v7().to_string(),
            event: event.into(),
            properties,
            timestamp: datetime(now_ms).0,
        };
        let mut delivery = self.delivery.lock().await;
        delivery.buffer.push_back(event);
        while delivery.buffer.len() > self.config.max_buffered_events {
            delivery.buffer.pop_front();
        }
    }
    pub async fn record_owners(&self, owners: &[TelemetryOwner], observed_by: &str) {
        if !["server", "client"].contains(&observed_by) {
            return;
        }
        for owner in owners {
            if !is_telemetry_hash(&owner.owner_id) {
                continue;
            }
            let source = serde_json::to_value(owner.source)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            let key = format!("{source}:{}", owner.owner_id);
            let mut delivery = self.delivery.lock().await;
            let seen = delivery.owners.entry(observed_by.into()).or_default();
            if seen.len() >= 1000 || !seen.insert(key) {
                continue;
            }
            drop(delivery);
            self.record(
                "install.owner_observed",
                [
                    ("ownerSource".into(), json!(source)),
                    ("ownerId".into(), json!(owner.owner_id)),
                    ("observedBy".into(), json!(observed_by)),
                ]
                .into_iter()
                .collect(),
            )
            .await;
        }
    }
    pub async fn buffered_count(&self) -> usize {
        let delivery = self.delivery.lock().await;
        delivery.buffer.len() + delivery.failed.len()
    }
    pub async fn flush(&self) -> Result<()> {
        self.flush_at(unix_millis(), false).await
    }
    pub async fn flush_at(&self, now_ms: u64, only_when_due: bool) -> Result<()> {
        let _flush = self.flush_lock.lock().await;
        if !self.enabled() {
            let mut delivery = self.delivery.lock().await;
            delivery.buffer.clear();
            delivery.failed.clear();
            return Ok(());
        }
        loop {
            if !self.enabled() {
                return Ok(());
            }
            let batch = {
                let mut delivery = self.delivery.lock().await;
                if only_when_due && now_ms < delivery.retry_at {
                    return Ok(());
                }
                if !delivery.failed.is_empty() {
                    std::mem::take(&mut delivery.failed)
                } else {
                    let count = self.config.batch_size.min(delivery.buffer.len());
                    delivery.buffer.drain(..count).collect()
                }
            };
            if batch.is_empty() {
                return Ok(());
            }
            let client_type = if self.config.mode == "desktop" {
                "desktop-app"
            } else {
                "cli-web-client"
            };
            let server_os = match self.config.platform.as_str() {
                "win32" => "Windows",
                "darwin" => "macOS",
                "linux" => "Linux",
                "android" => "Android",
                _ => "other",
            };
            let payload = json!({"api_key":self.config.api_key,"batch":batch.iter().map(|event|{
                let mut properties=event.properties.clone();
                properties.extend([("$process_person_profile".into(),json!(false)),("platform".into(),json!(self.config.platform)),("arch".into(),json!(self.config.arch)),("t3CodeVersion".into(),json!(self.config.version)),("telemetryChannel".into(),json!(self.config.channel)),("clientType".into(),json!(client_type)),("serverOs".into(),json!(server_os)),("serverArch".into(),json!(self.config.arch)),("serverAppVersion".into(),json!(self.config.version)),("serverMode".into(),json!(self.config.mode))]);
                if let Some(install)=&self.identity.install_id{properties.insert("installId".into(),json!(install));}
                if let Some(wsl)=&self.config.wsl_distro{properties.insert("wsl".into(),json!(wsl));properties.insert("serverWslDistro".into(),json!(wsl));}
                json!({"uuid":event.uuid,"event":event.event,"distinct_id":self.identity.distinct_id,"properties":properties,"timestamp":event.timestamp})
            }).collect::<Vec<_>>()});
            if !self.enabled() {
                return Ok(());
            }
            let result = self
                .client
                .post(format!("{}/batch/", self.config.host.trim_end_matches('/')))
                .json(&payload)
                .send()
                .await;
            let mut delivery = self.delivery.lock().await;
            match result {
                Ok(response) if response.status().is_success() => {
                    delivery.attempts = 0;
                    delivery.failures = 0;
                    delivery.first_failed_at = 0;
                    delivery.retry_at = 0;
                }
                response => {
                    let rejected = response
                        .as_ref()
                        .is_ok_and(|r| [400, 401, 413].contains(&r.status().as_u16()));
                    delivery.failures = delivery.failures.saturating_add(1);
                    delivery.attempts = delivery.attempts.saturating_add(1);
                    if delivery.attempts == 1 {
                        delivery.first_failed_at = now_ms;
                    }
                    let random = u64::from_be_bytes(
                        uuid::Uuid::new_v4().as_bytes()[..8]
                            .try_into()
                            .expect("eight bytes"),
                    ) as f64
                        / u64::MAX as f64;
                    delivery.retry_at =
                        now_ms.saturating_add(retry_delay_ms(delivery.failures, random));
                    if !rejected && now_ms.saturating_sub(delivery.first_failed_at) < 30 * 60_000 {
                        delivery.failed = batch;
                    } else {
                        delivery.attempts = 0;
                        delivery.first_failed_at = 0;
                    }
                    return Err(Error::Export);
                }
            }
        }
    }
    pub fn start(self: &Arc<Self>) -> AnalyticsWorker {
        let cancel = tokio_util::sync::CancellationToken::new();
        let token = cancel.clone();
        let weak = Arc::downgrade(self);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {_=token.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(10))=>{}}
                let Some(service) = weak.upgrade() else {
                    return;
                };
                let _ = service.flush_at(unix_millis(), true).await;
            }
            if let Some(service) = weak.upgrade() {
                let _ = tokio::time::timeout(Duration::from_secs(1), service.flush()).await;
            }
        });
        AnalyticsWorker {
            cancel,
            task: Some(task),
        }
    }
    pub async fn shutdown(&self) {
        let _ = tokio::time::timeout(Duration::from_secs(1), self.flush()).await;
    }
}
fn is_usage_counter(key: &str) -> bool {
    matches!(
        key,
        "inputTokens"
            | "outputTokens"
            | "cachedInputTokens"
            | "cacheCreationTokens"
            | "reasoningTokens"
            | "totalTokens"
            | "otherInputTokens"
            | "otherOutputTokens"
            | "childInputTokens"
            | "childOutputTokens"
    )
}
