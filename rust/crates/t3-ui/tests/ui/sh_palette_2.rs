//! CommandPaletteResults interaction tests at the client projection boundary.

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use std::rc::Rc;
use t3_client::{
    ClientStore,
    view_logic::shell::palette::{self as client, Group, Item},
};
use t3_ui::{
    keys::bindings::Keybindings,
    runtime::Shared,
    screens::palette::{
        ContentOptions, command_palette_content,
        results::{
            Results, ResultsOptions, command_palette_results, command_palette_virtualized_results,
        },
    },
    theme::{self, ThemeChoice},
    ui::command::{CommandOptions, CommandState},
};

use crate::support::{DESKTOP, PHONE};

#[derive(Clone, PartialEq, Props)]
struct ResultsFixtureProps {
    virtualized: bool,
}

#[component]
fn ResultsFixture(props: ResultsFixtureProps) -> impl IntoElement {
    theme::install(&ThemeChoice::default());
    let keys = Keybindings::attach(ClientStore::default(), signal(vec![]), signal(None));
    let mut disabled = Item::action("denied", "Denied action");
    disabled.disabled = true;
    let mut items = vec![disabled];
    items.extend(
        (0..90).map(|index| Item::action(format!("item:{index}"), format!("Result {index}"))),
    );
    let groups = signal(Shared(Rc::new(client::root_groups(items, vec![]))));
    let results = computed(move || Results::new(groups.get()));
    let chosen = signal(String::new());
    let choices = signal(0u32);
    let state = CommandState::new(
        move || results.with(Results::command_items),
        CommandOptions {
            filter: false,
            ..Default::default()
        },
        move |value| {
            choices.update(|count| *count += 1);
            chosen.set(value.into());
        },
    );
    let options = ResultsOptions {
        keys,
        empty_state_message: None,
        actions_only: computed(move || state.query.with(|query| query.starts_with('>'))),
    };
    view()
        .w_full()
        .h_full()
        .child(text(move || format!("chosen:{}", chosen.get())))
        .child(text(move || format!("choices:{}", choices.get())))
        .child(
            nsh_base::button()
                .accessibility_label(Some("Remove results".into()))
                .on_press(move || groups.set(Shared(Rc::new(Vec::<Group>::new()))))
                .child(text(String::from("Remove results"))),
        )
        .child(view().h(px(300.0)).child(command_palette_content(
            state,
            ContentOptions::default(),
            if props.virtualized {
                command_palette_virtualized_results(state, results, options).into_element()
            } else {
                command_palette_results(state, results, options).into_element()
            },
        )))
}

fn start(size_: (f32, f32), virtualized: bool) -> Headless {
    let mut registry = Registry::new();
    registry.register::<ResultsFixture>();
    let mut sets = t3_ui::registries();
    sets.push(registry);
    Headless::start(EngineConfig {
        ui_sets: sets,
        viewport: size(px(size_.0), px(size_.1)),
        ..EngineConfig::new(Root::new::<ResultsFixture>(move || ResultsFixtureProps {
            virtualized,
        }))
    })
    .expect("results fixture")
}

#[test]
fn disabled_results_are_painted_but_keyboard_selection_skips_them() {
    for size_ in [PHONE, DESKTOP] {
        let mut app = start(size_, false);
        app.wait_for("Denied action")
            .expect("disabled row remains visible");
        app.press_key("enter").expect("choose first enabled row");
        app.wait_for("text:chosen:item:0")
            .expect("disabled row was excluded from selection");
        app.press_key("down").expect("next enabled item");
        app.press_key("enter").expect("choose next");
        app.wait_for("text:chosen:item:1").expect("row 1 selected");
        app.press("role:button[Remove results]")
            .expect("live catalogue changed");
        app.wait_for("No matching commands, projects, or threads.")
            .expect("empty state");
        app.type_text("role:combobox[Command palette]", "")
            .expect("return focus after the fixture's resource update");
        app.press_key("enter")
            .expect("stale selection cannot execute");
        assert!(app.screen().shows("chosen:item:1"));
        assert!(app.screen().shows("choices:2"));
    }
}

#[test]
fn virtualized_keyboard_selection_reaches_rows_outside_the_initial_window() {
    for size_ in [PHONE, DESKTOP] {
        let mut app = start(size_, true);
        app.wait_for("Result 0").expect("first window");
        for _ in 0..75 {
            app.press_key("down").expect("move highlight");
        }
        app.press_key("enter")
            .expect("activate offscreen result through CommandState");
        app.wait_for("text:chosen:item:75")
            .expect("client item 75 executed");
        app.wait_for("Result 75")
            .expect("virtual list followed selection");
    }
}
