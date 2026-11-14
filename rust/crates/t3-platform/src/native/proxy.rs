//! The desktop's system proxy on Windows and macOS, read as Chromium (the TypeScript desktop app)
//! reads it: from the OS settings only, never the environment, with the OS bypass list and its
//! "simple hostnames go direct" rule. reqwest's own system proxy puts environment variables
//! first and ignores both. Linux keeps reqwest's environment-based proxy, as Chromium does there.

/// A proxy for each scheme, and the hosts that go direct.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SystemProxy {
    /// `host:port` (or a URL) for plain HTTP and WebSocket requests.
    pub http: Option<String>,
    /// `host:port` (or a URL) for HTTPS.
    pub https: Option<String>,
    /// Windows' `<local>`, macOS' "Exclude simple hostnames": names without a dot go direct.
    pub bypass_simple: bool,
    /// Glob patterns (`*.example.com`, `10.*`, `intranet`), lowercase, matched on the host.
    pub bypass: Vec<String>,
}

impl SystemProxy {
    /// Windows' `ProxyServer` (`host:port`, or `http=host:port;https=host:port;…`) and
    /// `ProxyOverride` (`;`-separated patterns and `<local>`), read when `ProxyEnable` is set.
    pub(crate) fn from_windows(server: &str, overrides: &str) -> Self {
        let mut proxy = Self::default();
        for entry in server.split(';').map(str::trim).filter(|entry| !entry.is_empty()) {
            match entry.split_once('=') {
                Some((scheme, address)) => match scheme.trim().to_ascii_lowercase().as_str() {
                    "http" => proxy.http = Some(address.trim().to_owned()),
                    "https" => proxy.https = Some(address.trim().to_owned()),
                    _ => {}
                },
                None => {
                    proxy.http.get_or_insert_with(|| entry.to_owned());
                    proxy.https.get_or_insert_with(|| entry.to_owned());
                }
            }
        }
        for entry in overrides.split(';') {
            proxy.add_bypass(entry);
        }
        proxy
    }

    fn add_bypass(&mut self, entry: &str) {
        let entry = entry.trim();
        if entry.eq_ignore_ascii_case("<local>") {
            self.bypass_simple = true;
        } else if !entry.is_empty() {
            self.bypass.push(entry.to_ascii_lowercase());
        }
    }

    /// The proxy `url` goes through, or `None` to go direct.
    pub(crate) fn proxy_for(&self, url: &url::Url) -> Option<String> {
        let host = url.host()?;
        if super::http::always_direct(&host) || self.bypasses(&host) {
            return None;
        }
        let address = match url.scheme() {
            "https" | "wss" => self.https.as_ref(),
            _ => self.http.as_ref(),
        }?;
        Some(if address.contains("://") {
            address.clone()
        } else {
            format!("http://{address}")
        })
    }

    fn bypasses(&self, host: &url::Host<&str>) -> bool {
        let name = match host {
            url::Host::Domain(name) => name.trim_end_matches('.').to_ascii_lowercase(),
            url::Host::Ipv4(address) => address.to_string(),
            url::Host::Ipv6(address) => address.to_string(),
        };
        (self.bypass_simple && matches!(host, url::Host::Domain(_)) && !name.contains('.'))
            || self
                .bypass
                .iter()
                .any(|pattern| glob(pattern_host(pattern), &name))
    }
}

/// The host part of a bypass entry: without a scheme (`http://`) or a port (`:8080`).
fn pattern_host(pattern: &str) -> &str {
    let pattern = pattern.split_once("://").map_or(pattern, |(_, rest)| rest);
    match pattern.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            host
        }
        _ => pattern,
    }
}

