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

use new_thread::{Destination, LinkAction, LinkChoice};
const LINK_QUERY: &str = "?prompt=123&repo=owner%2Frepo";
fn choice_label(action: LinkAction) -> &'static str {
    match action {
        LinkAction::CloneRepository => "Clone owner/repo",
        LinkAction::UseWithoutProject => "Use without project",
        LinkAction::CheckoutPullRequest => "Check out",
        LinkAction::ReplaceDraft => "Replace draft",
    }
}
fn unused<T: 'static>() -> LocalBoxFuture<'static, Result<T, Text>> {
    futures::future::ready(Err(Text::content("Unused test operation"))).boxed_local()
}
struct LinkFake {
    action: LinkAction,
    disabled: bool,
    success: bool,
    opened: bool,
    calls: Signal<usize>,
}
impl NewThreadClient for LinkFake {
    fn watch(&self, _: Target) -> LocalBoxStream<'static, Rc<NewThreadView>> {
        if self.opened {
            return futures::stream::once(futures::future::ready(Rc::new(
                NewThreadView::LinkOpened,
            )))
            .chain(futures::stream::pending())
            .boxed_local();
        }
        futures::stream::once(futures::future::ready(Rc::new(NewThreadView::LinkChoice {
            title: Text::content("Source link"),
            message: Text::content("Read before sending"),
            choices: vec![LinkChoice {
                action: self.action,
                label: Text::content(choice_label(self.action)),
                disabled_reason: self.disabled.then(|| Text::content("Checking out...")),
            }],
        })))
        .chain(futures::stream::pending())
        .boxed_local()
    }
    fn retry(&self, target: Target) -> LocalBoxFuture<'static, Result<(), Text>> {
        if self.opened {
            assert_eq!(target, Target::Link(LINK_QUERY.into()));
            self.calls.update(|calls| *calls += 1);
            return futures::future::ready(Err(Text::content("Reopen failed"))).boxed_local();
        }
        unused()
    }
    fn start_scratch(&self, _: Option<String>) -> LocalBoxFuture<'static, Result<(), Text>> {
        unused()
    }
    fn select_project(
        &self,
        _: String,
        _: ProjectKey,
    ) -> LocalBoxFuture<'static, Result<(), Text>> {
        unused()
    }
    fn set_prompt(&self, _: &str, _: String, _: usize) -> Result<u64, Text> {
        Err(Text::content("Unused test operation"))
    }
    fn submit(
        &self,
        _: String,
        _: u64,
        _: ComposerDispatchMode,
    ) -> LocalBoxFuture<'static, Result<ThreadKey, Text>> {
        unused()
    }
    fn attachments(&self, _: &str) -> Option<AttachmentSession> {
        None
    }
    fn model_picker(&self, _: &str) -> Option<PickerProps> {
        None
    }
    fn link_action(
        &self,
        search: String,
        action: LinkAction,
    ) -> LocalBoxFuture<'static, Result<Option<Destination>, Text>> {
        assert_eq!(search, LINK_QUERY);
        assert_eq!(action, self.action);
        self.calls.update(|calls| *calls += 1);
        futures::future::ready(if self.success {
            Ok(Some(Destination::Draft("created".into())))
        } else {
            Err(Text::content("Choice rejected"))
        })
        .boxed_local()
    }
}
#[derive(Clone, PartialEq, Props)]
struct LinkHarnessProps {
    action: String,
    disabled: bool,
    success: bool,
    opened: bool,
}
#[component]
fn LinkHarness(props: LinkHarnessProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let nav = Navigator::new(Route::NewThread);
    provide_context(nav);
    let requests = signal(Vec::new());
    if props.success {
        use t3_ui::state::url::{Location, UrlState};
        let (sender, receiver) = mpsc::unbounded();
        UrlState::install(
            Location::parse(&format!("/new{LINK_QUERY}")),
            nav,
            Rc::new(LinkUrls {
                sender,
                receiver: RefCell::new(Some(receiver)),
                requests,
            }),
        );
    }
    let calls = signal(0usize);
    provide_context(NewThreadRuntime(Rc::new(LinkFake {
        action: [
            LinkAction::CloneRepository,
            LinkAction::UseWithoutProject,
            LinkAction::CheckoutPullRequest,
            LinkAction::ReplaceDraft,
        ]
        .into_iter()
        .find(|action| choice_label(*action) == props.action)
        .expect("known fixture action"),
        disabled: props.disabled,
        success: props.success,
        opened: props.opened,
        calls,
    })));
    view()
        .w_full()
        .h_full()
        .child(new_thread::new_thread_link_route(LINK_QUERY))
        .child(text(move || format!("Link actions: {}", calls.get())))
        .child(text(move || {
            format!("Link URL requests: {:?}", requests.get())
        }))
}
#[test]
fn every_link_choice_delegates_raw_search_and_respects_the_latest_disabled_state() {
    for action in [
        LinkAction::CloneRepository,
        LinkAction::UseWithoutProject,
        LinkAction::CheckoutPullRequest,
        LinkAction::ReplaceDraft,
    ] {
        for window in [DESKTOP, PHONE] {
            for disabled in [false, true] {
                let mut app = Headless::start(EngineConfig {
                    ui_sets: t3_ui::registries(),
                    viewport: size(px(window.0), px(window.1)),
                    ..EngineConfig::new(Root::new::<LinkHarness>(move || LinkHarnessProps {
                        action: choice_label(action).into(),
                        disabled,
                        success: false,
                        opened: false,
                    }))
                })
                .expect("link harness");
                app.wait_for("Source link").expect("client choice");
                let _ = app.press(&format!("role:button[{}]", choice_label(action)));
                if disabled {
                    assert!(app.screen().shows("Link actions: 0"));
                } else {
                    app.wait_for("Choice rejected").expect("typed rejection");
                    assert!(app.screen().shows("Link actions: 1"));
                }
                assert!(app.screen().shows("Source link"));
            }
        }
    }
}

