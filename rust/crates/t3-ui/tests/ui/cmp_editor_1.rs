//! CMP-EDITOR-1: shared editor input, controlled state and native composition.

use crate::support::{DESKTOP, PHONE};
use nsh_base::rich_editor::view::SessionEventKind;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use std::sync::{Arc, Mutex};
use t3_ui::screens::composer::editor::{
    ComposerCommandKeyCallback, ComposerEditor, ComposerPromptEditorProps, ComposerResolvers,
    ComposerSubmitCallback, EditorMode, composer_prompt_editor_tiptap,
};

#[derive(Clone, PartialEq, Props)]
struct HarnessProps {
    initial: String,
    literal_labels: bool,
}

#[component]
fn Harness(props: HarnessProps) -> impl IntoElement {
    t3_ui::theme::install(&t3_ui::theme::ThemeChoice::default());
    if props.literal_labels {
        t3_ui::i18n::set_language("fr", &[]);
    }
    let display_label = signal(SharedString::from("Settings"));
    let editor = ComposerEditor::new(
        &props.initial,
        0,
        EditorMode::Rich,
        ComposerResolvers::default(),
    )
    .expect("composer session");
    let capture = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let recorded = capture.clone();
    let session = editor.session();
    let listener = session.on(
        SessionEventKind::Transaction,
        Arc::new(move |event| {
            recorded.lock().expect("events").push(event.payload.clone());
        }),
    );
    let listening = session.clone();
    cleanup(move || listening.off(listener));
    let snapshot = signal(editor.read_snapshot());
    let disabled = signal(false);
    let plain = editor.clone();
    let literal = editor.clone();
    let reset = editor.clone();
    let undo = editor.clone();
    let redo = editor.clone();
    let files = signal(0usize);
    let changes = signal(0usize);
    let focus_end = editor.clone();
    let focus_start = editor.clone();
    let compositions = signal(0usize);
    let context_count = signal(0usize);
    let custom_style = signal(false);
    let submitting = editor.clone();
    let clearing = editor.clone();
    let submitted = signal(String::new());
    let submissions = signal(0usize);
    let commands = signal(None::<bool>);
    view()
        .w_full()
        .p(px(16.0))
        .child(composer_prompt_editor_tiptap(
            editor,
            ComposerPromptEditorProps {
                disabled: Prop::bind(move || disabled.get()),
                placeholder: Prop::bind(move || {
                    if props.literal_labels {
                        display_label.get()
                    } else {
                        SharedString::from("Ask anything. @ files, $ skills, / commands")
                    }
                }),
                placeholder_suffix: Prop::new(SharedString::from(if props.literal_labels {
                    ""
                } else {
                    " ⇥ Tab"
                })),
                context_count: Prop::bind(move || context_count.get()),
                class_name: Prop::bind(move || {
                    SharedString::from(if custom_style.get() {
                        "min-h-[96px]"
                    } else {
                        ""
                    })
                }),
                placeholder_class_name: Prop::bind(move || {
                    SharedString::from(if custom_style.get() { "text-lg" } else { "" })
                }),
                aria_label: Prop::bind(move || {
                    if props.literal_labels {
                        display_label.get()
                    } else {
                        SharedString::from("Message")
                    }
                }),
                on_submit: Prop::new(Some(ComposerSubmitCallback::new(
                    move |next: t3_ui::screens::composer::editor::ComposerSnapshot| {
                        submitted.set(next.value);
                        submissions.update(|count| *count += 1);
                        true
                    },
                ))),
                on_command_key_down: Prop::new(Some(ComposerCommandKeyCallback::new(
                    move |event: &nsh_base::rich_editor::state::KeyEvent, in_list| {
                        if event.key.eq_ignore_ascii_case("F2") {
                            commands.set(Some(in_list));
                            true
                        } else {
                            false
                        }
                    },
                ))),
                ..Default::default()
            },
            move |next| {
                snapshot.set(next);
                changes.update(|count| *count += 1);
            },
            move |paste| files.set(paste.files.len() + paste.uris.len()),
        ))
        .child(text(move || format!("Source {:?}", snapshot.get().value)))
        .child(text(move || format!("Files {}", files.get())))
        .child(text(move || {
            format!("Submissions {} {:?}", submissions.get(), submitted.get())
        }))
        .child(text(move || format!("Command list {:?}", commands.get())))
        .child(text(move || format!("Changes {}", changes.get())))
        .child(text(move || {
            format!("Composition commits {}", compositions.get())
        }))
        .child(
            nsh_base::button()
                .on_press(move || {
                    submitting.submit();
                })
                .child(text("Submit")),
        )
        .child(
            nsh_base::button()
                .on_press(move || {
                    clearing.reset_after_send().expect("accepted send clear");
                    snapshot.set(clearing.read_snapshot());
                })
                .child(text("Reset after send")),
        )
        .child(
            nsh_base::button()
                .on_press(move || display_label.set(t3_ui::i18n::ts("Settings")))
                .child(text("Translate label")),
        )
        .child(
            nsh_base::button()
                .on_press(move || context_count.update(|count| *count = 1 - *count))
                .child(text("Toggle context")),
        )
        .child(
            nsh_base::button()
                .on_press(move || custom_style.update(|custom| *custom = !*custom))
                .child(text("Toggle classes")),
        )
        .child(
            nsh_base::button()
                .on_press(move || focus_start.focus_at(2).expect("focus offset"))
                .child(text("Focus offset")),
        )
        .child(
            nsh_base::button()
                .on_press(move || {
                    compositions.set(
                        capture
                            .lock()
                            .expect("events")
                            .iter()
                            .filter(|event| {
                                event["meta"]
                                    .as_array()
                                    .is_some_and(|meta| meta.iter().any(|key| key == "composition"))
                            })
                            .count(),
                    )
                })
                .child(text("Inspect composition")),
        )
        .child(
            nsh_base::button()
                .on_press(move || {
                    focus_end.focus_at_end().expect("focus end");
                })
                .child(text("Focus end")),
        )
        .child(
            nsh_base::button()
                .on_press(move || disabled.update(|value| *value = !*value))
                .child(text("Read only")),
        )
        .child(
            nsh_base::button()
                .on_press(move || {
                    plain.set_mode(EditorMode::Plain).expect("plain mode");
                })
                .child(text("Plain")),
        )
        .child(
            nsh_base::button()
                .on_press(move || {
                    literal.set_mode(EditorMode::Literal).expect("literal mode");
                })
                .child(text("Literal")),
        )
        .child(
            nsh_base::button()
                .on_press(move || {
                    reset.sync("Draft B", 7).expect("replace draft");
                    snapshot.set(reset.read_snapshot());
                })
                .child(text("Replace")),
        )
        .child(
            nsh_base::button()
                .on_press(move || {
                    undo.undo().expect("undo");
                })
                .child(text("Undo")),
        )
        .child(
            nsh_base::button()
                .on_press(move || {
                    redo.redo().expect("redo");
                })
                .child(text("Redo")),
        )
        .child(text("Editor ready"))
}

