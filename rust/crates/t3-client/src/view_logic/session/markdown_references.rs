use super::markdown::MarkdownNode;

/// Pass the flattened HTML representation and its parser tree, as used for rendering.
pub fn markdown_reference_definitions(flattened: &str, root: &MarkdownNode) -> Vec<String> {
    fn byte_offset(text: &str, units: usize) -> Option<usize> {
        let mut count = 0;
        for (index, ch) in text.char_indices() {
            if count == units {
                return Some(index);
            }
            count += ch.len_utf16();
        }
        (count == units).then_some(text.len())
    }
    fn visit(source: &str, node: &MarkdownNode, quotes: usize, definitions: &mut Vec<String>) {
        if node.kind == "definition"
            && let Some(position) = &node.position
            && let (Some(start), Some(end)) = (position.start.offset, position.end.offset)
            && let Some(authored) = byte_offset(source, start)
                .zip(byte_offset(source, end))
                .and_then(|(start, end)| source.get(start..end))
        {
            let mut result = String::new();
            let mut chars = authored.chars().peekable();
            while let Some(ch) = chars.next() {
                if matches!(ch, '\r' | '\n') {
                    if ch == '\r' && chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    for _ in 0..quotes {
                        let mut probe = chars.clone();
                        while probe.peek().is_some_and(|c| matches!(c, ' ' | '\t')) {
                            probe.next();
                        }
                        if probe.next() == Some('>') {
                            chars = probe;
                        } else {
                            break;
                        }
                    }
                    while chars.peek().is_some_and(|c| matches!(c, ' ' | '\t')) {
                        chars.next();
                    }
                    result.push(' ');
                } else {
                    result.push(ch);
                }
            }
            definitions.push(result);
        }
        let quotes = quotes + usize::from(node.kind == "blockquote");
        for child in &node.children {
            visit(source, child, quotes, definitions);
        }
    }
    if !flattened.contains("]:") {
        return vec![];
    }
    let mut definitions = Vec::new();
    visit(flattened, root, 0, &mut definitions);
    definitions
}

pub fn with_reference_definitions(
    fragment: &str,
    reply_definitions: &[String],
    fragment_definitions: impl FnOnce(&str) -> Vec<String>,
) -> String {
    if reply_definitions.is_empty() || !fragment.contains('[') {
        return fragment.into();
    }
    let mut definitions = Vec::new();
    for definition in fragment_definitions(fragment)
        .iter()
        .chain(reply_definitions)
    {
        if !definitions.contains(definition) {
            definitions.push(definition.clone());
        }
    }
    format!("{}\n\n{fragment}", definitions.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::super::markdown::{MarkdownPoint, MarkdownPosition};
    use super::*;
    fn node(
        kind: &str,
        source: &str,
        range: Option<(usize, usize)>,
        children: Vec<MarkdownNode>,
    ) -> MarkdownNode {
        MarkdownNode {
            kind: kind.into(),
            position: range.map(|(start, end)| MarkdownPosition {
                start: MarkdownPoint {
                    line: 1,
                    column: 1,
                    offset: Some(source[..start].encode_utf16().count()),
                },
                end: MarkdownPoint {
                    line: 1,
                    column: 1,
                    offset: Some(source[..end].encode_utf16().count()),
                },
            }),
            children,
            data: Default::default(),
        }
    }
    #[test]
    fn authored_definitions_preserve_windows_escapes_and_wrapped_quote_depth() {
        for (source, quotes, expected) in [
            (
                "[c]: C:\\work\\.t3\\settings.json",
                0,
                "[c]: C:\\work\\.t3\\settings.json",
            ),
            (
                "[a\\]b]: https://example.com/a",
                0,
                "[a\\]b]: https://example.com/a",
            ),
            (
                "[d]:\n> https://example.com\n> \"Title\"",
                1,
                "[d]: https://example.com \"Title\"",
            ),
            (
                "[part\n    > rest]: https://example.com/a",
                0,
                "[part > rest]: https://example.com/a",
            ),
            (
                "[d]:\r\n> > /repo/a\r> > \"Title\"",
                2,
                "[d]: /repo/a \"Title\"",
            ),
        ] {
            let mut root = node("definition", source, Some((0, source.len())), vec![]);
            for _ in 0..quotes {
                root = node("blockquote", source, None, vec![root]);
            }
            assert_eq!(markdown_reference_definitions(source, &root), [expected]);
        }
        let source = "😀\n[c]: /repo/a";
        let root = node(
            "root",
            source,
            None,
            vec![node("definition", source, Some((5, source.len())), vec![])],
        );
        assert_eq!(
            markdown_reference_definitions(source, &root),
            ["[c]: /repo/a"]
        );
    }
    #[test]
    fn fragments_prepend_own_definitions_first_without_reimplementing_label_matching() {
        let outside = vec![
            "[ x ]: https://outside.example".into(),
            "[SS]: /outside".into(),
        ];
        let inside = vec!["[X]: https://inside.example".into(), "[ß]: /inside".into()];
        let fragment = "See [x] and [SS].";
        assert_eq!(
            with_reference_definitions(fragment, &outside, |_| inside.clone()),
            "[X]: https://inside.example\n[ß]: /inside\n[ x ]: https://outside.example\n[SS]: /outside\n\nSee [x] and [SS]."
        );
        let defs = vec!["[d]: /repo/a".into()];
        assert_eq!(
            with_reference_definitions("[d]", &defs, |_| defs.clone()),
            "[d]: /repo/a\n\n[d]"
        );
        assert_eq!(
            with_reference_definitions("See [d].", &[], |_| panic!("unnecessary parse")),
            "See [d]."
        );
        assert_eq!(
            with_reference_definitions("Plain text.", &defs, |_| panic!("unnecessary parse")),
            "Plain text."
        );
    }
}
