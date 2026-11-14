//! Pure helpers shared by servers and clients. Must compile for wasm32-unknown-unknown.
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
pub mod background_activity;
pub mod find_text;
pub mod palette_search;
pub mod project_settings;
pub mod search_policy;
pub mod terminal_history;
pub mod terminal_output;
pub mod text_generators;
pub mod thread_search;
pub mod tool_output;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid {entity}: {detail}")]
    Decode {
        entity: &'static str,
        detail: String,
    },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn regex(pattern: &'static str) -> regex::Regex {
    static PATTERNS: std::sync::LazyLock<
        std::sync::Mutex<std::collections::BTreeMap<&'static str, regex::Regex>>,
    > = std::sync::LazyLock::new(Default::default);
    PATTERNS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .entry(pattern)
        .or_insert_with(|| regex::Regex::new(pattern).expect("constant transport regex"))
        .clone()
}