#[test]
fn controlled_rewrites_update_the_view_without_echoing_a_user_edit() {
    let mut app = start_editor(DESKTOP, "");
    app.type_text("id:composer-editor", "hello").expect("type");
    source(&mut app, "hello");
    app.wait_for("Changes 1").expect("one user edit");
    app.press("text:Replace").expect("controlled replacement");
    source(&mut app, "Draft B");
    assert!(app.screen().shows("Changes 1"));
}

fn start_editor(window: (f32, f32), initial: &str) -> Headless {
    start_with_labels(window, initial, false)
}

fn start_with_labels(window: (f32, f32), initial: &str, literal_labels: bool) -> Headless {
    let initial = initial.to_owned();
    // The application host is another package's file; this mounts the exact shared component.
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<Harness>(move || HarnessProps {
            initial: initial.clone(),
            literal_labels,
        }))
    })
    .expect("headless composer");
    app.wait_for("Editor ready").expect("mounted editor");
    app
}

fn source(app: &mut Headless, expected: &str) {
    app.wait_for(&format!("Source {expected:?}"))
        .expect("lossless source");
}

#[test]
fn real_paste_preserves_inline_code_and_crlf_at_both_widths() {
    for window in [DESKTOP, PHONE] {
        for value in ["see `**x**` now", "x\r\n\r\ny"] {
            let mut app = start_editor(window, "");
            app.type_text("id:composer-editor", "").expect("focus");
            let mut paste = nsh_core::PasteEvent::default();
            paste.text = value.into();
            app.paste("id:composer-editor", &paste).expect("paste");
            let expected = value.replace("\r\n", "\n");
            source(&mut app, &expected);
            if value.contains('`') {
                let style = app
                    .screen()
                    .span_style("id:composer-editor >> text:**x**")
                    .expect("inline code typography");
                let parent = app
                    .screen()
                    .span_style("id:composer-editor >> text:see")
                    .expect("prompt typography");
                assert!(
                    (style.font_size.0 - parent.font_size.0 * 0.92).abs() < 0.001,
                    "inline code font: {} / {} at {window:?}",
                    style.font_size.0,
                    parent.font_size.0
                );
            }
            app.press("text:Undo").expect("undo paste");
            source(&mut app, "");
            app.press("text:Redo").expect("redo paste");
            source(&mut app, &expected);
        }
    }
}

