//! Port of apps/web/src/components/ui/menu.tsx Menu and its slots.
//!
//! Public composition: `menu(trigger, options, popup)` / `context_menu(trigger, options,
//! disabled, popup)` own NSH's popup lifecycle; `menu_popup(children)` is its padded content.
//! `menu_button` is the same shared menu with a button trigger, including on web and Android.
//! Build filtered/dynamic popup trees with `menu_popup_with(|| ...)`: NSH binds regions at
//! construction, before an eager child can inherit its mounted menu context.
//! `MenuOptions` controls the open signal, placement and `Width::{Default, Content}`.
//! Rows take `ItemOptions`: `Variant::{Default, Destructive, Ghost}`, `Density::{Default, Touch}`,
//! inset, reactive disabled state and close-on-select. There is no web `Size` prop: density is
//! independent of the window's `sm` breakpoint. Checkbox rows use `CheckboxVariant` instead.
//! `menu_radio_group(label, value, children, on_value_change)` controls one string value;
//! `menu_radio_item(label, value, options, children, on_select)` reads that group's selection.
//! `menu_sub(trigger, disabled, popup)` owns a submenu; `menu_sub_trigger` and `menu_sub_popup`
//! are its slots. Group, label, shortcut, detail, radio indicator and search functions mirror
//! the named TSX components. Labels and content passed by callers are already translated.
//!
//! NSH owns portals, available-height scrolling, keyboard navigation, focus return, outside
//! dismissal, long-press cancellation and the adaptive sheet with submenu Back on touch devices.
//! Popup appearance belongs on NSH's surface (not on the inner content); submenus inherit it.
//! The web's `DropdownMenu*` aliases are exported in snake case too.

use std::{cell::RefCell, rc::Rc, time::Duration};

use nsh_core::{
    PropertyValue, StyleKey, Transition, TransitionProps,
    element::{ImageSrc, OrderWatch, Renderer, Style, StyleRefinement, watch_order},
    keymap::{Key, Keystroke, Modifiers, When, shifted},
    prelude::*,
};

use crate::i18n::t;
use crate::state::layout::{Pointer, layout_class, pointer};
use crate::theme::{Scope as ThemeScope, cn, palette, tw, tw_with};
use crate::ui::icon::Icon;

pub use self::{
    menu as dropdown_menu, menu_checkbox_item as dropdown_menu_checkbox_item,
    menu_group as dropdown_menu_group, menu_group_label as dropdown_menu_label,
    menu_item as dropdown_menu_item, menu_popup as dropdown_menu_content,
    menu_radio_group as dropdown_menu_radio_group, menu_radio_item as dropdown_menu_radio_item,
    menu_separator as dropdown_menu_separator, menu_shortcut as dropdown_menu_shortcut,
    menu_sub as dropdown_menu_sub, menu_sub_popup as dropdown_menu_sub_content,
    menu_sub_trigger as dropdown_menu_sub_trigger, menu_trigger as dropdown_menu_trigger,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PropEnum)]
pub enum Variant {
    #[default]
    Default,
    Destructive,
    Ghost,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PropEnum)]
pub enum Density {
    #[default]
    Default,
    Touch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PropEnum)]