/// `*` matches any run of characters, including none.
fn glob(pattern: &str, text: &str) -> bool {
    let (pattern, text) = (pattern.as_bytes(), text.as_bytes());
    let (mut p, mut t) = (0, 0);
    let mut resume = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == b'*' {
            resume = Some((p, t));
            p += 1;
        } else if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if let Some((star, matched)) = resume {
            p = star + 1;
            t = matched + 1;
            resume = Some((star, matched + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|byte| *byte == b'*')
}

/// The current user's Internet Settings, as Chromium reads them on Windows.
#[cfg(target_os = "windows")]
pub(crate) fn system() -> Option<SystemProxy> {
    let settings = windows_registry::CURRENT_USER
        .open(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
        .ok()?;
    if settings.get_u32("ProxyEnable").unwrap_or(0) == 0 {
        return None;
    }
    let server = settings.get_string("ProxyServer").ok()?;
    let overrides = settings.get_string("ProxyOverride").unwrap_or_default();
    Some(SystemProxy::from_windows(&server, &overrides))
}

/// The network settings' proxies, as Chromium reads them on macOS.
#[cfg(target_os = "macos")]
pub(crate) fn system() -> Option<SystemProxy> {
    use system_configuration::{
        core_foundation::{array::CFArray, base::CFType, number::CFNumber, string::CFString},
        dynamic_store::SCDynamicStoreBuilder,
    };
    let store = SCDynamicStoreBuilder::new("t3-platform").build()?;
    let proxies = store.get_proxies()?;
    let number = |key: &'static str| {
        proxies
            .find(&CFString::from_static_string(key))
            .and_then(|value| value.downcast::<CFNumber>())
            .and_then(|value| value.to_i32())
    };
    let string = |key: &'static str| {
        proxies
            .find(&CFString::from_static_string(key))
            .and_then(|value| value.downcast::<CFString>())
            .map(|value| value.to_string())
    };
    let address = |enable, host, port| {
        if number(enable) != Some(1) {
            return None;
        }
        let host = string(host)?;
        Some(match number(port) {
            Some(port) => format!("{host}:{port}"),
            None => host,
        })
    };
    let mut proxy = SystemProxy {
        http: address("HTTPEnable", "HTTPProxy", "HTTPPort"),
        https: address("HTTPSEnable", "HTTPSProxy", "HTTPSPort"),
        bypass_simple: number("ExcludeSimpleHostnames") == Some(1),
        bypass: Vec::new(),
    };
    if let Some(exceptions) = proxies
        .find(&CFString::from_static_string("ExceptionsList"))
        .and_then(|value| value.downcast::<CFArray<CFType>>())
    {
        for exception in exceptions.iter() {
            if let Some(exception) = exception.downcast::<CFString>() {
                proxy.add_bypass(&exception.to_string());
            }
        }
    }
    (proxy.http.is_some() || proxy.https.is_some()).then_some(proxy)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn route(proxy: &SystemProxy, url: &str) -> Option<String> {
        proxy.proxy_for(&url::Url::parse(url).unwrap())
    }

    #[test]
    fn windows_settings_route_as_chromium_does() {
        let proxy = SystemProxy::from_windows(
            "proxy.corp:8080",
            "<local>; *.corp.example; 10.*; 192.168.1.*;build-host:8443",
        );
        let through = Some("http://proxy.corp:8080".to_owned());
        for (url, expected) in [
            ("https://github.com/", through.clone()),
            ("http://198.51.100.7/", through.clone()),
            // `<local>`: a dotless intranet name goes direct, with its credentials.
            ("http://intranet/", None),
            ("http://INTRANET./", None),
            ("http://wiki.corp.example/", None),
            ("http://corp.example.org/", through.clone()),
            ("http://10.1.2.3/", None),
            ("http://192.168.1.40/", None),
            ("http://192.168.2.40/", through.clone()),
            ("https://build-host/", None),
            // Always direct, whatever the settings say.
            ("http://localhost:3773/", None),
            ("http://169.254.169.254/", None),
            ("http://[::ffff:127.0.0.1]/", None),
            ("http://[fe80::1]/", None),
        ] {
            assert_eq!(route(&proxy, url), expected, "{url}");
        }
    }

    #[test]
    fn a_proxy_per_scheme_applies_to_its_scheme_only() {
        let proxy = SystemProxy::from_windows("http=web:80;https=secure:443;socks=ignored:1080", "");
        assert_eq!(route(&proxy, "http://example.com/"), Some("http://web:80".into()));
        assert_eq!(route(&proxy, "ws://example.com/"), Some("http://web:80".into()));
        assert_eq!(route(&proxy, "https://example.com/"), Some("http://secure:443".into()));
        assert_eq!(route(&proxy, "wss://example.com/"), Some("http://secure:443".into()));
        let https_only = SystemProxy::from_windows("https=secure:443", "");
        assert_eq!(route(&https_only, "http://example.com/"), None);
    }

    #[test]
    fn without_local_a_dotless_name_is_proxied() {
        let proxy = SystemProxy::from_windows("proxy:3128", "");
        assert_eq!(route(&proxy, "http://intranet/"), Some("http://proxy:3128".into()));
    }
}
