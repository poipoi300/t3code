//! Picking files in the browser as the TypeScript web composer does, with a hidden
//! `<input type="file" multiple>`; the picked `File`s stay here under handles for `ReadResource`.
use super::{WebPlatform, js};
use std::{cell::RefCell, collections::BTreeMap};
use t3_client::platform::{HostReply, HostResource, Platform, PlatformError};
use wasm_bindgen::JsCast;
use web_sys::{File, HtmlInputElement};

/// What this page picked, by the handle the composer holds.
#[derive(Default)]
pub(crate) struct PickedFiles(RefCell<BTreeMap<String, File>>);

impl PickedFiles {
    pub(crate) fn contains(&self, handle: &str) -> bool {
        self.0.borrow().contains_key(handle)
    }
}

fn unavailable(_: wasm_bindgen::JsValue) -> PlatformError {
    PlatformError::Unsupported("file input".into())
}

/// Opens the browser's file dialog; a dismissed one is an empty selection. Browsers without the
/// input's `cancel` event (before Chrome 113, Firefox 91, Safari 16.4) leave a dismissal pending.
pub(crate) async fn pick(
    platform: &WebPlatform,
    multiple: bool,
) -> Result<HostReply, PlatformError> {
    let document = platform
        .inner
        .window
        .document()
        .ok_or_else(|| PlatformError::Unsupported("document".into()))?;
    let input: HtmlInputElement = document
        .create_element("input")
        .map_err(unavailable)?
        .dyn_into()
        .map_err(|_| PlatformError::Unsupported("file input".into()))?;
    input.set_type("file");
    input.set_multiple(multiple);
    let chosen = js::FirstEvent::new(
        &input,
        vec![
            ("change", Box::new(|_: &web_sys::Event| true)),
            ("cancel", Box::new(|_: &web_sys::Event| false)),
        ],
    )?;
    input.click();
    if !chosen.await? {
        return Ok(HostReply::Resources(Vec::new()));
    }
    let mut resources = Vec::new();
    if let Some(files) = input.files() {
        for index in 0..files.length() {
            let Some(file) = files.get(index) else {
                continue;
            };
            let handle: String = platform
                .random_bytes(16)?
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let handle = format!("picked-{handle}");
            let mime = file.type_();
            resources.push(HostResource {
                handle: handle.clone(),
                name: file.name(),
                mime: if mime.is_empty() {
                    "application/octet-stream".into()
                } else {
                    mime
                },
            });
            platform.inner.picked.0.borrow_mut().insert(handle, file);
        }
    }
    Ok(HostReply::Resources(resources))
}

/// The picked file's bytes, refused over `max_bytes` before reading them.
pub(crate) async fn read(
    platform: &WebPlatform,
    handle: &str,
    max_bytes: usize,
) -> Result<HostReply, PlatformError> {
    let file = platform
        .inner
        .picked
        .0
        .borrow()
        .get(handle)
        .cloned()
        .ok_or_else(|| PlatformError::InvalidData("unknown file handle".into()))?;
    if file.size() > max_bytes as f64 {
        return Err(PlatformError::TooLarge);
    }
    let buffer = wasm_bindgen_futures::JsFuture::from(file.array_buffer())
        .await
        .map_err(|_| PlatformError::Storage("the file cannot be read".into()))?;
    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
    if bytes.len() > max_bytes {
        return Err(PlatformError::TooLarge);
    }
    Ok(HostReply::Bytes(bytes))
}
