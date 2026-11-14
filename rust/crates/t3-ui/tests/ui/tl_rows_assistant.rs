//! TL-ROWS-ASSISTANT: real NSH input against the row components, independent of frozen routing.

use std::rc::Rc;
use std::time::Duration;

use futures::{FutureExt, channel::oneshot};
use nsh_base::text_view;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_client::view_logic::session::ChatMessage;
use t3_contracts::orchestration_v2::OrchestrationV2ProjectedTurnItem;
use t3_ui::i18n::t;
use t3_ui::screens::thread::rows::assistant::{
    self, AssistantRow, AssistantRowContext, AssistantRowsState,
};

use crate::support::{DESKTOP, PHONE};

fn message(text: &str, streaming: bool, status: &str) -> AssistantRow {
    let at = "2026-11-13T00:00:00Z";
    let message: ChatMessage = serde_json::from_value(json!({
        "id":"answer", "role":"assistant", "text":text, "streaming":streaming,
        "runId":"run", "createdAt":at, "updatedAt":at
    }))
    .expect("message fixture");
    let projected: OrchestrationV2ProjectedTurnItem = serde_json::from_value(json!({
        "position":0,"visibility":"inherited","sourceThreadId":"source-thread","sourceItemId":"source-item",
        "item": {
            "id":"source-item","threadId":"source-thread","type":"assistant_message",
            "messageId":"answer","text":text,"streaming":streaming,"runId":"run",
            "nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,
            "parentItemId":null,"ordinal":0,"status":status,"title":null,
            "startedAt":at,"completedAt":null,"updatedAt":at
        }
    })).expect("projected fixture");
    AssistantRow {
        id: "row-answer".into(),
        message: Rc::new(message),
        projected_item: Some(Rc::new(projected)),
        suppress_empty_response: false,
        show_assistant_meta: true,
        show_assistant_copy_button: true,
        assistant_copy_streaming: streaming,
        assistant_turn_diff_summary: None,
    }
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
    let value = signal(message("", true, "running"));
    let row = computed(move || Some(value.get()));
    let state = AssistantRowsState::default();
    let fork_count = signal(0);
    let reply_count = signal(0);
    let markdown_mounts = signal(0);
    let failure = signal(false);
    let (release, waiting) = oneshot::channel::<()>();
    let waiting = stored(Some(waiting));
    let release = stored(Some(release));
    let mut context = AssistantRowContext::new(
        Rc::new(move |markdown| {
            markdown_mounts.update(|count| *count += 1);
            view()
                .child(text_view().markdown(move || Some(markdown.get().text.into())))
                .child(text(move || {
                    format!("Streaming: {}", markdown.get().is_streaming)
                }))
                .child(text(move || {
                    format!(
                        "Directories: {}",
                        markdown
                            .get()
                            .agent_directories
                            .as_ref()
                            .map(|lookup| lookup.resolve().join(","))
                            .unwrap_or_default()
                    )
                }))
                .child(text(move || {
                    format!("Insights: {}", markdown.get().render_insights)
                }))
                .child(text(move || {
                    format!("Breaks: {}", markdown.get().line_breaks)
                }))
                .into_element()
        }),
        state,
    );
    context.thread_ref = Some(ThreadKey {
        environment: EnvironmentId("environment".into()),
        thread: ThreadId("conversation".into()),
    });
    context.markdown_cwd = Some("C:/project".into());
    context.agent_directories = Some(Rc::new(|item| {
        assert_eq!(
            serde_json::to_value(&item.source_thread_id).expect("source id"),
            json!("source-thread")
        );
        vec!["C:/earlier-checkout".into()]
    }));
    context.copy_text = Some(Rc::new(str::to_owned));
    context.action_control = Some(Rc::new(|control| {
        let disabled = control.disabled;
        let on_press = control.on_press;
        nsh_base::button()
            .accessibility_label(Some(t(control.label).into()))
            .disabled({let disabled = disabled.clone(); move || disabled()})
            .on_press(move || { if !disabled() { on_press(); } })
            .w(px(26.0)).h(px(24.0))
            .child(svg(nsh_core::element::ImageSrc::svg_markup(match control.label {
                "Fork from this response" => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="18" r="3"/><circle cx="6" cy="6" r="3"/><circle cx="18" cy="6" r="3"/><path d="M18 9a6 6 0 0 1-6 6 6 6 0 0 1-6-6"/></svg>"#,
                _ => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m9 17-5-5 5-5"/><path d="M20 18v-2a4 4 0 0 0-4-4H4"/></svg>"#,
            })).w(px(12.0)).h(px(12.0)).accessible(false))
            .into_element()
    }));
    context.timestamp = Some(Rc::new(|_| {
        ("12:34".into(), "12:34, 13th November 2026".into())
    }));
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
        let fails = failure.get();
        async move {
            if let Some(waiting) = waiting {
                waiting.await.map_err(|error| error.to_string())?;
            }
            if fails {
                Err("Permission denied".into())
            } else {
                Ok(())
            }
        }
        .boxed_local()
    }));
    context.on_reply_to_response = Some(Rc::new(move |request| {
        assert_eq!(request.thread_ref.thread.0, "conversation");
        assert_eq!(
            request.message.text,
            "**Final answer**\n\nA second paragraph."
        );
        assert_eq!(
            serde_json::to_value(request.projected_item.source_item_id.clone()).expect("item id"),
            json!("source-item")
        );
        reply_count.update(|count| *count += 1);
        Ok(())
    }));
    let row_visible = signal(true);
    let body_context = context.clone();
    let sent_context = context.clone();
    let sent = signal(false);
    view().w_full().h_full().child(
        view()
            .app_class(t3_ui::theme::tw(
                "group/assistant w-full px-3 sm:px-12 gap-2",
            ))
            .child(control("Finish", move || {
                value.set(message(
                    "**Final answer**\n\nA second paragraph.",
                    false,
                    "completed",
                ))
            }))
            .child(control("Interrupt", move || {
                let mut next = message("Partial answer", false, "interrupted");
                next.show_assistant_meta = false;
                value.set(next);
            }))
            .child(control("Empty", move || {
                value.set(message("", false, "completed"))
            }))
            .child(control("Suppress empty", move || {
                value.update(|row| row.suppress_empty_response = true)
            }))
            .child(control("Reveal empty", move || {
                value.update(|row| row.suppress_empty_response = false)
            }))
            .child(control("Hide row", move || row_visible.set(false)))
            .child(control("Show row", move || row_visible.set(true)))
            .child(control("Release fork", move || {
                if let Some(release) = release.with(Option::take) {
                    let _ = release.send(());
                }
            }))
            .child(control("Fail forks", move || failure.set(true)))
            .child(control("Sent message", move || sent.set(true)))
            .child(text(move || format!("Forks: {}", fork_count.get())))
            .child(text(move || format!("Replies: {}", reply_count.get())))
            .child(text(move || {
                format!("Markdown mounts: {}", markdown_mounts.get())
            }))
            .child(dynamic(move || {
                row_visible.get().then(|| {
                    untracked(|| assistant::assistant_timeline_row(row, body_context.clone()))
                })
            }))
            .child(dynamic(move || {
                sent.get().then(|| {
                    untracked(|| {
                        let content = computed(|| {
                            "[Across fragments][ref]\n\n[ref]: https://example.test".to_owned()
                        });
                        assistant::sent_message_timeline_row(content, sent_context.clone())
                    })
                })
            })),
    )
}

