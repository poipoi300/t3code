//! Port of apps/web/src/components/ui/toggle.tsx Toggle and toggle-group.tsx ToggleGroup / Toggle.
//!
//! `toggle(content, variant, size, pressed, disabled, on_change)` is controlled: a press requests
//! a value, and only `pressed` changes its display. `Content` is the label/icon/custom slot (labels are
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

/// Labels are translated. Custom children apply the TSX descendant icon classes at their source.
pub enum Content {
    Label(String),
    Icon { icon: Icon, label: String },
    IconLabel { icon: Icon, label: String },
    Slot { label: String, children: AnyElement },
}

impl From<super::button::ButtonContent> for Content {
    fn from(content: super::button::ButtonContent) -> Self {
        match content {
            super::button::ButtonContent::Label(label) => Self::Label(label),
            super::button::ButtonContent::Icon { icon, label } => Self::Icon { icon, label },
            super::button::ButtonContent::IconLabel { icon, label } => {
                Self::IconLabel { icon, label }
            }
        }
    }
}

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
    pub fn new(value: impl Into<String>, content: impl Into<Content>) -> Self {
        Self {
            value: value.into(),
            content: content.into(),
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
    content: impl Into<Content>,
    variant: Variant,
    size: Size,
    pressed: impl Fn() -> bool + 'static,
    disabled: impl Fn() -> bool + 'static,
    on_change: impl Fn(bool) + 'static,
) -> impl IntoElement {
    toggle_inner(
        content.into(),
        variant,
        size,
        Rc::new(pressed),
        Rc::new(disabled),
        on_change,
        None,
    )
}

#[derive(Clone, Copy)]
struct Position {
    options: GroupOptions,
    first: bool,
    last: bool,
    hovered: Signal<bool>,
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
    let (label, glyph, show_label, children) = match content {
        Content::Label(label) => (label, None, true, None),
        Content::Icon { icon, label } => (label, Some(icon), false, None),
        Content::IconLabel { icon, label } => (label, Some(icon), true, None),
        Content::Slot { label, children } => (label, None, false, Some(children)),
    };
    let pressed_prop = pressed.clone();
    let disabled_prop = disabled.clone();
    let disabled_event = disabled.clone();
    let pressed_classes = pressed.clone();
    let disabled_classes = disabled.clone();
    let before_pressed = pressed.clone();
    let before_disabled = disabled.clone();
    let glyph_disabled = disabled.clone();
    let hover_pressed = pressed.clone();
    let hover_disabled = disabled.clone();
    let measured = signal(nsh_core::Size::new(px(0.0), px(0.0)));
    nsh_base::toggle()
        .accessibility_label(Some(SharedString::from(label.clone())))
        .pressed(move || pressed_prop())
        .disabled(move || disabled_prop())
        .on_change(move |next: &bool| {
            if !disabled_event() {
                on_change(*next);
            }
        })
        .app_class(move || {
            Some(toggle_classes(
                variant,
                size,
                pressed_classes(),
                disabled_classes(),
                position,
            ))
        })
        .on_mouse_enter(move |_: &MouseEvent| {
            if let Some(p) = position {
                p.hovered.set(true);
            }
        })
        .on_mouse_leave(move |_: &MouseEvent| {
            if let Some(p) = position {
                p.hovered.set(false);
            }
        })
        // The pointer-coarse after layer is a hit target; groups constrain it on their main axis.
        .on_layout(move |bounds: &Bounds<Points>| measured.set(bounds.size))
        .hit_slop(move || {
            hit_slop(
                size,
                position.map(|p| p.options.orientation),
                measured.get(),
            )
        })
        // NSH has no pseudo-elements: before is the first, noninteractive child.
        .child(
            view()
                .app_class(move || {
                    Some(before_classes(
                        variant,
                        size,
                        before_pressed(),
                        before_disabled(),
                        position,
                    ))
                })
                .accessible(false),
        )
        // Descendant SVG selectors are applied directly to the decorative image slot.
        .child(glyph.map(|glyph| {
            image(glyph.asset())
                .app_class(move || {
                    Some(tw(&cn([
                        Some("-mx-0.5 pointer-events-none shrink-0"),
                        Some(if size == Size::Compact {
                            "size-3.5"
                        } else {
                            "size-4.5 sm:size-4"
                        }),
                        Some(if variant == Variant::Ghost && glyph_disabled() {
                            "opacity-100"
                        } else {
                            "opacity-80"
                        }),
                    ])))
                })
                .group_hover_named("toggle", |s| {
                    s.tint_color(move || {
                        if variant == Variant::Segmented {
                            palette().foreground
                        } else {
                            icon_color(variant, hover_pressed(), hover_disabled())
                        }
                    })
                })
                .tint_color(move || icon_color(variant, pressed(), disabled()))
                .accessible(false)
        }))
        .child(show_label.then(|| text(SharedString::from(label))))
        .child(children)
}

fn icon_color(variant: Variant, pressed: bool, disabled: bool) -> AdaptiveColor {
    match variant {
        Variant::Pill | Variant::Segmented if pressed => palette().foreground,
        Variant::Pill | Variant::Segmented => palette().muted_foreground,
        Variant::Ghost if disabled => palette().muted_foreground,
        _ if pressed => palette().accent_foreground,
        _ => palette().foreground,
    }
}

fn toggle_classes(
    variant: Variant,
    size: Size,
    pressed: bool,
    disabled: bool,
    position: Option<Position>,
) -> SharedString {
    let attrs = [("data-pressed", "")];
    let classes = cn([
        // Before/after and SVG selectors are ported above.
        Some(
            "group/toggle relative inline-flex shrink-0 cursor-pointer select-none items-center justify-center gap-2 whitespace-nowrap rounded-lg border font-medium text-base text-foreground outline-none transition-shadow hover:bg-accent focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background disabled:pointer-events-none disabled:opacity-64 aria-disabled:opacity-64 data-pressed:bg-input/64 data-pressed:text-accent-foreground sm:text-sm",
        ),
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
            Variant::Pill => {
                "border-transparent rounded-full px-3 font-normal text-muted-foreground data-pressed:text-foreground"
            }
            Variant::Ghost => {
                "border-transparent text-foreground shadow-none data-pressed:bg-accent data-pressed:text-accent-foreground disabled:opacity-100 disabled:text-muted-foreground"
            }
            // The outline's conditional pseudo-shadows live on before; background clipping is
            // light-only in CSS and has no effect on NSH's separately painted border.
            Variant::Outline => {
                "border-input bg-background shadow-xs/5 dark:bg-input/32 dark:data-pressed:bg-input dark:hover:bg-input/64"
            }
            Variant::Segmented => {
                "border-transparent text-muted-foreground shadow-none transition-colors hover:bg-background/55 hover:text-foreground data-pressed:bg-background data-pressed:text-foreground data-pressed:shadow-xs/10 dark:hover:bg-input/32 dark:data-pressed:bg-input/72"
            }
        }),
        (variant == Variant::Outline && (disabled || pressed)).then_some("shadow-none"),
        (variant == Variant::Outline).then_some("active:shadow-none"),
        position.map(|_| "focus-visible:z-10"),
        joined_classes(position, false),
    ]);
    tw_with(Scope::Root, &classes, if pressed { &attrs } else { &[] })
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
        (Orientation::Horizontal, false, false, false) => {
            "rounded-s-none rounded-e-none border-s-0 border-e-0"
        }
        (Orientation::Vertical, true, false, false) => "rounded-b-none border-b-0",
        (Orientation::Vertical, false, true, false) => "rounded-t-none border-t-0",
        (Orientation::Vertical, false, false, false) => {
            "rounded-t-none rounded-b-none border-t-0 border-b-0"
        }
        (Orientation::Horizontal, true, false, true) => "-end-[0.5px] rounded-e-none",
        (Orientation::Horizontal, false, true, true) => "-start-[0.5px] rounded-s-none",
        (Orientation::Horizontal, false, false, true) => {
            "-start-[0.5px] -end-[0.5px] rounded-s-none rounded-e-none"
        }
        (Orientation::Vertical, true, false, true) => {
            "-bottom-[0.5px] rounded-b-none hidden dark:block"
        }
        (Orientation::Vertical, false, true, true) => "-top-[0.5px] rounded-t-none dark:hidden",
        (Orientation::Vertical, false, false, true) => {
            "-top-[0.5px] -bottom-[0.5px] rounded-t-none rounded-b-none hidden"
        }
    })
}

