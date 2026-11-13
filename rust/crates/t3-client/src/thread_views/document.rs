//! Lossless composer source and pure edits. Selection offsets are UTF-16 source offsets.
use super::rich_text::{Mark, Span, parse_inline_markdown};

pub fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}
pub fn utf16_byte_offset(value: &str, offset: usize) -> Option<usize> {
    let mut units = 0;
    for (byte, ch) in value.char_indices() {
        if units == offset {
            return Some(byte);
        }
        units += ch.len_utf16();
        if units > offset {
            return None;
        }
    }
    (units == offset).then_some(value.len())
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}
impl Selection {
    pub fn caret(at: usize) -> Self {
        Self {
            anchor: at,
            head: at,
        }
    }
    pub fn from(self) -> usize {
        self.anchor.min(self.head)
    }
    pub fn to(self) -> usize {
        self.anchor.max(self.head)
    }
    pub fn collapsed(self) -> bool {
        self.anchor == self.head
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
    pub cursor_after: usize,
}
impl Edit {
    pub fn apply(&self, value: &str) -> Option<String> {
        if self.start > self.end {
            return None;
        }
        let start = utf16_byte_offset(value, self.start)?;
        let end = utf16_byte_offset(value, self.end)?;
        let next = format!("{}{}{}", &value[..start], self.replacement, &value[end..]);
        utf16_byte_offset(&next, self.cursor_after)?;
        Some(next)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtomRange {
    pub start: usize,
    pub end: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomKind {
    Mention,
    Skill,
    Model,
    Citation,
    ContextReference,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Atom {
    pub range: AtomRange,
    pub kind: AtomKind,
    pub value: String,
    pub source: String,
}
pub(crate) fn currency(ch: char) -> bool {
    matches!(
        ch,
        '$' | '¢'
            | '£'
            | '¤'
            | '¥'
            | '֏'
            | '؋'
            | '߾'
            | '߿'
            | '৲'
            | '৳'
            | '৻'
            | '૱'
            | '௹'
            | '฿'
            | '៛'
            | '꠸'
            | '﷼'
            | '﹩'
            | '＄'
            | '￠'
            | '￡'
            | '￥'
            | '￦'
            | '𑿝'
            | '𑿞'
            | '𑿟'
            | '𑿠'
            | '𞋿'
    ) || ('\u{20a0}'..='\u{20c1}').contains(&ch)
}
fn whitespace(ch: char) -> bool {
    ch.is_whitespace() || ch == '\u{feff}'
}
fn unescape(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            } else {
                out.push(ch);
            }
        } else {
            out.push(ch);
        }
    }
    out
}
fn decode_percent(value: &str) -> Option<String> {
    let mut bytes = vec![];
    let mut i = 0;
    while i < value.len() {
        if value.as_bytes()[i] == b'%' {
            let hex = value.get(i + 1..i + 3)?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            bytes.push(value.as_bytes()[i]);
            i += 1;
        }
    }
    String::from_utf8(bytes).ok()
}
pub fn parse_context_href(href: &str) -> Option<(&str, &str)> {
    let (kind, id) = href.strip_prefix("t3-context://v1/")?.split_once('/')?;
    (kind.len() <= 40
        && kind.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && kind
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')))
    .then_some((kind, id))
}
fn link_at(text: &str, at: usize) -> Option<(usize, String, AtomKind)> {
    let rest = &text[at..];
    if !rest.starts_with('[') {
        return None;
    }
    let mut escape = false;
    let mut label_end = None;
    for (i, ch) in rest[1..].char_indices() {
        if i > 2048 || ch == '\n' {
            return None;
        }
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' {
            escape = true;
            continue;
        }
        if ch == ']' {
            label_end = Some(i + 1);
            break;
        }
    }
    let end_label = label_end?;
    if utf16_len(&rest[1..end_label]) > 512 {
        return None;
    }
    let href_start = end_label + 2;
    if rest.as_bytes().get(end_label + 1) != Some(&b'(') {
        return None;
    }
    let end = href_start + rest[href_start..].find(')')?;
    let href = &rest[href_start..end];
    let label = unescape(&rest[1..end_label]);
    if let Some((_, id)) = parse_context_href(href) {
        return Some((at + end + 1, id.into(), AtomKind::ContextReference));
    }
    if href.chars().any(whitespace) {
        return None;
    }
    let path = decode_percent(href).unwrap_or_else(|| href.into());
    let basename = path.rsplit(['/', '\\']).next()?;
    let scheme = path.find(':').is_some_and(|i| {
        i > 0
            && path[..i]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
    });
    let drive = path.as_bytes().get(1) == Some(&b':')
        && matches!(path.as_bytes().get(2), Some(b'/' | b'\\'));
    if path.is_empty() || label != basename || (scheme && !drive) {
        return None;
    }
    Some((at + end + 1, path, AtomKind::Mention))
}
pub(crate) fn math_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = vec![];
    let mut opened = false;
    let mut dollars = false;
    for (index, byte) in text.bytes().enumerate() {
        if matches!(byte, b'\n' | b'\r') {
            opened = false;
        } else if byte == b'$' {
            if opened && text.as_bytes().get(index.wrapping_sub(1)) != Some(&b' ') {
                dollars = true;
                break;
            }
            opened = true;
        }
    }
    let has_table = text
        .lines()
        .any(|line| line.contains('|') && line.contains("---"));
    let mut at = 0;
    let mut checked = 0;
    let mut line_start = 0;
    let mut indented_code = false;
    while at < text.len() {
        if let Some(relative) = text[checked..at].rfind('\n') {
            line_start = checked + relative + 1;
        }
        checked = at;
        if let Some((end, _, AtomKind::Mention)) = link_at(text, at) {
            at = end;
            continue;
        }
        let tail = &text[at..];
        if at == line_start {
            let line = tail.split('\n').next().unwrap_or_default();
            let previous = text[..line_start]
                .strip_suffix('\n')
                .unwrap_or_default()
                .rsplit('\n')
                .next()
                .unwrap_or_default();
            let indented = tail.starts_with("    ") || tail.starts_with('\t');
            if (indented && (indented_code || line_start == 0 || previous.trim().is_empty()))
                || (indented_code && line.trim().is_empty())
            {
                indented_code = true;
                at += line.len() + usize::from(tail.as_bytes().get(line.len()) == Some(&b'\n'));
                continue;
            }
            indented_code = false;
        }
        let prefix = &text[line_start..at];
        if (tail.starts_with("```") || tail.starts_with("~~~"))
            && prefix.chars().all(|ch| matches!(ch, ' ' | '\t' | '>'))
            && let Some(line_end) = tail.find('\n')
            && let Some(fence) = parse_opening_fence(tail[..line_end].trim_end_matches('\r'))
            && fence.language.split_whitespace().next() == Some("math")
        {
            let mut end = at + line_end + 1;
            for line in text[end..].split_inclusive('\n') {
                let close = line.trim_end_matches(['\r', '\n']);
                if is_closing_fence(close.trim_start_matches([' ', '\t', '>']), &fence.fence) {
                    end += close.len();
                    ranges.push((at, end));
                    at = end;
                    break;
                }
                end += line.len();
            }
            if ranges.last().is_some_and(|(_, end)| *end == at) {
                continue;
            }
        }
        if (tail.starts_with("http://") || tail.starts_with("https://") || tail.starts_with("www."))
            && text[..at]
                .chars()
                .next_back()
                .is_none_or(|ch| !ch.is_alphanumeric())
        {
            at += tail
                .find(|ch: char| ch.is_whitespace() || matches!(ch, '<' | '>'))
                .unwrap_or(tail.len());
            continue;
        }
        if tail.starts_with("](") {
            let mut cursor = 2;
            let mut depth = 1;
            while cursor < tail.len() && depth > 0 {
                let ch = tail[cursor..].chars().next().unwrap_or_default();
                if ch == '\\' {
                    cursor += 1;
                    if cursor < tail.len() {
                        cursor += tail[cursor..].chars().next().map_or(0, char::len_utf8);
                    }
                    continue;
                }
                if ch == '(' {
                    depth += 1
                } else if ch == ')' {
                    depth -= 1
                }
                cursor += ch.len_utf8();
            }
            if depth == 0 {
                at += cursor;
                continue;
            }
        }
        if tail.starts_with('`') {
            let length = tail.bytes().take_while(|byte| *byte == b'`').count();
            if let Some(end) = tail[length..].find(&tail[..length]) {
                at += length + end + length;
                continue;
            }
        }
        let table_line = has_table && text[line_start..].starts_with('|');
        if tail.starts_with("\\(") || tail.starts_with("\\[") {
            let square = tail.starts_with("\\[");
            let close = if square { "\\]" } else { "\\)" };
            let mut cursor = 2;
            let mut substantive = false;
            let mut found = None;
            while cursor < tail.len() {
                let rest = &tail[cursor..];
                let ch = rest.chars().next().unwrap_or_default();
                if ch == '`' || (table_line && matches!(ch, '\r' | '\n')) {
                    break;
                }
                if rest.starts_with(close) {
                    if !square || substantive {
                        found = Some(cursor + 2);
                    }
                    break;
                }
                if rest.starts_with("\\\\") {
                    cursor += 2;
                    continue;
                }
                if ch != '\\'
                    && ch != ' '
                    && ch != ','
                    && !matches!(ch, '\r' | '\n')
                    && !ch.is_ascii_digit()
                {
                    substantive = true;
                }
                cursor += ch.len_utf8();
            }
            if let Some(end) = found {
                ranges.push((at, at + end));
                at += end;
                continue;
            }
        } else if tail.starts_with('\\') {
            at += 1;
            if let Some(ch) = text[at..].chars().next()
                && ch.is_ascii_punctuation()
            {
                at += ch.len_utf8();
            }
            continue;
        } else if dollars && tail.starts_with('$') {
            let count = tail.bytes().take_while(|byte| *byte == b'$').count();
            if let Some(end) = dollar_math_end(tail, count, table_line) {
                ranges.push((at, at + end));
                at += end;
                continue;
            }
            at += count;
            continue;
        }
        at += tail.chars().next().map_or(1, char::len_utf8);
    }
    ranges
}
fn dollar_math_end(text: &str, open: usize, single_line: bool) -> Option<usize> {
    let first = *text.as_bytes().get(open)?;
    let whitespace = matches!(first, b' ' | b'\t' | b',');
    let flow = open >= 2 && matches!(first, b'\n' | b'\r');
    if open == 1 && (whitespace || matches!(first, b'\n' | b'\r')) {
        return None;
    }
    let mut cursor = open;
    let mut slashes = 0;
    while cursor < text.len() {
        let ch = text[cursor..].chars().next()?;
        if ch == '`' || ((!flow && (open == 1 || single_line)) && matches!(ch, '\n' | '\r')) {
            return None;
        }
        if text[cursor..].starts_with("://") {
            return None;
        }
        if ch == '$' && slashes % 2 == 0 {
            let count = text[cursor..]
                .bytes()
                .take_while(|byte| *byte == b'$')
                .count();
            if count == open || (flow && count >= open) {
                let before = text[..cursor].chars().next_back()?;
                let after = text[cursor + count..].chars().next();
                if flow {
                    if text[cursor + count..]
                        .split(['\r', '\n'])
                        .next()?
                        .trim()
                        .is_empty()
                    {
                        return Some(cursor + count);
                    }
                    return None;
                }
                if matches!(before, '\r' | '\n') || after.is_some_and(|ch| ch.is_ascii_digit()) {
                    return None;
                }
                if before == ','
                    && !whitespace
                    && after.is_some_and(|ch| {
                        !ch.is_whitespace()
                            && !matches!(ch, '.' | ',' | ';' | ':' | ')' | ']' | '}')
                    })
                {
                    return None;
                }
                let close_whitespace =
                    matches!(before, ' ' | '\t') || (before == ',' && whitespace);
                return (whitespace == close_whitespace).then_some(cursor + count);
            }
            cursor += count;
            slashes = 0;
            continue;
        }
        slashes = if ch == '\\' { slashes + 1 } else { 0 };
        cursor += ch.len_utf8();
    }
    None
}
fn collect_inline_atoms(text: &str) -> Vec<Atom> {
    let math = math_ranges(text);
    let mut math_index = 0;
    let mut source_units = 0;
    let mut atoms = vec![];
    for (at, ch) in text.char_indices() {
        let start_units = source_units;
        source_units += ch.len_utf16();
        while math.get(math_index).is_some_and(|(_, end)| *end <= at) {
            math_index += 1;
        }
        if math
            .get(math_index)
            .is_some_and(|(s, e)| *s <= at && at < *e)
        {
            continue;
        }
        let before = text[..at].chars().next_back();
        let boundary = before.is_none_or(whitespace);
        let mut candidate = None;
        if ch == '!'
            && let Some(link) =
                link_at(text, at + 1).filter(|link| link.2 == AtomKind::ContextReference)
        {
            candidate = Some(link);
        } else if let Some(link) = link_at(text, at) {
            if link.2 == AtomKind::ContextReference
                || (boundary && text[link.0..].chars().next().is_some_and(whitespace))
            {
                candidate = Some(link);
            }
        } else if boundary && ch == '@' {
            let rest = &text[at + 1..];
            if let Some(quoted) = rest.strip_prefix('"') {
                let mut escape = false;
                for (i, c) in quoted.char_indices() {
                    if escape {
                        escape = false;
                        continue;
                    }
                    if c == '\\' {
                        escape = true;
                        continue;
                    }
                    if c == '"' {
                        let end = at + 2 + i + 1;
                        let path = unescape(&quoted[..i]);
                        if !path.is_empty() && text[end..].chars().next().is_some_and(whitespace) {
                            candidate = Some((end, path, AtomKind::Mention));
                        }
                        break;
                    }
                }
            } else {
                let count = rest
                    .char_indices()
                    .take_while(|(_, c)| !whitespace(*c) && !matches!(c, '@' | '"'))
                    .map(|(i, c)| i + c.len_utf8())
                    .last()
                    .unwrap_or(0);
                let path = &rest[..count];
                let end = at + 1 + count;
                let package = path.split_once('/').is_some_and(|(scope, name)| {
                    scope
                        .as_bytes()
                        .first()
                        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                        && scope.bytes().all(|b| {
                            b.is_ascii_lowercase()
                                || b.is_ascii_digit()
                                || matches!(b, b'.' | b'_' | b'-')
                        })
                        && name.split('/').next().is_some_and(|n| {
                            n.as_bytes()
                                .first()
                                .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                                && n.bytes().all(|b| {
                                    b.is_ascii_lowercase()
                                        || b.is_ascii_digit()
                                        || matches!(b, b'.' | b'_' | b'-')
                                })
                        })
                });
                let decorator = path.split_once('(').is_some_and(|(name, _)| {
                    name.chars()
                        .next()
                        .is_some_and(|c| c.is_alphabetic() || matches!(c, '_' | '$'))
                        && name
                            .chars()
                            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '$' | '.'))
                });
                if count > 0
                    && !package
                    && !decorator
                    && text[end..].chars().next().is_some_and(whitespace)
                {
                    candidate = Some((end, path.into(), AtomKind::Mention));
                }
            }
        } else if boundary && currency(ch) {
            let start = at + ch.len_utf8();
            let end = start
                + text[start..]
                    .bytes()
                    .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'_' | b'-'))
                    .count();
            let name = &text[start..end];
            if name
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
                && name.bytes().any(|b| b.is_ascii_alphabetic())
                && text[end..].chars().next().is_some_and(whitespace)
            {
                let amount = name.trim_end_matches(['k', 'K', 'm', 'M', 'b', 'B', 't', 'T']);
                let exponent = name.split_once(['e', 'E']);
                let is_amount = amount.bytes().all(|b| b.is_ascii_digit() || b == b'_')
                    || exponent.is_some_and(|(a, b)| {
                        !a.is_empty()
                            && !b.is_empty()
                            && a.bytes().all(|b| b.is_ascii_digit() || b == b'_')
                            && b.bytes().all(|b| b.is_ascii_digit())
                    });
                if !is_amount {
                    candidate = Some((end, name.into(), AtomKind::Skill));
                }
            }
        } else if text[at..].starts_with("model:")
            && before.is_none_or(|c| whitespace(c) || matches!(c, ',' | '(' | '[' | '"' | '\''))
        {
            let raw = &text[at + 6..];
            if let Some((instance, rest)) = raw.split_once('/')
                && instance
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphabetic)
                && instance
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            {
                let count = rest
                    .char_indices()
                    .take_while(|(_, c)| !whitespace(*c) && !matches!(c, ',' | '"' | '\''))
                    .map(|(i, c)| i + c.len_utf8())
                    .last()
                    .unwrap_or(0);
                let mut slug = &rest[..count];
                while let Some(last) = slug.chars().next_back() {
                    let bracket = match last {
                        ')' => Some('('),
                        ']' => Some('['),
                        _ => None,
                    };
                    if ".;:!?".contains(last)
                        || bracket.is_some_and(|open| {
                            slug.matches(last).count() > slug.matches(open).count()
                        })
                    {
                        slug = &slug[..slug.len() - last.len_utf8()];
                    } else {
                        break;
                    }
                }
                let end = at + 6 + instance.len() + 1 + slug.len();
                if !slug.is_empty() && text[end..].chars().next().is_some_and(whitespace) {
                    candidate = Some((end, format!("{instance}/{slug}"), AtomKind::Model));
                }
            }
        }
        if let Some((end, value, kind)) = candidate
            && atoms
                .last()
                .is_none_or(|a: &Atom| a.range.end <= start_units)
        {
            atoms.push(Atom {
                range: AtomRange {
                    start: start_units,
                    end: start_units + utf16_len(&text[at..end]),
                },
                kind,
                value,
                source: text[at..end].into(),
            });
        }
    }
    atoms
}
pub fn collect_atoms(text: &str) -> Vec<Atom> {
    let citations = super::composer::citations::collect(text);
    let mut references = Vec::new();
    let mut previous = 0;
    let mut units = 0;
    for reference in crate::view_logic::context_references::collect_references(text) {
        units += utf16_len(&text[previous..reference.start]);
        let end = units + utf16_len(&text[reference.start..reference.end]);
        references.push(Atom {
            range: AtomRange { start: units, end },
            kind: AtomKind::ContextReference,
            value: reference.context_id.into(),
            source: text[reference.start..reference.end].into(),
        });
        previous = reference.end;
        units = end;
    }
    let mut atoms = collect_inline_atoms(text);
    atoms.retain(|atom| {
        !citations.iter().any(|citation| {
            atom.range.start < citation.range.end && atom.range.end > citation.range.start
        }) && !references.iter().any(|reference| {
            atom.range.start < reference.range.end && atom.range.end > reference.range.start
        })
    });
    atoms.extend(citations.into_iter().map(|citation| Atom {
        range: citation.range,
        kind: AtomKind::Citation,
        value: citation.citation.text,
        source: citation.source,
    }));
    atoms.extend(references);
    atoms.sort_by_key(|atom| atom.range.start);
    atoms
}
pub fn selection_touches_mention_boundary(text: &str, selection: Selection) -> bool {
    if selection.collapsed() {
        return false;
    }
    let units: Vec<_> = text.encode_utf16().collect();
    collect_atoms(text)
        .iter()
        .filter(|atom| atom.kind == AtomKind::Mention)
        .any(|atom| {
            [atom.range.start.checked_sub(1), Some(atom.range.end)]
                .into_iter()
                .flatten()
                .any(|index| {
                    index >= selection.from()
                        && index < selection.to()
                        && units
                            .get(index)
                            .is_some_and(|u| super::rich_text::js_space(*u))
                })
        })
}
pub fn atom_ranges(value: &str) -> Vec<AtomRange> {
    collect_atoms(value).into_iter().map(|a| a.range).collect()
}
pub fn expand_cursor(text: &str, cursor: usize, literal: bool) -> usize {
    let cursor = cursor.min(utf16_len(text));
    if literal {
        return cursor;
    }
    let mut source = 0;
    let mut flat = 0;
    for atom in atom_ranges(text) {
        let plain = atom.start - source;
        if cursor <= flat + plain {
            return source + cursor - flat;
        }
        flat += plain;
        source = atom.start;
        if cursor <= flat + 1 {
            return if cursor == flat { source } else { atom.end };
        }
        flat += 1;
        source = atom.end;
    }
    (source + cursor.saturating_sub(flat)).min(utf16_len(text))
}
pub fn collapse_cursor(text: &str, cursor: usize, literal: bool) -> usize {
    let cursor = cursor.min(utf16_len(text));
    if literal {
        return cursor;
    }
    let mut removed = 0;
    for atom in atom_ranges(text) {
        if cursor <= atom.start {
            break;
        }
        if cursor < atom.end {
            return atom.start - removed + 1;
        }
        removed += atom.end - atom.start - 1;
    }
    cursor - removed
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fence {
    pub fence: String,
    pub language: String,
}
pub fn parse_opening_fence(line: &str) -> Option<Fence> {
    let ch = *line.as_bytes().first()?;
    if !matches!(ch, b'`' | b'~') || line.contains('\n') {
        return None;
    }
    let n = line.bytes().take_while(|c| *c == ch).count();
    if n < 3 {
        return None;
    }
    let language = &line[n..];
    if ch == b'`' && language.contains('`') {
        return None;
    }
    Some(Fence {
        fence: line[..n].into(),
        language: language.into(),
    })
}
pub fn is_closing_fence(line: &str, fence: &str) -> bool {
    let Some(ch) = line.as_bytes().first() else {
        return false;
    };
    if !matches!(ch, b'`' | b'~') || Some(ch) != fence.as_bytes().first() {
        return false;
    }
    let count = line.bytes().take_while(|c| c == ch).count();
    let tail = line[count..].strip_suffix('\r').unwrap_or(&line[count..]);
    count >= 3 && count >= fence.len() && tail.bytes().all(|b| matches!(b, b' ' | b'\t'))
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading {
        level: usize,
        prefix: String,
    },
    Quote {
        prefix: String,
    },
    ListItem {
        prefix: String,
    },
    Rule,
    Code {
        opening: Fence,
        content_start: usize,
        content_end: usize,
        close: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub start: usize,
    pub end: usize,
    pub kind: BlockKind,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    source: String,
    literal: bool,
    pub blocks: Vec<Block>,
}
fn prefix_kind(line: &str) -> BlockKind {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    if (1..=6).contains(&hashes) {
        let space = line[hashes..]
            .bytes()
            .take_while(|b| matches!(b, b' ' | b'\t'))
            .count();
        if space > 0 {
            return BlockKind::Heading {
                level: hashes,
                prefix: line[..hashes + space].into(),
            };
        }
    }
    if let Some(rest) = line.strip_prefix('>') {
        let space = rest
            .bytes()
            .take_while(|b| matches!(b, b' ' | b'\t'))
            .count();
        return BlockKind::Quote {
            prefix: line[..1 + space].into(),
        };
    }
    let trimmed = line.trim_end_matches([' ', '\t', '\r']);
    let rule: Vec<_> = trimmed
        .chars()
        .filter(|c| !matches!(c, ' ' | '\t'))
        .collect();
    if rule.len() >= 3 && matches!(rule[0], '-' | '*' | '_') && rule.iter().all(|c| *c == rule[0]) {
        return BlockKind::Rule;
    }
    let indent = line
        .bytes()
        .take_while(|b| matches!(b, b' ' | b'\t'))
        .count();
    let rest = &line[indent..];
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let marker = if digits > 0 && matches!(rest.as_bytes().get(digits), Some(b'.' | b')')) {
        digits + 1
    } else if matches!(rest.as_bytes().first(), Some(b'-' | b'*' | b'+')) {
        1
    } else {
        0
    };
    if marker > 0 {
        let padding = rest[marker..]
            .bytes()
            .take_while(|b| matches!(b, b' ' | b'\t'))
            .count();
        if padding > 0 || rest.len() == marker {
            let mut end = indent + marker + padding;
            let remaining = &line[end..];
            if marker == 1
                && rest.starts_with('-')
                && remaining.starts_with('[')
                && matches!(remaining.as_bytes().get(1), Some(b' ' | b'x' | b'X'))
                && remaining.as_bytes().get(2) == Some(&b']')
            {
                let space = remaining[3..]
                    .bytes()
                    .take_while(|b| matches!(b, b' ' | b'\t'))
                    .count();
                if space > 0 || remaining.len() == 3 {
                    end += 3 + space;
                }
            }
            return BlockKind::ListItem {
                prefix: line[..end].into(),
            };
        }
    }
    BlockKind::Paragraph
}
impl Document {
    pub fn parse(source: impl Into<String>, literal: bool) -> Self {
        let source = source.into();
        let lines: Vec<_> = source.split('\n').collect();
        let mut blocks = vec![];
        let mut index = 0;
        let mut byte = 0;
        let mut units = 0;
        while index < lines.len() {
            let line = lines[index];
            let start = units;
            if !literal && let Some(opening) = parse_opening_fence(line) {
                let content_byte = if byte + line.len() < source.len() {
                    byte + line.len() + 1
                } else {
                    source.len()
                };
                let mut next = index + 1;
                let mut end_byte = content_byte;
                while next < lines.len() && !is_closing_fence(lines[next], &opening.fence) {
                    end_byte += lines[next].len();
                    if next + 1 < lines.len() {
                        end_byte += 1;
                    }
                    next += 1;
                }
                let closed = next < lines.len();
                let content_end = if closed && next > index + 1 {
                    end_byte.saturating_sub(1)
                } else {
                    end_byte
                };
                let close = if closed {
                    format!("\n{}", lines[next])
                } else {
                    String::new()
                };
                let end_byte = if closed {
                    end_byte + lines[next].len()
                } else {
                    source.len()
                };
                let end_units = start + utf16_len(&source[byte..end_byte]);
                blocks.push(Block {
                    start,
                    end: end_units,
                    kind: BlockKind::Code {
                        opening,
                        content_start: start + utf16_len(&source[byte..content_byte]),
                        content_end: start + utf16_len(&source[byte..content_end]),
                        close,
                    },
                });
                byte = (end_byte + 1).min(source.len());
                units = end_units + usize::from(end_byte < source.len());
                index = if closed { next + 1 } else { lines.len() };
                continue;
            }
            blocks.push(Block {
                start,
                end: start + utf16_len(line),
                kind: if literal {
                    BlockKind::Paragraph
                } else {
                    prefix_kind(line)
                },
            });
            byte += line.len() + usize::from(index + 1 < lines.len());
            units += utf16_len(line) + usize::from(index + 1 < lines.len());
            index += 1;
        }
        Self {
            source,
            literal,
            blocks,
        }
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn is_literal(&self) -> bool {
        self.literal
    }
    pub fn replace(&self, selection: Selection, text: &str) -> Option<(Self, Selection)> {
        let edit = Edit {
            start: selection.from(),
            end: selection.to(),
            replacement: text.into(),
            cursor_after: selection.from() + utf16_len(text),
        };
        let next = edit.apply(&self.source)?;
        Some((
            Self::parse(next, self.literal),
            Selection::caret(edit.cursor_after),
        ))
    }
    pub fn inline(&self, block: &Block) -> Option<Vec<Span>> {
        let start = utf16_byte_offset(&self.source, block.start)?;
        let end = utf16_byte_offset(&self.source, block.end)?;
        let text = &self.source[start..end];
        Some(
            if self.literal || matches!(block.kind, BlockKind::Code { .. }) {
                vec![Span {
                    text: text.into(),
                    marks: vec![],
                }]
            } else {
                parse_inline_markdown(text)
            },
        )
    }
    fn code_at(&self, selection: Selection) -> Option<(&Block, usize, usize)> {
        utf16_byte_offset(&self.source, selection.from())?;
        utf16_byte_offset(&self.source, selection.to())?;
        self.blocks.iter().find_map(|block| {
            if let BlockKind::Code {
                content_start,
                content_end,
                ..
            } = &block.kind
            {
                (selection.from() >= *content_start && selection.to() <= *content_end).then_some((
                    block,
                    *content_start,
                    *content_end,
                ))
            } else {
                None
            }
        })
    }
    pub fn indent_code(&self, selection: Selection, out: bool) -> Option<(Self, Selection)> {
        let (_, start, end) = self.code_at(selection)?;
        if selection.collapsed() && !out {
            return self.replace(selection, "  ");
        }
        let first = utf16_byte_offset(&self.source, start)?;
        let last = utf16_byte_offset(&self.source, end)?;
        let text = &self.source[first..last];
        let from = utf16_byte_offset(text, selection.from() - start)?;
        let to = utf16_byte_offset(text, selection.to() - start)?;
        let line_start = text[..from].rfind('\n').map_or(0, |i| i + 1);
        let search = from.max(to.saturating_sub(1));
        let line_end = text.as_bytes()[search..]
            .iter()
            .position(|b| *b == b'\n')
            .map_or(text.len(), |i| search + i);
        let lines: Vec<_> = text[line_start..line_end].split('\n').collect();
        let next: Vec<_> = lines
            .iter()
            .map(|line| {
                if out {
                    line.strip_prefix("  ")
                        .or_else(|| line.strip_prefix('\t'))
                        .or_else(|| line.strip_prefix(' '))
                        .unwrap_or(line)
                        .to_string()
                } else if line.is_empty() {
                    String::new()
                } else {
                    format!("  {line}")
                }
            })
            .collect();
        let replacement = next.join("\n");
        let original = lines.join("\n");
        if replacement == original {
            return Some((self.clone(), selection));
        }
        let first_delta = utf16_len(&next[0]) as isize - utf16_len(lines[0]) as isize;
        let delta = utf16_len(&replacement) as isize - utf16_len(&original) as isize;
        let line_offset = start + utf16_len(&text[..line_start]);
        let next_from = selection
            .from()
            .saturating_add_signed(first_delta)
            .max(line_offset);
        let next_to = selection.to().saturating_add_signed(delta).max(next_from);
        let edit = Edit {
            start: line_offset,
            end: start + utf16_len(&text[..line_end]),
            replacement,
            cursor_after: next_to,
        };
        let source = edit.apply(&self.source)?;
        let selected = if selection.anchor <= selection.head {
            Selection {
                anchor: next_from,
                head: next_to,
            }
        } else {
            Selection {
                anchor: next_to,
                head: next_from,
            }
        };
        Some((Self::parse(source, self.literal), selected))
    }
    pub fn enter_code(&self, selection: Selection) -> Option<(Self, Selection)> {
        let (block, start, end) = self.code_at(selection)?;
        let BlockKind::Code { opening, close, .. } = &block.kind else {
            return None;
        };
        let byte = utf16_byte_offset(&self.source, selection.from())?;
        let content_start = utf16_byte_offset(&self.source, start)?;
        let content_end = utf16_byte_offset(&self.source, end)?;
        let before = &self.source[content_start..byte];
        if selection.collapsed() && selection.head == end {
            let trailing = before.trim_end_matches([' ', '\t']);
            if let Some(without_last) = trailing.strip_suffix('\n') {
                let previous = without_last.trim_end_matches([' ', '\t']);
                if let Some(body) = previous.strip_suffix('\n') {
                    let close = if close.is_empty() {
                        format!("\n{}", opening.fence)
                    } else {
                        close.clone()
                    };
                    let edit = Edit {
                        start: start + utf16_len(body),
                        end: block.end,
                        replacement: format!("{close}\n"),
                        cursor_after: start + utf16_len(body) + utf16_len(&close) + 1,
                    };
                    let value = edit.apply(&self.source)?;
                    return Some((
                        Self::parse(value, self.literal),
                        Selection::caret(edit.cursor_after),
                    ));
                }
            }
        }
        let line = before.rsplit('\n').next().unwrap_or("");
        let indent: String = line
            .chars()
            .take_while(|c| matches!(c, ' ' | '\t'))
            .collect();
        let _ = content_end;
        self.replace(selection, &format!("\n{indent}"))
    }
    pub fn unwrap_code_at_start(&self, selection: Selection) -> Option<(Self, Selection)> {
        let (block, start, end) = self.code_at(selection)?;
        if !selection.collapsed() || selection.head != start {
            return None;
        }
        let a = utf16_byte_offset(&self.source, start)?;
        let b = utf16_byte_offset(&self.source, end)?;
        let edit = Edit {
            start: block.start,
            end: block.end,
            replacement: self.source[a..b].into(),
            cursor_after: block.start,
        };
        Some((
            Self::parse(edit.apply(&self.source)?, self.literal),
            Selection::caret(block.start),
        ))
    }
    pub fn surround(&self, selection: Selection, mark: Mark) -> Option<(Self, Selection)> {
        if selection.collapsed() {
            return None;
        }
        let a = utf16_byte_offset(&self.source, selection.from())?;
        let b = utf16_byte_offset(&self.source, selection.to())?;
        let text = &self.source[a..b];
        let delimiter = mark.delimiter();
        let replacement = format!("{delimiter}{text}{delimiter}");
        let edit = Edit {
            start: selection.from(),
            end: selection.to(),
            replacement,
            cursor_after: selection.to() + utf16_len(delimiter),
        };
        let next = Self::parse(edit.apply(&self.source)?, self.literal);
        Some((
            next,
            Selection {
                anchor: selection.anchor + utf16_len(delimiter),
                head: selection.head + utf16_len(delimiter),
            },
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn utf16_boundaries() {
        assert_eq!(utf16_len("a😀b"), 4);
        assert_eq!(utf16_byte_offset("a😀b", 2), None);
        assert_eq!(utf16_byte_offset("a😀b", 3), Some(5));
    }
    #[test]
    fn literal_lossless() {
        for source in [
            "",
            "\n\n",
            "  - [X] done\n\n**keep** €my-skill @README.md  \n",
            "line one\r\nline two\r\n",
        ] {
            let doc = Document::parse(source, true);
            assert_eq!(doc.source(), source);
            assert!(doc.blocks.iter().all(|b| b.kind == BlockKind::Paragraph));
        }
    }
    #[test]
    fn source_lossless() {
        for source in [
            "#  heading\n> quote\n- [X] item\n---",
            "````ts\n```\n````",
            "~~~py\nx\n~~~\nprose",
            "```\nunclosed\n",
            "- 😀\n\t1) item",
        ] {
            let doc = Document::parse(source, false);
            assert_eq!(doc.source(), source);
            for b in &doc.blocks {
                assert!(b.start <= b.end);
                assert!(utf16_byte_offset(source, b.end).is_some());
            }
        }
    }
    #[test]
    fn opening_and_closing() {
        for (source, fence, language) in [
            ("```", "```", ""),
            ("```ts", "```", "ts"),
            ("~~~py", "~~~", "py"),
            ("````", "````", ""),
            ("```ts title=example", "```", "ts title=example"),
            ("``` js", "```", " js"),
        ] {
            assert_eq!(
                parse_opening_fence(source),
                Some(Fence {
                    fence: fence.into(),
                    language: language.into()
                })
            );
        }
        for source in ["``", "text ```", "```a`b"] {
            assert!(parse_opening_fence(source).is_none());
        }
        assert!(is_closing_fence("```` \t\r", "```"));
        assert!(!is_closing_fence(" ```", "```"));
        assert!(!is_closing_fence("~~~", "```"));
    }
    #[test]
    fn code_indentation_and_cross_block_selection() {
        let source = "```\none\ntwo\nthree\n```";
        let doc = Document::parse(source, false);
        let (next, s) = doc
            .indent_code(
                Selection {
                    anchor: 5,
                    head: 13,
                },
                false,
            )
            .unwrap();
        assert_eq!(next.source(), "```\n  one\n  two\n  three\n```");
        assert_eq!(
            s,
            Selection {
                anchor: 7,
                head: 19
            }
        );
        let (next, _) = doc
            .indent_code(Selection { anchor: 4, head: 8 }, false)
            .unwrap();
        assert_eq!(next.source(), "```\n  one\ntwo\nthree\n```");
        assert!(
            Document::parse("```\na\n```\n```\na\n```", false)
                .indent_code(
                    Selection {
                        anchor: 4,
                        head: 15
                    },
                    false
                )
                .is_none()
        );
    }
    #[test]
    fn code_indent_after_supplementary_character() {
        let doc = Document::parse("```\n😀\n```", false);
        let (next, selection) = doc
            .indent_code(Selection { anchor: 4, head: 6 }, false)
            .unwrap();
        assert_eq!(next.source(), "```\n  😀\n```");
        assert_eq!(selection, Selection { anchor: 6, head: 8 });
    }
    #[test]
    fn code_enter_copies_indent() {
        let d = Document::parse("```\n    deep()\nflush()\n```", false);
        let (n, _) = d.enter_code(Selection::caret(14)).unwrap();
        assert_eq!(n.source(), "```\n    deep()\n    \nflush()\n```");
    }
    #[test]
    fn code_exit_two_blanks_and_unwrap() {
        let d = Document::parse("```\nflush()\n\n", false);
        let (n, s) = d.enter_code(Selection::caret(13)).unwrap();
        assert_eq!(n.source(), "```\nflush()\n```\n");
        assert_eq!(s.head, 16);
        let (n, s) = Document::parse("```ts\na\nb\n```", false)
            .unwrap_code_at_start(Selection::caret(6))
            .unwrap();
        assert_eq!(n.source(), "a\nb");
        assert_eq!(s.head, 0);
    }
    #[test]
    fn atoms_and_positions() {
        let source = "😀 @README.md €skill model:codex/gpt-6 ";
        let atoms = collect_atoms(source);
        assert_eq!(atoms.len(), 3);
        for atom in atoms {
            assert_eq!(
                collapse_cursor(source, atom.range.start + 1, false),
                collapse_cursor(source, atom.range.end, false)
            );
        }
        let length = collapse_cursor(source, utf16_len(source), false);
        for cursor in 0..=length {
            assert_eq!(
                collapse_cursor(source, expand_cursor(source, cursor, false), false),
                cursor
            );
        }
        assert!(collect_atoms("@foo/bar @decorator(x) $100k ").is_empty());
    }
    #[test]
    fn multiline_and_math() {
        assert_eq!(
            collect_atoms("@\"file\n2. name\" ")[0].value,
            "file\n2. name"
        );
        assert!(collect_atoms("before $x + @README.md$ after").is_empty());
        assert!(
            collect_atoms("\\[\\text{use $browser then @README.md and **literal**}\\]").is_empty()
        );
        assert_eq!(
            collect_atoms("[posts.$id.tsx](app/posts.%24id.tsx) ").len(),
            1
        );
    }
    #[test]
    fn context_links() {
        let tokens = collect_atoms("[label](t3-context://v1/thread/ctx_1)");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, AtomKind::ContextReference);
        assert!(parse_context_href("t3-context://v1/thread/id/extra").is_none());
    }
}
