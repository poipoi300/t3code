use std::io::{Read, Write};
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input)?;
    let command_gate = std::env::var("T3_GIT_GATE_COMMAND").ok();
    let nested_gate = command_gate.is_none()
        && args
            .get(1)
            .is_some_and(|cwd| std::path::Path::new(cwd).ends_with("empty nested"))
        && args.iter().any(|arg| arg == "HEAD^{commit}");
    if nested_gate
        || command_gate
            .as_ref()
            .is_some_and(|command| args.iter().any(|arg| arg == command.as_str()))
    {
        let mut socket = std::net::TcpStream::connect(std::env::var("T3_GIT_GATE_ADDRESS")?)?;
        socket.write_all(b"ready")?;
        if command_gate.is_some() {
            let arguments: Vec<_> = args.iter().map(|arg| arg.to_string_lossy()).collect();
            writeln!(
                socket,
                "{}",
                serde_json::json!({
                    "args": arguments,
                    "stdin": String::from_utf8_lossy(&input),
                })
            )?;
        }
        let mut byte = [0];
        socket.read_exact(&mut byte)?;
    }
    let mut process = t3_process::ProcessOptions::new("git");
    process.args = args;
    process.timeout = None;
    process.check_exit = false;
    process.stdin = Some(input);
    let output = t3_process::run(process).await?;
    std::io::stdout().write_all(&output.stdout)?;
    std::io::stderr().write_all(&output.stderr)?;
    std::process::exit(output.code.unwrap_or(1));
}
