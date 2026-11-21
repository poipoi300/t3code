//! TL-ROWS-ASSISTANT: client-derived rows through the component's production entry points.

use crate::support::{DESKTOP, PHONE};
use futures::{
    FutureExt, StreamExt, channel::oneshot, future::LocalBoxFuture, stream::LocalBoxStream,
};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use std::rc::Rc;
use std::time::{Duration, SystemTime};
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRowContent, MessagesTimelineRowsInput, TimelineLatestRun,
    derive_messages_timeline_rows,
};
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_client::view_logic::session::TimelineEntry;
use t3_contracts::base_schemas::RunId;
use t3_ui::i18n::t;
use t3_ui::runtime::Shared;
use t3_ui::screens::thread::rewind::{
    RewindAvailability, RewindError, RewindService, RewindState, RewindTarget, TimelineRewindClient,
};
use t3_ui::screens::thread::rows::assistant::{self, AssistantRowContext, AssistantRowsState};
use t3_ui::screens::thread::timeline::TimelineServices;
use t3_ui::ui::toast;

const AT: &str = "2026-11-13T12:34:00Z";
const FINAL: &str = "**Final answer**\n\nA second paragraph.";
const DIRECTIVES: &str = "Created :codex-file-citation{path=\"outputs/report.xlsx\" purpose=\"output\"}.\n\n::artifact-template{skill_name=\"artifact-template-hello-world\" skill_directory=\"/test/skills/artifact-template-hello-world\" display_name=\"Hello World\" artifact_kind=\"document\"}";

fn entry(text: &str, streaming: bool, status: &str) -> Rc<TimelineEntry> {
    Rc::new(serde_json::from_value(json!({
        "id":"row-answer", "kind":"message", "createdAt":AT,
        "message":{"id":"answer","role":"assistant","text":text,"streaming":streaming,"runId":"run","createdAt":AT,"updatedAt":AT},
        "projectedItem":{"position":0,"visibility":"inherited","sourceThreadId":"source-thread","sourceItemId":"source-item",
          "item":{"id":"source-item","threadId":"source-thread","type":"assistant_message","messageId":"answer","text":text,
          "streaming":streaming,"runId":"run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,
          "parentItemId":null,"ordinal":0,"status":status,"title":null,"startedAt":AT,"completedAt":null,"updatedAt":AT}}
    })).expect("canonical timeline entry"))
}

fn control(label: &'static str, press: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(Some(t(label).into()))
        .on_press(press)
        .child(text(t(label)))
}

fn conversation() -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("environment".into()),
        thread: ThreadId("conversation".into()),
    }
}

/// The client's fork operation: checks the source identity, waits for its release once, then
/// succeeds or fails.
struct Forks {
    count: Signal<usize>,
    fail: Signal<bool>,
    waiting: Stored<Option<oneshot::Receiver<()>>>,
}
impl TimelineRewindClient for Forks {
    fn state(&self, _: &ThreadKey) -> RewindState {
        RewindState {
            reverting: false,
            can_fork: true,
        }
    }
    fn availability(&self, _: &ThreadKey, _: &RewindTarget) -> RewindAvailability {
        RewindAvailability::default()
    }
    fn changes(&self, _: &ThreadKey) -> LocalBoxStream<'static, ()> {
        futures::stream::pending().boxed_local()
    }
    fn rewind(
        &self,
        _: ThreadKey,
        _: RewindTarget,
        _: bool,
    ) -> LocalBoxFuture<'static, Result<String, RewindError>> {
        futures::future::pending().boxed_local()
    }
    fn fork(
        &self,
        viewed: ThreadKey,
        source: ThreadId,
        _: t3_contracts::base_schemas::TurnItemId,
        run: RunId,
    ) -> LocalBoxFuture<'static, Result<ThreadKey, RewindError>> {
        assert_eq!(source.0, "source-thread");
        assert_eq!(viewed.environment.0, "environment");
        assert_eq!(run.0, "run");
        self.count.update(|count| *count += 1);
        let waiting = self.waiting.with(Option::take);
        let fail = self.fail.get_untracked();
        async move {
            if let Some(waiting) = waiting {
                let _ = waiting.await;
            }
            if fail {
                Err(RewindError {
                    message: "Permission denied".into(),
                    detail: None,
                })
            } else {
                Ok(ThreadKey {
                    environment: EnvironmentId("environment".into()),
                    thread: ThreadId("fork".into()),
                })
            }
        }
        .boxed_local()
    }
}

