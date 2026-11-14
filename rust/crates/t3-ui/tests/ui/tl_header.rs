//! ChatHeader interactions use the real demo shell and the shared sidebar menu/input.

use crate::support::{DESKTOP, PHONE};
use futures::{
    FutureExt, StreamExt, channel::mpsc, future::LocalBoxFuture, stream::LocalBoxStream,
};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use std::{cell::RefCell, rc::Rc};
use t3_client::{
    ClientStore,
    connection::PairingInput,
    state::selectors::thread_shell,
    types::{EnvironmentId, ThreadKey},
    workspace::ProjectKey,
};
use t3_contracts::orchestration_v2::{
    AppThreadLineageRelationshipToParent, OrchestrationV2ThreadShell,
};
use t3_ui::{
    i18n::Text,
    runtime::{
        Rt,
        backend::Backend,
        demo::{self, DemoBackend},
        host::Choice,
    },
    screens::{
        new_thread::Destination,
        sidebar::actions::ThreadActionMenu,
        thread::header::{
            self, HeaderError, ThreadHeaderNavigation, ThreadHeaderNavigationService,
            ThreadHeaderRuntime, ThreadHeaderService, ThreadHeaderState,
        },
    },
    state::nav::{Navigator, Route},
    ui::toast,
};

const TITLE_BUTTON: &str = "role:button[Thread actions for Build the shared Rust UI slice]";

#[derive(Clone)]
struct Client {
    store: ClientStore,
    state: Rc<RefCell<ThreadHeaderState>>,
    listeners: Rc<RefCell<Vec<mpsc::UnboundedSender<Rc<ThreadHeaderState>>>>>,
    calls: Signal<Vec<String>>,
    fail: Rc<RefCell<Option<HeaderError>>>,
}

fn key() -> ThreadKey {
    demo::demo_thread(&EnvironmentId("demo-demo.t3.codes".into()))
}

impl Client {
    fn guest(&self, share: Option<t3_contracts::guest_share::ThreadGuestShare>) {
        let key = key();
        let session = self.store.snapshot().environments[&key.environment].session;
        self.store.update_thread(&key, session, |state| {
            let projection = state.projection.as_mut().expect("real thread projection");
            Rc::make_mut(projection).thread.guest_share = Some(share);
            true
        });
    }
    fn publish(&self, update: impl FnOnce(&mut OrchestrationV2ThreadShell)) {
        let key = key();
        let (session, sequence, mut thread) = {
            let snapshot = self.store.snapshot();
            let environment = &snapshot.environments[&key.environment];
            (
                environment.session,
                self.store
                    .shared_shell(&key.environment)
                    .expect("shell")
                    .snapshot_sequence
                    + 1,
                thread_shell(&environment.shell, &key.thread)
                    .expect("canonical thread")
                    .clone(),
            )
        };
        update(&mut thread);
        let event = serde_json::from_value(serde_json::json!({"kind":"thread.updated","sequence":sequence,"location":"active","thread":thread})).expect("wire event");
        self.store
            .apply_shell(&key.environment, session, vec![event]);
    }

    fn change(&self, update: impl FnOnce(&mut ThreadHeaderState)) {
        update(&mut self.state.borrow_mut());
        let next = Rc::new(self.state.borrow().clone());
        self.listeners
            .borrow_mut()
            .retain(|listener| listener.unbounded_send(next.clone()).is_ok());
    }
}

impl ThreadHeaderService for Client {
    fn watch(&self, _: ThreadKey) -> LocalBoxStream<'static, Rc<ThreadHeaderState>> {
        let (sender, receiver) = mpsc::unbounded();
        self.listeners.borrow_mut().push(sender);
        futures::stream::once(futures::future::ready(Rc::new(self.state.borrow().clone())))
            .chain(receiver)
            .boxed_local()
    }
    fn rename_thread(
        &self,
        _: ThreadKey,
        title: String,
    ) -> LocalBoxFuture<'static, Result<(), HeaderError>> {
        let this = self.clone();
        async move {
            if let Some(error) = this.fail.borrow_mut().take() {
                return Err(error);
            }
            this.calls
                .update(|calls| calls.push(format!("rename:{title}")));
            this.publish(|thread| thread.title = title);
            Ok(())
        }
        .boxed_local()
    }
    fn perform_action(
        &self,
        _: ThreadKey,
        action: String,
    ) -> LocalBoxFuture<'static, Result<(), HeaderError>> {
        self.calls.update(|calls| calls.push(action));
        futures::future::ready(Ok(())).boxed_local()
    }
    fn close_guest_share(&self, _: ThreadKey) -> LocalBoxFuture<'static, Result<(), HeaderError>> {
        self.calls.update(|calls| calls.push("stop-sharing".into()));
        self.guest(None);
        futures::future::ready(Ok(())).boxed_local()
    }
}

