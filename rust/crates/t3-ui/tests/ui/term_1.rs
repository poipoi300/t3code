//! TERM-1: the drawer as the thread screen mounts it, over the real client runtime on a store
//! seeded with the thread's project. The connected paths (attach, launch, close, resize) are
//! the client's and are tested against its fake server (`t3-client/tests/client_services_terminal.rs`).

use std::rc::Rc;

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use t3_client::ClientStore;
use t3_client::composition::ClientRuntime;
use t3_client::testing::FakePlatform;
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_contracts::orchestration_v2::{
    OrchestrationV2CreationSource, OrchestrationV2ShellStreamItem,
};
use t3_ui::runtime::services::terminal::TerminalServices;
use t3_ui::screens::terminal::drawer::thread_terminal_drawer;
use t3_ui::theme;

use crate::support::{DESKTOP, PHONE};

const NOW: &str = "2026-06-20T00:00:00.000Z";

fn key(thread: &str) -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("env".into()),
        thread: ThreadId(thread.into()),
    }
}

fn seeded_store() -> ClientStore {
    let store = ClientStore::default();
    let environment = EnvironmentId("env".into());
    let session = store.begin_session(environment.clone());
    let thread = json!({"id":"t1","projectId":"project","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"t1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":NOW,"updatedAt":NOW,"archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null,"latestRunId":null,"activeRunId":null,"status":"idle","pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0});
    let snapshot = json!({"kind":"snapshot","snapshot":{"schemaVersion":1,"snapshotSequence":1,"threads":[thread],"archivedThreads":[],"projects":[{"id":"project","title":"Project","workspaceRoot":"/workspace/project","repositoryIdentity":null,"defaultModelSelection":null,"scripts":[],"createdAt":NOW,"updatedAt":NOW}]}});
    let items = [snapshot, json!({"kind":"synchronized"})]
        .into_iter()
        .map(|item| {
            serde_json::from_value::<OrchestrationV2ShellStreamItem>(item)
                .unwrap_or_else(|error| panic!("{error}"))
        })
        .collect();
    store.apply_shell(&environment, session, items);
    store
}

#[derive(Clone, PartialEq, Props)]
struct HarnessProps {
    thread: String,
    open: bool,
}

#[component]
fn Harness(props: HarnessProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    // No connection: the drawer shows what a disconnected or offline environment shows.
    let runtime = ClientRuntime::new(
        FakePlatform::default(),
        seeded_store(),
        OrchestrationV2CreationSource::Web,
        Rc::new(|_| {}),
    );
    let services = TerminalServices::new(runtime.terminals());
    let key = key(&props.thread);
    if props.open {
        services.0.set_open(&key, true);
    }
    let toggle = (services.clone(), key.clone());
    view()
        .size_full()
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from("Toggle terminal")))
                // The chord's action (`terminal.toggle`); the shell installs the key itself.
                .on_press(move || toggle.0.0.toggle(&toggle.1))
                .child(text("Toggle terminal")),
        )
        .child(thread_terminal_drawer(services, key))
}

fn start(window: (f32, f32), thread: &str, open: bool) -> Headless {
    let thread = thread.to_owned();
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<Harness>(move || HarnessProps {
            thread: thread.clone(),
            open,
        }))
    };
    config.environment.idiom = if window == PHONE {
        nsh_core::platform::Idiom::Phone
    } else {
        nsh_core::platform::Idiom::Desktop
    };
    let mut app = Headless::start(config).expect("component starts");
    app.wait_for("role:button[Toggle terminal]")
        .expect("mounted");
    app
}

#[test]
fn a_disconnected_thread_shows_the_empty_drawer_with_new_terminal_disabled() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, "t1", true);
        app.wait_for("No terminal sessions for this thread yet.")
            .expect("empty drawer");
        let new = "role:button[New Terminal]";
        app.wait_for(new).expect("new terminal action");
        assert!(!app.screen().enabled(new).expect("control exists"));
        assert!(app.press(new).is_err(), "inert without operate permission");
        assert!(
            app.screen()
                .shows("No terminal sessions for this thread yet.")
        );
    }
}

#[test]
fn the_drawer_follows_its_open_state_both_ways() {
    let mut app = start(DESKTOP, "t1", false);
    assert!(
        !app.screen()
            .shows("No terminal sessions for this thread yet.")
    );
    app.press("role:button[Toggle terminal]").expect("open");
    app.wait_for("No terminal sessions for this thread yet.")
        .expect("opened");
    app.press("role:button[Toggle terminal]").expect("close");
    app.wait_for_gone(
        "No terminal sessions for this thread yet.",
        std::time::Duration::from_secs(5),
    )
    .expect("closed");
}

#[test]
fn a_thread_without_a_project_shows_no_drawer_even_when_open() {
    let mut app = start(DESKTOP, "missing", true);
    app.settle();
    assert!(
        !app.screen()
            .shows("No terminal sessions for this thread yet.")
    );
}
