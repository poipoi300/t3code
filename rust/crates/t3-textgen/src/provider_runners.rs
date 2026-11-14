//! One-shot structured generation through a provider CLI (`CodexTextGeneration.ts`,
//! `ClaudeTextGeneration.ts`). The host resolves the instance (binary, explicit environment,
//! model and options) and supplies it per request; this module owns the launch policy and
//! process I/O. No tool, approval-free execution or write access is granted beyond
//! TypeScript's: Codex runs `exec` in the `read-only` sandbox with plugins, apps and every
//! enabled MCP server turned off; Claude runs `-p` with no tools (review guides: read-only
//! tools and three `git` read commands), hooks and slash commands disabled, strict MCP config
//! and `dontAsk`. Errors are content-free: process output never leaves this module.
use crate::{
    CancellationToken,
    generation::{GenerationError, GenerationRequest, Operation},
};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

pub type Environment = BTreeMap<String, String>;

const CODEX_TIMEOUT: Duration = Duration::from_secs(180);
const CODEX_MCP_LIST_TIMEOUT: Duration = Duration::from_secs(10);
const CLAUDE_TIMEOUT: Duration = Duration::from_secs(180);
const NO_PLUGIN_ARGS: [&str; 4] = [
    "--config",
    "features.plugins=false",
    "--config",
    "features.apps=false",
];
pub const DEFAULT_TEXT_GENERATION_REASONING_EFFORT: &str = "low";

/// A resolved Codex instance for one request.
pub struct CodexRun {
    pub program: String,
    /// `resolveCodexLaunchArgs` tokenised (`T3CODE_CODEX_LAUNCH_ARGS` wins over the setting).
    pub launch_argv: Vec<String>,
    /// The complete child environment (`inherit_env` is off), CODEX_HOME included.
    pub environment: Environment,
    pub model: String,
    pub reasoning_effort: String,
    pub service_tier: Option<String>,
    /// Existing image attachment files, in request order.
    pub image_paths: Vec<PathBuf>,
}

/// A Codex model as the instance's snapshot lists it.
pub struct CodexModel {
    pub slug: String,
    pub is_custom: bool,
    pub is_default: bool,
    pub is_legacy: bool,
}

/// `codexModelFamily`.
pub fn codex_model_family(slug: &str) -> &str {
    if slug.starts_with("openai.gpt-") {
        &slug["openai.".len()..]
    } else {
        slug
    }
}

/// The model `runCodexJson` sends and whether it fell back from the requested one (then the
/// selection's options do not apply).
pub fn resolve_codex_model(
    requested: &str,
    models: &[CodexModel],
    custom: &[String],
) -> (String, bool) {
    let listed: Vec<&CodexModel> = models.iter().filter(|model| !model.is_custom).collect();
    let family = codex_model_family(requested);
    let matched = models
        .iter()
        .find(|model| model.slug == requested)
        .map(|model| model.slug.clone())
        .or_else(|| custom.iter().find(|slug| *slug == requested).cloned())
        .or_else(|| {
            listed
                .iter()
                .find(|model| codex_model_family(&model.slug) == family)
                .map(|model| model.slug.clone())
        });
    let model = matched.clone().unwrap_or_else(|| {
        listed
            .iter()
            .find(|model| model.is_default)
            .or_else(|| listed.iter().find(|model| !model.is_legacy))
            .or_else(|| listed.first())
            .map_or_else(|| requested.to_owned(), |model| model.slug.clone())
    });
    let fell_back = matched.is_none() && model != requested;
    (model, fell_back)
}

