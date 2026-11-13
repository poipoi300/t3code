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
        platform,
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
}
