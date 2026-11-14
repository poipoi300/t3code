//! Port of apps/web/src/markdown-incremental.ts and apps/web/src/components/chat/MessagesTimeline.tsx block identity.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ops::Range;
use std::rc::Rc;

use nsh_base::text_view::{
    Block as NshBlock, MarkdownOptions, MarkdownPlugin, ParseContext, Parser, mdast,
};
use t3_client::view_logic::session::markdown::{
    IncrementalMarkdownParser, MarkdownNode, MarkdownPoint, MarkdownPosition,
};

pub const CODE_CHUNK_LINES: usize = 40;

#[derive(Default)]
struct SourceIndex {
    unicode: Vec<(usize, usize)>,
    lines: Vec<usize>,
    crlf: Vec<(usize, usize)>,
}

impl SourceIndex {
    fn new(source: &str) -> Self {
        let mut index = Self {
            lines: vec![0],
            ..Default::default()
        };
        let mut utf16 = 0;
        let mut removed = 0;
        let mut chars = source.char_indices().peekable();
        while let Some((at, ch)) = chars.next() {
            utf16 += ch.len_utf16();
            let end = at + ch.len_utf8();
            if !ch.is_ascii() {
                index.unicode.push((end, end - utf16));
            }
            if ch == '\r' && chars.peek().is_some_and(|(_, c)| *c == '\n') {
                chars.next();
                utf16 += 1;
                removed += 1;
                index.crlf.push((end + 1 - removed, removed));
                index.lines.push(end + 1);
            } else if matches!(ch, '\n' | '\r') {
                index.lines.push(end);
            }
        }
        index
    }

    fn utf16(&self, byte: usize) -> usize {
        let at = self.unicode.partition_point(|(end, _)| *end <= byte);
        byte - at.checked_sub(1).map_or(0, |at| self.unicode[at].1)
    }

    fn byte(&self, utf16: usize) -> usize {
        let at = self
            .unicode
            .partition_point(|(end, delta)| end - delta <= utf16);
        utf16 + at.checked_sub(1).map_or(0, |at| self.unicode[at].1)
    }

    fn original_byte(&self, normalized: usize) -> usize {
        let at = self.crlf.partition_point(|(end, _)| *end <= normalized);
        normalized + at.checked_sub(1).map_or(0, |at| self.crlf[at].1)
    }

    fn point(&self, byte: usize) -> MarkdownPoint {
        let line = self.lines.partition_point(|start| *start <= byte);
        MarkdownPoint {
            line,
            column: self.utf16(byte) - self.utf16(self.lines[line - 1]) + 1,
            offset: Some(self.utf16(byte)),
        }
    }
}