impl ThreadHeaderNavigationService for Client {
    fn watch(&self, _: ProjectKey) -> LocalBoxStream<'static, Option<Text>> {
        futures::stream::once(futures::future::ready(None))
            .chain(futures::stream::pending())
            .boxed_local()
    }
    fn new_thread_in_project(
        &self,
        project: ProjectKey,
    ) -> LocalBoxFuture<'static, Result<Destination, HeaderError>> {
        self.calls.update(|calls| {
            calls.push(format!(
                "new:{}:{}",
                project.environment.0, project.project.0
            ))
        });
        futures::future::ready(Ok(Destination::Draft("project-draft".into()))).boxed_local()
    }
}

fn control(label: &'static str, press: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(label)
        .on_press(press)
        .child(text(label))
}

fn client(store: ClientStore) -> Client {
    Client {
        store,
        state: Rc::new(RefCell::new(ThreadHeaderState {
            can_operate: true, unavailable_reason: None, is_scratch: Some(false),
            menu: Some(ThreadActionMenu { unavailable_reason: None, items: serde_json::from_value(serde_json::json!([
                {"id":"rename","label":"Rename thread"}, {"id":"copy-thread-id","label":"Thread ID"}
            ])).expect("canonical menu items") }),
        })),
        listeners: Rc::default(), calls: signal(vec![]), fail: Rc::default(),
    }
}

fn fixture(bound: bool) -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let store = ClientStore::default();
    let backend: Rc<dyn Backend> = Rc::new(DemoBackend::new(store.clone()));
    provide_context(Rt::new(store.clone(), backend.clone(), Choice::Demo));
    let nav = Navigator::new(Route::Thread(key()));
    provide_context(nav);
    let panels = t3_ui::state::panels::Panels::new();
    provide_context(panels);
    t3_ui::state::clock::start();
    let client = client(store);
    if bound {
        provide_context(ThreadHeaderRuntime(Rc::new(client.clone())));
        provide_context(ThreadHeaderNavigation(Rc::new(client.clone())));
    }
    let ready = signal(false);
    spawn(async move {
        backend
            .pair(PairingInput {
                pairing_url: Some(demo::DEMO_PAIRING_URL.into()),
                ..Default::default()
            })
            .await
            .expect("seed real shell");
        backend.open_thread(&key());
        ready.set(true);
    });
    let concurrent = client.clone();
    let failure = client.clone();
    let interrupt = client.clone();
    let readonly = client.clone();
    let scratch = client.clone();
    let parent = client.clone();
    let share = client.clone();
    let expired_share = client.clone();
    let calls = client.calls;
    toast::toast_provider(
        view().w_full().h_full()
            .child(when(ready, || header::thread_header(key())))
            .child(control("Outside rename", || {}))
            .child(control("Toggle sidebar fixture", move || panels.sidebar_open.update(|open| *open = !*open)))
            .child(nsh_base::input().label("Another field"))
            .child(nsh_base::button().test_id("server-update").accessibility_label("Concurrent title").on_press(move || concurrent.publish(|thread| thread.title = "Concurrent server title".into())))
            .child(control("Fail next rename", move || *failure.fail.borrow_mut() = Some(HeaderError::Failed { title: "Failed to rename thread".into(), detail: Some(Text::content("Server rejected this name")) })))
            .child(control("Interrupt next rename", move || *interrupt.fail.borrow_mut() = Some(HeaderError::Interrupted)))
            .child(nsh_base::button().test_id("revoke-permission").accessibility_label("Revoke permission").on_press(move || readonly.change(|state| { state.can_operate = false; if let Some(menu) = &mut state.menu { menu.items[0].disabled = Some(true); } })))
            .child(control("Scratch project", move || scratch.change(|state| state.is_scratch = Some(true))))
            .child(control("Parent link", move || parent.publish(|thread| {
                thread.lineage.parent_thread_id = Some(t3_client::types::ThreadId("thread-timeline-perf".into()));
                thread.lineage.relationship_to_parent = Some(AppThreadLineageRelationshipToParent::Subagent);
            })))
            .child(control("Guest link", move || share.guest(Some(serde_json::from_value(serde_json::json!({"id":"guest-share","host":"test","createdAt":"2026-01-01T00:00:00.000Z","expiresAt":"2099-01-01T00:00:00.000Z","guestName":"Alex","messages":[]})).expect("guest share")))))
            .child(control("Expired guest link", move || expired_share.guest(Some(serde_json::from_value(serde_json::json!({"id":"guest-share","host":"test","createdAt":"2020-01-01T00:00:00.000Z","expiresAt":"2020-01-01T00:00:00.000Z","guestName":"Alex","messages":[]})).expect("expired share")))))
            .child(text(move || format!("calls:{}", calls.get().join("|"))))
            .child(text(move || format!("route:{}", nav.top().path()))),
        toast::Position::TopRight,
    )
}

