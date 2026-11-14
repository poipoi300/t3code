//! Port of apps/web/src/components/ui/select.tsx Select and its slots.
//!
//! [`select`] takes [`SelectProps`] and an `on_change(Option<SharedString>)` callback.
//! Values are opaque IDs; item titles, groups, label and placeholder are English translation
//! keys. `value` and `open` support controlled bindings; defaults are used only when uncontrolled.
//! [`Variant`] is `Default | Ghost`; [`Size`] is `Compact | Default | Lg | Sm | Xs`.
//! Primitives return opaque elements: put layout on their parent, never restyle their surface.
//!
//! Slots: `trigger`, `icon`, `render_item`, `render_group` and `empty` are optional factories.
//! The default trigger contains [`select_value`] then the chevron; custom triggers can compose
//! [`select_trigger`]. [`select_button`] is the standalone button with that same default look.
//! NSH owns the portal, positioner, virtual list and group containers; [`select_popup`] applies
//! their styles through NSH's part props instead of accepting separately mounted children.
//! `SelectContent` is the web alias of `SelectPopup`: [`select_content`] is the same function.
//! NSH uses a sheet on touch screens and its own placement/selected-item alignment policy.
//! The ghost chevron inherits the trigger's text color, including hover/open state.
//! Custom item/icon renderers own their children's size and tint (the TSX's SVG selectors).
//! Raw `glass-opacity` uses percentage points (80); future typed `glass().opacity` is [0,1].

use std::rc::Rc;

pub use nsh_base::{OpenChangeReason, SelectEntry, SelectGroup, SelectItem};
use nsh_core::prelude::*;

use crate::i18n::t;
use crate::theme::{self, Scope, cn, palette, tw, tw_with};
use crate::ui::icon::Icon;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Variant {
    #[default]
    Default,
    Ghost,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    Compact,
    #[default]
    Default,
    Lg,
    Sm,
    Xs,
}

pub type ElementSlot = Rc<dyn Fn() -> AnyElement>;
pub type ItemSlot = Rc<dyn Fn(&SelectEntry) -> AnyElement>;
pub type GroupSlot = Rc<dyn Fn(&SelectGroup) -> AnyElement>;
pub type OpenChange = Rc<dyn Fn(bool, OpenChangeReason)>;
pub use nsh_base::SelectValue as Value;

/// NSH fixes desktop dropdown width to the trigger, rather than a content-sized minimum;
/// touch sheets fill the viewport. Select has no custom-anchor or selected-item alignment prop.
#[derive(Clone, Copy, Debug)]
pub struct Popup {
    pub side: Placement,
    pub align: Align,
    pub side_offset: f32,
}

impl Default for Popup {
    fn default() -> Self {
        Self {
            side: Placement::Bottom,
            align: Align::Start,
            side_offset: 4.0,
        }
    }
}

/// Create under an NSH owner with [`SelectProps::new`], then replace reactive props with
/// `signal.into()` or `Prop::bind`. Item values must be unique (the NSH select contract).
pub struct SelectProps {
    pub items: Prop<Vec<SelectItem>>,
    pub value: Prop<Value>,
    pub default_value: Option<SharedString>,
    pub open: Prop<Option<bool>>,
    pub default_open: bool,
    pub disabled: Prop<bool>,
    pub invalid: Prop<bool>,
    pub label: Prop<SharedString>,
    pub placeholder: Prop<SharedString>,
    pub variant: Variant,
    pub size: Size,
    pub popup: Popup,
    pub trigger: Option<ElementSlot>,
    pub icon: Option<ElementSlot>,
    pub render_item: Option<ItemSlot>,
    pub render_group: Option<GroupSlot>,
    pub empty: Option<ElementSlot>,
    pub on_open_change: Option<OpenChange>,
}

impl SelectProps {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            items: Vec::new().into(),
            value: Value::Uncontrolled.into(),
            default_value: None,
            open: None.into(),
            default_open: false,
            disabled: false.into(),
            invalid: false.into(),
            label: label.into().into(),
            placeholder: SharedString::default().into(),
            variant: Variant::Default,
            size: Size::Default,
            popup: Popup::default(),
            trigger: None,
            icon: None,
            render_item: None,
            render_group: None,
            empty: None,
            on_open_change: None,
        }
    }
}

pub fn select(
    props: SelectProps,
    on_change: impl Fn(Option<SharedString>) + 'static,
) -> impl IntoElement {
    select_popup(props, on_change)
}

pub use select_popup as select_content;

