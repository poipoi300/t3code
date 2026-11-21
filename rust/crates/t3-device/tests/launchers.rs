#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};
use t3_device::{
    agent::{FileLaunchers, Launchers},
    io::{Command, Commands, NativeCommands, Secret},
};

async fn runtime() -> String {
    let out = NativeCommands
        .run(Command::new("node", ["-p", "process.execPath"]))
        .await
        .unwrap();
    assert_eq!(out.code, 0);
    out.stdout.trim().into()
}
#[tokio::test]
async fn bound_launcher_preserves_arguments_overrides_environment_and_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("fake-cli.mjs");
    tokio::fs::write(&entry,"console.log(JSON.stringify({args:process.argv.slice(2),base:process.env.AGENT_DEVICE_DAEMON_BASE_URL,token:process.env.AGENT_DEVICE_DAEMON_AUTH_TOKEN,config:process.env.AGENT_DEVICE_CONFIG}));process.exitCode=23;").await.unwrap();
    let launchers = FileLaunchers {
        state: dir.path().join("paths with spaces"),
        node: runtime().await.into(),
        entry,
        commands: Arc::new(NativeCommands),
        cli_provider: None,
    };
    let args = vec![
        "--platform".into(),
        "ios".into(),
        "--udid".into(),
        "UDID-1".into(),
        "--config".into(),
        "/host.json".into(),
        "--session".into(),
        "t3-a".into(),
    ];
    let launcher = launchers.bind("t3-a", &args).await.unwrap();
    let unbound = launchers.state.join("device/bin").join(if cfg!(windows) {
        "agent-device.cmd"
    } else {
        "agent-device"
    });
    let result = NativeCommands
        .run(Command::new(
            unbound.to_string_lossy(),
            ["help", "snapshot"],
        ))
        .await
        .unwrap();
    assert_eq!(result.code, 23);
    assert_eq!(
        serde_json::from_str::<Value>(&result.stdout).unwrap(),
        json!({"args":["help","snapshot"]})
    );
    for (input, expected) in [
        (
            vec!["snapshot", "-i"],
            [vec!["snapshot".into(), "-i".into()], args.clone()].concat(),
        ),
        (
            vec!["open", "--session=mine"],
            vec![
                "open",
                "--session=mine",
                "--platform",
                "ios",
                "--udid",
                "UDID-1",
                "--config",
                "/host.json",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        ),
        (
            vec![
                "fill",
                "@e5",
                "--",
                "literal text",
                "--help",
                "--session",
                "mine",
            ],
            [
                vec!["fill".into(), "@e5".into()],
                args.clone(),
                vec![
                    "--".into(),
                    "literal text".into(),
                    "--help".into(),
                    "--session".into(),
                    "mine".into(),
                ],
            ]
            .concat(),
        ),
        (
            vec!["help", "gesture"],
            vec!["help".into(), "gesture".into()],
        ),
        (
            vec!["cdp", "target", "list"],
            vec!["cdp".into(), "target".into(), "list".into()],
        ),
        (
            vec!["--config", "/override.json", "connection"],
            vec![
                "--config",
                "/override.json",
                "connection",
                "--session",
                "t3-a",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        ),
    ] {
        let mut command = Command::new(&launcher, input);
        for key in [
            "AGENT_DEVICE_DAEMON_BASE_URL",
            "AGENT_DEVICE_DAEMON_AUTH_TOKEN",
            "AGENT_DEVICE_CONFIG",
        ] {
            command
                .env
                .insert(key.into(), Some(Secret::new("must-not-inherit")));
        }
        let result = NativeCommands.run(command).await.unwrap();
        assert_eq!(result.code, 23, "{}", result.stderr);
        let output: Value = serde_json::from_str(&result.stdout).unwrap();
        assert_eq!(output, json!({"args":expected}));
    }
    for input in [
        vec!["connect"],
        vec!["--config", "/override.json", "auth", "status"],
        vec!["daemon", "stop"],
        vec!["device", "status"],
    ] {
        let result = NativeCommands
            .run(Command::new(&launcher, input))
            .await
            .unwrap();
        assert_eq!(result.code, 1);
        assert!(result.stdout.is_empty());
        assert!(result.stderr.contains("not available"));
    }
    launchers.bind("t3-ended", &args).await.unwrap();
    launchers
        .prune(&BTreeSet::from(["t3-a".into()]))
        .await
        .unwrap();
    assert!(std::path::Path::new(&launcher).exists());
    assert!(!launchers.state.join("device/targets/t3-ended").exists());
}

#[tokio::test]
async fn launcher_close_distinguishes_missing_sessions_from_failed_releases() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("fake-close.mjs");
    let launchers = FileLaunchers {
        state: dir.path().into(),
        node: runtime().await.into(),
        entry: entry.clone(),
        cli_provider: None,
        commands: Arc::new(NativeCommands),
    };
    let launcher = launchers
        .bind(
            "t3-close",
            &[
                "--config".into(),
                "/fixture.json".into(),
                "--session".into(),
                "t3-close".into(),
            ],
        )
        .await
        .unwrap();
    for (code, stderr, success) in [
        (0, "", true),
        (1, "SESSION_NOT_FOUND", true),
        (1, "private-token", false),
    ] {
        tokio::fs::write(
            &entry,
            format!(
                "process.stderr.write({});process.exitCode={code};",
                serde_json::to_string(stderr).unwrap()
            ),
        )
        .await
        .unwrap();
        let result = launchers.release(&launcher).await;
        assert_eq!(result.is_ok(), success);
        if let Err(error) = result {
            assert!(!format!("{error:?}").contains("private-token"));
        }
    }
}
#[tokio::test]
async fn missing_bound_runtime_reports_recovery_guidance() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("fake-cli.mjs");
    tokio::fs::write(&entry, "").await.unwrap();
    let launchers = FileLaunchers {
        state: dir.path().into(),
        node: dir
            .path()
            .join("runtime-gone.exe")
            .to_string_lossy()
            .into_owned()
            .into(),
        entry,
        commands: Arc::new(NativeCommands),
        cli_provider: None,
    };
    let launcher = launchers.bind("t3-a", &[]).await.unwrap();
    let result = NativeCommands
        .run(Command::new(launcher, ["help"]))
        .await
        .unwrap();
    assert_eq!(result.code, 1);
    assert!(
        result.stderr.contains("Call device_open again"),
        "{}",
        result.stderr
    );
}
