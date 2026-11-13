#[cfg(windows)]
use std::ffi::OsString;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use t3_process::{CancellationToken, OutputMode, ProcessError, ProcessOptions, run};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Notify,
};

fn fixture(args: &[&str]) -> ProcessOptions {
    ProcessOptions::new(env!("CARGO_BIN_EXE_process-fixture")).args(args.iter().copied())
}

#[tokio::test]
async fn stdin_streams_limits_and_invalid_utf8() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = fixture(&["echo"]);
    options.stdin = Some(vec![b'e'; 200_000]);
    assert_eq!(run(options).await?.stdout.len(), 200_000);
    let mut error_options = fixture(&["streams", "", &"x".repeat(2048)]);
    error_options.max_output_bytes = 128;
    assert!(matches!(
        run(error_options).await,
        Err(ProcessError::OutputLimit {
            stream: "stderr",
            ..
        })
    ));
    let mut options = fixture(&["streams", "exactly", "exactly"]);
    options.max_output_bytes = 7;
    assert!(!run(options).await?.stdout_truncated);
    let mut options = fixture(&["bytes", "2048"]);
    options.max_output_bytes = 128;
    assert!(matches!(
        run(options).await,
        Err(ProcessError::OutputLimit {
            stream: "stdout",
            max_bytes: 128,
            ..
        })
    ));
    let observed = Arc::new(Mutex::new(0));
    let mut options = fixture(&["bytes", "2048"]);
    options.max_output_bytes = 128;
    options.output_mode = OutputMode::Truncate;
    options.truncated_marker = "\n\n[truncated]".into();
    let chunks = observed.clone();
    options.on_stdout_chunk = Some(Arc::new(move |bytes| {
        *chunks.lock().expect("counter") += bytes.len()
    }));
    let output = run(options).await?;
    assert_eq!(output.stdout.len(), 128 + 13);
    assert!(output.stdout_truncated);
    assert_eq!(*observed.lock().expect("counter"), 2048);
    let observed_stderr = Arc::new(Mutex::new(0));
    let stderr_chunks = observed_stderr.clone();
    let mut options = fixture(&["streams", "", &"x".repeat(2048)]);
    options.max_output_bytes = 128;
    options.output_mode = OutputMode::Truncate;
    options.on_stderr_chunk = Some(Arc::new(move |bytes| {
        *stderr_chunks.lock().expect("stderr counter") += bytes.len();
    }));
    let output = run(options).await?;
    assert_eq!(output.stderr.len(), 128);
    assert!(output.stderr_truncated);
    assert_eq!(*observed_stderr.lock().expect("stderr counter"), 2048);
    let output = run(fixture(&["invalid"])).await?;
    assert!(output.stdout_invalid_utf8);
    assert_eq!(output.stdout_text(), "\u{fffd}é");
    Ok(())
}

#[tokio::test]
async fn cwd_environment_and_typed_exit_errors() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let mut options = fixture(&["environment"]);
    options.cwd = Some(temp.path().to_owned());
    options
        .env
        .insert("T3_PROCESS_TEST_VALUE".into(), Some("patched".into()));
    let output = run(options).await?.stdout_text();
    let (value, cwd) = output.split_once('\n').expect("environment and cwd");
    assert_eq!(value, "patched");
    assert_eq!(
        std::fs::canonicalize(cwd)?,
        std::fs::canonicalize(temp.path())?
    );
    let mut options = fixture(&["environment"]);
    options.cwd = Some(temp.path().join("missing"));
    options.spawn_cwd = Some(temp.path().to_owned());
    options.inherit_env = false;
    options
        .env
        .insert("T3_PROCESS_TEST_VALUE".into(), Some("isolated".into()));
    assert!(run(options).await?.stdout_text().starts_with("isolated\n"));
    let mut options = fixture(&["exit", "0"]);
    options.cwd = Some(temp.path().join("missing"));
    assert!(matches!(
        run(options).await,
        Err(ProcessError::Spawn { .. })
    ));
    assert!(matches!(
        run(fixture(&["exit", "7"])).await,
        Err(ProcessError::NonZeroExit { code: Some(7), .. })
    ));
    let mut options = fixture(&["exit", "7"]);
    options.check_exit = false;
    assert_eq!(run(options).await?.code, Some(7));
    assert!(matches!(
        run(ProcessOptions::new("t3-nonexistent-command-98ac2f")).await,
        Err(ProcessError::NotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn timeout_discards_partial_output_and_cancellation_is_typed()
-> Result<(), Box<dyn std::error::Error>> {
    for timed_out_result in [false, true] {
        let ready = Arc::new(Notify::new());
        let mut options = fixture(&["socket", "unused"]);
        options.timeout = Some(Duration::from_secs(600));
        options.timed_out_result = timed_out_result;
        let ready_callback = ready.clone();
        options.on_stdout_chunk = Some(Arc::new(move |_| {
            ready_callback.notify_one();
        }));
        // The socket fixture remains alive independently of stdin EOF.
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?.to_string();
        options.args = vec!["socket".into(), address.into()];
        let task = tokio::spawn(run(options));
        let (mut socket, _) = listener.accept().await?;
        let mut greeting = [0; 5];
        socket.read_exact(&mut greeting).await?;
        ready.notified().await;
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(600)).await;
        let result = task.await?;
        tokio::time::resume();
        if timed_out_result {
            let output = result?;
            assert!(output.timed_out);
            assert!(output.stdout.is_empty());
        } else {
            assert!(matches!(result, Err(ProcessError::Timeout { .. })));
        }
        assert_eq!(socket.read(&mut greeting).await.unwrap_or(0), 0);
    }
    let options = fixture(&["exit", "0"]);
    options.cancellation.cancel();
    assert!(matches!(run(options).await, Err(ProcessError::Cancelled)));
    Ok(())
}

#[tokio::test]
async fn cancellation_and_dropped_futures_kill_owned_descendants()
-> Result<(), Box<dyn std::error::Error>> {
    for mode in ["cancel", "drop", "timeout"] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let mut options = fixture(&["tree", &listener.local_addr()?.to_string()]);
        let cancellation = CancellationToken::new();
        options.cancellation = cancellation.clone();
        options.timeout = Some(Duration::from_secs(600));
        let task = tokio::spawn(run(options));
        let (mut socket, _) = listener.accept().await?;
        let mut greeting = [0; 5];
        socket.read_exact(&mut greeting).await?;
        assert_eq!(&greeting, b"ready");
        if mode == "drop" {
            task.abort();
            assert!(task.await.expect_err("aborted task").is_cancelled());
        } else if mode == "cancel" {
            cancellation.cancel();
            assert!(matches!(task.await?, Err(ProcessError::Cancelled)));
        } else {
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(600)).await;
            let result = task.await?;
            tokio::time::resume();
            assert!(matches!(result, Err(ProcessError::Timeout { .. })));
        }
        assert_eq!(socket.read(&mut greeting).await.unwrap_or(0), 0);
    }
    Ok(())
}

