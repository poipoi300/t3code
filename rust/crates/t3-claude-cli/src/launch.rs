//! Pure SDK-equivalent launch compilation. Resolution and environment inputs are caller-owned.
use crate::{
    Error, PROTOCOL_BASELINE,
    control::{Initialize, PermissionMode},
};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};

pub type Environment = BTreeMap<String, String>;

#[derive(Clone, PartialEq)]
pub struct SpawnDescription {
    pub command: String,
    pub args: Vec<String>,
    pub env: Environment,
    pub cwd: Option<PathBuf>,
    pub initialize: Initialize,
}

#[derive(Clone, PartialEq)]
pub enum SystemPrompt {
    /// SDK default for an omitted option: an empty replacement prompt.
    Unspecified,
    Custom(Vec<String>),
    ClaudeCode {
        append: Option<String>,
    },
}
#[derive(Clone)]
pub enum Thinking {
    Adaptive {
        display: Option<String>,
    },
    Enabled {
        budget_tokens: Option<u64>,
        display: Option<String>,
    },
    Disabled,
}
#[derive(Clone)]
pub enum Tools {
    Default,
    List(Vec<String>),
}
#[derive(Clone)]
pub enum Settings {
    Json(Value),
    Path(String),
}

#[derive(Clone)]
pub struct LaunchOptions {
    /// An installed native executable or JS entry; Windows shims must be resolved first.
    pub cli_path: Option<String>,
    /// Package candidate specifier -> resolved existing binary path, in SDK preference order.
    pub resolved_packages: Vec<(String, String)>,
    pub platform: String,
    pub arch: String,
    pub prefer_musl: bool,
    pub javascript_runtime: String,
    pub executable_args: Vec<String>,
    pub custom_spawn: bool,
    pub cwd: Option<PathBuf>,
    pub env: Environment,
    pub thinking: Option<Thinking>,
    pub effort: Option<String>,
    pub model: Option<String>,
    pub fallback_model: Option<String>,
    pub agent: Option<String>,
    pub betas: Vec<String>,
    pub json_schema: Option<Value>,
    pub max_turns: Option<u64>,
    pub max_budget_usd: Option<f64>,
    pub task_budget: Option<u64>,
    pub can_use_tool: bool,
    pub permission_prompt_tool: Option<String>,
    pub permission_prompts: Option<String>,
    pub continue_conversation: bool,
    pub resume: Option<String>,
    pub resume_session_at: Option<String>,
    pub resume_drops_turn: Option<String>,
    pub session_id: Option<String>,
    pub fork_session: bool,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub tools: Option<Tools>,
    pub mcp_servers: BTreeMap<String, Value>,
    pub setting_sources: Option<Vec<String>>,
    pub strict_mcp_config: bool,
    pub permission_mode: Option<PermissionMode>,
    pub allow_dangerously_skip_permissions: bool,
    pub include_partial_messages: bool,
    pub include_hook_events: bool,
    pub additional_directories: Vec<String>,
    pub project_config_root: Option<String>,
    pub persist_session: bool,
    pub settings: Option<Settings>,
    pub managed_settings: Option<Value>,
    /// Insertion-ordered, like JavaScript Object.entries. null means a valueless flag.
    pub extra_args: Vec<(String, Option<String>)>,
    pub debug: bool,
    pub debug_file: Option<String>,
    pub system_prompt: SystemPrompt,
    pub initialize: Initialize,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            cli_path: None,
            resolved_packages: Vec::new(),
            platform: std::env::consts::OS
                .replace("windows", "win32")
                .replace("macos", "darwin"),
            arch: std::env::consts::ARCH
                .replace("x86_64", "x64")
                .replace("aarch64", "arm64"),
            prefer_musl: false,
            javascript_runtime: "node".into(),
            executable_args: Vec::new(),
            custom_spawn: true,
            cwd: None,
            env: Environment::new(),
            thinking: None,
            effort: None,
            model: None,
            fallback_model: None,
            agent: None,
            betas: Vec::new(),
            json_schema: None,
            max_turns: None,
            max_budget_usd: None,
            task_budget: None,
            can_use_tool: false,
            permission_prompt_tool: None,
            permission_prompts: None,
            continue_conversation: false,
            resume: None,
            resume_session_at: None,
            resume_drops_turn: None,
            session_id: None,
            fork_session: false,
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            tools: None,
            mcp_servers: BTreeMap::new(),
            setting_sources: None,
            strict_mcp_config: false,
            permission_mode: None,
            allow_dangerously_skip_permissions: false,
            include_partial_messages: false,
            include_hook_events: false,
            additional_directories: Vec::new(),
            project_config_root: None,
            persist_session: true,
            settings: None,
            managed_settings: None,
            extra_args: Vec::new(),
            debug: false,
            debug_file: None,
            system_prompt: SystemPrompt::Unspecified,
            initialize: Initialize::default(),
        }
    }
}

