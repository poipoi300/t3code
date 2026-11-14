use crate::{
    ClientError,
    view_logic::{JsTrim, context_references::kind_scoped_id},
};
use serde_json::{Value, json};
use t3_contracts::composer_context::{ComposerContextRecord, PreviewAnnotationContextRecord};

fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}
fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key].as_array().map_or(&[], Vec::as_slice)
}
pub(super) fn truncate(text: &str, limit: usize, marker: &str) -> String {
    if text.encode_utf16().count() <= limit {
        return text.into();
    }
    let mut length = 0;
    let prefix = text
        .chars()
        .take_while(|ch| {
            length += ch.len_utf16();
            length <= limit.saturating_sub(marker.encode_utf16().count())
        })
        .collect::<String>();
    format!("{prefix}{marker}")
}
pub(super) fn label(text: &str, fallback: &str) -> String {
    let clean = text
        .replace(['[', ']', '\\'], " ")
        .split(crate::view_logic::is_js_whitespace)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if clean.is_empty() {
        fallback.into()
    } else {
        truncate(
            &clean,
            t3_contracts::composer_context::COMPOSER_CONTEXT_LABEL_MAX_CHARS,
            "",
        )
    }
}
fn element(raw: &Value) -> Option<Value> {
    let page = string(raw, "pageUrl").js_trim();
    let tag = string(raw, "tagName").js_trim().to_lowercase();
    if page.is_empty() || tag.is_empty() {
        return None;
    }
    let optional = |key| {
        raw[key]
            .as_str()
            .map(JsTrim::js_trim)
            .filter(|text| !text.is_empty())
    };
    let frame = raw
        .get("source")
        .filter(|source| !source.is_null())
        .or_else(|| array(raw, "stack").first());
    let source = frame.map(|frame| json!({
        "functionName":frame["functionName"].as_str().map(JsTrim::js_trim).filter(|text| !text.is_empty()),
        "fileName":frame["fileName"].as_str().map(JsTrim::js_trim).filter(|text| !text.is_empty()),
        "lineNumber":frame["lineNumber"], "columnNumber":frame["columnNumber"]
    }));
    let normalized = |key| {
        truncate(
            string(raw, key).replace("\r\n", "\n").trim_matches('\n'),
            4_000,
            "…",
        )
    };
    Some(
        json!({"pageUrl":page,"pageTitle":raw["pageTitle"].as_str().map(JsTrim::js_trim),"tagName":tag,"selector":optional("selector"),"componentName":optional("componentName"),"source":source,"htmlPreview":normalized("htmlPreview"),"styles":normalized("styles")}),
    )
}

pub fn preview_record(annotation: &Value) -> Result<PreviewAnnotationContextRecord, ClientError> {
    let _: t3_contracts::ipc::PreviewAnnotationPayloadSchema =
        serde_json::from_value(annotation.clone())?;
    let mut targets = vec![];
    let mut ids = vec![];
    for target in array(annotation, "elements") {
        if let Some(element) = element(&target["element"]) {
            targets.push(element);
            ids.push(target["id"].clone());
        }
    }
    let summary = [
        (array(annotation, "elements").len(), "selected element"),
        (array(annotation, "regions").len(), "marked region"),
        (array(annotation, "strokes").len(), "drawing"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, noun)| format!("{count} {noun}{}", if count == 1 { "" } else { "s" }))
    .collect::<Vec<_>>()
    .join(", ");
    let mut record = json!({"version":1,"kind":"preview-annotation","contextId":kind_scoped_id("preview-annotation",string(annotation,"id")),"label":label(&super::web_drafts::preview_label(annotation),"preview-annotation"),"annotationId":annotation["id"],"pageUrl":annotation["pageUrl"],"pageTitle":annotation["pageTitle"],"comment":string(annotation,"comment").js_trim(),"targetSummary":summary,"styleChanges":array(annotation,"styleChanges").iter().map(|change|format!("{}: {} → {}",string(change,"property"),if string(change,"previousValue").is_empty(){"(unset)"}else{string(change,"previousValue")},string(change,"value"))).collect::<Vec<_>>(),"styleChangeDetails":array(annotation,"styleChanges")});
    if !targets.is_empty() {
        record["elements"] = json!(targets);
        record["elementIds"] = json!(ids);
    }
    for (field, key) in [("regionCount", "regions"), ("strokeCount", "strokes")] {
        let count = array(annotation, key).len();
        if count > 0 {
            record[field] = json!(count);
        }
    }
    Ok(serde_json::from_value(record)?)
}

