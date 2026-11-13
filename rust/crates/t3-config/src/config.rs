use crate::{
    Result,
    home::{HomeLayout, resolve_path},
    invalid,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};
use t3_contracts::desktop_bootstrap::DesktopBackendBootstrap;

pub type Environment = BTreeMap<String, String>;
pub const DEFAULT_PORT: u16 = 3773;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeMode {
    #[default]
    Web,
    Desktop,
}
#[derive(Debug, Clone, Default)]
pub struct ServerFlags {
    pub mode: Option<RuntimeMode>,
    pub port: Option<u16>,
    pub host: Option<String>,
    pub base_dir: Option<String>,
    pub cwd: Option<String>,
    pub dev_url: Option<String>,
    pub no_browser: Option<bool>,
    pub bootstrap_fd: Option<i64>,
    pub auto_bootstrap_project_from_cwd: Option<bool>,
    pub log_websocket_events: Option<bool>,
    pub tailscale_serve_enabled: Option<bool>,
    pub tailscale_serve_port: Option<u16>,
    pub lan: Option<bool>,
}
#[derive(Clone)]
pub struct ConfigInputs {
    pub flags: ServerFlags,
    pub env: Environment,
    pub bootstrap: Option<DesktopBackendBootstrap>,
    pub home: PathBuf,
    pub process_cwd: PathBuf,
    pub headless: bool,
    pub force_auto_bootstrap: Option<bool>,
    pub service_managed: bool,
    pub cli_log_level: Option<String>,
    /// Result of the caller's OS port selection. No socket I/O occurs in resolution.
    pub automatic_port: Option<u16>,
    pub persisted_observability: BTreeMap<String, String>,
    pub static_dir: Option<PathBuf>,
}
impl std::fmt::Debug for ConfigInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigInputs").finish_non_exhaustive()
    }
}
#[derive(Clone)]
pub struct ResolvedConfig {
    pub mode: RuntimeMode,
    pub port: u16,
    pub needs_automatic_port: bool,
    pub host: Option<String>,
    pub private_network_only: bool,
    pub cwd: PathBuf,
    pub paths: HomeLayout,
    pub dev_url: Option<url::Url>,
    dev_auth_token: Option<String>,
    pub dev_allowed_origins: Vec<String>,
    pub no_browser: bool,
    pub headless: bool,
    pub bootstrap_fd: Option<i64>,
    pub bootstrap: Option<DesktopBackendBootstrap>,
    pub auto_bootstrap_project_from_cwd: bool,
    pub log_websocket_events: bool,
    pub tailscale_serve_enabled: bool,
    pub tailscale_serve_port: u16,
    pub log_level: String,
    pub trace_min_level: String,
    pub trace_timing_enabled: bool,
    pub trace_max_bytes: i64,
    pub trace_max_files: i64,
    pub trace_batch_window_ms: i64,
    pub server_trace_path: PathBuf,
    pub static_dir: Option<PathBuf>,
    pub telemetry: TelemetryConfig,
}
impl ResolvedConfig {
    pub fn dev_auth_token(&self) -> Option<&str> {
        self.dev_auth_token.as_deref()
    }
    pub fn ensure_directories(&self) -> Result<()> {
        std::fs::create_dir_all(&self.cwd)
            .map_err(|e| crate::files::io("create cwd", &self.cwd, e))?;
        self.paths.ensure_directories()?;
        self.paths
            .sweep_stale_pending_attachments(std::time::SystemTime::now());
        if let Some(parent) = self.server_trace_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| crate::files::io("create trace directory", parent, e))?;
        }
        Ok(())
    }
}
pub fn parse_bool(value: &str) -> Option<bool> {
    match value.to_lowercase().as_str() {
        "true" | "yes" | "on" | "1" | "y" => Some(true),
        "false" | "no" | "off" | "0" | "n" => Some(false),
        _ => None,
    }
}
fn bool_env(env: &Environment, key: &str) -> Result<Option<bool>> {
    env.get(key)
        .map(|s| parse_bool(s).ok_or_else(|| invalid(key, "expected a boolean")))
        .transpose()
}
fn number_env(env: &Environment, key: &str) -> Result<Option<i64>> {
    env.get(key)
        .map(|s| s.parse().map_err(|_| invalid(key, "expected an integer")))
        .transpose()
}
fn port_env(env: &Environment, key: &str) -> Result<Option<u16>> {
    number_env(env, key)?
        .map(|n| u16::try_from(n).map_err(|_| invalid(key, "expected port 0..65535")))
        .transpose()
}
fn level(env: &Environment, key: &str) -> Result<String> {
    let value = env.get(key).map(String::as_str).unwrap_or("Info");
    [
        "All", "Trace", "Debug", "Info", "Warning", "Error", "Fatal", "None",
    ]
    .into_iter()
    .find(|s| s.eq_ignore_ascii_case(value))
    .map(str::to_owned)
    .ok_or_else(|| invalid(key, "unknown log level"))
}
pub fn resolve_config(input: &ConfigInputs) -> Result<ResolvedConfig> {
    let ConfigInputs {
        flags: f,
        env: e,
        bootstrap: b,
        ..
    } = input;
    let env_mode = e
        .get("T3CODE_MODE")
        .map(|s| match s.as_str() {
            "web" => Ok(RuntimeMode::Web),
            "desktop" => Ok(RuntimeMode::Desktop),
            _ => Err(invalid("T3CODE_MODE", "expected web or desktop")),
        })
        .transpose()?;
    let env_port = port_env(e, "T3CODE_PORT")?;
    let env_tailscale_port = port_env(e, "T3CODE_TAILSCALE_SERVE_PORT")?;
    let env_no_browser = bool_env(e, "T3CODE_NO_BROWSER")?;
    let env_auto = bool_env(e, "T3CODE_AUTO_BOOTSTRAP_PROJECT_FROM_CWD")?;
    let env_log_ws = bool_env(e, "T3CODE_LOG_WS_EVENTS")?;
    let env_tailscale = bool_env(e, "T3CODE_TAILSCALE_SERVE")?;
    let env_lan = bool_env(e, "T3CODE_LAN")?;
    let env_dev = e
        .get("VITE_DEV_SERVER_URL")
        .map(|s| url::Url::parse(s).map_err(|e| invalid("VITE_DEV_SERVER_URL", e.to_string())))
        .transpose()?;
    let mode = f
        .mode
        .or(env_mode)
        .or(b.as_ref().map(|_| RuntimeMode::Desktop))
        .unwrap_or_default();
    let explicit_port = f.port.or(env_port).or(b.as_ref().map(|b| b.port));
    let needs_automatic_port = explicit_port.is_none() && mode == RuntimeMode::Web;
    let port = explicit_port
        .or(if needs_automatic_port {
            input.automatic_port
        } else {
            None
        })
        .unwrap_or(DEFAULT_PORT);
    let dev_url = f
        .dev_url
        .as_ref()
        .map(|s| url::Url::parse(s).map_err(|e| invalid("--dev-url", e.to_string())))
        .transpose()?
        .or(env_dev);
    let dev_auth_token = if mode == RuntimeMode::Web && dev_url.is_some() {
        let token = e
            .get("T3CODE_DEV_AUTH_TOKEN")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        if token.is_some_and(|s| s.encode_utf16().count() < 32) {
            return Err(invalid(
                "T3CODE_DEV_AUTH_TOKEN",
                "must contain at least 32 characters",
            ));
        }
        token.map(str::to_owned)
    } else {
        None
    };
    // Filtering follows first-present selection: a blank flag masks T3CODE_HOME.
    let explicit_base = f
        .base_dir
        .as_ref()
        .or(e.get("T3CODE_HOME"))
        .filter(|v| !v.trim().is_empty());
    let raw_base = explicit_base.map(String::as_str).or(b
        .as_ref()
        .and_then(|b| b.t3_home.as_ref().and_then(Option::as_deref)));
    let base = raw_base
        .filter(|s| !s.trim().is_empty())
        .map(|s| resolve_path(s.trim(), &input.home, &input.process_cwd))
        .unwrap_or_else(|| input.home.join(".t3"));
    let paths = HomeLayout::derive(base, dev_url.is_some(), explicit_base.is_some());
    let cwd = f
        .cwd
        .as_ref()
        .map(|s| resolve_path(s.trim(), &input.home, &input.process_cwd))
        .unwrap_or_else(|| input.process_cwd.clone());
    let explicit_host = f
        .host
        .clone()
        .or_else(|| e.get("T3CODE_HOST").cloned())
        .or_else(|| b.as_ref().map(|b| b.host.clone()));
    let private_network_only = explicit_host.is_none()
        && mode == RuntimeMode::Web
        && f.lan.or(env_lan).unwrap_or(input.service_managed);
    let host = explicit_host.or_else(|| {
        if private_network_only {
            Some("0.0.0.0".into())
        } else if mode == RuntimeMode::Desktop {
            Some("127.0.0.1".into())
        } else {
            None
        }
    });
    let telemetry = resolve_telemetry(e, b.as_ref(), &input.persisted_observability)?;
    Ok(ResolvedConfig {
        mode,
        port,
        needs_automatic_port,
        host,
        private_network_only,
        cwd,
        server_trace_path: e
            .get("T3CODE_TRACE_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| paths.server_trace()),
        paths,
        dev_auth_token,
        dev_allowed_origins: e
            .get("T3CODE_DEV_ALLOWED_ORIGINS")
            .into_iter()
            .flat_map(|s| s.split(','))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
        no_browser: input.headless
            || f.no_browser
                .or(env_no_browser)
                .or(b.as_ref().map(|b| b.no_browser))
                .unwrap_or(mode == RuntimeMode::Desktop),
        auto_bootstrap_project_from_cwd: input
            .force_auto_bootstrap
            .or(if input.headless { Some(false) } else { None })
            .or(f.auto_bootstrap_project_from_cwd)
            .or(env_auto)
            .unwrap_or(mode == RuntimeMode::Web),
        log_websocket_events: f
            .log_websocket_events
            .or(env_log_ws)
            .unwrap_or(dev_url.is_some()),
        tailscale_serve_enabled: f
            .tailscale_serve_enabled
            .or(env_tailscale)
            .or(b.as_ref().map(|b| b.tailscale_serve_enabled))
            .unwrap_or(false),
        tailscale_serve_port: f
            .tailscale_serve_port
            .or(env_tailscale_port)
            .or(b.as_ref().map(|b| b.tailscale_serve_port))
            .unwrap_or(443),
        bootstrap_fd: f.bootstrap_fd.or(number_env(e, "T3CODE_BOOTSTRAP_FD")?),
        bootstrap: b.clone(),
        headless: input.headless,
        log_level: input
            .cli_log_level
            .clone()
            .unwrap_or(level(e, "T3CODE_LOG_LEVEL")?),
        trace_min_level: level(e, "T3CODE_TRACE_MIN_LEVEL")?,
        trace_timing_enabled: bool_env(e, "T3CODE_TRACE_TIMING_ENABLED")?.unwrap_or(true),
        trace_max_bytes: number_env(e, "T3CODE_TRACE_MAX_BYTES")?.unwrap_or(10 * 1024 * 1024),
        trace_max_files: number_env(e, "T3CODE_TRACE_MAX_FILES")?.unwrap_or(10),
        trace_batch_window_ms: number_env(e, "T3CODE_TRACE_BATCH_WINDOW_MS")?.unwrap_or(1000),
        static_dir: if dev_url.is_none() {
            input.static_dir.clone()
        } else {
            None
        },
        dev_url,
        telemetry,
    })
}
pub fn resolve_static_dir(module_directory: &std::path::Path) -> Option<PathBuf> {
    [
        module_directory.join("client"),
        module_directory.join("../../web/dist"),
    ]
    .into_iter()
    .find(|path| path.join("index.html").is_file())
}
pub fn persisted_observability(raw: &str) -> BTreeMap<String, String> {
    let Ok(value) = crate::files::lenient_json(raw) else {
        return BTreeMap::new();
    };
    [
        ("traces", "otlpTracesUrl"),
        ("metrics", "otlpMetricsUrl"),
        ("logs", "otlpLogsUrl"),
    ]
    .into_iter()
    .filter_map(|(signal, key)| {
        value["observability"][key]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|v| (signal.into(), v.into()))
    })
    .collect()
}

