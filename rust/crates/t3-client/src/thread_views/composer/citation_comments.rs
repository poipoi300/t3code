//! Comment drafts survive node replacement; editor anchors remain opaque to the client.
use super::{
    chip_presentation::{CitationCommentDismissal, resolve_assistant_citation_comment_dismissal},
    citations,
    draft_service::ComposerDraftService,
    send::Changes,
};
use crate::{
    ClientError, platform::Platform, thread_views::document::utf16_byte_offset, types::ThreadKey,
    view_logic::JsTrim,
};
use futures::stream::LocalBoxStream;
use std::{cell::RefCell, rc::Rc};
use t3_contracts::{
    assistant_citations::{ASSISTANT_CITATION_MAX_COMMENT_LENGTH, AssistantCitation},
    wire::trim_js,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CitationInsertedSpaces {
    pub before: bool,
    pub after: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct OpenCitationComment {
    pub key: String,
    pub source_anchor: Option<String>,
    pub start: usize,
    pub source: String,
    pub citation: AssistantCitation,
    pub remove_on_cancel: Option<CitationInsertedSpaces>,
    pub draft: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CitationEdit {
    pub prompt: String,
    pub cursor: usize,
    pub restore_focus: bool,
}
#[derive(Default)]
struct State {
    open: Option<Rc<OpenCitationComment>>,
    changes: Changes,
}
#[derive(Clone)]
pub struct ComposerCitationCommentContext<P: Platform + 'static> {
    drafts: Rc<ComposerDraftService<P>>,
    thread: ThreadKey,
    state: Rc<RefCell<State>>,
}
impl<P: Platform + 'static> ComposerCitationCommentContext<P> {
    pub fn new(drafts: Rc<ComposerDraftService<P>>, thread: ThreadKey) -> Self {
        Self {
            drafts,
            thread,
            state: Rc::default(),
        }
    }
    pub fn snapshot(&self) -> Option<Rc<OpenCitationComment>> {
        self.state.borrow().open.clone()
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        self.state.borrow().changes.subscribe()
    }
    pub fn open(
        &self,
        key: String,
        start: usize,
        source_anchor: Option<String>,
        inserted: Option<CitationInsertedSpaces>,
    ) -> Result<(), ClientError> {
        let view = self.drafts.view(&self.thread);
        let found = citations::collect(&view.prompt)
            .into_iter()
            .find(|citation| citation.range.start == start)
            .ok_or(ClientError::Cancelled)?;
        let mut state = self.state.borrow_mut();
        if state.open.as_ref().is_some_and(|open| open.key == key) {
            return Ok(());
        }
        state.open = Some(Rc::new(OpenCitationComment {
            key,
            source_anchor,
            start,
            source: found.source,
            citation: found.citation,
            remove_on_cancel: inserted,
            draft: None,
        }));
        state.changes.notify();
        Ok(())
    }
    pub fn draft_changed(&self, key: &str, comment: String) -> Result<(), ClientError> {
        let mut state = self.state.borrow_mut();
        let open = state
            .open
            .as_mut()
            .filter(|open| open.key == key)
            .ok_or(ClientError::Cancelled)?;
        Rc::make_mut(open).draft = Some(comment);
        state.changes.notify();
        Ok(())
    }
    pub fn reanchor(&self, key: &str, start: usize, source: &str) -> Result<(), ClientError> {
        let mut state = self.state.borrow_mut();
        let open = state
            .open
            .as_mut()
            .filter(|open| open.key == key && open.source == source)
            .ok_or(ClientError::Cancelled)?;
        Rc::make_mut(open).start = start;
        Ok(())
    }
    fn replace(
        &self,
        open: &OpenCitationComment,
        prompt: &str,
        replacement: &str,
        remove_spaces: bool,
    ) -> Result<CitationEdit, ClientError> {
        let mut from = utf16_byte_offset(prompt, open.start).ok_or(ClientError::Cancelled)?;
        let mut to = from + open.source.len();
        if prompt.get(from..to) != Some(open.source.as_str()) {
            return Err(ClientError::Cancelled);
        }
        if remove_spaces && let Some(spaces) = open.remove_on_cancel {
            if spaces.before && prompt[..from].ends_with(' ') {
                from -= 1;
            }
            if spaces.after && prompt[to..].starts_with(' ') {
                to += 1;
            }
        }
        let cursor = prompt[..from].encode_utf16().count() + replacement.encode_utf16().count();
        Ok(CitationEdit {
            prompt: format!("{}{}{}", &prompt[..from], replacement, &prompt[to..]),
            cursor,
            restore_focus: true,
        })
    }
    fn saved_source(open: &OpenCitationComment, comment: &str) -> Result<String, ClientError> {
        if comment.js_trim().encode_utf16().count() > ASSISTANT_CITATION_MAX_COMMENT_LENGTH {
            return Err(ClientError::InvalidPayload(format!(
                "Comments can be up to {ASSISTANT_CITATION_MAX_COMMENT_LENGTH} characters."
            )));
        }
        let mut citation = open.citation.clone();
        let comment = trim_js(comment);
        citation.comment = (!comment.is_empty()).then(|| Some(comment.into()));
        Ok(citations::serialize(&citation))
    }
    pub fn save(&self, key: &str, comment: &str) -> Result<CitationEdit, ClientError> {
        let open = self
            .snapshot()
            .filter(|open| open.key == key)
            .ok_or(ClientError::Cancelled)?;
        let source = Self::saved_source(&open, comment)?;
        let prompt = self.drafts.view(&self.thread).prompt.clone();
        let edit = self.replace(&open, &prompt, &source, false)?;
        self.drafts
            .set_prompt(&self.thread, edit.prompt.clone(), edit.cursor)?;
        let mut state = self.state.borrow_mut();
        state.open = None;
        state.changes.notify();
        Ok(edit)
    }
    pub fn cancel(&self, key: &str) -> Result<Option<CitationEdit>, ClientError> {
        let open = self
            .snapshot()
            .filter(|open| open.key == key)
            .ok_or(ClientError::Cancelled)?;
        let edit = if open.remove_on_cancel.is_some() {
            let prompt = self.drafts.view(&self.thread).prompt.clone();
            let edit = self.replace(&open, &prompt, "", true)?;
            self.drafts
                .set_prompt(&self.thread, edit.prompt.clone(), edit.cursor)?;
            Some(edit)
        } else {
            None
        };
        let mut state = self.state.borrow_mut();
        state.open = None;
        state.changes.notify();
        Ok(edit)
    }
    pub fn dismiss(&self, key: &str, reason: &str) -> Result<bool, ClientError> {
        let open = self
            .snapshot()
            .filter(|open| open.key == key)
            .ok_or(ClientError::Cancelled)?;
        match resolve_assistant_citation_comment_dismissal(
            reason,
            open.draft.as_deref(),
            open.citation.comment.as_ref().and_then(Option::as_deref),
        ) {
            CitationCommentDismissal::Commit(comment) => {
                self.save(key, &comment)?;
                Ok(true)
            }
            CitationCommentDismissal::KeepOpen => Ok(false),
            CitationCommentDismissal::Close => {
                let mut state = self.state.borrow_mut();
                state.open = None;
                state.changes.notify();
                Ok(true)
            }
        }
    }
    pub fn before_controlled_rewrite(
        &self,
        previous: &str,
    ) -> Result<Option<CitationEdit>, ClientError> {
        let Some(open) = self.snapshot() else {
            return Ok(None);
        };
        let dismissal = resolve_assistant_citation_comment_dismissal(
            "none",
            open.draft.as_deref(),
            open.citation.comment.as_ref().and_then(Option::as_deref),
        );
        if let CitationCommentDismissal::Commit(comment) = dismissal {
            let source = Self::saved_source(&open, &comment)?;
            let edit = self.replace(&open, previous, &source, false)?;
            let mut state = self.state.borrow_mut();
            state.open = None;
            state.changes.notify();
            return Ok(Some(edit));
        }
        Ok(None)
    }
}