pub fn binary_candidates(platform: &str, arch: &str, prefer_musl: bool) -> Vec<String> {
    let prefix = "@anthropic-ai/claude-agent-sdk";
    let packages = match platform {
        "android" => vec![format!("{prefix}-linux-{arch}-android")],
        "linux" => {
            let native = format!("{prefix}-linux-{arch}");
            let musl = format!("{native}-musl");
            if prefer_musl {
                vec![musl, native]
            } else {
                vec![native, musl]
            }
        }
        _ => vec![format!("{prefix}-{platform}-{arch}")],
    };
    packages
        .into_iter()
        .map(|p| {
            format!(
                "{p}/claude{}",
                if platform == "win32" { ".exe" } else { "" }
            )
        })
        .collect()
}

/// T3's Windows npm-shim selection, using a caller-supplied PATH result and existence predicate.
pub fn resolve_executable(
    binary: &str,
    platform: &str,
    path_result: Option<&str>,
    exists: impl Fn(&str) -> bool,
) -> String {
    if platform != "win32" {
        return binary.into();
    }
    let resolved = path_result.unwrap_or(binary);
    let lower = resolved.to_ascii_lowercase();
    if ![".cmd", ".bat", ".ps1"]
        .iter()
        .any(|ext| lower.ends_with(ext))
    {
        return resolved.into();
    }
    let directory = resolved
        .rsplit_once(['\\', '/'])
        .map_or("", |(dir, _)| dir)
        .replace('/', "\\");
    for suffix in [
        "node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe",
        "node_modules\\@anthropic-ai\\claude-code\\cli.js",
    ] {
        let candidate = if directory.is_empty() {
            suffix.into()
        } else {
            format!("{directory}\\{suffix}")
        };
        if exists(&candidate) {
            return candidate;
        }
    }
    binary.into()
}

