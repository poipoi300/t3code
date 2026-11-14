use super::*;
use crate::thread_views::composer::{actions::ComposerActionsSession, draft_service::ComposerDraftService};

impl<P: Platform + Clone + 'static> ClientRuntime<P> {
    pub fn composer_drafts(&self) -> Rc<ComposerDraftService<P>> { self.inner.composer_drafts.clone() }

    pub fn composer_primary_actions(&self, key: &ThreadKey) -> ComposerActionsSession<P> {
        ComposerActionsSession::new(self.clone(), key.clone())
    }

    pub fn interrupt_thread(&self, key: &ThreadKey) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let runtime = self.clone();
        let key = key.clone();
        async move {
            let endpoint = runtime.client(&key.environment).and_then(|c| c.session()).ok_or(ClientError::Disconnected)?;
            let supports = runtime.store().snapshot().environments.get(&key.environment)
                .and_then(|e| e.config.as_ref()).is_some_and(|c| c.environment.capabilities.thread_stop == Some(true));
            crate::thread_views::composer::interrupt::interrupt_thread(&runtime.inner.platform, runtime.store(), endpoint, &key, supports).await
        }.boxed_local()
    }
}
