//! The guest page's network host: [`t3_client::guest::http::GuestHttp`] over the browser's
//! same-origin `fetch` (the browser owns the per-share cookie) and over a native `reqwest` client
//! with its own cookie jar. The request shapes, SSE decoding and rejection mapping are shared in
//! `t3_client::guest::http`; this file only moves bytes.

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(target_arch = "wasm32")]
pub use crate::web::{BrowserGuestHttp, browser_transport};
#[cfg(not(target_arch = "wasm32"))]
pub use native::NativeGuestHttp;
