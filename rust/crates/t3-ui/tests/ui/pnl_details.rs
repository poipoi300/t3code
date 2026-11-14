//! PNL-DETAILS: real client projections in the retained details target.

use std::rc::Rc;

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::{Value, json};
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_ui::runtime::{Rt, demo::DemoBackend, host::Choice};
use t3_ui::screens::panel::details::{
    VersionControlSlots, thread_details_popover, thread_details_target,
};
use t3_ui::screens::panel::{Action, PanelHost, PanelStore};

use crate::support::{DESKTOP, PHONE};

fn sample(source: &str) -> Value {
    serde_json::from_str(source.lines().next().expect("canonical sample")).expect("sample JSON")
}

fn config(environment: &str, label: &str) -> t3_contracts::server::ServerConfig {
    let mut source = sample(include_str!(
        "../../../../fixtures/samples/ServerConfig.jsonl"
    ));
    source["environment"]["environmentId"] = json!(environment);
    source["environment"]["label"] = json!(label);
    source["environment"]["platform"]["machine"] = json!("desktop");
    serde_json::from_value(source).expect("canonical config")
}

fn publish_shell(
    store: &t3_client::ClientStore,
    environment: &EnvironmentId,
    session: t3_client::types::Generation,
    sequence: u64,
) {
    let mut thread = sample(include_str!(
        "../../../../fixtures/samples/OrchestrationV2ThreadShell.jsonl"
    ));
    thread["id"] = json!("same-thread");
    thread["projectId"] = json!("project");
    thread["worktreePath"] = json!(null);
    let mut project = sample(include_str!(
        "../../../../fixtures/samples/OrchestrationProjectShell.jsonl"
    ));
    project["id"] = json!("project");
    let snapshot = json!({
        "kind":"snapshot", "snapshot": {
            "schemaVersion":1,"snapshotSequence":sequence,
            "threads":[thread],"archivedThreads":[],"projects":[project]
        }
    });
    store.apply_shell(
        environment,
        session,
        vec![serde_json::from_value(snapshot).expect("shell snapshot")],
    );
}

fn control(label: &str, on_press: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(SharedString::from(label.to_string()))
        .child(text(SharedString::from(label.to_string())))
        .on_press(on_press)
}

#[component]
fn DetailsPanelFixture() -> impl IntoElement {
    details_fixture(false)
}

#[component]
fn PopoverFixture() -> impl IntoElement {
    details_fixture(true)
}

fn details_fixture(popover: bool) -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let store = t3_client::ClientStore::default();
    let first = EnvironmentId("first".into());
    let second = EnvironmentId("second".into());
    let session = store.begin_session(first.clone());
    let second_session = store.begin_session(second.clone());
    publish_shell(&store, &first, session, 1);
    publish_shell(&store, &second, second_session, 1);
    assert!(store.set_config(&first, session, config("first", "Build machine")));
    assert!(store.set_config(&second, second_session, config("second", "Other machine")));
    provide_context(Rt::new(
        store.clone(),
        Rc::new(DemoBackend::new(store.clone())),
        Choice::Demo,
    ));
    let key = ThreadKey {
        environment: first.clone(),
        thread: ThreadId("same-thread".into()),
    };
    let host = PanelHost::new(PanelStore::default(), |_| {});
    let selected = stored(key.clone());
    let inline = signal(true);
    let publishing = store.clone();
    let removing = store.clone();
    let details = if popover {
        thread_details_popover(
            host,
            key,
            nsh_base::button()
                .accessibility_label("Show details")
                .child(text("Show details")),
            600.0.into(),
            VersionControlSlots::default(),
        )
        .into_element()
    } else {
        thread_details_target(
            host,
            key,
            Prop::bind(move || inline.get()),
            600.0.into(),
            VersionControlSlots::default(),
        )
        .into_element()
    };
    view()
        .w_full()
        .h_full()
        .child(control("Toggle details", move || {
            host.toggle_thread_panel(&selected.get(), true)
        }))
        .child(control("Use popover mode", move || inline.set(false)))
        .child(control("Use inline mode", move || inline.set(true)))
        .child(control("Open popover flag", move || {
            host.apply(
                &selected.get(),
                Action::SetThreadPanel {
                    inline: false,
                    open: true,
                },
            );
        }))
        .child(control("Rename selected environment", move || {
            assert!(publishing.set_config(&first, session, config("first", "Renamed machine")));
        }))
        .child(control("Rename other environment", move || {
            assert!(store.set_config(&second, second_session, config("second", "Elsewhere")));
        }))
        .child(control("Remove selected environment", move || {
            removing.remove_environment(&EnvironmentId("first".into()))
        }))
        .child(view().w(px(280.0)).child(details))
}

