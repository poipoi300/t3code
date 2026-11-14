//! SB-ROW: canonical shell state in the shared row, plus the real demo-backed app.

use std::rc::Rc;

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_contracts::orchestration_v2::OrchestrationV2ThreadShell;
use t3_ui::runtime::{Rt, Shared, demo::DemoBackend, host::Choice};
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

fn config() -> t3_contracts::server::ServerConfig {
    let mut config: serde_json::Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/ServerConfig.jsonl")
            .lines()
            .next()
            .expect("config sample"),
    )
    .expect("config JSON");
    let mut provider: serde_json::Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/ServerProvider.jsonl")
            .lines()
            .next()
            .expect("provider sample"),
    )
    .expect("provider JSON");
    config["environment"]["environmentId"] = json!("host");
    provider["instanceId"] = json!("codex");
    provider["driver"] = json!("codex");
    config["providers"] = json!([provider]);
    serde_json::from_value(config).expect("canonical config")
}

#[component]
fn RowFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let store = t3_client::ClientStore::default();
    let session = store.begin_session(EnvironmentId("host".into()));
    provide_context(Rt::new(
        store.clone(),
        Rc::new(DemoBackend::new(store.clone())),
        Choice::Demo,
    ));
    let publishing = store.clone();
    let source = signal(source("running"));
    let nav = Navigator::new(Route::NewThread);
    let row = computed(move || Some(Shared(Rc::new(source.get()))));
    let variant = signal(RowVariant::Card);
    let extras = signal(RowExtras::default());
    let project = computed(|| {
        Some(Shared(Rc::new(serde_json::from_value(json!({
        "id":"project","title":"t3code","workspaceRoot":"/workspace/t3code","defaultModelSelection":null,
        "scripts":[],"createdAt":"2026-11-13T10:00:00.000Z","updatedAt":"2026-11-13T12:00:00.000Z"
    })).expect("canonical project"))))
    });
    let item = computed(move || {
        Some(SidebarItem {
            key: ThreadKey {
                environment: EnvironmentId("host".into()),
                thread: ThreadId("thread".into()),
            },
            title: "Port the row".into(),
            project: "t3code".into(),
            activity_at: Some("2026-11-13T12:00:00.000Z".into()),
            variant: variant.get(),
        })
    });
    view()
        .w_full()
        .h_full()
        .child(sidebar_thread_row(
            item,
            row,
            project,
            computed(move || extras.get()),
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
        .child(
            nsh_base::button()
                .accessibility_label(Some("Show slim wake".into()))
                .child(text("Show slim wake"))
                .on_press(move || {
                    variant.set(RowVariant::Slim);
                    extras
                        .update(|extras| extras.woke_at = Some("2026-11-13T11:00:00.000Z".into()));
                }),
        )
        .child(text(move || {
            SharedString::from(if extras.with(|extras| extras.woke_at.is_some()) {
                "wake set"
            } else {
                "no wake"
            })
        }))
        .child(
            nsh_base::button()
                .accessibility_label(Some("Publish provider catalog".into()))
                .child(text("Publish provider catalog"))
                .on_press(move || {
                    assert!(publishing.set_config(
                        &EnvironmentId("host".into()),
                        session,
                        config()
                    ));
                }),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Clear provider catalog".into()))
                .child(text("Clear provider catalog"))
                .on_press(move || {
                    let mut config = config();
                    config.providers.0.clear();
                    assert!(store.set_config(&EnvironmentId("host".into()), session, config));
                }),
        )
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
        assert!(!app.screen().shows("Done"));
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
        app.wait_for("I added clamp next to sum and the tests pass.")
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

#[test]
fn slim_rows_show_woke_until_the_server_visit_watermark_acknowledges_it() {
    for window in [DESKTOP, PHONE] {
        let mut app = fixture(window);
        app.wait_for("Port the row").expect("row");
        app.press("role:button[Show slim wake]").expect("show wake");
        app.wait_for("text:wake set")
            .expect("wake observation installed");
        assert!(
            app.screen()
                .matches("role:button[Dismiss Woke notification]")
                .expect("valid selector")
        );
        assert!(app.screen().shows("Woke"));
        assert!(
            !app.screen()
                .enabled("role:button[Dismiss Woke notification]")
                .expect("wake control")
        );
        app.press("role:button[Mark read]").expect("visit advances");
        assert!(
            !app.screen()
                .matches("role:button[Dismiss Woke notification]")
                .expect("valid selector")
        );
        assert!(!app.screen().shows("Woke"));
    }
}

#[test]
fn provider_glyphs_follow_client_config_and_project_icons_use_the_shared_avatar() {
    for window in [DESKTOP, PHONE] {
        let mut app = fixture(window);
        app.wait_for("Port the row").expect("row");
        app.wait_for("id:project-monogram")
            .expect("shared project avatar");
        assert!(
            !app.screen()
                .matches("id:sidebar-provider-stack")
                .expect("valid selector")
        );
        assert!(
            !app.screen()
                .matches("role:button[Settle thread]")
                .expect("valid selector")
        );
        app.press("role:button[Publish provider catalog]")
            .expect("config push");
        app.wait_for("id:sidebar-provider-stack")
            .expect("provider glyph from client catalog");
        assert!(
            !app.screen()
                .matches("role:button[Settle thread]")
                .expect("valid selector"),
            "no fake mutation control even when server config is present"
        );
        app.press("role:button[Clear provider catalog]")
            .expect("catalog update");
        app.wait_for_gone(
            "id:sidebar-provider-stack",
            std::time::Duration::from_secs(5),
        )
        .expect("missing current instance removes the stack");
    }
}