/// NSH combines Portal/Positioner/Popup/List. Its list refs and scroll/viewability callbacks are
/// private and SelectProps has no arrow slots, so overflow-driven arrows cannot be wired here.
pub fn select_popup(
    props: SelectProps,
    on_change: impl Fn(Option<SharedString>) + 'static,
) -> impl IntoElement {
    let SelectProps {
        items,
        value,
        default_value,
        open,
        default_open,
        disabled,
        invalid,
        label,
        placeholder,
        variant,
        size,
        popup,
        trigger,
        icon: custom_icon,
        render_item,
        render_group,
        empty,
        on_open_change,
    } = props;
    let own_value = signal(default_value.clone());
    let own_open = signal(default_open);
    let current = computed(move || match value.get() {
        Value::Uncontrolled => own_value.get(),
        Value::Controlled(value) => value,
    });
    let pressed = Prop::bind(move || open.get().unwrap_or_else(|| own_open.get()));
    let translated = computed(move || {
        items
            .get()
            .into_iter()
            .map(|mut item| {
                item.title = t(&item.title).into();
                item.group = item.group.map(|group| t(&group).into());
                item
            })
            .collect::<Vec<_>>()
    });
    let trigger = trigger.map_or_else(
        || {
            select_trigger(
                select_value(items, current.into(), placeholder),
                variant,
                size,
                disabled,
                invalid,
                pressed,
                custom_icon.map(|render| render()),
            )
            .into_element()
        },
        |render| render(),
    );
    let change = Rc::new(on_change);
    let rendered_item = render_item;
    let rendered_group = render_group;
    let empty = empty.map(|render| render());
    nsh_base::select()
        .items(translated)
        .value(move || Value::Controlled(current.get()))
        .default_value(default_value)
        .open(open)
        .default_open(default_open)
        .disabled(disabled)
        .accessibility_label(move || Some(SharedString::from(t(&label.get()))))
        .placeholder(move || Some(SharedString::from(t(&placeholder.get()))))
        .trigger(trigger)
        .placement(popup.side)
        .align(popup.align)
        .offset(px(popup.side_offset))
        // `dropdown-glass` (index.css): the 18% outer mix gives 83.6% opacity at 80% glass.
        // NSH combines the positioner, popup and glass wrapper. Its positioning exposes no
        // --transform-origin CSS variable; Electron no-drag CSS has no native-shell equivalent.
        .popup_class(tw("z-[130] select-none relative rounded-lg text-foreground outline-none border backdrop-blur-(--glass-blur) dark:backdrop-blur-[16px] backdrop-saturate-[1.14] dark:backdrop-saturate-[1.08] shadow-[0_16px_40px_-18px_rgb(0_0_0/55%)] dark:shadow-[0_18px_44px_-18px_rgb(0_0_0/80%)]"))
        .popup_style(|style| style
            .bg(move || palette().popover.opacity(glass_alpha(theme::current_tokens().number("glass-opacity"))))
            .border_color(move || theme::current_tokens().color("contrast-foreground")
                .unwrap_or(palette().foreground).opacity(0.1)))
        .list_class(tw("p-1"))
        // NSH projects data-selected/highlighted/disabled into selected:/focus:/disabled:.
        // `in-data-[side=none]` width is NSH's selected-item cover geometry, not a CSS token.
        .item_class(tw("group flex min-h-8 cursor-pointer items-center rounded-sm px-2 py-1 text-base outline-none selected:bg-foreground/[0.08] disabled:pointer-events-none disabled:cursor-not-allowed focus:bg-accent focus:text-accent-foreground disabled:opacity-64 sm:min-h-7 sm:text-sm"))
        .render_item(move |entry: &SelectEntry| {
            select_item(rendered_item.as_ref().map_or_else(
                || text(entry.title.clone()).into_element(), |render| render(entry)
            ), false)
        })
        .render_group(move |group: &SelectGroup| {
            select_group(rendered_group.as_ref().map_or_else(
                || group_label(group.title.clone()).into_element(), |render| render(group)
            ))
        })
        .when_some(empty, |select, empty| select.empty(empty))
        .on_change(move |next: &Option<SharedString>| {
            if disabled.get_untracked() { return; }
            if matches!(value.get_untracked(), Value::Uncontrolled) { own_value.set(next.clone()); }
            change(next.clone());
        })
        .on_open_change(move |event: &nsh_base::SelectEvent| {
            if let nsh_base::SelectEvent::OpenChange(next, reason) = event {
                own_open.set(*next);
                if let Some(callback) = &on_open_change { callback(*next, *reason); }
            }
        })
}

fn glass_alpha(raw_percentage: Option<f32>) -> f32 {
    // Raw token units are percentage points; normalize once, as agreed with UI-MENU/UI-POPOVER.
    0.18 + 0.82 * (raw_percentage.unwrap_or(80.0) / 100.0)
}

