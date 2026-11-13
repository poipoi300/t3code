//! Structured code editing does not reparse a typed closing fence until Enter handles it.
use super::document::{
    Edit, Fence, Selection, is_closing_fence, parse_opening_fence, utf16_byte_offset, utf16_len,
};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBlock {
    pub opening: Fence,
    pub text: String,
    pub close: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeResult {
    Edited {
        block: CodeBlock,
        selection: Selection,
    },
    Exited {
        block: CodeBlock,
        paragraphs: Vec<String>,
    },
}
impl CodeBlock {
    pub fn from_opening_paragraph(
        text: &str,
        selection: Selection,
        top_level: bool,
    ) -> Option<Self> {
        if !top_level || !selection.collapsed() || selection.head != utf16_len(text) {
            return None;
        }
        let opening = parse_opening_fence(text)?;
        let close = format!("\n{}", opening.fence);
        Some(Self {
            opening,
            text: String::new(),
            close,
        })
    }
    pub fn source(&self, followed: bool) -> String {
        let mut fence = if self.opening.fence.is_empty() {
            "```".into()
        } else {
            self.opening.fence.clone()
        };
        let mut close = self.close.clone();
        let longest = self
            .text
            .split('\n')
            .filter(|line| is_closing_fence(line, &fence))
            .map(|line| {
                line.bytes()
                    .take_while(|c| matches!(c, b'`' | b'~'))
                    .count()
            })
            .max()
            .unwrap_or(0);
        if longest > 0
            && let Some(ch) = fence.chars().next()
        {
            fence = ch.to_string().repeat(longest + 1);
            if !close.is_empty() {
                close = format!("\n{fence}");
            }
        }
        if close.is_empty() && followed {
            close = format!("\n{fence}");
        }
        format!(
            "{fence}{}{}{}{close}",
            self.opening.language,
            if self.text.is_empty() { "" } else { "\n" },
            self.text
        )
    }
    fn replace(&self, selection: Selection, text: &str) -> Option<CodeResult> {
        let edit = Edit {
            start: selection.from(),
            end: selection.to(),
            replacement: text.into(),
            cursor_after: selection.from() + utf16_len(text),
        };
        let mut block = self.clone();
        block.text = edit.apply(&self.text)?;
        Some(CodeResult::Edited {
            block,
            selection: Selection::caret(edit.cursor_after),
        })
    }
    pub fn indented_newline(&self, selection: Selection) -> Option<CodeResult> {
        let from = utf16_byte_offset(&self.text, selection.from())?;
        utf16_byte_offset(&self.text, selection.to())?;
        let line = self.text[..from].rsplit('\n').next()?;
        let indent: String = line
            .chars()
            .take_while(|c| matches!(c, ' ' | '\t'))
            .collect();
        if indent.is_empty() {
            return None;
        }
        self.replace(selection, &format!("\n{indent}"))
    }
    pub fn exit_closing_fence(&self, selection: Selection) -> Option<CodeResult> {
        if !selection.collapsed() {
            return None;
        }
        let at = utf16_byte_offset(&self.text, selection.head)?;
        let start = self.text[..at].rfind('\n').map_or(0, |i| i + 1);
        let end = self.text[at..]
            .find('\n')
            .map_or(self.text.len(), |i| at + i);
        let line = &self.text[start..end];
        if at != end || !is_closing_fence(line, &self.opening.fence) {
            return None;
        }
        let mut block = self.clone();
        block.text = self.text[..start.saturating_sub(1)].into();
        block.close = format!("\n{line}");
        let mut paragraphs = vec![String::new()];
        if end < self.text.len() {
            paragraphs.extend(self.text[end + 1..].split('\n').map(str::to_string));
        }
        Some(CodeResult::Exited { block, paragraphs })
    }
    pub fn exit_trailing_blanks(&self, selection: Selection) -> Option<CodeResult> {
        if !selection.collapsed() || selection.head != utf16_len(&self.text) {
            return None;
        }
        let last = self.text.trim_end_matches([' ', '\t']).strip_suffix('\n')?;
        let prior = last.trim_end_matches([' ', '\t']).strip_suffix('\n')?;
        let mut block = self.clone();
        block.text = prior.into();
        if block.close.is_empty() {
            block.close = format!("\n{}", block.opening.fence);
        }
        Some(CodeResult::Exited {
            block,
            paragraphs: vec![String::new()],
        })
    }
    pub fn enter(&self, selection: Selection) -> Option<CodeResult> {
        self.exit_closing_fence(selection)
            .or_else(|| self.exit_trailing_blanks(selection))
            .or_else(|| self.indented_newline(selection))
            .or_else(|| self.replace(selection, "\n"))
    }
    pub fn indent(&self, selection: Selection, out: bool) -> Option<CodeResult> {
        if selection.collapsed() && !out {
            return self.replace(selection, "  ");
        }
        let from = utf16_byte_offset(&self.text, selection.from())?;
        let to = utf16_byte_offset(&self.text, selection.to())?;
        let start = self.text[..from].rfind('\n').map_or(0, |i| i + 1);
        let search = from.max(to.saturating_sub(1));
        let end = self.text.as_bytes()[search..]
            .iter()
            .position(|b| *b == b'\n')
            .map_or(self.text.len(), |i| search + i);
        let lines: Vec<_> = self.text[start..end].split('\n').collect();
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
        if original == replacement {
            return Some(CodeResult::Edited {
                block: self.clone(),
                selection,
            });
        }
        let first_delta = utf16_len(&next[0]) as isize - utf16_len(lines[0]) as isize;
        let total_delta = utf16_len(&replacement) as isize - utf16_len(&original) as isize;
        let next_from = selection
            .from()
            .saturating_add_signed(first_delta)
            .max(utf16_len(&self.text[..start]));
        let next_to = selection
            .to()
            .saturating_add_signed(total_delta)
            .max(next_from);
        let edit = Edit {
            start: utf16_len(&self.text[..start]),
            end: utf16_len(&self.text[..end]),
            replacement,
            cursor_after: next_to,
        };
        let mut block = self.clone();
        block.text = edit.apply(&self.text)?;
        let selection = if selection.anchor <= selection.head {
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
        Some(CodeResult::Edited { block, selection })
    }
    pub fn unwrap_at_start(&self, selection: Selection) -> Option<Vec<String>> {
        (selection == Selection::caret(0))
            .then(|| self.text.split('\n').map(str::to_string).collect())
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn block(text: &str) -> CodeBlock {
        CodeBlock {
            opening: Fence {
                fence: "```".into(),
                language: String::new(),
            },
            text: text.into(),
            close: "\n```".into(),
        }
    }
    fn edited(result: CodeResult) -> (CodeBlock, Selection) {
        match result {
            CodeResult::Edited { block, selection } => (block, selection),
            _ => panic!("expected edit"),
        }
    }
    #[test]
    fn indented_newline() {
        for (text, at, expected) in [
            (
                "function f() {\n  body()",
                23,
                "function f() {\n  body()\n  ",
            ),
            ("    deep()\nflush()", 10, "    deep()\n    \nflush()"),
        ] {
            let (next, _) = edited(block(text).indented_newline(Selection::caret(at)).unwrap());
            assert_eq!(next.text, expected);
        }
    }
    #[test]
    fn two_blank_lines_exit_and_close() {
        let mut b = block("flush()");
        b.close.clear();
        let mut caret = Selection::caret(7);
        for _ in 0..2 {
            (b, caret) = edited(b.enter(caret).unwrap());
        }
        assert_eq!(b.text, "flush()\n\n");
        let CodeResult::Exited { block, paragraphs } = b.enter(caret).unwrap() else {
            panic!("expected exit")
        };
        assert_eq!(block.text, "flush()");
        assert_eq!(block.close, "\n```");
        assert_eq!(paragraphs, vec![""]);
    }
    #[test]
    fn typed_closing_fence_moves_following_lines_out() {
        let mut b = block("const a = 1\n```");
        b.opening.language = "ts".into();
        let CodeResult::Exited { block: closed, .. } =
            b.exit_closing_fence(Selection::caret(15)).unwrap()
        else {
            panic!("exit")
        };
        assert_eq!(
            format!("{}\nexplain this", closed.source(true)),
            "```ts\nconst a = 1\n```\nexplain this"
        );
        let CodeResult::Exited {
            block: closed,
            paragraphs,
        } = block("a\n```\nb")
            .exit_closing_fence(Selection::caret(5))
            .unwrap()
        else {
            panic!("exit")
        };
        assert_eq!(
            format!("{}\n{}", closed.source(true), paragraphs.join("\n")),
            "```\na\n```\n\nb"
        );
        assert!(
            block("a\n``")
                .exit_closing_fence(Selection::caret(4))
                .is_none()
        );
        assert!(
            block("a\n```")
                .exit_closing_fence(Selection::caret(4))
                .is_none()
        );
    }
    #[test]
    fn indent_selection_and_noop_outdent() {
        for (text, selection, out, expected) in [
            (
                "one\ntwo\nthree",
                Selection { anchor: 1, head: 9 },
                false,
                "  one\n  two\n  three",
            ),
            (
                "one\ntwo",
                Selection { anchor: 0, head: 4 },
                false,
                "  one\ntwo",
            ),
            (
                "  one\n  two",
                Selection { anchor: 2, head: 9 },
                true,
                "one\ntwo",
            ),
            (
                "one\ntwo",
                Selection { anchor: 0, head: 7 },
                true,
                "one\ntwo",
            ),
            ("😀", Selection { anchor: 0, head: 2 }, false, "  😀"),
        ] {
            let (next, _) = edited(block(text).indent(selection, out).unwrap());
            assert_eq!(next.text, expected);
        }
    }
    #[test]
    fn repeated_indent_preserves_selection() {
        for (text, expected, anchor) in [
            ("one\ntwo", "    one\n    two", 4),
            ("\none\ntwo", "\n    one\n    two", 0),
        ] {
            let mut b = block(text);
            let mut s = Selection {
                anchor: 0,
                head: utf16_len(text),
            };
            for _ in 0..2 {
                (b, s) = edited(b.indent(s, false).unwrap());
            }
            assert_eq!(b.text, expected);
            assert_eq!(s.anchor, anchor);
        }
    }
    #[test]
    fn paragraph_conversion_and_unwrap() {
        for text in [
            "```",
            "```ts",
            "~~~py",
            "````",
            "```ts title=example",
            "``` js",
            "```ts @a.md",
        ] {
            let b =
                CodeBlock::from_opening_paragraph(text, Selection::caret(utf16_len(text)), true)
                    .unwrap();
            assert!(b.text.is_empty());
            assert_eq!(b.close, format!("\n{}", b.opening.fence));
            assert_eq!(
                b.unwrap_at_start(Selection::caret(0)),
                Some(vec!["".into()])
            );
            assert!(
                CodeBlock::from_opening_paragraph(text, Selection::caret(utf16_len(text)), false)
                    .is_none()
            );
        }
        for text in ["``", "text ```", "```a`b"] {
            assert!(
                CodeBlock::from_opening_paragraph(text, Selection::caret(utf16_len(text)), true)
                    .is_none()
            );
        }
    }
    #[test]
    fn serialization_uses_longer_fence_and_closes_before_prose() {
        let mut b = block("a\n````\nb");
        assert_eq!(b.source(false), "`````\na\n````\nb\n`````");
        b.text = "a".into();
        b.close.clear();
        assert_eq!(b.source(false), "```\na");
        assert_eq!(b.source(true), "```\na\n```");
    }
}
