use super::{
    GitHub,
    repository::encode,
    transport::{error, now},
};
use crate::{
    FailureReason, Result,
    forge::{MediaRequest, MediaResponse},
};
use futures_util::StreamExt;
use reqwest::Method;
use serde_json::json;
use std::{collections::BTreeMap, sync::LazyLock, time::Duration};

#[derive(Clone)]
pub(super) struct SignedUrl {
    at: u64,
    url: String,
}
fn secure(url: &url::Url) -> bool {
    url.scheme() == "https" && url.username().is_empty() && url.password().is_none()
}
fn credentialed(url: &url::Url) -> bool {
    secure(url)
        && url.port().is_none()
        && matches!(
            url.host_str(),
            Some(
                "github.com"
                    | "www.github.com"
                    | "raw.githubusercontent.com"
                    | "media.githubusercontent.com"
            )
        )
}
fn upload_repository(request: &MediaRequest) -> Option<String> {
    static ATTACHMENT: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"^/(?:user-attachments/assets/[\w-]+|[^/]+/[^/]+/assets/\d+/[\w-]+)$")
            .expect("attachment regex")
    });
    static REPOSITORY: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^[\w.-]+/[\w.-]+$").expect("repository regex"));
    let url = url::Url::parse(&request.url).ok()?;
    if !secure(&url)
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !ATTACHMENT.is_match(url.path())
    {
        return None;
    }
    let repository = if url.path().starts_with("/user-attachments/") {
        request.repository.clone()?
    } else {
        url.path()
            .trim_start_matches('/')
            .split('/')
            .take(2)
            .collect::<Vec<_>>()
            .join("/")
    };
    REPOSITORY.is_match(&repository).then_some(repository)
}
fn content_type(response: &MediaResponse) -> &str {
    response
        .headers
        .get("content-type")
        .and_then(|v| v.split(';').next())
        .map(str::trim)
        .unwrap_or("")
}
impl GitHub {
    async fn follow_media(
        &self,
        raw: &str,
        headers: &BTreeMap<String, String>,
        token: Option<&str>,
    ) -> Result<Option<MediaResponse>> {
        let mut target = match url::Url::parse(raw) {
            Ok(url) if secure(&url) => url,
            _ => return Ok(None),
        };
        for hop in 0..=3 {
            let actual = self.http.fixture_url(&target);
            let mut request = self
                .http
                .client()
                .get(actual)
                .timeout(Duration::from_secs(60));
            for (key, value) in headers {
                if !matches!(
                    key.to_ascii_lowercase().as_str(),
                    "authorization" | "proxy-authorization" | "cookie" | "host" | "accept-encoding"
                ) {
                    request = request.header(key, value);
                }
            }
            request = request.header("accept-encoding", "identity");
            if let Some(token) = token.filter(|_| credentialed(&target)) {
                request = request.bearer_auth(token);
            }
            let response = request.send().await.map_err(|_| {
                error(
                    "fetchMedia",
                    FailureReason::Failed,
                    "GitHub media request failed.",
                )
            })?;
            let status = response.status().as_u16();
            if (300..400).contains(&status) {
                let next = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| target.join(v).ok());
                let Some(next) = next.filter(|url| secure(url) && hop < 3) else {
                    return Ok(None);
                };
                target = next;
                continue;
            }
            let headers = response
                .headers()
                .iter()
                .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.into())))
                .collect();
            let mut chunks = response.bytes_stream();
            let mut body = Vec::new();
            while let Some(chunk) = chunks.next().await {
                let chunk = chunk.map_err(|_| {
                    error(
                        "fetchMedia",
                        FailureReason::Failed,
                        "GitHub media could not be read.",
                    )
                })?;
                if body.len() + chunk.len() > 64 * 1024 * 1024 {
                    return Err(error(
                        "fetchMedia",
                        FailureReason::Failed,
                        "GitHub media exceeds the response byte limit.",
                    ));
                }
                body.extend_from_slice(&chunk);
            }
            return Ok(Some(MediaResponse {
                status,
                headers,
                body,
            }));
        }
        Ok(None)
    }
    async fn signed_media_url(&self, request: &MediaRequest, repository: &str) -> Option<String> {
        let response = self.http.send(super::transport::Request {
            host: "github.com", cwd: ".", operation: "renderMarkdown", method: Method::POST, path: "markdown", body: Some(json!({"text":format!("![]({})",request.url),"mode":"gfm","context":repository})), graphql: false, interactive: true, accept: "text/html", max_bytes: 8 * 1024 * 1024, timeout: Duration::from_secs(30), if_none_match: None
        }).await.ok()?;
        if response.status != 200 || response.truncated {
            return None;
        }
        static IMAGE: LazyLock<regex::Regex> = LazyLock::new(|| {
            regex::Regex::new(r#"(?i)<img\b[^>]*\ssrc\s*=\s*["']([^"']*)["']"#)
                .expect("image regex")
        });
        for captures in IMAGE.captures_iter(&String::from_utf8_lossy(&response.body)) {
            let url = url::Url::parse(&captures[1].replace("&amp;", "&")).ok()?;
            if secure(&url)
                && url.host_str() == Some("private-user-images.githubusercontent.com")
                && url.port().is_none()
                && url
                    .query_pairs()
                    .any(|(key, value)| key == "jwt" && !value.is_empty())
            {
                return Some(url.into());
            }
        }
        None
    }
    pub(super) async fn media(&self, request: MediaRequest) -> Result<Option<MediaResponse>> {
        let credential = self
            .http
            .personal_credential("github.com", std::path::Path::new("."))
            .await
            .ok();
        let repository = credential
            .as_ref()
            .and_then(|_| upload_repository(&request));
        let key = credential.as_ref().zip(repository.as_ref()).map(|(c, r)| {
            format!(
                "{}|{}|{}",
                encode(&c.fingerprint),
                encode(r),
                encode(&request.url)
            )
        });
        let cached = if let Some(key) = &key {
            self.signed_urls
                .lock()
                .await
                .get(key)
                .filter(|c| now().saturating_sub(c.at) < 240_000)
                .cloned()
        } else {
            None
        };
        let response = if let Some(cached) = cached {
            let response = self
                .follow_media(&cached.url, &request.headers, None)
                .await?;
            if response.as_ref().is_none_or(|r| r.status >= 400)
                && let Some(key) = &key
            {
                self.signed_urls.lock().await.remove(key);
            }
            if response.as_ref().is_none_or(|r| r.status != 403) {
                return Ok(response);
            }
            response
        } else {
            if let Some(key) = &key {
                self.signed_urls.lock().await.remove(key);
            }
            let response = self
                .follow_media(
                    &request.url,
                    &request.headers,
                    credential.as_ref().map(|c| c.token.as_str()),
                )
                .await?;
            if response
                .as_ref()
                .is_none_or(|r| !content_type(r).eq_ignore_ascii_case("text/html"))
            {
                return Ok(response);
            }
            response
        };
        let Some((key, repository, credential)) = key
            .zip(repository)
            .zip(credential)
            .map(|((k, r), c)| (k, r, c))
        else {
            return Ok(response);
        };
        let Some(signed) = self.signed_media_url(&request, &repository).await else {
            return Ok(response);
        };
        let media = self.follow_media(&signed, &request.headers, None).await?;
        static MEDIA_TYPE: LazyLock<regex::Regex> = LazyLock::new(|| {
            regex::Regex::new(r"(?i)^(?:image|video|audio)/[\w!#$&^.+-]+$")
                .expect("media type regex")
        });
        if media
            .as_ref()
            .is_some_and(|r| matches!(r.status, 200 | 206) && MEDIA_TYPE.is_match(content_type(r)))
            && self
                .http
                .personal_credential("github.com", std::path::Path::new("."))
                .await
                .is_ok_and(|c| c.fingerprint == credential.fingerprint)
        {
            let mut cache = self.signed_urls.lock().await;
            if cache.len() >= 64
                && let Some(oldest) = cache
                    .iter()
                    .min_by_key(|(key, c)| (c.at, (*key).clone()))
                    .map(|(key, _)| key.clone())
            {
                cache.remove(&oldest);
            }
            cache.insert(
                key,
                SignedUrl {
                    at: now(),
                    url: signed,
                },
            );
        }
        Ok(media)
    }
}
