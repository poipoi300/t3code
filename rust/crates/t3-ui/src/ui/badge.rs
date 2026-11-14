//! Port of apps/web/src/components/ui/badge.tsx Badge and badgeVariants.

use std::rc::Rc;

use nsh_core::prelude::*;

use crate::i18n::Text;
use crate::theme::{self, Scope, cn, tw_with};
use crate::ui::icon::{Icon, icon_with_class};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Variant {
    #[default]
    Default,
    Destructive,
    Error,
    Info,
    Outline,
    Secondary,
    Success,
    Warning,
    /// The web's per-label `--label` custom property, required for this variant.
    Label(AdaptiveColor),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    Control,
    #[default]
    Default,
    Lg,
    Sm,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Props {
    pub variant: Variant,
    pub size: Size,
    pub scope: Scope,
}

/// Explicit SVG slots apply the source's descendant selectors, preserving sibling order.
#[allow(
    clippy::large_enum_variant,
    reason = "Mount-only descriptors become NSH's inline AnyElement children; preserve the unboxed element slot."
)]
pub enum Child {
    Text(Text),
    Icon(Icon),
    StyledIcon { icon: Icon, class_name: String },
    Element(AnyElement),
}

impl From<Text> for Child {
    fn from(text: Text) -> Self {
        Self::Text(text)
    }
}

impl From<AnyElement> for Child {
    fn from(element: AnyElement) -> Self {
        Self::Element(element)
    }
}