#[component]
fn AssistantFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let value = signal(entry("Working draft", true, "running"));
    let working = signal(true);
    let embedded = signal(false);
    let sent = signal(false);
    let rows = computed(move || {
        let mut entries = vec![value.get()];
        if embedded.get() {
            entries.push(Rc::new(serde_json::from_value(json!({"id":"html","kind":"html-render","createdAt":AT,"runId":"run","htmlRender":{"attachmentId":"preview","title":"Preview","height":420}})).expect("embedded entry")));
        }
        if sent.get() {
            entries.push(Rc::new(serde_json::from_value(json!({"id":"sent","kind":"sent-message","createdAt":AT,"runId":"run","text":"[Across fragments][ref]\n\n[ref]: https://example.test"})).expect("canonical sent message")));
        }
        derive_messages_timeline_rows(&MessagesTimelineRowsInput {
            timeline_entries: entries,
            is_working: working.get(),
            running_run_id: working.get().then(|| "run".into()),
            latest_run: Some(TimelineLatestRun {
                run_id: "run".into(),
                status: if working.get() {
                    "running"
                } else {
                    "completed"
                }
                .into(),
                started_at: Some(AT.into()),
                completed_at: (!working.get()).then(|| AT.into()),
                turn_ended: None,
            }),
            ..Default::default()
        })
    });
    let projected = computed(move || {
        rows.with(|rows| rows.iter().find(|row| matches!(&row.content, MessagesTimelineRowContent::Message { message, .. } if message.role == "assistant")).cloned().map(Shared))
    });
    let row = assistant::assistant_row(projected);
    let state = AssistantRowsState::default();
    let fork_count = signal(0);
    let fail = signal(false);
    let visible = signal(true);
    let (release, waiting) = oneshot::channel::<()>();
    let release = stored(Some(release));
    let waiting = stored(Some(waiting));
    provide_context(t3_ui::runtime::Rt::new(
        t3_client::ClientStore::default(),
        Rc::new(t3_ui::runtime::backend::Unavailable("fixture".into())),
        t3_ui::runtime::host::Choice::Demo,
    ));
    provide_context(t3_ui::state::nav::Navigator::new(
        t3_ui::state::nav::Route::Thread(conversation()),
    ));
    provide_context(RewindService(Rc::new(Forks {
        count: fork_count,
        fail,
        waiting,
    })));
    let services = TimelineServices::new(conversation());
    let mut context = AssistantRowContext::new(state, &services);
    context.timestamp_format = computed(|| t3_contracts::settings::TimestampFormat::V24Hour);
    context.markdown_cwd = Some("C:/project".into());
    let body_context = context.clone();
    let sent_context = context;
    toast::anchored_toast_provider(
        view().w_full().h_full().child(
            view()
                .app_class(t3_ui::theme::tw(
                    "group/assistant w-full px-3 sm:px-12 gap-2",
                ))
                .child(control("Finish", move || {
                    working.set(false);
                    value.set(entry(FINAL, false, "completed"));
                }))
                .child(control("Interrupt active", move || {
                    value.set(entry("Partial answer", false, "interrupted"))
                }))
                .child(control("Empty", move || {
                    working.set(false);
                    value.set(entry("", false, "completed"));
                }))
                .child(control("Suppress empty", move || embedded.set(true)))
                .child(control("Reveal empty", move || embedded.set(false)))
                .child(control("Directives", move || {
                    working.set(false);
                    value.set(entry(DIRECTIVES, false, "completed"));
                }))
                .child(control("Replace with code", move || {
                    working.set(false);
                    value.set(entry(
                        "```rust\nlet code_transition = 7;\n```",
                        false,
                        "completed",
                    ));
                }))
                .child(control("Hide row", move || visible.set(false)))
                .child(control("Show row", move || visible.set(true)))
                .child(control("Release fork", move || {
                    if let Some(release) = release.with(Option::take) {
                        let _ = release.send(());
                    }
                }))
                .child(control("Fail forks", move || fail.set(true)))
                .child(control("Sent message", move || sent.set(true)))
                .child(text(move || format!("Forks: {}", fork_count.get())))
                .child(dynamic(move || {
                    visible.get().then(|| {
                        untracked(|| assistant::assistant_timeline_row(row, body_context.clone()))
                    })
                }))
                .child(dynamic(move || {
                    sent.get().then(|| {
                        untracked(|| {
                            let content = computed(move || {
                                rows.with(|rows| {
                                    rows.iter()
                                        .find_map(|row| {
                                            if let MessagesTimelineRowContent::SentMessage {
                                                text,
                                            } = &row.content
                                            {
                                                Some(text.clone())
                                            } else {
                                                None
                                            }
                                        })
                                        .unwrap_or_default()
                                })
                            });
                            assistant::sent_message_timeline_row(content, sent_context.clone())
                        })
                    })
                })),
        ),
    )
}

