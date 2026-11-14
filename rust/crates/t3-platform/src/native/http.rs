//! reqwest without default features: rustls with the ring provider, no redirects, no cookies,
//! no HTTP/2, the system proxy on desktop. The body is read chunk by chunk against the caller's cap.
use super::Timeouts;
use std::{collections::BTreeMap, sync::Arc};
use t3_client::platform::{HttpRequest, HttpResponse, PlatformError};

#[derive(Clone)]
pub(crate) struct HttpClient {
    client: reqwest::Client,
    /// Loopback and link-local never go through a proxy, as Chromium's implicit bypass does in
    /// the TypeScript desktop app; the system proxy settings do not say so on their own.
    direct: reqwest::Client,
}

/// Hosts Chromium never sends through a proxy: loopback and link-local, in every spelling.
pub(crate) fn always_direct(host: &url::Host<&str>) -> bool {
    let v4 = |address: std::net::Ipv4Addr| address.is_loopback() || address.is_link_local();
    match host {
        url::Host::Domain(name) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
        url::Host::Ipv4(address) => v4(*address),
        url::Host::Ipv6(address) => {
            address.is_loopback()
                || address.segments()[0] & 0xffc0 == 0xfe80
                || address.to_ipv4_mapped().is_some_and(v4)
        }
    }
}

fn network(error: reqwest::Error) -> PlatformError {
    if error.is_timeout() {
        PlatformError::Network("HTTP request timed out".into())
    } else {
        // URLs can carry tickets; keep them out of error text.
        PlatformError::Network(error.without_url().to_string())
    }
}

impl HttpClient {
    pub(crate) fn new(
        tls: Arc<rustls::ClientConfig>,
        timeouts: &Timeouts,
    ) -> Result<Self, reqwest::Error> {
        let builder = || {
            reqwest::Client::builder()
                .use_preconfigured_tls((*tls).clone())
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(timeouts.connect)
                .timeout(timeouts.http)
        };
        // Desktop follows the system proxy as the TypeScript desktop app (Chromium) does: on
        // Windows and macOS the OS settings only, on Linux the environment. Mobile uses none yet.
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let client = match super::proxy::system() {
            Some(proxy) => builder()
                .proxy(reqwest::Proxy::custom(move |url| proxy.proxy_for(url)))
                .build()?,
            None => builder().no_proxy().build()?,
        };
        #[cfg(target_os = "linux")]
        let client = builder().build()?;
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        let client = builder().no_proxy().build()?;
        let direct = builder().no_proxy().build()?;
        Ok(Self { client, direct })
    }

    pub(crate) async fn send(&self, request: HttpRequest) -> Result<HttpResponse, PlatformError> {
        let url = url::Url::parse(&request.url)
            .map_err(|_| PlatformError::InvalidData("HTTP URL".into()))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(PlatformError::InvalidData("HTTP URL scheme".into()));
        }
        let client = if url.host().as_ref().is_some_and(always_direct) {
            &self.direct
        } else {
            &self.client
        };
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|_| PlatformError::InvalidData("HTTP method".into()))?;
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in &request.headers {
            headers.append(
                reqwest::header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| PlatformError::InvalidData("HTTP header name".into()))?,
                reqwest::header::HeaderValue::from_str(value)
                    .map_err(|_| PlatformError::InvalidData("HTTP header value".into()))?,
            );
        }
        let mut response = client
            .request(method, url)
            .headers(headers)
            .body(request.body)
            .send()
            .await
            .map_err(network)?;
        let cap = request.max_response_bytes;
        if response
            .content_length()
            .is_some_and(|length| length > cap as u64)
        {
            return Err(PlatformError::TooLarge);
        }
        let status = response.status().as_u16();
        let mut response_headers = BTreeMap::<String, String>::new();
        for (name, value) in response.headers() {
            let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
            response_headers
                .entry(name.as_str().to_owned())
                .and_modify(|existing| {
                    existing.push_str(", ");
                    existing.push_str(&value);
                })
                .or_insert(value);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(network)? {
            if body.len() + chunk.len() > cap {
                return Err(PlatformError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(HttpResponse {
            status,
            headers: response_headers,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::always_direct;

    #[test]
    fn loopback_hosts_are_the_ones_chromium_never_proxies() {
        for (url, loopback) in [
            ("http://localhost:3773/", true),
            ("http://LOCALHOST./", true),
            ("http://app.localhost/", true),
            ("http://127.0.0.1/", true),
            ("http://127.8.9.10/", true),
            ("http://[::1]:80/", true),
            ("http://localhost.example.com/", false),
            ("http://10.0.0.1/", false),
            ("https://example.com/", false),
            ("http://[::2]/", false),
            // Second re-review C2: Chromium's implicit bypass also covers link-local and the
            // IPv4-mapped spellings.
            ("http://169.254.10.20/", true),
            ("http://[fe80::1]/", true),
            ("http://[::ffff:127.0.0.1]/", true),
            ("http://[::ffff:169.254.1.1]/", true),
            ("http://[::ffff:10.0.0.1]/", false),
        ] {
            let parsed = url::Url::parse(url).unwrap();
            assert_eq!(always_direct(&parsed.host().unwrap()), loopback, "{url}");
        }
    }
}