#[component]
fn HeaderFixture() -> impl IntoElement {
    fixture(true)
}
#[component]
fn UnboundHeader() -> impl IntoElement {
    fixture(false)
}

#[component]
fn ProductionHeader() -> impl IntoElement {
    let client = client(ClientStore::default());
    *client.fail.borrow_mut() = Some(HeaderError::Failed {
        title: "Failed to rename thread".into(),
        detail: Some(Text::content("Server rejected this name")),
    });
    provide_context(ThreadHeaderRuntime(Rc::new(client)));
    t3_ui::app::app()
}

fn start<C: Component<Props = ()>>(window: (f32, f32)) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<C>())
    };
    config.environment.idiom = if window == PHONE {
        nsh_core::platform::Idiom::Phone
    } else {
        nsh_core::platform::Idiom::Desktop
    };
    config.environment.os = if window == PHONE {
        nsh_core::platform::Os::Android
    } else {
        nsh_core::platform::Os::Web
    };
    let mut app = Headless::start(config).expect("header fixture");
    app.wait_for(TITLE_BUTTON).expect("canonical title button");
    app
}

fn rename(app: &mut Headless) {
    app.press(TITLE_BUTTON)
        .expect("open thread actions immediately");
    app.wait_for("role:menuitem[Rename thread]")
        .expect("shared menu");
    app.press("role:menuitem[Rename thread]")
        .expect("start shared rename input");
    app.wait_for("label:Thread title").expect("rename input");
}

fn publish_event(app: &mut Headless, test_id: &str) {
    let id = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some(test_id))
        .expect("server event control")
        .id;
    // A server publication does not change the editor's native focus.
    app.engine().control().dispatch_event(
        id,
        nsh_core::EventKind::Press,
        nsh_core::EventPayload::Press(Default::default()),
    );
    assert!(app.settle(), "server publication applied");
}

#[test]
fn title_click_opens_shared_actions_and_untouched_rename_preserves_concurrent_updates() {
    for window in [DESKTOP, PHONE] {
        for enter in [true, false] {
            let mut app = start::<HeaderFixture>(window);
            rename(&mut app);
            publish_event(&mut app, "server-update");
            if enter {
                app.press_key("Enter").expect("finish untouched rename");
            } else {
                app.type_text("label:Another field", "x")
                    .expect("blur untouched rename");
            }
            app.wait_for("role:button[Thread actions for Concurrent server title]")
                .unwrap_or_else(|error| {
                    panic!("server title preserved at {window:?}, Enter={enter}: {error}")
                });
            assert!(app.screen().shows("calls:"));
            assert!(!app.screen().shows("rename:"));
        }
    }
}

