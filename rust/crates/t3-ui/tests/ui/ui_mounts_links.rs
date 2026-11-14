//! Round 2 of the app-root mounts (review `t3-platform-rereview-2` A3): every address the UI opens
//! outside the app goes through the client's policy. The client runs over a platform that records
//! what the host is asked to open and reports the desktop surface (the widest policy); the opener
//! comes from `app::provide_client_services`, as in the app.

use std::cell::RefCell;
use std::rc::Rc;

use futures::stream::LocalBoxStream;
use futures::{FutureExt, StreamExt, future::LocalBoxFuture};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use t3_client::ClientStore;
use t3_client::composition::ClientRuntime;
use t3_client::persistence::external_links::ExternalUrlSurface;
use t3_client::platform::{
    HostAction, HostFuture, HostReply, HttpRequest, HttpResponse, Platform, PlatformError,
    PlatformEvent, SocketRequest, Storage, StorageArea, StorageKey, StorageTransaction,
    StorageWrite, WebSocket,
};
use t3_client::services::resources::{Resource, ResourcePolicy};
use t3_client::testing::FakePlatform;
use t3_client::types::{EnvironmentId, Generation, ThreadId, ThreadKey};
use t3_client::ClientError;
use t3_contracts::git::*;
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_contracts::source_control::*;
use t3_ui::screens::panel::git_actions::*;

use crate::support::{DESKTOP, PHONE};

/// The review's five: only the last is a link the policy opens.
const ADDRESSES: [&str; 5] = [
    "search-ms:query=x",
    "vscode://file//server/share/x",
    "file:///c:/x",
    "javascript:alert(1)",
    "https://github.com/acme/web",
];
const SAFE: &str = "https://github.com/acme/web";

/// `FakePlatform` with the desktop's link surface, recording what the host is asked to open.
#[derive(Clone)]
struct Recording {
    fake: FakePlatform,
    opened: Rc<RefCell<Vec<String>>>,
}

impl Storage for Recording {
    fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
        self.fake.get(area, key)
    }
    fn write(&self, area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()> {
        self.fake.write(area, writes)
    }
    fn list(&self, area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>> {
        self.fake.list(area, namespace)
    }
    fn transaction(&self, transaction: StorageTransaction) -> HostFuture<'_, bool> {
        self.fake.transaction(transaction)
    }
}

impl Platform for Recording {
    fn now_epoch_ms(&self) -> u64 {
        self.fake.now_epoch_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.fake.monotonic_ms()
    }
    fn random_bytes(&self, length: usize) -> Result<Vec<u8>, PlatformError> {
        self.fake.random_bytes(length)
    }
    fn sleep(&self, milliseconds: u64) -> LocalBoxFuture<'_, ()> {
        self.fake.sleep(milliseconds)
    }
    fn yield_to_host(&self) -> LocalBoxFuture<'_, ()> {
        self.fake.yield_to_host()
    }
    fn events(&self) -> LocalBoxStream<'static, PlatformEvent> {
        self.fake.events()
    }
    fn http(&self, request: HttpRequest) -> HostFuture<'_, HttpResponse> {
        self.fake.http(request)
    }
    fn websocket(&self, request: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>> {
        self.fake.websocket(request)
    }
    fn host(&self, action: HostAction) -> HostFuture<'_, HostReply> {
        match action {
            HostAction::OpenUrl(url) | HostAction::OpenEditorLink(url) => {
                self.opened.borrow_mut().push(url);
                async { Ok(HostReply::Unit) }.boxed_local()
            }
            other => self.fake.host(other),
        }
    }
    fn external_url_surface(&self) -> ExternalUrlSurface {
        ExternalUrlSurface::Desktop
    }
}

