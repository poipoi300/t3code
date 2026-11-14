//! CMP-ATTACH: real client intake and shared attachment elements in an isolated composer host.

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use std::{cell::RefCell, rc::Rc};
use t3_client::thread_views::composer::attachment_intake::AttachmentSession;
use t3_client::{
    platform::{HostReply, HostResource},
    testing::FakePlatform,
    types::EnvironmentId,
};
use t3_ui::screens::composer::{
    attachments::{ComposerAttachments, composer_attach_button, composer_attachment_strip},
    editor::{
        ComposerEditor, ComposerPromptEditorProps, ComposerResolvers, EditorMode,
        composer_prompt_editor_tiptap,
    },
};

#[component]
fn Harness() -> impl IntoElement {
    t3_ui::theme::install(&t3_ui::theme::ThemeChoice::default());
    let platform = FakePlatform::default();
    platform.push_host(Ok(HostReply::Resources(vec![HostResource {
        handle: "picked".into(),
        name: "notes.txt".into(),
        mime: "text/plain".into(),
    }])));
    platform.push_host(Ok(HostReply::Bytes(b"hello".to_vec())));
    let client = AttachmentSession::new(
        Rc::new(platform),
        Rc::new(RefCell::new(json!({"draftsByThreadKey":{}}))),
        "env:thread".into(),
        EnvironmentId("env".into()),
    );
    let disabled = signal(false);
    let state = ComposerAttachments::new(client, disabled);
    let prompt = ComposerEditor::new("", 0, EditorMode::Plain, ComposerResolvers::default())
        .expect("editor");
    let focus = prompt.clone();
    view()
        .p(px(16.0))
        .w_full()
        .child(composer_attachment_strip(state))
        .child(composer_prompt_editor_tiptap(
            prompt,
            ComposerPromptEditorProps::default(),
            |_| {},
            move |paste| state.paste_files(paste),
        ))
        .child(composer_attach_button(state, move || {
            focus.focus_at_end().expect("focus");
        }))
        .child(
            nsh_base::button()
                .on_press(move || disabled.update(|value| *value = !*value))
                .child(text("Toggle permission")),
        )
        .child(text(move || format!("Attachments {}", state.items().len())))
        .child(text(move || {
            state
                .error()
                .map(|error| error.resolve())
                .unwrap_or_default()
        }))
        .child(text(move || {
            state
                .block_reason()
                .map(|reason| reason.resolve())
                .unwrap_or_default()
        }))
        .child(t3_ui::ui::confirm::confirm_dialog_host())
}

fn start(window: (f32, f32)) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<Harness>())
    })
    .expect("composer");
    app.wait_for("Attachments 0").expect("mounted");
    app
}

#[test]
fn picker_stages_in_the_real_draft_and_remove_reverses_it_at_both_widths() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window);
        app.press("role:button[Attach files]").expect("picker");
        app.wait_for("notes.txt").expect("picked file");
        app.wait_for("Attachments 1").expect("staged");
        app.wait_for("Waiting for the server before file attachments can send")
            .expect("send blocked by unknown capability");
        app.press("role:button[Remove notes.txt]").expect("remove");
        app.wait_for("Attachments 0").expect("removed");
        assert!(!app.screen().shows("notes.txt"));
    }
}

#[test]
fn disabled_picker_cannot_consume_the_host_selection() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window);
        app.press("text:Toggle permission").expect("deny");
        let _ = app.press("role:button[Attach files]");
        assert!(app.screen().shows("Attachments 0"));
        app.press("text:Toggle permission").expect("allow");
        app.press("role:button[Attach files]")
            .expect("pick after enabling");
        app.wait_for("notes.txt")
            .expect("selection was not consumed while disabled");
    }
}

#[test]
fn native_editor_file_paste_reaches_intake_and_image_preview_and_removal() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window);
        app.type_text("id:composer-editor", "").expect("focus");
        let mut paste = nsh_core::PasteEvent::default();
        paste.files.push(nsh_core::PastedFile::new(
            "photo.png",
            "image/png",
            vec![
                137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0,
                1, 8, 4, 0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 252,
                255, 31, 0, 3, 3, 2, 0, 239, 154, 21, 139, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96,
                130,
            ],
        ));
        app.paste("id:composer-editor", &paste)
            .expect("native image paste");
        app.wait_for("Attachments 1").expect("pasted into draft");
        app.press("role:button[Preview photo.png]")
            .expect("open preview");
        app.press_key("escape").expect("close preview");
        app.press("role:button[Remove photo.png]")
            .expect("remove pasted image");
        app.wait_for("Attachments 0").expect("removed");
    }
}
