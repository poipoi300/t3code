//! Thin bindings for the client thread and project operations.
use crate::screens::thread::header::{
    HeaderError, ThreadHeaderRuntime, ThreadHeaderService, ThreadHeaderState,
};
use crate::screens::thread::rows::{
    approval::ApprovalServices,
    plan::{DownloadPlan, WritePlan},
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use std::rc::Rc;
use t3_client::types::ThreadKey;
use t3_client::{ClientError, composition::ClientRuntime, platform::Platform};

struct HeaderBinding<P: Platform + Clone + 'static>(
    t3_client::thread_views::thread_ops::ThreadOperations<P>,
);
struct HeaderNavigationBinding<P: Platform + Clone + 'static> {
    service: t3_client::thread_views::thread_ops::ThreadOperations<P>,
    origin: ThreadKey,
}
pub fn thread_header_navigation<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
    origin: ThreadKey,
) -> crate::screens::thread::header::ThreadHeaderNavigation {
    crate::screens::thread::header::ThreadHeaderNavigation(Rc::new(HeaderNavigationBinding {
        service: client.thread_header(),
        origin,
    }))
}
impl<P: Platform + Clone + 'static> crate::screens::thread::header::ThreadHeaderNavigationService
    for HeaderNavigationBinding<P>
{
    fn watch(
        &self,
        project: t3_client::workspace::ProjectKey,
    ) -> LocalBoxStream<'static, Option<crate::i18n::Text>> {
        self.service
            .watch_project_new_thread(project, self.origin.clone())
            .map(|reason| reason.map(crate::i18n::Text::client_key))
            .boxed_local()
    }
    fn new_thread_in_project(
        &self,
        project: t3_client::workspace::ProjectKey,
    ) -> LocalBoxFuture<'static, Result<crate::screens::new_thread::Destination, HeaderError>> {
        self.service
            .new_thread_in_project(project, self.origin.clone())
            .map(|result| {
                result
                    .map(|destination| match destination {
                        t3_client::navigation::new_thread::Destination::Draft(id) => {
                            crate::screens::new_thread::Destination::Draft(id)
                        }
                        t3_client::navigation::new_thread::Destination::Thread(key) => {
                            crate::screens::new_thread::Destination::Thread(key)
                        }
                    })
                    .map_err(|error| header_error("new-thread-in-project", error))
            })
            .boxed_local()
    }
}
pub fn thread_header<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
) -> ThreadHeaderRuntime {
    bind_confirmation(client);
    ThreadHeaderRuntime(Rc::new(HeaderBinding(client.thread_header())))
}
pub fn bind_confirmation<P: Platform + Clone + 'static>(client: &ClientRuntime<P>) {
    client
        .thread_actions()
        .bind_confirmation(Rc::new(|message, destructive| {
            let request = crate::ui::confirm::request_confirm_dialog(
                message,
                Some(crate::ui::confirm::ConfirmDialogOptions {
                    variant: if destructive {
                        crate::ui::confirm::ConfirmDialogVariant::Destructive
                    } else {
                        crate::ui::confirm::ConfirmDialogVariant::Default
                    },
                    initial_focus: Some(crate::ui::confirm::ConfirmDialogInitialFocus::Cancel),
                }),
            );
            async move {
                match request {
                    Some(answer) => Ok(answer.await),
                    None => Err(ClientError::Unsupported("thread action confirmation")),
                }
            }
            .boxed_local()
        }));
}
fn header_error(action: &str, error: ClientError) -> HeaderError {
    match error {
        ClientError::Cancelled => HeaderError::Interrupted,
        error => HeaderError::Failed {
            title: t3_client::thread_views::thread_ops::header::action_failure_title(action).into(),
            detail: Some(crate::i18n::Text::content(display_error(error))),
        },
    }
}
impl<P: Platform + Clone + 'static> ThreadHeaderService for HeaderBinding<P> {
    fn watch(&self, key: ThreadKey) -> LocalBoxStream<'static, Rc<ThreadHeaderState>> {
        self.0
            .watch_header(key)
            .map(|state| {
                Rc::new(ThreadHeaderState {
                    can_operate: state.can_operate,
                    unavailable_reason: state
                        .menu
                        .unavailable_reason
                        .clone()
                        .map(crate::i18n::Text::client_key),
                    menu: Some(crate::screens::sidebar::actions::ThreadActionMenu {
                        items: state.menu.items.clone(),
                        unavailable_reason: state.menu.unavailable_reason.clone(),
                    }),
                    is_scratch: state.is_scratch,
                })
            })
            .boxed_local()
    }
    fn rename_thread(
        &self,
        key: ThreadKey,
        title: String,
    ) -> LocalBoxFuture<'static, Result<(), HeaderError>> {
        self.0
            .rename_thread(key, title)
            .map(|r| r.map_err(|error| header_error("rename", error)))
            .boxed_local()
    }
    fn perform_action(
        &self,
        key: ThreadKey,
        action: String,
    ) -> LocalBoxFuture<'static, Result<(), HeaderError>> {
        self.0
            .perform_action(key, action.clone())
            .map(move |r| r.map_err(|error| header_error(&action, error)))
            .boxed_local()
    }
    fn close_guest_share(
        &self,
        key: ThreadKey,
    ) -> LocalBoxFuture<'static, Result<(), HeaderError>> {
        self.0
            .close_guest_share(key)
            .map(|r| r.map_err(|error| header_error("close-guest-share", error)))
            .boxed_local()
    }
}