/// The app root's services over the recording client, and a line showing what was opened.
fn provide_recording_client(with_runtime: bool) -> (Rc<RefCell<Vec<String>>>, AnyElement) {
    let opened = Rc::new(RefCell::new(Vec::new()));
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        Recording {
            fake: FakePlatform::default(),
            opened: opened.clone(),
        },
        store.clone(),
        OrchestrationV2CreationSource::Web,
        Rc::new(|future| {
            spawn(future);
        }),
    );
    if with_runtime {
        let backend: Rc<dyn t3_ui::runtime::backend::Backend> = Rc::new(client.clone());
        provide_context(t3_ui::runtime::Rt::new(
            store,
            backend,
            t3_ui::runtime::host::Choice::Client,
        ));
        provide_context(t3_ui::state::nav::Navigator::new(
            t3_ui::state::nav::Route::Home,
        ));
    }
    t3_ui::app::provide_client_services(&client);
    let shown = signal(String::from("opened: unread"));
    let reading = opened.clone();
    let probe = nsh_base::button()
        .accessibility_label(Some(SharedString::from("Read opened")))
        .on_press(move || shown.set(format!("opened: {:?}", reading.borrow())))
        .child(text("Read opened"))
        .into_element();
    let line = view()
        .child(probe)
        .child(text(move || shown.get()))
        .into_element();
    (opened, line)
}

fn opened_after_settling(app: &mut Headless) -> String {
    app.settle();
    app.press("role:button[Read opened]").expect("probe");
    app.screen()
        .texts()
        .into_iter()
        .find(|text| text.starts_with("opened: ") && *text != "opened: unread")
        .map(str::to_owned)
        .expect("probe line")
}

fn start(window: (f32, f32), root: Root) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(root)
    };
    if window == DESKTOP {
        config.environment.os = nsh_core::platform::Os::Linux;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    let mut app = Headless::start(config).expect("fixture");
    app.wait_for("role:button[Read opened]").expect("mounted");
    app
}

// ---- The git publish result ("Open on GitHub") ----

fn scope() -> GitActionsScope {
    GitActionsScope {
        thread: ThreadKey {
            environment: EnvironmentId("env".into()),
            thread: ThreadId("thread".into()),
        },
        cwd: "/project".into(),
        draft_id: None,
    }
}

fn snapshot() -> GitActionsSnapshot {
    let status: VcsStatusResult = serde_json::from_value(json!({"isRepo":true,"hasPrimaryRemote":false,"isDefaultRef":true,"refName":"main","hasWorkingTreeChanges":false,
        "workingTree":{"rootPath":"/project","files":[],"insertions":0,"deletions":0},"hasUpstream":false,"aheadCount":0,"behindCount":0,"pr":null}))
        .expect("status");
    GitActionsSnapshot {
        status: Some(Rc::new(status)),
        can_write: true,
        can_operate: true,
        has_server_thread: true,
        publish_providers: vec![PublishProvider {
            kind: "github".into(),
            label: "GitHub".into(),
            host: "github.com".into(),
            path_placeholder: "owner/repo".into(),
            icon_svg: "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\"><path d=\"M2 2h20v20H2z\"/></svg>".into(),
        }],
        discovery: Some(Rc::new(serde_json::from_value(json!({"versionControlSystems":[],"sourceControlProviders":[{
            "kind":"github","label":"GitHub","status":"available","version":{"_tag":"None"},"installHint":"Install GitHub CLI.","detail":{"_tag":"None"},
            "auth":{"status":"authenticated","account":{"_tag":"Some","value":"alice"},"host":{"_tag":"None"},"detail":{"_tag":"None"}}
        }]})).expect("discovery"))),
        ..Default::default()
    }
}

/// The environment's publish service; its result carries the URL under test.
#[derive(Clone)]
struct Publishing {
    resource: Resource<GitActionsSnapshot>,
    url: &'static str,
}

impl GitActionsBackend for Publishing {
    fn watch(&self, _: &GitActionsScope, _: GitActionsWatch) -> LocalBoxStream<'static, GitActionsSnapshot> {
        let resource = self.resource.clone();
        self.resource
            .observe(0)
            .filter_map(move |_| futures::future::ready(resource.snapshot().value.as_deref().cloned()))
            .boxed_local()
    }
    fn refresh(&self, _: &GitActionsScope) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        async { Ok(()) }.boxed_local()
    }
    fn run_action(&self, _: &GitActionsScope, _: GitActionRequest) -> LocalBoxFuture<'static, Result<GitRunStackedActionResult, ClientError>> {
        async { Err(ClientError::Unsupported("git.runStackedAction")) }.boxed_local()
    }
    fn pull(&self, _: &GitActionsScope) -> LocalBoxFuture<'static, Result<VcsPullResult, ClientError>> {
        async { Err(ClientError::Unsupported("vcs.pull")) }.boxed_local()
    }
    fn initialize(&self, _: &GitActionsScope) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        async { Ok(()) }.boxed_local()
    }
    fn publish(&self, _: &GitActionsScope, input: SourceControlPublishRepositoryInput) -> LocalBoxFuture<'static, Result<SourceControlPublishRepositoryResult, ClientError>> {
        let url = self.url;
        async move {
            Ok(serde_json::from_value(json!({"repository":{"provider":"github","nameWithOwner":input.repository.0,"url":url,"sshUrl":"git@github.com:acme/web.git"},
                "remoteName":"origin","remoteUrl":"git@github.com:acme/web.git","branch":"main","status":"pushed"})).expect("publish result"))
        }
        .boxed_local()
    }
    fn sync_branch(&self, _: &GitActionsScope, _: Option<String>, _: Option<String>, _: bool) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        async { Ok(()) }.boxed_local()
    }
    fn open_file(&self, _: &GitActionsScope, _: String, _: String) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        async { Ok(()) }.boxed_local()
    }
}

