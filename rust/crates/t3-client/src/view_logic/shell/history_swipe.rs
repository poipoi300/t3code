//! Port of apps/web/src/components/HistorySwipeNavigation.tsx's gesture logic: two-finger
//! horizontal trackpad swipes go back and forward. The host feeds wheel events and the
//! trackpad scroll brackets; this decides the indicator and the navigation.

use super::navigation::Direction;

const SWIPE_TRIGGER_DISTANCE: f64 = 120.0;
/// Movement needed before the gesture picks an axis; first events are often diagonal.
const SWIPE_AXIS_LOCK_DISTANCE: f64 = 8.0;
/// Vertical scrolls drift sideways, so a swipe's horizontal travel must clearly dominate once
/// it has moved far enough for its direction to be more than noise.
const SWIPE_HORIZONTAL_DOMINANCE: f64 = 2.0;
const SWIPE_DOMINANCE_DISTANCE: f64 = 30.0;
const MOMENTUM_SAMPLE_SIZE: usize = 5;

/// The main content's box (`[data-slot=sidebar-inset]`), where the indicator is pinned.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SwipeBounds {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

/// One wheel event, with what the host knows about where it landed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwipeWheel {
    pub delta_x: f64,
    pub delta_y: f64,
    pub ctrl: bool,
    /// `deltaMode === DOM_DELTA_PIXEL` (a trackpad, not a line-stepping mouse wheel).
    pub pixels: bool,
    pub y: f64,
    /// The event started inside a dialog.
    pub in_dialog: bool,
    /// A scroller under the pointer can still scroll horizontally in the swipe's direction:
    /// asked with the accumulated horizontal delta when the axis locks.
    pub can_scroll_horizontally: bool,
}

/// `SwipeIndicatorState`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwipeIndicator {
    pub direction: Direction,
    pub progress: f64,
    pub y: f64,
    pub bounds: SwipeBounds,
    pub tracking: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum Gesture {
    Pending {
        y: f64,
        sum_x: f64,
        sum_y: f64,
        in_dialog: bool,
    },
    Tracking {
        direction: Direction,
        distance: f64,
        deltas: Vec<f64>,
        travel_x: f64,
        travel_y: f64,
        y: f64,
        bounds: SwipeBounds,
    },
    Ignored,
}

fn is_vertical_scroll(travel_x: f64, travel_y: f64) -> bool {
    travel_x + travel_y >= SWIPE_DOMINANCE_DISTANCE
        && travel_x <= SWIPE_HORIZONTAL_DOMINANCE * travel_y
}

/// `isMomentumTail`: macOS momentum after the fingers lift decays by a near-constant ratio per
/// event; finger movement is noisier. Deltas are signed along the swipe.
pub fn is_momentum_tail(deltas: &[f64]) -> bool {
    if deltas.len() < MOMENTUM_SAMPLE_SIZE {
        return false;
    }
    let recent = &deltas[deltas.len() - MOMENTUM_SAMPLE_SIZE..];
    if recent.iter().any(|delta| *delta <= 0.0) {
        return false;
    }
    let ratios: Vec<f64> = recent.windows(2).map(|pair| pair[1] / pair[0]).collect();
    let max = ratios.iter().copied().fold(f64::MIN, f64::max);
    let min = ratios.iter().copied().fold(f64::MAX, f64::min);
    ratios.iter().all(|ratio| (0.7..1.0).contains(ratio)) && max - min < 0.1
}

/// What one event decided.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SwipeUpdate {
    /// The indicator to show; `Some` with `tracking: false` fades it out; `None` leaves it.
    pub indicator: Option<SwipeIndicator>,
    pub navigate: Option<Direction>,
}

/// `HistorySwipeNavigation`'s listener state.
#[derive(Debug, Clone, Default)]
pub struct HistorySwipe {
    gesture: Option<Gesture>,
    shown: Option<SwipeIndicator>,
}

impl HistorySwipe {
    fn hide(&mut self) -> SwipeUpdate {
        let indicator = self.shown.map(|shown| SwipeIndicator {
            tracking: false,
            ..shown
        });
        self.shown = indicator;
        SwipeUpdate {
            indicator,
            navigate: None,
        }
    }

    /// The fingers lifted: navigate if the swipe went far enough, then ignore its momentum.
    fn release(&mut self) -> SwipeUpdate {
        let navigate = match &self.gesture {
            Some(Gesture::Tracking {
                direction,
                distance,
                ..
            }) if *distance >= SWIPE_TRIGGER_DISTANCE => Some(*direction),
            _ => None,
        };
        self.gesture = Some(Gesture::Ignored);
        SwipeUpdate {
            navigate,
            ..self.hide()
        }
    }

