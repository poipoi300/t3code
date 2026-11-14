//! CMP-EDITOR-1: exercise the editor foundation before CMP-PLAIN mounts it in the thread frame.

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
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
    let snapshot = signal(editor.read_snapshot());
    let disabled = signal(false);
    let plain = editor.clone();
    let literal = editor.clone();
    let reset = editor.clone();
    let undo = editor.clone();
    let redo = editor.clone();
    let files = signal(0usize);
    let changes = signal(0usize);
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
