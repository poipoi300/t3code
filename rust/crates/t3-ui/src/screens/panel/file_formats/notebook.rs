//! Inert Markdown and bounded saved outputs from NotebookPreview.tsx.

use std::cell::Cell;

use nsh_base::text_view::{
    Block, Inline, MarkdownOptions, MarkdownPlugin, ParseContext, Parser, mdast::Node,
};
use nsh_core::prelude::*;
use t3_client::workspace::project_files::formats::NotebookOutput;

use super::image::{bounded_size, raster_info};

use crate::{
    i18n::{Value, t, tr},
    screens::thread::markdown::base::chat_markdown_classes,
    theme::{cn, tw},
};

const MAX_OUTPUT_PIXELS: u64 = 4_000_000;
pub(super) const MAX_NOTEBOOK_PIXELS: u64 = 16_000_000;

fn output_pixels(width: u32, height: u32) -> Option<u64> {
    let pixels = u64::from(width) * u64::from(height);
    (bounded_size(width, height) && pixels <= MAX_OUTPUT_PIXELS).then_some(pixels)
}

fn reserve_image(budget: &Cell<u64>, width: u32, height: u32) -> Option<u64> {
    let pixels = output_pixels(width, height)?;
    budget.set(budget.get().checked_sub(pixels)?);
    Some(pixels)
}

pub(super) fn notebook_output(
    index: usize,
    output: &NotebookOutput,
    budget: &Cell<u64>,
) -> AnyElement {
    let body = match output {
        NotebookOutput::Text(value) => output_text(value.clone()).into_element(),
        NotebookOutput::HiddenHtml => output_text(t("HTML output not shown")).into_element(),
        NotebookOutput::Image { data_url, fallback } => {
            let label = tr(
                "Cell {index} output",
                &[("index", Value::Text((index + 1).to_string()))],
            );
            let fallback = fallback.clone().unwrap_or_else(|| label.clone());
            match admit_image(data_url, budget) {
                Some((width, height, reserved)) => {
                    notebook_image(data_url.clone(), label, fallback, (width, height), reserved)
                        .into_element()
                }
                None => output_text(fallback).into_element(),
            }
        }
    };
    view()
        .app_class(tw("border-t border-border/60"))
        .child(body)
        .into_element()
}

fn output_text(value: String) -> impl IntoElement {
    text(value).app_class(tw("p-4 font-mono text-xs"))
}

fn admit_image(data_url: &str, budget: &Cell<u64>) -> Option<(u32, u32, u64)> {
    if budget.get() == 0 {
        return None;
    }
    let (data_mime, bytes) = nsh_core::image::decode_data_uri(data_url).ok()?;
    let (mime, width, height) = raster_info(&bytes)?;
    if mime != data_mime {
        return None;
    }
    Some((width, height, reserve_image(budget, width, height)?))
}

fn notebook_image(
    data_url: String,
    label: String,
    fallback: String,
    dimensions: (u32, u32),
    reserved: u64,
) -> impl IntoElement {
    let natural = signal(dimensions);
    let failed = signal(false);
    when(
        move || !failed.get(),
        move || {
            view().app_class(tw("p-4")).child(
                image(data_url.clone())
                    .role(AccessibilityRole::Image)
                    .label(label.clone())
                    .w(move || {
                        let (width, height) = natural.get();
                        px(width as f32 * 1.0_f32.min(384.0 / height as f32))
                    })
                    .app_class(tw("max-w-full"))
                    .aspect_ratio(move || {
                        let (width, height) = natural.get();
                        width as f32 / height as f32
                    })
                    .on_load(move |size: &Size<DevicePixels>| {
                        let (width, height) =
                            (size.width.0.unsigned_abs(), size.height.0.unsigned_abs());
                        // A rotated image may swap axes. Its decoded area must stay within
                        // the reservation, which lasts for the lifetime of the notebook.
                        if output_pixels(width, height).is_some_and(|pixels| pixels <= reserved) {
                            natural.set((width, height));
                        } else {
                            failed.set(true);
                        }
                    })
                    .on_error(move |_: &str| failed.set(true)),
            )
        },
    )
    .otherwise(move || output_text(fallback.clone()))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notebook_images_draw_from_one_reservation_budget() {
        let budget = Cell::new(MAX_NOTEBOOK_PIXELS);
        assert_eq!(reserve_image(&budget, 2_001, 2_000), None);
        for _ in 0..4 {
            assert_eq!(
                reserve_image(&budget, 2_000, 2_000),
                Some(MAX_OUTPUT_PIXELS)
            );
        }
        assert_eq!(reserve_image(&budget, 1, 1), None);
        assert_eq!(budget.get(), 0);
        assert_eq!(output_pixels(0, 1), None);
    }
}
