//! CMP-EDITOR-2: real editor key dispatch, source preservation and native IME.

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::thread_views::composer::policy::SendShortcut;
use t3_ui::screens::composer::{
    editor::{
        ComposerCommandKeyCallback, ComposerEditor, ComposerPromptEditorProps, ComposerResolvers,
        ComposerSubmitCallback, EditorMode, composer_prompt_editor_tiptap,
    },
    keys::{ComposerKeyProps, composer_key_plugin},
};

#[derive(Clone, PartialEq, Props)]
struct HarnessProps {
    initial: String,
    rich: bool,
    literal: bool,
}

#[component]
fn Harness(props: HarnessProps) -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let mode = if props.literal {
        EditorMode::Literal
    } else if props.rich {
        EditorMode::Rich
    } else {
        EditorMode::Plain
    };
    let editor = ComposerEditor::new(&props.initial, 0, mode, ComposerResolvers::default())
        .expect("composer");
    let shortcut = signal(SendShortcut::Enter);
    let software_keyboard = signal(false);
    let disabled = signal(false);
    let intercept = signal(false);
    let command_list = signal(false);
    let accept = signal(true);
    editor
        .set_plugins(vec![composer_key_plugin(
            &editor,
            ComposerKeyProps {
                send_shortcut: Prop::bind(move || shortcut.get()),
                software_keyboard_open: Prop::bind(move || software_keyboard.get()),
                disabled: Prop::bind(move || disabled.get()),
                on_command_key_down: Prop::new(Some(ComposerCommandKeyCallback::new(
                    move |event, in_list| {
                        if event.key != "Enter" || !intercept.get() {
                            return false;
                        }
                        command_list.set(in_list);
                        true
                    },
                ))),
            },
        )])
        .expect("key plugin");
    let snapshot = signal(editor.read_snapshot());
    let submissions = signal(0usize);
    let submitted = signal(String::new());
    let focus = editor.clone();
    let undo = editor.clone();
    let redo = editor.clone();
    view()
        .w_full()
        .child(composer_prompt_editor_tiptap(
            editor,
            ComposerPromptEditorProps {
                disabled: Prop::bind(move || disabled.get()),
                on_submit: Prop::new(Some(ComposerSubmitCallback::new(move |value| {
                    if !accept.get() {
                        return false;
                    }
                    submissions.update(|n| *n += 1);
                    submitted.set(value.value);
                    true
                }))),
                ..Default::default()
            },
            move |next| snapshot.set(next),
            |_| {},
        ))
        .child(text(move || format!("Source {:?}", snapshot.get().value)))
        .child(text(move || {
            format!("Sent {} {:?}", submissions.get(), submitted.get())
        }))
        .child(
            nsh_base::button()
                .on_press(move || accept.set(false))
                .child(text("Refuse send")),
        )
        .child(text(move || format!("Command list {}", command_list.get())))
        .child(
            nsh_base::button()
                .on_press(move || intercept.set(true))
                .child(text("Intercept Enter")),
        )
        .child(
            nsh_base::button()
                .on_press(move || focus.focus_at_end().expect("focus"))
                .child(text("Focus end")),
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
        .child(
            nsh_base::button()
                .on_press(move || shortcut.set(SendShortcut::ModEnter))
                .child(text("Require modifier")),
        )
        .child(
            nsh_base::button()
                .on_press(move || shortcut.set(SendShortcut::ModEnterMultiline))
                .child(text("Modifier for multiline")),
        )
        .child(
            nsh_base::button()
                .on_press(move || software_keyboard.set(true))
                .child(text("Software keyboard")),
        )
        .child(
            nsh_base::button()
                .on_press(move || disabled.update(|v| *v = !*v))
                .child(text("Read only")),
        )
        .child(text("Keys ready"))
}

fn start(window: (f32, f32), initial: &str, mode: EditorMode) -> Headless {
    let initial = initial.to_owned();
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<Harness>(move || HarnessProps {
            initial: initial.clone(),
            rich: mode == EditorMode::Rich,
            literal: mode == EditorMode::Literal,
        }))
    })
    .expect("headless editor");
    app.wait_for("Keys ready").expect("mounted");
    app.press("text:Focus end").expect("focus");
    app
}

fn source(app: &mut Headless, expected: &str) {
    app.wait_for(&format!("Source {expected:?}"))
        .expect("source");
}

#[test]
fn enter_submits_the_current_snapshot_and_shift_enter_inserts_a_newline() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, "draft", EditorMode::Rich);
        app.press_key("enter").expect("send key");
        app.wait_for("Sent 1 \"draft\"")
            .expect("Enter invokes submit");
        source(&mut app, "draft");
        app.press_key("shift+enter").expect("newline");
        source(&mut app, "draft\n");
        app.press_key("x").expect("type");
        app.press_key("enter").expect("send latest");
        app.wait_for("Sent 2 \"draft\\nx\"")
            .expect("latest editor snapshot");
    }
}

