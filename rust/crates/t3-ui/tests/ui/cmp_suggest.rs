//! CMP-SUGGEST: real editor keys, query lifecycle and suggestion-list interactions.

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::i18n::Text;
use t3_ui::screens::composer::{
    editor::{
        ComposerCommandKeyCallback, ComposerEditor, ComposerPromptEditorProps, ComposerResolvers,
        EditorMode, composer_prompt_editor_tiptap,
    },
    suggestions::{
        ComposerCommandItem, ComposerCommandItemKind, ComposerSuggestionResults,
        ComposerSuggestions, SkillSourceKind, composer_command_menu_layer,
    },
};

#[derive(Clone, PartialEq, Props)]
struct HarnessProps {
    count: usize,
}

#[component]
fn Harness(props: HarnessProps) -> impl IntoElement {
    t3_ui::theme::install(&t3_ui::theme::ThemeChoice::default());
    t3_ui::i18n::set_language("fr", &[]);
    let editor = ComposerEditor::new("", 0, EditorMode::Plain, ComposerResolvers::default())
        .expect("editor");
    let requests = signal(0);
    let selected = signal(String::new());
    let source = signal(String::new());
    let selections = signal(0);
    let state = ComposerSuggestions::new(
        editor.clone(),
        move |_| requests.update(|n| *n += 1),
        move |id, _, snapshot| {
            selected.set(format!("{id}:{}", snapshot.value));
            selections.update(|n| *n += 1);
            false
        },
    );
    let stale = signal(None);
    let accepted = signal(false);
    let literal = editor.clone();
    let disabled = signal(false);
    let anchor = element_ref();
    let width = signal(0.0);
    let count = props.count;
    let publish = move |empty: bool| {
        let Some(request) = state.request.get_untracked() else {
            return;
        };
        stale.set(Some(request.clone()));
        let trigger_kind = request.trigger.kind;
        state.publish(ComposerSuggestionResults {
            request,
            // A source's canned, already ordered projection. The fixture never filters or inserts.
            items: if empty {
                vec![]
            } else {
                (0..count)
                    .map(|i| ComposerCommandItem {
                        id: format!("result-{i}"),
                        label: Text::content(if i == 0 {
                            "Settings".into()
                        } else {
                            format!("Candidate {i}")
                        }),
                        description: Text::content(format!("src/result-{i}.rs")),
                        kind: if i == 1 {
                            ComposerCommandItemKind::Skill {
                                source: SkillSourceKind::Repo,
                                display_name: "Settings".into(),
                            }
                        } else { match trigger_kind {
                            t3_client::thread_views::composer::policy::TriggerKind::Skill => ComposerCommandItemKind::Skill { source: SkillSourceKind::Personal, display_name: "Settings".into() },
                            t3_client::thread_views::composer::policy::TriggerKind::SlashCommand => ComposerCommandItemKind::SlashCommand,
                            _ => ComposerCommandItemKind::Path { directory: false },
                        } },
                    })
                    .collect()
            },
            loading: false,
            empty_state: None,
        });
    };
    view()
        .w_full()
        .h_full()
        .justify_end()
        .p(px(24.0))
        .child(
            view()
                .element_ref(&anchor)
                .w_full()
                .on_layout(move |bounds: &Bounds<Points>| width.set(bounds.size.width.0))
                .child(composer_prompt_editor_tiptap(
                    editor,
                    ComposerPromptEditorProps {
                        disabled: Prop::bind(move || disabled.get()),
                        on_command_key_down: Prop::new(Some(ComposerCommandKeyCallback::new(
                            move |event, _| state.handle_key(event, None),
                        ))),
                        ..Default::default()
                    },
                    move |snapshot| {
                        source.set(snapshot.value);
                        state.observe();
                    },
                    |_| {},
                )),
        )
        .child(composer_command_menu_layer(
            state,
            anchor,
            computed(move || width.get()),
            "suggestions".into(),
        ))
        .child(text(move || format!("Active {:?}", state.active_item_id())))
        .child(text(move || format!("Requests {}", requests.get())))
        .child(text(move || {
            format!("Selected {} {:?}", selections.get(), selected.get())
        }))
        .child(text(move || format!("Source {:?}", source.get())))
        .child(text(move || {
            format!("Query {:?}", state.trigger.get().map(|t| t.query))
        }))
        .child(text(move || format!("Stale accepted {}", accepted.get())))
        .child(
            nsh_base::button()
                .on_press(move || publish(false))
                .child(text("Publish")),
        )
        .child(
            nsh_base::button()
                .on_press(move || publish(true))
                .child(text("Empty")),
        )
        .child(
            nsh_base::button()
                .on_press(move || {
                    state.set_scope("another-provider".into());
                    if let Some(request) = stale.get_untracked() {
                        accepted.set(state.publish(ComposerSuggestionResults {
                            request,
                            items: vec![],
                            loading: false,
                            empty_state: None,
                        }));
                    }
                })
                .child(text("Switch scope")),
        )
        .child(
            nsh_base::button()
                .on_press(move || {
                    disabled.update(|d| *d = !*d);
                })
                .child(text("Disable")),
        )
        .child(
            nsh_base::button()
                .on_press(move || {
                    literal.set_mode(EditorMode::Literal).expect("literal");
                    state.observe();
                })
                .child(text("Literal")),
        )
        .child(text("Suggestions ready"))
}

