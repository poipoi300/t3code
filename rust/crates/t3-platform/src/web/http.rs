use super::js;
use js_sys::Uint8Array;
use std::collections::BTreeMap;
use t3_client::platform::{Credentials, HttpRequest, HttpResponse, PlatformError};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AbortController, Headers, ReadableStreamDefaultReader, RequestCredentials, RequestInit,
    RequestRedirect, Response,
};

struct AbortOnDrop(Option<AbortController>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(controller) = self.0.take() {
            controller.abort();
        }
    }
}

fn network(context: &str) -> impl Fn(JsValue) -> PlatformError + '_ {
    move |error| PlatformError::Network(format!("{context}: {}", js::error_name(&error)))
}

/// Redirects fail the request (the native host returns the 3xx unfollowed; a browser cannot).
pub(crate) async fn fetch(
    window: &web_sys::Window,
    request: HttpRequest,
) -> Result<HttpResponse, PlatformError> {
    let url =
        url::Url::parse(&request.url).map_err(|_| PlatformError::InvalidData("HTTP URL".into()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(PlatformError::InvalidData("HTTP URL scheme".into()));
    }
    let headers = Headers::new().map_err(network("headers"))?;
    for (name, value) in &request.headers {
        headers
            .append(name, value)
            .map_err(|_| PlatformError::InvalidData("HTTP header".into()))?;
    }
    let controller = AbortController::new().map_err(network("abort controller"))?;
    let init = RequestInit::new();
    init.set_method(&request.method);
    init.set_headers(&headers);
    if !request.body.is_empty() {
        init.set_body(&Uint8Array::from(request.body.as_slice()));
    }
    init.set_credentials(match request.credentials {
        Credentials::Omit => RequestCredentials::Omit,
        Credentials::SameOrigin => RequestCredentials::SameOrigin,
        Credentials::Include => RequestCredentials::Include,
    });
    init.set_redirect(RequestRedirect::Error);
    init.set_signal(Some(&controller.signal()));
    let mut guard = AbortOnDrop(Some(controller));
    let response: Response = JsFuture::from(window.fetch_with_str_and_init(url.as_str(), &init))
        .await
        .map_err(network("fetch failed"))?
        .unchecked_into();
    let cap = request.max_response_bytes;
    let mut response_headers = BTreeMap::new();
    if let Ok(Some(entries)) = js_sys::try_iter(&response.headers()) {
        for entry in entries.flatten() {
            let pair: js_sys::Array = entry.unchecked_into();
            if let (Some(name), Some(value)) = (pair.get(0).as_string(), pair.get(1).as_string()) {
                response_headers.insert(name.to_ascii_lowercase(), value);
            }
        }
    }
    if response_headers
        .get("content-length")
        .and_then(|length| length.parse::<u64>().ok())
        .is_some_and(|length| length > cap as u64)
    {
        return Err(PlatformError::TooLarge);
    }
    let mut body = Vec::new();
    if let Some(stream) = response.body() {
        let reader: ReadableStreamDefaultReader = stream.get_reader().unchecked_into();
        loop {
            let chunk = JsFuture::from(reader.read())
                .await
                .map_err(network("read body"))?;
            if js::get(&chunk, "done").as_bool() == Some(true) {
                break;
            }
            let bytes: Uint8Array = js::get(&chunk, "value")
                .dyn_into()
                .map_err(|_| PlatformError::InvalidData("HTTP body chunk".into()))?;
            if body.len() + bytes.length() as usize > cap {
                let _ = reader.cancel();
                return Err(PlatformError::TooLarge);
            }
            body.extend(bytes.to_vec());
        }
    }
    guard.0 = None;
    Ok(HttpResponse {
        status: response.status(),
        headers: response_headers,
        body,
    })
}
