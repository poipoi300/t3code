//! Port of apps/web/src/components/ui/tooltip.tsx Tooltip.
//!
//! `tooltip_create_handle()` connects `tooltip_trigger(handle, label, disabled, popup, children)`
//! to `tooltip(handle, options, content)`. The latter is both an element and an `IntoTooltip`,
//! so existing controls can attach it with `.tooltip(..)` without a second button.
//! `tooltip_popup(Variant::{Default, Glass, Code}, content)` is the styled popup slot; there is
//! no size prop in the TSX. Labels are English translation keys; callers translate content.
//!
//! Construct tooltip handles inside the lazy children of `tooltip_provider(delay, children)`
//! and `tooltip_scroll_dismiss_area(children)` to inherit them. The dismiss area is a scroll
//! view: on scrolling it closes hovered hints (including pending opens), while preserving a
//! keyboard-focused hint. NSH owns hint delays, focus-visible activation, Escape and grace time.

use nsh_core::element::IntoTooltip;
use nsh_core::element::desktop::title_bar;
use nsh_core::prelude::*;

use super::popover::{dropdown_glass_background, inset_highlight, with_popup_context};
use crate::i18n::t;
use crate::theme::{cn, tw};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Variant {
    #[default]
    Default,
    Glass,
    Code,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TooltipOptions {
    pub variant: Variant,
    pub side: Placement,
    pub align: Align,
    pub side_offset: f32,
    /// None inherits TooltipProvider, then Base UI's 600 ms default.
    pub delay: Option<f64>,
}

impl Default for TooltipOptions {
    fn default() -> Self {
        Self {
            variant: Variant::Default,
            side: Placement::Top,
            align: Align::Center,
            side_offset: 4.0,
            delay: None,
        }
    }
}

#[derive(Clone, Copy)]
struct TooltipProviderDelay(f64);

#[derive(Clone, Copy)]
pub struct TooltipHandle {
    open: Signal<bool>,
    disabled: Signal<bool>,
    dismissed: Signal<bool>,
    keyboard_focus: Signal<bool>,
    tracker: Option<TooltipHoverTracker>,
}

impl PartialEq for TooltipHandle {
    fn eq(&self, other: &Self) -> bool {
        self.open == other.open
    }
}

pub fn tooltip_create_handle() -> TooltipHandle {
    let handle = TooltipHandle {
        open: signal(false),
        disabled: signal(false),
        dismissed: signal(false),
        keyboard_focus: signal(false),
        tracker: use_context::<TooltipHoverTracker>(),
    };
    if let Some(tracker) = handle.tracker {
        cleanup(move || tracker.release(handle));
    }
    handle
}

impl TooltipHandle {
    pub fn is_open(self) -> bool {
        self.open.get() && !self.disabled.get() && !self.dismissed.get()
    }

    pub fn set_open(self, open: bool) {
        if !open || !self.disabled.get_untracked() {
            self.dismissed.set(false);
            self.open.set(open);
        }
    }

    pub fn set_disabled(self, disabled: bool) {
        self.disabled.set(disabled);
        if disabled {
            self.open.set(false);
        }
    }

    fn hover(self) {
        self.dismissed.set(false);
        if let Some(tracker) = self.tracker {
            tracker.hover(self);
        }
    }
}

#[derive(Clone, Copy)]
struct TooltipHoverTracker(Stored<Option<TooltipHandle>>);

impl TooltipHoverTracker {
    fn hover(self, handle: TooltipHandle) {
        self.0.replace(Some(handle));
    }

    fn release(self, handle: TooltipHandle) {
        self.0.with(|current| {
            if current.is_some_and(|current| current.open == handle.open) {
                *current = None;
            }
        });
    }

    fn dismiss_unfocused(self) {
        let Some(handle) = self.0.get() else { return };
        if handle.keyboard_focus.get_untracked() {
            return;
        }
        self.0.replace(None);
        // Disabling the native hint also cancels a hover whose delay has not fired yet.
        handle.dismissed.set(true);
        handle.open.set(false);
    }
}

pub fn tooltip(
    handle: TooltipHandle,
    options: TooltipOptions,
    children: impl IntoElement,
) -> impl IntoElement + IntoTooltip {
    let delay = options.delay.unwrap_or_else(|| {
        use_context::<TooltipProviderDelay>().map_or(600.0, |provider| provider.0)
    });
    let tooltip = nsh_base::tooltip()
        .open(move || Some(handle.is_open()))
        .disabled(move || handle.disabled.get() || handle.dismissed.get())
        .delay(delay)
        .starting_class(tw("scale-98 opacity-0"))
        .ending_class(tw("scale-98 opacity-0"))
        .on_open_change(move |event: &nsh_base::TooltipEvent| {
            if let nsh_base::TooltipEvent::OpenChange(open, reason) = event {
                if *reason == nsh_base::OpenChangeReason::Focus {
                    handle.keyboard_focus.set(*open);
                }
                if !*open || (!handle.disabled.get_untracked() && !handle.dismissed.get_untracked())
                {
                    handle.open.set(*open);
                }
                if !open && let Some(tracker) = handle.tracker {
                    tracker.release(handle);
                }
            }
        })
        .child(tooltip_popup(options.variant, children));
    tooltip_positioner(options, tooltip)
}

fn tooltip_positioner(
    options: TooltipOptions,
    tooltip: nsh_base::TooltipElement,
) -> nsh_base::TooltipElement {
    // Portal/Positioner share NSH's hint overlay. Its margin is uniform: reserve the
    // title-bar inset on every edge instead of Base UI's top-only collision padding.
    tooltip
        .placement(options.side)
        .align(options.align)
        .offset(px(options.side_offset))
        .margin(move || px(5.0 + title_bar().top.0))
        .app_class(tw("pointer-events-none z-[140] transition-[scale,opacity]"))
}

pub fn tooltip_trigger(
    handle: TooltipHandle,
    label: impl Into<String>,
    disabled: impl Fn() -> bool + 'static,
    popup: impl IntoTooltip + IntoElement + 'static,
    children: impl IntoElement + 'static,
) -> impl IntoElement {
    let label = label.into();
    with_popup_context(handle, move || {
        tooltip_trigger_root()
            .popup(popup)
            .trigger(
                nsh_base::button()
                    .accessibility_label(move || Some(t(&label).into()))
                    .disabled(disabled)
                    .app_class(tw(
                        "focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
                    ))
                    .child(children),
            )
            .into_element()
    })
}

#[derive(Clone, PartialEq, Props)]
struct TooltipTriggerRootProps {
    trigger: Prop<Option<Slot>>,
    popup: Prop<Option<Slot>>,
}

// The slot already contains a native hint overlay; wrapping it in another hint would
// impose a second delay and lose its controlled focus and dismissal state.
struct TooltipSlot(AnyElement);

impl IntoTooltip for TooltipSlot {
    fn apply_tooltip(self, node: &mut nsh_core::element::ElNode) {
        nsh_core::element::__hint_tooltip(node, self.0);
    }
}

#[component]
fn TooltipTriggerRoot(props: TooltipTriggerRootProps) -> impl IntoElement {
    let handle = use_context::<TooltipHandle>().expect("tooltip_trigger provides its handle");
    props.trigger.place(move |trigger| {
        trigger
            .on_mouse_enter(move |_: &MouseEvent| handle.hover())
            .on_press_down(move |_: &PressEvent| handle.keyboard_focus.set(false))
            .on_focus(move || handle.dismissed.set(false))
            .on_blur(move || handle.keyboard_focus.set(false))
            .tooltip(TooltipSlot(props.popup.place(|popup| popup)))
    })
}

pub fn tooltip_popup(variant: Variant, children: impl IntoElement) -> impl IntoElement {
    // CSS transform-origin/positioner interpolation and text-balance have no NSH counterpart;
    // NSH text wraps natively, so wrap-anywhere and whitespace-normal are omitted.
    view()
        .app_class(tw(&cn([
            Some("relative flex rounded-md text-popover-foreground text-xs"),
            Some(if variant == Variant::Glass {
                "shadow-xl shadow-black/25 border backdrop-blur-(--glass-blur) dark:backdrop-blur-[16px] backdrop-saturate-[1.14] dark:backdrop-saturate-[1.08]"
            } else {
                "border bg-popover bg-clip-padding dark:bg-clip-border shadow-md/5"
            }),
            Some(if variant == Variant::Code {
                "max-w-120 text-left font-mono text-[11px] leading-relaxed"
            } else {
                "max-w-80 leading-snug"
            }),
        ])))
        .max_w(move || {
            px((window().width.0 - 10.0)
                .max(0.0)
                .min(if variant == Variant::Code {
                    480.0
                } else {
                    320.0
                }))
        })
        .when(variant == Variant::Glass, |popup| {
            // `index.css` @utility dropdown-glass, as in PopoverPopup.
            popup
                .bg(dropdown_glass_background)
                .border_1()
                .border_color(move || {
                    tokens()
                        .color("contrast-foreground")
                        .unwrap_or_default()
                        .opacity(0.1)
                })
        })
        .child((variant != Variant::Glass).then(|| inset_highlight(true, false)))
        .child(view().app_class(tw("min-w-0 px-2 py-1")).child(children))
}

pub fn tooltip_provider(
    delay: f64,
    children: impl Fn() -> AnyElement + 'static,
) -> impl IntoElement {
    tooltip_provider_root()
        .delay(delay)
        .child(dynamic(move || untracked(&children)))
}

#[derive(Clone, PartialEq, Props)]
struct TooltipProviderRootProps {
    delay: f64,
    children: Prop<Children>,
}

#[component]
fn TooltipProviderRoot(props: TooltipProviderRootProps) -> impl IntoElement {
    provide_context(TooltipProviderDelay(props.delay));
    props.children
}

pub fn tooltip_scroll_dismiss_area(
    children: impl Fn() -> AnyElement + 'static,
) -> impl IntoElement {
    tooltip_scroll_dismiss_root().child(dynamic(move || untracked(&children)))
}

#[derive(Clone, PartialEq, Props)]
struct TooltipScrollDismissRootProps {
    children: Prop<Children>,
}

#[component]
fn TooltipScrollDismissRoot(props: TooltipScrollDismissRootProps) -> impl IntoElement {
    let tracker = TooltipHoverTracker(stored(None));
    provide_context(tracker);
    // NSH scroll events do not have a capture phase: this area owns the scroll view.
    scroll()
        .h_full()
        .min_h(px(0.0))
        .test_id("tooltip-scroll-dismiss-area")
        .on_scroll(move |_: &ScrollEvent| tracker.dismiss_unfocused())
        .child(props.children)
}
