//! Work entries from the real client projection through the production mount API.

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use std::rc::Rc;
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRowContent, MessagesTimelineRowsInput, derive_messages_timeline_rows,
};
use t3_client::view_logic::session::TimelineEntry;
use t3_ui::i18n::{Value, tr};
use t3_ui::runtime::Shared;
use t3_ui::screens::thread::rows::work::{self, WorkEntryRow, WorkRowContext, WorkRowState};

const AT: &str = "2026-11-13T12:34:00Z";

fn source_entry(id: &str, suffix: &str) -> Rc<TimelineEntry> {
    let mut work = json!({"id":id,"createdAt":AT,"runId":null,"label":"Ran command", "tone":"tool",
        "command":"vp test run", "detail":"42 tests passed", "itemType":"command_execution"});
    if id == "reasoning" {
        work["itemType"] = json!("reasoning");
        work["tone"] = json!("thinking");
        work["toolLifecycleStatus"] = json!("inProgress");
        work["detail"] = json!(format!(
            "**Reasoning heading**\n\nFull reasoning trace{suffix}"
        ));
        work.as_object_mut().expect("object").remove("command");
    }
    if id == "created" || id == "notified" {
        let mut item = json!({"id":id,"threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"title":"Child chat","startedAt":null,"completedAt":null,"updatedAt":AT,"type":"thread_created","status":"completed","targetThreadId":"created-child","targetRunId":null,"targetProviderInstanceId":"replay","targetModel":"replay"});
        if id == "notified" {
            item["type"] = json!("notification");
            item["source"] = json!({"kind":"subagent","childThreadId":"notified-child"});
            item["outcome"] = json!("completed");
            item["summary"] = json!("Child completed");
        }
        work["projectedItem"] = json!({"position":0,"visibility":"local","sourceThreadId":"thread","sourceItemId":id,"item":item});
    }
    if id == "custom" {
        work["toolLifecycleStatus"] = json!("failed");
        work["toolIcon"] = json!({"_tag":"themed-logo","logoUrl":"data:image/svg+xml,%3Csvg%20xmlns=%22http://www.w3.org/2000/svg%22/%3E"});
    }
    if id == "content" {
        work["command"] = json!("Copy");
        work["detail"] = json!("Settings");
    }
    if id == "omitted" || id == "limit" || id == "failure" || id == "workspace" {
        let mut item = json!({"id":id,"threadId":"thread","runId":null,"nodeId":null,
            "providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,
            "ordinal":1,"title":null,"startedAt":null,"completedAt":null,"updatedAt":AT,
            "type":"command_execution","status":"completed","input":"vp test run",
            "output":null,"exitCode":null,"outputOmitted":true});
        if id == "limit" || id == "failure" || id == "workspace" {
            item["type"] = json!("error");
            item["status"] = json!("failed");
            item["failure"] = json!({"class":if id == "limit" {"usage_limit"} else {"provider_error"},
                "message":"Settings", "code":null,"retryable":true,"resetAt":"2026-11-14T15:30:00Z"});
            if id == "workspace" {
                item["runId"] = json!("run");
                item["failure"]["code"] = json!("workspace_preparation_failed");
            }
            work["itemType"] = json!("error");
            work["label"] = json!("Provider error");
        }
        work["projectedItem"] = json!({"position":0,"visibility":"local","sourceThreadId":"thread","sourceItemId":id,"item":item});
    }
    Rc::new(
        serde_json::from_value(json!({"id":id,"createdAt":AT,"kind":"work","entry":work}))
            .expect("canonical timeline entry"),
    )
}

fn control(label: &'static str, id: &str, press: impl Fn() + 'static) -> impl IntoElement {
    let label = t3_ui::i18n::Text::from(label).resolve();
    nsh_base::button()
        .accessibility_label(Some(label.clone()))
        .test_id(id)
        .on_press(press)
        .child(text(label))
}

#[component]
fn WorkFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let nav = t3_ui::state::nav::Navigator::new(t3_ui::state::nav::Route::Thread(
        t3_client::types::ThreadKey {
            environment: t3_client::types::EnvironmentId("test-env".into()),
            thread: t3_client::types::ThreadId("thread".into()),
        },
    ));
    provide_context(nav);
    let selected = signal("command");
    let suffix = signal(String::new());
    let mounted = signal(true);
    let active = signal(true);
    let collapsed = signal(true);
    let rows = computed(move || {
        derive_messages_timeline_rows(&MessagesTimelineRowsInput {
            timeline_entries: vec![source_entry(selected.get(), &suffix.get())],
            tool_call_visibility: "showAll".into(),
            ..Default::default()
        })
    });
    let projected = computed(move || {
        rows.with(|rows| {
            rows.iter()
                .find(|row| {
                    matches!(
                        row.content,
                        MessagesTimelineRowContent::Work { .. }
                            | MessagesTimelineRowContent::WorkLive { .. }
                    )
                })
                .cloned()
                .map(Shared)
        })
    });
    let state = WorkRowState::default();
    let mut context = WorkRowContext::new(state);
    context.timestamp_format = computed(|| t3_contracts::settings::TimestampFormat::V24Hour);
    context.on_toggle_entry = Some(Rc::new(move |_, next| collapsed.set(next)));
    let mut root = view().w_full().h_full();
    for (label, id) in [
        ("Command example", "command"),
        ("Created example", "created"),
        ("Notified example", "notified"),
        ("Reasoning example", "reasoning"),
        ("Omitted example", "omitted"),
        ("Limit example", "limit"),
        ("Failure example", "failure"),
        ("Workspace failure example", "workspace"),
        ("Content example", "content"),
        ("Custom icon example", "custom"),
    ] {
        root = root.child(control(label, id, move || selected.set(id)));
    }
    root.child(text(move || {
        SharedString::from(format!("Route: {}", nav.top().path()))
    }))
    .child(control("Recycle cell", "recycle", move || {
        mounted.update(|value| *value = !*value)
    }))
    .child(control("Finish run", "finish", move || active.set(false)))
    .child(control("Append thought", "append", move || {
        suffix.update(|value| value.push_str(" appended"))
    }))
    .child(control("French", "french", || {
        t3_ui::i18n::set_language("fr", &["fr".into()])
    }))
    .child(text(move || {
        SharedString::from(tr(
            "Collapsed: {value}",
            &[("value", Value::from(collapsed.get().to_string()))],
        ))
    }))
    .child(when(
        move || mounted.get(),
        move || work::work_timeline_row(projected, context.clone()),
    ))
    .child(when(move || active.get(), work::working_row))
}

fn fixture(window: (f32, f32)) -> Headless {
    let mut sets = t3_ui::registries();
    let mut fixtures = Registry::new();
    fixtures.register::<WorkFixture>();
    sets.push(fixtures);
    let mut config = EngineConfig {
        ui_sets: sets,
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<WorkFixture>())
    };
    if window == DESKTOP {
        config.environment.os = nsh_core::platform::Os::Windows;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    let mut app = Headless::start(config).expect("fixture starts");
    app.wait_for("vp test run").expect("C3 command row mounted");
    app
}

#[test]
fn client_projected_disclosures_keep_keyed_state_after_recycling() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        assert!(!app.screen().shows("42 tests passed"));
        if window == DESKTOP {
            app.hover("id:work-entry-command").expect("timestamp hover");
            app.wait_for("text:/12:34$/")
                .expect("owned TimestampPresenter in the default UTC host");
        }
        app.press("id:work-entry-command").expect("expand");
        app.wait_for("42 tests passed")
            .expect("body from work entry");
        app.wait_for("Collapsed: false")
            .expect("collapsed polarity");
        app.press("id:recycle").expect("unmount");
        assert!(!app.screen().shows("42 tests passed"));
        app.press("id:recycle").expect("remount");
        app.wait_for("42 tests passed").expect("retained state");
        app.press("id:reasoning").expect("reuse cell");
        assert!(!app.screen().shows("Full reasoning trace"));
        app.press("id:work-entry-reasoning")
            .expect("expand reasoning");
        app.wait_for("Full reasoning trace")
            .expect("built-in ChatMarkdown");
        app.press("id:append").expect("stream delta");
        app.wait_for("Full reasoning trace appended")
            .expect("tail updates");
        app.press("id:command").expect("return");
        app.wait_for("42 tests passed")
            .expect("independent entry state");
    }
}

