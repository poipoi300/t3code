//! UI-COLOR: the plane and hue slider as ThemeColorPicker mounts them, holding HSV state and
//! showing the hex it would save (colorPickers.browser.test.tsx "theme color field").
use std::rc::Rc;

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::view_logic::color::{hex_to_hsv, hsv_to_hex};
use t3_ui::{
    i18n::Text,
    theme,
    ui::color_picker::{PlaneVariant, color_hue_slider, color_saturation_value_plane},
};

#[component]
fn Fixture() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let hsv = signal(hex_to_hsv("#804040"));
    let ends = signal(0u32);
    let end: Rc<dyn Fn()> = Rc::new(move || ends.update(|n| *n += 1));
    view()
        .app_class(theme::tw("flex w-64 flex-col gap-3 p-3"))
        .child(text(move || {
            let value = hsv.get();
            format!("hex {}", hsv_to_hex(value.h, value.s, value.v))
        }))
        .child(text(move || format!("hue {}", hsv.get().h.round())))
        .child(text(move || format!("ends {}", ends.get())))
        .child(color_saturation_value_plane(
            Text::content("Accent color picker"),
            move || hsv.get(),
            PlaneVariant::Inset,
            move |next| hsv.set(next),
            Some(end.clone()),
        ))
        .child(color_hue_slider(
            Text::content("Accent color picker hue"),
            move || hsv.get().h,
            move |h| hsv.update(|value| value.h = h),
            Some(end),
        ))
}

fn start(window: (f32, f32)) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<Fixture>())
    };
    // Tab stops at views on the web, macOS and Linux (Windows moves it between fields only).
    config.environment.os = nsh_core::platform::Os::Linux;
    let mut app = Headless::start(config).expect("the engine starts");
    app.wait_for("hex #804040").expect("initial colour");
    app
}

#[test]
fn hue_keys_reach_both_endpoints_without_changing_saturation_or_brightness() {
    let mut app = start(DESKTOP);
    // Tab order: saturation, brightness, then the hue slider.
    for _ in 0..3 {
        app.press_key("Tab").expect("tab");
    }
    app.press_key("Home").expect("home");
    app.wait_for("hue 0").expect("hue 0");
    assert!(app.shows("hex #804040"));
    app.press_key("End").expect("end");
    app.wait_for("hue 360").expect("End is 360, not wrapped");
    assert!(app.shows("hex #804040"));
    app.press_key("shift+ArrowRight").expect("large step");
    app.wait_for("hue 10").expect("wraps past 360");
    app.wait_for("hex #804b40").expect("hue applied");
    assert_eq!(
        app.screen()
            .count("role:adjustable[Accent color picker hue]")
            .expect("count"),
        1
    );
    assert!(app.shows("ends 0"), "keys are not drags");
}

#[test]
fn plane_keys_adjust_the_focused_axis_and_show_its_caption() {
    let mut app = start(DESKTOP);
    app.press_key("Tab").expect("focus saturation");
    app.press_key("Home").expect("no saturation");
    app.wait_for("Saturation 0%")
        .expect("caption follows keyboard focus");
    app.wait_for("hex #808080")
        .expect("grey at the same brightness");
    app.press_key("Tab").expect("focus brightness");
    app.press_key("End").expect("full brightness");
    app.wait_for("Brightness 100%").expect("brightness caption");
    app.wait_for("hex #ffffff").expect("white");
}

#[test]
fn dragging_to_the_planes_corner_picks_white_and_ends_once() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window);
        app.drag(
            "role:group[Accent color picker saturation and brightness]",
            point(px(-400.0), px(-400.0)),
        )
        .expect("drag to the top-left corner");
        app.wait_for("hex #ffffff").expect("white");
        app.wait_for("ends 1").expect("one interaction end");
        app.drag(
            "role:adjustable[Accent color picker hue]",
            point(px(400.0), px(0.0)),
        )
        .expect("drag hue to the end");
        app.wait_for("hue 360").expect("hue end");
        app.wait_for("ends 2").expect("one more end");
    }
}
