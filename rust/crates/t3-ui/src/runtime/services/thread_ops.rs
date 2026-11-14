//! Thin bindings for the client thread and project operations.
use crate::screens::thread::rows::{
    approval::ApprovalServices,
    plan::{DownloadPlan, WritePlan},
};
use futures::FutureExt;
use std::rc::Rc;
use t3_client::{ClientError, composition::ClientRuntime, platform::Platform};

fn display_error(error: ClientError) -> String {
    match error {
        ClientError::Rejected { message, detail } => {
            t3_client::error::rejection_text(&message, detail.as_deref())
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
        can_operate: Rc::new(move |key| availability.can_operate(key)),
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
