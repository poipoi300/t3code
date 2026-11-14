use super::{WebPlatform, idb, js};
use crate::common::{TEXT_HTML, TEXT_PLAIN, mime_essence, unsupported, uuid_v4, validate_open_url};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use js_sys::{Array, Function, Reflect, Uint8Array};
use t3_client::platform::{HostAction, HostReply, MimeData, Platform, PlatformError};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::CryptoKey;

/// The TypeScript web client's keys, used in place so a browser profile keeps its identity.
const DPOP_DATABASE: &str = "t3code:cloud-auth";
const DPOP_STORE: &str = "keys";
const DPOP_KEY_ID: &str = "relay-dpop-proof-key";
const ANALYTICS_DEVICE_ID_KEY: &str = "t3code:analytics-device-id";

#[derive(Clone)]
pub(super) struct BrowserDpopKey {
    private_key: CryptoKey,
    x: Vec<u8>,
    y: Vec<u8>,
}

fn crypto_error(context: &str) -> impl Fn(JsValue) -> PlatformError + '_ {
    move |error| PlatformError::Unsupported(format!("{context}: {}", js::error_name(&error)))
}

pub(super) async fn dispatch(
    platform: &WebPlatform,
    action: HostAction,
) -> Result<HostReply, PlatformError> {
    match action {
        HostAction::ClipboardRead => clipboard_read(platform).await,
        HostAction::PickResources { multiple, .. } => super::files::pick(platform, multiple).await,
        HostAction::ReadResource { handle, max_bytes }
            if platform.inner.picked.contains(&handle) =>
        {
            super::files::read(platform, &handle, max_bytes).await
        }
        HostAction::ClipboardWrite(data) => clipboard_write(platform, data).await,
        HostAction::OpenUrl(url) => {
            let url = validate_open_url(
                &url,
                t3_client::persistence::external_links::ExternalUrlSurface::Web,
            )?;
            // With noopener the browser returns null even on success, so blocking is invisible.
            platform
                .inner
                .window
                .open_with_url_and_target_and_features(&url, "_blank", "noopener,noreferrer")
                .map_err(|_| PlatformError::Denied("the browser refused to open the URL".into()))?;
            Ok(HostReply::Unit)
        }
        // TypeScript's `openRemoteEditorUrl`: the page navigates to the editor's scheme, which
        // the browser hands to the installed editor.
        HostAction::OpenEditorLink(url) => {
            let url = t3_client::persistence::external_links::safe_editor_link(
                &url,
                t3_client::persistence::external_links::ExternalUrlSurface::Web,
            )
            .ok_or_else(|| PlatformError::Denied("opening this link is not allowed".into()))?;
            platform
                .inner
                .window
                .location()
                .assign(&url)
                .map_err(|_| PlatformError::Denied("the browser refused the editor link".into()))?;
            Ok(HostReply::Unit)
        }
        HostAction::DpopThumbprint => {
            let key = dpop_key(platform).await?;
            Ok(HostReply::Text(Some(crate::common::dpop_thumbprint(
                &key.x, &key.y,
            ))))
        }
        HostAction::DpopPublicJwk => {
            let key = dpop_key(platform).await?;
            Ok(HostReply::Document(crate::common::dpop_public_jwk(
                &key.x, &key.y,
            )))
        }
        HostAction::DpopSign { signing_input } => {
            let key = dpop_key(platform).await?;
            let algorithm = js::object(&[("name", "ECDSA".into()), ("hash", "SHA-256".into())])?;
            let subtle = platform.subtle()?;
            let signature = JsFuture::from(
                subtle
                    .sign_with_object_and_u8_array(&algorithm, &key.private_key, &signing_input)
                    .map_err(crypto_error("DPoP sign"))?,
            )
            .await
            .map_err(crypto_error("DPoP sign"))?;
            Ok(HostReply::Bytes(Uint8Array::new(&signature).to_vec()))
        }
        HostAction::AnalyticsDeviceId => Ok(HostReply::Text(analytics_device_id(platform))),
        HostAction::RelayDeviceId => Ok(HostReply::Text(None)),
        HostAction::ClientPresentation if platform.inner.presentation.is_some() => {
            serde_json::to_value(&platform.inner.presentation)
                .map(HostReply::Document)
                .map_err(|_| PlatformError::InvalidData("client presentation".into()))
        }
        action => match &platform.inner.extension {
            Some(extension) => extension.host(action).await,
            None => Err(unsupported(&action)),
        },
    }
}

