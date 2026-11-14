//! Port of apps/web/src/lib/assistantTextSelection.ts `captureAssistantMessageOpening` and
//! `createAssistantTextSelector`, lib/composerContextRecords.ts `responseContextRecord`, and
//! ChatComposer.tsx `replyToResponse` / the reply banner's `setReplyTo(…, null)`.
use t3_contracts::assistant_citations::{ASSISTANT_CITATION_CONTEXT_LENGTH, AssistantCitation};
use t3_contracts::base_schemas::{MessageId, TurnItemId};
use t3_contracts::composer_context::{
    ComposerContextId, ResponseContextRecord, ResponseContextRecordKind,
    ResponseContextRecordSubagents, ResponseContextRecordVersion,
};

use crate::{ClientError, composition::ClientRuntime, platform::Platform, types::ThreadKey};

/// How much of a response a reply carries: enough to recognise it, not to resend it.
pub const RESPONSE_OPENING_LENGTH: usize = 300;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssistantTextSelector {
    pub text: String,
    pub start: usize,
    pub end: usize,
    pub prefix: String,
    pub suffix: String,
}

fn whitespace(unit: u16) -> bool {
    char::from_u32(u32::from(unit)).is_some_and(crate::view_logic::is_js_whitespace)
}

fn normalize_whitespace(units: &[u16]) -> Vec<u16> {
    let mut normalized = Vec::with_capacity(units.len());
    let mut in_run = false;
    for &unit in units {
        if whitespace(unit) {
            if !in_run {
                normalized.push(u16::from(b' '));
            }
            in_run = true;
        } else {
            normalized.push(unit);
            in_run = false;
        }
    }
    normalized
}

fn splits_surrogate_pair(units: &[u16], offset: usize) -> bool {
    offset > 0
        && offset < units.len()
        && (0xd800..=0xdbff).contains(&units[offset - 1])
        && (0xdc00..=0xdfff).contains(&units[offset])
}

/// Offsets are UTF-16 units of `text`, as the web's string indices are.
pub fn create_assistant_text_selector(
    text: &str,
    raw_start: usize,
    raw_end: usize,
) -> Option<AssistantTextSelector> {
    let units = text.encode_utf16().collect::<Vec<_>>();
    let quote = units.get(raw_start..raw_end)?;
    if quote.iter().all(|unit| whitespace(*unit)) {
        return None;
    }
    let normalized = normalize_whitespace(&units);
    let mut start = normalize_whitespace(&units[..raw_start]).len();
    if raw_start > 0 && whitespace(units[raw_start - 1]) && whitespace(units[raw_start]) {
        start -= 1;
    }
    let end = normalize_whitespace(&units[..raw_end]).len();
    let mut prefix_start = start.saturating_sub(ASSISTANT_CITATION_CONTEXT_LENGTH);
    let mut suffix_end = normalized
        .len()
        .min(end + ASSISTANT_CITATION_CONTEXT_LENGTH);
    if splits_surrogate_pair(&normalized, prefix_start) {
        prefix_start += 1;
    }
    if splits_surrogate_pair(&normalized, suffix_end) {
        suffix_end -= 1;
    }
    Some(AssistantTextSelector {
        text: String::from_utf16_lossy(quote),
        start,
        end,
        prefix: String::from_utf16_lossy(&normalized[prefix_start..start]),
        suffix: String::from_utf16_lossy(&normalized[end..suffix_end]),
    })
}

/// The opening of a response. Rust rows have no rendered DOM text to read, so this is always the
/// web's fallback branch over the message's own text.
pub fn capture_assistant_message_opening(text: &str) -> Option<AssistantTextSelector> {
    let units = text.encode_utf16().collect::<Vec<_>>();
    let leading = units.iter().take_while(|unit| whitespace(**unit)).count();
    let trailing = units[leading..]
        .iter()
        .rev()
        .take_while(|unit| whitespace(**unit))
        .count();
    let raw_start = leading;
    let mut raw_end = (units.len() - trailing).min(raw_start + RESPONSE_OPENING_LENGTH);
    if splits_surrogate_pair(&units, raw_end) {
        raw_end -= 1;
    }
    create_assistant_text_selector(text, raw_start, raw_end)
}

pub struct ResponseSubagent {
    pub thread_id: t3_contracts::base_schemas::ThreadId,
    pub title: String,
    pub environment_id: Option<t3_contracts::base_schemas::EnvironmentId>,
}