fn start(window: (f32, f32)) -> Headless {
    let epoch = t3_client::time::epoch_ms(AT).expect("fixture time") as u64;
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        clock: nsh_core::time::Clock::manual(SystemTime::UNIX_EPOCH + Duration::from_millis(epoch)),
        ..EngineConfig::new(Root::of::<AssistantFixture>())
    };
    config.environment.locale_settings.time_zone =
        nsh_core::platform::TimeZone::parse("UTC").expect("fixture timezone");
    if window == DESKTOP {
        config.environment.os = nsh_core::platform::Os::Windows;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    Headless::start(config).expect("assistant rows through client projection")
}

#[test]
fn client_flags_reveal_real_meta_and_copy_after_settlement_at_both_widths() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window);
        app.wait_for("Working draft").expect("streaming body");
        assert!(!app.screen().shows("role:button[Copy message]"));
        assert!(!app.screen().shows("12:34"));
        app.press("role:button[Finish]").expect("finish");
        app.wait_for("Final answer").expect("settled body");
        if window == DESKTOP {
            app.hover("role:button[Copy message]").expect("reveal meta");
        }
        app.wait_for("12:34").expect("client-formatted timestamp");
        let copy_button = app
            .screen()
            .id("role:button[Copy message]")
            .expect("copy button identity");
        app.press("role:button[Copy message]").expect("copy");
        app.wait_for("Copied!").expect("source anchored copy toast");
        assert_eq!(
            app.screen()
                .id("role:button[Copy message]")
                .expect("copied button identity"),
            copy_button
        );
        assert_eq!(app.screen().clipboard(), Some(FINAL));
        assert!(
            !app.screen()
                .enabled("role:button[Copy message]")
                .expect("copied state disables")
        );
        app.run_for(Duration::from_millis(1100))
            .expect("advance manual copy reset");
        assert!(
            app.screen()
                .enabled("role:button[Copy message]")
                .expect("copy state reset")
        );
        assert_eq!(
            app.screen()
                .id("role:button[Copy message]")
                .expect("reset button identity"),
            copy_button
        );
    }
}

#[test]
fn copies_the_rendered_representation_of_codex_directives_from_the_real_component() {
    let mut app = start(PHONE);
    app.wait_for("Working draft").expect("mounted");
    app.press("role:button[Directives]")
        .expect("directive reply");
    app.wait_for("role:button[Copy message]")
        .expect("copy action");
    app.press("role:button[Copy message]").expect("copy");
    app.wait_for("Copied!").expect("copy completed");
    assert_eq!(
        app.screen().clipboard(),
        Some("Created [report.xlsx](<outputs/report.xlsx>).\n\nHello World (Document template)")
    );
}

#[test]
fn client_suppresses_empty_fallback_for_embedded_output_and_interrupted_badge_survives_without_meta()
 {
    let mut app = start(PHONE);
    app.wait_for("Working draft").expect("mounted");
    app.press("role:button[Interrupt active]")
        .expect("interrupt");
    app.wait_for("interrupted").expect("badge without meta");
    assert!(!app.screen().shows("role:button[Copy message]"));
    app.press("role:button[Empty]")
        .expect("empty settled reply");
    app.wait_for("(empty response)").expect("fallback");
    app.press("role:button[Suppress empty]").expect("embed");
    app.wait_until(
        "embedded output suppresses empty text",
        Duration::from_secs(2),
        |screen| !screen.shows("(empty response)"),
    )
    .expect("suppressed by client flags");
    app.press("role:button[Reveal empty]")
        .expect("remove embed");
    app.wait_for("(empty response)").expect("fallback restored");
}