/// Scoped to a message (or work entry), independent of virtual-list positions.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockId {
    pub message_id: String,
    pub index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRange {
    pub bytes: Range<usize>,
    pub utf16: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeChunk {
    pub language: Option<String>,
    pub meta: Option<String>,
    /// Exact parsed code, including the newline between adjacent chunks.
    pub text: String,
    /// One-based line in this code block, not in the message.
    pub start_line: usize,
    pub first: bool,
    pub last: bool,
    /// Range in the code node's value; appends skip completed chunks.
    pub value_bytes: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Header,
    Prose,
    /// The web keeps document-wide syntax and Insight callouts together.
    Document,
    Table,
    Code(CodeChunk),
    Output {
        text: String,
        start_line: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub id: BlockId,
    pub kind: BlockKind,
    /// Disjoint ranges cover the original source, including fences and separators.
    pub source: SourceRange,
}

/// Whole-message data remains available when no corresponding view is mounted.
#[derive(Clone, Debug)]
pub struct MessageBlocks {
    pub source: Rc<str>,
    pub blocks: Rc<Vec<Rc<Block>>>,
    pub keys: Rc<Vec<BlockId>>,
    /// Resolve links/footnotes against this context, not a row in isolation.
    pub definitions: Rc<str>,
    pub document_wide: bool,
}

impl MessageBlocks {
    pub fn source_of(&self, block: &Block) -> Option<&str> {
        self.source.get(block.source.bytes.clone())
    }
}

/// Keep one instance per message outside recycled cells; use update_output for plain tool output.
pub struct MessageFlattener {
    message_id: String,
    parser: IncrementalMarkdownParser,
    previous: Option<MessageBlocks>,
    stable: usize,
    output: bool,
    index: SourceIndex,
}

impl MessageFlattener {
    pub fn new(message_id: impl Into<String>) -> Self {
        Self {
            message_id: message_id.into(),
            parser: IncrementalMarkdownParser::default(),
            previous: None,
            stable: 0,
            output: false,
            index: SourceIndex::default(),
        }
    }

    pub fn update(&mut self, source: &str) -> MessageBlocks {
        self.flatten(source, false)
    }

    pub fn update_output(&mut self, source: &str) -> MessageBlocks {
        self.flatten(source, true)
    }

    fn flatten(&mut self, source: &str, output: bool) -> MessageBlocks {
        if self.output == output
            && let Some(previous) = &self.previous
            && previous.source.as_ref() == source
        {
            return previous.clone();
        }
        let append = self.output == output
            && self
                .previous
                .as_ref()
                .is_some_and(|p| source.starts_with(p.source.as_ref()));
        self.index = SourceIndex::new(source);
        let parts = (!output).then(|| split_markdown_blocks(source));
        let document_wide = parts.as_ref().is_some_and(Option::is_none);
        let opaque = !output && (source.contains("[^") || has_document_syntax(source));
        // A late definition can change earlier rendering without changing those blocks' source.
        let stable = if append && !document_wide {
            self.stable
        } else {
            1
        };
        let mut blocks = self
            .previous
            .as_ref()
            .filter(|_| append)
            .map_or_else(Vec::new, |p| {
                p.blocks.iter().take(stable).cloned().collect()
            });
        if blocks.is_empty() {
            blocks.push(Rc::new(Block {
                id: BlockId {
                    message_id: self.message_id.clone(),
                    index: 0,
                },
                kind: BlockKind::Header,
                source: SourceRange {
                    bytes: 0..0,
                    utf16: 0..0,
                },
            }));
        }
        let resume = blocks.last().map_or(0, |b| b.source.bytes.end);
        let mut definitions = String::new();
        if output {
            self.output_chunks(source, resume, &mut blocks);
        } else {
            let root = self.parser.parse(source, parse_markdown);
            collect_definitions(&root, source, &self.index, &mut definitions);
            let nodes: Vec<_> = root
                .children
                .iter()
                .filter(|n| matches!(n.kind.as_str(), "code" | "table"))
                .filter_map(|node| {
                    let position = node.position.as_ref()?;
                    Some((
                        self.index.byte(position.start.offset?)
                            ..self.index.byte(position.end.offset?),
                        node,
                    ))
                })
                .collect();
            let mut offset = 0;
            let mut node_at = 0;
            for part in parts.flatten().unwrap_or_else(|| vec![source]) {
                let end = offset + part.len();
                if end > resume {
                    while node_at < nodes.len() && nodes[node_at].0.end <= offset.max(resume) {
                        node_at += 1;
                    }
                    let first = node_at;
                    while node_at < nodes.len() && nodes[node_at].0.start < end {
                        node_at += 1;
                    }
                    if opaque || part.split('\n').any(|line| insight(line, true).is_some()) {
                        self.push(offset.max(resume)..end, BlockKind::Document, &mut blocks);
                    } else {
                        self.markdown_part(
                            source,
                            &nodes[first..node_at],
                            offset..end,
                            resume,
                            &mut blocks,
                        );
                    }
                }
                offset = end;
            }
        }
        self.stable = stable_prefix(&blocks, source, output, document_wide);
        let keys: Vec<_> = blocks.iter().map(|b| b.id.clone()).collect();
        let keys = self
            .previous
            .as_ref()
            .filter(|p| p.keys.as_ref() == &keys)
            .map_or_else(|| Rc::new(keys), |p| p.keys.clone());
        if let Some(previous) = &self.previous {
            for (block, old) in blocks.iter_mut().zip(previous.blocks.iter()) {
                if block.as_ref() == old.as_ref() && previous.definitions.as_ref() == definitions {
                    *block = old.clone();
                }
            }
        }
        let next = MessageBlocks {
            source: Rc::from(source),
            blocks: Rc::new(blocks),
            keys,
            definitions: Rc::from(definitions),
            document_wide,
        };
        self.output = output;
        self.previous = Some(next.clone());
        next
    }

    fn push(&self, range: Range<usize>, kind: BlockKind, blocks: &mut Vec<Rc<Block>>) {
        blocks.push(Rc::new(Block {
            id: BlockId {
                message_id: self.message_id.clone(),
                index: blocks.len(),
            },
            source: SourceRange {
                utf16: self.index.utf16(range.start)..self.index.utf16(range.end),
                bytes: range,
            },
            kind,
        }));
    }

    fn markdown_part(
        &self,
        source: &str,
        nodes: &[(Range<usize>, &MarkdownNode)],
        range: Range<usize>,
        resume: usize,
        blocks: &mut Vec<Rc<Block>>,
    ) {
        let mut cursor = range.start.max(resume);
        for (node_range, node) in nodes {
            let start = node_range.start;
            let end = node_range.end;
            if start < range.start
                || end > range.end
                || end <= resume
                || !matches!(node.kind.as_str(), "code" | "table")
            {
                continue;
            }
            if cursor < start {
                self.push(cursor..start, BlockKind::Prose, blocks);
                cursor = start;
            }
            if node.kind == "table" {
                self.push(cursor..end, BlockKind::Table, blocks);
            } else {
                self.code_chunks(source, node, start..end, cursor, blocks);
            }
            cursor = end;
        }
        if cursor < range.end {
            if source[cursor..range.end].trim().is_empty() && blocks.len() > 1 {
                let last = blocks.pop().expect("a content block exists");
                self.push(
                    last.source.bytes.start..range.end,
                    last.kind.clone(),
                    blocks,
                );
            } else {
                self.push(cursor..range.end, BlockKind::Prose, blocks);
            }
        }
    }

    fn code_chunks(
        &self,
        source: &str,
        node: &MarkdownNode,
        range: Range<usize>,
        resume: usize,
        blocks: &mut Vec<Rc<Block>>,
    ) {
        let value = node
            .data
            .get("value")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let language = node
            .data
            .get("lang")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let meta = node
            .data
            .get("meta")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let previous = blocks.last().and_then(|b| match &b.kind {
            BlockKind::Code(c) if b.source.bytes.end == resume && !c.last => Some(c),
            _ => None,
        });
        let mut value_at = previous.map_or(0, |c| c.value_bytes.end);
        let mut line = previous.map_or(1, |c| c.start_line + CODE_CHUNK_LINES);
        let mut source_at = resume;
        let mut raw_at = if previous.is_some() {
            resume
        } else {
            let raw = &source[range.clone()];
            if fence(raw.lines().next().unwrap_or_default()).is_some() {
                range.start
                    + raw.find(['\r', '\n']).map_or(raw.len(), |at| {
                        at + if raw[at..].starts_with("\r\n") { 2 } else { 1 }
                    })
            } else {
                range.start
            }
        };
        loop {
            let end = chunk_end(value, value_at);
            let last = end == value.len();
            let raw_end = if last {
                range.end
            } else {
                raw_chunk_end(&source[..range.end], raw_at)
            };
            self.push(
                source_at..raw_end,
                BlockKind::Code(CodeChunk {
                    language: language.clone(),
                    meta: meta.clone(),
                    text: value[value_at..end].into(),
                    start_line: line,
                    first: value_at == 0,
                    last,
                    value_bytes: value_at..end,
                }),
                blocks,
            );
            if last {
                break;
            }
            source_at = raw_end;
            raw_at = raw_end;
            value_at = end;
            line += CODE_CHUNK_LINES;
        }
    }

    fn output_chunks(&self, source: &str, resume: usize, blocks: &mut Vec<Rc<Block>>) {
        let mut at = resume;
        let mut line = blocks
            .last()
            .and_then(|b| match b.kind {
                BlockKind::Output { start_line, .. } => Some(start_line + CODE_CHUNK_LINES),
                _ => None,
            })
            .unwrap_or(1);
        while at < source.len() {
            let end = raw_chunk_end(source, at);
            self.push(
                at..end,
                BlockKind::Output {
                    text: source[at..end].into(),
                    start_line: line,
                },
                blocks,
            );
            at = end;
            line += CODE_CHUNK_LINES;
        }
    }
}

fn chunk_end(source: &str, start: usize) -> usize {
    source[start..]
        .match_indices('\n')
        .nth(CODE_CHUNK_LINES - 1)
        .map_or(source.len(), |(at, _)| start + at + 1)
}

fn raw_chunk_end(source: &str, start: usize) -> usize {
    let mut at = start;
    for _ in 0..CODE_CHUNK_LINES {
        let Some(next) = source[at..].find(['\r', '\n']) else {
            return source.len();
        };
        at += next;
        at += if source[at..].starts_with("\r\n") {
            2
        } else {
            1
        };
    }
    at
}

fn stable_prefix(blocks: &[Rc<Block>], source: &str, output: bool, document_wide: bool) -> usize {
    if document_wide {
        return 1;
    }
    let mut stable = blocks.len().saturating_sub(1).max(1);
    if !output {
        while stable > 1 {
            let block = &blocks[stable - 1];
            if let BlockKind::Code(code) = &block.kind
                && code.last
                && !source[block.source.bytes.clone()].ends_with("\n\n")
            {
                stable -= 1;
            } else {
                break;
            }
        }
    }
    stable
}

fn collect_definitions(node: &MarkdownNode, source: &str, index: &SourceIndex, out: &mut String) {
    if matches!(node.kind.as_str(), "definition" | "footnoteDefinition") {
        if let Some(position) = &node.position
            && let (Some(start), Some(end)) = (position.start.offset, position.end.offset)
        {
            out.push_str(&source[index.byte(start)..index.byte(end)]);
            out.push_str("\n\n");
        }
    } else {
        for child in &node.children {
            collect_definitions(child, source, index, out);
        }
    }
}

// NSH exposes mdast through a plugin hook. Stop conversion there: the client owns the cache,
// and flattening must not build TextViews, syntax spans or native views.
fn parse_markdown(source: &str) -> MarkdownNode {
    #[derive(Clone)]
    struct Capture(Rc<RefCell<Vec<MarkdownNode>>>, Rc<SourceIndex>);
    impl MarkdownPlugin for Capture {
        fn name(&self) -> &str {
            "t3-timeline-blocks"
        }
        fn block(&self, node: &mdast::Node, _cx: &mut ParseContext<'_>) -> Option<NshBlock> {
            self.0.borrow_mut().push(convert_node(node, &self.1));
            Some(NshBlock::paragraph(Vec::new()))
        }
    }
    let nodes = Rc::new(RefCell::new(Vec::new()));
    let index = Rc::new(SourceIndex::new(source));
    Parser::new()
        .options(MarkdownOptions {
            // NSH's math rewriter moves source offsets; math groups go whole to their renderer.
            math: false,
            ..Default::default()
        })
        .plugin(Capture(nodes.clone(), index.clone()))
        .markdown(source);
    MarkdownNode {
        kind: "root".into(),
        position: Some(MarkdownPosition {
            start: MarkdownPoint {
                line: 1,
                column: 1,
                offset: Some(0),
            },
            end: index.point(source.len()),
        }),
        children: nodes.take(),
        data: BTreeMap::new(),
    }
}

fn convert_node(node: &mdast::Node, index: &SourceIndex) -> MarkdownNode {
    let kind = match node {
        mdast::Node::Code(_) => "code",
        mdast::Node::Table(_) => "table",
        mdast::Node::Definition(_) => "definition",
        mdast::Node::FootnoteDefinition(_) => "footnoteDefinition",
        _ => "prose",
    };
    let point = |offset| index.point(index.original_byte(offset));
    let position = node.position().map(|p| MarkdownPosition {
        start: point(p.start.offset),
        end: point(p.end.offset),
    });
    let mut data = BTreeMap::new();
    if let mdast::Node::Code(code) = node {
        data.insert("value".into(), code.value.clone().into());
        if let Some(lang) = &code.lang {
            data.insert("lang".into(), lang.clone().into());
        }
        if let Some(meta) = &code.meta {
            data.insert("meta".into(), meta.clone().into());
        }
    }
    MarkdownNode {
        kind: kind.into(),
        position,
        data,
        children: node.children().map_or_else(Vec::new, |children| {
            children.iter().map(|n| convert_node(n, index)).collect()
        }),
    }
}

fn fence(line: &str) -> Option<&str> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let marker = trimmed.as_bytes().first()?;
    if !matches!(marker, b'`' | b'~') {
        return None;
    }
    let len = trimmed.bytes().take_while(|b| b == marker).count();
    (len >= 3).then_some(&trimmed[..len])
}

fn continuation(line: &str) -> bool {
    if line.starts_with([' ', '\t']) {
        return true;
    }
    let bytes = line.as_bytes();
    let marker = if bytes
        .first()
        .is_some_and(|b| matches!(b, b'-' | b'*' | b'+'))
    {
        1
    } else {
        let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
        if !(1..=9).contains(&digits)
            || !bytes.get(digits).is_some_and(|b| matches!(b, b'.' | b')'))
        {
            return false;
        }
        digits + 1
    };
    bytes.get(marker).is_none_or(|b| matches!(b, b' ' | b'\t'))
}

fn document_syntax(line: &str) -> bool {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return false;
    }
    trimmed.starts_with(":::")
        || trimmed.starts_with("$$")
        || trimmed
            .strip_prefix('<')
            .and_then(|s| s.chars().next())
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '!' | '?' | '/'))
        || trimmed
            .strip_prefix('[')
            .and_then(|s| s.find("]:").map(|at| at > 0 && !s[..at].contains(']')))
            .unwrap_or(false)
}

