//! The platform caps the HTTP body during transfer (native chunks / browser ReadableStream).
//! Registry requests omit credentials. Each redirect destination is checked before contacting it.
use futures::future::{Either, select};
use nsh_core::prelude::{provide_context, use_context};
use std::rc::Rc;
use t3_client::platform::{Credentials, HttpRequest, Platform};
use t3_theme::import::{
    HttpError, HttpMethod, HttpResponse, MAX_DETAIL_BYTES, MAX_MANIFEST_BYTES, MAX_SEARCH_BYTES,
    MAX_VSIX_BYTES, OPEN_VSX_SEARCH_URL, OpenVsxError, OpenVsxHttp, OpenVsxThemeExtension,
    SEARCH_REQUEST_TIMEOUT_MS, open_vsx_url,
};

#[derive(Clone)]
pub(crate) struct ThemePlatform(Rc<dyn Platform>);

pub(crate) fn bind_platform(platform: impl Platform + 'static) {
    provide_context(ThemePlatform(Rc::new(platform)));
}

pub(crate) struct Transport {
    platform: ThemePlatform,
    extension: Option<OpenVsxThemeExtension>,
}

impl ThemePlatform {
    pub(crate) fn current() -> Option<Self> {
        use_context()
    }

    pub(crate) fn transport(&self, extension: Option<&OpenVsxThemeExtension>) -> Transport {
        Transport {
            platform: self.clone(),
            extension: extension.cloned(),
        }
    }
}

impl Transport {
    fn limit(&self, method: HttpMethod, url: &str) -> Result<u64, HttpError> {
        if open_vsx_url(url).is_none() {
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
        if url.split('?').next() == Some(OPEN_VSX_SEARCH_URL) {
            return Ok(MAX_SEARCH_BYTES);
        }
        Ok(MAX_DETAIL_BYTES)
    }
}

const MAX_REDIRECTS: usize = 3;
const PACKAGE_TIMEOUT_MS: u64 = 120_000;

#[cfg(any(target_arch = "wasm32", test))]
fn browser_file_url(raw: &str) -> Option<String> {
    // Fetch hides Location on manual redirects. Automatic follow contacts each hop before
    // a final-URL check could reject it, so keep redirects disabled in the browser.
    // Open VSX's IStorageService.getObjectKey
    // uses namespace/extension/[targetPlatform/]version/resource on the public CDN.
    let url = open_vsx_url(raw)?;
    if url.host_str()? != "open-vsx.org" || url.port().is_some() || url.query().is_some() {
        return None;
    }
    let segments: Vec<_> = url.path_segments()?.collect();
    let (key, file) = match segments.as_slice() {
        ["api", namespace, extension, version, "file", file @ ..] => {
            (vec![*namespace, *extension, *version], file)
        }
        [
            "api",
            namespace,
            extension,
            platform,
            version,
            "file",
            file @ ..,
        ] => (vec![*namespace, *extension, *platform, *version], file),
        _ => return None,
    };
    if file.is_empty() || key.iter().chain(file).any(|segment| segment.is_empty()) {
        return None;
    }
    Some(format!(
        "https://openvsx.eclipsecontent.org/{}/{}",
        key.join("/"),
        file.join("/")
    ))
}

impl Transport {
    async fn fetch(
        &self,
        method: HttpMethod,
        url: &str,
        limit: u64,
    ) -> Result<HttpResponse, HttpError> {
        #[cfg(target_arch = "wasm32")]
        let browser_url = browser_file_url(url);
        #[cfg(target_arch = "wasm32")]
        let url = browser_url.as_deref().unwrap_or(url);

        let mut current =
            open_vsx_url(url).ok_or_else(|| HttpError("Untrusted Open VSX URL.".into()))?;
        let mut redirects = 0;
        loop {
            let request = HttpRequest {
                url: current.to_string(),
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
            let response = self
                .platform
                .0
                .http_without_redirect_body(request)
                .await
                .map_err(|e| HttpError(e.to_string()))?;
            if matches!(response.status, 301 | 302 | 303 | 307 | 308) {
                if redirects == MAX_REDIRECTS {
                    return Err(HttpError("Too many Open VSX redirects.".into()));
                }
                redirects += 1;
                current = response
                    .headers
                    .get("location")
                    .and_then(|location| current.join(location).ok())
                    .and_then(|next| open_vsx_url(next.as_str()))
                    .ok_or_else(|| HttpError("Untrusted Open VSX redirect.".into()))?;
                continue;
            }
            return Ok(HttpResponse {
                status: response.status,
                content_length: response
                    .headers
                    .get("content-length")
                    .and_then(|v| v.parse().ok()),
                body: response.body,
            });
        }
    }

    fn request_timeout(&self, method: HttpMethod, url: &str) -> u64 {
        if method == HttpMethod::Get
            && self
                .extension
                .as_ref()
                .is_some_and(|ext| url == ext.vsix_url)
        {
            PACKAGE_TIMEOUT_MS
        } else {
            SEARCH_REQUEST_TIMEOUT_MS
        }
    }
}

impl OpenVsxHttp for Transport {
    async fn with_detail_timeout<T>(
        &self,
        work: impl Future<Output = Result<T, OpenVsxError>>,
    ) -> Result<T, OpenVsxError> {
        match select(
            std::pin::pin!(work),
            self.platform.0.sleep(SEARCH_REQUEST_TIMEOUT_MS),
        )
        .await
        {
            Either::Left((result, _)) => result,
            Either::Right(_) => Err(OpenVsxError::Message(
                "Open VSX took too long to respond.".into(),
            )),
        }
    }