#[component]
fn UnsupportedCopyFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let row = computed(|| Some(message("**Unchanged clipboard**", false, "completed")));
    let context = AssistantRowContext::new(
        Rc::new(|markdown| {
            text_view()
                .markdown(move || Some(markdown.get().text.into()))
                .into_element()
        }),
        AssistantRowsState::default(),
    );
    assistant::assistant_meta_timeline_row(row, context)
}

fn start(window: (f32, f32)) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<AssistantFixture>())
    };
    if window == DESKTOP {
        config.environment.os = nsh_core::platform::Os::Windows;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    Headless::start(config).expect("assistant fixture")
}

#[test]
fn streaming_settlement_updates_copy_and_timestamp_at_both_widths() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window);
        app.wait_for("Streaming: true").expect("streaming");
        assert!(!app.screen().shows("(empty response)"));
        assert!(!app.screen().shows("role:button[Copy message]"));
        assert!(!app.screen().shows("12:34"));
        app.press("role:button[Finish]").expect("finish");
        app.wait_for("Streaming: false").expect("settled");
        assert!(
            app.screen().shows("Markdown mounts: 1"),
            "streaming must update the existing markdown binding"
        );
        if window == DESKTOP {
            app.hover("role:button[Copy message]")
                .expect("reveal actions");
        }
        app.wait_for("12:34").expect("timestamp");
        app.press("role:button[Copy message]").expect("copy");
        app.wait_until(
            "clipboard contains the full source",
            Duration::from_secs(2),
            |screen| screen.clipboard() == Some("**Final answer**\n\nA second paragraph."),
        )
        .expect("copied complete source");
        assert!(
            !app.screen().shows("Copied!"),
            "unacknowledged writes cannot announce success"
        );
    }
}

