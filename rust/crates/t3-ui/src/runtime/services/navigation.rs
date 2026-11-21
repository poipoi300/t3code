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
    failure: Rc<RefCell<Option<t3_client::ClientError>>>,
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
            .map(move |item| match item {
                Ok(snapshot) => Some(snapshot),
                Err(cause) => {
                    *failure.borrow_mut() = Some(cause);
                    None
                }
            })
            .take_while(|item| futures::future::ready(item.is_some()))
            .filter_map(futures::future::ready)
            .boxed_local()
    }
    fn navigate(&self, request: UrlRequest) -> LocalBoxFuture<'static, Result<(), Text>> {
        let operation = self.client.navigate(request);
        async move { operation.await.map_err(error) }.boxed_local()
    }
    fn stream_error(&self) -> Option<Text> {
        self.failure.borrow().as_ref().map(error_ref)
    }
    fn boot_error(&self) -> Option<Text> {
        self.failure
            .borrow()
            .as_ref()
            .and_then(|cause| match cause {
                t3_client::ClientError::Platform(
                    t3_client::platform::PlatformError::Unsupported(_),
                ) => None,
                _ => Some(error_ref(cause)),
            })
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

use crate::screens::add_project as ui_project;
use t3_client::navigation::{ignored_selection as client_ignored, project_flow as client_project};

fn project_label(label: &client_project::Label) -> Text {
    match label {
        client_project::Label::Key(value) => Text::client_key(value.clone()),
        client_project::Label::Content(value) => Text::content(value.clone()),
    }
}
fn project_view(value: &client_project::View) -> ui_project::AddProjectView {
    use crate::ui::icon::Icon;
    ui_project::AddProjectView {
        revision: value.revision,
        step: match value.step {
            client_project::Step::Environments => ui_project::Step::Environments,
            client_project::Step::Sources => ui_project::Step::Sources,
            client_project::Step::LocalFolder => ui_project::Step::LocalFolder,
            client_project::Step::NewProject => ui_project::Step::NewProject,
            client_project::Step::Repository => ui_project::Step::Repository,
            client_project::Step::CloneDestination => ui_project::Step::CloneDestination,
        },
        query: value.query.clone(),
        placeholder: project_label(&value.placeholder),
        groups: value
            .groups
            .iter()
            .map(|group| ui_project::ProjectGroup {
                label: project_label(&group.label),
                items: group
                    .items
                    .iter()
                    .map(|item| ui_project::ProjectItem {
                        id: item.id.clone(),
                        title: project_label(&item.title),
                        description: item.description.as_ref().map(project_label),
                        icon: match item.glyph {
                            client_project::Glyph::Environment => Icon::Monitor,
                            client_project::Glyph::Folder => Icon::Folder,
                            client_project::Glyph::NewRepository => Icon::FolderGit,
                            client_project::Glyph::Repository => Icon::Link,
                            client_project::Glyph::Settings => Icon::Settings,
                        },
                        disabled: item.disabled,
                        setup_required: item.setup_required,
                    })
                    .collect(),
            })
            .collect(),
        summary: value.summary.as_ref().map(|summary| match summary {
            client_project::Summary::New {
                name,
                path,
                root,
                environment,
            } => ui_project::Summary::New {
                name: name.clone(),
                path: path.clone(),
                root: root.clone(),
                environment: environment.clone(),
            },
            client_project::Summary::Repository { title, url } => ui_project::Summary::Repository {
                title: title.clone(),
                url: url.clone(),
                icon: Icon::Link,
            },
        }),
        can_submit: value.can_submit,
        submit_label: project_label(&value.submit_label),
        submit_accessibility: project_label(&value.submit_accessibility),
        can_go_back: value.can_go_back,
        folder_picker_label: value.folder_picker_label.as_ref().map(project_label),
        can_pick_folder: value.can_pick_folder,
        loading_folders: value.loading_folders,
        error: value.error.as_ref().map(|e| Text::client_key(e.clone())),
        empty_message: value.empty_message.as_ref().map(project_label),
    }
}
struct AddProjectBinding<P: Platform + Clone + 'static>(client_project::AddProjectSession<P>);
pub fn add_project_runtime<P: Platform + Clone + 'static>(
    session: client_project::AddProjectSession<P>,
) -> ui_project::AddProjectRuntime {
    ui_project::AddProjectRuntime(Rc::new(AddProjectBinding(session)))
}
impl<P: Platform + Clone + 'static> ui_project::AddProjectClient for AddProjectBinding<P> {
    fn watch(&self) -> LocalBoxStream<'static, Rc<ui_project::AddProjectView>> {
        self.0
            .watch()
            .map(|view| Rc::new(project_view(&view)))
            .boxed_local()
    }
    fn completed(&self) -> LocalBoxStream<'static, ()> {
        self.0.completed()
    }
    fn publishing(&self) -> LocalBoxStream<'static, Option<Rc<ui_project::PublishingView>>> {
        let session = self.0.clone();
        self.0
            .changes()
            .map(move |_| {
                session.publishing().map(|value| {
                    Rc::new(ui_project::PublishingView {
                        title: project_label(&value.title),
                        description: Text::content(value.repository),
                        selected: value.selected.kind,
                        targets: value
                            .targets
                            .into_iter()
                            .map(|target| ui_project::PublishChoice {
                                kind: target.kind,
                                label: Text::content(target.label),
                            })
                            .collect(),
                        publishes: value.publishes,
                        can_change: value.can_change,
                    })
                })
            })
            .boxed_local()
    }
    fn set_publishing(&self, value: bool, kind: Option<String>) -> Result<(), Text> {
        self.0.set_publishing(value, kind).map_err(error)
    }
    fn set_query(&self, query: String) -> Result<(), Text> {
        self.0.set_query(query).map_err(error)
    }
    fn back(&self) -> Result<(), Text> {
        self.0.back().map_err(error)
    }
    fn select_item(
        &self,
        id: &str,
    ) -> LocalBoxFuture<'static, Result<Option<ui_new::Destination>, Text>> {
        let session = self.0.clone();
        let id = id.to_owned();
        async move {
            session
                .select_item(&id)
                .await
                .map(|value| value.map(destination))
                .map_err(error)
        }
        .boxed_local()
    }
    fn submit(
        &self,
        revision: u64,
    ) -> LocalBoxFuture<'static, Result<Option<ui_new::Destination>, Text>> {
        let session = self.0.clone();
        async move {
            session
                .submit(revision)
                .await
                .map(|value| value.map(destination))
                .map_err(error)
        }
        .boxed_local()
    }
    fn pick_folder(
        &self,
        revision: u64,
    ) -> LocalBoxFuture<'static, Result<Option<ui_new::Destination>, Text>> {
        let session = self.0.clone();
        async move {
            session
                .pick_folder(revision)
                .await
                .map(|value| value.map(destination))
                .map_err(error)
        }
        .boxed_local()
    }
}
struct IgnoredBinding<P: Platform + Clone + 'static>(client_ignored::IgnoredFilesSession<P>);
pub fn ignored_files_client<P: Platform + Clone + 'static>(
    session: client_ignored::IgnoredFilesSession<P>,
) -> Rc<dyn ui_project::IgnoredFilesClient> {
    Rc::new(IgnoredBinding(session))
}
impl<P: Platform + Clone + 'static> ui_project::IgnoredFilesClient for IgnoredBinding<P> {
    fn watch(&self) -> LocalBoxStream<'static, Rc<ui_project::IgnoredFilesView>> {
        self.0
            .watch()
            .map(|value| {
                let mut selected_summary = crate::i18n::tr(
                    "{count} of {total}",
                    &[
                        (
                            "count",
                            crate::i18n::Value::Int(value.selected_count as i64),
                        ),
                        ("total", crate::i18n::Value::Int(value.total_count as i64)),
                    ],
                );
                if value.selected_bytes > 0 {
                    selected_summary.push_str(&format!(" · {}", value.selected_size));
                }
                Rc::new(ui_project::IgnoredFilesView {
                    query: value.query.clone(),
                    can_search: value.can_search,
                    show_search: value.show_search,
                    loading: value.loading,
                    searching: value.searching,
                    truncated: value.truncated,
                    all_shown_selected: value.all_shown_selected,
                    selected_summary,
                    error: value.error.as_ref().map(|e| Text::client_key(e.clone())),
                    files: value
                        .files
                        .iter()
                        .map(|choice| ui_project::IgnoredFileChoice {
                            file: choice.file.clone(),
                            size_label: if choice.too_large {
                                Text::client_key("Over 1 MB")
                            } else {
                                Text::content(choice.size.clone())
                            },
                            selected: choice.selected,
                            disabled: choice.disabled,
                        })
                        .collect(),
                })
            })
            .boxed_local()
    }
    fn set_query(&self, query: String) -> Result<(), Text> {
        self.0.set_query(query).map_err(error)
    }
    fn toggle_file(&self, path: &str, checked: bool) -> Result<(), Text> {
        self.0.toggle_file(path, checked).map_err(error)
    }
    fn toggle_shown(&self) -> Result<(), Text> {
        self.0.toggle_shown().map_err(error)
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
        client_new::NewThreadView::LinkOpened => ui_new::NewThreadView::LinkOpened,
        client_new::NewThreadView::Loading => ui_new::NewThreadView::Loading,
        client_new::NewThreadView::Empty { can_start_scratch } => ui_new::NewThreadView::Empty {
            can_start_scratch: *can_start_scratch,
        },
        client_new::NewThreadView::StartFailed => ui_new::NewThreadView::StartFailed,
        client_new::NewThreadView::LinkChoice {
            title,
            message,
            choices,
        } => ui_new::NewThreadView::LinkChoice {
            title: link_text(title),
            message: link_text(message),
            choices: choices
                .iter()
                .map(|choice| ui_new::LinkChoice {
                    action: link_action_to_ui(choice.action),
                    label: link_text(&choice.label),
                    disabled_reason: choice.disabled_reason.as_ref().map(link_text),
                })
                .collect(),
        },
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
fn link_text(text: &client_new::LinkText) -> Text {
    let arguments = text
        .arguments
        .iter()
        .map(|(key, value)| (key.as_str(), crate::i18n::Value::Text(value.clone())))
        .collect::<Vec<_>>();
    Text::content(crate::i18n::tr(&text.key, &arguments))
}
fn link_action_to_ui(action: client_new::LinkAction) -> ui_new::LinkAction {
    match action {
        client_new::LinkAction::CloneRepository => ui_new::LinkAction::CloneRepository,
        client_new::LinkAction::UseWithoutProject => ui_new::LinkAction::UseWithoutProject,
        client_new::LinkAction::CheckoutPullRequest => ui_new::LinkAction::CheckoutPullRequest,
        client_new::LinkAction::ReplaceDraft => ui_new::LinkAction::ReplaceDraft,
    }
}
impl<P: Platform + Clone + 'static> ui_new::NewThreadClient for NewThreadBinding<P> {
    fn watch_link(
        &self,
        search: String,
        entry: String,
    ) -> LocalBoxStream<'static, Rc<ui_new::NewThreadView>> {
        self.0
            .watch_link(search, entry)
            .map(|v| Rc::new(new_view(&v)))
            .boxed_local()
    }
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
    fn model_picker(
        &self,
        id: &str,
    ) -> Option<crate::screens::composer::model_picker::PickerProps> {
        use crate::screens::composer::model_picker::{CatalogState, PickerProps};
        use nsh_core::prelude::*;
        let service = self.0.model_session(id).ok()?;
        // Picks close the picker that made them; their writes run under the draft's owner.
        let tasks = crate::runtime::tasks::TaskOwner::here();
        let navigation = self.0.clone();
        let id = id.to_owned();
        let resolve = Rc::new(move || navigation.model_session(&id));
        let current = signal(service.snapshot());
        let failure = signal(None::<String>);
        let mut changes = futures::stream::select(service.changes(), self.0.changes());
        let watching = resolve.clone();
        crate::runtime::tasks::watch(async move {
            while changes.next().await.is_some() {
                match watching() {
                    Ok(service) => current.set(service.snapshot()),
                    Err(error) => failure.set(Some(error.to_string())),
                }
            }
        });
        let picking = resolve.clone();
        let mut props = PickerProps::new(
            computed(move || super::composer::model_picker_data(&current.get())).into(),
            move |model| {
                failure.set(None);
                let service = match picking() {
                    Ok(service) => service,
                    Err(error) => {
                        failure.set(Some(error.to_string()));
                        return;
                    }
                };
                let operation =
                    service.set_model_selection(t3_contracts::model_selection::ModelSelection {
                        instance_id: model.instance_id,
                        model: model.model,
                        options: None,
                    });
                tasks.spawn(async move {
                    if let Err(error) = operation.await {
                        failure.set(Some(error.to_string()));
                    }
                });
            },
        );
        props.catalog = computed(move || {
            failure.get().map_or_else(
                || super::composer::model_catalog_state(&current.get().state),
                CatalogState::Error,
            )
        })
        .into();
        props.traits = computed(move || super::composer::model_traits(&current.get())).into();
        props.search = Some(super::composer::ranked_model_search());
        let favorites = resolve.clone();
        props.on_toggle_favorite = Some(Rc::new(move |model| {
            let service = match favorites() {
                Ok(service) => service,
                Err(error) => {
                    failure.set(Some(error.to_string()));
                    return;
                }
            };
            let operation = service.toggle_model_favorite(model.instance_id, model.model);
            tasks.spawn(async move {
                if let Err(error) = operation.await {
                    failure.set(Some(error.to_string()));
                }
            });
        }));
        let options = resolve.clone();
        props.on_option_pick = Some(Rc::new(move |model, selection| {
            let service = match options() {
                Ok(service) => service,
                Err(error) => {
                    failure.set(Some(error.to_string()));
                    return;
                }
            };
            let operation = service.change_model_trait(model.instance_id, model.model, selection);
            tasks.spawn(async move {
                if let Err(error) = operation.await {
                    failure.set(Some(error.to_string()));
                }
            });
        }));
        props.on_refresh = Some(Rc::new(move || {
            failure.set(None);
            let service = match resolve() {
                Ok(service) => service,
                Err(error) => {
                    failure.set(Some(error.to_string()));
                    return;
                }
            };
            let operation = service.refresh_model_catalog();
            tasks.spawn(async move {
                if let Err(error) = operation.await {
                    failure.set(Some(error.to_string()));
                }
            });
        }));
        Some(props)
    }
    fn link_action(
        &self,
        search: String,
        action: ui_new::LinkAction,
    ) -> LocalBoxFuture<'static, Result<Option<ui_new::Destination>, Text>> {
        let service = self.0.clone();
        let action = match action {
            ui_new::LinkAction::CloneRepository => client_new::LinkAction::CloneRepository,
            ui_new::LinkAction::UseWithoutProject => client_new::LinkAction::UseWithoutProject,
            ui_new::LinkAction::CheckoutPullRequest => client_new::LinkAction::CheckoutPullRequest,
            ui_new::LinkAction::ReplaceDraft => client_new::LinkAction::ReplaceDraft,
        };
        async move {
            service
                .link_action(search, action)
                .await
                .map(|result| result.map(destination))
                .map_err(error)
        }
        .boxed_local()
    }
}

pub fn palette_search_runtime<P: Platform + Clone + 'static>(
    service: t3_client::navigation::palette_service::PaletteService<P>,
    open_file: crate::screens::palette::files::OpenFile,
) -> crate::screens::palette::files::PaletteSearchRuntime {
    crate::screens::palette::files::PaletteSearchRuntime {
        client: Rc::new(service),
        open_file,
    }
}
