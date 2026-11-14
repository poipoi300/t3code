//! Port of apps/web/src/components/ui/sidebar.tsx SidebarRail and hooks/useResizeDrag.ts.
//!
//! `sidebar_rail(SidebarRailOptions { .. })` sits inside the sidebar's positioned container, as
//! the TSX rail sits inside `Sidebar`. With `resize` and the sidebar open, a drag resizes; it
//! writes the live width to `RailResize::width` while it runs and commits once through
//! `on_resize` (the shell persists it with the client's `sidebar_width` record). Without
//! `resize`, or closed, a press toggles the sidebar. The width rules (minimum, maximum, the
//! main content's minimum) are the client's `view_logic::shell::sidebar_width`.
//!
//! [`ResizeDrag`] is `useResizeDrag`'s session logic without the DOM, for any one-edge resize
//! handle (panes, the right panel).
//!
//! Departures, each forced by NSH:
//! - Pointer capture is NSH's touch responder, which a desktop mouse drag also reaches. The rail
//!   takes the pointer when it goes down (as `setPointerCapture` on pointerdown) and decides
//!   press, double press or drag on release: a native Button would cancel its press on the
//!   responder grant, so the rail is a view with the button role.
//! - The web batches moves per animation frame; NSH delivers the responder's moves per frame.
//! - `document.body.style.cursor`/`user-select` during the drag: the rail keeps its own resize
//!   cursor; NSH has no app-wide cursor override.
//! - The `[[data-panel-animations=true]_&]` transition follows the sidebar's own transition.
//! - Window blur committing a drag: NSH reports a lost pointer as the responder's termination,
//!   which cancels; the web commits on blur and cancels on pointercancel.

use std::cell::RefCell;
use std::rc::Rc;

use nsh_core::element::{ClaimRequest, ResponderEvent, TouchClaim};
use nsh_core::prelude::*;

use super::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};
use crate::i18n::t;
use crate::theme::{cn, tw};

/// `ResizeBounds`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizeBounds {
    pub min: f32,
    pub max: f32,
}

/// Which edge of the resized element the handle drags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
}

fn clamp_to_bounds(width: f32, bounds: ResizeBounds) -> f32 {
    bounds.min.max(bounds.max.min(width))
}

/// `ResizeSession`: what a drag needs, fixed when it starts.
pub struct ResizeSession {
    pub width: f32,
    pub edge: Edge,
    pub bounds: ResizeBounds,
    /// Takes a width clamped to the bounds and answers the width it applied.
    pub resize: Box<dyn FnMut(f32) -> f32>,
    pub finish: Box<dyn FnOnce(f32, bool)>,
    pub cleanup: Option<Box<dyn FnOnce(bool)>>,
}

struct ActiveDrag {
    session: ResizeSession,
    start_x: f32,
    pending_x: f32,
    width: f32,
    moved: bool,
}

/// `createResizeDrag`: one drag at a time; `finish(true)` commits, `finish(false)` abandons.
#[derive(Default)]
pub struct ResizeDrag {
    active: Option<ActiveDrag>,
}

impl ResizeDrag {
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// `begin`; refused while another drag runs.
    pub fn begin(&mut self, session: ResizeSession, x: f32) -> bool {
        if self.active.is_some() {
            return false;
        }
        let width = session.width;
        self.active = Some(ActiveDrag {
            session,
            start_x: x,
            pending_x: x,
            width,
            moved: false,
        });
        true
    }

    /// `onPointerMove` followed by its frame's `flush`.
    pub fn move_to(&mut self, x: f32) {
        if let Some(active) = &mut self.active {
            active.pending_x = x;
            if (x - active.start_x).abs() > 2.0 {
                active.moved = true;
            }
        }
        self.flush();
    }

    fn flush(&mut self) {
        let Some(active) = &mut self.active else {
            return;
        };
        let sign = if active.session.edge == Edge::Left {
            -1.0
        } else {
            1.0
        };
        let delta = (active.pending_x - active.start_x) * sign;
        if delta.abs() > 2.0 {
            active.moved = true;
        }
        let next = clamp_to_bounds(active.session.width + delta, active.session.bounds);
        active.width = (active.session.resize)(next);
    }