#[test]
fn real_paste_preserves_url_and_path_underscores() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_editor(window, "");
        app.type_text("id:composer-editor", "").expect("focus");
        let value = "Read https://example.com/_handle_/status/1 and open src/__test__.tsx";
        let mut paste = nsh_core::PasteEvent::default();
        paste.text = value.into();
        app.paste("id:composer-editor", &paste).expect("paste");
        source(&mut app, value);
    }
}

#[test]
fn typing_read_only_and_controlled_replacement_preserve_the_draft() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_editor(window, "");
        app.type_text("id:composer-editor", "hello").expect("type");
        source(&mut app, "hello");
        app.press("text:Read only").expect("disable edits");
        app.press("text:Undo").expect("disabled history");
        source(&mut app, "hello");
        app.press("text:Read only").expect("enable edits");
        app.press("text:Undo").expect("undo typing");
        source(&mut app, "");
        app.press("text:Redo").expect("redo typing");
        source(&mut app, "hello");
        app.press("text:Replace").expect("new controlled draft");
        source(&mut app, "Draft B");
        app.press("text:Undo")
            .expect("same-draft controlled rewrite is undoable");
        source(&mut app, "hello");
    }
}

#[test]
fn switching_to_literal_mode_preserves_unknown_dollar_text_and_markdown() {
    for window in [DESKTOP, PHONE] {
        let value = "echo $HOME **keep** @README.md";
        let mut app = start_editor(window, value);
        app.press("text:Plain").expect("plain mode");
        source(&mut app, value);
        app.press("text:Literal").expect("literal answer");
        source(&mut app, value);
        app.type_text("id:composer-editor", "X")
            .expect("edit literal source");
        source(&mut app, &format!("X{value}"));
    }
}

fn ime_text(app: &mut Headless, value: &str) {
    let id = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.kind == nsh_core::ViewKind::Input)
        .expect("native IME sink")
        .id;
    app.engine().control().dispatch_event(
        id,
        nsh_core::EventKind::ChangeText,
        nsh_core::EventPayload::Text {
            text: value.into(),
            count: 0,
        },
    );
    assert!(app.settle(), "platform input reaches the editor");
}