#[test]
fn rename_trims_commits_cancels_and_surfaces_empty_or_failed_values() {
    let mut app = start::<HeaderFixture>(DESKTOP);
    rename(&mut app);
    app.clear("label:Thread title").expect("replace title");
    app.type_text("label:Thread title", "   ")
        .expect("empty title");
    app.press_key("Enter").expect("reject empty");
    app.wait_for("Thread title cannot be empty")
        .expect("source warning");
    rename(&mut app);
    app.clear("label:Thread title").expect("replace title");
    app.type_text("label:Thread title", "  My edited title  ")
        .expect("edit title");
    app.press_key("Escape").expect("cancel");
    app.wait_for(TITLE_BUTTON).expect("original title");
    app.press("role:button[Fail next rename]")
        .expect("server failure");
    rename(&mut app);
    app.clear("label:Thread title").expect("replace title");
    app.type_text("label:Thread title", "Rejected name")
        .expect("edit");
    app.press_key("Enter").expect("submit");
    app.wait_for("Failed to rename thread")
        .expect("source failure toast");
    app.wait_for("Server rejected this name")
        .expect("literal server detail");
    rename(&mut app);
    app.clear("label:Thread title").expect("replace title");
    app.type_text("label:Thread title", "  My edited title  ")
        .expect("edit");
    app.press_key("Enter").expect("commit");
    app.wait_for("role:button[Thread actions for My edited title]")
        .expect("trimmed persisted title");
    app.wait_for("text:calls:rename:My edited title")
        .expect("exactly one commit");
}

#[test]
fn missing_service_and_revoked_permission_never_dispatch_mutations() {
    let app = start::<UnboundHeader>(DESKTOP);
    assert!(
        app.screen()
            .accessibility_state(TITLE_BUTTON)
            .expect("title")
            .disabled
    );
    assert!(
        app.screen()
            .accessibility_state("role:button[New thread in t3code]")
            .expect("project")
            .disabled
    );
    let mut app = start::<HeaderFixture>(DESKTOP);
    rename(&mut app);
    app.clear("label:Thread title").expect("replace title");
    app.type_text("label:Thread title", "Revoked edit")
        .expect("edit");
    publish_event(&mut app, "revoke-permission");
    app.wait_for(TITLE_BUTTON).expect("editor removed");
    app.press_key("Enter").expect("no stale submit");
    assert!(!app.screen().shows("rename:"));
}

#[test]
fn scratch_identity_project_navigation_and_guest_share_use_the_bound_services() {
    for window in [DESKTOP, PHONE] {
        let mut app = start::<HeaderFixture>(window);
        app.press("role:button[Scratch project]")
            .expect("canonical scratch update");
        app.wait_for("role:button[New thread in No project]")
            .expect("source scratch label");
        app.press("role:button[Guest link]")
            .expect("guest projection");
        app.wait_for("Alex is a guest").expect("source guest state");
        if window == DESKTOP {
            app.press(TITLE_BUTTON)
                .expect("title remains reachable beside guest controls");
            app.press_key("Escape").expect("close title actions");
        }
        app.press("role:button[Expired guest link]")
            .expect("expired projection");
        app.wait_for("Guest link expired")
            .expect("expired share remains visible");
        app.press("role:button[Stop sharing]")
            .unwrap_or_else(|error| {
                panic!(
                    "close share at {window:?}: {error}; bounds: {:?}",
                    app.screen()
                        .layout_nodes()
                        .into_iter()
                        .filter(|node| node.bounds.origin.y.0 < 52.0)
                        .map(|node| (node.id, node.parent, node.test_id, node.bounds))
                        .collect::<Vec<_>>()
                )
            });
        app.wait_for("text:calls:stop-sharing")
            .expect("client close operation");
        assert!(!app.screen().shows("Alex is a guest"));
        app.press("role:button[New thread in No project]")
            .expect("project-scoped new draft");
        app.wait_for("text:route:/draft/project-draft")
            .expect("real destination route");
        app.wait_for("text:calls:stop-sharing|new:demo-demo.t3.codes:project-t3code")
            .expect("physical project identity");
    }
}

#[test]
fn an_edited_rename_wins_over_a_concurrent_server_title() {
    for window in [DESKTOP, PHONE] {
        let mut app = start::<HeaderFixture>(window);
        rename(&mut app);
        app.clear("label:Thread title").expect("replace title");
        app.type_text("label:Thread title", "My edited title")
            .expect("user edit");
        publish_event(&mut app, "server-update");
        app.press_key("Enter").expect("commit user edit");
        app.wait_for("role:button[Thread actions for My edited title]")
            .expect("edited value preserved");
        app.wait_for("text:calls:rename:My edited title")
            .expect("one mutation");
    }
}

