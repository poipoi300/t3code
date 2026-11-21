//! App-root mounts (ui-mounts-app-root): the root's providers (`app::provide_client_services`)
//! and the root tree (`app::app_tree`) under a fake-platform client, as `App` runs them with the
//! host's. Nothing here assigns a registry, runtime, registration or service by hand.

use std::rc::Rc;
use std::time::Duration;

use nsh_core::platform::Os;
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
use t3_ui::runtime::{Rt, backend::Backend, host::Choice};
use t3_ui::screens::panel::{Action, PanelHost};
use t3_ui::state::nav::{Navigator, Route, SettingsPage};

use crate::support::{DESKTOP, PHONE};

const NOW: &str = "2026-06-20T00:00:00.000Z";

fn key() -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("env".into()),
        thread: ThreadId("t1".into()),
    }
}

fn environment() -> EnvironmentId {
    EnvironmentId("env".into())
}

/// A settled thread `old-NN` in the seeded project (`settledOverride: settled`).
fn settled_thread(index: usize) -> serde_json::Value {
    json!({"id":format!("old-{index:02}"),"projectId":"project","title":format!("Old thread {index:02}"),"providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":format!("old-{index:02}"),"parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":NOW,"updatedAt":NOW,"archivedAt":null,"settledOverride":"settled","settledAt":NOW,"lastVisitedAt":null,"deletedAt":null,"latestRunId":null,"activeRunId":null,"status":"idle","pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0})
}

/// The shell stream items that list thread `t1` with `status`, and `settled` settled threads.
fn shell_items(status: &str, completed_at: Option<&str>) -> Vec<OrchestrationV2ShellStreamItem> {
    shell_items_with(status, completed_at, 0)
}

fn shell_items_with(
    status: &str,
    completed_at: Option<&str>,
    settled: usize,
) -> Vec<OrchestrationV2ShellStreamItem> {
    shell_items_of(
        std::iter::once(thread_t1(status, completed_at))
            .chain((0..settled).map(settled_thread))
            .collect(),
    )
}

/// Thread `t1` ("Fix the build") with `status`.
fn thread_t1(status: &str, completed_at: Option<&str>) -> serde_json::Value {
    json!({"id":"t1","projectId":"project","title":"Fix the build","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"t1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":NOW,"updatedAt":NOW,"archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null,"latestRunId":"run-1","activeRunId":null,"status":status,"latestRunCompletedAt":completed_at,"pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":null,"hasActionableProposedPlan":false,"itemCount":1,"visibleItemCount":1})
}

/// `thread`, snoozed until long after the test's clock.
fn snoozed(mut thread: serde_json::Value) -> serde_json::Value {
    thread["snoozedAt"] = json!(NOW);
    thread["snoozedUntil"] = json!("2099-01-01T00:00:00.000Z");
    thread
}

/// A task `t1` started (a subagent created through MCP), still running and asking to run a
/// command. (A task's plain input question goes to its parent agent, not to the user.)
fn subagent_asking_for_approval() -> serde_json::Value {
    let mut child = thread_t1("running", None);
    child["id"] = json!("sub-1");
    child["title"] = json!("Run the tests");
    child["creationSource"] = json!("mcp");
    child["lineage"] =
        json!({"rootThreadId":"t1","parentThreadId":"t1","relationshipToParent":"subagent"});
    child["pendingRuntimeRequest"] = json!({"id":"q","kind":"command","createdAt":NOW});
    child
}

/// The shell stream items that list `threads` in the seeded project.
fn shell_items_of(threads: Vec<serde_json::Value>) -> Vec<OrchestrationV2ShellStreamItem> {
    let snapshot = json!({"kind":"snapshot","snapshot":{"schemaVersion":1,"snapshotSequence":1,"threads":threads,"archivedThreads":[],"projects":[{"id":"project","title":"Project","workspaceRoot":"/workspace/project","repositoryIdentity":null,"defaultModelSelection":null,"scripts":[],"createdAt":NOW,"updatedAt":NOW}]}});
    [snapshot, json!({"kind":"synchronized"})]
        .into_iter()
        .map(|item| {
            serde_json::from_value::<OrchestrationV2ShellStreamItem>(item)
                .unwrap_or_else(|error| panic!("{error}"))
        })
        .collect()
}

/// A store holding one live environment: its config, project and thread `t1`.
fn seeded_store(status: &str) -> (ClientStore, t3_client::types::Generation) {
    seeded_store_with(status, 0)
}

fn seeded_store_with(status: &str, settled: usize) -> (ClientStore, t3_client::types::Generation) {
    let store = ClientStore::default();
    let session = store.begin_session(environment());
    store.apply_shell(
        &environment(),
        session,
        shell_items_with(status, None, settled),
    );
    let mut config: serde_json::Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/ServerConfig.jsonl")
            .lines()
            .next()
            .expect("config sample"),
    )
    .expect("config JSON");
    config["environment"]["environmentId"] = json!("env");
    config["environment"]["label"] = json!("Build machine");
    assert!(store.set_config(
        &environment(),
        session,
        serde_json::from_value(config).expect("canonical config"),
    ));
    (store, session)
}

fn fake_client(store: &ClientStore, platform: FakePlatform) -> ClientRuntime<FakePlatform> {
    ClientRuntime::new(
        platform,
        store.clone(),
        OrchestrationV2CreationSource::Web,
        Rc::new(|future| {
            spawn(future);
        }),
    )
}

/// The fake client as the runtime's backend, listing its one environment as connected: the
/// connection catalogue is what pairing fills, and the fake platform has no server to pair with
/// (SB-LIST's fixture lists its environments the same way). Everything else is the client's.
struct ListedClient(ClientRuntime<FakePlatform>);

impl Backend for ListedClient {
    fn pair(
        &self,
        input: t3_client::connection::PairingInput,
    ) -> futures::future::LocalBoxFuture<
        'static,
        Result<EnvironmentId, t3_client::connection::ConnectionFailure>,
    > {
        self.0.pair(input)
    }
    fn environments(&self) -> Vec<t3_ui::runtime::backend::EnvironmentEntry> {
        vec![t3_ui::runtime::backend::EnvironmentEntry {
            id: environment(),
            label: "Build machine".into(),
            routes: Vec::new(),
            damaged: false,
            state: t3_client::connection::ConnectionState {
                phase: t3_client::connection::Phase::Connected,
                ..Default::default()
            },
        }]
    }
    fn changes(&self) -> futures::stream::LocalBoxStream<'static, ()> {
        futures::StreamExt::boxed_local(futures::stream::pending())
    }
    fn open_thread(&self, key: &ThreadKey) {
        self.0.open_thread(key);
    }
    fn close_thread(&self, key: &ThreadKey) {
        self.0.close_thread(key);
    }
    fn send_message(
        &self,
        key: &ThreadKey,
        text: String,
    ) -> futures::future::LocalBoxFuture<'static, Result<(), t3_ui::runtime::backend::SendError>>
    {
        self.0.send_message(key, text)
    }
}

