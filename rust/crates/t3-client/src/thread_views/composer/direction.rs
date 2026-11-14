use super::super::{
    document::{BlockKind, Document, utf16_byte_offset},
    rich_text::{Mark, parse_inline_markdown},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Ltr,
    Rtl,
}
/// The shared `textDirection.ts` decision (`view_logic::session::text`), which the composer's
/// TypeScript imports too, in the composer's direction type.
pub fn text_direction(text: &str) -> Option<Direction> {
    use crate::view_logic::session::text::TextDirection;
    crate::view_logic::session::text::text_direction(text).map(|direction| match direction {
        TextDirection::Ltr => Direction::Ltr,
        TextDirection::Rtl => Direction::Rtl,
    })
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
    fn letters_are_unicode_letters_as_in_the_shared_typescript() {
        // `textDirection.ts` (Node 22): Ⅻ is alphabetic but not \p{L}, so it carries no
        // direction and the Hebrew word decides.
        assert_eq!(text_direction("Ⅻ שלום"), Some(Direction::Rtl));
        assert_eq!(text_direction("שלום Ⅻ"), Some(Direction::Rtl));
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
