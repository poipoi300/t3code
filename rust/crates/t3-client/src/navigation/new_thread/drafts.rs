use crate::{
    ClientError,
    composition::ClientRuntime,
    platform::Platform,
    thread_views::composer::{
        attachment_intake::AttachmentSession, draft_service::ComposerDraftView,
        target::ComposerTarget,
    },
};
use futures::{FutureExt, future::LocalBoxFuture, stream::LocalBoxStream};
use serde_json::Value;
use std::{cell::RefCell, rc::Rc};

pub type DraftEdit = Box<dyn FnOnce(&mut Value) -> Result<(), ClientError>>;

/// The shared composer document owns revisions, persistence and attachment mutations.
pub trait DraftAuthority {
    fn changes(&self) -> LocalBoxStream<'static, ()>;
    fn document(&self) -> Rc<RefCell<Value>>;
    fn view(&self, id: &str) -> Rc<ComposerDraftView>;
    fn update(&self, change: DraftEdit) -> Result<bool, ClientError>;
    fn set_prompt(&self, id: &str, prompt: String, cursor: usize) -> Result<u64, ClientError>;
    fn clear_revision(&self, id: &str, expected: u64) -> Result<bool, ClientError>;
    fn load(&self) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn flush(&self) -> LocalBoxFuture<'static, Result<bool, ClientError>>;
    fn attachments(&self, target: &ComposerTarget) -> Result<AttachmentSession, ClientError>;
    fn creation(
        &self,
        mut creation: crate::thread_views::outbox::QueuedCreation,
        checkout_pull_request_url: Option<String>,
    ) -> Result<crate::thread_views::outbox::QueuedCreation, ClientError> {
        if let Some(url) = checkout_pull_request_url {
            creation.checkout_pull_request_url = Some(url);
            creation.pull_request_reference = None;
        }
        Ok(creation)
    }
}

pub(super) struct ComposerAuthority<P: Platform + Clone + 'static>(pub ClientRuntime<P>);
impl<P: Platform + Clone + 'static> DraftAuthority for ComposerAuthority<P> {
    fn changes(&self) -> LocalBoxStream<'static, ()> {
        self.0.composer_drafts().changes()
    }
    fn document(&self) -> Rc<RefCell<Value>> {
        self.0.composer_drafts().document()
    }
    fn view(&self, id: &str) -> Rc<ComposerDraftView> {
        self.0.composer_drafts().view_key(id)
    }
    fn update(&self, change: DraftEdit) -> Result<bool, ClientError> {
        self.0.composer_drafts().update(change)
    }
    fn set_prompt(&self, id: &str, prompt: String, cursor: usize) -> Result<u64, ClientError> {
        let drafts = self.0.composer_drafts();
        if !drafts.view_key(id).writable {
            return Err(ClientError::InvalidPayload(
                "The composer draft is not writable.".into(),
            ));
        }
        drafts.set_prompt_key(id, prompt, cursor)?;
        Ok(drafts.view_key(id).revision)
    }
    fn load(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let drafts = self.0.composer_drafts();
        async move {
            if !drafts.loaded() {
                drafts.load().await?;
            }
            Ok(())
        }
        .boxed_local()
    }
    fn clear_revision(&self, id: &str, expected: u64) -> Result<bool, ClientError> {
        self.0.composer_drafts().clear_key_revision(id, expected)
    }
    fn flush(&self) -> LocalBoxFuture<'static, Result<bool, ClientError>> {
        let drafts = self.0.composer_drafts();
        async move { drafts.flush().await }.boxed_local()
    }
    fn attachments(&self, target: &ComposerTarget) -> Result<AttachmentSession, ClientError> {
        self.0.composer_attachments(target)
    }
}
