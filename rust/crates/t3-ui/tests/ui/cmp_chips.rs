//! CMP-CHIPS: real client-parsed atoms and the shared sent-reference renderer.

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use std::{collections::BTreeMap, rc::Rc, sync::Arc};
use t3_client::thread_views::composer::attachment_intake::{AttachmentItem, UploadState};
use t3_client::thread_views::user_message::context_references;
use t3_ui::screens::composer::{
    chips::{
        ChipContext, ChipData, composer_prompt_editor_with_chips,
        user_message_context_reference_chip,
    },
    editor::{ComposerEditor, ComposerPromptEditorProps, ComposerResolvers, EditorMode},
};

const SOURCE: &str = "@src/main.rs $browser [capture](t3-context://v1/terminal/term) [notes](t3-context://v1/file/file_local) /command ";

fn data() -> ChipData {
    let terminal = serde_json::from_value(json!({
        "version": 1, "kind": "terminal", "contextId": "term", "label": "capture",
        "terminalId": "terminal-1", "terminalLabel": "PowerShell", "lineStart": 3,
        "lineEnd": 4, "text": "first line\nsecond line"
    }))
    .expect("canonical terminal context");
    let skill = serde_json::from_value(json!({
        "name": "browser", "displayName": "Settings", "description": "Read @src/main.rs literally.",
        "path": "skills/browser/SKILL.md", "enabled": true
    }))
    .expect("provider skill");
    ChipData {
        records: BTreeMap::from([("term".into(), terminal)]),
        skills: vec![skill],
        draft_attachments: vec![AttachmentItem {
            id: "local".into(),
            name: "notes.txt".into(),
            mime: "text/plain".into(),
            size: 3,
            image: false,
            video: false,
            preview: None,
            needs_reattach: false,
            non_persisted: false,
            upload: UploadState::Failed("Server details".into()),
        }],
        ..Default::default()
    }
}

#[component]
fn ChipsFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let catalogue = signal(data());
    let opens = signal(String::new());
    let mut context = ChipContext::new(computed(move || catalogue.get()));
    context.open_mention = Some(Rc::new(move |path| opens.set(path.into())));
    context.open_attachment = Some(Rc::new(move |id| opens.set(format!("attachment:{id}"))));
    let editor = ComposerEditor::new(
        SOURCE,
        0,
        EditorMode::Rich,
        ComposerResolvers {
            skill: Arc::new(|name| {
                (name == "browser").then(|| {
                    (
                        "Old provider label".into(),
                        Some("Old provider description".into()),
                    )
                })
            }),
            ..Default::default()
        },
    )
    .expect("client document");
    let source = signal(editor.read_snapshot().value);
    let undo = editor.clone();
    let replacement = editor.clone();
    let sent = context_references("[capture](t3-context://v1/terminal/term)").remove(0);
    let mismatched = context_references("[mismatch](t3-context://v1/file/term)").remove(0);
    let record = data().records.remove("term").expect("terminal");
    view()
        .w_full()
        .h_full()
        .p(px(24.0))
        .child(composer_prompt_editor_with_chips(
            editor,
            ComposerPromptEditorProps::default(),
            context.clone(),
            move |snapshot| source.set(snapshot.value),
            |_| {},
        ))
        .child(text(move || format!("Source: {}", source.get())))
        .child(text(move || format!("Opened: {}", opens.get())))
        .child(
            nsh_base::button()
                .on_press(move || {
                    replacement
                        .sync("@docs/next.rs ", 0)
                        .expect("controlled replacement");
                })
                .child(text("Replace document")),
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
                    catalogue.update(|data| {
                        data.skills[0].display_name =
                            Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                                "New provider label".into(),
                            )));
                        data.skills[0].description = None;
                    })
                })
                .child(text("Change provider label")),
        )
        .child(
            nsh_base::button()
                .on_press(move || {
                    catalogue.update(|data| {
                        if let Some(
                            t3_contracts::composer_context::ComposerContextRecord::Terminal(record),
                        ) = data.records.get_mut("term")
                        {
                            record.text = "\r\n".into();
                        }
                    })
                })
                .child(text("Expire captured output")),
        )
        .child(
            nsh_base::button()
                .on_press(move || {
                    catalogue.update(|data| data.draft_attachments[0].needs_reattach = true)
                })
                .child(text("Require reattach")),
        )
        .child(
            view()
                .test_id("sent-chip")
                .child(user_message_context_reference_chip(
                    &sent,
                    Some(&record),
                    context.clone(),
                )),
        )
        .child(
            view()
                .test_id("mismatched-chip")
                .child(user_message_context_reference_chip(
                    &mismatched,
                    Some(&record),
                    context,
                )),
        )
        .child(
            nsh_base::button()
                .on_press(|| t3_ui::i18n::set_language("fr", &[]))
                .child(text("French")),
        )
}

