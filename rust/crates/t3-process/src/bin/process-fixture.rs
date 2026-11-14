use std::{
    io::{Read, Write},
    net::TcpStream,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|mode| mode == "immediate-tree") {
        let mut command = std::process::Command::new(std::env::current_exe()?);
        command
            .args(["socket", &args[1]])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0000_0008);
        }
        command.spawn()?.wait()?;
        return Ok(());
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(execute(args))
}

async fn execute(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    match args.first().map(String::as_str) {
        Some("echo") => {
            let mut input = Vec::new();
            std::io::stdin().read_to_end(&mut input)?;
            std::io::stdout().write_all(&input)?;
        }
        Some("streams") => {
            std::io::stdout().write_all(args[1].as_bytes())?;
            std::io::stderr().write_all(args[2].as_bytes())?;
        }
        Some("argv") => {
            for arg in &args[1..] {
                print!("{}:{arg}", arg.len());
            }
        }
        // Reports its argument vector, each NUL-terminated, to a loopback listener.
        Some("argv-socket") => {
            let mut socket = TcpStream::connect(&args[1])?;
            for arg in std::env::args_os().skip(3) {
                socket.write_all(arg.to_string_lossy().as_bytes())?;
                socket.write_all(&[0])?;
            }
        }
        Some("helper") => {
            let mut socket = TcpStream::connect(&args[1])?;
            socket.write_all(b"ready")?;
            let mut byte = [0];
            while socket.read_exact(&mut byte).is_ok() {
                socket.write_all(&byte)?;
            }
        }
        Some("detached-tree") => {
            let mut command = std::process::Command::new(std::env::current_exe()?);
            command
                .args(["helper", &args[1]])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x0000_0008); // DETACHED_PROCESS still inherits the job.
            }
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                command.process_group(0);
            }
            let _child = command.spawn()?;
            std::io::stdin().read_exact(&mut [0])?;
        }
        #[cfg(windows)]
        Some("console-parent") => {
            let output =
                t3_process::run(t3_process::ProcessOptions::new(&args[1]).args(&args[2..])).await?;
            std::io::stdout().write_all(&output.stdout)?;
        }
        Some("bytes") => {
            let count: usize = args[1].parse()?;
            std::io::stdout().write_all(&vec![b'x'; count])?;
        }
        Some("invalid") => std::io::stdout().write_all(&[0xff, 0xc3, 0xa9])?,
        Some("exit") => std::process::exit(args[1].parse()?),
        Some("environment") => {
            print!(
                "{}\n{}",
                std::env::var("T3_PROCESS_TEST_VALUE")?,
                std::env::current_dir()?.display()
            );
        }
        Some("hold") => {
            println!("ready");
            std::io::stdout().flush()?;
            let mut input = Vec::new();
            std::io::stdin().read_to_end(&mut input)?;
        }
        Some("socket") => {
            println!("partial");
            std::io::stdout().flush()?;
            let mut socket = TcpStream::connect(&args[1])?;
            socket.write_all(b"ready")?;
            let mut byte = [0];
            socket.read_exact(&mut byte)?;
        }
        Some("tree") => {
            // Inherit the parent's group/job so this fixture exercises descendant termination.
            let mut command = tokio::process::Command::new(std::env::current_exe()?);
            command
                .args(["socket", &args[1]])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true);
            #[cfg(windows)]
            command.creation_flags(0x0000_0008);
            let mut child = command.spawn()?;
            child.wait().await?;
        }
        _ => return Err("unknown fixture mode".into()),
    }
    Ok(())
}