use t3_ui::state::url::{RoutePolicy, UrlClient, UrlRequest, UrlSnapshot};
struct LinkUrls {
    sender: mpsc::UnboundedSender<UrlSnapshot>,
    receiver: RefCell<Option<mpsc::UnboundedReceiver<UrlSnapshot>>>,
    requests: Signal<Vec<UrlRequest>>,
}
fn link_url(path: String) -> UrlSnapshot {
    UrlSnapshot {
        path,
        history_index: 0,
        entry_key: None,
        can_go_back: false,
        can_go_forward: false,
        policy: Some(RoutePolicy {
            authenticated_or_hosted: true,
            ..Default::default()
        }),
    }
}
impl UrlClient for LinkUrls {
    fn watch(&self) -> LocalBoxStream<'static, UrlSnapshot> {
        futures::stream::once(futures::future::ready(link_url(format!(
            "/new{LINK_QUERY}"
        ))))
        .chain(self.receiver.borrow_mut().take().expect("one URL observer"))
        .boxed_local()
    }
    fn navigate(&self, request: UrlRequest) -> LocalBoxFuture<'static, Result<(), Text>> {
        self.requests
            .update(|requests| requests.push(request.clone()));
        if let UrlRequest::Push(path) | UrlRequest::Replace(path) = request {
            self.sender
                .unbounded_send(link_url(path))
                .expect("URL listener");
        }
        futures::future::ready(Ok(())).boxed_local()
    }
}
#[test]
fn an_accepted_link_replaces_its_url_instead_of_trapping_back_navigation() {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::new::<LinkHarness>(|| LinkHarnessProps {
                action: choice_label(LinkAction::ReplaceDraft).into(),
                disabled: false,
                success: true,
                opened: false,
            }))
        })
        .expect("link URL harness");
        app.wait_for("Source link").expect("replacement choice");
        app.press("role:button[Replace draft]")
            .expect("accept prompt");
        app.wait_for(r#"Link URL requests: [Replace("/draft/created")]"#)
            .expect("replace the source URL");
    }
}

#[test]
fn an_opened_link_exposes_the_source_reopen_action_and_retains_a_failure() {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::new::<LinkHarness>(|| LinkHarnessProps {
                action: choice_label(LinkAction::ReplaceDraft).into(),
                disabled: false,
                success: false,
                opened: true,
            }))
        })
        .expect("opened link harness");
        app.wait_for("This link is open")
            .expect("opened entry view");
        app.press("role:button[Open it again]")
            .expect("explicit reopen");
        app.wait_for("Reopen failed").expect("reopen error");
        assert!(app.screen().shows("Link actions: 1"));
    }
}