fn fixture(window: (f32, f32)) -> Headless {
    Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<DetailsPanelFixture>())
    })
    .expect("details fixture starts")
}

#[test]
fn environment_publications_update_the_row_and_do_not_leak_across_environments() {
    for size in [DESKTOP, PHONE] {
        let mut app = fixture(size);
        app.wait_for("Build machine")
            .expect("client environment shown");
        app.press("role:button[Rename other environment]")
            .expect("publish unrelated config");
        app.press("role:button[Rename selected environment]")
            .expect("publish selected config");
        app.wait_for("Renamed machine")
            .expect("updated client name shown");
        assert!(!app.screen().shows("Build machine"));
        assert!(!app.screen().shows("Elsewhere"));
    }
}

#[test]
fn hiding_and_reopening_details_reads_the_latest_client_projection() {
    for size in [DESKTOP, PHONE] {
        let mut app = fixture(size);
        app.wait_for("Build machine")
            .expect("details default open inline");
        app.press("role:button[Toggle details]").expect("hide");
        assert!(!app.screen().shows("Build machine"));
        app.press("role:button[Rename selected environment]")
            .expect("publish while hidden");
        app.press("role:button[Toggle details]").expect("reopen");
        app.wait_for("Renamed machine")
            .expect("fresh projection after reopening");
    }
}

#[test]
fn removing_an_environment_removes_its_details_without_an_invented_fallback() {
    let mut app = fixture(DESKTOP);
    app.wait_for("Build machine").expect("details");
    app.press("role:button[Remove selected environment]")
        .expect("remove environment");
    app.wait_for("Thread details")
        .expect("target remains mounted");
    assert!(!app.screen().shows("Build machine"));
    assert!(!app.screen().shows("this machine"));
    assert!(!app.screen().shows("Open in editor"));
    assert!(!app.screen().shows("Add project script"));
}

#[test]
fn a_popover_is_closed_by_default_and_reopens_after_escape() {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::of::<PopoverFixture>())
        })
        .expect("popover fixture");
        app.wait_for("Show details").expect("anchor");
        assert!(!app.screen().shows("Build machine"));
        app.press("role:button[Show details]").expect("open");
        app.wait_for("Build machine").expect("details open");
        app.press_key("escape").expect("dismiss");
        assert!(!app.screen().shows("Build machine"));
        app.press("role:button[Show details]").expect("reopen");
        app.wait_for("Build machine").expect("details reopen");
    }
}

#[test]
fn returning_from_inline_does_not_reopen_a_stale_popover() {
    let mut app = fixture(DESKTOP);
    app.wait_for("Build machine").expect("inline initially");
    app.press("role:button[Use popover mode]")
        .expect("popover mode");
    assert!(!app.screen().shows("Build machine"));
    app.press("role:button[Open popover flag]")
        .expect("open popup");
    app.wait_for("Build machine").expect("popup content");
    app.press("role:button[Use inline mode]")
        .expect("switch inline");
    app.press("role:button[Use popover mode]")
        .expect("return to popup mode");
    assert!(!app.screen().shows("Build machine"));
}

// ---- Client services: editor picker, project scripts and item visibility, from fakes. ----

use std::cell::RefCell;
use std::collections::BTreeMap;