#[tokio::test]
async fn normal_exit_preserves_detached_helper() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let options = fixture(&["detached-tree", &listener.local_addr()?.to_string()]);
    let mut parent = t3_process::spawn(&options)?;
    let (mut socket, _) = listener.accept().await?;
    let mut greeting = [0; 5];
    socket.read_exact(&mut greeting).await?;
    assert_eq!(&greeting, b"ready");
    parent.stdin.take().expect("stdin").write_all(b"x").await?;
    assert!(parent.wait().await?.success());
    drop(parent);
    socket.write_all(b"x").await?;
    let mut reply = [0];
    socket.read_exact(&mut reply).await?;
    assert_eq!(&reply, b"x");
    // EOF releases the helper even if an assertion fails and unwinds the test.
    Ok(())
}

#[tokio::test]
async fn dropping_unfinished_handle_kills_owned_descendants()
-> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let parent = t3_process::spawn(&fixture(&["tree", &listener.local_addr()?.to_string()]))?;
    let (mut socket, _) = listener.accept().await?;
    let mut greeting = [0; 5];
    socket.read_exact(&mut greeting).await?;
    assert_eq!(&greeting, b"ready");
    drop(parent);
    assert_eq!(socket.read(&mut greeting).await.unwrap_or(0), 0);
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn timeout_kills_immediately_spawned_grandchildren_repeatedly()
-> Result<(), Box<dyn std::error::Error>> {
    for iteration in 0..100 {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let mut options = fixture(&["immediate-tree", &listener.local_addr()?.to_string()]);
        options.timeout = Some(Duration::from_secs(600));
        let task = tokio::spawn(run(options));
        let (mut socket, _) = listener.accept().await?;
        let mut greeting = [0; 5];
        socket.read_exact(&mut greeting).await?;
        assert_eq!(&greeting, b"ready", "iteration {iteration}");
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(600)).await;
        let result = task.await?;
        tokio::time::resume();
        assert!(
            matches!(result, Err(ProcessError::Timeout { .. })),
            "iteration {iteration}"
        );
        assert_eq!(
            socket.read(&mut greeting).await.unwrap_or(0),
            0,
            "iteration {iteration}"
        );
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn explicit_non_pathext_executable() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let executable = temp.path().join("fixture.custom");
    std::fs::copy(env!("CARGO_BIN_EXE_process-fixture"), &executable)?;
    assert_eq!(
        run(ProcessOptions::new(&executable).args(["exit", "0"]))
            .await?
            .code,
        Some(0)
    );
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn children_of_consoleless_parent_have_no_console() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("console.cs");
    let probe = temp.path().join("console.exe");
    std::fs::write(
        &source,
        r#"using System; using System.Runtime.InteropServices;
public static class Probe {
    [DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow();
    public static void Main() { Console.Write(GetConsoleWindow() == IntPtr.Zero ? "none" : "console"); }
}"#,
    )?;
    let powershell = std::path::PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let compiled = tokio::process::Command::new(powershell)
        .args(["-NoProfile", "-NonInteractive", "-Command", "Add-Type -TypeDefinition ([IO.File]::ReadAllText($env:T3_PROCESS_PROBE_SOURCE)) -OutputAssembly $env:T3_PROCESS_PROBE_EXE -OutputType ConsoleApplication"])
        .env("T3_PROCESS_PROBE_SOURCE", &source).env("T3_PROCESS_PROBE_EXE", &probe)
        .creation_flags(0x0800_0000).output().await?;
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_process-fixture"))
        .arg("console-parent")
        .arg(probe)
        .arg("unused")
        .creation_flags(0x0000_0008) // DETACHED_PROCESS: the parent has no console to inherit.
        .output()
        .await?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "none");
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn batch_shims_preserve_arguments_or_refuse_before_execution()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let injected = temp.path().join("injected.txt");
    let attack = "x\" & echo pwned>injected.txt & rem \"";
    let arguments = [
        "ordinary",
        "",
        "feature & release",
        "(topic)",
        "a|b<c>d",
        "a^b",
        "!name!",
        "a\"b",
        "%T3_PROCESS_BATCH_PAYLOAD%",
        "%PATH%",
        "a\nb",
        "a\rb",
        "trailing\\",
        "space trailing\\",
        "quote\\\"&whoami",
        "\"",
        "%",
        "^",
        "&",
        "|",
        "<",
        ">",
        "%%",
        "%1",
        "a^^&b",
        "\"&|<>^%",
        "two trailing\\\\",
        "space trailing\\\\",
        "tab\tvalue",
        "é日本語",
        "nul\0byte",
        attack,
    ];
    let mut failures = Vec::new();
    let mut accepted = 0;
    let mut refused = 0;
    for extension in ["cmd", "bat"] {
        for forwarding in ["%*", "%1 %2"] {
            let shim = temp.path().join(format!("test shim.{extension}"));
            std::fs::write(
                &shim,
                format!(
                    "@echo off\r\n\"{}\" argv {forwarding}\r\n",
                    env!("CARGO_BIN_EXE_process-fixture")
                ),
            )?;
            for arg in arguments {
                for provided in [[arg, "sentinel"], ["sentinel", arg]] {
                    let mut options = ProcessOptions::new(&shim).args(provided);
                    options.cwd = Some(temp.path().to_owned());
                    options
                        .env
                        .insert("T3_PROCESS_BATCH_PAYLOAD".into(), Some(attack.into()));
                    let result = run(options).await;
                    let intact_or_refused = match result {
                        Ok(output) => {
                            accepted += 1;
                            output.stdout_text()
                                == provided
                                    .iter()
                                    .map(|value| format!("{}:{value}", value.len()))
                                    .collect::<String>()
                        }
                        Err(ProcessError::Spawn { source, .. }) => {
                            refused += 1;
                            source.kind() == std::io::ErrorKind::InvalidInput
                        }
                        _ => false,
                    };
                    if injected.exists() || !intact_or_refused {
                        failures.push(format!("{extension} {forwarding} {provided:?}: injected={}, intact_or_refused={intact_or_refused}", injected.exists()));
                    }
                    if injected.exists() {
                        std::fs::remove_file(&injected)?;
                    }
                }
            }
        }
    }
    eprintln!(
        "batch matrix: {accepted} intact arguments, {refused} typed refusals, {} failures",
        failures.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn windows_cmd_shims_use_pathext_and_escape_shell_metacharacters()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let git_root = temp.path().join("git");
    std::fs::create_dir_all(git_root.join("cmd"))?;
    std::fs::create_dir_all(git_root.join("ucrt64/bin"))?;
    let launcher = git_root.join("cmd/git.exe");
    let native = git_root.join("ucrt64/bin/git.exe");
    std::fs::write(&launcher, "")?;
    std::fs::write(&native, "")?;
    let mut env = t3_process::Environment::new();
    env.insert("MSYSTEM".into(), Some("".into()));
    assert_eq!(
        std::fs::canonicalize(
            t3_process::resolve_command(launcher.as_os_str(), &[], &env, false)?.program
        )?,
        std::fs::canonicalize(&native)?
    );
    env.insert("MSYSTEM".into(), Some("MSYS".into()));
    assert_eq!(
        std::fs::canonicalize(
            t3_process::resolve_command(launcher.as_os_str(), &[], &env, false)?.program
        )?,
        std::fs::canonicalize(&launcher)?
    );
    let shim = temp.path().join("test shim.cmd");
    std::fs::write(
        &shim,
        format!(
            "@echo off\r\n\"{}\" streams %1 %2\r\n",
            env!("CARGO_BIN_EXE_process-fixture")
        ),
    )?;
    let mut options = ProcessOptions::new("test shim").args(["feature & release", "(topic)"]);
    options
        .env
        .insert("PATH".into(), Some(temp.path().as_os_str().to_owned()));
    options
        .env
        .insert("PATHEXT".into(), Some(OsString::from("CMD;EXE")));
    let output = run(options).await?;
    assert_eq!(output.stdout_text(), "feature & release");
    assert_eq!(output.stderr_text(), "(topic)");
    Ok(())
}