/// Blocked site storage only drops the id, as in the TypeScript client.
fn analytics_device_id(platform: &WebPlatform) -> Option<String> {
    let storage = platform.inner.window.local_storage().ok().flatten()?;
    if let Some(existing) = storage
        .get_item(ANALYTICS_DEVICE_ID_KEY)
        .ok()
        .flatten()
        .filter(|id| !id.is_empty())
    {
        return Some(existing);
    }
    let bytes = platform.random_bytes(16).ok()?;
    let mut seed = [0_u8; 16];
    seed.copy_from_slice(&bytes);
    let created = uuid_v4(seed);
    storage.set_item(ANALYTICS_DEVICE_ID_KEY, &created).ok()?;
    Some(created)
}

fn clipboard(platform: &WebPlatform) -> Result<web_sys::Clipboard, PlatformError> {
    let clipboard = platform.inner.window.navigator().clipboard();
    if JsValue::from(clipboard.clone()).is_undefined() {
        return Err(PlatformError::Unsupported(
            "the clipboard needs a secure context".into(),
        ));
    }
    Ok(clipboard)
}

async fn clipboard_read(platform: &WebPlatform) -> Result<HostReply, PlatformError> {
    let text = JsFuture::from(clipboard(platform)?.read_text())
        .await
        .map_err(|_| PlatformError::Denied("clipboard read was not permitted".into()))?
        .as_string()
        .unwrap_or_default();
    Ok(HostReply::Mime(if text.is_empty() {
        vec![]
    } else {
        vec![MimeData {
            mime: TEXT_PLAIN.into(),
            bytes: text.into_bytes(),
        }]
    }))
}

async fn clipboard_write(
    platform: &WebPlatform,
    data: Vec<MimeData>,
) -> Result<HostReply, PlatformError> {
    let mut items = Vec::new();
    for item in data {
        let essence = mime_essence(&item.mime);
        if essence != TEXT_PLAIN && essence != TEXT_HTML {
            return Err(PlatformError::Unsupported(format!(
                "clipboard type {essence} on this host"
            )));
        }
        let text = String::from_utf8(item.bytes)
            .map_err(|_| PlatformError::InvalidData("clipboard text".into()))?;
        items.push((essence, text));
    }
    let clipboard = clipboard(platform)?;
    let denied = |_| PlatformError::Denied("clipboard write was not permitted".into());
    if items.iter().all(|(mime, _)| mime == TEXT_PLAIN) {
        let text = items.pop().map(|(_, text)| text).unwrap_or_default();
        JsFuture::from(clipboard.write_text(&text))
            .await
            .map_err(denied)?;
        return Ok(HostReply::Unit);
    }
    // ClipboardItem carries the HTML and plain-text alternatives together.
    let global = js_sys::global();
    let constructor = |name: &str| -> Result<Function, PlatformError> {
        js::get(&global, name)
            .dyn_into::<Function>()
            .map_err(|_| PlatformError::Unsupported(format!("{name} in this browser")))
    };
    let record = js_sys::Object::new();
    for (mime, text) in items {
        let options = js::object(&[("type", JsValue::from_str(&mime))])?;
        let blob = Reflect::construct(
            &constructor("Blob")?,
            &Array::of2(&Array::of1(&JsValue::from_str(&text)), &options),
        )
        .map_err(|_| PlatformError::InvalidData("clipboard blob".into()))?;
        Reflect::set(&record, &JsValue::from_str(&mime), &blob)
            .map_err(|_| PlatformError::InvalidData("clipboard item".into()))?;
    }
    let item = Reflect::construct(&constructor("ClipboardItem")?, &Array::of1(&record))
        .map_err(|_| PlatformError::InvalidData("clipboard item".into()))?;
    let write = js::get(&clipboard, "write")
        .dyn_into::<Function>()
        .map_err(|_| PlatformError::Unsupported("rich clipboard writes".into()))?;
    let promise: js_sys::Promise = write
        .call1(&clipboard, &Array::of1(&item))
        .map_err(denied)?
        .unchecked_into();
    JsFuture::from(promise).await.map_err(denied)?;
    Ok(HostReply::Unit)
}

fn decode_coordinate(jwk: &JsValue, name: &str) -> Result<Vec<u8>, PlatformError> {
    js::get(jwk, name)
        .as_string()
        .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
        .filter(|value| value.len() == 32)
        .ok_or_else(|| PlatformError::InvalidData("DPoP public key".into()))
}