use futures::channel::mpsc;
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use t3_client::ClientError;
use t3_client::workspace::git_panel::editors::{
    EditorOption, EditorPickerSnapshot, RemoteOpenState,
};
use t3_client::workspace::git_panel::scripts::{NewProjectScriptInput, ProjectScriptsSnapshot};
use t3_client::workspace::ProjectKey;
use t3_contracts::editor::EditorId;
use t3_ui::screens::panel::details::{
    EditorPickerBackend, ProjectScriptsBackend, ThreadDetailsBackend, thread_details_runtime,
};

/// A value that every watcher receives first and again after each change.
struct Broadcast<T: Clone> {
    value: RefCell<T>,
    senders: RefCell<Vec<mpsc::UnboundedSender<T>>>,
}

impl<T: Clone + 'static> Broadcast<T> {
    fn new(value: T) -> Rc<Self> {
        Rc::new(Self {
            value: RefCell::new(value),
            senders: RefCell::default(),
        })
    }
    fn watch(&self) -> LocalBoxStream<'static, T> {
        let (sender, receiver) = mpsc::unbounded();
        let _ = sender.unbounded_send(self.value.borrow().clone());
        self.senders.borrow_mut().push(sender);
        receiver.boxed_local()
    }
    fn change(&self, f: impl FnOnce(&mut T)) {
        f(&mut self.value.borrow_mut());
        let value = self.value.borrow().clone();
        self.senders
            .borrow_mut()
            .retain(|s| s.unbounded_send(value.clone()).is_ok());
    }
    fn get(&self) -> T {
        self.value.borrow().clone()
    }
}

#[derive(Clone)]
struct FakeDetails {
    visibility: Rc<Broadcast<BTreeMap<String, bool>>>,
    editors: Rc<Broadcast<EditorPickerSnapshot>>,
    scripts: Rc<Broadcast<ProjectScriptsSnapshot>>,
    recorded: Signal<String>,
    refuse: Signal<bool>,
}

fn ready<T: 'static>(value: T) -> LocalBoxFuture<'static, T> {
    async move { value }.boxed_local()
}

impl FakeDetails {
    fn record(&self, value: String) {
        self.recorded.set(value);
    }
    fn refused(&self) -> Result<(), ClientError> {
        if self.refuse.get_untracked() {
            Err(ClientError::Rejected {
                message: "Settings write refused".into(),
                detail: None,
            })
        } else {
            Ok(())
        }
    }
}

impl ThreadDetailsBackend for FakeDetails {
    fn visibility(&self) -> LocalBoxStream<'static, BTreeMap<String, bool>> {
        self.visibility.watch()
    }
    fn set_item(&self, id: String, visible: bool) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.visibility.change(|v| {
            v.insert(id, visible);
        });
        ready(Ok(()))
    }
    fn reset_items(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.visibility.change(|v| {
            *v = t3_client::workspace::git_panel::details::DETAILS_ITEMS
                .iter()
                .map(|(id, _, visible)| ((*id).to_owned(), *visible))
                .collect()
        });
        ready(Ok(()))
    }
    fn editor_picker(&self, _: &ThreadKey) -> Rc<dyn EditorPickerBackend> {
        Rc::new(self.clone())
    }
    fn project_scripts(&self, _: &ThreadKey) -> Rc<dyn ProjectScriptsBackend> {
        Rc::new(self.clone())
    }
}

impl EditorPickerBackend for FakeDetails {
    fn watch(&self) -> LocalBoxStream<'static, Result<EditorPickerSnapshot, ClientError>> {
        self.editors.watch().map(Ok).boxed_local()
    }
    fn select_editor(&self, editor: EditorId) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.record(format!("preferred:{editor:?}"));
        self.editors.change(|s| s.preferred = Some(editor));
        ready(Ok(()))
    }
    fn open_editor(&self, editor: EditorId) -> LocalBoxFuture<'static, Result<bool, ClientError>> {
        self.record(format!("opened:{editor:?}:{}", self.editors.get().target_path.unwrap_or_default()));
        ready(Ok(true))
    }
}

