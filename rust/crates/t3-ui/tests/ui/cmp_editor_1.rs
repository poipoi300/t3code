//! CMP-EDITOR-1: exercise the editor foundation before CMP-PLAIN mounts it in the thread frame.

use crate::support::{DESKTOP, PHONE};
use nsh_base::rich_editor::view::SessionEventKind;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use std::sync::{Arc, Mutex};
use t3_ui::screens::composer::editor::{
    ComposerEditor, ComposerPromptEditorProps, ComposerResolvers, EditorMode,
    composer_prompt_editor_tiptap,
};

#[derive(Clone, PartialEq, Props)]
struct HarnessProps {
    initial: String,
}

#[component]
fn Harness(props: HarnessProps) -> impl IntoElement {
    t3_ui::theme::install(&t3_ui::theme::ThemeChoice::default());
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
            recorded
                .lock()
                .expect("events")
                .push(event.payload.clone());
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
    let compositions = signal(0usize);
    view()
        .w_full()
        .p(px(16.0))
        .child(composer_prompt_editor_tiptap(
            editor,
            ComposerPromptEditorProps {
                disabled: Prop::bind(move || disabled.get()),
                placeholder: Prop::new(SharedString::from(
                    "Ask anything. @ files, $ skills, / commands",
                )),
                aria_label: Prop::new(SharedString::from("Message")),
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
        .child(text(move || format!("Changes {}", changes.get())))
        .child(text(move || format!("Composition commits {}", compositions.get())))
        .child(
            nsh_base::button()
                .on_press(move || compositions.set(capture.lock().expect("events").iter().filter(|event| event["meta"].as_array().is_some_and(|meta| meta.iter().any(|key| key == "composition"))).count()))
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
    let initial = initial.to_owned();
    // The application host is another package's file; this mounts the exact shared component.
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<Harness>(move || HarnessProps {
            initial: initial.clone(),
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
        app.press("text:Undo").expect("old draft history is gone");
        source(&mut app, "Draft B");
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
        app.press("text:Inspect composition").expect("inspect metadata");
        app.wait_for("Composition commits 1").expect("IME metadata retained");
        app.compose("id:composer-editor", "取消")
            .expect("new preedit");
        ime_text(&mut app, "");
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
        for plain in [false, true] {
            let mut app = start_editor(window, "word");
            if plain {
                app.press("text:Plain").expect("plain mode");
            }
            app.type_text("id:composer-editor", "").expect("focus");
            app.press_key("home").expect("start");
            app.press_key("shift+end").expect("select word");
            app.compose("id:composer-editor", "·")
                .expect("composition starts");
            ime_text(&mut app, "\"");
            app.compose("id:composer-editor", "").expect("commit quote");
            source(&mut app, "\"");
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
