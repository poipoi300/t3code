//! Port of apps/web/src/components/ui/kbd.tsx Kbd and KbdGroup.

use nsh_core::prelude::*;

use crate::theme::{cn, tw};
use crate::ui::badge::{Child, children};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Variant {
    #[default]
    Default,
    Outline,
}

pub fn kbd(variant: Variant, content: Vec<Child>) -> impl IntoElement {
    view()
        // NSH has no kbd tag; noninteractive inline content preserves the source's semantics.
        .app_class(tw(&cn([
            Some("pointer-events-none inline-flex h-5 min-w-5 select-none items-center justify-center gap-1 rounded px-1 font-medium font-sans text-muted-foreground text-xs"),
            Some(match variant {
                Variant::Outline => "border border-b-2 border-foreground/15 px-1.5",
                Variant::Default => "bg-foreground/[0.07]",
            }),
        ])))
        .children(children(content, "size-3"))
}

pub fn kbd_group(children: impl IntoElement) -> impl IntoElement {
    view()
        .app_class(tw("inline-flex items-center gap-1"))
        .child(children)
}
