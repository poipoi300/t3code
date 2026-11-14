//! Work disclosures, tested independently of the integrator-owned timeline dispatcher.

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use std::rc::Rc;
use t3_client::view_logic::session::WorkLogEntry;
use t3_contracts::orchestration_v2::OrchestrationV2ProjectedTurnItem;
use t3_ui::i18n::{Value, t, tr};
use t3_ui::runtime::Shared;
use t3_ui::screens::thread::rows::work::{self, WorkEntryRow, WorkRowContext, WorkRowState};

fn projected(kind: &str) -> Rc<OrchestrationV2ProjectedTurnItem> {
    let mut item = json!({
        "id": kind, "threadId": "thread", "runId": "run", "nodeId": null,
        "providerThreadId": null, "providerTurnId": null, "nativeItemRef": null,
        "parentItemId": null, "ordinal": 1, "title": null, "startedAt": null,
        "completedAt": null, "updatedAt": "2026-11-13T12:00:00.000Z",
        "type": "command_execution", "status": "completed", "input": "vp test run",
        "output": null, "exitCode": null, "outputOmitted": true
    });
    if kind == "limit" || kind == "failure" {
        item["type"] = json!("error");
        item["status"] = json!("failed");
        item["failure"] = json!({
            "class": if kind == "limit" { "usage_limit" } else { "provider_error" },
            "message": "Provider failure detail", "code": null, "retryable": true,
            "resetAt": "2026-11-14T15:30:00.000Z"
        });
    }
    Rc::new(
        serde_json::from_value(json!({
            "position": 0, "visibility": "local", "sourceThreadId": "thread",
            "sourceItemId": kind, "item": item
        }))
        .expect("canonical projected item"),
    )
}

fn row(id: &str) -> WorkEntryRow {
    let mut entry = WorkLogEntry {
        id: id.into(),
        created_at: "2026-11-13T12:00:00.000Z".into(),
        tone: "tool".into(),
        label: "vp test run".into(),
        command: Some("vp test run".into()),
        detail: Some("42 tests passed".into()),
        ..Default::default()
    };
    let mut label = "vp test run";
    match id {
        "reasoning" => {
            entry.item_type = Some("reasoning".into());
            entry.tone = "thinking".into();
            entry.detail = Some("Full reasoning trace".into());
            entry.tool_lifecycle_status = Some("inProgress".into());
            label = "Reasoning preview";
        }
        "omitted" => {
            entry.projected_item = Some(projected(id));
            label = "Read package.json";
        }
        "limit" | "failure" => {
            entry.projected_item = Some(projected(id));
            entry.label = "Provider error".into();
            entry.item_type = Some("error".into());
        }
        "image" => {
            entry
                .presentation
                .insert("viewedImagePath".into(), json!("/repo/one.png"));
            label = "Viewed image";
        }
        _ => {}
    }
    let mut row = WorkEntryRow::new(Shared(Rc::new(entry)), label.into());
    row.compact_reasoning = Some("Reasoning preview".into());
    row.retry_workspace_preparation = id == "failure";
    if id == "omitted" {
        row.plain_output = Some(None);
    }
    if id == "image" {
        row.viewed_images = Rc::new(vec!["/repo/one.png".into(), "/repo/two.png".into()]);
    }
    row
}

#[component]
fn WorkFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let selected = signal("command");
    let mounted = signal(true);
    let active = signal(true);
    let zone = signal(false);
    let collapsed = signal(true);
    let actions = signal(0);
    let state = WorkRowState::default();
    let mut context = WorkRowContext::new(state);
    context.on_toggle_entry = Some(Rc::new(move |_, next| collapsed.set(next)));
    context.markdown = Some(Rc::new(|row| {
        text(t(row.entry.detail.as_deref().unwrap_or_default())).into_element()
    }));
    context.reset_time = Some(Rc::new(move |_| {
        Ok(if zone.get() {
            "10:30 (America/New_York)"
        } else {
            "16:30 (Europe/Madrid)"
        }
        .into())
    }));
    context.image = Some(Rc::new(|row| {
        text(t(&row.viewed_images.join(" → "))).into_element()
    }));
    context.trailing_thread_links = Some(Rc::new(move |_| {
        nsh_base::button()
            .accessibility_label(Some(SharedString::from(t("Nested action"))))
            .on_press(move || actions.update(|count| *count += 1))
            .child(text(t("Nested action")))
            .into_element()
    }));
    let mut root = view().w_full().h_full();
    for (label, id) in [
        ("Command example", "command"),
        ("Reasoning example", "reasoning"),
        ("Omitted example", "omitted"),
        ("Limit example", "limit"),
        ("Failure example", "failure"),
        ("Image example", "image"),
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
            .accessibility_label(Some(SharedString::from(t("Recycle cell"))))
            .on_press(move || mounted.update(|value| *value = !*value))
            .child(text(t("Recycle cell"))),
    )
    .child(
        nsh_base::button()
            .accessibility_label(Some(SharedString::from(t("Finish run"))))
            .on_press(move || active.set(false))
            .child(text(t("Finish run"))),
    )
    .child(
        nsh_base::button()
            .accessibility_label(Some(SharedString::from(t("Change timezone"))))
            .on_press(move || zone.update(|value| *value = !*value))
            .child(text(t("Change timezone"))),
    )
    .child(text(move || {
        SharedString::from(tr(
            "Collapsed: {value}",
            &[("value", Value::from(collapsed.get().to_string()))],
        ))
    }))
    .child(text(move || {
        SharedString::from(tr(
            "Actions: {count}",
            &[("count", Value::Int(actions.get()))],
        ))
    }))
    .child(when(
        move || mounted.get(),
        move || {
            work::simple_work_entry_row(
                computed(move || Some(row(selected.get()))),
                context.clone(),
            )
        },
    ))
    .child(when(move || active.get(), work::working_row))
}