#[test]
fn platform_ime_preedit_commit_and_cancel_preserve_text_and_history() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_editor(window, "");
        app.compose("id:composer-editor", "に")
            .expect("first preedit");
        ime_text(&mut app, "に");
        source(&mut app, "");
        app.compose("id:composer-editor", "にほ")
            .expect("updated preedit");
        ime_text(&mut app, "日本");
        source(&mut app, "");
        app.compose("id:composer-editor", "").expect("commit");
        source(&mut app, "日本");
        app.press("text:Inspect composition")
            .expect("inspect metadata");
        app.wait_for("Composition commits 1")
            .expect("IME metadata retained");
        app.compose("id:composer-editor", "取消")
            .expect("new preedit");
        // Cancellation restores the input's last committed value, removing only marked text.
        ime_text(&mut app, "日本");
        app.compose("id:composer-editor", "").expect("cancel");
        source(&mut app, "日本");
        app.press("text:Undo").expect("undo one composition");
        source(&mut app, "");
        app.press("text:Redo").expect("redo one composition");
        source(&mut app, "日本");
    }
}

#[test]
fn committing_an_ime_delimiter_replaces_selected_text_without_surrounding_it() {
    for window in [DESKTOP, PHONE] {
        for (plain, delimiter) in [false, true]
            .into_iter()
            .flat_map(|plain| ["\"", "`", "*"].map(|delimiter| (plain, delimiter)))
        {
            let mut app = start_editor(window, "word");
            if plain {
                app.press("text:Plain").expect("plain mode");
            }
            app.type_text("id:composer-editor", "").expect("focus");
            app.press_key("home").expect("start");
            app.press_key("shift+end").expect("select word");
            app.compose("id:composer-editor", "·")
                .expect("composition starts");
            ime_text(&mut app, delimiter);
            app.compose("id:composer-editor", "").expect("commit quote");
            source(&mut app, delimiter);
            app.press("text:Undo").expect("undo commit");
            source(&mut app, "word");
        }
    }
}

#[test]
fn pasting_into_a_fence_keeps_markdown_mentions_and_newlines_verbatim() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_editor(window, "```ts\na\n```");
        app.press("text:Focus end").expect("inside fence end");
        let mut paste = nsh_core::PasteEvent::default();
        paste.text = "\n**bold** @README.md\n- item\n$browser".into();
        app.paste("id:composer-editor", &paste).expect("paste code");
        source(
            &mut app,
            "```ts\na\n**bold** @README.md\n- item\n$browser\n```",
        );
        app.press("text:Undo").expect("undo paste");
        source(&mut app, "```ts\na\n```");
    }
}

// The native textblock's zero-width caret anchor precedes its placeholder part.
const PLACEHOLDER: &str = "\u{200b}Ask anything. @ files, $ skills, / commands ⇥ Tab";

fn editor_height(app: &Headless) -> f32 {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some("composer-editor"))
        .expect("editor bounds")
        .bounds
        .size
        .height
        .0
}

#[test]
fn placeholder_tracks_content_context_preedit_and_caller_classes() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_editor(window, "");
        app.wait_for(PLACEHOLDER).expect("placeholder and key hint");
        app.press("text:Toggle context").expect("context added");
        assert!(!app.screen().shows(PLACEHOLDER));
        app.press("text:Toggle context").expect("context removed");
        app.wait_for(PLACEHOLDER).expect("placeholder returns");
        let normal_height = editor_height(&app);
        app.press("text:Toggle classes").expect("caller styles");
        assert!(editor_height(&app) > normal_height);
        assert!(editor_height(&app) >= 96.0);
        let placeholder_style = app
            .screen()
            .span_style(&format!("text:{}", &PLACEHOLDER['\u{200b}'.len_utf8()..]))
            .expect("placeholder style");
        assert_eq!(placeholder_style.font_size.0, 18.0);
        app.compose("id:composer-editor", "に").expect("preedit");
        assert!(!app.screen().shows(PLACEHOLDER));
        ime_text(&mut app, "");
        app.compose("id:composer-editor", "")
            .expect("cancel preedit");
        app.wait_for(PLACEHOLDER)
            .expect("cancel restores placeholder");
        app.type_text("id:composer-editor", "x").expect("type");
        assert!(!app.screen().shows(PLACEHOLDER));
        app.press("text:Undo").expect("empty document again");
        app.wait_for(PLACEHOLDER)
            .expect("undo restores placeholder");
    }
}