#[test]
fn file_skill_and_command_triggers_reach_the_same_drawer_and_native_key_path() {
    for marker in ["@", "$", "/"] {
        let mut app = start(PHONE, 3);
        open(&mut app, marker);
        app.press_key("enter").expect("choose first result");
        app.wait_for(&format!("Selected 1 \"result-0:{marker}\""))
            .expect("trigger selection goes to client operation callback");
        app.type_text("id:composer-editor", "more")
            .expect("continue querying");
        app.wait_for("Query Some(\"more\")")
            .expect("detector receives continued text");
        app.wait_for("Active None")
            .expect("old trigger query rows cannot be selected");
    }
}

fn start(window: (f32, f32), count: usize) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<Harness>(move || HarnessProps { count }))
    })
    .expect("headless suggestions");
    app.wait_for("Suggestions ready").expect("mount");
    app
}

fn open(app: &mut Headless, trigger: &str) {
    app.type_text("id:composer-editor", trigger)
        .expect("trigger from real native input");
    app.press("text:Publish").expect("source resolves query");
    app.wait_for("Active Some(\"result-0\")")
        .expect("first result active");
    // Publishing happens outside the editor; restore its native keyboard focus.
    app.type_text("id:composer-editor", "")
        .expect("focus editor");
}

#[test]
fn arrows_wrap_enter_and_tab_select_the_live_source_without_submitting() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, 3);
        open(&mut app, "@");
        assert!(
            app.screen().shows("Settings"),
            "content that is also a translation key stays literal"
        );
        app.press_key("down")
            .expect("advance from the initially active first row");
        app.wait_for("Active Some(\"result-1\")").expect("second");
        app.press_key("down").expect("third");
        app.wait_for("Active Some(\"result-2\")").expect("third");
        app.press_key("up").expect("second");
        app.press_key("up").expect("first");
        app.press_key("up").expect("wrap to last");
        app.wait_for("Active Some(\"result-2\")").expect("wrap");
        app.press_key("tab").expect("accept with Tab");
        app.wait_for("Selected 1 \"result-2:@\"")
            .expect("selection receives live source");
        app.press_key("enter").expect("accept with Enter");
        app.wait_for("Selected 2 \"result-2:@\"")
            .expect("menu handles Enter");
        assert!(app.screen().shows("Source \"@\""));
    }
}

#[test]
fn typing_requeries_escape_suppresses_the_token_until_it_is_left() {
    let mut app = start(DESKTOP, 3);
    open(&mut app, "@");
    app.type_text("id:composer-editor", "src")
        .expect("continue native typing");
    app.wait_for("Query Some(\"src\")")
        .expect("client detector sees updated query");
    app.wait_for("Active None")
        .expect("old query cannot be selected");
    app.press_key("escape").expect("dismiss");
    app.wait_for("Query None").expect("closed");
    app.type_text("id:composer-editor", "more")
        .expect("same token");
    assert!(app.screen().shows("Query None"));
    app.type_text("id:composer-editor", " @")
        .expect("new token");
    app.wait_for("Query Some(\"\")").expect("new trigger opens");
}

