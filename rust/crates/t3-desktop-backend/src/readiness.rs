//! The readiness probe: `GET /.well-known/t3/environment` answers 2xx once the server serves.

use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

/// `BACKEND_READINESS_PATH`.
pub const READINESS_PATH: &str = "/.well-known/t3/environment";

/// One probe within `probe_timeout` (`DEFAULT_BACKEND_READINESS_REQUEST_TIMEOUT`).
pub async fn probe(host: &str, port: u16, probe_timeout: Duration) -> bool {
    timeout(probe_timeout, async {
        let mut stream = TcpStream::connect((host, port)).await.ok()?;
        let authority = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        let request = format!(
            "GET {READINESS_PATH} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.ok()?;
        // "HTTP/1.1 200": the status code is bytes 9 to 12.
        let mut status = [0u8; 12];
        stream.read_exact(&mut status).await.ok()?;
        (status.starts_with(b"HTTP/")
            && status[9] == b'2'
            && status[10].is_ascii_digit()
            && status[11].is_ascii_digit())
        .then_some(())
    })
    .await
    .is_ok_and(|ready| ready.is_some())
}
