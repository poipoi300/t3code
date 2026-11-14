//! Port of apps/web/src/components/chat/useTimelineFollow.ts useTimelineFollow,
//! timelineScrollAnchoring.ts and useTimelinePositionRestore.ts startTimelinePositionRestore.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use nsh_core::prelude::*;
use nsh_core::{TouchEvent, TouchPhase};
use t3_client::thread_views::messages_timeline::MessagesTimelineRow;
use t3_client::thread_views::messages_timeline::viewport::{
    TimelineEndState, resolve_timeline_is_at_end,
};
use t3_client::types::ThreadKey;

use crate::runtime::Shared;

use crate::runtime::tasks::watch;

/// Port of timelineViewport.ts TimelineFollow and transitionTimelineFollow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineFollow {
    Following,
    Released { at_end: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineFollowEvent {
    Navigated,
    FollowEnd,
    ReachedEnd,
    LeftEnd,
}

impl TimelineFollow {
    pub fn initial(follow_end: bool) -> Self {
        if follow_end {
            Self::Following
        } else {
            Self::Released { at_end: false }
        }
    }

    pub fn transition(self, event: TimelineFollowEvent) -> Self {
        use TimelineFollowEvent as E;
        match (self, event) {
            (Self::Following, E::Navigated) => Self::Released { at_end: true },
            (_, E::FollowEnd) | (Self::Released { at_end: false }, E::ReachedEnd) => {
                Self::Following
            }
            (Self::Released { at_end: true }, E::LeftEnd) => Self::Released { at_end: false },
            _ => self,
        }
    }
}

/// Port of timelineViewport.ts navigationReleasesFollow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineNavigation {
    Wheel { up: bool },
    Key { up: bool },
    Pointer { on_scrollbar: bool },
    Touch,
    Scroll,
    Command { releases_follow: bool },
}

impl TimelineNavigation {
    pub fn releases_follow(self, content_scrolls_up: bool, away_from_end: bool) -> bool {
        match self {
            Self::Wheel { up } => up && content_scrolls_up,
            Self::Key { up: true } | Self::Pointer { on_scrollbar: true } => content_scrolls_up,
            Self::Key { up: false }
            | Self::Pointer {
                on_scrollbar: false,
            }
            | Self::Touch => away_from_end,
            Self::Scroll => true,
            Self::Command { releases_follow } => releases_follow,
        }
    }
}

/// Geometry only: `ThreadTimelineMemory` caches client entries, not viewport positions.
#[derive(Clone, Debug, PartialEq)]
pub struct RememberedTimelinePosition {
    pub row_id: SharedString,
    pub offset_within_row: f32,
    pub scroll_offset: f32,
    pub at_end: bool,
}

#[derive(Default)]
struct Positions {
    entries: BTreeMap<ThreadKey, RememberedTimelinePosition>,
    order: VecDeque<ThreadKey>,
}

impl Positions {
    fn remember(&mut self, key: ThreadKey, mut position: RememberedTimelinePosition, follow: bool) {
        position.at_end |= follow;
        self.entries.insert(key.clone(), position);
        self.order.retain(|old| old != &key);
        self.order.push_back(key);
        if self.order.len() > 100
            && let Some(oldest) = self.order.pop_front()
        {
            self.entries.remove(&oldest);
        }
    }
}

#[derive(Clone, Default)]
struct PositionMemory(Rc<RefCell<Positions>>);

/// Stable keys prevent NSH from hashing and diffing every row on a streamed delta.
pub fn timeline_row_keys(
    rows: Memo<Shared<Vec<Rc<MessagesTimelineRow>>>>,
) -> Memo<Shared<Vec<SharedString>>> {
    let previous: Stored<Rc<Vec<SharedString>>> = stored(Rc::default());
    computed(move || {
        rows.with(|rows| {
            previous.with(|keys| {
                if keys.len() != rows.len()
                    || !keys
                        .iter()
                        .zip(rows.iter())
                        .all(|(id, row)| id.as_str() == row.id)
                {
                    *keys = Rc::new(rows.iter().map(|row| row.id.clone().into()).collect());
                }
                Shared(keys.clone())
            })
        })
    })
}

/// Where keyboard focus is when a scroll key arrives, for [`scroll_key_direction`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyFocus {
    /// An editable field, terminal or other text input has focus (TYPE_TO_FOCUS_EDITABLE_SELECTOR).
    pub editable: bool,
    /// A dialog or other floating layer is open (TYPE_TO_FOCUS_FLOATING_LAYER_SELECTOR).
    pub floating_layer: bool,
}

