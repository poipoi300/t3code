//! Port of apps/web/src/composerDraftStore.ts composer draft subscription and editor binding.

use std::rc::Rc;

use futures::{StreamExt, stream::LocalBoxStream};
use nsh_core::prelude::*;
use t3_client::ClientError;
use t3_client::types::ThreadKey;

use super::editor::{ComposerEditor, ComposerSnapshot};
use crate::i18n::Text;
use crate::runtime::Shared;

/// The client owns the draft identity, revision, hydration and saved caret, across mounts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerDraftView {
    pub draft_key: String,
    pub revision: u64,
    pub prompt: Rc<str>,
    pub cursor: usize,
    pub loaded: bool,
    pub writable: bool,
    pub error: Option<Text>,
}

/// Adapter for the composed client draft service; implementations must not live in the UI.
/// Mutations merge with the current client draft, retaining attachments, context and model picks.
pub trait ComposerDraftClient {
    fn changes(&self, key: &ThreadKey) -> LocalBoxStream<'static, ()>;
    fn view(&self, key: &ThreadKey) -> Rc<ComposerDraftView>;
    fn set_prompt(
        &self,
        key: &ThreadKey,
        prompt: String,
        utf16_cursor: usize,
    ) -> Result<(), ClientError>;
}

/// A scoped subscription, created under the composer's owner. NSH cancels its task on unmount.
#[derive(Clone, Copy)]
pub struct ComposerDrafts {
    client: Stored<Rc<dyn ComposerDraftClient>>,
    key: Stored<ThreadKey>,
    pub view: Signal<Shared<ComposerDraftView>>,
    pub error: Signal<Option<Text>>,
}

impl ComposerDrafts {
    pub fn attach(client: Rc<dyn ComposerDraftClient>, key: ThreadKey) -> Self {
        let mut changes = client.changes(&key);
        let view = signal(Shared(client.view(&key)));
        let source = client.clone();
        let scope = key.clone();
        spawn(async move {
            while changes.next().await.is_some() {
                view.set(Shared(source.view(&scope)));
            }
        });
        Self {
            client: stored(client),
            key: stored(key),
            view,
            error: signal(None),
        }
    }

    pub fn disabled(self) -> bool {
        self.view.with(|view| !view.loaded || !view.writable)
    }

    /// External draft changes synchronize the native editor; no persistence or draft policy here.
    pub fn bind_editor(self, editor: ComposerEditor) {
        effect(move || {
            let draft = self.view.get();
            if !draft.loaded {
                return;
            }
            untracked(|| {
                let result = editor.sync_draft(&draft.draft_key, &draft.prompt, draft.cursor);
                self.error
                    .set(result.err().map(|error| Text::content(error.to_string())));
            });
        });
    }

    /// Pass directly as CMP-EDITOR-1's on-change callback; the cursor uses source UTF-16 offsets.
    pub fn on_change(self, snapshot: ComposerSnapshot) {
        if untracked(|| self.disabled()) {
            return;
        }
        let result = self.client.with(|client| {
            self.key
                .with(|key| client.set_prompt(key, snapshot.value, snapshot.cursor))
        });
        self.error.set(result.err().map(client_error));
    }

    pub fn failure(self) -> Option<Text> {
        self.error
            .get()
            .or_else(|| self.view.with(|view| view.error.clone()))
    }
}

fn client_error(error: ClientError) -> Text {
    match error {
        ClientError::InvalidPayload(message) => Text::client_key(message),
        ClientError::Rejected { message, detail } => Text::content(
            t3_client::error::rejection_text(&message, detail.as_deref()),
        ),
        ClientError::Disconnected => "RPC disconnected".into(),
        ClientError::EnvironmentMismatch => "unexpected environment identity".into(),
        ClientError::Cancelled => "cancelled".into(),
        other => Text::content(other.to_string()),
    }
}
