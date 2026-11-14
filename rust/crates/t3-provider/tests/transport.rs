#![allow(clippy::unwrap_used)]
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use t3_provider::{sensitive::ServiceError, transport::*};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
async fn reply(body: Option<Vec<u8>>) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
        }
        if let Some(body) = body {
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            let _ = socket.write_all(&body).await;
        }
    });
    (format!("http://{address}/?key=private-cause-token"), server)
}
fn request(url: String) -> HttpRequest {
    HttpRequest {
        method: "GET",
        url,
        headers: BTreeMap::new(),
        body: None,
    }
}
#[tokio::test]
async fn transport_accepts_eight_mib_and_refuses_a_larger_response() {
    for size in [8 * 1024 * 1024, 8 * 1024 * 1024 + 1] {
        let (url, server) = reply(Some(vec![b'x'; size])).await;
        let result = ReqwestTransport::new().unwrap().raw(request(url)).await;
        server.await.unwrap();
        if size == 8 * 1024 * 1024 {
            assert_eq!(result.unwrap().body.expose().len(), size);
        } else {
            assert_eq!(result.unwrap_err(), ServiceError::Response);
        }
    }
}
#[derive(Clone)]
struct Writer(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[tokio::test]
async fn a_secret_bearing_transport_cause_is_logged_only_in_redacted_form() {
    let logs = Arc::new(Mutex::new(Vec::new()));
    let output = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || Writer(output.clone()))
        .finish();
    let _capture = tracing::subscriber::set_default(subscriber);
    let (url, server) = reply(None).await;
    let error = ReqwestTransport::new()
        .unwrap()
        .raw(request(url))
        .await
        .unwrap_err();
    server.await.unwrap();
    assert_eq!(error, ServiceError::Network);
    let logs = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("Provider operation failed"));
    assert!(logs.contains("REDACTED"));
    assert!(logs.contains("source_kind="));
    assert!(logs.contains("cause_type="));
    assert!(!logs.contains("private-cause-token"));
    assert!(!format!("{error:?} {error}").contains("private-cause-token"));
}