    /// `finish`: whether the drag that ended had moved, when one was running.
    pub fn finish(&mut self, commit: bool) -> Option<bool> {
        if commit {
            self.flush();
        }
        let active = self.active.take()?;
        let ActiveDrag {
            session,
            width,
            moved,
            ..
        } = active;
        if let Some(cleanup) = session.cleanup {
            cleanup(commit);
        }
        if commit {
            (session.finish)(width, moved);
        }
        Some(moved)
    }
}

/// The TSX `side` of the sidebar holding the rail.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Side {
    #[default]
    Left,
    Right,
}

/// SidebarResizableOptions as the rail uses them.
#[derive(Clone)]
pub struct RailResize {
    /// The live width: written during a drag, restored when a drag is abandoned.
    pub width: Signal<f32>,
    /// `minWidth`/`maxWidth`, read when a drag starts.
    pub bounds: Rc<dyn Fn() -> ResizeBounds>,
    /// `shouldAcceptWidth(current, next)`.
    pub should_accept: Option<Rc<dyn Fn(f32, f32) -> bool>>,
    /// The commit (`finish`): persistence and `onResize`.
    pub on_resize: Option<Rc<dyn Fn(f32)>>,
}

#[derive(Clone)]
pub struct SidebarRailOptions {
    pub side: Side,
    pub open: Reactive<bool>,
    /// `collapsible="offcanvas"` (the app sidebar's), which places the rail at the edge.
    pub offcanvas: bool,
    pub resize: Option<RailResize>,
    pub toggle: Rc<dyn Fn()>,
    pub on_double_click: Option<Rc<dyn Fn()>>,
}

/// Two presses within this many milliseconds are a double press (the desktop default).
const DOUBLE_PRESS_MS: f64 = 500.0;

