//! Source-preserving list transactions; all offsets count UTF-16 units.
use super::document::{AtomRange, Edit, atom_ranges, utf16_len};
#[derive(Debug, Clone)]
enum Kind {
    Ordered { number: String, delimiter: char },
    Task,
    Bullet(char),
}
#[derive(Debug, Clone)]
struct Marker {
    kind: Kind,
    indent: String,
    space: String,
    end: usize,
    content_column: usize,
}
fn indent_width(value: &str, from: usize) -> usize {
    let mut column = from;
    for ch in value.chars() {
        match ch {
            ' ' => column += 1,
            '\t' => column += 4 - column % 4,
            _ => break,
        }
    }
    column - from
}
fn parse_marker(line: &str) -> Option<Marker> {
    let indent_end = line
        .bytes()
        .take_while(|c| matches!(c, b' ' | b'\t'))
        .count();
    let indent = &line[..indent_end];
    let rest = &line[indent_end..];
    let number_end = rest.bytes().take_while(u8::is_ascii_digit).count();
    let (kind, marker_width, space_start, content_padding) =
        if number_end > 0 && matches!(rest.as_bytes().get(number_end), Some(b'.' | b')')) {
            (
                Kind::Ordered {
                    number: rest[..number_end].into(),
                    delimiter: rest.as_bytes()[number_end] as char,
                },
                number_end + 1,
                number_end + 1,
                None,
            )
        } else if rest.starts_with('-')
            && rest.chars().nth(1).is_some_and(char::is_whitespace)
            && rest.as_bytes().get(2) == Some(&b'[')
            && matches!(rest.as_bytes().get(3), Some(b' ' | b'x' | b'X'))
            && rest.as_bytes().get(4) == Some(&b']')
        {
            (Kind::Task, 1, 5, Some(&rest[1..2]))
        } else if matches!(rest.as_bytes().first(), Some(b'-' | b'*' | b'+')) {
            (Kind::Bullet(rest.as_bytes()[0] as char), 1, 1, None)
        } else {
            return None;
        };
    let tail = &rest[space_start..];
    let padding_end = tail
        .bytes()
        .take_while(|c| matches!(c, b' ' | b'\t'))
        .count();
    if padding_end == 0 && !tail.trim().is_empty() {
        return None;
    }
    let space = if padding_end == 0 {
        tail
    } else {
        &tail[..padding_end]
    };
    let marker_end = indent_width(indent, 0) + marker_width;
    let padding = indent_width(content_padding.unwrap_or(space), marker_end);
    Some(Marker {
        kind,
        indent: indent.into(),
        space: space.into(),
        end: indent_end + space_start + space.len(),
        content_column: marker_end
            + if (1..=4).contains(&padding) {
                padding
            } else {
                1
            },
    })
}
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
fn safe_number(text: &str) -> Option<u64> {
    text.parse::<u64>().ok().filter(|n| *n <= MAX_SAFE_INTEGER)
}
pub fn next_ordered_marker_text(marker: &str) -> String {
    let Some(delimiter) = marker.chars().last() else {
        return String::new();
    };
    let number = &marker[..marker.len() - delimiter.len_utf8()];
    let next = safe_number(number).map_or_else(
        || number.to_string(),
        |n| format!("{:0width$}", n + 1, width = number.len()),
    );
    format!("{next}{delimiter}")
}
fn next_marker(marker: &Marker) -> String {
    let value = match &marker.kind {
        Kind::Ordered { number, delimiter } => {
            next_ordered_marker_text(&format!("{number}{delimiter}"))
        }
        Kind::Task => "- [ ]".into(),
        Kind::Bullet(c) => c.to_string(),
    };
    format!("{}{value} ", marker.indent)
}
fn inside(tokens: &[AtomRange], byte: usize, value: &str) -> bool {
    let cursor = utf16_len(&value[..byte]);
    tokens.iter().any(|t| t.start < cursor && cursor < t.end)
}
fn line_range(value: &str, cursor: usize) -> (usize, usize) {
    let start = value[..cursor].rfind('\n').map_or(0, |i| i + 1);
    let end = value[cursor..]
        .find('\n')
        .map_or(value.len(), |i| cursor + i);
    (start, end)
}
fn markers_above(value: &str, mut line_start: usize, tokens: &[AtomRange]) -> Vec<Marker> {
    let mut result = vec![];
    while line_start > 0 {
        let line_end = line_start - 1;
        line_start = value[..line_end].rfind('\n').map_or(0, |i| i + 1);
        loop {
            let at = utf16_len(&value[..line_start]);
            let token = tokens.iter().find(|t| t.start < at && t.end > at);
            let Some(token) = token else {
                break;
            };
            let Some(byte) = super::document::utf16_byte_offset(value, token.start) else {
                break;
            };
            line_start = value[..byte].rfind('\n').map_or(0, |i| i + 1);
        }
        let Some(marker) = parse_marker(&value[line_start..line_end]) else {
            break;
        };
        result.push(marker);
    }
    result
}
fn parent(value: &str, line_start: usize, marker: &Marker, tokens: &[AtomRange]) -> Option<Marker> {
    let width = indent_width(&marker.indent, 0);
    markers_above(value, line_start, tokens)
        .into_iter()
        .find(|m| indent_width(&m.indent, 0) < width)
}
fn renumber(
    value: &str,
    after: usize,
    marker: &Marker,
    tokens: &[AtomRange],
) -> Option<(usize, String)> {
    let Kind::Ordered { number, delimiter } = &marker.kind else {
        return None;
    };
    let mut expected = safe_number(number)?.checked_add(2)?;
    let base = indent_width(&marker.indent, 0);
    let mut position = after;
    let mut copied = after;
    let mut parts = String::new();
    while position < value.len() && expected <= MAX_SAFE_INTEGER {
        let start = position + 1;
        let end = value[start..].find('\n').map_or(value.len(), |i| start + i);
        position = end;
        if inside(tokens, start, value) {
            continue;
        }
        let line = &value[start..end];
        let width = indent_width(line, 0);
        if line.trim().is_empty() || width < base {
            break;
        }
        if width > base {
            continue;
        }
        let Some(current) = parse_marker(line) else {
            break;
        };
        let Kind::Ordered {
            number: current_number,
            delimiter: current_delimiter,
        } = &current.kind
        else {
            break;
        };
        if current_delimiter != delimiter {
            break;
        }
        let number_start = start + current.indent.len();
        let number_end = number_start + current_number.len();
        let replacement = if current_number.starts_with('0') {
            format!("{expected:0width$}", width = current_number.len())
        } else {
            expected.to_string()
        };
        if replacement != *current_number {
            parts.push_str(&value[copied..number_start]);
            parts.push_str(&replacement);
            copied = number_end;
        }
        expected += 1;
    }
    (!parts.is_empty()).then_some((copied, parts))
}
fn edit(value: &str, start: usize, end: usize, replacement: String, cursor_after: usize) -> Edit {
    Edit {
        start: utf16_len(&value[..start]),
        end: utf16_len(&value[..end]),
        replacement,
        cursor_after,
    }
}
pub fn continuation_for_enter(value: &str, cursor: usize) -> Option<Edit> {
    continuation_with_atoms(value, cursor, &atom_ranges(value))
}
pub fn continuation_with_atoms(value: &str, cursor: usize, tokens: &[AtomRange]) -> Option<Edit> {
    let at = super::document::utf16_byte_offset(value, cursor)?;
    let (start, end) = line_range(value, at);
    let marker = parse_marker(&value[start..end])?;
    if marker.space.is_empty() || at < start + marker.end || inside(tokens, at, value) {
        return None;
    }
    if value[start + marker.end..end].trim().is_empty() {
        let p = parent(value, start, &marker, tokens);
        let replacement = p.as_ref().map_or_else(String::new, next_marker);
        let cursor_after = utf16_len(&value[..start]) + utf16_len(&replacement);
        let remove_end = at.max(start + marker.end);
        if let Some((stop, following)) = p.as_ref().and_then(|p| renumber(value, end, p, tokens)) {
            return Some(edit(
                value,
                start,
                stop,
                format!("{replacement}{}{following}", &value[remove_end..end]),
                cursor_after,
            ));
        }
        return Some(edit(value, start, remove_end, replacement, cursor_after));
    }
    let insertion = format!("\n{}", next_marker(&marker));
    let cursor_after = cursor + utf16_len(&insertion);
    if let Some((stop, following)) = renumber(value, end, &marker, tokens) {
        return Some(edit(
            value,
            at,
            stop,
            format!("{insertion}{}{following}", &value[at..end]),
            cursor_after,
        ));
    }
    Some(edit(value, at, at, insertion, cursor_after))
}
fn nested_number(marker: &Marker, previous: Option<&Marker>) -> (usize, String) {
    let Kind::Ordered { number, delimiter } = &marker.kind else {
        return (0, String::new());
    };
    let text = if let Some(Marker {
        kind: Kind::Ordered {
            number,
            delimiter: prior,
        },
        ..
    }) = previous.filter(|p| matches!(p.kind, Kind::Ordered { .. }))
    {
        if prior == delimiter {
            let next = next_ordered_marker_text(&format!("{number}{delimiter}"));
            next[..next.len() - 1].into()
        } else {
            "1".into()
        }
    } else {
        "1".into()
    };
    (number.len(), text)
}
pub fn indent_for_tab(value: &str, start: usize, end: usize) -> Option<Edit> {
    if start != end {
        return None;
    }
    let at = super::document::utf16_byte_offset(value, start)?;
    let (line_start, line_end) = line_range(value, at);
    let marker = parse_marker(&value[line_start..line_end])?;
    let tokens = atom_ranges(value);
    if inside(&tokens, at, value) {
        return None;
    }
    let width = indent_width(&marker.indent, 0);
    let mut child: Option<Marker> = None;
    for above in markers_above(value, line_start, &tokens) {
        let above_width = indent_width(&above.indent, 0);
        if above_width < width {
            break;
        }
        if above_width > width {
            if child
                .as_ref()
                .is_none_or(|c| above_width < indent_width(&c.indent, 0))
            {
                child = Some(above);
            }
            continue;
        }
        let indent = child.as_ref().map_or_else(
            || {
                format!(
                    "{}{}",
                    above.indent,
                    " ".repeat(above.content_column - above_width)
                )
            },
            |c| c.indent.clone(),
        );
        let (length, text) = nested_number(&marker, child.as_ref());
        let replaced_end = line_start + marker.indent.len() + length;
        let replacement = format!("{indent}{text}");
        let caret = if at < replaced_end {
            utf16_len(&value[..line_start]) + indent.len()
        } else {
            start - utf16_len(&value[line_start..replaced_end]) + utf16_len(&replacement)
        };
        return Some(edit(value, line_start, replaced_end, replacement, caret));
    }
    Some(edit(value, line_start, line_start, "  ".into(), start + 2))
}
pub fn outdent_for_tab(value: &str, start: usize, end: usize) -> Option<Edit> {
    if start != end {
        return None;
    }
    let at = super::document::utf16_byte_offset(value, start)?;
    let (line_start, line_end) = line_range(value, at);
    let marker = parse_marker(&value[line_start..line_end])?;
    if marker.indent.is_empty() {
        return None;
    }
    let tokens = atom_ranges(value);
    if inside(&tokens, at, value) {
        return None;
    }
    let p = parent(value, line_start, &marker, &tokens);
    let indent = p.as_ref().map_or_else(
        || {
            marker.indent[if marker.indent.starts_with('\t') {
                1
            } else {
                marker
                    .indent
                    .bytes()
                    .take_while(|b| *b == b' ')
                    .count()
                    .min(2)
            }..]
                .to_string()
        },
        |p| p.indent.clone(),
    );
    let continues = matches!((&p,&marker.kind),(Some(Marker{kind:Kind::Ordered{delimiter:a,..},..}),Kind::Ordered{delimiter:b,..}) if a==b);
    let (length, text) = if continues {
        nested_number(&marker, p.as_ref())
    } else {
        (0, String::new())
    };
    let replaced_end = line_start + marker.indent.len() + length;
    let replacement = format!("{indent}{text}");
    let caret = if at < replaced_end {
        utf16_len(&value[..line_start]) + indent.len()
    } else {
        start - utf16_len(&value[line_start..replaced_end]) + utf16_len(&replacement)
    };
    if let Some((stop, following)) = p
        .as_ref()
        .filter(|_| continues)
        .and_then(|p| renumber(value, line_end, p, &tokens))
    {
        return Some(edit(
            value,
            line_start,
            stop,
            format!("{replacement}{}{following}", &value[replaced_end..line_end]),
            caret,
        ));
    }
    Some(edit(value, line_start, replaced_end, replacement, caret))
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn continuation() {
        for (input, cursor, expected) in [
            ("- foo", 5, "- foo\n- "),
            ("  * foo", 7, "  * foo\n  * "),
            ("- [x] done", 10, "- [x] done\n- [ ] "),
            ("1. foobar", 5, "1. fo\n2. obar"),
            ("1. foo\n2. ", 10, "1. foo\n"),
            ("- ", 2, ""),
            ("1)\t", 3, ""),
            ("- [ ] ", 6, ""),
        ] {
            assert_eq!(
                continuation_for_enter(input, cursor)
                    .unwrap()
                    .apply(input)
                    .unwrap(),
                expected
            );
        }
    }
    #[test]
    fn lift_empty_nested() {
        let mut value = "1. parent\n   1. child\n      1. ".to_string();
        for expected in [
            "1. parent\n   1. child\n   2. ",
            "1. parent\n   1. child\n2. ",
            "1. parent\n   1. child\n",
        ] {
            value = continuation_for_enter(&value, utf16_len(&value))
                .unwrap()
                .apply(&value)
                .unwrap();
            assert_eq!(value, expected);
        }
    }
    #[test]
    fn lift_renumbers() {
        let value = "1. a\n   1. \n2. b";
        let e = continuation_for_enter(value, value.find("\n2.").unwrap()).unwrap();
        assert_eq!(e.apply(value).unwrap(), "1. a\n2. \n3. b");
        assert_eq!(e.cursor_after, 8);
    }
    #[test]
    fn atomic_interiors() {
        for value in ["1. @README.md go", "1. 𑿝review go"] {
            let cursor = utf16_len(&value[..value.find(" go").unwrap()]) - 1;
            assert!(continuation_for_enter(value, cursor).is_none());
        }
    }
    #[test]
    fn indent() {
        for (value, expected) in [
            ("1. parent\n2. child", "1. parent\n   1. child"),
            ("10. parent\n11. child", "10. parent\n    1. child"),
            ("1.\tparent\n2. child", "1.\tparent\n    1. child"),
            (
                "1. @\"file\n   9. name\"\n2. child",
                "1. @\"file\n   9. name\"\n   1. child",
            ),
            (
                "1. parent\n   1. first\n2. child",
                "1. parent\n   1. first\n   2. child",
            ),
        ] {
            let cursor = value.rfind("child").unwrap() + 2;
            let e = indent_for_tab(value, cursor, cursor).unwrap();
            assert_eq!(e.apply(value).unwrap(), expected);
            assert_eq!(e.cursor_after, expected.rfind("child").unwrap() + 2);
        }
    }
    #[test]
    fn indent_outdent_roundtrip() {
        for value in ["- parent\n- child", "1. parent\n2. child", "  7. child"] {
            let cursor = value.rfind("child").unwrap() + 2;
            let nest = indent_for_tab(value, cursor, cursor).unwrap();
            let nested = nest.apply(value).unwrap();
            let e = outdent_for_tab(&nested, nest.cursor_after, nest.cursor_after).unwrap();
            assert_eq!(e.apply(&nested).unwrap(), value);
            assert_eq!(e.cursor_after, cursor);
        }
    }
    #[test]
    fn outdent_parent() {
        for (value, expected) in [
            ("1. a\n   1. b\n2. c", "1. a\n2. b\n3. c"),
            ("- a\n  5. b", "- a\n5. b"),
        ] {
            let c = value.find('b').unwrap();
            assert_eq!(
                outdent_for_tab(value, c, c).unwrap().apply(value).unwrap(),
                expected
            );
        }
        assert!(outdent_for_tab("- item", 6, 6).is_none());
    }
    #[test]
    fn ordered_count() {
        for (a, b) in [
            ("1.", "2."),
            ("1)", "2)"),
            ("09.", "10."),
            ("001)", "002)"),
            ("99.", "100."),
            ("99999999999999999999.", "99999999999999999999."),
            ("9007199254740991.", "9007199254740992."),
        ] {
            assert_eq!(next_ordered_marker_text(a), b);
        }
    }
    #[test]
    fn renumber_following() {
        for (value, cursor, expected) in [
            (
                "1. @README.md go\n2. two\n03. three\n10. four",
                14,
                "1. @README.md \n2. go\n3. two\n04. three\n5. four",
            ),
            (
                "  1) a\n\t1) child\n  2) b\n1) parent\n  3) untouched",
                6,
                "  1) a\n  2) \n\t1) child\n  3) b\n1) parent\n  3) untouched",
            ),
            (
                "1. a\n2. @\"file\n2. name\" \n3. b",
                4,
                "1. a\n2. \n3. @\"file\n2. name\" \n4. b",
            ),
        ] {
            let e = continuation_for_enter(value, cursor).unwrap();
            assert_eq!(e.apply(value).unwrap(), expected);
        }
    }
    #[test]
    fn renumber_boundaries() {
        for boundary in ["", "text", "1) other list"] {
            let value = format!("1. a\n2. b\n{boundary}\n3. untouched");
            let e = continuation_for_enter(&value, 4).unwrap();
            assert_eq!(
                e.apply(&value).unwrap(),
                format!("1. a\n2. \n3. b\n{boundary}\n3. untouched")
            );
        }
    }
    #[test]
    fn invalid_and_utf16_caret() {
        assert!(continuation_for_enter("- 😀", 3).is_none());
        assert_eq!(
            continuation_for_enter("- 😀", 4)
                .unwrap()
                .apply("- 😀")
                .unwrap(),
            "- 😀\n- "
        );
        assert!(indent_for_tab("- x", 1, 2).is_none());
    }
}
