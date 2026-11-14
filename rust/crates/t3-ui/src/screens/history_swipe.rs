//! Port of apps/web/src/components/HistorySwipeNavigation.tsx HistorySwipeNavigation and
//! SwipeIndicator. The gesture logic is the client's (`view_logic::shell::history_swipe`).
//!
//! `history_swipe_area(availability, navigate, children)` wraps the main content (the web reads
//! `[data-slot="sidebar-inset"]`'s box; the wrapper's own box is that box). It listens only where
//! `crate::host::trackpad_history_swipe()` says the platform turns two-finger trackpad swipes
//! into wheel events that cannot be mistaken for a mouse's tilt wheel (the web: Electron on
//! macOS); elsewhere it is the children alone.
//!
//! Departures, each forced by NSH:
//! - Chromium's trackpad scroll brackets (`onTrackpadScrollBegin`/`End`) have no NSH event: a
//!   pause of 150 ms between wheel events ends the scroll (NSH's own wheel latching treats a
//!   200 ms gap as a new scroll), and the next wheel begins one.
//! - The web listens in the capture phase on the window and asks the composed path whether a
//!   scroller can still scroll horizontally or a dialog holds the event. NSH delivers a wheel to
//!   the nearest listener, a native horizontal scroller keeps the wheels it can use, and a modal
//!   layer is outside this wrapper, so those wheels never arrive here.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use nsh_core::element::IntoChild;
use nsh_core::prelude::*;
use nsh_core::{WheelDelta, WheelEvent};
use t3_client::view_logic::shell::history_swipe::{
    HistorySwipe, SwipeBounds, SwipeIndicator, SwipeUpdate, SwipeWheel, indicator_hidden_percent,
    indicator_top,
};
use t3_client::view_logic::shell::navigation::{Direction, HistoryAvailability};

use crate::theme::{cn, tw};
use crate::ui::icon::{Icon, icon_with_class};

const SCROLL_END_PAUSE: Duration = Duration::from_millis(150);

/// `HistorySwipeNavigation` around `children`.
pub fn history_swipe_area<M: 'static>(
    availability: Rc<dyn Fn() -> HistoryAvailability>,
    navigate: Rc<dyn Fn(Direction)>,
    children: impl IntoChild<M> + 'static,
) -> impl IntoElement {
    let area = view().app_class(tw("relative size-full min-h-0 min-w-0"));
    if !crate::host::trackpad_history_swipe() {
        return area.child(children);
    }
    let state = Rc::new(RefCell::new(HistorySwipe::default()));
    let indicator = signal(None::<SwipeIndicator>);
    let bounds = signal(SwipeBounds::default());
    let wheels = signal(0u64);

    let apply = {
        let navigate = navigate.clone();
        Rc::new(move |update: SwipeUpdate| {
            if let Some(next) = update.indicator {
                indicator.set(Some(next));
            }
            if let Some(direction) = update.navigate {
                navigate(direction);
            }
        })
    };

    // The pause after the last wheel ends the scroll; a new wheel restarts the timer.
    let end_state = state.clone();
    let end_apply = apply.clone();
    effect(move || {
        if wheels.get() == 0 {
            return;
        }
        let state = end_state.clone();
        let apply = end_apply.clone();
        set_timeout(SCROLL_END_PAUSE, move || {
            let update = state.borrow_mut().scroll_end();
            apply(update);
        });
    });

    let wheel_state = state;
    area.on_layout(move |layout: &Bounds<Points>| {
        bounds.set(SwipeBounds {
            left: 0.0,
            top: 0.0,
            width: f64::from(layout.size.width.0),
            height: f64::from(layout.size.height.0),
        })
    })
    .on_wheel(move |event: &WheelEvent| {
        let (delta_x, delta_y, pixels) = match event.delta {
            WheelDelta::Points(delta) => (f64::from(delta.x.0), f64::from(delta.y.0), true),
            WheelDelta::Lines(delta) => (f64::from(delta.x), f64::from(delta.y), false),
        };
        let mut swipe = wheel_state.borrow_mut();
        if wheels.get_untracked() == 0 || indicator.get_untracked().is_none_or(|i| !i.tracking) {
            swipe.scroll_begin();
        }
        let update = swipe.wheel(
            SwipeWheel {
                delta_x,
                delta_y,
                ctrl: event.modifiers.ctrl,
                pixels,
                y: f64::from(event.position.y.0),
                in_dialog: false,
                can_scroll_horizontally: false,
            },
            |direction| availability().can_navigate(direction),
            || bounds.get_untracked(),
        );
        drop(swipe);
        apply(update);
        wheels.update(|n| *n += 1);
    })
    .child(children)
    .child(swipe_indicator(indicator))
}

/// `SwipeIndicator`: a circle pinned to the edge of the main content that peeks in up to half
/// its width. One view for the life of the area, so its fade-out after release animates.
fn swipe_indicator(indicator: Signal<Option<SwipeIndicator>>) -> impl IntoElement {
    let back = move || {
        indicator
            .get()
            .is_none_or(|i| i.direction == Direction::Back)
    };
    let shown = move || indicator.get().is_some();
    // The glyph is rebuilt only when its direction or armed state changes.
    let armed = computed(move || indicator.get().is_some_and(|i| i.progress >= 1.0));
    let leftward = computed(back);
    view()
        .accessible(false)
        .app_class(move || {
            tw(&cn([
                Some("pointer-events-none absolute inset-0 z-100 overflow-hidden"),
                (!shown()).then_some("hidden"),
            ]))
        })
        .child(
            view()
                .translate_y(move || DefiniteLength::from(px(indicator.get().map_or(0.0, |i| indicator_top(&i) as f32))))
                .translate_x(move || {
                    let hidden = indicator.get().map_or(56.0, |i| indicator_hidden_percent(&i) as f32 / 100.0 * 56.0);
                    DefiniteLength::from(px(if back() { -hidden } else { hidden }))
                })
                .opacity(move || if indicator.get().is_some_and(|i| i.tracking) { 1.0_f32 } else { 0.0 })
                .app_class(move || {
                    let tracking = indicator.get().is_some_and(|i| i.tracking);
                    tw(&cn([
                        Some("absolute top-0 flex size-14 items-center rounded-full bg-muted text-foreground dark:bg-black dark:text-white"),
                        Some(if back() { "left-0 justify-end pr-2" } else { "right-0 justify-start pl-2" }),
                        (!tracking).then_some("transition duration-200 ease-out"),
                    ]))
                })
                .child(dynamic(move || {
                    let armed = armed.get();
                    icon_with_class(
                        if leftward.get() { Icon::ArrowLeft } else { Icon::ArrowRight },
                        if armed {
                            "size-4 transition-opacity duration-100 opacity-100"
                        } else {
                            "size-4 transition-opacity duration-100 opacity-40"
                        },
                    )
                    .into_element()
                })),
        )
}
