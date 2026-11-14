//! Static web client and SPA fallback: `handleStaticAndDevRequest` in `apps/server/src/http.ts`.
use crate::services::ServiceRegistry;
use axum::{
    body::Body,
    extract::OriginalUri,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use std::{
    path::{Component, PathBuf},
    sync::Arc,
    time::SystemTime,
};
use tokio_util::io::ReaderStream;

/// `packages/shared/src/devProxy.ts`. Unlike the TypeScript, which only reserves these while a dev
/// server is configured, they are never answered with the SPA: an unported API route should read
/// as missing, not as an HTML page a JSON client cannot parse.
const RESERVED_PREFIXES: [&str; 5] = ["/api", "/oauth", "/.well-known", "/ws", "/mcp"];

pub fn is_reserved(path: &str) -> bool {
    RESERVED_PREFIXES.iter().any(|prefix| {
        path == *prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

fn loopback_host(host: &str) -> bool {
    let host = host.trim().to_ascii_lowercase();
    let host = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or("").to_owned(),
        None => host.rsplit_once(':').map_or(host.clone(), |(h, port)| {
            if port.bytes().all(|b| b.is_ascii_digit()) {
                h.to_owned()
            } else {
                host.clone()
            }
        }),
    };
    matches!(host.as_str(), "127.0.0.1" | "::1" | "localhost")
}

/// `^assets/.+-[\w-]{8}\.[^/]+$`.
pub fn hashed_asset_name(relative: &str) -> bool {
    let Some(rest) = relative.strip_prefix("assets/") else {
        return false;
    };
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    let bytes = rest.as_bytes();
    bytes.iter().enumerate().any(|(dot, b)| {
        *b == b'.'
            && dot >= 10
            && bytes[dot - 8..dot].iter().all(|b| word(*b))
            && bytes[dot - 9] == b'-'
            && dot + 1 < bytes.len()
            && !bytes[dot + 1..].contains(&b'/')
    })
}

fn seconds(time: SystemTime) -> Option<SystemTime> {
    httpdate::parse_http_date(&httpdate::fmt_http_date(time)).ok()
}

pub async fn serve(
    axum::extract::State(s): axum::extract::State<Arc<ServiceRegistry>>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
) -> Response {
    if is_reserved(uri.path()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if method != Method::GET && method != Method::HEAD {
        let mut response = StatusCode::METHOD_NOT_ALLOWED.into_response();
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
        return response;
    }
    let config = &s.local.config;
    if let Some(dev) = &config.dev_url
        && headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .is_some_and(loopback_host)
    {
        let mut target = dev.clone();
        target.set_path(uri.path());
        target.set_query(uri.query());
        let mut response = StatusCode::FOUND.into_response();
        if let Ok(value) = HeaderValue::from_str(target.as_str()) {
            response.headers_mut().insert(header::LOCATION, value);
        }
        return response;
    }
    let Some(root) = &config.static_dir else {
        return if config.dev_url.is_some() {
            // The first web build has not landed yet; the page reloads itself until it does.
            let mut response = (
                StatusCode::SERVICE_UNAVAILABLE,
                "<!doctype html><meta http-equiv=\"refresh\" content=\"2\"><p>Waiting for the first web build. This page reloads by itself.</p>",
            )
                .into_response();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            );
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("2"));
            response
        } else {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "No static directory configured and no dev URL set.",
            )
                .into_response()
        };
    };
    let path = match percent_encoding::percent_decode_str(uri.path()).decode_utf8() {
        Ok(path) => path,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let relative = path.trim_start_matches('/');
    // Rejects traversal, NUL, backslash, drive letters and NTFS alternate data streams (`:`).
    if relative.contains(['\\', '\0', ':'])
        || PathBuf::from(relative)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let relative = if relative.is_empty() {
        "index.html"
    } else {
        relative
    };
    let mut file_path = root.join(relative);
    if file_path.extension().is_none() {
        file_path = file_path.join("index.html");
    }
    let file = match tokio::fs::File::open(&file_path).await {
        Ok(file) if file.metadata().await.is_ok_and(|m| m.is_file()) => file,
        _ => {
            file_path = root.join("index.html");
            match tokio::fs::File::open(&file_path).await {
                Ok(file) => file,
                Err(_) => return StatusCode::NOT_FOUND.into_response(),
            }
        }
    };
    let metadata = match file.metadata().await {
        Ok(m) if m.is_file() => m,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let mime = mime_guess::from_path(&file_path).first_or_octet_stream();
    let html = mime.essence_str() == "text/html";
    // Deployments can keep an HTML file's size and mtime while changing its bundle URLs, so HTML
    // carries no validators and is always revalidated.
    let modified = if html { None } else { metadata.modified().ok() };
    let etag = modified
        .and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|m| format!("W/\"{:x}-{:x}\"", metadata.len(), m.as_millis()));
    let unchanged = !html
        && match headers
            .get(header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
        {
            Some(value) => value.split(',').any(|v| {
                let v = v.trim();
                v == "*"
                    || etag.as_deref().is_some_and(|e| {
                        v.strip_prefix("W/")
                            .or_else(|| v.strip_prefix("w/"))
                            .unwrap_or(v)
                            == &e[2..]
                    })
            }),
            None => headers
                .get(header::IF_MODIFIED_SINCE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| httpdate::parse_http_date(v).ok())
                .zip(modified.and_then(seconds))
                .is_some_and(|(since, mtime)| mtime <= since),
        };
    let body = if unchanged || method == Method::HEAD {
        Body::empty()
    } else {
        Body::from_stream(ReaderStream::new(file))
    };
    let mut response = Response::new(body);
    *response.status_mut() = if unchanged {
        StatusCode::NOT_MODIFIED
    } else {
        StatusCode::OK
    };
    let relative = file_path
        .strip_prefix(root)
        .ok()
        .and_then(|p| p.to_str())
        .unwrap_or_default()
        .replace('\\', "/");
    // A hash-like name is not enough: custom static files can use the same pattern.
    let immutable =
        !html && hashed_asset_name(&relative) && s.local.immutable_assets.contains(&relative);
    let output = response.headers_mut();
    output.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if immutable {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        }),
    );
    output.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    if html {
        // The app can approve agents and start threads in one click, so no other site may
        // frame it (`SPA_FRAME_HEADERS` in apps/server/src/http.ts, upstream 94aa3d7736).
        output.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("frame-ancestors 'none'"),
        );
        output.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    }
    if !unchanged {
        let content_type = if html {
            "text/html; charset=utf-8"
        } else {
            mime.essence_str()
        };
        if let Ok(v) = content_type.parse() {
            output.insert(header::CONTENT_TYPE, v);
        }
        output.insert(header::CONTENT_LENGTH, HeaderValue::from(metadata.len()));
    }
    if let Some(etag) = etag
        && let Ok(v) = etag.parse()
    {
        output.insert(header::ETAG, v);
    }
    if let Some(mtime) = modified
        && let Ok(v) = httpdate::fmt_http_date(mtime).parse()
    {
        output.insert(header::LAST_MODIFIED, v);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashed_asset_pattern_matches_the_typescript_regex() {
        for (name, expected) in [
            ("assets/app-abcd1234.js", true),
            ("assets/index-DfX_3-ab.css", true),
            ("assets/x/app-abcd1234.js", true),
            ("assets/-abcd1234.js", false),
            ("assets/app-abcd123.js", false),
            ("assets/app-abcd1234", false),
            ("assets/app-abcd1234.", false),
            ("assets/app-abcd12!4.js", false),
            ("assets/app_abcd1234.js", false),
            ("static/app-abcd1234.js", false),
            ("assets/app-abcd1234.js/x", false),
        ] {
            assert_eq!(hashed_asset_name(name), expected, "{name}");
        }
    }

    #[test]
    fn loopback_hosts_ignore_port_case_and_brackets() {
        for host in [
            "localhost",
            "LOCALHOST:5733",
            "127.0.0.1:3773",
            "[::1]:3773",
            "[::1]",
        ] {
            assert!(loopback_host(host), "{host}");
        }
        for host in [
            "example.com",
            "localhost.example.com:80",
            "10.0.0.1",
            "[::2]",
        ] {
            assert!(!loopback_host(host), "{host}");
        }
    }

    #[test]
    fn reserved_prefixes_match_whole_segments() {
        for path in [
            "/api",
            "/api/x",
            "/oauth/token",
            "/.well-known/x",
            "/ws",
            "/mcp",
        ] {
            assert!(is_reserved(path), "{path}");
        }
        for path in ["/", "/apiary", "/wss", "/mcp-docs", "/threads/api"] {
            assert!(!is_reserved(path), "{path}");
        }
    }
}