struct LaunchArgFilter {
    switches: &'static [&'static str],
    value_flags: &'static [&'static str],
    inline_prefixes: &'static [&'static str],
}
const MCP_LIST_FILTER: LaunchArgFilter = LaunchArgFilter {
    switches: &[],
    value_flags: &["--config", "-c", "--enable", "--disable", "--profile", "-p"],
    inline_prefixes: &[
        "--config=",
        "-c=",
        "--enable=",
        "--disable=",
        "--profile=",
        "-p=",
    ],
};
const EXEC_FILTER: LaunchArgFilter = LaunchArgFilter {
    switches: &["--strict-config", "--oss"],
    value_flags: &[
        "--config",
        "-c",
        "--enable",
        "--disable",
        "--local-provider",
        "--profile",
        "-p",
    ],
    inline_prefixes: &[
        "--config=",
        "-c=",
        "--enable=",
        "--disable=",
        "--local-provider=",
        "--profile=",
        "-p=",
    ],
};
fn filter_launch_args(args: &[String], allowed: &LaunchArgFilter) -> Vec<String> {
    let mut kept = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if allowed.switches.contains(&arg.as_str()) {
            kept.push(arg.clone());
        } else if allowed.value_flags.contains(&arg.as_str()) {
            if let Some(value) = args.get(index + 1)
                && !value.starts_with('-')
            {
                kept.extend([arg.clone(), value.clone()]);
                index += 1;
            }
        } else if allowed
            .inline_prefixes
            .iter()
            .any(|prefix| arg.starts_with(prefix))
        {
            kept.push(arg.clone());
        }
        index += 1;
    }
    kept
}
/// `codexExecLaunchArgs`.
pub fn codex_exec_launch_args(argv: &[String]) -> Vec<String> {
    filter_launch_args(argv, &EXEC_FILTER)
}
/// `codexMcpListLaunchArgs`.
pub fn codex_mcp_list_launch_args(argv: &[String]) -> Vec<String> {
    filter_launch_args(argv, &MCP_LIST_FILTER)
}

fn process(
    program: &str,
    args: Vec<String>,
    environment: &Environment,
    cwd: PathBuf,
    cancellation: &CancellationToken,
) -> t3_process::ProcessOptions {
    let mut options = t3_process::ProcessOptions::new(program).args(args);
    options.inherit_env = false;
    options.env = environment
        .iter()
        .map(|(key, value)| (key.into(), Some(value.into())))
        .collect();
    options.cwd = Some(cwd);
    options.termination_grace = Duration::from_secs(1);
    options.check_exit = false;
    options.timeout = None;
    options.cancellation = cancellation.clone();
    options
}

/// `listEnabledMcpServers`: any failure keeps the servers listed (none disabled by name), as
/// TypeScript does; the exec still runs with plugins and apps off.
async fn enabled_mcp_servers(
    run: &CodexRun,
    cwd: &std::path::Path,
    cancellation: &CancellationToken,
) -> Vec<String> {
    let mut args = codex_mcp_list_launch_args(&run.launch_argv);
    args.extend(NO_PLUGIN_ARGS.map(str::to_owned));
    args.extend(["mcp", "list", "--json"].map(str::to_owned));
    let mut options = process(
        &run.program,
        args,
        &run.environment,
        cwd.to_path_buf(),
        cancellation,
    );
    options.timeout = Some(CODEX_MCP_LIST_TIMEOUT);
    let Ok(output) = t3_process::run(options).await else {
        return Vec::new();
    };
    if output.code != Some(0) || output.timed_out {
        return Vec::new();
    }
    let Ok(Value::Array(servers)) = serde_json::from_slice::<Value>(&output.stdout) else {
        return Vec::new();
    };
    let decoded: Option<Vec<(String, bool)>> = servers
        .iter()
        .map(|server| {
            Some((
                server["name"].as_str()?.to_owned(),
                server["enabled"].as_bool()?,
            ))
        })
        .collect();
    decoded
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(name, enabled)| enabled.then_some(name))
        .collect()
}

