//! Port of apps/web/src/components/ui/toggle.tsx Toggle and toggle-group.tsx ToggleGroup / Toggle.
//!
//! `toggle(content, variant, size, pressed, disabled, on_change)` is controlled: a press requests
//! a value, and only `pressed` changes its display. `Content` is the label/icon slot (labels are
//! already translated). `Variant` and `Size` retain every web cva name; neither accepts classes.
//! `toggle_group(label, children, options, value, disabled, on_change)` inherits its variant/size
//! into `GroupItem`s, with optional per-item overrides and `GroupChild::Separator` slots.
//! The default group is segmented, horizontal, single selection; pressing its selected item
//! clears it. Multiple selection preserves value order. Callers own selection and persistence.

use std::rc::Rc;

use nsh_core::prelude::*;

use crate::theme::{self, Scope, cn, metrics, palette, tw, tw_with};
use crate::ui::icon::Icon;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Variant {
    #[default]
    Default,
    Pill,
    Ghost,
    Outline,
    Segmented,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    Compact,
    #[default]
    Default,
    Lg,
    Segmented,
    Sm,
    Xs,
}

/// Content and its translated accessible name, shared with the button primitive.
pub use super::button::ButtonContent as Content;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Orientation {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupOptions {
    pub variant: Variant,
    /// None inherits segmented size for segmented groups, default size otherwise.
    pub size: Option<Size>,
    pub orientation: Orientation,
    pub multiple: bool,
}

impl Default for GroupOptions {
    fn default() -> Self {
        Self {
            variant: Variant::Segmented,
            size: None,
            orientation: Orientation::Horizontal,
            multiple: false,
        }
    }
}

pub struct GroupItem {
    pub value: String,
    pub content: Content,
    pub variant: Option<Variant>,
    pub size: Option<Size>,
    pub disabled: Prop<bool>,
}

impl GroupItem {
    pub fn new(value: impl Into<String>, content: Content) -> Self {
        Self {
            value: value.into(),
            content,
            variant: None,
            size: None,
            disabled: false.into(),
        }
    }
}

pub enum GroupChild {
    Item(GroupItem),
    Separator,
}

impl From<GroupItem> for GroupChild {
    fn from(item: GroupItem) -> Self {
        Self::Item(item)
    }
}

/// Port of toggle.tsx Toggle. A disabled control neither requests nor changes a value.
pub fn toggle(
    content: Content,
    variant: Variant,
    size: Size,
    pressed: impl Fn() -> bool + 'static,
    disabled: impl Fn() -> bool + 'static,
    on_change: impl Fn(bool) + 'static,
) -> impl IntoElement {
    toggle_inner(content, variant, size, Rc::new(pressed), Rc::new(disabled), on_change, None)
}

#[derive(Clone, Copy)]
struct Position {
    options: GroupOptions,
    first: bool,
    last: bool,
}

fn toggle_inner(
    content: Content,
    variant: Variant,
    size: Size,
    pressed: Rc<dyn Fn() -> bool>,
    disabled: Rc<dyn Fn() -> bool>,
    on_change: impl Fn(bool) + 'static,
    position: Option<Position>,
) -> impl IntoElement {
    let (label, glyph, show_label) = match content {
        Content::Label(label) => (label, None, true),
        Content::Icon { icon, label } => (label, Some(icon), false),
        Content::IconLabel { icon, label } => (label, Some(icon), true),
    };
    let pressed_prop = pressed.clone();
    let disabled_prop = disabled.clone();
    let disabled_event = disabled.clone();
    let pressed_classes = pressed.clone();
    let disabled_classes = disabled.clone();
    let before_pressed = pressed.clone();
    let before_disabled = disabled.clone();
    let glyph_disabled = disabled.clone();
    nsh_base::toggle()
        .accessibility_label(Some(SharedString::from(label.clone())))
        .pressed(move || pressed_prop())
        .disabled(move || disabled_prop())
        .on_change(move |next: &bool| {
            if !disabled_event() {
                on_change(*next);
            }
        })
        .app_class(move || Some(toggle_classes(variant, size, pressed_classes(), disabled_classes(), position)))
        // The pointer-coarse after layer is a hit target; groups constrain it on their main axis.
        .hit_slop(move || hit_slop(size, position.map(|p| p.options.orientation)))
        // NSH has no pseudo-elements: before is the first, noninteractive child.
        .child(view()
            .app_class(move || Some(before_classes(variant, before_pressed(), before_disabled(), position)))
            .rounded(move || {
                let radius = theme::current_tokens().number("radius").unwrap_or(8.0);
                px((radius - if matches!(size, Size::Compact | Size::Segmented) { 3.0 } else { 1.0 }).max(0.0))
            })
            .accessible(false))
        // Descendant SVG selectors are applied directly to the decorative image slot.
        .child(glyph.map(|glyph| image(glyph.asset())
            .app_class(move || Some(tw(&cn([
                Some("-mx-0.5 pointer-events-none shrink-0"),
                Some(if size == Size::Compact { "size-3.5" } else { "size-4.5 sm:size-4" }),
                Some(if variant == Variant::Ghost && glyph_disabled() { "opacity-100" } else { "opacity-80" }),
            ]))))
            .tint_color(move || if variant == Variant::Pill || variant == Variant::Segmented {
                if pressed() { palette().foreground } else { palette().muted_foreground }
            } else if variant == Variant::Ghost && disabled() { palette().muted_foreground }
            else if pressed() { palette().accent_foreground } else { palette().foreground })
            .accessible(false)))
        .child(show_label.then(|| text(SharedString::from(label))))
}

fn toggle_classes(variant: Variant, size: Size, pressed: bool, disabled: bool, position: Option<Position>) -> SharedString {
    let attrs = [("data-pressed", if pressed { "true" } else { "false" })];
    let classes = cn([
        // Whitespace wrapping is native; before/after and SVG selectors are ported above.
        Some("relative inline-flex shrink-0 cursor-pointer select-none items-center justify-center gap-2 rounded-lg border font-medium text-base text-foreground outline-none transition-shadow hover:bg-accent focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background disabled:pointer-events-none disabled:opacity-64 aria-disabled:opacity-64 data-pressed:bg-input/64 data-pressed:text-accent-foreground sm:text-sm"),
        Some(match size {
            // --spacing(n) is four pixels per unit; subtract the one-pixel border as in TSX.
            Size::Compact => "h-7 min-w-7 rounded-md px-[3px] text-xs",
            Size::Default => "h-9 min-w-9 px-[7px] sm:h-8 sm:min-w-8",
            Size::Lg => "h-10 min-w-10 px-[9px] sm:h-9 sm:min-w-9",
            Size::Segmented => "h-6 min-w-0 rounded-md px-2.5 text-xs",
            Size::Sm => "h-8 min-w-8 px-[5px] sm:h-7 sm:min-w-7",
            Size::Xs => "h-7 min-w-7 px-[3px] sm:h-6 sm:min-w-6 rounded-md",
        }),
        Some(match variant {
            Variant::Default => "border-transparent",
            Variant::Pill => "border-transparent rounded-full px-3 font-normal text-muted-foreground data-pressed:text-foreground",
            Variant::Ghost => "border-transparent text-foreground shadow-none data-pressed:bg-accent data-pressed:text-accent-foreground disabled:opacity-100 disabled:text-muted-foreground",
            // The outline's conditional pseudo-shadows live on before; background clipping is
            // light-only in CSS and has no effect on NSH's separately painted border.
            Variant::Outline => "border-input bg-background shadow-xs/5 dark:bg-input/32 dark:data-pressed:bg-input dark:hover:bg-input/64",
            Variant::Segmented => "border-transparent text-muted-foreground shadow-none transition-colors hover:bg-background/55 hover:text-foreground data-pressed:bg-background data-pressed:text-foreground data-pressed:shadow-xs/10 dark:hover:bg-input/32 dark:data-pressed:bg-input/72",
        }),
        (variant == Variant::Outline && (disabled || pressed)).then_some("shadow-none"),
        (variant == Variant::Outline).then_some("active:shadow-none"),
        position.map(|_| "focus-visible:z-10"),
        joined_classes(position, false),
    ]);
    tw_with(Scope::Root, &classes, &attrs)
}

fn joined_classes(position: Option<Position>, before: bool) -> Option<&'static str> {
    let p = position?;
    if matches!(p.options.variant, Variant::Default | Variant::Segmented) {
        return None;
    }
    Some(match (p.options.orientation, p.first, p.last, before) {
        (Orientation::Horizontal, true, true, _) | (Orientation::Vertical, true, true, _) => "",
        (Orientation::Horizontal, true, false, false) => "rounded-e-none border-e-0",
        (Orientation::Horizontal, false, true, false) => "rounded-s-none border-s-0",
        (Orientation::Horizontal, false, false, false) => "rounded-s-none rounded-e-none border-s-0 border-e-0",
        (Orientation::Vertical, true, false, false) => "rounded-b-none border-b-0",
        (Orientation::Vertical, false, true, false) => "rounded-t-none border-t-0",
        (Orientation::Vertical, false, false, false) => "rounded-t-none rounded-b-none border-t-0 border-b-0",
        (Orientation::Horizontal, true, false, true) => "-end-[0.5px] rounded-e-none",
        (Orientation::Horizontal, false, true, true) => "-start-[0.5px] rounded-s-none",
        (Orientation::Horizontal, false, false, true) => "-start-[0.5px] -end-[0.5px] rounded-s-none rounded-e-none",
        (Orientation::Vertical, true, false, true) => "-bottom-[0.5px] rounded-b-none dark:block",
        (Orientation::Vertical, false, true, true) => "-top-[0.5px] rounded-t-none dark:hidden",
        (Orientation::Vertical, false, false, true) => "-top-[0.5px] -bottom-[0.5px] rounded-t-none rounded-b-none hidden",
    })
}