#[test]
fn deferred_inspector_and_output_do_not_add_copy_absent_from_the_web() {
    let mut app = fixture(DESKTOP);
    app.press("id:omitted").expect("omitted output");
    app.press("id:work-entry-omitted").expect("disclosure");
    assert!(!app.screen().shows("unavailable"));
    assert!(!app.screen().shows("Loading output"));
    app.press("id:work-entry-omitted").expect("collapse");
    app.wait_for("Collapsed: true")
        .expect("disclosure still works without U8");
}

#[test]
fn usage_reset_and_failure_details_use_owned_presenters_and_preserve_content() {
    let mut app = fixture(DESKTOP);
    app.press("id:limit").expect("limit notice");
    app.wait_for(r"text:/^Usage limit reached\. Retry after .*15:30 \(UTC\)\.$/")
        .expect("complete warning with reset time in the actual host zone");
    assert!(!app.screen().shows("Settings"));
    app.press("id:workspace")
        .expect("workspace preparation failure");
    app.wait_for("Settings").expect("workspace failure detail");
    assert!(
        !app.screen()
            .matches("role:button[Retry]")
            .expect("retry selector"),
        "the web hides Retry without a bound service"
    );
    app.press("id:failure").expect("failure");
    app.wait_for("Settings").expect("server detail");
    app.press("id:french").expect("locale switch");
    app.wait_for("Settings")
        .expect("server detail remains literal");
    assert!(!app.screen().shows("unavailable"));
    assert!(app.press("role:button[Retry]").is_err());
}