fn has_document_syntax(text: &str) -> bool {
    text.split('\n').any(|line| {
        document_syntax(line)
            || line.char_indices().any(|(at, ch)| {
                matches!(ch, '\r' | '\u{2028}' | '\u{2029}')
                    && document_syntax(&line[at + ch.len_utf8()..])
            })
    })
}

fn insight(line: &str, opening: bool) -> Option<&str> {
    let trimmed = line.trim_start_matches(' ');
    let indent = line.len() - trimmed.len();
    if indent > 3 {
        return None;
    }
    let trimmed = trimmed.trim_end_matches([' ', '\t']);
    let body = if trimmed.starts_with('`') && trimmed.ends_with('`') && trimmed.len() > 1 {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };
    let rule = if opening {
        let rest = body.strip_prefix("★ Insight")?;
        if !rest.starts_with([' ', '\t']) {
            return None;
        }
        rest.trim_start_matches([' ', '\t'])
    } else {
        body
    };
    (rule.chars().count() >= 3 && rule.chars().all(|c| c == '─')).then_some(&line[..indent])
}

/// Port of apps/web/src/components/chat/markdownStreamBlocks.ts splitMarkdownBlocks.
pub fn split_markdown_blocks(text: &str) -> Option<Vec<&str>> {
    if text.contains('\r') || text.contains("[^") || has_document_syntax(text) {
        return None;
    }
    let mut blocks = Vec::new();
    let mut block_start = 0;
    let mut line_start = 0;
    let mut open_fence: Option<&str> = None;
    let mut fence_indent = 0;
    let mut after_blank = false;
    let mut insight_indent = None;
    for line in text.split('\n') {
        let opener = fence(line);
        if let Some(current) = open_fence {
            if line.chars().take(fence_indent).any(|c| !js_whitespace(c)) {
                break;
            }
            if let Some(opener) = opener
                && opener.as_bytes()[0] == current.as_bytes()[0]
                && opener.len() >= current.len()
                && line.trim_matches(js_whitespace) == opener
            {
                open_fence = None;
            }
        } else if insight_indent.is_some() && insight(line, false) == insight_indent {
            insight_indent = None;
            after_blank = false;
        } else if line.trim_matches(js_whitespace).is_empty() {
            after_blank = true;
        } else {
            if after_blank
                && insight_indent.is_none()
                && line_start > block_start
                && !continuation(line)
            {
                blocks.push(&text[block_start..line_start]);
                block_start = line_start;
            }
            after_blank = false;
            if insight_indent.is_none() {
                insight_indent = insight(line, true);
            }
            open_fence = opener;
            if let Some(opener) = opener {
                fence_indent = line.find(opener).unwrap_or(0);
            }
        }
        line_start += line.len() + 1;
    }
    blocks.push(&text[block_start..]);
    Some(blocks)
}