#[test]
fn empty_async_results_reset_highlight_and_reject_an_old_scope_response() {
    let mut app = start(DESKTOP, 3);
    open(&mut app, "@");
    app.press_key("up").expect("last result");
    app.wait_for("Active Some(\"result-2\")").expect("last");
    app.press("text:Empty")
        .expect("async source clears results");
    app.wait_for("Active None").expect("cleared");
    app.press("text:Publish").expect("restore results");
    app.wait_for("Active Some(\"result-0\")")
        .expect("first instead of stale highlight");
    app.press("text:Switch scope")
        .expect("another provider with same trigger text");
    app.wait_for("Stale accepted false")
        .expect("late response refused");
    app.wait_for("Active None")
        .expect("previous scope's items removed");
}

#[test]
fn selecting_a_row_preserves_the_native_caret_and_disabled_editor_never_dispatches() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, 3);
        open(&mut app, "@");
        app.press("role:option[Settings]")
            .expect("pointer or touch selects");
        app.wait_for("Selected 1 \"result-0:@\"")
            .expect("single dispatch");
        app.press_key("x").expect("continue typing at native caret");
        app.wait_for("Source \"@x\"")
            .expect("row has not stolen editor focus");
        app.press("text:Publish").expect("resolve changed query");
        app.press("text:Disable").expect("disable editor");
        app.press("role:option[Settings]")
            .expect("disabled row press is inert");
        assert!(app.screen().shows("Selected 1 \"result-0:@\""));
    }
}

#[test]
fn ime_candidate_keys_do_not_select_or_dismiss_and_literal_answers_hide_the_menu() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, 3);
        open(&mut app, "@");
        app.compose("id:composer-editor", "に").expect("preedit");
        for key in ["down", "enter", "tab", "escape"] {
            app.press_key(key).expect("IME candidate key");
        }
        assert!(app.screen().shows("Selected 0 \"\""));
        assert!(app.screen().shows("Query Some(\"\")"));
        let sink = app
            .screen()
            .layout_nodes()
            .into_iter()
            .find(|node| node.kind == nsh_core::ViewKind::Input)
            .expect("native input sink")
            .id;
        app.engine().control().dispatch_event(
            sink,
            nsh_core::EventKind::ChangeText,
            nsh_core::EventPayload::Text {
                // ChangeText carries the native sink's whole value, including the typed @.
                text: "@日本".into(),
                count: 0,
            },
        );
        assert!(app.settle());
        app.compose("id:composer-editor", "").expect("end preedit");
        app.wait_for("Source \"@日本\"")
            .expect("native composition committed once");
        app.wait_for("Query Some(\"日本\")")
            .expect("committed text requeries");
        app.wait_for("Active None")
            .expect("preedit results no longer selectable");
        app.press("text:Literal").expect("literal pending answer");
        app.wait_for("Query None")
            .expect("literal text has no suggestions");
    }
}

#[test]
fn highlighted_rows_scroll_into_the_constrained_drawer() {
    let mut app = start(PHONE, 35);
    open(&mut app, "@");
    app.press_key("up").expect("wrap directly to final row");
    app.wait_for("Active Some(\"result-34\")")
        .expect("final active row");
    let screen = app.screen();
    let list_node = screen.id("id:suggestions").expect("list id");
    let list = screen
        .layout_nodes()
        .into_iter()
        .find(|n| n.id == list_node)
        .expect("list");
    let row_id = t3_ui::screens::composer::suggestions::composer_suggestion_option_id(
        "suggestions",
        "result-34",
    );
    let row_node = screen.id(&format!("id:{row_id}")).expect("option id");
    let row = screen
        .layout_nodes()
        .into_iter()
        .find(|n| n.id == row_node)
        .expect("last option");
    assert!(row.bounds.origin.y.0 >= list.bounds.origin.y.0);
    assert!(
        row.bounds.origin.y.0 + row.bounds.size.height.0
            <= list.bounds.origin.y.0 + list.bounds.size.height.0 + 1.0
    );
    app.press_key("enter").expect("accept visible final row");
    app.wait_for("Selected 1 \"result-34:@\"")
        .expect("selected scrolled row");
}
