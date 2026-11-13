#![cfg(all(
    feature = "local-services",
    feature = "workspace",
    feature = "terminal"
))]
#![allow(clippy::unwrap_used)]
mod e_common;
use e_common::*;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn url(fixture: &Fixture, resource: Value) -> String {
    let (mut socket, mut client) = fixture.socket(&fixture.cookie).await;
    let result = success(
        call(
            &mut socket,
            &mut client,
            "assets.createUrl",
            json!({"resource":resource}),
            false,
        )
        .await,
    );
    socket.close(None).await.unwrap();
    format!(
        "{}{}",
        fixture.base,
        result["relativeUrl"].as_str().unwrap()
    )
}
#[tokio::test]
async fn signed_assets_use_authority_safe_headers_ranges_head_and_descriptor_identity() {
    let fixture = Fixture::new().await;
    std::fs::write(fixture.root.join("video.mp4"), b"0123456789").unwrap();
    std::fs::write(fixture.root.join("index.html"), b"<h1>sandboxed</h1>").unwrap();
    let media = url(
        &fixture,
        json!({"_tag":"media-file","threadId":"thread-e","path":"video.mp4"}),
    )
    .await;
    let response = fixture.http.get(&media).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "video/mp4");
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert!(!response.headers().contains_key("etag"));
    assert_eq!(response.bytes().await.unwrap(), "0123456789");
    for (header, status, expected, content_range) in [
        ("bytes=2-5", 206, "2345", Some("bytes 2-5/10")),
        ("bytes=-3", 206, "789", Some("bytes 7-9/10")),
        ("bytes=8-", 206, "89", Some("bytes 8-9/10")),
        (
            "bytes=0-999999999999999999999999999999999999999999999",
            206,
            "0123456789",
            Some("bytes 0-9/10"),
        ),
        (
            "bytes=999999999999999999999999999999999999999999999-",
            416,
            "",
            Some("bytes */10"),
        ),
        ("bytes=10-", 416, "", Some("bytes */10")),
        ("bytes=-0", 416, "", Some("bytes */10")),
        ("bytes=3-2", 200, "0123456789", None),
        (
            "bytes=999999999999999999999999999999999999999999999-888888888888888888888888888888888888888888888",
            200,
            "0123456789",
            None,
        ),
        ("bytes=0-1,3-4", 200, "0123456789", None),
    ] {
        let response = fixture
            .http
            .get(&media)
            .header("range", header)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{header}");
        assert_eq!(
            response
                .headers()
                .get("content-range")
                .map(|value| value.to_str().unwrap()),
            content_range
        );
        assert_eq!(response.text().await.unwrap(), expected, "{header}");
    }
    let response = fixture
        .http
        .get(&media)
        .header("range", "bytes=0-1")
        .header("if-range", "old-etag")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "0123456789");
    let response = fixture
        .http
        .head(&media)
        .header("range", "bytes=0-1")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-length"], "10");
    assert!(response.bytes().await.unwrap().is_empty());
    let html = url(
        &fixture,
        json!({"_tag":"workspace-file","threadId":"thread-e","path":"index.html"}),
    )
    .await;
    let response = fixture.http.get(html).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["content-security-policy"],
        "sandbox allow-scripts allow-forms allow-popups allow-downloads"
    );
    assert_eq!(response.text().await.unwrap(), "<h1>sandboxed</h1>");
    let outside = fixture.temp.path().join("outside-project");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("favicon.svg"), "<svg>private</svg>").unwrap();
    std::fs::write(fixture.root.join("favicon.svg"), "<svg>authorized</svg>").unwrap();
    let favicon = url(&fixture, json!({"_tag":"project-favicon","cwd":outside})).await;
    let response = fixture.http.get(favicon).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "<svg>authorized</svg>");
    let expired = fixture.workspace.create_url(decode(json!({"resource":{"_tag":"workspace-file","threadId":"thread-e","path":"index.html"}})), 1).await.unwrap();
    assert_eq!(
        fixture
            .http
            .get(format!("{}{}", fixture.base, expired.relative_url.0))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let tampered = media.replacen("/api/assets/", "/api/assets/tampered", 1);
    assert_eq!(
        fixture.http.get(tampered).send().await.unwrap().status(),
        404
    );
    // Keep the old inode alive so replacing the file cannot reuse its identity.
    std::fs::rename(fixture.root.join("video.mp4"), fixture.root.join("old.mp4")).unwrap();
    std::fs::write(fixture.root.join("video.mp4"), b"secret replacement").unwrap();
    assert_eq!(fixture.http.get(media).send().await.unwrap().status(), 404);
    fixture.allowed.store(false, Ordering::SeqCst);
    let result = fixture.workspace.create_url(decode(json!({"resource":{"_tag":"workspace-file","threadId":"thread-e","path":"index.html"}})), 1).await;
    assert!(result.is_err(), "minting must re-read the authority");
    fixture.close().await;
}