#[derive(Clone, PartialEq, Props)]
struct PublishProps {
    address: usize,
}

#[component]
fn PublishFixture(props: PublishProps) -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    // As the app mounts it: the runtime present (`provide_recording_client` provides it).
    let (_, line) = provide_recording_client(true);
    let resource = Resource::new(ResourcePolicy::default(), Generation(0));
    let token = resource.begin();
    resource.finish(token, snapshot(), 0, false);
    // The git service is the environment's (what this test is not about); the opener is the root's.
    provide_context(GitActionsRuntime(Rc::new(Publishing {
        resource,
        url: ADDRESSES[props.address],
    })));
    let props = GitActionsControlProps::new(Prop::new(Some(scope())));
    view()
        .w_full()
        .h_full()
        .child(line)
        .child(view().w(px(280.0)).child(git_actions_control(props)))
}

#[test]
fn the_publish_result_opens_only_a_link_the_policy_allows() {
    for (index, address) in ADDRESSES.iter().enumerate() {
        let window = if index % 2 == 0 { DESKTOP } else { PHONE };
        let mut app = start(
            window,
            Root::new::<PublishFixture>(move || PublishProps { address: index }),
        );
        app.wait_for("role:button[Publish repository]")
            .expect("the service's first snapshot");
        app.press("role:button[Publish repository]").expect("publish");
        app.press("role:button[Next]").expect("provider ready");
        app.clear("id:publish-repository-path").expect("clear");
        app.type_text("id:publish-repository-path", "acme/web")
            .expect("repository");
        app.press("role:button[Publish]").expect("publish");
        app.wait_for("Repository published").expect("result");
        app.press("role:button[Open on GitHub]").expect("open");
        let opened = opened_after_settling(&mut app);
        if *address == SAFE {
            assert_eq!(opened, format!("opened: [{SAFE:?}]"), "{address}");
        } else {
            assert_eq!(opened, "opened: []", "{address} must not reach the opener");
        }
    }
}

// ---- A Markdown link in an assistant message, and a badge link ----

#[component]
fn MarkdownFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    provide_context(t3_ui::theme::text_view_parts());
    let (_, line) = provide_recording_client(true);
    let message = ADDRESSES
        .iter()
        .enumerate()
        .map(|(index, address)| format!("[link {index}]({address})"))
        .collect::<Vec<_>>()
        .join("\n\n");
    view()
        .w_full()
        .h_full()
        .child(line)
        .child(t3_ui::screens::thread::markdown::base::chat_markdown(
            move || message.clone(),
            || false,
        ))
}

#[test]
fn assistant_markdown_links_open_only_what_the_policy_allows() {
    let mut app = start(DESKTOP, Root::of::<MarkdownFixture>());
    app.wait_for("link 4").expect("rendered");
    for index in 0..ADDRESSES.len() {
        // A refused address is not a link at all; pressing its text opens nothing.
        if app.press(&format!("label:link {index}")).is_err() {
            let _ = app.press(&format!("text:link {index}"));
        }
    }
    assert_eq!(opened_after_settling(&mut app), format!("opened: [{SAFE:?}]"));
}

#[component]
fn BadgeFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let (_, line) = provide_recording_client(true);
    view()
        .w_full()
        .h_full()
        .child(line)
        .children(ADDRESSES.iter().enumerate().map(|(index, address)| {
            t3_ui::ui::badge::badge_link(
                t3_ui::ui::badge::Props::default(),
                t3_ui::i18n::Text::content(format!("badge {index}")),
                move || SharedString::from(*address),
                vec![t3_ui::i18n::Text::content(format!("badge {index}")).into()],
                || false,
                |_| {},
            )
            .into_element()
        }))
}

