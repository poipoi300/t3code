//! Port of apps/web/src/components/ui/color-picker.tsx ColorSaturationValuePlane, ColorHueSlider
//! and their useColorDrag.
//!
//! The colour model and its hex conversions are the client's port of lib/color.ts
//! (`t3_client::view_logic::color`); these controls only read and report `HsvColor`s.
//!
//! Departures, each forced by NSH:
//! - Pointer capture is NSH's touch responder: the control that took the pointer hears it until
//!   release, and a mouse drag on desktop reaches it as a touch would.
//! - The plane's two `sr-only` range inputs are two focusable, undrawn adjustable views at the
//!   plane's corner, carrying the inputs' labels, values and keys. NSH has no `:focus-visible`
//!   query, so "keyboard focus" (the axis caption and the plane's ring) means focus that did not
//!   come from the plane's own pointer focus, or any key pressed while focused, which is the rule
//!   browsers apply.
//! - The thumbs move by transform (`translate`), so their 80ms linear tween is a transform
//!   transition; CSS animated `left`/`top`, which NSH cannot transition.
//! - The plane's two background gradients are two stacked layers (NSH paints one per view).
//! - `className` is not taken (no caller passes one); `touch-none` has no NSH meaning, the
//!   responder already keeps the gesture from scrolling.

use std::rc::Rc;

use nsh_core::element::{ClaimRequest, ResponderEvent, TouchClaim};
use nsh_core::geometry::point;
use nsh_core::keymap::{Key, Keystroke, Modifiers};
use nsh_core::prelude::*;
use nsh_core::{AccessibilityActionDef, AccessibilityValue, KeyCapture, KeyEvent};

use crate::i18n::{Text, Value, t, tr};
use crate::theme::{cn, tw};
pub use t3_client::view_logic::color::HsvColor;
use t3_client::view_logic::color::hsv_to_hex;

/// The TSX `variant` of ColorSaturationValuePlane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlaneVariant {
    #[default]
    Inset,
    Edge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Axis {
    Saturation,
    Brightness,
}

impl Axis {
    fn of(self, value: HsvColor) -> f64 {
        match self {
            Self::Saturation => value.s,
            Self::Brightness => value.v,
        }
    }

    fn with(self, value: HsvColor, next: f64) -> HsvColor {
        match self {
            Self::Saturation => HsvColor { s: next, ..value },
            Self::Brightness => HsvColor { v: next, ..value },
        }
    }
}

fn clamp(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

/// ColorSaturationValuePlane's `handleKeyDown` for one axis; `None` for a key it ignores.
pub fn plane_key_value(current: f64, key: &str, shift: bool) -> Option<f64> {
    let step = if shift { 0.1 } else { 0.02 };
    Some(match key {
        "ArrowRight" | "ArrowUp" => clamp(current + step),
        "ArrowLeft" | "ArrowDown" => clamp(current - step),
        "Home" => 0.0,
        "End" => 1.0,
        _ => return None,
    })
}

/// ColorHueSlider's `handleKeyDown`; `None` for a key it ignores.
pub fn hue_key_value(current: f64, key: &str, shift: bool) -> Option<f64> {
    match key {
        "Home" => Some(0.0),
        "End" => Some(360.0),
        "ArrowRight" | "ArrowUp" | "ArrowLeft" | "ArrowDown" => {
            let step = if shift { 10.0 } else { 1.0 };
            let direction = if matches!(key, "ArrowRight" | "ArrowUp") {
                1.0
            } else {
                -1.0
            };
            Some((current + direction * step + 360.0) % 360.0)
        }
        _ => None,
    }
}

/// The plane's drag update: saturation from x, brightness from y; `None` before layout.
pub fn plane_point_value(value: HsvColor, x: f32, y: f32, size: Size<Points>) -> Option<HsvColor> {
    let (width, height) = (f64::from(size.width.0), f64::from(size.height.0));
    (width > 0.0 && height > 0.0).then(|| HsvColor {
        s: clamp(f64::from(x) / width),
        v: 1.0 - clamp(f64::from(y) / height),
        ..value
    })
}

/// The hue slider's drag update; `None` before layout.
pub fn hue_point_value(x: f32, width: f32) -> Option<f64> {
    (width > 0.0).then(|| clamp(f64::from(x) / f64::from(width)) * 360.0)
}

fn hue_color(hue: f64) -> AdaptiveColor {
    // hsl(h 100% 50%) is hsv(h, 1, 1).
    let hex = hsv_to_hex(hue, 1.0, 1.0);
    let rgb = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0);
    AdaptiveColor::rgb(rgb, rgb)
}

fn percent(value: f64) -> i64 {
    (value * 100.0 + 0.5).floor() as i64
}