impl ProjectScriptsBackend for FakeDetails {
    fn watch(&self) -> LocalBoxStream<'static, Result<ProjectScriptsSnapshot, ClientError>> {
        self.scripts.watch().map(Ok).boxed_local()
    }
    fn add(&self, input: NewProjectScriptInput) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.record(format!(
            "added:{}|{}|{:?}|keys:{:?}",
            input.script.name.0, input.script.command.0, input.script.icon, input.keybinding
        ));
        if let Err(error) = self.refused() {
            return ready(Err(error));
        }
        let script = t3_client::workspace::project_ops::scripts::build_project_script(
            t3_contracts::base_schemas::TrimmedNonEmptyString("added".into()),
            input.script,
        );
        self.scripts.change(|s| s.scripts.push(script));
        ready(Ok(()))
    }
    fn update(&self, id: String, input: NewProjectScriptInput) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.record(format!("updated:{id}:{}|keys:{:?}", input.script.name.0, input.keybinding));
        ready(self.refused())
    }
    fn delete(&self, id: String) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.record(format!("deleted:{id}"));
        self.scripts.change(|s| s.scripts.retain(|script| script.id.0 != id));
        ready(Ok(()))
    }
    fn import(&self, script: t3_contracts::t3_project_file::T3ProjectFileScript) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.record(format!("imported:{}", script.name.0));
        ready(self.refused())
    }
    fn run(&self, id: String) -> LocalBoxFuture<'static, Result<String, ClientError>> {
        self.record(format!("ran:{id}"));
        ready(Ok("script-terminal".into()))
    }
}

fn editor_snapshot(remote: RemoteOpenState) -> EditorPickerSnapshot {
    EditorPickerSnapshot {
        options: vec![
            EditorOption { editor: EditorId::Vscode, label: "VS Code".into(), icon_identity: "vscode".into() },
            EditorOption { editor: EditorId::Zed, label: "Zed".into(), icon_identity: "zed".into() },
        ],
        preferred: Some(EditorId::Vscode),
        can_open: remote != RemoteOpenState::RemoteUnavailable,
        remote,
        unavailable_reason: None,
        hint_seen: true,
        shortcut: None,
        target_path: Some("/repo".into()),
        environment_label: "Build machine".into(),
    }
}

fn scripts_snapshot(scripts: serde_json::Value, importable: serde_json::Value) -> ProjectScriptsSnapshot {
    ProjectScriptsSnapshot {
        project: ProjectKey {
            environment: EnvironmentId("first".into()),
            project: t3_client::types::ProjectId("project".into()),
        },
        scripts: serde_json::from_value(scripts).expect("scripts"),
        file_scripts: Vec::new(),
        importable_scripts: serde_json::from_value(importable).expect("t3.json scripts"),
        primary_script_id: None,
        preferred_script_id: None,
        can_run: true,
        can_edit: true,
        can_edit_keybindings: true,
        failed_import: None,
        file_error: None,
        keybindings: Rc::new(serde_json::from_value(json!([])).expect("keybindings")),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PropEnum)]
enum Services {
    #[default]
    Empty,
    WithScript,
    Imports,
    Unreachable,
}

#[derive(Clone, PartialEq, Props)]
struct ServicesFixtureProps {
    services: Services,
    height: f32,
}