    /// `onTrackpadScrollBegin`.
    pub fn scroll_begin(&mut self) {
        if self.gesture == Some(Gesture::Ignored) {
            self.gesture = None;
        }
    }

    /// `onTrackpadScrollEnd`.
    pub fn scroll_end(&mut self) -> SwipeUpdate {
        let update = if matches!(self.gesture, Some(Gesture::Tracking { .. })) {
            self.release()
        } else {
            SwipeUpdate::default()
        };
        self.gesture = None;
        update
    }

    /// `onWheel`. `can_navigate` is `canNavigateHistory`; `bounds` reads the main content.
    pub fn wheel(
        &mut self,
        event: SwipeWheel,
        can_navigate: impl Fn(Direction) -> bool,
        bounds: impl Fn() -> SwipeBounds,
    ) -> SwipeUpdate {
        let gesture = self.gesture.get_or_insert(if event.ctrl || !event.pixels {
            Gesture::Ignored
        } else {
            Gesture::Pending {
                y: event.y,
                sum_x: 0.0,
                sum_y: 0.0,
                in_dialog: false,
            }
        });
        match gesture {
            Gesture::Ignored => return SwipeUpdate::default(),
            Gesture::Pending {
                y,
                sum_x,
                sum_y,
                in_dialog,
            } => {
                *sum_x += event.delta_x;
                *sum_y += event.delta_y;
                // The composed paths of every pending event count, as the web merges them.
                *in_dialog |= event.in_dialog;
                if sum_x.abs() + sum_y.abs() < SWIPE_AXIS_LOCK_DISTANCE {
                    return SwipeUpdate::default();
                }
                let (y, sum_x, sum_y, in_dialog) = (*y, *sum_x, *sum_y, *in_dialog);
                let direction = if sum_x < 0.0 {
                    Direction::Back
                } else {
                    Direction::Forward
                };
                let ignored = sum_x.abs() <= sum_y.abs()
                    || is_vertical_scroll(sum_x.abs(), sum_y.abs())
                    || in_dialog
                    || event.can_scroll_horizontally
                    || !can_navigate(direction);
                *gesture = if ignored {
                    Gesture::Ignored
                } else {
                    Gesture::Tracking {
                        direction,
                        distance: sum_x.abs(),
                        deltas: Vec::new(),
                        travel_x: sum_x.abs(),
                        travel_y: sum_y.abs(),
                        y,
                        bounds: bounds(),
                    }
                };
                if ignored {
                    return SwipeUpdate::default();
                }
            }
            Gesture::Tracking {
                direction,
                distance,
                deltas,
                travel_x,
                travel_y,
                ..
            } => {
                let delta = if *direction == Direction::Back {
                    -event.delta_x
                } else {
                    event.delta_x
                };
                if event.delta_x == 0.0 && event.delta_y == 0.0 {
                    return self.release();
                }
                *travel_x += event.delta_x.abs();
                *travel_y += event.delta_y.abs();
                if is_vertical_scroll(*travel_x, *travel_y) {
                    self.gesture = Some(Gesture::Ignored);
                    return self.hide();
                }
                if deltas.len() >= MOMENTUM_SAMPLE_SIZE {
                    deltas.remove(0);
                }
                deltas.push(delta);
                if is_momentum_tail(deltas) {
                    return self.release();
                }
                *distance = (*distance + delta).max(0.0);
            }
        }
        let Some(Gesture::Tracking {
            direction,
            distance,
            y,
            bounds,
            ..
        }) = &self.gesture
        else {
            return SwipeUpdate::default();
        };
        let indicator = SwipeIndicator {
            direction: *direction,
            progress: (distance / SWIPE_TRIGGER_DISTANCE).min(1.0),
            y: *y,
            bounds: *bounds,
            tracking: true,
        };
        self.shown = Some(indicator);
        SwipeUpdate {
            indicator: Some(indicator),
            navigate: None,
        }
    }
}

/// The indicator's geometry (SwipeIndicator): the circle's top within the bounds, clamped
/// inside them, and how far it is translated out, as a percentage of its width.
pub fn indicator_top(indicator: &SwipeIndicator) -> f64 {
    let size = 56.0;
    (indicator.y - indicator.bounds.top - size / 2.0)
        .clamp(0.0, (indicator.bounds.height - size).max(0.0))
}