fn arrow_capture() -> KeyCapture {
    let keys = [
        Key::Up,
        Key::Down,
        Key::Left,
        Key::Right,
        Key::Home,
        Key::End,
    ];
    KeyCapture::keys(keys.into_iter().flat_map(|key| {
        [false, true].map(|shift| Keystroke {
            key,
            modifiers: Modifiers {
                shift,
                ..Modifiers::default()
            },
        })
    }))
}

fn adjust_actions() -> Vec<AccessibilityActionDef> {
    ["increment", "decrement"]
        .map(|name| AccessibilityActionDef {
            name: name.into(),
            label: None,
        })
        .to_vec()
}

/// useColorDrag: one pointer at a time, updating on grant and every move, ending once.
#[derive(Clone, Copy)]
struct Drag {
    dragging: Signal<bool>,
    origin: Signal<Point<Points>>,
}

impl Drag {
    fn new() -> Self {
        Self {
            dragging: signal(false),
            origin: signal(Point::default()),
        }
    }

    fn thumb_transition(self) -> Option<&'static str> {
        (!self.dragging.get()).then_some("transition duration-80 ease-linear")
    }

    fn attach<E: Interactive>(
        self,
        element: E,
        update: Rc<dyn Fn(Point<Points>)>,
        on_start: Rc<dyn Fn()>,
        on_interaction_end: Option<Rc<dyn Fn()>>,
    ) -> E {
        let move_update = update.clone();
        let end = on_interaction_end.clone();
        let stop = move || {
            if self.dragging.get_untracked() {
                self.dragging.set(false);
                if let Some(end) = &end {
                    end();
                }
            }
        };
        let stop_released = stop.clone();
        element
            .claim_touches(TouchClaim::on_start().ask(move |request: &ClaimRequest| {
                if !self.dragging.get_untracked() {
                    request.claim();
                }
            }))
            .on_responder_grant(move |event: &ResponderEvent| {
                self.dragging.set(true);
                self.origin.set(event.location);
                on_start();
                update(event.location);
            })
            .on_responder_move(move |event: &ResponderEvent| {
                let origin = self.origin.get_untracked();
                move_update(point(
                    origin.x + event.gesture.delta.x,
                    origin.y + event.gesture.delta.y,
                ));
            })
            .on_responder_release(move |_: &ResponderEvent| stop_released())
            .on_responder_terminate(move |_: &ResponderEvent| stop())
    }
}

