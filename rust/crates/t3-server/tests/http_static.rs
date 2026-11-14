#![cfg(feature = "local-services")]
#![allow(clippy::unwrap_used)]
//! Static client serving: `handleStaticAndDevRequest` in `apps/server/src/http.ts`.
mod http_common;
use http_common::*;
use reqwest::Method;

const MANIFEST: &str = r#"{"src/main.tsx":{"file":"assets/app-abcd1234.js","css":[],"assets":[]}}"#;

async fn site() -> Fixture {
    let f = fixture(&[
        ("client.js", "export const x = 1"),
        ("assets/app-abcd1234.js", "listed"),
        (
            "assets/other-abcd1234.js",
            "same naming, not in the build manifest",
        ),
        ("assets/plain.js", "plain"),
        ("docs/index.html", "<html>docs</html>"),
        (".vite/manifest.json", MANIFEST),
    ])
    .await;
    std::fs::write(f.temp.path().join("secret.txt"), "outside the static root").unwrap();
    f
}

#[tokio::test]
async fn html_headers_validators_and_fallbacks() {
    let f = site().await;
    let root = f.get("/").send().await.unwrap();
    assert_eq!(root.status(), 200);
    let headers = root.headers().clone();
    assert_eq!(headers["content-type"], "text/html; charset=utf-8");
    assert_eq!(headers["cache-control"], "no-cache");
    assert_eq!(headers["vary"], "Accept-Encoding");
    // Upstream 94aa3d7736: no other site may frame the app.
    assert_eq!(headers["content-security-policy"], "frame-ancestors 'none'");
    assert_eq!(headers["x-frame-options"], "DENY");
    assert!(
        headers.get("etag").is_none() && headers.get("last-modified").is_none(),
        "HTML is always revalidated"
    );
    assert_eq!(root.text().await.unwrap(), "<html>t3 client</html>");

    // HTML is never answered 304, even for `*`.
    let conditional = f
        .get("/")
        .header("if-none-match", "*")
        .send()
        .await
        .unwrap();
    assert_eq!(conditional.status(), 200);

    // Extensionless paths try a directory index, then the SPA shell; `?query` is not part of the path.
    assert_eq!(
        f.get("/docs").send().await.unwrap().text().await.unwrap(),
        "<html>docs</html>"
    );
    assert_eq!(
        f.get("/docs/").send().await.unwrap().text().await.unwrap(),
        "<html>docs</html>"
    );
    assert_eq!(
        f.get("/threads/abc")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "<html>t3 client</html>"
    );
    assert_eq!(
        f.get("/missing.js")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "<html>t3 client</html>"
    );
    assert_eq!(
        f.get("/client.js?v=1")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "export const x = 1"
    );
    assert_eq!(f.get("/healthz").send().await.unwrap().status(), 200);
    f.stop().await;
}

#[tokio::test]
async fn non_html_validators_conditionals_and_head() {
    let f = site().await;
    let response = f.get("/client.js").send().await.unwrap();
    assert_eq!(response.status(), 200);
    let etag = response.headers()["etag"].to_str().unwrap().to_owned();
    let modified = response.headers()["last-modified"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(etag.starts_with("W/\"") && etag.ends_with('"'), "{etag}");
    assert_eq!(response.headers()["cache-control"], "no-cache");
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .contains("javascript")
    );
    assert_eq!(response.text().await.unwrap(), "export const x = 1");

    let not_modified = |name: &'static str, value: String| {
        let request = f.get("/client.js").header(name, value);
        async move { request.send().await.unwrap() }
    };
    for value in [
        etag.clone(),
        etag.replacen("W/", "w/", 1),
        etag.trim_start_matches("W/").to_owned(),
        "*".into(),
        format!("\"nope\", {etag}"),
    ] {
        let response = not_modified("if-none-match", value.clone()).await;
        assert_eq!(response.status(), 304, "{value}");
        assert_eq!(response.headers()["etag"], etag.as_str());
        assert_eq!(response.headers()["last-modified"], modified.as_str());
        assert_eq!(response.headers()["cache-control"], "no-cache");
        assert_eq!(response.headers()["vary"], "Accept-Encoding");
        assert!(response.bytes().await.unwrap().is_empty());
    }
    assert_eq!(
        not_modified("if-none-match", "\"other\"".into())
            .await
            .status(),
        200
    );
    // If-None-Match decides alone: a matching date cannot turn a mismatch into a 304.
    let precedence = f
        .get("/client.js")
        .header("if-none-match", "\"other\"")
        .header("if-modified-since", modified.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(precedence.status(), 200);
    assert_eq!(
        not_modified("if-modified-since", modified.clone())
            .await
            .status(),
        304
    );
    assert_eq!(
        not_modified("if-modified-since", "Thu, 01 Jan 1970 00:00:00 GMT".into())
            .await
            .status(),
        200
    );
    assert_eq!(
        not_modified("if-modified-since", "garbage".into())
            .await
            .status(),
        200
    );

    let head = f.request(Method::HEAD, "/client.js").send().await.unwrap();
    assert_eq!(head.status(), 200);
    assert_eq!(head.headers()["content-length"], "18");
    assert_eq!(head.headers()["etag"], etag.as_str());
    assert!(head.bytes().await.unwrap().is_empty());
    let head_html = f.request(Method::HEAD, "/").send().await.unwrap();
    assert_eq!(head_html.headers()["content-length"], "22");
    f.stop().await;
}

