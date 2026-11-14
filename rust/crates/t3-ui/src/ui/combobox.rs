//! Port of apps/web/src/components/ui/combobox.tsx Combobox and autocomplete.tsx Autocomplete.
//!
//! Create `ComboboxState::new` once under the mounted owner. `combobox`/`autocomplete` compose
//! input, popup and list slots; `combobox_field` supplies the ordinary complete composition.
//! Values are opaque IDs; every text slot is `Text`. Autocomplete preserves free text on close.
//! Optional change handlers control requests: update the public signal to accept, or leave it
//! unchanged to decline. Without a handler, state is uncontrolled. Never recreate the state
//! in a reactive binding. For remotely ranked results set `filter=false`.

use std::rc::Rc;

use nsh_core::{AccessibilityState, keymap::When, prelude::*};

use crate::i18n::{Text, t};
use crate::state::layout::{Pointer, pointer};
use crate::theme::{self, Scope, cn, palette, tw, tw_with};
use crate::ui::icon::Icon;
use crate::ui::scroll_area::{ScrollAreaOptions, scroll_area_with_options};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub value: SharedString,
    pub label: Text,
    pub disabled: bool,
    pub group: Option<Text>,
}

impl Item {
    pub fn new(value: impl Into<SharedString>, label: impl Into<Text>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            disabled: false,
            group: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Single,
    Multiple,
    Autocomplete,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    Sm,
    #[default]
    Default,
    Lg,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AutoHighlight {
    #[default]
    Never,
    Always,
    OnInput,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub mode: Mode,
    pub filter: bool,
    pub auto_highlight: AutoHighlight,
    pub loop_focus: bool,
    pub keep_highlight: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            mode: Mode::Single,
            filter: true,
            auto_highlight: AutoHighlight::Never,
            loop_focus: true,
            keep_highlight: false,
        }
    }
}

type ValuesChange = Rc<dyn Fn(Vec<SharedString>)>;
type OpenChange = Rc<dyn Fn(bool)>;
type QueryChange = Rc<dyn Fn(String)>;

#[derive(Clone, Copy)]
pub struct ComboboxState {
    pub value: Signal<Vec<SharedString>>,
    pub query: Signal<String>,
    pub open: Signal<bool>,
    pub disabled: Signal<bool>,
    pub invalid: Signal<bool>,
    pub loading: Signal<bool>,
    pub items: Memo<Vec<Item>>,
    pub highlighted: Memo<Option<SharedString>>,
    pub anchor: ElementRef,
    pub input: ElementRef,
    root: ElementRef,
    options: Options,
    typed: Signal<bool>,
    requested: Signal<Option<SharedString>>,
    list: ElementRef,
    width: Signal<f32>,
    available: Signal<nsh_core::Size<Points>>,
    active_row: Signal<Option<(SharedString, ElementRef)>>,
    change: Stored<Option<ValuesChange>>,
    open_change: Stored<Option<OpenChange>>,
    query_change: Stored<Option<QueryChange>>,
}

