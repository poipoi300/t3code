use futures::future::LocalBoxFuture;
use std::{cell::RefCell, collections::BTreeMap};
use t3_client::guest::{
    http::{GuestChunks, GuestHttp, GuestMethod, GuestRequest, GuestResponse, GuestStreamResponse},
    transport::{GuestError, GuestFuture},
};

/// Guest requests from a native process to one origin. A browser keeps the per-share cookie in its
/// own jar; this host keeps the cookies the origin sets, in memory only, and sends them back on
/// the guest routes. `client` is built by the caller (TLS, proxy, timeouts) and must not follow
/// redirects.
pub struct NativeGuestHttp {
    client: reqwest::Client,
    origin: url::Url,
    cookies: RefCell<BTreeMap<String, String>>,
}

impl NativeGuestHttp {
    pub fn new(client: reqwest::Client, origin: url::Url) -> Self {
        Self {
            client,
            origin,
            cookies: RefCell::default(),
        }
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
            let Ok(text) = value.to_str() else { continue };
            let pair = text.split(';').next().unwrap_or_default();
            if let Some((name, value)) = pair.split_once('=') {
                let mut jar = self.cookies.borrow_mut();
                if value.is_empty() {
                    jar.remove(name.trim());
                } else {
                    jar.insert(name.trim().to_owned(), value.trim().to_owned());
                }
            }
        }
        Ok(response)
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
