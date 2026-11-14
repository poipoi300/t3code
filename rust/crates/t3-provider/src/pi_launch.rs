use crate::{
    environment::Environment,
    sensitive::{Sensitive, ServiceError, ServiceResult},
};
use std::path::PathBuf;
const VALUES: &[&str] = &[
    "--api-key",
    "--append-system-prompt",
    "--exclude-tools",
    "-xt",
    "--extension",
    "-e",
    "--model",
    "--models",
    "--name",
    "-n",
    "--prompt-template",
    "--provider",
    "--session-dir",
    "--skill",
    "--system-prompt",
    "--theme",
    "--thinking",
    "--tools",
    "-t",
    "--tui-mode",
    "--use-theme",
];
const FLAGS: &[&str] = &[
    "--approve",
    "-a",
    "--no-approve",
    "-na",
    "--no-builtin-tools",
    "-nbt",
    "--no-context-files",
    "-nc",
    "--no-extensions",
    "-ne",
    "--no-prompt-templates",
    "-np",
    "--no-skills",
    "-ns",
    "--no-themes",
    "--no-tools",
    "-nt",
    "--offline",
    "--verbose",
];
const RESERVED: &[&str] = &[
    "--continue",
    "-c",
    "--export",
    "--fork",
    "--help",
    "-h",
    "--list-models",
    "--mode",
    "--no-session",
    "--print",
    "-p",
    "--resume",
    "-r",
    "--session",
    "--session-id",
    "--version",
    "-v",
];
pub fn resolve_launch_args(input: &str) -> ServiceResult<Sensitive<Vec<String>>> {
    let args = t3_codex_app_server::launch::tokenize_cli_args(input)
        .into_iter()
        .flat_map(|argument| {
            if let Some((key, value)) = argument.split_once('=')
                && VALUES.contains(&key)
            {
                vec![key.into(), value.into()]
            } else {
                vec![argument]
            }
        })
        .collect::<Vec<_>>();
    let (mut provider, mut model, mut index) = (false, false, 0);
    while index < args.len() {
        let arg = args[index].as_str();
        if RESERVED.iter().any(|reserved| {
            arg == *reserved
                || reserved.starts_with("--") && arg.starts_with(&format!("{reserved}="))
        }) || arg == "--"
        {
            return Err(ServiceError::Response);
        }
        if VALUES.contains(&arg) {
            if args.get(index + 1).is_none() {
                return Err(ServiceError::Response);
            }
            provider |= arg == "--provider";
            model |= arg == "--model";
            index += 2;
            continue;
        }
        if FLAGS.contains(&arg) || arg.starts_with("--") && arg.contains('=') {
            index += 1;
            continue;
        }
        if arg.starts_with("--") {
            if args
                .get(index + 1)
                .is_some_and(|next| !next.starts_with(['-', '@']))
            {
                index += 1;
            }
        } else {
            return Err(ServiceError::Response);
        }
        index += 1;
    }
    if provider && !model {
        return Err(ServiceError::Response);
    }
    Ok(Sensitive::new(args))
}
pub struct RpcLaunch {
    pub binary: String,
    pub args: Sensitive<Vec<String>>,
    pub cwd: PathBuf,
    pub environment: Sensitive<Environment>,
}
impl std::fmt::Debug for RpcLaunch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PiRpcLaunch([REDACTED])")
    }
}
pub struct LaunchOptions<'a> {
    pub binary: &'a str,
    pub args: &'a [String],
    pub cwd: &'a std::path::Path,
    pub environment: &'a Environment,
    pub session_path: Option<&'a str>,
    pub ephemeral: bool,
    pub short_lived: bool,
    pub disable_extensions: bool,
    pub disable_tools: bool,
    pub extension_path: Option<&'a str>,
    pub runtime_mode: Option<&'a str>,
    pub mcp: Option<(&'a str, &'a Sensitive<String>)>,
}
pub fn build_launch(input: LaunchOptions<'_>) -> RpcLaunch {
    let mut filtered = vec![];
    let mut index = 0;
    while index < input.args.len() {
        let arg = &input.args[index];
        if input.disable_extensions && matches!(arg.as_str(), "--extension" | "-e")
            || input.disable_tools
                && matches!(arg.as_str(), "--tools" | "-t" | "--exclude-tools" | "-xt")
        {
            index += 2;
            continue;
        }
        if input.disable_extensions
            && ["--extension=", "-e="]
                .iter()
                .any(|prefix| arg.starts_with(prefix))
            || input.disable_tools
                && (matches!(
                    arg.as_str(),
                    "--no-tools" | "-nt" | "--no-builtin-tools" | "-nbt"
                ) || ["--tools=", "-t=", "--exclude-tools=", "-xt="]
                    .iter()
                    .any(|prefix| arg.starts_with(prefix)))
        {
            index += 1;
            continue;
        }
        filtered.push(arg.clone());
        index += 1;
    }
    let mut args = vec!["--mode".into(), "rpc".into()];
    if input.ephemeral {
        args.push("--no-session".into());
    } else if let Some(path) = input.session_path {
        args.extend(["--session".into(), path.into()]);
    }
    args.extend(filtered);
    if input.short_lived && !args.iter().any(|arg| arg == "--offline") {
        args.push("--offline".into());
    }
    if input.disable_extensions {
        args.push("--no-extensions".into());
    }
    if input.disable_tools {
        args.push("--no-tools".into());
    }
    let extension = input.extension_path.filter(|_| !input.disable_extensions);
    if let Some(extension) = extension {
        let normalized = |path: &str| path.replace('\\', "/").trim_end_matches('/').to_owned();
        let present = args.windows(2).any(|args| {
            matches!(args[0].as_str(), "--extension" | "-e")
                && normalized(&args[1]) == normalized(extension)
        });
        if !present {
            args.extend(["--extension".into(), extension.into()]);
        }
    }
    let mut environment = input.environment.clone();
    for key in [
        "T3_MCP_URL",
        "T3_MCP_BEARER_TOKEN",
        "T3_PI_MCP_EXTENSION_PATH",
    ] {
        environment.remove(key);
    }
    if let Some(extension) = extension {
        environment.insert("T3_PI_MCP_EXTENSION_PATH".into(), extension.into());
        if let Some(mode) = input.runtime_mode {
            environment.insert(
                "T3_PI_RUNTIME_MODE".into(),
                if mode == "auto" {
                    "approval-required"
                } else {
                    mode
                }
                .into(),
            );
        }
        if let Some((endpoint, authorization)) = input.mcp {
            environment.insert("T3_MCP_URL".into(), endpoint.into());
            environment.insert(
                "T3_MCP_BEARER_TOKEN".into(),
                authorization
                    .expose()
                    .strip_prefix("Bearer ")
                    .unwrap_or(authorization.expose())
                    .into(),
            );
        }
    }
    RpcLaunch {
        binary: input.binary.into(),
        args: Sensitive::new(args),
        cwd: input.cwd.into(),
        environment: Sensitive::new(environment),
    }
}