#[tokio::test]
async fn only_manifest_listed_hashed_assets_are_immutable() {
    let f = site().await;
    let cache = |path: &'static str| {
        let request = f.get(path);
        async move {
            request.send().await.unwrap().headers()["cache-control"]
                .to_str()
                .unwrap()
                .to_owned()
        }
    };
    assert_eq!(
        cache("/assets/app-abcd1234.js").await,
        "public, max-age=31536000, immutable"
    );
    assert_eq!(cache("/assets/other-abcd1234.js").await, "no-cache");
    assert_eq!(cache("/assets/plain.js").await, "no-cache");
    assert_eq!(cache("/client.js").await, "no-cache");
    f.stop().await;
}

#[tokio::test]
async fn traversal_and_unsafe_paths_are_refused() {
    let f = site().await;
    for path in [
        "/..%2fsecret.txt",
        "/%2e%2e%2f%2e%2e%2fsecret.txt",
        "/docs/..%2f..%2fsecret.txt",
        "/%2e%2e%5csecret.txt",
        "/a%5cb",
        "/a%00b",
        "/%00",
        "/C:/Windows/win.ini",
        "/C%3A/Windows/win.ini",
        "/client.js:stream",
        "/client.js::$DATA",
        "/%ff",
    ] {
        let response = f.get(path).send().await.unwrap();
        assert_eq!(response.status(), 400, "{path}");
        let body = response.text().await.unwrap();
        assert!(
            !body.contains("outside the static root") && !body.contains("t3 client"),
            "{path}"
        );
    }
    // HTTP clients normalise dot segments away, so send the literal request line ourselves.
    for target in [
        "/../secret.txt",
        "/%2e%2e/secret.txt",
        "/docs/../../secret.txt",
        "/.%2e/secret.txt",
    ] {
        let response = raw_get(&f, target).await;
        assert!(response.starts_with("HTTP/1.1 400"), "{target}: {response}");
        assert!(!response.contains("outside the static root"), "{target}");
    }
    f.stop().await;
}

async fn raw_get(f: &Fixture, target: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let address = f.base.trim_start_matches("http://");
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let request = format!("GET {target} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response
}

#[tokio::test]
async fn reserved_paths_are_not_the_spa_and_other_methods_are_refused() {
    let f = site().await;
    for path in [
        "/api",
        "/api/nope",
        "/api/orchestration/nope",
        "/oauth/nope",
        "/.well-known/nope",
        "/ws/voice",
    ] {
        assert_eq!(f.get(path).send().await.unwrap().status(), 404, "{path}");
    }
    // A prefix match is a path-segment match: `/apiary` is a client route.
    assert_eq!(f.get("/apiary").send().await.unwrap().status(), 200);
    assert_eq!(f.post("/api/nope").send().await.unwrap().status(), 404);
    let post = f.post("/threads/abc").send().await.unwrap();
    assert_eq!(post.status(), 405);
    assert_eq!(post.headers()["allow"], "GET, HEAD");
    f.stop().await;
}
