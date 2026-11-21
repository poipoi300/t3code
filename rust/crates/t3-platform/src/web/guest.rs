use super::{js, timer};
use futures::future::LocalBoxFuture;
use js_sys::Uint8Array;
use std::rc::Rc;
use t3_client::guest::{
    http::{
        GuestChunks, GuestHttp, GuestMethod, GuestRequest, GuestResponse, GuestStreamResponse,
        HttpGuestTransport,
    },
    transport::{GuestError, GuestFuture, GuestTransport},
};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AbortController, Headers, ReadableStreamDefaultReader, RequestCredentials, RequestInit,
    RequestRedirect, Response, Window,
};

/// Aborts the request when dropped, which releases a stream the page no longer reads.
struct AbortOnDrop(AbortController);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct BodyReader {
    reader: ReadableStreamDefaultReader,
    _abort: AbortOnDrop,
}

impl GuestChunks for BodyReader {
    fn next(&mut self) -> GuestFuture<'_, Option<Vec<u8>>> {
        Box::pin(async move {
            let chunk = JsFuture::from(self.reader.read())
                .await
                .map_err(network("read"))?;
            if js::get(&chunk, "done").as_bool() == Some(true) {
                return Ok(None);
            }
            let bytes: Uint8Array = js::get(&chunk, "value")
                .dyn_into()
                .map_err(|_| GuestError::Network("body chunk".into()))?;
            Ok(Some(bytes.to_vec()))
        })
    }
}

fn network(context: &'static str) -> impl Fn(JsValue) -> GuestError {
    // Error names only: browsers can echo URLs in messages.
    move |error| GuestError::Network(format!("{context}: {}", js::error_name(&error)))
}

/// Same-origin guest requests. The page's own cookie jar carries `t3_guest_<share>`; this type
/// never reads or sets a cookie and passes no credential header.
pub struct BrowserGuestHttp {
    /// `None` outside a window context; every request then fails as a network error.
    window: Option<Window>,
}

impl Default for BrowserGuestHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserGuestHttp {
    pub fn new() -> Self {
        Self {
            window: web_sys::window(),
        }
    }

    async fn fetch(&self, request: GuestRequest) -> Result<(Response, AbortOnDrop), GuestError> {
        let window = self
            .window
            .as_ref()
            .ok_or_else(|| GuestError::Network("no window".into()))?;
        let controller = AbortController::new().map_err(network("abort controller"))?;
        let init = RequestInit::new();
        init.set_method(match request.method {
            GuestMethod::Get => "GET",
            GuestMethod::Post => "POST",
        });
        if let Some(json) = &request.json {
            let headers = Headers::new().map_err(network("headers"))?;
            headers
                .append("content-type", "application/json")
                .map_err(network("headers"))?;
            init.set_headers(&headers);
            init.set_body(&JsValue::from_str(json));
        }
        init.set_credentials(RequestCredentials::SameOrigin);
        init.set_redirect(RequestRedirect::Error);
        init.set_signal(Some(&controller.signal()));
        let abort = AbortOnDrop(controller);
        let response: Response =
            JsFuture::from(window.fetch_with_str_and_init(&request.path, &init))
                .await
                .map_err(network("fetch"))?
                .unchecked_into();
        Ok((response, abort))
    }
}

fn reader(response: &Response) -> Result<ReadableStreamDefaultReader, GuestError> {
    response
        .body()
        .map(|stream| stream.get_reader().unchecked_into())
        .ok_or_else(|| GuestError::Network("no response body".into()))
}

impl GuestHttp for BrowserGuestHttp {
    fn send(&self, request: GuestRequest) -> GuestFuture<'_, GuestResponse> {
        Box::pin(async move {
            let (response, abort) = self.fetch(request).await?;
            let status = response.status();
            let mut body = Vec::new();
            if let Ok(reader) = reader(&response) {
                let mut chunks = BodyReader {
                    reader,
                    _abort: abort,
                };
                // Replies are small JSON bodies; cap what a misbehaving origin can make us hold.
                while body.len() < 1024 * 1024 {
                    match chunks.next().await? {
                        Some(chunk) => body.extend(chunk),
                        None => break,
                    }
                }
            }
            Ok(GuestResponse { status, body })
        })
    }

    fn open(&self, request: GuestRequest) -> GuestFuture<'_, GuestStreamResponse> {
        Box::pin(async move {
            let (response, abort) = self.fetch(request).await?;
            Ok(GuestStreamResponse {
                status: response.status(),
                chunks: Box::new(BodyReader {
                    reader: reader(&response)?,
                    _abort: abort,
                }),
            })
        })
    }

    fn sleep(&self, millis: u64) -> LocalBoxFuture<'_, ()> {
        let window = self.window.clone();
        Box::pin(async move {
            match window {
                Some(window) => timer::sleep(window, millis).await,
                // No timer to wait on: never retry in a hot loop.
                None => std::future::pending().await,
            }
        })
    }
}