#[test]
fn badge_links_open_only_what_the_policy_allows() {
    let mut app = start(PHONE, Root::of::<BadgeFixture>());
    for index in 0..ADDRESSES.len() {
        app.press(&format!("label:badge {index}")).expect("badge");
    }
    assert_eq!(opened_after_settling(&mut app), format!("opened: [{SAFE:?}]"));
}

// ---- Git actions started from a press finish (rule 7 audit: CANCELLED-ON-REBUILD) ----

/// A branch one commit ahead of its upstream, not the default branch: the quick button and the
/// menu's "Push" both run a git action directly.
fn ahead_snapshot() -> GitActionsSnapshot {
    let status: VcsStatusResult = serde_json::from_value(json!({"isRepo":true,"hasPrimaryRemote":true,"isDefaultRef":false,"refName":"feature","hasWorkingTreeChanges":false,
        "workingTree":{"rootPath":"/project","files":[],"insertions":0,"deletions":0},"hasUpstream":true,"aheadCount":1,"behindCount":0,"pr":null}))
        .expect("status");
    GitActionsSnapshot {
        status: Some(Rc::new(status)),
        can_write: true,
        can_operate: true,
        has_server_thread: true,
        ..Default::default()
    }
}

/// The environment's git service: counts the actions that reached it.
#[derive(Clone)]
struct Acting {
    resource: Resource<GitActionsSnapshot>,
    runs: Rc<RefCell<Vec<String>>>,
    held: Rc<RefCell<Option<Vec<futures::channel::oneshot::Sender<()>>>>>,
}

impl GitActionsBackend for Acting {
    fn watch(&self, _: &GitActionsScope, _: GitActionsWatch) -> LocalBoxStream<'static, GitActionsSnapshot> {
        let resource = self.resource.clone();
        self.resource
            .observe(0)
            .filter_map(move |_| futures::future::ready(resource.snapshot().value.as_deref().cloned()))
            .boxed_local()
    }
    fn refresh(&self, _: &GitActionsScope) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        async { Ok(()) }.boxed_local()
    }
    fn run_action(&self, _: &GitActionsScope, request: GitActionRequest) -> LocalBoxFuture<'static, Result<GitRunStackedActionResult, ClientError>> {
        let runs = self.runs.clone();
        let held = self.held.borrow_mut().as_mut().map(|held| {
            let (sender, answered) = futures::channel::oneshot::channel();
            held.push(sender);
            answered
        });
        async move {
            runs.borrow_mut().push(format!("{:?}", request.action));
            if let Some(answered) = held {
                let _ = answered.await;
            }
            Ok(serde_json::from_value(json!({"action":request.action,"branch":{"status":"skipped_not_requested"},"commit":{"status":"skipped_not_requested"},"push":{"status":"pushed"},
                "pr":{"status":"skipped_not_requested"},"toast":{"title":"Pushed","cta":{"kind":"none"}}})).expect("result"))
        }
        .boxed_local()
    }
    fn pull(&self, _: &GitActionsScope) -> LocalBoxFuture<'static, Result<VcsPullResult, ClientError>> {
        async { Err(ClientError::Unsupported("vcs.pull")) }.boxed_local()
    }
    fn initialize(&self, _: &GitActionsScope) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        async { Ok(()) }.boxed_local()
    }
    fn publish(&self, _: &GitActionsScope, _: SourceControlPublishRepositoryInput) -> LocalBoxFuture<'static, Result<SourceControlPublishRepositoryResult, ClientError>> {
        async { Err(ClientError::Unsupported("sourceControl.publishRepository")) }.boxed_local()
    }
    fn sync_branch(&self, _: &GitActionsScope, _: Option<String>, _: Option<String>, _: bool) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        async { Ok(()) }.boxed_local()
    }
    fn open_file(&self, _: &GitActionsScope, _: String, _: String) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        async { Ok(()) }.boxed_local()
    }
}

