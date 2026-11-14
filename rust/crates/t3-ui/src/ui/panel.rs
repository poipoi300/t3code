//! Port of apps/web/src/components/RightPanelSheet.tsx RightPanelSheet.
//!
//! [`panel`] takes controlled `open`, `animation_duration_ms`, `on_close`, and one `children`
//! builder slot. It returns a pane on Wide, [`right_panel_sheet`] on Medium, and a native
//! navigation page on Compact. Put the compact result directly in the shell's `nav_stack`,
//! alongside its thread page; `on_close` must remove that page from the caller's route stack.
//! The inline pane's width and resize handle belong to its host (PNL-HOST / UI-RESIZE).
//! Neither TSX component declares variants or sizes, so there are no `Variant` / `Size` enums.
//!
//! Children are constructed once and passed through a non-lazy NSH child slot. The slot owns
//! their state while a closed surface leaves them detached; changing presentation reparents
//! the same nodes. Keep this panel instance mounted in the host across layout changes. A host
//! that changes its own wrappers can forward the panel through its own non-lazy child slot.

use std::rc::Rc;

use nsh_core::element::desktop::title_bar;
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
    panel_root()
        .open(open)
        .animation_duration_ms(animation_duration_ms)
        .adaptive(true)
        .on_close(on_close)
        .child(untracked(children))
}

/// `RightPanelSheet`: a named, controlled right sheet with no generated close button.
pub fn right_panel_sheet<E: IntoElement + 'static>(
    open: impl Fn() -> bool + 'static,
    animation_duration_ms: u32,
    on_close: impl Fn() + 'static,
    children: impl Fn() -> E + 'static,
) -> impl IntoElement {
    panel_root()
        .open(open)
        .animation_duration_ms(animation_duration_ms)
        .adaptive(false)
        .on_close(on_close)
        .child(untracked(children))
}

#[derive(Clone, PartialEq, Props)]
struct PanelRootProps {
    open: Prop<bool>,
    animation_duration_ms: u32,
    adaptive: bool,
    children: Prop<Children>,
}

#[derive(Clone, Debug, PartialEq, Event)]
enum PanelRootEvent {
    Close,
}

// A non-lazy slot keeps the content owned here while surface wrappers come and go.
#[component]
fn PanelRoot(props: PanelRootProps, cx: Scope<PanelRootEvent>) -> impl IntoElement {
    let presentation = computed(move || {
        if props.adaptive {
            layout_class()
        } else {
            LayoutClass::Medium
        }
    });
    dynamic(move || {
        let presentation = presentation.get();
        // Only the presentation changes this subtree; content bindings track their own data.
        untracked(|| match presentation {
            LayoutClass::Wide => when(
                move || props.open.get(),
                move || {
                    view()
                        .app_class(tw("flex min-h-0 min-w-0 h-full flex-col"))
                        .child(props.children)
                },
            )
            .into_element(),
            LayoutClass::Medium => sheet_surface(
                props.open,
                props.animation_duration_ms,
                move || cx.emit(PanelRootEvent::Close),
                props.children,
            )
            .into_element(),
            // Adaptive departure: the outer navigation stack pushes this page over the thread.
            LayoutClass::Compact => when(
                move || props.open.get(),
                move || {
                    nsh_base::nav_page()
                        .title(move || SharedString::from(t("Workspace panel")))
                        .pop_on_back(false)
                        .on_back_press(move || cx.emit(PanelRootEvent::Close))
                        .on_dismiss(move || {
                            if props.open.get_untracked() {
                                cx.emit(PanelRootEvent::Close);
                            }
                        })
                        .child(
                            view()
                                .app_class(tw("flex min-h-0 min-w-0 flex-1 flex-col"))
                                .child(props.children),
                        )
                },
            )
            .into_element(),
        })
    })
}

