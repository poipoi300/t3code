//! Parser caching and clipboard text policy; DOM traversal and HTML sanitization stay in UI.
use crate::view_logic::{JsTrim, is_js_whitespace};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownPoint {
    pub line: usize,
    pub column: usize,
    pub offset: Option<usize>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownPosition {
    pub start: MarkdownPoint,
    pub end: MarkdownPoint,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownNode {
    pub kind: String,
    pub position: Option<MarkdownPosition>,
    pub children: Vec<MarkdownNode>,
    pub data: std::collections::BTreeMap<String, serde_json::Value>,
}
impl MarkdownNode {
    fn has_definitions(&self) -> bool {
        matches!(self.kind.as_str(), "definition" | "footnoteDefinition")
            || self.children.iter().any(Self::has_definitions)
    }
    fn shift_positions(&mut self, offset: usize, lines: usize) {
        if let Some(p) = &mut self.position {
            for point in [&mut p.start, &mut p.end] {
                if let Some(o) = &mut point.offset {
                    *o += offset;
                }
                point.line += lines;
            }
        }
        for child in &mut self.children {
            child.shift_positions(offset, lines);
        }
    }
}
#[derive(Debug, Clone)]
struct ParsedPrefix {
    source: String,
    offset: usize,
    line: usize,
    children: Vec<MarkdownNode>,
}
#[derive(Default)]
pub struct IncrementalMarkdownParser {
    cached: Option<ParsedPrefix>,
}
pub struct MarkdownRenderParser<'a> {
    parser: &'a mut IncrementalMarkdownParser,
    document_parsed: bool,
}
fn byte_offset(s: &str, offset: usize) -> usize {
    let mut units = 0;
    for (index, ch) in s.char_indices() {
        if units >= offset {
            return index;
        }
        units += ch.len_utf16();
    }
    s.len()
}
impl IncrementalMarkdownParser {
    pub fn begin_render(&mut self) -> MarkdownRenderParser<'_> {
        MarkdownRenderParser {
            parser: self,
            document_parsed: false,
        }
    }
    pub fn parse(
        &mut self,
        source: &str,
        mut parse: impl FnMut(&str) -> MarkdownNode,
    ) -> MarkdownNode {
        if source.contains(['\r', '\u{feff}']) {
            return parse(source);
        }
        let prefix = self
            .cached
            .as_ref()
            .filter(|p| source.starts_with(&p.source));
        let root = if let Some(prefix) = prefix {
            let mut root = parse(&source[byte_offset(source, prefix.offset)..]);
            if root.has_definitions() {
                return parse(source);
            }
            root.shift_positions(prefix.offset, prefix.line.saturating_sub(1));
            if let Some(p) = &mut root.position {
                p.start = MarkdownPoint {
                    line: 1,
                    column: 1,
                    offset: Some(0),
                };
            }
            root.children.splice(0..0, prefix.children.clone());
            root
        } else {
            let root = parse(source);
            if root.has_definitions() {
                return root;
            }
            root
        };
        for (index, node) in root.children.iter().enumerate().rev() {
            if node.kind != "code" {
                continue;
            }
            let Some(p) = &node.position else {
                continue;
            };
            let (Some(start), Some(end)) = (p.start.offset, p.end.offset) else {
                continue;
            };
            if prefix.is_some_and(|p| end < p.offset) {
                break;
            }
            let (start_byte, end_byte) = (byte_offset(source, start), byte_offset(source, end));
            let Some(value) = source.get(start_byte..end_byte) else {
                continue;
            };
            let Some((opening, _)) = value.split_once('\n') else {
                continue;
            };
            let indent = opening.bytes().take_while(|b| *b == b' ').count();
            if indent > 3 {
                continue;
            }
            let opening = &opening[indent..];
            let Some(marker) = opening.chars().next().filter(|c| matches!(c, '`' | '~')) else {
                continue;
            };
            let length = opening.chars().take_while(|c| *c == marker).count();
            if length < 3 {
                continue;
            }
            let last = value.rsplit('\n').next().unwrap_or_default();
            let indent = last.bytes().take_while(|b| *b == b' ').count();
            if indent > 3 {
                continue;
            }
            let closing = &last[indent..];
            let run = closing.chars().take_while(|c| *c == marker).count();
            if run < length || !closing[run..].chars().all(|c| matches!(c, ' ' | '\t')) {
                continue;
            }
            let tail = &source[end_byte..];
            let Some(tail) = tail.strip_prefix('\n') else {
                continue;
            };
            let spaces = tail
                .bytes()
                .take_while(|b| matches!(b, b' ' | b'\t'))
                .count();
            if !tail[spaces..].starts_with('\n') {
                continue;
            }
            let offset_byte = end_byte + spaces + 2;
            self.cached = Some(ParsedPrefix {
                source: source[..offset_byte].into(),
                offset: source[..offset_byte].encode_utf16().count(),
                line: p.end.line + 2,
                children: root.children[..=index].to_vec(),
            });
            break;
        }
        root
    }
}
impl MarkdownRenderParser<'_> {
    pub fn parse(
        &mut self,
        source: &str,
        mut parse: impl FnMut(&str) -> MarkdownNode,
    ) -> MarkdownNode {
        if self.document_parsed {
            return parse(source);
        }
        self.document_parsed = true;
        self.parser.parse(source, parse)
    }
}
fn longest_backtick_run(text: &str) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}
pub fn serialize_code_block_to_markdown(source: &str, language: Option<&str>) -> String {
    let code = source.strip_suffix('\n').unwrap_or(source);
    let fence = "`".repeat(3.max(longest_backtick_run(code) + 1));
    format!(
        "{fence}{}\n{code}\n{fence}\n\n",
        language.unwrap_or_default()
    )
}
pub fn wrap_inline_code(code: &str) -> String {
    let fence = "`".repeat((longest_backtick_run(code) + 1).max(1));
    let pad = if code.starts_with('`') || code.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{code}{pad}{fence}")
}
pub fn wrap_inline_marker(content: &str, marker: &str) -> String {
    let core = content.js_trim();
    if core.is_empty() {
        return content.into();
    }
    let start = content.len() - content.js_trim_start().len();
    let end = content.js_trim_end().len();
    format!(
        "{}{marker}{core}{marker}{}",
        &content[..start],
        &content[end..]
    )
}
pub fn clipboard_csv_cell(value: &str) -> String {
    let mut normalized = String::new();
    let mut whitespace = false;
    for ch in value.chars() {
        if is_js_whitespace(ch) {
            whitespace = true;
        } else {
            if whitespace && !normalized.is_empty() {
                normalized.push(' ');
            }
            normalized.push(ch);
            whitespace = false;
        }
    }
    if normalized.contains([',', '"', '\n']) {
        format!("\"{}\"", normalized.replace('"', "\"\""))
    } else {
        normalized
    }
}
pub fn tidy_markdown(markdown: &str) -> String {
    let parts: Vec<_> = markdown.split('\0').collect();
    let last = parts.len().saturating_sub(1);
    parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if i % 2 == 1 {
                return (*p).into();
            }
            let lines: Vec<_> = p
                .split('\n')
                .map(|l| l.trim_end_matches([' ', '\t']))
                .collect();
            let joined = lines.join("\n");
            let mut result = String::new();
            let mut newlines = 0;
            for c in joined.chars() {
                if c == '\n' {
                    newlines += 1;
                    if newlines > 2 {
                        continue;
                    }
                } else {
                    newlines = 0;
                }
                result.push(c);
            }
            if i == 0 {
                result = result.js_trim_start().into();
            }
            if i == last {
                result = result.js_trim_end().into();
            }
            result
        })
        .collect()
}
