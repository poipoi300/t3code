//! Port of apps/web/src/components/ui/skeleton.tsx Skeleton and spinner.tsx Spinner.
//! Loading is static, as permitted by the package's repaint rule: no per-frame loop is started.
//! Skeleton's dimensions are layout props; the parent chooses its footprint. Spinner defaults
//! to the unsized source SVG; Button descendant sizing is a shared primitive seam.

use crate::i18n::t;
use crate::theme::{cn, tw};
use crate::ui::icon::Icon;
use nsh_core::prelude::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shape {
    #[default]
    Block,
    Card,
    Pill,
}

pub fn skeleton(
    shape: Shape,
    width: impl IntoReactive<Length>,
    height: impl IntoReactive<Length>,
) -> impl IntoElement {
    // motion-safe:animate-skeleton is static here; NSH has no visibility-gated stepped repaint.
    view()
        .accessible(false)
        .w(width)
        .h(height)
        .app_class(tw(&cn([
            Some("bg-muted-foreground/15"),
            Some(match shape {
                Shape::Block => "rounded-sm",
                Shape::Card => "rounded-lg",
                Shape::Pill => "rounded-full",
            }),
        ])))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpinnerSize {
    #[default]
    Inherit,
    Xs,
    Sm,
    Md,
    Lg,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Current,
    Muted,
}

/// Port of spinner.tsx Spinner. LoaderCircle stays a status glyph, never an unrelated ellipsis.
pub fn spinner(size: SpinnerSize, tone: Tone) -> impl IntoElement {
    // Static is the declared shared alternative to motion-safe:visible-animate-spin.
    svg(Icon::LoaderCircle.asset())
        .role(AccessibilityRole::Summary)
        .label(move || SharedString::from(t("Loading")))
        .app_class(tw(&cn([
            Some(match size {
                SpinnerSize::Inherit => "",
                SpinnerSize::Xs => "size-3",
                SpinnerSize::Sm => "size-3.5",
                SpinnerSize::Md => "size-4",
                SpinnerSize::Lg => "size-5",
            }),
            (tone == Tone::Muted).then_some("text-muted-foreground"),
        ])))
}
