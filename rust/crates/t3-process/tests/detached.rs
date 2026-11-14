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
                false,
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
        false,
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

/// The arguments one detached launch received, in order.
async fn received(arguments: &[&str], verbatim: bool) -> Vec<String> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let mut args = vec!["argv-socket", address.as_str()];
    args.extend_from_slice(arguments);
    spawn_detached(&fixture(&args), DetachedConsole::None, verbatim)
        .await
        .unwrap();
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut bytes = vec![];
    socket.read_to_end(&mut bytes).await.unwrap();
    bytes
        .split(|b| *b == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect()
}

/// Names a shell or PowerShell would read as code arrive as one argument each, unchanged.
#[tokio::test]
async fn hostile_arguments_arrive_as_one_argument_each() {
    let hostile = [
        "single'quote",
        "left\u{2018}quote",
        "x\u{2019}; Start-Process calc; \u{2019}y",
        "low\u{201A}quote",
        "reversed\u{201B}quote",
        "a;b & c | d",
        "back`tick $(whoami)",
        "-leading-dash",
        "comma,name with spaces",
        "percent%PATH%",
        "new\nline",
    ];
    assert_eq!(received(&hostile, false).await, hostile);
}

/// A verbatim argument reaches the command line exactly as given: the program parses it.
#[cfg(windows)]
#[tokio::test]
async fn verbatim_arguments_reach_the_command_line_unchanged() {
    let select = "/select,\"C:\\repo\\a,b \u{2019}; Start-Process calc; \u{2019}y.txt\"";
    assert_eq!(
        received(&[select], true).await,
        ["/select,C:\\repo\\a,b \u{2019}; Start-Process calc; \u{2019}y.txt"]
    );
}
