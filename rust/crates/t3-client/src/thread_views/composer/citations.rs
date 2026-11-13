use super::super::document::{AtomRange, utf16_len};
use t3_contracts::assistant_citations::{
    ASSISTANT_CITATION_CONTEXT_LENGTH, ASSISTANT_CITATION_MAX_COMMENT_LENGTH,
    ASSISTANT_CITATION_MAX_ID_LENGTH, ASSISTANT_CITATION_MAX_TEXT_LENGTH, AssistantCitation,
};
use url::Url;

const PREFIX: &str = "t3-citation://v1/";
const MAX_HREF: usize = 9
    * (ASSISTANT_CITATION_MAX_TEXT_LENGTH
        + ASSISTANT_CITATION_MAX_COMMENT_LENGTH
        + 3 * ASSISTANT_CITATION_MAX_ID_LENGTH
        + 2 * ASSISTANT_CITATION_CONTEXT_LENGTH)
    + 1000;
fn decode(value: &str) -> Option<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < value.len() {
        if value.as_bytes()[i] == b'%' {
            out.push(u8::from_str_radix(value.get(i + 1..i + 3)?, 16).ok()?);
            i += 3;
        } else {
            out.push(value.as_bytes()[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}
pub fn parse_href(href: &str) -> Option<AssistantCitation> {
    if !href.starts_with(PREFIX) || href.len() > MAX_HREF {
        return None;
    }
    let url = Url::parse(href).ok()?;
    if url.scheme() != "t3-citation"
        || url.host_str() != Some("v1")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url
        .path()
        .strip_prefix('/')?
        .split('/')
        .map(decode)
        .collect::<Option<_>>()?;
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.trim().is_empty() || utf16_len(p) > ASSISTANT_CITATION_MAX_ID_LENGTH)
    {
        return None;
    }
    let pairs: Vec<_> = url.query_pairs().collect();
    let get = |name: &str| {
        let mut found = pairs.iter().filter(|(key, _)| key == name);
        let value = found.next()?.1.as_ref();
        found.next().is_none().then_some(value)
    };
    let comment = get("comment");
    if pairs.len() != 5 + usize::from(comment.is_some()) {
        return None;
    }
    let start = get("start")?;
    let end = get("end")?;
    let position = |v: &str| {
        (!v.is_empty() && v.len() <= 16 && v.bytes().all(|b| b.is_ascii_digit()))
            .then(|| v.parse::<u64>().ok())
            .flatten()
            .filter(|n| *n <= 9_007_199_254_740_991)
    };
    let start = position(start)?;
    let end = position(end)?;
    let text = get("text")?;
    let prefix = get("prefix")?;
    let suffix = get("suffix")?;
    if text.is_empty()
        || utf16_len(text) > ASSISTANT_CITATION_MAX_TEXT_LENGTH
        || end <= start
        || utf16_len(prefix) > ASSISTANT_CITATION_CONTEXT_LENGTH
        || utf16_len(suffix) > ASSISTANT_CITATION_CONTEXT_LENGTH
        || comment
            .is_some_and(|s| s.is_empty() || utf16_len(s) > ASSISTANT_CITATION_MAX_COMMENT_LENGTH)
    {
        return None;
    }
    serde_json::from_value(serde_json::json!({"version":1,"environmentId":parts[0],"threadId":parts[1],"messageId":parts[2],"text":text,"start":start,"end":end,"prefix":prefix,"suffix":suffix,
        "comment":comment})).ok().map(|mut citation: AssistantCitation| { if comment.is_none() {citation.comment=None;} citation })
}
fn path_part(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
pub fn format_href(citation: &AssistantCitation) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("text", &citation.text)
        .append_pair("start", &citation.start.to_string())
        .append_pair("end", &citation.end.to_string())
        .append_pair("prefix", &citation.prefix)
        .append_pair("suffix", &citation.suffix);
    if let Some(Some(comment)) = &citation.comment {
        query.append_pair("comment", comment);
    }
    format!(
        "{PREFIX}{}/{}/{}?{}",
        path_part(&citation.environment_id.0),
        path_part(&citation.thread_id.0),
        path_part(&citation.message_id.0),
        query.finish()
    )
}
pub fn serialize(citation: &AssistantCitation) -> String {
    format!("[Assistant quote]({})", format_href(citation))
}
pub fn for_composer(citation: &AssistantCitation, comment: &str) -> String {
    let mut citation = citation.clone();
    let comment = comment.trim();
    citation.comment = (!comment.is_empty()).then(|| Some(comment.into()));
    format!("{} ", serialize(&citation))
}
#[derive(Debug, Clone, PartialEq)]
pub struct CitationMatch {
    pub range: AtomRange,
    pub citation: AssistantCitation,
    pub source: String,
}
pub fn collect(text: &str) -> Vec<CitationMatch> {
    let mut result = vec![];
    let mut offset = 0;
    while let Some(at) = text[offset..].find("[Assistant quote](") {
        let start = offset + at;
        let href_start = start + 18;
        let tail = &text[href_start..];
        let Some(end) = tail.find(|c: char| c == ')' || c.is_whitespace()) else {
            break;
        };
        offset = href_start + end + 1;
        if tail.as_bytes()[end] != b')' {
            continue;
        }
        if let Some(citation) = parse_href(&tail[..end]) {
            result.push(CitationMatch {
                range: AtomRange {
                    start: utf16_len(&text[..start]),
                    end: utf16_len(&text[..offset]),
                },
                citation,
                source: text[start..offset].into(),
            });
        }
    }
    result
}
pub fn label(citation: &AssistantCitation) -> String {
    let preview = citation
        .comment
        .as_ref()
        .and_then(Option::as_deref)
        .filter(|c| !c.trim().is_empty())
        .unwrap_or(&citation.text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if utf16_len(&preview) <= 64 {
        return preview;
    }
    let mut used = 0;
    let mut out: String = preview
        .chars()
        .take_while(|c| {
            used += c.len_utf16();
            used <= 64
        })
        .collect();
    out.push('…');
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn citation_roundtrip_and_bad_queries() {
        let citation:AssistantCitation=serde_json::from_value(json!({"version":1,"environmentId":"e/a","threadId":"t","messageId":"m","text":"Unicode 😀 (x)","start":3,"end":40,"prefix":"前: ","suffix":"後"})).expect("fixture");
        let source = for_composer(&citation, "  ask\n this  ");
        let matches = collect(&source);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].citation.comment, Some(Some("ask\n this".into())));
        assert_eq!(matches[0].range.end + 1, utf16_len(&source));
        let href = format_href(&citation);
        assert!(parse_href(&format!("{href}&start=3")).is_none());
        assert!(parse_href(&format!("{href}&extra=x")).is_none());
        assert!(parse_href(&format!("{href}#fragment")).is_none());
        assert_eq!(parse_href(&href), Some(citation));
    }
}
