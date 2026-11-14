//! OpenCode probing uses the bounded transport in `opencode_http`.
//!
//! ```compile_fail
//! use t3_provider::discovery::probe_opencode_server;
//! ```
use crate::{
    Error, Result,
    environment::{Environment, Platform, probe_environment, process_options},
    invalid,
    versions::*,
};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    time::Duration,
};
use t3_contracts::server::ServerProviderState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driver {
    Codex,
    Claude,
    Cursor,
    Grok,
    OpenCode,
    Antigravity,
    Kiro,
    Pi,
    Muse,
    Fx,
    AcpRegistry,
}

impl Driver {
    pub const ALL: [Self; 11] = [
        Self::Codex,
        Self::Claude,
        Self::Cursor,
        Self::Grok,
        Self::OpenCode,
        Self::Antigravity,
        Self::Kiro,
        Self::Pi,
        Self::Muse,
        Self::Fx,
        Self::AcpRegistry,
    ];
    pub fn kind(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claudeAgent",
            Self::Cursor => "cursor",
            Self::Grok => "grok",
            Self::OpenCode => "opencode",
            Self::Antigravity => "antigravity",
            Self::Kiro => "kiro",
            Self::Pi => "pi",
            Self::Muse => "muse",
            Self::Fx => "fx",
            Self::AcpRegistry => "acpRegistry",
        }
    }
    pub fn from_kind(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.kind() == value)
    }
    pub fn binary(self) -> Option<&'static str> {
        match self {
            Self::Codex => Some("codex"),
            Self::Claude => Some("claude"),
            Self::Grok => Some("grok"),
            Self::OpenCode => Some("opencode"),
            Self::Kiro => Some("kiro-cli"),
            Self::Pi => Some("pi"),
            Self::Muse => Some("muse"),
            Self::Fx => Some("fx"),
            _ => None,
        }
    }
    pub fn minimum_version(self) -> Option<&'static str> {
        match self {
            Self::Pi => Some("0.80.5"),
            Self::OpenCode => Some("1.14.19"),
            _ => None,
        }
    }
    pub fn default_probe_timeout(self) -> u64 {
        match self {
            Self::Claude => 20_000,
            Self::Muse => 4_000,
            _ => 10_000,
        }
    }
    pub fn parse_version(self, output: &str) -> Option<String> {
        match self {
            Self::Codex => parse_codex_version(output),
            Self::Muse => parse_muse_version(output),
            _ => parse_generic_version(output),
        }
    }
    pub fn discovery_route(self) -> DiscoveryRoute {
        match self {
            Self::Cursor => DiscoveryRoute::CursorSdk,
            Self::Antigravity => DiscoveryRoute::ManagedAcp,
            Self::AcpRegistry => DiscoveryRoute::RegistryDistribution,
            _ => DiscoveryRoute::Cli,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryRoute {
    Cli,
    CursorSdk,
    ManagedAcp,
    RegistryDistribution,
}

pub fn is_windows_command_not_found(platform: Platform, code: Option<i32>, stderr: &str) -> bool {
    if platform != Platform::Windows {
        return false;
    }
    if code == Some(9009) {
        return true;
    }
    let lower = stderr.to_lowercase();
    [
        "is not recognized as an internal or external command",
        "no se reconoce como un comando interno o externo",
        "wird nicht als interner oder externer befehl",
    ]
    .iter()
    .any(|s| lower.contains(s))
        || {
            static LOCALIZED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
                regex::Regex::new(r"(?i)n.o . reconhecido como um comando interno|non . riconosciuto come comando interno o esterno|n.est pas reconnu en tant que commande interne").expect("missing command regex")
            });
            LOCALIZED.is_match(stderr)
        }
}

#[derive(Debug, Clone)]
pub struct VersionProbe {
    pub installed: bool,
    pub version: Option<String>,
    pub status: ServerProviderState,
    pub message: Option<String>,
    pub resolved_executable: Option<PathBuf>,
}

