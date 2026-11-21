//! Bounded reads of signed preview assets, shared by all client hosts.

use base64::{Engine, engine::general_purpose::STANDARD};
use futures::{FutureExt, future::LocalBoxFuture};
use t3_contracts::assets::AssetResource;

use super::ProjectFiles;
use crate::{
    platform::{Credentials, HttpRequest, Platform, PlatformError},
    types::EnvironmentId,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AssetReadError {
    #[error("the preview asset is unavailable")]
    Unavailable,
    #[error("the preview asset exceeds the byte limit")]
    TooLarge,
}

impl<P: Platform + Clone + 'static> ProjectFiles<P> {
    /// Mint a fresh URL and read the file under the host HTTP adapter's streaming limit.
    pub fn asset_bytes(
        &self,
        environment: &EnvironmentId,
        resource: AssetResource,
        limit: usize,
        revision: Option<String>,
    ) -> LocalBoxFuture<'static, Result<Vec<u8>, AssetReadError>> {
        let signed = self.asset_url(environment, resource);
        let platform = self.platform.clone();
        async move {
            let url = signed.await.map_err(|_| AssetReadError::Unavailable)?;
            read_signed_asset(&platform, &url, limit, revision.as_deref()).await
        }
        .boxed_local()
    }
}

async fn read_signed_asset(
    platform: &impl Platform,
    url: &str,
    limit: usize,
    revision: Option<&str>,
) -> Result<Vec<u8>, AssetReadError> {
    let mut url = url::Url::parse(url).map_err(|_| AssetReadError::Unavailable)?;
    if let Some(revision) = revision {
        url.query_pairs_mut()
            .append_pair("workspace-revision", revision);
    }
    let response = platform
        .http(HttpRequest {
            method: "GET".into(),
            url: url.into(),
            headers: Default::default(),
            body: Vec::new(),
            credentials: Credentials::Omit,
            max_response_bytes: limit,
        })
        .await
        .map_err(|error| match error {
            PlatformError::TooLarge => AssetReadError::TooLarge,
            _ => AssetReadError::Unavailable,
        })?;
    if response.status != 200 {
        return Err(AssetReadError::Unavailable);
    }
    if response.body.len() > limit {
        return Err(AssetReadError::TooLarge);
    }
    Ok(response.body)
}

pub fn preview_data_url(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", STANDARD.encode(bytes))
}

/// The extended formats presently mounted for a sent file, with the web's MIME normalization.
#[derive(Clone, Copy)]
pub enum AttachmentPreview {
    Raster,
    Notebook,
    Binary,
}

pub fn attachment_preview(name: &str, mime: &str) -> Option<AttachmentPreview> {
    use super::preview::{self, PathKinds};

    let mime = mime
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let generic = preview::is_generic_mime(&mime);
    let kinds = PathKinds::of(name, false);
    if mime == "application/x-ipynb+json"
        || ((generic || mime == "application/json")
            && preview::office_preview_kind(name) == Some("ipynb"))
    {
        Some(AttachmentPreview::Notebook)
    } else if mime.starts_with("image/") || (generic && kinds.image) {
        Some(AttachmentPreview::Raster)
    } else if matches!(
        mime.as_str(),
        "application/zip"
            | "application/x-zip-compressed"
            | "application/java-archive"
            | "application/x-java-archive"
            | "application/x-tar"
            | "application/gzip"
            | "application/x-gzip"
            | "application/x-compressed-tar"
            | "application/vnd.sqlite3"
            | "application/x-sqlite3"
            | "application/x-sqlite"
    ) || (generic && preview::binary_preview_kind(name).is_some())
        || (mime == "application/octet-stream"
            && !kinds.media()
            && !kinds.document
            && !kinds.pdf
            && kinds.rendered_mode().is_none())
    {
        Some(AttachmentPreview::Binary)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{platform::HttpResponse, testing::FakePlatform};
    use futures::executor::block_on;

    #[test]
    fn preview_reads_give_the_host_a_byte_cap_and_omit_credentials() {
        let platform = FakePlatform::default();
        platform.push_http(Ok(HttpResponse {
            status: 200,
            headers: Default::default(),
            body: vec![0; 5],
        }));
        let signed = "https://environment.test/preview?token=signed";
        let result = block_on(read_signed_asset(&platform, signed, 4, Some("a &=é")));
        assert_eq!(result, Err(AssetReadError::TooLarge));
        assert_eq!(
            platform.http_requests(),
            [HttpRequest {
                method: "GET".into(),
                url: format!("{signed}&workspace-revision=a+%26%3D%C3%A9"),
                headers: Default::default(),
                body: Vec::new(),
                credentials: Credentials::Omit,
                max_response_bytes: 4,
            }]
        );
    }
}