#[test]
fn empty_response_suppression_and_interruption_update_without_remounting_the_row() {
    let mut app = start(PHONE);
    app.wait_for("Streaming: true").expect("mounted");
    app.press("role:button[Empty]").expect("empty");
    app.wait_for("(empty response)")
        .expect("settled empty fallback");
    app.press("role:button[Suppress empty]").expect("suppress");
    app.wait_until("fallback is suppressed", Duration::from_secs(2), |screen| {
        !screen.shows("(empty response)")
    })
    .expect("suppressed");
    app.press("role:button[Reveal empty]").expect("reveal");
    app.wait_for("(empty response)").expect("fallback restored");
    app.press("role:button[Interrupt]").expect("interrupt");
    app.wait_for("interrupted")
        .expect("badge remains when meta is absent");
    assert!(!app.screen().shows("role:button[Copy message]"));
}

#[test]
fn fork_uses_the_source_thread_and_remains_busy_across_cell_recycling_then_reports_failure() {
    let mut app = start(PHONE);
    app.wait_for("Streaming: true").expect("mounted");
    app.press("role:button[Finish]").expect("finish");
    app.wait_for("role:button[Fork from this response]")
        .expect("fork");
    app.press("role:button[Fork from this response]")
        .expect("fork");
    app.wait_for("Forks: 1").expect("invoked");
    app.press("role:button[Reply to this response]")
        .expect("reply while fork is pending");
    app.wait_for("Replies: 1")
        .expect("reply dispatched independently");
    assert!(
        !app.screen()
            .enabled("role:button[Fork from this response]")
            .expect("busy control")
    );
    app.press("role:button[Hide row]").expect("recycle");
    app.press("role:button[Show row]").expect("remount");
    app.wait_for("role:button[Fork from this response]")
        .expect("row restored");
    assert!(
        !app.screen()
            .enabled("role:button[Fork from this response]")
            .expect("still busy")
    );
    app.press("role:button[Release fork]").expect("resolve");
    app.wait_until(
        "fork is available again",
        Duration::from_secs(2),
        |screen| {
            screen
                .enabled("role:button[Fork from this response]")
                .unwrap_or(false)
        },
    )
    .expect("busy cleared");
    app.press("role:button[Fail forks]").expect("failure mode");
    app.press("role:button[Fork from this response]")
        .expect("retry fork");
    app.wait_for("Permission denied").expect("error is visible");
    assert!(
        app.screen()
            .enabled("role:button[Fork from this response]")
            .expect("failure clears busy")
    );
}

#[test]
fn reply_keeps_the_conversation_and_source_item_identity() {
    let mut app = start(PHONE);
    app.wait_for("Streaming: true").expect("mounted");
    assert!(!app.screen().shows("role:button[Reply to this response]"));
    app.press("role:button[Finish]").expect("finish");
    app.wait_for("role:button[Reply to this response]")
        .expect("reply action");
    app.press("role:button[Reply to this response]")
        .expect("reply");
    app.wait_for("Replies: 1")
        .expect("response context request");
}

#[test]
fn markdown_gets_pre_reply_directories_and_sent_messages_get_the_complete_source() {
    let mut app = start(PHONE);
    app.wait_for("Directories: C:/earlier-checkout")
        .expect("reply context passed");
    app.press("role:button[Sent message]")
        .expect("sent message");
    app.wait_for("Across fragments")
        .expect("standalone markdown, not raw link syntax");
    app.wait_for("Insights: false")
        .expect("sent message uses the source's distinct markdown options");
}

#[test]
fn unavailable_copy_formatter_is_disabled_and_does_not_change_the_clipboard() {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(PHONE.0), px(PHONE.1)),
        ..EngineConfig::new(Root::of::<UnsupportedCopyFixture>())
    })
    .expect("unsupported fixture");
    app.wait_for("role:button[Copy message]")
        .expect("copy affordance");
    app.set_clipboard(Some("existing text".into()));
    assert!(
        !app.screen()
            .enabled("role:button[Copy message]")
            .expect("disabled")
    );
    let _ = app.press("role:button[Copy message]");
    app.catch_up();
    assert_eq!(app.screen().clipboard(), Some("existing text"));
}
