//! Creates a thread on a running Rust server through the client's guarded session: the two RPC
//! calls `t3-platform/tests/client_connection.rs` makes (`projects.ensureScratch`, then
//! `orchestration.dispatchCommand` with `thread.create`). For UI proofs while the UI has no
//! new-thread flow. On a dev-mode server, `devReplay` / `showcase` gives a free replayed turn.
//!
//! cargo run --example seed_thread -- <origin> <pairing-token> <scratch-data-dir> <thread-id> <title> <instance> <model>

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [origin, token, dir, thread, title, instance, model] = args.as_slice() else {
        return Err("usage: seed_thread <origin> <pairing-token> <scratch-data-dir> <thread-id> <title> <instance> <model>".into());
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    tokio::task::LocalSet::new().block_on(
        &runtime,
        seed(origin, token, dir, thread, title, instance, model),
    )
}

#[cfg(not(target_arch = "wasm32"))]
async fn seed(
    origin: &str,
    token: &str,
    dir: &str,
    thread: &str,
    title: &str,
    instance: &str,
    model: &str,
) -> Result<(), String> {
    use t3_client::authorization::Sensitive;
    use t3_client::connection::{ServerCredential, connect};
    use t3_client::rpc::RpcEndpoint;
    use t3_contracts::orchestration_v2::OrchestrationV2Command;
    use t3_contracts::rpc::{
        Empty, WsOrchestrationV2DispatchCommandRpc, WsProjectsEnsureScratchRpc,
    };
    use t3_platform::{EphemeralSecretStore, NativeConfig, NativePlatform};

    let platform = NativePlatform::open(NativeConfig::new(
        dir,
        Box::new(EphemeralSecretStore::default()),
    ))
    .await
    .map_err(|error| format!("platform: {error}"))?;
    let (client, driver) = connect(
        platform,
        origin,
        ServerCredential::PairingToken(Sensitive(token.to_owned())),
    )
    .await
    .map_err(|error| format!("pairing: {error}"))?;
    let driving = tokio::task::spawn_local(async move {
        let _ = driver.await;
    });
    let session = client.session().ok_or("no session after pairing")?;
    let project = session
        .call::<WsProjectsEnsureScratchRpc>(Empty {})
        .await
        .map_err(|error| format!("projects.ensureScratch: {error}"))?
        .project_id;
    let command: OrchestrationV2Command = serde_json::from_value(serde_json::json!({
        "type": "thread.create", "createdBy": "user", "creationSource": "web",
        "commandId": format!("{thread}-create"), "threadId": thread, "projectId": project.0,
        "title": title, "modelSelection": {"instanceId": instance, "model": model},
        "runtimeMode": "full-access", "interactionMode": "default",
        "branch": null, "worktreePath": null
    }))
    .map_err(|error| format!("thread.create command: {error}"))?;
    session
        .call::<WsOrchestrationV2DispatchCommandRpc>(command)
        .await
        .map_err(|error| format!("orchestration.dispatchCommand: {error}"))?;
    println!(
        "created thread {thread} ({title}, {instance}/{model}) in project {}",
        project.0
    );
    client.close().map_err(|error| format!("close: {error}"))?;
    let _ = driving.await;
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn main() {}