fn js_whitespace(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFIX: &str = "# Before\n\n```ts\nconst values = [1, 2];\n```\n\n";
    const STREAM_CASES: &[&str] = &[
        "a\n===\n\nb\n---\n",
        "- first\n\n  continued\n\n- next\n",
        "> quoted\n>\n> ```js\n> abc\n> ```\n\nend",
        "<div>\nhello\n\n</div>\n\nend",
        "[ref]\n\n[ref]: /later",
        "a[^x]\n\n[^x]: note",
        "a | b\n--|--\na | b\n",
        "```\na\n```\n\nnext\n\n~~~\nb\n~~~\n\nmore",
        "\n\n\tcode\n\nmore",
        "text <https://example.com> *bold*",
        "> [!NOTE]\n> alert\n\n- [ ] task",
        "\u{feff}text after a byte-order mark",
    ];

    fn assert_document(message: &MessageBlocks) {
        let mut byte = 0;
        let mut utf16 = 0;
        for (index, block) in message.blocks.iter().enumerate() {
            assert_eq!(block.id.index, index);
            assert_eq!(block.source.bytes.start, byte);
            assert_eq!(block.source.utf16.start, utf16);
            let text = message.source_of(block).expect("valid UTF-8 boundaries");
            byte += text.len();
            utf16 += text.encode_utf16().count();
            assert_eq!(block.source.bytes.end, byte);
            assert_eq!(block.source.utf16.end, utf16);
            if let BlockKind::Code(code) = &block.kind {
                assert!(code.text.lines().count() <= CODE_CHUNK_LINES);
                let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
                assert!(
                    normalized.lines().count() <= CODE_CHUNK_LINES + 3,
                    "a code chunk's raw source must also be bounded: {:?}",
                    block.source
                );
            }
        }
        assert_eq!(byte, message.source.len());
        assert_eq!(utf16, message.source.encode_utf16().count());
    }

    fn assert_stream(source: &str) {
        let mut parser = IncrementalMarkdownParser::default();
        let mut flattener = MessageFlattener::new("message");
        for end in source
            .char_indices()
            .map(|(at, _)| at)
            .chain([source.len()])
        {
            let text = &source[..end];
            assert_eq!(
                parser.parse(text, parse_markdown),
                parse_markdown(text),
                "parse at {end}: {text:?}"
            );
            let incremental = flattener.update(text);
            let full = MessageFlattener::new("message").update(text);
            assert_eq!(
                incremental.blocks, full.blocks,
                "flatten at {end}: {text:?}"
            );
            assert_eq!(incremental.definitions, full.definitions);
            assert_document(&incremental);
        }
    }

    #[test]
    fn web_incremental_streaming_cases_preserve_tree_positions_and_blocks() {
        for tail in STREAM_CASES {
            assert_stream(&format!("{PREFIX}{tail}"));
        }
    }

    #[test]
    fn web_partial_cr_and_crlf_line_endings_preserve_original_offsets() {
        for newline in ["\r\n", "\r"] {
            assert_stream(&format!("{PREFIX}next\n\n```\nlast\n```\n\nend").replace('\n', newline));
        }
    }

    #[test]
    fn web_unsafe_fences_are_never_frozen() {
        for start in [
            "```\nopen\n\n",
            "````\n```\n\n",
            "> ```\n> code\n> ```\n\n",
            "- ```\n  code\n  ```\n\n",
            "    ```\n    code\n    ```\n\n",
            "<script>\n```\ncode\n```\n\n",
        ] {
            for tail in ["", "text", "\n```\n", "\n```\n\nnext"] {
                assert_stream(&format!("{start}{tail}"));
            }
        }
    }

    #[test]
    fn web_definitions_arriving_late_invalidate_earlier_context() {
        let before = format!("[later] and footnote[^note]\n\n{PREFIX}");
        let mut flattener = MessageFlattener::new("message");
        let initial = flattener.update(&format!("{before}text"));
        for tail in [
            "text",
            "[later]: /target",
            "[later]: /target\n\n[^note]: a note",
        ] {
            let source = format!("{before}{tail}");
            let next = flattener.update(&source);
            assert_eq!(
                next.blocks,
                MessageFlattener::new("message").update(&source).blocks
            );
            if tail.contains("/target") {
                assert!(next.definitions.contains("[later]: /target"));
                assert!(!Rc::ptr_eq(&initial.blocks[1], &next.blocks[1]));
            }
        }
    }

    #[test]
    fn web_edits_replacements_and_repeated_renders_do_not_leak_transformed_nodes() {
        let mut flattener = MessageFlattener::new("message");
        for source in [
            format!("{PREFIX}- first\n - second"),
            format!("{PREFIX}> [!NOTE]\n> transformed alert"),
            format!("{PREFIX}plain text"),
            "replacement without fences".into(),
            format!("{}edited prefix", PREFIX.replace("Before", "Edited")),
            format!("{PREFIX}plain text"),
            format!("{PREFIX}plain text"),
        ] {
            let next = flattener.update(&source);
            assert_eq!(
                next.blocks,
                MessageFlattener::new("message").update(&source).blocks
            );
            assert_document(&next);
        }
        let before = flattener.update("same");
        let after = flattener.update("same");
        assert!(Rc::ptr_eq(&before.blocks, &after.blocks));
        let mut parser = IncrementalMarkdownParser::default();
        let mut root = parser.parse(PREFIX, parse_markdown);
        root.children.clear();
        assert_eq!(
            parser.parse(&format!("{PREFIX}more"), parse_markdown),
            parse_markdown(&format!("{PREFIX}more"))
        );
    }

    #[test]
    fn web_recovery_parses_do_not_replace_the_document_cache() {
        let mut parser = IncrementalMarkdownParser::default();
        parser.parse(PREFIX, parse_markdown);
        let mut render = parser.begin_render();
        render.parse(&format!("{PREFIX}tail"), parse_markdown);
        render.parse(
            "t3-markdown-inline-prefix:\n```ts\nrecovery\n```\n\n",
            parse_markdown,
        );
        let mut calls = Vec::new();
        parser.parse(&format!("{PREFIX}tail more"), |source| {
            calls.push(source.to_owned());
            parse_markdown(source)
        });
        assert_eq!(calls, ["tail more"]);
    }

    #[test]
    fn web_splitter_preserves_closed_blocks_lists_and_insights() {
        assert_eq!(
            split_markdown_blocks("a\n\n```\nb\n\nc\n```\n\n- d\n\n- e\n\nf"),
            Some(vec!["a\n\n", "```\nb\n\nc\n```\n\n- d\n\n- e\n\n", "f"])
        );
        for source in [
            "# Title\n\nFirst paragraph with `code` and **bold**.\n\nSecond [link](https://example.com).",
            "- one\n- two\n\n- loose three\n\n  continued\n\nAfter the list.",
            "1. first\n\n2. second\n\nText\n\n3) restarted",
            "```ts\nconst a = 1;\n\n\nconst b = 2;\n```\n\nafter\n\n~~~\nx\n\n~~~~\n\ny\n~~~\n\nend",
            "> quote\n\n> another quote\n\n> [!NOTE]\n> alert",
            "| a | b |\n| - | - |\n| 1 | 2 |\n\nAfter the table.",
            "para\n\n    indented code\n\nnext",
            "Setext\n===\n\nnext\n---\n\n- [ ] task\n- [x] done",
            "- a\n\n  ```\n  x\n\n```\n\nb",
            "- a\n\n  ```\n  x\n\ny\n\n  ```\n\nb",
            "intro\n\n★ Insight ─────\nfirst\n\n```\n─────\n```\n\n- point\n─────\n\nafter\n\n`★ Insight ─────`\nsecond\n`─────`\n\nend",
        ] {
            for end in source
                .char_indices()
                .map(|(at, _)| at)
                .chain([source.len()])
            {
                let source = &source[..end];
                let blocks = split_markdown_blocks(source).expect("safe syntax");
                assert_eq!(blocks.concat(), source);
                // Compare actual parsed documents, not just a lossless string join.
                let joined: Vec<_> = blocks
                    .iter()
                    .flat_map(|s| Parser::new().markdown(s).blocks)
                    .collect();
                assert_eq!(joined, Parser::new().markdown(source).blocks, "{source:?}");
            }
        }
    }

    #[test]
    fn web_document_wide_syntax_keeps_the_full_parse_path() {
        for source in [
            "[ref]\n\n[ref]: /later",
            "a[^x]\n\n[^x]: note",
            "<details>\n\nbody\n\n</details>",
            ":::note\n\ncontent\n\n:::",
            "$$\na\n\nb\n$$",
            "a\r\n\r\nb",
            "text\u{2028}$$",
            "text\u{2029}<details>body</details>",
            "[label\u{2028}continued]: /target",
        ] {
            assert!(split_markdown_blocks(source).is_none());
        }
    }

    #[test]
    fn document_syntax_and_insight_callouts_retain_their_rendering_context() {
        for source in [
            "$$\n```\nmath, not a code row\n```\n$$",
            ":::note\n\n```\ncode in a directive\n```\n\n:::",
            "<details>\n\n```\ncode in HTML\n```\n\n</details>",
            "[ref]\n\n```\ncode\n```\n\n[ref]: /target",
            "★ Insight ───\n\n```\ncode in a callout\n```\n\n───",
        ] {
            let message = MessageFlattener::new("message").update(source);
            assert_eq!(message.blocks.len(), 2);
            assert_eq!(message.blocks[1].kind, BlockKind::Document);
            assert_eq!(message.source_of(&message.blocks[1]), Some(source));
            assert_stream(source);
        }
    }

    #[test]
    fn insight_backticks_must_touch_the_rule_as_in_the_web_pattern() {
        assert_eq!(
            split_markdown_blocks("`★ Insight ─── `\ntext\n\nafter"),
            Some(vec!["`★ Insight ─── `\ntext\n\n", "after"])
        );
        assert_eq!(
            split_markdown_blocks("`★ Insight ───`\ntext\n`─── `\n\nafter"),
            Some(vec!["`★ Insight ───`\ntext\n`─── `\n\nafter"])
        );
    }

    #[test]
    fn five_hundred_code_lines_form_thirteen_chunks_with_exact_values_and_metadata() {
        let value = (0..500)
            .map(|n| format!("let v{n} = {n};"))
            .collect::<Vec<_>>()
            .join("\n");
        let source = format!("Before\n\n```rust title=example\n{value}\n```\n\nAfter");
        let blocks = MessageFlattener::new("message").update(&source);
        assert_document(&blocks);
        let chunks: Vec<_> = blocks
            .blocks
            .iter()
            .filter_map(|b| {
                if let BlockKind::Code(c) = &b.kind {
                    Some(c)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(chunks.len(), 13);
        assert_eq!(
            chunks.iter().map(|c| c.text.as_str()).collect::<String>(),
            value
        );
        for (index, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.start_line, index * 40 + 1);
            assert_eq!(chunk.first, index == 0);
            assert_eq!(chunk.last, index == 12);
            assert_eq!(chunk.language.as_deref(), Some("rust"));
            assert_eq!(chunk.meta.as_deref(), Some("title=example"));
        }
    }

    #[test]
    fn streaming_reuses_completed_code_chunks_and_only_reflattens_the_tail() {
        let prefix = format!("Intro\n\n```rs\n{}", "x\n".repeat(100));
        let mut flattener = MessageFlattener::new("message");
        let before = flattener.update(&prefix);
        let after = flattener.update(&format!("{prefix}tail"));
        assert_document(&after);
        assert_eq!(
            after.blocks,
            MessageFlattener::new("message")
                .update(&format!("{prefix}tail"))
                .blocks
        );
        assert!(Rc::ptr_eq(&before.blocks[0], &after.blocks[0]));
        assert!(Rc::ptr_eq(&before.blocks[1], &after.blocks[1]));
        assert!(Rc::ptr_eq(&before.blocks[2], &after.blocks[2]));
        assert!(Rc::ptr_eq(&before.blocks[3], &after.blocks[3]));
        assert!(!Rc::ptr_eq(
            before.blocks.last().expect("tail"),
            after.blocks.last().expect("tail")
        ));
        assert!(Rc::ptr_eq(&before.keys, &after.keys));
        assert!(flattener.stable >= 4);
        assert_stream(&format!("```\n{}tail\n```\n\nend", "x\n".repeat(82)));
    }

    #[test]
    fn unicode_and_indented_code_preserve_utf16_ranges_and_chunk_values() {
        for newline in ["\n", "\r", "\r\n"] {
            let source = format!(
                "😀 café\n\n  ~~~~python extra\n{}  ~~~~\n\nfin",
                "  😀\n".repeat(85)
            )
            .replace('\n', newline);
            let blocks = MessageFlattener::new("unicode").update(&source);
            assert_document(&blocks);
            let code: String = blocks
                .blocks
                .iter()
                .filter_map(|b| {
                    if let BlockKind::Code(c) = &b.kind {
                        Some(c.text.as_str())
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(code, vec!["😀"; 85].join("\n"));
        }
    }

    #[test]
    fn plain_output_is_lossless_bounded_and_has_no_markdown_semantics() {
        let source = format!("```\n{}", "[ref]: 😀\r\n".repeat(100));
        let mut flattener = MessageFlattener::new("work:tool");
        let first = flattener.update_output(&source);
        assert_document(&first);
        let text: String = first
            .blocks
            .iter()
            .filter_map(|b| match &b.kind {
                BlockKind::Output { text, .. } => {
                    assert!(text.lines().count() <= 40);
                    Some(text.as_str())
                }
                BlockKind::Header => None,
                _ => panic!("plain output only"),
            })
            .collect();
        assert_eq!(text, source);
        let next = flattener.update_output(&format!("{source}tail"));
        assert!(Rc::ptr_eq(&first.blocks[1], &next.blocks[1]));
        assert_eq!(
            next.blocks,
            MessageFlattener::new("work:tool")
                .update_output(&format!("{source}tail"))
                .blocks
        );
        assert!(matches!(
            flattener.update("```rs\nx\n```").blocks[1].kind,
            BlockKind::Code(_)
        ));
    }
}
