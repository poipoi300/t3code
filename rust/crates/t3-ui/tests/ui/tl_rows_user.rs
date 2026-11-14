//! TL-ROWS-USER: the real row consumes the client's timeline projection, with the context
//! built by its production constructor from the thread's `TimelineServices`.
use crate::support::{DESKTOP, PHONE};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{Headless, LayoutNode};
use serde_json::json;
use std::rc::Rc;
use t3_client::ClientStore;
use t3_client::services::assets::AssetUrlState;
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRow, MessagesTimelineRowsInput, derive_messages_timeline_rows,
};
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_client::view_logic::session::{ChatMessage, TimelineContent, TimelineEntry};
use t3_contracts::assets::{AssetCreateUrlResult, AssetResource};
use t3_contracts::base_schemas::RunId;
use t3_ui::i18n::{Value as Argument, t, tr};
use t3_ui::runtime::services::assets::{AssetUrlBinding, AssetUrlClient};
use t3_ui::runtime::{Rt, backend::Unavailable, host::Choice};
use t3_ui::screens::thread::rewind::{
    RewindAvailability, RewindError, RewindService, RewindState, RewindTarget, TimelineRewindClient,
};
use t3_ui::screens::thread::rows::user::{self, UserRowContext, UserRowState};
use t3_ui::screens::thread::timeline::TimelineServices;
use t3_ui::state::nav::{Navigator, Route};

fn thread_key() -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("environment".into()),
        thread: ThreadId("thread".into()),
    }
}

fn projection(kind: &str) -> Option<Rc<MessagesTimelineRow>> {
    // A run and a turn-start intent: the client offers a rewind only for such messages.
    let mut message = json!({"id":kind,"role":"user","text":"","runId":"run","inputIntent":"turn_start","streaming":false,
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
        "chip" => json!({
            "text":"Please fix [a.ts](t3-context://v1/mention/mention-a)",
            "context":{"records":[{"version":1,"contextId":"mention-a","kind":"mention","label":"a.ts","path":"src/a.ts"}]}
        }),
        "image" => json!({
            "text":"Look at this",
            "attachments":[{"type":"image","id":"photo","name":"photo.png","mimeType":"image/png","sizeBytes":12}]
        }),
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

/// The client's rewind operation: counts confirmed rewinds.
struct Rewind(Signal<usize>);
impl TimelineRewindClient for Rewind {
    fn state(&self, _: &ThreadKey) -> RewindState {
        RewindState {
            reverting: false,
            can_fork: true,
        }
    }
    fn availability(&self, _: &ThreadKey, _: &RewindTarget) -> RewindAvailability {
        RewindAvailability {
            can_rewind: true,
            can_restore_files: false,
            reason: None,
        }
    }
    fn changes(&self, _: &ThreadKey) -> LocalBoxStream<'static, ()> {
        futures::stream::pending().boxed_local()
    }
    fn rewind(
        &self,
        _: ThreadKey,
        target: RewindTarget,
        _: bool,
    ) -> LocalBoxFuture<'static, Result<String, RewindError>> {
        assert_eq!(target.turn_count, 0);
        self.0.update(|count| *count += 1);
        futures::future::ready(Ok(String::new())).boxed_local()
    }
    fn fork(
        &self,
        _: ThreadKey,
        _: RunId,
    ) -> LocalBoxFuture<'static, Result<ThreadKey, RewindError>> {
        futures::future::pending().boxed_local()
    }
}

