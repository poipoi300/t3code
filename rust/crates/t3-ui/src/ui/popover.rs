//! Port of apps/web/src/components/ui/popover.tsx Popover and preview-card.tsx PreviewCard.
//!
//! `popover_create_handle()` owns controlled open state; `popover(handle, options, trigger,
//! content)` composes the trigger slot and a lazy popup. `popover_trigger`, `popover_close`,
//! `popover_title`, and `popover_popup` (also `popover_content`) are the corresponding slots.
//! Labels passed to trigger/title/close are English translation keys; arbitrary content is
//! translated by its caller. `Variant::{Default, Panel}`, `Size::{Auto, Sm, Md, Lg}` (the web's
//! width prop), and `Padding::{Default, Compact, None}` keep the web names. There is no restyling API.
//! `popover_trigger(handle, ..)` measures the panel anchor; a custom trigger can forward its
//! layout event to `handle.measure_anchor(bounds)`.
//!
//! `preview_card(options, trigger, content)` uses NSH's hover-card lifecycle; its trigger and
//! popup slots are `preview_card_trigger` and `preview_card_popup`. NSH owns portals, flipping,
//! safe-area clamping, hover delays, Escape/back and outside dismissal. It exposes no separate
//! anchor, align offset, configurable collision strategy, or positioner interpolation.
//! Preview triggers take NSH's `LinkOpenWith` and `on_open`; `App` delegates relative routes
//! to the caller, while `System` keeps native external URL opening.

use nsh_core::prelude::*;

use crate::i18n::t;
use crate::state::layout::{Pointer, pointer};
use crate::theme::{cn, tw};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Variant {
    #[default]
    Default,
    Panel,
}