/// `runCodexJson`: the reply text is the `--output-last-message` file.
pub async fn run_codex(
    run: &CodexRun,
    request: &GenerationRequest,
    cancellation: CancellationToken,
) -> Result<String, GenerationError> {
    let directory = tempfile::Builder::new()
        .prefix("t3code-codex-")
        .tempdir()
        .map_err(|_| GenerationError::Provider)?;
    let schema_path = directory.path().join("schema.json");
    let output_path = directory.path().join("output.txt");
    let schema =
        serde_json::to_string(&request.output_schema).map_err(|_| GenerationError::Provider)?;
    tokio::fs::write(&schema_path, schema)
        .await
        .map_err(|_| GenerationError::Provider)?;
    tokio::fs::write(&output_path, "")
        .await
        .map_err(|_| GenerationError::Provider)?;
    let work = async {
        let enabled = enabled_mcp_servers(run, &request.cwd, &cancellation).await;
        let mut args = crate::codex::codex_exec_mcp_prefix(
            &codex_exec_launch_args(&run.launch_argv),
            &enabled,
        );
        args.extend(
            [
                "--ephemeral",
                "--skip-git-repo-check",
                "-s",
                "read-only",
                "--model",
            ]
            .map(str::to_owned),
        );
        args.push(run.model.clone());
        args.push("--config".into());
        args.push(format!(
            "model_reasoning_effort=\"{}\"",
            run.reasoning_effort
        ));
        if let Some(tier) = &run.service_tier {
            args.push("--config".into());
            args.push(format!("service_tier=\"{tier}\""));
        }
        args.push("--output-schema".into());
        args.push(schema_path.to_string_lossy().into_owned());
        args.push("--output-last-message".into());
        args.push(output_path.to_string_lossy().into_owned());
        for image in &run.image_paths {
            args.push("--image".into());
            args.push(image.to_string_lossy().into_owned());
        }
        args.push("-".into());
        let mut options = process(
            &run.program,
            args,
            &run.environment,
            request.cwd.clone(),
            &cancellation,
        );
        options.stdin = Some(request.prompt.clone().into_bytes());
        let output = t3_process::run(options)
            .await
            .map_err(|error| match error {
                t3_process::ProcessError::Cancelled => GenerationError::Cancelled,
                _ => GenerationError::Provider,
            })?;
        if output.code != Some(0) {
            return Err(GenerationError::Provider);
        }
        tokio::fs::read_to_string(&output_path)
            .await
            .map_err(|_| GenerationError::InvalidOutput)
    };
    let timeout = request.investigation_timeout.unwrap_or(CODEX_TIMEOUT);
    tokio::select! {
        biased;
        () = cancellation.cancelled() => Err(GenerationError::Cancelled),
        reply = tokio::time::timeout(timeout, work) => reply.map_err(|_| GenerationError::Timeout)?,
    }
}

/// A resolved Claude instance for one request (the host resolves the catalog model, effort and
/// option-derived settings).
pub struct ClaudeRun {
    pub program: String,
    /// The complete child environment, CLAUDE_CONFIG_DIR included for an explicit home.
    pub environment: Environment,
    pub api_model: String,
    pub effort: Option<String>,
    pub thinking: Option<bool>,
    pub fast_mode: bool,
    pub ultracode: Option<bool>,
    /// `managed-mcp.json` exists (Claude then rejects `--strict-mcp-config`).
    pub managed_mcp_config: bool,
}

/// `--tools` takes bare names; patterns go in the allow list.
const CLAUDE_INVESTIGATION_TOOLS: &str = "Read,Grep,Glob,Bash";
const CLAUDE_INVESTIGATION_ALLOWED: &str =
    "Read,Grep,Glob,Bash(git diff:*),Bash(git log:*),Bash(git show:*)";
const CLAUDE_INVESTIGATION_DENIED: &str = "Bash(git * --output*),Bash(git * -o *)";
const NO_SESSION_PERSISTENCE: &str = "--no-session-persistence";
const NO_SESSION_PERSISTENCE_REJECTED: &str = "error: unknown option '--no-session-persistence'";

/// The `--settings` document: hooks off, and only option values the selection chose.
pub fn claude_settings(run: &ClaudeRun) -> Value {
    let mut settings = serde_json::Map::new();
    settings.insert("disableAllHooks".into(), Value::Bool(true));
    if let Some(thinking) = run.thinking {
        settings.insert("alwaysThinkingEnabled".into(), Value::Bool(thinking));
    }
    if run.fast_mode {
        settings.insert("fastMode".into(), Value::Bool(true));
    }
    if let Some(ultracode) = run.ultracode {
        settings.insert("ultracode".into(), Value::Bool(ultracode));
    }
    Value::Object(settings)
}

