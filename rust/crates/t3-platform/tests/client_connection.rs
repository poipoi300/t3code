#![cfg(all(not(target_arch = "wasm32"), feature = "integration-server"))]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use futures::{
    FutureExt, StreamExt,
    future::{Either, select},
};
use std::{process::Stdio, rc::Rc};
use t3_client::{
    authorization::Sensitive,
    connection::ConnectionStores,
    connection::{ServerCredential, connect},
    platform::{Storage, StorageArea},
};
use t3_platform::{EphemeralSecretStore, NativeConfig, NativePlatform};
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::test(flavor = "current_thread")]
async fn real_server_pairing_rpc_shell_snapshot_reconnect_and_bearer_restore_use_the_public_entry_point()
 {
    let home = tempfile::tempdir().unwrap();
    let client_home = tempfile::tempdir().unwrap();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_t3-client-test-server"));
    command
        .args(["serve", "--base-dir"])
        .arg(home.path())
        .args(["--host", "127.0.0.1", "--port", "0"])
        .current_dir(home.path())
        .env_clear()
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("TEMP", home.path())
        .env("TMP", home.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    for name in ["SystemRoot", "SystemDrive", "windir"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let mut child = command.spawn().unwrap();
    let pid = child.id().unwrap();
    println!("started isolated server PID {pid}");
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let (mut origin, mut pairing) = (None, None);
    while pairing.is_none() {
        let line = lines
            .next_line()
            .await
            .unwrap()
            .expect("server exited before pairing startup");
        if let Some(value) = line.strip_prefix("Connection string: ") {
            origin = Some(value.to_owned());
        }
        if let Some(value) = line.strip_prefix("Pairing URL: ") {
            pairing = Some(value.to_owned());
        }
    }
    let origin = origin.unwrap();
    println!("server startup ready; opening native platform");
    let pairing = url::Url::parse(&pairing.unwrap()).unwrap();
    let token = url::form_urlencoded::parse(pairing.fragment().unwrap().as_bytes())
        .find(|(key, _)| key == "token")
        .unwrap()
        .1
        .into_owned();
    let platform = NativePlatform::open(NativeConfig::new(
        client_home.path(),
        Box::new(EphemeralSecretStore::default()),
    ))
    .await
    .unwrap();
    println!("native platform ready; connecting through pairing entry point");
    let (client, mut driver) = connect(
        platform.clone(),
        &origin,
        ServerCredential::PairingToken(Sensitive(token)),
    )
    .await
    .unwrap();
    println!("pairing connection ready: RPC config received");
    let environment = client.environment().clone();
    let snapshot = client.store().snapshot();
    let state = snapshot.environments.get(&environment).unwrap();
    assert_eq!(
        state.config.as_ref().unwrap().environment.environment_id,
        environment
    );
    let old_session = state.session;
    let endpoint = client.session().unwrap();
    match select(endpoint.probe().boxed_local(), driver.as_mut()).await {
        Either::Left((result, _)) => result.unwrap(),
        Either::Right(_) => panic!("connection driver ended before server probe"),
    }
    println!("server probe succeeded through the guarded RPC session");
    let mut shell_changes = client.store().subscribe();
    let shell_ready = async {
        loop {
            let snapshot = client.store().snapshot();
            let state = snapshot.environments.get(&environment).unwrap();
            if state.shell.snapshot.is_some() {
                return Ok(());
            }
            if let Some(error) = &state.shell.error {
                return Err(error.clone());
            }
            shell_changes
                .next()
                .await
                .expect("store closed before shell snapshot");
        }
    }
    .boxed_local();
    let shell_result = match select(shell_ready, driver.as_mut()).await {
        Either::Left((result, _)) => result,
        Either::Right(_) => Err("connection driver ended before shell snapshot".into()),
    };
    let mut changes = client.store().subscribe();
    client.reconnect().unwrap();
    let reconnected = async {
        loop {
            let snapshot = client.store().snapshot();
            let state = snapshot.environments.get(&environment).unwrap();
            if state.session.0 > old_session.0
                && state
                    .connection
                    .as_ref()
                    .is_some_and(|state| state.phase == t3_client::connection::Phase::Connected)
                && client.session().is_some()
            {
                break;
            }
            changes
                .next()
                .await
                .expect("store subscription closed during reconnect");
        }
    }
    .boxed_local();
    assert!(matches!(
        select(reconnected, driver.as_mut()).await,
        Either::Left(((), _))
    ));
    assert!(endpoint.probe().now_or_never().unwrap().is_err());
    let replacement = client.session().unwrap();
    match select(replacement.probe().boxed_local(), driver.as_mut()).await {
        Either::Left((result, _)) => result.unwrap(),
        Either::Right(_) => panic!("connection driver ended before replacement probe"),
    }
    println!("reconnect succeeded with a new guarded session; the old session is invalid");
    client.close().unwrap();
    driver.await.unwrap();
    let records = platform
        .list(StorageArea::Secure, "connection-credential".into())
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    let credential = ConnectionStores::new(Rc::new(platform.clone()))
        .credential(&environment, &records[0].record)
        .await
        .unwrap()
        .unwrap();
    let (restored, mut driver) = connect(
        platform.clone(),
        &origin,
        ServerCredential::Bearer(credential.token().clone()),
    )
    .await
    .unwrap();
    let snapshot = restored.store().snapshot();
    let state = snapshot.environments.get(&environment).unwrap();
    assert!(state.config.is_some());
    println!(
        "server shell synchronization status: {:?}",
        state.shell.status
    );
    let restored_endpoint = restored.session().unwrap();
    match select(restored_endpoint.probe().boxed_local(), driver.as_mut()).await {
        Either::Left((result, _)) => result.unwrap(),
        Either::Right(_) => panic!("restored driver ended before probe"),
    }
    println!("saved bearer restoration and guarded RPC probe succeeded");
    restored.close().unwrap();
    driver.await.unwrap();
    let composed = tokio::task::LocalSet::new()
        .run_until(composition_root_follows_a_thread(platform, environment.clone()))
        .await;
    assert_eq!(child.id(), Some(pid));
    child.kill().await.unwrap();
    println!(
        "real client: config + RPC probe, reconnect, bearer restore; server PID {pid} stopped"
    );
    assert!(
        shell_result.is_ok(),
        "real server supplied config and a working RPC session but no shell snapshot: {:?}; server PID {pid} stopped",
        shell_result.err()
    );
    if let Err(step) = composed {
        panic!("composition root stopped at {step}; server PID {pid} stopped");
    }
}

/// Waits up to 30 seconds for `ready` to answer, re-checking on every store change.
async fn within<T>(
    store: &t3_client::ClientStore,
    step: &str,
    mut ready: impl FnMut() -> Option<T>,
) -> Result<T, String> {
    let mut changes = store.subscribe();
    let wait = async {
        loop {
            if let Some(value) = ready() {
                return Ok(value);
            }
            if changes.next().await.is_none() {
                return Err(format!("{step}: the store closed"));
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(30), wait)
        .await
        .unwrap_or_else(|_| Err(format!("{step}: nothing arrived within 30 s")))
}

/// Through `t3_client::composition::ClientRuntime` alone: restore the saved environment, load the
/// shell, create a thread over the guarded session, open it and receive its timeline.
async fn composition_root_follows_a_thread(
    platform: NativePlatform,
    environment: t3_client::types::EnvironmentId,
) -> Result<(), String> {
    use t3_client::{
        composition::ClientRuntime,
        connection::Phase,
        types::{SyncStatus, ThreadId, ThreadKey},
    };
    let runtime = ClientRuntime::new(
        platform,
        t3_client::ClientStore::default(),
        t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web,
        Rc::new(|future| {
            tokio::task::spawn_local(future);
        }),
    );
    runtime
        .restore()
        .await
        .map_err(|error| format!("restore: {error}"))?;
    let store = runtime.store().clone();
    within(&store, "restore (connected)", || {
        runtime
            .environments()
            .iter()
            .any(|entry| entry.id == environment && entry.state.phase == Phase::Connected)
            .then_some(())
    })
    .await?;
    println!("composition root: saved environment restored and connected");
    within(&store, "shell snapshot", || {
        store.shared_shell(&environment).map(|_| ())
    })
    .await?;
    println!("composition root: shell snapshot loaded");
    let session = runtime
        .client(&environment)
        .and_then(|client| client.session())
        .ok_or("thread creation: no session")?;
    use t3_client::rpc::RpcEndpoint;
    let project = session
        .call::<t3_contracts::rpc::WsProjectsEnsureScratchRpc>(t3_contracts::rpc::Empty {})
        .await
        .map_err(|error| format!("thread creation (projects.ensureScratch): {error}"))?
        .project_id;
    let thread = ThreadId("composition-root-proof".into());
    let command: t3_contracts::orchestration_v2::OrchestrationV2Command =
        serde_json::from_value(serde_json::json!({
            "type":"thread.create","createdBy":"user","creationSource":"web",
            "commandId":"composition-root-proof-create","threadId":thread.0,"projectId":project.0,
            "title":"Composition root proof","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},
            "runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null
        }))
        .map_err(|error| format!("thread creation (command): {error}"))?;
    session
        .call::<t3_contracts::rpc::WsOrchestrationV2DispatchCommandRpc>(command)
        .await
        .map_err(|error| format!("thread creation (orchestration.dispatchCommand): {error}"))?;
    println!("composition root: thread created over the guarded session");
    let key = ThreadKey {
        environment: environment.clone(),
        thread,
    };
    runtime.open_thread(&key);
    let rows = within(&store, "thread timeline", || {
        let snapshot = store.snapshot();
        let state = t3_client::state::selectors::thread(&snapshot, &key)?;
        (state.status == SyncStatus::Live)
            .then(|| store.shared_thread(&key))
            .flatten()
            .map(|projection| projection.visible_turn_items.len())
    })
    .await?;
    println!("composition root: thread timeline live with {rows} rows");
    runtime.close_thread(&key);
    runtime
        .remove(&environment)
        .await
        .map_err(|error| format!("removal: {error}"))?;
    Ok(())
}