/// `ColorSaturationValuePlane`.
pub fn color_saturation_value_plane(
    label: Text,
    value: impl IntoReactive<HsvColor>,
    variant: PlaneVariant,
    on_change: impl Fn(HsvColor) + 'static,
    on_interaction_end: Option<Rc<dyn Fn()>>,
) -> impl IntoElement {
    let value = value.into_reactive();
    let current = computed(move || value.get());
    let on_change: Rc<dyn Fn(HsvColor)> = Rc::new(on_change);
    let size = signal(Size::<Points>::default());
    let drag = Drag::new();
    let saturation_ref = element_ref();
    let keyboard_focus = signal(None::<Axis>);
    let pointer_focusing = signal(false);
    let name = label.resolve();
    let plane_label = SharedString::from(tr(
        "{label} saturation and brightness",
        &[("label", Value::Text(name.to_string()))],
    ));

    let change = on_change.clone();
    let update: Rc<dyn Fn(Point<Points>)> = Rc::new(move |at: Point<Points>| {
        if let Some(next) = plane_point_value(
            current.get_untracked(),
            at.x.0,
            at.y.0,
            size.get_untracked(),
        ) {
            change(next);
        }
    });
    let on_start: Rc<dyn Fn()> = Rc::new(move || {
        pointer_focusing.set(true);
        saturation_ref.focus();
        pointer_focusing.set(false);
    });

    let axis_input = move |axis: Axis| {
        let change = on_change.clone();
        let key_change = on_change.clone();
        let axis_name = SharedString::from(match axis {
            Axis::Saturation => tr(
                "{label} saturation",
                &[("label", Value::Text(name.to_string()))],
            ),
            Axis::Brightness => tr(
                "{label} brightness",
                &[("label", Value::Text(name.to_string()))],
            ),
        });
        let input = view()
            .focusable(true)
            .role(AccessibilityRole::Adjustable)
            .label(axis_name)
            .accessibility_description(t(
                "Use arrow keys to adjust the focused value. Hold Shift for larger steps. Use Home and End for the minimum and maximum. Press Tab to move between saturation and brightness.",
            ))
            .accessibility_value(move || {
                let now = axis.of(current.get());
                Some(AccessibilityValue {
                    min: Some(0.0),
                    max: Some(100.0),
                    now: Some((now * 100.0) as f32),
                    text: Some(format!("{}%", percent(now)).into()),
                })
            })
            .accessibility_actions(adjust_actions())
            .on_accessibility_action(move |action: &str| {
                let key = match action {
                    "increment" => "ArrowUp",
                    "decrement" => "ArrowDown",
                    _ => return,
                };
                let value = current.get_untracked();
                if let Some(next) = plane_key_value(axis.of(value), key, false) {
                    change(axis.with(value, next));
                }
            })
            .capture_keys(arrow_capture())
            .on_key_down(move |event: &KeyEvent| {
                keyboard_focus.set(Some(axis));
                let value = current.get_untracked();
                if let Some(next) =
                    plane_key_value(axis.of(value), event.key.as_str(), event.modifiers.shift)
                {
                    key_change(axis.with(value, next));
                }
            })
            .on_focus(move || {
                if !pointer_focusing.get_untracked() {
                    keyboard_focus.set(Some(axis));
                }
            })
            .on_blur(move || {
                if keyboard_focus.get_untracked() == Some(axis) {
                    keyboard_focus.set(None);
                }
            })
            .app_class(tw("absolute top-0 left-0 size-px"));
        let input = if axis == Axis::Saturation {
            input.element_ref(&saturation_ref)
        } else {
            input
        };
        let caption = text(move || {
            let percent = Value::Int(percent(axis.of(current.get())));
            match axis {
                Axis::Saturation => tr("Saturation {value}%", &[("value", percent)]),
                Axis::Brightness => tr("Brightness {value}%", &[("value", percent)]),
            }
        })
        .accessible(false)
        .app_class(move || {
            tw(&cn([
                Some("pointer-events-none absolute bottom-2 left-2 z-10 rounded bg-popover px-1.5 py-0.5 text-xs text-popover-foreground"),
                (keyboard_focus.get() != Some(axis)).then_some("invisible"),
            ]))
        });
        vec![input.into_element(), caption.into_element()]
    };

    let plane = view()
        .role(AccessibilityRole::Group)
        .label(plane_label)
        .on_layout(move |bounds: &Bounds<Points>| size.set(bounds.size))
        .bg(move || hue_color(current.get().h))
        .app_class(move || {
            tw(&cn([
                Some("relative cursor-crosshair overflow-hidden"),
                keyboard_focus
                    .get()
                    .is_some()
                    .then_some("ring-2 ring-inset ring-ring ring-offset-2 ring-offset-popover"),
                Some(match variant {
                    PlaneVariant::Edge => "h-36 rounded-none",
                    PlaneVariant::Inset => "h-32 rounded-lg",
                }),
            ]))
        })
        .child(view().app_class(tw(
            "pointer-events-none absolute inset-0 bg-[linear-gradient(to_right,#fff,transparent)]",
        )))
        .child(view().app_class(tw(
            "pointer-events-none absolute inset-0 bg-[linear-gradient(to_top,#000,transparent)]",
        )))
        .child(axis_input(Axis::Saturation))
        .child(axis_input(Axis::Brightness))
        .child(
            view()
                .accessible(false)
                .translate_x(move || {
                    let width = size.get().width.0;
                    DefiniteLength::from(px(current.get().s as f32 * (width - 12.0)))
                })
                .translate_y(move || {
                    let height = size.get().height.0;
                    DefiniteLength::from(px((1.0 - current.get().v as f32) * (height - 12.0)))
                })
                .app_class(move || {
                    tw(&cn([
                        Some("pointer-events-none absolute top-0 left-0 size-3 rounded-full border-2 border-white shadow-[0_0_0_1px_rgb(0_0_0/0.4)]"),
                        drag.thumb_transition(),
                    ]))
                }),
        );
    drag.attach(plane, update, on_start, on_interaction_end)
}