#[component]
fn ActingFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    // As the app mounts it: the runtime present.
    let (_, line) = provide_recording_client(true);
    let resource = Resource::new(ResourcePolicy::default(), Generation(0));
    let token = resource.begin();
    resource.finish(token, ahead_snapshot(), 0, false);
    let runs = Rc::new(RefCell::new(Vec::new()));
    let held: Rc<RefCell<Option<Vec<futures::channel::oneshot::Sender<()>>>>> = Rc::default();
    provide_context(GitActionsRuntime(Rc::new(Acting {
        resource,
        runs: runs.clone(),
        held: held.clone(),
    })));
    let (holding, releasing) = (held.clone(), held);
    let hold = nsh_base::button()
        .accessibility_label(Some(SharedString::from("Hold replies")))
        .on_press(move || *holding.borrow_mut() = Some(Vec::new()))
        .child(text("Hold replies"));
    let release = nsh_base::button()
        .accessibility_label(Some(SharedString::from("Release replies")))
        .on_press(move || {
            for sender in releasing.borrow_mut().take().unwrap_or_default() {
                let _ = sender.send(());
            }
        })
        .child(text("Release replies"));
    let shown = signal(String::from("runs: unread"));
    let probe = nsh_base::button()
        .accessibility_label(Some(SharedString::from("Read runs")))
        .on_press(move || shown.set(format!("runs: {:?}", runs.borrow())))
        .child(text("Read runs"));
    let props = GitActionsControlProps::new(Prop::new(Some(scope())));
    view()
        .w_full()
        .h_full()
        .child(line)
        .child(probe)
        .child(hold)
        .child(release)
        .child(text(move || shown.get()))
        .child(view().w(px(280.0)).child(git_actions_control(props)))
}

fn runs_after_settling(app: &mut Headless) -> String {
    app.settle();
    app.press("role:button[Read runs]").expect("probe");
    app.screen()
        .texts()
        .into_iter()
        .find(|text| text.starts_with("runs: ") && *text != "runs: unread")
        .map(str::to_owned)
        .expect("runs line")
}

#[test]
fn a_git_action_started_from_the_quick_button_or_the_menu_finishes() {
    // The quick button ("Push & create PR" for a branch ahead of its upstream).
    let mut app = start(DESKTOP, Root::of::<ActingFixture>());
    app.wait_for("role:button[Push & create PR]")
        .expect("the service's first snapshot");
    app.press("role:button[Push & create PR]")
        .expect("quick action");
    app.wait_for("Pushed")
        .expect("the action finished and its success replaced the in-flight state");
    let runs = runs_after_settling(&mut app);
    assert_eq!(runs.matches('"').count(), 2, "exactly one action reached the environment: {runs}");
    // A menu item: the menu closes (its content is disposed) while the action runs.
    let mut app = start(DESKTOP, Root::of::<ActingFixture>());
    app.wait_for("role:button[Push & create PR]")
        .expect("the service's first snapshot");
    app.press("role:button[Git action options]").expect("menu");
    app.press("role:menuitem[Push]").expect("menu item");
    app.wait_for("Pushed")
        .expect("the action finished although the menu that started it closed");
    assert_eq!(runs_after_settling(&mut app), "runs: [\"Push\"]");
}

// Rule 7 audit evidence: the menu's "Push" answered only after the menu has gone.
#[test]
fn a_git_action_from_the_menu_answered_after_the_menu_closed_still_finishes() {
    let mut app = start(DESKTOP, Root::of::<ActingFixture>());
    app.wait_for("role:button[Push & create PR]")
        .expect("the service's first snapshot");
    app.press("role:button[Hold replies]").expect("hold");
    app.press("role:button[Git action options]").expect("menu");
    app.press("role:menuitem[Push]").expect("menu item");
    app.wait_for_gone("role:menuitem[Push]", nsh_testing::WAIT)
        .expect("the menu closed");
    app.press("role:button[Release replies]").expect("answer");
    app.wait_for("Pushed")
        .expect("the action's answer was handled after its menu closed");
}

// Rule 7 audit evidence: the quick button, answered after its first state change rendered.
#[test]
fn a_quick_git_action_answered_after_the_press_rendered_still_finishes() {
    let mut app = start(DESKTOP, Root::of::<ActingFixture>());
    app.wait_for("role:button[Push & create PR]")
        .expect("the service's first snapshot");
    app.press("role:button[Hold replies]").expect("hold");
    app.press("role:button[Push & create PR]")
        .expect("quick action");
    app.settle();
    app.press("role:button[Release replies]").expect("answer");
    app.wait_for("Pushed")
        .expect("the action's answer was handled");
}
