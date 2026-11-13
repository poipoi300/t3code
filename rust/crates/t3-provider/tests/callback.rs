#![allow(clippy::unwrap_used)]
use t3_provider::{callback::*, chatgpt::CallbackFactory};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[test]
fn return_destinations_follow_the_source_allowlist_and_strip_auth_material() {
    for url in [
        "https://evil.invalid/settings",
        "https://app.t3.codes.evil.invalid/settings",
        "javascript:alert(1)",
        "http://user:password@localhost/settings",
        "http://localhost/threads/1",
        "t3code://elsewhere/settings",
    ] {
        assert!(provider_auth_return_url(Some(url)).is_none(), "{url}");
    }
    assert_eq!(
        provider_auth_return_url(Some(
            "http://localhost:3000/settings/providers?instanceId=work&code=secret#token"
        )),
        Some("http://localhost:3000/settings/providers?instanceId=work".into())
    );
    assert_eq!(
        provider_auth_return_url(Some("https://app.t3.codes/welcome?code=secret#agents:work")),
        Some("https://app.t3.codes/welcome#agents:work".into())
    );
    let page = callback_page(
        true,
        Some("t3code://app/settings?instanceId=work&code=secret"),
        "safe-nonce",
    );
    assert!(!page.contains("secret"));
    assert!(page.contains("history.replaceState"));
    assert!(page.contains("nonce=\"safe-nonce\""));
    assert!(page.contains("http-equiv=\"refresh\""));
    assert!(
        !callback_page(false, Some("https://app.t3.codes/settings"), "safe")
            .contains("http-equiv=\"refresh\"")
    );
}
#[tokio::test]
async fn loopback_callback_returns_nonce_protected_html_and_keeps_stray_requests_separate() {
    let mut listener = LoopbackCallbacks.listen().await.unwrap();
    listener.return_url(Some("http://localhost:3000/settings?code=secret"));
    let url = url::Url::parse(listener.redirect_uri()).unwrap();
    let address = format!("127.0.0.1:{}", url.port().unwrap());
    let client = tokio::spawn(async move {
        let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
        socket
            .write_all(
                b"GET /auth/callback?code=secret&state=state HTTP/1.1\r\nHost: localhost\r\n\r\n",
            )
            .await
            .unwrap();
        let mut bytes = vec![];
        socket.read_to_end(&mut bytes).await.unwrap();
        String::from_utf8(bytes).unwrap()
    });
    let callback = listener.receive().await.unwrap();
    assert!(callback.expose().contains("code=secret"));
    listener.finish(true).await;
    let response = client.await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("script-src 'nonce-"));
    assert!(!response.contains("code=secret"));
    assert!(response.contains("X-Content-Type-Options: nosniff"));
}