fn config(window: (f32, f32), os: Option<Os>, root: Root) -> EngineConfig {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        clock: nsh_engine::Clock::manual(std::time::UNIX_EPOCH),
        ..EngineConfig::new(root)
    };
    // As `support::start` arranges the desktop window.
    if window == DESKTOP {
        config.environment.os = Os::Linux;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    if let Some(os) = os {
        config.environment.os = os;
    }
    config
}

// ---- The thread workspace under the root's providers ----

#[derive(Clone, PartialEq, Props)]
struct WorkspaceProps {
    terminal_open: bool,
    terminal_granted: bool,
}

/// The client's terminal service with the session's `terminal:read` grant (the fake platform has
/// no live session, so the toggle would be disabled).
struct Granted<T>(T);

impl<T: t3_ui::runtime::services::terminal::TerminalClient>
    t3_ui::runtime::services::terminal::TerminalClient for Granted<T>
{
    fn load(&self) -> futures::future::LocalBoxFuture<'static, Result<(), t3_client::ClientError>> {
        self.0.load()
    }
    fn drawer(
        &self,
        key: &ThreadKey,
        launch: Option<t3_client::services::terminal_sessions::LaunchLocation>,
    ) -> t3_client::services::terminal_sessions::TerminalDrawerLease {
        self.0.drawer(key, launch)
    }
    fn session(
        &self,
        key: &ThreadKey,
        terminal: &str,
        launch: &t3_client::services::terminal_sessions::TerminalLaunch,
    ) -> t3_client::services::terminal_sessions::TerminalSessionLease {
        self.0.session(key, terminal, launch)
    }
    fn can_operate(&self, key: &ThreadKey) -> bool {
        self.0.can_operate(key)
    }
    fn can_read(&self, _: &ThreadKey) -> bool {
        true
    }
    fn is_open(&self, key: &ThreadKey) -> bool {
        self.0.is_open(key)
    }
    fn ui_changes(&self) -> futures::stream::LocalBoxStream<'static, ()> {
        self.0.ui_changes()
    }
    fn set_open(&self, key: &ThreadKey, open: bool) {
        self.0.set_open(key, open)
    }
    fn toggle(&self, key: &ThreadKey) {
        self.0.toggle(key)
    }
    fn set_height(&self, key: &ThreadKey, height: f64) {
        self.0.set_height(key, height)
    }
    fn activate(&self, key: &ThreadKey, terminal: &str) {
        self.0.activate(key, terminal)
    }
    fn launch(
        &self,
        key: &ThreadKey,
        placement: t3_client::services::terminal_sessions::Placement,
        location: Option<t3_client::services::terminal_sessions::LaunchLocation>,
    ) -> Option<(
        String,
        futures::future::LocalBoxFuture<'static, Result<(), t3_client::ClientError>>,
    )> {
        self.0.launch(key, placement, location)
    }
    fn close_terminal(
        &self,
        key: &ThreadKey,
        terminal: &str,
    ) -> Option<futures::future::LocalBoxFuture<'static, ()>> {
        self.0.close_terminal(key, terminal)
    }
    fn write(
        &self,
        key: &ThreadKey,
        terminal: &str,
        data: String,
    ) -> futures::future::LocalBoxFuture<'static, Result<(), t3_client::ClientError>> {
        self.0.write(key, terminal, data)
    }
    fn resize(
        &self,
        key: &ThreadKey,
        terminal: &str,
        grid: t3_client::services::terminal::TerminalGrid,
    ) -> futures::future::LocalBoxFuture<'static, Result<(), t3_client::ClientError>> {
        self.0.resize(key, terminal, grid)
    }
}

