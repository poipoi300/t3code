//! Inline Markdown parsing retains source offsets in browser UTF-16 units.
mod literal_underscore_runs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Bold,
    Italic,
    Strike,
    Code,
}
impl Mark {
    pub fn delimiter(self) -> &'static str {
        match self {
            Self::Bold => "**",
            Self::Italic => "*",
            Self::Strike => "~~",
            Self::Code => "`",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub marks: Vec<Mark>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePiece {
    pub start: usize,
    pub end: usize,
    pub marks: Vec<Mark>,
}
struct Frame {
    delimiter: Vec<u16>,
    mark: Mark,
    start: usize,
    pieces: Vec<SourcePiece>,
}
fn push(pieces: &mut Vec<SourcePiece>, start: usize, end: usize, marks: Vec<Mark>) {
    if end <= start {
        return;
    }
    if let Some(last) = pieces
        .last_mut()
        .filter(|last| last.end == start && last.marks == marks)
    {
        last.end = end;
    } else {
        pieces.push(SourcePiece { start, end, marks });
    }
}
fn append(
    root: &mut Vec<SourcePiece>,
    stack: &mut [Frame],
    start: usize,
    end: usize,
    marks: Vec<Mark>,
) {
    push(
        stack.last_mut().map_or(root, |f| &mut f.pieces),
        start,
        end,
        marks,
    );
}
pub(crate) fn js_space(ch: u16) -> bool {
    matches!(ch, 0x9..=0xd | 0x20 | 0xa0 | 0x1680 | 0x2000..=0x200a | 0x2028..=0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff)
}
fn word(ch: u16) -> bool {
    matches!(ch, 48..=57 | 65..=90 | 97..=122 | 95)
}
pub fn parse_inline_pieces(text: &str) -> Vec<SourcePiece> {
    let literal_runs = literal_underscore_runs::starts(text);
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut root = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut index = 0;
    while index < units.len() {
        let ch = units[index];
        if ch == 92 {
            append(
                &mut root,
                &mut stack,
                index,
                (index + 2).min(units.len()),
                vec![],
            );
            index += 2;
            continue;
        }
        if ch == 96 {
            let length = units[index..].iter().take_while(|u| **u == 96).count();
            let close = units[index + length..]
                .windows(length)
                .position(|s| s == &units[index..index + length])
                .map(|p| p + index + length);
            if length == 1
                && close.is_some_and(|c| {
                    c > index + 1 && !units[index..c].contains(&10) && units.get(c + 1) != Some(&96)
                })
            {
                let c = close.unwrap_or(index);
                append(&mut root, &mut stack, index + 1, c, vec![Mark::Code]);
                index = c + 1;
            } else {
                append(&mut root, &mut stack, index, index + length, vec![]);
                index += length;
            }
            continue;
        }
        if !matches!(ch, 42 | 95 | 126) {
            append(&mut root, &mut stack, index, index + 1, vec![]);
            index += 1;
            continue;
        }
        let end = index + units[index..].iter().take_while(|u| **u == ch).count();
        if literal_runs.contains(&index) {
            append(&mut root, &mut stack, index, end, vec![]);
            index = end;
            continue;
        }
        let before = index.checked_sub(1).and_then(|i| units.get(i)).copied();
        let after = units.get(end).copied();
        let can_close =
            before.is_some_and(|u| !js_space(u)) && (ch != 95 || !after.is_some_and(word));
        let can_open =
            after.is_some_and(|u| !js_space(u)) && (ch != 95 || !before.is_some_and(word));
        while index < end {
            let nested_bold = stack.last().is_some_and(|f| f.mark == Mark::Italic)
                && end - index == 2
                && can_open
                && matches!(ch, 42 | 95)
                && !stack.iter().any(|f| f.mark == Mark::Bold);
            let closes = !nested_bold
                && can_close
                && stack.last().is_some_and(|f| {
                    index + f.delimiter.len() <= end && units[index..].starts_with(&f.delimiter)
                });
            if closes {
                if let Some(frame) = stack.pop() {
                    for piece in frame.pieces {
                        let mut marks = vec![frame.mark];
                        marks.extend(piece.marks);
                        append(&mut root, &mut stack, piece.start, piece.end, marks);
                    }
                    index += frame.delimiter.len();
                }
            } else if can_open && (ch != 126 || end - index >= 2) {
                let length = if ch == 126 || end - index >= 2 { 2 } else { 1 };
                let mark = if ch == 126 {
                    Mark::Strike
                } else if length == 2 {
                    Mark::Bold
                } else {
                    Mark::Italic
                };
                if stack.iter().any(|f| f.mark == mark) {
                    append(&mut root, &mut stack, index, end, vec![]);
                    index = end;
                    continue;
                }
                stack.push(Frame {
                    delimiter: vec![ch; length],
                    mark,
                    start: index,
                    pieces: vec![],
                });
                index += length;
            } else {
                append(&mut root, &mut stack, index, end, vec![]);
                index = end;
            }
        }
    }
    while let Some(frame) = stack.pop() {
        append(
            &mut root,
            &mut stack,
            frame.start,
            frame.start + frame.delimiter.len(),
            vec![],
        );
        for piece in frame.pieces {
            append(&mut root, &mut stack, piece.start, piece.end, piece.marks);
        }
    }
    root
}
pub fn parse_inline_markdown(text: &str) -> Vec<Span> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut spans: Vec<Span> = Vec::new();
    for p in parse_inline_pieces(text) {
        let value = String::from_utf16_lossy(&units[p.start..p.end]);
        if let Some(last) = spans.last_mut().filter(|last| last.marks == p.marks) {
            last.text.push_str(&value);
        } else {
            spans.push(Span {
                text: value,
                marks: p.marks,
            });
        }
    }
    spans
}
pub fn inline_markdown_unit_marks(text: &str) -> Vec<Option<Vec<Mark>>> {
    let mut result = vec![None; text.encode_utf16().count()];
    for piece in parse_inline_pieces(text) {
        result[piece.start..piece.end].fill(Some(piece.marks));
    }
    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn spans(input: &str, expected: &[(&str, &[Mark])]) {
        assert_eq!(
            parse_inline_markdown(input),
            expected
                .iter()
                .map(|(text, marks)| Span {
                    text: (*text).into(),
                    marks: marks.to_vec()
                })
                .collect::<Vec<_>>()
        );
    }
    #[test]
    fn unmatched_escapes_identifiers() {
        for text in [
            "plain text",
            "snake_case",
            "unmatched **",
            "** spaced **",
            r"\*literal\*",
        ] {
            spans(text, &[(text, &[])]);
        }
    }
    #[test]
    fn nested() {
        use Mark::*;
        spans("***both***", &[("both", &[Bold, Italic])]);
        spans(
            "*a **b** c*",
            &[("a ", &[Italic]), ("b", &[Italic, Bold]), (" c", &[Italic])],
        );
        spans(
            "**a `code` c**",
            &[("a ", &[Bold]), ("code", &[Bold, Code]), (" c", &[Bold])],
        );
    }
    #[test]
    fn crossing() {
        use Mark::*;
        spans(
            "*a**b*****c**",
            &[("a", &[Italic]), ("b", &[Italic, Bold]), ("c", &[Bold])],
        );
    }
    #[test]
    fn bounded_repeated_delimiters() {
        let repeated = "*".repeat(10_000);
        spans(
            &format!("{repeated}text{repeated}"),
            &[
                (&format!("{}text", &repeated[2..]), &[Mark::Bold]),
                (&repeated[2..], &[]),
            ],
        );
        let alternate = "__".repeat(5_000);
        spans(
            &format!("**~~{alternate}text{alternate}~~**"),
            &[(
                &format!("{alternate}text{alternate}"),
                &[Mark::Bold, Mark::Strike],
            )],
        );
    }
    #[test]
    fn code_contents_literal() {
        spans("`**not bold**`", &[("**not bold**", &[Mark::Code])]);
    }
    #[test]
    fn source_unit_mapping() {
        use Mark::*;
        assert_eq!(
            inline_markdown_unit_marks("**`*` w** \\*"),
            vec![
                None,
                None,
                None,
                Some(vec![Bold, Code]),
                None,
                Some(vec![Bold]),
                Some(vec![Bold]),
                None,
                None,
                Some(vec![]),
                Some(vec![]),
                Some(vec![])
            ]
        );
        assert_eq!(
            inline_markdown_unit_marks("**😀**"),
            vec![None, None, Some(vec![Bold]), Some(vec![Bold]), None, None]
        );
    }
}