/// Signs every attachment image and records which ids were asked for.
struct Assets(Signal<Vec<String>>);
impl AssetUrlClient for Assets {
    fn watch(
        &self,
        _: EnvironmentId,
        resource: AssetResource,
    ) -> LocalBoxStream<'static, AssetUrlState> {
        let AssetResource::Attachment { attachment_id, .. } = &resource else {
            panic!("user images are attachments, got {resource:?}");
        };
        let id = attachment_id.0.clone();
        self.0.update(|ids| ids.push(id.clone()));
        let state = AssetUrlState::Success {
            url: format!("https://server.test/api/assets/{id}"),
            asset: Rc::new(
                serde_json::from_value::<AssetCreateUrlResult>(
                    json!({"relativeUrl":"/api/assets/x","expiresAt":1}),
                )
                .expect("asset"),
            ),
        };
        futures::stream::once(async move { state })
            .chain(futures::stream::pending())
            .boxed_local()
    }
}

#[component]
fn UserRowFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    t3_ui::i18n::set_language("en", &[]);
    provide_context(t3_ui::theme::text_view_parts());
    provide_context(Rt::new(
        ClientStore::default(),
        Rc::new(Unavailable("fixture".into())),
        Choice::Demo,
    ));
    provide_context(Navigator::new(Route::Thread(thread_key())));
    let edits = signal(0usize);
    let signed = signal(Vec::<String>::new());
    provide_context(RewindService(Rc::new(Rewind(edits))));
    provide_context(AssetUrlBinding(Rc::new(Assets(signed))));
    let selected = signal("automation");
    let state = UserRowState::new();
    let services = TimelineServices::new(thread_key());
    let mut context = UserRowContext::new(state, &services);
    // The row's projection inputs; its actions come from the constructor.
    let working = signal(false);
    let find_active = signal(false);
    context.working = computed(move || working.get());
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
        ("Chip example", "chip"),
        ("Image example", "image"),
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
                &[("count", Argument::Int(edits.get() as i64))],
            ))
        }))
        .child(text(move || {
            SharedString::from(format!("Signed: {}", signed.get().join(", ")))
        }))
        .child(user::user_message_row(
            computed(move || projection(selected.get())),
            context,
        ))
        .child(t3_ui::ui::confirm::confirm_dialog_host());
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
        app.wait_for("text:/Hidden agent payload/")
            .expect("agent body");
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
    app.press("role:button[Toggle working]").expect("busy");
    assert!(
        !app.screen()
            .enabled("role:button[Edit from here]")
            .expect("edit")
    );
    let _ = app.press("role:button[Edit from here]");
    assert!(!app.screen().shows("Edit from here?"));
    app.press("role:button[Toggle working]").expect("idle");
}

/// The constructor binds "Edit from here" to the thread's rewind service: it asks first, is
/// disabled while the request is open, and rewinds once on confirm.
#[test]
fn the_production_context_edits_from_here_through_the_rewind_service() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        app.wait_for("Daily audit").expect("row");
        app.press("role:button[Own example]").expect("own");
        app.wait_for("role:button[Edit from here]")
            .expect("edit offered");
        app.press("role:button[Edit from here]").expect("edit");
        app.wait_for("Edit from here?").expect("confirmation");
        assert!(
            !app.screen()
                .enabled("role:button[Edit from here]")
                .expect("edit")
        );
        assert!(app.screen().shows("Edits: 0"));
        app.press("role:button[Confirm]").expect("confirm");
        app.wait_for("Edits: 1").expect("one rewind");
    }
}

/// A sent context reference renders CMP-CHIPS' chip, not an empty gap.
#[test]
fn sent_context_references_render_their_chip() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        app.wait_for("Daily audit").expect("row");
        app.press("role:button[Chip example]").expect("chip");
        app.wait_for("text:/Please fix/").expect("message");
        app.wait_for("a.ts").expect("the mention chip");
    }
}

/// A committed image without a local preview shows through the thread's signed asset URL.
#[test]
fn committed_images_resolve_through_the_signed_asset_url() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        app.wait_for("Daily audit").expect("row");
        app.press("role:button[Image example]").expect("image");
        app.wait_for("Signed: photo")
            .expect("one signed URL for the attachment");
        app.wait_for("role:button[Preview photo.png]")
            .expect("the image is drawn, not its file-name fallback");
    }
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
