use super::{
    attachment_intake::AttachmentItem, context_records::draft_chip_records, target::ComposerTarget,
};
use crate::{
    ClientError,
    composition::ClientRuntime,
    platform::Platform,
    types::{ThreadId, ThreadKey},
};
use futures::{StreamExt, stream::LocalBoxStream};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_contracts::{
    composer_context::ComposerContextRecord,
    server::{ServerProvider, ServerProviderSkill},
};

#[derive(Clone, Default, PartialEq)]
pub struct ComposerChipsView {
    pub records: BTreeMap<String, ComposerContextRecord>,
    pub skills: Vec<ServerProviderSkill>,
    pub providers: Vec<ServerProvider>,
    pub previews: BTreeMap<String, String>,
    pub video_attachments: BTreeSet<String>,
    pub draft_attachments: Vec<AttachmentItem>,
}
#[derive(Clone)]
pub struct ComposerChipsSession<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
    target: ComposerTarget,
    view: Rc<RefCell<Option<Rc<ComposerChipsView>>>>,
}
impl<P: Platform + Clone + 'static> ComposerChipsSession<P> {
    pub fn new(runtime: ClientRuntime<P>, target: ComposerTarget) -> Self {
        Self {
            runtime,
            target,
            view: Rc::default(),
        }
    }
    fn key(&self) -> Option<ThreadKey> {
        match &self.target {
            ComposerTarget::Thread(key) => Some(key.clone()),
            ComposerTarget::Draft {
                draft_id,
                environment_id,
            } => {
                let document = self.runtime.composer_drafts().document();
                let document = document.borrow();
                Some(ThreadKey {
                    environment: environment_id.clone()?,
                    thread: ThreadId(
                        document["draftThreadsByThreadKey"][draft_id]["threadId"]
                            .as_str()?
                            .into(),
                    ),
                })
            }
        }
    }
    pub fn snapshot(&self) -> Result<Rc<ComposerChipsView>, ClientError> {
        let drafts = self.runtime.composer_drafts();
        let document = drafts.document();
        let document = document.borrow();
        let id = self
            .target
            .draft_key(&document)
            .ok_or(ClientError::Cancelled)?;
        let records = draft_chip_records(&document["draftsByThreadKey"][&id])?;
        drop(document);
        let mut next = ComposerChipsView {
            records,
            ..Default::default()
        };
        if let Some(key) = self.key() {
            let models = self.runtime.composer_models(&key).snapshot();
            let context = self.runtime.git_panel().workspace_context(&key, None);
            let cwd = context.as_ref().map(|context| {
                context
                    .worktree_path
                    .as_deref()
                    .unwrap_or(&context.project.workspace_root.0)
            });
            if let Some(provider) = models
                .providers
                .iter()
                .find(|provider| provider.snapshot.instance_id == models.selection.instance_id)
            {
                let workspace =
                    provider
                        .snapshot
                        .workspace_snapshots
                        .as_ref()
                        .and_then(|snapshots| {
                            snapshots
                                .iter()
                                .find(|snapshot| Some(snapshot.cwd.0.as_str()) == cwd)
                        });
                next.skills = workspace.map_or_else(
                    || provider.snapshot.skills.clone(),
                    |workspace| workspace.skills.clone(),
                );
            }
            next.providers = models
                .providers
                .iter()
                .map(|provider| provider.snapshot.clone())
                .collect();
        }
        if let Ok(attachments) = self.runtime.composer_attachments(&self.target) {
            next.draft_attachments = attachments.items();
            for item in &next.draft_attachments {
                if let Some(preview) = &item.preview {
                    next.previews.insert(item.id.clone(), preview.clone());
                }
                if item.video {
                    next.video_attachments.insert(item.id.clone());
                }
            }
        }
        let mut view = self.view.borrow_mut();
        if let Some(prior) = view.as_ref().filter(|prior| ***prior == next) {
            return Ok(prior.clone());
        }
        let next = Rc::new(next);
        *view = Some(next.clone());
        Ok(next)
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        let mut streams = vec![self.runtime.composer_drafts().changes()];
        if let Some(id) = self.target.environment() {
            streams.push(
                self.runtime
                    .store()
                    .subscribe_environment(id)
                    .map(|_| ())
                    .boxed_local(),
            );
        }
        if let Ok(attachments) = self.runtime.composer_attachments(&self.target) {
            streams.push(attachments.changes());
        }
        futures::stream::select_all(streams).boxed_local()
    }
}