fn before_classes(variant: Variant, pressed: bool, disabled: bool, position: Option<Position>) -> SharedString {
    tw(&cn([
        Some("pointer-events-none absolute inset-0"),
        // --theme colour shadows are explicit CSS equivalents, with the same zero blur.
        (variant == Variant::Outline && !pressed && !disabled).then_some("shadow-[0_1px_0_rgb(0_0_0/4%)] dark:shadow-[0_-1px_0_rgb(255_255_255/6%)] group-active/toggle:shadow-none"),
        (variant == Variant::Outline && disabled && !pressed).then_some("dark:shadow-[0_-1px_0_rgb(255_255_255/2%)]"),
        joined_classes(position, true),
    ]))
}

fn hit_slop(size: Size, group: Option<Orientation>) -> EdgeInsets {
    let m = metrics();
    let height = match size {
        Size::Compact => 28.0,
        Size::Default => m.control,
        Size::Lg => m.control_lg,
        Size::Segmented => 24.0,
        Size::Sm => m.control_sm,
        Size::Xs => m.control_xs,
    };
    let slack = ((m.hit_target - height) / 2.0).max(0.0);
    EdgeInsets {
        top: px(if group == Some(Orientation::Vertical) { 0.0 } else { slack }),
        bottom: px(if group == Some(Orientation::Vertical) { 0.0 } else { slack }),
        left: px(if group == Some(Orientation::Horizontal) { 0.0 } else { slack }),
        right: px(if group == Some(Orientation::Horizontal) { 0.0 } else { slack }),
    }
}

