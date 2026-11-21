//! TL-MARKDOWN-BASE: Port of apps/web/src/components/ChatMarkdown.tsx ChatMarkdown (the markdown
//! root) and apps/web/src/index.css's `.chat-markdown` rules, as TextView part classes. Every
//! fenced block shows TL-MARKDOWN-1's chrome.
//!
//! CSS block margins collapse; NSH lays blocks out in a flex column. `0.65rem` margins on every
//! block are therefore the column's gap, and a heading's `1.25rem` above becomes the gap plus
//! `0.6rem` (`first:` drops it, as `.chat-markdown > :first-child` does). A renderer that shows a
//! message as one view per block puts the blocks in [`chat_markdown_blocks`], so the spacing
//! between blocks is the same as inside one TextView.

use nsh_base::text_view::{
    Block, CodeBlock, CustomNode, Document, Image, ImageLength, MarkdownOptions, MarkdownPlugins,
    Parser, TextViewClasses, TextViewMath,
};
use nsh_core::prelude::*;

use super::code_block::{
    CodeBlockOptions, CodeBlockProps, CodeBlockState, extract_fence_title, markdown_code_block,
};
use crate::theme::{cn, tw};

/// ChatMarkdown over `text` (the whole source; see ui-package-addendum.md "Streaming text" for
/// how a streaming row must supply it). `streaming` keeps an unclosed fence open.
pub fn chat_markdown(
    text: impl Fn() -> String + 'static,
    streaming: impl Fn() -> bool + 'static,
) -> impl IntoElement {
    chat_markdown_with_root_classes(
        text,
        streaming,
        chat_markdown_classes().root.as_deref().unwrap_or_default(),
    )
}

/// ChatMarkdown with root overrides applied after its default layout and typography classes.
pub fn chat_markdown_with_root_classes(
    text: impl Fn() -> String + 'static,
    streaming: impl Fn() -> bool + 'static,
    root_classes: &str,
) -> AnyElement {
    let mut classes = chat_markdown_classes();
    if classes.root.as_deref() != Some(root_classes) {
        classes.root = Some(tw(&cn([classes.root.as_deref(), Some(root_classes)])));
    }
    let context = super::links::markdown_link_context();
    let plugins = use_context::<MarkdownPlugins>().unwrap_or_default();
    let images = use_context::<MarkdownImageRenderer>();
    let nested_images = images.clone();
    let streaming = std::rc::Rc::new(streaming);
    let parse_streaming = streaming.clone();
    let math = use_context::<ChatMarkdownSettings>().map(|settings| settings.math_enabled);
    let math_enabled = move || math.is_some_and(|math| math.get());
    let parsed = computed(move || {
        Parser::new()
            .options(MarkdownOptions {
                streaming: parse_streaming(),
                math: math_enabled(),
                ..Default::default()
            })
            .plugins(&plugins)
            .markdown(&text())
    });
    let document = computed(move || {
        let mut document = parsed.get();
        super::links::MarkdownLinkPlugin {
            context: context.get(),
        }
        .decorate_document(&mut document);
        document
    });
    nsh_base::text_view()
        .document(move || Some(document.get()))
        .options(move || MarkdownOptions {
            streaming: streaming(),
            math: math_enabled(),
            ..Default::default()
        })
        .classes(classes)
        .render_custom(move |node| {
            super::links::MarkdownLinkPlugin {
                context: context.get(),
            }
            .render_link(node, nested_images.clone())
            .unwrap_or_else(|| markdown_custom_fallback(node, nested_images.clone()))
        })
        .render_code_block(|code: &CodeBlock| fenced_code_block(code))
        .render_math(|formula: &TextViewMath| {
            let formula = formula.clone();
            super::rich::markdown_math(computed(move || formula.clone()), true).into_element()
        })
        .render_image(move |image: &Image| match &images {
            Some(renderer) => (renderer.0)(image),
            None => markdown_image(image),
        })
        .selectable(true)
        .into_element()
}

