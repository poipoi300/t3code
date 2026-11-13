use super::super::{
    document::{BlockKind, Document, utf16_byte_offset},
    rich_text::{Mark, parse_inline_markdown},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Ltr,
    Rtl,
}
fn rtl(c: char) -> bool {
    matches!(c,'\u{0590}'..='\u{08ff}'|'\u{fb1d}'..='\u{fdff}'|'\u{fe70}'..='\u{feff}'|'\u{10800}'..='\u{10fff}'|'\u{1e800}'..='\u{1efff}')
}
fn majority(units: impl IntoIterator<Item = Option<Direction>>) -> Option<Direction> {
    let mut first = None;
    let mut right = 0;
    let mut left = 0;
    for direction in units.into_iter().flatten() {
        first.get_or_insert(direction);
        match direction {
            Direction::Ltr => left += 1,
            Direction::Rtl => right += 1,
        }
    }
    if right == left {
        first
    } else {
        Some(if right > left {
            Direction::Rtl
        } else {
            Direction::Ltr
        })
    }
}
fn words(text: &str) -> Option<Direction> {
    majority(text.split_whitespace().map(|word| {
        word.chars().find(|c| c.is_alphabetic()).map(|c| {
            if rtl(c) {
                Direction::Rtl
            } else {
                Direction::Ltr
            }
        })
    }))
}
pub fn text_direction(text: &str) -> Option<Direction> {
    if !text.chars().any(rtl) {
        return text
            .chars()
            .any(char::is_alphabetic)
            .then_some(Direction::Ltr);
    }
    let mut sentences = vec![];
    let mut start = 0;
    let mut previous = '\0';
    for (index, c) in text.char_indices() {
        let abbreviation = text
            .get(index.saturating_sub(4)..index)
            .is_some_and(|suffix| {
                suffix.eq_ignore_ascii_case("e.g.") || suffix.eq_ignore_ascii_case("i.e.")
            });
        if c == '\n'
            || (c.is_whitespace() && matches!(previous, '.' | '!' | '?' | '؟') && !abbreviation)
        {
            sentences.push(words(&text[start..index]));
            start = index + c.len_utf8();
        }
        previous = c;
    }
    sentences.push(words(&text[start..]));
    let rtl_count = sentences
        .iter()
        .filter(|d| **d == Some(Direction::Rtl))
        .count();
    let ltr_count = sentences
        .iter()
        .filter(|d| **d == Some(Direction::Ltr))
        .count();
    if rtl_count == ltr_count {
        words(text)
    } else {
        Some(if rtl_count > ltr_count {
            Direction::Rtl
        } else {
            Direction::Ltr
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockDirection {
    pub start: usize,
    pub end: usize,
    pub direction: Option<Direction>,
}
pub fn composer_directions(document: &Document) -> Vec<BlockDirection> {
    let mut groups: Vec<(usize, usize, bool, String)> = vec![];
    let mut previous_list = false;
    let mut previous_quote = false;
    for block in &document.blocks {
        let start = utf16_byte_offset(document.source(), block.start).unwrap_or_default();
        let end = utf16_byte_offset(document.source(), block.end).unwrap_or(start);
        let raw = &document.source()[start..end];
        let is_list = matches!(block.kind, BlockKind::ListItem { .. });
        let is_quote = matches!(block.kind, BlockKind::Quote { .. });
        let code = matches!(block.kind, BlockKind::Code { .. });
        let prefix = match &block.kind {
            BlockKind::Heading { prefix, .. }
            | BlockKind::Quote { prefix }
            | BlockKind::ListItem { prefix } => prefix.len(),
            _ => 0,
        };
        let prose = if code {
            String::new()
        } else {
            parse_inline_markdown(&raw[prefix..])
                .into_iter()
                .map(|span| {
                    if span.marks.contains(&Mark::Code) {
                        " ".into()
                    } else {
                        span.text
                    }
                })
                .collect::<String>()
        };
        if (is_list && previous_list) || (is_quote && previous_quote) {
            if let Some(group) = groups.last_mut() {
                group.1 = block.end;
                group.3.push('\n');
                group.3.push_str(&prose);
            }
        } else {
            groups.push((block.start, block.end, code, prose));
        }
        previous_list = is_list;
        previous_quote = is_quote;
    }
    let mut previous = None;
    groups
        .into_iter()
        .map(|(start, end, code, text)| {
            let direction = if code {
                Some(Direction::Ltr)
            } else {
                text_direction(&text).or(previous)
            };
            if !code {
                previous = direction;
            }
            BlockDirection {
                start,
                end,
                direction,
            }
        })
        .collect()
}
pub fn caret_line_direction(document: &Document, head: usize) -> Option<Direction> {
    composer_directions(document)
        .into_iter()
        .find(|block| head >= block.start && head <= block.end)
        .and_then(|b| b.direction)
}
pub fn composer_tree_directions(document: &serde_json::Value) -> Vec<BlockDirection> {
    use super::editor::{children, name, node_size};
    fn prose(node: &serde_json::Value, result: &mut String) {
        use super::editor::{children, name};
        if name(node) == "text" {
            if node["marks"]
                .as_array()
                .is_some_and(|marks| marks.iter().any(|mark| mark["type"] == "code"))
            {
                result.push(' ');
            } else {
                result.push_str(node["text"].as_str().unwrap_or_default());
            }
        } else {
            result.push(' ');
            if !name(node).starts_with("composer-") {
                for child in children(node) {
                    prose(child, result);
                }
            }
        }
    }
    let mut previous = None;
    let mut start = 0;
    children(document)
        .iter()
        .map(|block| {
            let end = start + node_size(block);
            let direction = if name(block) == "codeBlock" {
                Some(Direction::Ltr)
            } else {
                let mut text = String::new();
                prose(block, &mut text);
                let direction = text_direction(&text).or(previous);
                previous = direction;
                direction
            };
            let result = BlockDirection {
                start,
                end,
                direction,
            };
            start = end;
            result
        })
        .collect()
}
pub fn caret_tree_direction(document: &serde_json::Value, head: usize) -> Option<Direction> {
    let blocks = composer_tree_directions(document);
    blocks
        .iter()
        .find(|block| head >= block.start && head < block.end)
        .or_else(|| blocks.last().filter(|block| head == block.end))
        .and_then(|block| block.direction)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tree_directions_ignore_atoms_and_inline_code_and_follow_each_distinct_top_level_list() {
        use serde_json::json;
        let document = json!({"type":"doc","content":[
            {"type":"paragraph","content":[{"type":"text","text":"مرحبا"},{"type":"composer-mention","attrs":{"path":"many english words"}},{"type":"text","text":"useEffect in React","marks":[{"type":"code"}]}]},
            {"type":"codeBlock","content":[{"type":"text","text":"English"}]},
            {"type":"paragraph","content":[{"type":"text","text":"42"}]},
            {"type":"bulletList","content":[{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"English"}]}]}]}
        ]});
        let blocks = composer_tree_directions(&document);
        assert_eq!(
            blocks
                .iter()
                .map(|block| block.direction)
                .collect::<Vec<_>>(),
            vec![
                Some(Direction::Rtl),
                Some(Direction::Ltr),
                Some(Direction::Rtl),
                Some(Direction::Ltr)
            ]
        );
        assert_eq!(
            caret_tree_direction(&document, blocks[2].start + 1),
            Some(Direction::Rtl)
        );
    }
    #[test]
    fn typescript_direction_cases() {
        for (source, expected) in [
            (
                "`useEffect` و `useLayoutEffect` في `React`",
                vec![Direction::Rtl],
            ),
            (
                "- Vitest\n- اختبر الدالة الجديدة\n- وأضف تعليق",
                vec![Direction::Rtl],
            ),
            (
                "اكتب الدالة\n\n42\nfix it",
                vec![
                    Direction::Rtl,
                    Direction::Rtl,
                    Direction::Rtl,
                    Direction::Ltr,
                ],
            ),
            (
                "اكتب الدالة\n```\n// احسب مجموع القيم هنا\n```\nثم اختبرها",
                vec![Direction::Rtl, Direction::Ltr, Direction::Rtl],
            ),
        ] {
            assert_eq!(
                composer_directions(&Document::parse(source, false))
                    .iter()
                    .map(|b| b.direction)
                    .collect::<Vec<_>>(),
                expected.into_iter().map(Some).collect::<Vec<_>>(),
                "{source}"
            );
        }
    }
    #[test]
    fn code_does_not_change_inheritance() {
        assert_eq!(
            composer_directions(&Document::parse("عربي\n```\nfoo\n```\n42", false))
                .last()
                .and_then(|b| b.direction),
            Some(Direction::Rtl)
        );
        assert_eq!(text_direction("42"), None);
    }
}