pub fn claude_args(
    run: &ClaudeRun,
    schema: &str,
    investigate: bool,
    no_session_persistence: bool,
) -> Vec<String> {
    let mut args: Vec<String> = ["-p", "--output-format", "json", "--json-schema"]
        .map(str::to_owned)
        .to_vec();
    args.push(schema.into());
    args.push("--model".into());
    args.push(run.api_model.clone());
    if let Some(effort) = &run.effort {
        args.push("--effort".into());
        args.push(effort.clone());
    }
    args.push("--settings".into());
    args.push(claude_settings(run).to_string());
    if investigate {
        args.extend(
            [
                "--tools",
                CLAUDE_INVESTIGATION_TOOLS,
                "--allowedTools",
                CLAUDE_INVESTIGATION_ALLOWED,
                "--disallowedTools",
                CLAUDE_INVESTIGATION_DENIED,
            ]
            .map(str::to_owned),
        );
    } else {
        args.extend(["--tools".to_owned(), String::new()]);
    }
    args.push("--disable-slash-commands".into());
    if no_session_persistence {
        args.push(NO_SESSION_PERSISTENCE.into());
    }
    if !run.managed_mcp_config {
        args.push("--strict-mcp-config".into());
    }
    args.extend(["--permission-mode", "dontAsk"].map(str::to_owned));
    args
}

/// The structured output of a `--output-format json` reply: the envelope, or the last
/// `result` message of a message list.
pub fn claude_structured_output(stdout: &[u8]) -> Result<Value, GenerationError> {
    let output: Value =
        serde_json::from_slice(stdout).map_err(|_| GenerationError::InvalidOutput)?;
    let envelope = match &output {
        Value::Object(object) if object.contains_key("structured_output") => Some(&output),
        Value::Array(messages) => {
            if messages.iter().any(|message| !message["type"].is_string()) {
                return Err(GenerationError::InvalidOutput);
            }
            messages
                .iter()
                .rev()
                .find(|message| message["type"] == "result")
        }
        _ => return Err(GenerationError::InvalidOutput),
    };
    envelope
        .and_then(|envelope| envelope.get("structured_output"))
        .filter(|value| !value.is_null())
        .cloned()
        .ok_or(GenerationError::InvalidOutput)
}