pub fn thread_action_menu<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
    key: &ThreadKey,
) -> crate::screens::sidebar::actions::ThreadActionMenu {
    let menu = client.thread_actions().thread_action_menu(key);
    crate::screens::sidebar::actions::ThreadActionMenu {
        items: menu.items,
        unavailable_reason: menu.unavailable_reason,
    }
}

fn display_error(error: ClientError) -> String {
    match error {
        ClientError::Rejected { message, detail } => {
            t3_client::error::rejection_text(&message, detail.as_deref())
        }
        ClientError::InvalidPayload(message) => {
            crate::i18n::Text::client_key(message).resolve().to_string()
        }
        error => crate::i18n::Text::client_key(error.to_string())
            .resolve()
            .to_string(),
    }
}
pub fn approval_services<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
) -> ApprovalServices {
    let availability = client.approval_services();
    let operation = client.approval_services();
    ApprovalServices {
        can_operate: Rc::new(move |key| availability.approvals_available(key)),
        respond: Rc::new(move |response| {
            operation
                .respond(response.thread, response.request_id, response.decision)
                .map(|r| r.map_err(display_error))
                .boxed_local()
        }),
    }
}
pub fn write_plan<P: Platform + Clone + 'static>(client: &ClientRuntime<P>) -> WritePlan {
    let service = client.plan_services();
    Rc::new(move |save| {
        service
            .write_workspace_file(save.thread, save.cwd, save.relative_path, save.contents)
            .map(|r| r.map_err(display_error))
            .boxed_local()
    })
}
pub fn download_plan<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
) -> Option<DownloadPlan> {
    let service = client.plan_services();
    service.can_download().then(|| {
        Rc::new(move |filename, contents| {
            service
                .download_text(filename, contents, "text/markdown;charset=utf-8".into())
                .map(|r| r.map_err(display_error))
                .boxed_local()
        }) as DownloadPlan
    })
}