/// Port of timelineViewport.ts handleKeyDown's guards: the direction a scroll key moves the
/// timeline, or none when the key belongs to something else. NSH reports no IME composition on
/// key events, and delivers only keys whose focus is inside the list, so `isComposing` and the
/// body-target case have no equivalent here.
pub fn scroll_key_direction(event: &KeyEvent, focus: &KeyFocus) -> Option<bool> {
    let up = match event.key.as_str() {
        "PageUp" | "Home" | "ArrowUp" => true,
        "PageDown" | "End" | "ArrowDown" | " " => false,
        _ => return None,
    };
    let modifiers = event.modifiers;
    if event.default_prevented
        || modifiers.alt
        || modifiers.ctrl
        || modifiers.cmd
        || modifiers.shift
        || focus.editable
        || focus.floating_layer
    {
        return None;
    }
    Some(up)
}

type RowIndexLookup = Rc<dyn Fn(&str) -> Option<usize>>;

#[derive(Clone)]
struct IndexedRows {
    count: Memo<usize>,
    key: Rc<dyn Fn(usize) -> Option<SharedString>>,
    index: RowIndexLookup,
}

struct Visit {
    key: ThreadKey,
    memory: PositionMemory,
    saved: Option<RememberedTimelinePosition>,
    metrics: Option<ScrollEvent>,
    started: bool,
    positioned: bool,
    restoring: bool,
    serial: u64,
    navigation: u64,
    sampling: bool,
    follow_pending: bool,
    frame: Option<Measurement>,
    indexed: Option<IndexedRows>,
}

/// One mounted visit; the list owns its keyed splices and height index.
#[derive(Clone, Copy)]
pub struct TimelineViewport {
    pub element: ElementRef,
    pub follow: Signal<TimelineFollow>,
    keys: Memo<Shared<Vec<SharedString>>>,
    dragging: Signal<bool>,
    visit: Stored<Visit>,
}

impl TimelineViewport {
    pub fn new(key: ThreadKey, keys: Memo<Shared<Vec<SharedString>>>) -> Self {
        let memory = global::<PositionMemory>().get_untracked();
        let saved = memory.0.borrow().entries.get(&key).cloned();
        let this = Self {
            element: element_ref(),
            follow: signal(TimelineFollow::initial(
                saved.as_ref().is_none_or(|p| p.at_end),
            )),
            keys,
            dragging: signal(false),
            visit: stored(Visit {
                key,
                memory,
                saved,
                metrics: None,
                started: false,
                positioned: false,
                restoring: false,
                serial: 0,
                navigation: 0,
                sampling: false,
                follow_pending: false,
                frame: None,
                indexed: None,
            }),
        };
        // Synchronizes data arriving after an empty/loading mount with the native list.
        effect(move || {
            this.keys.get();
            after_answer(move || this.position());
        });
        // NSH cannot disable threshold-based follow; aggregate metrics drive immediate corrections.
        watch_scroll(
            move || this.element.node(),
            move |metrics| this.scrolled(*metrics),
        );
        this
    }

    /// Block lists resolve identities on demand; NSH's keys() would clone/hash the whole reply.
    pub fn with_indexed_rows(
        self,
        count: Memo<usize>,
        key: impl Fn(usize) -> Option<SharedString> + 'static,
        index: impl Fn(&str) -> Option<usize> + 'static,
    ) -> Self {
        self.visit.with(|visit| {
            visit.indexed = Some(IndexedRows {
                count,
                key: Rc::new(key),
                index: Rc::new(index),
            })
        });
        effect(move || {
            count.get();
            after_answer(move || self.position());
        });
        self
    }

    fn row_count(self) -> usize {
        self.visit.with(|visit| visit.indexed.clone()).map_or_else(
            || self.keys.with_untracked(|keys| keys.len()),
            |rows| rows.count.get_untracked(),
        )
    }

    fn row_key(self, index: usize) -> Option<SharedString> {
        self.visit.with(|visit| visit.indexed.clone()).map_or_else(
            || self.keys.with_untracked(|keys| keys.get(index).cloned()),
            |rows| (rows.key)(index),
        )
    }

    fn row_index(self, key: &str) -> Option<usize> {
        self.visit.with(|visit| visit.indexed.clone()).map_or_else(
            || {
                self.keys
                    .with_untracked(|keys| keys.iter().position(|id| id.as_str() == key))
            },
            |rows| (rows.index)(key),
        )
    }

