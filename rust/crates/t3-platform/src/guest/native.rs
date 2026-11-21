use futures::future::LocalBoxFuture;
use std::{cell::RefCell, collections::BTreeMap};
use t3_client::guest::{
    http::{GuestChunks, GuestHttp, GuestMethod, GuestRequest, GuestResponse, GuestStreamResponse},
    transport::{GuestError, GuestFuture},
};

/// Guest requests from a native process to one origin. A browser keeps the per-share cookie in its
/// own jar; this host keeps the cookies the origin sets, in memory only, and sends them back to
/// that origin. The token cookie is `Secure`, so the origin must be https (or loopback). The
/// caller supplies the client builder (TLS, proxy, timeouts); this type never follows a redirect,
/// because a followed redirect would resend the redeem body, which holds the invite.
pub struct NativeGuestHttp {
    client: reqwest::Client,
    origin: url::Url,
    cookies: RefCell<BTreeMap<String, String>>,
}

impl NativeGuestHttp {
    pub fn new(builder: reqwest::ClientBuilder, origin: url::Url) -> Result<Self, GuestError> {
        if !origin.username().is_empty() || origin.password().is_some() {
            return Err(GuestError::Network(
                "guest origins cannot carry credentials".into(),
            ));
        }
        if origin.scheme() != "https" && !loopback(&origin) {
            return Err(GuestError::Network(
                "guest requests need an https origin".into(),
            ));
        }
        let client = builder
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| GuestError::Network(error.without_url().to_string()))?;
        Ok(Self {
            client,
            origin,
            cookies: RefCell::default(),
        })
    }

    async fn dispatch(&self, request: GuestRequest) -> Result<reqwest::Response, GuestError> {
        let url = self
            .origin
            .join(&request.path)
            .map_err(|_| GuestError::Network("guest URL".into()))?;
        let mut builder = match request.method {
            GuestMethod::Get => self.client.get(url),
            GuestMethod::Post => self.client.post(url),
        };
        if let Some(json) = request.json {
            builder = builder
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(json);
        }
        let cookie = self
            .cookies
            .borrow()
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        if !cookie.is_empty() {
            let mut value = reqwest::header::HeaderValue::from_str(&cookie)
                .map_err(|_| GuestError::Network("cookie".into()))?;
            value.set_sensitive(true);
            builder = builder.header(reqwest::header::COOKIE, value);
        }
        let response = builder
            .send()
            .await
            // URLs can carry share ids; keep them out of error text.
            .map_err(|error| GuestError::Network(error.without_url().to_string()))?;
        for value in response.headers().get_all(reqwest::header::SET_COOKIE) {
            if let Ok(text) = value.to_str() {
                remember(&mut self.cookies.borrow_mut(), text);
            }
        }
        Ok(response)
    }
}