#[component]
fn ClientWorkspace(props: WorkspaceProps) -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    provide_context(t3_ui::theme::text_view_parts());
    provide_context(t3_ui::state::panels::Panels::new());
    let (store, _) = seeded_store("idle");
    let client = fake_client(&store, FakePlatform::default());
    let backend: Rc<dyn Backend> = Rc::new(client.clone());
    provide_context(Rt::new(store, backend, Choice::Client));
    provide_context(Navigator::new(Route::Thread(key())));
    t3_ui::app::provide_client_services(&client);
    if props.terminal_granted {
        provide_context(t3_ui::runtime::services::terminal::TerminalServices(
            Rc::new(Granted(client.terminals())),
        ));
    }
    if props.terminal_open {
        client.terminals().set_open(&key(), true);
    }
    // What a file link in a reply does: the shared host from context (`markdown/links.rs`).
    let host = use_context::<PanelHost>().expect("the app root provides the panel host");
    view()
        .w_full()
        .h_full()
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from("Follow file link")))
                .on_press(move || {
                    host.apply(
                        &key(),
                        Action::OpenFile {
                            path: "src/main.rs".into(),
                            line: Some(3.0),
                        },
                    )
                })
                .child(text("Follow file link")),
        )
        .child(t3_ui::screens::panel::thread_workspace(key()))
}

fn workspace(window: (f32, f32), terminal_open: bool) -> Headless {
    start_workspace(window, terminal_open, false)
}

fn start_workspace(window: (f32, f32), terminal_open: bool, terminal_granted: bool) -> Headless {
    let root = Root::new::<ClientWorkspace>(move || WorkspaceProps {
        terminal_open,
        terminal_granted,
    });
    let mut app = Headless::start(config(window, None, root)).expect("client workspace");
    app.wait_for("role:button[Follow file link]")
        .expect("mounted");
    app
}

#[test]
fn the_launcher_opens_the_root_registered_diff_and_files_surfaces() {
    for window in [DESKTOP, PHONE] {
        for (kind, surface) in [
            ("Files", "id:file-preview-panel"),
            ("Diff", "id:panel-tab-diff"),
        ] {
            let mut app = workspace(window, false);
            app.press("role:button[Toggle right panel]")
                .expect("open workspace panel");
            app.wait_for("Open a surface").expect("launcher");
            app.press(&format!("role:button[{kind}]"))
                .unwrap_or_else(|error| panic!("{kind} offered: {error}"));
            app.wait_for(surface)
                .unwrap_or_else(|error| panic!("{kind} surface rendered: {error}"));
        }
    }
}

#[test]
fn a_file_link_opens_the_file_in_the_files_surface() {
    let mut app = workspace(DESKTOP, false);
    app.press("role:button[Follow file link]").expect("link");
    app.wait_for("id:file-preview-panel")
        .expect("the Files registration renders the file surface");
    app.wait_for("main.rs").expect("the file's tab");
}

#[test]
fn the_terminal_drawer_follows_the_clients_drawer_record() {
    for window in [DESKTOP, PHONE] {
        let mut app = workspace(window, true);
        app.wait_for("id:thread-terminal-drawer")
            .expect("the root's terminal registration mounts the drawer");
    }
    let mut app = workspace(DESKTOP, false);
    app.wait_for("role:button[Toggle right panel]")
        .expect("workspace");
    assert!(
        !app.screen()
            .matches("id:thread-terminal-drawer")
            .expect("valid selector"),
        "closed in the client's record, the drawer shows nothing"
    );
}

/// The header's toggle (integration 4 saw it highlight without a drawer on desktop).
#[test]
fn the_header_toggle_opens_and_closes_the_terminal_drawer() {
    let mut app = start_workspace(DESKTOP, false, true);
    app.press("role:button[Toggle terminal drawer]")
        .expect("the toggle is enabled for a thread with a project");
    app.wait_for("id:thread-terminal-drawer")
        .expect("pressing it opens the drawer");
    app.press("role:button[Toggle terminal drawer]")
        .expect("toggle again");
    app.settle();
    assert!(
        !app.screen()
            .matches("id:thread-terminal-drawer")
            .expect("valid selector"),
        "pressing it again closes the drawer"
    );
}

#[test]
fn the_details_card_reads_project_scripts_from_the_client() {
    let mut app = workspace(DESKTOP, false);
    app.wait_for("Build machine").expect("inline details");
    app.wait_for("role:button[Add project script]")
        .expect("project scripts from the root's ThreadDetailsRuntime");
}

// ---- The root tree (`app::app_tree`) ----

#[derive(Clone, PartialEq, Props)]
struct TreeProps {
    /// The route's path (`Route::path`); a route is not a component prop value.
    route: String,
    status: String,
    settled: usize,
}