/// A draft whose project choice and scratch start answer only when the test says so, as server
/// replies arriving after the picker closed or the draft changed.
struct SlowDraft {
    draft: RefCell<DraftView>,
    updates: mpsc::UnboundedSender<Rc<NewThreadView>>,
    receiver: RefCell<Option<mpsc::UnboundedReceiver<Rc<NewThreadView>>>>,
    answers: Rc<RefCell<Vec<futures::channel::oneshot::Sender<()>>>>,
    started: Signal<usize>,
}
impl SlowDraft {
    fn answer_later(&self, message: String) -> LocalBoxFuture<'static, Result<(), Text>> {
        let (sender, answered) = futures::channel::oneshot::channel();
        self.answers.borrow_mut().push(sender);
        let started = self.started;
        async move {
            started.update(|n| *n += 1);
            let _ = answered.await;
            Err(Text::content(message))
        }
        .boxed_local()
    }
}
fn slow_project(id: &str) -> ProjectKey {
    ProjectKey {
        environment: t3_client::types::EnvironmentId("env".into()),
        project: t3_client::types::ProjectId(id.into()),
    }
}
impl NewThreadClient for SlowDraft {
    fn watch(&self, _: Target) -> LocalBoxStream<'static, Rc<NewThreadView>> {
        let initial = Rc::new(NewThreadView::Draft(Rc::new(self.draft.borrow().clone())));
        futures::stream::once(futures::future::ready(initial))
            .chain(self.receiver.borrow_mut().take().expect("one session"))
            .boxed_local()
    }
    fn retry(&self, _: Target) -> LocalBoxFuture<'static, Result<(), Text>> {
        unused()
    }
    fn start_scratch(&self, _: Option<String>) -> LocalBoxFuture<'static, Result<(), Text>> {
        self.answer_later("scratch answered".into())
    }
    fn select_project(
        &self,
        _: String,
        project: ProjectKey,
    ) -> LocalBoxFuture<'static, Result<(), Text>> {
        self.answer_later(format!("{} answered", project.project.0))
    }
    fn set_prompt(&self, _: &str, prompt: String, cursor: usize) -> Result<u64, Text> {
        let mut draft = self.draft.borrow_mut();
        draft.prompt = prompt;
        draft.cursor = cursor;
        draft.revision += 1;
        let _ = self
            .updates
            .unbounded_send(Rc::new(NewThreadView::Draft(Rc::new(draft.clone()))));
        Ok(draft.revision)
    }
    fn submit(
        &self,
        _: String,
        _: u64,
        _: ComposerDispatchMode,
    ) -> LocalBoxFuture<'static, Result<ThreadKey, Text>> {
        unused()
    }
    fn attachments(&self, _: &str) -> Option<AttachmentSession> {
        None
    }
    fn model_picker(&self, _: &str) -> Option<PickerProps> {
        None
    }
    fn link_action(
        &self,
        _: String,
        _: LinkAction,
    ) -> LocalBoxFuture<'static, Result<Option<Destination>, Text>> {
        unused()
    }
}
#[component]
fn SlowDraftHarness() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    provide_context(Navigator::new(Route::Draft("saved".into())));
    let started = signal(0usize);
    let answers = Rc::new(RefCell::new(Vec::new()));
    let (updates, receiver) = mpsc::unbounded();
    provide_context(NewThreadRuntime(Rc::new(SlowDraft {
        draft: RefCell::new(DraftView {
            id: "saved".into(),
            prompt: String::new(),
            cursor: 0,
            revision: 1,
            project: Some(slow_project("first")),
            project_title: Some("First project".into()),
            projects: vec![
                new_thread::ProjectChoice {
                    key: slow_project("first"),
                    title: "First project".into(),
                    environment_label: None,
                },
                new_thread::ProjectChoice {
                    key: slow_project("second"),
                    title: "Second project".into(),
                    environment_label: None,
                },
            ],
            is_scratch: false,
            can_start_scratch: true,
            editable: true,
            actions: PrimaryActionState::default(),
            banner: None,
        }),
        updates,
        receiver: RefCell::new(Some(receiver)),
        answers: answers.clone(),
        started,
    })));
    view()
        .w_full()
        .h_full()
        .child(
            nsh_base::button()
                .accessibility_label(Some("Answer the pending requests".into()))
                .child(text("Answer the pending requests"))
                .on_press(move || {
                    for sender in answers.borrow_mut().drain(..) {
                        let _ = sender.send(());
                    }
                }),
        )
        .child(text(move || format!("Requests started: {}", started.get())))
        .child(new_thread::screen(Some("saved")))
}
fn slow_draft(window: (f32, f32)) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<SlowDraftHarness>())
    })
    .expect("slow draft harness");
    app.wait_for("role:button[Choose a project]")
        .expect("project trigger");
    app
}
// Rule 7: the picker closes on the press and the draft changes while the choice is pending.
#[test]
fn a_project_choice_answered_after_the_picker_closed_still_reports() {
    for window in [DESKTOP, PHONE] {
        let mut app = slow_draft(window);
        app.press("role:button[Choose a project]")
            .expect("open the picker");
        app.press("role:button[Second project]")
            .expect("choose another project");
        app.wait_for("Requests started: 1")
            .expect("the choice reached the client");
        app.wait_for_gone("role:button[Second project]", nsh_testing::WAIT)
            .expect("the picker closed");
        app.type_text("id:composer-editor", "typed while pending")
            .expect("type in the draft");
        app.press("role:button[Answer the pending requests]")
            .expect("the client answers");
        app.wait_for("second answered")
            .unwrap_or_else(|error| panic!("answer lost at {window:?}: {error}"));
    }
}
// Rule 7 and defect 3: typing changes the draft, which rebuilt the scratch button's region.
#[test]
fn starting_without_a_project_reports_its_answer_after_the_draft_changed() {
    for window in [DESKTOP, PHONE] {
        let mut app = slow_draft(window);
        app.press("role:button[or start without a project]")
            .expect("start without a project");
        app.wait_for("Requests started: 1")
            .expect("the start reached the client");
        app.type_text("id:composer-editor", "typed while pending")
            .expect("type in the draft");
        app.press("role:button[Answer the pending requests]")
            .expect("the client answers");
        app.wait_for("scratch answered")
            .unwrap_or_else(|error| panic!("answer lost at {window:?}: {error}"));
    }
}