#[test]
fn modifier_preferences_and_software_keyboard_are_read_at_each_keypress() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, "draft", EditorMode::Plain);
        app.press("text:Require modifier").expect("preference");
        app.press("text:Focus end").expect("focus");
        app.press_key("enter").expect("newline");
        source(&mut app, "draft\n");
        assert!(app.screen().shows("Sent 0"));
        app.press_key("ctrl+enter").expect("modifier send");
        app.wait_for("Sent 1 \"draft\\n\"").expect("send");

        let mut app = start(window, "draft", EditorMode::Plain);
        app.press("text:Modifier for multiline")
            .expect("preference");
        app.press("text:Focus end").expect("focus");
        app.press_key("enter").expect("single line send");
        app.wait_for("Sent 1 \"draft\"").expect("send single line");
        app.press_key("shift+enter").expect("new line");
        app.press_key("enter").expect("multiline requires modifier");
        source(&mut app, "draft\n\n");
        assert!(app.screen().shows("Sent 1"));

        let mut app = start(window, "draft", EditorMode::Plain);
        app.press("text:Software keyboard")
            .expect("software keyboard visible");
        app.press("text:Focus end").expect("focus");
        app.press_key("enter").expect("software newline");
        source(&mut app, "draft\n");
        assert!(app.screen().shows("Sent 0"));
        app.press_key("ctrl+enter").expect("hardware modifier send");
        app.wait_for("Sent 1 \"draft\\n\"")
            .expect("modifier remains available");
    }
}

#[test]
fn list_enter_and_tab_preserve_source_and_undo_as_one_edit() {
    for window in [DESKTOP, PHONE] {
        for (value, mode, expected) in [
            ("1. foo", EditorMode::Plain, "1. foo\n2. "),
            ("- [x] done", EditorMode::Rich, "- [x] done\n- [ ] "),
            ("- **word**", EditorMode::Rich, "- **word**\n- "),
            ("- 😀 @README.md", EditorMode::Plain, "- 😀 @README.md\n- "),
        ] {
            let mut app = start(window, value, mode);
            app.press_key("shift+enter").expect("list split");
            source(&mut app, expected);
            assert!(app.screen().shows("Sent 0"));
            app.press("text:Undo").expect("undo split");
            source(&mut app, value);
            app.press("text:Redo").expect("redo split");
            source(&mut app, expected);
        }
        for mode in [EditorMode::Plain, EditorMode::Rich] {
            let mut app = start(window, "1. parent\n2. child", mode);
            app.press_key("tab").expect("nest");
            source(&mut app, "1. parent\n   1. child");
            app.press_key("shift+tab").expect("outdent");
            source(&mut app, "1. parent\n2. child");
        }
    }
}

#[test]
fn code_enter_indents_exits_and_modifier_enter_sends() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, "```ts\n  body()\n```", EditorMode::Rich);
        app.press_key("enter").expect("code newline");
        source(&mut app, "```ts\n  body()\n  \n```");
        assert!(app.screen().shows("Sent 0"));
        app.press_key("tab").expect("indent code");
        source(&mut app, "```ts\n  body()\n    \n```");
        app.press_key("shift+tab").expect("outdent code");
        source(&mut app, "```ts\n  body()\n  \n```");
        app.press_key("ctrl+enter").expect("send code");
        app.wait_for("Sent 1")
            .expect("modifier bypasses code newline");

        let mut app = start(window, "```\nx\n```", EditorMode::Rich);
        for _ in 0..3 {
            app.press_key("enter").expect("trailing blanks and exit");
        }
        source(&mut app, "```\nx\n```\n");
        app.press_key("z").expect("outside fence");
        source(&mut app, "```\nx\n```\nz");
        assert!(app.screen().shows("Sent 0"));
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
    assert!(app.settle());
}

#[test]
fn ime_confirmation_never_submits_and_read_only_keys_preserve_the_draft() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, "", EditorMode::Rich);
        app.compose("id:composer-editor", "に")
            .expect("begin preedit");
        ime_text(&mut app, "日本");
        app.press_key("enter").expect("IME confirm key");
        source(&mut app, "");
        assert!(app.screen().shows("Sent 0"));
        app.compose("id:composer-editor", "").expect("commit");
        source(&mut app, "日本");
        assert!(app.screen().shows("Sent 0"));
        app.press_key("enter")
            .expect("deliberate send after commit");
        app.wait_for("Sent 1 \"日本\"").expect("committed snapshot");
        app.press("text:Read only").expect("disable editor");
        app.press("text:Focus end").expect("focus");
        app.press_key("enter").expect("disabled submit");
        app.press_key("shift+enter").expect("disabled split");
        source(&mut app, "日本");
        assert!(app.screen().shows("Sent 1"));
    }
}

#[test]
fn commands_precede_send_and_code_precedes_the_command_callback() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, "- item", EditorMode::Rich);
        app.press("text:Intercept Enter")
            .expect("open command consumer");
        app.press("text:Focus end").expect("focus");
        app.press_key("enter").expect("consume command");
        app.wait_for("Command list true")
            .expect("list context forwarded");
        source(&mut app, "- item");
        assert!(app.screen().shows("Sent 0"));

        let mut app = start(window, "```\nx\n```", EditorMode::Rich);
        app.press("text:Intercept Enter")
            .expect("open command consumer");
        app.press("text:Focus end").expect("focus");
        app.press_key("enter").expect("code has priority");
        source(&mut app, "```\nx\n\n```");
        assert!(app.screen().shows("Sent 0"));
    }
}

#[test]
fn a_refused_send_keeps_the_draft_and_literal_answers_keep_their_markers() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, "unsent", EditorMode::Plain);
        app.press("text:Refuse send").expect("send refuses");
        app.press("text:Focus end").expect("focus");
        app.press_key("enter").expect("attempt submit");
        source(&mut app, "unsent");
        assert!(app.screen().shows("Sent 0"));
        app.press_key("shift+enter").expect("newline still works");
        source(&mut app, "unsent\n");

        let mut app = start(window, "- **literal** @README.md", EditorMode::Literal);
        app.press_key("shift+enter").expect("literal newline");
        source(&mut app, "- **literal** @README.md\n");
        assert!(app.screen().shows("Sent 0"));
    }
}