pub fn describe_spawn(options: &LaunchOptions) -> Result<SpawnDescription, Error> {
    if options.can_use_tool && options.permission_prompt_tool.is_some() {
        return Err(Error::Options(
            "canUseTool and permissionPromptToolName are mutually exclusive".into(),
        ));
    }
    if options.fallback_model.is_some() && options.fallback_model == options.model {
        return Err(Error::Options("fallback model equals main model".into()));
    }
    let path = match &options.cli_path {
        Some(p) => p.clone(),
        None => binary_candidates(&options.platform, &options.arch, options.prefer_musl)
            .iter()
            .find_map(|candidate| {
                options
                    .resolved_packages
                    .iter()
                    .find(|(p, _)| p == candidate)
                    .map(|(_, p)| p.clone())
            })
            .ok_or_else(|| {
                Error::Options(
                    "no resolved SDK native binary; supply cli_path or resolved_packages".into(),
                )
            })?,
    };
    let mut args = vec![
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--input-format".into(),
        "stream-json".into(),
    ];
    let eq = |args: &mut Vec<String>, key: &str, value: &str| args.push(format!("--{key}={value}"));
    if let Some(thinking) = &options.thinking {
        let display = match thinking {
            Thinking::Disabled => {
                args.push("--thinking=disabled".into());
                None
            }
            Thinking::Adaptive { display } => {
                args.push("--thinking=adaptive".into());
                display.as_ref()
            }
            Thinking::Enabled {
                budget_tokens,
                display,
            } => {
                if let Some(budget) = budget_tokens {
                    eq(&mut args, "max-thinking-tokens", &budget.to_string());
                } else {
                    args.push("--thinking=adaptive".into());
                }
                display.as_ref()
            }
        };
        if let Some(d) = display.filter(|d| !d.is_empty()) {
            eq(&mut args, "thinking-display", d);
        }
    }
    if let Some(v) = options.effort.as_ref().filter(|v| !v.is_empty()) {
        eq(&mut args, "effort", v);
    }
    if let Some(n) = options.max_turns.filter(|n| *n > 0) {
        eq(&mut args, "max-turns", &n.to_string());
    }
    if let Some(n) = options.max_budget_usd {
        if !n.is_finite() {
            return Err(Error::Options("nonfinite budget".into()));
        }
        eq(&mut args, "max-budget-usd", &n.to_string());
    }
    if let Some(n) = options.task_budget {
        eq(&mut args, "task-budget", &n.to_string());
    }
    for (key, value) in [("model", &options.model), ("agent", &options.agent)] {
        if let Some(v) = value.as_ref().filter(|v| !v.is_empty()) {
            eq(&mut args, key, v);
        }
    }
    if !options.betas.is_empty() {
        eq(&mut args, "betas", &options.betas.join(","));
    }
    if let Some(v) = &options.json_schema {
        eq(&mut args, "json-schema", &serde_json::to_string(v)?);
    }
    if let Some(v) = &options.debug_file {
        eq(&mut args, "debug-file", v);
    } else if options.debug {
        args.push("--debug".into());
    }
    if options.can_use_tool {
        args.push("--permission-prompt-tool=stdio".into());
    } else if let Some(v) = &options.permission_prompt_tool {
        eq(&mut args, "permission-prompt-tool", v);
    }
    if let Some(v) = &options.permission_prompts {
        eq(&mut args, "permission-prompts", v);
    }
    if options.continue_conversation {
        args.push("--continue".into());
    }
    if let Some(v) = &options.resume {
        eq(&mut args, "resume", v);
    }
    for (key, list) in [
        ("allowedTools", &options.allowed_tools),
        ("disallowedTools", &options.disallowed_tools),
    ] {
        if !list.is_empty() {
            eq(&mut args, key, &list.join(","));
        }
    }
    if let Some(tools) = &options.tools {
        eq(
            &mut args,
            "tools",
            &match tools {
                Tools::Default => "default".into(),
                Tools::List(v) => v.join(","),
            },
        );
    }
    if !options.mcp_servers.is_empty() {
        split_flag(
            &mut args,
            "mcp-config",
            &serde_json::to_string(&serde_json::json!({"mcpServers": options.mcp_servers}))?,
        );
    }
    if let Some(v) = &options.setting_sources {
        eq(&mut args, "setting-sources", &v.join(","));
    }
    if options.strict_mcp_config {
        args.push("--strict-mcp-config".into());
    }
    if let Some(v) = options.permission_mode {
        eq(&mut args, "permission-mode", v.as_str());
    }
    if options.allow_dangerously_skip_permissions {
        args.push("--allow-dangerously-skip-permissions".into());
    }
    if let Some(v) = &options.fallback_model {
        eq(&mut args, "fallback-model", v);
    }
    if options.include_hook_events {
        args.push("--include-hook-events".into());
    }
    if options.include_partial_messages {
        args.push("--include-partial-messages".into());
    }
    if let Some(v) = &options.project_config_root {
        eq(&mut args, "project-config-root", v);
    }
    for v in &options.additional_directories {
        eq(&mut args, "add-dir", v);
    }
    if options.fork_session {
        args.push("--fork-session".into());
    }
    for (key, value) in [
        ("resume-session-at", &options.resume_session_at),
        ("resume-drops-turn", &options.resume_drops_turn),
        ("session-id", &options.session_id),
    ] {
        if let Some(v) = value {
            eq(&mut args, key, v);
        }
    }
    if !options.persist_session {
        args.push("--no-session-persistence".into());
    }
    if let Some(v) = &options.managed_settings {
        split_flag(&mut args, "managed-settings", &serde_json::to_string(v)?);
    }
    let mut extra = options.extra_args.clone();
    let auto_await = (options.resume.is_some() || options.continue_conversation)
        && options.cli_path.is_none()
        && !options.custom_spawn
        && !extra
            .iter()
            .any(|(k, _)| k == "sdk-url" || k.starts_with("sdk-url="))
        && !options
            .executable_args
            .iter()
            .any(|a| a == "--sdk-url" || a.starts_with("--sdk-url="));
    if auto_await {
        put_extra(&mut extra, "await-initialize", None);
    }
    if let Some(settings) = &options.settings {
        let v = match settings {
            Settings::Json(v) => serde_json::to_string(v)?,
            Settings::Path(p) => p.clone(),
        };
        put_extra(&mut extra, "settings", Some(v));
    }
    for (key, value) in extra {
        if let Some(v) = value {
            split_flag(&mut args, &key, &v);
        } else {
            args.push(format!("--{key}"));
        }
    }
    let mut env = options.env.clone();
    for (key, value) in [
        ("CLAUDE_CODE_ENTRYPOINT", "sdk-ts"),
        ("CLAUDE_AGENT_SDK_VERSION", PROTOCOL_BASELINE.sdk),
    ] {
        if env.get(key).is_none_or(String::is_empty) {
            env.insert(key.into(), value.into());
        }
    }
    if !env
        .keys()
        .any(|key| key.eq_ignore_ascii_case("CLAUDE_CODE_SDK_READS_SESSION_STATE"))
    {
        env.insert("CLAUDE_CODE_SDK_READS_SESSION_STATE".into(), "1".into());
    }
    env.remove("NODE_OPTIONS");
    if env
        .get("DEBUG_CLAUDE_AGENT_SDK")
        .is_some_and(|v| matches!(v.to_ascii_lowercase().trim(), "1" | "true" | "yes" | "on"))
    {
        env.insert("DEBUG".into(), "1".into());
    } else {
        env.remove("DEBUG");
    }
    let mut initialize = options.initialize.clone();
    match &options.system_prompt {
        SystemPrompt::Unspecified => initialize.system_prompt = Some(vec![String::new()]),
        SystemPrompt::Custom(v) => initialize.system_prompt = Some(v.clone()),
        SystemPrompt::ClaudeCode { append } => {
            initialize.system_prompt = None;
            initialize.append_system_prompt = append.clone();
        }
    }
    let native = ![".js", ".mjs", ".tsx", ".ts", ".jsx"]
        .iter()
        .any(|suffix| path.ends_with(suffix));
    let mut prefixed = options.executable_args.clone();
    if !native {
        prefixed.push(path.clone());
    }
    prefixed.extend(args);
    Ok(SpawnDescription {
        command: if native {
            path
        } else {
            options.javascript_runtime.clone()
        },
        args: prefixed,
        env,
        cwd: options.cwd.clone(),
        initialize,
    })
}

