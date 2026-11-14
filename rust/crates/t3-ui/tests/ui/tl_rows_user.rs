//! TL-ROWS-USER: row interactions isolated from the timeline integration seam.

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{Headless, LayoutNode};
use serde_json::json;
use std::rc::Rc;
use t3_client::view_logic::session::{ChatMessage, TimelineContent, TimelineEntry};
use t3_ui::i18n::{Value, t, tr};
use t3_ui::runtime::Shared;
use t3_ui::screens::thread::rows::user::{
    self, Attribution, FilePresentation, Intent, UserRow, UserRowContext, UserRowState,
};

fn fixture_row(id: &str) -> UserRow {
    let (attribution, text, summary) = match id {
        "automation" => (
            Attribution::Automation,
            "Run the full daily audit.".into(),
            "",
        ),
        "agent" => (
            Attribution::Agent,
            "Hidden agent payload".into(),
            "Agent summary",
        ),
        _ => (Attribution::Own, "Long source line\n".repeat(30), ""),
    };
    UserRow {
        id: id.into(),
        text: text.clone(),
        copy_text: text,
        prompt_link: None,
        attribution,
        summary: summary.into(),
        timestamp: "12:00".into(),
        timestamp_tooltip: "12:00, 13th November 2026".into(),
        status: None,
        intent: Some(Intent {
            promoted: true,
            unread: true,
        }),
        continues_steer: false,
        grouped_with_previous: false,
        grouped_with_next: false,
        sender_label: "Agent".into(),
        can_open_sender: false,
        scheduled_task_title: Some("Daily audit".into()),
        has_scheduled_task: id == "automation",
        revert_turn_count: Some(2),
        fresh_context: false,
        model_switch: false,
        workflow_opening: false,
        has_media: false,
        has_audio: false,
        has_files: false,
        media_only: false,
        media_above_text: false,
        file_cards_only: false,
        audio_only: false,
    }
}

fn entry(id: &str) -> Shared<TimelineEntry> {
    let message: ChatMessage = serde_json::from_value(json!({
        "id": id, "role": "user", "text": "fixture", "runId": null, "streaming": false,
        "createdAt": "2026-11-13T12:00:00.000Z", "updatedAt": "2026-11-13T12:00:00.000Z"
    }))
    .expect("message");
    Shared(Rc::new(TimelineEntry {
        id: id.into(),
        created_at: message.created_at.clone(),
        attempt: None,
        content: TimelineContent::Message {
            message: Rc::new(message),
            projected_item: None,
        },
    }))
}

#[component]
fn UserRowFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let selected = signal("automation");
    let edits = signal(0);
    let state = UserRowState::new();
    let mut context = UserRowContext::new(
        state,
        Rc::new(|entry| {
            if entry.id == "missing" {
                Err("User message presentation is unavailable.".into())
            } else {
                Ok(fixture_row(&entry.id))
            }
        }),
    );
    context.revert = Some(Rc::new(move |row, count| {
        assert_eq!(count, 2);
        assert!(!row.id.is_empty());
        edits.update(|n| *n += 1);
    }));
    let working = context.working;
    let reverting = context.reverting;
    let find_active = context.find_active;
    let mut root = view().w_full().h_full();
    for (label, id) in [
        ("Automation example", "automation"),
        ("Agent example", "agent"),
        ("Own example", "own"),
        ("Missing example", "missing"),
    ] {
        root = root.child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(t(label))))
                .on_press(move || selected.set(id))
                .child(text(t(label))),
        );
    }
    root.child(
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
        SharedString::from(tr("Edits: {count}", &[("count", Value::Int(edits.get()))]))
    }))
    .child(user::user_message_row(
        computed(move || Some(entry(selected.get()))),
        context,
    ))
}

#[component]
fn FileFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let opens = signal(0);
    let downloads = signal(0);
    let sending = signal(true);
    view().w_full().h_full()
        .child(nsh_base::button().accessibility_label(Some(SharedString::from(t("Finish upload"))))
            .on_press(move || sending.set(false)).child(text(t("Finish upload"))))
        .child(text(move || SharedString::from(tr("Opens: {count}", &[("count", Value::Int(opens.get()))]))))
        .child(text(move || SharedString::from(tr("Downloads: {count}", &[("count", Value::Int(downloads.get()))]))))
        .child(dynamic(move || user::user_file_card(FilePresentation {
            file: serde_json::from_value(json!({
                "type": "file", "id": "file", "name": "notes.txt", "mimeType": "text/plain", "sizeBytes": 12
            })).expect("file"),
            downloadable: !sending.get(), extension: "TXT".into(),
            open: Some(Rc::new(move |_| opens.update(|n| *n += 1))),
            download: Some(Rc::new(move |_| downloads.update(|n| *n += 1))),
            entry_icon: None, download_icon: None,
        })))
}

