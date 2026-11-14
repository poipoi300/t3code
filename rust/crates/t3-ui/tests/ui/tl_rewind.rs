//! The real user row requests confirmation, then calls a client service, never a raw rollback.

use crate::support::{DESKTOP, PHONE};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use std::rc::Rc;
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRowsInput, derive_messages_timeline_rows,
};
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_client::view_logic::session::TimelineEntry;
use t3_contracts::base_schemas::RunId;
use t3_ui::screens::thread::{
    rewind::*,
    rows::user::{UserRowContext, UserRowState, user_message_row},
};
use t3_ui::state::nav::{Navigator, Route};
use t3_ui::ui::confirm::confirm_dialog_host;

fn key(id: &str) -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("environment".into()),
        thread: ThreadId(id.into()),
    }
}

#[derive(Clone)]
struct Client {
    allowed: Signal<bool>,
    files: Signal<bool>,
    fail: Signal<bool>,
    calls: Signal<Vec<(String, u64, bool)>>,
    forks: Signal<usize>,
}

impl TimelineRewindClient for Client {
    fn state(&self, _: &ThreadKey) -> RewindState {
        RewindState {
            reverting: false,
            can_fork: true,
        }
    }
    fn availability(&self, _: &ThreadKey, _: &RewindTarget) -> RewindAvailability {
        RewindAvailability {
            can_rewind: self.allowed.get_untracked(),
            can_restore_files: self.files.get_untracked(),
            reason: (!self.allowed.get_untracked()).then(|| RewindError {
                message: "Interrupt the current turn before reverting checkpoints.".into(),
                detail: None,
            }),
        }
    }
    fn changes(&self, _: &ThreadKey) -> LocalBoxStream<'static, ()> {
        futures::stream::pending().boxed_local()
    }
    fn rewind(
        &self,
        thread: ThreadKey,
        target: RewindTarget,
        files: bool,
    ) -> LocalBoxFuture<'static, Result<String, RewindError>> {
        assert_eq!(thread, key("parent"));
        self.calls
            .update(|calls| calls.push((target.message_id.0, target.turn_count, files)));
        futures::future::ready(if self.fail.get_untracked() {
            Err(RewindError {
                message: "Failed to revert thread state.".into(),
                detail: Some("Settings".into()),
            })
        } else {
            Ok("Restored prompt from client".into())
        })
        .boxed_local()
    }
    fn fork(
        &self,
        _: ThreadKey,
        run: RunId,
    ) -> LocalBoxFuture<'static, Result<ThreadKey, RewindError>> {
        assert_eq!(run.0, "run");
        self.forks.update(|count| *count += 1);
        futures::future::ready(if self.fail.get_untracked() {
            Err(RewindError {
                message: "Failed to fork this response.".into(),
                detail: None,
            })
        } else {
            Ok(key("fork"))
        })
        .boxed_local()
    }
}

fn target_row() -> Rc<t3_client::thread_views::messages_timeline::MessagesTimelineRow> {
    let entry: TimelineEntry = serde_json::from_value(json!({"id":"prompt","kind":"message","createdAt":"2026-11-13T12:00:00Z","message":{"id":"prompt","role":"user","text":"Original prompt","runId":"run","inputIntent":"turn_start","streaming":false,"createdAt":"2026-11-13T12:00:00Z","updatedAt":"2026-11-13T12:00:00Z"}})).expect("canonical message");
    derive_messages_timeline_rows(&MessagesTimelineRowsInput {
        timeline_entries: vec![Rc::new(entry)],
        supports_conversation_rollback: true,
        thread_start_checkpoint_available: true,
        available_revert_turn_counts: Some([0].into()),
        ..Default::default()
    })
    .into_iter()
    .find(|row| RewindTarget::from_row(row).is_some())
    .expect("client rewindable user row")
}

fn control(id: &'static str, action: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .test_id(id)
        .child(text(id.to_owned()))
        .on_press(action)
}

