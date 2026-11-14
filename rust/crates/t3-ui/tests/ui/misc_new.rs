//! Draft UI at its service boundary; production mounting waits for the composed client service.

use std::{cell::RefCell, rc::Rc};

use futures::{
    FutureExt, StreamExt, channel::mpsc, future::LocalBoxFuture, stream::LocalBoxStream,
};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::{
    operations::ComposerDispatchMode, thread_views::composer::attachment_intake::AttachmentSession,
    types::ThreadKey, workspace::ProjectKey,
};
use t3_ui::{
    i18n::Text,
    screens::{
        composer::{model_picker::PickerProps, send::PrimaryActionState},
        new_thread::{self, DraftView, NewThreadClient, NewThreadRuntime, NewThreadView, Target},
    },
    state::nav::{Navigator, Route},
    theme,
};

use crate::support::{DESKTOP, PHONE};

struct RejectedDraft {
    draft: RefCell<DraftView>,
    updates: mpsc::UnboundedSender<Rc<NewThreadView>>,
    receiver: RefCell<Option<mpsc::UnboundedReceiver<Rc<NewThreadView>>>>,
    calls: Signal<usize>,
}

impl NewThreadClient for RejectedDraft {
    fn watch(&self, _: Target) -> LocalBoxStream<'static, Rc<NewThreadView>> {
        let initial = Rc::new(NewThreadView::Draft(Rc::new(self.draft.borrow().clone())));
        futures::stream::once(futures::future::ready(initial))
            .chain(self.receiver.borrow_mut().take().expect("one session"))
            .boxed_local()
    }
    fn retry(&self, _: Target) -> LocalBoxFuture<'static, Result<(), Text>> {
        futures::future::ready(Ok(())).boxed_local()
    }
    fn start_scratch(&self, _: Option<String>) -> LocalBoxFuture<'static, Result<(), Text>> {
        futures::future::ready(Ok(())).boxed_local()
    }
    fn select_project(
        &self,
        _: String,
        _: ProjectKey,
    ) -> LocalBoxFuture<'static, Result<(), Text>> {
        futures::future::ready(Ok(())).boxed_local()
    }
    fn set_prompt(&self, _: &str, prompt: String, cursor: usize) -> Result<u64, Text> {
        let mut draft = self.draft.borrow_mut();
        draft.prompt = prompt;
        draft.cursor = cursor;
        draft.revision += 1;
        self.updates
            .unbounded_send(Rc::new(NewThreadView::Draft(Rc::new(draft.clone()))))
            .expect("mounted session");
        Ok(draft.revision)
    }
    fn submit(
        &self,
        _: String,
        _: u64,
        _: ComposerDispatchMode,
    ) -> LocalBoxFuture<'static, Result<ThreadKey, Text>> {
        self.calls.update(|calls| *calls += 1);
        futures::future::ready(Err(Text::client_key(
            "This connection cannot add projects.",
        )))
        .boxed_local()
    }
    fn attachments(&self, _: &str) -> Option<AttachmentSession> {
        None
    }
    fn model_picker(&self, _: &str) -> Option<PickerProps> {
        None
    }
}

#[derive(Clone, PartialEq, Props)]
struct HarnessProps {
    denied: bool,
}

#[component]
fn Harness(props: HarnessProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    provide_context(Navigator::new(Route::Draft("saved".into())));
    let calls = signal(0usize);
    let (updates, receiver) = mpsc::unbounded();
    provide_context(NewThreadRuntime(Rc::new(RejectedDraft {
        draft: RefCell::new(DraftView {
            id: "saved".into(),
            prompt: "Retain this draft".into(),
            cursor: 17,
            revision: 1,
            project: None,
            project_title: None,
            projects: vec![],
            is_scratch: true,
            can_start_scratch: false,
            editable: true,
            actions: PrimaryActionState {
                can_operate_thread: !props.denied,
                has_sendable_content: true,
                ..Default::default()
            },
            banner: None,
        }),
        updates,
        receiver: RefCell::new(Some(receiver)),
        calls,
    })));
    view()
        .w_full()
        .h_full()
        .child(new_thread::screen(Some("saved")))
        .child(text(move || format!("Submission calls: {}", calls.get())))
}

fn start(window: (f32, f32), denied: bool) -> Headless {
    Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<Harness>(move || HarnessProps { denied }))
    })
    .expect("draft harness")
}

#[test]
fn a_rejected_creation_preserves_the_composer_and_its_prompt_at_both_sizes() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, false);
        app.wait_for("Retain this draft")
            .expect("saved client draft restored");
        app.press("role:button[Submit message]")
            .expect("submit through CMP-SEND");
        app.wait_for("This connection cannot add projects.")
            .expect("rejection shown");
        assert!(app.screen().shows("Retain this draft"));
        assert!(app.screen().shows("Submission calls: 1"));
        app.type_text("id:composer-editor", " and my edits")
            .expect("editor remains usable");
        app.wait_for("and my edits")
            .expect("typing survives rejection");
    }
}

#[test]
fn a_denied_creation_cannot_submit_and_keeps_the_saved_prompt() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, true);
        app.wait_for("Retain this draft").expect("saved draft");
        let _ = app.press("role:button[Submit message]");
        assert!(app.screen().shows("Submission calls: 0"));
        assert!(app.screen().shows("Retain this draft"));
    }
}