/// ChatMarkdown's `mathEnabled` (`mathRenderingEnabled`); the timeline provides it from the
/// client settings. Without a provider, math stays text, the setting's default.
#[derive(Clone, Copy)]
pub struct ChatMarkdownSettings {
    pub math_enabled: Memo<bool>,
}

/// ChatMarkdown.tsx `img` for a direct (http, https or data) source: ChatMarkdownImage at the
/// authored size. Workspace-file, context-reference and media sources need
/// `classifyMarkdownImageSource` and the asset binding, not ported; they show the unavailable
/// fallback.
fn markdown_image(image: &Image) -> AnyElement {
    use super::rich::{
        MarkdownImageState, MarkdownMediaProps, MediaKind, authored_image_size_style,
        chat_markdown_image, chat_markdown_image_fallback, markdown_image_copy,
    };
    let length = |length: Option<&ImageLength>| match length {
        Some(ImageLength::Points(points)) => Some(points.0.to_string()),
        _ => None,
    };
    let (width, height) = (length(image.width.as_ref()), length(image.height.as_ref()));
    let direct = ["http://", "https://", "data:image/"].iter().any(|scheme| {
        image
            .src
            .get(..scheme.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
    });
    let props = MarkdownMediaProps {
        src: direct.then(|| image.src.clone()),
        alt: image.alt.clone(),
        copy_markdown: Some(markdown_image_copy(
            &image.alt,
            &image.src,
            image.title.as_deref(),
        )),
        size: authored_image_size_style(width.as_deref(), height.as_deref(), 30.0),
        source_failed: !direct,
        ..Default::default()
    };
    let props = computed(move || props.clone());
    if direct {
        chat_markdown_image(props, MarkdownImageState::default(), false).into_element()
    } else {
        chat_markdown_image_fallback(props, MediaKind::Image).into_element()
    }
}

/// Applies to authored images, including images in nested link labels and custom nodes.
#[derive(Clone)]
pub struct MarkdownImageRenderer(pub std::rc::Rc<dyn Fn(&Image) -> AnyElement>);

pub(super) fn nested_text_view(
    images: Option<MarkdownImageRenderer>,
) -> nsh_base::text_view::TextViewElement {
    let view = nsh_base::text_view();
    match images {
        Some(renderer) => view.render_image(move |image: &Image| (renderer.0)(image)),
        None => view,
    }
}

fn markdown_custom_fallback(
    node: &CustomNode,
    images: Option<MarkdownImageRenderer>,
) -> AnyElement {
    let blocks = if !node.children.is_empty() {
        node.children.clone()
    } else if !node.content.is_empty() {
        vec![Block::paragraph(node.content.clone())]
    } else {
        return text(SharedString::from(node.text.clone())).into_element();
    };
    nested_text_view(images)
        .document(Some(Document::new(blocks)))
        .classes(chat_markdown_classes())
        .into_element()
}

/// The `.chat-markdown` rules by TextView part.
pub fn chat_markdown_classes() -> TextViewClasses {
    thread_local! {
        static CLASSES: TextViewClasses = TextViewClasses {
            // ChatMarkdown's root, `text-foreground/[calc(90%+var(--appearance-contrast-boost)/10)]`
            // at the default contrast, plus the collapsed block margin.
            root: Some(tw("w-full min-w-0 gap-[0.65rem] text-sm leading-relaxed text-foreground/90")),
            heading: Some(tw(
                "mt-[0.6rem] first:mt-0 font-semibold tracking-normal text-contrast-foreground",
            )),
            heading1: Some(tw("text-xl leading-[1.3]")),
            heading2: Some(tw("text-lg leading-[1.3]")),
            heading3: Some(tw("text-base leading-[1.3]")),
            heading4: Some(tw("text-sm leading-[1.3]")),
            heading5: Some(tw("text-sm leading-[1.3]")),
            heading6: Some(tw("text-sm leading-[1.3] text-contrast-muted-foreground")),
            // `.chat-markdown li + li`.
            list: Some(tw("gap-1")),
            link: Some(tw("text-info-foreground no-underline")),
            ..TextViewClasses::default()
        };
    }
    CLASSES.with(Clone::clone)
}

/// The `.chat-markdown` root for a message shown as one view per block (prose leaves, code
/// chunks, tables): the root typography and the collapsed block margin between its children.
pub fn chat_markdown_blocks() -> View {
    view().app_class(tw(
        "flex w-full min-w-0 flex-col gap-[0.65rem] text-sm leading-relaxed text-foreground/90",
    ))
}

/// One block of [`chat_markdown_blocks`]. A block whose source starts with a heading carries the
/// heading's top margin itself, since inside its own TextView the heading is the first block.
pub fn chat_markdown_leaf(starts_with_heading: bool, child: impl IntoElement) -> impl IntoElement {
    let leaf = if starts_with_heading {
        view().app_class(tw("mt-[0.6rem] first:mt-0"))
    } else {
        view()
    };
    leaf.child(child)
}

/// Whether a block's markdown source starts with a heading: an ATX heading (up to three spaces,
/// one to six `#`, then a space or the end) or a setext heading (a line, then `=`s or `-`s).
pub fn starts_with_heading(source: &str) -> bool {
    let mut lines = source.lines().skip_while(|line| line.trim().is_empty());
    let Some(first) = lines.next() else {
        return false;
    };
    let indent = first.len() - first.trim_start_matches(' ').len();
    if indent > 3 {
        return false;
    }
    let content = &first[indent..];
    let hashes = content.len() - content.trim_start_matches('#').len();
    if hashes > 0 {
        return hashes <= 6
            && content[hashes..]
                .chars()
                .next()
                .is_none_or(|c| c == ' ' || c == '\t');
    }
    let Some(underline) = lines.next() else {
        return false;
    };
    let underline = underline.trim_end();
    let underline_indent = underline.len() - underline.trim_start_matches(' ').len();
    let marks = underline.trim_start_matches(' ');
    underline_indent <= 3
        && !marks.is_empty()
        && (marks.chars().all(|c| c == '=') || marks.chars().all(|c| c == '-'))
        && !content.trim().is_empty()
}

/// A fenced block in MarkdownCodeBlock's chrome. The body is plain until TL-BLOCKS-2's
/// highlighted chunks mount; the wrap toggle's state lives with the block while it is shown.
fn fenced_code_block(code: &CodeBlock) -> AnyElement {
    let props = CodeBlockProps {
        code: code.code.clone(),
        language: code.language.clone().unwrap_or_else(|| "text".into()),
        fence_title: extract_fence_title(code.meta.as_deref()),
        ..CodeBlockProps::default()
    };
    if props.language == "mermaid" {
        return super::rich::markdown_mermaid_code_block(
            computed(move || props.clone()),
            CodeBlockState::new(),
        )
        .into_element();
    }
    let body = code.code.clone();
    markdown_code_block(
        computed(move || props.clone()),
        CodeBlockState::new(),
        CodeBlockOptions::default(),
        move |_wrapped| {
            text(SharedString::from(body.clone()))
                .select_text()
                .app_class(tw("font-mono"))
                .into_element()
        },
    )
    .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_are_found_at_the_start_of_a_block_as_commonmark_reads_them() {
        for (source, expected) in [
            ("# Done", true),
            ("## Done\n\nI added clamp.", true),
            ("   ### Indented three", true),
            ("    # Indented four is code", false),
            ("######", true),
            ("####### Seven is a paragraph", false),
            ("#hashtag", false),
            ("Title\n=====", true),
            ("Title\n---", true),
            ("\n\n# After blank lines", true),
            ("---", false),
            ("Plain paragraph\nsecond line", false),
            ("- a list", false),
            ("```ts\n# not a heading\n```", false),
            ("", false),
        ] {
            assert_eq!(starts_with_heading(source), expected, "{source:?}");
        }
    }
}
