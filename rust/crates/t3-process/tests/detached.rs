use t3_process::{DetachedConsole, ProcessError, ProcessOptions, spawn_detached};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn fixture(args: &[&str]) -> ProcessOptions {
    ProcessOptions::new(env!("CARGO_BIN_EXE_process-fixture")).args(args.iter().copied())
}

/// The child keeps running with no handle left anywhere and after the runtime that started it
/// is gone: nothing owns it.
#[tokio::test]
async fn a_detached_child_outlives_its_caller() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?.to_string();
    let pid = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(spawn_detached(
                &fixture(&["helper", &address]),
                DetachedConsole::None,
            ))
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    })
    .join()
    .map_err(|_| "spawning thread panicked")?
    .map_err(|e| e.to_string())?;
    assert!(pid > 0);
    let (mut socket, _) = listener.accept().await?;
    let mut greeting = [0; 5];
    socket.read_exact(&mut greeting).await?;
    assert_eq!(&greeting, b"ready");
    socket.write_all(b"x").await?;
    let mut reply = [0];
    socket.read_exact(&mut reply).await?;
    assert_eq!(&reply, b"x");
    // EOF releases the helper.
    Ok(())
}

#[tokio::test]
async fn a_missing_program_is_a_typed_failure() {
    let missing = std::env::temp_dir().join(format!("t3-missing-{}", std::process::id()));
    let result = spawn_detached(
        &ProcessOptions::new(missing.as_os_str()),
        DetachedConsole::None,
    )
    .await;
    assert!(
        matches!(
            result,
            Err(ProcessError::NotFound { .. } | ProcessError::Spawn { .. })
        ),
        "{result:?}"
    );
}
