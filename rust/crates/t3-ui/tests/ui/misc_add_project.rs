//! Add-project interactions against a rejecting service; no fake successful project is inserted.

use std::{cell::RefCell, rc::Rc};

use futures::{
    FutureExt, StreamExt, channel::mpsc, future::LocalBoxFuture, stream::LocalBoxStream,
};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::{
    i18n::Text,
    screens::{
        add_project::{self, AddProjectClient, AddProjectView, ProjectGroup, ProjectItem, Step},
        new_thread::Destination,
    },
    state::nav::{Navigator, Route},
    theme,
    ui::icon::Icon,
};

use crate::support::{DESKTOP, PHONE};

struct RejectedProject {
    view: RefCell<AddProjectView>,
    sender: mpsc::UnboundedSender<Rc<AddProjectView>>,
    receiver: RefCell<Option<mpsc::UnboundedReceiver<Rc<AddProjectView>>>>,
    calls: Signal<usize>,
}

impl AddProjectClient for RejectedProject {
    fn watch(&self) -> LocalBoxStream<'static, Rc<AddProjectView>> {
        futures::stream::once(futures::future::ready(Rc::new(self.view.borrow().clone())))
            .chain(self.receiver.borrow_mut().take().expect("one flow"))
            .boxed_local()
    }
    fn set_query(&self, query: String) -> Result<(), Text> {
        let mut value = self.view.borrow_mut();
        value.query = query;
        value.revision += 1;
        self.sender
            .unbounded_send(Rc::new(value.clone()))
            .expect("mounted flow");
        Ok(())
    }
    fn select_item(&self, _: &str) -> LocalBoxFuture<'static, Result<Option<Destination>, Text>> {
        self.calls.update(|calls| *calls += 1);
        futures::future::ready(Err("Could not create the project".into())).boxed_local()
    }
    fn back(&self) -> Result<(), Text> {
        Ok(())
    }
    fn submit(&self, _: u64) -> LocalBoxFuture<'static, Result<Option<Destination>, Text>> {
        self.calls.update(|calls| *calls += 1);
        futures::future::ready(Err("Could not create the project".into())).boxed_local()
    }
    fn pick_folder(&self, _: u64) -> LocalBoxFuture<'static, Result<Option<Destination>, Text>> {
        self.calls.update(|calls| *calls += 1);
        futures::future::ready(Ok(None)).boxed_local()
    }
}

#[derive(Clone, PartialEq, Props)]
struct HarnessProps {
    denied: bool,
}

#[component]
fn Harness(props: HarnessProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    provide_context(Navigator::new(Route::AddProject));
    let calls = signal(0usize);
    let (sender, receiver) = mpsc::unbounded();
    let client = Rc::new(RejectedProject {
        view: RefCell::new(AddProjectView {
            revision: 1,
            step: Step::LocalFolder,
            query: "~/projects/existing".into(),
            placeholder: "Project path".into(),
            groups: vec![ProjectGroup {
                label: "Sources".into(),
                items: vec![ProjectItem {
                    id: "disabled-source".into(),
                    title: "Git URL".into(),
                    description: Some("Clone from a remote URL".into()),
                    icon: Icon::Link,
                    disabled: true,
                    setup_required: false,
                }],
            }],
            summary: None,
            can_submit: !props.denied,
            submit_label: "Add".into(),
            submit_accessibility: "Add (Enter)".into(),
            can_go_back: false,
            folder_picker_label: None,
            can_pick_folder: false,
            loading_folders: false,
            error: None,
            empty_message: None,
        }),
        sender,
        receiver: RefCell::new(Some(receiver)),
        calls,
    });
    view()
        .w_full()
        .h_full()
        .child(add_project::command_palette_add_project(client))
        .child(text(move || {
            format!("Project mutation calls: {}", calls.get())
        }))
}

fn start(window: (f32, f32), denied: bool) -> Headless {
    Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<Harness>(move || HarnessProps { denied }))
    })
    .expect("project harness")
}

#[test]
fn failed_add_preserves_the_typed_folder_for_a_retry() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, false);
        app.wait_for("~/projects/existing").expect("client path");
        app.press("role:button[Add]").expect("submit");
        app.wait_for("Could not create the project")
            .expect("failure displayed");
        assert_eq!(
            app.screen()
                .input_text("label:Command palette")
                .expect("path input"),
            Some("~/projects/existing")
        );
        assert!(app.screen().shows("Project mutation calls: 1"));
        app.press("role:button[Add]")
            .expect("retry remains possible");
        app.wait_for("Project mutation calls: 2")
            .expect("busy state released after failure");
    }
}

#[test]
fn denied_add_and_unsupported_clone_do_not_dispatch() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, true);
        app.wait_for("~/projects/existing").expect("client path");
        let _ = app.press("role:button[Add]");
        let _ = app.press("role:button[Git URL]");
        assert!(app.screen().shows("Project mutation calls: 0"));
        assert_eq!(
            app.screen()
                .input_text("label:Command palette")
                .expect("path input"),
            Some("~/projects/existing")
        );
    }
}

