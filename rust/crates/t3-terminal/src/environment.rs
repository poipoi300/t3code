use crate::Environment;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Mac,
    Posix,
}
impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Posix
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellCandidate {
    pub shell: String,
    pub args: Vec<String>,
}

pub fn shell_candidates(
    platform: Platform,
    env: &Environment,
    preferred: Option<&str>,
) -> Vec<ShellCandidate> {
    let default = if platform == Platform::Windows {
        "pwsh.exe"
    } else {
        env.get("SHELL").map(String::as_str).unwrap_or("bash")
    };
    let mut commands = vec![preferred.unwrap_or(default).to_owned()];
    if platform == Platform::Windows {
        let root = env
            .get(&env_key(env, "SystemRoot", platform))
            .or_else(|| env.get(&env_key(env, "windir", platform)))
            .map(String::as_str)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("C:\\Windows");
        commands.extend([
            "pwsh.exe".to_owned(),
            format!(
                "{}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
                root.trim_end_matches(['/', '\\'])
            ),
            "powershell.exe".to_owned(),
        ]);
        commands.extend(env.get(&env_key(env, "ComSpec", platform)).cloned());
        commands.extend([
            format!("{}\\System32\\cmd.exe", root.trim_end_matches(['/', '\\'])),
            "cmd.exe".to_owned(),
        ]);
    } else {
        commands.extend(env.get("SHELL").cloned());
        commands
            .extend(["/bin/zsh", "/bin/bash", "/bin/sh", "zsh", "bash", "sh"].map(str::to_owned));
    }
    let mut result = Vec::new();
    for command in commands {
        let shell = if platform == Platform::Windows {
            command.trim()
        } else {
            command
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(['\'', '"'])
        };
        if shell.is_empty() {
            continue;
        }
        let name = shell
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(shell)
            .to_lowercase();
        let args = match (platform, name.as_str()) {
            (Platform::Windows, "pwsh.exe" | "powershell.exe") => vec!["-NoLogo".into()],
            (Platform::Mac | Platform::Posix, "zsh") => vec!["-o".into(), "nopromptsp".into()],
            (Platform::Mac, "bash") => vec!["-l".into()],
            _ => Vec::new(),
        };
        let candidate = ShellCandidate {
            shell: shell.into(),
            args,
        };
        if !result.contains(&candidate) {
            result.push(candidate);
        }
    }
    result
}

pub fn spawn_environment(
    base: &Environment,
    runtime: Option<&Environment>,
    platform: Platform,
    home: &Path,
    version: &str,
    managed_paths: &[String],
) -> Environment {
    let mut env: Environment = base
        .iter()
        .filter(|(key, _)| {
            let upper = key.to_uppercase();
            !upper.starts_with("T3CODE_")
                && !upper.starts_with("VITE_")
                && ![
                    "PORT",
                    "ELECTRON_RENDERER_PORT",
                    "ELECTRON_RUN_AS_NODE",
                    "TMUX",
                    "TMUX_PANE",
                    "STY",
                    "WINDOW",
                    "ZELLIJ",
                    "ZELLIJ_SESSION_NAME",
                    "ZELLIJ_PANE_ID",
                ]
                .contains(&upper.as_str())
                && !(platform == Platform::Windows
                    && ["TERM_PROGRAM", "TERM_PROGRAM_VERSION"].contains(&upper.as_str()))
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    env.insert("TERM_PROGRAM".into(), "t3code".into());
    env.insert("TERM_PROGRAM_VERSION".into(), version.into());
    for (key, value) in runtime.into_iter().flatten() {
        let actual = env_key(&env, key, platform);
        let normalized = if platform == Platform::Windows {
            key.to_uppercase()
        } else {
            key.clone()
        };
        let expanded = if ["CODEX_HOME", "CLAUDE_CONFIG_DIR", "GROK_HOME"]
            .contains(&normalized.as_str())
            && (value == "~" || value.starts_with("~/") || value.starts_with("~\\"))
        {
            if value == "~" {
                home.to_string_lossy().into_owned()
            } else {
                home.join(value[2..].trim_start_matches(['/', '\\']))
                    .to_string_lossy()
                    .into_owned()
            }
        } else {
            value.clone()
        };
        env.insert(actual, expanded);
    }
    if env.get("COLORTERM").is_none_or(String::is_empty)
        && runtime.is_none_or(|r| !r.contains_key("COLORTERM"))
    {
        env.insert("COLORTERM".into(), "truecolor".into());
    }
    let key = env_key(&env, "fish_features", platform);
    let features = env.entry(key).or_default();
    if !features.contains("query-term") {
        if !features.is_empty() {
            features.push(',');
        }
        features.push_str("no-query-term");
    }
    if env.contains_key("APPIMAGE") || env.contains_key("APPDIR") {
        let dir = env
            .get("APPDIR")
            .map(|s| s.trim_end_matches('/').to_owned());
        for key in ["APPIMAGE", "APPDIR", "ARGV0", "OWD"] {
            env.remove(key);
        }
        if let Some(dir) = dir.filter(|s| !s.is_empty()) {
            for key in [
                "PATH",
                "LD_LIBRARY_PATH",
                "XDG_DATA_DIRS",
                "GSETTINGS_SCHEMA_DIR",
            ] {
                if let Some(value) = env.remove(key) {
                    let kept = value
                        .split(':')
                        .filter(|s| {
                            !s.is_empty() && *s != dir && !s.starts_with(&format!("{dir}/"))
                        })
                        .collect::<Vec<_>>()
                        .join(":");
                    if !kept.is_empty() {
                        env.insert(key.into(), kept);
                    }
                }
            }
        }
    }
    let key = env_key(&env, "PATH", platform);
    let delimiter = if platform == Platform::Windows {
        ';'
    } else {
        ':'
    };
    let mut paths: Vec<String> = Vec::new();
    if !managed_paths.is_empty() {
        for path in env
            .get(&key)
            .into_iter()
            .flat_map(|s| s.split(delimiter))
            .chain(managed_paths.iter().map(String::as_str))
        {
            let path = path.trim();
            if path.is_empty() {
                continue;
            }
            if !paths.iter().any(|p| {
                if platform == Platform::Windows {
                    p.eq_ignore_ascii_case(path)
                } else {
                    p == path
                }
            }) {
                paths.push(path.to_owned());
            }
        }
    }
    if !managed_paths.is_empty() {
        env.insert(key, paths.join(&delimiter.to_string()));
    }
    if platform == Platform::Windows {
        env.entry("TERM".into())
            .or_insert_with(|| "xterm-256color".into());
    } else {
        env.insert("TERM".into(), "xterm-256color".into());
    }
    env
}
fn env_key(env: &Environment, key: &str, platform: Platform) -> String {
    if platform == Platform::Windows {
        env.keys()
            .find(|k| k.eq_ignore_ascii_case(key))
            .cloned()
            .unwrap_or_else(|| key.into())
    } else {
        key.into()
    }
}