fn fixture(window: (f32, f32)) -> Headless {
    let mut sets = t3_ui::registries();
    let mut fixtures = Registry::new();
    fixtures.register::<WorkFixture>();
    sets.push(fixtures);
    let mut app = Headless::start(EngineConfig {
        ui_sets: sets,
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<WorkFixture>())
    })
    .expect("work row fixture");
    app.wait_for("Command example").expect("fixture mounted");
    app
}

#[test]
fn disclosure_survives_recycling_and_nested_actions_do_not_toggle_it() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        app.wait_for("vp test run").expect("command");
        assert!(!app.screen().shows("42 tests passed"));
        app.press("id:work-entry-command").expect("expand command");
        app.wait_for("42 tests passed").expect("expanded body");
        app.wait_for("Collapsed: false")
            .expect("callback uses collapsed polarity");
        app.press("role:button[Nested action]")
            .expect("child action");
        app.wait_for("Actions: 1").expect("action ran");
        assert!(app.screen().shows("42 tests passed"));
        app.press("role:button[Recycle cell]").expect("unmount");
        assert!(!app.screen().shows("42 tests passed"));
        app.press("role:button[Recycle cell]").expect("remount");
        app.wait_for("42 tests passed")
            .expect("entry remains expanded");
        app.press("role:button[Reasoning example]")
            .expect("reuse cell");
        assert!(!app.screen().shows("Full reasoning trace"));
        app.press("id:work-entry-reasoning")
            .expect("expand reasoning");
        app.wait_for("Full reasoning trace")
            .expect("markdown dependency");
        app.wait_for("Thinking").expect("live heading");
        app.press("role:button[Command example]").expect("return");
        app.wait_for("42 tests passed")
            .expect("independent keyed state");
    }
}

#[test]
fn omitted_output_expands_to_an_honest_unavailable_state() {
    let mut app = fixture(DESKTOP);
    app.press("role:button[Omitted example]")
        .expect("read example");
    app.press("id:work-entry-omitted")
        .expect("expand omitted output");
    app.wait_for("Tool output retrieval is unavailable.")
        .expect("no unresolving loading state");
    assert!(!app.screen().shows("42 tests passed"));
}

#[test]
fn usage_reset_uses_the_current_named_zone_and_retry_without_a_service_is_inert() {
    let mut app = fixture(DESKTOP);
    app.press("role:button[Limit example]").expect("limit");
    app.wait_for("Usage limit reached. Retry after 16:30 (Europe/Madrid).")
        .expect("named zone");
    assert!(!app.screen().shows("Provider failure detail"));
    app.press("role:button[Change timezone]")
        .expect("new viewer zone");
    app.press("role:button[Command example]")
        .expect("recycle notice");
    app.press("role:button[Limit example]").expect("new notice");
    app.wait_for("Usage limit reached. Retry after 10:30 (America/New_York).")
        .expect("fresh zone");
    app.press("role:button[Failure example]")
        .expect("provider error");
    app.wait_for("Provider failure detail")
        .expect("failure detail retained");
    app.wait_for("Workspace preparation retry is unavailable.")
        .expect("unsupported retry");
    assert!(
        app.press("role:button[Retry]").is_err(),
        "disabled retry is inert"
    );
}

#[test]
fn expanded_image_receives_the_entire_groups_ordered_gallery() {
    let mut app = fixture(PHONE);
    app.press("role:button[Image example]")
        .expect("image entry");
    app.press("id:work-entry-image").expect("expand image");
    app.wait_for("/repo/one.png → /repo/two.png")
        .expect("group gallery reaches media owner");
}

#[test]
fn working_indicator_disappears_when_the_run_finishes() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        app.wait_for("Working...").expect("active run indicator");
        app.press("role:button[Finish run]").expect("settle run");
        assert!(!app.screen().shows("Working..."));
    }
}