fn before_classes(
    variant: Variant,
    size: Size,
    pressed: bool,
    disabled: bool,
    position: Option<Position>,
) -> SharedString {
    let radius = theme::current_tokens().number("radius").unwrap_or(8.0);
    let radius = (radius
        - if matches!(size, Size::Compact | Size::Segmented) {
            3.0
        } else {
            1.0
        })
    .max(0.0);
    let radius_class = format!("rounded-[{radius}px]");
    tw(&cn([
        Some("pointer-events-none absolute inset-0"),
        Some(&radius_class),
        // --theme colour shadows are explicit CSS equivalents, with the same zero blur.
        (variant == Variant::Outline && !pressed && !disabled).then_some("shadow-[0_1px_0_rgb(0_0_0/4%)] dark:shadow-[0_-1px_0_rgb(255_255_255/2%)] group-active/toggle:shadow-none dark:group-active/toggle:shadow-[0_-1px_0_rgb(255_255_255/2%)]"),
        joined_classes(position, true),
    ]))
}

fn hit_slop(
    size: Size,
    group: Option<Orientation>,
    measured: nsh_core::Size<Points>,
) -> EdgeInsets {
    let m = metrics();
    let height = match size {
        Size::Compact => 28.0,
        Size::Default => m.control,
        Size::Lg => m.control_lg,
        Size::Segmented => 24.0,
        Size::Sm => m.control_sm,
        Size::Xs => m.control_xs,
    };
    let horizontal = ((m.hit_target
        - if measured.width.0 > 0.0 {
            measured.width.0
        } else {
            height
        })
        / 2.0)
        .max(0.0);
    let vertical = ((m.hit_target
        - if measured.height.0 > 0.0 {
            measured.height.0
        } else {
            height
        })
        / 2.0)
        .max(0.0);
    EdgeInsets {
        top: px(if group == Some(Orientation::Vertical) {
            0.0
        } else {
            vertical
        }),
        bottom: px(if group == Some(Orientation::Vertical) {
            0.0
        } else {
            vertical
        }),
        left: px(if group == Some(Orientation::Horizontal) {
            0.0
        } else {
            horizontal
        }),
        right: px(if group == Some(Orientation::Horizontal) {
            0.0
        } else {
            horizontal
        }),
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
    let size = options
        .size
        .unwrap_or(if options.variant == Variant::Segmented {
            Size::Segmented
        } else {
            Size::Default
        });
    let len = children.len();
    let hovered: Vec<_> = (0..len).map(|_| signal(false)).collect();
    let neighbor = |index: usize| match children.get(index) {
        Some(GroupChild::Item(item)) => Some((item.value.clone(), hovered[index])),
        _ => None,
    };
    let neighbors: Vec<_> = (0..len)
        .map(|i| [i.checked_sub(1).and_then(neighbor), neighbor(i + 1)])
        .collect();
    let children: Vec<AnyElement> = children
        .into_iter()
        .enumerate()
        .map(|(index, child)| match child {
            GroupChild::Item(item) => toggle_group_item(
                item,
                size,
                value.clone(),
                disabled.clone(),
                on_change.clone(),
                Position {
                    options,
                    first: index == 0,
                    last: index + 1 == len,
                    hovered: hovered[index],
                },
            ),
            GroupChild::Separator => {
                group_separator(options, value.clone(), neighbors[index].clone())
            }
        })
        .collect();
    // NSH's ToggleGroup has neither label nor orientation props: a named group wraps its
    // native toolbar, whose roving focus accepts arrows on both axes.
    let group = nsh_base::toggle_group()
        .disabled(move || group_disabled())
        .app_class(tw(&cn([
            Some("flex w-fit"),
            // The TSX only adds flex-col for joined variants, even with vertical orientation.
            (options.orientation == Orientation::Vertical
                && !matches!(options.variant, Variant::Default | Variant::Segmented))
            .then_some("flex-col"),
            Some(match options.variant {
                Variant::Segmented => "gap-0.5 rounded-lg bg-input/40 p-0.5",
                Variant::Default => "gap-0.5",
                _ => "",
            }),
        ])))
        .children(children);
    view()
        .role(AccessibilityRole::Group)
        .label(SharedString::from(label))
        .app_class(tw("w-fit"))
        .child(group)
}

/// Port of toggle-group.tsx Toggle (also exported there as ToggleGroupItem).
fn toggle_group_item(
    item: GroupItem,
    size: Size,
    value: Rc<dyn Fn() -> Vec<String>>,
    disabled: Rc<dyn Fn() -> bool>,
    on_change: Rc<dyn Fn(Vec<String>)>,
    position: Position,
) -> AnyElement {
    let options = position.options;
    let item_value = item.value;
    let pressed_value = item_value.clone();
    let read = value.clone();
    let item_disabled = item.disabled;
    let disabled: Rc<dyn Fn() -> bool> = Rc::new(move || disabled() || item_disabled.get());
    let event_disabled = disabled.clone();
    toggle_inner(
        item.content,
        item.variant.unwrap_or(options.variant),
        item.size.unwrap_or(size),
        Rc::new(move || read().contains(&pressed_value)),
        disabled,
        move |pressed| {
            if event_disabled() {
                return;
            }
            on_change(next_value(value(), &item_value, pressed, options.multiple));
        },
        Some(position),
    )
    .into_element()
}

fn next_value(mut value: Vec<String>, item: &str, pressed: bool, multiple: bool) -> Vec<String> {
    if !multiple {
        return if pressed {
            vec![item.to_owned()]
        } else {
            Vec::new()
        };
    }
    if !pressed {
        if let Some(index) = value.iter().position(|v| v == item) {
            value.remove(index);
        }
        return value;
    }
    if !value.iter().any(|v| v == item) {
        value.push(item.to_owned());
    }
    value
}

fn group_separator(
    options: GroupOptions,
    value: Rc<dyn Fn() -> Vec<String>>,
    neighbors: [Option<(String, Signal<bool>)>; 2],
) -> AnyElement {
    // NSH has no Separator accessibility role; the perpendicular divider is decorative.
    view()
        .accessible(false)
        .app_class(tw(&cn([
            Some("relative shrink-0 bg-border"),
            Some(if options.orientation == Orientation::Horizontal {
                "w-px self-stretch"
            } else {
                "h-px w-full"
            }),
        ])))
        // The group's sibling has/hover selectors resolve from adjacent toggle state.
        .child(
            view()
                .app_class(move || {
                    Some(tw(&cn([
                        Some("pointer-events-none absolute inset-0"),
                        neighbors
                            .iter()
                            .flatten()
                            .any(|(_, hovered)| hovered.get())
                            .then_some("dark:bg-input/64"),
                        neighbors
                            .iter()
                            .flatten()
                            .any(|(item, _)| value().contains(item))
                            .then_some("dark:bg-input"),
                    ])))
                })
                .accessible(false),
        )
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_selection_replaces_clears_and_preserves_multiple_value_order() {
        assert_eq!(next_value(vec!["a".into()], "b", true, false), ["b"]);
        assert!(next_value(vec!["a".into()], "a", false, false).is_empty());
        assert!(next_value(vec!["a".into(), "b".into()], "a", false, false).is_empty());
        assert_eq!(next_value(vec!["b".into()], "a", true, true), ["b", "a"]);
        assert_eq!(
            next_value(vec!["b".into(), "a".into()], "b", false, true),
            ["a"]
        );
        assert_eq!(next_value(vec!["a".into()], "a", true, true), ["a"]);
        assert_eq!(
            next_value(vec!["a".into(), "a".into()], "a", false, true),
            ["a"]
        );
    }

    #[component]
    fn ClassChecks() -> impl IntoElement {
        theme::install(&theme::ThemeChoice::default());
        let off = toggle_classes(Variant::Pill, Size::Default, false, false, None);
        let on = toggle_classes(Variant::Pill, Size::Default, true, false, None);
        assert!(!off.split_whitespace().any(|s| s == "bg-input/64"));
        assert!(on.split_whitespace().any(|s| s == "bg-input/64"));
        assert!(!off.ends_with("text-foreground"));
        assert!(on.ends_with("text-foreground"));
        let tokens = theme::tokens_for(&theme::ThemeChoice::default());
        let mut theme = nsh_core::tailwind::Theme::default();
        theme.set_tokens(&tokens);
        for variant in [
            Variant::Default,
            Variant::Pill,
            Variant::Ghost,
            Variant::Outline,
            Variant::Segmented,
        ] {
            for size in [
                Size::Compact,
                Size::Default,
                Size::Lg,
                Size::Segmented,
                Size::Sm,
                Size::Xs,
            ] {
                for pressed in [false, true] {
                    for disabled in [false, true] {
                        let classes = toggle_classes(variant, size, pressed, disabled, None);
                        let parsed = nsh_core::tailwind::parse(&theme, &classes);
                        assert!(
                            parsed.unknown().is_empty(),
                            "{variant:?}/{size:?}: {:?}",
                            parsed.unknown()
                        );
                        for orientation in [Orientation::Horizontal, Orientation::Vertical] {
                            for (first, last) in
                                [(true, true), (true, false), (false, true), (false, false)]
                            {
                                let position = Some(Position {
                                    options: GroupOptions {
                                        variant,
                                        orientation,
                                        ..Default::default()
                                    },
                                    first,
                                    last,
                                    hovered: signal(false),
                                });
                                for classes in [
                                    toggle_classes(variant, size, pressed, disabled, position),
                                    before_classes(variant, size, pressed, disabled, position),
                                ] {
                                    let parsed = nsh_core::tailwind::parse(&theme, &classes);
                                    assert!(
                                        parsed.unknown().is_empty(),
                                        "joined {variant:?}/{size:?}: {:?}",
                                        parsed.unknown()
                                    );
                                }
                            }
                        }
                        let before = before_classes(variant, size, pressed, disabled, None);
                        let parsed = nsh_core::tailwind::parse(&theme, &before);
                        assert!(
                            parsed.unknown().is_empty(),
                            "before {variant:?}/{size:?}: {:?}",
                            parsed.unknown()
                        );
                    }
                }
            }
        }
        view().test_id("toggle-class-checks-passed")
    }

    #[test]
    fn pressed_presence_and_every_resolved_toggle_and_joined_variant_parse() {
        let mut app =
            nsh_testing::Headless::start(nsh_engine::EngineConfig::new(nsh_engine::Root::of::<
                ClassChecks,
            >()))
            .expect("class engine");
        app.first_render()
            .expect("all class assertions pass under the engine's token scope");
        assert_eq!(app.screen().count("id:toggle-class-checks-passed"), Ok(1));
    }
}
