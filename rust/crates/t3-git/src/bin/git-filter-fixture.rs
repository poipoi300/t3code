use std::io::{Read, Write};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(address) = std::env::var("T3_GIT_COMMAND_ADDRESS") {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        let output =
            std::process::Command::new(std::env::var_os("T3_REAL_GIT").ok_or("real Git missing")?)
                .args(&args)
                .stdin(std::process::Stdio::inherit())
                .output()?;
        if output.status.success() && args.iter().any(|arg| arg == "write-tree") {
            let mut socket = std::net::TcpStream::connect(address)?;
            socket.write_all(b"ready")?;
            let mut release = [0];
            socket.read_exact(&mut release)?;
        }
        std::io::stdout().write_all(&output.stdout)?;
        std::io::stderr().write_all(&output.stderr)?;
        std::process::exit(output.status.code().unwrap_or(1));
    }
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input)?;
    let mut socket = std::net::TcpStream::connect(std::env::var("T3_GIT_FILTER_ADDRESS")?)?;
    socket.write_all(b"ready")?;
    let mut byte = [0];
    socket.read_exact(&mut byte)?;
    std::io::stdout().write_all(&input)?;
    Ok(())
}