/// `ColorHueSlider`.
pub fn color_hue_slider(
    label: Text,
    value: impl IntoReactive<f64>,
    on_change: impl Fn(f64) + 'static,
    on_interaction_end: Option<Rc<dyn Fn()>>,
) -> impl IntoElement {
    let value = value.into_reactive();
    let current = computed(move || value.get());
    let on_change: Rc<dyn Fn(f64)> = Rc::new(on_change);
    let width = signal(0.0f32);
    let drag = Drag::new();
    let slider_ref = element_ref();

    let change = on_change.clone();
    let update: Rc<dyn Fn(Point<Points>)> = Rc::new(move |at: Point<Points>| {
        if let Some(next) = hue_point_value(at.x.0, width.get_untracked()) {
            change(next);
        }
    });
    let on_start: Rc<dyn Fn()> = Rc::new(move || slider_ref.focus());
    let key_change = on_change.clone();
    let action_change = on_change;

    let slider = view()
        .element_ref(&slider_ref)
        .focusable(true)
        .role(AccessibilityRole::Adjustable)
        .label(label.resolve())
        .accessibility_value(move || {
            Some(AccessibilityValue {
                min: Some(0.0),
                max: Some(360.0),
                now: Some((current.get() + 0.5).floor() as f32),
                text: None,
            })
        })
        .accessibility_actions(adjust_actions())
        .on_accessibility_action(move |action: &str| {
            let key = match action {
                "increment" => "ArrowUp",
                "decrement" => "ArrowDown",
                _ => return,
            };
            if let Some(next) = hue_key_value(current.get_untracked(), key, false) {
                action_change(next);
            }
        })
        .capture_keys(arrow_capture())
        .on_key_down(move |event: &KeyEvent| {
            if let Some(next) =
                hue_key_value(current.get_untracked(), event.key.as_str(), event.modifiers.shift)
            {
                key_change(next);
            }
        })
        .on_layout(move |bounds: &Bounds<Points>| width.set(bounds.size.width.0))
        .app_class(tw("relative flex h-6 cursor-pointer items-center rounded-full outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-popover"))
        .child(
            view().accessible(false).app_class(tw(
                "h-2.5 w-full rounded-full bg-[linear-gradient(to_right,#f00,#ff0,#0f0,#0ff,#00f,#f0f,#f00)] shadow-[inset_0_0_0_1px_rgb(0_0_0_/_12%)]",
            )),
        )
        .child(
            // top-1/2 -translate-y-1/2 of a size-4 thumb in an h-6 row is top-1.
            view()
                .accessible(false)
                .bg(move || hue_color(current.get()))
                .translate_x(move || {
                    DefiniteLength::from(px((current.get() / 360.0) as f32 * (width.get() - 16.0)))
                })
                .app_class(move || {
                    tw(&cn([
                        Some("pointer-events-none absolute top-1 left-0 size-4 rounded-full border-2 border-white shadow-[0_0_0_1px_rgb(0_0_0/0.4)]"),
                        drag.thumb_transition(),
                    ]))
                }),
        );
    drag.attach(slider, update, on_start, on_interaction_end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plane_keys_step_clamp_and_jump() {
        assert_eq!(plane_key_value(0.5, "ArrowRight", false), Some(0.52));
        assert_eq!(plane_key_value(0.5, "ArrowDown", true), Some(0.4));
        assert_eq!(plane_key_value(0.99, "ArrowUp", false), Some(1.0));
        assert_eq!(plane_key_value(0.01, "ArrowLeft", false), Some(0.0));
        assert_eq!(plane_key_value(0.4, "Home", false), Some(0.0));
        assert_eq!(plane_key_value(0.4, "End", true), Some(1.0));
        assert_eq!(plane_key_value(0.4, "PageUp", false), None);
    }

    #[test]
    fn hue_keys_wrap_and_reach_both_endpoints() {
        // colorPickers.browser.test.tsx: Home is 0 and End is 360, not wrapped to 0.
        assert_eq!(hue_key_value(120.0, "Home", false), Some(0.0));
        assert_eq!(hue_key_value(120.0, "End", false), Some(360.0));
        assert_eq!(hue_key_value(360.0, "ArrowRight", true), Some(10.0));
        assert_eq!(hue_key_value(0.0, "ArrowLeft", false), Some(359.0));
        assert_eq!(hue_key_value(5.0, "ArrowDown", true), Some(355.0));
        assert_eq!(hue_key_value(5.0, "Enter", false), None);
    }

    #[test]
    fn pointer_positions_map_to_the_plane_and_the_hue() {
        let size = Size {
            width: px(200.0),
            height: px(100.0),
        };
        let red = HsvColor {
            h: 10.0,
            s: 0.5,
            v: 0.5,
        };
        // The top-left corner is white: no saturation, full brightness; hue is kept.
        assert_eq!(
            plane_point_value(red, 0.0, 0.0, size),
            Some(HsvColor {
                h: 10.0,
                s: 0.0,
                v: 1.0
            })
        );
        assert_eq!(
            plane_point_value(red, 250.0, 150.0, size),
            Some(HsvColor {
                h: 10.0,
                s: 1.0,
                v: 0.0
            })
        );
        assert_eq!(plane_point_value(red, 5.0, 5.0, Size::default()), None);
        assert_eq!(hue_point_value(50.0, 200.0), Some(90.0));
        assert_eq!(hue_point_value(-5.0, 200.0), Some(0.0));
        assert_eq!(hue_point_value(10.0, 0.0), None);
    }
}