struct RewindBinding<P: Platform + Clone + 'static>(
    t3_client::thread_views::thread_ops::ThreadOperations<P>,
);
pub fn timeline_rewind<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
) -> crate::screens::thread::rewind::RewindService {
    crate::screens::thread::rewind::RewindService(Rc::new(RewindBinding(client.timeline_rewind())))
}
fn rewind_error(error: ClientError) -> crate::screens::thread::rewind::RewindError {
    match error {
        ClientError::Rejected { message, detail } => {
            crate::screens::thread::rewind::RewindError { message, detail }
        }
        ClientError::InvalidPayload(message) => crate::screens::thread::rewind::RewindError {
            message,
            detail: None,
        },
        error => crate::screens::thread::rewind::RewindError {
            message: error.to_string(),
            detail: None,
        },
    }
}
impl<P: Platform + Clone + 'static> crate::screens::thread::rewind::TimelineRewindClient
    for RewindBinding<P>
{
    fn state(&self, key: &ThreadKey) -> crate::screens::thread::rewind::RewindState {
        let state = self.0.rewind_state(key);
        crate::screens::thread::rewind::RewindState {
            reverting: state.reverting,
            can_fork: state.can_fork,
        }
    }
    fn availability(
        &self,
        key: &ThreadKey,
        target: &crate::screens::thread::rewind::RewindTarget,
    ) -> crate::screens::thread::rewind::RewindAvailability {
        let state = self.0.rewind_availability(
            key,
            &t3_client::thread_views::thread_ops::rewind::RewindTarget {
                message_id: target.message_id.clone(),
                turn_count: target.turn_count,
            },
        );
        crate::screens::thread::rewind::RewindAvailability {
            can_rewind: state.can_rewind,
            can_restore_files: state.can_restore_files,
            reason: state
                .reason
                .map(|message| crate::screens::thread::rewind::RewindError {
                    message,
                    detail: None,
                }),
        }
    }
    fn changes(&self, key: &ThreadKey) -> LocalBoxStream<'static, ()> {
        self.0.changes(key)
    }
    fn rewind(
        &self,
        key: ThreadKey,
        target: crate::screens::thread::rewind::RewindTarget,
        restore_files: bool,
    ) -> LocalBoxFuture<'static, Result<String, crate::screens::thread::rewind::RewindError>> {
        self.0
            .rewind(
                key,
                t3_client::thread_views::thread_ops::rewind::RewindTarget {
                    message_id: target.message_id,
                    turn_count: target.turn_count,
                },
                restore_files,
            )
            .map(|r| r.map_err(rewind_error))
            .boxed_local()
    }
    fn fork(
        &self,
        key: ThreadKey,
        run_id: t3_contracts::base_schemas::RunId,
    ) -> LocalBoxFuture<'static, Result<ThreadKey, crate::screens::thread::rewind::RewindError>>
    {
        self.0
            .fork(key, run_id)
            .map(|r| r.map_err(rewind_error))
            .boxed_local()
    }
}

pub fn thread_link_metadata<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
    environment: &t3_client::types::EnvironmentId,
    input: t3_contracts::orchestration_v2::OrchestrationV2GetThreadTitlesInput,
) -> Option<
    t3_client::composition::QueryLease<
        std::collections::BTreeMap<
            t3_contracts::base_schemas::ThreadId,
            t3_contracts::orchestration_v2::OrchestrationV2ThreadLinkMetadata,
        >,
    >,
> {
    client
        .thread_metadata()
        .thread_link_metadata(environment, input)
}
struct MarkdownBinding<P: Platform + Clone + 'static>(
    t3_client::thread_views::thread_ops::ThreadOperations<P>,
);
pub fn markdown_links<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
) -> crate::screens::thread::markdown::links::MarkdownLinkRuntime {
    crate::screens::thread::markdown::links::MarkdownLinkRuntime(Rc::new(MarkdownBinding(
        client.thread_metadata(),
    )))
}
impl<P: Platform + Clone + 'static> crate::screens::thread::markdown::links::MarkdownLinkService
    for MarkdownBinding<P>
{
    fn thread_link_metadata(
        &self,
        environment: t3_client::types::EnvironmentId,
        threads: Vec<t3_client::types::ThreadId>,
    ) -> LocalBoxStream<'static, crate::screens::thread::markdown::links::ThreadLinkLookup> {
        self.0
            .watch_thread_link_metadata(environment, threads)
            .map(
                |state| crate::screens::thread::markdown::links::ThreadLinkLookup {
                    settled: state.settled,
                    metadata: state.metadata,
                },
            )
            .boxed_local()
    }
}
pub fn retry_workspace_preparation<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
    key: ThreadKey,
) -> LocalBoxFuture<'static, Result<(), ClientError>> {
    client.thread_metadata().retry_workspace_preparation(key)
}
pub fn subagent_clock<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
    environment: &t3_client::types::EnvironmentId,
    client_ms: f64,
) -> Option<f64> {
    client
        .thread_header()
        .to_server_clock(environment, client_ms)
}

pub fn retry_workspace_preparation_run<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
    key: ThreadKey,
    run: t3_contracts::base_schemas::RunId,
) -> LocalBoxFuture<'static, Result<(), ClientError>> {
    client
        .thread_metadata()
        .retry_workspace_preparation_run(key, run)
}
