//! Pending user-input answer policy shared by all editor widgets.
use std::collections::BTreeMap;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionOption {
    pub label: String,
    pub value: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub id: String,
    pub options: Vec<QuestionOption>,
    pub multi_select: bool,
    pub allow_custom_answer: bool,
    pub initial_answer: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DraftAnswer {
    pub selected_option_values: Vec<String>,
    pub custom_answer: Option<String>,
    pub attachment_count: usize,
    pub attachments_blocked: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Text(String),
    Choices(Vec<String>),
}
fn trim(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}
fn normalized(value: Option<&str>) -> Option<&str> {
    value.map(trim).filter(|v| !v.is_empty())
}
fn option_value(question: &Question, value: &str) -> Option<String> {
    if question
        .options
        .iter()
        .any(|o| o.value.as_deref() == Some(value))
    {
        return Some(value.into());
    }
    let label = trim(value);
    (!label.is_empty()
        && question
            .options
            .iter()
            .any(|o| o.value.is_none() && trim(&o.label) == label))
    .then(|| label.into())
}
pub fn normalize_selected(question: &Question, values: &[String]) -> Vec<String> {
    let mut result = vec![];
    for value in values {
        if let Some(value) = option_value(question, value)
            && !result.contains(&value)
        {
            result.push(value);
        }
    }
    result
}
pub fn resolve_answer(question: &Question, draft: Option<&DraftAnswer>) -> Option<Answer> {
    if draft.is_some_and(|d| d.attachments_blocked) {
        return None;
    }
    let custom = if !question.allow_custom_answer {
        None
    } else if question.initial_answer.is_some() {
        draft.and_then(|d| d.custom_answer.as_deref())
    } else {
        normalized(draft.and_then(|d| d.custom_answer.as_deref()))
    };
    if let Some(custom) = custom.filter(|s| !s.is_empty()) {
        return Some(Answer::Text(custom.into()));
    }
    let selected = normalize_selected(
        question,
        draft.map_or(&[], |d| d.selected_option_values.as_slice()),
    );
    if question.multi_select && !selected.is_empty() {
        return Some(Answer::Choices(selected));
    }
    if !question.multi_select
        && let Some(first) = selected.first()
    {
        return Some(Answer::Text(first.clone()));
    }
    custom.map(|s| Answer::Text(s.into())).or_else(|| {
        (question.allow_custom_answer && draft.is_some_and(|d| d.attachment_count > 0))
            .then(|| Answer::Text(String::new()))
    })
}
pub fn set_custom_answer(question: &Question, draft: &DraftAnswer, text: String) -> DraftAnswer {
    if !question.allow_custom_answer {
        return draft.clone();
    }
    let selected = if question.initial_answer.is_some() || !trim(&text).is_empty() {
        vec![]
    } else {
        normalize_selected(question, &draft.selected_option_values)
    };
    DraftAnswer {
        custom_answer: Some(text),
        selected_option_values: selected,
        ..DraftAnswer::default()
    }
}
pub fn option_selected(question: &Question, draft: &DraftAnswer, value: &str) -> bool {
    if question.allow_custom_answer && normalized(draft.custom_answer.as_deref()).is_some() {
        return false;
    }
    option_value(question, value)
        .is_some_and(|v| normalize_selected(question, &draft.selected_option_values).contains(&v))
}
pub fn toggle_option(question: &Question, draft: &DraftAnswer, value: &str) -> DraftAnswer {
    let Some(value) = option_value(question, value) else {
        return draft.clone();
    };
    let selected = if question.multi_select {
        let mut selected = normalize_selected(question, &draft.selected_option_values);
        if selected.contains(&value) {
            selected.retain(|v| v != &value);
        } else {
            selected.push(value);
        }
        selected
    } else {
        vec![value]
    };
    DraftAnswer {
        custom_answer: Some(String::new()),
        selected_option_values: selected,
        ..DraftAnswer::default()
    }
}
pub fn build_answers(
    questions: &[Question],
    drafts: &BTreeMap<String, DraftAnswer>,
) -> Option<BTreeMap<String, Answer>> {
    questions
        .iter()
        .map(|q| resolve_answer(q, drafts.get(&q.id)).map(|a| (q.id.clone(), a)))
        .collect()
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn question(native: bool) -> Question {
        Question {
            id: "scope".into(),
            options: if native {
                vec![
                    QuestionOption {
                        label: "Result".into(),
                        value: Some(" first\t".into()),
                    },
                    QuestionOption {
                        label: "Result".into(),
                        value: Some("second".into()),
                    },
                ]
            } else {
                vec![
                    QuestionOption {
                        label: "Orchestration-first".into(),
                        value: None,
                    },
                    QuestionOption {
                        label: "Node.js".into(),
                        value: None,
                    },
                ]
            },
            multi_select: false,
            allow_custom_answer: !native,
            initial_answer: None,
        }
    }
    #[test]
    fn literal_initial_answers() {
        let mut q = question(false);
        q.initial_answer = Some("  Proposed message\n".into());
        for value in ["  Proposed message\n", "  Edited message\n\n", "", " \n"] {
            assert_eq!(
                resolve_answer(
                    &q,
                    Some(&DraftAnswer {
                        custom_answer: Some(value.into()),
                        ..DraftAnswer::default()
                    })
                ),
                Some(Answer::Text(value.into()))
            );
        }
        let d = set_custom_answer(
            &q,
            &DraftAnswer {
                selected_option_values: vec!["Orchestration-first".into()],
                ..DraftAnswer::default()
            },
            "".into(),
        );
        assert!(d.selected_option_values.is_empty());
    }
    #[test]
    fn custom_and_native_choices() {
        let d = DraftAnswer {
            selected_option_values: vec!["second".into()],
            custom_answer: Some(" use another ".into()),
            ..DraftAnswer::default()
        };
        assert_eq!(
            resolve_answer(&question(true), Some(&d)),
            Some(Answer::Text("second".into()))
        );
        assert_eq!(
            resolve_answer(&question(false), Some(&d)),
            Some(Answer::Text("use another".into()))
        );
        assert_eq!(
            resolve_answer(
                &question(true),
                Some(&DraftAnswer {
                    selected_option_values: vec!["Result".into(), "unknown".into()],
                    ..DraftAnswer::default()
                })
            ),
            None
        );
        assert_eq!(
            resolve_answer(
                &question(false),
                Some(&DraftAnswer {
                    selected_option_values: vec![" Node.js ".into()],
                    ..DraftAnswer::default()
                })
            ),
            Some(Answer::Text("Node.js".into()))
        );
    }
    #[test]
    fn attachment_only_and_blocked() {
        let mut d = DraftAnswer {
            attachment_count: 1,
            ..DraftAnswer::default()
        };
        assert_eq!(
            resolve_answer(&question(false), Some(&d)),
            Some(Answer::Text("".into()))
        );
        assert_eq!(resolve_answer(&question(true), Some(&d)), None);
        d.selected_option_values.push("Node.js".into());
        assert_eq!(
            resolve_answer(&question(false), Some(&d)),
            Some(Answer::Text("Node.js".into()))
        );
        d.attachments_blocked = true;
        assert_eq!(resolve_answer(&question(false), Some(&d)), None);
    }
    #[test]
    fn same_label_options_toggle_independently() {
        let mut q = question(true);
        q.multi_select = true;
        let first = toggle_option(&q, &DraftAnswer::default(), " first\t");
        assert!(option_selected(&q, &first, " first\t"));
        assert!(!option_selected(&q, &first, "second"));
        let both = toggle_option(&q, &first, "second");
        assert_eq!(
            resolve_answer(&q, Some(&both)),
            Some(Answer::Choices(vec![" first\t".into(), "second".into()]))
        );
        let second = toggle_option(&q, &both, " first\t");
        assert_eq!(
            resolve_answer(&q, Some(&second)),
            Some(Answer::Choices(vec!["second".into()]))
        );
        assert_eq!(toggle_option(&q, &first, "Result"), first);
    }
    #[test]
    fn custom_disallowed() {
        let q = question(true);
        assert_eq!(
            set_custom_answer(&q, &DraftAnswer::default(), "Other".into()),
            DraftAnswer::default()
        );
        assert_eq!(
            resolve_answer(
                &q,
                Some(&DraftAnswer {
                    custom_answer: Some("Other".into()),
                    ..DraftAnswer::default()
                })
            ),
            None
        );
    }
    #[test]
    fn maps_complete_and_special_ids() {
        let mut q = question(false);
        q.id = "__proto__".into();
        let mut q2 = q.clone();
        q2.id = "name".into();
        let d = DraftAnswer {
            selected_option_values: vec!["Orchestration-first".into()],
            ..DraftAnswer::default()
        };
        let drafts = BTreeMap::from([(q.id.clone(), d.clone()), (q2.id.clone(), d)]);
        assert_eq!(
            build_answers(&[q.clone(), q2.clone()], &drafts)
                .unwrap()
                .len(),
            2
        );
        assert!(build_answers(&[q, q2], &BTreeMap::new()).is_none());
    }
}