/// `SidebarRail`.
pub fn sidebar_rail(options: SidebarRailOptions) -> impl IntoElement {
    let SidebarRailOptions {
        side,
        open,
        offcanvas,
        resize,
        toggle,
        on_double_click,
    } = options;
    let open = computed(move || open.get());
    let resizable = resize.is_some();
    let can_resize = move || resizable && open.get();
    let drag = Rc::new(RefCell::new(ResizeDrag::default()));
    let origin = signal(0.0f32);
    let last_press = signal(None::<f64>);
    let dragging = signal(false);

    // Collapsing mid-drag abandons the drag instead of committing a width to a hidden sidebar.
    let collapse_drag = drag.clone();
    effect(move || {
        if !open.get() && dragging.get_untracked() {
            collapse_drag.borrow_mut().finish(false);
            dragging.set(false);
        }
    });
    // Unmounting the rail abandons a live drag too.
    let unmount_drag = drag.clone();
    cleanup(move || {
        let _ = unmount_drag
            .try_borrow_mut()
            .map(|mut drag| drag.finish(false));
    });

    let begin_drag = drag.clone();
    let begin_resize = resize.clone();
    let begin = move |x: f32| {
        let Some(resize) = begin_resize.clone() else {
            return;
        };
        if !open.get_untracked() {
            return;
        }
        let bounds = (resize.bounds)();
        let start_width = clamp_to_bounds(resize.width.get_untracked(), bounds);
        let live = resize.width;
        live.set(start_width);
        let accept = resize.should_accept.clone();
        let mut width = start_width;
        let commit = resize.on_resize.clone();
        let session = ResizeSession {
            width: start_width,
            edge: if side == Side::Left {
                Edge::Right
            } else {
                Edge::Left
            },
            bounds,
            resize: Box::new(move |next| {
                if accept.as_ref().is_none_or(|accept| accept(width, next)) {
                    live.set(next);
                    width = next;
                }
                width
            }),
            finish: Box::new(move |final_width, _moved| {
                live.set(final_width);
                if let Some(commit) = commit {
                    commit(final_width);
                }
            }),
            cleanup: Some(Box::new(move |committed| {
                if !committed {
                    let _ = live.try_set(start_width);
                }
            })),
        };
        if begin_drag.borrow_mut().begin(session, x) {
            dragging.set(true);
        }
    };

    let move_drag = drag.clone();
    let release_drag = drag.clone();
    let terminate_drag = drag;
    let press_toggle = toggle.clone();
    let release_toggle = toggle;
    let on_double = on_double_click.clone();
    // handleClick: a press after a drag that moved does nothing; an open resizable rail does
    // nothing on press; otherwise it toggles. A second press soon after is a double click.
    let click = Rc::new(move |moved: bool, at: f64| {
        let previous = last_press.get_untracked();
        last_press.set((!moved).then_some(at));
        if moved {
            return;
        }
        if previous.is_some_and(|previous| at - previous <= DOUBLE_PRESS_MS) {
            last_press.set(None);
            if let Some(on_double) = &on_double {
                on_double();
            }
        }
        if !(resizable && open.get_untracked()) {
            release_toggle();
        }
    });
    let release_click = click.clone();

    let handle = tooltip_create_handle();
    view()
        .role(AccessibilityRole::Button)
        .label(move || {
            SharedString::from(t(if can_resize() {
                "Resize Sidebar"
            } else {
                "Toggle Sidebar"
            }))
        })
        .test_id("sidebar-rail")
        .tooltip(tooltip(
            handle,
            TooltipOptions {
                side: Placement::Right,
                ..TooltipOptions::default()
            },
            text(move || {
                t(if can_resize() {
                    "Drag to resize sidebar"
                } else {
                    "Toggle Sidebar"
                })
            }),
        ))
        .on_accessibility_action(move |action: &str| {
            if action == "activate" && !(resizable && open.get_untracked()) {
                press_toggle();
            }
        })
        .claim_touches(TouchClaim::on_start().ask(move |request: &ClaimRequest| request.claim()))
        .on_responder_grant(move |event: &ResponderEvent| {
            origin.set(event.page.x.0);
            begin(event.page.x.0);
        })
        .on_responder_move(move |event: &ResponderEvent| {
            move_drag
                .borrow_mut()
                .move_to(origin.get_untracked() + event.gesture.delta.x.0);
        })
        .on_responder_release(move |event: &ResponderEvent| {
            let moved = release_drag
                .borrow_mut()
                .finish(true)
                .unwrap_or(event.gesture.delta.x.0.abs() > 2.0);
            dragging.set(false);
            release_click(moved, event.timestamp);
        })
        .on_responder_terminate(move |_: &ResponderEvent| {
            terminate_drag.borrow_mut().finish(false);
            dragging.set(false);
        })
        .app_class(move || {
            let collapsed = !open.get();
            tw(&cn([
                // A collapsed offcanvas rail overlaps the native scrollbar on Windows and Linux.
                Some("group -translate-x-1/2 absolute inset-y-0 z-20 hidden w-4 sm:flex"),
                (side == Side::Left).then_some("-right-4"),
                (side == Side::Right).then_some("left-0"),
                (offcanvas && collapsed).then_some("pointer-events-none"),
                Some(match (side, collapsed) {
                    (Side::Left, false) | (Side::Right, true) => "cursor-w-resize",
                    (Side::Right, false) | (Side::Left, true) => "cursor-e-resize",
                }),
                offcanvas.then_some("translate-x-0 hover:bg-sidebar"),
                (offcanvas && side == Side::Left).then_some("-right-2"),
                (offcanvas && side == Side::Right).then_some("-left-2"),
            ]))
        })
        // after: the 2px line, at the centre (or at the full width for offcanvas).
        .child(view().accessible(false).app_class(move || {
            tw(&cn([
                Some("pointer-events-none absolute inset-y-0 w-[2px]"),
                Some(if offcanvas { "left-full" } else { "left-1/2" }),
                Some("group-hover:bg-sidebar-border"),
            ]))
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn session(
        width: f32,
        edge: Edge,
        applied: Rc<Cell<f32>>,
        finished: Rc<Cell<Option<(f32, bool)>>>,
        cleaned: Rc<Cell<Option<bool>>>,
        accept_up_to: f32,
    ) -> ResizeSession {
        let mut current = width;
        ResizeSession {
            width,
            edge,
            bounds: ResizeBounds {
                min: 200.0,
                max: 600.0,
            },
            resize: Box::new(move |next| {
                if next <= current || next <= accept_up_to {
                    current = next;
                    applied.set(next);
                }
                current
            }),
            finish: Box::new(move |width, moved| finished.set(Some((width, moved)))),
            cleanup: Some(Box::new(move |committed| cleaned.set(Some(committed)))),
        }
    }

    #[test]
    fn a_right_edge_drag_follows_the_pointer_within_bounds_and_commits_once() {
        let (applied, finished, cleaned) = (Rc::default(), Rc::default(), Rc::default());
        let mut drag = ResizeDrag::default();
        assert!(drag.begin(
            session(
                300.0,
                Edge::Right,
                Rc::clone(&applied),
                Rc::clone(&finished),
                Rc::clone(&cleaned),
                1000.0
            ),
            100.0
        ));
        assert!(
            !drag.begin(
                session(
                    300.0,
                    Edge::Right,
                    Rc::default(),
                    Rc::default(),
                    Rc::default(),
                    1000.0
                ),
                0.0
            ),
            "one drag at a time"
        );
        drag.move_to(150.0);
        assert_eq!(applied.get(), 350.0);
        drag.move_to(900.0);
        assert_eq!(applied.get(), 600.0, "clamped to max");
        drag.move_to(-500.0);
        assert_eq!(applied.get(), 200.0, "clamped to min");
        assert_eq!(drag.finish(true), Some(true));
        assert_eq!(finished.get(), Some((200.0, true)));
        assert_eq!(cleaned.get(), Some(true));
        assert_eq!(drag.finish(true), None, "already finished");
    }

    #[test]
    fn a_left_edge_drag_grows_leftwards_and_refused_widths_hold() {
        let (applied, finished, cleaned) = (Rc::default(), Rc::default(), Rc::default());
        let mut drag = ResizeDrag::default();
        drag.begin(
            session(
                300.0,
                Edge::Left,
                Rc::clone(&applied),
                Rc::clone(&finished),
                Rc::clone(&cleaned),
                320.0,
            ),
            500.0,
        );
        drag.move_to(480.0);
        assert_eq!(applied.get(), 320.0);
        drag.move_to(400.0);
        assert_eq!(applied.get(), 320.0, "shouldAcceptWidth refused 400");
        drag.finish(true);
        assert_eq!(finished.get(), Some((320.0, true)));
    }

    #[test]
    fn small_moves_are_not_drags_and_abandoned_drags_do_not_commit() {
        let (applied, finished, cleaned) = (Rc::default(), Rc::default(), Rc::default());
        let mut drag = ResizeDrag::default();
        drag.begin(
            session(
                300.0,
                Edge::Right,
                Rc::clone(&applied),
                Rc::clone(&finished),
                Rc::clone(&cleaned),
                1000.0,
            ),
            100.0,
        );
        drag.move_to(101.5);
        assert_eq!(drag.finish(true), Some(false), "within 2px is a press");
        let (finished2, cleaned2) = (
            Rc::<Cell<Option<(f32, bool)>>>::default(),
            Rc::<Cell<Option<bool>>>::default(),
        );
        drag.begin(
            session(
                300.0,
                Edge::Right,
                Rc::default(),
                Rc::clone(&finished2),
                Rc::clone(&cleaned2),
                1000.0,
            ),
            0.0,
        );
        drag.move_to(80.0);
        assert_eq!(drag.finish(false), Some(true));
        assert_eq!(finished2.get(), None, "no commit");
        assert_eq!(cleaned2.get(), Some(false));
    }
}