/// One record per response, so replying to it again reuses the same reference.
pub fn response_context_record(
    citation: AssistantCitation,
    item_id: TurnItemId,
    subagents: Vec<ResponseSubagent>,
) -> ResponseContextRecord {
    let opening = citation.text.encode_utf16().take(64).collect::<Vec<_>>();
    ResponseContextRecord {
        version: ResponseContextRecordVersion,
        context_id: ComposerContextId(crate::view_logic::context_references::kind_scoped_id(
            "response",
            &citation.message_id.0,
        )),
        label: super::answer_context::label(&String::from_utf16_lossy(&opening), "response"),
        kind: ResponseContextRecordKind::Response,
        item_id,
        subagents: (!subagents.is_empty()).then(|| {
            Some(
                subagents
                    .into_iter()
                    .take(20)
                    .map(|subagent| ResponseContextRecordSubagents {
                        thread_id: subagent.thread_id,
                        title: super::answer_context::label(&subagent.title, "response"),
                        environment_id: subagent.environment_id.map(Some),
                    })
                    .collect(),
            )
        }),
        citation,
    }
}

/// `ChatComposer.replyToResponse`: false when an open question owns the composer, since its
/// answer would not carry the reply. Writes the thread draft's reply, not a question draft.
pub fn reply_to_response<P: Platform + Clone + 'static>(
    runtime: &ClientRuntime<P>,
    key: &ThreadKey,
    message_id: &MessageId,
    message_text: &str,
    item_id: TurnItemId,
) -> Result<bool, ClientError> {
    if runtime.pending_user_input(key).view().is_some() {
        return Ok(false);
    }
    let Some(opening) = capture_assistant_message_opening(message_text) else {
        return Ok(false);
    };
    let citation = AssistantCitation {
        version: 1,
        environment_id: key.environment.clone(),
        thread_id: key.thread.clone(),
        message_id: message_id.clone(),
        text: opening.text,
        comment: None,
        start: opening.start as u64,
        end: opening.end as u64,
        prefix: opening.prefix,
        suffix: opening.suffix,
    };
    // readResponseSubagents is not ported yet: replies carry no subagent list (the web's empty case).
    let record = response_context_record(citation, item_id, Vec::new());
    set_reply_to(runtime, key, Some(&record))?;
    Ok(true)
}

/// The reply banner's dismissal (`setReplyTo(target, null)`).
pub fn set_reply_to<P: Platform + Clone + 'static>(
    runtime: &ClientRuntime<P>,
    key: &ThreadKey,
    record: Option<&ResponseContextRecord>,
) -> Result<bool, ClientError> {
    let drafts = runtime.composer_drafts();
    let draft_key = drafts.thread_draft_key(key);
    drafts.update(|document| {
        super::web_context::set_reply(document, &draft_key, record);
        Ok(())
    })
}

/// The thread draft's current reply, read for the composer's reply banner.
pub fn reply_to<P: Platform + Clone + 'static>(
    runtime: &ClientRuntime<P>,
    key: &ThreadKey,
) -> Option<ResponseContextRecord> {
    let drafts = runtime.composer_drafts();
    let draft_key = drafts.thread_draft_key(key);
    let document = drafts.document();
    let document = document.borrow();
    let raw = document["draftsByThreadKey"].get(&draft_key)?.get("replyTo")?;
    serde_json::from_value(raw.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_opening_skips_outer_whitespace_and_stops_at_three_hundred_units() {
        let text = format!("  \n{}  ", "a".repeat(400));
        let opening = capture_assistant_message_opening(&text).expect("opening");
        assert_eq!(opening.text.len(), RESPONSE_OPENING_LENGTH);
        assert_eq!((opening.start, opening.end), (1, 301));
        assert_eq!(opening.prefix, " ");
        assert_eq!(opening.suffix, "a".repeat(32));
    }

    #[test]
    fn a_whitespace_only_reply_has_no_opening() {
        assert_eq!(capture_assistant_message_opening(" \n\t "), None);
    }

    #[test]
    fn the_opening_never_splits_a_surrogate_pair() {
        let text = format!("{}😀tail", "a".repeat(299));
        let opening = capture_assistant_message_opening(&text).expect("opening");
        assert_eq!(opening.text, "a".repeat(299));
    }

    #[test]
    fn selectors_count_normalized_whitespace_like_the_web() {
        // createAssistantTextSelector("one  two\n\nthree", 5, 8): runs collapse to one space.
        let selector = create_assistant_text_selector("one  two\n\nthree", 5, 8).expect("selector");
        assert_eq!(selector.text, "two");
        assert_eq!((selector.start, selector.end), (4, 7));
        assert_eq!(selector.prefix, "one ");
        assert_eq!(selector.suffix, " three");
    }

    #[test]
    fn the_record_is_keyed_by_message_and_labelled_by_its_opening() {
        let citation = AssistantCitation {
            version: 1,
            environment_id: t3_contracts::base_schemas::EnvironmentId("env".into()),
            thread_id: t3_contracts::base_schemas::ThreadId("thread".into()),
            message_id: MessageId("message".into()),
            text: format!("[Plan] {}", "b".repeat(80)),
            comment: None,
            start: 0,
            end: 87,
            prefix: String::new(),
            suffix: String::new(),
        };
        let record = response_context_record(citation, TurnItemId("item".into()), Vec::new());
        assert_eq!(record.context_id.0, "response_message");
        assert!(record.label.starts_with("Plan b"));
        assert_eq!(record.subagents, None);
    }
}
