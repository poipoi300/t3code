//! Typography: the text roles the web app uses, in the palette's colours.

use nsh_core::prelude::*;

use crate::theme::{metrics, palette};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Default,
    Muted,
    Destructive,
    Warning,
    Success,
}

impl Tone {
    pub fn color(self) -> AdaptiveColor {
        let p = palette();
        match self {
            Self::Default => p.foreground,
            Self::Muted => p.muted_foreground,
            Self::Destructive => p.destructive,
            Self::Warning => p.warning,
            Self::Success => p.success,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextRole {
    /// Page and dialog titles (`text-lg font-semibold`).
    Title,
    /// Section headers (`text-sm font-medium`).
    Heading,
    /// Body copy (`text-base sm:text-sm`).
    Body,
    /// Secondary lines, timestamps, hints (`text-xs`).
    Caption,
    /// Monospaced inline values (paths, ids).
    Code,
}

/// A run of text in a role and tone. `content` is already translated (`i18n::t`).
pub fn label(
    role: TextRole,
    tone: Tone,
    content: impl Fn() -> String + 'static,
) -> impl IntoElement {
    let size = move || {
        let m = metrics();
        px(match role {
            TextRole::Title => m.text_lg,
            TextRole::Heading | TextRole::Body => m.text,
            TextRole::Caption => m.text_xs,
            TextRole::Code => m.text_xs,
        })
    };
    let weight = match role {
        TextRole::Title => FontWeight::SEMIBOLD,
        TextRole::Heading => FontWeight::MEDIUM,
        _ => FontWeight::NORMAL,
    };
    let base = text(move || SharedString::from(content()))
        .text_size(size)
        .font_weight(weight)
        .text_color(move || tone.color());
    match role {
        TextRole::Code => base.font_family(SharedString::from("ui-monospace, Consolas, monospace")),
        _ => base,
    }
}
