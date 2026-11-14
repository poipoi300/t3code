//! Port of apps/web/src/components/ui/tooltip.tsx Tooltip.
//!
//! `tooltip_create_handle()` connects `tooltip_trigger(handle, label, disabled, popup, children)`
//! to `tooltip(handle, options, content)`. The latter is both an element and an `IntoTooltip`,
//! so existing controls can attach it with `.tooltip(..)` without a second button.
//! `tooltip_popup(Variant::{Default, Glass, Code}, content)` is the styled popup slot; there is
//! no size prop in the TSX. Labels are English translation keys; callers translate content.
//!
//! Construct tooltip handles inside the lazy children of `tooltip_provider(delay, children)`
//! and `tooltip_scroll_dismiss_area(target, children)` to inherit them. `target` names the
//! caller's existing scroll view or virtual list, so the wrapper adds no scroll container.
//! Scrolling closes hovered hints (including pending opens) while preserving a keyboard-focused
//! hint. NSH owns hint delays, focus-visible activation, Escape and grace time.
//! NSH exposes no focus-visible observer: unknown programmatic focus retains the hint on scroll.

use nsh_core::element::IntoTooltip;
use nsh_core::prelude::*;

use super::popover::{
    dropdown_glass_background, inset_highlight, inset_radius, with_popup_context,
};
use crate::i18n::t;
use crate::theme::{cn, palette, tw};

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

#[derive(Clone, Copy, PartialEq)]
enum FocusOrigin {
    Outside,
    Pointer,
    Keyboard,
    Unknown,
}

#[derive(Clone, Copy)]
pub struct TooltipHandle {
    open: Signal<bool>,
    disabled: Signal<bool>,
    dismissed: Signal<bool>,
    focus: Signal<FocusOrigin>,
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
        focus: signal(FocusOrigin::Outside),
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
        // NSH does not expose focus-visible for programmatic focus. Retain unknown
        // focus rather than taking a keyboard user's description away on scroll.
        if matches!(
            handle.focus.get_untracked(),
            FocusOrigin::Keyboard | FocusOrigin::Unknown
        ) {
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
                    handle.focus.set(if *open {
                        FocusOrigin::Keyboard
                    } else {
                        FocusOrigin::Outside
                    });
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
    // NSH only accepts a uniform margin beyond the native safe area. A top-only
    // titlebar exclusion needs per-edge collision insets in TooltipProps/OverlayConfig.
    tooltip
        .placement(options.side)
        .align(options.align)
        .offset(px(options.side_offset))
        .margin(px(5.0))
        .app_class(tw("pointer-events-none z-[140] transition"))
}

pub fn tooltip_trigger(
    handle: TooltipHandle,
    label: impl Into<String>,
    disabled: impl Fn() -> bool + 'static,
    popup: impl IntoTooltip + IntoElement + 'static,
    children: impl IntoElement + 'static,
) -> impl IntoElement {
    let label = label.into();
    let trigger = element_ref();
    trigger.on_focus_in(move || {
        handle.dismissed.set(false);
        if handle.focus.get_untracked() == FocusOrigin::Outside {
            handle.focus.set(FocusOrigin::Unknown);
        }
    });
    trigger.on_focus_out(move || handle.focus.set(FocusOrigin::Outside));
    // NSH Button cannot host `.tooltip`; its fitted wrapper anchors the hint and
    // observes focus within the real button without introducing a second control.
    view()
        .element_ref(&trigger)
        .app_class(tw("w-fit"))
        .tooltip(popup)
        .on_mouse_enter(move |_: &MouseEvent| handle.hover())
        .on_press_down(move |_: &PressEvent| handle.focus.set(FocusOrigin::Pointer))
        // An already-hovered hint emits no second OpenChange when Tab focuses it.
        // Key-up observes Tab after focus moved onto this trigger.
        .on_key_down(move |_: &nsh_core::KeyEvent| handle.focus.set(FocusOrigin::Keyboard))
        .on_key_up(move |_: &nsh_core::KeyEvent| handle.focus.set(FocusOrigin::Keyboard))
        .child(
            nsh_base::button()
                .accessibility_label(move || Some(t(&label).into()))
                .disabled(disabled)
                .child(children),
        )
}

pub fn tooltip_popup(variant: Variant, children: impl IntoElement) -> impl IntoElement {
    // NSH parses text-balance as ordinary wrapping, without balanced line breaking.
    // Transform-origin/positioner interpolation are unsupported; wrap-anywhere and
    // whitespace-normal are omitted per the native wrapping rule in ui-package.md.
    view()
        .app_class(tw(&cn([
            Some("relative flex text-balance rounded-md text-popover-foreground text-xs"),
            Some(if variant == Variant::Glass {
                "shadow-xl shadow-black/25 border backdrop-blur-(--glass-blur) dark:backdrop-blur-[16px] backdrop-saturate-[1.14] dark:backdrop-saturate-[1.08]"
            } else {
                "border shadow-md/5"
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
        // NSH has no background-clip: light paints only the padding box; dark keeps
        // the border-box fill, matching not-dark:bg-clip-padding in TooltipPopup.
        .when(variant != Variant::Glass, |popup| popup.bg(move || AdaptiveColor::new(
            AdaptiveColor::TRANSPARENT.light, palette().popover.dark,
        )))
        .child((variant != Variant::Glass).then(|| view()
            .pointer_events(PointerEvents::None)
            .app_class(tw("absolute inset-px"))
            .rounded(move || inset_radius(true, false))
            .bg(move || AdaptiveColor::new(palette().popover.light, AdaptiveColor::TRANSPARENT.dark))))
        .child((variant != Variant::Glass).then(|| inset_highlight(true, false)))
        .child(view().app_class(tw("min-w-0 px-2 py-1")).child(children))
}

pub fn tooltip_provider(
    delay: f64,
    children: impl Fn() -> AnyElement + 'static,
) -> impl IntoElement {
    with_popup_context(TooltipProviderDelay(delay), children)
}

pub fn tooltip_scroll_dismiss_area(
    target: ElementRef,
    children: impl Fn() -> AnyElement + 'static,
) -> impl IntoElement {
    with_popup_context((), move || {
        let tracker = TooltipHoverTracker(stored(None));
        provide_context(tracker);
        let previous = stored(None);
        // NSH has no scroll capture phase. Watch the existing scroller without replacing
        // its on_scroll handler; initial layout and size-only changes are not scrolls.
        watch_scroll(
            move || target.node(),
            move |event| {
                if previous
                    .replace(Some(event.offset))
                    .is_some_and(|offset| offset != event.offset)
                {
                    tracker.dismiss_unfocused();
                }
            },
        );
        children()
    })
}
