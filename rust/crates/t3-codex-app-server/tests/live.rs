#![allow(clippy::unwrap_used)]
use std::{path::PathBuf, process::Stdio, time::Duration};
use t3_codex_app_server::{Client, InitializeParams, ModelListParams};

#[tokio::test]
#[ignore = "Opt-in: installed Codex handshake and model listing only; no turns or tokens"]
async fn installed_codex_handshake_and_model_list() {
    let binary = std::env::var_os("CODEX_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .map(|path| path.join("Programs/OpenAI/Codex/bin/codex.exe"))
                .filter(|path| path.is_file())
                .unwrap_or_else(|| PathBuf::from("codex"))
        });
    let home = tempfile::tempdir().unwrap();
    let mut child = tokio::process::Command::new(binary)
        .args(["app-server", "-c", "thread_unload_delay_secs=0"])
        .env("CODEX_HOME", home.path())
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "T3 Code")
        .env_remove("OPENAI_API_KEY")
        .env_remove("ACCESS_TOKEN")
        .env_remove("T3CODE_CODEX_LAUNCH_ARGS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let drain =
        tokio::spawn(async move { tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await });
    let (client, mut events) = Client::connect(
        child.stdout.take().unwrap(),
        child.stdin.take().unwrap(),
        Default::default(),
    );
    let notifications = tokio::spawn(async move { while events.recv().await.is_some() {} });
    let probe = tokio::time::timeout(Duration::from_secs(30), async {
        let initialized = client
            .initialize(&InitializeParams::probe(env!("CARGO_PKG_VERSION")))
            .await
            .unwrap()
            .unwrap();
        assert!(!initialized.user_agent.is_empty());
        client.initialized().await.unwrap();
        let models = client
            .model_list(&ModelListParams::default())
            .await
            .unwrap();
        assert!(!models.data.is_empty());
        eprintln!(
            "Installed Codex: {}; platform={}/{}; models={}",
            initialized.user_agent,
            initialized.platform_family,
            initialized.platform_os,
            models.data.len()
        );
        client.shutdown().await.unwrap();
    })
    .await;
    if probe.is_err() {
        let _ = child.start_kill();
    }
    let exit = match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
        Ok(exit) => exit.unwrap(),
        Err(_) => {
            child.start_kill().unwrap();
            child.wait().await.unwrap()
        }
    };
    drop(client);
    notifications.await.unwrap();
    drain.await.unwrap().unwrap();
    probe.expect("Codex probe timed out");
    assert!(exit.success(), "Codex exited: {exit}");
}