#[component]
fn AppTree(props: TreeProps) -> impl IntoElement {
    let (store, session) = seeded_store_with(&props.status, props.settled);
    let platform = FakePlatform::default();
    let client = fake_client(&store, platform);
    let backend: Rc<dyn Backend> = Rc::new(ListedClient(client.clone()));
    let runtime = Rt::new(store.clone(), backend, Choice::Client);
    let services = client.clone();
    let probe = client.clone();
    // What the client's records hold, so tests assert on them rather than only on the screen.
    let rich_text = signal(
        probe
            .settings()
            .client_settings()
            .composer_rich_text_enabled,
    );
    let stored_width = signal(None::<f64>);
    let settings = probe.settings();
    spawn(async move {
        let mut changes = settings.changes();
        while futures::StreamExt::next(&mut changes).await.is_some() {
            let _ = rich_text.try_set(settings.client_settings().composer_rich_text_enabled);
        }
    });
    let completing = store.clone();
    let enabling = client.clone();
    view()
        .w_full()
        .h_full()
        .child(text(move || format!("rich text {}", rich_text.get())))
        .child(text(move || {
            format!("stored width {:?}", stored_width.get())
        }))
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from("Read stored width")))
                .on_press(move || {
                    let load = probe.sidebar_width().load(4000.0);
                    spawn(async move {
                        let _ = stored_width.try_set(Some(load.await));
                    });
                })
                .child(text("Read stored width")),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from("Complete the turn")))
                .on_press(move || {
                    completing.apply_shell(
                        &environment(),
                        session,
                        shell_items("completed", Some("2026-06-20T00:05:00.000Z")),
                    );
                })
                .child(text("Complete the turn")),
        )
        .child({
            let choosing = client.clone();
            nsh_base::button()
                .accessibility_label(Some(SharedString::from("Choose sound alerts")))
                .on_press(move || {
                    // What the Settings mode select writes.
                    let mut patch = serde_json::Map::new();
                    patch.insert("notificationMode".into(), json!("sound"));
                    let write = choosing.settings().update_client(patch);
                    spawn(async move {
                        write.await.expect("device settings write");
                    });
                })
                .child(text("Choose sound alerts"))
        })
        .child(shell_button(
            "Snooze the thread",
            store.clone(),
            session,
            vec![snoozed(thread_t1("running", None))],
        ))
        .child(shell_button(
            "Complete the snoozed turn",
            store.clone(),
            session,
            vec![snoozed(thread_t1(
                "completed",
                Some("2026-06-20T00:05:00.000Z"),
            ))],
        ))
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from("Enable in-app notifications")))
                .on_press(move || {
                    // What the Settings switch writes (`updateClientSettings`).
                    let mut patch = serde_json::Map::new();
                    patch.insert("inAppNotificationsEnabled".into(), json!(true));
                    let write = enabling.settings().update_client(patch);
                    spawn(async move {
                        write.await.expect("device settings write");
                    });
                })
                .child(text("Enable in-app notifications")),
        )
        .child(t3_ui::app::app_tree(
            runtime,
            Route::parse(&props.route),
            move || t3_ui::app::provide_client_services(&services),
        ))
}

/// A button that hands the environment the next shell snapshot listing `threads`.
fn shell_button(
    label: &'static str,
    store: ClientStore,
    session: t3_client::types::Generation,
    threads: Vec<serde_json::Value>,
) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(Some(SharedString::from(label)))
        .on_press(move || {
            store.apply_shell(&environment(), session, shell_items_of(threads.clone()));
        })
        .child(text(label))
}