impl ComboboxState {
    pub fn new(items: impl Fn() -> Vec<Item> + 'static, options: Options) -> Self {
        let query = signal(String::new());
        let typed = signal(false);
        let requested = signal(None::<SharedString>);
        let catalogue = computed(items);
        let items = computed(move || {
            let query = query.get();
            catalogue.with(|items| {
                items
                    .iter()
                    .filter(|item| {
                        !options.filter
                            || !typed.get()
                            || use_combobox_filter(&item.label.resolve(), &query)
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
        });
        let highlighted = computed(move || {
            let requested = requested.get();
            items.with(|items| {
                requested
                    .filter(|id| items.iter().any(|item| &item.value == id))
                    .or_else(|| {
                        (options.auto_highlight == AutoHighlight::Always
                            || (options.auto_highlight == AutoHighlight::OnInput && typed.get()))
                        .then(|| items.first().map(|item| item.value.clone()))
                        .flatten()
                    })
            })
        });
        let root = element_ref();
        Self {
            value: signal(Vec::new()),
            query,
            open: signal(false),
            disabled: signal(false),
            invalid: signal(false),
            loading: signal(false),
            items,
            highlighted,
            options,
            typed,
            requested,
            root,
            anchor: element_ref(),
            input: element_ref(),
            list: element_ref(),
            width: signal(0.0),
            available: signal(size(window().width, px(368.0))),
            active_row: signal(None),
            change: stored(None),
            open_change: stored(None),
            query_change: stored(None),
        }
    }

    pub fn on_value_change(self, callback: impl Fn(Vec<SharedString>) + 'static) {
        self.change.replace(Some(Rc::new(callback)));
    }

    pub fn on_open_change(self, callback: impl Fn(bool) + 'static) {
        self.open_change.replace(Some(Rc::new(callback)));
    }

    pub fn on_query_change(self, callback: impl Fn(String) + 'static) {
        self.query_change.replace(Some(Rc::new(callback)));
    }

    pub fn set_open(self, next: bool) {
        if self.disabled.get_untracked() || self.open.get_untracked() == next {
            return;
        }
        if let Some(callback) = self.open_change.get() {
            callback(next);
        } else {
            self.open.set(next);
        }
        if !next && !self.open.get_untracked() {
            self.requested.set(None);
            if self.options.mode != Mode::Autocomplete {
                self.typed.set(false);
                self.set_query(String::new());
            }
        }
    }

    fn set_query(self, query: String) {
        if self.query.get_untracked() == query {
            return;
        }
        if let Some(callback) = self.query_change.get() {
            callback(query);
        } else {
            self.query.set(query);
        }
    }

    pub fn type_query(self, query: String) {
        if self.disabled.get_untracked() {
            return;
        }
        self.typed.set(true);
        self.requested.set(None);
        self.set_query(query);
        self.set_open(true);
    }

    fn set_value(self, value: Vec<SharedString>) {
        if self.value.get_untracked() == value {
            return;
        }
        if let Some(callback) = self.change.get() {
            callback(value);
        } else {
            self.value.set(value);
        }
    }

    pub fn choose(self, value: &str) {
        if self.disabled.get_untracked() || !self.open.get_untracked() {
            return;
        }
        let Some(item) = self.items.with_untracked(|items| {
            items
                .iter()
                .find(|item| item.value == value && !item.disabled)
                .cloned()
        }) else {
            return;
        };
        if self.options.mode == Mode::Multiple {
            let mut next = self.value.get_untracked();
            if let Some(at) = next.iter().position(|id| id == &item.value) {
                next.remove(at);
            } else {
                next.push(item.value);
            }
            self.set_value(next);
            self.set_query(String::new());
            self.typed.set(false);
        } else {
            self.set_value(vec![item.value]);
            if self.options.mode == Mode::Autocomplete {
                self.set_query(item.label.resolve().to_string());
            }
            self.set_open(false);
        }
        self.input.focus();
    }

    pub fn clear(self) {
        if self.disabled.get_untracked() {
            return;
        }
        self.set_value(Vec::new());
        self.set_query(String::new());
        self.typed.set(false);
        self.requested.set(None);
        self.input.focus();
    }

    fn step(self, forward: bool) {
        if self.disabled.get_untracked() {
            return;
        }
        let was_open = self.open.get_untracked();
        self.set_open(true);
        let current = if was_open {
            self.highlighted.get_untracked()
        } else {
            self.value.with_untracked(|v| v.first().cloned())
        };
        if !was_open
            && let Some(current) = current.as_ref()
            && self
                .items
                .with_untracked(|items| items.iter().any(|item| &item.value == current))
        {
            self.requested.set(Some(current.clone()));
            return;
        }
        let next = self.items.with_untracked(|items| {
            let at = current
                .as_ref()
                .and_then(|id| items.iter().position(|i| &i.value == id));
            let index = match (at, forward) {
                (None, true) => Some(0),
                (None, false) => items.len().checked_sub(1),
                (Some(at), true) if at + 1 < items.len() => Some(at + 1),
                (Some(at), false) if at > 0 => Some(at - 1),
                (Some(at), _) if !self.options.loop_focus => Some(at),
                (_, _) if self.options.auto_highlight == AutoHighlight::Never => None,
                (_, true) => Some(0),
                (_, false) => items.len().checked_sub(1),
            };
            index.and_then(|at| items.get(at)).map(|i| i.value.clone())
        });
        self.requested.set(next);
    }

    fn display(self) -> String {
        if self.options.mode != Mode::Single || self.typed.get() {
            return self.query.get();
        }
        self.items.with(|items| {
            self.value.with(|values| {
                values
                    .first()
                    .and_then(|id| items.iter().find(|i| &i.value == id))
                    .map_or_else(String::new, |item| item.label.resolve().to_string())
            })
        })
    }
}

/// Default matching awaits t3-i18n's ICU search filter (same frozen seam as UI-COMMAND).
/// Case-insensitive substring is the current fallback; pass filter=false for client-ranked data.
pub fn use_combobox_filter(candidate: &str, query: &str) -> bool {
    candidate.to_lowercase().contains(&query.to_lowercase())
}

pub fn combobox<E: IntoElement>(
    state: ComboboxState,
    children: impl FnOnce(ComboboxState) -> E,
) -> impl IntoElement {
    // Native focus observation needs a scope node; Base UI's context-only root has no DOM node.
    state.root.on_focus_out(move || state.set_open(false));
    view().element_ref(&state.root).child(children(state))
}

/// Port of autocomplete.tsx Autocomplete (construct state with Mode::Autocomplete).
pub use combobox as autocomplete;

pub struct InputOptions {
    pub label: Text,
    pub placeholder: Text,
    pub size: Size,
    pub show_trigger: bool,
    pub show_clear: bool,
    pub unstyled: bool,
    pub auto_focus: bool,
    pub start_addon: Option<AnyElement>,
}

impl InputOptions {
    pub fn new(label: impl Into<Text>) -> Self {
        Self {
            label: label.into(),
            placeholder: Text::content(""),
            size: Size::Default,
            show_trigger: true,
            show_clear: false,
            unstyled: false,
            auto_focus: false,
            start_addon: None,
        }
    }

    pub fn autocomplete(label: impl Into<Text>) -> Self {
        Self {
            show_trigger: false,
            ..Self::new(label)
        }
    }
}

pub fn combobox_input(state: ComboboxState, options: InputOptions) -> impl IntoElement {
    input_impl(state, options, false)
}

fn input_impl(state: ComboboxState, options: InputOptions, search: bool) -> impl IntoElement {
    let focused = signal(false);
    effect(move || {
        if !focused.get() || state.disabled.get() {
            return;
        }
        for (key, forward) in [("down", true), ("up", false)] {
            on_key_with(
                key,
                KeyOptions {
                    when: When::Always,
                    repeat: true,
                    for_focus: true,
                    ..Default::default()
                },
                move |_| state.step(forward),
            );
        }
        if state.open.get() {
            for key in ["tab", "shift+tab"] {
                on_key_with(
                    key,
                    KeyOptions {
                        when: When::Always,
                        pass_through: true,
                        ..Default::default()
                    },
                    move |_| state.set_open(false),
                );
            }
            on_key_with(
                "escape",
                KeyOptions {
                    when: When::Always,
                    ..Default::default()
                },
                move |_| {
                    state.set_open(false);
                    state.input.focus();
                },
            );
        }
    });
    let label = options.label;
    let placeholder = options.placeholder;
    let has_addon = options.start_addon.is_some();
    let has_end = options.show_trigger || options.show_clear;
    let small = options.size == Size::Sm;
    // Frozen ui/input.rs lacks sm, unstyled, disabled and ref props; transcribe Input here.
    // Native scrolling/wrapping replaces web hints; bg-clip-padding and autofill CSS have no NSH API.
    view().element_ref(&state.anchor)
        .on_layout(move |bounds: &Bounds<Points>| state.width.set(bounds.size.width.0))
        .app_class(move || tw(&cn([Some("relative w-full text-foreground"),
            search.then_some("flex w-full"),
            state.disabled.get().then_some("opacity-64")])))
        .child(options.start_addon.map(|addon| view()
            .accessibility_elements_hidden(true)
            .pointer_events(if state.options.mode == Mode::Autocomplete { PointerEvents::Auto } else { PointerEvents::None })
            .app_class(tw(if small {
                "absolute inset-y-0 start-px z-10 flex items-center ps-[9px] opacity-80"
            } else { "absolute inset-y-0 start-px z-10 flex items-center ps-[11px] opacity-80" }))
            .child(addon)))
        .child(view().app_class(move || tw(&cn([
            (!options.unstyled).then_some("relative inline-flex w-full rounded-lg border border-input bg-background text-base text-foreground shadow-xs/5 ring-ring/24 transition-shadow sm:text-sm dark:bg-input/32 focus-within:border-ring focus-within:ring-[3px] focus-within:ring-inset"),
            options.unstyled.then_some("rounded-none bg-transparent text-sm"),
            state.invalid.get().then_some("border-destructive/36 focus-within:border-destructive/64 focus-within:ring-destructive/16 dark:ring-destructive/24"),
            (focused.get() || state.disabled.get() || state.invalid.get()).then_some("shadow-none"),
        ])))
            .child((!options.unstyled).then(|| view()
                .accessible(false)
                .rounded(move || px((tokens().number("radius-lg").unwrap_or(8.0) - 1.0).max(0.0)))
                .app_class(move || tw(&cn([Some("pointer-events-none absolute inset-0"),
                    (!focused.get() && !state.disabled.get() && !state.invalid.get()).then_some("shadow-[0_1px_rgb(0_0_0/4%)] dark:shadow-[0_-1px_rgb(255_255_255/6%)]")])))))
            .child(nsh_base::input().element_ref(&state.input)
                .role(AccessibilityRole::Combobox)
                .accessibility_state(move || AccessibilityState { expanded: Some(state.open.get()),
                    disabled: state.disabled.get(), invalid: state.invalid.get(), has_popup: true,
                    ..Default::default() })
                .accessibility_controls(Some(state.list))
                .accessibility_active_descendant(move || state.active_row.get()
                    .filter(|(id, _)| state.highlighted.get().as_ref() == Some(id)).map(|(_, row)| row))
                .value(move || state.display())
                .disabled(state.disabled)
                .label(move || label.resolve())
                .placeholder(move || placeholder.resolve().to_string())
                .auto_focus(options.auto_focus)
                .on_focus(move || focused.set(true))
                .on_blur(move || focused.set(false))
                .on_press(move || state.set_open(true))
                .on_change(move |query: &str| state.type_query(query.to_owned()))
                .on_submit(move |_: &str| {
                    if let Some(value) = state.highlighted.get_untracked() { state.choose(&value); }
                })
                .submit_behavior(SubmitBehavior::Submit)
                .app_class(tw(&cn([
                    Some("h-8.5 w-full min-w-0 rounded-lg px-[11px] leading-8.5 outline-none placeholder:text-placeholder sm:h-7.5 sm:leading-7.5"),
                    small.then_some("h-7.5 px-[9px] leading-7.5 sm:h-6.5 sm:leading-6.5"),
                    (options.size == Size::Lg).then_some("h-9.5 leading-9.5 sm:h-8.5 sm:leading-8.5"),
                    (has_addon && small).then_some("ps-[29px] sm:ps-[27px]"),
                    (has_addon && !small).then_some("ps-[33px] sm:ps-[31px]"),
                    (has_end && small).then_some("pe-6.5"),
                    (has_end && !small).then_some("pe-7"),
                    search.then_some("h-6.5 ps-5 font-sans leading-6.5 sm:h-6.5 sm:leading-6.5"),
                ])))))
        .child((options.show_trigger).then(|| dynamic(move || {
            (!options.show_clear || (state.value.with(|v| v.is_empty()) && state.query.with(|q| q.is_empty())))
                .then(|| end_button(state, small, false))
        })))
        .child(options.show_clear.then(|| dynamic(move || {
            (!state.value.with(|v| v.is_empty()) || !state.query.with(|q| q.is_empty()))
                .then(|| end_button(state, small, true))
        })))
}

/// Port of autocomplete.tsx AutocompleteInput; the web's default trigger is false.
pub fn autocomplete_input(state: ComboboxState, options: InputOptions) -> impl IntoElement {
    combobox_input(state, options)
}

fn end_button(state: ComboboxState, small: bool, clear: bool) -> impl IntoElement {
    end_button_with_transition(state, small, clear, false)
}

fn end_button_with_transition(
    state: ComboboxState,
    small: bool,
    clear: bool,
    standalone: bool,
) -> impl IntoElement {
    // Styling a Slot to override Button focusability creates a zero-height containing block.
    // Keep the native button as the positioned root; this NSH pin cannot remove its Tab stop.
    nsh_base::button().disabled(state.disabled)
        .inline_style(move || {
            use nsh_core::{PropertyValue, StyleKey, TransitionProps};
            let style = nsh_core::element::Style::default();
            // NSH groups color properties; the source's arbitrary property-list class is unsupported.
            if standalone { style.with(StyleKey::TransitionProperty, PropertyValue::TransitionProps(TransitionProps(
                if reduce_motion() { 0 } else { TransitionProps::COLORS | TransitionProps::SHADOW | TransitionProps::OPACITY }
            ))) } else { style }
        })
        .accessibility_label(move || Some(if clear { t("Clear") } else { t("Show suggestions") }.into()))
        .app_class(tw(&cn([Some("-translate-y-1/2 absolute top-1/2 inline-flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-md border border-transparent opacity-80 outline-none hover:opacity-100 sm:size-7"),
            Some(if state.options.mode == Mode::Autocomplete { "transition-colors" } else { "transition-opacity" }),
            Some(if small { "end-0" } else { "end-0.5" })])))
        .hit_slop(move || EdgeInsets::all(px(if pointer() == Pointer::Coarse {
            if crate::state::layout::layout_class().is_compact() { 6.0 } else { 8.0 }
        } else { 0.0 })))
        .on_press(move || { if clear { state.clear(); } else {
            state.set_open(!state.open.get_untracked()); state.input.focus();
        } })
        .child(svg(if clear { Icon::X.asset() } else { Icon::ChevronsUpDown.asset() })
            .app_class(tw("pointer-events-none size-4.5 shrink-0 sm:size-4")).accessible(false))
}

pub fn combobox_trigger(
    state: ComboboxState,
    label: impl Into<Text>,
    children: impl IntoElement,
) -> impl IntoElement {
    let label = label.into();
    // The standalone web trigger has no classes; appearance belongs to its supplied content.
    nsh_base::button()
        .disabled(state.disabled)
        .accessibility_label(move || Some(label.resolve()))
        .on_press(move || {
            state.set_open(!state.open.get_untracked());
            state.input.focus();
        })
        .child(children)
}

pub fn combobox_clear(state: ComboboxState, children: impl IntoElement) -> impl IntoElement {
    nsh_base::button()
        .disabled(state.disabled)
        .accessibility_label(move || Some(t("Clear").into()))
        .on_press(move || state.clear())
        .child(children)
}

pub use combobox_trigger as autocomplete_trigger;

pub fn autocomplete_clear(state: ComboboxState) -> impl IntoElement {
    end_button_with_transition(state, false, true, true)
}

pub fn combobox_search_input(state: ComboboxState, mut options: InputOptions) -> impl IntoElement {
    options.show_trigger = false;
    options.size = Size::Sm;
    options.unstyled = true;
    view().app_class(tw("min-w-0 shrink-0 px-3 pt-2.5"))
        .child(view().app_class(tw("relative -translate-y-px border-b border-border/70 pb-1.5 transition-colors focus-within:border-ring"))
            .child(svg(Icon::Search.asset()).accessible(false).app_class(tw("pointer-events-none absolute top-1.5 left-0 size-4 shrink-0 text-muted-foreground/55")))
            .child(input_impl(state, options, true)))
}

#[derive(Clone, Copy, Debug)]
pub struct PopupOptions {
    pub side: Placement,
    pub align: Align,
    pub side_offset: f32,
    pub anchor: Option<ElementRef>,
}

impl Default for PopupOptions {
    fn default() -> Self {
        Self {
            side: Placement::Bottom,
            align: Align::Start,
            side_offset: 4.0,
            anchor: None,
        }
    }
}

pub fn combobox_popup<E: IntoElement>(
    state: ComboboxState,
    options: PopupOptions,
    children: impl Fn() -> E + 'static,
) -> impl IntoElement {
    when(
        move || state.open.get() && !state.disabled.get(),
        move || {
            untracked(|| {
                // NSH anchored overlays supply collision placement and outside/cancel delivery. Unlike
                // native Combobox's closed list API, this keeps autocomplete free text and web slot order.
                overlay().anchor_content(&options.anchor.unwrap_or(state.anchor))
            .placement(options.side).align(options.align).offset(px(options.side_offset)).margin(px(5.0))
            .on_position(move |position: &OverlayPosition| state.available.set(position.available))
            .on_press_outside(move |_: &Point<Points>| state.set_open(false))
            .on_cancel(move || { state.set_open(false); state.input.focus(); })
            .window_drag(false)
            .app_class(tw("z-[130] select-none relative flex rounded-lg"))
            .min_w(move || px(state.width.get()))
            .max_w(move || state.available.get().width)
            .max_h(move || px(state.available.get().height.0.min(368.0)))
            .child(view().app_class(tw(if state.options.mode == Mode::Autocomplete {
                "relative flex max-h-full min-w-0 flex-1 flex-col rounded-lg border bg-popover text-foreground shadow-lg/5"
            } else {
                "relative flex max-h-full min-w-0 flex-1 flex-col overflow-hidden rounded-lg border text-foreground backdrop-blur-(--glass-blur) dark:backdrop-blur-[16px] backdrop-saturate-[1.14] dark:backdrop-saturate-[1.08] shadow-[0_16px_40px_-18px_rgb(0_0_0/55%)] dark:shadow-[0_18px_44px_-18px_rgb(0_0_0/80%)]"
            }))
                .bg(move || if state.options.mode == Mode::Autocomplete { palette().popover } else {
                    palette().popover.opacity(0.18 + 0.82 * (theme::current_tokens().number("glass-opacity").unwrap_or(80.0) / 100.0))
                })
                .border_color(move || if state.options.mode == Mode::Autocomplete { palette().border } else {
                    theme::current_tokens().color("contrast-foreground").unwrap_or(palette().foreground).opacity(0.1)
                })
                .child((state.options.mode == Mode::Autocomplete).then(|| view().accessible(false)
                    .rounded(move || px((tokens().number("radius-lg").unwrap_or(8.0) - 1.0).max(0.0)))
                    .app_class(tw("pointer-events-none absolute inset-0 shadow-[0_1px_rgb(0_0_0/4%)] dark:shadow-[0_-1px_rgb(255_255_255/6%)]"))))
                .child(children()))
            })
        },
    )
}

pub use combobox_popup as autocomplete_popup;

pub fn combobox_list(
    state: ComboboxState,
    label: impl Into<Text>,
    children: impl IntoElement,
) -> impl IntoElement {
    let label = label.into();
    scroll_area_with_options(
        ScrollAreaOptions {
            scroll_fade: true,
            scrollbar_gutter: true,
            viewport_ref: Some(state.list),
            role: AccessibilityRole::Listbox,
            label: Some(label),
            busy: state.loading.into(),
            on_mouse_leave: Some(Rc::new(move || {
                if !state.options.keep_highlight {
                    state.requested.set(None);
                }
            })),
            ..Default::default()
        },
        view()
            .app_class(move || {
                tw(&cn(
                    [(!state.items.with(|i| i.is_empty())).then_some("p-1")],
                ))
            })
            .child(children),
    )
}

pub use combobox_list as autocomplete_list;

pub fn combobox_list_virtualized<E: IntoElement>(
    state: ComboboxState,
    label: impl Into<Text>,
    render: impl Fn(Memo<Option<Item>>) -> E + 'static,
) -> impl IntoElement {
    let label = label.into();
    effect(move || {
        let highlighted = state.highlighted.get();
        if let Some(index) = state.items.with(|items| {
            items
                .iter()
                .position(|i| Some(&i.value) == highlighted.as_ref())
        }) {
            state
                .list
                .scroll_to_index(index as u32, Percentage(0.5), px(0.0), false);
        }
    });
    virtual_list(
        move || state.items.with(|items| items.len()),
        move |at| {
            render(computed(move || {
                state.items.with(|items| items.get(at.index()).cloned())
            }))
        },
    )
    .element_ref(&state.list)
    .role(AccessibilityRole::Listbox)
    .label(move || label.resolve())
    .keys(move || {
        state
            .items
            .with(|items| items.iter().map(|i| i.value.clone()).collect::<Vec<_>>())
    })
    .app_class(move || {
        tw(&cn([
            Some("size-full min-w-0"),
            (state.options.mode != Mode::Autocomplete
                && !state.items.with(|items| items.is_empty()))
            .then_some("px-1 py-1"),
        ]))
    })
    .estimated_item_size(move || {
        px(if crate::state::layout::layout_class().is_compact() {
            32.0
        } else {
            28.0
        })
    })
}

pub use combobox_list_virtualized as autocomplete_list_virtualized;

pub fn autocomplete_collection<E: IntoElement>(
    state: ComboboxState,
    render: impl Fn(Memo<Option<Item>>) -> E + 'static,
) -> impl IntoElement {
    each(
        move || state.items.get(),
        |item| item.value.clone(),
        move |item, _| render(computed(move || Some(item.get()))),
    )
}

pub fn combobox_item(
    state: ComboboxState,
    item: Memo<Option<Item>>,
    selected_fill: bool,
    children: impl IntoElement,
) -> impl IntoElement {
    let row = element_ref();
    let highlighted = computed(move || {
        item.with(|item| {
            item.as_ref()
                .is_some_and(|item| state.highlighted.get().as_ref() == Some(&item.value))
        })
    });
    effect(move || {
        if highlighted.get() {
            if let Some(item) = item.get_untracked() {
                state.active_row.set(Some((item.value, row)));
            }
            row.reveal(
                RevealOptions::default()
                    .vertical(ScrollAlignment::Nearest)
                    .animated(false),
            );
        }
    });
    view().element_ref(&row).role(AccessibilityRole::Option).focusable(false)
        .optional_label(move || item.with(|item| item.as_ref().map(|i| i.label.resolve())))
        .disabled(move || state.disabled.get() || !state.open.get() || item.with(|i| i.as_ref().is_none_or(|i| i.disabled)))
        .accessibility_state(move || AccessibilityState {
            selected: item.with(|i| i.as_ref().is_some_and(|i| state.value.with(|v| v.contains(&i.value)))),
            disabled: state.disabled.get() || item.with(|i| i.as_ref().is_none_or(|i| i.disabled)),
            ..Default::default()
        })
        .on_mouse_move(move |_: &MouseEvent| {
            if let Some(item) = item.get_untracked().filter(|i| !i.disabled) { state.requested.set(Some(item.value)); }
        })
        .on_press(move || { if let Some(item) = item.get_untracked() { state.choose(&item.value); } })
        .app_class(move || {
            let disabled = state.disabled.get() || item.with(|i| i.as_ref().is_none_or(|i| i.disabled));
            let selected = item.with(|i| i.as_ref().is_some_and(|i| state.value.with(|v| v.contains(&i.value))));
            let attrs = [disabled.then_some(("data-disabled", "")), selected.then_some(("data-selected", "")), highlighted.get().then_some(("data-highlighted", ""))].into_iter().flatten().collect::<Vec<_>>();
            tw_with(Scope::Root, &cn([
                Some("flex min-h-8 items-center rounded-sm px-2 py-1 text-base outline-none data-disabled:pointer-events-none data-selected:text-foreground data-disabled:opacity-64 sm:min-h-7 sm:text-sm"),
                Some(if state.options.mode == Mode::Autocomplete {
                    "cursor-default select-none gap-2 hover:bg-accent data-selected:bg-accent/50 data-highlighted:bg-accent data-highlighted:text-accent-foreground"
                } else { "cursor-pointer data-disabled:cursor-not-allowed data-highlighted:bg-foreground/[0.09] data-highlighted:text-accent-foreground" }),
                (state.options.mode != Mode::Autocomplete && !disabled).then_some("hover:bg-foreground/[0.09]"),
                (state.options.mode != Mode::Autocomplete && selected_fill).then_some("data-selected:bg-foreground/[0.06]"),
                highlighted.get().then_some(if state.options.mode == Mode::Autocomplete { "bg-accent text-accent-foreground" } else { "bg-foreground/[0.09] text-accent-foreground" }),
            ]), &attrs)
        })
        .child(if state.options.mode == Mode::Autocomplete { children.into_element() } else {
            view().app_class(tw("flex min-w-0 flex-1 items-center gap-2")).child(children).into_element()
        })
}

pub fn autocomplete_item(
    state: ComboboxState,
    item: Memo<Option<Item>>,
    children: impl IntoElement,
) -> impl IntoElement {
    combobox_item(state, item, true, children)
}

pub fn combobox_empty(state: ComboboxState, label: impl Into<Text>) -> impl IntoElement {
    let label = label.into();
    when(
        move || state.items.with(|i| i.is_empty()),
        move || {
            let label = label.clone();
            let label = computed(move || label.resolve());
            text(move || label.get()).app_class(move || {
                tw(&cn([
                    Some("text-center text-base text-muted-foreground sm:text-sm"),
                    (!label.with(|label| label.is_empty())).then_some("p-2"),
                ]))
            })
        },
    )
}

pub use combobox_empty as autocomplete_empty;

pub fn combobox_status(label: impl Into<Text>) -> impl IntoElement {
    let label = label.into();
    let label = computed(move || label.resolve());
    text(move || label.get())
        .role(AccessibilityRole::Summary)
        .app_class(move || {
            tw(&cn([
                Some("font-medium text-muted-foreground text-xs"),
                Some(if label.with(|label| label.is_empty()) {
                    "m-0 p-0"
                } else {
                    "px-3 py-2"
                }),
            ]))
        })
}

/// Port of autocomplete.tsx AutocompleteGroup; follows_group replaces the sibling selector.
pub fn autocomplete_group(
    label: impl Into<Text>,
    follows_group: bool,
    children: impl IntoElement,
) -> impl IntoElement {
    let label = label.into();
    view()
        .role(AccessibilityRole::Group)
        .label(move || label.resolve())
        .app_class(tw(if follows_group { "mt-1.5" } else { "" }))
        .child(children)
}

pub fn autocomplete_group_label(label: impl Into<Text>) -> impl IntoElement {
    let label = label.into();
    text(move || label.resolve())
        .app_class(tw("px-2 py-1.5 font-medium text-muted-foreground text-xs"))
}

pub use autocomplete_group_label as autocomplete_list_heading;

pub fn combobox_field(
    state: ComboboxState,
    options: InputOptions,
    popup: PopupOptions,
    empty: impl Into<Text>,
) -> impl IntoElement {
    let empty = empty.into();
    combobox(state, move |state| {
        view()
            .child(combobox_input(state, options))
            .child(combobox_popup(state, popup, move || {
                view()
                    .child(combobox_list(
                        state,
                        Text::from("Suggestions"),
                        autocomplete_collection(state, move |item| {
                            combobox_item(
                                state,
                                item,
                                true,
                                text(move || {
                                    item.with(|i| {
                                        i.as_ref().map(|i| i.label.resolve()).unwrap_or_default()
                                    })
                                }),
                            )
                        }),
                    ))
                    .child(combobox_empty(state, empty.clone()))
            }))
    })
}
