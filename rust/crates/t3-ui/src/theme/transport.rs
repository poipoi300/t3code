//! The platform caps the HTTP body during transfer (native chunks / browser ReadableStream).
//! Native does not follow redirects, and web fetch sets RequestRedirect::Error.
use futures::future::{Either, select};
use nsh_core::prelude::{provide_context, use_context};
use std::{cell::Cell, rc::Rc};
use t3_client::platform::{Credentials, HttpRequest, Platform};
use t3_theme::import::{
    HttpError, HttpMethod, HttpResponse, MAX_DETAIL_BYTES, MAX_MANIFEST_BYTES, MAX_SEARCH_BYTES,
    MAX_VSIX_BYTES, OPEN_VSX_SEARCH_URL, OpenVsxHttp, OpenVsxThemeExtension,
    SEARCH_REQUEST_TIMEOUT_MS,
};

#[derive(Clone)]
pub(crate) struct ThemePlatform(Rc<dyn Platform>);

pub(crate) fn bind_platform(platform: impl Platform + 'static) {
    provide_context(ThemePlatform(Rc::new(platform)));
}

pub(crate) struct Transport {
    platform: ThemePlatform,
    extension: Option<OpenVsxThemeExtension>,
    search_index: Cell<bool>,
    detail_deadline: Cell<Option<u64>>,
}

impl ThemePlatform {
    pub(crate) fn current() -> Option<Self> {
        use_context()
    }

    pub(crate) fn transport(&self, extension: Option<&OpenVsxThemeExtension>) -> Transport {
        Transport {
            platform: self.clone(),
            extension: extension.cloned(),
            search_index: Cell::new(true),
            detail_deadline: Cell::new(None),
        }
    }
}

impl Transport {
    fn limit(&self, method: HttpMethod, url: &str) -> Result<u64, HttpError> {
        // The catalogue normalises these URLs when decoding detail records. Match the
        // HTTPS hostname again at the boundary, including an optional numeric port.
        let authority = url
            .strip_prefix("https://")
            .and_then(|rest| rest.split_once('/'))
            .map(|(authority, _)| authority);
        let trusted = authority.is_some_and(|a| {
            a == "open-vsx.org"
                || a.strip_prefix("open-vsx.org:")
                    .is_some_and(|port| port.parse::<u16>().is_ok())
        });
        if !trusted {
            return Err(HttpError("Untrusted Open VSX URL.".into()));
        }
        if let Some(ext) = &self.extension {
            if url == ext.sha256_url {
                return Ok(256);
            }
            if url == ext.manifest_url {
                return Ok(MAX_MANIFEST_BYTES);
            }
            if url == ext.vsix_url {
                return Ok(MAX_VSIX_BYTES);
            }
            return Err(HttpError("Unexpected Open VSX package URL.".into()));
        }
        if method == HttpMethod::Head {
            return Ok(MAX_VSIX_BYTES);
        }
        if self.search_index.replace(false) && url.split('?').next() == Some(OPEN_VSX_SEARCH_URL) {
            return Ok(MAX_SEARCH_BYTES);
        }
        Ok(MAX_DETAIL_BYTES)
    }
}

impl OpenVsxHttp for Transport {
    async fn request(&self, method: HttpMethod, url: &str) -> Result<HttpResponse, HttpError> {
        let limit = self.limit(method, url)?;
        // Each search result has a single ten-second budget for its detail, manifest
        // and package HEAD requests, as in withSearchTimeout.
        let now = self.platform.0.monotonic_ms();
        let detail_url = url
            .strip_prefix("https://open-vsx.org/api/")
            .is_some_and(|path| path.split('/').count() == 2 && !path.starts_with("-/"));
        if self.extension.is_none() && detail_url && method == HttpMethod::Get {
            self.detail_deadline
                .set(Some(now.saturating_add(SEARCH_REQUEST_TIMEOUT_MS)));
        }
        let timeout = self
            .detail_deadline
            .get()
            .map_or(SEARCH_REQUEST_TIMEOUT_MS, |deadline| {
                deadline.saturating_sub(now)
            });
        if timeout == 0 {
            return Err(HttpError("Open VSX took too long to respond.".into()));
        }
        let request = HttpRequest {
            url: url.into(),
            method: match method {
                HttpMethod::Get => "GET",
                HttpMethod::Head => "HEAD",
            }
            .into(),
            headers: Default::default(),
            body: Vec::new(),
            credentials: Credentials::Omit,
            max_response_bytes: limit as usize,
        };
        let platform = &self.platform.0;
        let response = match select(platform.http(request), platform.sleep(timeout)).await {
            Either::Left((response, _)) => response.map_err(|err| HttpError(err.to_string()))?,
            Either::Right(_) => return Err(HttpError("Open VSX took too long to respond.".into())),
        };
        Ok(HttpResponse {
            status: response.status,
            content_length: response
                .headers
                .get("content-length")
                .and_then(|v| v.parse().ok()),
            body: response.body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use t3_client::{platform::HttpResponse as Reply, testing::FakePlatform};
    use t3_theme::import::zip::ZipArchive;

    #[test]
    fn themes_oversize_response_without_a_length_is_stopped_by_the_host() {
        let extension = OpenVsxThemeExtension {
            id: "acme.night".into(),
            collection_id: "open-vsx:acme.night".into(),
            name: "Night".into(),
            publisher: "acme".into(),
            description: String::new(),
            download_count: 0.,
            icon_url: None,
            source_url: None,
            manifest_url: "https://open-vsx.org/api/acme/night/package.json".into(),
            sha256_url: "https://open-vsx.org/api/acme/night/checksum".into(),
            vsix_url: "https://open-vsx.org/api/acme/night/package".into(),
            version: "1".into(),
            license: "MIT".into(),
        };
        let platform = FakePlatform::default();
        platform.push_http(Ok(Reply {
            status: 200,
            headers: Default::default(),
            body: vec![0; MAX_VSIX_BYTES as usize + 1],
        }));
        let http = ThemePlatform(Rc::new(platform.clone())).transport(Some(&extension));
        let error = block_on(http.request(HttpMethod::Get, &extension.vsix_url))
            .expect_err("host enforces streaming cap");
        assert!(error.0.contains("byte limit"));
        assert_eq!(
            platform.http_requests()[0].max_response_bytes,
            MAX_VSIX_BYTES as usize
        );
    }

    #[test]
    fn themes_import_rejects_archive_with_a_traversal_entry() {
        // A valid, empty ZIP member whose name tries to leave the extension directory.
        let archive = include_bytes!("testdata/outside.vsix");
        assert!(
            ZipArchive::open(archive)
                .expect_err("unsafe member")
                .to_string()
                .contains("escapes")
        );
    }
}
