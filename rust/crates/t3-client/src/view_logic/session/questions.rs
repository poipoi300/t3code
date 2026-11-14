use crate::{
    thread_views::{
        questions::{self, Answer, DraftAnswer, Question},
        requests::encode_uri_component,
    },
    types::ThreadKey,
    view_logic::JsTrim,
};
use serde_json::Value;
use std::{collections::BTreeMap, rc::Rc};
use t3_contracts::composer_context::{PreviewAnnotationContextRecord, ReviewCommentContextRecord, ThreadContextRecord};

pub enum AnswerContextRecord<'a> {
    Thread(&'a ThreadContextRecord),
    Review(&'a ReviewCommentContextRecord),
    Preview(&'a PreviewAnnotationContextRecord),
}
pub fn thread_contexts_referenced_by_answer<'a>(
    answer: &str,
    records: &'a [ThreadContextRecord],
) -> Vec<&'a ThreadContextRecord> {
    let references = crate::view_logic::context_references::collect_references(answer);
    records
        .iter()
        .filter(|record| {
            references
                .iter()
                .any(|r| r.context_id == record.context_id.0)
        })
        .collect()
}
pub fn project_pending_user_input_answer(
    answer: &Value,
    records: &[AnswerContextRecord<'_>],
) -> Value {
    let Some(text) = answer.as_str().filter(|_| !records.is_empty()) else {
        return answer.clone();
    };
    let mut output = String::new();
    let mut cursor = 0;
    for reference in crate::view_logic::context_references::collect_references(text) {
        output.push_str(&text[cursor..reference.start]);
        let source = &text[reference.start..reference.end];
        let record = records.iter().rev().find(|record| match record {
            AnswerContextRecord::Thread(record) => record.context_id.0 == reference.context_id,
            AnswerContextRecord::Review(record) => record.context_id.0 == reference.context_id,
            AnswerContextRecord::Preview(record) => record.context_id.0 == reference.context_id,
        });
        match record {
            Some(AnswerContextRecord::Thread(record)) if reference.kind == "thread" => {
                let label = source
                    .trim_start_matches('!')
                    .strip_prefix('[')
                    .and_then(|s| s.split_once("]("))
                    .map_or("", |(label, _)| label);
                let label = label
                    .replace(['[', ']', '\\', '\r', '\n'], " ")
                    .split(crate::view_logic::is_js_whitespace)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                let href = format!(
                    "t3-thread://v2/{}/{}",
                    record.environment_id.0, record.thread_id.0
                );
                let destination = if href.contains(['(', ')']) {
                    format!("<{href}>")
                } else {
                    href.clone()
                };
                output.push_str(&if label.is_empty() {
                    format!("<{href}>")
                } else {
                    format!("[{label}]({destination})")
                });
            }
            Some(AnswerContextRecord::Review(record)) if reference.kind == "review-comment" => {
                let value = super::work::wire_value(record);
                let string = |key: &str| value[key].as_str().unwrap_or_default();
                let indent = |s: &str| {
                    s.split('\n')
                        .map(|line| format!("  {line}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                let mut lines = vec![
                    format!("file: {}", string("filePath")),
                    format!(
                        "range: {} ({}-{})",
                        string("rangeLabel"),
                        value["startIndex"],
                        value["endIndex"]
                    ),
                    format!("section: {}", string("sectionTitle")),
                ];
                if !record.text.js_trim().is_empty() {
                    lines.extend(["comment:".into(), indent(record.text.js_trim())]);
                }
                if !record.diff.js_trim().is_empty() {
                    lines.extend([
                        format!("{}:", record.fence_language.as_deref().unwrap_or("diff")),
                        indent(record.diff.trim_end_matches('\n')),
                    ]);
                }
                let label = source
                    .trim_start_matches('!')
                    .strip_prefix('[')
                    .and_then(|s| s.split_once("]("))
                    .map_or("", |(label, _)| label);
                let label = label
                    .replace(['\r', '\n', ';', ']'], " ")
                    .split(crate::view_logic::is_js_whitespace)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                let escape = crate::view_logic::context_references::escape_provider_context_payload;
                output.push_str(&format!("[Review comment: {}; ref={}]\n\n<t3_context version=\"1\">\n<context kind=\"review-comment\" id=\"{}\">\n{}\n</context>\n</t3_context>",escape(&label),record.context_id.0,record.context_id.0,escape(&lines.join("\n"))));
            }
            Some(AnswerContextRecord::Preview(record)) if reference.kind == "preview-annotation" => {
                let label = source.trim_start_matches('!').strip_prefix('[').and_then(|text| text.split_once("](")).map_or("", |(label,_)| label);
                let label = label.replace(['\r', '\n', ';', ']'], " ").split(crate::view_logic::is_js_whitespace).filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" ");
                let escape = crate::view_logic::context_references::escape_provider_context_payload;
                let payload = crate::thread_views::composer::answer_context::preview_payload(record);
                output.push_str(&format!("[Preview annotation: {}; ref={}]\n\n<t3_context version=\"1\">\n<context kind=\"preview-annotation\" id=\"{}\">\n{}\n</context>\n</t3_context>",escape(&label),record.context_id.0,record.context_id.0,escape(&payload)));
            }
            _ => output.push_str(source),
        }
        cursor = reference.end;
    }
    output.push_str(&text[cursor..]);
    Value::String(output)
}

pub fn carry_displaced_custom_answer(prompt: &str, custom: Option<&str>) -> String {
    let answer = custom.unwrap_or_default().js_trim();
    if answer.is_empty() {
        prompt.into()
    } else if prompt.js_trim().is_empty() {
        answer.into()
    } else {
        format!("{}\n\n{answer}", prompt.js_trim_end())
    }
}
pub fn withdraw_displaced_custom_answer(prompt: &str, displaced: &str, later: &[String]) -> String {
    let text = displaced.js_trim();
    if text.is_empty() {
        return prompt.into();
    }
    let later: Vec<_> = later
        .iter()
        .map(|s| s.js_trim())
        .filter(|s| !s.is_empty())
        .collect();
    let after = later.join("\n\n");
    let suffix = std::iter::once(text)
        .chain(later)
        .collect::<Vec<_>>()
        .join("\n\n");
    let Some(before) = prompt.strip_suffix(&suffix) else {
        return prompt.into();
    };
    if before.is_empty() {
        return after;
    }
    let Some(before_without_separator) = before.strip_suffix("\n\n") else {
        return prompt.into();
    };
    if after.is_empty() {
        before_without_separator.into()
    } else {
        format!("{before}{after}")
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct PendingUserInputProgress<'a> {
    pub question_index: usize,
    pub active_question: Option<&'a Question>,
    pub active_draft: Option<&'a DraftAnswer>,
    pub selected_option_values: Vec<String>,
    pub custom_answer: &'a str,
    pub resolved_answer: Option<Answer>,
    pub using_custom_answer: bool,
    pub answered_question_count: usize,
    pub is_last_question: bool,
    pub is_complete: bool,
    pub can_advance: bool,
}
pub fn pending_user_input_progress<'a>(
    questions: &'a [Question],
    drafts: &'a BTreeMap<String, DraftAnswer>,
    index: i64,
) -> PendingUserInputProgress<'a> {
    let index = (index.max(0) as usize).min(questions.len().saturating_sub(1));
    let question = questions.get(index);
    let draft = question.and_then(|q| drafts.get(&q.id));
    let answer = question.and_then(|q| questions::resolve_answer(q, draft));
    let custom = question
        .filter(|q| q.allow_custom_answer)
        .and(draft)
        .and_then(|d| d.custom_answer.as_deref())
        .unwrap_or_default();
    PendingUserInputProgress {
        question_index: index,
        active_question: question,
        active_draft: draft,
        selected_option_values: question.map_or_else(Vec::new, |q| {
            questions::normalize_selected(
                q,
                draft.map_or(&[], |d| d.selected_option_values.as_slice()),
            )
        }),
        custom_answer: custom,
        using_custom_answer: !if question.is_some_and(|q| q.initial_answer.is_some()) {
            custom
        } else {
            custom.js_trim()
        }
        .is_empty(),
        answered_question_count: questions
            .iter()
            .filter(|q| questions::resolve_answer(q, drafts.get(&q.id)).is_some())
            .count(),
        is_last_question: questions.is_empty() || index >= questions.len() - 1,
        is_complete: questions::build_answers(questions, drafts).is_some(),
        can_advance: answer.is_some(),
        resolved_answer: answer,
    }
}
pub fn question_attachment_draft_prefix(key: &ThreadKey) -> String {
    format!(
        "{}:question-{}-",
        encode_uri_component(&serde_json::Value::String(key.environment.0.clone()).to_string()),
        encode_uri_component(&serde_json::Value::String(key.thread.0.clone()).to_string())
    )
}
pub fn question_attachment_draft_id(key: &ThreadKey, request: &str, question: &str) -> String {
    format!(
        "{}{}",
        question_attachment_draft_prefix(key),
        encode_uri_component(&serde_json::json!([request, question]).to_string())
    )
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenQuestionAttachmentDraft {
    pub draft_id: String,
    pub request_keys: Vec<String>,
}
#[derive(Default)]
pub struct QuestionAttachmentState {
    pub preparation_counts: BTreeMap<String, usize>,
    open: BTreeMap<ThreadKey, Rc<OpenQuestionAttachmentDraft>>,
}

#[derive(Default)]
pub struct ReturnedQuestionAnswers {
    pub prompt: String,
    pub thread_contexts: Vec<ThreadContextRecord>,
}
pub fn return_unsent_answers_to_thread_draft<'a>(
    prompt: &str,
    answers: impl IntoIterator<Item = (Option<&'a str>, &'a [ThreadContextRecord])>,
) -> ReturnedQuestionAnswers {
    let mut result = ReturnedQuestionAnswers {
        prompt: prompt.into(),
        thread_contexts: vec![],
    };
    for (custom, records) in answers {
        let next = carry_displaced_custom_answer(&result.prompt, custom);
        if next == result.prompt {
            continue;
        }
        result.prompt = next;
        result.thread_contexts.extend(
            thread_contexts_referenced_by_answer(custom.unwrap_or_default(), records)
                .into_iter()
                .cloned(),
        );
    }
    result
}
impl QuestionAttachmentState {
    pub fn track(&mut self, key: ThreadKey, draft: Rc<OpenQuestionAttachmentDraft>) {
        self.open.insert(key, draft);
    }
    pub fn untrack(&mut self, key: &ThreadKey, draft: &Rc<OpenQuestionAttachmentDraft>) {
        if self
            .open
            .get(key)
            .is_some_and(|current| Rc::ptr_eq(current, draft))
        {
            self.open.remove(key);
        }
    }
    pub fn open(&self, key: &ThreadKey) -> Option<&Rc<OpenQuestionAttachmentDraft>> {
        self.open.get(key)
    }
    pub fn change_preparation(&mut self, key: &str, delta: i64) {
        if delta < 0 && !self.preparation_counts.contains_key(key) {
            return;
        }
        let count = self.preparation_counts.entry(key.into()).or_default();
        *count = count.saturating_add_signed(delta as isize);
    }
    pub fn count(&self, keys: &[String], mut attachments: impl FnMut(&str) -> usize) -> usize {
        keys.iter()
            .map(|key| {
                attachments(key)
                    + self
                        .preparation_counts
                        .get(key)
                        .copied()
                        .unwrap_or_default()
            })
            .sum()
    }
    pub fn count_request(
        &self,
        draft_id: &str,
        attachments: impl FnMut(&str) -> usize,
    ) -> Option<usize> {
        self.open
            .values()
            .find(|draft| draft.draft_id == draft_id)
            .map(|draft| self.count(&draft.request_keys, attachments))
    }
    pub fn clear(&mut self, key: &str) {
        self.preparation_counts.remove(key);
    }
}