/// The TSX `width` prop; it has no separate height/size variant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    #[default]
    Auto,
    Sm,
    Md,
    Lg,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Padding {
    #[default]
    Default,
    Compact,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopoverOptions {
    pub variant: Variant,
    pub width: Size,
    pub padding: Padding,
    pub side: Placement,
    pub align: Align,
    pub side_offset: f32,
    pub tooltip_style: bool,
    pub keep_mounted: bool,
    /// NSH always consumes coarse outside taps for standard popovers; this opts retained
    /// popovers into the same backdrop behavior.
    pub touch_backdrop: bool,
    /// Replaces the TSX's `has-data-[slot=calendar]` descendant selector.
    pub calendar: bool,
}

impl Default for PopoverOptions {
    fn default() -> Self {
        Self {
            variant: Variant::Default,
            width: Size::Auto,
            padding: Padding::Default,
            side: Placement::Bottom,
            align: Align::Center,
            side_offset: 4.0,
            tooltip_style: false,
            keep_mounted: false,
            touch_backdrop: false,
            calendar: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopoverHandle {
    open: Signal<bool>,
    available: Signal<nsh_core::Size<Points>>,
    anchor_width: Signal<f32>,
    title: Signal<Option<String>>,
}

pub fn popover_create_handle() -> PopoverHandle {
    PopoverHandle {
        open: signal(false),
        available: signal(nsh_core::Size::new(window().width, window().height)),
        anchor_width: signal(0.0),
        title: signal(None),
    }
}

impl PopoverHandle {
    pub fn is_open(self) -> bool {
        self.open.get()
    }

    pub fn set_open(self, open: bool) {
        self.open.set(open);
    }

    pub fn measure_anchor(self, bounds: Bounds<Points>) {
        self.anchor_width.set(bounds.size.width.0);
    }
}

pub fn popover(
    handle: PopoverHandle,
    options: PopoverOptions,
    trigger: impl IntoElement + 'static,
    content: impl Fn() -> AnyElement + 'static,
) -> impl IntoElement {
    if options.keep_mounted {
        return with_popup_context((handle, options), move || {
            retained_popover()
                .trigger(trigger)
                .child(dynamic(move || {
                    untracked(|| popover_popup(handle, options, content()).into_element())
                }))
                .into_element()
        })
        .into_element();
    }
    with_popup_context((handle, options), move || {
        standard_popover(handle, options, trigger, content).into_element()
    })
    .into_element()
}

fn standard_popover(
    handle: PopoverHandle,
    options: PopoverOptions,
    trigger: impl IntoElement,
    content: impl Fn() -> AnyElement + 'static,
) -> impl IntoElement {
    // NSH combines Portal and Positioner into its anchored overlay. Its coarse-pointer
    // backdrop always consumes outside taps, preserving mobile's no-click-through behavior.
    nsh_base::popover()
        .open(move || Some(handle.is_open()))
        .on_open_change(move |event: &nsh_base::PopoverEvent| {
            if let nsh_base::PopoverEvent::OpenChange(open, _) = event {
                handle.set_open(*open);
            }
        })
        .placement(options.side)
        .align(options.align)
        .offset(px(options.side_offset))
        .margin(px(if options.variant == Variant::Panel {
            0.0
        } else {
            5.0
        }))
        // NSH has no width/height tween or arbitrary transition list. Only this
        // overlay's transform and opacity change; its painted surface is the child.
        .popup_class(tw(if options.variant == Variant::Panel {
            "z-[46] transition"
        } else {
            "z-[130] transition"
        }))
        .popup_starting_class(tw("scale-98 opacity-0"))
        .popup_ending_class(tw("scale-98 opacity-0"))
        .on_position(move |event: &nsh_base::PopoverEvent| {
            if let nsh_base::PopoverEvent::Position(_, _, available) = event {
                handle.available.set(*available);
            }
        })
        .trigger(trigger)
        .child(dynamic(move || {
            untracked(|| popover_popup(handle, options, content()).into_element())
        }))
}

pub fn popover_trigger(
    handle: PopoverHandle,
    label: impl Into<String>,
    disabled: impl Fn() -> bool + 'static,
    children: impl IntoElement,
) -> impl IntoElement {
    let label = label.into();
    nsh_base::button()
        .accessibility_label(move || Some(t(&label).into()))
        .disabled(disabled)
        .on_layout(move |bounds: &Bounds<Points>| handle.measure_anchor(*bounds))
        .app_class(tw(
            "focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
        ))
        .child(children)
}

pub fn popover_close(
    handle: PopoverHandle,
    label: impl Into<String>,
    children: impl IntoElement,
) -> impl IntoElement {
    let label = label.into();
    nsh_base::button()
        .accessibility_label(move || Some(t(&label).into()))
        .on_press(move || handle.set_open(false))
        .app_class(tw(
            "focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
        ))
        .child(children)
}

pub fn popover_title(source: impl Into<String>) -> impl IntoElement {
    let source = source.into();
    if let Some((handle, _)) = use_context::<(PopoverHandle, PopoverOptions)>() {
        handle.title.set(Some(source.clone()));
        let title = source.clone();
        cleanup(move || {
            if handle.title.get_untracked().as_ref() == Some(&title) {
                handle.title.set(None);
            }
        });
    }
    text(move || SharedString::from(t(&source)))
        .role(AccessibilityRole::Header)
        .app_class(tw("font-semibold text-sm leading-none"))
}

pub fn popover_popup(
    handle: PopoverHandle,
    options: PopoverOptions,
    children: impl IntoElement,
) -> impl IntoElement {
    let panel = options.variant == Variant::Panel;
    let viewport = view()
        // Runtime positioner variables become measured limits; subtree padding variables
        // become the same literal lengths. NSH text already wraps and preserves newlines.
        .app_class(tw(&cn([
            Some("relative w-full overflow-clip"),
            panel.then_some("overflow-visible"),
            options.calendar.then_some("p-2"),
            (!options.calendar).then_some(match (panel, options.tooltip_style, options.padding) {
                (true, _, _) => "px-2 py-2",
                (false, true, Padding::Default) => "px-2 py-1",
                (false, _, Padding::Default) => "px-4 py-4",
                (false, _, Padding::Compact) => "px-3 py-2",
                (false, _, Padding::None) => "px-0 py-0",
            }),
        ])))
        .when(options.padding == Padding::None, |viewport| {
            viewport.rounded(move || inset_radius(false, false))
        })
        .child(children);
    // `overflow-y-auto` is a real scroll view; panel/tooltip viewports stay unscrolled.
    let viewport = if panel || options.tooltip_style {
        viewport.into_element()
    } else {
        scroll()
            .w_full()
            .max_h(move || handle.available.get().height)
            .child(viewport)
            .into_element()
    };
    view()
        .role(AccessibilityRole::Dialog)
        .optional_label(move || handle.title.get().map(|title| t(&title).into()))
        .app_class(tw(&cn([
            Some("relative flex rounded-lg text-popover-foreground outline-none"),
            // index.css @utility dropdown-glass; the dark numeric token is not paired by theme.
            (!panel).then_some("border backdrop-blur-(--glass-blur) dark:backdrop-blur-[16px] backdrop-saturate-[1.14] dark:backdrop-saturate-[1.08]"),
            options.calendar.then_some("rounded-xl"),
            (!panel && !options.tooltip_style).then_some(
                "shadow-[0_16px_40px_-18px_rgb(0_0_0/55%)] dark:shadow-[0_18px_44px_-18px_rgb(0_0_0/80%)]",
            ),
            options.tooltip_style.then_some("w-fit rounded-md text-xs shadow-md/5"),
            Some(match options.width {
                Size::Auto => "",
                Size::Sm => "w-64",
                Size::Md => "w-80",
                Size::Lg => "w-96",
            }),
            panel.then_some("w-full overflow-visible rounded-none border-0 bg-transparent shadow-none"),
        ])))
        .max_w(move || {
            px(handle.available.get().width.0.min(
                window().width.0 - if options.width == Size::Auto { 0.0 } else { 32.0 },
            ).max(0.0))
        })
        .when(panel, |popup| popup.w(move || px(
            (tokens().number("thread-details-panel-width").unwrap_or(320.0) + 16.0)
                .min((handle.anchor_width.get() - 8.0).max(0.0)),
        )))
        .when(!panel, |popup| {
            popup
                .bg(dropdown_glass_background)
                .border_1()
                .border_color(move || tokens().color("contrast-foreground").unwrap_or_default().opacity(0.1))
        })
        // The `before:` inset highlight is a first child. Electron app-region CSS,
        // Base UI's cross-viewport transitions and text-balance have no NSH equivalent.
        .child((!panel).then(|| inset_highlight(options.tooltip_style, options.calendar)))
        .child(viewport)
}

pub use popover_popup as popover_content;

/// `index.css`'s `dropdown-glass`: nested transparent color mixes combine their alpha.
pub(crate) fn dropdown_glass_background() -> AdaptiveColor {
    let tokens = tokens();
    // The frozen token adapter omits percentages: index.css's stock value is 80%.
    let opacity = tokens.number("glass-opacity").unwrap_or(80.0) / 100.0;
    tokens
        .color("popover")
        .unwrap_or_default()
        .opacity(0.18 + 0.82 * opacity)
}

pub(crate) fn inset_highlight(tooltip: bool, calendar: bool) -> impl IntoElement {
    view()
        .pointer_events(PointerEvents::None)
        .app_class(tw("absolute inset-0 shadow-[0_1px_0_rgb(0_0_0/4%)] dark:shadow-[0_-1px_0_rgb(255_255_255/6%)]"))
        .rounded(move || inset_radius(tooltip, calendar))
}

/// `calc(var(--radius-*) - 1px)`: NSH's arbitrary radius parser cannot evaluate calc.
pub(crate) fn inset_radius(tooltip: bool, calendar: bool) -> Points {
    let tokens = tokens();
    let base = tokens.number("radius").unwrap_or(8.0);
    let (token, fallback) = if tooltip {
        ("radius-md", base - 2.0)
    } else if calendar {
        ("radius-xl", base + 4.0)
    } else {
        ("radius-lg", base)
    };
    px((tokens.number(token).unwrap_or(fallback) - 1.0).max(0.0))
}

// NSH captures context at a component boundary, not a dynamic region forwarding children.
// These Rust-only props carry one builder without exposing signals as JS prop values.
pub(crate) fn with_popup_context<T: Clone + 'static>(
    context: T,
    build: impl FnOnce() -> AnyElement + 'static,
) -> impl IntoElement {
    PopupScope::element(PopupScopeProps(std::rc::Rc::new(std::cell::RefCell::new(
        Some(Box::new(move || {
            provide_context(context);
            build()
        })),
    ))))
}

type ScopeBuilder = Box<dyn FnOnce() -> AnyElement>;

#[derive(Clone, Default)]
struct PopupScopeProps(std::rc::Rc<std::cell::RefCell<Option<ScopeBuilder>>>);

impl PartialEq for PopupScopeProps {
    fn eq(&self, other: &Self) -> bool {
        std::rc::Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Props for PopupScopeProps {
    fn spec(_: &mut nsh_core::element::TsTypes) -> Vec<nsh_core::element::PropSpec> {
        Vec::new()
    }
    fn initial() -> Self {
        Self::default()
    }
    fn set(
        &mut self,
        name: &str,
        _: Option<nsh_core::element::AnyValue>,
    ) -> Result<bool, nsh_core::element::PropError> {
        Err(nsh_core::element::PropError(format!(
            "unknown prop `{name}`"
        )))
    }
    fn set_slot(&mut self, _: &str, _: Option<Slot>) -> bool {
        false
    }
    fn set_children(&mut self, _: Children) -> bool {
        false
    }
}

struct PopupScope;

impl Component for PopupScope {
    type Props = PopupScopeProps;
    type Event = nsh_core::element::NoEvent;

    fn view(props: Self::Props, _: Scope<Self::Event>) -> impl IntoElement {
        let child = props.0.borrow_mut().take().map(|build| build());
        view().child(child)
    }
}

#[derive(Clone, PartialEq, Props)]
struct RetainedPopoverProps {
    trigger: Prop<Option<Slot>>,
    children: Prop<Children>,
}

// NSH's Popover unmounts its children on close. Persistent forms need an overlay that
// hides without disposing its owner, and must release focus before it becomes hidden.
#[component]
fn RetainedPopover(props: RetainedPopoverProps) -> impl IntoElement {
    let (handle, options) =
        use_context::<(PopoverHandle, PopoverOptions)>().expect("popover provides retained state");
    let anchor = element_ref();
    let popup = element_ref();
    let focus_inside = signal(false);
    popup.on_focus_in(move || focus_inside.set(true));
    popup.on_focus_out(move || focus_inside.set(false));
    effect(move || {
        if !handle.is_open() && focus_inside.get() {
            anchor.focus();
        }
    });
    view()
        .child(props.trigger.place(move |trigger| {
            trigger
                .element_ref(&anchor)
                .on_layout(move |bounds: &Bounds<Points>| {
                    handle.anchor_width.set(bounds.size.width.0)
                })
                .accessibility_state(move || AccessibilityState {
                    expanded: Some(handle.is_open()),
                    has_popup: true,
                    ..Default::default()
                })
                .on_press(move || handle.set_open(!handle.open.get_untracked()))
        }))
        // Mount before the retained popup: a backdrop created on open would cover it.
        .child(options.touch_backdrop.then(|| {
            overlay()
                .inset_0()
                .z_index(130)
                .focusable(false)
                .pointer_events(move || {
                    if pointer() == Pointer::Coarse && handle.is_open() {
                        PointerEvents::Auto
                    } else {
                        PointerEvents::None
                    }
                })
                .on_press(move || {
                    focus_inside.set(false);
                    handle.set_open(false);
                })
        }))
        .child(
            overlay()
                .anchor_content(&anchor)
                .placement(options.side)
                .align(options.align)
                .offset(px(options.side_offset))
                .margin(px(if options.variant == Variant::Panel {
                    0.0
                } else {
                    5.0
                }))
                .element_ref(&popup)
                .app_class(move || {
                    Some(tw(if handle.is_open() {
                        if options.variant == Variant::Panel {
                            "z-[46]"
                        } else {
                            "z-[130]"
                        }
                    } else {
                        "hidden"
                    }))
                })
                .accessibility_elements_hidden(move || !handle.is_open())
                .pointer_events(move || {
                    if handle.is_open() {
                        PointerEvents::Auto
                    } else {
                        PointerEvents::None
                    }
                })
                .on_position(move |position: &OverlayPosition| {
                    handle.available.set(position.available)
                })
                .on_cancel(move || {
                    if handle.open.get_untracked() {
                        handle.set_open(false);
                    }
                })
                .on_press_outside(move |_: &Point<Points>| {
                    if handle.open.get_untracked()
                        && (pointer() == Pointer::Fine || !options.touch_backdrop)
                    {
                        focus_inside.set(false);
                        handle.set_open(false);
                    }
                })
                .child(props.children),
        )
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewCardOptions {
    pub open: Option<Signal<bool>>,
    pub side: Placement,
    pub align: Align,
    pub side_offset: f32,
    pub open_delay: f64,
    pub close_delay: f64,
}

impl Default for PreviewCardOptions {
    fn default() -> Self {
        Self {
            open: None,
            side: Placement::Top,
            align: Align::Start,
            side_offset: 6.0,
            open_delay: 600.0,
            close_delay: 300.0,
        }
    }
}

/// Port of apps/web/src/components/ui/preview-card.tsx PreviewCard.
pub fn preview_card(
    options: PreviewCardOptions,
    trigger: impl IntoElement,
    content: impl Fn() -> AnyElement + 'static,
) -> impl IntoElement {
    nsh_base::hover_card()
        .open(move || options.open.map(|open| open.get()))
        .on_open_change(move |event: &nsh_base::HoverCardEvent| {
            if let nsh_base::HoverCardEvent::OpenChange(next, _) = event
                && let Some(open) = options.open
            {
                open.set(*next);
            }
        })
        .placement(options.side)
        .align(options.align)
        .offset(px(options.side_offset))
        .margin(px(5.0))
        .open_delay(options.open_delay)
        .close_delay(options.close_delay)
        // NSH's standard transition covers the overlay's changing transform and opacity.
        .popup_class(tw("z-[140] transition"))
        .popup_starting_class(tw("scale-98 opacity-0"))
        .popup_ending_class(tw("scale-98 opacity-0"))
        .trigger(trigger)
        .child(dynamic(move || {
            untracked(|| preview_card_popup(content()).into_element())
        }))
}

/// Port of apps/web/src/components/ui/preview-card.tsx PreviewCardTrigger.
pub fn preview_card_trigger(
    label: impl Into<String>,
    href: Option<SharedString>,
    open_with: nsh_base::LinkOpenWith,
    on_open: impl Fn(&SharedString) + 'static,
    children: impl IntoElement,
) -> impl IntoElement {
    let label = label.into();
    // NSH Link supplies native navigation and its destination hint when href is present.
    nsh_base::link()
        .href(href)
        .open_with(open_with)
        .on_open(on_open)
        .accessibility_label(move || Some(t(&label).into()))
        .app_class(tw(
            "focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
        ))
        .child(children)
}

/// Port of apps/web/src/components/ui/preview-card.tsx PreviewCardPopup.
pub fn preview_card_popup(children: impl IntoElement) -> impl IntoElement {
    // NSH handles overlay placement without CSS origin variables or a closing positioner.
    view()
        .app_class(tw(
            "rounded-lg border bg-popover text-popover-foreground shadow-lg outline-none",
        ))
        .max_w(move || px((window().width.0 - 10.0).max(0.0)))
        .child(children)
}