#[derive(Clone, PartialEq, Eq)]
pub struct SignalExport {
    pub protocol: String,
    pub headers: Option<BTreeMap<String, String>>,
    pub export_interval_ms: i64,
}
impl std::fmt::Debug for SignalExport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignalExport")
            .field("protocol", &self.protocol)
            .field("export_interval_ms", &self.export_interval_ms)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct SignalEndpoint {
    pub url: String,
    pub export: SignalExport,
    pub source: String,
    pub variable: Option<String>,
}
impl std::fmt::Debug for SignalEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignalEndpoint")
            .field("export", &self.export)
            .field("source", &self.source)
            .field("variable", &self.variable)
            .finish_non_exhaustive()
    }
}
#[derive(Clone)]
pub struct TelemetryConfig {
    pub disabled: bool,
    pub warnings: Vec<String>,
    pub resource_attributes: BTreeMap<String, String>,
    pub traces: Option<SignalEndpoint>,
    pub metrics: Option<SignalEndpoint>,
    pub logs: Option<SignalEndpoint>,
    pub default_export: SignalExport,
}
impl std::fmt::Debug for TelemetryConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelemetryConfig")
            .field("disabled", &self.disabled)
            .field("traces", &self.traces)
            .field("metrics", &self.metrics)
            .field("logs", &self.logs)
            .field("default_export", &self.default_export)
            .finish_non_exhaustive()
    }
}
fn nonblank<'a>(env: &'a Environment, key: &str) -> Option<&'a str> {
    env.get(key).map(|s| s.trim()).filter(|s| !s.is_empty())
}
fn percent_decode(raw: &str) -> Result<String> {
    let mut bytes = Vec::new();
    let mut iter = raw.bytes();
    while let Some(c) = iter.next() {
        if c != b'%' {
            bytes.push(c);
            continue;
        }
        let h = iter.next().and_then(|c| (c as char).to_digit(16));
        let l = iter.next().and_then(|c| (c as char).to_digit(16));
        match (h, l) {
            (Some(h), Some(l)) => bytes.push((h * 16 + l) as u8),
            _ => return Err(invalid("OTLP headers", "malformed percent encoding")),
        }
    }
    String::from_utf8(bytes).map_err(|_| invalid("OTLP headers", "invalid UTF-8"))
}
pub fn parse_otlp_headers(raw: &str) -> Result<BTreeMap<String, String>> {
    let mut headers = BTreeMap::new();
    for pair in raw.split(',').filter(|s| !s.trim().is_empty()) {
        let (key, value) = pair
            .split_once('=')
            .filter(|(k, _)| !k.trim().is_empty())
            .ok_or_else(|| invalid("OTLP headers", "expected key=value pairs"))?;
        headers.insert(key.trim().to_owned(), percent_decode(value.trim())?);
    }
    Ok(headers)
}
fn protocol(raw: &str) -> Result<String> {
    if matches!(raw, "http/json" | "http/protobuf") {
        Ok(raw.to_owned())
    } else {
        Err(invalid(
            "OTLP protocol",
            "expected http/json or http/protobuf",
        ))
    }
}
fn resolve_telemetry(
    env: &Environment,
    bootstrap: Option<&DesktopBackendBootstrap>,
    saved: &BTreeMap<String, String>,
) -> Result<TelemetryConfig> {
    let default_export = SignalExport {
        protocol: protocol(
            env.get("T3CODE_OTLP_PROTOCOL")
                .map(String::as_str)
                .unwrap_or("http/json"),
        )?,
        headers: env
            .get("T3CODE_OTLP_HEADERS")
            .map(|s| parse_otlp_headers(s))
            .transpose()?,
        export_interval_ms: number_env(env, "T3CODE_OTLP_EXPORT_INTERVAL_MS")?.unwrap_or(10000),
    };
    let mut warnings = Vec::new();
    let t3_disabled = nonblank(env, "T3CODE_OTEL_SDK_DISABLED").and_then(|s| {
        let v = parse_bool(&s.to_lowercase());
        if v.is_none() {
            warnings.push("T3CODE_OTEL_SDK_DISABLED is not a yes or a no and was ignored".into());
        }
        v
    });
    let disabled = t3_disabled.unwrap_or_else(|| {
        nonblank(env, "OTEL_SDK_DISABLED").is_some_and(|s| s.eq_ignore_ascii_case("true"))
    });
    let resource_attributes = match nonblank(env, "OTEL_RESOURCE_ATTRIBUTES")
        .map(parse_otlp_headers)
        .transpose()
    {
        Ok(Some(h)) => h
            .into_iter()
            .map(|(k, v)| percent_decode(&k).map(|k| (k, v)))
            .collect::<Result<_>>()
            .unwrap_or_default(),
        Ok(None) => BTreeMap::new(),
        Err(_) => {
            warnings.push("OTEL_RESOURCE_ATTRIBUTES was ignored".into());
            BTreeMap::new()
        }
    };
    let mut signal = |name: &str, boot: Option<&String>| -> Option<SignalEndpoint> {
        if disabled {
            return None;
        }
        let t3_key = format!("T3CODE_OTLP_{}_URL", name.to_uppercase());
        if let Some(value) = nonblank(env, &t3_key) {
            return Some(SignalEndpoint {
                url: value.into(),
                export: default_export.clone(),
                source: "environment".into(),
                variable: Some(t3_key),
            });
        }
        let exporters_key = format!("OTEL_{}_EXPORTER", name.to_uppercase());
        let exporters: Vec<_> = nonblank(env, &exporters_key)
            .unwrap_or("")
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .collect();
        if exporters.iter().any(|s| s == "none") && !exporters.iter().any(|s| s == "otlp") {
            return None;
        }
        let own_prefix = format!("OTEL_EXPORTER_OTLP_{}_", name.to_uppercase());
        let own_endpoint_key = format!("{own_prefix}ENDPOINT");
        let endpoint = nonblank(env, &own_endpoint_key)
            .map(|v| (v, own_endpoint_key.clone()))
            .or_else(|| {
                nonblank(env, "OTEL_EXPORTER_OTLP_ENDPOINT")
                    .map(|v| (v, "OTEL_EXPORTER_OTLP_ENDPOINT".into()))
            });
        if let Some((value, key)) = endpoint {
            let parsed = (|| -> Result<_> {
                let mut url = url::Url::parse(value).map_err(|e| invalid(&key, e.to_string()))?;
                if !matches!(url.scheme(), "http" | "https") {
                    return Err(invalid(&key, "expected HTTP URL"));
                }
                if key == "OTEL_EXPORTER_OTLP_ENDPOINT" {
                    url.set_path(&format!("{}/v1/{name}", url.path().trim_end_matches('/')));
                }
                let raw_protocol = nonblank(env, &format!("{own_prefix}PROTOCOL"))
                    .or_else(|| nonblank(env, "OTEL_EXPORTER_OTLP_PROTOCOL"))
                    .unwrap_or("http/protobuf");
                let raw_headers = nonblank(env, &format!("{own_prefix}HEADERS"))
                    .or_else(|| nonblank(env, "OTEL_EXPORTER_OTLP_HEADERS"));
                Ok(SignalEndpoint {
                    url: url.to_string(),
                    export: SignalExport {
                        protocol: protocol(&raw_protocol.to_lowercase())?,
                        headers: raw_headers.map(parse_otlp_headers).transpose()?,
                        export_interval_ms: default_export.export_interval_ms,
                    },
                    source: "environment".into(),
                    variable: Some(key),
                })
            })();
            return match parsed {
                Ok(e) => Some(e),
                Err(error) => {
                    warnings.push(error.to_string());
                    None
                }
            };
        }
        let saved = saved.get(name).map(|s| s.trim()).filter(|s| !s.is_empty());
        let chosen = boot.map(|s| s.trim()).filter(|s| !s.is_empty()).or(saved)?;
        Some(SignalEndpoint {
            url: chosen.into(),
            export: default_export.clone(),
            source: if Some(chosen) == saved {
                "settings"
            } else {
                "bootstrap"
            }
            .into(),
            variable: None,
        })
    };
    let traces = signal(
        "traces",
        bootstrap.and_then(|b| b.otlp_traces_url.as_ref().and_then(Option::as_ref)),
    );
    let metrics = signal(
        "metrics",
        bootstrap.and_then(|b| b.otlp_metrics_url.as_ref().and_then(Option::as_ref)),
    );
    let logs = signal(
        "logs",
        bootstrap.and_then(|b| b.otlp_logs_url.as_ref().and_then(Option::as_ref)),
    );
    Ok(TelemetryConfig {
        disabled,
        warnings,
        resource_attributes,
        traces,
        metrics,
        logs,
        default_export,
    })
}