struct CompletionFlow {
    finish: bool,
    publication: bool,
    updates: mpsc::UnboundedSender<Rc<AddProjectView>>,
    receiver: RefCell<Option<mpsc::UnboundedReceiver<Rc<AddProjectView>>>>,
    done: mpsc::UnboundedSender<()>,
    completed: RefCell<Option<mpsc::UnboundedReceiver<()>>>,
}
fn completion_view(query: &str) -> AddProjectView {
    AddProjectView {
        revision: 1,
        step: Step::CloneDestination,
        query: query.into(),
        placeholder: Text::content("Clone destination"),
        groups: vec![],
        summary: None,
        can_submit: true,
        submit_label: Text::client_key("Clone"),
        submit_accessibility: Text::client_key("Clone"),
        can_go_back: false,
        folder_picker_label: None,
        can_pick_folder: false,
        loading_folders: false,
        error: None,
        empty_message: None,
    }
}
impl AddProjectClient for CompletionFlow {
    fn watch(&self) -> LocalBoxStream<'static, Rc<AddProjectView>> {
        futures::stream::once(futures::future::ready(Rc::new(completion_view("~/clone"))))
            .chain(self.receiver.borrow_mut().take().expect("view observer"))
            .boxed_local()
    }
    fn completed(&self) -> LocalBoxStream<'static, ()> {
        self.completed
            .borrow_mut()
            .take()
            .expect("completion observer")
            .boxed_local()
    }
    fn publishing(&self) -> LocalBoxStream<'static, Option<Rc<add_project::PublishingView>>> {
        let value = self.publication.then(|| {
            Rc::new(add_project::PublishingView {
                title: Text::client_key("Create private repository"),
                description: Text::content("acme/garden"),
                selected: "github".into(),
                targets: vec![add_project::PublishChoice {
                    kind: "github".into(),
                    label: Text::content("GitHub"),
                }],
                publishes: false,
                can_change: true,
            })
        });
        futures::stream::once(futures::future::ready(value))
            .chain(futures::stream::pending())
            .boxed_local()
    }
    fn set_publishing(&self, publishes: bool, kind: Option<String>) -> Result<(), Text> {
        assert!(publishes);
        assert_eq!(kind.as_deref(), Some("github"));
        Err(Text::content("Publication denied"))
    }
    fn set_query(&self, _: String) -> Result<(), Text> {
        Ok(())
    }
    fn back(&self) -> Result<(), Text> {
        Ok(())
    }
    fn select_item(&self, _: &str) -> LocalBoxFuture<'static, Result<Option<Destination>, Text>> {
        futures::future::ready(Err(Text::content("Unused operation"))).boxed_local()
    }
    fn pick_folder(&self, _: u64) -> LocalBoxFuture<'static, Result<Option<Destination>, Text>> {
        futures::future::ready(Err(Text::content("Unused operation"))).boxed_local()
    }
    fn submit(&self, _: u64) -> LocalBoxFuture<'static, Result<Option<Destination>, Text>> {
        if self.finish {
            self.done.unbounded_send(()).expect("completion listener");
        } else {
            self.updates
                .unbounded_send(Rc::new(completion_view("Intermediate destination")))
                .expect("view listener");
        }
        futures::future::ready(Ok(None)).boxed_local()
    }
}
#[derive(Clone, PartialEq, Props)]
struct CompletionProps {
    finish: bool,
    publication: bool,
}
#[component]
fn CompletionHarness(props: CompletionProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let nav = Navigator::new(Route::AddProject);
    provide_context(nav);
    let (updates, receiver) = mpsc::unbounded();
    let (done, completed) = mpsc::unbounded();
    let client = stored(Rc::new(CompletionFlow {
        finish: props.finish,
        publication: props.publication,
        updates,
        receiver: RefCell::new(Some(receiver)),
        done,
        completed: RefCell::new(Some(completed)),
    }));
    dynamic(move || {
        if nav.top() == Route::AddProject {
            add_project::command_palette_add_project(client.get()).into_element()
        } else {
            text("Closed project flow").into_element()
        }
    })
}
fn completion_case(finish: bool) {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::new::<CompletionHarness>(move || CompletionProps {
                finish,
                publication: false,
            }))
        })
        .expect("completion harness");
        app.wait_for("~/clone").expect("clone destination");
        app.press("role:button[Clone]")
            .expect("continue clone flow");
        if finish {
            app.wait_for("Closed project flow")
                .expect("accepted completion closes the dialog");
        } else {
            app.wait_for("Intermediate destination")
                .expect("intermediate step remains mounted");
            assert!(!app.screen().shows("Closed project flow"));
        }
    }
}
#[test]
fn accepted_clone_completion_closes_the_project_dialog() {
    completion_case(true);
}
#[test]
fn an_intermediate_project_step_does_not_close_the_dialog() {
    completion_case(false);
}

#[test]
fn optional_publication_control_delegates_and_retains_a_failure() {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::new::<CompletionHarness>(|| CompletionProps {
                finish: false,
                publication: true,
            }))
        })
        .expect("publication harness");
        app.wait_for("acme/garden").expect("publication target");
        app.press("label:Create private repository")
            .expect("enable publication");
        app.wait_for("Publication denied")
            .expect("failure remains visible");
        assert_eq!(
            app.screen()
                .input_text("label:Command palette")
                .expect("clone path input"),
            Some("~/clone")
        );
        assert!(!app.screen().shows("Closed project flow"));
    }
}