#[test]
fn double_click_uses_native_click_data_and_ime_escape_does_not_cancel_early() {
    let mut app = start::<HeaderFixture>(DESKTOP);
    let id = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some("thread-title-actions"))
        .expect("actual title control")
        .id;
    app.engine().control().dispatch_event(
        id,
        nsh_core::EventKind::Press,
        nsh_core::EventPayload::Press(nsh_core::PressEvent {
            clicks: 2,
            position: nsh_core::geometry::point(px(2.0), px(2.0)),
            ..Default::default()
        }),
    );
    assert!(app.settle());
    app.wait_for("label:Thread title")
        .expect("double-click rename");
    app.compose("label:Thread title", "候補")
        .expect("IME composition");
    app.press_key("Escape").expect("IME owns escape");
    assert!(
        app.screen()
            .matches("label:Thread title")
            .expect("still editing")
    );
    app.compose("label:Thread title", "")
        .expect("composition ends");
    app.press_key("Escape").expect("cancel rename");
    app.wait_for(TITLE_BUTTON).expect("title restored");
    assert!(!app.screen().shows("rename:"));
}

#[test]
fn interrupted_rename_does_not_report_an_error() {
    let mut app = start::<HeaderFixture>(DESKTOP);
    app.press("role:button[Interrupt next rename]")
        .expect("interruption");
    rename(&mut app);
    app.clear("label:Thread title").expect("replace title");
    app.type_text("label:Thread title", "Interrupted title")
        .expect("edit");
    app.press_key("Enter").expect("submit");
    app.wait_for(TITLE_BUTTON)
        .expect("original persisted title");
    assert!(!app.screen().shows("Failed to rename thread"));
    assert!(!app.screen().shows("rename:"));
}

#[test]
fn title_context_menu_and_keyboard_use_the_shared_action_dispatch() {
    let mut app = start::<HeaderFixture>(DESKTOP);
    app.right_click(TITLE_BUTTON).expect("header context menu");
    app.wait_for("role:menuitem[Thread ID]")
        .expect("shared context action");
    app.press("role:menuitem[Thread ID]").expect("copy action");
    app.wait_for("text:calls:copy-thread-id")
        .expect("client action dispatch");
    app.press(TITLE_BUTTON).expect("focus title");
    app.press_key("Escape").expect("close popup");
    app.press_key("Enter").expect("keyboard title activation");
    app.wait_for("role:menuitem[Rename thread]")
        .expect("keyboard action menu");
}

#[test]
fn production_app_mounts_the_header_rename_failure_toast() {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(DESKTOP.0), px(DESKTOP.1)),
        ..EngineConfig::new(Root::of::<ProductionHeader>().url_schemes(["t3code"]))
    };
    config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    config.environment.os = nsh_core::platform::Os::Web;
    let mut app =
        Headless::start_with_link(config, crate::support::DEMO_LINK).expect("production App");
    app.wait_for("Connect to T3 Code").expect("pairing screen");
    app.press("role:button[Fill in the demo server]")
        .expect("demo link");
    app.type_text("label:Pairing link", "")
        .expect("focus pairing input");
    app.press_key("Enter").expect("submit pairing input");
    app.wait_for("Build the shared Rust UI slice")
        .expect("paired workspace");
    app.press("role:button[Build the shared Rust UI slice, t3code]")
        .expect("open thread");
    app.wait_for(TITLE_BUTTON).expect("production header");
    rename(&mut app);
    app.clear("label:Thread title").expect("replace title");
    app.type_text("label:Thread title", "Rejected name")
        .expect("edit");
    app.press_key("Enter").expect("submit");
    app.wait_for("Failed to rename thread")
        .expect("production toast host");
    app.wait_for("Server rejected this name")
        .expect("server detail");
}

#[test]
fn parent_collapses_from_measured_room_and_keeps_its_full_navigation_label() {
    let mut app = start::<HeaderFixture>(PHONE);
    app.press("role:button[Parent link]")
        .expect("subagent lineage");
    app.wait_for("id:parent-breadcrumb-ellipsis")
        .expect("narrow parent collapsed");
    app.wait_for("role:button[Open parent thread: Timeline virtualisation with long code blocks]")
        .expect("full accessible parent label");
    app.set_viewport(size(px(DESKTOP.0), px(DESKTOP.1)))
        .expect("wide room");
    assert!(
        !app.screen()
            .matches("id:parent-breadcrumb-ellipsis")
            .expect("parent expands")
    );
}