#[component]
fn RewindFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    t3_ui::i18n::set_language("en", &[]);
    provide_context(t3_ui::theme::text_view_parts());
    let nav = Navigator::new(Route::Thread(key("parent")));
    provide_context(nav);
    let client = Client {
        allowed: signal(true),
        files: signal(false),
        fail: signal(false),
        calls: signal(Vec::new()),
        forks: signal(0),
    };
    let calls = client.calls;
    let forks = client.forks;
    let allowed = client.allowed;
    let files = client.files;
    let fail = client.fail;
    let rewind = TimelineRewind::with_client(key("parent"), Some(Rc::new(client)));
    let row = target_row();
    let target = RewindTarget::from_row(&row).expect("target");
    let projection = computed(move || Some(row.clone()));
    let mut user = UserRowContext::new(UserRowState::new());
    rewind.bind_user_row(&mut user);
    on_key("ctrl+d", move |_| allowed.set(false));
    view()
        .w_full()
        .h_full()
        .child(user_message_row(projection, user))
        .child(control("deny", move || allowed.set(false)))
        .child(control("files", move || files.set(true)))
        .child(control("fail", move || fail.set(true)))
        .child(control("restore-files", move || {
            rewind.request_revert(target.clone(), true)
        }))
        .child(control("fork", move || {
            rewind.on_fork_from_run(RunId("run".into()))
        }))
        .child(text(move || {
            SharedString::from(format!("Calls: {}", calls.get().len()))
        }))
        .child(text(move || {
            SharedString::from(format!("Forks: {}", forks.get()))
        }))
        .child(text(move || {
            SharedString::from(format!("Busy: {}", rewind.reverting.get()))
        }))
        .child(text(move || SharedString::from(nav.top().path())))
        .child(text(move || {
            rewind
                .error
                .get()
                .map_or_else(SharedString::default, |error| error.message().resolve())
        }))
        .child(text(move || {
            SharedString::from(format!("Payload: {:?}", calls.get()))
        }))
        .child(text(move || {
            SharedString::from(format!("Restored: {:?}", rewind.restored_prompt.get()))
        }))
        .child(confirm_dialog_host())
}

#[component]
fn MissingServiceFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    t3_ui::i18n::set_language("en", &[]);
    provide_context(t3_ui::theme::text_view_parts());
    let nav = Navigator::new(Route::Thread(key("parent")));
    provide_context(nav);
    let rewind = use_timeline_rewind(key("parent"));
    let row = target_row();
    let target = RewindTarget::from_row(&row).expect("target");
    let projection = computed(move || Some(row.clone()));
    let mut user = UserRowContext::new(UserRowState::new());
    rewind.bind_user_row(&mut user);
    view()
        .w_full()
        .h_full()
        .child(user_message_row(projection, user))
        .child(control("direct-rewind", move || {
            rewind.request_revert(target.clone(), true)
        }))
        .child(control("fork", move || {
            rewind.on_fork_from_run(RunId("run".into()))
        }))
        .child(text(move || SharedString::from(nav.top().path())))
        .child(text(move || {
            SharedString::from(format!("Busy: {}", rewind.reverting.get()))
        }))
        .child(confirm_dialog_host())
}

fn fixture(window: (f32, f32)) -> Headless {
    start(Root::of::<RewindFixture>(), window)
}

fn start(root: Root, window: (f32, f32)) -> Headless {
    let mut sets = t3_ui::registries();
    let mut extra = Registry::new();
    extra.register::<RewindFixture>();
    extra.register::<MissingServiceFixture>();
    sets.push(extra);
    let mut app = Headless::start(EngineConfig {
        ui_sets: sets,
        viewport: size(px(window.0), px(window.1)),
        environment: nsh_core::platform::Environment {
            reduce_motion: true,
            ..Default::default()
        },
        ..EngineConfig::new(root)
    })
    .expect("rewind fixture");
    app.wait_for("Original prompt").expect("user row");
    app
}

