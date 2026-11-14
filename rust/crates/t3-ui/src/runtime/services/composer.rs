//! Thin bindings of the shared composer services.
use std::rc::Rc;
use futures::stream::LocalBoxStream;
use t3_client::{ClientError, composition::ClientRuntime, platform::Platform, types::ThreadKey};
use crate::{i18n::Text, screens::composer::{drafts::{ComposerDraftClient, ComposerDraftView}, send::{PrimaryActionState, FollowUpBehavior}}};

struct DraftBinding<P: Platform + 'static>(Rc<t3_client::thread_views::composer::draft_service::ComposerDraftService<P>>);

pub fn composer_drafts<P: Platform + Clone + 'static>(runtime: &ClientRuntime<P>) -> Rc<dyn ComposerDraftClient> {
    Rc::new(DraftBinding(runtime.composer_drafts()))
}

impl<P: Platform + 'static> ComposerDraftClient for DraftBinding<P> {
    fn changes(&self, _: &ThreadKey) -> LocalBoxStream<'static, ()> { self.0.changes() }
    fn view(&self, key: &ThreadKey) -> Rc<ComposerDraftView> {
        let view = self.0.view(key);
        Rc::new(ComposerDraftView { draft_key: view.draft_key.clone(), revision: view.revision,
            prompt: view.prompt.clone(), cursor: view.cursor, loaded: view.loaded,
            writable: view.writable, error: view.error.as_ref().map(|e| Text::content(e.to_string())) })
    }
    fn set_prompt(&self, key: &ThreadKey, prompt: String, cursor: usize) -> Result<(), ClientError> { self.0.set_prompt(key, prompt, cursor) }
}

pub fn primary_action_state(state: &t3_client::thread_views::composer::actions::PrimaryActionState) -> PrimaryActionState {
    PrimaryActionState { can_operate_thread: state.can_operate_thread, is_running: state.is_running,
        can_interrupt: state.can_interrupt, run_id: state.run_id.clone(),
        follow_up_behavior: state.follow_up_behavior.map(|value| match value {
            t3_client::thread_views::composer::actions::FollowUpBehavior::Queue => FollowUpBehavior::Queue,
            t3_client::thread_views::composer::actions::FollowUpBehavior::Steer => FollowUpBehavior::Steer,
        }), can_steer: state.can_steer, is_send_busy: state.is_send_busy,
        send_disabled_reason: state.send_disabled_reason.clone().map(Text::client_key),
        is_environment_unavailable: state.is_environment_unavailable,
        is_preparing_worktree: state.is_preparing_worktree, has_sendable_content: state.has_sendable_content,
        can_resume: state.can_resume, ..Default::default() }
}