pub enum CheckboxVariant {
    #[default]
    Default,
    Icon,
    Switch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Width {
    #[default]
    Default,
    Content,
}

#[derive(Clone, Copy, Debug)]
pub struct MenuOptions {
    /// `None` lets the primitive own its open state; otherwise the caller can open/close it too.
    pub open: Option<Signal<bool>>,
    pub width: Width,
    pub side: Placement,
    pub align: Align,
    pub side_offset: f32,
}

impl Default for MenuOptions {
    fn default() -> Self {
        Self {
            open: None,
            width: Width::Default,
            side: Placement::Bottom,
            align: Align::Center,
            side_offset: 4.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemOptions {
    pub variant: Variant,
    pub density: Density,
    pub inset: bool,
    pub disabled: Prop<bool>,
    pub close_on_select: bool,
}

impl Default for ItemOptions {
    fn default() -> Self {
        Self {
            variant: Variant::Default,
            density: Density::Default,
            inset: false,
            disabled: false.into(),
            close_on_select: true,
        }
    }
}

/// Menu root. Pass `menu_trigger(...)` or an existing translated button as `trigger`.
pub fn menu(
    trigger: impl IntoElement,
    options: MenuOptions,
    popup: impl IntoElement,
) -> impl IntoElement {
    let open = options.open.unwrap_or_else(|| signal(false));
    nsh_base::menu()
        .open(move || Some(open.get()))
        .on_open_change(move |event: &nsh_base::MenuEvent| {
            if let nsh_base::MenuEvent::OpenChange(next, _) = event {
                open.set(*next);
            }
        })
        .placement(options.side)
        .align(options.align)
        .offset(px(options.side_offset))
        .margin(px(16.0))
        .popup_class(move || Some(popup_class(options.width)))
        .popup_style(move |s| s.style_with(move |s| popup_style(s, options.width)))
        .trigger(trigger)
        .child(
            menu_content()
                .on_close(move || open.set(false))
                .child(popup),
        )
}

/// Shared context menu: secondary click, keyboard menu action, or NSH's held-finger gesture.
pub fn context_menu(
    trigger: impl IntoElement,
    options: MenuOptions,
    disabled: impl Fn() -> bool + 'static,
    popup: impl IntoElement,
) -> impl IntoElement {
    let open = options.open.unwrap_or_else(|| signal(false));
    let disabled = Prop::bind(disabled);
    nsh_base::context_menu()
        .disabled(disabled)
        .open(move || Some(open.get() && !disabled.get()))
        .on_open_change(move |event: &nsh_base::MenuEvent| {
            if let nsh_base::MenuEvent::OpenChange(next, _) = event {
                open.set(*next && !disabled.get_untracked());
            }
        })
        .offset(px(options.side_offset))
        .margin(px(16.0))
        .popup_class(move || Some(popup_class(options.width)))
        .popup_style(move |s| s.style_with(move |s| popup_style(s, options.width)))
        .trigger(trigger)
        .child(
            menu_content()
                .on_close(move || open.set(false))
                .child(popup),
        )
}

/// An app-rendered MenuButton: NSH's system `menu_button` opens nothing on web or Android,
/// and cannot carry the TSX's row slots. Use NSH `menu` on every surface instead.
pub fn menu_button(
    label: String,
    disabled: impl Fn() -> bool + 'static,
    trigger_content: impl IntoElement,
    options: MenuOptions,
    popup: impl IntoElement,
) -> impl IntoElement {
    menu(
        menu_trigger(label, disabled, trigger_content),
        options,
        popup,
    )
}

pub fn menu_trigger(
    label: String,
    disabled: impl Fn() -> bool + 'static,
    children: impl IntoElement,
) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(Some(SharedString::from(label)))
        .disabled(disabled)
        .child(children)
}

fn popup_class(width: Width) -> SharedString {
    // `dropdown-glass` (index.css) is resolved in popup_style; native overlays replace the
    // portal/positioner and -webkit-app-region, and have no CSS transform-origin variable.
    tw(&cn([
        Some(
            "relative flex rounded-lg shadow-[0_16px_40px_-18px_rgb(0_0_0/55%)] outline-none focus:outline-none dark:shadow-[0_18px_44px_-18px_rgb(0_0_0/80%)]",
        ),
        (width == Width::Content).then_some("w-max min-w-0"),
    ]))
}

fn popup_style(style: StyleRefinement, width: Width) -> StyleRefinement {
    let room = (window().width.0 - 32.0).max(0.0);
    let tokens = tokens();
    // Raw glass-opacity is percentage points (80 means 80%); normalize exactly once.
    // The bridge omits percentages today; future theme::glass().opacity is already [0,1].
    let glass = tokens.number("glass-opacity").unwrap_or(80.0) / 100.0;
    // `dropdown-glass`: 18% popover + 82% of the glass-opacity surface. The GPUI shell's
    // missing blur is the plan's NSH problem 7.
    style
        .flex_col()
        .bg(palette().popover.opacity(0.18 + 0.82 * glass))
        // The bridge exports only light lengths; index.css's dark --glass-blur is 16px.
        .backdrop_blur(px(if dark() {
            16.0
        } else {
            tokens.number("glass-blur").unwrap_or(12.0)
        }))
        .backdrop_saturate(tokens.number("glass-saturation").unwrap_or(if dark() {
            1.08
        } else {
            1.14
        }))
        .border_1()
        .border_color(
            tokens
                .color("contrast-foreground")
                .unwrap_or(palette().foreground)
                .opacity(0.1),
        )
        // The viewport min()/calc() is measured here, including narrow-window clamping.
        .max_w(px(room))
        // NSH sheets pin both edges; keep the web popup's 1rem viewport inset on coarse pointers.
        .mx(px(if pointer() == Pointer::Coarse {
            16.0
        } else {
            0.0
        }))
        .min_w(px(if width == Width::Default {
            160.0_f32.min(room)
        } else {
            0.0
        }))
}

pub fn menu_popup(children: impl IntoElement) -> impl IntoElement {
    // NSH supplies the max-available-height scroll view outside this padded content.
    popup_content().child(children)
}

/// Construct filtered rows under the popup's context, including NSH's native menu context.
/// Eager regions are bound under their caller before NSH reparents static child components.
pub fn menu_popup_with<E: IntoElement>(children: impl Fn() -> E + 'static) -> impl IntoElement {
    popup_content().build(move |_: &bool| children())
}

#[derive(Clone, PartialEq, Props)]
struct PopupContentProps {
    children: Prop<Children>,
    build: Prop<Option<Renderer<bool>>>,
}

#[component]
fn PopupContent(props: PopupContentProps) -> impl IntoElement {
    view()
        .app_class(tw("w-full p-1"))
        .child(props.children)
        .children(
            props
                .build
                .get_untracked()
                .map(|build| build.render("menu-popup", false)),
        )
}

#[derive(Clone)]
struct SearchRows {
    rows: Rc<RefCell<Vec<SearchRow>>>,
    order: OrderWatch,
    focused: Signal<Option<ElementRef>>,
    editing: Signal<bool>,
    query: Rc<RefCell<String>>,
    reset: Signal<u64>,
    owner: Owner,
}

#[derive(Clone)]
struct SearchRow {
    node: ElementRef,
    disabled: Prop<bool>,
    label: Prop<SharedString>,
}

impl Default for SearchRows {
    fn default() -> Self {
        let rows = Rc::new(RefCell::new(Vec::<SearchRow>::new()));
        let listed = rows.clone();
        let ordered = rows.clone();
        let order = watch_order(
            move || listed.borrow().iter().map(|row| row.node).collect(),
            move |indices| {
                let mut rows = ordered.borrow_mut();
                let previous = rows.clone();
                *rows = indices.iter().map(|&i| previous[i].clone()).collect();
            },
        );
        Self {
            rows,
            order,
            focused: signal(None),
            editing: signal(false),
            query: Rc::default(),
            reset: signal(0),
            owner: Owner::current(),
        }
    }
}

impl SearchRows {
    fn add(&self, node: ElementRef, disabled: Prop<bool>, label: Prop<SharedString>) {
        self.rows.borrow_mut().push(SearchRow {
            node,
            disabled,
            label,
        });
        self.order.invalidate();
        let focused = self.focused;
        node.on_focus_in(move || focused.set(Some(node)));
        node.on_focus_out(move || {
            if focused.get_untracked() == Some(node) {
                focused.set(None);
            }
        });
        let rows = self.clone();
        cleanup(move || {
            rows.rows.borrow_mut().retain(|item| item.node != node);
            rows.order.invalidate();
        });
    }

    fn first(&self) {
        if let Some(row) = self
            .rows
            .borrow()
            .iter()
            .find(|row| !row.disabled.get_untracked())
        {
            row.node.focus();
        }
    }

    fn typeahead(&self, key: &str) {
        if self.focused.get_untracked().is_none() || self.editing.get_untracked() {
            return;
        }
        let query = {
            let mut query = self.query.borrow_mut();
            query.push_str(&key.to_lowercase());
            query.clone()
        };
        let epoch = self.reset.get_untracked().wrapping_add(1);
        self.reset.set(epoch);
        let current = self.clone();
        // Focus changes replace the key bindings' owner; the popup owns this expiry.
        self.owner.run(move || {
            set_timeout(Duration::from_millis(500), move || {
                if current.reset.get_untracked() == epoch {
                    current.query.borrow_mut().clear();
                }
            });
        });
        let rows = self.rows.borrow();
        let focused = self.focused.get_untracked();
        let at = rows.iter().position(|row| Some(row.node) == focused);
        let labels: Vec<_> = rows
            .iter()
            .map(|row| {
                (
                    row.label.get_untracked().to_string(),
                    row.disabled.get_untracked(),
                )
            })
            .collect();
        if let Some(index) = typeahead_match(&labels, &query, at) {
            rows[index].node.focus();
        }
    }
}

fn typeahead_match(rows: &[(String, bool)], query: &str, focused: Option<usize>) -> Option<usize> {
    let first = query.chars().next()?;
    let repeated = query.chars().all(|ch| ch == first);
    let query = if repeated {
        first.to_string()
    } else {
        query.to_owned()
    };
    let start = focused.map_or(0, |at| if repeated { at + 1 } else { at });
    (0..rows.len())
        .map(|offset| (start + offset) % rows.len())
        .find(|&i| !rows[i].1 && rows[i].0.to_lowercase().starts_with(&query))
}

#[derive(Clone)]
struct CloseMenu(Rc<dyn Fn()>);

#[derive(Clone, PartialEq, Props)]
struct MenuContentProps {
    children: Prop<Children>,
    #[prop(default)]
    inherit_close: Prop<bool>,
}

#[derive(Clone, Debug, PartialEq, Event)]
enum MenuContentEvent {
    Close,
}

#[component]
fn MenuContent(props: MenuContentProps, cx: Scope<MenuContentEvent>) -> impl IntoElement {
    let rows = SearchRows::default();
    provide_context(rows.clone());
    let close = props
        .inherit_close
        .get_untracked()
        .then(use_context::<CloseMenu>)
        .flatten()
        .unwrap_or_else(|| CloseMenu(Rc::new(move || cx.emit(MenuContentEvent::Close))));
    provide_context(close.clone());
    // Bind while mounted so Tab from the pointer-opened trigger dismisses too; pass it on
    // for native traversal, as NSH's Select does. Arrows and Escape remain NSH's.
    // NSH's programmatic menu close restores the trigger; preserving a moved Tab destination
    // and dismissing on programmatic focus leave require its Focus close path upstream.
    for key in ["tab", "shift+tab"] {
        let close = close.clone();
        on_key_with(
            key,
            KeyOptions {
                when: When::Always,
                pass_through: true,
                ..Default::default()
            },
            move |_| (close.0)(),
        );
    }
    let keyed = rows.clone();
    // Raw key notifications currently reach only NSH's Linux renderer; keymap bindings
    // provide the same printable-key behaviour on the other desktop renderers and web.
    effect(move || {
        if keyed.focused.get().is_some() && !keyed.editing.get() {
            for key in r"0123456789abcdefghijklmnopqrstuvwxyz`-=[]\;',./".chars() {
                for shift in [false, true] {
                    let stroke = Keystroke {
                        key: Key::Char(key),
                        modifiers: Modifiers {
                            shift,
                            ..Default::default()
                        },
                    };
                    let typed = if shift { shifted(key) } else { key }.to_string();
                    let rows = keyed.clone();
                    on_key_with(
                        &stroke.to_string(),
                        KeyOptions {
                            when: When::NotEditing,
                            ..Default::default()
                        },
                        move |_| rows.typeahead(&typed),
                    );
                }
            }
        }
    });
    view()
        .on_key_down(move |event: &KeyEvent| {
            let key = event.key.as_ref();
            if !key.is_ascii()
                && key.chars().count() == 1
                && !event.modifiers.ctrl
                && !event.modifiers.alt
                && !event.modifiers.cmd
            {
                rows.typeahead(key);
            }
        })
        .child(props.children)
}

pub fn menu_search_input(
    value: Signal<String>,
    placeholder: String,
    label: String,
    busy: impl Fn() -> bool + 'static,
) -> impl IntoElement {
    search_input()
        .value(move || SharedString::from(value.get()))
        .on_change(move |next: &SharedString| {
            value.set(next.to_string());
        })
        .placeholder(placeholder)
        .label(label)
        .busy(busy)
}

#[derive(Clone, PartialEq, Props)]
struct SearchInputProps {
    value: Prop<SharedString>,
    placeholder: Prop<SharedString>,
    label: Prop<SharedString>,
    busy: Prop<bool>,
}

#[derive(Clone, Debug, PartialEq, Event)]
enum SearchInputEvent {
    Change(SharedString),
}

#[component]
fn SearchInput(props: SearchInputProps, cx: Scope<SearchInputEvent>) -> impl IntoElement {
    let rows = use_context::<SearchRows>().unwrap_or_default();
    let focused = signal(false);
    let editing = rows.editing;
    effect(move || {
        if focused.get() {
            let rows = rows.clone();
            on_key_with(
                "down",
                KeyOptions {
                    when: When::Always,
                    for_focus: true,
                    ..Default::default()
                },
                move |_| rows.first(),
            );
        }
    });
    view().app_class(tw("min-w-0 shrink-0 px-2 pt-1.5 pb-1")).child(
        view().app_class(tw("relative -translate-y-px border-b border-border/70 pb-1.5 transition-colors focus-within:border-ring"))
            // Busy is a static status mark: this crate forbids continuously repainting spinners.
            .child(when(move || props.busy.get(), || svg(Icon::Ellipsis.asset()).accessible(false)
                    .app_class(tw("pointer-events-none absolute top-1.5 left-0 size-4 shrink-0 text-muted-foreground")))
                .otherwise(|| svg(Icon::Search.asset()).accessible(false)
                    .app_class(tw("pointer-events-none absolute top-1.5 left-0 size-4 shrink-0 text-muted-foreground/55"))))
            .child(view().app_class(tw("rounded-none bg-transparent text-sm")).child(
                // Input's selector rules live on the native input; its frozen wrapper has no
                // unstyled/sm variant, so use the same NSH input that wrapper uses.
                nsh_base::input()
                    .value(move || props.value.get().to_string())
                    .on_change(move |value: &str| cx.emit(SearchInputEvent::Change(value.to_owned().into())))
                    .placeholder(move || props.placeholder.get().to_string())
                    .label(props.label)
                    .on_focus(move || { focused.set(true); editing.set(true); })
                    .on_blur(move || { focused.set(false); editing.set(false); })
                    .placeholder_color(move || color(palette().placeholder))
                    .app_class(tw("h-6.5 px-[9px] ps-5 font-sans leading-6.5 rounded-none w-full min-w-0 bg-transparent text-sm text-foreground outline-none"))
            ))
    )
}

pub fn menu_group(label: String, children: impl IntoElement) -> impl IntoElement {
    view()
        .role(AccessibilityRole::Group)
        .label(SharedString::from(label))
        .child(children)
}

pub fn menu_item(
    label: String,
    options: ItemOptions,
    children: impl IntoElement,
    on_select: impl Fn() + 'static,
) -> impl IntoElement {
    row_options(options)
        .label(label)
        .on_select(on_select)
        .child(children)
}

#[derive(Clone, PartialEq, Props)]
struct ItemRowProps {
    label: Prop<SharedString>,
    variant: Prop<Variant>,
    density: Prop<Density>,
    inset: Prop<bool>,
    disabled: Prop<bool>,
    #[prop(default = true)]
    close_on_select: Prop<bool>,
    #[prop(default)]
    kind: Prop<RowKind>,
    checked: Prop<Option<bool>>,
    radio_value: Prop<SharedString>,
    #[prop(default)]
    checkbox_variant: Prop<CheckboxVariant>,
    children: Prop<Children>,
}

#[derive(Clone, Debug, PartialEq, Event)]
enum ItemRowEvent {
    Select,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PropEnum)]
enum RowKind {
    #[default]
    Action,
    Checkbox,
    Radio,
}

#[component]
fn ItemRow(props: ItemRowProps, cx: Scope<ItemRowEvent>) -> impl IntoElement {
    let node = element_ref();
    let radio = use_context::<RadioSelection>();
    let has_group = radio.is_some();
    let is_radio = props.kind.get_untracked() == RowKind::Radio;
    let disabled = Prop::bind(move || props.disabled.get() || (is_radio && !has_group));
    let group = radio.clone();
    let checked = Prop::bind(move || {
        if is_radio {
            Some(
                group
                    .as_ref()
                    .is_some_and(|group| group.value.get() == props.radio_value.get()),
            )
        } else {
            props.checked.get()
        }
    });
    if is_radio {
        provide_context(RadioChecked(checked));
    }
    if let Some(rows) = use_context::<SearchRows>() {
        rows.add(node, disabled, props.label);
    }
    let item = nsh_base::menu_item()
        .element_ref(&node)
        .accessibility_label(move || Some(props.label.get()))
        .disabled(disabled)
        .checked(checked)
        .role(move || match props.kind.get() {
            RowKind::Radio => Some(AccessibilityRole::MenuItemRadio),
            _ => None,
        })
        .close_on_select(props.close_on_select)
        .inline_style(move || {
            if props.variant.get() == Variant::Ghost {
                transition_style(
                    TransitionProps::SHADOW | TransitionProps::TRANSFORM,
                    150.0,
                    [0.4, 0.0, 0.2, 1.0],
                )
            } else {
                Style::default()
            }
        })
        .on_select(move || {
            if !disabled.get_untracked() {
                if is_radio && let Some(group) = &radio {
                    let value = props.radio_value.get_untracked();
                    if group.value.get_untracked() != value {
                        (group.choose)(value);
                    }
                }
                cx.emit(ItemRowEvent::Select);
            }
        })
        .app_class(move || {
            Some(row_class(
                ItemOptions {
                    variant: props.variant.get(),
                    density: props.density.get(),
                    inset: props.inset.get(),
                    disabled,
                    close_on_select: props.close_on_select.get(),
                },
                props.kind.get(),
                props.checkbox_variant.get(),
            ))
        })
        // NSH highlights through hover/focus instead of Base UI's data-highlighted attribute.
        .hover(|s| {
            s.bg(move || palette().accent)
                .text_color(move || palette().accent_foreground)
        })
        .focus(|s| {
            s.bg(move || palette().accent)
                .text_color(move || palette().accent_foreground)
        })
        .focus_visible(|s| {
            s.ring(px(2.0))
                .ring_inset()
                .ring_color(move || palette().ring)
        })
        .checked_style(move |s| {
            if props.kind.get() == RowKind::Radio {
                s.bg(move || palette().foreground.opacity(0.08))
            } else {
                s
            }
        })
        .child(when(
            move || props.variant.get() == Variant::Ghost,
            || {
                // buttonVariants' compact before layer; NSH has no CSS pseudo-elements.
                view()
                    .accessible(false)
                    .app_class(tw("pointer-events-none absolute inset-0"))
                    .rounded(move || px(tokens().number("radius-md").unwrap_or(6.0) - 1.0))
            },
        ))
        .child(
            when(
                move || props.kind.get() == RowKind::Checkbox,
                move || checkbox_content(props.checkbox_variant, checked, disabled, props.children),
            )
            .otherwise(move || {
                if props.kind.get_untracked() == RowKind::Radio {
                    view()
                        .app_class(tw("min-w-0 flex-1"))
                        .child(props.children)
                        .into_element()
                } else {
                    props.children.into_element()
                }
            }),
        );
    hit_target()
        .insets(move || row_hit_slop(props.density.get(), props.variant.get()))
        .target(item)
}

#[derive(Clone, PartialEq, Props)]
struct HitTargetProps {
    target: Prop<Option<Slot>>,
    insets: Prop<EdgeInsets>,
}

#[component]
fn HitTarget(props: HitTargetProps) -> impl IntoElement {
    // MenuItem has no hit-slop prop; the NSH slot decorates its existing root without a wrapper.
    props
        .target
        .place(move |target| target.hit_slop(props.insets))
}

fn row_options(options: ItemOptions) -> ItemRowElement {
    item_row()
        .variant(options.variant)
        .density(options.density)
        .inset(options.inset)
        .disabled(options.disabled)
        .close_on_select(options.close_on_select)
}

fn row_class(options: ItemOptions, kind: RowKind, checkbox: CheckboxVariant) -> SharedString {
    let mut attrs = vec![];
    if options.disabled.get() {
        attrs.push(("data-disabled", ""));
    }
    if options.inset {
        attrs.push(("data-inset", ""));
    }
    if options.variant == Variant::Destructive {
        attrs.push(("data-variant", "destructive"));
    }
    let classes = cn([
        Some(match kind {
        RowKind::Action => {
            "flex min-h-8 cursor-pointer select-none items-center gap-2 rounded-sm px-2 py-1 text-base text-foreground outline-none data-disabled:pointer-events-none data-disabled:cursor-not-allowed data-inset:ps-8 data-[variant=destructive]:text-destructive-foreground data-disabled:opacity-64 sm:min-h-7 sm:text-sm"
        }
        // The side=none anchor-width rule has no equivalent in NSH: sheet widths follow the window.
        RowKind::Checkbox => {
            "group grid min-h-8 cursor-pointer items-center gap-2 rounded-sm py-1 ps-2 text-base text-foreground outline-none data-disabled:pointer-events-none data-disabled:cursor-not-allowed data-disabled:opacity-64 sm:min-h-7 sm:text-sm"
        }
        RowKind::Radio => {
            "flex min-h-8 cursor-pointer items-center rounded-sm px-2 py-1 text-base text-foreground outline-none data-disabled:pointer-events-none data-disabled:cursor-not-allowed data-disabled:opacity-64 sm:min-h-7 sm:text-sm"
        }
        }),
        (kind == RowKind::Checkbox).then_some(match checkbox {
            CheckboxVariant::Switch => "grid-cols-[1fr_auto] gap-4 pe-1.5",
            CheckboxVariant::Icon => "grid-cols-[0.625rem_minmax(0,1fr)] pe-4",
            CheckboxVariant::Default => "grid-cols-[1rem_minmax(0,1fr)] pe-4",
        }),
        // button.tsx compact: selectors style child icons; the coarse after area is row_hit_slop.
        // --spacing(2)-1px is 7px; drop will-change (native press feedback owns scheduling).
        // transition_style replaces NSH's unsupported transition-[...] utility.
        // [:hover,[data-pressed]] maps to NSH hover/active (a press in progress).
        (options.variant == Variant::Ghost).then_some("relative inline-flex shrink-0 cursor-pointer items-center justify-center gap-2 whitespace-nowrap rounded-[var(--control-radius)] border font-medium text-base outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background disabled:pointer-events-none disabled:opacity-64 sm:text-sm h-7 gap-1 rounded-md px-[7px] text-xs border-transparent text-foreground h-auto min-h-7 w-full sm:text-xs hover:bg-accent active:scale-[0.97] active:bg-accent"),
        (options.density == Density::Touch).then_some("min-h-10 sm:min-h-10"),
    ]);
    tw_with(ThemeScope::Root, &classes, &attrs)
}

fn row_hit_slop(density: Density, variant: Variant) -> EdgeInsets {
    let height = if density == Density::Touch {
        40.0
    } else if variant == Variant::Ghost {
        28.0
    } else if layout_class().is_compact() {
        32.0
    } else {
        28.0
    };
    let slack = if pointer() == Pointer::Coarse {
        (44.0 - height) / 2.0
    } else {
        0.0
    };
    EdgeInsets::all(px(slack))
}

pub fn menu_item_label(label: String) -> impl IntoElement {
    // text-box trim and supports text-box padding are browser-only typography hints.
    text(SharedString::from(label)).app_class(tw("min-w-0 truncate"))
}

/// Leading icon selector equivalents. Explicit icon size/tint belongs at its call site.
pub fn menu_item_icon(which: Icon, variant: Variant) -> impl IntoElement {
    svg(which.asset())
        .app_class(tw(&cn([
            Some("-mx-0.5 opacity-80 pointer-events-none shrink-0 size-4.5 sm:size-4 text-muted-foreground"),
            (variant == Variant::Destructive).then_some("text-current"),
            (variant == Variant::Ghost).then_some("size-3.5 sm:size-3.5"),
        ])))
        .accessible(false)
}

/// `options` supplies disabled/close-on-select; its row styling belongs to `menu_item`.
pub fn menu_checkbox_item(
    label: String,
    checked: impl Fn() -> bool + 'static,
    variant: CheckboxVariant,
    options: ItemOptions,
    children: impl IntoElement,
    on_checked_change: impl Fn(bool) + 'static,
) -> impl IntoElement {
    let checked = Prop::bind(checked);
    row_options(ItemOptions {
        variant: Variant::Default,
        density: Density::Default,
        inset: false,
        ..options
    })
    .label(label)
    .kind(RowKind::Checkbox)
    .checked(move || Some(checked.get()))
    .checkbox_variant(variant)
    .on_select(move || on_checked_change(!checked.get_untracked()))
    .child(children)
}

fn checkbox_content(
    variant: Prop<CheckboxVariant>,
    checked: Prop<Option<bool>>,
    disabled: Prop<bool>,
    children: Prop<Children>,
) -> impl IntoElement {
    // NSH's indicator slot is always before children; the TSX switch puts it after the label.
    // Draw decorative indicators in the TSX's order; the menu item alone carries checked/role.
    when(
        move || variant.get() == CheckboxVariant::Switch,
        move || {
            vec![
                view()
                    .app_class(tw("col-start-1"))
                    .child(children)
                    .into_element(),
                checkbox_switch(checked, disabled).into_element(),
            ]
        },
    )
    .otherwise(move || {
        vec![
            view()
                .app_class(move || {
                    Some(tw(&cn([
                        Some("col-start-1"),
                        (variant.get() == CheckboxVariant::Icon).then_some("-mx-0.5"),
                    ])))
                })
                .child(
                    view()
                        .opacity(move || {
                            if checked.get() == Some(true) {
                                1.0_f32
                            } else {
                                0.0
                            }
                        })
                        .child(
                            svg(ImageSrc::svg_markup(r#"<svg fill="none" height="24" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.75" viewBox="0 0 24 24" width="24" xmlns="http://www.w3.org/2000/svg"><path d="M5.252 12.7 10.2 18.63 18.748 5.37"/></svg>"#))
                                .accessible(false)
                                .app_class(move || Some(tw(&cn([
                                    Some("size-4.5 sm:size-4 pointer-events-none shrink-0"),
                                    (variant.get() == CheckboxVariant::Icon).then_some("size-3.5 sm:size-3.5"),
                                ])))),
                        ),
                )
                .into_element(),
            view()
                .app_class(tw("col-start-2"))
                .child(children)
                .into_element(),
        ]
    })
}

fn checkbox_switch(checked: Prop<Option<bool>>, disabled: Prop<bool>) -> impl IntoElement {
    // --thumb-size is local in the TSX (16/12px), not a theme token. Resolve its calc() here.
    let thumb = || {
        if layout_class().is_compact() {
            16.0
        } else {
            12.0
        }
    };
    view().accessible(false)
        .app_class(move || Some(tw(&cn([
            Some("inset-shadow-[0_1px_rgb(0_0_0/4%)] inline-flex shrink-0 items-center rounded-full p-px outline-none"),

            disabled.get().then_some("opacity-64"),
        ]))))
        // NSH accepts property-group masks, not the CSS transition list utility.
        .inline_style(move || transition_style(TransitionProps::COLORS | TransitionProps::SHADOW, 200.0, [0.4, 0.0, 0.2, 1.0]))
        .h(move || px(thumb() + 2.0)).w(move || px(thumb() * 2.0 - 2.0))
        .bg(move || if checked.get() == Some(true) { palette().primary } else { palette().input })
        // NSH groups transform/shape timings: use the TSX's 150ms translation timing for both,
        // without its delayed 100ms scale or origin tween; corner radii remain uniform.
        .child(view().app_class(move || Some(tw(&cn([
            Some("pointer-events-none block aspect-square h-full origin-left bg-background shadow-sm/5 group-active:scale-x-110"),
            (checked.get() == Some(true)).then_some("origin-[16px_50%] sm:origin-[12px_50%]"),

        ]))))
            .inline_style(move || transition_style(TransitionProps::TRANSFORM | TransitionProps::SHAPE, 150.0, Transition::EASE))
            .rounded(move || px(thumb()))
            .translate_x(move || px(if checked.get() == Some(true) { thumb() - 4.0 } else { 0.0 })))
}

fn transition_style(properties: u8, duration: f32, easing: [f32; 4]) -> Style {
    let properties = if reduce_motion() { 0 } else { properties };
    Style::default()
        .with(
            StyleKey::TransitionProperty,
            PropertyValue::TransitionProps(TransitionProps(properties)),
        )
        .with(
            StyleKey::TransitionDuration,
            PropertyValue::Ms(duration.into()),
        )
        .with(
            StyleKey::TransitionTimingFunction,
            PropertyValue::Easing(easing.map(Into::into)),
        )
}

pub fn menu_radio_group(
    label: String,
    value: impl Fn() -> String + 'static,
    children: impl IntoElement,
    on_value_change: impl Fn(String) + 'static,
) -> impl IntoElement {
    radio_group()
        .label(label)
        .value(move || SharedString::from(value()))
        .on_change(move |value: &SharedString| on_value_change(value.to_string()))
        .child(children)
}

#[derive(Clone)]
struct RadioSelection {
    value: Prop<SharedString>,
    choose: Rc<dyn Fn(SharedString)>,
}

#[derive(Clone, Copy)]
struct RadioChecked(Prop<Option<bool>>);

#[derive(Clone, PartialEq, Props)]
struct RadioGroupProps {
    label: Prop<SharedString>,
    value: Prop<SharedString>,
    children: Prop<Children>,
}

#[derive(Clone, Debug, PartialEq, Event)]
enum RadioGroupEvent {
    Change(SharedString),
}

#[component]
fn RadioGroup(props: RadioGroupProps, cx: Scope<RadioGroupEvent>) -> impl IntoElement {
    provide_context(RadioSelection {
        value: props.value,
        choose: Rc::new(move |value| cx.emit(RadioGroupEvent::Change(value))),
    });
    view()
        .role(AccessibilityRole::Group)
        .label(props.label)
        .child(props.children)
}

/// Reselecting emits `on_select`, but does not report a changed group value.
/// `options` supplies disabled/close-on-select; its row styling belongs to `menu_item`.
pub fn menu_radio_item(
    label: String,
    value: String,
    options: ItemOptions,
    children: impl IntoElement,
    on_select: impl Fn() + 'static,
) -> impl IntoElement {
    row_options(ItemOptions {
        variant: Variant::Default,
        density: Density::Default,
        inset: false,
        ..options
    })
    .label(label)
    .kind(RowKind::Radio)
    .radio_value(value)
    .on_select(on_select)
    .child(children)
}

pub fn menu_radio_item_indicator() -> impl IntoElement {
    menu_radio_item_indicator_with(
        svg(Icon::Check.asset())
            .app_class(tw("size-3.5"))
            .accessible(false),
    )
}

/// The TSX indicator's optional children slot, replacing its default Check icon.
pub fn menu_radio_item_indicator_with(children: impl IntoElement) -> impl IntoElement {
    radio_indicator().child(children)
}

#[derive(Clone, PartialEq, Props)]
struct RadioIndicatorProps {
    children: Prop<Children>,
}

#[component]
fn RadioIndicator(props: RadioIndicatorProps) -> impl IntoElement {
    let checked = use_context::<RadioChecked>();
    // hideIndicator is ignored in the TSX too; callers place this separate indicator explicitly.
    view()
        .accessible(false)
        .app_class(tw("flex shrink-0"))
        .child(when(
            move || checked.is_some_and(|checked| checked.0.get() == Some(true)),
            move || props.children,
        ))
}

pub fn menu_group_label(label: String, inset: bool) -> impl IntoElement {
    let attrs: &[(&str, &str)] = if inset { &[("data-inset", "")] } else { &[] };
    text(SharedString::from(label)).app_class(tw_with(
        ThemeScope::Root,
        "px-2 py-1.5 font-medium text-muted-foreground text-xs data-inset:ps-9 sm:data-inset:ps-8",
        attrs,
    ))
}

pub fn menu_separator() -> impl IntoElement {
    nsh_base::menu_separator().app_class(tw("mx-2 my-1 h-px bg-border"))
}

pub fn menu_shortcut(label: String) -> impl IntoElement {
    text(SharedString::from(label)).app_class(tw(
        "ms-auto font-medium font-sans text-secondary-label text-xs tracking-widest",
    ))
}

pub fn menu_item_detail(label: String) -> impl IntoElement {
    text(SharedString::from(label)).app_class(tw(
        "ms-auto shrink-0 ps-4 text-secondary-label tabular-nums",
    ))
}

pub fn menu_sub(
    trigger: impl IntoElement,
    disabled: impl Fn() -> bool + 'static,
    popup: impl IntoElement,
) -> impl IntoElement {
    nsh_base::submenu()
        .disabled(disabled)
        .offset(px(0.0))
        .trigger(trigger)
        .back(submenu_back())
        .child(popup)
}

fn submenu_back() -> impl IntoElement {
    // NSH sheets replace cascading popups with a Back row, using the TSX's touch row density.
    nsh_base::button().accessibility_label(t("Back"))
        .app_class(tw("flex min-h-10 items-center gap-2 rounded-sm px-2 py-1 text-base text-foreground outline-none sm:text-sm hover:bg-accent hover:text-accent-foreground focus:bg-accent focus:text-accent-foreground focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"))
        .hit_slop(move || row_hit_slop(Density::Touch, Variant::Default))
        .child(svg(Icon::ArrowLeft.asset()).accessible(false).app_class(tw("size-4.5 sm:size-4 shrink-0 text-muted-foreground")))
        .child(menu_item_label(t("Back")))
}

pub fn menu_sub_trigger(
    label: String,
    options: ItemOptions,
    children: impl IntoElement,
) -> impl IntoElement {
    sub_trigger()
        .label(label)
        .disabled(options.disabled)
        .density(options.density)
        .inset(options.inset)
        .child(children)
}

#[derive(Clone, PartialEq, Props)]
struct SubTriggerProps {
    label: Prop<SharedString>,
    disabled: Prop<bool>,
    density: Prop<Density>,
    inset: Prop<bool>,
    children: Prop<Children>,
}

#[component]
fn SubTrigger(props: SubTriggerProps) -> impl IntoElement {
    let node = element_ref();
    if let Some(rows) = use_context::<SearchRows>() {
        rows.add(node, props.disabled, props.label);
    }
    // NSH's submenu adds press/focus to this slot; no nested button or menu item is created.
    view().element_ref(&node).label(props.label).disabled(props.disabled)
        .app_class(move || {
            let attrs = [
                props.inset.get().then_some(("data-inset", "")),
                props.disabled.get().then_some(("data-disabled", "")),
            ].into_iter().flatten().collect::<Vec<_>>();
            Some(tw_with(ThemeScope::Root,
                &cn([
                    Some("flex min-h-8 cursor-pointer items-center gap-2 rounded-sm px-2 py-1 text-base text-foreground outline-none data-disabled:cursor-not-allowed data-disabled:pointer-events-none data-inset:ps-8 data-disabled:opacity-64 sm:min-h-7 sm:text-sm"),
                    (props.density.get() == Density::Touch).then_some("min-h-10 sm:min-h-10"),
                ]),
                &attrs))
        })
        .hover(|s| s.bg(move || palette().accent).text_color(move || palette().accent_foreground))
        .focus(|s| s.bg(move || palette().accent).text_color(move || palette().accent_foreground))
        // NSH exposes Base UI's data-popup-open state as accessibilityState.expanded.
        .expanded(|s| s.bg(move || palette().accent).text_color(move || palette().accent_foreground))
        .focus_visible(|s| s.ring(px(2.0)).ring_inset().ring_color(move || palette().ring))
        .hit_slop(move || row_hit_slop(props.density.get(), Variant::Default))
        .child(props.children)
        .child(svg(Icon::ChevronRight.asset()).app_class(tw("-me-0.5 ms-auto opacity-80 size-4.5 sm:size-4 pointer-events-none shrink-0 text-muted-foreground")).accessible(false))
}

pub fn menu_sub_popup(children: impl IntoElement) -> impl IntoElement {
    // NSH owns inline-end/start cascading placement; it exposes no alignOffset (-5 in TSX).
    menu_content()
        .inherit_close(true)
        .child(menu_popup(children))
}

#[cfg(test)]
mod tests {
    use super::typeahead_match;

    #[test]
    fn typeahead_wraps_skips_disabled_and_supports_prefixes_and_repeated_letters() {
        let rows = [
            ("Archive".into(), true),
            ("Alpha".into(), false),
            ("Beta".into(), false),
            ("Another".into(), false),
        ];
        assert_eq!(typeahead_match(&rows, "a", None), Some(1));
        assert_eq!(typeahead_match(&rows, "aa", Some(1)), Some(3));
        assert_eq!(typeahead_match(&rows, "a", Some(3)), Some(1));
        assert_eq!(typeahead_match(&rows, "an", Some(1)), Some(3));
        assert_eq!(typeahead_match(&rows, "unknown", Some(1)), None);
        assert_eq!(typeahead_match(&[], "a", None), None);
        assert_eq!(typeahead_match(&rows, "", None), None);
    }
}