    /// Attach to the real list, without a second scroll container or any per-row measurement.
    pub fn attach(self, list: VirtualList) -> VirtualList {
        let keys = use_context::<crate::keys::bindings::Keybindings>();
        let key_focus = move || {
            let context = keys.map(|keys| keys.context.read());
            let flag = |name| context.as_ref().is_some_and(|context| context.get(name));
            KeyFocus {
                editable: flag("editableFocus") || flag("terminalFocus"),
                floating_layer: crate::ui::confirm::is_confirm_dialog_active(),
            }
        };
        let saved = self.visit.with(|v| v.saved.clone());
        // NSH follows index/end commands through measurement; no RAF settling loop is needed.
        let list = if let Some(saved) = saved.filter(|p| !p.at_end) {
            match self.row_index(saved.row_id.as_str()) {
                Some(index) => list.initial_scroll_index(index),
                None => list,
            }
        } else {
            list.initial_scroll_at_end()
        };
        let list = if self.visit.with(|visit| visit.indexed.is_some()) {
            list
        } else {
            list.keys(move || self.keys.get().0.as_ref().clone())
        };
        list.alignment(ListAlignment::Top)
            .element_ref(&self.element)
            // NSH has no indicator inset: keep the list frame inside the vertical-bar safe area.
            .mr(move || safe_area().right)
            .on_layout(move |_: &Bounds<Points>| {
                self.position();
                watch(async move {
                    let frame = self.element.measure().await;
                    self.visit.with(|v| v.frame = frame);
                });
            })
            // timelineViewport.ts handleWheel. NSH's wheel event names no target, so a nested
            // scroller's own wheel still counts here (isTimelineScrollTarget has no equivalent).
            .on_wheel(move |event: &WheelEvent| {
                let delta = event.delta.points(px(16.0)).y.0;
                if event.modifiers.ctrl || delta == 0.0 {
                    return;
                }
                self.navigate(TimelineNavigation::Wheel { up: delta < 0.0 });
            })
            .on_key_down(move |event: &KeyEvent| {
                if let Some(up) = scroll_key_direction(event, &key_focus()) {
                    self.navigate(TimelineNavigation::Key { up });
                }
            })
            .on_touch(move |event: &TouchEvent| {
                if event.phase != TouchPhase::Start {
                    return;
                }
                let Some(point) = event.points.iter().find(|p| p.changed) else {
                    return;
                };
                let navigation = if point.pointer_type == PointerType::Mouse {
                    let on_scrollbar = self.visit.with(|v| {
                        v.frame.is_some_and(|frame| {
                            point.page.x.0 >= frame.page.x.0 + frame.frame.size.width.0 - 16.0
                        })
                    });
                    TimelineNavigation::Pointer { on_scrollbar }
                } else {
                    TimelineNavigation::Touch
                };
                self.navigate(navigation);
            })
            .on_scroll_drag_start(move |_: &Point<Points>| {
                self.dragging.set(true);
                self.navigate(TimelineNavigation::Touch);
            })
            .on_scroll_drag_end(move |_: &Point<Points>| self.end_drag())
            .on_momentum_end(move |_: &Point<Points>| self.end_drag())
    }

    fn content_changed(self) {
        if self.follow.get_untracked() != TimelineFollow::Following || self.dragging.get_untracked()
        {
            return;
        }
        let schedule = self.visit.with(|v| {
            if v.follow_pending || v.restoring {
                return false;
            }
            v.follow_pending = true;
            true
        });
        if schedule {
            after_answer(move || {
                self.visit.with(|v| v.follow_pending = false);
                if self.follow.get_untracked() == TimelineFollow::Following
                    && !self.dragging.get_untracked()
                {
                    self.element.scroll_to_end(false);
                }
            });
        }
    }

    pub fn navigate(self, navigation: TimelineNavigation) {
        let (overflow, away) = self.visit.with(|v| {
            v.metrics.map_or((false, false), |m| {
                (m.content_size.height > m.size.height, !is_at_end(m))
            })
        });
        let releases = navigation.releases_follow(overflow, away);
        self.visit.with(|v| {
            v.navigation += 1;
            v.serial += 1;
            if v.restoring {
                v.restoring = false;
                v.positioned = true;
                if let Some(metrics) = v.metrics {
                    self.element.scroll_to_offset(metrics.offset.y, false);
                }
            }
        });
        if releases {
            self.follow
                .update(|follow| *follow = follow.transition(TimelineFollowEvent::Navigated));
            self.sample_position();
        }
    }

    pub fn scroll_to_end(self) {
        self.navigate(TimelineNavigation::Command {
            releases_follow: false,
        });
        self.follow.set(TimelineFollow::Following);
        self.element.scroll_to_end(false);
    }

