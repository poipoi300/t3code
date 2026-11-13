use super::links::*;
use crate::view_logic::{JsTrim, is_js_whitespace};
use serde::Serialize;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalLinkKind {
    Url,
    Path,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TerminalLinkMatch {
    pub kind: TerminalLinkKind,
    pub text: String,
    pub start: usize,
    pub end: usize,
}
fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}
pub fn is_terminal_url(value: &str) -> bool {
    value
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
        || value
            .get(..8)
            .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
}
fn trim_closing_delimiters(value: &str, kind: TerminalLinkKind) -> &str {
    let mut s = value
        .trim_end_matches(|c| ".,;!?".contains(c) || (kind == TerminalLinkKind::Path && c == ':'));
    for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
        let opens = s.chars().filter(|c| *c == open).count();
        let mut closes = s.chars().filter(|c| *c == close).count();
        while s.ends_with(close) && closes > opens {
            s = &s[..s.len() - 1];
            closes -= 1;
        }
    }
    s
}
fn name_char(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(c, '_' | '.' | '-')
        || matches!(c,'\u{0300}'..='\u{036f}'|'\u{1ab0}'..='\u{1aff}')
}
fn absolute_prefix(s: &str) -> bool {
    s.starts_with('/')
        || s.starts_with("\\\\")
        || s.starts_with("~/")
        || s.starts_with("~\\")
        || s.starts_with("./")
        || s.starts_with(".\\")
        || s.starts_with("../")
        || s.starts_with("..\\")
        || is_windows_absolute(s)
}
fn relative_end(s: &str) -> Option<usize> {
    let mut index = 0;
    for c in s.chars().take_while(|c| name_char(*c)) {
        index += c.len_utf8();
    }
    if index == 0 || !s[index..].starts_with(['/', '\\']) {
        return None;
    }
    let mut slash = false;
    while index < s.len() {
        let c = s[index..].chars().next()?;
        if matches!(c, '/' | '\\') {
            slash = true;
            index += 1;
            continue;
        }
        if name_char(c) || matches!(c, '$' | '+' | '@') {
            index += c.len_utf8();
            continue;
        }
        if matches!(c, '[' | '(') {
            let close = if c == '[' { ']' } else { ')' };
            let Some(end) = s[index..].find(close) else {
                break;
            };
            let run = &s[index..index + end + 1];
            if run
                .chars()
                .all(|c| name_char(c) || matches!(c, '[' | ']' | '(' | ')'))
                && (close == ']' || s[index + end + 1..].starts_with(['/', '\\']))
            {
                index += end + 1;
                continue;
            }
        }
        break;
    }
    if !slash {
        return None;
    }
    for _ in 0..2 {
        if let Some(tail) = s[index..].strip_prefix(':') {
            let count = tail.bytes().take_while(|b| b.is_ascii_digit()).count();
            if count > 0 {
                index += count + 1;
            } else {
                break;
            }
        }
    }
    Some(index)
}
pub fn extract_terminal_links(line: &str) -> Vec<TerminalLinkMatch> {
    let mut ranges: Vec<(usize, usize, TerminalLinkKind, String)> = vec![];
    let mut excluded = vec![];
    let mut remote_tails = vec![];
    if let Some(rest) = line.strip_prefix("PS ")
        && absolute_prefix(rest)
        && let Some(end) = rest.find('>')
    {
        ranges.push((3, 3 + end, TerminalLinkKind::Path, rest[..end].into()));
    }
    let overlaps = |a: usize, b: usize, ranges: &[(usize, usize, TerminalLinkKind, String)]| {
        ranges.iter().any(|(c, d, _, _)| a < *d && *c < b)
    };
    let tokens: Vec<_> = line.split_whitespace().collect();
    let mut search = 0;
    let mut prior = "";
    for token in tokens {
        let start = line[search..].find(token).map_or(search, |i| search + i);
        let end = start + token.len();
        search = end;
        let token_clean = token.trim_matches(['"', '\'', '`']);
        if [
            "GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "CONNECT", "TRACE",
            "proxying",
        ]
        .contains(&prior)
            && token_clean.contains('?')
            && (token_clean.starts_with('/')
                || token_clean.split('/').next().is_some_and(|s| {
                    s.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
                }))
        {
            excluded.push((start, end));
        }
        if let Some(colon) = token.find("://")
            && colon >= 2
            && token[..colon]
                .trim_start_matches(['+', '-', '('])
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            excluded.push((start, end));
        }
        if let Some(at) = token.find('@')
            && token[..at]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
            && let Some(colon) = token[at..].find(':')
        {
            let tail = start + at + colon + 1;
            remote_tails.push((tail, end));
        }
        prior = token;
    }
    for (start, _) in line.char_indices() {
        let s = &line[start..];
        if !is_terminal_url(s) {
            continue;
        }
        let raw = s
            .split(|c| is_js_whitespace(c) || matches!(c, '"' | '\'' | '`' | '<' | '>'))
            .next()
            .unwrap_or_default();
        let text = trim_closing_delimiters(raw, TerminalLinkKind::Url);
        let end = start + text.len();
        if !text.is_empty() && !overlaps(start, end, &ranges) {
            ranges.push((start, end, TerminalLinkKind::Url, text.into()));
        }
    }
    let mut cursor = 0;
    while cursor < line.len() {
        let Some(c) = line[cursor..].chars().next() else {
            break;
        };
        if matches!(c, '"' | '\'' | '`')
            && let Some(close) = line[cursor + 1..].find(c)
        {
            let end = cursor + 1 + close;
            let raw = &line[cursor + 1..end];
            let shape = absolute_prefix(raw)
                || raw.contains(['/', '\\'])
                || split_file_path_position(raw, "")
                    .path
                    .rsplit_once('.')
                    .is_some_and(|(_, ext)| !ext.is_empty() && ext.chars().all(name_char));
            if shape && !raw.contains(['\r', '\n']) && !is_terminal_url(raw) {
                let mut position_end = end + 1;
                for _ in 0..2 {
                    if let Some(tail) = line[position_end..].strip_prefix(':') {
                        let count = tail.bytes().take_while(|c| c.is_ascii_digit()).count();
                        if count > 0 {
                            position_end += count + 1;
                        }
                    }
                }
                let suffix = &line[end + 1..position_end];
                let target = format!("{raw}{suffix}");
                let span_end = if suffix.is_empty() { end } else { position_end };
                if !overlaps(cursor + 1, span_end, &ranges) {
                    ranges.push((cursor + 1, span_end, TerminalLinkKind::Path, target));
                }
            }
            cursor = end + 1;
            continue;
        }
        cursor += c.len_utf8();
    }
    let mut cursor = 0;
    while cursor < line.len() {
        let s = &line[cursor..];
        let Some(c) = s.chars().next() else {
            break;
        };
        let previous = line[..cursor].chars().next_back();
        let length = if absolute_prefix(s) {
            Some(
                s.find(|c| is_js_whitespace(c) || matches!(c, '"' | '\'' | '`' | '<' | '>'))
                    .unwrap_or(s.len()),
            )
        } else if previous.is_none_or(|c| !name_char(c)) {
            relative_end(s)
        } else {
            None
        };
        if let Some(length) = length.filter(|n| *n > 0) {
            let text = trim_closing_delimiters(&s[..length], TerminalLinkKind::Path);
            let end = cursor + text.len();
            if !text.is_empty() && !is_terminal_url(text) && !overlaps(cursor, end, &ranges) {
                ranges.push((cursor, end, TerminalLinkKind::Path, text.into()));
            }
            cursor += length;
        } else {
            cursor += c.len_utf8();
        }
    }
    ranges.sort_by_key(|r| r.0);
    ranges
        .into_iter()
        .filter(|(start, end, kind, _)| {
            *kind == TerminalLinkKind::Url
                || (!excluded.iter().any(|(a, b)| start < b && a < end)
                    && !remote_tails.iter().any(|(a, b)| start >= a && end <= b))
        })
        .map(|(start, end, kind, text)| TerminalLinkMatch {
            kind,
            text,
            start: utf16_len(&line[..start]),
            end: utf16_len(&line[..end]),
        })
        .collect()
}
pub fn resolve_terminal_link_target(text: &str, cwd: &str) -> String {
    if text
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("file://"))
        && let Some((path, hash)) = parse_file_url_href(text)
    {
        return format_file_path_position(&split_file_path_position(
            &safe_decode_uri_component(&path),
            &hash,
        ));
    }
    resolve_path_link_target(text, cwd)
}
pub fn is_terminal_link_activation(meta: bool, ctrl: bool, platform: &str) -> bool {
    if platform.is_empty() {
        return false;
    }
    if platform.to_ascii_lowercase().contains("mac") {
        meta && !ctrl
    } else {
        ctrl && !meta
    }
}
#[derive(Debug, Clone)]
pub struct TerminalBufferLine {
    pub text: String,
    pub is_wrapped: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WrappedTerminalLinkLineSegment {
    pub buffer_line_number: usize,
    pub text: String,
    pub start_index: usize,
    pub end_index: usize,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WrappedTerminalLinkLine {
    pub text: String,
    pub segments: Vec<WrappedTerminalLinkLineSegment>,
}
pub fn collect_wrapped_terminal_link_line(
    buffer_line_number: usize,
    get_line: impl Fn(usize) -> Option<TerminalBufferLine>,
) -> Option<WrappedTerminalLinkLine> {
    let mut start = buffer_line_number;
    let mut line = get_line(start.checked_sub(1)?)?;
    while start > 1 && line.is_wrapped {
        start -= 1;
        line = get_line(start - 1)?;
    }
    let mut result = WrappedTerminalLinkLine {
        text: String::new(),
        segments: vec![],
    };
    let mut index = 0;
    while let Some(line) = get_line(start - 1) {
        let wrapped = get_line(start).is_some_and(|l| l.is_wrapped);
        let text = if wrapped {
            line.text
        } else {
            line.text.js_trim_end().into()
        };
        let end = index + utf16_len(&text);
        result.text.push_str(&text);
        result.segments.push(WrappedTerminalLinkLineSegment {
            buffer_line_number: start,
            text,
            start_index: index,
            end_index: end,
        });
        index = end;
        if !wrapped {
            break;
        }
        start += 1;
    }
    Some(result)
}