pub(crate) fn children(children: Vec<Child>, icon_classes: &str) -> Vec<AnyElement> {
    children
        .into_iter()
        .map(|child| match child {
            Child::Text(value) => text(move || value.resolve()).into_element(),
            Child::Icon(glyph) => icon_with_class(glyph, icon_classes).into_element(),
            Child::StyledIcon { icon, class_name } => {
                let defaults = icon_classes
                    .split_whitespace()
                    .filter(|class| {
                        !(class.contains("size-") && class_name.contains("size-")
                            || class.contains("opacity-") && class_name.contains("opacity-"))
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let enforced = icon_classes
                    .split_whitespace()
                    .filter(|class| matches!(*class, "pointer-events-none" | "shrink-0"))
                    .collect::<Vec<_>>()
                    .join(" ");
                icon_with_class(
                    icon,
                    &cn([
                        Some(defaults.as_str()),
                        Some(class_name.as_str()),
                        Some(enforced.as_str()),
                    ]),
                )
                .into_element()
            }
            Child::Element(element) => element,
        })
        .collect()
}

fn classes(props: Props, interactive: bool) -> String {
    cn([
        // Descendant SVG defaults live on Child::Icon; coarse after targets become hit_slop.
        Some(
            "relative inline-flex shrink-0 items-center justify-center gap-1 whitespace-nowrap rounded-sm border border-transparent font-medium outline-none transition-shadow focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background disabled:pointer-events-none disabled:opacity-64",
        ),
        Some(match props.size {
            // NSH cannot parse --spacing() arithmetic; these are spacing(n)-1px at the web's 4px unit.
            Size::Control => {
                "h-7 min-w-7 rounded-(--control-radius) px-[7px] text-sm sm:h-6 sm:min-w-6 sm:text-xs"
            }
            Size::Default => "h-5.5 min-w-5.5 px-[3px] text-sm sm:h-4.5 sm:min-w-4.5 sm:text-xs",
            Size::Lg => "h-6.5 min-w-6.5 px-[5px] text-base sm:h-5.5 sm:min-w-5.5 sm:text-sm",
            Size::Sm => {
                "h-5 min-w-5 rounded-[.25rem] px-[3px] text-xs leading-none sm:h-4 sm:min-w-4 sm:text-[.625rem]"
            }
        }),
        Some(match props.variant {
            Variant::Default => "bg-primary text-primary-foreground",
            Variant::Destructive => "bg-destructive text-white",
            Variant::Error => "bg-destructive/8 text-destructive-foreground dark:bg-destructive/16",
            Variant::Info => "bg-info/8 text-info-foreground dark:bg-info/16",
            Variant::Outline => "border-input bg-background text-foreground dark:bg-input/32",
            Variant::Secondary => "bg-secondary text-secondary-foreground",
            Variant::Success => "bg-success/8 text-success-foreground dark:bg-success/16",
            Variant::Warning => "bg-warning/8 text-warning-foreground dark:bg-warning/16",
            Variant::Label(_) => "",
        }),
        interactive.then_some("cursor-pointer"),
        interactive.then_some(match props.variant {
            Variant::Default => "hover:bg-primary/90",
            Variant::Destructive => "hover:bg-destructive/90",
            Variant::Outline => "hover:bg-accent/50 dark:hover:bg-input/48",
            Variant::Secondary => "hover:bg-secondary/90",
            _ => "",
        }),
    ])
}

const ICON_CLASSES: &str = "opacity-80 size-3.5 sm:size-3 pointer-events-none shrink-0";

pub fn badge(props: Props, content: Vec<Child>) -> impl IntoElement {
    let root = view()
        .app_class(tw_with(props.scope, &classes(props, false), &[]))
        .children(children(content, ICON_CLASSES));
    match props.variant {
        Variant::Label(color) => root
            // Subtree CSS custom properties/color-mix cannot be represented as NSH classes.
            .bg(move || label_colors(color, props.scope).0)
            .text_color(move || label_colors(color, props.scope).1),
        _ => root,
    }
}

pub fn badge_button(
    props: Props,
    label: Text,
    content: Vec<Child>,
    disabled: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let disabled = Rc::new(disabled);
    let read_disabled = disabled.clone();
    let measured = signal(size(px(0.0), px(0.0)));
    let root = nsh_base::button()
        .accessibility_label(move || label.resolve())
        .disabled(move || read_disabled())
        .hit_slop(move || coarse_target(measured.get()))
        .on_press(move || {
            if !disabled() {
                on_press();
            }
        })
        .app_class(tw_with(props.scope, &classes(props, true), &[]))
        .children(children(content, ICON_CLASSES));
    let root = match props.variant {
        Variant::Label(color) => root
            .bg(move || label_colors(color, props.scope).0)
            .text_color(move || label_colors(color, props.scope).1),
        _ => root,
    };
    view()
        .app_class(crate::theme::tw("inline-flex"))
        .on_layout(move |bounds: &Bounds<Points>| measured.set(bounds.size))
        .child(root)
}

pub fn badge_link(
    props: Props,
    label: Text,
    href: impl Fn() -> SharedString + 'static,
    content: Vec<Child>,
    disabled: impl Fn() -> bool + 'static,
    on_open: impl Fn(&str) + 'static,
) -> impl IntoElement {
    let measured = signal(size(px(0.0), px(0.0)));
    let root = nsh_base::link()
        .accessibility_label(move || label.resolve())
        .href(href)
        .disabled(disabled)
        .hit_slop(move || coarse_target(measured.get()))
        .on_open(move |href: &SharedString| on_open(href))
        .app_class(tw_with(props.scope, &classes(props, true), &[]))
        .children(children(content, ICON_CLASSES));
    let root = match props.variant {
        Variant::Label(color) => root
            .bg(move || label_colors(color, props.scope).0)
            .text_color(move || label_colors(color, props.scope).1),
        _ => root,
    };
    view()
        .app_class(crate::theme::tw("inline-flex"))
        .on_layout(move |bounds: &Bounds<Points>| measured.set(bounds.size))
        .child(root)
}

fn coarse_target(measured: nsh_core::Size<Points>) -> EdgeInsets {
    if crate::state::layout::pointer() != crate::state::layout::Pointer::Coarse {
        return EdgeInsets::default();
    }
    let horizontal = (44.0 - measured.width.0).max(0.0) / 2.0;
    let vertical = (44.0 - measured.height.0).max(0.0) / 2.0;
    EdgeInsets {
        left: px(horizontal),
        right: px(horizontal),
        top: px(vertical),
        bottom: px(vertical),
    }
}

fn label_colors(color: AdaptiveColor, scope: Scope) -> (AdaptiveColor, AdaptiveColor) {
    let foreground = match scope {
        Scope::Root => theme::palette().foreground,
        Scope::Sidebar => tokens()
            .color("sb-foreground")
            .unwrap_or(theme::palette().foreground),
    };
    (
        AdaptiveColor::new(color.light.opacity(0.08), color.dark.opacity(0.12)),
        AdaptiveColor::new(
            mix(color.light, foreground.light, 0.30),
            mix(color.dark, foreground.dark, 0.45),
        ),
    )
}

fn mix(label: Hsla, foreground: Hsla, amount: f32) -> Hsla {
    let label = Rgba::from(label);
    let foreground = Rgba::from(foreground);
    let a = label.a * amount + foreground.a * (1.0 - amount);
    if a == 0.0 {
        return Hsla::transparent_black();
    }
    let channel = |x: f32, y: f32| (x * label.a * amount + y * foreground.a * (1.0 - amount)) / a;
    Rgba {
        r: channel(label.r, foreground.r),
        g: channel(label.g, foreground.g),
        b: channel(label.b, foreground.b),
        a,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_color_mix_uses_srgb_and_premultiplied_alpha() {
        let red = Rgba {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        let blue = Rgba {
            r: 0.0,
            g: 0.0,
            b: 1.0,
            a: 1.0,
        };
        let mixed = Rgba::from(mix(red.into(), blue.into(), 0.30));
        assert!((mixed.r - 0.30).abs() < 0.00001);
        assert!((mixed.b - 0.70).abs() < 0.00001);
        let clear_red = Rgba { a: 0.0, ..red };
        let mixed = Rgba::from(mix(clear_red.into(), blue.into(), 0.45));
        assert!(mixed.r.abs() < 0.00001);
        assert!((mixed.b - 1.0).abs() < 0.00001);
        assert!((mixed.a - 0.55).abs() < 0.00001);
        assert!(mix(clear_red.into(), Rgba { a: 0.0, ..blue }.into(), 0.5).is_transparent());
    }
}
