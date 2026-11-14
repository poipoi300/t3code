use super::AttachmentView;
use crate::view_logic::JsTrim;
use crate::{
    ClientError,
    platform::{Credentials, HttpRequest, Platform},
};
use base64::Engine;
use futures::future::try_join_all;
use std::{collections::BTreeMap, future::Future};
use t3_contracts::{
    assets::{AssetCreateUrlInput, AssetCreateUrlResult, AssetResource},
    base_schemas::TrimmedNonEmptyString,
    chat_attachment::{ChatAttachment, ChatFileAttachment, PROVIDER_SEND_TURN_MAX_FILE_BYTES},
};

/// The video MIME type a file plays as (`videoMimeType`), from its type or, when that is generic, its extension.
pub fn video_mime_type(name: &str, mime: &str) -> Option<String> {
    let mime = mime
        .split(';')
        .next()
        .unwrap_or("")
        .js_trim()
        .to_lowercase();
    let generic = matches!(
        mime.as_str(),
        "application/octet-stream"
            | "binary/octet-stream"
            | "application/unknown"
            | "video/mp2t"
            | "video/vnd.dlna.mpeg-tts"
    );
    if mime.starts_with("video/") && !generic {
        return Some(mime);
    }
    if !mime.is_empty() && !generic {
        return None;
    }
    let extension = name.rsplit_once('.')?.1.to_lowercase();
    match extension.as_str() {
        "avi" => Some("video/x-msvideo"),
        "m4v" | "mp4" => Some("video/mp4"),
        "mkv" => Some("video/x-matroska"),
        "mov" => Some("video/quicktime"),
        "ogv" => Some("video/ogg"),
        "webm" => Some("video/webm"),
        _ => None,
    }
    .map(str::to_owned)
}

pub async fn read_file_as_data_url<P: Platform + ?Sized>(
    platform: &P,
    resource: &crate::platform::HostResource,
    max_bytes: usize,
) -> Result<String, ClientError> {
    let bytes = platform
        .read_resource(resource.handle.clone(), max_bytes)
        .await?;
    let mime = if resource.mime.is_empty() {
        "application/octet-stream"
    } else {
        &resource.mime
    };
    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

fn attachment_resource(id: &str, name: &str, mime: &str) -> AssetCreateUrlInput {
    AssetCreateUrlInput {
        resource: AssetResource::Attachment {
            attachment_id: TrimmedNonEmptyString(id.into()),
            file_name: Some(TrimmedNonEmptyString(name.into())),
            mime_type: Some(TrimmedNonEmptyString(mime.into())),
            disposition: None,
            render_intent: None,
        },
    }
}

fn resolve_url(base: &str, relative: &str) -> Result<String, ClientError> {
    url::Url::parse(base)
        .and_then(|base| base.join(relative))
        .map(Into::into)
        .map_err(|_| {
            ClientError::InvalidPayload(
                "The environment returned an invalid attachment URL.".into(),
            )
        })
}

pub async fn resolve_file_attachment_url<F, Fut>(
    attachment: &ChatFileAttachment,
    http_base_url: &str,
    create_asset_url: F,
) -> Result<String, ClientError>
where
    F: FnOnce(AssetCreateUrlInput) -> Fut,
    Fut: Future<Output = Result<AssetCreateUrlResult, ClientError>>,
{
    let mime = video_mime_type(&attachment.name, &attachment.mime_type)
        .unwrap_or_else(|| attachment.mime_type.clone());
    let asset =
        create_asset_url(attachment_resource(&attachment.id, &attachment.name, &mime)).await?;
    resolve_url(http_base_url, &asset.relative_url.0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredAttachment {
    pub name: String,
    pub mime_type: String,
    pub bytes: Vec<u8>,
}

pub async fn prepare_reverted_message_attachments<P, F, Fut>(
    platform: &P,
    attachments: &[AttachmentView],
    http_base_url: &str,
    create_asset_url: F,
) -> Result<Vec<RestoredAttachment>, ClientError>
where
    P: Platform + ?Sized,
    F: Fn(AssetCreateUrlInput) -> Fut,
    Fut: Future<Output = Result<AssetCreateUrlResult, ClientError>>,
{
    try_join_all(attachments.iter().map(|attachment| async {
        let (id, name, mime) = match &attachment.attachment {
            ChatAttachment::Image(a) => (&a.id, &a.name, &a.mime_type),
            ChatAttachment::File(a) => (&a.id, &a.name, &a.mime_type),
            ChatAttachment::Unknown(_) => {
                return Err(ClientError::InvalidPayload(
                    "This message has an attachment that cannot be restored.".into(),
                ));
            }
        };
        let asset = create_asset_url(attachment_resource(id, name, mime)).await?;
        let url = resolve_url(http_base_url, &asset.relative_url.0)?;
        let request = platform.http(HttpRequest {
            method: "GET".into(),
            url,
            headers: BTreeMap::new(),
            body: Vec::new(),
            credentials: Credentials::SameOrigin,
            max_response_bytes: PROVIDER_SEND_TURN_MAX_FILE_BYTES as usize,
        });
        let timeout = platform.sleep(30_000);
        futures::pin_mut!(request, timeout);
        let response = match futures::future::select(request, timeout).await {
            futures::future::Either::Left((response, _)) => response?,
            futures::future::Either::Right(_) => return Err(ClientError::Cancelled),
        };
        if !(200..300).contains(&response.status) {
            return Err(ClientError::InvalidPayload(format!(
                "Could not restore attachment: {name}"
            )));
        }
        Ok(RestoredAttachment {
            name: name.clone(),
            mime_type: mime.clone(),
            bytes: response.body,
        })
    }))
    .await
}
