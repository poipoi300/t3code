use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};
use t3_contracts::provider_instance::{
    ProviderInstanceEnvironment, ResolvedProviderInstanceEnvironment,
};

pub type Environment = BTreeMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Macos,
    Linux,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::Linux
        }
    }
}

pub fn expand_home_path(value: &str, home: &Path) -> String {
    if value == "~" {
        return home.to_string_lossy().into_owned();
    }
    if value.starts_with("~/") || value.starts_with("~\\") {
        return home.join(&value[2..]).to_string_lossy().into_owned();
    }
    value.to_owned()
}

pub fn merge_environment(
    configured: &ResolvedProviderInstanceEnvironment,
    base: &Environment,
    home: &Path,
    platform: Platform,
) -> Environment {
    let mut next = base.clone();
    for variable in configured {
        let name = if platform == Platform::Windows {
            variable.name.to_uppercase()
        } else {
            variable.name.clone()
        };
        if platform == Platform::Windows {
            next.retain(|key, _| !key.eq_ignore_ascii_case(&name));
        }
        let value = if matches!(
            name.as_str(),
            "CODEX_HOME" | "CLAUDE_CONFIG_DIR" | "GROK_HOME"
        ) {
            expand_home_path(&variable.value, home)
        } else {
            variable.value.clone()
        };
        next.insert(name, value);
    }
    next
}

/// This low-level merge skips unresolved references; enabled instances must resolve them first.
pub fn merge_stored_environment(
    configured: &ProviderInstanceEnvironment,
    base: &Environment,
    home: &Path,
    platform: Platform,
) -> Environment {
    let resolved = configured
        .iter()
        .filter(|v| v.secret.is_none())
        .map(
            |v| t3_contracts::provider_instance::ResolvedProviderInstanceEnvironmentVariable {
                name: v.name.clone(),
                value: v.value.clone(),
                sensitive: v.sensitive,
                value_redacted: v.value_redacted,
            },
        )
        .collect();
    merge_environment(&resolved, base, home, platform)
}

pub fn probe_environment(base: &Environment) -> Environment {
    let mut next = base.clone();
    next.insert("MISE_OFFLINE".into(), "1".into());
    next
}

pub fn claude_process_environment(
    base: &Environment,
    configured: &ResolvedProviderInstanceEnvironment,
    home: &Path,
    platform: Platform,
    settings_entrypoint: Option<&str>,
) -> Environment {
    let mut next = base.clone();
    for name in [
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
        next.remove(name);
    }
    next.insert("CLAUDE_CODE_ENTRYPOINT".into(), "t3-code".into());
    let mut next = merge_environment(configured, &next, home, platform);
    if let Some(entrypoint) = settings_entrypoint.filter(|value| !value.is_empty()) {
        next.insert("CLAUDE_CODE_ENTRYPOINT".into(), entrypoint.to_owned());
    }
    next
}

pub fn absolute_path(path: &Path, cwd: &Path) -> PathBuf {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexHomeLayout {
    pub shared_home: PathBuf,
    pub effective_home: Option<PathBuf>,
    pub auth_overlay: bool,
    pub continuation_key: String,
}

pub fn codex_home_layout(
    home_path: &str,
    shadow_home_path: &str,
    home: &Path,
    cwd: &Path,
) -> CodexHomeLayout {
    let shared = if home_path.trim().is_empty() {
        home.join(".codex")
    } else {
        PathBuf::from(expand_home_path(home_path, home))
    };
    let shared_home = absolute_path(&shared, cwd);
    let auth_overlay = !shadow_home_path.trim().is_empty();
    let effective_home = if auth_overlay {
        Some(absolute_path(
            Path::new(&expand_home_path(shadow_home_path.trim(), home)),
            cwd,
        ))
    } else if home_path.trim().is_empty() {
        None
    } else {
        Some(shared_home.clone())
    };
    let continuation_key = format!("codex:home:{}", shared_home.display());
    CodexHomeLayout {
        shared_home,
        effective_home,
        auth_overlay,
        continuation_key,
    }
}

pub fn claude_home(home_path: &str, env: &Environment, home: &Path, cwd: &Path) -> PathBuf {
    if !home_path.trim().is_empty() {
        return absolute_path(Path::new(&expand_home_path(home_path.trim(), home)), cwd);
    }
    match env
        .get("CLAUDE_CONFIG_DIR")
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    {
        Some(inherited) => absolute_path(Path::new(inherited), cwd),
        None => absolute_path(&home.join(".claude"), cwd),
    }
}

pub fn process_options(
    program: impl Into<std::ffi::OsString>,
    args: &[String],
    env: &Environment,
) -> t3_process::ProcessOptions {
    let mut options = t3_process::ProcessOptions::new(program).args(args);
    options.inherit_env = false;
    options.env = env
        .iter()
        .map(|(k, v)| (k.into(), Some(v.into())))
        .collect();
    options.termination_grace = std::time::Duration::from_secs(1);
    options
}