fn stored_key(value: &JsValue) -> Result<BrowserDpopKey, PlatformError> {
    let private_key = js::get(value, "privateKey")
        .dyn_into::<CryptoKey>()
        .map_err(|_| PlatformError::InvalidData("stored DPoP key".into()))?;
    let jwk = js::get(value, "publicJwk");
    Ok(BrowserDpopKey {
        private_key,
        x: decode_coordinate(&jwk, "x")?,
        y: decode_coordinate(&jwk, "y")?,
    })
}

/// Same database, store, key and record shape as `apps/web/src/cloud/dpop.ts`.
async fn dpop_key(platform: &WebPlatform) -> Result<BrowserDpopKey, PlatformError> {
    let mut cached = platform.inner.dpop.lock().await;
    if let Some(key) = cached.as_ref() {
        return Ok(key.clone());
    }
    let database = idb::open_dpop(&platform.inner.factory, DPOP_DATABASE, DPOP_STORE).await?;
    let existing = idb::legacy_value(&database, DPOP_STORE, DPOP_KEY_ID).await;
    let key = match existing? {
        Some(value) => stored_key(&value)?,
        None => {
            let (record, key) = generate_dpop_key(platform).await?;
            // Two tabs can both find no key; the first add wins and the other uses its key,
            // so neither caches a key that is no longer stored.
            if idb::add_value(&database, DPOP_STORE, DPOP_KEY_ID, &record).await? {
                key
            } else {
                match idb::legacy_value(&database, DPOP_STORE, DPOP_KEY_ID).await? {
                    Some(value) => stored_key(&value)?,
                    None => {
                        return Err(PlatformError::Storage("could not save the DPoP key".into()));
                    }
                }
            }
        }
    };
    database.close();
    *cached = Some(key.clone());
    Ok(key)
}

async fn generate_dpop_key(
    platform: &WebPlatform,
) -> Result<(JsValue, BrowserDpopKey), PlatformError> {
    let subtle = platform.subtle()?;
    let curve = js::object(&[("name", "ECDSA".into()), ("namedCurve", "P-256".into())])?;
    let usages = Array::of2(&"sign".into(), &"verify".into());
    let pair = JsFuture::from(
        subtle
            .generate_key_with_object(&curve, true, &usages)
            .map_err(crypto_error("DPoP key generation"))?,
    )
    .await
    .map_err(crypto_error("DPoP key generation"))?;
    let exported = |name: &str| -> Result<CryptoKey, PlatformError> {
        js::get(&pair, name)
            .dyn_into::<CryptoKey>()
            .map_err(|_| PlatformError::InvalidData("generated DPoP key".into()))
    };
    let private_jwk = JsFuture::from(
        subtle
            .export_key("jwk", &exported("privateKey")?)
            .map_err(crypto_error("DPoP key export"))?,
    )
    .await
    .map_err(crypto_error("DPoP key export"))?;
    let public = JsFuture::from(
        subtle
            .export_key("jwk", &exported("publicKey")?)
            .map_err(crypto_error("DPoP key export"))?,
    )
    .await
    .map_err(crypto_error("DPoP key export"))?;
    let x = decode_coordinate(&public, "x")?;
    let y = decode_coordinate(&public, "y")?;
    // Re-imported without extractability, so the stored private key can never be read back.
    let private_key: CryptoKey = JsFuture::from(
        subtle
            .import_key_with_object(
                "jwk",
                private_jwk.unchecked_ref(),
                &curve,
                false,
                &Array::of1(&"sign".into()),
            )
            .map_err(crypto_error("DPoP key import"))?,
    )
    .await
    .map_err(crypto_error("DPoP key import"))?
    .unchecked_into();
    let public_jwk = js::object(&[
        ("kty", "EC".into()),
        ("crv", "P-256".into()),
        ("x", URL_SAFE_NO_PAD.encode(&x).into()),
        ("y", URL_SAFE_NO_PAD.encode(&y).into()),
    ])?;
    let record = js::object(&[
        ("privateKey", private_key.clone().into()),
        ("publicJwk", public_jwk.into()),
        ("thumbprint", crate::common::dpop_thumbprint(&x, &y).into()),
    ])?;
    Ok((record.into(), BrowserDpopKey { private_key, x, y }))
}
