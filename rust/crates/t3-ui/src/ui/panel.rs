//! Port of apps/web/src/components/RightPanelSheet.tsx RightPanelSheet.
//!
//! [`panel`] takes controlled `open`, `animation_duration_ms`, `on_close`, and one `children`
//! builder slot. It returns a pane on Wide, [`right_panel_sheet`] on Medium, and a native
//! navigation page on Compact. Put the compact result directly in the shell's `nav_stack`,
//! alongside its thread page; `on_close` must remove that page from the caller's route stack.
//! The inline pane's width and resize handle belong to its host (PNL-HOST / UI-RESIZE).
//! Neither TSX component declares variants or sizes, so there are no `Variant` / `Size` enums.
//!
//! Keep edit state outside `children`: the pinned NSH modal cannot retain a mounted subtree
//! after dismissal or reparent it across presentations. Children remain through the exit
//! animation, but are constructed again on reopening or changing layout class.

use std::rc::Rc;

use nsh_core::prelude::*;

use crate::i18n::t;
use crate::state::layout::{LayoutClass, layout_class};
use crate::theme::tw;

/// The adaptive container; the caller owns visibility, route changes and the content's state.
pub fn panel<E: IntoElement + 'static>(
    open: impl Fn() -> bool + 'static,
    animation_duration_ms: u32,
    on_close: impl Fn() + 'static,
    children: impl Fn() -> E + 'static,
) -> impl IntoElement {
    let open = computed(open);
    let presentation = computed(layout_class);
    let on_close = Rc::new(on_close);
    let children = Rc::new(children);
    dynamic(move || {
        let presentation = presentation.get();
        let on_close = on_close.clone();
        let children = children.clone();
        // Only the presentation changes this subtree; content bindings track their own data.
        untracked(|| match presentation {
            LayoutClass::Wide => when(open, move || {
                view()
                    .app_class(tw("flex min-h-0 min-w-0 h-full flex-col"))
                    .child(untracked(|| children()))
            })
            .into_element(),
            LayoutClass::Medium => right_panel_sheet(
                move || open.get(),
                animation_duration_ms,
                move || on_close(),
                move || children(),
            )
            .into_element(),
            // Adaptive departure: the outer navigation stack pushes this page over the thread.
            LayoutClass::Compact => when(open, move || {
                let on_dismiss = on_close.clone();
                nsh_base::nav_page()
                    .title(SharedString::from(t("Workspace panel")))
                    .on_dismiss(move || on_dismiss())
                    .child(
                        view()
                            .app_class(tw("flex min-h-0 min-w-0 flex-1 flex-col"))
                            .child(untracked(|| children())),
                    )
            })
            .into_element(),
        })
    })
}

/// `RightPanelSheet`: a named, controlled right sheet with no generated close button.
pub fn right_panel_sheet<E: IntoElement + 'static>(
    open: impl Fn() -> bool + 'static,
    animation_duration_ms: u32,
    on_close: impl Fn() + 'static,
    children: impl Fn() -> E + 'static,
) -> impl IntoElement {
    let open = computed(open);
    let shown = signal(false);
    let progress = animated_value(0.0);
    let translate_x = progress.interpolate(&[0.0, 1.0], &[32.0, 0.0]);
    let on_close = Rc::new(on_close);
    effect(move || {
        let next = open.get();
        if next {
            shown.set(true);
        }
        let duration = if reduce_motion() { 0 } else { animation_duration_ms };
        if duration == 0 {
            progress.set(if next { 1.0 } else { 0.0 });
            shown.set(next);
            return;
        }
        let end = progress.timing(if next { 1.0 } else { 0.0 }, duration as f32, Easing::EaseInOut);
        spawn(async move {
            if end.await && !open.get_untracked() {
                shown.set(false);
            }
        });
    });
    when(shown, move || {
        let cancel = on_close.clone();
        let backdrop_press = on_close.clone();
        // NSH's styled modal supplies focus containment/restoration; its Sheet has no title slot.
        nsh_base::dialog()
            .label(move || SharedString::from(t("Workspace panel")))
            .animation(ModalAnimation::None)
            .close_on_escape(false)
            .close_on_swipe(false)
            .on_cancel(move || {
                if open.get_untracked() {
                    cancel();
                }
            })
            // SheetPortal / SheetViewport are native overlay layers, rather than DOM wrappers.
            .child(
                overlay()
                    .inset_0()
                    .focusable(false)
                    // Transition classes become finite native bindings; z-order is the modal's.
                    .app_class(tw("bg-background/60 backdrop-blur-xs"))
                    .bind_opacity(&progress)
                    .on_press(move || {
                        if open.get_untracked() {
                            backdrop_press();
                        }
                    }),
            )
            .child(
                overlay()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    // Viewport min()/vw lengths are resolved from the same window width in NSH.
                    .w(move || px(sheet_width(window().width.0)))
                    // wco insets are supplied by the shared shell, not a browser CSS variant.
                    // will-change-transform is dropped: native animation bindings own scheduling.
                    .app_class(tw("flex max-h-full min-h-0 min-w-0 flex-col bg-popover not-dark:bg-clip-padding text-popover-foreground shadow-lg/5 border-s"))
                    .bind_opacity(&progress)
                    .bind_translate_x(&translate_x)
                    // SheetPopup's before shadow is an explicit first child; max-sm hides it.
                    .child(view().app_class(tw("pointer-events-none absolute inset-0 shadow-[0_1px_rgb(0_0_0/4%)] dark:shadow-[0_-1px_rgb(255_255_255/6%)] max-sm:hidden")))
                    // SheetHeader's sr-only SheetTitle becomes the dialog's accessible label.
                    .child(untracked(|| children())),
            )
    })
}

/// `RightPanelSheet`'s 760 px media query, 28rem cap and narrow 24rem cap (16 px root).
pub fn sheet_width(viewport_width: f32) -> f32 {
    if viewport_width <= 760.0 {
        (viewport_width * 0.88).min(384.0)
    } else {
        (viewport_width * 0.42).clamp(320.0, 448.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_width_matches_the_web_media_query_and_caps() {
        for (viewport, expected) in [
            (390.0, 343.2),
            (640.0, 384.0),
            (760.0, 384.0),
            (761.0, 320.0),
            (800.0, 336.0),
            (1024.0, 430.08),
            (1280.0, 448.0),
        ] {
            assert!((sheet_width(viewport) - expected).abs() < 0.01, "{viewport}");
        }
    }
}