#[test]
fn runtime_labels_and_expanded_content_are_never_translation_keys() {
    let mut app = fixture(PHONE);
    app.press("id:content").expect("content command");
    app.press("id:french").expect("French");
    app.wait_for("Copy").expect("command content is not Copier");
    app.press("id:work-entry-content").expect("expand content");
    app.wait_for("Settings")
        .expect("body content is not Parametres");
}

#[test]
fn from_row_consumes_the_client_presentation_and_preserves_entry_identity() {
    let entry = source_entry("command", "");
    let rows = derive_messages_timeline_rows(&MessagesTimelineRowsInput {
        timeline_entries: vec![entry],
        ..Default::default()
    });
    let source = rows
        .iter()
        .find(|row| {
            matches!(
                row.content,
                MessagesTimelineRowContent::Work { .. }
                    | MessagesTimelineRowContent::WorkLive { .. }
            )
        })
        .expect("work row");
    let row = WorkEntryRow::from_row(source).expect("adapted row");
    assert_eq!(row.display_label.resolve().to_string(), "vp test run");
    let entry = match &source.content {
        MessagesTimelineRowContent::Work {
            grouped_entries, ..
        } => &grouped_entries[0],
        MessagesTimelineRowContent::WorkLive { entry, .. } => entry,
        _ => unreachable!(),
    };
    assert!(Rc::ptr_eq(entry, &row.entry.0));
}

#[test]
fn working_indicator_disappears_when_the_run_finishes() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        app.wait_for("Working...").expect("working");
        app.press("id:finish").expect("finish");
        assert!(!app.screen().shows("Working..."));
    }
}

#[test]
fn thread_actions_are_owned_and_do_not_toggle_the_work_disclosure() {
    let mut app = fixture(DESKTOP);
    app.press("id:created").expect("created entry");
    app.press("role:button[Open Child chat]")
        .expect("owned Open chat action");
    app.wait_for("text:Route: /test-env/created-child")
        .expect("scoped created thread route");
    assert!(app.screen().shows("Collapsed: true"));
    app.press("id:notified").expect("notification entry");
    app.press("role:button[Open subagent thread]")
        .expect("owned Open subagent action");
    app.wait_for("text:Route: /test-env/notified-child")
        .expect("scoped subagent route");
    assert!(app.screen().shows("Collapsed: true"));
}

#[test]
fn untintable_tool_icons_keep_the_separate_failure_marker() {
    let mut app = fixture(DESKTOP);
    app.press("id:custom").expect("custom tool icon");
    app.wait_for("id:work-tool-failure")
        .expect("web failure X beside untintable icon");
    app.wait_for("label:Tool call failed")
        .expect("web failure icon label");
    app.press("id:command")
        .expect("ordinary successful command");
    assert!(
        !app.screen()
            .matches("id:work-tool-failure")
            .expect("failure selector")
    );
}