/// A version-only check; authentication and native catalog probing are separate operations.
pub async fn probe_version(
    driver: Driver,
    binary: Option<&str>,
    env: &Environment,
    cwd: Option<&Path>,
) -> Result<VersionProbe> {
    if driver.discovery_route() != DiscoveryRoute::Cli {
        return Err(Error::NotPorted(match driver {
            Driver::Cursor => "Cursor SDK discovery",
            Driver::Antigravity => "Antigravity managed installation and ACP discovery",
            _ => "ACP registry installation discovery",
        }));
    }
    let command = binary
        .filter(|s| !s.is_empty())
        .or(driver.binary())
        .ok_or_else(|| invalid("provider executable", "missing binary"))?;
    let mut env = probe_environment(env);
    if driver == Driver::Grok {
        env.insert("GROK_DISABLE_AUTOUPDATER".into(), "1".into());
    }
    if driver == Driver::Muse {
        env.insert("MUSE_NO_AUTO_UPDATE".into(), "1".into());
    }
    let mut options = process_options(command, &["--version".into()], &env);
    options.timeout = Some(Duration::from_millis(probe_timeout_ms(
        if driver == Driver::Claude {
            "claude"
        } else {
            driver.kind()
        },
        driver.default_probe_timeout(),
        &env,
    )));
    options.cwd = cwd.map(Path::to_path_buf);
    options.check_exit = false;
    let resolved = t3_process::resolve_command(OsStr::new(command), &[], &options.env, false)
        .ok()
        .map(|r| PathBuf::from(r.program));
    match t3_process::run(options).await {
        Err(t3_process::ProcessError::NotFound { .. }) => Ok(VersionProbe {
            installed: false,
            version: None,
            status: ServerProviderState::Error,
            message: Some(format!("{command} is not installed or not on PATH.")),
            resolved_executable: None,
        }),
        Err(t3_process::ProcessError::Timeout { .. }) => Ok(VersionProbe {
            installed: true,
            version: None,
            status: ServerProviderState::Error,
            message: Some(format!("{command} version check timed out.")),
            resolved_executable: resolved,
        }),
        Err(error) => Err(error.into()),
        Ok(output) => {
            if is_windows_command_not_found(Platform::current(), output.code, &output.stderr_text())
            {
                return Ok(VersionProbe {
                    installed: false,
                    version: None,
                    status: ServerProviderState::Error,
                    message: Some(format!("{command} is not installed or not on PATH.")),
                    resolved_executable: resolved,
                });
            }
            let version = if output.code == Some(0) {
                driver.parse_version(&if driver == Driver::Codex || driver == Driver::OpenCode {
                    output.stdout_text()
                } else {
                    format!("{}\n{}", output.stdout_text(), output.stderr_text())
                })
            } else {
                None
            };
            let message = if output.code != Some(0) {
                Some(
                    if driver == Driver::Muse
                        && output.stderr_text().contains("installed binary is missing")
                    {
                        "Muse installed binary is missing.".into()
                    } else {
                        format!("{command} is installed but failed to run.")
                    },
                )
            } else if let Some(minimum) = driver.minimum_version() {
                match version.as_deref() {
                    None => Some(format!(
                        "Could not determine {command} version; {minimum} or newer is required."
                    )),
                    Some(v) if compare_versions(v, minimum).is_lt() => Some(format!(
                        "{command} {v} is unsupported; update to {minimum} or newer."
                    )),
                    _ => None,
                }
            } else {
                None
            };
            Ok(VersionProbe {
                installed: true,
                version,
                status: if message.is_some() {
                    ServerProviderState::Error
                } else {
                    ServerProviderState::Warning
                },
                message,
                resolved_executable: resolved,
            })
        }
    }
}

pub fn resolve_claude_sdk_executable(binary: &str, env: &Environment) -> String {
    if !cfg!(windows) {
        return binary.into();
    }
    let options = process_options(binary, &[], env);
    let resolved = t3_process::resolve_command(OsStr::new(binary), &[], &options.env, false)
        .ok()
        .map(|r| r.program.to_string_lossy().into_owned())
        .unwrap_or_else(|| binary.into());
    let path = Path::new(&resolved);
    if !path.extension().is_some_and(|ext| {
        ["cmd", "bat", "ps1"]
            .iter()
            .any(|e| ext.eq_ignore_ascii_case(e))
    }) {
        return resolved;
    }
    let Some(parent) = path.parent() else {
        return binary.into();
    };
    for suffix in [
        "node_modules/@anthropic-ai/claude-code/bin/claude.exe",
        "node_modules/@anthropic-ai/claude-code/cli.js",
    ] {
        let candidate = parent.join(suffix);
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    binary.into()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeVersion {
    pub generation: u8,
    pub version: String,
}
pub fn classify_opencode_version(version: &str) -> Option<OpenCodeVersion> {
    Some(OpenCodeVersion {
        generation: if parse_version(version)?.numbers[0] >= 2 {
            2
        } else {
            1
        },
        version: version.into(),
    })
}

pub fn classify_opencode_response(
    path: &str,
    status: u16,
    content_type: Option<&str>,
    body: &str,
) -> Result<Option<OpenCodeVersion>> {
    if status == 401 {
        return Err(invalid(
            "OpenCode server",
            "401 Unauthorized: the OpenCode server rejected the password.",
        ));
    }
    if status != 200
        || content_type
            .and_then(|s| s.split(';').next())
            .is_none_or(|s| !s.trim().eq_ignore_ascii_case("application/json"))
    {
        return Ok(None);
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return Ok(None);
    };
    if path == "/api/info" {
        if !value.get("pid").is_some_and(|v| v.as_i64().is_some()) {
            return Ok(None);
        }
    } else if path == "/global/health" {
        if value.get("healthy").and_then(serde_json::Value::as_bool) != Some(true) {
            return Ok(None);
        }
    } else {
        return Ok(None);
    }
    Ok(value
        .get("version")
        .and_then(serde_json::Value::as_str)
        .and_then(classify_opencode_version))
}

pub fn managed_release<'a>(
    installed: Option<&str>,
    pinned: Option<&'a str>,
    upstream: Option<&'a str>,
) -> Option<&'a str> {
    let Some(installed) = installed else {
        return pinned;
    };
    let mut selected = None;
    for candidate in [pinned, upstream].into_iter().flatten() {
        let current = selected.unwrap_or(installed);
        let newer = if parse_version(candidate).is_some() && parse_version(current).is_some() {
            compare_versions(candidate, current).is_gt()
        } else {
            candidate != current
        };
        if newer {
            selected = Some(candidate);
        }
    }
    selected
}