#[tokio::test]
async fn upload_tokens_bound_raw_bodies_are_idempotent_and_cannot_overwrite() {
    let fixture = Fixture::new().await;
    let (mut socket, mut client) = fixture.socket(&fixture.cookie).await;
    let grant = success(
        call(
            &mut socket,
            &mut client,
            "attachments.createUploadUrl",
            json!({"type":"file","name":"notes.txt","mimeType":"text/plain","sizeBytes":3}),
            false,
        )
        .await,
    );
    let upload_path = grant["relativeUrl"].as_str().unwrap();
    let upload_url = format!("{}{upload_path}", fixture.base);
    assert_eq!(
        fixture
            .http
            .post(format!("{}/api/attachments/upload/invalid", fixture.base))
            .body("abc")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        fixture
            .http
            .post(&upload_url)
            .body("abcd")
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let address = fixture.base.strip_prefix("http://").unwrap();
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    stream.write_all(format!("POST {upload_path} HTTP/1.1\r\nHost: {address}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n4\r\nabcd\r\n0\r\n\r\n").as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 400"), "{response}");
    for payload in ["abc", "abc"] {
        assert_eq!(
            fixture
                .http
                .post(&upload_url)
                .body(payload)
                .send()
                .await
                .unwrap()
                .status(),
            204
        );
    }
    assert_eq!(
        fixture
            .http
            .post(&upload_url)
            .body("xyz")
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let asset = url(&fixture, json!({"_tag":"attachment","attachmentId":grant["attachmentId"],"fileName":"report.html","mimeType":"text/html","disposition":"attachment"})).await;
    let response = fixture.http.get(&asset).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["content-type"],
        "application/octet-stream"
    );
    assert_eq!(
        response.headers()["content-security-policy"],
        "default-src 'none'; sandbox"
    );
    assert_eq!(response.text().await.unwrap(), "abc");
    let html_grant = success(
        call(
            &mut socket,
            &mut client,
            "attachments.createUploadUrl",
            json!({"type":"file","name":"app.html","mimeType":"text/html","sizeBytes":3}),
            false,
        )
        .await,
    );
    assert_eq!(
        fixture
            .http
            .post(format!(
                "{}{}",
                fixture.base,
                html_grant["relativeUrl"].as_str().unwrap()
            ))
            .body("abc")
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let mcp_app = url(&fixture, json!({"_tag":"attachment","attachmentId":html_grant["attachmentId"],"mimeType":"text/html","disposition":"inline","renderIntent":"mcp-app"})).await;
    assert_eq!(
        fixture.http.get(mcp_app).send().await.unwrap().status(),
        501
    );
    success(
        call(
            &mut socket,
            &mut client,
            "attachments.delete",
            json!({"attachmentId":grant["attachmentId"]}),
            false,
        )
        .await,
    );
    assert_eq!(fixture.http.get(asset).send().await.unwrap().status(), 404);
    socket.close(None).await.unwrap();
    fixture.close().await;
}

#[test]
fn download_names_do_not_inject_headers_and_preserve_unicode_with_rfc5987() {
    use t3_server::handlers::workspace_terminal::download_disposition;
    assert_eq!(download_disposition(None), "attachment");
    assert_eq!(
        download_disposition(Some("a\"\r\n\\b.txt")),
        "attachment; filename=\"a____b.txt\""
    );
    assert_eq!(
        download_disposition(Some("résumé (1).pdf")),
        "attachment; filename=\"r_sum_ (1).pdf\"; filename*=UTF-8''r%C3%A9sum%C3%A9%20%281%29.pdf"
    );
}
