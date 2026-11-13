//! General Translation's catalogue keys.
//!
//! GT keys a message by the first 16 hex digits of the SHA-256 of
//! `stableStringify({ source, context?, dataFormat })` (`hashSource` in `generaltranslation`).
//! Reproducing it lets code ask for a message by its English text, as the TypeScript does.

use sha2::{Digest, Sha256};

/// How GT interprets the source text. `gt("…")` and `msg("…")` use ICU; `<T>text</T>` children
/// that are plain text use JSX.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataFormat {
    Icu,
    Jsx,
}

impl DataFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::Icu => "ICU",
            Self::Jsx => "JSX",
        }
    }
}

/// The catalogue key of a plain-text message, with an optional GT `$context`.
pub fn message_key(source: &str, format: DataFormat, context: Option<&str>) -> String {
    // Keys sorted, as `stableStringify` does: context, dataFormat, source. serde_json escapes
    // strings exactly as `JSON.stringify` does (only `"`, `\` and control characters).
    let mut canonical = String::from("{");
    if let Some(context) = context.filter(|context| !context.is_empty()) {
        canonical.push_str("\"context\":");
        canonical.push_str(&json_string(context));
        canonical.push(',');
    }
    canonical.push_str("\"dataFormat\":\"");
    canonical.push_str(format.as_str());
    canonical.push_str("\",\"source\":");
    canonical.push_str(&json_string(source));
    canonical.push('}');
    let digest = Sha256::digest(canonical.as_bytes());
    digest[..8]
        .iter()
        .fold(String::with_capacity(16), |mut key, byte| {
            key.push_str(&format!("{byte:02x}"));
            key
        })
}

fn json_string(value: &str) -> String {
    serde_json::Value::String(value.to_owned()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_keys_in_the_shipped_catalogues() {
        assert_eq!(
            message_key("Settings", DataFormat::Icu, None),
            "0a75e464d833bca8"
        );
        assert_eq!(
            message_key("12-hour", DataFormat::Icu, None),
            "7fa9414ef5e9dd6c"
        );
    }
}
