//! `ReqwestTransport` behaves as Node's fetch: ambient proxy variables are ignored. The client
//! reads the environment when it is built, so the request runs in a child process whose
//! environment points every proxy variable at a dead address.
#![allow(clippy::unwrap_used)]
use std::collections::BTreeMap;
use t3_provider::transport::{HttpRequest, HttpTransport, ReqwestTransport};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CHILD_URL: &str = "T3_PROVIDER_TRANSPORT_PROXY_CHILD_URL";
const TEST: &str = "a_request_with_a_dead_ambient_proxy_reaches_the_loopback_stand_in";

#[tokio::test]
async fn a_request_with_a_dead_ambient_proxy_reaches_the_loopback_stand_in() {
    if let Ok(url) = std::env::var(CHILD_URL) {
        let response = ReqwestTransport::new()
            .unwrap()
            .raw(HttpRequest {
                method: "GET",
                url,
                headers: BTreeMap::new(),
                body: None,
            })
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body.expose().as_slice(), b"stand-in");
        return;
    }
    // A port that was just free: nothing listens there, so a proxied request would fail.
    let dead = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let stand_in = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut byte = [0u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nstand-in")
            .await
            .unwrap();
        String::from_utf8(request).unwrap()
    });
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child
        .args([TEST, "--exact", "--test-threads=1"])
        .env(CHILD_URL, format!("http://{address}/rates"))
        .env("HTTP_PROXY", &dead)
        .env("HTTPS_PROXY", &dead)
        .env("ALL_PROXY", &dead)
        .env("http_proxy", &dead)
        .env_remove("NO_PROXY")
        .env_remove("no_proxy");
    let output = tokio::task::spawn_blocking(move || child.output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "child failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // The stand-in saw an origin-form request, not a proxy's absolute-form one.
    let request = stand_in.await.unwrap();
    assert!(request.starts_with("GET /rates HTTP/1.1\r\n"), "{request}");
}