/// What the app spoke through the screen reader (`announce_for_accessibility`).
fn announced(app: &Headless) -> Vec<String> {
    app.screen()
        .commands()
        .iter()
        .filter_map(|(_, command)| match command {
            nsh_core::Command::Announce(message) => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn start_app_tree(window: (f32, f32), os: Option<Os>, route: Route, status: &str) -> Headless {
    start_app_tree_with(window, os, route, status, 0)
}

fn start_app_tree_with(
    window: (f32, f32),
    os: Option<Os>,
    route: Route,
    status: &str,
    settled: usize,
) -> Headless {
    let status = status.to_owned();
    let route = route.path();
    let root = Root::new::<AppTree>(move || TreeProps {
        route: route.clone(),
        status: status.clone(),
        settled,
    });
    let mut app = Headless::start(config(window, os, root)).expect("app tree");
    app.wait_for("Read stored width").expect("mounted");
    app
}

// ---- Settings through `app::booted_tree` over the host's own client, as `App` boots it ----
// The settings shell reads only `rt().backend().client()` (the host's client type), so these
// boot `runtime::client::start` over a fresh data directory, as the settings lane's tests do.

#[derive(Clone, PartialEq, Props)]
struct HostProps {
    page: usize,
}

#[component]
fn HostTree(props: HostProps) -> impl IntoElement {
    use std::cell::RefCell;
    use t3_ui::runtime::host::Started;
    let store = ClientStore::default();
    let booted = signal(false);
    let slot: Rc<RefCell<Option<Started>>> = Rc::default();
    {
        let slot = slot.clone();
        let store = store.clone();
        spawn(async move {
            *slot.borrow_mut() = Some(t3_ui::runtime::client::start(store).await);
            booted.set(true);
        });
    }
    let route = Route::Settings(SettingsPage::ALL[props.page]);
    view().w_full().h_full().child(dynamic(move || {
        if !booted.get() {
            return None;
        }
        let started = slot.borrow_mut().take()?;
        let store = store.clone();
        let route = route.clone();
        Some(untracked(move || {
            // What the client's record holds, read through the client `App` boots.
            let settings = started.backend.client().map(|client| client.settings());
            let rich_text = signal(None::<bool>);
            if let Some(settings) = settings {
                rich_text.set(Some(settings.client_settings().composer_rich_text_enabled));
                spawn(async move {
                    let mut changes = settings.changes();
                    while futures::StreamExt::next(&mut changes).await.is_some() {
                        let _ = rich_text
                            .try_set(Some(settings.client_settings().composer_rich_text_enabled));
                    }
                });
            }
            view()
                .w_full()
                .h_full()
                .child(text(move || format!("rich text {:?}", rich_text.get())))
                .child(t3_ui::app::booted_tree(route, store, started))
                .into_element()
        }))
    }))
}

fn fresh_data_dir() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "t3-ui-app-root-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn start_host_tree(window: (f32, f32), page: SettingsPage) -> Headless {
    let index = SettingsPage::ALL
        .iter()
        .position(|p| *p == page)
        .unwrap_or(0);
    let mut config = config(
        window,
        None,
        Root::new::<HostTree>(move || HostProps { page: index }),
    );
    config.data_dir = Some(fresh_data_dir());
    let mut app = Headless::start(config).expect("host tree");
    app.wait_for("text:rich text Some(true)")
        .expect("the host's client booted and read its record (rich text is on by default)");
    app
}

fn packaged_app_root() -> Root {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../../../../shells/nsh.json")).expect("shell manifest");
    let schemes: Vec<String> =
        serde_json::from_value(config["schemes"].clone()).expect("claimed schemes");
    Root::of::<t3_ui::App>().url_schemes(schemes)
}

#[test]
fn an_unknown_launch_address_uses_host_history_to_return_to_pairing() {
    let mut config = config(DESKTOP, Some(Os::Linux), packaged_app_root());
    config.data_dir = Some(fresh_data_dir());
    let mut app = Headless::start_with_link(config, "t3code-dev://unknown-path")
        .expect("start from the cold link");
    app.wait_for_within("Page not found", Duration::from_secs(30))
        .expect("host kept the launch address");
    app.press("role:button[Go home]")
        .expect("replace the unknown address");
    app.wait_for("Connect to T3 Code")
        .expect("unpaired home resolves to Pair");
    app.open_with_url("t3code-dev://another-unknown-path")
        .expect("deliver a warm link");
    app.wait_for("Page not found")
        .expect("the same history receives the warm link");
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn a_cold_pairing_link_prompts_once_when_an_environment_is_already_saved() {
    use t3_client::connection::{ConnectionMode, Route as ServerRoute, Time};
    use t3_client::environment::{EnvironmentRegistration, EnvironmentRegistry};
    use t3_client::platform::NetworkStatus;
    use t3_platform::{NativeConfig, NativePlatform};
    let dir = fresh_data_dir();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("storage runtime");
    runtime.block_on(async {
        let platform = NativePlatform::open(NativeConfig::desktop(dir.join("client")))
            .await
            .expect("storage");
        let mut registry =
            EnvironmentRegistry::<()>::load(&platform, NetworkStatus::Offline, Time::default())
                .await
                .expect("catalog");
        let id = EnvironmentId("saved".into());
        registry
            .register(
                EnvironmentRegistration {
                    id: id.clone(),
                    label: "Saved backend".into(),
                    enabled: false,
                    routes: vec![ServerRoute {
                        id: "direct".into(),
                        environment: id,
                        mode: ConnectionMode::Remote,
                        http_base: "http://127.0.0.1:1/".into(),
                        ws_base: "ws://127.0.0.1:1/".into(),
                    }],
                },
                false,
                NetworkStatus::Offline,
                Time::default(),
            )
            .expect("registration");
        registry
            .save(&platform)
            .await
            .expect("persist disabled backend");
    });
    let mut config = config(DESKTOP, Some(Os::Linux), packaged_app_root());
    config.data_dir = Some(dir);
    let mut app = Headless::start_with_link(
        config,
        "t3code-dev://pair?pairingUrl=wss%3A%2F%2Fbox.test%2Fpair%23token%3Dtest",
    )
    .expect("app starts");
    let prompt = "Add the backend at https://box.test?";
    app.wait_for_within(prompt, Duration::from_secs(30))
        .expect("cold pairing request");
    app.press("role:button[Cancel]").expect("decline");
    app.wait_for_gone(prompt, Duration::from_secs(5))
        .expect("only one confirmation");
}

#[test]
fn a_settings_route_renders_the_shell_with_its_navigation_and_pages() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_host_tree(window, SettingsPage::General);
        app.wait_for("Notifications")
            .expect("the General page inside the shell");
        if window == DESKTOP {
            assert!(app.screen().shows("Appearance"), "the shell's navigation");
        }
    }
    let mut app = start_host_tree(DESKTOP, SettingsPage::Chat);
    app.wait_for("Follow-ups").expect("the Chat page");
    app.press("label:Rich text composer").expect("switch");
    app.wait_for("text:rich text Some(false)")
        .expect("the switch writes through the client's settings service");
}

#[test]
fn a_settings_page_not_built_yet_renders_inside_the_shell() {
    let mut app = start_host_tree(DESKTOP, SettingsPage::Voice);
    app.wait_for("This screen is not built yet in the Rust client.")
        .expect("the page's not-built body");
    assert!(
        app.screen().shows("Appearance"),
        "inside the shell's navigation"
    );
}

#[test]
fn dragging_the_sidebar_rail_persists_the_width_through_the_clients_rule() {
    let mut app = start_app_tree(DESKTOP, None, Route::Home, "idle");
    app.wait_for("role:button[Resize Sidebar]").expect("rail");
    assert_eq!(
        app.screen()
            .layout_nodes()
            .iter()
            .filter(|node| node.test_id.as_deref() == Some("connection-notices"))
            .count(),
        1,
        "the connection notices host is mounted once in the root"
    );
    app.drag("role:button[Resize Sidebar]", point(px(900.0), px(0.0)))
        .expect("drag far right");
    app.press("role:button[Read stored width]").expect("read");
    // The client's maximum keeps 40rem for the main content: 1280 - 640.
    app.wait_for("stored width Some(640.0)")
        .expect("the commit is the client's clamped width, stored by the client");
}

#[test]
fn a_trackpad_swipe_goes_back_only_where_the_host_allows_it() {
    let swipe = |app: &mut Headless| {
        for (x, y) in [(-10.0, 0.0), (-60.0, 0.0), (-70.0, 0.0)] {
            // Over the header: the timeline's own scroll view keeps wheels it can use
            // (SH-SWIPE's declared difference 2).
            app.wheel("id:thread-title-actions", point(px(x), px(y)))
                .expect("wheel");
        }
        app.fast_forward(Duration::from_millis(400)).expect("pause");
        app.settle();
    };
    let mut app = start_app_tree(DESKTOP, Some(Os::MacOs), Route::Thread(key()), "idle");
    app.wait_for("role:button[Toggle right panel]")
        .expect("the thread");
    swipe(&mut app);
    app.wait_for_gone("role:button[Toggle right panel]", Duration::from_secs(1))
        .expect("the swipe went back to the list");
    for os in [Os::Windows, Os::Linux] {
        let mut app = start_app_tree(DESKTOP, Some(os), Route::Thread(key()), "idle");
        app.wait_for("role:button[Toggle right panel]")
            .expect("the thread");
        swipe(&mut app);
        assert!(
            app.screen()
                .matches("role:button[Toggle right panel]")
                .expect("valid selector"),
            "{os:?} keeps the thread"
        );
    }
}

#[test]
fn a_completed_background_turn_raises_a_toast_that_opens_the_thread() {
    let mut app = start_app_tree(DESKTOP, None, Route::Home, "running");
    app.press("role:button[Enable in-app notifications]")
        .expect("enable");
    app.press("role:button[Complete the turn]")
        .expect("complete");
    app.wait_for("Thread completed").expect("the in-app toast");
    app.press("role:button[Open thread]").expect("open");
    app.wait_for("role:button[Toggle right panel]")
        .expect("the thread opened");
}

// The root tree over a client paired with a connectable fake environment: the client's
// registry lists it, as pairing does in the app (the thread shell views read the registry).
#[component]
fn PairedTree() -> impl IntoElement {
    use futures::FutureExt;
    use t3_client::testing::environment::FakeEnvironment;
    let store = ClientStore::default();
    let owner = Owner::current();
    let spawner: t3_client::composition::Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        });
    });
    let server = FakeEnvironment::new("env", spawner.clone());
    let snapshot = serde_json::to_value(&shell_items_of(vec![thread_t1("running", None)])[0])
        .expect("snapshot item");
    server.set_shell(snapshot["snapshot"].clone());
    let client = ClientRuntime::new(
        server.platform(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let pairing = client.pair(server.pairing());
    spawner(
        async move {
            pairing.await.expect("paired");
        }
        .boxed_local(),
    );
    let backend: Rc<dyn Backend> = Rc::new(client.clone());
    let runtime = Rt::new(store.clone(), backend, Choice::Client);
    let services = client.clone();
    let enabling = client.clone();
    let asking = store.clone();
    view()
        .w_full()
        .h_full()
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from("Enable in-app notifications")))
                .on_press(move || {
                    let mut patch = serde_json::Map::new();
                    patch.insert("inAppNotificationsEnabled".into(), json!(true));
                    let write = enabling.settings().update_client(patch);
                    spawn(async move {
                        write.await.expect("device settings write");
                    });
                })
                .child(text("Enable in-app notifications")),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from("Subagent asks for approval")))
                .on_press(move || {
                    // The environment's next shell snapshot, on the client's live session.
                    let session = asking
                        .snapshot()
                        .environments
                        .get(&environment())
                        .expect("paired environment")
                        .session;
                    asking.apply_shell(
                        &environment(),
                        session,
                        shell_items_of(vec![
                            thread_t1("running", None),
                            subagent_asking_for_approval(),
                        ]),
                    );
                })
                .child(text("Subagent asks for approval")),
        )
        .child(t3_ui::app::app_tree(runtime, Route::Home, move || {
            t3_ui::app::provide_client_services(&services)
        }))
}

