//! TL-MARKDOWN-BASE: Port of apps/web/src/components/ChatMarkdown.tsx ChatMarkdown (the markdown
//! root). Until the package starts, the integrator keeps here the one place a message row renders
//! markdown, so every fenced block shows TL-MARKDOWN-1's chrome: rows call [`chat_markdown`]
//! instead of building their own TextView. Prose typography from `.chat-markdown` is the
//! package's.

use nsh_base::text_view::{CodeBlock, MarkdownOptions};
use nsh_core::prelude::*;

use super::code_block::{
    CodeBlockOptions, CodeBlockProps, CodeBlockState, extract_fence_title, markdown_code_block,
};
use crate::theme::tw;

/// ChatMarkdown over `text` (the whole source; see ui-package-addendum.md "Streaming text" for
/// how a streaming row must supply it). `streaming` keeps an unclosed fence open.
pub fn chat_markdown(
    text: impl Fn() -> String + 'static,
    streaming: impl Fn() -> bool + 'static,
) -> impl IntoElement {
    nsh_base::text_view()
        .markdown(move || Some(SharedString::from(text())))
        .options(move || MarkdownOptions {
            streaming: streaming(),
            ..Default::default()
        })
        .render_code_block(|code: &CodeBlock| fenced_code_block(code))
        .selectable(true)
        .app_class(tw("w-full text-sm text-foreground/90"))
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