/// Port of toggle-group.tsx ToggleGroup. Separators occupy their TSX sibling positions.
pub fn toggle_group(
    label: String,
    children: Vec<GroupChild>,
    options: GroupOptions,
    value: impl Fn() -> Vec<String> + 'static,
    disabled: impl Fn() -> bool + 'static,
    on_change: impl Fn(Vec<String>) + 'static,
) -> impl IntoElement {
    let value: Rc<dyn Fn() -> Vec<String>> = Rc::new(value);
    let disabled: Rc<dyn Fn() -> bool> = Rc::new(disabled);
    let on_change: Rc<dyn Fn(Vec<String>)> = Rc::new(on_change);
    let group_disabled = disabled.clone();
    let size = options.size.unwrap_or(if options.variant == Variant::Segmented { Size::Segmented } else { Size::Default });
    let len = children.len();
    let children: Vec<AnyElement> = children.into_iter().enumerate().map(|(index, child)| match child {
        GroupChild::Item(item) => toggle_group_item(item, options, size, value.clone(), disabled.clone(), on_change.clone(), index == 0, index + 1 == len),
        GroupChild::Separator => group_separator(options, value.clone(), &[]),
    }).collect();
    nsh_base::toggle_group()
        .accessibility_label(Some(SharedString::from(label)))
        .disabled(move || group_disabled())
        .app_class(tw(&cn([
            Some("flex w-fit"),
            (options.orientation == Orientation::Vertical).then_some("flex-col"),
            Some(match options.variant {
                Variant::Segmented => "gap-0.5 rounded-lg bg-input/40 p-0.5",
                Variant::Default => "gap-0.5",
                _ => "",
            }),
        ])))
        .children(children)
}

/// Port of toggle-group.tsx Toggle (also exported there as ToggleGroupItem).
#[allow(clippy::too_many_arguments)]
fn toggle_group_item(
    item: GroupItem,
    options: GroupOptions,
    size: Size,
    value: Rc<dyn Fn() -> Vec<String>>,
    disabled: Rc<dyn Fn() -> bool>,
    on_change: Rc<dyn Fn(Vec<String>)>,
    first: bool,
    last: bool,
) -> AnyElement {
    let item_value = item.value;
    let pressed_value = item_value.clone();
    let read = value.clone();
    let item_disabled = item.disabled;
    let disabled: Rc<dyn Fn() -> bool> = Rc::new(move || disabled() || item_disabled.get());
    let event_disabled = disabled.clone();
    toggle_inner(item.content, item.variant.unwrap_or(options.variant), item.size.unwrap_or(size),
        Rc::new(move || read().contains(&pressed_value)), disabled,
        move |pressed| {
            if event_disabled() { return; }
            on_change(next_value(value(), &item_value, pressed, options.multiple));
        }, Some(Position { options, first, last }))
        .into_element()
}

fn next_value(mut value: Vec<String>, item: &str, pressed: bool, multiple: bool) -> Vec<String> {
    if !pressed {
        value.retain(|v| v != item);
        return value;
    }
    if !multiple { return vec![item.to_owned()]; }
    if !value.iter().any(|v| v == item) { value.push(item.to_owned()); }
    value
}

fn group_separator(options: GroupOptions, _value: Rc<dyn Fn() -> Vec<String>>, _neighbors: &[String]) -> AnyElement {
    // Separator's orientation is perpendicular to the group's; its child selector is local.
    view().role(AccessibilityRole::Separator).app_class(tw(&cn([
        Some("shrink-0 bg-border"),
        Some(if options.orientation == Orientation::Horizontal { "w-px self-stretch" } else { "h-px w-full" }),
    ]))).into_element()
}
