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
        assert!(app.screen().shows("~/projects/existing"));
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
        assert!(app.screen().shows("~/projects/existing"));
    }
}
