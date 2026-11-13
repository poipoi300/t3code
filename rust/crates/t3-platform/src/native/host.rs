use super::NativePlatform;
use crate::common::{
    decode_device_identity, device_identity_key, dpop_public_jwk, dpop_thumbprint,
    encode_device_identity, unsupported, uuid_v4,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::{
    rand::SystemRandom,
    signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair},
};
use std::rc::Rc;
use t3_client::{
    persistence::{Record, decode_record},
    platform::{
        HostAction, HostReply, Platform, PlatformError, Storage, StorageArea, StorageCondition,
        StorageKey, StorageTransaction, StorageWrite,
    },
};

#[derive(Default)]
pub(super) struct HostState {
    dpop: futures::lock::Mutex<Option<Rc<EcdsaKeyPair>>>,
    device: futures::lock::Mutex<Option<String>>,
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    clipboard: std::sync::Arc<std::sync::Mutex<Option<arboard::Clipboard>>>,
}

const DPOP_KIND: &str = "dpop-key";
const DPOP_RECORD: &str = "relay";
fn dpop_key() -> StorageKey {
    StorageKey::new(DPOP_KIND, None, DPOP_RECORD)
}

pub(super) async fn dispatch(
    platform: &NativePlatform,
    action: HostAction,
) -> Result<HostReply, PlatformError> {
    match action {
        HostAction::DpopThumbprint => {
            let key = load_dpop_key(platform).await?;
            let (x, y) = coordinates(&key)?;
            Ok(HostReply::Text(Some(dpop_thumbprint(x, y))))
        }
        HostAction::DpopPublicJwk => {
            let key = load_dpop_key(platform).await?;
            let (x, y) = coordinates(&key)?;
            Ok(HostReply::Document(dpop_public_jwk(x, y)))
        }
        HostAction::DpopSign { signing_input } => {
            let key = load_dpop_key(platform).await?;
            let signature = key
                .sign(&SystemRandom::new(), &signing_input)
                .map_err(|_| PlatformError::InvalidData("DPoP signature".into()))?;
            Ok(HostReply::Bytes(signature.as_ref().to_vec()))
        }
        HostAction::AnalyticsDeviceId => Ok(HostReply::Text(Some(device_id(platform).await?))),
        // Mobile shares one persisted id with analytics; desktop and web send none.
        #[cfg(any(target_os = "android", target_os = "ios"))]
        HostAction::RelayDeviceId => Ok(HostReply::Text(Some(device_id(platform).await?))),
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        HostAction::RelayDeviceId => Ok(HostReply::Text(None)),
        HostAction::ClientPresentation if platform.inner.presentation.is_some() => {
            serde_json::to_value(&platform.inner.presentation)
                .map(HostReply::Document)
                .map_err(|_| PlatformError::InvalidData("client presentation".into()))
        }
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        HostAction::ClipboardRead => desktop::clipboard_read(platform).await,
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        HostAction::ClipboardWrite(data) => desktop::clipboard_write(platform, data).await,
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        HostAction::OpenUrl(url) => desktop::open_url(platform, url).await,
        action => match &platform.inner.extension {
            Some(extension) => extension.host(action).await,
            None => Err(unsupported(&action)),
        },
    }
}

fn coordinates(key: &EcdsaKeyPair) -> Result<(&[u8], &[u8]), PlatformError> {
    match key.public_key().as_ref() {
        [0x04, point @ ..] if point.len() == 64 => Ok(point.split_at(32)),
        _ => Err(PlatformError::InvalidData("DPoP public key".into())),
    }
}

/// Reads or creates a record once; a concurrent creator (another process) wins the condition.
async fn load_or_create(
    platform: &NativePlatform,
    key: StorageKey,
    create: impl Fn() -> Result<Vec<u8>, PlatformError>,
) -> Result<Vec<u8>, PlatformError> {
    for _ in 0..2 {
        if let Some(bytes) = platform.get(StorageArea::Secure, key.clone()).await? {
            return Ok(bytes);
        }
        let bytes = create()?;
        let committed = platform
            .transaction(StorageTransaction {
                conditions: vec![StorageCondition {
                    area: StorageArea::Secure,
                    key: key.clone(),
                    expected: None,
                }],
                ordinary: vec![],
                secure: vec![StorageWrite {
                    key: key.clone(),
                    value: Some(bytes.clone()),
                }],
            })
            .await?;
        if committed {
            return Ok(bytes);
        }
    }
    Err(PlatformError::Storage("record changed concurrently".into()))
}

