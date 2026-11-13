//! The inline-reference operations used when snapshotting a composer send.
use crate::view_logic::JsTrim;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextReference<'a> {
    pub kind: &'a str,
    pub context_id: &'a str,
    pub start: usize,
    pub end: usize,
}

pub fn collect_references(text: &str) -> Vec<ContextReference<'_>> {
    let mut references = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = text[cursor..].find("](t3-context://v1/") {
        let close = cursor + relative;
        let label_start = text[..close]
            .rfind(['\n', ']'])
            .map_or(0, |i| i + 1)
            .max(cursor);
        let mut label_units = 0;
        let mut open = None;
        for (offset, ch) in text[label_start..close].char_indices().rev() {
            if ch == '[' {
                open = Some(label_start + offset);
            }
            label_units += ch.len_utf16();
            if label_units > 512 {
                break;
            }
        }
        let Some(open) = open else {
            cursor = close + 2;
            continue;
        };
        let href_start = close + 2;
        let Some(end) = text[href_start..].find(')').map(|i| i + href_start) else {
            break;
        };
        let href = &text[href_start..end];
        let rest = &href["t3-context://v1/".len()..];
        if let Some((kind, id)) = rest.split_once('/')
            && valid_kind(kind)
            && valid_id(id)
            && text[open + 1..close].encode_utf16().count() <= 512
            && href["t3-context://v1/".len()..].encode_utf16().count() <= 200
        {
            references.push(ContextReference {
                kind,
                context_id: id,
                start: if open > 0 && text.as_bytes()[open - 1] == b'!' {
                    open - 1
                } else {
                    open
                },
                end: end + 1,
            });
        }
        cursor = end + 1;
    }
    references
}

fn valid_kind(kind: &str) -> bool {
    !kind.is_empty()
        && kind.len() <= 40
        && kind.as_bytes()[0].is_ascii_lowercase()
        && kind
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn strip_references(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut cursor = 0;
    for reference in collect_references(text) {
        result.push_str(&text[cursor..reference.start]);
        cursor = reference.end;
    }
    result.push_str(&text[cursor..]);
    result
}

pub fn provider_text_for_display(text: &str) -> String {
    let mut result = String::new();
    let mut cursor = 0;
    let mut scan = 0;
    while let Some(relative) = text[scan..].find("; ref=") {
        let marker = scan + relative;
        scan = marker + 6;
        let Some(start) = text[..marker].rfind('[').filter(|start| *start >= cursor) else {
            continue;
        };
        let Some(end) = text[marker..].find(']').map(|n| marker + n + 1) else {
            break;
        };
        let label = &text[start + 1..marker];
        let prefix = "\n\n<t3_context version=\"1\">\n<context kind=\"";
        let Some(tail) = text[end..].strip_prefix(prefix) else {
            result.push_str(&text[cursor..end]);
            cursor = end;
            continue;
        };
        let id = &text[marker + 6..end - 1];
        let valid_label = label.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            && label.contains(": ")
            && !label.contains(['[', ']', ';', '\n']);
        let Some(kind_end) = tail
            .find('"')
            .filter(|n| tail[n + 1..].starts_with(&format!(" id=\"{id}\"")))
        else {
            result.push_str(&text[cursor..end]);
            cursor = end;
            continue;
        };
        let close = "</t3_context>";
        let Some(close_index) = tail[kind_end..].find(close).filter(|_| valid_label) else {
            result.push_str(&text[cursor..end]);
            cursor = end;
            continue;
        };
        result.push_str(&text[cursor..start]);
        result.push('[');
        result.push_str(label);
        result.push(']');
        cursor = end + prefix.len() + kind_end + close_index + close.len();
        scan = cursor;
    }
    result.push_str(&text[cursor..]);
    result
}

pub(crate) fn escape_provider_context_payload(text: &str) -> String {
    let mut result = String::new();
    let mut cursor = 0;
    for (index, _) in text.match_indices('<') {
        let tail = text[index + 1..]
            .strip_prefix('/')
            .unwrap_or(&text[index + 1..]);
        let matched = ["t3_context", "context"].iter().any(|tag| {
            tail.get(..tag.len())
                .is_some_and(|s| s.eq_ignore_ascii_case(tag))
                && tail[tag.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
        });
        if matched {
            result.push_str(&text[cursor..index]);
            result.push_str("&lt;");
            cursor = index + 1;
        }
    }
    result.push_str(&text[cursor..]);
    result
}

pub fn remove_reference(text: &str, context_id: &str) -> String {
    let mut result = text.to_owned();
    let mut cursor = text.len();
    for reference in collect_references(text)
        .into_iter()
        .rev()
        .filter(|r| r.context_id == context_id)
    {
        let mut start = reference.start;
        let mut end = reference.end;
        if result.as_bytes().get(end) == Some(&b' ') {
            end += 1;
        } else if start > 0 && result.as_bytes()[start - 1] == b' ' {
            start -= 1;
        }
        result.replace_range(start..end, "");
        cursor = start;
    }
    if cursor >= result.len() {
        result.js_trim_end().to_owned()
    } else {
        result
    }
}

pub fn kind_scoped_id(kind: &str, producer_id: &str) -> String {
    let id = format!("{kind}_{producer_id}");
    if valid_id(&id) {
        return id;
    }
    let units: Vec<_> = id.encode_utf16().collect();
    let mut forward = 0x811c9dc5_u32;
    let mut reverse = 0x9dc5811c_u32;
    for (a, b) in units.iter().zip(units.iter().rev()) {
        forward = (forward ^ u32::from(*a)).wrapping_mul(0x01000193);
        reverse = (reverse ^ u32::from(*b)).wrapping_mul(0x01000193);
    }
    let mut slug = String::new();
    let mut separator = false;
    for ch in id.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            separator = false;
            slug.push(ch);
        } else {
            separator = true;
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(48).collect();
    format!(
        "{}-{forward:08x}{reverse:08x}",
        if slug.is_empty() { "ctx" } else { &slug }
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn references_keep_surrounding_whitespace_and_ignore_invalid_links() {
        assert_eq!(
            strip_references("yoo [Terminal 1](t3-context://v1/terminal/ctx-expired) waddup"),
            "yoo  waddup"
        );
        assert_eq!(strip_references("![x](t3-context://v1/image/id)"), "");
        for text in [
            "[x](t3-context://v2/terminal/id)",
            "[x](t3-context://v1/Terminal/id)",
            "[x](t3-context://v1/terminal/id/path)",
        ] {
            assert_eq!(strip_references(text), text);
        }
    }

    #[test]
    fn nested_open_brackets_follow_the_source_regex() {
        assert_eq!(
            strip_references("prefix [outer[inner](t3-context://v1/terminal/a) suffix"),
            "prefix  suffix"
        );
    }

    #[test]
    fn removal_handles_repeated_references_and_boundary_spaces() {
        assert_eq!(
            remove_reference(
                "[x](t3-context://v1/terminal/a) hello [x](t3-context://v1/terminal/a) ",
                "a"
            ),
            "hello "
        );
        assert_eq!(
            remove_reference("hello [x](t3-context://v1/terminal/a) world", "a"),
            "hello world"
        );
        assert_eq!(remove_reference("hello", "a"), "hello");
        assert_eq!(kind_scoped_id("terminal", "ctx-1"), "terminal_ctx-1");
    }
}
