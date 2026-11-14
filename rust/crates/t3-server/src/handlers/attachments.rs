//! `attachments.createUploadUrl` and `attachments.delete` over the upload ledger
//! (`adapters/attachments.rs`): the calling session is the upload's issuer. Without the ledger
//! (no workspace or orchestration) both stay unserved.
use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::*;

pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(all(feature = "workspace", feature = "orchestration"))]
    if let Some(adapter) = domains.get::<crate::adapters::attachments::AttachmentsAdapter>() {
        use crate::{
            adapters::attachments::Principal,
            registry::{Failure, ServerDefect},
        };
        let issuer = |ctx: &crate::registry::Context| Principal::Session(ctx.session_id.0.clone());
        let uploads = adapter.clone();
        registry.register::<WsAttachmentsCreateUploadUrlRpc, _, _>(move |ctx, input| {
            let uploads = uploads.clone();
            let issuer = issuer(&ctx);
            async move {
                uploads
                    .create_upload_url(issuer, input)
                    .await
                    .map_err(|error| {
                        Failure::Typed(
                            WsAttachmentsCreateUploadUrlError::AttachmentUploadSigningKeyError(
                                Box::new(t3_contracts::assets::AttachmentUploadSigningKeyError {
                                    tag: t3_contracts::assets::AttachmentUploadSigningKeyErrorTag::Error,
                                    cause: error.to_string().into(),
                                }),
                            ),
                        )
                    })
            }
        })?;
        registry.register::<WsAttachmentsDeleteRpc, _, _>(move |ctx, input| {
            let adapter = adapter.clone();
            let issuer = issuer(&ctx);
            async move {
                adapter
                    .delete_pending(&issuer, &input.attachment_id.0)
                    .await
                    .map_err(|error| {
                        Failure::Defect(ServerDefect::Internal {
                            message: error.to_string(),
                        })
                    })
            }
        })?;
        return Ok(());
    }
    let _ = domains;
    registry.unimplemented::<WsAttachmentsCreateUploadUrlRpc>()?;
    registry.unimplemented::<WsAttachmentsDeleteRpc>()?;
    Ok(())
}