    /// Find, minimap and citation commands cancel a restore before moving the list.
    pub fn scroll_to_index(self, index: usize, offset: Points) {
        self.navigate(TimelineNavigation::Command {
            releases_follow: true,
        });
        self.element
            .scroll_to_index(index as u32, Percentage(0.0), offset, false);
    }

    fn position(self) {
        if self.row_count() == 0 || self.element.node().is_none() {
            return;
        }
        let start = self.visit.with(|v| {
            if v.started {
                return None;
            }
            v.started = true;
            v.restoring = true;
            Some(v.saved.clone())
        });
        let Some(saved) = start else { return };
        match saved.filter(|p| !p.at_end) {
            Some(saved) => {
                if let Some(index) = self.row_index(saved.row_id.as_str()) {
                    self.element.scroll_to_index(
                        index as u32,
                        Percentage(0.0),
                        px(-saved.offset_within_row),
                        false,
                    );
                } else {
                    self.element
                        .scroll_to_offset(px(saved.scroll_offset), false);
                }
            }
            None => self.element.scroll_to_end(false),
        }
        let navigation = self.visit.with(|v| v.navigation);
        watch(async move {
            let measured = self.element.measure_scroll_position().await;
            self.visit.with(|v| {
                if v.navigation == navigation && measured.is_some() {
                    v.restoring = false;
                    v.positioned = true;
                } else if measured.is_none() {
                    v.started = false;
                    v.restoring = false;
                }
            });
            self.content_changed();
            self.sample_position();
        });
    }

    fn scrolled(self, metrics: ScrollEvent) {
        let resized = self.visit.with(|v| {
            let resized = v.metrics.is_none_or(|before| {
                before.content_size != metrics.content_size || before.size != metrics.size
            });
            v.metrics = Some(metrics);
            v.serial += 1;
            resized
        });
        if self.dragging.get_untracked() {
            self.navigate(TimelineNavigation::Touch);
        }
        if self.visit.with(|v| v.positioned && !v.restoring) {
            self.follow.update(|follow| {
                *follow = follow.transition(if is_at_end(metrics) {
                    TimelineFollowEvent::ReachedEnd
                } else {
                    TimelineFollowEvent::LeftEnd
                })
            });
            self.sample_position();
        }
        if resized {
            self.content_changed();
        }
    }

    fn end_drag(self) {
        self.dragging.set(false);
        if self.follow.get_untracked() == TimelineFollow::Following {
            self.element.scroll_to_end(false);
        }
    }

    fn sample_position(self) {
        let serial = self.visit.with(|v| {
            if v.sampling || !v.positioned || v.restoring || v.metrics.is_none() {
                return None;
            }
            v.sampling = true;
            Some(v.serial)
        });
        let Some(serial) = serial else { return };
        let keys = self.keys.get_untracked();
        let count = self.row_count();
        watch(async move {
            let position = self.element.measure_scroll_position().await;
            let current_keys = self.keys.get_untracked();
            let current_count = self.row_count();
            let anchor = position.and_then(|position| {
                self.row_key(position.index as usize)
                    .map(|key| (key, position))
            });
            let dirty = self.visit.with(|v| {
                v.sampling = false;
                if serial != v.serial
                    || count != current_count
                    || !Rc::ptr_eq(&keys.0, &current_keys.0)
                {
                    return true;
                }
                if let Some((row_id, position)) = anchor
                    && let Some(metrics) = v.metrics
                {
                    v.memory.0.borrow_mut().remember(
                        v.key.clone(),
                        RememberedTimelinePosition {
                            row_id,
                            offset_within_row: position.offset.0,
                            scroll_offset: metrics.offset.y.0,
                            at_end: is_at_end(metrics),
                        },
                        self.follow.get_untracked() == TimelineFollow::Following,
                    );
                }
                false
            });
            // A new event dirtied the in-flight sample; take that event's position once.
            if dirty {
                self.sample_position();
            }
        });
    }
}