#[test]
fn the_production_context_forks_through_the_rewind_service_and_keeps_busy_state_through_recycling()
{
    let mut app = start(PHONE);
    app.wait_for("Working draft").expect("mounted");
    app.press("role:button[Finish]").expect("finish");
    app.wait_for("role:button[Fork from this response]")
        .expect("concrete fork control");
    app.press("role:button[Fork from this response]")
        .expect("fork");
    app.wait_for("Forks: 1").expect("source operation");
    assert!(
        !app.screen()
            .enabled("role:button[Fork from this response]")
            .expect("busy")
    );
    app.press("role:button[Hide row]").expect("recycle");
    app.press("role:button[Show row]").expect("remount");
    app.wait_for("role:button[Fork from this response]")
        .expect("restored");
    assert!(
        !app.screen()
            .enabled("role:button[Fork from this response]")
            .expect("still busy")
    );
    app.press("role:button[Release fork]")
        .expect("complete operation");
    app.wait_until("fork is enabled", Duration::from_secs(2), |screen| {
        screen
            .enabled("role:button[Fork from this response]")
            .unwrap_or(false)
    })
    .expect("finally clears busy");
    app.press("role:button[Fail forks]")
        .expect("service failure");
    app.press("role:button[Fork from this response]")
        .expect("retry");
    app.wait_for("Forks: 2").expect("service called");
    app.wait_until("failure clears busy", Duration::from_secs(2), |screen| {
        screen
            .enabled("role:button[Fork from this response]")
            .unwrap_or(false)
    })
    .expect("finally on failure");
    assert!(
        !app.screen().shows("Permission denied"),
        "the web row has no destructive error line"
    );
}

#[test]
fn a_settled_edit_reuses_the_message_identity_and_switches_its_prose_leaf_to_code() {
    let mut app = start(PHONE);
    app.wait_for("Working draft").expect("initial prose");
    app.press("role:button[Finish]").expect("settle prose");
    app.wait_for("Final answer").expect("settled prose");
    app.press("role:button[Replace with code]")
        .expect("edit same message");
    app.wait_for("role:button[Copy code]")
        .expect("code chrome replaces prose leaf");
    app.press("role:button[Copy code]")
        .expect("copy actual code");
    app.wait_until("code is copied", Duration::from_secs(2), |screen| {
        screen.clipboard() == Some("let code_transition = 7;\n")
    })
    .expect("copy uses the code leaf source");
}

#[test]
fn sent_message_uses_the_complete_client_source_for_reference_links() {
    let mut app = start(PHONE);
    app.wait_for("Working draft").expect("mounted");
    app.press("role:button[Sent message]")
        .expect("client sent message");
    app.wait_for("Across fragments")
        .expect("standalone Markdown resolves its reference definition");
}

#[test]
fn the_demo_timeline_mounts_meta_and_copies_the_terminal_response_without_an_injected_renderer() {
    let response_copy = "label:T3 Code >> role:button[Copy message]";
    for window in [PHONE, DESKTOP] {
        let mut app = crate::support::start(window);
        crate::support::pair(&mut app);
        app.press("role:button[Build the shared Rust UI slice, t3code]")
            .expect("open the real thread page");
        app.wait_for("I added clamp next to sum and the tests pass.")
            .expect("real timeline body");
        app.wait_for(response_copy)
            .expect("client-derived terminal response meta");
        if window == DESKTOP {
            app.hover(response_copy)
                .expect("assistant frame reveals meta on hover");
        }
        app.press(response_copy).expect("copy reply");
        app.wait_until(
            "terminal reply reaches the clipboard",
            Duration::from_secs(2),
            |screen| {
                screen.clipboard().is_some_and(|text| {
                    text.contains("I added `clamp` next to `sum` and the tests pass.")
                })
            },
        )
        .expect("the timeline uses the component's real copy path");
    }
}

