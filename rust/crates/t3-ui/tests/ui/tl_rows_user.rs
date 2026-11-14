//! TL-ROWS-USER: the real row consumes the client's timeline projection.
use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{Headless, LayoutNode};
use serde_json::json;
use std::rc::Rc;
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRow, MessagesTimelineRowsInput, derive_messages_timeline_rows,
};
use t3_client::view_logic::session::{ChatMessage, TimelineContent, TimelineEntry};
use t3_ui::i18n::{Value as Argument, t, tr};
use t3_ui::screens::thread::rows::user::{self, UserRowContext, UserRowState};

fn projection(kind: &str) -> Option<Rc<MessagesTimelineRow>> {
    let mut message = json!({"id":kind,"role":"user","text":"","runId":null,"streaming":false,
        "createdAt":"2026-11-13T12:00:00.000Z","updatedAt":"2026-11-13T12:00:00.000Z"});
    let extra = match kind {
        "automation" => {
            json!({"scheduledTaskId":"task","text":"Daily audit\nRun the full daily audit."})
        }
        "agent" => {
            json!({"createdBy":"agent","creationSource":"mcp","text":"Agent summary\nHidden agent payload"})
        }
        "own" => json!({"text":"Long source line\n".repeat(30)}),
        "markdown" => json!({"text":"**Strong**\nSecond line"}),
        "file" => {
            json!({"attachments":[{"type":"file","id":"file","name":"notes.txt","mimeType":"text/plain","sizeBytes":12}]})
        }
        "guest" => {
            json!({"text":"Message from guest Sam, sent by the owner:\n\nWhy?","context":{"guest":{"guestMessageId":"guest-message:one","name":"Sam","text":"Why?"}}})
        }
        _ => json!({"createdBy":"agent","creationSource":"server","text":"Server restarted."}),
    };
    message
        .as_object_mut()
        .expect("message")
        .extend(extra.as_object().expect("fields").clone());
    let message: ChatMessage = serde_json::from_value(message).expect("chat message");
    let entry = TimelineEntry {
        id: kind.into(),
        created_at: message.created_at.clone(),
        attempt: None,
        content: TimelineContent::Message {
            message: Rc::new(message),
            projected_item: None,
        },
    };
    derive_messages_timeline_rows(&MessagesTimelineRowsInput {
        timeline_entries: vec![Rc::new(entry)], supports_conversation_rollback:true,
        thread_start_checkpoint_available:true, available_revert_turn_counts:Some([0].into()),
        ..Default::default()
    }).into_iter().find(|row| matches!(&row.content,t3_client::thread_views::messages_timeline::MessagesTimelineRowContent::Message{message,..} if message.role=="user"))
}

