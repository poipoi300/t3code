//! Native scrolling drives the actual scroll-area viewport and its overflow observation.
use crate::support::{DESKTOP, PHONE};
use nsh_core::{
    platform::{Idiom, Os},
    prelude::*,
};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::{i18n::Text, theme, ui::scroll_area::*};

#[derive(Clone, PartialEq, Props)]
struct FixtureProps {
    horizontal: bool,
    hidden: bool,
}
#[component]
fn Fixture(props: FixtureProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let target = element_ref();
    let edges = scroll_fade_edges(target);
    view()
        .w_full()
        .h_full()
        .child(
            view()
                .w(px(240.0))
                .h(px(160.0))
                .child(scroll_area_with_options(
                    ScrollAreaOptions {
                        horizontal: props.horizontal,
                        hide_scrollbars: props.hidden,
                        scroll_fade: true,
                        viewport_ref: Some(target),
                        label: Some(Text::from("Rows")),
                        ..Default::default()
                    },
                    view()
                        .w(px(if props.horizontal { 900.0 } else { 220.0 }))
                        .h(px(if props.horizontal { 140.0 } else { 900.0 }))
                        .child(text(String::from("Scrollable content"))),
                )),
        )
        .child(text(move || {
            format!(
                "fade {:.0},{:.0},{:.0},{:.0}",
                edges.get().top,
                edges.get().bottom,
                edges.get().left,
                edges.get().right
            )
        }))
}

fn start(window: (f32, f32), horizontal: bool, hidden: bool) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<Fixture>(move || FixtureProps {
            horizontal,
            hidden,
        }))
    };
    config.environment.os = Os::Web;
    config.environment.idiom = if window == PHONE {
        Idiom::Phone
    } else {
        Idiom::Desktop
    };
    let mut app = Headless::start(config).expect("scroll fixture");
    app.wait_for("Scrollable content").expect("viewport");
    app
}

#[test]
fn vertical_and_horizontal_viewports_scroll_and_report_current_fade_edges() {
    for window in [DESKTOP, PHONE] {
        for horizontal in [false, true] {
            let mut app = start(window, horizontal, false);
            app.scroll("label:Rows", px(80.0))
                .expect("scroll real viewport");
            app.wait_for(if horizontal {
                "fade 0,0,24,24"
            } else {
                "fade 24,24,0,0"
            })
            .expect("edge distances update");
            app.scroll("label:Rows", px(-900.0))
                .expect("back to beginning");
            app.wait_for(if horizontal {
                "fade 0,0,0,24"
            } else {
                "fade 0,24,0,0"
            })
            .expect("start fade removed");
        }
    }
}

#[test]
fn hiding_indicators_keeps_content_scrollable() {
    let mut app = start(PHONE, false, true);
    assert!(!app.screen().shows("role:scrollbar[Vertical scroll]"));
    app.scroll("label:Rows", px(100.0))
        .expect("scroll without indicator");
    app.wait_for("fade 24,24,0,0")
        .expect("viewport still moves");
}
