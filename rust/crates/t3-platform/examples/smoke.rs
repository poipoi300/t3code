//! Headless public-client smoke test. The native platform and server state stay isolated.
//! T3_SMOKE_URL=http://127.0.0.1:3773 T3_SMOKE_PAIRING_TOKEN=... cargo run -p t3-platform --example smoke
//! Use T3_SMOKE_BEARER instead of the pairing token to restore an existing credential.
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("could not start tokio: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("smoke test failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn run() -> Result<(), String> {
    use futures::{
        FutureExt, StreamExt,
        future::{Either, select},
    };
    use t3_client::{
        authorization::Sensitive,
        connection::{ServerCredential, connect},
    };
    use t3_platform::{EphemeralSecretStore, NativeConfig, NativePlatform};
    let read = |name: &str| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    };
    let origin = read("T3_SMOKE_URL").ok_or("set T3_SMOKE_URL")?;
    let credential = match (read("T3_SMOKE_BEARER"), read("T3_SMOKE_PAIRING_TOKEN")) {
        (Some(token), None) => ServerCredential::Bearer(Sensitive(token)),
        (None, Some(token)) => ServerCredential::PairingToken(Sensitive(token)),
        _ => return Err("set exactly one of T3_SMOKE_BEARER or T3_SMOKE_PAIRING_TOKEN".into()),
    };
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    let platform = NativePlatform::open(NativeConfig::new(
        directory.path(),
        Box::new(EphemeralSecretStore::default()),
    ))
    .await
    .map_err(|error| error.to_string())?;
    let (client, mut driver) = connect(platform, &origin, credential)
        .await
        .map_err(|error| error.to_string())?;
    let snapshot = client.store().snapshot();
    let state = snapshot
        .environments
        .get(client.environment())
        .ok_or("environment is missing")?;
    let config = state.config.as_ref().ok_or("server config is missing")?;
    println!(
        "environment: {}, server {}",
        client.environment().0,
        config.environment.server_version.0
    );
    let mut changes = client.store().subscribe();
    let shell_ready = async {
        loop {
            let snapshot = client.store().snapshot();
            let state = snapshot
                .environments
                .get(client.environment())
                .ok_or("environment is missing")?;
            if state.shell.snapshot.is_some() {
                return Ok(());
            }
            if let Some(error) = &state.shell.error {
                return Err(error.clone());
            }
            changes
                .next()
                .await
                .ok_or("store closed before shell snapshot")?;
        }
    }
    .boxed_local();
    let result = match select(shell_ready, driver.as_mut()).await {
        Either::Left((result, _)) => result,
        Either::Right(_) => return Err("connection ended before shell snapshot".into()),
    };
    if let Err(error) = result {
        client.close().map_err(|error| error.to_string())?;
        driver.await.map_err(|error| error.to_string())?;
        return Err(error);
    }
    let snapshot = client.store().snapshot();
    let state = snapshot
        .environments
        .get(client.environment())
        .ok_or("environment is missing")?;
    let shell = state
        .shell
        .snapshot
        .as_ref()
        .ok_or("shell snapshot is missing")?;
    println!(
        "threads: {} active, {} archived, {} projects; shell {:?}",
        shell.threads.len(),
        shell.archived_threads.len(),
        shell.projects.len(),
        state.shell.status
    );
    for thread in &shell.threads {
        println!("  - {}", thread.title);
    }
    client.close().map_err(|error| error.to_string())?;
    driver.await.map_err(|error| error.to_string())
}
