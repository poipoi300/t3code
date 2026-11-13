#![allow(clippy::unwrap_used)]
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};
use t3_claude_cli::{
    Event, Session, SessionOptions,
    control::Initialize,
    launch::{
        Environment, LaunchOptions, Settings, SystemPrompt, describe_spawn, resolve_executable,
    },
    message::{Message, SystemMessage},
};

fn cli_on_path() -> Option<String> {
    let platform = if cfg!(windows) {
        "win32"
    } else {
        std::env::consts::OS
    };
    for directory in std::env::split_paths(&std::env::var_os("PATH")?) {
        for name in if cfg!(windows) {
            vec!["claude.exe", "claude.cmd"]
        } else {
            vec!["claude"]
        } {
            let path = directory.join(name);
            if !path.is_file() {
                continue;
            }
            let existing = [
                directory.join("node_modules/@anthropic-ai/claude-code/bin/claude.exe"),
                directory.join("node_modules/@anthropic-ai/claude-code/cli.js"),
            ]
            .into_iter()
            .filter(|p| p.is_file())
            .map(|p| p.to_string_lossy().replace('/', "\\"))
            .collect::<Vec<_>>();
            let path = path.to_string_lossy().into_owned();
            let resolved = resolve_executable(&path, platform, Some(&path), |p| {
                existing.iter().any(|file| file == p)
            });
            if !resolved.to_ascii_lowercase().ends_with(".cmd") {
                return Some(resolved);
            }
        }
    }
    None
}

struct ProbeDirectory(PathBuf);
impl Drop for ProbeDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Explicit opt-in: cargo test -p t3-claude-cli --test cli_handshake -- --ignored
#[tokio::test]
#[ignore = "starts an isolated real CLI for initialize only; no prompt or model request"]
async fn isolated_cli_initialization_without_a_prompt() {
    let Some(cli) = cli_on_path() else {
        eprintln!("claude is not on PATH; handshake skipped");
        return;
    };
    let directory = ProbeDirectory(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".cli-probes")
            .join(uuid::Uuid::new_v4().to_string()),
    );
    std::fs::create_dir_all(&directory.0).unwrap();
    let mut env = Environment::new();
    for key in [
        "PATH",
        "Path",
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
        "TEMP",
        "TMP",
    ] {
        if let Ok(value) = std::env::var(key) {
            env.insert(key.into(), value);
        }
    }
    let isolated = directory.0.to_string_lossy().into_owned();
    for key in [
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "XDG_CONFIG_HOME",
        "CLAUDE_CONFIG_DIR",
        "CLAUDE_SECURESTORAGE_CONFIG_DIR",
    ] {
        env.insert(key.into(), isolated.clone());
    }
    for (key, value) in [
        ("ENABLE_CLAUDEAI_MCP_SERVERS", "false"),
        ("CLAUDE_CODE_AUTO_CONNECT_IDE", "0"),
        ("CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL", "1"),
    ] {
        env.insert(key.into(), value.into());
    }
    let spawn = describe_spawn(&LaunchOptions {
        cli_path: Some(cli),
        cwd: Some(directory.0.clone()),
        env,
        persist_session: false,
        setting_sources: Some(vec![]),
        tools: Some(t3_claude_cli::launch::Tools::List(vec![])),
        settings: Some(Settings::Json(serde_json::json!({"disableAllHooks":true}))),
        system_prompt: SystemPrompt::Unspecified,
        ..Default::default()
    })
    .unwrap();
    let mut child = tokio::process::Command::new(&spawn.command)
        .args(&spawn.args)
        .env_clear()
        .envs(&spawn.env)
        .current_dir(&directory.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut session = Session::connect(
        child.stdout.take().unwrap(),
        child.stdin.take().unwrap(),
        SessionOptions::default(),
    );
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let initialized = session.client().initialize(Initialize::default()).await?;
        assert!(
            !initialized.get().models.is_empty() || initialized.get().claude_code_version.is_some()
        );
        while let Some(event) = session.next_event().await {
            if let Event::Message(message) = event?
                && let Message::System(system) = *message
                && matches!(system.get(), SystemMessage::Init(_))
            {
                return Ok::<_, t3_claude_cli::Error>(());
            }
        }
        Err(t3_claude_cli::Error::Closed)
    })
    .await;
    session.shutdown().await.unwrap();
    if child.try_wait().unwrap().is_none() {
        child.kill().await.unwrap();
    }
    let _ = child.wait().await;
    result
        .expect("initialize/init deadline exceeded")
        .expect("isolated initialize failed");
}