#[component]
fn ServicesFixture(props: ServicesFixtureProps) -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let store = t3_client::ClientStore::default();
    let first = EnvironmentId("first".into());
    let session = store.begin_session(first.clone());
    publish_shell(&store, &first, session, 1);
    assert!(store.set_config(&first, session, config("first", "Build machine")));
    provide_context(Rt::new(store.clone(), Rc::new(DemoBackend::new(store.clone())), Choice::Demo));
    let fake = FakeDetails {
        visibility: Broadcast::new(
            t3_client::workspace::git_panel::details::DETAILS_ITEMS
                .iter()
                .map(|(id, _, visible)| ((*id).to_owned(), *visible))
                .collect(),
        ),
        editors: Broadcast::new(editor_snapshot(if props.services == Services::Unreachable {
            RemoteOpenState::RemoteUnavailable
        } else {
            RemoteOpenState::LocalExec
        })),
        scripts: Broadcast::new(match props.services {
            Services::WithScript => scripts_snapshot(
                json!([{"id":"test","name":"Test","command":"bun test","icon":"test","runOnWorktreeCreate":false}]),
                json!([]),
            ),
            Services::Imports => scripts_snapshot(json!([]), json!([{"name":"Lint","command":"bun lint","icon":"lint"}])),
            _ => scripts_snapshot(json!([]), json!([])),
        }),
        recorded: signal("no call".into()),
        refuse: signal(false),
    };
    provide_context(thread_details_runtime(Rc::new(fake.clone())));
    let key = ThreadKey { environment: first, thread: ThreadId("same-thread".into()) };
    let host = PanelHost::new(PanelStore::default(), |_| {});
    let revoke = fake.clone();
    let refuse = fake.clone();
    let hide_all = fake.clone();
    t3_ui::ui::toast::toast_provider(
        view()
            .w_full()
            .h_full()
            .child(control("Revoke run", move || revoke.scripts.change(|s| s.can_run = false)))
            .child(control("Refuse writes", move || refuse.refuse.set(true)))
            .child(control("Hide everything", move || {
                hide_all.visibility.change(|v| v.values_mut().for_each(|visible| *visible = false))
            }))
            .child(text(move || fake.recorded.get()))
            .child(view().w(px(280.0)).child(thread_details_target(
                host,
                key,
                true.into(),
                props.height.into(),
                VersionControlSlots::default(),
            ))),
        t3_ui::ui::toast::Position::TopRight,
    )
}

fn services(window: (f32, f32), services: Services, height: f32) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<ServicesFixture>(move || ServicesFixtureProps { services, height }))
    })
    .expect("services fixture starts");
    app.wait_for("no call").expect("details mounted");
    app
}

#[test]
fn open_in_opens_the_preferred_editor_and_the_menu_changes_it() {
    for window in [DESKTOP, PHONE] {
        let mut app = services(window, Services::Empty, 600.0);
        app.wait_for("role:button[Open in VS Code]").expect("client snapshot");
        app.press("role:button[Open in VS Code]").expect("primary");
        app.wait_for("text:opened:Vscode:/repo").expect("client opened the checkout");
        app.press("role:button[Choose editor]").expect("menu");
        app.press("role:menuitem[Zed]").expect("choose Zed");
        app.wait_for("text:preferred:Zed").expect("preference saved through the client");
        app.wait_for("role:button[Open in Zed]").expect("primary follows the preference");
    }
}

#[test]
fn an_unreachable_host_disables_open_and_says_why() {
    let mut app = services(DESKTOP, Services::Unreachable, 600.0);
    app.wait_for("role:button[Open in VS Code]").expect("primary");
    assert!(!app.screen().enabled("role:button[Open in VS Code]").expect("exists"));
    let _ = app.press("role:button[Open in VS Code]");
    assert!(app.screen().shows("no call"), "a disabled open never reaches the client");
    app.press("role:button[Choose editor]").expect("menu");
    app.wait_for("No SSH route to Build machine").expect("the web's reason");
    assert!(!app.screen().shows("Zed"), "no choices without a route");
}

#[test]
fn adding_a_script_saves_through_the_client_and_keeps_input_on_refusal() {
    for window in [DESKTOP, PHONE] {
        let mut app = services(window, Services::Empty, 600.0);
        app.press("role:button[Refuse writes]").expect("refuse");
        app.wait_for("role:button[Add project script]").expect("client snapshot");
        app.press("role:button[Add project script]").expect("add row");
        app.wait_for("Add Action").expect("editor dialog");
        app.press("role:button[Save action]").expect("save empty");
        app.wait_for("Name is required.").expect("validation");
        assert!(app.screen().shows("no call"));
        app.type_text("id:script-name", "Dev").expect("name");
        app.type_text("id:script-command", "bun dev").expect("command");
        app.press("role:button[Save action]").unwrap_or_else(|e| panic!("save at {window:?}: {e}"));
        app.wait_for("text:added:Dev|bun dev|Play|keys:Some(None)").expect("canonical input");
        app.wait_for("Settings write refused").expect("server error in the dialog");
        assert_eq!(app.screen().input_text("id:script-name"), Ok(Some("Dev")));
    }
}

