use crate::{
    chatgpt::{CallbackFactory, CallbackListener},
    sensitive::{Sensitive, ServiceError, ServiceResult},
};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

pub struct LoopbackCallbacks;
struct Listener {
    listener: TcpListener,
    redirect: String,
    pending: Option<TcpStream>,
    return_url: Option<String>,
    nonce: String,
}
#[async_trait]
impl CallbackFactory for LoopbackCallbacks {
    async fn listen(&self) -> ServiceResult<Box<dyn CallbackListener>> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| ServiceError::Callback)?;
        let port = listener
            .local_addr()
            .map_err(|_| ServiceError::Callback)?
            .port();
        let mut nonce = [0; 24];
        getrandom::fill(&mut nonce).map_err(|_| ServiceError::Callback)?;
        Ok(Box::new(Listener {
            listener,
            redirect: format!("http://127.0.0.1:{port}/auth/callback"),
            pending: None,
            return_url: None,
            nonce: URL_SAFE_NO_PAD.encode(nonce),
        }))
    }
}
#[async_trait]
impl CallbackListener for Listener {
    fn return_url(&mut self, return_url: Option<&str>) {
        self.return_url = provider_auth_return_url(return_url);
    }
    fn redirect_uri(&self) -> &str {
        &self.redirect
    }
    async fn receive(&mut self) -> ServiceResult<Sensitive<String>> {
        loop {
            let (mut socket, _) = self
                .listener
                .accept()
                .await
                .map_err(|_| ServiceError::Callback)?;
            let request = tokio::time::timeout(Duration::from_secs(5), async {
                let mut bytes = Vec::new();
                let mut chunk = [0; 1024];
                while bytes.len() <= 20 * 1024 {
                    let read = socket.read(&mut chunk).await.ok()?;
                    if read == 0 {
                        return None;
                    }
                    bytes.extend_from_slice(&chunk[..read]);
                    if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                        let line = std::str::from_utf8(&bytes).ok()?.split("\r\n").next()?;
                        let mut fields = line.split_whitespace();
                        if fields.next()? != "GET" {
                            return None;
                        }
                        let target = fields.next()?;
                        if !target.starts_with("/auth/callback?") {
                            return None;
                        }
                        return Some(format!(
                            "{}{}",
                            self.redirect.trim_end_matches("/auth/callback"),
                            target
                        ));
                    }
                }
                None
            })
            .await
            .ok()
            .flatten();
            if let Some(request) = request {
                self.pending = Some(socket);
                return Ok(Sensitive::new(request));
            }
            let _ = socket
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
        }
    }
    async fn respond(&mut self, accepted: bool) {
        if let Some(mut socket) = self.pending.take() {
            let body = if accepted {
                "Sign-in complete. You can close this tab."
            } else {
                "This response does not belong to the active sign-in."
            };
            let response = format!(
                "HTTP/1.1 {}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                if accepted {
                    "200 OK"
                } else {
                    "400 Bad Request"
                },
                body.len()
            );
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                socket.write_all(response.as_bytes()),
            )
            .await;
        }
    }
    async fn finish(&mut self, accepted: bool) {
        if let Some(mut socket) = self.pending.take() {
            let body = callback_page(accepted, self.return_url.as_deref(), &self.nonce);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; script-src 'nonce-{}'; base-uri 'none'; frame-ancestors 'none'\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                self.nonce,
                body.len()
            );
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                socket.write_all(response.as_bytes()),
            )
            .await;
        }
    }
}
pub fn provider_auth_return_url(value: Option<&str>) -> Option<String> {
    let mut url = url::Url::parse(value?).ok()?;
    let desktop = matches!(url.scheme(), "t3code" | "t3code-dev")
        && url.host_str() == Some("app")
        && url.port().is_none();
    let web = matches!(url.scheme(), "http" | "https")
        && (matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
            || url.origin().ascii_serialization() == "https://app.t3.codes");
    if !url.username().is_empty()
        || url.password().is_some()
        || !desktop && !web
        || !(url.path() == "/welcome"
            || url.path() == "/settings"
            || url.path().starts_with("/settings/"))
    {
        return None;
    }
    let welcome = url.path() == "/welcome";
    let pairs: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| {
            !welcome
                && [
                    "machine",
                    "project",
                    "checkout",
                    "environmentId",
                    "instanceId",
                ]
                .contains(&key.as_ref())
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if pairs.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(pairs);
    }
    let fragment = url.fragment().unwrap_or_default();
    if !welcome
        || !fragment.strip_prefix("agents:").is_some_and(|s| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
    {
        url.set_fragment(None);
    }
    Some(url.into())
}
pub fn callback_page(success: bool, return_url: Option<&str>, nonce: &str) -> String {
    let destination = provider_auth_return_url(return_url);
    let title = if success {
        "You're signed in"
    } else {
        "Sign-in couldn't finish"
    };
    let description = if success {
        if destination.is_some() {
            "Returning to T3 Code. You're ready to continue."
        } else {
            "Return to T3 Code to continue. You can close this tab."
        }
    } else {
        "Return to T3 Code and try signing in again."
    };
    let escape = |value: &str| {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&#39;")
    };
    let destination = destination.map(|s| escape(&s));
    include_str!("../data/codex-auth-callback.html")
        .replace("@@TITLE@@", &escape(title))
        .replace(
            "@@REFRESH@@",
            &destination
                .as_ref()
                .filter(|_| success)
                .map(|url| format!("<meta http-equiv=\"refresh\" content=\"1;url={url}\">"))
                .unwrap_or_default(),
        )
        .replace("@@LIGHT_BG@@", if success { "#ecfdf5" } else { "#fef2f2" })
        .replace("@@LIGHT_FG@@", if success { "#059669" } else { "#dc2626" })
        .replace("@@DARK_BG@@", if success { "#064e3b" } else { "#450a0a" })
        .replace("@@DARK_FG@@", if success { "#34d399" } else { "#f87171" })
        .replace(
            "@@ICON@@",
            if success {
                "<path d=\"m5 12 4 4L19 6\"/>"
            } else {
                "<path d=\"m6 6 12 12M6 18 18 6\"/>"
            },
        )
        .replace("@@DESCRIPTION@@", description)
        .replace(
            "@@LINK@@",
            &destination
                .map(|url| format!("<a href=\"{url}\">Return to T3 Code</a>"))
                .unwrap_or_default(),
        )
        .replace("@@NONCE@@", &escape(nonce))
}
