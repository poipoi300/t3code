//! Native Claude Code CLI stdio protocol client; this crate does not target wasm32.
//! Process ownership and T3 event projection belong to the caller.

pub mod control;
pub mod launch;
pub mod message;
pub mod session;

pub use session::{
    Client, ControlReply, Event, McpReply, McpServer, PendingRequest, Responder, Session,
    SessionOptions,
};

/// The single compatibility baseline for wire shapes and SDK launch behavior.
pub const PROTOCOL_BASELINE: ProtocolBaseline = ProtocolBaseline {
    sdk: "0.3.321",
    companion_cli: "2.1.321",
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolBaseline {
    pub sdk: &'static str,
    pub companion_cli: &'static str,
}

#[derive(Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Claude transport is closed")]
    Closed,
    #[error("Claude JSON decode {kind:?} at line {line}, column {column}")]
    Decode {
        kind: DecodeKind,
        line: usize,
        column: usize,
    },
    #[error("Claude transport I/O failure")]
    Io(String),
    #[error("Claude protocol violation")]
    Protocol(String),
    #[error("Claude control request failed")]
    Cli { request_id: String, message: String },
    #[error("unsupported Claude control request")]
    Unsupported(String),
    #[error("Claude control request was cancelled")]
    Cancelled,
    #[error("invalid Claude launch option")]
    Options(String),
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::Decode {
            kind: match value.classify() {
                serde_json::error::Category::Io => DecodeKind::Io,
                serde_json::error::Category::Syntax => DecodeKind::Syntax,
                serde_json::error::Category::Data => DecodeKind::Data,
                serde_json::error::Category::Eof => DecodeKind::Eof,
            },
            line: value.line(),
            column: value.column(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeKind {
    Io,
    Syntax,
    Data,
    Eof,
}
impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

macro_rules! redacted_debug {
    ($($ty:ty),* $(,)?) => { $(
        impl std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($ty), " { <redacted> }"))
            }
        }
    )* };
}
pub(crate) use redacted_debug;