#[test]
fn absent_service_disables_the_user_action_and_direct_calls_do_nothing() {
    let mut app = start(Root::of::<MissingServiceFixture>(), DESKTOP);
    let _ = app.press("role:button[Edit from here]");
    app.press("id:direct-rewind").expect("direct request");
    app.press("id:fork").expect("fork request");
    assert!(app.screen().shows("Busy: false"));
    assert!(app.screen().shows("/environment/parent"));
    assert!(!app.screen().shows("Edit from here?"));
}

#[test]
fn cancel_keeps_the_thread_and_confirm_forwards_the_exact_user_target_once() {
    for size in [PHONE, DESKTOP] {
        let mut app = fixture(size);
        app.press("role:button[Edit from here]").expect("edit");
        app.wait_for("Edit from here?").expect("root host");
        assert!(app.screen().shows("Calls: 0"));
        app.press("role:button[Cancel]").expect("cancel");
        app.wait_for("Busy: false").expect("cancel settled");
        assert!(app.screen().shows("Calls: 0"));
        assert!(app.screen().shows("/environment/parent"));
        app.press("role:button[Edit from here]")
            .expect("edit again");
        app.wait_for("Edit from here?").expect("confirm again");
        app.press("role:button[Confirm]").expect("confirm");
        app.wait_for("Calls: 1").expect("client called");
        app.wait_for("Busy: false").expect("completed");
        app.wait_for("Restored prompt from client")
            .expect("client outcome reaches the editor binding");
        assert!(app.screen().shows("(\"prompt\", 0, false)"));
    }
}

#[test]
fn client_denials_and_unavailable_file_restoration_never_dispatch() {
    let mut app = fixture(DESKTOP);
    app.press("id:restore-files").expect("client refuses files");
    assert!(app.screen().shows("Calls: 0"));
    assert!(!app.screen().shows("Edit from here?"));
    app.press("id:deny").expect("permission changed");
    app.press("role:button[Edit from here]")
        .expect("guarded action");
    app.wait_for("Interrupt the current turn before reverting checkpoints.")
        .expect("client reason");
    assert!(!app.screen().shows("Edit from here?"));
    assert!(app.screen().shows("Calls: 0"));
}

#[test]
fn confirmation_rechecks_availability_and_failures_release_the_busy_state() {
    let mut app = fixture(DESKTOP);
    app.press("role:button[Edit from here]").expect("edit");
    app.wait_for("Edit from here?").expect("confirm");
    app.press_key("ctrl+d")
        .expect("revoke client authorization while modal open");
    app.press("role:button[Confirm]").expect("answer");
    app.wait_for("Busy: false").expect("guard resolved");
    assert!(app.screen().shows("Calls: 0"));
    let mut app = fixture(DESKTOP);
    app.press("id:fail").expect("client error");
    app.press("role:button[Edit from here]").expect("edit");
    app.wait_for("Edit from here?").expect("confirm");
    app.press("role:button[Confirm]").expect("answer");
    app.wait_for("Failed to revert thread state.")
        .expect("error surfaced");
    app.wait_for("Busy: false").expect("error releases state");
    assert!(app.screen().shows("/environment/parent"));
}

#[test]
fn isolated_files_are_explicit_and_fork_opens_only_the_clients_ready_target() {
    let mut app = fixture(PHONE);
    app.press("id:files")
        .expect("client reports isolated ready checkpoint");
    app.press("id:restore-files").expect("request files");
    app.wait_for("Edit from here?").expect("confirm files");
    app.press("role:button[Confirm]").expect("answer");
    app.wait_for("Calls: 1").expect("dispatch");
    assert!(app.screen().shows("(\"prompt\", 0, true)"));
    app.press("id:fork").expect("fork");
    app.wait_for("/environment/fork")
        .expect("client returned ready scoped thread");
    let mut app = fixture(DESKTOP);
    app.press("id:fail").expect("fork fails");
    app.press("id:fork").expect("fork");
    app.wait_for("Failed to fork this response.")
        .expect("error");
    assert!(app.screen().shows("/environment/parent"));
}