/// The web's `pendingSubagentRequestsAtom`: a task's request is the parent's notification; the
/// task, shown under its parent, has none of its own.
#[test]
fn a_subagent_asking_for_approval_notifies_its_parent_once() {
    let mut app =
        Headless::start(config(DESKTOP, None, Root::of::<PairedTree>())).expect("paired tree");
    app.wait_for("Working")
        .expect("paired: the running thread is shelved");
    app.press("role:button[Enable in-app notifications]")
        .expect("enable");
    app.settle();
    app.press("role:button[Subagent asks for approval]")
        .expect("the task asks");
    app.wait_for("Approval needed")
        .expect("the parent's in-app notification");
    app.settle();
    let toasts = app
        .screen()
        .texts()
        .into_iter()
        .filter(|text| *text == "Approval needed")
        .count();
    assert_eq!(toasts, 1, "one notification, the parent's");
}

/// A speaker that decodes anything and records what it was asked (the headless one decodes WAV
/// only; the web's alerts are MP3).
#[derive(Clone, Default)]
struct Speaker(std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>);

impl nsh_core::audio::AudioBackend for Speaker {
    fn command(&self, command: nsh_core::audio::AudioCommand, events: &nsh_core::audio::AudioSink) {
        use nsh_core::audio::{AudioCommand, AudioEvent};
        let name = match command {
            AudioCommand::Load { .. } => {
                events.send(AudioEvent::Loaded {
                    duration: Duration::from_millis(500),
                    bytes: 1,
                    truncated: false,
                });
                "load"
            }
            AudioCommand::Play { .. } => "play",
            _ => "other",
        };
        self.0.lock().expect("speaker").push(name);
    }
}