fn split_flag(args: &mut Vec<String>, key: &str, value: &str) {
    if value.len() > 1 && value.starts_with('-') {
        args.push(format!("--{key}={value}"));
    } else {
        args.extend([format!("--{key}"), value.into()]);
    }
}
fn put_extra(args: &mut Vec<(String, Option<String>)>, key: &str, value: Option<String>) {
    if let Some((_, existing)) = args.iter_mut().find(|(k, _)| k == key) {
        *existing = value;
    } else {
        args.push((key.into(), value));
    }
}

/// T3's host-session cleanup, applied before explicit instance overrides. No files are read.
pub fn t3_environment(
    inherited: &Environment,
    configured: &Environment,
    resolved_home: Option<&str>,
    resume: bool,
) -> Environment {
    let mut env = inherited.clone();
    for key in [
        "CLAUDECODE",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_CODE_HOST_SESSION_ID",
        "CLAUDE_CODE_CHILD_SESSION",
        "CLAUDE_CODE_SESSION_ATTENDED",
        "CLAUDE_CODE_MESSAGING_SOCKET",
        "CLAUDE_CODE_MESSAGING_TOKEN",
        "CLAUDE_CODE_EXECPATH",
        "CLAUDE_CODE_DESKTOP_APP_VERSION",
        "CLAUDE_EFFORT",
        "CLAUDE_PID",
        "CLAUDE_AGENT_SDK_VERSION",
        "ELECTRON_RUN_AS_NODE",
    ] {
        env.remove(key);
    }
    env.insert("CLAUDE_CODE_ENTRYPOINT".into(), "t3-code".into());
    for (name, value) in configured {
        let name = if cfg!(windows) {
            name.to_uppercase()
        } else {
            name.clone()
        };
        if cfg!(windows) {
            env.retain(|key, _| key.to_uppercase() != name);
        }
        env.insert(name, value.clone());
    }
    if let Some(home) = resolved_home.filter(|h| !h.is_empty()) {
        env.insert("CLAUDE_CONFIG_DIR".into(), home.into());
    }
    if resume {
        env.entry("MCP_CONNECTION_NONBLOCKING".into())
            .or_insert_with(|| "0".into());
    }
    env
}

crate::redacted_debug!(
    SpawnDescription,
    SystemPrompt,
    Thinking,
    Tools,
    Settings,
    LaunchOptions
);