pub fn records(
    question_draft: Option<&Value>,
    thread_draft: Option<&Value>,
) -> Result<Vec<ComposerContextRecord>, ClientError> {
    let mut result = vec![];
    if let Some(draft) = question_draft {
        for record in array(draft, "threadContexts") {
            result.push(ComposerContextRecord::Thread(serde_json::from_value(
                record.clone(),
            )?));
        }
    }
    if let Some(draft) = thread_draft {
        for comment in array(draft, "reviewComments") {
            let mut record = comment.clone();
            record["version"] = json!(1);
            record["kind"] = json!("review-comment");
            record["contextId"] = json!(kind_scoped_id("review-comment", string(comment, "id")));
            record["label"] = json!(label(
                &super::web_drafts::review_label(comment),
                "review-comment"
            ));
            for (key, limit) in [
                (
                    "text",
                    t3_contracts::composer_context::COMPOSER_CONTEXT_REVIEW_TEXT_MAX_CHARS,
                ),
                (
                    "diff",
                    t3_contracts::composer_context::COMPOSER_CONTEXT_REVIEW_DIFF_MAX_CHARS,
                ),
            ] {
                record[key] = json!(truncate(string(comment, key), limit, "\n… truncated …"));
            }
            result.push(ComposerContextRecord::ReviewComment(
                serde_json::from_value(record)?,
            ));
        }
        for annotation in array(draft, "previewAnnotations") {
            result.push(ComposerContextRecord::PreviewAnnotation(preview_record(
                annotation,
            )?));
        }
    }
    Ok(result)
}

pub fn project_answer(
    answer: &Value,
    question_draft: Option<&Value>,
    thread_draft: Option<&Value>,
) -> Result<Value, ClientError> {
    use crate::view_logic::session::questions::{
        AnswerContextRecord, project_pending_user_input_answer,
    };
    let records = records(question_draft, thread_draft)?;
    let borrowed = records
        .iter()
        .filter_map(|record| match record {
            ComposerContextRecord::Thread(record) => Some(AnswerContextRecord::Thread(record)),
            ComposerContextRecord::ReviewComment(record) => {
                Some(AnswerContextRecord::Review(record))
            }
            ComposerContextRecord::PreviewAnnotation(record) => {
                Some(AnswerContextRecord::Preview(record))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    Ok(project_pending_user_input_answer(answer, &borrowed))
}

pub fn preview_payload(record: &PreviewAnnotationContextRecord) -> String {
    let value = serde_json::to_value(record).unwrap_or(Value::Null);
    let mut lines = vec![
        format!(
            "page: {}",
            value["pageTitle"]
                .as_str()
                .map(JsTrim::js_trim)
                .filter(|title| !title.is_empty())
                .unwrap_or(string(&value, "pageUrl"))
        ),
        format!("url: {}", string(&value, "pageUrl")),
    ];
    if !record.comment.js_trim().is_empty() {
        lines.push(format!("comment: {}", record.comment.js_trim()));
    }
    if !string(&value, "targetSummary").is_empty() {
        lines.push(format!("targets: {}", string(&value, "targetSummary")));
    }
    if !record.style_changes.is_empty() {
        lines.push("requested visual changes:".into());
        lines.extend(
            array(&value, "styleChanges")
                .iter()
                .map(|change| format!("- {}", change.as_str().unwrap_or_default())),
        );
    }
    if let Some(id) = &record.screenshot_context_id {
        lines.push(format!("screenshot: ref={}", id.0));
    }
    for (index, element) in array(&value, "elements").iter().enumerate() {
        let mut details = vec![
            format!("url: {}", string(element, "pageUrl")),
            format!("tag: {}", string(element, "tagName")),
        ];
        for (key, label) in [
            ("pageTitle", "title"),
            ("selector", "selector"),
            ("componentName", "component"),
        ] {
            if let Some(text) = element[key].as_str().filter(|text| !text.is_empty()) {
                details.push(format!("{label}: {text}"));
            }
        }
        let source = &element["source"];
        if let Some(file) = source["fileName"].as_str().filter(|file| !file.is_empty()) {
            let mut location = file.to_owned();
            if !source["lineNumber"].is_null() {
                location.push_str(&format!(":{}", source["lineNumber"]));
                if !source["columnNumber"].is_null() {
                    location.push_str(&format!(":{}", source["columnNumber"]));
                }
            }
            details.push(format!("source: {location}"));
        }
        for (key, label) in [("htmlPreview", "html"), ("styles", "styles")] {
            let text = string(element, key).js_trim();
            if !text.is_empty() {
                details.push(format!(
                    "{label}:\n{}",
                    text.split('\n')
                        .map(|line| format!("  {line}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }
        }
        lines.push(format!(
            "element {}:\n{}",
            index + 1,
            details
                .join("\n")
                .split('\n')
                .map(|line| format!("  {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    lines.join("\n")
}
