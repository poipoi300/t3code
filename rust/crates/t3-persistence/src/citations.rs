use std::{collections::HashMap, sync::OnceLock};

use percent_encoding::percent_decode_str;
use regex::Regex;
use url::Url;

const MAX_HREF: usize = 9 * (8_000 + 8_000 + 3 * 4_096 + 2 * 32) + 1_000;

fn js_whitespace(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000d}' | ' ' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

fn length(value: &str) -> usize {
    value.encode_utf16().count()
}

fn citation_text(href: &str) -> Option<String> {
    if length(href) > MAX_HREF {
        return None;
    }
    let url = Url::parse(href).ok()?;
    if url.scheme() != "t3-citation"
        || url.host_str() != Some("v1")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some_and(|fragment| !fragment.is_empty())
    {
        return None;
    }
    let parts: Vec<_> = url.path().strip_prefix('/')?.split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    for part in parts {
        // decodeURIComponent rejects malformed escapes and invalid UTF-8 in path ids.
        let bytes = part.as_bytes();
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'%'
                && (bytes.get(index + 1).is_none_or(|b| !b.is_ascii_hexdigit())
                    || bytes.get(index + 2).is_none_or(|b| !b.is_ascii_hexdigit()))
            {
                return None;
            }
        }
        let decoded = percent_decode_str(part).decode_utf8().ok()?;
        let trimmed = decoded.trim_matches(js_whitespace);
        if trimmed.is_empty() || length(trimmed) > 4_096 {
            return None;
        }
    }
    let pairs: Vec<_> = url.query_pairs().collect();
    let query: HashMap<_, _> = pairs
        .iter()
        .map(|(k, v)| (k.as_ref(), v.as_ref()))
        .collect();
    let required = ["text", "start", "end", "prefix", "suffix"];
    if pairs.len() != 5 + usize::from(query.contains_key("comment"))
        || query.len() != pairs.len()
        || required.iter().any(|key| !query.contains_key(key))
    {
        return None;
    }
    let offset = |key| -> Option<u64> {
        let value = query.get(key)?;
        if value.is_empty() || value.len() > 16 || !value.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        value
            .parse()
            .ok()
            .filter(|&value| value <= 9_007_199_254_740_991)
    };
    if offset("end")? <= offset("start")? {
        return None;
    }
    let text = *query.get("text")?;
    if text.trim_matches(js_whitespace).is_empty()
        || length(text) > 8_000
        || length(query.get("prefix")?) > 32
        || length(query.get("suffix")?) > 32
    {
        return None;
    }
    match query.get("comment") {
        Some(comment) if length(comment) > 8_000 => None,
        Some(comment) => Some(format!("{text}\nComment: {comment}")),
        None => Some(text.to_owned()),
    }
}

pub(crate) fn readable_search_text(text: &str) -> Option<String> {
    static LINKS: OnceLock<Regex> = OnceLock::new();
    let links = LINKS.get_or_init(|| {
        // JavaScript's \s excludes U+0085 and includes U+FEFF, unlike Rust regex's Unicode \s.
        Regex::new(r"\[Assistant quote\]\((t3-citation://v1/[^\u{9}-\u{d}\u{20}\u{a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff})]+)\)")
            .expect("constant citation regex")
    });
    let mut readable = String::new();
    let mut after = 0;
    for captures in links.captures_iter(text) {
        if let Some(replacement) = citation_text(&captures[1]) {
            let Some(full) = captures.get(0) else {
                continue;
            };
            readable.push_str(&text[after..full.start()]);
            readable.push_str(&replacement);
            after = full.end();
        }
    }
    if after == 0 {
        return None;
    }
    readable.push_str(&text[after..]);
    (readable != text).then_some(readable)
}