#[test]
fn a_saved_script_becomes_the_run_control_and_closes_the_dialog() {
    let mut app = services(DESKTOP, Services::Empty, 600.0);
    app.wait_for("role:button[Add project script]").expect("client snapshot");
    app.press("role:button[Add project script]").expect("add row");
    app.type_text("id:script-name", "Dev").expect("name");
    app.type_text("id:script-command", "bun dev").expect("command");
    app.press("role:button[Save action]").expect("save");
    app.wait_for("role:button[Run Dev]").expect("primary script row");
    app.wait_for_gone("Add Action", nsh_testing::WAIT)
        .expect("a successful save closes the dialog");
    app.press("role:button[Run Dev]").expect("run");
    app.wait_for("text:ran:added").expect("client opened a terminal");
}

#[test]
fn running_needs_the_terminal_grant_and_edit_and_delete_go_through_the_client() {
    let mut app = services(DESKTOP, Services::WithScript, 600.0);
    app.press("role:button[Revoke run]").expect("grant lost");
    let _ = app.press("role:button[Run Test]");
    assert!(app.screen().shows("no call"), "aria-disabled run is inert");
    app.press("role:button[Script actions]").expect("menu");
    app.press("role:button[Edit Test]").expect("edit");
    app.wait_for("Edit Action").expect("edit dialog");
    assert!(app.screen().shows("no call"), "editing does not run the script");
    assert_eq!(app.screen().input_text("id:script-command"), Ok(Some("bun test")));
    app.press("role:button[Delete]").expect("delete");
    app.wait_for("This action cannot be undone.").expect("confirmation");
    app.press("role:button[Delete action]").expect("confirm");
    app.wait_for("text:deleted:test").expect("client delete");
    app.wait_for("role:button[Add project script]").expect("empty project again");
}

#[test]
fn a_failed_t3_json_import_reopens_the_editor_with_its_payload() {
    let mut app = services(DESKTOP, Services::Imports, 600.0);
    app.press("role:button[Refuse writes]").expect("refuse");
    app.press("role:button[Choose project action]").expect("actions menu");
    app.wait_for("From t3.json").expect("import group");
    app.press("role:menuitem[Lint]").expect("import");
    app.wait_for("text:imported:Lint").expect("client import");
    app.wait_for("Add Action").expect("editor with the failed import");
    assert_eq!(app.screen().input_text("id:script-command"), Ok(Some("bun lint")));
}

#[test]
fn an_emptied_card_offers_choose_what_to_show_and_refills_from_the_menu() {
    let mut app = services(DESKTOP, Services::Empty, 600.0);
    assert!(!app.screen().shows("Choose what to show"));
    app.press("role:button[Hide everything]").expect("hide");
    app.wait_for("role:button[Choose what to show]").expect("refill row");
    assert!(!app.screen().shows("Open in VS Code"));
    app.press("role:button[Choose what to show]").expect("menu");
    app.press("role:menuitemcheckbox[Open in editor]").expect("show open in");
    app.wait_for("role:button[Open in VS Code]").expect("row back");
}

#[test]
fn a_short_card_drops_the_environment_row_then_open_in() {
    let mut app = services(DESKTOP, Services::Empty, 600.0);
    app.wait_for("role:button[Open in VS Code]").expect("full density");
    let mut short = services(DESKTOP, Services::Empty, 60.0);
    short.wait_for("role:button[Add project script]").expect("scripts stay at every density");
    assert!(!short.screen().shows("Build machine"), "compact drops the environment row");
    assert!(!short.screen().shows("Open in VS Code"), "essential drops open in");
}
