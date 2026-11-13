use crate::{
    environment::{Environment, probe_environment, process_options},
    sensitive::{Sensitive, ServiceError, ServiceResult},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use tokio::{io::AsyncReadExt, time::Instant};

#[derive(Clone)]
pub struct ProbeLaunch {
    pub executable: String,
    pub environment: Environment,
    pub cwd: PathBuf,
    pub launch_args: String,
    pub managed_mcp: bool,
    pub include_status: bool,
    pub gateway_discovery_grace: Duration,
}
impl std::fmt::Debug for ProbeLaunch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProbeLaunch([REDACTED])")
    }
}
#[async_trait]
pub trait NativeProbe: Send + Sync {
    async fn codex(&self, launch: ProbeLaunch) -> ServiceResult<Sensitive<Value>>;
    async fn claude(&self, launch: ProbeLaunch) -> ServiceResult<Sensitive<Value>>;
    async fn consume_codex_reset(
        &self,
        _launch: ProbeLaunch,
        _key: String,
    ) -> ServiceResult<t3_contracts::provider_usage_limits::ProviderConsumeResetCreditOutcome> {
        Err(ServiceError::Unsupported)
    }
}
pub struct ProcessProbes {
    pub server_version: String,
}
struct Task(tokio::task::JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}
async fn spawn(options: t3_process::ProcessOptions) -> ServiceResult<t3_process::SpawnedProcess> {
    tokio::task::spawn_blocking(move || t3_process::spawn(&options))
        .await
        .map_err(|_| ServiceError::Response)?
        .map_err(|_| ServiceError::Response)
}
fn drain_stderr(process: &mut t3_process::SpawnedProcess) -> Option<Task> {
    process.stderr.take().map(|mut stderr| {
        Task(tokio::spawn(async move {
            let mut buffer = [0; 8192];
            while let Ok(read) = stderr.read(&mut buffer).await {
                if read == 0 {
                    break;
                }
            }
        }))
    })
}
async fn finish(process: &mut t3_process::SpawnedProcess) {
    if tokio::time::timeout(Duration::from_secs(2), process.wait())
        .await
        .is_err()
    {
        let _ = process.kill().await;
    }
}
fn remaining(deadline: Instant) -> Duration {
    deadline
        .saturating_duration_since(Instant::now())
        .saturating_sub(Duration::from_millis(250))
        .min(Duration::from_secs(3))
}
async fn codex_models(client: &t3_codex_app_server::Client) -> Option<Vec<Value>> {
    use t3_codex_app_server::wire::Params;
    let mut models = vec![];
    let mut cursor = None;
    let mut seen = std::collections::BTreeSet::new();
    loop {
        let params = cursor
            .as_ref()
            .map_or_else(|| json!({}), |c: &String| json!({"cursor":c}));
        let response = client
            .request_raw("model/list", Params::Present(params))
            .await
            .ok()?;
        models.extend(response.get("data")?.as_array()?.iter().cloned());
        let next = response
            .get("nextCursor")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty());
        match next {
            None => return Some(models),
            Some(next) if seen.insert(next.to_owned()) => cursor = Some(next.to_owned()),
            Some(_) => return None,
        }
    }
}
async fn wait_codex_catalog(home: &str, spawned_at: i64) {
    let path = std::path::Path::new(home).join("models_cache.json");
    loop {
        if let Ok(text) = tokio::fs::read_to_string(&path).await
            && let Ok(stamp) = serde_json::from_str::<Value>(&text)
            && let Some(stamp) = stamp["fetched_at"].as_str()
            && chrono::DateTime::parse_from_rfc3339(stamp)
                .is_ok_and(|stamp| stamp.timestamp_millis() >= spawned_at)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
#[async_trait]
impl NativeProbe for ProcessProbes {
    async fn consume_codex_reset(
        &self,
        launch: ProbeLaunch,
        key: String,
    ) -> ServiceResult<t3_contracts::provider_usage_limits::ProviderConsumeResetCreditOutcome> {
        use t3_codex_app_server::{Client, ClientOptions, InitializeParams, wire::Params};
        let mut env = probe_environment(&launch.environment);
        env.entry("CODEX_INTERNAL_ORIGINATOR_OVERRIDE".into())
            .or_insert_with(|| "T3 Code".into());
        let configured = env
            .get("T3CODE_CODEX_LAUNCH_ARGS")
            .filter(|s| !s.trim().is_empty())
            .map_or(launch.launch_args.as_str(), String::as_str);
        let mut args = vec![
            "app-server".into(),
            "-c".into(),
            "thread_unload_delay_secs=0".into(),
        ];
        args.extend(t3_codex_app_server::launch::tokenize_cli_args(configured));
        let mut options = process_options(&launch.executable, &args, &env);
        options.cwd = Some(launch.cwd);
        let mut process = spawn(options).await?;
        let _stderr = drain_stderr(&mut process);
        let stdout = process.stdout.take().ok_or(ServiceError::Response)?;
        let stdin = process.stdin.take().ok_or(ServiceError::Response)?;
        let (client, mut events) = Client::connect(stdout, stdin, ClientOptions::default());
        let _events = Task(tokio::spawn(async move {
            while events.recv().await.is_some() {}
        }));
        let work = async {
            client
                .initialize(&InitializeParams::probe(&self.server_version))
                .await
                .map_err(|_| ServiceError::Response)?;
            client
                .initialized()
                .await
                .map_err(|_| ServiceError::Response)?;
            let response = client
                .request_raw(
                    "account/rateLimitResetCredit/consume",
                    Params::Present(json!({"idempotencyKey":key})),
                )
                .await
                .map_err(|_| ServiceError::Response)?;
            serde_json::from_value(response["outcome"].clone()).map_err(|_| ServiceError::Response)
        };
        let result = tokio::time::timeout(Duration::from_secs(20), work)
            .await
            .map_err(|_| ServiceError::Response)
            .and_then(|r| r);
        let _ = tokio::time::timeout(Duration::from_secs(2), client.shutdown()).await;
        finish(&mut process).await;
        result
    }
    async fn codex(&self, launch: ProbeLaunch) -> ServiceResult<Sensitive<Value>> {
        use t3_codex_app_server::{Client, ClientOptions, InitializeParams, wire::Params};
        let mut env = probe_environment(&launch.environment);
        env.entry("CODEX_INTERNAL_ORIGINATOR_OVERRIDE".into())
            .or_insert_with(|| "T3 Code".into());
        let configured = env
            .get("T3CODE_CODEX_LAUNCH_ARGS")
            .filter(|s| !s.trim().is_empty())
            .map_or(launch.launch_args.as_str(), String::as_str);
        let mut args = vec![
            "app-server".into(),
            "-c".into(),
            "thread_unload_delay_secs=0".into(),
        ];
        args.extend(t3_codex_app_server::launch::tokenize_cli_args(configured));
        let mut options = process_options(&launch.executable, &args, &env);
        options.cwd = Some(launch.cwd.clone());
        let timeout =
            Duration::from_millis(crate::versions::probe_timeout_ms("codex", 20000, &env));
        let deadline = Instant::now() + timeout;
        let spawned_at = crate::transport::ProviderClock::now_ms(&crate::transport::SystemClock);
        let mut process = spawn(options).await?;
        let _stderr = drain_stderr(&mut process);
        let stdout = process.stdout.take().ok_or(ServiceError::Response)?;
        let stdin = process.stdin.take().ok_or(ServiceError::Response)?;
        let (client, mut events) = Client::connect(stdout, stdin, ClientOptions::default());
        let _events = Task(tokio::spawn(async move {
            while events.recv().await.is_some() {}
        }));
        let work = async {
            let initialized = client
                .initialize(&InitializeParams::probe(&self.server_version))
                .await
                .map_err(|_| ServiceError::Response)?;
            client
                .initialized()
                .await
                .map_err(|_| ServiceError::Response)?;
            if !launch.include_status {
                let skills = client
                    .request_raw("skills/list", Params::Present(json!({"cwds":[launch.cwd]})))
                    .await
                    .map_err(|_| ServiceError::Response)?;
                return Ok(Sensitive::new(
                    json!({"initialize":initialized,"skills":skills}),
                ));
            }
            let account = client
                .request_raw("account/read", Params::Present(json!({})))
                .await
                .map_err(|_| ServiceError::Response)?;
            let mut result = json!({"initialize":initialized,"account":account});
            if result["account"]["account"].is_null()
                && result["account"]["requiresOpenaiAuth"].as_bool() == Some(true)
            {
                result["models"] = json!([]);
                result["skills"] = json!({"data":[]});
                return Ok(Sensitive::new(result));
            }
            let budget = deadline
                .saturating_duration_since(Instant::now())
                .saturating_sub(Duration::from_millis(250));
            let models = async {
                let models = codex_models(&client).await?;
                if account["account"]["type"] == "chatgpt"
                    && let Some(home) = initialized
                        .as_ref()
                        .map(|response| response.codex_home.as_str())
                    && tokio::time::timeout(
                        remaining(deadline),
                        wait_codex_catalog(home, spawned_at),
                    )
                    .await
                    .is_ok()
                {
                    return codex_models(&client).await.or(Some(models));
                }
                Some(models)
            };
            let config = async {
                let config = tokio::time::timeout(
                    remaining(deadline),
                    client.request_raw(
                        "config/read",
                        Params::Present(json!({"includeLayers":false})),
                    ),
                )
                .await
                .ok()?
                .ok()?;
                let native = crate::probe_models::codex_native_quota(&account, &config["config"]);
                let limits = if native && launch.include_status {
                    client
                        .request_raw("account/rateLimits/read", Params::Present(Value::Null))
                        .await
                        .ok()
                } else {
                    None
                };
                Some((config, native, limits))
            };
            let skills =
                client.request_raw("skills/list", Params::Present(json!({"cwds":[launch.cwd]})));
            let (models, config, skills) = tokio::join!(
                tokio::time::timeout(budget, models),
                tokio::time::timeout(budget, config),
                tokio::time::timeout(budget, skills)
            );
            if let Ok(Some(models)) = models {
                result["models"] = json!(models);
            }
            if let Ok(Some((config, native, limits))) = config {
                result["config"] = config;
                result["nativeUsage"] = json!(native);
                if let Some(limits) = limits {
                    result["rateLimits"] = limits;
                }
            }
            if let Ok(Ok(skills)) = skills {
                result["skills"] = skills;
            }
            Ok(Sensitive::new(result))
        };
        let result = tokio::time::timeout_at(deadline, work)
            .await
            .map_err(|_| ServiceError::Response)
            .and_then(|result| result);
        let _ = tokio::time::timeout(Duration::from_secs(2), client.shutdown()).await;
        finish(&mut process).await;
        result
    }
    async fn claude(&self, launch: ProbeLaunch) -> ServiceResult<Sensitive<Value>> {
        use t3_claude_cli::{
            Session, SessionOptions,
            control::OutboundRequest,
            launch::{LaunchOptions, Settings, describe_spawn},
        };
        let mut env = probe_environment(&launch.environment);
        env.insert("ENABLE_CLAUDEAI_MCP_SERVERS".into(), "false".into());
        env.remove("FORCE_CODE_TERMINAL");
        env.insert("CLAUDE_CODE_AUTO_CONNECT_IDE".into(), "0".into());
        env.insert("CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL".into(), "1".into());
        let executable = crate::discovery::resolve_claude_sdk_executable(&launch.executable, &env);
        let options = LaunchOptions {
            cli_path: Some(executable),
            cwd: Some(launch.cwd),
            env,
            setting_sources: Some(vec!["user".into(), "project".into(), "local".into()]),
            persist_session: false,
            settings: Some(Settings::Json(json!({"disableAllHooks":true}))),
            strict_mcp_config: !launch.managed_mcp,
            ..Default::default()
        };
        let description = describe_spawn(&options).map_err(|_| ServiceError::Response)?;
        let mut process_options =
            process_options(description.command, &description.args, &description.env);
        process_options.cwd = description.cwd;
        let mut process = spawn(process_options).await?;
        let _stderr = drain_stderr(&mut process);
        let stdout = process.stdout.take().ok_or(ServiceError::Response)?;
        let stdin = process.stdin.take().ok_or(ServiceError::Response)?;
        let mut session = Session::connect(stdout, stdin, SessionOptions::default());
        let client = session.client();
        let _events = Task(tokio::spawn(async move {
            while session.next_event().await.is_some() {}
        }));
        let initialized = tokio::time::timeout(
            Duration::from_millis(crate::versions::probe_timeout_ms(
                "claude",
                25000,
                &launch.environment,
            )),
            client.initialize(description.initialize),
        )
        .await
        .map_err(|_| ServiceError::Response)?
        .map_err(|_| ServiceError::Response)?;
        let mut result = json!({"initialize":initialized.raw()});
        if launch.include_status {
            let (usage, settings) = tokio::join!(
                tokio::time::timeout(
                    Duration::from_secs(10),
                    client.request(OutboundRequest::GetUsage {
                        skip_behaviors: true
                    })
                ),
                tokio::time::timeout(
                    Duration::from_secs(10),
                    client.request(OutboundRequest::GetSettings)
                )
            );
            if let Ok(Ok(usage)) = usage {
                result["usage"] = usage.payload.unwrap_or(Value::Null);
            }
            if let Ok(Ok(settings)) = settings {
                result["settings"] = settings.payload.unwrap_or(Value::Null);
            }
        }
        if !launch.gateway_discovery_grace.is_zero() {
            tokio::spawn(async move {
                let _stderr = _stderr;
                let _events = _events;
                tokio::time::sleep(launch.gateway_discovery_grace).await;
                let _ = tokio::time::timeout(Duration::from_secs(2), client.end_input()).await;
                finish(&mut process).await;
            });
            return Ok(Sensitive::new(result));
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), client.end_input()).await;
        finish(&mut process).await;
        Ok(Sensitive::new(result))
    }
}
pub fn claude_gateway_discovery_grace(
    environment: &std::collections::BTreeMap<String, Value>,
) -> Duration {
    let string = |key: &str| {
        environment
            .get(key)
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| v.to_string())
            })
            .unwrap_or_default()
    };
    let enabled = string("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY")
        .trim()
        .to_lowercase();
    if matches!(enabled.as_str(), "" | "0" | "false")
        || string("ANTHROPIC_BASE_URL").trim().is_empty()
    {
        return Duration::ZERO;
    }
    let timeout = string("CLAUDE_CODE_GATEWAY_MODEL_DISCOVERY_TIMEOUT_MS")
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && *n > 0.)
        .unwrap_or(3000.);
    Duration::from_secs_f64(timeout.min(i32::MAX as f64) / 1000.)
}