#[component]
fn UserRowFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    provide_context(t3_ui::theme::text_view_parts());
    let selected = signal("automation");
    let edits = signal(0);
    let state = UserRowState::new();
    let mut context = UserRowContext::new(state);
    context.revert = Some(Rc::new(move |row, count| {
        assert_eq!(count, 0);
        assert!(!row.id.is_empty());
        edits.update(|n| *n += 1);
    }));
    let working = signal(false);
    let reverting = signal(false);
    let find_active = signal(false);
    context.working = computed(move || working.get());
    context.reverting = computed(move || reverting.get());
    context.find_active = computed(move || find_active.get());
    let mut root = view().w_full().h_full();
    for (label, kind) in [
        ("Automation example", "automation"),
        ("Agent example", "agent"),
        ("Own example", "own"),
        ("Markdown example", "markdown"),
        ("File example", "file"),
        ("Guest example", "guest"),
        ("Server example", "server"),
    ] {
        root = root.child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(label)))
                .on_press(move || selected.set(kind))
                .child(text(label)),
        );
    }
    let content = root
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(t("Toggle working"))))
                .on_press(move || working.update(|value| *value = !*value))
                .child(text(t("Toggle working"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(t("Toggle reverting"))))
                .on_press(move || reverting.update(|value| *value = !*value))
                .child(text(t("Toggle reverting"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(t("Reveal for find"))))
                .on_press(move || {
                    find_active.set(true);
                    state.reveal(selected.get_untracked());
                })
                .child(text(t("Reveal for find"))),
        )
        .child(text(move || {
            SharedString::from(tr(
                "Edits: {count}",
                &[("count", Argument::Int(edits.get()))],
            ))
        }))
        .child(user::user_message_row(
            computed(move || projection(selected.get())),
            context,
        ));
    t3_ui::ui::toast::anchored_toast_provider(content)
}
fn fixture(window: (f32, f32)) -> Headless {
    let mut registries = t3_ui::registries();
    let mut fixtures = Registry::new();
    fixtures.register::<UserRowFixture>();
    registries.push(fixtures);
    Headless::start(EngineConfig {
        ui_sets: registries,
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<UserRowFixture>())
    })
    .expect("row fixture")
}
fn body_node(app: &Headless) -> LayoutNode {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some("data-user-message-body"))
        .expect("body layout")
}
#[test]
fn client_attribution_folds_are_independent_when_the_cell_is_reused() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        app.wait_for("Daily audit").expect("summary");
        assert!(!app.screen().shows(r"text:/Run the full daily audit\./"));
        app.press("role:button[Automation]").expect("expand");
        app.wait_for(r"text:/Run the full daily audit\./")
            .expect("automation body");
        app.press("role:button[Agent example]").expect("recycle");
        app.wait_for("Agent summary").expect("agent summary");
        assert!(!app.screen().shows("text:/Hidden agent payload/"));
        app.press("role:button[Agent]").expect("expand agent");
        app.wait_for("text:/Hidden agent payload/").expect("agent body");
        app.press("role:button[Automation example]").expect("reuse");
        app.wait_for(r"text:/Run the full daily audit\./")
            .expect("preserved fold");
        app.press("role:button[Automation]").expect("collapse");
        assert!(!app.screen().shows(r"text:/Run the full daily audit\./"));
        app.press("role:button[Agent example]")
            .expect("reuse agent");
        app.wait_for("text:/Hidden agent payload/")
            .expect("independent state");
    }
}
#[test]
fn client_projected_edit_guards_and_default_copy_work_without_renderers() {
    let mut app = fixture(PHONE);
    app.wait_for("Daily audit").expect("row");
    app.press("role:button[Automation]").expect("expand");
    assert!(
        app.screen()
            .enabled("role:button[Copy message]")
            .expect("default copy")
    );
    app.press("role:button[Copy message]").expect("copy");
    app.wait_for("Copied!").expect("anchored feedback");
    assert_eq!(
        app.screen().clipboard(),
        Some("Daily audit\nRun the full daily audit.")
    );
    assert!(
        !app.screen()
            .enabled("role:button[Copy message]")
            .expect("copied temporarily disabled")
    );
    for toggle in [
        "role:button[Toggle working]",
        "role:button[Toggle reverting]",
    ] {
        app.press(toggle).expect("busy");
        assert!(
            !app.screen()
                .enabled("role:button[Edit from here]")
                .expect("edit")
        );
        let _ = app.press("role:button[Edit from here]");
        assert!(app.screen().shows("Edits: 0"));
        app.press(toggle).expect("idle");
    }
    app.press("role:button[Edit from here]").expect("edit");
    app.wait_for("Edits: 1").expect("one action");
}
#[test]
fn client_file_guest_and_markdown_content_render_without_injected_presentations() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        app.wait_for("Daily audit").expect("row");
        app.press("role:button[File example]").expect("file");
        app.wait_for("TXT · 12 B").expect("file card");
        assert!(app.screen().shows("notes.txt"));
        app.press("role:button[Guest example]").expect("guest");
        app.wait_for("Why?").expect("guest words");
        assert!(app.screen().shows("Sam"));
        assert!(
            !app.screen()
                .shows("Message from guest Sam, sent by the owner:")
        );
        app.press("role:button[Markdown example]")
            .expect("markdown");
        app.wait_for(r"text:/^Strong\nSecond line$/")
            .expect("parsed strong");
        assert!(!app.screen().shows(r"text:/\*\*Strong\*\*/"));
        app.press("role:button[Server example]").expect("server");
        app.wait_for("Sent by T3 Code").expect("server attribution");
    }
}
#[test]
fn long_messages_measure_the_rem_threshold_and_find_reveals_them() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        app.wait_for("Daily audit").expect("row");
        app.press("role:button[Own example]").expect("own");
        app.wait_for("Show full message").expect("measured fold");
        assert!(body_node(&app).bounds.size.height <= px(176.0));
        app.press("role:button[Show full message]").expect("expand");
        app.wait_for("Show less").expect("reverse");
        assert!(body_node(&app).bounds.size.height >= px(352.0));
        app.press("role:button[Show less]").expect("collapse");
        app.wait_for("Show full message").expect("folded");
        app.press("role:button[Reveal for find]").expect("find");
        app.wait_for("Show less").expect("revealed");
        assert!(body_node(&app).bounds.size.height >= px(352.0));
    }
}
