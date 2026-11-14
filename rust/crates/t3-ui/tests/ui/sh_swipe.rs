//! SH-SWIPE: two-finger horizontal trackpad swipes over the main content go back and forward on
//! the macOS desktop app, and nowhere else.
use std::rc::Rc;

use crate::support::DESKTOP;
use nsh_core::platform::Os;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::view_logic::shell::navigation::{Direction, HistoryAvailability};
use t3_ui::{screens::history_swipe::history_swipe_area, theme};

#[component]
fn Fixture() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let backs = signal(0u32);
    view().w_full().h_full().child(history_swipe_area(
        Rc::new(|| HistoryAvailability {
            back: true,
            forward: false,
        }),
        Rc::new(move |direction| {
            if direction == Direction::Back {
                backs.update(|n| *n += 1);
            }
        }),
        view()
            .test_id("main")
            .w_full()
            .h_full()
            .child(text(move || format!("backs {}", backs.get()))),
    ))
}

fn start(os: Os) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(DESKTOP.0), px(DESKTOP.1)),
        clock: nsh_engine::Clock::manual(std::time::UNIX_EPOCH),
        ..EngineConfig::new(Root::of::<Fixture>())
    };
    config.environment.os = os;
    let mut app = Headless::start(config).expect("the engine starts");
    app.wait_for("backs 0").expect("mounted");
    app
}

fn swipe(app: &mut Headless, events: &[(f32, f32)]) {
    for (x, y) in events {
        app.wheel("id:main", point(px(*x), px(*y))).expect("wheel");
    }
    // The fingers lift: the pause ends the scroll.
    app.fast_forward(std::time::Duration::from_millis(400))
        .expect("pause");
    app.settle();
}

#[test]
fn a_back_swipe_navigates_once_on_macos_and_a_vertical_scroll_does_not() {
    let mut app = start(Os::MacOs);
    swipe(&mut app, &[(-10.0, 0.0), (-60.0, 0.0), (-70.0, 0.0)]);
    app.wait_for("backs 1").expect("navigated back");
    let drift: Vec<(f32, f32)> = std::iter::once((-5.0, 3.0))
        .chain(std::iter::repeat_n((-4.0, 15.0), 30))
        .collect();
    swipe(&mut app, &drift);
    assert!(
        app.shows("backs 1"),
        "a slightly diagonal vertical scroll keeps the thread"
    );
    // Forward is not available: a forward swipe does nothing.
    swipe(&mut app, &[(10.0, 0.0), (60.0, 0.0), (70.0, 0.0)]);
    assert!(app.shows("backs 1"));
}

#[test]
fn other_platforms_never_navigate_by_wheel() {
    for os in [Os::Windows, Os::Web] {
        let mut app = start(os);
        swipe(&mut app, &[(-10.0, 0.0), (-60.0, 0.0), (-70.0, 0.0)]);
        assert!(app.shows("backs 0"), "{os:?}: a tilt wheel is not a swipe");
    }
}
