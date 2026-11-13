//! Pure launch description; executable probing, containment and process I/O are host work.
use std::{collections::BTreeMap, path::PathBuf};

pub type Environment = BTreeMap<String, String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Posix,
}

#[derive(Clone, Debug)]
pub struct LaunchInput {
    pub platform: Platform,
    pub binary_path: String,
    /// Result of the process host's PATH/PATHEXT lookup, if any.
    pub resolved_executable: Option<String>,
    pub home_directory: String,
    pub effective_codex_home: Option<String>,
    pub launch_args: Option<String>,
    pub host_environment: Environment,
    /// Ordered patches: a later variable wins, including Windows case aliases.
    pub instance_environment: Vec<(String, String)>,
    /// Live shared processes leave this absent. Probes may supply their cwd.
    pub cwd: Option<PathBuf>,
    pub extend_env: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchSpec {
    pub command: String,
    pub args: Vec<String>,
    pub environment: Environment,
    pub cwd: Option<PathBuf>,
    pub extend_env: Option<bool>,
    /// Node-compatible shell command for Windows batch shims; host must use cmd.exe.
    pub shell: bool,
    pub force_kill_after: std::time::Duration,
    pub containment_name: &'static str,
    pub shared: bool,
}

pub fn app_server_launch(input: LaunchInput) -> LaunchSpec {
    let windows = input.platform == Platform::Windows;
    let mut environment = input.host_environment;
    for (key, value) in input.instance_environment {
        let key = if windows { key.to_uppercase() } else { key };
        if windows {
            environment.retain(|old, _| !old.eq_ignore_ascii_case(&key));
        }
        let value = if matches!(
            key.as_str(),
            "CODEX_HOME" | "CLAUDE_CONFIG_DIR" | "GROK_HOME"
        ) {
            expand_home(&value, &input.home_directory, input.platform)
        } else {
            value
        };
        environment.insert(key, value);
    }
    // This is the TS object spread order: an instance's explicit originator wins.
    environment
        .entry("CODEX_INTERNAL_ORIGINATOR_OVERRIDE".into())
        .or_insert_with(|| "T3 Code".into());
    if let Some(home) = input.effective_codex_home.filter(|home| !home.is_empty()) {
        if windows {
            environment.retain(|key, _| !key.eq_ignore_ascii_case("CODEX_HOME"));
        }
        environment.insert(
            "CODEX_HOME".into(),
            expand_home(&home, &input.home_directory, input.platform),
        );
    }
    let configured = environment
        .get("T3CODE_CODEX_LAUNCH_ARGS")
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .or(input.launch_args.as_deref())
        .unwrap_or("")
        .trim();
    let mut args = vec![
        "app-server".into(),
        "-c".into(),
        "thread_unload_delay_secs=0".into(),
    ];
    args.extend(tokenize_cli_args(configured));
    let binary = if input.binary_path.is_empty() {
        "codex"
    } else {
        &input.binary_path
    };
    let command = input
        .resolved_executable
        .unwrap_or_else(|| expand_home(binary, &input.home_directory, input.platform));
    let shell = windows
        && (command.to_ascii_lowercase().ends_with(".cmd")
            || command.to_ascii_lowercase().ends_with(".bat"));
    let (command, args) = if shell {
        (
            std::iter::once(command.as_str())
                .chain(args.iter().map(String::as_str))
                .map(escape_windows_shell_arg)
                .collect::<Vec<_>>()
                .join(" "),
            vec![],
        )
    } else {
        (command, args)
    };
    LaunchSpec {
        command,
        args,
        environment,
        cwd: input.cwd,
        extend_env: input.extend_env,
        shell,
        force_kill_after: std::time::Duration::from_secs(2),
        containment_name: "codex",
        shared: true,
    }
}

fn expand_home(value: &str, home: &str, platform: Platform) -> String {
    if value == "~" {
        return home.into();
    }
    if let Some(rest) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        return match platform {
            Platform::Windows => format!(
                "{}\\{}",
                home.trim_end_matches(['/', '\\']),
                rest.replace('/', "\\")
            ),
            Platform::Posix => format!("{}/{}", home.trim_end_matches('/'), rest),
        };
    }
    value.into()
}

/// Matches shared/cliArgs.ts, including empty and unterminated quoted arguments.
pub fn tokenize_cli_args(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut quoted = false;
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if let Some(delimiter) = quote {
            if ch == delimiter {
                quote = None;
                quoted = true;
            } else if ch == '\\'
                && delimiter == '"'
                && chars
                    .peek()
                    .is_some_and(|next| matches!(next, '"' | '\\' | '$' | '`'))
            {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            } else {
                current.push(ch);
            }
        } else if matches!(ch, '\'' | '"') {
            quote = Some(ch);
            quoted = true;
        } else if ch.is_whitespace() || ch == '\u{feff}' {
            if !current.is_empty() || quoted {
                tokens.push(std::mem::take(&mut current));
                quoted = false;
            }
        } else if ch == '\\'
            && chars
                .peek()
                .is_some_and(|next| next.is_whitespace() || matches!(next, '\'' | '"' | '\u{feff}'))
        {
            if let Some(next) = chars.next() {
                current.push(next);
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() || quoted {
        tokens.push(current);
    }
    tokens
}

fn escape_windows_shell_arg(arg: &str) -> String {
    let mut escaped = String::from("\"");
    let mut slashes = 0;
    for ch in arg.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        escaped.extend(std::iter::repeat_n(
            '\\',
            if ch == '"' { slashes * 2 + 1 } else { slashes },
        ));
        slashes = 0;
        escaped.push(ch);
    }
    escaped.extend(std::iter::repeat_n('\\', slashes * 2));
    escaped.push('"');
    let mut result = String::new();
    for ch in escaped.chars() {
        if "()[]%!^\"`<>&|;, *?".contains(ch) {
            result.push('^');
        }
        result.push(ch);
    }
    result
}