/// The transport `t3_ui::screens::guest::install_transport` installs on the web target.
pub fn browser_transport() -> Rc<dyn GuestTransport> {
    Rc::new(HttpGuestTransport::new(BrowserGuestHttp::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use t3_client::guest::transport::GuestFailure;
    use wasm_bindgen::prelude::wasm_bindgen;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen(inline_js = "
        const original = globalThis.fetch;
        let calls = [];
        let aborted = 0;
        const bytes = (text) => new TextEncoder().encode(text);
        export function installFetch() {
            calls = [];
            aborted = 0;
            globalThis.fetch = async (url, init) => {
                calls.push({
                    url, method: init.method, credentials: init.credentials,
                    redirect: init.redirect, body: init.body ?? null,
                    contentType: init.headers ? init.headers.get('content-type') : null,
                });
                init.signal.addEventListener('abort', () => { aborted += 1; });
                const path = url.split('?')[0];
                if (path === '/api/guest/stream') {
                    // Stays open until the page lets go of it.
                    return new Response(new ReadableStream({
                        start(controller) {
                            controller.enqueue(bytes(': ping\\n\\ndata: {\"type\":\"view\"}\\n'));
                            controller.enqueue(bytes('\\ndata: {\"type\":\"ended\"}\\n\\n'));
                        },
                    }), { status: 200 });
                }
                if (url.includes('share=gone')) {
                    return new Response(
                        JSON.stringify({ reason: 'inactive', message: 'This guest link is no longer active.' }),
                        { status: 403 },
                    );
                }
                if (url.includes('share=down')) {
                    return new Response('bad gateway', { status: 502 });
                }
                if (url.includes('share=offline')) {
                    throw new TypeError('Failed to fetch');
                }
                return new Response(null, { status: 204 });
            };
        }
        export function restoreFetch() { globalThis.fetch = original; }
        export function callLog() { return JSON.stringify(calls); }
        export function abortCount() { return aborted; }
    ")]
    extern "C" {
        #[wasm_bindgen(js_name = installFetch)]
        fn install_fetch();
        #[wasm_bindgen(js_name = restoreFetch)]
        fn restore_fetch();
        #[wasm_bindgen(js_name = callLog)]
        fn call_log() -> String;
        #[wasm_bindgen(js_name = abortCount)]
        fn abort_count() -> u32;
    }

    struct RestoreFetch;
    impl Drop for RestoreFetch {
        fn drop(&mut self) {
            restore_fetch();
        }
    }

    // Node supplies fetch, Response, ReadableStream and AbortController; the browser supplies the
    // same names, so the adapter runs here against a scripted `fetch`.
    fn transport() -> HttpGuestTransport<BrowserGuestHttp> {
        let window: Window = js_sys::global().unchecked_into();
        HttpGuestTransport::new(BrowserGuestHttp {
            window: Some(window),
        })
    }

    #[wasm_bindgen_test]
    async fn browser_guest_requests_are_same_origin_and_map_statuses() {
        install_fetch();
        let _restore = RestoreFetch;
        let guest = transport();
        assert_eq!(guest.redeem("share_1.secret", "Alex").await, Ok(()));
        assert_eq!(guest.submit("s1", "hello").await, Ok(()));
        assert!(matches!(
            guest.session("gone").await,
            Err(GuestError::Rejected {
                status: 403,
                reason: GuestFailure::Inactive,
                ..
            })
        ));
        assert!(matches!(
            guest.session("down").await,
            Err(GuestError::Network(_))
        ));
        assert!(matches!(
            guest.session("offline").await,
            Err(GuestError::Network(_))
        ));
        let calls: Vec<Value> = serde_json::from_str(&call_log()).unwrap();
        assert_eq!(calls[0]["url"], "/api/guest/redeem");
        assert_eq!(calls[0]["method"], "POST");
        assert_eq!(calls[0]["contentType"], "application/json");
        assert_eq!(
            serde_json::from_str::<Value>(calls[0]["body"].as_str().unwrap()).unwrap(),
            json!({"credential": "share_1.secret", "name": "Alex"})
        );
        assert_eq!(calls[1]["url"], "/api/guest/messages?share=s1");
        assert_eq!(calls[2]["method"], "GET");
        for call in &calls {
            assert_eq!(call["credentials"], "same-origin");
            assert_eq!(call["redirect"], "error");
            // The invite is only ever in the redeem body.
            assert!(!call["url"].as_str().unwrap().contains("secret"));
        }
    }

    #[wasm_bindgen_test]
    async fn browser_guest_stream_is_incremental_and_dropping_it_aborts_the_request() {
        install_fetch();
        let _restore = RestoreFetch;
        let guest = transport();
        let mut stream = guest.open_stream("s1").await.unwrap();
        assert_eq!(stream.next().await, Ok(Some(json!({"type": "view"}))));
        assert_eq!(stream.next().await, Ok(Some(json!({"type": "ended"}))));
        assert_eq!(abort_count(), 0);
        drop(stream);
        assert_eq!(abort_count(), 1);
    }
}