/// `runClaudeJson`: titles run in an empty temporary directory, everything else in the
/// request's cwd; a Claude Code too old for `--no-session-persistence` is retried without it.
pub async fn run_claude(
    run: &ClaudeRun,
    request: &GenerationRequest,
    cancellation: CancellationToken,
) -> Result<String, GenerationError> {
    let schema =
        serde_json::to_string(&request.output_schema).map_err(|_| GenerationError::Provider)?;
    let investigate = request.investigation_timeout.is_some();
    let work = async {
        let title_directory = if request.operation == Operation::GenerateThreadTitle {
            Some(
                tempfile::Builder::new()
                    .prefix("t3code-claude-title-")
                    .tempdir()
                    .map_err(|_| GenerationError::Provider)?,
            )
        } else {
            None
        };
        let cwd = title_directory.as_ref().map_or_else(
            || request.cwd.clone(),
            |directory| directory.path().to_path_buf(),
        );
        let attempt = |persistence: bool| {
            let mut options = process(
                &run.program,
                claude_args(run, &schema, investigate, persistence),
                &run.environment,
                cwd.clone(),
                &cancellation,
            );
            options.stdin = Some(request.prompt.clone().into_bytes());
            t3_process::run(options)
        };
        let failed = |error: t3_process::ProcessError| match error {
            t3_process::ProcessError::Cancelled => GenerationError::Cancelled,
            _ => GenerationError::Provider,
        };
        let mut output = attempt(true).await.map_err(failed)?;
        if output.code != Some(0)
            && String::from_utf8_lossy(&output.stderr).contains(NO_SESSION_PERSISTENCE_REJECTED)
        {
            output = attempt(false).await.map_err(failed)?;
        }
        if output.code != Some(0) {
            return Err(GenerationError::Provider);
        }
        let structured = claude_structured_output(&output.stdout)?;
        serde_json::to_string(&structured).map_err(|_| GenerationError::InvalidOutput)
    };
    let timeout = request.investigation_timeout.unwrap_or(CLAUDE_TIMEOUT);
    tokio::select! {
        biased;
        () = cancellation.cancelled() => Err(GenerationError::Cancelled),
        reply = tokio::time::timeout(timeout, work) => reply.map_err(|_| GenerationError::Timeout)?,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn launch_args_keep_only_typescripts_flags() {
        let argv = strings(&[
            "--dangerously-bypass-approvals-and-sandbox",
            "-c",
            "model=x",
            "--profile",
            "work",
            "--oss",
            "--full-auto",
            "-c",
            "--next",
            "--enable=foo",
            "--local-provider",
            "ollama",
        ]);
        assert_eq!(
            codex_exec_launch_args(&argv),
            strings(&[
                "-c",
                "model=x",
                "--profile",
                "work",
                "--oss",
                "--enable=foo",
                "--local-provider",
                "ollama"
            ])
        );
        assert_eq!(
            codex_mcp_list_launch_args(&argv),
            strings(&["-c", "model=x", "--profile", "work", "--enable=foo"])
        );
    }

    #[test]
    fn codex_models_resolve_as_typescript_does() {
        let model = |slug: &str, is_default: bool, is_legacy: bool| CodexModel {
            slug: slug.into(),
            is_custom: false,
            is_default,
            is_legacy,
        };
        let models = vec![model("gpt-old", false, true), model("gpt-5", true, false)];
        assert_eq!(
            resolve_codex_model("gpt-5", &models, &[]),
            ("gpt-5".into(), false)
        );
        assert_eq!(
            resolve_codex_model("openai.gpt-5", &models, &[]),
            ("gpt-5".into(), false)
        );
        assert_eq!(
            resolve_codex_model("mine", &models, &["mine".into()]),
            ("mine".into(), false)
        );
        assert_eq!(
            resolve_codex_model("gpt-6-luna", &models, &[]),
            ("gpt-5".into(), true)
        );
        assert_eq!(
            resolve_codex_model("gpt-6-luna", &[], &[]),
            ("gpt-6-luna".into(), false)
        );
    }

    #[test]
    fn claude_runs_without_tools_unless_investigating() {
        let run = ClaudeRun {
            program: "claude".into(),
            environment: Environment::new(),
            api_model: "claude-haiku".into(),
            effort: Some("low".into()),
            thinking: None,
            fast_mode: false,
            ultracode: None,
            managed_mcp_config: false,
        };
        let args = claude_args(&run, "{}", false, true);
        let tools = args.iter().position(|arg| arg == "--tools").unwrap();
        assert_eq!(args[tools + 1], "");
        assert!(!args.iter().any(|arg| arg == "--allowedTools"));
        assert_eq!(&args[args.len() - 2..], ["--permission-mode", "dontAsk"]);
        assert!(args.contains(&"--strict-mcp-config".to_owned()));
        assert!(args.contains(&"--disable-slash-commands".to_owned()));
        let settings = &args[args.iter().position(|arg| arg == "--settings").unwrap() + 1];
        assert_eq!(settings, r#"{"disableAllHooks":true}"#);
        let investigating = claude_args(&run, "{}", true, false);
        assert!(!investigating.contains(&NO_SESSION_PERSISTENCE.to_owned()));
        // The investigation policy, exactly: a widened allow list (e.g. plain `Bash`) must fail.
        let value = |args: &[String], flag: &str| {
            let at = args.iter().position(|arg| arg == flag).unwrap();
            args[at + 1].clone()
        };
        assert_eq!(value(&investigating, "--tools"), "Read,Grep,Glob,Bash");
        assert_eq!(
            value(&investigating, "--allowedTools"),
            "Read,Grep,Glob,Bash(git diff:*),Bash(git log:*),Bash(git show:*)"
        );
        assert_eq!(
            value(&investigating, "--disallowedTools"),
            "Bash(git * --output*),Bash(git * -o *)"
        );
        // A checkout's own `.claude/settings*.json` never loads: `-p` skips the trust check, so
        // its allow rules and `env` would otherwise apply (review H1).
        for args in [&args, &investigating] {
            assert_eq!(value(args, "--setting-sources"), "user", "{args:?}");
        }
        assert_eq!(
            claude_structured_output(
                br#"[{"type":"system"},{"type":"result","structured_output":{"title":"x"}}]"#
            ),
            Ok(serde_json::json!({"title":"x"}))
        );
        assert_eq!(
            claude_structured_output(br#"{"structured_output":null}"#),
            Err(GenerationError::InvalidOutput)
        );
    }
}
