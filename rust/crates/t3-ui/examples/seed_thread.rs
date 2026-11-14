//! Creates threads (and projects) on a running Rust server through the client's guarded session:
//! `projects.ensureScratch` or `projects.mutate` (`project.create`), then
//! `orchestration.dispatchCommand` with `thread.create`. For UI proofs that need seeded data. On a
//! dev-mode server, `devReplay` / `showcase` gives a free replayed turn.
//!
//! cargo run --example seed_thread -- <origin> <pairing-token> <scratch-data-dir> <thread-id> <title> <instance> <model> [runtime-mode, default full-access]
//! cargo run --example seed_thread -- <origin> <pairing-token> <scratch-data-dir> <spec.json>
//!
//! A spec seeds several from one pairing:
//! `{"projects": [{"id", "title", "root", "scripts": [ProjectScript]}],
//!   "threads": [{"id", "title", "project" (a spec project id, or absent for the scratch project),
//!                "instance", "model", "mode", "settle"}]}`; `{"id", "settleOnly": true}` settles an
//!   existing thread.

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: seed_thread <origin> <pairing-token> <scratch-data-dir> (<thread-id> <title> <instance> <model> [runtime-mode] | <spec.json>)";
    let [origin, token, dir, rest @ ..] = args.as_slice() else {
        return Err(usage.into());
    };
    let spec = match rest {
        [path] => {
            let text = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
            serde_json::from_str(&text).map_err(|error| format!("{path}: {error}"))?
        }
        [thread, title, instance, model, mode @ ..] if mode.len() <= 1 => serde_json::json!({
            "threads": [{
                "id": thread, "title": title, "instance": instance, "model": model,
                "mode": mode.first().map_or("full-access", String::as_str)
            }]
        }),
        _ => return Err(usage.into()),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    tokio::task::LocalSet::new().block_on(&runtime, seed(origin, token, dir, spec))
}

#[cfg(not(target_arch = "wasm32"))]
async fn seed(origin: &str, token: &str, dir: &str, spec: serde_json::Value) -> Result<(), String> {
    use std::collections::HashMap;

    use serde_json::json;
    use t3_client::authorization::Sensitive;
    use t3_client::connection::{ServerCredential, connect};
    use t3_client::rpc::RpcEndpoint;
    use t3_contracts::orchestration_v2::OrchestrationV2Command;
    use t3_contracts::project::ProjectMutation;
    use t3_contracts::rpc::{
        Empty, WsOrchestrationV2DispatchCommandRpc, WsProjectsEnsureScratchRpc,
        WsProjectsMutateRpc,
    };
    use t3_platform::{EphemeralSecretStore, NativeConfig, NativePlatform};

    let text = |value: &serde_json::Value, key: &str| -> Result<String, String> {
        value[key].as_str().map(str::to_owned).ok_or(format!("spec: missing \"{key}\" in {value}"))
    };
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

    let mut projects = HashMap::new();
    for project in spec["projects"].as_array().into_iter().flatten() {
        let id = text(project, "id")?;
        let mutation: ProjectMutation = serde_json::from_value(json!({
            "type": "project.create", "commandId": format!("{id}-create"), "projectId": id,
            "title": text(project, "title")?, "workspaceRoot": text(project, "root")?,
            "createWorkspaceRootIfMissing": false,
            "scripts": project.get("scripts").cloned().unwrap_or(json!([])),
        }))
        .map_err(|error| format!("project.create mutation: {error}"))?;
        let created = session
            .call::<WsProjectsMutateRpc>(mutation)
            .await
            .map_err(|error| format!("projects.mutate: {error}"))?;
        println!("created project {} ({})", created.id.0, created.title.0);
        projects.insert(id, created.id.0);
    }
    let mut scratch: Option<String> = None;
    for thread in spec["threads"].as_array().into_iter().flatten() {
        let id = text(thread, "id")?;
        if thread["settleOnly"].as_bool() == Some(true) {
            settle(&session, &id).await?;
            continue;
        }
        let title = text(thread, "title")?;
        let project = match thread["project"].as_str() {
            // A key not created by this spec is an existing project id.
            Some(key) => projects.get(key).cloned().unwrap_or_else(|| key.to_owned()),
            None => match &scratch {
                Some(project) => project.clone(),
                None => {
                    let project = session
                        .call::<WsProjectsEnsureScratchRpc>(Empty {})
                        .await
                        .map_err(|error| format!("projects.ensureScratch: {error}"))?
                        .project_id
                        .0;
                    scratch = Some(project.clone());
                    project
                }
            },
        };
        let (instance, model) = (text(thread, "instance")?, text(thread, "model")?);
        let mode = thread["mode"].as_str().unwrap_or("full-access");
        let command: OrchestrationV2Command = serde_json::from_value(json!({
            "type": "thread.create", "createdBy": "user", "creationSource": "web",
            "commandId": format!("{id}-create"), "threadId": id, "projectId": project,
            "title": title, "modelSelection": {"instanceId": instance, "model": model},
            "runtimeMode": mode, "interactionMode": "default",
            "branch": null, "worktreePath": null
        }))
        .map_err(|error| format!("thread.create command: {error}"))?;
        session
            .call::<WsOrchestrationV2DispatchCommandRpc>(command)
            .await
            .map_err(|error| format!("orchestration.dispatchCommand: {error}"))?;
        println!("created thread {id} ({title}, {instance}/{model}, {mode}) in project {project}");
        if thread["settle"].as_bool() == Some(true) {
            settle(&session, &id).await?;
        }
    }
    client.close().map_err(|error| format!("close: {error}"))?;
    let _ = driving.await;
    Ok(())
}

/// Moves a thread to the sidebar's settled shelf (`thread.settle`, the web's Settle action).
#[cfg(not(target_arch = "wasm32"))]
async fn settle(session: &t3_client::rpc::SessionEndpoint, id: &str) -> Result<(), String> {
    use t3_client::rpc::RpcEndpoint;
    use t3_contracts::orchestration_v2::OrchestrationV2Command;
    use t3_contracts::rpc::WsOrchestrationV2DispatchCommandRpc;
    let command: OrchestrationV2Command = serde_json::from_value(serde_json::json!({
        "type": "thread.settle", "commandId": format!("{id}-settle"), "threadId": id
    }))
    .map_err(|error| format!("thread.settle command: {error}"))?;
    session
        .call::<WsOrchestrationV2DispatchCommandRpc>(command)
        .await
        .map_err(|error| format!("thread.settle {id}: {error}"))?;
    println!("settled thread {id}");
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn main() {}