fn start_app_tree_with_speaker(speaker: Speaker) -> Headless {
    let root = Root::new::<AppTree>(|| TreeProps {
        route: Route::Home.path(),
        status: "running".into(),
        settled: 0,
    });
    let mut config = config(DESKTOP, None, root);
    config.audio = Some(std::sync::Arc::new(speaker));
    let mut app = Headless::start(config).expect("app tree");
    app.wait_for("Read stored width").expect("mounted");
    app
}

/// `playNotificationSound`: in a sound mode a finished turn plays the completion alert, once.
#[test]
fn a_completed_turn_plays_the_alert_only_in_a_sound_mode() {
    let plays = |speaker: &Speaker| {
        speaker
            .0
            .lock()
            .expect("speaker")
            .iter()
            .filter(|call| **call == "play")
            .count()
    };
    let speaker = Speaker::default();
    let mut app = start_app_tree_with_speaker(speaker.clone());
    app.press("role:button[Complete the turn]")
        .expect("complete");
    app.settle();
    assert_eq!(plays(&speaker), 0, "control: alerts off by default");
    let speaker = Speaker::default();
    let mut app = start_app_tree_with_speaker(speaker.clone());
    app.press("role:button[Choose sound alerts]")
        .expect("sound mode");
    app.settle();
    app.press("role:button[Complete the turn]")
        .expect("complete");
    app.settle();
    assert_eq!(
        plays(&speaker),
        1,
        "one alert: {:?}",
        speaker.0.lock().expect("speaker")
    );
}

/// BackgroundThreadAnnouncer's `effectiveSnoozed`: a snoozed thread finishing is not spoken.
#[test]
fn a_snoozed_thread_finishing_in_the_background_is_not_announced() {
    let mut app = start_app_tree(DESKTOP, None, Route::Home, "running");
    app.press("role:button[Complete the turn]")
        .expect("complete");
    app.settle();
    assert!(
        announced(&app)
            .iter()
            .any(|message| message.contains("Fix the build")),
        "control: an unsnoozed thread's completion is spoken: {:?}",
        announced(&app)
    );
    let mut app = start_app_tree(DESKTOP, None, Route::Home, "running");
    app.press("role:button[Snooze the thread]").expect("snooze");
    app.settle();
    app.press("role:button[Complete the snoozed turn]")
        .expect("complete while snoozed");
    app.settle();
    assert!(
        !announced(&app)
            .iter()
            .any(|message| message.contains("Fix the build")),
        "a snoozed thread stays quiet: {:?}",
        announced(&app)
    );
}

#[test]
fn switching_away_from_a_running_thread_can_recycle_its_sidebar_row() {
    let mut app = start_app_tree_with(DESKTOP, None, Route::Thread(key()), "running", 1);
    app.press("role:button[Settled]")
        .expect("show the second thread");
    app.wait_for("Old thread 00").expect("settled row");
    for _ in 0..3 {
        app.press("id:sidebar-threads >> text:Old thread 00")
            .expect("leave running thread");
        app.wait_for("role:button[Working]")
            .expect("running thread moved to its shelf");
        if !app
            .screen()
            .matches("id:sidebar-threads >> text:Fix the build")
            .expect("working row")
        {
            app.press("role:button[Working]")
                .expect("expand working shelf");
        }
        app.press("id:sidebar-threads >> text:Fix the build")
            .expect("return to running thread");
        app.wait_for("role:button[Thread actions for Fix the build]")
            .expect("running thread header");
    }
}

