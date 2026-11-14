//! TL-ROWS-ASSISTANT: client-derived rows through the component's production entry points.

use crate::support::{DESKTOP, PHONE};
use futures::{FutureExt, channel::oneshot};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use std::rc::Rc;
use std::time::{Duration, SystemTime};
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRowContent, MessagesTimelineRowsInput, SentMessageEntry, TimelineLatestRun,
    derive_messages_timeline_rows,
};
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_client::view_logic::session::TimelineEntry;
use t3_ui::i18n::t;
use t3_ui::runtime::Shared;
use t3_ui::screens::thread::rows::assistant::{self, AssistantRowContext, AssistantRowsState};
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
            sent_messages: if sent.get() {
                vec![SentMessageEntry {
                    id: "sent".into(),
                    created_at: AT.into(),
                    run_id: Some("run".into()),
                    text: "[Across fragments][ref]\n\n[ref]: https://example.test".into(),
                    before_entry_id: None,
                }]
            } else {
                vec![]
            },
            ..Default::default()
        })
    });
    let projected = computed(move || {
        rows.with(|rows| rows.iter().find(|row| matches!(&row.content, MessagesTimelineRowContent::Message { message, .. } if message.role == "assistant")).cloned().map(Shared))
    });
    let row = assistant::assistant_row(projected);
    let state = AssistantRowsState::default();
    let fork_count = signal(0);
    let reply_count = signal(0);
    let fail = signal(false);
    let visible = signal(true);
    let (release, waiting) = oneshot::channel::<()>();
    let release = stored(Some(release));
    let waiting = stored(Some(waiting));
    let mut context = AssistantRowContext::for_timeline(state);
    context.timestamp_format = t3_contracts::settings::TimestampFormat::V24Hour;
    context.thread_ref = Some(ThreadKey {
        environment: EnvironmentId("environment".into()),
        thread: ThreadId("conversation".into()),
    });
    context.markdown_cwd = Some("C:/project".into());
    context.on_fork_from_run = Some(Rc::new(move |request| {
        assert_eq!(
            serde_json::to_value(request.source_thread_id).expect("source id"),
            json!("source-thread")
        );
        assert_eq!(
            serde_json::to_value(request.run_id).expect("run id"),
            json!("run")
        );
        fork_count.update(|count| *count += 1);
        let waiting = waiting.with(Option::take);
        let fail = fail.get();
        async move {
            if let Some(waiting) = waiting {
                waiting.await.map_err(|e| e.to_string())?;
            }
            if fail {
                Err("Permission denied".into())
            } else {
                Ok(())
            }
        }
        .boxed_local()
    }));
    context.on_reply_to_response = Some(Rc::new(move |request| {
        assert_eq!(request.thread_ref.thread.0, "conversation");
        assert_eq!(request.message.text, FINAL);
        assert_eq!(
            serde_json::to_value(request.projected_item.source_item_id.clone())
                .expect("source item"),
            json!("source-item")
        );
        reply_count.update(|count| *count += 1);
        Ok(())
    }));
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
                .child(text(move || format!("Replies: {}", reply_count.get())))
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
        app.press("role:button[Copy message]").expect("copy");
        app.wait_for("Copied!").expect("source anchored copy toast");
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
fn concrete_fork_and_reply_controls_keep_source_identity_and_busy_state_through_recycling() {
    let mut app = start(PHONE);
    app.wait_for("Working draft").expect("mounted");
    app.press("role:button[Finish]").expect("finish");
    app.wait_for("role:button[Fork from this response]")
        .expect("concrete fork control");
    app.press("role:button[Fork from this response]")
        .expect("fork");
    app.wait_for("Forks: 1").expect("source operation");
    app.press("role:button[Reply to this response]")
        .expect("concrete reply control");
    app.wait_for("Replies: 1").expect("source response context");
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
fn sent_message_uses_the_complete_client_source_for_reference_links() {
    let mut app = start(PHONE);
    app.wait_for("Working draft").expect("mounted");
    app.press("role:button[Sent message]")
        .expect("client sent message");
    app.wait_for("Across fragments")
        .expect("standalone Markdown resolves its reference definition");
}
