//! Thin adapters from the shared navigation services to UI contracts.
use crate::{
    i18n::Text,
    state::url::{UrlClient, UrlRequest, UrlSnapshot},
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use std::{cell::RefCell, rc::Rc};
use t3_client::{composition::ClientRuntime, platform::Platform};
struct UrlBinding<P> {
    client: t3_client::navigation::UrlClient<P>,
    failure: Rc<RefCell<Option<Text>>>,
}
pub fn url_client<P: Platform + Clone + 'static>(runtime: &ClientRuntime<P>) -> Rc<dyn UrlClient> {
    Rc::new(UrlBinding {
        client: runtime.navigation(),
        failure: Rc::default(),
    })
}
fn error(error: t3_client::ClientError) -> Text {
    error_ref(&error)
}
impl<P: Platform + Clone + 'static> UrlClient for UrlBinding<P> {
    fn watch(&self) -> LocalBoxStream<'static, UrlSnapshot> {
        let failure = self.failure.clone();
        self.client
            .watch()
            .inspect(move |item| {
                if let Err(cause) = item {
                    *failure.borrow_mut() = Some(error_ref(cause));
                }
            })
            .take_while(|item| futures::future::ready(item.is_ok()))
            .filter_map(|item| async { item.ok() })
            .boxed_local()
    }
    fn navigate(&self, request: UrlRequest) -> LocalBoxFuture<'static, Result<(), Text>> {
        let operation = self.client.navigate(request);
        async move { operation.await.map_err(error) }.boxed_local()
    }
    fn stream_error(&self) -> Option<Text> {
        self.failure.borrow().clone()
    }
}
fn error_ref(error: &t3_client::ClientError) -> Text {
    match error {
        t3_client::ClientError::Rejected { message, detail } => {
            Text::content(t3_client::error::rejection_text(message, detail.as_deref()))
        }
        error => Text::client_key(error.to_string()),
    }
}

use crate::screens::new_thread as ui_new;
use t3_client::navigation::new_thread as client_new;

struct NewThreadBinding<P: Platform + Clone + 'static>(client_new::NewThreadRuntime<P>);
pub fn new_thread_runtime<P: Platform + Clone + 'static>(
    runtime: &ClientRuntime<P>,
) -> ui_new::NewThreadRuntime {
    ui_new::NewThreadRuntime(Rc::new(NewThreadBinding(runtime.new_threads())))
}
fn new_target(target: ui_new::Target) -> client_new::Target {
    match target {
        ui_new::Target::Home => client_new::Target::Home,
        ui_new::Target::Draft(id) => client_new::Target::Draft(id),
        ui_new::Target::Link(search) => client_new::Target::Link(search),
    }
}
fn destination(target: client_new::Destination) -> ui_new::Destination {
    match target {
        client_new::Destination::Draft(id) => ui_new::Destination::Draft(id),
        client_new::Destination::Thread(key) => ui_new::Destination::Thread(key),
    }
}
fn new_view(view: &client_new::NewThreadView) -> ui_new::NewThreadView {
    match view {
        client_new::NewThreadView::Loading => ui_new::NewThreadView::Loading,
        client_new::NewThreadView::Empty { can_start_scratch } => ui_new::NewThreadView::Empty {
            can_start_scratch: *can_start_scratch,
        },
        client_new::NewThreadView::StartFailed => ui_new::NewThreadView::StartFailed,
        client_new::NewThreadView::LinkFailed { title, message } => {
            ui_new::NewThreadView::LinkFailed {
                title: Text::client_key(title.clone()),
                message: Text::client_key(message.clone()),
            }
        }
        client_new::NewThreadView::Navigate(target) => {
            ui_new::NewThreadView::Navigate(destination(target.clone()))
        }
        client_new::NewThreadView::Draft(draft) => {
            ui_new::NewThreadView::Draft(Rc::new(ui_new::DraftView {
                id: draft.id.clone(),
                prompt: draft.prompt.clone(),
                cursor: draft.cursor,
                revision: draft.revision,
                project: Some(draft.project.clone()),
                project_title: Some(draft.project_title.clone()),
                projects: draft
                    .projects
                    .iter()
                    .map(|p| ui_new::ProjectChoice {
                        key: p.key.clone(),
                        title: p.title.clone(),
                        environment_label: p.environment_label.clone(),
                    })
                    .collect(),
                is_scratch: draft.is_scratch,
                can_start_scratch: draft.can_start_scratch,
                editable: draft.editable,
                actions: crate::screens::composer::send::PrimaryActionState {
                    can_operate_thread: draft.can_operate,
                    is_send_busy: draft.busy,
                    send_disabled_reason: draft.disabled_reason.clone().map(Text::client_key),
                    is_environment_unavailable: draft.unavailable,
                    has_sendable_content: draft.has_content,
                    ..Default::default()
                },
                banner: draft.banner.clone().map(Text::content),
            }))
        }
    }
}
impl<P: Platform + Clone + 'static> ui_new::NewThreadClient for NewThreadBinding<P> {
    fn watch(&self, target: ui_new::Target) -> LocalBoxStream<'static, Rc<ui_new::NewThreadView>> {
        self.0
            .watch(new_target(target))
            .map(|v| Rc::new(new_view(&v)))
            .boxed_local()
    }
    fn retry(&self, target: ui_new::Target) -> LocalBoxFuture<'static, Result<(), Text>> {
        let service = self.0.clone();
        async move { service.retry(new_target(target)).await.map_err(error) }.boxed_local()
    }
    fn start_scratch(&self, id: Option<String>) -> LocalBoxFuture<'static, Result<(), Text>> {
        let service = self.0.clone();
        async move { service.start_scratch(id).await.map_err(error) }.boxed_local()
    }
    fn select_project(
        &self,
        id: String,
        project: t3_client::workspace::ProjectKey,
    ) -> LocalBoxFuture<'static, Result<(), Text>> {
        let service = self.0.clone();
        async move { service.select_project(id, project).await.map_err(error) }.boxed_local()
    }
    fn set_prompt(&self, id: &str, prompt: String, cursor: usize) -> Result<u64, Text> {
        self.0.set_prompt(id, prompt, cursor).map_err(error)
    }
    fn submit(
        &self,
        id: String,
        revision: u64,
        mode: t3_client::operations::ComposerDispatchMode,
    ) -> LocalBoxFuture<'static, Result<t3_client::types::ThreadKey, Text>> {
        let service = self.0.clone();
        async move { service.submit(id, revision, mode).await.map_err(error) }.boxed_local()
    }
    fn attachments(
        &self,
        id: &str,
    ) -> Option<t3_client::thread_views::composer::attachment_intake::AttachmentSession> {
        self.0.attachments(id)
    }
    fn model_picker(&self, _: &str) -> Option<crate::screens::composer::model_picker::PickerProps> {
        None
    }
}
