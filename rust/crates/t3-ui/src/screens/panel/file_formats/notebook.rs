//! Inert notebook Markdown, ported from NotebookPreview.tsx's remark plugin.

use nsh_base::text_view::{
    Block, Inline, MarkdownOptions, MarkdownPlugin, ParseContext, Parser, mdast::Node,
};
use nsh_core::prelude::*;

use crate::{
    screens::thread::markdown::base::chat_markdown_classes,
    theme::{cn, tw},
};

pub(super) fn notebook_markdown(source: &str) -> AnyElement {
    let document = Parser::new()
        .options(MarkdownOptions {
            html: false,
            math: false,
            ..Default::default()
        })
        .plugin(NotebookMarkdown)
        .markdown(source);
    let mut document = document;
    for block in &mut document.blocks {
        unlink_block(block);
    }
    for footnote in &mut document.footnotes {
        for block in &mut footnote.children {
            unlink_block(block);
        }
    }
    let mut classes = chat_markdown_classes();
    classes.root = Some(tw(&cn([classes.root.as_deref(), Some("px-4 py-3")])));
    nsh_base::text_view()
        .document(Some(document))
        .classes(classes)
        .selectable(true)
        .into_element()
}

struct NotebookMarkdown;

impl MarkdownPlugin for NotebookMarkdown {
    fn name(&self) -> &str {
        "inertNotebookMarkdown"
    }

    fn block(&self, node: &Node, _: &mut ParseContext<'_>) -> Option<Block> {
        match node {
            Node::Html(_) => Some(Block::paragraph(Vec::new())),
            _ => None,
        }
    }

    fn inline(&self, node: &Node, _: &mut ParseContext<'_>) -> Option<Inline> {
        let text = match node {
            Node::Html(_) => String::new(),
            Node::Image(image) => image.alt.clone(),
            Node::ImageReference(image) => image.alt.clone(),
            Node::InlineCode(code) => code.value.clone(),
            _ => return None,
        };
        Some(Inline::Text { text })
    }
}

fn unlink_block(block: &mut Block) {
    match block {
        Block::Paragraph { content } | Block::Heading { content, .. } => unlink(content),
        Block::Blockquote { children, .. } => children.iter_mut().for_each(unlink_block),
        Block::List { items, .. } => {
            for item in items {
                item.children.iter_mut().for_each(unlink_block);
            }
        }
        Block::Table(table) => {
            for cell in table.head.iter_mut().chain(table.rows.iter_mut().flatten()) {
                unlink(&mut cell.content);
            }
        }
        Block::CodeBlock(code) if code.language.as_deref() == Some("mermaid") => {
            code.language = Some("text".into());
        }
        _ => {}
    }
}

fn unlink(inlines: &mut Vec<Inline>) {
    if !inlines
        .iter()
        .any(|inline| !matches!(inline, Inline::Text { .. }))
    {
        return;
    }
    let mut cleaned = Vec::with_capacity(inlines.len());
    for mut inline in std::mem::take(inlines) {
        match &mut inline {
            Inline::Link { content, .. } => {
                unlink(content);
                cleaned.append(content);
                continue;
            }
            Inline::Image(image) => {
                inline = Inline::Text {
                    text: std::mem::take(&mut image.alt),
                }
            }
            Inline::Strong { content }
            | Inline::Emphasis { content }
            | Inline::Strikethrough { content } => unlink(content),
            _ => {}
        }
        cleaned.push(inline);
    }
    *inlines = cleaned;
}
