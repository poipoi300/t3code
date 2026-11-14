//! A real table caller keeps controls and row state while horizontally scrolling.
use crate::support::{DESKTOP, PHONE};
use nsh_core::{
    platform::{Idiom, Os},
    prelude::*,
};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::{theme, ui::table::*};

#[component]
fn Fixture() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let expanded = signal(false);
    let selected = signal(false);
    view()
        .w_full()
        .h_full()
        .child(view().w(px(300.0)).child(table(
            "Models",
            vec![200.0, 200.0, 200.0],
            move |layout| {
                view()
                    .child(table_header(table_row(
                        Default::default(),
                        vec![
                            table_head(&layout, 0, false, text(String::from("Name")))
                                .into_element(),
                            table_head(&layout, 1, false, text(String::from("Provider")))
                                .into_element(),
                            table_head(&layout, 2, false, text(String::from("Actions")))
                                .into_element(),
                        ],
                    )))
                    .child(table_body(table_row(
                        RowOptions {
                            expanded: expanded.into(),
                            selected: selected.into(),
                            last_body_row: true.into(),
                        },
                        vec![
                            table_cell(
                                &layout,
                                0,
                                false,
                                nsh_base::button()
                                    .accessibility_label("Expand model")
                                    .on_press(move || expanded.update(|v| *v = !*v))
                                    .child(text(String::from("Model"))),
                            )
                            .into_element(),
                            table_cell(&layout, 1, false, text(String::from("Example provider")))
                                .into_element(),
                            table_cell(
                                &layout,
                                2,
                                false,
                                nsh_base::button()
                                    .accessibility_label("Select model")
                                    .on_press(move || selected.update(|v| *v = !*v))
                                    .child(text(String::from("Select"))),
                            )
                            .into_element(),
                        ],
                    )))
                    .child(when(
                        move || expanded.get(),
                        || text(String::from("Model details")),
                    ))
            },
        )))
        .child(text(move || format!("selected {}", selected.get())))
}

fn start(window: (f32, f32)) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<Fixture>())
    };
    config.environment.os = Os::Web;
    config.environment.idiom = if window == PHONE {
        Idiom::Phone
    } else {
        Idiom::Desktop
    };
    let mut app = Headless::start(config).expect("table");
    app.wait_for("role:table[Models]").expect("table mounted");
    app
}

#[test]
fn horizontal_scroll_reaches_actions_without_losing_expansion_or_selection() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window);
        app.press("role:button[Expand model]")
            .expect("expand first column");
        app.wait_for("Model details").expect("expanded content");
        app.scroll("role:table[Models]", px(300.0))
            .expect("reach last column");
        app.press("role:button[Select model]")
            .expect("choose last-column control");
        app.wait_for("selected true").expect("caller state updated");
        assert!(app.screen().shows("Model details"));
        app.scroll("role:table[Models]", px(-300.0))
            .expect("return to first column");
        app.press("role:button[Expand model]").expect("collapse");
        assert!(!app.screen().shows("Model details"));
        assert!(app.screen().shows("selected true"));
    }
}