fn trigger_classes(
    variant: Variant,
    size: Size,
    disabled: bool,
    invalid: bool,
    pressed: bool,
) -> SharedString {
    // Selector utilities are applied to the icon below. `before:` becomes the first child;
    // coarse pointer `after:` hit area is `hit_slop`, and spacing calculations are 4px units.
    // NSH has no `bg-clip-padding`; its border paints over the background on every appearance.
    let classes = cn([
        Some(
            "group relative inline-flex cursor-pointer select-none items-center justify-between gap-2 border rounded-lg text-left text-base outline-none data-disabled:pointer-events-none data-disabled:opacity-64 sm:text-sm",
        ),
        Some(match variant {
            Variant::Default => {
                "w-full min-w-36 border-input bg-background text-foreground shadow-xs/5 ring-ring/24 focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-inset aria-invalid:border-destructive/36 focus-visible:aria-invalid:border-destructive/64 focus-visible:aria-invalid:ring-destructive/16 dark:bg-input/32 dark:aria-invalid:ring-destructive/24"
            }
            Variant::Ghost => {
                "border-transparent text-secondary-label focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring data-pressed:bg-accent hover:bg-accent hover:text-foreground data-pressed:text-foreground"
            }
        }),
        Some(match size {
            Size::Compact => "h-7 gap-1 rounded-md px-[7px] text-xs",
            Size::Default => "min-h-9 px-[11px] sm:min-h-8",
            Size::Lg => "min-h-10 px-[11px] sm:min-h-9",
            Size::Sm => "min-h-8 gap-1.5 px-[9px] sm:min-h-7",
            Size::Xs => "h-7 gap-1 rounded-md px-[7px] text-sm sm:h-6 sm:text-xs",
        }),
        (variant == Variant::Default).then_some("focus-visible:shadow-none"),
        (variant == Variant::Default && (disabled || invalid || pressed)).then_some("shadow-none"),
    ]);
    let attrs: Vec<_> = [
        disabled.then_some(("data-disabled", "")),
        invalid.then_some(("aria-invalid", "true")),
        pressed.then_some(("data-pressed", "")),
    ]
    .into_iter()
    .flatten()
    .collect();
    tw_with(Scope::Root, &classes, &attrs)
}

fn trigger_transition() -> nsh_core::element::Style {
    use nsh_core::{PropertyValue, StyleKey, TransitionProps};
    // NSH rejects the TSX's `transition-[color,box-shadow,background-color]` class.
    let properties = if reduce_motion() {
        0
    } else {
        TransitionProps::COLORS | TransitionProps::SHADOW
    };
    nsh_core::element::Style::default().with(
        StyleKey::TransitionProperty,
        PropertyValue::TransitionProps(TransitionProps(properties)),
    )
}

/// Select's trigger slot. NSH supplies its combobox role, name, expanded state and press handler.
pub fn select_trigger(
    children: impl IntoElement,
    variant: Variant,
    size: Size,
    disabled: Prop<bool>,
    invalid: Prop<bool>,
    pressed: Prop<bool>,
    custom_icon: Option<AnyElement>,
) -> impl IntoElement {
    // Select binds its combobox behavior onto this slot; NSH ButtonProps cannot carry `invalid`.
    view()
        .disabled(disabled)
        .inline_style(trigger_transition)
        .accessibility_state(move || AccessibilityState {
            invalid: invalid.get(),
            ..Default::default()
        })
        .app_class(move || {
            Some(trigger_classes(
                variant,
                size,
                disabled.get(),
                invalid.get(),
                pressed.get(),
            ))
        })
        .hit_slop(move || {
            let m = theme::metrics();
            EdgeInsets::all(px(((m.hit_target
                - trigger_height(size, m.control > 32.0))
                / 2.0)
                .max(0.0)))
        })
        .child(
            (variant == Variant::Default).then(|| trigger_relief(size, disabled, invalid, pressed)),
        )
        .child(children)
        .child(
            view()
                .accessible(false)
                .app_class(tw("shrink-0"))
                .child(custom_icon.unwrap_or_else(|| default_icon(variant).into_element())),
        )
}

fn trigger_relief(
    size: Size,
    disabled: Prop<bool>,
    invalid: Prop<bool>,
    pressed: Prop<bool>,
) -> impl IntoElement {
    // `before:` relief is visible only in the idle state; focus is projected to children.
    view().accessible(false).pointer_events(PointerEvents::None)
        .app_class(tw("absolute inset-0 group-focus-visible:opacity-0 shadow-[0_1px_rgb(0_0_0/4%)] dark:shadow-[0_-1px_rgb(255_255_255/6%)]"))
        .rounded(move || {
            let small = matches!(size, Size::Compact | Size::Xs);
            px(theme::current_tokens().number(if small { "radius-md" } else { "radius-lg" }).unwrap_or(if small { 6.0 } else { 8.0 }) - 1.0)
        })
        .opacity(move || if disabled.get() || invalid.get() || pressed.get() { 0.0_f32 } else { 1.0_f32 })
}

