//! TL-PLAN: the production row mounted from the client's timeline projection.

use crate::support::{DESKTOP, PHONE};
use futures::{FutureExt, channel::oneshot};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use std::rc::Rc;
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRowContent, MessagesTimelineRowsInput, derive_messages_timeline_rows,
};
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_client::view_logic::session::TimelineEntry;
use t3_ui::runtime::Shared;
use t3_ui::screens::thread::rows::plan::{
    PlanContext, PlanRowsState, SavePlan, proposed_plan_timeline_row,
};

const AT: &str = "2026-11-14T12:00:00Z";
fn source() -> String {
    format!(
        "# Settings\n\n## Summary\n\nOpening body\n\n{}\nClosing body\n",
        (1..=22)
            .map(|n| format!("- Step {n}\n"))
            .collect::<String>()
    )
}
fn key() -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("remote-env".into()),
        thread: ThreadId("plan-thread".into()),
    }
}
fn projected(
    markdown: &str,
) -> Shared<t3_client::thread_views::messages_timeline::MessagesTimelineRow> {
    let entry: TimelineEntry = serde_json::from_value(json!({"id":"plan-row","kind":"proposed-plan","createdAt":AT,"proposedPlan":{"id":"plan","runId":null,"planMarkdown":markdown,"status":"completed","createdAt":AT,"updatedAt":AT}})).expect("canonical timeline entry");
    Shared(
        derive_messages_timeline_rows(&MessagesTimelineRowsInput {
            timeline_entries: vec![Rc::new(entry)],
            ..Default::default()
        })
        .into_iter()
        .find(|row| matches!(row.content, MessagesTimelineRowContent::ProposedPlan { .. }))
        .expect("client plan row"),
    )
}
fn control(label: &'static str, action: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(label.to_owned())
        .on_press(action)
        .child(text(label.to_owned()))
}
#[component]
fn PlanFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let visible = signal(true);
    let allowed = signal(true);
    let find = signal(false);
    let requests = signal(Vec::<SavePlan>::new());
    let failed = signal(true);
    let (release, waiting) = oneshot::channel::<()>();
    let release = stored(Some(release));
    let waiting = stored(Some(waiting));
    let state = PlanRowsState::default();
    let row = computed(|| Some(projected(&source())));
    let mut context = PlanContext::new(key(), state);
    context.workspace_root = computed(|| Some("C:/remote/workspace".into()));
    context.can_write = computed(move || allowed.get());
    context.find_active = computed(move || find.get());
    context.write_file = Some(Rc::new(move |request| {
        assert_eq!(request.thread, key());
        assert_eq!(request.cwd, "C:/remote/workspace");
        assert_eq!(request.contents, source());
        assert_eq!(request.relative_path, "plans/settings.md");
        requests.update(|requests| requests.push(request));
        let gate = waiting.with(Option::take);
        async move {
            if let Some(gate) = gate {
                gate.await.expect("release service");
            }
            if failed.get_untracked() {
                Err("Remote write failed: Settings".into())
            } else {
                Ok(Some("plans/settings.md".into()))
            }
        }
        .boxed_local()
    }));
    on_key("ctrl+r", move |_| {
        if let Some(release) = release.with(Option::take) {
            release.send(()).expect("release");
        }
    });
    on_key("ctrl+s", move |_| failed.set(false));
    on_key("ctrl+p", move |_| allowed.update(|value| *value = !*value));
    on_key("ctrl+d", move |_| visible.update(|value| *value = !*value));
    view()
        .w_full()
        .h_full()
        .child(control("Recycle", move || visible.update(|v| *v = !*v)))
        .child(control("Permission", move || allowed.update(|v| *v = !*v)))
        .child(control("Find", move || find.update(|v| *v = !*v)))
        .child(control("Release", move || {
            if let Some(release) = release.with(Option::take) {
                release.send(()).expect("release");
            }
        }))
        .child(control("Succeed", move || failed.set(false)))
        .child(text(move || format!("writes:{}", requests.with(Vec::len))))
        .child(dynamic(move || {
            visible
                .get()
                .then(|| untracked(|| proposed_plan_timeline_row(row, context.clone())))
        }))
}
// The integrator owns timeline.rs. This dedicated root calls the exact production mount;
// the fixture supplies a client projection and service result, never card presentation.
fn start(window: (f32, f32)) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<PlanFixture>())
    })
    .expect("start plan fixture");
    app.first_render().expect("render");
    app
}
#[test]
fn the_client_plan_expands_collapses_and_retains_expansion_when_recycled() {
    for size in [DESKTOP, PHONE] {
        let mut app = start(size);
        assert!(
            !app.screen().layout_nodes().iter().any(|node| node
                .text
                .as_ref()
                .is_some_and(|text| text.text.text.contains("Closing body"))),
            "the collapsed preview is bounded"
        );
        app.press("role:button[Expand plan]").expect("expand");
        app.wait_for("Closing body").expect("complete body");
        app.press("role:button[Recycle]").expect("unmount");
        app.press("role:button[Recycle]").expect("remount");
        app.press("role:button[Collapse plan]")
            .expect("state retained above the cell");
        app.press("role:button[Find]")
            .expect("find mounts full text");
        assert!(
            !app.screen().shows("Closing body"),
            "find mounts content without expanding the clipped plan"
        );
        assert!(
            app.screen().layout_nodes().iter().any(|node| node
                .text
                .as_ref()
                .is_some_and(|text| text.text.text.contains("Closing body"))),
            "find must mount the clipped source to count hidden matches"
        );
    }
}
#[test]
fn copy_exports_original_markdown_including_the_heading_with_one_final_newline() {
    let mut app = start(DESKTOP);
    app.press("role:button[Plan actions]").expect("menu");
    app.press("role:menuitem[Copy to clipboard]").expect("copy");
    assert_eq!(app.screen().clipboard(), Some(source().as_str()));
    app.press("role:button[Plan actions]").expect("reopen");
    assert!(
        app.press("role:menuitem[Download as markdown]").is_err(),
        "no download capability must stay inert"
    );
}
#[test]
fn saving_guards_concurrency_rechecks_permission_and_preserves_a_failed_form_for_retry() {
    let mut app = start(DESKTOP);
    app.press("role:button[Plan actions]").expect("menu");
    app.press("role:menuitem[Save to workspace]")
        .expect("save form");
    app.clear("label:Workspace path")
        .expect("replace default filename");
    app.type_text("label:Workspace path", "plans/settings.md")
        .expect("path");
    app.press_key("ctrl+d").expect("recycle the plan cell");
    app.press_key("ctrl+d").expect("remount the plan cell");
    assert_eq!(
        app.screen()
            .input_text("label:Workspace path")
            .expect("path draft"),
        Some("plans/settings.md"),
        "the draft belongs to the plan above recycled cells"
    );
    let input = app.screen().id("label:Workspace path").expect("path input");
    app.press_key("ctrl+p")
        .expect("permission revoked while open");
    assert_eq!(
        app.screen()
            .id("label:Workspace path")
            .expect("same path input"),
        input,
        "permission changes must not rebuild the form or lose its draft"
    );
    assert!(app.press("role:button[Save]").is_err());
    assert!(app.screen().shows("writes:0"));
    app.press_key("ctrl+p").expect("permission restored");
    app.press("role:button[Save]").expect("submit");
    app.wait_for("text:writes:1").expect("one service call");
    assert!(app.press("role:button[Save]").is_err());
    assert!(app.press("role:button[Cancel]").is_err());
    app.press_key("ctrl+r")
        .expect("service completes with failure");
    app.wait_for("Save plan to workspace")
        .expect("failed form remains open");
    app.press_key("ctrl+s").expect("service recovers");
    app.press("role:button[Save]").expect("retry");
    app.wait_for("text:writes:2")
        .expect("retry reached the service");
}

fn code_value() -> String {
    (0..85)
        .map(|line| format!("let line_{line} = {line};"))
        .collect::<Vec<_>>()
        .join("\n")
}
#[component]
fn CodePlanFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    provide_context(t3_ui::theme::text_view_parts());
    let row = computed(|| {
        Some(projected(&format!(
            "# Code plan\n\n```rust title=example.rs\n{}\n```",
            code_value()
        )))
    });
    proposed_plan_timeline_row(row, PlanContext::new(key(), PlanRowsState::default()))
}
#[test]
fn code_copy_collects_every_chunk_in_the_fence() {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(DESKTOP.0), px(DESKTOP.1)),
        ..EngineConfig::new(Root::of::<CodePlanFixture>())
    })
    .expect("start code plan");
    app.first_render().expect("render");
    app.press("role:button[Expand plan]")
        .expect("expand full fence");
    app.press("role:button[Copy code]")
        .expect("copy complete fence");
    assert_eq!(app.screen().clipboard(), Some(code_value().as_str()));
}