    async fn request(&self, method: HttpMethod, url: &str) -> Result<HttpResponse, HttpError> {
        let limit = self.limit(method, url)?;
        let transfer = self.fetch(method, url, limit);
        let timeout = self.platform.0.sleep(self.request_timeout(method, url));
        match select(std::pin::pin!(transfer), timeout).await {
            Either::Left((result, _)) => result,
            Either::Right(_) => Err(HttpError("Open VSX took too long to respond.".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use t3_client::{platform::HttpResponse as Reply, testing::FakePlatform};
    use t3_theme::import::zip::ZipArchive;

    #[test]
    fn registry_redirects_validate_each_destination_before_sending() {
        let initial = "https://open-vsx.org/api/demo/theme";
        for (location, allowed) in [
            (
                "https://openvsx.eclipsecontent.org/demo/theme/1/package.json",
                true,
            ),
            ("/new/package.json", true),
            ("https://example.org/package.json", false),
            ("http://openvsx.eclipsecontent.org/package.json", false),
            ("https://open-vsx.org@example.org/package.json", false),
        ] {
            let platform = FakePlatform::default();
            platform.push_http(Ok(Reply {
                status: 302,
                headers: [("location".into(), location.into())].into(),
                body: vec![],
            }));
            platform.push_http(Ok(Reply {
                status: 200,
                headers: Default::default(),
                body: b"manifest".to_vec(),
            }));
            let transport = ThemePlatform(Rc::new(platform.clone())).transport(None);
            let result = block_on(transport.request(HttpMethod::Get, initial));
            if allowed {
                assert_eq!(result.expect("allowed redirect").body, b"manifest");
                let requests = platform.http_requests();
                assert_eq!(requests.len(), 2);
                let expected = open_vsx_url(initial)
                    .expect("registry URL")
                    .join(location)
                    .expect("redirect");
                assert_eq!(requests[1].url, expected.as_str());
                assert_eq!(
                    platform.http_requests()[1].max_response_bytes,
                    MAX_DETAIL_BYTES as usize
                );
            } else {
                assert!(result.is_err(), "{location}");
                assert_eq!(platform.http_requests().len(), 1);
            }
        }
    }

    #[test]
    fn registry_redirects_share_a_hop_limit_and_body_cap() {
        let platform = FakePlatform::default();
        let initial = "https://open-vsx.org/api/demo/theme";
        let redirect = || Reply {
            status: 302,
            headers: [("location".into(), "/again".into())].into(),
            body: vec![],
        };
        for _ in 0..=MAX_REDIRECTS {
            platform.push_http(Ok(redirect()));
        }
        let http = ThemePlatform(Rc::new(platform.clone())).transport(None);
        let error = block_on(http.request(HttpMethod::Get, initial)).expect_err("redirect limit");
        assert!(error.0.contains("Too many"));
        assert_eq!(platform.http_requests().len(), MAX_REDIRECTS + 1);
        platform.push_http(Ok(redirect()));
        platform.push_http(Ok(Reply {
            status: 200,
            headers: Default::default(),
            body: vec![0; MAX_DETAIL_BYTES as usize + 1],
        }));
        let error =
            block_on(http.request(HttpMethod::Get, initial)).expect_err("redirected body cap");
        assert!(error.0.contains("byte limit"));
    }

    #[test]
    fn browser_uses_the_public_content_key_for_versioned_file_urls_only() {
        for (path, key) in [
            (
                "acme/theme/1.0/file/package.json",
                "acme/theme/1.0/package.json",
            ),
            (
                "acme/theme/linux-x64/1.0/file/a/b%20c",
                "acme/theme/linux-x64/1.0/a/b%20c",
            ),
        ] {
            assert_eq!(
                browser_file_url(&format!("https://open-vsx.org/api/{path}")),
                Some(format!("https://openvsx.eclipsecontent.org/{key}"))
            );
        }
        for url in [
            "https://open-vsx.org/api/acme/theme",
            "https://open-vsx.org/api/-/search",
            "http://open-vsx.org/api/acme/theme/1/file/package.json",
            "https://example.com/api/acme/theme/1/file/package.json",
        ] {
            assert!(browser_file_url(url).is_none());
        }
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    #[ignore = "contacts the live Open VSX registry"]
    fn live_registry_returns_installable_dracula_themes() {
        use t3_platform::{EphemeralSecretStore, NativeConfig, NativePlatform};
        use t3_theme::import::{NoCancel, OpenVsxClient, OpenVsxThemeSort};
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("I/O runtime");
        runtime.block_on(async {
            let data =
                std::env::temp_dir().join(format!("t3-theme-registry-{}", std::process::id()));
            std::fs::create_dir(&data).expect("fresh test directory");
            let config = NativeConfig::new(data.clone(), Box::new(EphemeralSecretStore::default()));
            let platform = ThemePlatform(Rc::new(
                NativePlatform::open(config).await.expect("native HTTP"),
            ));
            let results = OpenVsxClient::new(platform.transport(None))
                .search("dracula", OpenVsxThemeSort::DownloadCount, &NoCancel)
                .await
                .expect("live registry search");
            let selected = results.first().expect("installable theme");
            let themes = OpenVsxClient::new(platform.transport(Some(selected)))
                .import(selected, &NoCancel)
                .await
                .expect("checksum-verified live install");
            assert!(!themes.is_empty());
            println!(
                "Search returned {} results. Imported {} theme(s) from {} {}.",
                results.len(),
                themes.len(),
                selected.id,
                selected.version
            );
            drop(platform);
            std::fs::remove_dir_all(data).expect("remove temporary data");
        });
    }

    #[test]
    fn fake_platform_rejects_a_package_larger_than_the_transport_cap() {
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
        assert_eq!(
            http.request_timeout(HttpMethod::Get, &extension.manifest_url),
            10_000
        );
        assert_eq!(
            http.request_timeout(HttpMethod::Get, &extension.vsix_url),
            120_000
        );
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
