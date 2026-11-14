//! Port of apps/web/src/components/ui/scroll-area.tsx ScrollArea and ScrollBar.
//!
//! `scroll_area(scroll_fade, children)` is the common vertical composition; use
//! `scroll_area_with_options` for a horizontal viewport, a caller ref, hidden scrollbars or
//! a focusable viewport. `scroll_bar` can also follow an existing virtual list without adding
//! a scroll container. Native scrollbars own track presses, dragging and offset observation.
//! NSH has no opacity-mask or scroll-padding style: fade edge data is exposed, but the source's
//! translucent content masks cannot be painted by this pin. No opaque overlay substitutes them.

use crate::i18n::Text;
use crate::theme::tw;
use nsh_base::{ScrollbarMode, ScrollbarMotion};
use nsh_core::{AccessibilityState, element::ScrollEvent, geometry::Axis, prelude::*};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Radius {
    #[default]
    Inherit,
    None,
}

#[derive(Clone)]
pub struct ScrollAreaOptions {
    pub scroll_fade: bool,
    pub scroll_fade_padding: bool,
    pub scrollbar_gutter: bool,
    pub hide_scrollbars: bool,
    pub chain_vertical_scroll: bool,
    pub radius: Radius,
    pub viewport_tab_index: Option<i32>,
    pub horizontal: bool,
    pub viewport_ref: Option<ElementRef>,
    pub label: Option<Text>,
    pub role: AccessibilityRole,
    pub busy: Prop<bool>,
    pub on_mouse_leave: Option<Rc<dyn Fn()>>,
}

impl Default for ScrollAreaOptions {
    fn default() -> Self {
        Self {
            scroll_fade: false,
            scroll_fade_padding: true,
            scrollbar_gutter: false,
            hide_scrollbars: false,
            chain_vertical_scroll: false,
            radius: Radius::Inherit,
            viewport_tab_index: None,
            horizontal: false,
            viewport_ref: None,
            label: None,
            role: AccessibilityRole::None,
            busy: false.into(),
            on_mouse_leave: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FadeEdges {
    pub top: f32,
    pub bottom: f32,
    pub left: f32,
    pub right: f32,
}

impl FadeEdges {
    pub fn from_scroll(event: &ScrollEvent) -> Self {
        // CSS's min(fade-size, overflow-start/end) is distance-based, including partial fades.
        let cap = 1.5 * nsh_core::geometry::REM_SIZE.0;
        Self {
            top: event.offset.y.0.clamp(0.0, cap),
            bottom: (event.content_size.height.0 - event.size.height.0 - event.offset.y.0)
                .clamp(0.0, cap),
            left: event.offset.x.0.clamp(0.0, cap),
            right: (event.content_size.width.0 - event.size.width.0 - event.offset.x.0)
                .clamp(0.0, cap),
        }
    }
}

/// Native equivalent of the source helper's four conditional mask selections. The actual
/// mask requires NSH alpha-mask support; these distances must not be used as opaque gradients.
pub fn get_virtualized_scroll_fade_class_name(top: bool, bottom: bool) -> Option<FadeEdges> {
    (top || bottom).then_some(FadeEdges {
        top: if top { 24.0 } else { 0.0 },
        bottom: if bottom { 24.0 } else { 0.0 },
        ..Default::default()
    })
}

pub fn scroll_fade_edges(target: ElementRef) -> Signal<FadeEdges> {
    let edges = signal(FadeEdges::default());
    watch_scroll(
        move || target.node(),
        move |event| edges.set(FadeEdges::from_scroll(event)),
    );
    edges
}

pub fn scroll_area<M>(
    scroll_fade: bool,
    children: impl nsh_core::element::IntoChild<M>,
) -> impl IntoElement {
    scroll_area_with_options(
        ScrollAreaOptions {
            scroll_fade,
            ..Default::default()
        },
        children,
    )
}

pub fn scroll_area_with_options<M>(
    options: ScrollAreaOptions,
    children: impl nsh_core::element::IntoChild<M>,
) -> impl IntoElement {
    let target = options.viewport_ref.unwrap_or_else(element_ref);
    let label = options.label;
    let on_mouse_leave = options.on_mouse_leave;
    // Native scroll() is one axis. Consumers choose horizontal explicitly; two-axis content
    // needs two independently controlled viewports until NSH exposes a Both scroll axis.
    let viewport = scroll().element_ref(&target).scrollbar(false)
        .focusable(options.viewport_tab_index.is_none_or(|index| index >= 0))
        .role(options.role)
        .accessibility_state(move || AccessibilityState { busy: options.busy.get(), ..Default::default() })
        .optional_label(move || label.as_ref().map(Text::resolve))
        .app_class(tw("h-full outline-none transition-shadow focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background"))
        .when(options.horizontal, |viewport| viewport.horizontal())
        .on_mouse_leave(move |_: &MouseEvent| { if let Some(callback) = &on_mouse_leave { callback(); } })
        .child(children);
    // rounded-[inherit], overscroll, stable gutter and scroll padding have no native style
    // property in this pin. Root clipping and native scroll physics replace the CSS hints.
    view()
        .app_class(tw("relative size-full min-h-0 overflow-hidden"))
        .when(options.radius == Radius::None, |root| {
            root.app_class(tw("rounded-none"))
        })
        .child(viewport)
        .child((!options.hide_scrollbars).then(|| scroll_bar(target, options.horizontal)))
}

pub fn scroll_bar(target: ElementRef, horizontal: bool) -> impl IntoElement {
    let mut motion = ScrollbarMotion::default();
    motion.idle = 300.0;
    motion.enter = 100.0;
    motion.exit = 150.0;
    nsh_base::scrollbar().scroll(target).axis(if horizontal { Axis::Horizontal } else { Axis::Vertical })
        .mode(ScrollbarMode::Hover).motion(motion)
        .accessibility_label(move || Some(Text::from(if horizontal { "Horizontal scroll" } else { "Vertical scroll" }).resolve()))
        .app_class(tw(if horizontal {
            "absolute inset-x-0 bottom-px mx-1 h-1.5 flex-col"
        } else { "absolute inset-y-0 right-px my-1 w-1.5" }))
        .thumb_class(tw("relative flex-1 rounded-full bg-[var(--app-scrollbar-thumb)] transition-colors hover:bg-[var(--app-scrollbar-thumb-hover)]"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fades_use_actual_overflow_distance_and_clamp_overscroll_and_small_content() {
        let event = ScrollEvent {
            offset: point(px(7.0), px(10.0)),
            content_size: size(px(120.0), px(200.0)),
            size: size(px(100.0), px(180.0)),
            ..Default::default()
        };
        assert_eq!(
            FadeEdges::from_scroll(&event),
            FadeEdges {
                top: 10.0,
                bottom: 10.0,
                left: 7.0,
                right: 13.0
            }
        );
        let no_overflow = ScrollEvent {
            offset: point(px(-5.0), px(-8.0)),
            ..Default::default()
        };
        assert_eq!(FadeEdges::from_scroll(&no_overflow).top, 0.0);
        assert_eq!(
            FadeEdges::from_scroll(&ScrollEvent::default()),
            FadeEdges::default()
        );
        assert_eq!(get_virtualized_scroll_fade_class_name(false, false), None);
        for (top, bottom) in [(true, false), (false, true), (true, true)] {
            let mask = get_virtualized_scroll_fade_class_name(top, bottom).expect("mask");
            assert_eq!(mask.top > 0.0, top);
            assert_eq!(mask.bottom > 0.0, bottom);
        }
    }
}
