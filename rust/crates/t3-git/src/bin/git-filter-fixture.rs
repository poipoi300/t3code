use std::io::{Read, Write};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input)?;
    let mut socket = std::net::TcpStream::connect(std::env::var("T3_GIT_FILTER_ADDRESS")?)?;
    socket.write_all(b"ready")?;
    let mut byte = [0];
    socket.read_exact(&mut byte)?;
    std::io::stdout().write_all(&input)?;
    Ok(())
}
