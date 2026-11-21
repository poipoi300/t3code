//! The guest page's network edge. The guest's per-share cookie is browser-managed: nothing here
//! sees, stores or sends it, and the invite credential is passed once to `redeem`.

use futures::future::LocalBoxFuture;
use serde_json::Value;

/// How the server (or the network) refused a guest request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuestError {
    /// A terminal HTTP answer with the TS `{reason, message}` body. Retrying cannot help.
    Rejected {
        status: u16,
        reason: GuestFailure,
        message: String,
    },
    /// No answer, a dropped stream or a 5xx: the page reconnects.
    Network(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestFailure {
    Invalid,
    Inactive,
    Used,
    Busy,
    StalePage,
    Unavailable,
    Rejected,
    Empty,
    TooManyFiles,
    TooLarge,
    UnsupportedFile,
    FilesBusy,
    Unknown,
}

impl GuestFailure {
    pub fn from_reason(reason: &str) -> Self {
        match reason {
            "invalid" => Self::Invalid,
            "inactive" => Self::Inactive,
            "used" => Self::Used,
            "busy" => Self::Busy,
            "stale_page" => Self::StalePage,
            "unavailable" => Self::Unavailable,
            "rejected" => Self::Rejected,
            "empty" => Self::Empty,
            "too_many_files" => Self::TooManyFiles,
            "too_large" => Self::TooLarge,
            "unsupported_file" => Self::UnsupportedFile,
            "files_busy" => Self::FilesBusy,
            _ => Self::Unknown,
        }
    }
}

pub type GuestFuture<'a, T> = LocalBoxFuture<'a, Result<T, GuestError>>;

/// One open `/api/guest/stream`. Dropping it releases the connection.
pub trait GuestStreamHandle {
    /// The next decoded `data:` JSON frame; `None` when the server closed the stream cleanly.
    fn next(&mut self) -> GuestFuture<'_, Option<Value>>;
}

pub trait GuestTransport {
    /// `POST /api/guest/redeem`. The response cookie stays with the browser.
    fn redeem(&self, credential: &str, name: &str) -> GuestFuture<'_, ()>;
    /// `GET /api/guest/session?share=` with the browser's cookie for that share.
    fn session(&self, share: &str) -> GuestFuture<'_, ()>;
    /// `GET /api/guest/stream?share=`.
    fn open_stream(&self, share: &str) -> GuestFuture<'_, Box<dyn GuestStreamHandle>>;
    /// `POST /api/guest/messages?share=` with `{shareId, text}`.
    fn submit(&self, share: &str, text: &str) -> GuestFuture<'_, ()>;
    /// Reconnect backoff, supplied by the host so tests control time.
    fn sleep(&self, millis: u64) -> LocalBoxFuture<'_, ()>;
}