pub fn indicator_hidden_percent(indicator: &SwipeIndicator) -> f64 {
    100.0 - indicator.progress * 50.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn momentum_tails_are_recognised() {
        let cases: [(&str, &[f64], bool); 5] = [
            (
                "steady momentum decay",
                &[40.0, 37.0, 34.0, 31.3, 28.8],
                true,
            ),
            (
                "fingers still moving",
                &[12.0, 18.0, 9.0, 15.0, 11.0],
                false,
            ),
            (
                "fingers slowing unevenly",
                &[30.0, 22.0, 20.0, 12.0, 11.0],
                false,
            ),
            ("reversing", &[10.0, 8.0, 6.0, 4.0, -2.0], false),
            ("too few samples", &[40.0, 37.0, 34.0], false),
        ];
        for (name, deltas, expected) in cases {
            assert_eq!(is_momentum_tail(deltas), expected, "{name}");
        }
    }

    fn wheel(delta_x: f64, delta_y: f64) -> SwipeWheel {
        SwipeWheel {
            delta_x,
            delta_y,
            ctrl: false,
            pixels: true,
            y: 100.0,
            in_dialog: false,
            can_scroll_horizontally: false,
        }
    }

    /// The browser test's sequence: the events, then the `[0, 0]` the fingers lifting sends.
    fn navigations(events: &[(f64, f64)], scroller_can_scroll: bool, in_dialog: bool) -> usize {
        let mut swipe = HistorySwipe::default();
        let mut count = 0;
        for (index, (x, y)) in events.iter().chain([(0.0, 0.0)].iter()).enumerate() {
            let event = SwipeWheel {
                can_scroll_horizontally: scroller_can_scroll,
                in_dialog: in_dialog && index == 0,
                ..wheel(*x, *y)
            };
            count += usize::from(
                swipe
                    .wheel(event, |_| true, SwipeBounds::default)
                    .navigate
                    .is_some(),
            );
        }
        count
    }

    #[test]
    fn swipes_navigate_and_vertical_scrolls_do_not() {
        let drift: Vec<(f64, f64)> = std::iter::once((-5.0, 3.0))
            .chain(std::iter::repeat_n((-4.0, 15.0), 30))
            .collect();
        assert_eq!(
            navigations(&drift, false, false),
            0,
            "slightly diagonal vertical scroll"
        );
        assert_eq!(
            navigations(&[(-5.0, 3.0), (-2.0, 2.0), (-150.0, 0.0)], false, false),
            1,
            "a swipe leaning diagonally for two events"
        );
        assert_eq!(
            navigations(&[(-130.0, 100.0)], false, false),
            0,
            "one diagonal event"
        );
        assert_eq!(
            navigations(&[(-130.0, 0.0)], false, false),
            1,
            "one long horizontal event"
        );
        assert_eq!(navigations(&[(-10.0, 0.0), (-130.0, 0.0)], false, false), 1);
        assert_eq!(
            navigations(&[(-10.0, 0.0), (-130.0, 0.0)], true, false),
            0,
            "a scroller that can still scroll keeps the thread"
        );
        assert_eq!(
            navigations(&[(-1.0, 0.0), (-10.0, 0.0), (-130.0, 0.0)], false, true),
            0,
            "a swipe that started in a dialog keeps the thread"
        );
    }

    #[test]
    fn line_wheels_ctrl_and_unnavigable_directions_are_ignored_until_the_next_scroll() {
        let mut swipe = HistorySwipe::default();
        let line = SwipeWheel {
            pixels: false,
            ..wheel(-200.0, 0.0)
        };
        assert_eq!(
            swipe.wheel(line, |_| true, SwipeBounds::default),
            SwipeUpdate::default()
        );
        assert_eq!(
            swipe.wheel(wheel(-200.0, 0.0), |_| true, SwipeBounds::default),
            SwipeUpdate::default(),
            "the rest of an ignored gesture is ignored"
        );
        swipe.scroll_end();
        swipe.scroll_begin();
        let update = swipe.wheel(
            wheel(-20.0, 0.0),
            |direction| direction == Direction::Forward,
            SwipeBounds::default,
        );
        assert_eq!(update.indicator, None, "back is not possible");
    }

    #[test]
    fn the_indicator_tracks_progress_and_scroll_end_releases() {
        let mut swipe = HistorySwipe::default();
        swipe.scroll_begin();
        let first = swipe.wheel(wheel(-60.0, 0.0), |_| true, SwipeBounds::default);
        let indicator = first.indicator.expect("tracking");
        assert_eq!(indicator.direction, Direction::Back);
        assert_eq!(indicator.progress, 0.5);
        let second = swipe.wheel(wheel(-90.0, 0.0), |_| true, SwipeBounds::default);
        assert_eq!(second.indicator.map(|i| i.progress), Some(1.0));
        let end = swipe.scroll_end();
        assert_eq!(end.navigate, Some(Direction::Back));
        assert_eq!(end.indicator.map(|i| i.tracking), Some(false));
        assert_eq!(indicator_hidden_percent(&indicator), 75.0);
    }
}
