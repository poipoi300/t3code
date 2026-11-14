use crate::{FailureReason, ForgeError, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;
pub(crate) fn decode<T: DeserializeOwned>(
    provider: &str,
    operation: &str,
    value: Value,
) -> Result<T> {
    serde_json::from_value(value).map_err(|_| {
        ForgeError::provider(
            provider,
            operation,
            FailureReason::Failed,
            "The source control host returned an invalid response.",
        )
    })
}
pub(crate) fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}
pub(crate) fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value[key].as_str().map(str::trim).filter(|s| !s.is_empty())
}
pub(crate) fn rows(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}
pub(crate) fn nodes(value: &Value) -> impl Iterator<Item = &Value> {
    rows(&value["nodes"]).filter(|n| !n.is_null())
}
pub(crate) fn sha(value: &str) -> bool {
    (7..=64).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_hexdigit())
}
pub(crate) fn resolve_author<'a>(author: &'a str, viewer: &'a str) -> &'a str {
    let author = author.trim();
    if (author.eq_ignore_ascii_case("me") || author.eq_ignore_ascii_case("@me"))
        && !viewer.trim().is_empty()
    {
        viewer
    } else {
        author
    }
}
pub(crate) fn quote_patch_path(value: &str) -> String {
    if value
        .bytes()
        .any(|b| b < 32 || b == 127 || matches!(b, b'"' | b'\\'))
    {
        let mut result = String::from("\"");
        for byte in value.as_bytes() {
            match byte {
                b'"' => result.push_str("\\\""),
                b'\\' => result.push_str("\\\\"),
                32..=126 => result.push(*byte as char),
                _ => result.push_str(&format!("\\{byte:03o}")),
            }
        }
        result.push('"');
        result
    } else {
        value.into()
    }
}