#[test]
fn imperative_focus_places_the_caret_and_autosize_grows_caps_and_shrinks() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_editor(window, "abcd");
        app.press("text:Focus offset").expect("focus source offset");
        app.press_key("x").expect("type at focused caret");
        source(&mut app, "abxcd");
        app.press("text:Focus end").expect("focus end");
        app.press_key("z").expect("type at end");
        source(&mut app, "abxcdz");
        let short_height = editor_height(&app);
        let mut paste = nsh_core::PasteEvent::default();
        paste.text = "\nline\nline\nline".into();
        app.paste("id:composer-editor", &paste).expect("grow");
        let grown = editor_height(&app);
        assert!(grown > short_height, "{short_height} -> {grown}");
        paste.text = "\nline".repeat(30);
        app.paste("id:composer-editor", &paste).expect("cap");
        let capped = editor_height(&app);
        assert!(capped >= grown && capped <= 208.0, "{grown} -> {capped}");
        assert!(app.screen().layout_nodes().iter().any(|node| {
            node.scroll
                .as_ref()
                .and_then(|scroll| scroll.content_size)
                .is_some_and(|size| size.height.0 > capped)
        }));
        app.press("text:Replace").expect("short controlled draft");
        source(&mut app, "Draft B");
        assert!(editor_height(&app) < capped);
        assert_eq!(editor_height(&app), short_height);
    }
}

#[test]
fn submit_uses_current_text_defers_during_ime_and_clear_retains_same_draft_undo() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_editor(window, "draft");
        app.type_text("id:composer-editor", "new ")
            .expect("edit draft");
        app.press("text:Submit").expect("submit hook");
        app.wait_for("Submissions 1 \"new draft\"")
            .expect("fresh source delivered");
        app.press("text:Read only").expect("disable");
        app.press("text:Submit").expect("disabled submit");
        assert!(app.screen().shows("Submissions 1 \"new draft\""));
        app.press("text:Read only").expect("enable");
        app.compose("id:composer-editor", "に").expect("preedit");
        app.press("text:Submit").expect("deferred submit");
        assert!(app.screen().shows("Submissions 1 \"new draft\""));
        ime_text(&mut app, "new ");
        app.compose("id:composer-editor", "")
            .expect("cancel composition");
        app.press("text:Reset after send").expect("accepted send");
        source(&mut app, "");
        app.wait_for(PLACEHOLDER).expect("empty composer");
        app.press("text:Undo").expect("same draft undo");
        source(&mut app, "new draft");
        app.press("text:Redo").expect("redo clear");
        source(&mut app, "");
    }
}

#[test]
fn native_command_keys_forward_list_context_without_a_fixture_key_plugin() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_editor(window, "- item");
        app.type_text("id:composer-editor", "").expect("focus");
        app.press_key("F2").expect("native command");
        app.wait_for("Command list Some(true)")
            .expect("web command context");
        source(&mut app, "- item");
    }
}

#[test]
fn runtime_and_caller_translated_labels_are_displayed_without_translation_lookup() {
    let mut app = start_with_labels(PHONE, "", true);
    app.wait_for("\u{200b}Settings")
        .expect("runtime content remains literal in French");
    assert!(
        app.screen()
            .matches("label:Settings")
            .expect("literal label")
    );
    app.press("text:Translate label")
        .expect("caller translates its key");
    let translated = t3_i18n::Translator::new(t3_i18n::Surface::Unified, "fr").t("Settings");
    assert_ne!(translated, "Settings");
    app.wait_for(&format!("\u{200b}{translated}"))
        .expect("final translated text");
    assert!(
        app.screen()
            .matches(&format!("label:{translated}"))
            .expect("final label")
    );
}