fn sheet_surface(
    open: Prop<bool>,
    animation_duration_ms: u32,
    on_close: impl Fn() + 'static,
    children: Prop<Children>,
) -> impl IntoElement {
    let open = computed(move || open.get());
    let shown = signal(false);
    let progress = animated_value(0.0);
    let translate_x = progress.interpolate(&[0.0, 1.0], &[32.0, 0.0]);
    let on_close = Rc::new(on_close);
    effect(move || {
        let next = open.get();
        if !next && !shown.get_untracked() {
            return;
        }
        if next {
            shown.set(true);
        }
        let duration = if reduce_motion() {
            0
        } else {
            animation_duration_ms
        };
        if duration == 0 {
            progress.set(if next { 1.0 } else { 0.0 });
            shown.set(next);
            return;
        }
        let end = progress.timing(
            if next { 1.0 } else { 0.0 },
            duration as f32,
            Easing::EaseInOut,
        );
        spawn(async move {
            if end.await && !open.get_untracked() {
                shown.set(false);
            }
        });
    });
    when(shown, move || {
        let cancel = on_close.clone();
        let backdrop_press = on_close.clone();
        let escape = on_close.clone();
        // NSH's styled modal supplies focus containment/restoration; its Sheet has no title slot.
        nsh_base::dialog()
            .label(move || SharedString::from(t("Workspace panel")))
            .animation(ModalAnimation::None)
            .close_on_escape(false)
            .close_on_backdrop_press(false)
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
                    // wco's topbar margin/height becomes NSH's reactive title-bar inset.
                    .test_id("workspace-panel-popup")
                    .top(move || title_bar().top)
                    .right_0()
                    // Match nsh-base's Sheet keyboard_lift for a panel docked at the right.
                    .bottom(move || (keyboard_height() - safe_area().bottom).max(Points::ZERO))
                    // Viewport min()/vw lengths are resolved from the same window width in NSH.
                    .w(move || px(sheet_width(window().width.0)))
                    // will-change-transform is dropped: native animation bindings own scheduling.
                    // NSH has no background-clip property (`not-dark:bg-clip-padding`).
                    .app_class(tw("flex max-h-full min-h-0 min-w-0 flex-col bg-popover text-popover-foreground shadow-lg/5 border-s min-w-80 max-w-[28rem] max-[760px]:min-w-0"))
                    .disabled(move || !open.get())
                    .bind_opacity(&progress)
                    .bind_translate_x(&translate_x)
                    // SheetPopup's before shadow is an explicit first child; max-sm hides it.
                    .child(view().app_class(tw("pointer-events-none absolute inset-0 shadow-[0_1px_rgb(0_0_0/4%)] dark:shadow-[0_-1px_rgb(255_255_255/6%)] max-sm:hidden")))
                    // SheetHeader's sr-only SheetTitle becomes the dialog's accessible label.
                    // NSH consumes an editor's Escape; bind inside the modal's window context.
                    .child(
                        sheet_content()
                            .open(move || open.get())
                            .on_close(move || escape())
                            .child(children),
                    ),
            )
    })
}

#[derive(Clone, PartialEq, Props)]
struct SheetContentProps {
    open: Prop<bool>,
    children: Prop<Children>,
}

#[derive(Clone, Debug, PartialEq, Event)]
enum SheetContentEvent {
    Close,
}

// A component mounts under the modal's WindowRef; a function called while building cannot.
#[component]
fn SheetContent(props: SheetContentProps, cx: Scope<SheetContentEvent>) -> impl IntoElement {
    on_key_with(
        "escape",
        KeyOptions {
            when: nsh_core::keymap::When::Always,
            ..Default::default()
        },
        move |_| {
            if props.open.get_untracked() {
                cx.emit(SheetContentEvent::Close);
            }
        },
    );
    view()
        .app_class(tw("flex min-h-0 min-w-0 flex-1 flex-col"))
        .child(props.children)
}

/// `RightPanelSheet`'s 760 px media query, 28rem cap and narrow 24rem cap (16 px root).
pub fn sheet_width(viewport_width: f32) -> f32 {
    if viewport_width < 760.0 {
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
            (759.0, 384.0),
            (760.0, 320.0),
            (761.0, 320.0),
            (800.0, 336.0),
            (1024.0, 430.08),
            (1280.0, 448.0),
        ] {
            assert!(
                (sheet_width(viewport) - expected).abs() < 0.01,
                "{viewport}"
            );
        }
    }
}
