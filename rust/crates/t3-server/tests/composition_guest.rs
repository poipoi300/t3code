//! Guest transport over a bound loopback server.
#![cfg(all(feature = "cloud", feature = "orchestration"))]
#![allow(clippy::unwrap_used)]
mod c_support;

use c_support::{Fixture, unary};
use serde_json::json;

#[tokio::test]
async fn guest_transport_requires_its_own_cookie_and_owner_create_requires_access_write() {
    let server = Fixture::start().await;
    for name in ["Line\nbreak", "Alex\u{202e}"] {
        let response = server
            .http
            .post(format!("{}/api/guest/redeem", server.base))
            .json(&json!({"credential": "fixture.invalid", "name": name}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
    }

    for path in [
        "session?share=fixture",
        "stream?share=fixture",
        "history?share=fixture&cursor=fixture",
        "item?share=fixture&item=other-thread-item",
    ] {
        let response = server
            .http
            .get(format!("{}/api/guest/{path}", server.base))
            .header("cookie", &server.cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403, "{path}");
        let error = response.json::<serde_json::Value>().await.unwrap();
        assert_eq!(error["reason"], "invalid", "{path}");
    }

    let denied = server.scoped_cookie(&["access:read"]).await;
    let (mut socket, mut client) = server.connect(&denied, "").await;
    let body = json!({"threadId": "thread:fixture"});
    let refusal = unary(&mut socket, &mut client, "guestShare.create", body.clone()).await;
    let denied = serde_json::to_value(refusal).unwrap();
    assert_eq!(
        denied["cause"][0]["error"]["_tag"],
        "EnvironmentAuthorizationError"
    );
    assert_eq!(denied["cause"][0]["error"]["requiredScope"], "access:write");

    let (mut socket, mut client) = server.connect(&server.cookie, "").await;
    let result = unary(&mut socket, &mut client, "guestShare.create", body).await;
    assert!(format!("{result:?}").contains("disabled"), "{result:?}");
    server.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn owner_session_cannot_submit_guest_messages_or_open_guest_command_output() {
    let server = Fixture::start().await;
    let response = server
        .http
        .post(format!("{}/api/guest/messages?share=fixture", server.base))
        .header("cookie", &server.cookie)
        .json(&json!({"shareId": "fixture", "text": "incoming"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["reason"],
        "invalid"
    );

    let response = server
        .http
        .get(format!(
            "{}/api/guest/command-output?share=fixture&item=unshared",
            server.base
        ))
        .header("cookie", &server.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["reason"],
        "invalid"
    );
    server.runtime.shutdown().await.unwrap();
}