fn fixture<T: nsh_core::element::Component>(window: (f32, f32)) -> Headless
where
    T::Props: Default,
{
    let mut registries = t3_ui::registries();
    let mut fixtures = Registry::new();
    fixtures.register::<UserRowFixture>();
    fixtures.register::<FileFixture>();
    registries.push(fixtures);
    Headless::start(EngineConfig {
        ui_sets: registries,
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<T>())
    })
    .expect("row fixture starts")
}

fn body_node(app: &Headless) -> LayoutNode {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some("data-user-message-body"))
        .expect("body has layout")
}

#[test]
fn automation_and_agent_folds_keep_their_own_state_when_a_cell_is_reused() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture::<UserRowFixture>(window);
        app.wait_for("Daily audit").expect("scheduled title");
        assert!(!app.screen().shows("Run the full daily audit."));
        app.press("role:button[Automation]")
            .expect("expand automation");
        app.wait_for("Run the full daily audit.")
            .expect("runbook expanded");
        app.press("role:button[Agent example]")
            .expect("recycle as agent");
        app.wait_for("Agent summary").expect("folded sender");
        assert!(!app.screen().shows("Hidden agent payload"));
        app.press("role:button[Agent]").expect("expand sender");
        app.wait_for("Hidden agent payload").expect("agent body");
        app.press("role:button[Automation example]")
            .expect("reuse automation");
        app.wait_for("Run the full daily audit.")
            .expect("automation kept expansion");
        app.press("role:button[Automation]")
            .expect("collapse automation");
        assert!(!app.screen().shows("Run the full daily audit."));
        app.press("role:button[Agent example]")
            .expect("reuse agent");
        app.wait_for("Hidden agent payload")
            .expect("agent kept its independent expansion");
    }
}

#[test]
fn edits_are_inert_during_work_and_revert_and_missing_copy_never_runs() {
    let mut app = fixture::<UserRowFixture>(PHONE);
    app.wait_for("Daily audit").expect("row");
    app.press("role:button[Automation]").expect("expand");
    assert!(
        !app.screen()
            .enabled("role:button[Copy message]")
            .expect("copy action")
    );
    for toggle in [
        "role:button[Toggle working]",
        "role:button[Toggle reverting]",
    ] {
        app.press(toggle).expect("busy");
        assert!(
            !app.screen()
                .enabled("role:button[Edit from here]")
                .expect("edit action")
        );
        let _ = app.press("role:button[Edit from here]");
        assert!(app.screen().shows("Edits: 0"));
        app.press(toggle).expect("idle");
    }
    app.press("role:button[Edit from here]").expect("edit");
    app.wait_for("Edits: 1").expect("one edit");
    app.press("role:button[Missing example]")
        .expect("unresolved row");
    app.wait_for("User message presentation is unavailable.")
        .expect("honest unsupported state");
    assert!(!app.screen().shows("Daily audit"));
}

#[test]
fn long_messages_fold_by_measured_height_and_find_reveals_them() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture::<UserRowFixture>(window);
        app.wait_for("Daily audit").expect("initial row mounted");
        app.press("role:button[Own example]").expect("own message");
        app.wait_for("Show full message")
            .expect("measured tall message");
        assert!(body_node(&app).bounds.size.height <= px(176.0));
        app.press("role:button[Show full message]").expect("expand");
        app.wait_for("Show less").expect("reverse action");
        assert!(body_node(&app).bounds.size.height >= px(352.0));
        app.press("role:button[Show less]").expect("collapse");
        app.wait_for("Show full message").expect("collapsed again");
        app.press("role:button[Reveal for find]")
            .expect("find reveals");
        app.wait_for("Show less").expect("expanded for find");
        assert!(body_node(&app).bounds.size.height >= px(352.0));
    }
}

#[test]
fn sending_attachments_cannot_open_or_download_until_the_client_marks_them_ready() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture::<FileFixture>(window);
        app.wait_for("Sending…").expect("pending file");
        assert!(
            !app.screen()
                .enabled("role:button[Sending attachment]")
                .expect("sending action")
        );
        let _ = app.press("role:button[Sending attachment]");
        assert!(app.screen().shows("Downloads: 0"));
        assert!(!app.screen().shows("Preview notes.txt"));
        app.press("role:button[Finish upload]")
            .expect("upload completed");
        app.wait_for("TXT · 12 B").expect("file ready");
        app.press("role:button[Preview notes.txt]")
            .expect("open file");
        app.wait_for("Opens: 1").expect("preview action ran");
        app.press("role:button[Download notes.txt]")
            .expect("download file");
        app.wait_for("Downloads: 1").expect("download action ran");
    }
}
