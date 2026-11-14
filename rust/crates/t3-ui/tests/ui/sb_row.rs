//! SB-ROW: canonical shell state in the shared row, plus the real demo-backed app.

use std::rc::Rc;

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_contracts::orchestration_v2::OrchestrationV2ThreadShell;
use t3_ui::runtime::Shared;
use t3_ui::screens::sidebar::{
    RowVariant, SidebarItem,
    row::{RowExtras, sidebar_thread_row},
};
use t3_ui::state::nav::{Navigator, Route};

use crate::support::{DESKTOP, PHONE, pair, start};

fn source(status: &str) -> OrchestrationV2ThreadShell {
    serde_json::from_value(json!({
        "id":"thread", "projectId":"project", "title":"Port the row",
        "providerInstanceId":"codex", "modelSelection":{"instanceId":"codex","model":"gpt-5.4"},
        "runtimeMode":"full-access", "interactionMode":"default", "branch":"feature/shared-row","worktreePath":null,
        "lineage":{"rootThreadId":"thread","parentThreadId":null,"relationshipToParent":null},
        "forkedFrom":null,"activeProviderThreadId":null,"latestRunId":"run","activeRunId":null,
        "status":status,"pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":null,
        "latestRunCompletedAt":if status=="completed" { Some("2026-11-13T11:00:00.000Z") } else {None},
        "lastVisitedAt":"2026-11-13T10:00:00.000Z",
        "hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,
        "createdBy":"user","creationSource":"web","createdAt":"2026-11-13T10:00:00.000Z",
        "updatedAt":"2026-11-13T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"deletedAt":null
    })).expect("canonical shell fixture")
}

#[component]
fn RowFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let source = signal(source("running"));
    let nav = Navigator::new(Route::NewThread);
    let row = computed(move || Some(Shared(Rc::new(source.get()))));
    let item = computed(|| {
        Some(SidebarItem {
            key: ThreadKey {
                environment: EnvironmentId("host".into()),
                thread: ThreadId("thread".into()),
            },
            title: "Port the row".into(),
            project: "t3code".into(),
            activity_at: Some("2026-11-13T12:00:00.000Z".into()),
            variant: RowVariant::Card,
        })
    });
    view()
        .w_full()
        .h_full()
        .child(sidebar_thread_row(
            item,
            row,
            computed(|| None),
            computed(RowExtras::default),
            nav,
        ))
        .child(
            nsh_base::button()
                .accessibility_label(Some("Complete run".into()))
                .child(text("Complete run"))
                .on_press(move || source.set(self::source("completed"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Mark read".into()))
                .child(text("Mark read"))
                .on_press(move || {
                    source.update(|source| {
                        source.last_visited_at = Some(Some(t3_contracts::wire::WireDateTime(
                            "2026-11-13T12:00:00.000Z".into(),
                        )))
                    })
                }),
        )
        .child(text(move || SharedString::from(nav.top().path())))
}

fn fixture(window: (f32, f32)) -> Headless {
    Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<RowFixture>())
    })
    .expect("row fixture starts")
}

#[test]
fn live_shell_status_and_read_watermark_update_the_mounted_row_at_both_widths() {
    for window in [DESKTOP, PHONE] {
        let mut app = fixture(window);
        app.wait_for("role:button[Port the row, Working, t3code]")
            .expect("running status in accessible name");
        assert!(app.screen().shows("feature/shared-row"));
        app.press("role:button[Complete run]").expect("complete");
        app.wait_for("role:button[Port the row, Done, t3code]")
            .expect("unread completion");
        app.press("role:button[Mark read]").expect("mark read");
        app.wait_for("role:button[Port the row, t3code]")
            .expect("completion acknowledged");
        assert!(!app.screen().shows("text:Done"));
        assert!(
            app.press("role:button[Settle thread]").is_err(),
            "unavailable mutations cannot run"
        );
        app.press("role:button[Port the row, t3code]")
            .expect("open");
        app.wait_for("/host/thread")
            .expect("destination is scoped to this environment");
    }
}

#[test]
fn the_real_app_opens_a_row_and_retains_the_desktop_sidebar() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window);
        pair(&mut app);
        app.press("role:button[Build the shared Rust UI slice, t3code]")
            .expect("row opens");
        app.wait_for("The project has a single math module. I will add a clamp helper beside sum.")
            .expect("thread conversation");
        if window == DESKTOP {
            assert!(app.screen().shows("Rich composer: chips, IME and paste"));
        } else {
            app.press("role:button[Back]").expect("reverse navigation");
            app.wait_for("Rich composer: chips, IME and paste")
                .expect("list returns");
        }
    }
}
