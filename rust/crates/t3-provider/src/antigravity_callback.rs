use crate::sensitive::{Sensitive, ServiceError, ServiceResult};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
#[derive(Debug, Clone)]
pub struct Authorization {
    pub url: Sensitive<String>,
    pub redirect: Sensitive<String>,
    pub state: Sensitive<String>,
}
fn one(url: &url::Url, name: &str) -> Option<String> {
    let values = url
        .query_pairs()
        .filter(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
        .collect::<Vec<_>>();
    (values.len() == 1).then(|| values[0].clone())
}
pub fn authorization(input: &str) -> ServiceResult<Authorization> {
    if input.encode_utf16().count() > 16384
        || input
            .chars()
            .any(|character| t3_contracts::wire::trim_js(&character.to_string()).is_empty())
    {
        return Err(ServiceError::Callback);
    }
    let url = url::Url::parse(input).map_err(|_| ServiceError::Callback)?;
    let state = one(&url, "state")
        .filter(|state| {
            !state.is_empty()
                && state.encode_utf16().count() <= 512
                && !state
                    .chars()
                    .any(|character| t3_contracts::wire::trim_js(&character.to_string()).is_empty())
        })
        .ok_or(ServiceError::Callback)?;
    let redirect = one(&url, "redirect_uri").ok_or(ServiceError::Callback)?;
    if url.origin().ascii_serialization() != "https://accounts.google.com"
        || url.path() != "/o/oauth2/v2/auth"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || one(&url, "response_type").as_deref() != Some("code")
        || !regex::Regex::new(r"^http://127\.0\.0\.1:[1-9][0-9]{0,4}/$")
            .expect("fixed loopback redirect")
            .is_match(&redirect)
    {
        return Err(ServiceError::Callback);
    }
    let parsed = url::Url::parse(&redirect).map_err(|_| ServiceError::Callback)?;
    if parsed.port().is_none_or(|port| port < 1024) {
        return Err(ServiceError::Callback);
    }
    Ok(Authorization {
        url: Sensitive::new(input.into()),
        redirect: Sensitive::new(redirect),
        state: Sensitive::new(state),
    })
}
pub fn validate(pending: &Authorization, input: &str) -> ServiceResult<Sensitive<url::Url>> {
    if input.encode_utf16().count() > 16384 {
        return Err(ServiceError::Callback);
    }
    let callback = url::Url::parse(input).map_err(|_| ServiceError::Callback)?;
    let expected =
        url::Url::parse(pending.redirect.expose()).map_err(|_| ServiceError::Callback)?;
    if callback.scheme() != "http"
        || callback.host_str() != Some("127.0.0.1")
        || callback.origin() != expected.origin()
        || callback.path() != expected.path()
        || !callback.username().is_empty()
        || callback.password().is_some()
        || callback.fragment().is_some()
        || one(&callback, "state").as_deref() != Some(pending.state.expose())
    {
        return Err(ServiceError::Callback);
    }
    let count = |key: &str| {
        callback
            .query_pairs()
            .filter(|(name, _)| name == key)
            .count()
    };
    let code = one(&callback, "code").filter(|code| !code.is_empty());
    let error = one(&callback, "error").filter(|error| !error.is_empty());
    if !(code.is_some() && count("error") == 0 || error.is_some() && count("code") == 0)
        || count("iss") > 1
        || count("iss") == 1
            && one(&callback, "iss").as_deref() != Some("https://accounts.google.com")
    {
        return Err(ServiceError::Callback);
    }
    Ok(Sensitive::new(callback))
}
pub async fn forward(callback: &Sensitive<url::Url>) -> ServiceResult<()> {
    let callback = callback.expose();
    if callback.scheme() != "http"
        || callback.host_str() != Some("127.0.0.1")
        || callback.port().is_none()
    {
        return Err(ServiceError::Callback);
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        let port = callback.port().ok_or(ServiceError::Callback)?;
        let mut socket = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|_| ServiceError::Callback)?;
        let path = format!(
            "{}{}",
            callback.path(),
            callback
                .query()
                .map(|query| format!("?{query}"))
                .unwrap_or_default()
        );
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
        socket
            .write_all(request.as_bytes())
            .await
            .map_err(|_| ServiceError::Callback)?;
        let mut header = vec![];
        let mut buffer = [0; 1024];
        let mut status = None;
        loop {
            let count = socket
                .read(&mut buffer)
                .await
                .map_err(|_| ServiceError::Callback)?;
            if count == 0 {
                break;
            }
            if status.is_none() {
                header.extend_from_slice(&buffer[..count]);
                if let Some(end) = header.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    let first = std::str::from_utf8(&header[..end])
                        .map_err(|_| ServiceError::Callback)?
                        .split("\r\n")
                        .next()
                        .ok_or(ServiceError::Callback)?;
                    status = first
                        .split_whitespace()
                        .nth(1)
                        .and_then(|status| status.parse::<u16>().ok());
                    if !status.is_some_and(|status| (200..300).contains(&status)) {
                        return Err(ServiceError::Callback);
                    }
                    header.clear();
                } else if header.len() > 65536 {
                    return Err(ServiceError::Callback);
                }
            }
        }
        if status.is_some_and(|status| (200..300).contains(&status)) {
            Ok(())
        } else {
            Err(ServiceError::Callback)
        }
    })
    .await
    .map_err(|_| ServiceError::Callback)?
}