/// Literal loopback addresses and exactly `localhost`. Names under `.localhost` are not trusted:
/// outside a browser's own resolver they go to DNS.
fn loopback(origin: &url::Url) -> bool {
    match origin.host() {
        Some(url::Host::Domain(name)) => {
            name.trim_end_matches('.').eq_ignore_ascii_case("localhost")
        }
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

/// Applies one `Set-Cookie` header: a value stored under its name, removed when it is empty or
/// the cookie is already expired (`Max-Age` of zero or less).
fn remember(jar: &mut BTreeMap<String, String>, header: &str) {
    let mut parts = header.split(';');
    let Some((name, value)) = parts.next().and_then(|pair| pair.split_once('=')) else {
        return;
    };
    let expired = parts.any(|attribute| {
        attribute.trim().split_once('=').is_some_and(|(key, age)| {
            key.eq_ignore_ascii_case("max-age")
                && age.trim().parse::<i64>().is_ok_and(|age| age <= 0)
        })
    });
    let (name, value) = (name.trim(), value.trim());
    // A cookie that cannot be sent back would fail every later request; never store one.
    let sendable = !name.is_empty()
        && reqwest::header::HeaderValue::from_str(&format!("{name}={value}")).is_ok();
    if expired || value.is_empty() || !sendable {
        jar.remove(name);
    } else {
        jar.insert(name.to_owned(), value.to_owned());
    }
}

/// The most a non-streaming guest reply may hold.
const RESPONSE_LIMIT: usize = 1024 * 1024;

struct Body(reqwest::Response);

impl GuestChunks for Body {
    fn next(&mut self) -> GuestFuture<'_, Option<Vec<u8>>> {
        Box::pin(async move {
            self.0
                .chunk()
                .await
                .map(|chunk| chunk.map(|bytes| bytes.to_vec()))
                .map_err(|error| GuestError::Network(error.without_url().to_string()))
        })
    }
}

impl GuestHttp for NativeGuestHttp {
    fn send(&self, request: GuestRequest) -> GuestFuture<'_, GuestResponse> {
        Box::pin(async move {
            let response = self.dispatch(request).await?;
            let status = response.status().as_u16();
            let mut body = Body(response);
            let mut bytes = Vec::new();
            while let Some(chunk) = body.next().await? {
                if bytes.len() + chunk.len() > RESPONSE_LIMIT {
                    return Err(GuestError::Network("the response is too large".into()));
                }
                bytes.extend(chunk);
            }
            Ok(GuestResponse {
                status,
                body: bytes,
            })
        })
    }

    fn open(&self, request: GuestRequest) -> GuestFuture<'_, GuestStreamResponse> {
        Box::pin(async move {
            let response = self.dispatch(request).await?;
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            Ok(GuestStreamResponse {
                status: response.status().as_u16(),
                content_type,
                chunks: Box::new(Body(response)),
            })
        })
    }

    fn sleep(&self, millis: u64) -> LocalBoxFuture<'_, ()> {
        Box::pin(tokio::time::sleep(std::time::Duration::from_millis(millis)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// reqwest here has no built-in TLS provider; hosts pass a builder with their own TLS.
    fn builder() -> reqwest::ClientBuilder {
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::Client::builder()
    }

    fn origin(text: &str) -> url::Url {
        text.parse().unwrap_or_else(|_| unreachable!("test URL"))
    }

    #[test]
    fn a_secure_cookie_origin_must_be_https_or_loopback() {
        for allowed in [
            "https://box.example",
            "http://127.0.0.1:3773",
            "http://localhost:3773",
            "http://[::1]:3773",
        ] {
            assert!(
                NativeGuestHttp::new(builder(), origin(allowed)).is_ok(),
                "{allowed}"
            );
        }
        for refused in [
            "http://box.example",
            "http://192.168.1.20:3773",
            // Resolved by DNS outside a browser, so not trusted by name.
            "http://evil.localhost:3773",
            "http://[::ffff:127.0.0.1]:3773",
            // Would be sent as an Authorization header on every request.
            "https://user:secret@box.example",
            "http://user@127.0.0.1:3773",
        ] {
            assert!(
                matches!(
                    NativeGuestHttp::new(builder(), origin(refused)),
                    Err(GuestError::Network(_))
                ),
                "{refused}"
            );
        }
    }

    #[test]
    fn set_cookie_stores_replaces_and_expires_by_name() {
        let mut jar = BTreeMap::new();
        remember(
            &mut jar,
            "t3_guest_s1=token; Max-Age=86400; Path=/api/guest; HttpOnly; Secure",
        );
        assert_eq!(jar.get("t3_guest_s1").map(String::as_str), Some("token"));
        remember(&mut jar, "t3_guest_s1=next; Max-Age=86400");
        assert_eq!(jar.get("t3_guest_s1").map(String::as_str), Some("next"));
        remember(&mut jar, "t3_guest_s2=other");
        remember(&mut jar, "t3_guest_s1=next; Max-Age=0");
        assert!(!jar.contains_key("t3_guest_s1"), "Max-Age=0 expires it");
        remember(&mut jar, "bad=va\u{1}lue");
        assert!(
            !jar.contains_key("bad"),
            "an unsendable cookie is never stored"
        );
        remember(&mut jar, "t3_guest_s2=");
        assert!(jar.is_empty(), "an empty value clears it");
    }

    use std::sync::{Arc, Mutex};
    use t3_client::guest::http::{GuestHttp, GuestMethod, GuestRequest};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A one-answer-fits-all HTTP/1.1 server on loopback; records each request head.
    async fn serve(answer: Vec<u8>) -> (u16, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|_| unreachable!("bind loopback"));
        let port = listener.local_addr().map_or(0, |address| address.port());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let (log, answer) = (log.clone(), answer.clone());
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut buffer = [0u8; 1024];
                    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                        match socket.read(&mut buffer).await {
                            Ok(0) | Err(_) => break,
                            Ok(count) => head.extend_from_slice(&buffer[..count]),
                        }
                    }
                    if let Ok(mut log) = log.lock() {
                        log.push(String::from_utf8_lossy(&head).into_owned());
                    }
                    let _ = socket.write_all(&answer).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        (port, seen)
    }

    fn redeem() -> GuestRequest {
        GuestRequest {
            method: GuestMethod::Post,
            path: "/api/guest/redeem".into(),
            json: Some(r#"{"credential":"share_1.secret","name":"Alex"}"#.into()),
        }
    }

    #[tokio::test]
    async fn a_redirect_is_returned_and_never_followed_so_the_invite_stays_home() {
        let (other, other_seen) =
            serve(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n".to_vec()).await;
        let redirect = format!(
            "HTTP/1.1 307 Temporary Redirect\r\nlocation: http://127.0.0.1:{other}/api/guest/redeem\r\ncontent-length: 0\r\n\r\n"
        );
        let (port, seen) = serve(redirect.into_bytes()).await;
        let host = NativeGuestHttp::new(builder(), origin(&format!("http://127.0.0.1:{port}")))
            .unwrap_or_else(|_| unreachable!("loopback origin"));
        let reply = host.send(redeem()).await.ok();
        assert_eq!(reply.map(|reply| reply.status), Some(307));
        assert_eq!(seen.lock().map(|seen| seen.len()).unwrap_or(0), 1);
        // The redirect target saw nothing: not the body, not a cookie.
        assert_eq!(other_seen.lock().map(|seen| seen.len()).unwrap_or(99), 0);
    }

    #[tokio::test]
    async fn the_cookie_goes_back_to_the_origin_only() {
        let (port, seen) = serve(
            b"HTTP/1.1 204 No Content\r\nset-cookie: t3_guest_s1=token; Path=/api/guest; HttpOnly; Secure\r\n\r\n".to_vec(),
        )
        .await;
        let host = NativeGuestHttp::new(builder(), origin(&format!("http://localhost:{port}")))
            .unwrap_or_else(|_| unreachable!("loopback origin"));
        let _ = host.send(redeem()).await;
        let _ = host
            .send(GuestRequest {
                method: GuestMethod::Get,
                path: "/api/guest/session?share=s1".into(),
                json: None,
            })
            .await;
        let seen = seen.lock().map(|seen| seen.clone()).unwrap_or_default();
        assert_eq!(seen.len(), 2);
        assert!(!seen[0].to_ascii_lowercase().contains("cookie:"));
        assert!(seen[1].contains("t3_guest_s1=token"), "{}", seen[1]);
    }

    #[tokio::test]
    async fn a_reply_larger_than_the_limit_is_a_network_error() {
        let mut answer = format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n",
            2 * RESPONSE_LIMIT
        )
        .into_bytes();
        answer.extend(vec![b'x'; 2 * RESPONSE_LIMIT]);
        let (port, _) = serve(answer).await;
        let host = NativeGuestHttp::new(builder(), origin(&format!("http://127.0.0.1:{port}")))
            .unwrap_or_else(|_| unreachable!("loopback origin"));
        assert!(matches!(
            host.send(redeem()).await,
            Err(GuestError::Network(_))
        ));
    }
}