fn is_at_end(metrics: ScrollEvent) -> bool {
    resolve_timeline_is_at_end(Some(&TimelineEndState {
        content_length: Some(metrics.content_size.height.0.into()),
        scroll: Some(metrics.offset.y.0.into()),
        scroll_length: Some(metrics.size.height.0.into()),
        ..Default::default()
    })) == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use t3_client::types::{EnvironmentId, ThreadId};

    fn key_event(name: &'static str, change: impl FnOnce(&mut KeyEvent)) -> KeyEvent {
        let mut event = KeyEvent::default();
        event.key = SharedString::new_static(name);
        change(&mut event);
        event
    }

    #[test]
    fn scroll_keys_move_the_timeline_only_without_modifiers_editables_or_layers() {
        let plain = KeyFocus::default();
        assert_eq!(
            scroll_key_direction(&key_event("PageUp", |_| ()), &plain),
            Some(true)
        );
        assert_eq!(
            scroll_key_direction(&key_event("ArrowDown", |_| ()), &plain),
            Some(false)
        );
        assert_eq!(
            scroll_key_direction(&key_event(" ", |_| ()), &plain),
            Some(false)
        );
        assert_eq!(scroll_key_direction(&key_event("a", |_| ()), &plain), None);
        // Shift+Arrow extends a text selection; it must not release follow.
        for modify in [
            |e: &mut KeyEvent| e.modifiers.shift = true,
            |e: &mut KeyEvent| e.modifiers.alt = true,
            |e: &mut KeyEvent| e.modifiers.ctrl = true,
            |e: &mut KeyEvent| e.modifiers.cmd = true,
            |e: &mut KeyEvent| e.default_prevented = true,
        ] {
            assert_eq!(
                scroll_key_direction(&key_event("ArrowUp", modify), &plain),
                None
            );
        }
        for focus in [
            KeyFocus {
                editable: true,
                floating_layer: false,
            },
            KeyFocus {
                editable: false,
                floating_layer: true,
            },
        ] {
            assert_eq!(
                scroll_key_direction(&key_event("Home", |_| ()), &focus),
                None
            );
        }
    }

    #[test]
    fn follow_releases_and_rearms_only_after_leaving_and_returning() {
        use TimelineFollow as F;
        use TimelineFollowEvent as E;
        assert_eq!(F::Following.transition(E::LeftEnd), F::Following);
        let released = F::Following.transition(E::Navigated);
        assert_eq!(released, F::Released { at_end: true });
        assert_eq!(released.transition(E::ReachedEnd), released);
        let away = released.transition(E::LeftEnd);
        assert_eq!(away, F::Released { at_end: false });
        assert_eq!(away.transition(E::Navigated), away);
        assert_eq!(away.transition(E::ReachedEnd), F::Following);
        assert_eq!(away.transition(E::FollowEnd), F::Following);
        assert_eq!(F::initial(false), away);
        assert_eq!(F::initial(true), F::Following);
    }

    #[test]
    fn navigation_obeys_the_webs_direction_and_end_band_rules() {
        use TimelineNavigation as N;
        for (navigation, overflow, away, releases) in [
            (N::Wheel { up: true }, true, false, true),
            (N::Wheel { up: true }, false, true, false),
            (N::Wheel { up: false }, true, true, false),
            (N::Key { up: true }, true, false, true),
            (N::Key { up: false }, true, false, false),
            (N::Key { up: false }, false, true, true),
            (N::Touch, true, false, false),
            (N::Touch, true, true, true),
            (
                N::Pointer {
                    on_scrollbar: false,
                },
                true,
                false,
                false,
            ),
            (N::Pointer { on_scrollbar: true }, true, false, true),
            (N::Scroll, false, false, true),
            (
                N::Command {
                    releases_follow: false,
                },
                true,
                true,
                false,
            ),
            (
                N::Command {
                    releases_follow: true,
                },
                false,
                false,
                true,
            ),
        ] {
            assert_eq!(
                navigation.releases_follow(overflow, away),
                releases,
                "{navigation:?}"
            );
        }
    }

    fn key(environment: &str, id: usize) -> ThreadKey {
        ThreadKey {
            environment: EnvironmentId(environment.into()),
            thread: ThreadId(id.to_string()),
        }
    }

    #[test]
    fn positions_follow_the_webs_hundred_visit_limit_and_environment_identity() {
        let mut positions = Positions::default();
        let position = RememberedTimelinePosition {
            row_id: "reply".into(),
            offset_within_row: 37.0,
            scroll_offset: 900.0,
            at_end: false,
        };
        for id in 0..100 {
            positions.remember(key("a", id), position.clone(), false);
        }
        positions.remember(key("a", 0), position.clone(), true);
        positions.remember(key("b", 0), position.clone(), false);
        assert_eq!(positions.entries.len(), 100);
        assert!(!positions.entries.contains_key(&key("a", 1)));
        assert!(positions.entries[&key("a", 0)].at_end);
        assert_eq!(positions.entries[&key("b", 0)], position);
    }
}