async fn load_dpop_key(platform: &NativePlatform) -> Result<Rc<EcdsaKeyPair>, PlatformError> {
    let mut cached = platform.inner.host.dpop.lock().await;
    if let Some(key) = cached.as_ref() {
        return Ok(key.clone());
    }
    let invalid = || PlatformError::InvalidData("stored DPoP key".into());
    let bytes = load_or_create(platform, dpop_key(), || {
        let document =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
                .map_err(|_| PlatformError::Unsupported("OS randomness is unavailable".into()))?;
        let payload = serde_json::json!({ "pkcs8": URL_SAFE_NO_PAD.encode(document.as_ref()) });
        Record::new(DPOP_KIND, None, DPOP_RECORD, 1, payload)
            .encode()
            .map_err(|_| invalid())
    })
    .await?;
    let record =
        decode_record::<serde_json::Value>(&bytes, &dpop_key(), 1).map_err(|_| invalid())?;
    let pkcs8 = record.payload["pkcs8"]
        .as_str()
        .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
        .ok_or_else(invalid)?;
    let key = Rc::new(
        EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_FIXED_SIGNING,
            &pkcs8,
            &SystemRandom::new(),
        )
        .map_err(|_| invalid())?,
    );
    *cached = Some(key.clone());
    Ok(key)
}

async fn device_id(platform: &NativePlatform) -> Result<String, PlatformError> {
    let mut cached = platform.inner.host.device.lock().await;
    if let Some(id) = cached.as_ref() {
        return Ok(id.clone());
    }
    let bytes = load_or_create(platform, device_identity_key(), || {
        let random = platform.random_bytes(16)?;
        let mut seed = [0_u8; 16];
        seed.copy_from_slice(&random);
        encode_device_identity(&uuid_v4(seed))
    })
    .await?;
    let id = decode_device_identity(&bytes)?;
    *cached = Some(id.clone());
    Ok(id)
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod desktop {
    use super::*;
    use crate::common::{TEXT_HTML, TEXT_PLAIN, mime_essence, validate_open_url};
    use t3_client::platform::MimeData;

    async fn with_clipboard<T: Send + 'static>(
        platform: &NativePlatform,
        work: impl FnOnce(&mut arboard::Clipboard) -> Result<T, PlatformError> + Send + 'static,
    ) -> Result<T, PlatformError> {
        let clipboard = platform.inner.host.clipboard.clone();
        platform
            .inner
            .runtime
            .spawn_blocking(move || {
                let mut slot = clipboard
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if slot.is_none() {
                    // Kept alive: on X11 the copied text lives only as long as its owner.
                    *slot = Some(arboard::Clipboard::new().map_err(clipboard_error)?);
                }
                match slot.as_mut() {
                    Some(clipboard) => work(clipboard),
                    None => Err(PlatformError::Unsupported("clipboard".into())),
                }
            })
            .await
            .map_err(|_| PlatformError::Cancelled)?
    }

    fn clipboard_error(error: arboard::Error) -> PlatformError {
        match error {
            arboard::Error::ClipboardOccupied => {
                PlatformError::Denied("the clipboard is busy".into())
            }
            other => PlatformError::Unsupported(format!("clipboard unavailable: {other}")),
        }
    }

    pub(super) async fn clipboard_read(
        platform: &NativePlatform,
    ) -> Result<HostReply, PlatformError> {
        with_clipboard(platform, |clipboard| match clipboard.get_text() {
            Ok(text) => Ok(HostReply::Mime(vec![MimeData {
                mime: TEXT_PLAIN.into(),
                bytes: text.into_bytes(),
            }])),
            Err(arboard::Error::ContentNotAvailable) => Ok(HostReply::Mime(vec![])),
            Err(error) => Err(clipboard_error(error)),
        })
        .await
    }

    pub(super) async fn clipboard_write(
        platform: &NativePlatform,
        data: Vec<MimeData>,
    ) -> Result<HostReply, PlatformError> {
        let mut plain = None;
        let mut html = None;
        for item in data {
            let text = String::from_utf8(item.bytes)
                .map_err(|_| PlatformError::InvalidData("clipboard text".into()));
            match mime_essence(&item.mime).as_str() {
                TEXT_PLAIN => plain = Some(text?),
                TEXT_HTML => html = Some(text?),
                other => {
                    return Err(PlatformError::Unsupported(format!(
                        "clipboard type {other} on this host"
                    )));
                }
            }
        }
        with_clipboard(platform, move |clipboard| {
            match (html, plain) {
                (Some(html), plain) => clipboard.set_html(html, plain),
                (None, Some(plain)) => clipboard.set_text(plain),
                (None, None) => clipboard.clear(),
            }
            .map_err(clipboard_error)?;
            Ok(HostReply::Unit)
        })
        .await
    }

    pub(super) async fn open_url(
        platform: &NativePlatform,
        url: String,
    ) -> Result<HostReply, PlatformError> {
        let url = validate_open_url(&url)?;
        platform
            .inner
            .runtime
            .spawn_blocking(move || open::that_detached(url))
            .await
            .map_err(|_| PlatformError::Cancelled)?
            .map_err(|_| PlatformError::Unsupported("no application opens this URL".into()))?;
        Ok(HostReply::Unit)
    }
}