fn default_icon(variant: Variant) -> impl IntoElement {
    svg(Icon::ChevronDown.asset())
        .accessible(false)
        .app_class(tw(&cn([
            Some("-me-1 size-3.5 shrink-0 opacity-70 pointer-events-none"),
            (variant == Variant::Default).then_some("text-icon-muted"),
        ])))
}

fn trigger_height(size: Size, compact: bool) -> f32 {
    match (size, compact) {
        (Size::Compact, _) | (Size::Xs, true) | (Size::Sm, false) => 28.0,
        (Size::Xs, false) => 24.0,
        (Size::Default, true) | (Size::Lg, false) => 36.0,
        (Size::Default, false) | (Size::Sm, true) => 32.0,
        (Size::Lg, true) => 40.0,
    }
}

/// Standalone SelectButton. NSH's typed button replaces `useRender`'s arbitrary tag override.
pub fn select_button(
    children: impl IntoElement,
    label: impl Into<SharedString>,
    size: Size,
    disabled: Prop<bool>,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let label = label.into();
    let on_press = Rc::new(on_press);
    nsh_base::button()
        .accessibility_label(move || Some(SharedString::from(t(&label))))
        .disabled(disabled)
        .inline_style(trigger_transition)
        .app_class(move || {
            Some(trigger_classes(
                Variant::Default,
                size,
                disabled.get(),
                false,
                false,
            ))
        })
        .hit_slop(move || {
            let m = theme::metrics();
            EdgeInsets::all(px(((m.hit_target
                - trigger_height(size, m.control > 32.0))
                / 2.0)
                .max(0.0)))
        })
        .on_press(move || {
            if !disabled.get_untracked() {
                on_press();
            }
        })
        .child(trigger_relief(size, disabled, false.into(), false.into()))
        .child(
            view()
                .app_class(tw("min-w-0 flex-1 truncate text-left"))
                .child(children),
        )
        .child(default_icon(Variant::Default))
}

pub fn select_value(
    items: Prop<Vec<SelectItem>>,
    value: Prop<Option<SharedString>>,
    placeholder: Prop<SharedString>,
) -> impl IntoElement {
    let title = computed(move || {
        value.get().filter(|value| !value.is_empty()).map(|value| {
            items.with(|items| {
                items
                    .iter()
                    .find(|item| item.value == value)
                    .map_or(value, |item| item.title.clone())
            })
        })
    });
    text(move || SharedString::from(t(&title.get().unwrap_or_else(|| placeholder.get()))))
        .app_class(move || {
            let attrs: Vec<_> = title
                .get()
                .is_none()
                .then_some(("data-placeholder", ""))
                .into_iter()
                .collect();
            Some(tw_with(
                Scope::Root,
                "flex-1 truncate data-placeholder:text-placeholder",
                &attrs,
            ))
        })
}

/// NSH supplies the surrounding option's behavior and state. Web's `hideIndicator` is unused.
pub fn select_item(children: impl IntoElement, _hide_indicator: bool) -> impl IntoElement {
    view().app_class(tw("min-w-0 flex-1")).child(children)
}

/// NSH supplies the group container; its renderer mounts this group's label once per section.
pub fn select_group(children: impl IntoElement) -> impl IntoElement {
    view().child(children)
}

pub fn select_group_label(label: impl Into<SharedString>) -> impl IntoElement {
    let label = label.into();
    group_label(Prop::bind(move || SharedString::from(t(&label))))
}

fn group_label(label: impl Into<Prop<SharedString>>) -> impl IntoElement {
    let label = label.into();
    text(move || label.get()).app_class(tw("px-2 py-1.5 font-medium text-muted-foreground text-xs"))
}

#[cfg(test)]
mod tests {
    use super::glass_alpha;

    #[test]
    fn raw_glass_opacity_uses_percentage_points_and_is_normalized_once() {
        for (raw, expected) in [
            (None, 0.836),
            (Some(0.0), 0.18),
            (Some(0.8), 0.18656),
            (Some(50.0), 0.59),
            (Some(80.0), 0.836),
            (Some(100.0), 1.0),
        ] {
            assert!((glass_alpha(raw) - expected).abs() < 0.000001, "{raw:?}");
        }
    }
}
