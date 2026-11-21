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

fn loopback(origin: &url::Url) -> bool {
    match origin.host() {
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
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
    if expired || value.is_empty() {
        jar.remove(name);
    } else {
        jar.insert(name.to_owned(), value.to_owned());
    }
}

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
            while bytes.len() < 1024 * 1024 {
                match body.next().await? {
                    Some(chunk) => bytes.extend(chunk),
                    None => break,
                }
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
            Ok(GuestStreamResponse {
                status: response.status().as_u16(),
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
        for refused in ["http://box.example", "http://192.168.1.20:3773"] {
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
        remember(&mut jar, "t3_guest_s2=");
        assert!(jar.is_empty(), "an empty value clears it");
    }
}