#[test]
fn native_titlebar_insets_keep_header_controls_clear_when_wide_narrow_and_collapsed() {
    use nsh_core::{
        desktop::WindowState,
        geometry::Edges,
        platform::{Idiom, Os},
    };
    for (os, top, left, right) in [
        (Os::MacOs, 52.0, 78.0, 0.0),
        (Os::Windows, 40.0, 0.0, 138.0),
    ] {
        for width in [DESKTOP.0, 700.0] {
            let mut config = EngineConfig {
                ui_sets: t3_ui::registries(),
                viewport: size(px(width), px(DESKTOP.1)),
                ..EngineConfig::new(Root::of::<HeaderFixture>())
            };
            config.environment.os = os;
            config.environment.idiom = Idiom::Desktop;
            config.environment.title_bar = Edges {
                top: px(top),
                left: px(left),
                right: px(right),
                bottom: px(0.0),
            };
            config.environment.window_state = Some(WindowState::default());
            let mut app = Headless::start(config).expect("native header");
            app.wait_for(TITLE_BUTTON).expect("actual title button");
            let bounds = |app: &Headless, id: &str| {
                app.screen()
                    .layout_nodes()
                    .into_iter()
                    .find(|node| node.test_id.as_deref() == Some(id))
                    .expect("header element")
                    .bounds
            };
            for collapsed in [false, true] {
                if collapsed {
                    app.press("role:button[Toggle sidebar fixture]")
                        .expect("collapse sidebar");
                }
                let content_left = if collapsed || width < 768.0 {
                    left + 52.0
                } else {
                    12.0
                };
                let title = bounds(&app, "thread-title-actions");
                let controls = bounds(&app, "thread-panel-layout-controls");
                assert!(title.size.width.0 > 0.0);
                assert!(controls.size.width.0 > 48.0);
                assert!(
                    title.origin.x.0 >= content_left,
                    "{os:?}, width={width}, collapsed={collapsed}: {title:?}"
                );
                assert!(
                    controls.origin.x.0 + controls.size.width.0 <= width - right - 12.0,
                    "{os:?}, width={width}, collapsed={collapsed}: {controls:?}"
                );
                assert_eq!(bounds(&app, "workspace-page-header").size.height, px(top));
            }
        }
    }
}

#[component]
fn ReactiveReservation() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    provide_context(t3_ui::state::panels::Panels::new());
    let reserve = signal(true);
    t3_ui::screens::titlebar::workspace_page_header_with_reservation(
        Prop::bind(move || reserve.get()),
        view()
            .flex_1()
            .child(t3_ui::screens::titlebar::titlebar_controls(
                nsh_base::button()
                    .test_id("reservation-control")
                    .accessibility_label("Toggle reservation")
                    .w_full()
                    .h(px(28.0))
                    .on_press(move || reserve.update(|value| *value = !*value)),
            )),
    )
}

#[test]
fn pane_reservation_changes_without_recreating_the_header_control() {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(DESKTOP.0), px(DESKTOP.1)),
        ..EngineConfig::new(Root::of::<ReactiveReservation>())
    };
    config.environment.os = nsh_core::platform::Os::Windows;
    config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    config.environment.title_bar = nsh_core::geometry::Edges {
        top: px(40.0),
        right: px(138.0),
        ..Default::default()
    };
    config.environment.window_state = Some(Default::default());
    let mut app = Headless::start(config).expect("reactive reservation");
    app.wait_for("id:reservation-control").expect("control");
    let node = |app: &Headless| {
        app.screen()
            .layout_nodes()
            .into_iter()
            .find(|node| node.test_id.as_deref() == Some("reservation-control"))
            .expect("control node")
    };
    let reserved = node(&app);
    app.press("id:reservation-control")
        .expect("pane owns native controls");
    let released = node(&app);
    assert_eq!(reserved.id, released.id);
    assert_eq!(
        released.bounds.size.width - reserved.bounds.size.width,
        px(138.0)
    );
    app.press("id:reservation-control")
        .expect("header owns native controls again");
    assert_eq!(node(&app).bounds, reserved.bounds);
}
