//! SH-TITLEBAR: native geometry, fullscreen reversal and the web's responsive inset rule.

use nsh_core::desktop::{TitleBarStyle, WindowMode, WindowState};
use nsh_core::geometry::{Bounds, Edges, Points};
use nsh_core::platform::Os;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::i18n::Text;
use t3_ui::screens::titlebar::{
    desktop_titlebar, install_window_titlebar, titlebar_controls, window_title_bar_options,
    workspace_page_header,
};
use t3_ui::state::panels::Panels;

use crate::support::{DESKTOP, PHONE};

#[component]
fn TitlebarFixture() -> impl IntoElement {
    let panels = Panels::new();
    provide_context(panels);
    install_window_titlebar(Text::content("T3 Code"));
    view()
        .w_full()
        .h_full()
        .child(desktop_titlebar())
        .child(workspace_page_header(
            true,
            titlebar_controls(
                nsh_base::button()
                    .test_id("header-action")
                    .accessibility_label(Some(SharedString::from("Toggle sidebar")))
                    .on_press(move || panels.sidebar_open.update(|open| *open = !*open))
                    .w(px(32.0))
                    .h(px(28.0))
                    .child(text(SharedString::from("Toggle sidebar"))),
            ),
        ))
}

fn start(platform: Os, window: (f32, f32)) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<TitlebarFixture>())
    };
    config.environment.os = platform;
    let mut app = Headless::start(config).expect("titlebar engine");
    app.first_render().expect("header mounts");
    app
}

fn bounds(app: &Headless, id: &str) -> Bounds<Points> {
    app.screen()
        .layout_nodes()
        .iter()
        .find(|node| node.test_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("mounted {id}"))
        .bounds
}

#[test]
fn native_controls_are_requested_only_for_desktop_with_the_webs_platform_geometry() {
    for platform in [Os::Web, Os::Android, Os::Ios] {
        assert!(window_title_bar_options(platform, false).is_none());
    }
    let mac = window_title_bar_options(Os::MacOs, true).expect("desktop chrome");
    assert_eq!(mac.style, TitleBarStyle::Hidden);
    assert_eq!(mac.height, Some(px(52.0)));
    assert_eq!(mac.controls_offset, Some(point(px(16.0), px(20.0))));
    for platform in [Os::Windows, Os::Linux] {
        let options = window_title_bar_options(platform, true).expect("caption buttons");
        assert_eq!(options.style, TitleBarStyle::Hidden);
        assert_eq!(options.height, Some(px(40.0)));
        assert!(options.controls_offset.is_none());
    }
}

#[test]
fn web_and_mobile_draw_no_native_bar_and_use_the_same_collapsed_and_narrow_insets() {
    for platform in [Os::Web, Os::Android, Os::Ios] {
        for viewport in [DESKTOP, PHONE] {
            let mut app = start(platform, viewport);
            assert_eq!(app.screen().count("id:desktop-titlebar"), Ok(0));
            assert_eq!(bounds(&app, "workspace-page-header").size.height, px(52.0));
            let expanded_left = if viewport == PHONE { 52.0 } else { 12.0 };
            assert_eq!(bounds(&app, "header-action").origin.x, px(expanded_left));
            app.press("id:header-action")
                .expect("interactive child is not a drag region");
            assert_eq!(bounds(&app, "header-action").origin.x, px(52.0));
        }
    }
}

#[test]
fn native_geometry_clears_traffic_lights_and_fullscreen_releases_the_inset() {
    let mut app = start(Os::MacOs, DESKTOP);
    app.press("id:header-action").expect("collapse sidebar");
    let report = |app: &mut Headless, area: Edges<Points>, fullscreen: bool| {
        app.set_environment("native titlebar geometry", move |environment| {
            environment.title_bar = area;
            let mut state = environment
                .window_state
                .unwrap_or_else(WindowState::default);
            state.mode = if fullscreen {
                WindowMode::FullScreen
            } else {
                WindowMode::Normal
            };
            environment.window_state = Some(state);
        })
        .expect("native geometry reaches shared bindings");
    };
    report(
        &mut app,
        Edges {
            top: px(52.0),
            left: px(78.0),
            right: px(0.0),
            bottom: px(0.0),
        },
        false,
    );
    assert_eq!(bounds(&app, "header-action").origin.x, px(130.0));
    report(&mut app, Edges::default(), true);
    assert_eq!(bounds(&app, "header-action").origin.x, px(52.0));
    report(
        &mut app,
        Edges {
            top: px(52.0),
            left: px(78.0),
            right: px(0.0),
            bottom: px(0.0),
        },
        false,
    );
    assert_eq!(bounds(&app, "header-action").origin.x, px(130.0));
}
