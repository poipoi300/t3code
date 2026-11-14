use super::{draft_service::{ComposerDraftService, draft_key}, send::SendService, web_content, web_context::{self, Kind, Placement}, web_models};
use crate::{ClientError, platform::Platform, rpc::RpcEndpoint, thread_views::{drafts::merge_text, outbox::QueuedMessage}, types::ThreadKey};
use serde_json::{Value, json};
use std::rc::Rc;

impl<P: Platform + 'static, E: RpcEndpoint + 'static> SendService<P, E> {
    pub fn attach_web_drafts(&self, drafts: Rc<ComposerDraftService<P>>) { *self.inner.web_drafts.borrow_mut() = Some(drafts); }

    pub(super) fn capture_web_draft(&self, key: &ThreadKey, text: &str) -> Option<(u64, Value)> {
        let drafts = self.inner.web_drafts.borrow().clone()?;
        let view = drafts.view(key);
        if view.prompt.as_ref() != text { return None; }
        let document = drafts.document();
        let draft = document.borrow()["draftsByThreadKey"][draft_key(key)].clone();
        Some((view.revision, draft))
    }

    pub(super) async fn accept_web_draft(&self, key: &ThreadKey, captured: Option<(u64, Value)>) -> Result<(), ClientError> {
        let drafts = self.inner.web_drafts.borrow().clone();
        if let (Some(drafts), Some((revision, _))) = (drafts, captured) {
            drafts.clear_revision(key, revision)?;
            drafts.flush().await?;
        }
        Ok(())
    }
}

impl<P: Platform + 'static> ComposerDraftService<P> {
    pub async fn recover_message(&self, message: &QueuedMessage) -> Result<String, ClientError> {
        if !self.loaded() { self.load().await?; }
        let key = if message.creation.is_some() { format!("{}:restored-{}", message.environment_id.0, message.message_id.0) }
            else { draft_key(&message.thread_key()) };
        self.update(|document| {
            let receipt = serde_json::to_value(&message.command_id)?;
            if document["draftsByThreadKey"][&key]["recoveredMessages"].get(&message.message_id.0) == Some(&receipt) { return Ok(()); }
            if document["draftsByThreadKey"][&key].get("recoveredMessages").is_some_and(|v| !v.is_object()) {
                return Err(ClientError::InvalidPayload("Unreadable draft recovery receipts were retained.".into()));
            }
            let mut incoming = message.extra.get("composerDraft").cloned().unwrap_or_else(|| json!({"prompt":message.text}));
            if incoming["prompt"].as_str().is_none_or(str::is_empty) { incoming["prompt"] = json!(message.text); }
            let prior = web_models::draft(document, &key);
            let mut next = incoming.clone();
            if let (Some(next), Some(prior)) = (next.as_object_mut(), prior.as_object()) { next.extend(prior.clone()); }
            next["prompt"] = json!(merge_text(prior["prompt"].as_str().unwrap_or_default(), incoming["prompt"].as_str().unwrap_or_default()));
            document["draftsByThreadKey"][&key] = next;
            web_content::add_images(document, &key, incoming["images"].as_array().map_or(&[], Vec::as_slice), false);
            web_content::add_files(document, &key, incoming["files"].as_array().map_or(&[], Vec::as_slice), false, false);
            for kind in [Kind::Terminal, Kind::Preview, Kind::Review, Kind::Thread, Kind::Environment] {
                web_context::add(document, &key, kind, incoming[kind.field()].as_array().map_or(&[], Vec::as_slice), &message.thread_id.0,
                    Placement { append_reference: false, allow_duplicate_reference: false, insertion: None });
            }
            if !document["draftsByThreadKey"][&key]["recoveredMessages"].is_object() { document["draftsByThreadKey"][&key]["recoveredMessages"] = json!({}); }
            document["draftsByThreadKey"][&key]["recoveredMessages"][&message.message_id.0] = receipt;
            Ok(())
        })?;
        self.flush().await?;
        Ok(key)
    }
}