/// The thread page's timeline and composer over one client: Reply from an assistant row puts the
/// composer's "Replying to" banner up for that response.
#[component]
fn ReplyFixture() -> impl IntoElement {
    use t3_client::ClientStore;
    use t3_client::composition::{ClientRuntime, Spawner};
    use t3_client::testing::{FakePlatform, environment};
    use t3_contracts::orchestration_v2::{
        OrchestrationV2CreationSource, OrchestrationV2ShellStreamItem,
        OrchestrationV2ThreadStreamItem,
    };
    use t3_ui::runtime::{Rt, backend::Unavailable, host::Choice};
    use t3_ui::screens::thread::composer::{reply_to_response, thread_composer};
    use t3_ui::screens::thread::rows::assistant::ReplyService;
    use t3_ui::state::nav::{Navigator, Route};
    t3_ui::theme::install(&Default::default());
    let key = ThreadKey {
        environment: EnvironmentId("env".into()),
        thread: ThreadId("thread-a".into()),
    };
    let store = ClientStore::default();
    let session = store.begin_session(key.environment.clone());
    let shell = [
        json!({"kind":"snapshot","snapshot":environment::shell(&[("thread-a", "Reply thread")])}),
        json!({"kind":"synchronized"}),
    ]
    .into_iter()
    .map(|item| serde_json::from_value::<OrchestrationV2ShellStreamItem>(item).expect("shell"))
    .collect();
    store.apply_shell(&key.environment, session, shell);
    let item = json!({"id":"answer-item","threadId":"thread-a","type":"assistant_message",
        "messageId":"answer","text":"Here is the plan for the parser.","streaming":false,
        "runId":"run","nodeId":null,"providerThreadId":null,"providerTurnId":null,
        "nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,
        "startedAt":AT,"completedAt":AT,"updatedAt":AT});
    let mut projection =
        environment::projection("thread-a", "Reply thread", json!([]), json!([item]));
    projection["visibleTurnItems"] = json!([{"position":0,"visibility":"local",
        "sourceThreadId":"thread-a","sourceItemId":"answer-item","item":item}]);
    let thread = [
        json!({"kind":"snapshot","snapshotSequence":1,"projection":projection}),
        json!({"kind":"synchronized"}),
    ]
    .into_iter()
    .map(|item| serde_json::from_value::<OrchestrationV2ThreadStreamItem>(item).expect("thread"))
    .collect();
    store
        .apply_thread(key.clone(), session, thread)
        .expect("thread applies");
    provide_context(Rt::new(
        store.clone(),
        Rc::new(Unavailable("fixture".into())),
        Choice::Demo,
    ));
    provide_context(Navigator::new(Route::Thread(key.clone())));
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        });
    });
    let client = ClientRuntime::new(
        FakePlatform::default(),
        store,
        OrchestrationV2CreationSource::Web,
        spawner,
    );
    let reply_client = client.clone();
    let reply_key = key.clone();
    provide_context(ReplyService(Rc::new(move |request| {
        reply_to_response(&reply_client, &reply_key, request)
    })));
    toast::anchored_toast_provider(
        view()
            .w_full()
            .h_full()
            .child(
                // The thread page gives the timeline the space above the composer.
                view()
                    .h(px(420.0))
                    .child(t3_ui::screens::thread::timeline::timeline(key.clone())),
            )
            .child(thread_composer(client, key)),
    )
}

#[test]
fn reply_puts_the_response_in_the_composers_replying_to_banner() {
    for window in [PHONE, DESKTOP] {
        let mut sets = t3_ui::registries();
        let mut fixtures = Registry::new();
        fixtures.register::<ReplyFixture>();
        sets.push(fixtures);
        let mut config = EngineConfig {
            ui_sets: sets,
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::of::<ReplyFixture>())
        };
        if window == DESKTOP {
            config.environment.os = nsh_core::platform::Os::Linux;
            config.environment.idiom = nsh_core::platform::Idiom::Desktop;
        }
        let mut app = Headless::start(config).expect("fixture");
        app.wait_for("Here is the plan for the parser.")
            .expect("assistant row");
        app.wait_for("role:button[Attach files]")
            .expect("the composer has opened its writable draft");
        assert!(!app.screen().shows("Replying to"));
        app.press("role:button[Reply to this response]")
            .expect("Reply is bound by the row's constructor");
        app.wait_for("Replying to")
            .expect("the composer's reply banner");
    }
}