#[test]
fn the_sidebar_shelves_mount_their_markers_and_page_the_settled_tail() {
    let mut app = start_app_tree_with(DESKTOP, None, Route::Home, "idle", 40);
    app.wait_for("role:button[Settled]")
        .expect("the Settled shelf header");
    app.wait_for("text:40")
        .expect("its count includes the collapsed rows");
    let old_rows = |app: &Headless| {
        app.screen()
            .texts()
            .into_iter()
            .filter(|text| text.starts_with("Old thread"))
            .count()
    };
    assert_eq!(
        old_rows(&app),
        0,
        "the shelf starts collapsed, as on the web"
    );
    app.press("role:button[Settled]").expect("expand");
    app.wait_for("Old thread 00")
        .expect("the first page of the tail");
    app.press("label:Show 25 more").expect("show more");
    app.wheel("id:sidebar-threads", point(px(0.0), px(4000.0)))
        .expect("scroll to the end of the list");
    app.wait_for("label:Show 5 more").expect("the next page");
    // 35 settled rows are listed; the virtual list builds only those in view.
    assert!(old_rows(&app) < 35, "markers keep the list virtualised");
}

// ---- The workspace panel's header and the native window controls (integration 4) ----

#[component]
fn PanelHeader() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    provide_context(t3_ui::state::panels::Panels::new());
    let host =
        t3_ui::screens::panel::PanelHost::new(t3_ui::screens::panel::PanelStore::default(), |_| {});
    view()
        .w_full()
        .h_full()
        .child(t3_ui::screens::panel::right_panel_tabs(
            host,
            key(),
            t3_ui::screens::panel::PanelRegistry::new(vec![]),
            view().test_id("panel-layout-test").w(px(32.0)).h(px(28.0)),
        ))
}

fn start_panel_header(
    os: Os,
    window: (f32, f32),
    title_bar: nsh_core::geometry::Edges<nsh_core::geometry::Points>,
) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<PanelHeader>())
    };
    config.environment.os = os;
    let mut app = Headless::start(config).expect("panel header");
    app.first_render().expect("mounted");
    if title_bar != nsh_core::geometry::Edges::default() {
        app.set_environment("native titlebar geometry", move |environment| {
            environment.title_bar = title_bar;
            environment.window_state = Some(
                environment
                    .window_state
                    .unwrap_or_else(nsh_core::desktop::WindowState::default),
            );
        })
        .expect("native geometry");
    }
    app
}

fn node_bounds(app: &Headless, id: &str) -> nsh_core::geometry::Bounds<nsh_core::geometry::Points> {
    app.screen()
        .layout_nodes()
        .iter()
        .find(|node| node.test_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("mounted {id}"))
        .bounds
}

fn edges(left: f32, right: f32, top: f32) -> nsh_core::geometry::Edges<nsh_core::geometry::Points> {
    nsh_core::geometry::Edges {
        top: px(top),
        left: px(left),
        right: px(right),
        bottom: px(0.0),
    }
}

#[test]
fn the_panel_header_leaves_the_native_window_controls_free() {
    // Windows and Linux: caption buttons on the right; the header's last control ends before them.
    for os in [Os::Windows, Os::Linux] {
        let app = start_panel_header(os, DESKTOP, edges(0.0, 138.0, 40.0));
        let controls = node_bounds(&app, "panel-layout-test");
        assert!(
            controls.right().0 <= DESKTOP.0 - 138.0,
            "{os:?}: the header's controls end at {} under the caption buttons from {}",
            controls.right().0,
            DESKTOP.0 - 138.0
        );
    }
    // macOS, compact window (the panel is a full-width page): the traffic lights on the left.
    let narrow = (600.0, 800.0);
    let app = start_panel_header(Os::MacOs, narrow, edges(78.0, 0.0, 52.0));
    let tabs = node_bounds(&app, "panel-tabs-scroll");
    assert!(
        tabs.origin.x.0 >= 78.0,
        "the tab strip starts at {} under the traffic lights to 78",
        tabs.origin.x.0
    );
    // Web and mobile: no native controls, the source's pl-2 pr-3.
    for os in [Os::Web, Os::Android] {
        let app = start_panel_header(os, DESKTOP, nsh_core::geometry::Edges::default());
        assert_eq!(
            node_bounds(&app, "panel-layout-test").right().0,
            DESKTOP.0 - 12.0,
            "{os:?}"
        );
        assert_eq!(
            node_bounds(&app, "panel-tabs-scroll").origin.x.0,
            8.0,
            "{os:?}"
        );
    }
}

/// The desktop pass drives the "+" menu by role and name: "Diff" is an enabled `menuitem`
/// named by its label, and invoking it opens the Diff surface.
#[test]
fn the_add_surface_menu_items_are_named_and_invocable() {
    let mut app = workspace(DESKTOP, false);
    app.press("role:button[Toggle right panel]")
        .expect("open workspace panel");
    app.press("role:button[Files]").expect("first surface");
    app.wait_for("id:file-preview-panel").expect("files");
    app.press("role:button[Add panel surface]").expect("menu");
    app.press("role:menuitem[Diff]")
        .expect("the Diff item is a named, enabled menuitem");
    app.wait_for("id:panel-tab-diff")
        .expect("invoking it opens Diff");
}