fn start(window: (f32, f32)) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<ChipsFixture>())
    })
    .expect("chip fixture");
    app.first_render().expect("render");
    app
}

#[test]
fn client_atoms_render_without_changing_source_and_replaced_nodes_keep_current_actions() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window);
        app.wait_for("Settings")
            .expect("current provider label replaces stored label");
        app.press("role:button[Preview src/main.rs]")
            .expect("mention action");
        app.wait_for("Opened: src/main.rs").expect("path action");
        assert!(
            app.screen().shows(&format!("Source: {SOURCE}")),
            "display never rewrites source"
        );
        app.press("text:Replace document")
            .expect("controlled source change");
        app.press("role:button[Preview docs/next.rs]")
            .expect("updated node action");
        app.wait_for("Opened: docs/next.rs")
            .expect("action uses current attrs");
        assert!(
            !app.screen()
                .matches("role:button[Preview src/main.rs]")
                .expect("old mention gone")
        );
    }
}

#[test]
fn skill_details_use_live_catalogue_content_and_popovers_close_on_escape() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window);
        app.press("role:button[Skill Settings. Show details]")
            .expect("skill popover");
        app.wait_for("Read @src/main.rs literally.")
            .expect("literal description");
        app.press("role:button[View instructions]")
            .expect("instructions action");
        app.wait_for("Opened: skills/browser/SKILL.md")
            .expect("instruction path");
        app.press_key("escape").expect("dismiss");
        assert!(!app.screen().shows("Read @src/main.rs literally."));
        app.press("text:Change provider label")
            .expect("catalogue update");
        app.wait_for("New provider label")
            .expect("same atom reacts to catalogue");
        assert!(
            app.screen().shows("New provider label"),
            "provider labels are content"
        );
        app.press("role:button[Skill New provider label. Show details]")
            .expect("new skill details");
        app.wait_for("No description is available for this skill.")
            .expect("current provider has no description");
        assert!(!app.screen().shows("Old provider description"));
    }
}

#[test]
fn provider_content_does_not_become_a_translation_key_after_a_language_change() {
    let mut app = start(DESKTOP);
    app.press("text:French").expect("change language");
    assert!(
        app.screen().shows("Settings"),
        "a skill named Settings is content"
    );
    assert!(
        !app.screen().shows("Paramètres"),
        "the skill label must not become a translation key"
    );
}

#[test]
fn a_pending_attachment_renders_from_the_client_item_and_reattach_removes_its_action() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window);
        app.press("role:button[File attachment, notes.txt, 1 KB, upload failed]")
            .expect("failed local attachment remains previewable");
        app.wait_for("Opened: attachment:local")
            .expect("draft attachment identity");
        app.press("text:Require reattach")
            .expect("resource was lost");
        app.wait_for("label:File attachment, notes.txt, 1 KB, attach again")
            .expect("reattach takes precedence");
        assert!(
            !app.screen()
                .matches("role:button[File attachment, notes.txt, 1 KB, attach again]")
                .expect("no preview action without local or uploaded bytes")
        );
        assert!(app.screen().shows(&format!("Source: {SOURCE}")));
    }
}

#[test]
fn an_expired_draft_terminal_cannot_open_captured_output() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window);
        assert!(
            app.screen()
                .matches(
                    "id:composer-editor >> role:button[Terminal excerpt, capture. Show details]"
                )
                .expect("active draft excerpt")
        );
        app.press("text:Expire captured output")
            .expect("client context update");
        app.wait_for("label:Terminal excerpt, capture, expired")
            .expect("expired state");
        assert!(
            !app.screen()
                .matches(
                    "id:composer-editor >> role:button[Terminal excerpt, capture. Show details]"
                )
                .expect("expired excerpt has no details action")
        );
        assert!(!app.screen().shows("first line\nsecond line"));
    }
}

#[test]
fn sent_context_uses_the_shared_details_and_kind_mismatch_cannot_open_terminal_data() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window);
        app.press("id:sent-chip >> role:button[Terminal excerpt, capture. Show details]")
            .expect("sent terminal chip");
        app.wait_for("PowerShell").expect("terminal title");
        app.wait_for("Lines 3–4").expect("line range");
        app.wait_for("first line\nsecond line")
            .expect("captured output");
        app.press_key("escape").expect("dismiss");
        assert!(app.screen().shows("mismatch"));
        assert!(
            app.press(
                "id:mismatched-chip >> role:button[Terminal excerpt, mismatch. Show details]"
            )
            .is_err(),
            "wrong-kind record must use unavailable fallback"
        );
        assert!(!app.screen().shows("first line\nsecond line"));
    }
}
